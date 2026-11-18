//! Counted loops printed as `for` (unit `native/lift-for-loops`).
//!
//! The mod view's `lower::For` (`mod/readable-for-loops`) on the native
//! lift's structured form: a loop of one block whose branch back to itself
//! is taken while a register, its variable, steps through a range, entered
//! from one block that leaves the variable a constant. The range is proven
//! by running the block's IR over it from the registers and flags its one
//! way in leaves known ([`values`]), at most [`MAX`] times, and must step
//! evenly ([`crate::lower::counted`], shared with the mod view).
//!
//! Printed as `'lN: for rK in (lo..=hi) { … }` with the block's write of
//! the step left out: the binding is the register's value at the block's
//! start on each round, the branch back is still printed (`continue 'lN`,
//! taken on every round but the last), and the last round leaves by the
//! block's other exits as before. The `for` binding shadows the function's
//! `rK`, so the finder ([`super::FnPrinter::find_fors`]) also asks that
//! nothing after the loop reads the register before writing it, and that
//! nothing printed inside the loop writes it.

use analysis::ir::{Inst, Op};

/// The most rounds a `for`'s range is run over.
pub const MAX: usize = 4096;

/// r0-r15 and the four flags at a point in a block, where known.
#[derive(Clone, Debug)]
pub struct State {
    pub r: [Option<u32>; 16],
    pub f: [Option<bool>; 4],
}

impl State {
    pub fn unknown() -> State {
        State {
            r: [None; 16],
            f: [None; 4],
        }
    }
}

/// Run `insts[..end]` from `s`, which it leaves as they leave it: each
/// value, where known. A guarded write whose guard isn't known leaves its
/// register or flag unknown; a write of the CPSR or a user-mode register,
/// everything.
pub fn run(insts: &[Inst], s: &mut State, end: usize) -> Vec<Option<u32>> {
    let mut vals: Vec<Option<u32>> = vec![None; end];
    for (i, inst) in insts[..end].iter().enumerate() {
        let guard = inst.guard.map(|g| vals[g.index()].map(|g| g != 0));
        // Whether a write happens: yes, no, or not known (`None`).
        let writes = match guard {
            None => Some(true),
            Some(g) => g,
        };
        vals[i] = match inst.op {
            Op::GetReg(q) => s.r[q.index()],
            Op::GetFlag(f) => s.f[f.index()].map(u32::from),
            Op::SetReg(q, x) => {
                match writes {
                    Some(true) => s.r[q.index()] = vals[x.index()],
                    Some(false) => {}
                    None => s.r[q.index()] = None,
                }
                None
            }
            Op::SetFlag(f, x) => {
                match writes {
                    Some(true) => s.f[f.index()] = vals[x.index()].map(|v| v != 0),
                    Some(false) => {}
                    None => s.f[f.index()] = None,
                }
                None
            }
            Op::SetCpsr { .. } | Op::SetUserReg(..) => {
                *s = State::unknown();
                None
            }
            ref op => {
                let ops = op.operands();
                if ops.iter().all(|o| vals[o.index()].is_some()) {
                    op.eval_pure(
                        |o| vals[o.index()].unwrap(),
                        |o| vals[o.index()].unwrap() != 0,
                    )
                } else {
                    None
                }
            }
        };
    }
    vals
}

/// The values register `r` takes at the start of a loop block (`insts`),
/// round by round, entered with `s`: while the guard of the branch back
/// (`back`, its exit inst) holds. `None` if a round's guard or `r` isn't
/// known, or it runs more than [`MAX`] rounds.
pub fn values(insts: &[Inst], back: usize, r: usize, mut s: State) -> Option<Vec<u32>> {
    let guard = insts[back].guard?;
    let mut vs = Vec::new();
    loop {
        if vs.len() >= MAX {
            return None;
        }
        vs.push(s.r[r]?);
        let vals = run(insts, &mut s, back);
        if vals[guard.index()]? == 0 {
            return Some(vs);
        }
    }
}

/// A loop printed as `for`: its variable's register and range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct For {
    pub reg: usize,
    /// Whether anything printed inside the loop writes the binding.
    pub mutable: bool,
    pub first: u32,
    pub last: u32,
    pub step: i32,
}

impl For {
    /// The range the variable steps through, as `vs` (its values in order)
    /// does.
    pub fn of(reg: usize, vs: &[u32]) -> Option<For> {
        let (first, last, step) = crate::lower::counted(vs)?;
        Some(For {
            reg,
            mutable: false,
            first,
            last,
            step,
        })
    }

    /// The range's Rust: `(lo..=hi)`, `.rev()` counting down, and
    /// `.step_by(k)` by a step of more than one.
    pub fn range(&self) -> String {
        let lo = crate::print::word(self.first.min(self.last), true);
        let hi = crate::print::word(self.first.max(self.last), false);
        let mut s = format!("({lo}..={hi})");
        if self.step < 0 {
            s.push_str(".rev()");
        }
        let by = self.step.unsigned_abs();
        if by != 1 {
            s.push_str(&format!(".step_by({by})"));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use analysis::ir::{BinOp, Exit, ValueId};
    use armv4t::sem::{Flag, Reg};

    fn inst(op: Op) -> Inst {
        Inst { op, guard: None }
    }

    /// `r0 -= 1; Z = r0 == 0; if !Z goto back`: from 10, it runs at 10
    /// down to 1.
    #[test]
    fn a_countdown_runs_its_range() {
        let block = [
            inst(Op::GetReg(Reg(0))),
            inst(Op::Const(1)),
            inst(Op::Bin(BinOp::Sub, ValueId(0), ValueId(1))),
            inst(Op::SetReg(Reg(0), ValueId(2))),
            inst(Op::IsZero(ValueId(2))),
            inst(Op::SetFlag(Flag::Z, ValueId(4))),
            inst(Op::GetFlag(Flag::Z)),
            inst(Op::BNot(ValueId(6))),
            Inst {
                op: Op::Exit(Exit::Jump(ValueId(1))),
                guard: Some(ValueId(7)),
            },
        ];
        let mut s = State::unknown();
        s.r[0] = Some(10);
        let vs = values(&block, 8, 0, s.clone()).unwrap();
        assert_eq!(vs, (1..=10).rev().collect::<Vec<u32>>());
        let f = For::of(0, &vs).unwrap();
        assert_eq!(f.range(), "(1_u32..=10).rev()");
        // From 0 it wraps round: more than `MAX` rounds.
        s.r[0] = Some(0);
        assert_eq!(values(&block, 8, 0, s), None);
        // Not known at entry: no range.
        assert_eq!(values(&block, 8, 0, State::unknown()), None);
    }
}
