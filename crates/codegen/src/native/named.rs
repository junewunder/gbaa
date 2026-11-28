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
//! - A constant address in a named datum, wherever the code prints it
//!   (`let v3: u32 = 0x3001234u32;`, or inlined into its one use since
//!   native/lift-inline-temps: `r0 = 0x8000638u32;`, a call's argument;
//!   unit native/lift-data-names-inline): the datum's name, as a `pub
//!   const` in `main.rs`, with the address in a comment (`gSamusData /*
//!   0x03001234 */`, or `(gSamusData + 0x4) /* 0x03001238 */` inside it).
//!   ROM, EWRAM and IWRAM only. A sum can't be a match pattern, so one
//!   there stays a number, counted in [`Named::unnamed`].
//!
//! The map's identifiers ([`crate::names::SourceMap`]) are already valid
//! and unique among the map's names; a name that meets one of the crate's
//! own (the prelude's items, a local, a type: any word the crate's code has
//! but a field or method after a `.`, or one in a comment or string) gets a
//! `_` until it doesn't. A stand-in's own uses of the names are the lift's
//! ([`resolve`], before the lift reads the stand-ins), so they take none.

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
    /// Constants inside a named datum left as numbers (a sum in a match
    /// pattern): barred, at 0 on the C programs.
    pub unnamed: usize,
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

/// The words of `text` that may name a value or item: identifiers in its
/// code (not in a comment or string literal, whose words meet no name:
/// a stand-in's `why` is printed as a comment), not after a `.` (a field
/// or method) and not starting with a digit.
fn words(text: &str, out: &mut BTreeSet<String>) {
    for (s, e) in code_words(text) {
        out.insert(text[s..e].to_string());
    }
}

/// The words of `text`'s code ([`words`]).
pub fn code_word_set(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    words(text, &mut out);
    out
}

/// The spans of [`words`]: identifiers outside comments, string and char
/// literals, not after a `.` and not starting with a digit.
fn code_words(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'\'' if b.get(i + 2) == Some(&b'\'') || b.get(i + 1) == Some(&b'\\') => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            }
            c if word(c) => {
                let s = i;
                while i < b.len() && word(b[i]) {
                    i += 1;
                }
                if !b[s].is_ascii_digit() && !super::after_dot(b, s) {
                    out.push((s, i));
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// A stand-in's code (`[[standin]] code`) with the names files' names it
/// uses for the lift's functions and data as the lift prints them before
/// the names pass (unit `native/standin-lift-names`): a function's name
/// called (`Name(`) as its entry `f_<key>`, else as its key (`call_fn(g,
/// m4aSoundMain, …)`: `0x08150111u32`, the reach's and `call_fn`'s), and a
/// datum's as its address (`0x03001234u32`), which [`apply`] then prints
/// as the name again. A call by name is written as the lift's own is
/// before its later passes (`fill_rect(g, sp, x, y, w, h)?`: plain `u32`s,
/// `?`), which adapt it to the callee's signature as they do every call.
/// So a stand-in calls
/// `TaskGetNextSlot(g, sp)?` and reads `gCurTask`, and its words don't
/// take the names from the lift (which would print `TaskGetNextSlot_`).
/// A word after `.` (a field: `g.ram.gCurTask`) or `::` (a path) is left,
/// as are comments, strings and the words of `own` (the prelude's: a
/// stand-in's `Game` is the prelude's type, not a function named so).
pub fn resolve(
    code: &str,
    fn_names: &BTreeMap<u32, String>,
    data: &BTreeMap<u32, (String, u32)>,
    own: &BTreeSet<String>,
) -> String {
    let fns: BTreeMap<&str, u32> = fn_names.iter().map(|(&k, n)| (n.as_str(), k)).collect();
    let data: BTreeMap<&str, u32> = data.iter().map(|(&k, (n, _))| (n.as_str(), k)).collect();
    let b = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let mut last = 0;
    for (s, e) in code_words(code) {
        if s >= 2 && &b[s - 2..s] == b"::" {
            continue;
        }
        let w = &code[s..e];
        if own.contains(w) {
            continue;
        }
        let called = code[e..].trim_start().starts_with('(');
        let to = match (fns.get(w), data.get(w)) {
            (Some(k), _) if called => format!("f_{k:08x}"),
            (Some(k), _) => format!("{k:#010x}u32"),
            (None, Some(x)) => format!("{x:#010x}u32"),
            (None, None) => continue,
        };
        out.push_str(&code[last..s]);
        out.push_str(&to);
        last = e;
    }
    out.push_str(&code[last..]);
    out
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

/// The `0x<hex>u32` literals of lifted code `text`, as (start, end,
/// value), outside comments and string literals and not inside a longer
/// word: every form the printer gives a constant (`let v3: u32 =
/// 0x3001234u32;`, `r0 = 0x8000638u32;`, a call's argument, an operand).
fn literals(text: &str) -> Vec<(usize, usize, u32)> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            // A char literal holds no `"`; a lifetime is a word.
            b'\'' if b.get(i + 2) == Some(&b'\'') || b.get(i + 1) == Some(&b'\\') => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            }
            b'0' if b.get(i + 1) == Some(&b'x')
                && (i == 0 || !word(b[i - 1]) && b[i - 1] != b'.') =>
            {
                let s = i;
                let mut e = i + 2;
                while e < b.len() && b[e].is_ascii_hexdigit() {
                    e += 1;
                }
                let lit = text[e..].starts_with("u32") && b.get(e + 3).is_none_or(|&c| !word(c));
                let x = u32::from_str_radix(&text[s + 2..e], 16).ok();
                match x.filter(|_| lit && e > s + 2) {
                    Some(x) => {
                        out.push((s, e + 3, x));
                        i = e + 3;
                    }
                    None => i = e,
                }
            }
            c if word(c) => {
                while i < b.len() && word(b[i]) {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Whether the literal ending at `e` is a match pattern (`=>` or `|`
/// after it), where only a constant's own name, not a sum, may stand.
fn in_pattern(text: &str, e: usize) -> bool {
    let rest = text[e..].trim_start();
    rest.starts_with("=>") || (rest.starts_with('|') && !rest.starts_with("||"))
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
        for (_, _, x) in literals(t) {
            if addressable(x) {
                used.extend(datum(data, x));
            }
        }
    }
    let consts: BTreeMap<u32, String> = used.iter().map(|&k| (k, free(&data[&k].0))).collect();
    let (mut constants, mut unnamed) = (0, 0);
    for (_, t) in funcs.iter_mut() {
        let mut out = String::with_capacity(t.len() + 64);
        let mut last = 0;
        for (s, e, x) in literals(t) {
            let Some(k) = Some(x)
                .filter(|&x| addressable(x))
                .and_then(|x| datum(data, x))
            else {
                continue;
            };
            let n = &consts[&k];
            out.push_str(&t[last..s]);
            last = e;
            if x == k {
                write!(out, "{n} /* {x:#010x} */").unwrap();
            } else if in_pattern(t, e) {
                // `(gHeap + 0x8)` is no pattern: the number stays.
                out.push_str(&t[s..e]);
                unnamed += 1;
                continue;
            } else {
                write!(out, "({n} + {:#x}) /* {x:#010x} */", x - k).unwrap();
            }
            constants += 1;
        }
        out.push_str(&t[last..]);
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
        unnamed,
        const_lines,
    }
}

#[cfg(test)]
mod tests;
