//! Interpreter tests on hand-picked instructions. SingleStepTests
//! (`tests/sst.rs`) is the broad check; these pin the rules it taught us
//! and run without the test data.

use super::{Bus, Cpu, I_BIT, T_BIT, mode};
use crate::sem::prim::{self, MulKind};
use crate::sem::{ShiftType, Width};

/// 4 KiB of RAM, mirrored over the whole address space.
struct Ram(Vec<u8>);

impl Ram {
    fn new() -> Ram {
        Ram(vec![0; 0x1000])
    }
    fn at(&self, addr: u32) -> usize {
        addr as usize % self.0.len()
    }
    fn word(&self, addr: u32) -> u32 {
        let a = self.at(addr);
        u32::from_le_bytes(self.0[a..a + 4].try_into().unwrap())
    }
    fn set_word(&mut self, addr: u32, v: u32) {
        let a = self.at(addr);
        self.0[a..a + 4].copy_from_slice(&v.to_le_bytes());
    }
}

impl Bus for Ram {
    fn load(&mut self, w: Width, addr: u32) -> u32 {
        let a = self.at(addr);
        (0..w.bytes() as usize).fold(0, |v, i| v | (self.0[a + i] as u32) << (8 * i))
    }
    fn store(&mut self, w: Width, addr: u32, v: u32) {
        let a = self.at(addr);
        for i in 0..w.bytes() as usize {
            self.0[a + i] = (v >> (8 * i)) as u8;
        }
    }
}

const AT: u32 = 0x100;

/// A CPU in system mode at `AT`, ARM state.
fn cpu() -> Cpu {
    Cpu {
        pc: AT,
        cpsr: mode::SYS,
        ..Cpu::default()
    }
}

fn thumb() -> Cpu {
    Cpu {
        cpsr: mode::SYS | T_BIT,
        ..cpu()
    }
}

fn flags(c: &Cpu) -> [bool; 4] {
    [31, 30, 29, 28].map(|b| c.cpsr & (1 << b) != 0)
}

fn run(c: &mut Cpu, raw: u32) -> Ram {
    let mut ram = Ram::new();
    c.execute(&mut ram, raw);
    ram
}

#[test]
fn prim_shift_edge_amounts() {
    use ShiftType::*;
    assert_eq!(prim::shift(Lsl, 0x8000_0001, 0, true), (0x8000_0001, true));
    assert_eq!(prim::shift(Lsl, 0x8000_0001, 1, false), (2, true));
    assert_eq!(prim::shift(Lsl, 1, 32, false), (0, true));
    assert_eq!(prim::shift(Lsl, 1, 33, true), (0, false));
    assert_eq!(prim::shift(Lsr, 0x8000_0000, 32, false), (0, true));
    assert_eq!(prim::shift(Asr, 0x8000_0000, 40, false), (u32::MAX, true));
    assert_eq!(
        prim::shift(Ror, 0x8000_0000, 32, false),
        (0x8000_0000, true)
    );
    assert_eq!(prim::shift(Ror, 3, 0x101, false), (0x8000_0001, true));
    assert_eq!(prim::rrx(3, true), (0x8000_0001, true));
}

#[test]
fn adds_takes_carry_from_the_addition() {
    // adds r0, r1, r2, lsl r3
    let mut c = cpu();
    c.r[1] = 0xFFFF_FFFF;
    c.r[2] = 1;
    c.r[3] = 1;
    run(&mut c, 0xE091_0312);
    assert_eq!(c.r[0], 1);
    assert_eq!(flags(&c), [false, false, true, false]);
    assert_eq!(c.pc, AT + 4);
}

#[test]
fn movs_takes_carry_from_the_shifter() {
    // movs r0, r1, lsr #32
    let mut c = cpu();
    c.r[1] = 0x8000_0000;
    run(&mut c, 0xE1B0_0021);
    assert_eq!(c.r[0], 0);
    assert_eq!(flags(&c), [false, true, true, false]);
}

#[test]
fn shift_by_register_reads_pc_twelve_ahead() {
    // add r0, pc, r1, lsl r2
    let mut c = cpu();
    c.r[1] = 1;
    c.r[2] = 2;
    run(&mut c, 0xE08F_0211);
    assert_eq!(c.r[0], AT + 12 + 4);
}

#[test]
fn condition_failing_skips_the_instruction() {
    // moveq r0, #1
    let mut c = cpu();
    run(&mut c, 0x03A0_0001);
    assert_eq!((c.r[0], c.pc), (0, AT + 4));
    c.pc = AT;
    c.cpsr |= 1 << 30;
    run(&mut c, 0x03A0_0001);
    assert_eq!(c.r[0], 1);
}

/// `cond_passes` agrees with the semantics on every condition and flag set.
#[test]
fn cond_passes_matches_the_semantics() {
    for cond in 0..16u32 {
        for nzcv in 0..16u32 {
            // mov<cond> r0, #1
            let mut c = cpu();
            c.cpsr |= nzcv << 28;
            let expect = c.cond_passes(crate::decode::Cond::from_bits(cond));
            run(&mut c, cond << 28 | 0x03A0_0001);
            assert_eq!(c.r[0] == 1, expect, "cond {cond:x}, nzcv {nzcv:04b}");
        }
    }
}

#[test]
fn compare_with_rd_pc_copies_spsr() {
    // cmp pc, r0 with rd = 15 (teqp-style), in IRQ mode.
    let mut c = cpu();
    c.switch_mode(mode::IRQ);
    c.spsr[super::Bank::Irq as usize] = 0x2000_0000 | mode::SVC;
    run(&mut c, 0xE15F_F000);
    assert_eq!(c.cpsr, 0x2000_0000 | mode::SVC);
    assert_eq!(c.pc, AT + 4);
}

#[test]
fn subs_pc_lr_returns_from_an_exception() {
    let mut c = cpu();
    c.switch_mode(mode::SVC);
    c.r[14] = 0x204;
    c.spsr[super::Bank::Svc as usize] = mode::USR | T_BIT;
    run(&mut c, 0xE25E_F004);
    assert_eq!(c.cpsr, mode::USR | T_BIT);
    assert_eq!(c.pc, 0x200);
}

#[test]
fn movs_pc_in_system_mode_sets_flags_and_jumps() {
    // movs pc, r0: no SPSR, so the flags come from the result.
    let mut c = cpu();
    c.r[0] = 0x8000_0000;
    run(&mut c, 0xE1B0_F000);
    assert_eq!(c.cpsr, 0x8000_0000 | mode::SYS);
    assert_eq!(c.pc, 0x8000_0000);
}

#[test]
fn ldr_misaligned_rotates_and_writes_back() {
    // ldr r0, [r1, #2]!
    let mut c = cpu();
    c.r[1] = 0x200;
    let mut ram = Ram::new();
    ram.set_word(0x200, 0x1122_3344);
    c.execute(&mut ram, 0xE5B1_0002);
    assert_eq!(c.r[0], 0x3344_1122);
    assert_eq!(c.r[1], 0x202);
}

#[test]
fn ldr_into_its_own_base_keeps_the_loaded_value() {
    // ldr r1, [r1], #4
    let mut c = cpu();
    c.r[1] = 0x200;
    let mut ram = Ram::new();
    ram.set_word(0x200, 0xABCD);
    c.execute(&mut ram, 0xE491_1004);
    assert_eq!(c.r[1], 0xABCD);
}

#[test]
fn ldr_pc_relative_then_ldr_pc_jumps_without_interworking() {
    let mut c = cpu();
    let mut ram = Ram::new();
    ram.set_word(AT + 12, 0x301);
    // ldr r0, [pc, #4]
    c.execute(&mut ram, 0xE59F_0004);
    assert_eq!(c.r[0], 0x301);
    // ldr pc, [r1] with [r1] = 0x301: stays ARM, keeps the low bits.
    c.r[1] = AT + 12;
    c.execute(&mut ram, 0xE591_F000);
    assert_eq!(c.pc, 0x301);
    assert!(!c.thumb());
}

#[test]
fn strb_post_indexed_down() {
    // strb r0, [r1], #-1
    let mut c = cpu();
    c.r[0] = 0x1234_56AB;
    c.r[1] = 0x203;
    let ram = run(&mut c, 0xE441_0001);
    assert_eq!(ram.word(0x200), 0xAB00_0000);
    assert_eq!(c.r[1], 0x202);
}

#[test]
fn str_pc_stores_twelve_ahead() {
    // str pc, [r0]
    let mut c = cpu();
    c.r[0] = 0x200;
    let ram = run(&mut c, 0xE580_F000);
    assert_eq!(ram.word(0x200), AT + 12);
}

#[test]
fn bx_switches_to_thumb_and_bl_links() {
    let mut c = cpu();
    c.r[14] = 0x301;
    run(&mut c, 0xE12F_FF1E);
    assert!(c.thumb());
    assert_eq!(c.pc, 0x300);

    let mut c = cpu();
    run(&mut c, 0xEB00_0000);
    assert_eq!((c.pc, c.r[14]), (AT + 8, AT + 4));
}

#[test]
fn thumb_bl_pair() {
    // bl to AT + 4 + 0x1000: hi then lo.
    let mut c = thumb();
    let mut ram = Ram::new();
    c.execute(&mut ram, 0xF001);
    c.execute(&mut ram, 0xF800);
    assert_eq!(c.pc, AT + 4 + 0x1000);
    assert_eq!(c.r[14], (AT + 4) | 1);
}

#[test]
fn swi_and_undefined_enter_their_modes() {
    let mut c = cpu();
    c.cpsr |= 0xF000_0000;
    run(&mut c, 0xEF05_0000);
    assert_eq!(c.cpsr & 0x1F, mode::SVC);
    assert_eq!(c.cpsr & (I_BIT | T_BIT), I_BIT);
    assert_eq!((c.pc, c.r[14]), (0x08, AT + 4));
    assert_eq!(c.spsr[super::Bank::Svc as usize], 0xF000_0000 | mode::SYS);

    // Thumb `b<cond>` with cond 1110 is undefined on ARMv4T.
    let mut c = thumb();
    run(&mut c, 0xDEAF);
    assert_eq!(c.cpsr & 0x1F, mode::UND);
    assert_eq!((c.pc, c.r[14]), (0x04, AT + 2));
}

#[test]
fn thumb_alu_forms() {
    // lsl r0, r1
    let mut c = thumb();
    c.r[0] = 0x1000_0001;
    c.r[1] = 4;
    run(&mut c, 0x4088);
    assert_eq!(c.r[0], 0x10);
    assert_eq!(flags(&c), [false, false, true, false]);
    assert_eq!(c.pc, AT + 2);

    // cmp r0, r1 then neg r2, r1
    let mut c = thumb();
    c.r[0] = 3;
    c.r[1] = 5;
    run(&mut c, 0x4288);
    assert_eq!(flags(&c), [true, false, false, false]);
    run(&mut c, 0x424A);
    assert_eq!(c.r[2], (-5i32) as u32);
}

#[test]
fn thumb_reads_pc_four_ahead_word_aligned_for_add_pc() {
    // add r0, pc, #4 at a halfword-aligned address
    let mut c = thumb();
    c.pc = AT + 2;
    run(&mut c, 0xA001);
    assert_eq!(c.r[0], ((AT + 2 + 4) & !3) + 4);
    // mov r0, pc (hi-register form): not aligned
    let mut c = thumb();
    c.pc = AT + 2;
    run(&mut c, 0x4678);
    assert_eq!(c.r[0], AT + 2 + 4);
}

#[test]
fn mode_switch_banks_registers() {
    let mut c = cpu();
    c.r[8] = 8;
    c.r[13] = 13;
    // msr cpsr_c, #0x11 (FIQ)
    run(&mut c, 0xE321_F011);
    assert_eq!(c.cpsr & 0x1F, mode::FIQ);
    assert_eq!((c.r[8], c.r[13]), (0, 0));
    c.r[8] = 88;
    // msr cpsr_c, #0x1F (back to system)
    c.pc = AT;
    run(&mut c, 0xE321_F01F);
    assert_eq!((c.r[8], c.r[13]), (8, 13));
    assert_eq!(c.banks[super::Bank::Fiq as usize][0], 88);
}

#[test]
fn user_mode_msr_writes_only_flags() {
    let mut c = Cpu {
        cpsr: mode::USR,
        ..cpu()
    };
    // msr cpsr_fc, #0xF0000013
    run(&mut c, 0xE329_F4F1);
    assert_eq!(c.cpsr & 0x1F, mode::USR);
}

#[test]
fn stm_and_ldm_with_the_base_in_the_list() {
    // stmia r0!, {r0, r1}: r0 is lowest, so the old base is stored.
    let mut c = cpu();
    c.r[0] = 0x200;
    c.r[1] = 7;
    let mut ram = Ram::new();
    c.execute(&mut ram, 0xE8A0_0003);
    assert_eq!((ram.word(0x200), ram.word(0x204)), (0x200, 7));
    assert_eq!(c.r[0], 0x208);

    // stmia r1!, {r0, r1}: r1 isn't lowest, so the new base is stored.
    c.pc = AT;
    c.r[1] = 0x300;
    c.execute(&mut ram, 0xE8A1_0003);
    assert_eq!(ram.word(0x304), 0x308);

    // ldmia r0!, {r0, r1}: the loaded value wins over the writeback.
    c.pc = AT;
    c.r[0] = 0x200;
    c.execute(&mut ram, 0xE8B0_0003);
    assert_eq!((c.r[0], c.r[1]), (0x200, 7));
}

#[test]
fn empty_register_list_transfers_pc_and_moves_the_base_by_0x40() {
    // stmia r0!, {}
    let mut c = cpu();
    c.r[0] = 0x200;
    let ram = run(&mut c, 0xE8A0_0000);
    assert_eq!(ram.word(0x200), AT + 12);
    assert_eq!(c.r[0], 0x240);
}

/// The carry rule's closed form for a zero multiplicand, from zaydlang's
/// test program (github.com/zaydlang/multiplication-algorithm, `main.c`).
fn mul_zero_carry(x: u32) -> bool {
    for (top, low_bits) in [(0xFF_FFFF, 8), (0xFFFF, 16), (0xFF, 24)] {
        if x >> low_bits == top {
            let low = x & ((1 << low_bits) - 1);
            return low < (0xC0 << (low_bits - 8)) && low & 0x5555_5555 != 0;
        }
    }
    x >> 30 == 2
}

#[test]
fn mul_carry_matches_the_closed_form_for_zero() {
    let mut x = 0x1234_5678u32;
    for _ in 0..10_000 {
        // xorshift, plus the patterns the closed form branches on
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        for v in [
            x,
            x | 0xFFFF_FF00,
            x | 0xFFFF_0000,
            x | 0xFF00_0000,
            x >> 2 | 0x8000_0000,
        ] {
            assert_eq!(
                prim::mul_carry(MulKind::Short, 0, v, 0),
                mul_zero_carry(v),
                "{v:#x}"
            );
        }
    }
}

#[test]
fn muls_sets_carry() {
    // muls r0, r1, r2 with r2 = 0x8000_0000: full iterations, C = (rs >> 30) == 2.
    let mut c = cpu();
    c.r[1] = 0;
    c.r[2] = 0x8000_0000;
    run(&mut c, 0xE010_0291);
    assert_eq!(flags(&c), [false, true, true, false]);
}

#[test]
fn arm_pc_reads_keep_a_misaligned_pc() {
    // After a jump to a misaligned ARM address, `add r0, pc, #0` and
    // `add r0, pc, r1` agree: only Thumb word-aligns pc for immediates.
    let mut c = cpu();
    c.pc = AT + 2;
    run(&mut c, 0xE28F_0000);
    assert_eq!(c.r[0], AT + 2 + 8);
    c.pc = AT + 2;
    run(&mut c, 0xE08F_0001);
    assert_eq!(c.r[0], AT + 2 + 8);
}
