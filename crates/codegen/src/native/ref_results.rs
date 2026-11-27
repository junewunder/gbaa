//! Pointer results typed in their signatures (unit
//! `native/lift-pointer-signatures`): a function whose every `return`
//! gives a `Ref<T>` it holds (`return dst.addr();`) returns `Ref<T>`,
//! `return dst;`, and each call takes the `Ref`.
//!
//! A pass over the whole program's printed text after
//! [`super::ref_params`] (so a parameter it typed counts as held), while
//! every function's plain entry is still `f_<key>`. A function's result
//! is typed when its signature returns `u32` or `Result<u32, Exit>` and
//! every `return` line in its body is one of
//!
//! - `p.addr()` (or `Ok(p.addr())`), `p` a name its text declares only
//!   as `Ref<T>`, one `T` for the whole body, and not a register local
//!   (`r0`-`r15`: a register holds a pointer at one point and a count at
//!   the next, as list_length's `r0` does);
//! - a constant (a null), which becomes `Ref::new(c)`;
//!
//! with at least one of the first kind, and its body ends in a `return`,
//! `unreachable!()` or a labelled `loop` (no tail expression). Not a
//! stand-in's, a checked function's or a cut's callee (`skip`), nor a
//! function named anywhere but a call or its definition (a table, a
//! function pointer: their type is the word's).
//!
//! At each call: `let q: Ref<T> = Ref::new(f_<key>(…));` becomes
//! `let q: Ref<T> = f_<key>(…);`; a call standing as its own statement
//! is left as it is (its result unused); anywhere else the call takes
//! `.addr()`, the word the guest's r0 held (the adapters `x_` and `a_`
//! so). **No byte changes**: `Ref::new` and `addr` are the same word.

use std::collections::{BTreeMap, BTreeSet};

use super::ref_params::{declared_refs, definition, has_word, ident, is_word};

/// What [`lift`] did: results typed; calls that took the `Ref` whole,
/// and calls that took its `addr()`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Typed {
    pub results: usize,
    pub held: usize,
    pub words: usize,
}

/// A register local's name: `r0`-`r15`.
fn register(n: &str) -> bool {
    n.strip_prefix('r')
        .and_then(|d| d.parse::<u32>().ok())
        .is_some_and(|d| d < 16 && n.len() <= 3)
}

/// A constant the lift prints: `0x1fu32`, `0u32`, `7`.
fn constant(e: &str) -> bool {
    let d = e.strip_suffix("u32").unwrap_or(e);
    match d.strip_prefix("0x") {
        Some(h) => !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()),
        None => !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// One `return` line's value: `Some(Ok(name))` for `name.addr()`,
/// `Some(Err(c))` for a constant, `None` for anything else.
fn returned(l: &str, result: bool) -> Option<Result<&str, &str>> {
    let v = l.trim().strip_prefix("return ")?.strip_suffix(';')?;
    let v = match result {
        true => v.strip_prefix("Ok(")?.strip_suffix(')')?,
        false => v,
    };
    if let Some(n) = v.strip_suffix(".addr()") {
        return (ident(n) && !register(n)).then_some(Ok(n));
    }
    constant(v).then_some(Err(v))
}

/// A function this pass types: its text, the bytes of `u32` in its
/// signature, its `return` lines' edits, and `T`.
struct Plan {
    text: usize,
    ret: (usize, usize),
    lines: Vec<(usize, usize, String)>,
    ty: String,
}

/// The function `f_<k>` in `t` if its result is typed (see the module's
/// doc).
fn plan(t: &str, k: u32, text: usize) -> Option<Plan> {
    let (list, body) = definition(t, k)?;
    let eol = list.1 + t[list.1..].find('\n')?;
    let head = &t[list.1..eol];
    let (result, word) = if head.ends_with(") -> u32 {") {
        (false, "u32 {")
    } else if head.ends_with(") -> Result<u32, Exit> {") {
        (true, "u32, Exit> {")
    } else {
        return None;
    };
    let ret = (eol - word.len(), eol - word.len() + 3);
    let b = &t[body.0..body.1];
    // No tail expression: the last line returns, can't be reached, or
    // closes a labelled loop at the body's level.
    let last = b.lines().rev().find(|l| !l.trim().is_empty())?;
    let ends = last.trim_start().starts_with("return ")
        || last == "    unreachable!()"
        || (last == "    }"
            && b.lines()
                .rfind(|l| l.starts_with("    ") && !l.starts_with("     ") && l != &"    }")
                .is_some_and(|l| l.starts_with("    '") && l.ends_with(": loop {")));
    if !ends {
        return None;
    }
    // Its declarations: the parameter list and the body.
    let decls = format!("{}\n{b}", &t[list.0..list.1]);
    let mut ty: Option<String> = None;
    let mut lines = Vec::new();
    let mut at = body.0;
    let mut held = 0;
    for l in b.split_inclusive('\n') {
        if has_word(l, "return") {
            let v = returned(l, result)?;
            let line = match v {
                Ok(n) => {
                    let (refs, other) = declared_refs(&decls, n);
                    if other || refs.len() != 1 {
                        return None;
                    }
                    let r = refs.into_iter().next().unwrap();
                    if ty.as_ref().is_some_and(|t| *t != r) {
                        return None;
                    }
                    ty = Some(r);
                    held += 1;
                    l.replacen(&format!("{n}.addr()"), n, 1)
                }
                Err(c) => l.replacen(c, &format!("Ref::new({c})"), 1),
            };
            lines.push((at, at + l.len(), line));
        }
        at += l.len();
    }
    (held > 0).then_some(Plan {
        text,
        ret,
        lines,
        ty: ty?,
    })
}

/// Every function of `out` whose returns all give a `Ref<T>` it holds
/// made to return `Ref<T>`, and its calls taking it. Not those of `skip`
/// (stand-ins, checked functions, a cut's callees).
pub fn lift(out: &mut [(u32, String)], skip: &BTreeSet<u32>) -> Typed {
    let mut plans: BTreeMap<u32, Plan> = BTreeMap::new();
    for (ti, (k, t)) in out.iter().enumerate() {
        if skip.contains(k) {
            continue;
        }
        if let Some(p) = plan(t, *k, ti) {
            plans.insert(*k, p);
        }
    }
    if plans.is_empty() {
        return Typed::default();
    }
    // A function named anywhere but by a call or its definition keeps
    // its `u32`.
    for (_, t) in out.iter() {
        let b = t.as_bytes();
        plans.retain(|k, _| {
            let f = format!("f_{k:08x}");
            !t.match_indices(&f).any(|(i, _)| {
                (i == 0 || !is_word(b[i - 1]))
                    && b.get(i + f.len()).is_none_or(|&c| c != b'(' && !is_word(c))
            })
        });
    }
    let mut edits: BTreeMap<usize, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut typed = Typed::default();
    for p in plans.values() {
        typed.results += 1;
        let e = edits.entry(p.text).or_default();
        e.push((p.ret.0, p.ret.1, format!("Ref<{}>", p.ty)));
        e.extend(p.lines.iter().cloned());
    }
    for (ti, (_, t)) in out.iter().enumerate() {
        for (j, letter, k, close) in super::lifted_calls(t) {
            let Some(p) = plans.get(&k) else { continue };
            if letter != b'f' {
                continue;
            }
            // The call's end, past a `?`.
            let end = close + 1 + usize::from(t[close + 1..].starts_with('?'));
            let line_start = t[..j].rfind('\n').map_or(0, |i| i + 1);
            let before = &t[line_start..j];
            let after = &t[end..];
            let e = edits.entry(ti).or_default();
            let bind = format!(": Ref<{}> = Ref::new(", p.ty);
            if before.trim_start().starts_with("let ")
                && before.ends_with(&bind)
                && after.starts_with(");")
            {
                typed.held += 1;
                e.push((j - "Ref::new(".len(), j, String::new()));
                e.push((end, end + 1, String::new()));
            } else if (before.trim().is_empty() || before.ends_with("; ") || before.ends_with("{ "))
                && after.starts_with(';')
            {
                // Its own statement: the result goes unused.
            } else {
                typed.words += 1;
                e.push((end, end, ".addr()".into()));
            }
        }
    }
    for (ti, mut es) in edits {
        // From the end; an insertion at a replacement's end after it.
        es.sort_by_key(|e| std::cmp::Reverse((e.0, e.1)));
        let t = &mut out[ti].1;
        for (s, e, put) in es {
            t.replace_range(s..e, &put);
        }
    }
    typed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(texts: &[(u32, &str)]) -> (Vec<String>, Typed) {
        let mut out: Vec<(u32, String)> = texts.iter().map(|(k, t)| (*k, t.to_string())).collect();
        let n = lift(&mut out, &BTreeSet::new());
        (out.into_iter().map(|o| o.1).collect(), n)
    }

    const COPY: &str = "pub fn x_00000010(g: &mut Game, s: Regs) -> u32 { f_00000010(g, s.r[13], Ref::new(s.r[0])) }\npub fn f_00000010(g: &mut Game, _: u32, dst: Ref<u8>) -> u32 {\n    if g.get(dst) == 0 {\n        return 0x0u32;\n    }\n    return dst.addr();\n}\n";

    #[test]
    fn types_a_result_and_its_calls() {
        let caller = "pub fn f_00000020(g: &mut Game, r13: u32) -> () {\n    let mut r0 = 0u32;\n    let d: Ref<u8> = Ref::new(f_00000010(g, r13, Ref::new(4)));\n    r0 = f_00000010(g, r13, d);\n    f_00000010(g, r13, d);\n    let e: Ref<u16> = Ref::new(f_00000010(g, r13, d));\n    return ();\n}\n";
        let (o, n) = run(&[(0x10, COPY), (0x20, caller)]);
        assert_eq!(
            n,
            Typed {
                results: 1,
                held: 1,
                words: 3
            }
        );
        assert_eq!(
            o[0],
            "pub fn x_00000010(g: &mut Game, s: Regs) -> u32 { f_00000010(g, s.r[13], Ref::new(s.r[0])).addr() }\npub fn f_00000010(g: &mut Game, _: u32, dst: Ref<u8>) -> Ref<u8> {\n    if g.get(dst) == 0 {\n        return Ref::new(0x0u32);\n    }\n    return dst;\n}\n"
        );
        assert_eq!(
            o[1],
            "pub fn f_00000020(g: &mut Game, r13: u32) -> () {\n    let mut r0 = 0u32;\n    let d: Ref<u8> = f_00000010(g, r13, Ref::new(4));\n    r0 = f_00000010(g, r13, d).addr();\n    f_00000010(g, r13, d);\n    let e: Ref<u16> = Ref::new(f_00000010(g, r13, d).addr());\n    return ();\n}\n"
        );
    }

    #[test]
    fn a_result_in_ok_and_a_question_mark() {
        let callee = "pub fn f_00000010(g: &mut Game, _: u32) -> Result<u32, Exit> {\n    let mut data: Ref<t::Obj> = Ref::new(0x0u32);\n    'l1: loop {\n        if g.get(data.x()) == 0 {\n            return Ok(data.addr());\n        }\n        data = Ref::new(data.addr().wrapping_add(4));\n    }\n}\n";
        let caller = "pub fn f_00000020(g: &mut Game) -> Result<u32, Exit> {\n    let v: u32 = f_00000010(g, 0)?;\n    return Ok(v);\n}\n";
        let (o, n) = run(&[(0x10, callee), (0x20, caller)]);
        assert_eq!(n.results, 1);
        assert!(o[0].contains(") -> Result<Ref<t::Obj>, Exit> {\n"));
        assert!(o[0].contains("return Ok(data);"));
        assert!(o[1].contains("f_00000010(g, 0)?.addr();"));
    }

    #[test]
    fn a_register_another_value_or_a_tail_keeps_u32() {
        let reg = "pub fn f_00000010(g: &mut Game, _: u32, mut r0: Ref<t::List>) -> u32 {\n    return r0.addr();\n}\n";
        let two = "pub fn f_00000020(g: &mut Game, _: u32, a: Ref<u8>, b: u32) -> u32 {\n    if b == 0 {\n        return b;\n    }\n    return a.addr();\n}\n";
        let tail = "pub fn f_00000030(g: &mut Game, _: u32, a: Ref<u8>, b: u32) -> u32 {\n    if b == 0 {\n        return a.addr();\n    }\n    b\n}\n";
        let mixed = "pub fn f_00000040(g: &mut Game, _: u32, a: Ref<u8>, b: Ref<u16>) -> u32 {\n    if g.get(b) == 0 {\n        return b.addr();\n    }\n    return a.addr();\n}\n";
        let named = "pub fn f_00000050(g: &mut Game, _: u32, a: Ref<u8>) -> u32 {\n    return a.addr();\n}\npub static T: [Fn; 1] = [f_00000050];\n";
        let texts = [
            (0x10, reg),
            (0x20, two),
            (0x30, tail),
            (0x40, mixed),
            (0x50, named),
        ];
        let (o, n) = run(&texts);
        assert_eq!(n, Typed::default());
        for (a, (_, b)) in o.iter().zip(texts) {
            assert_eq!(a, b);
        }
    }
}
