//! Dead register writes (`docs/structured-codegen-design.md`, step 1;
//! `docs/research/registers-and-types.md`, "Where state must be in
//! `Machine`"): delete each `SetReg` a later write to the same register
//! overwrites with nothing between that can read `cpu.r`.
//!
//! The walk is backwards. Inside a block only these can see a register's
//! value in `Machine`:
//!
//! - an exit, guarded or not (a call, a return, a `swi`, the dispatcher, a
//!   tick's yield after a static edge): everything is live after it;
//! - `GetReg(r)`, which reads `r` (the builder forwards writes, so it only
//!   comes before the block's first write of `r`, but the pass doesn't
//!   rely on it);
//! - the user-bank ops, `msr` and `mrs`: a mode change rebanks r8 to r14,
//!   and in user and system mode the user bank is the current one, so each
//!   is taken as reading every register.
//!
//! Loads, stores and timing ops reach only the bus and the wait-state
//! counters, and no IRQ is taken inside a block, so they read nothing.
//! An unguarded `SetReg(r)` kills `r`; a guarded one kills it only for an
//! earlier write under the same guard, which runs exactly when it does.

use super::{Block, Op, ValueId};

impl Block {
    /// Delete the `SetReg`s a later write overwrites with nothing between
    /// that reads `cpu.r`, then DCE. Returns how many it deleted.
    pub fn remove_dead_reg_writes(&mut self) -> usize {
        // Registers a later write overwrites, unguarded; and per guard.
        let mut killed: u16 = 0;
        let mut under: Vec<(ValueId, u16)> = Vec::new();
        let mut dead = Vec::new();
        for (i, inst) in self.insts.iter().enumerate().rev() {
            match inst.op {
                Op::Exit(_)
                | Op::GetUserReg(_)
                | Op::SetUserReg(..)
                | Op::GetCpsr
                | Op::GetSpsr
                | Op::SetCpsr { .. }
                | Op::SetSpsr { .. } => {
                    killed = 0;
                    under.clear();
                }
                Op::GetReg(r) => {
                    killed &= !(1 << r.index());
                    for (_, k) in &mut under {
                        *k &= !(1 << r.index());
                    }
                }
                Op::SetReg(r, _) => {
                    let bit = 1 << r.index();
                    let same = inst
                        .guard
                        .and_then(|g| under.iter_mut().find(|(h, _)| *h == g));
                    if killed & bit != 0 || same.as_ref().is_some_and(|(_, k)| k & bit != 0) {
                        dead.push(i);
                    } else if let Some(g) = inst.guard {
                        match same {
                            Some((_, k)) => *k |= bit,
                            None => under.push((g, bit)),
                        }
                    } else {
                        killed |= bit;
                    }
                }
                _ => {}
            }
        }
        for &i in &dead {
            self.nop(i);
        }
        if !dead.is_empty() {
            self.dce();
        }
        dead.len()
    }
}
