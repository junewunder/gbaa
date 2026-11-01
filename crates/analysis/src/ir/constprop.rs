//! Constant propagation (decision 9; `docs/sem-design.md`, "What flag
//! liveness and const-prop need"), forward over one block.
//!
//! - A pure op whose operands are all constants becomes a constant, with
//!   the same evaluation the reference evaluator uses ([`Op::eval_pure`]).
//! - A few identities fold with one operand known: `x + 0`, `x & 0`,
//!   `x & !0`, a shift by 0, a `select` on a known condition or with equal
//!   arms, and their bit forms.
//! - A guard known true is dropped; an inst under a guard known false is
//!   deleted. An exit whose guard folds to true ends the block there.
//! - A `Load` from a constant address in the cartridge ROM ([`Rom`]) is
//!   followed by a constant with the ROM's value, which its users read
//!   instead, so literal pools fold into the code that uses them. The load
//!   itself stays: the runtime counts its wait states. A word's constant
//!   is an [`Op::Pool`], so a named print can tell a pool word.
//!
//! Values entering the block (`GetReg`) aren't known: only the builder's
//! forwarding links one instruction's result to the next instruction's read.

use super::{BinOp, BitOp, Block, Inst, Op, ValueId};
use armv4t::sem::Width;

/// The cartridge ROM's bytes, which const-prop may read loads from. Empty
/// (`Rom::NONE`) folds no loads.
#[derive(Clone, Copy, Debug)]
pub struct Rom<'a>(pub &'a [u8]);

/// The ROM window's size: each of its regions mirrors the ROM at this
/// period (`runtime::bus`).
const ROM_MAX: usize = 32 << 20;

/// ROM offsets that read the cartridge's GPIO port when it's enabled
/// (`runtime::gpio`), not the ROM.
const GPIO: std::ops::Range<usize> = 0xC4..0xCA;

impl Rom<'_> {
    pub const NONE: Rom<'static> = Rom(&[]);

    /// What a load of width `w` at `addr` reads, if it is always the same:
    /// a load inside the ROM image, in regions `0x08` to `0x0C` (`0x0D` may
    /// be the EEPROM), away from the GPIO port. `runtime::bus::Bus::read*`
    /// is the reference.
    pub fn load(&self, w: Width, addr: u32) -> Option<u32> {
        let n = w.bytes() as usize;
        let addr = addr & !(w.bytes() - 1);
        if !(0x08..=0x0C).contains(&(addr >> 24)) {
            return None;
        }
        let o = addr as usize & (ROM_MAX - 1);
        if o < GPIO.end && GPIO.start < o + n {
            return None;
        }
        let bytes = self.0.get(o..o + n)?;
        Some(bytes.iter().rev().fold(0, |acc, &b| acc << 8 | b as u32))
    }
}

/// What an op folds to.
enum Fold {
    Const(u32),
    /// The value of one of its operands.
    Same(ValueId),
    Keep,
}

/// Fold `op` given what is known of its operands.
fn fold(op: &Op, k: impl Fn(ValueId) -> Option<u32>) -> Fold {
    let ops = op.operands();
    if ops.iter().all(|&v| k(v).is_some())
        && let Some(x) = op.eval_pure(|v| k(v).unwrap(), |v| k(v).unwrap() != 0)
    {
        return Fold::Const(x);
    }
    identity(op, &k)
}

/// What `op` folds to with one operand known.
fn identity(op: &Op, k: &impl Fn(ValueId) -> Option<u32>) -> Fold {
    let is = |v: ValueId, x: u32| k(v) == Some(x);
    use BinOp::*;
    match *op {
        Op::Bin(Add | Sub | Or | Xor, a, b) if is(b, 0) => Fold::Same(a),
        Op::Bin(Add | Or | Xor, a, b) if is(a, 0) => Fold::Same(b),
        Op::Bin(And | Mul, a, b) if is(a, 0) || is(b, 0) => Fold::Const(0),
        Op::Bin(And, a, b) if is(b, !0) => Fold::Same(a),
        Op::Bin(And, a, b) if is(a, !0) => Fold::Same(b),
        Op::Bin(Mul, a, b) if is(b, 1) => Fold::Same(a),
        Op::Bin(Mul, a, b) if is(a, 1) => Fold::Same(b),
        // `prim::shift` by 0 (mod 256) returns the value and the carry in.
        Op::Shift { v, amount, .. } if k(amount).is_some_and(|n| n & 0xFF == 0) => Fold::Same(v),
        Op::ShiftCarry {
            amount, carry_in, ..
        } if k(amount).is_some_and(|n| n & 0xFF == 0) => Fold::Same(carry_in),
        Op::Select { c, a, b } | Op::BSelect { c, a, b } => match k(c) {
            Some(0) => Fold::Same(b),
            Some(_) => Fold::Same(a),
            None if a == b => Fold::Same(a),
            None => Fold::Keep,
        },
        Op::BBin(BitOp::And, a, b) if is(a, 0) || is(b, 0) => Fold::Const(0),
        Op::BBin(BitOp::Or, a, b) if is(a, 1) || is(b, 1) => Fold::Const(1),
        Op::BBin(BitOp::And, a, b) if is(b, 1) => Fold::Same(a),
        Op::BBin(BitOp::And, a, b) if is(a, 1) => Fold::Same(b),
        Op::BBin(BitOp::Or | BitOp::Xor, a, b) if is(b, 0) => Fold::Same(a),
        Op::BBin(BitOp::Or | BitOp::Xor, a, b) if is(a, 0) => Fold::Same(b),
        _ => Fold::Keep,
    }
}

impl Block {
    /// Propagate constants forward through the block, reading loads from
    /// `rom`, and renumber. Leaves dead pure ops behind for [`Block::dce`].
    pub fn const_prop(&mut self, rom: Rom) {
        let old = std::mem::take(&mut self.insts);
        // Each old value's new id, and what each new value is known to be.
        let mut map: Vec<ValueId> = Vec::with_capacity(old.len());
        let mut known: Vec<Option<u32>> = Vec::with_capacity(old.len());
        let mut out: Vec<Inst> = Vec::with_capacity(old.len());
        fn push(
            out: &mut Vec<Inst>,
            known: &mut Vec<Option<u32>>,
            op: Op,
            guard: Option<ValueId>,
            k: Option<u32>,
        ) -> ValueId {
            out.push(Inst { op, guard });
            known.push(k);
            ValueId(out.len() as u32 - 1)
        }
        for inst in old {
            let mut op = inst.op;
            for v in op.operands_mut() {
                *v = map[v.index()];
            }
            let guard = inst.guard.map(|g| map[g.index()]);
            let guard = match guard.map(|g| known[g.index()]) {
                // Never runs. Only a guarded load has a value, and only
                // insts under the same guard (deleted too) or selects on it
                // (folded to their other arm) used it.
                Some(Some(0)) => {
                    map.push(push(&mut out, &mut known, Op::Const(0), None, None));
                    continue;
                }
                Some(Some(_)) => None,
                _ => guard,
            };
            let ends = matches!(op, Op::Exit(_)) && guard.is_none();
            let id = match fold(&op, |v| known[v.index()]) {
                Fold::Const(x) if op.is_pure_bit() => {
                    push(&mut out, &mut known, Op::Bit(x != 0), None, Some(x))
                }
                // A pool word, or an identity of one (an ARM `ldr`'s
                // rotate by 0), stays a pool word.
                Fold::Const(x)
                    if matches!(op, Op::Pool(_))
                        || matches!(identity(&op, &|v| known[v.index()]),
                            Fold::Same(v) if matches!(out[v.index()].op, Op::Pool(_))) =>
                {
                    push(&mut out, &mut known, Op::Pool(x), None, Some(x))
                }
                Fold::Const(x) => push(&mut out, &mut known, Op::Const(x), None, Some(x)),
                Fold::Same(v) => v,
                Fold::Keep => {
                    let id = push(&mut out, &mut known, op, guard, None);
                    let rom_value = match op {
                        Op::Load { width, addr } => {
                            known[addr.index()].and_then(|a| rom.load(width, a))
                        }
                        _ => None,
                    };
                    // A word is a pool word's value: named prints may
                    // name it.
                    let c = |x| match op {
                        Op::Load {
                            width: Width::Word, ..
                        } => Op::Pool(x),
                        _ => Op::Const(x),
                    };
                    match rom_value {
                        Some(x) => push(&mut out, &mut known, c(x), None, Some(x)),
                        None => id,
                    }
                }
            };
            map.push(id);
            if ends {
                break;
            }
        }
        self.insts = out;
    }
}
