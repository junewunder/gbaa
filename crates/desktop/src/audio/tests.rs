use super::sim::{self, Device};
use super::*;
use runtime::AudioChunk;
use std::time::Instant;

/// Cycles per output frame at 48 kHz.
const STEP_48K: f64 = CLOCK_HZ as f64 / 48000.0;

fn chunk<'a>(samples: &'a [(i16, i16)], period: u32) -> AudioChunk<'a> {
    AudioChunk {
        fifo: &[],
        samples,
        period,
        spans: &[],
        restarts: &[],
        swaps: &[],
        dropped: 0,
        psg: &[],
        psg_dropped: 0,
    }
}

#[test]
fn resampler_interpolates_linearly() {
    let mut r = Resampler::default();
    let mut out = Vec::new();
    // From 0 to 1 over 4 cycles, a frame every cycle: 0, 1/4, 1/2, 3/4.
    r.push([1.0, -1.0], 4.0, 1.0, &mut out);
    assert_eq!(out, [[0.0, 0.0], [0.25, -0.25], [0.5, -0.5], [0.75, -0.75]]);
    // The phase carries over: a step of 3 lands at 0, 3, then 2 and 5
    // into the next span.
    let mut out = Vec::new();
    let mut r = Resampler::default();
    r.push([1.0; 2], 4.0, 3.0, &mut out);
    r.push([1.0; 2], 4.0, 3.0, &mut out);
    assert_eq!(out.len(), 3);
}

#[test]
fn resampler_keeps_the_rate() {
    // A frame of the 18157 Hz ROM's stream (304 records of 924 cycles) makes
    // a frame's worth of 48 kHz frames, with no drift over 1000 frames.
    let mut r = Resampler::default();
    let mut out = Vec::new();
    for _ in 0..304 * 1000 {
        r.push([0.5; 2], 924.0, STEP_48K, &mut out);
    }
    let want = 1000.0 * FRAME_CYCLES as f64 / STEP_48K;
    assert!(
        (out.len() as f64 - want).abs() <= 1.0,
        "{} vs {want}",
        out.len()
    );
}

#[test]
fn queue_waits_for_its_target_then_counts_underruns() {
    let mut q = Queue::new(48000);
    let target = q.target();
    assert_eq!(target, 2400, "50 ms at 48 kHz");
    let mut out = [[9.0; 2]; 100];
    q.push(&vec![[0.5; 2]; target - 1]);
    q.pull(&mut out);
    assert!(!q.started && out == [[0.0; 2]; 100] && q.delivered == 0);
    q.push(&[[0.5; 2]]);
    assert!(q.started && q.ready());
    q.push(&[[0.5; 2]]);
    assert!(!q.ready());
    let mut out = vec![[0.0; 2]; target + 1];
    q.pull(&mut out);
    assert_eq!((q.delivered, q.underruns), (target as u64 + 1, 0));
    q.pull(&mut out[..1]);
    assert_eq!(q.underruns, 1);
    // A pull bigger than 10 ms raises the target past it.
    q.pull(&mut vec![[0.0; 2]; 4096]);
    assert_eq!(q.target(), 4096 + 1920);
}

#[test]
fn the_pacer_counts_what_the_device_played_since_its_last_pull() {
    let mut q = Queue::new(48000);
    q.max_pull = 1024;
    let target = q.target();
    q.push(&vec![[0.5; 2]; target]);
    let t = Instant::now();
    assert_eq!(q.due(t), None);
    // 240 frames over, 5 ms at 48 kHz: due 5 ms after the last pull.
    q.push(&[[0.5; 2]; 240]);
    assert_eq!(q.due(t), Some(Duration::MAX), "no pull yet");
    q.pulled_at = Some(t);
    assert_eq!(q.due(t), Some(Duration::from_millis(5)));
    assert_eq!(q.due(t + Duration::from_millis(5)), None);
    // More over than a pull: only a pull will do.
    q.push(&[[0.5; 2]; 1024]);
    assert_eq!(q.due(t + Duration::from_secs(1)), Some(Duration::MAX));
}

/// The 18157 Hz ROM's stream: 304 records of 924 cycles a frame, a 440 Hz
/// tone.
struct Synth {
    player: Player,
    n: u64,
    cost: f64,
}

impl Synth {
    fn frame(&mut self) -> f64 {
        let samples: Vec<(i16, i16)> = (0..304)
            .map(|_| {
                let t = self.n as f64 * 924.0 / CLOCK_HZ as f64;
                self.n += 1;
                let s = (t * 440.0 * std::f64::consts::TAU).sin() * 8000.0;
                (s as i16, -s as i16)
            })
            .collect();
        // The runtime hands a frame over in up to two chunks.
        self.player.audio(&chunk(&samples[..150], 924));
        self.player.audio(&chunk(&samples[150..], 924));
        self.player.end_frame();
        self.cost
    }
}

fn pace(drift: f64, quantum: usize, jitter: bool, cost: f64, seconds: f64) -> sim::Report {
    let mut s = Synth {
        player: Player::simulated(48000),
        n: 0,
        cost,
    };
    let device = Device {
        rate: 48000,
        drift,
        quantum,
        jitter,
        record: false,
    };
    sim::run(&mut s, |s| &mut s.player, Synth::frame, device, seconds)
}

/// The issue's pacing test: a fake 48 kHz device ±0.5% off, pulling on its
/// own clock, never underruns after the first second and keeps the queue
/// between 20 and 100 ms, with regular pulls and with irregular ones. And
/// the emulator runs at the device's speed: 59.73 frames a second, scaled
/// by the device's clock error.
#[test]
fn the_device_paces_the_emulator() {
    let seconds = 60.0;
    let fps = CLOCK_HZ as f64 / FRAME_CYCLES as f64;
    for drift in [-0.005, 0.0, 0.005] {
        for (quantum, jitter) in [256, 480, 1024, 2048]
            .into_iter()
            .flat_map(|q| [(q, false), (q, true)])
        {
            for cost in [0.0005, 0.012] {
                let r = pace(drift, quantum, jitter, cost, seconds);
                let what =
                    format!("drift {drift}, quantum {quantum}, jitter {jitter}, cost {cost}: {r}");
                assert!(r.passes(), "{what}");
                let speed = r.frames as f64 / (seconds * fps) / (1.0 + drift);
                assert!((speed - 1.0).abs() < 0.003, "speed {speed}, {what}");
            }
        }
    }
}

/// An emulator slower than real time underruns: the check can fail.
#[test]
fn a_slow_emulator_underruns() {
    let r = pace(0.0, 1024, false, 0.020, 10.0);
    assert!(r.underruns > 0 && !r.passes(), "{r}");
}

/// A silent game still runs at full speed: each frame without sound is
/// queued as a frame of silence.
#[test]
fn silence_paces_too() {
    let mut p = Player::simulated(48000);
    let device = Device {
        rate: 48000,
        drift: 0.0,
        quantum: 1024,
        jitter: false,
        record: false,
    };
    let r = sim::run(
        &mut p,
        |p| p,
        |p| {
            p.end_frame();
            0.001
        },
        device,
        10.0,
    );
    let fps = CLOCK_HZ as f64 / FRAME_CYCLES as f64;
    assert!(r.passes(), "{r}");
    assert!((r.frames as f64 / (10.0 * fps) - 1.0).abs() < 0.01, "{r}");
}

/// A fake device on a thread, pulling 480 frames every 10 ms of real time
/// (48 kHz) for `pulls` pulls, then stopping.
fn device_thread(shared: Arc<Shared>, pulls: u32) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = vec![[0.0; 2]; 480];
        let start = Instant::now();
        for i in 1..=pulls {
            let at = start + Duration::from_millis(10 * i as u64);
            std::thread::sleep(at.saturating_duration_since(Instant::now()));
            shared.pull(&mut buf);
        }
    })
}

/// The real pacer, `Player::wait`, against a device on its own thread:
/// the emulator runs no faster than the device plays, and the queue never
/// grows past a frame over its target.
#[test]
fn the_pacer_waits_for_the_device() {
    // A loaded machine can leave the device's thread unscheduled for
    // longer than `WAIT_LIMIT`; the pacer then rightly falls back on the
    // frame clock, and the queue grows until the device is back (what
    // `a_stalled_device_falls_back_to_the_frame_clock` tests). Such a run
    // says nothing about this test's case, so it runs again.
    for _ in 0..5 {
        let shared = Shared::new(48000);
        let mut s = Synth {
            player: Player::new(shared.clone(), None, true),
            n: 0,
            cost: 0.0,
        };
        let device = device_thread(shared, 200);
        let start = Instant::now();
        let mut stalled = false;
        for _ in 0..60 {
            s.frame();
            s.player.wait();
            stalled |= s.player.stalled.is_some();
        }
        let took = start.elapsed().as_secs_f64();
        device.join().unwrap();
        if stalled {
            continue;
        }
        let q = s.player.queue();
        // 60 frames are 1.005 s; the first 50 ms of sound is queued up front.
        assert!(took > 0.9, "60 frames in {took:.3} s");
        assert!(
            q.ms(q.max_len) < 80.0,
            "queue up to {:.1} ms",
            q.ms(q.max_len)
        );
        return;
    }
    panic!("the device's thread fell behind in every run");
}

/// A device that stops pulling costs one wait of `WAIT_LIMIT`; after that
/// the frame clock paces, at 59.73 frames a second.
#[test]
fn a_stalled_device_falls_back_to_the_frame_clock() {
    let shared = Shared::new(48000);
    let mut s = Synth {
        player: Player::new(shared.clone(), None, true),
        n: 0,
        cost: 0.0,
    };
    device_thread(shared, 20).join().unwrap();
    // The queue starts full enough to wait on, past what the device could
    // have played since its last pull: nothing pulls any more. (Over by a
    // pull or less, it would be due at a moment that may pass first.)
    while s.player.queue().due(Instant::now()) != Some(Duration::MAX) {
        s.frame();
    }
    let start = Instant::now();
    s.player.wait();
    assert!(s.player.stalled.is_some());
    for _ in 0..30 {
        s.frame();
        s.player.wait();
    }
    let took = start.elapsed().as_secs_f64();
    // 200 ms, then 30 frames of 16.7 ms: 0.70 s. The frame clock absorbs
    // a late wake-up, so a loaded machine adds little; waiting out
    // `WAIT_LIMIT` each frame instead would take 6 s.
    assert!((0.65..3.0).contains(&took), "took {took:.3} s");
    // The device comes back: the stale queue goes, and it refills from
    // empty, so the next wait doesn't sit through half a second of it.
    {
        let q = s.player.queue();
        assert_eq!(q.len(), q.cap());
    }
    // It pulls here rather than on a thread, so the pull is sure to come
    // before the wait.
    s.player.shared.pull(&mut [[0.0; 2]; 480]);
    s.frame();
    let start = Instant::now();
    s.player.wait();
    assert!(s.player.stalled.is_none() && s.player.level.is_none());
    assert!(start.elapsed() < WAIT_LIMIT);
    let q = s.player.queue();
    assert!(!q.started && q.len() < 1000, "{} queued", q.len());
}

#[test]
fn channels_map_from_stereo() {
    let frames = [[0.5, -0.5], [1.0, 0.0]];
    let mut mono = [9.0f32; 2];
    device::interleave(&frames, 1, &mut mono);
    assert_eq!(mono, [0.0, 0.5]);
    let mut stereo = [0i16; 4];
    device::interleave(&frames, 2, &mut stereo);
    assert_eq!(stereo, [16384, -16384, 32767, 0]);
    // Past the second channel: silence, which for u16 is the midpoint.
    let mut quad = [0u16; 8];
    device::interleave(&frames, 4, &mut quad);
    assert_eq!(&quad[2..4], &[32768, 32768]);
    assert_eq!(&quad[6..8], &[32768, 32768]);
}

/// The master gain: −6 dB by default, applied to what's resampled for the
/// device, and exact (a halving in `f32`), so it only scales the stream.
#[test]
fn the_gain_scales_what_plays() {
    let played = |gain: Option<f32>| {
        let mut p = Player::simulated(48000);
        if let Some(g) = gain {
            p.set_gain(g);
        }
        let samples: Vec<(i16, i16)> = (0..304).map(|i| (i * 100, -i * 100)).collect();
        p.audio(&chunk(&samples, 924));
        p.pending
    };
    let (default, whole) = (played(None), played(Some(1.0)));
    assert_eq!(DEFAULT_GAIN, 0.5);
    assert_eq!(default.len(), whole.len());
    assert!(whole.iter().any(|v| v[0] > 0.5));
    for (d, w) in default.iter().zip(&whole) {
        assert_eq!(*d, [w[0] * 0.5, w[1] * 0.5]);
    }
}

/// Netplay's hold queues a frame's worth of silence, as a silent frame
/// does, so the pacer waits a frame for it instead of catching up.
#[test]
fn a_held_frame_queues_a_frame_of_silence() {
    let mut p = Player::undrained(48000);
    p.hold();
    let frame = 48000.0 * FRAME_CYCLES as f64 / CLOCK_HZ as f64;
    let len = p.queue().len() as f64;
    assert!(
        (len - frame).abs() <= 2.0,
        "{len} queued, a frame is {frame}"
    );
}
