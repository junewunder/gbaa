//! `Sem<V = ValueId>`: running a semantic function on a [`Builder`] appends
//! its IR to the block.

use super::{BinOp, BitOp, Block, Exit, Inst, Op, Timing, ValueId};
use armv4t::cycles;
use armv4t::decode::Instr;
use armv4t::sem::prim::MulKind;
use armv4t::sem::{Flag, PsrMask, Reg, Sem, ShiftType, Width, cond, read_reg};

/// Builds one [`Block`]. The driver calls [`begin_insn`](Builder::begin_insn)
/// before each instruction's semantic function, stops once
/// [`ended`](Builder::ended) is true, and calls [`finish`](Builder::finish).
///
/// The builder forwards register and flag values within the block: a read
/// after a write in the same block reuses the written value instead of
/// emitting a `Get*`, so values flow through SSA and const-prop sees them.
/// Every write is still emitted as an effect, in order.
pub struct Builder {
    block: Block,
    /// The address of the instruction being built.
    at: u32,
    /// The conjunction of the enclosing `when` conditions.
    guard: Option<ValueId>,
    regs: [Option<ValueId>; 16],
    flags: [Option<ValueId>; 4],
    ended: bool,
    /// An exit ran under the current guard, so this path is over.
    exited_under_guard: bool,
    /// Add [`Op::YieldFlag`]s before each exit.
    yield_flags: bool,
}

impl Builder {
    pub fn new(start: u32, thumb: bool) -> Self {
        Builder {
            block: Block {
                start,
                thumb,
                insts: Vec::new(),
            },
            at: start,
            guard: None,
            regs: [None; 16],
            flags: [None; 4],
            ended: false,
            exited_under_guard: false,
            yield_flags: false,
        }
    }

    /// Before each exit, add an [`Op::YieldFlag`] for each flag the block
    /// has a value for there (one it wrote or read), so that flag
    /// liveness can keep the value of a write it deletes for the exit's
    /// yield path ([`super::flags`]).
    pub fn yield_flags(mut self) -> Self {
        self.yield_flags = true;
        self
    }

    /// Start the instruction at `addr`.
    pub fn begin_insn(&mut self, addr: u32) {
        assert!(!self.ended, "instruction after the block's exit");
        self.at = addr;
        self.push(Op::Insn(addr));
    }

    /// Add the time of `instr` (at `at`, just begun) that hangs on values,
    /// as [`Op::Timing`]s: [`Timing::Skipped`] where its condition fails (if
    /// it costs more than a failed one);
    /// where it passes, a multiply's operand or an `ldm`/`stm`'s access
    /// count. Call it before the instruction's semantic function, which may
    /// change the flags and `rs`.
    pub fn timing(&mut self, instr: &Instr, at: u32) {
        let passed = |b: &mut Builder| match *instr {
            Instr::Mul { rs, .. } | Instr::MulLong { rs, .. } => {
                let pc = 2 * b.insn_size();
                let v = read_reg(b, rs.into(), at, pc);
                b.effect(Op::Timing(Timing::Multiply(v)));
            }
            _ => {
                if let Some(n) = cycles::block_accesses(instr) {
                    b.effect(Op::Timing(Timing::Block(n)));
                }
            }
        };
        let costs_more = cycles::cost(instr) != cycles::SKIPPED;
        let hangs = cycles::mul_form(instr).is_some() || cycles::block_accesses(instr).is_some();
        if instr.cond() == armv4t::decode::Cond::Al {
            passed(self);
        } else if costs_more || hangs {
            // Only an instruction that costs more than a failed one needs
            // the time back where it fails.
            let c = cond(self, instr.cond()).unwrap();
            self.when(c, passed);
            if costs_more {
                let failed = self.bnot(c);
                self.when(failed, |b| {
                    b.effect(Op::Timing(Timing::Skipped));
                });
            }
        }
    }

    /// Whether an unguarded exit has ended the block.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Close the block, falling through to `next` if nothing exited.
    pub fn finish(mut self, next: u32) -> Block {
        if !self.ended {
            self.exit(Exit::Fallthrough(next));
        }
        self.block
    }

    fn push(&mut self, op: Op) -> ValueId {
        let id = ValueId(self.block.insts.len() as u32);
        self.block.insts.push(Inst { op, guard: None });
        id
    }

    /// Append an effect under the current guard.
    fn effect(&mut self, op: Op) -> ValueId {
        assert!(!self.ended && !self.exited_under_guard, "effect after exit");
        let id = self.push(op);
        self.block.insts[id.index()].guard = self.guard;
        id
    }

    fn exit(&mut self, e: Exit) {
        if self.yield_flags {
            for f in Flag::ALL {
                if let Some(v) = self.flags[f.index()] {
                    self.push(Op::YieldFlag(f, v));
                }
            }
        }
        self.effect(Op::Exit(e));
        if self.guard.is_none() {
            self.ended = true;
        } else {
            self.exited_under_guard = true;
        }
    }

    fn insn_size(&self) -> u32 {
        if self.block.thumb { 2 } else { 4 }
    }
}

impl Sem for Builder {
    type V = ValueId;
    type B = ValueId;

    fn imm(&mut self, x: u32) -> ValueId {
        self.push(Op::Const(x))
    }
    fn bit(&mut self, b: bool) -> ValueId {
        self.push(Op::Bit(b))
    }
    fn add(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::Bin(BinOp::Add, a, b))
    }
    fn sub(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::Bin(BinOp::Sub, a, b))
    }
    fn and(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::Bin(BinOp::And, a, b))
    }
    fn or(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::Bin(BinOp::Or, a, b))
    }
    fn xor(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::Bin(BinOp::Xor, a, b))
    }
    fn not(&mut self, a: ValueId) -> ValueId {
        self.push(Op::Not(a))
    }
    fn mul(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::Bin(BinOp::Mul, a, b))
    }
    fn mul_long(&mut self, signed: bool, a: ValueId, b: ValueId) -> (ValueId, ValueId) {
        let lo = self.push(Op::MulLongLo { signed, a, b });
        let hi = self.push(Op::MulLongHi { signed, a, b });
        (lo, hi)
    }
    fn add_with_carry(
        &mut self,
        a: ValueId,
        b: ValueId,
        c: ValueId,
    ) -> (ValueId, ValueId, ValueId) {
        let r = self.push(Op::Adc { a, b, c });
        let carry = self.push(Op::AdcCarry { a, b, c });
        let overflow = self.push(Op::AdcOverflow { a, b, c });
        (r, carry, overflow)
    }
    fn mul_carry(
        &mut self,
        kind: MulKind,
        a: ValueId,
        b: ValueId,
        acc_lo: ValueId,
        acc_hi: ValueId,
    ) -> ValueId {
        self.push(Op::MulCarry {
            kind,
            a,
            b,
            acc_lo,
            acc_hi,
        })
    }
    fn shift(
        &mut self,
        kind: ShiftType,
        v: ValueId,
        amount: ValueId,
        carry_in: ValueId,
    ) -> (ValueId, ValueId) {
        let r = self.push(Op::Shift { kind, v, amount });
        let c = self.push(Op::ShiftCarry {
            kind,
            v,
            amount,
            carry_in,
        });
        (r, c)
    }
    fn rrx(&mut self, v: ValueId, carry_in: ValueId) -> (ValueId, ValueId) {
        let r = self.push(Op::Rrx { v, carry_in });
        let c = self.push(Op::TestBit { v, n: 0 });
        (r, c)
    }
    fn sext(&mut self, v: ValueId, from_bits: u32) -> ValueId {
        self.push(Op::Sext { v, bits: from_bits })
    }
    fn select(&mut self, c: ValueId, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::Select { c, a, b })
    }
    fn bit_to_word(&mut self, b: ValueId) -> ValueId {
        self.push(Op::BitToWord(b))
    }
    fn test_bit(&mut self, v: ValueId, n: u32) -> ValueId {
        self.push(Op::TestBit { v, n })
    }
    fn is_zero(&mut self, v: ValueId) -> ValueId {
        self.push(Op::IsZero(v))
    }
    fn bnot(&mut self, a: ValueId) -> ValueId {
        self.push(Op::BNot(a))
    }
    fn band(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::BBin(BitOp::And, a, b))
    }
    fn bor(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::BBin(BitOp::Or, a, b))
    }
    fn bxor(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.push(Op::BBin(BitOp::Xor, a, b))
    }

    fn reg(&mut self, r: Reg) -> ValueId {
        assert_ne!(r, Reg::PC, "pc reads are constants");
        if let Some(v) = self.regs[r.index()] {
            return v;
        }
        let v = self.push(Op::GetReg(r));
        self.regs[r.index()] = Some(v);
        v
    }
    fn set_reg(&mut self, r: Reg, v: ValueId) {
        assert_ne!(r, Reg::PC, "pc writes are jumps");
        // Read the old value before the write, for the guarded merge.
        let old = self.guard.map(|_| self.reg(r));
        self.effect(Op::SetReg(r, v));
        self.regs[r.index()] = Some(match (self.guard, old) {
            (Some(g), Some(old)) => self.select(g, v, old),
            _ => v,
        });
    }
    fn user_reg(&mut self, r: Reg) -> ValueId {
        assert_ne!(r, Reg::PC, "pc reads are constants");
        self.push(Op::GetUserReg(r))
    }
    fn set_user_reg(&mut self, r: Reg, v: ValueId) {
        self.effect(Op::SetUserReg(r, v));
        // In user and system mode the user bank is the current bank.
        self.regs = [None; 16];
    }
    fn flag(&mut self, f: Flag) -> ValueId {
        if let Some(v) = self.flags[f.index()] {
            return v;
        }
        let v = self.push(Op::GetFlag(f));
        self.flags[f.index()] = Some(v);
        v
    }
    fn set_flag(&mut self, f: Flag, b: ValueId) {
        let old = self.guard.map(|_| self.flag(f));
        self.effect(Op::SetFlag(f, b));
        self.flags[f.index()] = Some(match (self.guard, old) {
            (Some(c), Some(old)) => self.push(Op::BSelect { c, a: b, b: old }),
            _ => b,
        });
    }
    fn cpsr(&mut self) -> ValueId {
        self.push(Op::GetCpsr)
    }
    fn set_cpsr(&mut self, v: ValueId, mask: PsrMask) {
        self.effect(Op::SetCpsr { v, mask });
        if mask.contains(PsrMask::CONTROL) {
            // A mode switch rebanks r8 to r14.
            self.regs = [None; 16];
        }
        if mask.contains(PsrMask::FLAGS) {
            self.flags = [None; 4];
        }
    }
    fn spsr(&mut self) -> ValueId {
        self.push(Op::GetSpsr)
    }
    fn set_spsr(&mut self, v: ValueId, mask: PsrMask) {
        self.effect(Op::SetSpsr { v, mask });
    }
    fn load(&mut self, width: Width, addr: ValueId) -> ValueId {
        self.effect(Op::Load { width, addr })
    }
    fn store(&mut self, width: Width, addr: ValueId, value: ValueId) {
        self.effect(Op::Store { width, addr, value });
    }
    fn when(&mut self, c: ValueId, f: impl FnOnce(&mut Self)) {
        let outer = self.guard;
        let inner = match outer {
            Some(g) => self.band(g, c),
            None => c,
        };
        let outer_exited = self.exited_under_guard;
        self.guard = Some(inner);
        f(self);
        // The path where `c` is false carries on.
        self.guard = outer;
        self.exited_under_guard = outer_exited;
    }
    fn jump(&mut self, target: ValueId) {
        self.exit(Exit::Jump(target));
    }
    fn jump_exchange(&mut self, target: ValueId) {
        self.exit(Exit::JumpExchange(target));
    }
    fn exception_return(&mut self, target: ValueId) {
        self.exit(Exit::ExceptionReturn(target));
    }
    fn swi(&mut self, comment: u32) {
        let next = self.at.wrapping_add(self.insn_size());
        self.exit(Exit::Swi { comment, next });
    }
    fn undefined(&mut self) {
        self.exit(Exit::Undefined);
    }
}
