//! When a single-player frame goes to the window (deck/even-presents),
//! and a sim of what a 60 Hz display then shows.
//!
//! The pacer (the sound, or a frame clock when muted) starts each frame
//! on an even 59.73 Hz beat. Handed to the window the moment it's done, a
//! frame reaches the display at its start plus its cost, so its time
//! against the display's refresh moves with the cost: a frame whose finish
//! crosses a refresh the one before didn't makes a doubled frame (a
//! refresh with nothing new) and a skipped one (two new between
//! refreshes). Held back until a set time after its start, frames arrive
//! on the beat and only the beat's own doubles are left (60 against 59.73
//! Hz: one every 3.7 s, 16 a minute). Netplay found the same and presents
//! 8 ms into the turn (multiplayer/rollback-feel-sim).
//!
//! A single game's costs vary much less than a rollback session's, so
//! [`Present::Slowest`] holds a frame only as long as the slowest of the
//! last [`WINDOW`] frames took (plus a margin), not a fixed 8 ms: on a
//! fast build that's a fraction of a millisecond of display lag, and on a
//! slow one the frames are still even. [`sim`] measures all three on a
//! game's own costs (`gbaa-desktop --headless --frame-times`), in
//! simulated time; `gbaa-present-sim` prints them (docs/tools/commands.md).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A GBA frame, µs: 280896 cycles at 2^24 Hz (59.73 Hz).
pub const GBA_FRAME_US: f64 = 280_896.0 / 16_777_216.0 * 1e6;
/// A display refresh at 60 Hz, µs.
pub const REFRESH_US: f64 = 1e6 / 60.0;
/// How long before a refresh a frame must reach the window to be on it
/// (as `netplay::feel`'s).
pub const DRAW_US: f64 = 1_000.0;
/// The frames [`Present::Slowest`] looks back over: about a second.
pub const WINDOW: usize = 60;
/// What [`Present::Slowest`] adds to the slowest frame, µs: a frame a
/// little slower than any lately still goes on the beat.
pub const MARGIN_US: f64 = 500.0;
/// The most [`Present::Slowest`] holds a frame, µs (netplay's 8 ms): a
/// slower frame goes when done.
pub const MOST_US: f64 = 8_000.0;

/// When a frame goes to the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Present {
    /// As soon as it's done (the loop before deck/even-presents).
    Done,
    /// This many µs after its start, or when done if later (netplay's).
    Fixed(f64),
    /// The slowest of the last [`WINDOW`] frames' costs plus
    /// [`MARGIN_US`] after its start, at most [`MOST_US`], or when done
    /// if later: the window's.
    Slowest,
}

/// [`Present::Slowest`]'s memory of recent costs.
#[derive(Clone, Debug, Default)]
pub struct Holder {
    costs: VecDeque<f64>,
}

impl Holder {
    /// How long after its start to hand over a frame that cost `cost_us`
    /// (µs), as `p` says; remembers the cost.
    pub fn hold_us(&mut self, p: Present, cost_us: f64) -> f64 {
        if self.costs.len() == WINDOW {
            self.costs.pop_front();
        }
        self.costs.push_back(cost_us);
        match p {
            Present::Done => 0.0,
            Present::Fixed(us) => us,
            Present::Slowest => {
                let slowest = self.costs.iter().copied().fold(0.0, f64::max);
                (slowest + MARGIN_US).min(MOST_US)
            }
        }
    }

    /// Start afresh (after a pause, fast-forward or rewind).
    pub fn reset(&mut self) {
        self.costs.clear();
    }
}

/// The window's side: when each frame's turn began, and the hold before
/// its picture goes to the window ([`Present::Slowest`]).
#[derive(Clone, Debug, Default)]
pub struct Hold {
    turn: Option<Instant>,
    holder: Holder,
    /// The holds' sleeps, in all.
    pub slept: Duration,
}

impl Hold {
    /// The frame is done: wait until its picture is due at the window.
    /// Nothing for the first frame after [`Hold::reset`].
    pub fn before_show(&mut self) {
        let Some(turn) = self.turn else {
            return;
        };
        let now = Instant::now();
        let cost = (now - turn).as_secs_f64() * 1e6;
        let at = turn + Duration::from_secs_f64(self.holder.hold_us(Present::Slowest, cost) / 1e6);
        if at > now {
            self.slept += at - now;
            std::thread::sleep(at - now);
        }
    }

    /// The pacer let the next frame start.
    pub fn started(&mut self) {
        self.turn = Some(Instant::now());
    }

    /// Start afresh: a pause, fast-forward or rewind came between.
    pub fn reset(&mut self) {
        self.turn = None;
        self.holder.reset();
    }
}

/// What paces the frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pacer {
    /// The sound: the device plays at a steady rate, so a frame late for
    /// any reason lets the next ones run at once (the queue catches up).
    Sound,
    /// The muted window's [`crate::fast::Clock`]: a late frame moves the
    /// beat on.
    Clock,
}

/// What the display showed over a run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub frames: u64,
    pub refreshes: u64,
    /// Refreshes showing what the one before showed.
    pub doubled: u64,
    /// Frames handed over but never on screen.
    pub skipped: u64,
    /// Frames done more than a frame after their start on the beat.
    pub late: u64,
    /// The doubled frames 60 Hz against 59.73 makes anyway.
    pub beat: u64,
    /// From a frame's start to its first refresh on screen, mean, µs.
    pub lag_us: f64,
}

impl Shown {
    /// `n` a minute of GBA frames.
    pub fn per_minute(&self, n: u64) -> f64 {
        n as f64 * 60.0 * 1e6 / GBA_FRAME_US / self.frames.max(1) as f64
    }
}

/// Run frames costing `costs` (µs each) through `pacer`, handed to the
/// window as `present` says, to a 60 Hz display whose first refresh is at
/// `phase_us`; what it showed.
pub fn sim(costs: &[f64], pacer: Pacer, present: Present, phase_us: f64) -> Shown {
    let mut holder = Holder::default();
    // (handed over, started)
    let mut handed = Vec::with_capacity(costs.len());
    let mut late = 0;
    let mut next = 0.0f64; // the pacer's next start
    let mut free = 0.0f64; // when the last frame was handed over
    for &c in costs {
        let start = next.max(free);
        let done = start + c;
        if done > next + GBA_FRAME_US {
            late += 1;
        }
        let at = done.max(start + holder.hold_us(present, c));
        handed.push((at, start));
        free = at;
        next = match pacer {
            Pacer::Sound => next + GBA_FRAME_US,
            Pacer::Clock => start + GBA_FRAME_US,
        };
    }
    let mut out = Shown {
        frames: costs.len() as u64,
        late,
        ..Shown::default()
    };
    let (Some(first), Some(last)) = (handed.first(), handed.last()) else {
        return out;
    };
    let mut n = ((first.0 + DRAW_US - phase_us) / REFRESH_US)
        .ceil()
        .max(0.0) as u64;
    let mut i = 0;
    let mut shown: Option<usize> = None;
    let mut lags = 0.0;
    let mut on = 0u64;
    loop {
        let v = phase_us + n as f64 * REFRESH_US;
        if v - DRAW_US > last.0 + REFRESH_US {
            break;
        }
        let mut newest = None;
        while i < handed.len() && handed[i].0 <= v - DRAW_US {
            newest = Some(i);
            i += 1;
        }
        out.refreshes += 1;
        match (newest, shown) {
            (None, Some(_)) => out.doubled += 1,
            (Some(x), prev) => {
                out.skipped += (x - prev.map_or(x, |p| p + 1)) as u64;
                lags += v - handed[x].1;
                on += 1;
                shown = Some(x);
            }
            (None, None) => {}
        }
        n += 1;
    }
    out.lag_us = lags / on.max(1) as f64;
    let span = last.0 - first.0;
    out.beat = (span / REFRESH_US - span / GBA_FRAME_US).max(0.0).round() as u64;
    out
}

/// [`sim`] averaged over `phases` display phases spread across a refresh,
/// so where the beat falls doesn't decide the count.
pub fn sim_phases(costs: &[f64], pacer: Pacer, present: Present, phases: u32) -> Shown {
    let mut sum = Shown::default();
    for k in 0..phases {
        let s = sim(costs, pacer, present, REFRESH_US * k as f64 / phases as f64);
        sum.frames += s.frames;
        sum.refreshes += s.refreshes;
        sum.doubled += s.doubled;
        sum.skipped += s.skipped;
        sum.late += s.late;
        sum.beat += s.beat;
        sum.lag_us += s.lag_us / phases as f64;
    }
    sum
}

/// Read a `--frame-times` file: `frame N us T` lines, the costs in order.
pub fn read_times(text: &str) -> Result<Vec<f64>, String> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.split_whitespace()
                .nth(3)
                .and_then(|t| t.parse().ok())
                .ok_or(format!("bad frame-times line {l:?}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic jitter: costs `base` ± `spread` µs.
    fn jittery(n: usize, base: f64, spread: f64) -> Vec<f64> {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                base + spread * ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0)
            })
            .collect()
    }

    const MINUTE: usize = 3584;

    #[test]
    fn even_costs_show_only_the_beat() {
        let costs = vec![3_000.0; MINUTE];
        for p in [Present::Done, Present::Slowest] {
            let s = sim_phases(&costs, Pacer::Sound, p, 8);
            assert_eq!(s.skipped, 0, "{p:?}: {s:?}");
            assert!(s.doubled <= s.beat + 8, "{p:?}: {s:?}");
        }
    }

    #[test]
    fn holding_frames_takes_off_the_judder() {
        // About Kirby's interpreted frame-to-frame swing (deck/even-presents).
        let costs = jittery(MINUTE, 4_000.0, 400.0);
        let before = sim_phases(&costs, Pacer::Sound, Present::Done, 8);
        let after = sim_phases(&costs, Pacer::Sound, Present::Slowest, 8);
        // Per phase: the beat's 16, and before about 20 more.
        assert!(before.doubled > before.beat + 8 * 10, "{before:?}");
        assert!(after.doubled <= after.beat + 8, "{after:?}");
        // At the cost of under a millisecond of lag, not netplay's 8 ms.
        assert!(
            after.lag_us - before.lag_us < 1_000.0,
            "{before:?} {after:?}"
        );
    }

    #[test]
    fn a_muted_window_too() {
        let costs = jittery(MINUTE, 4_000.0, 400.0);
        let before = sim_phases(&costs, Pacer::Clock, Present::Done, 8);
        let after = sim_phases(&costs, Pacer::Clock, Present::Slowest, 8);
        assert!(before.doubled > before.beat + 8 * 10, "{before:?}");
        assert!(after.doubled <= after.beat + 8, "{after:?}");
    }

    #[test]
    fn holds_no_more_than_netplay() {
        let mut h = Holder::default();
        assert_eq!(h.hold_us(Present::Slowest, 2_000.0), 2_500.0);
        assert_eq!(h.hold_us(Present::Slowest, 1_000.0), 2_500.0);
        assert_eq!(h.hold_us(Present::Slowest, 12_000.0), MOST_US);
        h.reset();
        assert_eq!(h.hold_us(Present::Slowest, 1_000.0), 1_500.0);
        assert_eq!(h.hold_us(Present::Done, 1_000.0), 0.0);
    }

    #[test]
    fn the_window_holds_a_quick_frame_as_long_as_a_slow_one() {
        let mut h = Hold::default();
        h.before_show(); // no turn yet: no hold
        assert_eq!(h.slept, Duration::ZERO);
        h.started();
        std::thread::sleep(Duration::from_millis(3));
        h.before_show();
        let first = h.slept;
        assert!(
            first <= Duration::from_micros(MARGIN_US as u64),
            "{first:?}"
        );
        h.started();
        h.before_show();
        // The 3 ms frame's cost and the margin, less what this one took.
        let second = h.slept - first;
        assert!(second >= Duration::from_millis(3), "{second:?}");
        h.reset();
        h.before_show();
        assert_eq!(h.slept, first + second);
    }

    #[test]
    fn reads_frame_times() {
        assert_eq!(
            read_times("frame 1 us 300\nframe 2 us 410\n"),
            Ok(vec![300.0, 410.0])
        );
        assert!(read_times("frame 1\n").is_err());
    }
}
