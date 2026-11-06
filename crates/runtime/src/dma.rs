//! DMA channels 0–3: immediate, V-blank and H-blank starts, halfword and
//! word units, address control, repeat, and the IRQ at the end. The special
//! start runs channels 1 and 2 aimed at a sound FIFO when the FIFO asks
//! ([`fifo_refill`], called by [`crate::sound`]); video capture (channel 3)
//! and a special DMA1/2 aimed anywhere else never run.
//!
//! A transfer runs whole as soon as it starts, with the CPU stopped, as on
//! hardware, including when that's in the middle of a store to its control
//! register ([`Bus`] calls [`control_written`]). Its cost (2 cycles a unit
//! plus 2, and both regions' wait states, [`crate::waits::Waits::dma`]) is
//! returned to the caller to add to the clock.

use crate::bus::Bus;
use crate::io::{irq, reg};
use crate::sound::FIFO_ADDR;

/// `DMAxCNT_H` bits.
pub mod ctl {
    /// Destination control: 0 increment, 1 decrement, 2 fixed, 3 increment
    /// and reload on repeat.
    pub const DST_SHIFT: u16 = 5;
    /// Source control: 0 increment, 1 decrement, 2 fixed (3 is prohibited;
    /// treated as increment).
    pub const SRC_SHIFT: u16 = 7;
    pub const REPEAT: u16 = 1 << 9;
    /// 32-bit units (else 16-bit).
    pub const WORD: u16 = 1 << 10;
    /// Start timing: 0 immediate, 1 V-blank, 2 H-blank, 3 special.
    pub const TIMING_SHIFT: u16 = 12;
    pub const IRQ: u16 = 1 << 14;
    pub const ENABLE: u16 = 1 << 15;
}

/// When a channel starts, from `DMAxCNT_H`'s timing field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Immediate,
    VBlank,
    HBlank,
    Special,
}

impl Timing {
    fn of(control: u16) -> Timing {
        [
            Timing::Immediate,
            Timing::VBlank,
            Timing::HBlank,
            Timing::Special,
        ][(control >> ctl::TIMING_SHIFT & 3) as usize]
    }
}

/// One channel's internal registers: latched from the I/O registers when
/// the channel is enabled, and walked by its transfers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub src: u32,
    pub dst: u32,
    /// Units left to the transfer (or per repeat).
    pub count: u32,
    /// `DMAxCNT_H` as last seen.
    pub control: u16,
    /// A transfer is running.
    pub busy: bool,
    /// Switched on (and latched) since the running transfer began.
    pub relatched: bool,
}

/// Channel `n`'s first register.
fn base(n: usize) -> u32 {
    reg::DMA0SAD + 12 * n as u32
}

fn read32(bus: &Bus, o: u32) -> u32 {
    bus.io.raw16(o) as u32 | (bus.io.raw16(o + 2) as u32) << 16
}

/// The unit count from `DMAxCNT_L`, where 0 means the maximum.
pub(crate) fn count(bus: &Bus, n: usize) -> u32 {
    match bus.io.raw16(base(n) + 8) as u32 {
        0 if n == 3 => 0x1_0000,
        0 => 0x4000,
        c => c,
    }
}

/// Channel `n`'s control was written. A channel switching on latches its
/// addresses and count, and an immediate one runs now. Returns the cycles
/// spent.
pub fn control_written(bus: &mut Bus, n: usize) -> u32 {
    let control = bus.io.raw16(base(n) + 10);
    let ch = &mut bus.dma[n];
    let was_on = ch.control & ctl::ENABLE != 0;
    ch.control = control;
    if control & ctl::ENABLE == 0 || was_on {
        return 0;
    }
    let (src, dst, count) = (
        read32(bus, base(n)),
        read32(bus, base(n) + 4),
        count(bus, n),
    );
    let ch = &mut bus.dma[n];
    (ch.src, ch.dst, ch.count) = (src, dst, count);
    ch.relatched = true;
    let busy = ch.busy;
    if let Some(f) = feeds(bus, n) {
        let now = bus.io.now;
        bus.sound.restarted(f, now);
    }
    if busy {
        // Re-armed by its own transfer, which picks this up when it ends.
        return 0;
    }
    if Timing::of(control) == Timing::Immediate {
        transfer(bus, n)
    } else {
        0
    }
}

/// The sound FIFO channel `n` feeds, if it's an enabled DMA1 or DMA2 with
/// the special start and a FIFO as its latched destination.
fn feeds(bus: &Bus, n: usize) -> Option<usize> {
    let ch = &bus.dma[n];
    if !(1..=2).contains(&n)
        || ch.control & ctl::ENABLE == 0
        || Timing::of(ch.control) != Timing::Special
    {
        return None;
    }
    FIFO_ADDR.iter().position(|&a| ch.dst & !3 == a)
}

/// What a FIFO refill costs the clock: 4 words, 2 cycles each, and 2.
/// Native programs' FIFO DMA ([`crate::hw::Sound`]) charges the same.
pub const FIFO_REFILL_CYCLES: u64 = 2 * 4 + 2;

/// Source address step of a FIFO refill under `DMAxCNT_H` `control`: up,
/// down or fixed (prohibited 3 counts up).
pub fn fifo_step(control: u16) -> u32 {
    match control >> ctl::SRC_SHIFT & 3 {
        1 => 4u32.wrapping_neg(),
        2 => 0,
        _ => 4,
    }
}

/// Sound FIFO `fifo` popped down to its refill level: run the channel that
/// feeds it, if any (the lower one if both do). Moves exactly 4 words from
/// the source into the FIFO, whatever `DMAxCNT_L` and the destination
/// control say; repeat, the source's address control and the IRQ apply as
/// usual. Returns whether one ran.
pub fn fifo_refill(bus: &mut Bus, fifo: usize) -> bool {
    let Some(n) = (1..=2).find(|&n| feeds(bus, n) == Some(fifo)) else {
        return false;
    };
    let mut ch = bus.dma[n];
    let c = ch.control;
    let step = fifo_step(c);
    #[cfg(feature = "provenance")]
    let _copying = crate::prov::enter_copy(crate::prov::via::DMA);
    for _ in 0..4 {
        let v = bus.read32(ch.src & !3);
        #[cfg(feature = "provenance")]
        crate::prov::fifo_dma_word();
        for b in v.to_le_bytes() {
            bus.sound.fifo[fifo].push(b);
        }
        ch.src = ch.src.wrapping_add(step);
    }
    if c & ctl::IRQ != 0 {
        bus.io.request_irq(irq::DMA0 << n);
    }
    if c & ctl::REPEAT == 0 {
        ch.control &= !ctl::ENABLE;
        let o = base(n) + 10;
        bus.io.set_raw16(o, bus.io.raw16(o) & !ctl::ENABLE);
    }
    bus.dma[n] = ch;
    bus.io.now += FIFO_REFILL_CYCLES;
    true
}

/// Start every enabled channel waiting on `timing`, lowest channel first.
/// Returns the cycles spent.
pub fn trigger(bus: &mut Bus, timing: Timing) -> u32 {
    let mut cycles = 0;
    for n in 0..4 {
        let c = bus.dma[n].control;
        if c & ctl::ENABLE != 0 && Timing::of(c) == timing {
            cycles += transfer(bus, n);
        }
    }
    cycles
}

/// Run channel `n`'s transfer now, whatever its start timing (a pending
/// one from a save state). Returns the cycles spent.
pub fn start(bus: &mut Bus, n: usize) -> u32 {
    transfer(bus, n)
}

/// Run channel `n`'s transfer, then repeat-reload or switch it off.
///
/// A transfer can write its own control register. If that switches the
/// channel back on, the new latch stands, and an immediate one runs when
/// this one ends: in a loop, not by recursion, so a channel that keeps
/// re-arming itself hangs the guest (as it would hardware) rather than
/// overflowing the host's stack.
fn transfer(bus: &mut Bus, n: usize) -> u32 {
    #[cfg(feature = "provenance")]
    let _copying = crate::prov::enter_copy(crate::prov::via::DMA);
    let mut cycles = 0;
    loop {
        bus.dma[n].busy = true;
        bus.dma[n].relatched = false;
        let mut ch = bus.dma[n];
        let c = ch.control;
        let word = c & ctl::WORD != 0;
        let size: u32 = if word { 4 } else { 2 };
        let step = |control: u16| match control & 3 {
            1 => size.wrapping_neg(),
            2 => 0,
            _ => size,
        };
        let dst_ctl = c >> ctl::DST_SHIFT;
        let (src_step, dst_step) = (step(c >> ctl::SRC_SHIFT), step(dst_ctl));
        cycles += bus.waits.dma(size, ch.src, ch.dst, ch.count) + 2;
        // The EEPROM is read and written a bit a halfword, and a write
        // tells the address from the data by the units left (save::Save::
        // eeprom_write).
        let (ee_src, ee_dst) = (bus.eeprom_at(ch.src), bus.eeprom_at(ch.dst));
        #[cfg(feature = "provenance")]
        let start = (ch.src, ch.dst);
        for i in 0..ch.count {
            if word {
                let v = bus.read32(ch.src & !3);
                bus.write32(ch.dst & !3, v);
            } else {
                let v = if ee_src {
                    bus.save.eeprom_read()
                } else {
                    bus.read16(ch.src & !1)
                };
                if ee_dst {
                    bus.save.eeprom_write(v, ch.count - i);
                } else {
                    bus.write16(ch.dst & !1, v);
                }
            }
            ch.src = ch.src.wrapping_add(src_step);
            ch.dst = ch.dst.wrapping_add(dst_step);
        }
        #[cfg(feature = "provenance")]
        if !ee_src && !ee_dst {
            crate::prov::dma_loaded(start.0, start.1, ch.count, size, src_step);
        }
        if c & ctl::IRQ != 0 {
            bus.io.request_irq(irq::DMA0 << n);
        }
        let now = &mut bus.dma[n];
        now.busy = false;
        if now.relatched {
            // Switched off and on again during the transfer.
            if Timing::of(now.control) == Timing::Immediate {
                continue;
            }
            return cycles;
        }
        // Any other write to the control during the transfer was seen
        // already; the addresses are the transfer's.
        ch.control = now.control;
        ch.busy = false;
        if c & ctl::REPEAT != 0 && Timing::of(c) != Timing::Immediate {
            ch.count = count(bus, n);
            if dst_ctl & 3 == 3 {
                ch.dst = read32(bus, base(n) + 4);
            }
        } else {
            ch.control &= !ctl::ENABLE;
            let o = base(n) + 10;
            bus.io.set_raw16(o, bus.io.raw16(o) & !ctl::ENABLE);
        }
        bus.dma[n] = ch;
        return cycles;
    }
}

crate::snap::snap!(Channel {
    src,
    dst,
    count,
    control,
    busy,
    relatched
});

#[cfg(test)]
mod tests;
