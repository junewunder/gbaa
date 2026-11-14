//! Loading mGBA save states (decision 16): a checkpoint deep in a game is a
//! state plus N frames, run from the same state in mGBA and here.
//!
//! The format is mGBA 0.10's `GBASerializedState` (its
//! `include/mgba/internal/gba/serialize.h` documents every offset): the
//! uncompressed state `mgba-dump --save-state` writes, which is also how a
//! non-PNG `.ss` file from mGBA starts (extra data after it is ignored).
//! What carries over is all the guest state [`Machine`] keeps: registers
//! and banks, memory, I/O registers, timers, DMA channels, the sound FIFOs,
//! the scanline position and halt, the GPIO port and its clock's
//! registers. Cartridge save data isn't in the state (mGBA appends it as
//! extra data, which the desktop's `state_file` reads) and isn't loaded
//! here.
//!
//! Relative times (the next scanline event, a timer's last tick) are
//! relative to mGBA's clock at the save, which becomes [`Io::now`].
//!
//! **mGBA's BIOS.** mGBA runs a small HLE BIOS image, where we run calls as
//! Rust ([`crate::bios`]). A state saved while a game waits in
//! `IntrWait`/`VBlankIntrWait` or `Halt` has the CPU halted inside that
//! image. The loader unwinds the image's stack frames back to the caller,
//! as its own return path would, and records the wait as [`Pending`], which
//! is how [`crate::bios`] holds the same wait; an IRQ then finishes it the
//! same way. A state stopped anywhere else in the BIOS region is refused. A
//! handler that returns to the image's IRQ stub is handled by the runner
//! ([`crate::sched::MGBA_IRQ_RETURN`]).
//!
//! [`Io::now`]: crate::io::Io::now

use crate::bios::{Bios, Pending};
use crate::bus::{BIOS_SIZE, EWRAM_SIZE, IWRAM_SIZE, OAM_SIZE, PALETTE_SIZE, VRAM_SIZE};
use crate::dma;
use crate::io::{IO_SIZE, reg};
use crate::machine::{Machine, Prefetch};
use crate::overlay::Overlays;
use crate::platform::Platform;
use crate::sched::Sched;
use crate::timer::ctl;
use armv4t::interp::{Bank, T_BIT, mode};

/// A state's size: everything [`Machine::load_mgba_state`] reads.
pub const STATE_SIZE: usize = 0x6_1000;
/// `versionMagic`: mGBA's GBA magic plus a version, at most the newest
/// this loader knows (mGBA 0.10's).
const MAGIC: u32 = 0x0100_0000;
const MAX_VERSION: u32 = 7;

/// Byte offsets into the state.
mod at {
    pub const MAGIC: usize = 0x000;
    pub const ROM_CRC32: usize = 0x008;
    pub const GPRS: usize = 0x020;
    pub const CPSR: usize = 0x060;
    pub const SPSR: usize = 0x064;
    pub const CYCLES: usize = 0x068;
    /// `[bank][7]` words: sp, lr, then r8-r12 (user and FIQ banks only).
    pub const BANKED: usize = 0x070;
    pub const BANKED_SPSRS: usize = 0x118;
    pub const FIFO_A: usize = 0x18C;
    pub const FIFO_B: usize = 0x1AC;
    pub const INTERNAL_A: usize = 0x1CC;
    pub const INTERNAL_B: usize = 0x1D0;
    pub const SAMPLE_A: usize = 0x1D8;
    pub const SAMPLE_B: usize = 0x1D9;
    pub const AUDIO_FLAGS: usize = 0x1DA;
    pub const VIDEO_NEXT_EVENT: usize = 0x1F4;
    pub const VIDEO_FLAGS: usize = 0x1F8;
    /// 0x14 bytes a timer: reload (u16), then last event at +4.
    pub const TIMERS: usize = 0x200;
    /// 0x10 bytes a channel: next source, destination, count.
    pub const DMA: usize = 0x250;
    /// The GPIO port: pins, direction, then the RTC's fields (i32s from
    /// bytes remaining to the command, the control byte at `+0x1C`, the
    /// time at `+0x20`), the devices at `+0x27`, flags at `+0x2E`.
    pub const GPIO: usize = 0x290;
    pub const BIOS_PREFETCH: usize = 0x2F4;
    pub const GLOBAL_CYCLES: usize = 0x310;
    pub const MISC_FLAGS: usize = 0x31C;
    pub const IO: usize = 0x400;
    pub const PALETTE: usize = 0x800;
    pub const OAM: usize = 0xC00;
    pub const VRAM: usize = 0x1000;
    pub const IWRAM: usize = 0x1_9000;
    pub const EWRAM: usize = 0x2_1000;
}

/// mGBA's register banks, in its order (`BANK_NONE`, `BANK_FIQ`,
/// `BANK_IRQ`, `BANK_SUPERVISOR`, `BANK_ABORT`, `BANK_UNDEFINED`).
const MGBA_BANKS: [Bank; 6] = [
    Bank::User,
    Bank::Fiq,
    Bank::Irq,
    Bank::Svc,
    Bank::Abt,
    Bank::Und,
];

/// Where mGBA's HLE BIOS image (`src/gba/hle-bios.s`, 0.10) sits halted.
mod hle {
    /// In `Halt`, after its `HALTCNT` store.
    pub const HALT: u32 = 0x1C0;
    /// In `IntrWait`'s loop, after its `HALTCNT` store.
    pub const INTR_WAIT: u32 = 0x1F4;
}

/// Why a state didn't load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateError {
    /// Shorter than [`STATE_SIZE`] bytes.
    Short(usize),
    /// A PNG: mGBA's GUI states, which the desktop's `state_file` unpacks.
    Png,
    /// Not an mGBA GBA state, or a newer version.
    Magic(u32),
    /// Saved from another ROM (the CRC-32s).
    OtherRom { state: u32, rom: u32 },
    /// The CPU is in mGBA's BIOS somewhere other than a wait.
    InBios(u32),
}

impl core::fmt::Display for StateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StateError::Short(n) => {
                write!(f, "{n} bytes, short of an mGBA state's {STATE_SIZE:#x}")
            }
            StateError::Png => write!(
                f,
                "a PNG state (mGBA's GUI's): unpack it first (desktop::state_file)"
            ),
            StateError::Magic(m) => write!(f, "not an mGBA 0.10 GBA state (magic {m:#010x})"),
            StateError::OtherRom { state, rom } => write!(
                f,
                "saved from another ROM (CRC-32 {state:08x}, this ROM's is {rom:08x})"
            ),
            StateError::InBios(pc) => write!(
                f,
                "the CPU is inside mGBA's BIOS at {pc:#010x}, not in a wait: \
                 save the state at another frame"
            ),
        }
    }
}

/// Little-endian reads from the state.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u8(&self, o: usize) -> u8 {
        self.0[o]
    }
    fn u16(&self, o: usize) -> u16 {
        u16::from_le_bytes([self.0[o], self.0[o + 1]])
    }
    fn u32(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    /// A time relative to the save.
    fn rel(&self, o: usize) -> i64 {
        self.u32(o) as i32 as i64
    }
}

impl<P: Platform> Machine<P> {
    /// Replace the guest state with mGBA's save state `s`, saved from this
    /// machine's ROM. Runs on from where it was saved: a frame boundary if
    /// mGBA saved between frames, as `mgba-dump` does. Nothing is changed
    /// if it fails.
    pub fn load_mgba_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        if s.starts_with(b"\x89PNG") {
            return Err(StateError::Png);
        }
        if s.len() < STATE_SIZE {
            return Err(StateError::Short(s.len()));
        }
        let s = Reader(s);
        let magic = s.u32(at::MAGIC);
        if magic & !0xFF != MAGIC || magic & 0xFF > MAX_VERSION {
            return Err(StateError::Magic(magic));
        }
        let rom = self.bus.rom_crc();
        let state = s.u32(at::ROM_CRC32);
        if state != rom {
            return Err(StateError::OtherRom { state, rom });
        }
        let cpsr = s.u32(at::CPSR) | 0x10;
        // Between instructions mGBA's r15 is the fetch address, one
        // instruction past the next to run.
        let r15 = s.u32(at::GPRS + 60);
        let pc = r15.wrapping_sub(if cpsr & T_BIT != 0 { 2 } else { 4 });
        let halted = s.u32(at::MISC_FLAGS) & 1 != 0;
        let wait = match pc {
            hle::INTR_WAIT if halted => Some(3),
            hle::HALT if halted => Some(0),
            _ if (pc as usize) < BIOS_SIZE => return Err(StateError::InBios(pc)),
            _ => None,
        };

        // The CPU, as mGBA had it.
        let cpu = &mut self.cpu;
        for (i, r) in cpu.r.iter_mut().enumerate() {
            *r = s.u32(at::GPRS + 4 * i);
        }
        cpu.pc = pc;
        cpu.cpsr = cpsr;
        for (i, bank) in MGBA_BANKS.into_iter().enumerate() {
            let w = |j: usize| s.u32(at::BANKED + 4 * (7 * i + j));
            let b = &mut cpu.banks[bank as usize];
            for (k, r) in b[..5].iter_mut().enumerate() {
                *r = w(2 + k);
            }
            (b[5], b[6]) = (w(0), w(1));
            cpu.spsr[bank as usize] = s.u32(at::BANKED_SPSRS + 4 * i);
        }
        let current = cpu.bank();
        if current != Bank::User {
            cpu.spsr[current as usize] = s.u32(at::SPSR);
        }

        // Memory.
        let bus = &mut self.bus;
        let copy = |dst: &mut [u8], o: usize| dst.copy_from_slice(&s.0[o..o + dst.len()]);
        copy(&mut bus.ewram[..EWRAM_SIZE], at::EWRAM);
        copy(&mut bus.iwram[..IWRAM_SIZE], at::IWRAM);
        copy(&mut bus.video.palette[..PALETTE_SIZE], at::PALETTE);
        copy(&mut bus.video.vram[..VRAM_SIZE], at::VRAM);
        copy(&mut bus.video.oam[..OAM_SIZE], at::OAM);
        // Recompiled IWRAM code re-checks its bytes before it next runs.
        bus.overlays = Overlays::default();
        bus.bios_latch = s.u32(at::BIOS_PREFETCH);
        bus.open_bus = 0;

        // I/O registers. mGBA saves what each reads (the timers' current
        // counters) and the write-only ones as stored.
        let io_reg = |o: u32| s.u16(at::IO + o as usize);
        for o in (0..IO_SIZE).step_by(2) {
            bus.io.restore16(o, io_reg(o));
        }
        // POSTFLG; HALTCNT isn't a state.
        bus.io.restore16(reg::POSTFLG, io_reg(reg::POSTFLG) & 1);
        bus.waitcnt_written();
        bus.io.events = 0;
        let now = s.u32(at::GLOBAL_CYCLES) as u64 | (s.u32(at::GLOBAL_CYCLES + 4) as u64) << 32;
        // Plus 2^32, so times before the save (a timer's last tick, mGBA's
        // 32-bit clock) never fall below 0 early in a run.
        let now = now.wrapping_add(s.rel(at::CYCLES) as u64) + (1 << 32);
        bus.io.now = now;
        let then = |rel: i64| now.saturating_add_signed(rel).min(now);
        for n in 0..4 {
            let t = at::TIMERS + 0x14 * n;
            let cnt = reg::TM0CNT_L + 4 * n as u32;
            let control = io_reg(cnt + 2);
            bus.io.restore16(cnt, s.u16(t));
            let timer = &mut bus.io.timers[n];
            timer.counter = io_reg(cnt);
            timer.control = control;
            // As `timer::control_written` has it: timer 0 can't cascade.
            timer.ticking = control & ctl::ENABLE != 0 && (n == 0 || control & ctl::CASCADE == 0);
            timer.since = then(s.rel(t + 4));
        }

        // DMA channels: the addresses and count the next unit uses.
        for n in 0..4 {
            let d = at::DMA + 0x10 * n;
            let control = io_reg(reg::DMA0SAD + 12 * n as u32 + 10);
            let count = match s.u32(d + 8) {
                0 if control & dma::ctl::ENABLE != 0 => dma::count(bus, n),
                c => c,
            };
            bus.dma[n] = dma::Channel {
                src: s.u32(d),
                dst: s.u32(d + 4),
                count,
                control,
                busy: false,
                relatched: false,
            };
        }

        // The sound FIFOs: the word being played (its unplayed low bytes),
        // then the queued words, oldest first.
        let flags = s.u16(at::AUDIO_FLAGS);
        for (i, (words, internal, remaining, shift)) in [
            (at::FIFO_A, at::INTERNAL_A, flags >> 5 & 3, 7),
            (at::FIFO_B, at::INTERNAL_B, flags & 3, 2),
        ]
        .into_iter()
        .enumerate()
        {
            let fifo = &mut bus.sound.fifo[i];
            fifo.reset();
            let bytes = s.u32(internal).to_le_bytes();
            for &b in &bytes[..remaining as usize] {
                fifo.push(b);
            }
            for w in 0..(flags >> shift & 7) as usize {
                for b in s.u32(words + 4 * w).to_le_bytes() {
                    fifo.push(b);
                }
            }
        }
        bus.sound.out = [s.u8(at::SAMPLE_A) as i8, s.u8(at::SAMPLE_B) as i8];
        bus.sound.psg.load(&mut bus.io, now as i64, |o| s.u32(o));

        // The GPIO port and its clock. The clock's time isn't in the state
        // (mGBA keeps its source's in extra data); ours stays as it was.
        let g = at::GPIO;
        let i32_at = |o: usize| s.u32(g + o) as i32;
        let gpio = &mut bus.gpio;
        gpio.has_rtc = s.u8(g + 0x27) & 1 != 0;
        gpio.pins = s.u16(g);
        gpio.direction = s.u16(g + 2);
        gpio.read_write = s.u16(g + 0x2E) & 1;
        gpio.rtc = crate::gpio::Rtc {
            bytes_remaining: i32_at(4),
            transfer_step: i32_at(8),
            bits_read: i32_at(0xC),
            bits: s.u32(g + 0x10),
            command_active: s.u32(g + 0x14) != 0,
            command: s.u32(g + 0x18),
            control: s.u8(g + 0x1C),
            time: core::array::from_fn(|i| s.u8(g + 0x20 + i)),
        };
        // mGBA writes the registers into its ROM copy on load.
        if gpio.has_rtc {
            gpio.show();
        } else {
            gpio.shown = None;
        }

        // A V-blank or H-blank DMA mGBA had started but not yet run (its
        // `nextCount` set: it runs 3 cycles after the blank, and a frame
        // ends at the blank) runs now, as mGBA's would first thing.
        for n in 0..4 {
            let c = bus.dma[n].control;
            let blank = matches!(c >> dma::ctl::TIMING_SHIFT & 3, 1 | 2);
            if c & dma::ctl::ENABLE != 0 && blank && s.u32(at::DMA + 0x10 * n + 8) != 0 {
                let cycles = dma::start(bus, n);
                bus.io.now += cycles as u64;
            }
        }

        // The scanline position: mGBA's next video event is the next H-blank
        // start (mode 2) or the next line (mode 1, in H-blank).
        let video_next = now.saturating_add_signed(s.rel(at::VIDEO_NEXT_EVENT));
        self.sched = Sched {
            next_line_event: video_next,
            hblank: s.u32(at::VIDEO_FLAGS) & 3 == 1,
            // Sync at the first tick, which works out the real next event.
            next_event: now,
            halted,
            frame_ended: false,
            irq_line: false,
            held: 0,
            // A link hub's, not the state's.
            deadline: self.sched.deadline,
            window: true,
            fresh: false,
            spins: [(0, 0); crate::sched::SPIN_SLOTS],
        };
        self.bios = Bios::default();
        self.fault = None;
        self.prefetch = Prefetch {
            at: u64::MAX,
            ..Prefetch::default()
        };
        if let Some(own) = wait {
            self.leave_mgba_wait(own);
        }
        Ok(())
    }

    /// The CPU is halted in mGBA's `IntrWait` (with `own` = 3 words on the
    /// stack: r2, r3, lr) or `Halt` (none), in system mode, called from
    /// its `swi` handler. Return to the caller the way that handler would,
    /// and record the wait as [`crate::bios`] does.
    fn leave_mgba_wait(&mut self, own: u32) {
        let (cpu, bus) = (&mut self.cpu, &self.bus);
        debug_assert_eq!(cpu.cpsr & 0x1F, mode::SYS);
        let sp = cpu.r[13];
        let mask = if own == 3 {
            // `IntrWait`'s `stmfd sp!, {r2-r3, lr}`.
            cpu.r[3] = bus.read32(sp.wrapping_add(4));
            cpu.r[1] as u16
        } else {
            0
        };
        // The handler's `stmfd sp!, {r2, lr}` in system mode...
        let sp = sp.wrapping_add(4 * own);
        cpu.r[2] = bus.read32(sp);
        cpu.r[14] = bus.read32(sp.wrapping_add(4));
        cpu.r[13] = sp.wrapping_add(8);
        // ... after `stmfd sp!, {r11, r12, lr}` and the SPSR in SVC mode.
        let svc = &mut cpu.banks[Bank::Svc as usize];
        let sp = svc[5];
        let caller = bus.read32(sp);
        cpu.r[11] = bus.read32(sp.wrapping_add(4));
        cpu.r[12] = bus.read32(sp.wrapping_add(8));
        let ret = bus.read32(sp.wrapping_add(12));
        (svc[5], svc[6]) = (sp.wrapping_add(16), ret);
        cpu.spsr[Bank::Svc as usize] = caller;
        // `movs pc, lr`.
        cpu.write_cpsr(caller, !0);
        cpu.pc = ret;
        // Which handler level the caller is at isn't saved; a wait inside
        // an IRQ handler is taken as one level deep.
        self.bios.irq_depth = (caller & 0x1F == mode::IRQ) as u32;
        self.bios.push(Pending {
            pc: ret,
            depth: self.bios.irq_depth,
            mask,
        });
    }
}

/// The CRC-32 (zlib's) mGBA keys states to a ROM with.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_continue(0, data)
}

/// The CRC-32 of what gave `crc`, then `data`.
pub fn crc32_continue(crc: u32, data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    !data.iter().fold(!crc, |c, &b| {
        TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8)
    })
}

#[cfg(test)]
mod tests;
