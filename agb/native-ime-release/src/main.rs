//! gbaa test ROM for the stage-A lifter and the play build at `--timing
//! native` (native/ime-release; docs/native-stage-b.md, piece 1): an IRQ
//! that comes due while `IME` is clear is taken the moment `IME` is
//! written back to 1, as the hardware takes it. agbcc games' critical
//! sections mask this way, by `IME` rather than the CPSR's I bit (agb
//! 0.25's own use the I bit: `agb/native-irq-hold`).
//!
//! A V-blank handler counts frames in `COUNT`. Each frame the main loop
//! calls `ir_probe(&COUNT)`, Thumb in `global_asm!`:
//!
//! 1. polls `VCOUNT` until line 100, with IRQs enabled;
//! 2. `ir_ime(0)` clears `IME`; reads the count, `c0`;
//! 3. polls `VCOUNT` until line 170, past V-blank's start at line 160,
//!    whose IRQ comes due there; reads the count, `c1`;
//! 4. `ir_ime(1)` sets `IME` again, the store its last act before `bx
//!    lr`; two `nop`s later (mGBA takes an IRQ a few cycles after the
//!    write that lets it through), reads the count, `c2`.
//!
//! On the hardware the V-blank IRQ waits in `IF` while `IME` is clear and
//! is taken as soon as the store sets it: `c1 - c0` is 0 and `c2 - c0` is
//! 1. `ir_probe` returns `(c1 - c0) | (c2 - c0) << 8`.
//!
//! The main loop stores the frame's two differences and the count to
//! `RESULTS`, a global in RAM, and draws them in mode 3 as one pixel at
//! (f % 240, f / 240 % 80) for frame `f` (from 0): `0x7C00 | (c2 - c0) <<
//! 5 | (c1 - c0)`. A build that holds the IRQ past the store to the next
//! wait gets `c2 - c0` 0. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

core::arch::global_asm!(
    ".section .text.native_ime_release, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".thumb",
    // u32 ir_probe(u32 *count)
    ".global ir_probe",
    ".type ir_probe, %function",
    ".thumb_func",
    "ir_probe:",
    "    push {{r4-r7, lr}}",
    "    movs r4, r0",
    "    ldr r5, =0x04000006",
    // Line 100, IRQs on.
    "1:  ldrh r0, [r5]",
    "    cmp r0, #100",
    "    bne 1b",
    "    movs r0, #0",
    "    bl ir_ime",
    "    ldr r6, [r4]",
    // Line 170, IME clear: V-blank comes due at 160.
    "2:  ldrh r0, [r5]",
    "    cmp r0, #170",
    "    bne 2b",
    "    ldr r7, [r4]",
    "    subs r7, r7, r6",
    "    movs r0, #1",
    "    bl ir_ime",
    "    nop",
    "    nop",
    "    ldr r0, [r4]",
    "    subs r0, r0, r6",
    "    lsls r0, r0, #8",
    "    orrs r0, r7",
    "    pop {{r4-r7}}",
    "    pop {{r1}}",
    "    bx r1",
    ".ltorg",
    // void ir_ime(u32 v): IME = v, the store the last act before the
    // return.
    ".global ir_ime",
    ".type ir_ime, %function",
    ".thumb_func",
    "ir_ime:",
    "    ldr r1, =0x04000208",
    "    strh r0, [r1]",
    "    bx lr",
    ".ltorg",
);

unsafe extern "C" {
    fn ir_probe(count: *mut u32) -> u32;
}

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

/// Frames counted by the V-blank handler. (ARMv4T has no atomics.)
struct Counter(UnsafeCell<u32>);

// SAFETY: only the V-blank handler writes it; the main loop and
// `ir_probe` only read it.
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
        // SAFETY: ir_probe takes the counter's address in r0 and returns
        // in r0, keeping r4-r11 and sp, as AAPCS does; it leaves IME on,
        // as it found it.
        let r = unsafe { ir_probe(COUNT.0.get()) };
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
