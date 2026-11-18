//! DMA by its registers (`DMA0SAD` to `DMA3CNT_H`, offsets `0x0B0` to
//! `0x0DF`): what a lifted program that sets a channel up by address gets,
//! as the play build's [`crate::dma`] does it.
//!
//! The registers are stored in the sound's I/O block (as the play build
//! stores them, through its write masks), so [`Hw::read16`] and
//! [`Hw::raw16`] read them as it does. A store to a channel's control
//! latches it as [`crate::dma::control_written`] does: switched on, its
//! addresses and count are taken; an immediate one is marked to run, which
//! [`write_dma`] (or the next [`super::sync`]) does through the program's
//! memory ([`Native::dma_load`], [`Native::dma_store`]), its price going
//! on the clock with no sync and the sound not counted up to it, as the
//! play build's `io_write` adds it. V-
//! blank and H-blank channels run at the beam's events ([`trigger`]); a
//! special-start DMA1 or DMA2 aimed at a sound FIFO becomes the FIFO's
//! [`super::FifoDma`] ([`Hw::fifo_dma`]). Each transfer requests its IRQ at the
//! end if its control asks.
//!
//! Not here: the EEPROM's bit-serial DMA (the program's memory reads the
//! ROM at `0x0D`), video capture (DMA3's special start, which never runs,
//! as in the play build).

use super::{Hw, Native};
use crate::dma::{Channel, Timing, ctl};
use crate::io::{event, irq, reg};
use crate::sound::FIFO_ADDR;

/// The DMA registers' offsets.
pub const DMA_START: u32 = reg::DMA0SAD;
pub const DMA_END: u32 = reg::DMA0SAD + 4 * 12;

/// The four channels' internal registers, latched from the I/O registers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Channels {
    pub ch: [Channel; 4],
    /// Immediate channels switched on and not yet run, a bit each.
    pending: u8,
}

/// Channel `n`'s first register.
fn base(n: usize) -> u32 {
    reg::DMA0SAD + 12 * n as u32
}

/// The sound FIFO a latched channel feeds: DMA1 or DMA2, enabled, the
/// special start, a FIFO its destination.
fn feeds(n: usize, ch: &Channel) -> Option<usize> {
    if !(1..=2).contains(&n)
        || ch.control & ctl::ENABLE == 0
        || Timing::of(ch.control) != Timing::Special
    {
        return None;
    }
    FIFO_ADDR.iter().position(|&a| ch.dst & !3 == a)
}

/// A FIFO DMA's word, from the program's memory.
fn fifo_word<G: Native>(g: &mut G, a: u32) -> u32 {
    g.dma_load(a, true)
}

impl<G: Native> Hw<G> {
    fn dma_read32(&self, o: u32) -> u32 {
        let io = &self.sound.io;
        io.raw16(o) as u32 | (io.raw16(o + 2) as u32) << 16
    }

    /// The unit count from `DMAxCNT_L`, where 0 means the maximum.
    fn dma_count(&self, n: usize) -> u32 {
        match self.sound.io.raw16(base(n) + 8) as u32 {
            0 if n == 3 => 0x1_0000,
            0 => 0x4000,
            c => c,
        }
    }

    /// A store of `bytes` to a DMA register, as the play build's bus makes
    /// it: through the sound path (the overflows due before a DMA1 or DMA2
    /// store walked, the write masks), then each control written latched.
    pub(super) fn dma_store(&mut self, o: u32, bytes: u32, v: u32) {
        crate::sound::store(&mut self.sound.store(), o, bytes, v);
        let written = self.sound.io.events & (0xF * event::DMA0);
        self.take_sound_irqs();
        for n in 0..4 {
            if written & (event::DMA0 << n) != 0 {
                self.dma_control_written(n);
            }
        }
    }

    /// Channel `n`'s control was written ([`crate::dma::control_written`]):
    /// switched on, it latches its addresses and count, and an immediate
    /// one is marked to run. A FIFO's channel is linked to the sound.
    fn dma_control_written(&mut self, n: usize) {
        let control = self.sound.io.raw16(base(n) + 10);
        let (src, dst, count) = (
            self.dma_read32(base(n)),
            self.dma_read32(base(n) + 4),
            self.dma_count(n),
        );
        let regs = &mut self.dma.regs;
        let ch = &mut regs.ch[n];
        let was_on = ch.control & ctl::ENABLE != 0;
        ch.control = control;
        let latched = control & ctl::ENABLE != 0 && !was_on;
        if latched {
            (ch.src, ch.dst, ch.count) = (src, dst, count);
            ch.relatched = true;
            if !ch.busy && Timing::of(control) == Timing::Immediate {
                regs.pending |= 1 << n;
            }
        }
        if !(1..=2).contains(&n) {
            return;
        }
        let ch = self.dma.regs.ch[n];
        match feeds(n, &ch) {
            Some(fifo) if latched => self.fifo_dma(n, fifo, ch.src, control, fifo_word::<G>),
            // Still on: the play build's refill reads the new control (a
            // timing turned to special feeds the FIFO from the latched
            // source, with no restart).
            Some(fifo) => match self.sound.dma[n - 1].as_mut() {
                Some(d) => d.control = control,
                None => {
                    self.sound.dma[n - 1] = Some(super::FifoDma {
                        fifo,
                        src: ch.src,
                        control,
                        read: fifo_word::<G>,
                    })
                }
            },
            None => self.fifo_dma_stop(n),
        }
    }

    /// A FIFO refill by channel `n` (1 or 2) moved its source to `src`;
    /// without repeat it switched the channel off.
    pub(super) fn fifo_refilled(&mut self, n: usize, src: u32, control: u16) {
        let ch = &mut self.dma.regs.ch[n];
        ch.src = src;
        if control & ctl::REPEAT == 0 {
            ch.control &= !ctl::ENABLE;
            let o = base(n) + 10;
            let io = &mut self.sound.io;
            io.set_raw16(o, io.raw16(o) & !ctl::ENABLE);
        }
    }

    /// Whether an immediate channel waits to run ([`run_pending`]).
    pub(super) fn dma_pending(&self) -> bool {
        self.dma.regs.pending != 0
    }
}

/// A store of `v` to the DMA register at offset `o` (even) by the program:
/// [`Hw::write16`], then an immediate channel it switched on runs through
/// the program's memory, its price on the clock with no sync (the play
/// build's `io_write`). The prelude's `io_write16` routes the DMA
/// registers here.
pub fn write_dma<G: Native>(g: &mut G, o: u32, v: u16) {
    super::write_io(g, o, v);
    run_pending(g);
}

/// Run each immediate channel switched on and not yet run, lowest first,
/// each one's price on the clock as it ends. The sound isn't brought up to
/// it: as in the play build, the timers and FIFOs are counted at the next
/// sync, or before the next store to their registers ([`super::write_io`]),
/// either of which refills a FIFO through the program's memory.
pub(super) fn run_pending<G: Native>(g: &mut G) {
    loop {
        let regs = &mut g.hw().dma.regs;
        if regs.pending == 0 {
            break;
        }
        let n = regs.pending.trailing_zeros() as usize;
        regs.pending &= !(1 << n);
        let c = transfer(g, n);
        g.hw().sound.io.now += c as u64;
    }
}

/// Start every enabled channel waiting on `timing`, lowest channel first
/// ([`crate::dma::trigger`]). Returns the cycles spent.
pub(super) fn trigger<G: Native>(g: &mut G, timing: Timing) -> u32 {
    let mut cycles = 0;
    for n in 0..4 {
        let c = g.hw().dma.regs.ch[n].control;
        if c & ctl::ENABLE != 0 && Timing::of(c) == timing {
            cycles += transfer(g, n);
        }
    }
    cycles
}

/// Run channel `n`'s transfer through the program's memory, then
/// repeat-reload or switch it off, as [`crate::dma`]'s `transfer`: a
/// transfer that switches its own channel off and on again stands, and an
/// immediate one runs again when it ends, in a loop. Returns the cycles
/// spent.
fn transfer<G: Native>(g: &mut G, n: usize) -> u32 {
    let mut cycles = 0;
    loop {
        let hw = g.hw();
        hw.dma.regs.ch[n].busy = true;
        hw.dma.regs.ch[n].relatched = false;
        let mut ch = hw.dma.regs.ch[n];
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
        cycles += super::dma_cycles(hw.waitcnt, size, ch.src, ch.dst, ch.count);
        let align = !(size - 1);
        for _ in 0..ch.count {
            let v = g.dma_load(ch.src & align, word);
            g.dma_store(ch.dst & align, v, word);
            ch.src = ch.src.wrapping_add(src_step);
            ch.dst = ch.dst.wrapping_add(dst_step);
        }
        let hw = g.hw();
        if c & ctl::IRQ != 0 {
            hw.request(irq::DMA0 << n);
        }
        let now = &mut hw.dma.regs.ch[n];
        now.busy = false;
        if now.relatched {
            // Switched off and on again during the transfer.
            if Timing::of(now.control) == Timing::Immediate {
                continue;
            }
            return cycles;
        }
        ch.control = now.control;
        ch.busy = false;
        if c & ctl::REPEAT != 0 && Timing::of(c) != Timing::Immediate {
            ch.count = hw.dma_count(n);
            if dst_ctl & 3 == 3 {
                ch.dst = hw.dma_read32(base(n) + 4);
            }
        } else {
            ch.control &= !ctl::ENABLE;
            let o = base(n) + 10;
            let io = &mut hw.sound.io;
            io.set_raw16(o, io.raw16(o) & !ctl::ENABLE);
        }
        hw.dma.regs.ch[n] = ch;
        return cycles;
    }
}
