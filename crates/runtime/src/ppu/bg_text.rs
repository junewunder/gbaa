//! Text backgrounds: the tiled, scrolling BG0-BG3 of mode 0 and BG0-BG1 of
//! mode 1 (GBATEK "LCD VRAM Character Data" and "BG Map Data").
//!
//! `BGxCNT` says where things are and how they're coloured:
//!
//! | bits  | field                                          |
//! |-------|------------------------------------------------|
//! | 0-1   | priority (read by the compose step)            |
//! | 2-3   | character base, in 16 KB blocks                |
//! | 7     | 0: 4bpp tiles in 16 banks of 16 colours; 1: 8bpp, 256 colours |
//! | 8-12  | screen base, in 2 KB blocks                    |
//! | 14-15 | size: 256×256, 512×256, 256×512, 512×512       |
//!
//! The map is one to four 32×32-entry screen blocks; a 512-wide map puts
//! the right half in the next block, a 512-tall one the bottom half after
//! the top's. Each entry is a tile number (bits 0-9), horizontal and
//! vertical flips (10, 11) and, for 4bpp tiles, a palette bank (12-15).
//! `BGxHOFS`/`VOFS` (9 bits) scroll the map, which wraps. Colour 0 of a
//! tile is transparent. Tiles reaching past the 64 KB background area
//! (into sprite VRAM) draw nothing, as on hardware.
//!
//! Bit 6 (mosaic) is the caller's: it picks the line and holds pixels
//! ([`super::mosaic`]).

use super::Line;
use crate::video::Video;

/// Where background tiles and maps may live: the first 64 KB of VRAM.
const BG_VRAM: usize = 0x1_0000;

/// Draw text background `n` for line `y` into `out`.
pub fn line(v: &Video, n: usize, y: usize, out: &mut Line) {
    let bg = &v.bg[n];
    let cnt = bg.cnt.0;
    let hofs = (bg.scroll_x & 0x1FF) as usize;
    let vofs = (bg.scroll_y & 0x1FF) as usize;
    let char_base = ((cnt >> 2) & 3) as usize * 0x4000;
    let screen_base = ((cnt >> 8) & 0x1F) as usize * 0x800;
    let bpp8 = cnt & (1 << 7) != 0;
    let size = cnt >> 14;
    let wide = size & 1 != 0;
    let tall = size & 2 != 0;
    let width = if wide { 512 } else { 256 };
    let height = if tall { 512 } else { 256 };

    let my = (y + vofs) % height;
    let ty = my / 8;
    // Screen blocks before this row's: the top half's one or two.
    let row_block = ty / 32 * if wide { 2 } else { 1 };
    let vram = &v.vram;

    for (x, px) in out.iter_mut().enumerate() {
        let mx = (x + hofs) % width;
        let tx = mx / 8;
        let block = row_block + tx / 32;
        let at = (screen_base + block * 0x800 + ((ty % 32) * 32 + tx % 32) * 2) % BG_VRAM;
        let entry = u16::from_le_bytes([vram[at], vram[at + 1]]);
        let tile = (entry & 0x3FF) as usize;
        let mut col = mx % 8;
        let mut row = my % 8;
        if entry & (1 << 10) != 0 {
            col = 7 - col;
        }
        if entry & (1 << 11) != 0 {
            row = 7 - row;
        }
        let index = if bpp8 {
            let a = char_base + tile * 64 + row * 8 + col;
            if a >= BG_VRAM {
                continue;
            }
            vram[a] as usize
        } else {
            let a = char_base + tile * 32 + row * 4 + col / 2;
            if a >= BG_VRAM {
                continue;
            }
            let i = (vram[a] >> ((col & 1) * 4)) & 0xF;
            if i == 0 {
                continue;
            }
            (entry >> 12) as usize * 16 + i as usize
        };
        if index == 0 {
            continue;
        }
        let o = index * 2;
        *px = u16::from_le_bytes([v.palette[o], v.palette[o + 1]]) & 0x7FFF;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::super::dispcnt;
    use crate::bus::Bus;
    use crate::test_util::IO;
    use crate::test_util::ppu::*;

    const BG0CNT: u32 = IO + 0x8;
    const BG0HOFS: u32 = IO + 0x10;

    /// `BGxCNT` with character base `cb`, screen base `sb` and size `size`.
    fn cnt(prio: u16, cb: u16, sb: u16, bpp8: bool, size: u16) -> u16 {
        prio | cb << 2 | (bpp8 as u16) << 7 | sb << 8 | size << 14
    }

    /// Fill 4bpp tile `t` at character base `cb` with colour `c`.
    fn fill4(b: &mut Bus, cb: u32, t: u32, c: u8) {
        tile4(b, VRAM + cb * 0x4000 + t * 32, c);
    }

    /// Map entry `(tx, ty)` of screen block `sb`.
    fn map(b: &mut Bus, sb: u32, tx: u32, ty: u32, e: u16) {
        b.write16(VRAM + sb * 0x800 + (ty * 32 + tx) * 2, e);
    }

    #[test]
    fn tiles_4bpp_banks_and_transparency() {
        let mut b = bus();
        b.write16(PAL, 0x0042);
        b.write16(PAL + 2 * 3, 0x001F);
        b.write16(PAL + 2 * (2 * 16 + 3), 0x03E0);
        fill4(&mut b, 1, 5, 3);
        map(&mut b, 2, 0, 0, 5);
        map(&mut b, 2, 1, 0, 5 | 2 << 12);
        // Tile 0 is all colour 0: transparent.
        b.write16(BG0CNT, cnt(0, 1, 2, false, 0));
        b.write16(IO, dispcnt::BG0);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 0x001F);
        assert_eq!(px(&p, 7, 7), 0x001F);
        assert_eq!(px(&p, 8, 0), 0x03E0);
        assert_eq!(px(&p, 16, 0), 0x0042);
        assert_eq!(px(&p, 0, 8), 0x0042);
    }

    #[test]
    fn tiles_8bpp() {
        let mut b = bus();
        b.write16(PAL + 2 * 200, 0x7C00);
        // Tile 1, row 2: a byte store fills columns 2 and 3.
        b.write8(VRAM + 64 + 2 * 8 + 3, 200);
        // 8bpp ignores the palette bank bits.
        map(&mut b, 4, 0, 0, 1 | 5 << 12);
        b.write16(BG0CNT + 2, cnt(0, 0, 4, true, 0));
        b.write16(IO, dispcnt::BG0 << 1);
        let p = render(&b);
        assert_eq!(px(&p, 3, 2), 0x7C00);
        assert_eq!(px(&p, 4, 2), 0);
    }

    #[test]
    fn flips() {
        let mut b = bus();
        b.write16(PAL + 2, 0x001F);
        // Tile 1: only its top-left pixel.
        b.write8(VRAM + 32, 0x01);
        map(&mut b, 8, 0, 0, 1 | 1 << 10);
        map(&mut b, 8, 1, 0, 1 | 1 << 11);
        map(&mut b, 8, 2, 0, 1 | 3 << 10);
        b.write16(BG0CNT, cnt(0, 0, 8, false, 0));
        b.write16(IO, dispcnt::BG0);
        let p = render(&b);
        assert_eq!(px(&p, 7, 0), 0x001F);
        assert_eq!(px(&p, 0, 0), 0);
        assert_eq!(px(&p, 8, 7), 0x001F);
        assert_eq!(px(&p, 23, 7), 0x001F);
    }

    #[test]
    fn scrolling_wraps_and_big_maps() {
        let mut b = bus();
        for (c, colour) in [(1, 0x0001), (2, 0x0002), (3, 0x0003), (4, 0x0004)] {
            b.write16(PAL + 2 * c, colour);
            fill4(&mut b, 0, c, c as u8);
        }
        // 512x512: blocks 16 (top left), 17 (top right), 18, 19.
        for (i, sb) in (16..20).enumerate() {
            map(&mut b, sb, 0, 0, 1 + i as u16);
        }
        b.write16(BG0CNT, cnt(0, 0, 16, false, 3));
        b.write16(IO, dispcnt::BG0);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 1);
        b.write16(BG0HOFS, 256);
        assert_eq!(px(&render(&b), 0, 0), 2);
        b.write16(BG0HOFS + 2, 256);
        assert_eq!(px(&render(&b), 0, 0), 4);
        b.write16(BG0HOFS, 0);
        assert_eq!(px(&render(&b), 0, 0), 3);
        // Scrolling by 4 then wrapping: map x 508 is in block 17's
        // column 31, and the screen's x 4 is map x 0.
        b.write16(BG0HOFS, 508);
        b.write16(BG0HOFS + 2, 0);
        assert_eq!(px(&render(&b), 4, 0), 1);
        assert_eq!(px(&render(&b), 3, 0), 0);
        // 256x256 wraps at 256: block 17 never shows.
        b.write16(BG0CNT, cnt(0, 0, 16, false, 0));
        b.write16(BG0HOFS, 256);
        assert_eq!(px(&render(&b), 0, 0), 1);
        // 256x512: the bottom half is the next block.
        b.write16(BG0CNT, cnt(0, 0, 16, false, 2));
        b.write16(BG0HOFS, 0);
        b.write16(BG0HOFS + 2, 256);
        assert_eq!(px(&render(&b), 0, 0), 2);
    }

    #[test]
    fn priority_between_backgrounds() {
        let mut b = bus();
        b.write16(PAL + 2, 0x0001);
        b.write16(PAL + 4, 0x0002);
        fill4(&mut b, 0, 1, 1);
        fill4(&mut b, 0, 2, 2);
        map(&mut b, 8, 0, 0, 1);
        map(&mut b, 9, 0, 0, 2);
        map(&mut b, 9, 1, 0, 2);
        b.write16(BG0CNT, cnt(1, 0, 8, false, 0));
        b.write16(BG0CNT + 6, cnt(0, 0, 9, false, 0));
        b.write16(IO, dispcnt::BG0 | dispcnt::BG0 << 3);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 2);
        // Equal priority: the lower BG number wins.
        b.write16(BG0CNT + 6, cnt(1, 0, 9, false, 0));
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 1);
        assert_eq!(px(&p, 8, 0), 2);
    }

    #[test]
    fn each_background_scrolls_by_its_own_registers() {
        let mut b = bus();
        b.write16(PAL + 2, 0x001F);
        fill4(&mut b, 0, 1, 1);
        // Tile 1 at map (1, 2) of BG3 shows at the screen's (0, 0) when
        // BG3 is scrolled by (8, 16) and BG0 is not.
        map(&mut b, 9, 1, 2, 1);
        b.write16(BG0CNT + 6, cnt(0, 0, 9, false, 0));
        b.write16(BG0HOFS + 12, 8);
        b.write16(BG0HOFS + 14, 16);
        b.write16(BG0HOFS, 100);
        b.write16(IO, dispcnt::BG0 << 3);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 0x001F);
        assert_eq!(px(&p, 8, 0), 0);
        // Wrapping vertically mid-screen: map row 0 at screen line 152.
        b.write16(BG0HOFS + 12, 0);
        b.write16(BG0HOFS + 14, 256 - 152);
        map(&mut b, 9, 0, 0, 1);
        let p = render(&b);
        assert_eq!(px(&p, 0, 152), 0x001F);
        assert_eq!(px(&p, 0, 151), 0);
    }

    #[test]
    fn mode1_text_layers() {
        let mut b = bus();
        b.write16(PAL + 2, 0x001F);
        b.write16(PAL + 4, 0x03E0);
        fill4(&mut b, 0, 1, 1);
        fill4(&mut b, 0, 2, 2);
        map(&mut b, 8, 0, 0, 1);
        map(&mut b, 9, 1, 0, 2);
        b.write16(BG0CNT, cnt(0, 0, 8, false, 0));
        b.write16(BG0CNT + 2, cnt(0, 0, 9, false, 0));
        b.write16(IO, 1 | dispcnt::BG0 | dispcnt::BG0 << 1);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 0x001F);
        assert_eq!(px(&p, 8, 0), 0x03E0);
    }

    #[test]
    fn tiles_past_background_vram_are_blank() {
        let mut b = bus();
        b.write16(PAL + 2, 0x001F);
        // Character base 3 (0xC000), tile 512: 0x10000, sprite VRAM.
        b.write16(VRAM + 0x1_0000, 0x1111);
        assert_eq!(b.video.vram[0x1_0000], 0x11);
        map(&mut b, 0, 0, 0, 512);
        b.write16(BG0CNT, cnt(0, 3, 0, false, 0));
        b.write16(IO, dispcnt::BG0);
        assert_eq!(px(&render(&b), 0, 0), 0);
    }
}
