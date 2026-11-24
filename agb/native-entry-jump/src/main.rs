//! gbaa test ROM for the stage-A lifter (native/entry-jump-test-program;
//! docs/native-stage-b.md, piece 1, "Falls and calls into an entry"): the
//! two ways into another function's entry besides a fall
//! (agb/native-fallthrough's), each with registers and flags the entered
//! function reads as they arrive:
//!
//! - a `b` (not a fall: the target is elsewhere in ROM) into an entry;
//! - a call whose return site is another function's entry, so the callee
//!   returns into it with its r0-r3 and the caller's other registers and
//!   flags (the callee here leaves the flags alone, as the lifter assumes).
//!
//! Thumb functions in `global_asm!`, in this order in ROM:
//!
//! - `ej_b`: returns `(r0 + r4 + C) ^ 0x3C3C`, then pops the frame its
//!   caller pushed (r4, then the return address) and returns through it.
//! - `ej_bp(x)`: calls `ej_b` through a function pointer (`bl` to a
//!   `bx r3` stub, agbcc's `_call_via_r3`, so discovery makes `ej_b` a
//!   function of its own) with that frame, r4 0 and the carry clear:
//!   `x ^ 0x3C3C`.
//! - `ej_a(x)`: pushes r4 and lr, sets r4 = 3x + 7 and the carry to x's
//!   bit 0, and `b`s back to `ej_b`: `(4x + 7 + (x & 1)) ^ 0x3C3C`.
//! - `ej_c(x)`: pushes r4 and lr, sets r4 = x ^ 0x99 and the carry to x's
//!   bit 1, and `bl`s `ej_d`, whose return site is `ej_e`'s entry.
//! - `ej_e`: returns `(r0 + r4 + C) ^ 0x5280`, then pops its caller's
//!   frame as `ej_b` does. From `ej_c`, r0 is `ej_d`'s result and r4 and
//!   the carry `ej_c`'s: `((x + 0x1234) + (x ^ 0x99) + (x >> 1 & 1)) ^
//!   0x5280`.
//! - `ej_ep(x)`: calls `ej_e` through the pointer stub as `ej_bp` does
//!   `ej_b`: `x ^ 0x5280`.
//! - `ej_d(x)`: returns `x + 0x1234` and leaves the flags as they came
//!   (an `ldr`, and the high-register `mov` and `add`, which set none).
//!
//! Each frame `f` (from 0) the program stores `ej_a(f)`, `ej_bp(f)`,
//! `ej_c(f)` and `ej_ep(f)` to `RESULTS`, a global in RAM, and draws them
//! in mode 3 as four pixels, each the result's low 15 bits, at (f % 240,
//! f / 240 % 40) and 40, 80 and 120 rows below. A lift that enters `ej_b`
//! by its plain entry (r4 and the flags zeroed) where `ej_a` jumps into it
//! (stage A's old tail call, `gbaa-lift --fall-as-tail-call`) draws
//! `ej_a`'s wrong from frame 0; one that takes `ej_c`'s call for one that
//! never returns stops there. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::write_volatile;

core::arch::global_asm!(
    ".section .text.native_entry_jump, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".thumb",
    // ej_b: r0, r4 and the carry as they arrive, the caller's frame pushed.
    ".global ej_b",
    ".type ej_b, %function",
    ".thumb_func",
    "ej_b:",
    "    adcs r0, r4",
    "    movs r1, #0x3C",
    "    lsls r1, r1, #8",
    "    adds r1, #0x3C",
    "    eors r0, r1",
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    // u32 ej_bp(u32 x)
    ".global ej_bp",
    ".type ej_bp, %function",
    ".thumb_func",
    "ej_bp:",
    "    push {{r4, lr}}",
    "    movs r4, #0",
    "    adds r1, r4, #0",
    "    ldr r3, =ej_b",
    "    bl ej_via_r3",
    // Never run on the hardware: ej_b returned for us.
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    // u32 ej_a(u32 x)
    ".global ej_a",
    ".type ej_a, %function",
    ".thumb_func",
    "ej_a:",
    "    push {{r4, lr}}",
    "    lsls r4, r0, #1",
    "    adds r4, r4, r0",
    "    adds r4, #7",
    "    lsrs r1, r0, #1",
    // A jump, not a fall, into ej_b's entry.
    "    b ej_b",
    // u32 ej_c(u32 x)
    ".global ej_c",
    ".type ej_c, %function",
    ".thumb_func",
    "ej_c:",
    "    push {{r4, lr}}",
    "    movs r4, #0x99",
    "    eors r4, r0",
    "    lsrs r1, r0, #2",
    // Returns into ej_e's entry.
    "    bl ej_d",
    // ej_e: r0, r4 and the carry as they arrive, the caller's frame pushed.
    ".global ej_e",
    ".type ej_e, %function",
    ".thumb_func",
    "ej_e:",
    "    adcs r0, r4",
    "    movs r1, #0xA5",
    "    lsls r1, r1, #7",
    "    eors r0, r1",
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    // u32 ej_ep(u32 x)
    ".global ej_ep",
    ".type ej_ep, %function",
    ".thumb_func",
    "ej_ep:",
    "    push {{r4, lr}}",
    "    movs r4, #0",
    "    adds r1, r4, #0",
    "    ldr r3, =ej_e",
    "    bl ej_via_r3",
    // Never run on the hardware: ej_e returned for us.
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    // u32 ej_d(u32 x): x + 0x1234, the flags as they came.
    ".global ej_d",
    ".type ej_d, %function",
    ".thumb_func",
    "ej_d:",
    "    ldr r1, =0x1234",
    "    mov r12, r1",
    "    add r0, r12",
    "    bx lr",
    ".ltorg",
    // agbcc's _call_via_r3: a call through a function pointer.
    ".thumb_func",
    "ej_via_r3:",
    "    bx r3",
);

unsafe extern "C" {
    fn ej_a(x: u32) -> u32;
    fn ej_bp(x: u32) -> u32;
    fn ej_c(x: u32) -> u32;
    fn ej_ep(x: u32) -> u32;
}

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

/// The last frame's four results, for the native check to compare.
struct Results(UnsafeCell<[u32; 4]>);

// SAFETY: only the main loop touches it; there are no interrupt handlers.
unsafe impl Sync for Results {}

static RESULTS: Results = Results(UnsafeCell::new([0; 4]));

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let mut f: u32 = 0;
    loop {
        // SAFETY: each takes x in r0 and returns in r0, keeping r4-r11 and
        // sp, as AAPCS does.
        let r = unsafe { [ej_a(f), ej_bp(f), ej_c(f), ej_ep(f)] };
        let (x, y) = ((f % 240) as usize, (f / 240 % 40) as usize);
        // SAFETY: RESULTS is ours (see its Sync); the pixels are in mode
        // 3's frame buffer, which takes halfword stores.
        unsafe {
            write_volatile(RESULTS.0.get(), r);
            for (i, v) in r.iter().enumerate() {
                write_volatile(VRAM.add((y + 40 * i) * 240 + x), *v as u16 & 0x7FFF);
            }
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
