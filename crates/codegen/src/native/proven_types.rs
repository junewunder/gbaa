//! Pointer types the lift proves, scored against DWARF (unit
//! `native/lift-proven-pointer-types`): a measurement. For a game with no
//! decomp the names files give no `t::` type, so what a pointer parameter
//! points at has to come from the code. Here the proof is its accesses:
//! each load and store a function makes through one of its parameters
//! (r0-r3 at entry, plus constants), at an offset and a width, and those
//! of every callee it passes the pointer to, carried to callers and
//! callees until they agree. Nothing here uses the names files' types
//! ([`super::fields`] is the flow that does), so what it proves is what
//! `--no-field-types` would have.
//!
//! The names files are the answer key: a parameter whose prototype makes
//! it a pointer to a struct or union (`t.proto_of`, [`Ty::Named`]) is
//! scored, and its proven accesses **agree** where every one is a field
//! of DWARF's type (an exact field or element, [`Types::walk`], or bytes
//! holding bitfields, [`Types::joined`]), **disagree** where one isn't,
//! and are **unproven** where there is none. The three are the metrics
//! `proven_types_agree`, `proven_types_disagree` and
//! `proven_types_unproven` (no bars: a measurement) and a row of the pret
//! score (`just pret-score`).
//!
//! The flow, per function, as [`super::fields::flow`]'s but with a
//! parameter for a type: a register holds `(param, off)` where every way
//! in agrees; a constant added or taken off moves `off`; anything else
//! added drops it (an index: an element of no known size); a word a
//! frame's local slot holds is what every store to it stored; a direct
//! call's r0-r3 holding one are the callee's parameters (an edge), and a
//! call clears r0-r3, r12 and lr. Across functions: a callee's accesses
//! through its parameter `j`, passed `param + c`, are the caller's at
//! `+ c`; and where `c` is 0 the two point at one type, so the caller's
//! are the callee's too. Sets only grow, so the rounds end.

use std::collections::{BTreeMap, BTreeSet};

use analysis::ir::{BinOp, Inst, Op, ValueId};

use super::fields::After;
use crate::types::{Ty, Types};

/// A value that is parameter `.0`'s pointer plus `.1` bytes.
type P = (u8, u32);

/// What one function does with its parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The accesses through a parameter: `(param, off, width)`.
    pub access: BTreeSet<(u8, u32, u32)>,
    /// Parameter `.0` plus `.1` passed as argument `.3` of a direct call
    /// to `.2`.
    pub calls: BTreeSet<(u8, u32, u32, u8)>,
}

/// The function's [`Facts`]. `blocks[0]` is the entry; `out(b, i)` and
/// `slot(b, i)` as [`super::fields::flow`]'s.
pub fn flow(
    blocks: &[&[Inst]],
    out: impl Fn(usize, usize) -> Vec<(usize, After)>,
    slot: impl Fn(usize, usize) -> Option<i32>,
) -> Facts {
    type Regs = [Option<P>; 16];
    let mut ins: Vec<Option<Regs>> = vec![None; blocks.len()];
    if !blocks.is_empty() {
        let mut start: Regs = Default::default();
        for (r, s) in start.iter_mut().enumerate().take(4) {
            *s = Some((r as u8, 0));
        }
        ins[0] = Some(start);
    }
    let mut slots: BTreeMap<i32, Option<P>> = BTreeMap::new();
    let mut facts = Facts::default();
    for round in 0.. {
        let mut changed = false;
        facts = Facts::default();
        for b in 0..blocks.len() {
            let Some(mut regs) = ins[b] else {
                continue;
            };
            let insts = blocks[b];
            let mut vt: Vec<Option<P>> = vec![None; insts.len()];
            let konst = |x: ValueId| match insts[x.index()].op {
                Op::Const(c) | Op::Pool(c) => Some(c),
                _ => None,
            };
            let moved = |p: Option<P>, c: u32| p.map(|(r, o)| (r, o.wrapping_add(c)));
            for (i, inst) in insts.iter().enumerate() {
                match inst.op {
                    Op::GetReg(r) => vt[i] = regs[r.index()],
                    Op::Bin(op @ (BinOp::Add | BinOp::Sub), x, y) => {
                        let (px, py) = (vt[x.index()], vt[y.index()]);
                        vt[i] = match (px, py, konst(x), konst(y), op) {
                            (Some(_), None, _, Some(c), BinOp::Add) => moved(px, c),
                            (None, Some(_), Some(c), _, BinOp::Add) => moved(py, c),
                            (Some(_), None, _, Some(c), BinOp::Sub) => moved(px, c.wrapping_neg()),
                            _ => None,
                        };
                    }
                    Op::Shift { v, amount, .. }
                        if vt[v.index()].is_some()
                            && super::aligned::load_shift(insts, v, amount).is_some() =>
                    {
                        vt[i] = vt[v.index()];
                    }
                    Op::Adc { a, b: y, c } => {
                        let carry = match insts[c.index()].op {
                            Op::Bit(c) => Some(u32::from(c)),
                            _ => None,
                        };
                        let k = |x: ValueId| match insts[x.index()].op {
                            Op::Not(z) => konst(z).map(|c| !c),
                            _ => konst(x),
                        };
                        let (pa, pb) = (vt[a.index()], vt[y.index()]);
                        vt[i] = match (pa, pb, carry) {
                            (Some(_), None, Some(c)) => {
                                k(y).and_then(|n| moved(pa, n.wrapping_add(c)))
                            }
                            (None, Some(_), Some(c)) => {
                                k(a).and_then(|n| moved(pb, n.wrapping_add(c)))
                            }
                            _ => None,
                        };
                    }
                    Op::Load { width, addr } => {
                        if let Some(s) = slot(b, i) {
                            vt[i] = slots.get(&s).copied().flatten();
                        } else if let Some((r, o)) = vt[addr.index()] {
                            facts.access.insert((r, o, width.bytes()));
                        }
                    }
                    Op::Store {
                        addr, value, width, ..
                    } => {
                        if let Some(s) = slot(b, i) {
                            let v = vt[value.index()];
                            let new = match slots.get(&s) {
                                None => v,
                                Some(old) if *old == v => v,
                                Some(_) => None,
                            };
                            if slots.get(&s) != Some(&new) {
                                slots.insert(s, new);
                                changed = true;
                            }
                        } else if let Some((r, o)) = vt[addr.index()] {
                            facts.access.insert((r, o, width.bytes()));
                        }
                    }
                    Op::SetReg(r, v) => {
                        let r = r.index();
                        let new = vt[v.index()];
                        regs[r] = match inst.guard {
                            Some(_) if regs[r] != new => None,
                            _ => new,
                        };
                    }
                    Op::Exit(_) => {
                        for (s, after) in out(b, i) {
                            let mut next = regs;
                            if let After::Call(callee) = after {
                                if let Some(k) = callee {
                                    for (j, p) in regs.iter().enumerate().take(4) {
                                        if let Some((r, o)) = *p {
                                            facts.calls.insert((r, o, k, j as u8));
                                        }
                                    }
                                }
                                for r in [0, 1, 2, 3, 12, 14] {
                                    next[r] = None;
                                }
                            }
                            let joined = match &ins[s] {
                                None => next,
                                Some(old) => {
                                    let mut j = *old;
                                    for r in 0..16 {
                                        if j[r] != next[r] {
                                            j[r] = None;
                                        }
                                    }
                                    j
                                }
                            };
                            if ins[s].as_ref() != Some(&joined) {
                                ins[s] = Some(joined);
                                changed = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !changed || round > 64 {
            break;
        }
    }
    facts
}

/// The furthest into its object a carried access goes: a call that
/// passes `p + c` round a cycle (a recursive walk) would add `c` on every
/// round.
pub const CARRIED_OFF: u32 = 0x1_0000;

/// The most accesses a parameter's set carries in: past it, it takes no
/// more (with so many, a type of one object isn't what it is anyway).
pub const CARRIED_MAX: usize = 1024;

/// Each parameter's accesses, `(function, param)` to `(off, width)`s,
/// carried over the call edges of `facts`, under [`CARRIED_OFF`] and
/// [`CARRIED_MAX`]: first up, a parameter taking its own and its
/// callees' (theirs through `j`, passed `param + c`, at `+ c`), until no
/// set grows; then down, a callee's parameter passed a caller's whole
/// (`c` 0) taking what the caller's has, up and down. Never up again
/// after down: a callee taking any pointer (a copy, a fill) would carry
/// every caller's type to every other.
pub fn carried(facts: &BTreeMap<u32, Facts>) -> BTreeMap<(u32, u8), BTreeSet<(u32, u32)>> {
    type Node = (u32, u8);
    type Sets = BTreeMap<Node, BTreeSet<(u32, u32)>>;
    let mut up: Sets = BTreeMap::new();
    for (&k, f) in facts {
        for &(r, o, w) in &f.access {
            up.entry((k, r)).or_default().insert((o, w));
        }
    }
    let mut callers: BTreeMap<Node, Vec<(Node, u32)>> = BTreeMap::new();
    let mut callees: BTreeMap<Node, Vec<Node>> = BTreeMap::new();
    for (&k, f) in facts {
        for &(r, c, g, j) in &f.calls {
            callers.entry((g, j)).or_default().push(((k, r), c));
            if c == 0 {
                callees.entry((k, r)).or_default().push((g, j));
            }
        }
    }
    // `from`'s set into `to`'s at `+ c`: whether it grew.
    fn add(sets: &mut Sets, from: &BTreeSet<(u32, u32)>, to: Node, c: u32) -> bool {
        let s = sets.entry(to).or_default();
        let mut grew = false;
        for &(o, w) in from {
            if s.len() >= CARRIED_MAX {
                break;
            }
            let o = o.wrapping_add(c);
            if o < CARRIED_OFF && s.insert((o, w)) {
                grew = true;
            }
        }
        grew
    }
    let mut work: Vec<Node> = up.keys().copied().collect();
    while let Some(n) = work.pop() {
        let from = up[&n].clone();
        for &(to, c) in callers.get(&n).map_or(&[][..], Vec::as_slice) {
            if add(&mut up, &from, to, c) {
                work.push(to);
            }
        }
    }
    let mut all = up.clone();
    let mut work: Vec<Node> = all.keys().copied().collect();
    while let Some(n) = work.pop() {
        let from = all[&n].clone();
        for &to in callees.get(&n).map_or(&[][..], Vec::as_slice) {
            if add(&mut all, &from, to, 0) {
                work.push(to);
            }
        }
    }
    all
}

/// The score against the names files' prototypes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Score {
    pub agree: usize,
    pub disagree: usize,
    pub unproven: usize,
}

/// Whether an access of `w` bytes at `off` is a field of `ty`: one
/// [`Types::walk`] names (a field or element, exactly), bytes holding
/// bitfields ([`Types::joined`]), or whole fields side by side, each one
/// of those (a compiler's joined store, `pData->a = pData->b = 0` as one
/// word, or a struct copied a word at a time).
pub fn is_field(t: &Types, ty: &Ty, off: u32, w: u32) -> bool {
    let one = |o: u32, k: u32| t.walk(ty, o, k, None).is_some() || t.joined(ty, o, k).is_some();
    if one(off, w) {
        return true;
    }
    let Some(end) = off.checked_add(w) else {
        return false;
    };
    let mut p = off;
    while p < end {
        match [4, 2, 1].into_iter().find(|&k| p + k <= end && one(p, k)) {
            Some(k) => p += k,
            None => return false,
        }
    }
    true
}

/// Each lifted function's parameters that its prototype makes a pointer
/// to a struct or union, scored by their carried accesses.
pub fn score(t: &Types, facts: &BTreeMap<u32, Facts>) -> Score {
    let sets = carried(facts);
    let mut s = Score::default();
    for &k in facts.keys() {
        let Some(p) = t.proto_of(k) else {
            continue;
        };
        for (r, a) in p.args.iter().enumerate().take(4) {
            let Some((_, Some(ty @ Ty::Named(_)))) = a else {
                continue;
            };
            match sets.get(&(k, r as u8)) {
                None => s.unproven += 1,
                Some(acc) if acc.is_empty() => s.unproven += 1,
                Some(acc) if acc.iter().all(|&(o, w)| is_field(t, ty, o, w)) => s.agree += 1,
                Some(_) => s.disagree += 1,
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(access: &[(u8, u32, u32)], calls: &[(u8, u32, u32, u8)]) -> Facts {
        Facts {
            access: access.iter().copied().collect(),
            calls: calls.iter().copied().collect(),
        }
    }

    #[test]
    fn carried_both_ways_at_zero_and_up_at_an_offset() {
        let mut m = BTreeMap::new();
        // 0x10 passes its r0 to 0x20's r1, and r0 + 8 to 0x30's r0.
        m.insert(0x10, f(&[(0, 0, 4)], &[(0, 0, 0x20, 1), (0, 8, 0x30, 0)]));
        m.insert(0x20, f(&[(1, 4, 2)], &[]));
        m.insert(0x30, f(&[(0, 2, 1)], &[]));
        let s = carried(&m);
        let want = |x: &[(u32, u32)]| x.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(s[&(0x10, 0)], want(&[(0, 4), (4, 2), (10, 1)]));
        // The caller's own, and 0x30's carried up, come down at 0.
        assert_eq!(s[&(0x20, 1)], want(&[(0, 4), (4, 2), (10, 1)]));
        // Not down past an offset.
        assert_eq!(s[&(0x30, 0)], want(&[(2, 1)]));
    }

    #[test]
    fn a_shared_callee_carries_no_caller_to_another() {
        let mut m = BTreeMap::new();
        // 0x10 and 0x20 each pass their r0 to 0x30 (a copy taking any
        // pointer), which reads a byte at 0.
        m.insert(0x10, f(&[(0, 4, 4)], &[(0, 0, 0x30, 0)]));
        m.insert(0x20, f(&[(0, 8, 2)], &[(0, 0, 0x30, 0)]));
        m.insert(0x30, f(&[(0, 0, 1)], &[]));
        let s = carried(&m);
        let want = |x: &[(u32, u32)]| x.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(s[&(0x10, 0)], want(&[(0, 1), (4, 4)]));
        assert_eq!(s[&(0x20, 0)], want(&[(0, 1), (8, 2)]));
        assert_eq!(s[&(0x30, 0)], want(&[(0, 1), (4, 4), (8, 2)]));
    }
}
