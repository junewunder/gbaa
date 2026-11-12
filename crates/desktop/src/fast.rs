//! Fast-forward: while a button is held, or after a toggle, the window runs
//! frames unthrottled or at a set multiple of the GBA's 59.73 Hz. The
//! machine runs exactly the frames it would anyway, so what it computes
//! doesn't change; only the front end does. The sound is muted (the queue
//! emptied, so the device plays silence rather than a starved, crackling
//! stream), the window is redrawn [`DRAW_HZ`] times a second ([`DrawPacer`]), and
//! on release the queue refills and the usual pacer takes over again.

use crate::capture::CLOCK_HZ;
use runtime::sched::FRAME_CYCLES;
use std::time::{Duration, Instant};

/// Fast-forward's speed when neither `--fast-speed` nor the settings file
/// ([`crate::settings`]) says: unthrottled is uncontrollably fast in play.
pub const DEFAULT_SPEED: f64 = 2.5;

/// The slowest and fastest speeds `--fast-speed` takes, besides unlimited.
pub const MIN_SPEED: f64 = 1.1;
pub const MAX_SPEED: f64 = 16.0;

/// Parse a speed as `--fast-speed` and the settings file write it: a
/// multiple of normal speed from [`MIN_SPEED`] to [`MAX_SPEED`], e.g. 2.5,
/// or `unlimited` (0) for unthrottled.
pub fn parse_speed(v: &str) -> Result<f64, String> {
    if v == "unlimited" {
        return Ok(0.0);
    }
    v.parse::<f64>()
        .ok()
        .filter(|s| *s == 0.0 || (MIN_SPEED..=MAX_SPEED).contains(s))
        .ok_or(format!(
            "bad fast-forward speed {v:?} ({MIN_SPEED}-{MAX_SPEED}, or unlimited)"
        ))
}

/// How often the window is redrawn while fast-forwarding.
pub const DRAW_HZ: f64 = 60.0;

/// When to redraw while fast-forwarding: on a fixed [`DRAW_HZ`] schedule,
/// the first frame at or past each deadline. Counting from the last draw
/// instead lands late every time (at 2.5x, the first 6.7 ms frame past
/// 16.7 ms comes at 20.1: 50 draws a second); on the schedule the gaps
/// even out to 1/[`DRAW_HZ`] at any speed.
#[derive(Clone, Debug, Default)]
pub struct DrawPacer {
    next: Option<Instant>,
}

impl DrawPacer {
    /// Whether the frame finished at `now` is drawn. Each draw moves the
    /// deadline on one period; a draw that falls more than a period behind
    /// (frames slower than [`DRAW_HZ`]) starts the schedule afresh rather
    /// than drawing in a burst to catch up.
    pub fn due(&mut self, now: Instant) -> bool {
        let next = self.next.unwrap_or(now);
        if now < next {
            return false;
        }
        let period = Duration::from_secs_f64(1.0 / DRAW_HZ);
        let after = next + period;
        self.next = Some(if after > now { after } else { now + period });
        true
    }

    /// Start afresh: the next frame is drawn.
    pub fn reset(&mut self) {
        self.next = None;
    }
}

/// One GBA frame of wall-clock time: 280896 cycles at 2^24 Hz, 59.73 Hz.
pub fn frame() -> Duration {
    Duration::from_secs_f64(FRAME_CYCLES as f64 / CLOCK_HZ as f64)
}

/// A frame clock: each [`Clock::wait`] returns one period after the last.
/// A frame that runs late doesn't make the next ones hurry to catch up.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    next: Option<Instant>,
    /// The sleeps it has asked for, in all (what the tests check, since
    /// wall-clock time grows with the machine's load).
    pub slept: Duration,
}

impl Clock {
    /// Wait out the rest of this period.
    pub fn wait(&mut self, period: Duration) {
        let now = Instant::now();
        let next = self.next.unwrap_or(now);
        if next > now {
            self.slept += next - now;
            std::thread::sleep(next - now);
        }
        self.next = Some(next.max(now) + period);
    }

    /// Start afresh: the next wait returns at once.
    pub fn reset(&mut self) {
        self.next = None;
    }
}

/// A frame clock that catches up, as pacing by sound does: frame k is
/// due k periods after the first, and a frame that runs late lets the
/// next ones run at once until they are on time again, unless more than
/// [`Pacer::SLACK`] behind (the sound queue's worth), when it starts
/// afresh. Netplay's: a peer that slid back on every late frame would
/// fall behind the others, and time sync would hold them back to match.
#[derive(Clone, Debug, Default)]
pub struct Pacer {
    next: Option<Instant>,
}

impl Pacer {
    /// The most it catches up: `audio::TARGET_MS`.
    pub const SLACK: Duration = Duration::from_millis(50);

    /// Wait until the next frame is due.
    pub fn wait(&mut self, period: Duration) {
        let now = Instant::now();
        let mut due = *self.next.get_or_insert(now);
        if due > now {
            std::thread::sleep(due - now);
        } else if now - due > Pacer::SLACK {
            due = now - Pacer::SLACK;
        }
        self.next = Some(due + period);
    }

    /// A frame held back (netplay's time sync): the next is due a period
    /// later, as if it had run.
    pub fn hold(&mut self, period: Duration) {
        if let Some(n) = &mut self.next {
            *n += period;
        }
    }
}

/// Whether fast-forward is on, and how fast.
#[derive(Clone, Debug, Default)]
pub struct FastForward {
    /// The hold button (or key) is down.
    pub held: bool,
    /// Toggled on.
    pub toggled: bool,
    /// The multiple of normal speed; 0 is unthrottled.
    pub speed: f64,
    /// The toggle button was down last frame, so a press is an edge.
    toggle_down: bool,
}

impl FastForward {
    pub fn new(speed: f64, on: bool) -> FastForward {
        FastForward {
            speed,
            toggled: on,
            ..FastForward::default()
        }
    }

    pub fn active(&self) -> bool {
        self.held || self.toggled
    }

    /// Take this frame's controls: `held` the hold button, `toggle` the
    /// toggle button, which flips fast-forward as it goes down.
    pub fn input(&mut self, held: bool, toggle: bool) {
        self.held = held;
        if toggle && !self.toggle_down {
            self.toggled = !self.toggled;
        }
        self.toggle_down = toggle;
    }

    /// The frame period while fast-forwarding; `None` when unthrottled.
    pub fn period(&self) -> Option<Duration> {
        (self.speed > 0.0).then(|| frame().div_f64(self.speed))
    }
}
