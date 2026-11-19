//! Compare trees printed as a `match` (unit `native/lift-switch-match`;
//! `docs/research/llm-pilot.md`'s pattern list). agbcc compiles a small
//! switch as a binary search of `cmp`/`beq`/`bgt` on one register: in the
//! IR, a run of guarded exits at a block's end, each taken on the flags of
//! one compare of the same value against a constant, and the block a
//! branch leads to may go on comparing it. [`run`] finds such a run in a
//! block; [`arms`] gives each leaf of a tree of runs the values of the
//! compared value that reach it, which the printer prints as the `match`'s
//! patterns.
//!
//! The run holds no loads, stores or reads of the flags: its other effects
//! (register and flag writes) are the arms', each arm printing those before
//! its exit. A guard's condition is its truth table over N, Z, C and V,
//! evaluated from the compare's `AddWithCarry` on every value: the values
//! split at five points per compare, between which no flag changes.

use analysis::ir::{BitOp, Op, ValueId};
use analysis::ir::{Exit, Inst};
use armv4t::sem::Reg;

/// One compare's guard: taken on the flags of `x + b + c` where its table
/// has bit `n << 3 | z << 2 | c << 1 | v` set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Test {
    pub b: u32,
    pub c: bool,
    pub table: u16,
}

impl Test {
    /// Whether the guard holds when the compared value is `x`.
    pub fn taken(&self, x: u32) -> bool {
        let s = u64::from(x) + u64::from(self.b) + u64::from(self.c);
        let r = s as u32;
        let n = r >> 31 != 0;
        let z = r == 0;
        let c = s >> 32 != 0;
        let v = ((x ^ r) & (self.b ^ r)) >> 31 != 0;
        self.table >> idx(n, z, c, v) & 1 != 0
    }

    /// The values at which a flag of `x + b + c` may change.
    fn breaks(&self) -> [u32; 5] {
        let t = self.b.wrapping_add(u32::from(self.c));
        let zero = 0u32.wrapping_sub(t);
        [
            0,
            0x8000_0000,
            zero,
            zero.wrapping_add(1),
            0x8000_0000u32.wrapping_sub(t),
        ]
    }
}

fn idx(n: bool, z: bool, c: bool, v: bool) -> u32 {
    u32::from(n) << 3 | u32::from(z) << 2 | u32::from(c) << 1 | u32::from(v)
}

/// A block's compare run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The value compared.
    pub value: ValueId,
    /// Its first inst: the register and flag writes from here to its end
    /// are the arms' ([`Run::deferred`]).
    pub start: usize,
    /// The guarded exits, by inst, with their compares, in order.
    pub tests: Vec<(usize, Test)>,
    /// Where it goes when no test is taken.
    pub end: End,
}

/// Where a run goes when no test is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// The block's last exit, at this inst.
    Exit(usize),
    /// The rest of the block, from this inst (the last test's next).
    Rest(usize),
}

impl Run {
    /// The inst its run of insts ends before.
    fn stop(&self) -> usize {
        match self.end {
            End::Exit(i) | End::Rest(i) => i,
        }
    }

    /// Whether inst `i` is a register or flag write of the run, which
    /// the arms print.
    pub fn deferred(&self, insts: &[Inst], i: usize) -> bool {
        i >= self.start
            && i < self.stop()
            && matches!(insts[i].op, Op::SetReg(..) | Op::SetFlag(..))
    }

    /// Whether inst `i` is one of the run's exits.
    pub fn exit(&self, i: usize) -> bool {
        self.end == End::Exit(i) || self.tests.iter().any(|&(j, _)| j == i)
    }

    /// The exits, the last after the tests.
    pub fn exits(&self) -> impl Iterator<Item = usize> + '_ {
        let last = match self.end {
            End::Exit(i) => Some(i),
            End::Rest(_) => None,
        };
        self.tests.iter().map(|&(j, _)| j).chain(last)
    }

    /// The inst at which the block prints its `match`: the last exit, or
    /// the last test before the rest.
    pub fn at(&self) -> usize {
        match self.end {
            End::Exit(i) => i,
            End::Rest(i) => i - 1,
        }
    }
}

/// The compare run of a block, of at least `min` tests, or how many of
/// its guarded exits compare one value against a constant (the most on
/// any one value), when they don't make one. The run ends at the block's
/// last exit, else at its last test, the block going on after it (the
/// last such run of the block's).
pub fn run(insts: &[Inst], min: usize) -> Result<Run, usize> {
    let compares = compares(insts);
    let last = insts.len().checked_sub(1).ok_or(0usize)?;
    if !matches!(insts[last].op, Op::Exit(_)) || insts[last].guard.is_some() {
        return Err(compares);
    }
    let min = min.max(1);
    let (start, tests) = scan(insts, last);
    if tests.len() >= min {
        return Ok(Run {
            value: tests[0].0,
            start,
            tests: tests.into_iter().map(|(_, i, t)| (i, t)).collect(),
            end: End::Exit(last),
        });
    }
    for e in (0..last).rev() {
        if !matches!(insts[e].op, Op::Exit(_)) || insts[e].guard.is_none() {
            continue;
        }
        let (start, tests) = scan(insts, e + 1);
        if tests.len() >= min && tests.last().is_some_and(|t| t.1 == e) {
            return Ok(Run {
                value: tests[0].0,
                start,
                tests: tests.into_iter().map(|(_, i, t)| (i, t)).collect(),
                end: End::Rest(e + 1),
            });
        }
    }
    Err(compares)
}

/// The run of insts before `top`, back to the first a run can't hold or
/// a guarded exit that isn't a compare of the same value: its first inst
/// and its tests (value, inst, compare), in order. A run holds no loads,
/// stores, flag or PSR reads, and no register read after a write to it
/// (the reads print before the `match`, the writes in its arms).
fn scan(insts: &[Inst], top: usize) -> (usize, Vec<(ValueId, usize, Test)>) {
    let mut read_later = [false; 16];
    let mut tests: Vec<(ValueId, usize, Test)> = Vec::new();
    let mut start = 0;
    for i in (0..top).rev() {
        let ok = match insts[i].op {
            Op::GetReg(r) => {
                read_later[r.index()] = true;
                true
            }
            Op::SetReg(r, _) => !read_later[r.index()],
            Op::SetFlag(..) | Op::Insn(_) | Op::Timing(_) | Op::YieldFlag(..) => true,
            Op::Exit(_) => match insts[i].guard.and_then(|g| classify(insts, g)) {
                Some((x, t)) if tests.first().is_none_or(|f| f.0 == x) => {
                    tests.insert(0, (x, i, t));
                    true
                }
                _ => false,
            },
            Op::GetFlag(_) => false,
            ref op => super::pure(op),
        };
        if !ok {
            start = i + 1;
            break;
        }
    }
    (start, tests)
}

/// The most guarded exits of the block that compare one value against a
/// constant.
pub fn compares(insts: &[Inst]) -> usize {
    let mut by: Vec<(ValueId, usize)> = Vec::new();
    for inst in insts {
        let (Op::Exit(_), Some(g)) = (inst.op, inst.guard) else {
            continue;
        };
        if let Some((x, _)) = classify(insts, g) {
            match by.iter_mut().find(|(y, _)| *y == x) {
                Some((_, n)) => *n += 1,
                None => by.push((x, 1)),
            }
        }
    }
    by.iter().map(|&(_, n)| n).max().unwrap_or(0)
}

/// A guard as a compare of one value against a constant: the value and
/// the [`Test`].
pub fn classify(insts: &[Inst], g: ValueId) -> Option<(ValueId, Test)> {
    let mut triple = None;
    eval(insts, g, 0, &mut triple)?;
    let (a, b, c) = triple?;
    let carry = match insts[c.index()].op {
        Op::Bit(x) => x,
        _ => return None,
    };
    let (x, k) = match (super::konst(insts, a), super::konst(insts, b)) {
        (None, Some(k)) => (a, k),
        (Some(k), None) => (b, k),
        _ => return None,
    };
    let mut table = 0u16;
    for f in 0..16u32 {
        let mut t = triple;
        if eval(insts, g, f, &mut t)? {
            table |= 1 << f;
        }
    }
    Some((
        x,
        Test {
            b: k,
            c: carry,
            table,
        },
    ))
}

/// A guard's value with the flags `f` (bits N, Z, C, V from 3 down), if
/// it is made only of the flags of one `AddWithCarry`, whose operands it
/// records in `triple`.
fn eval(
    insts: &[Inst],
    g: ValueId,
    f: u32,
    triple: &mut Option<(ValueId, ValueId, ValueId)>,
) -> Option<bool> {
    let mut of = |a, b, c| match *triple {
        Some(t) if t != (a, b, c) => None,
        _ => {
            *triple = Some((a, b, c));
            Some(())
        }
    };
    let adc = |v: ValueId| match insts[v.index()].op {
        Op::Adc { a, b, c } => Some((a, b, c)),
        _ => None,
    };
    Some(match insts[g.index()].op {
        Op::Bit(b) => b,
        Op::BNot(x) => !eval(insts, x, f, triple)?,
        Op::BBin(o, x, y) => {
            let (x, y) = (eval(insts, x, f, triple)?, eval(insts, y, f, triple)?);
            match o {
                BitOp::And => x & y,
                BitOp::Or => x | y,
                BitOp::Xor => x ^ y,
            }
        }
        Op::TestBit { v, n: 31 } => {
            let (a, b, c) = adc(v)?;
            of(a, b, c)?;
            f & 8 != 0
        }
        Op::IsZero(v) => {
            let (a, b, c) = adc(v)?;
            of(a, b, c)?;
            f & 4 != 0
        }
        Op::AdcCarry { a, b, c } => {
            of(a, b, c)?;
            f & 2 != 0
        }
        Op::AdcOverflow { a, b, c } => {
            of(a, b, c)?;
            f & 1 != 0
        }
        _ => return None,
    })
}

/// The register holding `value` at inst `at` of a block: one written it
/// last, unguarded, or the one it was read from, unwritten since.
pub fn holder(insts: &[Inst], value: ValueId, at: usize) -> Option<Reg> {
    let written = |r: Reg| {
        insts[..at]
            .iter()
            .rposition(|i| matches!(i.op, Op::SetReg(s, _) if s == r))
    };
    if let Op::GetReg(r) = insts[value.index()].op
        && written(r).is_none()
    {
        return Some(r);
    }
    for r in 0..13u8 {
        let r = Reg(r);
        if let Some(w) = written(r)
            && insts[w].op == Op::SetReg(r, value)
            && insts[w].guard.is_none()
        {
            return Some(r);
        }
    }
    None
}

/// Whether a block's insts before `start` are only reads of registers
/// and pure ops: a block a tree may print inside another's arm.
pub fn quiet_before(insts: &[Inst], start: usize) -> bool {
    insts[..start].iter().all(|i| match i.op {
        Op::GetReg(_) | Op::GetFlag(_) | Op::Insn(_) | Op::Timing(_) => true,
        Op::Exit(_) => false,
        ref op => super::pure(op),
    })
}

/// A tree of runs: a run, and for each of its exits (the tests', then
/// the last) the run its edge goes on to, if it goes on comparing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    pub run: Run,
    pub next: Vec<Option<Tree>>,
}

impl Tree {
    /// Its tests, in every run.
    pub fn tests(&self) -> usize {
        self.run.tests.len() + self.next.iter().flatten().map(Tree::tests).sum::<usize>()
    }

    /// Its leaves, depth first: the exits that leave the tree.
    pub fn leaves(&self) -> usize {
        self.next
            .iter()
            .map(|n| n.as_ref().map_or(1, Tree::leaves))
            .sum()
    }

    fn breaks(&self, out: &mut Vec<u32>) {
        for (_, t) in &self.run.tests {
            out.extend(t.breaks());
        }
        for n in self.next.iter().flatten() {
            n.breaks(out);
        }
    }

    /// The leaf (depth first) that value `x` reaches.
    fn leaf(&self, x: u32) -> usize {
        let mut base = 0;
        let pick = self
            .run
            .tests
            .iter()
            .position(|(_, t)| t.taken(x))
            .unwrap_or(self.run.tests.len());
        for n in &self.next[..pick] {
            base += n.as_ref().map_or(1, Tree::leaves);
        }
        base + self.next[pick].as_ref().map_or(0, |n| n.leaf(x))
    }
}

/// Each leaf's values, as inclusive ranges in order; `None` if a leaf
/// has none (an exit no value takes).
pub fn arms(t: &Tree) -> Option<Vec<Vec<(u32, u32)>>> {
    let mut b = Vec::new();
    t.breaks(&mut b);
    b.push(0);
    b.sort_unstable();
    b.dedup();
    let mut arms = vec![Vec::<(u32, u32)>::new(); t.leaves()];
    for (i, &lo) in b.iter().enumerate() {
        let hi = b.get(i + 1).map_or(u32::MAX, |&n| n - 1);
        let l = t.leaf(lo);
        if t.leaf(hi) != l {
            return None;
        }
        match arms[l].last_mut() {
            Some((_, h)) if h.wrapping_add(1) == lo => *h = hi,
            _ => arms[l].push((lo, hi)),
        }
    }
    arms.iter().all(|a| !a.is_empty()).then_some(arms)
}

/// A leaf's ranges as a `match` pattern.
pub fn pattern(ranges: &[(u32, u32)]) -> String {
    let n = |x: u32| {
        if x < 10 {
            x.to_string()
        } else {
            format!("{x:#x}")
        }
    };
    ranges
        .iter()
        .map(|&(lo, hi)| {
            if lo == hi {
                n(lo)
            } else {
                format!("{}..={}", n(lo), n(hi))
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Whether `e` is an exit a tree's edge may go on through: a plain jump.
pub fn plain(e: &Op) -> bool {
    matches!(e, Op::Exit(Exit::Jump(_) | Exit::Fallthrough(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use analysis::ir::Inst;

    fn i(op: Op) -> Inst {
        Inst { op, guard: None }
    }

    /// `cmp r0, #k` then a guard from its flags, as the builder makes it.
    fn cmp(insts: &mut Vec<Inst>, x: ValueId, k: u32, cond: &str) -> ValueId {
        let mut push = |op| {
            insts.push(i(op));
            ValueId(insts.len() as u32 - 1)
        };
        let b = push(Op::Const(!k));
        let c = push(Op::Bit(true));
        let r = push(Op::Adc { a: x, b, c });
        let n = push(Op::TestBit { v: r, n: 31 });
        let z = push(Op::IsZero(r));
        let cf = push(Op::AdcCarry { a: x, b, c });
        let v = push(Op::AdcOverflow { a: x, b, c });
        match cond {
            "eq" => z,
            "hi" => {
                let nz = push(Op::BNot(z));
                push(Op::BBin(BitOp::And, cf, nz))
            }
            "gt" => {
                let nv = push(Op::BBin(BitOp::Xor, n, v));
                let o = push(Op::BBin(BitOp::Or, z, nv));
                push(Op::BNot(o))
            }
            _ => unreachable!(),
        }
    }

    fn exit(insts: &mut Vec<Inst>, to: u32, guard: Option<ValueId>) {
        insts.push(i(Op::Const(to)));
        let t = ValueId(insts.len() as u32 - 1);
        insts.push(Inst {
            op: Op::Exit(Exit::Jump(t)),
            guard,
        });
    }

    /// IntroHandler's shape: beq 1, bgt 1, beq 0, else.
    fn intro() -> Vec<Inst> {
        let mut s = vec![i(Op::GetReg(Reg(0)))];
        let x = ValueId(0);
        for (k, c) in [(1, "eq"), (1, "gt"), (0, "eq")] {
            let g = cmp(&mut s, x, k, c);
            exit(&mut s, 0x100 + k, Some(g));
        }
        exit(&mut s, 0x200, None);
        s
    }

    #[test]
    fn a_binary_search_splits_the_values() {
        let s = intro();
        let r = run(&s, 2).unwrap();
        assert_eq!(r.tests.len(), 3);
        assert_eq!(r.value, ValueId(0));
        assert_eq!(r.end, End::Exit(s.len() - 1));
        let t = Tree {
            next: vec![None; 4],
            run: r,
        };
        let a = arms(&t).unwrap();
        let p: Vec<String> = a.iter().map(|r| pattern(r)).collect();
        assert_eq!(p, ["1", "2..=0x7fffffff", "0", "0x80000000..=0xffffffff"]);
    }

    #[test]
    fn a_nested_run_takes_its_parents_values() {
        let s = intro();
        let mut inner = vec![i(Op::GetReg(Reg(0)))];
        for k in [2, 3] {
            let g = cmp(&mut inner, ValueId(0), k, "eq");
            exit(&mut inner, 0x300 + k, Some(g));
        }
        exit(&mut inner, 0x400, None);
        let inner = Tree {
            next: vec![None; 3],
            run: run(&inner, 2).unwrap(),
        };
        let t = Tree {
            next: vec![None, Some(inner), None, None],
            run: run(&s, 2).unwrap(),
        };
        let p: Vec<String> = arms(&t).unwrap().iter().map(|r| pattern(r)).collect();
        assert_eq!(
            p,
            [
                "1",
                "2",
                "3",
                "4..=0x7fffffff",
                "0",
                "0x80000000..=0xffffffff"
            ]
        );
    }

    #[test]
    fn an_unreachable_leaf_is_no_tree() {
        let mut s = vec![i(Op::GetReg(Reg(0)))];
        for (k, c) in [(5, "hi"), (7, "eq")] {
            let g = cmp(&mut s, ValueId(0), k, c);
            exit(&mut s, 0x100 + k, Some(g));
        }
        exit(&mut s, 0x200, None);
        let t = Tree {
            next: vec![None; 3],
            run: run(&s, 2).unwrap(),
        };
        assert_eq!(arms(&t), None);
    }

    #[test]
    fn a_store_between_ends_the_run() {
        let mut s = vec![i(Op::GetReg(Reg(0)))];
        let g = cmp(&mut s, ValueId(0), 1, "eq");
        exit(&mut s, 0x101, Some(g));
        s.push(i(Op::Store {
            width: armv4t::sem::Width::Word,
            addr: ValueId(0),
            value: ValueId(0),
        }));
        let g = cmp(&mut s, ValueId(0), 2, "eq");
        exit(&mut s, 0x102, Some(g));
        exit(&mut s, 0x200, None);
        assert_eq!(run(&s, 2), Err(2));
    }

    #[test]
    fn a_run_may_end_at_its_last_test() {
        // gcc's: the last test's fall is the case's code.
        let mut s = vec![i(Op::GetReg(Reg(0)))];
        for k in [1, 5] {
            let g = cmp(&mut s, ValueId(0), k, "eq");
            exit(&mut s, 0x100 + k, Some(g));
        }
        let rest = s.len();
        s.push(i(Op::Store {
            width: armv4t::sem::Width::Word,
            addr: ValueId(0),
            value: ValueId(0),
        }));
        exit(&mut s, 0x200, None);
        let r = run(&s, 2).unwrap();
        assert_eq!(r.end, End::Rest(rest));
        assert_eq!(r.at(), rest - 1);
        assert_eq!(r.exits().count(), 2);
    }

    #[test]
    fn the_flags_follow_add_with_carry() {
        // cmp x, #3, "gt" (signed): taken for 4..=i32::MAX only.
        let mut s = vec![i(Op::GetReg(Reg(0)))];
        let g = cmp(&mut s, ValueId(0), 3, "gt");
        let (_, t) = classify(&s, g).unwrap();
        for (x, want) in [
            (3, false),
            (4, true),
            (0x7fff_ffff, true),
            (0x8000_0000, false),
            (0, false),
        ] {
            assert_eq!(t.taken(x), want, "{x:#x}");
        }
    }
}
