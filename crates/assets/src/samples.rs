//! m4a's instruments (docs/asset-extraction-design.md, "Samples and
//! voicegroups"): voicegroups (tables of 12-byte `ToneData`), the
//! direct-sound `WaveData` they play (to WAV, DPCM decoded), programmable
//! waves (to `.pcm`), and a soundfont of each voicegroup.
//!
//! **Finding:** every `WaveData` holds one sample past its `size` (m4a's
//! mixer reads it when interpolating; pret keeps it in the `.wav` and gives
//! `size` in wav2agb's `agbl` chunk). So a WAV here holds `size + 1`
//! samples and an `agbl` of `size`, which rebuilds all of pret's `.bin`s.

use std::collections::{BTreeMap, BTreeSet};

use crate::m4a::{Cmd, Mem, Song};
use crate::sf2;

/// A `ToneData`: type, key, length, pan or sweep, the wave (or nested
/// voicegroup) pointer, and the envelope (or keysplit table pointer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tone {
    pub kind: u8,
    pub key: u8,
    pub length: u8,
    pub pan_sweep: u8,
    pub wav: u32,
    pub adsr: [u8; 4],
}

impl Tone {
    pub fn read(mem: Mem, a: u32) -> Option<Tone> {
        Some(Tone {
            kind: mem.u8(a)?,
            key: mem.u8(a + 1)?,
            length: mem.u8(a + 2)?,
            pan_sweep: mem.u8(a + 3)?,
            wav: mem.u32(a + 4)?,
            adsr: [
                mem.u8(a + 8)?,
                mem.u8(a + 9)?,
                mem.u8(a + 10)?,
                mem.u8(a + 11)?,
            ],
        })
    }

    /// The pan byte (`0x80 | pan`, 0 for none): the fourth for direct
    /// sound, the third for the PSG channels (whose fourth is square 1's
    /// sweep).
    pub fn pan(&self) -> u8 {
        if self.is_direct() {
            self.pan_sweep
        } else {
            self.length
        }
    }

    pub fn is_direct(&self) -> bool {
        self.kind & 0xC7 == 0
    }

    /// The keysplit table's pointer, in the envelope's place.
    pub fn split_table(&self) -> u32 {
        u32::from_le_bytes(self.adsr)
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            0x40 => "keysplit",
            0x80 => "drumset",
            k if k & 0xC7 == 0 => "directsound",
            k => match k & 7 {
                1 => "square_1",
                2 => "square_2",
                3 => "programmable_wave",
                4 => "noise",
                _ => "unknown",
            },
        }
    }
}

/// A direct-sound `WaveData`: flags (DPCM in bit 0, loop in bit 30),
/// pitch (`rate * 1024`), loop start, size, and `size + 1` samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wave {
    pub addr: u32,
    pub dpcm: bool,
    pub looped: bool,
    pub pitch: u32,
    pub loop_start: u32,
    pub size: u32,
    pub samples: Vec<i8>,
}

/// The DPCM step for each nibble (wav2agb's `dpcmLookupTable`).
pub const DPCM: [i8; 16] = [
    0, 1, 4, 9, 16, 25, 36, 49, -64, -49, -36, -25, -16, -9, -4, -1,
];

/// `n` samples from DPCM blocks of 33 bytes: a starting sample, then 64
/// steps, high nibble first (the first is 0 as wav2agb writes it).
pub fn decode_dpcm(bytes: &[u8], n: usize) -> Vec<i8> {
    let mut out = Vec::with_capacity(n);
    for block in bytes.chunks(33) {
        let mut s = block[0] as i8;
        for j in 0..64 {
            if out.len() == n {
                return out;
            }
            let b = block.get(1 + j / 2).copied().unwrap_or(0);
            let nib = if j % 2 == 0 { b >> 4 } else { b & 15 };
            s = s.wrapping_add(DPCM[nib as usize]);
            out.push(s);
        }
    }
    out.resize(n, 0);
    out
}

/// How many bytes wav2agb's DPCM (`--no-pad`) writes for `n` samples.
pub fn dpcm_len(n: usize) -> usize {
    n / 64 * 33
        + if n.is_multiple_of(64) {
            0
        } else {
            1 + n % 64 / 2
        }
}

impl Wave {
    pub fn read(mem: Mem, addr: u32) -> Result<Wave, String> {
        let r = |a| {
            mem.u32(a)
                .ok_or(format!("WaveData {addr:#010x} runs off the ROM"))
        };
        let flags = r(addr)?;
        let (pitch, loop_start, size) = (r(addr + 4)?, r(addr + 8)?, r(addr + 12)?);
        if flags & !0x4000_0001 != 0 || size > 0x40_0000 || pitch == 0 {
            return Err(format!(
                "{addr:#010x} isn't a WaveData (flags {flags:#x}, size {size})"
            ));
        }
        let dpcm = flags & 1 != 0;
        let n = size as usize + 1;
        let len = if dpcm { dpcm_len(n) } else { n };
        let data: Vec<u8> = (0..len as u32)
            .map(|i| {
                mem.u8(addr + 16 + i)
                    .ok_or(format!("{addr:#010x}: data runs off the ROM"))
            })
            .collect::<Result<_, _>>()?;
        let samples = if dpcm {
            decode_dpcm(&data, n)
        } else {
            data.iter().map(|&b| b as i8).collect()
        };
        Ok(Wave {
            addr,
            dpcm,
            looped: flags & 0x4000_0000 != 0,
            pitch,
            loop_start,
            size,
            samples,
        })
    }

    /// Where the next `WaveData` would start: past the data, 4-aligned.
    pub fn next(&self) -> u32 {
        let n = self.samples.len();
        let len = if self.dpcm { dpcm_len(n) } else { n } as u32;
        (self.addr + 16 + len + 3) & !3
    }

    /// 8-bit mono WAV at `pitch / 1024` Hz: `smpl` (root key 60, the loop),
    /// `agbp` (the exact pitch) and `agbl` (`size`), so wav2agb rebuilds it.
    pub fn to_wav(&self) -> Vec<u8> {
        let rate = (self.pitch + 512) / 1024;
        let mut fmt = Vec::new();
        fmt.extend(1u16.to_le_bytes());
        fmt.extend(1u16.to_le_bytes());
        fmt.extend(rate.to_le_bytes());
        fmt.extend(rate.to_le_bytes());
        fmt.extend(1u16.to_le_bytes());
        fmt.extend(8u16.to_le_bytes());
        let data: Vec<u8> = self.samples.iter().map(|&s| s as u8 ^ 0x80).collect();
        let mut smpl = Vec::new();
        let period = 1_000_000_000 / rate.max(1);
        for w in [0, 0, period, 60, 0, 0, 0, u32::from(self.looped), 0] {
            smpl.extend(w.to_le_bytes());
        }
        if self.looped {
            let last = self.samples.len() as u32 - 1;
            for w in [0, 0, self.loop_start, last, 0, 0] {
                smpl.extend(w.to_le_bytes());
            }
        }
        let mut body = b"WAVE".to_vec();
        for (id, b) in [
            (b"fmt ", fmt),
            (b"data", data),
            (b"smpl", smpl),
            (b"agbp", self.pitch.to_le_bytes().to_vec()),
            (b"agbl", self.size.to_le_bytes().to_vec()),
        ] {
            body.extend(id);
            body.extend((b.len() as u32).to_le_bytes());
            body.extend(&b);
            if b.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    /// The sample as the GBA plays it, 16-bit, reversed for a reverse tone.
    fn pcm16(&self, reverse: bool) -> Vec<i16> {
        let mut v: Vec<i16> = self.samples[..self.size as usize]
            .iter()
            .map(|&s| i16::from(s) << 8)
            .collect();
        if reverse {
            v.reverse();
        }
        v
    }
}

/// A voicegroup: the tones its songs select, by program (or by key, for
/// a drum set), and what was played with each.
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub tones: BTreeMap<u8, Tone>,
    /// Keys played, per program.
    pub keys: BTreeMap<u8, BTreeSet<u8>>,
    pub songs: BTreeSet<String>,
    /// For a cry table: every entry, not only those played.
    pub table: bool,
}

/// Each program each track selects, and the keys it plays with it
/// (KEYSH added).
pub fn programs(song: &Song) -> BTreeMap<u8, BTreeSet<u8>> {
    let mut out: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    for t in &song.tracks {
        let (mut voice, mut keysh) = (None, 0i32);
        for (_, c) in &t.events {
            match *c {
                Cmd::Voice(v) => {
                    voice = Some(v);
                    out.entry(v).or_default();
                }
                Cmd::KeySh(k) => keysh = i32::from(k as i8),
                Cmd::Note { key, .. } | Cmd::Tie { key, .. } => {
                    if let Some(v) = voice {
                        let k = (i32::from(key) + keysh).clamp(0, 127) as u8;
                        out.entry(v).or_default().insert(k);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Every voicegroup reached from `roots` (address, programs and keys
/// played): the roots, and the keysplit and drum-set groups their tones
/// point at, read to what was played.
pub fn walk(mem: Mem, roots: &BTreeMap<u32, Group>) -> Result<BTreeMap<u32, Group>, String> {
    let mut groups = roots.clone();
    let mut todo: Vec<u32> = roots.keys().copied().collect();
    while let Some(g) = todo.pop() {
        let wanted: Vec<(u8, BTreeSet<u8>)> = groups[&g]
            .keys
            .iter()
            .map(|(p, k)| (*p, k.clone()))
            .collect();
        for (p, keys) in wanted {
            let a = g + 12 * u32::from(p);
            let tone = Tone::read(mem, a).ok_or(format!("tone {a:#010x} runs off the ROM"))?;
            groups.get_mut(&g).unwrap().tones.insert(p, tone);
            // The nested group and the programs in it this tone plays.
            let nested: Vec<(u8, u8)> = match tone.kind {
                0x40 => keys
                    .iter()
                    .filter_map(|&k| Some((mem.u8(tone.split_table() + u32::from(k))?, k)))
                    .collect(),
                0x80 => keys.iter().map(|&k| (k, k)).collect(),
                _ => continue,
            };
            if !mem.holds(tone.wav) {
                return Err(format!(
                    "tone {a:#010x}: nested voicegroup {:#010x} isn't in ROM",
                    tone.wav
                ));
            }
            let songs = groups[&g].songs.clone();
            let sub = groups.entry(tone.wav).or_default();
            let mut grew = false;
            for (q, k) in nested {
                // A drum plays at its tone's own key; a split's at the note's.
                let key = if tone.kind == 0x80 {
                    Tone::read(mem, tone.wav + 12 * u32::from(q)).map_or(k, |t| t.key)
                } else {
                    k
                };
                grew |= sub.keys.entry(q).or_default().insert(key);
            }
            sub.songs.extend(songs);
            if grew && !todo.contains(&tone.wav) {
                todo.push(tone.wav);
            }
        }
    }
    Ok(groups)
}

/// Runs of at least `min` `ToneData` in a row whose waves are all DPCM
/// `WaveData` (Pokémon cry tables, which no song plays): address and
/// entry count. Found by their shape alone.
pub fn cry_tables(mem: Mem, min: usize) -> Vec<(u32, usize)> {
    let dpcm_tone = |a: u32| {
        Tone::read(mem, a).is_some_and(|t| {
            t.kind & 0xEF == 0x20
                && t.length == 0
                && mem.u32(t.wav).is_some_and(|f| f & !0x4000_0000 == 1)
                && Wave::read(mem, t.wav).is_ok()
        })
    };
    let end = mem.base + mem.bytes.len() as u32;
    let mut out = Vec::new();
    let mut a = mem.base;
    while a + 12 <= end {
        if !dpcm_tone(a) {
            a += 4;
            continue;
        }
        let mut n = 0;
        // A table is all forward cries or all reversed.
        let kind = mem.u8(a);
        while dpcm_tone(a + 12 * n as u32) && mem.u8(a + 12 * n as u32) == kind {
            n += 1;
        }
        if n >= min {
            out.push((a, n));
            a += 12 * n as u32;
        } else {
            a += 4;
        }
    }
    out
}

/// m4a's envelope as SF2 generators: attack, decay and release in
/// timecents, sustain in centibels. Direct sound counts per frame in
/// 0–255; the PSG channels step in 1/64 s, over 15 levels.
pub fn envelope(tone: &Tone) -> Vec<(u16, i16)> {
    const FRAME: f64 = 1.0 / 59.7275;
    let tc = |secs: f64| {
        if secs <= 0.001 {
            -12000
        } else {
            (1200.0 * secs.log2()).round().clamp(-12000.0, 8000.0) as i16
        }
    };
    let [a, d, s, r] = tone.adsr.map(f64::from);
    let (attack, decay, sustain, release) = if tone.is_direct() {
        let attack = if a >= 255.0 {
            0.0
        } else {
            255.0 / a.max(1.0) * FRAME
        };
        let level = s.max(1.0) / 255.0;
        let per_frame = |x: f64| {
            if x <= 0.0 {
                0.0
            } else {
                (x / 256.0).min(0.999)
            }
        };
        let frames = |x: f64, to: f64| {
            let f = per_frame(x);
            if f == 0.0 { 0.0 } else { to.ln() / f.ln() }
        };
        (
            attack,
            frames(d, level) * FRAME,
            200.0 * (255.0 / s.max(0.5)).log10(),
            frames(r, 1e-5) * FRAME,
        )
    } else {
        (
            a * 15.0 / 64.0,
            d * 15.0 / 64.0,
            200.0 * (15.0 / s.max(0.5)).log10(),
            r * 15.0 / 64.0,
        )
    };
    vec![
        (sf2::op::ATTACK_VOL_ENV, tc(attack)),
        (sf2::op::DECAY_VOL_ENV, tc(decay)),
        (
            sf2::op::SUSTAIN_VOL_ENV,
            sustain.round().clamp(0.0, 1440.0) as i16,
        ),
        (sf2::op::RELEASE_VOL_ENV, tc(release)),
    ]
}

/// Middle C's frequency, for the generated PSG waves.
const C4: f64 = 261.625_565;

/// The soundfont's samples, made once each: direct-sound waves (forward
/// and reversed), programmable waves, square duties and noise.
#[derive(Default)]
pub struct Bank {
    pub samples: Vec<sf2::Sample>,
    made: BTreeMap<(u32, u8), usize>,
}

impl Bank {
    /// The sample a tone plays, made if new; `None` for one with no sound.
    pub fn sample(&mut self, mem: Mem, tone: &Tone, waves: &BTreeMap<u32, Wave>) -> Option<usize> {
        let kind = if tone.is_direct() {
            tone.kind & 0x10
        } else {
            tone.kind & 7
        };
        let id = match kind {
            1 | 2 => (0, tone.wav as u8 & 3),
            4 => (1, tone.wav as u8 & 1),
            _ => (tone.wav, kind),
        };
        if let Some(&i) = self.made.get(&id) {
            return Some(i);
        }
        let s = if tone.is_direct() {
            let w = waves.get(&tone.wav)?;
            let rate = w.pitch as f64 / 1024.0;
            let r = rate.round().max(1.0);
            let reverse = tone.kind & 0x10 != 0;
            sf2::Sample {
                name: format!(
                    "w{:07x}{}",
                    w.addr & 0xFF_FFFF,
                    if reverse { "r" } else { "" }
                ),
                data: w.pcm16(reverse),
                rate: r as u32,
                root: 60,
                correction: (1200.0 * (rate / r).log2()).round() as i8,
                looped: (w.looped && !reverse).then_some((w.loop_start, w.size)),
            }
        } else {
            match kind {
                // One cycle of a square: 12.5%, 25%, 50% or 75% high.
                1 | 2 => {
                    let duty = [4, 8, 16, 24][usize::from(tone.wav as u8 & 3)];
                    let data = (0..32)
                        .map(|i| if i < duty { 0x3000 } else { -0x3000 })
                        .collect();
                    psg(format!("square{duty}"), data, 32)
                }
                // 4 bit samples, high nibble first.
                3 => {
                    let data = (0..32u32)
                        .map(|i| {
                            let b = mem.u8(tone.wav + i / 2).unwrap_or(0);
                            let n = if i % 2 == 0 { b >> 4 } else { b & 15 };
                            (i16::from(n) - 8) * 0x800
                        })
                        .collect();
                    psg(format!("wave{:07x}", tone.wav & 0xFF_FFFF), data, 32)
                }
                // The LFSR, 15 or 7 bits.
                4 => {
                    let short = tone.wav & 1 != 0;
                    let (mut lfsr, mut data) = (0x7FFFu16, Vec::new());
                    for _ in 0..if short { 127 } else { 32767 } {
                        let bit = (lfsr ^ (lfsr >> 1)) & 1;
                        lfsr = (lfsr >> 1) | (bit << 14);
                        if short {
                            lfsr = (lfsr & !0x40) | (bit << 6);
                        }
                        data.push(if lfsr & 1 == 0 { 0x3000 } else { -0x3000 });
                    }
                    let n = data.len();
                    let mut s = psg(format!("noise{}", if short { 7 } else { 15 }), data, n);
                    s.rate = 32768;
                    s
                }
                _ => return None,
            }
        };
        self.samples.push(s);
        self.made.insert(id, self.samples.len() - 1);
        Some(self.samples.len() - 1)
    }
}

/// A looped cycle of `period` samples pitched at middle C.
fn psg(name: String, data: Vec<i16>, period: usize) -> sf2::Sample {
    let len = data.len() as u32;
    sf2::Sample {
        name,
        data,
        rate: (C4 * period as f64).round() as u32,
        root: 60,
        correction: 0,
        looped: Some((0, len)),
    }
}

/// One tone's zone over `keys`: its sample, pan, envelope, and for a drum
/// (`fixed_key`) the pitch of its own key whatever key plays it.
pub fn zone(
    mem: Mem,
    bank: &mut Bank,
    waves: &BTreeMap<u32, Wave>,
    tone: &Tone,
    keys: (u8, u8),
    fixed_key: bool,
) -> Option<sf2::Zone> {
    let sample = bank.sample(mem, tone, waves)?;
    let mut gens = envelope(tone);
    let pan = tone.pan();
    if pan & 0x80 != 0 {
        let p = i16::from(pan & 0x7F) - 64;
        gens.push((sf2::op::PAN, p * 500 / 64));
    }
    if tone.is_direct() && tone.kind & 0x08 != 0 {
        // No resampling: the rate it was recorded at, for every key.
        gens.push((sf2::op::SCALE_TUNING, 0));
    } else if fixed_key {
        gens.push((sf2::op::SCALE_TUNING, 0));
        gens.push((sf2::op::COARSE_TUNE, i16::from(tone.key) - 60));
    }
    if bank.samples[sample].looped.is_some() {
        gens.push((sf2::op::SAMPLE_MODES, 1));
    }
    if !fixed_key {
        gens.push((sf2::op::OVERRIDING_ROOT_KEY, 60));
    }
    Some(sf2::Zone { keys, sample, gens })
}

/// A voicegroup's program as zones: one tone over every key, or a
/// keysplit's or drum set's tones over theirs.
pub fn program_zones(
    mem: Mem,
    bank: &mut Bank,
    waves: &BTreeMap<u32, Wave>,
    groups: &BTreeMap<u32, Group>,
    tone: &Tone,
) -> Vec<sf2::Zone> {
    let sub = |q: u8| groups.get(&tone.wav).and_then(|g| g.tones.get(&q));
    match tone.kind {
        0x40 => {
            // Runs of keys with the same entry; keys whose entry no song
            // played are left out.
            let mut out = Vec::new();
            let mut start = 0u8;
            let entry = |k: u8| mem.u8(tone.split_table() + u32::from(k));
            let keys: Vec<u8> = (0..=127).collect();
            for (i, &k) in keys.iter().enumerate() {
                let last = i + 1 == keys.len() || entry(k + 1) != entry(k);
                if last {
                    if let Some(t) = entry(k).and_then(sub)
                        && let Some(z) = zone(mem, bank, waves, t, (start, k), false)
                    {
                        out.push(z);
                    }
                    start = k.saturating_add(1);
                }
            }
            out
        }
        0x80 => groups
            .get(&tone.wav)
            .map(|g| {
                g.tones
                    .iter()
                    .filter_map(|(&k, t)| zone(mem, bank, waves, t, (k, k), true))
                    .collect()
            })
            .unwrap_or_default(),
        _ => zone(mem, bank, waves, tone, (0, 127), false)
            .into_iter()
            .collect(),
    }
}
