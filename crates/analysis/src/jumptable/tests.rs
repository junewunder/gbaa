//! One hand-assembled switch per compiler idiom in
//! `docs/research/jump-tables.md` ("The patterns each compiler emits"),
//! recovered with no knowledge of the compiler.

use super::*;
use crate::program::{Func, ROM_BASE, Region};
use crate::vsa::domain::Src;

/// A ROM being assembled, Thumb only. Addresses are offsets from the start
/// of ROM.
struct Asm(Vec<u8>);

impl Asm {
    fn new() -> Asm {
        Asm(vec![0; 0x200])
    }

    fn h(&mut self, at: u32, v: u16) -> &mut Self {
        self.0[at as usize..at as usize + 2].copy_from_slice(&v.to_le_bytes());
        self
    }

    fn w(&mut self, at: u32, v: u32) -> &mut Self {
        self.0[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
        self
    }

    fn b(&mut self, at: u32, to: u32) -> &mut Self {
        let off = (to as i32 - (at as i32 + 4)) / 2;
        self.h(at, 0xe000 | (off as u16 & 0x7ff))
    }

    /// `b<cond>`: 8 is `hi`, 9 is `ls`.
    fn bcond(&mut self, at: u32, cond: u16, to: u32) -> &mut Self {
        let off = (to as i32 - (at as i32 + 4)) / 2;
        self.h(at, 0xd000 | cond << 8 | (off as u16 & 0xff))
    }

    fn bl(&mut self, at: u32, to: u32) -> &mut Self {
        let off = to as i32 - (at as i32 + 4);
        self.h(at, 0xf000 | ((off >> 12) as u16 & 0x7ff));
        self.h(at + 2, 0xf800 | ((off >> 1) as u16 & 0x7ff))
    }

    /// Cases at `at`, `at + 4`, ...: `movs r0, #k ; bx lr` each.
    fn cases(&mut self, at: u32, n: u32) -> &mut Self {
        for k in 0..n {
            self.h(at + 4 * k, 0x2000 | (k as u16 + 10))
                .h(at + 4 * k + 2, 0x4770);
        }
        self
    }

    /// A program with one Thumb function at the start of ROM, `0..end`.
    fn program(&self, end: u32) -> Program {
        self.program_with(end, end)
    }

    /// Thumb code at `0..code_end`, and one function at `0..func_end`.
    fn program_with(&self, code_end: u32, func_end: u32) -> Program {
        self.funcs(code_end, true, &[(0, func_end)])
    }

    /// Code in one instruction set at `0..code_end`, and these functions.
    fn funcs(&self, code_end: u32, thumb: bool, funcs: &[(u32, u32)]) -> Program {
        let r = |a: u32| ROM_BASE + a;
        let region = Region {
            start: r(0),
            end: r(code_end),
            thumb,
        };
        let funcs = funcs.iter().map(|&(start, end)| Func {
            start: r(start),
            end: r(end),
            thumb,
        });
        Program::new(self.0.clone(), vec![region], funcs.collect())
    }
}

fn thumb(offsets: impl IntoIterator<Item = u32>) -> Vec<u32> {
    offsets.into_iter().map(|a| (ROM_BASE + a) | 1).collect()
}

fn resolved(rec: &Recovery, at: u32) -> &Jump {
    match &rec.jumps[&(ROM_BASE + at)] {
        JumpRes::Resolved(j) => j,
        r => panic!("jump at {at:#x}: {r:?}"),
    }
}

fn word_table(at: u32, n: u32) -> Option<Src> {
    Some(Src {
        first: ROM_BASE + at,
        last: ROM_BASE + at + 4 * (n - 1),
        size: 4,
    })
}

/// agbcc (`GetBgControlAttribute`): the table follows inline, after one
/// pool word holding its address.
#[test]
fn agbcc_word_table() {
    let mut a = Asm::new();
    a.h(0x00, 0x2803) // cmp r0, #3
        .bcond(0x02, 8, 0x40) // bhi default
        .h(0x04, 0x0080) // lsls r0, r0, #2
        .h(0x06, 0x4902) // ldr r1, [pc, #8] (the pool word)
        .h(0x08, 0x1840) // adds r0, r0, r1
        .h(0x0a, 0x6800) // ldr r0, [r0, #0]
        .h(0x0c, 0x4687) // mov pc, r0
        .h(0x0e, 0x46c0) // nop
        .w(0x10, ROM_BASE + 0x14);
    for k in 0..4 {
        a.w(0x14 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4).h(0x40, 0x2000).h(0x42, 0x4770);
    let rec = recover(&a.program(0x44));
    let j = resolved(&rec, 0x0c);
    assert_eq!(j.targets, thumb([0x30, 0x34, 0x38, 0x3c]));
    assert_eq!(j.table, word_table(0x14, 4));
}

/// Modern GCC at `-O2`: the table is out of line in `.rodata`, outside
/// the function, indexed with a register-offset load.
#[test]
fn modern_gcc_word_table() {
    let mut a = Asm::new();
    a.h(0x00, 0x2903) // cmp r1, #3
        .bcond(0x02, 8, 0x40) // bhi default
        .h(0x04, 0x4a01) // ldr r2, [pc, #4] (the pool word)
        .h(0x06, 0x0089) // lsls r1, r1, #2
        .h(0x08, 0x5852) // ldr r2, [r2, r1]
        .h(0x0a, 0x4697) // mov pc, r2
        .w(0x0c, ROM_BASE + 0x100);
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4).h(0x40, 0x2000).h(0x42, 0x4770);
    let rec = recover(&a.program(0x44));
    let j = resolved(&rec, 0x0a);
    assert_eq!(j.targets, thumb([0x30, 0x34, 0x38, 0x3c]));
    assert_eq!(j.table, word_table(0x100, 4));
}

/// LLVM (rustc, `thumbv4t`): an inline byte table of halved offsets from
/// the `add pc`.
#[test]
fn llvm_inline_byte_table() {
    let mut a = Asm::new();
    a.h(0x00, 0x2803) // cmp r0, #3
        .bcond(0x02, 8, 0x20) // bhi default
        .h(0x04, 0x4478) // add r0, pc (r0 += 0x08)
        .h(0x06, 0x7900) // ldrb r0, [r0, #4] (the table at 0x0c)
        .h(0x08, 0x0040) // lsls r0, r0, #1
        .h(0x0a, 0x4487); // add pc, r0 (pc reads 0x0e)
    // (target - 0x0e) / 2 for cases at 0x10, 0x14, 0x18, 0x1c.
    a.0[0x0c..0x10].copy_from_slice(&[1, 3, 5, 7]);
    a.cases(0x10, 4).h(0x20, 0x2000).h(0x22, 0x4770);
    let rec = recover(&a.program(0x24));
    let j = resolved(&rec, 0x0a);
    assert_eq!(j.targets, thumb([0x10, 0x14, 0x18, 0x1c]));
    assert_eq!(
        j.table,
        Some(Src {
            first: ROM_BASE + 0x0c,
            last: ROM_BASE + 0x0f,
            size: 1
        })
    );
}

/// agbcc with a far default: `bls ok ; bl default`, the `bl` a jump into
/// the function's own epilogue. Taken as a call, it would clobber r0 and
/// lose the bound.
#[test]
fn far_jump_default() {
    let mut a = Asm::new();
    a.h(0x00, 0xb510) // push {r4, lr}
        .h(0x02, 0x2803) // cmp r0, #3
        .bcond(0x04, 9, 0x0a) // bls ok
        .bl(0x06, 0x40) // bl default
        .h(0x0a, 0x0080) // ok: lsls r0, r0, #2
        .h(0x0c, 0x4901) // ldr r1, [pc, #4] (the pool word)
        .h(0x0e, 0x1840) // adds r0, r0, r1
        .h(0x10, 0x6800) // ldr r0, [r0, #0]
        .h(0x12, 0x4687) // mov pc, r0
        .w(0x14, ROM_BASE + 0x18);
    for k in 0..4 {
        a.w(0x18 + 4 * k, ROM_BASE + 0x30 + 4 * k);
        a.h(0x30 + 4 * k, 0x2000 | k as u16).b(0x32 + 4 * k, 0x50);
    }
    a.h(0x40, 0x2000) // default: movs r0, #0
        .b(0x42, 0x50)
        .h(0x50, 0xbc10) // pop {r4}
        .h(0x52, 0xbc02) // pop {r1}
        .h(0x54, 0x4708); // bx r1
    let p = a.program(0x56);
    let rec = recover(&p);
    assert!(rec.far_jumps.contains(&(ROM_BASE + 0x06)));
    assert_eq!(
        resolved(&rec, 0x12).targets,
        thumb([0x30, 0x34, 0x38, 0x3c])
    );

    // Without the far-jump rule the jump doesn't resolve.
    let r = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    assert_eq!(r.jumps[&(ROM_BASE + 0x12)], JumpRes::Unresolved);
}

/// A helper inside its caller's span that returns with `bx lr` but
/// branches, so it isn't a straight-line leaf: the probe says it returns
/// balanced, so the `bl` stays a call and the code after it is reached.
#[test]
fn branching_helper_in_the_span_is_a_call() {
    let mut a = Asm::new();
    a.h(0x00, 0xb500) // push {lr}
        .bl(0x02, 0x0a) // bl helper
        .b(0x06, 0x12) // b out
        .h(0x08, 0x46c0) // nop
        .h(0x0a, 0x2800) // helper: cmp r0, #0
        .bcond(0x0c, 0, 0x10) // beq ret
        .h(0x0e, 0x2001) // movs r0, #1
        .h(0x10, 0x4770) // ret: bx lr
        .h(0x12, 0xbd00); // out: pop {pc}
    let p = a.program(0x14);
    let rec = recover(&p);
    assert!(rec.far_jumps.is_empty());
    let r = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Balanced);
    assert!(r.insns.iter().any(|&(at, _)| at == ROM_BASE + 0x12));
    assert_eq!(r.ret_sp, [Some(0)]);

    // Without the probe's answer the own-span rule takes it as a jump.
    let r = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    assert!(r.far_jumps.contains(&(ROM_BASE + 0x02)));
}

/// A table with a target outside the function is rejected, not followed.
#[test]
fn table_leaving_the_function_is_rejected() {
    let mut a = Asm::new();
    // As modern_gcc_word_table, with two cases.
    a.h(0x00, 0x2901)
        .bcond(0x02, 8, 0x40)
        .h(0x04, 0x4a01)
        .h(0x06, 0x0089)
        .h(0x08, 0x5852)
        .h(0x0a, 0x4697)
        .w(0x0c, ROM_BASE + 0x100)
        .w(0x100, ROM_BASE + 0x30)
        .w(0x104, ROM_BASE + 0x80);
    a.cases(0x30, 1)
        .h(0x40, 0x2000)
        .h(0x42, 0x4770)
        .cases(0x80, 1);
    // 0x80 is code, but not this function's.
    let rec = recover(&a.program_with(0x84, 0x44));
    assert!(matches!(
        rec.jumps[&(ROM_BASE + 0x0a)],
        JumpRes::Rejected(_)
    ));

    // Inside the function, but it doesn't decode.
    a.h(0x80, 0xde00);
    let rec = recover(&a.program_with(0x84, 0x84));
    assert!(matches!(
        rec.jumps[&(ROM_BASE + 0x0a)],
        JumpRes::Rejected(_)
    ));
}

/// GCC's `__gnu_thumb1_case_uqi` (moonbeam): `bl` to a helper that reads a
/// byte table after the `bl` and returns past it. The helper runs inline,
/// and the `bl` resolves like any jump, with no knowledge of the helper.
#[test]
fn gcc_case_helper() {
    let mut a = Asm::new();
    a.h(0x00, 0x2803) // cmp r0, #3
        .bcond(0x02, 9, 0x06) // bls
        .b(0x04, 0x20) // b default
        .bl(0x06, 0x40); // bl __gnu_thumb1_case_uqi
    // (target - 0x0a) / 2 for cases at 0x10, 0x14, 0x18, 0x1c.
    a.0[0x0a..0x0e].copy_from_slice(&[3, 5, 7, 9]);
    a.cases(0x10, 4).h(0x20, 0x2000).h(0x22, 0x4770);
    let helper = [
        0xb402, // push {r1}
        0x4671, // mov r1, lr
        0x0849, // lsrs r1, r1, #1
        0x0049, // lsls r1, r1, #1
        0x5c09, // ldrb r1, [r1, r0]
        0x0049, // lsls r1, r1, #1
        0x448e, // add lr, r1
        0xbc02, // pop {r1}
        0x4770, // bx lr
    ];
    for (k, h) in helper.into_iter().enumerate() {
        a.h(0x40 + 2 * k as u32, h);
    }
    let rec = recover(&a.funcs(0x52, true, &[(0, 0x24), (0x40, 0x52)]));
    assert_eq!(
        resolved(&rec, 0x06).targets,
        thumb([0x10, 0x14, 0x18, 0x1c])
    );
}

/// LLVM (rustc, `armv4t`): `adr` then `ldr pc, [r1, r0, lsl #2]`, ARM.
#[test]
fn llvm_arm_word_table() {
    let mut a = Asm::new();
    a.w(0x00, 0xe350_0003) // cmp r0, #3
        .w(0x04, 0x8a00_000d) // bhi 0x40
        .w(0x08, 0xe28f_1000) // add r1, pc, #0 (the table at 0x10)
        .w(0x0c, 0xe791_f100); // ldr pc, [r1, r0, lsl #2]
    for k in 0..4 {
        a.w(0x10 + 4 * k, ROM_BASE + 0x20 + 8 * k);
        a.w(0x20 + 8 * k, 0xe3a0_0000 | k)
            .w(0x24 + 8 * k, 0xe12f_ff1e);
    }
    a.w(0x40, 0xe12f_ff1e);
    let rec = recover(&a.funcs(0x44, false, &[(0, 0x44)]));
    let j = resolved(&rec, 0x0c);
    let arm: Vec<u32> = [0x20, 0x28, 0x30, 0x38].map(|a| ROM_BASE + a).into();
    assert_eq!(j.targets, arm);
    assert_eq!(j.table, word_table(0x10, 4));
}

/// The switch value lives on the stack, and its address is passed to a
/// call, which may change it (`int x = 0; get(&x); switch (x)`): the call
/// forgets the frame, so all four cases stay, not just case 0.
#[test]
fn escaped_local_is_forgotten_at_a_call() {
    let mut a = Asm::new();
    a.h(0x00, 0xb500) // push {lr}
        .h(0x02, 0xb081) // sub sp, #4
        .h(0x04, 0x2000) // movs r0, #0
        .h(0x06, 0x9000) // str r0, [sp, #0]
        .h(0x08, 0x4668) // mov r0, sp
        .bl(0x0a, 0x60) // bl get
        .h(0x0e, 0x9800) // ldr r0, [sp, #0]
        .h(0x10, 0x2803) // cmp r0, #3
        .bcond(0x12, 8, 0x50) // bhi default
        .h(0x14, 0x0080) // lsls r0, r0, #2
        .h(0x16, 0x4902) // ldr r1, [pc, #8] (the pool word)
        .h(0x18, 0x1840) // adds r0, r0, r1
        .h(0x1a, 0x6800) // ldr r0, [r0, #0]
        .h(0x1c, 0x4687) // mov pc, r0
        .h(0x1e, 0x46c0) // nop
        .w(0x20, ROM_BASE + 0x24);
    for k in 0..4 {
        a.w(0x24 + 4 * k, ROM_BASE + 0x40 + 4 * k);
    }
    a.cases(0x40, 4).h(0x50, 0x2000).h(0x52, 0x4770);
    // get: *r0 = 2, with a call so it isn't run inline.
    a.h(0x60, 0xb500) // push {lr}
        .h(0x62, 0x2102) // movs r1, #2
        .h(0x64, 0x6001) // str r1, [r0, #0]
        .bl(0x66, 0x70) // bl nothing
        .h(0x6a, 0xbd00) // pop {pc}
        .h(0x70, 0x4770); // nothing: bx lr
    let p = a.funcs(0x72, true, &[(0, 0x54), (0x60, 0x6c), (0x70, 0x72)]);
    let rec = recover(&p);
    assert_eq!(
        resolved(&rec, 0x1c).targets,
        thumb([0x40, 0x44, 0x48, 0x4c])
    );
}

/// A switch inside a loop over its own index: the index grows until the
/// loop head widens it, and the bounds check still bounds it at the jump,
/// since only loop heads widen.
#[test]
fn switch_in_a_loop() {
    let mut a = Asm::new();
    a.h(0x00, 0x2400) // movs r4, #0
        .h(0x02, 0x2c03) // loop: cmp r4, #3
        .bcond(0x04, 8, 0x60) // bhi done
        .h(0x06, 0x00a0) // lsls r0, r4, #2
        .h(0x08, 0x4902) // ldr r1, [pc, #8] (the pool word at 0x14)
        .h(0x0a, 0x1840) // adds r0, r0, r1
        .h(0x0c, 0x6800) // ldr r0, [r0, #0]
        .h(0x0e, 0x4687) // mov pc, r0
        .w(0x14, ROM_BASE + 0x18);
    for k in 0..4 {
        a.w(0x18 + 4 * k, ROM_BASE + 0x40 + 4 * k);
        // adds r4, #1 ; b loop
        a.h(0x40 + 4 * k, 0x3401).b(0x42 + 4 * k, 0x02);
    }
    a.h(0x60, 0x4770);
    let rec = recover(&a.program(0x62));
    assert_eq!(
        resolved(&rec, 0x0e).targets,
        thumb([0x40, 0x44, 0x48, 0x4c])
    );
}

/// As `escaped_local_is_forgotten_at_a_call`, but the store is inline
/// (`int a[4]; a[0] = 0; a[i] = 3; switch (a[0])`) or in a leaf callee run
/// inline: a store through a pointer into the frame forgets the frame.
#[test]
fn store_through_an_escaped_pointer() {
    for leaf in [false, true] {
        let mut a = Asm::new();
        a.h(0x00, 0xb500) // push {lr}
            .h(0x02, 0xb081) // sub sp, #4
            .h(0x04, 0x2000) // movs r0, #0
            .h(0x06, 0x9000) // str r0, [sp, #0]
            .h(0x08, 0x4668) // mov r0, sp
            .h(0x0a, 0x1c29) // adds r1, r5, #0
            .h(0x0c, 0x2203); // movs r2, #3
        if leaf {
            a.bl(0x0e, 0x60); // bl set
        } else {
            a.h(0x0e, 0x0089).h(0x10, 0x5042); // lsls r1, r1, #2 ; str r2, [r0, r1]
        }
        a.h(0x12, 0x9800) // ldr r0, [sp, #0]
            .h(0x14, 0x2803) // cmp r0, #3
            .bcond(0x16, 8, 0x50) // bhi default
            .h(0x18, 0x0080) // lsls r0, r0, #2
            .h(0x1a, 0x4902) // ldr r1, [pc, #8] (the pool word at 0x24)
            .h(0x1c, 0x1840) // adds r0, r0, r1
            .h(0x1e, 0x6800) // ldr r0, [r0, #0]
            .h(0x20, 0x4687) // mov pc, r0
            .w(0x24, ROM_BASE + 0x28);
        for k in 0..4 {
            a.w(0x28 + 4 * k, ROM_BASE + 0x40 + 4 * k);
        }
        a.cases(0x40, 4).h(0x50, 0x2000).h(0x52, 0x4770);
        // set: lsls r1, r1, #2 ; str r2, [r0, r1] ; bx lr
        a.h(0x60, 0x0089).h(0x62, 0x5042).h(0x64, 0x4770);
        let p = a.funcs(0x66, true, &[(0, 0x54), (0x60, 0x66)]);
        let rec = recover(&p);
        let j = resolved(&rec, 0x20);
        assert_eq!(j.targets, thumb([0x40, 0x44, 0x48, 0x4c]), "leaf {leaf}");
    }
}

#[test]
fn what_a_probe_says() {
    let probe = |ret_sp: Vec<Option<i32>>| FuncResult {
        ret_sp,
        ..FuncResult::default()
    };
    // Every known return pops the caller's frame: a jump. One that doesn't:
    // a call.
    assert_eq!(probe_says(&probe(vec![Some(8)]), Bl::Call), Bl::Jump);
    assert_eq!(
        probe_says(&probe(vec![Some(8), Some(0)]), Bl::Jump),
        Bl::Call
    );
    // Every one leaves sp where it was: a balanced call.
    assert_eq!(
        probe_says(&probe(vec![Some(0), None]), Bl::Jump),
        Bl::Balanced
    );
    // No return with sp known keeps a jump, and is a call otherwise.
    for old in [Bl::Call, Bl::NoReturn] {
        assert_eq!(probe_says(&probe(vec![None]), old), Bl::Call);
    }
    assert_eq!(probe_says(&probe(vec![None]), Bl::Jump), Bl::Jump);
    assert_eq!(probe_says(&probe(vec![]), Bl::Jump), Bl::Jump);
    // Unless what stopped it is a call that never returns.
    let mut stopped = probe(vec![]);
    stopped.no_returns.insert(ROM_BASE);
    assert_eq!(probe_says(&stopped, Bl::Jump), Bl::NoReturn);
    // Or data: the earlier jump ran through a pool decoded as code.
    let data = FuncResult {
        stopped: true,
        ..probe(vec![])
    };
    assert_eq!(probe_says(&data, Bl::Jump), Bl::NoReturn);
    // No return at all: never returns, unless a jump went unresolved.
    assert_eq!(probe_says(&probe(vec![]), Bl::Call), Bl::NoReturn);
    let mut open = probe(vec![]);
    open.jumps.insert(ROM_BASE, JumpRes::Unresolved);
    assert_eq!(probe_says(&open, Bl::Call), Bl::Call);
    let mut thunk = probe(vec![]);
    thunk.exits.insert((ROM_BASE, ROM_BASE + 0x100));
    assert_eq!(probe_says(&thunk, Bl::Call), Bl::Call);
    let capped = FuncResult {
        capped: true,
        ..probe(vec![Some(8)])
    };
    assert_eq!(probe_says(&capped, Bl::Jump), Bl::Call);
}
