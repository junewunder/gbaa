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
//!   first line, `let w: Ref<T> = Ref::new(w);`, after the `{` line the
//!   function's entries end with;
//! - each `Ref::<T>::new(w)` prints `w`, and every other use `w.addr()`
//!   (C's pointer as an integer: a compare, arithmetic, an argument).
//!
//! A register local (unit `native/lift-pointer-registers`): a `mut`
//! parameter, a `let mut w: u32`, or a name of the entries' tuple
//! `let (mut r4, mut pData) = (0u32, 0u32);`, declared nowhere else in
//! its function, whose made `Ref`s are all of one type, is declared that
//! type: its own line out of the tuple (`let mut pData: Ref<T> =
//! Ref::new(0u32);`), a parameter bound again as `mut`, and each plain
//! assignment's right side wrapped, `pData = Ref::new(e);` (or as it is,
//! a variable bound to the same type). A register that holds a number in
//! between holds it as a `Ref` too: the word is the same. One assigned
//! with others (`(a, w) = …`) or by an operator (`w += 4`) isn't bound.
//!
//! Otherwise only a variable declared once in its scope (no shadowing,
//! no pattern) and made a `Ref` of one type at least once is bound; a
//! `Ref::<T>::to(w, k)` (an object `w` is a field of) is an other use. **A type changes no byte read**: `Ref::new` and `addr` are
//! the same word, so the accesses are the ones the untyped text made.
//!
//! What it counts, apart by where the type came from: `named` (a type of
//! the names files, `t::…`; [`super::fields`]) and `proven` (a heap run's
//! type, [`super::heap`]). [`at_use`] counts the `Ref::<T>::new(…)` and
//! `Ref::<T>::to(…)` left at a use (`ref_new_at_use`, barred), and
//! [`to_at_use`] the `::to(…)` alone (`ref_to_at_use`, barred;
//! [`super::field_base`]).

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
    made(text, &["::new(", "::to("])
}

/// `Ref::<T>::to(` in `text`: a typed pointer made at its use from one
/// into its object (`ref_to_at_use`; native/lift-field-base).
pub fn to_at_use(text: &str) -> u64 {
    made(text, &["::to("])
}

fn made(text: &str, ends: &[&str]) -> u64 {
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
        if depth == 0 && ends.iter().any(|e| text[j..].starts_with(e)) {
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
    /// A parameter (bound again after the entries).
    param: bool,
    /// Declared once in its function, and followed through its plain
    /// assignments (`w = e;`): a register local.
    reg: bool,
    /// A parameter's `mut` token, dropped where it is bound again.
    mut_tok: Option<usize>,
    /// Its tuple `let` (in `lets`) and its place in it.
    tuple: Option<(usize, usize)>,
}

/// A `let (mut a, b) = (x, y);`: its `let` and `;` tokens, each name's
/// tokens (`mut`'s, the name's) and each value's bytes.
struct TupleLet {
    first: usize,
    semi: usize,
    names: Vec<(Option<usize>, usize)>,
    vals: Vec<(usize, usize)>,
}

/// A plain assignment `w = e` of a register local: its right side's
/// first token and the token after its last.
type Assign = (usize, usize);

/// The bytes that stay as they are, `frozen` (an edit there drops the
/// function's), and the bytes [`bind_in`] says where they went, `track`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keep {
    pub frozen: (usize, usize),
    pub track: (usize, usize),
}

/// `text` (one or more functions) with its pointer variables bound; a
/// parameter's binding goes no earlier than byte `after` (the end of the
/// entries the printer finds by their text).
pub fn bind(text: &str, after: usize) -> (String, Bound) {
    let (t, b, _) = bind_in(text, after, None);
    (t, b)
}

/// [`bind`], leaving `keep.frozen` as it is and giving where
/// `keep.track` went: the entries' bytes, of which the printer keeps two
/// copies ([`super::Entries`]) and only their common end may change.
pub fn bind_in(
    text: &str,
    after: usize,
    keep: Option<Keep>,
) -> (String, Bound, Option<(usize, usize)>) {
    let mut bound = Bound::default();
    let unchanged = |b| (text.to_string(), b, keep.map(|k| k.track));
    let Some(toks) = tokens(text) else {
        return unchanged(bound);
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    while i + 2 < toks.len() {
        if word(text, &toks, i) == Some("fn")
            && word(text, &toks, i + 1).is_some()
            && punct(&toks, i + 2) == Some(b'(')
        {
            let (n, b) = (edits.len(), bound);
            if let Some(next) = one_fn(text, &toks, i + 2, after, &mut edits, &mut bound) {
                if let Some(k) = keep
                    && edits[n..].iter().any(|e| touches(e, k.frozen))
                {
                    edits.truncate(n);
                    bound = b;
                }
                i = next;
                continue;
            }
        }
        i += 1;
    }
    if edits.is_empty() {
        return unchanged(bound);
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let track = keep.and_then(|k| {
        Some((
            moved(&edits, k.track.0, true)?,
            moved(&edits, k.track.1, false)?,
        ))
    });
    if keep.is_some() && track.is_none() {
        return unchanged(Bound::default());
    }
    let mut out = String::with_capacity(text.len() + edits.len() * 8);
    let mut at = 0;
    for (s, e, t) in edits {
        debug_assert!(s >= at, "overlapping edits");
        out.push_str(&text[at..s]);
        out.push_str(&t);
        at = e;
    }
    out.push_str(&text[at..]);
    (out, bound, track)
}

/// Whether edit `e` changes a byte of `r`, or inserts inside it.
fn touches(e: &(usize, usize, String), r: (usize, usize)) -> bool {
    e.0 < r.1 && (e.1 > r.0 || (e.0 == e.1 && e.0 > r.0))
}

/// Where byte `p` goes under `edits` (sorted): an insertion at `p` goes
/// after it where `start`, else before. `None` inside a replacement.
fn moved(edits: &[(usize, usize, String)], p: usize, start: bool) -> Option<usize> {
    let mut q = p as isize;
    for (s, e, t) in edits {
        if *s < p && *e > p {
            return None;
        }
        if *e < p || (*e == p && (*s < p || !start)) {
            q += t.len() as isize - (*e - *s) as isize;
        }
    }
    Some(q as usize)
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

/// The byte after token `i`.
fn tok_end(toks: &[(Tok, usize)], i: usize) -> usize {
    match toks[i].0 {
        Tok::Word(_, e) => e,
        Tok::Punct(_) => toks[i].1 + 1,
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

/// Token `k` begins a statement (or a match arm's expression).
fn stmt_start(toks: &[(Tok, usize)], k: usize) -> bool {
    let p = |i: usize| punct(toks, i);
    k >= 2
        && (matches!(p(k - 1), Some(b';' | b'{' | b'}'))
            || (p(k - 1) == Some(b'>') && p(k - 2) == Some(b'=')))
}

/// `[from, to)`'s items split by `,` at its top: each item's tokens.
fn items(toks: &[(Tok, usize)], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = from;
    let mut d = 0;
    for j in from..to {
        match punct(toks, j) {
            Some(b'(' | b'[' | b'{' | b'<') => d += 1,
            Some(b')' | b']' | b'}' | b'>') => d -= 1,
            Some(b',') if d == 0 => {
                out.push((start, j));
                start = j + 1;
            }
            _ => {}
        }
    }
    if start < to {
        out.push((start, to));
    }
    out
}

/// A pattern item of a name: its `mut` token, if it has one, and the
/// name's.
fn name_item(
    text: &str,
    toks: &[(Tok, usize)],
    (s, e): (usize, usize),
) -> Option<(Option<usize>, usize)> {
    let n = |i| word(text, toks, i).filter(|w| *w != "mut" && *w != "_");
    match e - s {
        1 => n(s).map(|_| (None, s)),
        2 if word(text, toks, s) == Some("mut") => n(s + 1).map(|_| (Some(s), s + 1)),
        _ => None,
    }
}

/// The function whose parameters open at `lp`: its edits. The token
/// after its body, or `None` where it has none (a declaration).
fn one_fn(
    text: &str,
    toks: &[(Tok, usize)],
    lp: usize,
    after: usize,
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
    let mut decls: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let param = |name: usize, mut_tok: Option<usize>| Cand {
        name: w(name).unwrap_or_default().to_string(),
        decl: name,
        lett: None,
        param: true,
        reg: mut_tok.is_some(),
        mut_tok,
        tuple: None,
    };
    // Parameters, each between commas at the top: `w: u32`, `mut w:
    // u32` and `[mut r0, r1, _, r3]: [u32; 4]`.
    for (s, e) in items(toks, lp + 1, rp) {
        if e == s + 3 && p(s + 1) == Some(b':') && w(s + 2) == Some("u32") {
            if let Some((None, n)) = name_item(text, toks, (s, s + 1)) {
                cands.push(param(n, None));
            }
        } else if e == s + 4 && p(s + 2) == Some(b':') && w(s + 3) == Some("u32") {
            if let Some((Some(m), n)) = name_item(text, toks, (s, s + 2)) {
                cands.push(param(n, Some(m)));
            }
        } else if p(s) == Some(b'[')
            && let Some(c) = close_of(toks, s)
            && p(c + 1) == Some(b':')
            && w(c + 3) == Some("u32")
        {
            for it in items(toks, s + 1, c) {
                if let Some((m, n)) = name_item(text, toks, it) {
                    decls.entry(w(n)?).or_default().push(n);
                    cands.push(param(n, m));
                }
            }
        }
    }
    // Every name a `let`, `for` or parameter pattern declares (its
    // token), and the plain `let w: u32 = e;`s.
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
    let mut tuples: Vec<TupleLet> = Vec::new();
    // A tuple or array assignment's targets: `(a, b) = …`.
    let mut targets: Vec<(usize, usize)> = Vec::new();
    let mut j = open;
    while j < close {
        match p(j) {
            Some(b'{') => groups.push(close_of(toks, j)?),
            Some(b'}') => {
                groups.pop();
            }
            Some(b'(' | b'[') if stmt_start(toks, j) => {
                if let Some(c) = close_of(toks, j)
                    && p(c + 1) == Some(b'=')
                    && p(c + 2) != Some(b'=')
                {
                    targets.push((j, c));
                }
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
            // `let (mut a, b) = (x, y);`: each name a register local.
            if kw == Some("let")
                && p(j + 1) == Some(b'(')
                && close_of(toks, j + 1) == Some(k - 1)
                && p(k) == Some(b'=')
                && p(k + 1) == Some(b'(')
                && let Some(vc) = close_of(toks, k + 1)
                && p(vc + 1) == Some(b';')
            {
                let names: Option<Vec<_>> = items(toks, j + 2, k - 1)
                    .into_iter()
                    .map(|it| name_item(text, toks, it))
                    .collect();
                let vals: Vec<_> = items(toks, k + 2, vc)
                    .into_iter()
                    .map(|(s, e)| (toks[s].1, tok_end(toks, e - 1)))
                    .collect();
                if let Some(names) = names
                    && names.len() == vals.len()
                {
                    for (ei, &(m, n)) in names.iter().enumerate() {
                        cands.push(Cand {
                            name: w(n)?.to_string(),
                            decl: n,
                            lett: None,
                            param: false,
                            reg: true,
                            mut_tok: m,
                            tuple: Some((tuples.len(), ei)),
                        });
                    }
                    tuples.push(TupleLet {
                        first: j,
                        semi: vc + 1,
                        names,
                        vals,
                    });
                }
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
                if p(k) == Some(b':')
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
                        param: false,
                        reg: at == j + 2,
                        mut_tok: None,
                        tuple: None,
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
    // Where a parameter is bound again: after the `{` line, and the
    // entries' text if it goes on past it (`one_entry` finds them by it);
    // its uses are from there.
    let rebind = {
        let b = toks[open].1 + 1;
        let mut at = text[b..].find('\n').map_or(b, |n| b + n + 1);
        if at < after && after <= toks[close].1 {
            at = after;
        }
        at
    };
    let rebind_tok = (open..close)
        .find(|&k| toks[k].1 >= rebind)
        .unwrap_or(close);
    // Each candidate's uses: (token, the `Ref::<T>::new(w)` around it).
    let mut types: Vec<Option<String>> = vec![None; cands.len()];
    let mut owner: BTreeMap<usize, usize> = BTreeMap::new();
    let mut plans: Vec<(usize, Vec<Use>, Vec<Assign>)> = Vec::new();
    for (ci, c) in cands.iter().enumerate() {
        if body.contains(&format!("{{{}}}", c.name)) || body.contains(&format!("{{{}:", c.name)) {
            continue;
        }
        let mine = decls.get(c.name.as_str()).map_or(&[][..], |v| &v[..]);
        // Where its uses are: a parameter's, the body past its binding; a
        // `let`'s, from its `;` to its group's end. Up to a later `let` of
        // its name in that group (its right side still this one's), and
        // but the scope of one in a group inside; any other binding of
        // its name there, and it isn't bound. A register local is the
        // only binding of its name.
        let (from, mut to) = match (c.lett, c.tuple) {
            _ if c.param => (rebind_tok, close),
            (Some(l), _) => (l.2, if c.reg { close } else { lets[&c.decl].1 }),
            (_, Some((t, _))) => (tuples[t].semi, close),
            _ => continue,
        };
        if c.reg && mine != [c.decl] {
            continue;
        }
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
        let mut assigns = Vec::new();
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
            // `a.w` (not a range's `0..w`).
            let field = p(k - 1) == Some(b'.') && p(k - 2) != Some(b'.');
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
            if targets.iter().any(|t| k > t.0 && k < t.1) {
                // Assigned with others: not followed.
                ok = false;
                break;
            }
            if stmt_start(toks, k) {
                let op = |i: usize| {
                    matches!(
                        p(i),
                        Some(b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'|' | b'&' | b'<' | b'>')
                    )
                };
                if p(k + 1) == Some(b'=') && p(k + 2) != Some(b'=') {
                    // `w = e`: its right side to the `;`, `,` or brace
                    // that ends it.
                    let mut e = k + 2;
                    let mut d = 0;
                    while e < to {
                        match p(e) {
                            Some(b'(' | b'[' | b'{') => d += 1,
                            Some(b')' | b']' | b'}') if d == 0 => break,
                            Some(b')' | b']' | b'}') => d -= 1,
                            Some(b';' | b',') if d == 0 => break,
                            _ => {}
                        }
                        e += 1;
                    }
                    if e == k + 2 || e >= to || !c.reg {
                        ok = false;
                        break;
                    }
                    assigns.push((k + 2, e));
                    continue;
                }
                if op(k + 1) && (p(k + 2) == Some(b'=') || (op(k + 2) && p(k + 3) == Some(b'='))) {
                    ok = false;
                    break;
                }
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
            plans.push((ci, uses, assigns));
        }
    }
    // A right side that is one variable bound to its type: a copy.
    let same = |ci: usize, s: usize, e: usize| -> bool {
        e == s + 1 && owner.get(&s).is_some_and(|&o| types[o] == types[ci])
    };
    let copy_of = |ci: usize| -> Option<usize> {
        let (_, s, e) = cands[ci].lett?;
        same(ci, s, e).then_some(s)
    };
    let mut copies: Vec<usize> = plans.iter().filter_map(|&(ci, ..)| copy_of(ci)).collect();
    for (ci, _, assigns) in &plans {
        copies.extend(assigns.iter().filter(|a| same(*ci, a.0, a.1)).map(|a| a.0));
    }
    let mut in_tuples: BTreeMap<usize, Vec<(usize, String)>> = BTreeMap::new();
    for (ci, uses, assigns) in &plans {
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
        for &(s, e) in assigns {
            if !same(*ci, s, e) {
                edits.push((toks[s].1, toks[s].1, "Ref::new(".into()));
                let end = tok_end(toks, e - 1);
                edits.push((end, end, ")".into()));
            }
        }
        let m = if c.reg { "mut " } else { "" };
        match (c.lett, c.tuple) {
            (Some((u, s, e)), _) => {
                let at = toks[u].1;
                edits.push((at, at + 3, format!("Ref<{t}>")));
                if copy_of(*ci).is_none() {
                    edits.push((toks[s].1, toks[s].1, "Ref::new(".into()));
                    edits.push((toks[e].1, toks[e].1, ")".into()));
                }
            }
            (_, Some((ti, ei))) => in_tuples.entry(ti).or_default().push((ei, t.to_string())),
            _ => {
                if let Some(mt) = c.mut_tok {
                    edits.push((toks[mt].1, toks[c.decl].1, String::new()));
                }
                let n = &c.name;
                edits.push((
                    rebind,
                    rebind,
                    format!("    let {m}{n}: Ref<{t}> = Ref::new({n});\n"),
                ));
            }
        }
    }
    // A tuple `let` with names bound: the others' `let`, then a line
    // each.
    for (ti, bound_here) in in_tuples {
        let tl = &tuples[ti];
        let s = toks[tl.first].1;
        let indent = &text[text[..s].rfind('\n').map_or(0, |n| n + 1)..s];
        let item = |ei: usize| {
            let (m, n) = tl.names[ei];
            let m = if m.is_some() { "mut " } else { "" };
            (
                format!("{m}{}", w(n).unwrap_or_default()),
                &text[tl.vals[ei].0..tl.vals[ei].1],
            )
        };
        let rest: Vec<_> = (0..tl.names.len())
            .filter(|ei| bound_here.iter().all(|b| b.0 != *ei))
            .map(item)
            .collect();
        let mut lines: Vec<String> = match &rest[..] {
            [] => Vec::new(),
            [(n, v)] => vec![format!("let {n} = {v};")],
            ns => vec![format!(
                "let ({}) = ({});",
                ns.iter()
                    .map(|x| x.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                ns.iter().map(|x| x.1).collect::<Vec<_>>().join(", ")
            )],
        };
        let mut bound_here = bound_here;
        bound_here.sort();
        for (ei, t) in bound_here {
            let (n, v) = item(ei);
            lines.push(format!("let {n}: Ref<{t}> = Ref::new({v});"));
        }
        edits.push((
            s,
            tok_end(toks, tl.semi),
            lines.join(&format!("\n{indent}")),
        ));
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
        let (o, b) = bind(t, 0);
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
        let (o, b) = bind(t, 0);
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
        // Two types; an object `n` is in; a register local assigned with
        // another, or by `+=`.
        for t in [
            "fn f(g: &mut Game) {\n    let (mut n, mut m) = (0u32, 0u32);\n    g.get(Ref::<t::A>::new(n).x());\n    (n, m) = (m, n);\n}\n",
            "fn f(g: &mut Game) {\n    let (mut n, mut m) = (0u32, 0u32);\n    g.get(Ref::<t::A>::new(n).x());\n    n += 4;\n}\n",
            "fn f(g: &mut Game, n: u32) {\n    g.get(Ref::<t::A>::new(n).x());\n    g.get(Ref::<t::B>::new(n).y());\n}\n",
            "fn f(g: &mut Game, n: u32) {\n    g.get(Ref::<t::A>::to(n, 4).x());\n}\n",
        ] {
            let (o, b) = bind(t, 0);
            assert_eq!(o, t);
            assert_eq!(b, Bound::default());
        }
    }

    #[test]
    fn lets_of_one_name_in_two_blocks() {
        let t = "fn f(g: &mut Game) {\n    if c {\n    let n: u32 = r5;\n    g.get(Ref::<t::A>::new(n).x());\n    } else {\n    let n: u32 = r4;\n    g.set(Ref::<t::A>::new(n).y(), n);\n    }\n}\n";
        let (o, b) = bind(t, 0);
        assert_eq!(b.named, 2);
        assert!(
            o.contains("let n: Ref<t::A> = Ref::new(r5);\n    g.get(n.x());"),
            "{o}"
        );
        assert!(o.contains("g.set(n.y(), n.addr());"), "{o}");
        // An outer `let` or parameter a block's `let` shadows is bound
        // around it.
        let t = "fn f(g: &mut Game, n: u32) {\n    g.get(Ref::<t::A>::new(n).x());\n    {\n    let n: u32 = r4;\n    g.get(Ref::<t::B>::new(n).y());\n    }\n    call(g, n);\n}\n";
        let (o, b) = bind(t, 0);
        assert_eq!(b.named, 2);
        assert!(
            o.contains("{\n    let n: Ref<t::A> = Ref::new(n);\n    g.get(n.x());"),
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
        let (o, bd) = bind(t, 0);
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
        let (o, b) = bind(t, 0);
        assert_eq!(b.named, 1, "{o}");
    }

    #[test]
    fn a_compare_is_a_use_not_a_macro() {
        let t = "fn f(g: &mut Game) {\n    let p: u32 = r8;\n    g.set(Ref::<u8>::new(p), 1);\n    let c: bool = p != r1;\n}\n";
        let (o, _) = bind(t, 0);
        assert!(o.contains("let c: bool = p.addr() != r1;"), "{o}");
    }

    #[test]
    fn a_parameter_binds_after_the_entries() {
        let head = "pub fn f(g: &mut Game, r13: u32, s: u32) -> () {\n    let (mut r4, mut r1) = (0u32, 0u32);\n";
        let t = format!("{head}    g.get(Ref::<t::A>::new(s).x());\n}}\n");
        let (o, _) = bind(&t, head.len());
        assert!(o.starts_with(head), "{o}");
        assert!(
            o.contains("= (0u32, 0u32);\n    let s: Ref<t::A> = Ref::new(s);\n    g.get(s.x());"),
            "{o}"
        );
    }

    #[test]
    fn an_array_type_and_fields_of_the_same_name() {
        let t = "fn f(g: &mut Game, x: u32, i: u32) {\n    let a: u32 = g.get(Ref::<[t::Obj; 4]>::new(x).idx(i).x());\n    g.ram.x = a;\n}\n";
        let (o, _) = bind(t, 0);
        assert!(
            o.contains("{\n    let x: Ref<[t::Obj; 4]> = Ref::new(x);\n"),
            "{o}"
        );
        assert!(o.contains("g.get(x.idx(i).x())"), "{o}");
        assert!(o.contains("g.ram.x = a;"), "{o}");
    }

    #[test]
    fn a_register_local_follows_its_assignments() {
        let t = "fn f(g: &mut Game, sp: u32, [mut r0, r1, _, mut r3]: [u32; 4]) -> () {\n    let (mut r4, r13, mut pData) = (0u32, sp, 0u32);\n    let e0: u32 = r0;\n    pData = r1;\n    g.get(Ref::<t::S>::new(pData).x());\n    if c { pData = pData.wrapping_add(4) } else { pData = r4 }\n    r0 = pData;\n    r4 = 7;\n    call(Regs { r: [r0, pData, r3] });\n    g.set(Ref::<t::S>::new(r0).y(), 1);\n    r3 = 2;\n    g.set(Ref::<u8>::new(r3), 1);\n}\n";
        let head = "fn f(g: &mut Game, sp: u32, [mut r0, r1, _, mut r3]: [u32; 4]) -> () {\n    let (mut r4, r13, mut pData) = (0u32, sp, 0u32);\n    let e0: u32 = r0;\n";
        let (o, b) = bind(t, head.len());
        assert_eq!(b.named, 2, "{o}");
        assert_eq!(b.proven, 1, "{o}");
        assert!(o.starts_with("fn f(g: &mut Game, sp: u32, [r0, r1, _, r3]: [u32; 4]) -> () {\n    let (mut r4, r13) = (0u32, sp);\n    let mut pData: Ref<t::S> = Ref::new(0u32);\n    let e0: u32 = r0;\n    let mut r0: Ref<t::S> = Ref::new(r0);\n    let mut r3: Ref<u8> = Ref::new(r3);\n"), "{o}");
        assert!(
            o.contains("pData = Ref::new(r1);\n    g.get(pData.x());"),
            "{o}"
        );
        assert!(
            o.contains(
                "{ pData = Ref::new(pData.addr().wrapping_add(4)) } else { pData = Ref::new(r4) }"
            ),
            "{o}"
        );
        // A copy of one of its type.
        assert!(o.contains("r0 = pData;\n    r4 = 7;"), "{o}");
        assert!(
            o.contains("Regs { r: [r0.addr(), pData.addr(), r3.addr()] }"),
            "{o}"
        );
        assert!(
            o.contains("g.set(r0.y(), 1);\n    r3 = Ref::new(2);\n    g.set(r3, 1);"),
            "{o}"
        );
        assert_eq!(at_use(&o), 0);
    }

    #[test]
    fn a_range_bound_is_a_use() {
        let t = "fn f(g: &mut Game) {\n    let (mut n) = (0u32);\n    g.get(Ref::<u8>::new(n));\n    for i in 0..n { }\n}\n";
        let (o, _) = bind(t, 0);
        assert!(o.contains("for i in 0..n.addr() { }"), "{o}");
    }

    #[test]
    fn a_mut_let_and_a_lone_tuple_name() {
        let t = "fn f(g: &mut Game) {\n    let (mut p) = (0u32);\n    let mut q: u32 = r5;\n    g.get(Ref::<t::A>::new(q).x());\n    q = p;\n    g.get(Ref::<t::A>::new(p).x());\n}\n";
        let (o, b) = bind(t, 0);
        assert_eq!(b.named, 2, "{o}");
        assert!(o.contains("{\n    let mut p: Ref<t::A> = Ref::new(0u32);\n    let mut q: Ref<t::A> = Ref::new(r5);\n    g.get(q.x());\n    q = p;\n    g.get(p.x());"), "{o}");
    }

    #[test]
    fn the_entries_kept_but_their_end() {
        let t = "fn x(g: &mut Game) { let (mut p) = (0u32); }\nfn f(g: &mut Game) {\n    let (mut p, mut q) = (0u32, 0u32);\n    g.get(Ref::<t::A>::new(p).x());\n}\n";
        let at = t.find("fn f").unwrap();
        let line = t.find("    let (mut p, mut q)").unwrap();
        let end = t.find(";\n    g.get").unwrap() + 2;
        let keep = Keep {
            frozen: (0, line),
            track: (line, end),
        };
        let (o, b, r) = bind_in(t, end, Some(keep));
        assert_eq!(b.named, 1);
        let (s, e) = r.unwrap();
        assert_eq!(&o[..s], &t[..line]);
        assert_eq!(
            &o[s..e],
            "    let mut q = 0u32;\n    let mut p: Ref<t::A> = Ref::new(0u32);\n"
        );
        // An edit in the frozen part leaves that function as it was.
        let keep = Keep {
            frozen: (at, line + 5),
            track: (line + 5, end),
        };
        let (o, b, _) = bind_in(t, end, Some(keep));
        assert_eq!(b, Bound::default());
        assert_eq!(o, t);
    }
}
