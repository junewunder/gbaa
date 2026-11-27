//! Pointer parameters typed in their signatures (unit
//! `native/lift-pointer-signatures`): a parameter [`super::ref_binds`]
//! bound again as its body's line `let n: Ref<t::Node> = Ref::new(n);`
//! takes the type in the signature, `n: Ref<t::Node>`, and the line goes.
//!
//! A pass over the whole program's printed text, after its entries are
//! trimmed ([`super::one_entry`]) and before the names pass, so every
//! function's plain entry is still `f_<key>` and each call of it is
//! `f_<key>(…)` ([`super::lifted_calls`]). At each call the argument
//! becomes the `Ref` the callee takes:
//!
//! - `p.addr()`, where every `Ref` the caller declares `p` is the type
//!   the parameter takes: `p`, the pointer the caller holds (a parameter
//!   this pass types too counts as its bound type);
//! - else `Ref::new(e)`, made once at the call (the adapters `x_` and
//!   `a_` so: `Ref::new(s.r[0])`), which is the word the guest passed.
//!
//! **A type changes no byte passed**: `Ref::new` and `addr` are the same
//! word. Where the callers all hold the argument as one other type, a
//! register local's parameter (a `let mut` line: its register holds the
//! body's type later, as list_insert's `r1` is `n`, then `link`) takes
//! theirs, and its line converts it, `let mut r1: Ref<Ref<t::Node>> =
//! Ref::new(r1.addr());`. A parameter stays as it is (bound again in its
//! body) where callers hold it as `Ref`s of two other types, or of one
//! other where it isn't a register local (its callers disagree), where
//! anything names `f_<key>` other than a call or its definition (a
//! table, a function pointer), where a call's arguments don't parse, for
//! a stand-in's and a checked function's (their text is the holes
//! file's or the check's) and for a function a mod's cut calls (its text
//! is the mod's). A parameter printed as `Lent` was never `u32`.
//!
//! [`rebound`] counts the parameters still bound again in their bodies
//! (`params_rebound`, barred).

use std::collections::{BTreeMap, BTreeSet};

/// A parameter typed here: its function's key, its place in the
/// parameter list (`g` is 0), its name and `mut`, its type, the bytes of
/// its signature item, and the line binding it again.
#[derive(Clone, Debug)]
struct Param {
    key: u32,
    at: usize,
    name: String,
    mutable: bool,
    ty: String,
    item: (usize, usize),
    line: (usize, usize),
    /// Its text's place in `out`.
    text: usize,
}

/// `[from, to)` of `t` split at its top-level commas: each piece's bytes,
/// as written (spaces kept).
fn split_top(t: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut d = 0i32;
    let mut s = from;
    for (i, c) in t[from..to].bytes().enumerate() {
        match c {
            b'(' | b'[' | b'{' => d += 1,
            b')' | b']' | b'}' => d -= 1,
            b',' if d == 0 => {
                out.push((s, from + i));
                s = from + i + 1;
            }
            _ => {}
        }
    }
    // Not after a trailing comma.
    if !t[s..to].trim().is_empty() {
        out.push((s, to));
    }
    out
}

/// The byte of the `)` closing the `(` at `open`.
fn close_paren(t: &str, open: usize) -> Option<usize> {
    let mut d = 0usize;
    for (i, c) in t.bytes().enumerate().skip(open) {
        match c {
            b'(' => d += 1,
            b')' => {
                d -= 1;
                if d == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn ident(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(is_word) && !s.as_bytes()[0].is_ascii_digit()
}

/// Whether `w` is a whole word anywhere in `t`.
fn has_word(t: &str, w: &str) -> bool {
    let b = t.as_bytes();
    t.match_indices(w).any(|(i, _)| {
        (i == 0 || !is_word(b[i - 1])) && b.get(i + w.len()).is_none_or(|&c| !is_word(c))
    })
}

/// A parameter item `name: u32` or `mut name: u32`: its name and `mut`.
fn u32_param(item: &str) -> Option<(&str, bool)> {
    let (pat, ty) = item.split_once(':')?;
    if ty.trim() != "u32" {
        return None;
    }
    let pat = pat.trim();
    let (m, n) = match pat.strip_prefix("mut ") {
        Some(n) => (true, n.trim()),
        None => (false, pat),
    };
    ident(n).then_some((n, m))
}

/// A body line `let [mut ]n: Ref<T> = Ref::new(n);`: `n`, `mut`, `T`.
fn rebind_line(l: &str) -> Option<(&str, bool, &str)> {
    let l = l.trim();
    let rest = l.strip_prefix("let ")?;
    let (m, rest) = match rest.strip_prefix("mut ") {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let (n, rest) = rest.split_once(": Ref<")?;
    let (ty, rest) = rest.rsplit_once("> = Ref::new(")?;
    let arg = rest.strip_suffix(");")?;
    (ident(n) && arg == n).then_some((n, m, ty))
}

/// The function `f_<key>`'s definition in `t`: the bytes of its
/// parameter list (inside the parens) and of its body (after the `{`
/// line, to its closing `}` line).
fn definition(t: &str, k: u32) -> Option<((usize, usize), (usize, usize))> {
    let head = format!("pub fn f_{k:08x}(");
    let at = t
        .match_indices(&head)
        .map(|(i, _)| i)
        .find(|&i| i == 0 || t.as_bytes()[i - 1] == b'\n')?;
    let open = at + head.len() - 1;
    let close = close_paren(t, open)?;
    let eol = close + t[close..].find('\n')?;
    if !t[close..eol].trim_end().ends_with('{') {
        return None;
    }
    let body = eol + 1;
    let mut end = body;
    for l in t[body..].split_inclusive('\n') {
        if l.trim_end() == "}" {
            return Some(((open + 1, close), (body, end)));
        }
        end += l.len();
    }
    None
}

/// The `Ref` types `t` declares `name` with (`name: Ref<T>` as a
/// parameter or a `let`'s), and whether it declares it anything else
/// typed.
fn declared_refs(t: &str, name: &str) -> (BTreeSet<String>, bool) {
    let b = t.as_bytes();
    let pat = format!("{name}: ");
    let mut refs = BTreeSet::new();
    let mut other = false;
    for (i, _) in t.match_indices(&pat) {
        if i > 0 && is_word(b[i - 1]) {
            continue;
        }
        let rest = &t[i + pat.len()..];
        match rest.strip_prefix("Ref<") {
            Some(r) => {
                // To the `>` closing it.
                let mut d = 1;
                let end = r.bytes().position(|c| {
                    match c {
                        b'<' => d += 1,
                        b'>' => d -= 1,
                        _ => {}
                    }
                    d == 0
                });
                match end {
                    Some(e) => {
                        refs.insert(r[..e].to_string());
                    }
                    None => other = true,
                }
            }
            None => other = true,
        }
    }
    (refs, other)
}

/// What [`lift`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Typed {
    /// Parameters now typed in their signatures.
    pub params: usize,
    /// Arguments passed as the `Ref` the caller holds, and made at the
    /// call.
    pub held: usize,
    pub made: usize,
}

/// Every parameter of `out`'s functions bound again as its body's `Ref`
/// line typed so in its signature, and each call passing it a `Ref`. Not
/// those of `skip` (stand-ins, checked functions, a cut's callees).
pub fn lift(out: &mut [(u32, String)], skip: &BTreeSet<u32>) -> Typed {
    let mut params: Vec<Param> = Vec::new();
    for (ti, (k, t)) in out.iter().enumerate() {
        if skip.contains(k) {
            continue;
        }
        let Some((list, body)) = definition(t, *k) else {
            continue;
        };
        let items = split_top(t, list.0, list.1);
        // Each `u32` parameter by name.
        let mut by_name: BTreeMap<&str, (usize, bool, (usize, usize))> = BTreeMap::new();
        for (i, &(s, e)) in items.iter().enumerate() {
            if let Some((n, m)) = u32_param(&t[s..e]) {
                by_name.insert(n, (i, m, (s, e)));
            }
        }
        let mut at = body.0;
        for l in t[body.0..body.1].split_inclusive('\n') {
            if let Some((n, m, ty)) = rebind_line(l)
                && let Some(&(i, _, item)) = by_name.get(n)
                && !has_word(&t[body.0..at], n)
                && l.starts_with("    let ")
            {
                params.push(Param {
                    key: *k,
                    at: i,
                    name: n.to_string(),
                    mutable: m,
                    ty: ty.to_string(),
                    item,
                    line: (at, at + l.len()),
                    text: ti,
                });
            }
            at += l.len();
        }
    }
    if params.is_empty() {
        return Typed::default();
    }
    // A function named anywhere but by a call or its definition keeps
    // its parameters.
    let keys: BTreeSet<u32> = params.iter().map(|p| p.key).collect();
    let mut dropped: BTreeSet<u32> = BTreeSet::new();
    for (_, t) in out.iter() {
        let b = t.as_bytes();
        for &k in &keys {
            let f = format!("f_{k:08x}");
            for (i, _) in t.match_indices(&f) {
                if i > 0 && is_word(b[i - 1]) {
                    continue;
                }
                let next = b.get(i + f.len()).copied();
                if next.is_some_and(is_word) {
                    continue;
                }
                if next != Some(b'(') {
                    dropped.insert(k);
                }
            }
        }
    }
    // Each call's arguments: (text, key, argument bytes).
    type Call = (usize, u32, Vec<(usize, usize)>);
    let mut calls: Vec<Call> = Vec::new();
    for (ti, (_, t)) in out.iter().enumerate() {
        for (j, letter, k, close) in super::lifted_calls(t) {
            if letter != b'f' || !keys.contains(&k) {
                continue;
            }
            let args = split_top(t, j + 11, close);
            calls.push((ti, k, args));
        }
    }
    // The parameters each function's own text types, by name: what its
    // name is past its line.
    let typed_in = |ti: usize, name: &str| -> Option<String> {
        params
            .iter()
            .find(|p| p.text == ti && p.name == name)
            .map(|p| p.ty.clone())
    };
    // Each argument, per parameter, with the one `Ref` type the caller
    // holds it as, if it does. A parameter with a call that doesn't
    // parse keeps its line.
    type Arg = (usize, usize, usize, Option<String>);
    let mut plans: Vec<Option<Vec<Arg>>> = Vec::new();
    for p in &params {
        if dropped.contains(&p.key) {
            plans.push(None);
            continue;
        }
        let want = definition(&out[p.text].1, p.key)
            .map(|(l, _)| split_top(&out[p.text].1, l.0, l.1).len());
        let mut plan = Some(Vec::new());
        for (ti, k, args) in &calls {
            if *k != p.key {
                continue;
            }
            let t = &out[*ti].1;
            if want != Some(args.len()) {
                plan = None;
                break;
            }
            let (s, e) = args[p.at];
            let a = t[s..e].trim();
            let s = s + (t[s..e].len() - t[s..e].trim_start().len());
            let e = s + a.len();
            let held = a
                .strip_suffix(".addr()")
                .filter(|h| ident(h))
                .and_then(|h| {
                    let (mut refs, _) = declared_refs(t, h);
                    if let Some(ty) = typed_in(*ti, h) {
                        refs.insert(ty);
                    }
                    // A name declared `Ref`s of two types: made at the call.
                    (refs.len() == 1).then(|| refs.pop_first()).flatten()
                });
            if let Some(v) = plan.as_mut() {
                v.push((*ti, s, e, held));
            }
        }
        plans.push(plan);
    }
    // A held `p.addr()` passes `p`: a parameter of the caller's that
    // keeps its line is still that line's `Ref` at the call (the line
    // binds it before any use).
    let mut edits: BTreeMap<usize, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut typed = Typed::default();
    for (p, plan) in params.iter().zip(&plans) {
        let Some(plan) = plan else { continue };
        // The type the callers hold it as: the body's, else (callers
        // that agree on another, a register local's parameter, whose
        // register holds the body's type later: list_insert's `n`, then
        // `link`) theirs, the body's line converting it.
        let others: BTreeSet<&String> = plan
            .iter()
            .filter_map(|a| a.3.as_ref())
            .filter(|t| **t != p.ty)
            .collect();
        let sig = match others.len() {
            0 => &p.ty,
            1 if p.mutable => others.first().copied().unwrap(),
            _ => continue,
        };
        typed.params += 1;
        // Its line converting it binds it `mut` again.
        let m = if p.mutable && *sig == p.ty {
            "mut "
        } else {
            ""
        };
        let e = edits.entry(p.text).or_default();
        e.push((p.item.0, p.item.1, format!(" {m}{}: Ref<{sig}>", p.name)));
        let line = match *sig == p.ty {
            true => String::new(),
            false => {
                let l = &out[p.text].1[p.line.0..p.line.1];
                l.replacen(
                    &format!("Ref::new({});", p.name),
                    &format!("Ref::new({}.addr());", p.name),
                    1,
                )
            }
        };
        e.push((p.line.0, p.line.1, line));
        for &(ti, s, en, ref held) in plan {
            let t = &out[ti].1;
            let e = edits.entry(ti).or_default();
            if held.as_ref() == Some(sig) {
                typed.held += 1;
                let h = t[s..en].strip_suffix(".addr()").unwrap_or(&t[s..en]);
                e.push((s, en, h.to_string()));
            } else {
                typed.made += 1;
                e.push((s, s, "Ref::new(".into()));
                e.push((en, en, ")".into()));
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

/// The parameters bound again in their function's body: a `u32`
/// parameter `n` and a line `let n: Ref<T> = Ref::new(n);` in its body
/// (`params_rebound`). Over printed text, named or not.
pub fn rebound(t: &str) -> u64 {
    let mut n = 0;
    let mut params: Vec<&str> = Vec::new();
    for l in t.lines() {
        let fun = l.strip_prefix("pub fn ").or_else(|| l.strip_prefix("fn "));
        if let Some(rest) = fun {
            params.clear();
            if let Some(open) = rest.find('(')
                && let Some(close) = close_paren(rest, open)
            {
                for (s, e) in split_top(rest, open + 1, close) {
                    if let Some((p, _)) = u32_param(&rest[s..e]) {
                        params.push(p);
                    }
                }
            }
            continue;
        }
        if l == "}" {
            params.clear();
            continue;
        }
        if let Some((p, _, _)) = rebind_line(l)
            && params.contains(&p)
        {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(texts: &[(u32, &str)]) -> (Vec<String>, Typed) {
        let mut out: Vec<(u32, String)> = texts.iter().map(|(k, t)| (*k, t.to_string())).collect();
        let n = lift(&mut out, &BTreeSet::new());
        (out.into_iter().map(|o| o.1).collect(), n)
    }

    const CALLEE: &str = "pub fn x_00000010(g: &mut Game, s: Regs) -> () { f_00000010(g, s.r[13], s.r[0], s.r[1]) }\npub fn f_00000010(g: &mut Game, _: u32, b: u32, i: u32) -> () {\n    let b: Ref<t::Ball> = Ref::new(b);\n    g.set_bits(b.left(), i);\n    return ();\n}\n";

    #[test]
    fn types_a_parameter_and_its_calls() {
        let caller = "pub fn f_00000020(g: &mut Game, r13: u32, b: u32) -> () {\n    let b: Ref<t::Ball> = Ref::new(b);\n    let v: u32 = 3;\n    f_00000010(g, r13, b.addr(), v);\n    f_00000010(g, r13, v, v);\n    return ();\n}\n";
        let (o, n) = run(&[(0x10, CALLEE), (0x20, caller)]);
        assert_eq!(
            n,
            Typed {
                params: 2,
                held: 1,
                made: 2
            }
        );
        assert_eq!(
            o[0],
            "pub fn x_00000010(g: &mut Game, s: Regs) -> () { f_00000010(g, s.r[13], Ref::new(s.r[0]), s.r[1]) }\npub fn f_00000010(g: &mut Game, _: u32, b: Ref<t::Ball>, i: u32) -> () {\n    g.set_bits(b.left(), i);\n    return ();\n}\n"
        );
        assert_eq!(
            o[1],
            "pub fn f_00000020(g: &mut Game, r13: u32, b: Ref<t::Ball>) -> () {\n    let v: u32 = 3;\n    f_00000010(g, r13, b, v);\n    f_00000010(g, r13, Ref::new(v), v);\n    return ();\n}\n"
        );
        assert_eq!(rebound(&o[0]) + rebound(&o[1]), 0);
        assert_eq!(rebound(CALLEE) + rebound(caller), 2);
    }

    #[test]
    fn a_caller_with_another_type_keeps_the_line() {
        let caller = "pub fn f_00000020(g: &mut Game, r13: u32, b: u32) -> () {\n    let b: Ref<t::Obj> = Ref::new(b);\n    f_00000010(g, r13, b.addr(), 0);\n    return ();\n}\n";
        let (o, n) = run(&[(0x10, CALLEE), (0x20, caller)]);
        // The caller's own parameter, which nothing calls, is typed.
        assert_eq!(
            n,
            Typed {
                params: 1,
                held: 0,
                made: 0
            }
        );
        assert_eq!(o[0], CALLEE);
        assert_eq!(
            o[1],
            "pub fn f_00000020(g: &mut Game, r13: u32, b: Ref<t::Obj>) -> () {\n    f_00000010(g, r13, b.addr(), 0);\n    return ();\n}\n"
        );
    }

    #[test]
    fn a_mut_line_and_a_use_before_it() {
        let callee = "pub fn f_00000010(g: &mut Game, _: u32, mut r0: u32, r1: u32) -> () {\n    let (mut r4, mut r12) = (0u32, 0u32);\n    let mut r1: Ref<Ref<t::Node>> = Ref::new(r1);\n    let e: u32 = r0;\n    let mut r0: Ref<u16> = Ref::new(r0);\n    g.set(r0, 1);\n    r1 = Ref::new(4);\n}\n";
        let (o, n) = run(&[
            (0x10, callee),
            (
                0x20,
                "pub fn f_00000020(g: &mut Game) -> () {\n    f_00000010(g, 0, 1, f_00000030(g));\n}\n",
            ),
        ]);
        assert_eq!(n.params, 1);
        assert_eq!(
            o[0],
            "pub fn f_00000010(g: &mut Game, _: u32, mut r0: u32, mut r1: Ref<Ref<t::Node>>) -> () {\n    let (mut r4, mut r12) = (0u32, 0u32);\n    let e: u32 = r0;\n    let mut r0: Ref<u16> = Ref::new(r0);\n    g.set(r0, 1);\n    r1 = Ref::new(4);\n}\n"
        );
        assert_eq!(
            o[1],
            "pub fn f_00000020(g: &mut Game) -> () {\n    f_00000010(g, 0, 1, Ref::new(f_00000030(g)));\n}\n"
        );
    }

    #[test]
    fn a_register_parameter_takes_its_callers_type() {
        let callee = "pub fn f_00000010(g: &mut Game, _: u32, r1: u32) -> () {\n    let mut r1: Ref<Ref<t::Node>> = Ref::new(r1);\n    let n: Ref<t::Node> = Ref::new(r1.addr());\n}\n";
        let caller = "pub fn f_00000020(g: &mut Game) -> () {\n    let node: Ref<t::Node> = Ref::new(4);\n    f_00000010(g, 0, node.addr());\n}\n";
        let (o, n) = run(&[(0x10, callee), (0x20, caller)]);
        assert_eq!(
            n,
            Typed {
                params: 1,
                held: 1,
                made: 0
            }
        );
        assert_eq!(
            o[0],
            "pub fn f_00000010(g: &mut Game, _: u32, r1: Ref<t::Node>) -> () {\n    let mut r1: Ref<Ref<t::Node>> = Ref::new(r1.addr());\n    let n: Ref<t::Node> = Ref::new(r1.addr());\n}\n"
        );
        assert_eq!(
            o[1],
            "pub fn f_00000020(g: &mut Game) -> () {\n    let node: Ref<t::Node> = Ref::new(4);\n    f_00000010(g, 0, node);\n}\n"
        );
        assert_eq!(rebound(&o[0]), 0);
    }

    #[test]
    fn a_function_named_but_not_called_keeps_its_lines() {
        let table = "pub static T: [Fn; 1] = [f_00000010];\n";
        let (o, n) = run(&[(0x10, CALLEE), (0x20, table)]);
        assert_eq!(n.params, 0);
        assert_eq!(o[0], CALLEE);
    }
}
