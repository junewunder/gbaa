//! The feel sim (multiplayer/rollback-feel-sim): what each player's window
//! shows in a rollback session, measured in simulated time so a run
//! repeats exactly and takes milliseconds.
//!
//! `ping-sim` checks hashes; this checks feel. Every peer is a
//! [`Rollback`] on its own consoles, as in a real session, joined by a
//! simulated network whose every path follows a ping-sim profile
//! ([`crate::proxy::Line`], the proxy's own delay model). Each peer is
//! driven as `gbaa-desktop`'s netplay loop drives it ([`Window`]: paced by
//! its sound, or by a clock when muted or headless), each turn costing
//! the time a [`Cost`] says, and each frame it finishes is handed to a
//! display refreshing at 60 Hz on a phase of its own. The consoles are
//! the toy one: what a turn costs comes from a model of two recompiled
//! Kirbys, or from a game's own costs frame by frame ([`Trace`], measured
//! once on its real consoles), never from this machine's clock, so a run
//! repeats exactly. From that:
//!
//! - **late**: a frame finished more than a frame after it was due;
//! - **doubled**: a refresh showing the frame the one before showed (60
//!   Hz against the GBA's 59.73 makes one every 3.7 s whatever the network
//!   does: [`Feel::beat`]);
//! - **skipped**: a frame finished but never shown (two finished between
//!   refreshes);
//! - **rollbacks**, their depth, the frames run again; **holds** (time
//!   sync holding a frame back) and **stalls** (at the window's edge);
//! - **jumps**: shown frames a rollback rewrote since the last shown one,
//!   so the other player is seen to move that many frames' worth at once
//!   (the other Kirby's teleports), and how far.
//!
//! [`Version`] is the desktop's loop before and after this unit's fixes,
//! so both can be measured: `gbaa-pingsim feel` prints the numbers, and
//! the tests put budgets on them.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::rc::Rc;

use crate::proxy::{Line, Profile};
use crate::{Delivery, Error, Netplay, Rollback, Snapshot, Status, To, Transport};

/// A GBA frame, µs: 280896 cycles at 2^24 Hz (59.73 Hz).
pub const GBA_FRAME_US: f64 = 280_896.0 / 16_777_216.0 * 1e6;
/// A display refresh at 60 Hz, µs.
pub const REFRESH_US: f64 = 1e6 / 60.0;
/// How long before a refresh a frame must reach the window to be on it.
pub const DRAW_US: f64 = 1_000.0;
/// The sound queued ahead of the device (`desktop::audio::TARGET_MS`), µs.
pub const AUDIO_TARGET_US: f64 = 50_000.0;
/// [`Version::After`]'s [`Window::present_us`].
pub const PRESENT_US: f64 = 8_000.0;
/// How often a waiting peer tries again (the desktop sleeps 1 ms), µs.
pub const RETRY_US: f64 = 1_000.0;
/// A reliable message lost is resent this much later, µs.
pub const RESEND_US: u64 = 100_000;

/// (when it arrives, order sent, from, to, payload)
type InFlight = Reverse<(u64, u64, usize, usize, Vec<u8>)>;

/// The simulated network: a [`Line`] per pair of seats, each following
/// the profile (seeded per pair), in time the sim moves.
pub struct Net {
    lines: Vec<Line>,
    seats: usize,
    now: u64,
    seq: u64,
    queue: BinaryHeap<InFlight>,
    inbox: Vec<VecDeque<(usize, Vec<u8>)>>,
    /// When the last reliable message from one seat to another arrives.
    last: Vec<Vec<u64>>,
}

impl Net {
    pub fn new(seats: usize, profile: &Profile) -> Rc<RefCell<Net>> {
        let mut lines = Vec::new();
        for a in 0..seats {
            for b in a + 1..seats {
                let mut p = profile.clone();
                p.seed = p.seed.wrapping_add((a * seats + b) as u64 * 1_000_003);
                lines.push(Line::new(p));
            }
        }
        Rc::new(RefCell::new(Net {
            lines,
            seats,
            now: 0,
            seq: 0,
            queue: BinaryHeap::new(),
            inbox: vec![VecDeque::new(); seats],
            last: vec![vec![0; seats]; seats],
        }))
    }

    pub fn endpoint(net: &Rc<RefCell<Net>>, seat: usize) -> Endpoint {
        Endpoint {
            net: net.clone(),
            seat,
        }
    }

    /// Move time to `t`, µs, and deliver what has arrived by then.
    pub fn advance_to(&mut self, t: u64) {
        self.now = self.now.max(t);
        while let Some(Reverse((at, ..))) = self.queue.peek() {
            if *at > self.now {
                break;
            }
            let Reverse((_, _, from, to, payload)) = self.queue.pop().unwrap();
            self.inbox[to].push_back((from, payload));
        }
    }

    fn line(&mut self, from: usize, to: usize) -> (&mut Line, usize) {
        let (a, b) = (from.min(to), from.max(to));
        let n = self.seats;
        // Pairs (a, b) in order: a's row starts after the rows before it.
        let i = a * n - a * (a + 1) / 2 + (b - a - 1);
        (&mut self.lines[i], usize::from(to > from))
    }

    fn send(&mut self, from: usize, to: usize, how: Delivery, payload: Vec<u8>) {
        let now = self.now;
        let (line, out) = self.line(from, to);
        let dues = match how {
            Delivery::Unreliable => line.route(now, out),
            Delivery::Reliable => {
                let mut sent = now;
                let due = loop {
                    if let Some(&d) = line.route(sent, out).first() {
                        break d;
                    }
                    sent += RESEND_US;
                };
                let due = due.max(self.last[from][to]);
                self.last[from][to] = due;
                vec![due]
            }
        };
        for due in dues {
            self.seq += 1;
            self.queue
                .push(Reverse((due, self.seq, from, to, payload.clone())));
        }
    }
}

/// A seat's end of the [`Net`].
pub struct Endpoint {
    net: Rc<RefCell<Net>>,
    seat: usize,
}

impl Transport for Endpoint {
    fn send(&mut self, to: To, how: Delivery, payload: Vec<u8>) {
        let mut net = self.net.borrow_mut();
        match to {
            To::Seat(s) => net.send(self.seat, s, how, payload),
            To::Everyone => {
                for s in (0..net.seats).filter(|&s| s != self.seat) {
                    net.send(self.seat, s, how, payload.clone());
                }
            }
        }
    }

    fn recv(&mut self) -> Option<(usize, Vec<u8>)> {
        self.net.borrow_mut().inbox[self.seat].pop_front()
    }
}

/// What paces a peer's frames, as `gbaa-desktop`'s netplay loop has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    /// A window with sound: the device takes the sound at a steady rate,
    /// and the next frame runs once the queue has room, so frames late
    /// for one reason are caught up after.
    Audio,
    /// A window without sound (`--mute`, or no device): a frame clock.
    Muted,
    /// Headless `--paced`: a frame clock, no window.
    Paced,
}

/// The desktop's netplay loop before this unit's fixes, or after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// Until 2026-11-08. A frame held back slept a frame, and the sound's
    /// pacing then caught that frame up at once, so the hold was undone
    /// (and time sync held again, and again); a muted window slept a
    /// frame after every frame on top of its clock (a frame took its cost
    /// plus a frame); clocks never caught a late frame up.
    Before,
    /// Since: a hold puts a frame's silence in the sound queue (or moves
    /// the clock a frame on), so the peer really falls back a frame; every
    /// pacing catches a late frame up, as sound does.
    After,
}

/// A peer's front end and display.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub pacing: Pacing,
    pub version: Version,
    /// The display's refresh, µs.
    pub refresh_us: f64,
    /// When its first refresh comes, µs.
    pub phase_us: f64,
    /// A frame goes to the window this long after its turn began, or when
    /// done if later (`None`: when done). Frames then reach the display
    /// on the pacer's even beat instead of their costs' uneven one.
    pub present_us: Option<f64>,
}

impl Window {
    pub fn new(pacing: Pacing, version: Version, seat: usize) -> Window {
        Window {
            pacing,
            version,
            refresh_us: REFRESH_US,
            // Each display on a phase of its own.
            phase_us: 3_100.0 + 5_300.0 * seat as f64,
            present_us: (version == Version::After).then_some(PRESENT_US),
        }
    }
}

/// What a peer's turns cost, µs: one frame of all its consoles, one run
/// again (not drawn), a snapshot, a restore; each frame's cost varied by
/// up to `noise` of it either way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Costs {
    pub step_us: f64,
    pub rerun_us: f64,
    pub snapshot_us: f64,
    pub restore_us: f64,
    pub noise: f64,
}

impl Costs {
    /// Two Kirbys on quack, recompiled (docs/research/multiplayer.md, "On
    /// recompiled code": 3.5 ms p50 a frame; a snapshot 0.35 ms, a restore
    /// 0.7 ms), a frame run again undrawn a little cheaper.
    pub const KIRBY_TWO: Costs = Costs {
        step_us: 3_500.0,
        rerun_us: 3_000.0,
        snapshot_us: 350.0,
        restore_us: 700.0,
        noise: 0.3,
    };

    /// The same, scaled: a slower machine (the Deck, about 2.2x), more
    /// consoles.
    pub fn scaled(self, k: f64) -> Costs {
        Costs {
            step_us: self.step_us * k,
            rerun_us: self.rerun_us * k,
            snapshot_us: self.snapshot_us * k,
            restore_us: self.restore_us * k,
            noise: self.noise,
        }
    }
}

/// A game's own costs, frame by frame, measured once on a straight run
/// of its consoles (`gbaa-pingsim feel --rom`), so that every version of
/// the loop is run on the same costs, however busy the machine was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    /// Frame `f` of every console, µs, only the shown seat's picture drawn.
    pub shown: Vec<f64>,
    /// The same, every seat's picture drawn ([`Version::Before`]).
    pub all: Vec<f64>,
    /// The same, none drawn: a frame rollback runs again.
    pub undrawn: Vec<f64>,
    pub snapshot_us: f64,
    pub restore_us: f64,
}

impl Trace {
    fn at(v: &[f64], f: u32) -> f64 {
        // Past its end, the trace repeats its last minute.
        let n = v.len();
        let f = f as usize;
        v[if f < n {
            f
        } else {
            n - 3600.min(n) + (f - n) % 3600.min(n)
        }]
    }

    /// A text file: a header line, then `frame shown all undrawn` lines.
    pub fn write(&self) -> String {
        let mut out = format!(
            "# feel trace: snapshot_us {} restore_us {}\n",
            self.snapshot_us, self.restore_us
        );
        for f in 0..self.shown.len() {
            out += &format!(
                "{f} {:.1} {:.1} {:.1}\n",
                self.shown[f], self.all[f], self.undrawn[f]
            );
        }
        out
    }

    pub fn read(text: &str) -> Result<Trace, String> {
        let mut t = Trace::default();
        for (i, l) in text.lines().enumerate() {
            let w: Vec<&str> = l.split_whitespace().collect();
            let num = |s: &str| {
                s.parse::<f64>()
                    .map_err(|_| format!("line {}: {s}?", i + 1))
            };
            match w.as_slice() {
                ["#", "feel", "trace:", "snapshot_us", s, "restore_us", r] => {
                    t.snapshot_us = num(s)?;
                    t.restore_us = num(r)?;
                }
                [_, a, b, c] => {
                    t.shown.push(num(a)?);
                    t.all.push(num(b)?);
                    t.undrawn.push(num(c)?);
                }
                _ => return Err(format!("line {}: {l:?}?", i + 1)),
            }
        }
        if t.shown.is_empty() {
            return Err("an empty trace".into());
        }
        Ok(t)
    }
}

/// Where a turn's cost comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cost<'a> {
    /// From what the turn did, by [`Costs`]: deterministic.
    Model(Costs),
    /// From what the turn did, frame by frame, by a game's [`Trace`],
    /// times `scale` (a slower machine): deterministic too.
    Trace(&'a Trace, f64),
}

impl Cost<'_> {
    /// A turn that ran frame `frame` (`stepped`), first running `rerun`
    /// frames again for `rolled` rollbacks; `r` a random number in 0..1.
    fn of(
        self,
        version: Version,
        frame: u32,
        stepped: bool,
        rerun: u64,
        rolled: u64,
        r: f64,
    ) -> f64 {
        let n = u64::from(stepped);
        match self {
            Cost::Model(m) => {
                let wobble = 1.0 + m.noise * (2.0 * r - 1.0);
                (m.step_us * n as f64 + m.rerun_us * rerun as f64) * wobble
                    + m.snapshot_us * (n + rerun) as f64
                    + m.restore_us * rolled as f64
            }
            Cost::Trace(t, scale) => {
                let drawn = match version {
                    Version::Before => &t.all,
                    Version::After => &t.shown,
                };
                let again: f64 = (frame.saturating_sub(rerun as u32)..frame)
                    .map(|g| Trace::at(&t.undrawn, g))
                    .sum();
                let step = if stepped {
                    Trace::at(drawn, frame)
                } else {
                    0.0
                };
                scale
                    * (step
                        + again
                        + t.snapshot_us * (n + rerun) as f64
                        + t.restore_us * rolled as f64)
            }
        }
    }
}

/// What one player saw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feel {
    pub seat: usize,
    pub frames: u32,
    /// Refreshes from this peer's first frame shown to its last.
    pub refreshes: u64,
    pub late: u64,
    pub doubled: u64,
    pub skipped: u64,
    /// Doubles the 60 Hz display makes of 59.73 Hz frames anyway: what
    /// `doubled` would be with no network at all.
    pub beat: u64,
    pub rollbacks: u64,
    pub rerun: u64,
    pub deepest: u32,
    /// The 99th percentile of a rollback's depth, frames.
    pub depth_p99: u32,
    pub holds: u64,
    pub stalls: u64,
    /// Times the sound queue ran dry ([`Pacing::Audio`]).
    pub underruns: u64,
    /// Shown frames on which the other player jumped, and the biggest
    /// jump, frames of their motion.
    pub jumps: u64,
    pub jump_max: u32,
    /// Jumps of 4 frames or more: a teleport a player sees.
    pub big_jumps: u64,
    /// From a frame being due to its first refresh on screen, mean and
    /// 99th percentile, µs: what a press takes to show, less the input's
    /// own wait for the frame.
    pub lag_us: f64,
    pub lag_p99_us: f64,
    /// Turn costs, µs: median, 99th percentile, most.
    pub turn_p50_us: f64,
    pub turn_p99_us: f64,
    pub turn_max_us: f64,
    /// The end hash, to check the peers agree.
    pub end: u64,
}

impl Feel {
    /// One line of numbers, as `gbaa-pingsim feel` prints them.
    pub fn line(&self) -> String {
        format!(
            "seat {}: {} frames, {} late, {} doubled ({} the display's beat), {} skipped, \
             {} rollbacks, {} run again, depth p99 {} most {}, {} held back, {} stalls, \
             {} underruns, {} jumps ({} of 4+ frames, most {}), shown {:.1} ms after due \
             (p99 {:.1}), turn p50 {:.1} p99 {:.1} \
             most {:.1} ms, end {:016x}",
            self.seat + 1,
            self.frames,
            self.late,
            self.doubled,
            self.beat,
            self.skipped,
            self.rollbacks,
            self.rerun,
            self.depth_p99,
            self.deepest,
            self.holds,
            self.stalls,
            self.underruns,
            self.jumps,
            self.big_jumps,
            self.jump_max,
            self.lag_us / 1e3,
            self.lag_p99_us / 1e3,
            self.turn_p50_us / 1e3,
            self.turn_p99_us / 1e3,
            self.turn_max_us / 1e3,
            self.end,
        )
    }
}

/// A frame handed to the window: when, and the biggest rewrite of the
/// other players' frames since the last one (frames).
#[derive(Clone, Copy, Debug)]
struct Handoff {
    at: f64,
    due: f64,
    rewrote: u32,
}

/// One peer's front end as the sim runs it.
struct Front {
    w: Window,
    next: f64,
    /// The pacer's frame count and origin: frame k is due at
    /// `origin + k * GBA_FRAME_US` (catching up).
    k: f64,
    origin: Option<f64>,
    /// A plain clock's next tick (no catching up).
    tick: Option<f64>,
    handoffs: Vec<Handoff>,
    rewrote: u32,
    late: u64,
    underruns: u64,
    depths: Vec<u32>,
    turns: Vec<f64>,
    rng: u64,
}

impl Front {
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// When the pacer lets the next frame start, catching up.
    fn due(&self) -> f64 {
        self.origin.map_or(0.0, |o| o + self.k * GBA_FRAME_US)
    }

    /// A frame (or, `silent`, a hold's frame of silence) went into the
    /// pacer at `end`: when the next turn may start.
    fn catch_up(&mut self, end: f64, audio: bool) -> f64 {
        let o = *self
            .origin
            .get_or_insert(end - GBA_FRAME_US * (self.k + 1.0));
        self.k += 1.0;
        let due = o + self.k * GBA_FRAME_US;
        // The sound queue (or a catching-up clock's slack) holds so much:
        // later than that and it ran dry, and the schedule moves on.
        if end > due + AUDIO_TARGET_US {
            if audio {
                self.underruns += 1;
            }
            self.origin = Some(end - AUDIO_TARGET_US - self.k * GBA_FRAME_US);
        }
        end.max(self.due())
    }

    /// A plain clock's wait, as `fast::Clock`: return a period after the
    /// last return, or now if that's past.
    fn clock(&mut self, now: f64) -> f64 {
        let at = self.tick.map_or(now, |t| t.max(now));
        self.tick = Some(at + GBA_FRAME_US);
        at
    }
}

/// Run `peers` (seat order) on `net` until each has run `frames`, each
/// driven as its window says, `input(seat, frame)` what its player holds.
/// Gives up after a simulated hour.
pub fn run<C: Snapshot>(
    peers: &mut [Rollback<C, Endpoint>],
    net: &Rc<RefCell<Net>>,
    windows: &[Window],
    cost: Cost,
    frames: u32,
    mut input: impl FnMut(usize, u32) -> C::Input,
) -> Result<Vec<Feel>, Error> {
    let mut fronts: Vec<Front> = windows
        .iter()
        .enumerate()
        .map(|(s, &w)| Front {
            w,
            // The joiners a little after the host.
            next: 7_300.0 * s as f64,
            k: 0.0,
            origin: None,
            tick: None,
            handoffs: Vec::new(),
            rewrote: 0,
            late: 0,
            underruns: 0,
            depths: Vec::new(),
            turns: Vec::new(),
            rng: 0x5EED_0000 + s as u64 * 7919,
        })
        .collect();
    loop {
        let live: Vec<usize> = (0..peers.len())
            .filter(|&s| peers[s].frame() < frames)
            .collect();
        let Some(&s) = live
            .iter()
            .min_by(|&&a, &&b| fronts[a].next.total_cmp(&fronts[b].next))
        else {
            break;
        };
        let t = fronts[s].next;
        if t > 3.6e9 {
            return Err(Error::Console(format!(
                "stalled: frames {:?}",
                peers.iter().map(|p| p.frame()).collect::<Vec<_>>()
            )));
        }
        net.borrow_mut().advance_to(t as u64);
        // The peers done still answer, as the desktop's do at the end.
        for (d, p) in peers.iter_mut().enumerate() {
            if d != s && p.frame() >= frames {
                p.pump()?;
            }
        }
        let p = &mut peers[s];
        let f = &mut fronts[s];
        let before = p.stats;
        let frame = p.frame();
        let status = p.advance(input(s, p.frame()))?;
        let after = p.stats;
        let rolled = after.rollbacks - before.rollbacks;
        let rerun = after.rerun - before.rerun;
        if let Some(depth) = rerun.checked_div(rolled) {
            f.depths.extend((0..rolled).map(|_| depth as u32));
            f.rewrote = f.rewrote.max(rerun as u32);
        }
        let stepped = status == Status::Stepped;
        let r = f.random();
        let c = cost.of(f.w.version, frame, stepped, rerun, rolled, r);
        let end = t + c;
        let audio = f.w.pacing == Pacing::Audio;
        let after_fix = f.w.version == Version::After;
        f.next = match status {
            Status::Stepped => {
                f.turns.push(c);
                // Due: when the pacer let this frame start.
                let due = match (f.w.pacing, f.w.version) {
                    (Pacing::Audio, _) | (_, Version::After) if f.origin.is_some() => {
                        f.due().min(t)
                    }
                    _ => t,
                };
                // As the desktop does it: so long after the turn began.
                let at = f.w.present_us.map_or(end, |l| end.max(t + l));
                f.handoffs.push(Handoff {
                    at,
                    due,
                    rewrote: std::mem::take(&mut f.rewrote),
                });
                if f.origin.is_some() && end - due > GBA_FRAME_US {
                    f.late += 1;
                }
                match (f.w.pacing, f.w.version) {
                    (Pacing::Audio, _) | (_, Version::After) => f.catch_up(end, audio),
                    (Pacing::Muted, Version::Before) => {
                        f.origin.get_or_insert(end);
                        // A frame's sleep, then the clock (never later).
                        let at = end + GBA_FRAME_US;
                        f.clock(at)
                    }
                    (Pacing::Paced, Version::Before) => {
                        f.origin.get_or_insert(end);
                        f.clock(end)
                    }
                }
            }
            Status::Skipped => match (f.w.pacing, after_fix) {
                (_, true) => f.catch_up(end, false),
                (Pacing::Audio, false) => end + GBA_FRAME_US,
                (Pacing::Muted, false) => {
                    let at = end + GBA_FRAME_US;
                    f.clock(at)
                }
                (Pacing::Paced, false) => f.clock(end),
            },
            Status::Waiting | Status::Starting => end + RETRY_US,
        };
    }
    // Let the last inputs and hashes land.
    for _ in 0..2000 {
        let t = net.borrow().now + 1_000;
        net.borrow_mut().advance_to(t);
        for p in peers.iter_mut() {
            p.pump()?;
        }
        if peers.iter().all(|p| p.settled()) {
            break;
        }
    }
    let mut out = Vec::new();
    for (s, (p, f)) in peers.iter_mut().zip(&mut fronts).enumerate() {
        out.push(feel(s, p, f));
    }
    Ok(out)
}

fn percentile<T: Copy + PartialOrd>(v: &mut [T], q: f64) -> Option<T> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    Some(v[i])
}

fn feel<C: Snapshot>(seat: usize, p: &mut Rollback<C, Endpoint>, f: &mut Front) -> Feel {
    let st = p.stats;
    let mut out = Feel {
        seat,
        frames: p.frame(),
        late: f.late,
        rollbacks: st.rollbacks,
        rerun: st.rerun,
        deepest: st.deepest,
        depth_p99: percentile(&mut f.depths, 0.99).unwrap_or(0),
        holds: st.skips,
        stalls: st.stalls,
        underruns: f.underruns,
        turn_p50_us: percentile(&mut f.turns, 0.5).unwrap_or(0.0),
        turn_p99_us: percentile(&mut f.turns, 0.99).unwrap_or(0.0),
        turn_max_us: f.turns.iter().copied().fold(0.0, f64::max),
        end: p.console_mut().hash(),
        ..Feel::default()
    };
    let h = &f.handoffs;
    let (Some(first), Some(last)) = (h.first(), h.last()) else {
        return out;
    };
    // The refreshes from the first frame's to the last's.
    let w = f.w;
    let mut n = ((first.at + DRAW_US - w.phase_us) / w.refresh_us)
        .ceil()
        .max(0.0) as u64;
    let mut i = 0; // handoffs on screen so far
    let mut shown: Option<usize> = None;
    let mut pending: u32 = 0;
    let mut lags = Vec::new();
    loop {
        let v = w.phase_us + n as f64 * w.refresh_us;
        if v - DRAW_US > last.at + w.refresh_us {
            break;
        }
        let mut newest = None;
        while i < h.len() && h[i].at <= v - DRAW_US {
            pending = pending.max(h[i].rewrote);
            lags.push(v - h[i].due);
            newest = Some(i);
            i += 1;
        }
        out.refreshes += 1;
        match (newest, shown) {
            (None, Some(_)) => out.doubled += 1,
            (Some(x), prev) => {
                let from = prev.map_or(x, |p| p + 1);
                out.skipped += (x - from) as u64;
                if pending > 0 {
                    out.jumps += 1;
                    out.jump_max = out.jump_max.max(pending);
                    if pending >= 4 {
                        out.big_jumps += 1;
                    }
                }
                pending = 0;
                shown = Some(x);
            }
            (None, None) => {}
        }
        n += 1;
    }
    // A skipped frame was never on screen: its lag is the next one's.
    out.lag_us = lags.iter().sum::<f64>() / lags.len().max(1) as f64;
    out.lag_p99_us = percentile(&mut lags, 0.99).unwrap_or(0.0);
    let span = last.at - first.at;
    out.beat = (span / w.refresh_us - span / GBA_FRAME_US).max(0.0).round() as u64;
    out
}

/// Feel through `profile` with `players` seats of the toy console, each
/// pressing what `scripts[seat]` says, every peer's window as `window`
/// gives (by seat), its turns costing what `cost` says; `frames` frames. The
/// test's and `gbaa-pingsim feel --console toy`'s run.
pub fn toy(
    profile: &Profile,
    scripts: &[crate::script::Script],
    window: impl Fn(usize) -> Window,
    cost: Cost,
    frames: u32,
) -> Result<Vec<Feel>, Error> {
    let players = scripts.len();
    let net = Net::new(players, profile);
    let mut peers: Vec<_> = (0..players)
        .map(|s| {
            Rollback::new(
                crate::demo::Toy::peer(players, s),
                Net::endpoint(&net, s),
                s,
                (s == 0).then(crate::Config::default),
                crate::rollback::WINDOW,
            )
        })
        .collect();
    let windows: Vec<Window> = (0..players).map(window).collect();
    run(&mut peers, &net, &windows, cost, frames, |s, f| {
        scripts[s].keys(f as u64)
    })
}

/// The presses of the lag report's Kirby runs (docs/research/multiplayer.md,
/// "What made Kirby laggy"), from frame 0: both players moving and
/// jumping, player 3 and 4 (four seats) on their own rhythm.
pub const PLAY: [&str; 4] = [
    "right@0-3600/90+40,a@10-3600/50+8",
    "left@20-3600/70+30,a@30-3600/45+10,b@100-3600/120+5",
    "right@5-3600/80+35,a@40-3600/55+9",
    "left@15-3600/65+25,a@25-3600/60+7",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Script;

    fn scripts(n: usize) -> Vec<Script> {
        PLAY[..n]
            .iter()
            .map(|s| Script::parse(s).unwrap())
            .collect()
    }

    fn profile(name: &str) -> Profile {
        let text = match name {
            "calm" => include_str!("../profiles/calm.toml"),
            "wander" => include_str!("../profiles/wander.toml"),
            "spiky" => include_str!("../profiles/spiky.toml"),
            "wifi" => include_str!("../profiles/wifi.toml"),
            "fly" => include_str!("../profiles/fly.toml"),
            _ => unreachable!(),
        };
        Profile::parse(text).unwrap()
    }

    /// The most a minute of play may show on any seat
    /// (docs/research/multiplayer.md, "Feel": the numbers these were set
    /// over, after the 2026-11-08 fixes, and before).
    struct Budget {
        profile: &'static str,
        players: usize,
        late: u64,
        doubled: u64,
        skipped: u64,
        depth_p99: u32,
        holds: u64,
        big_jumps: u64,
        lag_ms: f64,
    }

    const BUDGETS: [Budget; 5] = [
        Budget {
            profile: "calm",
            players: 2,
            late: 5,
            doubled: 80,
            skipped: 70,
            depth_p99: 3,
            holds: 5,
            big_jumps: 0,
            lag_ms: 20.0,
        },
        Budget {
            profile: "wifi",
            players: 2,
            late: 15,
            doubled: 45,
            skipped: 25,
            depth_p99: 5,
            holds: 6,
            big_jumps: 15,
            lag_ms: 20.0,
        },
        Budget {
            profile: "fly",
            players: 2,
            late: 60,
            doubled: 130,
            skipped: 100,
            depth_p99: 7,
            holds: 15,
            big_jumps: 20,
            lag_ms: 20.0,
        },
        Budget {
            profile: "wander",
            players: 2,
            late: 300,
            doubled: 300,
            skipped: 280,
            depth_p99: 17,
            holds: 20,
            big_jumps: 250,
            lag_ms: 22.0,
        },
        Budget {
            profile: "wifi",
            players: 4,
            late: 450,
            doubled: 420,
            skipped: 380,
            depth_p99: 5,
            holds: 40,
            big_jumps: 25,
            lag_ms: 22.0,
        },
    ];

    /// What `b` doesn't allow of `f`, if anything.
    fn over(b: &Budget, f: &Feel) -> Vec<String> {
        let mut out = Vec::new();
        let mut check = |what: &str, got: f64, most: f64| {
            if got > most {
                out.push(format!("{what} {got} > {most}"));
            }
        };
        check("late", f.late as f64, b.late as f64);
        check("doubled", f.doubled as f64, b.doubled as f64);
        check("skipped", f.skipped as f64, b.skipped as f64);
        check("depth p99", f.depth_p99 as f64, b.depth_p99 as f64);
        check("held back", f.holds as f64, b.holds as f64);
        check("jumps of 4+", f.big_jumps as f64, b.big_jumps as f64);
        check("ms shown after due", f.lag_us / 1e3, b.lag_ms);
        out
    }

    fn minute(b: &Budget, pacing: Pacing, version: Version) -> Vec<Feel> {
        let costs = Costs::KIRBY_TWO.scaled(b.players as f64 / 2.0);
        let window = |s| Window::new(pacing, version, s);
        let cost = Cost::Model(costs);
        let f = toy(&profile(b.profile), &scripts(b.players), window, cost, 3600).unwrap();
        assert!(f.iter().all(|p| p.end == f[0].end), "the peers ended apart");
        assert!(f.iter().all(|p| p.stalls == 0), "{f:?}");
        f
    }

    #[test]
    fn every_window_stays_in_its_budgets() {
        let mut failed = Vec::new();
        for b in &BUDGETS {
            for pacing in [Pacing::Audio, Pacing::Muted, Pacing::Paced] {
                for f in minute(b, pacing, Version::After) {
                    let over = over(b, &f);
                    if !over.is_empty() {
                        failed.push(format!(
                            "{} x{} {pacing:?}: {}\n    {}",
                            b.profile,
                            b.players,
                            over.join(", "),
                            f.line()
                        ));
                    }
                }
            }
        }
        assert!(failed.is_empty(), "over budget:\n  {}", failed.join("\n  "));
    }

    /// The budgets catch what june felt: the loop before the fixes
    /// breaks them on fly, with sound and muted alike.
    #[test]
    fn the_old_loop_breaks_the_budgets() {
        let fly = &BUDGETS[2];
        for pacing in [Pacing::Audio, Pacing::Muted] {
            let f = minute(fly, pacing, Version::Before);
            assert!(
                f.iter().any(|p| !over(fly, p).is_empty()),
                "{pacing:?}: {f:?}"
            );
        }
    }

    #[test]
    fn a_session_with_no_network_feels_like_playing_alone() {
        let f = toy(
            &Profile::default(),
            &scripts(2),
            |s| Window::new(Pacing::Audio, Version::After, s),
            Cost::Model(Costs::KIRBY_TWO),
            1800,
        )
        .unwrap();
        for p in &f {
            // At delay 0 the peer a few ms ahead runs each frame before
            // the other's input for it exists: a frame's rollback at most.
            assert!(p.deepest <= 1, "{}", p.line());
            assert_eq!(p.late, 0, "{}", p.line());
            assert_eq!(p.holds, 0, "{}", p.line());
        }
        assert_eq!(f[0].end, f[1].end);
    }

    #[test]
    fn runs_repeat_exactly() {
        let run = || {
            toy(
                &profile("wander"),
                &scripts(2),
                |s| Window::new(Pacing::Audio, Version::After, s),
                Cost::Model(Costs::KIRBY_TWO),
                900,
            )
            .unwrap()
        };
        assert_eq!(run(), run());
    }
}
