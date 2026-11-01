//! A reference evaluator for the IR, for tests: it runs a [`Block`] on the
//! interpreter's [`Cpu`] and [`Bus`], so a block built from an
//! instruction's semantics can be checked against the interpreter running
//! the same semantics (`just sst-ir`). It is slow (one `match` per inst) and
//! is not how recompiled code runs.
//!
//! CPU state goes through the same `Cpu` methods the interpreter uses
//! (banking, `msr` masking, SPSR access, exits), so a difference between
//! the two is a difference in what the builder emitted.

use super::{Block, Exit, Op, ValueId};
use armv4t::interp::{self, Bus, Cpu};

/// The value of an inst that didn't run.
const SKIPPED: u32 = 0xDEAD_BEEF;

/// Run `block` from its start on `cpu` and `bus`, then take its exit the
/// way the interpreter would. `cpu.pc` must be the block's start, and the
/// CPU's T bit must match the block's instruction set. Returns the exit
/// taken, `None` for a fallthrough (after which pc is the next
/// instruction).
pub fn run(block: &Block, cpu: &mut Cpu, bus: &mut impl Bus) -> Option<interp::Exit> {
    assert_eq!(cpu.pc, block.start, "block run from the wrong pc");
    assert_eq!(cpu.thumb(), block.thumb, "block run in the wrong state");
    // Bits are 0 or 1, and reading anything else as a bit panics, so a word
    // used as a bit shows up. A skipped inst's value is a poison word, so a
    // guarded load's value used outside its guard shows up as a difference.
    let mut v: Vec<u32> = Vec::with_capacity(block.insts.len());
    let mut at = block.start;
    for inst in &block.insts {
        let g = |i: ValueId| v[i.index()];
        let b = |i: ValueId| match v[i.index()] {
            0 => false,
            1 => true,
            w => panic!("{i} = {w:#x} used as a bit"),
        };
        if let Some(guard) = inst.guard
            && !b(guard)
        {
            v.push(SKIPPED);
            continue;
        }
        if let Some(x) = inst.op.eval_pure(g, b) {
            v.push(x);
            continue;
        }
        let out = match inst.op {
            Op::GetReg(r) => cpu.r[r.index()],
            Op::GetUserReg(r) => cpu.user_reg(r),
            Op::GetFlag(f) => cpu.flag(f) as u32,
            Op::GetCpsr => cpu.cpsr,
            Op::GetSpsr => cpu.spsr(),
            Op::SetReg(r, x) => {
                cpu.r[r.index()] = g(x);
                0
            }
            Op::SetUserReg(r, x) => {
                cpu.set_user_reg(r, g(x));
                0
            }
            Op::SetFlag(f, x) => {
                cpu.set_flag(f, b(x));
                0
            }
            // Only a yield path writes it; this evaluator never yields.
            Op::YieldFlag(..) => 0,
            Op::SetCpsr { v: x, mask } => {
                cpu.msr_cpsr(g(x), mask);
                0
            }
            Op::SetSpsr { v: x, mask } => {
                cpu.msr_spsr(g(x), mask);
                0
            }
            Op::Load { width, addr } => bus.load(width, g(addr) & !(width.bytes() - 1)),
            Op::Store { width, addr, value } => {
                bus.store(width, g(addr) & !(width.bytes() - 1), g(value));
                0
            }
            Op::Exit(e) => {
                let exit = match e {
                    Exit::Jump(t) => Some(interp::Exit::Jump(g(t))),
                    Exit::JumpExchange(t) => Some(interp::Exit::Exchange(g(t))),
                    Exit::ExceptionReturn(t) => Some(interp::Exit::ExceptionReturn(g(t))),
                    Exit::Swi { next, .. } => {
                        let size = if block.thumb { 2 } else { 4 };
                        assert_eq!(next, at.wrapping_add(size), "swi resumes after itself");
                        Some(interp::Exit::Swi)
                    }
                    Exit::Undefined => Some(interp::Exit::Undefined),
                    Exit::Fallthrough(next) => {
                        cpu.pc = next;
                        return None;
                    }
                };
                cpu.apply_exit(at, block.thumb, exit);
                return exit;
            }
            Op::Insn(a) => {
                at = a;
                0
            }
            // The reference evaluator counts no time.
            Op::Timing(_) => 0,
            _ => unreachable!("pure op {:?} not evaluated", inst.op),
        };
        v.push(out);
    }
    panic!("block without an exit");
}
