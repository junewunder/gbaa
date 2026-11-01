//! The display as data: the display registers decoded into fields, and the
//! three memories the PPU reads (palette, VRAM, OAM). [`crate::ppu::Ppu`]
//! draws from a [`Video`] and nothing else, so one renderer serves both
//! builds (docs/native-port-design.md, section 2):
//!
//! - **The play build** keeps the GBA's memory map. [`crate::Bus`] owns a
//!   `Video` and routes the palette, VRAM and OAM regions into its arrays.
//!   The display registers stay stored in [`Io`] (its masks, its save
//!   state layout, its readers), and [`Video::load_regs`] decodes them into
//!   the fields before each line is drawn ([`crate::Bus::video_for_draw`]).
//!   Nothing the guest sees changed: its loads and stores still see bytes
//!   at addresses.
//! - **A native program** owns a `Video` directly ([`crate::hw::Hw::ppu`])
//!   and writes the fields: `hw.ppu.bg[0].scroll_x = x`. Code that
//!   addresses registers by offset (a table of register writes) goes
//!   through [`Video::write16`], which decodes the offset into the field
//!   with the register's write mask, as [`Io::write16`] stores it.
//!
//! Fields hold what the register holds, at its width: a scroll offset is
//! 9 bits on hardware, and the PPU masks it as hardware does, so a native
//! program writing 600 sees what a store of 600 shows.
//!
//! `DISPSTAT` and `VCOUNT` (offsets 4 and 6) are the beam's, not the
//! picture's: the scheduler owns them in the play build ([`Io`]) and
//! [`crate::hw::Hw`] in a native program.

use crate::io::{self, Io};

pub const PALETTE_SIZE: usize = 0x400;
pub const VRAM_SIZE: usize = 0x1_8000;
pub const OAM_SIZE: usize = 0x400;
/// The display registers' block: offsets `0x000`-`0x055` of the I/O block.
pub const REGS_END: u32 = 0x056;
/// Where sprite tiles start in VRAM (character blocks 4 and 5).
pub const OBJ_VRAM: usize = 0x1_0000;

/// `DISPCNT`: the mode, the layers on, and how sprites are mapped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DispCnt(pub u16);

impl DispCnt {
    pub const MODE_0: DispCnt = DispCnt(0);
    pub const MODE_1: DispCnt = DispCnt(1);
    pub const MODE_2: DispCnt = DispCnt(2);
    pub const MODE_3: DispCnt = DispCnt(3);
    pub const MODE_4: DispCnt = DispCnt(4);
    pub const MODE_5: DispCnt = DispCnt(5);
    /// Which page modes 4 and 5 show.
    pub const PAGE: DispCnt = DispCnt(crate::ppu::dispcnt::PAGE);
    pub const HBLANK_FREE: DispCnt = DispCnt(crate::ppu::dispcnt::HBLANK_FREE);
    /// Sprite tiles mapped 1D rather than as a 2D sheet.
    pub const OBJ_1D: DispCnt = DispCnt(crate::ppu::dispcnt::OBJ_1D);
    /// Every line white.
    pub const FORCED_BLANK: DispCnt = DispCnt(crate::ppu::dispcnt::FORCED_BLANK);
    pub const BG0: DispCnt = DispCnt(crate::ppu::dispcnt::BG0);
    pub const BG1: DispCnt = DispCnt(crate::ppu::dispcnt::BG0 << 1);
    pub const BG2: DispCnt = DispCnt(crate::ppu::dispcnt::BG0 << 2);
    pub const BG3: DispCnt = DispCnt(crate::ppu::dispcnt::BG0 << 3);
    pub const OBJ: DispCnt = DispCnt(crate::ppu::dispcnt::OBJ);
    pub const WIN0: DispCnt = DispCnt(crate::ppu::dispcnt::WIN0);
    pub const WIN1: DispCnt = DispCnt(crate::ppu::dispcnt::WIN1);
    pub const OBJ_WIN: DispCnt = DispCnt(crate::ppu::dispcnt::OBJ_WIN);

    /// The background mode, 0-7.
    pub fn mode(self) -> u16 {
        self.0 & crate::ppu::dispcnt::MODE
    }

    /// Whether every bit of `flags` is set.
    pub fn contains(self, flags: DispCnt) -> bool {
        self.0 & flags.0 == flags.0
    }
}

impl core::ops::BitOr for DispCnt {
    type Output = DispCnt;
    fn bitor(self, o: DispCnt) -> DispCnt {
        DispCnt(self.0 | o.0)
    }
}

impl core::ops::BitOrAssign for DispCnt {
    fn bitor_assign(&mut self, o: DispCnt) {
        self.0 |= o.0;
    }
}

/// `BGxCNT`, built up field by field:
/// `BgCnt::new().priority(1).char_base(0).screen_base(31)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BgCnt(pub u16);

impl BgCnt {
    pub const fn new() -> BgCnt {
        BgCnt(0)
    }
    /// Bits 0-1: priority, 0 in front.
    pub const fn priority(self, p: u16) -> BgCnt {
        BgCnt(self.0 & !3 | p & 3)
    }
    /// Bits 2-3: the 16 KB character block tiles start in.
    pub const fn char_base(self, b: u16) -> BgCnt {
        BgCnt(self.0 & !(3 << 2) | (b & 3) << 2)
    }
    /// Bit 6: mosaic.
    pub const fn mosaic(self, on: bool) -> BgCnt {
        BgCnt(self.0 & !(1 << 6) | (on as u16) << 6)
    }
    /// Bit 7: 256 colours, one palette (8bpp) rather than 16 of 16.
    pub const fn colors_256(self, on: bool) -> BgCnt {
        BgCnt(self.0 & !(1 << 7) | (on as u16) << 7)
    }
    /// Bits 8-12: the 2 KB screen block the map starts in.
    pub const fn screen_base(self, b: u16) -> BgCnt {
        BgCnt(self.0 & !(0x1F << 8) | (b & 0x1F) << 8)
    }
    /// Bit 13: an affine map wraps.
    pub const fn wrap(self, on: bool) -> BgCnt {
        BgCnt(self.0 & !(1 << 13) | (on as u16) << 13)
    }
    /// Bits 14-15: the map's size.
    pub const fn size(self, s: u16) -> BgCnt {
        BgCnt(self.0 & !(3 << 14) | (s & 3) << 14)
    }
    pub const fn get_priority(self) -> u16 {
        self.0 & 3
    }
}

/// One background's registers: `BGxCNT`, `BGxHOFS` and `BGxVOFS`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Bg {
    pub cnt: BgCnt,
    /// Write-only on hardware; 9 bits used.
    pub scroll_x: u16,
    pub scroll_y: u16,
}

/// BG2's or BG3's affine parameters: `PA`-`PD` (signed 8.8) and the
/// reference point `X`, `Y` (signed 20.8, 28 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Affine {
    pub pa: i16,
    pub pb: i16,
    pub pc: i16,
    pub pd: i16,
    pub x: i32,
    pub y: i32,
}

impl Default for Affine {
    /// The identity, as the BIOS leaves it.
    fn default() -> Affine {
        Affine {
            pa: 0x100,
            pb: 0,
            pc: 0,
            pd: 0x100,
            x: 0,
            y: 0,
        }
    }
}

/// A window's rectangle: `WINxH` (`left`, `right`) and `WINxV` (`top`,
/// `bottom`), right and bottom exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Window {
    pub left: u8,
    pub right: u8,
    pub top: u8,
    pub bottom: u8,
}

/// One OAM entry: its three attributes and the affine-parameter halfword
/// OAM interleaves with them (entry `4k + i` holds matrix `k`'s `i`th).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OamEntry {
    pub attr0: u16,
    pub attr1: u16,
    pub attr2: u16,
    pub affine: u16,
}

/// The display: its registers as fields, then palette, VRAM and OAM as
/// the bytes they are on the hardware.
#[derive(Clone)]
pub struct Video {
    pub dispcnt: DispCnt,
    /// Undocumented green swap (offset 2); stored, not drawn.
    pub green_swap: u16,
    pub bg: [Bg; 4],
    /// BG2's and BG3's affine parameters.
    pub affine: [Affine; 2],
    pub win: [Window; 2],
    /// `WININ`: low byte window 0's layers, high byte window 1's.
    pub winin: u16,
    /// `WINOUT`: low byte outside the windows, high byte the OBJ window.
    pub winout: u16,
    /// `MOSAIC`: BG width, height, OBJ width, height, a nibble each.
    pub mosaic: u16,
    pub bldcnt: u16,
    pub bldalpha: u16,
    pub bldy: u16,
    /// BG palette (bytes 0-0x1FF), then OBJ palette; little-endian BGR555.
    pub palette: [u8; PALETTE_SIZE],
    pub vram: [u8; VRAM_SIZE],
    pub oam: [u8; OAM_SIZE],
}

impl Default for Video {
    fn default() -> Video {
        Video::new()
    }
}

impl Video {
    /// After the BIOS hands over: memories zero, registers as [`Io::new`]
    /// leaves them.
    pub fn new() -> Video {
        Video {
            dispcnt: DispCnt(0),
            green_swap: 0,
            bg: [Bg::default(); 4],
            affine: [Affine::default(); 2],
            win: [Window::default(); 2],
            winin: 0,
            winout: 0,
            mosaic: 0,
            bldcnt: 0,
            bldalpha: 0,
            bldy: 0,
            palette: [0; PALETTE_SIZE],
            vram: [0; VRAM_SIZE],
            oam: [0; OAM_SIZE],
        }
    }

    /// Decode the display registers from `io`'s stored halfwords: what the
    /// play build does before drawing each line.
    pub fn load_regs(&mut self, io: &Io) {
        let mut o = 0;
        while o < REGS_END {
            if o != io::reg::DISPSTAT && o != io::reg::VCOUNT {
                self.set_raw16(o, io.raw16(o));
            }
            o += 2;
        }
    }

    /// The register halfword at offset `o` (even, below [`REGS_END`]) as
    /// stored, write-only bits included (what [`Io::raw16`] gives).
    /// `DISPSTAT`, `VCOUNT` and unused offsets read 0.
    pub fn raw16(&self, o: u32) -> u16 {
        let hi = |v: i32| ((v as u32) >> 16) as u16 & 0x0FFF;
        match o {
            0x000 => self.dispcnt.0,
            0x002 => self.green_swap,
            0x008..=0x00E => self.bg[(o as usize - 8) / 2].cnt.0,
            0x010..=0x01E => {
                let bg = &self.bg[(o as usize - 0x10) / 4];
                if o & 2 == 0 { bg.scroll_x } else { bg.scroll_y }
            }
            0x020..=0x03E => {
                let a = &self.affine[(o as usize - 0x20) / 0x10];
                match o & 0xF {
                    0x0 => a.pa as u16,
                    0x2 => a.pb as u16,
                    0x4 => a.pc as u16,
                    0x6 => a.pd as u16,
                    0x8 => a.x as u16,
                    0xA => hi(a.x),
                    0xC => a.y as u16,
                    _ => hi(a.y),
                }
            }
            0x040 | 0x042 => {
                let w = &self.win[(o as usize - 0x40) / 2];
                (w.left as u16) << 8 | w.right as u16
            }
            0x044 | 0x046 => {
                let w = &self.win[(o as usize - 0x44) / 2];
                (w.top as u16) << 8 | w.bottom as u16
            }
            0x048 => self.winin,
            0x04A => self.winout,
            0x04C => self.mosaic,
            0x050 => self.bldcnt,
            0x052 => self.bldalpha,
            0x054 => self.bldy,
            _ => 0,
        }
    }

    /// Store the register halfword at `o` as is, no masks: the inverse of
    /// [`Video::raw16`]. A reference point's high half is sign-extended
    /// from its bit 11, as the PPU reads it.
    pub fn set_raw16(&mut self, o: u32, v: u16) {
        let lo = |old: i32| (old & !0xFFFF) | v as i32;
        let hi = |old: i32| ((((v as u32 & 0x0FFF) << 20) as i32) >> 4) | (old & 0xFFFF);
        match o {
            0x000 => self.dispcnt = DispCnt(v),
            0x002 => self.green_swap = v,
            0x008..=0x00E => self.bg[(o as usize - 8) / 2].cnt = BgCnt(v),
            0x010..=0x01E => {
                let bg = &mut self.bg[(o as usize - 0x10) / 4];
                if o & 2 == 0 {
                    bg.scroll_x = v;
                } else {
                    bg.scroll_y = v;
                }
            }
            0x020..=0x03E => {
                let a = &mut self.affine[(o as usize - 0x20) / 0x10];
                match o & 0xF {
                    0x0 => a.pa = v as i16,
                    0x2 => a.pb = v as i16,
                    0x4 => a.pc = v as i16,
                    0x6 => a.pd = v as i16,
                    0x8 => a.x = lo(a.x),
                    0xA => a.x = hi(a.x),
                    0xC => a.y = lo(a.y),
                    _ => a.y = hi(a.y),
                }
            }
            0x040 | 0x042 => {
                let w = &mut self.win[(o as usize - 0x40) / 2];
                (w.left, w.right) = ((v >> 8) as u8, v as u8);
            }
            0x044 | 0x046 => {
                let w = &mut self.win[(o as usize - 0x44) / 2];
                (w.top, w.bottom) = ((v >> 8) as u8, v as u8);
            }
            0x048 => self.winin = v,
            0x04A => self.winout = v,
            0x04C => self.mosaic = v,
            0x050 => self.bldcnt = v,
            0x052 => self.bldalpha = v,
            0x054 => self.bldy = v,
            _ => {}
        }
    }

    /// Write the register halfword at `o` as the CPU does: through its
    /// write mask, as [`Io::write16`] stores it.
    pub fn write16(&mut self, o: u32, v: u16) {
        let m = io::write_mask(o);
        self.set_raw16(o, (self.raw16(o) & !m) | (v & m));
    }

    /// Write one byte of a register, the other byte left alone.
    pub fn write8(&mut self, o: u32, v: u8) {
        let shift = (o & 1) * 8;
        let m = io::write_mask(o & !1) & (0xFF << shift);
        let o = o & !1;
        self.set_raw16(o, (self.raw16(o) & !m) | (((v as u16) << shift) & m));
    }

    /// Read the register halfword at `o` as the CPU does: through its read
    /// mask (write-only registers read 0).
    pub fn read16(&self, o: u32) -> u16 {
        self.raw16(o) & io::read_mask(o)
    }

    /// Colour `i` of the BG palette (0-255), then the OBJ palette's
    /// (256-511), BGR555.
    pub fn color(&self, i: usize) -> u16 {
        u16::from_le_bytes([self.palette[2 * i], self.palette[2 * i + 1]])
    }

    pub fn set_color(&mut self, i: usize, c: u16) {
        self.palette[2 * i..2 * i + 2].copy_from_slice(&c.to_le_bytes());
    }

    /// Colours from `i` on: the palette half of a DMA or `CpuSet`.
    pub fn copy_colors(&mut self, i: usize, src: &[u16]) {
        for (k, &c) in src.iter().enumerate() {
            self.set_color(i + k, c);
        }
    }

    /// The VRAM halfword at byte offset `off` (even).
    pub fn vram16(&self, off: usize) -> u16 {
        u16::from_le_bytes([self.vram[off], self.vram[off + 1]])
    }

    pub fn set_vram16(&mut self, off: usize, v: u16) {
        self.vram[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }

    /// Halfwords into VRAM from byte offset `off` (even): a copy to VRAM.
    pub fn copy_vram16(&mut self, off: usize, src: &[u16]) {
        for (k, &h) in src.iter().enumerate() {
            self.set_vram16(off + 2 * k, h);
        }
    }

    /// `n` halfwords of `v` into VRAM from byte offset `off`: a fill.
    pub fn fill_vram16(&mut self, off: usize, n: usize, v: u16) {
        for k in 0..n {
            self.set_vram16(off + 2 * k, v);
        }
    }

    /// Byte offset of 4bpp tile `t` of character block `cb` (BG: 0-3;
    /// OBJ: 4-5): 32 bytes a tile.
    pub const fn tile4(cb: usize, t: usize) -> usize {
        cb * 0x4000 + t * 32
    }

    /// Byte offset of 2 KB screen block `n` (0-31).
    pub const fn screen_block(n: usize) -> usize {
        n * 0x800
    }

    /// OAM entry `i` (0-127).
    pub fn obj(&self, i: usize) -> OamEntry {
        let h =
            |k: usize| u16::from_le_bytes([self.oam[8 * i + 2 * k], self.oam[8 * i + 2 * k + 1]]);
        OamEntry {
            attr0: h(0),
            attr1: h(1),
            attr2: h(2),
            affine: h(3),
        }
    }

    pub fn set_obj(&mut self, i: usize, e: OamEntry) {
        for (k, h) in [e.attr0, e.attr1, e.attr2, e.affine]
            .into_iter()
            .enumerate()
        {
            self.oam[8 * i + 2 * k..8 * i + 2 * k + 2].copy_from_slice(&h.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests;
