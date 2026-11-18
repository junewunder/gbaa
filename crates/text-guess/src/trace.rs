//! Running a ROM in the interpreter and recording the CPU's data loads from
//! the cartridge ([`runtime::debug`]'s watch): what the text renderer
//! reads while a scene's text appears. DMA and the BIOS's decompression
//! calls aren't seen, so a font the game copies whole by DMA, or
//! decompresses, shows no per-glyph reads.

use runtime::Machine;
use runtime::debug::{Access, Debugger, Watch};
use runtime::{Frame, Platform};

/// One load from the cartridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Load {
    pub frame: u32,
    pub addr: u32,
    /// In bytes: 1, 2 or 4.
    pub width: u32,
    pub value: u32,
}

/// Keys held on given frames: `(first, last, keys)`.
pub struct Pad {
    pub presses: Vec<(u32, u32, u16)>,
    pub frame: u32,
}

impl Platform for Pad {
    fn present(&mut self, _frame: &Frame) {}

    fn keys(&mut self) -> u16 {
        self.presses
            .iter()
            .filter(|(a, b, _)| (*a..=*b).contains(&self.frame))
            .fold(0, |k, p| k | p.2)
    }
}

/// A key by its name: `a`, `b`, `select`, `start`, `right`, `left`, `up`,
/// `down`, `r`, `l`.
pub fn key(name: &str) -> Option<u16> {
    use runtime::io::key::*;
    Some(match name {
        "a" => A,
        "b" => B,
        "select" => SELECT,
        "start" => START,
        "right" => RIGHT,
        "left" => LEFT,
        "up" => UP,
        "down" => DOWN,
        "r" => R,
        "l" => L,
        _ => return None,
    })
}

/// `start@60` (held one frame) or `a@100+3` (held four): a press.
pub fn parse_press(s: &str) -> Option<(u32, u32, u16)> {
    let (k, at) = s.split_once('@')?;
    let (f, extra) = match at.split_once('+') {
        Some((f, n)) => (f.parse().ok()?, n.parse().ok()?),
        None => (at.parse().ok()?, 0),
    };
    Some((f, f + extra, key(k)?))
}

struct Recorder {
    on: bool,
    frame: u32,
    loads: Vec<Load>,
}

impl Watch for Recorder {
    fn access(&mut self, a: Access) {
        if self.on && !a.write && (a.addr >> 24 != 4) {
            self.loads.push(Load {
                frame: self.frame,
                addr: a.addr,
                width: a.width,
                value: a.old,
            });
        }
    }
}

impl Debugger<Pad> for Recorder {
    fn before(&mut self, _m: &mut Machine<Pad>) {}
    fn watching(&self) -> bool {
        self.on
    }
    fn after(&mut self, _m: &mut Machine<Pad>) {}
}

/// The cartridge loads of frames `from..to` (counted from power-on, the
/// first frame 0), with `presses` held. Timing exact, as checks run.
pub fn record(rom: &'static [u8], presses: Vec<(u32, u32, u16)>, from: u32, to: u32) -> Vec<Load> {
    let mut m = Box::new(Machine::new(rom, Pad { presses, frame: 0 }));
    m.set_timing(runtime::Timing::Exact);
    let mut spins = Vec::new();
    runtime::spins::listed(&runtime::sha1::sha1(rom), |e| spins.push(e));
    spins.sort_unstable();
    m.spin_list = Box::leak(spins.into_boxed_slice());
    let mut r = Recorder {
        on: false,
        frame: 0,
        loads: Vec::new(),
    };
    for f in 0..to {
        m.platform.frame = f;
        r.frame = f;
        r.on = f >= from;
        if r.on {
            m.run_frame_debug(&mut r);
        } else {
            m.run_frame();
        }
        if m.fault.is_some() {
            break;
        }
    }
    r.loads
}

/// Each code the renderer loaded from inside `strings` (a string's own
/// bytes, in `unit`-byte loads), with the cartridge addresses it read
/// after, before the next code: the reads [`crate::fit_font`] sorts into
/// tables by code. A code read twice keeps both runs' addresses.
pub fn code_reads(loads: &[Load], strings: (u32, u32), unit: u32) -> Vec<(u32, Vec<u32>)> {
    let mut out: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut cur: Option<usize> = None;
    for l in loads {
        if (strings.0..strings.1).contains(&l.addr) {
            if l.width == unit {
                cur = Some(match out.iter().position(|(c, _)| *c == l.value) {
                    Some(i) => i,
                    None => {
                        out.push((l.value, Vec::new()));
                        out.len() - 1
                    }
                });
            }
            continue;
        }
        if let Some(i) = cur
            && l.addr >> 24 >= 8
            && !out[i].1.contains(&l.addr)
        {
            out[i].1.push(l.addr);
        }
    }
    out
}
