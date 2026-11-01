//! gbaa test ROM for the stage-A lifter (native/lift-irq-hold;
//! docs/native-stage-b.md, piece 1): an IRQ that comes due inside a
//! critical section (the CPSR's I bit set) is held until the section
//! ends, as the hardware holds it.
//!
//! A V-blank handler counts frames in `COUNT`. Each frame the main loop
//! calls `ih_probe(&COUNT)`, Thumb in `global_asm!`:
//!
//! 1. polls `VCOUNT` until line 100, with IRQs enabled;
//! 2. `ih_disable` (ARM: `mrs r0, cpsr; orr r1, r0, #0x80; msr cpsr_c,
//!    r1`, as portable_atomic's `interrupt::disable`) sets the I bit and
//!    returns the old CPSR; reads the count, `c0`;
//! 3. polls `VCOUNT` until line 170, past V-blank's start at line 160,
//!    whose IRQ comes due there; reads the count, `c1`;
//! 4. `ih_restore` (ARM: `msr cpsr_c, r0`, as `interrupt::restore`) puts
//!    the old CPSR back, which clears I; reads the count, `c2`.
//!
//! On the hardware the V-blank IRQ waits in `IF` while I is set and is
//! taken as soon as the `msr` clears it, before `ih_restore` returns: `c1
//! - c0` is 0 and `c2 - c0` is 1. `ih_probe` returns `(c1 - c0) | (c2 -
//! c0) << 8`.
//!
//! The ARM functions are called through `bx r3` (`ih_via_r3`, agbcc's
//! `_call_via_r3`), as Thumb can't `bl` to ARM code on the ARM7TDMI.
//!
//! The main loop stores the frame's two differences and the count to
//! `RESULTS`, a global in RAM, and draws them in mode 3 as one pixel at
//! (f % 240, f / 240 % 80) for frame `f` (from 0): `0x7C00 | (c2 - c0) <<
//! 5 | (c1 - c0)`. A build that runs the handler at the wait inside the
//! section (stage A's old `Game::cpsr`, bookkeeping only) gets `c1 - c0`
//! 1; one that holds it past the `msr` to the next wait gets `c2 - c0` 0.
//! What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

core::arch::global_asm!(
    ".section .text.native_irq_hold, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".thumb",
    // u32 ih_probe(u32 *count)
    ".global ih_probe",
    ".type ih_probe, %function",
    ".thumb_func",
    "ih_probe:",
    "    push {{r4-r7, lr}}",
    "    movs r4, r0",
    "    ldr r5, =0x04000006",
    // Line 100, IRQs on.
    "1:  ldrh r0, [r5]",
    "    cmp r0, #100",
    "    bne 1b",
    "    ldr r3, =ih_disable",
    "    bl ih_via_r3",
    "    movs r7, r0",
    "    ldr r6, [r4]",
    // Line 170, IRQs held: V-blank comes due at 160.
    "2:  ldrh r0, [r5]",
    "    cmp r0, #170",
    "    bne 2b",
    "    ldr r1, [r4]",
    "    subs r1, r1, r6",
    "    push {{r1}}",
    "    movs r0, r7",
    "    ldr r3, =ih_restore",
    "    bl ih_via_r3",
    "    ldr r2, [r4]",
    "    subs r2, r2, r6",
    "    lsls r2, r2, #8",
    "    pop {{r0}}",
    "    orrs r0, r2",
    "    pop {{r4-r7}}",
    "    pop {{r1}}",
    "    bx r1",
    ".ltorg",
    ".thumb_func",
    "ih_via_r3:",
    "    bx r3",
    ".balign 4",
    ".arm",
    // u32 ih_disable(void): the old CPSR, I now set.
    ".global ih_disable",
    ".type ih_disable, %function",
    "ih_disable:",
    "    mrs r0, cpsr",
    "    orr r1, r0, #0x80",
    "    msr cpsr_c, r1",
    "    bx lr",
    // void ih_restore(u32 cpsr)
    ".global ih_restore",
    ".type ih_restore, %function",
    "ih_restore:",
    "    msr cpsr_c, r0",
    "    bx lr",
    ".thumb",
);

unsafe extern "C" {
    fn ih_probe(count: *mut u32) -> u32;
}

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

/// Frames counted by the V-blank handler. (ARMv4T has no atomics.)
struct Counter(UnsafeCell<u32>);

// SAFETY: only the V-blank handler writes it; the main loop and
// `ih_probe` only read it.
unsafe impl Sync for Counter {}

static COUNT: Counter = Counter(UnsafeCell::new(0));

/// The last frame's `c1 - c0`, `c2 - c0` and count, for the native check
/// to compare.
struct Results(UnsafeCell<[u32; 3]>);

// SAFETY: only the main loop touches it.
unsafe impl Sync for Results {}

static RESULTS: Results = Results(UnsafeCell::new([0; 3]));

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    // SAFETY: the handler is kept for the whole program; it only steps
    // COUNT (see its Sync).
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
    loop {
        // SAFETY: ih_probe takes the counter's address in r0 and returns
        // in r0, keeping r4-r11 and sp, as AAPCS does.
        let r = unsafe { ih_probe(COUNT.0.get()) };
        let (held, after) = (r & 0xFF, r >> 8 & 0xFF);
        let (x, y) = ((f % 240) as usize, (f / 240 % 80) as usize);
        // SAFETY: RESULTS is ours (see its Sync), COUNT only read; the
        // pixel is in mode 3's frame buffer, which takes halfword stores.
        unsafe {
            let count = read_volatile(COUNT.0.get());
            write_volatile(RESULTS.0.get(), [held, after, count]);
            write_volatile(
                VRAM.add(y * 240 + x),
                0x7C00 | (after as u16 & 0x1F) << 5 | held as u16 & 0x1F,
            );
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
