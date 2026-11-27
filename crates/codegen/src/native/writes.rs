//! What RAM a function may store to, itself or through what it calls
//! (unit `native/lift-for-ram-bounds`): a loop counting to a value it
//! loads from RAM each round prints as `for` only where nothing in a
//! round may store to the bound's bytes ([`super::bounds`]).
//!
//! A function's [`Writes`] are the RAM byte ranges its constant-address
//! stores and those Recovery's value sets bound reach, or `any`: a store
//! it can't place, a call through a pointer, a BIOS call that writes
//! memory (all but the arithmetic ones), an I/O store that may start a
//! DMA. A call through a pointer whose callee set the function flow
//! gives complete calls those (unit `native/lift-for-ram-bounds-games`). Its callees' (direct calls, tail jumps and falls into another
//! function) are joined in, to a fixed point; a stand-in's or one outside
//! the context is `any`. Not counted:
//! - a store to its own stack frame (sp-relative, below its entry sp):
//!   a fresh frame no caller's object is in. One to its caller's frame
//!   is `any`;
//! - a store whose address is a sum with a constant outside RAM and I/O
//!   (`0x0600_0000 + y * 480 + x * 2`, a VRAM pixel): the trust is C's,
//!   that pointer arithmetic stays in its object ([`Prov`]), as the
//!   function flow takes a pointer into dull bytes as a number;
//! - the IRQ handlers' stores, which [`ram_writes`] gives on their own:
//!   a loop's check joins them in.
//!
//! A jump by value that isn't a call or a table's (a return) adds
//! nothing.

use super::Holes;
use crate::func::{Ctx, Func, Plan, exits};
use analysis::ir::{BinOp, Exit, Inst, Op};
use armv4t::sem::ShiftType;
use std::collections::BTreeMap;

/// Past this many ranges, a function's writes are `any`.
const MAX_RANGES: usize = 64;

/// RAM a function may store to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Writes {
    /// Anywhere.
    pub any: bool,
    /// Byte ranges `lo..hi`, mirrors folded ([`fold`]), sorted and merged.
    pub ranges: Vec<(u32, u32)>,
    /// A block's: it stores to its function's own frame.
    pub stack: bool,
}

impl Writes {
    pub fn any() -> Writes {
        Writes {
            any: true,
            ranges: Vec::new(),
            stack: false,
        }
    }

    /// Nothing in RAM.
    pub fn none(&self) -> bool {
        !self.any && self.ranges.is_empty()
    }

    pub fn add(&mut self, lo: u32, hi: u32) {
        if self.any {
            return;
        }
        for r in fold(lo, hi) {
            self.ranges.push(r);
        }
        self.merge();
    }

    pub fn join(&mut self, o: &Writes) {
        self.stack |= o.stack;
        if self.any {
            return;
        }
        if o.any {
            *self = Writes::any();
            return;
        }
        self.ranges.extend(o.ranges.iter().copied());
        self.merge();
    }

    fn merge(&mut self) {
        self.ranges.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::new();
        for &(a, b) in &self.ranges {
            match out.last_mut() {
                Some(l) if a <= l.1 => l.1 = l.1.max(b),
                _ => out.push((a, b)),
            }
        }
        self.ranges = out;
        if self.ranges.len() > MAX_RANGES {
            *self = Writes::any();
        }
    }

    /// Whether it may store to a byte of `lo..hi` (RAM).
    pub fn hits(&self, lo: u32, hi: u32) -> bool {
        self.any
            || fold(lo, hi)
                .iter()
                .any(|&(a, b)| self.ranges.iter().any(|&(c, d)| a < d && c < b))
    }
}

/// `lo..hi` in RAM with its mirrors folded onto the first copy: EWRAM's
/// 256 KiB at `0x0200_0000`, IWRAM's 32 KiB at `0x0300_0000`; a range
/// that wraps a copy's end, or isn't in RAM, is the whole of its region's.
pub fn fold(lo: u32, hi: u32) -> Vec<(u32, u32)> {
    let region = |a: u32| match a >> 24 {
        0x02 => Some((0x0200_0000u32, 0x4_0000u32)),
        0x03 => Some((0x0300_0000, 0x8000)),
        _ => None,
    };
    let (Some(a), Some(b)) = (region(lo), region(hi.saturating_sub(1).max(lo))) else {
        // Partly outside RAM: all of it.
        return vec![(0x0200_0000, 0x0204_0000), (0x0300_0000, 0x0300_8000)];
    };
    if a != b || hi.saturating_sub(lo) >= a.1 {
        return [a, b].iter().map(|&(base, n)| (base, base + n)).collect();
    }
    let (base, n) = a;
    let l = base + (lo - base) % n;
    let h = l + (hi - lo);
    if h > base + n {
        vec![(base, base + n)]
    } else {
        vec![(l, h)]
    }
}

/// Where a value may point, for a store's address: a constant, or one
/// plus an index (anything not itself a pointer: the trust is C's, that
/// pointer arithmetic stays in its object), scaled as the index may be:
/// GCC makes VRAM's `0x0600_0000 + x * 2` as `(0x0300_0000 + x) << 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prov {
    /// Not reached.
    Bot,
    Const(u32),
    /// The constant plus an index.
    Off(u32),
    /// Anything: a pointer, maybe.
    Any,
}

/// Which region a sum with constant `k` is in: 0 RAM or anything, 1
/// I/O, 2 the display memories, ROM or SRAM.
fn class(k: u32) -> u8 {
    match k >> 24 {
        0x04 => 1,
        0x05..=0x0f => 2,
        _ => 0,
    }
}

impl Prov {
    fn is_const(self) -> bool {
        matches!(self, Prov::Const(_))
    }

    fn join(self, o: Prov) -> Prov {
        match (self, o) {
            (Prov::Bot, x) | (x, Prov::Bot) => x,
            (a, b) if a == b => a,
            (Prov::Const(a) | Prov::Off(a), Prov::Const(b) | Prov::Off(b))
                if class(a) != 0 && class(a) == class(b) =>
            {
                Prov::Off(a.min(b))
            }
            _ => Prov::Any,
        }
    }

    fn add(self, o: Prov) -> Prov {
        match (self, o) {
            (Prov::Bot, _) | (_, Prov::Bot) => Prov::Bot,
            (Prov::Const(a), Prov::Const(b)) => Prov::Const(a.wrapping_add(b)),
            (Prov::Const(a) | Prov::Off(a), Prov::Any)
            | (Prov::Any, Prov::Const(a) | Prov::Off(a)) => Prov::Off(a),
            (Prov::Const(a), Prov::Off(b)) | (Prov::Off(a), Prov::Const(b)) => {
                Prov::Off(a.wrapping_add(b))
            }
            // Two pointers summed, or two unknowns: nothing known.
            _ => Prov::Any,
        }
    }

    fn sub(self, o: Prov) -> Prov {
        match (self, o) {
            (Prov::Bot, _) | (_, Prov::Bot) => Prov::Bot,
            (Prov::Const(a), Prov::Const(b)) => Prov::Const(a.wrapping_sub(b)),
            (Prov::Off(a), Prov::Const(b)) => Prov::Off(a.wrapping_sub(b)),
            (Prov::Const(a) | Prov::Off(a), Prov::Any) => Prov::Off(a),
            _ => Prov::Any,
        }
    }

    /// Scaled by `f(k)`, the index too.
    fn scale(self, o: Prov, f: impl Fn(u32, u32) -> u32) -> Prov {
        match (self, o) {
            (Prov::Bot, _) | (_, Prov::Bot) => Prov::Bot,
            (Prov::Const(a), Prov::Const(b)) => Prov::Const(f(a, b)),
            (Prov::Off(a), Prov::Const(b)) => Prov::Off(f(a, b)),
            _ => Prov::Any,
        }
    }
}

/// Each value's [`Prov`] in `insts`, from the registers' `regs`; `regs` is
/// left as the block leaves them.
fn provs(insts: &[Inst], regs: &mut [Prov; 16]) -> Vec<Prov> {
    let mut p = vec![Prov::Any; insts.len()];
    for (i, inst) in insts.iter().enumerate() {
        let g = |x: analysis::ir::ValueId| p[x.index()];
        p[i] = match inst.op {
            Op::Const(k) | Op::Pool(k) => Prov::Const(k),
            Op::GetReg(q) => regs[q.index()],
            Op::SetReg(q, x) => {
                let v = g(x);
                regs[q.index()] = match inst.guard {
                    None => v,
                    Some(_) => regs[q.index()].join(v),
                };
                Prov::Any
            }
            Op::SetCpsr { .. } | Op::SetUserReg(..) => {
                *regs = [Prov::Any; 16];
                Prov::Any
            }
            Op::Not(x) => match g(x) {
                Prov::Const(k) => Prov::Const(!k),
                _ => Prov::Any,
            },
            Op::Bin(BinOp::Add, a, b) => g(a).add(g(b)),
            Op::Bin(BinOp::Sub, a, b) => g(a).sub(g(b)),
            Op::Bin(BinOp::Mul, a, b) => match (g(a), g(b)) {
                (x, k @ Prov::Const(_)) | (k @ Prov::Const(_), x) => x.scale(k, u32::wrapping_mul),
                _ => Prov::Any,
            },
            // An alignment mask keeps the region.
            Op::Bin(BinOp::And, a, b) => match (g(a), g(b)) {
                (x, Prov::Const(m)) | (Prov::Const(m), x) if m >= 0xff00_0000 || x.is_const() => {
                    x.scale(Prov::Const(m), |k, m| k & m)
                }
                _ => Prov::Any,
            },
            Op::Bin(o, a, b) => match (g(a), g(b)) {
                (Prov::Const(x), Prov::Const(y)) => Prov::Const(match o {
                    BinOp::Or => x | y,
                    BinOp::Xor => x ^ y,
                    _ => unreachable!(),
                }),
                _ => Prov::Any,
            },
            Op::Adc { a, b, c } => match (insts[c.index()].op, insts[b.index()].op) {
                (Op::Bit(true), Op::Not(y)) => g(a).sub(g(y)),
                (Op::Bit(carry), _) => g(a).add(g(b)).add(Prov::Const(u32::from(carry))),
                _ => Prov::Any,
            },
            Op::Shift { kind, v, amount } => match (g(v), g(amount)) {
                (Prov::Const(x), Prov::Const(n)) => {
                    Prov::Const(armv4t::sem::prim::shift(kind, x, n & 0xff, false).0)
                }
                (Prov::Off(x), Prov::Const(n)) if kind == ShiftType::Lsl && n < 32 => {
                    Prov::Off(x << n)
                }
                _ => Prov::Any,
            },
            _ => Prov::Any,
        };
    }
    p
}

/// The BIOS calls that store nothing: the arithmetic ones, and those that
/// wait for an interrupt (whose handlers [`ram_writes`] gives apart).
fn swi_quiet(comment: u32, thumb: bool) -> bool {
    let n = if thumb {
        comment & 0xff
    } else {
        comment >> 16 & 0xff
    };
    matches!(
        n,
        0x02 | 0x04 | 0x05 | 0x06 | 0x07 | 0x08 | 0x09 | 0x0a | 0x0d
    )
}

/// The stores of each of a function's own blocks (a call's, a pointer
/// call's or a BIOS call's `any` among them, a direct callee's not), and
/// the functions it runs ([`Writes`]).
fn local(
    ctx: &Ctx,
    holes: &Holes,
    f: &Func,
    frame: Option<&super::Frame>,
    sites: &BTreeMap<u32, super::fnref::Site>,
) -> (BTreeMap<u32, Writes>, Vec<u32>) {
    let mut by_block = BTreeMap::new();
    let mut callees = Vec::new();
    // Each block's registers' provenance at its start, to a fixed point.
    let mut at_start: BTreeMap<u32, [Prov; 16]> = BTreeMap::new();
    at_start.insert(f.entry, [Prov::Any; 16]);
    let mut todo: Vec<u32> = vec![f.entry];
    let mut ends: BTreeMap<u32, Vec<Prov>> = BTreeMap::new();
    loop {
        // A block the entry doesn't reach (an extended entry's): from
        // registers that may hold anything.
        if todo.is_empty() {
            let Some(&k) = f.blocks.keys().find(|k| !at_start.contains_key(k)) else {
                break;
            };
            at_start.insert(k, [Prov::Any; 16]);
            todo.push(k);
        }
        let Some(k) = todo.pop() else { break };
        let Some(b) = f.blocks.get(&k) else { continue };
        let mut regs = at_start[&k];
        let p = provs(&b.block.insts, &mut regs);
        ends.insert(k, p);
        for x in exits(ctx, b) {
            // The registers as the exit leaves them.
            let mut regs = at_start[&k];
            provs(&b.block.insts[..x.index], &mut regs);
            let clobbered = || {
                let mut r = [Prov::Any; 16];
                r[13] = regs[13];
                r
            };
            let (succs, out): (Vec<u32>, [Prov; 16]) = match x.plan {
                Plan::Static(t) | Plan::Rekey(t) => (vec![t], regs),
                Plan::Call { ret, .. } | Plan::CallVia { ret, .. } | Plan::Swi { next: ret } => {
                    (vec![ret], clobbered())
                }
                Plan::Dynamic { ret: Some(r), .. } => (vec![r], clobbered()),
                Plan::Dynamic {
                    ret: None, target, ..
                } => (
                    super::dyn_targets(ctx, holes, x.at, x.thumb, target)
                        .unwrap_or_else(|_| target.into_iter().collect()),
                    regs,
                ),
                Plan::Undefined { .. } | Plan::Unwind => (vec![], regs),
            };
            for t in succs {
                if !f.blocks.contains_key(&t) {
                    continue;
                }
                let old = at_start.get(&t).copied();
                let new = match old {
                    None => out,
                    Some(o) => std::array::from_fn(|q| o[q].join(out[q])),
                };
                if old != Some(new) {
                    at_start.insert(t, new);
                    todo.push(t);
                }
            }
        }
    }
    for (&k, b) in &f.blocks {
        let insts = &b.block.insts;
        let w = by_block.entry(k).or_insert_with(Writes::default);
        let p = &ends[&k];
        let mut at = b.block.start;
        for (i, inst) in insts.iter().enumerate() {
            let (width, addr) = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    continue;
                }
                Op::Store { width, addr, .. } => (width, addr),
                _ => continue,
            };
            let n = match width {
                armv4t::sem::Width::Byte => 1u32,
                armv4t::sem::Width::Half => 2,
                armv4t::sem::Width::Word => 4,
            };
            match p[addr.index()] {
                Prov::Const(a) => {
                    let a = a & !(n - 1);
                    match a >> 24 {
                        0x02 | 0x03 => w.add(a, a + n),
                        // DMA's registers: a copy into anywhere.
                        0x04 if (0x0400_00b0..0x0400_00e0).contains(&a) => w.any = true,
                        0x04..=0x0f => {}
                        _ => w.any = true,
                    }
                    continue;
                }
                Prov::Off(k) if class(k) == 2 => continue,
                _ => {}
            }
            match ctx.disc.rec.accesses.get(&at) {
                Some(x) if x.stack_only() => {
                    // Its own frame, or its caller's.
                    if frame.is_some_and(|fr| fr.own_at(k, i)) {
                        w.stack = true;
                    } else {
                        w.any = true;
                    }
                }
                Some(x) if !x.top && !x.sp => {
                    if let Some((lo, hi)) = x.hull {
                        match (lo >> 24, hi >> 24) {
                            (0x02 | 0x03, _) | (_, 0x02 | 0x03) | (0x04, _) | (_, 0x04) => {
                                w.add(lo & !(n - 1), hi.saturating_add(n))
                            }
                            (0x05..=0x0f, 0x05..=0x0f) => {}
                            _ => w.any = true,
                        }
                    }
                }
                _ => w.any = true,
            }
            // An I/O hull may reach DMA's registers.
            if let Some(x) = ctx.disc.rec.accesses.get(&at)
                && let Some((lo, hi)) = x.hull
                && lo < 0x0400_00e0
                && hi >= 0x0400_00b0
                && !x.top
                && !x.sp
            {
                w.any = true;
            }
        }
        for x in exits(ctx, b) {
            match x.plan {
                Plan::Static(t) | Plan::Rekey(t) if !f.blocks.contains_key(&t) => callees.push(t),
                Plan::Static(_) | Plan::Rekey(_) | Plan::Undefined { .. } | Plan::Unwind => {}
                Plan::Call { target, .. } => callees.push(target),
                Plan::Swi { .. } => {
                    if let Op::Exit(Exit::Swi { comment, .. }) = insts[x.index].op
                        && swi_quiet(comment, x.thumb)
                    {
                        continue;
                    }
                    w.any = true;
                }
                // A call through a pointer: its callees where the function
                // flow's set is complete (unit
                // `native/lift-for-ram-bounds-games`), else anywhere.
                Plan::CallVia { .. } | Plan::Dynamic { ret: Some(_), .. } => {
                    match sites.get(&x.at) {
                        Some(s) if s.complete && !s.tail => {
                            callees.extend(s.callees.iter().copied())
                        }
                        _ => w.any = true,
                    }
                }
                Plan::Dynamic {
                    ret: None, target, ..
                } => {
                    if let Ok(ts) = super::dyn_targets(ctx, holes, x.at, x.thumb, target) {
                        callees.extend(ts.into_iter().filter(|t| !f.blocks.contains_key(t)));
                    } else if let Some(t) = target.filter(|t| !f.blocks.contains_key(t)) {
                        callees.push(t);
                    }
                }
            }
        }
    }
    (by_block, callees)
}

/// What [`ram_writes`] finds.
#[derive(Default)]
pub struct RamWrites {
    /// Each function's, its callees' joined in, by entry (a function's own
    /// frame left out).
    pub by_fn: BTreeMap<u32, Writes>,
    /// Each block's own, by function and block.
    pub by_block: BTreeMap<(u32, u32), Writes>,
    /// The IRQ handlers', joined.
    pub irq: Writes,
}

impl RamWrites {
    /// A call's of `k`'s: `any` for one outside every context.
    pub fn call(&self, k: u32) -> Writes {
        self.by_fn.get(&k).cloned().unwrap_or_else(Writes::any)
    }
}

/// [`RamWrites`] over every code context (`codes`: each one's functions
/// and frames), the IRQ handlers' `irq` entries; `sites` the settled
/// function flow's callee sets of the calls through a pointer
/// ([`super::fnref::Flow::sites`]), none where it didn't run or settle.
pub fn ram_writes<'a>(
    codes: impl IntoIterator<Item = (&'a Ctx<'a>, &'a [Func], &'a BTreeMap<u32, super::Frame>)>,
    holes: &Holes,
    irq: impl IntoIterator<Item = u32>,
    sites: &BTreeMap<u32, super::fnref::Site>,
) -> RamWrites {
    let mut by_block = BTreeMap::new();
    let mut own: BTreeMap<u32, (Writes, Vec<u32>)> = BTreeMap::new();
    for (ctx, funcs, frames) in codes {
        for f in funcs {
            let (bs, cs) = local(ctx, holes, f, frames.get(&f.entry), sites);
            let mut w = Writes::default();
            for (k, b) in bs {
                w.join(&b);
                by_block.insert((f.entry, k), b);
            }
            w.stack = false;
            if holes.standins.contains_key(&f.entry) {
                w = Writes::any();
            }
            own.insert(f.entry, (w, cs));
        }
    }
    let mut all: BTreeMap<u32, Writes> = own.iter().map(|(&k, (w, _))| (k, w.clone())).collect();
    loop {
        let mut changed = false;
        for (k, (_, cs)) in &own {
            let mut w = all[k].clone();
            for c in cs {
                match all.get(c) {
                    Some(cw) => w.join(cw),
                    None => w = Writes::any(),
                }
            }
            if w != all[k] {
                all.insert(*k, w);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let irq: Vec<u32> = irq.into_iter().collect();
    if std::env::var_os("GBAA_FOR_WHY").is_some() {
        irq_why(&irq, &own, &all, &by_block);
    }
    let mut on_irq = Writes::default();
    for k in irq {
        match all.get(&k) {
            Some(w) => on_irq.join(w),
            None => on_irq = Writes::any(),
        }
    }
    RamWrites {
        by_fn: all,
        by_block,
        irq: on_irq,
    }
}

/// Why the IRQ handlers may store anywhere (`GBAA_FOR_WHY`): each
/// function they reach whose own blocks may (by block), or a callee no
/// context has, to stderr.
fn irq_why(
    irq: &[u32],
    own: &BTreeMap<u32, (Writes, Vec<u32>)>,
    all: &BTreeMap<u32, Writes>,
    by_block: &BTreeMap<(u32, u32), Writes>,
) {
    let mut seen: Vec<u32> = irq.to_vec();
    let mut todo = seen.clone();
    while let Some(k) = todo.pop() {
        let Some((w, cs)) = own.get(&k) else {
            eprintln!("for-why irq: {k:#010x} in no context: anywhere");
            continue;
        };
        if w.any {
            let blocks: Vec<String> = by_block
                .range((k, 0)..=(k, u32::MAX))
                .filter(|(_, b)| b.any)
                .map(|((_, b), _)| format!("{b:#010x}"))
                .collect();
            eprintln!(
                "for-why irq: {k:#010x} may store anywhere, at {}",
                if blocks.is_empty() {
                    "a stand-in".to_string()
                } else {
                    blocks.join(" ")
                }
            );
        }
        for c in cs {
            if all.get(c).is_some_and(|w| w.any) && !seen.contains(c) {
                seen.push(*c);
                todo.push(*c);
            } else if all.get(c).is_none() {
                eprintln!("for-why irq: {k:#010x} calls {c:#010x}, in no context: anywhere");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_fold_and_ranges_merge() {
        assert_eq!(
            fold(0x0300_8010, 0x0300_8012),
            vec![(0x0300_0010, 0x0300_0012)]
        );
        // The BIOS's IRQ vector through IWRAM's last mirror.
        assert_eq!(
            fold(0x03ff_fffc, 0x0400_0000),
            vec![(0x0300_7ffc, 0x0300_8000)]
        );
        // Wrapping a copy's end: all of it.
        assert_eq!(
            fold(0x0300_7ffe, 0x0300_8002),
            vec![(0x0300_0000, 0x0300_8000)]
        );
        // Partly outside RAM: all of both.
        assert_eq!(fold(0x03ff_fffe, 0x0400_0002).len(), 2);
        let mut w = Writes::default();
        w.add(0x0300_0010, 0x0300_0014);
        w.add(0x0300_0014, 0x0300_0016);
        assert_eq!(w.ranges, vec![(0x0300_0010, 0x0300_0016)]);
        assert!(w.hits(0x0300_8015, 0x0300_8016));
        assert!(!w.hits(0x0300_0016, 0x0300_0017));
        assert!(!w.hits(0x0200_0010, 0x0200_0014));
    }

    #[test]
    fn a_sum_with_vram_stays_out_of_ram() {
        let k = Prov::Const;
        assert_eq!(class(0x0600_0000), 2);
        assert_eq!(k(0x0600_0000).add(Prov::Any), Prov::Off(0x0600_0000));
        assert_eq!(
            Prov::Any.add(k(0x0600_0000)).add(k(2)),
            Prov::Off(0x0600_0002)
        );
        // GCC's (0x0300_0000 + x) << 1.
        let gcc = k(0x0300_0000).add(Prov::Any).scale(k(2), u32::wrapping_mul);
        assert_eq!(
            class(match gcc {
                Prov::Off(a) => a,
                _ => 0,
            }),
            2
        );
        assert_eq!(Prov::Off(0x0600_0000).add(Prov::Off(0x10)), Prov::Any);
        assert_eq!(
            Prov::Off(0x0600_0000).join(k(0x0600_0002)),
            Prov::Off(0x0600_0000)
        );
        assert_eq!(Prov::Off(0x0600_0000).join(Prov::Any), Prov::Any);
        assert_eq!(k(1).join(k(2)), Prov::Any);
    }
}
