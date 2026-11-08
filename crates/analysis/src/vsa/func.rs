//! The per-function fixpoint: run [`exec::step`] forward from a function's
//! entry until the state at every reached instruction stops changing, then
//! read the jumps and returns off the final states.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use armv4t::decode::{AluOp, Index, Instr, Operand2, Reg as DReg, Shift};
use armv4t::sem::Reg;

use super::domain::{AbsVal, CLOBBER, Dom, JOIN, MAX_SET, Src, Sym};
use super::exec::{self, Exit, ExitKind, State, writes_pc};
use crate::program::{Insn, Program};

/// A merge point is widened once it has been joined into this many times,
/// and only if it is a loop head (the target of a backward edge). Widening
/// anywhere else would throw away the bound a check just established, before
/// the jump that uses it.
pub const WIDEN_AFTER: u32 = 6;
/// Instructions executed per function before giving up on a fixpoint.
pub const STEP_CAP: u64 = 2_000_000;
/// How often [`analyze_capped`] reruns the fixpoint for `bl`s into the
/// function's own code.
const INNER_TRIES: u32 = 4;
/// The longest straight-line callee run inline at its call.
const LEAF_MAX: usize = 24;
/// The most instructions [`Analysis::kept`] reads of a callee.
const KEPT_MAX: usize = 64;
/// The registers a call may change under the AAPCS that [`Analysis::kept`]
/// can keep: r0-r3 and r12.
const CALLER_SAVED: u16 = 0b1_0000_0000_1111;

/// How control leaves an instruction that writes pc indirectly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctl {
    Return,
    Call,
    Jump,
}

fn plain_reg(op2: &Operand2, r: DReg) -> bool {
    matches!(op2, Operand2::Reg { rm, shift: Shift::Lsl(0) } if *rm == r)
}

/// Classify an instruction that writes pc, other than `b` and `bl`
/// (`docs/research/jump-tables.md`, "Definitions"). Returns: `bx lr`,
/// `mov pc, lr`, any `ldm`/`pop` loading pc, `ldr pc, [sp], #4`, and
/// `bx rN` right after a `pop` that loads rN. Calls: `bx`/`mov pc` right
/// after `mov lr, pc`, and a lone second half of Thumb `bl`. Everything
/// else is a jump.
pub fn classify(prog: &Program, insn: &Insn) -> Ctl {
    let p = prog.prev(insn.addr, insn.thumb);
    let after_mov_lr_pc = matches!(
        p,
        Some(Insn { instr: Instr::Alu { op: AluOp::Mov, rd: DReg::Lr, op2, .. }, .. })
            if plain_reg(&op2, DReg::Pc)
    );
    match insn.instr {
        Instr::Bx { rm: DReg::Lr, .. } => Ctl::Return,
        Instr::Bx { rm, .. } => match p {
            Some(Insn {
                instr:
                    Instr::Block {
                        load: true,
                        rn: DReg::Sp,
                        regs,
                        ..
                    },
                ..
            }) if regs & (1 << rm as u16) != 0 => Ctl::Return,
            _ if after_mov_lr_pc => Ctl::Call,
            _ => Ctl::Jump,
        },
        Instr::Alu {
            op: AluOp::Mov,
            op2,
            ..
        } if plain_reg(&op2, DReg::Lr) => Ctl::Return,
        Instr::Block { .. } => Ctl::Return,
        Instr::Load { addr, .. }
            if addr.rn == DReg::Sp && matches!(addr.index, Index::Post { .. }) =>
        {
            Ctl::Return
        }
        Instr::ThumbBlLo { .. } => Ctl::Call,
        _ if after_mov_lr_pc => Ctl::Call,
        _ => Ctl::Jump,
    }
}

/// What a `bl` is, as the far-jump rule (decision 5a) answers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bl {
    #[default]
    Call,
    /// A far jump: the target is part of the caller.
    Jump,
    /// A call that never returns (LLVM's panics): nothing follows it.
    NoReturn,
    /// A call whose target returns with sp where it was on entry, wherever
    /// sp is known: a function of its own, even inside the caller's span.
    Balanced,
}

/// A resolved indirect jump. Targets are in interworking form (bit 0 set
/// for Thumb), sorted. `table` is where the target values were read from,
/// if they came from ROM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jump {
    pub targets: Vec<u32>,
    pub table: Option<Src>,
}

/// What became of one indirect jump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JumpRes {
    Resolved(Jump),
    /// The target isn't a small finite set of ROM addresses.
    Unresolved,
    /// A table with a target that doesn't decode or leaves the function.
    Rejected(Jump),
}

/// Whether code may hold the undefined instruction `insn` on purpose, so
/// that it doesn't make the code data ([`FuncResult::undefined`]): a
/// compiler's trap (`__builtin_trap`, a Rust abort: Thumb `udf #255`
/// from GCC or `udf #254` from LLVM, ARM 0xe7ffdefe from LLVM or
/// 0xe7f000f0 from GCC), or the word after an unconditional `bl`, which
/// may be a call that never returns (Super Mariomon's assert ends in
/// 0xefff after its print).
pub fn deliberate(prog: &Program, insn: &Insn) -> bool {
    let trap = if insn.thumb {
        matches!(prog.read(insn.addr, 2), Some(0xdeff | 0xdefe))
    } else {
        matches!(prog.read(insn.addr, 4), Some(0xe7ff_defe | 0xe7f0_00f0))
    };
    let before = insn.addr.checked_sub(4);
    trap || before
        .and_then(|p| prog.decode(p, insn.thumb))
        .is_some_and(|b| {
            b.next() == insn.addr
                && b.instr.cond() == armv4t::decode::Cond::Al
                && (b.bl.is_some() || matches!(b.instr, Instr::Branch { link: true, .. }))
        })
}

#[derive(Debug, Default)]
pub struct FuncResult {
    /// Instructions reached.
    pub reached: usize,
    /// Every indirect jump reached (by address), from the final states. A
    /// `bl` to a callee that returns to a computed address counts.
    pub jumps: BTreeMap<u32, JumpRes>,
    /// sp's offset from its entry value at each return reached, if known.
    pub ret_sp: Vec<Option<i32>>,
    /// The `bl`s taken as jumps (decision 5a's far-jump rule).
    pub far_jumps: BTreeSet<u32>,
    /// The `bl`s taken as calls that never return.
    pub no_returns: BTreeSet<u32>,
    /// Some path ran into data, an undefined instruction or bytes that
    /// don't decode.
    pub stopped: bool,
    /// An undefined instruction some path reached, other than a trap
    /// ([`deliberate`]), the lowest. Code doesn't hold one; data decoded as
    /// code often does, so discovery drops a scanned root that reaches one.
    pub undefined: Option<u32>,
    /// Hit the step cap ([`STEP_CAP`] by default); the results aren't a
    /// fixpoint.
    pub capped: bool,
    /// Steps the fixpoint took.
    pub steps: u64,
    /// The instructions reached, as (address, size).
    pub insns: Vec<(u32, u32)>,
    /// What the function calls, in interworking form: `bl` targets taken
    /// as calls, and constant targets of indirect calls. A `bl` to a lone
    /// `bx rN` (a call-via-register stub) calls rN, not the stub.
    pub calls: BTreeSet<u32>,
    /// Resolved jumps the analysis didn't follow, because the target is in
    /// the other instruction set or outside the function: (jump, target),
    /// the target in interworking form.
    pub exits: BTreeSet<(u32, u32)>,
    /// The literal-pool data pc-relative loads read, as `start..end`.
    /// (The tables of resolved jumps are in [`jumps`](FuncResult::jumps).)
    pub pools: Vec<(u32, u32)>,
    /// The function extents the result depends on, as one range: a new
    /// function start inside it can change the result. (New data can
    /// change it only where it overlaps [`insns`](FuncResult::insns) or
    /// [`reads`](FuncResult::reads).)
    pub span: Option<(u32, u32)>,
    /// Code read outside [`insns`](FuncResult::insns), as `start..end`,
    /// sorted: jump targets not followed, callees run inline or checked
    /// for a `bx rN` stub, and the instruction before each pc write.
    pub reads: Vec<(u32, u32)>,
    /// BIOS `CpuSet`/`CpuFastSet` copies out of ROM with constant
    /// arguments: (source, destination, bytes). agb's `crt0` copies its
    /// IWRAM and EWRAM sections this way.
    pub copies: BTreeSet<(u32, u32, u32)>,
    /// Constant code addresses in EWRAM or IWRAM ([`is_ram`]) outside the
    /// image, in interworking form: call and jump targets (a `bl` to a
    /// `bx rN` stub calls rN), and words stored to the BIOS's IRQ vector.
    /// Code copied to RAM runs there (decision 7).
    pub ram_entries: BTreeSet<u32>,
    /// The loads and stores reached, by instruction address: how their
    /// addresses are known ([`Access`]), joined over every visit.
    pub accesses: BTreeMap<u32, Access>,
}

/// How the addresses one memory instruction uses are known, over every
/// access it makes (an `ldm`'s every word) in every state that reaches
/// it. The lifter's report (`codegen::native`) counts them by region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Access {
    /// Some access is sp-relative: the entry's sp plus a constant.
    pub sp: bool,
    /// Some access isn't, and its address could be anything.
    pub top: bool,
    /// The lowest and highest address of the accesses that aren't
    /// sp-relative and are known.
    pub hull: Option<(u32, u32)>,
}

impl Access {
    /// One access: sp-relative, or at an address in `dom`.
    pub fn of(sp: bool, dom: &Dom) -> Access {
        if sp {
            return Access {
                sp: true,
                ..Access::default()
            };
        }
        let hull = match dom {
            Dom::Top => None,
            Dom::Const(k) => Some((*k, *k)),
            Dom::Set(v) => Some((v[0], v[v.len() - 1])),
            Dom::Si { lo, hi, .. } => Some((*lo, *hi)),
        };
        Access {
            sp: false,
            top: hull.is_none(),
            hull,
        }
    }

    pub fn join(self, o: Access) -> Access {
        Access {
            sp: self.sp || o.sp,
            top: self.top || o.top,
            hull: match (self.hull, o.hull) {
                (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
                (h, None) | (None, h) => h,
            },
        }
    }

    /// Only sp-relative accesses.
    pub fn stack_only(self) -> bool {
        self.sp && !self.top && self.hull.is_none()
    }
}

/// Whether `a` is in EWRAM or IWRAM.
pub fn is_ram(a: u32) -> bool {
    (0x0200_0000..0x0400_0000).contains(&a)
}

/// The literal-pool range a pc-relative load reads, if `insn` is one.
pub fn pool_read(insn: &Insn) -> Option<(u32, u32)> {
    let Instr::Load {
        size,
        addr:
            armv4t::decode::Addr {
                rn: DReg::Pc,
                offset: armv4t::decode::Offset::Imm(off),
                index: Index::Pre { .. },
            },
        ..
    } = insn.instr
    else {
        return None;
    };
    let pc = if insn.thumb {
        insn.addr.wrapping_add(4) & !3
    } else {
        insn.addr.wrapping_add(8)
    };
    let n = match size {
        armv4t::decode::MemSize::Word => 4,
        armv4t::decode::MemSize::Half | armv4t::decode::MemSize::SignedHalf => 2,
        _ => 1,
    };
    let a = pc.wrapping_add(off as u32);
    Some((a, a.wrapping_add(n)))
}

/// Analyse the function at `entry`. `far(t, thumb)` says what a Thumb `bl`
/// to `t` is.
pub fn analyze(
    prog: &Program,
    entry: u32,
    thumb: bool,
    far: &mut dyn FnMut(u32, bool) -> Bl,
) -> FuncResult {
    analyze_capped(prog, entry, thumb, far, STEP_CAP)
}

/// [`analyze`] with `cap` in place of [`STEP_CAP`].
pub fn analyze_capped(
    prog: &Program,
    entry: u32,
    thumb: bool,
    far: &mut dyn FnMut(u32, bool) -> Bl,
    cap: u64,
) -> FuncResult {
    // Functions don't nest: a Thumb `bl` from a function with a prologue
    // into the span of its own code (and pools) is a jump, unless the
    // target looks like a function of its own (a prologue, a `bx rN`
    // stub, a short leaf returning with `bx lr`, a target the probe says
    // returns balanced; a case block ending in the epilogue pops a frame
    // it didn't push). Rerun the fixpoint with those taken as jumps
    // until no new one turns up.
    let mut inner = BTreeSet::new();
    let mut tries = 0;
    let prologue = thumb && is_push_lr(prog, entry);
    let (mut a, states) = loop {
        let (mut a, states) = fixpoint(prog, entry, thumb, far, cap, inner.clone());
        let mut more = Vec::new();
        if prologue && !a.res.capped && tries < INNER_TRIES {
            // The span of the code reached and the pools it reads.
            let (mut lo, mut hi) = (u32::MAX, 0);
            let mut bls = Vec::new();
            for &at in states.keys() {
                let Some(i) = a.decode(at) else { continue };
                let end = pool_read(&i).map_or(i.next(), |(_, e)| e.max(i.next()));
                (lo, hi) = (lo.min(at), hi.max(end));
                if let Some(t) = i.bl {
                    bls.push(t);
                }
            }
            for t in bls {
                if t != entry
                    && lo < t
                    && t < hi
                    && !inner.contains(&t)
                    && !is_stub(prog, t)
                    && !is_push_lr(prog, t)
                    && !is_leaf(&mut a, t)
                    && (a.far)(t, true) != Bl::Balanced
                {
                    more.push(t);
                }
            }
            more.sort_unstable();
            more.dedup();
        }
        if more.is_empty() {
            break (a, states);
        }
        inner.extend(more);
        tries += 1;
    };
    // The results come from the final states only: an earlier visit may
    // have resolved a jump with a narrower state than the fixpoint allows.
    a.record = true;
    for (&at, st) in &states {
        if let Some(insn) = a.decode(at) {
            a.res.reached += 1;
            a.res.insns.push((at, insn.size));
            if let Some(d) = pool_read(&insn) {
                a.res.pools.push(d);
            }
            a.transfer(&insn, st.clone());
        }
    }
    a.res.span = a.span;
    a.res.reads.sort_unstable();
    a.res.reads.dedup();
    a.res
}

/// The state on entry to every instruction reached from `entry`, at the
/// fixpoint.
pub fn states(
    prog: &Program,
    entry: u32,
    thumb: bool,
    far: &mut dyn FnMut(u32, bool) -> Bl,
) -> BTreeMap<u32, State> {
    fixpoint(prog, entry, thumb, far, STEP_CAP, BTreeSet::new()).1
}

fn fixpoint<'p, 'f>(
    prog: &'p Program,
    entry: u32,
    thumb: bool,
    far: &'f mut dyn FnMut(u32, bool) -> Bl,
    cap: u64,
    inner: BTreeSet<u32>,
) -> (Analysis<'p, 'f>, BTreeMap<u32, State>) {
    let mut a = Analysis {
        prog,
        thumb,
        sp0: State::sp0(entry),
        lr0: State::lr0(entry),
        decoded: HashMap::new(),
        kept: HashMap::new(),
        far,
        inner,
        record: false,
        res: FuncResult::default(),
        span: None,
    };
    let mut states: HashMap<u32, State> = HashMap::new();
    let mut visits: HashMap<u32, u32> = HashMap::new();
    let mut work: BTreeSet<u32> = BTreeSet::new();
    states.insert(entry, State::entry(entry));
    work.insert(entry);
    let mut steps = 0u64;
    while let Some(at) = work.pop_first() {
        steps += 1;
        a.res.steps = steps;
        if steps > cap {
            a.res.capped = true;
            break;
        }
        let Some(insn) = a.decode(at) else {
            a.res.stopped = true;
            continue;
        };
        for (to, st) in a.transfer(&insn, states[&at].clone()) {
            if !prog.is_code(to, thumb) {
                a.res.stopped = true;
                continue;
            }
            match states.get(&to) {
                None => {
                    states.insert(to, st);
                    work.insert(to);
                }
                Some(old) => {
                    let n = visits.entry(to).or_default();
                    *n += 1;
                    let widen = *n > WIDEN_AFTER && to <= at;
                    let j = old.join(&st, |r| Sym::new(JOIN, to, 0, r), widen);
                    if &j != old {
                        states.insert(to, j);
                        work.insert(to);
                    }
                }
            }
        }
    }
    (a, states.into_iter().collect())
}

struct Analysis<'p, 'f> {
    prog: &'p Program,
    thumb: bool,
    sp0: Sym,
    /// lr's value on entry: a jump to it unchanged is a return.
    lr0: Sym,
    decoded: HashMap<u32, Option<Insn>>,
    /// [`Analysis::kept`]'s answers, with the bytes each read.
    kept: HashMap<u32, (u16, Vec<(u32, u32)>)>,
    far: &'f mut dyn FnMut(u32, bool) -> Bl,
    /// `bl` targets the function reaches by itself: jumps.
    inner: BTreeSet<u32>,
    /// The final pass: record results.
    record: bool,
    res: FuncResult,
    /// The function extents consulted, as one range.
    span: Option<(u32, u32)>,
}

impl Analysis<'_, '_> {
    /// The function holding `at`, noting its extent in the span.
    fn func_at(&mut self, at: u32) -> Option<crate::program::Func> {
        let f = self.prog.func_at(at).copied();
        if let Some(f) = f {
            let (lo, hi) = self.span.unwrap_or((f.start, f.end));
            self.span = Some((lo.min(f.start), hi.max(f.end)));
        }
        f
    }

    /// Decode an instruction outside the fixpoint's own, noting it in the
    /// result's reads.
    fn peek(&mut self, at: u32, thumb: bool) -> Option<Insn> {
        if self.record {
            self.res.reads.push((at, at.wrapping_add(4)));
        }
        self.prog.decode(at, thumb)
    }

    fn decode(&mut self, at: u32) -> Option<Insn> {
        let (prog, thumb) = (self.prog, self.thumb);
        *self
            .decoded
            .entry(at)
            .or_insert_with(|| prog.decode(at, thumb))
    }

    /// The successors of `insn` run on `st`.
    fn transfer(&mut self, insn: &Insn, st: State) -> Vec<(u32, State)> {
        if let Some(target) = insn.bl {
            return self.bl(insn, target, st);
        }
        let step = exec::step(self.prog, self.sp0, 0, insn, st);
        if self.record {
            for (sp, dom) in &step.accesses {
                let a = Access::of(*sp, dom);
                let e = self.res.accesses.entry(insn.addr).or_insert(a);
                *e = e.join(a);
            }
        }
        if self.record
            && let Some(h) = step.irq_vector
        {
            // The BIOS enters it with `ldr pc`: in ARM state.
            self.ram_entry(h & !3);
        }
        let mut out = Vec::new();
        if let Some(f) = step.fall {
            out.push((insn.next(), f));
        }
        for e in step.exits {
            self.exit(insn, e, &mut out);
        }
        out
    }

    fn exit(&mut self, insn: &Insn, e: Exit, out: &mut Vec<(u32, State)>) {
        let Exit {
            kind,
            target,
            mut state,
        } = e;
        match kind {
            ExitKind::Undefined => {
                self.res.stopped = true;
                if !deliberate(self.prog, insn) {
                    let u = self.res.undefined.get_or_insert(insn.addr);
                    *u = (*u).min(insn.addr);
                }
            }
            // BIOS calls return, clobbering r0 to r3.
            ExitKind::Swi => {
                if self.record
                    && let Some(c) = bios_copy(self.prog, insn, &state)
                {
                    self.res.copies.insert(c);
                }
                state.clobber_call(insn.addr, 0);
                out.push((insn.next(), state));
            }
            _ => {
                if let Instr::Branch { target, .. } = insn.instr {
                    out.push((target, state));
                    return;
                }
                if self.record {
                    self.res.reads.push((insn.addr.wrapping_sub(4), insn.addr));
                }
                let ctl = match classify(self.prog, insn) {
                    // A jump through a copy of the entry lr (m4a's `mov
                    // r12, lr; ... bx r12`) is a return.
                    Ctl::Jump
                        if target
                            .as_ref()
                            .is_some_and(|v| v.sym == self.lr0 && v.off == 0) =>
                    {
                        Ctl::Return
                    }
                    c => c,
                };
                match ctl {
                    Ctl::Return => {
                        if self.record {
                            self.res.ret_sp.push(state.sp_offset(self.sp0));
                        }
                    }
                    Ctl::Call => {
                        if self.record
                            && let Some(k) = target.as_ref().and_then(|v| v.dom.konst())
                        {
                            let t = match kind {
                                ExitKind::Exchange => k,
                                _ if self.thumb => k | 1,
                                _ => k & !3,
                            };
                            if self.prog.in_rom(t & !1) {
                                self.res.calls.insert(t);
                            } else {
                                self.ram_entry(t);
                            }
                        }
                        state.clobber_call(insn.addr, 0);
                        out.push((insn.next(), state));
                    }
                    Ctl::Jump => {
                        let target = target.expect("jumps have targets");
                        self.jump(insn.addr, &target, kind, state, out);
                    }
                }
            }
        }
    }

    /// The targets of a jump to `v`, if it is a finite set of ROM addresses,
    /// with the instruction set of each.
    fn targets(&self, v: &AbsVal, kind: ExitKind) -> Option<Vec<(u32, bool)>> {
        if kind == ExitKind::ExceptionReturn || v.dom.count() > MAX_SET as u64 {
            return None;
        }
        let mut out = Vec::new();
        for t in v.dom.elems()? {
            let (a, thumb) = match kind {
                ExitKind::Exchange => (t & !1, t & 1 != 0),
                _ if self.thumb => (t & !1, true),
                _ => (t & !3, false),
            };
            if !self.prog.in_rom(a) {
                return None;
            }
            out.push((a, thumb));
        }
        Some(out)
    }

    /// Check a jump's targets: they must decode, and a table's (more than one
    /// target, or read from more than one entry) must stay inside the
    /// function holding the jump.
    fn check(&mut self, at: u32, targets: &[(u32, bool)], table: Option<Src>) -> JumpRes {
        let j = Jump {
            targets: targets.iter().map(|&(a, t)| a | t as u32).collect(),
            table,
        };
        let decodes = targets.iter().all(|&(a, t)| {
            self.peek(a, t)
                .is_some_and(|i| !matches!(i.instr, Instr::Undefined { .. }))
        });
        let is_table = targets.len() > 1 || table.is_some_and(|s| s.entries() > 1);
        let inside = |f: Option<crate::program::Func>| {
            f.is_some_and(|f| targets.iter().all(|&(a, _)| (f.start..f.end).contains(&a)))
        };
        if !decodes || (is_table && !inside(self.func_at(at))) {
            JumpRes::Rejected(j)
        } else {
            JumpRes::Resolved(j)
        }
    }

    /// An indirect jump at `at` to `v`: resolve it, check it, and follow the
    /// targets that stay in this function and instruction set.
    fn jump(
        &mut self,
        at: u32,
        v: &AbsVal,
        kind: ExitKind,
        st: State,
        out: &mut Vec<(u32, State)>,
    ) {
        let Some(ts) = self.targets(v, kind) else {
            if self.record {
                self.res.jumps.insert(at, JumpRes::Unresolved);
                if let Some(k) = v.dom.konst() {
                    self.ram_entry(match kind {
                        ExitKind::Exchange => k,
                        _ if self.thumb => k | 1,
                        _ => k & !3,
                    });
                }
            }
            return;
        };
        let res = self.check(at, &ts, v.src);
        if let JumpRes::Resolved(_) = &res {
            let func = self.func_at(at);
            for &(t, thumb) in &ts {
                let inside = func.is_none_or(|f| (f.start..f.end).contains(&t));
                if thumb == self.thumb && inside {
                    out.push((t, st.clone()));
                } else if self.record {
                    self.res.exits.insert((at, t | thumb as u32));
                }
            }
        }
        if self.record {
            self.res.jumps.insert(at, res);
        }
    }

    /// A `bl` to `target`: a far jump, a callee run inline, or a call.
    fn bl(&mut self, insn: &Insn, target: u32, st: State) -> Vec<(u32, State)> {
        let next = insn.next();
        let ret = next | self.thumb as u32;
        let (taken, skipped) =
            exec::split(self.prog, self.sp0, 0, insn.addr, insn.instr.cond(), st);
        let mut out: Vec<(u32, State)> = skipped.map(|s| (next, s)).into_iter().collect();
        let Some(mut st) = taken else { return out };
        st.set_reg(Reg::LR, AbsVal::konst(ret));
        // ARM `b` reaches as far as `bl`, so only a Thumb `bl` can be a
        // far jump.
        let what = if !self.thumb {
            Bl::Call
        } else if self.inner.contains(&target) {
            Bl::Jump
        } else {
            (self.far)(target, self.thumb)
        };
        if what == Bl::Jump {
            if self.record {
                self.res.far_jumps.insert(insn.addr);
            }
            out.push((target, st));
            return out;
        }
        if self.record {
            self.record_call(target, &st);
            // A call to a `swi; bx lr` wrapper (libgba's and pret's
            // `CpuSet`) is the BIOS call made with the caller's arguments.
            if let Some(swi) = self.swi_wrapper(target)
                && let Some(c) = bios_copy(self.prog, &swi, &st)
            {
                self.res.copies.insert(c);
            }
        }
        if what == Bl::NoReturn {
            if self.record {
                self.res.no_returns.insert(insn.addr);
            }
            return out;
        }
        if let Some(e) = self
            .leaf(target)
            .and_then(|body| self.run_inline(&body, insn.addr, st.clone()))
        {
            let ts = e.target.as_ref().and_then(|v| self.targets(v, e.kind));
            if ts.as_deref() == Some(&[(next, self.thumb)]) {
                out.push((next, e.state));
                return out;
            }
            // The callee returns somewhere computed (`__gnu_thumb1_case_*`):
            // an indirect jump at the `bl`.
            if ts.is_some() {
                let mut s = e.state;
                s.retire(insn.addr, 0);
                s.set_reg(Reg::LR, AbsVal::top(Sym::new(CLOBBER, insn.addr, 0, 14)));
                self.jump(insn.addr, e.target.as_ref().unwrap(), e.kind, s, &mut out);
                return out;
            }
        }
        let kept = self.kept(target);
        // A kept value may be one this call made last time round a loop:
        // renamed first, as clobber_call does.
        st.retire(insn.addr, 0);
        let old = st.clone();
        st.clobber_call(insn.addr, 0);
        for r in 0..15 {
            if kept & 1 << r != 0 {
                st.regs[r] = old.regs[r].clone();
            }
        }
        out.push((next, st));
        out
    }

    /// The caller-saved registers (r0-r3, r12) a callee at `t` never
    /// writes, as a mask: what a `bl` to it keeps (m4a's `ply_*` hold the
    /// return address in r12 across `bl ld_r3_tp_adr_i`). Read from the
    /// callee's own code, up to [`KEPT_MAX`] instructions following
    /// branches to its returns; none where it calls, jumps through a
    /// register, traps or runs longer.
    fn kept(&mut self, t: u32) -> u16 {
        if !self.kept.contains_key(&t) {
            let mut reads = Vec::new();
            let mask = self
                .callee_writes(t, &mut reads)
                .map_or(0, |w| CALLER_SAVED & !w);
            self.kept.insert(t, (mask, reads));
        }
        let (mask, reads) = &self.kept[&t];
        if self.record {
            self.res.reads.extend_from_slice(reads);
        }
        *mask
    }

    /// The registers the code at `t` may write before it returns, if it is
    /// a short callee with no calls (see [`Analysis::kept`]).
    fn callee_writes(&self, t: u32, reads: &mut Vec<(u32, u32)>) -> Option<u16> {
        let sp0 = State::sp0(t);
        let entry = State::entry(t);
        let mut seen = BTreeSet::new();
        let mut work = vec![t];
        let mut written = 0u16;
        while let Some(a) = work.pop() {
            if !seen.insert(a) {
                continue;
            }
            if seen.len() > KEPT_MAX {
                return None;
            }
            reads.push((a, a.wrapping_add(4)));
            let i = self.prog.decode(a, self.thumb)?;
            if i.bl.is_some()
                || matches!(
                    i.instr,
                    Instr::Undefined { .. } | Instr::Swi { .. } | Instr::ThumbBlLo { .. }
                )
            {
                return None;
            }
            let s = exec::step(self.prog, sp0, 0, &i, entry.clone());
            let states = s.fall.iter().chain(s.exits.iter().map(|e| &e.state));
            for st in states {
                for r in 0..15 {
                    if st.regs[r] != entry.regs[r] {
                        written |= 1 << r;
                    }
                }
            }
            if s.fall.is_some() {
                work.push(i.next());
            }
            for e in &s.exits {
                match i.instr {
                    Instr::Branch { target, .. } => work.push(target),
                    _ if e.kind == ExitKind::ExceptionReturn => return None,
                    _ => {
                        // classify reads the instruction before.
                        reads.push((a.wrapping_sub(4), a));
                        if classify(self.prog, &i) != Ctl::Return {
                            return None;
                        }
                    }
                }
            }
        }
        Some(written)
    }

    /// Record what a `bl` to `target` calls: the target, or, if it is a
    /// lone `bx rN` (GCC's call-via-register stub; agbcc's `_call_via_rN`),
    /// rN's value if it is known (decision 5a).
    fn record_call(&mut self, target: u32, st: &State) {
        let thumb = self.thumb;
        match self.peek(target, thumb).map(|i| i.instr) {
            Some(Instr::Bx {
                rm,
                cond: armv4t::decode::Cond::Al,
            }) if rm != DReg::Lr && rm != DReg::Pc => {
                if let Some(k) = st.reg(Reg(rm as u8)).dom.konst() {
                    if self.prog.in_rom(k & !1) {
                        self.res.calls.insert(k);
                    } else {
                        self.ram_entry(k);
                    }
                }
            }
            _ => {
                self.res.calls.insert(target | thumb as u32);
            }
        }
    }

    /// The `swi` of a callee at `t` that is only `swi N; bx lr`. (Not
    /// [`Analysis::peek`]: the callee's bytes aren't the caller's reads.)
    fn swi_wrapper(&self, t: u32) -> Option<Insn> {
        let swi = self.prog.decode(t, self.thumb)?;
        let ret = self.prog.decode(swi.next(), self.thumb)?;
        (matches!(swi.instr, Instr::Swi { .. })
            && matches!(
                ret.instr,
                Instr::Bx {
                    rm: DReg::Lr,
                    cond: armv4t::decode::Cond::Al
                }
            ))
        .then_some(swi)
    }

    /// Note `k` (interworking form) as a [`FuncResult::ram_entries`] if it
    /// is RAM outside the image.
    fn ram_entry(&mut self, k: u32) {
        if is_ram(k & !1) && !self.prog.in_rom(k & !1) {
            self.res.ram_entries.insert(k);
        }
    }

    /// The body of a short leaf callee at `t`: straight-line, unconditional,
    /// up to and including its return.
    fn leaf(&mut self, t: u32) -> Option<Vec<Insn>> {
        let mut body = Vec::new();
        let mut a = t;
        for _ in 0..LEAF_MAX {
            let i = self.peek(a, self.thumb)?;
            body.push(i);
            if i.bl.is_some() || writes_pc(&i.instr) {
                // classify reads the instruction before.
                if self.record && a == t {
                    self.res.reads.push((t.wrapping_sub(4), t));
                }
                return (i.bl.is_none() && classify(self.prog, &i) == Ctl::Return).then_some(body);
            }
            if matches!(i.instr, Instr::Undefined { .. } | Instr::Swi { .. })
                || i.instr.cond() != armv4t::decode::Cond::Al
            {
                return None;
            }
            a = i.next();
        }
        None
    }

    /// Run a leaf body on `st`; the exit its return takes. Names are salted
    /// with the call site, so two calls of one helper don't share unknowns.
    fn run_inline(&self, body: &[Insn], site: u32, mut st: State) -> Option<Exit> {
        let (last, init) = body.split_last()?;
        for i in init {
            let s = exec::step(self.prog, self.sp0, site, i, st);
            if !s.exits.is_empty() {
                return None;
            }
            st = s.fall?;
        }
        let mut s = exec::step(self.prog, self.sp0, site, last, st);
        if s.fall.is_some() || s.exits.len() != 1 {
            return None;
        }
        s.exits.pop()
    }
}

/// Whether Thumb code at `t` starts with `push {…, lr}`: a prologue.
fn is_push_lr(prog: &Program, t: u32) -> bool {
    matches!(
        prog.decode(t, true).map(|i| i.instr),
        Some(Instr::Block { load: false, rn: DReg::Sp, writeback: true, regs, .. }) if regs & 1 << 14 != 0
    )
}

/// Whether Thumb code at `t` is a lone `bx rN` (rN not lr): a
/// call-via-register stub.
fn is_stub(prog: &Program, t: u32) -> bool {
    matches!(
        prog.decode(t, true).map(|i| i.instr),
        Some(Instr::Bx { rm, cond: armv4t::decode::Cond::Al }) if rm != DReg::Lr && rm != DReg::Pc
    )
}

/// The copy a BIOS `CpuSet` (0x0b) or `CpuFastSet` (0x0c) call at `insn`
/// makes on `st`, if r0 to r2 are constant, the source is in ROM and it
/// copies rather than fills: (source, destination, bytes).
fn bios_copy(prog: &Program, insn: &Insn, st: &State) -> Option<(u32, u32, u32)> {
    let Instr::Swi { comment, .. } = insn.instr else {
        return None;
    };
    let n = if insn.thumb { comment } else { comment >> 16 } & 0xff;
    let k = |r: u8| st.reg(Reg(r)).dom.konst();
    let (src, dst, ctl) = (k(0)?, k(1)?, k(2)?);
    let count = ctl & 0x1f_ffff;
    let bytes = match n {
        0x0b if ctl & 1 << 26 != 0 => count * 4,
        0x0b => count * 2,
        // CpuFastSet copies 8 words at a time.
        0x0c => count.div_ceil(8) * 32,
        _ => return None,
    };
    (ctl & 1 << 24 == 0 && bytes > 0 && prog.in_rom(src) && prog.in_rom(src + bytes - 1))
        .then_some((src, dst, bytes))
}

/// Whether `t` is a short straight-line leaf that leaves lr and sp alone
/// and returns with an unconditional `bx lr`. (LLVM's ARMv4T epilogue
/// `pop {…}; pop {r0}; mov lr, r0; bx lr` is a case block's, not a leaf's.)
fn is_leaf(a: &mut Analysis, t: u32) -> bool {
    a.leaf(t).is_some_and(|body| {
        matches!(
            body.last().map(|i| i.instr),
            Some(Instr::Bx {
                rm: DReg::Lr,
                cond: armv4t::decode::Cond::Al
            })
        ) && !body.iter().any(|i| touches_frame(&i.instr))
    })
}

/// Whether `i` may write lr or sp.
fn touches_frame(i: &Instr) -> bool {
    let frame = |r: DReg| matches!(r, DReg::Lr | DReg::Sp);
    let wb = |a: &armv4t::decode::Addr| {
        a.rn == DReg::Sp && matches!(a.index, Index::Post { .. } | Index::Pre { writeback: true })
    };
    match i {
        Instr::Block { .. } => true,
        Instr::Alu { rd, .. }
        | Instr::Mul { rd, .. }
        | Instr::Swap { rd, .. }
        | Instr::Mrs { rd, .. } => frame(*rd),
        Instr::MulLong { rd_lo, rd_hi, .. } => frame(*rd_lo) || frame(*rd_hi),
        Instr::Load { rd, addr, .. } => frame(*rd) || wb(addr),
        Instr::Store { addr, .. } => wb(addr),
        _ => false,
    }
}
