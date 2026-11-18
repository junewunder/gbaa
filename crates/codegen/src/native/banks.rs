//! Where a `bank_sp` the lift would print before a store is redundant
//! (unit `native/lift-bank-sp-once`): the current mode's bank already
//! holds the sp it would put there.
//!
//! The lift banks sp (`bank_sp(g, r13)`) before every store that may
//! reach I/O, as such a store may set `IME` again and run a handler on
//! the current stack. A bank is redundant when, on every path to it
//! within the function, the last thing to touch the bank was a bank of
//! the same `r13`:
//!
//! - an unguarded bank leaves the bank holding `r13`; a guarded one
//!   leaves it as it was (its false path skips it);
//! - a write of `r13` (or of the user-mode `r13`) changes the value it
//!   would bank, and a `msr` may switch modes, which swaps the bank, or
//!   run a held IRQ's handler: either ends the run;
//! - so does every exit that runs other code before it goes on in the
//!   function: a call (its callee banks its own, lower, sp), a `swi`
//!   (the BIOS helpers bank and run the clock), a wait (`hw::idle`,
//!   `hw::spin`, `hw::idle_exit`); only a plain jump within the function
//!   keeps it. The function's entry starts with nothing banked.
//!
//! A handler run by the store itself (`IME` set again, a `HALTCNT` halt,
//! an immediate DMA that reaches either) doesn't end the run: the
//! prelude's `irq_entry`, the one way a handler runs, puts the
//! interrupted mode's banked sp back as it found it (as the hardware's
//! `r13` comes back to the code an IRQ stopped), so after it the bank
//! still holds `r13`.

use analysis::ir::{Inst, Op};

/// Walk a block's `insts` forwards from `held` (whether the bank holds
/// `r13` at its start), `banks[i]` saying which stores print a bank:
/// call `exit(i, held)` at each exit, and return the stores whose bank is
/// redundant.
fn walk(
    insts: &[Inst],
    banks: &[bool],
    mut held: bool,
    mut exit: impl FnMut(usize, bool),
) -> Vec<bool> {
    let mut redundant = vec![false; insts.len()];
    for (i, inst) in insts.iter().enumerate() {
        let guarded = inst.guard.is_some();
        match inst.op {
            Op::Store { .. } if banks.get(i).copied().unwrap_or(false) => {
                redundant[i] = held;
                held |= !guarded;
            }
            Op::SetReg(r, _) | Op::SetUserReg(r, _) if r.index() == 13 => held = false,
            Op::SetCpsr { .. } => held = false,
            Op::Exit(_) => {
                exit(i, held);
                if !guarded {
                    held = false;
                }
            }
            _ => {}
        }
    }
    redundant
}

/// The redundant banks of a function's blocks (`blocks[b]`, a block's
/// insts; block 0 its entry): `redundant[b][i]` for a store whose bank
/// (`banks[b][i]`) the bank already holds. `out(b, i)` says where the
/// exit at inst `i` of block `b` goes on in the function: each block, and
/// whether the bank survives the way there (a plain jump's does; a
/// call's, a `swi`'s or a wait's doesn't).
pub fn redundant(
    blocks: &[&[Inst]],
    banks: &[Vec<bool>],
    out: impl Fn(usize, usize) -> Vec<(usize, bool)>,
) -> Vec<Vec<bool>> {
    let n = blocks.len();
    let entry = |held: &mut Vec<bool>| {
        if let Some(h) = held.first_mut() {
            *h = false;
        }
    };
    // The greatest fixpoint: every block but the entry held, shrinking.
    let mut held = vec![true; n];
    entry(&mut held);
    loop {
        let mut next = vec![true; n];
        entry(&mut next);
        for b in 0..n {
            walk(blocks[b], &banks[b], held[b], |i, h| {
                for (s, keeps) in out(b, i) {
                    next[s] &= h && keeps;
                }
            });
        }
        if next == held {
            break;
        }
        held = next;
    }
    (0..n)
        .map(|b| walk(blocks[b], &banks[b], held[b], |_, _| {}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use analysis::ir::{Exit, ValueId};
    use armv4t::sem::{Reg, Width};

    fn inst(op: Op) -> Inst {
        Inst { op, guard: None }
    }

    fn store() -> Op {
        let v = ValueId(0);
        Op::Store {
            width: Width::Half,
            addr: v,
            value: v,
        }
    }

    fn jump() -> Op {
        Op::Exit(Exit::Jump(ValueId(0)))
    }

    #[test]
    fn a_run_of_stores_banks_once_until_sp_changes() {
        let b0 = [
            inst(Op::Const(0)),
            inst(store()),
            inst(store()),
            inst(Op::SetReg(Reg(13), ValueId(0))),
            inst(store()),
            inst(store()),
            inst(jump()),
        ];
        let banks = vec![vec![false, true, true, false, true, true, false]];
        let r = redundant(&[&b0], &banks, |_, _| Vec::new());
        assert_eq!(r[0], [false, false, true, false, false, true, false]);
    }

    #[test]
    fn a_guarded_bank_holds_nothing_and_a_loop_needs_every_way_in() {
        let v = ValueId(0);
        // Block 0 banks under a guard, then jumps to 1; 1 banks, then
        // loops to itself.
        let b0 = [
            inst(Op::Const(0)),
            Inst {
                op: store(),
                guard: Some(v),
            },
            inst(jump()),
        ];
        let b1 = [inst(Op::Const(0)), inst(store()), inst(jump())];
        let banks = vec![vec![false, true, false], vec![false, true, false]];
        let r = redundant(&[&b0, &b1], &banks, |b, _| match b {
            0 => vec![(1, true)],
            _ => vec![(1, true)],
        });
        assert_eq!(r[0], [false, false, false]);
        assert_eq!(r[1], [false, false, false], "block 0's way in is unbanked");
        // Unguarded in block 0, the loop's bank is redundant on both ways in.
        let b0 = [inst(Op::Const(0)), inst(store()), inst(jump())];
        let r = redundant(&[&b0, &b1], &banks, |_, _| vec![(1, true)]);
        assert_eq!(r[1], [false, true, false]);
        // A call on the back edge ends the run.
        let r = redundant(&[&b0, &b1], &banks, |b, _| match b {
            0 => vec![(1, true)],
            _ => vec![(1, false)],
        });
        assert_eq!(r[1], [false, false, false]);
    }

    #[test]
    fn a_msr_and_the_entry_end_the_run() {
        let v = ValueId(0);
        let b0 = [
            inst(Op::Const(0)),
            inst(store()),
            inst(Op::SetCpsr {
                v,
                mask: armv4t::sem::PsrMask::CONTROL,
            }),
            inst(store()),
            inst(jump()),
        ];
        let banks = vec![vec![false, true, false, true, false]];
        // A jump back to the entry: the entry still starts unbanked.
        let r = redundant(&[&b0], &banks, |_, _| vec![(0, true)]);
        assert_eq!(r[0], [false; 5]);
    }
}
