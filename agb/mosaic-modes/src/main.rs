//! gbaa test ROM for mosaic and mode 1: two text backgrounds (one in
//! mosaic, scrolled, with flipped tiles) over a rotated affine BG2 in
//! mosaic, and regular and affine sprites with and without mosaic, set up
//! by writing the display registers directly.
//!
//! `SCENE.md` lists the pixels it must draw. VRAM, palette and OAM take
//! halfword and word stores only, so everything here goes through
//! `store16`; never write those regions through a `u8` pointer, `memcpy`
//! or `memset` (which may store bytes).
#![no_std]
#![no_main]

use core::ptr::write_volatile;

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const BG0CNT: usize = IO + 0x8;
const BG1CNT: usize = IO + 0xA;
const BG2CNT: usize = IO + 0xC;
const BG0HOFS: usize = IO + 0x10;
const BG1HOFS: usize = IO + 0x14;
const BG2PA: usize = IO + 0x20;
const MOSAIC: usize = IO + 0x4C;
const PAL: usize = 0x0500_0000;
const OBJ_PAL: usize = PAL + 0x200;
const VRAM: usize = 0x0600_0000;
const OBJ_VRAM: usize = VRAM + 0x1_0000;
const OAM: usize = 0x0700_0000;

const MODE1: u16 = 1;
const BG0_ON: u16 = 1 << 8;
const BG1_ON: u16 = 1 << 9;
const BG2_ON: u16 = 1 << 10;
const OBJ_ON: u16 = 1 << 12;
const OBJ_1D: u16 = 1 << 6;

/// `BGxCNT`: mosaic, 8bpp, affine wrap.
const BG_MOSAIC: u16 = 1 << 6;
const BG_8BPP: u16 = 1 << 7;
const WRAP: u16 = 1 << 13;
/// Text map entries: flips.
const HFLIP: u16 = 1 << 10;
const VFLIP: u16 = 1 << 11;
/// Attribute 0: affine, double size, mosaic, 8bpp.
const AFFINE: u16 = 1 << 8;
const DOUBLE: u16 = 1 << 9;
const OBJ_MOSAIC: u16 = 1 << 12;
const BPP8: u16 = 1 << 13;
/// Attribute 1: size 1 (16x16 square), flips.
const SIZE16: u16 = 1 << 14;
const OBJ_HFLIP: u16 = 1 << 12;
const OBJ_VFLIP: u16 = 1 << 13;

/// One halfword at `addr`.
fn store16(addr: usize, v: u16) {
    // SAFETY: callers pass addresses in IO, palette, VRAM or OAM, which
    // take halfword stores.
    unsafe { write_volatile(addr as *mut u16, v) }
}

/// BG palette index of texel (c, r) of background `k`'s tile.
fn bg_index(k: usize, c: usize, r: usize) -> u16 {
    (1 + c + 8 * r + 64 * k) as u16
}

/// Sprite palette index of texel (c, r) of the 16x16 sprite; the last
/// texel wraps to 0, transparent.
fn obj_index(c: usize, r: usize) -> u16 {
    ((1 + c + 16 * r) & 0xFF) as u16
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

/// A 32x32 text map at screen base `sb`, entry (tx, ty) `f(tx, ty)`.
fn text_map(sb: usize, f: impl Fn(usize, usize) -> u16) {
    for ty in 0..32 {
        for tx in 0..32 {
            store16(VRAM + sb * 0x800 + (ty * 32 + tx) * 2, f(tx, ty));
        }
    }
}

/// The scene `SCENE.md` describes.
fn scene() {
    // Forced blank while setting up.
    store16(DISPCNT, 1 << 7);

    // BG palette: grey backdrop; background k's texel (c, r) red 4c,
    // green 4r, blue 6 + 6k. OBJ palette: index i red 2 (i % 16), green
    // 2 (i / 16), blue full.
    store16(PAL, 0x4210);
    for k in 0..3 {
        for r in 0..8 {
            for c in 0..8 {
                let colour = (4 * c | (4 * r) << 5 | (6 + 6 * k) << 10) as u16;
                store16(PAL + bg_index(k, c, r) as usize * 2, colour);
            }
        }
    }
    for i in 1..256 {
        store16(OBJ_PAL + i * 2, ((i & 15) << 1 | (i >> 4) << 6 | 0x7C00) as u16);
    }

    // BG tiles (8bpp, character base 0): 0 transparent, 1 + k background
    // k's, every texel its own colour.
    for k in 0..3 {
        for r in 0..8 {
            for c in (0..8).step_by(2) {
                let v = bg_index(k, c, r) | bg_index(k, c + 1, r) << 8;
                store16(VRAM + (1 + k) * 64 + r * 8 + c, v);
            }
        }
    }

    // BG0 (priority 0, mosaic): 256x256, screen base 24, scrolled by
    // (3, 5). Tile 1 where tx + ty is a multiple of 4, flipped
    // horizontally in odd columns.
    text_map(24, |tx, ty| match ((tx + ty) % 4, tx % 2) {
        (0, 0) => 1,
        (0, _) => 1 | HFLIP,
        _ => 0,
    });
    store16(BG0CNT, BG_MOSAIC | BG_8BPP | 24 << 8);
    store16(BG0HOFS, 3);
    store16(BG0HOFS + 2, 5);

    // BG1 (priority 1, no mosaic): screen base 26, scrolled by (7, 2).
    // Tile 2 in even columns of rows 1, 4, 7, ..., flipped vertically in
    // columns 2, 6, 10, ....
    text_map(26, |tx, ty| match (ty % 3, tx % 4) {
        (1, 0) => 2,
        (1, 2) => 2 | VFLIP,
        _ => 0,
    });
    store16(BG1CNT, 1 | BG_8BPP | 26 << 8);
    store16(BG1HOFS, 7);
    store16(BG1HOFS + 2, 2);

    // BG2 (priority 2, mosaic): affine 128x128, wrapping, screen base 28.
    // Tile 3 on a checker. Rotated by 30 degrees from map (-1000, 3000)/256.
    for ty in 0..16 {
        for tx in (0..16).step_by(2) {
            let t = |tx: usize| if (tx + ty) % 2 == 0 { 3 } else { 0 };
            store16(VRAM + 28 * 0x800 + ty * 16 + tx, t(tx) | t(tx + 1) << 8);
        }
    }
    store16(BG2CNT, 2 | BG_MOSAIC | 28 << 8 | WRAP);
    bg_affine(BG2PA, [222, 128, -128, 222], -1000, 3000);

    // Background blocks 5 wide and 3 tall, sprite blocks 4 wide and 6 tall.
    store16(MOSAIC, 4 | 2 << 4 | 3 << 8 | 5 << 12);

    // The 16x16 8bpp sprite, 1D: tile (tc, tr) at unit (2 tr + tc) * 2.
    for r in 0..16 {
        for c in (0..16).step_by(2) {
            let at = ((r / 8) * 4 + (c / 8) * 2) * 32 + (r % 8) * 8 + c % 8;
            store16(OBJ_VRAM + at, obj_index(c, r) | obj_index(c + 1, r) << 8);
        }
    }

    // Sprites: all 128 hidden, then
    // 0: mosaic at (21, 13), off the blocks' grid, priority 0;
    // 1: mosaic, flipped both ways, at (60, 36), priority 1 (behind BG0);
    // 2: no mosaic at (100, 50);
    // 3: affine, double size, 45 degrees (group 0), at (140, 60);
    // 4: affine, mosaic, 90 degrees (group 1), at (180, 96).
    for n in 0..128 {
        obj(n, 1 << 9, 0, 0);
    }
    obj_affine(0, [181, 181, -181, 181]);
    obj_affine(1, [0, 0x100, -0x100, 0]);
    obj(0, 13 | OBJ_MOSAIC | BPP8, 21 | SIZE16, 0);
    obj(1, 36 | OBJ_MOSAIC | BPP8, 60 | OBJ_HFLIP | OBJ_VFLIP | SIZE16, 1 << 10);
    obj(2, 50 | BPP8, 100 | SIZE16, 0);
    obj(3, 60 | AFFINE | DOUBLE | BPP8, 140 | SIZE16, 0);
    obj(4, 96 | AFFINE | OBJ_MOSAIC | BPP8, 180 | 1 << 9 | SIZE16, 0);

    store16(DISPCNT, MODE1 | BG0_ON | BG1_ON | BG2_ON | OBJ_ON | OBJ_1D);
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    scene();
    loop {
        vblank.wait_for_vblank();
    }
}
