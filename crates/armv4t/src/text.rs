//! Assembly text for decoded instructions, in UAL syntax as
//! `arm-none-eabi-objdump` prints it: `lsls r0, r0, #17`,
//! `ldr r0, [pc, #36]`, `bl 0x080a6a54`. The readable form of the mod view
//! prints it in each `insn!` marker (`docs/mod-view-design.md`).
//!
//! An [`Instr`] doesn't say whether it was ARM or Thumb, and a few forms are
//! printed differently in each (Thumb's two-operand ALU forms, `negs`,
//! `muls rd, rm`, `push`/`pop`), so [`Instr::text`] takes the state.
//! Registers are `r0`-`r12`, `sp`, `lr`, `pc`. Immediates below 256 are
//! decimal, larger ones hex; addresses are hex.
//!
//! The halves of a Thumb `bl` print on their own as `bl.hi`/`bl.lo` with
//! their offsets; [`Bl`] prints the joined call. `just insn-text` checks the
//! text against objdump's.

use core::fmt::{self, Display, Formatter};

use crate::decode::{
    AluOp, BlockMode, Cond, Index, Instr, MemSize, MsrSrc, Offset, Operand2, PsrFields, Reg, Shift,
    ShiftType,
};

impl Instr {
    /// The instruction's assembly text; `thumb` is the state it was decoded
    /// in.
    pub const fn text(self, thumb: bool) -> Text {
        Text { instr: self, thumb }
    }
}

/// An instruction and its state, printed as assembly by `Display`.
#[derive(Debug, Clone, Copy)]
pub struct Text {
    pub instr: Instr,
    pub thumb: bool,
}

/// A whole Thumb `bl`, both halves: `bl 0x080a6a54`.
#[derive(Debug, Clone, Copy)]
pub struct Bl(pub u32);

impl Display for Bl {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "bl {:#010x}", self.0)
    }
}

impl Display for Reg {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Reg::Sp => f.write_str("sp"),
            Reg::Lr => f.write_str("lr"),
            Reg::Pc => f.write_str("pc"),
            r => write!(f, "r{}", *r as u8),
        }
    }
}

impl Display for Cond {
    /// The suffix: empty for `Al`.
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(match self {
            Cond::Eq => "eq",
            Cond::Ne => "ne",
            Cond::Cs => "cs",
            Cond::Cc => "cc",
            Cond::Mi => "mi",
            Cond::Pl => "pl",
            Cond::Vs => "vs",
            Cond::Vc => "vc",
            Cond::Hi => "hi",
            Cond::Ls => "ls",
            Cond::Ge => "ge",
            Cond::Lt => "lt",
            Cond::Gt => "gt",
            Cond::Le => "le",
            Cond::Al => "",
            Cond::Nv => "nv",
        })
    }
}

impl Display for AluOp {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(match self {
            AluOp::And => "and",
            AluOp::Eor => "eor",
            AluOp::Sub => "sub",
            AluOp::Rsb => "rsb",
            AluOp::Add => "add",
            AluOp::Adc => "adc",
            AluOp::Sbc => "sbc",
            AluOp::Rsc => "rsc",
            AluOp::Tst => "tst",
            AluOp::Teq => "teq",
            AluOp::Cmp => "cmp",
            AluOp::Cmn => "cmn",
            AluOp::Orr => "orr",
            AluOp::Mov => "mov",
            AluOp::Bic => "bic",
            AluOp::Mvn => "mvn",
        })
    }
}

impl Display for ShiftType {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(match self {
            ShiftType::Lsl => "lsl",
            ShiftType::Lsr => "lsr",
            ShiftType::Asr => "asr",
            ShiftType::Ror => "ror",
        })
    }
}

/// `#n`: decimal below 256 (and above -256), else hex.
struct Imm(i64);

impl Display for Imm {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            n @ -255..=255 => write!(f, "#{n}"),
            n if n < 0 => write!(f, "#-{:#x}", -n),
            n => write!(f, "#{n:#x}"),
        }
    }
}

/// `, lsl #2` and friends after a register; nothing for `lsl #0`.
struct ShiftText(Shift);

impl Display for ShiftText {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            Shift::Lsl(0) => Ok(()),
            Shift::Rrx => f.write_str(", rrx"),
            s => {
                let (ty, _) = s.encoded();
                let n = match s {
                    Shift::Lsl(n) | Shift::Lsr(n) | Shift::Asr(n) | Shift::Ror(n) => n,
                    Shift::Rrx => unreachable!(),
                };
                write!(f, ", {ty} #{n}")
            }
        }
    }
}

impl Display for Operand2 {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match *self {
            // A small value from a rotated encoding sets C from bit 31, so
            // keep the encoding visible, as objdump does.
            Operand2::Imm { value, rotate } if rotate != 0 && value < 256 => {
                write!(f, "#{}, {rotate}", value.rotate_left(rotate as u32))
            }
            Operand2::Imm { value, .. } => Imm(value as i64).fmt(f),
            Operand2::Reg { rm, shift } => write!(f, "{rm}{}", ShiftText(shift)),
            Operand2::RegShiftReg { rm, ty, rs } => write!(f, "{rm}, {ty} {rs}"),
        }
    }
}

/// The register list of `ldm`/`stm`: `{r0, r4, lr}`.
struct RegList(u16);

impl Display for RegList {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str("{")?;
        let mut first = true;
        for n in 0..16 {
            if self.0 & (1 << n) != 0 {
                if !first {
                    f.write_str(", ")?;
                }
                first = false;
                Reg::from_bits(n).fmt(f)?;
            }
        }
        f.write_str("}")
    }
}

impl Display for PsrFields {
    /// objdump's order: `fsxc`.
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        for (on, c) in [(self.f, "f"), (self.s, "s"), (self.x, "x"), (self.c, "c")] {
            if on {
                f.write_str(c)?;
            }
        }
        Ok(())
    }
}

fn s(on: bool) -> &'static str {
    if on { "s" } else { "" }
}

impl Display for Text {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let thumb = self.thumb;
        match self.instr {
            Instr::Alu {
                cond,
                op,
                s: set,
                rd,
                rn,
                op2,
            } => alu(f, thumb, cond, op, set, rd, rn, op2),
            Instr::Mul {
                cond,
                s: set,
                rd,
                rm,
                rs,
                acc,
            } => match acc {
                Some(rn) => write!(f, "mla{}{cond} {rd}, {rm}, {rs}, {rn}", s(set)),
                // Thumb `muls rd, rm` is `rd = rm * rd`.
                None if thumb && rs == rd => write!(f, "mul{}{cond} {rd}, {rm}", s(set)),
                None => write!(f, "mul{}{cond} {rd}, {rm}, {rs}", s(set)),
            },
            Instr::MulLong {
                cond,
                s: set,
                signed,
                acc,
                rd_lo,
                rd_hi,
                rm,
                rs,
            } => {
                let sign = if signed { "s" } else { "u" };
                let op = if acc { "mlal" } else { "mull" };
                write!(f, "{sign}{op}{}{cond} {rd_lo}, {rd_hi}, {rm}, {rs}", s(set))
            }
            Instr::Load {
                cond,
                size,
                rd,
                addr,
            } => {
                let sz = match size {
                    MemSize::Word => "",
                    MemSize::Byte => "b",
                    MemSize::Half => "h",
                    MemSize::SignedByte => "sb",
                    MemSize::SignedHalf => "sh",
                };
                let t = user(addr.index);
                write!(f, "ldr{sz}{t}{cond} {rd}, ")?;
                address(f, thumb, addr.rn, addr.offset, addr.index)
            }
            Instr::Store {
                cond,
                size,
                rd,
                addr,
            } => {
                let sz = match size {
                    MemSize::Byte | MemSize::SignedByte => "b",
                    MemSize::Half | MemSize::SignedHalf => "h",
                    MemSize::Word => "",
                };
                let t = user(addr.index);
                write!(f, "str{sz}{t}{cond} {rd}, ")?;
                address(f, thumb, addr.rn, addr.offset, addr.index)
            }
            Instr::Block {
                cond,
                load,
                mode,
                rn,
                writeback,
                regs,
                psr,
            } => {
                let list = RegList(regs);
                let hat = if psr { "^" } else { "" };
                let stack = rn == Reg::Sp && writeback && !psr;
                let many = regs.count_ones() > 1;
                match (load, mode) {
                    (true, BlockMode::Ia) if stack && (thumb || many) => {
                        return write!(f, "pop{cond} {list}");
                    }
                    (false, BlockMode::Db) if stack && (thumb || many) => {
                        return write!(f, "push{cond} {list}");
                    }
                    _ => {}
                }
                let op = if load { "ldm" } else { "stm" };
                let m = match mode {
                    BlockMode::Da => "da",
                    BlockMode::Ia if thumb => "ia",
                    BlockMode::Ia => "",
                    BlockMode::Db => "db",
                    BlockMode::Ib => "ib",
                };
                // Thumb `ldmia` with the base in the list doesn't write it
                // back: the loaded value wins.
                let wb = writeback && !(thumb && load && regs & (1 << rn as u8) != 0);
                let bang = if wb { "!" } else { "" };
                write!(f, "{op}{m}{cond} {rn}{bang}, {list}{hat}")
            }
            Instr::Swap {
                cond,
                byte,
                rd,
                rm,
                rn,
            } => {
                let b = if byte { "b" } else { "" };
                write!(f, "swp{b}{cond} {rd}, {rm}, [{rn}]")
            }
            Instr::Mrs { cond, rd, spsr } => write!(f, "mrs{cond} {rd}, {}", psr(spsr)),
            Instr::Msr {
                cond,
                spsr,
                fields,
                src,
            } => {
                write!(f, "msr{cond} {}_{fields}, ", psr(spsr))?;
                match src {
                    MsrSrc::Imm(v) => Imm(v as i64).fmt(f),
                    MsrSrc::Reg(r) => r.fmt(f),
                }
            }
            Instr::Branch { cond, link, target } => {
                let l = if link { "l" } else { "" };
                write!(f, "b{l}{cond} {target:#010x}")
            }
            Instr::Bx { cond, rm } => write!(f, "bx{cond} {rm}"),
            Instr::ThumbBlHi { offset } => write!(f, "bl.hi {}", Imm(offset as i64)),
            Instr::ThumbBlLo { offset } => write!(f, "bl.lo {}", Imm(offset as i64)),
            Instr::Swi { cond, comment } => write!(f, "svc{cond} {comment:#x}"),
            Instr::Undefined { raw, .. } if thumb => write!(f, ".short {raw:#06x}"),
            Instr::Undefined { raw, .. } => write!(f, ".word {raw:#010x}"),
        }
    }
}

fn psr(spsr: bool) -> &'static str {
    if spsr { "spsr" } else { "cpsr" }
}

fn user(index: Index) -> &'static str {
    match index {
        Index::Post { user: true } => "t",
        _ => "",
    }
}

#[allow(clippy::too_many_arguments)]
fn alu(
    f: &mut Formatter,
    thumb: bool,
    cond: Cond,
    op: AluOp,
    set: bool,
    rd: Reg,
    rn: Reg,
    op2: Operand2,
) -> fmt::Result {
    let sf = s(set);
    if op.is_compare() {
        // `cmpp`: rd = pc copies SPSR to CPSR on an ARM7TDMI.
        let p = if rd == Reg::Pc && !thumb { "p" } else { "" };
        return write!(f, "{op}{p}{cond} {rn}, {op2}");
    }
    if op == AluOp::Mov {
        // UAL writes a shifted `mov` as the shift.
        return match op2 {
            Operand2::Reg {
                rm,
                shift: Shift::Rrx,
            } => write!(f, "rrx{sf}{cond} {rd}, {rm}"),
            Operand2::Reg {
                rm,
                shift: Shift::Lsl(0),
            } => write!(f, "mov{sf}{cond} {rd}, {rm}"),
            Operand2::Reg { rm, shift } => {
                let (ty, _) = shift.encoded();
                let n = match shift {
                    Shift::Lsl(n) | Shift::Lsr(n) | Shift::Asr(n) | Shift::Ror(n) => n,
                    Shift::Rrx => unreachable!(),
                };
                write!(f, "{ty}{sf}{cond} {rd}, {rm}, #{n}")
            }
            Operand2::RegShiftReg { rm, ty, rs } if thumb && rm == rd => {
                write!(f, "{ty}{sf}{cond} {rd}, {rs}")
            }
            Operand2::RegShiftReg { rm, ty, rs } => write!(f, "{ty}{sf}{cond} {rd}, {rm}, {rs}"),
            Operand2::Imm { .. } => write!(f, "mov{sf}{cond} {rd}, {op2}"),
        };
    }
    if op == AluOp::Mvn {
        return write!(f, "mvn{sf}{cond} {rd}, {op2}");
    }
    if thumb {
        if let (AluOp::Rsb, Operand2::Imm { value: 0, .. }) = (op, op2) {
            return write!(f, "neg{sf}{cond} {rd}, {rn}");
        }
        // Thumb's two-operand forms: `adds r0, #1`, `ands r0, r1`,
        // `add sp, #8` (and `sub sp, #8`, a negative add).
        if rd == rn {
            if let (AluOp::Add, Reg::Sp, Operand2::Imm { value, .. }) = (op, rd, op2)
                && (value as i32) < 0
            {
                return write!(f, "sub{sf}{cond} sp, {}", Imm(-(value as i32 as i64)));
            }
            return write!(f, "{op}{sf}{cond} {rd}, {op2}");
        }
    }
    write!(f, "{op}{sf}{cond} {rd}, {rn}, {op2}")
}

/// `[rn, #4]`, `[rn, -rm, lsl #2]!`, `[rn], #4`. ARM drops a zero offset
/// (`[rn]`); Thumb, whose immediate forms always have one, keeps it.
fn address(f: &mut Formatter, thumb: bool, rn: Reg, offset: Offset, index: Index) -> fmt::Result {
    let off = OffsetText(offset);
    match index {
        Index::Pre { writeback: false } if offset == Offset::Imm(0) && !thumb => {
            write!(f, "[{rn}]")
        }
        Index::Pre { writeback } => {
            let bang = if writeback { "!" } else { "" };
            write!(f, "[{rn}, {off}]{bang}")
        }
        Index::Post { .. } => write!(f, "[{rn}], {off}"),
    }
}

struct OffsetText(Offset);

impl Display for OffsetText {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            // Offsets stay decimal, as objdump prints them.
            Offset::Imm(n) => write!(f, "#{n}"),
            Offset::Reg {
                rm,
                shift,
                subtract,
            } => {
                let minus = if subtract { "-" } else { "" };
                write!(f, "{minus}{rm}{}", ShiftText(shift))
            }
        }
    }
}

#[cfg(test)]
mod tests;
