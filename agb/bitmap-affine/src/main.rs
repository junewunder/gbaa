//! gbaa test ROM for the bitmap modes' affine transform: mode 5's second
//! page drawn through BG2's matrix and reference point, rotated by 30
//! degrees and scaled up, with the wrap bit set (which bitmaps ignore),
//! set up by writing the display registers directly.
//!
//! `SCENE.md` lists the pixels it must draw. VRAM and palette take
//! halfword and word stores only, so everything here goes through
//! `store16`; never write those regions through a `u8` pointer, `memcpy`
//! or `memset` (which may store bytes).
#![no_std]
#![no_main]

use core::ptr::write_volatile;

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const BG2CNT: usize = IO + 0xC;
const BG2PA: usize = IO + 0x20;
const PAL: usize = 0x0500_0000;
const VRAM: usize = 0x0600_0000;
/// Mode 5's second page.
const PAGE_1: usize = VRAM + 0xA000;

const MODE5: u16 = 5;
const PAGE: u16 = 1 << 4;
const BG2_ON: u16 = 1 << 10;

/// One halfword at `addr`.
fn store16(addr: usize, v: u16) {
    // SAFETY: callers pass addresses in IO, palette or VRAM, which take
    // halfword stores.
    unsafe { write_volatile(addr as *mut u16, v) }
}

/// BG2's `PA`-`PD`, then its 28-bit reference point.
fn bg2_affine(m: [i16; 4], x: i32, y: i32) {
    for (i, v) in m.into_iter().enumerate() {
        store16(BG2PA + i * 2, v as u16);
    }
    for (o, v) in [(8, x), (12, y)] {
        store16(BG2PA + o, v as u16);
        store16(BG2PA + o + 2, (v >> 16) as u16 & 0x0FFF);
    }
}

/// Bitmap pixel (x, y)'s colour on page 1: red is x / 8, green y / 8, and
/// blue full on a checker of single pixels, so every 8×8 block differs.
fn colour(x: usize, y: usize) -> u16 {
    let blue = if (x ^ y) & 1 == 0 { 0x7C00 } else { 0 };
    (x / 8) as u16 | ((y / 8) as u16) << 5 | blue
}

/// The scene `SCENE.md` describes.
fn scene() {
    // Forced blank while setting up.
    store16(DISPCNT, 1 << 7);

    // Grey backdrop.
    store16(PAL, 0x4210);

    // Mode 5's 160×128 pages: page 0 all white (it must not show), page 1
    // the pattern `colour` makes.
    for y in 0..128 {
        for x in 0..160 {
            let at = (y * 160 + x) * 2;
            store16(VRAM + at, 0x7FFF);
            store16(PAGE_1 + at, colour(x, y));
        }
    }

    // Wrap bit set: bitmaps ignore it.
    store16(BG2CNT, 1 << 13);
    // 30 degrees, 1.25 screen pixels per bitmap pixel each way (PA = PD =
    // 0.8 cos, PB = 0.8 sin, PC = -0.8 sin), bitmap (80, 64) at screen
    // (120, 80).
    bg2_affine([177, 102, -102, 177], -8920, 14464);

    store16(DISPCNT, MODE5 | PAGE | BG2_ON);
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    scene();
    loop {
        vblank.wait_for_vblank();
    }
}
