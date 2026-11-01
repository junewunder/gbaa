//! Standard MIDI files: a writer, and a reader for the check. An event is
//! its raw bytes (status and data, or `FF type len data` for a meta event)
//! at an absolute tick.

/// A meta event's bytes.
pub fn meta(kind: u8, data: &[u8]) -> Vec<u8> {
    let mut v = vec![0xFF, kind];
    vlq(&mut v, data.len() as u32);
    v.extend_from_slice(data);
    v
}

pub const END_OF_TRACK: u8 = 0x2F;
pub const TEXT: u8 = 0x01;
pub const TRACK_NAME: u8 = 0x03;
pub const MARKER: u8 = 0x06;
pub const TEMPO: u8 = 0x51;
pub const TIME_SIGNATURE: u8 = 0x58;

/// A format-1 file: tracks of `(tick, event)`, each in tick order. Each
/// track's end-of-track event is its own, last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Smf {
    pub ticks_per_beat: u16,
    pub tracks: Vec<Vec<(u32, Vec<u8>)>>,
}

fn vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut bytes = vec![(v & 0x7F) as u8];
    v >>= 7;
    while v > 0 {
        bytes.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out.extend(bytes.iter().rev());
}

impl Smf {
    /// The file's bytes, with no running status.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = b"MThd".to_vec();
        out.extend(6u32.to_be_bytes());
        out.extend(1u16.to_be_bytes());
        out.extend((self.tracks.len() as u16).to_be_bytes());
        out.extend(self.ticks_per_beat.to_be_bytes());
        for t in &self.tracks {
            let mut body = Vec::new();
            let mut last = 0;
            for (tick, ev) in t {
                vlq(&mut body, tick - last);
                last = *tick;
                body.extend(ev);
            }
            out.extend(b"MTrk");
            out.extend((body.len() as u32).to_be_bytes());
            out.extend(body);
        }
        out
    }

    /// Reads a format-0 or 1 file, running status expanded.
    pub fn parse(b: &[u8]) -> Result<Smf, String> {
        let short = || "file ends early".to_string();
        let be32 = |i: usize| -> Result<u32, String> {
            let s = b.get(i..i + 4).ok_or_else(short)?;
            Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        };
        if b.get(0..4) != Some(b"MThd") || be32(4)? != 6 {
            return Err("not a MIDI file".into());
        }
        let ntracks = u16::from_be_bytes([b[10], b[11]]);
        let ticks_per_beat = u16::from_be_bytes([b[12], b[13]]);
        let mut i = 14;
        let mut tracks = Vec::new();
        for _ in 0..ntracks {
            if b.get(i..i + 4) != Some(b"MTrk") {
                return Err(format!("no track at {i}"));
            }
            let len = be32(i + 4)? as usize;
            let body = b.get(i + 8..i + 8 + len).ok_or_else(short)?;
            i += 8 + len;
            tracks.push(parse_track(body)?);
        }
        Ok(Smf {
            ticks_per_beat,
            tracks,
        })
    }
}

fn parse_track(b: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    let mut i = 0;
    let next = |i: &mut usize| -> Result<u8, String> {
        let v = *b.get(*i).ok_or("track ends early")?;
        *i += 1;
        Ok(v)
    };
    let read_vlq = |i: &mut usize| -> Result<u32, String> {
        let mut v = 0u32;
        loop {
            let c = next(i)?;
            v = v << 7 | u32::from(c & 0x7F);
            if c & 0x80 == 0 {
                return Ok(v);
            }
        }
    };
    let (mut tick, mut status) = (0u32, 0u8);
    let mut out = Vec::new();
    while i < b.len() {
        tick += read_vlq(&mut i)?;
        let mut s = next(&mut i)?;
        if s < 0x80 {
            i -= 1;
            s = status;
        }
        let ev = match s {
            0xFF => {
                let kind = next(&mut i)?;
                let len = read_vlq(&mut i)? as usize;
                let data = b.get(i..i + len).ok_or("meta runs off")?;
                i += len;
                meta(kind, data)
            }
            0xF0 | 0xF7 => {
                let len = read_vlq(&mut i)? as usize;
                i += len;
                continue;
            }
            0x80..=0xEF => {
                status = s;
                let n = if (0xC0..0xE0).contains(&s) { 1 } else { 2 };
                let mut ev = vec![s];
                for _ in 0..n {
                    ev.push(next(&mut i)?);
                }
                ev
            }
            _ => return Err(format!("bad status {s:#04x}")),
        };
        out.push((tick, ev));
    }
    Ok(out)
}
