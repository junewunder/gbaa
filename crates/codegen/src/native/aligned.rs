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
//!   and from the registers' bits known 0 as the block starts ([`starts`],
//!   unit `native/lift-aligned-params`): what every edge into it leaves,
//!   over the function's blocks (a pointer stepped by 4 round a loop
//!   stays a word's), and at its entry what every caller passes, where
//!   every way into it is a direct call from its own code context
//!   (`native::aligned_entries`: none for a function a pointer, a
//!   stand-in, a jump, a fall or an unwind may enter). Such a load prints
//!   with no `& !3` (`ld32(g, p)`), and a guest-pc build's `ld32` and
//!   `ld16` stop the program on an address they don't find aligned, so a
//!   wrong proof shows on the native check (`gbaa-lift --plant-aligned`,
//!   c-align's planted control);
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

/// Each register's bits known to be 0 (r0-r15).
pub type Regs = [u32; 16];

/// Nothing known: sp alone word-aligned, as every block takes it.
pub const NONE: Regs = {
    let mut r = [0; 16];
    r[13] = 3;
    r
};

/// For each value of a block, the bits known to be 0: `known` its
/// constants ([`super::narrow::values`]), `start` the registers' as the
/// block starts ([`starts`]); sp is word-aligned at the block's start and
/// after every write of a word-aligned value.
pub fn zero_bits(insts: &[Inst], known: &[Option<u32>], start: &Regs) -> Vec<u32> {
    run(insts, known, start, insts.len()).0
}

/// The registers' known-zero bits after `insts[..stop]`, from `start`.
pub fn regs_at(insts: &[Inst], known: &[Option<u32>], start: &Regs, stop: usize) -> Regs {
    run(insts, known, start, stop).1
}

fn run(insts: &[Inst], known: &[Option<u32>], start: &Regs, stop: usize) -> (Vec<u32>, Regs) {
    let mut regs = *start;
    regs[13] |= 3;
    let mut kz: Vec<u32> = Vec::with_capacity(stop);
    for (i, inst) in insts[..stop].iter().enumerate() {
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
    (kz, regs)
}

/// How an edge between a function's blocks leaves the registers: as the
/// block left them by its exit inst (a jump); as a call leaves them
/// (r4-r11 and sp, `saves::KEPT`, less `clear`: those a callee gives
/// back); or nothing known.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub from: u32,
    pub exit: usize,
    pub to: u32,
    pub after: super::narrow::After,
    /// After a call: the registers it may change (a giver's r4-r11).
    pub clear: u16,
    /// A BIOS call's (`swi`): kept as a call keeps them here, nothing
    /// known after it in the constants' flow ([`super::narrow::starts`]).
    pub bios: bool,
}

/// Each block's registers' known-zero bits at its start
/// (native/lift-aligned-params): `entry`'s at the function's entry
/// (its callers' facts, [`NONE`] where they aren't all known), and at
/// every block what each edge in leaves, met (the bits all agree are 0).
/// A block no edge reaches isn't listed. `edges` must be every edge
/// between the function's blocks; `block` gives a block's IR and
/// constants. A block of `pinned` (one an unwind lands in, with
/// registers no edge brings) starts with nothing known.
pub fn starts<'a>(
    entry: u32,
    at_entry: Regs,
    edges: &[Edge],
    pinned: &std::collections::BTreeSet<u32>,
    block: impl Fn(u32) -> (&'a [Inst], &'a [Option<u32>]),
) -> std::collections::BTreeMap<u32, Regs> {
    use super::narrow::After;
    let first = if pinned.contains(&entry) {
        NONE
    } else {
        at_entry
    };
    let mut at: std::collections::BTreeMap<u32, Regs> = [(entry, first)].into();
    loop {
        let mut next: std::collections::BTreeMap<u32, Regs> = [(entry, first)].into();
        for e in edges {
            let Some(from) = at.get(&e.from) else {
                continue;
            };
            let out = match e.after {
                After::Unknown => NONE,
                after => {
                    let (insts, known) = block(e.from);
                    let mut r = regs_at(insts, known, from, e.exit);
                    if after == After::KeptCall {
                        for (i, z) in r.iter_mut().enumerate() {
                            if crate::saves::KEPT & !e.clear & 1 << i == 0 {
                                *z = 0;
                            }
                        }
                    }
                    r
                }
            };
            next.entry(e.to)
                .and_modify(|r| {
                    for (a, b) in r.iter_mut().zip(out) {
                        *a &= b;
                    }
                })
                .or_insert(out);
        }
        for (k, r) in next.iter_mut() {
            if pinned.contains(k) {
                *r = NONE;
            }
        }
        if next == at {
            return at;
        }
        at = next;
    }
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
        let kz = zero_bits(b, &known, &NONE);
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

    /// A loop of one block: `r0 = r0 + step`, back to itself; the entry
    /// passes `r0` with `entry` bits known 0. What the block starts knowing.
    fn looped(entry: u32, step: u32, after: super::super::narrow::After) -> Regs {
        let b = vec![
            inst(Op::GetReg(Reg(0))),
            inst(Op::Const(step)),
            inst(Op::Bin(BinOp::Add, ValueId(0), ValueId(1))),
            inst(Op::SetReg(Reg(0), ValueId(2))),
            inst(Op::GetReg(Reg(4))),
        ];
        let known = super::super::narrow::values(&b, &std::collections::BTreeMap::new());
        let mut at = NONE;
        at[0] = entry;
        at[4] = 3;
        let edges = [Edge {
            from: 0x100,
            exit: 4,
            to: 0x100,
            after,
            clear: 0,
            bios: false,
        }];
        starts(0x100, at, &edges, &Default::default(), |_| (&b, &known))[&0x100]
    }

    #[test]
    fn aligned_across_blocks() {
        use super::super::narrow::After;
        // p += 4 round a loop keeps a word-aligned p; p += 2 keeps its
        // low bit only; nothing known on the way in, nothing after.
        assert_eq!(looped(3, 4, After::Jump)[0], 3);
        assert_eq!(looped(3, 2, After::Jump)[0], 1);
        assert_eq!(looped(0, 4, After::Jump)[0], 0);
        // A call clears r0-r3 and keeps r4-r11.
        let r = looped(3, 4, After::KeptCall);
        assert_eq!((r[0], r[4]), (0, 3));
        // An unwind's landing knows nothing.
        let b: Vec<Inst> = vec![inst(Op::GetReg(Reg(4)))];
        let mut at = NONE;
        at[4] = 3;
        let pinned = [0x100].into();
        let s = starts(0x100, at, &[], &pinned, |_| (&b, &[None][..]));
        assert_eq!(s[&0x100], NONE);
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
