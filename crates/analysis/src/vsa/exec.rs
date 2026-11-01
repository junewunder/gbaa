//! `Sem<V = AbsVal>`: running an instruction's semantics on an [`Exec`]
//! computes its effect on an abstract [`State`].
//!
//! Values live in a per-instruction arena, so `Sem::V` is an index. A
//! condition code (`Sem::when`) splits the state: the guarded effects run
//! on the state narrowed by the condition, the skipped path on the state
//! narrowed by its negation, and the two join afterwards. An exit ends its
//! path and is returned with the state it left with.

use std::collections::BTreeMap;

use armv4t::decode::{Cond, Instr};
use armv4t::sem::prim::{self, MulKind};
use armv4t::sem::{self, Flag, PsrMask, Reg, Sem, ShiftType, Width};

use super::domain::{
    AbsVal, Bit, CLOBBER, DOp, Derived, Dom, ENTRY, MAX_ENUM, OP, Ranges, STALE, Src, Sym, WHEN,
};
use crate::program::Program;

/// The abstract CPU state at one point: r0 to r14, the flags, and the stack
/// slots, keyed by their offset from sp at the function's entry. Memory
/// other than the stack and the ROM isn't tracked.
#[derive(Clone, PartialEq, Debug)]
pub struct State {
    pub regs: [AbsVal; 15],
    pub flags: [Bit; 4],
    pub stack: BTreeMap<i32, AbsVal>,
    /// A pointer into the frame may have escaped: a value made from sp went
    /// somewhere other than sp (another register, memory, an operation that
    /// loses track of it). From then on a call may write the frame.
    pub escaped: bool,
}

impl State {
    /// The state on entry to a function at `entry`: everything unknown,
    /// each register its own unknown. sp is the unknown [`State::sp0`].
    pub fn entry(entry: u32) -> State {
        State {
            regs: std::array::from_fn(|r| AbsVal::top(Sym::new(ENTRY, entry, 0, r as u32))),
            flags: std::array::from_fn(|_| Bit::Top),
            stack: BTreeMap::new(),
            escaped: false,
        }
    }

    /// The name of sp's value on entry to the function at `entry`.
    pub fn sp0(entry: u32) -> Sym {
        Sym::new(ENTRY, entry, 0, 13)
    }

    pub fn reg(&self, r: Reg) -> &AbsVal {
        &self.regs[r.index()]
    }

    pub fn set_reg(&mut self, r: Reg, v: AbsVal) {
        self.regs[r.index()] = v;
    }

    /// sp's offset from its entry value, if known.
    pub fn sp_offset(&self, sp0: Sym) -> Option<i32> {
        let sp = self.reg(Reg::SP);
        (sp.sym == sp0).then_some(sp.off as i32)
    }

    /// What a call does to the caller's state (the AAPCS): r0 to r3, r12
    /// and lr are clobbered, and so are the flags. If a pointer into the
    /// frame has escaped, so is the stack: the callee may write it
    /// (`int x; get(&x); switch (x)`). `at` names the new unknowns.
    pub fn clobber_call(&mut self, at: u32, salt: u32) {
        self.retire(at, salt);
        for r in [0, 1, 2, 3, 12, 14] {
            self.regs[r] = AbsVal::top(Sym::new(CLOBBER, at, salt, r as u32));
        }
        self.flags = std::array::from_fn(|_| Bit::Top);
        if self.escaped {
            self.stack.clear();
        }
    }

    /// The locations that hold values: registers 0 to 14 (by number) and
    /// stack slots (see [`slot_loc`]).
    fn locations(&self) -> impl Iterator<Item = (u32, &AbsVal)> {
        let regs = self.regs.iter().enumerate().map(|(r, v)| (r as u32, v));
        let slots = self.stack.iter().map(|(k, v)| (slot_loc(*k), v));
        regs.chain(slots)
    }

    fn locations_mut(&mut self) -> impl Iterator<Item = &mut AbsVal> {
        self.regs.iter_mut().chain(self.stack.values_mut())
    }

    /// The join at a merge point. `name(n)` names the unknowns it makes, n
    /// being the location they are first held in (see `locations`).
    ///
    /// Locations whose values pair the same unknowns at the same distance
    /// on both sides (`a = A + x` and `b = B + y` with the same `A`, `B` and
    /// `x - y`) still share one unknown after the join, so a relation such
    /// as `r0 = r5 - 1` survives. A flag predicate over `A` on one side and
    /// `B` on the other survives too, if it says the same thing about the
    /// shared unknown. With `widen`, a set that changed goes straight to
    /// unknown.
    pub fn join(&self, o: &State, name: impl Fn(u32) -> Sym, widen: bool) -> State {
        // The locations both sides hold, paired.
        let mut pairs: Vec<(u32, &AbsVal, &AbsVal)> = self
            .regs
            .iter()
            .zip(&o.regs)
            .enumerate()
            .map(|(r, (a, b))| (r as u32, a, b))
            .collect();
        for (k, a) in &self.stack {
            if let Some(b) = o.stack.get(k) {
                pairs.push((slot_loc(*k), a, b));
            }
        }
        // Group them by (A, B, x - y). A group keeps its unknown if both
        // sides already agree on it, else it gets a new one, named by its
        // first location.
        type Key = (Sym, Sym, u32);
        // key, first location, and its offsets on each side
        let mut groups: Vec<(Key, u32, u32, u32)> = Vec::new();
        let mut group_of = Vec::with_capacity(pairs.len());
        for &(loc, a, b) in &pairs {
            let key = (a.sym, b.sym, a.off.wrapping_sub(b.off));
            let g = match groups.iter().position(|g| g.0 == key) {
                Some(g) => g,
                None => {
                    groups.push((key, loc, a.off, b.off));
                    groups.len() - 1
                }
            };
            group_of.push(g);
        }
        let mut names: Vec<Option<Sym>> = groups
            .iter()
            .map(|((x, y, d), first, _, _)| (x != y || *d != 0).then(|| name(*first)))
            .collect();
        // A kept unknown that is also a new name here is a stale copy of an
        // older value: rename its group too.
        loop {
            let made: Vec<Sym> = names.iter().flatten().copied().collect();
            let mut changed = false;
            for (g, n) in names.iter_mut().enumerate() {
                if n.is_none() && made.contains(&groups[g].0.0) {
                    *n = Some(name(groups[g].1));
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // Each group's unknown, and its offset in the joined state: a kept
        // unknown keeps the offsets; a new one is the first location's
        // value, so offsets are relative to it.
        let made: Vec<Sym> = names.iter().flatten().copied().collect();
        // A derivation survives if its bases are paired the same way.
        let derived = |x: Option<Derived>, y: Option<Derived>| {
            let (x, y) = (x?, y?);
            if x.op != y.op {
                return None;
            }
            match groups.iter().position(|g| (g.0.0, g.0.1) == (x.sym, y.sym)) {
                Some(g) => {
                    let (_, _, xa, xb) = groups[g];
                    let d = x.off.wrapping_sub(xa);
                    if d != y.off.wrapping_sub(xb) {
                        return None;
                    }
                    Some(match names[g] {
                        None => x,
                        Some(n) => Derived {
                            sym: n,
                            off: d,
                            op: x.op,
                        },
                    })
                }
                None => (x == y && !made.contains(&x.sym)).then_some(x),
            }
        };
        let joined = |i: usize| {
            let (_, a, b) = pairs[i];
            let g = group_of[i];
            let (sym, off) = match names[g] {
                None => (a.sym, a.off),
                Some(n) => (n, a.off.wrapping_sub(groups[g].2)),
            };
            let d = derived(a.derived, b.derived);
            if a == b && names[g].is_none() && d == a.derived {
                a.clone()
            } else {
                a.join_as(b, sym, off, d, widen)
            }
        };
        let regs: [AbsVal; 15] = std::array::from_fn(joined);
        let mut stack = BTreeMap::new();
        let mut i = 15;
        for k in self.stack.keys() {
            if o.stack.contains_key(k) {
                stack.insert(*k, joined(i));
                i += 1;
            }
        }
        let flag = |f: usize| -> Bit {
            let (p, q) = (&self.flags[f], &o.flags[f]);
            let (
                Bit::Pred {
                    sym: s1,
                    set: r1,
                    now: n1,
                },
                Bit::Pred {
                    sym: s2,
                    set: r2,
                    now: n2,
                },
            ) = (p, q)
            else {
                return p.join(q);
            };
            let now = if n1 == n2 { *n1 } else { None };
            // In terms of a location both sides hold the unknowns in.
            for (i, &(_, a, b)) in pairs.iter().enumerate() {
                if a.sym == *s1 && b.sym == *s2 {
                    let (va, vb) = (r1.shift(a.off), r2.shift(b.off));
                    if va != vb {
                        return Bit::Top;
                    }
                    let v = &joined(i);
                    return Bit::pred(v.sym, va.shift(v.off.wrapping_neg()), now);
                }
            }
            if p == q && !made.contains(s1) {
                p.clone()
            } else {
                Bit::Top
            }
        };
        State {
            regs,
            flags: std::array::from_fn(flag),
            stack,
            escaped: self.escaped || o.escaped,
        }
    }

    /// Every name the state refers to: held values, the bases of derived
    /// values, and the flags' predicates. Sorted, without duplicates.
    fn names(&self) -> Vec<Sym> {
        let mut v: Vec<Sym> = Vec::new();
        for (_, x) in self.locations() {
            v.push(x.sym);
            v.extend(x.derived.map(|d| d.sym));
        }
        for f in &self.flags {
            if let Bit::Pred { sym, .. } = f {
                v.push(*sym);
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Rename every value the instruction at `at` (run with `salt`) makes,
    /// before running it again: a copy of last time's result must not look
    /// equal to this time's. Each gets a [`STALE`] name not used yet.
    pub fn retire(&mut self, at: u32, salt: u32) {
        let used = self.names();
        let old: Vec<Sym> = used
            .iter()
            .copied()
            .filter(|s| s.made_by(at, salt))
            .collect();
        if old.is_empty() {
            return;
        }
        let mut map = Vec::new();
        let mut k = 0;
        for s in old {
            let new = loop {
                let n = Sym::new(STALE, at, salt, k);
                k += 1;
                if !used.contains(&n) {
                    break n;
                }
            };
            map.push((s, new));
        }
        let rename = |s: Sym| map.iter().find(|m| m.0 == s).map_or(s, |m| m.1);
        for v in self.locations_mut() {
            v.sym = rename(v.sym);
            if let Some(d) = &mut v.derived {
                d.sym = rename(d.sym);
            }
        }
        for f in &mut self.flags {
            if let Bit::Pred { sym, .. } = f {
                *sym = rename(*sym);
            }
        }
    }

    /// The state on the path where `b` is `taken`, or `None` if no state
    /// allows it.
    pub fn assume(&self, b: &Bit, taken: bool) -> Option<State> {
        match b {
            Bit::Const(x) | Bit::Pred { now: Some(x), .. } => (*x == taken).then(|| self.clone()),
            Bit::Top => Some(self.clone()),
            Bit::Pred { sym, set, .. } => {
                let set = if taken { set.clone() } else { set.complement() };
                let mut out = self.clone();
                for v in out.locations_mut() {
                    *v = v.assume(*sym, &set)?;
                }
                if out
                    .locations()
                    .any(|(_, v)| v.derived.is_some_and(|d| d.sym == *sym))
                {
                    // The unknown's own set: from a location holding it, or
                    // the predicate's.
                    let base = match out.locations().find(|(_, v)| v.sym == *sym) {
                        Some((_, v)) => v.dom.add_const(v.off.wrapping_neg()),
                        None => Dom::Top.meet(&set)?,
                    };
                    for v in out.locations_mut() {
                        *v = v.assume_derived(*sym, &base)?;
                    }
                }
                Some(out)
            }
        }
    }
}

/// The location number of the stack slot at offset `k` (see
/// `State::locations`): 0x800 + k / 4. Only slots within 4 KB of the entry
/// sp are tracked, so the numbers stay in 0x400..0xc00: distinct, in 12
/// bits, and clear of the register numbers.
fn slot_loc(k: i32) -> u32 {
    (0x800 + (k >> 2)) as u32
}

/// Whether the stack slot at offset `k` is tracked (see [`slot_loc`]).
fn slot_tracked(k: i32) -> bool {
    (-0x1000..0x1000).contains(&k)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExitKind {
    Jump,
    Exchange,
    ExceptionReturn,
    Swi,
    Undefined,
}

/// An exit taken by an instruction: its kind, its target (for the jumps),
/// and the state it leaves with.
#[derive(Clone, Debug)]
pub struct Exit {
    pub kind: ExitKind,
    pub target: Option<AbsVal>,
    pub state: State,
}

/// The effect of one instruction: the state falling through to the next
/// instruction, if any path does, and the exits taken.
#[derive(Debug)]
pub struct Step {
    pub fall: Option<State>,
    pub exits: Vec<Exit>,
    /// A constant word stored to the BIOS's IRQ vector ([`IRQ_VECTOR`]):
    /// the IRQ handler's address.
    pub irq_vector: Option<u32>,
    /// The address of each load and store, in order: whether it is
    /// sp-relative (`sp0 + k`), and its set.
    pub accesses: Vec<(bool, Dom)>,
}

/// Where the BIOS's IRQ stub reads the handler's address from (and its
/// mirror at the top of the IWRAM window).
pub const IRQ_VECTOR: [u32; 2] = [0x0300_7FFC, 0x03FF_FFFC];

#[derive(Clone, Copy)]
pub struct V(u32);
#[derive(Clone, Copy)]
pub struct B(u32);

/// The abstract interpreter for one instruction. See the module docs.
pub struct Exec<'p> {
    prog: &'p Program,
    /// sp's entry value, for the stack slots.
    sp0: Sym,
    /// Mixed into new names: the call site when a callee runs inline.
    salt: u32,
    at: u32,
    /// Operations so far in this instruction, to name their results.
    ops: u32,
    cur: Option<State>,
    vals: Vec<AbsVal>,
    bits: Vec<Bit>,
    exits: Vec<Exit>,
    irq_vector: Option<u32>,
    accesses: Vec<(bool, Dom)>,
}

/// Run one instruction on `st`.
pub fn step(
    prog: &Program,
    sp0: Sym,
    salt: u32,
    insn: &crate::program::Insn,
    mut st: State,
) -> Step {
    st.retire(insn.addr, salt);
    let mut ex = Exec::new(prog, sp0, salt, insn.addr, st);
    sem::arm::execute(&mut ex, insn.addr, insn.thumb, insn.instr);
    Step {
        fall: ex.cur,
        exits: ex.exits,
        irq_vector: ex.irq_vector,
        accesses: ex.accesses,
    }
}

/// Split `st` on condition `c`: the states where it holds and where it
/// doesn't (`None` where no state allows it).
pub fn split(
    prog: &Program,
    sp0: Sym,
    salt: u32,
    at: u32,
    c: Cond,
    st: State,
) -> (Option<State>, Option<State>) {
    let mut ex = Exec::new(prog, sp0, salt, at, st.clone());
    match sem::cond(&mut ex, c) {
        None => (Some(st), None),
        Some(b) => {
            let b = ex.bits[b.0 as usize].clone();
            (st.assume(&b, true), st.assume(&b, false))
        }
    }
}

/// Whether `i` writes pc (other than by `swi` or an undefined trap).
pub fn writes_pc(i: &Instr) -> bool {
    use armv4t::decode::Reg as R;
    match *i {
        Instr::Alu { op, rd, .. } => rd == R::Pc && !op.is_compare(),
        Instr::Load { rd, .. } => rd == R::Pc,
        Instr::Block { load, regs, .. } => load && regs & 0x8000 != 0,
        Instr::Mul { rd, .. } => rd == R::Pc,
        Instr::MulLong { rd_lo, rd_hi, .. } => rd_lo == R::Pc || rd_hi == R::Pc,
        Instr::Swap { rd, .. } | Instr::Mrs { rd, .. } => rd == R::Pc,
        Instr::Branch { .. } | Instr::Bx { .. } | Instr::ThumbBlLo { .. } => true,
        _ => false,
    }
}

impl<'p> Exec<'p> {
    fn new(prog: &'p Program, sp0: Sym, salt: u32, at: u32, st: State) -> Exec<'p> {
        Exec {
            prog,
            sp0,
            salt,
            at,
            ops: 0,
            cur: Some(st),
            vals: Vec::new(),
            bits: Vec::new(),
            exits: Vec::new(),
            irq_vector: None,
            accesses: Vec::new(),
        }
    }

    fn fresh(&mut self) -> Sym {
        self.ops += 1;
        Sym::new(OP, self.at, self.salt, self.ops)
    }

    fn val(&self, v: V) -> &AbsVal {
        &self.vals[v.0 as usize]
    }

    fn bitv(&self, b: B) -> &Bit {
        &self.bits[b.0 as usize]
    }

    fn put(&mut self, v: AbsVal) -> V {
        self.vals.push(v);
        V(self.vals.len() as u32 - 1)
    }

    fn put_dom(&mut self, d: Dom) -> V {
        let sym = self.fresh();
        self.put(AbsVal::new(d, sym))
    }

    fn put_bit(&mut self, b: Bit) -> B {
        self.bits.push(b);
        B(self.bits.len() as u32 - 1)
    }

    fn state(&mut self) -> &mut State {
        self.cur.as_mut().expect("effect on an ended path")
    }

    /// `f` on every pair of values, if there are few enough.
    fn pairwise(a: &Dom, b: &Dom, f: impl Fn(u32, u32) -> u32) -> Option<Dom> {
        let (x, y) = (a.elems()?, b.elems()?);
        if x.len() as u64 * y.len() as u64 > MAX_ENUM {
            return None;
        }
        let out = x.iter().flat_map(|&p| y.iter().map(move |&q| (p, q)));
        Some(Dom::from_vals(out.map(|(p, q)| f(p, q)).collect()))
    }

    /// A binary op whose result has no relation to its operands. The ROM
    /// provenance survives an op with a constant operand.
    /// Note that an operation used a value made from sp in a way that loses
    /// track of it.
    fn escape(&mut self, vs: &[V]) {
        if vs.iter().any(|&v| self.val(v).sym == self.sp0)
            && let Some(st) = self.cur.as_mut()
        {
            st.escaped = true;
        }
    }

    fn binop(&mut self, a: V, b: V, d: Dom, keep_src: bool) -> V {
        self.escape(&[a, b]);
        let (x, y) = (self.val(a), self.val(b));
        let src = if y.dom.konst().is_some() {
            x.src
        } else if keep_src && x.dom.konst().is_some() {
            y.src
        } else {
            None
        };
        let v = self.put_dom(d);
        self.vals[v.0 as usize].src = src;
        v
    }

    fn mk_bit(vals: &[u32], f: impl Fn(u32) -> bool) -> Bit {
        let first = f(vals[0]);
        if vals.iter().all(|&x| f(x) == first) {
            Bit::Const(first)
        } else {
            Bit::Top
        }
    }

    /// `x + k + c` for constants `k` and `c`: the value, and the carry and
    /// overflow as predicates over `x`.
    fn add_const_flags(&mut self, x: V, k: u32, c: bool) -> (V, B, B) {
        let xv = self.val(x).clone();
        let sum = k as u64 + c as u64;
        let carry = match sum {
            0 => Ranges::empty(),
            s if s >= 1 << 32 => Ranges::full(),
            s => Ranges::new(((1u64 << 32) - s) as u32, u32::MAX),
        };
        let signed = k as i32 as i64 + c as i64;
        let overflow = match signed {
            0 => Ranges::empty(),
            s if s > 0 => Ranges::new((0x8000_0000 - s) as u32, 0x7fff_ffff),
            s => Ranges::new(0x8000_0000, (0x8000_0000 - s - 1) as u32),
        };
        let (cb, vb) = (xv.pred(&carry), xv.pred(&overflow));
        let v = self.put(xv.add_const(sum as u32));
        (v, self.put_bit(cb), self.put_bit(vb))
    }

    /// A shift by the constant `n` (1 to 255) as a [`DOp`], if it is one.
    fn shift_op(kind: ShiftType, n: u32) -> Option<DOp> {
        match kind {
            _ if n >= 32 => None,
            ShiftType::Lsl => Some(DOp::Lsl(n as u8)),
            ShiftType::Lsr => Some(DOp::Lsr(n as u8)),
            ShiftType::Asr => Some(DOp::Asr(n as u8)),
            ShiftType::Ror => None,
        }
    }

    fn shift_dom(kind: ShiftType, d: &Dom, n: u32) -> Dom {
        match (kind, Self::shift_op(kind, n)) {
            (_, Some(op)) => op.apply(d),
            (ShiftType::Lsl | ShiftType::Lsr, None) => Dom::Const(0),
            _ => Dom::Top,
        }
    }

    /// Record on `r` that it is `op` of `x`.
    fn derive(&mut self, r: V, x: &AbsVal, op: Option<DOp>) {
        self.vals[r.0 as usize].derived = op.map(|op| Derived {
            sym: x.sym,
            off: x.off,
            op,
        });
    }

    fn load_dom(&mut self, w: Width, addr: &AbsVal) -> AbsVal {
        let sym = self.fresh();
        let top = AbsVal::top(sym);
        if addr.sym == self.sp0 {
            return match self.state().stack.get(&(addr.off as i32)) {
                Some(v) if w == Width::Word => v.clone(),
                _ => top,
            };
        }
        let Some(addrs) = addr.dom.elems() else {
            return top;
        };
        let size = w.bytes();
        let mut out = Vec::with_capacity(addrs.len());
        let mut aligned: Vec<u32> = Vec::with_capacity(addrs.len());
        for a in addrs {
            let a = a & !(size - 1);
            match self.prog.read(a, size) {
                Some(v) => out.push(v),
                None => return top,
            }
            aligned.push(a);
        }
        let (first, last) = (
            *aligned.iter().min().unwrap(),
            *aligned.iter().max().unwrap(),
        );
        AbsVal {
            src: Some(Src { first, last, size }),
            ..AbsVal::new(Dom::from_vals(out), sym)
        }
    }

    fn exit(&mut self, kind: ExitKind, target: Option<V>) {
        let state = self.cur.take().expect("exit on an ended path");
        let target = target.map(|t| self.val(t).clone());
        self.exits.push(Exit {
            kind,
            target,
            state,
        });
    }
}

impl Sem for Exec<'_> {
    type V = V;
    type B = B;

    fn imm(&mut self, x: u32) -> V {
        self.put(AbsVal::konst(x))
    }

    fn bit(&mut self, b: bool) -> B {
        self.put_bit(Bit::Const(b))
    }

    fn add(&mut self, a: V, b: V) -> V {
        let (x, y) = (self.val(a), self.val(b));
        if let Some(k) = y.dom.konst() {
            let v = x.add_const(k);
            return self.put(v);
        }
        if let Some(k) = x.dom.konst() {
            let v = y.add_const(k);
            return self.put(v);
        }
        let d = Self::pairwise(&x.dom, &y.dom, u32::wrapping_add);
        self.binop(a, b, d.unwrap_or(Dom::Top), true)
    }

    fn sub(&mut self, a: V, b: V) -> V {
        if let Some(k) = self.val(b).dom.konst() {
            let v = self.val(a).add_const(k.wrapping_neg());
            return self.put(v);
        }
        let d = Self::pairwise(&self.val(a).dom, &self.val(b).dom, u32::wrapping_sub);
        self.binop(a, b, d.unwrap_or(Dom::Top), false)
    }

    fn and(&mut self, a: V, b: V) -> V {
        let (x, y) = (self.val(a).clone(), self.val(b).clone());
        let (v, m) = match (x.dom.konst(), y.dom.konst()) {
            (None, Some(m)) => (&x, m),
            (Some(m), None) => (&y, m),
            _ => {
                let d = Self::pairwise(&x.dom, &y.dom, |p, q| p & q);
                return self.binop(a, b, d.unwrap_or(Dom::Top), false);
            }
        };
        if m == u32::MAX {
            return self.put(v.clone());
        }
        let low_mask = (m + 1).is_power_of_two();
        let d = match &v.dom {
            // sp is word-aligned on entry, so its low bits are known.
            _ if v.sym == self.sp0 && m < 4 => return self.put_dom(Dom::Const(v.off & m)),
            &Dom::Si { s, lo, .. } if low_mask && s.is_multiple_of(m + 1) => Dom::Const(lo & m),
            // Masking off bits that are already clear changes nothing.
            d if low_mask && d.max() <= m => return self.put(v.clone()),
            d => match d.elems() {
                Some(e) => Dom::from_vals(e.iter().map(|x| x & m).collect()),
                None => Dom::si(1, 0, m),
            },
        };
        self.binop(a, b, d, false)
    }

    fn or(&mut self, a: V, b: V) -> V {
        let d = Self::pairwise(&self.val(a).dom, &self.val(b).dom, |p, q| p | q);
        self.binop(a, b, d.unwrap_or(Dom::Top), false)
    }

    fn xor(&mut self, a: V, b: V) -> V {
        let d = Self::pairwise(&self.val(a).dom, &self.val(b).dom, |p, q| p ^ q);
        self.binop(a, b, d.unwrap_or(Dom::Top), false)
    }

    fn not(&mut self, a: V) -> V {
        self.escape(&[a]);
        let d = match self.val(a).dom.elems() {
            Some(e) => Dom::from_vals(e.iter().map(|x| !x).collect()),
            None => Dom::Top,
        };
        self.put_dom(d)
    }

    fn mul(&mut self, a: V, b: V) -> V {
        let (x, y) = (self.val(a).clone(), self.val(b).clone());
        let by = match (x.dom.konst(), y.dom.konst()) {
            (None, Some(k)) => Some((k, &x)),
            (Some(k), None) => Some((k, &y)),
            _ => None,
        };
        let d = Self::pairwise(&x.dom, &y.dom, u32::wrapping_mul)
            .or_else(|| by.map(|(k, v)| DOp::Mul(k).apply(&v.dom)))
            .unwrap_or(Dom::Top);
        let r = self.binop(a, b, d, true);
        if let Some((k, v)) = by {
            self.derive(r, v, Some(DOp::Mul(k)));
        }
        r
    }

    fn mul_long(&mut self, signed: bool, a: V, b: V) -> (V, V) {
        self.escape(&[a, b]);
        let (x, y) = (&self.val(a).dom, &self.val(b).dom);
        let lo = Self::pairwise(x, y, |p, q| prim::mul_long(signed, p, q).0);
        let hi = Self::pairwise(x, y, |p, q| prim::mul_long(signed, p, q).1);
        let lo = self.put_dom(lo.unwrap_or(Dom::Top));
        let hi = self.put_dom(hi.unwrap_or(Dom::Top));
        (lo, hi)
    }

    fn add_with_carry(&mut self, a: V, b: V, c: B) -> (V, B, B) {
        let c = self.bitv(c).clone();
        let (x, y) = (self.val(a).clone(), self.val(b).clone());
        if let Some(c) = c.konst() {
            if let Some(k) = y.dom.konst() {
                return self.add_const_flags(a, k, c);
            }
            if let Some(k) = x.dom.konst() {
                return self.add_const_flags(b, k, c);
            }
        }
        self.escape(&[a, b]);
        let cs: &[bool] = match c.konst() {
            Some(c) => &[c][..],
            None => &[false, true],
        };
        let all = (|| {
            let (p, q) = (x.dom.elems()?, y.dom.elems()?);
            if (p.len() * q.len() * cs.len()) as u64 > MAX_ENUM {
                return None;
            }
            let mut v = Vec::new();
            for &p in &p {
                for &q in &q {
                    for &c in cs {
                        v.push(prim::add_with_carry(p, q, c));
                    }
                }
            }
            Some(v)
        })();
        match all {
            Some(v) => {
                let uniform = |f: fn(&(u32, bool, bool)) -> bool| {
                    let first = f(&v[0]);
                    if v.iter().all(|t| f(t) == first) {
                        Bit::Const(first)
                    } else {
                        Bit::Top
                    }
                };
                let (carry, over) = (uniform(|t| t.1), uniform(|t| t.2));
                let r = self.put_dom(Dom::from_vals(v.iter().map(|t| t.0).collect()));
                (r, self.put_bit(carry), self.put_bit(over))
            }
            None => {
                let r = self.put_dom(Dom::Top);
                (r, self.put_bit(Bit::Top), self.put_bit(Bit::Top))
            }
        }
    }

    fn shift(&mut self, kind: ShiftType, v: V, amount: V, carry_in: B) -> (V, B) {
        let x = self.val(v).clone();
        let n = self.val(amount).dom.konst();
        if n.is_some_and(|n| n & 0xff == 0) {
            return (v, carry_in);
        }
        let cin = self.bitv(carry_in).konst();
        // Element by element, if the carry in can't matter (amount ≠ 0) or
        // is known.
        let amounts = self.val(amount).dom.clone();
        let pairs = (|| {
            let (p, q) = (x.dom.elems()?, amounts.elems()?);
            if p.len() as u64 * q.len() as u64 > MAX_ENUM {
                return None;
            }
            let mut out = Vec::new();
            for &p in &p {
                for &q in &q {
                    if q & 0xff == 0 && cin.is_none() {
                        return None;
                    }
                    out.push(prim::shift(kind, p, q, cin.unwrap_or(false)));
                }
            }
            Some(out)
        })();
        if let Some(out) = pairs {
            let d = Dom::from_vals(out.iter().map(|o| o.0).collect());
            let first = out[0].1;
            let carry = if out.iter().all(|o| o.1 == first) {
                Bit::Const(first)
            } else {
                Bit::Top
            };
            let r = self.binop(v, amount, d, false);
            self.derive(r, &x, n.and_then(|n| Self::shift_op(kind, n & 0xff)));
            return (r, self.put_bit(carry));
        }
        let d = match n {
            Some(n) => Self::shift_dom(kind, &x.dom, n & 0xff),
            None => Dom::Top,
        };
        let r = self.binop(v, amount, d, false);
        self.derive(r, &x, n.and_then(|n| Self::shift_op(kind, n & 0xff)));
        (r, self.put_bit(Bit::Top))
    }

    fn mul_carry(&mut self, _: MulKind, _: V, _: V, _: V, _: V) -> B {
        self.put_bit(Bit::Top)
    }

    fn rrx(&mut self, v: V, carry_in: B) -> (V, B) {
        self.escape(&[v]);
        if let Some(c) = self.bitv(carry_in).konst()
            && let Some(e) = self.val(v).dom.elems()
        {
            let out: Vec<(u32, bool)> = e.iter().map(|&x| prim::rrx(x, c)).collect();
            let carry = Self::mk_bit(&e, |x| x & 1 != 0);
            let r = self.put_dom(Dom::from_vals(out.iter().map(|o| o.0).collect()));
            return (r, self.put_bit(carry));
        }
        let r = self.put_dom(Dom::Top);
        (r, self.put_bit(Bit::Top))
    }

    fn sext(&mut self, v: V, from_bits: u32) -> V {
        let x = self.val(v);
        if from_bits == 32 || (x.dom.max() as u64) < 1 << (from_bits - 1) {
            return v;
        }
        let d = match x.dom.elems() {
            Some(e) => Dom::from_vals(e.iter().map(|&y| prim::sext(y, from_bits)).collect()),
            None => Dom::Top,
        };
        let src = x.src;
        let r = self.put_dom(d);
        self.vals[r.0 as usize].src = src;
        r
    }

    fn select(&mut self, c: B, a: V, b: V) -> V {
        self.escape(&[a, b]);
        match self.bitv(c).konst() {
            Some(true) => a,
            Some(false) => b,
            _ => {
                let (x, y) = (self.val(a), self.val(b));
                let j = if (x.sym, x.off) == (y.sym, y.off) {
                    x.join_as(y, x.sym, x.off, None, false)
                } else {
                    let sym = Sym::new(OP, self.at, self.salt, self.ops + 1);
                    self.ops += 1;
                    self.val(a).join_as(self.val(b), sym, 0, None, false)
                };
                self.put(j)
            }
        }
    }

    fn bit_to_word(&mut self, b: B) -> V {
        let d = match self.bitv(b).konst() {
            Some(x) => Dom::Const(x as u32),
            None => Dom::Set(vec![0, 1].into()),
        };
        self.put_dom(d)
    }

    fn test_bit(&mut self, v: V, n: u32) -> B {
        let x = self.val(v);
        let b = if n == 31 {
            x.pred(&Ranges::new(0x8000_0000, u32::MAX))
        } else {
            match x.dom.elems() {
                Some(e) => Self::mk_bit(&e, |y| y >> n & 1 != 0),
                None => Bit::Top,
            }
        };
        self.put_bit(b)
    }

    fn is_zero(&mut self, v: V) -> B {
        let b = self.val(v).pred(&Ranges::new(0, 0));
        self.put_bit(b)
    }

    fn bnot(&mut self, a: B) -> B {
        let b = self.bitv(a).not();
        self.put_bit(b)
    }

    fn band(&mut self, a: B, b: B) -> B {
        let r = self.bitv(a).and(self.bitv(b));
        self.put_bit(r)
    }

    fn bor(&mut self, a: B, b: B) -> B {
        let r = self.bitv(a).or(self.bitv(b));
        self.put_bit(r)
    }

    fn bxor(&mut self, a: B, b: B) -> B {
        let r = self.bitv(a).xor(self.bitv(b));
        self.put_bit(r)
    }

    fn reg(&mut self, r: Reg) -> V {
        let v = self.state().reg(r).clone();
        self.put(v)
    }

    fn set_reg(&mut self, r: Reg, v: V) {
        if r != Reg::SP {
            self.escape(&[v]);
        }
        let v = self.val(v).clone();
        self.state().set_reg(r, v);
    }

    // Banks aren't tracked: the user bank reads as the current one.
    fn user_reg(&mut self, r: Reg) -> V {
        self.reg(r)
    }

    fn set_user_reg(&mut self, r: Reg, v: V) {
        self.set_reg(r, v)
    }

    fn flag(&mut self, f: Flag) -> B {
        let b = self.state().flags[f.index()].clone();
        self.put_bit(b)
    }

    fn set_flag(&mut self, f: Flag, b: B) {
        let b = self.bitv(b).clone();
        self.state().flags[f.index()] = b;
    }

    fn cpsr(&mut self) -> V {
        self.put_dom(Dom::Top)
    }

    fn set_cpsr(&mut self, _: V, mask: PsrMask) {
        if mask.contains(PsrMask::FLAGS) {
            self.state().flags = std::array::from_fn(|_| Bit::Top);
        }
    }

    fn spsr(&mut self) -> V {
        self.put_dom(Dom::Top)
    }

    fn set_spsr(&mut self, _: V, _: PsrMask) {}

    fn load(&mut self, w: Width, addr: V) -> V {
        let a = self.val(addr).clone();
        self.accesses.push((a.sym == self.sp0, a.dom.clone()));
        let v = self.load_dom(w, &a);
        self.put(v)
    }

    fn store(&mut self, w: Width, addr: V, v: V) {
        self.escape(&[v]);
        // Stores outside the stack aren't tracked (the analysis ignores
        // writes through unknown pointers).
        let a = self.val(addr).clone();
        self.accesses.push((a.sym == self.sp0, a.dom.clone()));
        if w == Width::Word
            && a.dom.konst().is_some_and(|x| IRQ_VECTOR.contains(&x))
            && let Some(k) = self.val(v).dom.konst()
        {
            self.irq_vector = Some(k);
        }
        if a.sym != self.sp0 {
            // Through a pointer that may be into the frame: forget the
            // frame (`int a[4]; a[0] = 0; a[i] = 3`). Known addresses
            // outside IWRAM, where the stack lives, can't be.
            let off_stack = a
                .dom
                .elems()
                .is_some_and(|e| e.iter().all(|&x| !(0x0300_0000..0x0400_0000).contains(&x)));
            if self.state().escaped && !off_stack {
                self.state().stack.clear();
            }
            return;
        }
        let off = a.off as i32;
        let v = self.val(v).clone();
        let stack = &mut self.state().stack;
        if w == Width::Word && off & 3 == 0 && slot_tracked(off) {
            stack.insert(off, v);
        } else {
            stack.remove(&(off & !3));
        }
    }

    fn when(&mut self, c: B, f: impl FnOnce(&mut Self)) {
        let b = self.bitv(c).clone();
        let saved = self.cur.take().expect("when on an ended path");
        self.cur = saved.assume(&b, true);
        if self.cur.is_some() {
            f(self);
        }
        let skipped = saved.assume(&b, false);
        self.cur = match (self.cur.take(), skipped) {
            (Some(x), Some(y)) => {
                let (at, salt) = (self.at, self.salt);
                Some(x.join(&y, |n| Sym::new(WHEN, at, salt, n), false))
            }
            (x, y) => x.or(y),
        };
    }

    fn jump(&mut self, target: V) {
        self.exit(ExitKind::Jump, Some(target));
    }

    fn jump_exchange(&mut self, target: V) {
        self.exit(ExitKind::Exchange, Some(target));
    }

    fn exception_return(&mut self, target: V) {
        self.exit(ExitKind::ExceptionReturn, Some(target));
    }

    fn swi(&mut self, _: u32) {
        self.exit(ExitKind::Swi, None);
    }

    fn undefined(&mut self) {
        self.exit(ExitKind::Undefined, None);
    }
}
