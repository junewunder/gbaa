//! Playback (#53): the runtime's sound stream, resampled to the host
//! device's rate and played through `cpal`, with the device's consumption
//! pacing the emulator (docs/sound-design.md, "Playback").
//!
//! The emulator thread owns a [`Player`]. Each chunk the runtime hands over
//! is resampled from its own rate (16777216 / `period` Hz) to the device's,
//! and at the end of each frame the result goes into a [`Queue`] the device
//! drains from its own thread. The emulator then waits until the queue is
//! back down to its target, so it runs exactly as fast as the device plays.
//! [`sim`] drives the same `Player` from a simulated device clock, for the
//! pacing test and `just audio-pace`.

mod device;
pub mod sim;

use crate::capture::CLOCK_HZ;
use runtime::AudioChunk;
use runtime::sched::FRAME_CYCLES;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// One stereo frame at the device's rate, left then right, in −1..1.
pub type Stereo = [f32; 2];

/// How much sound the emulator keeps queued ahead of the device: it runs
/// the next frame only once the queue is down to this. The device's own
/// pulls can raise it (see [`Queue::target`]).
pub const TARGET_MS: f64 = 50.0;

/// Headroom over the device's largest pull: the target is never under
/// that pull plus this, so a late pull followed straight away by the next
/// one, during a slow frame, still leaves sound queued.
pub const MARGIN_MS: f64 = 40.0;

/// The master gain on what the desktop plays: −6 dB, which puts our
/// loudness near desktop audio's instead of past it (docs/sound-design.md,
/// "Level"). Applied here, in `f32`, never in the runtime's `mix()`, whose
/// output the audio hashes and the mGBA oracle judge; headless `--wav` and
/// `--audio-hash` stay ungained.
pub const DEFAULT_GAIN: f32 = 0.5;

/// The master volume, in percent: 100 is [`DEFAULT_GAIN`], and the gain
/// scales with it, so 200 plays the runtime's stream as it is and 0 is
/// silent (still paced by the device).
pub const DEFAULT_VOLUME: u32 = 100;
pub const MAX_VOLUME: u32 = 200;

/// Parse a volume as `--volume` and the settings file write it: a whole
/// percent from 0 to [`MAX_VOLUME`], `%` optional.
pub fn parse_volume(v: &str) -> Result<u32, String> {
    v.strip_suffix('%')
        .unwrap_or(v)
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|p| *p <= MAX_VOLUME)
        .ok_or(format!("bad volume {v:?} (0-{MAX_VOLUME} percent)"))
}

/// The master gain at `volume` percent.
pub fn volume_gain(volume: u32) -> f32 {
    DEFAULT_GAIN * volume as f32 / DEFAULT_VOLUME as f32
}

/// Rate control: the resampler's ratio moves by up to this fraction (0.5%,
/// under 9 cents) with the queue's distance from its usual level.
pub const RATE_CONTROL: f64 = 0.005;

/// How many frames the queue's usual level averages over (about a second).
const LEVEL_FRAMES: f64 = 64.0;

/// Past this the queue drops what's pushed (only if the device stops
/// pulling and the pacer gives up waiting).
pub const MAX_MS: f64 = 500.0;

/// How long the pacer waits for a pull before deciding the device has
/// stopped and pacing by the frame clock instead, until it pulls again.
const WAIT_LIMIT: Duration = Duration::from_millis(200);

/// The sound waiting for the device, at its rate.
#[derive(Debug)]
pub struct Queue {
    buf: VecDeque<Stereo>,
    /// The device's frames a second.
    pub rate: u32,
    /// The largest pull the device has made, in frames.
    pub max_pull: usize,
    /// True once the queue first reached its target after the device's
    /// first pull (which says how big its pulls are): until then the
    /// device gets silence and the queue fills.
    pub started: bool,
    /// Frames the device took from the queue.
    pub delivered: u64,
    /// Pulls that found the queue short, after it started.
    pub underruns: u64,
    /// Frames dropped because the queue was at [`MAX_MS`].
    pub overflowed: u64,
    /// Pulls ever.
    pub pulls: u64,
    /// The queue's extremes once started, in frames: the least right after
    /// a pull, the most right after a push.
    pub min_len: usize,
    pub max_len: usize,
    /// When the device last pulled (`None` under [`sim`], which paces by
    /// [`Queue::ready`] alone).
    pub pulled_at: Option<Instant>,
}

impl Queue {
    pub fn new(rate: u32) -> Queue {
        // Reserved up front, so a push never reallocates while the device
        // waits on the lock.
        Queue {
            buf: VecDeque::with_capacity((rate as f64 * MAX_MS / 1000.0) as usize),
            rate,
            max_pull: 0,
            started: false,
            delivered: 0,
            underruns: 0,
            overflowed: 0,
            pulls: 0,
            min_len: usize::MAX,
            max_len: 0,
            pulled_at: None,
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// `frames` in milliseconds.
    pub fn ms(&self, frames: usize) -> f64 {
        frames as f64 * 1000.0 / self.rate as f64
    }

    /// Frames the emulator keeps queued: [`TARGET_MS`], or [`MARGIN_MS`]
    /// more than the device's largest pull, so a pull never takes the queue
    /// near empty right after the emulator stopped filling it.
    pub fn target(&self) -> usize {
        let frames = |ms: f64| (self.rate as f64 * ms / 1000.0) as usize;
        frames(TARGET_MS).max(self.max_pull + frames(MARGIN_MS))
    }

    /// The pacer's question: may the emulator run its next frame?
    pub fn ready(&self) -> bool {
        self.buf.len() <= self.target()
    }

    /// How long, from `now`, until the emulator may run its next frame,
    /// or `None` if it may now. The device takes the queue a pull at a
    /// time (21 ms under PipeWire's default quantum) but plays it steadily,
    /// so up to a pull's worth played since the last pull counts as gone:
    /// frames then run a frame apart, not two at once after each pull,
    /// and the window draws them evenly. Waiting on [`Queue::ready`]
    /// alone, the queue would sit about a pull lower. `Duration::MAX`:
    /// only a pull will do.
    pub fn due(&self, now: Instant) -> Option<Duration> {
        let over = self.buf.len().saturating_sub(self.target());
        if over == 0 {
            return None;
        }
        match self.pulled_at {
            Some(t) if over <= self.max_pull => {
                let at = t + Duration::from_secs_f64(over as f64 / self.rate as f64);
                Some(at.saturating_duration_since(now)).filter(|d| !d.is_zero())
            }
            _ => Some(Duration::MAX),
        }
    }

    /// The most the queue holds: [`MAX_MS`].
    pub fn cap(&self) -> usize {
        (self.rate as f64 * MAX_MS / 1000.0) as usize
    }

    /// Empty the queue and fill it to the target again before playing,
    /// after the device stalled (what's queued by then is stale).
    pub fn restart(&mut self) {
        self.buf.clear();
        self.started = false;
    }

    pub fn push(&mut self, frames: &[Stereo]) {
        let room = self.cap().saturating_sub(self.buf.len()).min(frames.len());
        self.buf.extend(&frames[..room]);
        self.overflowed += (frames.len() - room) as u64;
        self.started |= self.max_pull > 0 && self.buf.len() >= self.target();
        if self.started {
            self.max_len = self.max_len.max(self.buf.len());
        }
    }

    /// Fill `out` from the queue: silence until it has started, and
    /// silence for whatever is missing after that (an underrun).
    pub fn pull(&mut self, out: &mut [Stereo]) {
        self.pulls += 1;
        self.max_pull = self.max_pull.max(out.len());
        self.started |= self.buf.len() >= self.target();
        let n = if self.started {
            out.len().min(self.buf.len())
        } else {
            0
        };
        for (o, s) in out.iter_mut().zip(self.buf.drain(..n)) {
            *o = s;
        }
        out[n..].fill([0.0; 2]);
        self.delivered += n as u64;
        if self.started {
            self.min_len = self.min_len.min(self.buf.len());
            if n < out.len() {
                self.underruns += 1;
            }
        }
    }
}

/// The queue, shared with the device's thread, and the signal its pulls
/// give the pacer.
#[derive(Debug)]
pub struct Shared {
    queue: Mutex<Queue>,
    pulled: Condvar,
}

impl Shared {
    pub fn new(rate: u32) -> Arc<Shared> {
        Arc::new(Shared {
            queue: Mutex::new(Queue::new(rate)),
            pulled: Condvar::new(),
        })
    }

    pub fn queue(&self) -> MutexGuard<'_, Queue> {
        // A panic on the other side leaves the queue usable.
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The device's side: fill `out` and wake the pacer.
    pub fn pull(&self, out: &mut [Stereo]) {
        let mut q = self.queue();
        q.pull(out);
        q.pulled_at = Some(Instant::now());
        drop(q);
        self.pulled.notify_one();
    }
}

/// Linear interpolation from the stream's samples to the device's frames,
/// on the machine's clock: each input sample spans some guest cycles, and
/// an output frame falls every `CLOCK_HZ / device rate` cycles.
#[derive(Clone, Debug, Default)]
pub struct Resampler {
    /// The last input sample.
    prev: Stereo,
    /// Cycles past `prev` the next output frame falls.
    pos: f64,
}

impl Resampler {
    /// Take `value` as the sample `span` cycles after the last one, and
    /// emit the output frames between the two, one every `step` cycles.
    pub fn push(&mut self, value: Stereo, span: f64, step: f64, out: &mut Vec<Stereo>) {
        while self.pos < span {
            let t = (self.pos / span) as f32;
            out.push([0, 1].map(|c| self.prev[c] + (value[c] - self.prev[c]) * t));
            self.pos += step;
        }
        self.pos -= span;
        self.prev = value;
    }
}

/// The emulator's side of playback, owned by the desktop platform.
pub struct Player {
    shared: Arc<Shared>,
    resampler: Resampler,
    /// Resampled this frame, not yet queued.
    pending: Vec<Stereo>,
    /// Records this frame.
    records: u64,
    /// Cycles per output frame, with rate control applied; set each flush.
    step: f64,
    /// The queue's usual length right after a push (an average over
    /// [`LEVEL_FRAMES`]), which rate control steers around; `None` until
    /// the queue is playing, and again after a stall.
    level: Option<f64>,
    /// The device stream; `None` under [`sim`].
    output: Option<device::Output>,
    /// Whether [`Player::wait`] blocks (false under [`sim`]).
    blocking: bool,
    /// When the device has stopped pulling: the next frame's deadline on
    /// the frame clock.
    stalled: Option<Instant>,
    /// The queue's pull count when the pacer last returned.
    pulls_seen: u64,
    /// How long [`Player::wait`] has blocked, in all.
    pub waited: Duration,
    /// The master gain, [`DEFAULT_GAIN`] unless set.
    gain: f32,
}

impl Player {
    /// Open the host's default output device.
    pub fn open() -> Result<Player, String> {
        let (output, shared) = device::open()?;
        Ok(Player::new(shared, Some(output), true))
    }

    /// A player whose queue only [`sim`] drains.
    pub fn simulated(rate: u32) -> Player {
        Player::new(Shared::new(rate), None, false)
    }

    /// A blocking player no device drains, so its pacer falls back on the
    /// frame clock: the window's pacing, for tests.
    #[cfg(test)]
    pub(crate) fn undrained(rate: u32) -> Player {
        Player::new(Shared::new(rate), None, true)
    }

    fn new(shared: Arc<Shared>, output: Option<device::Output>, blocking: bool) -> Player {
        let rate = shared.queue().rate;
        Player {
            shared,
            resampler: Resampler::default(),
            pending: Vec::new(),
            records: 0,
            step: CLOCK_HZ as f64 / rate as f64,
            level: None,
            output,
            blocking,
            stalled: None,
            waited: Duration::ZERO,
            pulls_seen: 0,
            gain: DEFAULT_GAIN,
        }
    }

    /// The master gain, as a factor on the runtime's stream.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Set the master gain: from the next chunk on, what's already
    /// resampled keeps the old one.
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    /// What the device got, for the line a windowed run ends with.
    pub fn summary(&self) -> String {
        let q = self.queue();
        format!(
            "audio: {} samples delivered, {} underruns, queue {:.1}..{:.1} ms",
            q.delivered,
            q.underruns,
            q.ms(q.min_len.min(q.max_len)),
            q.ms(q.max_len),
        )
    }

    /// The device's name and rate, for the startup line.
    pub fn describe(&self) -> String {
        let rate = self.shared.queue().rate;
        match &self.output {
            Some(o) => format!("{} at {rate} Hz", o.name),
            None => format!("simulated device at {rate} Hz"),
        }
    }

    pub fn queue(&self) -> MutexGuard<'_, Queue> {
        self.shared.queue()
    }

    /// Resample a chunk of the runtime's stream, each run of it at its own
    /// rate (FIFO records, or the PSG's grid while no timer clocks a FIFO).
    pub fn audio(&mut self, chunk: &AudioChunk<'_>) {
        let whole = [(0, chunk.period)];
        let spans = if chunk.spans.is_empty() {
            &whole[..]
        } else {
            chunk.spans
        };
        let len = chunk.samples.len();
        let scale = self.gain / 32768.0;
        for (i, &(from, period)) in spans.iter().enumerate() {
            let to = spans.get(i + 1).map_or(len, |s| (s.0 as usize).min(len));
            let from = (from as usize).min(to);
            if period == 0 {
                continue;
            }
            let span = period as f64;
            for &(l, r) in &chunk.samples[from..to] {
                let v = [l as f32 * scale, r as f32 * scale];
                self.resampler.push(v, span, self.step, &mut self.pending);
            }
            self.records += (to - from) as u64;
        }
    }

    /// The frame is over. A frame with no sound still lasts a frame: it
    /// becomes a frame of silence (a ramp to 0 from the last sample), so
    /// the device paces silent games too. Then, unless simulated, the
    /// frame's sound is queued.
    pub fn end_frame(&mut self) {
        if self.records == 0 {
            let span = FRAME_CYCLES as f64;
            self.resampler
                .push([0.0; 2], span, self.step, &mut self.pending);
        }
        self.records = 0;
        if self.blocking {
            self.flush();
        }
    }

    /// Netplay's time sync held a frame back: a frame of silence (a ramp
    /// to 0, as a silent frame's) is queued in its place, so the pacer
    /// lets the next frame run a frame later, as if this one had run.
    /// Without it the pacer, finding the queue a frame short, would run
    /// the next frame at once and undo the hold.
    pub fn hold(&mut self) {
        let span = FRAME_CYCLES as f64;
        self.resampler
            .push([0.0; 2], span, self.step, &mut self.pending);
        self.records = 0;
        if self.blocking {
            self.flush();
        }
    }

    /// A fast-forwarded frame: its sound is dropped, not queued.
    pub fn drop_frame(&mut self) {
        self.records = 0;
        self.pending.clear();
    }

    /// Fast-forward started or stopped: empty the queue, so the device
    /// plays silence at once rather than a stream that no longer keeps up,
    /// and let it fill to the target again before playing.
    pub fn silence(&mut self) {
        self.pending.clear();
        self.resampler = Resampler::default();
        self.records = 0;
        let mut q = self.shared.queue();
        q.restart();
        self.pulls_seen = q.pulls;
        drop(q);
        self.level = None;
        self.stalled = None;
    }

    /// Queue what's been resampled, and set the next frame's rate from how
    /// far the queue is from its usual level.
    ///
    /// Where the queue sits after a push depends on how the device's pulls
    /// beat against the frames, so rather than a fixed point the centre is
    /// its own running average: the error averages out to 0 and the pitch
    /// has no standing offset. The average only follows a playing queue
    /// under its cap, so filling up at the start or during a stall doesn't
    /// skew it.
    pub fn flush(&mut self) {
        let mut q = self.shared.queue();
        q.push(&self.pending);
        self.pending.clear();
        let len = q.len() as f64;
        let mut error = 0.0;
        if q.started && q.len() < q.cap() {
            let level = self.level.get_or_insert(len);
            error = ((len - *level) / level.max(1.0)).clamp(-1.0, 1.0);
            *level += (len - *level) / LEVEL_FRAMES;
        }
        // Fuller than usual: a longer step, fewer frames.
        self.step = CLOCK_HZ as f64 / q.rate as f64 * (1.0 + RATE_CONTROL * error);
    }

    /// The pacer: block until the device has drained the queue to its
    /// target ([`Queue::due`]). If the device goes [`WAIT_LIMIT`] without a pull (unplugged,
    /// the sound server restarted, or slow to start), pace by the frame
    /// clock until it pulls again; then drop the stale queue and refill it.
    pub fn wait(&mut self) {
        if !self.blocking {
            // Simulated: `sim` does the waiting on its own clock.
            return;
        }
        let entered = Instant::now();
        self.pace();
        self.waited += entered.elapsed();
    }

    /// [`Player::wait`]'s blocking part.
    fn pace(&mut self) {
        let frame = Duration::from_secs_f64(FRAME_CYCLES as f64 / CLOCK_HZ as f64);
        let mut q = self.shared.queue();
        if let Some(deadline) = self.stalled {
            if q.pulls == self.pulls_seen {
                drop(q);
                let now = Instant::now();
                std::thread::sleep(deadline.saturating_duration_since(now));
                self.stalled = Some(deadline.max(now) + frame);
                return;
            }
            if self.pulls_seen > 0 {
                eprintln!("gbaa-desktop: audio: the device is back");
            }
            q.restart();
            self.level = None;
            self.stalled = None;
        }
        let start = Instant::now();
        while let Some(due) = q.due(Instant::now()) {
            let waited = start.elapsed();
            if waited >= WAIT_LIMIT {
                // A device that hasn't started yet isn't news.
                if q.pulls > 0 {
                    eprintln!(
                        "gbaa-desktop: audio: the device stopped taking sound; \
                         pacing by the frame clock"
                    );
                }
                self.stalled = Some(Instant::now() + frame);
                break;
            }
            q = self
                .shared
                .pulled
                .wait_timeout(q, due.min(WAIT_LIMIT - waited))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        self.pulls_seen = q.pulls;
    }
}

#[cfg(test)]
mod tests;
