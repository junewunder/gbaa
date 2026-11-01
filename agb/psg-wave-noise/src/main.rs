//! gbaa test ROM for the PSG's wave channel 3 and noise channel 4, mixed
//! with FIFO A (the `sound/psg-wave-noise` unit; what it plays is in
//! `SOUND.md`).
//!
//! Every PSG register write after setup comes from DMA at V-blank, so the
//! writes land at the same cycle in mGBA and in ours, whatever the CPU's
//! timing: each V-blank DMA3 copies the next row of `SCORE`, five words,
//! to `0x04000078`-`0x0400008B` (channel 4, `SOUNDCNT_L`/`_H`/`_X`,
//! `SOUNDBIAS`), and in the rows that change channel 3, DMA2 (armed by the
//! CPU the frame before) copies its two words to `0x04000070` first. Only
//! those rows write channel 3, since each write to its `SOUND3CNT_X` sets
//! when it next steps (`SOUND.md`). The CPU writes wave RAM just after a
//! V-blank, and only to the bank not played. FIFO A plays `WAVE` through
//! DMA1, clocked by timer 0 every 1024 cycles (16384 Hz).
#![no_std]
#![no_main]

use core::ptr::write_volatile;

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const SOUND3CNT_L: usize = IO + 0x70;
const SOUNDCNT_H: usize = IO + 0x82;
const SOUNDCNT_X: usize = IO + 0x84;
const WAVE_RAM: usize = IO + 0x90;
const FIFO_A: usize = IO + 0xA0;
const DMA1SAD: usize = IO + 0xBC;
const DMA2SAD: usize = IO + 0xC8;
const DMA3SAD: usize = IO + 0xD4;
const TM0CNT: usize = IO + 0x100;

/// Frames the score covers; the check runs fewer.
const FRAMES: usize = 128;
/// Words DMA3 writes each row, from `0x04000078`.
const ROW: usize = 5;

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

/// Channel 3 rate whose step is `512 * n` cycles: `8 * (2048 - r)`, a
/// whole number of samples.
const fn rate(n: u16) -> u16 {
    2048 - 64 * n
}

const RESTART: u16 = 1 << 15;
const STOP: u16 = 1 << 14;

/// `SOUND3CNT_L`: DAC on, bank played, 64-sample mode.
const fn bank(on: bool, bank: u16, size: bool) -> u16 {
    (on as u16) << 7 | bank << 6 | (size as u16) << 5
}

/// `SOUND3CNT_H`: length byte and volume (0 mute, 1 100%, 2 50%, 3 25%,
/// 4 75%).
const fn vol3(vol: u16, len: u16) -> u16 {
    vol << 13 | len
}

/// Envelope byte: initial volume, step time, increasing.
const fn env(vol: u16, step: u16, up: bool) -> u16 {
    vol << 4 | step | if up { 8 } else { 0 }
}

/// `SOUND4CNT_H` low byte: ratio, 7-bit LFSR, shift. A clock every
/// `32 * r * 2^s` cycles (`r` = ½ for 0).
const fn noise(ratio: u16, short: bool, shift: u16) -> u16 {
    ratio | (short as u16) << 3 | shift << 4
}

/// `SOUNDCNT_H`: PSG at `psg` (0 25%, 1 50%, 2 100%), FIFO A at 100% to
/// both speakers on timer 0.
const fn cnt_h(psg: u16) -> u16 {
    psg | 1 << 2 | 1 << 8 | 1 << 9
}

/// Thirty-two 4-bit samples as wave RAM words, high nibble first.
const fn pack(n: [u8; 32]) -> [u32; 4] {
    let mut w = [0u32; 4];
    let mut i = 0;
    while i < 32 {
        let byte = i / 2;
        let shift = 8 * (byte % 4) + if i % 2 == 0 { 4 } else { 0 };
        w[byte / 4] |= (n[i] as u32 & 0xF) << shift;
        i += 1;
    }
    w
}

/// Triangle, 0 up to 15 and down.
const fn triangle() -> [u8; 32] {
    let mut n = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        n[i] = if i < 16 { i as u8 } else { 31 - i as u8 };
        i += 1;
    }
    n
}

/// Sawtooth, 0 up to 15 twice.
const fn saw() -> [u8; 32] {
    let mut n = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        n[i] = (i % 16) as u8;
        i += 1;
    }
    n
}

/// Steps: eight samples each of 15, 3, 10 and 0.
const fn steps() -> [u8; 32] {
    let mut n = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        n[i] = [15, 3, 10, 0][i / 8];
        i += 1;
    }
    n
}

const W_TRIANGLE: [u32; 4] = pack(triangle());
const W_SAW: [u32; 4] = pack(saw());
const W_STEPS: [u32; 4] = pack(steps());
/// No pattern.
const W_MIXED: [u32; 4] = [0x1E2D_3C4B, 0x5A69_7887, 0x96A5_B4C3, 0xD2E1_F00F];

/// The registers one row writes.
#[derive(Clone, Copy)]
struct Regs {
    cnt3_l: u16,
    cnt3_h: u16,
    /// `SOUND3CNT_X`: rate, length enable (restart added per row).
    cnt3_x: u16,
    /// `SOUND4CNT_L`: length (low byte), envelope (high byte).
    cnt4_l: u16,
    /// `SOUND4CNT_H`: clock (low byte), length enable (restart added).
    cnt4_h: u16,
    cnt_l: u16,
    cnt_h: u16,
    /// Wave RAM: the bank not played.
    wave: [u32; 4],
}

/// One row of the score: DMA3's words, channel 3's (if the row writes
/// it), and the wave RAM the CPU writes after the V-blank (if any).
#[derive(Clone, Copy)]
struct Row {
    dma3: [u32; ROW],
    ch3: Option<[u32; 2]>,
    wave: Option<[u32; 4]>,
}

/// The score: what each V-blank writes. `SOUND.md` lists it.
const fn score() -> [Row; FRAMES] {
    let mut t = [Row {
        dma3: [0; ROW],
        ch3: None,
        wave: None,
    }; FRAMES];
    let mut s = Regs {
        cnt3_l: bank(false, 0, false),
        cnt3_h: 0,
        cnt3_x: 0,
        cnt4_l: 0,
        cnt4_h: 0,
        // Both PSG volumes 7; channels 3 and 4 to both speakers.
        cnt_l: 0x77 | 3 << 10 | 3 << 14,
        cnt_h: cnt_h(2),
        wave: W_SAW,
    };
    let mut prev_ch3 = [0u32; 2];
    let mut prev_wave = [0u32; 4];
    let mut f = 0;
    while f < FRAMES {
        let (mut r3, mut r4) = (0, 0);
        match f {
            // Channel 3 plays bank 1, the sawtooth, at 100%: a step every
            // 1024 cycles, 512 Hz. Bank 0 (the triangle) is the one written.
            2 => {
                s.cnt3_l = bank(true, 1, false);
                s.cnt3_h = vol3(1, 0);
                s.cnt3_x = rate(2);
                r3 = RESTART;
                s.wave = W_TRIANGLE;
            }
            10 => s.cnt3_h = vol3(2, 0),
            14 => s.cnt3_h = vol3(3, 0),
            18 => s.cnt3_h = vol3(4, 0),
            22 => s.cnt3_h = vol3(0, 0),
            // Bank 0 (the triangle) at 100%, no restart; bank 1 written.
            26 => {
                s.cnt3_l = bank(true, 0, false);
                s.cnt3_h = vol3(1, 0);
                s.wave = W_STEPS;
            }
            // Bank 1, now the steps.
            32 => {
                s.cnt3_l = bank(true, 1, false);
                s.wave = W_MIXED;
            }
            // 64-sample mode, a step every 512 cycles; the DMA rewrites
            // bank 1 as it plays.
            36 => {
                s.cnt3_l = bank(true, 0, true);
                s.cnt3_x = rate(1);
                s.wave = W_STEPS;
            }
            // DAC off: the channel stops.
            44 => s.cnt3_l = bank(false, 0, false),
            // Restart with length 16/256 s (stop on).
            48 => {
                s.cnt3_l = bank(true, 1, false);
                s.cnt3_h = vol3(1, 240);
                s.cnt3_x = rate(3) | STOP;
                r3 = RESTART;
                s.wave = W_TRIANGLE;
            }
            // Restart without length: bank 0, a step every 2048 cycles.
            56 => {
                s.cnt3_l = bank(true, 0, false);
                s.cnt3_h = vol3(1, 0);
                s.cnt3_x = rate(4);
                r3 = RESTART;
                s.wave = W_SAW;
            }
            // Channel 3 muted; channel 4: volume 15 held, 15-bit, a clock
            // every 512 cycles.
            60 => {
                s.cnt3_h = vol3(0, 0);
                s.cnt4_l = env(15, 0, false) << 8;
                s.cnt4_h = noise(0, false, 4);
                r4 = RESTART;
            }
            // 7-bit, every 512 cycles.
            66 => {
                s.cnt4_h = noise(1, true, 3);
                r4 = RESTART;
            }
            // Volume 12 falling every 1/64 s, 15-bit, every 1024 cycles.
            72 => {
                s.cnt4_l = env(12, 1, false) << 8;
                s.cnt4_h = noise(2, false, 3);
                r4 = RESTART;
            }
            // Volume 3 rising every 2/64 s, 7-bit, every 3072 cycles.
            80 => {
                s.cnt4_l = env(3, 2, true) << 8;
                s.cnt4_h = noise(3, true, 4);
                r4 = RESTART;
            }
            // Length 2/256 s (stop on), volume 15: silent within a frame.
            88 => {
                s.cnt4_l = env(15, 0, false) << 8 | 62;
                s.cnt4_h = noise(0, false, 5) | STOP;
                r4 = RESTART;
            }
            // Volume 10, 15-bit, every 2048 cycles, no length.
            94 => {
                s.cnt4_l = env(10, 0, false) << 8;
                s.cnt4_h = noise(0, false, 6);
                r4 = RESTART;
            }
            // A new clock (every 1024 cycles) without a restart.
            100 => s.cnt4_h = noise(1, false, 4),
            // Both: channel 3 at 100% left, channel 4 right; left volume
            // 7, right 3; PSG at 50%.
            104 => {
                s.cnt3_h = vol3(1, 0);
                s.cnt_l = 0x73 | 1 << 11 | 1 << 14;
                s.cnt_h = cnt_h(1);
            }
            110 => s.cnt_h = cnt_h(0),
            // PSG at 100%; channel 3 restarted in 64-sample mode; channel
            // 4 restarted at volume 0 rising.
            114 => {
                s.cnt_h = cnt_h(2);
                s.cnt3_l = bank(true, 1, true);
                r3 = RESTART;
                s.cnt4_l = env(0, 1, true) << 8;
                r4 = RESTART;
            }
            _ => {}
        }
        let ch3 = [
            s.cnt3_l as u32 | (s.cnt3_h as u32) << 16,
            (s.cnt3_x | r3) as u32,
        ];
        let wave = s.wave;
        t[f].dma3 = [
            s.cnt4_l as u32,
            (s.cnt4_h | r4) as u32,
            s.cnt_l as u32 | (s.cnt_h as u32) << 16,
            // SOUNDCNT_X: master on. SOUNDBIAS: the BIOS's.
            0x80,
            0x200,
        ];
        // A restart, or a change: the restart bit alone isn't one.
        let held = [ch3[0], s.cnt3_x as u32];
        if f == 0 || r3 != 0 || !same(&held, &prev_ch3) {
            t[f].ch3 = Some(ch3);
        }
        // In 64-sample mode both banks play: no writes then.
        if s.cnt3_l & 0x20 == 0 && (f == 0 || !same(&wave, &prev_wave)) {
            t[f].wave = Some(wave);
        }
        prev_ch3 = held;
        prev_wave = wave;
        f += 1;
    }
    t
}

/// Whether two word slices are equal (in a `const fn`).
const fn same(a: &[u32], b: &[u32]) -> bool {
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

static SCORE: [Row; FRAMES] = score();

/// DMA3's words of every row, back to back, for it to walk.
static DMA3_SCORE: [[u32; ROW]; FRAMES] = {
    let mut t = [[0u32; ROW]; FRAMES];
    let mut f = 0;
    while f < FRAMES {
        t[f] = SCORE[f].dma3;
        f += 1;
    }
    t
};

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
    // Wave RAM: the triangle into bank 0 (bank 1 selected), then the
    // sawtooth into bank 1 (bank 0 selected), DAC off.
    for (bank, w) in [(1, W_TRIANGLE), (0, W_SAW)] {
        store16(SOUND3CNT_L, bank << 6);
        write_wave(&w);
    }
    // DMA1: FIFO A from WAVE, 32-bit, repeat, special timing.
    store32(DMA1SAD, WAVE.0.as_ptr() as u32);
    store32(DMA1SAD + 4, FIFO_A as u32);
    store16(DMA1SAD + 10, 0x8000 | 3 << 12 | 1 << 10 | 1 << 9 | 2 << 5);
    // DMA3: five words of the score to 0x04000078 every V-blank, the
    // destination reloaded each time.
    store32(DMA3SAD, DMA3_SCORE.as_ptr() as u32);
    store32(DMA3SAD + 4, (IO + 0x78) as u32);
    store16(DMA3SAD + 8, ROW as u16);
    store16(DMA3SAD + 10, 0x8000 | 1 << 12 | 1 << 10 | 1 << 9 | 3 << 5);
    // Timer 0: overflow every 1024 cycles.
    store16(TM0CNT, 0xFC00);
    store16(TM0CNT + 2, 0x80);
    let vblank = agb::interrupt::VBlank::get();
    // Row f is written at the f-th V-blank from here.
    for row in SCORE.iter() {
        if let Some(ch3) = &row.ch3 {
            // DMA2: channel 3's two words at the next V-blank, once.
            store32(DMA2SAD, ch3.as_ptr() as u32);
            store32(DMA2SAD + 4, SOUND3CNT_L as u32);
            store16(DMA2SAD + 8, 2);
            store16(DMA2SAD + 10, 0x8000 | 1 << 12 | 1 << 10);
        }
        vblank.wait_for_vblank();
        if let Some(w) = &row.wave {
            write_wave(w);
        }
    }
    loop {
        vblank.wait_for_vblank();
    }
}

/// Wave RAM's four words: the bank not played.
fn write_wave(w: &[u32; 4]) {
    for (i, &word) in w.iter().enumerate() {
        store32(WAVE_RAM + 4 * i, word);
    }
}
