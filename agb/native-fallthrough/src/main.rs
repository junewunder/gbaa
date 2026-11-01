//! gbaa test ROM for the stage-A lifter (native/lift-fallthrough;
//! docs/native-stage-b.md, piece 1): a function that falls into another
//! function's entry with registers and flags the second one reads.
//!
//! Three Thumb functions in `global_asm!`, in this order in ROM:
//!
//! - `ft_a(x)` pushes r4 and lr, sets r4 = 4x + 3 and the carry to x's
//!   bit 0, and runs straight on into `ft_b`'s first instruction (no
//!   branch: the next function's entry is the next halfword).
//! - `ft_b` returns `(r0 + r4 + C) ^ 0x5A5A`, then pops the frame its
//!   caller pushed (r4, then the return address) and returns through it:
//!   the shared epilogue `ft_a` relies on. It reads r4 and the carry as
//!   they arrive, so what `ft_a` set reaches the result.
//! - `ft_c(x)` calls `ft_b` through a function pointer (`bl` to a `bx r3`
//!   stub, agbcc's `_call_via_r3`: a plain `bl` to code that pops its
//!   caller's frame is a far jump to discovery, which would copy `ft_b`
//!   into both callers rather than make it a function), with the frame it expects
//!   (r4 and lr pushed), r4 0 and the carry clear: `(x + 0 + 0) ^ 0x5A5A`.
//!   `ft_b` returns straight to `ft_c`'s caller; the epilogue after the
//!   `bl` is never run on the hardware (a lift that runs it pops `ft_c`'s
//!   own frame, which comes to the same).
//!
//! Each frame `f` (from 0) the program stores `ft_a(f)` and `ft_c(f)` to
//! `RESULTS`, a global in RAM, and draws them in mode 3 as two pixels:
//! `ft_a`'s at (f % 240, f / 240 % 80), `ft_c`'s 80 rows below, each the
//! result's low 15 bits. A lift that starts `ft_b` with r4 and the flags
//! zeroed when `ft_a` falls into it (stage A's old tail call) gets
//! `(f + 0 + 0) ^ 0x5A5A` for `ft_a`, wrong from frame 0. What it draws:
//! `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::write_volatile;

core::arch::global_asm!(
    ".section .text.native_fallthrough, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".thumb",
    // u32 ft_a(u32 x)
    ".global ft_a",
    ".type ft_a, %function",
    ".thumb_func",
    "ft_a:",
    "    push {{r4, lr}}",
    "    lsls r4, r0, #2",
    "    adds r4, r4, #3",
    "    lsrs r1, r0, #1",
    // Falls into ft_b.
    ".global ft_b",
    ".type ft_b, %function",
    ".thumb_func",
    "ft_b:",
    "    adcs r0, r4",
    "    movs r1, #0x5A",
    "    lsls r1, r1, #8",
    "    adds r1, #0x5A",
    "    eors r0, r1",
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    // u32 ft_c(u32 x)
    ".global ft_c",
    ".type ft_c, %function",
    ".thumb_func",
    "ft_c:",
    "    push {{r4, lr}}",
    "    movs r4, #0",
    "    adds r1, r4, #0",
    "    ldr r3, =ft_b",
    "    bl ft_via_r3",
    // Never run on the hardware: ft_b returned for us.
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    ".ltorg",
    // agbcc's _call_via_r3: a call through a function pointer.
    ".thumb_func",
    "ft_via_r3:",
    "    bx r3",
);

unsafe extern "C" {
    fn ft_a(x: u32) -> u32;
    fn ft_c(x: u32) -> u32;
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
        // SAFETY: ft_a and ft_c take x in r0 and return in r0, keeping
        // r4-r11 and sp, as AAPCS does.
        let (a, c) = unsafe { (ft_a(f), ft_c(f)) };
        let (x, y) = ((f % 240) as usize, (f / 240 % 80) as usize);
        // SAFETY: RESULTS is ours (see its Sync); the pixels are in mode
        // 3's frame buffer, which takes halfword stores.
        unsafe {
            write_volatile(RESULTS.0.get(), [a, c]);
            write_volatile(VRAM.add(y * 240 + x), a as u16 & 0x7FFF);
            write_volatile(VRAM.add((y + 80) * 240 + x), c as u16 & 0x7FFF);
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
