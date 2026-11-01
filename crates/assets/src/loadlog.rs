//! The load-log check (assets/load-log, docs/asset-extraction-design.md,
//! Recording): a recording run's `loads.tsv` against what reached the
//! screen's memory along it (`reached.tsv`, the provenance spike's blobs),
//! and its `uses.tsv` against every tile shown on its last frame
//! (`shown.tsv`). All four are `gbaa-desktop --assets-log`'s.
//!
//! - Every compressed blob that reached palette, VRAM or OAM is the source
//!   of a decompression in the load log, and its length there (bytes the
//!   decoder read, rounded up to 4) is the source map's size for the
//!   symbol at that address, where the map has one: the whole blob.
//! - Every copied run, joined across gaps under 16 bytes as the spike's
//!   ranges are, is loaded whole: each of its bytes is in a load's ROM
//!   source or in a RAM source's ROM runs.
//! - Every tile shown on the last frame has a use with that frame as its
//!   last.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Copied ROM bytes this close together are one range, as the spike's.
const GAP: u32 = 16;

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

/// A load log's line, as far as the check needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadLine {
    pub frame: u64,
    pub op: String,
    pub src: u32,
    pub src_len: u32,
    /// A RAM source's ROM runs: (rom address, bytes, decompressed).
    pub runs: Vec<(u32, u32, bool)>,
}

impl LoadLine {
    fn decompressed(&self) -> bool {
        matches!(
            self.op.as_str(),
            "lz77" | "rl" | "huff" | "bitunpack" | "diff"
        )
    }
}

fn rows(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .skip(1)
        .map(|l| l.split('\t').collect())
}

pub fn parse_loads(text: &str) -> Result<Vec<LoadLine>, String> {
    rows(text)
        .map(|f| {
            let bad = || format!("loads.tsv: bad line {:?}", f.join("\t"));
            let [frame, op, src, src_len, _, _, runs] = f[..] else {
                return Err(bad());
            };
            let mut rs = Vec::new();
            for r in runs.split(',').filter(|r| *r != "-" && *r != "…") {
                let p: Vec<&str> = r.split('/').collect();
                let [rom, _, len, via] = p[..] else {
                    return Err(bad());
                };
                rs.push((
                    hex(rom).ok_or_else(bad)?,
                    len.parse().map_err(|_| bad())?,
                    via.split('+').any(|v| v == "lz77" || v == "unpack"),
                ));
            }
            Ok(LoadLine {
                frame: frame.parse().map_err(|_| bad())?,
                op: op.into(),
                src: hex(src).ok_or_else(bad)?,
                src_len: src_len.parse().map_err(|_| bad())?,
                runs: rs,
            })
        })
        .collect()
}

/// A run of ROM bytes: (start, bytes).
pub type Span = (u32, u32);

/// `reached.tsv`: compressed blobs' first bytes, and copied runs.
pub fn parse_reached(text: &str) -> Result<(Vec<u32>, Vec<Span>), String> {
    let (mut packed, mut copied) = (Vec::new(), Vec::new());
    for f in rows(text) {
        let bad = || format!("reached.tsv: bad line {:?}", f.join("\t"));
        match f[..] {
            ["packed", s, _] => packed.push(hex(s).ok_or_else(bad)?),
            ["copied", s, n] => {
                copied.push((hex(s).ok_or_else(bad)?, n.parse().map_err(|_| bad())?))
            }
            _ => return Err(bad()),
        }
    }
    Ok((packed, copied))
}

/// Runs joined across gaps under [`GAP`]: (start, end, the runs in it).
fn join(runs: &[Span]) -> Vec<(u32, u32, Vec<Span>)> {
    let mut out: Vec<(u32, u32, Vec<Span>)> = Vec::new();
    for &(s, n) in runs {
        match out.last_mut() {
            Some((_, end, rs)) if s < *end + GAP => {
                *end = s + n;
                rs.push((s, n));
            }
            _ => out.push((s, s + n, vec![(s, n)])),
        }
    }
    out
}

/// A pret symbol for an address: (name, size) of the data item at it.
pub type Symbols = BTreeMap<u32, (String, Option<u64>)>;

fn symbol_at(syms: &Symbols, a: u32) -> String {
    match syms.range(..=a).next_back() {
        Some((s, (name, size))) if size.is_none_or(|n| a < s + n as u32) => {
            if *s == a {
                name.clone()
            } else {
                format!("{name}+{:#x}", a - s)
            }
        }
        _ => "-".into(),
    }
}

pub struct Report {
    pub text: String,
    pub pass: bool,
}

/// The check, from the four files' text.
pub fn check(
    loads: &str,
    reached: &str,
    uses: &str,
    shown: &str,
    syms: &Symbols,
) -> Result<Report, String> {
    let loads = parse_loads(loads)?;
    let (packed, copied) = parse_reached(reached)?;
    let mut t = String::new();
    let mut pass = true;

    // Compressed blobs.
    let mut by_src: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for l in loads.iter().filter(|l| l.decompressed()) {
        by_src.entry(l.src).or_default().insert(l.src_len);
    }
    let (mut found, mut whole, mut no_symbol) = (0, 0, 0);
    let mut lines = String::new();
    for &b in &packed {
        let lens = by_src.get(&b);
        let size = syms.get(&b).and_then(|s| s.1);
        let verdict = match (lens, size) {
            (None, _) => {
                pass = false;
                "MISSING: no decompression from it".to_string()
            }
            (Some(l), _) if l.len() > 1 => {
                pass = false;
                found += 1;
                format!("MISMATCH: lengths {l:?}")
            }
            (Some(l), Some(n)) if *l.first().unwrap() as u64 != n => {
                pass = false;
                found += 1;
                format!("MISMATCH: {} bytes, pret's symbol {n}", l.first().unwrap())
            }
            (Some(l), Some(_)) => {
                found += 1;
                whole += 1;
                format!("whole: {} bytes", l.first().unwrap())
            }
            (Some(l), None) => {
                found += 1;
                no_symbol += 1;
                format!("{} bytes (no pret size)", l.first().unwrap())
            }
        };
        let _ = writeln!(lines, "  {b:#010x} {}: {verdict}", symbol_at(syms, b));
    }
    let _ = writeln!(
        t,
        "compressed blobs that reached the screen: {}; decompressed in the load log {found}, \
         its length the pret symbol's size {whole}, no pret size {no_symbol}\n{lines}",
        packed.len()
    );

    // Copied ranges.
    let mut loaded: Vec<(u32, u32)> = Vec::new();
    for l in &loads {
        if l.runs.is_empty() && !l.decompressed() {
            loaded.push((l.src, l.src + l.src_len));
        }
        for &(rom, n, dec) in &l.runs {
            if !dec {
                loaded.push((rom, rom + n));
            }
        }
    }
    loaded.sort();
    // Merged: the bytes loaded, as disjoint spans.
    let mut spans: Vec<(u32, u32)> = Vec::new();
    for (s, e) in loaded.iter().copied() {
        match spans.last_mut() {
            Some((_, pe)) if s <= *pe => *pe = (*pe).max(e),
            _ => spans.push((s, e)),
        }
    }
    let covered = |s: u32, e: u32| {
        let i = spans.partition_point(|sp| sp.1 <= s);
        spans.get(i).is_some_and(|sp| sp.0 <= s && e <= sp.1)
    };
    let ranges = join(&copied);
    let mut ok = 0;
    let mut lines = String::new();
    for (s, e, rs) in &ranges {
        let shown: u32 = rs.iter().map(|r| r.1).sum();
        let missing: Vec<String> = rs
            .iter()
            .filter(|(rs, n)| !covered(*rs, rs + n))
            .map(|(rs, n)| format!("{rs:#010x}+{n}"))
            .collect();
        // The longest load reaching into the range: its whole length.
        let longest = loaded
            .iter()
            .filter(|(ls, le)| *ls < *e && *le > *s)
            .map(|(ls, le)| le - ls)
            .max()
            .unwrap_or(0);
        let verdict = if missing.is_empty() {
            ok += 1;
            format!("loaded; {shown} bytes shown, longest load {longest}")
        } else {
            pass = false;
            format!("NOT LOADED: {}", missing.join(" "))
        };
        let _ = writeln!(
            lines,
            "  {s:#010x}..{e:#010x} {}: {verdict}",
            symbol_at(syms, *s)
        );
    }
    let _ = writeln!(
        t,
        "copied ranges that reached the screen (joined across gaps under {GAP}): {}; \
         every byte in the load log {ok}\n{lines}",
        ranges.len()
    );

    // Tiles shown on the last frame.
    let key = |f: &[&str]| f.join("\t");
    let shown_rows: Vec<Vec<&str>> = rows(shown).collect();
    let last: u64 = shown
        .lines()
        .next()
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|n| n.parse().ok())
        .ok_or("shown.tsv: no frame in its first line")?;
    let mut used: BTreeSet<String> = BTreeSet::new();
    for f in rows(uses) {
        let n = f.len();
        if n < 16 {
            return Err(format!("uses.tsv: bad line {:?}", f.join("\t")));
        }
        if f[n - 2].parse::<u64>().ok() == Some(last) {
            used.insert(key(&f[..n - 3]));
        }
    }
    let (mut with_use, mut blob, mut rom_no_blob, mut computed, mut never) = (0, 0, 0, 0, 0);
    let (mut bg, mut obj) = (0, 0);
    let mut missing = Vec::new();
    for f in &shown_rows {
        let k = key(&f[1..]);
        if f[1] == "obj" {
            obj += 1;
        } else {
            bg += 1;
        }
        if used.contains(&k) {
            with_use += 1;
        } else {
            missing.push(k);
        }
        // tile_src, tile_via, blob.
        match (f[5], f[6], f[8]) {
            (_, _, b) if b != "-" => blob += 1,
            ("-", "-", _) => never += 1,
            (s, _, _) if s != "-" && !f[6].contains("derived") => rom_no_blob += 1,
            _ => computed += 1,
        }
    }
    if !missing.is_empty() {
        pass = false;
    }
    let _ = writeln!(
        t,
        "tiles shown on frame {last}: {} ({bg} bg, {obj} obj); with a use record {with_use}. \
         Their tiles: from a loaded blob {blob}, copied from rom outside the load log \
         {rom_no_blob}, computed {computed}, never written {never}",
        shown_rows.len()
    );
    for k in missing.iter().take(20) {
        let _ = writeln!(t, "  NO USE: {k}");
    }
    let _ = writeln!(t, "\nloads logged: {}", loads.len());
    let _ = writeln!(t, "{}", if pass { "PASS" } else { "FAIL" });
    Ok(Report { text: t, pass })
}
