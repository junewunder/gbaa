//! The names file's names in the lifted crate (unit `native/lift-fn-names`):
//! `gbaa-lift --names DIR` names RAM's globals ([`super::globals`]) and,
//! here, each lifted function and the data whose address the code takes
//! as a constant.
//!
//! - A function with a name: its entry, `f_<key>`, is printed as the name,
//!   at its definition and every call and table entry. Its body `x_<key>`
//!   and four-register adapter `a_<key>` keep the key: a stop's backtrace
//!   finds the lifted functions on the stack by those
//!   (`native_record::stop::functions_in`), and the body is defined right
//!   below the named entry (so a named function keeps its `x_` where an
//!   unnamed one's body would be its `f_`: `super::one_entry`).
//! - A constant address in a named datum (`let v3: u32 = 0x3001234u32;`,
//!   the only form the printer gives a constant): the datum's name, as a
//!   `pub const` in `main.rs`, with the address in a comment
//!   (`let v3: u32 = gSamusData /* 0x03001234 */;`, or `(gSamusData +
//!   0x4) /* 0x03001238 */` inside it). ROM, EWRAM and IWRAM only.
//!
//! The map's identifiers ([`crate::names::SourceMap`]) are already valid
//! and unique among the map's names; a name that meets one of the crate's
//! own (the prelude's items, a local, a type: any word the crate has but a
//! field or method after a `.`) gets a `_` until it doesn't.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// What the names pass named, for the report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Named {
    /// Lifted functions given their name, of [`Named::of_functions`].
    pub functions: usize,
    pub of_functions: usize,
    /// Data given a constant, and the constants printed as one.
    pub data: usize,
    pub constants: usize,
    /// The lines of the data constants in `main.rs` (not lifted code).
    pub const_lines: usize,
}

impl Named {
    /// The report's line.
    pub fn line(&self) -> String {
        format!(
            "names (gbaa-lift --names): {} of {} functions named; {} data named, at {} constants",
            self.functions, self.of_functions, self.data, self.constants
        )
    }
}

/// The words of `text` that may name a value or item: identifiers not
/// after a `.` (a field or method) and not starting with a digit.
fn words(text: &str, out: &mut BTreeSet<String>) {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    while i < b.len() {
        if !word(b[i]) {
            i += 1;
            continue;
        }
        let s = i;
        while i < b.len() && word(b[i]) {
            i += 1;
        }
        if !b[s].is_ascii_digit() && (s == 0 || b[s - 1] != b'.') {
            out.insert(text[s..i].to_string());
        }
    }
}

/// `f_`, `x_` or `a_` and eight hex digits, the lifter's numbered names.
fn numbered(w: &str) -> Option<u32> {
    let b = w.as_bytes();
    if b.len() == 10 && matches!(b[0], b'f' | b'x' | b'a') && b[1] == b'_' {
        u32::from_str_radix(&w[2..], 16).ok()
    } else {
        None
    }
}

/// `f_<key>` as a whole word replaced by `names[key]`, where it has one.
fn rename(text: &str, names: &BTreeMap<u32, String>) -> String {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(text.len());
    let (mut last, mut j) = (0, 0);
    while j + 10 <= b.len() {
        let hit = b[j] == b'f'
            && b[j + 1] == b'_'
            && (j == 0 || !word(b[j - 1]))
            && b.get(j + 10).is_none_or(|&c| !word(c))
            && b[j + 2..j + 10].iter().all(u8::is_ascii_hexdigit);
        let name = hit
            .then(|| u32::from_str_radix(&text[j + 2..j + 10], 16).ok())
            .flatten()
            .and_then(|k| names.get(&k));
        match name {
            Some(n) => {
                out.push_str(&text[last..j]);
                out.push_str(n);
                j += 10;
                last = j;
            }
            None => j += 1,
        }
    }
    out.push_str(&text[last..]);
    out
}

/// A constant's line, `let v<n>: u32 = 0x<hex>u32;`: the span of its
/// literal (up to the `;`) and its value.
fn constant(line: &str) -> Option<(usize, usize, u32)> {
    let t = line.trim_end();
    let (n, _) = t.trim_start().strip_prefix("let v")?.split_once(':')?;
    if n.is_empty() || !n.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let s = t.find(": u32 = 0x")? + ": u32 = ".len();
    let e = t.len() - 1;
    let hex = t.get(s + 2..)?.strip_suffix("u32;")?;
    let x = u32::from_str_radix(hex, 16).ok()?;
    Some((s, e, x))
}

/// ROM and work RAM, where a constant may be a datum's address.
fn addressable(x: u32) -> bool {
    (0x0800_0000..0x0a00_0000).contains(&x)
        || (0x0200_0000..0x0204_0000).contains(&x)
        || (0x0300_0000..0x0300_8000).contains(&x)
}

/// The datum `x` is in, by `data` (key → name and size, 0 unknown): its
/// key, if `x` is inside its size (one with no size holds only its own
/// address).
fn datum(data: &BTreeMap<u32, (String, u32)>, x: u32) -> Option<u32> {
    let (&k, (_, size)) = data.range(..=x).next_back()?;
    (x - k < (*size).max(1)).then_some(k)
}

/// The crate's texts with the names in: `head` (`main.rs` before its data
/// constants, which are appended), `table` (`fns.rs`'s) and each
/// function's text in `funcs`. `fn_names` are the functions' names by key
/// and `data` the data's (with sizes), as the names file gives them;
/// `lifted` the keys of every lifted function. `skip` is text of the
/// crate that only fields are named in (`Ram`), left out of the words.
pub fn apply(
    head: &mut String,
    skip: &str,
    table: &mut String,
    funcs: &mut [(u32, String)],
    lifted: &[u32],
    fn_names: &BTreeMap<u32, String>,
    data: &BTreeMap<u32, (String, u32)>,
) -> Named {
    let mut taken = BTreeSet::new();
    let body = head.replace(skip, "");
    words(&body, &mut taken);
    words(table, &mut taken);
    for (_, t) in funcs.iter() {
        words(t, &mut taken);
    }
    taken.retain(|w| numbered(w).is_none());
    // A name that's taken takes `_`s until it isn't.
    let mut free = |base: &str| -> String {
        let mut id = base.to_string();
        while taken.contains(&id) {
            id.push('_');
        }
        taken.insert(id.clone());
        id
    };
    let mut names = BTreeMap::new();
    for &k in lifted {
        if let Some(n) = fn_names.get(&k) {
            names.insert(k, free(n));
        }
    }
    // The data each function's constants are in.
    let mut used = BTreeSet::new();
    for (_, t) in funcs.iter() {
        for line in t.lines() {
            if let Some((_, _, x)) = constant(line).filter(|c| addressable(c.2)) {
                used.extend(datum(data, x));
            }
        }
    }
    let consts: BTreeMap<u32, String> = used.iter().map(|&k| (k, free(&data[&k].0))).collect();
    let mut constants = 0;
    for (_, t) in funcs.iter_mut() {
        let mut out = String::with_capacity(t.len() + 64);
        for line in t.split_inclusive('\n') {
            let c = constant(line).filter(|c| addressable(c.2));
            match c.and_then(|(s, e, x)| Some((s, e, x, datum(data, x)?))) {
                Some((s, e, x, k)) => {
                    let n = &consts[&k];
                    out.push_str(&line[..s]);
                    if x == k {
                        write!(out, "{n} /* {x:#010x} */").unwrap();
                    } else {
                        write!(out, "({n} + {:#x}) /* {x:#010x} */", x - k).unwrap();
                    }
                    out.push_str(&line[e..]);
                    constants += 1;
                }
                None => out.push_str(line),
            }
        }
        *t = rename(&out, &names);
    }
    *table = rename(table, &names);
    *head = rename(head, &names);
    let mut const_lines = 0;
    if !consts.is_empty() {
        head.push_str(
            "\n// The data whose address the code takes, from the names file (gbaa-lift --names).\n",
        );
        const_lines += 2;
        for (k, n) in &consts {
            writeln!(head, "pub const {n}: u32 = {k:#010x};").unwrap();
            const_lines += 1;
        }
    }
    Named {
        functions: names.len(),
        of_functions: lifted.len(),
        data: consts.len(),
        constants,
        const_lines,
    }
}

#[cfg(test)]
mod tests;
