//! Rewind: the window keeps the machine's state ([`runtime::snap`]) after
//! every frame, and while the rewind control is held (R, or the pad's
//! `rewind`, the right stick pushed left by default) it steps back one
//! frame each frame, showing each, with the sound muted. Let go and the
//! game runs on from there exactly as it ran the first time.
//!
//! The newest state is kept whole; each older one only as what differs
//! from the state after it ([`delta`]), so stepping back decodes one delta
//! and dropping the oldest costs nothing. Most of a frame's state (the
//! ROM's RAM, VRAM, the cartridge save) doesn't change from one frame to
//! the next, so a delta is a few kilobytes where a state is about 640 KB.
//! The oldest are dropped to keep the whole under a cap
//! (`--rewind-mb`, [`DEFAULT_MB`] by default).

use crate::DesktopMachine;
use std::collections::VecDeque;

/// The default cap on what rewind keeps, in MB: a few minutes of most
/// games (docs/tools/commands.md has the measured rates), and a small
/// share of the Steam Deck's 16 GB.
pub const DEFAULT_MB: usize = 256;

/// The machine's state as rewind keeps it: the payload alone
/// ([`runtime::Machine::save_payload`]), which
/// [`runtime::Machine::load_payload`] loads.
pub fn state(m: &mut DesktopMachine) -> Vec<u8> {
    let mut bytes = Vec::new();
    m.save_payload(&mut |s| bytes.extend_from_slice(s));
    bytes
}

/// The states of the last frames, newest whole and older ones as deltas.
#[derive(Clone, Debug)]
pub struct Rewind {
    cap: usize,
    /// The newest state, and the frame it was saved after.
    newest: Option<(u64, Vec<u8>)>,
    /// Older states, oldest first: each frame's, as a delta from the state
    /// after it.
    older: VecDeque<(u64, Vec<u8>)>,
    /// The deltas' bytes.
    bytes: usize,
}

impl Rewind {
    /// Keep at most `cap` bytes of states.
    pub fn new(cap: usize) -> Rewind {
        Rewind {
            cap,
            newest: None,
            older: VecDeque::new(),
            bytes: 0,
        }
    }

    /// The bytes kept: the newest state and the deltas.
    pub fn bytes(&self) -> usize {
        self.bytes + self.newest.as_ref().map_or(0, |(_, s)| s.len())
    }

    /// How many frames back it reaches.
    pub fn len(&self) -> usize {
        self.older.len()
    }

    pub fn is_empty(&self) -> bool {
        self.older.is_empty()
    }

    /// What it holds: the frames, the bytes, and at that rate the bytes
    /// a minute of rewind takes (3584 frames).
    pub fn summary(&self) -> String {
        let mb = |b: f64| b / f64::from(1 << 20);
        let per_frame = self.bytes as f64 / self.older.len().max(1) as f64;
        format!(
            "rewind: {} frames back in {:.1} MB, {:.1} MB a minute (cap {} MB)",
            self.older.len(),
            mb(self.bytes() as f64),
            mb(per_frame * 3584.0),
            self.cap >> 20,
        )
    }

    /// Keep `state`, the machine's after `frame`, dropping the oldest
    /// states while over the cap.
    pub fn push(&mut self, frame: u64, state: Vec<u8>) {
        if let Some((f, old)) = self.newest.take() {
            let d = delta(&state, &old);
            self.bytes += d.len();
            self.older.push_back((f, d));
        }
        self.newest = Some((frame, state));
        while self.bytes() > self.cap
            && let Some((_, d)) = self.older.pop_front()
        {
            self.bytes -= d.len();
        }
    }

    /// Step back from the newest state to the one before it, and return
    /// that: its frame and the state to load. `None`, with nothing
    /// changed, when there's nothing older.
    pub fn back(&mut self) -> Option<(u64, &[u8])> {
        let (f, d) = self.older.pop_back()?;
        self.bytes -= d.len();
        let newest = self.newest.as_mut().expect("deltas have a newest state");
        *newest = (f, undelta(&newest.1, &d));
        Some((newest.0, &newest.1))
    }
}

/// A run of equal bytes this long or longer ends a literal run in a
/// [`delta`]; shorter ones are cheaper copied.
const MIN_SKIP: usize = 8;

/// `target` as it differs from `base`: pairs of a run of bytes as in
/// `base` and a run of `target`'s own (each length a LEB128 varint, then
/// the literal bytes), to the end of `target`. A tag byte first: 0 for
/// that, 1 for `target` whole when the two lengths differ.
pub fn delta(base: &[u8], target: &[u8]) -> Vec<u8> {
    if base.len() != target.len() {
        let mut out = Vec::with_capacity(1 + target.len());
        out.push(1);
        out.extend_from_slice(target);
        return out;
    }
    let mut out = vec![0];
    let n = target.len();
    let mut i = 0;
    while i < n {
        let start = i;
        i = same_until(base, target, i);
        varint(&mut out, i - start);
        let lit = i;
        while i < n {
            while i < n && base[i] != target[i] {
                i += 1;
            }
            let same = same_until(base, target, i);
            if same - i >= MIN_SKIP || same == n {
                break;
            }
            i = same;
        }
        varint(&mut out, i - lit);
        out.extend_from_slice(&target[lit..i]);
    }
    out.shrink_to_fit();
    out
}

/// Rebuild the target of `d`, a [`delta`] from `base`.
pub fn undelta(base: &[u8], d: &[u8]) -> Vec<u8> {
    let (tag, mut d) = d.split_first().expect("a delta has its tag");
    if *tag == 1 {
        return d.to_vec();
    }
    let mut out = Vec::with_capacity(base.len());
    while !d.is_empty() {
        let skip = read_varint(&mut d);
        out.extend_from_slice(&base[out.len()..out.len() + skip]);
        let lit = read_varint(&mut d);
        let (now, rest) = d.split_at(lit);
        out.extend_from_slice(now);
        d = rest;
    }
    debug_assert_eq!(out.len(), base.len());
    out
}

/// The first index from `i` where `a` and `b` differ, or their length.
fn same_until(a: &[u8], b: &[u8], mut i: usize) -> usize {
    let n = a.len();
    while i + 8 <= n && a[i..i + 8] == b[i..i + 8] {
        i += 8;
    }
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

fn varint(out: &mut Vec<u8>, mut n: usize) {
    while n >= 0x80 {
        out.push(n as u8 | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

fn read_varint(d: &mut &[u8]) -> usize {
    let mut n = 0;
    let mut shift = 0;
    loop {
        let (b, rest) = d.split_first().expect("a whole varint");
        *d = rest;
        n |= ((b & 0x7f) as usize) << shift;
        if b & 0x80 == 0 {
            return n;
        }
        shift += 7;
    }
}
