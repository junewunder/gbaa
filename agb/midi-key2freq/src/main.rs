//! gbaa test ROM for the BIOS's `MidiKey2Freq` (swi 0x1F): every pixel of
//! a mode 3 bitmap is the low bits of one call's result, for a key that
//! rises along x and a fine adjust and frequency that change along y.
//!
//! `SCENE.md` says what the frame shows. VRAM and IO take halfword stores
//! only, so everything goes through `store16`.
#![no_std]
#![no_main]

use core::arch::asm;
use core::ptr::write_volatile;

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const VRAM: usize = 0x0600_0000;

const MODE3: u16 = 3;
const BG2_ON: u16 = 1 << 10;

/// One halfword at `addr`.
fn store16(addr: usize, v: u16) {
    // SAFETY: callers pass addresses in IO or VRAM, which take halfword
    // stores.
    unsafe { write_volatile(addr as *mut u16, v) }
}

/// What `MidiKey2Freq` reads of a WaveData: its frequency, at +4.
#[repr(C, align(4))]
struct Wave {
    header: u32,
    freq: u32,
}

/// The frequencies, one per row mod 4.
static WAVES: [Wave; 4] = [
    Wave { header: 0, freq: 0x2B11_0000 },
    Wave { header: 0, freq: 3_546_000 },
    Wave { header: 0, freq: 0xE000_0000 },
    Wave { header: 0, freq: 12_345_678 },
];

/// `MidiKey2Freq` on `wave`, `key` and `fine`.
fn midi_key2freq(wave: &Wave, key: u32, fine: u32) -> u32 {
    let out;
    // SAFETY: the call reads the word at r0 + 4 and returns in r0. The
    // BIOS may change r1-r3.
    unsafe {
        asm!(
            "swi 0x1F",
            inout("r0") wave as *const Wave => out,
            inout("r1") key => _,
            inout("r2") fine => _,
            out("r3") _,
        );
    }
    out
}

/// Pixel (x, y)'s colour: the call for key 3x/4, fine adjust 37y mod 256
/// and frequency `WAVES[y % 4]`, folded into 15 bits.
fn pixel(x: usize, y: usize) -> u16 {
    let r = midi_key2freq(&WAVES[y % 4], (x * 3 / 4) as u32, (y * 37 % 256) as u32);
    ((r ^ r >> 15 ^ r >> 30) & 0x7FFF) as u16
}

/// The scene `SCENE.md` describes.
fn scene() {
    // Forced blank while drawing.
    store16(DISPCNT, 1 << 7);
    for y in 0..160 {
        for x in 0..240 {
            store16(VRAM + (y * 240 + x) * 2, pixel(x, y));
        }
    }
    store16(DISPCNT, MODE3 | BG2_ON);
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    scene();
    loop {
        vblank.wait_for_vblank();
    }
}
