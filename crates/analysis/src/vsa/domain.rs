//! The abstract values: what one register or stack slot can hold.
//!
//! A value has two parts:
//!
//! - its *set* ([`Dom`]): unknown, a constant, up to [`MAX_SET`] constants,
//!   or a strided interval;
//! - its *relation* (`sym + off`): every value names an unknown it equals
//!   plus a constant. A copy keeps the name, and so does adding a constant,
//!   so after `subs r0, r5, #1` both r5 and r0 are known to be offsets of
//!   the same unknown. A flag computed from a value is a [`Bit::Pred`] over
//!   that unknown, so a conditional branch can narrow every register
//!   holding it.

use std::sync::Arc;

/// The most constants a [`Dom::Set`] holds before it becomes an interval.
pub const MAX_SET: usize = 1024;
/// The most elements an operation enumerates (a binary op on two sets, a
/// load from a set of addresses).
pub const MAX_ENUM: u64 = 4096;

/// The name of an unknown value. Names are derived from where the value was
/// made (its kind, an address, a salt and an index), so re-running the same
/// instruction makes the same names and the fixpoint can settle.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Sym(pub u64);

// The kinds of names.
/// A register on entry to the function at `addr`.
pub const ENTRY: u8 = 1;
/// The result of operation `n` of the instruction at `addr`.
pub const OP: u8 = 2;
/// A value made by the join at `addr`.
pub const JOIN: u8 = 3;
/// A register clobbered by the call at `addr`.
pub const CLOBBER: u8 = 4;
/// A value made by the join after a condition code at `addr`.
pub const WHEN: u8 = 5;
/// The constant `addr`.
pub const CONST: u8 = 6;
/// An older value that a re-run of the instruction at `addr` would have
/// named the same as its new one (see [`crate::vsa::exec::State::retire`]).
pub const STALE: u8 = 7;

impl Sym {
    pub fn new(kind: u8, addr: u32, salt: u32, n: u32) -> Sym {
        let salt = (salt.wrapping_mul(0x9e37_79b1) >> 16) as u64;
        Sym((kind as u64 & 0xf) << 60 | (n as u64 & 0xfff) << 48 | salt << 32 | addr as u64)
    }

    pub fn kind(self) -> u8 {
        (self.0 >> 60) as u8
    }

    /// The `n` it was made with (for [`ENTRY`], the register).
    pub fn index(self) -> u32 {
        (self.0 >> 48) as u32 & 0xfff
    }

    /// Whether this names a value the instruction at `addr`, run with
    /// `salt`, makes.
    pub fn made_by(self, addr: u32, salt: u32) -> bool {
        matches!(self.kind(), OP | CLOBBER | WHEN)
            && self.0 & 0xffff_ffff_ffff == Sym::new(0, addr, salt, 0).0
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A set of 32-bit values.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Dom {
    Top,
    Const(u32),
    /// At least two and at most [`MAX_SET`] values, sorted.
    Set(Arc<[u32]>),
    /// `lo, lo + s, ..., hi`, with `s > 0`, `lo < hi`, `s` dividing
    /// `hi - lo`, and not every value.
    Si {
        s: u32,
        lo: u32,
        hi: u32,
    },
}

impl Dom {
    /// The set of `v`, which must not be empty.
    pub fn from_vals(mut v: Vec<u32>) -> Dom {
        v.sort_unstable();
        v.dedup();
        match v.len() {
            0 => panic!("empty value set"),
            1 => Dom::Const(v[0]),
            n if n <= MAX_SET => Dom::Set(v.into()),
            _ => Dom::hull(&v),
        }
    }

    /// The smallest strided interval holding the sorted values `v`.
    fn hull(v: &[u32]) -> Dom {
        let lo = v[0];
        let s = v.iter().fold(0, |g, x| gcd(g, x - lo));
        Dom::si(s, lo, *v.last().unwrap())
    }

    /// `lo..=hi` step `s`, normalised.
    pub fn si(s: u32, lo: u32, hi: u32) -> Dom {
        if lo == hi {
            Dom::Const(lo)
        } else if s <= 1 && lo == 0 && hi == u32::MAX {
            Dom::Top
        } else {
            Dom::Si {
                s: s.max(1),
                lo,
                hi,
            }
        }
    }

    pub fn konst(&self) -> Option<u32> {
        match self {
            Dom::Const(k) => Some(*k),
            _ => None,
        }
    }

    pub fn count(&self) -> u64 {
        match self {
            Dom::Top => 1 << 32,
            Dom::Const(_) => 1,
            Dom::Set(v) => v.len() as u64,
            Dom::Si { s, lo, hi } => ((hi - lo) / s) as u64 + 1,
        }
    }

    /// The values, if there are at most [`MAX_ENUM`] of them.
    pub fn elems(&self) -> Option<Vec<u32>> {
        if self.count() > MAX_ENUM {
            return None;
        }
        Some(match self {
            Dom::Top => unreachable!(),
            Dom::Const(k) => vec![*k],
            Dom::Set(v) => v.to_vec(),
            Dom::Si { s, lo, .. } => (0..self.count() as u32).map(|k| lo + k * s).collect(),
        })
    }

    pub fn max(&self) -> u32 {
        match self {
            Dom::Top => u32::MAX,
            Dom::Const(k) => *k,
            Dom::Set(v) => *v.last().unwrap(),
            Dom::Si { hi, .. } => *hi,
        }
    }

    /// Every value plus `k`, wrapping.
    pub fn add_const(&self, k: u32) -> Dom {
        match self {
            Dom::Top => Dom::Top,
            Dom::Const(x) => Dom::Const(x.wrapping_add(k)),
            Dom::Set(v) => Dom::from_vals(v.iter().map(|x| x.wrapping_add(k)).collect()),
            &Dom::Si { s, lo, hi } => {
                let (l, h) = (lo.wrapping_add(k), hi.wrapping_add(k));
                // Only if the whole interval wraps or none of it does.
                if (l < lo) == (h < hi) {
                    Dom::si(s, l, h)
                } else {
                    Dom::Top
                }
            }
        }
    }

    /// The union, over-approximated.
    pub fn join(&self, other: &Dom) -> Dom {
        if self == other {
            return self.clone();
        }
        if let (Some(mut x), Some(y)) = (self.elems(), other.elems()) {
            x.extend(y);
            return Dom::from_vals(x);
        }
        match (self.bounds(), other.bounds()) {
            (Some((s1, l1, h1)), Some((s2, l2, h2))) => {
                let lo = l1.min(l2);
                let s = gcd(gcd(s1, s2), l1.abs_diff(l2));
                Dom::si(s, lo, h1.max(h2))
            }
            _ => Dom::Top,
        }
    }

    /// Stride, low and high bound of a non-[`Top`](Dom::Top) set.
    fn bounds(&self) -> Option<(u32, u32, u32)> {
        match self {
            Dom::Top => None,
            Dom::Const(k) => Some((0, *k, *k)),
            Dom::Set(v) => {
                let lo = v[0];
                Some((
                    v.iter().fold(0, |g, x| gcd(g, x - lo)),
                    lo,
                    *v.last().unwrap(),
                ))
            }
            &Dom::Si { s, lo, hi } => Some((s, lo, hi)),
        }
    }

    /// The values that are also in `r`; `None` if there are none.
    pub fn meet(&self, r: &Ranges) -> Option<Dom> {
        if r.is_full() {
            return Some(self.clone());
        }
        match self {
            Dom::Top => {
                let (lo, hi) = (r.0.first()?.0, r.0.last()?.1);
                Some(Dom::si(1, lo, hi))
            }
            Dom::Const(k) => r.contains(*k).then(|| self.clone()),
            Dom::Set(v) => {
                let kept: Vec<u32> = v.iter().copied().filter(|x| r.contains(*x)).collect();
                (!kept.is_empty()).then(|| Dom::from_vals(kept))
            }
            &Dom::Si { s, lo, hi } => {
                let mut out: Option<Dom> = None;
                for &(a, b) in &r.0 {
                    if b < lo || a > hi {
                        continue;
                    }
                    // The first and last members of the interval in a..=b.
                    let (lo64, s64) = (lo as u64, s as u64);
                    let nlo = if a <= lo {
                        lo64
                    } else {
                        lo64 + (a as u64 - lo64).div_ceil(s64) * s64
                    };
                    let nhi = if b >= hi {
                        hi as u64
                    } else {
                        lo64 + (b as u64 - lo64) / s64 * s64
                    };
                    if nlo > nhi {
                        continue;
                    }
                    let piece = Dom::si(s, nlo as u32, nhi as u32);
                    out = Some(match out {
                        None => piece,
                        Some(o) => o.join(&piece),
                    });
                }
                out
            }
        }
    }

    pub fn contains(&self, x: u32) -> bool {
        match self {
            Dom::Top => true,
            Dom::Const(k) => *k == x,
            Dom::Set(v) => v.binary_search(&x).is_ok(),
            &Dom::Si { s, lo, hi } => (lo..=hi).contains(&x) && (x - lo).is_multiple_of(s),
        }
    }

    /// The values in both, over-approximated; `None` if there are none.
    pub fn intersect(&self, o: &Dom) -> Option<Dom> {
        match (self, o) {
            (Dom::Top, d) | (d, Dom::Top) => Some(d.clone()),
            (d @ (Dom::Const(_) | Dom::Set(_)), e) | (e, d @ (Dom::Const(_) | Dom::Set(_))) => {
                let kept: Vec<u32> = d.elems()?.into_iter().filter(|&x| e.contains(x)).collect();
                (!kept.is_empty()).then(|| Dom::from_vals(kept))
            }
            (&Dom::Si { lo: l1, hi: h1, .. }, &Dom::Si { lo: l2, hi: h2, .. }) => {
                if l1.max(l2) > h1.min(h2) {
                    return None;
                }
                // Either interval is a sound answer; keep the smaller.
                Some(if self.count() <= o.count() {
                    self.clone()
                } else {
                    o.clone()
                })
            }
        }
    }

    /// Whether every value is in `r`.
    pub fn within(&self, r: &Ranges) -> bool {
        self.meet(&r.complement()).is_none()
    }

    /// Whether no value is in `r`.
    pub fn disjoint(&self, r: &Ranges) -> bool {
        self.meet(r).is_none()
    }
}

/// A set of `u32`s as sorted, disjoint, non-adjacent inclusive ranges.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ranges(Vec<(u32, u32)>);

impl Ranges {
    pub fn empty() -> Ranges {
        Ranges(Vec::new())
    }

    pub fn full() -> Ranges {
        Ranges(vec![(0, u32::MAX)])
    }

    /// `lo..=hi`, or empty if `lo > hi`.
    pub fn new(lo: u32, hi: u32) -> Ranges {
        if lo > hi {
            Ranges::empty()
        } else {
            Ranges(vec![(lo, hi)])
        }
    }

    fn normalize(mut v: Vec<(u32, u32)>) -> Ranges {
        v.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(v.len());
        for (a, b) in v {
            match out.last_mut() {
                Some(last) if last.1 == u32::MAX || a <= last.1 + 1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        Ranges(out)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.0 == [(0, u32::MAX)]
    }

    pub fn contains(&self, x: u32) -> bool {
        let i = self.0.partition_point(|r| r.0 <= x);
        i > 0 && x <= self.0[i - 1].1
    }

    pub fn complement(&self) -> Ranges {
        let mut out = Vec::new();
        let mut next = Some(0u32);
        for &(a, b) in &self.0 {
            if let Some(n) = next
                && a > n
            {
                out.push((n, a - 1));
            }
            next = b.checked_add(1);
        }
        if let Some(n) = next {
            out.push((n, u32::MAX));
        }
        Ranges(out)
    }

    pub fn intersect(&self, o: &Ranges) -> Ranges {
        let mut out = Vec::new();
        for &(a, b) in &self.0 {
            for &(c, d) in &o.0 {
                let (lo, hi) = (a.max(c), b.min(d));
                if lo <= hi {
                    out.push((lo, hi));
                }
            }
        }
        Ranges::normalize(out)
    }

    pub fn union(&self, o: &Ranges) -> Ranges {
        Ranges::normalize(self.0.iter().chain(&o.0).copied().collect())
    }

    /// Every element plus `k`, wrapping.
    pub fn shift(&self, k: u32) -> Ranges {
        let mut out = Vec::with_capacity(self.0.len() + 1);
        for &(a, b) in &self.0 {
            let (lo, hi) = (a.wrapping_add(k), b.wrapping_add(k));
            if lo <= hi {
                out.push((lo, hi));
            } else {
                out.push((lo, u32::MAX));
                out.push((0, hi));
            }
        }
        Ranges::normalize(out)
    }
}

/// A one-bit value: a flag or a condition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Bit {
    Const(bool),
    /// True exactly when the unknown `sym` is in `set`. `now` is the value
    /// if the sets known when it was made already decided it: a decided
    /// predicate is kept as a predicate, so that joining it with an
    /// undecided one from another path (the first trip round a loop, with
    /// the counter still a constant) still says something.
    Pred {
        sym: Sym,
        set: Ranges,
        now: Option<bool>,
    },
    Top,
}

impl Bit {
    pub fn pred(sym: Sym, set: Ranges, now: Option<bool>) -> Bit {
        if set.is_empty() {
            Bit::Const(false)
        } else if set.is_full() {
            Bit::Const(true)
        } else {
            Bit::Pred { sym, set, now }
        }
    }

    /// The bit's value, if known.
    pub fn konst(&self) -> Option<bool> {
        match *self {
            Bit::Const(b) => Some(b),
            Bit::Pred { now, .. } => now,
            Bit::Top => None,
        }
    }

    pub fn not(&self) -> Bit {
        match self {
            Bit::Const(b) => Bit::Const(!b),
            Bit::Pred { sym, set, now } => Bit::pred(*sym, set.complement(), now.map(|b| !b)),
            Bit::Top => Bit::Top,
        }
    }

    /// Combine two bits; `f` on values, `g` on the sets of two predicates
    /// over the same unknown.
    fn combine(
        &self,
        o: &Bit,
        f: fn(bool, bool) -> bool,
        g: impl Fn(&Ranges, &Ranges) -> Ranges,
    ) -> Bit {
        let now = match (self.konst(), o.konst()) {
            (Some(a), Some(b)) => Some(f(a, b)),
            (Some(a), None) if f(a, false) == f(a, true) => Some(f(a, false)),
            (None, Some(b)) if f(false, b) == f(true, b) => Some(f(false, b)),
            _ => None,
        };
        match (self, o) {
            (
                Bit::Pred {
                    sym: s1, set: a, ..
                },
                Bit::Pred {
                    sym: s2, set: b, ..
                },
            ) if s1 == s2 => Bit::pred(*s1, g(a, b), now),
            (Bit::Const(k), x) | (x, Bit::Const(k)) => {
                // f(k, x) is a constant, x, or not x.
                match (f(*k, false), f(*k, true)) {
                    (a, b) if a == b => Bit::Const(a),
                    (false, true) => x.clone(),
                    _ => x.not(),
                }
            }
            _ => now.map_or(Bit::Top, Bit::Const),
        }
    }

    pub fn and(&self, o: &Bit) -> Bit {
        self.combine(o, |a, b| a & b, |a, b| a.intersect(b))
    }

    pub fn or(&self, o: &Bit) -> Bit {
        self.combine(o, |a, b| a | b, |a, b| a.union(b))
    }

    pub fn xor(&self, o: &Bit) -> Bit {
        self.combine(
            o,
            |a, b| a ^ b,
            |a, b| {
                a.intersect(&b.complement())
                    .union(&a.complement().intersect(b))
            },
        )
    }

    pub fn join(&self, o: &Bit) -> Bit {
        if self == o { self.clone() } else { Bit::Top }
    }
}

/// A one-operand operation by a constant, recorded on a value computed by
/// it ([`Derived`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DOp {
    Lsl(u8),
    /// `x << n` of an `x` below `1 << (32 - n)`: no bits lost, so a
    /// `lsr` by the same `n` gives `x` back (GCC's zero-extending
    /// `lsls r3, r0, #16; lsrs r3, r3, #16`).
    LslExact(u8),
    Lsr(u8),
    Asr(u8),
    Mul(u32),
    /// Sign-extending the low `w` bits of an `x` below `1 << w` (an
    /// `ldrsh`'s halfword, an `ldrsb`'s byte).
    Sext(u8),
}

impl DOp {
    pub fn eval(self, x: u32) -> u32 {
        match self {
            DOp::Lsl(n) | DOp::LslExact(n) => x << n,
            DOp::Lsr(n) => x >> n,
            DOp::Asr(n) => ((x as i32) >> n) as u32,
            DOp::Mul(k) => x.wrapping_mul(k),
            DOp::Sext(w) => armv4t::sem::prim::sext(x, w as u32),
        }
    }

    /// The operation over a set (`n` is 1 to 31 for the shifts).
    pub fn apply(self, d: &Dom) -> Dom {
        if let Some(e) = d.elems() {
            return Dom::from_vals(e.into_iter().map(|x| self.eval(x)).collect());
        }
        match (self, d) {
            (DOp::Lsl(n) | DOp::LslExact(n), &Dom::Si { s, lo, hi }) if (hi as u64) << n <= u32::MAX as u64 => {
                Dom::si(s << n, lo << n, hi << n)
            }
            (DOp::Lsr(n), &Dom::Si { lo, hi, .. }) => Dom::si(1, lo >> n, hi >> n),
            (DOp::Lsr(n), Dom::Top) => Dom::si(1, 0, u32::MAX >> n),
            (DOp::Asr(n), &Dom::Si { lo, hi, .. }) if hi < 0x8000_0000 => {
                Dom::si(1, lo >> n, hi >> n)
            }
            (DOp::Mul(k), &Dom::Si { s, lo, hi })
                if k > 0 && hi as u64 * k as u64 <= u32::MAX as u64 =>
            {
                Dom::si(s * k, lo * k, hi * k)
            }
            // Nothing to extend below the sign bit.
            (DOp::Sext(w), Dom::Si { hi, .. }) if (*hi as u64) < 1 << (w - 1) => d.clone(),
            _ => Dom::Top,
        }
    }
}

/// How a value was computed from another: it is `op(sym + off)`. When a
/// branch narrows `sym`, the value narrows with it, so an index scaled
/// before its bounds check (`r4 = r6 * 4; cmp r6, #5`) is still bounded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Derived {
    pub sym: Sym,
    pub off: u32,
    pub op: DOp,
}

/// Where a value was read from ROM: `first..=last`, entries of `size`
/// bytes. Kept through operations with a constant, so a jump target says
/// which table it came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Src {
    pub first: u32,
    pub last: u32,
    pub size: u32,
}

impl Src {
    pub fn entries(&self) -> u32 {
        (self.last - self.first) / self.size + 1
    }

    fn join(a: Option<Src>, b: Option<Src>) -> Option<Src> {
        match (a, b) {
            (Some(x), Some(y)) if x.size == y.size => Some(Src {
                first: x.first.min(y.first),
                last: x.last.max(y.last),
                size: x.size,
            }),
            _ => None,
        }
    }
}

/// A register's or stack slot's value: its set, and `sym + off` (see the
/// module docs).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AbsVal {
    pub dom: Dom,
    pub sym: Sym,
    pub off: u32,
    pub src: Option<Src>,
    pub derived: Option<Derived>,
}

impl AbsVal {
    /// A value with no relation to any other: it names itself.
    pub fn new(dom: Dom, sym: Sym) -> AbsVal {
        AbsVal {
            dom,
            sym,
            off: 0,
            src: None,
            derived: None,
        }
    }

    pub fn top(sym: Sym) -> AbsVal {
        AbsVal::new(Dom::Top, sym)
    }

    /// The constant `k`. Constants are named by their value: two equal
    /// constants are the same value.
    pub fn konst(k: u32) -> AbsVal {
        AbsVal::new(Dom::Const(k), Sym::new(CONST, k, 0, 0))
    }

    /// The value plus `k`: same unknown, offset moved.
    pub fn add_const(&self, k: u32) -> AbsVal {
        AbsVal {
            dom: self.dom.add_const(k),
            sym: self.sym,
            off: self.off.wrapping_add(k),
            src: self.src,
            derived: None,
        }
    }

    /// The bit "this value is in `vals`": a predicate over the value's
    /// unknown, decided if the set decides it.
    pub fn pred(&self, vals: &Ranges) -> Bit {
        let now = if self.dom.within(vals) {
            Some(true)
        } else if self.dom.disjoint(vals) {
            Some(false)
        } else {
            None
        };
        Bit::pred(self.sym, vals.shift(self.off.wrapping_neg()), now)
    }

    /// Narrow to the values allowed when `sym` is in `set`; `None` if none
    /// are. Values over a different unknown are unchanged.
    pub fn assume(&self, sym: Sym, set: &Ranges) -> Option<AbsVal> {
        if self.sym != sym {
            return Some(self.clone());
        }
        let dom = self.dom.meet(&set.shift(self.off))?;
        Some(AbsVal {
            dom,
            ..self.clone()
        })
    }

    /// If this value is derived from `sym`, narrow it to what `sym`'s set
    /// `base` allows; `None` if nothing is left.
    pub fn assume_derived(&self, sym: Sym, base: &Dom) -> Option<AbsVal> {
        match self.derived {
            Some(d) if d.sym == sym => {
                let dom = self.dom.intersect(&d.op.apply(&base.add_const(d.off)))?;
                Some(AbsVal {
                    dom,
                    ..self.clone()
                })
            }
            _ => Some(self.clone()),
        }
    }

    /// This value's set joined with `o`'s, under name `sym + off`, derived
    /// as `derived` says. With
    /// `widen`, a set that changed goes straight to unknown.
    pub fn join_as(
        &self,
        o: &AbsVal,
        sym: Sym,
        off: u32,
        derived: Option<Derived>,
        widen: bool,
    ) -> AbsVal {
        let dom = if self.dom == o.dom {
            self.dom.clone()
        } else if widen {
            Dom::Top
        } else {
            self.dom.join(&o.dom)
        };
        AbsVal {
            dom,
            sym,
            off,
            src: Src::join(self.src, o.src),
            derived,
        }
    }
}
