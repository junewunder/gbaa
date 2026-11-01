//! DWARF names, lines and call values in the readable form
//! (`docs/mod-view-design.md`, "The readable form" and "Naming locals"):
//! from the source map's `[locals]`, `[lines]` and prototypes, what one
//! readable block prints besides its code.
//!
//! - A value takes a local's name where DWARF and liveness agree: every
//!   register that holds it holds the same variable, from its definition
//!   to its last use (to the block's end, if a register carries it out).
//!   agbcc gives a variable one location for its whole function or
//!   lexical block, so its `[from, to)` is that scope.
//! - A register the block reads, in a variable's scope, that no value of
//!   the block names: `// r4: keyInput` at the block's start.
//! - A load or store at `sp + slot` after the prologue, a stack variable's
//!   slot: `// sp+8: bytesTransferred` before its instruction.
//! - Each `[lines]` row's `// src/item.c:123` before its instruction, and
//!   the line in effect at the block's first.
//! - A call to a function with a prototype: `// r0 = Random(): u16`, the
//!   arguments by register (`// SetMonData(mon: r0, field: r1, data:
//!   r2)` for a `void` one).
//!
//! The comments are `note!("…")` statements, which [`crate::emit::pretty`]
//! prints as `// …` (and which expand to nothing).

use crate::func::GBlock;
use crate::names::{Home, Local, SourceMap};
use analysis::ir::{BinOp, Op, ValueId};
use armv4t::sem::Reg;
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use std::collections::BTreeMap;

/// One function's DWARF, as the readable form prints it.
pub struct FnDwarf<'a> {
    names: &'a SourceMap,
    /// Its source file, for a `[lines]` row that gives none.
    src: Option<&'a str>,
    locals: &'a [Local],
    /// Its first address, below which no line row is its.
    start: u32,
    /// Its prototype, as far as its registers.
    proto: Option<&'a crate::types::Proto>,
    /// Nothing jumps back to its entry block ([`FnDwarf::entered_once`]).
    once: bool,
}

/// What one readable block prints from DWARF ([`FnDwarf::block`]).
#[derive(Default)]
pub struct BlockNames {
    /// Each inst's value's name, where it has one.
    pub values: Vec<Option<Ident>>,
    /// Comments to print before each inst.
    pub notes: BTreeMap<usize, Vec<String>>,
    /// Each register read in a variable's scope: the read's inst, the
    /// inst to print its comment before (`r4: keyInput`), and the
    /// comment.
    pub reads: Vec<(usize, usize, String)>,
}

/// A `note!("…")` statement: a comment in the printed text.
pub fn note(text: &str) -> TokenStream {
    quote!(note!(#text);)
}

impl<'a> FnDwarf<'a> {
    /// The function at `entry`'s DWARF, if the map has any for it.
    pub fn of(names: Option<&'a SourceMap>, entry: u32) -> Option<FnDwarf<'a>> {
        let names = names?;
        let item = names.functions.get(&entry);
        let locals = names.locals.get(&entry).map_or(&[][..], Vec::as_slice);
        if item.is_none() && locals.is_empty() {
            return None;
        }
        Some(FnDwarf {
            names,
            src: item.and_then(|i| i.src.as_deref()),
            locals,
            start: entry & !1,
            proto: names.typed.proto_of(entry),
            once: false,
        })
    }

    /// Prints its parameters only if `once`: nothing jumps back to its
    /// entry block, so the block's reads of r0–r3 are the arguments.
    pub fn entered_once(mut self, once: bool) -> FnDwarf<'a> {
        self.once = once;
        self
    }

    /// The entry block's parameters by argument register, as
    /// [`crate::print::Printer::args`] takes them.
    pub fn args(&self) -> Vec<Option<(Ident, Option<crate::types::Ty>)>> {
        let Some(p) = self.proto.filter(|_| self.once) else {
            return Vec::new();
        };
        p.args
            .iter()
            .map(|a| a.as_ref().map(|(n, t)| (format_ident!("{n}"), t.clone())))
            .collect()
    }

    /// Whether its returns print the value it returns, r0.
    pub fn returns(&self) -> bool {
        self.proto.is_some_and(|p| p.returns)
    }

    /// The `[lines]` row at `a` exactly, as `file:line`.
    fn row(&self, a: u32) -> Option<String> {
        self.text(self.names.lines.get(&a)?)
    }

    /// The `[lines]` row in effect at `a`: the last at or before it in
    /// the function. None below the entry (a block placed before it): the
    /// row before `a` there may be another function's.
    fn in_effect(&self, a: u32) -> Option<String> {
        if a < self.start {
            return None;
        }
        let (_, l) = self.names.lines.range(self.start..=a).next_back()?;
        self.text(l)
    }

    fn text(&self, l: &crate::names::Line) -> Option<String> {
        let file = l.file.as_deref().or(self.src)?;
        Some(format!("{file}:{}", l.line))
    }

    /// The innermost variable at `home` whose scope holds `a`.
    fn var(&self, home: Home, a: u32) -> Option<usize> {
        (0..self.locals.len())
            .filter(|&n| {
                let l = &self.locals[n];
                l.home == home && l.from <= a && a < l.to
            })
            .min_by_key(|&n| self.locals[n].to - self.locals[n].from)
    }

    /// Whether inst `i`'s value can be variable `n` by its C type, where
    /// agbcc's one register for the whole scope can't say (a register
    /// reused in it): a pointer isn't a narrow load or a shift, an integer
    /// isn't an address as it is, and a narrow one isn't the `x << 16`
    /// half of its zero or sign extension. `address`: an access uses it
    /// as its address.
    fn fits(&self, n: usize, insts: &[analysis::ir::Inst], i: usize, address: &bool) -> bool {
        let Some(ty) = self.locals[n].ty.as_deref() else {
            return true;
        };
        let pointer = ty.contains('*') || ty.contains('[');
        let shift = |k: u32| match insts[i].op {
            Op::Shift { kind, amount, .. } => {
                kind == armv4t::sem::ShiftType::Lsl
                    && matches!(insts[amount.index()].op, Op::Const(c) if c == k)
            }
            _ => false,
        };
        if pointer {
            let narrow =
                matches!(insts[i].op, Op::Load { width, .. } if width != armv4t::sem::Width::Word);
            return !narrow && !matches!(insts[i].op, Op::Shift { .. });
        }
        if *address {
            return false;
        }
        let bits = if ty.contains("char") || ["u8", "s8", "bool8"].contains(&ty) {
            8
        } else if ty.contains("short") || ["u16", "s16"].contains(&ty) {
            16
        } else {
            32
        };
        bits == 32 || !shift(32 - bits)
    }

    /// Whether variable `n`'s scope holds `a`.
    fn holds(&self, n: usize, a: u32) -> bool {
        let l = &self.locals[n];
        l.from <= a && a < l.to
    }

    /// What block `gb` prints from DWARF. `entry`: it is the function's
    /// entry block, whose prologue sets the `sp` the slots are from.
    pub fn block(&self, gb: &GBlock, entry: bool) -> BlockNames {
        let insts = &gb.block.insts;
        let n = insts.len();
        let mut out = BlockNames {
            values: vec![None; n],
            notes: BTreeMap::new(),
            reads: Vec::new(),
        };
        // Each inst's instruction address, and its first inst's index.
        let mut at = vec![gb.key & !1; n];
        let mut cur = gb.key & !1;
        let mut first = None;
        for (i, inst) in insts.iter().enumerate() {
            if let Op::Insn(a) = inst.op {
                cur = a;
                first.get_or_insert(i);
            }
            at[i] = cur;
        }
        let last = gb.insns.last().map_or(gb.key & !1, |(a, _)| *a);
        let mut note = |i: usize, s: String| out.notes.entry(i).or_default().push(s);

        // Lines.
        let mut shown = None;
        for (i, inst) in insts.iter().enumerate() {
            let Op::Insn(a) = inst.op else { continue };
            let l = match (Some(i) == first, self.row(a)) {
                (_, Some(l)) => Some(l),
                (true, None) => self.in_effect(a),
                (false, None) => None,
            };
            if let Some(l) = l
                && shown.as_ref() != Some(&l)
            {
                note(i, l.clone());
                shown = Some(l);
            }
        }

        // Each value's uses' addresses, and whether a register carries it
        // out of the block.
        let mut last_use: Vec<Option<u32>> = vec![None; n];
        let mut held: Vec<Vec<(usize, u32)>> = vec![Vec::new(); n];
        for (i, inst) in insts.iter().enumerate() {
            for o in inst.op.operands().into_iter().chain(inst.guard) {
                let u = &mut last_use[o.index()];
                *u = Some(u.map_or(at[i], |x| x.max(at[i])));
            }
            if let Op::SetReg(r, v) = inst.op {
                held[v.index()].push((r.index(), at[i]));
            }
        }
        for (i, inst) in insts.iter().enumerate() {
            if let Op::GetReg(r) = inst.op {
                held[i].push((r.index(), at[i]));
            }
        }
        // The values an access uses as its address as it is; and those
        // no variable is: an `ldm`'s or `pop`'s loads (saved registers,
        // the return address) and an exit's target. `multiple`: an
        // `ldm`'s or `stm`'s access.
        let mut address = vec![false; n];
        let mut never = vec![false; n];
        let mut multiple = vec![false; n];
        let mut ldm = false;
        for (i, inst) in insts.iter().enumerate() {
            multiple[i] = ldm;
            match inst.op {
                Op::Insn(_) => ldm = false,
                Op::Timing(analysis::ir::Timing::Block(_)) => ldm = true,
                Op::Load { .. } if ldm => never[i] = true,
                Op::Exit(
                    analysis::ir::Exit::Jump(t)
                    | analysis::ir::Exit::JumpExchange(t)
                    | analysis::ir::Exit::ExceptionReturn(t),
                ) => never[t.index()] = true,
                _ => {}
            }
            if let Op::Load { addr, .. } | Op::Store { addr, .. } = inst.op {
                address[addr.index()] = true;
            }
        }
        let mut named: Vec<Option<usize>> = vec![None; n];
        for i in 0..n {
            if held[i].is_empty() || never[i] || crate::print::is_bit(&insts[i].op) {
                continue;
            }
            let carried = held[i]
                .iter()
                .any(|&(r, _)| r < 13 && !matches!(insts[i].op, Op::GetReg(q) if q.index() == r));
            let vars: Vec<Option<usize>> = held[i]
                .iter()
                .map(|&(r, a)| (r < 13).then(|| self.var(Home::Reg(r as u8), a)).flatten())
                .collect();
            let Some(Some(v)) = vars.first().copied() else {
                continue;
            };
            let end = match (carried, last_use[i]) {
                (true, _) => Some(last),
                (false, u) => u,
            };
            if vars.iter().all(|&x| x == Some(v))
                && self.fits(v, insts, i, &address[i])
                && self.holds(v, at[i])
                && end.is_none_or(|e| self.holds(v, e))
            {
                named[i] = Some(v);
            }
        }
        // A name another value of it still lives under when it is
        // defined takes its value's number: `i_v12`. A value a register
        // carries lives to the block's end: its sync is at the exits.
        // A value printed inside a later one's expression is used where
        // that one is (a sync block's at the exits): so, backwards, the
        // last of its users' own last uses.
        let mut uses: Vec<usize> = (0..n).collect();
        for i in (0..n).rev() {
            let inst = &insts[i];
            let end = match inst.op {
                Op::SetReg(..) | Op::SetFlag(..) | Op::YieldFlag(..) => n,
                _ => uses[i],
            };
            for o in inst.op.operands().into_iter().chain(inst.guard) {
                uses[o.index()] = uses[o.index()].max(end);
            }
        }
        let mut live: BTreeMap<String, usize> = BTreeMap::new();
        for i in 0..n {
            let Some(v) = named[i] else { continue };
            let Some(id) = self.names.local_ident(&self.locals[v].name) else {
                continue;
            };
            let id = match live.get(&id) {
                Some(&e) if e > i => format!("{id}_v{i}"),
                _ => id,
            };
            live.insert(id.clone(), uses[i]);
            out.values[i] = Some(format_ident!("{id}"));
        }

        // Registers read in a variable's scope: their comments, which
        // the printer keeps where the read doesn't print by its name.
        // Not a read only an `stm`'s or `push`'s stores use: it saves
        // the caller's value, not the variable.
        let mut stored = vec![false; n];
        let mut used = vec![false; n];
        for (i, inst) in insts.iter().enumerate() {
            for o in inst.op.operands().into_iter().chain(inst.guard) {
                match inst.op {
                    Op::Store { value, addr, .. } if multiple[i] && o == value && o != addr => {
                        stored[o.index()] = true;
                    }
                    _ => used[o.index()] = true,
                }
            }
        }
        for (i, inst) in insts.iter().enumerate() {
            let Op::GetReg(r) = inst.op else { continue };
            if stored[i] && !used[i] {
                continue;
            }
            let r = r.index();
            if let Some(v) = (r < 13)
                .then(|| self.var(Home::Reg(r as u8), at[i]))
                .flatten()
            {
                let text = format!("r{r}: {}", self.locals[v].name);
                out.reads.push((i, first.unwrap_or(0), text));
            }
        }

        // Stack slots: accesses through `sp` while it is the prologue's.
        let sp = sp_offsets(insts);
        let sets: Vec<(usize, i64)> = insts
            .iter()
            .enumerate()
            .filter_map(|(i, x)| match x.op {
                Op::SetReg(r, v) if r == Reg::SP => Some((i, sp[v.index()]?)),
                _ => None,
            })
            .collect();
        let window = if entry {
            // From the prologue's last `sp` write (its lowest) to the
            // next write.
            sets.iter()
                .min_by_key(|&&(i, o)| (o, std::cmp::Reverse(i)))
                .map(|&(i, o)| {
                    let to = sets.iter().find(|&&(j, _)| j > i).map_or(n, |&(j, _)| j);
                    (i, to, o)
                })
        } else {
            Some((0, sets.first().map_or(n, |&(j, _)| j), 0))
        };
        if let Some((from, to, base)) = window {
            let mut said: Vec<(u32, usize)> = Vec::new();
            for i in from..to {
                let addr = match insts[i].op {
                    Op::Load { addr, .. } | Op::Store { addr, .. } if !multiple[i] => addr,
                    _ => continue,
                };
                let Some(o) = sp[addr.index()] else { continue };
                let Ok(slot) = i32::try_from(o - base) else {
                    continue;
                };
                if let Some(v) = self.var(Home::Slot(slot), at[i])
                    && !said.contains(&(at[i], v))
                {
                    said.push((at[i], v));
                    let insn = (0..=i).rev().find(|&j| matches!(insts[j].op, Op::Insn(_)));
                    note(
                        insn.unwrap_or(i),
                        format!("sp+{slot}: {}", self.locals[v].name),
                    );
                }
            }
        }
        out
    }

    /// The comment for a call to `target`, if it has a prototype: its
    /// arguments by register and, unless it is `void`, its value in `r0`.
    pub fn call(&self, target: u32) -> Option<String> {
        let item = self.names.functions.get(&target)?;
        let (ret, params) = proto_parts(item.proto.as_deref()?, &item.name)?;
        let args: Vec<String> = params
            .iter()
            .enumerate()
            .map(|(i, p)| match i {
                0..4 => format!("{p}: r{i}"),
                _ => format!("{p}: sp+{}", 4 * (i - 4)),
            })
            .collect();
        let call = format!("{}({})", item.name, args.join(", "));
        Some(match ret.as_str() {
            "void" => call,
            _ => format!("r0 = {call}: {ret}"),
        })
    }

    /// The identifiers this function's locals can print as.
    pub fn idents(names: &SourceMap, entry: u32) -> Vec<String> {
        let locals = names.locals.get(&entry).map_or(&[][..], Vec::as_slice);
        locals
            .iter()
            .filter(|l| matches!(l.home, Home::Reg(_)))
            .filter_map(|l| names.local_ident(&l.name))
            .collect()
    }
}

/// Each value's offset from `sp` at the block's start, where it is one:
/// `sp` read, plus or minus constants.
fn sp_offsets(insts: &[analysis::ir::Inst]) -> Vec<Option<i64>> {
    let mut off: Vec<Option<i64>> = vec![None; insts.len()];
    let k = |v: ValueId| match insts[v.index()].op {
        Op::Const(c) | Op::Pool(c) => Some(c),
        _ => None,
    };
    for (i, inst) in insts.iter().enumerate() {
        let of = |v: ValueId| off[v.index()];
        let k = |v: ValueId| k(v).map(|c| i64::from(c as i32));
        let o = match inst.op {
            Op::GetReg(r) if r == Reg::SP => Some(0),
            Op::Bin(BinOp::Add, a, b) => of(a)
                .zip(k(b))
                .or_else(|| of(b).zip(k(a)))
                .map(|(o, c)| o + c),
            Op::Bin(BinOp::Sub, a, b) => of(a).zip(k(b)).map(|(o, c)| o - c),
            Op::Adc { a, b, c } => match insts[c.index()].op {
                Op::Bit(x) => of(a).zip(k(b)).map(|(o, c)| o + c + i64::from(x)),
                _ => None,
            },
            _ => None,
        };
        off[i] = o;
    }
    off
}

/// A prototype's return type and parameter names: `u16
/// GetBagItemQuantity(u16 *quantity)` gives `u16` and `quantity`. A
/// parameter with no name prints as `_`; `(void)` has none.
fn proto_parts(proto: &str, name: &str) -> Option<(String, Vec<String>)> {
    let (ret, params) = proto_params(proto, name)?;
    let names = params
        .into_iter()
        .map(|(n, _)| n.unwrap_or_else(|| "_".into()));
    Some((ret, names.collect()))
}

/// A parameter's name, if it has one, and its C type.
pub(crate) type Param = (Option<String>, String);

/// [`proto_parts`] with each parameter's C type: `u16 *quantity` gives
/// `quantity` and `u16 *`; a parameter with no name, `None` and its whole
/// text. `...` is a parameter of that type.
pub(crate) fn proto_params(proto: &str, name: &str) -> Option<(String, Vec<Param>)> {
    let at = proto.find(&format!("{name}("))?;
    let ret = proto[..at].trim().to_string();
    let open = at + name.len();
    let mut depth = 0;
    let mut close = None;
    for (i, c) in proto[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let inner = &proto[open + 1..close?];
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (i, c) in inner.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    let params = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty() && *p != "void")
        .map(|p| match param_name(p) {
            Some(n) => {
                let ty = match p.find("(*") {
                    Some(_) => p.replacen(&n, "", 1),
                    None => match p.rfind(n.as_str()) {
                        Some(i) => format!("{}{}", &p[..i], &p[i + n.len()..]),
                        None => p.to_string(),
                    },
                };
                (Some(n), ty.split_whitespace().collect::<Vec<_>>().join(" "))
            }
            None => (None, p.to_string()),
        })
        .collect();
    Some((if ret.is_empty() { "int".into() } else { ret }, params))
}

/// A C parameter's name: the identifier after `(*` in a function
/// pointer, else the last identifier before any `[`.
fn param_name(p: &str) -> Option<String> {
    let p = match p.find("(*") {
        Some(i) => &p[i + 2..],
        None => p.split('[').next()?,
    };
    let words: Vec<&str> = p
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    let w = if p.contains(')') {
        words.first()?
    } else {
        words.last()?
    };
    const TYPES: &[&str] = &[
        "const", "volatile", "unsigned", "signed", "int", "char", "short", "long", "void",
        "struct", "union", "enum", "u8", "u16", "u32", "s8", "s16", "s32", "bool8", "bool32",
    ];
    // A bare type (`u8`, `struct Pokemon *`) names nothing.
    let tagged = matches!(words[..], [t, _] if ["struct", "union", "enum"].contains(&t));
    let bare = p.trim_end().ends_with('*') || tagged || words.len() < 2;
    (!bare && !TYPES.contains(w)).then(|| w.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protos() {
        let p = |s: &str, n: &str| proto_parts(s, n).unwrap();
        assert_eq!(p("u16 Random(void)", "Random"), ("u16".into(), vec![]));
        assert_eq!(
            p(
                "u16 GetBagItemQuantity(u16 *quantity)",
                "GetBagItemQuantity"
            ),
            ("u16".into(), vec!["quantity".into()])
        );
        assert_eq!(
            p(
                "void SetMainCallback2(void (*callback)(void), u8 x[4], struct Pokemon *)",
                "SetMainCallback2"
            ),
            (
                "void".into(),
                vec!["callback".into(), "x".into(), "_".into()]
            )
        );
        assert_eq!(
            p("struct Sprite *GetSprite(u8 id)", "GetSprite").0,
            "struct Sprite *"
        );
    }

    #[test]
    fn param_types() {
        let p = |s: &str, n: &str| proto_params(s, n).unwrap().1;
        let s = |x: &str| Some(x.to_string());
        assert_eq!(
            p(
                "u16 GetBagItemQuantity(u16 *quantity)",
                "GetBagItemQuantity"
            ),
            vec![(s("quantity"), "u16 *".into())]
        );
        assert_eq!(
            p(
                "void F(void (*callback)(u8), const u8 x[4], struct Pokemon *)",
                "F"
            ),
            vec![
                (s("callback"), "void (*)(u8)".into()),
                (s("x"), "const u8 [4]".into()),
                (None, "struct Pokemon *".into()),
            ]
        );
    }
}
