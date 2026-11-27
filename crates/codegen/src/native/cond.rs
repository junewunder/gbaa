//! Branch conditions as comparisons and bit tests
//! (`native/lift-branch-conditions`; `docs/research/llm-pilot.md`, "Branch
//! conditions").
//!
//! Stage A computes N, Z, C and V at every flag-setting instruction and a
//! condition from them, so `cmp r1, #128; bgt` printed as
//! `add_with_carry(r1, !128, true)` and `!(z | (n ^ v))`. Each condition
//! after a subtraction (`cmp`, `subs`, `rsbs`, `negs`: `add_with_carry(a,
//! !b, 1)`) has an exact closed form over the operands, and after a logical
//! op or a flag-setting shift it is a bit test. [`forms`] gives each
//! bit-valued value of a block the form it prints as, and what the form
//! reads: the printer then neither prints the flags nor the arithmetic only
//! they used. Every form is an identity on every input, not just the
//! check's:
//!
//! - after `a - b`: Z is `a == b`, C `a >= b`, N ^ V `a as i32 < b as i32`,
//!   Z | (N ^ V) `<=`, C & !Z `a > b`, their negations the opposite
//!   comparisons; V `(a as i32).overflowing_sub(b as i32).1`; N, the
//!   result's sign, or `a as i32 < 0` when b is 0;
//! - `a + k` with a constant k other than 0 and 0x80000000 is `a - (-k)`
//!   (same result and the same four flags); another addition's C and V
//!   are `overflowing_add`'s;
//! - Z of `p & q` is `p & q == 0`; bit n of a value shifted by a constant
//!   is a bit of the value; a constant shift's carry out is one too;
//! - a combination of closed forms is their combination.
//!
//! A side printed at a width compares in it ([`super::casts`]): `(x as
//! i16 as u32) as i32 <= 9` prints `(x as i16) <= 9`, and a signed
//! compare's constant is an `i32` literal.
//!
//! What stays raw: a flag from another block (`GetFlag`), one an
//! instruction's condition selected (`BSelect`), a multiply's carry, and
//! an add with carry in from a flag. [`raw`] counts the conditions resting
//! on one: the metric `branches_on_raw_flags`.

use super::{casts, konst, v};
use analysis::ir::{BinOp, BitOp, Inst, Op, ValueId};
use armv4t::sem::ShiftType;

/// A comparison's relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    Eq,
    Ne,
    Lt,
    Ge,
    Gt,
    Le,
}

impl Rel {
    /// The opposite relation: `a rel b` false.
    pub fn negated(self) -> Rel {
        self.not()
    }

    fn not(self) -> Rel {
        match self {
            Rel::Eq => Rel::Ne,
            Rel::Ne => Rel::Eq,
            Rel::Lt => Rel::Ge,
            Rel::Ge => Rel::Lt,
            Rel::Gt => Rel::Le,
            Rel::Le => Rel::Gt,
        }
    }

    /// The same relation with its sides swapped.
    pub fn mirror(self) -> Rel {
        match self {
            Rel::Lt => Rel::Gt,
            Rel::Gt => Rel::Lt,
            Rel::Ge => Rel::Le,
            Rel::Le => Rel::Ge,
            r => r,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Rel::Eq => "==",
            Rel::Ne => "!=",
            Rel::Lt => "<",
            Rel::Ge => ">=",
            Rel::Gt => ">",
            Rel::Le => "<=",
        }
    }

    pub fn holds<T: Ord>(self, a: T, b: T) -> bool {
        match self {
            Rel::Eq => a == b,
            Rel::Ne => a != b,
            Rel::Lt => a < b,
            Rel::Ge => a >= b,
            Rel::Gt => a > b,
            Rel::Le => a <= b,
        }
    }
}

/// One side of a comparison: a value, a constant, or two sides' `&`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Side {
    Val(ValueId),
    Const(u32),
    And(Box<Side>, Box<Side>),
}

impl Side {
    /// The values the side reads.
    fn uses(&self, out: &mut Vec<ValueId>) {
        match self {
            Side::Val(x) => out.push(*x),
            Side::Const(_) => {}
            Side::And(a, b) => {
                a.uses(out);
                b.uses(out);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum F {
    /// `l rel r`, on `u32`s or as `i32`s.
    Cmp {
        l: Side,
        rel: Rel,
        r: Side,
        signed: bool,
    },
    Bool(bool),
    /// Any other `bool` expression.
    Text(String),
}

/// What a bit-valued value prints as, and the values that reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    f: F,
    pub uses: Vec<ValueId>,
}

/// One side of a comparison, for code that reasons about it
/// ([`Form::compare`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opnd {
    Val(ValueId),
    Const(u32),
}

impl Form {
    /// The comparison it is, `l rel r` (`signed`: as `i32`s), where its
    /// sides are values or constants.
    pub fn compare(&self) -> Option<(Opnd, Rel, Opnd, bool)> {
        let F::Cmp { l, rel, r, signed } = &self.f else {
            return None;
        };
        let o = |s: &Side| match *s {
            Side::Val(x) => Some(Opnd::Val(x)),
            Side::Const(k) => Some(Opnd::Const(k)),
            Side::And(..) => None,
        };
        Some((o(l)?, *rel, o(r)?, *signed))
    }

    fn new(f: F, uses: Vec<ValueId>) -> Form {
        Form { f, uses }.normal()
    }

    fn cmp(l: Side, rel: Rel, r: Side, signed: bool) -> Form {
        let mut uses = Vec::new();
        l.uses(&mut uses);
        r.uses(&mut uses);
        Form::new(F::Cmp { l, rel, r, signed }, uses)
    }

    /// The constant on the right, decided where both sides are constants
    /// or the constant is the type's least or greatest (so no comparison
    /// prints that is always true or false).
    fn normal(self) -> Form {
        let F::Cmp { l, rel, r, signed } = self.f else {
            return self;
        };
        let (l, rel, r) = match (l, r) {
            (Side::Const(a), Side::Const(b)) => {
                let t = if signed {
                    rel.holds(a as i32, b as i32)
                } else {
                    rel.holds(a, b)
                };
                return Form {
                    f: F::Bool(t),
                    uses: Vec::new(),
                };
            }
            (Side::Const(a), r) => (r, rel.mirror(), Side::Const(a)),
            (l, r) => (l, rel, r),
        };
        let (min, max) = if signed {
            (0x8000_0000, 0x7fff_ffff)
        } else {
            (0, u32::MAX)
        };
        let f = match (rel, &r) {
            (Rel::Ge, &Side::Const(k)) if k == min => F::Bool(true),
            (Rel::Lt, &Side::Const(k)) if k == min => F::Bool(false),
            (Rel::Le, &Side::Const(k)) if k == max => F::Bool(true),
            (Rel::Gt, &Side::Const(k)) if k == max => F::Bool(false),
            (Rel::Gt, &Side::Const(k)) if k == min => F::Cmp {
                l,
                rel: Rel::Ne,
                r,
                signed,
            },
            (Rel::Le, &Side::Const(k)) if k == min => F::Cmp {
                l,
                rel: Rel::Eq,
                r,
                signed,
            },
            (Rel::Lt, &Side::Const(k)) if k == max => F::Cmp {
                l,
                rel: Rel::Ne,
                r,
                signed,
            },
            (Rel::Ge, &Side::Const(k)) if k == max => F::Cmp {
                l,
                rel: Rel::Eq,
                r,
                signed,
            },
            _ => F::Cmp { l, rel, r, signed },
        };
        let uses = if matches!(f, F::Bool(_)) {
            Vec::new()
        } else {
            self.uses
        };
        Form { f, uses }
    }

    pub(crate) fn not(&self) -> Form {
        let f = match &self.f {
            F::Cmp { l, rel, r, signed } => F::Cmp {
                l: l.clone(),
                rel: rel.not(),
                r: r.clone(),
                signed: *signed,
            },
            F::Bool(b) => F::Bool(!b),
            F::Text(t) => F::Text(format!("!({t})")),
        };
        Form::new(f, self.uses.clone())
    }

    fn join(&self, o: BitOp, other: &Form) -> Form {
        let op = match o {
            BitOp::And => "&&",
            BitOp::Or => "||",
            BitOp::Xor => "!=",
        };
        let mut uses = self.uses.clone();
        uses.extend(&other.uses);
        Form::new(
            F::Text(format!("({}) {op} ({})", self.text(), other.text())),
            uses,
        )
    }

    /// The Rust expression, each value by its `let`'s name.
    pub fn text(&self) -> String {
        self.text_with(&|x| v(x))
    }

    /// Whether the form prints its values through [`Form::text_with`]'s
    /// `val`, so they may be inlined; a joined form's text names their
    /// `let`s.
    pub fn takes_inlined(&self) -> bool {
        !matches!(self.f, F::Text(_))
    }

    /// The Rust expression, value `x` printed as `val(x)` (its name, or
    /// its inlined expression in parentheses) where the form allows.
    pub fn text_with(&self, val: &dyn Fn(ValueId) -> String) -> String {
        self.text_named(val, &|_, _| None)
    }

    /// [`Form::text_with`], a compare of a value with a constant `name`
    /// gives a name for (one of a field's named values) printing that
    /// name.
    pub fn text_named(
        &self,
        val: &dyn Fn(ValueId) -> String,
        name: &dyn Fn(ValueId, u32) -> Option<String>,
    ) -> String {
        match &self.f {
            F::Cmp { l, rel, r, signed } => {
                if let Some(t) = narrow_side(l, *rel, r, *signed, val, name) {
                    return t;
                }
                format!(
                    "{} {} {}",
                    side(l, *signed, true, val),
                    rel.text(),
                    named_side(l, r, *signed, name).unwrap_or_else(|| side(r, *signed, false, val))
                )
            }
            F::Bool(b) => format!("{b}"),
            F::Text(t) => t.clone(),
        }
    }
}

/// A compare's constant side by its name, where `name` gives one for the
/// value on the other side and the constant (native/lift-value-names).
fn named_side(
    l: &Side,
    r: &Side,
    signed: bool,
    name: &dyn Fn(ValueId, u32) -> Option<String>,
) -> Option<String> {
    let (&Side::Val(x), &Side::Const(k)) = (l, r) else {
        return None;
    };
    let n = name(x, k)?;
    Some(if signed { format!("({n} as i32)") } else { n })
}

/// A compare of a value extended from a narrower type (`x as u8 as u32`,
/// a `u16` field's load) in that type ([`casts::narrow_cmp`]), where the
/// other side is a constant (one no name is given for) or a value
/// extended from the same type.
fn narrow_side(
    l: &Side,
    rel: Rel,
    r: &Side,
    signed: bool,
    val: &dyn Fn(ValueId) -> String,
    name: &dyn Fn(ValueId, u32) -> Option<String>,
) -> Option<String> {
    let &Side::Val(x) = l else {
        return None;
    };
    let rt;
    let r = match *r {
        Side::Const(k) if name(x, k).is_none() => Ok(k),
        Side::Val(y) => {
            rt = val(y);
            Err(rt.as_str())
        }
        _ => return None,
    };
    casts::narrow_cmp(&val(x), rel.text(), r, signed)
}

/// A side as Rust. A constant on the left is typed (nothing else gives
/// its type); on the right it takes the other side's.
fn side(s: &Side, signed: bool, left: bool, val: &dyn Fn(ValueId) -> String) -> String {
    match (s, signed) {
        (Side::Val(x), false) => val(*x),
        (Side::Val(x), true) => format!("({})", casts::to_i32(&val(*x))),
        (Side::And(a, b), false) => format!("({} & {})", and(a, val), and(b, val)),
        (Side::And(..), true) => format!("({} as i32)", side(s, false, false, val)),
        (&Side::Const(k), false) if left => format!("{k:#x}u32"),
        (&Side::Const(k), false) => lit(k),
        (&Side::Const(k), true) if !left && (-0x10000..=0x10000).contains(&(k as i32)) => {
            format!("{}", k as i32)
        }
        // An `i32` literal: any but the least, which Rust can't negate.
        (&Side::Const(k), true) if !left && k != 0x8000_0000 => casts::literal(k as i32 as i64),
        (&Side::Const(k), true) => format!("({k:#x}u32 as i32)"),
    }
}

/// An `&`'s operand: a constant in hex.
fn and(s: &Side, val: &dyn Fn(ValueId) -> String) -> String {
    match *s {
        Side::Const(k) => format!("{k:#x}"),
        ref s => side(s, false, false, val),
    }
}

/// A side as an `i32` argument.
fn arg(s: &Side) -> String {
    match s {
        Side::Val(x) => format!("{} as i32", v(*x)),
        s => side(s, true, false, &|x| v(x)),
    }
}

/// The values two sides read.
fn sides_uses(l: &Side, r: &Side) -> Vec<ValueId> {
    let mut uses = Vec::new();
    l.uses(&mut uses);
    r.uses(&mut uses);
    uses
}

/// An unsigned constant: small ones in decimal.
fn lit(k: u32) -> String {
    if k <= 255 {
        format!("{k}")
    } else {
        format!("{k:#x}")
    }
}

/// `x` as a side: its constant if it has one.
fn opnd(insts: &[Inst], x: ValueId) -> Side {
    konst(insts, x).map_or(Side::Val(x), Side::Const)
}

/// What an `add_with_carry(a, b, c)` computes, where that is plain.
enum Arith {
    /// `l - r`.
    Sub(Side, Side),
    /// `a + b`, neither a constant that makes it a subtraction.
    Add(ValueId, ValueId),
}

fn arith(insts: &[Inst], a: ValueId, b: ValueId, c: ValueId) -> Option<Arith> {
    let Op::Bit(c) = insts[c.index()].op else {
        return None;
    };
    if c {
        // a + !y + 1 is a - y; a + k + 1 is a - !k.
        if let Op::Not(y) = insts[b.index()].op {
            return Some(Arith::Sub(opnd(insts, a), opnd(insts, y)));
        }
        if let Op::Not(x) = insts[a.index()].op {
            return Some(Arith::Sub(opnd(insts, b), opnd(insts, x)));
        }
        if let Some(k) = konst(insts, b) {
            return Some(Arith::Sub(opnd(insts, a), Side::Const(!k)));
        }
        if let Some(k) = konst(insts, a) {
            return Some(Arith::Sub(opnd(insts, b), Side::Const(!k)));
        }
        return None;
    }
    // a + k is a - (-k), flags and all, unless -k is k.
    let neg = |k: u32| (k != 0 && k != 0x8000_0000).then(|| Side::Const(k.wrapping_neg()));
    if let Some(k) = konst(insts, b).and_then(neg) {
        return Some(Arith::Sub(opnd(insts, a), k));
    }
    if let Some(k) = konst(insts, a).and_then(neg) {
        return Some(Arith::Sub(opnd(insts, b), k));
    }
    Some(Arith::Add(a, b))
}

/// The `(a, b, c)` of an `add_with_carry` result or flag.
fn adc(op: Op) -> Option<(ValueId, ValueId, ValueId)> {
    match op {
        Op::Adc { a, b, c } => Some((a, b, c)),
        _ => None,
    }
}

/// `x`'s subtraction, if `x` is N ^ V after one: its sides.
fn less(insts: &[Inst], x: ValueId) -> Option<(Side, Side, (ValueId, ValueId, ValueId))> {
    let Op::BBin(BitOp::Xor, p, q) = insts[x.index()].op else {
        return None;
    };
    less_of(insts, p, q)
}

/// [`less`] of N ^ V's operands `p` and `q`.
fn less_of(
    insts: &[Inst],
    p: ValueId,
    q: ValueId,
) -> Option<(Side, Side, (ValueId, ValueId, ValueId))> {
    for (n, o) in [(p, q), (q, p)] {
        let (Op::TestBit { v: r, n: 31 }, Op::AdcOverflow { a, b, c }) =
            (insts[n.index()].op, insts[o.index()].op)
        else {
            continue;
        };
        if adc(insts[r.index()].op) != Some((a, b, c)) {
            continue;
        }
        if let Some(Arith::Sub(l, r)) = arith(insts, a, b, c) {
            return Some((l, r, (a, b, c)));
        }
    }
    None
}

/// Whether `x` is Z of `add_with_carry(t)`'s result.
fn zero_of(insts: &[Inst], x: ValueId, t: (ValueId, ValueId, ValueId)) -> bool {
    matches!(insts[x.index()].op, Op::IsZero(r) if adc(insts[r.index()].op) == Some(t))
}

/// Bit `n` of `x` set.
fn bit(insts: &[Inst], x: ValueId, n: u32) -> Form {
    if let Some(k) = konst(insts, x) {
        return Form::new(F::Bool((k >> n) & 1 != 0), Vec::new());
    }
    if n == 31 {
        return Form::cmp(Side::Val(x), Rel::Lt, Side::Const(0), true);
    }
    Form::cmp(
        Side::And(Box::new(Side::Val(x)), Box::new(Side::Const(1 << n))),
        Rel::Ne,
        Side::Const(0),
        false,
    )
}

/// Each value's form, where it has one: bit-valued pure values only.
pub fn forms(insts: &[Inst]) -> Vec<Option<Form>> {
    let mut out: Vec<Option<Form>> = Vec::with_capacity(insts.len());
    for inst in insts {
        let f = form(insts, &out, inst.op);
        out.push(f);
    }
    out
}

fn form(insts: &[Inst], done: &[Option<Form>], op: Op) -> Option<Form> {
    let get = |x: ValueId| done[x.index()].clone();
    Some(match op {
        Op::Bit(b) => Form::new(F::Bool(b), Vec::new()),
        Op::AdcCarry { a, b, c } => match arith(insts, a, b, c)? {
            Arith::Sub(l, r) => Form::cmp(l, Rel::Ge, r, false),
            Arith::Add(x, y) => {
                let (l, r) = (opnd(insts, x), opnd(insts, y));
                Form::new(
                    F::Text(format!(
                        "{}.overflowing_add({}).1",
                        side(&l, false, true, &|x| v(x)),
                        side(&r, false, false, &|x| v(x))
                    )),
                    sides_uses(&l, &r),
                )
            }
        },
        Op::AdcOverflow { a, b, c } => match arith(insts, a, b, c)? {
            // a - 0 never overflows.
            Arith::Sub(_, Side::Const(0)) => Form::new(F::Bool(false), Vec::new()),
            Arith::Sub(l, r) => {
                let mut uses = Vec::new();
                l.uses(&mut uses);
                r.uses(&mut uses);
                Form::new(
                    F::Text(format!(
                        "{}.overflowing_sub({}).1",
                        side(&l, true, true, &|x| v(x)),
                        arg(&r)
                    )),
                    uses,
                )
            }
            Arith::Add(x, y) => {
                let (l, r) = (opnd(insts, x), opnd(insts, y));
                Form::new(
                    F::Text(format!(
                        "{}.overflowing_add({}).1",
                        side(&l, true, true, &|x| v(x)),
                        arg(&r)
                    )),
                    sides_uses(&l, &r),
                )
            }
        },
        Op::IsZero(r) => match insts[r.index()].op {
            Op::Adc { a, b, c } => match arith(insts, a, b, c) {
                Some(Arith::Sub(l, s)) => Form::cmp(l, Rel::Eq, s, false),
                _ => Form::cmp(opnd(insts, r), Rel::Eq, Side::Const(0), false),
            },
            Op::Bin(BinOp::And, p, q) if konst(insts, r).is_none() => Form::cmp(
                Side::And(Box::new(opnd(insts, p)), Box::new(opnd(insts, q))),
                Rel::Eq,
                Side::Const(0),
                false,
            ),
            _ => Form::cmp(opnd(insts, r), Rel::Eq, Side::Const(0), false),
        },
        Op::TestBit { v: x, n } => match insts[x.index()].op {
            Op::Adc { a, b, c } if n == 31 => match arith(insts, a, b, c) {
                Some(Arith::Sub(l, Side::Const(0))) => Form::cmp(l, Rel::Lt, Side::Const(0), true),
                _ => bit(insts, x, n),
            },
            Op::Shift { kind, v: y, amount } if konst(insts, x).is_none() => {
                match (kind, konst(insts, amount).map(|k| k & 0xff)) {
                    (ShiftType::Lsl, Some(k @ 0..=31)) if n >= k => bit(insts, y, n - k),
                    (ShiftType::Lsl, Some(0..=31)) => Form::new(F::Bool(false), Vec::new()),
                    (ShiftType::Lsr, Some(k @ 0..=31)) if n + k < 32 => bit(insts, y, n + k),
                    (ShiftType::Lsr, Some(0..=31)) => Form::new(F::Bool(false), Vec::new()),
                    (ShiftType::Asr, Some(k @ 0..=31)) => bit(insts, y, (n + k).min(31)),
                    _ => bit(insts, x, n),
                }
            }
            _ => bit(insts, x, n),
        },
        // A constant shift's carry out (`prim::shift`).
        Op::ShiftCarry {
            kind, v: y, amount, ..
        } => {
            let k = konst(insts, amount)? & 0xff;
            let b = match (kind, k) {
                (_, 0) => return None,
                (ShiftType::Lsl, 1..=31) => Some(32 - k),
                (ShiftType::Lsl, 32) => Some(0),
                (ShiftType::Lsr, 1..=31) => Some(k - 1),
                (ShiftType::Lsr, 32) => Some(31),
                (ShiftType::Lsl | ShiftType::Lsr, _) => None,
                (ShiftType::Asr, 1..=31) => Some(k - 1),
                (ShiftType::Asr, _) => Some(31),
                (ShiftType::Ror, _) => Some((k & 31).checked_sub(1).unwrap_or(31)),
            };
            match b {
                Some(b) => bit(insts, y, b),
                None => Form::new(F::Bool(false), Vec::new()),
            }
        }
        Op::BNot(x) => get(x)?.not(),
        Op::BBin(o, p, q) => {
            // N ^ V after a - b: a < b, signed.
            if o == BitOp::Xor
                && let Some((l, r, _)) = less_of(insts, p, q)
            {
                return Some(Form::cmp(l, Rel::Lt, r, true));
            }
            // Z | (N ^ V) after a - b: a <= b, signed.
            if o == BitOp::Or {
                for (z, lt) in [(p, q), (q, p)] {
                    if let Some((l, r, t)) = less(insts, lt)
                        && zero_of(insts, z, t)
                    {
                        return Some(Form::cmp(l, Rel::Le, r, true));
                    }
                }
            }
            // C & !Z after a - b: a > b.
            if o == BitOp::And {
                for (cy, nz) in [(p, q), (q, p)] {
                    if let (Op::AdcCarry { a, b, c }, Op::BNot(z)) =
                        (insts[cy.index()].op, insts[nz.index()].op)
                        && zero_of(insts, z, (a, b, c))
                        && let Some(Arith::Sub(l, r)) = arith(insts, a, b, c)
                    {
                        return Some(Form::cmp(l, Rel::Gt, r, false));
                    }
                }
            }
            get(p)?.join(o, &get(q)?)
        }
        _ => return None,
    })
}

/// The conditions of a block resting on a raw flag: each distinct guard
/// with no form.
pub fn raw(insts: &[Inst], forms: &[Option<Form>]) -> u64 {
    let mut gs: Vec<ValueId> = insts.iter().filter_map(|i| i.guard).collect();
    gs.sort_by_key(|g| g.0);
    gs.dedup();
    gs.iter().filter(|g| forms[g.index()].is_none()).count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use analysis::ir::Block;
    use analysis::ir::build::Builder;
    use analysis::ir::constprop::Rom;
    use armv4t::decode::{Cond, decode_thumb};
    use armv4t::interp::{Bus, Cpu, T_BIT, mode};
    use armv4t::sem::arm::execute;
    use armv4t::sem::{Width, prim};

    const START: u32 = 0x0800_0000;

    struct NoMem;
    impl Bus for NoMem {
        fn load(&mut self, _: Width, _: u32) -> u32 {
            0
        }
        fn store(&mut self, _: Width, _: u32, _: u32) {}
    }

    /// The Thumb instructions `raw` as the lift builds them.
    fn block(raw: &[u16]) -> Block {
        let mut b = Builder::new(START, true);
        let mut at = START;
        for &r in raw {
            b.begin_insn(at);
            execute(&mut b, at, true, decode_thumb(r, at));
            at += 2;
            if b.ended() {
                break;
            }
        }
        let mut block = b.finish(at);
        block.const_prop(Rom(&[]));
        block.dce();
        block
    }

    /// Value `x` with r0 and r1 as given.
    fn val(insts: &[Inst], x: ValueId, r: [u32; 2]) -> u32 {
        let w = |y| val(insts, y, r);
        match insts[x.index()].op {
            Op::GetReg(g) => r[g.index()],
            Op::Const(k) | Op::Pool(k) => k,
            Op::Not(a) => !w(a),
            Op::Bin(BinOp::And, a, b) => w(a) & w(b),
            Op::Shift { kind, v, amount } => prim::shift(kind, w(v), w(amount), false).0,
            Op::Bit(b) => b as u32,
            Op::Adc { a, b, c } => prim::add_with_carry(w(a), w(b), w(c) != 0).0,
            ref op => panic!("no value for {op:?}"),
        }
    }

    fn side_val(insts: &[Inst], s: &Side, r: [u32; 2]) -> u32 {
        match s {
            Side::Val(x) => val(insts, *x, r),
            Side::Const(k) => *k,
            Side::And(a, b) => side_val(insts, a, r) & side_val(insts, b, r),
        }
    }

    /// A form's truth, if it is a comparison or a constant.
    fn holds(insts: &[Inst], f: &Form, r: [u32; 2]) -> Option<bool> {
        match &f.f {
            F::Bool(b) => Some(*b),
            F::Cmp {
                l,
                rel,
                r: rs,
                signed,
            } => {
                let (a, b) = (side_val(insts, l, r), side_val(insts, rs, r));
                Some(if *signed {
                    rel.holds(a as i32, b as i32)
                } else {
                    rel.holds(a, b)
                })
            }
            F::Text(_) => None,
        }
    }

    const EDGES: [u32; 10] = [
        0,
        1,
        3,
        0x7f,
        0x80,
        0x7fff_ffff,
        0x8000_0000,
        0x8000_0001,
        0xffff_ffff,
        0x1234_5678,
    ];

    /// Every condition after `op` (a Thumb instruction setting flags from
    /// r0 and r1): its form's truth is the CPU's on every pair of edge
    /// values; returns how many conditions printed as something other
    /// than a comparison or constant.
    fn agrees(op: u16) -> usize {
        let mut texts = 0;
        for c in 0..14u16 {
            let cond = Cond::from_bits(c as u32);
            let b = block(&[op, 0xD004 | c << 8]);
            let forms = forms(&b.insts);
            let g = b
                .insts
                .iter()
                .find_map(|i| matches!(i.op, Op::Exit(_)).then_some(i.guard).flatten())
                .expect("a guarded branch");
            let f = forms[g.index()]
                .as_ref()
                .unwrap_or_else(|| panic!("{op:#06x} {cond:?}: no form"));
            if matches!(f.f, F::Text(_)) {
                texts += 1;
                continue;
            }
            let flags = block(&[op]);
            for a in EDGES {
                for x in EDGES {
                    let mut cpu = Cpu {
                        pc: START,
                        cpsr: mode::SYS | T_BIT,
                        ..Cpu::default()
                    };
                    cpu.r[0] = a;
                    cpu.r[1] = x;
                    analysis::ir::eval::run(&flags, &mut cpu, &mut NoMem);
                    assert_eq!(
                        holds(&b.insts, f, [a, x]),
                        Some(cpu.cond_passes(cond)),
                        "{op:#06x} {cond:?} as `{}` on r0 {a:#x}, r1 {x:#x}",
                        f.text()
                    );
                }
            }
        }
        texts
    }

    #[test]
    fn conditions_after_a_compare_are_comparisons() {
        // cmp r0, r1; cmp r0, #128; cmp r0, #0; subs r0, r0, r1; negs
        // r0, r1; adds r2, r0, #1: all but VS and VC (overflow) compare.
        // Against 0, V is never set, so they are constants.
        for (op, texts) in [
            (0x4288, 2),
            (0x2880, 2),
            (0x2800, 0),
            (0x1A40, 2),
            (0x4248, 2),
            (0x1C42, 2),
        ] {
            assert_eq!(agrees(op), texts, "{op:#06x}");
        }
        // cmn r0, r1: C, V and the signed ones are overflowing_add's.
        agrees(0x42C8);
    }

    #[test]
    fn conditions_after_a_logical_op_or_shift_are_bit_tests() {
        // tst r0, r1; ands r0, r1; lsls, lsrs, asrs r0, r0, #3: N, Z and
        // C (the shifts') are bit tests or comparisons with zero; a flag
        // as it came (V; tst's and ands's C) has no form there (a GetFlag),
        // so only the conditions on N, Z and the shifts' C.
        for (op, shift) in [
            (0x4208, false),
            (0x4008, false),
            (0x00C0, true),
            (0x08C0, true),
            (0x10C0, true),
        ] {
            for c in [0u16, 1, 2, 3, 4, 5, 8, 9] {
                let b = block(&[op, 0xD004 | c << 8]);
                let forms = forms(&b.insts);
                let g = b.insts.iter().find_map(|i| i.guard).unwrap();
                let on_c = matches!(c, 2 | 3 | 8 | 9);
                let Some(f) = forms[g.index()].as_ref() else {
                    assert!(on_c && !shift, "{op:#06x} {c}: no form");
                    continue;
                };
                let flags = block(&[op]);
                for a in EDGES {
                    for x in EDGES {
                        let mut cpu = Cpu {
                            pc: START,
                            cpsr: mode::SYS | T_BIT,
                            ..Cpu::default()
                        };
                        cpu.r[0] = a;
                        cpu.r[1] = x;
                        // Carry in set, so a carry the op leaves shows.
                        cpu.set_flag(armv4t::sem::Flag::C, true);
                        analysis::ir::eval::run(&flags, &mut cpu, &mut NoMem);
                        let cond = Cond::from_bits(c as u32);
                        let got = holds(&b.insts, f, [a, x]);
                        if got.is_none() {
                            continue;
                        }
                        assert_eq!(
                            got,
                            Some(cpu.cond_passes(cond)),
                            "{op:#06x} {cond:?} `{}`",
                            f.text()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_compare_prints_as_the_pilots_rewrite() {
        // cmp r1, #128; bgt (ResetFreeOam's).
        let b = block(&[0x2980, 0xDC04]);
        let forms = forms(&b.insts);
        let g = b.insts.iter().find_map(|i| i.guard).unwrap();
        assert_eq!(
            forms[g.index()].as_ref().unwrap().text(),
            "(v1 as i32) > 128"
        );
    }
}
