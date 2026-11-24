//! Counted loops to a bound that isn't a constant, printed as `for`
//! (unit `native/lift-for-bounds`).
//!
//! [`super::counted`] runs a loop of one block from constants. Most C
//! loops count to a parameter or start from a value: `for (i = 0; i < n;
//! i++)` compiles (agbcc and GCC alike) to a test of the range before the
//! loop, then a body of any number of blocks whose last, the latch, steps
//! the counter and jumps back while it is in range. Here the proof is
//! symbolic: each value of a block as `narrow(base + add)` ([`Sym`]) over
//! registers at a point, the latch's over the registers at the loop's
//! start, the block that enters it over its own start's.
//!
//! What [`prove`] asks, given the loop's facts ([`Loop`]):
//! - the latch writes the counter once, as itself plus or minus 1, as a
//!   `u32` or narrowed to a `u8` or `u16` (`(i + 1) as u8`), directly or
//!   through a register an earlier block of the round set ([`Loop::via`];
//!   unit `native/lift-for-step-via`), and jumps
//!   back while the new value is below the bound (above it counting
//!   down), or not equal to it; the bound a constant or a register
//!   nothing in the loop writes, which may be narrowed too;
//! - the way in leaves the counter its start, and its guards (each exit
//!   before it not taken, its own taken) prove the range isn't empty: the
//!   start below the bound (above it counting down), as the same compare
//!   or one that implies it ([`implies`]);
//! - no value wraps: a narrow counter's bound fits its width, as a
//!   constant or a value narrowed as wide; a signed compare's range has
//!   no negative value, so the `u32` range is the same.
//!
//! The test may instead be a latch exit before an unguarded jump back
//! (agbcc's `bge out; b back`), round on while it isn't taken; or the
//! loop's start's, of the counter as a round begins (unit
//! `native/lift-for-top-tested`, [`Test::Top`]). Then the way in needs
//! no guard: an empty range leaves at once, as the `for` does, which the
//! range must then say for every start and bound: a signed start not
//! negative, a signed bound that may be ends at `(b as i32).max(0)`, a
//! `!=` only between constants in order, and a bound counted down to
//! never the type's greatest (`b + 1`).
//!
//! The range prints as `s..b` (or `(b + 1..=s).rev()` counting down): the
//! start a constant or the counter's register itself (the binding
//! shadows it only inside the loop), the bound a constant or its
//! register.

use super::cond::{self, Opnd, Rel};
use analysis::ir::{BinOp, Inst, Op};
use armv4t::sem::ShiftType;

/// A value: `narrow(base + add)`, where `base` is a register's value at
/// the block's reference point (0 to 15), a pure value of the block's
/// own it knows no more of (16 and up: value `base - 16`), or nothing (a
/// constant `add`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sym {
    pub base: Option<usize>,
    pub add: u32,
    /// The low `bits` bits, sign-extended if `.1`.
    pub narrow: Option<(u32, bool)>,
}

impl Sym {
    pub fn konst(k: u32) -> Sym {
        Sym {
            base: None,
            add: k,
            narrow: None,
        }
    }

    pub fn reg(r: usize) -> Sym {
        Sym {
            base: Some(r),
            add: 0,
            narrow: None,
        }
    }

    pub fn as_const(&self) -> Option<u32> {
        match self.base {
            None => Some(self.add),
            Some(_) => None,
        }
    }

    fn plus(&self, k: u32) -> Option<Sym> {
        if k == 0 {
            return Some(self.clone());
        }
        match (self.base, self.narrow) {
            (None, _) => Some(Sym::konst(self.add.wrapping_add(k))),
            (Some(_), None) => Some(Sym {
                add: self.add.wrapping_add(k),
                ..self.clone()
            }),
            _ => None,
        }
    }

    fn narrowed(&self, bits: u32, signed: bool) -> Option<Sym> {
        if let Some(k) = self.as_const() {
            let m = (1u32 << bits) - 1;
            let v = k & m;
            let v = if signed && v >> (bits - 1) != 0 {
                v | !m
            } else {
                v
            };
            return Some(Sym::konst(v));
        }
        match self.narrow {
            None => Some(Sym {
                narrow: Some((bits, signed)),
                ..self.clone()
            }),
            // Already zero-extended from as few bits or fewer.
            Some((b, false)) if b <= bits => Some(self.clone()),
            Some((b, false)) if !signed => Some(Sym {
                narrow: Some((bits.min(b), false)),
                ..self.clone()
            }),
            Some((b, true)) if b == bits && signed => Some(self.clone()),
            _ => None,
        }
    }

    /// The most it can be, as a `u32`, where that is less than `u32::MAX`
    /// by being a constant or zero-extended from fewer bits.
    pub fn max(&self) -> Option<u32> {
        match (self.base, self.narrow) {
            (None, _) => Some(self.add),
            (Some(_), Some((b, false))) if b < 32 => Some((1u32 << b) - 1),
            _ => None,
        }
    }

    /// Whether it is never negative as an `i32`.
    pub fn non_negative(&self) -> bool {
        self.max().is_some_and(|m| (m as i32) >= 0)
    }
}

/// Each value of `insts[..end]` as a [`Sym`], where it is one, from the
/// registers `regs` holds at the start; `regs` is left as the insts leave
/// it.
pub fn eval(insts: &[Inst], regs: &mut [Option<Sym>; 16], end: usize) -> Vec<Option<Sym>> {
    let mut vals: Vec<Option<Sym>> = vec![None; end];
    let konst = |vals: &[Option<Sym>], x: analysis::ir::ValueId| {
        vals[x.index()].as_ref().and_then(Sym::as_const)
    };
    for i in 0..end {
        let inst = &insts[i];
        let get = |x: analysis::ir::ValueId| vals[x.index()].clone();
        // a - b: b a constant, or both the same base plus constants.
        let sub = |a: Option<Sym>, b: Option<Sym>| -> Option<Sym> {
            let (a, b) = (a?, b?);
            match b.as_const() {
                Some(k) => a.plus(k.wrapping_neg()),
                None if a.base == b.base && a.narrow.is_none() && b.narrow.is_none() => {
                    Some(Sym::konst(a.add.wrapping_sub(b.add)))
                }
                None => None,
            }
        };
        let add = |a: Option<Sym>, b: Option<Sym>| -> Option<Sym> {
            let (a, b) = (a?, b?);
            match (a.as_const(), b.as_const()) {
                (_, Some(k)) => a.plus(k),
                (Some(k), None) => b.plus(k),
                _ => None,
            }
        };
        vals[i] = match inst.op {
            Op::Const(k) | Op::Pool(k) => Some(Sym::konst(k)),
            Op::GetReg(q) => regs[q.index()].clone(),
            Op::SetReg(q, x) => {
                regs[q.index()] = if inst.guard.is_none() { get(x) } else { None };
                None
            }
            Op::SetCpsr { .. } | Op::SetUserReg(..) => {
                *regs = Default::default();
                None
            }
            Op::Not(x) => konst(&vals, x).map(|k| Sym::konst(!k)),
            // a + b + carry; a subtraction is a + !k + 1.
            Op::Adc { a, b, c } => match (insts[c.index()].op, insts[b.index()].op) {
                (Op::Bit(true), Op::Not(y)) => sub(get(a), get(y)),
                (Op::Bit(carry), _) => add(get(a), get(b)).and_then(|s| s.plus(u32::from(carry))),
                _ => None,
            },
            Op::Bin(BinOp::Add, a, b) => add(get(a), get(b)),
            Op::Bin(BinOp::Sub, a, b) => sub(get(a), get(b)),
            Op::Bin(BinOp::And, a, b) => {
                let (x, m) = match (konst(&vals, a), konst(&vals, b)) {
                    (None, Some(m)) => (a, m),
                    (Some(m), None) => (b, m),
                    (Some(p), Some(q)) => {
                        vals[i] = Some(Sym::konst(p & q));
                        continue;
                    }
                    _ => {
                        vals[i] = None;
                        continue;
                    }
                };
                match m {
                    0xff => get(x).and_then(|s| s.narrowed(8, false)),
                    0xffff => get(x).and_then(|s| s.narrowed(16, false)),
                    _ => None,
                }
            }
            Op::Shift { kind, v, amount } => {
                let c = konst(&vals, amount);
                match (kind, insts[v.index()].op, c) {
                    (
                        ShiftType::Lsr | ShiftType::Asr,
                        Op::Shift {
                            kind: ShiftType::Lsl,
                            v: x,
                            amount: b,
                        },
                        Some(c @ (16 | 24)),
                    ) if konst(&vals, b) == Some(c) => {
                        get(x).and_then(|s| s.narrowed(32 - c, kind == ShiftType::Asr))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        // Any other pure value, or a load's (fixed once loaded): itself.
        if vals[i].is_none()
            && (super::pure(&inst.op) || matches!(inst.op, Op::Load { .. }))
            && !matches!(inst.op, Op::GetReg(_) | Op::GetFlag(_))
        {
            vals[i] = Some(Sym {
                base: Some(16 + i),
                add: 0,
                narrow: None,
            });
        }
    }
    vals
}

/// A compare in a normal form: `l rel r` with `rel` one of `<`, `<=`,
/// `==`, `!=` (`>` and `>=` mirrored), `<=` against a constant made `<`,
/// and `==`, `!=` unsigned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmp {
    pub l: Sym,
    pub rel: Rel,
    pub r: Sym,
    pub signed: bool,
}

impl Cmp {
    pub fn new(l: Sym, rel: Rel, r: Sym, signed: bool) -> Cmp {
        let (l, rel, r) = match rel {
            Rel::Gt => (r, Rel::Lt, l),
            Rel::Ge => (r, Rel::Le, l),
            _ => (l, rel, r),
        };
        let (min, max) = if signed {
            (0x8000_0000, 0x7fff_ffff)
        } else {
            (0, u32::MAX)
        };
        let (l, rel, r) = match (rel, l.as_const(), r.as_const()) {
            (Rel::Le, _, Some(k)) if k != max => (l, Rel::Lt, Sym::konst(k.wrapping_add(1))),
            (Rel::Le, Some(k), _) if k != min => (Sym::konst(k.wrapping_sub(1)), Rel::Lt, r),
            _ => (l, rel, r),
        };
        let signed = signed && matches!(rel, Rel::Lt | Rel::Le);
        Cmp { l, rel, r, signed }
    }

    /// A value of a block's as a compare, by `forms` (its [`cond::forms`])
    /// and `vals` (its [`eval`]): `None` where either side isn't a
    /// [`Sym`].
    pub fn of(
        forms: &[Option<cond::Form>],
        vals: &[Option<Sym>],
        x: analysis::ir::ValueId,
        not: bool,
    ) -> Option<Cmp> {
        let (l, rel, r, signed) = forms.get(x.index())?.as_ref()?.compare()?;
        let side = |o: Opnd| match o {
            Opnd::Const(k) => Some(Sym::konst(k)),
            Opnd::Val(v) => vals.get(v.index())?.clone(),
        };
        let rel = if not { rel.negated() } else { rel };
        Some(Cmp::new(side(l)?, rel, side(r)?, signed))
    }

    /// Whether it holds, both sides constants.
    fn decided(&self) -> Option<bool> {
        let (a, b) = (self.l.as_const()?, self.r.as_const()?);
        Some(if self.signed {
            self.rel.holds(a as i32, b as i32)
        } else {
            self.rel.holds(a, b)
        })
    }
}

/// Whether `fact` holding proves `need`.
pub fn implies(fact: &Cmp, need: &Cmp) -> bool {
    if fact == need {
        return true;
    }
    let swapped = |c: &Cmp| Cmp {
        l: c.r.clone(),
        r: c.l.clone(),
        ..c.clone()
    };
    if matches!(fact.rel, Rel::Eq | Rel::Ne) && swapped(fact) == *need {
        return true;
    }
    if need.rel != Rel::Lt {
        return false;
    }
    let same_sides = fact.l == need.l && fact.r == need.r;
    // Both sides never negative: signed and unsigned agree.
    let both_sure = need.l.non_negative() && need.r.non_negative();
    match (need.signed, fact.rel, fact.signed) {
        (_, Rel::Lt, _) if same_sides && both_sure => true,
        // l < r as i32, l a constant not negative: both not negative.
        (false, Rel::Lt, true) if same_sides => need.l.as_const().is_some_and(|k| (k as i32) >= 0),
        // 0 < r: r != 0 (as an i32, r never negative).
        (signed, Rel::Ne, _)
            if need.l.as_const() == Some(0) && (!signed || need.r.non_negative()) =>
        {
            (fact.l == need.r && fact.r.as_const() == Some(0))
                || (fact.r == need.r && fact.l.as_const() == Some(0))
        }
        // l < r as u32, r a constant not negative as an i32.
        (true, Rel::Lt, false) if same_sides => need.r.as_const().is_some_and(|k| (k as i32) > 0),
        _ => false,
    }
}

/// Where a loop tests its range ([`Loop`]'s `test`).
#[derive(Clone, Copy, Debug)]
pub enum Test<'a> {
    /// The latch's jump back, guarded: round on while it is taken.
    Back,
    /// The latch's exit at this index, guarded, before its jump back
    /// (unguarded): round on while it isn't taken. Of the stepped value,
    /// as [`Test::Back`].
    Exit(usize),
    /// The loop's start block (`insts`), the index of its exit, guarded,
    /// and whether that exit leaves the loop: round on while it isn't
    /// taken, or else while it is (unit `native/lift-for-top-tested`). Of the counter as the round starts,
    /// not the stepped value; a round of the range may be none, so the
    /// way in needs no guard, but the range must say so as the loop does.
    Top(&'a [Inst], usize, bool),
}

/// What [`prove`] needs of a loop, gathered by the printer.
pub struct Loop<'a> {
    /// The counter's register.
    pub reg: usize,
    /// The latch's insts and the index of its jump back.
    pub latch: &'a [Inst],
    pub back: usize,
    /// The block that enters the loop, and the index of its jump in.
    pub way_in: &'a [Inst],
    pub enter: usize,
    /// The registers nothing in the loop writes (bit `n` for rN).
    pub kept: u16,
    /// Where the loop tests its range.
    pub test: Test<'a>,
    /// Registers the latch starts with a known value of, over the
    /// registers at the loop's start: each written once a round, in a
    /// block on every way to the latch (agbcc's `r9 = r8 + 1` early in
    /// the body, `r8 = r9 as u8` in the latch; unit
    /// `native/lift-for-step-via`).
    pub via: &'a [(usize, Sym)],
}

/// A proven range: the index of the latch's step, and the range's Rust;
/// the counter's start where it is a constant, and its step a round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proven {
    pub step: usize,
    pub range: String,
    pub start: Option<u32>,
    pub by: i32,
}

fn text(s: &Sym, reg: usize) -> Option<String> {
    if let Some(k) = s.as_const() {
        return Some(crate::print::word(k, false).to_string());
    }
    let q = s.base?;
    if q == reg || q >= 16 || s.add != 0 {
        return None;
    }
    Some(match s.narrow {
        None => format!("r{q}"),
        Some((8, false)) => format!("(r{q} as u8 as u32)"),
        Some((16, false)) => format!("(r{q} as u16 as u32)"),
        Some(_) => return None,
    })
}

/// The range of [`Loop`]'s counter, round by round, where it is proven
/// ([`self`]).
pub fn prove(l: &Loop) -> Option<Proven> {
    why(l).ok()
}

/// [`prove`], or why not: what failed first.
pub fn why(l: &Loop) -> Result<Proven, &'static str> {
    let r = l.reg;
    // The latch over the registers at the loop's start: the counter and
    // those nothing in the loop writes.
    let mut regs: [Option<Sym>; 16] = Default::default();
    for (q, s) in regs.iter_mut().enumerate() {
        if q == r || l.kept & 1 << q != 0 {
            *s = Some(Sym::reg(q));
        }
    }
    let at_start = regs.clone();
    for (q, s) in l.via {
        if *q != r {
            regs[*q] = Some(s.clone());
        }
    }
    let writes: Vec<usize> = (0..l.back)
        .filter(|&i| matches!(l.latch[i].op, Op::SetReg(q, _) if q.index() == r))
        .collect();
    let [step] = writes[..] else {
        return Err("not one step");
    };
    if l.latch[step].guard.is_some() {
        return Err("step guarded");
    }
    let vals = eval(l.latch, &mut regs, l.back);
    let next = regs[r].clone().ok_or("step not a sum")?;
    if next.base != Some(r) {
        return Err("step not of the counter");
    }
    // The step: up or down by `by`, at most 0x10000.
    let c = next.add;
    let up = (c as i32) > 0;
    let by = (c as i32).unsigned_abs();
    if c == 0 || by > 0x10000 {
        return Err("step too large");
    }
    let mask = match next.narrow {
        None => u32::MAX,
        Some((8, false)) => 0xff,
        Some((16, false)) => 0xffff,
        Some(_) => return Err("counter signed narrow"),
    };
    // Round on while `cont`: of the stepped value (`cur` none), or, the
    // start testing it, of the counter as a round starts.
    let (cont, cur) = match l.test {
        Test::Back => {
            let guard = l.latch[l.back].guard.ok_or("jump back unguarded")?;
            let forms = cond::forms(&l.latch[..=l.back]);
            let c = Cmp::of(&forms, &vals, guard, false).ok_or("jump back not a compare")?;
            (c, None)
        }
        Test::Exit(e) => {
            let guard = l.latch[e].guard.ok_or("exit unguarded")?;
            let forms = cond::forms(&l.latch[..=e]);
            let c = Cmp::of(&forms, &vals, guard, true).ok_or("exit not a compare")?;
            (c, None)
        }
        Test::Top(insts, e, leaves) => {
            let guard = insts[e].guard.ok_or("start's exit unguarded")?;
            let mut regs = at_start;
            let hvals = eval(insts, &mut regs, e);
            let forms = cond::forms(&insts[..=e]);
            let c = Cmp::of(&forms, &hvals, guard, leaves).ok_or("start's exit not a compare")?;
            (c, Some(Sym::reg(r)))
        }
    };
    let top = cur.is_some();
    // The counter's side and the bound's, as `next rel b` (`at_left`) or
    // `b rel next`.
    let ours = |s: &Sym| s.base == Some(r);
    let (x, b, at_left) = match (ours(&cont.l), ours(&cont.r)) {
        (true, false) => (cont.l.clone(), cont.r.clone(), true),
        (false, true) => (cont.r.clone(), cont.l.clone(), false),
        _ => return Err("compare not of the counter and a bound"),
    };
    // The start's test may read the counter narrowed as wide as its
    // values are, or wider: the same values.
    let counter = |x: &Sym| match (&cur, x.narrow) {
        (None, _) => *x == next,
        (Some(c), None) => x == c,
        (Some(c), Some((bits, false))) => {
            bits < 32 && x.base == c.base && x.add == 0 && mask < (1u32 << bits)
        }
        _ => false,
    };
    let b = if counter(&x) {
        b
    } else if top {
        return Err("compare not of the counter");
    } else if cont.rel == Rel::Ne && next.narrow.is_none() && x.narrow.is_none() {
        // x is next + (x - next): x != b is next != b - (x - next).
        b.plus(next.add.wrapping_sub(x.add))
            .ok_or("bound not a sum")?
    } else {
        return Err("compare not of the step");
    };
    // The bound: a constant, or a register nothing in the loop writes.
    if let Some(q) = b.base
        && (q == r || q >= 16 || l.kept & 1 << q == 0 || b.add != 0)
    {
        return Err("bound written in the loop");
    }
    let signed = match (cont.rel, at_left, up) {
        // Past a bound it may step over: no.
        (Rel::Ne, _, _) if by != 1 => return Err("not equal by a step of more than 1"),
        (Rel::Ne, _, _) => false,
        (Rel::Lt, true, true) | (Rel::Lt, false, false) => cont.signed,
        _ => return Err("compare the wrong way"),
    };
    // The way in: the counter's start, the bound there, and its guards.
    let mut regs: [Option<Sym>; 16] = std::array::from_fn(|q| Some(Sym::reg(q)));
    let pvals = eval(l.way_in, &mut regs, l.enter);
    let s = regs[r].clone().ok_or("start unknown")?;
    let bp = match b.base {
        None => b.clone(),
        Some(q) => {
            let v = regs[q].clone().ok_or("bound unknown at the way in")?;
            match b.narrow {
                None => v,
                Some((n, sg)) => v.narrowed(n, sg).ok_or("bound unknown at the way in")?,
            }
        }
    };
    let need = if up {
        Cmp::new(s.clone(), Rel::Lt, bp.clone(), signed)
    } else {
        Cmp::new(bp.clone(), Rel::Lt, s.clone(), signed)
    };
    let shown = top
        || need.decided().unwrap_or_else(|| {
            let pforms = cond::forms(&l.way_in[..=l.enter]);
            let mut facts = Vec::new();
            for (i, inst) in l.way_in[..=l.enter].iter().enumerate() {
                let (Op::Exit(_), Some(g)) = (inst.op, inst.guard) else {
                    continue;
                };
                facts.extend(Cmp::of(&pforms, &pvals, g, i != l.enter));
            }
            facts.iter().any(|f| implies(f, &need))
        });
    if !shown {
        return Err("range not shown non-empty");
    }
    // No wrap: a narrow counter's values fit its width.
    if mask != u32::MAX {
        let (hi, lo) = if up { (&bp, &s) } else { (&s, &bp) };
        // Counting up, next reaches the bound; down, it starts at s.
        if hi.max().is_none_or(|m| m > mask) || lo.max().is_none_or(|m| m > mask) {
            return Err("narrow counter may wrap");
        }
        if signed && !lo.non_negative() {
            return Err("narrow counter may wrap");
        }
    }
    // A step of more than 1 doesn't wrap past the bound: counting up,
    // the bound at most the type's greatest less the step; down, at
    // least its least plus it (a signed bound is -1 or more, below).
    if by != 1 {
        if mask != u32::MAX {
            return Err("narrow counter by a step of more than 1");
        }
        let max = if signed { 0x7fff_ffff } else { u32::MAX };
        let ok = if up {
            let top = if signed {
                bp.as_const().filter(|&k| (k as i32) >= 0).or(bp.max())
            } else {
                bp.max()
            };
            top.is_some_and(|t| t <= max - by + 1)
        } else {
            signed || bp.as_const().is_some_and(|k| k >= by - 1)
        };
        if !ok {
            return Err("a step of more than 1 may wrap");
        }
    }
    // A signed compare's range in `u32`s: its least value not negative.
    if signed {
        let least_ok = if up {
            s.non_negative()
        } else {
            bp.as_const().is_some_and(|k| (k as i32) >= -1) || bp.non_negative()
        };
        if !least_ok {
            return Err("signed range may be negative");
        }
    }
    // Tested at the start, the range may be empty as the loop's rounds
    // may be none: it is, wherever the start's test leaves at once.
    let mut clamp = false;
    if top {
        if cont.rel == Rel::Ne {
            // Stepping by 1 to an end it is past: round the whole width.
            let (s, b) = (
                s.as_const()
                    .ok_or("not equal from a start not a constant")?,
                bp.as_const().ok_or("not equal to a bound not a constant")?,
            );
            if (up && s > b) || (!up && s < b) {
                return Err("not equal past its bound");
            }
        }
        if signed && !s.non_negative() {
            return Err("signed start may be negative");
        }
        if up && signed && !bp.non_negative() {
            // A bound below 0 leaves at once: `(b as i32).max(0)` as the
            // range's end, but a constant's (no rounds at all).
            if bp.as_const().is_some() {
                return Err("signed bound negative");
            }
            clamp = true;
        }
        // Counting down to `b + 1`: never past the type's greatest.
        if !up && !signed && bp.max().is_none_or(|m| m == u32::MAX) {
            return Err("bound may be the greatest");
        }
    }
    // The bound a constant where the way in leaves it one (a register
    // nothing in the loop writes, set before it).
    let b = if bp.as_const().is_some() {
        bp.clone()
    } else {
        b
    };
    // Both ends constants: the first typed, as nothing else types it.
    let both = s.as_const().is_some() && b.as_const().is_some();
    let start = match s.as_const() {
        Some(k) => crate::print::word(k, both && up).to_string(),
        None => format!("r{r}"),
    };
    let mut range = if up && clamp {
        format!(
            "{start}..({} as i32).max(0) as u32",
            text(&b, r).ok_or("bound text")?
        )
    } else if up {
        format!("{start}..{}", text(&b, r).ok_or("bound text")?)
    } else {
        let lo = match b.as_const() {
            Some(k) => crate::print::word(k.wrapping_add(1), both).to_string(),
            None => format!("{} + 1", text(&b, r).ok_or("bound text")?),
        };
        format!("({lo}..={start}).rev()")
    };
    if by != 1 {
        if up {
            range = format!("({range})");
        }
        range.push_str(&format!(".step_by({by})"));
    }
    Ok(Proven {
        step,
        range,
        start: s.as_const(),
        by: c as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use analysis::ir::{BitOp, Exit, ValueId};
    use armv4t::sem::Reg;

    fn i(op: Op) -> Inst {
        Inst { op, guard: None }
    }

    fn v(n: u32) -> ValueId {
        ValueId(n)
    }

    /// `adds r4, #1` (narrowed by `and` with 0xff when `u8`), `cmp r4,
    /// r5; blt back`.
    fn latch(u8_counter: bool) -> Vec<Inst> {
        let mut b = vec![
            i(Op::GetReg(Reg(4))),
            i(Op::Const(1)),
            i(Op::Bit(false)),
            i(Op::Adc {
                a: v(0),
                b: v(1),
                c: v(2),
            }),
            i(Op::Const(0xff)),
            i(Op::Bin(BinOp::And, v(3), v(4))),
        ];
        let next = if u8_counter { v(5) } else { v(3) };
        b.extend([
            i(Op::SetReg(Reg(4), next)),
            i(Op::GetReg(Reg(5))),
            i(Op::Not(v(7))),
            i(Op::Bit(true)),
            i(Op::Adc {
                a: next,
                b: v(8),
                c: v(9),
            }),
            i(Op::TestBit { v: v(10), n: 31 }),
            i(Op::AdcOverflow {
                a: next,
                b: v(8),
                c: v(9),
            }),
            i(Op::BBin(BitOp::Xor, v(11), v(12))),
            i(Op::Const(0x0800_0100)),
            Inst {
                op: Op::Exit(Exit::Jump(v(14))),
                guard: Some(v(13)),
            },
            i(Op::Const(0x0800_0200)),
            i(Op::Exit(Exit::Jump(v(16)))),
        ]);
        b
    }

    /// `adds r5, r0, #0; movs r4, #0; cmp r5, #0; ble out; b loop`.
    fn way_in() -> Vec<Inst> {
        vec![
            i(Op::GetReg(Reg(0))),
            i(Op::Const(0)),
            i(Op::Bit(false)),
            i(Op::Adc {
                a: v(0),
                b: v(1),
                c: v(2),
            }),
            i(Op::SetReg(Reg(5), v(3))),
            i(Op::SetReg(Reg(4), v(1))),
            i(Op::Const(u32::MAX)),
            i(Op::Bit(true)),
            i(Op::Adc {
                a: v(3),
                b: v(6),
                c: v(7),
            }),
            i(Op::IsZero(v(8))),
            i(Op::TestBit { v: v(8), n: 31 }),
            i(Op::AdcOverflow {
                a: v(3),
                b: v(6),
                c: v(7),
            }),
            i(Op::BBin(BitOp::Xor, v(10), v(11))),
            i(Op::BBin(BitOp::Or, v(9), v(12))),
            i(Op::Const(0x0800_0200)),
            Inst {
                op: Op::Exit(Exit::Jump(v(14))),
                guard: Some(v(13)),
            },
            i(Op::Const(0x0800_0100)),
            i(Op::Exit(Exit::Jump(v(16)))),
        ]
    }

    /// `for (i = 0; i < n; i++)`: the guard `n <= 0` not taken proves the
    /// range; a `u8` counter to an `n` that may pass 255 isn't one.
    #[test]
    fn a_loop_to_a_parameter_is_a_range() {
        let (l, w) = (latch(false), way_in());
        let lp = Loop {
            reg: 4,
            latch: &l,
            back: 15,
            way_in: &w,
            enter: 17,
            kept: 1 << 5,
            test: Test::Back,
            via: &[],
        };
        assert_eq!(
            prove(&lp),
            Some(Proven {
                step: 6,
                range: "0..r5".into(),
                start: Some(0),
                by: 1,
            })
        );
        // Without the guard, nothing says n > 0.
        let mut bare = w.clone();
        bare[15].guard = None;
        assert_eq!(
            prove(&Loop {
                way_in: &bare,
                ..lp
            }),
            None
        );
        // n loaded (`p->cap`, unit native/lift-for-index): a value fixed
        // once loaded, which the guard bounds as it does a register's.
        let mut loaded = w.clone();
        loaded[0] = i(Op::Load {
            width: armv4t::sem::Width::Word,
            addr: v(1),
        });
        assert_eq!(
            prove(&Loop {
                way_in: &loaded,
                ..lp
            })
            .map(|p| p.range),
            Some("0..r5".into())
        );
        // r5 written in the loop: not a bound.
        assert_eq!(prove(&Loop { kept: 0, ..lp }), None);
        let l8 = latch(true);
        assert_eq!(prove(&Loop { latch: &l8, ..lp }), None);
    }

    /// The same latch leaving by a guarded exit when `r4 >= r5`, then
    /// jumping back unguarded (agbcc's `bge out; b back`): the same range.
    #[test]
    fn a_latch_leaving_before_its_jump_back_is_a_range() {
        let mut l = latch(false)[..14].to_vec();
        l.extend([
            i(Op::BNot(v(13))),
            i(Op::Const(0x0800_0200)),
            Inst {
                op: Op::Exit(Exit::Jump(v(15))),
                guard: Some(v(14)),
            },
            i(Op::Const(0x0800_0100)),
            i(Op::Exit(Exit::Jump(v(17)))),
        ]);
        let w = way_in();
        let lp = Loop {
            reg: 4,
            latch: &l,
            back: 18,
            way_in: &w,
            enter: 17,
            kept: 1 << 5,
            test: Test::Exit(16),
            via: &[],
        };
        assert_eq!(prove(&lp).map(|p| p.range), Some("0..r5".into()));
        // As the jump back's own guard, it is the wrong way round.
        assert_eq!(
            why(&Loop {
                back: 16,
                test: Test::Back,
                ..lp
            })
            .err(),
            Some("compare the wrong way")
        );
    }

    /// `cmp r4, r5` then a jump on signed `r4 >= r5` out of the loop
    /// (`leaves`), or on `r4 < r5` into it; `bound` in place of r5.
    fn start(leaves: bool, bound: Option<u32>) -> Vec<Inst> {
        let b = match bound {
            Some(k) => Op::Const(k),
            None => Op::GetReg(Reg(5)),
        };
        let (taken, other) = if leaves {
            (0x0800_0200, 0x0800_0110)
        } else {
            (0x0800_0110, 0x0800_0200)
        };
        vec![
            i(Op::GetReg(Reg(4))),
            i(b),
            i(Op::Not(v(1))),
            i(Op::Bit(true)),
            i(Op::Adc {
                a: v(0),
                b: v(2),
                c: v(3),
            }),
            i(Op::TestBit { v: v(4), n: 31 }),
            i(Op::AdcOverflow {
                a: v(0),
                b: v(2),
                c: v(3),
            }),
            i(Op::BBin(BitOp::Xor, v(5), v(6))),
            i(Op::BNot(v(7))),
            i(Op::Const(taken)),
            Inst {
                op: Op::Exit(Exit::Jump(v(9))),
                guard: Some(if leaves { v(8) } else { v(7) }),
            },
            i(Op::Const(other)),
            i(Op::Exit(Exit::Jump(v(11)))),
        ]
    }

    /// `i = 0; while (i < n) { …; i++; }` (unit native/lift-for-top-tested):
    /// no guard before it, as the start's test leaves when the range is
    /// empty; a signed bound that may be negative ends the range at 0.
    #[test]
    fn a_loop_tested_at_its_start_is_a_range() {
        // `adds r4, #1; b start`.
        let l = vec![
            i(Op::GetReg(Reg(4))),
            i(Op::Const(1)),
            i(Op::Bit(false)),
            i(Op::Adc {
                a: v(0),
                b: v(1),
                c: v(2),
            }),
            i(Op::SetReg(Reg(4), v(3))),
            i(Op::Const(0x0800_0100)),
            i(Op::Exit(Exit::Jump(v(5)))),
        ];
        // `movs r4, #0; b start`.
        let w = vec![
            i(Op::Const(0)),
            i(Op::SetReg(Reg(4), v(0))),
            i(Op::Const(0x0800_0100)),
            i(Op::Exit(Exit::Jump(v(2)))),
        ];
        let (out, into) = (start(true, None), start(false, None));
        let lp = Loop {
            reg: 4,
            latch: &l,
            back: 6,
            way_in: &w,
            enter: 3,
            kept: 1 << 5,
            test: Test::Top(&out, 10, true),
            via: &[],
        };
        let clamped = Some("0..(r5 as i32).max(0) as u32".to_string());
        assert_eq!(prove(&lp).map(|p| p.range), clamped);
        let lp2 = Loop {
            test: Test::Top(&into, 10, false),
            ..lp
        };
        assert_eq!(prove(&lp2).map(|p| p.range), clamped);
        // To a constant: typed, as nothing else types it.
        let ten = start(true, Some(10));
        let lp3 = Loop {
            test: Test::Top(&ten, 10, true),
            ..lp
        };
        assert_eq!(prove(&lp3).map(|p| p.range), Some("0_u32..10".into()));
        // A start that may be negative: a signed range not the u32's.
        let w2 = vec![
            i(Op::GetReg(Reg(0))),
            i(Op::SetReg(Reg(4), v(0))),
            i(Op::Const(0x0800_0100)),
            i(Op::Exit(Exit::Jump(v(2)))),
        ];
        assert_eq!(
            why(&Loop { way_in: &w2, ..lp }).err(),
            Some("signed range may be negative")
        );
    }

    /// `r6 = r4 + 1` early in the body, `r4 = r6 + 0` in the latch (unit
    /// native/lift-for-step-via): the step, carried through r6.
    #[test]
    fn a_step_through_a_second_register() {
        let mut l = latch(false);
        l[0] = i(Op::GetReg(Reg(6)));
        l[1] = i(Op::Const(0));
        let w = way_in();
        let one = Sym {
            add: 1,
            ..Sym::reg(4)
        };
        let lp = Loop {
            reg: 4,
            latch: &l,
            back: 15,
            way_in: &w,
            enter: 17,
            kept: 1 << 5,
            test: Test::Back,
            via: &[],
        };
        assert_eq!(prove(&lp), None);
        let via = [(6, one)];
        assert_eq!(
            prove(&Loop { via: &via, ..lp }).map(|p| (p.step, p.range)),
            Some((6, "0..r5".into()))
        );
        // r6 = r5 + 1: not of the counter.
        let via = [(
            6,
            Sym {
                add: 1,
                ..Sym::reg(5)
            },
        )];
        assert_eq!(prove(&Loop { via: &via, ..lp }), None);
    }

    fn c(l: Sym, rel: Rel, r: Sym, signed: bool) -> Cmp {
        Cmp::new(l, rel, r, signed)
    }

    #[test]
    fn compares_normalise_and_imply() {
        let n = Sym::reg(1);
        let zero = Sym::konst(0);
        // n > 0 as i32 is 0 < n; it proves 0 < n as u32 too.
        let fact = c(n.clone(), Rel::Gt, zero.clone(), true);
        assert_eq!(fact, c(zero.clone(), Rel::Lt, n.clone(), true));
        assert!(implies(&fact, &c(zero.clone(), Rel::Lt, n.clone(), false)));
        // n != 0 proves 0 < n as u32, not as i32.
        let ne = c(n.clone(), Rel::Ne, zero.clone(), false);
        assert!(implies(&ne, &c(zero.clone(), Rel::Lt, n.clone(), false)));
        assert!(!implies(&ne, &c(zero.clone(), Rel::Lt, n.clone(), true)));
        // n a u8: never negative, so as an i32 too.
        let n8 = n.narrowed(8, false).unwrap();
        let ne8 = c(n8.clone(), Rel::Ne, zero.clone(), false);
        assert!(implies(&ne8, &c(zero.clone(), Rel::Lt, n8.clone(), true)));
        // n >= 0 as i32 is -1 < n.
        assert_eq!(
            c(n.clone(), Rel::Ge, zero.clone(), true),
            c(Sym::konst(u32::MAX), Rel::Lt, n.clone(), true)
        );
        // n <= 3 is n < 4.
        assert_eq!(
            c(n.clone(), Rel::Le, Sym::konst(3), false),
            c(n, Rel::Lt, Sym::konst(4), false)
        );
    }

    #[test]
    fn narrows_compose() {
        let x = Sym::reg(2);
        let b = x.narrowed(8, false).unwrap();
        assert_eq!(b.max(), Some(0xff));
        assert_eq!(b.narrowed(16, false), Some(b.clone()));
        assert_eq!(b.plus(1), None);
        assert_eq!(Sym::konst(0x1ff).narrowed(8, false), Some(Sym::konst(0xff)));
        assert_eq!(
            Sym::konst(0xff).narrowed(8, true),
            Some(Sym::konst(u32::MAX))
        );
    }
}
