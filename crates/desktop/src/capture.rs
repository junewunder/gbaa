//! [`Capture`]: what a headless run keeps of the sound for `--wav`,
//! `--fifo-dump`, `--psg-dump`, `--audio-hash` and `--audio-log` (docs/sound-design.md).

use runtime::AudioChunk;
use std::fmt::Write as _;
use std::path::Path;

/// The machine's clock, in cycles a second.
pub const CLOCK_HZ: u32 = 1 << 24;

/// One frame's line of the audio log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub records: u64,
    pub restarts: Vec<u64>,
    pub swaps: Vec<u64>,
}

/// The sound stream, as handed over chunk by chunk.
#[derive(Clone, Debug, Default)]
pub struct Capture {
    /// FIFO records as bytes, `A0 B0 A1 B1 …`.
    pub fifo: Vec<u8>,
    /// The PSG's samples as bytes, `L0 R0 L1 R1 …`, each `i16` LE.
    pub psg: Vec<u8>,
    /// PSG samples the runtime dropped.
    pub psg_dropped: u64,
    /// The mapped samples, if kept (`--wav`).
    pub samples: Option<Vec<(i16, i16)>>,
    /// Records ever, dropped ones excluded.
    pub records: u64,
    /// Records the runtime dropped (its buffer full).
    pub dropped: u64,
    /// Cycles per record, from the last chunk that had one.
    pub period: u32,
    /// Per frame: records, restart indices and swap indices (global).
    pub frames: Vec<Frame>,
    /// The frame being collected.
    frame: Frame,
}

impl Capture {
    /// Keep the mapped samples too (for a WAV).
    pub fn with_samples(self) -> Capture {
        Capture {
            samples: Some(Vec::new()),
            ..self
        }
    }

    pub fn add(&mut self, c: &AudioChunk<'_>) {
        let at = |i: &u16| self.records + *i as u64;
        self.frame.restarts.extend(c.restarts.iter().map(at));
        self.frame.swaps.extend(c.swaps.iter().map(at));
        for &(a, b) in c.fifo {
            self.fifo.extend([a as u8, b as u8]);
        }
        if let Some(s) = &mut self.samples {
            s.extend_from_slice(c.samples);
        }
        self.records += c.fifo.len() as u64;
        self.frame.records += c.fifo.len() as u64;
        self.dropped += c.dropped as u64;
        for &(l, r) in c.psg {
            self.psg.extend(l.to_le_bytes());
            self.psg.extend(r.to_le_bytes());
        }
        self.psg_dropped += c.psg_dropped as u64;
        if c.period != 0 {
            self.period = c.period;
        }
    }

    /// A frame was presented: close its line.
    pub fn end_frame(&mut self) {
        self.frames.push(core::mem::take(&mut self.frame));
    }

    /// FNV-1a 64 over the FIFO dump.
    pub fn hash(&self) -> u64 {
        fnv1a(&self.fifo)
    }

    /// The sample rate in Hz, rounded, from the period.
    pub fn rate(&self) -> u32 {
        (CLOCK_HZ + self.period / 2)
            .checked_div(self.period)
            .unwrap_or(0)
    }

    /// One line per frame: `frame records`, then each restart as `r<index>`
    /// and each swap as `s<index>` (sample indices from the start).
    pub fn log(&self) -> String {
        let mut s = String::new();
        for (i, f) in self.frames.iter().enumerate() {
            let _ = write!(s, "{i} {}", f.records);
            for r in &f.restarts {
                let _ = write!(s, " r{r}");
            }
            for w in &f.swaps {
                let _ = write!(s, " s{w}");
            }
            s.push('\n');
        }
        s
    }
}

pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A sample in −1..1 as 16-bit PCM, on the runtime's scale (1.0 is 32768),
/// clipped.
pub fn pcm16(v: f32) -> i16 {
    (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

/// `samples` as a 16-bit stereo PCM WAV at `rate` Hz.
pub fn wav(samples: &[(i16, i16)], rate: u32) -> Vec<u8> {
    let data = samples.len() as u32 * 4;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&2u16.to_le_bytes()); // channels
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes()); // bytes a second
    w.extend_from_slice(&4u16.to_le_bytes()); // bytes a frame
    w.extend_from_slice(&16u16.to_le_bytes()); // bits a sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for &(l, r) in samples {
        w.extend_from_slice(&l.to_le_bytes());
        w.extend_from_slice(&r.to_le_bytes());
    }
    w
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}
