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

/// m4a's `ply_*` routines keep the return address in r12 (`mov r12, lr`)
/// and leave by `bx r12`: a return, not an unresolved jump. A `bx` through
/// any other register, with sp and lr as they came, is a tail call through
/// a pointer (librfu's callback stubs under modern GCC); with a frame
/// pushed it stays a jump.
#[test]
fn bx_through_a_copy_of_lr_is_a_return() {
    let mut a = Asm::new();
    a.h(0x00, 0x46f4) // mov r12, lr
        .h(0x02, 0x2001) // movs r0, #1
        .h(0x04, 0x4760); // bx r12
    let p = a.program(0x06);
    let r = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    assert!(r.jumps.is_empty(), "{:?}", r.jumps);
    assert_eq!(r.ret_sp, [Some(0)]);

    let mut a = Asm::new();
    a.h(0x00, 0x468c) // mov r12, r1
        .h(0x02, 0x4760); // bx r12
    let p = a.program(0x04);
    let r = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    assert!(r.jumps.is_empty(), "{:?}", r.jumps);
    assert_eq!(r.tail_calls, [ROM_BASE + 0x02].into());
    // A stub of it still leaves: a `bl` to one isn't a call that never
    // returns.
    assert_eq!(probe_says(&r, Bl::Call), Bl::Call);

    let mut a = Asm::new();
    a.h(0x00, 0xb510) // push {r4, lr}
        .h(0x02, 0x468c) // mov r12, r1
        .h(0x04, 0x4760); // bx r12
    let p = a.program(0x06);
    let r = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    assert_eq!(r.jumps[&(ROM_BASE + 0x04)], JumpRes::Unresolved);
    assert!(r.tail_calls.is_empty());

    // `mov pc` isn't a tail call however sp and lr stand: a switch's.
    let mut a = Asm::new();
    a.h(0x00, 0x468f); // mov pc, r1
    let p = a.program(0x02);
    let r = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    assert_eq!(r.jumps[&ROM_BASE], JumpRes::Unresolved);
}

/// A `bl` to an interworking veneer (`bx pc; nop`, then ARM code)
/// placed right after another function's `pop {pc}` is a call that
/// returns: the `bx pc` isn't `pop {rN}; bx rN`'s return.
#[test]
fn bx_pc_after_pop_pc_is_a_veneer() {
    let mut a = Asm::new();
    a.bl(0x00, 0x10)
        .h(0x04, 0x2001) // movs r0, #1
        .h(0x06, 0x4770) // bx lr
        .h(0x0c, 0x2000) // movs r0, #0 (the function before the veneer)
        .h(0x0e, 0xbd00) // pop {pc}
        .h(0x10, 0x4778) // bx pc
        .h(0x12, 0x46c0) // nop
        .w(0x14, 0xe12fff1e); // ARM: bx lr
    let r = |a: u32| ROM_BASE + a;
    let region = |s: u32, e: u32, thumb: bool| Region {
        start: r(s),
        end: r(e),
        thumb,
    };
    let func = |s: u32, e: u32, thumb: bool| Func {
        start: r(s),
        end: r(e),
        thumb,
    };
    let p = Program::new(
        a.0.clone(),
        vec![region(0, 0x14, true), region(0x14, 0x18, false)],
        vec![
            func(0, 0x08, true),
            func(0x0c, 0x10, true),
            func(0x10, 0x14, true),
            func(0x14, 0x18, false),
        ],
    );
    let res = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    assert!(res.jumps.is_empty(), "{:?}", res.jumps);
    assert_eq!(res.ret_sp, [Some(0)]);
}

/// The same through a `bl` to a helper that branches (m4a's
/// `ld_r3_tp_adr_i`) but never writes r12: the call keeps r12, so the
/// `bx r12` after it is still a return. A helper that writes r12 doesn't.
#[test]
fn a_call_keeps_registers_its_callee_never_writes() {
    let helper = |a: &mut Asm, r12: bool| {
        a.h(0x10, 0x2800) // cmp r0, #0
            .bcond(0x12, 0, 0x16) // beq ret
            .h(0x14, if r12 { 0x4684 } else { 0x2301 }) // mov r12, r0 / movs r3, #1
            .h(0x16, 0x4770); // ret: bx lr
    };
    for r12 in [false, true] {
        let mut a = Asm::new();
        a.h(0x00, 0x46f4) // mov r12, lr
            .bl(0x02, 0x10)
            .h(0x06, 0x4760); // bx r12
        helper(&mut a, r12);
        let p = a.program_with(0x18, 0x08);
        let r = crate::vsa::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
        if r12 {
            assert_eq!(r.jumps[&(ROM_BASE + 0x06)], JumpRes::Unresolved);
        } else {
            assert!(r.jumps.is_empty(), "{:?}", r.jumps);
        }
    }
}

/// Modern GCC's narrowed index (`AnimTask_SetGrayscaleOrOriginalPal`): an
/// `s16` is zero-extended into a copy, the copy bounded, and the table
/// indexed with the original. The bound on the copy bounds the halfword,
/// and so the sign-extended original.
#[test]
fn narrowed_copy_bounds_the_index() {
    let mut a = Asm::new();
    a.h(0x00, 0x2300) // movs r3, #0
        .h(0x02, 0x5ee0) // ldrsh r0, [r4, r3]
        .h(0x04, 0x0403) // lsls r3, r0, #16
        .h(0x06, 0x0c1b) // lsrs r3, r3, #16
        .h(0x08, 0x2b03) // cmp r3, #3
        .bcond(0x0a, 8, 0x40) // bhi default
        .h(0x0c, 0x4a01) // ldr r2, [pc, #4] (the pool word)
        .h(0x0e, 0x0083) // lsls r3, r0, #2
        .h(0x10, 0x58d3) // ldr r3, [r2, r3]
        .h(0x12, 0x469f) // mov pc, r3
        .w(0x14, ROM_BASE + 0x100);
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4).h(0x40, 0x2000).h(0x42, 0x4770);
    let rec = recover(&a.program(0x44));
    let j = resolved(&rec, 0x12);
    assert_eq!(j.targets, thumb([0x30, 0x34, 0x38, 0x3c]));
    assert_eq!(j.table, word_table(0x100, 4));
}

/// `get: push {lr}; movs r1, #2; str r1, [r0]; bl nothing; pop {pc}` at
/// 0x60 and `nothing: bx lr` at 0x70.
fn getter(a: &mut Asm) {
    a.h(0x60, 0xb500)
        .h(0x62, 0x2102)
        .h(0x64, 0x6001)
        .bl(0x66, 0x70)
        .h(0x6a, 0xbd00)
        .h(0x70, 0x4770);
}

/// A table base spilled below the local whose address a call gets: the
/// call can write the local and up, not the spill slot under it.
#[test]
fn spill_below_an_escaped_local_survives_a_call() {
    let mut a = Asm::new();
    a.h(0x00, 0xb500) // push {lr}
        .h(0x02, 0xb082) // sub sp, #8
        .h(0x04, 0x4907) // ldr r1, [pc, #28] (the pool word at 0x24)
        .h(0x06, 0x9100) // str r1, [sp, #0]
        .h(0x08, 0xa801) // add r0, sp, #4
        .bl(0x0a, 0x60) // bl get
        .h(0x0e, 0x2c03) // cmp r4, #3
        .bcond(0x10, 8, 0x50) // bhi default
        .h(0x12, 0x00a4) // lsls r4, r4, #2
        .h(0x14, 0x9a00) // ldr r2, [sp, #0]
        .h(0x16, 0x5913) // ldr r3, [r2, r4]
        .h(0x18, 0x469f) // mov pc, r3
        .w(0x24, ROM_BASE + 0x100);
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4).h(0x50, 0x2000).h(0x52, 0x4770);
    getter(&mut a);
    let p = a.funcs(0x72, true, &[(0, 0x54), (0x60, 0x6c), (0x70, 0x72)]);
    let rec = recover(&p);
    assert_eq!(
        resolved(&rec, 0x18).targets,
        thumb([0x30, 0x34, 0x38, 0x3c])
    );
}

/// The lr a prologue saved survives a call that gets a local's address,
/// so `pop {r3}; bx r3` is a return, not an unresolved jump.
#[test]
fn saved_lr_survives_an_escaped_local() {
    let mut a = Asm::new();
    a.h(0x00, 0xb510) // push {r4, lr}
        .h(0x02, 0xb081) // sub sp, #4
        .h(0x04, 0x4668) // mov r0, sp
        .bl(0x06, 0x60) // bl get
        .h(0x0a, 0xb001) // add sp, #4
        .h(0x0c, 0xbc10) // pop {r4}
        .h(0x0e, 0xbc08) // pop {r3}
        .h(0x10, 0x4718); // bx r3
    getter(&mut a);
    let p = a.funcs(0x72, true, &[(0, 0x12), (0x60, 0x6c), (0x70, 0x72)]);
    let rec = recover(&p);
    assert_eq!(rec.jumps.get(&(ROM_BASE + 0x10)), None);
}

/// A local a call may have written, bounded and then loaded again: both
/// loads read the same unknown, so the bound holds at the second.
#[test]
fn local_reloaded_after_its_bounds_check() {
    let mut a = Asm::new();
    a.h(0x00, 0xb500) // push {lr}
        .h(0x02, 0xb081) // sub sp, #4
        .h(0x04, 0x4668) // mov r0, sp
        .bl(0x06, 0x60) // bl get
        .h(0x0a, 0x9b00) // ldr r3, [sp, #0]
        .h(0x0c, 0x2b03) // cmp r3, #3
        .bcond(0x0e, 8, 0x50) // bhi default
        .h(0x10, 0x9b00) // ldr r3, [sp, #0]
        .h(0x12, 0x4a02) // ldr r2, [pc, #8] (the pool word at 0x1c)
        .h(0x14, 0x009b) // lsls r3, r3, #2
        .h(0x16, 0x58d3) // ldr r3, [r2, r3]
        .h(0x18, 0x469f) // mov pc, r3
        .w(0x1c, ROM_BASE + 0x100);
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4).h(0x50, 0x2000).h(0x52, 0x4770);
    getter(&mut a);
    let p = a.funcs(0x72, true, &[(0, 0x54), (0x60, 0x6c), (0x70, 0x72)]);
    let rec = recover(&p);
    assert_eq!(
        resolved(&rec, 0x18).targets,
        thumb([0x30, 0x34, 0x38, 0x3c])
    );
}

/// Modern GCC narrows a byte with `lsls r2, r0, #24; lsrs r3, r2, #24`,
/// bounds r3 and indexes with `lsrs r3, r2, #22`: r3 times 4, as r2's low
/// 24 bits are clear. The bound on r3 bounds the index.
#[test]
fn bound_on_a_narrower_shift_bounds_the_index() {
    let mut a = Asm::new();
    a.h(0x00, 0x0602) // lsls r2, r0, #24
        .h(0x02, 0x0e13) // lsrs r3, r2, #24
        .h(0x04, 0x2b03) // cmp r3, #3
        .bcond(0x06, 8, 0x40) // bhi default
        .h(0x08, 0x0d93) // lsrs r3, r2, #22
        .h(0x0a, 0x4a01) // ldr r2, [pc, #4] (the pool word)
        .h(0x0c, 0x58d3) // ldr r3, [r2, r3]
        .h(0x0e, 0x469f) // mov pc, r3
        .w(0x10, ROM_BASE + 0x100);
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4).h(0x40, 0x2000).h(0x42, 0x4770);
    let rec = recover(&a.program(0x44));
    let j = resolved(&rec, 0x0e);
    assert_eq!(j.targets, thumb([0x30, 0x34, 0x38, 0x3c]));
    assert_eq!(j.table, word_table(0x100, 4));
}

/// Modern GCC takes the entry's address, `r9 = table + 4 * r7`, before a
/// loop, and bounds r7 inside it, just before `ldr r2, [r9]; mov pc, r2`:
/// the address is derived from r7 plus a constant, so the bound reaches
/// it.
#[test]
fn entry_address_made_before_the_bound() {
    let mut a = Asm::new();
    a.h(0x00, 0x0401) // lsls r1, r0, #16
        .h(0x02, 0x0c0f) // lsrs r7, r1, #16
        .h(0x04, 0x4b05) // ldr r3, [pc, #20] (the pool word at 0x1c)
        .h(0x06, 0x00ba) // lsls r2, r7, #2
        .h(0x08, 0x18d3) // adds r3, r2, r3
        .h(0x0a, 0x4699) // mov r9, r3
        .bl(0x0c, 0x60) // loop: bl get
        .h(0x10, 0x2f03) // cmp r7, #3
        .bcond(0x12, 8, 0x0c) // bhi loop
        .h(0x14, 0x464a) // mov r2, r9
        .h(0x16, 0x6812) // ldr r2, [r2, #0]
        .h(0x18, 0x4697) // mov pc, r2
        .w(0x1c, ROM_BASE + 0x100);
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4);
    getter(&mut a);
    let p = a.funcs(0x72, true, &[(0, 0x54), (0x60, 0x6c), (0x70, 0x72)]);
    let j = resolved(&recover(&p), 0x18).clone();
    assert_eq!(j.targets, thumb([0x30, 0x34, 0x38, 0x3c]));
    assert_eq!(j.table, word_table(0x100, 4));
}

/// An `ldrsh` index joined from three paths (two loads and a constant),
/// bounded through its zero-extended copy: each path's value is the
/// sign extension of a halfword, so the joined one is too, and the bound
/// on the copy bounds it.
#[test]
fn sign_extended_index_joined_from_paths() {
    // Path 3 is `movs r2, #0`, or, with `asr`, the halfword after the
    // first load plus one, narrowed with `lsls; asrs` (another sign
    // extension).
    for asr in [false, true] {
        let mut a = Asm::new();
        a.h(0x00, 0x2800) // cmp r0, #0
            .bcond(0x02, 0, 0x10) // beq path 2
            .h(0x04, 0x2801) // cmp r0, #1
            .bcond(0x06, 0, 0x50) // beq path 3
            .h(0x08, 0x2300) // movs r3, #0
            .h(0x0a, 0x5ee2) // ldrsh r2, [r4, r3]
            .b(0x0c, 0x18)
            .h(0x0e, 0x46c0) // nop
            .h(0x10, 0x2302) // path 2: movs r3, #2
            .h(0x12, 0x5ee2) // ldrsh r2, [r4, r3]
            .b(0x14, 0x18)
            .h(0x16, 0x46c0) // nop
            .h(0x18, 0x0411) // lsls r1, r2, #16
            .h(0x1a, 0x0c09) // lsrs r1, r1, #16
            .h(0x1c, 0x2903) // cmp r1, #3
            .bcond(0x1e, 8, 0x40) // bhi default
            .h(0x20, 0x4901) // ldr r1, [pc, #4] (the pool word at 0x28)
            .h(0x22, 0x0092) // lsls r2, r2, #2
            .h(0x24, 0x588a) // ldr r2, [r1, r2]
            .h(0x26, 0x4697) // mov pc, r2
            .w(0x28, ROM_BASE + 0x100);
        if asr {
            a.h(0x50, 0x2300) // path 3: movs r3, #0
                .h(0x52, 0x5ee2) // ldrsh r2, [r4, r3]
                .h(0x54, 0x3201) // adds r2, #1
                .h(0x56, 0x0412) // lsls r2, r2, #16
                .h(0x58, 0x1412) // asrs r2, r2, #16
                .b(0x5a, 0x18);
        } else {
            a.h(0x50, 0x2200) // path 3: movs r2, #0
                .b(0x52, 0x18);
        }
        for k in 0..4 {
            a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
        }
        a.cases(0x30, 4).h(0x40, 0x2000).h(0x42, 0x4770);
        let rec = recover(&a.program(0x5c));
        let j = resolved(&rec, 0x26);
        assert_eq!(j.targets, thumb([0x30, 0x34, 0x38, 0x3c]), "asr {asr}");
    }
}

/// A local array indexed by a register (`add r3, sp, #4; strb r5, [r3,
/// r6]`, or the sum made first with a flag-setting `adds r3, r3, r6`)
/// lets a call write the array and up, not the table base spilled below
/// it.
#[test]
fn spill_below_an_indexed_local_survives_a_call() {
    for adds in [false, true] {
        let mut a = Asm::new();
        a.h(0x00, 0xb500) // push {lr}
            .h(0x02, 0xb082) // sub sp, #8
            .h(0x04, 0x4907) // ldr r1, [pc, #28] (the pool word at 0x24)
            .h(0x06, 0x9100) // str r1, [sp, #0]
            .h(0x08, 0xab01); // add r3, sp, #4
        if adds {
            a.h(0x0a, 0x199b); // adds r3, r3, r6
        } else {
            a.h(0x0a, 0x559d); // strb r5, [r3, r6]
        }
        a.bl(0x0c, 0x60) // bl get
            .h(0x10, 0x2c03) // cmp r4, #3
            .bcond(0x12, 8, 0x50) // bhi default
            .h(0x14, 0x00a4) // lsls r4, r4, #2
            .h(0x16, 0x9a00) // ldr r2, [sp, #0]
            .h(0x18, 0x5913) // ldr r3, [r2, r4]
            .h(0x1a, 0x469f) // mov pc, r3
            .w(0x24, ROM_BASE + 0x100);
        for k in 0..4 {
            a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
        }
        a.cases(0x30, 4).h(0x50, 0x2000).h(0x52, 0x4770);
        getter(&mut a);
        let p = a.funcs(0x72, true, &[(0, 0x54), (0x60, 0x6c), (0x70, 0x72)]);
        assert_eq!(
            resolved(&recover(&p), 0x1a).targets,
            thumb([0x30, 0x34, 0x38, 0x3c]),
            "adds {adds}"
        );
    }
}

/// A continuation entry inside a function, which its cases far-jump back
/// to (`bl`), analysed as a root of its own: it starts without the table
/// base the function spilled to the stack. The function's analysis runs
/// through it and resolves the jump, so the entry's doesn't count.
#[test]
fn far_jump_entry_yields_to_its_function() {
    let mut a = Asm::new();
    a.h(0x00, 0xb500) // push {lr}
        .h(0x02, 0xb082) // sub sp, #8
        .h(0x04, 0x4904) // ldr r1, [pc, #16] (the pool word at 0x18)
        .h(0x06, 0x9101) // str r1, [sp, #4]
        .h(0x08, 0x7802) // entry: ldrb r2, [r0, #0]
        .h(0x0a, 0x2a03) // cmp r2, #3
        .bcond(0x0c, 8, 0x08) // bhi entry
        .h(0x0e, 0x9b01) // ldr r3, [sp, #4]
        .h(0x10, 0x0091) // lsls r1, r2, #2
        .h(0x12, 0x5859) // ldr r1, [r3, r1]
        .h(0x14, 0x468f) // mov pc, r1
        .w(0x18, ROM_BASE + 0x100);
    for k in 0..3 {
        a.bl(0x30 + 4 * k, 0x08); // case k: bl entry (a far jump)
    }
    a.h(0x3c, 0xb002) // case 3: add sp, #8
        .h(0x3e, 0xbd00); // pop {pc}
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    let p = a.program(0x40);
    let (f, entry) = ((ROM_BASE, true), (ROM_BASE + 0x08, true));
    let mut e = Engine::default();
    e.run(&p, [f, entry]);
    assert_eq!(
        e.results[&entry].jumps[&(ROM_BASE + 0x14)],
        JumpRes::Unresolved
    );
    let rec = e.collect([f, entry]);
    assert_eq!(
        resolved(&rec, 0x14).targets,
        thumb([0x30, 0x34, 0x38, 0x3c])
    );
}

/// A switch whose cases come back past the bounds check, each with its
/// own constant index (modern GCC's state machines): the merge point
/// they come back to is joined into many times, but no one loop edge
/// more than [`crate::vsa::func::WIDEN_AFTER`], so the index isn't
/// widened there.
#[test]
fn cases_back_to_the_jump_with_constants() {
    let mut a = Asm::new();
    a.h(0x00, 0x2807) // cmp r0, #7
        .bcond(0x02, 8, 0x60) // bhi default
        .h(0x04, 0x0003) // movs r3, r0
        .h(0x06, 0x4a02) // head: ldr r2, [pc, #8] (the pool word at 0x10)
        .h(0x08, 0x0099) // lsls r1, r3, #2
        .h(0x0a, 0x5851) // ldr r1, [r2, r1]
        .h(0x0c, 0x468f) // mov pc, r1
        .h(0x0e, 0x46c0) // nop
        .w(0x10, ROM_BASE + 0x100);
    for k in 0..8 {
        // case k: movs r3, #(k + 1) % 8; b head
        a.h(0x20 + 4 * k, 0x2300 | ((k as u16 + 1) & 7))
            .b(0x22 + 4 * k, 0x06);
        a.w(0x100 + 4 * k, ROM_BASE + 0x20 + 4 * k);
    }
    a.h(0x60, 0x4770); // default: bx lr
    let j = resolved(&recover(&a.program(0x62)), 0x0c).clone();
    assert_eq!(j.targets, thumb((0..8).map(|k| 0x20 + 4 * k)));
}

/// A loop counter that has settled isn't widened while another register
/// still grows: the widening is of sets the join grows. The jump at the
/// head indexes with the counter, bounded only where the cases come back.
#[test]
fn settled_counter_survives_widening() {
    let mut a = Asm::new();
    a.h(0x00, 0x2300) // movs r3, #0
        .h(0x02, 0x2201) // movs r2, #1
        .h(0x04, 0x4902) // head: ldr r1, [pc, #8] (the pool word at 0x10)
        .h(0x06, 0x0098) // lsls r0, r3, #2
        .h(0x08, 0x5808) // ldr r0, [r1, r0]
        .h(0x0a, 0x4687) // mov pc, r0
        .w(0x10, ROM_BASE + 0x100)
        .h(0x30, 0x0052) // every case: lsls r2, r2, #1 (grows)
        .h(0x32, 0x3301) // adds r3, #1
        .h(0x34, 0x2b03) // cmp r3, #3
        .bcond(0x36, 9, 0x04) // bls head
        .h(0x38, 0x4770); // bx lr
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30);
    }
    let j = resolved(&recover(&a.program(0x3a)), 0x0a).clone();
    assert_eq!(j.targets, thumb([0x30]));
    assert_eq!(j.table, word_table(0x100, 4));
}

/// A frame address only held in a register (modern GCC keeps `buf - 33`
/// in r5 for a loop that indexes it from 33) hasn't escaped: a call that
/// isn't passed it can't write the frame.
#[test]
fn frame_address_in_a_register_does_not_escape() {
    let mut a = Asm::new();
    a.h(0x00, 0xb500) // push {lr}
        .h(0x02, 0xb082) // sub sp, #8
        .h(0x04, 0x4907) // ldr r1, [pc, #28] (the pool word at 0x24)
        .h(0x06, 0x9100) // str r1, [sp, #0]
        .h(0x08, 0x466d) // mov r5, sp
        .h(0x0a, 0x46a8) // mov r8, r5
        .bl(0x0c, 0x60) // bl get
        .h(0x10, 0x2c03) // cmp r4, #3
        .bcond(0x12, 8, 0x50) // bhi default
        .h(0x14, 0x00a4) // lsls r4, r4, #2
        .h(0x16, 0x9a00) // ldr r2, [sp, #0]
        .h(0x18, 0x5913) // ldr r3, [r2, r4]
        .h(0x1a, 0x469f) // mov pc, r3
        .w(0x24, ROM_BASE + 0x100);
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4).h(0x50, 0x2000).h(0x52, 0x4770);
    getter(&mut a);
    let p = a.funcs(0x72, true, &[(0, 0x54), (0x60, 0x6c), (0x70, 0x72)]);
    assert_eq!(
        resolved(&recover(&p), 0x1a).targets,
        thumb([0x30, 0x34, 0x38, 0x3c])
    );
}

/// But one joined with another value is lost, and escapes: the call
/// after the join may be passed it (`get(c ? &base : 0)` writes the slot
/// the table base was spilled to).
#[test]
fn frame_address_lost_at_a_join_escapes() {
    let mut a = Asm::new();
    a.h(0x00, 0xb500) // push {lr}
        .h(0x02, 0xb082) // sub sp, #8
        .h(0x04, 0x4907) // ldr r1, [pc, #28] (the pool word at 0x24)
        .h(0x06, 0x9100) // str r1, [sp, #0]
        .h(0x08, 0x4668) // mov r0, sp
        .h(0x0a, 0x2c09) // cmp r4, #9
        .bcond(0x0c, 1, 0x10) // bne 1f
        .h(0x0e, 0x2000) // movs r0, #0
        .bl(0x10, 0x60) // 1: bl get
        .h(0x14, 0x2c03) // cmp r4, #3
        .bcond(0x16, 8, 0x50) // bhi default
        .h(0x18, 0x00a4) // lsls r4, r4, #2
        .h(0x1a, 0x9a00) // ldr r2, [sp, #0]
        .h(0x1c, 0x5913) // ldr r3, [r2, r4]
        .h(0x1e, 0x469f) // mov pc, r3
        .w(0x24, ROM_BASE + 0x100);
    for k in 0..4 {
        a.w(0x100 + 4 * k, ROM_BASE + 0x30 + 4 * k);
    }
    a.cases(0x30, 4).h(0x50, 0x2000).h(0x52, 0x4770);
    getter(&mut a);
    let p = a.funcs(0x72, true, &[(0, 0x54), (0x60, 0x6c), (0x70, 0x72)]);
    assert_eq!(rec_jump(&recover(&p), 0x1e), Some(&JumpRes::Unresolved));
}

fn rec_jump(rec: &Recovery, at: u32) -> Option<&JumpRes> {
    rec.jumps.get(&(ROM_BASE + at))
}
