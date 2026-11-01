//! An independent decoder written from the ARM7TDMI TRM and GBATEK bit
//! layouts, without `unarm`. Classification follows the same rule as the
//! adapter (mGBA's: bits [27:20] and [7:4], "should be" bits ignored).

use super::super::*;

fn bit(w: u32, n: u32) -> bool {
    w >> n & 1 != 0
}

fn r(w: u32, at: u32) -> Reg {
    Reg::from_bits(w >> at)
}

fn imm_shift(ty: u32, amt: u32) -> Shift {
    let n = amt as u8;
    match ty & 3 {
        0 => Shift::Lsl(n),
        1 => Shift::Lsr(if n == 0 { 32 } else { n }),
        2 => Shift::Asr(if n == 0 { 32 } else { n }),
        _ if n == 0 => Shift::Rrx,
        _ => Shift::Ror(n),
    }
}

fn shift_type(ty: u32) -> ShiftType {
    [
        ShiftType::Lsl,
        ShiftType::Lsr,
        ShiftType::Asr,
        ShiftType::Ror,
    ][(ty & 3) as usize]
}

fn fields(w: u32) -> PsrFields {
    PsrFields {
        c: bit(w, 16),
        x: bit(w, 17),
        s: bit(w, 18),
        f: bit(w, 19),
    }
}

fn signed(u: bool, imm: u32) -> i32 {
    if u { imm as i32 } else { -(imm as i32) }
}

pub fn arm(w: u32, addr: u32) -> Instr {
    let cond = Cond::from_bits(w >> 28);
    let und = Instr::Undefined { cond, raw: w };
    let (p, u, b, wb, l) = (bit(w, 24), bit(w, 23), bit(w, 22), bit(w, 21), bit(w, 20));
    match w >> 25 & 7 {
        0b000 if bit(w, 7) && bit(w, 4) => {
            let sh = w >> 5 & 3;
            if sh == 0 {
                if w & 0x0FC0_0000 == 0 {
                    Instr::Mul {
                        cond,
                        s: l,
                        rd: r(w, 16),
                        rm: r(w, 0),
                        rs: r(w, 8),
                        acc: wb.then(|| r(w, 12)),
                    }
                } else if w & 0x0F80_0000 == 0x0080_0000 {
                    Instr::MulLong {
                        cond,
                        s: l,
                        signed: b,
                        acc: wb,
                        rd_lo: r(w, 12),
                        rd_hi: r(w, 16),
                        rm: r(w, 0),
                        rs: r(w, 8),
                    }
                } else if w & 0x0FB0_0000 == 0x0100_0000 {
                    Instr::Swap {
                        cond,
                        byte: b,
                        rd: r(w, 12),
                        rm: r(w, 0),
                        rn: r(w, 16),
                    }
                } else {
                    und
                }
            } else {
                if !l && sh != 1 {
                    return und;
                }
                let size = match sh {
                    1 => MemSize::Half,
                    2 => MemSize::SignedByte,
                    _ => MemSize::SignedHalf,
                };
                let offset = if b {
                    Offset::Imm(signed(u, (w >> 4 & 0xF0) | (w & 0xF)))
                } else {
                    Offset::Reg {
                        rm: r(w, 0),
                        shift: Shift::Lsl(0),
                        subtract: !u,
                    }
                };
                let index = if p {
                    Index::Pre { writeback: wb }
                } else {
                    Index::Post { user: false }
                };
                let addr = Addr {
                    rn: r(w, 16),
                    offset,
                    index,
                };
                if l {
                    Instr::Load {
                        cond,
                        size,
                        rd: r(w, 12),
                        addr,
                    }
                } else {
                    Instr::Store {
                        cond,
                        size,
                        rd: r(w, 12),
                        addr,
                    }
                }
            }
        }
        0b000 if w & 0x0190_0000 == 0x0100_0000 => match (wb, w >> 4 & 0xF) {
            (false, 0) => Instr::Mrs {
                cond,
                rd: r(w, 12),
                spsr: b,
            },
            (true, 0) => Instr::Msr {
                cond,
                spsr: b,
                fields: fields(w),
                src: MsrSrc::Reg(r(w, 0)),
            },
            (true, 1) if !b => Instr::Bx { cond, rm: r(w, 0) },
            _ => und,
        },
        0b001 if w & 0x0190_0000 == 0x0100_0000 => {
            if wb {
                Instr::Msr {
                    cond,
                    spsr: b,
                    fields: fields(w),
                    src: MsrSrc::Imm((w & 0xFF).rotate_right((w >> 8 & 0xF) * 2)),
                }
            } else {
                und
            }
        }
        op @ (0b000 | 0b001) => {
            let op2 = if op == 0b001 {
                let rot = (w >> 8 & 0xF) * 2;
                Operand2::Imm {
                    value: (w & 0xFF).rotate_right(rot),
                    rotate: rot as u8,
                }
            } else if bit(w, 4) {
                Operand2::RegShiftReg {
                    rm: r(w, 0),
                    ty: shift_type(w >> 5),
                    rs: r(w, 8),
                }
            } else {
                Operand2::Reg {
                    rm: r(w, 0),
                    shift: imm_shift(w >> 5, w >> 7 & 31),
                }
            };
            Instr::Alu {
                cond,
                op: AluOp::from_bits(w >> 21),
                s: l,
                rd: r(w, 12),
                rn: r(w, 16),
                op2,
            }
        }
        op @ (0b010 | 0b011) => {
            if op == 0b011 && bit(w, 4) {
                return und;
            }
            let offset = if op == 0b010 {
                Offset::Imm(signed(u, w & 0xFFF))
            } else {
                Offset::Reg {
                    rm: r(w, 0),
                    shift: imm_shift(w >> 5, w >> 7 & 31),
                    subtract: !u,
                }
            };
            let index = if p {
                Index::Pre { writeback: wb }
            } else {
                Index::Post { user: wb }
            };
            let size = if b { MemSize::Byte } else { MemSize::Word };
            let addr = Addr {
                rn: r(w, 16),
                offset,
                index,
            };
            if l {
                Instr::Load {
                    cond,
                    size,
                    rd: r(w, 12),
                    addr,
                }
            } else {
                Instr::Store {
                    cond,
                    size,
                    rd: r(w, 12),
                    addr,
                }
            }
        }
        0b100 => Instr::Block {
            cond,
            load: l,
            mode: [BlockMode::Da, BlockMode::Ia, BlockMode::Db, BlockMode::Ib]
                [(w >> 23 & 3) as usize],
            rn: r(w, 16),
            writeback: wb,
            regs: w as u16,
            psr: b,
        },
        0b101 => Instr::Branch {
            cond,
            link: p,
            target: addr
                .wrapping_add(8)
                .wrapping_add((((w << 8) as i32) >> 6) as u32),
        },
        0b111 if p => Instr::Swi {
            cond,
            comment: w & 0xFF_FFFF,
        },
        _ => und,
    }
}

pub fn thumb(half: u16, addr: u32) -> Instr {
    let h = u32::from(half);
    let al = Cond::Al;
    let und = Instr::Undefined { cond: al, raw: h };
    let lo = |at: u32| Reg::from_bits(h >> at & 7);
    let alu = |op, s, rd, rn, op2| Instr::Alu {
        cond: al,
        op,
        s,
        rd,
        rn,
        op2,
    };
    let reg = |rm| Operand2::Reg {
        rm,
        shift: Shift::Lsl(0),
    };
    let imm = |value| Operand2::Imm { value, rotate: 0 };
    let mem = |load, size, rd, rn, offset| {
        let addr = Addr {
            rn,
            offset,
            index: Index::Pre { writeback: false },
        };
        if load {
            Instr::Load {
                cond: al,
                size,
                rd,
                addr,
            }
        } else {
            Instr::Store {
                cond: al,
                size,
                rd,
                addr,
            }
        }
    };
    let reg_off = |rm| Offset::Reg {
        rm,
        shift: Shift::Lsl(0),
        subtract: false,
    };
    match h >> 13 {
        0b000 if h >> 11 & 3 == 3 => {
            // Format 2: add/sub register or 3-bit immediate.
            let n = h >> 6 & 7;
            let op2 = if bit(h, 10) {
                imm(n)
            } else {
                reg(Reg::from_bits(n))
            };
            let op = if bit(h, 9) { AluOp::Sub } else { AluOp::Add };
            alu(op, true, lo(0), lo(3), op2)
        }
        0b000 => {
            // Format 1: move shifted register.
            let shift = imm_shift(h >> 11, h >> 6 & 31);
            alu(
                AluOp::Mov,
                true,
                lo(0),
                Reg::R0,
                Operand2::Reg { rm: lo(3), shift },
            )
        }
        0b001 => {
            // Format 3: mov/cmp/add/sub 8-bit immediate.
            let rd = lo(8);
            let i = imm(h & 0xFF);
            match h >> 11 & 3 {
                0 => alu(AluOp::Mov, true, rd, Reg::R0, i),
                1 => alu(AluOp::Cmp, true, Reg::R0, rd, i),
                2 => alu(AluOp::Add, true, rd, rd, i),
                _ => alu(AluOp::Sub, true, rd, rd, i),
            }
        }
        0b010 if bit(h, 12) => {
            // Formats 7 and 8: register offset.
            let (rd, rb, ro) = (lo(0), lo(3), lo(6));
            let (load, size) = match (bit(h, 9), h >> 10 & 3) {
                (false, 0) => (false, MemSize::Word),
                (false, 1) => (false, MemSize::Byte),
                (false, 2) => (true, MemSize::Word),
                (false, _) => (true, MemSize::Byte),
                (true, 0) => (false, MemSize::Half),
                (true, 1) => (true, MemSize::SignedByte),
                (true, 2) => (true, MemSize::Half),
                (true, _) => (true, MemSize::SignedHalf),
            };
            mem(load, size, rd, rb, reg_off(ro))
        }
        0b010 if bit(h, 11) => {
            // Format 6: PC-relative load.
            mem(
                true,
                MemSize::Word,
                lo(8),
                Reg::Pc,
                Offset::Imm(((h & 0xFF) << 2) as i32),
            )
        }
        0b010 if bit(h, 10) => {
            // Format 5: hi-register operations and bx.
            let rd = Reg::from_bits((h & 7) | (h >> 4 & 8));
            let rm = Reg::from_bits(h >> 3 & 0xF);
            match h >> 8 & 3 {
                0 => alu(AluOp::Add, false, rd, rd, reg(rm)),
                1 => alu(AluOp::Cmp, true, Reg::R0, rd, reg(rm)),
                2 => alu(AluOp::Mov, false, rd, Reg::R0, reg(rm)),
                _ if bit(h, 7) => und,
                _ => Instr::Bx { cond: al, rm },
            }
        }
        0b010 => {
            // Format 4: ALU operations.
            let (rd, rs) = (lo(0), lo(3));
            let by = |ty| Operand2::RegShiftReg { rm: rd, ty, rs };
            use AluOp::*;
            match h >> 6 & 0xF {
                0 => alu(And, true, rd, rd, reg(rs)),
                1 => alu(Eor, true, rd, rd, reg(rs)),
                2 => alu(Mov, true, rd, Reg::R0, by(ShiftType::Lsl)),
                3 => alu(Mov, true, rd, Reg::R0, by(ShiftType::Lsr)),
                4 => alu(Mov, true, rd, Reg::R0, by(ShiftType::Asr)),
                5 => alu(Adc, true, rd, rd, reg(rs)),
                6 => alu(Sbc, true, rd, rd, reg(rs)),
                7 => alu(Mov, true, rd, Reg::R0, by(ShiftType::Ror)),
                8 => alu(Tst, true, Reg::R0, rd, reg(rs)),
                9 => alu(Rsb, true, rd, rs, imm(0)),
                10 => alu(Cmp, true, Reg::R0, rd, reg(rs)),
                11 => alu(Cmn, true, Reg::R0, rd, reg(rs)),
                12 => alu(Orr, true, rd, rd, reg(rs)),
                13 => Instr::Mul {
                    cond: al,
                    s: true,
                    rd,
                    rm: rs,
                    rs: rd,
                    acc: None,
                },
                14 => alu(Bic, true, rd, rd, reg(rs)),
                _ => alu(Mvn, true, rd, Reg::R0, reg(rs)),
            }
        }
        0b011 => {
            // Format 9: immediate offset, word or byte.
            let byte = bit(h, 12);
            let n = h >> 6 & 31;
            let (size, off) = if byte {
                (MemSize::Byte, n)
            } else {
                (MemSize::Word, n << 2)
            };
            mem(bit(h, 11), size, lo(0), lo(3), Offset::Imm(off as i32))
        }
        0b100 if bit(h, 12) => {
            // Format 11: SP-relative.
            mem(
                bit(h, 11),
                MemSize::Word,
                lo(8),
                Reg::Sp,
                Offset::Imm(((h & 0xFF) << 2) as i32),
            )
        }
        0b100 => {
            // Format 10: halfword, immediate offset.
            mem(
                bit(h, 11),
                MemSize::Half,
                lo(0),
                lo(3),
                Offset::Imm(((h >> 6 & 31) << 1) as i32),
            )
        }
        0b101 if !bit(h, 12) => {
            // Format 12: load address.
            let rn = if bit(h, 11) { Reg::Sp } else { Reg::Pc };
            alu(AluOp::Add, false, lo(8), rn, imm((h & 0xFF) << 2))
        }
        0b101 if h >> 8 & 0xF == 0 => {
            // Format 13: add offset to SP.
            let op = if bit(h, 7) { AluOp::Sub } else { AluOp::Add };
            alu(op, false, Reg::Sp, Reg::Sp, imm((h & 0x7F) << 2))
        }
        0b101 if h >> 9 & 3 == 2 => {
            // Format 14: push/pop.
            let load = bit(h, 11);
            let extra = if bit(h, 8) {
                1 << if load { 15 } else { 14 }
            } else {
                0
            };
            Instr::Block {
                cond: al,
                load,
                mode: if load { BlockMode::Ia } else { BlockMode::Db },
                rn: Reg::Sp,
                writeback: true,
                regs: (h & 0xFF) as u16 | extra,
                psr: false,
            }
        }
        0b101 => und,
        0b110 if !bit(h, 12) => Instr::Block {
            // Format 15: multiple load/store.
            cond: al,
            load: bit(h, 11),
            mode: BlockMode::Ia,
            rn: lo(8),
            writeback: true,
            regs: (h & 0xFF) as u16,
            psr: false,
        },
        0b110 => match h >> 8 & 0xF {
            // Format 16 and 17: conditional branch, swi.
            0xF => Instr::Swi {
                cond: al,
                comment: h & 0xFF,
            },
            0xE => und,
            c => Instr::Branch {
                cond: Cond::from_bits(c),
                link: false,
                target: addr
                    .wrapping_add(4)
                    .wrapping_add(((((h & 0xFF) << 24) as i32) >> 23) as u32),
            },
        },
        _ if bit(h, 12) => {
            // Format 19: bl halves.
            if bit(h, 11) {
                Instr::ThumbBlLo {
                    offset: (h & 0x7FF) << 1,
                }
            } else {
                Instr::ThumbBlHi {
                    offset: (((h & 0x7FF) << 21) as i32) >> 9,
                }
            }
        }
        _ if bit(h, 11) => und,
        _ => Instr::Branch {
            // Format 18: unconditional branch.
            cond: al,
            link: false,
            target: addr
                .wrapping_add(4)
                .wrapping_add(((((h & 0x7FF) << 21) as i32) >> 20) as u32),
        },
    }
}
