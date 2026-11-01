//! gbaa test ROM for the BIOS's diff unfilter calls: the scene's palette,
//! tiles and map come out of them, each from delta-filtered data in ROM.
//!
//! - `Diff16bitUnFilter` (swi 0x18) writes the 16 BG colours straight into
//!   palette RAM.
//! - `Diff8bitUnFilterVram` (swi 0x17) writes four 4bpp tiles straight
//!   into VRAM, in halfwords.
//! - `Diff8bitUnFilterWram` (swi 0x16) writes 64 tile numbers into EWRAM,
//!   which become an 8x8 corner of BG0's map.
//!
//! `SCENE.md` says what the frame shows. VRAM, palette and OAM take
//! halfword stores only, so everything else goes through `store16`.
#![no_std]
#![no_main]

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const BG0CNT: usize = IO + 0x8;
const PAL: usize = 0x0500_0000;
const VRAM: usize = 0x0600_0000;
const MAP: usize = VRAM + 31 * 0x800;

const MODE0: u16 = 0;
const BG0_ON: u16 = 1 << 8;

/// One halfword at `addr`.
fn store16(addr: usize, v: u16) {
    // SAFETY: callers pass addresses in IO, palette or VRAM, which take
    // halfword stores.
    unsafe { write_volatile(addr as *mut u16, v) }
}

/// BG colour `i`: red 2i, green 31 - 2i, blue 5i mod 32.
const fn colour(i: usize) -> u16 {
    (2 * i | (31 - 2 * i) << 5 | (5 * i % 32) << 10) as u16
}

/// Byte `i` of the four tiles (32 bytes each, 4 a row): its two pixels,
/// low nibble left, are colours `(i + i / 4) & 15` and the next one up.
const fn tile_byte(i: usize) -> u8 {
    let c = i + i / 4;
    (c & 15 | (c + 1) % 16 << 4) as u8
}

/// Map entry `i` (row `i / 8`, column `i % 8`): tile `(7i + i / 8) % 4`.
const fn map_byte(i: usize) -> u8 {
    ((7 * i + i / 8) % 4) as u8
}

/// A filter header and its deltas, word-aligned as the BIOS wants them.
#[repr(C, align(4))]
struct Filtered<T> {
    header: u32,
    deltas: T,
}

/// `out` as 8-bit deltas: each byte minus the one before.
const fn filter8<const N: usize>(out: [u8; N]) -> Filtered<[u8; N]> {
    let mut deltas = [0; N];
    let mut i = 0;
    while i < N {
        let before = if i == 0 { 0 } else { out[i - 1] };
        deltas[i] = out[i].wrapping_sub(before);
        i += 1;
    }
    Filtered {
        header: 0x81 | (N as u32) << 8,
        deltas,
    }
}

/// `out` as 16-bit deltas; the size in the header is in bytes.
const fn filter16<const N: usize>(out: [u16; N]) -> Filtered<[u16; N]> {
    let mut deltas = [0; N];
    let mut i = 0;
    while i < N {
        let before = if i == 0 { 0 } else { out[i - 1] };
        deltas[i] = out[i].wrapping_sub(before);
        i += 1;
    }
    Filtered {
        header: 0x82 | (2 * N as u32) << 8,
        deltas,
    }
}

static PALETTE: Filtered<[u16; 16]> = filter16({
    let mut out = [0; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = colour(i);
        i += 1;
    }
    out
});

static TILES: Filtered<[u8; 128]> = filter8({
    let mut out = [0; 128];
    let mut i = 0;
    while i < 128 {
        out[i] = tile_byte(i);
        i += 1;
    }
    out
});

static MAP_BYTES: Filtered<[u8; 64]> = filter8({
    let mut out = [0; 64];
    let mut i = 0;
    while i < 64 {
        out[i] = map_byte(i);
        i += 1;
    }
    out
});

/// Where `Diff8bitUnFilterWram` writes the map's tile numbers.
#[repr(align(4))]
struct Buf([u8; 64]);
static mut MAP_BUF: Buf = Buf([0; 64]);

/// BIOS call `N` (an unfilter) from `src` to `dst`.
fn unfilter<const N: u32, T>(src: &Filtered<T>, dst: usize) {
    // SAFETY: the unfilter calls read the header and deltas at r0 and write
    // the size the header gives at r1; callers size `dst` for it. The BIOS
    // may change r0-r3.
    unsafe {
        asm!(
            "swi {n}",
            n = const N,
            inout("r0") src as *const Filtered<T> => _,
            inout("r1") dst => _,
            out("r2") _,
            out("r3") _,
        );
    }
}

/// The scene `SCENE.md` describes.
fn scene() {
    // Forced blank while setting up.
    store16(DISPCNT, 1 << 7);

    unfilter::<0x18, _>(&PALETTE, PAL);
    unfilter::<0x17, _>(&TILES, VRAM);
    let buf = &raw mut MAP_BUF;
    unfilter::<0x16, _>(&MAP_BYTES, buf as usize);

    // BG0: character base 0, screen block 31, 32x32 tiles, 4bpp. The map's
    // top-left 8x8 entries are the unfiltered tile numbers; the rest is
    // tile 0, which the unfilter wrote too.
    for i in 0..32 * 32 {
        store16(MAP + i * 2, 0);
    }
    for i in 0..64 {
        // SAFETY: the BIOS wrote the buffer and nothing else touches it.
        let t = unsafe { read_volatile((buf as *const u8).add(i)) };
        store16(MAP + ((i / 8) * 32 + i % 8) * 2, t as u16);
    }
    store16(BG0CNT, 31 << 8);

    store16(DISPCNT, MODE0 | BG0_ON);
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    scene();
    loop {
        vblank.wait_for_vblank();
    }
}
