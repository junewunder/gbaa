//! Regular sprites (GBATEK "LCD OBJ - OAM Attributes" and "OBJ Tile
//! Mapping").
//!
//! OAM holds 128 sprites of three attribute halfwords each (every fourth
//! halfword is affine data):
//!
//! | attr | bits  | field                                                |
//! |------|-------|------------------------------------------------------|
//! | 0    | 0-7   | y, wrapping at 256                                   |
//! | 0    | 8     | affine                                               |
//! | 0    | 9     | regular sprite: hidden; affine: double-size box      |
//! | 0    | 10-11 | mode: normal, semi-transparent, OBJ window, prohibited |
//! | 0    | 13    | 0: 4bpp in 16 banks of 16 colours; 1: 8bpp, 256 colours |
//! | 0    | 14-15 | shape: square, wide, tall, prohibited                |
//! | 1    | 0-8   | x, signed                                            |
//! | 1    | 9-13  | affine: parameter group, 0-31                        |
//! | 1    | 12-13 | regular: horizontal and vertical flips               |
//! | 1    | 14-15 | size, which with the shape picks one of 12 sizes     |
//! | 2    | 0-9   | first tile, in 32-byte units from sprite VRAM's start |
//! | 2    | 10-11 | priority against the backgrounds                     |
//! | 2    | 12-15 | palette bank, for 4bpp                               |
//!
//! Sprite tiles live in the 32 KB after the background area; the bitmap
//! modes use its first 16 KB for the picture, so there tiles below 512 draw
//! nothing. `DISPCNT` bit 6 picks 1D mapping, where a sprite's tiles follow
//! each other, or 2D, where sprite VRAM is a 32-tile-wide sheet. 8bpp tiles
//! take two units each. Colours come from the sprite palette, the second
//! half of palette RAM; colour 0 is transparent.
//!
//! Among sprites, the lowest OAM number with an opaque pixel wins unless a
//! later sprite has a strictly lower priority value (what most emulators
//! settle on). The compose step then puts the winning pixel in front of
//! every background whose priority value is the same or higher.
//!
//! The hardware has 1210 cycles per line to fetch sprites (954 when
//! `DISPCNT` bit 5 frees H-blank); a regular sprite on the line costs its
//! width, an affine one 10 + 2 × its box's width. Sprites past the budget
//! aren't drawn. That is approximate: the real fetch also spends cycles on
//! sprites outside the line, and the hardware fetches each line's sprites during the line before, so an OAM
//! change mid-frame shows here one line early.
//!
//! In 2D mapping an 8bpp sprite's tile number ignores bit 0. In the bitmap
//! modes the cutoff is per tile, so a sprite starting below tile 512 still
//! draws its tiles from 512 on (mGBA drops the whole sprite).
//!
//! A sprite in OBJ-window mode draws nothing; its opaque pixels mark
//! [`ObjLine::window`] instead (whatever their priority), for
//! [`super::window`]. A semi-transparent sprite's pixels are marked in
//! [`ObjLine::semi`], for [`super::blend`]. Attribute 0 bit 12 draws the
//! sprite in mosaic blocks ([`super::mosaic`]).
//!
//! An affine sprite (GBATEK "LCD OBJ - OAM Rotation/Scaling Parameters")
//! draws a box the sprite's size, or twice each way with the double-size
//! bit. Each box pixel's offset from the box's centre goes through the
//! matrix `PA`-`PD` (signed 8.8, the fourth halfword of OAM entries
//! 4g to 4g + 3 for parameter group g) to an offset from the sprite's
//! centre; offsets landing outside the sprite draw nothing.

use super::mosaic::Mosaic;
use super::{Line, TRANSPARENT, dispcnt};
use crate::platform::WIDTH;
use crate::video::Video;

/// Where sprite VRAM starts.
const OBJ_VRAM: usize = 0x1_0000;
/// Its size.
const OBJ_VRAM_SIZE: usize = 0x8000;
/// What the bitmap modes take of it.
const BITMAP_TAKES: usize = 0x4000;
/// Where sprite colours start in palette RAM.
const OBJ_PALETTE: usize = 0x200;

/// Fetch cycles per line, and with H-blank free.
pub const BUDGET: i32 = 1210;
pub const BUDGET_HBLANK_FREE: i32 = 954;

/// Width and height by shape (rows) and size (columns).
pub const SIZES: [[(usize, usize); 4]; 3] = [
    [(8, 8), (16, 16), (32, 32), (64, 64)],
    [(16, 8), (32, 8), (32, 16), (64, 32)],
    [(8, 16), (8, 32), (16, 32), (32, 64)],
];

/// The sprite layer for one line: each pixel's colour (or
/// [`TRANSPARENT`]), the priority of the sprite that drew it and whether
/// that sprite is semi-transparent, and which pixels are in the OBJ window.
pub struct ObjLine {
    pub colour: Line,
    pub priority: [u8; WIDTH],
    pub semi: [bool; WIDTH],
    pub window: [bool; WIDTH],
}

impl ObjLine {
    pub fn new() -> ObjLine {
        ObjLine {
            colour: [TRANSPARENT; WIDTH],
            priority: [4; WIDTH],
            semi: [false; WIDTH],
            window: [false; WIDTH],
        }
    }
}

impl Default for ObjLine {
    fn default() -> ObjLine {
        ObjLine::new()
    }
}

/// Draw the sprites on line `y` into `out`, given `DISPCNT` as `cnt` and
/// `MOSAIC`'s block sizes.
pub fn line(v: &Video, cnt: u16, y: usize, mosaic: &Mosaic, out: &mut ObjLine) {
    let one_d = cnt & dispcnt::OBJ_1D != 0;
    let bitmap = (cnt & dispcnt::MODE) >= 3;
    let mut budget = if cnt & dispcnt::HBLANK_FREE != 0 {
        BUDGET_HBLANK_FREE
    } else {
        BUDGET
    };
    let oam = &v.oam;
    let half = |at: usize| u16::from_le_bytes([oam[at], oam[at + 1]]);
    for s in 0..128 {
        let at = s * 8;
        let a0 = half(at);
        // Bit 9 hides a regular sprite and doubles an affine one's box.
        let affine = a0 & (1 << 8) != 0;
        if !affine && a0 & (1 << 9) != 0 {
            continue;
        }
        let shape = (a0 >> 14) as usize;
        if shape == 3 {
            continue;
        }
        let a1 = half(at + 2);
        let a2 = half(at + 4);
        let (w, h) = SIZES[shape][(a1 >> 14) as usize];
        let (bw, bh) = if affine && a0 & (1 << 9) != 0 {
            (w * 2, h * 2)
        } else {
            (w, h)
        };
        let row = (y + 256 - (a0 & 0xFF) as usize) % 256;
        if row >= bh {
            continue;
        }
        budget -= if affine { 10 + 2 * bw as i32 } else { w as i32 };
        if budget < 0 {
            break;
        }
        // Mosaic: the row the block starts on, but not above the sprite.
        let mosaic_on = a0 & (1 << 12) != 0;
        let row = if mosaic_on {
            row.saturating_sub(y % mosaic.obj_h)
        } else {
            row
        };
        let mode = (a0 >> 10) & 3;
        let x0 = (a1 & 0x1FF) as i32 - if a1 & 0x100 != 0 { 512 } else { 0 };
        let bpp8 = a0 & (1 << 13) != 0;
        // An affine sprite's matrix, from its parameter group (bits 9-13
        // of attribute 1, which a regular sprite uses for its flips).
        let matrix = affine.then(|| {
            let g = ((a1 >> 9) & 0x1F) as usize * 32;
            [6, 14, 22, 30].map(|o| half(g + o) as i16 as i32)
        });
        let hflip = a1 & (1 << 12) != 0;
        let vflip = a1 & (1 << 13) != 0;
        let tile = (a2 & 0x3FF) as usize;
        let tile = if bpp8 && !one_d { tile & !1 } else { tile };
        let priority = ((a2 >> 10) & 3) as u8;
        let bank = (a2 >> 12) as usize;
        let units = if bpp8 { 2 } else { 1 };
        let stride = if one_d { w / 8 * units } else { 32 };

        // A regular sprite's last mosaic block runs on to the block's end,
        // past the sprite's right edge.
        let across = if mosaic_on && !affine {
            let last = x0 + w as i32 - 1;
            bw + (mosaic.obj_w - 1 - last.rem_euclid(mosaic.obj_w as i32) as usize)
        } else {
            bw
        };
        for col in 0..across {
            let x = x0 + col as i32;
            if !(0..WIDTH as i32).contains(&x) {
                continue;
            }
            let x = x as usize;
            // Mosaic: the column the block starts on; a regular sprite
            // clamps it to its first, an affine one draws nothing.
            let col = if mosaic_on {
                let c = x as i32 - (x % mosaic.obj_w) as i32 - x0;
                match (c < 0, affine) {
                    (false, _) => c as usize,
                    (true, false) => 0,
                    (true, true) => continue,
                }
            } else {
                col
            };
            let window = mode == 2;
            if window && out.window[x]
                || !window && out.colour[x] != TRANSPARENT && out.priority[x] <= priority
            {
                continue;
            }
            // The texel (c, r) this pixel shows. An affine sprite maps its
            // box's centre to the sprite's and the rest through the
            // matrix; what lands outside the sprite draws nothing.
            let (c, r) = match matrix {
                Some([pa, pb, pc, pd]) => {
                    let dx = col as i32 - (bw / 2) as i32;
                    let dy = row as i32 - (bh / 2) as i32;
                    let c = ((pa * dx + pb * dy) >> 8) + (w / 2) as i32;
                    let r = ((pc * dx + pd * dy) >> 8) + (h / 2) as i32;
                    if !(0..w as i32).contains(&c) || !(0..h as i32).contains(&r) {
                        continue;
                    }
                    (c as usize, r as usize)
                }
                None => (
                    if hflip { w - 1 - col } else { col },
                    if vflip { h - 1 - row } else { row },
                ),
            };
            let unit = tile + r / 8 * stride + c / 8 * units;
            let within = if bpp8 {
                (r % 8) * 8 + c % 8
            } else {
                (r % 8) * 4 + c % 8 / 2
            };
            let off = (unit * 32 + within) % OBJ_VRAM_SIZE;
            if bitmap && off < BITMAP_TAKES {
                continue;
            }
            let byte = v.vram[OBJ_VRAM + off];
            let index = if bpp8 {
                byte as usize
            } else {
                let i = (byte >> ((c & 1) * 4)) & 0xF;
                if i == 0 {
                    continue;
                }
                bank * 16 + i as usize
            };
            if index == 0 {
                continue;
            }
            if window {
                out.window[x] = true;
                continue;
            }
            let o = OBJ_PALETTE + index * 2;
            out.colour[x] = u16::from_le_bytes([v.palette[o], v.palette[o + 1]]) & 0x7FFF;
            out.priority[x] = priority;
            out.semi[x] = mode == 1;
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::super::dispcnt;
    use super::SIZES;
    use crate::bus::Bus;
    use crate::platform::WIDTH;
    use crate::test_util::IO;
    use crate::test_util::ppu::*;
    use std::boxed::Box;

    const BG0CNT: u32 = IO + 0x8;
    const OBJ_1D: u16 = dispcnt::OBJ | dispcnt::OBJ_1D;

    /// The three attributes of a regular sprite.
    struct Obj {
        x: i32,
        y: i32,
        shape: u16,
        size: u16,
        tile: u16,
        bpp8: bool,
        prio: u16,
        bank: u16,
        flips: u16,
        a0: u16,
    }

    impl Obj {
        fn new(x: i32, y: i32, tile: u16) -> Obj {
            Obj {
                x,
                y,
                shape: 0,
                size: 0,
                tile,
                bpp8: false,
                prio: 0,
                bank: 0,
                flips: 0,
                a0: 0,
            }
        }

        fn sized(self, shape: u16, size: u16) -> Obj {
            Obj {
                shape,
                size,
                ..self
            }
        }

        fn put(&self, b: &mut Bus, n: u32) {
            let a0 = (self.y as u16 & 0xFF) | (self.bpp8 as u16) << 13 | self.shape << 14 | self.a0;
            let a1 = (self.x as u16 & 0x1FF) | self.flips << 12 | self.size << 14;
            let a2 = self.tile | self.prio << 10 | self.bank << 12;
            b.write16(OAM + n * 8, a0);
            b.write16(OAM + n * 8 + 2, a1);
            b.write16(OAM + n * 8 + 4, a2);
        }
    }

    /// Affine parameter group `g`: `PA`, `PB`, `PC`, `PD`.
    fn params(b: &mut Bus, g: u32, m: [i16; 4]) {
        for (i, v) in m.into_iter().enumerate() {
            b.write16(OAM + g * 32 + 6 + i as u32 * 8, v as u16);
        }
    }

    /// Make sprite `n` (already put) affine with parameter group `g`,
    /// double-size if `double`.
    fn make_affine(b: &mut Bus, n: u32, g: u16, double: bool) {
        let a0 = b.read16(OAM + n * 8);
        b.write16(OAM + n * 8, a0 | 1 << 8 | (double as u16) << 9);
        let a1 = b.read16(OAM + n * 8 + 2);
        b.write16(OAM + n * 8 + 2, a1 & !(0x1F << 9) | g << 9);
    }

    /// Fill 4bpp tile unit `t` with colour `c`.
    fn fill4(b: &mut Bus, t: u32, c: u8) {
        tile4(b, OBJ_VRAM + t * 32, c);
    }

    /// Fill the 8bpp tile starting at unit `t` with colour `c`.
    fn fill8(b: &mut Bus, t: u32, c: u8) {
        tile8(b, OBJ_VRAM + t * 32, c);
    }

    fn setup() -> Box<Bus> {
        let mut b = bus();
        hide_objs(&mut b);
        b.write16(PAL, 0x0042);
        b
    }

    #[test]
    fn sprite_palette_banks_and_transparency() {
        let mut b = setup();
        // Same index in the BG palette must not be used.
        b.write16(PAL + 2 * 3, 0x7FFF);
        b.write16(OBJ_PAL + 2 * 3, 0x001F);
        b.write16(OBJ_PAL + 2 * (5 * 16 + 3), 0x03E0);
        fill4(&mut b, 1, 3);
        // Left half of each row opaque, right half colour 0.
        for r in 0..8 {
            b.write16(OBJ_VRAM + 2 * 32 + r * 4, 0x3333);
        }
        Obj::new(10, 20, 1).put(&mut b, 0);
        Obj {
            bank: 5,
            ..Obj::new(30, 20, 2)
        }
        .put(&mut b, 1);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 10, 20), 0x001F);
        assert_eq!(px(&p, 17, 27), 0x001F);
        assert_eq!(px(&p, 18, 20), 0x0042);
        assert_eq!(px(&p, 10, 28), 0x0042);
        assert_eq!(px(&p, 33, 20), 0x03E0);
        assert_eq!(px(&p, 34, 20), 0x0042);
        // Sprites off in DISPCNT.
        b.write16(IO, dispcnt::OBJ_1D);
        assert_eq!(px(&render(&b), 10, 20), 0x0042);
    }

    #[test]
    fn all_twelve_sizes() {
        for shape in 0..3 {
            for size in 0..4 {
                let (w, h) = SIZES[shape as usize][size as usize];
                let mut b = setup();
                b.write16(OBJ_PAL + 2, 0x001F);
                for t in 0..64 {
                    fill4(&mut b, t, 1);
                }
                Obj::new(100, 50, 0).sized(shape, size).put(&mut b, 0);
                b.write16(IO, OBJ_1D);
                let p = render(&b);
                let what = (shape, size, w, h);
                assert_eq!(px(&p, 100, 50), 0x001F, "{what:?}");
                assert_eq!(px(&p, 99 + w, 49 + h), 0x001F, "{what:?}");
                assert_eq!(px(&p, 100 + w, 50), 0x0042, "{what:?}");
                assert_eq!(px(&p, 100, 50 + h), 0x0042, "{what:?}");
                assert_eq!(px(&p, 99, 50), 0x0042, "{what:?}");
                assert_eq!(px(&p, 100, 49), 0x0042, "{what:?}");
            }
        }
    }

    #[test]
    fn prohibited_shape_draws_nothing() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 0, 1);
        Obj::new(0, 0, 0).sized(3, 0).put(&mut b, 0);
        b.write16(IO, OBJ_1D);
        assert_eq!(px(&render(&b), 0, 0), 0x0042);
    }

    #[test]
    fn mapping_1d_and_2d_4bpp() {
        let mut b = setup();
        for (t, c) in [(0, 1), (1, 2), (2, 3), (3, 4), (32, 5), (33, 6)] {
            b.write16(OBJ_PAL + 2 * c, c as u16);
            fill4(&mut b, t, c as u8);
        }
        // 16x16: tiles (0, 1 / 2, 3) in 1D, (0, 1 / 32, 33) in 2D.
        Obj::new(0, 0, 0).sized(0, 1).put(&mut b, 0);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(
            [px(&p, 0, 0), px(&p, 8, 0), px(&p, 0, 8), px(&p, 8, 8)],
            [1, 2, 3, 4]
        );
        b.write16(IO, dispcnt::OBJ);
        let p = render(&b);
        assert_eq!(
            [px(&p, 0, 0), px(&p, 8, 0), px(&p, 0, 8), px(&p, 8, 8)],
            [1, 2, 5, 6]
        );
    }

    #[test]
    fn mapping_1d_and_2d_8bpp() {
        let mut b = setup();
        for (t, c) in [(0, 1), (2, 2), (4, 3), (6, 4), (32, 5), (34, 6)] {
            b.write16(OBJ_PAL + 2 * c, c as u16);
            fill8(&mut b, t, c as u8);
        }
        // 8bpp takes two units a tile, and ignores the bank.
        Obj {
            bpp8: true,
            bank: 7,
            ..Obj::new(0, 0, 0).sized(0, 1)
        }
        .put(&mut b, 0);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(
            [px(&p, 0, 0), px(&p, 8, 0), px(&p, 0, 8), px(&p, 8, 8)],
            [1, 2, 3, 4]
        );
        b.write16(IO, dispcnt::OBJ);
        let p = render(&b);
        assert_eq!(
            [px(&p, 0, 0), px(&p, 8, 0), px(&p, 0, 8), px(&p, 8, 8)],
            [1, 2, 5, 6]
        );
    }

    #[test]
    fn pixels_within_a_tile_4bpp_and_8bpp() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2 * 9, 0x001F);
        b.write16(OBJ_PAL + 2 * 200, 0x03E0);
        // 4bpp tile 1, row 2: the high nibble is the odd column.
        b.write16(OBJ_VRAM + 32 + 2 * 4, 0x9000);
        // 8bpp tile at unit 4, row 5, column 6 (the low byte).
        b.write16(OBJ_VRAM + 4 * 32 + 5 * 8 + 6, 200);
        Obj::new(0, 0, 1).put(&mut b, 0);
        Obj {
            bpp8: true,
            ..Obj::new(16, 0, 4)
        }
        .put(&mut b, 1);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 3, 2), 0x001F);
        assert_eq!(px(&p, 2, 2), 0x0042);
        assert_eq!(px(&p, 16 + 6, 5), 0x03E0);
        assert_eq!(px(&p, 16 + 7, 5), 0x0042);
    }

    #[test]
    fn flips() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        // A 16x16 sprite whose only pixel is tile 0's top left. (Byte
        // writes to sprite VRAM are ignored.)
        b.write16(OBJ_VRAM, 0x01);
        for (n, flips) in [0u16, 1, 2, 3].iter().enumerate() {
            Obj {
                flips: *flips,
                ..Obj::new(20 * n as i32, 0, 0).sized(0, 1)
            }
            .put(&mut b, n as u32);
        }
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 0x001F);
        assert_eq!(px(&p, 20 + 15, 0), 0x001F);
        assert_eq!(px(&p, 20, 0), 0x0042);
        assert_eq!(px(&p, 40, 15), 0x001F);
        assert_eq!(px(&p, 40, 0), 0x0042);
        assert_eq!(px(&p, 60 + 15, 15), 0x001F);
    }

    #[test]
    fn priority_against_backgrounds() {
        let mut b = setup();
        // BG0 at priority 1: tile 1 everywhere in screen block 8.
        b.write16(PAL + 2, 0x0001);
        tile4(&mut b, VRAM + 32, 1);
        for i in 0..32 * 32 {
            b.write16(VRAM + 8 * 0x800 + i * 2, 1);
        }
        b.write16(BG0CNT, 1 | 8 << 8);
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 0, 1);
        for (n, prio) in [0u16, 1, 2].iter().enumerate() {
            Obj {
                prio: *prio,
                ..Obj::new(10 * n as i32, 0, 0)
            }
            .put(&mut b, n as u32);
        }
        b.write16(IO, OBJ_1D | dispcnt::BG0);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 0x001F);
        // Equal priority: the sprite is in front.
        assert_eq!(px(&p, 10, 0), 0x001F);
        assert_eq!(px(&p, 20, 0), 0x0001);
        // With nothing opaque behind it, a low-priority sprite still shows.
        b.write16(IO, OBJ_1D);
        assert_eq!(px(&render(&b), 20, 0), 0x001F);
    }

    #[test]
    fn priority_between_sprites() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 1);
        b.write16(OBJ_PAL + 4, 2);
        fill4(&mut b, 0, 1);
        fill4(&mut b, 1, 2);
        // Equal priority: the lower OAM number is in front.
        Obj::new(0, 0, 0).put(&mut b, 0);
        Obj::new(4, 0, 1).put(&mut b, 1);
        // A later sprite with a lower priority value is in front.
        Obj {
            prio: 2,
            ..Obj::new(20, 0, 0)
        }
        .put(&mut b, 2);
        Obj {
            prio: 1,
            ..Obj::new(24, 0, 1)
        }
        .put(&mut b, 3);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!([px(&p, 3, 0), px(&p, 4, 0), px(&p, 8, 0)], [1, 1, 2]);
        assert_eq!([px(&p, 23, 0), px(&p, 24, 0), px(&p, 28, 0)], [1, 2, 2]);
    }

    #[test]
    fn edges_wrap() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        for t in 0..4 {
            fill4(&mut b, t, 1);
        }
        // x = -4 (511), y = -6 (250): 16x16 shows 12x10 at the top left.
        Obj::new(-4, -6, 0).sized(0, 1).put(&mut b, 0);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 0x001F);
        assert_eq!(px(&p, 11, 9), 0x001F);
        assert_eq!(px(&p, 12, 0), 0x0042);
        assert_eq!(px(&p, 0, 10), 0x0042);
        // y 150 with height 16 runs off the bottom, not onto the top.
        Obj::new(0, 150, 0).sized(0, 1).put(&mut b, 0);
        let p = render(&b);
        assert_eq!(px(&p, 0, 159), 0x001F);
        assert_eq!(px(&p, 0, 0), 0x0042);
    }

    #[test]
    fn hidden_and_window_sprites_draw_nothing() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 0, 1);
        Obj {
            a0: 1 << 9,
            ..Obj::new(0, 0, 0)
        }
        .put(&mut b, 0);
        Obj {
            a0: 2 << 10,
            ..Obj::new(20, 0, 0)
        }
        .put(&mut b, 2);
        // Semi-transparent draws opaque until blending exists.
        Obj {
            a0: 1 << 10,
            ..Obj::new(30, 0, 0)
        }
        .put(&mut b, 3);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!([px(&p, 0, 0), px(&p, 20, 0)], [0x0042; 2]);
        assert_eq!(px(&p, 30, 0), 0x001F);
    }

    #[test]
    fn cycle_budget_per_line() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        for t in 0..64 {
            fill4(&mut b, t, 1);
        }
        // 64-wide sprites cost 64 each: 18 fit in 1210, 14 in 954. Off
        // screen to the left, they still cost.
        for n in 0..18 {
            Obj::new(-64, 0, 0).sized(0, 3).put(&mut b, n);
        }
        Obj::new(0, 0, 0).sized(0, 0).put(&mut b, 18);
        b.write16(IO, OBJ_1D);
        // 18 * 64 = 1152; the 8-wide one fits (1160).
        assert_eq!(px(&render(&b), 0, 0), 0x001F);
        b.write16(IO, OBJ_1D | dispcnt::HBLANK_FREE);
        assert_eq!(px(&render(&b), 0, 0), 0x0042);
        // Sprites not on the line cost nothing.
        for n in 0..18 {
            Obj::new(-64, 100, 0).sized(0, 3).put(&mut b, n);
        }
        assert_eq!(px(&render(&b), 0, 0), 0x001F);
        // A 64-wide sprite after 18 doesn't fit (1216), until the 64x32
        // ones ahead of it end.
        for n in 0..18 {
            Obj::new(-64, 0, 0).sized(1, 3).put(&mut b, n);
        }
        Obj::new(0, 0, 0).sized(0, 3).put(&mut b, 18);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 0, 31), 0x0042);
        assert_eq!(px(&p, 0, 32), 0x001F);
    }

    #[test]
    fn bitmap_modes_lose_the_first_512_tiles() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 511, 1);
        fill4(&mut b, 512, 1);
        Obj::new(0, 0, 511).put(&mut b, 0);
        Obj::new(10, 0, 512).put(&mut b, 1);
        b.write16(IO, 3 | OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 0x0042);
        assert_eq!(px(&p, 10, 0), 0x001F);
        b.write16(IO, OBJ_1D);
        assert_eq!(px(&render(&b), 0, 0), 0x001F);
    }

    #[test]
    fn sprite_between_two_backgrounds() {
        let mut b = setup();
        // BG0 at priority 0 covers x 0..7 only; BG1 at priority 2 covers
        // the whole line.
        b.write16(PAL + 2, 0x0001);
        b.write16(PAL + 4, 0x0002);
        for i in 0..16 {
            b.write16(VRAM + 32 + i * 2, 0x1111);
            b.write16(VRAM + 64 + i * 2, 0x2222);
        }
        b.write16(VRAM + 8 * 0x800, 1);
        for i in 0..32 {
            b.write16(VRAM + 9 * 0x800 + i * 2, 2);
        }
        b.write16(BG0CNT, 8 << 8);
        b.write16(BG0CNT + 2, 2 | 9 << 8);
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 0, 1);
        fill4(&mut b, 1, 1);
        Obj {
            prio: 1,
            ..Obj::new(4, 0, 0).sized(1, 0)
        }
        .put(&mut b, 0);
        b.write16(IO, OBJ_1D | dispcnt::BG0 | dispcnt::BG0 << 1);
        let p = render(&b);
        assert_eq!([px(&p, 4, 0), px(&p, 8, 0), px(&p, 20, 0)], [1, 0x001F, 2]);
    }

    #[test]
    fn mapping_2d_wide_and_odd_8bpp_tiles() {
        let mut b = setup();
        for (t, c) in [(0, 1), (2, 2), (6, 3), (32, 4), (35, 5), (38, 6)] {
            b.write16(OBJ_PAL + 2 * c, c as u16);
            fill4(&mut b, t, c as u8);
        }
        // 4bpp 64x32 in 2D: tile column 6 of row 1 is unit 32 + 6.
        Obj::new(0, 0, 0).sized(1, 3).put(&mut b, 0);
        b.write16(IO, dispcnt::OBJ);
        let p = render(&b);
        assert_eq!(
            [px(&p, 0, 0), px(&p, 48, 0), px(&p, 0, 8), px(&p, 48, 8)],
            [1, 3, 4, 6]
        );
        // 8bpp in 2D ignores the tile number's bit 0: tile 1 draws tile
        // 0, whose bytes are all 0x11.
        hide_objs(&mut b);
        b.write16(OBJ_PAL + 2 * 0x11, 0x001F);
        Obj {
            bpp8: true,
            ..Obj::new(0, 0, 1)
        }
        .put(&mut b, 0);
        assert_eq!(px(&render(&b), 0, 0), 0x001F);
        // In 1D it doesn't: tile 1 starts at unit 1, which is empty.
        b.write16(IO, OBJ_1D);
        assert_eq!(px(&render(&b), 0, 0), 0x0042);
    }

    #[test]
    fn x_past_the_right_edge() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 0, 1);
        fill4(&mut b, 1, 1);
        // x 236 shows 4 columns; x 250 (bit 8 clear) none, and doesn't wrap.
        Obj::new(236, 0, 0).put(&mut b, 0);
        Obj::new(250, 10, 0).sized(1, 0).put(&mut b, 1);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 239, 0), 0x001F);
        assert!((0..WIDTH).all(|x| px(&p, x, 10) == 0x0042));
    }

    #[test]
    fn bitmap_cutoff_in_modes_4_and_5() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 511, 1);
        fill4(&mut b, 512, 1);
        Obj::new(0, 0, 511).put(&mut b, 0);
        Obj::new(10, 0, 512).put(&mut b, 1);
        for mode in [4, 5] {
            b.write16(IO, mode | OBJ_1D);
            let p = render(&b);
            assert_eq!(
                [px(&p, 0, 0), px(&p, 10, 0)],
                [0x0042, 0x001F],
                "mode {mode}"
            );
        }
    }

    #[test]
    fn sprite_against_the_bitmap_background() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 512, 1);
        b.write16(VRAM, 0x0300);
        b.write16(VRAM + 2, 0x0300);
        b.write16(BG0CNT + 4, 1);
        Obj {
            prio: 2,
            ..Obj::new(0, 0, 512)
        }
        .put(&mut b, 0);
        Obj {
            prio: 1,
            ..Obj::new(1, 0, 512)
        }
        .put(&mut b, 1);
        b.write16(IO, 3 | OBJ_1D | dispcnt::BG0 << 2);
        let p = render(&b);
        assert_eq!([px(&p, 0, 0), px(&p, 1, 0)], [0x0300, 0x001F]);
    }

    /// A 16x16 sprite at tiles 0-3 (1D): its top-left 8x8 colour 1, the
    /// rest colour 2.
    fn quadrants(b: &mut Bus) {
        b.write16(OBJ_PAL + 2, 0x001F);
        b.write16(OBJ_PAL + 4, 0x7C00);
        fill4(b, 0, 1);
        for t in 1..4 {
            fill4(b, t, 2);
        }
    }

    #[test]
    fn affine_identity() {
        let mut b = setup();
        quadrants(&mut b);
        params(&mut b, 0, [0x100, 0, 0, 0x100]);
        Obj::new(40, 30, 0).sized(0, 1).put(&mut b, 0);
        make_affine(&mut b, 0, 0, false);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        assert_eq!(px(&p, 40, 30), 0x001F);
        assert_eq!(px(&p, 47, 37), 0x001F);
        assert_eq!(px(&p, 48, 30), 0x7C00);
        assert_eq!(px(&p, 55, 45), 0x7C00);
        assert_eq!(px(&p, 56, 30), 0x0042);
        assert_eq!(px(&p, 40, 46), 0x0042);
    }

    #[test]
    fn affine_flip_by_matrix_and_double_size() {
        let mut b = setup();
        quadrants(&mut b);
        // -1 horizontally: the colour-1 corner goes top right.
        params(&mut b, 3, [-0x100, 0, 0, 0x100]);
        Obj::new(40, 30, 0).sized(0, 1).put(&mut b, 0);
        make_affine(&mut b, 0, 3, false);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        // Box column 0 is offset -8, sprite column 16: outside it.
        assert_eq!(px(&p, 40, 30), 0x0042);
        assert_eq!(px(&p, 41, 30), 0x7C00);
        // Box column 8 is offset 0, sprite column 8: the right half.
        assert_eq!(px(&p, 48, 30), 0x7C00);
        assert_eq!(px(&p, 49, 30), 0x001F);
        assert_eq!(px(&p, 55, 37), 0x001F);
        // Double size at scale 1: a 32x32 box with the sprite in its
        // middle 16x16.
        params(&mut b, 3, [0x100, 0, 0, 0x100]);
        make_affine(&mut b, 0, 3, true);
        let p = render(&b);
        assert_eq!(px(&p, 47, 37), 0x0042);
        assert_eq!(px(&p, 48, 38), 0x001F);
        assert_eq!(px(&p, 63, 53), 0x7C00);
        assert_eq!(px(&p, 64, 53), 0x0042);
        // Half a texel per pixel draws the sprite twice the size, filling
        // the box.
        params(&mut b, 3, [0x80, 0, 0, 0x80]);
        let p = render(&b);
        assert_eq!(px(&p, 40, 30), 0x001F);
        assert_eq!(px(&p, 55, 45), 0x001F);
        assert_eq!(px(&p, 56, 30), 0x7C00);
        assert_eq!(px(&p, 71, 61), 0x7C00);
        assert_eq!(px(&p, 72, 61), 0x0042);
    }

    #[test]
    fn affine_rotation_by_ninety_degrees() {
        let mut b = setup();
        quadrants(&mut b);
        // Texel column follows box row, texel row follows -box column: the
        // colour-1 corner (top left) turns to the top right.
        params(&mut b, 0, [0, 0x100, -0x100, 0]);
        Obj::new(40, 30, 0).sized(0, 1).put(&mut b, 0);
        make_affine(&mut b, 0, 0, false);
        b.write16(IO, OBJ_1D);
        let p = render(&b);
        // Box (col, row) = (15, 0): dx 7, dy -8 -> texel (0, 1).
        assert_eq!(px(&p, 55, 30), 0x001F);
        assert_eq!(px(&p, 49, 37), 0x001F);
        assert_eq!(px(&p, 48, 37), 0x7C00);
        assert_eq!(px(&p, 41, 30), 0x7C00);
        // Box column 0 lands on texel row 16, outside the sprite.
        assert_eq!(px(&p, 40, 30), 0x0042);
        assert_eq!(px(&p, 55, 38), 0x7C00);
    }

    #[test]
    fn affine_sprites_cost_more() {
        let mut b = setup();
        b.write16(OBJ_PAL + 2, 0x001F);
        fill4(&mut b, 0, 1);
        params(&mut b, 0, [0x100, 0, 0, 0x100]);
        // A double-size 64x64 costs 10 + 2 * 128 = 266: four fit in 1210,
        // and a fifth doesn't.
        for n in 0..5 {
            Obj::new(-128, 0, 0).sized(0, 3).put(&mut b, n);
            make_affine(&mut b, n, 0, true);
        }
        Obj::new(0, 0, 0).put(&mut b, 5);
        b.write16(IO, dispcnt::OBJ);
        assert_eq!(px(&render(&b), 0, 0), 0x0042);
        Obj::new(0, 100, 0).put(&mut b, 4);
        assert_eq!(px(&render(&b), 0, 0), 0x001F);
    }
}
