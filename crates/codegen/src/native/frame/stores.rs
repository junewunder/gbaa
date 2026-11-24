//! Whether a function's frame addresses may be stored outside the stack
//! (`native/lift-frame-exact-narrow`): where they may, the native check
//! holds them to the guest's, so the function's callers must pass sp on
//! as the guest has it (`exact_sp` in `native.rs`); where they can't, a
//! caller whose own frame is gone may hand it a different sp.
//!
//! Per function, a run of its IR from its entry tracking, for each value,
//! the registers at entry it may be made from, through arithmetic (as the
//! frame run follows it: a shift's amount and a condition make none): a
//! bit each, bit 13 ([`SP`]) for its own sp (its frame's addresses). A
//! source escapes when a value made from it is stored anywhere but a frame
//! slot that doesn't escape ([`super::Frame`]'s: a store into an escaping
//! slot, through arithmetic, or outside the frame, but into the DMA
//! registers alone: a DMA's source, which the native check doesn't
//! compare, reads the frame where the native program has it), or passed
//! to a callee (a call, a tail call or a fall, a call by value to each of
//! its callee set) in a register that escapes there. What a function returns in r0-r3
//! is made of its sources, which its caller puts its own values for (a
//! frame address passed in and handed back, as `memcpy`'s destination,
//! escapes only if the caller stores it). A stand-in is the code it stands
//! in for, which the native check holds it to. A callee nothing is known
//! of (not lifted, no frame, a run that gave up, a call by value with no
//! complete callee set) lets what it is passed escape and returns any of it: r0-r3 at a call
//! (all the lift passes), every register at a tail call. sp itself isn't
//! passed: what a callee does with its own frame is its own (`exact_sp`
//! follows the calls). A frame word that doesn't escape (or a register's
//! save) holds what was stored to it, path by path; a load from anywhere
//! else is made of no source (a value stored there escaped where it was
//! stored). A BIOS call stores no pointer; its results are made of its
//! arguments. Interprocedural: rounds over every
//! function until nothing grows.
//!
//! Sound where the frames are: a function with no frame is one nothing is
//! known of. In one whose sp the frame run lost (a mode change, offsets
//! joined) every slot escapes, so whatever it stores in its frame but a
//! register's save escapes.

use super::{Edge, FBlock, Frame, Place};
use analysis::ir::{Inst, Op, ValueId};
use std::collections::BTreeMap;

/// The bit of a function's own sp.
pub const SP: u16 = 1 << 13;

/// Everything escapes.
const ALL: u16 = 0xffff;

/// The most rounds over the program before everything escapes.
const ROUNDS: usize = 200;

/// One function for [`stores`]: its entry, its blocks (a stand-in's, the
/// code it stands in for), its frame (`None`: nothing is known of it),
/// and whether a store, by its block's IR, its address and its
/// instruction's address, reaches only the DMA registers, which the
/// native check doesn't compare.
pub struct StoreFn<'a, 'b> {
    pub entry: u32,
    pub blocks: &'b BTreeMap<u32, FBlock<'a>>,
    pub frame: Option<&'b Frame>,
    pub io: &'b dyn Fn(&[Inst], ValueId, u32) -> bool,
}

/// What a function lets escape, and what it returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Esc {
    /// The sources (bit `n` for rN at entry, [`SP`] its own sp) that may
    /// be stored outside the stack.
    stores: u16,
    /// What r0-r3 may be made of on a return.
    rets: [u16; 4],
}

/// A function nothing is known of: no frame, or a run that gave up.
const OPEN: Esc = Esc {
    stores: ALL,
    rets: [ALL; 4],
};

/// What a call to code nothing is known of does: the lift passes it r0-r3
/// alone (r4-r11 only to a function that takes them, which is lifted, as
/// is any member of a callee set the holes file names), and it returns
/// what it was passed. Not an address in the stack it ran on: that is
/// above the caller's sp, a frame the callee is done with.
const CALL_OPEN: Esc = Esc {
    stores: 0xf,
    rets: [0xf; 4],
};

/// What a tail call or a fall into code nothing is known of does: it gets
/// every register.
const TAIL_OPEN: Esc = Esc {
    stores: ALL,
    rets: [ALL & !SP; 4],
};

/// For each function of `funcs`, the sources (bit `n` for rN at entry,
/// [`SP`] for its own sp) that may be stored outside the stack: [`SP`]
/// set, its frame's addresses may be. `callees(at)`: the complete callee
/// set of the call by value at `at`, if one is known.
pub fn stores(funcs: &[StoreFn], callees: &dyn Fn(u32) -> Option<Vec<u32>>) -> BTreeMap<u32, u16> {
    let none = Esc {
        stores: 0,
        rets: [0; 4],
    };
    let mut e: BTreeMap<u32, Esc> = funcs
        .iter()
        .map(|f| (f.entry, if f.frame.is_some() { none } else { OPEN }))
        .collect();
    for _ in 0..ROUNDS {
        let mut changed = false;
        for f in funcs {
            let Some(fr) = f.frame else { continue };
            let old = e[&f.entry];
            let one = one(f, fr, &e, callees);
            let new = Esc {
                stores: old.stores | one.stores,
                rets: std::array::from_fn(|j| old.rets[j] | one.rets[j]),
            };
            if new != old {
                e.insert(f.entry, new);
                changed = true;
            }
        }
        if !changed {
            return e.into_iter().map(|(k, v)| (k, v.stores)).collect();
        }
    }
    e.into_keys().map(|k| (k, ALL)).collect()
}

/// What the callees of an exit do (a call's if `call`, else a tail
/// call's): `target`'s, or those of the call by value at `at`; one not
/// lifted or [`OPEN`], or no complete set, as [`CALL_OPEN`] or
/// [`TAIL_OPEN`].
fn callees_of(
    e: &BTreeMap<u32, Esc>,
    callees: &dyn Fn(u32) -> Option<Vec<u32>>,
    target: Option<u32>,
    at: u32,
    call: bool,
) -> Esc {
    let open = if call { CALL_OPEN } else { TAIL_OPEN };
    let ts = match target {
        Some(t) => vec![t],
        None => match callees(at) {
            Some(ts) if !ts.is_empty() => ts,
            _ => return open,
        },
    };
    let mut out = Esc {
        stores: 0,
        rets: [0; 4],
    };
    for t in ts {
        let c = e.get(&t).copied().filter(|c| *c != OPEN).unwrap_or(open);
        out.stores |= c.stores;
        for (a, b) in out.rets.iter_mut().zip(c.rets) {
            *a |= b;
        }
    }
    out
}

/// What the registers `which` (a bit each, sp left out) hold in `regs`,
/// joined.
fn of(regs: &[u16; 16], which: u16) -> u16 {
    (0..16)
        .filter(|&r| r != 13 && which & 1 << r != 0)
        .fold(0, |m, r| m | regs[r])
}

/// After a call to `c` from `s`: r0-r3 what it returns, in the caller's
/// sources (its own sp as the caller's sp); r12 and lr nothing.
fn after(s: &[u16; 16], c: &Esc) -> [u16; 16] {
    let mut a = *s;
    for j in 0..4 {
        let sp = if c.rets[j] & SP != 0 { s[13] } else { 0 };
        a[j] = of(s, c.rets[j]) | sp;
    }
    a[12] = 0;
    a[14] = 0;
    a
}

/// Whether the access at inst `i` of block `k` is to a frame word the run
/// follows: a slot that doesn't escape, or a saved register's (every
/// store to it a register as it came, which only the epilogue reads
/// back: in a frame whose sp the run lost, every slot escapes).
fn kept(fr: &Frame, k: u32, i: usize) -> bool {
    match fr.places.get(&(k, i)) {
        Some(&Place::Exact(o, _)) => {
            fr.escapes_at(k, i) == Some(false)
                || fr.slots.iter().any(|s| s.saved && s.lo <= o && o < s.hi)
        }
        _ => false,
    }
}

/// The run's state at a point: what each register holds, and each frame
/// word it follows ([`kept`]), by its offset rounded down to a word.
#[derive(Clone, Debug, PartialEq, Eq)]
struct St {
    r: [u16; 16],
    w: BTreeMap<i32, u16>,
}

impl St {
    /// Joins `o` in: whether anything grew.
    fn join(&mut self, o: &St) -> bool {
        let old = self.clone();
        for (a, b) in self.r.iter_mut().zip(&o.r) {
            *a |= b;
        }
        for (k, v) in &o.w {
            *self.w.entry(*k).or_default() |= v;
        }
        *self != old
    }
}

/// The words `o..o + n` reaches, by offset rounded down to a word.
fn span(o: i32, n: u8) -> impl Iterator<Item = i32> {
    (o.div_euclid(4) * 4..o + n as i32).step_by(4)
}

/// One run of function `f` against the program's `e` so far.
fn one(
    f: &StoreFn,
    fr: &Frame,
    e: &BTreeMap<u32, Esc>,
    callees: &dyn Fn(u32) -> Option<Vec<u32>>,
) -> Esc {
    let mut out = Esc {
        stores: 0,
        rets: [0; 4],
    };
    let Some(&first) = f
        .blocks
        .keys()
        .find(|&&k| k == f.entry)
        .or(f.blocks.keys().next())
    else {
        return out;
    };
    let mut start = St {
        r: [0; 16],
        w: BTreeMap::new(),
    };
    for (r, m) in start.r.iter_mut().enumerate().take(14) {
        *m = 1 << r;
    }
    let mut ins: BTreeMap<u32, St> = BTreeMap::from([(first, start)]);
    let mut work = vec![first];
    let flow = |ins: &mut BTreeMap<u32, St>, work: &mut Vec<u32>, to: u32, s: &St| {
        if !f.blocks.contains_key(&to) {
            return;
        }
        match ins.get_mut(&to) {
            Some(old) => {
                if old.join(s) {
                    work.push(to);
                }
            }
            None => {
                ins.insert(to, s.clone());
                work.push(to);
            }
        }
    };
    while let Some(k) = work.pop() {
        let b = &f.blocks[&k];
        let mut s = ins[&k].clone();
        let mut vals = vec![0u16; b.insts.len()];
        let mut at = k & !1;
        for (i, inst) in b.insts.iter().enumerate() {
            let v = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    0
                }
                Op::GetReg(r) => s.r[r.index()],
                Op::GetUserReg(_) | Op::GetFlag(_) | Op::GetCpsr | Op::GetSpsr => 0,
                Op::SetReg(r, x) => {
                    let r = r.index();
                    let x = vals[x.index()];
                    s.r[r] = if inst.guard.is_some() { s.r[r] | x } else { x };
                    0
                }
                Op::Load { .. } => match fr.places.get(&(k, i)) {
                    Some(&Place::Exact(o, n)) if kept(fr, k, i) => {
                        span(o, n).fold(0, |m, w| m | s.w.get(&w).copied().unwrap_or(0))
                    }
                    Some(Place::Through(_)) => s.w.values().fold(0, |m, w| m | w),
                    _ => 0,
                },
                Op::Store { value, addr, .. } => {
                    let x = vals[value.index()];
                    match fr.places.get(&(k, i)) {
                        Some(Place::Elsewhere) if (f.io)(b.insts, addr, at) => {}
                        Some(&Place::Exact(o, n)) if kept(fr, k, i) => {
                            let whole = n == 4 && o % 4 == 0 && inst.guard.is_none();
                            for w in span(o, n) {
                                let old = if whole {
                                    0
                                } else {
                                    s.w.get(&w).copied().unwrap_or(0)
                                };
                                s.w.insert(w, old | x);
                            }
                        }
                        _ => out.stores |= x,
                    }
                    0
                }
                Op::Exit(_) => {
                    match b.edges.get(&i) {
                        Some(Edge::Goto(ks)) => {
                            for &t in ks {
                                flow(&mut ins, &mut work, t, &s);
                            }
                        }
                        Some(Edge::Call { target, ret }) => {
                            let c = callees_of(e, callees, *target, at, true);
                            out.stores |= of(&s.r, c.stores);
                            let a = St {
                                r: after(&s.r, &c),
                                w: s.w.clone(),
                            };
                            if f.blocks.contains_key(ret) {
                                flow(&mut ins, &mut work, *ret, &a);
                            } else {
                                // Returns into another function's entry: a
                                // tail call after the call.
                                let t = callees_of(e, callees, Some(*ret), at, false);
                                out.stores |= of(&a.r, t.stores);
                                for (r, x) in out.rets.iter_mut().zip(after(&a.r, &t)) {
                                    *r |= x;
                                }
                            }
                        }
                        Some(Edge::Return) => {
                            for (r, x) in out.rets.iter_mut().zip(s.r) {
                                *r |= x;
                            }
                        }
                        Some(Edge::Tail(t)) => {
                            let c = callees_of(e, callees, *t, at, false);
                            out.stores |= of(&s.r, c.stores);
                            for (r, x) in out.rets.iter_mut().zip(after(&s.r, &c)) {
                                *r |= x;
                            }
                        }
                        Some(Edge::TailOr(ks)) => {
                            let c = callees_of(e, callees, None, at, false);
                            out.stores |= of(&s.r, c.stores);
                            for (r, x) in out.rets.iter_mut().zip(after(&s.r, &c)) {
                                *r |= x;
                            }
                            for &t in ks {
                                flow(&mut ins, &mut work, t, &s);
                            }
                        }
                        Some(Edge::Swi(next)) => {
                            // A BIOS call stores no pointer; its results
                            // are made of its arguments.
                            let mut a = s.clone();
                            let m = s.r[..4].iter().fold(0, |m, x| m | x);
                            a.r[..4].iter_mut().for_each(|r| *r = m);
                            flow(&mut ins, &mut work, *next, &a);
                        }
                        Some(Edge::Stop) => {}
                        // An exit the lifter gives no edge: nothing is
                        // known of where it goes.
                        None => {
                            out.stores |= of(&s.r, ALL);
                            out.rets = [ALL; 4];
                        }
                    }
                    0
                }
                // As the frame run: a shift is its value's (its amount, as
                // a load's rotation by the address's low bits, makes no
                // address); a carry or a condition is a bit, no address.
                Op::Bin(_, a, b)
                | Op::Adc { a, b, .. }
                | Op::Select { a, b, .. }
                | Op::MulLongLo { a, b, .. }
                | Op::MulLongHi { a, b, .. } => vals[a.index()] | vals[b.index()],
                Op::Not(v) | Op::Sext { v, .. } | Op::Rrx { v, .. } | Op::Shift { v, .. } => {
                    vals[v.index()]
                }
                _ => 0,
            };
            vals[i] = v;
        }
    }
    out
}
