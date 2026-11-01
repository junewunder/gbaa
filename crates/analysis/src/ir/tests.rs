//! Builds IR from the semantics of decoded instructions, checks its printed
//! shape, and runs it through a small IR evaluator to check the builder kept
//! the meaning.

use super::Block;
use super::build::Builder;
use armv4t::decode::{decode_arm, decode_thumb};
use armv4t::interp::{self, Bus, Cpu, T_BIT, mode};
use armv4t::sem::arm::execute;
use armv4t::sem::{Flag, Reg, Width, prim};

const START: u32 = 0x0800_0000;

/// One instruction's semantics, applied at an address.
type Insn<'a> = &'a dyn Fn(&mut Builder, u32);

/// Build a block from one semantic fn per instruction, ARM or Thumb.
fn build(thumb: bool, insns: &[Insn]) -> Block {
    let size = if thumb { 2 } else { 4 };
    let mut b = Builder::new(START, thumb);
    let mut at = START;
    for insn in insns {
        b.begin_insn(at);
        insn(&mut b, at);
        at += size;
        if b.ended() {
            break;
        }
    }
    let mut block = b.finish(at);
    block.dce();
    block
}

#[derive(Default, Debug)]
struct State {
    r: [u32; 16],
    flags: [bool; 4],
    mem: [u8; 32],
}

#[derive(Debug, PartialEq, Eq)]
enum Taken {
    Jump(u32),
    Exchange(u32),
    Fallthrough(u32),
    Other,
}

/// A toy memory: 32 bytes, addresses taken mod 32.
struct Mem<'a>(&'a mut [u8; 32]);

impl Bus for Mem<'_> {
    fn load(&mut self, w: Width, addr: u32) -> u32 {
        let a = addr as usize % self.0.len();
        (0..w.bytes() as usize).fold(0, |acc, i| acc | (self.0[a + i] as u32) << (8 * i))
    }
    fn store(&mut self, w: Width, addr: u32, v: u32) {
        let a = addr as usize % self.0.len();
        for i in 0..w.bytes() as usize {
            self.0[a + i] = (v >> (8 * i)) as u8;
        }
    }
}

/// Run a block on `st` through [`super::eval`], in system mode.
fn eval(block: &Block, st: &mut State) -> Taken {
    let mut cpu = Cpu {
        pc: block.start,
        cpsr: mode::SYS | if block.thumb { T_BIT } else { 0 },
        ..Cpu::default()
    };
    cpu.r.copy_from_slice(&st.r[..15]);
    for f in [Flag::N, Flag::Z, Flag::C, Flag::V] {
        cpu.set_flag(f, st.flags[f.index()]);
    }
    let exit = super::eval::run(block, &mut cpu, &mut Mem(&mut st.mem));
    st.r[..15].copy_from_slice(&cpu.r);
    for f in [Flag::N, Flag::Z, Flag::C, Flag::V] {
        st.flags[f.index()] = cpu.flag(f);
    }
    match exit {
        None => Taken::Fallthrough(cpu.pc),
        Some(interp::Exit::Jump(t)) => Taken::Jump(t),
        Some(interp::Exit::Exchange(t)) => Taken::Exchange(t),
        Some(_) => Taken::Other,
    }
}

fn r(n: u8) -> Reg {
    Reg(n)
}

fn adds_r0_r1_r2_lsl_r3(s: &mut Builder, at: u32) {
    execute(s, at, false, decode_arm(0xE091_0312, at));
}

fn moveq_r4_1(s: &mut Builder, at: u32) {
    execute(s, at, false, decode_arm(0x03A0_4001, at));
}

fn ldr_r5_pc_4(s: &mut Builder, at: u32) {
    execute(s, at, false, decode_arm(0xE59F_5004, at));
}

fn bx_lr(s: &mut Builder, at: u32) {
    execute(s, at, false, decode_arm(0xE12F_FF1E, at));
}

#[test]
fn arm_block_shape() {
    let block = build(
        false,
        &[&adds_r0_r1_r2_lsl_r3, &moveq_r4_1, &ldr_r5_pc_4, &bx_lr],
    );
    let expected = "\
block 0x08000000 arm
  ; 0x08000000
  v1 = get r3
  v2 = get r2
  v3 = lsl v2, v1
  v4 = get r1
  v5 = bit false
  v6 = adc v4, v3, v5
  v7 = adc.carry v4, v3, v5
  v8 = adc.overflow v4, v3, v5
  v9 = test_bit v6, 31
  v10 = is_zero v6
  set N, v9
  set Z, v10
  set C, v7
  set V, v8
  set r0, v6
  ; 0x08000004
  v17 = const 0x1
  if v10: set r4, v17
  ; 0x08000008
  v20 = const 0x4
  v21 = const 0x8000010
  v22 = add v21, v20
  v23 = load.word v22
  v24 = const 0x3
  v25 = and v22, v24
  v26 = lsl v25, v24
  v27 = ror v23, v26
  set r5, v27
  ; 0x0800000c
  v30 = get lr
  exit jump_exchange v30
";
    assert_eq!(block.to_string(), expected);
}

#[test]
fn arm_block_runs() {
    let block = build(
        false,
        &[&adds_r0_r1_r2_lsl_r3, &moveq_r4_1, &ldr_r5_pc_4, &bx_lr],
    );
    // r1 + (r2 << 1) = 0xFFFF_FFFE + 2 = 0: Z set, so moveq runs.
    let mut st = State::default();
    st.r[1] = 0xFFFF_FFFE;
    st.r[2] = 1;
    st.r[3] = 1;
    st.r[14] = 0x0800_0101;
    // The literal at 0x0800_0008 + 8 + 4 (mod 32 in the toy memory).
    st.mem[20..24].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes());
    assert_eq!(eval(&block, &mut st), Taken::Exchange(0x0800_0101));
    assert_eq!(st.r[0], 0);
    assert_eq!(st.flags, [false, true, true, false]);
    assert_eq!(st.r[4], 1);
    assert_eq!(st.r[5], 0xCAFE_F00D);

    // Nonzero result: moveq is skipped.
    let mut st = State::default();
    st.r[1] = 5;
    st.r[14] = 0x0800_0100;
    eval(&block, &mut st);
    assert_eq!(st.r[0], 5);
    assert_eq!(st.r[4], 0);
}

/// `beq` mid-block is a guarded side exit; the block continues after it.
fn cmp_r0_0(s: &mut Builder, at: u32) {
    execute(s, at, false, decode_arm(0xE350_0000, at));
}

fn beq_minus_8(s: &mut Builder, at: u32) {
    execute(s, at, false, decode_arm(0x0AFF_FFFE, at));
}

fn add_r0_r0_1(s: &mut Builder, at: u32) {
    execute(s, at, false, decode_arm(0xE280_0001, at));
}

#[test]
fn conditional_branch_is_a_guarded_side_exit() {
    let block = build(false, &[&cmp_r0_0, &beq_minus_8, &add_r0_r0_1]);
    let text = block.to_string();
    assert!(text.contains("if v"), "{text}");
    assert!(text.contains("exit jump"), "{text}");
    assert!(text.ends_with("exit fallthrough 0x0800000c\n"), "{text}");

    let mut st = State::default();
    assert_eq!(eval(&block, &mut st), Taken::Jump(START + 4 + 8 - 8));
    assert_eq!(st.r[0], 0);

    let mut st = State::default();
    st.r[0] = 7;
    assert_eq!(eval(&block, &mut st), Taken::Fallthrough(START + 12));
    assert_eq!(st.r[0], 8);
}

#[test]
fn guarded_write_is_forwarded_through_a_select() {
    // moveq r4, #1; add r0, r4, #1: the add must see r4 merged.
    fn add_r0_r4_1(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0xE284_0001, at));
    }
    let block = build(false, &[&moveq_r4_1, &add_r0_r4_1]);
    assert!(block.to_string().contains("select"));
    for (z, r4, want) in [(true, 9, 2), (false, 9, 10)] {
        let mut st = State::default();
        st.flags[Flag::Z.index()] = z;
        st.r[4] = r4;
        eval(&block, &mut st);
        assert_eq!(st.r[0], want);
    }
}

#[test]
fn thumb_lsl_block() {
    let block = build(
        true,
        &[&|s, at| execute(s, at, true, decode_thumb(0x4088, at))],
    );
    let expected = "\
block 0x08000000 thumb
  ; 0x08000000
  v1 = get C
  v2 = get r1
  v3 = get r0
  v4 = lsl v3, v2
  v5 = lsl.carry v3, v2, v1
  v6 = test_bit v4, 31
  v7 = is_zero v4
  set N, v6
  set Z, v7
  set C, v5
  set r0, v4
  exit fallthrough 0x08000002
";
    assert_eq!(block.to_string(), expected);

    let mut st = State::default();
    st.r[0] = 0x1000_0001;
    st.r[1] = 4;
    assert_eq!(eval(&block, &mut st), Taken::Fallthrough(START + 2));
    assert_eq!(st.r[0], 0x10);
    assert_eq!(st.flags, [false, false, true, false]);
}

#[test]
fn store_with_writeback_runs() {
    // str r0, [r1, #-4]!
    fn str_r0(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0xE521_0004, at));
    }
    let block = build(false, &[&str_r0]);
    let mut st = State::default();
    st.r[0] = 0xDEAD_BEEF;
    st.r[1] = 12;
    eval(&block, &mut st);
    assert_eq!(st.mem[8..12], 0xDEAD_BEEFu32.to_le_bytes());
    assert_eq!(st.r[1], 8);
}

#[test]
fn guarded_flag_write_is_forwarded_through_a_bselect() {
    // movnes r0, r1, lsr #32 (C = r1 bit 31 when it runs), then adc r2, r2, #0.
    fn movnes(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0x11B0_0021, at));
    }
    fn adc_r2(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0xE2A2_2000, at));
    }
    let block = build(false, &[&movnes, &adc_r2]);
    assert!(block.to_string().contains("bselect"));
    // (Z, old C, r1) -> r2 = C after the mov.
    for (z, c, r1, want) in [
        (false, false, 0x8000_0000, 1),
        (false, true, 0, 0),
        (true, true, 0x8000_0000, 1),
        (true, false, 0x8000_0000, 0),
    ] {
        let mut st = State::default();
        st.flags[Flag::Z.index()] = z;
        st.flags[Flag::C.index()] = c;
        st.r[1] = r1;
        eval(&block, &mut st);
        assert_eq!(st.r[2], want, "z={z} c={c} r1={r1:#x}");
    }
}

#[test]
fn nested_when_ands_guards_and_cpsr_writes_flush_the_cache() {
    use armv4t::sem::{PsrMask, Sem};
    let mut b = Builder::new(START, false);
    b.begin_insn(START);
    let one = b.imm(1);
    b.set_reg(r(0), one);
    let n = b.flag(Flag::N);
    let z = b.flag(Flag::Z);
    b.when(n, |b| b.when(z, |b| b.set_reg(r(1), one)));
    let v = b.imm(0x1F);
    b.set_cpsr(v, PsrMask::CONTROL);
    // After a mode switch r0 must be read again, not forwarded.
    let r0 = b.reg(r(0));
    b.set_reg(r(2), r0);
    let block = b.finish(START + 4);
    let text = block.to_string();
    assert!(text.contains("band"), "{text}");
    assert_eq!(text.matches("get r0").count(), 1, "{text}");
    assert!(
        text.find("set cpsr").unwrap() < text.find("get r0").unwrap(),
        "{text}"
    );
}

#[test]
#[should_panic(expected = "effect after exit")]
fn effect_after_a_guarded_exit_is_caught() {
    use armv4t::sem::Sem;
    let mut b = Builder::new(START, false);
    b.begin_insn(START);
    let z = b.flag(Flag::Z);
    b.when(z, |b| {
        let t = b.imm(0);
        b.jump(t);
        b.set_reg(r(0), t);
    });
}

#[test]
fn swi_exit_resumes_after_the_instruction() {
    let block = build(
        true,
        &[&|s, at| execute(s, at, true, decode_thumb(0xDF05, at))],
    );
    assert!(
        block
            .to_string()
            .ends_with("exit swi 0x5, next 0x08000002\n")
    );
}

#[test]
fn muls_sets_carry_through_mul_carry() {
    // muls r0, r1, r2
    let block = build(
        false,
        &[&|s, at| execute(s, at, false, decode_arm(0xE010_0291, at))],
    );
    assert!(block.to_string().contains("mul_carry short"), "{block}");
    for (r1, r2) in [
        (0, 0x8000_0000),
        (0x1234_5678, 0x9ABC_DEF0),
        (7, 0xFFFF_FF3F),
    ] {
        let mut st = State::default();
        st.r[1] = r1;
        st.r[2] = r2;
        eval(&block, &mut st);
        let want_c = prim::mul_carry(prim::MulKind::Short, r1, r2, 0);
        assert_eq!(st.r[0], r1.wrapping_mul(r2));
        assert_eq!(st.flags[Flag::C.index()], want_c, "{r1:#x} * {r2:#x}");
    }
}

/// Run one ARM instruction from `cpu` through the IR and through the
/// interpreter, check they agree, and return the CPU after it.
fn ir_matches_interp(raw: u32, cpu: Cpu) -> Cpu {
    let block = build(
        false,
        &[&|s, at| execute(s, at, false, decode_arm(raw, at))],
    );
    let (mut ir, mut mem) = (cpu.clone(), [0; 32]);
    super::eval::run(&block, &mut ir, &mut Mem(&mut mem));
    let (mut want, mut mem) = (cpu, [0; 32]);
    want.execute(&mut Mem(&mut mem), raw);
    assert_eq!(ir, want, "{raw:#010x}\n{block}");
    ir
}

#[test]
fn msr_in_user_mode_writes_only_the_flags() {
    // msr cpsr_fc, r0, asking for system mode with all four flags.
    let mut cpu = Cpu {
        pc: START,
        cpsr: mode::USR,
        ..Cpu::default()
    };
    cpu.r[0] = 0xF000_0000 | mode::SYS;
    let cpu = ir_matches_interp(0xE129_F000, cpu);
    assert_eq!(cpu.cpsr, 0xF000_0000 | mode::USR);
}

#[test]
fn movs_pc_without_an_spsr_keeps_the_flags_it_set() {
    // movs pc, lr in user mode: N from lr, then a jump with the CPSR as is.
    let mut cpu = Cpu {
        pc: START,
        cpsr: mode::USR | 0x4000_0000,
        ..Cpu::default()
    };
    cpu.r[14] = 0x8000_0010;
    let cpu = ir_matches_interp(0xE1B0_F00E, cpu);
    assert_eq!(cpu.cpsr, mode::USR | 0x8000_0000);
    assert_eq!(cpu.pc, 0x8000_0010);
}

#[test]
fn subs_pc_lr_returns_to_the_spsr_mode() {
    // subs pc, lr, #4 in IRQ mode, back to Thumb in user mode.
    let mut cpu = Cpu {
        pc: START,
        cpsr: mode::IRQ,
        ..Cpu::default()
    };
    cpu.spsr[interp::Bank::Irq as usize] = 0x2000_0000 | T_BIT | mode::USR;
    cpu.r[14] = 0x0800_0105;
    cpu.banks[interp::Bank::User as usize][6] = 0x1234;
    let cpu = ir_matches_interp(0xE25E_F004, cpu);
    assert_eq!(cpu.cpsr, 0x2000_0000 | T_BIT | mode::USR);
    assert_eq!(cpu.pc, 0x0800_0101);
    assert_eq!(cpu.r[14], 0x1234);
}

#[test]
#[should_panic(expected = "used as a bit")]
fn a_word_used_as_a_bit_is_caught() {
    use armv4t::sem::Sem;
    let mut b = Builder::new(START, false);
    b.begin_insn(START);
    let w = b.imm(2);
    let word = b.bit_to_word(w);
    b.set_reg(r(0), word);
    let block = b.finish(START + 4);
    let mut cpu = Cpu {
        pc: START,
        cpsr: mode::SYS,
        ..Cpu::default()
    };
    super::eval::run(&block, &mut cpu, &mut Mem(&mut [0; 32]));
}

// Const-prop and flag liveness (decision 9).

use super::constprop::Rom;
use super::flags::{self, Flags};

/// A ROM whose word at `0x0800_0014` (the sample block's literal) is
/// `0xCAFE_F00D`.
fn literal_rom() -> Vec<u8> {
    let mut rom = vec![0; 0x100];
    rom[0x14..0x18].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes());
    rom
}

#[test]
fn const_prop_folds_the_literal_pool() {
    let rom = literal_rom();
    let mut block = build(
        false,
        &[&adds_r0_r1_r2_lsl_r3, &moveq_r4_1, &ldr_r5_pc_4, &bx_lr],
    );
    block.optimize(Rom(&rom));
    // The load stays for its wait states; r5 gets the ROM's word. The
    // adds' flags stay live: the block leaves through `bx`.
    let expected = "\
block 0x08000000 arm
  ; 0x08000000
  v1 = get r3
  v2 = get r2
  v3 = lsl v2, v1
  v4 = get r1
  v5 = bit false
  v6 = adc v4, v3, v5
  v7 = adc.carry v4, v3, v5
  v8 = adc.overflow v4, v3, v5
  v9 = test_bit v6, 31
  v10 = is_zero v6
  set N, v9
  set Z, v10
  set C, v7
  set V, v8
  set r0, v6
  ; 0x08000004
  v17 = const 0x1
  if v10: set r4, v17
  ; 0x08000008
  v20 = const 0x8000014
  v21 = load.word v20
  v22 = pool 0xcafef00d
  set r5, v22
  ; 0x0800000c
  v25 = get lr
  exit jump_exchange v25
";
    assert_eq!(block.to_string(), expected);
    let mut st = State::default();
    st.r[14] = 0x0800_0100;
    st.mem[20..24].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes());
    eval(&block, &mut st);
    assert_eq!(st.r[5], 0xCAFE_F00D);
}

#[test]
fn rom_loads_fold_only_from_the_rom() {
    let mut rom = vec![0; 0x200];
    rom[0x100..0x104].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    rom[0xC4] = 1;
    let rom = Rom(&rom);
    assert_eq!(rom.load(Width::Word, 0x0800_0100), Some(0x1234_5678));
    assert_eq!(rom.load(Width::Word, 0x0800_0102), Some(0x1234_5678));
    assert_eq!(rom.load(Width::Half, 0x0800_0102), Some(0x1234));
    assert_eq!(rom.load(Width::Byte, 0x0800_0101), Some(0x56));
    // The wait-state mirrors, but not 0x0D (the EEPROM's).
    assert_eq!(rom.load(Width::Word, 0x0A00_0100), Some(0x1234_5678));
    assert_eq!(rom.load(Width::Word, 0x0C00_0100), Some(0x1234_5678));
    assert_eq!(rom.load(Width::Word, 0x0D00_0100), None);
    // Past the image, the GPIO port, and not ROM at all.
    assert_eq!(rom.load(Width::Word, 0x0800_0200), None);
    assert_eq!(rom.load(Width::Byte, 0x0800_00C4), None);
    assert_eq!(rom.load(Width::Word, 0x0800_00C4), None);
    assert_eq!(rom.load(Width::Half, 0x0800_00C8), None);
    assert_eq!(rom.load(Width::Word, 0x0800_00C0), Some(0));
    assert_eq!(rom.load(Width::Half, 0x0800_00CA), Some(0));
    assert_eq!(rom.load(Width::Word, 0x0300_0100), None);
    assert_eq!(Rom::NONE.load(Width::Word, 0x0800_0100), None);
}

#[test]
fn const_prop_folds_arithmetic_through_registers() {
    // mov r0, #3; add r1, r0, r0, lsl #2; eor r2, r1, r1
    let block = |opt: bool| {
        let mut b = build(
            false,
            &[
                &|s, at| execute(s, at, false, decode_arm(0xE3A0_0003, at)),
                &|s, at| execute(s, at, false, decode_arm(0xE080_1100, at)),
                &|s, at| execute(s, at, false, decode_arm(0xE021_2001, at)),
            ],
        );
        if opt {
            b.optimize(Rom::NONE);
        }
        b
    };
    let block = block(true);
    let text = block.to_string();
    assert!(text.contains("const 0xf\n  set r1"), "{text}");
    assert!(!text.contains("lsl") && !text.contains("add"), "{text}");
    let mut st = State::default();
    assert_eq!(eval(&block, &mut st), Taken::Fallthrough(START + 12));
    assert_eq!(st.r[..3], [3, 15, 0]);
}

#[test]
fn a_guard_known_true_ends_the_block_at_its_exit() {
    // movs r0, #0; beq .; add r0, r0, #1: Z is known set, so beq always
    // jumps and the add is unreachable.
    fn movs_r0_0(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0xE3B0_0000, at));
    }
    let mut block = build(false, &[&movs_r0_0, &beq_minus_8, &add_r0_r0_1]);
    block.optimize(Rom::NONE);
    let text = block.to_string();
    assert!(text.contains("exit jump v"), "{text}");
    assert!(
        !text.contains("if v") && !text.contains("fallthrough"),
        "{text}"
    );
    let mut st = State::default();
    st.r[0] = 9;
    assert_eq!(eval(&block, &mut st), Taken::Jump(START + 4));
    assert_eq!(st.r[0], 0);
    assert_eq!(st.flags, [false, true, false, false]);
}

#[test]
fn a_guard_known_false_deletes_what_it_guards() {
    // movs r0, #1; moveq r4, #1; add r0, r4, #1: Z is known clear, so the
    // moveq never runs and the add reads r4 itself.
    fn movs_r0_1(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0xE3B0_0001, at));
    }
    fn add_r0_r4_1(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0xE284_0001, at));
    }
    let mut block = build(false, &[&movs_r0_1, &moveq_r4_1, &add_r0_r4_1]);
    block.optimize(Rom::NONE);
    let text = block.to_string();
    assert!(!text.contains("select") && !text.contains("if v"), "{text}");
    let mut st = State::default();
    st.r[4] = 9;
    eval(&block, &mut st);
    assert_eq!(st.r[0], 10);
    assert_eq!(st.r[4], 9);
}

#[test]
fn overwritten_flags_are_dead_within_a_block() {
    // adds r0, r1, r2, lsl r3; moveq r4, #1; cmp r0, #0; bx lr: the cmp
    // overwrites all four of the adds' flags, and the moveq reads Z as the
    // forwarded value, not the flag, so all four writes are dead.
    let mut block = build(
        false,
        &[&adds_r0_r1_r2_lsl_r3, &moveq_r4_1, &cmp_r0_0, &bx_lr],
    );
    block.optimize(Rom::NONE);
    let text = block.to_string();
    let sets = |f: &str| text.matches(&format!("set {f},")).count();
    assert_eq!(
        [sets("N"), sets("Z"), sets("C"), sets("V")],
        [1, 1, 1, 1],
        "{text}"
    );
    assert!(
        !text.contains("adc.carry v4") && !text.contains("adc.overflow v4"),
        "{text}"
    );
    let mut st = State::default();
    st.r[1] = 0xFFFF_FFFE;
    st.r[2] = 1;
    st.r[3] = 1;
    eval(&block, &mut st);
    assert_eq!(st.r[4], 1);
    assert_eq!(st.flags, [false, true, true, false]);
}

#[test]
fn a_guarded_flag_write_does_not_kill() {
    // adds r0, ...; movnes r1, r2: the movnes's N and Z are guarded, so the
    // adds' N and Z may survive to the exit and stay.
    fn movnes_r1_r2(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0x11B0_1002, at));
    }
    let mut block = build(false, &[&adds_r0_r1_r2_lsl_r3, &movnes_r1_r2]);
    let before = block.clone();
    block.optimize(Rom::NONE);
    assert_eq!(block, before, "{block}");
}

#[test]
fn flag_liveness_follows_static_edges() {
    // Block A: adds r0, r1, r2, lsl r3, falling through to block B:
    // beq (to B itself); cmp r0, #0; then leaving the function. B reads
    // only Z before the cmp writes all four, so A keeps only its Z write.
    let mut a = build(false, &[&adds_r0_r1_r2_lsl_r3]);
    let mut b = Builder::new(START + 4, false);
    for (i, insn) in [&beq_minus_8 as Insn, &cmp_r0_0].iter().enumerate() {
        let at = START + 4 + 4 * i as u32;
        b.begin_insn(at);
        insn(&mut b, at);
    }
    let mut b = b.finish(START + 12);
    b.dce();
    let (b_before, fall, b_last) = (b.clone(), a.insts.len() - 1, b.insts.len() - 1);
    let removed = flags::remove_dead(&mut [&mut a, &mut b], |blk, i| match blk {
        0 => (i == fall).then_some(1),
        _ => (i != b_last).then_some(1),
    });
    assert_eq!(removed, 3);
    let text = a.to_string();
    assert!(
        text.contains("set Z,") && !text.contains("set N,"),
        "{text}"
    );
    assert!(
        !text.contains("set C,") && !text.contains("set V,"),
        "{text}"
    );
    assert_eq!(b, b_before);
    let z = Flags(1 << Flag::Z.index());
    assert_eq!(
        b.live_flags(|i| if i == b_last { Flags::ALL } else { z }),
        z
    );
}

#[test]
fn flags_stay_live_into_a_loop() {
    // A block that falls through to itself: addne r0, r0, #1; cmp r0, #0.
    // The next round's addne reads the cmp's Z, so only Z stays.
    fn addne_r0_r0_1(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0x1280_0001, at));
    }
    let mut a = build(false, &[&addne_r0_r0_1, &cmp_r0_0]);
    let fall = a.insts.len() - 1;
    let removed = flags::remove_dead(&mut [&mut a], |_, i| (i == fall).then_some(0));
    assert_eq!(removed, 3);
    let text = a.to_string();
    assert!(
        text.contains("set Z,") && !text.contains("set N,"),
        "{text}"
    );
}

#[test]
fn mrs_spsr_without_an_spsr_reads_the_flags() {
    // adds r0, ...; mrs r3, spsr; cmp r0, #0; bx lr, in system mode: the
    // mrs reads the CPSR, so the cmp doesn't make the adds' flags dead.
    fn mrs_r3_spsr(s: &mut Builder, at: u32) {
        execute(s, at, false, decode_arm(0xE14F_3000, at));
    }
    let mut block = build(
        false,
        &[&adds_r0_r1_r2_lsl_r3, &mrs_r3_spsr, &cmp_r0_0, &bx_lr],
    );
    block.optimize(Rom::NONE);
    let mut st = State::default();
    st.r[1] = 0x8000_0000;
    eval(&block, &mut st);
    assert_eq!(st.r[3] >> 28, 0b1000, "{block}");
}

/// The ARM instruction `raw`'s semantics.
fn arm(raw: u32) -> impl Fn(&mut Builder, u32) {
    move |s: &mut Builder, at: u32| execute(s, at, false, decode_arm(raw, at))
}

/// How many `SetReg(r)`s `block` has.
fn reg_writes(block: &Block, r: Reg) -> usize {
    block
        .insts
        .iter()
        .filter(|i| matches!(i.op, super::Op::SetReg(x, _) if x == r))
        .count()
}

#[test]
fn a_register_write_overwritten_in_the_block_is_dead() {
    // mov r0, #1; str r0, [r2]; mov r0, #2; bx lr: the store reads the
    // forwarded value, not `cpu.r`, so only the second write stays.
    let (mov1, str_r0, mov2) = (arm(0xE3A0_0001), arm(0xE582_0000), arm(0xE3A0_0002));
    let mut block = build(false, &[&mov1, &str_r0, &mov2, &bx_lr]);
    assert_eq!(block.remove_dead_reg_writes(), 1, "{block}");
    assert_eq!(reg_writes(&block, r(0)), 1, "{block}");
    let mut st = State::default();
    st.r[2] = 4;
    eval(&block, &mut st);
    assert_eq!((st.r[0], st.mem[4]), (2, 1), "{block}");
}

#[test]
fn register_writes_stay_where_something_can_read_them() {
    // A guarded write doesn't kill: its false path keeps the first.
    let (mov1, movne2, mov2) = (arm(0xE3A0_0001), arm(0x13A0_0002), arm(0xE3A0_0002));
    let mut block = build(false, &[&mov1, &movne2, &bx_lr]);
    assert_eq!(block.remove_dead_reg_writes(), 0, "{block}");
    // A side exit, an `mrs` or an `msr` between two writes keeps both.
    let between: [Insn; 3] = [&beq_minus_8, &arm(0xE10F_3000), &arm(0xE121_F001)];
    for mid in between {
        let mut block = build(false, &[&mov1, mid, &mov2, &bx_lr]);
        assert_eq!(block.remove_dead_reg_writes(), 0, "{block}");
        assert_eq!(reg_writes(&block, r(0)), 2, "{block}");
    }
}
