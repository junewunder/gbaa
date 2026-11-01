//! m4a (MusicPlayer2000, "Sappy"), the sound driver most GBA games use:
//! its song table and headers, and a decoder for its track bytecode that
//! follows `MPlayMain`'s rules (pret's `src/m4a_1.s` and `src/m4a.c`;
//! docs/research/asset-extraction.md, "Music: m4a").

use std::collections::HashMap;

/// Where the cartridge is mapped.
pub const ROM_BASE: u32 = 0x0800_0000;

/// Bytes at addresses from `base`: the ROM (based at [`ROM_BASE`]) or an
/// assembled song (based at 0, its words resolved to offsets).
#[derive(Clone, Copy)]
pub struct Mem<'a> {
    pub base: u32,
    pub bytes: &'a [u8],
}

impl Mem<'_> {
    pub fn u8(&self, a: u32) -> Option<u8> {
        let i = a.checked_sub(self.base)? as usize;
        self.bytes.get(i).copied()
    }

    pub fn u16(&self, a: u32) -> Option<u16> {
        Some(u16::from(self.u8(a)?) | u16::from(self.u8(a + 1)?) << 8)
    }

    pub fn u32(&self, a: u32) -> Option<u32> {
        Some(u32::from(self.u16(a)?) | u32::from(self.u16(a + 2)?) << 16)
    }

    pub fn holds(&self, a: u32) -> bool {
        self.u8(a).is_some()
    }
}

/// `gClockTable`: the clocks of waits W00–W96 (0x80–0xB0) and of notes
/// N01–N96 (0xD0–0xFF, from index 1).
pub const CLOCKS: [u8; 49] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 28,
    30, 32, 36, 40, 42, 44, 48, 52, 54, 56, 60, 64, 66, 68, 72, 76, 78, 80, 84, 88, 90, 92, 96,
];

/// A `SongHeader`: `trackCount, blockCount, priority, reverb`, the
/// voicegroup (`tone`) and a pointer to each track's bytecode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub addr: u32,
    pub block_count: u8,
    pub priority: u8,
    pub reverb: u8,
    pub tone: u32,
    pub tracks: Vec<u32>,
}

impl Header {
    /// The header at `addr`, read as it is (no check that it is one).
    pub fn read(mem: Mem, addr: u32) -> Option<Header> {
        let count = mem.u8(addr)?;
        let tracks = (0..u32::from(count))
            .map(|i| mem.u32(addr + 8 + 4 * i))
            .collect::<Option<Vec<_>>>()?;
        Some(Header {
            addr,
            block_count: mem.u8(addr + 1)?,
            priority: mem.u8(addr + 2)?,
            reverb: mem.u8(addr + 3)?,
            tone: mem.u32(addr + 4)?,
            tracks,
        })
    }

    /// The header at `addr` if it looks like one: aligned, at most 16
    /// tracks, and (unless it has none, as the table's dummy) its voicegroup
    /// aligned and every track in the ROM.
    fn valid(rom: Mem, addr: u32) -> Option<Header> {
        if addr & 3 != 0 {
            return None;
        }
        let h = Header::read(rom, addr)?;
        let ok = h.tracks.len() <= 16
            && (h.tracks.is_empty()
                || (h.tone & 3 == 0
                    && rom.holds(h.tone)
                    && h.tracks.iter().all(|&t| rom.holds(t))));
        ok.then_some(h)
    }
}

/// A `Song` table entry: its header and the music player numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub header: u32,
    pub ms: u16,
    pub me: u16,
}

/// The entry at `a`, if it is one by agbplay's rules (`MP2KScanner.cpp`):
/// a valid header, `ms == me`, a small player number.
fn entry(rom: Mem, a: u32) -> Option<Entry> {
    let e = Entry {
        header: rom.u32(a)?,
        ms: rom.u16(a + 4)?,
        me: rom.u16(a + 6)?,
    };
    (e.ms == e.me && e.ms < 32 && Header::valid(rom, e.header).is_some()).then_some(e)
}

#[derive(Clone, Debug)]
pub struct SongTable {
    pub addr: u32,
    pub entries: Vec<Entry>,
    /// Some ROM word points at the table's start (agbplay's last test).
    pub referenced: bool,
}

/// The song table holding `played`, a song header the game was seen to
/// start: every run of valid entries with one pointing at `played`, grown
/// both ways, the longest kept.
pub fn find_song_table(rom: Mem, played: u32) -> Option<SongTable> {
    let mut best: Option<(u32, u32)> = None;
    let end = rom.base + rom.bytes.len() as u32;
    let mut a = rom.base;
    while a + 8 <= end {
        if rom.u32(a) == Some(played) && entry(rom, a).is_some() {
            let mut start = a;
            while start >= rom.base + 8 && entry(rom, start - 8).is_some() {
                start -= 8;
            }
            let mut stop = a + 8;
            while entry(rom, stop).is_some() {
                stop += 8;
            }
            if best.is_none_or(|(s, e)| stop - start > e - s) {
                best = Some((start, stop));
            }
        }
        a += 4;
    }
    let (start, stop) = best?;
    let entries = (start..stop)
        .step_by(8)
        .map(|a| entry(rom, a))
        .collect::<Option<Vec<_>>>()?;
    let referenced = rom
        .bytes
        .as_chunks::<4>()
        .0
        .iter()
        .any(|w| u32::from_le_bytes(*w) == start);
    Some(SongTable {
        addr: start,
        entries,
        referenced,
    })
}

/// One decoded command. Arguments are the stored bytes: PAN, BEND and TUNE
/// centred on 0x40 (`c_v`), TEMPO half the beats a minute.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cmd {
    /// N01–N96: a note `len` clocks long, gate time added.
    Note {
        key: u8,
        vel: u8,
        len: u8,
    },
    /// TIE: a note held until the EOT of its key.
    Tie {
        key: u8,
        vel: u8,
    },
    Eot {
        key: u8,
    },
    Voice(u8),
    Vol(u8),
    Pan(u8),
    Bend(u8),
    BendR(u8),
    Lfos(u8),
    Lfodl(u8),
    Mod(u8),
    Modt(u8),
    Tune(u8),
    KeySh(u8),
    Tempo(u8),
    Prio(u8),
    /// XCMD's sub-command and its argument bytes.
    Xcmd(u8, Vec<u8>),
    /// MEMACC's operation, address in the memory area and data.
    MemAcc(u8, u8, u8),
}

/// A track decoded once through: its commands at their clocks, patterns
/// (PATT) flattened in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    pub events: Vec<(u32, Cmd)>,
    /// The clock of its FINE (or of what ended it).
    pub end: u32,
    /// A GOTO back: the clock it goes back to and its own clock. What
    /// follows it up to the FINE is never played, but mid2agb writes
    /// there what the MIDI holds after the loop's end (a held note's EOT),
    /// so it is decoded too.
    pub loop_at: Option<(u32, u32)>,
    /// Each wait command's clock and length, and each PATT's clock: where
    /// mid2agb split the waits shows where its bar lines were.
    pub waits: Vec<(u32, u32)>,
    pub patts: Vec<u32>,
    /// Each top-level PATT's clock and the clock its target was first
    /// played at (a pattern mid2agb wrote at that bar), if it was.
    pub patt_defs: Vec<(u32, Option<u32>)>,
    /// Each pattern's clocks played (from, to): each PATT's, and the
    /// first time through its target. mid2agb's patterns are a bar, from
    /// a bar line to the next (or to a loop marker or the end).
    pub spans: Vec<(u32, u32)>,
    /// What MIDI through mid2agb can't carry (REPT, nested PATT, ...).
    pub odd: Vec<String>,
}

const MAX_STEPS: usize = 1_000_000;

/// The track whose bytecode starts at `start`, decoded until its FINE or a
/// GOTO back to a place already played.
pub fn decode_track(mem: Mem, start: u32) -> Result<Track, String> {
    let mut t = Track::default();
    let mut pc = start;
    let mut clock = 0u32;
    let mut stack: Vec<u32> = Vec::new();
    // Each PATT being played: its clock, and when its target was first.
    let mut calls: Vec<(u32, Option<u32>)> = Vec::new();
    // Where each top-level command started, the first time it was played.
    let mut seen: HashMap<u32, u32> = HashMap::new();
    let (mut key, mut vel, mut status, mut rep_n) = (60u8, 127u8, 0u8, 0u8);
    let byte = |a: u32| {
        mem.u8(a)
            .ok_or_else(|| format!("track runs off at {a:#010x}"))
    };
    let word = |a: u32| {
        mem.u32(a)
            .ok_or_else(|| format!("track runs off at {a:#010x}"))
    };
    for _ in 0..MAX_STEPS {
        if stack.is_empty() {
            seen.entry(pc).or_insert(clock);
        }
        let b = byte(pc)?;
        let cmd = if b < 0x80 {
            if status == 0 {
                return Err(format!(
                    "argument byte with no running status at {pc:#010x}"
                ));
            }
            status
        } else {
            pc += 1;
            if b >= 0xBD {
                status = b;
            }
            b
        };
        let arg = |pc: &mut u32| -> Result<u8, String> {
            let v = byte(*pc)?;
            *pc += 1;
            Ok(v)
        };
        // An optional argument: the next byte if it isn't a command.
        let opt = |pc: &mut u32| -> Result<Option<u8>, String> {
            let v = byte(*pc)?;
            if v < 0x80 {
                *pc += 1;
                Ok(Some(v))
            } else {
                Ok(None)
            }
        };
        let ev = match cmd {
            0x80..=0xB0 => {
                let len = u32::from(CLOCKS[usize::from(cmd - 0x80)]);
                if len > 0 {
                    t.waits.push((clock, len));
                }
                clock += len;
                None
            }
            0xB1 => {
                t.end = clock;
                return Ok(t);
            }
            0xB2 => {
                let target = word(pc)?;
                match seen.get(&target) {
                    Some(&c) if stack.is_empty() && t.loop_at.is_none() => {
                        t.loop_at = Some((c, clock));
                        pc += 4;
                    }
                    Some(_) if stack.is_empty() => {
                        t.odd.push("a second GOTO back".into());
                        t.end = clock;
                        return Ok(t);
                    }
                    _ if target > pc => {
                        t.odd.push(format!("GOTO forward to {target:#010x}"));
                        pc = target;
                    }
                    _ => {
                        t.odd
                            .push(format!("GOTO to {target:#010x}, not a place played"));
                        t.end = clock;
                        return Ok(t);
                    }
                }
                None
            }
            0xB3 => {
                let target = word(pc)?;
                if stack.len() == 3 {
                    return Err(format!("PATT deeper than 3 at {pc:#010x}"));
                }
                if !stack.is_empty() {
                    t.odd.push("nested PATT".into());
                }
                stack.push(pc + 4);
                if stack.len() == 1 {
                    t.patt_defs.push((clock, seen.get(&target).copied()));
                }
                calls.push((clock, seen.get(&target).copied()));
                t.patts.push(clock);
                pc = target;
                None
            }
            0xB4 => {
                if let Some(r) = stack.pop() {
                    pc = r;
                    if let Some((from, first)) = calls.pop() {
                        t.spans.push((from, clock));
                        if let Some(f) = first {
                            t.spans.push((f, f + clock - from));
                        }
                    }
                }
                None
            }
            0xB5 => {
                // ply_rept: a count of 0 is a GOTO; else jump back until the
                // count is reached.
                let count = byte(pc)?;
                let target = word(pc + 1)?;
                t.odd.push("REPT".into());
                if count == 0 {
                    if let Some(&c) = seen.get(&target) {
                        t.loop_at = Some((c, clock));
                        t.end = clock;
                        return Ok(t);
                    }
                    pc = target;
                } else {
                    rep_n += 1;
                    if rep_n < count {
                        pc = target;
                    } else {
                        rep_n = 0;
                        pc += 5;
                    }
                }
                None
            }
            0xB9 => {
                let (op, addr, data) = (arg(&mut pc)?, arg(&mut pc)?, arg(&mut pc)?);
                if op >= 6 {
                    // A conditional jump; the decoder can't know the memory,
                    // so takes it as not taken.
                    t.odd.push(format!("MEMACC jump (op {op})"));
                    pc += 4;
                }
                Some(Cmd::MemAcc(op, addr, data))
            }
            0xBA => Some(Cmd::Prio(arg(&mut pc)?)),
            0xBB => Some(Cmd::Tempo(arg(&mut pc)?)),
            0xBC => Some(Cmd::KeySh(arg(&mut pc)?)),
            0xBD => Some(Cmd::Voice(arg(&mut pc)?)),
            0xBE => Some(Cmd::Vol(arg(&mut pc)?)),
            0xBF => Some(Cmd::Pan(arg(&mut pc)?)),
            0xC0 => Some(Cmd::Bend(arg(&mut pc)?)),
            0xC1 => Some(Cmd::BendR(arg(&mut pc)?)),
            0xC2 => Some(Cmd::Lfos(arg(&mut pc)?)),
            0xC3 => Some(Cmd::Lfodl(arg(&mut pc)?)),
            0xC4 => Some(Cmd::Mod(arg(&mut pc)?)),
            0xC5 => Some(Cmd::Modt(arg(&mut pc)?)),
            0xC8 => Some(Cmd::Tune(arg(&mut pc)?)),
            0xCD => {
                let n = arg(&mut pc)?;
                let len = match n {
                    1 | 0x0D => 4,
                    0x0C => 2,
                    2 | 4..=11 => 1,
                    _ => {
                        // ply_xxx: plays as FINE.
                        t.odd.push(format!("XCMD {n:#04x}"));
                        t.end = clock;
                        return Ok(t);
                    }
                };
                if n != 8 && n != 9 {
                    t.odd.push(format!("XCMD {n:#04x}"));
                }
                let args = (0..len).map(|_| arg(&mut pc)).collect::<Result<_, _>>()?;
                Some(Cmd::Xcmd(n, args))
            }
            0xCE => {
                if let Some(k) = opt(&mut pc)? {
                    key = k;
                }
                Some(Cmd::Eot { key })
            }
            0xCF..=0xFF => {
                let mut len = CLOCKS[usize::from(cmd - 0xCF)];
                if let Some(k) = opt(&mut pc)? {
                    key = k;
                    if let Some(v) = opt(&mut pc)? {
                        vel = v;
                        if let Some(g) = opt(&mut pc)? {
                            len = len.wrapping_add(g);
                        }
                    }
                }
                Some(if cmd == 0xCF {
                    Cmd::Tie { key, vel }
                } else {
                    Cmd::Note { key, vel, len }
                })
            }
            _ => {
                // 0xB6–0xB8, 0xC6, 0xC7, 0xC9–0xCC: ply_fine or ply_port.
                t.odd.push(format!("command {cmd:#04x}"));
                t.end = clock;
                return Ok(t);
            }
        };
        if let Some(ev) = ev {
            t.events.push((clock, ev));
        }
    }
    Err(format!(
        "track at {start:#010x} runs past {MAX_STEPS} commands"
    ))
}

/// A song: its header and each track decoded.
#[derive(Clone, Debug)]
pub struct Song {
    pub header: Header,
    pub tracks: Vec<Track>,
}

pub fn decode_song(mem: Mem, header: u32) -> Result<Song, String> {
    let header = Header::read(mem, header).ok_or("header runs off the end")?;
    let tracks = header
        .tracks
        .iter()
        .map(|&t| decode_track(mem, t))
        .collect::<Result<_, _>>()?;
    Ok(Song { header, tracks })
}
