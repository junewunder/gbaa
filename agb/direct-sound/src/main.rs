//! gbaa test ROM for direct sound as a software mixer drives it (the
//! `native/hw-sound` unit; what it plays is in `SOUND.md`): FIFO A and B
//! fed by DMA1 and DMA2 from double buffers in IWRAM, clocked by timer 0,
//! and timer 1 counting up on timer 0's overflows to raise an IRQ every
//! 256 samples, whose handler re-latches both DMAs on the other buffer
//! and fills the one just played (agb's mixer's shape).
//!
//! The buffers and the synth's state are at fixed IWRAM addresses, not in
//! statics, so a native port keeps them at the same place and the native
//! check compares them.
#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const SOUNDCNT_L: usize = IO + 0x80;
const SOUNDCNT_H: usize = IO + 0x82;
const SOUNDCNT_X: usize = IO + 0x84;
const FIFO_A: usize = IO + 0xA0;
const FIFO_B: usize = IO + 0xA4;
const DMA1SAD: usize = IO + 0xBC;
const DMA2SAD: usize = IO + 0xC8;
const TM0CNT: usize = IO + 0x100;
const TM1CNT: usize = IO + 0x104;

/// Samples a buffer holds: timer 1's IRQ comes after each.
const LEN: usize = 256;
/// The buffers: FIFO A's two halves, then FIFO B's.
const BUF: usize = 0x0300_4000;
/// The synth's state, three words: swaps so far, A's phase, B's phase.
const STATE: usize = 0x0300_4400;

/// DMA to a FIFO: enabled, special start, 32-bit, repeat, destination
/// fixed.
const FIFO_DMA: u16 = 0x8000 | 3 << 12 | 1 << 10 | 1 << 9 | 2 << 5;

fn store16(addr: usize, v: u16) {
    // SAFETY: callers pass I/O registers.
    unsafe { write_volatile(addr as *mut u16, v) }
}

fn store32(addr: usize, v: u32) {
    // SAFETY: callers pass I/O registers.
    unsafe { write_volatile(addr as *mut u32, v) }
}

/// Word `i` of the synth's state.
fn state(i: usize) -> u32 {
    // SAFETY: STATE's three words are IWRAM nothing else uses.
    unsafe { read_volatile((STATE + 4 * i) as *const u32) }
}

fn set_state(i: usize, v: u32) {
    // SAFETY: as `state`.
    unsafe { write_volatile((STATE + 4 * i) as *mut u32, v) }
}

/// Buffer `half` of FIFO `ch`'s address.
const fn buf(ch: usize, half: usize) -> usize {
    BUF + (2 * ch + half) * LEN
}

/// Fill buffer `half` of both FIFOs with the next `LEN` samples: on A a
/// sawtooth whose step changes with each buffer, on B a square whose
/// period does.
fn fill(half: usize) {
    let n = state(0);
    let (mut a, mut b) = (state(1), state(2));
    let step = 2 + n % 5;
    let period = 32 + 8 * (n % 4);
    for i in 0..LEN {
        a = a.wrapping_add(step);
        b = (b + 1) % period;
        let sa = (a % 128) as i32 - 64;
        let sb: i32 = if b < period / 2 { 40 } else { -40 };
        // SAFETY: the buffers are IWRAM nothing else uses.
        unsafe {
            write_volatile((buf(0, half) + i) as *mut i8, sa as i8);
            write_volatile((buf(1, half) + i) as *mut i8, sb as i8);
        }
    }
    set_state(0, n + 1);
    set_state(1, a);
    set_state(2, b);
}

/// Point FIFO DMA channel `sad` (DMA1SAD or DMA2SAD) at `src` and switch it
/// on afresh (off first, so it latches).
fn latch(sad: usize, src: usize, fifo: usize) {
    store16(sad + 10, 0);
    store32(sad, src as u32);
    store32(sad + 4, fifo as u32);
    store16(sad + 10, FIFO_DMA);
}

/// Timer 1's IRQ: buffer `half` has played. Play the other, refill this.
fn swap() {
    let played = (state(0) % 2) as usize;
    let next = 1 - played;
    latch(DMA1SAD, buf(0, next), FIFO_A);
    latch(DMA2SAD, buf(1, next), FIFO_B);
    fill(played);
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    store16(DISPCNT, 1 << 7);
    for i in 0..3 {
        set_state(i, 0);
    }
    fill(0);
    fill(1);
    // Swaps counts buffers filled; the first swap plays buffer 1.
    set_state(0, 0);
    // Master on, no PSG; FIFO A left and B right at 100%, both on timer
    // 0, both reset.
    store16(SOUNDCNT_X, 0x80);
    store16(SOUNDCNT_L, 0);
    store16(SOUNDCNT_H, 1 << 2 | 1 << 3 | 1 << 9 | 1 << 11 | 1 << 12 | 1 << 15);
    latch(DMA1SAD, buf(0, 0), FIFO_A);
    latch(DMA2SAD, buf(1, 0), FIFO_B);
    // SAFETY: the handler touches only the sound registers, the DMA's and
    // the IWRAM above, which main no longer does.
    let _swap = unsafe {
        agb::interrupt::add_interrupt_handler(agb::interrupt::Interrupt::Timer1, |_| swap())
    };
    // Timer 1 counts timer 0's overflows, IRQ every LEN; timer 0
    // overflows every 1024 cycles (16384 Hz). The FIFOs refill at pops
    // 1, 2, then every 16th (17, 33, ...): the first period is 8 pops
    // longer, so each swap comes 8 pops before a refill and its handler
    // has 8192 cycles to re-latch, in any emulator's timing (SOUND.md).
    store16(TM1CNT, (0x1_0000 - LEN - 8) as u16);
    store16(TM1CNT + 2, 0x80 | 0x40 | 0x04);
    store16(TM1CNT, (0x1_0000 - LEN) as u16);
    store16(TM0CNT, 0xFC00);
    store16(TM0CNT + 2, 0x80);
    let vblank = agb::interrupt::VBlank::get();
    loop {
        vblank.wait_for_vblank();
    }
}
