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
//! Two things it assumes, which every function compiled from C relies on:
//! a store through an address it can't place (not the entry sp plus a
//! constant), or above the entry sp, doesn't hit the slots a prologue
//! (this function's or a caller's) saved registers in, and
//! an IRQ handler restores r4–r11 (taken in IRQ mode, its sp and lr are
//! banked). fn-diff's `saves` form checks the result against the
//! reference on recorded states.

use crate::func::{Ctx, ExitInfo, Func, GBlock, Plan, exits, successors};
use analysis::ir::{BinOp, Exit, Op, ValueId};
use armv4t::sem::Width;
use std::collections::{BTreeMap, BTreeSet};

mod summary;
pub use summary::{
    Assume, Bits, Category, Cause, Comparison, Summaries, Summary, Verdict, compare, proto_regs,
    summarize, summarize_with,
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
    /// The entry sp plus this.
    Sp(i32),
    Const(u32),
    Other,
}

impl V {
    fn join(self, o: V) -> V {
        if self == o { self } else { V::Other }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    regs: [V; 16],
    /// Words on the stack, by offset from the entry sp.
    slots: BTreeMap<i32, V>,
    /// Where r0-r3 and r12 came from ([`summary`]); empty in the proof.
    org: Vec<Org>,
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
        State { regs, slots, org }
    }

    /// r4–r11 and sp as they came.
    fn kept(&self) -> bool {
        (0..16)
            .filter(|i| KEPT & 1 << i != 0)
            .all(|i| self.regs[i] == if i == 13 { V::Sp(0) } else { V::Entry(i as u8) })
    }

    /// After a call into code that keeps r4–r11 and sp: the rest are
    /// unknown, and so is the stack below sp, where the callee's frame
    /// was.
    fn called(&mut self) {
        for i in [0, 1, 2, 3, 12, 14] {
            self.regs[i] = V::Other;
        }
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
    for f in funcs {
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
        work: vec![f.entry],
        callees: BTreeSet::new(),
        sum,
    };
    if f.overlay.is_some() {
        w.fail("runs from IWRAM".into())?;
    }
    // Each block's in-state only loses facts (a summary's origins only
    // grow), so this ends; the cap is a guard.
    let mut steps = 0;
    while let Some(k) = w.work.pop() {
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
    work: Vec<u32>,
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

    /// Go on at `to` with `s`.
    fn flow(&mut self, to: u32, s: &State) -> Result<(), String> {
        if !self.blocks.contains_key(&to) {
            // A tail call: the callee returns to our caller.
            let func = self.ctx.disc.funcs.contains_key(&to) && self.ctx.entries.contains(&to);
            if func && s.kept() && s.regs[14] == V::Entry(14) {
                self.callees.insert(to);
                if let Some(sum) = &mut self.sum {
                    // A call, then a return.
                    let mut after = s.clone();
                    sum.call(&mut after, Some(to), to);
                    sum.ret(&after);
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
            self.work.push(to);
        }
        Ok(())
    }

    /// Run block `k` from its in-state.
    fn block(&mut self, k: u32) -> Result<(), String> {
        let gb = self.blocks[&k];
        let mut s = self.ins[&k].clone();
        let mut vals = vec![V::Other; gb.block.insts.len()];
        let xs = exits(self.ctx, gb);
        // The values something uses: a read whose value nothing uses
        // reads nothing.
        let used: BTreeSet<ValueId> = match self.sum {
            Some(_) => gb
                .block
                .insts
                .iter()
                .flat_map(|i| i.op.operands().into_iter().chain(i.guard))
                .collect(),
            None => BTreeSet::new(),
        };
        let mut at = gb.block.start;
        for (i, inst) in gb.block.insts.iter().enumerate() {
            let val = |v: ValueId| vals[v.index()];
            let guarded = inst.guard.is_some();
            let v = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    V::Other
                }
                Op::Const(c) | Op::Pool(c) => V::Const(c),
                Op::GetReg(r) => {
                    if let Some(sum) = &mut self.sum
                        && used.contains(&ValueId(i as u32))
                    {
                        sum.read(&s, r.index(), Cause::Reads(at));
                    }
                    s.regs[r.index()]
                }
                Op::Bit(b) => V::Const(b as u32),
                Op::Bin(op, a, b) => bin(op, val(a), val(b)),
                // `sub sp, #4` is `sp + !4 + 1`.
                Op::Adc { a, b, c } => {
                    let ab = bin(BinOp::Add, val(a), val(b));
                    bin(BinOp::Add, ab, val(c))
                }
                Op::Select { a, b, .. } => val(a).join(val(b)),
                Op::Load {
                    width: Width::Word,
                    addr,
                } => match val(addr) {
                    V::Sp(o) => s.slots.get(&o).copied().unwrap_or(V::Other),
                    _ => V::Other,
                },
                Op::Store { width, addr, value } => {
                    if let V::Sp(o) = val(addr) {
                        let n = match width {
                            Width::Byte => 1,
                            Width::Half => 2,
                            Width::Word => 4,
                        };
                        let old = s.slots.get(&o).copied();
                        s.slots.retain(|&k, _| k + 4 <= o || o + n <= k);
                        let new = match (n, guarded, old) {
                            (4, false, _) => Some(val(value)),
                            (4, true, Some(old)) => Some(old.join(val(value))),
                            _ => None,
                        };
                        s.slots.extend(new.map(|v| (o, v)));
                    }
                    V::Other
                }
                Op::SetReg(r, x) => {
                    let r = r.index();
                    if let Some(sum) = &self.sum {
                        sum.set(&mut s, r, guarded);
                    }
                    s.regs[r] = if guarded {
                        s.regs[r].join(val(x))
                    } else {
                        val(x)
                    };
                    V::Other
                }
                Op::SetUserReg(..) | Op::SetCpsr { .. } | Op::SetSpsr { .. } => {
                    self.fail("changes mode or the PSRs".into())?;
                    V::Other
                }
                Op::Exit(e) => {
                    let x = xs.iter().find(|x| x.index == i).expect("an exit's info");
                    let t = match e {
                        Exit::Jump(t) | Exit::JumpExchange(t) => val(t),
                        _ => V::Other,
                    };
                    self.exit(x, e, t, &s)?;
                    V::Other
                }
                _ => V::Other,
            };
            vals[i] = v;
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
                // The callee's own proof is checked in `prove`.
                self.callees.insert(target);
                let mut after = s.clone();
                if let Some(sum) = &mut self.sum {
                    sum.call(&mut after, Some(target), at);
                }
                after.called();
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
                    self.fail(format!("0x{at:08x}: returns with r4-r11 or sp changed"))?;
                }
                if let Some(sum) = &mut self.sum {
                    sum.ret(s);
                }
                Ok(())
            }
            Plan::Dynamic { ret: Some(ret), .. } | Plan::CallVia { ret, .. } => {
                self.fail(format!("0x{at:08x}: an indirect call"))?;
                let mut after = s.clone();
                if let Some(sum) = &mut self.sum {
                    sum.call(&mut after, None, at);
                }
                after.called();
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
