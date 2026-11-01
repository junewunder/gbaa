//! Which functions read or write a struct field, by their readable print
//! (`just mod-find`; `docs/research/agent-first-mod.md`, item 5): what to
//! cut for a change to that field.
//!
//! A field is accessed where the print names it, `….field()`. The access
//! a write is in the `Ptr` argument of `m.set`, `m.set_via`, `m.set_bits`
//! or `m.set_bits_via` (each's second-to-last), every field on that `Ptr`'s
//! path counting; any other mention is a read. The pret line is the last
//! `// file:line` comment before it.

use std::collections::BTreeMap;

use proc_macro2::{Delimiter, TokenStream, TokenTree};

use crate::func::{Ctx, Func};

/// One access to the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// The pret `file:line` the access prints under, if any.
    pub at: Option<String>,
    /// The field's path from its base: `SaveBlock2.optionsTextSpeed`
    /// (the base's type, or the global it starts from).
    pub path: String,
    /// Whether it writes the field.
    pub write: bool,
}

/// One function's accesses to the field, in print order.
#[derive(Debug)]
pub struct Hit {
    pub key: u32,
    pub name: String,
    pub accesses: Vec<Access>,
}

/// Every function of `funcs` whose readable print in `ctx` names `field`.
pub fn find(ctx: &Ctx, funcs: &[Func], field: &str) -> Vec<Hit> {
    funcs
        .iter()
        .filter_map(|f| {
            let (tokens, _) = crate::emit::print_fn(ctx, f, false);
            let accesses = accesses(tokens, field);
            (!accesses.is_empty()).then(|| Hit {
                key: f.entry,
                name: ctx.fn_ident(f.entry).to_string(),
                accesses,
            })
        })
        .collect()
}

/// The accesses to `field` in a printed function's `tokens`.
pub fn accesses(tokens: TokenStream, field: &str) -> Vec<Access> {
    let mut out = Vec::new();
    scan(tokens, field, false, &mut None, &mut out);
    out
}

const WRITES: [&str; 4] = ["set", "set_via", "set_bits", "set_bits_via"];

fn scan(ts: TokenStream, field: &str, write: bool, at: &mut Option<String>, out: &mut Vec<Access>) {
    let tt: Vec<TokenTree> = ts.into_iter().collect();
    let ident = |i: usize, s: &str| matches!(tt.get(i), Some(TokenTree::Ident(x)) if x == s);
    let punct =
        |i: usize, c: char| matches!(tt.get(i), Some(TokenTree::Punct(p)) if p.as_char() == c);
    let paren = |i: usize| match tt.get(i) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => Some(g.stream()),
        _ => None,
    };
    let mut i = 0;
    while i < tt.len() {
        // `note!("src/new_game.c:93")`: the pret line from here on.
        if ident(i, "note")
            && punct(i + 1, '!')
            && let Some(s) = paren(i + 2).and_then(|s| syn::parse2::<syn::LitStr>(s).ok())
        {
            if is_line(&s.value()) {
                *at = Some(s.value());
            }
            i += 3;
            continue;
        }
        // `m.set(at, p, v)`: `p` is written.
        if ident(i, "m")
            && punct(i + 1, '.')
            && WRITES.iter().any(|w| ident(i + 2, w))
            && let Some(args) = paren(i + 3)
        {
            let args = split_args(args);
            let n = args.len();
            for (j, a) in args.into_iter().enumerate() {
                scan(a, field, n >= 2 && j == n - 2, at, out);
            }
            i += 4;
            continue;
        }
        if punct(i, '.') && ident(i + 1, field) && paren(i + 2).is_some_and(|s| s.is_empty()) {
            out.push(Access {
                at: at.clone(),
                path: path(&tt, i),
                write,
            });
            i += 3;
            continue;
        }
        if let TokenTree::Group(g) = &tt[i] {
            // Inside a group (an index, `at(..)`'s address), a read.
            scan(g.stream(), field, false, at, out);
        }
        i += 1;
    }
}

/// Whether a comment is a pret line, `file:line`.
fn is_line(s: &str) -> bool {
    s.rsplit_once(':').is_some_and(|(f, n)| {
        !f.is_empty() && !f.contains(' ') && f.contains('.') && n.parse::<u32>().is_ok()
    })
}

/// `ts` split at its top-level commas.
fn split_args(ts: TokenStream) -> Vec<TokenStream> {
    let mut args = vec![TokenStream::new()];
    for t in ts {
        match &t {
            TokenTree::Punct(p) if p.as_char() == ',' => args.push(TokenStream::new()),
            _ => args.last_mut().unwrap().extend([t]),
        }
    }
    if args.last().is_some_and(|a| a.is_empty()) {
        args.pop();
    }
    args
}

/// The path to the field accessed at `tt[dot]` (its `.`): the fields before
/// it back to the base, `Ptr::<T>::at(..)`'s `T` or a global's name.
fn path(tt: &[TokenTree], dot: usize) -> String {
    let field = tt[dot + 1].to_string();
    let mut steps = vec![field];
    let mut i = dot;
    // Back over `.name(..)` steps; `.idx(..)` shows as `[]`.
    while i >= 3 {
        match (&tt[i - 3], &tt[i - 2], &tt[i - 1]) {
            (TokenTree::Punct(p), TokenTree::Ident(n), TokenTree::Group(_))
                if p.as_char() == '.' =>
            {
                steps.push(if n == "idx" {
                    "[]".into()
                } else {
                    n.to_string()
                });
                i -= 3;
            }
            _ => break,
        }
    }
    // The base: `Ptr::<T>::at(..)` or a name.
    let is_ident = |j: usize, s: &str| matches!(&tt[j], TokenTree::Ident(x) if x == s);
    let base = if i >= 2 && is_ident(i - 2, "at") && matches!(tt[i - 1], TokenTree::Group(_)) {
        // `Ptr :: < T > :: at (..)`: `T` is what's between.
        match tt[..i - 2]
            .iter()
            .rposition(|t| matches!(t, TokenTree::Ident(x) if x == "Ptr"))
        {
            Some(p) if p + 4 <= i - 5 => tt[p + 4..i - 5].iter().map(|t| t.to_string()).collect(),
            _ => "?".to_string(),
        }
    } else if i >= 1 && matches!(tt[i - 1], TokenTree::Ident(_)) {
        tt[i - 1].to_string()
    } else {
        "?".to_string()
    };
    steps.push(base);
    steps.reverse();
    steps.join(".").replace(".[]", "[]")
}

/// `hits` as `just mod-find` prints them: writers, then readers, a line a
/// function, each access by pret line and path.
pub fn report(field: &str, hits: &[Hit], of: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let side = |write: bool| -> Vec<(&Hit, Vec<&Access>)> {
        hits.iter()
            .map(|h| {
                (
                    h,
                    h.accesses
                        .iter()
                        .filter(|a| a.write == write)
                        .collect::<Vec<_>>(),
                )
            })
            .filter(|(_, a)| !a.is_empty())
            .collect()
    };
    let (w, r) = (side(true), side(false));
    let _ = writeln!(
        s,
        "{field}: {} writers, {} readers, of {of} functions printed readable",
        w.len(),
        r.len()
    );
    for (title, list) in [("writers", w), ("readers", r)] {
        let _ = writeln!(s, "{title}:");
        for (h, accs) in list {
            // Each pret line once, with the paths it names.
            let mut lines: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for a in &accs {
                let ps = lines
                    .entry(a.at.as_deref().unwrap_or("no line"))
                    .or_default();
                if !ps.contains(&a.path.as_str()) {
                    ps.push(&a.path);
                }
            }
            let lines: Vec<String> = lines
                .into_iter()
                .map(|(l, ps)| format!("{l} {}", ps.join(" ")))
                .collect();
            let _ = writeln!(s, "  {} (0x{:08x}): {}", h.name, h.key, lines.join(", "));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn writes_reads_and_lines() {
        let t = quote! {
            pub fn SetDefaultOptions(m: &mut M, mut pc: u32) -> Flow {
                note!("src/new_game.c:93");
                let v7: u32 = m.get_via(0x0808_433c, 0x0808_438c, 0x0808_433e, gSaveBlock2Ptr);
                m.set_bits(0x0808_4340, 0x0808_434c, Ptr::<SaveBlock2>::at(v7).optionsTextSpeed(), OPTIONS_TEXT_SPEED_MID);
                note!("src/new_game.c:94");
                let v31: u32 = m.get(0x0808_4350, Ptr::<SaveBlock2>::at(v27).optionsTextSpeed());
                note!("calls Foo(1)");
                if v31 != 0 {
                    m.set(0x0808_4356, Ptr::<Big>::at(v1).inner().idx(v2).optionsTextSpeed(), gThing.optionsTextSpeed());
                }
            }
        };
        let a = accesses(t, "optionsTextSpeed");
        let line = |s: &str| Some(s.to_string());
        assert_eq!(
            a,
            [
                Access {
                    at: line("src/new_game.c:93"),
                    path: "SaveBlock2.optionsTextSpeed".into(),
                    write: true
                },
                Access {
                    at: line("src/new_game.c:94"),
                    path: "SaveBlock2.optionsTextSpeed".into(),
                    write: false
                },
                Access {
                    at: line("src/new_game.c:94"),
                    path: "Big.inner[].optionsTextSpeed".into(),
                    write: true
                },
                Access {
                    at: line("src/new_game.c:94"),
                    path: "gThing.optionsTextSpeed".into(),
                    write: false
                },
            ]
        );
    }
}
