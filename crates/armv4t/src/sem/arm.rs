//! The ARMv4T semantics, one function per [`Instr`] variant, all generic over
//! [`Sem`]. The decoder folds Thumb into the ARM variants, so Thumb runs
//! through the same functions; [`execute`] is the entry point for both.
//!
//! Every function takes `at`, the instruction's own address, and `base`, the
//! pc offset of its instruction set: 8 in ARM, 4 in Thumb. A plain pc operand
//! reads `at + base`; the ARM forms that read pc one cycle later (an operand
//! of a shift by register, a stored pc) read [`late`]`(base)` ahead. The
//! condition code is applied by [`execute`].

use super::prim::MulKind;
use super::{Flag, PsrMask, Reg, Sem, ShiftType, Width, conditional, read_reg, set_nz, write_reg};
use crate::decode::{
    self, AluOp, BlockMode, Index, Instr, MemSize, MsrSrc, Offset, Operand2, Shift,
};

/// The pc offset of an instruction set: 8 in ARM, 4 in Thumb.
pub const fn pc_base(thumb: bool) -> u32 {
    if thumb { 4 } else { 8 }
}

/// pc as read one cycle late: one instruction further than `base` (12 in
/// ARM). Thumb never encodes the forms that read it, but an empty `stm`
/// stores it in both states.
const fn late(base: u32) -> u32 {
    base + base / 2
}

fn r(x: decode::Reg) -> Reg {
    x.into()
}

/// Run one decoded instruction: check its condition, then its semantics.
/// `thumb` picks the pc offset; the instruction itself is already decoded
/// for its instruction set.
pub fn execute<S: Sem>(s: &mut S, at: u32, thumb: bool, i: Instr) {
    let base = pc_base(thumb);
    conditional(s, i.cond(), |s| match i {
        Instr::Alu {
            op,
            s: set,
            rd,
            rn,
            op2,
            ..
        } => data_processing(s, at, base, op, set, r(rd), r(rn), op2),
        Instr::Mul {
            s: set,
            rd,
            rm,
            rs,
            acc,
            ..
        } => multiply(s, at, base, set, r(rd), r(rm), r(rs), acc.map(r)),
        Instr::MulLong {
            s: set,
            signed,
            acc,
            rd_lo,
            rd_hi,
            rm,
            rs,
            ..
        } => multiply_long(
            s,
            at,
            base,
            set,
            signed,
            acc,
            r(rd_lo),
            r(rd_hi),
            r(rm),
            r(rs),
        ),
        Instr::Load { size, rd, addr, .. } => load(s, at, base, size, r(rd), addr),
        Instr::Store { size, rd, addr, .. } => store(s, at, base, size, r(rd), addr),
        Instr::Block {
            load,
            mode,
            rn,
            writeback,
            regs,
            psr,
            ..
        } => block(s, at, base, load, mode, r(rn), writeback, regs, psr),
        Instr::Swap {
            byte, rd, rm, rn, ..
        } => swap(s, at, base, byte, r(rd), r(rm), r(rn)),
        Instr::Mrs { rd, spsr, .. } => mrs(s, r(rd), spsr),
        Instr::Msr {
            spsr, fields, src, ..
        } => msr(s, at, base, spsr, fields, src),
        Instr::Branch { link, target, .. } => branch(s, at, target, link),
        Instr::Bx { rm, .. } => bx(s, at, base, r(rm)),
        Instr::ThumbBlHi { offset } => thumb_bl_hi(s, at, offset),
        Instr::ThumbBlLo { offset } => thumb_bl_lo(s, at, offset),
        Instr::Swi { comment, .. } => s.swi(comment),
        Instr::Undefined { .. } => s.undefined(),
    });
}

/// A register shifted by an immediate (the decoder has already resolved the
/// `#0` encodings into `Lsr(32)`, `Asr(32)` and `Rrx`).
fn shift_imm<S: Sem>(s: &mut S, v: S::V, shift: Shift, carry_in: S::B) -> (S::V, S::B) {
    let (kind, n) = match shift {
        Shift::Lsl(0) => return (v, carry_in),
        Shift::Rrx => return s.rrx(v, carry_in),
        Shift::Lsl(n) => (ShiftType::Lsl, n),
        Shift::Lsr(n) => (ShiftType::Lsr, n),
        Shift::Asr(n) => (ShiftType::Asr, n),
        Shift::Ror(n) => (ShiftType::Ror, n),
    };
    let amount = s.imm(n as u32);
    s.shift(kind, v, amount, carry_in)
}

/// The shifter operand's value and carry out.
fn operand2<S: Sem>(s: &mut S, at: u32, base: u32, op2: Operand2, carry_in: S::B) -> (S::V, S::B) {
    match op2 {
        Operand2::Imm { value, rotate } => {
            let carry = if rotate == 0 {
                carry_in
            } else {
                s.bit(value >> 31 != 0)
            };
            (s.imm(value), carry)
        }
        Operand2::Reg { rm, shift } => {
            let v = read_reg(s, r(rm), at, base);
            shift_imm(s, v, shift, carry_in)
        }
        Operand2::RegShiftReg { rm, ty, rs } => {
            // Shifting by a register takes an extra cycle: rs is read in the
            // first, rm (and rn) in the second, when pc reads late.
            let amount = read_reg(s, r(rs), at, base);
            let v = read_reg(s, r(rm), at, late(base));
            s.shift(ty, v, amount, carry_in)
        }
    }
}

/// `and`, `add`, `cmp`, `mov` and the rest of the data-processing format,
/// with Thumb's ALU, shift, hi-register and `add rd, pc|sp` forms.
#[allow(clippy::too_many_arguments)]
pub fn data_processing<S: Sem>(
    s: &mut S,
    at: u32,
    base: u32,
    op: AluOp,
    set_flags: bool,
    rd: Reg,
    rn: Reg,
    op2: Operand2,
) {
    use AluOp::*;
    let c_in = s.flag(Flag::C);
    let (op2_v, shifter_carry) = operand2(s, at, base, op2, c_in);
    let rn_v = match op2 {
        Operand2::RegShiftReg { .. } => read_reg(s, rn, at, late(base)),
        // Thumb `add rd, pc, #imm` reads pc word-aligned.
        Operand2::Imm { .. } if rn == Reg::PC && base == 4 => s.imm(at.wrapping_add(base) & !3),
        _ => read_reg(s, rn, at, base),
    };

    // (result, carry, overflow); `None` leaves the flag alone.
    let (result, carry, overflow) = match op {
        And | Tst => (s.and(rn_v, op2_v), Some(shifter_carry), None),
        Eor | Teq => (s.xor(rn_v, op2_v), Some(shifter_carry), None),
        Orr => (s.or(rn_v, op2_v), Some(shifter_carry), None),
        Mov => (op2_v, Some(shifter_carry), None),
        Bic => {
            let n = s.not(op2_v);
            (s.and(rn_v, n), Some(shifter_carry), None)
        }
        Mvn => (s.not(op2_v), Some(shifter_carry), None),
        Add | Cmn | Adc | Sub | Cmp | Sbc | Rsb | Rsc => {
            let (a, b) = match op {
                Rsb | Rsc => (op2_v, rn_v),
                _ => (rn_v, op2_v),
            };
            let b = match op {
                Sub | Cmp | Sbc | Rsb | Rsc => s.not(b),
                _ => b,
            };
            let c = match op {
                Add | Cmn => s.bit(false),
                Sub | Cmp | Rsb => s.bit(true),
                _ => c_in,
            };
            let (res, c, v) = s.add_with_carry(a, b, c);
            (res, Some(c), Some(v))
        }
    };

    if set_flags {
        set_nz(s, result);
        if let Some(c) = carry {
            s.set_flag(Flag::C, c);
        }
        if let Some(v) = overflow {
            s.set_flag(Flag::V, v);
        }
    }
    if op.is_compare() {
        if set_flags && rd == Reg::PC {
            // A compare with rd = pc (the raw field) also copies SPSR to
            // CPSR on the ARM7TDMI, and doesn't branch.
            let spsr = s.spsr();
            s.set_cpsr(spsr, PsrMask::ALL);
        }
    } else if set_flags && rd == Reg::PC {
        // `movs pc, lr`, `subs pc, lr, #4`: CPSR comes from SPSR. In user
        // and system mode there is no SPSR and the flags set above stay.
        s.exception_return(result);
    } else {
        write_reg(s, rd, result);
    }
}

/// Set N, Z and C after a multiply. C is unpredictable on ARMv4; the
/// ARM7TDMI's rule is [`prim::mul_carry`](super::prim::mul_carry).
#[allow(clippy::too_many_arguments)]
fn set_mul_flags<S: Sem>(
    s: &mut S,
    kind: MulKind,
    sign: S::V,
    zero: S::B,
    a: S::V,
    b: S::V,
    acc_lo: S::V,
    acc_hi: S::V,
) {
    let n = s.test_bit(sign, 31);
    let c = s.mul_carry(kind, a, b, acc_lo, acc_hi);
    s.set_flag(Flag::N, n);
    s.set_flag(Flag::Z, zero);
    s.set_flag(Flag::C, c);
}

/// `mul`, `mla`, Thumb `mul`. Operands are read after pc has moved on, so
/// a pc operand reads late (unpredictable on ARMv4; NanoBoyAdvance's rule).
#[allow(clippy::too_many_arguments)]
pub fn multiply<S: Sem>(
    s: &mut S,
    at: u32,
    base: u32,
    set_flags: bool,
    rd: Reg,
    rm: Reg,
    rs: Reg,
    acc: Option<Reg>,
) {
    let a = read_reg(s, rm, at, late(base));
    let b = read_reg(s, rs, at, late(base));
    let product = s.mul(a, b);
    let (result, acc) = match acc {
        Some(rn) => {
            let c = read_reg(s, rn, at, late(base));
            (s.add(product, c), c)
        }
        None => (product, s.imm(0)),
    };
    if set_flags {
        let z = s.is_zero(result);
        let zero = s.imm(0);
        set_mul_flags(s, MulKind::Short, result, z, a, b, acc, zero);
    }
    write_reg(s, rd, result);
}

/// `umull`, `umlal`, `smull`, `smlal`. Operands read pc late, as in
/// [`multiply`].
#[allow(clippy::too_many_arguments)]
pub fn multiply_long<S: Sem>(
    s: &mut S,
    at: u32,
    base: u32,
    set_flags: bool,
    signed: bool,
    acc: bool,
    rd_lo: Reg,
    rd_hi: Reg,
    rm: Reg,
    rs: Reg,
) {
    let a = read_reg(s, rm, at, late(base));
    let b = read_reg(s, rs, at, late(base));
    let (mut lo, mut hi) = s.mul_long(signed, a, b);
    let (acc_lo, acc_hi) = if acc {
        let acc_lo = read_reg(s, rd_lo, at, late(base));
        let acc_hi = read_reg(s, rd_hi, at, late(base));
        let no_carry = s.bit(false);
        let (l, c, _) = s.add_with_carry(lo, acc_lo, no_carry);
        let (h, _, _) = s.add_with_carry(hi, acc_hi, c);
        (lo, hi) = (l, h);
        (acc_lo, acc_hi)
    } else {
        let zero = s.imm(0);
        (zero, zero)
    };
    if set_flags {
        let both = s.or(lo, hi);
        let z = s.is_zero(both);
        let kind = if signed {
            MulKind::LongSigned
        } else {
            MulKind::LongUnsigned
        };
        set_mul_flags(s, kind, hi, z, a, b, acc_lo, acc_hi);
    }
    // rd_lo then rd_hi, so rd_hi wins if they're the same register; a pc
    // destination (unpredictable) jumps after the other write.
    match (rd_lo, rd_hi) {
        (_, Reg::PC) => {
            if rd_lo != Reg::PC {
                s.set_reg(rd_lo, lo);
            }
            s.jump(hi);
        }
        (Reg::PC, _) => {
            s.set_reg(rd_hi, hi);
            s.jump(lo);
        }
        _ => {
            s.set_reg(rd_lo, lo);
            s.set_reg(rd_hi, hi);
        }
    }
}

/// The address a single transfer accesses and the value written back to the
/// base (`None` for no writeback).
///
/// Writeback to pc is unpredictable on ARMv4. NanoBoyAdvance (and so
/// SingleStepTests) computes the written-back pc from pc read late, after
/// the access has used the usual `at + 8`, and then jumps there.
fn address<S: Sem>(s: &mut S, at: u32, base: u32, a: decode::Addr) -> (S::V, Option<S::V>) {
    let rn = r(a.rn);
    let (offset, subtract) = match a.offset {
        Offset::Imm(i) => (s.imm(i.unsigned_abs()), i < 0),
        Offset::Reg {
            rm,
            shift,
            subtract,
        } => {
            // Only `rrx` needs the real carry; the carry out is unused.
            let c = if shift == Shift::Rrx {
                s.flag(Flag::C)
            } else {
                s.bit(false)
            };
            let v = read_reg(s, r(rm), at, base);
            (shift_imm(s, v, shift, c).0, subtract)
        }
    };
    let rn_v = match a.offset {
        // Thumb `ldr rd, [pc, #imm]` reads pc word-aligned.
        Offset::Imm(_) if rn == Reg::PC && base == 4 => s.imm(at.wrapping_add(base) & !3),
        _ => read_reg(s, rn, at, base),
    };
    let step = |s: &mut S, v| {
        if subtract {
            s.sub(v, offset)
        } else {
            s.add(v, offset)
        }
    };
    let moved = step(s, rn_v);
    let (addr, writeback) = match a.index {
        Index::Pre { writeback } => (moved, writeback),
        // `ldrt`/`strt` run as the plain forms: the GBA has no MMU.
        Index::Post { .. } => (rn_v, true),
    };
    let wb = match writeback {
        false => None,
        true if rn == Reg::PC => {
            let late_pc = s.imm(at.wrapping_add(late(base)));
            Some(step(s, late_pc))
        }
        true => Some(moved),
    };
    (addr, wb)
}

/// A word load as the ARM7TDMI does it: the bus reads the aligned word and
/// the core rotates it right by 8 × (addr & 3).
pub fn load_word_rotated<S: Sem>(s: &mut S, addr: S::V) -> S::V {
    let v = s.load(Width::Word, addr);
    rotate_by_low_bits(s, v, addr, 3)
}

/// Rotate `v` right by 8 × (`addr` & `mask`).
fn rotate_by_low_bits<S: Sem>(s: &mut S, v: S::V, addr: S::V, mask: u32) -> S::V {
    let three = s.imm(3);
    let mask = if mask == 3 { three } else { s.imm(mask) };
    let low = s.and(addr, mask);
    let no_carry = s.bit(false);
    let (bits, _) = s.shift(ShiftType::Lsl, low, three, no_carry);
    s.shift(ShiftType::Ror, v, bits, no_carry).0
}

/// `ldr`, `ldrb`, `ldrh`, `ldrsb`, `ldrsh`, in both states.
pub fn load<S: Sem>(s: &mut S, at: u32, base: u32, size: MemSize, rd: Reg, a: decode::Addr) {
    let (addr, writeback) = address(s, at, base, a);
    let v = match size {
        MemSize::Word => load_word_rotated(s, addr),
        MemSize::Byte => s.load(Width::Byte, addr),
        MemSize::SignedByte => {
            let v = s.load(Width::Byte, addr);
            s.sext(v, 8)
        }
        // A misaligned `ldrh` rotates the halfword like a misaligned `ldr`.
        MemSize::Half => {
            let v = s.load(Width::Half, addr);
            rotate_by_low_bits(s, v, addr, 1)
        }
        // A misaligned `ldrsh` loads the odd byte, sign-extended: the
        // aligned halfword's high byte, so shift the extended half right.
        MemSize::SignedHalf => {
            let v = s.load(Width::Half, addr);
            let v = s.sext(v, 16);
            let one = s.imm(1);
            let odd = s.and(addr, one);
            let three = s.imm(3);
            let no_carry = s.bit(false);
            let (bits, _) = s.shift(ShiftType::Lsl, odd, three, no_carry);
            s.shift(ShiftType::Asr, v, bits, no_carry).0
        }
    };
    // With rd == rn the loaded value wins, so skip the writeback. `ldr pc`
    // doesn't interwork on ARMv4: it stays in the same state.
    match writeback.filter(|_| r(a.rn) != rd) {
        Some(wb) if a.rn == decode::Reg::Pc => {
            write_reg(s, rd, v);
            s.jump(wb);
        }
        Some(wb) => {
            s.set_reg(r(a.rn), wb);
            write_reg(s, rd, v);
        }
        None => write_reg(s, rd, v),
    }
}

/// `str`, `strb`, `strh`, in both states.
pub fn store<S: Sem>(s: &mut S, at: u32, base: u32, size: MemSize, rd: Reg, a: decode::Addr) {
    let (addr, writeback) = address(s, at, base, a);
    // A stored pc reads late.
    let v = read_reg(s, rd, at, late(base));
    let w = match size {
        MemSize::Byte => Width::Byte,
        MemSize::Half => Width::Half,
        _ => Width::Word,
    };
    s.store(w, addr, v);
    if let Some(wb) = writeback {
        write_reg(s, r(a.rn), wb);
    }
}

/// `ldm`, `stm`, `push`, `pop` and Thumb `ldmia`/`stmia`.
///
/// Registers go to ascending addresses in ascending order. ARM7TDMI rules:
/// an empty list transfers pc alone and moves the base by 0x40; `stm` with
/// the base in the list stores the old base if it is the lowest register,
/// else the written-back one; `ldm` with the base in the list skips the
/// writeback. With `psr` (`^`): `stm`, and `ldm` without pc, use the user
/// bank; `ldm` with pc also copies SPSR to CPSR.
#[allow(clippy::too_many_arguments)]
pub fn block<S: Sem>(
    s: &mut S,
    at: u32,
    base: u32,
    load: bool,
    mode: BlockMode,
    rn: Reg,
    writeback: bool,
    regs: u16,
    psr: bool,
) {
    let list = if regs == 0 { 1 << 15 } else { regs };
    // Bytes the base moves by: an empty list counts as 16 registers.
    let size = if regs == 0 {
        0x40
    } else {
        4 * regs.count_ones()
    };
    let base_v = read_reg(s, rn, at, base);
    let (first, wb) = match mode {
        BlockMode::Ia => (0, size),
        BlockMode::Ib => (4, size),
        BlockMode::Da => (4u32.wrapping_sub(size), size.wrapping_neg()),
        BlockMode::Db => (size.wrapping_neg(), size.wrapping_neg()),
    };
    let first = s.imm(first);
    let mut addr = s.add(base_v, first);
    let wb = s.imm(wb);
    let new_base = s.add(base_v, wb);
    let four = s.imm(4);
    let in_list = list & (1 << rn.0) != 0;
    let user_bank = psr && !(load && list & (1 << 15) != 0);
    let regs_in = |list: u16| (0..16u8).filter(move |i| list & (1 << i) != 0).map(Reg);

    if load {
        // Writeback to pc (unpredictable) jumps once the loads are done.
        if writeback && !in_list && rn != Reg::PC {
            write_back(s, rn, new_base, user_bank);
        }
        let mut pc = None;
        for reg in regs_in(list) {
            let v = s.load(Width::Word, addr);
            addr = s.add(addr, four);
            match reg {
                Reg::PC => pc = Some(v),
                _ if user_bank => s.set_user_reg(reg, v),
                _ => s.set_reg(reg, v),
            }
        }
        match pc {
            Some(t) if psr => s.exception_return(t),
            Some(t) => s.jump(t),
            None if writeback && rn == Reg::PC => s.jump(new_base),
            None => {}
        }
    } else {
        let lowest = list.trailing_zeros() as u8;
        for reg in regs_in(list) {
            let v = match reg {
                _ if writeback && reg == rn && reg.0 != lowest => new_base,
                Reg::PC => s.imm(at.wrapping_add(late(base))),
                _ if user_bank => s.user_reg(reg),
                _ => s.reg(reg),
            };
            s.store(Width::Word, addr, v);
            addr = s.add(addr, four);
        }
        if writeback {
            write_back(s, rn, new_base, user_bank);
        }
    }
}

/// Write an `ldm`/`stm` base back. With `^` the transfer runs on the user
/// bank, and so does the writeback.
fn write_back<S: Sem>(s: &mut S, rn: Reg, v: S::V, user_bank: bool) {
    match rn {
        Reg::PC => s.jump(v),
        _ if user_bank => s.set_user_reg(rn, v),
        _ => s.set_reg(rn, v),
    }
}

/// `swp`, `swpb`: load from `[rn]`, store `rm` there, then write `rd`.
#[allow(clippy::too_many_arguments)]
pub fn swap<S: Sem>(s: &mut S, at: u32, base: u32, byte: bool, rd: Reg, rm: Reg, rn: Reg) {
    // NanoBoyAdvance reads pc late for both (unpredictable on ARMv4).
    let addr = read_reg(s, rn, at, late(base));
    let src = read_reg(s, rm, at, late(base));
    let v = if byte {
        s.load(Width::Byte, addr)
    } else {
        load_word_rotated(s, addr)
    };
    let w = if byte { Width::Byte } else { Width::Word };
    s.store(w, addr, src);
    write_reg(s, rd, v);
}

/// `mrs rd, cpsr|spsr`.
pub fn mrs<S: Sem>(s: &mut S, rd: Reg, spsr: bool) {
    let v = if spsr { s.spsr() } else { s.cpsr() };
    write_reg(s, rd, v);
}

/// `msr cpsr|spsr_<fields>, src`. Privilege checks (user mode writes only
/// the flags) are the implementer's: the mode is dynamic state.
pub fn msr<S: Sem>(
    s: &mut S,
    at: u32,
    base: u32,
    spsr: bool,
    fields: decode::PsrFields,
    src: MsrSrc,
) {
    let v = match src {
        MsrSrc::Imm(x) => s.imm(x),
        MsrSrc::Reg(rm) => read_reg(s, r(rm), at, base),
    };
    let mask = PsrMask(
        fields.c as u8 | (fields.x as u8) << 1 | (fields.s as u8) << 2 | (fields.f as u8) << 3,
    );
    if spsr {
        s.set_spsr(v, mask);
    } else {
        s.set_cpsr(v, mask);
    }
}

/// `b`, `bl` (ARM) and Thumb `b`, `b<cond>`. `target` is absolute; only ARM
/// links, so lr is the next ARM instruction.
pub fn branch<S: Sem>(s: &mut S, at: u32, target: u32, link: bool) {
    if link {
        let ret = s.imm(at.wrapping_add(4));
        s.set_reg(Reg::LR, ret);
    }
    let target = s.imm(target);
    s.jump(target);
}

/// `bx rm`, in both states.
pub fn bx<S: Sem>(s: &mut S, at: u32, base: u32, rm: Reg) {
    let target = read_reg(s, rm, at, base);
    s.jump_exchange(target);
}

/// The first half of Thumb `bl`: `lr = at + 4 + offset`.
pub fn thumb_bl_hi<S: Sem>(s: &mut S, at: u32, offset: i32) {
    let v = s.imm(at.wrapping_add(4).wrapping_add_signed(offset));
    s.set_reg(Reg::LR, v);
}

/// The second half of Thumb `bl`: jump to `lr + offset`, and lr = the next
/// instruction with bit 0 set.
pub fn thumb_bl_lo<S: Sem>(s: &mut S, at: u32, offset: u32) {
    let lr = s.reg(Reg::LR);
    let offset = s.imm(offset);
    let target = s.add(lr, offset);
    let ret = s.imm(at.wrapping_add(2) | 1);
    s.set_reg(Reg::LR, ret);
    s.jump(target);
}
