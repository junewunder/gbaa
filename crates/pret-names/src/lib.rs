//! Exact names for a ROM a pret decomp builds byte-identically (the mod
//! view, decision 2: "The decomps are the oracle"): the ELF's symbols
//! matched to the function entries codegen compiles (ROM code, and the
//! IWRAM copies discovery finds) and to the data the code points at, as a
//! names file later printers read.
//!
//! Addresses are in interworking form, as codegen's keys are: bit 0 set for
//! Thumb, clear for ARM. Code copied into IWRAM is named by its ROM
//! address in the ELF (pret links it there and copies it at run time), so a
//! RAM entry is looked up at its source.

use disc_check::{Sym, SymKind, TruthRow, build_truth};
pub mod dwarf;
pub mod text;
pub mod values;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// What gbaa found in the ROM, reduced to what the matcher reads.
#[derive(Debug, Default)]
pub struct Ours {
    /// Codegen's function entries in ROM (`Ctx::entries`): discovered
    /// function starts and continuation roots, as keys.
    pub entries: BTreeSet<u32>,
    /// Every function start discovery found, before codegen dropped those
    /// with no code in their mode.
    pub discovered: BTreeSet<u32>,
    /// Every instruction reached in ROM: address and mode.
    pub code: BTreeMap<u32, bool>,
    /// Literal pools, jump tables and copy sources, `start..end`.
    pub data: Vec<(u32, u32)>,
    /// The BIOS copies out of ROM, each with its entries at the run
    /// address (none where discovery doesn't follow the copy).
    pub copies: Vec<Copy>,
}

#[derive(Debug, Default)]
pub struct Copy {
    pub src: u32,
    pub dst: u32,
    pub len: u32,
    pub entries: BTreeSet<u32>,
}

impl Copy {
    /// The ROM key of the run-address key `k`.
    pub fn rom_key(&self, k: u32) -> u32 {
        (self.src + ((k & !1) - self.dst)) | (k & 1)
    }
}

/// What an entry of ours is in pret's symbols.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Name {
    /// A function symbol at exactly this address and mode.
    Func { name: String, size: u64 },
    /// An assembly label (no function type) at this address.
    Label(String),
    /// No symbol: `off` bytes into function `func` (a continuation root,
    /// or a second entry into the function).
    Inside { func: String, off: u32 },
    /// A function symbol is here in the other mode.
    OtherMode(String),
    /// In pret's data, `off` bytes into `datum`: discovery took data for
    /// code.
    Data { datum: String, off: u32 },
    /// Nothing in the symbols covers it.
    Unknown,
}

/// Why a pret function isn't one of our entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Miss {
    /// Discovery found the start but no code in its mode there.
    NoCode,
    /// Its code is reached, but only by falling or branching into it from
    /// another function (a tail call or shared code), never as a start.
    Reached,
    /// Reached in the other mode.
    OtherMode,
    /// Inside a range discovery took for data.
    Data,
    /// Unreached, but its address is a word in the ROM: a pointer
    /// discovery doesn't follow.
    Pointer,
    /// Unreached, and no word in the ROM points at it: dead code, or
    /// reached through a computed address.
    Dead,
}

impl Miss {
    pub fn why(self) -> &'static str {
        match self {
            Miss::NoCode => "discovered, but no code in its mode",
            Miss::Reached => "code reached, but not as a function start",
            Miss::OtherMode => "code reached in the other mode",
            Miss::Data => "inside data discovery found",
            Miss::Pointer => "unreached; a ROM word points at it",
            Miss::Dead => "unreached; no ROM word points at it",
        }
    }
}

/// A named piece of data (a variable, a table, a buffer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Datum {
    pub name: String,
    pub size: u64,
    /// The other names at the same address, in symbol order.
    pub aliases: Vec<String>,
    /// The object file of a static (local) datum; a global's isn't kept.
    pub file: Option<String>,
}

/// How the pointers in our literal pools land on pret's data.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PoolHits {
    pub exact: usize,
    pub inside: usize,
    pub unnamed: usize,
}

/// One matched entry of ours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The key codegen uses (the run address for an IWRAM copy).
    pub key: u32,
    /// Its ROM key, where that differs.
    pub rom: Option<u32>,
    pub name: Name,
    /// The object file its symbol is in (`main.o`), where the ELF says.
    pub file: Option<String>,
}

#[derive(Debug, Default)]
pub struct Names {
    pub entries: Vec<Entry>,
    /// pret functions none of our entries is, by address.
    pub missed: Vec<(TruthRow, Miss)>,
    pub data: BTreeMap<u32, Datum>,
    /// Pool words pointing into EWRAM or IWRAM, and into ROM outside code.
    pub ram: PoolHits,
    pub rom: PoolHits,
    pub pret_funcs: usize,
    pub copies: Vec<CopyNames>,
}

/// A copy out of ROM and the pret functions in its source, keyed by run
/// address.
#[derive(Debug, PartialEq, Eq)]
pub struct CopyNames {
    pub src: u32,
    pub dst: u32,
    pub len: u32,
    /// Its entries discovery followed.
    pub entries: usize,
    pub funcs: Vec<(u32, String)>,
}

fn is_mapping(name: &str) -> bool {
    name.starts_with('$')
}

/// pret's functions, each one's extent (its size, or up to the next one
/// when the symbol has none), and asm labels in code sections.
struct Funcs {
    rows: BTreeMap<u32, TruthRow>,
    /// `addr & !1` → (end, key).
    extent: BTreeMap<u32, (u32, u32)>,
    labels: BTreeMap<u32, String>,
}

impl Funcs {
    fn new(syms: &[Sym]) -> Funcs {
        let rows: BTreeMap<u32, TruthRow> = build_truth(syms)
            .rows
            .into_iter()
            .map(|r| (r.addr, r))
            .collect();
        let starts: Vec<u32> = rows.keys().map(|&k| k & !1).collect();
        let mut extent = BTreeMap::new();
        for (i, (&k, r)) in rows.iter().enumerate() {
            let a = k & !1;
            let next = starts.get(i + 1).copied().unwrap_or(a + 4);
            let end = if r.size > 0 { a + r.size as u32 } else { next };
            extent.insert(a, (end, k));
        }
        let mut labels = BTreeMap::new();
        for s in syms {
            if s.kind == SymKind::NoType
                && s.exec
                && !is_mapping(&s.name)
                && !s.name.starts_with('.')
                && !s.name.is_empty()
            {
                labels.entry(s.value & !1).or_insert_with(|| s.name.clone());
            }
        }
        Funcs {
            rows,
            extent,
            labels,
        }
    }

    /// The function whose extent holds `a` (bit 0 clear).
    fn holding(&self, a: u32) -> Option<(&TruthRow, u32)> {
        let (&start, &(end, k)) = self.extent.range(..=a).next_back()?;
        (a < end).then(|| (&self.rows[&k], a - start))
    }

    fn name(&self, k: u32) -> Name {
        if let Some(r) = self.rows.get(&k) {
            return Name::Func {
                name: r.name.clone(),
                size: r.size,
            };
        }
        let a = k & !1;
        let other = if k & 1 != 0 { a } else { a | 1 };
        if let Some(r) = self.rows.get(&other) {
            return Name::OtherMode(r.name.clone());
        }
        if let Some(l) = self.labels.get(&a) {
            return Name::Label(l.clone());
        }
        match self.holding(a) {
            Some((r, off)) => Name::Inside {
                func: r.name.clone(),
                off,
            },
            None => Name::Unknown,
        }
    }
}

/// The input sections of a GNU ld map file (`pokeemerald_modern.map`):
/// start → (end, object file name), for each placed section with bytes.
/// A section's name and its address may be on two lines.
pub fn map_sections(map: &str) -> BTreeMap<u32, (u32, String)> {
    let mut out = BTreeMap::new();
    let mut pending = false;
    for line in map.lines() {
        let mut w = line.split_whitespace();
        let starts = [" .", " ewram", " common", " COMMON"]
            .iter()
            .any(|p| line.starts_with(p));
        let fields: Vec<&str> = if starts {
            w.next();
            let rest: Vec<&str> = w.collect();
            pending = rest.is_empty();
            rest
        } else if pending && line.starts_with("    ") {
            pending = false;
            w.collect()
        } else {
            pending = false;
            continue;
        };
        let [addr, size, path] = fields[..] else {
            continue;
        };
        let hex = |s: &str| {
            s.strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
        };
        let (Some(a), Some(n)) = (hex(addr), hex(size)) else {
            continue;
        };
        if a == 0 || n == 0 {
            continue;
        }
        let file = path.rsplit('/').next().unwrap_or(path);
        out.insert(a, (a + n, file.to_string()));
    }
    out
}

/// Modern GCC compiles pret's C from a pipe, so every object's `STT_FILE`
/// is `<stdin>`: each such one is renamed to the object, by the map
/// (`map_sections`), holding the first symbol after it that it places.
/// Returns how many were renamed.
pub fn name_stdin_files(syms: &mut [Sym], sections: &BTreeMap<u32, (u32, String)>) -> usize {
    let mut renamed = 0;
    for i in 0..syms.len() {
        if syms[i].kind != SymKind::File || syms[i].name != "<stdin>" {
            continue;
        }
        let found = syms[i + 1..]
            .iter()
            .take_while(|s| s.kind != SymKind::File)
            .filter(|s| s.section.is_some() && !s.global)
            .find_map(|s| {
                // Bit 0 is the Thumb bit only on code.
                let a = if s.exec { s.value & !1 } else { s.value };
                let (_, (end, f)) = sections.range(..=a).next_back()?;
                (a < *end).then(|| f.clone())
            });
        if let Some(f) = found {
            syms[i].name = f;
            renamed += 1;
        }
    }
    renamed
}

/// The object file each symbol is in. A local symbol follows the
/// `STT_FILE` before it; globals come after every local, so a global
/// function is placed by the nearest local symbol at or below it in code
/// (each object's code starts with a `$a` or `$t`).
#[derive(Default)]
pub struct Files {
    exact: BTreeMap<(u32, String), String>,
    code: BTreeMap<u32, String>,
}

impl Files {
    pub fn new(syms: &[Sym]) -> Files {
        let mut out = Files::default();
        let mut file: Option<&str> = None;
        for s in syms {
            if s.kind == SymKind::File {
                file = Some(&s.name);
                continue;
            }
            let (Some(f), false, Some(_)) = (file, s.global, s.section) else {
                continue;
            };
            out.exact.insert((s.value, s.name.clone()), f.to_string());
            if s.exec {
                out.code.insert(s.value & !1, f.to_string());
            }
        }
        out
    }

    /// The file of the function `name` at key `k`.
    pub fn func(&self, k: u32, name: &str) -> Option<String> {
        if let Some(f) = self.exact.get(&(k, name.to_string())) {
            return Some(f.clone());
        }
        self.code
            .range(..=k & !1)
            .next_back()
            .map(|(_, f)| f.clone())
    }

    /// The file of a datum `name` at `a`, if it is a static.
    pub fn datum(&self, a: u32, name: &str) -> Option<String> {
        self.exact.get(&(a, name.to_string())).cloned()
    }
}

/// pret's data symbols by address: every symbol that isn't a function, a
/// mapping symbol, a section-local `.L` label or an asm label in code (one
/// in a code section whose last mapping symbol isn't `$d`). At an address
/// with several, a typed object beats a global label, which beats a local
/// one; the rest are aliases.
fn data_syms(syms: &[Sym], funcs: &Funcs, files: &Files) -> BTreeMap<u32, Datum> {
    let mut mapping: BTreeMap<(usize, u32), bool> = BTreeMap::new();
    for s in syms {
        if let (Some(sec), true) = (s.section, is_mapping(&s.name)) {
            mapping.insert((sec, s.value & !1), s.name.starts_with("$d"));
        }
    }
    let in_code = |s: &Sym| {
        let sec = s.section.unwrap_or(0);
        let last = mapping.range((sec, 0)..=(sec, s.value & !1)).next_back();
        !last.is_some_and(|(_, &data)| data)
    };
    let rank = |s: &Sym| match (s.kind, s.global) {
        (SymKind::Object, _) => 0,
        (_, true) => 1,
        _ => 2,
    };
    let mut by: BTreeMap<u32, Vec<&Sym>> = BTreeMap::new();
    for s in syms {
        let ok = matches!(s.kind, SymKind::Object | SymKind::NoType)
            && s.section.is_some()
            && !s.name.is_empty()
            && !is_mapping(&s.name)
            && !s.name.starts_with('.')
            && s.value != 0;
        if !ok || funcs.rows.contains_key(&s.value) {
            continue;
        }
        if s.exec && s.kind == SymKind::NoType && in_code(s) {
            continue;
        }
        by.entry(s.value).or_default().push(s);
    }
    by.into_iter()
        .map(|(a, mut v)| {
            v.sort_by_key(|s| rank(s));
            let mut aliases: Vec<String> = Vec::new();
            for s in &v[1..] {
                if s.name != v[0].name && !aliases.contains(&s.name) {
                    aliases.push(s.name.clone());
                }
            }
            let datum = Datum {
                name: v[0].name.clone(),
                size: v.iter().map(|s| s.size).max().unwrap_or(0),
                aliases,
                file: files.datum(a, &v[0].name),
            };
            (a, datum)
        })
        .collect()
}

/// The data symbol holding `a`: the nearest at or below it, within its
/// size, or within [`UNSIZED`] bytes if it has none (asm labels: battle
/// and event scripts).
fn holding_datum(data: &BTreeMap<u32, Datum>, a: u32) -> Option<(String, u32)> {
    let (&s, d) = data.range(..=a).next_back()?;
    let len = if d.size > 0 { d.size } else { UNSIZED };
    ((a - s) < len as u32).then(|| (d.name.clone(), a - s))
}

const UNSIZED: u64 = 0x100;

fn in_ram(v: u32) -> bool {
    (0x0200_0000..0x0204_0000).contains(&v) || (0x0300_0000..0x0300_8000).contains(&v)
}

fn word(rom: &[u8], a: u32) -> Option<u32> {
    let o = a.checked_sub(disc_check::ROM_LO)? as usize;
    let b = rom.get(o..o + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Matches `syms` (an ELF's symbols in table order) to `ours`, found in
/// `rom`, the bytes the ELF builds.
pub fn match_names(syms: &[Sym], rom: &[u8], ours: &Ours) -> Names {
    let funcs = Funcs::new(syms);
    let files = Files::new(syms);
    let file = |k: u32, n: &Name| match n {
        Name::Func { name, .. } | Name::Label(name) => files.func(k, name),
        _ => None,
    };
    let mut out = Names {
        pret_funcs: funcs.rows.len(),
        ..Names::default()
    };
    for &k in &ours.entries {
        let name = funcs.name(k);
        out.entries.push(Entry {
            key: k,
            rom: None,
            file: file(k, &name),
            name,
        });
    }
    let mut ram_named = BTreeSet::new();
    for c in &ours.copies {
        for &k in &c.entries {
            let rk = c.rom_key(k);
            ram_named.insert(rk);
            let name = funcs.name(rk);
            out.entries.push(Entry {
                key: k,
                rom: Some(rk),
                file: file(rk, &name),
                name,
            });
        }
    }

    for c in &ours.copies {
        out.copies.push(CopyNames {
            src: c.src,
            dst: c.dst,
            len: c.len,
            entries: c.entries.len(),
            funcs: funcs
                .rows
                .range(c.src..c.src + c.len)
                .map(|(&k, r)| ((k - c.src) + c.dst, r.name.clone()))
                .collect(),
        });
    }

    let words: BTreeSet<u32> = (0..rom.len() / 4)
        .map(|i| u32::from_le_bytes(rom[i * 4..i * 4 + 4].try_into().unwrap()))
        .collect();
    let in_data = |a: u32| {
        let i = ours.data.partition_point(|&(_, e)| e <= a);
        ours.data.get(i).is_some_and(|&(s, _)| s <= a)
    };
    for (&k, r) in &funcs.rows {
        if ours.entries.contains(&k) || ram_named.contains(&k) {
            continue;
        }
        let (a, thumb) = (k & !1, k & 1 != 0);
        let miss = if ours.discovered.contains(&k) {
            Miss::NoCode
        } else if let Some(&t) = ours.code.get(&a) {
            if t == thumb {
                Miss::Reached
            } else {
                Miss::OtherMode
            }
        } else if in_data(a) {
            Miss::Data
        } else if words.contains(&k) {
            Miss::Pointer
        } else {
            Miss::Dead
        };
        out.missed.push((r.clone(), miss));
    }

    out.data = data_syms(syms, &funcs, &files);
    for e in &mut out.entries {
        if e.name == Name::Unknown {
            // The key itself, bit 0 included: data labels are byte
            // addresses, and a Thumb key's bit 0 lands on an odd one.
            if let Some((datum, off)) = holding_datum(&out.data, e.rom.unwrap_or(e.key)) {
                e.name = Name::Data { datum, off };
            }
        }
    }
    let sized: Vec<(u32, u32)> = out
        .data
        .iter()
        .map(|(&a, d)| (a, a + d.size.max(1) as u32))
        .collect();
    let inside = |v: u32| {
        let i = sized.partition_point(|&(s, _)| s <= v);
        i > 0 && v < sized[i - 1].1
    };
    for &(s, e) in &ours.data {
        for a in (s.next_multiple_of(4)..e.saturating_sub(3)).step_by(4) {
            let Some(v) = word(rom, a) else { continue };
            let hits = if in_ram(v) {
                &mut out.ram
            } else if disc_check::in_rom(v) && !ours.code.contains_key(&(v & !1)) {
                &mut out.rom
            } else {
                continue;
            };
            if out.data.contains_key(&v) {
                hits.exact += 1;
            } else if inside(v) {
                hits.inside += 1;
            } else {
                hits.unnamed += 1;
            }
        }
    }
    out
}

/// A TOML string: names are C identifiers, but escape anyway.
pub(crate) fn quoted(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if c.is_control() => write!(o, "\\u{:04x}", c as u32).unwrap(),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

impl Names {
    /// Entries named by a function symbol (or an asm label) at their exact
    /// address.
    pub fn named(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.name, Name::Func { .. } | Name::Label(_)))
            .count()
    }

    /// The names file: `header` lines as comments at the top, then the
    /// `[map]` header (`docs/mod-view-design.md`) pinning it to the ROM
    /// whose sha1 is `rom_sha1`, its names from `source`.
    /// With `dbg`, its DWARF: `src` and `proto` on functions, `type` on
    /// data, and `[locals]`, `[lines]` and `[types]` at the end.
    pub fn toml(
        &self,
        header: &[String],
        rom_sha1: &str,
        source: &str,
        dbg: Option<&dwarf::Debug>,
    ) -> String {
        let mut o = String::new();
        for h in header {
            writeln!(o, "# {h}").unwrap();
        }
        writeln!(
            o,
            "\n[map]\nrom_sha1 = {}\nsource = {}\nformat = 1",
            quoted(rom_sha1),
            quoted(source)
        )
        .unwrap();
        o.push_str(
            "#\n# Keys are codegen's function keys (bit 0 set = Thumb; an IWRAM copy's\n\
             # run address, with `rom` its source) or data addresses.\n\n",
        );
        o.push_str("# Our entries with a pret function symbol (`label`: an asm label) here.\n[functions]\n");
        let rom = |e: &Entry| {
            let file = e
                .file
                .as_ref()
                .map_or(String::new(), |f| format!(", file = {}", quoted(f)));
            let rom = e
                .rom
                .map_or(String::new(), |r| format!(", rom = 0x{r:08x}"));
            let dw = dbg.map_or(String::new(), |d| d.func_fields(e.rom.unwrap_or(e.key)));
            file + &rom + &dw
        };
        for e in &self.entries {
            match &e.name {
                Name::Func { name, size } => writeln!(
                    o,
                    "0x{:08x} = {{ name = {}, size = {size}{} }}",
                    e.key,
                    quoted(name),
                    rom(e)
                ),
                Name::Label(name) => writeln!(
                    o,
                    "0x{:08x} = {{ name = {}, label = true{} }}",
                    e.key,
                    quoted(name),
                    rom(e)
                ),
                _ => continue,
            }
            .unwrap();
        }
        o.push_str(
            "\n# Our entries pret has no symbol for, inside one of its functions.\n[inside]\n",
        );
        for e in &self.entries {
            if let Name::Inside { func, off } = &e.name {
                writeln!(
                    o,
                    "0x{:08x} = {{ func = {}, offset = 0x{off:x}{} }}",
                    e.key,
                    quoted(func),
                    rom(e)
                )
                .unwrap();
            }
        }
        o.push_str(
            "\n# Our entries with no function name: a pret function in the other mode,\n\
             # pret data (discovery took it for code), or nothing.\n[unnamed]\n",
        );
        for e in &self.entries {
            let why = match &e.name {
                Name::OtherMode(n) => format!("other mode: {n}"),
                Name::Data { datum, off } => format!("data: {datum}+0x{off:x}"),
                Name::Unknown => "no symbol".to_string(),
                _ => continue,
            };
            writeln!(o, "0x{:08x} = {}", e.key, quoted(&why)).unwrap();
        }
        o.push_str("\n# pret functions that aren't our entries, and why.\n[missed]\n");
        for (r, m) in &self.missed {
            writeln!(
                o,
                "0x{:08x} = {{ name = {}, size = {}, why = {} }}",
                r.addr,
                quoted(&r.name),
                r.size,
                quoted(m.why())
            )
            .unwrap();
        }
        o.push_str(
            "\n# Code copied out of ROM to run in RAM (BIOS copies discovery found), by\n\
             # destination: the pret functions in it, keyed by run address.\n[copies]\n",
        );
        // One row per destination (a key appears once in TOML): later copies
        // to the same place, such as two palettes loaded into one buffer, go
        // in its `also`, and their functions join its own.
        let mut by_dst: BTreeMap<u32, Vec<&CopyNames>> = BTreeMap::new();
        for c in &self.copies {
            by_dst.entry(c.dst).or_default().push(c);
        }
        let mut written = BTreeSet::new();
        for c in &self.copies {
            if !written.insert(c.dst) {
                continue;
            }
            let all = &by_dst[&c.dst];
            let mut keys = BTreeSet::new();
            let fs: Vec<String> = all
                .iter()
                .flat_map(|c| &c.funcs)
                .filter(|(k, _)| keys.insert(*k))
                .map(|(k, n)| format!("{{ key = 0x{k:08x}, name = {} }}", quoted(n)))
                .collect();
            let also: Vec<String> = all[1..]
                .iter()
                .map(|c| format!("{{ rom = 0x{:08x}, len = {} }}", c.src, c.len))
                .collect();
            let also = if also.is_empty() {
                String::new()
            } else {
                format!(", also = [{}]", also.join(", "))
            };
            writeln!(
                o,
                "0x{:08x} = {{ rom = 0x{:08x}, len = {}{also}, functions = [{}] }}",
                c.dst,
                c.src,
                c.len,
                fs.join(", ")
            )
            .unwrap();
        }
        o.push_str("\n# pret's data: variables, tables and buffers in RAM and ROM.\n[data]\n");
        for (a, d) in &self.data {
            write!(
                o,
                "0x{a:08x} = {{ name = {}, size = {}",
                quoted(&d.name),
                d.size
            )
            .unwrap();
            if let Some(f) = &d.file {
                write!(o, ", file = {}", quoted(f)).unwrap();
            }
            if !d.aliases.is_empty() {
                let al: Vec<String> = d.aliases.iter().map(|s| quoted(s)).collect();
                write!(o, ", aliases = [{}]", al.join(", ")).unwrap();
            }
            if let Some(dbg) = dbg {
                o.push_str(&dbg.data_fields(*a));
            }
            o.push_str(" }\n");
        }
        if let Some(dbg) = dbg {
            let keys: Vec<u32> = self
                .entries
                .iter()
                .filter(|e| e.rom.is_none() && matches!(e.name, Name::Func { .. }))
                .map(|e| e.key)
                .collect();
            o.push_str(&dbg.tables(&keys));
        }
        o
    }

    /// The report: counts, then up to `show` examples of each kind.
    pub fn report(&self, show: usize) -> String {
        let mut o = String::new();
        let n = self.entries.len();
        let count = |f: &dyn Fn(&Name) -> bool| self.entries.iter().filter(|e| f(&e.name)).count();
        let ram = self.entries.iter().filter(|e| e.rom.is_some()).count();
        writeln!(
            o,
            "our entries: {n} ({} in ROM, {ram} in IWRAM copies)",
            n - ram
        )
        .unwrap();
        let named = self.named();
        writeln!(
            o,
            "  named exactly: {named} ({:.1}%): {} by a function symbol, {} by an asm label",
            100.0 * named as f64 / n.max(1) as f64,
            count(&|x| matches!(x, Name::Func { .. })),
            count(&|x| matches!(x, Name::Label(_)))
        )
        .unwrap();
        writeln!(
            o,
            "  inside a pret function, no symbol: {}",
            count(&|x| matches!(x, Name::Inside { .. }))
        )
        .unwrap();
        writeln!(
            o,
            "  a pret function in the other mode: {}",
            count(&|x| matches!(x, Name::OtherMode(_)))
        )
        .unwrap();
        writeln!(
            o,
            "  in pret's data (taken for code): {}",
            count(&|x| matches!(x, Name::Data { .. }))
        )
        .unwrap();
        writeln!(o, "  no symbol at all: {}", count(&|x| *x == Name::Unknown)).unwrap();
        writeln!(
            o,
            "pret functions: {}; not our entries: {}",
            self.pret_funcs,
            self.missed.len()
        )
        .unwrap();
        let mut by: BTreeMap<Miss, Vec<&TruthRow>> = BTreeMap::new();
        for (r, m) in &self.missed {
            by.entry(*m).or_default().push(r);
        }
        for (m, rows) in &by {
            let ex: Vec<String> = rows
                .iter()
                .take(show)
                .map(|r| format!("{} 0x{:08x}", r.name, r.addr))
                .collect();
            writeln!(o, "  {}: {} (e.g. {})", m.why(), rows.len(), ex.join(", ")).unwrap();
        }
        for (what, f) in [
            (
                "other mode",
                &(|x: &Name| matches!(x, Name::OtherMode(_))) as &dyn Fn(&Name) -> bool,
            ),
            ("in data", &|x: &Name| matches!(x, Name::Data { .. })),
            ("no symbol", &|x: &Name| *x == Name::Unknown),
        ] {
            let ex: Vec<String> = self
                .entries
                .iter()
                .filter(|e| f(&e.name))
                .take(show)
                .map(|e| match &e.name {
                    Name::Data { datum, off } => format!("0x{:08x} ({datum}+0x{off:x})", e.key),
                    Name::OtherMode(n) => format!("0x{:08x} ({n})", e.key),
                    _ => format!("0x{:08x}", e.key),
                })
                .collect();
            if !ex.is_empty() {
                writeln!(o, "our entries, {what}: {}", ex.join(" ")).unwrap();
            }
        }
        for c in &self.copies {
            let (src, dst, len, entries, fs) = (c.src, c.dst, c.len, c.entries, &c.funcs);
            if fs.is_empty() && entries == 0 {
                continue;
            }
            let names: Vec<&str> = fs.iter().take(show).map(|(_, n)| n.as_str()).collect();
            writeln!(
                o,
                "copy 0x{src:08x} -> 0x{dst:08x} ({len} bytes): {entries} entries followed, pret functions {} ({})",
                fs.len(),
                names.join(", ")
            )
            .unwrap();
        }
        let ram_data = self.data.keys().filter(|&&a| in_ram(a)).count();
        writeln!(
            o,
            "pret data symbols: {} ({ram_data} in RAM, {} elsewhere)",
            self.data.len(),
            self.data.len() - ram_data
        )
        .unwrap();
        for (what, h) in [("RAM", &self.ram), ("ROM data", &self.rom)] {
            let t = h.exact + h.inside + h.unnamed;
            writeln!(
                o,
                "pool words pointing into {what}: {t}: at a symbol {} ({:.1}%), inside one {}, unnamed {}",
                h.exact,
                100.0 * h.exact as f64 / t.max(1) as f64,
                h.inside,
                h.unnamed
            )
            .unwrap();
        }
        o
    }
}

#[cfg(test)]
mod tests;
