//! The `#define` families a struct field takes (mod/value-names;
//! `docs/mod-view-design.md`, "Value names"): pret's constants headers
//! read for `#define NAME <number>` lines, and each scalar field of the
//! DWARF's types matched to the family its name spells in pret's style
//! (`optionsTextSpeed` takes `OPTIONS_TEXT_SPEED_*`) where pret's source
//! sets or compares a field of that name with one of the family's names
//! (`->optionsTextSpeed = OPTIONS_TEXT_SPEED_MID`): the name alone links
//! `cool` to `COOL_RIBBON_*`, which it never holds. And the enums a
//! field or global takes by its C type (native/lift-value-names: mzm's
//! `GameMode gMainGameMode`, c/switch's `PenState state`), from the
//! headers or the DWARF ([`enum_families`]). The names file gets the
//! names and values only, never pret's text.

use crate::dwarf::Layout;
use crate::quoted;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// One `#define NAME <number>` of a header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Define {
    pub name: String,
    pub value: u32,
    /// The header, from the source tree's root (`include/constants/global.h`).
    pub file: String,
}

/// A family of defines and the fields that take it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Family {
    /// The header its names are in.
    pub file: String,
    /// Each value's one name, in the header's order.
    pub names: Vec<(String, u32)>,
    /// `type.field`, by the type's C name (`struct SaveBlock2.optionsTextSpeed`).
    pub fields: Vec<String>,
    /// Globals of the enum's type that take it, by name (`gMainGameMode`;
    /// [`enum_families`]).
    pub globals: Vec<String>,
    /// Function parameters pret's calls pass its names to, as
    /// `function:index` by the names file's function name
    /// (`GetMonData2:1`; [`arg_families`]).
    pub args: Vec<String>,
    /// Those of `args` some call passes a sum one of its names is the
    /// base of (`MON_DATA_MOVE1 + j`).
    pub sums: Vec<String>,
}

/// Names that mark a family's bounds rather than one of its values.
const BOUNDS: &[&str] = &["COUNT", "MAX", "MIN", "FIRST", "LAST", "START", "END"];

/// The defines of `src`'s `include/constants/*.h` whose value is a
/// non-negative number literal (`0x1f`, `12`), by header name and then in
/// each header's order; a name defined twice keeps its first. A flat
/// tree with no `include/constants/` (a C program of `c/`) gives its own
/// `.h` and `.c` files' ([`flat`]).
pub fn read_defines(src: &Path) -> Vec<Define> {
    let dir = src.join("include/constants");
    let files = if dir.is_dir() {
        files_in(&dir, &["h"])
    } else {
        flat(src, &["h"])
    };
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for p in files {
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let file = p
            .strip_prefix(src)
            .unwrap_or(&p)
            .to_string_lossy()
            .into_owned();
        for d in defines_in(&text, &file) {
            if seen.insert(d.name.clone()) {
                out.push(d);
            }
        }
    }
    out
}

/// The files of `dir` (not below it) with one of `exts`, by name.
fn files_in(dir: &Path, exts: &[&str]) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| exts.iter().any(|e| x == *e)))
        .collect();
    files.sort();
    files
}

/// A flat tree's files (a C program of `c/`, its source beside nothing
/// else): `src`'s own `exts` files and the `.c` files too, by name.
fn flat(src: &Path, exts: &[&str]) -> Vec<std::path::PathBuf> {
    let mut all: Vec<&str> = exts.to_vec();
    if !all.contains(&"c") {
        all.push("c");
    }
    files_in(src, &all)
}

/// The `#define NAME <number>` lines of one header's `text`: an upper-case
/// name, then one number and nothing but a comment after it.
pub fn defines_in(text: &str, file: &str) -> Vec<Define> {
    let mut out = Vec::new();
    for l in text.lines() {
        let Some(rest) = l.trim_start().strip_prefix('#') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix("define") else {
            continue;
        };
        if !rest.starts_with([' ', '\t']) {
            continue;
        }
        let mut w = rest.split_whitespace();
        let (Some(name), Some(v)) = (w.next(), w.next()) else {
            continue;
        };
        if w.next()
            .is_some_and(|c| !c.starts_with("//") && !c.starts_with("/*"))
        {
            continue;
        }
        let upper = name.starts_with(|c: char| c.is_ascii_uppercase())
            && name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        let value = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            Some(h) => u32::from_str_radix(h, 16).ok(),
            None => v.parse().ok(),
        };
        if let (true, Some(value)) = (upper, value) {
            out.push(Define {
                name: name.to_string(),
                value,
                file: file.to_string(),
            });
        }
    }
    out
}

/// Each (field, name) pret's C sets or compares: `.f = NAME`, `->f =
/// NAME`, `.f == NAME` or `->f != NAME`, in `src/`'s `.c` and `.h` files
/// (designated initializers too), at any depth.
pub fn read_uses(src: &Path) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let mut dirs = vec![src.join("src")];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "c" || x == "h")
                && let Ok(text) = std::fs::read_to_string(&p)
            {
                out.extend(uses_in(&text));
            }
        }
    }
    out
}

/// [`read_uses`] of one file's `text`.
pub fn uses_in(text: &str) -> Vec<(String, String)> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let skip_ws = |mut i: usize| {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        i
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // A member after `.` or `->`.
        let start = match (b[i], b.get(i + 1)) {
            (b'.', _) => i + 1,
            (b'-', Some(b'>')) => i + 2,
            _ => {
                i += 1;
                continue;
            }
        };
        let mut j = start;
        while j < b.len() && word(b[j]) {
            j += 1;
        }
        if j == start || b[start].is_ascii_digit() {
            i = j.max(i + 1);
            continue;
        }
        let field = &text[start..j];
        let k = skip_ws(j);
        let op = match (b.get(k), b.get(k + 1)) {
            (Some(b'='), Some(b'=')) | (Some(b'!'), Some(b'=')) => k + 2,
            (Some(b'='), _) => k + 1,
            _ => {
                i = j;
                continue;
            }
        };
        let v = skip_ws(op);
        let mut w = v;
        while w < b.len() && word(b[w]) {
            w += 1;
        }
        if w > v && b[v].is_ascii_uppercase() {
            out.push((field.to_string(), text[v..w].to_string()));
        }
        i = j;
    }
    out
}

/// A C field name in pret's constant style: `optionsTextSpeed` is
/// `OPTIONS_TEXT_SPEED`.
pub fn upper_snake(name: &str) -> String {
    let mut o = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            o.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        o.push(c.to_ascii_uppercase());
    }
    o
}

/// The family each scalar field of `types` takes, by its prefix: the
/// defines named `<FIELD>_…` (in [`upper_snake`]) in the one header that
/// has the most of them, less the bounds (`…_COUNT`, `…_MAX`) and any
/// value two of them share. A field takes a family only if it has two
/// values left, every one fits the field (its bits, for a bitfield) and
/// `uses` (from [`read_uses`]) has a field of its name set or compared
/// with one of them and with no define of another header.
pub fn families(
    defines: &[Define],
    uses: &BTreeSet<(String, String)>,
    types: &BTreeMap<String, Layout>,
) -> BTreeMap<String, Family> {
    let mut out: BTreeMap<String, Family> = BTreeMap::new();
    let mut tried: BTreeMap<String, Option<Family>> = BTreeMap::new();
    let file_of: BTreeMap<&str, &str> = defines
        .iter()
        .map(|d| (d.name.as_str(), d.file.as_str()))
        .collect();
    for (tname, l) in types {
        let (Layout::Struct { fields, .. } | Layout::Union { fields, .. }) = l else {
            continue;
        };
        for f in fields {
            if f.size > 4
                || f.ty.contains(['*', '['])
                || f.ty.starts_with("struct ")
                || f.ty.starts_with("union ")
            {
                continue;
            }
            let prefix = upper_snake(&f.name);
            let fam = tried
                .entry(prefix.clone())
                .or_insert_with(|| family(defines, &prefix));
            let Some(fam) = fam else { continue };
            // Set or compared with one of its names, and with no define
            // of another header: a name two structs give two meanings
            // (`effect`, a move's and a contest move's) takes none.
            let mut with = uses
                .range((f.name.clone(), String::new())..)
                .take_while(|(x, _)| *x == f.name)
                .filter_map(|(_, n)| file_of.get(n.as_str()));
            let used = fam
                .names
                .iter()
                .any(|(n, _)| uses.contains(&(f.name.clone(), n.clone())));
            if !used || with.any(|&h| h != fam.file) {
                continue;
            }
            let bits = f.bits.map_or(f.size * 8, |(_, w)| w);
            let max = fam.names.iter().map(|&(_, v)| v).max().unwrap_or(0);
            if bits < 32 && max >> bits != 0 {
                continue;
            }
            out.entry(prefix)
                .or_insert_with(|| fam.clone())
                .fields
                .push(format!("{tname}.{}", f.name));
        }
    }
    out
}

/// The family of `prefix`, before its fields: see [`families`].
fn family(defines: &[Define], prefix: &str) -> Option<Family> {
    let p = format!("{prefix}_");
    let mut by_file: BTreeMap<&str, Vec<&Define>> = BTreeMap::new();
    for d in defines {
        let Some(rest) = d.name.strip_prefix(&p) else {
            continue;
        };
        if rest.is_empty() || BOUNDS.contains(&rest) {
            continue;
        }
        by_file.entry(&d.file).or_default().push(d);
    }
    // The header with the most; of two, the first by name.
    let (file, ds) = by_file.into_iter().rev().max_by_key(|(_, ds)| ds.len())?;
    let mut count: BTreeMap<u32, usize> = BTreeMap::new();
    for d in &ds {
        *count.entry(d.value).or_default() += 1;
    }
    let names: Vec<(String, u32)> = ds
        .iter()
        .filter(|d| count[&d.value] == 1)
        .map(|d| (d.name.clone(), d.value))
        .collect();
    (names.len() >= 2).then(|| Family {
        file: file.to_string(),
        names,
        ..Family::default()
    })
}

/// The names file's `[values]` tables.
pub fn toml(fams: &BTreeMap<String, Family>) -> String {
    let mut o = String::from(
        "\n# #define families of pret's include/constants/ that a field takes, by\n\
         # the prefix its name spells (`optionsTextSpeed`: OPTIONS_TEXT_SPEED_*),\n\
         # and enums a field or global takes by its type (`GameMode`):\n\
         # each value's one name, the fields, as `type.field`, the globals,\n\
         # and the parameters pret's calls pass them to, as `function:index`.\n",
    );
    for (prefix, f) in fams {
        writeln!(o, "\n[values.{}]", quoted(prefix)).unwrap();
        writeln!(o, "file = {}", quoted(&f.file)).unwrap();
        let fields: Vec<String> = f.fields.iter().map(|s| quoted(s)).collect();
        writeln!(o, "fields = [{}]", fields.join(", ")).unwrap();
        if !f.globals.is_empty() {
            let gs: Vec<String> = f.globals.iter().map(|s| quoted(s)).collect();
            writeln!(o, "globals = [{}]", gs.join(", ")).unwrap();
        }
        if !f.args.is_empty() {
            let xs: Vec<String> = f.args.iter().map(|s| quoted(s)).collect();
            writeln!(o, "args = [{}]", xs.join(", ")).unwrap();
        }
        if !f.sums.is_empty() {
            let xs: Vec<String> = f.sums.iter().map(|s| quoted(s)).collect();
            writeln!(o, "sums = [{}]", xs.join(", ")).unwrap();
        }
        o.push_str("names = [\n");
        for (n, v) in &f.names {
            writeln!(o, "  {{ name = {}, value = {v} }},", quoted(n)).unwrap();
        }
        o.push_str("]\n");
    }
    o
}

/// An enum's enumerators (native/lift-value-names): from a header's
/// `enum Name { … }` or mzm's `MAKE_ENUM(u8, Name) { … }` (a typedef
/// `Name` of that integer and `enum Name_`), or from the DWARF's named
/// enums. A field or global whose C type is the enum's (`GameMode
/// gMainGameMode`, `PenState state`) takes it: the type is the link, so
/// no use in the source is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enum {
    /// The type name the fields use: the typedef's, or the enum's tag
    /// (`GameMode`, `PenState`).
    pub name: String,
    /// The header it is in, or `""` for the DWARF's.
    pub file: String,
    pub values: Vec<(String, u32)>,
}

/// The enums of `src`'s `include/constants/*.h` ([`enums_in`]), by header
/// name.
pub fn read_enums(src: &Path) -> Vec<Enum> {
    let dir = src.join("include/constants");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "h"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for p in files {
        if let Ok(text) = std::fs::read_to_string(&p) {
            let file = format!(
                "include/constants/{}",
                p.file_name().unwrap().to_string_lossy()
            );
            out.extend(enums_in(&text, &file));
        }
    }
    out
}

/// `text` with its comments blanked.
fn uncommented(text: &str) -> String {
    let mut o = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("//") {
            let end = r.find('\n').unwrap_or(r.len());
            rest = &r[end..];
            o.push(' ');
        } else if let Some(r) = rest.strip_prefix("/*") {
            let end = r.find("*/").map_or(r.len(), |e| e + 2);
            rest = &r[end..];
            o.push(' ');
        } else {
            let c = rest.chars().next().unwrap();
            o.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    o
}

/// The enums of one header's `text`: `enum Name { … }` and `MAKE_ENUM(t,
/// Name) { … }`, each enumerator a number literal, an earlier enumerator
/// of it, or one more than the one before (from 0). An enum with any
/// other value (an expression, a negative number) or a preprocessor line
/// inside is left out whole.
pub fn enums_in(text: &str, file: &str) -> Vec<Enum> {
    let mut es = enums_any(text, file);
    es.retain(|e| !e.name.is_empty());
    es
}

/// [`enums_in`] with the anonymous enums too (`enum { MON_DATA_HP, … }`),
/// each with an empty name.
pub fn enums_any(text: &str, file: &str) -> Vec<Enum> {
    let t = uncommented(text);
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(k) = t[i..].find(['e', 'M']).map(|k| k + i) {
        i = k + 1;
        if k > 0 && t[..k].ends_with(word) {
            continue;
        }
        let rest = &t[k..];
        let (name, after) = if let Some(r) = rest.strip_prefix("enum") {
            if r.starts_with(word) {
                continue;
            }
            let r = r.trim_start();
            let n: String = r.chars().take_while(|&c| word(c)).collect();
            (n.clone(), &r[n.len()..])
        } else if let Some(r) = rest.strip_prefix("MAKE_ENUM") {
            let r = r.trim_start();
            let Some(r) = r.strip_prefix('(') else {
                continue;
            };
            let Some(close) = r.find(')') else { continue };
            let Some((_, n)) = r[..close].split_once(',') else {
                continue;
            };
            (n.trim().to_string(), &r[close + 1..])
        } else {
            continue;
        };
        let Some(body) = after.trim_start().strip_prefix('{') else {
            continue;
        };
        let Some(end) = body.find('}') else { continue };
        if let Some(values) = enumerators(&body[..end]) {
            out.push(Enum {
                name,
                file: file.to_string(),
                values,
            });
        }
    }
    out
}

/// An enum body's enumerators and values, or `None` if one isn't a
/// number [`enums_in`] can give.
fn enumerators(body: &str) -> Option<Vec<(String, u32)>> {
    if body.contains('#') {
        return None;
    }
    let mut out: Vec<(String, u32)> = Vec::new();
    let mut next: u64 = 0;
    for item in body.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (name, v) = match item.split_once('=') {
            Some((n, v)) => {
                let v = v.trim();
                let x = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
                    Some(h) => u64::from_str_radix(h, 16).ok(),
                    None => v.parse().ok(),
                };
                let x =
                    x.or_else(|| out.iter().find(|(n, _)| n == v).map(|&(_, x)| u64::from(x)))?;
                (n.trim(), x)
            }
            None => (item, next),
        };
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || name.is_empty()
            || name.starts_with(|c: char| c.is_ascii_digit())
        {
            return None;
        }
        out.push((name.to_string(), u32::try_from(v).ok()?));
        next = v + 1;
    }
    Some(out)
}

/// The DWARF's named enums ([`Layout::Enum`]) as [`Enum`]s: a negative
/// value leaves the enum out.
pub fn dwarf_enums(types: &BTreeMap<String, Layout>) -> Vec<Enum> {
    types
        .iter()
        .filter_map(|(n, l)| {
            let Layout::Enum { values, .. } = l else {
                return None;
            };
            let name = n.strip_prefix("enum ").unwrap_or(n).to_string();
            let values = values
                .iter()
                .map(|(n, v)| Some((n.clone(), u32::try_from(*v).ok()?)))
                .collect::<Option<Vec<_>>>()?;
            Some(Enum {
                name,
                file: String::new(),
                values,
            })
        })
        .collect()
}

/// Each `extern Type name;` (or `extern Type name[…];`'s array left out)
/// of `src`'s `include/` headers, at any depth: a global's C type by its
/// name, where no DWARF gives it (mzm's `extern GameMode gMainGameMode;`).
pub fn read_externs(src: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut dirs = vec![src.join("include")];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut ps: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        ps.sort();
        for p in ps {
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "h")
                && let Ok(text) = std::fs::read_to_string(&p)
            {
                for (n, t) in externs_in(&text) {
                    out.entry(n).or_insert(t);
                }
            }
        }
    }
    out
}

/// [`read_externs`] of one header's `text`: lines `extern <words> name;`
/// whose type is one or more plain words (qualifiers kept), by name.
pub fn externs_in(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for l in uncommented(text).lines() {
        let Some(rest) = l.trim().strip_prefix("extern ") else {
            continue;
        };
        let Some(decl) = rest.trim().strip_suffix(';') else {
            continue;
        };
        let words: Vec<&str> = decl.split_whitespace().collect();
        let ok = words.len() >= 2
            && words
                .iter()
                .all(|w| w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        if ok {
            let (name, ty) = words.split_last().unwrap();
            out.push((name.to_string(), ty.join(" ")));
        }
    }
    out
}

/// The enum a C type `ty` is, if it is one of `enums`' by name: `Name`,
/// `enum Name`, or the typedef `Name` of `MAKE_ENUM`'s `enum Name_`.
fn enum_of<'a>(enums: &'a BTreeMap<String, &'a Enum>, ty: &str) -> Option<&'a Enum> {
    let ty = ty
        .split_whitespace()
        .filter(|w| !matches!(*w, "const" | "volatile"))
        .collect::<Vec<_>>()
        .join(" ");
    let ty = ty.strip_prefix("enum ").unwrap_or(&ty);
    enums
        .get(ty)
        .or_else(|| enums.get(&format!("{ty}_")))
        .copied()
}

/// An enum's [`Family`], before its takers: its names less the bounds
/// (`…_COUNT`) and any value two share.
fn enum_family(e: &Enum) -> Family {
    let vals: Vec<&(String, u32)> = e
        .values
        .iter()
        .filter(|(n, _)| !BOUNDS.contains(&n.rsplit('_').next().unwrap_or(n)))
        .collect();
    let mut count: BTreeMap<u32, usize> = BTreeMap::new();
    for (_, v) in &vals {
        *count.entry(*v).or_default() += 1;
    }
    let names: Vec<(String, u32)> = vals
        .into_iter()
        .filter(|(_, v)| count[v] == 1)
        .cloned()
        .collect();
    Family {
        file: e.file.clone(),
        names,
        ..Family::default()
    }
}

/// The enum families ([`Family`]) the fields of `types` and the globals
/// of `globals` (each global's C type, by name) take by their type: per
/// enum, its names less the bounds (`…_COUNT`) and any value two share,
/// keyed by its type name less a trailing `_` (`GameMode`), where it has
/// two values left that every taker's width holds. A family's `fields`
/// are `type.field`; its globals `globals`. The first of two enums of one
/// name wins; a key `defines` already has is left to it.
pub fn enum_families(
    enums: &[Enum],
    types: &BTreeMap<String, Layout>,
    globals: &BTreeMap<String, (String, u32)>,
    defines: &BTreeMap<String, Family>,
) -> BTreeMap<String, Family> {
    let mut by: BTreeMap<String, &Enum> = BTreeMap::new();
    for e in enums {
        by.entry(e.name.clone()).or_insert(e);
    }
    let mut out: BTreeMap<String, Family> = BTreeMap::new();
    let fam = enum_family;
    let fits = |f: &Family, bits: u32| {
        let max = f.names.iter().map(|&(_, v)| v).max().unwrap_or(0);
        f.names.len() >= 2 && (bits >= 32 || max >> bits == 0)
    };
    let mut take = |e: &Enum, bits: u32, taker: String, global: bool| {
        let key = e.name.strip_suffix('_').unwrap_or(&e.name).to_string();
        if defines.contains_key(&key) {
            return;
        }
        let f = fam(e);
        if !fits(&f, bits) {
            return;
        }
        let f = out.entry(key).or_insert(f);
        if global {
            f.globals.push(taker);
        } else {
            f.fields.push(taker);
        }
    };
    for (tname, l) in types {
        let (Layout::Struct { fields, .. } | Layout::Union { fields, .. }) = l else {
            continue;
        };
        for f in fields {
            if f.size > 4 {
                continue;
            }
            if let Some(e) = enum_of(&by, &f.ty) {
                let bits = f.bits.map_or(f.size * 8, |(_, w)| w);
                take(e, bits, format!("{tname}.{}", f.name), false);
            }
        }
    }
    for (name, (ty, size)) in globals {
        if *size == 0 || *size > 4 {
            continue;
        }
        if let Some(e) = enum_of(&by, ty) {
            take(e, size * 8, name.clone(), true);
        }
    }
    out
}

/// One constant argument of a call in pret's C (native/lift-value-args):
/// `GetMonData(mon, MON_DATA_MAX_HP)` passes `MON_DATA_MAX_HP` as its
/// argument 1 of 2, and `GetMonData(mon, MON_DATA_MOVE1 + j)` its sum's
/// base `MON_DATA_MOVE1`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CallArg {
    /// The callee as the call writes it (a function or a macro).
    pub func: String,
    /// How many arguments the call passes.
    pub nargs: usize,
    pub index: usize,
    pub name: String,
    /// The argument is a sum `name` is the base of.
    pub sum: bool,
}

/// The [`CallArg`]s of `src/`'s `.c` files, at any depth; a flat tree's
/// own `.c` files where there is no `src/`.
pub fn read_calls(src: &Path) -> Vec<CallArg> {
    let mut out = Vec::new();
    if !src.join("src").is_dir() {
        for p in files_in(src, &["c"]) {
            if let Ok(text) = std::fs::read_to_string(&p) {
                out.extend(calls_in(&text));
            }
        }
        out.sort();
        return out;
    }
    let mut dirs = vec![src.join("src")];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "c")
                && let Ok(text) = std::fs::read_to_string(&p)
            {
                out.extend(calls_in(&text));
            }
        }
    }
    out.sort();
    out
}

/// Words a `(` follows that aren't calls.
const NOT_CALLS: &[&str] = &[
    "if", "while", "for", "switch", "return", "sizeof", "case", "do", "else",
];

/// [`read_calls`] of one file's `text`: each `f(a, b, …)` whose argument
/// is a constant's name alone (an upper-case word) or a sum it is the
/// base of (`NAME + j`). Calls inside arguments count too.
pub fn calls_in(text: &str) -> Vec<CallArg> {
    let t = uncommented(text);
    let b = t.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // Skip string and character literals.
        if b[i] == b'"' || b[i] == b'\'' {
            i = literal_end(b, i);
            continue;
        }
        if !word(b[i]) || (i > 0 && word(b[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && word(b[i]) {
            i += 1;
        }
        let func = &t[start..i];
        if func.as_bytes()[0].is_ascii_digit() || NOT_CALLS.contains(&func) {
            continue;
        }
        let mut k = i;
        while k < b.len() && b[k].is_ascii_whitespace() {
            k += 1;
        }
        if b.get(k) != Some(&b'(') {
            continue;
        }
        let Some(args) = split_args(&t, k) else {
            continue;
        };
        let nargs = args.len();
        for (index, a) in args.iter().enumerate() {
            if let Some((name, sum)) = const_base(a) {
                out.push(CallArg {
                    func: func.to_string(),
                    nargs,
                    index,
                    name: name.to_string(),
                    sum,
                });
            }
        }
    }
    out
}

/// The index past the string or character literal at `i`.
fn literal_end(b: &[u8], i: usize) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < b.len() && b[j] != q && b[j] != b'\n' {
        j += if b[j] == b'\\' { 2 } else { 1 };
    }
    j + 1
}

/// The arguments of the call whose `(` is at `open`, each trimmed, split
/// at the commas outside brackets and literals; `None` if it never closes.
fn split_args(t: &str, open: usize) -> Option<Vec<&str>> {
    let b = t.as_bytes();
    let mut depth = 0usize;
    let mut start = open + 1;
    let mut out = Vec::new();
    let mut j = open;
    while j < b.len() {
        match b[j] {
            b'"' | b'\'' => {
                j = literal_end(b, j);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    let last = t[start..j].trim();
                    if !(out.is_empty() && last.is_empty()) {
                        out.push(last);
                    }
                    return Some(out);
                }
            }
            b',' if depth == 1 => {
                out.push(t[start..j].trim());
                start = j + 1;
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// The constant an argument `a` is, or is the base of a sum of (then
/// `true`): an upper-case word of two or more characters alone, or before
/// `+`.
fn const_base(a: &str) -> Option<(&str, bool)> {
    let end = a
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(a.len());
    let (name, rest) = a.split_at(end);
    let upper = name.len() >= 2
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    let rest = rest.trim_start();
    let sum = rest.starts_with('+') && !rest.starts_with("++");
    (upper && (rest.is_empty() || sum)).then_some((name, sum))
}

/// The families pret's calls tie to function parameters (native/lift-
/// value-args), added to `fams` with each parameter in its `args`.
/// `resolve` gives the names file's function a call of `func` with
/// `nargs` arguments reaches (a macro's too, as pret's `GetMonData(…)`
/// that picks `GetMonData2` or `GetMonData3` by its count), or `None`.
///
/// A parameter takes a family only where every constant name pret's
/// calls pass it is one of that family's: a define family (each name a
/// define of one header, the family the longest prefix they share spells,
/// as [`families`] makes it) or an enum (each an enumerator of one
/// `enums`, keyed by its name, or an anonymous one's by its enumerators'
/// shared prefix). A name of no family or of another (`TRUE`, a define of
/// a second header), or names two enums both hold, ties none: the tie is
/// the decomp's or there is none.
pub fn arg_families(
    defines: &[Define],
    enums: &[Enum],
    calls: &[CallArg],
    resolve: impl Fn(&str, usize) -> Option<String>,
    fams: &mut BTreeMap<String, Family>,
) {
    let mut at: BTreeMap<(String, usize), BTreeSet<&str>> = BTreeMap::new();
    let mut sums: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut resolved: BTreeMap<(&str, usize), Option<String>> = BTreeMap::new();
    for c in calls {
        if c.index >= 4 {
            continue;
        }
        let f = resolved
            .entry((&c.func, c.nargs))
            .or_insert_with(|| resolve(&c.func, c.nargs));
        if let Some(f) = f {
            at.entry((f.clone(), c.index)).or_default().insert(&c.name);
            if c.sum {
                sums.insert((f.clone(), c.index));
            }
        }
    }
    let def: BTreeMap<&str, &Define> = defines.iter().map(|d| (d.name.as_str(), d)).collect();
    let mut enum_of: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, e) in enums.iter().enumerate() {
        for (n, _) in &e.values {
            enum_of.entry(n.as_str()).or_default().push(i);
        }
    }
    let mut tried: BTreeMap<String, Option<Family>> = BTreeMap::new();
    for ((f, index), names) in at {
        let taker = format!("{f}:{index}");
        let sum = sums.contains(&(f.clone(), index));
        let pick = if names.iter().all(|n| def.contains_key(n)) {
            define_family(defines, &def, &names, &mut tried)
        } else if names.iter().all(|n| enum_of.contains_key(n)) {
            enum_arg_family(enums, &enum_of, &names)
        } else {
            None
        };
        let Some((key, fam)) = pick else { continue };
        match fams.get_mut(&key) {
            // The same family, from a field's or global's tie.
            Some(have) if have.file == fam.file && have.names == fam.names => {
                if sum {
                    have.sums.push(taker.clone());
                }
                have.args.push(taker);
            }
            Some(_) => {}
            None => {
                let mut fam = fam;
                if sum {
                    fam.sums.push(taker.clone());
                }
                fam.args.push(taker);
                fams.insert(key, fam);
            }
        }
    }
}

/// The define family every one of `names` is in, by key: the longest
/// prefix of whole words they share (short of one name whole) whose
/// family is their header's and holds each of them by its prefix.
fn define_family(
    defines: &[Define],
    def: &BTreeMap<&str, &Define>,
    names: &BTreeSet<&str>,
    tried: &mut BTreeMap<String, Option<Family>>,
) -> Option<(String, Family)> {
    let file = &def[names.first()?].file;
    if names.iter().any(|n| &def[n].file != file) {
        return None;
    }
    let words: Vec<Vec<&str>> = names.iter().map(|n| n.split('_').collect()).collect();
    let shortest = words.iter().map(Vec::len).min()?;
    let mut common = 0;
    while common < shortest - 1 && words.iter().all(|w| w[common] == words[0][common]) {
        common += 1;
    }
    for len in (1..=common).rev() {
        let prefix = words[0][..len].join("_");
        let fam = tried
            .entry(prefix.clone())
            .or_insert_with(|| family(defines, &prefix));
        if let Some(fam) = fam
            && &fam.file == file
        {
            return Some((prefix, fam.clone()));
        }
    }
    None
}

/// The one enum every one of `names` is an enumerator of, as a family
/// keyed as [`enum_families`] keys it, or an anonymous enum by the prefix
/// of whole words its enumerators share.
fn enum_arg_family(
    enums: &[Enum],
    enum_of: &BTreeMap<&str, Vec<usize>>,
    names: &BTreeSet<&str>,
) -> Option<(String, Family)> {
    let mut all: Option<BTreeSet<usize>> = None;
    for n in names {
        let these: BTreeSet<usize> = enum_of[n].iter().copied().collect();
        all = Some(match all {
            None => these,
            Some(a) => a.intersection(&these).copied().collect(),
        });
    }
    // Two enums of one name (a header's and the DWARF's `enum Sound_`)
    // are one: the first, as in [`enum_families`]. Of two names, none.
    let key_of = |e: &Enum| e.name.strip_suffix('_').unwrap_or(&e.name).to_string();
    let mut it = all?.into_iter().map(|i| &enums[i]);
    let e = it.next()?;
    if it.any(|o| e.name.is_empty() || key_of(o) != key_of(e)) {
        return None;
    }
    let key = if e.name.is_empty() {
        let words: Vec<Vec<&str>> = e
            .values
            .iter()
            .map(|(n, _)| n.split('_').collect())
            .collect();
        let shortest = words.iter().map(Vec::len).min()?;
        let mut common = 0;
        while common < shortest - 1 && words.iter().all(|w| w[common] == words[0][common]) {
            common += 1;
        }
        if common == 0 {
            return None;
        }
        words[0][..common].join("_")
    } else {
        key_of(e)
    };
    let fam = enum_family(e);
    (fam.names.len() >= 2).then_some((key, fam))
}

/// The enums of `src`'s `include/` headers at any depth, anonymous ones
/// too ([`enums_any`]), by path; a flat tree's own `.h` and `.c` files'
/// where there is no `include/`.
pub fn read_all_enums(src: &Path) -> Vec<Enum> {
    let inc = src.join("include");
    let mut out = Vec::new();
    let mut dirs = vec![inc.clone()];
    let mut files = Vec::new();
    if !inc.is_dir() {
        files = flat(src, &["h"]);
        dirs.clear();
    }
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "h") {
                files.push(p);
            }
        }
    }
    files.sort();
    for p in files {
        if let Ok(text) = std::fs::read_to_string(&p) {
            let rel = p.strip_prefix(src).unwrap_or(&p).to_string_lossy();
            out.extend(enums_any(&text, &rel));
        }
    }
    out
}
