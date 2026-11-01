//! How a block's insts become Rust expressions and statements
//! (`docs/codegen-design.md`, "Printing a block"). Most of rustc's cost in
//! a generated crate is in its front end, which grows with the number of
//! statements and locals, so the printer binds as few as it can:
//!
//! - constants are literals at their uses;
//! - a value used once is printed inside its use, if it is pure, or a read
//!   with no write to what it reads (nor a `rt::stall`) between it and the
//!   statement it ends up in, or a load with no other access between and whose use runs
//!   unconditionally (not under a guard or in one arm of a `select`);
//! - the results of one `add_with_carry`, `shift` or `mul_long` that are
//!   used apart (a result and its carry) come from one call bound to a
//!   tuple, since the IR keeps them as separate insts;
//! - a shift by a constant is a plain `<<`, `>>` or `rotate_right`, and
//!   so is a rotate by a register when its carry isn't used.
//!
//! The printer only reorders what can't tell: a pure value is the same
//! wherever it is computed, a read moves only where no write could change
//! it, and a load keeps its place among the bus accesses and runs exactly
//! once.

use crate::func::Clock;
use crate::io_regs;
use crate::locals::{Locals, Point};
use crate::names::{Kind, SourceMap};
use crate::types::{Step, Ty};
use analysis::ir::{BinOp, BitOp, Inst, Op, ValueId};
use armv4t::sem::{Flag, ShiftType, Width};
use proc_macro2::{Delimiter, Group, Ident, Literal, Span, TokenStream, TokenTree};
use quote::{format_ident, quote};
use std::collections::{BTreeMap, HashMap};

#[cfg(test)]
mod tests;

pub fn v(i: ValueId) -> Ident {
    format_ident!("v{}", i.0)
}

fn ident(name: impl std::fmt::Debug) -> Ident {
    Ident::new(&format!("{name:?}"), Span::call_site())
}

/// Whether the inst at `op` produces a bit (`bool`) rather than a word.
pub fn is_bit(op: &Op) -> bool {
    matches!(
        op,
        Op::Bit(_)
            | Op::AdcCarry { .. }
            | Op::AdcOverflow { .. }
            | Op::MulCarry { .. }
            | Op::ShiftCarry { .. }
            | Op::TestBit { .. }
            | Op::IsZero(_)
            | Op::BNot(_)
            | Op::BBin(..)
            | Op::BSelect { .. }
            | Op::GetFlag(_)
    )
}

/// A word literal: decimal below 256, else hex in groups of four. With
/// `typed`, suffixed, for places where nothing else gives it a type.
pub(crate) fn word(x: u32, typed: bool) -> TokenStream {
    let mut s = if x < 256 {
        x.to_string()
    } else if x <= 0xFFFF {
        format!("{x:#x}")
    } else {
        format!("0x{:x}_{:04x}", x >> 16, x & 0xFFFF)
    };
    if typed {
        s.push_str("_u32");
    }
    s.parse().unwrap()
}

/// The alignment `ir::eval` applies to an access's address.
fn mask(w: Width) -> TokenStream {
    match w {
        Width::Byte => quote!(),
        Width::Half => quote!(&!1),
        Width::Word => quote!(&!3),
    }
}

/// A shift amount, if it is a constant: its low byte, as the shifter
/// reads it.
fn const_amount(insts: &[Inst], amount: ValueId) -> Option<u32> {
    match insts[amount.index()].op {
        Op::Const(n) | Op::Pool(n) => Some(n & 0xFF),
        _ => None,
    }
}

/// Which of a multi-result prim's results an inst is, and the call's
/// identity (the prim and the operands its results share).
fn slot(insts: &[Inst], op: &Op) -> Option<(Key, usize)> {
    use Op::*;
    Some(match *op {
        Adc { a, b, c } => (Key::Adc(a, b, c), 0),
        AdcCarry { a, b, c } => (Key::Adc(a, b, c), 1),
        AdcOverflow { a, b, c } => (Key::Adc(a, b, c), 2),
        MulLongLo { signed, a, b } => (Key::MulLong(signed, a, b), 0),
        MulLongHi { signed, a, b } => (Key::MulLong(signed, a, b), 1),
        // A shift by a constant is printed natively, no call to share.
        Shift { kind, v, amount } if const_amount(insts, amount).is_none() => {
            (Key::Shift(kind, v, amount), 0)
        }
        ShiftCarry {
            kind, v, amount, ..
        } if const_amount(insts, amount).is_none() => (Key::Shift(kind, v, amount), 1),
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Adc(ValueId, ValueId, ValueId),
    MulLong(bool, ValueId, ValueId),
    Shift(ShiftType, ValueId, ValueId),
}

/// The results of one call that several insts share.
struct Tuple {
    key: Key,
    /// The first member's index: where the call is bound.
    at: usize,
    /// The member in each result slot.
    slots: [Option<usize>; 3],
}

impl Tuple {
    /// The operands the call names.
    fn operands(&self, insts: &[Inst]) -> Vec<ValueId> {
        match self.key {
            Key::Adc(a, b, c) => vec![a, b, c],
            Key::MulLong(_, a, b) => vec![a, b],
            Key::Shift(_, x, n) => {
                let mut o = vec![x, n];
                o.extend(self.carry_in(insts));
                o
            }
        }
    }

    /// A shift's carry in, if its carry is one of the results.
    fn carry_in(&self, insts: &[Inst]) -> Option<ValueId> {
        match self.slots[1].map(|s| insts[s].op) {
            Some(Op::ShiftCarry { carry_in, .. }) => Some(carry_in),
            _ => None,
        }
    }
}

/// What one inst's value becomes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    /// Not printed: nothing uses it (or it is printed with its tuple).
    Gone,
    /// Bound by a `let` (or a tuple) at its place.
    Bound,
    /// Printed inside its one use.
    Inline,
}

/// What a printed value is as an operand: an atom (a name, a literal, a
/// call) or something an operator needs parenthesised.
struct E {
    t: TokenStream,
    atom: bool,
}

impl E {
    fn atom(t: TokenStream) -> E {
        E { t, atom: true }
    }

    fn compound(t: TokenStream) -> E {
        E { t, atom: false }
    }

    /// As an operand of an operator, a cast or a method call.
    fn operand(self) -> TokenStream {
        if self.atom {
            self.t
        } else {
            TokenTree::Group(Group::new(Delimiter::Parenthesis, self.t)).into()
        }
    }
}

/// One block's printing plan: which values are bound, inlined or gone.
pub struct Printer<'a> {
    insts: &'a [Inst],
    form: Vec<Form>,
    /// How many times each value is printed by name or inline.
    uses: Vec<usize>,
    /// The tuple each of its members belongs to.
    tuple_of: Vec<Option<usize>>,
    tuples: Vec<Tuple>,
    /// The tuple bound just before each inst.
    tuple_at: Vec<Option<usize>>,
    /// For the last flag write of a run (see [`flag_runs`]), the run.
    flag_run: Vec<Option<Vec<usize>>>,
    /// Whether each inst is a flag write in a run of two or more.
    in_run: Vec<bool>,
    /// The CPU state the function holds in locals.
    loc: &'a Locals,
    /// The source map a named print names pool words from.
    names: Option<&'a SourceMap>,
    /// The readable form's accesses: for each `Load` or `Store` printed
    /// with its instruction's stall, the instruction's address.
    fused: Vec<Option<u32>>,
    /// See [`Hints::rotated`]: the loads printed inside their rotate.
    rotated: Vec<Option<usize>>,
    /// See [`Hints::via`].
    via: Vec<Option<(u32, u32)>>,
    /// See [`Hints::carried`].
    carried: u16,
    /// See [`Hints::index`].
    index: Vec<Option<Index>>,
    /// Values named from DWARF (`crate::dwarf`), by inst.
    value_names: Vec<Option<Ident>>,
    /// How the code keeps time: without mGBA's model an access carries
    /// no instruction address and a pool load nothing uses isn't printed.
    clock: Clock,
    /// The entry block's parameters, by argument register: each read of
    /// one prints its identifier (`.0` for a pointer's `Ptr`), and an
    /// access through a pointer one its path ([`Printer::args`]).
    args: Vec<Option<(Ident, Option<Ty>)>>,
}

/// How the readable form (`docs/mod-view-design.md`) prints a block,
/// beyond the play view's plan: register and flag writes moved to the
/// exits that sync them, accesses that carry their stall, and loads a
/// macro binds.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// For each `SetReg` and `SetFlag` moved out of its place, the exits
    /// whose sync block writes it, and whether each runs only under its
    /// guard. Empty: written nowhere (a later write in the block replaces
    /// it before any exit).
    pub moved: Vec<Option<Vec<(usize, bool)>>>,
    /// For each `Load` or `Store` printed with its instruction's stall
    /// (`m.get`, `m.set`, `m.pool`), the instruction's address.
    pub fused: Vec<Option<u32>>,
    /// Values bound by a `let` whatever their uses: loads a statement the
    /// lowering prints binds (`pop!`), and rotates that print a load.
    pub bound: Vec<bool>,
    /// For a misaligned load's rotate (`ldr`, `ldrh`), the load it
    /// rotates: the rotate prints as the CPU's load, `m.get`, if the load
    /// has no other use.
    pub rotated: Vec<Option<usize>>,
    /// The operands an inst's printed form names, where the lowering
    /// prints it (a `push!`'s access names only its values and the base).
    pub operands: Vec<Option<Vec<ValueId>>>,
    /// For an access with the pool load before it folded in
    /// (`m.get_via`), that load's instruction address and literal.
    pub via: Vec<Option<(u32, u32)>>,
    /// The registers whose value at the block's start a Rust local `rN`
    /// holds (`lower::Carry`): their reads print as the local.
    pub carried: u16,
    /// For an access printed with its stall whose address is a typed
    /// array's element (see [`index_of`]), the element: its `operands`
    /// name the index, not the address.
    pub index: Vec<Option<Index>>,
}

/// An access's address as an element of a typed array, plus a constant
/// (`docs/mod-view-design.md`, gap 5): `((i << 2) + i) << 3` plus
/// `gTasks` is `gTasks.idx(i)`, as 40 is a `Task`'s size. Or, with no
/// index, a typed datum's address plus a constant that is no IR constant
/// (a register the block starts with it in).
#[derive(Clone, Debug)]
pub struct Index {
    /// The typed datum's address, and the steps from it to the array.
    datum: u32,
    to: Vec<Step>,
    /// The index, and the element it counts from (`idx(i + k)`).
    pub(crate) i: Option<ValueId>,
    k: u32,
    /// The element's type, and the access's offset into it.
    elem: Ty,
    off: u32,
}

/// `addr` as an element of a typed datum's array plus a constant: a
/// constant inside the datum plus a value scaled by the size of an
/// array's elements there (through adds, shifts left and multiplies by
/// constants). `known`: the registers the block starts with a known
/// constant in, which may make the whole address one. A byte array's
/// index is only taken for a datum that is one: a byte offset into a
/// struct is no element.
pub fn index_of(
    insts: &[Inst],
    names: &SourceMap,
    known: &dyn Fn(usize) -> Option<u32>,
    addr: ValueId,
) -> Option<Index> {
    let typed = &names.typed;
    let (c, i) = linear(insts, known, addr, 8);
    let Some((i, s)) = i else {
        // A constant the printer's own typed path doesn't see.
        if matches!(insts[addr.index()].op, Op::Const(_) | Op::Pool(_)) {
            return None;
        }
        let (datum, ty) = typed.global_at(c)?;
        names.ident(Kind::Data, datum)?;
        return Some(Index {
            datum,
            to: Vec::new(),
            i: None,
            k: 0,
            elem: ty.clone(),
            off: c - datum,
        });
    };
    if s == 0 || matches!(insts[i.index()].op, Op::Const(_) | Op::Pool(_)) {
        return None;
    }
    let (datum, ty) = typed.global_at(c)?;
    let (to, elem, k, off) = typed.array_at(ty, c - datum, s)?;
    if s == 1 && !to.is_empty() || names.ident(Kind::Data, datum).is_none() {
        return None;
    }
    Some(Index {
        datum,
        to,
        i: Some(i),
        k,
        elem,
        off,
    })
}

/// `x` as `c + leaf * s`: `(c, Some((leaf, s)))`, or `(c, None)` for a
/// constant. Through adds with no carry in, shifts left and multiplies
/// by constants, `depth` deep; a sum of two other values is a leaf.
fn linear(
    insts: &[Inst],
    known: &dyn Fn(usize) -> Option<u32>,
    x: ValueId,
    depth: u32,
) -> (u32, Option<(ValueId, u32)>) {
    let leaf = (0, Some((x, 1)));
    if depth == 0 {
        return leaf;
    }
    let lin = |y: ValueId| linear(insts, known, y, depth - 1);
    let konst = |y: ValueId| match insts[y.index()].op {
        Op::Const(c) | Op::Pool(c) => Some(c),
        _ => None,
    };
    let sum = |a: ValueId, b: ValueId| {
        let ((ca, ta), (cb, tb)) = (lin(a), lin(b));
        let c = ca.wrapping_add(cb);
        match (ta, tb) {
            (None, t) | (t, None) => (c, t),
            (Some((la, sa)), Some((lb, sb))) if la == lb => (c, Some((la, sa.wrapping_add(sb)))),
            _ => leaf,
        }
    };
    let scale = |y: ValueId, k: u32| {
        let (c, t) = lin(y);
        (c.wrapping_mul(k), t.map(|(l, s)| (l, s.wrapping_mul(k))))
    };
    match insts[x.index()].op {
        Op::Const(c) | Op::Pool(c) => (c, None),
        Op::GetReg(r) => known(r.index()).map_or(leaf, |c| (c, None)),
        Op::Bin(BinOp::Add, a, b) => sum(a, b),
        Op::Adc { a, b, c } if insts[c.index()].op == Op::Bit(false) => sum(a, b),
        Op::Shift {
            kind: ShiftType::Lsl,
            v,
            amount,
        } => match const_amount(insts, amount) {
            Some(n) if n < 32 => scale(v, 1 << n),
            _ => leaf,
        },
        Op::Bin(BinOp::Mul, a, b) => match (konst(a), konst(b)) {
            (_, Some(k)) => scale(a, k),
            (Some(k), _) => scale(b, k),
            _ => leaf,
        },
        _ => leaf,
    }
}

/// One use of a value: the statement it is printed in (a tuple's is its
/// place), and whether it runs there only conditionally: in one arm of a
/// `select`, or under the statement's guard.
#[derive(Clone, Copy)]
struct Use {
    by: usize,
    cond: bool,
}

impl<'a> Printer<'a> {
    /// Plans `insts`. `stall_before[i]`: a `rt::stall` is printed just
    /// before inst `i` (a call on the whole machine, so nothing moves past
    /// it).
    /// `loc`: what the function holds in locals.
    pub fn new(insts: &'a [Inst], stall_before: &[bool], loc: &'a Locals) -> Printer<'a> {
        Printer::hinted(insts, stall_before, loc, &Hints::default())
    }

    /// [`Printer::new`], for the readable form's `hints`.
    pub fn hinted(
        insts: &'a [Inst],
        stall_before: &[bool],
        loc: &'a Locals,
        hints: &Hints,
    ) -> Printer<'a> {
        let n = insts.len();
        let moved = |i: usize| hints.moved.get(i).and_then(Option::as_ref);
        let printed = |i: usize| match hints.operands.get(i) {
            Some(Some(o)) => o.clone(),
            _ => printed_operands(insts, &insts[i].op),
        };

        // Live insts, backwards: effects, and whatever a live inst's
        // printed form names.
        let mut live = vec![false; n];
        for i in (0..n).rev() {
            let inst = &insts[i];
            let root = inst.op.is_root() && moved(i).is_none_or(|m| !m.is_empty());
            if !(live[i] || root) {
                continue;
            }
            live[i] = true;
            for o in printed(i).into_iter().chain(inst.guard) {
                live[o.index()] = true;
            }
        }

        // Tuples: the live results of one call, two or more, if all their
        // operands are defined before the first of them.
        let mut first: HashMap<Key, usize> = HashMap::new();
        let mut tuples: Vec<Tuple> = Vec::new();
        let mut tuple_of = vec![None; n];
        for (i, inst) in insts.iter().enumerate() {
            if !live[i] || inst.guard.is_some() {
                continue;
            }
            let Some((key, s)) = slot(insts, &inst.op) else {
                continue;
            };
            match first.get(&key) {
                Some(&t)
                    if tuples[t].slots[s].is_none()
                        && inst.op.operands().iter().all(|o| o.index() < tuples[t].at) =>
                {
                    tuples[t].slots[s] = Some(i);
                    tuple_of[i] = Some(t);
                }
                Some(_) => {}
                None => {
                    first.insert(key, tuples.len());
                    tuple_of[i] = Some(tuples.len());
                    let mut slots = [None; 3];
                    slots[s] = Some(i);
                    tuples.push(Tuple { key, at: i, slots });
                }
            }
        }
        for t in &mut tuples {
            if t.slots.iter().flatten().count() < 2 {
                for m in t.slots.iter().flatten() {
                    tuple_of[*m] = None;
                }
                t.slots = [None; 3];
            }
        }
        let mut tuple_at = vec![None; n];
        for (k, t) in tuples.iter().enumerate() {
            if t.slots.iter().any(Option::is_some) {
                tuple_at[t.at] = Some(k);
            }
        }

        // Uses, by the statement each is printed in.
        let mut uses: Vec<Vec<Use>> = vec![Vec::new(); n];
        let mut add = |o: ValueId, by: usize, cond: bool| uses[o.index()].push(Use { by, cond });
        for (i, inst) in insts.iter().enumerate() {
            if let Some(t) = tuple_at[i] {
                for o in tuples[t].operands(insts) {
                    add(o, i, false);
                }
            }
            if !live[i] || tuple_of[i].is_some() {
                continue;
            }
            if let Some(at) = moved(i) {
                for &(by, cond) in at {
                    for o in printed(i) {
                        add(o, by, cond);
                    }
                }
                continue;
            }
            let arms = match inst.op {
                Op::Select { a, b, .. } | Op::BSelect { a, b, .. } => vec![a, b],
                _ => vec![],
            };
            // A `YieldFlag`'s value is written only on its exit's yield path.
            let guarded = inst.guard.is_some() || matches!(inst.op, Op::YieldFlag(..));
            for o in printed(i) {
                add(o, i, guarded || arms.contains(&o));
            }
            if let Some(g) = inst.guard {
                add(g, i, false);
            }
        }

        // Bus barriers, as prefix counts over the insts before each index:
        // accesses, exits, timing ops and stalls.
        let mut acc = vec![0u32; n + 1];
        for (i, inst) in insts.iter().enumerate() {
            // A timing op too: an `ldm`'s count must come before its loads.
            let x = matches!(
                inst.op,
                Op::Load { .. } | Op::Store { .. } | Op::Exit(_) | Op::Timing(_)
            );
            acc[i + 1] = acc[i] + stall_before[i] as u32 + x as u32;
        }

        // Forms, backwards so each value's one user is decided first.
        // `pos[i]`: the statement an inlined value ends up in; `cond[i]`:
        // whether it runs there only conditionally.
        let mut form = vec![Form::Gone; n];
        let mut pos = vec![0; n];
        let mut cond = vec![false; n];
        for i in (0..n).rev() {
            let inst = &insts[i];
            if !live[i] {
                continue;
            }
            form[i] = Form::Bound;
            let bound = hints.bound.get(i).copied().unwrap_or(false);
            if bound || inst.guard.is_some() || tuple_of[i].is_some() || uses[i].is_empty() {
                continue;
            }
            // (A carried register's read is a local, read where it is
            // printed.)
            let carried = matches!(inst.op, Op::GetReg(r) if hints.carried & 1 << r.index() != 0);
            if matches!(inst.op, Op::Const(_) | Op::Pool(_) | Op::Bit(_)) || carried {
                form[i] = Form::Inline;
                continue;
            }
            let [u] = uses[i][..] else { continue };
            let (p, c) = if form[u.by] == Form::Inline {
                (pos[u.by], cond[u.by] || u.cond)
            } else {
                (u.by, u.cond)
            };
            // Nothing between here and there that could change a read or
            // reorder an access.
            let at_exit = matches!(insts[p].op, Op::Exit(_) | Op::YieldFlag(..));
            let ok = match inst.op {
                op if is_read(&op) => {
                    !at_exit
                        && !stall_before[i + 1..=p].contains(&true)
                        && !insts[i + 1..p].iter().any(|w| changes(&w.op, &op))
                }
                // A load runs exactly once, where it was.
                // Not into another access either: both borrow `m.bus`.
                Op::Load { .. } => {
                    !matches!(
                        insts[p].op,
                        Op::Exit(_) | Op::Load { .. } | Op::Store { .. }
                    ) && !c
                        && acc[p] == acc[i + 1]
                        && !stall_before[p]
                }
                op => !op.is_root(),
            };
            if ok {
                form[i] = Form::Inline;
                pos[i] = p;
                cond[i] = c;
            }
        }
        // A rotate prints its load only if the load is printed nowhere
        // else.
        let rotated = (0..n)
            .map(|j| {
                let i = hints.rotated.get(j).copied().flatten()?;
                (form[i] == Form::Inline && uses[i].len() == 1).then_some(i)
            })
            .collect();
        // With the flags in locals each write is one assignment.
        let flag_runs = if loc.flags {
            vec![None; n]
        } else {
            flag_runs(insts, stall_before)
        };
        Printer {
            loc,
            insts,
            form,
            uses: uses.iter().map(Vec::len).collect(),
            tuple_of,
            tuples,
            tuple_at,
            in_run: flag_runs
                .iter()
                .flatten()
                .flatten()
                .fold(vec![false; n], |mut v, &i| {
                    v[i] = true;
                    v
                }),
            flag_run: flag_runs,
            names: None,
            fused: (0..n)
                .map(|i| hints.fused.get(i).copied().flatten())
                .collect(),
            rotated,
            via: (0..n)
                .map(|i| hints.via.get(i).copied().flatten())
                .collect(),
            carried: hints.carried,
            index: (0..n)
                .map(|i| hints.index.get(i).cloned().flatten())
                .collect(),
            value_names: Vec::new(),
            args: Vec::new(),
            clock: Clock::Exact,
        }
    }

    /// Prints for `clock` ([`crate::func::Config::clock`]).
    pub fn clocked(mut self, clock: Clock) -> Printer<'a> {
        self.clock = clock;
        self
    }

    /// How the code keeps time.
    pub fn clock(&self) -> Clock {
        self.clock
    }

    /// An access's instruction address as its call's first argument
    /// (`0x0805_9818,`), or nothing without mGBA's model, which is all it
    /// was for.
    pub(crate) fn at(&self, at: u32) -> TokenStream {
        match self.clock {
            Clock::Exact => {
                let at = crate::lower::hex(at);
                quote!(#at,)
            }
            _ => quote!(),
        }
    }

    /// Whether load `i` prints as `m.pool`: a literal-pool word nothing
    /// uses, whose value const-prop has folded.
    fn is_pool(&self, i: usize) -> bool {
        matches!(
            (self.insts[i].op, self.fused[i]),
            (Op::Load { width: Width::Word, addr }, Some(_))
                if self.uses[i] == 0
                    && matches!(self.insts[addr.index()].op,
                        Op::Const(lit) | Op::Pool(lit) if (0x0800_0000..0x0e00_0000).contains(&lit))
        )
    }

    /// Names the pool words that land in `names`' data as `addr!(name)`
    /// or `addr!(name + off)`; `None` prints them as literals.
    pub fn named(mut self, names: Option<&'a SourceMap>) -> Printer<'a> {
        self.names = names;
        self
    }

    /// Names values from DWARF (`crate::dwarf`): `names[i]` for inst
    /// `i`'s, where it has one.
    pub fn values_named(mut self, names: Vec<Option<Ident>>) -> Printer<'a> {
        self.value_names = names;
        self
    }

    /// Prints the entry block's reads of r0–r3 as the parameters `args`
    /// (`crate::types::Proto::args`): the block reads each register as the
    /// function was entered with it, since its writes wait for its exits
    /// and nothing jumps back to its start.
    pub fn args(mut self, args: Vec<Option<(Ident, Option<Ty>)>>) -> Printer<'a> {
        self.args = args;
        self
    }

    /// The parameter inst `i` reads, if it reads one.
    pub fn arg_of(&self, i: usize) -> Option<&(Ident, Option<Ty>)> {
        match self.insts[i].op {
            Op::GetReg(r) => self.args.get(r.index())?.as_ref(),
            _ => None,
        }
    }

    /// Whether inst `i`'s value is bound by a `let` (or a tuple's).
    pub fn binds(&self, i: usize) -> bool {
        self.form[i] != Form::Inline
    }

    /// Whether inst `i`'s value is printed by its identifier: bound by a
    /// `let` and used.
    pub fn shows(&self, i: usize) -> bool {
        self.form[i] == Form::Bound && self.uses[i] > 0
    }

    /// Value `i`'s identifier: its DWARF name, else `v<i>`.
    pub fn id(&self, i: ValueId) -> Ident {
        match self.value_names.get(i.index()) {
            Some(Some(id)) => id.clone(),
            _ => v(i),
        }
    }

    /// The source map a named print names from.
    pub fn names(&self) -> Option<&'a SourceMap> {
        self.names
    }

    /// The identifier of the fn for the function whose entry is `key`.
    pub fn fn_ident(&self, key: u32) -> Ident {
        match self.names {
            Some(n) => n.fn_ident(key),
            None => crate::lower::fn_name(key),
        }
    }

    /// Whether inst `i`'s value is printed anywhere.
    pub fn used(&self, i: usize) -> bool {
        self.uses[i] > 0
    }

    /// Inst `i`'s op.
    pub fn op(&self, i: usize) -> Op {
        self.insts[i].op
    }

    /// What the function holds in locals.
    pub fn locals(&self) -> &'a Locals {
        self.loc
    }

    /// `i`'s value as a function argument or the right side of a `let`.
    pub fn arg(&self, i: ValueId) -> TokenStream {
        self.val(i, false).t
    }

    /// `i`'s value as an operand of an operator. `typed`: nothing else
    /// gives a literal in it a type (a method's receiver, a cast, `==`).
    fn opnd(&self, i: ValueId, typed: bool) -> TokenStream {
        self.val(i, typed).operand()
    }

    fn val(&self, i: ValueId, typed: bool) -> E {
        if self.form[i.index()] != Form::Inline {
            let id = self.id(i);
            return E::atom(quote!(#id));
        }
        self.expr_at(i.index(), typed)
    }

    /// The expression inst `i` computes: [`Self::expr`], or a fused
    /// access's.
    fn expr_at(&self, i: usize, typed: bool) -> E {
        if let Some(l) = self.rotated[i]
            && let (Op::Load { width, addr }, Some(at)) = (self.insts[l].op, self.fused[l])
        {
            let (at, p) = (self.at(at), self.ptr(l, width, addr));
            let (get, via) = self.via_of(l, "get");
            return E::atom(quote!(m.#get(#via #at #p)));
        }
        match (self.insts[i].op, self.fused[i]) {
            (Op::Load { width, addr }, Some(at)) => {
                let at = self.at(at);
                // A literal-pool word, whose value const-prop has folded.
                if self.is_pool(i)
                    && let Op::Const(lit) | Op::Pool(lit) = self.insts[addr.index()].op
                {
                    let lit = crate::lower::hex(lit);
                    return E::atom(quote!(m.pool(#at #lit)));
                }
                let p = self.ptr(i, width, addr);
                // `m.get` rotates a misaligned halfword or word as the CPU
                // does; a load whose rotate isn't printed with it is the
                // bus's read, `m.load`, unless its address is aligned.
                let aligned = match self.insts[addr.index()].op {
                    Op::Const(a) | Op::Pool(a) => a & (width.bytes() - 1) == 0,
                    _ => width == Width::Byte,
                };
                let (f, via) = self.via_of(i, if aligned { "get" } else { "load" });
                E::atom(quote!(m.#f(#via #at #p)))
            }
            (op, _) => self.expr(&op, typed),
        }
    }

    /// The `Ptr` access `i` (a `Load` or `Store` of `width` at `addr`)
    /// goes through: typed from the source map where its address is a
    /// typed datum's or a pointer loaded from one, plus a constant
    /// (`gTasks.idx(15).next()`, [`crate::types`]), else
    /// `Ptr::<uN>::at(addr)`.
    fn ptr(&self, i: usize, width: Width, addr: ValueId) -> TokenStream {
        let bits = matches!(self.insts[i].op, Op::Load { .. })
            .then(|| self.bits_kept(i, width))
            .flatten();
        self.ptr_bits(i, width, addr, bits).0
    }

    /// [`Self::ptr`] with the bits `bits` picking a bitfield, and whether
    /// it names a bitfield of exactly those bits
    /// ([`crate::types::Types::is_bitfield`]).
    fn ptr_bits(
        &self,
        i: usize,
        width: Width,
        addr: ValueId,
        bits: Option<(u32, u32)>,
    ) -> (TokenStream, bool) {
        let raw = || {
            let (t, a) = (ptr_ty(width), self.arg(addr));
            quote!(Ptr::<#t>::at(#a))
        };
        let Some(names) = self.names else {
            return (raw(), false);
        };
        if let Some(ix) = &self.index[i] {
            return self.indexed(ix, width, bits);
        }
        let Some((base, ty, off, global)) = self.typed_addr(addr) else {
            let io = self
                .const_addr(addr)
                .and_then(|x| io_regs::at(x, width.bytes()));
            let Some((reg, off)) = io else {
                return (raw(), false);
            };
            let reg = format_ident!("{}", io_regs::ident(reg));
            let rest = rest_of(width, off);
            return (quote!(#reg #rest), false);
        };
        let t = &names.typed;
        let (steps, rest) = t.path(&ty, off, width.bytes(), bits);
        // Through a loaded pointer, only a path that names something.
        if !global && steps.is_empty() {
            return (raw(), false);
        }
        let field = bits.is_some_and(|b| t.is_bitfield(&ty, off, width.bytes(), b));
        let (steps, rest) = (path(&steps), rest_of(width, rest));
        (quote!(#base #steps #rest), field)
    }

    /// Access `i`'s `Ptr` through an element of a typed array, `ix`:
    /// `gTasks.idx(i).func()` (or a datum's path, with no index).
    fn indexed(&self, ix: &Index, width: Width, bits: Option<(u32, u32)>) -> (TokenStream, bool) {
        let names = self
            .names
            .expect("an index is planned only for a named print");
        let base = names
            .ident(Kind::Data, ix.datum)
            .expect("a typed datum is named");
        let base = format_ident!("{base}");
        let at = match (ix.i, ix.k) {
            (None, _) => None,
            (Some(i), 0) => Some(self.arg(i)),
            (Some(i), k) => {
                let (i, k) = (self.opnd(i, true), word(k, false));
                Some(quote!(#i.wrapping_add(#k)))
            }
        };
        let at = at.map(|at| quote!(.idx(#at)));
        let t = &names.typed;
        let (steps, rest) = t.path(&ix.elem, ix.off, width.bytes(), bits);
        let field = bits.is_some_and(|b| t.is_bitfield(&ix.elem, ix.off, width.bytes(), b));
        let (to, steps, rest) = (path(&ix.to), path(&steps), rest_of(width, rest));
        (quote!(#base #to #at #steps #rest), field)
    }

    /// Load `l` and store `s` of one bitfield's bits `bits` (`s` storing
    /// what `l` read with those bits replaced by `v`, through `x`) as one
    /// `m.set_bits(..)` statement binding the value read and the value
    /// stored where they are used, if the store's address names that
    /// bitfield.
    pub fn set_bits(
        &self,
        l: usize,
        x: usize,
        s: usize,
        bits: (u32, u32),
        v: crate::lower::BitValue,
    ) -> Option<TokenStream> {
        let Op::Store { width, addr, .. } = self.insts[s].op else {
            return None;
        };
        let (lat, sat) = (self.fused[l]?, self.fused[s]?);
        let (p, field) = self.ptr_bits(s, width, addr, Some(bits));
        if !field {
            return None;
        }
        // A constant by its name, where the field takes one.
        let v = match v {
            crate::lower::BitValue::Of(y) => self.arg(ValueId(y as u32)),
            crate::lower::BitValue::Const(v) => {
                match self.values_in(s, Some(bits)).and_then(|vs| vs.get(&v)) {
                    Some(n) => {
                        let id = format_ident!("{n}");
                        quote!(#id)
                    }
                    None => {
                        let v = Literal::u32_unsuffixed(v);
                        quote!(#v)
                    }
                }
            }
        };
        let (lat, sat) = (self.at(lat), self.at(sat));
        let (f, via) = self.via_of(l, "set_bits");
        let call = quote!(m.#f(#via #lat #sat #p, #v));
        // Printed by name elsewhere than in the write itself.
        let name = |i: usize| {
            if self.uses[i] > 1 {
                let id = self.id(ValueId(i as u32));
                quote!(#id)
            } else {
                quote!(_)
            }
        };
        Some(if self.uses[l] > 1 || self.uses[x] > 1 {
            let (old, new) = (name(l), name(x));
            quote!(let (#old, #new) = #call;)
        } else {
            quote!(#call;)
        })
    }

    /// The names of the values the field access `i` (a fused `Load` or
    /// `Store`) reads or writes takes (`[values]`, [`crate::types`]), by
    /// value: where its `Ptr` names that field.
    fn values_at(&self, i: usize) -> Option<&'a BTreeMap<u32, String>> {
        let Op::Load { width, .. } = self.insts[i].op else {
            return self.values_in(i, None);
        };
        self.values_in(i, self.bits_kept(i, width))
    }

    /// [`Self::values_at`], `bits` picking the bitfield.
    fn values_in(&self, i: usize, bits: Option<(u32, u32)>) -> Option<&'a BTreeMap<u32, String>> {
        let names = self.names?;
        let (Op::Load { width, addr } | Op::Store { width, addr, .. }) = self.insts[i].op else {
            return None;
        };
        self.fused[i]?;
        let t = &names.typed;
        let (ty, off) = match &self.index[i] {
            Some(ix) => (ix.elem.clone(), ix.off),
            None => {
                let (_, ty, off, _) = self.typed_addr(addr)?;
                (ty, off)
            }
        };
        let (steps, rest) = t.path(&ty, off, width.bytes(), bits);
        if rest.is_some() {
            return None;
        }
        t.values_of(&ty, &steps)
    }

    /// Constant `x` by the name of the value it is where it is one of
    /// `vs` (a field's, [`Self::values_at`]) and printed in place.
    fn value_name(&self, vs: Option<&BTreeMap<u32, String>>, x: ValueId) -> Option<TokenStream> {
        let (Op::Const(c), Form::Inline) = (self.insts[x.index()].op, self.form[x.index()]) else {
            return None;
        };
        let id = format_ident!("{}", vs?.get(&c)?);
        Some(quote!(#id))
    }

    /// `add_with_carry`'s second argument: `cmp a, #k` of a field with
    /// one of its named values is `a + !k + 1`, printed `!NAME`.
    fn adc_b(&self, a: ValueId, b: ValueId, c: ValueId) -> TokenStream {
        if let (Op::Const(k), Op::Bit(true), Form::Inline) = (
            self.insts[b.index()].op,
            self.insts[c.index()].op,
            self.form[b.index()],
        ) && let Some(id) = self.values_at(a.index()).and_then(|vs| vs.get(&!k))
        {
            let id = format_ident!("{id}");
            return quote!(!#id);
        }
        self.arg(b)
    }

    /// `addr`'s value where it is a constant, or a constant plus one.
    fn const_addr(&self, addr: ValueId) -> Option<u32> {
        match self.insts[addr.index()].op {
            Op::Const(x) | Op::Pool(x) => Some(x),
            Op::Bin(BinOp::Add, a, b) => {
                Some(self.const_addr(a)?.wrapping_add(self.const_addr(b)?))
            }
            _ => None,
        }
    }

    /// `addr` as a typed object and an offset into it: the expression of
    /// its base (a typed datum's constant, a pointer parameter, or
    /// `Ptr::<T>::at(vN)` for a pointer loaded from one), the base's type,
    /// the offset, and whether the base names something by itself (a datum
    /// or a parameter).
    fn typed_addr(&self, addr: ValueId) -> Option<(TokenStream, Ty, u32, bool)> {
        let names = self.names?;
        let konst = |v: ValueId| match self.insts[v.index()].op {
            Op::Const(c) | Op::Pool(c) => Some(c),
            _ => None,
        };
        match self.insts[addr.index()].op {
            // A pointer parameter, as its `Ptr`.
            Op::GetReg(r) if self.form[addr.index()] == Form::Inline => {
                let (id, to) = self.args.get(r.index())?.as_ref()?;
                Some((quote!(#id), to.clone()?, 0, true))
            }
            Op::Const(x) | Op::Pool(x) => {
                let (k, ty) = names.typed.global_at(x)?;
                let id = format_ident!("{}", names.ident(Kind::Data, k)?);
                Some((quote!(#id), ty.clone(), x - k, true))
            }
            Op::Bin(BinOp::Add, a, b) | Op::Adc { a, b, .. } => {
                if let Op::Adc { c, .. } = self.insts[addr.index()].op
                    && self.insts[c.index()].op != Op::Bit(false)
                {
                    return None;
                }
                let (v, c) = match (konst(a), konst(b)) {
                    (_, Some(c)) => (a, c),
                    (Some(c), _) => (b, c),
                    _ => return None,
                };
                let (base, ty, off, g) = self.typed_addr(v)?;
                let off = off.wrapping_add(c);
                (off < names.typed.size(&ty)).then_some((base, ty, off, g))
            }
            Op::Load {
                width: Width::Word,
                addr: a,
            } if self.form[addr.index()] != Form::Inline => {
                let (_, ty, off, _) = self.typed_addr(a)?;
                let (_, Ty::Ptr(to)) = names.typed.walk(&ty, off, 4, None)? else {
                    return None;
                };
                if !matches!(*to, Ty::Named(_) | Ty::Array(..)) {
                    return None;
                }
                let (t, id) = (names.typed.rust(&to), self.id(addr));
                Some((quote!(Ptr::<#t>::at(#id)), *to, 0, false))
            }
            _ => None,
        }
    }

    /// The bits of load `i`'s value of `width` the code keeps, if its one
    /// use picks out a run of them: `& mask`, `>> n`, or `<< a` then
    /// `>> b`. Bit 0 is the access's lowest.
    fn bits_kept(&self, i: usize, width: Width) -> Option<(u32, u32)> {
        // The printed insts that read `x` (not a flag write a later one
        // replaces).
        let readers = |x: usize| {
            let id = ValueId(x as u32);
            self.insts
                .iter()
                .enumerate()
                .filter(move |&(k, j)| self.form[k] != Form::Gone && j.op.operands().contains(&id))
        };
        let users = |x: usize| {
            let mut u = readers(x);
            match (u.next(), u.next()) {
                (Some((k, j)), None) => Some((k, j.op)),
                _ => None,
            }
        };
        let konst = |v: ValueId| match self.insts[v.index()].op {
            Op::Const(c) => Some(c),
            _ => None,
        };
        let bits = width.bytes() * 8;
        match users(i)?.1 {
            Op::Bin(BinOp::And, a, b) => {
                let m = konst(b).or_else(|| konst(a))?;
                let lo = m.trailing_zeros();
                let w = (m >> lo).trailing_ones();
                (m != 0 && (m >> lo) >> w == 0).then_some((lo, w))
            }
            Op::Shift {
                kind: ShiftType::Lsr,
                v,
                amount,
            } if v.index() == i => {
                let b = konst(amount)?;
                (b < bits).then_some((b, bits - b))
            }
            Op::Shift {
                kind: ShiftType::Lsl,
                v,
                amount,
            } if v.index() == i => {
                let (k, _) = users(i)?;
                let a = konst(amount)?;
                // The shift right's value, and its carry out if the flags
                // take it: bit `b - 1`, below the kept bits.
                let (mut b, mut out) = (None, None);
                for (_, j) in readers(k) {
                    match j.op {
                        Op::Shift {
                            kind: ShiftType::Lsr | ShiftType::Asr,
                            amount,
                            ..
                        } if b.is_none() => b = Some(konst(amount)?),
                        Op::ShiftCarry {
                            kind: ShiftType::Lsr | ShiftType::Asr,
                            amount,
                            ..
                        } if out.is_none() => out = Some(konst(amount)?),
                        Op::TestBit { n, .. } if out.is_none() => out = Some(n + 1),
                        _ => return None,
                    }
                }
                let b = b?;
                if out.is_some_and(|o| o != b) {
                    return None;
                }
                (a <= b && b < 32 && b - a < bits).then_some((b - a, 32 - b))
            }
            _ => None,
        }
    }

    /// Access `i`'s method, `name` or, with a pool load folded in,
    /// `name_via`, and the pool load's arguments that go first.
    fn via_of(&self, i: usize, name: &str) -> (Ident, TokenStream) {
        match self.via[i] {
            // Without mGBA's model the pool load is only a read of ROM.
            Some(_) if self.clock != Clock::Exact => (format_ident!("{name}"), quote!()),
            Some((at, lit)) => {
                let (at, lit) = (crate::lower::hex(at), crate::lower::hex(lit));
                (format_ident!("{name}_via"), quote!(#at, #lit,))
            }
            None => (format_ident!("{name}"), quote!()),
        }
    }

    /// The readable form's sync block: the registers and flags a block
    /// set, written before an exit (`regs!(m; 0 = x; z = y)`). Empty if
    /// none.
    pub fn sync(&self, regs: &[(usize, ValueId)], flags: &[(Flag, ValueId)]) -> TokenStream {
        if regs.is_empty() && flags.is_empty() {
            return quote!();
        }
        let r = regs.iter().map(|&(r, x)| {
            let (r, x) = (Literal::usize_unsuffixed(r), self.arg(x));
            quote!(#r = #x)
        });
        let f = flags.iter().map(|&(f, x)| {
            let (f, x) = (
                format_ident!("{}", format!("{f:?}").to_lowercase()),
                self.arg(x),
            );
            quote!(#f = #x)
        });
        if flags.is_empty() {
            quote!(regs!(m; #(#r),*);)
        } else {
            quote!(regs!(m; #(#r),*; #(#f),*);)
        }
    }

    /// The expression a value-producing op computes, with its operands
    /// printed by [`Self::val`].
    fn expr(&self, op: &Op, typed: bool) -> E {
        use Op::*;
        match *op {
            Const(x) => E::atom(word(x, typed)),
            Pool(x) => E::atom(match self.names.and_then(|n| n.data_at(x)) {
                Some((id, 0)) => {
                    let id = format_ident!("{id}");
                    quote!(addr!(#id))
                }
                Some((id, off)) => {
                    let (id, off) = (format_ident!("{id}"), word(off, false));
                    quote!(addr!(#id + #off))
                }
                // A function's key (`TaskDummy`), as a callback's word.
                None => match self.names.and_then(|n| n.ident(Kind::Func, x)) {
                    Some(id) => {
                        let id = format_ident!("{id}");
                        quote!(addr!(#id))
                    }
                    None => word(x, typed),
                },
            }),
            Bit(b) => E::atom(quote!(#b)),
            Bin(o, a, b) => {
                // A compare of a field with one of its named values
                // (`cmp`'s `wrapping_sub`, or `^`): the value by name.
                let named = |x: ValueId, by: ValueId| {
                    matches!(o, BinOp::Sub | BinOp::Xor)
                        .then(|| self.value_name(self.values_at(by.index()), x))
                        .flatten()
                };
                let method = |name: &str| {
                    let f = format_ident!("{name}");
                    let (a, b) = (
                        self.opnd(a, true),
                        named(b, a).unwrap_or_else(|| self.arg(b)),
                    );
                    E::atom(quote!(#a.#f(#b)))
                };
                let x = || named(a, b).unwrap_or_else(|| self.opnd(a, typed));
                let y = || named(b, a).unwrap_or_else(|| self.opnd(b, typed));
                match o {
                    BinOp::Add => method("wrapping_add"),
                    BinOp::Sub => method("wrapping_sub"),
                    BinOp::Mul => method("wrapping_mul"),
                    BinOp::And => {
                        let (a, b) = (x(), y());
                        E::compound(quote!(#a & #b))
                    }
                    BinOp::Or => {
                        let (a, b) = (x(), y());
                        E::compound(quote!(#a | #b))
                    }
                    BinOp::Xor => {
                        let (a, b) = (x(), y());
                        E::compound(quote!(#a ^ #b))
                    }
                }
            }
            Not(a) | BNot(a) => {
                let a = self.opnd(a, typed);
                E::compound(quote!(!#a))
            }
            MulLongLo { signed, a, b } | MulLongHi { signed, a, b } => {
                let (a, b) = (self.arg(a), self.arg(b));
                let n = Literal::usize_unsuffixed(matches!(op, MulLongHi { .. }) as usize);
                E::atom(quote!(prim::mul_long(#signed, #a, #b).#n))
            }
            Adc { a, b, c } | AdcCarry { a, b, c } | AdcOverflow { a, b, c } => {
                let (a, b, c) = (self.arg(a), self.adc_b(a, b, c), self.arg(c));
                let n = Literal::usize_unsuffixed(match op {
                    Adc { .. } => 0,
                    AdcCarry { .. } => 1,
                    _ => 2,
                });
                E::atom(quote!(prim::add_with_carry(#a, #b, #c).#n))
            }
            MulCarry {
                kind,
                a,
                b,
                acc_lo,
                acc_hi,
            } => {
                let (k, a, b) = (ident(kind), self.arg(a), self.arg(b));
                let (lo, hi) = (self.opnd(acc_lo, true), self.opnd(acc_hi, true));
                E::atom(
                    quote!(prim::mul_carry(MulKind::#k, #a, #b, (#hi as u64) << 32 | #lo as u64)),
                )
            }
            Shift { kind, v: x, amount } => match const_amount(self.insts, amount) {
                Some(n) => self.shift_by(kind, x, n, typed),
                // A rotate by the low byte is one by the amount mod 32, as
                // `rotate_right` takes it (0 leaves the value).
                None if kind == ShiftType::Ror => {
                    let (x, n) = (self.opnd(x, true), self.arg(amount));
                    E::atom(quote!(#x.rotate_right(#n)))
                }
                None => {
                    let (k, x, n) = (ident(kind), self.arg(x), self.arg(amount));
                    E::atom(quote!(prim::shift(ShiftType::#k, #x, #n, false).0))
                }
            },
            ShiftCarry {
                kind,
                v: x,
                amount,
                carry_in,
            } => match const_amount(self.insts, amount) {
                Some(n) => self.carry_by(kind, x, n, carry_in),
                None => {
                    let (k, x, n, c) = (
                        ident(kind),
                        self.arg(x),
                        self.arg(amount),
                        self.arg(carry_in),
                    );
                    E::atom(quote!(prim::shift(ShiftType::#k, #x, #n, #c).1))
                }
            },
            Rrx { v: x, carry_in } => {
                let (x, c) = (self.arg(x), self.arg(carry_in));
                E::atom(quote!(prim::rrx(#x, #c).0))
            }
            Sext { v: x, bits } => {
                let (x, bits) = (self.arg(x), Literal::u32_unsuffixed(bits));
                E::atom(quote!(prim::sext(#x, #bits)))
            }
            Select { c, a, b } | BSelect { c, a, b } => {
                let (c, a, b) = (self.arg(c), self.val(a, typed).t, self.val(b, typed).t);
                E::compound(quote!(if #c { #a } else { #b }))
            }
            BitToWord(b) => {
                let b = self.opnd(b, false);
                E::compound(quote!(#b as u32))
            }
            TestBit { v: x, n } => bit(self.opnd(x, true), n),
            IsZero(x) => {
                let x = self.opnd(x, true);
                E::compound(quote!(#x == 0))
            }
            BBin(o, a, b) => {
                let (a, b) = (self.opnd(a, false), self.opnd(b, false));
                E::compound(match o {
                    BitOp::And => quote!(#a & #b),
                    BitOp::Or => quote!(#a | #b),
                    BitOp::Xor => quote!(#a ^ #b),
                })
            }
            GetReg(r) if self.carried & 1 << r.index() != 0 => {
                let r = format_ident!("r{}", r.index());
                E::atom(quote!(#r))
            }
            GetReg(r) => match self.args.get(r.index()) {
                Some(Some((id, Some(_)))) => E::atom(quote!(#id.0)),
                Some(Some((id, None))) => E::atom(quote!(#id)),
                _ => E::atom(self.loc.reg(r.index())),
            },
            GetUserReg(r) => {
                let n = Literal::u8_unsuffixed(r.0);
                self.synced(Point::Read, quote!(m.cpu.user_reg(Reg(#n))))
            }
            GetFlag(f) => match self.loc.flag(f) {
                Some(x) => E::atom(x),
                None => {
                    let f = ident(f);
                    E::atom(quote!(m.cpu.flag(Flag::#f)))
                }
            },
            GetCpsr => E::atom(self.loc.cpsr()),
            GetSpsr => self.synced(Point::Read, quote!(m.cpu.spsr())),
            Load { width, addr } => {
                let (w, a) = (ident(width), self.address(width, addr));
                E::atom(quote!(m.bus.load(Width::#w, #a)))
            }
            _ => unreachable!("not a value: {op:?}"),
        }
    }

    /// An access's aligned address.
    fn address(&self, w: Width, addr: ValueId) -> TokenStream {
        let m = mask(w);
        if m.is_empty() {
            self.arg(addr)
        } else {
            let a = self.opnd(addr, false);
            quote!(#a #m)
        }
    }

    /// `prim::shift(kind, x, n, _).0` for a constant `n` (its low byte).
    fn shift_by(&self, kind: ShiftType, x: ValueId, n: u32, typed: bool) -> E {
        use ShiftType::*;
        let k = Literal::u32_unsuffixed(n);
        match (kind, n) {
            (_, 0) => self.val(x, typed),
            (Lsl, 1..=31) => {
                let x = self.opnd(x, typed);
                E::compound(quote!(#x << #k))
            }
            (Lsr, 1..=31) => {
                let x = self.opnd(x, typed);
                E::compound(quote!(#x >> #k))
            }
            (Lsl | Lsr, _) => E::atom(word(0, typed)),
            (Asr, _) => {
                let (x, k) = (self.opnd(x, true), Literal::u32_unsuffixed(n.min(31)));
                E::compound(quote!(((#x as i32) >> #k) as u32))
            }
            (Ror, _) if n & 31 == 0 => self.val(x, typed),
            (Ror, _) => {
                let (x, k) = (self.opnd(x, true), Literal::u32_unsuffixed(n & 31));
                E::atom(quote!(#x.rotate_right(#k)))
            }
        }
    }

    /// `prim::shift(kind, x, n, carry_in).1` for a constant `n`.
    fn carry_by(&self, kind: ShiftType, x: ValueId, n: u32, carry_in: ValueId) -> E {
        use ShiftType::*;
        let i = match (kind, n) {
            (_, 0) => return self.val(carry_in, false),
            (Lsl, 1..=32) => 32 - n,
            (Lsr, 1..=32) => n - 1,
            (Lsl | Lsr, _) => return E::atom(quote!(false)),
            (Asr, _) => n.min(32) - 1,
            (Ror, _) => (n + 31) & 31,
        };
        bit(self.opnd(x, true), i)
    }

    /// The statement that binds the tuple placed at inst `i`, if any.
    pub fn tuple_before(&self, i: usize) -> Option<TokenStream> {
        let t = &self.tuples[self.tuple_at[i]?];
        let n = if matches!(t.key, Key::Adc(..)) { 3 } else { 2 };
        let names = t.slots[..n].iter().map(|s| match s {
            Some(m) => {
                let id = self.id(ValueId(*m as u32));
                quote!(#id)
            }
            None => quote!(_),
        });
        let call = match t.key {
            Key::Adc(a, b, c) => {
                let (a, b, c) = (self.arg(a), self.adc_b(a, b, c), self.arg(c));
                quote!(prim::add_with_carry(#a, #b, #c))
            }
            Key::MulLong(signed, a, b) => {
                let (a, b) = (self.arg(a), self.arg(b));
                quote!(prim::mul_long(#signed, #a, #b))
            }
            Key::Shift(kind, x, n) => {
                let c = match t.carry_in(self.insts) {
                    Some(c) => self.arg(c),
                    None => quote!(false),
                };
                let (k, x, n) = (ident(kind), self.arg(x), self.arg(n));
                quote!(prim::shift(ShiftType::#k, #x, #n, #c))
            }
        };
        Some(quote!(let (#(#names),*) = #call;))
    }

    /// The statement that prints inst `i`, if it is a value (`None` for
    /// effects, exits and markers; empty if it is printed elsewhere or
    /// not at all).
    pub fn value(&self, i: usize) -> Option<TokenStream> {
        let inst = &self.insts[i];
        let op = &inst.op;
        if op.is_root() && !matches!(op, Op::Load { .. }) {
            return None;
        }
        if self.form[i] != Form::Bound || self.tuple_of[i].is_some() {
            return Some(quote!());
        }
        let e = self.expr_at(i, false).t;
        let guard = inst.guard.map(|g| self.guard(g));
        // A pool word's read has no effect but its time.
        if self.clock != Clock::Exact && self.is_pool(i) {
            return Some(quote!());
        }
        // A load nothing uses is still a bus access.
        if self.uses[i] == 0 {
            return Some(match guard {
                Some(g) => quote!(if #g { #e; }),
                None => quote!(#e;),
            });
        }
        let id = self.id(ValueId(i as u32));
        let (ty, zero) = if is_bit(op) {
            (quote!(bool), quote!(false))
        } else {
            (quote!(u32), quote!(0))
        };
        Some(match guard {
            Some(g) => quote!(let #id: #ty = if #g { #e } else { #zero };),
            None => quote!(let #id: #ty = #e;),
        })
    }

    /// A guard, as an `if`'s condition.
    pub fn guard(&self, g: ValueId) -> TokenStream {
        let e = self.val(g, false);
        if matches!(
            self.insts[g.index()].op,
            Op::Select { .. } | Op::BSelect { .. }
        ) && self.form[g.index()] == Form::Inline
        {
            e.operand()
        } else {
            e.t
        }
    }

    /// The statement an effect performs (exits and markers aside).
    pub fn effect(&self, i: usize) -> TokenStream {
        use Op::*;
        let op = &self.insts[i].op;
        match *op {
            SetReg(r, x) => self.loc.set_reg(r.index(), self.arg(x)),
            SetUserReg(r, x) => {
                let n = Literal::u8_unsuffixed(r.0);
                self.around(
                    Point::Psr,
                    self.arg(x),
                    |x| quote!(m.cpu.set_user_reg(Reg(#n), #x);),
                )
            }
            SetFlag(..) if self.in_run[i] => match &self.flag_run[i] {
                Some(run) => self.flags(run),
                None => quote!(),
            },
            SetFlag(f, x) => match self.loc.flag(f) {
                Some(id) => {
                    let x = self.arg(x);
                    quote!(#id = #x;)
                }
                None => {
                    let (f, x) = (ident(f), self.arg(x));
                    quote!(m.cpu.set_flag(Flag::#f, #x);)
                }
            },
            // Printed on its exit's yield path, by `yield_flags`.
            YieldFlag(..) => quote!(),
            SetCpsr { v: x, mask } => {
                let k = Literal::u8_unsuffixed(mask.0);
                self.around(
                    Point::Psr,
                    self.arg(x),
                    |x| quote!(m.cpu.msr_cpsr(#x, PsrMask(#k));),
                )
            }
            SetSpsr { v: x, mask } => {
                let k = Literal::u8_unsuffixed(mask.0);
                self.around(
                    Point::Psr,
                    self.arg(x),
                    |x| quote!(m.cpu.msr_spsr(#x, PsrMask(#k));),
                )
            }
            Store { width, addr, value } if let Some(at) = self.fused[i] => {
                let x = self
                    .value_name(self.values_at(i), value)
                    .unwrap_or_else(|| self.arg(value));
                let (at, p) = (self.at(at), self.ptr(i, width, addr));
                let (set, via) = self.via_of(i, "set");
                quote!(m.#set(#via #at #p, #x);)
            }
            Store { width, addr, value } => {
                let (w, a) = (ident(width), self.address(width, addr));
                let x = self.arg(value);
                quote!(m.bus.store(Width::#w, #a, #x);)
            }
            _ => unreachable!("not an effect: {op:?}"),
        }
    }

    /// Statement `s` of argument `x` with what point `p` needs around
    /// it: the argument computed first (it may itself read the CPU).
    fn around(
        &self,
        p: Point,
        x: TokenStream,
        s: impl Fn(TokenStream) -> TokenStream,
    ) -> TokenStream {
        let a = self.loc.sync(p);
        let (before, after) = (a.before, a.after);
        if before.is_empty() && after.is_empty() {
            return s(x);
        }
        let s = s(quote!(x));
        quote!({ let x = #x; #before #s #after })
    }

    /// Expression `e` with what point `p` needs before it (a point that
    /// reloads nothing).
    fn synced(&self, p: Point, e: TokenStream) -> E {
        let a = self.loc.sync(p);
        debug_assert!(a.after.is_empty());
        let before = a.before;
        if before.is_empty() {
            return E::atom(e);
        }
        E::atom(quote!({ #before #e }))
    }

    /// A run of flag writes as one write of the CPSR's flag bits:
    /// `Cpu::set_flag`'s, with every value computed first.
    fn flags(&self, run: &[usize]) -> TokenStream {
        self.cpsr_flags(run.iter().map(|&i| {
            let Op::SetFlag(f, x) = self.insts[i].op else {
                unreachable!()
            };
            (f, x)
        }))
    }

    /// What an exit's yield path writes: the [`Op::YieldFlag`]s at
    /// `yields`, as one write of the CPSR's flag bits (none if empty).
    pub fn yield_flags(&self, yields: &[usize]) -> TokenStream {
        if yields.is_empty() {
            return quote!();
        }
        if self.loc.flags {
            // Into the locals; the yield path writes them back.
            return yields
                .iter()
                .map(|&i| {
                    let Op::YieldFlag(f, x) = self.insts[i].op else {
                        unreachable!()
                    };
                    let (id, x) = (self.loc.flag(f).unwrap(), self.arg(x));
                    quote!(#id = #x;)
                })
                .collect();
        }
        self.cpsr_flags(yields.iter().map(|&i| {
            let Op::YieldFlag(f, x) = self.insts[i].op else {
                unreachable!()
            };
            (f, x)
        }))
    }

    /// Flag values written into the CPSR at once.
    fn cpsr_flags(&self, writes: impl Iterator<Item = (Flag, ValueId)>) -> TokenStream {
        let (mut clear, mut set, mut bits) = (0u32, 0u32, Vec::new());
        for (f, x) in writes {
            let b = f.cpsr_bit();
            clear |= 1 << b;
            match self.insts[x.index()].op {
                Op::Bit(true) => set |= 1 << b,
                Op::Bit(false) => {}
                _ => {
                    let (x, b) = (self.opnd(x, false), Literal::u32_unsuffixed(b));
                    bits.push(quote!((#x as u32) << #b));
                }
            }
        }
        let clear = word(!clear, false);
        let set = (set != 0).then(|| word(set, false)).into_iter();
        quote!(m.cpu.cpsr = m.cpu.cpsr & #clear #(| #set)* #(| #bits)*;)
    }
}

/// A path's steps as accessor calls: `.data().idx(3)`.
fn path(steps: &[Step]) -> TokenStream {
    steps
        .iter()
        .map(|s| match s {
            Step::Field(f) => {
                let f = format_ident!("{f}");
                quote!(.#f())
            }
            Step::Idx(k) => {
                let k = Literal::u32_unsuffixed(*k);
                quote!(.idx(#k))
            }
        })
        .collect()
}

/// What a path leaves of an access as a raw field: `.field::<u8>(1)`.
fn rest_of(width: Width, rest: Option<u32>) -> TokenStream {
    rest.map(|o| {
        let (t, o) = (ptr_ty(width), Literal::u32_unsuffixed(o));
        quote!(.field::<#t>(#o))
    })
    .unwrap_or_default()
}

/// The type a readable access's `Ptr` points at, for its width.
fn ptr_ty(w: Width) -> TokenStream {
    match w {
        Width::Byte => quote!(u8),
        Width::Half => quote!(u16),
        Width::Word => quote!(u32),
    }
}

/// `(x >> i) & 1 != 0`.
fn bit(x: TokenStream, i: u32) -> E {
    if i == 0 {
        return E::compound(quote!(#x & 1 != 0));
    }
    let i = Literal::u32_unsuffixed(i);
    E::compound(quote!((#x >> #i) & 1 != 0))
}

fn is_read(op: &Op) -> bool {
    use Op::*;
    matches!(
        op,
        GetReg(_) | GetUserReg(_) | GetFlag(_) | GetCpsr | GetSpsr
    )
}

/// Whether running `w` may change what `read` reads (or anything: an exit
/// leaves the block).
fn changes(w: &Op, read: &Op) -> bool {
    use Op::*;
    match (w, read) {
        // A mode change rebanks everything.
        (SetCpsr { .. } | Exit(_), _) => true,
        (SetReg(a, _), GetReg(b)) => a == b,
        // The user bank and the current one may be the same registers.
        (SetReg(..) | SetUserReg(..), GetUserReg(_)) | (SetUserReg(..), GetReg(_)) => true,
        (SetFlag(a, _), GetFlag(b)) => a == b,
        (SetFlag(..), GetCpsr) => true,
        // In user and system mode the SPSR read is the CPSR.
        (SetSpsr { .. } | SetFlag(..), GetSpsr) => true,
        _ => false,
    }
}

/// Runs of flag writes printed as one CPSR write, each at its last
/// write: two or more under one guard, of different flags, with only
/// pure values between them. The values move after whatever is between,
/// and the writes after the values, so nothing between may read the CPU
/// or touch the bus.
fn flag_runs(insts: &[Inst], stall_before: &[bool]) -> Vec<Option<Vec<usize>>> {
    let mut runs = vec![None; insts.len()];
    let mut i = 0;
    while i < insts.len() {
        let Op::SetFlag(..) = insts[i].op else {
            i += 1;
            continue;
        };
        let (guard, mut run, mut flags) = (insts[i].guard, vec![i], 0u8);
        let mut k = i;
        loop {
            if let Op::SetFlag(f, _) = insts[k].op {
                flags |= 1 << f.index();
            }
            k += 1;
            let Some(next) = insts.get(k) else { break };
            if stall_before[k] {
                break;
            }
            match next.op {
                Op::SetFlag(f, _) if next.guard == guard && flags & 1 << f.index() == 0 => {
                    run.push(k)
                }
                ref op if is_read(op) => break,
                ref op if op.is_root() => break,
                _ => {}
            }
        }
        let last = *run.last().unwrap();
        if run.len() > 1 {
            runs[last] = Some(run);
        }
        i = last + 1;
    }
    runs
}

/// The operands `op`'s printed form names: a shift by a constant may drop
/// its value or its carry in, and leaves out the amount (a literal).
fn printed_operands(insts: &[Inst], op: &Op) -> Vec<ValueId> {
    use ShiftType::*;
    match *op {
        Op::Shift { kind, v, amount } => match const_amount(insts, amount) {
            None => vec![v, amount],
            Some(n) if n >= 32 && matches!(kind, Lsl | Lsr) => vec![],
            Some(_) => vec![v],
        },
        Op::ShiftCarry {
            kind,
            v,
            amount,
            carry_in,
        } => match const_amount(insts, amount) {
            None => vec![v, amount, carry_in],
            Some(0) => vec![carry_in],
            Some(n) if n > 32 && matches!(kind, Lsl | Lsr) => vec![],
            Some(_) => vec![v],
        },
        // The exit's target is printed (or not) by its plan; counting it
        // keeps a value it names bound.
        _ => op.operands(),
    }
}
