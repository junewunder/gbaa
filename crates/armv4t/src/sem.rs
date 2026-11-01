//! Instruction semantics, written once over a tagless-final [`Sem`] trait
//! (decision 3). See `docs/sem-design.md` for the design and its rationale.
//!
//! A semantic function takes its decoded fields as plain Rust values (static)
//! and computes with `S::V` / `S::B` values (dynamic). The same function is
//! the interpreter when `S::V = u32` and builds IR when `S::V` is an IR value
//! id (`analysis::ir`).
//!
//! This module only uses `core`, so the interpreter can live in the `no_std`
//! runtime (decision 6).

pub mod arm;
pub mod prim;

// The decoder's condition and shift enums serve the semantics too.
pub use crate::decode::{Cond, ShiftType};

/// A general-purpose register number, 0 to 15.
///
/// [`Sem::reg`] and [`Sem::set_reg`] never see [`Reg::PC`]: the semantic
/// helpers turn pc reads into constants and pc writes into jumps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reg(pub u8);

impl Reg {
    pub const SP: Reg = Reg(13);
    pub const LR: Reg = Reg(14);
    pub const PC: Reg = Reg(15);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl From<crate::decode::Reg> for Reg {
    fn from(r: crate::decode::Reg) -> Reg {
        Reg(r as u8)
    }
}

/// One of the CPSR condition flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flag {
    N,
    Z,
    C,
    V,
}

impl Flag {
    pub const ALL: [Flag; 4] = [Flag::N, Flag::Z, Flag::C, Flag::V];

    /// The flag's bit position in the CPSR.
    pub fn cpsr_bit(self) -> u32 {
        match self {
            Flag::N => 31,
            Flag::Z => 30,
            Flag::C => 29,
            Flag::V => 28,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Flag::N => 0,
            Flag::Z => 1,
            Flag::C => 2,
            Flag::V => 3,
        }
    }
}

/// Which bytes of a PSR an `msr` writes, in the encoding's bit order
/// (instruction bits 16 to 19): control, extension, status, flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PsrMask(pub u8);

impl PsrMask {
    pub const CONTROL: PsrMask = PsrMask(0b0001);
    pub const EXTENSION: PsrMask = PsrMask(0b0010);
    pub const STATUS: PsrMask = PsrMask(0b0100);
    pub const FLAGS: PsrMask = PsrMask(0b1000);
    pub const ALL: PsrMask = PsrMask(0b1111);

    /// The PSR bits this mask selects.
    pub fn bits(self) -> u32 {
        (0..4)
            .filter(|i| self.0 & (1 << i) != 0)
            .fold(0, |m, i| m | (0xFF << (8 * i)))
    }

    pub fn contains(self, other: PsrMask) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The width of a memory access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Width {
    Byte,
    Half,
    Word,
}

impl Width {
    pub fn bytes(self) -> u32 {
        match self {
            Width::Byte => 1,
            Width::Half => 2,
            Width::Word => 4,
        }
    }
}

/// The operations semantic functions are written against.
///
/// Implementers: the interpreter (`V = u32`, `B = bool`), the IR builder
/// (`V = B = ValueId`), later an abstract interpreter. Rules every
/// implementation follows (details in `docs/sem-design.md`):
///
/// - **Effects happen in call order.** State reads see every earlier write
///   by the same instruction. Semantic functions read their operands before
///   writing anything, except where the hardware itself orders it otherwise.
/// - **Exits come last.** [`jump`](Sem::jump), [`jump_exchange`](Sem::jump_exchange),
///   [`exception_return`](Sem::exception_return), [`swi`](Sem::swi) and
///   [`undefined`](Sem::undefined) end the instruction. A semantic function
///   performs no effect after an exit on the same path.
/// - **No exit means fall through** to the next instruction; the caller of
///   the semantic function (interpreter loop, block builder) handles that.
/// - **pc is static.** Semantic functions get the instruction's address as a
///   plain `u32` and turn pc reads into [`imm`](Sem::imm) themselves, so
///   [`reg`](Sem::reg) and [`set_reg`](Sem::set_reg) never see r15.
pub trait Sem: Sized {
    /// A 32-bit value.
    type V: Copy;
    /// A one-bit value (flags and conditions).
    type B: Copy;

    // Constants.

    fn imm(&mut self, x: u32) -> Self::V;
    fn bit(&mut self, b: bool) -> Self::B;

    // Pure word operations. Exact meaning (the `u32` reference) is in `prim`.

    /// Wrapping addition, no flags.
    fn add(&mut self, a: Self::V, b: Self::V) -> Self::V;
    /// Wrapping subtraction, no flags.
    fn sub(&mut self, a: Self::V, b: Self::V) -> Self::V;
    fn and(&mut self, a: Self::V, b: Self::V) -> Self::V;
    fn or(&mut self, a: Self::V, b: Self::V) -> Self::V;
    fn xor(&mut self, a: Self::V, b: Self::V) -> Self::V;
    fn not(&mut self, a: Self::V) -> Self::V;
    /// Low 32 bits of the product.
    fn mul(&mut self, a: Self::V, b: Self::V) -> Self::V;
    /// The 64-bit product as `(low, high)`, signed or unsigned.
    fn mul_long(&mut self, signed: bool, a: Self::V, b: Self::V) -> (Self::V, Self::V);
    /// The ARM ARM's `AddWithCarry`: `a + b + c` as `(result, carry, overflow)`.
    /// Subtraction is `add_with_carry(a, not(b), 1)`.
    fn add_with_carry(&mut self, a: Self::V, b: Self::V, c: Self::B)
    -> (Self::V, Self::B, Self::B);
    /// A barrel shift by a register amount, as `(result, carry out)`. Uses the
    /// low byte of `amount`, with the ARM7TDMI's rules for amounts 0 and ≥ 32
    /// ([`prim::shift`]). Immediate shifts are mapped onto this by the
    /// semantic helpers, which handle the `#0` encodings statically.
    fn shift(
        &mut self,
        kind: ShiftType,
        v: Self::V,
        amount: Self::V,
        carry_in: Self::B,
    ) -> (Self::V, Self::B);
    /// C after a flag-setting multiply of `a` (rm) by `b` (rs), with the
    /// accumulator `acc_hi:acc_lo` (both 0 without one): the ARM7TDMI's
    /// rule, [`prim::mul_carry`].
    fn mul_carry(
        &mut self,
        kind: prim::MulKind,
        a: Self::V,
        b: Self::V,
        acc_lo: Self::V,
        acc_hi: Self::V,
    ) -> Self::B;
    /// Rotate right by one through carry, as `(result, carry out)`.
    fn rrx(&mut self, v: Self::V, carry_in: Self::B) -> (Self::V, Self::B);
    /// Sign-extend the low `from_bits` bits.
    fn sext(&mut self, v: Self::V, from_bits: u32) -> Self::V;
    /// `if c { a } else { b }`.
    fn select(&mut self, c: Self::B, a: Self::V, b: Self::V) -> Self::V;
    /// 0 or 1.
    fn bit_to_word(&mut self, b: Self::B) -> Self::V;
    /// Bit `n` of `v`.
    fn test_bit(&mut self, v: Self::V, n: u32) -> Self::B;
    fn is_zero(&mut self, v: Self::V) -> Self::B;

    // Pure bit operations.

    fn bnot(&mut self, a: Self::B) -> Self::B;
    fn band(&mut self, a: Self::B, b: Self::B) -> Self::B;
    fn bor(&mut self, a: Self::B, b: Self::B) -> Self::B;
    fn bxor(&mut self, a: Self::B, b: Self::B) -> Self::B;

    // CPU state. Registers are the current mode's bank; the implementer owns
    // banking.

    /// Read r0 to r14 of the current mode's bank.
    fn reg(&mut self, r: Reg) -> Self::V;
    /// Write r0 to r14 of the current mode's bank.
    fn set_reg(&mut self, r: Reg, v: Self::V);
    /// Read the user-mode bank (`stm ..^`).
    fn user_reg(&mut self, r: Reg) -> Self::V;
    /// Write the user-mode bank (`ldm ..^` without pc).
    fn set_user_reg(&mut self, r: Reg, v: Self::V);
    fn flag(&mut self, f: Flag) -> Self::B;
    fn set_flag(&mut self, f: Flag, b: Self::B);
    /// The whole CPSR, flags included.
    fn cpsr(&mut self) -> Self::V;
    /// Write the bytes of the CPSR selected by `mask`. Writing the control
    /// byte can switch mode (rebanking registers) and change the I/F bits;
    /// it never changes the T bit on purpose (the ARM ARM calls that
    /// unpredictable), but implementers store whatever bit 5 says.
    fn set_cpsr(&mut self, v: Self::V, mask: PsrMask);
    /// The current mode's SPSR. In user or system mode (no SPSR) the
    /// implementer returns the CPSR, as the ARM7TDMI does.
    fn spsr(&mut self) -> Self::V;
    /// Write the current mode's SPSR. Ignored in user or system mode.
    fn set_spsr(&mut self, v: Self::V, mask: PsrMask);

    // Memory. Raw bus accesses: the bus aligns the address down to the
    // access width, loads zero-extend, stores truncate. The ARM7TDMI's
    // rotation of misaligned loads and its sign-extension rules are written
    // in the semantic functions, not here.

    fn load(&mut self, w: Width, addr: Self::V) -> Self::V;
    fn store(&mut self, w: Width, addr: Self::V, v: Self::V);

    // Control.

    /// Run `f` only if `c` holds. Used for condition codes; `f` cannot return
    /// a value, so everything it does goes through effects.
    fn when(&mut self, c: Self::B, f: impl FnOnce(&mut Self));
    /// Exit: continue at `target` in the current instruction set. What
    /// happens to the low bits is the implementer's: the interpreter keeps
    /// both in ARM state and clears bit 0 in Thumb, as NanoBoyAdvance does
    /// (ARMv4 calls a misaligned ARM target unpredictable). Fetches align.
    fn jump(&mut self, target: Self::V);
    /// Exit: `bx`. Bit 0 of `target` selects Thumb (1) or ARM (0); the
    /// implementer sets T and clears bit 0 (the interpreter keeps bit 1 of
    /// an ARM target, as for [`jump`](Sem::jump)).
    fn jump_exchange(&mut self, target: Self::V);
    /// Exit: CPSR = SPSR (mode and T may change), then continue at `target`.
    /// The interpreter doesn't align `target` (NanoBoyAdvance doesn't
    /// either); fetches align. With no SPSR (user, system mode) CPSR stays
    /// as it is. Covers `movs pc, ..`, `subs pc, lr, #4`, and `ldm ..^` with
    /// pc in the list.
    fn exception_return(&mut self, target: Self::V);
    /// Exit: software interrupt with its comment field (24 bits in ARM, 8 in
    /// Thumb). The implementer either enters supervisor mode at 0x08 or runs
    /// the BIOS call in Rust (decision 12) and resumes after the `swi`.
    fn swi(&mut self, comment: u32);
    /// Exit: undefined-instruction exception.
    fn undefined(&mut self);
}

/// Evaluate a condition code. `None` means "always" (AL), so callers skip
/// the [`Sem::when`] entirely.
pub fn cond<S: Sem>(s: &mut S, c: Cond) -> Option<S::B> {
    let flag = |s: &mut S, f| s.flag(f);
    let b = match c {
        Cond::Al => return None,
        Cond::Nv => s.bit(false),
        Cond::Eq => flag(s, Flag::Z),
        Cond::Ne => {
            let z = flag(s, Flag::Z);
            s.bnot(z)
        }
        Cond::Cs => flag(s, Flag::C),
        Cond::Cc => {
            let c = flag(s, Flag::C);
            s.bnot(c)
        }
        Cond::Mi => flag(s, Flag::N),
        Cond::Pl => {
            let n = flag(s, Flag::N);
            s.bnot(n)
        }
        Cond::Vs => flag(s, Flag::V),
        Cond::Vc => {
            let v = flag(s, Flag::V);
            s.bnot(v)
        }
        Cond::Hi | Cond::Ls => {
            let carry = flag(s, Flag::C);
            let z = flag(s, Flag::Z);
            let nz = s.bnot(z);
            let hi = s.band(carry, nz);
            if c == Cond::Hi { hi } else { s.bnot(hi) }
        }
        Cond::Ge | Cond::Lt => {
            let n = flag(s, Flag::N);
            let v = flag(s, Flag::V);
            let lt = s.bxor(n, v);
            if c == Cond::Lt { lt } else { s.bnot(lt) }
        }
        Cond::Gt | Cond::Le => {
            let n = flag(s, Flag::N);
            let v = flag(s, Flag::V);
            let z = flag(s, Flag::Z);
            let lt = s.bxor(n, v);
            let le = s.bor(z, lt);
            if c == Cond::Le { le } else { s.bnot(le) }
        }
    };
    Some(b)
}

/// Run `f` under condition `c`. AL runs it unconditionally with no `when`, so
/// unconditional instructions (most of them) build no guard.
pub fn conditional<S: Sem>(s: &mut S, c: Cond, f: impl FnOnce(&mut S)) {
    match cond(s, c) {
        None => f(s),
        Some(b) => s.when(b, f),
    }
}

/// Read a register as an operand. r15 reads as the instruction's address
/// plus `pc_offset`: 8 for most ARM operands, 12 for ARM operands of an
/// instruction that shifts by a register or stores pc, 4 in Thumb.
pub fn read_reg<S: Sem>(s: &mut S, r: Reg, at: u32, pc_offset: u32) -> S::V {
    if r == Reg::PC {
        s.imm(at.wrapping_add(pc_offset))
    } else {
        s.reg(r)
    }
}

/// Write a register as a result. Writing r15 is a [`Sem::jump`], so this is
/// an exit when `r` is pc and must come last.
pub fn write_reg<S: Sem>(s: &mut S, r: Reg, v: S::V) {
    if r == Reg::PC {
        s.jump(v)
    } else {
        s.set_reg(r, v)
    }
}

/// Set N and Z from a result.
pub fn set_nz<S: Sem>(s: &mut S, result: S::V) {
    let n = s.test_bit(result, 31);
    let z = s.is_zero(result);
    s.set_flag(Flag::N, n);
    s.set_flag(Flag::Z, z);
}
