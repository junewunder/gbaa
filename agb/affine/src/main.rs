//! gbaa test ROM for affine backgrounds and sprites: mode 2's BG2
//! (rotated, no wrap) and BG3 (scaled, wrapping), and affine sprites,
//! regular and double-size, rotated, scaled and sheared, set up by writing
//! the display registers directly.
//!
//! `SCENE.md` lists the pixels it must draw. VRAM, palette and OAM take
//! halfword and word stores only, so everything here goes through
//! `store16` and `fill16`; never write those regions through a `u8`
//! pointer, `memcpy` or `memset` (which may store bytes).
#![no_std]
#![no_main]

use core::ptr::write_volatile;

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const BG2CNT: usize = IO + 0xC;
const BG3CNT: usize = IO + 0xE;
const BG2PA: usize = IO + 0x20;
const BG3PA: usize = IO + 0x30;
const PAL: usize = 0x0500_0000;
const OBJ_PAL: usize = PAL + 0x200;
const VRAM: usize = 0x0600_0000;
const OBJ_VRAM: usize = VRAM + 0x1_0000;
const OAM: usize = 0x0700_0000;

const MODE2: u16 = 2;
const BG2_ON: u16 = 1 << 10;
const BG3_ON: u16 = 1 << 11;
const OBJ_ON: u16 = 1 << 12;
const OBJ_1D: u16 = 1 << 6;

/// Attribute 0: affine, double size, 8bpp.
const AFFINE: u16 = 1 << 8;
const DOUBLE: u16 = 1 << 9;
const BPP8: u16 = 1 << 13;
/// Attribute 1: size 1 (16x16 square) and 2 (32x32).
const SIZE16: u16 = 1 << 14;
const SIZE32: u16 = 2 << 14;

/// One halfword at `addr`.
fn store16(addr: usize, v: u16) {
    // SAFETY: callers pass addresses in IO, palette, VRAM or OAM, which
    // take halfword stores.
    unsafe { write_volatile(addr as *mut u16, v) }
}

/// `n` halfwords of `v` from `addr` up.
fn fill16(addr: usize, n: usize, v: u16) {
    for i in 0..n {
        store16(addr + i * 2, v);
    }
}

/// 8bpp BG tile `t` at character base 0, pixel (r, c) colour `f(r, c)`.
fn bg_tile(t: usize, f: impl Fn(usize, usize) -> u16) {
    for r in 0..8 {
        for c in (0..8).step_by(2) {
            store16(VRAM + t * 64 + r * 8 + c, f(r, c) | f(r, c + 1) << 8);
        }
    }
}

/// A one-byte-per-tile affine map at `base`, `n` tiles square, tile
/// (tx, ty) `f(tx, ty)`.
fn affine_map(base: usize, n: usize, f: impl Fn(usize, usize) -> u16) {
    for ty in 0..n {
        for tx in (0..n).step_by(2) {
            store16(base + ty * n + tx, f(tx, ty) | f(tx + 1, ty) << 8);
        }
    }
}

/// BG `PA`-`PD` at `at`, then the 28-bit reference point.
fn bg_affine(at: usize, m: [i16; 4], x: i32, y: i32) {
    for (i, v) in m.into_iter().enumerate() {
        store16(at + i * 2, v as u16);
    }
    for (o, v) in [(8, x), (12, y)] {
        store16(at + o, v as u16);
        store16(at + o + 2, (v >> 16) as u16 & 0x0FFF);
    }
}

/// Sprite parameter group `g`: `PA`-`PD`.
fn obj_affine(g: usize, m: [i16; 4]) {
    for (i, v) in m.into_iter().enumerate() {
        store16(OAM + g * 32 + 6 + i * 8, v as u16);
    }
}

/// Sprite `n`: attributes 0, 1 and 2.
fn obj(n: usize, a0: u16, a1: u16, a2: u16) {
    store16(OAM + n * 8, a0);
    store16(OAM + n * 8 + 2, a1);
    store16(OAM + n * 8 + 4, a2);
}

/// The scene `SCENE.md` describes.
fn scene() {
    // Forced blank while setting up.
    store16(DISPCNT, 1 << 7);

    // BG palette: grey backdrop, 1 red, 2 blue, 3 yellow, 4 white, 5 dark
    // grey. OBJ palette: 1 green, 2 magenta, 3 cyan.
    for (i, c) in [0x4210, 0x001F, 0x7C00, 0x03FF, 0x7FFF, 0x0C63]
        .into_iter()
        .enumerate()
    {
        store16(PAL + i * 2, c);
    }
    for (i, c) in [0x03E0, 0x7C1F, 0x7FE0].into_iter().enumerate() {
        store16(OBJ_PAL + 2 + i * 2, c);
    }

    // BG tiles (8bpp, character base 0): 0 transparent, 1 solid red, 2
    // solid blue, 3 a dark top row over yellow left and white right halves.
    bg_tile(0, |_, _| 0);
    bg_tile(1, |_, _| 1);
    bg_tile(2, |_, _| 2);
    bg_tile(3, |r, c| match (r, c) {
        (0, _) => 5,
        (_, 0..4) => 3,
        _ => 4,
    });

    // BG2 (priority 1): 128x128, no wrap, screen base 16. A checker of
    // tiles 1 and 3, every fourth tile column empty. Rotated by 30 degrees
    // (PA = PD = cos, PB = sin, PC = -sin), map (64, 64) at screen
    // (120, 80).
    affine_map(VRAM + 16 * 0x800, 16, |tx, ty| match (tx % 4, (tx + ty) % 2) {
        (3, _) => 0,
        (_, 0) => 1,
        _ => 3,
    });
    store16(BG2CNT, 1 | 16 << 8);
    bg_affine(BG2PA, [222, 128, -128, 222], -80 << 8, 14003);

    // BG3 (priority 2): 256x256, wrapping, screen base 20. Blue 16-pixel
    // squares on a checker, scaled down to two thirds (1.5 map pixels per
    // screen pixel), starting at map (-37.5, 12.25).
    affine_map(VRAM + 20 * 0x800, 32, |tx, ty| {
        if (tx / 2 + ty / 2) % 2 == 0 { 2 } else { 0 }
    });
    store16(BG3CNT, 2 | 20 << 8 | 1 << 13 | 1 << 14);
    bg_affine(BG3PA, [0x180, 0, 0, 0x180], -9600, 3136);

    // Sprite tiles. 4bpp 32x32 at tiles 0-15 (1D): tile 0 magenta, the
    // rest of the top tile row and left tile column green, the others
    // cyan. 8bpp 16x16 at units 32-39: rows striped green and cyan, two
    // rows each.
    for t in 0..16 {
        let c = if t == 0 {
            2
        } else if t < 4 || t % 4 == 0 {
            1
        } else {
            3
        };
        fill16(OBJ_VRAM + t * 32, 16, c * 0x1111);
    }
    for t in 0..4 {
        for r in 0..8 {
            let c = if r % 4 < 2 { 1 } else { 3 };
            fill16(OBJ_VRAM + (32 + t * 2) * 32 + r * 8, 4, c * 0x0101);
        }
    }

    // Sprites: all 128 hidden, then
    // 0: the 32x32, double size, group 0 (45 degrees), at (8, 8), prio 0;
    // 1: the 32x32, group 1 (twice the size, clipped to its box), at
    //    (100, 100), priority 2: behind BG2, in front of BG3;
    // 2: the 8bpp 16x16, double size, group 2 (sheared), at (180, 20);
    // 3: the 32x32, double size, group 3 (90 degrees), at (200, 230): its
    //    box wraps to the top of the screen and runs off the right.
    for n in 0..128 {
        obj(n, 1 << 9, 0, 0);
    }
    obj_affine(0, [181, 181, -181, 181]);
    obj_affine(1, [0x80, 0, 0, 0x80]);
    obj_affine(2, [0x100, 0x80, 0, 0x100]);
    obj_affine(3, [0, 0x100, -0x100, 0]);
    obj(0, 8 | AFFINE | DOUBLE, 8 | SIZE32, 0);
    obj(1, 100 | AFFINE, 100 | 1 << 9 | SIZE32, 2 << 10);
    obj(2, 20 | AFFINE | DOUBLE | BPP8, 180 | 2 << 9 | SIZE16, 32);
    obj(3, 230 | AFFINE | DOUBLE, 200 | 3 << 9 | SIZE32, 0);

    store16(DISPCNT, MODE2 | BG2_ON | BG3_ON | OBJ_ON | OBJ_1D);
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    scene();
    loop {
        vblank.wait_for_vblank();
    }
}
