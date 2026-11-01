//! A simulated output device, for the pacing test and `just audio-pace`:
//! it pulls from a [`Player`]'s queue on its own clock, independent of
//! the emulator's frames, and the emulator runs a frame whenever the same
//! pacer rule as on a real device ([`Queue::ready`]) lets it.
//!
//! [`Queue::ready`]: super::Queue::ready

use super::{Player, Stereo};

/// The device: its nominal rate, how far its real clock is off, and how
/// many frames it takes a pull.
#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub rate: u32,
    /// The real clock's error, as a fraction (0.005 is 0.5% fast).
    pub drift: f64,
    pub quantum: usize,
    /// Irregular pulls: each takes between half and all of `quantum`
    /// frames, and comes up to 30% of a full pull's time late (the clock
    /// itself keeps time, so the rate is unchanged).
    pub jitter: bool,
    /// Keep what the device played, in [`Report::played`].
    pub record: bool,
}

/// What a simulated run saw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Emulator frames run.
    pub frames: u64,
    /// Frames the device took from the queue.
    pub delivered: u64,
    /// Pulls that found the queue short, after the first second.
    pub underruns: u64,
    /// Frames the queue dropped for being full.
    pub overflowed: u64,
    /// The queue's extremes once playing ([`Queue::min_len`]).
    ///
    /// [`Queue::min_len`]: super::Queue::min_len
    pub min_ms: f64,
    pub max_ms: f64,
    /// What the device played, if [`Device::record`]: the desktop's own
    /// output, gain and all (`--played-wav`).
    pub played: Vec<Stereo>,
}

impl Report {
    /// The issue's bar: no underrun after the first second, and the queue
    /// between 20 and 100 ms throughout.
    pub fn passes(&self) -> bool {
        self.underruns == 0
            && self.overflowed == 0
            && self.delivered > 0
            && self.min_ms >= 20.0
            && self.max_ms <= 100.0
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} frames, {} samples delivered, {} underruns, {} overflowed, \
             queue {:.1}..{:.1} ms",
            self.frames, self.delivered, self.underruns, self.overflowed, self.min_ms, self.max_ms
        )
    }
}

/// Run `seconds` of the device's (real) clock. `frame` runs one emulator
/// frame on `state`, which hands its sound to `player(state)`, and returns
/// the host time the frame took, in seconds: its sound reaches the queue
/// that long after the frame starts.
pub fn run<S>(
    state: &mut S,
    player: impl Fn(&mut S) -> &mut Player,
    mut frame: impl FnMut(&mut S) -> f64,
    device: Device,
    seconds: f64,
) -> Report {
    let mut dev = Pulls {
        device,
        clock: 0.0,
        at: 0.0,
        size: device.quantum,
        end: seconds,
        buf: vec![[0.0; 2]; device.quantum],
        seed: 0x9e37_79b9_7f4a_7c15,
    };
    dev.next();
    let mut report = Report::default();
    // When the emulator next looks at the queue.
    let mut emu_at = 0.0;
    while dev.at <= seconds {
        dev.until(emu_at, player(state), &mut report);
        if player(state).queue().ready() {
            let done = emu_at + frame(state);
            report.frames += 1;
            dev.until(done, player(state), &mut report);
            player(state).flush();
            emu_at = done;
        } else {
            // Waiting: the next pull wakes it.
            emu_at = dev.at;
        }
    }
    let q = player(state).queue();
    report.delivered = q.delivered;
    report.overflowed = q.overflowed;
    report.min_ms = q.ms(q.min_len.min(q.max_len));
    report.max_ms = q.ms(q.max_len);
    report
}

/// The device's pulls, on its own clock.
struct Pulls {
    device: Device,
    /// When the device has played everything it pulled so far.
    clock: f64,
    /// The next pull, and its size.
    at: f64,
    size: usize,
    end: f64,
    buf: Vec<Stereo>,
    /// For the jitter (xorshift).
    seed: u64,
}

impl Pulls {
    /// A number in 0..1.
    fn random(&mut self) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Schedule the next pull: it comes when the device has played the
    /// last one (plus any lateness), for the next size.
    fn next(&mut self) {
        let d = self.device;
        let rate = d.rate as f64 * (1.0 + d.drift);
        let (size, late) = if d.jitter {
            let half = d.quantum / 2;
            let size = half + (self.random() * (d.quantum - half + 1) as f64) as usize;
            (
                size.clamp(1, d.quantum),
                self.random() * 0.3 * d.quantum as f64 / rate,
            )
        } else {
            (d.quantum, 0.0)
        };
        self.clock += size as f64 / rate;
        self.size = size;
        self.at = self.clock + late;
    }

    /// Make every pull due by `t`.
    fn until(&mut self, t: f64, player: &mut Player, report: &mut Report) {
        while self.at <= t && self.at <= self.end {
            let mut q = player.queue();
            let underruns = q.underruns;
            q.pull(&mut self.buf[..self.size]);
            if self.device.record {
                report.played.extend_from_slice(&self.buf[..self.size]);
            }
            if self.at > 1.0 {
                report.underruns += q.underruns - underruns;
            }
            drop(q);
            self.next();
        }
    }
}
