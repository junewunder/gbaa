//! Callee-saved registers proven per callee (`docs/mod-view-design.md`,
//! "How close exact Rust gets to pret's C", gap 2): a function *keeps*
//! r4–r11 and sp if every way it returns leaves them as it found them, and
//! every function it calls or jumps to keeps them too. agbcc's push/pop
//! pairs make that a pattern over the function's IR: the registers it
//! writes come back from the stack slots its prologue stored them in.
//!
//! Where a call's callee keeps them, a caller holding registers in locals
//! ([`crate::locals`]) need not reload r4–r11 and sp after the call
//! ([`crate::locals::Locals::reload_kept_body`]), so a loop variable in r4
//! survives it.
//!
//! The proof is an abstract run of each function's blocks from its entry.
//! Each register holds its entry value (r4–r11, lr), the entry sp plus a
//! constant, a constant, or something else; each word on the stack at the
//! entry sp plus a constant holds one of those. It fails a function that:
//!
//! - returns (jumps to its entry lr) with r4–r11 or sp not as they came;
//! - makes a call it can't name (through a register, a `bx rN` stub or a
//!   jump the dispatcher finds), or jumps where nothing is known;
//! - calls or tail-jumps to a function that doesn't keep them;
//! - changes mode (`msr`, a user-bank write, an exception return, an
//!   undefined instruction, `SoftReset` or `HardReset`);
//! - runs from IWRAM, where its code may change.
//!
//! A library routine the runtime stands in for (`Ctx::library`: libgcc's
//! `__muldf3` and `__divdf3`) keeps them, as its code does on the whole;
//! the functions inside its code (its special-case paths, which return
//! r4 and r5 changed) are neither proven nor summarised.
//!
//! A conditional instruction's writes are kept apart per condition, so a
//! pop and return under one condition (libgcc's `ldmhi sp!, {r4, r5, lr};
//! bxhi lr`) returns with what it popped ([`Walk::block`]).
//!
//! A summary's run takes an exit whose condition is a certain constant
//! ([`State::sure`]) always or never, so what a path that can't run would
//! read isn't read (unit `native/nightmare-entry-reads`). The proof takes
//! every exit both ways.
//!
//! Three things it assumes, which every function compiled from C relies on:
//! the entry sp is word-aligned;
//! a store through an address it can't place (not the entry sp plus a
//! constant), or above the entry sp, doesn't hit the slots a prologue
//! (this function's or a caller's) saved registers in, and
//! an IRQ handler restores r4–r11 (taken in IRQ mode, its sp and lr are
//! banked). fn-diff's `saves` form checks the result against the
//! reference on recorded states.

use crate::func::{Ctx, ExitInfo, Func, GBlock, Plan, exits, successors};
use analysis::ir::{BinOp, Exit, Inst, Op, ValueId};
use armv4t::sem::Width;
use std::collections::{BTreeMap, BTreeSet};

mod summary;
pub use summary::{
    Assume, Bits, Category, Cause, Comparison, Passing, Sets, Summaries, Summary, Verdict, compare,
    proto_regs, summarize, summarize_passing, summarize_sets, summarize_with,
};
use summary::{Org, Sum};

/// The registers a kept call leaves as they were: r4–r11 and sp.
pub const KEPT: u16 = 0x2ff0;

/// What a register or a stack word holds, in terms of the function's
/// entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum V {
    /// Register `n`'s value at entry (r4–r11, lr).
    Entry(u8),
    /// A value some of whose bits may depend on r4–r11's entry values:
    /// `regs` names them (bit `n - 4` for rN), `bits` the value's bits
    /// that may (an `and` with a constant clears some, a shift moves
    /// them, an `add` carries them up). The rest are unknown. Never empty
    /// ([`V::part`]).
    Part {
        regs: u8,
        bits: u32,
    },
    /// The entry sp plus this.
    Sp(i32),
    Const(u32),
    Other,
}

impl V {
    /// Two values paths may hold: equal, else, where both hold bits of
    /// r4–r11's entry values, whatever bits either may. Where only one
    /// does, unknown: a register set on one path and not on another is
    /// most often a path that can't happen (or a local C never set, whose
    /// value doesn't matter there), as the summaries took it before they
    /// followed bits (`native/entry-read-masking`).
    fn join(self, o: V) -> V {
        if self == o {
            return self;
        }
        match (self.taint(), o.taint()) {
            (Some((ra, ba)), Some((rb, bb))) => V::part(ra | rb, ba | bb),
            _ => V::Other,
        }
    }

    /// The entry values of r4–r11 it may hold bits of (bit `n - 4` for
    /// rN), and which of its bits.
    fn taint(self) -> Option<(u8, u32)> {
        match self {
            V::Entry(r @ 4..=11) => Some((1 << (r - 4), !0)),
            V::Part { regs, bits } => Some((regs, bits)),
            _ => None,
        }
    }

    /// `bits` of the entry values of `regs`: [`V::Other`] if none.
    fn part(regs: u8, bits: u32) -> V {
        if regs == 0 || bits == 0 {
            V::Other
        } else {
            V::Part { regs, bits }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    regs: [V; 16],
    /// Words on the stack, by offset from the entry sp.
    slots: BTreeMap<i32, V>,
    /// Where r0-r3 and r12 came from ([`summary`]); empty in the proof.
    org: Vec<Org>,
    /// For each of r4–r11, a callee that may have changed it
    /// ([`Summary::changes`]), or 0; all 0 in the proof.
    chg: [u32; 8],
    /// N, Z, C and V, as bits (whether they hold bits of r4–r11's entry
    /// values, or a constant).
    flags: [V; 4],
    /// The registers (bit `n`) and flags (bit `16 + f`) whose constant is
    /// certain: made in this function from constants alone, through no
    /// load from memory (a local whose address a callee has may have
    /// changed), and since kept by every call (one to a function proven
    /// to keep r4–r11). A summary's run takes an exit under a guard made
    /// of such constants always or never ([`Walk::block`]).
    sure: u32,
}

impl State {
    fn entry() -> State {
        let mut regs = [V::Other; 16];
        for (i, r) in regs.iter_mut().enumerate() {
            if KEPT & 1 << i != 0 || i == 14 {
                *r = V::Entry(i as u8);
            }
        }
        regs[13] = V::Sp(0);
        State {
            regs,
            slots: BTreeMap::new(),
            org: Vec::new(),
            chg: [0; 8],
            flags: [V::Other; 4],
            sure: 0,
        }
    }

    fn join(&self, o: &State) -> State {
        let mut regs = self.regs;
        for (r, b) in regs.iter_mut().zip(o.regs) {
            *r = r.join(b);
        }
        let slots = self
            .slots
            .iter()
            .filter(|&(k, v)| o.slots.get(k) == Some(v))
            .map(|(&k, &v)| (k, v))
            .collect();
        let org = self
            .org
            .iter()
            .zip(&o.org)
            .map(|(a, b)| a.join(b))
            .collect();
        State {
            regs,
            slots,
            org,
            chg: std::array::from_fn(|i| {
                if self.chg[i] != 0 {
                    self.chg[i]
                } else {
                    o.chg[i]
                }
            }),
            flags: std::array::from_fn(|i| self.flags[i].join(o.flags[i])),
            sure: self.sure & o.sure,
        }
    }

    /// r4–r11 and sp as they came.
    fn kept(&self) -> bool {
        (0..16)
            .filter(|i| KEPT & 1 << i != 0)
            .all(|i| self.regs[i] == if i == 13 { V::Sp(0) } else { V::Entry(i as u8) })
    }

    /// The registers of [`KEPT`] not as they came, and what each holds.
    fn changed(&self) -> String {
        let mut out = Vec::new();
        for i in (0..16).filter(|i| KEPT & 1 << i != 0) {
            let want = if i == 13 { V::Sp(0) } else { V::Entry(i as u8) };
            let v = self.regs[i];
            if v == want {
                continue;
            }
            let held = match v {
                V::Entry(n) => format!("r{n}'s entry value"),
                V::Part { .. } => "bits of r4-r11's entry values".into(),
                V::Sp(k) => format!("entry sp{k:+}"),
                V::Const(c) => format!("constant {c:#x}"),
                V::Other => "unknown".into(),
            };
            let name = if i == 13 {
                "sp".into()
            } else {
                format!("r{i}")
            };
            out.push(format!("{name} = {held}"));
        }
        out.join(", ")
    }

    /// After a call into code that keeps r4–r11 and sp: the rest are
    /// unknown, and so is the stack below sp, where the callee's frame
    /// was. `kept`: the callee is proven to keep them, so a certain
    /// constant in one stays certain ([`State::sure`]).
    fn called(&mut self, kept: bool) {
        for i in [0, 1, 2, 3, 12, 14] {
            self.regs[i] = V::Other;
        }
        self.flags = [V::Other; 4];
        self.sure &= if kept { KEPT as u32 & 0x0ff0 } else { 0 };
        match self.regs[13] {
            V::Sp(sp) => self.slots.retain(|&k, _| k >= sp),
            _ => self.slots.clear(),
        }
    }
}

/// Which functions keep r4–r11 and sp, and why each other one doesn't.
#[derive(Clone, Debug, Default)]
pub struct Proof {
    pub kept: BTreeSet<u32>,
    pub why: BTreeMap<u32, String>,
}

/// The proof over `funcs`, which should be every function `ctx` has: a
/// callee missing from them fails its callers.
pub fn prove(ctx: &Ctx, funcs: &[Func]) -> Proof {
    let by_entry: BTreeMap<u32, &Func> = funcs.iter().map(|f| (f.entry, f)).collect();
    let mut callees: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut why = BTreeMap::new();
    // A library routine the runtime stands in for keeps them (its
    // special-case paths, which don't, are inside it); the functions
    // inside its code aren't run.
    let lib = ctx.library();
    for &k in lib.routines.keys() {
        callees.insert(k, BTreeSet::new());
    }
    for f in funcs.iter().filter(|f| !lib.inside(f.entry)) {
        match local(ctx, &by_entry, f) {
            Ok(c) => {
                callees.insert(f.entry, c);
            }
            Err(e) => {
                why.insert(f.entry, e);
            }
        }
    }
    // The greatest set closed under calls: a recursive function keeps them
    // if nothing else it does breaks them.
    loop {
        let fails: Vec<(u32, u32)> = callees
            .iter()
            .filter_map(|(&f, cs)| Some((f, *cs.iter().find(|c| !callees.contains_key(c))?)))
            .collect();
        if fails.is_empty() {
            break;
        }
        for (f, c) in fails {
            callees.remove(&f);
            why.insert(f, format!("calls 0x{c:08x}, which doesn't keep them"));
        }
    }
    Proof {
        kept: callees.into_keys().collect(),
        why,
    }
}

/// `f`'s own part of the proof: the functions it calls or jumps to, which
/// must keep them too, or why it fails.
fn local(ctx: &Ctx, funcs: &BTreeMap<u32, &Func>, f: &Func) -> Result<BTreeSet<u32>, String> {
    walk(ctx, funcs, f, None).map(|w| w.callees)
}

/// The abstract run of `f`. With `sum` (a summary's run), what the proof
/// would fail on is a hole in `sum` instead, and the run goes on where it
/// can.
fn walk<'a>(
    ctx: &'a Ctx<'a>,
    funcs: &'a BTreeMap<u32, &'a Func>,
    f: &'a Func,
    sum: Option<Sum<'a>>,
) -> Result<Walk<'a>, String> {
    let mut entry = State::entry();
    if sum.is_some() {
        entry.org = vec![Org::entry(); 5];
    }
    let mut w = Walk {
        ctx,
        funcs,
        blocks: f.blocks.iter().map(|(&k, b)| (k, b)).collect(),
        ins: BTreeMap::from([(f.entry, entry)]),
        work: BTreeSet::from([f.entry]),
        callees: BTreeSet::new(),
        sum,
    };
    if f.overlay.is_some() {
        w.fail("runs from IWRAM".into())?;
    }
    // Each block's in-state only loses facts (a summary's origins only
    // grow), so this ends; the cap is a guard.
    let mut steps = 0;
    while let Some(k) = w.work.pop_first() {
        steps += 1;
        if steps > 64 * w.blocks.len() + 64 {
            w.stop("didn't settle".into())?;
            break;
        }
        w.block(k)?;
    }
    Ok(w)
}

struct Walk<'a> {
    ctx: &'a Ctx<'a>,
    funcs: &'a BTreeMap<u32, &'a Func>,
    /// The function's blocks, and those of the continuation roots it
    /// jumps to (discovery's entries that aren't functions, which codegen
    /// builds apart: a jump table's cases, the code after a call that
    /// doesn't return).
    blocks: BTreeMap<u32, &'a GBlock>,
    /// Each block's state at its start, so far.
    ins: BTreeMap<u32, State>,
    /// The blocks to run again, lowest address first: a loop's head
    /// (behind its exit, as compiled code lays loops out) settles before
    /// the code after the loop runs, so a value the loop's first pass had
    /// (a counter's first constant) doesn't reach past it and lose stack
    /// slots a store at it seemed to hit.
    work: BTreeSet<u32>,
    callees: BTreeSet<u32>,
    /// A summary's run: its callees' summaries so far and what it finds.
    sum: Option<Sum<'a>>,
}

impl Walk<'_> {
    /// What the proof fails on: an error in the proof, a hole in a
    /// summary's run (which goes on).
    fn fail(&mut self, why: String) -> Result<(), String> {
        match &mut self.sum {
            Some(s) => {
                s.holes.insert(why);
                Ok(())
            }
            None => Err(why),
        }
    }

    /// [`Walk::fail`] where the path's run ends: what runs after it (a
    /// jump into code the run doesn't follow) is unseen, so the summary
    /// may miss reads there ([`Summary::stopped`]).
    fn stop(&mut self, why: String) -> Result<(), String> {
        if let Some(s) = &mut self.sum {
            s.stopped = true;
        }
        self.fail(why)
    }

    /// `k` is in a library routine's code but not its entry: a path of
    /// the routine's own, which its stand-in doesn't have
    /// ([`crate::func::Library`]).
    fn inner(&self, k: u32) -> bool {
        let lib = self.ctx.library();
        lib.inside(k) && !lib.routines.contains_key(&k)
    }

    /// Go on at `to` with `s`.
    fn flow(&mut self, to: u32, s: &State) -> Result<(), String> {
        if !self.blocks.contains_key(&to) {
            if self.inner(to) {
                return self.stop(format!("0x{to:08x}: jumps into a library routine's code"));
            }
            // A tail call: the callee returns to our caller.
            let func = self.ctx.disc.funcs.contains_key(&to) && self.ctx.entries.contains(&to)
                || self.ctx.library().routines.contains_key(&to);
            if func && s.kept() && s.regs[14] == V::Entry(14) {
                self.callees.insert(to);
                if let Some(sum) = &mut self.sum {
                    // A call, then a return.
                    let mut after = s.clone();
                    sum.call(&mut after, Some(to), to);
                    after.called(false);
                    sum.ret(&after, to);
                }
                return Ok(());
            }
            // Code codegen builds apart, with this frame: a continuation
            // root (a jump table's case, say), or a function start
            // discovery found inside this function. Go on through it.
            match self.funcs.get(&to) {
                Some(r) if r.overlay.is_none() => {
                    for (&k, b) in &r.blocks {
                        self.blocks.entry(k).or_insert(b);
                    }
                    if func && let Some(sum) = &mut self.sum {
                        sum.falls.insert(to);
                    }
                }
                _ => return self.stop(format!("jumps to 0x{to:08x}, which has no code here")),
            }
        }
        let next = match self.ins.get(&to) {
            Some(old) => old.join(s),
            None => s.clone(),
        };
        if self.ins.get(&to) != Some(&next) {
            self.ins.insert(to, next);
            self.work.insert(to);
        }
        Ok(())
    }

    /// Run block `k` from its in-state.
    ///
    /// Besides the block's state, which joins each guarded write with what
    /// was there, the run keeps two states per guard seen (`cond`): what
    /// holds where that guard is true, its writes made outright, and where
    /// it is false, its writes not made. A guarded exit leaves with its
    /// guard's true state and the block goes on in its false one, so
    /// `ldmhi sp!, {r4, r5, lr}; bxhi lr` returns with r4, r5 and sp
    /// restored, not "maybe", and the code after it still has its frame.
    fn block(&mut self, k: u32) -> Result<(), String> {
        let gb = self.blocks[&k];
        let insts = &gb.block.insts;
        let mut s = self.ins[&k].clone();
        let mut cond: Vec<(ValueId, State, State)> = Vec::new();
        // The guards known false here (after an exit under each).
        let mut falses: Vec<ValueId> = Vec::new();
        let mut vals = vec![V::Other; insts.len()];
        // The values that are certain where constants ([`State::sure`]).
        let mut sure = vec![false; insts.len()];
        let xs = exits(self.ctx, gb);
        // The values something uses: a read whose value nothing uses
        // reads nothing.
        let used: BTreeSet<ValueId> = match self.sum {
            Some(_) => insts
                .iter()
                .flat_map(|i| i.op.operands().into_iter().chain(i.guard))
                .collect(),
            None => BTreeSet::new(),
        };
        let mut at = gb.block.start;
        for (i, inst) in insts.iter().enumerate() {
            let val = |v: ValueId| vals[v.index()];
            let guard = inst.guard;
            if let Some(g) = guard
                && matches!(inst.op, Op::Store { .. } | Op::SetReg(..))
                && !cond.iter().any(|(c, ..)| same(insts, *c, g))
            {
                cond.push((g, s.clone(), s.clone()));
            }
            let held = match inst.op {
                Op::GetReg(r) => s.sure >> r.index() & 1 != 0,
                Op::GetFlag(f) => s.sure >> (16 + f as usize) & 1 != 0,
                _ => false,
            };
            let v = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    V::Other
                }
                op if pure(insts, op, &vals, &falses).is_some() => {
                    pure(insts, op, &vals, &falses).expect("a pure value")
                }
                Op::GetReg(r) => {
                    if let Some(sum) = &mut self.sum
                        && used.contains(&ValueId(i as u32))
                    {
                        sum.read(&s, r.index(), Cause::Reads(at));
                    }
                    s.regs[r.index()]
                }
                Op::Load {
                    width: Width::Word,
                    addr,
                } => match val(addr) {
                    V::Sp(o) => s.slots.get(&o).copied().unwrap_or(V::Other),
                    _ => V::Other,
                },
                // A byte or halfword of a stack word holding bits of
                // r4-r11's entry values may hold them too.
                Op::Load { width, addr } => match val(addr) {
                    V::Sp(o) => match s.slots.get(&(o & !3)).and_then(|v| v.taint()) {
                        Some((regs, _)) => V::part(regs, width_mask(width)),
                        None => V::Other,
                    },
                    _ => V::Other,
                },
                Op::Store { width, addr, value } => {
                    let (addr, value) = (val(addr), val(value));
                    store(&mut s, width, addr, value, guard.is_some());
                    for (c, yes, no) in &mut cond {
                        let this = guard.is_some_and(|g| same(insts, *c, g));
                        let maybe = guard.is_some() && !this;
                        store(yes, width, addr, value, maybe);
                        if !this {
                            store(no, width, addr, value, maybe);
                        }
                    }
                    V::Other
                }
                Op::SetReg(r, x) => {
                    let certain = sure[x.index()];
                    let (r, x) = (r.index(), val(x));
                    let guarded = guard.is_some();
                    if let Some(sum) = &mut self.sum {
                        sum.frame_record(&s, r, x);
                        sum.set(&mut s, r, guarded);
                    }
                    set(&mut s, r, x, guarded, certain);
                    for (c, yes, no) in &mut cond {
                        let this = guard.is_some_and(|g| same(insts, *c, g));
                        let maybe = guarded && !this;
                        let states = if this { vec![yes] } else { vec![yes, no] };
                        for st in states {
                            if let Some(sum) = &self.sum {
                                sum.set(st, r, maybe);
                            }
                            set(st, r, x, maybe, certain);
                        }
                    }
                    V::Other
                }
                Op::GetFlag(f) => s.flags[f as usize],
                Op::SetFlag(f, x) => {
                    let certain = sure[x.index()];
                    let x = val(x);
                    let flag = |st: &mut State, maybe: bool| {
                        let old = &mut st.flags[f as usize];
                        *old = if maybe { old.join(x) } else { x };
                        let bit = 1 << (16 + f as usize);
                        st.sure = if certain && !maybe {
                            st.sure | bit
                        } else {
                            st.sure & !bit
                        };
                    };
                    flag(&mut s, guard.is_some());
                    for (c, yes, no) in &mut cond {
                        let this = guard.is_some_and(|g| same(insts, *c, g));
                        let maybe = guard.is_some() && !this;
                        flag(yes, maybe);
                        if !this {
                            flag(no, maybe);
                        }
                    }
                    V::Other
                }
                Op::SetUserReg(..) | Op::SetCpsr { .. } | Op::SetSpsr { .. } => {
                    self.fail("changes mode or the PSRs".into())?;
                    V::Other
                }
                Op::Exit(e) => {
                    let x = xs.iter().find(|x| x.index == i).expect("an exit's info");
                    // Under a guard: where it holds, and the target as it
                    // is there.
                    let found =
                        guard.and_then(|g| cond.iter().position(|(c, ..)| same(insts, *c, g)));
                    // A summary's run, under a guard that is a certain
                    // constant here ([`State::sure`]): the exit is taken
                    // always or never, so a path that can't run (agbcc's
                    // `if (best == -1) … else if (v < low)` reads `low`,
                    // a register it never set, only once `best` is set)
                    // reads nothing (unit `native/nightmare-entry-reads`).
                    let taken = match guard.map(|g| (sure[g.index()], val(g))) {
                        Some((true, V::Const(c))) if self.sum.is_some() => Some(c != 0),
                        _ => None,
                    };
                    match found {
                        Some(j) => {
                            let (g, yes, no) = cond[j].clone();
                            if taken != Some(false) {
                                let t = under(insts, g, e).map_or(V::Other, val);
                                self.exit(x, e, t, &yes)?;
                            }
                            // The block goes on where the guard is false,
                            // and so do the values chosen by it.
                            s = no;
                            falses.push(g);
                            for j in 0..i {
                                if let Some(v) = pure(insts, insts[j].op, &vals, &falses) {
                                    vals[j] = v;
                                }
                            }
                        }
                        None if taken != Some(false) => {
                            let t = jump_target(e).map_or(V::Other, val);
                            self.exit(x, e, t, &s)?;
                        }
                        None => {}
                    }
                    // Nothing after an exit always taken runs.
                    if taken == Some(true) {
                        break;
                    }
                    V::Other
                }
                _ => V::Other,
            };
            vals[i] = v;
            sure[i] = matches!(v, V::Const(_))
                && match inst.op {
                    Op::GetReg(_) | Op::GetFlag(_) => held,
                    Op::Load { .. } => false,
                    op => op.operands().iter().all(|o| sure[o.index()]),
                };
            if let Some(sum) = &mut self.sum {
                sum.entry_uses(insts, i, at, &vals);
            }
        }
        Ok(())
    }

    /// Exit `x` (inst `e`, its target `t`, if it jumps) with state `s`:
    /// where it goes on in the function, or whether it returns with the
    /// registers kept.
    fn exit(&mut self, x: &ExitInfo, e: Exit, t: V, s: &State) -> Result<(), String> {
        let at = x.at;
        match x.plan {
            Plan::Static(k) => self.flow(k, s),
            Plan::Call { target, ret } => {
                if self.inner(target) {
                    return self.stop(format!(
                        "0x{at:08x}: calls into a library routine's code, 0x{target:08x}"
                    ));
                }
                // The callee's own proof is checked in `prove`.
                self.callees.insert(target);
                let mut after = s.clone();
                if let Some(sum) = &mut self.sum {
                    sum.call(&mut after, Some(target), at);
                    // A callee whose summary never returns (and lost no
                    // return to a hole): the code after the call runs only
                    // from jumps into it (agb's panic paths after their
                    // `bl`s), with the jumper's registers, not ours.
                    if sum.never_returns(target) {
                        return Ok(());
                    }
                }
                after.called(self.sum.as_ref().is_some_and(|s| s.keeps(target)));
                self.flow(ret, &after)
            }
            Plan::Swi { next } => {
                let Exit::Swi { comment, .. } = e else {
                    unreachable!("a swi's plan for {e:?}")
                };
                let n = if x.thumb {
                    comment as u8
                } else {
                    (comment >> 16) as u8
                };
                // The BIOS calls write r0-r3 at most, but the two resets.
                if n == 0x00 || n == 0x26 {
                    return self.fail(format!("0x{at:08x}: a reset"));
                }
                let mut after = s.clone();
                if let Some(sum) = &mut self.sum {
                    sum.swi(&mut after, n, at);
                }
                for r in &mut after.regs[..4] {
                    *r = V::Other;
                }
                after.flags = [V::Other; 4];
                after.sure = 0;
                self.flow(next, &after)
            }
            Plan::Dynamic {
                ret: None,
                target: None,
                local: false,
            } => {
                if t != V::Entry(14) {
                    return self.stop(format!("0x{at:08x}: a jump to an unknown place"));
                }
                if !s.kept() {
                    let what = if s.regs[13] == V::Sp(0) {
                        "r4-r11"
                    } else {
                        "sp"
                    };
                    self.fail(format!(
                        "0x{at:08x}: returns with {what} changed: {}",
                        s.changed()
                    ))?;
                }
                if let Some(sum) = &mut self.sum {
                    sum.ret(s, at);
                }
                Ok(())
            }
            Plan::Dynamic { ret: Some(ret), .. } | Plan::CallVia { ret, .. } => {
                self.fail(format!("0x{at:08x}: an indirect call"))?;
                let mut after = s.clone();
                if let Some(sum) = &mut self.sum {
                    sum.call(&mut after, None, at);
                }
                after.called(false);
                self.flow(ret, &after)
            }
            Plan::Dynamic { .. } => {
                if matches!(e, Exit::ExceptionReturn(_)) {
                    return self.fail(format!("0x{at:08x}: an exception return"));
                }
                for k in successors(self.ctx, x) {
                    self.flow(k, s)?;
                }
                Ok(())
            }
            Plan::Rekey(_) => self.stop(format!("0x{at:08x}: changes mode")),
            Plan::Undefined => self.fail(format!("0x{at:08x}: an undefined instruction")),
        }
    }
}

fn bin(op: BinOp, a: V, b: V) -> V {
    use V::*;
    match (op, a, b) {
        (BinOp::Add, Sp(k), Const(c)) | (BinOp::Add, Const(c), Sp(k)) => {
            Sp(k.wrapping_add(c as i32))
        }
        (BinOp::Sub, Sp(k), Const(c)) => Sp(k.wrapping_sub(c as i32)),
        // The entry sp is word-aligned, as the ARM procedure call standard
        // keeps it: an address's low bits, for an `ldr`'s rotation.
        (BinOp::And, Sp(k), Const(c)) | (BinOp::And, Const(c), Sp(k)) if c <= 3 => {
            Const(k as u32 & c)
        }
        // A `pop {pc}` or `ldm {.., pc}` clears the return address's low
        // bits. Returning elsewhere than lr is still safe: the caller drops
        // its reloads only when control comes back to its return site.
        (BinOp::And, Entry(14), Const(c)) | (BinOp::And, Const(c), Entry(14)) if c | 3 == !0 => {
            Entry(14)
        }
        (op, Const(a), Const(b)) => Const(match op {
            BinOp::Add => a.wrapping_add(b),
            BinOp::Sub => a.wrapping_sub(b),
            BinOp::And => a & b,
            BinOp::Or => a | b,
            BinOp::Xor => a ^ b,
            BinOp::Mul => a.wrapping_mul(b),
        }),
        _ => Other,
    }
}

/// Exit `e`'s target, if it jumps.
fn jump_target(e: Exit) -> Option<ValueId> {
    match e {
        Exit::Jump(t) | Exit::JumpExchange(t) => Some(t),
        _ => None,
    }
}

/// Exit `e`'s target where guard `g` holds: a select on `g` is its first
/// arm (a register written under `g`, then a jump to it under `g`).
fn under(insts: &[Inst], g: ValueId, e: Exit) -> Option<ValueId> {
    let mut t = jump_target(e)?;
    while let Op::Select { c, a, .. } = insts[t.index()].op {
        if !same(insts, c, g) {
            break;
        }
        t = a;
    }
    Some(t)
}

/// Whether bits `a` and `b` are the same condition: one value, or the
/// same pure bit op of the same operands (each conditional instruction
/// builds its condition anew from the flags).
fn same(insts: &[Inst], a: ValueId, b: ValueId) -> bool {
    if a == b {
        return true;
    }
    let (x, y) = (&insts[a.index()], &insts[b.index()]);
    if x.guard.is_some() || y.guard.is_some() || !x.op.is_pure_bit() || !y.op.is_pure_bit() {
        return false;
    }
    let (xs, ys) = (x.op.operands(), y.op.operands());
    if xs.len() != ys.len() || !xs.iter().zip(&ys).all(|(&p, &q)| same(insts, p, q)) {
        return false;
    }
    // The ops with their operands taken out.
    let (mut x, mut y) = (x.op, y.op);
    for v in x.operands_mut().into_iter().chain(y.operands_mut()) {
        *v = ValueId(0);
    }
    x == y
}

/// A store of `value` at `addr`, in every case or (`maybe`) only maybe.
fn store(s: &mut State, width: Width, addr: V, value: V, maybe: bool) {
    let V::Sp(o) = addr else { return };
    let n = match width {
        Width::Byte => 1,
        Width::Half => 2,
        Width::Word => 4,
    };
    let old = s.slots.get(&o).copied();
    s.slots.retain(|&k, _| k + 4 <= o || o + n <= k);
    let new = match (n, maybe, old) {
        (4, false, _) => Some(value),
        (4, true, Some(old)) => Some(old.join(value)),
        _ => None,
    };
    s.slots.extend(new.map(|v| (o, v)));
}

/// Register `r` set to `x`, in every case or (`maybe`) only maybe;
/// `certain`: `x` is, where a constant ([`State::sure`]).
fn set(s: &mut State, r: usize, x: V, maybe: bool, certain: bool) {
    s.regs[r] = if maybe { s.regs[r].join(x) } else { x };
    s.sure = if certain && !maybe {
        s.sure | 1 << r
    } else {
        s.sure & !(1 << r)
    };
    if !maybe && (4..12).contains(&r) {
        s.chg[r - 4] = 0;
    }
}

/// The bits a load or store of `width` moves.
fn width_mask(width: Width) -> u32 {
    match width {
        Width::Byte => 0xff,
        Width::Half => 0xffff,
        Width::Word => !0,
    }
}

/// The value of a pure op, from its operands' values, where each of
/// `falses` is false (a select on one is its second arm); `None` for the
/// rest. One it knows nothing more of whose operands hold bits of
/// r4–r11's entry values holds the bits of them that survive it
/// ([`spread`]).
fn pure(insts: &[Inst], op: Op, vals: &[V], falses: &[ValueId]) -> Option<V> {
    let known = pure_known(insts, op, vals, falses);
    if matches!(known, None | Some(V::Other))
        && let Some(v) = spread(insts, op, vals)
    {
        return Some(v);
    }
    known
}

/// Bits whose dependence an add, a subtract or a multiply carries up:
/// every bit from the lowest of `bits`.
fn up(bits: u32) -> u32 {
    if bits == 0 {
        0
    } else {
        !0 << bits.trailing_zeros()
    }
}

/// What bits of r4–r11's entry values pure op `op` passes on, if an
/// operand holds some: [`V::Other`] where none survive (an `and` with a
/// constant that clears every one, as agbcc's `ands r4, r0; orrs r4, r1`
/// building a word byte by byte in an unset register comes to). `None`
/// if no operand holds any, or `op` isn't pure.
fn spread(insts: &[Inst], op: Op, vals: &[V]) -> Option<V> {
    let val = |v: ValueId| vals[v.index()];
    let t = |v: ValueId| val(v).taint().unwrap_or((0, 0));
    let konst = |v: ValueId| match val(v) {
        V::Const(c) => Some(c),
        _ => None,
    };
    // A bit-valued op of any: a bit that may depend on them.
    let bit = |vs: &[ValueId]| {
        let regs = vs.iter().fold(0, |m, &v| m | t(v).0);
        V::part(regs, 1)
    };
    let ops = op.operands();
    if ops.iter().all(|&v| val(v).taint().is_none()) {
        return None;
    }
    let all = ops
        .iter()
        .fold((0, 0), |(r, b), &v| (r | t(v).0, b | t(v).1));
    Some(match op {
        Op::Bin(k, a, b) => {
            let (ra, ba) = t(a);
            let (rb, bb) = t(b);
            let regs = ra | rb;
            // An operand's bits a constant other operand lets through.
            let masked = |keep: &dyn Fn(u32) -> u32| match (konst(a), konst(b)) {
                (Some(c), _) => bb & keep(c),
                (_, Some(c)) => ba & keep(c),
                _ => ba | bb,
            };
            let bits = match k {
                BinOp::And => masked(&|c| c),
                BinOp::Or => masked(&|c| !c),
                BinOp::Xor => ba | bb,
                BinOp::Add | BinOp::Sub => up(ba | bb),
                // A multiple of 2^k: the low k bits are clear.
                BinOp::Mul => match (konst(a), konst(b)) {
                    (Some(0), _) | (_, Some(0)) => 0,
                    (Some(c), _) | (_, Some(c)) => up(ba | bb) << c.trailing_zeros(),
                    _ => up(ba | bb),
                },
            };
            V::part(regs, bits)
        }
        Op::Not(a) => V::part(t(a).0, t(a).1),
        // `sbcs r5, r5` (modern GCC's 0 or -1 of a compare): the carry in
        // extended to a word and carried out again, whatever r5 holds.
        Op::Adc { a, b, c } if complement(insts, a, b) => V::part(t(c).0, !0),
        Op::AdcCarry { a, b, c } | Op::AdcOverflow { a, b, c } if complement(insts, a, b) => {
            V::part(t(c).0, 1)
        }
        Op::Adc { .. } => V::part(all.0, up(all.1)),
        Op::MulLongLo { .. } | Op::MulLongHi { .. } => V::part(all.0, !0),
        Op::Shift { kind, v, amount } => match (konst(amount), val(v).taint()) {
            (Some(n), Some((regs, bits))) => {
                V::part(regs, armv4t::sem::prim::shift(kind, bits, n, false).0)
            }
            // Shifted by an amount that may depend on them, or by an
            // unknown one.
            _ => V::part(all.0, !0),
        },
        Op::Rrx { v, carry_in } => {
            let (rv, bv) = t(v);
            let (rc, bc) = t(carry_in);
            V::part(rv | rc, armv4t::sem::prim::rrx(bv, bc != 0).0)
        }
        Op::Sext { v, bits } => {
            let (regs, b) = t(v);
            V::part(regs, armv4t::sem::prim::sext(b, bits))
        }
        Op::BitToWord(v) => V::part(t(v).0, 1),
        Op::TestBit { v, n } => V::part(t(v).0, t(v).1.checked_shr(n).unwrap_or(0) & 1),
        Op::IsZero(_)
        | Op::AdcCarry { .. }
        | Op::AdcOverflow { .. }
        | Op::MulCarry { .. }
        | Op::ShiftCarry { .. }
        | Op::BNot(_)
        | Op::BBin(..)
        | Op::BSelect { .. } => bit(&ops),
        _ => return None,
    })
}

/// Whether one of `a` and `b` is the other's complement: `sbc rN, rN`'s
/// operands (the builder reads a register once an instruction).
fn complement(insts: &[Inst], a: ValueId, b: ValueId) -> bool {
    let of = |a: ValueId, b: ValueId| matches!(insts[b.index()].op, Op::Not(x) if x == a);
    of(a, b) || of(b, a)
}

/// [`pure`] without [`spread`].
fn pure_known(insts: &[Inst], op: Op, vals: &[V], falses: &[ValueId]) -> Option<V> {
    let val = |v: ValueId| vals[v.index()];
    Some(match op {
        Op::Const(c) | Op::Pool(c) => V::Const(c),
        Op::Bit(b) => V::Const(b as u32),
        Op::Bin(op, a, b) => bin(op, val(a), val(b)),
        // `sub sp, #4` is `sp + !4 + 1`.
        Op::Adc { a, b, c } => {
            let ab = bin(BinOp::Add, val(a), val(b));
            bin(BinOp::Add, ab, val(c))
        }
        Op::Select { c, a, b } => {
            if falses.iter().any(|&g| same(insts, g, c)) {
                val(b)
            } else {
                val(a).join(val(b))
            }
        }
        // An ARM `ldr` rotates by its address's low bits, none at sp plus
        // a multiple of 4: a shift by 0 leaves the value.
        Op::Shift { v, amount, .. } if val(amount) == V::Const(0) => val(v),
        op => {
            // Any other pure op of constants: its value.
            let consts: Option<Vec<u32>> = op
                .operands()
                .into_iter()
                .map(|v| match val(v) {
                    V::Const(c) => Some(c),
                    _ => None,
                })
                .collect();
            let consts = consts?;
            if consts.is_empty() {
                return None;
            }
            let c = |v: ValueId| match val(v) {
                V::Const(c) => c,
                _ => 0,
            };
            V::Const(op.eval_pure(c, |v| c(v) != 0)?)
        }
    })
}
