//! DWARF names for a lifted function's register locals and parameters
//! (unit `native/lift-local-names`; `docs/mod-view-design.md`, "Naming
//! locals: only where liveness agrees"), from the names files' `[locals]`
//! and prototypes.
//!
//! - A register's local `rN` takes a variable's name when the variable
//!   is in rN wherever the function touches rN: every read and write of
//!   rN (but a `push`'s save of the caller's value and a `pop`'s restore)
//!   is in that variable's ranges and in no other variable's of rN, every
//!   block rN is live into starts in them, the value rN comes in with is
//!   read only by a `push` (but a parameter's, in r0-r3), and every
//!   value written to the local fits the
//!   variable's C type ([`crate::dwarf::fits`]: agbcc keeps a variable in
//!   one register for its whole scope, so a register it reuses there for
//!   a temporary shows only in its type).
//! - A parameter register the prototype names takes that name where its
//!   local is never written: it is the argument throughout. A plain
//!   entry's `aN` argument (a body behind `Regs::call`) takes it too.
//! - Any other variable in a register the function touches gets a
//!   comment where its range starts, after its block's `pc`:
//!   `// dwarf r0: fixedPersonality`. Comments, so `lines` leaves them out
//!   ([`LOCAL_NOTE`]); the metrics count names (`locals_named`) and
//!   comments (`locals_noted`).
//!
//! A name is an identifier change over the function's text, never inside
//! a string or a comment, and only to a word the function's text doesn't
//! already have (so nothing it calls or reads is shadowed). The adapter
//! `a_<key>` keeps its registers.

use crate::func::Func;
use crate::names::{Home, SourceMap};
use analysis::ir::Op;
use std::collections::{BTreeMap, BTreeSet};

/// The start of a register variable's comment: lines starting so are
/// left out of `lines`.
pub const LOCAL_NOTE: &str = "// dwarf ";

/// What one function's text takes from DWARF ([`plan`]).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Naming {
    /// Words of the printed text and their names.
    pub renames: BTreeMap<String, String>,
    /// Comments by block key: printed after the block's `pc`.
    pub notes: BTreeMap<u32, Vec<String>>,
    /// Variables and parameters named.
    pub named: usize,
}

/// What liveness says of one function, as the printer has it
/// ([`super::FnPrinter::dead_writes`]).
pub struct Facts<'a> {
    pub f: &'a Func,
    /// Writes left out, by block and inst.
    pub dead: &'a BTreeMap<u32, Vec<bool>>,
    /// Writes printed as `let`s of their own, by block and inst.
    pub local: &'a BTreeMap<u32, Vec<Option<super::live::Local>>>,
    /// What is live at each block's start.
    pub live_in: &'a BTreeMap<u32, super::live::Live>,
}

/// One register's accesses in a function: the guest address of each read
/// and write that is the variable's (not a save or restore), whether rN
/// is live into a block (its start address), and the values printed
/// writes put in its local, each fitting a type or not.
#[derive(Default)]
struct Uses {
    at: Vec<u32>,
    live_into: Vec<(u32, bool)>,
    /// The value rN has at entry may be read but by a `push`: something
    /// in the entry block reads it before writing rN, or nothing there
    /// writes rN (it goes on into another block).
    entry_read: bool,
    /// (block key, inst) of each write printed to the local.
    writes: Vec<(u32, usize)>,
}

fn uses(facts: &Facts, r: usize) -> Uses {
    let mut u = Uses::default();
    let bit = super::live::reg(r);
    for (&k, gb) in &facts.f.blocks {
        let insts = &gb.block.insts;
        if facts.live_in.get(&k).is_some_and(|l| l & bit != 0) {
            u.live_into.push((k & !1, k == facts.f.entry));
        }
        // An `ldm`'s or `stm`'s accesses (a `push`, a `pop`).
        let mut multiple = vec![false; insts.len()];
        let mut m = false;
        for (i, inst) in insts.iter().enumerate() {
            match inst.op {
                Op::Insn(_) => m = false,
                Op::Timing(analysis::ir::Timing::Block(_)) => m = true,
                _ => {}
            }
            multiple[i] = m;
        }
        // Reads only a `push` stores: the caller's value saved.
        let mut stored = vec![false; insts.len()];
        let mut used = vec![false; insts.len()];
        for (i, inst) in insts.iter().enumerate() {
            for o in inst.op.operands().into_iter().chain(inst.guard) {
                match inst.op {
                    Op::Store { value, addr, .. } if multiple[i] && o == value && o != addr => {
                        stored[o.index()] = true
                    }
                    _ => used[o.index()] = true,
                }
            }
        }
        if k == facts.f.entry {
            u.entry_read = true;
            for (i, inst) in insts.iter().enumerate() {
                match inst.op {
                    Op::GetReg(q) if q.index() == r && (used[i] || !stored[i]) => break,
                    Op::SetReg(q, _) if q.index() == r && inst.guard.is_none() => {
                        u.entry_read = false;
                        break;
                    }
                    _ => {}
                }
            }
        }
        let mut cur = k & !1;
        for (i, inst) in insts.iter().enumerate() {
            match inst.op {
                Op::Insn(a) => cur = a,
                Op::GetReg(q) if q.index() == r && (used[i] || !stored[i]) => u.at.push(cur),
                Op::SetReg(q, v) if q.index() == r => {
                    // A `pop`'s restore of the caller's value.
                    let restore =
                        multiple[v.index()] && matches!(insts[v.index()].op, Op::Load { .. });
                    if !restore {
                        u.at.push(cur);
                    }
                    let dead = facts.dead.get(&k).and_then(|d| d.get(i).copied());
                    let local = facts.local.get(&k).and_then(|l| l.get(i)).cloned();
                    if dead != Some(true) && local.flatten().is_none() && !restore {
                        u.writes.push((k, i));
                    }
                }
                _ => {}
            }
        }
    }
    u
}

/// Whether `w` is one of the printer's own names: `r4`, `v12`, `l8`,
/// `a0`, `f_08000101` and the like, `g` and `sp`.
fn printers(w: &str) -> bool {
    let b = w.as_bytes();
    let digits = |r: &[u8]| !r.is_empty() && r.iter().all(u8::is_ascii_digit);
    let hex = |r: &[u8]| !r.is_empty() && r.iter().all(u8::is_ascii_hexdigit);
    match b {
        [b'f' | b'x' | b'a', b'_', rest @ ..] => hex(rest),
        [b'r' | b'v' | b'l' | b'a' | b'b', rest @ ..] => digits(rest),
        _ => ["g", "sp"].contains(&w),
    }
}

/// The words of a function's printed text that a name must not be: all
/// but those of a one-line entry that only calls another (`x_` calling
/// the body in `f_` with `s: Regs`), which is a function of its own and
/// has no register local or argument to rename.
pub fn words_of(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for l in text.lines() {
        let ws = super::rust_words(l);
        let own = |w: &String| {
            let b = w.as_bytes();
            matches!(b, [b'r' | b'a', d @ ..] if !d.is_empty() && d.iter().all(u8::is_ascii_digit))
        };
        let one_line = l.starts_with("pub fn ") && l.trim_end().ends_with('}');
        if !one_line || ws.iter().any(own) {
            out.extend(ws);
        }
    }
    out
}

/// The names for function `f`'s text. `words`: every word of its printed
/// text (entries included); `written`: the words it assigns. `params`:
/// its argument registers (bit `n` for rN).
pub fn plan(
    names: &SourceMap,
    facts: &Facts,
    words: &BTreeSet<String>,
    written: &BTreeSet<String>,
) -> Naming {
    let f = facts.f;
    let mut out = Naming::default();
    let locals = names.locals.get(&f.entry).map_or(&[][..], Vec::as_slice);
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let ok = |id: &str, taken: &BTreeSet<String>| {
        !words.contains(id)
            && !taken.contains(id)
            && !printers(id)
            && syn::parse_str::<syn::Ident>(id).is_ok()
    };
    let mut noted: BTreeSet<(usize, u8)> = BTreeSet::new();
    for r in 0..13u8 {
        let reg = format!("r{r}");
        let vars: Vec<usize> = (0..locals.len())
            .filter(|&n| locals[n].home == Home::Reg(r))
            .collect();
        if vars.is_empty() {
            continue;
        }
        let u = uses(facts, usize::from(r));
        if u.at.is_empty() && u.live_into.is_empty() {
            // The function never touches rN: nothing to name.
            continue;
        }
        let holds = |n: usize, a: u32| locals[n].from <= a && a < locals[n].to;
        // The variables of rN at its accesses, by name.
        let mut at_names: BTreeSet<&str> = BTreeSet::new();
        let mut outside = false;
        for &a in &u.at {
            let here: Vec<usize> = vars.iter().copied().filter(|&n| holds(n, a)).collect();
            if here.is_empty() {
                outside = true;
            }
            at_names.extend(here.iter().map(|&n| locals[n].name.as_str()));
        }
        let mut chosen = None;
        if let (false, [name]) = (outside, &at_names.iter().copied().collect::<Vec<_>>()[..]) {
            let mine: Vec<usize> = vars
                .iter()
                .copied()
                .filter(|&n| locals[n].name == *name)
                .collect();
            let param = mine.iter().any(|&n| locals[n].param);
            // The value rN comes in with is the variable's only for a
            // parameter in an argument register; else only a `push`
            // may read it (it saves the caller's value).
            let entry_ok = !u.entry_read || (param && r < 4);
            let live_ok = entry_ok
                && u.live_into
                    .iter()
                    .all(|&(a, _)| mine.iter().any(|&n| holds(n, a)));
            let ty = locals[mine[0]].ty.as_deref();
            let types_ok = u.writes.iter().all(|&(k, i)| {
                let insts = &f.blocks[&k].block.insts;
                let Op::SetReg(_, v) = insts[i].op else {
                    return true;
                };
                let address = insts.iter().any(
                    |x| matches!(x.op, Op::Load { addr, .. } | Op::Store { addr, .. } if addr == v),
                );
                crate::dwarf::fits(ty, insts, v.index(), address)
            });
            if live_ok
                && types_ok
                && words.contains(&reg)
                && let Some(id) = names.lift_local_ident(name)
                && ok(&id, &taken)
            {
                chosen = Some(id);
            }
        }
        match chosen {
            Some(id) => {
                taken.insert(id.clone());
                out.renames.insert(reg, id);
                out.named += 1;
            }
            None => {
                // A comment where each variable's range starts.
                for &n in &vars {
                    let l = &locals[n];
                    if !u.at.iter().any(|&a| holds(n, a)) || !noted.insert((n, r)) {
                        continue;
                    }
                    let Some(k) = block_at(f, l.from, l.to) else {
                        continue;
                    };
                    out.notes
                        .entry(k)
                        .or_default()
                        .push(format!("{LOCAL_NOTE}r{r}: {}", l.name));
                }
            }
        }
    }
    // Parameters by the prototype: a register local never written, and
    // a plain entry's `aN`.
    // The C names: the prototype's are the readable form's identifiers.
    let raw = names
        .functions
        .get(&f.entry)
        .and_then(|i| crate::dwarf::proto_params(i.proto.as_deref()?, &i.name))
        .map(|(_, ps)| ps)
        .unwrap_or_default();
    if let Some(p) = names.typed.proto_of(f.entry) {
        for (r, a) in p.args.iter().enumerate().take(4) {
            let Some((_, _)) = a else { continue };
            let Some(Some(name)) = raw.get(r).map(|(n, _)| n.as_deref()) else {
                continue;
            };
            let Some(id) = names.lift_local_ident(name) else {
                continue;
            };
            let reg = format!("r{r}");
            let arg = format!("a{r}");
            let mut any = false;
            if !out.renames.contains_key(&reg)
                && words.contains(&reg)
                && !written.contains(&reg)
                && ok(&id, &taken)
            {
                out.renames.insert(reg, id.clone());
                any = true;
            }
            // Its own name, or the one rN took, may name aN too.
            if words.contains(&arg) && (any || ok(&id, &taken)) {
                out.renames.insert(arg, id.clone());
                any = true;
            }
            if any {
                taken.insert(id);
                out.named += 1;
            }
        }
    }
    out
}

/// The block where a range `[from, to)` starts: the one holding `from`,
/// else the first that starts inside the range.
fn block_at(f: &Func, from: u32, to: u32) -> Option<u32> {
    f.blocks
        .iter()
        .find(|(k, gb)| *k & !1 <= from && from < gb.end)
        .or_else(|| {
            f.blocks
                .iter()
                .find(|(k, _)| (from..to).contains(&(*k & !1)))
        })
        .map(|(&k, _)| k)
}

/// `text` with each whole word of `renames` replaced, but in strings,
/// char literals and `//` comments, and after a `.` (a field or method)
/// or a `'` (a label).
pub fn rename(text: &str, renames: &BTreeMap<String, String>) -> String {
    if renames.is_empty() {
        return text.to_string();
    }
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            // A string to its closing quote.
            let s = i;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            out.push_str(&text[s..i]);
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let e = text[i..].find('\n').map_or(b.len(), |n| i + n);
            out.push_str(&text[i..e]);
            i = e;
        } else if word(c) {
            let s = i;
            while i < b.len() && word(b[i]) {
                i += 1;
            }
            let w = &text[s..i];
            let after_dot_or_tick = s > 0 && matches!(b[s - 1], b'.' | b'\'');
            match renames.get(w) {
                Some(n) if !after_dot_or_tick && !b[s].is_ascii_digit() => out.push_str(n),
                _ => out.push_str(w),
            }
        } else {
            let n = text[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&text[i..i + n]);
            i += n;
        }
    }
    out
}

/// `text` with each block's comments after its first `pc(0x…);` line, at
/// that line's indent. Comments whose block prints no `pc` are dropped;
/// gives how many went in.
pub fn insert_notes(text: &str, notes: &BTreeMap<u32, Vec<String>>) -> (String, usize) {
    let mut by_line: BTreeMap<usize, &Vec<String>> = BTreeMap::new();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for (k, ns) in notes {
        let pc = format!("pc(0x{k:08x});");
        if let Some(i) = lines.iter().position(|l| l.trim() == pc) {
            by_line.insert(i, ns);
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut n = 0;
    for (i, l) in lines.iter().enumerate() {
        out.push_str(l);
        if let Some(ns) = by_line.get(&i) {
            let ind = &l[..l.len() - l.trim_start().len()];
            if !l.ends_with('\n') {
                out.push('\n');
            }
            for c in *ns {
                out.push_str(ind);
                out.push_str(c);
                out.push('\n');
                n += 1;
            }
        }
    }
    (out, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_words_only() {
        let m: BTreeMap<String, String> = [("r4".to_string(), "p".to_string())].into();
        let t = "let v1: u32 = r4; s.r4 = r40; // r4 here\npanic!(\"r4\"); 'r4: loop { r4 += 1 }";
        assert_eq!(
            rename(t, &m),
            "let v1: u32 = p; s.r4 = r40; // r4 here\npanic!(\"r4\"); 'r4: loop { p += 1 }"
        );
    }

    #[test]
    fn notes_after_pc() {
        let t = "fn f() {\n    pc(0x08000101);\n    r0 = 1;\n        pc(0x08000105);\n}\n";
        let notes: BTreeMap<u32, Vec<String>> =
            [(0x0800_0105, vec![format!("{LOCAL_NOTE}r0: x")])].into();
        let (s, n) = insert_notes(t, &notes);
        assert_eq!(n, 1);
        assert!(s.contains("        pc(0x08000105);\n        // dwarf r0: x\n}"));
    }

    #[test]
    fn printers_names() {
        for w in [
            "r4",
            "v12",
            "l8",
            "a0",
            "f_08000101",
            "x_08000101",
            "g",
            "sp",
        ] {
            assert!(printers(w), "{w}");
        }
        for w in ["p", "row", "rx", "value", "x"] {
            assert!(!printers(w), "{w}");
        }
    }
}
