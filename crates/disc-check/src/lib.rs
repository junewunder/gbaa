//! The discovery-accuracy check specified in
//! `docs/research/discovery-prior-art.md` ("Proposed discovery-accuracy
//! check"): a truth file of function starts generated from an ELF, and a
//! scorer that prints precision and recall of a discovered-starts file
//! against it.
//!
//! Addresses are in interworking form throughout: bit 0 set means Thumb,
//! clear means ARM.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

pub mod elf;
pub mod swis;

/// ROM is `ROM_LO <= a < ROM_HI` for an address `a` with bit 0 cleared.
pub const ROM_LO: u32 = 0x0800_0000;
pub const ROM_HI: u32 = 0x0a00_0000;

pub fn in_rom(v: u32) -> bool {
    (ROM_LO..ROM_HI).contains(&(v & !1))
}

/// Symbol type, as far as the truth rules care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind {
    Func,
    NoType,
    /// `STT_OBJECT`: a variable or table.
    Object,
    /// `STT_FILE`: the object file the local symbols after it come from.
    File,
    Other,
}

/// One ELF symbol, reduced to what the truth rules read. `syms` passed to
/// [`build_truth`] must be in symbol-table (`readelf -sW`) order.
#[derive(Clone, Debug)]
pub struct Sym {
    pub name: String,
    pub value: u32,
    pub size: u64,
    pub kind: SymKind,
    pub global: bool,
    /// Section index, if the symbol is defined in a section.
    pub section: Option<usize>,
    /// The section has `SHF_EXECINSTR`.
    pub exec: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruthRow {
    pub addr: u32,
    pub size: u64,
    pub name: String,
}

/// The truth rows, sorted by address, and the names rule 2 added.
#[derive(Debug, Default)]
pub struct Truth {
    pub rows: Vec<TruthRow>,
    pub rule2_added: Vec<String>,
}

/// `$a`/`$t` (optionally `$a.<suffix>`): the mode of the code at a mapping
/// symbol, `Some(true)` for Thumb.
fn mapping_mode(name: &str) -> Option<bool> {
    let base = name.split_once('.').map_or(name, |(b, _)| b);
    match base {
        "$a" => Some(false),
        "$t" => Some(true),
        _ => None,
    }
}

/// Applies the spec's three row rules.
///
/// 1. every `STT_FUNC` symbol whose address, bit 0 cleared, is in ROM;
/// 2. every `GLOBAL NOTYPE` symbol in an executable section that sits on a
///    `$a` or `$t` mapping symbol in the same section, mode from the mapping
///    symbol;
/// 3. aliases collapse to one row, keeping the first name in symbol order.
///
/// A row takes the largest size among its aliases, so an asm label listed
/// before the function it sits on doesn't zero the function's extent. Rule
/// 2's additions are the rows no `STT_FUNC` symbol reached.
pub fn build_truth(syms: &[Sym]) -> Truth {
    let mut mapping: BTreeMap<(usize, u32), bool> = BTreeMap::new();
    for s in syms {
        if let (Some(sec), Some(thumb)) = (s.section, mapping_mode(&s.name)) {
            mapping.insert((sec, s.value & !1), thumb);
        }
    }
    // Each row, and whether any STT_FUNC symbol reached it.
    let mut rows: BTreeMap<u32, (TruthRow, bool)> = BTreeMap::new();
    for s in syms {
        let a = s.value & !1;
        if !in_rom(a) {
            continue;
        }
        let (key, func) = match s.kind {
            SymKind::Func => (s.value, true),
            SymKind::NoType if s.global && s.exec && mapping_mode(&s.name).is_none() => {
                match s.section.and_then(|sec| mapping.get(&(sec, a))) {
                    Some(&thumb) => (a | thumb as u32, false),
                    None => continue,
                }
            }
            _ => continue,
        };
        let row = rows.entry(key).or_insert_with(|| {
            let row = TruthRow {
                addr: key,
                size: s.size,
                name: s.name.clone(),
            };
            (row, func)
        });
        row.0.size = row.0.size.max(s.size);
        row.1 |= func;
    }
    let rule2_added = rows
        .values()
        .filter(|(_, func)| !func)
        .map(|(r, _)| r.name.clone())
        .collect();
    Truth {
        rows: rows.into_values().map(|(r, _)| r).collect(),
        rule2_added,
    }
}

/// The truth file: `0x%08x<TAB>size<TAB>name`, one row per line.
pub fn format_truth(rows: &[TruthRow]) -> String {
    let mut out = String::new();
    for r in rows {
        writeln!(out, "0x{:08x}\t{}\t{}", r.addr, r.size, r.name).unwrap();
    }
    out
}

fn parse_hex(s: &str) -> Option<u32> {
    let digits = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Strips a `#` comment and surrounding whitespace.
fn content(line: &str) -> &str {
    line.split_once('#').map_or(line, |(c, _)| c).trim()
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ParseError {}

/// Reads a truth file. Blank lines and `#` comments are skipped (the
/// generator writes neither, but hand-written test files may).
pub fn parse_truth(text: &str) -> Result<Vec<TruthRow>, ParseError> {
    let mut rows: BTreeMap<u32, TruthRow> = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let err = |msg: &str| ParseError {
            line: i + 1,
            msg: format!("{msg}: {line:?}"),
        };
        let c = content(line);
        if c.is_empty() {
            continue;
        }
        let mut cols = c.splitn(3, '\t');
        let (Some(a), Some(s), Some(name)) = (cols.next(), cols.next(), cols.next()) else {
            return Err(err("expected address<TAB>size<TAB>name"));
        };
        let addr = parse_hex(a.trim()).ok_or_else(|| err("bad address"))?;
        let size = s.trim().parse().map_err(|_| err("bad size"))?;
        let name = name.trim().to_string();
        rows.entry(addr).or_insert(TruthRow { addr, size, name });
    }
    Ok(rows.into_values().collect())
}

/// A parsed discovered-starts file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Discovered {
    /// Distinct starts in ROM.
    pub in_rom: BTreeSet<u32>,
    /// Distinct starts outside ROM: counted and printed, not scored.
    pub outside: BTreeSet<u32>,
    /// ARM starts (bit 0 clear) that are not 4-aligned.
    pub misaligned_arm: BTreeSet<u32>,
}

/// Reads a discovered-starts file: one hex start per line, `0x` optional,
/// any case; `#` comments; blank lines ignored; duplicates collapse. A
/// misaligned ARM start is collected even outside ROM: any one means the
/// discovery is broken.
pub fn parse_discovered(text: &str) -> Result<Discovered, ParseError> {
    let mut d = Discovered::default();
    for (i, line) in text.lines().enumerate() {
        let c = content(line);
        if c.is_empty() {
            continue;
        }
        let v = parse_hex(c).ok_or_else(|| ParseError {
            line: i + 1,
            msg: format!("bad start: {line:?}"),
        })?;
        if v & 1 == 0 && v & 3 != 0 {
            d.misaligned_arm.insert(v);
        } else if in_rom(v) {
            d.in_rom.insert(v);
        } else {
            d.outside.insert(v);
        }
    }
    Ok(d)
}

/// Why a discovered start in ROM is not a match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FalseKind {
    /// A truth row has this address in the other mode.
    WrongMode,
    /// Strictly inside a truth function's extent (`start < a < start +
    /// max(size, 1)`).
    Inside,
    Elsewhere,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FalseStart {
    pub addr: u32,
    pub kind: FalseKind,
    /// The truth row it relates to: the other-mode row, the containing
    /// function, or the nearest truth row below it.
    pub near: Option<(u32, String)>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Score {
    pub matched: usize,
    pub discovered: usize,
    pub truth: usize,
    pub outside: Vec<u32>,
    pub false_starts: Vec<FalseStart>,
    pub misses: Vec<TruthRow>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Misaligned(pub Vec<u32>);

impl fmt::Display for Misaligned {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} misaligned ARM start(s), discovery is broken; not scoring:",
            self.0.len()
        )?;
        for v in self.0.iter().take(20) {
            write!(f, " 0x{v:08x}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Misaligned {}

/// Scores `disc` against `truth` (sorted by address, as [`parse_truth`]
/// returns it). A start matches a row iff the 32-bit values are equal.
pub fn score(truth: &[TruthRow], disc: &Discovered) -> Result<Score, Misaligned> {
    if !disc.misaligned_arm.is_empty() {
        return Err(Misaligned(disc.misaligned_arm.iter().copied().collect()));
    }
    let by_addr: BTreeMap<u32, &TruthRow> = truth.iter().map(|r| (r.addr, r)).collect();
    // Extents keyed by start (bit 0 cleared). Each entry carries the row,
    // among those starting at or below it, whose extent reaches furthest,
    // so a nested or overlapping function can't hide a containing one.
    let mut extents: BTreeMap<u32, (u64, &TruthRow)> = BTreeMap::new();
    for r in truth {
        let end = (r.addr & !1) as u64 + r.size.max(1);
        let e = extents.entry(r.addr & !1).or_insert((end, r));
        if end > e.0 {
            *e = (end, r);
        }
    }
    let mut furthest: Option<(u64, &TruthRow)> = None;
    for e in extents.values_mut() {
        match furthest {
            Some(f) if f.0 >= e.0 => *e = f,
            _ => furthest = Some(*e),
        }
    }
    let named = |r: &TruthRow| Some((r.addr, r.name.clone()));
    let mut matched = 0;
    let mut false_starts = Vec::new();
    for &v in &disc.in_rom {
        if by_addr.contains_key(&v) {
            matched += 1;
            continue;
        }
        let a = v & !1;
        let (kind, near) = if let Some(r) = by_addr.get(&(v ^ 1)) {
            (FalseKind::WrongMode, named(r))
        } else {
            // Rows starting strictly below a; the one at a, if any, is the
            // other mode and was handled above.
            match extents.range(..a).next_back() {
                Some((_, &(end, r))) if (a as u64) < end => (FalseKind::Inside, named(r)),
                Some(_) => (
                    FalseKind::Elsewhere,
                    by_addr.range(..v).next_back().and_then(|(_, r)| named(r)),
                ),
                None => (FalseKind::Elsewhere, None),
            }
        };
        false_starts.push(FalseStart {
            addr: v,
            kind,
            near,
        });
    }
    let misses = truth
        .iter()
        .filter(|r| !disc.in_rom.contains(&r.addr))
        .cloned()
        .collect();
    Ok(Score {
        matched,
        discovered: disc.in_rom.len(),
        truth: truth.len(),
        outside: disc.outside.iter().copied().collect(),
        false_starts,
        misses,
    })
}

fn ratio(m: usize, n: usize) -> f64 {
    if n == 0 { 0.0 } else { m as f64 / n as f64 }
}

impl Score {
    pub fn precision(&self) -> f64 {
        ratio(self.matched, self.discovered)
    }

    pub fn recall(&self) -> f64 {
        ratio(self.matched, self.truth)
    }

    fn count(&self, kind: FalseKind) -> usize {
        self.false_starts.iter().filter(|f| f.kind == kind).count()
    }

    /// The report: the two numbers, then diagnostics, listing at most
    /// `show` misses and false starts.
    pub fn report(&self, show: usize) -> String {
        let mut o = String::new();
        let (m, d, t) = (self.matched, self.discovered, self.truth);
        writeln!(o, "precision {:.4} ({m}/{d})", self.precision()).unwrap();
        writeln!(o, "recall {:.4} ({m}/{t})", self.recall()).unwrap();
        writeln!(
            o,
            "false starts: {} ({} wrong mode, {} inside a true function, {} elsewhere)",
            self.false_starts.len(),
            self.count(FalseKind::WrongMode),
            self.count(FalseKind::Inside),
            self.count(FalseKind::Elsewhere),
        )
        .unwrap();
        write!(o, "ignored outside ROM: {}", self.outside.len()).unwrap();
        for v in self.outside.iter().take(show) {
            write!(o, " 0x{v:08x}").unwrap();
        }
        writeln!(o).unwrap();
        writeln!(o, "misses: {}", self.misses.len()).unwrap();
        for r in self.misses.iter().take(show) {
            writeln!(o, "  0x{:08x} {}", r.addr, r.name).unwrap();
        }
        if !self.false_starts.is_empty() {
            writeln!(o, "false starts:").unwrap();
        }
        for f in self.false_starts.iter().take(show) {
            let kind = match f.kind {
                FalseKind::WrongMode => "wrong mode",
                FalseKind::Inside => "inside",
                FalseKind::Elsewhere => "elsewhere",
            };
            write!(o, "  0x{:08x} {kind}", f.addr).unwrap();
            if let Some((a, name)) = &f.near {
                // `near` is never above the start.
                let off = (f.addr & !1) - (a & !1);
                write!(o, " ({name}+0x{off:x})").unwrap();
            }
            writeln!(o).unwrap();
        }
        o
    }
}

#[cfg(test)]
mod tests;

/// What code discovery found, in a few lines: functions, jump tables and
/// far jumps first.
pub fn discovery_report(d: &analysis::discovery::Discovery) -> String {
    use analysis::discovery::Evidence;
    let scanned =
        |m: &BTreeMap<u32, Evidence>| m.values().filter(|&&e| e == Evidence::Scanned).count();
    let code_scanned = d
        .code
        .values()
        .filter(|c| c.evidence == Evidence::Scanned)
        .count();
    let mut out = String::new();
    writeln!(
        out,
        "functions {} ({} scanned), jump tables {}, far jumps {}",
        d.funcs.len(),
        scanned(&d.funcs),
        d.tables(),
        d.rec.far_jumps.len()
    )
    .unwrap();
    let thumb = d.code.values().filter(|c| c.thumb).count();
    writeln!(
        out,
        "  instructions {} ({} Thumb, {} scanned), continuation entries {}, data ranges {}",
        d.code.len(),
        thumb,
        code_scanned,
        d.entries.len(),
        d.data.len()
    )
    .unwrap();
    let unresolved = d.rec.jumps.len() - d.rec.resolved().count();
    writeln!(
        out,
        "  indirect jumps {} ({} resolved), {} rounds, {} analyses ({} frozen, {} capped, {} never return)",
        d.rec.jumps.len(),
        d.rec.jumps.len() - unresolved,
        d.rounds,
        d.rec.analyses,
        d.rec.frozen,
        d.rec.capped,
        d.rec.no_returns
    )
    .unwrap();
    writeln!(
        out,
        "  pointer scan: {} batches, {} candidates, {} valid, {} taken, {} dropped (capped), {} dropped (undefined)",
        d.scan.batches,
        d.scan.candidates,
        d.scan.valid,
        d.scan.accepted,
        d.dropped,
        d.undefined
    )
    .unwrap();
    for &(src, dst, n) in &d.copies {
        writeln!(
            out,
            "  BIOS copy: 0x{src:08x}..0x{:08x} to 0x{dst:08x} ({n} bytes)",
            src + n
        )
        .unwrap();
    }
    if !d.ram_entries.is_empty() {
        let e: Vec<String> = d.ram_entries.iter().map(|a| format!("0x{a:08x}")).collect();
        writeln!(out, "  RAM entries: {}", e.join(" ")).unwrap();
    }
    out
}
