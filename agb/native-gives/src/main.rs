//! gbaa test ROM for the stage-A lifter (native/lift-gives;
//! docs/native-stage-b.md): hand-written functions that share their
//! caller's registers and leave one changed for it, so the lift's direct
//! call takes back the r4-r11 its callee gives
//! (`crate::saves::Summary::gives`). The Pokémon games' m4a mixer does
//! this: it calls its compressed-sample path by `bl` and stores the r9
//! it comes back with.
//!
//! Hand-written in `global_asm!`, all ARM but the way in:
//!
//! - `gv_mix(x, out)` keeps r4, r9 and lr, sets r9 = x + 0x21 and `bl`s
//!   `gv_step`, then stores r9 to `out[0]`; then `bl`s `gv_twice` and
//!   stores r9 to `out[1]`.
//! - `gv_step()` reads r9 as its caller left it and leaves it
//!   `r9 * 3 ^ 0x155`: a taker of r9 that gives it back.
//! - `gv_twice()` adds 0x1000 to r9 and jumps into `gv_step` (`b`): what
//!   it gives back is what `gv_step` left, so `gv_step` gives to a
//!   function that jumps into it too.
//! - `gv_run(x, out)` (Thumb), the main loop's way in: Thumb can't `bl`
//!   to ARM code on the ARM7TDMI, so it calls `gv_mix` through `bx r3`
//!   (`gv_via_r3`, agbcc's `_call_via_r3`).
//!
//! Each frame `f` (from 0) the program runs `gv_run(f, RESULTS)`,
//! RESULTS a global in RAM, and draws the two results in mode 3 as two
//! pixels: the first at (f % 240, f / 240 % 80), the second 80 rows
//! below, each the result's low 15 bits. A lift whose caller keeps its
//! own r9 after each call (`gbaa-lift --gives-kept`, stage A's old call)
//! draws `f + 0x21` twice instead, wrong from frame 0. What it draws:
//! `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::write_volatile;

core::arch::global_asm!(
    ".section .text.native_gives, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".arm",
    // void gv_mix(u32 x, u32 out[2])
    ".global gv_mix",
    ".type gv_mix, %function",
    "gv_mix:",
    "    push {{r4, r9, lr}}",
    "    mov r4, r1",
    "    add r9, r0, #0x21",
    "    bl gv_step",
    "    str r9, [r4]",
    "    bl gv_twice",
    "    str r9, [r4, #4]",
    "    pop {{r4, r9, lr}}",
    "    bx lr",
    // gv_step: r9 its caller's, left changed for it
    ".type gv_step, %function",
    "gv_step:",
    "    add r9, r9, r9, lsl #1",
    "    eor r9, r9, #0x154",
    "    eor r9, r9, #0x1",
    "    bx lr",
    // gv_twice: the same through a jump into gv_step
    ".type gv_twice, %function",
    "gv_twice:",
    "    add r9, r9, #0x1000",
    "    b gv_step",
    ".thumb",
    // void gv_run(u32 x, u32 out[2])
    ".global gv_run",
    ".type gv_run, %function",
    ".thumb_func",
    "gv_run:",
    "    push {{r4, lr}}",
    "    ldr r3, =gv_mix",
    "    bl gv_via_r3",
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    ".ltorg",
    // agbcc's _call_via_r3: a call through a function pointer.
    ".thumb_func",
    "gv_via_r3:",
    "    bx r3",
);

unsafe extern "C" {
    fn gv_run(x: u32, out: *mut [u32; 2]);
}

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

/// The last frame's two results, for the native check to compare.
struct Results(UnsafeCell<[u32; 2]>);

// SAFETY: only the main loop touches it; there are no interrupt handlers.
unsafe impl Sync for Results {}

static RESULTS: Results = Results(UnsafeCell::new([0; 2]));

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let mut f: u32 = 0;
    loop {
        // SAFETY: gv_run takes x and out in r0 and r1, writes out's two
        // words and keeps r4-r11 and sp, as AAPCS does.
        let [a, b] = unsafe {
            gv_run(f, RESULTS.0.get());
            *RESULTS.0.get()
        };
        let (x, y) = ((f % 240) as usize, (f / 240 % 80) as usize);
        // SAFETY: the pixels are in mode 3's frame buffer, which takes
        // halfword stores.
        unsafe {
            write_volatile(VRAM.add(y * 240 + x), a as u16 & 0x7FFF);
            write_volatile(VRAM.add((y + 80) * 240 + x), b as u16 & 0x7FFF);
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
