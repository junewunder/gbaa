//! gbaa test ROM for the stage-A lifter (native/lift-bios-region;
//! docs/native-stage-b.md, "The first game"): loads from the BIOS region
//! read its protection latch, the last opcode fetched from the BIOS, and
//! stores there go nowhere. m4a's `SoundInit` starts a DMA from address 0,
//! which reads it; the play build moves the latch as its BIOS runs
//! (`runtime::bus::BIOS_LATCH_*`): after boot, after a `swi` returns,
//! inside an IRQ handler (the BIOS's IRQ stub called it) and after the IRQ
//! returns.
//!
//! `OUT`, a global in RAM, holds what it reads:
//!
//! 0. the word at `0x4` at `main`'s start (after agb's start-up);
//! 1. after `swi 6` (`Div`): the word at 0,
//! 2. the halfword at 2,
//! 3. the byte at 1 (constant addresses, 0 in a register),
//! 4. the word at `0x3FFC`, the BIOS's last, at an address the compiler
//!    can't see (`black_box`);
//! 5. in the V-blank handler: the word at `0x8`;
//! 6. after polling `VCOUNT` from line 100 to 170, across V-blank's IRQ:
//!    the word at `0x10`,
//! 7. the byte at `0x13`, computed;
//! 8. `Div`'s quotient,
//! 9. the frame,
//! 10. after `swi 2` (`Halt`), woken by V-blank's IRQ: the word at 0 (the
//!     latch the IRQ left: the play build moves it as the call is made,
//!     before halting).
//!
//! Each frame it also stores to 0 and to `0x100` (nowhere). It draws `OUT`
//! in mode 3: frame `f` (from 0) row `f % 160`, one pixel per slot, the
//! slot's halves xored, low 15 bits. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::hint::black_box;
use core::ptr::{read_volatile, write_volatile};

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VCOUNT: *const u16 = 0x0400_0006 as *const u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;
const SLOTS: usize = 11;

/// What it read (see the header). (ARMv4T has no atomics.)
struct Out(UnsafeCell<[u32; SLOTS]>);

// SAFETY: the main loop writes every slot but 5, the V-blank handler only
// slot 5; each is a single word store.
unsafe impl Sync for Out {}

static OUT: Out = Out(UnsafeCell::new([0; SLOTS]));

/// Store `v` to `OUT[k]`.
fn put(k: usize, v: u32) {
    // SAFETY: in bounds (k < SLOTS at every call); see Out's Sync.
    unsafe { write_volatile(OUT.0.get().cast::<u32>().add(k), v) }
}

/// The word at BIOS address `a` (not 0: Rust's pointers can't be null).
fn bios32(a: u32) -> u32 {
    // SAFETY: the BIOS region reads without side effects.
    unsafe { read_volatile(a as *const u32) }
}

fn bios8(a: u32) -> u32 {
    // SAFETY: as bios32.
    unsafe { read_volatile(a as *const u8) as u32 }
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    put(0, bios32(0x4));
    // SAFETY: the handler is kept for the whole program; it only writes
    // slot 5 (see Out's Sync).
    let _h = unsafe {
        agb::interrupt::add_interrupt_handler(agb::interrupt::Interrupt::VBlank, |_| {
            put(5, bios32(black_box(0x8)))
        })
    };
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let mut f: u32 = 0;
    loop {
        // Across V-blank's IRQ by polling, with no BIOS call.
        // SAFETY: VCOUNT reads without side effects.
        while unsafe { read_volatile(VCOUNT) } != 100 {}
        while unsafe { read_volatile(VCOUNT) } != 170 {}
        put(6, bios32(0x10));
        put(7, bios8(black_box(0x13)));

        // After a BIOS call: Div of (f + 1000) by 7.
        let (mut q, mut d) = (f + 1000, 7u32);
        let (w, h, b): (u32, u32, u32);
        // SAFETY: Div reads r0 and r1 and changes r0, r1, r3 (and r12 in
        // the BIOS). The loads read the BIOS region at 0, the store goes
        // nowhere.
        unsafe {
            asm!("swi 6", inout("r0") q, inout("r1") d, out("r3") _, out("r12") _);
            asm!(
                "movs {t}, #0",
                "ldr {w}, [{t}]",
                "ldrh {h}, [{t}, #2]",
                "ldrb {b}, [{t}, #1]",
                "str {w}, [{t}]",
                t = out(reg) _,
                w = out(reg) w,
                h = out(reg) h,
                b = out(reg) b,
            );
        }
        let _ = d;
        put(1, w);
        put(2, h);
        put(3, b);
        put(4, bios32(black_box(0x3FFC)));
        put(8, q);
        put(9, f);
        // SAFETY: a store to the BIOS region goes nowhere.
        unsafe { write_volatile(black_box(0x100) as *mut u32, f) };

        let y = (f % 160) as usize;
        for k in 0..SLOTS {
            // SAFETY: OUT's slot k (see its Sync); the pixel is in mode
            // 3's frame buffer, which takes halfword stores.
            unsafe {
                let v = read_volatile(OUT.0.get().cast::<u32>().add(k));
                write_volatile(VRAM.add(y * 240 + k), ((v ^ v >> 15) & 0x7FFF) as u16);
            }
        }
        // Halt until V-blank's IRQ (enabled with its handler), then read
        // the latch it left. agb's `wait_for_vblank` then skips its wait.
        let a: u32;
        // SAFETY: Halt changes r2 and r12 in the BIOS (r0-r3 declared
        // clobbered); the load reads the BIOS region at 0.
        unsafe {
            asm!(
                "swi 2",
                "movs {t}, #0",
                "ldr {a}, [{t}]",
                t = out(reg) _,
                a = out(reg) a,
                out("r0") _, out("r1") _, out("r2") _, out("r3") _, out("r12") _,
            );
        }
        put(10, a);
        vblank.wait_for_vblank();
        f += 1;
    }
}
