//! One test per instruction class. Each expected text is what
//! arm-none-eabi-objdump 15.2 (`-m armv4t`) prints for the encoding, in this
//! module's style (no comments, `r11` for `fp`, hex above 255); the
//! `insn-text` check compares the two on whole ROMs.

use super::*;
use crate::decode::{decode_arm, decode_thumb};

const AT: u32 = 0x0800_0000;

fn arm(cases: &[(u32, &str)]) {
    for &(w, want) in cases {
        let got = decode_arm(w, AT).text(false).to_string();
        assert_eq!(got, want, "{w:#010x}");
    }
}

fn thumb(cases: &[(u16, &str)]) {
    for &(h, want) in cases {
        let got = decode_thumb(h, AT).text(true).to_string();
        assert_eq!(got, want, "{h:#06x}");
    }
}

#[test]
fn arm_data_processing() {
    arm(&[
        (0xE3A000FF, "mov r0, #255"),
        (0xE3800CFF, "orr r0, r0, #0xff00"),
        (0xE3A00101, "mov r0, #0x40000000"),
        (0xE2384E01, "eors r4, r8, #1, 28"),
        (0x03FE52FF, "mvnseq r5, #0xf000000f"),
        (0xE0500001, "subs r0, r0, r1"),
        (0xE0900211, "adds r0, r0, r1, lsl r2"),
        (0x619ACAA3, "orrsvs r12, r10, r3, lsr #21"),
        (0x00000073, "andeq r0, r0, r3, ror r0"),
        (0xE2610000, "rsb r0, r1, #0"),
        (0xE08F0001, "add r0, pc, r1"),
        (0xE1500000, "cmp r0, r0"),
        (0xE13F0001, "teq pc, r1"),
        (0xC1194BCE, "tstgt r9, lr, asr #23"),
        (0x735D849F, "cmpvc sp, #0x9f000000"),
        (0xE15FF000, "cmpp pc, r0"),
        (0xE1B0F00E, "movs pc, lr"),
    ]);
}

#[test]
fn arm_shifts_as_moves() {
    arm(&[
        (0xE1A00001, "mov r0, r1"),
        (0xE1A00101, "lsl r0, r1, #2"),
        (0xE1A00021, "lsr r0, r1, #32"),
        (0xE1A00040, "asr r0, r0, #32"),
        (0xE1B00061, "rrxs r0, r1"),
        (0xE1A00211, "lsl r0, r1, r2"),
        (0xE1A00000, "mov r0, r0"),
    ]);
}

#[test]
fn arm_multiplies() {
    arm(&[
        (0xE0010291, "mul r1, r1, r2"),
        (0xE0210392, "mla r1, r2, r3, r0"),
        (0xE0810392, "umull r0, r1, r2, r3"),
        (0xC08A5694, "umullgt r5, r10, r4, r6"),
        (0xE0C10392, "smull r0, r1, r2, r3"),
        (0xE0F10392, "smlals r0, r1, r2, r3"),
    ]);
}

#[test]
fn arm_single_transfers() {
    arm(&[
        (0xE5910000, "ldr r0, [r1]"),
        (0xE5310000, "ldr r0, [r1, #0]!"),
        (0xE4910000, "ldr r0, [r1], #0"),
        (0xE7910102, "ldr r0, [r1, r2, lsl #2]"),
        (0x07F8D421, "ldrbeq sp, [r8, r1, lsr #8]!"),
        (0x44033EBB, "strmi r3, [r3], #-3771"),
        (0x943A1188, "ldrtls r1, [r10], #-392"),
        (0xE4ED0001, "strbt r0, [sp], #1"),
        (0xE5BF0004, "ldr r0, [pc, #4]!"),
        (0xE79F0000, "ldr r0, [pc, r0]"),
        (0xE6900060, "ldr r0, [r0], r0, rrx"),
        (0xE49D0004, "ldr r0, [sp], #4"),
        (0xE52D0004, "str r0, [sp, #-4]!"),
    ]);
}

#[test]
fn arm_halfword_transfers() {
    arm(&[
        (0xE1D100B0, "ldrh r0, [r1]"),
        (0xE1F100B0, "ldrh r0, [r1, #0]!"),
        (0xE19100B2, "ldrh r0, [r1, r2]"),
        (0xE0D100D4, "ldrsb r0, [r1], #4"),
        (0xE1C000B0, "strh r0, [r0]"),
        (0xE0C000B0, "strh r0, [r0], #0"),
        (0xE11000F2, "ldrsh r0, [r0, -r2]"),
    ]);
}

#[test]
fn arm_block_transfers() {
    arm(&[
        (0xE92D4010, "push {r4, lr}"),
        (0xE8BD4010, "pop {r4, lr}"),
        (0xE8BD0010, "ldm sp!, {r4}"),
        (0xE92D0010, "stmdb sp!, {r4}"),
        (0xE8900003, "ldm r0, {r0, r1}"),
        (0x19BE52A3, "ldmibne lr!, {r0, r1, r5, r7, r9, r12, lr}"),
        (0x988B2411, "stmls r11, {r0, r4, r10, sp}"),
        (0xE8D08000, "ldm r0, {pc}^"),
        (0xE96D0003, "stmdb sp!, {r0, r1}^"),
    ]);
}

#[test]
fn arm_swap_and_psr() {
    arm(&[
        (0xE1020091, "swp r0, r1, [r2]"),
        (0xE1420091, "swpb r0, r1, [r2]"),
        (0xE10F0000, "mrs r0, cpsr"),
        (0xE14F0000, "mrs r0, spsr"),
        (0xE129F000, "msr cpsr_fc, r0"),
        (0xE161F000, "msr spsr_c, r0"),
        (0xE120F000, "msr cpsr_, r0"),
        (0xE328F20F, "msr cpsr_f, #0xf0000000"),
    ]);
}

#[test]
fn arm_branches_and_swi() {
    arm(&[
        (0xEA00002E, "b 0x080000c0"),
        (0xEB000780, "bl 0x08001e08"),
        (0x0A82843D, "beq 0x060a10fc"),
        (0xE12FFF1E, "bx lr"),
        (0x012FFF10, "bxeq r0"),
        (0xEF000005, "svc 0x5"),
        (0xBFCEE382, "svclt 0xcee382"),
        (0xE6000010, ".word 0xe6000010"),
    ]);
}

#[test]
fn thumb_alu() {
    thumb(&[
        (0x07C0, "lsls r0, r0, #31"),
        (0x0800, "lsrs r0, r0, #32"),
        (0x1000, "asrs r0, r0, #32"),
        (0x0008, "movs r0, r1"),
        (0x1C08, "adds r0, r1, #0"),
        (0x1888, "adds r0, r1, r2"),
        (0x1A88, "subs r0, r1, r2"),
        (0x2001, "movs r0, #1"),
        (0x2801, "cmp r0, #1"),
        (0x3001, "adds r0, #1"),
        (0x3801, "subs r0, #1"),
        (0x4008, "ands r0, r1"),
        (0x40C8, "lsrs r0, r1"),
        (0x41C8, "rors r0, r1"),
        (0x4140, "adcs r0, r0"),
        (0x4200, "tst r0, r0"),
        (0x4248, "negs r0, r1"),
        (0x4348, "muls r0, r1"),
        (0x43C0, "mvns r0, r0"),
    ]);
}

#[test]
fn thumb_hi_registers_and_sp() {
    thumb(&[
        (0x4440, "add r0, r8"),
        (0x4608, "mov r0, r1"),
        (0x4687, "mov pc, r0"),
        (0x46C0, "mov r8, r8"),
        (0x4580, "cmp r8, r0"),
        (0x4708, "bx r1"),
        (0x4770, "bx lr"),
        (0xA802, "add r0, sp, #8"),
        (0xA002, "add r0, pc, #8"),
        (0xB07F, "add sp, #0x1fc"),
        (0xB0FE, "sub sp, #0x1f8"),
    ]);
}

#[test]
fn thumb_transfers() {
    thumb(&[
        (0x4801, "ldr r0, [pc, #4]"),
        (0x5888, "ldr r0, [r1, r2]"),
        (0x5688, "ldrsb r0, [r1, r2]"),
        (0x5E88, "ldrsh r0, [r1, r2]"),
        (0x5288, "strh r0, [r1, r2]"),
        (0x6848, "ldr r0, [r1, #4]"),
        (0x6030, "str r0, [r6, #0]"),
        (0x7848, "ldrb r0, [r1, #1]"),
        (0x8848, "ldrh r0, [r1, #2]"),
        (0x9801, "ldr r0, [sp, #4]"),
        (0xB510, "push {r4, lr}"),
        (0xBD10, "pop {r4, pc}"),
        (0xB401, "push {r0}"),
        (0xC806, "ldmia r0!, {r1, r2}"),
        (0xC803, "ldmia r0, {r0, r1}"),
        (0xC003, "stmia r0!, {r0, r1}"),
    ]);
}

#[test]
fn thumb_branches() {
    thumb(&[
        (0xD100, "bne 0x08000004"),
        (0xD3FE, "bcc 0x08000000"),
        (0xE002, "b 0x08000008"),
        (0xDF05, "svc 0x5"),
        (0xDE00, ".short 0xde00"),
        (0xF000, "bl.hi #0"),
        (0xF802, "bl.lo #4"),
    ]);
    assert_eq!(Bl(0x080a_6a54).to_string(), "bl 0x080a6a54");
}
