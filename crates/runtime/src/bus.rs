//! The memory map. [`Bus`] owns every memory region and the I/O block, and
//! its accessors are what both the interpreter and generated code read and
//! write through ([`armv4t::interp::Bus`]). EWRAM and IWRAM accesses inline;
//! the rest of the map is one out-of-line call, which halves rustc's time
//! on a generated part crate and doesn't measurably slow it down.
//!
//! Regions, by the address's top byte:
//!
//! | top byte | region  | size  | mirrored every | byte stores        |
//! |----------|---------|-------|----------------|--------------------|
//! | `00`     | BIOS    | 16 KB | no             | ignored            |
//! | `02`     | EWRAM   | 256 KB| 256 KB         | yes                |
//! | `03`     | IWRAM   | 32 KB | 32 KB          | yes                |
//! | `04`     | I/O     | 1 KB  | no             | yes                |
//! | `05`     | palette | 1 KB  | 1 KB           | as a halfword      |
//! | `06`     | VRAM    | 96 KB | 128 KB (below) | BG: halfword; OBJ: ignored |
//! | `07`     | OAM     | 1 KB  | 1 KB           | ignored            |
//! | `08`–`0D`| ROM     | ≤32 MB| three windows  | ignored            |
//! | `0E`–`0F`| save    | 64 KB | 64 KB          | 8-bit bus          |
//!
//! VRAM's last 32 KB window (`0x18000`–`0x1FFFF`) mirrors `0x10000`–`0x17FFF`,
//! except that in the bitmap modes (3–5) `0x18000`–`0x1BFFF` reads 0 and
//! ignores writes, as in mGBA. Unmapped addresses read [`Bus::open_bus`];
//! ROM reads past the end of the image return the halfword address, as on
//! hardware. The cartridge's GPIO port (its real-time clock) sits at
//! `0x080000C4`–`C9` ([`crate::gpio`]). The save region is SRAM or a flash
//! chip ([`crate::save`]).
//!
//! The CPU's loads and stores (through [`armv4t::interp::Bus`], which the
//! interpreter and generated code both use) count their cycles, wait states
//! included ([`crate::waits`]), in [`Bus::data_cycles`] for the next tick.
//! An `ldm`/`stm` first says how many accesses it makes ([`Bus::block`]),
//! and those count as mGBA's `GBALoadMultiple` counts them. The bus's own
//! `read*`/`write*` (DMA, BIOS calls, the PPU) don't count.

use crate::dma::{self, Channel};
use crate::gpio::Gpio;
use crate::io::{self, Io, event};
use crate::overlay::Overlays;
use crate::save::{self, Save};
use crate::sound::{self, Sound};
use crate::video::Video;
use crate::waits::{self, Waits};
use armv4t::sem::Width;

pub const BIOS_SIZE: usize = 0x4000;
pub const EWRAM_SIZE: usize = 0x4_0000;
pub const IWRAM_SIZE: usize = 0x8000;
pub use crate::video::{OAM_SIZE, PALETTE_SIZE, VRAM_SIZE};
pub const SRAM_SIZE: usize = save::BANK_SIZE;
/// The largest ROM the cartridge bus can address.
pub const ROM_MAX: usize = 0x200_0000;

/// What the BIOS region reads while code runs outside it (BIOS
/// protection: the last opcode fetched from the BIOS). These are GBATEK's
/// values after boot, inside an IRQ handler (called from the BIOS's IRQ
/// stub), after an IRQ returns and after a `swi` returns. BIOS HLE
/// ([`crate::bios`]) moves [`Bus::bios_latch`] between them.
pub const BIOS_LATCH_AFTER_BOOT: u32 = 0xE129_F000;
pub const BIOS_LATCH_IN_IRQ: u32 = 0xE25E_F004;
pub const BIOS_LATCH_AFTER_IRQ: u32 = 0xE55E_C002;
pub const BIOS_LATCH_AFTER_SWI: u32 = 0xE3A0_2004;

/// All of the GBA's memory and its I/O registers.
pub struct Bus {
    pub ewram: [u8; EWRAM_SIZE],
    pub iwram: [u8; IWRAM_SIZE],
    /// The display's palette, VRAM and OAM, and its registers decoded
    /// ([`crate::video`]): the registers' truth is [`Bus::io`], copied in
    /// before each line is drawn ([`Bus::video_for_draw`]).
    pub video: Video,
    /// The cartridge's save memory, SRAM, flash or EEPROM.
    pub save: Save,
    pub io: Io,
    /// The DMA channels' internal registers.
    pub dma: [Channel; 4],
    /// The sound FIFOs and the sample stream ([`crate::sound`]).
    pub sound: Sound,
    /// The cartridge image. `'static` so the desktop can leak a loaded file
    /// and embedded builds can point into flash or `include_bytes!`.
    pub rom: &'static [u8],
    /// What the BIOS region reads. No BIOS image is executed (decision 12),
    /// so reads always see the protection latch.
    pub bios_latch: u32,
    /// What unmapped addresses read. On hardware that's the last prefetched
    /// opcode; the runner keeps it up to date if it cares (#27), so the
    /// accessors don't pay for it on every load.
    pub open_bus: u32,
    /// The guards of the code recompiled into IWRAM ([`crate::overlay`]):
    /// every IWRAM store tells them.
    pub overlays: Overlays,
    /// The cartridge's GPIO port and real-time clock.
    pub gpio: Gpio,
    /// Each region's wait states, from `WAITCNT`.
    pub waits: Waits,
    /// The cycles of the CPU's data accesses since the last tick, or since
    /// the last [`Bus::stall`], and of its multiplies (which mGBA's stall
    /// overlaps too). An `ldm`'s may take some off.
    pub data_cycles: i32,
    /// One of those accesses was to the cartridge (or past it), which the
    /// prefetch buffer can't overlap.
    pub data_cart: bool,
    /// The cycles of the data accesses [`Bus::stall`] settled since the
    /// last tick, its prefetch overlap taken off.
    pub stall_cycles: i32,
    /// The last address the prefetch buffer fetched ([`Waits::stall`]).
    pub prefetched: u32,
    /// The accesses of the `ldm`/`stm` under way still to come.
    pub block_left: u32,
    /// Its first access is still to come.
    pub block_first: bool,
    /// [`Bus::rom_crc`]'s answer, with the address and length of the ROM
    /// it was for (so a new `rom` is hashed afresh).
    rom_crc: Option<(usize, usize, u32)>,
    /// A mod's replacements for compressed blobs (docs/asset-extraction-design.md,
    /// "Overrides"): (ROM address of the blob's header, its unpacked bytes),
    /// sorted by address. The HLE LZ77 and RL calls write these instead of
    /// what they decode. Empty, the play view, unless the front end sets it.
    pub asset_overrides: &'static [(u32, &'static [u8])],
}

impl crate::snap::Snap for Bus {
    fn snap(&mut self, v: &mut dyn crate::snap::Visitor) {
        use crate::snap::bytes;
        let Bus {
            ewram,
            iwram,
            video,
            save,
            io,
            dma,
            sound,
            rom: _,
            bios_latch,
            open_bus,
            overlays: _,
            gpio,
            waits,
            data_cycles,
            data_cart,
            stall_cycles,
            prefetched,
            // An `ldm`'s count lasts only within its instruction: every
            // tick clears it.
            block_left: _,
            block_first: _,
            rom_crc: _,
            asset_overrides: _,
        } = self;
        // The payload as before the split: the registers are in `io`.
        let Video {
            palette, vram, oam, ..
        } = video;
        for b in [&mut ewram[..], iwram, palette, vram, oam] {
            bytes(b, v);
        }
        save.snap(v);
        io.snap(v);
        dma.snap(v);
        sound.snap(v);
        bios_latch.snap(v);
        open_bus.snap(v);
        gpio.snap(v);
        waits.snap(v);
        data_cycles.snap(v);
        data_cart.snap(v);
        stall_cycles.snap(v);
        prefetched.snap(v);
    }
}

impl Bus {
    pub fn new(rom: &'static [u8]) -> Bus {
        assert!(rom.len() <= ROM_MAX, "ROM larger than 32 MB");
        Bus {
            ewram: [0; EWRAM_SIZE],
            iwram: [0; IWRAM_SIZE],
            video: Video::new(),
            save: Save::new(save::Kind::detect(rom)),
            io: Io::new(),
            dma: [Channel::default(); 4],
            sound: Sound::default(),
            rom,
            bios_latch: BIOS_LATCH_AFTER_BOOT,
            open_bus: 0,
            overlays: Overlays::default(),
            gpio: Gpio::for_rom(rom),
            waits: Waits::default(),
            data_cycles: 0,
            data_cart: false,
            stall_cycles: 0,
            prefetched: 0,
            block_left: 0,
            block_first: false,
            rom_crc: None,
            asset_overrides: &[],
        }
    }

    /// The display as the PPU draws it now: [`Bus::video`] with its
    /// registers decoded from [`Bus::io`].
    pub fn video_for_draw(&mut self) -> &Video {
        self.video.load_regs(&self.io);
        &self.video
    }

    /// The ROM's CRC-32 ([`crate::state::crc32`]), computed once: a 16 MB
    /// ROM takes tens of milliseconds, far more than a save state's payload.
    pub fn rom_crc(&mut self) -> u32 {
        let key = (self.rom.as_ptr() as usize, self.rom.len());
        match self.rom_crc {
            Some((p, n, crc)) if (p, n) == key => crc,
            _ => {
                let crc = crate::state::crc32(self.rom);
                self.rom_crc = Some((key.0, key.1, crc));
                crc
            }
        }
    }

    /// Have [`Bus::rom_crc`] answer `crc` for the ROM now in [`Bus::rom`]:
    /// a mod's patched copy keeps the original's CRC, so its save states
    /// and saves stay interchangeable with unmodded play.
    pub fn keep_rom_crc(&mut self, crc: u32) {
        self.rom_crc = Some((self.rom.as_ptr() as usize, self.rom.len(), crc));
    }

    /// Take `WAITCNT`'s value into [`Bus::waits`] (after a store to it, or
    /// a save state's load).
    pub fn waitcnt_written(&mut self) {
        self.waits.set_waitcnt(self.io.raw16(io::reg::WAITCNT));
    }

    /// The cycles of the data accesses since the last call, as the tick
    /// adds them.
    #[inline(always)]
    pub fn take_data_cycles(&mut self) -> i32 {
        let c = self.data_cycles + self.stall_cycles;
        self.data_cycles = 0;
        self.data_cart = false;
        self.stall_cycles = 0;
        self.block_left = 0;
        c
    }

    /// After a memory instruction run from ROM region `r` (`pc` its
    /// address plus two fetches; `load`: it has an I cycle): with the
    /// prefetch buffer on, let it overlap the instruction's data accesses
    /// ([`Waits::stall`]).
    #[inline]
    pub fn stall(&mut self, r: usize, pc: u32, load: bool) {
        // Settle this instruction's accesses alone, as the interpreter's
        // per-instruction tick would: generated code ticks once a block.
        let d = core::mem::take(&mut self.data_cycles);
        let cart = core::mem::take(&mut self.data_cart);
        if d == 0 {
            return;
        }
        #[cfg(feature = "timing-lab")]
        let ahead = self.prefetched.wrapping_sub(pc);
        let cost = if !self.waits.prefetch || cart {
            d
        } else {
            let i = load as i32;
            self.waits.stall(r, pc, &mut self.prefetched, d + i) - i
        };
        #[cfg(feature = "timing-lab")]
        let cost = crate::timing_lab::cost(&crate::timing_lab::Stall {
            region: r,
            pc,
            load,
            data: d,
            cart,
            waitcnt: self.io.raw16(io::reg::WAITCNT),
            ahead,
            cost,
        });
        self.stall_cycles += cost;
    }

    /// The next `n` data accesses are an `ldm`'s or `stm`'s
    /// ([`armv4t::cycles::block_accesses`]).
    #[inline(always)]
    pub fn block(&mut self, n: u32) {
        self.block_left = n;
        self.block_first = true;
    }

    /// Count a CPU data access of `bytes` at `addr`.
    #[inline(always)]
    fn count_data(&mut self, bytes: u32, addr: u32) {
        self.data_cart |= addr >= 0x0800_0000;
        if self.block_left == 0 {
            self.data_cycles += self.waits.data(bytes, addr) as i32;
            return;
        }
        // mGBA's `GBALoadMultiple`/`GBAStoreMultiple`: every access is
        // sequential, and the first takes off its region's N less S once.
        self.block_left -= 1;
        let r = waits::region(addr);
        let seq = self.waits.seq32[r] as i32;
        self.data_cycles += 1 + seq;
        if core::mem::take(&mut self.block_first) {
            self.data_cycles += seq - self.waits.nonseq32[r] as i32;
        }
    }

    /// Whether the display is in a bitmap mode (3, 4 or 5), which moves the
    /// VRAM BG/OBJ boundary.
    #[inline(always)]
    fn bitmap_mode(&self) -> bool {
        self.io.dispcnt() & 7 >= 3
    }

    /// The offset into [`Video::vram`] for `addr`, or `None` for the bitmap
    /// modes' dead window.
    #[inline(always)]
    fn vram_offset(&self, addr: u32) -> Option<usize> {
        let o = (addr & 0x1_FFFF) as usize;
        if o < VRAM_SIZE {
            Some(o)
        } else if o < 0x1_C000 && self.bitmap_mode() {
            None
        } else {
            Some(o - 0x8000)
        }
    }

    /// Whether `addr` reaches the EEPROM: all of `0D` behind a ROM of up
    /// to 16 MB, its last 256 bytes behind a larger one (as mGBA).
    #[inline(always)]
    pub fn eeprom_at(&self, addr: u32) -> bool {
        self.save.kind == save::Kind::Eeprom
            && addr >> 24 == 0x0D
            && (self.rom.len() <= 16 << 20 || addr & 0x00FF_FF00 == 0x00FF_FF00)
    }

    /// A ROM halfword at `o` (an offset into the 32 MB window, halfword
    /// aligned).
    #[inline(always)]
    fn rom16(&self, o: usize) -> u16 {
        if let Some(v) = self.gpio.read(o) {
            return v;
        }
        match self.rom.get(o..o + 2) {
            Some(b) => u16::from_le_bytes([b[0], b[1]]),
            None => (o >> 1) as u16,
        }
    }

    #[inline(always)]
    pub fn read8(&self, addr: u32) -> u8 {
        #[cfg(feature = "provenance")]
        crate::prov::read(1, addr);
        match addr >> 24 {
            0x02 => self.ewram[addr as usize & (EWRAM_SIZE - 1)],
            0x03 => self.iwram[addr as usize & (IWRAM_SIZE - 1)],
            0x04 => match io::offset(addr) {
                Some(o) => (self.io.read16(o & !1) >> ((o & 1) * 8)) as u8,
                None => self.open_bus_lane(addr, 0xFF) as u8,
            },
            0x05 => self.video.palette[addr as usize & (PALETTE_SIZE - 1)],
            0x06 => self.vram_offset(addr).map_or(0, |o| self.video.vram[o]),
            0x07 => self.video.oam[addr as usize & (OAM_SIZE - 1)],
            0x08..=0x0D => {
                let o = addr as usize & (ROM_MAX - 1);
                (self.rom16(o & !1) >> ((o & 1) * 8)) as u8
            }
            0x0E | 0x0F => self.save.read(addr as u16),
            0x00 if (addr as usize) < BIOS_SIZE => (self.bios_latch >> ((addr & 3) * 8)) as u8,
            _ => self.open_bus_lane(addr, 0xFF) as u8,
        }
    }

    #[inline(always)]
    pub fn read16(&self, addr: u32) -> u16 {
        #[cfg(feature = "provenance")]
        crate::prov::read(2, addr);
        let addr = addr & !1;
        match addr >> 24 {
            0x02 => le16(&self.ewram, addr as usize & (EWRAM_SIZE - 1)),
            0x03 => le16(&self.iwram, addr as usize & (IWRAM_SIZE - 1)),
            0x04 => match io::offset(addr) {
                Some(o) => self.io.read16(o),
                None => self.open_bus_lane(addr, 0xFFFF) as u16,
            },
            0x05 => le16(&self.video.palette, addr as usize & (PALETTE_SIZE - 1)),
            0x06 => self
                .vram_offset(addr)
                .map_or(0, |o| le16(&self.video.vram, o)),
            0x07 => le16(&self.video.oam, addr as usize & (OAM_SIZE - 1)),
            0x0D if self.eeprom_at(addr) => self.save.eeprom_status(),
            0x08..=0x0D => self.rom16(addr as usize & (ROM_MAX - 1)),
            // The save has an 8-bit bus: wider reads see the byte repeated.
            0x0E | 0x0F => self.save.read(addr as u16) as u16 * 0x0101,
            0x00 if (addr as usize) < BIOS_SIZE => (self.bios_latch >> ((addr & 2) * 8)) as u16,
            _ => self.open_bus_lane(addr, 0xFFFF) as u16,
        }
    }

    #[inline(always)]
    pub fn read32(&self, addr: u32) -> u32 {
        #[cfg(feature = "provenance")]
        crate::prov::read(4, addr);
        let addr = addr & !3;
        match addr >> 24 {
            0x02 => le32(&self.ewram, addr as usize & (EWRAM_SIZE - 1)),
            0x03 => le32(&self.iwram, addr as usize & (IWRAM_SIZE - 1)),
            0x04 => match io::offset(addr) {
                Some(o) => self.io.read16(o) as u32 | (self.io.read16(o + 2) as u32) << 16,
                None => self.open_bus,
            },
            0x05 => le32(&self.video.palette, addr as usize & (PALETTE_SIZE - 1)),
            0x06 => self
                .vram_offset(addr)
                .map_or(0, |o| le32(&self.video.vram, o)),
            0x07 => le32(&self.video.oam, addr as usize & (OAM_SIZE - 1)),
            0x08..=0x0D => {
                let o = addr as usize & (ROM_MAX - 1);
                self.rom16(o) as u32 | (self.rom16(o + 2) as u32) << 16
            }
            0x0E | 0x0F => self.save.read(addr as u16) as u32 * 0x0101_0101,
            0x00 if (addr as usize) < BIOS_SIZE => self.bios_latch,
            _ => self.open_bus,
        }
    }

    #[inline(always)]
    pub fn write8(&mut self, addr: u32, v: u8) {
        #[cfg(feature = "provenance")]
        crate::prov::write(1, addr, v as u32);
        match addr >> 24 {
            0x02 => self.ewram[addr as usize & (EWRAM_SIZE - 1)] = v,
            0x03 => {
                self.iwram[addr as usize & (IWRAM_SIZE - 1)] = v;
                self.overlays.wrote(addr as usize);
            }
            0x04 => {
                if let Some(o) = io::offset(addr) {
                    self.io_write(o, Width::Byte, v as u32);
                }
            }
            // Palette and VRAM take a byte store as a halfword of it.
            0x05 => {
                let o = addr as usize & (PALETTE_SIZE - 1) & !1;
                self.video.palette[o..o + 2].copy_from_slice(&[v, v]);
            }
            0x06 => {
                let bg_end = if self.bitmap_mode() {
                    0x1_4000
                } else {
                    0x1_0000
                };
                if let Some(o) = self.vram_offset(addr).filter(|&o| o < bg_end) {
                    let o = o & !1;
                    self.video.vram[o..o + 2].copy_from_slice(&[v, v]);
                }
            }
            0x0E | 0x0F => self.save.write(addr as u16, v),
            // BIOS, OAM (byte stores ignored), ROM, unmapped.
            _ => {}
        }
    }

    #[inline(always)]
    pub fn write16(&mut self, addr: u32, v: u16) {
        #[cfg(feature = "provenance")]
        crate::prov::write(2, addr, v as u32);
        let unaligned = addr;
        let addr = addr & !1;
        match addr >> 24 {
            0x02 => set16(&mut self.ewram, addr as usize & (EWRAM_SIZE - 1), v),
            0x03 => {
                set16(&mut self.iwram, addr as usize & (IWRAM_SIZE - 1), v);
                self.overlays.wrote(addr as usize);
            }
            0x04 => {
                if let Some(o) = io::offset(addr) {
                    self.io_write(o, Width::Half, v as u32);
                }
            }
            0x05 => set16(
                &mut self.video.palette,
                addr as usize & (PALETTE_SIZE - 1),
                v,
            ),
            0x06 => {
                if let Some(o) = self.vram_offset(addr) {
                    set16(&mut self.video.vram, o, v);
                }
            }
            0x07 => set16(&mut self.video.oam, addr as usize & (OAM_SIZE - 1), v),
            // Only halfword stores reach the GPIO port, and only in the
            // first ROM window (as in mGBA).
            0x08 => self.gpio.write(addr & 0xFF_FFFF, v),
            0x0D if self.eeprom_at(addr) => self.save.eeprom_write(v, 1),
            // The save's 8-bit bus stores the addressed byte lane, at the
            // unaligned address (as mGBA does).
            0x0E | 0x0F => self
                .save
                .write(unaligned as u16, (v >> ((unaligned & 1) * 8)) as u8),
            _ => {}
        }
    }

    #[inline(always)]
    pub fn write32(&mut self, addr: u32, v: u32) {
        #[cfg(feature = "provenance")]
        crate::prov::write(4, addr, v);
        let unaligned = addr;
        let addr = addr & !3;
        match addr >> 24 {
            0x02 => set32(&mut self.ewram, addr as usize & (EWRAM_SIZE - 1), v),
            0x03 => {
                set32(&mut self.iwram, addr as usize & (IWRAM_SIZE - 1), v);
                self.overlays.wrote(addr as usize);
            }
            0x04 => {
                if let Some(o) = io::offset(addr) {
                    self.io_write(o, Width::Word, v);
                }
            }
            0x05 => set32(
                &mut self.video.palette,
                addr as usize & (PALETTE_SIZE - 1),
                v,
            ),
            0x06 => {
                if let Some(o) = self.vram_offset(addr) {
                    set32(&mut self.video.vram, o, v);
                }
            }
            0x07 => set32(&mut self.video.oam, addr as usize & (OAM_SIZE - 1), v),
            0x0E | 0x0F => self
                .save
                .write(unaligned as u16, (v >> ((unaligned & 3) * 8)) as u8),
            _ => {}
        }
    }

    /// An I/O store. DMA and timer writes take effect here, in the store,
    /// not at the next sync: compiled code syncs once a block, and a block
    /// can set a DMA channel up twice (decision 11's "the device effect
    /// happens in the store"). Timer writes still leave their event, so
    /// the next tick reschedules overflows.
    #[inline(never)]
    fn io_write(&mut self, o: u32, w: Width, v: u32) {
        // The registers, the sound and the timers, as a native program's
        // library stores them too.
        sound::store(self, o, w.bytes(), v);
        if o & !3 == io::reg::WAITCNT {
            self.waitcnt_written();
        }
        let dmas = self.io.events & (0xF * event::DMA0);
        self.io.events &= !dmas;
        for n in 0..4 {
            if dmas & (event::DMA0 << n) != 0 {
                let c = dma::control_written(self, n);
                self.io.now += c as u64;
            }
        }
    }

    /// Whether overlay `i` may run ([`Overlays::valid`]).
    #[inline(always)]
    pub fn overlay_valid(&mut self, i: usize) -> bool {
        self.overlays.valid(i, &self.iwram, self.rom)
    }

    /// The open-bus value's lane for a narrow read at `addr`.
    #[inline(always)]
    fn open_bus_lane(&self, addr: u32, mask: u32) -> u32 {
        (self.open_bus >> ((addr & 3) * 8)) & mask
    }
}

impl armv4t::interp::Bus for Bus {
    // EWRAM and IWRAM inline; the rest of the map is one call, so that a
    // generated crate doesn't inline all of it at every access (rustc's
    // time on a part crate).
    #[inline(always)]
    fn load(&mut self, w: Width, addr: u32) -> u32 {
        match addr >> 24 {
            0x02 | 0x03 => self.load_any(w, addr),
            _ => self.load_slow(w, addr),
        }
    }

    #[inline(always)]
    fn store(&mut self, w: Width, addr: u32, v: u32) {
        match addr >> 24 {
            0x02 | 0x03 => self.store_any(w, addr, v),
            _ => self.store_slow(w, addr, v),
        }
    }
}

impl Bus {
    #[inline(always)]
    fn load_any(&mut self, w: Width, addr: u32) -> u32 {
        self.count_data(w.bytes(), addr);
        let v = match w {
            Width::Byte => self.read8(addr) as u32,
            Width::Half => self.read16(addr) as u32,
            Width::Word => self.read32(addr),
        };
        #[cfg(feature = "trace")]
        crate::trace::access(false, w.bytes(), addr, v);
        #[cfg(feature = "provenance")]
        crate::prov::cpu_load(w.bytes(), addr, v);
        v
    }

    #[inline(always)]
    fn store_any(&mut self, w: Width, addr: u32, v: u32) {
        self.count_data(w.bytes(), addr);
        #[cfg(feature = "trace")]
        crate::trace::access(true, w.bytes(), addr, v);
        match w {
            Width::Byte => self.write8(addr, v as u8),
            Width::Half => self.write16(addr, v as u16),
            Width::Word => self.write32(addr, v),
        }
    }

    #[inline(never)]
    fn load_slow(&mut self, w: Width, addr: u32) -> u32 {
        self.load_any(w, addr)
    }

    #[inline(never)]
    fn store_slow(&mut self, w: Width, addr: u32, v: u32) {
        self.store_any(w, addr, v)
    }
}

#[inline(always)]
fn le16(m: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([m[o], m[o + 1]])
}

#[inline(always)]
fn le32(m: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]])
}

#[inline(always)]
fn set16(m: &mut [u8], o: usize, v: u16) {
    m[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

#[inline(always)]
fn set32(m: &mut [u8], o: usize, v: u32) {
    m[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// The play build's sound path runs on the bus: its I/O block and sound
/// state, and DMA channels 1 and 2 refilling the FIFOs.
impl sound::Audio for Bus {
    #[inline(always)]
    fn parts(&mut self) -> (&mut Io, &mut Sound) {
        (&mut self.io, &mut self.sound)
    }

    fn fifo_refill(&mut self, fifo: usize) -> bool {
        dma::fifo_refill(self, fifo)
    }
}

#[cfg(test)]
mod tests;
