//! `gbaa-assets coverage` (docs/asset-extraction-design.md, Graphics
//! coverage): pret's Emerald image, palette and tilemap files as the
//! denominator for june's ~90% target, and a fuzz or route output scored
//! against them.
//!
//! - **pret's files**: every `INCGFX` under pret's `src/`, by its source
//!   file and output kind (`graphics/x.png` as `.4bpp.lz`), including the
//!   elements of an aggregate (`gTilesetPalettes_General[][16] = { INCGFX…,
//!   … }`), which share the aggregate's symbol.
//! - **ROM ranges**: each file's symbol in the source map (pret's ELF), an
//!   element its share of the aggregate when the elements are uncompressed
//!   and alike, else the whole aggregate.
//! - **Never used in play**, left out of the denominator with the reason:
//!   no symbol in the ROM (built out), no aligned ROM word pointing into
//!   the symbol (unreferenced), or pret's name or path calling it unused or
//!   debug, or a known piece of cut content ([`CUT`]), or a tileset
//!   palette slot the map loader never copies ([`never_loaded`]), or a
//!   battle-animation sheet or palette whose `ANIM_TAG_*` no battle script
//!   loads ([`unscripted`]).
//! - **Found**: a compressed blob's start, or a byte of a copied range, from
//!   a fuzz run (`out/fuzz/NAME/w*/events.tsv`, its `packed` and `copied`
//!   rows; the route phase scored on its own too) or a route's manifest
//!   (`manifest.toml`'s `[tiles]`, `[palettes]`, `[tilemaps]`) inside the
//!   file's range; for a palette, a fuzz run's ROM load (`load` rows)
//!   reaching into it too, since the fades' colour maths loses the copied
//!   bytes' tags on the way to palette RAM; for a 1bpp image (footprints,
//!   Spinda's spots), a fuzz run's computed bytes (`table` rows: the table
//!   the provenance tags guess they were built from), since the game
//!   expands those in code and never copies them as they are; for a
//!   compressed file, a `table` row at its blob's start (its unpacked data
//!   rewritten in code before it reached VRAM, as a contest appeal's
//!   backgrounds have their palette bits relocated).
//! - **Palette**: a found image is "with palette" when a palette file pret
//!   keeps beside it (its folder, or the same file name, as
//!   `items/icons/potion.png` and `items/icon_palettes/potion.pal`) was
//!   found too, or for a manifest, when the export drew it with one;
//!   "greyscale" when none was, and "unpaired" when pret keeps no palette
//!   beside it (Pokémon icons share `icon_palettes/`).
//! - **Scan only**: with `--scan`, a file the run missed whose compressed
//!   blob [`crate::scan`] finds at its start: there as a greyscale sheet,
//!   without palette or layout, and not counted as found.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

/// What a pret file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Image,
    Palette,
    Tilemap,
}

/// One `INCGFX`, as the element `index` of `count` in its symbol's
/// initializer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inc {
    pub symbol: String,
    /// The C file it's in, relative to pret's root.
    pub file: String,
    pub source: String,
    pub ext: String,
    pub index: usize,
    pub count: usize,
}

/// Every `INCGFX` in one C source's text, with the symbol whose
/// initializer holds it.
pub fn parse(text: &str, file: &str) -> Vec<Inc> {
    let mut out: Vec<Inc> = Vec::new();
    let mut start = 0;
    // One declaration at a time: up to each `;`.
    for stmt in text.split_inclusive(';') {
        let base = start;
        start += stmt.len();
        if !stmt.contains("INCGFX_U") {
            continue;
        }
        let Some(eq) = stmt.find('=') else { continue };
        let lhs = &stmt[..eq];
        let lhs = lhs.find('[').map_or(lhs, |b| &lhs[..b]).trim_end();
        let symbol: String = lhs
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if symbol.is_empty() {
            continue;
        }
        let first = out.len();
        let mut rest = &text[base + eq..base + stmt.len()];
        while let Some(i) = rest.find("INCGFX_U") {
            rest = &rest[i + 8..];
            let Some(open) = rest.find('(') else { break };
            let Some((source, r)) = quoted(&rest[open + 1..]) else {
                continue;
            };
            let Some((ext, _)) = r.trim_start().strip_prefix(',').and_then(quoted) else {
                continue;
            };
            out.push(Inc {
                symbol: symbol.clone(),
                file: file.into(),
                source: source.into(),
                ext: ext.into(),
                index: out.len() - first,
                count: 0,
            });
        }
        let n = out.len() - first;
        for i in &mut out[first..] {
            i.count = n;
        }
    }
    out
}

fn quoted(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start().strip_prefix('"')?;
    let end = s.find('"')?;
    Some((&s[..end], &s[end + 1..]))
}

/// Every C source and header under pret's `src/`, in path order.
fn sources(pret: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut files = Vec::new();
    let mut stack = vec![pret.join("src")];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "c" || x == "h") {
                files.push(p);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Every `INCGFX` under pret's `src/`, in path order.
pub fn scan(pret: &Path) -> Result<Vec<Inc>, String> {
    let mut out = Vec::new();
    for p in sources(pret)? {
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let rel = p.strip_prefix(pret).unwrap_or(&p).display().to_string();
        out.extend(parse(&text, &rel));
    }
    Ok(out)
}

/// [`unscripted`] on pret's tree: `src/data/battle_anim.h`'s tag tables,
/// `data/battle_anim_scripts.s` and the rest of `src/`.
pub fn scan_unscripted(pret: &Path) -> Result<BTreeMap<String, String>, String> {
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let table_path = pret.join("src/data/battle_anim.h");
    let table = read(&table_path)?;
    let scripts = read(&pret.join("data/battle_anim_scripts.s"))?;
    let mut code = Vec::new();
    for p in sources(pret)? {
        if p != table_path
            && let Ok(t) = std::fs::read_to_string(&p)
        {
            code.push(t);
        }
    }
    let code: Vec<&str> = code.iter().map(String::as_str).collect();
    Ok(unscripted(&table, &scripts, &code))
}

/// The class of what `source` built as `ext` holds.
pub fn class(source: &str, ext: &str) -> Class {
    if ext.contains("gbapal") || source.ends_with(".pal") || source.ends_with(".gbapal") {
        Class::Palette
    } else if source.ends_with(".bin") {
        Class::Tilemap
    } else {
        Class::Image
    }
}

/// The kind a pret file is reported under, from its path.
pub fn kind(source: &str) -> &'static str {
    let name = source.rsplit('/').next().unwrap_or(source);
    let has = |p: &str| source.contains(p);
    if has("graphics/pokemon/") {
        if has("icon") {
            "Pokémon icons"
        } else if name.starts_with("footprint") {
            "Pokémon footprints"
        } else {
            "Pokémon pictures"
        }
    } else if has("graphics/items/") {
        "item icons"
    } else if has("graphics/trainers/") {
        "trainers"
    } else if has("data/tilesets/") {
        "tilesets"
    } else if has("graphics/object_events/") {
        "overworld objects"
    } else if has("graphics/battle_anims/") {
        "battle animations"
    } else if has("graphics/battle_") {
        "battle interface and transitions"
    } else if has("graphics/fonts/") {
        "fonts"
    } else if has("graphics/contest/") || has("graphics/battle_frontier/") {
        "contests and Battle Frontier"
    } else {
        "menus and other screens"
    }
}

/// Pieces of cut content pret keeps, referenced from the ROM but never
/// drawn in play: (part of the symbol, why).
const CUT: &[(&str, &str)] = &[
    (
        "CircledQuestionMark",
        "SPECIES_NONE's picture, no Pokémon is ever species 0",
    ),
    (
        "DoubleQuestionMark",
        "the 25 cut old-Unown species' picture",
    ),
];

/// A tileset palette slot the game never loads. Every tileset keeps 16,
/// but `LoadTilesetPalette` (pret's `fieldmap.c`) copies a primary's 0-5
/// and a secondary's 6-12 (`NUM_PALS_IN_PRIMARY`, `NUM_PALS_TOTAL`), and
/// nothing else reads them: except the secret base's primary, whose any
/// slot `decoration.c` copies for a decoration's picture, so it's kept.
fn never_loaded(inc: &Inc) -> Option<&'static str> {
    if !inc.symbol.starts_with("gTilesetPalettes_")
        || inc.symbol == "gTilesetPalettes_SecretBase"
        || inc.count != 16
    {
        return None;
    }
    if inc.source.contains("tilesets/primary/") {
        (inc.index >= 6).then_some(
            "never loaded: a primary tileset's palettes 6-15 (the map loader copies 0-5)",
        )
    } else {
        (!(6..13).contains(&inc.index))
            .then_some("never loaded: a secondary tileset's palettes 0-5 and 13-15 (the map loader copies 6-12)")
    }
}

/// The battle-animation sheets and palettes no play loads, by symbol,
/// with why. Animations load them by tag from the tag tables
/// (`gBattleAnimPicTable`, `gBattleAnimPaletteTable` in `table`, pret's
/// `src/data/battle_anim.h`): a script's `loadspritegfx ANIM_TAG_X` (in
/// `scripts`, `data/battle_anim_scripts.s`), or C indexing a table by the
/// tag (`gBattleAnimPicTable[GET_TRUE_SPRITE_INDEX(ANIM_TAG_ROCKS)]`, in
/// `code`: the intro's rocks, the ball's gold stars, the Pokéblock, the
/// status impact). A symbol every tag of which is loaded neither way, and
/// which no C outside the table names, has only the table's pointer: the
/// ROM word that makes it count as referenced, but no play reaches it.
pub fn unscripted(table: &str, scripts: &str, code: &[&str]) -> BTreeMap<String, String> {
    let mut loaded: HashSet<&str> = HashSet::new();
    let mut rest = scripts;
    while let Some(i) = rest.find("loadspritegfx") {
        rest = &rest[i + 13..];
        loaded.extend(ident(rest.trim_start()).filter(|t| t.starts_with("ANIM_TAG_")));
    }
    for c in code {
        for t in ["gBattleAnimPicTable[", "gBattleAnimPaletteTable["] {
            let mut rest = *c;
            while let Some(i) = rest.find(t) {
                rest = &rest[i + t.len()..];
                let inner = &rest[..rest.find(']').unwrap_or(rest.len())];
                if let Some(j) = inner.find("ANIM_TAG_") {
                    loaded.extend(ident(&inner[j..]));
                }
            }
        }
    }
    // `{gBattleAnimSpriteGfx_Fangs, 0x0400, ANIM_TAG_FANGS},`: each
    // symbol's tags.
    let mut tags: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in table.split('{').skip(1) {
        let e = &e[..e.find('}').unwrap_or(e.len())];
        let Some(sym) = ident(e.trim_start()) else {
            continue;
        };
        let Some(tag) = e.rsplit(',').next().map(str::trim) else {
            continue;
        };
        if tag.starts_with("ANIM_TAG_") && ident(tag) == Some(tag) {
            tags.entry(sym).or_default().push(tag);
        }
    }
    let named_elsewhere = |sym: &str| {
        code.iter().any(|c| {
            c.match_indices(sym).any(|(i, _)| {
                let word =
                    |ch: Option<char>| ch.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
                let line_at = c[..i].rfind('\n').map_or(0, |n| n + 1);
                let line = &c[line_at..c[i..].find('\n').map_or(c.len(), |n| i + n)];
                !word(c[..i].chars().next_back())
                    && !word(c[i + sym.len()..].chars().next())
                    && !line.contains("INCGFX")
                    && !line.contains("extern")
            })
        })
    };
    tags.into_iter()
        .filter(|(s, ts)| !ts.iter().any(|t| loaded.contains(t)) && !named_elsewhere(s))
        .map(|(s, _)| {
            let why =
                "unscripted: no battle script loads its ANIM_TAG, only the tag table points at it";
            (s.to_string(), why.to_string())
        })
        .collect()
}

/// The identifier `s` starts with.
fn ident(s: &str) -> Option<&str> {
    let n = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    (n > 0).then(|| &s[..n])
}

/// Why a symbol at `addr` is never shown in play, if it isn't; `unscripted`
/// is [`unscripted`]'s.
fn unused(
    inc: &Inc,
    sym: Option<(u32, u32)>,
    words: &BTreeSet<u32>,
    unscripted: &BTreeMap<String, String>,
) -> Option<String> {
    let Some((addr, size)) = sym else {
        return Some("not in the ROM: no symbol in the source map".into());
    };
    let low = |s: &str| s.to_ascii_lowercase();
    if low(&inc.symbol).contains("unused") || low(&inc.source).contains("unused") {
        return Some("pret names it unused".into());
    }
    if low(&inc.symbol).contains("debug") || low(&inc.source).contains("debug") {
        return Some("debug".into());
    }
    if let Some((_, why)) = CUT.iter().find(|(s, _)| inc.symbol.contains(s)) {
        return Some(format!("cut content: {why}"));
    }
    if let Some(why) = never_loaded(inc) {
        return Some(why.into());
    }
    if let Some(why) = unscripted.get(&inc.symbol) {
        return Some(why.clone());
    }
    // Into it, not only at it: code indexes some (`sButtons_Gfx + 0x80`).
    if words.range(addr..addr + size.max(1)).next().is_none() {
        return Some("unreferenced: no ROM word points into its symbol".into());
    }
    None
}

/// One pret file: a source built one way, with the ROM ranges of the
/// symbols (or elements) holding it.
#[derive(Debug, Clone)]
pub struct File {
    pub source: String,
    pub ext: String,
    pub class: Class,
    pub kind: &'static str,
    pub symbols: Vec<String>,
    /// `(start, end, compressed)`.
    pub ranges: Vec<(u32, u32, bool)>,
    /// Why it's never shown in play (all its symbols), if so.
    pub unused: Option<String>,
}

/// A source map data symbol: address, name, size, object file.
pub type Symbol = (u32, String, u32, Option<String>);

/// pret's files joined to the ROM: `data` is the source map's data
/// symbols, `rom` the ROM's bytes, `unscripted` [`unscripted`]'s symbols.
pub fn files(
    incs: &[Inc],
    data: &[Symbol],
    rom: &[u8],
    unscripted: &BTreeMap<String, String>,
) -> Vec<File> {
    let words: BTreeSet<u32> = rom
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect();
    let mut by_name: HashMap<&str, Vec<&Symbol>> = HashMap::new();
    for d in data.iter().filter(|d| d.0 >= 0x0800_0000) {
        by_name.entry(d.1.as_str()).or_default().push(d);
    }
    // Elements of one symbol share its range evenly when uncompressed and
    // all built alike.
    let mut alike: HashMap<(&str, &str), (&str, bool)> = HashMap::new();
    for i in incs {
        let e = alike.entry((&i.file, &i.symbol)).or_insert((&i.ext, true));
        e.1 &= !packed(&i.ext) && e.0 == i.ext;
    }
    let mut out: BTreeMap<(String, String), File> = BTreeMap::new();
    let mut whys: BTreeMap<(String, String), Vec<Option<String>>> = BTreeMap::new();
    for i in incs {
        let cands = by_name.get(i.symbol.as_str());
        let sym = cands.and_then(|c| {
            c.iter()
                .find(|d| {
                    d.3.as_deref()
                        .is_some_and(|f| Path::new(&i.file).file_stem() == Path::new(f).file_stem())
                })
                .or(c.first())
        });
        let range = sym.map(|d| {
            let size = d.2.max(1);
            if i.count > 1 && alike[&(i.file.as_str(), i.symbol.as_str())].1 {
                let each = size / i.count as u32;
                let s = d.0 + each * i.index as u32;
                (s, s + each.max(1), false)
            } else {
                (d.0, d.0 + size, packed(&i.ext))
            }
        });
        let key = (i.source.clone(), i.ext.clone());
        let f = out.entry(key.clone()).or_insert_with(|| File {
            source: i.source.clone(),
            ext: i.ext.clone(),
            class: class(&i.source, &i.ext),
            kind: kind(&i.source),
            symbols: Vec::new(),
            ranges: Vec::new(),
            unused: None,
        });
        if !f.symbols.contains(&i.symbol) {
            f.symbols.push(i.symbol.clone());
        }
        let why = unused(i, sym.map(|d| (d.0, d.2)), &words, unscripted);
        if why.is_none()
            && let Some(r) = range
        {
            f.ranges.push(r);
        }
        whys.entry(key).or_default().push(why);
    }
    for (k, f) in out.iter_mut() {
        // Used if any of its symbols is.
        let w = &whys[k];
        if w.iter().all(Option::is_some) {
            f.unused = w.iter().flatten().next().cloned();
        }
    }
    out.into_values().collect()
}

fn packed(ext: &str) -> bool {
    ext.ends_with(".lz") || ext.ends_with(".rl")
}

/// What a run found: compressed blobs by start, and copied ROM bytes.
/// `paired` is a manifest's word on whether a blob was drawn with a
/// palette.
#[derive(Debug, Default)]
pub struct Found {
    pub packed: HashSet<u32>,
    pub bytes: BTreeSet<u32>,
    pub paired: HashMap<u32, bool>,
    /// A fuzz run's ROM loads (DMA, `CpuSet`, decompression) by start,
    /// with their end: what palettes are found by.
    pub loads: BTreeMap<u32, u32>,
    /// ROM bytes computed values were guessed to be built from: what 1bpp
    /// images are found by.
    pub computed: BTreeSet<u32>,
}

impl Found {
    /// Whether any of `ranges` was found, and if the run says, whether
    /// with a palette. A palette is found by a ROM load too: the game's
    /// fades recompute its colours in code, which the copied bytes' tags
    /// don't follow to palette RAM. A 1bpp image (`expanded`) is found by
    /// the bytes computed from it, and a compressed file by bytes computed
    /// from its unpacked data (tagged with the blob's start).
    fn hit(&self, ranges: &[(u32, u32, bool)], pal: bool, expanded: bool) -> Option<Option<bool>> {
        let mut hit = None;
        for &(s, e, compressed) in ranges {
            // Computed bytes guessed from a compressed blob are tagged with
            // its start: its unpacked data, rewritten in code on the way
            // to VRAM (a contest's tilemaps, their palette bits relocated).
            let computed = (expanded && self.computed.range(s..e).next().is_some())
                || (compressed && self.computed.contains(&s));
            let loaded = pal
                && (self.loads.range(s..e).next().is_some()
                    || self
                        .loads
                        .range(..s)
                        .next_back()
                        .is_some_and(|(_, &end)| end > s));
            if loaded
                || computed
                || self.packed.contains(&s)
                || self.bytes.range(s..e).next().is_some()
            {
                let p = self.paired.get(&s).copied();
                hit = Some(match (hit.flatten(), p) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (a, b) => a.or(b),
                });
            }
        }
        hit
    }
}

/// A fuzz run's `w*/events.tsv`: (the route phase's finds, everything's).
pub fn from_fuzz(dir: &Path) -> Result<(Found, Found), String> {
    from_fuzz_until(dir, f64::INFINITY).map(|(route, all, _)| (route, all))
}

/// [`from_fuzz`] for the first `until` seconds of the run, and the last
/// event's time.
pub fn from_fuzz_until(dir: &Path, until: f64) -> Result<(Found, Found, f64), String> {
    let mut last = 0f64;
    let mut route = Found::default();
    let mut all = Found::default();
    let mut ws: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path().join("events.tsv"))
        .filter(|p| p.exists())
        .collect();
    if ws.is_empty() {
        return Err(format!("{}: no w*/events.tsv", dir.display()));
    }
    ws.sort();
    for p in ws {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split('\t').collect();
            let [secs, phase, kind, addr, len, _] = f[..] else {
                continue;
            };
            let secs = secs.parse::<f64>().unwrap_or(0.0);
            last = last.max(secs);
            if secs > until {
                continue;
            }
            let (Ok(addr), Ok(len)) = (
                u32::from_str_radix(addr.trim_start_matches("0x"), 16),
                len.parse::<u32>(),
            ) else {
                continue;
            };
            for found in [Some(&mut all), (phase == "route").then_some(&mut route)]
                .into_iter()
                .flatten()
            {
                match kind {
                    "packed" => {
                        found.packed.insert(addr);
                    }
                    "copied" => found.bytes.extend(addr..addr + len),
                    "table" => found.computed.extend(addr..addr + len),
                    // The operation in the top byte, the source's length
                    // below.
                    "load" => {
                        let end = addr + (len & 0xff_ffff).max(1);
                        let e = found.loads.entry(addr).or_insert(end);
                        *e = (*e).max(end);
                    }
                    _ => {}
                }
            }
        }
    }
    Ok((route, all, last))
}

/// A fuzz run's coverage over time, scored every `step` seconds of run
/// time and at its end: a TSV line each of the seconds, the used files
/// found (with palette, greyscale, unpaired or found; not scan only), the
/// share, and the images, palettes and tilemaps among them.
pub fn curve(files: &[File], dir: &Path, step: f64) -> Result<String, String> {
    let (_, _, end) = from_fuzz_until(dir, f64::INFINITY)?;
    let used = files.iter().filter(|f| f.unused.is_none()).count();
    let mut out = String::from("secs\tfound\tused\tshare\timages\tpalettes\ttilemaps\n");
    let mut t = step;
    loop {
        let t_now = t.min(end);
        let (_, all, _) = from_fuzz_until(dir, t_now)?;
        let mut by = [0usize; 3];
        for (f, st) in files.iter().zip(score(files, &all)) {
            if matches!(st, Some(s) if s < Status::ScanOnly) {
                by[f.class as usize] += 1;
            }
        }
        let found: usize = by.iter().sum();
        let share = 100.0 * found as f64 / used.max(1) as f64;
        let [i, p, m] = by;
        let _ = writeln!(
            out,
            "{t_now:.0}\t{found}\t{used}\t{share:.1}\t{i}\t{p}\t{m}"
        );
        if t >= end {
            return Ok(out);
        }
        t += step;
    }
}

/// A route's export: its manifest's tile sheets, palettes and tilemaps.
pub fn from_manifest(text: &str) -> Result<Found, String> {
    let mut f = Found::default();
    for e in crate::pretgfx::manifest_entries(text)? {
        if e.store == "raw" {
            f.bytes.extend(e.addr..e.addr + e.size.max(1));
        } else {
            f.packed.insert(e.addr);
        }
        if e.kind == crate::graphics::Kind::Tiles {
            f.paired.insert(e.addr, e.paired);
        }
    }
    Ok(f)
}

/// A used file's standing in one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    WithPalette,
    Greyscale,
    Unpaired,
    Found,
    /// Missed by the run, but its blob found by the static scan.
    ScanOnly,
    Missing,
}

impl Status {
    fn tag(self) -> &'static str {
        match self {
            Status::WithPalette => "palette",
            Status::Greyscale => "grey",
            Status::Unpaired => "unpaired",
            Status::Found => "found",
            Status::ScanOnly => "scan",
            Status::Missing => "missing",
        }
    }
}

/// Each used file's status in a run (`None` for unused ones).
pub fn score(files: &[File], found: &Found) -> Vec<Option<Status>> {
    let dir = |s: &str| s.rsplit_once('/').map_or("", |d| d.0).to_string();
    let stem = |s: &str| {
        let n = s.rsplit('/').next().unwrap_or(s);
        n.split('.').next().unwrap_or(n).to_string()
    };
    // Found palettes, and every palette pret keeps, by folder and name.
    let mut pals: HashMap<String, bool> = HashMap::new();
    for f in files.iter().filter(|f| f.class == Class::Palette) {
        let hit = f.unused.is_none() && found.hit(&f.ranges, true, false).is_some();
        for k in [
            format!("d:{}", dir(&f.source)),
            format!("s:{}", stem(&f.source)),
        ] {
            *pals.entry(k).or_default() |= hit;
        }
    }
    files
        .iter()
        .map(|f| {
            if f.unused.is_some() {
                return None;
            }
            let Some(said) =
                found.hit(&f.ranges, f.class == Class::Palette, f.ext.contains("1bpp"))
            else {
                return Some(Status::Missing);
            };
            if f.class != Class::Image {
                return Some(Status::Found);
            }
            if let Some(p) = said {
                return Some(if p {
                    Status::WithPalette
                } else {
                    Status::Greyscale
                });
            }
            let near = [
                pals.get(&format!("d:{}", dir(&f.source))),
                pals.get(&format!("s:{}", stem(&f.source))),
            ];
            Some(if near.iter().flatten().any(|&&b| b) {
                Status::WithPalette
            } else if near.iter().any(Option::is_some) {
                Status::Greyscale
            } else {
                Status::Unpaired
            })
        })
        .collect()
}

/// `st` with each missing file whose compressed range starts at one of
/// `starts` (the static scan's blobs) marked [`Status::ScanOnly`].
pub fn with_scan(
    files: &[File],
    st: &[Option<Status>],
    starts: &HashSet<u32>,
) -> Vec<Option<Status>> {
    files
        .iter()
        .zip(st)
        .map(|(f, s)| match s {
            Some(Status::Missing) if f.ranges.iter().any(|&(a, _, p)| p && starts.contains(&a)) => {
                Some(Status::ScanOnly)
            }
            s => *s,
        })
        .collect()
}

/// The scan's blobs against pret's: how many start one of the source
/// map's data symbols (`named`), and of the used LZ77 files, how many it
/// finds (run-length files it doesn't look for).
pub fn scan_summary(files: &[File], named: &HashSet<u32>, starts: &HashSet<u32>) -> String {
    let at = starts.iter().filter(|a| named.contains(a)).count();
    let used = |ext: &str| {
        files
            .iter()
            .filter(|f| f.unused.is_none() && f.ext.ends_with(ext) && f.ranges.iter().any(|r| r.2))
            .collect::<Vec<_>>()
    };
    let lz = used(".lz");
    let hit = lz
        .iter()
        .filter(|f| f.ranges.iter().any(|&(a, _, p)| p && starts.contains(&a)))
        .count();
    format!(
        "\nstatic scan (gbaa-assets scan, LZ77 by Nintenlord's rule): {} blobs, {at} at the start of a \
         source map symbol, {} elsewhere; {hit} of the {} used LZ77 files found ({} run-length \
         ones not looked for)\n",
        starts.len(),
        starts.len() - at,
        lz.len(),
        used(".rl").len()
    )
}

#[derive(Default, Clone, Copy)]
struct Row {
    used: usize,
    palette: usize,
    grey: usize,
    unpaired: usize,
    found: usize,
    scan: usize,
}

impl Row {
    fn add(&mut self, s: Status) {
        self.used += 1;
        match s {
            Status::WithPalette => self.palette += 1,
            Status::Greyscale => self.grey += 1,
            Status::Unpaired => self.unpaired += 1,
            Status::Found => self.found += 1,
            Status::ScanOnly => self.scan += 1,
            Status::Missing => {}
        }
    }
    fn hits(&self) -> usize {
        self.palette + self.grey + self.unpaired + self.found
    }
    fn line(&self, name: &str) -> String {
        format!(
            "  {name:<34} {:>6} {:>6} {:>5.1}% {:>8} {:>6} {:>9} {:>9} {:>8}\n",
            self.used,
            self.hits(),
            100.0 * self.hits() as f64 / self.used.max(1) as f64,
            self.palette,
            self.grey,
            self.unpaired,
            self.scan,
            self.used - self.hits() - self.scan,
        )
    }
}

/// The report: the denominator, what's left out and why, then each run's
/// score by class and kind; and a TSV line per file.
pub fn report(files: &[File], runs: &[(&str, Vec<Option<Status>>)]) -> (String, String) {
    let mut t = String::new();
    let used = files.iter().filter(|f| f.unused.is_none()).count();
    let _ = writeln!(
        t,
        "# Emerald graphics coverage against pret (gbaa-assets coverage)\n\n\
         pret's files (INCGFX source and output kind): {}; used in play: {used}; left out: {}",
        files.len(),
        files.len() - used
    );
    let mut why: BTreeMap<String, usize> = BTreeMap::new();
    for f in files {
        if let Some(w) = &f.unused {
            *why.entry(w.clone()).or_default() += 1;
        }
    }
    for (w, n) in &why {
        let _ = writeln!(t, "  {n:>5}  {w}");
    }
    for (name, st) in runs {
        let mut total = Row::default();
        let mut by_class: BTreeMap<Class, Row> = BTreeMap::new();
        let mut by_kind: BTreeMap<(Class, &str), Row> = BTreeMap::new();
        for (f, s) in files.iter().zip(st) {
            let Some(s) = *s else { continue };
            total.add(s);
            by_class.entry(f.class).or_default().add(s);
            by_kind.entry((f.class, f.kind)).or_default().add(s);
        }
        let _ = writeln!(
            t,
            "\n## {name}: {} of {} used files found ({:.1}%){}\n\n  {:<34} {:>6} {:>6} {:>6} {:>8} {:>6} {:>9} {:>9} {:>8}",
            total.hits(),
            total.used,
            100.0 * total.hits() as f64 / total.used.max(1) as f64,
            if total.scan > 0 {
                format!(
                    "; with the scan's greyscale sheets {} ({:.1}%)",
                    total.hits() + total.scan,
                    100.0 * (total.hits() + total.scan) as f64 / total.used.max(1) as f64
                )
            } else {
                String::new()
            },
            "",
            "used",
            "found",
            "share",
            "palette",
            "grey",
            "unpaired",
            "scan only",
            "missing"
        );
        t += &total.line("all");
        for (c, r) in &by_class {
            t += &r.line(&format!("{c:?}s"));
            for ((_, k), r) in by_kind.range((*c, "")..).take_while(|((c2, _), _)| c2 == c) {
                t += &r.line(&format!("  {k}"));
            }
        }
    }
    let mut tsv = String::from("source\text\tclass\tkind\tsymbols\tranges\tunused");
    for (name, _) in runs {
        let _ = write!(tsv, "\t{name}");
    }
    tsv.push('\n');
    for (i, f) in files.iter().enumerate() {
        let ranges: Vec<String> = f
            .ranges
            .iter()
            .map(|(s, e, _)| format!("{s:#010x}+{}", e - s))
            .collect();
        let _ = write!(
            tsv,
            "{}\t{}\t{:?}\t{}\t{}\t{}\t{}",
            f.source,
            f.ext,
            f.class,
            f.kind,
            f.symbols.join(","),
            ranges.join(","),
            f.unused.as_deref().unwrap_or("")
        );
        for (_, st) in runs {
            let _ = write!(tsv, "\t{}", st[i].map_or("", Status::tag));
        }
        tsv.push('\n');
    }
    (t, tsv)
}
