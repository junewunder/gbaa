//! gbaa test ROM template: a fixed mode-0 scene of one text background
//! and two sprites, set up by writing the display registers directly.
//!
//! Screen-feature units copy this directory to `agb/<name>/`, rename the
//! package (`Cargo.toml` and `Cargo.lock`), change `scene` to set up what
//! they test, and list the pixels it must draw in `SCENE.md`.
//!
//! VRAM, palette and OAM take halfword and word stores only: a byte store
//! to OAM or OBJ VRAM is dropped, and one to palette or BG VRAM writes the
//! byte to both halves. So everything here goes through `store16` and
//! `fill16`; never write those regions through a `u8` pointer, `memcpy`
//! or `memset` (which may store bytes).
#![no_std]
#![no_main]

use core::ptr::write_volatile;

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const BG0CNT: usize = IO + 0x8;
const PAL: usize = 0x0500_0000;
const OBJ_PAL: usize = PAL + 0x200;
const VRAM: usize = 0x0600_0000;
const OBJ_VRAM: usize = VRAM + 0x1_0000;
const OAM: usize = 0x0700_0000;

const MODE0: u16 = 0;
const BG0_ON: u16 = 1 << 8;
const OBJ_ON: u16 = 1 << 12;
const OBJ_1D: u16 = 1 << 6;

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

/// The 4bpp tile (32 bytes) at `addr`, every pixel colour `c` (0-15).
fn tile4(addr: usize, c: u16) {
    fill16(addr, 16, (c & 0xF) * 0x1111);
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

    // Palettes: grey backdrop, BG colour 1 red, OBJ colour 1 green.
    store16(PAL, 0x4210);
    store16(PAL + 2, 0x001F);
    store16(OBJ_PAL + 2, 0x03E0);

    // BG0: character base 0, screen block 31, 32x32 tiles, 4bpp. Tile 1
    // is solid colour 1; the map's top-left 4x4 entries use it, the rest
    // tile 0 (transparent, so the backdrop shows).
    tile4(VRAM + 32, 1);
    fill16(VRAM + 31 * 0x800, 32 * 32, 0);
    for ty in 0..4 {
        for tx in 0..4 {
            store16(VRAM + 31 * 0x800 + (ty * 32 + tx) * 2, 1);
        }
    }
    store16(BG0CNT, 31 << 8);

    // Sprites: OBJ tiles 1-4 solid colour 1. All 128 hidden, then sprite 0
    // (8x8, tile 1) at (16, 16) over the BG and sprite 1 (16x16, tiles 1-4
    // in 1D mapping) at (100, 60) over the backdrop.
    for t in 1..5 {
        tile4(OBJ_VRAM + t * 32, 1);
    }
    for n in 0..128 {
        obj(n, 1 << 9, 0, 0);
    }
    obj(0, 16, 16, 1);
    obj(1, 60, 100 | 1 << 14, 1);

    store16(DISPCNT, MODE0 | BG0_ON | OBJ_ON | OBJ_1D);
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    scene();
    loop {
        vblank.wait_for_vblank();
    }
}
