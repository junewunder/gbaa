//! `native-psg-square`: agb/psg-square as a native Rust program, the
//! native check's sound program. The same flags as `native-spike`
//! (src/host.rs).
//!
//! What it plays (agb/psg-square/SOUND.md): each V-blank, DMA3 copies the
//! next row of `SCORE` to the sound registers `0x60`-`0x83`, here a
//! V-blank DMA callback writing them into [`Hw::sound`]; FIFO A plays a
//! triangle wave through DMA1 on timer 0. The library has no timers, no
//! FIFO DMA and no mixer yet, so the FIFO's set-up (DMA1, timer 0) isn't
//! here and nothing is played: the check compares the sound registers
//! this program writes with the play build's, not the audio.

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
    // (DMA1 feeding FIFO A, and timer 0 clocking it: not in the library.)
    // DMA3: the next row of SCORE to the sound registers every V-blank.
    g.hw.dma.vblank(3, score_row);
    // agb's `VBlank::get`.
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, |_| {});
    loop {
        hw::wait_vblank(g)?;
    }
}

/// DMA3's V-blank copy: nine words of the score from `0x60`.
fn score_row(g: &mut Game) {
    let row = &SCORE[g.row];
    for (i, &w) in row.iter().enumerate() {
        let o = 0x60 + 4 * i as u32;
        g.hw.write16(o, w as u16);
        g.hw.write16(o + 2, (w >> 16) as u16);
    }
    // Past the score DMA3 reads on into the ROM, which this doesn't
    // have: the index panics there, and the check runs fewer frames.
    g.row += 1;
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
