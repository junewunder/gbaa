//! gbaa test ROM for the PSG's square channels 1 and 2, mixed with FIFO A
//! (the `sound/psg-square` unit; what it plays is in `SOUND.md`).
//!
//! Every PSG write comes from DMA3 at V-blank: each V-blank it copies the
//! next row of `SCORE`, nine words, to `0x04000060`-`0x04000083` (channels
//! 1-4 and `SOUNDCNT_L`/`_H`), so the writes land at the same cycle in
//! mGBA and in ours, whatever the CPU's timing. FIFO A plays `WAVE` through
//! DMA1, clocked by timer 0 every 1024 cycles (16384 Hz).
#![no_std]
#![no_main]

use core::ptr::write_volatile;

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const SOUNDCNT_H: usize = IO + 0x82;
const SOUNDCNT_X: usize = IO + 0x84;
const FIFO_A: usize = IO + 0xA0;
const DMA1SAD: usize = IO + 0xBC;
const DMA3SAD: usize = IO + 0xD4;
const TM0CNT: usize = IO + 0x100;

/// Frames the score covers; the check runs fewer.
const FRAMES: usize = 128;

/// One halfword of I/O.
fn store16(addr: usize, v: u16) {
    // SAFETY: callers pass I/O registers.
    unsafe { write_volatile(addr as *mut u16, v) }
}

/// One word of I/O.
fn store32(addr: usize, v: u32) {
    // SAFETY: callers pass I/O registers.
    unsafe { write_volatile(addr as *mut u32, v) }
}

/// Square channel frequency `f` (11 bits) as a period that's a multiple of
/// 512 cycles: `16 * (2048 - f) = 512 * n`, `4096 / n` Hz.
const fn note(n: u16) -> u16 {
    2048 - 32 * n
}

const RESTART: u16 = 1 << 15;
const STOP: u16 = 1 << 14;

/// The registers one row writes.
#[derive(Clone, Copy)]
struct Regs {
    /// `SOUND1CNT_L`: sweep.
    nr10: u16,
    /// `SOUND1CNT_H`: length, duty (low byte), envelope (high byte).
    nr1112: u16,
    /// `SOUND1CNT_X`: frequency, length enable (restart added per row).
    ch1x: u16,
    /// `SOUND2CNT_L`: as `nr1112`, channel 2.
    nr2122: u16,
    /// `SOUND2CNT_H`: as `ch1x`, channel 2.
    ch2x: u16,
    cnt_l: u16,
    cnt_h: u16,
}

/// `SOUNDCNT_H`: PSG at `psg` (0 25%, 1 50%, 2 100%), FIFO A at 100% to
/// both speakers on timer 0.
const fn cnt_h(psg: u16) -> u16 {
    psg | 1 << 2 | 1 << 8 | 1 << 9
}

/// Envelope byte: initial volume, step time, increasing.
const fn env(vol: u16, step: u16, up: bool) -> u16 {
    vol << 4 | step | if up { 8 } else { 0 }
}

/// Duty and length byte: duty 0-3 (12.5%, 25%, 50%, 75%), length 0-63.
const fn duty(d: u16, len: u16) -> u16 {
    d << 6 | len
}

/// The score: what each V-blank writes. `SOUND.md` lists it.
const fn score() -> [[u32; 9]; FRAMES] {
    let mut t = [[0u32; 9]; FRAMES];
    let mut s = Regs {
        nr10: 0,
        nr1112: 0,
        ch1x: 0,
        nr2122: 0,
        ch2x: 0,
        // Both PSG volumes 7; channel 1 to both speakers, channel 2 right.
        cnt_l: 0x77 | 1 << 8 | 1 << 9 | 1 << 12,
        cnt_h: cnt_h(2),
    };
    let mut f = 0;
    while f < FRAMES {
        let (mut r1, mut r2) = (0, 0);
        match f {
            // Channel 2: 50% duty, volume 15 held, 512 Hz.
            2 => {
                s.nr2122 = duty(2, 0) | env(15, 0, false) << 8;
                s.ch2x = note(8);
                r2 = RESTART;
            }
            // Channel 1: 25% duty, volume 12 falling every 2/64 s, 409.6 Hz.
            10 => {
                s.nr1112 = duty(1, 0) | env(12, 2, false) << 8;
                s.ch1x = note(10);
                r1 = RESTART;
            }
            // Channel 2 to 256 Hz without a restart.
            20 => s.ch2x = note(16),
            // Channel 1: sweep up (time 2, shift 3) from frequency 1024
            // until it overflows and stops the channel.
            30 => {
                s.nr10 = 2 << 4 | 3;
                s.nr1112 = duty(2, 0) | env(15, 0, false) << 8;
                s.ch1x = 1024;
                r1 = RESTART;
            }
            // Channel 1: sweep down (time 1, shift 2) from 1800, volume
            // rising from 4.
            45 => {
                s.nr10 = 1 << 4 | 1 << 3 | 2;
                s.nr1112 = duty(3, 0) | env(4, 1, true) << 8;
                s.ch1x = 1800;
                r1 = RESTART;
            }
            // Channel 2: 12.5% duty, length 4/256 s, then silent.
            55 => {
                s.nr2122 = duty(0, 60) | env(15, 0, false) << 8;
                s.ch2x = note(8) | STOP;
                r2 = RESTART;
            }
            // Routing: both channels to both speakers, left volume 7, right 3.
            60 => {
                s.nr10 = 0;
                s.nr1112 = duty(2, 0) | env(10, 0, false) << 8;
                s.ch1x = note(12);
                r1 = RESTART;
                s.nr2122 = duty(1, 0) | env(9, 3, false) << 8;
                s.ch2x = note(6);
                r2 = RESTART;
                s.cnt_l = 0x73 | 3 << 8 | 3 << 12;
            }
            // PSG at 50%, then 25%. (Not in a row 8k + 3: its V-blank is
            // so close before a sample is taken that mGBA's DMA, which
            // reaches SOUNDCNT_H tens of cycles in, writes it after. SOUND.md.)
            70 => s.cnt_h = cnt_h(1),
            76 => s.cnt_h = cnt_h(0),
            // Channel 1: volume rising from 0, PSG back to 100%.
            80 => {
                s.cnt_h = cnt_h(2);
                s.nr1112 = duty(2, 0) | env(0, 1, true) << 8;
                s.ch1x = note(4);
                r1 = RESTART;
            }
            // Channel 2 restarted with its DAC off (volume 0, falling).
            90 => {
                s.nr2122 = duty(2, 0) | env(0, 1, false) << 8;
                r2 = RESTART;
            }
            // Channel 1: length 32/256 s with sweep down from 2000.
            95 => {
                s.nr10 = 3 << 4 | 1 << 3 | 1;
                s.nr1112 = duty(1, 32) | env(13, 0, false) << 8;
                s.ch1x = 2000 | STOP;
                r1 = RESTART;
            }
            // Channel 2 at 1024 Hz, volume 8 falling.
            105 => {
                s.nr2122 = duty(3, 0) | env(8, 4, false) << 8;
                s.ch2x = note(4);
                r2 = RESTART;
            }
            _ => {}
        }
        t[f] = [
            s.nr10 as u32 | (s.nr1112 as u32) << 16,
            (s.ch1x | r1) as u32,
            s.nr2122 as u32,
            (s.ch2x | r2) as u32,
            0,
            0,
            0,
            0,
            s.cnt_l as u32 | (s.cnt_h as u32) << 16,
        ];
        f += 1;
    }
    t
}

static SCORE: [[u32; 9]; FRAMES] = score();

/// FIFO A's stream: a triangle wave, period 64 samples, peak ±48.
const fn wave() -> [i8; 32768] {
    let mut w = [0i8; 32768];
    let mut i = 0;
    while i < w.len() {
        let p = (i % 64) as i32;
        let v = if p < 32 { p * 3 - 48 } else { (63 - p) * 3 - 48 };
        w[i] = v as i8;
        i += 1;
    }
    w
}

#[repr(align(4))]
struct Wave([i8; 32768]);

static WAVE: Wave = Wave(wave());

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    store16(DISPCNT, 1 << 7);
    // Master enable, then FIFO A set up and reset.
    store16(SOUNDCNT_X, 0x80);
    store16(SOUNDCNT_H, cnt_h(2) | 1 << 11);
    // DMA1: FIFO A from WAVE, 32-bit, repeat, special timing.
    store32(DMA1SAD, WAVE.0.as_ptr() as u32);
    store32(DMA1SAD + 4, FIFO_A as u32);
    store16(DMA1SAD + 10, 0x8000 | 3 << 12 | 1 << 10 | 1 << 9 | 2 << 5);
    // DMA3: nine words of SCORE to 0x04000060 every V-blank, the
    // destination reloaded each time.
    store32(DMA3SAD, SCORE.as_ptr() as u32);
    store32(DMA3SAD + 4, (IO + 0x60) as u32);
    store16(DMA3SAD + 8, 9);
    store16(DMA3SAD + 10, 0x8000 | 1 << 12 | 1 << 10 | 1 << 9 | 3 << 5);
    // Timer 0: overflow every 1024 cycles.
    store16(TM0CNT, 0xFC00);
    store16(TM0CNT + 2, 0x80);
    let vblank = agb::interrupt::VBlank::get();
    loop {
        vblank.wait_for_vblank();
    }
}
