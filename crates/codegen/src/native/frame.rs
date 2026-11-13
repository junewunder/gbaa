//! Stack frames and which of their slots escape (`native/frame-escape`;
//! `docs/native-stage-b.md`, piece 4; `docs/native-port-design.md`, "The
//! stack is gone").
//!
//! Per function, an abstract run of its IR from its entry, tracking which
//! values are the entry sp plus a constant (sp moves included: a push, a
//! `sub sp`), finds:
//!
//! - its **slots**: the byte ranges its sp-relative loads and stores reach,
//!   overlapping ranges merged (`ldm`/`stm` words are an access each);
//! - for each, whether its **address escapes**, and how ([`Kind`]): passed
//!   to a callee in a register; read by a callee as a stack argument;
//!   stored to memory (or to a slot that escapes itself); returned; used
//!   in arithmetic the run can't follow to a constant offset (an index
//!   into a local array: where it lands isn't known); or the frame's sp
//!   lost altogether.
//!
//! An address that escapes at offset `k` is the start of an object of
//! unknown size: the slot holding `k` escapes, and so does every slot above
//! it up to the first slot a prologue saved a register in (every store to
//! it a word holding r4-r11 or lr as they came) or the entry sp, whichever
//! comes first. Locals sit below the saved registers in every frame
//! compiled from C or Rust, so that is as far as the object can reach.
//!
//! A callee may also reach up into its caller's frame from an address at
//! or above its own entry sp that escapes from it (a pointer to a stack
//! argument passed on): at a call to it, that address in the caller's
//! frame escapes too ([`Kind::Args`]), unless the frame is done with after
//! the call (the callee never returns, or it is a tail call), where the
//! lift writes the locals to the frame's bytes before it instead
//! ([`Frame::flush`]). For a call through a register, every function
//! whose address is a word of the ROM may be the callee.
//!
//! The lift (`native/frame-locals`) prints the slots that don't escape, in
//! the function's own frame, as Rust locals ([`Frame::locals`]), and their
//! loads and stores as the locals'; the rest stay the frame's IWRAM bytes.
//! The report counts slots, escaping slots and locals, and the metrics the
//! sp-relative accesses left in memory: to slots that don't escape (none:
//! the bar is 0), to slots that do, and to the caller's frame.
//!
//! Two gaps, weighed by `native/frame-locals`: an escaping address makes
//! only the slots at and above it escape (a pointer into an object's
//! middle reaching below it, or a loop walking down from an end pointer,
//! isn't followed: walking down too would leave three times the accesses
//! in memory on the agb programs); and what a callee reaches up from is
//! followed one call deep (a tail call's callee's reach isn't the tail
//! caller's). `gbaa-lift --shadow-locals` checks a lift against both at
//! run time.
//!
//! The run is on [`FBlock`]s (a block's IR and what each exit does), so
//! the tests build them by hand; [`of_code`] builds them from a lifted
//! context's functions.

use crate::func::{Ctx, ExitInfo, Func, GBlock, Plan, exits, successors};
use crate::saves::Summaries;
use analysis::ir::{BinOp, Exit, Inst, Op, ValueId};
use armv4t::sem::Width;
use std::collections::{BTreeMap, BTreeSet};

/// How a slot's address escapes, a bit each in [`Slot::escapes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// In a register a callee (a call, a tail call, a BIOS call) reads.
    Call = 1,
    /// Read by a callee as a stack argument: the slot is at or above sp at
    /// the call, within the bytes the callee reads above its entry sp.
    Args = 2,
    /// Stored to memory outside the frame, through an address the run
    /// can't place, or to a slot that escapes.
    Stored = 4,
    /// In a register on a return.
    Returned = 8,
    /// Used as an address after arithmetic the run can't follow to a
    /// constant offset.
    Arithmetic = 16,
    /// sp itself lost its offset (a mode change, a join of two different
    /// offsets, sp computed): no slot can be placed for certain.
    Lost = 32,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Call,
        Kind::Args,
        Kind::Stored,
        Kind::Returned,
        Kind::Arithmetic,
        Kind::Lost,
    ];

    /// The report's words for it.
    pub fn what(self) -> &'static str {
        match self {
            Kind::Call => "passed to a callee",
            Kind::Args => "a callee's stack argument",
            Kind::Stored => "stored",
            Kind::Returned => "returned",
            Kind::Arithmetic => "arithmetic past the slot",
            Kind::Lost => "sp lost",
        }
    }
}

/// One slot: bytes `lo..hi` from the entry sp. A slot no access reaches,
/// only an escaping address (a local only a callee writes), is `lo..lo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub lo: i32,
    pub hi: i32,
    /// A prologue's: every store to it is a word holding r4-r11 or lr as
    /// they came.
    pub saved: bool,
    /// The [`Kind`]s it escapes by, a bit each; 0 if it doesn't.
    pub escapes: u8,
}

impl Slot {
    pub fn escapes(&self) -> bool {
        self.escapes != 0
    }

    fn holds(&self, k: i32) -> bool {
        (self.lo <= k && k < self.hi) || (self.lo == k && self.hi == k)
    }
}

/// Where one load or store reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// The entry sp plus this, this many bytes.
    Exact(i32, u8),
    /// Somewhere from these offsets on, by arithmetic the run didn't
    /// follow.
    Through(BTreeSet<i32>),
    /// Not into the frame, as far as the run knows.
    Elsewhere,
}

/// One function's frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// By offset.
    pub slots: Vec<Slot>,
    /// sp lost its offset somewhere: every slot escapes ([`Kind::Lost`]).
    pub lost: bool,
    /// The bytes it reads above its entry sp: its stack arguments.
    pub args: u32,
    /// Each load and store the run reached, by (block, inst index).
    pub places: BTreeMap<(u32, usize), Place>,
    /// The calls, by their exit's (block, inst index), before which the
    /// frame's locals go to its bytes: the callee may reach up into the
    /// frame from an address above its own sp (a pointer to a stack
    /// argument), and the frame is done with after it (the callee never
    /// returns, or it is a tail call).
    pub flush: BTreeSet<(u32, usize)>,
    /// Its stack switches ([`switches`](Frame::switches)): the
    /// instructions that set sp to a value not made from the frame's.
    pub switches: BTreeSet<u32>,
    /// Its fresh stacks: the instructions that set sp, made from the
    /// frame's until then, to a constant (the SDK's `SoftReset`, `mov sp,
    /// r3` with r3 the stack's top, before a reset). Nothing on the old
    /// stack is reached after one in a function that never returns.
    pub fresh: BTreeSet<u32>,
}

impl Frame {
    /// Whether the access at inst `i` of block `b` reaches a slot that
    /// escapes: `None` if the run didn't reach it or it isn't into the
    /// frame.
    pub fn escapes_at(&self, b: u32, i: usize) -> Option<bool> {
        match self.places.get(&(b, i))? {
            Place::Exact(o, n) => Some(
                self.slots
                    .iter()
                    .filter(|s| s.lo < o + *n as i32 && *o < s.hi)
                    .any(Slot::escapes),
            ),
            Place::Through(_) => Some(true),
            Place::Elsewhere => None,
        }
    }

    pub fn escaping(&self) -> usize {
        self.slots.iter().filter(|s| s.escapes()).count()
    }

    /// The slot an access placed exactly reaches, with its offset and
    /// width.
    fn slot_at(&self, b: u32, i: usize) -> Option<(&Slot, i32, u8)> {
        match self.places.get(&(b, i))? {
            Place::Exact(o, n) => {
                let s = self.slots.iter().find(|s| s.lo <= *o && *o < s.hi)?;
                Some((s, *o, *n))
            }
            _ => None,
        }
    }

    /// The slots the lift prints as Rust locals (`native/frame-locals`):
    /// those that don't escape, lie below the entry sp (above it is the
    /// caller's frame: the stack arguments, a frame a fall shares), and
    /// that every access reaches at an offset its width divides (an
    /// unaligned one reaches the bytes the hardware's rounding picks).
    pub fn locals(&self) -> Vec<Local> {
        let mut odd = BTreeSet::new();
        let mut words = BTreeSet::new();
        for p in self.places.values() {
            if let Place::Exact(o, n) = p {
                if o.rem_euclid(*n as i32) != 0 {
                    odd.insert(*o);
                }
                if *n != 4 {
                    words.insert(*o);
                }
            }
        }
        self.slots
            .iter()
            .filter(|s| !s.escapes() && s.lo < s.hi && s.hi <= 0)
            .filter(|s| !odd.iter().any(|&o| s.lo <= o && o < s.hi))
            .map(|s| Local {
                lo: s.lo,
                len: (s.hi - s.lo) as u32,
                word: s.lo % 4 == 0
                    && s.hi - s.lo == 4
                    && !words.iter().any(|&o| s.lo <= o && o < s.hi),
            })
            .collect()
    }

    /// Where the access at inst `i` of block `b` lands in `locals` (this
    /// frame's [`Frame::locals`]): the local, the offset in it, and the
    /// access's width.
    pub fn local_at(&self, locals: &[Local], b: u32, i: usize) -> Option<(Local, u32, u8)> {
        let (s, o, n) = self.slot_at(b, i)?;
        let l = locals.iter().find(|l| l.lo == s.lo)?;
        Some((*l, (o - l.lo) as u32, n))
    }

    /// Whether the access at inst `i` of block `b` is into a slot of the
    /// caller's frame that doesn't escape here: at or above the entry sp.
    pub fn in_callers(&self, b: u32, i: usize) -> bool {
        self.slot_at(b, i)
            .is_some_and(|(s, _, _)| !s.escapes() && s.hi > 0)
    }
}

/// A slot printed as a Rust local: bytes `lo..lo + len` from the entry sp,
/// a `u32` if it is one aligned word only ever accessed whole (`word`),
/// else bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub lo: i32,
    pub len: u32,
    pub word: bool,
}

impl Local {
    /// Its Rust name: `l<bytes below the entry sp>`.
    pub fn name(&self) -> String {
        format!("l{}", -self.lo)
    }
}

/// What an exit does, for the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Go on at these blocks of the function.
    Goto(Vec<u32>),
    /// Call `target` (`None`: through a register), then go on at `ret`.
    Call { target: Option<u32>, ret: u32 },
    /// Return to the caller.
    Return,
    /// Go on in another function (a tail call, a fall into its entry),
    /// which gets every register and the frame.
    Tail(Option<u32>),
    /// A jump by value: to these blocks of the function, or into another
    /// function ([`Edge::Tail`] with no target known).
    TailOr(Vec<u32>),
    /// A BIOS call, then go on at `next`.
    Swi(u32),
    /// Nothing the frame cares about follows (an undefined instruction,
    /// an exception return).
    Stop,
}

/// One block, for the run: its IR and what each exit does, by inst index.
#[derive(Clone, Debug)]
pub struct FBlock<'a> {
    pub insts: &'a [Inst],
    pub edges: BTreeMap<usize, Edge>,
}

/// What the run needs to know of the rest of the program.
pub trait Env {
    /// The registers (bit `n` for rN) a call to `target` reads.
    fn params(&self, target: Option<u32>) -> u16;

    /// Whether function `f`'s address may be taken: a call through a
    /// register may reach it. Every function, unless told otherwise.
    fn taken(&self, _f: u32) -> bool {
        true
    }

    /// Whether a call to `target` may return. Yes, unless told otherwise.
    fn returns(&self, _target: Option<u32>) -> bool {
        true
    }
}

/// Every register r0-r3, for a callee nothing is known of.
pub struct AllFour;

impl Env for AllFour {
    fn params(&self, _: Option<u32>) -> u16 {
        0xf
    }
}

/// What a value is, in terms of the function's entry.
#[derive(Clone, Debug, PartialEq, Eq)]
enum V {
    /// The entry sp plus this.
    Sp(i32),
    Const(u32),
    /// Register `n` as it came (r4-r11, lr).
    Entry(u8),
    /// Made from the entry sp plus these offsets, by arithmetic the run
    /// doesn't follow.
    Taint(BTreeSet<i32>),
    Other,
}

impl V {
    fn bases(&self) -> BTreeSet<i32> {
        match self {
            V::Sp(k) => BTreeSet::from([*k]),
            V::Taint(s) => s.clone(),
            _ => BTreeSet::new(),
        }
    }

    fn tainted(bases: BTreeSet<i32>) -> V {
        if bases.is_empty() {
            V::Other
        } else {
            V::Taint(bases)
        }
    }

    fn join(&self, o: &V) -> V {
        if self == o {
            return self.clone();
        }
        let mut b = self.bases();
        b.extend(o.bases());
        V::tainted(b)
    }
}

fn bin(op: BinOp, a: &V, b: &V) -> V {
    use V::*;
    match (op, a, b) {
        (BinOp::Add, Sp(k), Const(c)) | (BinOp::Add, Const(c), Sp(k)) => {
            Sp(k.wrapping_add(*c as i32))
        }
        (BinOp::Sub, Sp(k), Const(c)) => Sp(k.wrapping_sub(*c as i32)),
        // The distance between two addresses in the frame is a number.
        (BinOp::Sub, Sp(a), Sp(b)) => Const(a.wrapping_sub(*b) as u32),
        (op, Const(a), Const(b)) => Const(match op {
            BinOp::Add => a.wrapping_add(*b),
            BinOp::Sub => a.wrapping_sub(*b),
            BinOp::And => a & b,
            BinOp::Or => a | b,
            BinOp::Xor => a ^ b,
            BinOp::Mul => a.wrapping_mul(*b),
        }),
        _ => {
            let mut s = a.bases();
            s.extend(b.bases());
            V::tainted(s)
        }
    }
}

fn width(w: Width) -> u8 {
    match w {
        Width::Byte => 1,
        Width::Half => 2,
        Width::Word => 4,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    regs: Vec<V>,
    /// Words in the frame, by offset: what the run knows they hold.
    words: BTreeMap<i32, V>,
}

impl State {
    fn entry() -> State {
        let mut regs = vec![V::Other; 16];
        for (i, r) in regs.iter_mut().enumerate() {
            if (4..=11).contains(&i) || i == 14 {
                *r = V::Entry(i as u8);
            }
        }
        regs[13] = V::Sp(0);
        State {
            regs,
            words: BTreeMap::new(),
        }
    }

    fn join(&self, o: &State) -> State {
        State {
            regs: self
                .regs
                .iter()
                .zip(&o.regs)
                .map(|(a, b)| a.join(b))
                .collect(),
            words: self
                .words
                .iter()
                .filter_map(|(k, v)| Some((*k, v.join(o.words.get(k)?))))
                .collect(),
        }
    }
}

/// One call: its sp (if known), its callee (if known), and whether it is
/// a tail call or a fall.
type Call = (Option<i32>, Option<u32>, bool);

/// What one function's run found, before the program's other functions
/// are known.
#[derive(Debug, Default)]
struct Raw {
    places: BTreeMap<(u32, usize), Place>,
    /// Addresses that escape, by offset.
    escapes: BTreeSet<(i32, Kind)>,
    /// Each call, by its exit's (block, inst index).
    calls: BTreeMap<(u32, usize), Call>,
    /// The frame addresses stored in each frame word.
    holds: BTreeMap<i32, BTreeSet<i32>>,
    /// Whether every store to a word is a saved register, by offset.
    saves: BTreeMap<i32, bool>,
    lost: bool,
    /// Stack switches, by instruction: sp, made from the frame's sp until
    /// then, set to a value made from no frame address (`mov sp, r2` with
    /// r2 a parameter, `ldr sp, [r3]` from a global). Not sp set after a
    /// mode change, another mode's: the `msr` is a hole of its own, but
    /// in the reset and the IRQ wrappers, which print it (`PsrFns`).
    switches: BTreeSet<u32>,
    /// Those whose value is a constant ([`Frame::fresh`]), apart.
    fresh: BTreeSet<u32>,
}

struct Run<'a, 'e> {
    blocks: &'a BTreeMap<u32, FBlock<'a>>,
    env: &'e dyn Env,
    ins: BTreeMap<u32, State>,
    work: Vec<u32>,
    raw: Raw,
    /// The previous pass's `holds`: a load of a word whose contents the
    /// run lost yields any frame address ever stored there.
    held: BTreeMap<i32, BTreeSet<i32>>,
}

impl Run<'_, '_> {
    fn flow(&mut self, to: u32, s: &State) {
        if !self.blocks.contains_key(&to) {
            return;
        }
        let next = match self.ins.get(&to) {
            Some(old) => old.join(s),
            None => s.clone(),
        };
        if self.ins.get(&to) != Some(&next) {
            self.ins.insert(to, next);
            self.work.push(to);
        }
    }

    fn escape(&mut self, v: &V, k: Kind) {
        for b in v.bases() {
            self.raw.escapes.insert((b, k));
        }
    }

    fn pass(&mut self, regs: u16, s: &State, k: Kind) {
        for (r, v) in s.regs[..16].iter().enumerate() {
            if regs & 1 << r != 0 && r != 13 {
                self.escape(v, k);
            }
        }
    }

    fn sp(s: &State) -> Option<i32> {
        match s.regs[13] {
            V::Sp(k) => Some(k),
            _ => None,
        }
    }

    fn block(&mut self, key: u32) {
        let b = &self.blocks[&key];
        let mut s = self.ins[&key].clone();
        if !matches!(s.regs[13], V::Sp(_)) {
            self.raw.lost = true;
        }
        let mut vals: Vec<V> = vec![V::Other; b.insts.len()];
        // The instruction each value was made in.
        let mut born: Vec<u32> = vec![key & !1; b.insts.len()];
        let mut at = key & !1;
        for (i, inst) in b.insts.iter().enumerate() {
            born[i] = at;
            let val = |x: ValueId| vals[x.index()].clone();
            let guarded = inst.guard.is_some();
            let v = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    born[i] = a;
                    V::Other
                }
                Op::Const(c) | Op::Pool(c) => V::Const(c),
                Op::Bit(x) => V::Const(x as u32),
                Op::GetReg(r) => s.regs[r.index()].clone(),
                Op::Bin(op, a, b) => bin(op, &val(a), &val(b)),
                // `sub sp, #4` is `sp + !4 + 1`.
                Op::Adc { a, b, c } => bin(BinOp::Add, &bin(BinOp::Add, &val(a), &val(b)), &val(c)),
                Op::Select { a, b, .. } => val(a).join(&val(b)),
                Op::Shift { v, amount, .. } => match val(amount) {
                    V::Const(0) => val(v),
                    _ => V::tainted(val(v).bases()),
                },
                Op::Not(a) if matches!(val(a), V::Const(_)) => match val(a) {
                    V::Const(c) => V::Const(!c),
                    _ => unreachable!(),
                },
                Op::Not(a) | Op::Sext { v: a, .. } | Op::Rrx { v: a, .. } => {
                    V::tainted(val(a).bases())
                }
                Op::MulLongLo { a, b, .. } | Op::MulLongHi { a, b, .. } => {
                    V::tainted(bin(BinOp::Mul, &val(a), &val(b)).bases())
                }
                Op::Load { width: w, addr } => {
                    let n = width(w);
                    match val(addr) {
                        V::Sp(o) => {
                            self.raw.places.insert((key, i), Place::Exact(o, n));
                            match (n, s.words.get(&o)) {
                                (4, Some(x)) => x.clone(),
                                (4, None) => {
                                    V::tainted(self.held.get(&o).cloned().unwrap_or_default())
                                }
                                _ => V::Other,
                            }
                        }
                        a @ V::Taint(_) => {
                            self.escape(&a, Kind::Arithmetic);
                            self.raw.places.insert((key, i), Place::Through(a.bases()));
                            V::Other
                        }
                        _ => {
                            self.raw.places.insert((key, i), Place::Elsewhere);
                            V::Other
                        }
                    }
                }
                Op::Store {
                    width: w,
                    addr,
                    value,
                } => {
                    let n = width(w);
                    let x = val(value);
                    match val(addr) {
                        V::Sp(o) => {
                            self.raw.places.insert((key, i), Place::Exact(o, n));
                            let saved = n == 4 && matches!(x, V::Entry(_));
                            *self.raw.saves.entry(o).or_insert(true) &= saved;
                            let b = x.bases();
                            if !b.is_empty() {
                                self.raw.holds.entry(o).or_default().extend(b);
                            }
                            let old = s.words.get(&o).cloned();
                            s.words.retain(|&k, _| k + 4 <= o || o + n as i32 <= k);
                            let new = match (n, guarded, old) {
                                (4, false, _) => Some(x),
                                (4, true, Some(old)) => Some(old.join(&x)),
                                _ => None,
                            };
                            s.words.extend(new.map(|x| (o, x)));
                        }
                        a @ V::Taint(_) => {
                            self.escape(&a, Kind::Arithmetic);
                            self.escape(&x, Kind::Stored);
                            self.raw.places.insert((key, i), Place::Through(a.bases()));
                        }
                        _ => {
                            self.escape(&x, Kind::Stored);
                            self.raw.places.insert((key, i), Place::Elsewhere);
                        }
                    }
                    V::Other
                }
                Op::SetReg(r, id) => {
                    let r = r.index();
                    let x = val(id);
                    let framed = |v: &V| matches!(v, V::Sp(_)) || !v.bases().is_empty();
                    // Loaded through a frame address: a saved sp restored
                    // (APCS's `ldmdb r11, {r11, sp, lr}`), where the run
                    // lost what the word holds.
                    let root = origin(b.insts, id);
                    let restored = matches!(b.insts[root.index()].op,
                        Op::Load { addr, .. } if framed(&vals[addr.index()]));
                    if r == 13 && framed(&s.regs[13]) && !framed(&x) && !restored {
                        let set = match x {
                            V::Const(_) => &mut self.raw.fresh,
                            _ => &mut self.raw.switches,
                        };
                        set.insert(born[root.index()]);
                    }
                    s.regs[r] = if guarded { s.regs[r].join(&x) } else { x };
                    if r == 13 && !matches!(s.regs[13], V::Sp(_)) {
                        self.raw.lost = true;
                    }
                    V::Other
                }
                // A mode change banks sp: the frame is another one.
                Op::SetCpsr { .. } | Op::SetUserReg(..) => {
                    self.raw.lost = true;
                    s.regs[13] = V::Other;
                    V::Other
                }
                Op::Exit(_) => {
                    self.exit((key, i), &b.edges[&i], &s);
                    V::Other
                }
                _ => V::Other,
            };
            vals[i] = v;
        }
    }

    fn exit(&mut self, at: (u32, usize), e: &Edge, s: &State) {
        match e {
            Edge::Goto(ks) => {
                for &k in ks {
                    self.flow(k, s);
                }
            }
            Edge::Call { target, ret } => {
                self.pass(self.env.params(*target) & 0xf, s, Kind::Call);
                self.raw.calls.insert(at, (Run::sp(s), *target, false));
                let mut after = s.clone();
                for r in [0, 1, 2, 3, 12, 14] {
                    after.regs[r] = V::Other;
                }
                // The callee's frame was below sp.
                match Run::sp(s) {
                    Some(sp) => after.words.retain(|&k, _| k >= sp),
                    None => after.words.clear(),
                }
                self.flow(*ret, &after);
            }
            Edge::Return => self.pass(0xf, s, Kind::Returned),
            Edge::Tail(t) => {
                self.pass(0x5fff, s, Kind::Call);
                self.raw.calls.insert(at, (Run::sp(s), *t, true));
            }
            Edge::TailOr(ks) => {
                self.exit(at, &Edge::Tail(None), s);
                self.exit(at, &Edge::Goto(ks.clone()), s);
            }
            Edge::Swi(next) => {
                self.pass(0xf, s, Kind::Call);
                let mut after = s.clone();
                for r in &mut after.regs[..4] {
                    *r = V::Other;
                }
                self.flow(*next, &after);
            }
            Edge::Stop => {}
        }
    }
}

/// The value a new sp `x` is made from: past adds and subtracts of
/// constants (a `pop` after `mov sp, r2`, which the IR folds into one
/// write of sp), its base, made at the `mov` or the `ldr sp`.
fn origin(insts: &[Inst], mut x: ValueId) -> ValueId {
    let konst = |v: ValueId| matches!(insts[v.index()].op, Op::Const(_) | Op::Pool(_));
    loop {
        match insts[x.index()].op {
            Op::Bin(BinOp::Add | BinOp::Sub, a, c) if konst(c) => x = a,
            Op::Bin(BinOp::Add, c, a) if konst(c) => x = a,
            Op::Adc { a, b, c } if konst(b) && konst(c) => x = a,
            _ => return x,
        }
    }
}

/// One function's run, from `entry` over `blocks`.
fn run(entry: u32, blocks: &BTreeMap<u32, FBlock>, env: &dyn Env) -> Raw {
    let mut held = BTreeMap::new();
    let (mut switches, mut fresh) = (BTreeSet::new(), BTreeSet::new());
    // Each pass's `holds` only grows, and feeds the next: rerun until it
    // stops.
    for _ in 0..8 {
        let mut r = Run {
            blocks,
            env,
            ins: BTreeMap::new(),
            work: Vec::new(),
            raw: Raw::default(),
            held,
        };
        if blocks.contains_key(&entry) {
            r.ins.insert(entry, State::entry());
            r.work.push(entry);
        }
        let mut steps = 0;
        while let Some(k) = r.work.pop() {
            steps += 1;
            if steps > 64 * blocks.len() + 64 {
                r.raw.lost = true;
                break;
            }
            r.block(k);
        }
        if r.raw.holds == r.held {
            return r.raw;
        }
        held = std::mem::take(&mut r.raw.holds);
        switches = std::mem::take(&mut r.raw.switches);
        fresh = std::mem::take(&mut r.raw.fresh);
    }
    Raw {
        lost: true,
        holds: held,
        switches,
        fresh,
        ..Raw::default()
    }
}

impl Raw {
    /// The bytes read above the entry sp.
    fn args(&self) -> u32 {
        self.places
            .values()
            .filter_map(|p| match p {
                Place::Exact(o, n) if *o >= 0 => Some((o + *n as i32) as u32),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    }

    /// The lowest offset at or above the entry sp whose address escapes
    /// or that an access reaches by arithmetic the run didn't follow: from
    /// there the function may reach up into its caller's frame, by an
    /// amount nobody knows (`va_list`, a pointer to a stack argument).
    fn reaches_up(&self) -> Option<i32> {
        let through = self.places.values().flat_map(|p| match p {
            Place::Through(b) => b.iter().copied().collect(),
            _ => Vec::new(),
        });
        self.escapes
            .iter()
            .map(|&(k, _)| k)
            .chain(through)
            .filter(|&k| k >= 0)
            .min()
    }
}

/// What a callee reads of its caller's frame: its exact stack-argument
/// bytes, and the offset above its sp from which it may reach further up
/// ([`Raw::reaches_up`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Reads {
    args: u32,
    up: Option<i32>,
}

impl Reads {
    fn of(raw: &Raw) -> Reads {
        Reads {
            args: raw.args(),
            up: raw.reaches_up(),
        }
    }

    /// What a callee not known reads: the most any function in `all`
    /// reads exactly, and the lowest any whose address `taken` reaches up
    /// from. (The exact bytes stay the most of every function, as
    /// `native/frame-escape` measured them.)
    fn most(all: &BTreeMap<u32, Reads>, taken: impl Fn(u32) -> bool) -> Reads {
        Reads {
            args: all.values().map(|r| r.args).max().unwrap_or(0),
            up: all
                .iter()
                .filter(|(f, _)| taken(**f))
                .filter_map(|(_, r)| r.up)
                .min(),
        }
    }
}

/// The frames of a program's functions, from their runs: `reads` gives
/// what each callee reads of its caller's frame (what any function does,
/// for a callee not known).
fn resolve(raw: Raw, reads: &dyn Fn(Option<u32>) -> Reads, env: &dyn Env) -> Frame {
    // The slots: accessed ranges, overlaps merged.
    let mut ranges: Vec<(i32, i32)> = raw
        .places
        .values()
        .filter_map(|p| match p {
            Place::Exact(o, n) => Some((*o, o + *n as i32)),
            _ => None,
        })
        .collect();
    ranges.sort_unstable();
    let mut slots: Vec<Slot> = Vec::new();
    for (lo, hi) in ranges {
        match slots.last_mut() {
            Some(s) if lo < s.hi => s.hi = s.hi.max(hi),
            _ => slots.push(Slot {
                lo,
                hi,
                saved: false,
                escapes: 0,
            }),
        }
    }
    // An escaping address no access reaches is a slot of its own.
    let unplaced = |slots: &mut Vec<Slot>, b: i32| {
        if !slots.iter().any(|s| s.holds(b)) {
            let at = slots.partition_point(|s| s.lo < b);
            slots.insert(
                at,
                Slot {
                    lo: b,
                    hi: b,
                    saved: false,
                    escapes: 0,
                },
            );
        }
    };
    let mut points: BTreeSet<(i32, Kind)> = raw.escapes.clone();
    for (b, _) in &points {
        unplaced(&mut slots, *b);
    }
    // Held addresses too, as they may escape below.
    for &h in raw.holds.values().flatten() {
        unplaced(&mut slots, h);
    }
    for s in &mut slots {
        let words: Vec<bool> = raw
            .saves
            .range(s.lo..s.hi.max(s.lo + 1))
            .map(|(_, &v)| v)
            .collect();
        s.saved = s.hi - s.lo == 4 && words == [true];
    }
    // Stack arguments: what each callee reads above its sp. One that may
    // reach further up is an escaping address there, an object of unknown
    // size like any other; unless the frame is done with after the call
    // (a callee that never returns, a tail call): then the lift writes its
    // locals to the frame's bytes before it ([`Frame::flush`]).
    let mut ups = Vec::new();
    let mut flush = BTreeSet::new();
    for (&at, &(sp, callee, tail)) in &raw.calls {
        if let Some(sp) = sp {
            let r = reads(callee);
            let n = r.args as i32;
            for s in &mut slots {
                if s.lo < sp + n && sp < s.hi.max(s.lo + 1) {
                    s.escapes |= Kind::Args as u8;
                }
            }
            if let Some(k) = r.up {
                if tail || !env.returns(callee) {
                    flush.insert(at);
                } else {
                    ups.push(sp + k);
                }
            }
        }
    }
    for &b in &ups {
        unplaced(&mut slots, b);
        points.insert((b, Kind::Args));
    }
    // What the stack-argument slots hold escapes too (`f(1, 2, 3, 4, &x)`).
    for s in slots.iter().filter(|s| s.escapes()) {
        for (_, held) in raw.holds.range(s.lo..s.hi.max(s.lo + 1)) {
            points.extend(held.iter().map(|&h| (h, Kind::Stored)));
        }
    }
    // Each escaping address and what it reaches, then what escaping slots
    // hold, until nothing new escapes.
    let mut done = BTreeSet::new();
    while let Some(&p) = points.iter().find(|p| !done.contains(*p)) {
        done.insert(p);
        let (b, k) = p;
        let Some(first) = slots.iter().position(|s| s.holds(b)) else {
            continue;
        };
        for (j, s) in slots.iter_mut().enumerate().skip(first) {
            if j > first && (s.saved || (b < 0 && s.lo >= 0)) {
                break;
            }
            s.escapes |= k as u8;
        }
        for s in slots.iter().filter(|s| s.escapes()) {
            for (_, held) in raw.holds.range(s.lo..s.hi.max(s.lo + 1)) {
                for &h in held {
                    points.insert((h, Kind::Stored));
                }
            }
        }
    }
    if raw.lost {
        for s in &mut slots {
            s.escapes |= Kind::Lost as u8;
        }
    }
    Frame {
        args: raw.args(),
        slots,
        lost: raw.lost,
        places: raw.places,
        flush,
        switches: raw.switches,
        fresh: raw.fresh,
    }
}

/// The frames of the functions in `funcs`, each run over the blocks
/// `blocks(f)` gives from its entry.
pub fn frames<'a>(
    funcs: &[(u32, BTreeMap<u32, FBlock<'a>>)],
    env: &dyn Env,
) -> BTreeMap<u32, Frame> {
    let raws: Vec<(u32, Raw)> = funcs.iter().map(|(e, bs)| (*e, run(*e, bs, env))).collect();
    let by: BTreeMap<u32, Reads> = raws.iter().map(|(e, r)| (*e, Reads::of(r))).collect();
    let most = Reads::most(&by, |f| env.taken(f));
    let reads = |c: Option<u32>| c.and_then(|c| by.get(&c).copied()).unwrap_or(most);
    raws.into_iter()
        .map(|(e, r)| (e, resolve(r, &reads, env)))
        .collect()
}

/// The parameters from the summaries: a callee's, where its summary has
/// no hole; all four otherwise. A function's address is taken if it is a
/// word of the context's bytes: a literal pool, a table in ROM. (Not any
/// constant of the IR: a call's target and its return address are
/// constants too.)
pub struct FromSummaries<'a> {
    pub sums: &'a Summaries,
    pub taken: BTreeSet<u32>,
}

impl Env for FromSummaries<'_> {
    fn params(&self, target: Option<u32>) -> u16 {
        match target.and_then(|t| self.sums.get(t)) {
            Some(s) if s.holes.is_empty() => s.params as u16,
            _ => 0xf,
        }
    }

    fn taken(&self, f: u32) -> bool {
        self.taken.contains(&f)
    }

    fn returns(&self, target: Option<u32>) -> bool {
        target
            .and_then(|t| self.sums.get(t))
            .is_none_or(|s| s.returns)
    }
}

/// The functions of `all` whose address is a word of `bytes`.
fn taken(all: &BTreeSet<u32>, bytes: &[u8]) -> BTreeSet<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .filter(|w| all.contains(w))
        .collect()
}

/// What exit `x` of block `gb` of `f` does, as the lifter prints it: a
/// jump to a block that isn't the function's own goes into another
/// function ([`Edge::Tail`]). `all`: every function's entry.
pub fn edge(ctx: &Ctx, f: &Func, all: &BTreeSet<u32>, gb: &GBlock, x: &ExitInfo) -> Edge {
    let e = match gb.block.insts[x.index].op {
        Op::Exit(e) => e,
        _ => unreachable!("an exit's inst is an exit"),
    };
    let to = |k: u32| {
        if f.blocks.contains_key(&k) {
            Edge::Goto(vec![k])
        } else {
            Edge::Tail(Some(k))
        }
    };
    match x.plan {
        Plan::Static(k) | Plan::Rekey(k) => to(k),
        Plan::Call { target, ret } => Edge::Call {
            target: Some(target),
            ret,
        },
        Plan::CallVia { ret, .. } => Edge::Call { target: None, ret },
        Plan::Dynamic {
            ret: Some(ret),
            target,
            ..
        } => Edge::Call {
            target: target.filter(|t| all.contains(t)),
            ret,
        },
        Plan::Dynamic { .. } if matches!(e, Exit::ExceptionReturn(_)) => Edge::Stop,
        Plan::Dynamic {
            target: None,
            local: false,
            ..
        } => Edge::Return,
        Plan::Dynamic { .. } => {
            let mut own = Vec::new();
            let mut other = false;
            for k in successors(ctx, x) {
                if f.blocks.contains_key(&k) {
                    own.push(k);
                } else {
                    other = true;
                }
            }
            if other {
                Edge::TailOr(own)
            } else {
                Edge::Goto(own)
            }
        }
        Plan::Swi { next } => Edge::Swi(next),
        Plan::Undefined => Edge::Stop,
    }
}

/// The frames of one lifted context's functions, as the lifter prints
/// them ([`edge`]).
pub fn of_code(ctx: &Ctx, funcs: &[Func], sums: &Summaries) -> BTreeMap<u32, Frame> {
    let all: BTreeSet<u32> = funcs.iter().map(|f| f.entry).collect();
    let fs: Vec<(u32, BTreeMap<u32, FBlock>)> = funcs
        .iter()
        .map(|f| {
            let bs = f
                .blocks
                .iter()
                .map(|(&k, gb)| {
                    let edges = exits(ctx, gb)
                        .iter()
                        .map(|x| (x.index, edge(ctx, f, &all, gb, x)))
                        .collect();
                    (
                        k,
                        FBlock {
                            insts: &gb.block.insts,
                            edges,
                        },
                    )
                })
                .collect();
            (f.entry, bs)
        })
        .collect();
    let env = FromSummaries {
        sums,
        taken: taken(&all, ctx.rom),
    };
    frames(&fs, &env)
}

#[cfg(test)]
mod tests;
