//! gbaa test ROM: the BIOS sound driver's calls (swi 0x1A-0x1E, 0x20-0x24,
//! 0x28, 0x29) and `SoundDriverGetJumpList` (0x2A), as mGBA's HLE BIOS has
//! them: the driver's calls return at once, and the jump list is 36 words
//! of the address of a `bx lr`, which a call through returns from.
//!
//! Each call's registers after it are drawn as pixels in mode 3, so a frame
//! hash compares them with mGBA's bit for bit; a block below is green when
//! every one is what mGBA's BIOS leaves, red otherwise. `SCENE.md` lists
//! the rows.
#![no_std]
#![no_main]

use core::arch::asm;
use core::ptr::write_volatile;

const DISPCNT: usize = 0x0400_0000;
const VRAM: usize = 0x0600_0000;
const WIDTH: usize = 240;

const MODE3: u16 = 3;
const BG2_ON: u16 = 1 << 10;

const GREEN: u16 = 0x03E0;
const RED: u16 = 0x001F;
const GREY: u16 = 0x4210;

/// The address of the `bx lr` the jump list holds (mGBA's `NopCall`).
const NOP_CALL: u32 = 0x1B0;

/// The registers a call is given: distinct values, to see any it changes.
const ARGS: [u32; 4] = [0x0300_1000, 0x1111_2222, 0x3333_4444, 0x5555_6666];

/// The pixel at (x, y).
fn pixel(x: usize, y: usize, c: u16) {
    // SAFETY: (x, y) is on screen, in mode 3's frame in VRAM, which takes
    // halfword stores.
    unsafe { write_volatile((VRAM + (y * WIDTH + x) * 2) as *mut u16, c) }
}

/// Word `w` as three pixels from (x, y): bits 0-14, 15-29 and 30-31.
fn word(x: usize, y: usize, w: u32) {
    pixel(x, y, (w & 0x7FFF) as u16);
    pixel(x + 1, y, (w >> 15 & 0x7FFF) as u16);
    pixel(x + 2, y, (w >> 30) as u16);
}

/// `swi N` from Thumb, with r0-r3 set to `r`; returns r0-r3 after it.
macro_rules! swi {
    ($n:literal, $r:expr) => {{
        let mut r: [u32; 4] = $r;
        // SAFETY: the sound driver's calls and the jump list touch only
        // the registers named here and, for the jump list, the 36 words at
        // r0, which callers point at a buffer that size.
        unsafe {
            asm!(
                concat!("swi ", $n),
                inout("r0") r[0],
                inout("r1") r[1],
                inout("r2") r[2],
                inout("r3") r[3],
                out("r12") _,
            );
        }
        r
    }};
}

fn run() -> bool {
    let mut ok = true;
    // Row 0: each driver call's r0-r3, which it leaves as they were.
    let after = [
        swi!("0x1A", ARGS),
        swi!("0x1B", ARGS),
        swi!("0x1C", ARGS),
        swi!("0x1D", ARGS),
        swi!("0x1E", ARGS),
        swi!("0x20", ARGS),
        swi!("0x21", ARGS),
        swi!("0x22", ARGS),
        swi!("0x23", ARGS),
        swi!("0x24", ARGS),
        swi!("0x28", ARGS),
        swi!("0x29", ARGS),
    ];
    for (i, r) in after.iter().enumerate() {
        for (j, &v) in r.iter().enumerate() {
            word(i * 12 + j * 3, 0, v);
        }
        ok &= *r == ARGS;
    }

    // Row 2: the jump list's 36 words; row 4: r0-r3 after it (r0 past the
    // list, r1 0, r2 kept, r3 the `bx lr`).
    let mut list = [0u32; 36];
    let at = list.as_mut_ptr() as u32;
    let r = swi!("0x2A", [at, ARGS[1], ARGS[2], ARGS[3]]);
    // SAFETY: the BIOS wrote the list behind the compiler's back.
    let list = unsafe { core::ptr::read_volatile(&list) };
    for (i, &v) in list.iter().enumerate() {
        word(i * 3, 2, v);
        ok &= v == NOP_CALL;
    }
    for (j, &v) in r.iter().enumerate() {
        word(j * 3, 4, v.wrapping_sub(if j == 0 { at } else { 0 }));
    }
    ok &= r == [at + 36 * 4, 0, ARGS[2], NOP_CALL];

    // Row 6: a call through the list, which returns its argument.
    // SAFETY: the list's entry is a `bx lr` in ARM state, which leaves r0.
    let f: extern "C" fn(u32) -> u32 = unsafe { core::mem::transmute(list[0] as usize) };
    let back = f(0xCAFE_F00D);
    word(0, 6, back);
    ok &= back == 0xCAFE_F00D;
    ok
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // Forced blank while drawing.
    // SAFETY: DISPCNT takes halfword stores.
    unsafe { write_volatile(DISPCNT as *mut u16, 1 << 7) };
    for y in 0..160 {
        for x in 0..WIDTH {
            pixel(x, y, GREY);
        }
    }
    let c = if run() { GREEN } else { RED };
    for y in 40..80 {
        for x in 40..80 {
            pixel(x, y, c);
        }
    }
    // SAFETY: as above.
    unsafe { write_volatile(DISPCNT as *mut u16, MODE3 | BG2_ON) };
    loop {
        vblank.wait_for_vblank();
    }
}
