//! The per-block SSA IR (decision 3). [`build::Builder`] is the `Sem`
//! implementation with `V = B = ValueId` that produces it. Design and
//! rationale are in `docs/sem-design.md`.
//!
//! A [`Block`] is one guest superblock: a single entry, a straight line of
//! [`Inst`]s, guarded side exits, and a final unguarded exit. Each inst
//! defines the value named by its index (insts that produce nothing just
//! leave theirs unused). Operands always refer to earlier insts.
//!
//! Insts run in order. An inst with a guard runs only if the guard bit is
//! set; the first exit that runs leaves the block. A skipped inst's value
//! (a guarded `Load`) is unspecified: the builder only uses it under the
//! same guard or through a `select` on that guard.

pub mod build;
pub mod constprop;
pub mod eval;
pub mod flags;
pub mod regs;

#[cfg(test)]
mod tests;

use armv4t::sem::prim::MulKind;
use armv4t::sem::{Flag, PsrMask, Reg, ShiftType, Width};
use std::fmt;

/// An SSA value: the result of the inst at this index in [`Block::insts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ValueId(pub u32);

impl ValueId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Mul,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BitOp {
    And,
    Or,
    Xor,
}

/// How a block is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Exit {
    /// Continue at a target in the current instruction set.
    Jump(ValueId),
    /// `bx`: bit 0 of the target picks Thumb or ARM.
    JumpExchange(ValueId),
    /// CPSR = SPSR, then continue at the target. In user and system mode,
    /// which have no SPSR, the CPSR is left as it is.
    ExceptionReturn(ValueId),
    /// Software interrupt; resumes at the next instruction after the BIOS
    /// call (or enters supervisor mode, at the runtime's choice).
    Swi {
        comment: u32,
        next: u32,
    },
    Undefined,
    /// Ran off the end of the block into the instruction at this address.
    Fallthrough(u32),
}

/// What an inst does. The groups matter to passes:
///
/// - **pure** ops have no effect and depend only on their operands;
/// - **reads** (`Get*`) see CPU state as of their position, so they don't
///   move past writes, but an unused one can be dropped;
/// - **effects** (`Set*`, `Load`, `Store`) and **exits** always stay, in
///   order;
/// - `Insn` marks where a guest instruction starts (for comments, debugging
///   and mapping back to guest addresses).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    // Pure, word-valued.
    Const(u32),
    /// A constant const-prop read from a word load at a constant ROM
    /// address: in compiled code, a literal-pool word. Its value is the
    /// same as a `Const`'s; only a named print tells them apart (it may
    /// name the word as a data address, `docs/mod-view-design.md`).
    Pool(u32),
    Bin(BinOp, ValueId, ValueId),
    Not(ValueId),
    MulLongLo {
        signed: bool,
        a: ValueId,
        b: ValueId,
    },
    MulLongHi {
        signed: bool,
        a: ValueId,
        b: ValueId,
    },
    /// `AddWithCarry`'s result. Its carry and overflow are separate insts so
    /// that a dead flag is simply an unused value.
    Adc {
        a: ValueId,
        b: ValueId,
        c: ValueId,
    },
    Shift {
        kind: ShiftType,
        v: ValueId,
        amount: ValueId,
    },
    Rrx {
        v: ValueId,
        carry_in: ValueId,
    },
    Sext {
        v: ValueId,
        bits: u32,
    },
    Select {
        c: ValueId,
        a: ValueId,
        b: ValueId,
    },
    BitToWord(ValueId),

    // Pure, bit-valued.
    Bit(bool),
    AdcCarry {
        a: ValueId,
        b: ValueId,
        c: ValueId,
    },
    /// C after a flag-setting multiply (`prim::mul_carry`).
    MulCarry {
        kind: MulKind,
        a: ValueId,
        b: ValueId,
        acc_lo: ValueId,
        acc_hi: ValueId,
    },
    AdcOverflow {
        a: ValueId,
        b: ValueId,
        c: ValueId,
    },
    ShiftCarry {
        kind: ShiftType,
        v: ValueId,
        amount: ValueId,
        carry_in: ValueId,
    },
    TestBit {
        v: ValueId,
        n: u32,
    },
    IsZero(ValueId),
    BNot(ValueId),
    BBin(BitOp, ValueId, ValueId),
    /// Bit-valued select. Not a `Sem` op: the builder makes it when a guarded
    /// flag write is forwarded.
    BSelect {
        c: ValueId,
        a: ValueId,
        b: ValueId,
    },

    // Reads.
    GetReg(Reg),
    GetUserReg(Reg),
    GetFlag(Flag),
    GetCpsr,
    GetSpsr,

    // Effects.
    SetReg(Reg, ValueId),
    SetUserReg(Reg, ValueId),
    SetFlag(Flag, ValueId),
    /// The flag's value at the exit just after: written only on that
    /// exit's yield path, for a flag write flag liveness deleted
    /// ([`flags`]; `docs/structured-codegen-design.md`, step 2). The
    /// builder adds them only when asked ([`build::Builder::yield_flags`]).
    YieldFlag(Flag, ValueId),
    /// `msr cpsr` (`Sem::set_cpsr`): write the fields in `mask`, except
    /// that user mode writes only the flags; a mode change rebanks
    /// (`Cpu::msr_cpsr` is the reference).
    SetCpsr {
        v: ValueId,
        mask: PsrMask,
    },
    /// `msr spsr`: no effect in user and system mode, which have no SPSR
    /// (`Cpu::msr_spsr`).
    SetSpsr {
        v: ValueId,
        mask: PsrMask,
    },
    /// A load is an effect: I/O reads can have side effects, and ordering
    /// against stores matters. Const-prop may later prove a load pure (ROM).
    Load {
        width: Width,
        addr: ValueId,
    },
    Store {
        width: Width,
        addr: ValueId,
        value: ValueId,
    },

    Exit(Exit),
    Insn(u32),
    /// Time that hangs on values, which the instruction's static cost
    /// leaves out (`armv4t::cycles`); the builder adds it under the
    /// instruction's condition ([`build::Builder::timing`]).
    Timing(Timing),
}

/// See [`Op::Timing`]. Each counts as the interpreter's `Machine::step` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Timing {
    /// The instruction's condition failed: it costs `armv4t::cycles::SKIPPED`,
    /// not its own cost.
    Skipped,
    /// The multiply's internal cycles, from its operand `rs` before it runs
    /// (`armv4t::cycles::mul_wait`).
    Multiply(ValueId),
    /// The next `n` data accesses are an `ldm`'s or `stm`'s.
    Block(u32),
}

impl Op {
    /// Whether DCE must keep this inst even when its value is unused.
    pub fn is_root(&self) -> bool {
        matches!(
            self,
            Op::SetReg(..)
                | Op::SetUserReg(..)
                | Op::SetFlag(..)
                | Op::YieldFlag(..)
                | Op::SetCpsr { .. }
                | Op::SetSpsr { .. }
                | Op::Load { .. }
                | Op::Store { .. }
                | Op::Exit(_)
                | Op::Insn(_)
                | Op::Timing(_)
        )
    }

    /// The values this op uses, mutably (for renumbering passes).
    pub fn operands_mut(&mut self) -> Vec<&mut ValueId> {
        use Op::*;
        match self {
            Const(_)
            | Pool(_)
            | Bit(_)
            | GetReg(_)
            | GetUserReg(_)
            | GetFlag(_)
            | GetCpsr
            | GetSpsr
            | Insn(_)
            | Timing(self::Timing::Skipped | self::Timing::Block(_)) => vec![],
            Not(a)
            | BitToWord(a)
            | IsZero(a)
            | BNot(a)
            | SetReg(_, a)
            | SetUserReg(_, a)
            | SetFlag(_, a)
            | YieldFlag(_, a)
            | Timing(self::Timing::Multiply(a)) => vec![a],
            Sext { v, .. } | TestBit { v, .. } | SetCpsr { v, .. } | SetSpsr { v, .. } => vec![v],
            Load { addr, .. } => vec![addr],
            Bin(_, a, b) | BBin(_, a, b) | MulLongLo { a, b, .. } | MulLongHi { a, b, .. } => {
                vec![a, b]
            }
            Rrx { v, carry_in } => vec![v, carry_in],
            Shift { v, amount, .. } => vec![v, amount],
            Store { addr, value, .. } => vec![addr, value],
            Adc { a, b, c } | AdcCarry { a, b, c } | AdcOverflow { a, b, c } => vec![a, b, c],
            MulCarry {
                a,
                b,
                acc_lo,
                acc_hi,
                ..
            } => vec![a, b, acc_lo, acc_hi],
            Select { c, a, b } | BSelect { c, a, b } => vec![c, a, b],
            ShiftCarry {
                v,
                amount,
                carry_in,
                ..
            } => vec![v, amount, carry_in],
            Exit(e) => match e {
                self::Exit::Jump(t)
                | self::Exit::JumpExchange(t)
                | self::Exit::ExceptionReturn(t) => {
                    vec![t]
                }
                self::Exit::Swi { .. } | self::Exit::Undefined | self::Exit::Fallthrough(_) => {
                    vec![]
                }
            },
        }
    }

    /// The values this op uses.
    pub fn operands(&self) -> Vec<ValueId> {
        let mut op = *self;
        op.operands_mut().into_iter().map(|v| *v).collect()
    }

    /// Whether this is a pure op whose value is a bit.
    pub fn is_pure_bit(&self) -> bool {
        matches!(
            self,
            Op::Bit(_)
                | Op::AdcCarry { .. }
                | Op::MulCarry { .. }
                | Op::AdcOverflow { .. }
                | Op::ShiftCarry { .. }
                | Op::TestBit { .. }
                | Op::IsZero(_)
                | Op::BNot(_)
                | Op::BBin(..)
                | Op::BSelect { .. }
        )
    }

    /// The value of a pure op, given its operands' values as words (`w`)
    /// and bits (`b`); a bit's value is 0 or 1. `None` for reads, effects,
    /// exits and markers. The reference evaluator and const-prop both use
    /// it, so `just sst-ir` checks what const-prop folds.
    pub fn eval_pure(
        &self,
        w: impl Fn(ValueId) -> u32,
        b: impl Fn(ValueId) -> bool,
    ) -> Option<u32> {
        use Op::*;
        use armv4t::sem::prim;
        Some(match *self {
            Const(x) | Pool(x) => x,
            Bit(x) => x as u32,
            Bin(op, x, y) => {
                let (x, y) = (w(x), w(y));
                match op {
                    BinOp::Add => x.wrapping_add(y),
                    BinOp::Sub => x.wrapping_sub(y),
                    BinOp::And => x & y,
                    BinOp::Or => x | y,
                    BinOp::Xor => x ^ y,
                    BinOp::Mul => x.wrapping_mul(y),
                }
            }
            Not(x) => !w(x),
            MulLongLo { signed, a, b: y } => prim::mul_long(signed, w(a), w(y)).0,
            MulLongHi { signed, a, b: y } => prim::mul_long(signed, w(a), w(y)).1,
            Adc { a, b: y, c } => prim::add_with_carry(w(a), w(y), b(c)).0,
            AdcCarry { a, b: y, c } => prim::add_with_carry(w(a), w(y), b(c)).1 as u32,
            AdcOverflow { a, b: y, c } => prim::add_with_carry(w(a), w(y), b(c)).2 as u32,
            MulCarry {
                kind,
                a,
                b: y,
                acc_lo,
                acc_hi,
            } => prim::mul_carry(
                kind,
                w(a),
                w(y),
                (w(acc_hi) as u64) << 32 | w(acc_lo) as u64,
            ) as u32,
            // The result doesn't depend on the carry in.
            Shift { kind, v: x, amount } => prim::shift(kind, w(x), w(amount), false).0,
            ShiftCarry {
                kind,
                v: x,
                amount,
                carry_in,
            } => prim::shift(kind, w(x), w(amount), b(carry_in)).1 as u32,
            Rrx { v: x, carry_in } => prim::rrx(w(x), b(carry_in)).0,
            Sext { v: x, bits } => prim::sext(w(x), bits),
            Select { c, a, b: y } | BSelect { c, a, b: y } => {
                if b(c) {
                    w(a)
                } else {
                    w(y)
                }
            }
            BitToWord(x) => b(x) as u32,
            TestBit { v: x, n } => (w(x) >> n) & 1,
            IsZero(x) => (w(x) == 0) as u32,
            BNot(x) => !b(x) as u32,
            BBin(op, x, y) => {
                (match op {
                    BitOp::And => b(x) & b(y),
                    BitOp::Or => b(x) | b(y),
                    BitOp::Xor => b(x) ^ b(y),
                }) as u32
            }
            GetReg(_)
            | GetUserReg(_)
            | GetFlag(_)
            | GetCpsr
            | GetSpsr
            | SetReg(..)
            | SetUserReg(..)
            | SetFlag(..)
            | YieldFlag(..)
            | SetCpsr { .. }
            | SetSpsr { .. }
            | Load { .. }
            | Store { .. }
            | Exit(_)
            | Insn(_)
            | Timing(_) => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Inst {
    pub op: Op,
    /// Effects and exits only: run this inst only if the bit is set.
    pub guard: Option<ValueId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub start: u32,
    pub thumb: bool,
    /// Ends with an unguarded [`Op::Exit`].
    pub insts: Vec<Inst>,
}

impl Block {
    /// Remove pure ops and reads whose values nothing uses, renumbering the
    /// rest. One backward pass, since operands always point backwards.
    pub fn dce(&mut self) {
        let mut live = vec![false; self.insts.len()];
        for (i, inst) in self.insts.iter().enumerate().rev() {
            if !(live[i] || inst.op.is_root()) {
                continue;
            }
            live[i] = true;
            for v in inst.op.operands().into_iter().chain(inst.guard) {
                live[v.index()] = true;
            }
        }
        let mut renumber = vec![ValueId(u32::MAX); self.insts.len()];
        let mut kept = Vec::new();
        for (i, mut inst) in self.insts.drain(..).enumerate() {
            if !live[i] {
                continue;
            }
            for v in inst
                .op
                .operands_mut()
                .into_iter()
                .chain(inst.guard.as_mut())
            {
                *v = renumber[v.index()];
            }
            renumber[i] = ValueId(kept.len() as u32);
            kept.push(inst);
        }
        self.insts = kept;
    }

    /// Make inst `i` a pure op nothing uses, for [`Block::dce`] to remove:
    /// how a pass deletes an inst without renumbering.
    pub fn nop(&mut self, i: usize) {
        self.insts[i] = Inst {
            op: Op::Const(0),
            guard: None,
        };
    }

    /// The block-local passes, as a one-block caller (the SingleStepTests
    /// harnesses) runs them: const-prop reading loads from `rom`, DCE, dead
    /// register writes, and flag liveness with all four flags live at every
    /// exit.
    pub fn optimize(&mut self, rom: constprop::Rom) {
        self.const_prop(rom);
        self.dce();
        self.remove_dead_reg_writes();
        self.remove_dead_flags(|_| flags::Flags::ALL);
    }
}

impl fmt::Display for ValueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

fn reg_name(r: Reg) -> String {
    match r {
        Reg::SP => "sp".into(),
        Reg::LR => "lr".into(),
        Reg::PC => "pc".into(),
        Reg(n) => format!("r{n}"),
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Op::*;
        let lower = |x: &dyn fmt::Debug| format!("{x:?}").to_lowercase();
        match *self {
            Const(x) => write!(f, "const {x:#x}"),
            Pool(x) => write!(f, "pool {x:#x}"),
            Bit(b) => write!(f, "bit {b}"),
            Bin(op, a, b) => write!(f, "{} {a}, {b}", lower(&op)),
            Not(a) => write!(f, "not {a}"),
            MulLongLo { signed, a, b } => write!(f, "mul_long_lo signed={signed} {a}, {b}"),
            MulLongHi { signed, a, b } => write!(f, "mul_long_hi signed={signed} {a}, {b}"),
            Adc { a, b, c } => write!(f, "adc {a}, {b}, {c}"),
            AdcCarry { a, b, c } => write!(f, "adc.carry {a}, {b}, {c}"),
            AdcOverflow { a, b, c } => write!(f, "adc.overflow {a}, {b}, {c}"),
            MulCarry {
                kind,
                a,
                b,
                acc_lo,
                acc_hi,
            } => write!(f, "mul_carry {} {a}, {b}, {acc_lo}, {acc_hi}", lower(&kind)),
            Shift { kind, v, amount } => write!(f, "{} {v}, {amount}", lower(&kind)),
            ShiftCarry {
                kind,
                v,
                amount,
                carry_in,
            } => write!(f, "{}.carry {v}, {amount}, {carry_in}", lower(&kind)),
            Rrx { v, carry_in } => write!(f, "rrx {v}, {carry_in}"),
            Sext { v, bits } => write!(f, "sext{bits} {v}"),
            Select { c, a, b } => write!(f, "select {c}, {a}, {b}"),
            BSelect { c, a, b } => write!(f, "bselect {c}, {a}, {b}"),
            BitToWord(b) => write!(f, "bit_to_word {b}"),
            TestBit { v, n } => write!(f, "test_bit {v}, {n}"),
            IsZero(v) => write!(f, "is_zero {v}"),
            BNot(a) => write!(f, "bnot {a}"),
            BBin(op, a, b) => write!(f, "b{} {a}, {b}", lower(&op)),
            GetReg(r) => write!(f, "get {}", reg_name(r)),
            GetUserReg(r) => write!(f, "get_user {}", reg_name(r)),
            GetFlag(fl) => write!(f, "get {fl:?}"),
            GetCpsr => write!(f, "get cpsr"),
            GetSpsr => write!(f, "get spsr"),
            SetReg(r, v) => write!(f, "set {}, {v}", reg_name(r)),
            SetUserReg(r, v) => write!(f, "set_user {}, {v}", reg_name(r)),
            SetFlag(fl, v) => write!(f, "set {fl:?}, {v}"),
            YieldFlag(fl, v) => write!(f, "yield {fl:?}, {v}"),
            SetCpsr { v, mask } => write!(f, "set cpsr mask={:#06b}, {v}", mask.0),
            SetSpsr { v, mask } => write!(f, "set spsr mask={:#06b}, {v}", mask.0),
            Load { width, addr } => write!(f, "load.{} {addr}", lower(&width)),
            Store { width, addr, value } => write!(f, "store.{} {addr}, {value}", lower(&width)),
            Exit(e) => match e {
                self::Exit::Jump(t) => write!(f, "exit jump {t}"),
                self::Exit::JumpExchange(t) => write!(f, "exit jump_exchange {t}"),
                self::Exit::ExceptionReturn(t) => write!(f, "exit exception_return {t}"),
                self::Exit::Swi { comment, next } => {
                    write!(f, "exit swi {comment:#x}, next {next:#010x}")
                }
                self::Exit::Undefined => write!(f, "exit undefined"),
                self::Exit::Fallthrough(a) => write!(f, "exit fallthrough {a:#010x}"),
            },
            Insn(a) => write!(f, "; {a:#010x}"),
            Timing(self::Timing::Skipped) => write!(f, "timing skipped"),
            Timing(self::Timing::Multiply(v)) => write!(f, "timing multiply {v}"),
            Timing(self::Timing::Block(n)) => write!(f, "timing block {n}"),
        }
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let isa = if self.thumb { "thumb" } else { "arm" };
        writeln!(f, "block {:#010x} {isa}", self.start)?;
        for (i, inst) in self.insts.iter().enumerate() {
            let guard = inst.guard.map(|g| format!("if {g}: ")).unwrap_or_default();
            match inst.op {
                Op::Insn(_) => writeln!(f, "  {}", inst.op)?,
                op if produces_value(&op) => writeln!(f, "  {guard}v{i} = {op}")?,
                op => writeln!(f, "  {guard}{op}")?,
            }
        }
        Ok(())
    }
}

fn produces_value(op: &Op) -> bool {
    !op.is_root() || matches!(op, Op::Load { .. })
}
