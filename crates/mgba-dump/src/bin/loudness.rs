//! `loudness A.wav [B.wav] [--frames F..G]`: how loud 16-bit stereo PCM
//! WAVs are: the peak (dBFS), the RMS over both channels (dBFS, a full-scale
//! square wave being 0) and the integrated loudness (LUFS, ITU-R BS.1770-4:
//! K-weighted, 400 ms blocks every 100 ms, gated at -70 LUFS and 10 LU
//! under the ungated level). `--frames` takes only GBA frames F to G of
//! each (a frame is 280896 cycles of 2^24 Hz), so a `gbaa-desktop --wav`
//! and a `mgba-dump --wav` of the same run compare over the same stretch
//! though their sample rates differ. With two files it prints B minus A.
//! For the volume measurements in docs/sound-design.md ("Playback").

use std::process::ExitCode;

/// Seconds a GBA frame lasts.
const FRAME_SECONDS: f64 = 280_896.0 / 16_777_216.0;

struct Wav {
    rate: u32,
    samples: Vec<(i16, i16)>,
}

fn read(path: &str) -> Result<Wav, String> {
    let err = |e: &dyn std::fmt::Display| format!("{path}: {e}");
    let b = std::fs::read(path).map_err(|e| err(&e))?;
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(err(&"not a WAV"));
    }
    let (mut at, mut fmt, mut data) = (12, None, None);
    while at + 8 <= b.len() {
        let len = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &b[at + 8..(at + 8 + len).min(b.len())];
        match &b[at..at + 4] {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + len + len % 2;
    }
    let (Some(fmt), Some(data)) = (fmt, data) else {
        return Err(err(&"no fmt or data chunk"));
    };
    let u16_at = |i: usize| u16::from_le_bytes([fmt[i], fmt[i + 1]]);
    if fmt.len() < 16 || u16_at(0) != 1 || u16_at(2) != 2 || u16_at(14) != 16 {
        return Err(err(&"not 16-bit stereo PCM"));
    }
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
    let samples = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|s| {
            (
                i16::from_le_bytes([s[0], s[1]]),
                i16::from_le_bytes([s[2], s[3]]),
            )
        })
        .collect();
    Ok(Wav { rate, samples })
}

/// A biquad, `b` over `a` with `a0` = 1.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        // Transposed direct form II.
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// BS.1770's K-weighting at `rate` Hz: its high shelf, then its high pass,
/// from the analog prototype (libebur128's), so any rate works; at 48 kHz
/// these are the standard's coefficients.
fn k_weighting(rate: f64) -> [Biquad; 2] {
    let (f0, gain, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(gain / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    [shelf, high_pass]
}

struct Level {
    seconds: f64,
    peak: f64,
    rms: f64,
    lufs: f64,
}

fn db(power: f64) -> f64 {
    10.0 * power.log10()
}

/// `samples` at `rate` Hz, each scaled to ±1.
fn level(samples: &[(f64, f64)], rate: u32) -> Level {
    let peak = samples
        .iter()
        .fold(0f64, |m, &(l, r)| m.max(l.abs()).max(r.abs()));
    let power = samples.iter().map(|&(l, r)| l * l + r * r).sum::<f64>();
    let rms = db(power / (2 * samples.len().max(1)) as f64);
    // K-weighted power per 100 ms step; a block is four steps.
    let mut filters = [k_weighting(rate as f64), k_weighting(rate as f64)];
    let step = (rate as usize / 10).max(1);
    let mut steps = Vec::new();
    for chunk in samples.chunks(step) {
        if chunk.len() < step {
            break;
        }
        let mut sum = 0.0;
        for &(l, r) in chunk {
            for (x, f) in [l, r].into_iter().zip(&mut filters) {
                let shelved = f[0].run(x);
                let y = f[1].run(shelved);
                sum += y * y;
            }
        }
        steps.push(sum / step as f64);
    }
    let blocks: Vec<f64> = steps
        .windows(4)
        .map(|w| w.iter().sum::<f64>() / 4.0)
        .collect();
    let loudness = |p: f64| -0.691 + db(p);
    let gated = |floor: f64| {
        let kept: Vec<f64> = blocks
            .iter()
            .copied()
            .filter(|&p| loudness(p) > floor)
            .collect();
        kept.iter().sum::<f64>() / kept.len() as f64
    };
    let ungated = gated(-70.0);
    let lufs = loudness(gated(loudness(ungated) - 10.0));
    Level {
        seconds: samples.len() as f64 / rate as f64,
        peak: 20.0 * peak.log10(),
        rms,
        lufs,
    }
}

fn frames(s: &str) -> Result<(u64, u64), String> {
    let bad = || format!("--frames {s}: want F..G");
    let (a, b) = s.split_once("..").ok_or_else(bad)?;
    let (a, b) = (a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?);
    if a >= b {
        return Err(bad());
    }
    Ok((a, b))
}

fn run() -> Result<(), String> {
    let (mut files, mut span) = (Vec::new(), None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => span = Some(frames(&args.next().ok_or("--frames needs F..G")?)?),
            _ if a.starts_with("--") => return Err(format!("unknown option {a}")),
            _ => files.push(a),
        }
    }
    if files.is_empty() || files.len() > 2 {
        return Err("usage: loudness A.wav [B.wav] [--frames F..G]".into());
    }
    let mut levels = Vec::new();
    for path in &files {
        let w = read(path)?;
        let at =
            |f: u64| ((f as f64 * FRAME_SECONDS * w.rate as f64) as usize).min(w.samples.len());
        let (from, to) = span.map_or((0, w.samples.len()), |(a, b)| (at(a), at(b)));
        let scaled: Vec<(f64, f64)> = w.samples[from..to]
            .iter()
            .map(|&(l, r)| (l as f64 / 32768.0, r as f64 / 32768.0))
            .collect();
        let l = level(&scaled, w.rate);
        println!(
            "{path}: {} Hz, {:.1} s, peak {:.2} dBFS, RMS {:.2} dBFS, {:.2} LUFS",
            w.rate, l.seconds, l.peak, l.rms, l.lufs
        );
        levels.push(l);
    }
    if let [a, b] = &levels[..] {
        println!(
            "B - A: peak {:+.2} dB, RMS {:+.2} dB, {:+.2} LU",
            b.peak - a.peak,
            b.rms - a.rms,
            b.lufs - a.lufs
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loudness: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, amp: f64, seconds: f64) -> Vec<(f64, f64)> {
        (0..(rate as f64 * seconds) as usize)
            .map(|i| {
                let x = amp * (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin();
                (x, x)
            })
            .collect()
    }

    #[test]
    fn k_weighting_at_48k_is_the_standards() {
        let [s, h] = k_weighting(48_000.0);
        let close = |a: f64, b: f64| assert!((a - b).abs() < 1e-8, "{a} vs {b}");
        close(s.b[0], 1.535_124_859_586_97);
        close(s.b[1], -2.691_696_189_406_38);
        close(s.b[2], 1.198_392_810_852_85);
        close(s.a[0], -1.690_659_293_182_41);
        close(s.a[1], 0.732_480_774_215_85);
        close(h.a[0], -1.990_047_454_833_98);
        close(h.a[1], 0.990_072_250_366_21);
    }

    #[test]
    fn a_full_scale_1k_sine_in_both_channels_is_0_lufs() {
        // BS.1770: a 0 dBFS 997 Hz sine in one channel reads -3.01 LUFS, so
        // in both 0. The weighting must hold at a GBA mixer's rates too.
        for rate in [13_379, 32_768, 48_000] {
            let l = level(&sine(rate, 997.0, 1.0, 5.0), rate);
            assert!(l.lufs.abs() < 0.1, "{rate} Hz: {} LUFS", l.lufs);
            assert!((l.rms + 3.01).abs() < 0.01, "{rate} Hz: RMS {}", l.rms);
            assert!(l.peak.abs() < 0.01);
        }
    }

    #[test]
    fn the_gate_ignores_silence() {
        let rate = 32_768;
        let mut s = sine(rate, 997.0, 0.1, 5.0);
        s.extend(std::iter::repeat_n((0.0, 0.0), rate as usize * 20));
        // Three blocks straddle the end, so a little under -20; the
        // ungated power over all 25 s would read 7 dB under.
        let lufs = level(&s, rate).lufs;
        assert!((-20.5..-19.9).contains(&lufs), "{lufs}");
    }
}
