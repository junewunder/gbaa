//! Register and flag liveness over one function's blocks, as the native
//! lift passes them (unit `native/lift-dead-writes`): the writes of r0-r12
//! and of N, Z, C and V nothing printed reads before they are written
//! again or the function's are gone, which the lift leaves out.
//!
//! What follows each exit is the caller's to say ([`dead`]'s `out`): the
//! function's blocks it may go on at, and the registers and flags read
//! after it besides theirs. As the lift prints them: a return reads its
//! results only (callers keep r4-r11 in their own locals, and get no
//! flags); a direct call reads its parameters and the r4-r11 its callee
//! takes, and leaves the caller's flags as they were; a fall or jump into
//! another function's extended entry reads everything. r13-r15 are never
//! left out: sp and lr reach the printed code by name (`bank_sp`, calls).
//!
//! Within a block the walk is backwards: an unguarded write kills, a
//! guarded one doesn't (the old value survives its false path), a read
//! makes live, `mrs` reads every flag, an unguarded `msr` of the flags
//! writes all four. The same walk as [`analysis::ir::flags`]'s, with the
//! registers too and no IRQ yield (natively a handler is a call of its
//! own, which sees none of the interrupted function's locals).

use analysis::ir::{Inst, Op};
use armv4t::sem::{Flag, PsrMask};

/// Registers and flags: bit `n` for rN, bit 16 + [`Flag::index`] for a
/// flag.
pub type Live = u32;

/// The four flags.
pub const FLAGS: Live = 0xf << 16;
/// Every register and flag.
pub const ALL: Live = 0xffff | FLAGS;
/// sp, lr and pc: never left out.
const KEPT: Live = 0xe000;

/// The bit of register `r`.
pub fn reg(r: usize) -> Live {
    1 << r
}

/// The bit of flag `f`.
pub fn flag(f: Flag) -> Live {
    1 << (16 + f.index())
}

/// The registers of `m`, bit `n` for rN of r0-r3.
pub fn low(m: u8) -> Live {
    Live::from(m & 0xf)
}

/// The registers of `m`, bit `n - 4` for rN of r4-r11.
pub fn high(m: u8) -> Live {
    Live::from(m) << 4
}

/// Walk `insts` backwards with `out(i)` live after the exit at inst `i`:
/// call `dead(i)` for each write of a register (but r13-r15) or flag
/// nothing reads after it, and return what is live at entry.
fn walk(insts: &[Inst], out: impl Fn(usize) -> Live, mut dead: impl FnMut(usize)) -> Live {
    let mut live: Live = 0;
    for (i, inst) in insts.iter().enumerate().rev() {
        let guarded = inst.guard.is_some();
        let mut write = |bit: Live, live: &mut Live| {
            if *live & bit == 0 && bit & KEPT == 0 {
                dead(i);
            } else if !guarded {
                *live &= !bit;
            }
        };
        match inst.op {
            Op::Exit(_) if guarded => live |= out(i),
            Op::Exit(_) => live = out(i),
            Op::SetReg(r, _) => write(reg(r.index()), &mut live),
            Op::SetFlag(f, _) => write(flag(f), &mut live),
            Op::GetReg(r) => live |= reg(r.index()),
            Op::GetFlag(f) => live |= flag(f),
            // `mrs_cpsr` takes the flags; in user and system mode, which
            // have no SPSR, `GetSpsr` reads the CPSR.
            Op::GetCpsr | Op::GetSpsr => live |= FLAGS,
            Op::SetCpsr { mask, .. } if !guarded && mask.contains(PsrMask::FLAGS) => live &= !FLAGS,
            _ => {}
        }
    }
    live
}

/// The dead writes of a function's blocks (`blocks[b]`, a block's insts):
/// `dead[b][i]` for a `SetReg` or `SetFlag` whose register or flag
/// nothing reads after it. `out(b, i)` says what follows the exit at
/// inst `i` of block `b`: the blocks it may go on at, and what is read
/// after it besides what they read ([`ALL`] where it may reach code that
/// reads anything).
pub fn dead(
    blocks: &[&[Inst]],
    out: impl Fn(usize, usize) -> (Vec<usize>, Live),
) -> Vec<Vec<bool>> {
    let n = blocks.len();
    // The least fixpoint: nothing live, growing.
    let mut live_in: Vec<Live> = vec![0; n];
    let after = |live_in: &[Live], b: usize, i: usize| {
        let (succ, extra) = out(b, i);
        succ.iter().fold(extra, |l, &s| l | live_in[s])
    };
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..n).rev() {
            let l = walk(blocks[b], |i| after(&live_in, b, i), |_| {});
            if l != live_in[b] {
                live_in[b] = l;
                changed = true;
            }
        }
    }
    blocks
        .iter()
        .enumerate()
        .map(|(b, insts)| {
            let mut dead = vec![false; insts.len()];
            walk(insts, |i| after(&live_in, b, i), |i| dead[i] = true);
            dead
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use analysis::ir::{Exit, ValueId};
    use armv4t::sem::Reg;

    fn inst(op: Op) -> Inst {
        Inst { op, guard: None }
    }

    #[test]
    fn a_flag_and_a_register_a_return_does_not_read_are_dead() {
        // v0 = 1; r4 = v0; N = v0 != 0; Z = ...; return (reads r0).
        let one = ValueId(0);
        let block = [
            inst(Op::Const(1)),
            inst(Op::SetReg(Reg(4), one)),
            inst(Op::IsZero(one)),
            inst(Op::SetFlag(Flag::Z, ValueId(2))),
            inst(Op::SetReg(Reg(0), one)),
            inst(Op::SetReg(Reg(13), one)),
            inst(Op::Exit(Exit::Jump(one))),
        ];
        let dead = dead(&[&block], |_, _| (Vec::new(), reg(0)));
        assert_eq!(
            dead[0],
            [false, true, false, true, false, false, false],
            "r4 and Z are dead, r0 is returned, sp is kept"
        );
    }

    #[test]
    fn a_flag_a_later_block_reads_is_live_and_a_guarded_write_does_not_kill() {
        let one = ValueId(0);
        let z = ValueId(1);
        // Block 0 sets C, then C again under a guard, and goes to 1, which
        // reads C; block 1 then returns.
        let b0 = [
            inst(Op::Bit(true)),
            inst(Op::SetFlag(Flag::C, one)),
            Inst {
                op: Op::SetFlag(Flag::C, one),
                guard: Some(one),
            },
            inst(Op::Exit(Exit::Jump(one))),
        ];
        let b1 = [
            inst(Op::GetFlag(Flag::C)),
            inst(Op::SetFlag(Flag::N, z)),
            inst(Op::Exit(Exit::Jump(one))),
        ];
        let dead = dead(&[&b0, &b1], |b, _| {
            if b == 0 {
                (vec![1], 0)
            } else {
                (Vec::new(), 0)
            }
        });
        assert_eq!(dead[0], [false, false, false, false]);
        assert_eq!(dead[1], [false, true, false]);
        // A fall into another function reads everything.
        let dead = super::dead(&[&b1], |_, _| (Vec::new(), ALL));
        assert_eq!(dead[0], [false, false, false]);
    }
}
