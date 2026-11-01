//! The `#define` families a struct field takes (mod/value-names;
//! `docs/mod-view-design.md`, "Value names"): pret's constants headers
//! read for `#define NAME <number>` lines, and each scalar field of the
//! DWARF's types matched to the family its name spells in pret's style
//! (`optionsTextSpeed` takes `OPTIONS_TEXT_SPEED_*`) where pret's source
//! sets or compares a field of that name with one of the family's names
//! (`->optionsTextSpeed = OPTIONS_TEXT_SPEED_MID`): the name alone links
//! `cool` to `COOL_RIBBON_*`, which it never holds. The names file gets
//! the names and values only, never pret's text.

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
}

/// Names that mark a family's bounds rather than one of its values.
const BOUNDS: &[&str] = &["COUNT", "MAX", "MIN", "FIRST", "LAST", "START", "END"];

/// The defines of `src`'s `include/constants/*.h` whose value is a
/// non-negative number literal (`0x1f`, `12`), by header name and then in
/// each header's order; a name defined twice keeps its first. A tree
/// with no such headers has none.
pub fn read_defines(src: &Path) -> Vec<Define> {
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
    let mut seen = BTreeSet::new();
    for p in files {
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let file = format!(
            "include/constants/{}",
            p.file_name().unwrap().to_string_lossy()
        );
        for d in defines_in(&text, &file) {
            if seen.insert(d.name.clone()) {
                out.push(d);
            }
        }
    }
    out
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
        fields: Vec::new(),
    })
}

/// The names file's `[values]` tables.
pub fn toml(fams: &BTreeMap<String, Family>) -> String {
    let mut o = String::from(
        "\n# #define families of pret's include/constants/ that a field takes, by\n\
         # the prefix its name spells (`optionsTextSpeed`: OPTIONS_TEXT_SPEED_*):\n\
         # each value's one name, and the fields, as `type.field`.\n",
    );
    for (prefix, f) in fams {
        writeln!(o, "\n[values.{}]", quoted(prefix)).unwrap();
        writeln!(o, "file = {}", quoted(&f.file)).unwrap();
        let fields: Vec<String> = f.fields.iter().map(|s| quoted(s)).collect();
        writeln!(o, "fields = [{}]", fields.join(", ")).unwrap();
        o.push_str("names = [\n");
        for (n, v) in &f.names {
            writeln!(o, "  {{ name = {}, value = {v} }},", quoted(n)).unwrap();
        }
        o.push_str("]\n");
    }
    o
}
