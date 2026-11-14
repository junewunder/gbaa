//! gbaa test ROM for the stage-A lifter and the hardware library's DMA by
//! its registers (native/hw-dma-registers; docs/native-stage-b.md, "The
//! first game"): each of DMA's four starts set up by address, as agbcc
//! games (m4a's `SoundInit`, their copy and fill macros) set them up.
//!
//! Every channel is set up through `dma_set(regs, src, dst, cnt)`, Thumb
//! in `global_asm!`: three word stores, source, destination, then count
//! and control as one word (`DMAxCNT`). The lift check plants a wrong
//! value in that last store (one more unit, the "wrong control word").
//!
//! - **Immediate**, DMA3: once, 64 words of `TILES` from ROM to VRAM,
//!   with its IRQ at the end; each frame, a 16-bit fill of `FILL` (32
//!   halfwords in RAM) from the frame's number, a fixed source, IRQ too.
//!   The DMA3 handler counts in `COUNTS[1]`.
//! - **H-blank**, DMA0: each frame after V-blank, from `LINES` (one
//!   halfword a line, filled from the frame's number) to the backdrop
//!   colour, a fixed destination, repeating: the picture is a gradient
//!   that scrolls by a line a frame.
//! - **V-blank**, DMA2: set once, 8 words of `SHADOW` (filled each frame)
//!   to palette entries 16-31, repeating with the destination reloaded.
//! - **Special**, DMA1: FIFO A fed from `SAMPLES` in ROM, timer 0 at 16384
//!   Hz, words, repeat and the IRQ at each refill; restarted at the
//!   samples' start each frame, as m4a restarts it. The DMA1 handler
//!   counts refills in `COUNTS[0]`.
//!
//! What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::{addr_of, read_volatile, write_volatile};

core::arch::global_asm!(
    ".section .text.native_dma, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".thumb",
    // void dma_set(u32 *regs, u32 src, u32 dst, u32 cnt)
    ".global dma_set",
    ".type dma_set, %function",
    ".thumb_func",
    "dma_set:",
    "    str r1, [r0]",
    "    str r2, [r0, #4]",
    "    str r3, [r0, #8]",
    "    bx lr",
);

unsafe extern "C" {
    fn dma_set(regs: *mut u32, src: u32, dst: u32, cnt: u32);
}

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const SOUNDCNT_H: *mut u16 = 0x0400_0082 as *mut u16;
const SOUNDCNT_X: *mut u16 = 0x0400_0084 as *mut u16;
const FIFO_A: u32 = 0x0400_00A0;
const TM0CNT: *mut u32 = 0x0400_0100 as *mut u32;
const VRAM: u32 = 0x0600_0000;
const PALETTE: u32 = 0x0500_0000;

/// Channel `n`'s first register, `DMAxSAD`.
fn dma(n: u32) -> *mut u32 {
    (0x0400_00B0 + 12 * n) as *mut u32
}

/// Channel `n`'s control alone, `DMAxCNT_H`.
fn dma_cnt_h(n: u32) -> *mut u16 {
    (0x0400_00BA + 12 * n) as *mut u16
}

/// `DMAxCNT_H` bits.
const ENABLE: u32 = 1 << 15;
const IRQ: u32 = 1 << 14;
const VBLANK: u32 = 1 << 12;
const HBLANK: u32 = 2 << 12;
const SPECIAL: u32 = 3 << 12;
const WORD: u32 = 1 << 10;
const REPEAT: u32 = 1 << 9;
const SRC_FIXED: u32 = 2 << 7;
const DST_FIXED: u32 = 2 << 5;
const DST_RELOAD: u32 = 3 << 5;

/// `DMAxCNT`: count and control as one word.
fn cnt(count: u32, control: u32) -> u32 {
    count | control << 16
}

static TILES: [u32; 64] = {
    let mut t = [0; 64];
    let mut i = 0;
    while i < 64 {
        t[i] = 0x0101_0101 * (i as u32 % 16) ^ (i as u32) << 20;
        i += 1;
    }
    t
};

static SAMPLES: [u8; 1024] = {
    let mut s = [0; 1024];
    let mut i = 0;
    while i < 1024 {
        // A saw a period of 64 samples, louder each 256.
        s[i] = ((i % 64) as u8).wrapping_mul(2 + (i / 256) as u8);
        i += 1;
    }
    s
};

/// RAM the channels read and write, and the handlers' counts. (ARMv4T has
/// no atomics.)
struct Shared<T>(UnsafeCell<T>);

// SAFETY: the main loop writes LINES, SHADOW and FILL_VALUE between the
// DMAs that read them; each handler writes only its own count, which the
// main loop only reads.
unsafe impl<T> Sync for Shared<T> {}

static LINES: Shared<[u16; 228]> = Shared(UnsafeCell::new([0; 228]));
static SHADOW: Shared<[u32; 8]> = Shared(UnsafeCell::new([0; 8]));
static FILL: Shared<[u16; 32]> = Shared(UnsafeCell::new([0; 32]));
static FILL_VALUE: Shared<u16> = Shared(UnsafeCell::new(0));
/// DMA1's refills, DMA3's transfers.
static COUNTS: Shared<[u32; 2]> = Shared(UnsafeCell::new([0; 2]));

fn count(i: usize) {
    let c = COUNTS.0.get().cast::<u32>();
    // SAFETY: each handler steps only its own element (see Shared).
    unsafe { write_volatile(c.add(i), read_volatile(c.add(i)) + 1) };
}

/// Set up channel `n`.
fn set(n: u32, src: u32, dst: u32, count: u32, control: u32) {
    // SAFETY: dma_set stores the channel's three words, as AAPCS keeps
    // everything else.
    unsafe { dma_set(dma(n), src, dst, cnt(count, control)) };
}

/// FIFO A from the samples' start, as m4a's `SoundInit` sets DMA1 up.
fn restart_fifo() {
    // SAFETY: DMA1's control register; off, then set up again.
    unsafe { write_volatile(dma_cnt_h(1), 0) };
    let c = ENABLE | IRQ | SPECIAL | WORD | REPEAT;
    set(1, addr_of!(SAMPLES) as u32, FIFO_A, 4, c);
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    use agb::interrupt::{Interrupt, add_interrupt_handler};
    // SAFETY: the handlers are kept for the whole program; each only steps
    // its count (see Shared).
    let _refills = unsafe { add_interrupt_handler(Interrupt::Dma1, |_| count(0)) };
    let _copies = unsafe { add_interrupt_handler(Interrupt::Dma3, |_| count(1)) };
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: the display, sound and timer registers, as their bits say:
    // mode 0 with nothing on (the backdrop), FIFO A at 100% to both
    // speakers on timer 0 (reset), timer 0 at 1024 cycles a sample.
    unsafe {
        write_volatile(DISPCNT, 0);
        write_volatile(SOUNDCNT_X, 0x80);
        write_volatile(SOUNDCNT_H, 0x0B04);
        write_volatile(TM0CNT, 0x0080_FC00);
    }
    set(3, addr_of!(TILES) as u32, VRAM, 64, ENABLE | IRQ | WORD);
    let vb = ENABLE | VBLANK | WORD | REPEAT | DST_RELOAD;
    set(2, SHADOW.0.get() as u32, PALETTE + 0x20, 8, vb);
    restart_fifo();
    let mut f: u32 = 0;
    loop {
        // SAFETY: LINES, SHADOW and FILL_VALUE are the main loop's (see
        // Shared); the DMAs that read them run at H-blank and V-blank.
        unsafe {
            let lines = LINES.0.get().cast::<u16>();
            for y in 0..228u32 {
                let v = (y + f) % 32;
                write_volatile(
                    lines.add(y as usize),
                    (v | (31 - v) << 10 | (f % 32) << 5) as u16,
                );
            }
            let shadow = SHADOW.0.get().cast::<u32>();
            for i in 0..8u32 {
                write_volatile(shadow.add(i as usize), f.wrapping_mul(0x0001_0003) ^ i << 4);
            }
            write_volatile(FILL_VALUE.0.get(), f as u16 ^ 0x5A5A);
        }
        let fixed = ENABLE | IRQ | SRC_FIXED;
        set(3, FILL_VALUE.0.get() as u32, FILL.0.get() as u32, 32, fixed);
        vblank.wait_for_vblank();
        // SAFETY: DMA0's control register; off, then set up again from
        // the first line.
        unsafe { write_volatile(dma_cnt_h(0), 0) };
        set(
            0,
            LINES.0.get() as u32,
            PALETTE,
            1,
            ENABLE | HBLANK | REPEAT | DST_FIXED,
        );
        restart_fifo();
        f += 1;
    }
}
