//! Songs to MIDI the way pret's mid2agb reads them back (its `midi.cpp`
//! and `agb.cpp`; docs/asset-extraction-design.md, "Songs to MIDI"):
//! format 1 at 24 ticks a beat, a conductor track with the tempo and the
//! loop markers, then one track a m4a track on its own channel.

use std::collections::{BTreeSet, HashMap};

use crate::m4a::{CLOCKS, Cmd, Song};
use crate::midi::{self, Smf};

/// m4a's clocks a beat, and so the file's ticks a beat.
pub const TICKS_PER_BEAT: u16 = 24;

/// The text event marking the VOL mid2agb writes by itself at a track's
/// start (`VOL 127*<song>_mvl/mxv`, when the track has none before its
/// first note): the CC 7 after it holds that stored volume, so a player
/// hears the song right, and `check-pret` drops it before mid2agb, which
/// writes it again.
pub const AUTO_VOL: &str = "mid2agb VOL";

/// A song's MIDI file, and what in it mid2agb can't turn back into the
/// ROM's bytes.
pub struct Midi {
    pub smf: Smf,
    pub odd: Vec<String>,
}

fn cc(ch: u8, n: u8, v: u8) -> Vec<u8> {
    vec![0xB0 | ch, n, v]
}

fn text(s: &str) -> Vec<u8> {
    midi::meta(midi::TEXT, s.as_bytes())
}

pub fn to_midi(song: &Song, name: &str) -> Midi {
    let bars = bars(song);
    let mut marks: Vec<Vec<(u32, u8)>> = marks(song, &bars)
        .into_iter()
        .map(|m| m.into_iter().map(|c| (c, 0)).collect())
        .collect();
    // mid2agb makes a PATT of each bar equal to an earlier one: where the
    // ROM didn't (pret's MIDI held a difference that doesn't reach the
    // ROM, as two volumes that scale to one byte), a marker with a value
    // of its own keeps the bar, and the bars the ROM made PATTs of it,
    // apart. One at a time, as each changes what mid2agb makes after.
    let first = song.tracks.iter().position(|t| {
        t.events
            .iter()
            .any(|e| matches!(e.1, Cmd::Note { .. } | Cmd::Tie { .. }))
    });
    let mut next = vec![1u8; song.tracks.len()];
    for _ in 0..256 {
        let midi = build(song, name, &bars, &marks);
        let extra = song.tracks.iter().enumerate().find_map(|(i, t)| {
            let made = crate::mid2agb::patterns(&midi.smf, i + 1, Some(i) == first);
            let rom: HashMap<u32, Option<u32>> = t.patt_defs.iter().copied().collect();
            made.into_iter()
                .find(|&(at, of)| rom.get(&at) != Some(&Some(of)))
                .map(|(at, _)| (i, rom.get(&at).copied().flatten().unwrap_or(at)))
        });
        let Some((i, bar)) = extra else {
            return midi;
        };
        if next[i] > 127 || marks[i].iter().any(|&(c, v)| c == bar && v != 0) {
            return midi;
        }
        let v = next[i];
        next[i] += 1;
        marks[i].push((bar, v));
        for &(at, def) in &song.tracks[i].patt_defs {
            if def == Some(bar) {
                marks[i].push((at, v));
            }
        }
    }
    build(song, name, &bars, &marks)
}

/// The song's MIDI with `bars`, and `marks` ([`MARK_CC`] with a value) in
/// each track.
fn build(song: &Song, name: &str, bars: &Bars, marks: &[Vec<(u32, u8)>]) -> Midi {
    let mut odd = Vec::new();
    let mut conductor: Vec<(u32, u8, Vec<u8>)> =
        vec![(0, 1, midi::meta(midi::TRACK_NAME, name.as_bytes()))];
    for &(at, len) in &bars.segs {
        if at == 0 && len == 96 {
            continue;
        }
        match time_signature(len) {
            Some(ts) => conductor.push((at, 1, midi::meta(midi::TIME_SIGNATURE, &ts))),
            None => odd.push(format!("bars of {len} clocks")),
        }
    }
    // The loop is one pair of markers for the whole song (mid2agb reads
    // them from the first track only): the first looping track's.
    let looping = song.tracks.iter().find_map(|t| t.loop_at);
    if let Some((start, end)) = looping {
        conductor.push((start, 0, midi::meta(midi::MARKER, b"[")));
        conductor.push((end, 0, midi::meta(midi::MARKER, b"]")));
        if song.tracks.iter().any(|t| t.loop_at != Some((start, end))) {
            odd.push("tracks loop at different places".into());
        }
    }
    let mut tracks = Vec::new();
    if song.tracks.len() > 16 {
        odd.push("more than 16 tracks".into());
    }
    for (i, t) in song.tracks.iter().enumerate() {
        let ch = (i % 16) as u8;
        let n = i + 1;
        // (tick, order, event): note-offs (order 0) before the rest at a
        // tick, the rest as the ROM has them.
        let mut evs: Vec<(u32, u8, Vec<u8>)> = vec![(
            0,
            1,
            midi::meta(midi::TRACK_NAME, format!("track {n}").as_bytes()),
        )];
        for &(m, v) in &marks[i] {
            evs.push((m, 1, cc(ch, MARK_CC, v)));
        }
        let mut held: Vec<u8> = Vec::new();
        let mut first_keysh = true;
        let mut any_note = false;
        for (j, (tick, cmd)) in t.events.iter().enumerate() {
            let tick = *tick;
            let mut push = |e: Vec<u8>| evs.push((tick, 1, e));
            match *cmd {
                Cmd::Note { key, vel, len } => {
                    any_note = true;
                    push(vec![0x90 | ch, key, vel.max(1)]);
                    evs.push((tick + u32::from(len), 0, vec![0x80 | ch, key, 0]));
                }
                Cmd::Tie { key, vel } => {
                    any_note = true;
                    if held.contains(&key) {
                        odd.push(format!("track {n}: TIE on a held key"));
                    }
                    held.push(key);
                    push(vec![0x90 | ch, key, vel.max(1)]);
                }
                Cmd::Eot { key } => match held.iter().position(|&k| k == key) {
                    Some(p) => {
                        held.remove(p);
                        evs.push((tick, 0, vec![0x80 | ch, key, 0]));
                    }
                    None => odd.push(format!("track {n}: EOT of a key not held")),
                },
                Cmd::Voice(v) => push(vec![0xC0 | ch, v]),
                Cmd::Vol(v) => {
                    // VOL then KEYSH at the start is mid2agb's own VOL.
                    if j == 0 && matches!(t.events.get(1), Some((_, Cmd::KeySh(0)))) {
                        push(text(AUTO_VOL));
                    }
                    push(cc(ch, 7, v));
                }
                Cmd::Pan(v) => push(cc(ch, 10, v)),
                Cmd::Bend(v) => push(vec![0xE0 | ch, 0, v]),
                Cmd::BendR(v) => push(cc(ch, 20, v)),
                Cmd::Lfos(v) => push(cc(ch, 21, v)),
                Cmd::Modt(v) => push(cc(ch, 22, v)),
                Cmd::Tune(v) => push(cc(ch, 24, v)),
                Cmd::Lfodl(v) => push(cc(ch, 26, v)),
                Cmd::Mod(v) => push(cc(ch, 1, v)),
                Cmd::Prio(v) => push(cc(ch, 33, v)),
                Cmd::KeySh(0) if first_keysh && !any_note => {
                    // mid2agb writes `KEYSH , <song>_key+0` itself.
                    first_keysh = false;
                }
                Cmd::KeySh(v) => {
                    odd.push(format!("track {n}: KEYSH {v}"));
                    push(text(&format!("KEYSH {v}")));
                }
                Cmd::Tempo(v) => {
                    if i != 0 {
                        odd.push(format!("track {n}: TEMPO"));
                    }
                    if v == 0 {
                        odd.push(format!("track {n}: TEMPO 0"));
                        continue;
                    }
                    let us = (60_000_000.0 / (2.0 * f64::from(v))).round() as u32;
                    conductor.push((tick, 1, midi::meta(midi::TEMPO, &us.to_be_bytes()[1..])));
                }
                Cmd::Xcmd(x @ (8 | 9), ref a) => {
                    push(cc(ch, 30, x));
                    push(cc(ch, 29, a[0]));
                }
                Cmd::Xcmd(x, ref a) => push(text(&format!("XCMD {x:#04x} {a:02x?}"))),
                Cmd::MemAcc(op, addr, data) if op < 6 => {
                    push(cc(ch, 13, op));
                    push(cc(ch, 14, addr));
                    push(cc(ch, 12, data));
                }
                Cmd::MemAcc(op, addr, data) => {
                    push(text(&format!("MEMACC {op} {addr:#04x} {data}")));
                }
            }
        }
        for key in held {
            odd.push(format!("track {n}: TIE with no EOT"));
            evs.push((t.end, 0, vec![0x80 | ch, key, 0]));
        }
        if !any_note {
            odd.push(format!("track {n}: no notes (mid2agb drops it)"));
        }
        for o in &t.odd {
            odd.push(format!("track {n}: {o}"));
        }
        tracks.push(finish(evs, t.end));
    }
    let end = conductor.iter().map(|e| e.0).max().unwrap_or(0);
    let mut all = vec![finish(conductor, end)];
    all.extend(tracks);
    odd.dedup();
    Midi {
        smf: Smf {
            ticks_per_beat: TICKS_PER_BEAT,
            tracks: all,
        },
        odd,
    }
}

/// A track's events in order and its end-of-track at `end` or its last
/// event.
fn finish(mut evs: Vec<(u32, u8, Vec<u8>)>, end: u32) -> Vec<(u32, Vec<u8>)> {
    // Stable: same tick and order keeps the ROM's order.
    evs.sort_by_key(|e| (e.0, e.1));
    let last = evs.last().map_or(0, |e| e.0).max(end);
    let mut out: Vec<_> = evs.into_iter().map(|(t, _, e)| (t, e)).collect();
    out.push((last, midi::meta(midi::END_OF_TRACK, &[])));
    out
}

/// The longest wait mid2agb writes as one up to `x` clocks
/// (`g_noteDurationLUT`).
fn lut(x: u32) -> u32 {
    CLOCKS
        .iter()
        .map(|&c| u32::from(c))
        .rfind(|&c| c <= x)
        .unwrap_or(0)
}

/// Where mid2agb's `SplitTime` splits a wait of `d` clocks from `from`:
/// whole notes, then the longest wait it can write and the rest.
fn splits(from: u32, d: u32) -> Vec<u32> {
    if d == 0 {
        return Vec::new();
    }
    let whole = (d - 1) / 96;
    let mut out: Vec<u32> = (1..=whole).map(|i| from + 96 * i).collect();
    let r = d - 96 * whole;
    let l = lut(r);
    if l != r && l != 0 {
        out.push(from + 96 * whole + l);
    }
    out
}

/// Where mid2agb's bar lines ("whole notes") fall: each time signature
/// restarts them at its clock, every `len` clocks after until the next
/// one. `segs` is each (clock, len), the first at 0 (96 is 4/4, mid2agb's
/// default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bars {
    pub segs: Vec<(u32, u32)>,
}

impl Bars {
    fn seg(&self, c: u32) -> (u32, u32) {
        *self.segs.iter().rfind(|s| s.0 <= c).unwrap_or(&(0, 96))
    }

    /// Whether a bar line falls at `c`.
    pub fn has(&self, c: u32) -> bool {
        let (s, l) = self.seg(c);
        (c - s).is_multiple_of(l)
    }

    /// The bar lines strictly between `a` and `z`.
    fn lines_in(&self, a: u32, z: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for (i, &(s, l)) in self.segs.iter().enumerate() {
            let to = self.segs.get(i + 1).map_or(z, |n| n.0.min(z));
            let mut c = if a < s { s } else { s + (a - s) / l * l + l };
            while c < to {
                if c > a {
                    out.push(c);
                }
                c += l;
            }
        }
        out
    }
}

/// A track as [`bars`] tests a guess against it: its runs of waits with
/// no command between (from, to, where the waits inside end), its PATTs'
/// clocks and spans, and its end.
struct Shape {
    runs: Vec<(u32, u32, Vec<u32>)>,
    patts: Vec<u32>,
    /// Its patterns' spans, and where else than at a bar line one may end
    /// (the loop's markers, the track's end, and an EOT, which mid2agb
    /// ends a pattern at too).
    spans: Vec<(u32, u32)>,
    ends: Vec<u32>,
    end: u32,
}

fn shape(t: &crate::m4a::Track) -> Shape {
    let mut stops: BTreeSet<u32> = t.events.iter().map(|e| e.0).collect();
    stops.extend(&t.patts);
    if let Some((start, end)) = t.loop_at {
        stops.insert(start);
        stops.insert(end);
    }
    let mut runs = Vec::new();
    let mut i = 0;
    while i < t.waits.len() {
        let start = t.waits[i].0;
        let mut ends = vec![start + t.waits[i].1];
        while i + 1 < t.waits.len()
            && t.waits[i + 1].0 == *ends.last().unwrap()
            && !stops.contains(ends.last().unwrap())
        {
            i += 1;
            ends.push(t.waits[i].0 + t.waits[i].1);
        }
        i += 1;
        let end = ends.pop().unwrap();
        runs.push((start, end, ends));
    }
    let mut ends = vec![t.end];
    if let Some((start, end)) = t.loop_at {
        ends.extend([start, end]);
    }
    ends.extend(
        t.events
            .iter()
            .filter(|e| matches!(e.1, Cmd::Eot { .. }))
            .map(|e| e.0),
    );
    Shape {
        runs,
        patts: t.patts.clone(),
        spans: t.spans.clone(),
        ends,
        end: t.end,
    }
}

/// The first clock at or after `from` where mid2agb, given `b`'s bar
/// lines, would write the song differently: a pattern not a bar, or a
/// run of waits split other than the ROM has it (mid2agb splits a wait at
/// each bar line, then as `SplitTime` does; with `marks`, a split the
/// ROM has early is a split marker's, not a conflict). `runs` is every
/// track's, in order of their start.
fn conflict(
    b: &Bars,
    shapes: &[Shape],
    runs: &[(usize, usize)],
    from: u32,
    marks: bool,
) -> Option<u32> {
    let mut first: Option<u32> = None;
    for s in shapes {
        if let Some(&p) = s.patts.iter().find(|&&p| p >= from && !b.has(p)) {
            first = Some(first.map_or(p, |f| f.min(p)));
        }
        // A pattern is a bar: from a line to the next, or to a loop
        // marker, an EOT or the end.
        for &(a, z) in &s.spans {
            if z <= from {
                continue;
            }
            let c = if !b.has(a) {
                Some(a)
            } else if let Some(&l) = b.lines_in(a, z).first() {
                Some(l)
            } else if !b.has(z) && !s.ends.contains(&z) {
                Some(z)
            } else {
                None
            };
            if let Some(c) = c {
                first = Some(first.map_or(c, |f| f.min(c)));
            }
        }
    }
    for &(t, r) in runs {
        let (a, z, ref ends) = shapes[t].runs[r];
        if first.is_some_and(|f| a >= f) {
            break;
        }
        if z <= from {
            continue;
        }
        if let Err(c) = split_run(b, a, z, z.min(shapes[t].end), ends, marks) {
            first = Some(first.map_or(c, |f| f.min(c)));
        }
    }
    first
}

/// How mid2agb, given `b`'s bar lines, splits a run of waits from `a` to
/// `z` (bar lines up to `to`) where the ROM split it at `ends`: the
/// split markers ([`MARK_CC`]) it needs where the ROM splits early, or
/// the first clock mid2agb splits at and the ROM doesn't (and, without
/// `marks`, the first the ROM splits at and mid2agb doesn't).
fn split_run(
    b: &Bars,
    a: u32,
    z: u32,
    to: u32,
    ends: &[u32],
    marks: bool,
) -> Result<Vec<u32>, u32> {
    let mut out = Vec::new();
    let mut ends = ends.iter().copied().chain([z]).peekable();
    let mut at = a;
    for l in b.lines_in(a, to).into_iter().chain([z]) {
        // mid2agb's splits from `at` to the line, restarted at a marker.
        'seg: loop {
            for w in splits(at, l - at).into_iter().chain([l]) {
                match ends.peek().copied() {
                    Some(e) if e == w => {
                        ends.next();
                    }
                    Some(e) if e < w && marks => {
                        ends.next();
                        out.push(e);
                        at = e;
                        continue 'seg;
                    }
                    Some(e) => return Err(w.min(e)),
                    None => return Err(w),
                }
            }
            break;
        }
        at = l;
    }
    Ok(out)
}

/// The controller (undefined in General MIDI) whose events mark a wait
/// mid2agb must split with no command there: it writes them as nothing
/// but a split, as pret's MIDIs' stray controllers.
pub const MARK_CC: u8 = 3;

/// Where each track needs a split marker ([`MARK_CC`]) under `b`.
pub fn marks(song: &Song, b: &Bars) -> Vec<Vec<u32>> {
    song.tracks
        .iter()
        .map(|t| {
            let s = shape(t);
            s.runs
                .iter()
                .flat_map(|(a, z, ends)| {
                    split_run(b, *a, *z, (*z).min(s.end), ends, true).unwrap_or_default()
                })
                .collect()
        })
        .collect()
}

/// The song's bar lines, which the MIDI's time signatures give mid2agb:
/// its patterns (PATT) are a bar each and it splits waits at them, so the
/// ROM's waits and PATTs show them. Found left to right: from the first
/// clock the bars so far can't explain, a time signature restarts them at
/// a place every track has a boundary at. Of the lengths a time signature
/// gives, the one that explains the rest of the song is taken (4/4, else
/// the shortest), else one a 4/4 restart after explains the rest with,
/// else the one that explains furthest.
pub fn bars(song: &Song) -> Bars {
    let strict = bars_with(song, false);
    if strict.1 {
        strict.0
    } else {
        bars_with(song, true).0
    }
}

/// [`bars`], with split markers allowed or not, and whether they explain
/// the whole song.
fn bars_with(song: &Song, marks: bool) -> (Bars, bool) {
    // A guess: its rank (explains the rest alone 2, with a 4/4 restart
    // after 1, neither 0, then the order within each), restarts, reach.
    type Best = ((u32, u32, u32), Vec<(u32, u32)>, u32);
    let shapes: Vec<Shape> = song.tracks.iter().map(shape).collect();
    let mut runs: Vec<(usize, usize)> = shapes
        .iter()
        .enumerate()
        .flat_map(|(t, s)| (0..s.runs.len()).map(move |r| (t, r)))
        .collect();
    runs.sort_by_key(|&(t, r)| shapes[t].runs[r].0);
    // Where every track still playing has a command or a wait boundary.
    let bound = |c: u32| {
        shapes.iter().all(|s| {
            c >= s.end
                || s.runs
                    .iter()
                    .all(|(a, z, ends)| c <= *a || c >= *z || ends.contains(&c))
        })
    };
    // Bar lengths a time signature gives, the usual first: n/4, n/8,
    // n/16 and n/32.
    let mut lens: Vec<u32> = vec![96, 48, 72, 144, 24, 192, 12];
    for (unit, most) in [(24, 12), (12, 12), (6, 16), (3, 12)] {
        for n in 1..=most {
            if !lens.contains(&(unit * n)) {
                lens.push(unit * n);
            }
        }
    }
    // How far `segs` explains the song (u32::MAX: all of it).
    let reach_of = |segs: &[(u32, u32)], from: u32| {
        conflict(
            &Bars {
                segs: segs.to_vec(),
            },
            &shapes,
            &runs,
            from,
            marks,
        )
        .unwrap_or(u32::MAX)
    };
    // Where a restart after `segs`, which explain up to `reach`, can go.
    let places = |segs: &[(u32, u32)], reach: u32| -> Vec<u32> {
        let last = segs.last().map_or(0, |s| s.0);
        if segs.is_empty() {
            return vec![0];
        }
        (last.max(reach.saturating_sub(384))..=reach)
            .filter(|&c| bound(c))
            .collect()
    };
    // `segs` with a restart at `r`.
    let restart = |segs: &[(u32, u32)], r: u32, len: u32| {
        let mut s = segs.to_vec();
        if s.last().is_some_and(|l| l.0 == r) {
            s.pop();
        }
        s.push((r, len));
        s
    };
    let mut segs: Vec<(u32, u32)> = Vec::new();
    let mut reach: u32 = 0;
    // A restart for each of 64 places is past any real MIDI.
    for _ in 0..64 {
        let at = places(&segs, reach);
        // Each length at its best place. Best is one that explains the
        // rest of the song (4/4, else the shortest bar), then one a 4/4
        // restart after explains the rest with (4/4 first, the restart
        // earliest), then the one that explains furthest.
        let mut best: Option<Best> = None;
        for (k, &len) in lens.iter().enumerate() {
            let Some((c, s)) = at
                .iter()
                .map(|&r| {
                    let s = restart(&segs, r, len);
                    (reach_of(&s, r), s)
                })
                .max_by_key(|(c, _)| *c)
            else {
                continue;
            };
            if c <= reach && !segs.is_empty() {
                continue;
            }
            let pref = u32::MAX - k as u32;
            let (key, s, c) = if c == u32::MAX {
                (
                    (
                        2,
                        if len == 96 {
                            u32::MAX
                        } else {
                            u32::MAX - 1 - len
                        },
                        0,
                    ),
                    s,
                    c,
                )
            } else if let Some(&r) = places(&s, c)
                .iter()
                .find(|&&r| reach_of(&restart(&s, r, 96), r) == u32::MAX)
            {
                ((1, pref, u32::MAX - r), restart(&s, r, 96), u32::MAX)
            } else {
                ((0, c, pref), s, c)
            };
            if best.as_ref().is_none_or(|b| key > b.0) {
                best = Some((key, s, c));
            }
        }
        let Some((_, s, c)) = best else { break };
        segs = s;
        reach = c;
        if c == u32::MAX {
            break;
        }
    }
    if segs.is_empty() {
        segs.push((0, 96));
    }
    (Bars { segs }, reach == u32::MAX)
}

/// A time signature (`FF 58`'s data) whose bar is `clocks`: mid2agb takes
/// a bar as `96 * n / 2^d` clocks.
fn time_signature(clocks: u32) -> Option<[u8; 4]> {
    [2u32, 3, 4, 5, 6, 1, 0].into_iter().find_map(|d| {
        let n = (clocks << d) / 96;
        ((clocks << d).is_multiple_of(96) && (1..=255).contains(&n))
            .then_some([n as u8, d as u8, 24, 8])
    })
}
