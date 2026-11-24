//! Names for the values a lifted function prints as `let vN` (unit
//! `native/lift-let-names`): a value read once prints inside its use
//! (native/lift-inline-temps); one read more often keeps a `let`, which
//! takes a name from what it holds where the text shows it:
//!
//! - what it loads: a global's (`let num_tasks: u32 = g.ram.gNumTasks;`,
//!   a leading `g` or `s` of a C global dropped), a typed field's
//!   (`let flags: u32 = g.get(Ref::<t::Task>::new(task).flags());`);
//! - the register it copies: a named local's (`let task_copy: u32 =
//!   task;`), or an `rN`'s DWARF variable where it holds rN at its block's
//!   start (`pc(0x…)`), which is where a register read's value comes from
//!   (not r0-r3 or r12 at a call's return site: the callee's);
//! - else what it feeds: a global or a typed field it is stored to, or the
//!   type a typed pointer is made of it (`Ref::<t::Task>::new(v4)`: `task`).
//!
//! A name that is only an address (`d_03000000`, `unk_03001234`) or a
//! register (a decompiler's variable `r4`) is none.
//! The rest stay `vN`. It is a pass over the function's printed text, run
//! last: each `let vN` is a binding resolved as Rust does (a brace group's
//! scope, a `let` in scope from its `;`), so the same `vN` in two blocks is
//! two values. A name is never a word the function's text already has (so
//! nothing it calls or reads is shadowed), nor a printer's numbered name,
//! nor a datum's, function's or value's identifier of the names map (a
//! `let` would match a const instead of binding). Two values take one name
//! only where no use of one could see the other; else the later is `name2`
//! (`unk_04_2` for a numbered name).

use crate::names::{Home, SourceMap};
use std::collections::{BTreeMap, BTreeSet};

/// What the pass may consult: the names map (DWARF locals, the items a
/// name must not be) and the function's key.
pub struct Ctx<'a> {
    pub map: Option<&'a SourceMap>,
    pub values: Option<&'a crate::types::Types>,
    pub entry: u32,
    /// The return sites of its calls (bit 0 clear): a block starting at
    /// one has the callee's r0-r3 and r12, whatever DWARF says.
    pub returns: BTreeSet<u32>,
}

/// One `let vN`.
#[derive(Debug)]
struct Binding {
    /// `vN` as printed, and where (bytes).
    word: String,
    at: (usize, usize),
    /// Its right side (bytes).
    rhs: (usize, usize),
    /// Its name's token; the token from which it is in scope (its `;`),
    /// and the token its brace group closes at.
    decl: usize,
    act: usize,
    end: usize,
    active: bool,
    /// Its uses: bytes and token index.
    uses: Vec<(usize, usize, usize)>,
    /// The block whose `pc(0x…)` it follows in its scope.
    pc: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    /// A word's bytes.
    Word(usize, usize),
    Punct(u8),
}

/// `text`'s tokens: words and punctuation, but strings, char literals,
/// labels and comments. `None` where it can't tell (an unclosed string).
fn tokens(text: &str) -> Option<Vec<(Tok, usize)>> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            if i >= b.len() {
                return None;
            }
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            i = text[i..].find('\n').map_or(b.len(), |n| i + n);
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = text[i + 2..].find("*/").map(|n| i + 2 + n + 2)?;
        } else if c == b'\'' {
            // A char literal (`'a'`, `'\n'`), else a label (`'b3`).
            if b.get(i + 1) == Some(&b'\\') {
                i += 2;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
                i += 1;
            } else if b.get(i + 2) == Some(&b'\'') {
                i += 3;
            } else {
                i += 1;
                while i < b.len() && word(b[i]) {
                    i += 1;
                }
            }
        } else if word(c) {
            let s = i;
            while i < b.len() && word(b[i]) {
                i += 1;
            }
            out.push((Tok::Word(s, i), s));
        } else {
            if !c.is_ascii_whitespace() {
                out.push((Tok::Punct(c), i));
            }
            i += 1;
        }
    }
    Some(out)
}

/// A printer's numbered value: `v12`.
fn value_word(w: &str) -> bool {
    w.strip_prefix('v')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// The `let vN` bindings of `text` with their uses. `None` where the text
/// binds a `vN` some other way (a `for`, a tuple's `let`) or its braces
/// don't balance: then nothing is named.
fn bindings(text: &str) -> Option<Vec<Binding>> {
    let toks = tokens(text)?;
    let w = |t: Tok| match t {
        Tok::Word(s, e) => Some(&text[s..e]),
        Tok::Punct(_) => None,
    };
    let mut out: Vec<Binding> = Vec::new();
    // Each open brace group: its bindings in scope, and its last `pc`.
    let mut frames: Vec<(Vec<usize>, Option<u32>)> = vec![(Vec::new(), None)];
    // `let`s not yet in scope: binding, brace depth.
    let mut pending: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let (t, _) = toks[i];
        match t {
            Tok::Punct(b'{') => frames.push((Vec::new(), None)),
            Tok::Punct(b'}') => {
                let (bs, _) = frames.pop()?;
                frames.first()?;
                for b in bs {
                    out[b].end = i;
                }
                let d = frames.len() - 1;
                if pending.iter().any(|&(_, pd)| pd > d) {
                    return None;
                }
            }
            Tok::Punct(b';') => {
                let d = frames.len() - 1;
                let (now, later): (Vec<_>, Vec<_>) = pending.iter().partition(|&&(_, pd)| pd == d);
                pending = later;
                for (b, _) in now {
                    out[b].act = i;
                    out[b].active = true;
                    frames[d].0.push(b);
                }
            }
            Tok::Word(s, e) => {
                let word = &text[s..e];
                let prev = i.checked_sub(1).map(|j| toks[j].0);
                let dot = matches!(prev, Some(Tok::Punct(b'.')))
                    && !(i >= 2 && matches!(toks[i - 2].0, Tok::Punct(b'.')));
                if word == "pc" || word == "set" {
                    // A block's marker: `pc(0x…)`, `guest_pc::set(0x…)`.
                    if let (Some((Tok::Punct(b'('), _)), Some(&(Tok::Word(hs, he), _))) =
                        (toks.get(i + 1), toks.get(i + 2))
                        && let Some(h) = text[hs..he].strip_prefix("0x")
                        && let Ok(pc) = u32::from_str_radix(h.trim_end_matches("u32"), 16)
                    {
                        frames.last_mut()?.1 = Some(pc);
                    }
                }
                if word == "let" {
                    let mut j = i + 1;
                    if toks.get(j).and_then(|t| w(t.0)) == Some("mut") {
                        j += 1;
                    }
                    match toks.get(j) {
                        Some(&(Tok::Word(vs, ve), _)) if value_word(&text[vs..ve]) => {
                            // Its right side: after the `=` to the `;` at
                            // its depth.
                            let mut k = j + 1;
                            while k < toks.len() && toks[k].0 != Tok::Punct(b'=') {
                                k += 1;
                            }
                            let start = toks.get(k + 1).map(|t| t.1)?;
                            let mut depth = 0i32;
                            let mut m = k + 1;
                            while m < toks.len() {
                                match toks[m].0 {
                                    Tok::Punct(b'{' | b'(' | b'[') => depth += 1,
                                    Tok::Punct(b'}' | b')' | b']') => depth -= 1,
                                    Tok::Punct(b';') if depth == 0 => break,
                                    _ => {}
                                }
                                m += 1;
                            }
                            let end = toks.get(m).map(|t| t.1)?;
                            let pc = frames.iter().rev().find_map(|f| f.1);
                            out.push(Binding {
                                word: text[vs..ve].to_string(),
                                at: (vs, ve),
                                rhs: (start, end),
                                decl: j,
                                act: 0,
                                end: toks.len(),
                                active: false,
                                uses: Vec::new(),
                                pc,
                            });
                            pending.push((out.len() - 1, frames.len() - 1));
                            i = j + 1;
                            continue;
                        }
                        Some(&(Tok::Punct(b'('), _)) => {
                            // A tuple: refuse one that binds a `vN`.
                            let mut k = j;
                            while k < toks.len() && toks[k].0 != Tok::Punct(b'=') {
                                if toks[k].0 != Tok::Punct(b'(')
                                    && w(toks[k].0).is_some_and(value_word)
                                {
                                    return None;
                                }
                                k += 1;
                            }
                        }
                        _ => {}
                    }
                } else if !dot && value_word(word) {
                    if matches!(prev, Some(Tok::Word(..)))
                        && i >= 1
                        && w(toks[i - 1].0) == Some("for")
                    {
                        return None;
                    }
                    if matches!(prev, Some(Tok::Punct(b'|'))) {
                        // A closure's parameter would bind it.
                        if matches!(toks.get(i + 1), Some((Tok::Punct(b'|' | b','), _)))
                            && (i < 2 || !matches!(toks[i - 2].0, Tok::Word(..)))
                        {
                            return None;
                        }
                    }
                    let found = frames
                        .iter()
                        .rev()
                        .find_map(|f| f.0.iter().rev().find(|&&b| out[b].word == word).copied());
                    if let Some(b) = found {
                        out[b].uses.push((s, e, i));
                    }
                }
            }
            Tok::Punct(_) => {}
        }
        i += 1;
    }
    (frames.len() == 1 && pending.is_empty()).then_some(out)
}

/// `name` in snake case: `structOffset` is `struct_offset`, `BGCnt`
/// `bg_cnt`; a C global's `g` or static's `s` prefix (`gNumTasks`)
/// dropped.
pub fn snake(name: &str) -> String {
    let b = name.as_bytes();
    let name = match b {
        [b'g' | b's', c, ..] if c.is_ascii_uppercase() => &name[1..],
        _ => name,
    };
    let cs: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in cs.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev = i.checked_sub(1).map(|j| cs[j]);
            let next = cs.get(i + 1);
            // Not after a digit: `unk1C` is `unk1c`, `Unk_0203AD10` `unk_0203ad10`.
            let lower_before = prev.is_some_and(|p| p.is_ascii_lowercase());
            let word_after = prev.is_some_and(|p| p.is_ascii_uppercase())
                && next.is_some_and(|n| n.is_ascii_lowercase());
            if (lower_before || word_after) && !out.ends_with('_') {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out.trim_matches('_').to_string()
}

/// The last identifier of a path of fields and indexes
/// (`gKirby.pos[2].x`: `x`), if `p` is only that.
fn path_last(p: &str) -> Option<&str> {
    let ok = p
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"_.[]".contains(&c));
    if !ok || !p.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    // Index expressions here are only literals: drop them.
    let mut last = None;
    for seg in p.split('.') {
        let id = seg.split('[').next()?;
        if id.is_empty() {
            return None;
        }
        last = Some(id);
    }
    last
}

/// Whether `s` is one call `head…)` whose parenthesis closes at its end.
fn one_call(s: &str, head: &str) -> bool {
    let Some(rest) = s.strip_prefix(head) else {
        return false;
    };
    let mut depth = 1i32;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i == rest.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

/// The field a typed access names last: `X.field()` at the end of `s`.
fn last_field(s: &str) -> Option<&str> {
    let s = s.strip_suffix("()")?;
    let d = s.rfind('.')?;
    let f = &s[d + 1..];
    (!f.is_empty() && f.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')).then_some(f)
}

/// What a value's right side holds, as a name's stem.
fn from_rhs(rhs: &str, b: &Binding, ctx: &Ctx) -> Option<String> {
    let mut e = rhs.trim();
    // A pinned global: `{ pin(…); g.ram.x }`.
    if let Some(inner) = e.strip_prefix("{ pin(").and_then(|r| r.strip_suffix(" }"))
        && let Some((_, last)) = inner.rsplit_once("; ")
    {
        e = last;
    }
    if let Some(p) = e.strip_prefix("g.ram.") {
        if let Some(l) = path_last(p) {
            return Some(snake(l));
        }
        // A bitfield's getter on a global: `g.ram.gX.field()`.
        if let Some(f) = last_field(p)
            && path_last(&p[..p.len() - f.len() - 3]).is_some()
        {
            return Some(snake(f));
        }
        return None;
    }
    if one_call(e, "g.get(") {
        return last_field(&e[6..e.len() - 1]).map(snake);
    }
    let reg = e
        .strip_prefix('r')
        .and_then(|d| d.parse::<u8>().ok())
        .filter(|_| e.len() <= 3);
    if let Some(r) = reg {
        let (map, pc) = (ctx.map?, b.pc? & !1);
        if (r < 4 || r == 12) && ctx.returns.contains(&pc) {
            return None;
        }
        let locals = map.locals.get(&ctx.entry)?;
        let here: BTreeSet<&str> = locals
            .iter()
            .filter(|l| l.home == Home::Reg(r) && l.from <= pc && pc < l.to)
            .map(|l| l.name.as_str())
            .collect();
        let [name] = here.into_iter().collect::<Vec<_>>()[..] else {
            return None;
        };
        return map.lift_local_ident(name).map(|n| snake(&n));
    }
    // A named local's copy.
    let local = !e.is_empty()
        && e.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && e.starts_with(|c: char| c.is_ascii_lowercase())
        && !super::local_names::printers(e)
        && !matches!(e, "nf" | "zf" | "cf" | "vf" | "true" | "false");
    local.then(|| format!("{}_copy", e.trim_end_matches('_')))
}

/// What a value's use shows it holds: the global or field it is stored
/// to, or the type of a pointer made of it.
fn from_use(text: &str, s: usize, e: usize) -> Option<String> {
    let ls = text[..s].rfind('\n').map_or(0, |n| n + 1);
    let le = text[e..].find('\n').map_or(text.len(), |n| e + n);
    let (before, after) = (&text[ls..s], &text[e..le]);
    if after.starts_with(")")
        && let Some(r) = before.strip_suffix(">::new(")
        && let Some(p) = r.rfind("Ref::<")
    {
        let ty = &r[p + 6..];
        let ty = ty.strip_prefix("t::").unwrap_or(ty);
        let plain = ty.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
        let prim = ["u8", "u16", "u32", "i8", "i16", "i32"].contains(&ty);
        if plain && !prim && ty.bytes().any(|c| c.is_ascii_lowercase()) {
            return Some(snake(ty));
        }
        return None;
    }
    if after.starts_with(';')
        && let Some(r) = before.strip_suffix(" = ")
        && let Some(p) = r.rfind("g.ram.")
        && (p == 0 || r.as_bytes()[p - 1] == b' ')
        && let Some(l) = path_last(&r[p + 6..])
    {
        return Some(snake(l));
    }
    if after.starts_with(')')
        && let Some(r) = before.strip_suffix(", ")
        && r.contains("g.set(")
        && let Some(f) = last_field(r)
    {
        return Some(snake(f));
    }
    None
}

/// A name that is only an address: an unnamed datum's (`d_03000000`) or
/// a map's placeholder (`unk_03001234`, of `gUnk_03001234`); or only a
/// register's (a decompiler's `r4` variable).
fn anonymous(stem: &str) -> bool {
    let hex = |h: &str, n: usize| h.len() >= n && h.bytes().all(|c| c.is_ascii_hexdigit());
    let bare = stem.trim_end_matches('_');
    stem.strip_prefix("d_").is_some_and(|h| hex(h, 7))
        || stem.strip_prefix("unk_").is_some_and(|h| hex(h, 6))
        // A DWARF variable named as a register (`r4`, `a1`).
        || super::local_names::printers(bare)
        || crate::names::printer_pattern(bare)
}

/// Whether a use of `a` could see `b` instead, were both one name; or
/// is in `b`'s right side (`let x = f(x)` reads as one value, and
/// [`super::unread`] reads it so).
fn clash(a: &Binding, b: &Binding) -> bool {
    let sees = |x: &Binding, y: &Binding| {
        x.act < y.act && x.uses.iter().any(|&(_, _, t)| y.decl < t && t < y.end)
    };
    sees(a, b) || sees(b, a)
}

/// `text` with its `let vN` values named, and how many were.
pub fn name(text: &str, ctx: &Ctx) -> (String, usize) {
    let Some(bs) = bindings(text) else {
        return (text.to_string(), 0);
    };
    if bs.is_empty() {
        return (text.to_string(), 0);
    }
    let words = super::local_names::words_of(text);
    let free = |id: &str| {
        !words.contains(id)
            && !super::local_names::printers(id)
            && !crate::names::printer_pattern(id)
            && syn::parse_str::<syn::Ident>(id).is_ok()
            && ctx
                .map
                .is_none_or(|m| m.lift_local_ident(id).as_deref() == Some(id))
            && ctx.values.is_none_or(|v| v.value_named(id).is_none())
    };
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut chosen: Vec<Option<String>> = vec![None; bs.len()];
    for (n, b) in bs.iter().enumerate() {
        if !b.active || b.uses.is_empty() {
            continue;
        }
        let stem = from_rhs(&text[b.rhs.0..b.rhs.1], b, ctx)
            .or_else(|| b.uses.iter().find_map(|&(s, e, _)| from_use(text, s, e)));
        let Some(stem) = stem.filter(|s| !s.is_empty() && !anonymous(s)) else {
            continue;
        };
        // `unk_04` numbers as `unk_04_2`; where a form is a printer's
        // (`a2`, `x_2`), the other.
        let digit = stem.ends_with(|c: char| c.is_ascii_digit());
        let ids = (1..100).flat_map(|k| match k {
            1 => vec![stem.clone()],
            _ if digit => vec![format!("{stem}_{k}"), format!("{stem}{k}")],
            _ => vec![format!("{stem}{k}"), format!("{stem}_{k}")],
        });
        for id in ids {
            if !free(&id) {
                continue;
            }
            let others = by_name.entry(id.clone()).or_default();
            if others.iter().any(|&o| clash(b, &bs[o])) {
                continue;
            }
            others.push(n);
            chosen[n] = Some(id);
            break;
        }
    }
    // Every rename, by byte position.
    let mut edits: Vec<(usize, usize, &str)> = Vec::new();
    for (b, id) in bs.iter().zip(&chosen) {
        let Some(id) = id else { continue };
        edits.push((b.at.0, b.at.1, id));
        edits.extend(b.uses.iter().map(|&(s, e, _)| (s, e, id.as_str())));
    }
    edits.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (s, e, id) in edits {
        out.push_str(&text[at..s]);
        out.push_str(id);
        at = e;
    }
    out.push_str(&text[at..]);
    (out, chosen.iter().flatten().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &str) -> String {
        name(
            t,
            &Ctx {
                map: None,
                values: None,
                entry: 0,
                returns: BTreeSet::new(),
            },
        )
        .0
    }

    #[test]
    fn snake_cases() {
        assert_eq!(snake("gNumTasks"), "num_tasks");
        assert_eq!(snake("structOffset"), "struct_offset");
        assert_eq!(snake("BGCnt"), "bg_cnt");
        assert_eq!(snake("Task"), "task");
        assert_eq!(snake("unk_04"), "unk_04");
        assert_eq!(snake("unk1C"), "unk1c");
        assert_eq!(snake("gUnk_0203AD10"), "unk_0203ad10");
        assert_eq!(snake("sKirby"), "kirby");
        assert_eq!(snake("speed"), "speed");
    }

    #[test]
    fn names_a_global_load_and_its_uses() {
        let t = "    let v74: u32 = g.ram.gNumTasks;\n    r2 = v74;\n    g.ram.gNumTasks = v74.wrapping_add(0x1);\n";
        let n = run(t);
        assert!(n.contains("let num_tasks: u32 = g.ram.gNumTasks;"), "{n}");
        assert!(n.contains("r2 = num_tasks;"), "{n}");
        assert!(
            n.contains("g.ram.gNumTasks = num_tasks.wrapping_add"),
            "{n}"
        );
    }

    #[test]
    fn names_a_field_load_and_a_pointer() {
        let t = "let v4: u32 = r4;\nlet v9: u32 = g.get(Ref::<t::Task>::new(v4).flags());\nr2 = v9; r3 = v9; r5 = v4;\n";
        let n = run(t);
        assert!(n.contains("let task: u32 = r4;"), "{n}");
        assert!(
            n.contains("let flags: u32 = g.get(Ref::<t::Task>::new(task).flags());"),
            "{n}"
        );
        assert!(n.contains("r2 = flags; r3 = flags; r5 = task;"), "{n}");
    }

    #[test]
    fn same_vn_in_two_blocks_is_two_values() {
        // The inner block's v4 shadows the outer's inside its braces only.
        let t = "let v4: u32 = g.ram.gA;\nif x {\n    let v4: u32 = g.ram.gB;\n    r0 = v4; r1 = v4;\n} else {\n    r0 = v4;\n}\nr2 = v4;\n";
        let n = run(t);
        assert!(n.contains("let a: u32 = g.ram.gA;"), "{n}");
        assert!(
            n.contains("let b: u32 = g.ram.gB;\n    r0 = b; r1 = b;"),
            "{n}"
        );
        assert!(n.contains("r0 = a;\n}\nr2 = a;"), "{n}");
    }

    #[test]
    fn a_later_same_name_that_would_shadow_takes_a_number() {
        // Two loads of one global, the first read after the second's let.
        let t = "let v1: u32 = g.ram.gAlpha;\nlet v2: u32 = g.ram.gAlpha;\nr0 = v1; r1 = v2; r2 = v1; r3 = v2;\n";
        let n = run(t);
        assert!(
            n.contains("let alpha: u32 = g.ram.gAlpha;\nlet alpha2: u32 = g.ram.gAlpha;"),
            "{n}"
        );
        assert!(
            n.contains("r0 = alpha; r1 = alpha2; r2 = alpha; r3 = alpha2;"),
            "{n}"
        );
        // A numbered stem takes `_2`; a printer's form is skipped.
        let n = run(&t.replace("gAlpha", "gUnk_04"));
        assert!(n.contains("let unk_04_2: u32"), "{n}");
        // An address is no name.
        let n = run(&t.replace("gAlpha", "gUnk_03001234"));
        assert!(n.contains("let v2: u32"), "{n}");
        let n = run(&t.replace("gAlpha", "gX"));
        assert!(n.contains("let x2: u32"), "{n}");
        // In sibling blocks they share it.
        let t = "if x {\n let v1: u32 = g.ram.gA; r0 = v1; r1 = v1;\n} else {\n let v1: u32 = g.ram.gA; r0 = v1; r1 = v1;\n}\n";
        let n = run(t);
        assert_eq!(n.matches("let a: u32").count(), 2, "{n}");
        // One read in the next one's right side: two names.
        let t = "let v1: u32 = g.ram.gAlpha;\nr0 = v1;\nlet v2: u32 = g.ram.gAlpha;\ng.ram.gB = v1;\nr1 = v2; r2 = v2;\n";
        let t = t.replace(
            "let v2: u32 = g.ram.gAlpha;\ng.ram.gB = v1;",
            "let v2: u32 = { pin(v1, 0); g.ram.gAlpha };",
        );
        let n = run(&t);
        assert!(
            n.contains("let alpha2: u32 = { pin(alpha, 0); g.ram.gAlpha };"),
            "{n}"
        );
    }

    #[test]
    fn a_name_the_text_has_is_not_taken() {
        // `task` is already a word (a register local): the copy is `task2`
        // where it feeds a pointer, `task_copy` where it copies the local.
        let t = "let v1: u32 = r4;\nlet v2: u32 = task;\nr0 = g.get(Ref::<t::Task>::new(v1).x()); r1 = v1; task = v2; r2 = v2;\n";
        let n = run(t);
        assert!(n.contains("let task2: u32 = r4;"), "{n}");
        assert!(n.contains("let task_copy: u32 = task;"), "{n}");
    }

    #[test]
    fn rhs_reads_the_outer_value_before_its_let_is_in_scope() {
        let t = "let v1: u32 = g.ram.gA;\nif c {\n    let v1: u32 = { v1 + 1 }; r0 = v1; r1 = v1;\n}\nr2 = v1;\n";
        let n = run(t);
        assert!(
            n.contains("let v1: u32 = { a + 1 }; r0 = v1; r1 = v1;"),
            "{n}"
        );
        assert!(n.contains("r2 = a;"), "{n}");
    }

    #[test]
    fn strings_comments_and_fields_are_left() {
        let t = "let v1: u32 = g.ram.gA; // v1 here\neprintln!(\"v1\"); x.v1 = v1; r0 = v1; 'v1: loop {}\n";
        let n = run(t);
        assert!(n.contains("// v1 here"), "{n}");
        assert!(n.contains("eprintln!(\"v1\")"), "{n}");
        assert!(n.contains("x.v1 = a; r0 = a;"), "{n}");
        assert!(n.contains("'v1: loop"), "{n}");
    }

    #[test]
    fn a_for_binding_a_value_word_names_nothing() {
        let t = "let v1: u32 = g.ram.gA; r0 = v1; for v1 in 0..3 { r1 = v1; }\n";
        assert_eq!(run(t), t);
    }

    #[test]
    fn a_value_with_nothing_to_go_by_stays() {
        let t = "let v1: u32 = r0.wrapping_add(4);\nr1 = v1; r2 = v1;\n";
        assert_eq!(run(t), t);
    }

    #[test]
    fn a_store_names_what_it_stores() {
        let t = "let v3: u32 = r0.wrapping_add(4);\ng.set(Ref::<t::Kirby>::new(r4).speedX(), v3);\nr1 = v3;\nlet v5: u32 = ld32(g, r2);\ng.ram.gFrame = v5;\nr3 = v5;\n";
        let n = run(t);
        assert!(n.contains("let speed_x: u32"), "{n}");
        assert!(n.contains("let frame: u32 = ld32"), "{n}");
    }

    #[test]
    fn a_register_read_takes_its_dwarf_variable_but_not_after_a_call() {
        let map = "[map]\nrom_sha1 = \"00\"\n[functions]\n\
                   0x08000100 = { name = \"walk\", size = 64 }\n[locals]\n0x08000100 = [\n\
                   { name = \"speed\", reg = 0, from = 0x08000100, to = 0x08000140 },\n\
                   { name = \"obj\", reg = 5, from = 0x08000100, to = 0x08000140 },\n]\n";
        let m = SourceMap::from_texts("00", &[map]).unwrap();
        let t = "pc(0x08000111);\nlet v1: u32 = r5;\nlet v2: u32 = r0;\nr1 = v1; r2 = v1; r3 = v2; r4 = v2;\n";
        let mut ctx = Ctx {
            map: Some(&m),
            values: None,
            entry: 0x0800_0100,
            returns: BTreeSet::new(),
        };
        let n = name(t, &ctx).0;
        assert!(
            n.contains("let obj: u32 = r5;\nlet speed: u32 = r0;"),
            "{n}"
        );
        // Block 0x08000110 is a call's return site: r0 is the callee's.
        ctx.returns.insert(0x0800_0110);
        let n = name(t, &ctx).0;
        assert!(n.contains("let obj: u32 = r5;\nlet v2: u32 = r0;"), "{n}");
        // Outside the variable's range: nothing.
        let n = name(&t.replace("0x08000111", "0x08000151"), &ctx).0;
        assert!(n.contains("let v1: u32 = r5;"), "{n}");
    }
}
