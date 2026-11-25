//! gbaa test ROM for the stage-A lifter (native/lift-takes-across-copies;
//! docs/research/native-holes.md, "Entry reads of r4-r11"): a function
//! copied to IWRAM that pointer-calls a ROM function reading r8 and r9 as
//! the copy's caller left them, itself pointer-called from ROM, so both
//! of the holes file's `[[takes]]` sites cross a code context, as Minish
//! Cap's IWRAM entity loop (called from ROM, calling the ROM NPC updater)
//! does.
//!
//! Hand-written in `global_asm!`:
//!
//! - `tc_set(x, out)` (ARM, ROM) keeps r4, r8, r9 and lr, sets
//!   r8 = x + 0x23 and r9 = x ^ 0x2A0, and calls `tc_c(x, out)` through
//!   `mov lr, pc; bx r12`, r12 its IWRAM address.
//! - `tc_c(x, out)` (ARM, `.iwram`: agb's start-up copies it to IWRAM)
//!   sets neither r8 nor r9, and calls `tc_b(x, out)` in ROM through
//!   `mov lr, pc; bx r12`, so they are `tc_set`'s.
//! - `tc_b(x, out)` (ARM, ROM) stores `(r8 - x) ^ (r9 << 1)` to
//!   `out[0]`, by way of r2. (Stored, not returned, and not left in r0:
//!   the summaries take an entry value returned in r0-r3 for a hard
//!   hole, not a parameter.)
//! - `tc_run(x, out)` (Thumb), the main loop's way in: Thumb can't `bl`
//!   to ARM code on the ARM7TDMI, so it calls `tc_set` through `bx r3`
//!   (`tc_via_r3`, agbcc's `_call_via_r3`).
//!
//! `holes/native-copy-takes.toml`'s `[[takes]]` name both calls. Each
//! frame `f` (from 0) the program runs `tc_run(f, RESULT)`, RESULT a
//! global in RAM, and draws the result in mode 3 as one pixel at
//! (f % 240, f / 240 % 160), its low 15 bits. A lift whose named calls
//! pass no r8 and r9 (`gbaa-lift --takes-zeroed`, the old plain entry)
//! draws `-f` instead, wrong from frame 0. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::write_volatile;

core::arch::global_asm!(
    ".section .text.native_copy_takes, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".arm",
    // void tc_set(u32 x, u32 out[1])
    ".global tc_set",
    ".type tc_set, %function",
    "tc_set:",
    "    push {{r4, r8, r9, lr}}",
    "    add r8, r0, #0x23",
    "    eor r9, r0, #0x2A0",
    "    ldr r12, =tc_c",
    "    mov lr, pc",
    "    bx r12",
    "    pop {{r4, r8, r9, lr}}",
    "    bx lr",
    // void tc_b(u32 x, u32 out[1]), r8 and r9 tc_c's caller's
    ".global tc_b",
    ".type tc_b, %function",
    "tc_b:",
    "    sub r2, r8, r0",
    "    eor r2, r2, r9, lsl #1",
    "    str r2, [r1]",
    "    bx lr",
    ".ltorg",
    ".thumb",
    // void tc_run(u32 x, u32 out[1])
    ".global tc_run",
    ".type tc_run, %function",
    ".thumb_func",
    "tc_run:",
    "    push {{r4, lr}}",
    "    ldr r3, =tc_set",
    "    bl tc_via_r3",
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    ".ltorg",
    // agbcc's _call_via_r3: a call through a function pointer.
    ".thumb_func",
    "tc_via_r3:",
    "    bx r3",
    // void tc_c(u32 x, u32 out[1]), r8 and r9 its caller's; in IWRAM
    // (agb's crt0 copies `.iwram` there at start-up)
    ".section .iwram.native_copy_takes, \"ax\", %progbits",
    ".balign 4",
    // A word first, so tc_c isn't at the copy's start (crt0's copy
    // destination, a ROM word, would be tc_c's key too).
    "    .word 0",
    ".arm",
    ".global tc_c",
    ".type tc_c, %function",
    "tc_c:",
    "    push {{r4, lr}}",
    "    ldr r12, =tc_b",
    "    mov lr, pc",
    "    bx r12",
    "    pop {{r4, lr}}",
    "    bx lr",
    ".ltorg",
    ".thumb",
);

unsafe extern "C" {
    fn tc_run(x: u32, out: *mut u32);
}

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

/// The last frame's result, for the native check to compare.
struct Result(UnsafeCell<u32>);

// SAFETY: only the main loop touches it; there are no interrupt handlers.
unsafe impl Sync for Result {}

static RESULT: Result = Result(UnsafeCell::new(0));

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let mut f: u32 = 0;
    loop {
        // SAFETY: tc_run takes x and out in r0-r1, writes out's word and
        // keeps r4-r11 and sp, as AAPCS does.
        let c = unsafe {
            tc_run(f, RESULT.0.get());
            *RESULT.0.get()
        };
        let (x, y) = ((f % 240) as usize, (f / 240 % 160) as usize);
        // SAFETY: the pixel is in mode 3's frame buffer, which takes
        // halfword stores.
        unsafe { write_volatile(VRAM.add(y * 240 + x), c as u16 & 0x7FFF) };
        vblank.wait_for_vblank();
        f += 1;
    }
}
