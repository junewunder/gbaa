//! Decoder tests. `reference` below is a second, independent decoder written
//! straight from the ARM7TDMI TRM / GBATEK bit layouts (no `unarm`); most
//! tests check `decode_*` against it.

use super::*;
use AluOp::*;
use Reg::*;

mod reference;

/// A deterministic spread over all 32-bit words (splitmix32-style), so the
/// sample covers every condition and every class.
fn spread(i: u32) -> u32 {
    let mut z = i.wrapping_mul(0x9E37_79B9);
    z = (z ^ (z >> 16)).wrapping_mul(0x85EB_CA6B);
    z = (z ^ (z >> 13)).wrapping_mul(0xC2B2_AE35);
    z ^ (z >> 16)
}

#[track_caller]
fn check_arm(word: u32, addr: u32) {
    let got = decode_arm(word, addr);
    let want = reference::arm(word, addr);
    assert_eq!(got, want, "ARM {word:08X} at {addr:08X}");
}

#[track_caller]
fn check_thumb(half: u16, addr: u32) {
    let got = decode_thumb(half, addr);
    let want = reference::thumb(half, addr);
    assert_eq!(got, want, "Thumb {half:04X} at {addr:08X}");
}

// ---- Done-when checks -------------------------------------------------------

/// Every Thumb halfword decodes without panicking, matches the reference
/// decoder, and exactly the ARM7TDMI's undefined ranges are `Undefined`.
#[test]
fn thumb_exhaustive() {
    let mut undefined = 0;
    for h in 0..=u16::MAX {
        check_thumb(h, 0x0800_1000);
        if matches!(decode_thumb(h, 0), Instr::Undefined { .. }) {
            undefined += 1;
        }
    }
    // 4780-47FF (blx), B100-B3FF, B600-BBFF, BE00-BFFF (bkpt etc.),
    // DE00-DEFF (b<cond> with cond 1110), E800-EFFF (blx suffix).
    assert_eq!(undefined, 0x80 + 0x300 + 0x600 + 0x200 + 0x100 + 0x800);
}

/// 2^24 ARM words spread over the whole 32-bit space (all conditions): no
/// panic, and every one matches the reference decoder.
#[test]
fn arm_sampled() {
    for i in 0..1u32 << 24 {
        check_arm(spread(i), 0x0800_0000);
    }
}

/// Every ARM word. About a minute in release: `just decode-sweep`.
#[test]
#[ignore = "slow; run with `just decode-sweep`"]
fn arm_exhaustive() {
    let mut w = 0u32;
    loop {
        check_arm(w, 0x0800_0000);
        w = match w.checked_add(1) {
            Some(w) => w,
            None => break,
        };
    }
}

// ---- Aliases fold into the ARM-shaped variants ------------------------------

fn mov(s: bool, rd: Reg, op2: Operand2) -> Instr {
    Instr::Alu {
        cond: Cond::Al,
        op: Mov,
        s,
        rd,
        rn: R0,
        op2,
    }
}

fn shifted(rm: Reg, shift: Shift) -> Operand2 {
    Operand2::Reg { rm, shift }
}

#[test]
fn shift_aliases_fold_into_mov() {
    // lsl r0, r1, #30 / lsr r0, r1, #32 / asr r0, r1, #32 / ror r0, r1, #1 / rrx r0, r1
    assert_eq!(
        decode_arm(0xE1A00F01, 0),
        mov(false, R0, shifted(R1, Shift::Lsl(30)))
    );
    assert_eq!(
        decode_arm(0xE1A00021, 0),
        mov(false, R0, shifted(R1, Shift::Lsr(32)))
    );
    assert_eq!(
        decode_arm(0xE1A00041, 0),
        mov(false, R0, shifted(R1, Shift::Asr(32)))
    );
    assert_eq!(
        decode_arm(0xE1A000E1, 0),
        mov(false, R0, shifted(R1, Shift::Ror(1)))
    );
    assert_eq!(
        decode_arm(0xE1A00061, 0),
        mov(false, R0, shifted(R1, Shift::Rrx))
    );
    // lsl r0, r1, r15
    assert_eq!(
        decode_arm(0xE1A00F11, 0),
        mov(
            false,
            R0,
            Operand2::RegShiftReg {
                rm: R1,
                ty: ShiftType::Lsl,
                rs: Pc
            }
        )
    );
    // Thumb: lsr r0, r0, #32 (encoded #0), asrs r0, r0 (register), rors r0, r1
    assert_eq!(
        decode_thumb(0x0800, 0),
        mov(true, R0, shifted(R0, Shift::Lsr(32)))
    );
    assert_eq!(
        decode_thumb(0x4100, 0),
        mov(
            true,
            R0,
            Operand2::RegShiftReg {
                rm: R0,
                ty: ShiftType::Asr,
                rs: R0
            }
        )
    );
    assert_eq!(
        decode_thumb(0x41C8, 0),
        mov(
            true,
            R0,
            Operand2::RegShiftReg {
                rm: R0,
                ty: ShiftType::Ror,
                rs: R1
            }
        )
    );
    // Thumb lsl r1, r0, #0 is a flag-setting mov.
    assert_eq!(
        decode_thumb(0x0001, 0),
        mov(true, R1, shifted(R0, Shift::Lsl(0)))
    );
}

fn block(load: bool, mode: BlockMode, rn: Reg, writeback: bool, regs: u16, psr: bool) -> Instr {
    Instr::Block {
        cond: Cond::Al,
        load,
        mode,
        rn,
        writeback,
        regs,
        psr,
    }
}

#[test]
fn push_pop_fold_into_block() {
    use BlockMode::*;
    // push {lr} / pop {pc}, ARM and Thumb
    assert_eq!(
        decode_arm(0xE92D4000, 0),
        block(false, Db, Sp, true, 1 << 14, false)
    );
    assert_eq!(
        decode_arm(0xE8BD8000, 0),
        block(true, Ia, Sp, true, 1 << 15, false)
    );
    assert_eq!(
        decode_thumb(0xB500, 0),
        block(false, Db, Sp, true, 1 << 14, false)
    );
    assert_eq!(
        decode_thumb(0xBD00, 0),
        block(true, Ia, Sp, true, 1 << 15, false)
    );
    assert_eq!(
        decode_thumb(0xB4F0, 0),
        block(false, Db, Sp, true, 0xF0, false)
    );
    // ldmia sp!, {pc}^ keeps the S bit
    assert_eq!(
        decode_arm(0xE8FD8000, 0),
        block(true, Ia, Sp, true, 1 << 15, true)
    );
    // Thumb ldmia r1!, {r1}: writeback stays set with the base in the list.
    assert_eq!(
        decode_thumb(0xC902, 0),
        block(true, Ia, R1, true, 1 << 1, false)
    );
}

#[test]
fn thumb_folds_into_arm_variants() {
    let alu = |op, s, rd, rn, op2| Instr::Alu {
        cond: Cond::Al,
        op,
        s,
        rd,
        rn,
        op2,
    };
    let imm = |value| Operand2::Imm { value, rotate: 0 };
    let reg = |rm| shifted(rm, Shift::Lsl(0));
    // movs r0, #1 / adds r0, r1, #0 / add sp, #508 / sub sp, #508
    assert_eq!(decode_thumb(0x2001, 0), alu(Mov, true, R0, R0, imm(1)));
    assert_eq!(decode_thumb(0x1C08, 0), alu(Add, true, R0, R1, imm(0)));
    assert_eq!(decode_thumb(0xB07F, 0), alu(Add, false, Sp, Sp, imm(508)));
    assert_eq!(decode_thumb(0xB0FF, 0), alu(Sub, false, Sp, Sp, imm(508)));
    // add r0, pc, #4 (aligned PC) / add r0, sp, #4
    assert_eq!(decode_thumb(0xA001, 0), alu(Add, false, R0, Pc, imm(4)));
    assert_eq!(decode_thumb(0xA801, 0), alu(Add, false, R0, Sp, imm(4)));
    // hi-register add r0, pc / mov r8, r8 / cmp r0, r0 (no flags-free form)
    assert_eq!(decode_thumb(0x4478, 0), alu(Add, false, R0, R0, reg(Pc)));
    assert_eq!(decode_thumb(0x46C0, 0), alu(Mov, false, R8, R0, reg(R8)));
    assert_eq!(decode_thumb(0x4500, 0), alu(Cmp, true, R0, R0, reg(R0)));
    // neg r0, r0 is rsbs r0, r0, #0
    assert_eq!(decode_thumb(0x4240, 0), alu(Rsb, true, R0, R0, imm(0)));
    // muls r0, r1: r0 = r1 * r0
    assert_eq!(
        decode_thumb(0x4348, 0),
        Instr::Mul {
            cond: Cond::Al,
            s: true,
            rd: R0,
            rm: R1,
            rs: R0,
            acc: None
        }
    );
    // ldr r0, [pc, #4]
    assert_eq!(
        decode_thumb(0x4801, 0),
        Instr::Load {
            cond: Cond::Al,
            size: MemSize::Word,
            rd: R0,
            addr: Addr {
                rn: Pc,
                offset: Offset::Imm(4),
                index: Index::Pre { writeback: false }
            }
        }
    );
    // beq . (at 0x1000) / b . / swi 0
    assert_eq!(
        decode_thumb(0xD0FE, 0x1000),
        Instr::Branch {
            cond: Cond::Eq,
            link: false,
            target: 0x1000
        }
    );
    assert_eq!(
        decode_thumb(0xE7FE, 0x1000),
        Instr::Branch {
            cond: Cond::Al,
            link: false,
            target: 0x1000
        }
    );
    assert_eq!(
        decode_thumb(0xDF00, 0),
        Instr::Swi {
            cond: Cond::Al,
            comment: 0
        }
    );
}

// ---- Everything else is Undefined, never a panic ----------------------------

#[track_caller]
fn assert_undefined_arm(word: u32) {
    assert_eq!(
        decode_arm(word, 0),
        Instr::Undefined {
            cond: Cond::from_bits(word >> 28),
            raw: word
        },
        "{word:08X}"
    );
}

#[track_caller]
fn assert_undefined_thumb(half: u16) {
    assert_eq!(
        decode_thumb(half, 0),
        Instr::Undefined {
            cond: Cond::Al,
            raw: u32::from(half)
        },
        "{half:04X}"
    );
}

#[test]
fn v5_and_coprocessor_encodings_are_undefined() {
    for w in [
        0xE16F0F10, // clz r0, r0
        0xE12FFF30, // blx r0
        0xE1200070, // bkpt
        0xE1000050, // qadd
        0xE1000080, // smlabb
        0xE1C000D0, // ldrd
        0xE1C000F0, // strd
        0xE0400090, // umaal (v6)
        0xE6000010, // media / undefined space
        0xE7F000F0, // permanently undefined
        0xE3000000, // movw (v6T2): tst immediate with S = 0
        0xE3400000, // movt: cmp immediate with S = 0
        0xEE000000, // cdp
        0xEE100010, // mrc
        0xEC000000, // stc
        0xED900000, // ldc
        0x2E000000, // cdp, conditional: keeps its condition
    ] {
        assert_undefined_arm(w);
    }
    for h in [0x4780, 0x47F0, 0xBE00, 0xB100, 0xB600, 0xDE00, 0xE800] {
        assert_undefined_thumb(h);
    }
}

#[test]
fn condition_nv_decodes_like_al() {
    assert_eq!(
        decode_arm(0xF3A00000, 0),
        Instr::Alu {
            cond: Cond::Nv,
            op: Mov,
            s: false,
            rd: R0,
            rn: R0,
            op2: Operand2::Imm {
                value: 0,
                rotate: 0
            }
        }
    );
    // F-prefixed branch is blx in v5; here it is a never-executed b.
    assert_eq!(
        decode_arm(0xFAFFFFFE, 0x1000),
        Instr::Branch {
            cond: Cond::Nv,
            link: false,
            target: 0x1000
        }
    );
}

// ---- Raw fallback for "should be zero" bits ---------------------------------

#[test]
fn mul_ignores_sbz_rn() {
    // mul r0, r1, r0 with bits [15:12] = 0xF
    assert_eq!(
        decode_arm(0xE000F091, 0),
        Instr::Mul {
            cond: Cond::Al,
            s: false,
            rd: R0,
            rm: R1,
            rs: R0,
            acc: None
        }
    );
}

#[test]
fn bx_ignores_sbo_bits() {
    let bx = |rm| Instr::Bx { cond: Cond::Al, rm };
    assert_eq!(decode_arm(0xE12FFF1E, 0), bx(Lr));
    assert_eq!(decode_arm(0xE12F0F1E, 0), bx(Lr)); // bits [19:8] = 0xF0F
    assert_eq!(decode_arm(0xE120001E, 0), bx(Lr)); // bits [19:8] = 0
    // Thumb bx lr with bits [2:0] = 0b111; all 112 such encodings decode.
    assert_eq!(decode_thumb(0x4777, 0), bx(Lr));
    let mut n = 0;
    for h in 0x4700..=0x477Fu16 {
        if h & 7 != 0 {
            assert_eq!(decode_thumb(h, 0), bx(Reg::from_bits(u32::from(h >> 3))));
            n += 1;
        }
    }
    assert_eq!(n, 112);
}

#[test]
fn compares_keep_raw_rd() {
    let teq = |rd| Instr::Alu {
        cond: Cond::Al,
        op: Teq,
        s: true,
        rd,
        rn: R0,
        op2: shifted(R0, Shift::Lsl(0)),
    };
    assert_eq!(decode_arm(0xE1300000, 0), teq(R0));
    assert_eq!(decode_arm(0xE130F000, 0), teq(Pc)); // teqp-style rd = 15
    // tst/cmp/cmn, register and immediate, with rd != 0
    for (w, op) in [(0xE1115000, Tst), (0xE1515000, Cmp), (0xE1715000, Cmn)] {
        assert!(
            matches!(decode_arm(w, 0), Instr::Alu { op: o, rd: R5, rn: R1, s: true, .. } if o == op),
            "{w:08X}"
        );
    }
    assert!(matches!(
        decode_arm(0xE3315001, 0),
        Instr::Alu {
            op: Teq,
            rd: R5,
            rn: R1,
            ..
        }
    ));
}

#[test]
fn mov_mvn_keep_raw_rn() {
    // mov r0, #0 with rn = r1; mvns r0, r2 with rn = r3
    assert!(matches!(
        decode_arm(0xE3A10000, 0),
        Instr::Alu {
            op: Mov,
            rd: R0,
            rn: R1,
            ..
        }
    ));
    assert!(matches!(
        decode_arm(0xE1F30002, 0),
        Instr::Alu {
            op: Mvn,
            s: true,
            rd: R0,
            rn: R3,
            ..
        }
    ));
}

#[test]
fn misc_sbz_bits_ignored() {
    // ldrh r0, [r0, -r0]! style with bits [11:8] = 0xF (register offset)
    assert!(matches!(
        decode_arm(0xE1B00FB1, 0),
        Instr::Load {
            size: MemSize::Half,
            ..
        }
    ));
    // post-indexed strh with W set: W ignored, still post-indexed
    assert!(matches!(
        decode_arm(0xE0A000B1, 0),
        Instr::Store {
            size: MemSize::Half,
            addr: Addr {
                index: Index::Post { user: false },
                ..
            },
            ..
        }
    ));
    // swp r0, r1, [r2] with bits [11:8] = 0xF
    assert_eq!(
        decode_arm(0xE1020F91, 0),
        Instr::Swap {
            cond: Cond::Al,
            byte: false,
            rd: R0,
            rm: R1,
            rn: R2
        }
    );
    // mrs r0, cpsr with junk in [19:16] and [11:0]
    assert_eq!(
        decode_arm(0xE1050F0F, 0),
        Instr::Mrs {
            cond: Cond::Al,
            rd: R0,
            spsr: false
        }
    );
    // msr cpsr_fc, r0 with bits [15:12] = 0 and [11:8] = 0xF
    assert!(matches!(
        decode_arm(0xE1290F00, 0),
        Instr::Msr {
            src: MsrSrc::Reg(R0),
            fields: PsrFields {
                c: true,
                x: false,
                s: false,
                f: true
            },
            ..
        }
    ));
}

#[test]
fn empty_register_lists() {
    use BlockMode::*;
    assert_eq!(
        decode_arm(0xE8100000, 0),
        block(true, Da, R0, false, 0, false)
    );
    assert_eq!(
        decode_arm(0xE8AD0000, 0),
        block(false, Ia, Sp, true, 0, false)
    );
    assert_eq!(
        decode_thumb(0xC000, 0),
        block(false, Ia, R0, true, 0, false)
    );
    assert_eq!(decode_thumb(0xC800, 0), block(true, Ia, R0, true, 0, false));
    assert_eq!(
        decode_thumb(0xB400, 0),
        block(false, Db, Sp, true, 0, false)
    );
    assert_eq!(decode_thumb(0xBC00, 0), block(true, Ia, Sp, true, 0, false));
}

// ---- msr immediate fix and csdb ---------------------------------------------

#[test]
fn msr_immediate_rotates_by_twice_rot() {
    // msr cpsr_f, #0xF0000000: imm8 = 0x0F, rot = 2 (ror #4)
    assert_eq!(
        decode_arm(0xE328F20F, 0),
        Instr::Msr {
            cond: Cond::Al,
            spsr: false,
            fields: PsrFields {
                c: false,
                x: false,
                s: false,
                f: true
            },
            src: MsrSrc::Imm(0xF000_0000)
        }
    );
    // msr spsr_fsxc, #0x1F with rot = 0, and conditional
    assert!(matches!(
        decode_arm(0x136FF01F, 0),
        Instr::Msr {
            cond: Cond::Ne,
            spsr: true,
            src: MsrSrc::Imm(0x1F),
            ..
        }
    ));
}

#[test]
fn csdb_is_an_msr_that_writes_nothing() {
    let got = decode_arm(0xE320F014, 0);
    assert_eq!(
        got,
        Instr::Msr {
            cond: Cond::Al,
            spsr: false,
            fields: PsrFields {
                c: false,
                x: false,
                s: false,
                f: false
            },
            src: MsrSrc::Imm(0x14)
        }
    );
    // Same for the other v6K hints (nop, yield, wfe, wfi, sev).
    for w in [0xE320F000, 0xE320F001, 0xE320F002, 0xE320F003, 0xE320F004] {
        assert!(matches!(decode_arm(w, 0), Instr::Msr { fields, .. } if fields.mask() == 0));
    }
}

// ---- Thumb bl halves --------------------------------------------------------

#[test]
fn thumb_bl_halves_decode_separately() {
    assert_eq!(decode_thumb(0xF000, 0), Instr::ThumbBlHi { offset: 0 });
    assert_eq!(
        decode_thumb(0xF7FF, 0),
        Instr::ThumbBlHi { offset: -0x1000 }
    );
    assert_eq!(
        decode_thumb(0xF400, 0),
        Instr::ThumbBlHi { offset: -0x40_0000 }
    );
    assert_eq!(decode_thumb(0xF800, 0), Instr::ThumbBlLo { offset: 0 });
    assert_eq!(decode_thumb(0xFFFF, 0), Instr::ThumbBlLo { offset: 0xFFE });
}

#[test]
fn pair_thumb_bl_joins_halves() {
    // bl . (F7FF FFFE) at 0x08000100
    assert_eq!(
        pair_thumb_bl(0xF7FF, 0xFFFE, 0x0800_0100),
        Some(0x0800_0100)
    );
    // bl +0x1000: hi offset 1 << 12, lo 0 → 0x1000 + 4 + 0x1000
    assert_eq!(pair_thumb_bl(0xF001, 0xF800, 0x1000), Some(0x2004));
    // Maximum backward and forward reach (±4 MiB).
    assert_eq!(
        pair_thumb_bl(0xF400, 0xF800, 0x0840_0000),
        Some(0x0800_0004)
    );
    assert_eq!(
        pair_thumb_bl(0xF3FF, 0xFFFF, 0x0800_0000),
        Some(0x0840_0002)
    );
    // Not a pair: halves swapped, or a non-bl instruction.
    assert_eq!(pair_thumb_bl(0xF800, 0xF000, 0), None);
    assert_eq!(pair_thumb_bl(0xF000, 0x46C0, 0), None);
}

// ---- Spot check against the TRM / GBATEK ------------------------------------

/// Hand-picked encodings: one or more templates per instruction class, with
/// register and immediate fields at their edge values. Each must match the
/// reference decoder field for field.
fn hand_picked_arm() -> Vec<u32> {
    // (template, fields to vary as (shift, values))
    let regs = [0u32, 1, 7, 13, 14, 15];
    let mut out = Vec::new();
    let vary = |out: &mut Vec<u32>, base: u32, shifts: &[u32]| {
        for &s in shifts {
            for &r in &regs {
                out.push(base & !(0xF << s) | r << s);
            }
        }
    };
    // Data processing, every opcode, S on and off, three operand forms.
    for op in 0..16u32 {
        for s in [0, 1] {
            if op & 0xC == 0x8 && s == 0 {
                continue; // misc space
            }
            out.push(0xE0000000 | op << 21 | s << 20 | 0x1 << 16 | 0x2 << 12 | 0x3); // reg
            out.push(0xE0000000 | op << 21 | s << 20 | 0x1 << 16 | 0x2 << 12 | 0x5 << 8 | 0x13); // lsl reg
            out.push(0xE2000000 | op << 21 | s << 20 | 0x1 << 16 | 0x2 << 12 | 0xF << 8 | 0xFF); // imm
        }
    }
    // Immediate shifts at every edge: types × amounts 0, 1, 31.
    for ty in 0..4u32 {
        for amt in [0u32, 1, 31] {
            out.push(0xE1A00001 | amt << 7 | ty << 5);
        }
    }
    vary(&mut out, 0xE0901293, &[16, 12, 8, 0]); // adds r1, r0, r3, lsl r2
    vary(&mut out, 0xE0000291, &[16, 8, 0]); // mul
    vary(&mut out, 0xE0301392, &[16, 12, 8, 0]); // mlas
    vary(&mut out, 0xE0821394, &[16, 12, 8, 0]); // umull
    out.extend([0xE0A21394, 0xE0C21394, 0xE0F21394]); // umlal, smull, smlals
    vary(&mut out, 0xE5912004, &[16, 12]); // ldr r2, [r1, #4]
    out.extend([
        0xE5112FFF, // ldr r2, [r1, #-0xFFF]
        0xE5B12004, // ldr r2, [r1, #4]!
        0xE4912004, // ldr r2, [r1], #4
        0xE4B12004, // ldrt r2, [r1], #4
        0xE5D12001, // ldrb
        0xE4E12001, // strbt
        0xE7912103, // ldr r2, [r1, r3, lsl #2]
        0xE7112123, // ldr r2, [r1, -r3, lsr #2]
        0xE7912063, // ldr r2, [r1, r3, rrx]
        0xE6912143, // ldr r2, [r1], r3, asr #2
        0xE59FF000, // ldr pc, [pc]
    ]);
    vary(&mut out, 0xE1D120B4, &[16, 12]); // ldrh r2, [r1, #4]
    out.extend([
        0xE15120BF, // ldrh r2, [r1, #-0xF]
        0xE1F120D4, // ldrsb r2, [r1, #4]!
        0xE0D120F4, // ldrsh r2, [r1], #4
        0xE19120B3, // ldrh r2, [r1, r3]
        0xE01120B3, // ldrh r2, [r1], -r3
        0xE1C120B4, // strh r2, [r1, #4]
    ]);
    vary(&mut out, 0xE8915555, &[16]); // ldmia r1, {...}
    for pu in 0..4u32 {
        for (s, w, l) in [(0, 0, 0), (0, 1, 1), (1, 0, 1), (1, 1, 0)] {
            out.push(0xE8000000 | pu << 23 | s << 22 | w << 21 | l << 20 | 2 << 16 | 0x80F0);
        }
    }
    out.extend([0xE1012093, 0xE1412093]); // swp, swpb
    out.extend([0xE10F2000, 0xE14F2000]); // mrs cpsr, spsr
    for mask in 1..16u32 {
        out.push(0xE120F001 | mask << 16); // msr reg, every field mask
        out.push(0xE360F4FF | mask << 16); // msr spsr imm
    }
    out.extend([0xE12FFF11, 0x012FFF1E]); // bx r1, bxeq lr
    out.extend([0xEA000000, 0xEAFFFFFE, 0xEB7FFFFF, 0xEB800000, 0x0A000010]); // b, bl
    out.extend([0xEF000000, 0xEFFFFFFF, 0x1F123456]); // swi
    // Every condition on one instruction.
    for c in 0..16u32 {
        out.push(c << 28 | 0x01A00001);
    }
    out
}

fn hand_picked_thumb() -> Vec<u16> {
    let mut out = vec![
        0x0000, 0x07FF, 0x0848, 0x1048, 0x17FF, // lsl, lsr, asr
        0x1888, 0x1A88, 0x1C88, 0x1FFF, // add/sub reg, imm3
        0x2000, 0x27FF, 0x2900, 0x3001, 0x3FFF, // mov/cmp/add/sub imm8
        0x4448, 0x4487, 0x45F7, 0x4640, 0x46F7, // hi-register ops
        0x4700, 0x4778, 0x47F8, // bx r0, bx pc, (blx pc)
        0x4800, 0x4FFF, // ldr pc-relative
        0x5088, 0x5488, 0x5888, 0x5C88, // str/strb/ldr/ldrb reg
        0x5288, 0x5688, 0x5A88, 0x5E88, // strh/ldsb/ldrh/ldsh reg
        0x6000, 0x67FF, 0x6800, 0x6FFF, 0x7000, 0x7FFF, // word/byte imm5
        0x8000, 0x87FF, 0x8800, 0x8FFF, // halfword imm5
        0x9000, 0x97FF, 0x9800, 0x9FFF, // sp-relative
        0xA000, 0xA7FF, 0xA800, 0xAFFF, // add rd, pc/sp
        0xB000, 0xB07F, 0xB080, 0xB0FF, // add/sub sp
        0xB401, 0xB5FF, 0xBC80, 0xBDFF, // push/pop
        0xC0FF, 0xC701, 0xC8FF, 0xCF80, // stmia/ldmia
        0xD000, 0xD07F, 0xD080, 0xDD7F, // b<cond>
        0xDF00, 0xDFFF, // swi
        0xE000, 0xE3FF, 0xE400, 0xE7FF, // b
        0xF000, 0xF7FF, 0xF800, 0xFFFF, // bl halves
    ];
    // Every format-4 ALU op on r1, r6.
    out.extend((0..16u16).map(|op| 0x4000 | op << 6 | 6 << 3 | 1));
    out
}

#[test]
fn spot_check_against_trm() {
    let arm = hand_picked_arm();
    let thumb = hand_picked_thumb();
    assert!(
        arm.len() + thumb.len() >= 200,
        "{} + {}",
        arm.len(),
        thumb.len()
    );
    for w in arm {
        check_arm(w, 0x0800_1000);
    }
    for h in thumb {
        check_thumb(h, 0x0800_1000);
    }
}

/// A few encodings written out in full from GBATEK, independent of both
/// decoders.
#[test]
fn literal_gbatek_examples() {
    // E0912003: adds r2, r1, r3
    assert_eq!(
        decode_arm(0xE0912003, 0),
        Instr::Alu {
            cond: Cond::Al,
            op: Add,
            s: true,
            rd: R2,
            rn: R1,
            op2: shifted(R3, Shift::Lsl(0))
        }
    );
    // E3A00102: mov r0, #0x80000000 (imm 2, ror #2)
    assert_eq!(
        decode_arm(0xE3A00102, 0),
        mov(
            false,
            R0,
            Operand2::Imm {
                value: 0x8000_0000,
                rotate: 2
            }
        )
    );
    // E0202391: mla r0, r1, r3, r2
    assert_eq!(
        decode_arm(0xE0202391, 0),
        Instr::Mul {
            cond: Cond::Al,
            s: false,
            rd: R0,
            rm: R1,
            rs: R3,
            acc: Some(R2)
        }
    );
    // E0D10392: smulls r0, r1, r2, r3
    assert_eq!(
        decode_arm(0xE0D10392, 0),
        Instr::MulLong {
            cond: Cond::Al,
            s: true,
            signed: true,
            acc: false,
            rd_lo: R0,
            rd_hi: R1,
            rm: R2,
            rs: R3
        }
    );
    // E7112103: ldr r2, [r1, -r3, lsl #2]
    assert_eq!(
        decode_arm(0xE7112103, 0),
        Instr::Load {
            cond: Cond::Al,
            size: MemSize::Word,
            rd: R2,
            addr: Addr {
                rn: R1,
                offset: Offset::Reg {
                    rm: R3,
                    shift: Shift::Lsl(2),
                    subtract: true
                },
                index: Index::Pre { writeback: false }
            }
        }
    );
    // E4E12001: strbt r2, [r1], #1
    assert_eq!(
        decode_arm(0xE4E12001, 0),
        Instr::Store {
            cond: Cond::Al,
            size: MemSize::Byte,
            rd: R2,
            addr: Addr {
                rn: R1,
                offset: Offset::Imm(1),
                index: Index::Post { user: true }
            }
        }
    );
    // E1F120D4: ldrsb r2, [r1, #4]!
    assert_eq!(
        decode_arm(0xE1F120D4, 0),
        Instr::Load {
            cond: Cond::Al,
            size: MemSize::SignedByte,
            rd: R2,
            addr: Addr {
                rn: R1,
                offset: Offset::Imm(4),
                index: Index::Pre { writeback: true }
            }
        }
    );
    // E9AD5FFF: stmib sp!, {r0-r12, lr}
    assert_eq!(
        decode_arm(0xE9AD5FFF, 0),
        block(false, BlockMode::Ib, Sp, true, 0x5FFF, false)
    );
    // E14F0000: mrs r0, spsr
    assert_eq!(
        decode_arm(0xE14F0000, 0),
        Instr::Mrs {
            cond: Cond::Al,
            rd: R0,
            spsr: true
        }
    );
    // EB000000 at 0x08000000: bl 0x08000008
    assert_eq!(
        decode_arm(0xEB000000, 0x0800_0000),
        Instr::Branch {
            cond: Cond::Al,
            link: true,
            target: 0x0800_0008
        }
    );
    // EAFFFFFE at 0: b . (offset -8 from the pipelined PC)
    assert_eq!(
        decode_arm(0xEAFFFFFE, 0),
        Instr::Branch {
            cond: Cond::Al,
            link: false,
            target: 0
        }
    );
    // EF0000FF: swi 0xFF
    assert_eq!(
        decode_arm(0xEF0000FF, 0),
        Instr::Swi {
            cond: Cond::Al,
            comment: 0xFF
        }
    );
    // Thumb 5A88: ldrh r0, [r1, r2]
    assert_eq!(
        decode_thumb(0x5A88, 0),
        Instr::Load {
            cond: Cond::Al,
            size: MemSize::Half,
            rd: R0,
            addr: Addr {
                rn: R1,
                offset: Offset::Reg {
                    rm: R2,
                    shift: Shift::Lsl(0),
                    subtract: false
                },
                index: Index::Pre { writeback: false }
            }
        }
    );
    // Thumb 8888: ldrh r0, [r1, #4]
    assert_eq!(
        decode_thumb(0x8888, 0),
        Instr::Load {
            cond: Cond::Al,
            size: MemSize::Half,
            rd: R0,
            addr: Addr {
                rn: R1,
                offset: Offset::Imm(4),
                index: Index::Pre { writeback: false }
            }
        }
    );
}

// ---- SingleStepTests opcodes ------------------------------------------------

/// The opcodes of one SingleStepTests file (`<name>.json.bin`).
fn sst_opcodes(path: &std::path::Path) -> Vec<u32> {
    let b = std::fs::read(path).unwrap();
    let u32_at = |p: usize| u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
    assert_eq!(u32_at(0), 0xD33D_BAE0, "{}", path.display());
    let mut p = 8;
    (0..u32_at(4))
        .map(|_| {
            let full = u32_at(p) as usize;
            // Initial state, final state and transactions, then the opcode
            // record (size, magic, opcode, base address).
            let mut q = p + 4;
            for _ in 0..3 {
                q += u32_at(q) as usize;
            }
            p += full;
            u32_at(q + 8)
        })
        .collect()
}

/// Every opcode in the SingleStepTests suite (`just fetch-tests`) decodes to
/// the instruction class its file is named for.
#[test]
#[ignore = "needs $GBAA_DATA; run with `just decode-sweep`"]
fn sst_opcodes_match_their_class() {
    let data = std::env::var("GBAA_DATA")
        .unwrap_or_else(|_| format!("{}/.local/share/gbaa", std::env::var("HOME").unwrap()));
    let dir = std::path::Path::new(&data).join("SingleStepTests-ARM7TDMI/v1");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".json.bin"))
        .collect();
    files.sort();
    assert!(files.len() >= 45, "only {} test files", files.len());
    for path in files {
        let name = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(".json.bin", "");
        for op in sst_opcodes(&path) {
            let ok = if let Some(class) = name.strip_prefix("arm_") {
                let i = decode_arm(op, 0);
                match class {
                    "b_bl" => matches!(i, Instr::Branch { .. }),
                    "bx" => matches!(i, Instr::Bx { .. }),
                    "cdp" | "mcr_mrc" | "stc_ldc" => matches!(i, Instr::Undefined { .. }),
                    c if c.starts_with("data_proc") => matches!(i, Instr::Alu { .. }),
                    "ldm_stm" => matches!(i, Instr::Block { .. }),
                    c if c.starts_with("ldr_str") => matches!(
                        i,
                        Instr::Load {
                            size: MemSize::Word | MemSize::Byte,
                            ..
                        } | Instr::Store {
                            size: MemSize::Word | MemSize::Byte,
                            ..
                        }
                    ),
                    "ldrh_strh" => matches!(
                        i,
                        Instr::Load {
                            size: MemSize::Half,
                            ..
                        } | Instr::Store {
                            size: MemSize::Half,
                            ..
                        }
                    ),
                    "ldrsb_ldrsh" => matches!(
                        i,
                        Instr::Load {
                            size: MemSize::SignedByte | MemSize::SignedHalf,
                            ..
                        }
                    ),
                    "mrs" => matches!(i, Instr::Mrs { .. }),
                    "msr_imm" => matches!(
                        i,
                        Instr::Msr {
                            src: MsrSrc::Imm(_),
                            ..
                        }
                    ),
                    "msr_reg" => matches!(
                        i,
                        Instr::Msr {
                            src: MsrSrc::Reg(_),
                            ..
                        }
                    ),
                    "mul_mla" => matches!(i, Instr::Mul { .. }),
                    "mull_mlal" => matches!(i, Instr::MulLong { .. }),
                    "swi" => matches!(i, Instr::Swi { .. }),
                    "swp" => matches!(i, Instr::Swap { .. }),
                    c => panic!("unknown file arm_{c}"),
                }
            } else {
                let h = op as u16;
                let i = decode_thumb(h, 0);
                match name.strip_prefix("thumb_").unwrap() {
                    "b" | "bcc" => matches!(i, Instr::Branch { .. }),
                    "bl_blx_prefix" => matches!(i, Instr::ThumbBlHi { .. }),
                    "bl_suffix" => matches!(i, Instr::ThumbBlLo { .. }),
                    // The suite puts v5's blx (H1 = 1) in here too; the
                    // ARM7TDMI doesn't execute it (mGBA agrees).
                    "bx" if h & 0x80 != 0 => matches!(i, Instr::Undefined { .. }),
                    "bx" => matches!(i, Instr::Bx { .. }),
                    "data_proc" => matches!(i, Instr::Alu { .. } | Instr::Mul { .. }),
                    "ldm_stm" | "push_pop" => matches!(i, Instr::Block { .. }),
                    c if c.starts_with("ldr") => {
                        matches!(i, Instr::Load { .. } | Instr::Store { .. })
                    }
                    "swi" => matches!(i, Instr::Swi { .. }),
                    "undefined_bcc" => matches!(i, Instr::Undefined { .. }),
                    _ => matches!(i, Instr::Alu { .. }),
                }
            };
            assert!(
                ok,
                "{name}: {op:08X} decoded as {:?}",
                if name.starts_with("arm") {
                    decode_arm(op, 0)
                } else {
                    decode_thumb(op as u16, 0)
                }
            );
        }
    }
}

// ---- Other unarm quirks -----------------------------------------------------

#[test]
fn halfword_register_writeback_with_r0_base() {
    // ldrsh r4, [r0, r5]! and strh r0, [r0, r1]!: unarm says Illegal.
    let pre_wb = |rm| Addr {
        rn: R0,
        offset: Offset::Reg {
            rm,
            shift: Shift::Lsl(0),
            subtract: false,
        },
        index: Index::Pre { writeback: true },
    };
    assert_eq!(
        decode_arm(0xE1B040F5, 0),
        Instr::Load {
            cond: Cond::Al,
            size: MemSize::SignedHalf,
            rd: R4,
            addr: pre_wb(R5)
        }
    );
    assert_eq!(
        decode_arm(0xE1A000B1, 0),
        Instr::Store {
            cond: Cond::Al,
            size: MemSize::Half,
            rd: R0,
            addr: pre_wb(R1)
        }
    );
}

#[test]
fn single_register_sp_transfers_stay_load_store() {
    // str r11, [sp, #-4]! and ldr r11, [sp], #4: unarm calls them push/pop.
    assert_eq!(
        decode_arm(0xE52DB004, 0),
        Instr::Store {
            cond: Cond::Al,
            size: MemSize::Word,
            rd: R11,
            addr: Addr {
                rn: Sp,
                offset: Offset::Imm(-4),
                index: Index::Pre { writeback: true }
            }
        }
    );
    assert_eq!(
        decode_arm(0xE49DB004, 0),
        Instr::Load {
            cond: Cond::Al,
            size: MemSize::Word,
            rd: R11,
            addr: Addr {
                rn: Sp,
                offset: Offset::Imm(4),
                index: Index::Post { user: false }
            }
        }
    );
}

/// `Shift::encoded` recovers bits [6:5] and [11:7] of every ARM
/// immediate-shift data-processing word.
#[test]
fn shift_encoded_round_trips() {
    for raw in 0..0x80u32 {
        let (ty, amt) = (raw & 3, raw >> 2);
        let w = 0xE1A00001 | amt << 7 | ty << 5;
        let Instr::Alu {
            op2: Operand2::Reg { shift, .. },
            ..
        } = decode_arm(w, 0)
        else {
            panic!("{w:08X}");
        };
        assert_eq!(shift.encoded(), (shift_type_of(ty), amt as u8), "{w:08X}");
    }
}

fn shift_type_of(ty: u32) -> ShiftType {
    [
        ShiftType::Lsl,
        ShiftType::Lsr,
        ShiftType::Asr,
        ShiftType::Ror,
    ][ty as usize]
}

#[test]
fn cond_accessor() {
    assert_eq!(decode_arm(0x1A000000, 0).cond(), Cond::Ne);
    assert_eq!(decode_arm(0xFE000000, 0).cond(), Cond::Nv);
    assert_eq!(decode_thumb(0xD100, 0).cond(), Cond::Ne);
    assert_eq!(decode_thumb(0xF000, 0).cond(), Cond::Al);
}
