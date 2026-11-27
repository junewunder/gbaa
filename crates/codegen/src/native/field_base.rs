//! A field's address held in a register, printed on its base (unit
//! `native/lift-field-base`): agbcc keeps `&kirby->unk56` in a register
//! (`r5 = kirby + 0x56`) and loads, stores and reloads through it blocks
//! later. The access types by subtracting the offset back,
//! `Ref::<t::Kirby>::to(r5, 0x56).base().unk56()`; where a register
//! still holds the base, `Ref::<t::Kirby>::new(r6).base().unk56()` is
//! the same address and names the object.
//!
//! A forward pass over one function's blocks finds, at each block's
//! start and on each edge, the facts `rN == rM + k` (r0-r12) and
//! `rN == k` that hold on every path there: within a block each value is
//! a register's value at the block's start, a constant, or some other
//! value, plus a constant (`add` and `sub` of a constant, Thumb's `adds`
//! with its carry in), and two registers that are the same such root
//! plus constants make a fact; a fact coming in carries over to the
//! registers still holding its sides. Any write of a register kills its
//! facts (a guarded one too), and so does what an exit sets as printed
//! (a call's results, the r4-r11 its callee gives back: the caller says,
//! [`Edge::kills`]); a `msr` or user-bank transfer kills all. A block
//! entered other than by the edges given (the function's entry) starts
//! with none.
//!
//! [`base_of`] then answers for a pointer at an access: the object's
//! start as a constant (`(gRow + 0x2)` 2 bytes into a `t::Row` is
//! `gRow`), or as a register holding it, which the printer names
//! instead. The printer takes only a register whose value comes into the
//! block (not written in it before the access), that is live there, and
//! whose `for` step isn't left out, so the writes that reach it are all
//! printed as the register's own (no `let` of the block's, none left out
//! as dead): the variable holds the value. [`base_in`] answers for a
//! `for`'s stepped pointer, printed as an element of what it pointed at
//! before the loop: from the facts on the edges into the loop, a register
//! the loop doesn't write.

use std::collections::{BTreeMap, BTreeSet};

use analysis::ir::{BinOp, Inst, Op, ValueId};

/// `rN == rM + k`, as `(n, m, k)`; `m` [`ZERO`] for `rN == k`.
pub type Fact = (u8, u8, u32);

/// The registers tracked: r0-r12 (sp, lr and pc are no object's).
const REGS: usize = 13;

/// A register that is always 0, so that `rN == k` is a fact: slot 13 of
/// the walk (never sp's: sp isn't tracked).
pub const ZERO: u8 = 13;

/// Where an exit goes on in the function, and the registers (bit `n` for
/// rN) the printed exit sets on the way: a call's results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub to: usize,
    pub kills: u16,
}

/// The start of an object: a register holding it, or a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Reg(u8),
    Const(u32),
}

/// What [`facts`] finds: the facts at each block's start (`None`: no
/// edge reaches it), and on each edge, by its blocks.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub inc: Vec<Option<BTreeSet<Fact>>>,
    pub edges: BTreeMap<(usize, usize), BTreeSet<Fact>>,
}

/// What a value is: a register's value at the block's start, or the
/// value of an inst of the block's, plus a constant (a constant is
/// [`ZERO`]'s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Root {
    Entry(u8),
    Val(usize),
}

type Sym = (Root, u32);

/// The walk of one block from its start.
struct Walk<'a> {
    insts: &'a [Inst],
    sym: Vec<Sym>,
    /// r0-r12, then [`ZERO`].
    cur: [Sym; REGS + 1],
    /// Everything was killed (a mode change).
    lost: bool,
}

impl<'a> Walk<'a> {
    fn new(insts: &'a [Inst]) -> Self {
        Walk {
            insts,
            sym: Vec::with_capacity(insts.len()),
            cur: std::array::from_fn(|r| (Root::Entry(r as u8), 0)),
            lost: false,
        }
    }

    /// Up to inst `at`.
    fn to(insts: &'a [Inst], at: usize) -> Self {
        let mut w = Walk::new(insts);
        for i in 0..at {
            w.step(i);
        }
        w
    }

    /// `x` as a constant: a word, a bit, or their complement or sum.
    fn konst(&self, x: ValueId) -> Option<u32> {
        match self.insts[x.index()].op {
            Op::Const(c) | Op::Pool(c) => Some(c),
            Op::Bit(b) => Some(u32::from(b)),
            Op::Not(y) => self.konst(y).map(|c| !c),
            Op::Bin(BinOp::Add, a, b) => Some(self.konst(a)?.wrapping_add(self.konst(b)?)),
            Op::Bin(BinOp::Sub, a, b) => Some(self.konst(a)?.wrapping_sub(self.konst(b)?)),
            _ => None,
        }
    }

    /// Take inst `i` (the walk is at `i`).
    fn step(&mut self, i: usize) {
        let inst = &self.insts[i];
        let fresh = (Root::Val(i), 0);
        let s = match inst.op {
            Op::Const(c) | Op::Pool(c) => (Root::Entry(ZERO), c),
            Op::GetReg(r) if r.index() < REGS => self.cur[r.index()],
            Op::Bin(BinOp::Add, a, b) => match (self.konst(a), self.konst(b)) {
                (_, Some(c)) => add(self.sym[a.index()], c),
                (Some(c), _) => add(self.sym[b.index()], c),
                _ => fresh,
            },
            Op::Bin(BinOp::Sub, a, b) => match self.konst(b) {
                Some(c) => add(self.sym[a.index()], c.wrapping_neg()),
                None => fresh,
            },
            // Thumb's `adds` and `subs`: a carry in that is a constant.
            Op::Adc { a, b, c } => match (self.konst(a), self.konst(b), self.konst(c)) {
                (_, Some(k), Some(c)) => add(self.sym[a.index()], k.wrapping_add(c)),
                (Some(k), _, Some(c)) => add(self.sym[b.index()], k.wrapping_add(c)),
                _ => fresh,
            },
            _ => fresh,
        };
        self.sym.push(s);
        match inst.op {
            Op::SetReg(r, x) if r.index() < REGS => {
                self.cur[r.index()] = match inst.guard {
                    Some(_) => fresh,
                    None => self.sym[x.index()],
                };
            }
            Op::SetCpsr { .. } | Op::SetUserReg(..) => {
                self.lost = true;
                for (r, c) in self.cur[..REGS].iter_mut().enumerate() {
                    *c = (Root::Val(usize::MAX - r), 0);
                }
            }
            _ => {}
        }
    }

    /// The facts that hold now, from those `inc` at the block's start.
    fn facts(&self, inc: &BTreeSet<Fact>) -> BTreeSet<Fact> {
        let mut out = BTreeSet::new();
        for n in 0..REGS {
            for m in 0..=REGS {
                if n != m && self.cur[n].0 == self.cur[m].0 {
                    out.insert((n as u8, m as u8, self.cur[n].1.wrapping_sub(self.cur[m].1)));
                }
            }
        }
        if !self.lost {
            for &(a, b, k) in inc {
                for n in (0..REGS).filter(|&n| self.cur[n].0 == Root::Entry(a)) {
                    for m in (0..=REGS).filter(|&m| self.cur[m].0 == Root::Entry(b)) {
                        if n != m {
                            let d = k.wrapping_add(self.cur[n].1).wrapping_sub(self.cur[m].1);
                            out.insert((n as u8, m as u8, d));
                        }
                    }
                }
            }
        }
        out
    }
}

fn add((r, k): Sym, c: u32) -> Sym {
    (r, k.wrapping_add(c))
}

/// The facts at each block's start and on each edge, with `entry`
/// entered from outside and `edges(b, i)` the edges of block `b`'s exit
/// at inst `i` (none for one leaving the function).
pub fn facts(blocks: &[&[Inst]], entry: usize, edges: impl Fn(usize, usize) -> Vec<Edge>) -> Facts {
    let mut inc: Vec<Option<BTreeSet<Fact>>> = vec![None; blocks.len()];
    inc[entry] = Some(BTreeSet::new());
    let mut work: BTreeSet<usize> = [entry].into();
    while let Some(b) = work.pop_first() {
        let here = inc[b].clone().expect("only reached blocks are queued");
        let mut w = Walk::new(blocks[b]);
        for (i, inst) in blocks[b].iter().enumerate() {
            if let Op::Exit(_) = inst.op {
                let now = w.facts(&here);
                for e in edges(b, i) {
                    let out = killed(&now, e.kills);
                    let merged = match &inc[e.to] {
                        None => out,
                        Some(old) => old.intersection(&out).copied().collect(),
                    };
                    if inc[e.to].as_ref() != Some(&merged) {
                        inc[e.to] = Some(merged);
                        work.insert(e.to);
                    }
                }
            }
            w.step(i);
        }
    }
    // The entry may be a loop's head too: whatever reaches it from
    // outside holds nothing.
    inc[entry] = Some(BTreeSet::new());
    // Each edge's, from its block's start as settled.
    let mut on: BTreeMap<(usize, usize), BTreeSet<Fact>> = BTreeMap::new();
    for (b, here) in inc.iter().enumerate() {
        let Some(here) = here else { continue };
        let mut w = Walk::new(blocks[b]);
        for (i, inst) in blocks[b].iter().enumerate() {
            if let Op::Exit(_) = inst.op {
                let now = w.facts(here);
                for e in edges(b, i) {
                    let out = killed(&now, e.kills);
                    on.entry((b, e.to))
                        .and_modify(|old| *old = old.intersection(&out).copied().collect())
                        .or_insert(out);
                }
            }
            w.step(i);
        }
    }
    Facts { inc, edges: on }
}

/// `now` but the facts of the registers of `kills`.
fn killed(now: &BTreeSet<Fact>, kills: u16) -> BTreeSet<Fact> {
    now.iter()
        .copied()
        .filter(|&(n, m, _)| m == ZERO && kills & 1 << n == 0 || kills & (1 << n | 1 << m) == 0)
        .collect()
}

/// The start of the object a pointer `x` (`k` bytes into it) points
/// into, at inst `at` of a block whose insts are `insts` and that `inc`
/// holds at the start of (`x` an earlier value of the block): a constant,
/// or a register of `live` (bit `n` for rN) written nowhere in the block
/// before `at`.
pub fn base_of(
    insts: &[Inst],
    inc: &BTreeSet<Fact>,
    at: usize,
    x: ValueId,
    k: u32,
    live: u32,
) -> Option<Base> {
    let w = Walk::to(insts, at);
    if w.lost || x.index() >= at {
        return None;
    }
    let (root, kx) = w.sym[x.index()];
    let Root::Entry(a) = root else {
        return None;
    };
    // The start is the register `a` came in as, less `d`.
    let d = k.wrapping_sub(kx);
    if a == ZERO {
        return Some(Base::Const(kx.wrapping_sub(k)));
    }
    let unwritten = |m: u8| w.cur[m as usize] == (Root::Entry(m), 0);
    let start = |m: u8| (m == a && d == 0) || inc.contains(&(a, m, d));
    // `a` came in holding a constant.
    if let Some(&(_, _, c)) = inc.iter().find(|&&(n, m, _)| n == a && m == ZERO) {
        return Some(Base::Const(c.wrapping_add(kx).wrapping_sub(k)));
    }
    pick((0..REGS as u8).filter(|&m| live & 1 << m != 0 && unwritten(m) && start(m)))
}

/// The start of the object a register `n` points `k` bytes into, as
/// the facts `on` give it (those on the edges into a loop): a constant,
/// or a register of `ok` (bit `m` for rM).
pub fn base_in(on: &BTreeSet<Fact>, n: u8, k: u32, ok: u32) -> Option<Base> {
    if let Some(&(_, _, c)) = on.iter().find(|&&(a, m, _)| a == n && m == ZERO) {
        return Some(Base::Const(c.wrapping_sub(k)));
    }
    pick((0..REGS as u8).filter(|&m| ok & 1 << m != 0 && on.contains(&(n, m, k))))
}

/// One of r4-r11 first (where agbcc keeps a variable), then the lowest.
fn pick(found: impl Iterator<Item = u8>) -> Option<Base> {
    let found: Vec<u8> = found.collect();
    found
        .iter()
        .copied()
        .find(|m| (4..12).contains(m))
        .or_else(|| found.first().copied())
        .map(Base::Reg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use analysis::ir::Exit;
    use armv4t::sem::Reg;

    fn inst(op: Op) -> Inst {
        Inst { op, guard: None }
    }

    fn get(r: u8) -> Op {
        Op::GetReg(Reg(r))
    }

    fn set(r: u8, x: u32) -> Op {
        Op::SetReg(Reg(r), ValueId(x))
    }

    fn exit() -> Op {
        Op::Exit(Exit::Fallthrough(0))
    }

    /// Block 0: `r5 = r6 + 0x56`; block 1 reads r5 as a pointer 0x56 into
    /// its object: r6 holds the start.
    #[test]
    fn a_field_address_names_its_base() {
        let b0 = [
            inst(get(6)),
            inst(Op::Const(0x56)),
            inst(Op::Bin(BinOp::Add, ValueId(0), ValueId(1))),
            inst(set(5, 2)),
            inst(exit()),
        ];
        let b1 = [inst(get(5)), inst(exit())];
        let blocks: [&[Inst]; 2] = [&b0, &b1];
        let edges = |b: usize, _| match b {
            0 => vec![Edge { to: 1, kills: 0 }],
            _ => vec![],
        };
        let inc = facts(&blocks, 0, edges).inc;
        let at1 = inc[1].as_ref().unwrap();
        assert!(at1.contains(&(5, 6, 0x56)), "{at1:?}");
        assert_eq!(
            base_of(&b1, at1, 1, ValueId(0), 0x56, 1 << 6),
            Some(Base::Reg(6))
        );
        // Another offset, a dead base or a killed edge: none.
        assert_eq!(base_of(&b1, at1, 1, ValueId(0), 0x58, 1 << 6), None);
        assert_eq!(base_of(&b1, at1, 1, ValueId(0), 0x56, 0), None);
        let killed = facts(&blocks, 0, |b: usize, _| match b {
            0 => vec![Edge {
                to: 1,
                kills: 1 << 6,
            }],
            _ => vec![],
        })
        .inc;
        assert!(killed[1].as_ref().unwrap().is_empty());
    }

    /// Thumb's `adds r1, r6, #0x62` and `subs r2, r1, #2`: an add with a
    /// constant carry in.
    #[test]
    fn a_thumb_add_with_carry_is_a_sum() {
        let b0 = [
            inst(get(6)),
            inst(Op::Const(0x62)),
            inst(Op::Bit(false)),
            inst(Op::Adc {
                a: ValueId(1),
                b: ValueId(0),
                c: ValueId(2),
            }),
            inst(set(1, 3)),
            inst(Op::Const(2)),
            inst(Op::Not(ValueId(5))),
            inst(Op::Bit(true)),
            inst(Op::Adc {
                a: ValueId(3),
                b: ValueId(6),
                c: ValueId(7),
            }),
            inst(set(2, 8)),
            inst(exit()),
        ];
        let b1 = [inst(get(1)), inst(exit())];
        let blocks: [&[Inst]; 2] = [&b0, &b1];
        let inc = facts(&blocks, 0, |b: usize, _| match b {
            0 => vec![Edge { to: 1, kills: 0 }],
            _ => vec![],
        })
        .inc;
        let at = inc[1].as_ref().unwrap();
        assert!(at.contains(&(1, 6, 0x62)), "{at:?}");
        assert!(at.contains(&(2, 6, 0x60)), "{at:?}");
        assert_eq!(
            base_of(&b1, at, 1, ValueId(0), 0x62, 1 << 6),
            Some(Base::Reg(6))
        );
    }

    /// Two paths into a join: the fact holds only if both give it.
    #[test]
    fn a_join_keeps_what_every_path_gives() {
        let sum = [
            inst(get(6)),
            inst(Op::Const(0x10)),
            inst(Op::Bin(BinOp::Add, ValueId(0), ValueId(1))),
            inst(set(4, 2)),
            inst(exit()),
        ];
        let other = [inst(Op::Const(3)), inst(set(4, 0)), inst(exit())];
        let join = [inst(get(4)), inst(exit())];
        let blocks: [&[Inst]; 4] = [&[inst(exit())], &sum, &other, &join];
        let two = |b: usize, _| match b {
            0 => vec![Edge { to: 1, kills: 0 }, Edge { to: 2, kills: 0 }],
            1 | 2 => vec![Edge { to: 3, kills: 0 }],
            _ => vec![],
        };
        let inc = facts(&blocks, 0, two).inc;
        assert!(inc[3].as_ref().unwrap().is_empty());
        let one = |b: usize, _| match b {
            0 => vec![Edge { to: 1, kills: 0 }],
            1 => vec![Edge { to: 3, kills: 0 }],
            _ => vec![],
        };
        let inc = facts(&blocks, 0, one).inc;
        assert!(inc[3].as_ref().unwrap().contains(&(4, 6, 0x10)));
        // A write of the base in the block before the read: none.
        let rewrite = [
            inst(get(4)),
            inst(Op::Const(1)),
            inst(set(6, 1)),
            inst(exit()),
        ];
        let at = inc[3].as_ref().unwrap();
        assert_eq!(base_of(&rewrite, at, 3, ValueId(0), 0x10, 1 << 6), None);
        assert_eq!(
            base_of(&rewrite, at, 2, ValueId(0), 0x10, 1 << 6),
            Some(Base::Reg(6))
        );
    }

    /// A fact carries through a block that copies its base.
    #[test]
    fn a_copy_carries_the_fact() {
        let b0 = [
            inst(get(6)),
            inst(Op::Const(0x56)),
            inst(Op::Bin(BinOp::Add, ValueId(0), ValueId(1))),
            inst(set(5, 2)),
            inst(exit()),
        ];
        let b1 = [
            inst(get(6)),
            inst(set(7, 0)),
            inst(Op::Const(1)),
            inst(set(6, 2)),
            inst(exit()),
        ];
        let b2 = [inst(get(5)), inst(exit())];
        let blocks: [&[Inst]; 3] = [&b0, &b1, &b2];
        let edges = |b: usize, _| match b {
            0 => vec![Edge { to: 1, kills: 0 }],
            1 => vec![Edge { to: 2, kills: 0 }],
            _ => vec![],
        };
        let inc = facts(&blocks, 0, edges).inc;
        let at = inc[2].as_ref().unwrap();
        assert!(at.contains(&(5, 7, 0x56)), "{at:?}");
        assert!(at.contains(&(6, ZERO, 1)), "{at:?}");
        assert!(
            !at.iter().any(|&(n, m, _)| m == 6 || n == 6 && m != ZERO),
            "{at:?}"
        );
        assert_eq!(
            base_of(&b2, at, 1, ValueId(0), 0x56, 1 << 7 | 1 << 6),
            Some(Base::Reg(7))
        );
    }

    /// A register set to a constant blocks before, and a loop's way in:
    /// the constant's start, and the facts on the edge into the loop
    /// that its latch's step leaves out.
    #[test]
    fn a_constant_and_a_loop_s_way_in() {
        // Block 0: r4 = 0x3000052, r6 = r7 + 2; block 1, the loop:
        // r4 += 2 and r6 += 2, back to itself.
        let b0 = [
            inst(Op::Const(0x300_0052)),
            inst(set(4, 0)),
            inst(get(7)),
            inst(Op::Const(2)),
            inst(Op::Bin(BinOp::Add, ValueId(2), ValueId(3))),
            inst(set(6, 4)),
            inst(exit()),
        ];
        let b1 = [
            inst(get(4)),
            inst(Op::Const(2)),
            inst(Op::Bin(BinOp::Add, ValueId(0), ValueId(1))),
            inst(set(4, 2)),
            inst(get(6)),
            inst(Op::Bin(BinOp::Add, ValueId(4), ValueId(1))),
            inst(set(6, 5)),
            inst(exit()),
        ];
        let blocks: [&[Inst]; 2] = [&b0, &b1];
        let f = facts(&blocks, 0, |_, _| vec![Edge { to: 1, kills: 0 }]);
        let at = f.inc[1].as_ref().unwrap();
        // The loop's start holds neither: the step moves both.
        assert!(!at.iter().any(|&(n, _, _)| n == 4 || n == 6), "{at:?}");
        let on = &f.edges[&(0, 1)];
        assert!(on.contains(&(4, ZERO, 0x300_0052)), "{on:?}");
        assert_eq!(base_in(on, 4, 2, !0), Some(Base::Const(0x300_0050)));
        assert_eq!(base_in(on, 6, 2, 1 << 7), Some(Base::Reg(7)));
        assert_eq!(base_in(on, 6, 2, 0), None);
        // Block 0's own constant, read blocks later.
        let b2 = [inst(get(4)), inst(exit())];
        let blocks: [&[Inst]; 2] = [&b0, &b2];
        let f = facts(&blocks, 0, |b: usize, _| match b {
            0 => vec![Edge { to: 1, kills: 0 }],
            _ => vec![],
        });
        let at = f.inc[1].as_ref().unwrap();
        assert_eq!(
            base_of(&b2, at, 1, ValueId(0), 2, 0),
            Some(Base::Const(0x300_0050))
        );
    }
}
