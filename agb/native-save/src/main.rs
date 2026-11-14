//! gbaa test ROM for the save chip in lifted code (native/lift-save-memory):
//! a program whose cartridge is 32 KB of SRAM (`0x0E00_0000`), written and
//! read back by the CPU as an SRAM game does.
//!
//! Each frame `f` (from 0) the main loop stores eight bytes into SRAM
//! (`strb`) at addresses that walk the whole 32 KB, a halfword at an odd
//! address and a word at an address 3 past a word (`strh`, `str`: the
//! chip's 8-bit bus takes only the addressed byte lane), then loads every
//! one back (`ldrb`), a halfword and a word (`ldrh`, `ldr`: the byte
//! repeated), and sixteen bytes stored 64 frames earlier. What it read is
//! kept in `OUT`, a global in RAM, and folded into a pixel of mode 3: row
//! `f / 240 % 4`, column `f % 240`. A lift that stores to or loads from
//! the chip wrongly, or takes the cartridge for another kind of chip,
//! leaves different RAM and draws a different frame. What it draws:
//! `SCENE.md`.
//!
//! The ROM carries `SRAM_V113`, the tag of Nintendo's SRAM library, which
//! the play build's and the lift's save detection (`runtime::save::Kind::
//! detect`) read; no other library tag is in it.
#![no_std]
#![no_main]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: u32 = 0x0600_0000;
const SRAM: u32 = 0x0E00_0000;
const SRAM_SIZE: u32 = 0x8000;

/// The SRAM library's tag, as a game's ROM has it.
static TAG: [u8; 12] = *b"SRAM_V113\0\0\0";

/// The last frame's loads, for the native check to compare.
#[repr(C, align(4))]
struct Out {
    /// The eight bytes stored this frame, loaded back.
    bytes: [u8; 8],
    /// Sixteen bytes from where frame `f - 64` stored.
    old: [u8; 16],
    /// `ldrh` and `ldr` of this frame's halfword and word stores' bytes.
    half: u32,
    word: u32,
    /// `ldrb` of the bytes either side of each lane store.
    lanes: [u8; 4],
}

struct Global(UnsafeCell<Out>);

// SAFETY: only the main loop touches it; agb's handler doesn't.
unsafe impl Sync for Global {}

static OUT: Global = Global(UnsafeCell::new(Out {
    bytes: [0; 8],
    old: [0; 16],
    half: 0,
    word: 0,
    lanes: [0; 4],
}));

/// Where frame `f`'s `i`th byte goes: spread over the whole chip.
fn at(f: u32, i: u32) -> u32 {
    SRAM + (f.wrapping_mul(37).wrapping_add(i * 4099)) % SRAM_SIZE
}

fn ldrb(a: u32) -> u8 {
    // SAFETY: a byte of SRAM.
    unsafe { read_volatile(a as *const u8) }
}

/// One frame's stores and loads, into `o`.
fn frame(f: u32, o: &mut Out) {
    for i in 0..8 {
        // SAFETY: a byte of SRAM.
        unsafe { write_volatile(at(f, i) as *mut u8, (f.wrapping_mul(13) ^ i * 0x35) as u8) };
    }
    // A halfword at an odd address and a word 3 past a word: SRAM takes
    // the addressed byte lane only.
    let h = SRAM + 0x100 + (f * 2 % 0x400) + 1;
    let w = SRAM + 0x800 + (f * 4 % 0x400) + 3;
    // SAFETY: SRAM addresses; the chip has no alignment.
    unsafe {
        asm!("strh {v}, [{a}]", v = in(reg) 0xA500 | (f & 0xFF), a = in(reg) h);
        asm!("str {v}, [{a}]", v = in(reg) 0x1234_5600 | (f & 0xFF) | (f & 0xFF) << 24, a = in(reg) w);
    }
    for i in 0..8 {
        o.bytes[i as usize] = ldrb(at(f, i));
    }
    let old = f.wrapping_sub(64);
    for i in 0..16 {
        o.old[i as usize] = ldrb(at(old, i % 8) ^ (i / 8));
    }
    // SAFETY: SRAM, a halfword and a word load (aligned: the chip repeats
    // the addressed byte across the value).
    unsafe {
        o.half = read_volatile((h - 1) as *const u16) as u32;
        o.word = read_volatile((w - 3) as *const u32);
    }
    o.lanes = [ldrb(h - 1), ldrb(h), ldrb(w - 1), ldrb(w)];
}

/// The frame's loads folded into a BGR555 colour.
fn colour(o: &Out) -> u16 {
    let mut v: u32 = o.half ^ o.word.rotate_left(7);
    for (i, &b) in o.bytes.iter().chain(&o.old).chain(&o.lanes).enumerate() {
        v = v.rotate_left(5) ^ (b as u32) << (i % 3);
    }
    ((v ^ v >> 15 ^ v >> 30) & 0x7FFF) as u16
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: the tag is ROM; reading it keeps it in the ROM.
    let _ = unsafe { read_volatile(&TAG[0]) };
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let mut f: u32 = 0;
    loop {
        // SAFETY: OUT is ours (see its Sync).
        let o = unsafe { &mut *OUT.0.get() };
        frame(f, o);
        let px = VRAM + ((f / 240 % 4) * 240 + f % 240) * 2;
        // SAFETY: a pixel of mode 3's frame buffer, rows 0-3.
        unsafe { write_volatile(px as *mut u16, colour(o)) };
        vblank.wait_for_vblank();
        f += 1;
    }
}
