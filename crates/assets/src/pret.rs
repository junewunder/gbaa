//! `check-pret` for songs (docs/asset-extraction-design.md, "Checking
//! against pret"): each exported `.mid` through pret's own mid2agb with
//! `midi.cfg`'s flags, assembled, against pret's built `.s` assembled.
//!
//! - **exact**: the bytes are equal (labels as offsets, the voicegroup by
//!   name): our MIDI rebuilds the ROM's song, since pret's `.s` assembles to
//!   the ROM's bytes (checked too, as `pret = ROM`).
//! - **events**: both decoded and flattened (patterns played in, running
//!   status read) give the same commands at the same clocks, each clock's
//!   commands in any order, and the same loop: mid2agb only factored the
//!   song differently.
//!
//! mid2agb scales VOL by the song's `-V` (`v*<song>_mvl/mxv`); the MIDI
//! holds the stored byte, so the check first sets each CC 7 to the least
//! value that scales back to it, and drops the VOL mid2agb writes itself
//! ([`songs::AUTO_VOL`]).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use crate::m4a::{self, Cmd, Mem, ROM_BASE};
use crate::midi::{self, Smf};
use crate::mplay::{self, Asm};
use crate::songs;

/// `song_table.inc`'s entries in order: each one's header symbol.
pub fn song_table(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|l| l.trim().strip_prefix("song "))
        .map(|r| r.split(',').next().unwrap_or("").trim().to_string())
        .collect()
}

/// `midi.cfg`: each song's mid2agb flags, by song name.
pub fn midi_cfg(src: &str) -> HashMap<String, Vec<String>> {
    src.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(f, opts)| {
            let name = f.trim().trim_end_matches(".mid").to_string();
            (name, opts.split_whitespace().map(String::from).collect())
        })
        .collect()
}

/// `-V`'s master volume, 127 without one (mid2agb's default).
fn master_volume(flags: &[String]) -> i64 {
    flags
        .iter()
        .find_map(|f| {
            f.strip_prefix("-V")
                .or_else(|| f.strip_prefix("-v"))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(127)
}

/// Our file as mid2agb with `-V mvl` must read it: each CC 7 the least
/// value `v` with `v*mvl/127` the stored byte, and mid2agb's own VOLs
/// dropped.
pub fn for_mid2agb(smf: &Smf, mvl: i64) -> Smf {
    let mut out = smf.clone();
    for t in &mut out.tracks {
        let mut drop_next = false;
        t.retain_mut(|(_, ev)| {
            if ev.len() > 3
                && ev[0] == 0xFF
                && ev[1] == midi::TEXT
                && ev[3..] == *songs::AUTO_VOL.as_bytes()
            {
                drop_next = true;
                return false;
            }
            if ev.len() == 3 && ev[0] & 0xF0 == 0xB0 && ev[1] == 7 {
                if drop_next {
                    drop_next = false;
                    return false;
                }
                let want = i64::from(ev[2]);
                ev[2] = (0..=127).find(|v| v * mvl / 127 == want).unwrap_or(127) as u8;
            }
            true
        });
    }
    out
}

/// A song decoded from assembled bytes: its tracks' commands, each clock's
/// sorted, with their ends and loops.
type Flat = Vec<(Vec<(u32, Cmd)>, u32, Option<(u32, u32)>)>;

fn flatten(a: &Asm, header: &str) -> Result<Flat, String> {
    let &h = a.labels.get(header).ok_or(format!("no label {header}"))?;
    let song = m4a::decode_song(
        Mem {
            base: 0,
            bytes: &a.bytes,
        },
        h,
    )?;
    Ok(song
        .tracks
        .into_iter()
        .map(|t| {
            let mut e = t.events;
            e.sort();
            (e, t.end, t.loop_at)
        })
        .collect())
}

/// Where two flattened songs first differ.
fn first_difference(ours: &Flat, theirs: &Flat) -> String {
    if ours.len() != theirs.len() {
        return format!("{} tracks, pret {}", ours.len(), theirs.len());
    }
    for (i, (o, t)) in ours.iter().zip(theirs).enumerate() {
        if let Some(j) = (0..o.0.len().max(t.0.len())).find(|&j| o.0.get(j) != t.0.get(j)) {
            return format!("track {}: {:?}, pret {:?}", i + 1, o.0.get(j), t.0.get(j));
        }
        if (o.1, o.2) != (t.1, t.2) {
            return format!(
                "track {}: ends at {} (loop {:?}), pret at {} (loop {:?})",
                i + 1,
                o.1,
                o.2,
                t.1,
                t.2
            );
        }
    }
    "the same".into()
}

pub struct Report {
    pub songs: usize,
    pub exported: usize,
    pub exact: usize,
    pub events: usize,
    pub pret_rom: usize,
    pub lines: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        format!(
            "songs: {} of pret's {} exported; {} exact, {} equal as events; pret's .s is the ROM's bytes for {}\n",
            self.exported, self.songs, self.exact, self.events, self.pret_rom
        )
    }

    pub fn text(&self) -> String {
        let mut s = String::from(
            "# gbaa-assets check-pret songs: our .mid through mid2agb against pret's .s\n",
        );
        s += &self.summary();
        for l in &self.lines {
            s += l;
            s.push('\n');
        }
        s
    }
}

/// Our manifest's songs: header address to (indices, file).
fn manifest_songs(out: &Path) -> Result<BTreeMap<u32, (Vec<usize>, String)>, String> {
    let p = out.join("manifest.toml");
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let t: toml::Table = toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
    let songs = t
        .get("songs")
        .and_then(|s| s.as_table())
        .ok_or("manifest has no [songs]")?;
    let mut out = BTreeMap::new();
    for (k, v) in songs {
        let addr =
            u32::from_str_radix(k.trim_start_matches("0x"), 16).map_err(|e| format!("{k}: {e}"))?;
        let idx = v
            .get("index")
            .and_then(|i| i.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_integer())
                    .map(|x| x as usize)
                    .collect()
            })
            .unwrap_or_default();
        let file = v
            .get("file")
            .and_then(|f| f.as_str())
            .unwrap_or("")
            .to_string();
        out.insert(addr, (idx, file));
    }
    Ok(out)
}

pub fn check_songs(rom_bytes: &[u8], out: &Path, pret: &Path) -> Result<Report, String> {
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let table = song_table(&read(&pret.join("sound/song_table.inc"))?);
    let cfg = midi_cfg(&read(&pret.join("sound/songs/midi/midi.cfg"))?);
    let mut defs = HashMap::new();
    mplay::equs(&read(&pret.join("sound/MPlayDef.s"))?, &mut defs);
    let mid2agb = pret.join("tools/mid2agb/mid2agb");
    let ours = manifest_songs(out)?;
    let rom = Mem {
        base: ROM_BASE,
        bytes: rom_bytes,
    };
    let work = out.join("check/songs");
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;

    let real: Vec<&String> = {
        let mut v: Vec<&String> = table.iter().filter(|n| cfg.contains_key(*n)).collect();
        v.sort();
        v.dedup();
        v
    };
    let mut r = Report {
        songs: real.len(),
        exported: 0,
        exact: 0,
        events: 0,
        pret_rom: 0,
        lines: Vec::new(),
    };
    let mut covered = std::collections::HashSet::new();
    for (&addr, (indices, file)) in &ours {
        let Some(name) = indices
            .iter()
            .filter_map(|&i| table.get(i))
            .find(|n| cfg.contains_key(*n))
        else {
            r.lines
                .push(format!("{file}: no pret song at index {indices:?}"));
            continue;
        };
        covered.insert(name.clone());
        r.exported += 1;
        let line = match check_one(
            rom,
            addr,
            &out.join(file),
            name,
            &cfg[name],
            &defs,
            &mid2agb,
            &work,
            &pret.join("sound/songs/midi"),
        ) {
            Ok(c) => {
                r.pret_rom += usize::from(c.pret_rom);
                r.exact += usize::from(c.exact);
                r.events += usize::from(c.events);
                if c.exact {
                    continue;
                }
                format!("{name}: {}", c.why)
            }
            Err(e) => format!("{name}: error: {e}"),
        };
        r.lines.push(line);
    }
    for n in real {
        if !covered.contains(n) {
            r.lines.push(format!("{n}: not exported"));
        }
    }
    Ok(r)
}

struct One {
    exact: bool,
    events: bool,
    pret_rom: bool,
    why: String,
}

#[allow(clippy::too_many_arguments)]
fn check_one(
    rom: Mem,
    addr: u32,
    mid: &Path,
    name: &str,
    flags: &[String],
    defs: &HashMap<String, String>,
    mid2agb: &Path,
    work: &Path,
    pret_midi: &Path,
) -> Result<One, String> {
    let bytes = std::fs::read(mid).map_err(|e| format!("{}: {e}", mid.display()))?;
    let smf = Smf::parse(&bytes)?;
    let prepared = for_mid2agb(&smf, master_volume(flags));
    let in_mid = work.join(format!("{name}.mid"));
    let out_s = work.join(format!("{name}.s"));
    std::fs::write(&in_mid, prepared.to_bytes())
        .map_err(|e| format!("{}: {e}", in_mid.display()))?;
    let run = Command::new(mid2agb)
        .arg(&in_mid)
        .arg(&out_s)
        .args(flags)
        .output()
        .map_err(|e| format!("{}: {e}", mid2agb.display()))?;
    if !run.status.success() {
        return Err(format!(
            "mid2agb: {}",
            String::from_utf8_lossy(&run.stderr).trim()
        ));
    }
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let ours = mplay::assemble(&read(&out_s)?, defs).map_err(|e| format!("our .s: {e}"))?;
    let theirs = mplay::assemble(&read(&pret_midi.join(format!("{name}.s")))?, defs)
        .map_err(|e| format!("pret's .s: {e}"))?;
    let pret_rom = placed(&theirs, name, rom, addr);
    let exact = ours.bytes == theirs.bytes
        && ours
            .externs
            .iter()
            .map(|e| &e.1)
            .eq(theirs.externs.iter().map(|e| &e.1))
        && ours.labels.get(name) == theirs.labels.get(name);
    let (fo, ft) = (flatten(&ours, name)?, flatten(&theirs, name)?);
    let events = fo == ft;
    let why = if exact {
        String::new()
    } else if events {
        format!(
            "equal as events, bytes differ ({} vs pret's {})",
            ours.bytes.len(),
            theirs.bytes.len()
        )
    } else {
        first_difference(&fo, &ft)
    };
    let mut why = why;
    if !pret_rom {
        let _ = write!(why, "; pret's .s isn't the ROM's bytes");
    }
    Ok(One {
        exact,
        events,
        pret_rom,
        why,
    })
}

/// Whether `a`, placed so its header label lands at the ROM header
/// `addr`, is the ROM's bytes: label words relocated, the voicegroup word
/// the ROM header's.
fn placed(a: &Asm, name: &str, rom: Mem, addr: u32) -> bool {
    let Some(&h) = a.labels.get(name) else {
        return false;
    };
    let Some(base) = addr.checked_sub(h) else {
        return false;
    };
    let mut want = a.bytes.clone();
    for &p in &a.relocs {
        let p = p as usize;
        let v = u32::from_le_bytes([want[p], want[p + 1], want[p + 2], want[p + 3]]) + base;
        want[p..p + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (p, _) in &a.externs {
        let p = *p as usize;
        match rom.u32(base + p as u32) {
            Some(v) => want[p..p + 4].copy_from_slice(&v.to_le_bytes()),
            None => return false,
        }
    }
    (0..want.len() as u32).all(|i| rom.u8(base + i) == Some(want[i as usize]))
}

/// Each label in a `.inc` of `label::` then `.incbin "path"` lines, with
/// its path.
pub fn incbins(src: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut label = None;
    for l in src.lines().map(str::trim) {
        if let Some((n, _)) = l.split_once("::") {
            label = Some(n.to_string());
        } else if let Some(p) = l.strip_prefix(".incbin")
            && let Some(n) = label.take()
        {
            out.insert(n, p.trim().trim_matches('"').to_string());
        }
    }
    out
}

pub struct SampleReport {
    /// pret's direct-sound samples and programmable waves.
    pub samples: usize,
    pub waves: usize,
    /// Ours: exported, and of those joined to a pret file by name.
    pub exported: usize,
    pub joined: usize,
    /// Our `.wav` through wav2agb equals pret's `.bin` and the ROM's bytes.
    pub exact: usize,
    pub waves_exported: usize,
    pub waves_exact: usize,
    pub lines: Vec<String>,
}

impl SampleReport {
    pub fn summary(&self) -> String {
        format!(
            "samples: {} exported ({} joined to pret's), {} exact, of pret's {}; programmable waves: {} exported, {} equal pret's, of {}\n",
            self.exported,
            self.joined,
            self.exact,
            self.samples,
            self.waves_exported,
            self.waves_exact,
            self.waves
        )
    }

    pub fn text(&self) -> String {
        let mut s = String::from(
            "# gbaa-assets check-pret samples: our .wav through wav2agb against pret's .bin and the ROM\n",
        );
        s += &self.summary();
        for l in &self.lines {
            s += l;
            s.push('\n');
        }
        s
    }
}

/// Our samples (the manifest's `[samples]`) against pret's: each `.wav`
/// through wav2agb (`-b`, and `-c -l 1 --no-pad` for DPCM as pret builds
/// cries) against pret's `.bin` and the ROM's bytes at its address; each
/// `.pcm` against pret's.
pub fn check_samples(rom_bytes: &[u8], out: &Path, pret: &Path) -> Result<SampleReport, String> {
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let direct = incbins(&read(&pret.join("sound/direct_sound_data.inc"))?);
    let prog = incbins(&read(&pret.join("sound/programmable_wave_data.inc"))?);
    let manifest: toml::Table = toml::from_str(&read(&out.join("manifest.toml"))?)
        .map_err(|e| format!("manifest.toml: {e}"))?;
    let samples = manifest
        .get("samples")
        .and_then(|s| s.as_table())
        .ok_or("manifest has no [samples]: run `gbaa-assets samples` first")?;
    let wav2agb = pret.join("tools/wav2agb/wav2agb");
    let tmp = std::env::temp_dir().join(format!("gbaa-wav2agb-{}.bin", std::process::id()));
    let mut r = SampleReport {
        samples: direct.len(),
        waves: prog.len(),
        exported: 0,
        joined: 0,
        exact: 0,
        waves_exported: 0,
        waves_exact: 0,
        lines: Vec::new(),
    };
    let mut seen = BTreeMap::new();
    for (k, v) in samples {
        let addr =
            u32::from_str_radix(k.trim_start_matches("0x"), 16).map_err(|e| format!("{k}: {e}"))?;
        let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("");
        let file = v.get("file").and_then(|x| x.as_str()).unwrap_or("");
        let name = Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let ours = out.join(file);
        let at = (addr - ROM_BASE) as usize;
        if kind == "wave" {
            r.waves_exported += 1;
            let Some(path) = prog.get(&name) else {
                r.lines.push(format!("{name} ({k}): no pret file"));
                continue;
            };
            let a = std::fs::read(&ours).map_err(|e| format!("{}: {e}", ours.display()))?;
            let b = std::fs::read(pret.join(path)).map_err(|e| format!("{path}: {e}"))?;
            if a == b {
                r.waves_exact += 1;
            } else {
                r.lines.push(format!("{name}: .pcm differs from {path}"));
            }
            continue;
        }
        r.exported += 1;
        let Some(path) = direct.get(&name) else {
            r.lines.push(format!("{name} ({k}): no pret file"));
            continue;
        };
        r.joined += 1;
        seen.insert(name.clone(), ());
        let mut cmd = Command::new(&wav2agb);
        cmd.arg("-b");
        if kind == "dpcm" {
            cmd.args(["-c", "-l", "1", "--no-pad"]);
        }
        let run = cmd
            .arg(&ours)
            .arg(&tmp)
            .output()
            .map_err(|e| format!("{}: {e}", wav2agb.display()))?;
        if !run.status.success() {
            r.lines.push(format!(
                "{name}: wav2agb failed: {}",
                String::from_utf8_lossy(&run.stderr).trim()
            ));
            continue;
        }
        let built = std::fs::read(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let theirs = std::fs::read(pret.join(path)).map_err(|e| format!("{path}: {e}"))?;
        let in_rom = rom_bytes.get(at..at + built.len()) == Some(&built[..]);
        if built == theirs && in_rom {
            r.exact += 1;
        } else {
            let first = built.iter().zip(&theirs).position(|(a, b)| a != b);
            r.lines.push(format!(
                "{name}: ours {} bytes, pret's {}, first difference at {first:?}; {} the ROM's",
                built.len(),
                theirs.len(),
                if in_rom { "equals" } else { "differs from" }
            ));
        }
    }
    let _ = std::fs::remove_file(&tmp);
    let missing: Vec<&String> = direct.keys().filter(|n| !seen.contains_key(*n)).collect();
    r.lines.push(format!(
        "pret's samples not exported (no song or cry table reached them): {}",
        missing.len()
    ));
    for n in missing {
        r.lines.push(format!("  {n}"));
    }
    Ok(r)
}
