//! `native-psg-square`: agb/psg-square as a native Rust program, the
//! native check's sound program. The same flags as `native-spike`
//! (src/host.rs).
//!
//! What it plays (agb/psg-square/SOUND.md): each V-blank, DMA3 copies the
//! next row of `SCORE` to the sound registers `0x60`-`0x83`, here a
//! V-blank DMA callback writing them into [`Hw::sound`] a word at a time,
//! at the play build's price for the copy; FIFO A plays `WAVE`, a
//! triangle wave, through DMA1 ([`Hw::fifo_dma`]) on timer 0. The
//! library mixes them with the play build's own sound path, so the check
//! compares every frame's samples.

mod host;

use std::convert::Infallible;

use runtime::hw::{self, DispCnt, Exit, Hw, Native};
use runtime::io::{irq, reg};

pub use host::AnyHost;

/// The whole program's state.
pub struct Game {
    pub ram: Box<Ram>,
    pub hw: Hw<Game>,
    /// DMA3's source, as the row of `SCORE` it copies next: a DMA
    /// register, not RAM.
    row: usize,
}

impl Native for Game {
    type Host = AnyHost;
    fn hw(&mut self) -> &mut Hw<Game> {
        &mut self.hw
    }
    /// No CPSR: written over the library, never in a critical section.
    fn irq_masked(&self) -> bool {
        false
    }
}

/// The program has no globals of its own; in the check layout its RAM is
/// the ROM's EWRAM and IWRAM, all of it agb's (skipped by the check by
/// name, as `native-spike`'s are).
#[derive(zerocopy::IntoBytes, zerocopy::FromZeros, zerocopy::Immutable)]
#[repr(C)]
pub struct Ram {
    _ewram: [u8; 0x4_0000],
    _iwram: [u8; 0x8000],
}

impl Ram {
    fn new() -> Box<Ram> {
        <Ram as zerocopy::FromZeros>::new_box_zeroed().expect("RAM's 288 KB")
    }
}

/// Frames the score covers.
const FRAMES: usize = 128;

/// agb/psg-square's `main`, from agb's entry on.
pub fn program(g: &mut Game) -> Result<Infallible, Exit> {
    host::agb_crt0(
        g,
        &host::Sections {
            ewram: (0x0802_DEB0, 0xB8),
            iwram: (0x0802_DC84, 0x22C),
        },
    )?;
    g.hw.ppu.dispcnt = DispCnt::FORCED_BLANK;
    // Master enable, then FIFO A set up and reset.
    g.hw.write16(reg::SOUNDCNT_X, 0x80);
    g.hw.write16(reg::SOUNDCNT_H, cnt_h(2) | 1 << 11);
    // DMA1: FIFO A from WAVE, 32-bit, repeat, special timing.
    g.hw.fifo_dma(
        1,
        0,
        WAVE_AT,
        0x8000 | 3 << 12 | 1 << 10 | 1 << 9 | 2 << 5,
        rom32,
    );
    // DMA3: the next row of SCORE to the sound registers every V-blank.
    g.hw.dma.vblank(3, score_row);
    // Timer 0: overflow every 1024 cycles.
    g.hw.write16(reg::TM0CNT_L, 0xFC00);
    g.hw.write16(reg::TM0CNT_L + 2, 0x80);
    // agb's `VBlank::get`.
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, |_| {});
    loop {
        hw::wait_vblank(g)?;
    }
}

/// DMA3's V-blank copy: nine words of the score from `0x60`, each a word
/// store as DMA's 32-bit units are. Returns the play build's price for it.
fn score_row(g: &mut Game) -> u32 {
    let row = &SCORE[g.row];
    for (i, &w) in row.iter().enumerate() {
        g.hw.write32(0x60 + 4 * i as u32, w);
    }
    // Past the score DMA3 reads on into the ROM, which this doesn't
    // have: the index panics there, and the check runs fewer frames.
    let src = SCORE_AT + 36 * g.row as u32;
    g.row += 1;
    hw::dma_cycles(g.hw.waitcnt, 4, src, 0x0400_0060, 9)
}

/// Where the ROM keeps `WAVE` and, right after it, `SCORE` (the ELF's
/// symbols): FIFO A's DMA reads on from one into the other.
const WAVE_AT: u32 = 0x0800_70C8;
const SCORE_AT: u32 = WAVE_AT + 32768;

/// The ROM's word at `a` (aligned), for FIFO A's DMA: `WAVE`, then
/// `SCORE`; past those, the ROM this program doesn't have (the check runs
/// fewer frames than reach it).
fn rom32(_: &mut Game, a: u32) -> u32 {
    let o = a.wrapping_sub(WAVE_AT) as usize;
    if o < WAVE.len() {
        // The planted fault (`--features plant`): one sample of the
        // wave's 32768 a step off, which only the audio compare sees.
        let b = |i: usize| {
            let off = cfg!(feature = "plant") && o + i == 20_000;
            (WAVE[o + i] as u8).wrapping_add(off as u8)
        };
        return u32::from_le_bytes([b(0), b(1), b(2), b(3)]);
    }
    let w = (o - WAVE.len()) / 4;
    SCORE[w / 9][w % 9]
}

/// FIFO A's stream: a triangle wave, period 64 samples, peak ±48 (the
/// agb source's `wave`, run at compile time).
static WAVE: [i8; 32768] = wave();

const fn wave() -> [i8; 32768] {
    let mut w = [0i8; 32768];
    let mut i = 0;
    while i < w.len() {
        let p = (i % 64) as i32;
        let v = if p < 32 {
            p * 3 - 48
        } else {
            (63 - p) * 3 - 48
        };
        w[i] = v as i8;
        i += 1;
    }
    w
}

fn record(g: &mut Game) {
    host::record(&g.hw, zerocopy::IntoBytes::as_bytes(&*g.ram));
}

fn main() -> std::process::ExitCode {
    host::main(
        "native-psg-square",
        true,
        |h| {
            let mut g = Box::new(Game {
                ram: Ram::new(),
                hw: Hw::new(h),
                row: 0,
            });
            g.hw.on_frame_end(record);
            g
        },
        program,
    )
}

const RESTART: u16 = 1 << 15;
const STOP: u16 = 1 << 14;

/// The ROM's `SCORE`, by the agb source's `const fn`s (agb/psg-square),
/// run at compile time: the same bytes.
static SCORE: [[u32; 9]; FRAMES] = score();

const fn note(n: u16) -> u16 {
    2048 - 32 * n
}

const fn cnt_h(psg: u16) -> u16 {
    psg | 1 << 2 | 1 << 8 | 1 << 9
}

const fn env(vol: u16, step: u16, up: bool) -> u16 {
    vol << 4 | step | if up { 8 } else { 0 }
}

const fn duty(d: u16, len: u16) -> u16 {
    d << 6 | len
}

struct Regs {
    nr10: u16,
    nr1112: u16,
    ch1x: u16,
    nr2122: u16,
    ch2x: u16,
    cnt_l: u16,
    cnt_h: u16,
}

const fn score() -> [[u32; 9]; FRAMES] {
    let mut t = [[0u32; 9]; FRAMES];
    let mut s = Regs {
        nr10: 0,
        nr1112: 0,
        ch1x: 0,
        nr2122: 0,
        ch2x: 0,
        cnt_l: 0x77 | 1 << 8 | 1 << 9 | 1 << 12,
        cnt_h: cnt_h(2),
    };
    let mut f = 0;
    while f < FRAMES {
        let (mut r1, mut r2) = (0, 0);
        match f {
            2 => {
                s.nr2122 = duty(2, 0) | env(15, 0, false) << 8;
                s.ch2x = note(8);
                r2 = RESTART;
            }
            10 => {
                s.nr1112 = duty(1, 0) | env(12, 2, false) << 8;
                s.ch1x = note(10);
                r1 = RESTART;
            }
            20 => s.ch2x = note(16),
            30 => {
                s.nr10 = 2 << 4 | 3;
                s.nr1112 = duty(2, 0) | env(15, 0, false) << 8;
                s.ch1x = 1024;
                r1 = RESTART;
            }
            45 => {
                s.nr10 = 1 << 4 | 1 << 3 | 2;
                s.nr1112 = duty(3, 0) | env(4, 1, true) << 8;
                s.ch1x = 1800;
                r1 = RESTART;
            }
            55 => {
                s.nr2122 = duty(0, 60) | env(15, 0, false) << 8;
                s.ch2x = note(8) | STOP;
                r2 = RESTART;
            }
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
            70 => s.cnt_h = cnt_h(1),
            76 => s.cnt_h = cnt_h(0),
            80 => {
                s.cnt_h = cnt_h(2);
                s.nr1112 = duty(2, 0) | env(0, 1, true) << 8;
                s.ch1x = note(4);
                r1 = RESTART;
            }
            90 => {
                s.nr2122 = duty(2, 0) | env(0, 1, false) << 8;
                r2 = RESTART;
            }
            95 => {
                s.nr10 = 3 << 4 | 1 << 3 | 1;
                s.nr1112 = duty(1, 32) | env(13, 0, false) << 8;
                s.ch1x = 2000 | STOP;
                r1 = RESTART;
            }
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
