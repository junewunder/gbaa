//! gbaa test ROM for the stage-A lifter (native/taker-test-program;
//! docs/native-stage-b.md): two functions that read r8 and r9 as their
//! caller left them, one called by `bl` and one through a function
//! pointer, so the lift passes a caller's r4-r11 to what takes them
//! (`crate::saves::Summary::takes`, unit `native/metroid-entry-reads`).
//!
//! Hand-written in `global_asm!`:
//!
//! - `tk_set(x, p, out)` (ARM) keeps r4-r6, r8, r9 and lr, sets r8 =
//!   x + 0x11 and r9 = x ^ 0x1C0, and `bl`s `tk_a(x, out)`. Then it adds
//!   0x300 to r8 and calls `p(x, out)` through the pointer (`mov lr, pc;
//!   bx r4`).
//! - `tk_a(x, out)` (ARM) stores `(x + r8) ^ (r9 << 2)` to `out[0]`: a
//!   taker of r8 and r9, called directly, so the lift's call passes
//!   `tk_set`'s. (Stored, not returned: the summaries take an entry value
//!   only returned in r0-r3 for a hard hole, not a parameter.)
//! - `tk_b(x, out)` (Thumb) stores `(r8 - x) ^ (r9 >> 1)` to `out[1]`: a
//!   taker reached
//!   only through `p`, which the Rust main loop passes in (`tk_b`'s
//!   address), so the lift passes `tk_set`'s r8 and r9 only because
//!   `holes/native-takes.toml`'s `[[takes]]` names that `bx r4`.
//! - `tk_run(x, p, out)` (Thumb), the main loop's way in: Thumb can't
//!   `bl` to ARM code on the ARM7TDMI, so it calls `tk_set` through
//!   `bx r3` (`tk_via_r3`, agbcc's `_call_via_r3`).
//!
//! Each frame `f` (from 0) the program runs `tk_run(f, tk_b, RESULTS)`,
//! RESULTS a global in RAM, and draws the two results in mode 3 as two
//! pixels: `tk_a`'s at (f % 240, f / 240 % 80), `tk_b`'s 80 rows below,
//! each the result's low 15 bits. A lift that starts either with r8 and
//! r9 zeroed (`gbaa-lift --takes-zeroed`, the old plain entry) draws `f`
//! and `-f` instead, wrong from frame 0. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::write_volatile;

core::arch::global_asm!(
    ".section .text.native_takes, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".arm",
    // void tk_set(u32 x, u32 (*p)(u32), u32 out[2])
    ".global tk_set",
    ".type tk_set, %function",
    "tk_set:",
    "    push {{r4, r5, r6, r8, r9, lr}}",
    "    mov r4, r1",
    "    mov r5, r0",
    "    mov r6, r2",
    "    add r8, r0, #0x11",
    "    eor r9, r0, #0x1C0",
    "    mov r1, r6",
    "    bl tk_a",
    "    add r8, r8, #0x300",
    "    mov r0, r5",
    "    mov r1, r6",
    "    mov lr, pc",
    "    bx r4",
    "    pop {{r4, r5, r6, r8, r9, lr}}",
    "    bx lr",
    // void tk_a(u32 x, u32 out[2]), r8 and r9 its caller's
    ".global tk_a",
    ".type tk_a, %function",
    "tk_a:",
    "    add r0, r0, r8",
    "    eor r0, r0, r9, lsl #2",
    "    str r0, [r1]",
    "    bx lr",
    ".thumb",
    // void tk_b(u32 x, u32 out[2]), r8 and r9 its caller's
    ".global tk_b",
    ".type tk_b, %function",
    ".thumb_func",
    "tk_b:",
    "    mov r2, r8",
    "    subs r0, r2, r0",
    "    mov r3, r9",
    "    lsrs r3, r3, #1",
    "    eors r0, r3",
    "    str r0, [r1, #4]",
    "    bx lr",
    // void tk_run(u32 x, u32 (*p)(u32), u32 out[2])
    ".global tk_run",
    ".type tk_run, %function",
    ".thumb_func",
    "tk_run:",
    "    push {{r4, lr}}",
    "    ldr r3, =tk_set",
    "    bl tk_via_r3",
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    ".ltorg",
    // agbcc's _call_via_r3: a call through a function pointer.
    ".thumb_func",
    "tk_via_r3:",
    "    bx r3",
);

unsafe extern "C" {
    fn tk_run(x: u32, p: unsafe extern "C" fn(u32, *mut [u32; 2]), out: *mut [u32; 2]);
    fn tk_b(x: u32, out: *mut [u32; 2]);
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
        // SAFETY: tk_run takes x, p and out in r0-r2, writes out's two
        // words and keeps r4-r11 and sp, as AAPCS does; tk_b is such a p.
        let [a, b] = unsafe {
            tk_run(f, tk_b, RESULTS.0.get());
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
