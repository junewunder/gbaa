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
//! makes live if something kept takes its value (not only a dead write), `mrs` reads every flag, an unguarded `msr` of the flags
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
/// nothing reads after it, and return what is live at entry. A read of a
/// register or flag makes it live only if something kept takes its value
/// (through pure ops): an effect, an exit, a guard, or a write that isn't
/// dead; so a read only a dead write took (a shift's carry in, kept for
/// a C nobody reads) doesn't (unit `native/lift-plain-arith`).
fn walk(insts: &[Inst], out: impl Fn(usize) -> Live, mut dead: impl FnMut(usize)) -> Live {
    let mut live: Live = 0;
    let mut needed = vec![false; insts.len()];
    for (i, inst) in insts.iter().enumerate().rev() {
        let guarded = inst.guard.is_some();
        // Whether the inst is kept, so that what it takes is needed.
        let mut kept = true;
        let mut write = |bit: Live, live: &mut Live| {
            if *live & bit == 0 && bit & KEPT == 0 {
                dead(i);
                kept = false;
            } else if !guarded {
                *live &= !bit;
            }
        };
        match inst.op {
            Op::Exit(_) if guarded => live |= out(i),
            Op::Exit(_) => live = out(i),
            Op::SetReg(r, _) => write(reg(r.index()), &mut live),
            Op::SetFlag(f, _) => write(flag(f), &mut live),
            Op::GetReg(r) => {
                kept = needed[i];
                if kept {
                    live |= reg(r.index());
                }
            }
            Op::GetFlag(f) => {
                kept = needed[i];
                if kept {
                    live |= flag(f);
                }
            }
            // `mrs_cpsr` takes the flags; in user and system mode, which
            // have no SPSR, `GetSpsr` reads the CPSR.
            Op::GetCpsr | Op::GetSpsr => live |= FLAGS,
            Op::SetCpsr { mask, .. } if !guarded && mask.contains(PsrMask::FLAGS) => live &= !FLAGS,
            // The flags an IRQ would see: the lift prints none (no IRQ
            // yield), so what it takes isn't read.
            Op::YieldFlag(..) => kept = false,
            ref op if pure(op) => kept = needed[i],
            _ => {}
        }
        if kept {
            for x in inst.op.operands().into_iter().chain(inst.guard) {
                needed[x.index()] = true;
            }
        }
    }
    live
}

/// Whether `op` has no effect and reads no CPU state: kept only for what
/// takes its value.
fn pure(op: &Op) -> bool {
    !matches!(
        op,
        Op::Insn(_)
            | Op::Timing(_)
            | Op::YieldFlag(..)
            | Op::SetReg(..)
            | Op::SetFlag(..)
            | Op::GetReg(_)
            | Op::GetFlag(_)
            | Op::GetUserReg(_)
            | Op::SetUserReg(..)
            | Op::GetCpsr
            | Op::GetSpsr
            | Op::SetCpsr { .. }
            | Op::SetSpsr { .. }
            | Op::Load { .. }
            | Op::Store { .. }
            | Op::Exit(_)
    )
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
    dead_and_live(blocks, out).0
}

/// [`dead`], and what is live at each block's start.
pub fn dead_and_live(
    blocks: &[&[Inst]],
    out: impl Fn(usize, usize) -> (Vec<usize>, Live),
) -> (Vec<Vec<bool>>, Vec<Live>) {
    dead_and_live_killing(blocks, out, |_, _| 0)
}

/// [`dead_and_live`], with what the exit at inst `i` of block `b` itself
/// sets as printed, `kills(b, i)` (a call's results, a BIOS call's): not
/// live before it for the blocks it goes on at, though `out`'s own reads
/// still are.
pub fn dead_and_live_killing(
    blocks: &[&[Inst]],
    out: impl Fn(usize, usize) -> (Vec<usize>, Live),
    kills: impl Fn(usize, usize) -> Live,
) -> (Vec<Vec<bool>>, Vec<Live>) {
    let n = blocks.len();
    // The least fixpoint: nothing live, growing.
    let mut live_in: Vec<Live> = vec![0; n];
    let after = |live_in: &[Live], b: usize, i: usize| {
        let (succ, extra) = out(b, i);
        let on = succ.iter().fold(0, |l, &s| l | live_in[s]);
        on & !(kills(b, i) & !KEPT) | extra
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
    let dead = blocks
        .iter()
        .enumerate()
        .map(|(b, insts)| {
            let mut dead = vec![false; insts.len()];
            walk(insts, |i| after(&live_in, b, i), |i| dead[i] = true);
            dead
        })
        .collect();
    (dead, live_in)
}

/// A write of a register whose value only its own block reads
/// ([`block_local`]): what reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Local {
    /// Its `GetReg`s (the block's insts) before the register is written
    /// again.
    pub reads: Vec<usize>,
    /// Whether an exit that prints its reads through the block's names
    /// reads it.
    pub by_exit: bool,
}

/// The writes of r0-r12 whose value only its own block reads, by
/// `GetReg`s and its exits, before the register is written again (unit
/// `native/lift-register-locals`): `local[b][i]` for an unguarded
/// `SetReg` with no exit after it, up to the next unguarded write of its
/// register, after which the register is live (`out` and `live_in` as
/// [`dead_and_live`] takes and gives them), and no guarded write of it
/// there (the old value would join the new). An exit `by_name(b, i)`
/// gives `Some(kills)` for one that prints its own reads through the
/// block's names (a direct call's arguments, a return's results), and
/// what it sets (a call's results): only what is live after it but those
/// flows on. The lift prints such a value, where something printed reads
/// it, as a `let` of its own, not a write of the register's local.
pub fn block_local(
    blocks: &[&[Inst]],
    live_in: &[Live],
    out: impl Fn(usize, usize) -> (Vec<usize>, Live),
    by_name: impl Fn(usize, usize) -> Option<Live>,
) -> Vec<Vec<Option<Local>>> {
    // What flows on past exit `i`, and what it reads by name.
    let after = |b: usize, i: usize| {
        let (succ, extra) = out(b, i);
        let on = succ.iter().fold(0, |l, &s| l | live_in[s]);
        match by_name(b, i) {
            Some(kills) => (on & !kills, extra),
            None => (on | extra, 0),
        }
    };
    blocks
        .iter()
        .enumerate()
        .map(|(b, insts)| {
            (0..insts.len())
                .map(|i| {
                    let r = match insts[i].op {
                        Op::SetReg(r, _) if insts[i].guard.is_none() && r.index() < 13 => r.index(),
                        _ => return None,
                    };
                    let mut l = Local::default();
                    for (j, inst) in insts.iter().enumerate().skip(i + 1) {
                        match inst.op {
                            Op::GetReg(q) if q.index() == r => l.reads.push(j),
                            Op::SetReg(q, _) if q.index() == r => {
                                return inst.guard.is_none().then_some(l);
                            }
                            Op::Exit(_) => {
                                let (on, named) = after(b, j);
                                if on & reg(r) != 0 {
                                    return None;
                                }
                                l.by_exit |= named & reg(r) != 0;
                                if inst.guard.is_none() {
                                    return Some(l);
                                }
                            }
                            _ => {}
                        }
                    }
                    // The block's end with no exit: nothing follows.
                    Some(l)
                })
                .collect()
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
        let z = ValueId(0);
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
            inst(Op::Exit(Exit::Jump(ValueId(0)))),
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
        // A read only a dead write takes makes nothing live: here C goes
        // only to N, which nothing reads.
        let b2 = [
            inst(Op::GetFlag(Flag::C)),
            inst(Op::SetFlag(Flag::N, ValueId(0))),
            inst(Op::Const(0)),
            inst(Op::Exit(Exit::Jump(ValueId(2)))),
        ];
        let (dead, live_in) = super::dead_and_live(&[&b2], |_, _| (Vec::new(), 0));
        assert_eq!(
            (&dead[0][..], &live_in[..]),
            (&[false, true, false, false][..], &[0][..])
        );
        // A fall into another function reads everything.
        let dead = super::dead(&[&b1], |_, _| (Vec::new(), ALL));
        assert_eq!(dead[0], [false, false, false]);
    }
    #[test]
    fn a_value_only_its_block_reads_is_local() {
        let one = ValueId(0);
        // r1 = 1; r1 read; r2 = 1 (read by the jump's next block); exit.
        let b0 = [
            inst(Op::Const(1)),
            inst(Op::SetReg(Reg(1), one)),
            inst(Op::GetReg(Reg(1))),
            inst(Op::SetReg(Reg(2), one)),
            Inst {
                op: Op::SetReg(Reg(3), one),
                guard: Some(one),
            },
            inst(Op::SetReg(Reg(4), one)),
            Inst {
                op: Op::SetReg(Reg(4), one),
                guard: Some(one),
            },
            inst(Op::Exit(Exit::Jump(one))),
        ];
        let b1 = [inst(Op::GetReg(Reg(2))), inst(Op::Exit(Exit::Jump(one)))];
        let out = |b: usize, _: usize| {
            if b == 0 {
                (vec![1], 0)
            } else {
                (Vec::new(), 0)
            }
        };
        let (_, live_in) = dead_and_live(&[&b0, &b1], out);
        let local = block_local(&[&b0, &b1], &live_in, out, |_, _| None);
        let is: Vec<bool> = local[0].iter().map(Option::is_some).collect();
        assert_eq!(local[0][1].as_ref().map(|l| l.reads.clone()), Some(vec![2]));
        assert_eq!(
            is,
            [false, true, false, false, false, false, false, false],
            "r1 is the block's own; r2 is read after the jump; a guarded write \
             is never one; r4's value joins a guarded write's"
        );
        // A call that reads r0 and r1 by name and sets r0, whose return
        // site reads r0 and r1: r0's argument is the block's own, r1's
        // goes on.
        let call = [
            inst(Op::Const(1)),
            inst(Op::SetReg(Reg(0), one)),
            inst(Op::SetReg(Reg(1), one)),
            inst(Op::Exit(Exit::Jump(one))),
        ];
        // The return site returns r0 + r1 in r4.
        let after = [
            inst(Op::GetReg(Reg(0))),
            inst(Op::GetReg(Reg(1))),
            inst(Op::Bin(analysis::ir::BinOp::Add, ValueId(0), ValueId(1))),
            inst(Op::SetReg(Reg(4), ValueId(2))),
            inst(Op::Exit(Exit::Jump(one))),
        ];
        let out = |b: usize, _: usize| {
            if b == 0 {
                (vec![1], reg(0) | reg(1))
            } else {
                (Vec::new(), reg(4))
            }
        };
        let (_, live_in) = dead_and_live(&[&call, &after], out);
        let by_name = |b: usize, _: usize| (b == 0).then_some(reg(0));
        let local = block_local(&[&call, &after], &live_in, out, by_name);
        let read_by_call = Local {
            reads: Vec::new(),
            by_exit: true,
        };
        assert_eq!(local[0], [None, Some(read_by_call), None, None]);
        let local = block_local(&[&call, &after], &live_in, out, |_, _| None);
        assert_eq!(
            local[0],
            [None, None, None, None],
            "read by a call's printed names"
        );
    }
}
