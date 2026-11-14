//! gbaa test ROM for the stage-A lifter (native/lift-irq-wrapper-tail;
//! `docs/native-port-design.md`, "The stage-A lifter as built"): an IRQ
//! wrapper whose `msr` and `mrs` the lifter prints, run every frame. No
//! other program of the lifter's check runs a printed PSR statement: they
//! keep a stand-in for agb's wrapper.
//!
//! Two wrappers run here, both lifted (the holes file has no stand-in for
//! either):
//!
//! 1. Frames 0-31: agb's own `InterruptHandler`, through
//!    `add_interrupt_handler` and `wait_for_vblank` as every agb program
//!    uses them.
//! 2. From frame 32: `iw_main` below, installed at `0x0300_7FFC` in its
//!    place, written for this test. It has what the wrappers agbcc-era
//!    games link have that agb's lacks: the handler is called with `add
//!    lr, pc, #0; bx r0`, and half the PSR statements come after that
//!    call. It saves the SPSR, picks the IRQ (V-count before V-blank),
//!    acknowledges it, leaves only V-blank in `IE` to nest, switches to
//!    system mode with I clear (`mrs`, `bic`, `orr`, `msr`), calls the
//!    handler from a table, then switches back to IRQ mode with I set and
//!    puts `IE` and the SPSR back. Code discovery once took that `bx`
//!    for a jump, so the half after it was in no lift.
//!
//! The main loop alternates the V-count line (`DISPSTAT`'s LYC)
//! between 100 and 160:
//!
//! - at 100 the V-count IRQ comes alone while the loop waits in
//!   `VBlankIntrWait`, then V-blank at line 160: the handlers run in the
//!   order V-count, V-blank;
//! - at 160 both come due together. `iw_main` takes V-count; its `msr`
//!   to system mode clears I with V-blank still pending and enabled, so
//!   the hardware takes V-blank there, before the call: a second
//!   `iw_main` runs inside the first and the handlers run in the order
//!   V-blank, V-count.
//!
//! Each handler shifts its number (V-blank 1, V-count 2) into `seq`, two
//! bits a call, counts itself, and notes its sp and the `IE` it sees
//! (`IW_STATE`). The main loop copies that to `RESULTS` with `IE` as the
//! wrapper left it, and draws one pixel a frame in mode 3 at (f % 240, f
//! / 240 % 80): `0x03E0 | count & 0x1F` under agb's wrapper, `0x7C00 |
//! (seq & 0xF) << 5 | (vblanks & 3) << 2 | vcounts & 3` under `iw_main`.
//! `seq & 0xF` is 9 after a frame with LYC 100 and 6 after one with LYC
//! 160; a build that doesn't take the nested IRQ at the `msr` draws 9
//! for both. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

core::arch::global_asm!(
    ".section .text.native_irq_wrapper, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".arm",
    // The IRQ wrapper, entered from the BIOS's stub in IRQ mode with r0
    // = 0x04000000 and IME on (the stub leaves it as the IRQ found it).
    ".global iw_main",
    ".type iw_main, %function",
    "iw_main:",
    "    mrs r12, spsr",
    "    stmfd sp!, {{r12, lr}}",
    // IE, IF, and what is both enabled and requested.
    "    add r1, r0, #0x200",
    "    ldrh r2, [r1]",
    "    ldrh r3, [r1, #2]",
    "    and r3, r3, r2",
    // V-count before V-blank: its bit, and its handler from the table.
    "    ldr r12, =iw_table",
    "    tst r3, #4",
    "    movne r3, #4",
    "    ldrne r0, [r12]",
    "    moveq r3, #1",
    "    ldreq r0, [r12, #4]",
    // Acknowledge it; of the others, only V-blank may nest.
    "    strh r3, [r1, #2]",
    "    bic r3, r2, r3",
    "    and r3, r3, #1",
    "    strh r3, [r1]",
    "    stmfd sp!, {{r1, r2}}",
    // System mode, I clear.
    "    mrs r2, cpsr",
    "    bic r2, r2, #0xdf",
    "    orr r2, r2, #0x1f",
    "    msr cpsr_fc, r2",
    "    stmfd sp!, {{lr}}",
    "    add lr, pc, #0",
    "    bx r0",
    "    ldmfd sp!, {{lr}}",
    // IRQ mode, I set.
    "    mrs r2, cpsr",
    "    bic r2, r2, #0xdf",
    "    orr r2, r2, #0x92",
    "    msr cpsr_fc, r2",
    // IE as it was, then the SPSR.
    "    ldmfd sp!, {{r1, r2}}",
    "    strh r2, [r1]",
    "    ldmfd sp!, {{r12, lr}}",
    "    msr spsr_fc, r12",
    "    bx lr",
    ".ltorg",
    ".balign 4",
    "iw_table:",
    "    .word iw_vcount",
    "    .word iw_vblank",
    ".thumb",
    // The V-count handler: number 2. The handlers have a compiled
    // function's prologue and return, by which the pointer scan takes a
    // table's entries for functions (nothing else names them).
    ".type iw_vcount, %function",
    ".thumb_func",
    "iw_vcount:",
    "    push {{r4, lr}}",
    "    ldr r1, =IW_STATE",
    "    ldr r2, [r1]",
    "    lsls r2, r2, #2",
    "    adds r2, #2",
    "    str r2, [r1]",
    "    ldr r2, [r1, #8]",
    "    adds r2, #1",
    "    str r2, [r1, #8]",
    "    mov r2, sp",
    "    str r2, [r1, #16]",
    "    ldr r3, =0x04000200",
    "    ldrh r2, [r3]",
    "    str r2, [r1, #24]",
    "    pop {{r4}}",
    "    pop {{r0}}",
    "    bx r0",
    ".ltorg",
    // The V-blank handler: number 1. It sets the BIOS's flag, which ends
    // VBlankIntrWait.
    ".type iw_vblank, %function",
    ".thumb_func",
    "iw_vblank:",
    "    push {{r4, lr}}",
    "    ldr r1, =0x03007ff8",
    "    ldrh r2, [r1]",
    "    movs r3, #1",
    "    orrs r2, r3",
    "    strh r2, [r1]",
    "    ldr r1, =IW_STATE",
    "    ldr r2, [r1]",
    "    lsls r2, r2, #2",
    "    adds r2, #1",
    "    str r2, [r1]",
    "    ldr r2, [r1, #4]",
    "    adds r2, #1",
    "    str r2, [r1, #4]",
    "    mov r2, sp",
    "    str r2, [r1, #12]",
    "    ldr r3, =0x04000200",
    "    ldrh r2, [r3]",
    "    str r2, [r1, #20]",
    "    pop {{r4}}",
    "    pop {{r0}}",
    "    bx r0",
    ".ltorg",
    // void iw_install(void): iw_main is the IRQ handler from here on.
    ".global iw_install",
    ".type iw_install, %function",
    ".thumb_func",
    "iw_install:",
    "    ldr r0, =iw_main",
    "    ldr r1, =0x03007ffc",
    "    str r0, [r1]",
    "    bx lr",
    ".ltorg",
    // void iw_wait(void): VBlankIntrWait.
    ".global iw_wait",
    ".type iw_wait, %function",
    ".thumb_func",
    "iw_wait:",
    "    swi 0x05",
    "    bx lr",
    // void iw_line_200(void): polls VCOUNT until line 200.
    ".global iw_line_200",
    ".type iw_line_200, %function",
    ".thumb_func",
    "iw_line_200:",
    "    ldr r1, =0x04000006",
    "1:  ldrh r0, [r1]",
    "    cmp r0, #200",
    "    bne 1b",
    "    bx lr",
    ".ltorg",
);

unsafe extern "C" {
    fn iw_install();
    fn iw_wait();
    fn iw_line_200();
}

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const DISPSTAT: *mut u16 = 0x0400_0004 as *mut u16;
const IE: *mut u16 = 0x0400_0200 as *mut u16;
const IME: *mut u16 = 0x0400_0208 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

/// The frames agb's own wrapper runs before `iw_main` takes over.
const AGB_FRAMES: u32 = 32;

/// `DISPSTAT` with the V-blank and V-count IRQs on and the V-count line.
const fn dispstat(line: u16) -> u16 {
    0x0028 | line << 8
}

/// Frames counted by agb's V-blank handler. (ARMv4T has no atomics.)
struct Counter(UnsafeCell<u32>);

// SAFETY: only agb's V-blank handler writes it; the main loop only reads
// it.
unsafe impl Sync for Counter {}

static COUNT: Counter = Counter(UnsafeCell::new(0));

/// What `iw_main`'s handlers note, by word: `seq`, the V-blank count, the
/// V-count count, the V-blank handler's sp, the V-count handler's, the
/// `IE` the V-blank handler saw, the `IE` the V-count handler saw.
#[repr(transparent)]
pub struct State(UnsafeCell<[u32; 7]>);

// SAFETY: the handlers write it and the main loop reads it after
// VBlankIntrWait, when both of a frame's handlers have run.
unsafe impl Sync for State {}

#[unsafe(no_mangle)]
pub static IW_STATE: State = State(UnsafeCell::new([0; 7]));

/// The last frame's `IW_STATE`, `IE` as the wrapper left it, and agb's
/// count, for the native check to compare.
struct Results(UnsafeCell<[u32; 9]>);

// SAFETY: only the main loop touches it.
unsafe impl Sync for Results {}

static RESULTS: Results = Results(UnsafeCell::new([0; 9]));

/// Frame `f`'s pixel.
fn pixel(f: u32, colour: u16) {
    let (x, y) = ((f % 240) as usize, (f / 240 % 80) as usize);
    // SAFETY: the pixel is in mode 3's frame buffer, which takes halfword
    // stores.
    unsafe { write_volatile(VRAM.add(y * 240 + x), colour) };
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    // SAFETY: the handler is kept for the whole program (agb's wrapper
    // stops running it once `iw_main` is installed); it only steps COUNT
    // (see its Sync).
    let _counter = unsafe {
        agb::interrupt::add_interrupt_handler(agb::interrupt::Interrupt::VBlank, |_| {
            let c = COUNT.0.get();
            write_volatile(c, read_volatile(c) + 1)
        })
    };
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let mut f: u32 = 0;
    let mut results = [0u32; 9];
    while f < AGB_FRAMES {
        vblank.wait_for_vblank();
        // SAFETY: COUNT is only read here; RESULTS is ours (see its Sync).
        let count = unsafe { read_volatile(COUNT.0.get()) };
        results[8] = count;
        unsafe { write_volatile(RESULTS.0.get(), results) };
        pixel(f, 0x03E0 | count as u16 & 0x1F);
        f += 1;
    }
    // SAFETY: iw_line_200 only reads VCOUNT and iw_install only stores
    // the handler's address; both keep r4-r11 and sp, as AAPCS does. The
    // registers are the interrupt controller's and the display's: with
    // IME off no IRQ is taken while they change, and at line 200 neither
    // IRQ is due (the V-count line is set while VCOUNT is elsewhere).
    unsafe {
        iw_line_200();
        write_volatile(IME, 0);
        iw_install();
        write_volatile(DISPSTAT, dispstat(100));
        write_volatile(IE, 0x0005);
        write_volatile(IME, 1);
    }
    loop {
        // SAFETY: as above. IW_STATE is only read (see its Sync) and
        // RESULTS is ours. DISPSTAT's new V-count line is written at line
        // 200, which is neither line.
        unsafe {
            iw_wait();
            let s = read_volatile(IW_STATE.0.get());
            results[..7].copy_from_slice(&s);
            results[7] = read_volatile(IE) as u32;
            write_volatile(RESULTS.0.get(), results);
            pixel(
                f,
                0x7C00 | (s[0] as u16 & 0xF) << 5 | (s[1] as u16 & 3) << 2 | s[2] as u16 & 3,
            );
            iw_line_200();
            write_volatile(DISPSTAT, dispstat(if f & 1 == 0 { 160 } else { 100 }));
        }
        f += 1;
    }
}
