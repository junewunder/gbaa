//! What pret's mid2agb makes of one track of a MIDI file, as far as its
//! patterns: its `ReadMidiTracks` steps (merge with the conductor's
//! events, bar lines, ties, `SplitTime`, waits) and then `Compress`, which
//! turns each bar equal to an earlier one into a PATT of it (`midi.cpp`).
//! The song export runs it on its own MIDI to find bars the ROM keeps
//! apart that mid2agb would make one pattern
//! (docs/asset-extraction-design.md, "Songs to MIDI").

use crate::midi::{self, Smf};

// mid2agb's `EventType`s; up to `PATTERN` (and `END`) end a pattern.
const END_OF_TIE: u8 = 0x01;
const LABEL: u8 = 0x11;
const LOOP_END: u8 = 0x12;
const LOOP_END_BEGIN: u8 = 0x13;
const LOOP_BEGIN: u8 = 0x14;
const ORIGINAL_TIME_SIGNATURE: u8 = 0x15;
const WHOLE_NOTE_MARK: u8 = 0x16;
const PATTERN: u8 = 0x17;
const TIME_SIGNATURE: u8 = 0x18;
const TEMPO: u8 = 0x19;
const INSTRUMENT_CHANGE: u8 = 0x21;
const CONTROLLER: u8 = 0x22;
const PITCH_BEND: u8 = 0x23;
const NOTE: u8 = 0x40;
const TIME_SPLIT: u8 = 0xFE;
const END: u8 = 0xFF;

/// mid2agb's `Event`, and the clock it started at (`time` becomes the
/// wait after it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ev {
    time: i32,
    kind: u8,
    note: u8,
    param1: u8,
    param2: i32,
    at: u32,
}

impl Ev {
    fn new(at: u32, kind: u8, note: u8, param1: u8, param2: i32) -> Ev {
        Ev {
            time: at as i32,
            kind,
            note,
            param1,
            param2,
            at,
        }
    }

    /// `operator==`: all but where it started.
    fn same(&self, o: &Ev) -> bool {
        (self.time, self.kind, self.note, self.param1, self.param2)
            == (o.time, o.kind, o.note, o.param1, o.param2)
    }
}

fn boundary(kind: u8) -> bool {
    kind == END || kind <= PATTERN
}

/// `EventCompare`: by time, then type (a note's plus its key), then an
/// EOT's key.
fn before(a: &Ev, b: &Ev) -> bool {
    let rank = |e: &Ev| u32::from(e.kind) + if e.kind == NOTE { u32::from(e.note) } else { 0 };
    if a.time != b.time {
        return a.time < b.time;
    }
    if rank(a) != rank(b) {
        return rank(a) < rank(b);
    }
    a.kind == END_OF_TIE && a.note < b.note
}

/// `g_noteDurationLUT`: the longest note mid2agb writes as one up to `x`.
fn lut(x: i32) -> i32 {
    if x < 0 {
        return x;
    }
    crate::m4a::CLOCKS
        .iter()
        .map(|&c| i32::from(c))
        .rfind(|&c| c <= x)
        .unwrap_or(0)
}

/// The conductor's events (`ReadSeqEvents`).
fn seq_events(t: &[(u32, Vec<u8>)]) -> Vec<Ev> {
    let mut out = Vec::new();
    let mut blocks = 0u8;
    for (at, e) in t {
        if e.len() < 3 || e[0] != 0xFF {
            continue;
        }
        let data = &e[3..];
        let kind = match e[1] {
            1..=7 => match data {
                b"[" => LOOP_BEGIN,
                b"][" => LOOP_END_BEGIN,
                b"]" => LOOP_END,
                b":" => LABEL,
                _ => continue,
            },
            midi::END_OF_TRACK => {
                out.push(Ev::new(*at, END, 0, 0, 0));
                break;
            }
            midi::TEMPO if data.len() == 3 => {
                let us = i32::from(data[0]) << 16 | i32::from(data[1]) << 8 | i32::from(data[2]);
                out.push(Ev::new(*at, TEMPO, 0, 0, us));
                continue;
            }
            midi::TIME_SIGNATURE if data.len() == 4 => {
                let ts = 96 * i32::from(data[0]) / (1 << data[1]);
                out.push(Ev::new(*at, TIME_SIGNATURE, 0, 0, ts));
                continue;
            }
            _ => continue,
        };
        out.push(Ev::new(*at, kind, 0, blocks, 0));
        blocks = blocks.wrapping_add(1);
    }
    out
}

/// A track's events on its channel (`ReadTrackEvents`): a note's length
/// is to the first note-off of its key after it.
fn track_events(t: &[(u32, Vec<u8>)]) -> Vec<Ev> {
    let mut out = Vec::new();
    for (i, (at, e)) in t.iter().enumerate() {
        match (e[0] & 0xF0, e.len()) {
            (0x90, 3) if e[2] != 0 => {
                let end = t[i + 1..]
                    .iter()
                    .find(|(_, o)| {
                        o.len() == 3
                            && o[1] == e[1]
                            && (o[0] & 0xF0 == 0x80 || (o[0] & 0xF0 == 0x90 && o[2] == 0))
                    })
                    .map_or(*at, |o| o.0);
                out.push(Ev::new(*at, NOTE, e[1], e[2], (end - at) as i32));
            }
            (0xB0, 3) => out.push(Ev::new(*at, CONTROLLER, 0, e[1], i32::from(e[2]))),
            (0xC0, 2) => out.push(Ev::new(*at, INSTRUMENT_CHANGE, 0, e[1], 0)),
            (0xE0, 3) => out.push(Ev::new(*at, PITCH_BEND, 0, e[1], i32::from(e[2]))),
            _ if e[0] == 0xFF && e[1] == midi::END_OF_TRACK => {
                out.push(Ev::new(*at, END, 0, 0, 0));
                break;
            }
            _ => {}
        }
    }
    out
}

/// The patterns mid2agb makes of track `n` of `smf` (the conductor is
/// track 0): each bar it writes as a PATT, by clock, and the clock of the
/// bar it calls. `first` is whether this is mid2agb's first track (the
/// only one it keeps the tempo in).
pub fn patterns(smf: &Smf, n: usize, first: bool) -> Vec<(u32, u32)> {
    let mut seq = seq_events(&smf.tracks[0]);
    if !first {
        seq.retain(|e| e.kind != TEMPO);
    }
    let track = track_events(&smf.tracks[n]);
    if seq.last().is_none_or(|e| e.kind != END) || track.last().is_none_or(|e| e.kind != END) {
        return Vec::new();
    }
    // MergeEvents.
    let (mut i, mut j) = (0, 0);
    let mut evs = Vec::new();
    while track[i].kind != END && seq[j].kind != END {
        if before(&track[i], &seq[j]) {
            evs.push(track[i]);
            i += 1;
        } else {
            evs.push(seq[j]);
            j += 1;
        }
    }
    evs.extend(track[i..].iter().filter(|e| e.kind != END));
    evs.extend(seq[j..].iter().filter(|e| e.kind != END));
    // The end of track with the later time.
    let (te, se) = (track[track.len() - 1], seq[seq.len() - 1]);
    evs.push(if before(&te, &se) { se } else { te });

    // InsertTimingEvents.
    let mut timed = Vec::new();
    let mut mark = Ev::new(0, TIME_SIGNATURE, 0, 0, 96);
    for e in evs {
        while before(&mark, &e) {
            timed.push(mark);
            mark.time += mark.param2;
            mark.at = mark.time as u32;
        }
        if e.kind == TIME_SIGNATURE {
            if first && e.param2 != mark.param2 {
                timed.push(Ev {
                    kind: ORIGINAL_TIME_SIGNATURE,
                    ..e
                });
            }
            mark.param2 = e.param2;
            mark.time = e.time + mark.param2;
            mark.at = mark.time as u32;
        }
        timed.push(e);
    }

    // CreateTies.
    let mut tied = Vec::new();
    for e in timed {
        if e.kind == NOTE && e.param2 > 96 {
            tied.push(Ev { param2: -1, ..e });
            let end = e.time + e.param2;
            tied.push(Ev::new(end as u32, END_OF_TIE, e.note, 0, 0));
        } else {
            tied.push(e);
        }
    }
    // std::stable_sort with EventCompare.
    tied.sort_by(|a, b| {
        if before(a, b) {
            std::cmp::Ordering::Less
        } else if before(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });

    // SplitTime.
    let mut evs = Vec::new();
    let mut time = 0i32;
    for e in tied {
        let mut diff = e.time - time;
        if diff > 96 {
            let whole = (diff - 1) / 96;
            diff -= 96 * whole;
            for _ in 0..whole {
                time += 96;
                evs.push(Ev::new(time as u32, TIME_SPLIT, 0, 0, 0));
            }
        }
        let l = lut(diff);
        if l != diff {
            evs.push(Ev::new((time + l) as u32, TIME_SPLIT, 0, 0, 0));
        }
        time = e.time;
        evs.push(e);
    }

    // CalculateWaits.
    let mut count = 0;
    for i in 0..evs.len() {
        if evs[i].kind == END {
            break;
        }
        evs[i].time = evs[i + 1].time - evs[i].time;
        if evs[i].kind == TIME_SIGNATURE {
            evs[i].kind = WHOLE_NOTE_MARK;
            evs[i].param2 = count;
            count += 1;
        }
    }

    // Compress.
    let mut out = Vec::new();
    let mut i = 0;
    while evs[i].kind != END {
        if evs[i].kind != WHOLE_NOTE_MARK {
            i += 1;
            continue;
        }
        if score(&evs, i) >= 6 {
            for j in i + 1..evs.len() {
                if evs[j].kind == WHOLE_NOTE_MARK && matches(&evs, i, j) {
                    evs[j].kind = PATTERN;
                    evs[i].param2 |= i32::MIN;
                    out.push((evs[j].at, evs[i].at));
                }
            }
        }
        i += 1;
    }
    out.sort_unstable();
    out
}

/// `CalculateCompressionScore`.
fn score(evs: &[Ev], index: usize) -> i32 {
    let mut score = 0;
    let mut last_param1 = evs[index].param1;
    let mut last_velocity = 0x80u8;
    let mut last_kind = evs[index].kind;
    let mut last_duration = i32::MIN;
    let mut last_note = 0x40u8;
    if evs[index].time > 0 {
        score += 1;
    }
    for e in evs[index + 1..].iter().take_while(|e| !boundary(e.kind)) {
        if e.kind == NOTE {
            let mut val = 0;
            if e.note != last_note {
                val += 1;
                last_note = e.note;
            }
            if e.param1 != last_velocity {
                val += 1;
                last_velocity = e.param1;
            }
            if lut(e.param2) != last_duration {
                val += 1;
                last_duration = lut(e.param2);
            }
            if e.param2 != last_duration {
                val += 1;
            }
            score += val.max(1);
        } else {
            last_duration = i32::MIN;
            if e.kind == last_kind {
                if (last_kind != CONTROLLER && last_kind != 0x25 && last_kind != END_OF_TIE)
                    || e.param1 == last_param1
                {
                    score += 1;
                } else {
                    score += 2;
                }
            } else {
                score += 2;
            }
        }
        last_param1 = e.param1;
        last_kind = e.kind;
        if e.time != 0 {
            score += 1;
        }
    }
    score
}

/// `IsCompressionMatch`: the bars from `a` and `b` up to their next
/// pattern boundary are the same.
fn matches(evs: &[Ev], a: usize, b: usize) -> bool {
    let (x, y) = (&evs[a], &evs[b]);
    if (x.kind, x.note, x.param1, x.time) != (y.kind, y.note, y.param1, y.time) {
        return false;
    }
    let (mut a, mut b) = (a + 1, b + 1);
    loop {
        match (evs.get(a), evs.get(b)) {
            (Some(x), Some(y)) if x.same(y) => {}
            _ => return false,
        }
        a += 1;
        b += 1;
        if evs.get(a).is_none_or(|e| boundary(e.kind)) {
            break;
        }
    }
    evs.get(b).is_some_and(|e| boundary(e.kind))
}
