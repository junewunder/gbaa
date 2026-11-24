//! Pointer variables declared as what they point at (unit
//! `native/lift-pointer-types`, its first part): a parameter or `let`
//! the printer gives `u32` whose typed accesses all make one type of it
//! (`g.get(Ref::<t::Node>::new(n).x())`) is declared `Ref<t::Node>`, so
//! each access is `g.get(n.x())`, with no `Ref::new` at the use.
//!
//! It is a pass over a function's printed text, after the names
//! ([`super::value_names`]), like that one:
//!
//! - a `let w: u32 = e;` becomes `let w: Ref<T> = Ref::new(e);`, or
//!   `let w: Ref<T> = v;` where `e` is a variable bound here to the same
//!   type (`let n_copy: Ref<t::Node> = n;`);
//! - a parameter `w: u32` keeps its type in the signature (callers and
//!   adapters pass the guest's word) and is bound again as the body's
//!   first line, `let w: Ref<T> = Ref::new(w);`;
//! - each `Ref::<T>::new(w)` prints `w`, and every other use `w.addr()`
//!   (C's pointer as an integer: a compare, arithmetic, an argument).
//!
//! Only a variable declared once in its function (no shadowing, no
//! pattern, not `mut`) and made a `Ref` of one type at least once is
//! bound; a `Ref::<T>::to(w, k)` (an object `w` is a field of) is an
//! other use. **A type changes no byte read**: `Ref::new` and `addr` are
//! the same word, so the accesses are the ones the untyped text made.
//!
//! What it counts, apart by where the type came from: `named` (a type of
//! the names files, `t::…`; [`super::fields`]) and `proven` (a heap run's
//! type, [`super::heap`]). [`at_use`] counts the `Ref::<T>::new(…)` and
//! `Ref::<T>::to(…)` left at a use (`ref_new_at_use`, barred).

use std::collections::BTreeMap;

use super::value_names::{Tok, tokens};

/// The variables bound, by where their type came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bound {
    pub named: usize,
    pub proven: usize,
}

/// `Ref::<T>::new(` and `Ref::<T>::to(` in `text`: a typed pointer made
/// at its use.
pub fn at_use(text: &str) -> u64 {
    let mut n = 0;
    let mut from = 0;
    while let Some(p) = text[from..].find("Ref::<") {
        let open = from + p + "Ref::<".len();
        let b = text.as_bytes();
        let mut depth = 1;
        let mut j = open;
        while j < b.len() && depth > 0 {
            match b[j] {
                b'<' => depth += 1,
                b'>' => depth -= 1,
                b'\n' => break,
                _ => {}
            }
            j += 1;
        }
        if depth == 0 && (text[j..].starts_with("::new(") || text[j..].starts_with("::to(")) {
            n += 1;
        }
        from = open;
    }
    n
}

/// A use of a variable: its token, and the bytes of the
/// `Ref::<T>::new(w)` around it if it is made a `Ref` there.
type Use = (usize, Option<(usize, usize)>);

/// One variable that may be bound.
struct Cand {
    name: String,
    /// Its name's token at its declaration.
    decl: usize,
    /// A `let`'s `u32` token and its right side (tokens, the `;` last).
    lett: Option<(usize, usize, usize)>,
}

/// `text` (one or more functions) with its pointer variables bound.
pub fn bind(text: &str) -> (String, Bound) {
    let mut bound = Bound::default();
    let Some(toks) = tokens(text) else {
        return (text.to_string(), bound);
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    while i + 2 < toks.len() {
        if word(text, &toks, i) == Some("fn")
            && word(text, &toks, i + 1).is_some()
            && punct(&toks, i + 2) == Some(b'(')
            && let Some(next) = one_fn(text, &toks, i + 2, &mut edits, &mut bound)
        {
            i = next;
            continue;
        }
        i += 1;
    }
    if edits.is_empty() {
        return (text.to_string(), bound);
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(text.len() + edits.len() * 8);
    let mut at = 0;
    for (s, e, t) in edits {
        debug_assert!(s >= at, "overlapping edits");
        out.push_str(&text[at..s]);
        out.push_str(&t);
        at = e;
    }
    out.push_str(&text[at..]);
    (out, bound)
}

fn word<'a>(text: &'a str, toks: &[(Tok, usize)], i: usize) -> Option<&'a str> {
    match toks.get(i)?.0 {
        Tok::Word(s, e) => Some(&text[s..e]),
        Tok::Punct(_) => None,
    }
}

fn punct(toks: &[(Tok, usize)], i: usize) -> Option<u8> {
    match toks.get(i)?.0 {
        Tok::Punct(c) => Some(c),
        Tok::Word(..) => None,
    }
}

/// The token closing the group `open` opens (`(`, `[` or `{`).
fn close_of(toks: &[(Tok, usize)], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (j, t) in toks.iter().enumerate().skip(open) {
        match t.0 {
            Tok::Punct(b'(' | b'[' | b'{') => depth += 1,
            Tok::Punct(b')' | b']' | b'}') => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

/// The function whose parameters open at `lp`: its edits. The token
/// after its body, or `None` where it has none (a declaration).
fn one_fn(
    text: &str,
    toks: &[(Tok, usize)],
    lp: usize,
    edits: &mut Vec<(usize, usize, String)>,
    bound: &mut Bound,
) -> Option<usize> {
    let rp = close_of(toks, lp)?;
    // Its body: the first `{` past the return type (`[u32; 4]`'s `;` is
    // inside brackets), else it is a declaration.
    let mut open = rp + 1;
    let mut d = 0;
    loop {
        match (toks.get(open)?.0, punct(toks, open)) {
            (_, Some(b'[' | b'(')) => d += 1,
            (_, Some(b']' | b')')) => d -= 1,
            (_, Some(b'{')) if d == 0 => break,
            (_, Some(b';')) if d == 0 => return None,
            _ => {}
        }
        open += 1;
    }
    let close = close_of(toks, open)?;
    let w = |i| word(text, toks, i);
    let p = |i| punct(toks, i);
    let mut cands: Vec<Cand> = Vec::new();
    // Parameters: `w: u32`, each between commas at the top.
    let mut start = lp + 1;
    let mut depth = 0;
    for j in lp + 1..=rp {
        match p(j) {
            Some(b'(' | b'[' | b'<') => depth += 1,
            Some(b')' | b']' | b'>') if j < rp => depth -= 1,
            _ => {}
        }
        if (p(j) == Some(b',') && depth == 0) || j == rp {
            if j == start + 3
                && let Some(name) = w(start)
                && p(start + 1) == Some(b':')
                && w(start + 2) == Some("u32")
                && name != "_"
            {
                cands.push(Cand {
                    name: name.to_string(),
                    decl: start,
                    lett: None,
                });
            }
            start = j + 1;
        }
    }
    // Every name a `let`, `for` or parameter pattern declares (its
    // token), and the plain `let w: u32 = e;`s.
    let mut decls: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut depth = 0;
    for j in lp + 1..rp {
        match p(j) {
            Some(b'(' | b'[' | b'<') => depth += 1,
            Some(b')' | b']' | b'>') => depth -= 1,
            _ => {}
        }
        if let Some(n) = w(j)
            && depth == 0
            && p(j + 1) == Some(b':')
        {
            decls.entry(n).or_default().push(j);
        }
    }
    // The brace group each token is in: its closing token.
    let mut groups: Vec<usize> = Vec::new();
    let mut lets: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    let mut j = open;
    while j < close {
        match p(j) {
            Some(b'{') => groups.push(close_of(toks, j)?),
            Some(b'}') => {
                groups.pop();
            }
            _ => {}
        }
        let kw = w(j);
        if j > open && matches!(kw, Some("let" | "for")) {
            // The pattern: to `=`, `:` or `in` at its top.
            let mut k = j + 1;
            let mut d = 0;
            while k < close {
                match p(k) {
                    Some(b'(' | b'[') => d += 1,
                    Some(b')' | b']') => d -= 1,
                    Some(b'=' | b':' | b';' | b'{') if d == 0 => break,
                    _ => {}
                }
                if d == 0 && w(k) == Some("in") {
                    break;
                }
                if let Some(n) = w(k)
                    && n != "mut"
                {
                    decls.entry(n).or_default().push(k);
                }
                k += 1;
            }
            // A `let` of one name: its `;` (from which it is in scope)
            // and its group's end.
            let one = match (w(j + 1), w(j + 2)) {
                (Some("mut"), Some(_)) if k == j + 3 => Some(j + 2),
                (Some(n), _) if k == j + 2 && n != "mut" => Some(j + 1),
                _ => None,
            };
            if kw == Some("let")
                && let Some(at) = one
            {
                let mut e = k;
                let mut d = 0;
                while e < close {
                    match p(e) {
                        Some(b'(' | b'[' | b'{') => d += 1,
                        Some(b')' | b']' | b'}') => d -= 1,
                        Some(b';') if d == 0 => break,
                        _ => {}
                    }
                    e += 1;
                }
                lets.insert(at, (e, *groups.last()?));
                if at == j + 1
                    && p(k) == Some(b':')
                    && w(k + 1) == Some("u32")
                    && p(k + 2) == Some(b'=')
                    && p(k + 3) != Some(b'=')
                    && e < close
                    && e > k + 3
                {
                    cands.push(Cand {
                        name: w(at)?.to_string(),
                        decl: at,
                        lett: Some((k + 1, k + 3, e)),
                    });
                }
            }
            j = k;
            continue;
        }
        // A name an arm's pattern binds (`x @ …`) or a closure's: two, so
        // it is never bound.
        if let Some(n) = w(j)
            && (p(j + 1) == Some(b'@') || p(j.wrapping_sub(1)) == Some(b'|'))
        {
            decls.entry(n).or_default().extend([j, j]);
        }
        j += 1;
    }
    let body = &text[toks[lp].1..toks[close].1];
    // Each candidate's uses: (token, the `Ref::<T>::new(w)` around it).
    let mut types: Vec<Option<String>> = vec![None; cands.len()];
    let mut owner: BTreeMap<usize, usize> = BTreeMap::new();
    let mut plans: Vec<(usize, Vec<Use>)> = Vec::new();
    for (ci, c) in cands.iter().enumerate() {
        if body.contains(&format!("{{{}}}", c.name)) || body.contains(&format!("{{{}:", c.name)) {
            continue;
        }
        // Where its uses are: a parameter's, the body; a `let`'s, from
        // its `;` to its group's end. Up to a later `let` of its name in
        // that group (its right side still this one's), and but the scope
        // of one in a group inside; any other binding of its name there,
        // and it isn't bound.
        let (from, mut to) = match c.lett {
            Some(l) => (l.2, lets[&c.decl].1),
            None => (open, close),
        };
        let mine = decls.get(c.name.as_str()).map_or(&[][..], |v| &v[..]);
        let mut holes: Vec<(usize, usize)> = Vec::new();
        let mut shadowed = false;
        for &d in mine {
            if d == c.decl || d < from || d >= to {
                continue;
            }
            match lets.get(&d) {
                Some(&(semi, end)) if end == to => to = semi,
                Some(&(semi, end)) => holes.push((semi, end)),
                None => shadowed = true,
            }
        }
        if shadowed {
            continue;
        }
        let mut uses = Vec::new();
        let mut ty: Option<String> = None;
        let mut ok = true;
        for k in from..to {
            if w(k) != Some(c.name.as_str())
                || mine.contains(&k)
                || holes.iter().any(|h| k > h.0 && k < h.1)
            {
                continue;
            }
            let path_before = p(k - 1) == Some(b':') && p(k - 2) == Some(b':');
            let field = p(k - 1) == Some(b'.');
            let after_colon = p(k + 1) == Some(b':');
            let mac = p(k + 1) == Some(b'!') && matches!(p(k + 2), Some(b'(' | b'[' | b'{'));
            if field || path_before || after_colon || mac {
                if after_colon && p(k + 2) != Some(b':') {
                    // `w: …` in the body: a struct literal's field, or a
                    // binding this doesn't follow.
                    ok = false;
                }
                continue;
            }
            match made_ref(text, toks, k) {
                Some((s, e, t)) => {
                    if ty.as_ref().is_some_and(|x| *x != t) {
                        ok = false;
                        break;
                    }
                    ty = Some(t);
                    uses.push((k, Some((s, e))));
                }
                None => uses.push((k, None)),
            }
        }
        if let (true, Some(t)) = (ok, ty) {
            types[ci] = Some(t);
            owner.extend(uses.iter().map(|u| (u.0, ci)));
            plans.push((ci, uses));
        }
    }
    // A `let` whose right side is one variable bound to its type.
    let copy_of = |ci: usize| -> Option<usize> {
        let (_, s, e) = cands[ci].lett?;
        let o = *owner.get(&s)?;
        (e == s + 1 && types[o] == types[ci]).then_some(s)
    };
    let copies: Vec<usize> = plans.iter().filter_map(|&(ci, _)| copy_of(ci)).collect();
    for (ci, uses) in &plans {
        let c = &cands[*ci];
        let t = types[*ci].as_deref().unwrap_or_default();
        if t.starts_with("t::") || t.starts_with("[t::") {
            bound.named += 1;
        } else {
            bound.proven += 1;
        }
        for &(k, around) in uses {
            match around {
                Some((s, e)) => edits.push((s, e, c.name.clone())),
                None if copies.contains(&k) => {}
                None => {
                    let s = toks[k].1;
                    edits.push((s, s + c.name.len(), format!("{}.addr()", c.name)));
                }
            }
        }
        match c.lett {
            Some((u, s, e)) => {
                let at = toks[u].1;
                edits.push((at, at + 3, format!("Ref<{t}>")));
                if copy_of(*ci).is_none() {
                    edits.push((toks[s].1, toks[s].1, "Ref::new(".into()));
                    edits.push((toks[e].1, toks[e].1, ")".into()));
                }
            }
            None => {
                let at = toks[open].1 + 1;
                let n = &c.name;
                edits.push((at, at, format!("\n    let {n}: Ref<{t}> = Ref::new({n});")));
            }
        }
    }
    Some(close + 1)
}

/// Where the use at token `k` is `Ref::<T>::new(k)`: that text's bytes
/// and `T`.
fn made_ref(text: &str, toks: &[(Tok, usize)], k: usize) -> Option<(usize, usize, String)> {
    let p = |i: usize| punct(toks, i);
    if k < 8
        || p(k - 1) != Some(b'(')
        || p(k + 1) != Some(b')')
        || word(text, toks, k - 2) != Some("new")
        || p(k - 3) != Some(b':')
        || p(k - 4) != Some(b':')
        || p(k - 5) != Some(b'>')
    {
        return None;
    }
    let gt = k - 5;
    let mut depth = 0;
    let mut lt = gt;
    loop {
        match p(lt) {
            Some(b'>') => depth += 1,
            Some(b'<') => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            break;
        }
        lt = lt.checked_sub(1)?;
    }
    if lt < 3
        || p(lt - 1) != Some(b':')
        || p(lt - 2) != Some(b':')
        || word(text, toks, lt - 3) != Some("Ref")
        || p(lt - 4) == Some(b':')
    {
        return None;
    }
    let ty = text[toks[lt].1 + 1..toks[gt].1].trim().to_string();
    Some((toks[lt - 3].1, toks[k + 1].1 + 1, ty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_a_parameter_and_its_copy() {
        let t = "pub fn act(g: &mut Game, r13: u32, n: u32) -> () {\n    let n_copy: u32 = n;\n    let x: u32 = g.get(Ref::<t::Node>::new(n_copy).x());\n    g.set(Ref::<t::Node>::new(n).dx(), x);\n    if n == 0 { return; }\n    g.set(Ref::<t::Node>::new(n_copy).x(), x);\n}\n";
        let (o, b) = bind(t);
        assert_eq!(
            b,
            Bound {
                named: 2,
                proven: 0
            }
        );
        assert_eq!(
            o,
            "pub fn act(g: &mut Game, r13: u32, n: u32) -> () {\n    let n: Ref<t::Node> = Ref::new(n);\n    let n_copy: Ref<t::Node> = n;\n    let x: u32 = g.get(n_copy.x());\n    g.set(n.dx(), x);\n    if n.addr() == 0 { return; }\n    g.set(n_copy.x(), x);\n}\n"
        );
        assert_eq!(at_use(t), 3);
        assert_eq!(at_use(&o), 0);
    }

    #[test]
    fn wraps_a_let_and_keeps_other_uses() {
        let t = "fn f(g: &mut Game) {\n    let p: u32 = ld32(g, 0x3000000);\n    let v: u32 = (g.at(Ref::<H_08000290>::to(p, 0x8)).f_8 as u32);\n    g.at_mut(Ref::<H_08000290>::new(p)).f_8 = v as u16;\n    call(g, p.wrapping_add(4));\n}\n";
        let (o, b) = bind(t);
        assert_eq!(
            b,
            Bound {
                named: 0,
                proven: 1
            }
        );
        assert!(
            o.contains("let p: Ref<H_08000290> = Ref::new(ld32(g, 0x3000000));"),
            "{o}"
        );
        assert!(o.contains("Ref::<H_08000290>::to(p.addr(), 0x8)"), "{o}");
        assert!(o.contains("g.at_mut(p).f_8"), "{o}");
        assert!(o.contains("call(g, p.addr().wrapping_add(4))"), "{o}");
    }

    #[test]
    fn leaves_mut_shadowed_and_mixed_types() {
        // `mut`: assigned again; two types; an object `n` is in.
        for t in [
            "fn f(g: &mut Game, mut n: u32) {\n    g.get(Ref::<t::A>::new(n).x());\n    n = 3;\n}\n",
            "fn f(g: &mut Game, n: u32) {\n    g.get(Ref::<t::A>::new(n).x());\n    g.get(Ref::<t::B>::new(n).y());\n}\n",
            "fn f(g: &mut Game, n: u32) {\n    g.get(Ref::<t::A>::to(n, 4).x());\n}\n",
        ] {
            let (o, b) = bind(t);
            assert_eq!(o, t);
            assert_eq!(b, Bound::default());
        }
    }

    #[test]
    fn lets_of_one_name_in_two_blocks() {
        let t = "fn f(g: &mut Game) {\n    if c {\n    let n: u32 = r5;\n    g.get(Ref::<t::A>::new(n).x());\n    } else {\n    let n: u32 = r4;\n    g.set(Ref::<t::A>::new(n).y(), n);\n    }\n}\n";
        let (o, b) = bind(t);
        assert_eq!(b.named, 2);
        assert!(
            o.contains("let n: Ref<t::A> = Ref::new(r5);\n    g.get(n.x());"),
            "{o}"
        );
        assert!(o.contains("g.set(n.y(), n.addr());"), "{o}");
        // An outer `let` or parameter a block's `let` shadows is bound
        // around it.
        let t = "fn f(g: &mut Game, n: u32) {\n    g.get(Ref::<t::A>::new(n).x());\n    {\n    let n: u32 = r4;\n    g.get(Ref::<t::B>::new(n).y());\n    }\n    call(g, n);\n}\n";
        let (o, b) = bind(t);
        assert_eq!(b.named, 2);
        assert!(
            o.contains("let n: Ref<t::A> = Ref::new(n);\n    g.get(n.x());"),
            "{o}"
        );
        assert!(
            o.contains("let n: Ref<t::B> = Ref::new(r4);\n    g.get(n.y());"),
            "{o}"
        );
        assert!(o.contains("call(g, n.addr());"), "{o}");
    }

    #[test]
    fn a_let_a_later_one_shadows() {
        // The first's uses end at the second's `;`: its right side still
        // reads the first. One in a block inside hides it only there.
        let t = "fn f(g: &mut Game) {\n    let b: u32 = r4;\n    g.get(Ref::<t::A>::new(b).x());\n    {\n    let b: u32 = 7;\n    call(g, b);\n    }\n    g.get(Ref::<t::A>::new(b).y());\n    let b: u32 = b.wrapping_add(4);\n    g.get(Ref::<t::A>::new(b).z());\n}\n";
        let (o, bd) = bind(t);
        assert_eq!(bd.named, 2);
        assert!(
            o.contains("let b: Ref<t::A> = Ref::new(r4);\n    g.get(b.x());"),
            "{o}"
        );
        assert!(o.contains("let b: u32 = 7;\n    call(g, b);"), "{o}");
        assert!(o.contains("g.get(b.y());\n    let b: Ref<t::A> = Ref::new(b.addr().wrapping_add(4));\n    g.get(b.z());"), "{o}");
    }

    #[test]
    fn a_function_returning_an_array() {
        let t = "pub fn f(g: &mut Game, mut r13: u32, [mut r0, mut r1, mut r2, mut r3]: [u32; 4]) -> Result<[u32; 4], Exit> {\n    let n: u32 = r4;\n    g.get(Ref::<t::A>::new(n).x());\n    Ok([r0, r1, r2, r3])\n}\n";
        let (o, b) = bind(t);
        assert_eq!(b.named, 1, "{o}");
    }

    #[test]
    fn a_compare_is_a_use_not_a_macro() {
        let t = "fn f(g: &mut Game) {\n    let p: u32 = r8;\n    g.set(Ref::<u8>::new(p), 1);\n    let c: bool = p != r1;\n}\n";
        let (o, _) = bind(t);
        assert!(o.contains("let c: bool = p.addr() != r1;"), "{o}");
    }

    #[test]
    fn an_array_type_and_fields_of_the_same_name() {
        let t = "fn f(g: &mut Game, x: u32, i: u32) {\n    let a: u32 = g.get(Ref::<[t::Obj; 4]>::new(x).idx(i).x());\n    g.ram.x = a;\n}\n";
        let (o, _) = bind(t);
        assert!(o.contains("let x: Ref<[t::Obj; 4]> = Ref::new(x);"), "{o}");
        assert!(o.contains("g.get(x.idx(i).x())"), "{o}");
        assert!(o.contains("g.ram.x = a;"), "{o}");
    }
}
