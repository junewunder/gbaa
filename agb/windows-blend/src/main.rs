//! gbaa test ROM for windows and colour effects: two text backgrounds,
//! WIN0, WIN1 and the OBJ window, brightening and semi-transparent
//! sprites, set up by writing the display registers directly.
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
const BG0CNT: usize = IO + 0x8;
const BG1CNT: usize = IO + 0xA;
const WIN0H: usize = IO + 0x40;
const WIN1H: usize = IO + 0x42;
const WIN0V: usize = IO + 0x44;
const WIN1V: usize = IO + 0x46;
const WININ: usize = IO + 0x48;
const WINOUT: usize = IO + 0x4A;
const BLDCNT: usize = IO + 0x50;
const BLDALPHA: usize = IO + 0x52;
const BLDY: usize = IO + 0x54;
const PAL: usize = 0x0500_0000;
const OBJ_PAL: usize = PAL + 0x200;
const VRAM: usize = 0x0600_0000;
const OBJ_VRAM: usize = VRAM + 0x1_0000;
const OAM: usize = 0x0700_0000;

const MODE0: u16 = 0;
const BG0_ON: u16 = 1 << 8;
const BG1_ON: u16 = 1 << 9;
const OBJ_ON: u16 = 1 << 12;
const OBJ_1D: u16 = 1 << 6;
const WIN0_ON: u16 = 1 << 13;
const WIN1_ON: u16 = 1 << 14;
const OBJWIN_ON: u16 = 1 << 15;

/// Window enable bits.
const W_BG0: u16 = 1;
const W_BG1: u16 = 2;
const W_OBJ: u16 = 1 << 4;
const W_FX: u16 = 1 << 5;

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

    // Palettes: grey backdrop, BG colour 1 red, 2 blue, OBJ colour 1 green.
    store16(PAL, 0x4210);
    store16(PAL + 2, 0x001F);
    store16(PAL + 4, 0x7C00);
    store16(OBJ_PAL + 2, 0x03E0);

    // Character base 0: tile 1 solid colour 1, tile 2 solid colour 2.
    tile4(VRAM + 32, 1);
    tile4(VRAM + 64, 2);
    // BG0 (screen block 30, priority 0): red over x < 120, every row.
    fill16(VRAM + 30 * 0x800, 32 * 32, 0);
    for ty in 0..32 {
        fill16(VRAM + 30 * 0x800 + ty * 64, 15, 1);
    }
    store16(BG0CNT, 30 << 8);
    // BG1 (screen block 31, priority 1): blue over y < 128, every column.
    fill16(VRAM + 31 * 0x800, 32 * 32, 0);
    fill16(VRAM + 31 * 0x800, 16 * 32, 2);
    store16(BG1CNT, 31 << 8 | 1);

    // Sprites: OBJ tiles 1-16 solid colour 1, all 128 hidden, then
    // 0: OBJ window, 32x32 at (104, 20);
    // 1: semi-transparent 16x16 at (180, 100), over BG1;
    // 2: normal 8x8 at (40, 136), over BG0;
    // 3: semi-transparent 8x8 at (140, 64), in WIN1.
    for t in 1..17 {
        tile4(OBJ_VRAM + t * 32, 1);
    }
    for n in 0..128 {
        obj(n, 1 << 9, 0, 0);
    }
    obj(0, 20 | 2 << 10, 104 | 2 << 14, 1);
    obj(1, 100 | 1 << 10, 180 | 1 << 14, 1);
    obj(2, 136, 40, 1);
    obj(3, 64 | 1 << 10, 140, 1);

    // WIN0 (16..48, 16..48): BG1 only, no effects. WIN1 (32..160,
    // 32..96): both BGs and sprites, no effects. OBJ window: BG1, effects.
    // Outside: everything.
    store16(WIN0H, 16 << 8 | 48);
    store16(WIN0V, 16 << 8 | 48);
    store16(WIN1H, 32 << 8 | 160);
    store16(WIN1V, 32 << 8 | 96);
    store16(WININ, W_BG1 | (W_BG0 | W_BG1 | W_OBJ) << 8);
    store16(WINOUT, W_BG0 | W_BG1 | W_OBJ | W_FX | (W_BG1 | W_FX) << 8);

    // Brighten BG0 and sprites by 8/16; BG1 is the second target, for the
    // semi-transparent sprites, at 8/16 each. (Sprites are a first target
    // because mGBA, with the OBJ window on, brightens sprites that aren't.)
    store16(BLDCNT, 1 | 1 << 4 | 2 << 6 | 2 << 8);
    store16(BLDALPHA, 8 << 8 | 8);
    store16(BLDY, 8);

    store16(
        DISPCNT,
        MODE0 | BG0_ON | BG1_ON | OBJ_ON | OBJ_1D | WIN0_ON | WIN1_ON | OBJWIN_ON,
    );
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    scene();
    loop {
        vblank.wait_for_vblank();
    }
}
