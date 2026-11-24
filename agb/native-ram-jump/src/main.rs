//! gbaa test ROM for the stage-A lifter (native/ram-jump-passes-regs;
//! docs/research/native-holes.md, "Entry reads of r4-r11"): a function
//! in ROM that jumps into code copied to IWRAM, which reads r8 and r9 as
//! the jumper's caller left them, so the lift's jump passes them and the
//! jumper takes them from its callers (`crate::saves::Passing::foreign`),
//! as WarioWare's sound driver's `bx r0` into its IWRAM routines does.
//!
//! Hand-written in `global_asm!`:
//!
//! - `tj_set(x, out)` (ARM) keeps r8, r9 and lr, sets r8 = x + 0x23 and
//!   r9 = x ^ 0x2A0, and `bl`s `tj_jump(x, out)`.
//! - `tj_jump(x, out)` (ARM, ROM) is `ldr r12, =tj_c; bx r12`: a
//!   constant jump into IWRAM, with every register its caller's. It
//!   sets neither r8 nor r9, so they are `tj_set`'s.
//! - `tj_c(x, out)` (ARM, `.iwram`: agb's start-up copies it to IWRAM)
//!   stores `(r8 - x) ^ (r9 << 1)` to `out[0]`. (Stored, not returned:
//!   the summaries take an entry value only returned in r0-r3 for a
//!   hard hole, not a parameter.)
//! - `tj_run(x, out)` (Thumb), the main loop's way in: Thumb can't `bl`
//!   to ARM code on the ARM7TDMI, so it calls `tj_set` through `bx r3`
//!   (`tj_via_r3`, agbcc's `_call_via_r3`).
//!
//! Each frame `f` (from 0) the program runs `tj_run(f, RESULT)`, RESULT a
//! global in RAM, and draws the result in mode 3 as one pixel at
//! (f % 240, f / 240 % 160), its low 15 bits. A lift whose call of
//! `tj_jump` passes it no r8 and r9 (`gbaa-lift --takes-zeroed`, the old
//! plain entry) draws `-f` instead, wrong from frame 0. What it draws:
//! `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::write_volatile;

core::arch::global_asm!(
    ".section .text.native_ram_jump, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".arm",
    // void tj_set(u32 x, u32 out[1])
    ".global tj_set",
    ".type tj_set, %function",
    "tj_set:",
    "    push {{r8, r9, lr}}",
    "    add r8, r0, #0x23",
    "    eor r9, r0, #0x2A0",
    "    bl tj_jump",
    "    pop {{r8, r9, lr}}",
    "    bx lr",
    // void tj_jump(u32 x, u32 out[1]): into tj_c with the caller's r8, r9
    ".global tj_jump",
    ".type tj_jump, %function",
    "tj_jump:",
    "    ldr r12, =tj_c",
    "    bx r12",
    ".ltorg",
    ".thumb",
    // void tj_run(u32 x, u32 out[1])
    ".global tj_run",
    ".type tj_run, %function",
    ".thumb_func",
    "tj_run:",
    "    push {{r4, lr}}",
    "    ldr r3, =tj_set",
    "    bl tj_via_r3",
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    ".ltorg",
    // agbcc's _call_via_r3: a call through a function pointer.
    ".thumb_func",
    "tj_via_r3:",
    "    bx r3",
    // void tj_c(u32 x, u32 out[1]), r8 and r9 its jumper's caller's; in
    // IWRAM (agb's crt0 copies `.iwram` there at start-up)
    ".section .iwram.native_ram_jump, \"ax\", %progbits",
    ".balign 4",
    // A word first, so tj_c isn't at the copy's start: crt0's copy
    // destination, a ROM word, would be tj_c's key, which the lift takes
    // for a pointer that may call it (WarioWare's 0x03006900 is so, and
    // its holes file says it isn't).
    "    .word 0",
    ".arm",
    ".global tj_c",
    ".type tj_c, %function",
    "tj_c:",
    "    sub r0, r8, r0",
    "    eor r0, r0, r9, lsl #1",
    "    str r0, [r1]",
    "    bx lr",
    ".thumb",
);

unsafe extern "C" {
    fn tj_run(x: u32, out: *mut u32);
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
        // SAFETY: tj_run takes x and out in r0-r1, writes out's word and
        // keeps r4-r11 and sp, as AAPCS does.
        let c = unsafe {
            tj_run(f, RESULT.0.get());
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
