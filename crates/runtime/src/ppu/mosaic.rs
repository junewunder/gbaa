//! Mosaic (GBATEK "LCD I/O Mosaic Function"): backgrounds with `BGxCNT`
//! bit 6 and sprites with attribute 0 bit 12 are drawn in blocks, each
//! showing the pixel at its top-left corner.
//!
//! `MOSAIC` holds four sizes, each one less than the block's: background
//! width (bits 0-3) and height (4-7), sprite width (8-11) and height
//! (12-15). Blocks line up with the screen's (0, 0), not the layer's.
//!
//! - A background line `y` is drawn as the line its block starts on,
//!   `y - y % height`: text backgrounds by scrolling, affine and bitmap
//!   ones by stepping the reference point back by (`PB`, `PD`) per line,
//!   as mGBA does. [`hold`] then copies each block's first pixel across
//!   it, transparent or not.
//! - A sprite shows, on each of its lines, the row its block starts on,
//!   and in each column the column its block starts on, both clamped to
//!   the sprite's first (a block starting above or left of it shows its
//!   first row or column). A regular sprite's last block runs on past its
//!   right edge to the block's end; an affine sprite's block starting
//!   left of its box draws nothing there. Both as in mGBA; that lives in
//!   [`super::obj`].
//!
//! Windows and blending still work per pixel.

use super::Line;
use crate::platform::WIDTH;
use crate::video::Video;

/// The block sizes `MOSAIC` sets, in pixels (1-16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mosaic {
    pub bg_w: usize,
    pub bg_h: usize,
    pub obj_w: usize,
    pub obj_h: usize,
}

impl Mosaic {
    pub fn new(video: &Video) -> Mosaic {
        let v = video.mosaic as usize;
        Mosaic {
            bg_w: (v & 0xF) + 1,
            bg_h: (v >> 4 & 0xF) + 1,
            obj_w: (v >> 8 & 0xF) + 1,
            obj_h: (v >> 12) + 1,
        }
    }
}

/// Copy each `w`-pixel block's first pixel across the block.
pub fn hold(line: &mut Line, w: usize) {
    if w > 1 {
        for x in 0..WIDTH {
            line[x] = line[x - x % w];
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::super::dispcnt;
    use crate::bus::Bus;
    use crate::test_util::IO;
    use crate::test_util::ppu::*;
    use std::boxed::Box;

    const BG0CNT: u32 = IO + 0x8;
    const BG2CNT: u32 = IO + 0xC;
    const BG2PA: u32 = IO + 0x20;
    const MOSAIC: u32 = IO + 0x4C;
    const BACKDROP: u16 = 0x4210;

    /// Texel (c, r)'s colour in [`gradient`]'s tile: its own index.
    fn texel(c: u16, r: u16) -> u16 {
        1 + c + 8 * r
    }

    /// A bus with sprites hidden, a grey backdrop, and BG and OBJ palette
    /// entries 1-64 each their own index as a colour; the 8bpp tile at
    /// `addr` has texel (c, r) colour [`texel`]`(c, r)`.
    fn gradient(addr: u32) -> Box<Bus> {
        let mut b = bus();
        hide_objs(&mut b);
        b.write16(PAL, BACKDROP);
        for i in 1..=64 {
            b.write16(PAL + 2 * i, i as u16);
            b.write16(OBJ_PAL + 2 * i, i as u16);
        }
        let bytes: std::vec::Vec<u8> = (0..64).map(|i| texel(i % 8, i / 8) as u8).collect();
        store_bytes(&mut b, addr, &bytes);
        b
    }

    #[test]
    fn text_background_blocks_follow_the_screen() {
        // Tile 0 everywhere (the map at screen base 8 is zeroed).
        let mut b = gradient(VRAM);
        b.write16(BG0CNT, 1 << 7 | 8 << 8 | 1 << 6);
        // Blocks 4 wide, 3 tall.
        b.write16(MOSAIC, 0x23);
        b.write16(IO, dispcnt::BG0);
        let p = render(&b);
        assert_eq!(px(&p, 5, 7), texel(4, 6));
        assert_eq!(px(&p, 13, 2), texel(4, 0));
        assert_eq!(px(&p, 239, 159), texel(4, 7));
        // Scrolling moves the picture under the blocks, not the blocks.
        b.write16(IO + 0x10, 1);
        assert_eq!(px(&render(&b), 5, 7), texel(5, 6));
        // Bit 6 off: no mosaic.
        b.write16(BG0CNT, 1 << 7 | 8 << 8);
        b.write16(IO + 0x10, 0);
        assert_eq!(px(&render(&b), 5, 7), texel(5, 7));
    }

    #[test]
    fn transparent_block_starts_hold_too() {
        let mut b = gradient(VRAM + 64);
        // Map row 0: tile 0 (blank) then tile 1; blocks of 6 cover x 6-11
        // from x 6, which is tile 0's.
        b.write16(VRAM + 0x4002, 1);
        b.write16(BG0CNT, 1 << 7 | 8 << 8 | 1 << 6);
        b.write16(MOSAIC, 0x05);
        b.write16(IO, dispcnt::BG0);
        let p = render(&b);
        assert_eq!(px(&p, 11, 0), BACKDROP);
        assert_eq!(px(&p, 12, 0), texel(4, 0));
    }

    #[test]
    fn affine_background_steps_its_reference_point_back() {
        let mut b = gradient(VRAM);
        b.write16(BG2CNT, 8 << 8 | 1 << 6);
        b.write16(MOSAIC, 0x23);
        b.write16(IO, 2 | dispcnt::BG0 << 2);
        let p = render(&b);
        assert_eq!(px(&p, 5, 7), texel(4, 6));
        // Twice as tall: line 7 draws as line 6, map row 12.
        b.write16(BG2PA + 6, 0x200);
        assert_eq!(px(&render(&b), 5, 7), texel(4, 4));
    }

    #[test]
    fn bitmap_background() {
        let mut b = bus();
        b.write16(VRAM + (6 * 240 + 4) * 2, 0x1234);
        b.write16(BG2CNT, 1 << 6);
        b.write16(MOSAIC, 0x23);
        b.write16(IO, 3 | dispcnt::BG0 << 2);
        let p = render(&b);
        assert_eq!(px(&p, 5, 7), 0x1234);
        assert_eq!(px(&p, 7, 8), 0x1234);
        assert_eq!(px(&p, 8, 8), 0);
        assert_eq!(px(&p, 5, 9), 0);
    }

    /// Sprite 0: 8×8 8bpp at (3, 3), tile 0, mosaic, with `a1`'s flip or
    /// affine bits.
    fn sprite(b: &mut Bus, a0: u16, a1: u16) {
        b.write16(OAM, 3 | 1 << 13 | 1 << 12 | a0);
        b.write16(OAM + 2, 3 | a1);
        b.write16(OAM + 4, 0);
    }

    #[test]
    fn regular_sprite_blocks_clamp_to_its_first_texel() {
        let mut b = gradient(OBJ_VRAM);
        sprite(&mut b, 0, 0);
        b.write16(MOSAIC, 0x3300);
        b.write16(IO, dispcnt::OBJ | dispcnt::OBJ_1D);
        let p = render(&b);
        // Blocks start at screen x and y 0, 4, 8: the sprite's columns and
        // rows -3, 1, 5.
        assert_eq!(px(&p, 3, 3), texel(0, 0));
        assert_eq!(px(&p, 4, 4), texel(1, 1));
        assert_eq!(px(&p, 7, 7), texel(1, 1));
        assert_eq!(px(&p, 10, 10), texel(5, 5));
        assert_eq!(px(&p, 2, 3), BACKDROP);
        // The last block, x 8-11, runs past the sprite's edge at 10.
        assert_eq!(px(&p, 11, 10), texel(5, 5));
        assert_eq!(px(&p, 12, 10), BACKDROP);
        // The flip applies to the block's column.
        sprite(&mut b, 0, 1 << 12);
        assert_eq!(px(&render(&b), 10, 10), texel(2, 5));
        // Bit 12 off, the BG sizes ignored: column 7 flipped.
        b.write16(OAM, 3 | 1 << 13);
        b.write16(MOSAIC, 0x00FF);
        assert_eq!(px(&render(&b), 10, 10), texel(0, 7));
    }

    #[test]
    fn affine_sprite_block_left_of_its_box_draws_nothing() {
        let mut b = gradient(OBJ_VRAM);
        // Group 0, identity.
        b.write16(OAM + 6, 0x100);
        b.write16(OAM + 30, 0x100);
        sprite(&mut b, 1 << 8, 0);
        b.write16(MOSAIC, 0x3300);
        b.write16(IO, dispcnt::OBJ | dispcnt::OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 3, 4), BACKDROP);
        assert_eq!(px(&p, 4, 4), texel(1, 1));
        assert_eq!(px(&p, 10, 10), texel(5, 5));
    }
}
