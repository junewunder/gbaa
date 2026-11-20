//! Bitfield accesses as their fields (unit `native/lift-bitfields`; the
//! mod view's `set_bits`, `docs/mod-view-parity.md`).
//!
//! C's `b->left = x` of a bitfield `left` compiles to a load of the
//! bytes that hold it, its bits cleared and the new ones or-ed in, and a
//! store back; `b->speed` to a load and a mask or a pair of shifts. The
//! access is the bitfields' accessor ([`crate::types::Types::bits_at`]),
//! which several fields share, so [`super::fields`] names it only when
//! the code says which one: by the bits it changes or reads.
//!
//! - **A store** names the field whose bits are all it changes of what a
//!   load of the same address in the block read ([`kept`]: the bits of
//!   the value stored known to be that load's). A store that changes two
//!   fields' bits at once (a compiler joins `b->left = 0; b->hue = 1;`)
//!   names none.
//! - **A load** names the field whose bits are all its uses read: a mask,
//!   a test of a bit, a shift right then a mask, a shift left then right
//!   (the extracting idioms), or the keeping half of a store above that
//!   names that field. A load that feeds anything else (a register
//!   another block reads) names none.
//! - **A fold.** A named store whose value is `(old & k) | e`, `old & k`
//!   or `old | c`, where `k` clears the whole field and `e` (`c`) is
//!   proven to lie in it, `old` the block's load of the same address used
//!   by nothing else, with no store or exit between: it prints as one
//!   `g.set_bits(p, v)`, the load and the masking left out. `set_bits`
//!   loads and stores the same bytes; `v` is what `e` puts in the field.
//!
//! The naming is a label, as [`super::fields`]' is: the printed access is
//! the same `ld*`/`st*`. Where a store's `e` isn't proven to lie in the
//! field, its name trusts the compiler's mask (`(old & k) | e` writes the
//! bits `k` clears); only the fold needs the proof, since `set_bits`
//! masks `v`.

use std::collections::{BTreeMap, BTreeSet};

use analysis::ir::{BinOp, Inst, Op, ValueId};
use armv4t::sem::{ShiftType, Width};

/// The value a [`Fold`] puts in its field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldValue {
    Const(u32),
    /// A value of the block, shifted right `shift` bits.
    Of(ValueId, u32),
}

/// A load and store printed as one `g.set_bits(..)` at the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    /// The load it takes the place of, which isn't printed.
    pub load: usize,
    pub v: FoldValue,
}

/// A block's bitfield accesses.
#[derive(Clone, Debug, Default)]
pub struct Found {
    /// The bits of the bitfield each access names, by inst.
    pub bits: BTreeMap<usize, (u32, u32)>,
    /// The stores printed as `set_bits`, by inst.
    pub folds: BTreeMap<usize, Fold>,
}

/// The bitfield accesses of a block's `insts`: `fields(i)` gives the
/// bitfields access `i` holds (empty for one that holds none), `unread(i)`
/// whether inst `i` is a write nothing reads (left out of the print).
pub fn find(
    insts: &[Inst],
    fields: impl Fn(usize) -> Vec<(u32, u32)>,
    unread: impl Fn(usize) -> bool,
) -> Found {
    let n = insts.len();
    // What the printed code reads: a write it leaves out (`unread`) and
    // a pure value only those read (a dead flag's test) read nothing.
    let mut live = vec![false; n];
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in (0..n).rev() {
        let inst = &insts[i];
        live[i] = if super::pure(&inst.op) {
            !users[i].is_empty()
        } else {
            !unread(i)
        };
        if !live[i] {
            continue;
        }
        for x in inst.op.operands().into_iter().chain(inst.guard) {
            users[x.index()].push(i);
        }
    }
    for u in &mut users {
        u.reverse();
    }
    let keys = addr_keys(insts);
    let mut out = Found::default();
    let all: Vec<Vec<(u32, u32)>> = (0..n).map(&fields).collect();
    // Each named store's load and the values between them.
    let mut chains: BTreeMap<usize, (usize, (u32, u32), BTreeSet<usize>)> = BTreeMap::new();
    for s in 0..n {
        let Op::Store { width, addr, value } = insts[s].op else {
            continue;
        };
        if all[s].is_empty() {
            continue;
        }
        let w = mask_of(width);
        let tree = tree(insts, value.index());
        // The load of the same address the value keeps bits of.
        let Some(l) = tree.iter().rev().copied().find(|&l| {
            matches!(insts[l].op, Op::Load { width: lw, addr: la }
                if lw == width && keys[la.index()] == keys[addr.index()])
        }) else {
            continue;
        };
        let changed = !kept(insts, value.index(), l, true) & w;
        let Some(&f) = all[s]
            .iter()
            .find(|&&(lo, k)| changed != 0 && changed & !run(lo, k) == 0)
        else {
            continue;
        };
        out.bits.insert(s, f);
        let between: BTreeSet<usize> = tree
            .iter()
            .copied()
            .filter(|&x| x == l || reaches(insts, x, l))
            .collect();
        if let Some(fold) = fold(insts, &users, s, l, f, w, &between) {
            out.folds.insert(s, fold);
        }
        chains.insert(s, (l, f, between));
    }
    for l in 0..n {
        let Op::Load { width, .. } = insts[l].op else {
            continue;
        };
        if all[l].is_empty() || out.folds.values().any(|f| f.load == l) {
            if let Some((_, (_, f, _))) = chains.iter().find(|(_, (cl, _, _))| *cl == l) {
                out.bits.insert(l, *f);
            }
            continue;
        }
        let w = mask_of(width);
        let mut read = 0u32;
        let mut ok = !users[l].is_empty();
        // The field a store this load keeps the rest of names.
        let mut via: Option<(u32, u32)> = None;
        for &u in &users[l] {
            if let Some((_, (_, f, _))) = chains
                .iter()
                .find(|(_, (cl, _, between))| *cl == l && between.contains(&u))
            {
                match via {
                    Some(g) if g != *f => ok = false,
                    _ => via = Some(*f),
                }
                continue;
            }
            match picked(insts, &users, l, u) {
                Some(m) => read |= m & w,
                None => ok = false,
            }
        }
        if !ok {
            continue;
        }
        let f = match via {
            Some(f) => (read & !run(f.0, f.1) == 0).then_some(f),
            None => all[l]
                .iter()
                .copied()
                .find(|&(lo, k)| read != 0 && read & !run(lo, k) == 0),
        };
        if let Some(f) = f {
            out.bits.insert(l, f);
        }
    }
    out
}

/// The bits of a load's value its use `u` reads: a mask, a bit test, a
/// shift right (then a mask), a shift left then right. `None`: all of
/// them, or more than the value.
fn picked(insts: &[Inst], users: &[Vec<usize>], l: usize, u: usize) -> Option<u32> {
    let k = |x: ValueId| konst(insts, x);
    match insts[u].op {
        Op::Bin(BinOp::And, a, b) => {
            let other = if a.index() == l { b } else { a };
            k(other)
        }
        Op::TestBit { n, .. } => Some(1 << n),
        Op::Shift {
            kind: ShiftType::Lsr,
            v,
            amount,
        } if v.index() == l => {
            let s = k(amount).filter(|&s| s > 0 && s < 32)?;
            // Then masked, by every use.
            let masks: Option<Vec<u32>> = users[u]
                .iter()
                .map(|&j| match insts[j].op {
                    Op::Bin(BinOp::And, a, b) => k(if a.index() == u { b } else { a }),
                    _ => None,
                })
                .collect();
            match masks {
                Some(m) if !m.is_empty() => Some(m.iter().fold(0, |x, y| x | y) << s),
                _ => Some(u32::MAX << s),
            }
        }
        Op::Shift {
            kind: ShiftType::Lsl,
            v,
            amount,
        } if v.index() == l => {
            let a = k(amount).filter(|&a| a < 32)?;
            let rights: Option<Vec<u32>> = users[u]
                .iter()
                .map(|&j| match insts[j].op {
                    Op::Shift {
                        kind: ShiftType::Lsr | ShiftType::Asr,
                        v,
                        amount,
                    } if v.index() == u => k(amount).filter(|&b| b >= a && b < 32),
                    _ => None,
                })
                .collect();
            // Bits b-a up, below 32-a.
            let below = u32::MAX >> a;
            match rights {
                Some(r) if !r.is_empty() => {
                    let b = r.into_iter().min().expect("one");
                    Some(below & (u32::MAX << (b - a)))
                }
                _ => Some(below),
            }
        }
        _ => None,
    }
}

/// Whether store `s` (old value load `l`, field `f`) folds into
/// `set_bits`, and what it puts in the field.
fn fold(
    insts: &[Inst],
    users: &[Vec<usize>],
    s: usize,
    l: usize,
    f: (u32, u32),
    w: u32,
    between: &BTreeSet<usize>,
) -> Option<Fold> {
    let Op::Store { value, .. } = insts[s].op else {
        return None;
    };
    let m = run(f.0, f.1);
    // Nothing between the load and the store may write memory or leave.
    if (l + 1..s).any(|i| {
        matches!(
            insts[i].op,
            Op::Store { .. } | Op::Exit(_) | Op::SetCpsr { .. } | Op::SetSpsr { .. }
        )
    }) || l > s
    {
        return None;
    }
    // All under the store's condition (an ARM `strbne`'s), if any.
    if between.iter().any(|&x| insts[x].guard != insts[s].guard) {
        return None;
    }
    // The load and the values from it to the store used by nothing else.
    if between
        .iter()
        .any(|&x| users[x].iter().any(|u| *u != s && !between.contains(u)))
    {
        return None;
    }
    // Proven: what the store changes is the whole field and no more.
    if !kept(insts, value.index(), l, false) & w != m {
        return None;
    }
    let k = |x: ValueId| konst(insts, x);
    let is_old = |x: ValueId| between.contains(&x.index());
    // `old & k`, either way round.
    let cleared = |x: ValueId| match insts[x.index()].op {
        Op::Bin(BinOp::And, a, b) => (is_old(a) && k(b).is_some()) || (is_old(b) && k(a).is_some()),
        _ => false,
    };
    let v = match insts[value.index()].op {
        _ if cleared(value) => FoldValue::Const(0),
        Op::Bin(BinOp::Or, a, b) => {
            let (old, e) = if is_old(a) { (a, b) } else { (b, a) };
            if !is_old(old) || is_old(e) {
                return None;
            }
            match k(e) {
                Some(c) => FoldValue::Const((c & w) >> f.0),
                None if cleared(old) => value_of(insts, e, f),
                None => return None,
            }
        }
        _ => return None,
    };
    Some(Fold { load: l, v })
}

/// What `e` (its bits all in the field `f`) puts in the field: `y` of
/// `y << lo` or `(y & m) << lo` with `m` keeping the field's width, else
/// `e >> lo`.
fn value_of(insts: &[Inst], e: ValueId, f: (u32, u32)) -> FoldValue {
    let k = |x: ValueId| konst(insts, x);
    let low = run(0, f.1);
    let y = match insts[e.index()].op {
        Op::Shift {
            kind: ShiftType::Lsl,
            v,
            amount,
        } if k(amount) == Some(f.0) => v,
        _ if f.0 == 0 => e,
        _ => return FoldValue::Of(e, f.0),
    };
    match insts[y.index()].op {
        Op::Bin(BinOp::And, a, b) if k(b).is_some_and(|m| m & low == low) => FoldValue::Of(a, 0),
        Op::Bin(BinOp::And, a, b) if k(a).is_some_and(|m| m & low == low) => FoldValue::Of(b, 0),
        _ => FoldValue::Of(y, 0),
    }
}

/// The bits of value `x` known to be those load `l` read. `trust`: in
/// `(a & k) | b`, `b` has no bits `k` keeps (the compiler's mask says so;
/// else only what [`zeros`] proves).
pub fn kept(insts: &[Inst], x: usize, l: usize, trust: bool) -> u32 {
    if x == l {
        return u32::MAX;
    }
    let k = |v: ValueId| konst(insts, v);
    let kk = |v: ValueId| kept(insts, v.index(), l, trust);
    let z = |v: ValueId| zeros(insts, v.index());
    let ones = |v: ValueId| k(v).unwrap_or(0);
    match insts[x].op {
        Op::Bin(BinOp::And, a, b) => (kk(a) & (ones(b) | kk(b))) | (kk(b) & ones(a)),
        Op::Bin(BinOp::Or, a, b) => {
            // `(y & k) | e`: `e` lies in the bits `k` clears.
            let mask = |v: ValueId| match insts[v.index()].op {
                Op::Bin(BinOp::And, p, q) if trust => k(q).or_else(|| k(p)).unwrap_or(0),
                _ => 0,
            };
            let (za, zb) = (z(a) | mask(b), z(b) | mask(a));
            (kk(a) & (zb | kk(b))) | (kk(b) & za)
        }
        Op::Bin(BinOp::Xor, a, b) => (kk(a) & z(b)) | (kk(b) & z(a)),
        Op::Bin(BinOp::Add, a, b) if k(b) == Some(0) => kk(a),
        Op::Bin(BinOp::Add, a, b) if k(a) == Some(0) => kk(b),
        Op::Adc { a, b, c } if k(b) == Some(0) && bit(insts, c) == Some(false) => kk(a),
        _ => 0,
    }
}

/// The bits of value `x` known to be 0.
pub fn zeros(insts: &[Inst], x: usize) -> u32 {
    let z = |v: ValueId| zeros(insts, v.index());
    let k = |v: ValueId| konst(insts, v);
    if let Some(c) = konst(insts, ValueId(x as u32)) {
        return !c;
    }
    match insts[x].op {
        Op::Load { width, .. } => !mask_of(width),
        Op::Bin(BinOp::And, a, b) => z(a) | z(b),
        Op::Bin(BinOp::Or | BinOp::Xor, a, b) => z(a) & z(b),
        // A carry out of the highest bit either may set.
        Op::Bin(BinOp::Add, a, b) | Op::Adc { a, b, .. } => {
            let top = !z(a) | !z(b);
            match top.leading_zeros() {
                0 => 0,
                n => !(u32::MAX >> (n - 1)),
            }
        }
        Op::Shift { kind, v, amount } => match (kind, k(amount)) {
            (ShiftType::Lsl, Some(s)) if s < 32 => (z(v) << s) | !(u32::MAX << s),
            (ShiftType::Lsr, Some(s)) if s < 32 => (z(v) >> s) | !(u32::MAX >> s),
            (ShiftType::Lsl | ShiftType::Lsr, Some(_)) => u32::MAX,
            _ => 0,
        },
        Op::BitToWord(_) => !1,
        _ => 0,
    }
}

/// Inst `x` and the block's values it is computed from, through pure
/// ops.
fn tree(insts: &[Inst], x: usize) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![x];
    while let Some(i) = todo.pop() {
        if !seen.insert(i) {
            continue;
        }
        if matches!(insts[i].op, Op::Load { .. }) {
            continue;
        }
        todo.extend(insts[i].op.operands().into_iter().map(|v| v.index()));
    }
    seen.into_iter().collect()
}

/// Whether value `x` is computed from inst `l`.
fn reaches(insts: &[Inst], x: usize, l: usize) -> bool {
    x == l || tree(insts, x).contains(&l)
}

/// Each value's address key: the same for two values the block computes
/// as the same base plus the same constant (a register read twice with no
/// write between is one base).
fn addr_keys(insts: &[Inst]) -> Vec<(usize, u32)> {
    let mut keys: Vec<(usize, u32)> = Vec::with_capacity(insts.len());
    let mut reg_read: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, inst) in insts.iter().enumerate() {
        let key = match inst.op {
            Op::GetReg(r) => (*reg_read.entry(r.index()).or_insert(i), 0),
            Op::SetReg(r, _) => {
                reg_read.remove(&r.index());
                (i, 0)
            }
            Op::Bin(BinOp::Add, a, b) => match (konst(insts, a), konst(insts, b)) {
                (None, Some(c)) => (keys[a.index()].0, keys[a.index()].1.wrapping_add(c)),
                (Some(c), None) => (keys[b.index()].0, keys[b.index()].1.wrapping_add(c)),
                _ => (i, 0),
            },
            Op::Adc { a, b, c } if bit(insts, c) == Some(false) => {
                match (konst(insts, a), konst(insts, b)) {
                    (None, Some(k)) => (keys[a.index()].0, keys[a.index()].1.wrapping_add(k)),
                    (Some(k), None) => (keys[b.index()].0, keys[b.index()].1.wrapping_add(k)),
                    _ => (i, 0),
                }
            }
            _ => (i, 0),
        };
        keys.push(key);
    }
    keys
}

fn konst(insts: &[Inst], x: ValueId) -> Option<u32> {
    match insts[x.index()].op {
        Op::Const(c) | Op::Pool(c) => Some(c),
        Op::Not(a) => konst(insts, a).map(|c| !c),
        _ => None,
    }
}

fn bit(insts: &[Inst], x: ValueId) -> Option<bool> {
    match insts[x.index()].op {
        Op::Bit(b) => Some(b),
        _ => None,
    }
}

fn mask_of(w: Width) -> u32 {
    match w {
        Width::Byte => 0xff,
        Width::Half => 0xffff,
        Width::Word => u32::MAX,
    }
}

/// `n` bits from bit `lo`.
pub fn run(lo: u32, n: u32) -> u32 {
    (u32::MAX >> (32 - n)) << lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use armv4t::sem::Reg;

    /// A block built one op at a time, each its index's value.
    #[derive(Default)]
    struct B(Vec<Inst>);

    impl B {
        fn op(&mut self, op: Op) -> ValueId {
            self.0.push(Inst { op, guard: None });
            ValueId(self.0.len() as u32 - 1)
        }
        fn k(&mut self, c: u32) -> ValueId {
            self.op(Op::Const(c))
        }
        fn bin(&mut self, o: BinOp, a: ValueId, b: ValueId) -> ValueId {
            self.op(Op::Bin(o, a, b))
        }
        fn shift(&mut self, kind: ShiftType, v: ValueId, s: u32) -> ValueId {
            let amount = self.k(s);
            self.op(Op::Shift { kind, v, amount })
        }
        /// `r0 + 4`, read anew.
        fn addr(&mut self) -> ValueId {
            let r = self.op(Op::GetReg(Reg(0)));
            let c = self.k(4);
            self.bin(BinOp::Add, r, c)
        }
        fn load(&mut self) -> ValueId {
            let addr = self.addr();
            self.op(Op::Load {
                width: Width::Byte,
                addr,
            })
        }
        fn store(&mut self, value: ValueId) -> ValueId {
            let addr = self.addr();
            self.op(Op::Store {
                width: Width::Byte,
                addr,
                value,
            })
        }
    }

    /// `struct { u8 left : 1, up : 1, speed : 2, hue : 4; }`'s byte.
    const BALL: [(u32, u32); 4] = [(0, 1), (1, 1), (2, 2), (4, 4)];

    fn found(b: &B, unread: &[usize]) -> Found {
        find(
            &b.0,
            |i| match b.0[i].op {
                Op::Load { .. } | Op::Store { .. } => BALL.to_vec(),
                _ => Vec::new(),
            },
            |i| unread.contains(&i),
        )
    }

    #[test]
    fn a_write_of_one_field_folds_into_set_bits() {
        // b->left = r1 & 1: (old & ~1) | (r1 & 1).
        let mut b = B::default();
        let old = b.load();
        let k = b.k(0xffff_fffe);
        let kept = b.bin(BinOp::And, k, old);
        let r1 = b.op(Op::GetReg(Reg(1)));
        let one = b.k(1);
        let v = b.bin(BinOp::And, r1, one);
        let new = b.bin(BinOp::Or, kept, v);
        let s = b.store(new);
        let f = found(&b, &[]);
        assert_eq!(f.bits.get(&s.index()), Some(&(0, 1)));
        assert_eq!(
            f.folds.get(&s.index()),
            Some(&Fold {
                load: old.index(),
                v: FoldValue::Of(r1, 0)
            })
        );
        // b->hue = 15: `old | 0xf0`, all of hue set.
        let mut b = B::default();
        let old = b.load();
        let c = b.k(0xf0);
        let new = b.bin(BinOp::Or, old, c);
        let s = b.store(new);
        let f = found(&b, &[]);
        assert_eq!(f.folds[&s.index()].v, FoldValue::Const(0xf));
        // b->hue = y: (old & 0xf) | (y << 4).
        let mut b = B::default();
        let old = b.load();
        let k = b.k(0xf);
        let kept = b.bin(BinOp::And, old, k);
        let y = b.op(Op::GetReg(Reg(2)));
        let fifteen = b.k(0xf);
        let ym = b.bin(BinOp::And, y, fifteen);
        let e = b.shift(ShiftType::Lsl, ym, 4);
        let new = b.bin(BinOp::Or, kept, e);
        let s = b.store(new);
        let f = found(&b, &[]);
        assert_eq!(f.bits.get(&s.index()), Some(&(4, 4)));
        assert_eq!(f.folds[&s.index()].v, FoldValue::Of(y, 0));
    }

    #[test]
    fn a_write_the_mask_names_but_nothing_proves_is_named_not_folded() {
        // b->speed = y: (old & ~0xc) | (y << 2), y's bits unknown.
        let mut b = B::default();
        let old = b.load();
        let k = b.k(0xffff_fff3);
        let kept = b.bin(BinOp::And, old, k);
        let y = b.op(Op::GetReg(Reg(2)));
        let e = b.shift(ShiftType::Lsl, y, 2);
        let new = b.bin(BinOp::Or, kept, e);
        let s = b.store(new);
        let f = found(&b, &[]);
        assert_eq!(f.bits.get(&s.index()), Some(&(2, 2)));
        assert_eq!(f.bits.get(&old.index()), Some(&(2, 2)));
        assert!(f.folds.is_empty());
    }

    #[test]
    fn a_toggle_and_a_load_another_reads_are_named_not_folded() {
        // b->up = !b->up: (old & ~2) | (((old >> 1) ^ 1) & 1) << 1.
        let mut b = B::default();
        let old = b.load();
        let k = b.k(0xffff_fffd);
        let kept = b.bin(BinOp::And, old, k);
        let up = b.shift(ShiftType::Lsr, old, 1);
        let one = b.k(1);
        let not = b.bin(BinOp::Xor, up, one);
        let bit = b.bin(BinOp::And, not, one);
        let e = b.shift(ShiftType::Lsl, bit, 1);
        let new = b.bin(BinOp::Or, kept, e);
        let s = b.store(new);
        let f = found(&b, &[]);
        assert_eq!(f.bits.get(&s.index()), Some(&(1, 1)));
        assert_eq!(f.bits.get(&old.index()), Some(&(1, 1)));
        assert!(f.folds.is_empty(), "the value is the old one's");
        // A fold's load a register keeps for later: named, not folded,
        // unless the register's write is left out.
        let mut b = B::default();
        let old = b.load();
        let set = b.op(Op::SetReg(Reg(3), old));
        let k = b.k(0xffff_fffd);
        let new = b.bin(BinOp::And, old, k);
        let s = b.store(new);
        assert!(found(&b, &[]).folds.is_empty());
        assert_eq!(
            found(&b, &[set.index()]).folds[&s.index()].v,
            FoldValue::Const(0)
        );
    }

    #[test]
    fn joined_accesses_name_no_field() {
        // left and hue in one store: (old & 0xe) | e.
        let mut b = B::default();
        let old = b.load();
        let k = b.k(0xe);
        let kept = b.bin(BinOp::And, old, k);
        let y = b.op(Op::GetReg(Reg(2)));
        let new = b.bin(BinOp::Or, kept, y);
        let s = b.store(new);
        let f = found(&b, &[]);
        assert!(!f.bits.contains_key(&s.index()));
        // One load read for speed (shifts) and left (a mask): no field;
        // for speed alone, speed.
        let mut b = B::default();
        let old = b.load();
        let up = b.shift(ShiftType::Lsl, old, 28);
        let speed = b.shift(ShiftType::Lsr, up, 30);
        let one = b.k(1);
        let left = b.bin(BinOp::And, one, old);
        b.op(Op::SetReg(Reg(1), speed));
        let set = b.op(Op::SetReg(Reg(2), left));
        assert!(!found(&b, &[]).bits.contains_key(&old.index()));
        assert_eq!(
            found(&b, &[set.index()]).bits.get(&old.index()),
            Some(&(2, 2))
        );
    }
}
