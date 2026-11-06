//! gbaa test ROM for the native lifter's stack locals (native/frame-locals;
//! docs/native-stage-b.md, piece 4): stack slots whose address never
//! escapes, which the lift prints as Rust locals, with values that reach a
//! global; and one slot whose address does, which stays in the frame's
//! bytes.
//!
//! Four Thumb functions in `global_asm!`:
//!
//! - `fl_keep(x)` pushes r4 and lr, makes an 8-byte frame, and keeps three
//!   values in it across a call that clobbers r0-r3: `x + 7` as a word at
//!   sp+4 (`str r1, [sp, #4]`, the store the check's planted control makes
//!   wrong), `x`'s low half at sp+2 and `0x5A` as a byte at sp+0 (through
//!   a copy of sp: Thumb has no sp-relative `strh`/`strb`). It returns
//!   `((x + 7) ^ ((x & 0xFFFF) << 8)) + 0x5A`. None of the three
//!   addresses leaves the function: all three are locals.
//! - `fl_clobber()` zeroes r0-r3.
//! - `fl_esc(x)` stores `x` at sp+0 and passes that address to
//!   `fl_double`, then returns what the slot holds: `2x + 1`. The slot's
//!   address escapes, so it stays in the frame's bytes.
//! - `fl_double(p)` makes `*p` `2 * *p + 1`.
//!
//! Each frame `f` (from 0) the program stores `fl_keep(f)` and `fl_esc(f)`
//! to `RESULTS`, a global in RAM, and draws them in mode 3 as two pixels:
//! `fl_keep`'s at (f % 240, f / 240 % 80), `fl_esc`'s 80 rows below, each
//! the result's low 15 bits. A lift that gets a local wrong (the planted
//! `x + 8` for `x + 7`) gets `fl_keep` wrong from frame 0; one that took
//! `fl_esc`'s slot for a local would miss `fl_double`'s store and draw
//! `x`. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::write_volatile;

core::arch::global_asm!(
    ".section .text.native_frame_locals, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".thumb",
    // u32 fl_keep(u32 x)
    ".global fl_keep",
    ".type fl_keep, %function",
    ".thumb_func",
    "fl_keep:",
    "    push {{r4, lr}}",
    "    sub sp, #8",
    "    adds r1, r0, #7",
    "    str r1, [sp, #4]",
    "    mov r3, sp",
    "    strh r0, [r3, #2]",
    "    movs r2, #0x5A",
    "    strb r2, [r3, #0]",
    // No copy of sp in a register at the call: fl_clobber reads none,
    // but a callee given one could reach the frame.
    "    movs r3, #1",
    "    bl fl_clobber",
    "    ldr r0, [sp, #4]",
    "    mov r3, sp",
    "    ldrh r1, [r3, #2]",
    "    ldrb r2, [r3, #0]",
    "    lsls r1, r1, #8",
    "    eors r0, r1",
    "    adds r0, r0, r2",
    // Nor at the return: the lift takes r0-r3 for results.
    "    movs r3, #0",
    "    add sp, #8",
    "    pop {{r4}}",
    "    pop {{r1}}",
    "    bx r1",
    // void fl_clobber(void)
    ".type fl_clobber, %function",
    ".thumb_func",
    "fl_clobber:",
    "    movs r0, #0",
    "    movs r1, #0",
    "    movs r2, #0",
    "    movs r3, #0",
    "    bx lr",
    // u32 fl_esc(u32 x)
    ".global fl_esc",
    ".type fl_esc, %function",
    ".thumb_func",
    "fl_esc:",
    "    push {{lr}}",
    "    sub sp, #4",
    "    str r0, [sp, #0]",
    "    mov r0, sp",
    "    bl fl_double",
    "    ldr r0, [sp, #0]",
    "    add sp, #4",
    "    pop {{r1}}",
    "    bx r1",
    // void fl_double(u32 *p)
    ".type fl_double, %function",
    ".thumb_func",
    "fl_double:",
    "    ldr r1, [r0]",
    "    lsls r1, r1, #1",
    "    adds r1, #1",
    "    str r1, [r0]",
    "    bx lr",
);

unsafe extern "C" {
    fn fl_keep(x: u32) -> u32;
    fn fl_esc(x: u32) -> u32;
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
        // SAFETY: fl_keep and fl_esc take x in r0 and return in r0,
        // keeping r4-r11 and sp, as AAPCS does.
        let (k, e) = unsafe { (fl_keep(f), fl_esc(f)) };
        let (x, y) = ((f % 240) as usize, (f / 240 % 80) as usize);
        // SAFETY: RESULTS is ours (see its Sync); the pixels are in mode
        // 3's frame buffer, which takes halfword stores.
        unsafe {
            write_volatile(RESULTS.0.get(), [k, e]);
            write_volatile(VRAM.add(y * 240 + x), k as u16 & 0x7FFF);
            write_volatile(VRAM.add((y + 80) * 240 + x), e as u16 & 0x7FFF);
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
