//! gbaa test ROM for the per-function check's second reference
//! (native/fn-check-interp): ARM functions that return without
//! interworking, as ARMv4T code called only from ARM may.
//!
//! Four functions in `global_asm!`:
//!
//! - `ar_call_t(x)` (Thumb) jumps to `ar_call` with `bx`, lr untouched.
//! - `ar_call(x)` (ARM) pushes r4 and lr, calls `ar_mov(x)` and
//!   `ar_ldm(x, ar_mov(x))` with `bl`, and returns `ar_ldm`'s result with
//!   `bx lr` to its Thumb caller.
//! - `ar_mov(x)` (ARM) stores `3x + 1` to `LOG[0]` and returns it with
//!   `mov pc, lr`: the return lands in ARM state at lr's word.
//! - `ar_ldm(x, y)` (ARM) pushes r4, r5 and lr, stores `(x ^ y) + 7` to
//!   `LOG[1]` and returns it with `pop {r4, r5, pc}` (an `ldmfd`, which on
//!   ARMv4T never switches to Thumb).
//!
//! `just fn-check` on `ar_mov` and `ar_ldm` must count their interpreter
//! runs as checked: a check that only sees a return to its Thumb sentinel
//! never sees these. Each frame `f` (from 0) the program draws `ar_call(f)`
//! in mode 3 as a pixel at (f % 240, f / 240 % 80), its low 15 bits, and
//! `LOG[0]` (`ar_mov`'s `3f + 1`) 80 rows below.
//! What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

/// What `ar_mov` and `ar_ldm` last stored.
struct Log(UnsafeCell<[u32; 2]>);

// SAFETY: only the main loop's calls touch it; there are no interrupt
// handlers.
unsafe impl Sync for Log {}

static LOG: Log = Log(UnsafeCell::new([0; 2]));

core::arch::global_asm!(
    ".section .text.native_arm_return, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".thumb",
    ".global ar_call_t",
    ".type ar_call_t, %function",
    ".thumb_func",
    "ar_call_t:",
    "    ldr r1, =ar_call",
    "    bx r1",
    ".ltorg",
    ".balign 4",
    ".arm",
    ".type ar_call, %function",
    "ar_call:",
    "    push {{r4, lr}}",
    "    mov r4, r0",
    "    bl ar_mov",
    "    mov r1, r0",
    "    mov r0, r4",
    "    bl ar_ldm",
    "    pop {{r4, lr}}",
    "    bx lr",
    ".type ar_mov, %function",
    "ar_mov:",
    "    add r1, r0, r0, lsl #1",
    "    add r0, r1, #1",
    "    ldr r2, ={log}",
    "    str r0, [r2]",
    "    mov pc, lr",
    ".type ar_ldm, %function",
    "ar_ldm:",
    "    push {{r4, r5, lr}}",
    "    eor r4, r0, r1",
    "    add r0, r4, #7",
    "    ldr r5, ={log}",
    "    str r0, [r5, #4]",
    "    pop {{r4, r5, pc}}",
    ".ltorg",
    log = sym LOG,
);

unsafe extern "C" {
    fn ar_call_t(x: u32) -> u32;
}

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let mut f: u32 = 0;
    loop {
        // SAFETY: ar_call_t takes x in r0 and returns in r0, keeping r4-r11
        // and sp, as AAPCS does; it writes only LOG.
        let v = unsafe { ar_call_t(f) };
        // SAFETY: LOG is ours (see its Sync).
        let m = unsafe { read_volatile(LOG.0.get()) }[0];
        let (x, y) = ((f % 240) as usize, (f / 240 % 80) as usize);
        // SAFETY: the pixels are in mode 3's frame buffer, which takes
        // halfword stores.
        unsafe {
            write_volatile(VRAM.add(y * 240 + x), v as u16 & 0x7FFF);
            write_volatile(VRAM.add((y + 80) * 240 + x), m as u16 & 0x7FFF);
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
