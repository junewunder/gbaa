//! The interpreter: [`Sem`] with `V = u32`, over a [`Cpu`] and a [`Bus`].
//!
//! The semantics in [`sem::arm`](crate::sem::arm) do the work; this module
//! owns the CPU state (register banks, PSRs, pc) and what the exits do.
//! Registers are banked the way NanoBoyAdvance banks them, which is what
//! SingleStepTests records: the current mode's registers are live in
//! [`Cpu::r`], and every other bank keeps the copy saved when it was last
//! switched out.

use crate::decode::{Cond, Instr, decode_arm, decode_thumb};
use crate::sem::{self, Flag, PsrMask, Reg, Sem, ShiftType, Width, prim};

/// Memory as the interpreter sees it. Addresses arrive aligned down to the
/// access width; loads return the value zero-extended, stores get it
/// unmasked and keep the low `w` bytes.
pub trait Bus {
    fn load(&mut self, w: Width, addr: u32) -> u32;
    fn store(&mut self, w: Width, addr: u32, v: u32);
}

/// The CPSR mode field values.
pub mod mode {
    pub const USR: u32 = 0x10;
    pub const FIQ: u32 = 0x11;
    pub const IRQ: u32 = 0x12;
    pub const SVC: u32 = 0x13;
    pub const ABT: u32 = 0x17;
    pub const UND: u32 = 0x1B;
    pub const SYS: u32 = 0x1F;
}

/// The CPSR's T bit.
pub const T_BIT: u32 = 1 << 5;
/// The CPSR's I bit (IRQs disabled).
pub const I_BIT: u32 = 1 << 7;

/// A register bank, indexing [`Cpu::banks`] and [`Cpu::spsr`]. The order
/// after `User` is SingleStepTests' (fiq, svc, abt, irq, und).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bank {
    /// User and system mode. Has no SPSR.
    User,
    Fiq,
    Svc,
    Abt,
    Irq,
    Und,
}

impl Bank {
    /// The bank a mode uses. Invalid modes use the undefined bank, as in
    /// NanoBoyAdvance.
    pub fn of_mode(m: u32) -> Bank {
        match m & 0x1F {
            mode::USR | mode::SYS => Bank::User,
            mode::FIQ => Bank::Fiq,
            mode::IRQ => Bank::Irq,
            mode::SVC => Bank::Svc,
            mode::ABT => Bank::Abt,
            _ => Bank::Und,
        }
    }
}

/// ARM7TDMI state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cpu {
    /// r0 to r14 of the current mode.
    pub r: [u32; 15],
    /// The address of the next instruction to execute (not the pipelined
    /// r15 value an instruction reads).
    pub pc: u32,
    pub cpsr: u32,
    /// r8 to r14 of each bank, as saved when it was last switched out. Only
    /// the user and FIQ banks use r8 to r12.
    pub banks: [[u32; 7]; 6],
    /// Each bank's SPSR; the user bank's slot is unused.
    pub spsr: [u32; 6],
}

impl Cpu {
    pub fn thumb(&self) -> bool {
        self.cpsr & T_BIT != 0
    }

    pub fn bank(&self) -> Bank {
        Bank::of_mode(self.cpsr)
    }

    /// Change the mode field, swapping banked registers in and out.
    pub fn switch_mode(&mut self, new_mode: u32) {
        let old = self.bank();
        let new = Bank::of_mode(new_mode);
        self.cpsr = (self.cpsr & !0x1F) | (new_mode & 0x1F);
        if old == new {
            return;
        }
        let (o, n) = (old as usize, new as usize);
        if old == Bank::Fiq || new == Bank::Fiq {
            // r8 to r12 swap only between the FIQ bank and everyone else's.
            let (o, n) = if old == Bank::Fiq { (o, 0) } else { (0, n) };
            self.banks[o][..5].copy_from_slice(&self.r[8..13]);
            self.r[8..13].copy_from_slice(&self.banks[n][..5]);
        }
        self.banks[o][5..].copy_from_slice(&self.r[13..15]);
        self.r[13..15].copy_from_slice(&self.banks[n][5..]);
    }

    /// Write the CPSR bits in `bits`, rebanking if the mode changes.
    /// No privilege check: callers apply it. Bit 4 of the mode always
    /// reads 1 on the ARM7TDMI.
    pub fn write_cpsr(&mut self, v: u32, bits: u32) {
        let new = (self.cpsr & !bits) | (v & bits) | 0x10;
        if (new ^ self.cpsr) & 0x1F != 0 {
            self.switch_mode(new);
        }
        self.cpsr = new;
    }

    /// The current mode's SPSR. User and system mode have none and read
    /// the CPSR.
    pub fn spsr(&self) -> u32 {
        match self.bank() {
            Bank::User => self.cpsr,
            b => self.spsr[b as usize],
        }
    }

    /// The user-mode copy of `r` (r0 to r14), wherever it lives now.
    pub fn user_reg(&self, r: Reg) -> u32 {
        let i = r.index();
        match (self.bank(), i) {
            (Bank::User, _) | (_, 0..=7) => self.r[i],
            (Bank::Fiq, _) | (_, 13..) => self.banks[Bank::User as usize][i - 8],
            _ => self.r[i],
        }
    }

    pub fn set_user_reg(&mut self, r: Reg, v: u32) {
        let i = r.index();
        match (self.bank(), i) {
            (Bank::User, _) | (_, 0..=7) => self.r[i] = v,
            (Bank::Fiq, _) | (_, 13..) => self.banks[Bank::User as usize][i - 8] = v,
            _ => self.r[i] = v,
        }
    }

    pub fn flag(&self, f: Flag) -> bool {
        self.cpsr & (1 << f.cpsr_bit()) != 0
    }

    pub fn set_flag(&mut self, f: Flag, b: bool) {
        let bit = 1 << f.cpsr_bit();
        self.cpsr = (self.cpsr & !bit) | if b { bit } else { 0 };
    }

    /// Whether condition `c` passes on the current flags.
    pub fn cond_passes(&self, c: Cond) -> bool {
        let (n, z, cf, v) = (
            self.flag(Flag::N),
            self.flag(Flag::Z),
            self.flag(Flag::C),
            self.flag(Flag::V),
        );
        match c {
            Cond::Eq => z,
            Cond::Ne => !z,
            Cond::Cs => cf,
            Cond::Cc => !cf,
            Cond::Mi => n,
            Cond::Pl => !n,
            Cond::Vs => v,
            Cond::Vc => !v,
            Cond::Hi => cf && !z,
            Cond::Ls => !cf || z,
            Cond::Ge => n == v,
            Cond::Lt => n != v,
            Cond::Gt => !z && n == v,
            Cond::Le => z || n != v,
            Cond::Al => true,
            Cond::Nv => false,
        }
    }

    /// `msr cpsr`: write the fields in `mask`. User mode can only write the
    /// flags.
    pub fn msr_cpsr(&mut self, v: u32, mask: PsrMask) {
        let mask = if self.bank() == Bank::User && self.cpsr & 0x1F == mode::USR {
            PsrMask(mask.0 & PsrMask::FLAGS.0)
        } else {
            mask
        };
        self.write_cpsr(v, mask.bits());
    }

    /// `msr spsr`: write the fields in `mask` of the current mode's SPSR.
    /// User and system mode have none, so there it does nothing.
    pub fn msr_spsr(&mut self, v: u32, mask: PsrMask) {
        let b = self.bank();
        if b != Bank::User {
            let bits = mask.bits();
            let old = &mut self.spsr[b as usize];
            *old = (*old & !bits) | (v & bits);
        }
    }

    /// Finish the instruction at `at`, run in `thumb` state: take its exit,
    /// or advance pc past it if it had none. Returns whether it exited.
    pub fn apply_exit(&mut self, at: u32, thumb: bool, exit: Option<Exit>) -> bool {
        let next = at.wrapping_add(if thumb { 2 } else { 4 });
        match exit {
            None => self.pc = next,
            // ARM keeps the low bits of a jump target (only fetches are
            // aligned), as NanoBoyAdvance does; Thumb clears bit 0.
            Some(Exit::Jump(t)) => self.pc = if thumb { t & !1 } else { t },
            Some(Exit::Exchange(t)) => {
                let thumb = t & 1 != 0;
                self.cpsr = (self.cpsr & !T_BIT) | if thumb { T_BIT } else { 0 };
                self.pc = if thumb { t & !1 } else { t };
            }
            Some(Exit::ExceptionReturn(t)) => {
                let spsr = self.spsr();
                self.write_cpsr(spsr, !0);
                self.pc = t;
            }
            Some(Exit::Swi) => self.enter_exception(mode::SVC, 0x08, next),
            Some(Exit::Undefined) => self.enter_exception(mode::UND, 0x04, next),
        }
        exit.is_some()
    }

    /// Take an exception: save CPSR to the new mode's SPSR, switch to
    /// `new_mode` in ARM state with IRQs off, set lr to `ret` and jump to
    /// `vector`.
    pub fn enter_exception(&mut self, new_mode: u32, vector: u32, ret: u32) {
        let old = self.cpsr;
        self.switch_mode(new_mode);
        self.spsr[self.bank() as usize] = old;
        self.cpsr = (self.cpsr & !T_BIT) | I_BIT;
        self.r[14] = ret;
        self.pc = vector;
    }

    /// Fetch, decode and run the instruction at `pc`. Returns whether it
    /// ended in an exit (a jump, an exception) rather than falling through.
    pub fn step(&mut self, bus: &mut impl Bus) -> bool {
        let raw = if self.thumb() {
            bus.load(Width::Half, self.pc & !1)
        } else {
            bus.load(Width::Word, self.pc & !3)
        };
        self.execute(bus, raw)
    }

    /// Run `raw` (a word in ARM state, a halfword in Thumb) as the
    /// instruction at `pc`. Returns whether it ended in an exit.
    pub fn execute(&mut self, bus: &mut impl Bus, raw: u32) -> bool {
        let at = self.pc;
        let instr = if self.thumb() {
            decode_thumb(raw as u16, at)
        } else {
            decode_arm(raw, at)
        };
        self.execute_decoded(bus, instr)
    }

    /// Run an already decoded instruction as the one at `pc`. Returns
    /// whether it ended in an exit.
    pub fn execute_decoded(&mut self, bus: &mut impl Bus, instr: Instr) -> bool {
        let at = self.pc;
        let thumb = self.thumb();
        let mut s = Interp {
            cpu: self,
            bus,
            exit: None,
        };
        sem::arm::execute(&mut s, at, thumb, instr);
        let exit = s.exit;
        self.apply_exit(at, thumb, exit)
    }
}

/// How an instruction ended, if not by falling through. Targets are as the
/// semantics gave them; [`Cpu::apply_exit`] aligns them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Jump(u32),
    Exchange(u32),
    ExceptionReturn(u32),
    Swi,
    Undefined,
}

/// The [`Sem`] implementation for one instruction.
struct Interp<'a, B> {
    cpu: &'a mut Cpu,
    bus: &'a mut B,
    exit: Option<Exit>,
}

impl<B> Interp<'_, B> {
    fn effect(&self) {
        debug_assert!(self.exit.is_none(), "effect after exit");
    }

    fn exit(&mut self, e: Exit) {
        self.effect();
        self.exit = Some(e);
    }
}

impl<B: Bus> Sem for Interp<'_, B> {
    type V = u32;
    type B = bool;

    fn imm(&mut self, x: u32) -> u32 {
        x
    }
    fn bit(&mut self, b: bool) -> bool {
        b
    }
    fn add(&mut self, a: u32, b: u32) -> u32 {
        a.wrapping_add(b)
    }
    fn sub(&mut self, a: u32, b: u32) -> u32 {
        a.wrapping_sub(b)
    }
    fn and(&mut self, a: u32, b: u32) -> u32 {
        a & b
    }
    fn or(&mut self, a: u32, b: u32) -> u32 {
        a | b
    }
    fn xor(&mut self, a: u32, b: u32) -> u32 {
        a ^ b
    }
    fn not(&mut self, a: u32) -> u32 {
        !a
    }
    fn mul(&mut self, a: u32, b: u32) -> u32 {
        a.wrapping_mul(b)
    }
    fn mul_long(&mut self, signed: bool, a: u32, b: u32) -> (u32, u32) {
        prim::mul_long(signed, a, b)
    }
    fn add_with_carry(&mut self, a: u32, b: u32, c: bool) -> (u32, bool, bool) {
        prim::add_with_carry(a, b, c)
    }
    fn shift(&mut self, kind: ShiftType, v: u32, amount: u32, carry_in: bool) -> (u32, bool) {
        prim::shift(kind, v, amount, carry_in)
    }
    fn mul_carry(&mut self, kind: prim::MulKind, a: u32, b: u32, lo: u32, hi: u32) -> bool {
        prim::mul_carry(kind, a, b, (hi as u64) << 32 | lo as u64)
    }
    fn rrx(&mut self, v: u32, carry_in: bool) -> (u32, bool) {
        prim::rrx(v, carry_in)
    }
    fn sext(&mut self, v: u32, from_bits: u32) -> u32 {
        prim::sext(v, from_bits)
    }
    fn select(&mut self, c: bool, a: u32, b: u32) -> u32 {
        if c { a } else { b }
    }
    fn bit_to_word(&mut self, b: bool) -> u32 {
        b as u32
    }
    fn test_bit(&mut self, v: u32, n: u32) -> bool {
        (v >> n) & 1 != 0
    }
    fn is_zero(&mut self, v: u32) -> bool {
        v == 0
    }
    fn bnot(&mut self, a: bool) -> bool {
        !a
    }
    fn band(&mut self, a: bool, b: bool) -> bool {
        a & b
    }
    fn bor(&mut self, a: bool, b: bool) -> bool {
        a | b
    }
    fn bxor(&mut self, a: bool, b: bool) -> bool {
        a ^ b
    }

    fn reg(&mut self, r: Reg) -> u32 {
        self.cpu.r[r.index()]
    }
    fn set_reg(&mut self, r: Reg, v: u32) {
        self.effect();
        self.cpu.r[r.index()] = v;
    }
    fn user_reg(&mut self, r: Reg) -> u32 {
        self.cpu.user_reg(r)
    }
    fn set_user_reg(&mut self, r: Reg, v: u32) {
        self.effect();
        self.cpu.set_user_reg(r, v);
    }
    fn flag(&mut self, f: Flag) -> bool {
        self.cpu.flag(f)
    }
    fn set_flag(&mut self, f: Flag, b: bool) {
        self.effect();
        self.cpu.set_flag(f, b);
    }
    fn cpsr(&mut self) -> u32 {
        self.cpu.cpsr
    }
    fn set_cpsr(&mut self, v: u32, mask: PsrMask) {
        self.effect();
        self.cpu.msr_cpsr(v, mask);
    }
    fn spsr(&mut self) -> u32 {
        self.cpu.spsr()
    }
    fn set_spsr(&mut self, v: u32, mask: PsrMask) {
        self.effect();
        self.cpu.msr_spsr(v, mask);
    }

    fn load(&mut self, w: Width, addr: u32) -> u32 {
        self.effect();
        self.bus.load(w, addr & !(w.bytes() - 1))
    }
    fn store(&mut self, w: Width, addr: u32, v: u32) {
        self.effect();
        self.bus.store(w, addr & !(w.bytes() - 1), v);
    }

    fn when(&mut self, c: bool, f: impl FnOnce(&mut Self)) {
        if c {
            f(self)
        }
    }
    fn jump(&mut self, target: u32) {
        self.exit(Exit::Jump(target));
    }
    fn jump_exchange(&mut self, target: u32) {
        self.exit(Exit::Exchange(target));
    }
    fn exception_return(&mut self, target: u32) {
        self.exit(Exit::ExceptionReturn(target));
    }
    fn swi(&mut self, _comment: u32) {
        self.exit(Exit::Swi);
    }
    fn undefined(&mut self) {
        self.exit(Exit::Undefined);
    }
}

#[cfg(test)]
mod tests;
