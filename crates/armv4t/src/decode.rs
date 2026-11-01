//! Instruction decoding: our own [`Instr`] enum, filled in by `unarm`
//! (decision 4 in `docs/decisions.md`).
//!
//! [`decode_arm`] and [`decode_thumb`] never panic: every 32-bit ARM word and
//! every 16-bit Thumb halfword maps to an [`Instr`], with anything the
//! ARM7TDMI doesn't execute as [`Instr::Undefined`].
//!
//! # How the adapter works
//!
//! `unarm` (pinned, `Version::V4T`) does the parsing, but it is stricter than
//! the hardware: it rejects encodings whose "should be zero/one" bits are
//! wrong, which the ARM7TDMI ignores. So each word is first *normalized*:
//! those bits are set to their canonical values, the result is parsed by
//! `unarm`, and any field that normalization erased is put back from the raw
//! word. The normalization follows mGBA, which classifies ARM words by bits
//! [27:20] and [7:4] alone, with one deliberate difference: `tst`/`cmp`
//! immediate with S = 0 (bits [27:20] = 0x30, 0x34) is `Undefined`, as the
//! ARMv4 ARM ARM says, where mGBA runs it as `tst`/`cmp`. The cases:
//!
//! - data processing: compares (`tst`/`teq`/`cmp`/`cmn`) with rd != 0, and
//!   `mov`/`mvn` with rn != 0 (the raw field is kept in [`Instr::Alu`]);
//! - `mul` with bits [15:12] != 0;
//! - halfword/signed transfers with bits [11:8] != 0 in the register form, or
//!   with W set on a post-indexed one (W is ignored);
//! - halfword/signed transfers with a pre-indexed register offset and
//!   writeback: unarm rejects `ldrh rd, [r0, rm]!` and friends (rn = r0),
//!   because they also match its `mov`/`movs` register-shift pattern;
//! - `swp` with bits [11:8] != 0; `mrs` with bits [19:16] != 0xF or
//!   [11:0] != 0; `msr` with bits [15:12] != 0xF, bits [11:4] != 0 in the
//!   register form, or an empty field mask (this is how `csdb`, E320F014,
//!   comes out: an `msr` that writes nothing);
//! - ARM `bx` with bits [19:8] != 0xFFF, and the 112 Thumb `bx` encodings
//!   with bits [2:0] != 0;
//! - empty register lists in `ldm`/`stm`/`push`/`pop`;
//! - condition 0b1111 (NV): decoded like AL, with [`Cond::Nv`].
//!
//! `unarm`'s `msr` immediate is wrong (it rotates by `rot`, not `rot * 2`), so
//! the immediate is recomputed from the raw word. Thumb `bl` is two
//! instructions here, [`Instr::ThumbBlHi`] and [`Instr::ThumbBlLo`];
//! [`pair_thumb_bl`] joins them.
//!
//! # Reading PC
//!
//! An operand that reads PC sees the instruction's address plus 8 (ARM) or
//! plus 4 (Thumb). Where `rn` is PC and the second operand is an immediate
//! (Thumb `ldr rd, [pc, #imm]` and `add rd, pc, #imm`), the value is
//! word-aligned (`& !3`). In ARM state that changes nothing, so the rule holds
//! for both states.
//!
//! Two ARM exceptions read PC one cycle later, as address plus 12: any
//! register operand of an [`Operand2::RegShiftReg`] instruction, and the PC
//! value written to memory by `str`/`stm` with r15 as a source.

use unarm::{Ins, Options, Version};

/// A decoded ARMv4T instruction, ARM or Thumb.
///
/// Thumb instructions are folded into the same variants as their ARM
/// equivalents (Thumb `lsl r0, r1, #2` is `Alu { op: Mov, s: true, .. }` with
/// a shifted register), so semantics only handle each operation once. Thumb
/// instructions have `cond: Cond::Al`, except conditional branches.
///
/// Register fields use the names in the ARM7TDMI TRM, not `unarm`'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Instr {
    /// Data processing: `rd = rn <op> op2`, setting flags if `s`.
    ///
    /// For the compares (`Tst`, `Teq`, `Cmp`, `Cmn`) nothing is written and
    /// `rd` is the raw "should be zero" field, kept because an ARM7TDMI with
    /// `rd == Pc` and `s` copies SPSR to CPSR, as for any `s` instruction
    /// writing PC. For `Mov` and `Mvn`, `rn` is the raw ignored field (`R0`
    /// in Thumb).
    Alu {
        cond: Cond,
        op: AluOp,
        s: bool,
        rd: Reg,
        rn: Reg,
        op2: Operand2,
    },
    /// `mul`/`mla`: `rd = rm * rs (+ acc)`. Thumb `mul rd, rs` is
    /// `rd = rs * rd`, i.e. `rm = rs`, `rs = rd`, as in the TRM's ARM
    /// equivalent.
    Mul {
        cond: Cond,
        s: bool,
        rd: Reg,
        rm: Reg,
        rs: Reg,
        /// The accumulate register (`mla`'s Rn), or `None` for `mul`.
        acc: Option<Reg>,
    },
    /// `umull`/`umlal`/`smull`/`smlal`: `rd_hi:rd_lo = rm * rs` (64-bit),
    /// plus `rd_hi:rd_lo` if `acc`.
    MulLong {
        cond: Cond,
        s: bool,
        signed: bool,
        acc: bool,
        rd_lo: Reg,
        rd_hi: Reg,
        rm: Reg,
        rs: Reg,
    },
    /// `ldr`, `ldrb`, `ldrh`, `ldrsb`, `ldrsh` (and the `t` forms, see
    /// [`Index::Post`]).
    Load {
        cond: Cond,
        size: MemSize,
        rd: Reg,
        addr: Addr,
    },
    /// `str`, `strb`, `strh`. `size` is never `SignedByte` or `SignedHalf`.
    Store {
        cond: Cond,
        size: MemSize,
        rd: Reg,
        addr: Addr,
    },
    /// `ldm`/`stm`, including `push`/`pop` (Thumb too) and Thumb
    /// `ldmia`/`stmia`, which always have `writeback` set (their encoding
    /// has no W bit).
    ///
    /// `regs` is the register list exactly as encoded, bit n = rn. It can be
    /// 0: an ARM7TDMI then transfers only r15 and moves the base by 0x40.
    Block {
        cond: Cond,
        load: bool,
        mode: BlockMode,
        rn: Reg,
        writeback: bool,
        regs: u16,
        /// The S bit (ARM only): user-bank registers, or for `ldm` with r15
        /// in the list, also SPSR → CPSR.
        psr: bool,
    },
    /// `swp`/`swpb`: `rd = [rn]; [rn] = rm`, atomically.
    Swap {
        cond: Cond,
        byte: bool,
        rd: Reg,
        rm: Reg,
        rn: Reg,
    },
    /// `mrs rd, cpsr|spsr`.
    Mrs { cond: Cond, rd: Reg, spsr: bool },
    /// `msr cpsr|spsr_<fields>, src`. `fields` can be empty (a no-op write).
    Msr {
        cond: Cond,
        spsr: bool,
        fields: PsrFields,
        src: MsrSrc,
    },
    /// `b`/`bl` (ARM) and `b`/`b<cond>` (Thumb). `target` is the absolute
    /// address, already including the pipeline offset. `link` sets LR to the
    /// next instruction's address.
    Branch { cond: Cond, link: bool, target: u32 },
    /// `bx rm`: jump to `rm & !1`, Thumb state if bit 0 is set.
    Bx { cond: Cond, rm: Reg },
    /// First half of Thumb `bl` (H = 0): `lr = pc + offset`, where pc is this
    /// halfword's address + 4. `offset` is sign-extended and already shifted
    /// left by 12. See [`pair_thumb_bl`].
    ThumbBlHi { offset: i32 },
    /// Second half of Thumb `bl` (H = 1): `pc = lr + offset`, then `lr` = this
    /// halfword's address + 2, with bit 0 set. `offset` is already shifted
    /// left by 1.
    ThumbBlLo { offset: u32 },
    /// `swi`: the 24-bit (ARM) or 8-bit (Thumb) comment field.
    Swi { cond: Cond, comment: u32 },
    /// Anything the ARM7TDMI doesn't execute: the undefined-instruction trap,
    /// taken only if `cond` passes. Includes all coprocessor instructions (the
    /// GBA has no coprocessors) and v5+ encodings. `raw` is the original word
    /// (ARM) or halfword (Thumb).
    Undefined { cond: Cond, raw: u32 },
}

impl Instr {
    /// The instruction's condition; `Al` for the Thumb `bl` halves.
    pub const fn cond(&self) -> Cond {
        match *self {
            Instr::Alu { cond, .. }
            | Instr::Mul { cond, .. }
            | Instr::MulLong { cond, .. }
            | Instr::Load { cond, .. }
            | Instr::Store { cond, .. }
            | Instr::Block { cond, .. }
            | Instr::Swap { cond, .. }
            | Instr::Mrs { cond, .. }
            | Instr::Msr { cond, .. }
            | Instr::Branch { cond, .. }
            | Instr::Bx { cond, .. }
            | Instr::Swi { cond, .. }
            | Instr::Undefined { cond, .. } => cond,
            Instr::ThumbBlHi { .. } | Instr::ThumbBlLo { .. } => Cond::Al,
        }
    }
}

/// Condition field. The discriminants are the encoding.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cond {
    Eq,
    Ne,
    /// Carry set (unsigned higher or same).
    Cs,
    /// Carry clear (unsigned lower).
    Cc,
    Mi,
    Pl,
    Vs,
    Vc,
    Hi,
    Ls,
    Ge,
    Lt,
    Gt,
    Le,
    Al,
    /// 0b1111: never executes on ARMv4.
    Nv,
}

impl Cond {
    /// The condition with encoding `bits & 0xF`.
    pub const fn from_bits(bits: u32) -> Cond {
        use Cond::*;
        [
            Eq, Ne, Cs, Cc, Mi, Pl, Vs, Vc, Hi, Ls, Ge, Lt, Gt, Le, Al, Nv,
        ][(bits & 0xF) as usize]
    }
}

/// A general-purpose register. The discriminants are the register number, so
/// `reg as usize` indexes a register file.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reg {
    R0,
    R1,
    R2,
    R3,
    R4,
    R5,
    R6,
    R7,
    R8,
    R9,
    R10,
    R11,
    R12,
    /// r13.
    Sp,
    /// r14.
    Lr,
    /// r15.
    Pc,
}

impl Reg {
    /// The register numbered `bits & 0xF`.
    pub const fn from_bits(bits: u32) -> Reg {
        use Reg::*;
        [
            R0, R1, R2, R3, R4, R5, R6, R7, R8, R9, R10, R11, R12, Sp, Lr, Pc,
        ][(bits & 0xF) as usize]
    }
}

/// Data-processing opcode. The discriminants are the encoding (bits [24:21]).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AluOp {
    And,
    Eor,
    Sub,
    Rsb,
    Add,
    Adc,
    Sbc,
    Rsc,
    Tst,
    Teq,
    Cmp,
    Cmn,
    Orr,
    Mov,
    Bic,
    Mvn,
}

impl AluOp {
    /// The opcode with encoding `bits & 0xF`.
    pub const fn from_bits(bits: u32) -> AluOp {
        use AluOp::*;
        [
            And, Eor, Sub, Rsb, Add, Adc, Sbc, Rsc, Tst, Teq, Cmp, Cmn, Orr, Mov, Bic, Mvn,
        ][(bits & 0xF) as usize]
    }

    /// `Tst`, `Teq`, `Cmp`, `Cmn`: set flags, write no register.
    pub const fn is_compare(self) -> bool {
        matches!(self, AluOp::Tst | AluOp::Teq | AluOp::Cmp | AluOp::Cmn)
    }

    /// `And`, `Eor`, `Tst`, `Teq`, `Orr`, `Mov`, `Bic`, `Mvn`: C comes from
    /// the shifter and V is unchanged.
    pub const fn is_logical(self) -> bool {
        matches!(
            self,
            AluOp::And
                | AluOp::Eor
                | AluOp::Tst
                | AluOp::Teq
                | AluOp::Orr
                | AluOp::Mov
                | AluOp::Bic
                | AluOp::Mvn
        )
    }
}

/// The second operand of [`Instr::Alu`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operand2 {
    /// An immediate, already rotated. `rotate` is the rotate amount in bits
    /// (0, 2, ..., 30); when it is nonzero, the shifter carry-out is bit 31 of
    /// `value`, otherwise C is unchanged.
    Imm { value: u32, rotate: u8 },
    /// A register shifted by an immediate. `Shift::Lsl(0)` is the plain
    /// register.
    Reg { rm: Reg, shift: Shift },
    /// A register shifted by the bottom byte of another register.
    RegShiftReg { rm: Reg, ty: ShiftType, rs: Reg },
}

/// A shift by an immediate, with the amount as the hardware applies it (the
/// encodings for LSR #32, ASR #32 and RRX are already resolved).
///
/// [`Shift::encoded`] gives back the raw form (type, 5-bit amount): RRX is
/// ROR #0, and LSR #32 / ASR #32 are amount 0. Every other value is
/// unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shift {
    /// 0..=31. LSL #0 leaves the value and C unchanged.
    Lsl(u8),
    /// 1..=32.
    Lsr(u8),
    /// 1..=32.
    Asr(u8),
    /// 1..=31.
    Ror(u8),
    /// Rotate right by one through carry.
    Rrx,
}

impl Shift {
    /// The shift as encoded: its type and the 5-bit amount field (bits
    /// [11:7] in ARM). The inverse of the decoder's resolution: `Rrx` →
    /// `(Ror, 0)`, `Lsr(32)` → `(Lsr, 0)`, `Asr(32)` → `(Asr, 0)`.
    pub const fn encoded(self) -> (ShiftType, u8) {
        match self {
            Shift::Lsl(n) => (ShiftType::Lsl, n),
            Shift::Lsr(n) => (ShiftType::Lsr, n & 31),
            Shift::Asr(n) => (ShiftType::Asr, n & 31),
            Shift::Ror(n) => (ShiftType::Ror, n),
            Shift::Rrx => (ShiftType::Ror, 0),
        }
    }
}

/// The kind of a register-specified shift.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShiftType {
    Lsl,
    Lsr,
    Asr,
    Ror,
}

/// Transfer size for [`Instr::Load`] and [`Instr::Store`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemSize {
    Word,
    Byte,
    Half,
    /// Loads only: sign-extended byte.
    SignedByte,
    /// Loads only: sign-extended halfword.
    SignedHalf,
}

/// The address of a single load or store: `rn` plus `offset`, indexed as
/// `index` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Addr {
    pub rn: Reg,
    pub offset: Offset,
    pub index: Index,
}

/// The offset in an [`Addr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Offset {
    /// Signed byte offset (the U bit is folded in).
    Imm(i32),
    /// A shifted register, subtracted if `subtract` (U = 0). Halfword and
    /// Thumb register offsets have `shift: Shift::Lsl(0)`.
    Reg {
        rm: Reg,
        shift: Shift,
        subtract: bool,
    },
}

/// Pre- or post-indexing for an [`Addr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Index {
    /// Access `rn + offset`; write it back to `rn` if `writeback`.
    Pre { writeback: bool },
    /// Access `rn`, then write `rn + offset` back to `rn` (always). `user`
    /// marks `ldrt`/`strt`/`ldrbt`/`strbt`, which access memory as user mode.
    Post { user: bool },
}

/// Address stepping for [`Instr::Block`]. The discriminants are the P and U
/// bits (bits [24:23]).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockMode {
    /// Decrement after.
    Da,
    /// Increment after (Thumb `ldmia`/`stmia`/`pop`).
    Ia,
    /// Decrement before (`push`).
    Db,
    /// Increment before.
    Ib,
}

/// The PSR field mask of [`Instr::Msr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PsrFields {
    /// Bits [7:0]: mode, T, I, F.
    pub c: bool,
    /// Bits [15:8].
    pub x: bool,
    /// Bits [23:16].
    pub s: bool,
    /// Bits [31:24]: N, Z, C, V.
    pub f: bool,
}

impl PsrFields {
    /// The bits of the PSR these fields cover.
    pub const fn mask(self) -> u32 {
        (if self.c { 0x0000_00FF } else { 0 })
            | (if self.x { 0x0000_FF00 } else { 0 })
            | (if self.s { 0x00FF_0000 } else { 0 })
            | (if self.f { 0xFF00_0000 } else { 0 })
    }
}

/// The source operand of [`Instr::Msr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MsrSrc {
    /// An 8-bit immediate rotated right by an even amount, already applied.
    Imm(u32),
    Reg(Reg),
}

/// Joins the two halves of a Thumb `bl`: if `hi` is a [`Instr::ThumbBlHi`]
/// halfword at `addr` and `lo` a [`Instr::ThumbBlLo`] halfword, returns the
/// call target. The halves can be executed separately, so this is a
/// convenience for analysis, not a requirement.
pub fn pair_thumb_bl(hi: u16, lo: u16, addr: u32) -> Option<u32> {
    match (
        decode_thumb(hi, addr),
        decode_thumb(lo, addr.wrapping_add(2)),
    ) {
        (Instr::ThumbBlHi { offset: hi }, Instr::ThumbBlLo { offset: lo }) => Some(
            addr.wrapping_add(4)
                .wrapping_add_signed(hi)
                .wrapping_add(lo),
        ),
        _ => None,
    }
}

fn options() -> Options {
    Options {
        version: Version::V4T,
        ..Options::default()
    }
}

/// What normalization erased and the adapter has to restore.
#[derive(Default)]
struct Fixups {
    /// ARM data processing: take rd and rn from the raw word.
    alu_regs: bool,
    /// The `msr` field mask was empty.
    msr_no_fields: bool,
    /// The register list was empty.
    empty_regs: bool,
    /// A halfword transfer with register offset had pre-indexed writeback.
    half_writeback: bool,
}

/// Decodes the ARM instruction `word` at address `addr` (used for branch
/// targets).
pub fn decode_arm(word: u32, addr: u32) -> Instr {
    let cond = Cond::from_bits(word >> 28);
    let undefined = Instr::Undefined { cond, raw: word };
    let Some((normal, fix)) = normalize_arm(word) else {
        return undefined;
    };
    let Some(mut instr) = from_unarm(unarm::parse_arm(normal, addr, &options())) else {
        return undefined;
    };
    match &mut instr {
        Instr::Alu { rd, rn, .. } if fix.alu_regs => {
            *rd = Reg::from_bits(word >> 12);
            *rn = Reg::from_bits(word >> 16);
        }
        Instr::Msr { fields, src, .. } => {
            if fix.msr_no_fields {
                *fields = PsrFields {
                    c: false,
                    x: false,
                    s: false,
                    f: false,
                };
            }
            if word & (1 << 25) != 0 {
                // unarm rotates by `rot` instead of `rot * 2`.
                *src = MsrSrc::Imm((word & 0xFF).rotate_right(((word >> 8) & 0xF) * 2));
            }
        }
        Instr::Block { regs, .. } if fix.empty_regs => *regs = 0,
        // unarm folds `str rd, [sp, #-4]!` and `ldr rd, [sp], #4` into
        // push/pop; they stay single transfers here.
        Instr::Block {
            load, regs, cond, ..
        } if word >> 25 & 7 == 0b010 => {
            let rd = Reg::from_bits(regs.trailing_zeros());
            instr = if *load {
                Instr::Load {
                    cond: *cond,
                    size: MemSize::Word,
                    rd,
                    addr: Addr {
                        rn: Reg::Sp,
                        offset: Offset::Imm(4),
                        index: Index::Post { user: false },
                    },
                }
            } else {
                Instr::Store {
                    cond: *cond,
                    size: MemSize::Word,
                    rd,
                    addr: Addr {
                        rn: Reg::Sp,
                        offset: Offset::Imm(-4),
                        index: Index::Pre { writeback: true },
                    },
                }
            };
        }
        Instr::Load { addr, .. } | Instr::Store { addr, .. } if fix.half_writeback => {
            addr.index = Index::Pre { writeback: true };
        }
        _ => {}
    }
    set_cond(&mut instr, cond);
    instr
}

/// `word` with its should-be-zero/one bits as the ARM7TDMI reads them (the
/// normalization above, without its parsing workarounds), or `word` itself
/// if it is undefined. Tools that compare with a stricter decoder use it to
/// tell those bits from a real difference.
pub fn canonical_arm(word: u32) -> u32 {
    match normalize_arm(word) {
        Some((w, fix)) => {
            let mut w = (w & 0x0FFF_FFFF) | (word & 0xF000_0000);
            if fix.empty_regs {
                w &= !1;
            }
            if fix.half_writeback {
                w |= 1 << 21;
            }
            if fix.msr_no_fields {
                w &= !0x1_0000;
            }
            w
        }
        None => word,
    }
}

/// [`canonical_arm`] for Thumb: `bx` with bits [2:0] clear.
pub fn canonical_thumb(half: u16) -> u16 {
    match half {
        0x4700..=0x477F => half & !0x7,
        _ => half,
    }
}

/// Rewrites `word` so that `unarm` accepts every encoding the ARM7TDMI
/// executes. Returns `None` for encodings it doesn't execute.
fn normalize_arm(word: u32) -> Option<(u32, Fixups)> {
    // Condition NV decodes like AL; decode_arm puts the real one back.
    let mut w = (word & 0x0FFF_FFFF) | 0xE000_0000;
    let mut fix = Fixups::default();
    let op = (w >> 20) & 0xFF; // bits [27:20]
    let lo = (w >> 4) & 0xF; // bits [7:4]
    match op >> 5 {
        // 000: data processing, multiplies, misc, halfword transfers.
        0b000 if lo & 0b1001 == 0b1001 => {
            if lo == 0b1001 {
                match op {
                    0x00..=0x03 if op & 0x2 == 0 => w &= !0xF000, // mul: Rn SBZ
                    0x00..=0x03 | 0x08..=0x0F => {}
                    0x10 | 0x14 => w &= !0xF00, // swp: SBZ
                    _ => return None,
                }
            } else {
                // Halfword and signed transfers.
                if op & 0x01 == 0 && lo != 0b1011 {
                    return None; // ldrd/strd slots (v5TE)
                }
                if op & 0x04 == 0 {
                    w &= !0xF00; // register offset: SBZ
                    if op & 0x12 == 0x12 {
                        // unarm rejects these with rn = r0 (they collide
                        // with its mov/movs patterns); parse without W.
                        w &= !(1 << 21);
                        fix.half_writeback = true;
                    }
                }
                if op & 0x10 == 0 {
                    w &= !(1 << 21); // post-indexed: W ignored
                }
            }
        }
        0b000 if op & 0x19 == 0x10 => match (op, lo) {
            (0x10 | 0x14, 0) => w = (w & !0xFFF) | 0xF_0000, // mrs
            (0x12 | 0x16, 0) => {
                w = (w & !0xFF0) | 0xF000; // msr register
                w = msr_mask(w, &mut fix);
            }
            (0x12, 1) => w |= 0xFFF00, // bx
            _ => return None,
        },
        0b001 if op & 0x19 == 0x10 => match op {
            0x32 | 0x36 => {
                w |= 0xF000; // msr immediate
                w = msr_mask(w, &mut fix);
            }
            _ => return None,
        },
        0b000 | 0b001 => {
            fix.alu_regs = true;
            match AluOp::from_bits(op >> 1) {
                AluOp::Tst | AluOp::Teq | AluOp::Cmp | AluOp::Cmn => w &= !0xF000,
                AluOp::Mov | AluOp::Mvn => w &= !0xF_0000,
                _ => {}
            }
        }
        0b100 if w & 0xFFFF == 0 => {
            w |= 1;
            fix.empty_regs = true;
        }
        _ => {}
    }
    Some((w, fix))
}

/// An empty `msr` field mask parses as a hint (`nop`, `csdb`, ...) in unarm;
/// give it one field and remember to clear it.
fn msr_mask(w: u32, fix: &mut Fixups) -> u32 {
    if w & 0xF_0000 == 0 {
        fix.msr_no_fields = true;
        w | 0x1_0000
    } else {
        w
    }
}

/// Decodes the Thumb instruction `half` at address `addr` (used for branch
/// targets).
pub fn decode_thumb(half: u16, addr: u32) -> Instr {
    let h = u32::from(half);
    let undefined = Instr::Undefined {
        cond: Cond::Al,
        raw: h,
    };
    let mut empty_regs = false;
    let normal = match h {
        // bx with H1 = 0; bits [2:0] are should-be-zero.
        0x4700..=0x477F => h & !0x7,
        // H1 = 1 is v5's blx.
        0x4780..=0x47FF => return undefined,
        // bl halves (H = 0, H = 1); 0xE800.. is v5's blx suffix.
        0xF000..=0xF7FF => {
            return Instr::ThumbBlHi {
                offset: (((h & 0x7FF) << 21) as i32) >> 9,
            };
        }
        0xF800..=0xFFFF => {
            return Instr::ThumbBlLo {
                offset: (h & 0x7FF) << 1,
            };
        }
        0xE800..=0xEFFF => return undefined,
        // push/pop/ldmia/stmia with an empty list.
        _ if (h & 0xF7FF == 0xB400 || h & 0xF000 == 0xC000) && h & 0xFF == 0 => {
            empty_regs = true;
            h | 1
        }
        _ => h,
    };
    // Only the low halfword is passed, so unarm never sees a `bl` pair.
    let (ins, _) = unarm::parse_thumb(normal, addr, &options());
    let Some(mut instr) = from_unarm(ins) else {
        return undefined;
    };
    // Hi-register add: unarm writes `add rd, sp, rd` as `rn = sp, rm = rd`;
    // keep the encoding's order (`rn = rd`).
    if let (0x4400..=0x44FF, Instr::Alu { rd, rn, op2, .. }) = (h, &mut instr) {
        *rn = *rd;
        *op2 = Operand2::Reg {
            rm: Reg::from_bits(h >> 3),
            shift: Shift::Lsl(0),
        };
    }
    if let Instr::Block {
        regs, writeback, ..
    } = &mut instr
    {
        if empty_regs {
            *regs = 0;
        }
        // Thumb ldmia/stmia always write back; unarm drops the `!` when the
        // base is in the list.
        if h & 0xF000 == 0xC000 {
            *writeback = true;
        }
    }
    instr
}

fn set_cond(instr: &mut Instr, new: Cond) {
    match instr {
        Instr::Alu { cond, .. }
        | Instr::Mul { cond, .. }
        | Instr::MulLong { cond, .. }
        | Instr::Load { cond, .. }
        | Instr::Store { cond, .. }
        | Instr::Block { cond, .. }
        | Instr::Swap { cond, .. }
        | Instr::Mrs { cond, .. }
        | Instr::Msr { cond, .. }
        | Instr::Branch { cond, .. }
        | Instr::Bx { cond, .. }
        | Instr::Swi { cond, .. }
        | Instr::Undefined { cond, .. } => *cond = new,
        Instr::ThumbBlHi { .. } | Instr::ThumbBlLo { .. } => {}
    }
}

fn reg(r: unarm::Reg) -> Reg {
    Reg::from_bits(r as u32)
}

fn cond(c: unarm::Cond) -> Cond {
    Cond::from_bits(c as u32)
}

fn shift_type(s: unarm::ShiftOp) -> ShiftType {
    match s {
        unarm::ShiftOp::Lsl => ShiftType::Lsl,
        unarm::ShiftOp::Lsr => ShiftType::Lsr,
        unarm::ShiftOp::Asr => ShiftType::Asr,
        unarm::ShiftOp::Ror => ShiftType::Ror,
    }
}

/// An immediate shift as unarm reports it (amount as encoded, except that
/// LSR/ASR #0 already reads as 32) to the amount the hardware applies.
fn shift(op: unarm::ShiftOp, imm: u32) -> Shift {
    let n = imm as u8;
    match op {
        unarm::ShiftOp::Lsl => Shift::Lsl(n),
        unarm::ShiftOp::Lsr => Shift::Lsr(if n == 0 { 32 } else { n }),
        unarm::ShiftOp::Asr => Shift::Asr(if n == 0 { 32 } else { n }),
        unarm::ShiftOp::Ror if n == 0 => Shift::Rrx,
        unarm::ShiftOp::Ror => Shift::Ror(n),
    }
}

fn op2(o: unarm::Op2) -> Operand2 {
    match o {
        unarm::Op2::Imm(i) => Operand2::Imm {
            value: i.imm,
            rotate: i.rotate_imm as u8,
        },
        unarm::Op2::ShiftImm(s) => Operand2::Reg {
            rm: reg(s.rm),
            shift: shift(s.shift_op, s.imm),
        },
        unarm::Op2::ShiftReg(s) => Operand2::RegShiftReg {
            rm: reg(s.rm),
            ty: shift_type(s.shift_op),
            rs: reg(s.rs),
        },
    }
}

/// unarm's `Lsl`/`Lsr`/`Asr`/`Ror` aliases: `rn` is the shifted register.
fn shift_alias(ty: ShiftType, rm: unarm::Reg, by: unarm::Op2Shift) -> Operand2 {
    let rm = reg(rm);
    match by {
        unarm::Op2Shift::Reg(rs) => Operand2::RegShiftReg {
            rm,
            ty,
            rs: reg(rs),
        },
        unarm::Op2Shift::Imm(n) => Operand2::Reg {
            rm,
            shift: match ty {
                ShiftType::Lsl => shift(unarm::ShiftOp::Lsl, n),
                ShiftType::Lsr => shift(unarm::ShiftOp::Lsr, n),
                ShiftType::Asr => shift(unarm::ShiftOp::Asr, n),
                ShiftType::Ror => shift(unarm::ShiftOp::Ror, n),
            },
        },
    }
}

fn ldr_str_offset(o: unarm::LdrStrOffset) -> Offset {
    match o {
        unarm::LdrStrOffset::Imm(i) => Offset::Imm(i),
        unarm::LdrStrOffset::Reg {
            subtract,
            rm,
            shift_op,
            imm,
        } => Offset::Reg {
            rm: reg(rm),
            shift: shift(shift_op, imm),
            subtract,
        },
    }
}

fn addr_ldr_str(a: unarm::AddrLdrStr) -> Addr {
    match a {
        unarm::AddrLdrStr::Pre {
            rn,
            offset,
            writeback,
        } => Addr {
            rn: reg(rn),
            offset: ldr_str_offset(offset),
            index: Index::Pre { writeback },
        },
        unarm::AddrLdrStr::Post(p) => addr_post(p, false),
    }
}

fn addr_post(p: unarm::AddrLdrStrPost, user: bool) -> Addr {
    Addr {
        rn: reg(p.rn),
        offset: ldr_str_offset(p.offset),
        index: Index::Post { user },
    }
}

fn misc_offset(o: unarm::MiscLoadOffset) -> Offset {
    match o {
        unarm::MiscLoadOffset::Imm(i) => Offset::Imm(i),
        unarm::MiscLoadOffset::Reg { subtract, rm } => Offset::Reg {
            rm: reg(rm),
            shift: Shift::Lsl(0),
            subtract,
        },
    }
}

fn addr_misc(a: unarm::AddrMiscLoad) -> Addr {
    match a {
        unarm::AddrMiscLoad::Pre {
            rn,
            offset,
            writeback,
        } => Addr {
            rn: reg(rn),
            offset: misc_offset(offset),
            index: Index::Pre { writeback },
        },
        unarm::AddrMiscLoad::Post { rn, offset } => Addr {
            rn: reg(rn),
            offset: misc_offset(offset),
            index: Index::Post { user: false },
        },
    }
}

fn block_mode(m: unarm::LdmStmMode) -> BlockMode {
    match m {
        unarm::LdmStmMode::Da => BlockMode::Da,
        unarm::LdmStmMode::Ia => BlockMode::Ia,
        unarm::LdmStmMode::Db => BlockMode::Db,
        unarm::LdmStmMode::Ib => BlockMode::Ib,
    }
}

/// Bit mask of a register list.
fn regs(list: unarm::RegList) -> u16 {
    list.iter().fold(0, |m, r| m | 1 << (r as u16))
}

/// Maps one of unarm's v4T instructions to ours; `None` for anything the
/// ARM7TDMI doesn't execute.
#[allow(unreachable_patterns)] // v5+ variants exist only if a crate enables unarm's other features
fn from_unarm(ins: Ins) -> Option<Instr> {
    use AluOp::*;
    let alu = |c, op, s, rd: unarm::Reg, rn: unarm::Reg, o| Instr::Alu {
        cond: cond(c),
        op,
        s,
        rd: reg(rd),
        rn: reg(rn),
        op2: o,
    };
    // `rn` of instructions that have none: r0, which decode_arm replaces with
    // the raw field.
    let r0 = unarm::Reg::R0;
    let shift_mov = |c, s, rd, o| Instr::Alu {
        cond: cond(c),
        op: Mov,
        s,
        rd: reg(rd),
        rn: Reg::R0,
        op2: o,
    };
    let load = |c, size, rd, addr| Instr::Load {
        cond: cond(c),
        size,
        rd: reg(rd),
        addr,
    };
    let store = |c, size, rd, addr| Instr::Store {
        cond: cond(c),
        size,
        rd: reg(rd),
        addr,
    };
    let block = |c, load, mode, rn, writeback, list, psr| Instr::Block {
        cond: cond(c),
        load,
        mode,
        rn: reg(rn),
        writeback,
        regs: regs(list),
        psr,
    };
    Some(match ins {
        Ins::And {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
            ..
        } => alu(c, And, s, rd, rn, op2(o)),
        Ins::Eor {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
            ..
        } => alu(c, Eor, s, rd, rn, op2(o)),
        Ins::Sub {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
            ..
        } => alu(c, Sub, s, rd, rn, op2(o)),
        Ins::Rsb {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
        } => alu(c, Rsb, s, rd, rn, op2(o)),
        Ins::Add {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
            ..
        } => alu(c, Add, s, rd, rn, op2(o)),
        Ins::Adc {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
            ..
        } => alu(c, Adc, s, rd, rn, op2(o)),
        Ins::Sbc {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
            ..
        } => alu(c, Sbc, s, rd, rn, op2(o)),
        Ins::Rsc {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
        } => alu(c, Rsc, s, rd, rn, op2(o)),
        Ins::Tst {
            cond: c,
            rn,
            op2: o,
        } => alu(c, Tst, true, r0, rn, op2(o)),
        Ins::Teq {
            cond: c,
            rn,
            op2: o,
        } => alu(c, Teq, true, r0, rn, op2(o)),
        Ins::Cmp {
            cond: c,
            rn,
            op2: o,
        } => alu(c, Cmp, true, r0, rn, op2(o)),
        Ins::Cmn {
            cond: c,
            rn,
            op2: o,
        } => alu(c, Cmn, true, r0, rn, op2(o)),
        Ins::Orr {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
            ..
        } => alu(c, Orr, s, rd, rn, op2(o)),
        Ins::Mov {
            cond: c,
            s,
            rd,
            op2: o,
            ..
        } => alu(c, Mov, s, rd, r0, op2(o)),
        Ins::Bic {
            cond: c,
            s,
            rd,
            rn,
            op2: o,
            ..
        } => alu(c, Bic, s, rd, rn, op2(o)),
        Ins::Mvn {
            cond: c,
            s,
            rd,
            op2: o,
            ..
        } => alu(c, Mvn, s, rd, r0, op2(o)),
        // Thumb `neg rd, rm` is `rsbs rd, rm, #0`.
        Ins::Neg { rd, rm } => alu(
            unarm::Cond::Al,
            Rsb,
            true,
            rd,
            rm,
            Operand2::Imm {
                value: 0,
                rotate: 0,
            },
        ),
        Ins::Lsl {
            cond: c,
            s,
            rd,
            rn,
            op2: by,
            ..
        } => shift_mov(c, s, rd, shift_alias(ShiftType::Lsl, rn, by)),
        Ins::Lsr {
            cond: c,
            s,
            rd,
            rn,
            op2: by,
            ..
        } => shift_mov(c, s, rd, shift_alias(ShiftType::Lsr, rn, by)),
        Ins::Asr {
            cond: c,
            s,
            rd,
            rn,
            op2: by,
            ..
        } => shift_mov(c, s, rd, shift_alias(ShiftType::Asr, rn, by)),
        Ins::Ror {
            cond: c,
            s,
            rd,
            rn,
            op2: by,
            ..
        } => shift_mov(c, s, rd, shift_alias(ShiftType::Ror, rn, by)),
        Ins::Rrx { cond: c, s, rd, rm } => shift_mov(
            c,
            s,
            rd,
            Operand2::Reg {
                rm: reg(rm),
                shift: Shift::Rrx,
            },
        ),
        // unarm names the multiply operands differently: its `rn` is the
        // TRM's Rm (bits [3:0]), its `rm` the TRM's Rs (bits [11:8]), and
        // its `ra` the TRM's Rn.
        Ins::Mul {
            cond: c,
            s,
            rd,
            rn,
            rm,
            ..
        } => Instr::Mul {
            cond: cond(c),
            s,
            rd: reg(rd),
            rm: reg(rn),
            rs: reg(rm),
            acc: None,
        },
        Ins::Mla {
            cond: c,
            s,
            rd,
            rn,
            rm,
            ra,
        } => Instr::Mul {
            cond: cond(c),
            s,
            rd: reg(rd),
            rm: reg(rn),
            rs: reg(rm),
            acc: Some(reg(ra)),
        },
        Ins::Umull {
            cond: c,
            s,
            rd_lo,
            rd_hi,
            rn,
            rm,
        } => mul_long(c, s, false, false, rd_lo, rd_hi, rn, rm),
        Ins::Umlal {
            cond: c,
            s,
            rd_lo,
            rd_hi,
            rn,
            rm,
        } => mul_long(c, s, false, true, rd_lo, rd_hi, rn, rm),
        Ins::Smull {
            cond: c,
            s,
            rd_lo,
            rd_hi,
            rn,
            rm,
        } => mul_long(c, s, true, false, rd_lo, rd_hi, rn, rm),
        Ins::Smlal {
            cond: c,
            s,
            rd_lo,
            rd_hi,
            rn,
            rm,
        } => mul_long(c, s, true, true, rd_lo, rd_hi, rn, rm),
        Ins::Ldr { cond: c, rd, addr } => load(c, MemSize::Word, rd, addr_ldr_str(addr)),
        Ins::Ldrb { cond: c, rd, addr } => load(c, MemSize::Byte, rd, addr_ldr_str(addr)),
        Ins::Ldrt { cond: c, rd, addr } => load(c, MemSize::Word, rd, addr_post(addr, true)),
        Ins::Ldrbt { cond: c, rd, addr } => load(c, MemSize::Byte, rd, addr_post(addr, true)),
        Ins::Ldrh { cond: c, rd, addr } => load(c, MemSize::Half, rd, addr_misc(addr)),
        Ins::Ldrsb { cond: c, rd, addr } => load(c, MemSize::SignedByte, rd, addr_misc(addr)),
        Ins::Ldrsh { cond: c, rd, addr } => load(c, MemSize::SignedHalf, rd, addr_misc(addr)),
        Ins::Str { cond: c, rd, addr } => store(c, MemSize::Word, rd, addr_ldr_str(addr)),
        Ins::Strb { cond: c, rd, addr } => store(c, MemSize::Byte, rd, addr_ldr_str(addr)),
        Ins::Strt { cond: c, rd, addr } => store(c, MemSize::Word, rd, addr_post(addr, true)),
        Ins::Strbt { cond: c, rd, addr } => store(c, MemSize::Byte, rd, addr_post(addr, true)),
        Ins::Strh { cond: c, rd, addr } => store(c, MemSize::Half, rd, addr_misc(addr)),
        Ins::Ldm {
            mode,
            cond: c,
            rn,
            writeback,
            regs: list,
            user_mode,
        } => block(c, true, block_mode(mode), rn, writeback, list, user_mode),
        Ins::Stm {
            mode,
            cond: c,
            rn,
            writeback,
            regs: list,
            user_mode,
        } => block(c, false, block_mode(mode), rn, writeback, list, user_mode),
        Ins::Push {
            cond: c,
            regs: list,
        } => block(c, false, BlockMode::Db, unarm::Reg::Sp, true, list, false),
        Ins::Pop {
            cond: c,
            regs: list,
        } => block(c, true, BlockMode::Ia, unarm::Reg::Sp, true, list, false),
        Ins::Swp {
            cond: c,
            rd,
            rd2,
            rn,
        } => Instr::Swap {
            cond: cond(c),
            byte: false,
            rd: reg(rd),
            rm: reg(rd2),
            rn: reg(rn),
        },
        Ins::Swpb {
            cond: c,
            rd,
            rd2,
            rn,
        } => Instr::Swap {
            cond: cond(c),
            byte: true,
            rd: reg(rd),
            rm: reg(rd2),
            rn: reg(rn),
        },
        Ins::Mrs {
            cond: c,
            rd,
            status_reg,
        } => Instr::Mrs {
            cond: cond(c),
            rd: reg(rd),
            spsr: status_reg == unarm::StatusReg::Spsr,
        },
        Ins::Msr {
            cond: c,
            status_fields: f,
            op2: o,
        } => Instr::Msr {
            cond: cond(c),
            spsr: f.reg == unarm::StatusReg::Spsr,
            fields: PsrFields {
                c: f.c,
                x: f.x,
                s: f.s,
                f: f.f,
            },
            // decode_arm recomputes the immediate (unarm's is wrong).
            src: match o {
                unarm::MsrOp2::Imm(i) => MsrSrc::Imm(i),
                unarm::MsrOp2::Reg(r) => MsrSrc::Reg(reg(r)),
            },
        },
        Ins::B { cond: c, target } => Instr::Branch {
            cond: cond(c),
            link: false,
            target: target.addr,
        },
        Ins::Bl { cond: c, target } => Instr::Branch {
            cond: cond(c),
            link: true,
            target: target.addr,
        },
        Ins::Bx { cond: c, rm } => Instr::Bx {
            cond: cond(c),
            rm: reg(rm),
        },
        Ins::Svc { cond: c, imm } => Instr::Swi {
            cond: cond(c),
            comment: imm,
        },
        // Coprocessor instructions, `udf`, `csdb`, data and v5+ encodings:
        // all take the undefined-instruction trap on the GBA.
        Ins::Cdp { .. }
        | Ins::Mcr { .. }
        | Ins::Mrc { .. }
        | Ins::Ldc { .. }
        | Ins::Stc { .. }
        | Ins::Udf { .. }
        | Ins::Csdb { .. }
        | Ins::Word(_)
        | Ins::HalfWord(_)
        | Ins::Byte(_)
        | Ins::Illegal => return None,
        _ => return None,
    })
}

#[allow(clippy::too_many_arguments)]
fn mul_long(
    c: unarm::Cond,
    s: bool,
    signed: bool,
    acc: bool,
    rd_lo: unarm::Reg,
    rd_hi: unarm::Reg,
    rn: unarm::Reg,
    rm: unarm::Reg,
) -> Instr {
    Instr::MulLong {
        cond: cond(c),
        s,
        signed,
        acc,
        rd_lo: reg(rd_lo),
        rd_hi: reg(rd_hi),
        rm: reg(rn),
        rs: reg(rm),
    }
}

#[cfg(test)]
mod tests;
