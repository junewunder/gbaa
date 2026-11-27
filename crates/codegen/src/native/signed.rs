//! Values compared signed declared `i32` (unit `native/lift-signed-values`;
//! `docs/research/mirror-machine-count.md`, row 8). Stage A computes in
//! `u32`, so a value C keeps in an `int` prints `(v as i32)` at each
//! signed compare. Where declaring it `i32` saves more casts than it adds,
//! this pass does: a pass over a function's printed text, after the names
//! and the `Ref` bindings ([`super::ref_binds`]), like those.
//!
//! The variables are the `let w: u32 = e;` of the text (not `mut`: a value
//! the lift computes once). Declared `i32`, each of its uses prints as:
//!
//! - `w as i32` (in parentheses or not): `w`, a cast fewer;
//! - `w.wrapping_add(k) as i32` (or `_sub`, `k` a literal): the sum in
//!   `i32`, `w.wrapping_add(k')` (`k'` the same bits as an `i32` literal),
//!   a cast fewer;
//! - `w as T`, `T` `u8`, `i8`, `u16`, `i16` or `u32`: as it is (the
//!   cast keeps the same bits of an `i32` as of a `u32`);
//! - `w == k` or `w != k` (`k` a literal, the compare's whole operand):
//!   `k` as an `i32` literal (`v1 == -1`);
//! - a copy into another such `let` (`let x: u32 = w;`): `w` where both are
//!   `i32`, else the cast between them;
//! - anything else: `(w as u32)`, a cast more (`w as u32` where nothing
//!   binds tighter around it: an argument, a right side).
//!
//! A variable shifted right (`w >> 3`) or compared unsigned (`w < v2`,
//! `w >= 0x10u32`) stays `u32` whatever it would save: it is a `u32` by
//! what it does.
//!
//! Its right side `e` becomes `e as i32`, which costs nothing where `e`
//! ends in a cast at least 32 bits wide (`x as i16 as u32` is `x as i16 as
//! i32`, [`casts::cast`]) or is a literal, saves one where `e` is an `i32`
//! made `u32` (`((x as i32) >> 16) as u32` is `(x as i32) >> 16`), and
//! adds one otherwise. Which variables are `i32` is the choice of least
//! casts over the function: costs on each variable, and one on each copy
//! between two whose types differ, a minimum cut.
//!
//! **Each rewrite keeps every bit**: an `i32` and a `u32` of the same bits
//! convert to each other by `as`, wrapping sums are the same bits in
//! either, and each use reads the type it read before.

use super::casts;
use super::value_names::{Tok, tokens};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// What a use of a variable costs once it is `i32`, and its text then.
#[derive(Debug)]
enum Use {
    /// `w as i32` (the bytes it spans, parentheses included): `w`.
    Signed(usize, usize),
    /// `w.wrapping_op(k) as i32` (its bytes): the sum in `i32`.
    Sum(usize, usize, String),
    /// A cast that reads the same bits of either type: as it is.
    Same,
    /// `w == k` or `w != k` (`k` a literal: its bytes): `k` as an `i32`.
    Eq(usize, usize, String),
    /// The whole right side of another variable (its index).
    Copy(usize),
    /// Anything else: the word's bytes and whether it needs parentheses.
    Other(usize, usize, bool),
}

/// One `let w: u32 = e;`.
#[derive(Debug)]
struct Var {
    name: String,
    /// The `u32` type's bytes.
    ty: (usize, usize),
    /// Its right side's bytes.
    rhs: (usize, usize),
    uses: Vec<Use>,
}

/// The right side as `i32`, and the casts that adds (-1 to 1).
fn rhs_i32(e: &str) -> (String, i32) {
    let t = e.trim();
    if let Some(k) = int_lit(t) {
        return (lit_i32(k), 0);
    }
    if let Some((a, ty)) = casts::top_cast(t)
        && matches!(ty, "u32" | "i32" | "u64" | "i64")
    {
        if casts::is_i32(a) {
            return (casts::unwrap(a).to_string(), -1);
        }
        return (casts::cast(t, "i32"), 0);
    }
    if casts::is_i32(t) {
        return (t.to_string(), 0);
    }
    (format!("{} as i32", operand(t)), 1)
}

/// `e` as an operand of `as`: in parentheses unless it is one term.
fn operand(e: &str) -> String {
    let one = match syn::parse_str::<syn::Expr>(e) {
        Ok(x) => matches!(
            x,
            syn::Expr::Path(_)
                | syn::Expr::Call(_)
                | syn::Expr::MethodCall(_)
                | syn::Expr::Field(_)
                | syn::Expr::Index(_)
                | syn::Expr::Paren(_)
                | syn::Expr::Lit(_)
                | syn::Expr::Macro(_)
        ),
        Err(_) => false,
    };
    if one { e.to_string() } else { format!("({e})") }
}

/// An integer literal's value (`0x10`, `16`, `0x10u32`).
fn int_lit(t: &str) -> Option<u32> {
    let t = t.strip_suffix("u32").unwrap_or(t);
    match t.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => t.parse().ok(),
    }
}

/// The `i32` literal of `k`'s bits.
fn lit_i32(k: u32) -> String {
    match k as i32 {
        i32::MIN => "i32::MIN".to_string(),
        v => casts::literal(v as i64),
    }
}

const SAME: [&str; 5] = ["u8", "i8", "u16", "i16", "u32"];

/// `text` (one function's printed text) with its variables that save
/// casts as `i32` declared so, and how many there are. `None` where it
/// can't tell how the text binds its names.
pub fn sign(text: &str) -> Option<(String, usize)> {
    let toks = tokens(text)?;
    let word = |i: usize| match toks.get(i) {
        Some(&(Tok::Word(s, e), _)) => Some(&text[s..e]),
        _ => None,
    };
    let punct = |i: usize, c: u8| matches!(toks.get(i), Some(&(Tok::Punct(p), _)) if p == c);
    let start = |i: usize| toks.get(i).map_or(text.len(), |t| t.1);
    let end = |i: usize| match toks.get(i) {
        Some(&(Tok::Word(_, e), _)) => e,
        Some(&(_, s)) => s + 1,
        None => text.len(),
    };
    // Names bound some way this pass doesn't follow: left alone.
    let mut poisoned: BTreeSet<&str> = BTreeSet::new();
    for i in 0..toks.len() {
        if let Some(w) = word(i)
            && ((i > 0 && word(i - 1) == Some("for"))
                || (i > 0 && punct(i - 1, b'|') && (punct(i + 1, b'|') || punct(i + 1, b','))))
        {
            poisoned.insert(w);
        }
    }
    let mut vars: Vec<Var> = Vec::new();
    // Each open brace group's bindings (`None`: a name bound otherwise,
    // which hides an outer variable of that name).
    let mut frames: Vec<Vec<(String, Option<usize>)>> = vec![Vec::new()];
    let mut pending: Vec<(String, Option<usize>, usize)> = Vec::new();
    // Each use: the variable and the word's token.
    let mut uses: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match toks[i].0 {
            Tok::Punct(b'{') => frames.push(Vec::new()),
            Tok::Punct(b'}') => {
                frames.pop()?;
                frames.first()?;
                let d = frames.len() - 1;
                if pending.iter().any(|p| p.2 > d) {
                    return None;
                }
            }
            Tok::Punct(b';') => {
                let d = frames.len() - 1;
                let (now, later): (Vec<_>, Vec<_>) = pending.drain(..).partition(|p| p.2 == d);
                pending = later;
                for (n, v, _) in now {
                    frames[d].push((n, v));
                }
            }
            Tok::Word(..) if word(i) == Some("let") => {
                let mut j = i + 1;
                if word(j) == Some("mut") {
                    j += 1;
                }
                let d = frames.len() - 1;
                if punct(j, b'(') || punct(j, b'[') {
                    // A tuple or an array: each name in it hides an outer one.
                    let mut k = j;
                    while k < toks.len() && !punct(k, b'=') {
                        if let Some(w) = word(k).filter(|w| *w != "mut") {
                            pending.push((w.to_string(), None, d));
                        }
                        k += 1;
                    }
                    i = k;
                    continue;
                }
                let name = word(j)?;
                // Its right side: after the `=` to the `;` at its depth.
                let mut k = j + 1;
                while k < toks.len() && !punct(k, b'=') {
                    k += 1;
                }
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
                let plain = word(j - 1) == Some("let")
                    && punct(j + 1, b':')
                    && word(j + 2) == Some("u32")
                    && k == j + 3
                    && !poisoned.contains(name);
                let v = plain.then(|| {
                    vars.push(Var {
                        name: name.to_string(),
                        ty: (start(j + 2), end(j + 2)),
                        rhs: (start(k + 1), start(m)),
                        uses: Vec::new(),
                    });
                    vars.len() - 1
                });
                pending.push((name.to_string(), v, d));
                // The right side reads names bound before it.
                i = k + 1;
                continue;
            }
            Tok::Word(..) => {
                let w = word(i)?;
                let dot = i > 0 && punct(i - 1, b'.') && !(i > 1 && punct(i - 2, b'.'));
                let path = i > 0 && punct(i - 1, b':');
                // A field of a struct literal: `Regs { r0: x }`.
                let field = punct(i + 1, b':')
                    && !punct(i + 2, b':')
                    && i > 0
                    && (punct(i - 1, b'{') || punct(i - 1, b','));
                if !dot && !path && !field {
                    let found = frames
                        .iter()
                        .rev()
                        .find_map(|f| f.iter().rev().find(|(n, _)| n == w).map(|b| b.1));
                    if let Some(Some(v)) = found {
                        uses.push((v, i));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    if !pending.is_empty() {
        return None;
    }
    // Each use's form.
    let rhs_of: BTreeMap<(usize, usize), usize> =
        vars.iter().enumerate().map(|(n, v)| (v.rhs, n)).collect();
    // A compare's operand is the whole of what is next to it: nothing
    // binds tighter on either side (`a & w == 0` compares `a & w`).
    let whole_left = |i: usize| {
        i > 1
            && (punct(i - 1, b'(')
                || punct(i - 1, b',')
                || punct(i - 1, b'{')
                || (punct(i - 1, b'&') && punct(i - 2, b'&'))
                || (punct(i - 1, b'|') && punct(i - 2, b'|'))
                || (punct(i - 1, b'=') && !punct(i - 2, b'=') && !punct(i - 2, b'!')))
    };
    let whole_right = |i: usize| {
        punct(i, b')')
            || punct(i, b';')
            || punct(i, b',')
            || punct(i, b'{')
            || punct(i, b'}')
            || (punct(i, b'&') && punct(i + 1, b'&'))
            || (punct(i, b'|') && punct(i + 1, b'|'))
    };
    // A use shifted right (`w >> 3`) or an operand of `<`, `>`, `<=` or
    // `>=` without its `as i32`: a `u32` value by what it does, never typed
    // `i32` (`<<`, `->` and `=>` are not compares).
    let unsigned = |i: usize| {
        let after = punct(i + 1, b'>') || (punct(i + 1, b'<') && !punct(i + 2, b'<'));
        let before = i > 1
            && ((punct(i - 1, b'<') && !punct(i - 2, b'<'))
                || (punct(i - 1, b'>')
                    && !punct(i - 2, b'>')
                    && !punct(i - 2, b'-')
                    && !punct(i - 2, b'='))
                || (punct(i - 1, b'=')
                    && (punct(i - 2, b'<') || punct(i - 2, b'>'))
                    && !(i > 2 && (punct(i - 3, b'<') || punct(i - 3, b'>')))));
        after || before
    };
    let mut pinned = vec![false; vars.len()];
    for &(v, i) in &uses {
        let (s, e) = (start(i), end(i));
        let unary = i > 0
            && (punct(i - 1, b'!')
                || (punct(i - 1, b'&')
                    && !(i > 1 && matches!(toks[i - 2].0, Tok::Word(..)))
                    && !(i > 1 && (punct(i - 2, b')') || punct(i - 2, b']')))));
        // Parentheses that are only grouping: not a call's or an index's.
        let grouped = |a: usize, b: usize| {
            a > 0
                && punct(a, b'(')
                && punct(b, b')')
                && !(matches!(toks[a - 1].0, Tok::Word(..))
                    || punct(a - 1, b')')
                    || punct(a - 1, b']')
                    || punct(a - 1, b'>')
                    || punct(a - 1, b'!'))
        };
        let u = if unary || i == 0 {
            Use::Other(s, e, true)
        } else if word(i + 1) == Some("as") && word(i + 2) == Some("i32") {
            match grouped(i - 1, i + 3) {
                true => Use::Signed(start(i - 1), end(i + 3)),
                false => Use::Signed(s, end(i + 2)),
            }
        } else if word(i + 1) == Some("as") && word(i + 2).is_some_and(|t| SAME.contains(&t)) {
            Use::Same
        } else if (punct(i + 1, b'=') || punct(i + 1, b'!'))
            && punct(i + 2, b'=')
            && let Some(k) = word(i + 3).and_then(int_lit)
            && whole_left(i)
            && whole_right(i + 4)
        {
            Use::Eq(start(i + 3), end(i + 3), lit_i32(k))
        } else if punct(i + 1, b'.')
            && let Some(op @ ("wrapping_add" | "wrapping_sub")) = word(i + 2)
            && punct(i + 3, b'(')
            && let Some(k) = word(i + 4).and_then(int_lit)
            && punct(i + 5, b')')
            && word(i + 6) == Some("as")
            && word(i + 7) == Some("i32")
        {
            let sum = format!("{}.{op}({})", vars[v].name, lit_i32(k));
            match grouped(i - 1, i + 8) {
                true => Use::Sum(start(i - 1), end(i + 8), sum),
                false => Use::Sum(s, end(i + 7), sum),
            }
        } else if let Some(&c) = rhs_of.get(&(s, start(i + 1)))
            && text[s..vars[c].rhs.1].trim() == vars[v].name
        {
            Use::Copy(c)
        } else {
            // `w as u32` binds tighter than anything around it but a
            // postfix or `<` (read as a type's generics).
            let bare = i > 1
                && (punct(i - 1, b'(')
                    || punct(i - 1, b',')
                    || punct(i - 1, b'{')
                    || punct(i - 1, b'=') && !punct(i - 2, b'=') && !punct(i - 2, b'!'))
                && (punct(i + 1, b',')
                    || punct(i + 1, b')')
                    || punct(i + 1, b';')
                    || punct(i + 1, b'}'));
            // Shifted right or compared unsigned: it stays `u32`.
            if unsigned(i) {
                pinned[v] = true;
            }
            Use::Other(s, e, !bare)
        };
        vars[v].uses.push(u);
    }
    // What each variable saves (negative) or costs as `i32`, and the copies.
    let n = vars.len();
    let mut cost = vec![0i32; n];
    let mut copies: Vec<(usize, usize)> = Vec::new();
    let mut copied = vec![false; n];
    for (v, var) in vars.iter().enumerate() {
        for u in &var.uses {
            match u {
                Use::Signed(..) | Use::Sum(..) => cost[v] -= 1,
                Use::Same | Use::Eq(..) => {}
                Use::Copy(c) => {
                    copies.push((v, *c));
                    copied[*c] = true;
                }
                Use::Other(..) => cost[v] += 1,
            }
        }
    }
    for (v, var) in vars.iter().enumerate() {
        if !copied[v] {
            cost[v] += rhs_i32(&text[var.rhs.0..var.rhs.1]).1;
        }
        if pinned[v] {
            // More than any saving in a function: on the sink side.
            cost[v] = 1 << 24;
        }
    }
    let signed = min_cut(&cost, &copies);
    if signed.is_empty() {
        return Some((text.to_string(), 0));
    }
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for &v in &signed {
        let var = &vars[v];
        edits.push((var.ty.0, var.ty.1, "i32".to_string()));
        let e = &text[var.rhs.0..var.rhs.1];
        let from = copies.iter().find(|c| c.1 == v).map(|c| c.0);
        match from {
            Some(f) if signed.contains(&f) => {}
            // Its source stays `u32`: that use is the cast.
            Some(_) => edits.push((var.rhs.0, var.rhs.1, format!("{} as i32", e.trim()))),
            None => edits.push((var.rhs.0, var.rhs.1, rhs_i32(e).0)),
        }
        for u in &var.uses {
            match u {
                Use::Signed(s, e) => edits.push((*s, *e, var.name.clone())),
                Use::Sum(s, e, t) => edits.push((*s, *e, t.clone())),
                Use::Same => {}
                Use::Eq(s, e, k) => edits.push((*s, *e, k.clone())),
                // Its copy takes it as `i32`; one left `u32` casts it.
                Use::Copy(c) if signed.contains(c) => {}
                Use::Copy(c) => {
                    let r = vars[*c].rhs;
                    edits.push((r.0, r.1, format!("{} as u32", var.name)));
                }
                Use::Other(s, e, true) => edits.push((*s, *e, format!("({} as u32)", var.name))),
                Use::Other(s, e, false) => edits.push((*s, *e, format!("{} as u32", var.name))),
            }
        }
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    // Two edits of one span (a right side both copied and rewritten): the
    // text can't say which; leave the function as it is.
    if edits.windows(2).any(|w| w[1].1 > w[0].0) {
        return Some((text.to_string(), 0));
    }
    let mut out = text.to_string();
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    Some((out, signed.len()))
}

/// The variables to make `i32`: those on the source side of a minimum
/// cut, where a variable's `cost` is paid on the source side (its saving
/// on the sink side) and each copy costs 1 across it.
fn min_cut(cost: &[i32], copies: &[(usize, usize)]) -> BTreeSet<usize> {
    let n = cost.len();
    let (s, t) = (n, n + 1);
    let mut cap: Vec<BTreeMap<usize, i32>> = vec![BTreeMap::new(); n + 2];
    let add = |a: usize, b: usize, c: i32, cap: &mut Vec<BTreeMap<usize, i32>>| {
        *cap[a].entry(b).or_insert(0) += c;
        cap[b].entry(a).or_insert(0);
    };
    for (v, &c) in cost.iter().enumerate() {
        if c < 0 {
            add(s, v, -c, &mut cap);
        } else if c > 0 {
            add(v, t, c, &mut cap);
        }
    }
    for &(a, b) in copies {
        add(a, b, 1, &mut cap);
        add(b, a, 1, &mut cap);
    }
    // Augmenting paths, shortest first.
    loop {
        let mut prev = vec![usize::MAX; n + 2];
        prev[s] = s;
        let mut q = VecDeque::from([s]);
        while let Some(a) = q.pop_front() {
            for (&b, &c) in &cap[a] {
                if c > 0 && prev[b] == usize::MAX {
                    prev[b] = a;
                    q.push_back(b);
                }
            }
        }
        if prev[t] == usize::MAX {
            // The source side: what the residual reaches from it.
            return (0..n).filter(|&v| prev[v] != usize::MAX).collect();
        }
        let mut f = i32::MAX;
        let mut b = t;
        while b != s {
            let a = prev[b];
            f = f.min(cap[a][&b]);
            b = a;
        }
        let mut b = t;
        while b != s {
            let a = prev[b];
            *cap[a].get_mut(&b).unwrap() -= f;
            *cap[b].get_mut(&a).unwrap() += f;
            b = a;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: &str) -> String {
        sign(t).unwrap().0
    }

    #[test]
    fn a_value_compared_signed() {
        assert_eq!(
            s(
                "fn f() {\n    let v1: u32 = g.ram.x as i16 as u32;\n    let v2: bool = (v1 as i32) < 0;\n    let v3: bool = (v1 as i32) > 9;\n}\n"
            ),
            "fn f() {\n    let v1: i32 = g.ram.x as i16 as i32;\n    let v2: bool = v1 < 0;\n    let v3: bool = v1 > 9;\n}\n"
        );
    }

    #[test]
    fn other_uses_cast_back() {
        // One signed use, a cast-free def: worth it even with a narrow store.
        assert_eq!(
            s(
                "fn f() {\n    let v1: u32 = ld16(g, a) as i16 as u32;\n    g.ram.y = v1 as u16;\n    let c: bool = (v1 as i32) <= 3;\n}\n"
            ),
            "fn f() {\n    let v1: i32 = ld16(g, a) as i16 as i32;\n    g.ram.y = v1 as u16;\n    let c: bool = v1 <= 3;\n}\n"
        );
        // Two other uses outweigh one signed.
        let t = "fn f() {\n    let v1: u32 = r0;\n    h(g, v1);\n    let c: bool = (v1 as i32) <= 3;\n}\n";
        assert_eq!(s(t), t);
        // A tie stays `u32`.
        let t = "fn f() {\n    let v1: u32 = ld16(g, a) as i16 as u32;\n    h(g, v1);\n    let c: bool = (v1 as i32) <= 3;\n}\n";
        assert_eq!(s(t), t);
        assert_eq!(
            s(
                "fn f() {\n    let v1: u32 = ld16(g, a) as i16 as u32;\n    h(g, v1);\n    let c: bool = (v1 as i32) <= 3;\n    let d: bool = (v1 as i32) > 7;\n}\n"
            ),
            "fn f() {\n    let v1: i32 = ld16(g, a) as i16 as i32;\n    h(g, v1 as u32);\n    let c: bool = v1 <= 3;\n    let d: bool = v1 > 7;\n}\n"
        );
    }

    #[test]
    fn unsigned_uses_stay_u32() {
        // Two signed compares and a cast-free right side, but shifted right.
        let t = "fn f() {\n    let v1: u32 = ld16(g, a) as i16 as u32;\n    let c: bool = (v1 as i32) < 0;\n    let d: bool = (v1 as i32) > 9;\n    let e: u32 = v1 >> 4;\n}\n";
        assert_eq!(s(t), t);
        // Compared unsigned, on either side of each compare.
        for cmp in ["v1 < v2", "v2 > v1", "v1 >= 0x10u32", "v2 <= v1"] {
            let t = format!(
                "fn f() {{\n    let v2: u32 = r1;\n    let v1: u32 = ld16(g, a) as i16 as u32;\n    let c: bool = (v1 as i32) < 0;\n    let d: bool = (v1 as i32) > 9;\n    let e: bool = {cmp};\n}}\n"
            );
            assert_eq!(s(&t), t, "{cmp}");
        }
        // A shift left, a shift amount and a match arm are not.
        assert_eq!(
            s(
                "fn f() {\n    let v1: u32 = ld16(g, a) as i16 as u32;\n    let c: bool = (v1 as i32) < 0;\n    let d: bool = (v1 as i32) > 9;\n    let e: u32 = v1 << 4;\n}\n"
            ),
            "fn f() {\n    let v1: i32 = ld16(g, a) as i16 as i32;\n    let c: bool = v1 < 0;\n    let d: bool = v1 > 9;\n    let e: u32 = (v1 as u32) << 4;\n}\n"
        );
    }

    #[test]
    fn equalities() {
        assert_eq!(
            s(
                "fn f() {\n    let v1: u32 = ld16(g, a) as i16 as u32;\n    let c: bool = v1 == 0xffffffffu32 && (v1 as i32) < 9;\n    let d: bool = (q & v1 == 0x0u32);\n    let e: bool = (v1 as i32) > 2;\n}\n"
            ),
            "fn f() {\n    let v1: i32 = ld16(g, a) as i16 as i32;\n    let c: bool = v1 == -1 && v1 < 9;\n    let d: bool = (q & (v1 as u32) == 0x0u32);\n    let e: bool = v1 > 2;\n}\n"
        );
    }

    #[test]
    fn sums_and_shifts() {
        assert_eq!(
            s(
                "fn f() {\n    let v1: u32 = ((r0 as i32) >> 16) as u32;\n    let c: bool = (v1.wrapping_sub(0x1) as i32) < 0;\n}\n"
            ),
            "fn f() {\n    let v1: i32 = (r0 as i32) >> 16;\n    let c: bool = v1.wrapping_sub(1) < 0;\n}\n"
        );
        assert_eq!(
            s(
                "fn f() {\n    let v1: u32 = ((r0 as i32) >> 16) as u32;\n    let c: bool = (v1.wrapping_add(0xffffff00u32) as i32) < 0;\n}\n"
            ),
            "fn f() {\n    let v1: i32 = (r0 as i32) >> 16;\n    let c: bool = v1.wrapping_add(-0x100) < 0;\n}\n"
        );
    }

    #[test]
    fn copies_go_together() {
        assert_eq!(
            s(
                "fn f() {\n    let a: u32 = ld16(g, x) as i16 as u32;\n    let b: u32 = a;\n    let c: bool = (b as i32) < 0;\n}\n"
            ),
            "fn f() {\n    let a: i32 = ld16(g, x) as i16 as i32;\n    let b: i32 = a;\n    let c: bool = b < 0;\n}\n"
        );
    }

    #[test]
    fn scopes() {
        // The inner `v1` is another value; a `mut` one is never typed.
        let t = "fn f() {\n    let v1: u32 = r0;\n    {\n        let v1: u32 = ld16(g, x) as i16 as u32;\n        let c: bool = (v1 as i32) < 0;\n    }\n    h(g, v1);\n    let mut r4: u32 = 0;\n    let d: bool = (r4 as i32) < 0;\n}\n";
        assert_eq!(
            s(t),
            "fn f() {\n    let v1: u32 = r0;\n    {\n        let v1: i32 = ld16(g, x) as i16 as i32;\n        let c: bool = v1 < 0;\n    }\n    h(g, v1);\n    let mut r4: u32 = 0;\n    let d: bool = (r4 as i32) < 0;\n}\n"
        );
    }

    #[test]
    fn identities() {
        for v in [0u32, 1, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 0x1234_8765] {
            let i = v as i32;
            for k in [1u32, 0xffff_ff00, 0x8000_0000] {
                assert_eq!(v.wrapping_add(k) as i32, i.wrapping_add(k as i32));
                assert_eq!(v.wrapping_sub(k) as i32, i.wrapping_sub(k as i32));
            }
            for t in [
                i as u8 == v as u8,
                i as i8 == v as i8,
                i as u16 == v as u16,
                i as i16 == v as i16,
                i as u32 == v,
            ] {
                assert!(t);
            }
        }
    }
}
