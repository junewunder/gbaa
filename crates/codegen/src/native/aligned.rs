//! Loads the lift proves aligned print without their rotate (unit
//! `native/lift-aligned-loads`; `docs/research/mirror-machine-count.md`,
//! row 5). The IR gives a misaligned `ldr` or `ldrh` the CPU's rotate,
//! `ror v, (a & 3) << 3` after the load from `a & !3`, and `ldrsh` its
//! shift, `asr sext(v), (a & 1) << 3`: a shift by an amount that is 0
//! whenever `a` is aligned. Such a shift prints as the value it shifts
//! when its amount is proven 0, either way:
//!
//! - **by the address's arithmetic** ([`zero_bits`]): the bits of each
//!   value of a block known to be 0, from constants (the block's own and
//!   the registers it starts with a constant in, [`super::narrow::starts`]),
//!   sp (word-aligned, as the frame's locals already take it: a word
//!   local is a slot at an offset 4 divides, [`super::frame`]) and the
//!   arithmetic between: `(n << 2) + 0x3002560`, `sp + 8`, `x & !3`;
//! - **by a check**: the load's own rotate, where the load reads through
//!   a pointer whose alignment the prelude checks before it reads: a heap
//!   object's field (`Game::at`, an object aligned as its widest field,
//!   at an offset its width divides) or a typed field (`Game::get`, which
//!   stops the program on a pointer its C type's alignment rules out).
//!
//! The metric `load_rotates_left` (barred) counts the shifts by a load
//! address's low bits that still print.

use analysis::ir::{BinOp, Inst, Op, ValueId};
use armv4t::sem::{ShiftType, Width};

/// For each value of a block, the bits known to be 0: `known` its
/// constants ([`super::narrow::values`]); sp is word-aligned at the
/// block's start and after every write of a word-aligned value.
pub fn zero_bits(insts: &[Inst], known: &[Option<u32>]) -> Vec<u32> {
    // Each register's known zero bits: sp's low two.
    let mut regs = [0u32; 16];
    regs[13] = 3;
    let mut kz: Vec<u32> = Vec::with_capacity(insts.len());
    for (i, inst) in insts.iter().enumerate() {
        let z = |x: ValueId| kz[x.index()];
        // The low bits known 0 in both: a sum's.
        let low = |a: u32, b: u32| {
            let n = (!a).trailing_zeros().min((!b).trailing_zeros());
            mask(n)
        };
        let v = match known[i] {
            Some(c) => !c,
            None => match inst.op {
                Op::GetReg(r) => regs[r.index()],
                Op::SetReg(r, x) => {
                    let r = r.index();
                    regs[r] = match inst.guard {
                        None => z(x),
                        Some(_) => regs[r] & z(x),
                    };
                    0
                }
                // A mode change's sp is another bank's.
                Op::SetCpsr { .. } => {
                    regs[13] = 0;
                    0
                }
                Op::Bin(o, a, b) => match o {
                    BinOp::Add | BinOp::Sub => low(z(a), z(b)),
                    BinOp::Mul => {
                        let n = (!z(a)).trailing_zeros() + (!z(b)).trailing_zeros();
                        mask(n.min(32))
                    }
                    BinOp::And => z(a) | z(b),
                    BinOp::Or | BinOp::Xor => z(a) & z(b),
                },
                // `a + b + c`: a sum, `a + !y + 1` a difference, and a
                // constant `b` plus the carry one constant.
                Op::Adc { a, b, c } => match insts[c.index()].op {
                    Op::Bit(false) => low(z(a), z(b)),
                    Op::Bit(true) => match (insts[b.index()].op, known[b.index()]) {
                        (_, Some(k)) => low(z(a), !k.wrapping_add(1)),
                        (Op::Not(y), _) => low(z(a), z(y)),
                        _ => 0,
                    },
                    _ => 0,
                },
                // By the amount's low byte, as `prim::shift` takes it.
                Op::Shift { kind, v: x, amount } => {
                    let x = z(x);
                    match known[amount.index()].map(|n| n & 0xff) {
                        _ if z(amount) | !0xff == u32::MAX => x,
                        Some(n) if n < 32 => match kind {
                            ShiftType::Lsl => x << n | mask(n),
                            ShiftType::Lsr => x >> n | !(u32::MAX >> n),
                            ShiftType::Asr if x & 1 << 31 != 0 => x >> n | !(u32::MAX >> n),
                            ShiftType::Asr => x >> n,
                            ShiftType::Ror => x.rotate_right(n),
                        },
                        Some(_) => match kind {
                            ShiftType::Lsl | ShiftType::Lsr => u32::MAX,
                            ShiftType::Asr if x & 1 << 31 != 0 => u32::MAX,
                            _ => 0,
                        },
                        None => 0,
                    }
                }
                Op::Sext { v: x, bits } if bits < 32 => z(x) & mask(bits),
                Op::Select { a, b, .. } => z(a) & z(b),
                Op::Load { width, .. } => match width {
                    Width::Byte => !0xff,
                    Width::Half => !0xffff,
                    Width::Word => 0,
                },
                _ => 0,
            },
        };
        kz.push(v);
    }
    kz
}

/// The low `n` bits.
fn mask(n: u32) -> u32 {
    if n >= 32 { u32::MAX } else { (1 << n) - 1 }
}

/// Each shift of a block that prints as the value it shifts, its amount
/// proven 0 ([`zero_bits`], `kz`), or a load's own rotate where `checked`
/// marks the load as one whose address the prelude checks aligned.
pub fn zero_shifts(insts: &[Inst], kz: &[u32], checked: &[bool]) -> Vec<bool> {
    (0..insts.len())
        .map(|i| {
            let Op::Shift { v, amount, .. } = insts[i].op else {
                return false;
            };
            kz[amount.index()] | !0xff == u32::MAX
                || load_shift(insts, v, amount).is_some_and(|l| checked[l])
        })
        .collect()
}

/// The load a shift of `v` by `amount` is the rotate (or `ldrsh`'s
/// shift) of: `amount` is `(a & m) << 3`, `v` the load from `a` or its
/// sign extension, `m` 1 for a halfword, 3 for a word.
pub fn load_shift(insts: &[Inst], v: ValueId, amount: ValueId) -> Option<usize> {
    let l = match insts[v.index()].op {
        Op::Sext { v: x, .. } => x.index(),
        _ => v.index(),
    };
    let Op::Load { width, addr } = insts[l].op else {
        return None;
    };
    let m = match width {
        Width::Byte => return None,
        Width::Half => 1,
        Width::Word => 3,
    };
    let konst = |x: ValueId| match insts[x.index()].op {
        Op::Const(c) | Op::Pool(c) => Some(c),
        _ => None,
    };
    let Op::Shift {
        kind: ShiftType::Lsl,
        v: low,
        amount: three,
    } = insts[amount.index()].op
    else {
        return None;
    };
    let Op::Bin(BinOp::And, a, k) = insts[low.index()].op else {
        return None;
    };
    (konst(three) == Some(3) && a == addr && konst(k).is_some_and(|k| k & !m == 0)).then_some(l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use analysis::ir::Inst;
    use armv4t::sem::Reg;

    fn inst(op: Op) -> Inst {
        Inst { op, guard: None }
    }

    /// `v` loaded from `addr`'s word and rotated by its low bits: the
    /// rotate's index.
    fn rotated(b: &mut Vec<Inst>, addr: ValueId) -> usize {
        let at = |b: &Vec<Inst>| ValueId(b.len() as u32);
        let n3 = at(b);
        b.push(inst(Op::Const(3)));
        let l = at(b);
        b.push(inst(Op::Load {
            width: Width::Word,
            addr,
        }));
        let lo = at(b);
        b.push(inst(Op::Bin(BinOp::And, addr, n3)));
        let amt = at(b);
        b.push(inst(Op::Shift {
            kind: ShiftType::Lsl,
            v: lo,
            amount: n3,
        }));
        b.push(inst(Op::Shift {
            kind: ShiftType::Ror,
            v: l,
            amount: amt,
        }));
        b.len() - 1
    }

    fn shifts(b: &[Inst], checked: &[usize]) -> Vec<bool> {
        let none = std::collections::BTreeMap::new();
        let known = super::super::narrow::values(b, &none);
        let kz = zero_bits(b, &known);
        let mut c = vec![false; b.len()];
        for &i in checked {
            c[i] = true;
        }
        zero_shifts(b, &kz, &c)
    }

    #[test]
    fn aligned_by_arithmetic() {
        // `(n << 2) + 0x3002560`: a word's address.
        let mut b = vec![
            inst(Op::GetReg(Reg(0))),
            inst(Op::Const(2)),
            inst(Op::Shift {
                kind: ShiftType::Lsl,
                v: ValueId(0),
                amount: ValueId(1),
            }),
            inst(Op::Const(0x0300_2560)),
            inst(Op::Bin(BinOp::Add, ValueId(2), ValueId(3))),
        ];
        let r = rotated(&mut b, ValueId(4));
        assert!(shifts(&b, &[])[r]);
        // sp + 8, and sp + 6 not.
        let mut b = vec![
            inst(Op::GetReg(Reg::SP)),
            inst(Op::Const(8)),
            inst(Op::Bin(BinOp::Add, ValueId(0), ValueId(1))),
            inst(Op::Const(6)),
            inst(Op::Bin(BinOp::Add, ValueId(0), ValueId(3))),
        ];
        let r8 = rotated(&mut b, ValueId(2));
        let r6 = rotated(&mut b, ValueId(4));
        let s = shifts(&b, &[]);
        assert!(s[r8] && !s[r6]);
        // A register's own value proves nothing, nor sp after r0 is
        // written to it.
        let mut b = vec![
            inst(Op::GetReg(Reg(4))),
            inst(Op::GetReg(Reg(0))),
            inst(Op::SetReg(Reg::SP, ValueId(1))),
            inst(Op::GetReg(Reg::SP)),
        ];
        let r4 = rotated(&mut b, ValueId(0));
        let sp = rotated(&mut b, ValueId(3));
        let s = shifts(&b, &[]);
        assert!(!s[r4] && !s[sp]);
    }

    #[test]
    fn aligned_by_a_check() {
        // r4 + 8 through a checked pointer: its own rotate goes.
        let mut b = vec![
            inst(Op::GetReg(Reg(4))),
            inst(Op::Const(8)),
            inst(Op::Bin(BinOp::Add, ValueId(0), ValueId(1))),
        ];
        let r = rotated(&mut b, ValueId(2));
        let Op::Shift { v: l, .. } = b[r].op else {
            unreachable!()
        };
        assert!(!shifts(&b, &[])[r]);
        assert!(shifts(&b, &[l.index()])[r]);
    }
}
