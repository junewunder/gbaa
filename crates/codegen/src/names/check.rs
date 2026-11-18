//! The named print's token check (`docs/mod-view-design.md`, "The check
//! that proves it"): each function printed unnamed and named from the
//! same lowering, walked in lockstep. Where the named print has an
//! identifier of the map's table, the unnamed one must have that key's
//! unnamed form (`f_<key>`); where it has `addr!(X)` or `addr!(X + n)`,
//! the literal `key(X) + n` (X a datum, or a function as a pool word);
//! where it has one of the function's DWARF
//! locals (`crate::dwarf`), `vN` (`name_vN`: that `vN`); where it has an
//! I/O register (`REG_IME`, `crate::io_regs`), the `Ptr::<t>::at(lit)`
//! that register's address and width print as; a doc attribute
//! and a `note!` comment only the named print has are skipped; anything
//! else must be the same token.

use super::{Kind, SourceMap};
use crate::func::{Ctx, Func};
use crate::lower::{fn_name, lower_func_checked};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use std::collections::BTreeSet;

/// What one function's check found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Named {
    /// Function identifiers printed from the map.
    pub idents: usize,
    /// `addr!` constants printed.
    pub addrs: usize,
    /// DWARF register variables printed by name somewhere in their
    /// function (`crate::dwarf`).
    pub vars_named: usize,
    /// Those printed only as comments (`// r4: keyInput`).
    pub vars_commented: usize,
    /// Comments: `src/file.c:line`, stack slots, calls.
    pub lines: usize,
    pub slots: usize,
    pub calls: usize,
    /// Text a pool word points at (`gText_X: "…"`, `crate::text`).
    pub texts: usize,
    /// Tables of text a pool word points at (`sNames: ["A", "B"]`,
    /// `crate::text::Charmap::table_note`).
    pub text_tables: usize,
    /// Bytes a pool word points at where no table decodes them
    /// (`sData: [u8; 4] = [..]`, `crate::text::Preview`).
    pub previews: usize,
}

impl std::ops::AddAssign for Named {
    fn add_assign(&mut self, o: Named) {
        self.idents += o.idents;
        self.addrs += o.addrs;
        self.vars_named += o.vars_named;
        self.vars_commented += o.vars_commented;
        self.lines += o.lines;
        self.slots += o.slots;
        self.calls += o.calls;
        self.texts += o.texts;
        self.text_tables += o.text_tables;
        self.previews += o.previews;
    }
}

/// `f` printed unnamed in `plain` and named in `named` (the same context
/// but for its `names`), and checked. `Err` names the first differing
/// token.
pub fn check_func(plain: &Ctx, named: &Ctx, f: &Func) -> Result<Named, String> {
    let names = named.names.ok_or("the named context has no source map")?;
    let a = lower_func_checked(plain, f, &fn_name(f.entry)).0;
    let b = lower_func_checked(named, f, &named.fn_ident(f.entry)).0;
    let locals = crate::dwarf::FnDwarf::idents(names, f.entry);
    check_tokens_in(names, &locals, a, b)
}

/// The walk: `plain` the unnamed print, `named` the named one.
pub fn check_tokens(
    names: &SourceMap,
    plain: TokenStream,
    named: TokenStream,
) -> Result<Named, String> {
    check_tokens_in(names, &[], plain, named)
}

/// [`check_tokens`], for a function whose DWARF locals print as `locals`
/// (`crate::dwarf::FnDwarf::idents`).
pub fn check_tokens_in(
    names: &SourceMap,
    locals: &[String],
    plain: TokenStream,
    named: TokenStream,
) -> Result<Named, String> {
    let mut n = Named::default();
    let mut w = Walk {
        names,
        locals: locals.iter().map(String::as_str).collect(),
        named: BTreeSet::new(),
        commented: BTreeSet::new(),
        scopes: vec![Vec::new()],
        in_pattern: false,
        pattern: Vec::new(),
    };
    w.walk(plain, named, &mut n)?;
    n.vars_named = w.named.len();
    n.vars_commented = w.commented.difference(&w.named).count();
    Ok(n)
}

/// The walk's state: the function's local identifiers, and the variables
/// it has seen printed by name and in comments.
struct Walk<'a> {
    names: &'a SourceMap,
    locals: BTreeSet<&'a str>,
    named: BTreeSet<String>,
    commented: BTreeSet<String>,
    /// The `let` bindings in scope, a frame per brace group: each named
    /// identifier with the unnamed one it stands for. A use must resolve
    /// to the same binding in both prints, so a name can't shadow a value
    /// its `vN` didn't.
    scopes: Vec<Vec<(String, String)>>,
    /// Inside a `let`'s pattern, and the pairs it binds so far.
    in_pattern: bool,
    pattern: Vec<(String, String)>,
}

impl Walk<'_> {
    /// `id`, printed where the unnamed print has `plain`: the DWARF local
    /// it names, if it is one.
    fn local(&self, plain: &str, id: &str) -> Option<String> {
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !plain.strip_prefix('v').is_some_and(digits) {
            return None;
        }
        if self.locals.contains(id) {
            return Some(id.to_string());
        }
        let base = id.strip_suffix(plain)?.strip_suffix('_')?;
        self.locals.contains(base).then(|| base.to_string())
    }

    /// A use of `named` where the unnamed print has `plain`: `Err` if
    /// either resolves to a binding the other doesn't.
    fn resolve(&self, plain: &str, named: &str) -> Result<(), String> {
        let frames = || self.scopes.iter().rev().flat_map(|f| f.iter().rev());
        let by_named = frames().find(|(n, _)| n == named).map(|(_, p)| p);
        let by_plain = frames().find(|(_, p)| p == plain).map(|(n, _)| n);
        if by_named.is_some_and(|p| p != plain) || by_plain.is_some_and(|n| n != named) {
            return Err(format!(
                "named `{named}` and unnamed `{plain}` resolve to different bindings (`{}` and `{}`)",
                by_named.map_or("none", |s| s.as_str()),
                by_plain.map_or("none", |s| s.as_str()),
            ));
        }
        Ok(())
    }

    /// A `note!` comment: counted by kind.
    fn note(&mut self, text: &str, n: &mut Named) {
        let reg = text
            .strip_prefix('r')
            .and_then(|t| t.split_once(": "))
            .filter(|(r, _)| r.parse::<u8>().is_ok());
        match reg {
            Some((_, name)) => {
                self.commented.extend(self.names.local_ident(name));
            }
            None if text.starts_with("sp+") => n.slots += 1,
            None if text.ends_with('"') && text.contains(": \"") => n.texts += 1,
            None if text.ends_with(']') && text.contains(": [\"") => n.text_tables += 1,
            None if text.contains(": [u8; ") && text.contains("; encoding unknown") => {
                n.previews += 1
            }
            None if text.contains('(') => n.calls += 1,
            None => n.lines += 1,
        }
    }

    fn walk(
        &mut self,
        plain: TokenStream,
        named: TokenStream,
        n: &mut Named,
    ) -> Result<(), String> {
        let names = self.names;
        let a: Vec<TokenTree> = plain.into_iter().collect();
        let b: Vec<TokenTree> = named.into_iter().collect();
        let (mut i, mut j) = (0, 0);
        // A `let` at this level: where its pattern ends, and what it binds
        // at its `;`.
        let mut pat_end = 0;
        let mut pending: Option<Vec<(String, String)>> = None;
        let punct = |t: Option<&TokenTree>, c: char| matches!(t, Some(TokenTree::Punct(p)) if p.as_char() == c);
        let differ = |x: Option<&TokenTree>, y: Option<&TokenTree>| {
            let s = |t: Option<&TokenTree>| t.map_or("the end".to_string(), |t| format!("`{t}`"));
            Err(format!("unnamed {} but named {}", s(x), s(y)))
        };
        while j < b.len() {
            // A doc attribute, in the named print only.
            if let (TokenTree::Punct(p), Some(TokenTree::Group(g))) = (&b[j], b.get(j + 1))
                && p.as_char() == '#'
                && g.delimiter() == Delimiter::Bracket
                && matches!(g.stream().into_iter().next(), Some(TokenTree::Ident(d)) if d == "doc")
                && !matches!(a.get(i), Some(TokenTree::Punct(q)) if q.as_char() == '#')
            {
                j += 2;
                continue;
            }
            // A comment, in the named print only (`--asm-comments`' are
            // in both).
            if let Some(text) = note(&b[j..])
                && note(&a[i.min(a.len())..]).as_ref() != Some(&text)
            {
                self.note(&text, n);
                j += 4;
                continue;
            }
            if let Some((x, off)) = addr(&b[j..]) {
                let want = match names.key_of(&x) {
                    Some((Kind::Data, k)) => k.wrapping_add(off),
                    // A function's key, as a pool word (`addr!(TaskDummy)`).
                    Some((Kind::Func, k)) if off == 0 => k,
                    _ => return Err(format!("addr!({x}) names no datum or function")),
                };
                match a.get(i) {
                    Some(TokenTree::Literal(l)) if literal(&l.to_string()) == Some(want) => {}
                    other => return differ(other, Some(&b[j])),
                }
                n.addrs += 1;
                (i, j) = (i + 1, j + 3);
                continue;
            }
            // An I/O register by name (`crate::io_regs`), where the
            // unnamed print has the `Ptr::<t>::at(lit)` it stands for.
            if matches!(&b[j], TokenTree::Ident(r) if r.to_string().starts_with("REG_")) {
                let Some((len, want)) = io(&a[i..]) else {
                    return differ(a.get(i), Some(&b[j]));
                };
                let got: Vec<String> = b[j..]
                    .iter()
                    .take(want.len())
                    .map(|t| t.to_string())
                    .collect();
                if got != want {
                    return Err(format!(
                        "unnamed `{}` but named `{}`",
                        want.join(" "),
                        got.join(" ")
                    ));
                }
                (i, j) = (i + len, j + want.len());
                continue;
            }
            let Some(x) = a.get(i) else {
                return differ(None, Some(&b[j]));
            };
            if pat_end > 0 && j == pat_end {
                pending = Some(std::mem::take(&mut self.pattern));
            }
            let in_pattern = self.in_pattern || j < pat_end;
            match (x, &b[j]) {
                (TokenTree::Ident(ix), TokenTree::Ident(iy)) if iy == "let" && ix == "let" => {
                    let mut k = j + 1;
                    while k < b.len() && !punct(b.get(k), ':') && !punct(b.get(k), '=') {
                        k += 1;
                    }
                    pat_end = k;
                    self.pattern.clear();
                }
                (TokenTree::Punct(px), TokenTree::Punct(py))
                    if px.as_char() == ';' && py.as_char() == ';' =>
                {
                    if let Some(p) = pending.take() {
                        self.scopes.last_mut().expect("a frame").extend(p);
                    }
                    pat_end = 0;
                }
                (TokenTree::Group(gx), TokenTree::Group(gy))
                    if gx.delimiter() == gy.delimiter() =>
                {
                    let (brace, outer) = (gy.delimiter() == Delimiter::Brace, self.in_pattern);
                    self.in_pattern = in_pattern;
                    if brace {
                        self.scopes.push(Vec::new());
                    }
                    let r = self.walk(gx.stream(), gy.stream(), n);
                    if brace {
                        self.scopes.pop();
                    }
                    self.in_pattern = outer;
                    r?;
                }
                (TokenTree::Ident(ix), TokenTree::Ident(iy))
                    if in_pattern && !["mut", "ref"].contains(&iy.to_string().as_str()) =>
                {
                    let (px, py) = (ix.to_string(), iy.to_string());
                    if px != py && self.local(&px, &py).is_none() {
                        return differ(Some(x), Some(&b[j]));
                    }
                    if let Some(v) = (px != py).then(|| self.local(&px, &py)).flatten() {
                        self.named.insert(v);
                    }
                    self.pattern.push((py, px));
                }
                (TokenTree::Ident(ix), TokenTree::Ident(iy)) => match names.key_of(&iy.to_string())
                {
                    Some((Kind::Func, k)) if *ix == fn_name(k) => n.idents += 1,
                    Some(_) => return differ(Some(x), Some(&b[j])),
                    None => {
                        let (px, py) = (ix.to_string(), iy.to_string());
                        if px != py {
                            match self.local(&px, &py) {
                                Some(v) => {
                                    self.named.insert(v);
                                }
                                None => return differ(Some(x), Some(&b[j])),
                            }
                        }
                        // A field, a path's segment or a macro isn't a
                        // local.
                        let path = j > 0 && (punct(b.get(j - 1), '.') || punct(b.get(j - 1), ':'))
                            || punct(b.get(j + 1), '!')
                            || punct(b.get(j + 1), ':') && punct(b.get(j + 2), ':');
                        if !path && let Err(e) = self.resolve(&px, &py) {
                            let at: Vec<String> =
                                b[j..].iter().take(8).map(|t| t.to_string()).collect();
                            return Err(format!("{e}, at `{}`", at.join(" ")));
                        }
                    }
                },
                (TokenTree::Punct(px), TokenTree::Punct(py))
                    if px.as_char() == py.as_char() && px.spacing() == py.spacing() => {}
                (TokenTree::Literal(lx), TokenTree::Literal(ly))
                    if lx.to_string() == ly.to_string() => {}
                _ => return differ(Some(x), Some(&b[j])),
            }
            (i, j) = (i + 1, j + 1);
        }
        match a.get(i) {
            Some(x) => differ(Some(x), None),
            None => Ok(()),
        }
    }
}

/// `note!("…");` at the start of `t`: its text.
fn note(t: &[TokenTree]) -> Option<String> {
    let [
        TokenTree::Ident(m),
        TokenTree::Punct(bang),
        TokenTree::Group(g),
        TokenTree::Punct(semi),
        ..,
    ] = t
    else {
        return None;
    };
    if m != "note" || bang.as_char() != '!' || semi.as_char() != ';' {
        return None;
    }
    let lit: syn::LitStr = syn::parse2(g.stream()).ok()?;
    Some(lit.value())
}

/// `addr!(X)` or `addr!(X + n)` at the start of `t`: `X` and `n`.
fn addr(t: &[TokenTree]) -> Option<(String, u32)> {
    let [
        TokenTree::Ident(m),
        TokenTree::Punct(bang),
        TokenTree::Group(g),
        ..,
    ] = t
    else {
        return None;
    };
    if m != "addr" || bang.as_char() != '!' || g.delimiter() != Delimiter::Parenthesis {
        return None;
    }
    let inner: Vec<TokenTree> = g.stream().into_iter().collect();
    match &inner[..] {
        [TokenTree::Ident(x)] => Some((x.to_string(), 0)),
        [
            TokenTree::Ident(x),
            TokenTree::Punct(p),
            TokenTree::Literal(o),
        ] if p.as_char() == '+' => Some((x.to_string(), literal(&o.to_string())?)),
        _ => None,
    }
}

/// `Ptr::<t>::at(lit)` at the start of `t`, at an I/O register: its
/// length in tokens, and the tokens the named print has for it (`REG_IME`,
/// `REG_VCOUNT.field::<u8>(0)`).
fn io(t: &[TokenTree]) -> Option<(usize, Vec<String>)> {
    let Some(TokenTree::Group(g)) = t.get(9) else {
        return None;
    };
    let s: Vec<String> = t[..9].iter().map(|t| t.to_string()).collect();
    if s != ["Ptr", ":", ":", "<", &s[4], ">", ":", ":", "at"]
        || g.delimiter() != Delimiter::Parenthesis
    {
        return None;
    }
    let w = match s[4].as_str() {
        "u8" => 1,
        "u16" => 2,
        "u32" => 4,
        _ => return None,
    };
    let x = literal(&g.stream().to_string())?;
    let (reg, off) = crate::io_regs::at(x, w)?;
    let mut want = vec![crate::io_regs::ident(reg)];
    if let Some(o) = off {
        let f = [".", "field", ":", ":", "<", &s[4], ">"];
        want.extend(f.iter().map(|x| x.to_string()));
        want.push(format!("({o})"));
    }
    Some((10, want))
}

/// An integer literal's value: decimal or `0x` hex, with `_`s and an
/// optional `u32` suffix.
fn literal(s: &str) -> Option<u32> {
    let s = s.strip_suffix("u32").unwrap_or(s).replace('_', "");
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// What [`check_rom`] found: functions checked, what was named, and each
/// failure with its function's key.
#[derive(Debug, Default)]
pub struct Report {
    pub funcs: usize,
    pub named: Named,
    /// The DWARF register variables of the readable functions checked
    /// (by identifier: two `i`s of one function are one).
    pub vars: usize,
    pub failed: Vec<(u32, String)>,
}

/// Every function of `rom` and of its IWRAM copies, built with `cfg` as
/// `gbaa-recompile` builds them, checked with `names`. `readable`: the
/// functions printed readable, as `--readable` lists them.
pub fn check_rom(
    rom: &[u8],
    disc: &analysis::discovery::Discovery,
    variants: u32,
    cfg: crate::func::Config,
    names: &SourceMap,
    readable: Option<&BTreeSet<u32>>,
) -> Report {
    use crate::emit::{build_all, ram_code, ram_ctx};
    let mut plain = Ctx::new(rom, disc);
    plain.cfg = cfg;
    plain.readable = readable;
    let r = build_all(&plain);
    let ram = ram_code(rom, disc, variants, cfg);
    let ram_plain = ram_ctx(rom, &ram, cfg);
    let mut rep = Report::default();
    let mut run = |plain: &Ctx, funcs: &[Func]| {
        let named = Ctx {
            rom: plain.rom,
            base: plain.base,
            disc: plain.disc,
            entries: plain.entries.clone(),
            cfg: plain.cfg,
            names: Some(names),
            kept: plain.kept.clone(),
            readable: plain.readable,
            pointed: plain.pointed.clone(),
            spins: plain.spins.clone(),
            library: plain.library.clone(),
            given_jumps: plain.given_jumps.clone(),
        };
        for f in funcs {
            rep.funcs += 1;
            if plain.readable.is_some_and(|r| r.contains(&f.entry)) {
                let ids = crate::dwarf::FnDwarf::idents(names, f.entry);
                rep.vars += ids.iter().collect::<BTreeSet<_>>().len();
            }
            match check_func(plain, &named, f) {
                Ok(n) => rep.named += n,
                Err(e) => rep.failed.push((f.entry, e)),
            }
        }
    };
    run(&plain, &r.funcs);
    for (c, rc) in ram_plain.iter().zip(&ram) {
        run(c, &rc.code.funcs);
    }
    rep
}
