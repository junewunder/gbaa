//! Stack slots lent to callees, and the parameters that borrow them
//! (`native/lift-stack-arrays`; `docs/native-port-design.md`, "Stack
//! locals passed by address").
//!
//! A **borrowed parameter** is one of a direct callee's r0-r3 whose value
//! the callee only adds constants to, loads and stores through at a
//! constant offset, and passes in r0-r3 to another borrowed parameter: no
//! store of it, no return of it, no compare or other arithmetic that
//! reaches an effect, no BIOS or pointer call with it, no two values met
//! at a join that something then uses. A function whose address is a word
//! of the ROM (a pointer call may reach it) borrows nothing, and nor does
//! one whose summary has a hole. Every caller must pass, there, an exact
//! address of its own frame that it lends, or a borrowed parameter of its
//! own plus a constant: one type a parameter.
//!
//! A **lent slot** escapes only so ([`Kind::Call`] alone, every pass of
//! an address below or in it an exact frame address in r0-r3 of a direct
//! call, [`Frame::lends`], to a borrowed parameter). The sets are the
//! greatest such, found by taking out what fails until nothing does.
//!
//! Each function's run tracks, through registers only, which values are a
//! parameter (or the entry sp) plus a constant; a parameter spilled to the
//! frame, or met with another value where the result is used, isn't
//! borrowed. The frame's own run ([`super::frames`]) places the slots.
//!
//! `GBAA_LEND_DEBUG=1` prints, per function, its pointer parameters and
//! why each escaped (`guard`: a compare that reaches a branch; `store
//! value`; `load addr`/`store addr`: arithmetic past a constant offset;
//! `return`), and each parameter the program's rules take out.

use super::{Edge, FBlock, Frame, Kind};
use analysis::ir::{BinOp, Op};
use std::collections::{BTreeMap, BTreeSet};

/// What a value is, for the run: parameter `r` (r0-r3; 13 the entry sp)
/// plus a constant, a constant, made from parameters (a bit each) by
/// arithmetic, or anything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum W {
    Arg(u8, i32),
    Const(u32),
    Mixed(u8),
    Other,
}

impl W {
    /// The parameters r0-r3 it is made from, a bit each.
    fn params(self) -> u8 {
        match self {
            W::Arg(r, _) if r < 4 => 1 << r,
            W::Mixed(m) => m,
            _ => 0,
        }
    }

    fn mixed(m: u8) -> W {
        if m == 0 { W::Other } else { W::Mixed(m) }
    }

    fn join(self, o: W) -> W {
        if self == o {
            self
        } else {
            W::mixed(self.params() | o.params())
        }
    }
}

fn add(a: W, b: W, sub: bool) -> W {
    match (a, b) {
        (W::Arg(r, k), W::Const(c)) => W::Arg(
            r,
            if sub {
                k.wrapping_sub(c as i32)
            } else {
                k.wrapping_add(c as i32)
            },
        ),
        (W::Const(c), W::Arg(r, k)) if !sub => W::Arg(r, k.wrapping_add(c as i32)),
        (W::Const(a), W::Const(b)) => W::Const(if sub {
            a.wrapping_sub(b)
        } else {
            a.wrapping_add(b)
        }),
        _ => W::mixed(a.params() | b.params()),
    }
}

/// An exit, by its (block, inst index).
type At = (u32, usize);

/// A load or store through a parameter: (parameter, offset, width).
type Through = (u8, i32, u8);

/// Bytes `lo..hi` from an address.
type Span = (i32, i32);

/// What one function's run found.
#[derive(Clone, Debug, Default)]
struct FnRun {
    /// Its parameters (a bit each) that aren't borrowed by what it does
    /// with them itself.
    escaped: u8,
    /// The bytes it reaches through each parameter: the lowest and the
    /// end of the highest offset.
    reach: BTreeMap<u8, (i32, i32)>,
    /// What each direct call (and tail call to a known function) passes
    /// in the callee's parameters r0-r3: (callee, its register, the
    /// value), by the exit's (block, inst index).
    passes: BTreeMap<At, (u32, Vec<(u8, W)>)>,
    /// Its loads and stores through a parameter, by (block, inst index):
    /// (parameter, offset, width).
    through: BTreeMap<(u32, usize), (u8, i32, u8)>,
    /// The parameters (a bit each) spilled to each frame word: borrowed
    /// only if no lent address reaches it.
    spills: Vec<(u8, i32)>,
    /// Why each parameter escaped, for `GBAA_LEND_DEBUG`: (block, what).
    whys: Vec<(u8, &'static str)>,
}

fn esc(out: &mut FnRun, m: u8, why: &'static str) {
    if m & !out.escaped != 0 {
        out.whys.push((m & !out.escaped, why));
    }
    out.escaped |= m;
}

/// What the program's run needs of the rest of it.
pub trait LendEnv {
    /// The registers r0-r3 a direct call to `f` reads (bit `n` for rN),
    /// and whether they are known (its summary has no hole).
    fn params(&self, f: u32) -> (u8, bool);
    /// The registers r0-r3 a caller may read after `f` returns.
    fn read_after(&self, f: u32) -> u8;
    /// The registers r4-r11 a direct call to `f` passes it (bit `n` for
    /// rN), which it reads as they came.
    fn takes(&self, f: u32) -> u16;
    /// Whether a pointer call may reach `f`.
    fn taken(&self, f: u32) -> bool;
}

#[derive(Clone, PartialEq, Eq)]
struct State {
    regs: [W; 16],
    /// The parameters each flag (N, Z, C, V) was made from: a compare's
    /// flags, or a flag-setting add's never read.
    flags: [u8; 4],
    /// What the run knows its frame's words hold, by offset from the
    /// entry sp: a parameter spilled (agbcc keeps one there across calls).
    words: BTreeMap<i32, W>,
}

/// `frame`: the function's own, whose escaping slots a callee may write
/// (a word spilled there isn't known after a call).
fn run(
    entry: u32,
    blocks: &BTreeMap<u32, FBlock>,
    frame: Option<&Frame>,
    env: &dyn LendEnv,
) -> FnRun {
    // A word a callee may write other than through a lent address (which
    // the program's lends check after: [`FnRun::spills`]).
    let escapes = |k: i32| {
        frame.is_none_or(|f| {
            f.lost
                || f.slots.iter().any(|s| {
                    s.escapes & !(Kind::Call as u8) != 0 && s.lo < k + 4 && k < s.hi.max(s.lo + 1)
                })
        })
    };
    let mut out = FnRun::default();
    let mut regs = [W::Other; 16];
    for (r, w) in regs.iter_mut().enumerate().take(4) {
        *w = W::Arg(r as u8, 0);
    }
    regs[13] = W::Arg(13, 0);
    let mut ins: BTreeMap<u32, State> = BTreeMap::new();
    let mut work = Vec::new();
    if blocks.contains_key(&entry) {
        ins.insert(
            entry,
            State {
                regs,
                flags: [0; 4],
                words: BTreeMap::new(),
            },
        );
        work.push(entry);
    }
    let mut steps = 0;
    let flow = |to: u32, s: &State, ins: &mut BTreeMap<u32, State>, work: &mut Vec<u32>| {
        if !blocks.contains_key(&to) {
            return;
        }
        let next = match ins.get(&to) {
            Some(old) => State {
                regs: std::array::from_fn(|r| old.regs[r].join(s.regs[r])),
                flags: std::array::from_fn(|f| old.flags[f] | s.flags[f]),
                words: old
                    .words
                    .iter()
                    .filter_map(|(k, v)| Some((*k, v.join(*s.words.get(k)?))))
                    .collect(),
            },
            None => s.clone(),
        };
        if ins.get(&to) != Some(&next) {
            ins.insert(to, next);
            work.push(to);
        }
    };
    while let Some(key) = work.pop() {
        steps += 1;
        if steps > 64 * blocks.len() + 64 {
            out.escaped = 0xf;
            break;
        }
        let b = &blocks[&key];
        let mut s = ins[&key].clone();
        let mut vals = vec![W::Other; b.insts.len()];
        let mut return_word: Option<W> = None;
        for (i, inst) in b.insts.iter().enumerate() {
            let val = |x: analysis::ir::ValueId| vals[x.index()];
            // A skipped instruction's timing: no use of what it reads.
            if let (Some(g), false) = (inst.guard, matches!(inst.op, Op::Timing(_))) {
                esc(&mut out, val(g).params(), "guard");
            }
            let guarded = inst.guard.is_some();
            let v = match inst.op {
                Op::Const(c) | Op::Pool(c) => W::Const(c),
                Op::Bit(x) => W::Const(x as u32),
                // sp is word-aligned: a frame address's low bits are known.
                Op::Bin(BinOp::And, a, c)
                    if matches!((val(a), val(c)), (W::Arg(13, _), W::Const(0..=3))) =>
                {
                    match (val(a), val(c)) {
                        (W::Arg(_, k), W::Const(c)) => W::Const(k as u32 & c),
                        _ => unreachable!(),
                    }
                }
                // A rotation's amount, from a known one.
                Op::Shift {
                    kind: armv4t::sem::ShiftType::Lsl,
                    v,
                    amount,
                } if matches!((val(v), val(amount)), (W::Const(_), W::Const(0..=31))) => {
                    match (val(v), val(amount)) {
                        (W::Const(x), W::Const(n)) => W::Const(x << n),
                        _ => unreachable!(),
                    }
                }
                // An unaligned load's rotation, from the address's low
                // bits: a borrowed access is checked aligned, so these
                // are known, not a use of the pointer.
                Op::Bin(BinOp::And, a, c)
                    if matches!(val(a), W::Arg(..)) && matches!(val(c), W::Const(0..=3)) =>
                {
                    W::Other
                }
                // `adds r4, r0, #0` (agbcc's move): its carry and
                // overflow are clear whatever r0 is.
                Op::AdcCarry { b, c, .. } | Op::AdcOverflow { b, c, .. }
                    if val(b) == W::Const(0) && val(c) == W::Const(0) =>
                {
                    W::Const(0)
                }
                Op::GetReg(r) => s.regs[r.index()],
                Op::GetFlag(f) => W::mixed(s.flags[f as usize]),
                Op::SetFlag(f, x) => {
                    let m = val(x).params();
                    s.flags[f as usize] = if guarded { s.flags[f as usize] | m } else { m };
                    W::Other
                }
                Op::YieldFlag(..) => W::Other,
                Op::Bin(BinOp::Add, a, c) => add(val(a), val(c), false),
                Op::Bin(BinOp::Sub, a, c) => add(val(a), val(c), true),
                // `sub r0, #4` is `r0 + !4 + 1`.
                Op::Adc { a, b, c } => add(add(val(a), val(b), false), val(c), false),
                Op::Shift { v, amount, .. } if val(amount) == W::Const(0) => val(v),
                Op::Not(a) if matches!(val(a), W::Const(_)) => match val(a) {
                    W::Const(c) => W::Const(!c),
                    _ => unreachable!(),
                },
                Op::Select { c, a, b } => {
                    let m = val(c).params();
                    let j = val(a).join(val(b));
                    if m == 0 { j } else { W::mixed(m | j.params()) }
                }
                Op::Load { width, addr } => {
                    match val(addr) {
                        W::Arg(r, k) if r < 4 => {
                            let n = super::width(width);
                            reach(&mut out, r, k, n);
                            out.through.insert((key, i), (r, k, n));
                        }
                        W::Arg(13, k) if super::width(width) == 4 => {
                            return_word = s.words.get(&k).copied();
                        }
                        w => esc(&mut out, w.params(), "load addr"),
                    }
                    return_word.take().unwrap_or(W::Other)
                }
                Op::Store { width, addr, value } => {
                    match val(addr) {
                        W::Arg(r, k) if r < 4 => {
                            let n = super::width(width);
                            reach(&mut out, r, k, n);
                            out.through.insert((key, i), (r, k, n));
                        }
                        W::Arg(13, k) => {
                            let n = super::width(width) as i32;
                            s.words.retain(|&o, _| o + 4 <= k || k + n <= o);
                            // A spill to a word nothing else reaches.
                            if n == 4 && !guarded && !escapes(k) {
                                let m = val(value).params();
                                if m != 0 {
                                    out.spills.push((m, k));
                                }
                                s.words.insert(k, val(value));
                                vals[i] = W::Other;
                                continue;
                            }
                        }
                        w => esc(&mut out, w.params(), "store addr"),
                    }
                    esc(&mut out, val(value).params(), "store value");
                    W::Other
                }
                Op::SetReg(r, x) => {
                    let r = r.index();
                    s.regs[r] = if guarded {
                        s.regs[r].join(val(x))
                    } else {
                        val(x)
                    };
                    W::Other
                }
                Op::Exit(_) => {
                    exit(&mut out, entry, (key, i), &b.edges[&i], &s, env);
                    match &b.edges[&i] {
                        Edge::Goto(ks) | Edge::TailOr(ks) => {
                            for &k in ks {
                                flow(k, &s, &mut ins, &mut work);
                            }
                        }
                        Edge::Call { ret, .. } => {
                            let mut after = s.clone();
                            for r in [0, 1, 2, 3, 12, 14] {
                                after.regs[r] = W::Other;
                            }
                            after.words.retain(|&k, _| !escapes(k));
                            flow(*ret, &after, &mut ins, &mut work);
                        }
                        Edge::Swi(next) => {
                            let mut after = s.clone();
                            for r in &mut after.regs[..4] {
                                *r = W::Other;
                            }
                            flow(*next, &after, &mut ins, &mut work);
                        }
                        Edge::Return | Edge::Tail(_) | Edge::Stop => {}
                    }
                    W::Other
                }
                // A pure op makes a value of what it reads; an effect
                // (a flag, the CPSR, a jump's target) with a parameter in
                // it is a use the lift can't print for a borrowed one.
                ref op => {
                    let m = op.operands().iter().fold(0, |m, &x| m | val(x).params());
                    if op.is_root() {
                        esc(&mut out, m, "effect");
                    }
                    W::mixed(m)
                }
            };
            vals[i] = v;
        }
    }
    out
}

fn reach(out: &mut FnRun, r: u8, k: i32, n: u8) {
    let e = out.reach.entry(r).or_insert((k, k + n as i32));
    e.0 = e.0.min(k);
    e.1 = e.1.max(k + n as i32);
}

fn exit(out: &mut FnRun, entry: u32, at: (u32, usize), e: &Edge, s: &State, env: &dyn LendEnv) {
    let any = |regs: u16| {
        (0..16)
            .filter(|r| regs & 1 << r != 0 && *r != 13)
            .fold(0, |m, r| m | s.regs[r].params())
    };
    match e {
        Edge::Call {
            target: Some(t), ..
        }
        | Edge::Tail(Some(t)) => {
            let (params, _) = env.params(*t);
            let passed = (0..4u8)
                .filter(|r| params & 1 << r != 0)
                .map(|r| (r, s.regs[r as usize]))
                .collect();
            out.passes.insert(at, (*t, passed));
            // r4-r11 a callee takes, r12, lr: not a borrowed parameter.
            let rest = env.takes(*t);
            esc(out, any(rest), "call rest");
        }
        Edge::Call { target: None, .. } | Edge::Tail(None) => esc(out, any(0x5fff), "ptr call"),
        Edge::Swi(_) => esc(out, any(0xf), "swi"),
        // What callers read of r0-r3 after it returns.
        Edge::Return => esc(out, any(env.read_after(entry) as u16), "return"),
        Edge::Goto(_) | Edge::TailOr(_) | Edge::Stop => {}
    }
    if let Edge::TailOr(_) = e {
        esc(out, any(0x5fff), "tailor");
    }
}

/// The program's borrowed parameters and lent slots.
#[derive(Clone, Debug, Default)]
pub struct Lends {
    /// Each function's borrowed parameters, a bit each (rN, r0-r3); only
    /// functions with one.
    pub borrowed: BTreeMap<u32, u8>,
    /// Each function's lent slots, by their `lo` ([`Frame::slots`]).
    pub lent: BTreeMap<u32, BTreeSet<i32>>,
    /// The pointer parameters considered: r0-r3 a function reads that some
    /// load or store reaches through, or some call passes on.
    pub pointers: usize,
    /// Each function's loads and stores through a borrowed parameter, by
    /// (block, inst index): (parameter, offset, width).
    pub through: BTreeMap<u32, BTreeMap<At, Through>>,
    /// The bytes each borrowed parameter reaches, by (function, register).
    pub reach: BTreeMap<(u32, u8), (i32, i32)>,
}

impl Lends {
    /// The sp-relative accesses into lent slots.
    pub fn accesses(&self, frames: &BTreeMap<u32, Frame>) -> usize {
        let mut n = 0;
        for (f, lent) in &self.lent {
            let fr = &frames[f];
            for p in fr.places.values() {
                if let super::Place::Exact(o, w) = p
                    && fr
                        .slots
                        .iter()
                        .any(|s| lent.contains(&s.lo) && s.lo < o + *w as i32 && *o < s.hi)
                {
                    n += 1;
                }
            }
        }
        n
    }
}

/// What [`lent_slots`] finds in one frame.
#[derive(Clone, Debug, Default)]
struct FrameLends {
    /// The lent slots, by `lo`.
    lent: BTreeSet<i32>,
    /// The frame addresses lent: each passed exactly, only to borrowed
    /// parameters, the bytes they reach all lent slots.
    good: BTreeSet<i32>,
    /// The slots some passed address may reach (lent or not), by `lo`. A
    /// slot that escapes by calls alone and isn't here escaped only by
    /// the frame's guess at an object's size: a lent address reaches no
    /// further than its borrowers do.
    reached: BTreeSet<i32>,
}

/// The slots of `fr` lent, given what each of its calls' r0-r3 borrows
/// reach (`reach(exit, register)`, the bytes from the address passed;
/// `None` if it isn't a borrowed parameter). A slot is lent when it
/// escapes by calls alone, lies below the entry sp, some lent address
/// reaches it and no other passed address may: an address passed any
/// other way reaches up to the saved registers, as the frame takes any
/// escape's object ([`super::resolve`]); a lent one only the bytes its
/// borrowers reach.
fn lent_slots(fr: &Frame, reach: &dyn Fn(&At, u8) -> Option<Span>) -> FrameLends {
    if fr.lost {
        return FrameLends::default();
    }
    // The slots an address passed any other way may reach, by index.
    let object = |k: i32| -> Vec<usize> {
        let Some(first) = fr
            .slots
            .iter()
            .position(|s| s.lo <= k && k < s.hi.max(s.lo + 1))
        else {
            return Vec::new();
        };
        let mut v = vec![first];
        for (j, s) in fr.slots.iter().enumerate().skip(first + 1) {
            if s.saved || (k < 0 && s.lo >= 0) {
                break;
            }
            v.push(j);
        }
        v
    };
    // The slots bytes `lo..hi` overlap.
    let span = |lo: i32, hi: i32| -> Vec<usize> {
        (0..fr.slots.len())
            .filter(|&j| {
                let s = &fr.slots[j];
                s.lo < hi && lo < s.hi.max(s.lo + 1)
            })
            .collect()
    };
    // Each address passed: the bytes it reaches if lent so far.
    let mut bases: BTreeMap<i32, Option<(i32, i32)>> =
        fr.passed_else.iter().map(|&k| (k, None)).collect();
    for (at, ps) in &fr.lends {
        for &(reg, k) in ps {
            let r = reach(at, reg)
                .map(|(lo, hi)| (k + lo, k + hi))
                .filter(|&(_, hi)| hi <= 0);
            let e = bases.entry(k).or_insert(r);
            *e = match (*e, r) {
                (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
                _ => None,
            };
        }
    }
    loop {
        let mut lent: BTreeSet<usize> = (0..fr.slots.len())
            .filter(|&j| fr.slots[j].escapes == Kind::Call as u8 && fr.slots[j].lo < 0)
            .collect();
        for (&k, r) in &bases {
            if r.is_none() {
                for j in object(k) {
                    lent.remove(&j);
                }
            }
        }
        let mut changed = false;
        for r in bases.values_mut() {
            if let Some((lo, hi)) = *r
                && !span(lo, hi).iter().all(|j| lent.contains(j))
            {
                *r = None;
                changed = true;
            }
        }
        if !changed {
            let mut reached = BTreeSet::new();
            let mut good_slots = BTreeSet::new();
            for (&k, r) in &bases {
                match r {
                    Some((lo, hi)) => {
                        let sp = span(*lo, *hi);
                        good_slots.extend(sp.iter().copied());
                        reached.extend(sp);
                    }
                    None => reached.extend(object(k)),
                }
            }
            return FrameLends {
                lent: lent
                    .intersection(&good_slots)
                    .map(|&j| fr.slots[j].lo)
                    .collect(),
                good: bases
                    .iter()
                    .filter(|(_, r)| r.is_some())
                    .map(|(&k, _)| k)
                    .collect(),
                reached: reached.iter().map(|&j| fr.slots[j].lo).collect(),
            };
        }
    }
}

/// The bytes each borrowed parameter reaches, through itself and the
/// borrowed parameters it passes itself on to, from its value: by
/// (function, register). One whose reach doesn't settle (a recursion
/// passing itself on further each time) isn't here.
fn reaches(
    runs: &BTreeMap<u32, FnRun>,
    borrowed: &BTreeMap<u32, u8>,
) -> BTreeMap<(u32, u8), (i32, i32)> {
    let mut out: BTreeMap<(u32, u8), (i32, i32)> = BTreeMap::new();
    for (f, m) in borrowed {
        for (p, r) in &runs[f].reach {
            if m & 1 << p != 0 {
                out.insert((*f, *p), *r);
            }
        }
    }
    for _ in 0..64 {
        let mut changed = false;
        for (f, m) in borrowed {
            for (c, ps) in runs[f].passes.values() {
                for (r, w) in ps {
                    let W::Arg(p, k) = *w else { continue };
                    if p >= 4 || m & 1 << p == 0 {
                        continue;
                    }
                    let Some(&(lo, hi)) = out.get(&(*c, *r)) else {
                        continue;
                    };
                    let (lo, hi) = (lo + k, hi + k);
                    let e = out.entry((*f, p)).or_insert((lo, hi));
                    let new = (e.0.min(lo), e.1.max(hi));
                    if new != *e {
                        *e = new;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return out;
        }
    }
    BTreeMap::new()
}

/// The program's lends, from its functions' blocks and frames.
pub fn lends(
    funcs: &[(u32, BTreeMap<u32, FBlock>)],
    frames: &BTreeMap<u32, Frame>,
    env: &dyn LendEnv,
) -> Lends {
    let runs: BTreeMap<u32, FnRun> = funcs
        .iter()
        .map(|(e, bs)| (*e, run(*e, bs, frames.get(e), env)))
        .collect();
    // Who calls each function, and how: (caller, exit).
    let mut callers: BTreeMap<u32, Vec<(u32, At)>> = BTreeMap::new();
    for (f, r) in &runs {
        for (at, (t, _)) in &r.passes {
            callers.entry(*t).or_default().push((*f, *at));
        }
    }
    let mut pointers = 0;
    let mut borrowed: BTreeMap<u32, u8> = BTreeMap::new();
    for (f, r) in &runs {
        let (params, known) = env.params(*f);
        let used: u8 = r.reach.keys().fold(0, |m, &p| m | 1 << p)
            | r.passes
                .values()
                .flat_map(|(_, ps)| ps.iter())
                .fold(0, |m, (_, w)| m | w.params());
        let ptrs = params & used;
        pointers += ptrs.count_ones() as usize;
        if std::env::var_os("GBAA_LEND_DEBUG").is_some() {
            eprintln!(
                "lend {f:#x}: params {params:#x} known {known} pointers {used:#x} escaped {:#x} {:?} taken {}",
                r.escaped,
                r.whys,
                env.taken(*f)
            );
        }
        let ok = if known && !env.taken(*f) && callers.contains_key(f) {
            ptrs & !r.escaped
        } else {
            0
        };
        if ok != 0 {
            borrowed.insert(*f, ok);
        }
    }
    loop {
        let mut changed = false;
        let reach = reaches(&runs, &borrowed);
        let mut fl: BTreeMap<u32, FrameLends> = BTreeMap::new();
        for (f, fr) in frames {
            let t_of =
                |at: &(u32, usize)| runs.get(f).and_then(|r| r.passes.get(at)).map(|(t, _)| *t);
            let l = lent_slots(fr, &|at, reg| {
                let t = t_of(at)?;
                borrowed.get(&t).filter(|m| *m & 1 << reg != 0)?;
                Some(reach.get(&(t, reg)).copied().unwrap_or((0, 0)))
            });
            fl.insert(*f, l);
        }
        // A spilled parameter, in a word a passed address may reach.
        for (f, r) in &runs {
            for &(m, k) in &r.spills {
                let hit = fl.get(f).is_some_and(|l| {
                    frames[f].slots.iter().any(|s| {
                        l.reached.contains(&s.lo) && s.lo < k + 4 && k < s.hi.max(s.lo + 1)
                    })
                });
                if let Some(b) = borrowed.get_mut(f).filter(|b| **b & m != 0 && hit) {
                    *b &= !m;
                    changed = true;
                }
            }
        }
        let good: BTreeMap<u32, BTreeSet<i32>> =
            fl.iter().map(|(f, l)| (*f, l.good.clone())).collect();
        // A borrowed parameter every caller passes a lent slot or a
        // borrowed parameter of its own, and that passes itself only to
        // borrowed parameters.
        let mut next = borrowed.clone();
        for (t, m) in &borrowed {
            for reg in 0..4u8 {
                if m & 1 << reg == 0 {
                    continue;
                }
                let ok_callers = callers.get(t).is_some_and(|cs| {
                    cs.iter().all(|(f, at)| {
                        let w = runs[f].passes[at]
                            .1
                            .iter()
                            .find(|(r, _)| *r == reg)
                            .map_or(W::Other, |(_, w)| *w);
                        match w {
                            W::Arg(13, k) => {
                                frames.get(f).is_some_and(|fr| {
                                    fr.lends.get(at).is_some_and(|ps| ps.contains(&(reg, k)))
                                }) && good.get(f).is_some_and(|g| g.contains(&k))
                            }
                            W::Arg(p, _) => borrowed.get(f).is_some_and(|bm| bm & 1 << p != 0),
                            _ => false,
                        }
                    })
                });
                let ok_passes = runs[t].passes.values().all(|(c, ps)| {
                    ps.iter().all(|(r, w)| match w {
                        W::Arg(p, _) if *p == reg => {
                            borrowed.get(c).is_some_and(|cm| cm & 1 << r != 0)
                        }
                        W::Mixed(mm) if mm & 1 << reg != 0 => false,
                        _ => true,
                    })
                });
                if !(ok_callers && ok_passes) && std::env::var_os("GBAA_LEND_DEBUG").is_some() {
                    eprintln!(
                        "lend {t:#x}: r{reg} unborrowed: callers {ok_callers} passes on {ok_passes}"
                    );
                }
                if !(ok_callers && ok_passes) {
                    let e = next.get_mut(t).unwrap();
                    *e &= !(1 << reg);
                    changed = true;
                }
            }
        }
        next.retain(|_, m| *m != 0);
        borrowed = next;
        if !changed {
            let through = borrowed
                .iter()
                .map(|(f, m)| {
                    let t = runs[f]
                        .through
                        .iter()
                        .filter(|(_, (p, _, _))| m & 1 << p != 0)
                        .map(|(k, v)| (*k, *v))
                        .collect();
                    (*f, t)
                })
                .collect();
            let lent = fl
                .into_iter()
                .filter(|(_, l)| !l.lent.is_empty())
                .map(|(f, l)| (f, l.lent))
                .collect();
            return Lends {
                borrowed,
                lent,
                pointers,
                through,
                reach,
            };
        }
    }
}

/// The lift's [`LendEnv`]: from the summaries; a function's address is
/// taken as for the frames ([`super::FromSummaries`]), and a stand-in
/// (`skip`) borrows nothing.
struct FromSummaries<'a> {
    sums: &'a crate::saves::Summaries,
    taken: BTreeSet<u32>,
    skip: &'a BTreeSet<u32>,
}

impl LendEnv for FromSummaries<'_> {
    fn params(&self, f: u32) -> (u8, bool) {
        match self.sums.get(f) {
            Some(s) => (s.params & 0xf, s.holes.is_empty() && !s.stopped),
            None => (0xf, false),
        }
    }

    fn takes(&self, f: u32) -> u16 {
        match self.sums.get(f) {
            Some(s) => (s.may_take() as u16) << 4,
            None => 0xff0,
        }
    }

    fn read_after(&self, f: u32) -> u8 {
        match self.sums.get(f) {
            Some(s) if s.holes.is_empty() => ((s.read_after | s.results) & 0xf) as u8,
            _ => 0xf,
        }
    }

    fn taken(&self, f: u32) -> bool {
        self.taken.contains(&f) || self.skip.contains(&f)
    }
}

/// The lends of one lifted context's functions, whose `frames` are
/// [`super::of_code`]'s; `skip`: the stand-ins.
pub fn of_code(
    ctx: &crate::func::Ctx,
    funcs: &[crate::func::Func],
    sums: &crate::saves::Summaries,
    frames: &BTreeMap<u32, Frame>,
    skip: &BTreeSet<u32>,
) -> Lends {
    let all: BTreeSet<u32> = funcs.iter().map(|f| f.entry).collect();
    let fs: Vec<(u32, BTreeMap<u32, FBlock>)> = funcs
        .iter()
        .map(|f| (f.entry, super::fblocks(ctx, f, &all)))
        .collect();
    let env = FromSummaries {
        sums,
        taken: super::taken(&all, ctx.rom),
        skip,
    };
    lends(&fs, frames, &env)
}
