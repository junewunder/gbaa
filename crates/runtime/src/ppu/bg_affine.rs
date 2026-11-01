//! Affine backgrounds: BG2 of mode 1 and BG2-BG3 of mode 2, rotated and
//! scaled (GBATEK "LCD I/O BG Rotation/Scaling").
//!
//! `BGxCNT` is read as for text backgrounds ([`super::bg_text`]), except
//! that tiles are always 8bpp, bit 13 makes the map wrap rather than show
//! nothing outside it, and the size (bits 14-15) is a square of 128, 256,
//! 512 or 1024 pixels. The map is one byte per tile, a tile number with no
//! flips or palette bank, `size / 8` tiles to a row.
//!
//! Each pixel samples the map at a point that moves by (`PA`, `PC`) per
//! pixel across a line, from the line's reference point, which moves by
//! (`PB`, `PD`) per line. `PA`-`PD` are signed 8.8 fixed point; the
//! reference point (`BGxX`, `BGxY`) is signed 20.8, 28 bits wide.
//!
//! The hardware copies `BGxX`/`BGxY` into internal registers at the start
//! of each frame and whenever the CPU writes them, and adds `PB`/`PD`
//! after each line. [`Refs`] keeps those; it sees a write as a change in
//! the register's value, so writing the value already there mid-frame
//! (which restarts the hardware's sum) goes unnoticed.

use super::Line;
use crate::video::{Affine, Video};

/// Where background tiles and maps may live: the first 64 KB of VRAM.
const BG_VRAM: usize = 0x1_0000;

/// BG `n`'s (2 or 3) parameters: `PA`, `PB`, `PC`, `PD`, `X`, `Y`.
fn params(v: &Video, n: usize) -> &Affine {
    &v.affine[n - 2]
}

/// The internal reference points of BG2 and BG3.
#[derive(Clone, Copy, Debug, Default)]
pub struct Refs {
    /// The point line `y` starts from, per background: (x, y).
    point: [(i32, i32); 2],
    /// The `BGxX`/`BGxY` values last loaded.
    loaded: [(i32, i32); 2],
}

impl Refs {
    /// The reference points for line `y` (BG2's, BG3's), then step them
    /// on to the next line's. Line 0 reloads them from the registers; a
    /// later line reloads either coordinate whose register has changed.
    pub fn line(&mut self, v: &Video, y: usize) -> [(i32, i32); 2] {
        let mut now = [(0, 0); 2];
        for (i, out) in now.iter_mut().enumerate() {
            let a = params(v, i + 2);
            let regs = (a.x, a.y);
            let (p, l) = (&mut self.point[i], &mut self.loaded[i]);
            if y == 0 || regs.0 != l.0 {
                p.0 = regs.0;
            }
            if y == 0 || regs.1 != l.1 {
                p.1 = regs.1;
            }
            *l = regs;
            *out = *p;
            let (pb, pd) = (a.pb as i32, a.pd as i32);
            p.0 = p.0.wrapping_add(pb);
            p.1 = p.1.wrapping_add(pd);
        }
        now
    }
}

/// BG `n`'s (2 or 3) reference point `point`, moved back `lines` lines:
/// less `lines` times (`PB`, `PD`). Mosaic draws a line from its block's
/// first this way.
pub fn step_back(v: &Video, n: usize, point: (i32, i32), lines: usize) -> (i32, i32) {
    let a = params(v, n);
    let k = lines as i32;
    let (pb, pd) = (a.pb as i32, a.pd as i32);
    (
        point.0.wrapping_sub(pb.wrapping_mul(k)),
        point.1.wrapping_sub(pd.wrapping_mul(k)),
    )
}

/// Draw affine background `n` for a line starting at reference point
/// `start` into `out`.
pub fn line(v: &Video, n: usize, start: (i32, i32), out: &mut Line) {
    let cnt = v.bg[n].cnt.0;
    let a = params(v, n);
    let pa = a.pa as i32;
    let pc = a.pc as i32;
    let char_base = ((cnt >> 2) & 3) as usize * 0x4000;
    let screen_base = ((cnt >> 8) & 0x1F) as usize * 0x800;
    let wrap = cnt & (1 << 13) != 0;
    let size = 128i32 << (cnt >> 14);
    let tiles = (size / 8) as usize;
    let vram = &v.vram;

    let (mut sx, mut sy) = start;
    for px in out.iter_mut() {
        let (mut mx, mut my) = (sx >> 8, sy >> 8);
        sx = sx.wrapping_add(pa);
        sy = sy.wrapping_add(pc);
        if wrap {
            mx &= size - 1;
            my &= size - 1;
        } else if !(0..size).contains(&mx) || !(0..size).contains(&my) {
            continue;
        }
        let (mx, my) = (mx as usize, my as usize);
        let at = (screen_base + my / 8 * tiles + mx / 8) % BG_VRAM;
        let tile = vram[at] as usize;
        let a = char_base + tile * 64 + (my % 8) * 8 + mx % 8;
        if a >= BG_VRAM {
            continue;
        }
        let index = vram[a] as usize;
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

    const BG2CNT: u32 = IO + 0xC;
    const BG2PA: u32 = IO + 0x20;
    const BG3CNT: u32 = IO + 0xE;
    const BG3PA: u32 = IO + 0x30;
    const BG2: u16 = dispcnt::BG0 << 2;
    const BG3: u16 = dispcnt::BG0 << 3;

    /// Write a 28-bit reference point register.
    fn set_ref(b: &mut Bus, o: u32, v: i32) {
        b.write16(o, v as u16);
        b.write16(o + 2, (v >> 16) as u16 & 0x0FFF);
    }

    /// A bus with a grey backdrop, tile 1 solid colour 1 (red), tile 2
    /// solid colour 2 (blue) and tile 0 transparent, at character base 0.
    fn setup() -> std::boxed::Box<Bus> {
        let mut b = bus();
        hide_objs(&mut b);
        b.write16(PAL, 0x0042);
        b.write16(PAL + 2, 0x001F);
        b.write16(PAL + 4, 0x7C00);
        tile8(&mut b, VRAM + 64, 1);
        tile8(&mut b, VRAM + 128, 2);
        b
    }

    /// Put tile `t` at (`tx`, `ty`) of the 16-tile-wide map at screen base 1.
    fn map(b: &mut Bus, tx: u32, ty: u32, t: u8) {
        let at = VRAM + 0x800 + ty * 16 + tx;
        let old = b.read16(at & !1);
        let v = if at & 1 == 0 {
            old & 0xFF00 | t as u16
        } else {
            old & 0x00FF | (t as u16) << 8
        };
        b.write16(at & !1, v);
    }

    #[test]
    fn identity_draws_the_map_and_nothing_outside_it() {
        let mut b = setup();
        map(&mut b, 0, 0, 1);
        map(&mut b, 15, 15, 2);
        b.write16(BG2CNT, 1 << 8);
        b.write16(IO, 2 | BG2);
        let p = render(&b);
        assert_eq!(px(&p, 0, 0), 0x001F);
        assert_eq!(px(&p, 7, 7), 0x001F);
        assert_eq!(px(&p, 8, 0), 0x0042);
        assert_eq!(px(&p, 127, 127), 0x7C00);
        // No wrap: past the 128-pixel map is transparent.
        assert_eq!(px(&p, 128, 0), 0x0042);
        assert_eq!(px(&p, 0, 128), 0x0042);
        // Wrap on: (128, 128) is (0, 0) again.
        b.write16(BG2CNT, 1 << 8 | 1 << 13);
        let p = render(&b);
        assert_eq!(px(&p, 128, 128), 0x001F);
        assert_eq!(px(&p, 128, 0), 0x001F);
        assert_eq!(px(&p, 135, 135), 0x001F);
    }

    #[test]
    fn scale_and_reference_point() {
        let mut b = setup();
        map(&mut b, 1, 1, 1);
        b.write16(BG2CNT, 1 << 8);
        // Half size: each map pixel takes two screen pixels each way.
        b.write16(BG2PA, 0x80);
        b.write16(BG2PA + 6, 0x80);
        b.write16(IO, 2 | BG2);
        let p = render(&b);
        assert_eq!(px(&p, 16, 16), 0x001F);
        assert_eq!(px(&p, 31, 31), 0x001F);
        assert_eq!(px(&p, 32, 16), 0x0042);
        assert_eq!(px(&p, 15, 16), 0x0042);
        // Start 8.5 map pixels right and 4 down (negative coordinates
        // are outside the map).
        set_ref(&mut b, BG2PA + 8, 8 << 8 | 0x80);
        set_ref(&mut b, BG2PA + 12, -(4 << 8));
        let p = render(&b);
        // Screen x 0 is map x 8.5; y 24 is map y 8.
        assert_eq!(px(&p, 0, 24), 0x001F);
        assert_eq!(px(&p, 14, 24), 0x001F);
        assert_eq!(px(&p, 15, 24), 0x0042);
        assert_eq!(px(&p, 0, 23), 0x0042);
    }

    #[test]
    fn rotation_by_ninety_degrees() {
        let mut b = setup();
        map(&mut b, 2, 0, 1);
        b.write16(BG3CNT, 1 << 8);
        // Map x follows screen y and map y follows screen -x: start at
        // map (0, 127.x) so screen x 0 is the map's bottom row.
        b.write16(BG3PA, 0);
        b.write16(BG3PA + 2, 0x100);
        b.write16(BG3PA + 4, 0xFF00);
        b.write16(BG3PA + 6, 0);
        set_ref(&mut b, BG3PA + 8, 0);
        set_ref(&mut b, BG3PA + 12, 127 << 8);
        b.write16(IO, 2 | BG3);
        let p = render(&b);
        // Map (16..24, 0..8) is screen x 120..128, y 16..24.
        assert_eq!(px(&p, 127, 16), 0x001F);
        assert_eq!(px(&p, 120, 23), 0x001F);
        assert_eq!(px(&p, 119, 16), 0x0042);
        assert_eq!(px(&p, 127, 24), 0x0042);
    }

    #[test]
    fn sizes_tiles_and_mode1() {
        let mut b = setup();
        // 256x256: 32 tiles to a row.
        let at = VRAM + 0x800 + 3 * 32 + 4;
        b.write16(at, 2);
        b.write16(BG2CNT, 1 << 8 | 1 << 14);
        // Mode 1 has BG2 affine too; BG3 isn't drawn.
        b.write16(BG3CNT, 1 << 8 | 1 << 13);
        b.write16(IO, 1 | BG2 | BG3);
        let p = render(&b);
        assert_eq!(px(&p, 32, 24), 0x7C00);
        assert_eq!(px(&p, 39, 31), 0x7C00);
        assert_eq!(px(&p, 40, 24), 0x0042);
        // As BG3's 128-pixel map, that byte is tile (4, 6).
        assert_eq!(px(&p, 32, 48), 0x0042);
        b.write16(IO, 2 | BG3);
        assert_eq!(px(&render(&b), 32, 48), 0x7C00);
    }

    #[test]
    fn reference_point_reloads_on_change_mid_frame() {
        let mut b = setup();
        map(&mut b, 0, 0, 1);
        b.write16(BG2CNT, 1 << 8);
        b.write16(IO, 2 | BG2);
        let mut p = std::boxed::Box::new(crate::ppu::Ppu::new());
        for y in 0..10 {
            p.render_line(&video(&b), y);
        }
        // Line 10 would be map row 10; moving the point to row 3 restarts
        // the count from there.
        set_ref(&mut b, BG2PA + 12, 3 << 8);
        for y in 10..20 {
            p.render_line(&video(&b), y);
        }
        // Rows 10..15 show map rows 3..8: rows 3..7 are tile (0, 0).
        assert_eq!(px(&p, 0, 9), 0x0042);
        assert_eq!(px(&p, 0, 10), 0x001F);
        assert_eq!(px(&p, 0, 14), 0x001F);
        assert_eq!(px(&p, 0, 15), 0x0042);
    }
}

crate::snap::snap!(Refs { point, loaded });
