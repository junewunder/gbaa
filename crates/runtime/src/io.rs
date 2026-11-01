//! The I/O register block (`0x0400_0000`–`0x0400_03FF`): storage, and the
//! read and write masks of every register.
//!
//! Behaviour is kept to what milestone 2 needs: `DISPCNT`/`DISPSTAT`/
//! `VCOUNT`, `IE`/`IF`/`IME`, `KEYINPUT`, `WAITCNT` (stored only). Everything
//! else is stored through its masks. Two hooks let other parts of the
//! runtime add behaviour without the bus knowing about them:
//!
//! - **Writes** to registers with side effects (DMA enable, timer control,
//!   `HALTCNT`, the sound registers, the interrupt registers) set a bit in
//!   [`Io::events`]. (Stores to the sound FIFOs push in the store itself:
//!   [`crate::sound::io_written`].) The runner handles and clears them at its next sync
//!   point; I/O writes are sync points (decision 11).
//! - **State the hardware owns** goes in through setters: the scanline
//!   counter, blanking flags, interrupt requests and keys through
//!   [`Io::set_vcount`], [`Io::set_blanking`], [`Io::request_irq`] and
//!   [`Io::set_keys`]; anything else (a DMA clearing its enable bit, sound
//!   channel status) through [`Io::set_raw16`], which raises no events.
//!   Timers live in [`Io::timers`], which `TMxCNT_L` reads (writes set the
//!   reload value); [`Io::now`] is the machine's cycle counter, which they
//!   count from.
//!
//! Bits a register doesn't let you read come back 0, as do unused
//! addresses inside the block. (On hardware unused addresses read open bus;
//! jsmolka's `unsafe` tests are where that would show.)

use crate::timer::Timer;

/// Register offsets from `0x0400_0000`.
pub mod reg {
    pub const DISPCNT: u32 = 0x000;
    pub const DISPSTAT: u32 = 0x004;
    pub const VCOUNT: u32 = 0x006;
    pub const BG0CNT: u32 = 0x008;
    pub const BG0HOFS: u32 = 0x010;
    pub const BG2PA: u32 = 0x020;
    pub const BG3PA: u32 = 0x030;
    pub const WIN0H: u32 = 0x040;
    pub const WIN0V: u32 = 0x044;
    pub const WININ: u32 = 0x048;
    pub const WINOUT: u32 = 0x04A;
    pub const MOSAIC: u32 = 0x04C;
    pub const BLDCNT: u32 = 0x050;
    pub const BLDALPHA: u32 = 0x052;
    pub const BLDY: u32 = 0x054;
    pub const SOUNDCNT_H: u32 = 0x082;
    pub const SOUNDCNT_X: u32 = 0x084;
    pub const SOUNDBIAS: u32 = 0x088;
    pub const WAVE_RAM: u32 = 0x090;
    pub const FIFO_A: u32 = 0x0A0;
    pub const FIFO_B: u32 = 0x0A4;
    /// DMA channel `n`'s registers start at `DMA0SAD + 12 * n`.
    pub const DMA0SAD: u32 = 0x0B0;
    /// Timer `n`'s registers start at `TM0CNT_L + 4 * n`.
    pub const TM0CNT_L: u32 = 0x100;
    /// `SIOMULTI0`-`3`: what each player sent in the last multiplayer
    /// transfer.
    pub const SIOMULTI0: u32 = 0x120;
    pub const SIOCNT: u32 = 0x128;
    /// `SIOMLT_SEND` (`SIODATA8` in normal mode).
    pub const SIOMLT_SEND: u32 = 0x12A;
    pub const KEYINPUT: u32 = 0x130;
    pub const KEYCNT: u32 = 0x132;
    pub const RCNT: u32 = 0x134;
    pub const IE: u32 = 0x200;
    pub const IF: u32 = 0x202;
    pub const WAITCNT: u32 = 0x204;
    pub const IME: u32 = 0x208;
    /// `POSTFLG` (byte 0x300) and `HALTCNT` (byte 0x301).
    pub const POSTFLG: u32 = 0x300;
}

/// Interrupt sources, as bits of `IE` and `IF`.
pub mod irq {
    pub const VBLANK: u16 = 1 << 0;
    pub const HBLANK: u16 = 1 << 1;
    pub const VCOUNT: u16 = 1 << 2;
    /// Timer `n` is `TIMER0 << n`.
    pub const TIMER0: u16 = 1 << 3;
    pub const SERIAL: u16 = 1 << 7;
    /// DMA channel `n` is `DMA0 << n`.
    pub const DMA0: u16 = 1 << 8;
    pub const KEYPAD: u16 = 1 << 12;
    pub const GAMEPAK: u16 = 1 << 13;
}

/// Keys, as bits of [`Io::set_keys`]'s argument (1 = pressed; `KEYINPUT`
/// itself is active low).
pub mod key {
    pub const A: u16 = 1 << 0;
    pub const B: u16 = 1 << 1;
    pub const SELECT: u16 = 1 << 2;
    pub const START: u16 = 1 << 3;
    pub const RIGHT: u16 = 1 << 4;
    pub const LEFT: u16 = 1 << 5;
    pub const UP: u16 = 1 << 6;
    pub const DOWN: u16 = 1 << 7;
    pub const R: u16 = 1 << 8;
    pub const L: u16 = 1 << 9;
    pub const ALL: u16 = 0x3FF;
}

/// Bits of [`Io::events`]: writes the runner must act on.
pub mod event {
    /// DMA channel `n`'s control was written: `DMA0 << n`.
    pub const DMA0: u32 = 1 << 0;
    /// Timer `n`'s control or reload was written: `TIMER0 << n`.
    pub const TIMER0: u32 = 1 << 4;
    /// `HALTCNT` written with bit 7 clear.
    pub const HALT: u32 = 1 << 8;
    /// `HALTCNT` written with bit 7 set.
    pub const STOP: u32 = 1 << 9;
    /// `IE`, `IF` or `IME` changed: an interrupt may now be due.
    pub const IRQ: u32 = 1 << 10;
    /// A sound register other than the FIFOs was written.
    pub const SOUND: u32 = 1 << 13;
    /// `SOUNDCNT_H` was written with bit 11 set: reset FIFO A. (The bit
    /// itself isn't stored.) The bus acts on it in the store.
    pub const FIFO_A_RESET: u32 = 1 << 14;
    /// `SOUNDCNT_H` was written with bit 15 set: reset FIFO B.
    pub const FIFO_B_RESET: u32 = 1 << 15;
    /// `SIOCNT` was written on a machine on a link ([`Io::sio_linked`]).
    pub const SERIAL: u32 = 1 << 16;
}

/// The block's size in bytes. Addresses past it inside the I/O region read
/// open bus and ignore writes.
pub const IO_SIZE: u32 = 0x400;

/// The offset into the block for an I/O address, or `None` outside it.
#[inline(always)]
pub fn offset(addr: u32) -> Option<u32> {
    let o = addr & 0x00FF_FFFF;
    (o < IO_SIZE).then_some(o)
}

/// The readable and writable bits of the halfword at offset `o` (even).
/// From GBATEK's register list.
const fn masks(o: u32) -> (u16, u16) {
    const RW: (u16, u16) = (0xFFFF, 0xFFFF);
    const WO: (u16, u16) = (0, 0xFFFF);
    const NONE: (u16, u16) = (0, 0);
    match o {
        // DISPCNT: bit 3 (CGB mode) only the BIOS can set.
        0x000 => (0xFFF7, 0xFFF7),
        // Green swap.
        0x002 => (0x0001, 0x0001),
        // DISPSTAT: bits 0-2 are status, set by the scheduler.
        0x004 => (0xFF3F, 0xFF38),
        0x006 => (0x00FF, 0),
        // BG0CNT, BG1CNT have no overflow bit; BG2CNT, BG3CNT do.
        0x008 | 0x00A => (0xDFFF, 0xDFFF),
        0x00C | 0x00E => RW,
        // Scroll offsets, write-only, 9 bits.
        0x010..=0x01E => (0, 0x01FF),
        // BG2/BG3 affine: PA-PD, then 28-bit reference points.
        0x020..=0x026 | 0x030..=0x036 => WO,
        0x028 | 0x02C | 0x038 | 0x03C => WO,
        0x02A | 0x02E | 0x03A | 0x03E => (0, 0x0FFF),
        // Window bounds.
        0x040..=0x046 => WO,
        0x048 | 0x04A => (0x3F3F, 0x3F3F),
        0x04C => WO,
        0x050 => (0x3FFF, 0x3FFF),
        0x052 => (0x1F1F, 0x1F1F),
        0x054 => (0, 0x001F),
        // Sound channel 1.
        0x060 => (0x007F, 0x007F),
        0x062 => (0xFFC0, 0xFFFF),
        0x064 => (0x4000, 0xC7FF),
        // Sound channel 2.
        0x068 => (0xFFC0, 0xFFFF),
        0x06C => (0x4000, 0xC7FF),
        // Sound channel 3.
        0x070 => (0x00E0, 0x00E0),
        0x072 => (0xE000, 0xE0FF),
        0x074 => (0x4000, 0xC7FF),
        // Sound channel 4.
        0x078 => (0xFF00, 0xFF3F),
        0x07C => (0x40FF, 0xC0FF),
        // SOUNDCNT_L, SOUNDCNT_H (bits 11 and 15 reset the FIFOs), SOUNDCNT_X
        // (bits 0-3 are channel status), SOUNDBIAS.
        0x080 => (0xFF77, 0xFF77),
        0x082 => (0x770F, 0x770F),
        0x084 => (0x008F, 0x0080),
        0x088 => (0xC3FE, 0xC3FE),
        0x090..=0x09E => RW,
        0x0A0..=0x0A6 => WO,
        // DMA: source, destination, count, control, 12 bytes a channel.
        0x0B0..=0x0DE => {
            let ch = (o - 0x0B0) / 12;
            match (o - 0x0B0) % 12 {
                // Source high: DMA0 reaches internal memory only.
                2 => (0, if ch == 0 { 0x07FF } else { 0x0FFF }),
                // Destination high: only DMA3 reaches the cartridge.
                6 => (0, if ch == 3 { 0x0FFF } else { 0x07FF }),
                8 => (0, if ch == 3 { 0xFFFF } else { 0x3FFF }),
                // Control: bit 11 (gamepak DRQ) is DMA3's alone.
                10 => {
                    let m = if ch == 3 { 0xFFE0 } else { 0xF7E0 };
                    (m, m)
                }
                _ => WO,
            }
        }
        // Timers: the reload value (reads give the counter), control.
        0x100..=0x10E => {
            if o & 2 == 0 {
                RW
            } else {
                (0x00C7, 0x00C7)
            }
        }
        // Serial and joybus, stored as is; a link hub owns some of SIOCNT
        // (`Io::sio_owned`, `crate::link`).
        0x120..=0x12A => RW,
        0x134 => (0xC1FF, 0xC1FF),
        0x140 => (0x0047, 0x0047),
        0x150..=0x15A => RW,
        0x130 => (0x03FF, 0),
        0x132 => (0xC3FF, 0xC3FF),
        0x200 | 0x202 => (0x3FFF, 0x3FFF),
        // WAITCNT: bit 15 is the cartridge type, read-only and 0 (GBA).
        0x204 => (0x5FFF, 0x5FFF),
        0x208 => (0x0001, 0x0001),
        // POSTFLG reads back; HALTCNT is write-only.
        0x300 => (0x0001, 0xFF01),
        _ => NONE,
    }
}

const MASKS: [(u16, u16); (IO_SIZE / 2) as usize] = {
    let mut t = [(0, 0); (IO_SIZE / 2) as usize];
    let mut i = 0;
    while i < t.len() {
        t[i] = masks(i as u32 * 2);
        i += 1;
    }
    t
};

/// The bits of the halfword at offset `o` (even, inside the block) a CPU
/// store sets.
pub fn write_mask(o: u32) -> u16 {
    MASKS[(o / 2) as usize].1
}

/// The bits of the halfword at offset `o` (even, inside the block) a CPU
/// load sees.
pub fn read_mask(o: u32) -> u16 {
    MASKS[(o / 2) as usize].0
}

/// The I/O registers.
pub struct Io {
    /// Each halfword as last written, through its write mask.
    regs: [u16; (IO_SIZE / 2) as usize],
    /// Side-effecting writes since the runner last looked; see [`event`].
    pub events: u32,
    /// The timers' running state; `TMxCNT_L` reads their counters. The
    /// register's stored value is the reload. See [`crate::timer`].
    pub timers: [Timer; 4],
    /// Cycles since power-on: the machine's clock, advanced by
    /// [`crate::Machine::tick`]. Here rather than on the machine so timer
    /// reads see the current time.
    pub now: u64,
    /// The machine is on a link hub ([`crate::link`]): `SIOCNT` writes
    /// raise [`event::SERIAL`]. Not guest state: the hub sets it.
    pub sio_linked: bool,
    /// The `SIOCNT` bits the link hub owns (the terminals, the player ID,
    /// a busy flag), which the CPU's writes leave alone. 0 off a link.
    pub sio_owned: u16,
}

impl Default for Io {
    fn default() -> Io {
        Io::new()
    }
}

impl Io {
    /// The registers after the BIOS hands over, as mGBA sets them: 0
    /// apart from `KEYINPUT` (no keys), `POSTFLG` (booted), the BG2/BG3
    /// affine identity (`PA` = `PD` = 1.0), `SOUNDBIAS` and `RCNT`.
    pub fn new() -> Io {
        let mut io = Io {
            regs: [0; (IO_SIZE / 2) as usize],
            events: 0,
            timers: [Timer::default(); 4],
            now: 0,
            sio_linked: false,
            sio_owned: 0,
        };
        io.set_keys(0);
        for (o, v) in [
            (reg::POSTFLG, 1),
            (reg::BG2PA, 0x100),
            (reg::BG2PA + 6, 0x100),
            (reg::BG3PA, 0x100),
            (reg::BG3PA + 6, 0x100),
            (reg::SOUNDBIAS, 0x200),
            (reg::RCNT, 0x8000),
        ] {
            io.regs[o as usize / 2] = v;
        }
        io
    }

    /// The halfword at offset `o` (even, inside the block), through its
    /// read mask.
    #[inline]
    pub fn read16(&self, o: u32) -> u16 {
        let i = (o / 2) as usize;
        match o {
            0x100 | 0x104 | 0x108 | 0x10C => {
                self.timers[((o - 0x100) / 4) as usize].read(self.now, self.regs[i])
            }
            _ => self.regs[i] & MASKS[i].0,
        }
    }

    /// The raw stored halfword at `o`, ignoring the read mask: for the
    /// PPU, DMA and timers, which see write-only registers too.
    #[inline(always)]
    pub fn raw16(&self, o: u32) -> u16 {
        self.regs[(o / 2) as usize]
    }

    /// Store the halfword at `o` (even) as the hardware sets it: no masks,
    /// no events.
    #[inline(always)]
    pub fn set_raw16(&mut self, o: u32, v: u16) {
        self.regs[(o / 2) as usize] = v;
    }

    /// Store the halfword at `o` (even) from a save state
    /// ([`crate::state`]): the bits it can read or write, no events.
    pub fn restore16(&mut self, o: u32, v: u16) {
        let i = (o / 2) as usize;
        self.regs[i] = v & (MASKS[i].0 | MASKS[i].1);
    }

    /// Store the halfword at `o` as the CPU writes it.
    #[inline]
    pub fn write16(&mut self, o: u32, v: u16) {
        self.write_masked(o, v, 0xFFFF);
    }

    /// Store a byte at `o` as the CPU writes it: the other byte of the
    /// halfword is left alone, and `IF` clears only this byte's bits.
    #[inline]
    pub fn write8(&mut self, o: u32, v: u8) {
        let shift = (o & 1) * 8;
        self.write_masked(o & !1, (v as u16) << shift, 0xFF << shift);
    }

    /// Store the bits `lanes` of `v` at the halfword `o`.
    fn write_masked(&mut self, o: u32, v: u16, lanes: u16) {
        let i = (o / 2) as usize;
        let mut m = MASKS[i].1 & lanes;
        let old = self.regs[i];
        match o {
            reg::SIOCNT => {
                m &= !self.sio_owned;
                if self.sio_linked {
                    self.events |= event::SERIAL;
                }
            }
            // Writing 1 acknowledges an interrupt.
            reg::IF => {
                self.regs[i] = old & !(v & m);
                self.events |= event::IRQ;
                return;
            }
            reg::IE | reg::IME => self.events |= event::IRQ,
            0x0BA | 0x0C6 | 0x0D2 | 0x0DE => {
                self.events |= event::DMA0 << ((o - 0x0BA) / 12);
            }
            0x100..=0x10E => self.events |= event::TIMER0 << ((o - 0x100) / 4),
            reg::SOUNDCNT_H => {
                let v = v & lanes;
                self.events |= event::SOUND
                    | if v & 0x0800 != 0 {
                        event::FIFO_A_RESET
                    } else {
                        0
                    }
                    | if v & 0x8000 != 0 {
                        event::FIFO_B_RESET
                    } else {
                        0
                    };
            }
            0x060..=0x09E => self.events |= event::SOUND,
            reg::POSTFLG if lanes & 0xFF00 != 0 => {
                self.events |= if v & 0x8000 != 0 {
                    event::STOP
                } else {
                    event::HALT
                };
            }
            _ => {}
        }
        self.regs[i] = (old & !m) | (v & m);
        if o == reg::DISPSTAT {
            // The V-count flag compares continuously, so a new target line
            // counts at once.
            self.set_vcount(self.vcount());
        }
    }

    #[inline(always)]
    pub fn dispcnt(&self) -> u16 {
        self.regs[reg::DISPCNT as usize / 2]
    }

    #[inline(always)]
    pub fn vcount(&self) -> u16 {
        self.regs[reg::VCOUNT as usize / 2]
    }

    /// Set the current scanline, and with it `DISPSTAT`'s V-count match flag.
    /// Returns whether the line matches the `DISPSTAT` target (the caller
    /// raises the V-count interrupt if it's enabled).
    pub fn set_vcount(&mut self, line: u16) -> bool {
        self.regs[reg::VCOUNT as usize / 2] = line;
        let stat = &mut self.regs[reg::DISPSTAT as usize / 2];
        let hit = *stat >> 8 == line;
        *stat = (*stat & !4) | ((hit as u16) << 2);
        hit
    }

    /// Set `DISPSTAT`'s V-blank and H-blank flags.
    pub fn set_blanking(&mut self, vblank: bool, hblank: bool) {
        let stat = &mut self.regs[reg::DISPSTAT as usize / 2];
        *stat = (*stat & !3) | vblank as u16 | (hblank as u16) << 1;
    }

    /// Set the pressed keys (a mask of [`key`] bits, 1 = pressed).
    pub fn set_keys(&mut self, pressed: u16) {
        self.regs[reg::KEYINPUT as usize / 2] = !pressed & key::ALL;
    }

    /// Raise interrupt requests (bits of [`irq`]) in `IF`.
    pub fn request_irq(&mut self, bits: u16) {
        self.regs[reg::IF as usize / 2] |= bits & 0x3FFF;
        self.events |= event::IRQ;
    }

    /// Whether an enabled interrupt is requested, ignoring `IME` and the
    /// CPSR's I bit: what wakes the CPU from halt.
    #[inline]
    pub fn irq_requested(&self) -> bool {
        self.regs[reg::IE as usize / 2] & self.regs[reg::IF as usize / 2] != 0
    }

    /// Whether the CPU should take an IRQ, apart from the CPSR's I bit.
    #[inline]
    pub fn irq_pending(&self) -> bool {
        self.regs[reg::IME as usize / 2] & 1 != 0 && self.irq_requested()
    }

    /// Take and clear the pending events.
    #[inline]
    pub fn take_events(&mut self) -> u32 {
        core::mem::take(&mut self.events)
    }
}

crate::snap::snap!(Io {
    regs,
    events,
    timers,
    now
}; skip sio_linked, sio_owned);
