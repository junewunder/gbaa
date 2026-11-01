//! The bitmap modes, drawn as BG2:
//!
//! | mode | size    | pixels                         | pages          |
//! |------|---------|--------------------------------|----------------|
//! | 3    | 240×160 | 15-bit colour                  | one            |
//! | 4    | 240×160 | 8-bit index into the BG palette, 0 transparent | `0x0000`, `0xA000` |
//! | 5    | 160×128 | 15-bit colour; outside it, transparent | `0x0000`, `0xA000` |
//!
//! `DISPCNT` bit 4 picks the page in modes 4 and 5.
//!
//! BG2 samples the bitmap through its affine matrix and reference point,
//! as an affine background samples its map ([`super::bg_affine`]): pixel
//! (x, y) of the bitmap shows wherever the transformed point lands on it.
//! Outside the bitmap is transparent; `BG2CNT`'s wrap bit doesn't apply.

use super::{Line, dispcnt};
use crate::platform::{HEIGHT, WIDTH};
use crate::video::Video;

/// Where the second page starts.
pub const PAGE_1: usize = 0xA000;
/// Mode 5's bitmap size.
pub const MODE5_WIDTH: usize = 160;
pub const MODE5_HEIGHT: usize = 128;

/// Draw a line of the bitmap mode `DISPCNT` selects into `out`, starting
/// at BG2's reference point `start` for the line.
pub fn line(v: &Video, start: (i32, i32), out: &mut Line) {
    let cnt = v.dispcnt.0;
    let page = if cnt & dispcnt::PAGE != 0 { PAGE_1 } else { 0 };
    let mode = cnt & dispcnt::MODE;
    let (w, h) = match mode {
        3 | 4 => (WIDTH as i32, HEIGHT as i32),
        5 => (MODE5_WIDTH as i32, MODE5_HEIGHT as i32),
        _ => return,
    };
    let pa = v.affine[0].pa as i32;
    let pc = v.affine[0].pc as i32;
    let vram = &v.vram;

    let (mut sx, mut sy) = start;
    for px in out.iter_mut() {
        let (x, y) = (sx >> 8, sy >> 8);
        sx = sx.wrapping_add(pa);
        sy = sy.wrapping_add(pc);
        if !(0..w).contains(&x) || !(0..h).contains(&y) {
            continue;
        }
        let i = (y * w + x) as usize;
        if mode == 4 {
            let index = vram[page + i] as usize;
            if index != 0 {
                let o = index * 2;
                *px = u16::from_le_bytes([v.palette[o], v.palette[o + 1]]) & 0x7FFF;
            }
        } else {
            let o = page + i * 2;
            *px = u16::from_le_bytes([vram[o], vram[o + 1]]) & 0x7FFF;
        }
    }
}
