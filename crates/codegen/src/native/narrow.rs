//! The compilers' narrowing idioms printed as casts
//! (`native/lift-narrowing-casts`; `docs/research/llm-pilot.md`,
//! "Narrowing idioms"). agbcc narrows a value to a `u8`, `s8`, `u16` or
//! `s16` with a pair of shifts, `lsls #24; lsrs #24` (or `asrs`), or with
//! an `ands` by a register it loaded 0xff into, often before a loop. The
//! shift pair is a bit identity within a block's IR; the mask needs the
//! register's constant carried into the block, which [`starts`] finds
//! (the play build's `lower::known`, over the edges the native printer
//! prints).
//!
//! What prints, over the value `x` narrowed:
//! - a shift left by `c` then right by `c`, logical: `x as u8 as u32`
//!   (`c` 24), `x as u16 as u32` (16), else `x & mask`; arithmetic:
//!   `x as i8 as u32`, `x as i16 as u32`, else `prim::sext(x, 32 - c)`;
//! - `x & c` with `c` 0xff or 0xffff, either side a constant as the
//!   block knows it, carried registers included: `x as u8 as u32`,
//!   `x as u16 as u32`;
//! - any other shift by a constant 1 to 31 as Rust's operator:
//!   `x << c`, `x >> c`, `((x as i32) >> c) as u32`, `x.rotate_right(c)`.
//!
//! A narrowed or shifted value that already printed at a width goes
//! through [`super::casts`]: `(x as u16 as u32) as u8 as u32` is `x as u8
//! as u32`, and an arithmetic shift of `x as i16 as u32` by less than 16
//! shifts the `i16`.
//!
//! The metrics: `shift_pairs_left` (barred) counts the guest's shift
//! pairs, a `lsl #c` and the `lsr #c` or `asr #c` of its result right
//! after it, whose second shift still printed as a shift (one the IR
//! folded to a constant, a ROM literal's, prints as that);
//! `narrowing_casts` (no bar, it should rise) those that did, with the
//! masks.

use crate::lower::{Sets, Val, known};
use analysis::ir::{BinOp, Inst, Op, ValueId};
use armv4t::decode::{AluOp, Cond, Instr, Operand2, Shift};
use armv4t::sem::ShiftType;
use std::collections::BTreeMap;

/// The registers each block starts with a known constant in, by key.
pub type Starts = BTreeMap<u32, BTreeMap<usize, u32>>;

/// How an edge out of a block leaves the registers' locals: as the block
/// left them (a jump), as a call left them (r4-r11 and sp: a printed
/// call sets only r0-r3's), or unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    Jump,
    KeptCall,
    Unknown,
}

/// One edge between a function's blocks: its block, the exit inst's
/// index, its target and how it leaves the registers.
pub struct Edge {
    pub from: u32,
    pub exit: usize,
    pub to: u32,
    pub after: After,
}

/// The registers each block of a function starts with a known constant
/// in (`lower::known`): nothing at `entry`, and at another block what
/// every edge in agrees on. `edges` must be every edge between the
/// function's blocks; `insts` gives a block's IR by key.
pub fn starts<'a>(entry: u32, edges: &[Edge], insts: impl Fn(u32) -> &'a [Inst]) -> Starts {
    let sets: Vec<Option<Sets>> = edges
        .iter()
        .map(|e| match e.after {
            After::Unknown => None,
            a => Some((exit_sets(insts(e.from), e.exit), a == After::KeptCall)),
        })
        .collect();
    let steps: Vec<_> = edges
        .iter()
        .zip(&sets)
        .map(|(e, s)| (e.from, e.to, s.as_ref()))
        .collect();
    known(entry, &steps)
}

/// What a block writes each register's local by the exit at inst `x`
/// (its last write): a constant, a register's value at the block's
/// start, or something else (a guarded write is something else, and an
/// `msr` sets sp).
fn exit_sets(insts: &[Inst], x: usize) -> Vec<(usize, Val)> {
    let mut set: BTreeMap<usize, Val> = BTreeMap::new();
    let mut written = 0u16;
    for inst in &insts[..x] {
        if let Op::SetCpsr { .. } = inst.op {
            set.insert(13, Val::Other);
            written |= 1 << 13;
        }
        if let Op::SetReg(r, v) = inst.op
            && r.index() < 15
        {
            let val = if inst.guard.is_some() {
                Val::Other
            } else {
                match insts[v.index()].op {
                    Op::GetReg(q) if q.index() < 15 && written & 1 << q.index() == 0 => {
                        Val::Reg(q.index())
                    }
                    _ => block_const(insts, v).map_or(Val::Other, Val::Const),
                }
            };
            set.insert(r.index(), val);
            written |= 1 << r.index();
        }
    }
    set.into_iter().collect()
}

/// `v`'s value if it is a constant within its block.
fn block_const(insts: &[Inst], v: ValueId) -> Option<u32> {
    let none = BTreeMap::new();
    values(insts, &none)[v.index()]
}

/// Each value of a block that is a constant, the registers it starts
/// with as `start` has them: a literal, a pool word, a register read
/// before the block writes it, or arithmetic on them.
pub fn values(insts: &[Inst], start: &BTreeMap<usize, u32>) -> Vec<Option<u32>> {
    let mut regs = start.clone();
    let mut out: Vec<Option<u32>> = Vec::with_capacity(insts.len());
    for inst in insts {
        let k = |x: ValueId| out[x.index()];
        let v = match inst.op {
            Op::Const(c) | Op::Pool(c) => Some(c),
            Op::GetReg(r) => regs.get(&r.index()).copied(),
            Op::Not(a) => k(a).map(|a| !a),
            Op::Bin(o, a, b) => match (k(a), k(b)) {
                (Some(a), Some(b)) => Some(match o {
                    BinOp::Add => a.wrapping_add(b),
                    BinOp::Sub => a.wrapping_sub(b),
                    BinOp::Mul => a.wrapping_mul(b),
                    BinOp::And => a & b,
                    BinOp::Or => a | b,
                    BinOp::Xor => a ^ b,
                }),
                _ => None,
            },
            Op::SetReg(r, x) => {
                match k(x).filter(|_| inst.guard.is_none()) {
                    Some(c) => regs.insert(r.index(), c),
                    None => regs.remove(&r.index()),
                };
                None
            }
            // The printer's `msr` sets sp's local (`msr_cpsr`).
            Op::SetCpsr { .. } => {
                regs.remove(&13);
                None
            }
            _ => None,
        };
        out.push(v);
    }
    out
}

/// How a value prints when it narrows or shifts another by a constant,
/// or adds to it or takes from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// Its low `bits` bits, sign-extended if `signed`.
    Narrow {
        bits: u32,
        signed: bool,
    },
    /// A shift by a constant 1 to 31.
    Shift(ShiftType, u32),
    /// `x + 0` (`adds rd, rs, #0`, agbcc's move, or `subs rd, rs, #0`):
    /// `x` itself (`native/lift-add-zero`). Its flags are their own
    /// values, so the sum prints as the value it copies whatever reads them.
    Copy,
    /// `x + k`, `x - k` and `k - x`, a constant `k`.
    AddConst(u32),
    SubConst(u32),
    ConstSub(u32),
    /// `x + y`, `x - y`.
    Add(ValueId),
    Sub(ValueId),
    /// `x + y + c`, a carry bit `c`.
    AddCarry(ValueId, ValueId),
    /// `x + c`.
    CarryOnly(ValueId),
    /// `x` itself: a load's rotate by an amount proven 0
    /// ([`super::aligned`]).
    Same,
}

impl Form {
    /// The values it reads besides `x`.
    pub fn others(self) -> Vec<ValueId> {
        match self {
            Form::Add(y) | Form::Sub(y) => vec![y],
            Form::AddCarry(y, c) => vec![y, c],
            Form::CarryOnly(c) => vec![c],
            _ => Vec::new(),
        }
    }

    /// The expression over `x`, the value it narrows or shifts (an
    /// operand, parenthesised unless it is one term), and the others
    /// ([`Form::others`]) each as `w` prints a whole expression.
    pub fn print_with(self, x: &str, w: &dyn Fn(ValueId) -> String) -> String {
        match self {
            Form::Copy => x.to_string(),
            Form::AddConst(k) => format!("{x}.wrapping_add({k:#x})"),
            Form::SubConst(k) => format!("{x}.wrapping_sub({k:#x})"),
            Form::ConstSub(0) => format!("{x}.wrapping_neg()"),
            Form::ConstSub(k) => format!("{k:#x}u32.wrapping_sub({x})"),
            Form::Add(y) => format!("{x}.wrapping_add({})", w(y)),
            Form::Sub(y) => format!("{x}.wrapping_sub({})", w(y)),
            Form::AddCarry(y, c) => {
                format!(
                    "{x}.wrapping_add({}).wrapping_add(u32::from({}))",
                    w(y),
                    w(c)
                )
            }
            Form::CarryOnly(c) => format!("{x}.wrapping_add(u32::from({}))", w(c)),
            f => f.print(x),
        }
    }

    /// [`Form::print_with`] of a form that reads no other value.
    pub fn print(self, x: &str) -> String {
        match self {
            Form::Narrow {
                bits: b @ (8 | 16),
                signed,
            } => {
                let t = format!("{}{b}", if signed { "i" } else { "u" });
                format!("{} as u32", super::casts::cast(x, &t))
            }
            Form::Narrow {
                bits,
                signed: false,
            } => format!("{x} & {:#x}", (1u32 << bits) - 1),
            Form::Narrow { bits, signed: true } => format!("prim::sext({x}, {bits})"),
            Form::Shift(ShiftType::Lsl, c) => format!("{x} << {c}"),
            Form::Shift(ShiftType::Lsr, c) => format!("{x} >> {c}"),
            Form::Shift(ShiftType::Asr, c) => super::casts::asr(x, c),
            Form::Shift(ShiftType::Ror, c) => format!("{x}.rotate_right({c})"),
            Form::Same => x.to_string(),
            f => panic!("{f:?} reads another value"),
        }
    }

    pub fn narrows(self) -> bool {
        matches!(self, Form::Narrow { .. })
    }
}

/// How inst `i` prints over another value, with `known` the block's
/// constants ([`values`]): the value it reads and its [`Form`], or
/// `None` to print it as the IR has it.
pub fn form(insts: &[Inst], known: &[Option<u32>], i: usize) -> Option<(ValueId, Form)> {
    let amount = |a: ValueId| known[a.index()].filter(|c| (1..32).contains(c));
    match insts[i].op {
        Op::Shift { kind, v, amount: a } => {
            let c = amount(a)?;
            if let (
                ShiftType::Lsr | ShiftType::Asr,
                Op::Shift {
                    kind: ShiftType::Lsl,
                    v: x,
                    amount: b,
                },
            ) = (kind, insts[v.index()].op)
                && amount(b) == Some(c)
            {
                let signed = kind == ShiftType::Asr;
                return Some((
                    x,
                    Form::Narrow {
                        bits: 32 - c,
                        signed,
                    },
                ));
            }
            Some((v, Form::Shift(kind, c)))
        }
        Op::Bin(BinOp::And, a, b) => {
            let bits = |c: Option<u32>| match c {
                Some(0xff) => Some(8),
                Some(0xffff) => Some(16),
                _ => None,
            };
            let (x, bits) = match (bits(known[a.index()]), bits(known[b.index()])) {
                (_, Some(n)) if known[a.index()].is_none() => (a, n),
                (Some(n), _) if known[b.index()].is_none() => (b, n),
                _ => return None,
            };
            Some((
                x,
                Form::Narrow {
                    bits,
                    signed: false,
                },
            ))
        }
        Op::Adc { a, b, c } => sum(insts, known, a, b, c),
        _ => None,
    }
}

/// An `add_with_carry`'s result (unit `native/lift-plain-arith`), which
/// its flags don't change: `a + b + c` as plain arithmetic over the value
/// it adds to. A subtraction, `a + !y + 1`, is `a - y`; a constant
/// operand (as the block knows it, carried registers included) is added
/// or taken, whichever is smaller, so `a + !k + 1` is `a - k`.
fn sum(
    insts: &[Inst],
    known: &[Option<u32>],
    a: ValueId,
    b: ValueId,
    c: ValueId,
) -> Option<(ValueId, Form)> {
    let Op::Bit(carry) = insts[c.index()].op else {
        // `adc`, `sbc`: plus a carry flag; `adc rN, #0` its carry alone.
        return Some(match (known[a.index()], known[b.index()]) {
            (_, Some(0)) => (a, Form::CarryOnly(c)),
            (Some(0), _) => (b, Form::CarryOnly(c)),
            _ => (a, Form::AddCarry(b, c)),
        });
    };
    let carry = u32::from(carry);
    let not = |x: ValueId| match insts[x.index()].op {
        Op::Not(y) => Some(y),
        _ => None,
    };
    let add = |x: ValueId, k: u32| {
        Some(if k == 0 {
            (x, Form::Copy)
        } else if (k as i32) < 0 {
            (x, Form::SubConst(k.wrapping_neg()))
        } else {
            (x, Form::AddConst(k))
        })
    };
    match (known[a.index()], known[b.index()]) {
        (_, Some(k)) => add(a, k.wrapping_add(carry)),
        // `k + !y + 1`: `k - y`.
        (Some(k), None) => match not(b) {
            Some(y) if carry == 1 => Some((y, Form::ConstSub(k))),
            _ => add(b, k.wrapping_add(carry)),
        },
        (None, None) if carry == 1 => match (not(b), not(a)) {
            (Some(y), _) => Some((a, Form::Sub(y))),
            (None, Some(y)) => Some((b, Form::Sub(y))),
            (None, None) => Some((a, Form::AddCarry(b, c))),
        },
        (None, None) => Some((a, Form::Add(b))),
    }
}

/// The guest's shift pairs in a block's instructions: the address of each
/// `lsr #c` or `asr #c` of the register a `lsl #c` right before it set.
pub fn guest_pairs(insns: &[(u32, Instr)]) -> Vec<u32> {
    let shift = |i: &Instr| match *i {
        Instr::Alu {
            cond: Cond::Al,
            op: AluOp::Mov,
            rd,
            op2: Operand2::Reg { rm, shift },
            ..
        } => Some((rd, rm, shift)),
        _ => None,
    };
    insns
        .windows(2)
        .filter_map(|w| {
            let (d, _, Shift::Lsl(c)) = shift(&w[0].1)? else {
                return None;
            };
            let (_, m, s) = shift(&w[1].1)?;
            (c > 0 && m == d && matches!(s, Shift::Lsr(n) | Shift::Asr(n) if n == c))
                .then_some(w[1].0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `adds r0, r1, #0` and `subs r0, r1, #0` (`r1 + !0 + 1`) print as
    /// `r1`'s value itself.
    #[test]
    fn an_add_of_zero_is_a_copy() {
        use armv4t::sem::Reg;
        let i = |op| Inst { op, guard: None };
        for (k, carry) in [(0, false), (!0, true)] {
            let insts = [
                i(Op::GetReg(Reg(1))),
                i(Op::Const(k)),
                i(Op::Bit(carry)),
                i(Op::Adc {
                    a: ValueId(0),
                    b: ValueId(1),
                    c: ValueId(2),
                }),
            ];
            let known = values(&insts, &BTreeMap::new());
            let (x, f) = form(&insts, &known, 3).unwrap();
            assert_eq!((x, f), (ValueId(0), Form::Copy));
            assert_eq!(f.print_with("v0", &|_| unreachable!()), "v0");
        }
    }

    #[test]
    fn forms_print_as_casts() {
        let n = |bits, signed| Form::Narrow { bits, signed }.print("v1");
        assert_eq!(n(8, false), "v1 as u8 as u32");
        assert_eq!(n(16, true), "v1 as i16 as u32");
        assert_eq!(n(8, true), "v1 as i8 as u32");
        assert_eq!(n(20, false), "v1 & 0xfffff");
        assert_eq!(n(12, true), "prim::sext(v1, 12)");
        assert_eq!(
            Form::Shift(ShiftType::Asr, 3).print("v2"),
            "((v2 as i32) >> 3) as u32"
        );
    }

    /// The casts are what the shifts compute, on values around each edge.
    #[test]
    fn casts_match_the_shifts() {
        for x in [
            0u32,
            1,
            0x7f,
            0x80,
            0xff,
            0x100,
            0x7fff,
            0x8000,
            0xffff_ff80,
            !0,
        ] {
            for c in [16u32, 24] {
                let lsl = x << c;
                assert_eq!(
                    lsl >> c,
                    if c == 24 {
                        x as u8 as u32
                    } else {
                        x as u16 as u32
                    }
                );
                let asr = ((lsl as i32) >> c) as u32;
                assert_eq!(
                    asr,
                    if c == 24 {
                        x as i8 as u32
                    } else {
                        x as i16 as u32
                    }
                );
            }
        }
    }
}
