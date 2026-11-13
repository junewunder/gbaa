//! Block walks and lowering on small hand-assembled programs, with
//! discovery's output written by hand.

use crate::emit::{build_all, pretty};
use crate::func::{Ctx, Idle, Insn, Plan, build_func, exits};
use crate::lower::{lower_func, lower_func_checked};
use analysis::discovery::{Code, Discovery, Evidence};
use analysis::program::ROM_BASE;
use analysis::vsa::func::{Jump, JumpRes};

/// ARM words at the ROM's start, all marked as code, with these function
/// entries.
fn program(words: &[u32], funcs: &[u32]) -> (Vec<u8>, Discovery) {
    let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut d = Discovery::default();
    for i in 0..words.len() as u32 {
        let code = Code {
            size: 4,
            thumb: false,
            evidence: Evidence::Confirmed,
        };
        d.code.insert(ROM_BASE + 4 * i, code);
    }
    for &f in funcs {
        d.funcs.insert(f, Evidence::Confirmed);
    }
    (rom, d)
}

/// `bl f; blne f; b .; nop; f: add r0, r0, #1; bx lr`.
fn calls() -> (Vec<u8>, Discovery) {
    calls_then(0xEAFF_FFFE)
}

/// [`calls`] with `last` in place of its `b .`.
fn calls_then(last: u32) -> (Vec<u8>, Discovery) {
    program(
        &[
            0xEB00_0002, // 0x08000000 bl 0x08000010
            0x1B00_0001, // 0x08000004 blne 0x08000010
            last,        // 0x08000008 b . (or `last`)
            0xE1A0_0000, // 0x0800000C nop
            0xE280_0001, // 0x08000010 add r0, r0, #1
            0xE12F_FF1E, // 0x08000014 bx lr
        ],
        &[0x0800_0000, 0x0800_0010],
    )
}

#[test]
fn calls_return_sites_and_splits() {
    let (rom, d) = calls();
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    // The guarded call's return site splits the second block.
    let starts: Vec<u32> = f.blocks.keys().copied().collect();
    assert_eq!(starts, [0x0800_0000, 0x0800_0004, 0x0800_0008]);
    let plans =
        |k: u32| -> Vec<Plan> { exits(&ctx, &f.blocks[&k]).iter().map(|x| x.plan).collect() };
    let call = |ret| Plan::Call {
        target: 0x0800_0010,
        ret,
    };
    assert_eq!(plans(0x0800_0000), [call(0x0800_0004)]);
    assert_eq!(
        plans(0x0800_0004),
        [call(0x0800_0008), Plan::Static(0x0800_0008)]
    );
    assert_eq!(plans(0x0800_0008), [Plan::Static(0x0800_0008)]);

    let callee = build_func(&ctx, 0x0800_0010);
    assert_eq!(callee.blocks.len(), 1);
    let x = exits(&ctx, &callee.blocks[&0x0800_0010]);
    assert_eq!(
        x[0].plan,
        Plan::Dynamic {
            ret: None,
            target: None,
            local: false
        }
    );
}

#[test]
fn lowering_parses_and_calls_by_name() {
    // The caller returns (`bx lr` for its `b .`), so it is structured: one
    // that never returns stays v1.
    let (rom, d) = calls_then(0xE12F_FF1E);
    let ctx = Ctx::new(&rom, &d);
    let r = build_all(&ctx);
    assert!(
        r.uncovered.iter().all(|&(a, _)| a == 0x0800_000C),
        "{:x?}",
        r.uncovered
    );
    // The two entries are keys; the caller's other blocks, structured,
    // aren't.
    assert_eq!(r.table.len(), 2);
    assert_eq!(r.inner.len(), 2);
    let text = pretty(lower_func(&ctx, &r.funcs[0]));
    assert_eq!(
        text.matches("f_08000010(m, 0x0800_0010)").count(),
        2,
        "{text}"
    );
    // Every arm ends by continuing or returning; the fallback returns.
    assert!(text.contains("_ => return Flow::Jump(pc)"), "{text}");
    // The callee's `bx lr` goes through apply_exit.
    let text = pretty(lower_func(&ctx, &r.funcs[1]));
    assert!(
        text.contains("rt::exit(m, 0x0800_0014, false, Exit::Exchange(v"),
        "{text}"
    );
}

#[test]
fn misaligned_arm_target_keeps_its_low_bits() {
    // `ldr r1, [pc], #1`: the writeback makes pc a constant (odd),
    // which the interpreter keeps unaligned (SingleStepTests has these).
    let (rom, d) = program(&[0xE49F_1001], &[0x0800_0000]);
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    let x = exits(&ctx, &f.blocks[&0x0800_0000]);
    assert_eq!(
        x[0].plan,
        Plan::Dynamic {
            ret: None,
            target: Some(0x0800_000C), // the key is aligned; cpu.pc isn't
            local: true
        }
    );
}

#[test]
fn blocks_stop_at_other_functions() {
    // `nop; nop` with a function entry at the second: the first block
    // falls through out of its function.
    let (rom, d) = program(&[0xE1A0_0000, 0xE1A0_0000], &[0x0800_0000, 0x0800_0004]);
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    assert_eq!(f.blocks.len(), 1);
    let x = exits(&ctx, &f.blocks[&0x0800_0000]);
    assert_eq!(x[0].plan, Plan::Static(0x0800_0004));
}

/// Thumb halfwords at the ROM's start. `sizes` gives each instruction's
/// size in bytes (4 for a `bl` pair).
fn thumb_program(halves: &[u16], sizes: &[u32], funcs: &[u32]) -> (Vec<u8>, Discovery) {
    let rom: Vec<u8> = halves.iter().flat_map(|h| h.to_le_bytes()).collect();
    let mut d = Discovery::default();
    let mut a = ROM_BASE;
    for &size in sizes {
        let code = Code {
            size,
            thumb: true,
            evidence: Evidence::Confirmed,
        };
        d.code.insert(a, code);
        a += size;
    }
    for &f in funcs {
        d.funcs.insert(f, Evidence::Confirmed);
    }
    (rom, d)
}

/// `bl f; b .; nop; f: bx lr`, in Thumb.
fn thumb_calls() -> (Vec<u8>, Discovery) {
    thumb_program(
        &[0xF000, 0xF802, 0xE7FE, 0x46C0, 0x4770],
        &[4, 2, 2, 2],
        &[0x0800_0001, 0x0800_0009],
    )
}

#[test]
fn thumb_bl_pair_is_a_call() {
    let (rom, d) = thumb_calls();
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0001);
    let x = exits(&ctx, &f.blocks[&0x0800_0001]);
    assert_eq!(
        x[0].plan,
        Plan::Call {
            target: 0x0800_0009,
            ret: 0x0800_0005
        }
    );
    // Both halves ran through the builder.
    assert_eq!(f.blocks[&0x0800_0001].insns.len(), 2);
}

#[test]
fn table_under_a_bl_pairs_first_half() {
    // Discovery records a jump a `bl` into a case helper makes under the
    // pair's first half; its targets are blocks of the caller.
    let (rom, mut d) = thumb_calls();
    let jump = Jump {
        targets: vec![0x0800_0007],
        table: None,
    };
    d.rec.jumps.insert(0x0800_0000, JumpRes::Resolved(jump));
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0001);
    assert!(
        f.blocks.contains_key(&0x0800_0007),
        "{:x?}",
        f.blocks.keys()
    );
}

#[test]
fn holes_file_targets_follow_an_unresolved_jump() {
    // `mov pc, r0` Recovery left unresolved; the holes file's targets
    // (`[[jump]]`, Ctx::with_jumps) are its cases, as a table's would be.
    let (rom, mut d) = program(
        &[
            0xE1A0_F000, // 0x08000000 mov pc, r0
            0xE280_0001, // 0x08000004 add r0, r0, #1
            0xE12F_FF1E, // 0x08000008 bx lr
            0xE12F_FF1E, // 0x0800000C bx lr
        ],
        &[0x0800_0000],
    );
    d.rec.jumps.insert(0x0800_0000, JumpRes::Unresolved);
    let ctx = Ctx::new(&rom, &d);
    assert_eq!(ctx.table_targets(0x0800_0000, false), None);
    let f = build_func(&ctx, 0x0800_0000);
    assert_eq!(f.blocks.keys().copied().collect::<Vec<_>>(), [0x0800_0000]);
    let given = [0x0800_0004, 0x0800_000C];
    let ctx = Ctx::new(&rom, &d).with_jumps([(0x0800_0000, &given[..])]);
    assert_eq!(ctx.table_targets(0x0800_0000, false), Some(given.to_vec()));
    let f = build_func(&ctx, 0x0800_0000);
    let starts: Vec<u32> = f.blocks.keys().copied().collect();
    assert_eq!(starts, [0x0800_0000, 0x0800_0004, 0x0800_000C]);
}

#[test]
fn entry_without_code_in_its_mode_is_skipped() {
    // ARM code, with a Thumb root at the same address.
    let (rom, d) = program(&[0xE1A0_0000], &[0x0800_0000, 0x0800_0001]);
    let ctx = Ctx::new(&rom, &d);
    assert_eq!(
        ctx.entries.iter().copied().collect::<Vec<_>>(),
        [0x0800_0000]
    );
}

#[test]
fn msr_control_rekeys_the_fallthrough() {
    // `msr cpsr_c, r0; nop`: the T bit may have changed.
    let (rom, d) = program(&[0xE121_F000, 0xE1A0_0000], &[0x0800_0000]);
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    let x = exits(&ctx, &f.blocks[&0x0800_0000]);
    // The block ends after the `msr`; the `nop` may be Thumb now.
    assert_eq!(x[0].plan, Plan::Rekey(0x0800_0004));
    assert_eq!(f.blocks[&0x0800_0000].insns.len(), 1);
    let text = pretty(lower_func(&ctx, &f));
    assert!(text.contains("pc = rt::rekey(m, 0x0800_0004);"), "{text}");
}

#[test]
fn a_store_to_ime_ends_a_native_block() {
    // `ldr r1, =0x0400_0208; strh r0, [r1]; nop; bx lr`, the literal after.
    let words = [0xE59F_1008, 0xE1C1_00B0, 0xE1A0_0000, 0xE12F_FF1E];
    let (mut rom, d) = program(&words, &[0x0800_0000]);
    rom.extend(0x0400_0208u32.to_le_bytes());
    let mut ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    assert_eq!(f.blocks[&0x0800_0000].insns.len(), 4);
    // Under native timing it ends after the store, so the tick that takes
    // the IRQ it lets through is the interpreter's (native/ime-release).
    ctx.cfg.clock = crate::func::Clock::Native;
    let f = build_func(&ctx, 0x0800_0000);
    assert_eq!(f.blocks[&0x0800_0000].insns.len(), 2);
    assert_eq!(f.blocks[&0x0800_0008].insns.len(), 2);
}

#[test]
fn a_conditional_instructions_effects_share_one_if() {
    // `movseq r0, r1; bx lr`: N, Z, C and r0 under one guard, the flags
    // as one CPSR write (C rewritten with its old value: `mov` with no
    // shift keeps it).
    let (rom, d) = program(&[0x01B0_0001, 0xE12F_FF1E], &[0x0800_0000]);
    let ctx = Ctx::new(&rom, &d);
    let text = pretty(lower_func(&ctx, &build_func(&ctx, 0x0800_0000)));
    let body = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        body.contains(
            "m.cpu.cpsr = m.cpu.cpsr & 0x1fff_ffff | (((v3 >> 31) & 1 != 0) as u32) << 31 \
             | ((v3 == 0) as u32) << 30 | (m.cpu.flag(Flag::C) as u32) << 29; m.cpu.r[0] = v3; }"
        ),
        "{text}"
    );
    assert_eq!(body.matches("if v").count(), 1, "{text}");
    assert!(!body.contains("set_flag"), "{text}");
}

#[test]
fn crate_names() {
    use crate::emit::crate_name;
    assert_eq!(
        crate_name("Pokemon - Emerald Version (U)"),
        "pokemon-emerald-version-u"
    );
    assert_eq!(
        crate_name("WarioWare, Inc. - Mega Microgame$!"),
        "warioware-inc-mega-microgame"
    );
    assert_eq!(crate_name("$$$"), "game");
}

#[test]
fn call_to_a_non_function_keeps_its_return_site() {
    // `bl stub; nop; b .; stub: bx r3`, with no function at the stub (as
    // discovery treats a `bl` to a `bx rN` stub).
    let (rom, d) = thumb_program(
        &[0xF000, 0xF802, 0x46C0, 0xE7FE, 0x4718],
        &[4, 2, 2, 2],
        &[0x0800_0001],
    );
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0001);
    assert!(
        f.blocks.contains_key(&0x0800_0005),
        "{:x?}",
        f.blocks.keys()
    );
}

#[test]
fn swi_returns_when_the_machine_must_yield() {
    // `swi 0x05` (VBlankIntrWait) then `b .`.
    let (rom, d) = program(&[0xEF05_0000, 0xEAFF_FFFE], &[0x0800_0000]);
    let ctx = Ctx::new(&rom, &d);
    let text = pretty(lower_func(&ctx, &build_func(&ctx, 0x0800_0000)));
    let at = text
        .find("pc = rt::swi(m, 0x0800_0000, false, 0x0005_0000);")
        .expect(&text);
    let tail = &text[at..];
    assert!(tail.contains("|| rt::yielding(m)"), "{text}");
    // The halt is taken there, unwinding only when it must.
    assert!(
        tail.contains("if let Some(k) = yield_at(m, pc) {"),
        "{text}"
    );
}

#[test]
fn ticks_take_irqs_in_place() {
    // `add r0, r0, #1; b .-4; bx lr`: every tick, the loop's and the
    // return's, takes an IRQ where it is and never returns `pc` itself.
    let (rom, d) = program(&[0xE280_0001, 0xEAFF_FFFD, 0xE12F_FF1E], &[0x0800_0000]);
    let ctx = Ctx::new(&rom, &d);
    let text = pretty(lower_func(&ctx, &build_func(&ctx, 0x0800_0000)));
    let ticks = text.matches("if rt::tick(m, cycles) {").count();
    assert!(ticks >= 1, "{text}");
    assert_eq!(text.matches("yield_at(m, pc)").count(), ticks, "{text}");
    assert!(!text.contains("let _ = rt::tick"), "{text}");
}

#[test]
fn call_through_a_stub_calls_the_register() {
    // `bl stub; nop; b .; stub: bx r3`: an indirect call through r3 that
    // keeps its return site, not a jump to the stub.
    let (rom, d) = thumb_program(
        &[0xF000, 0xF802, 0x46C0, 0xE7FE, 0x4718],
        &[4, 2, 2, 2],
        &[0x0800_0001],
    );
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0001);
    let plans: Vec<Plan> = exits(&ctx, &f.blocks[&0x0800_0001])
        .iter()
        .map(|x| x.plan)
        .collect();
    assert!(
        matches!(
            plans[..],
            [Plan::CallVia {
                stub: 0x0800_0009,
                reg: armv4t::sem::Reg(3),
                ret: 0x0800_0005,
                ..
            }]
        ),
        "{plans:x?}"
    );
    let text = pretty(lower_func(&ctx, &f));
    let at = text
        .find("pc = rt::exit(m, 0x0800_0008, true, Exit::Exchange(t));")
        .expect(&text);
    assert!(
        text[at..].contains("let Flow::Jump(a) = call(m, pc);"),
        "{text}"
    );
    // Coming back anywhere else, it runs on to the return site in place.
    assert!(
        text[at..].contains("pc = settle(m, a, 0x0800_0005, sp0);"),
        "{text}"
    );
    assert!(!text.contains("0x0800_0009 =>"), "{text}");
}

/// ARM `add r0, r0, #1; b 1f; .word 0; 1: bx lr`, copied from ROM 0x0800_0100
/// to IWRAM.
fn iwram_copy() -> (Vec<u8>, analysis::discovery::Overlay) {
    let words: [u32; 4] = [0xE280_0001, 0xEA00_0000, 0, 0xE12F_FF1E];
    let image: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut d = Discovery::default();
    for a in [0x0300_0000, 0x0300_0004, 0x0300_000C] {
        let code = Code {
            size: 4,
            thumb: false,
            evidence: Evidence::Confirmed,
        };
        d.code.insert(a, code);
    }
    d.funcs.insert(0x0300_0000, Evidence::Confirmed);
    let copy = analysis::discovery::Overlay {
        src: 0x0800_0100,
        dst: 0x0300_0000,
        len: 16,
        disc: d,
    };
    (image, copy)
}

#[test]
fn overlay_is_built_where_it_runs_and_guarded() {
    let (image, copy) = iwram_copy();
    let ctx = Ctx::at(copy.dst, &image, &copy.disc);
    let mut f = build_func(&ctx, 0x0300_0000);
    let keys: Vec<u32> = f.blocks.keys().copied().collect();
    assert_eq!(keys, [0x0300_0000, 0x0300_000C]);
    // Each block's guard covers its instructions, not the word between.
    assert_eq!(
        crate::emit::guard_ranges(&copy, &f, 0x0300_0000),
        [(0x0300_0000, 8, 0x0800_0100)]
    );
    assert_eq!(
        crate::emit::guard_ranges(&copy, &f, 0x0300_000C),
        [(0x0300_000C, 4, 0x0800_010C)]
    );
    f.overlay = Some(3);
    let text = pretty(lower_func(&ctx, &f));
    // Each block checks its own guard before it runs.
    let first = text.find("0x0300_0000 => {").expect(&text);
    let guard = text.find("if rt::overlay_valid(m, 3) {").expect(&text);
    let second = text.find("0x0300_000c => {").expect(&text);
    let guard2 = text.find("if rt::overlay_valid(m, 4) {").expect(&text);
    assert!(first < guard && guard < second && second < guard2, "{text}");
    assert!(text.contains("rt::tick_ram(m, "), "{text}");
    assert!(!text.contains("rt::tick(m, "), "{text}");
}

#[test]
fn an_overlays_guard_covers_the_stubs_it_calls_through() {
    // Thumb in IWRAM: `bl stub; bx lr; stub: bx r3`.
    let halves: [u16; 4] = [0xF000, 0xF801, 0x4770, 0x4718];
    let image: Vec<u8> = halves.iter().flat_map(|h| h.to_le_bytes()).collect();
    let mut d = Discovery::default();
    for (a, size) in [(0x0300_0000, 4), (0x0300_0004, 2), (0x0300_0006, 2)] {
        let code = Code {
            size,
            thumb: true,
            evidence: Evidence::Confirmed,
        };
        d.code.insert(a, code);
    }
    d.funcs.insert(0x0300_0001, Evidence::Confirmed);
    let copy = analysis::discovery::Overlay {
        src: 0x0800_0200,
        dst: 0x0300_0000,
        len: 8,
        disc: d,
    };
    let ctx = Ctx::at(copy.dst, &image, &copy.disc);
    let f = build_func(&ctx, 0x0300_0001);
    assert!(
        !f.blocks.contains_key(&0x0300_0007),
        "{:x?}",
        f.blocks.keys()
    );
    assert_eq!(f.stubs.iter().copied().collect::<Vec<_>>(), [0x0300_0007]);
    assert_eq!(
        crate::emit::guard_ranges(&copy, &f, 0x0300_0001),
        [(0x0300_0000, 4, 0x0800_0200), (0x0300_0006, 2, 0x0800_0206)]
    );
}

#[test]
fn a_patched_version_of_a_block_is_its_own_overlay() {
    let (image, copy) = iwram_copy();
    let ctx = Ctx::at(copy.dst, &image, &copy.disc);
    let mut f = build_func(&ctx, 0x0300_0000);
    // The code rewrote `add r0, r0, #1` to `add r0, r0, #2`.
    let mut patched = image.clone();
    patched[..4].copy_from_slice(&0xE280_0002u32.to_le_bytes());
    let pctx = Ctx::at(copy.dst, &patched, &copy.disc);
    let block = build_func(&pctx, 0x0300_0000).blocks[&0x0300_0000].clone();
    let bytes = patched[..8].to_vec();
    let v = crate::func::Variant { block, bytes };
    f.variants.insert(0x0300_0000, vec![v]);
    f.overlay = Some(3);
    assert_eq!((f.versions(0x0300_0000), f.overlays()), (2, 3));
    assert_eq!(f.block_overlay(0x0300_000C), Some(5));
    let text = pretty(lower_func(&ctx, &f));
    // Both versions of the first block, in order, then the second block's.
    let at = |s: &str| text.find(s).expect(&text);
    assert!(
        at("if rt::overlay_valid(m, 3) {") < at("if rt::overlay_valid(m, 4) {")
            && at("if rt::overlay_valid(m, 4) {") < at("0x0300_000c => {")
            && at("0x0300_000c => {") < at("if rt::overlay_valid(m, 5) {"),
        "{text}"
    );
}

#[test]
fn code_dma_copied_to_iwram_is_found_by_running_it() {
    // DMA3 copies `f` (4 words at 0x0800_0040) to IWRAM 0x0300_0100, then
    // calls it there: no BIOS copy for discovery to see.
    let words: [u32; 20] = [
        0xE59F_0020, // ldr r0, =0x040000D4 (DMA3SAD)
        0xE59F_1020, // ldr r1, =f
        0xE59F_2020, // ldr r2, =0x03000100
        0xE59F_3020, // ldr r3, =0x84000004 (enable, 32-bit, 4 words)
        0xE880_000E, // stmia r0, {r1, r2, r3}
        0xE1A0_E00F, // mov lr, pc
        0xE12F_FF12, // bx r2
        0xEAFF_FFFE, // b .
        0,
        0,
        0x0400_00D4,
        0x0800_0040,
        0x0300_0100,
        0x8400_0004,
        0,
        0,
        0xE280_0001, // f: add r0, r0, #1
        0xE280_0002, // add r0, r0, #2
        0xE281_1003, // add r1, r1, #3
        0xE12F_FF1E, // bx lr
    ];
    let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let found = crate::seen::overlays(&rom, &[], 2);
    assert_eq!(found.len(), 1);
    let o = &found[0];
    // Widened back over the zeros before `f`, which IWRAM holds too.
    assert_eq!((o.src, o.dst, o.len), (0x0800_0038, 0x0300_00F8, 0x18));
    assert!(
        o.disc.funcs.contains_key(&0x0300_0100),
        "{:x?}",
        o.disc.funcs
    );
    assert!(o.disc.code.contains_key(&0x0300_010C));
    // A copy already known isn't found again.
    assert!(crate::seen::overlays(&rom, &found, 2).is_empty());
}

/// How many `SetFlag`s the block at `k` of `f` kept.
fn flag_writes(f: &crate::func::Func, k: u32) -> usize {
    let insts = &f.blocks[&k].block.insts;
    insts
        .iter()
        .filter(|i| matches!(i.op, analysis::ir::Op::SetFlag(..)))
        .count()
}

/// `adds r0, r0, r1; <second>; nop; cmp r0, #0; bx lr`: the cmp at
/// 0x0C overwrites all four of the adds' flags.
fn adds_then(second: u32) -> Vec<u32> {
    vec![0xE090_0001, second, 0xE1A0_0000, 0xE350_0000, 0xE12F_FF1E]
}

#[test]
fn flag_liveness_follows_static_edges_only() {
    // b 0x0C: a static edge, so the adds' flags are dead.
    let (rom, d) = program(&adds_then(0xEA00_0000), &[0x0800_0000]);
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    assert_eq!(flag_writes(&f, 0x0800_0000), 0);
    assert_eq!(flag_writes(&f, 0x0800_000C), 4);
    // An IRQ taken at the branch's tick may save the CPSR, so its yield
    // path writes the four (`docs/structured-codegen-design.md`, step 2).
    let text = pretty(lower_func(&ctx, &f));
    let arm = &text[..text.find("0x0800_000c,").unwrap()];
    let tick = &arm[arm.find("if rt::tick").unwrap()..];
    let tick = &tick[..tick.find("return").unwrap()];
    assert!(
        tick.contains("m.cpu.cpsr = m.cpu.cpsr & 0xfff_ffff"),
        "{text}"
    );
    // With liveness off (`fn-diff`'s form), every flag write stays, and
    // the yield path writes none.
    let mut off = Ctx::new(&rom, &d);
    off.cfg.flag_liveness = false;
    let g = build_func(&off, 0x0800_0000);
    assert_eq!(flag_writes(&g, 0x0800_0000), 4);
    let text = pretty(lower_func(&off, &g));
    assert_eq!(text.matches("m.cpu.cpsr =").count(), 2, "{text}");

    // adds; b 0x0C; nop; b 0x14; nop; cmp r0, #0; bx lr: the block at
    // 0x0C can yield before anything overwrites the flags, and has no
    // value for them, so the adds keep all four.
    let words = [
        0xE090_0001,
        0xEA00_0000,
        0xE1A0_0000,
        0xEA00_0000,
        0xE1A0_0000,
        0xE350_0000,
        0xE12F_FF1E,
    ];
    let (rom, d) = program(&words, &[0x0800_0000]);
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    assert!(f.blocks.contains_key(&0x0800_000C));
    assert_eq!(flag_writes(&f, 0x0800_0000), 4);

    // bl 0x0C, a function: the callee may read them.
    let (rom, d) = program(&adds_then(0xEB00_0000), &[0x0800_0000, 0x0800_000C]);
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    assert!(matches!(
        exits(&ctx, &f.blocks[&0x0800_0000])[0].plan,
        Plan::Call { .. }
    ));
    assert_eq!(flag_writes(&f, 0x0800_0000), 4);

    // The same code copied to IWRAM: its blocks may not run as built.
    let image: Vec<u8> = adds_then(0xEA00_0000)
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let (_, mut d) = program(&adds_then(0xEA00_0000), &[]);
    d.code = d
        .code
        .into_iter()
        .map(|(a, c)| (a - ROM_BASE + 0x0300_0000, c))
        .collect();
    d.funcs.insert(0x0300_0000, Evidence::Confirmed);
    let ctx = Ctx::at(0x0300_0000, &image, &d);
    let f = build_func(&ctx, 0x0300_0000);
    assert!(f.blocks.contains_key(&0x0300_000C));
    assert_eq!(flag_writes(&f, 0x0300_0000), 4);
}

#[test]
fn a_rom_literal_jump_target_becomes_known() {
    // ldr r1, [pc]; bx r1; .word 0x0800000C; bx lr: const-prop reads the
    // literal, so the bx's target is known and its block is the function's.
    let (rom, mut d) = program(
        &[0xE59F_1000, 0xE12F_FF11, 0x0800_000C, 0xE12F_FF1E],
        &[0x0800_0000],
    );
    d.code.remove(&0x0800_0008);
    let ctx = Ctx::new(&rom, &d);
    let f = build_func(&ctx, 0x0800_0000);
    let x = exits(&ctx, &f.blocks[&0x0800_0000]);
    assert_eq!(
        x[0].plan,
        Plan::Dynamic {
            ret: None,
            target: Some(0x0800_000C),
            local: true
        }
    );
    assert!(f.blocks.contains_key(&0x0800_000C));
}

#[test]
fn timing_that_hangs_on_values_is_counted_at_run_time() {
    // `muleq r0, r1, r2; ldmia r3, {r4, r5}; bx lr`: the multiply's
    // operand where Z is set, its cost beyond 1S back where it isn't, and
    // the `ldm`'s two accesses.
    let (rom, d) = program(&[0x0000_0291, 0xE893_0030, 0xE12F_FF1E], &[0x0800_0000]);
    let ctx = Ctx::new(&rom, &d);
    let text = pretty(lower_func(&ctx, &build_func(&ctx, 0x0800_0000)));
    let body = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        body.contains("if v1 { rt::multiply(m, v2, true, 0); }"),
        "{text}"
    );
    assert!(
        body.contains("if !v1 { rt::skipped(m, 8, false, 1, 0, 1); }"),
        "{text}"
    );
    assert!(body.contains("rt::block(m, 2);"), "{text}");
    // The count comes before the loads it counts.
    assert!(
        body.find("rt::block").unwrap() < body.find("m.bus.load").unwrap(),
        "{text}"
    );
}

#[test]
fn locals_pass_their_check_and_a_missing_write_back_fails_it() {
    use crate::locals::{Locals, check};
    use crate::lower::{fn_name, lower_func_checked};
    // A caller (calls, a conditional call, a loop) in v1 form, as it never
    // returns, and structured (`bne` back to its start, then on out of
    // it), and `adds; b; ...`, whose yield path writes flags back.
    let progs = [
        calls(),
        calls_then(0x1AFF_FFFC),
        program(&adds_then(0xEA00_0000), &[0x0800_0000]),
    ];
    let (rom, d) = &progs[1];
    let ctx = Ctx::new(rom, d);
    assert!(crate::lower::structured(&ctx, &build_func(&ctx, 0x0800_0000)).is_some());
    for (rom, d) in &progs {
        let mut ctx = Ctx::new(rom, d);
        ctx.cfg.locals = true;
        let r = build_all(&ctx);
        for f in &r.funcs {
            let (t, why) = lower_func_checked(&ctx, f, &fn_name(f.entry));
            assert_eq!(why, None, "{}", pretty(t));
            let text = pretty(t.clone());
            assert!(text.contains("let mut r"), "{text}");
            // Drop each write-back statement in turn: the check fails if
            // it was needed (one after a reload with no write between
            // isn't). In these, the first of each follows a write.
            let loc = Locals::of(&ctx.cfg, f);
            let cuts = |w: &str| {
                text.match_indices(w)
                    .map(|(at, _)| format!("{}{}", &text[..at], &text[at + w.len()..]))
                    .collect::<Vec<_>>()
            };
            let wb = cuts("wb!();");
            assert!(!wb.is_empty());
            let item: syn::ItemFn = syn::parse_str(&wb[0]).unwrap();
            assert!(
                check(&loc, &item, None, &Default::default()).is_err(),
                "{}",
                wb[0]
            );
            // And a write-back that leaves a register out.
            for i in (0..15).filter(|&i| loc.written & 1 << i != 0) {
                for cut in cuts(&format!("m.cpu.r[{i}] = r{i};")) {
                    let item: syn::ItemFn = syn::parse_str(&cut).unwrap();
                    assert!(
                        check(&loc, &item, None, &Default::default()).is_err(),
                        "{cut}"
                    );
                }
            }
            // So does a register use between a call and its reload.
            if let Some(at) = text.find("pc = settle(") {
                let r = (0..15).find(|&i| loc.holds(i)).unwrap();
                let cut = format!("{}let _ = r{r};{}", &text[..at], &text[at..]);
                let item: syn::ItemFn = syn::parse_str(&cut).unwrap();
                assert!(
                    check(&loc, &item, None, &Default::default()).is_err(),
                    "{cut}"
                );
            }
        }
    }
}

#[test]
fn asm_text_prints_insn_markers_as_assembly() {
    let (rom, d) = thumb_calls();
    let mut ctx = Ctx::new(&rom, &d);
    let debug = pretty(lower_func(&ctx, &build_func(&ctx, 0x0800_0001)));
    assert!(debug.contains("\"ThumbBlHi { offset: 0 }\""), "{debug}");
    ctx.cfg.insn = Insn::Asm;
    let text = pretty(lower_func(&ctx, &build_func(&ctx, 0x0800_0001)));
    // The `bl` prints whole at its first half.
    assert!(text.contains("\"bl 0x08000008\""), "{text}");
    assert!(text.contains("\"(bl, second half)\""), "{text}");
    let callee = pretty(lower_func(&ctx, &build_func(&ctx, 0x0800_0009)));
    assert!(callee.contains("\"bx lr\""), "{callee}");
    ctx.cfg.insn = Insn::Comment;
    let text = pretty(lower_func(&ctx, &build_func(&ctx, 0x0800_0001)));
    assert!(text.contains("// 0x0800_0000 bl 0x08000008\n"), "{text}");
    assert!(
        !text.contains("second half") && !text.contains("insn!"),
        "{text}"
    );
}

#[test]
fn readable_leaves_insn_markers_out_unless_asked() {
    let (rom, d) = thumb_calls();
    let mut ctx = Ctx::new(&rom, &d);
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    ctx.readable = Some(&list);
    assert_eq!(ctx.cfg_for(0x0800_0001).insn, Insn::Off);
    assert_eq!(ctx.cfg_for(0x0800_0009).insn, Insn::Debug);
    ctx.cfg.insn = Insn::Asm;
    assert_eq!(ctx.cfg_for(0x0800_0001).insn, Insn::Asm);
    ctx.cfg.insn = Insn::Comment;
    assert_eq!(ctx.cfg_for(0x0800_0001).insn, Insn::Comment);
}

/// `f: ldr r0, =0x02000008; ldr r1, =0x03001000; bl g; b .; pool; g: add
/// r0, r0, #1; bx lr`, the pool words not code.
fn pool_calls() -> (Vec<u8>, Discovery) {
    let (rom, mut d) = program(
        &[
            0xE59F_0008, // 0x08000000 ldr r0, [pc, #8]
            0xE59F_1008, // 0x08000004 ldr r1, [pc, #8]
            0xEB00_0002, // 0x08000008 bl 0x08000018
            0xEAFF_FFFE, // 0x0800000C b .
            0x0200_0008, // 0x08000010 inside sBuf (a.o)
            0x0300_1000, // 0x08000014 sBuf (b.o)
            0xE280_0001, // 0x08000018 add r0, r0, #1
            0xE12F_FF1E, // 0x0800001C bx lr
        ],
        &[0x0800_0000, 0x0800_0018],
    );
    d.code.remove(&0x0800_0010);
    d.code.remove(&0x0800_0014);
    (rom, d)
}

#[test]
fn a_named_print_inverts_to_the_unnamed_one() {
    use crate::names::check::{check_func, check_tokens};
    use crate::names::{SourceMap, sha1_hex};
    let (rom, d) = pool_calls();
    let sha = sha1_hex(&rom);
    // Every function named (one a keyword), and every pool word, in a
    // static two files share.
    let map = format!(
        r#"[map]
rom_sha1 = "{sha}"
[functions]
0x08000000 = {{ name = "CallBoth", file = "main.o", proto = "void CallBoth(void)" }}
0x08000018 = {{ name = "loop", file = "main.o" }}
[data]
0x02000000 = {{ name = "sBuf", size = 16, file = "a.o" }}
0x03001000 = {{ name = "sBuf", size = 4, file = "b.o" }}
"#
    );
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let plain = Ctx::new(&rom, &d);
    let mut named = Ctx::new(&rom, &d);
    named.names = Some(&names);
    let r = build_all(&plain);
    let mut addrs = 0;
    for f in &r.funcs {
        let n = check_func(&plain, &named, f).unwrap();
        addrs += n.addrs;
        assert!(n.idents >= 1, "{:x}: its own name", f.entry);
    }
    assert_eq!(addrs, 2);
    let caller = pretty(lower_func_checked(&named, &r.funcs[0], &named.fn_ident(0x0800_0000)).0);
    for want in [
        "fn CallBoth(",
        "loop_(m, 0x0800_0018)",
        "addr!(sBuf__a + 8)",
        "addr!(sBuf__b)",
    ] {
        assert!(caller.contains(want), "{want} in {caller}");
    }
    // Unnamed, the same words are literals, and nothing says `addr!`.
    let text = pretty(lower_func(&plain, &r.funcs[0]));
    assert!(
        !text.contains("addr!") && text.contains("0x200_0008"),
        "{text}"
    );

    // A wrong offset, a name on the wrong key, or any other change fails.
    let a = lower_func(&plain, &r.funcs[0]);
    let b = lower_func_checked(&named, &r.funcs[0], &named.fn_ident(0x0800_0000)).0;
    let b = b.to_string();
    for bad in [
        b.replace("sBuf__a + 8", "sBuf__a + 4"),
        b.replace("sBuf__b", "sBuf__a"),
        b.replace("loop_", "CallBoth"),
        b.replacen("pc", "cp", 1),
    ] {
        assert_ne!(bad, b);
        assert!(
            check_tokens(&names, a.clone(), bad.parse().unwrap()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn an_io_register_name_inverts_to_its_address() {
    use crate::names::SourceMap;
    use crate::names::check::check_tokens;
    let names = SourceMap::from_texts("00", &["[map]\nrom_sha1 = \"00\"\n"]).unwrap();
    let check = |a: &str, b: &str| check_tokens(&names, a.parse().unwrap(), b.parse().unwrap());
    let plain = "m.set(0x1, Ptr::<u16>::at(0x400_0208), 0); m.get(0x3, Ptr::<u8>::at(0x400_0006))";
    let named = "m.set(0x1, REG_IME, 0); m.get(0x3, REG_VCOUNT.field::<u8>(0))";
    check(plain, named).unwrap();
    // Another register, offset or width fails.
    for bad in [
        named.replace("REG_IME", "REG_WAITCNT"),
        named.replace("field::<u8>(0)", "field::<u8>(1)"),
        named.replace("REG_VCOUNT.field::<u8>(0)", "REG_VCOUNT"),
    ] {
        assert!(check(plain, &bad).is_err(), "{bad}");
    }
    assert!(check(&plain.replace("u16", "u32"), named).is_err());
}

#[test]
fn the_play_view_names_nothing() {
    // No source map: the pool words print as literals and the functions
    // by key, exactly as before names existed.
    let (rom, d) = pool_calls();
    let ctx = Ctx::new(&rom, &d);
    let r = build_all(&ctx);
    let text = pretty(lower_func(&ctx, &r.funcs[0]));
    assert!(text.contains("pub fn f_08000000("), "{text}");
    assert!(text.contains("f_08000018(m, 0x0800_0018)"), "{text}");
    assert!(text.contains("0x300_1000"), "{text}");
}

/// A caller counting r4 down from 4 around `bl f`, and `f`: with `keeps`,
/// `push {r4, lr}; movs r4, #1; pop {r4}; pop {r0}; bx r0`, else
/// `movs r4, #1; bx lr`, which doesn't.
fn kept_loop(keeps: bool) -> (Vec<u8>, Discovery) {
    let mut h = vec![
        0xB510, // 0x00 push {r4, lr}
        0x2404, // 0x02 movs r4, #4
        0xF000, 0xF806, // 0x04 bl 0x14
        0x3C01, // 0x08 subs r4, #1
        0xD1FB, // 0x0a bne 0x04
        0xBC10, // 0x0c pop {r4}
        0xBC01, // 0x0e pop {r0}
        0x4700, // 0x10 bx r0
        0x46C0, // 0x12 nop
    ];
    let f: &[u16] = if keeps {
        &[0xB510, 0x2401, 0xBC10, 0xBC01, 0x4700]
    } else {
        &[0x2401, 0x4770]
    };
    h.extend(f);
    let mut sizes = vec![2, 2, 4, 2, 2, 2, 2, 2, 2];
    sizes.extend(f.iter().map(|_| 2));
    thumb_program(&h, &sizes, &[0x0800_0001, 0x0800_0015])
}

#[test]
fn a_push_pop_callee_keeps_its_registers_and_the_loop_skips_their_reload() {
    use crate::lower::{fn_name, lower_func_checked};
    for keeps in [true, false] {
        let (rom, d) = kept_loop(keeps);
        let mut ctx = Ctx::new(&rom, &d);
        ctx.cfg.locals = true;
        ctx.cfg.callee_saves = true;
        let r = build_all(&ctx);
        let proof = crate::saves::prove(&ctx, &r.funcs);
        let want: &[u32] = if keeps {
            &[0x0800_0001, 0x0800_0015]
        } else {
            &[]
        };
        assert_eq!(
            proof.kept.iter().copied().collect::<Vec<_>>(),
            want,
            "{:?}",
            proof.why
        );
        ctx.prove_saves(&r.funcs);
        let caller = r.funcs.iter().find(|f| f.entry == 0x0800_0001).unwrap();
        let (t, why) = lower_func_checked(&ctx, caller, &fn_name(caller.entry));
        assert_eq!(why, None, "{}", pretty(t));
        let text = pretty(t);
        assert_eq!(text.contains("reload_kept!();"), keeps, "{text}");
        if keeps {
            // Without the proof, the check refuses it.
            let loc = crate::locals::Locals::of(&ctx.cfg, caller);
            let item: syn::ItemFn = syn::parse_str(&text).unwrap();
            let none = Default::default();
            assert!(crate::locals::check(&loc, &item, None, &none).is_err());
            assert_eq!(crate::locals::check(&loc, &item, None, &ctx.kept), Ok(()));
        }
    }
}

#[test]
fn saves_fail_an_unbalanced_frame_and_an_unknown_jump() {
    // sub sp, #4; bx lr: sp is 4 lower. bx r1: anywhere.
    for (h, why) in [
        (
            &[0xB081u16, 0x4770][..],
            "returns with sp changed: sp = entry sp-4",
        ),
        (&[0x4708][..], "unknown place"),
    ] {
        let sizes: Vec<u32> = h.iter().map(|_| 2).collect();
        let (rom, d) = thumb_program(h, &sizes, &[0x0800_0001]);
        let ctx = Ctx::new(&rom, &d);
        let proof = crate::saves::prove(&ctx, &build_all(&ctx).funcs);
        assert!(proof.kept.is_empty());
        assert!(proof.why[&0x0800_0001].contains(why), "{:?}", proof.why);
    }
}

#[test]
fn a_summary_of_a_function_that_clobbers_r4_fails_its_proof() {
    for keeps in [true, false] {
        let (rom, d) = kept_loop(keeps);
        let ctx = Ctx::new(&rom, &d);
        let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
        let (caller, f) = (&sums.by_entry[&0x0800_0001], &sums.by_entry[&0x0800_0015]);
        assert_eq!((caller.keeps, f.keeps), (keeps, keeps));
        assert_eq!(f.holes.is_empty(), keeps, "{:?}", f.holes);
        if !keeps {
            assert!(
                f.holes
                    .iter()
                    .any(|h| h.contains("returns with r4-r11 changed: r4 = constant 0x1")),
                "{:?}",
                f.holes
            );
            assert_eq!(f.changes, 1 << 4);
            assert!(caller.holes.iter().any(|h| h.contains("doesn't keep them")));
            // The loop reads the r4 it changed: the caller's hole is hard.
            assert!(
                caller
                    .holes
                    .contains("0x08000008: reads r4-r11 a callee changed: r4, by 0x08000015"),
                "{:?}",
                caller.holes
            );
        }
        // Neither reads an argument.
        assert_eq!((caller.params, f.params), (0, 0));
    }
}

/// The summary holes of `f` of `sums` that are hard.
fn hard_holes(sums: &crate::saves::Summaries, f: u32) -> Vec<String> {
    use analysis::holes::{summary_hard, summary_kind};
    sums.by_entry[&f]
        .holes
        .iter()
        .filter(|h| summary_hard(&summary_kind(h)))
        .cloned()
        .collect()
}

#[test]
fn a_conditional_pop_and_return_keeps_the_registers() {
    // libgcc's `RETLDM "r4, r5" hi`: the pop and the return share a
    // condition, so the return has them back.
    let (rom, d) = program(
        &[
            0xE92D_4030, // push {r4, r5, lr}
            0xE3A0_4001, // mov r4, #1
            0xE350_0000, // cmp r0, #0
            0x88BD_4030, // pophi {r4, r5, lr}
            0x812F_FF1E, // bxhi lr
            0xE3A0_5002, // mov r5, #2
            0xE8BD_4030, // pop {r4, r5, lr}
            0xE12F_FF1E, // bx lr
        ],
        &[ROM_BASE],
    );
    let ctx = Ctx::new(&rom, &d);
    let funcs = build_all(&ctx).funcs;
    let proof = crate::saves::prove(&ctx, &funcs);
    assert!(proof.kept.contains(&ROM_BASE), "{:?}", proof.why);
    let sums = crate::saves::summarize(&ctx, &funcs);
    assert_eq!(
        sums.by_entry[&ROM_BASE].holes,
        std::collections::BTreeSet::new()
    );
}

/// `w: push {r4, lr}; bl s; pop {r4}; pop {r0}; bx r0` and `s`, which uses
/// r4 and r5 as scratch: `movs r4, #1; movs r5, #2; bx lr`, or (`param`)
/// reads r4 as a parameter: `ldr r0, [r4, #0]; movs r4, #1; bx lr`.
fn scratch_callee(param: bool) -> (Vec<u8>, Discovery) {
    let mut h = vec![
        0xB510, // 0x00 push {r4, lr}
        0xF000, 0xF804, // 0x02 bl 0x0e
        0xBC10, // 0x06 pop {r4}
        0xBC01, // 0x08 pop {r0}
        0x4700, // 0x0a bx r0
        0x46C0, // 0x0c nop
    ];
    let s: &[u16] = if param {
        &[0x6820, 0x2401, 0x4770]
    } else {
        &[0x2401, 0x2502, 0x4770]
    };
    h.extend(s);
    let mut sizes = vec![2, 4, 2, 2, 2, 2];
    sizes.extend(s.iter().map(|_| 2));
    thumb_program(&h, &sizes, &[0x0800_0001, 0x0800_000f])
}

#[test]
fn scratch_r4_r11_no_caller_reads_is_soft_and_a_register_parameter_hard() {
    for param in [false, true] {
        let (rom, d) = scratch_callee(param);
        let ctx = Ctx::new(&rom, &d);
        let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
        let s = &sums.by_entry[&0x0800_000f];
        assert_eq!(s.changes, if param { 1 << 4 } else { 0b11 << 4 });
        // The wrapper pops r4 back: it keeps them, and reads nothing s
        // changed.
        assert_eq!(hard_holes(&sums, 0x0800_0001), Vec::<String>::new());
        // A register parameter comes from the `bl`, which passes it: soft.
        assert_eq!(hard_holes(&sums, 0x0800_000f), Vec::<String>::new());
        assert_eq!(s.passed(), if param { 1 } else { 0 }, "{:?}", s.holes);
        assert_eq!(
            s.holes
                .contains("0x0800000e: takes r4-r11 from its callers: r4"),
            param
        );
    }
    // The wrapper passes the r4 it came with, so it takes it in turn:
    // hard where a pointer may call the wrapper, or the reader itself.
    for (f, hole) in [
        (
            0x0800_0001u32,
            "0x08000004: reads r4-r11 at entry: r4, which 0x0800000f takes",
        ),
        (0x0800_000f, "0x0800000e: reads r4-r11 at entry: r4"),
    ] {
        let (mut rom, d) = scratch_callee(true);
        rom.extend(f.to_le_bytes());
        let ctx = Ctx::new(&rom, &d);
        let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
        assert_eq!(hard_holes(&sums, f)[0], hole);
    }
    // A pointer to the scratch function in the ROM: callers unknown.
    let (mut rom, d) = scratch_callee(false);
    rom.extend(0x0800_000fu32.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(
        hard_holes(&sums, 0x0800_000f),
        vec!["0x0800000f: changes r4-r11 and may be called through a pointer".to_string()]
    );
}

/// The summary holes of the function at 0x08000001 of a Thumb program of
/// one-halfword instructions, with a pointer to it in the ROM (so its
/// callers are unknown, and enter it with r4-r11 zeroed).
fn one_function_holes(h: &[u16]) -> Vec<String> {
    let (mut rom, d) = thumb_program(h, &vec![2; h.len()], &[0x0800_0001]);
    rom.resize(rom.len().next_multiple_of(4), 0);
    rom.extend(0x0800_0001u32.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    sums.by_entry[&0x0800_0001].holes.iter().cloned().collect()
}

#[test]
fn an_entry_read_masked_away_is_soft_and_one_that_survives_hard() {
    // agbcc's byte-by-byte word in an r4 it never set: every old bit
    // cleared before r4 is used.
    let masked = one_function_holes(&[
        0xB510, // 0x00 push {r4, lr}
        0x21FF, // 0x02 movs r1, #0xff
        0x438C, // 0x04 bics r4, r1: bits 8-31 survive
        0x4304, // 0x06 orrs r4, r0
        0x400C, // 0x08 ands r4, r1: none do
        0x1C20, // 0x0a adds r0, r4, #0
        0xBC10, // 0x0c pop {r4}
        0xBC02, // 0x0e pop {r1}
        0x4708, // 0x10 bx r1
    ]);
    assert_eq!(
        masked,
        vec!["0x08000008: masks away r4-r11's entry bits: r4".to_string()]
    );
    // Without the last `and`, bits 8-31 are stored: `str r4, [r0, #0]`.
    let kept = one_function_holes(&[
        0xB510, 0x21FF, 0x438C, 0x4304, 0x46C0, 0x6004, 0xBC10, 0xBC02, 0x4708,
    ]);
    assert_eq!(
        kept,
        vec!["0x0800000a: reads r4-r11 at entry: r4".to_string()]
    );
    // An entry r5 used as an address (Minish's 0x08060ba1), in a
    // function that keeps them.
    let deref = one_function_holes(&[
        0xB510, // push {r4, lr}
        0x6828, // ldr r0, [r5, #0]
        0xBC10, // pop {r4}
        0xBC02, // pop {r1}
        0x4708, // bx r1
    ]);
    // (The load's rotation by the address's low bits makes the r0 it
    // returns depend on r5 too, but no caller reads it.)
    assert_eq!(
        deref,
        vec!["0x08000002: reads r4-r11 at entry: r5".to_string()]
    );
    // An entry r6 compared: the branch's condition holds its bits.
    let cmp = one_function_holes(&[
        0x2E00, // 0x00 cmp r6, #0
        0xD000, // 0x02 beq 0x06
        0x2001, // 0x04 movs r0, #1
        0x4770, // 0x06 bx lr
    ]);
    assert_eq!(
        cmp,
        vec!["0x08000002: reads r4-r11 at entry: r6".to_string()]
    );
    // Modern GCC's `sbcs r5, r5` after a compare (Rogue's 0x0820c629):
    // 0 or -1 by the carry, whatever r5 held.
    let sbc = |cmp: u16| {
        one_function_holes(&[
            0xB530, // 0x00 push {r4, r5, lr}
            cmp,    // 0x02
            0x41AD, // 0x04 sbcs r5, r5
            0x6828, // 0x06 ldr r0, [r5, #0]
            0xBC30, // 0x08 pop {r4, r5}
            0xBC02, // 0x0a pop {r1}
            0x4708, // 0x0c bx r1
        ])
    };
    assert_eq!(
        sbc(0x4288), // cmp r0, r1
        vec!["0x08000004: masks away r4-r11's entry bits: r5".to_string()]
    );
    // The carry of a compare of the entry r6 is still r6's.
    assert_eq!(
        sbc(0x2E00), // cmp r6, #0
        vec!["0x08000006: reads r4-r11 at entry: r6".to_string()]
    );
}

#[test]
fn an_entry_value_returned_is_hard_where_a_caller_reads_it() {
    // `c: push {lr}; bl f; adds r2, r0, #0; pop {r0}; bx r0` and `f: adds
    // r0, r5, #0; bx lr`, or (`unread`) c without its `adds`.
    for unread in [false, true] {
        let read = if unread { 0x46C0 } else { 0x1C02 };
        let (rom, d) = thumb_program(
            &[
                0xB500, // 0x00 push {lr}
                0xF000, 0xF803, // 0x02 bl 0x0c
                read,   // 0x06 adds r2, r0, #0 (or nop)
                0xBC01, // 0x08 pop {r0}
                0x4700, // 0x0a bx r0
                0x1C28, // 0x0c adds r0, r5, #0
                0x4770, // 0x0e bx lr
            ],
            &[2, 4, 2, 2, 2, 2, 2],
            &[0x0800_0001, 0x0800_000d],
        );
        let ctx = Ctx::new(&rom, &d);
        let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
        let want: Vec<String> = if unread {
            vec![]
        } else {
            vec!["0x0800000e: reads r4-r11 at entry: r5, returned in r0".into()]
        };
        assert_eq!(hard_holes(&sums, 0x0800_000d), want);
    }
}

#[test]
fn an_entry_hi_register_passed_to_a_callee_is_hard() {
    // `w: push {lr}; mov r0, r8; bl s; pop {r0}; bx r0`, `s: adds r0, #1;
    // bx lr`: s reads the r8 that w took as it came (TMC's `player.s`
    // passing r8 and r11 on).
    let (rom, d) = thumb_program(
        &[
            0xB500, // 0x00 push {lr}
            0x4640, // 0x02 mov r0, r8
            0xF000, 0xF802, // 0x04 bl 0x0c
            0xBC01, // 0x08 pop {r0}
            0x4700, // 0x0a bx r0
            0x3001, // 0x0c adds r0, #1
            0x4770, // 0x0e bx lr
        ],
        &[2, 2, 4, 2, 2, 2, 2],
        &[0x0800_0001, 0x0800_000d],
    );
    // A pointer to w: its callers are unknown.
    let mut rom = rom;
    rom.extend(0x0800_0001u32.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(
        hard_holes(&sums, 0x0800_0001),
        vec!["0x08000006: reads r4-r11 at entry: r8, passed in r0".to_string()]
    );
    // Without the pointer nothing enters w by a call: it would be
    // jumped into, with every register.
    rom.truncate(rom.len() - 4);
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(hard_holes(&sums, 0x0800_0001), Vec::<String>::new());
    assert!(
        sums.by_entry[&0x0800_0001]
            .holes
            .contains("0x08000006: reads r4-r11 at entry, only jumped into: r8, passed in r0")
    );
}

#[test]
fn an_entry_read_tail_called_from_a_called_function_is_hard() {
    // `g: b f`, `f: ldr r0, [r5, #0]; bx lr`: f takes g's r4-r11, which
    // `Regs::call` zeroed where a pointer enters g: g's hole, since the
    // jump passes f what g has.
    let (rom, d) = thumb_program(
        &[
            0xE000, // 0x00 b 0x04
            0x46C0, // 0x02 nop
            0x6828, // 0x04 ldr r0, [r5, #0]
            0x4770, // 0x06 bx lr
        ],
        &[2, 2, 2, 2],
        &[0x0800_0001, 0x0800_0005],
    );
    let mut rom = rom;
    rom.extend(0x0800_0001u32.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(
        hard_holes(&sums, 0x0800_0001),
        vec!["0x08000005: reads r4-r11 at entry: r5, which 0x08000005 takes".to_string()]
    );
    assert_eq!(hard_holes(&sums, 0x0800_0005), Vec::<String>::new());
    assert_eq!(sums.by_entry[&0x0800_0005].passed(), 1 << 1);
    // Without the pointer g is only jumped into, and so is f.
    rom.truncate(rom.len() - 4);
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(hard_holes(&sums, 0x0800_0005), Vec::<String>::new());
    assert!(
        sums.by_entry[&0x0800_0005]
            .holes
            .contains("0x08000004: reads r4-r11 at entry, only jumped into: r5")
    );
}

/// `d: push {r5, lr}; movs r5, #3 (or, `unset`, nop); bl v; pop {r5}; pop
/// {r0}; bx r0`, the stub `v: bx r0`, `t: ldr r0, [r5, #0]; bx lr`, and a pointer
/// to t in the ROM: d calls through a pointer, t reads r5 as it came.
fn pointer_call_to_a_reader(unset: bool) -> (Vec<u8>, Discovery) {
    let (mut rom, d) = thumb_program(
        &[
            0xB520,                              // 0x00 push {r5, lr}
            if unset { 0x46C0 } else { 0x2503 }, // 0x02 movs r5, #3
            0xF000,
            0xF804, // 0x04 bl 0x10
            0xBC20, // 0x08 pop {r5}
            0xBC01, // 0x0a pop {r0}
            0x4700, // 0x0c bx r0
            0x46C0, // 0x0e nop
            0x4700, // 0x10 bx r0
            0x46C0, // 0x12 nop
            0x6828, // 0x14 ldr r0, [r5, #0]
            0x4770, // 0x16 bx lr
        ],
        &[2, 2, 4, 2, 2, 2, 2, 2, 2, 2, 2],
        &[0x0800_0001, 0x0800_0015],
    );
    rom.extend(0x0800_0015u32.to_le_bytes());
    (rom, d)
}

#[test]
fn a_register_a_callee_changed_is_not_passed_on() {
    // `c: push {lr}; bl a; bl b; pop {r0}; bx r0`, `a: movs r5, #1; bx
    // lr`, `b: ldr r0, [r5, #0]; bx lr`: the machine gives b the r5 a
    // left, the lift c's own local.
    let (rom, d) = thumb_program(
        &[
            0xB500, // 0x00 push {lr}
            0xF000, 0xF805, // 0x02 bl 0x10
            0xF000, 0xF805, // 0x06 bl 0x14
            0xBC01, // 0x0a pop {r0}
            0x4700, // 0x0c bx r0
            0x46C0, // 0x0e nop
            0x2501, // 0x10 movs r5, #1
            0x4770, // 0x12 bx lr
            0x6828, // 0x14 ldr r0, [r5, #0]
            0x4770, // 0x16 bx lr
        ],
        &[2, 4, 4, 2, 2, 2, 2, 2, 2, 2],
        &[0x0800_0001, 0x0800_0011, 0x0800_0015],
    );
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(
        hard_holes(&sums, 0x0800_0001),
        vec!["0x08000008: reads r4-r11 a callee changed: r5, by 0x08000011".to_string()]
    );
    let c = &sums.by_entry[&0x0800_0001];
    assert_eq!(
        c.passes,
        std::collections::BTreeSet::from([(0x0800_0008, 0x0800_0015)])
    );
}

#[test]
fn a_pointer_call_the_holes_file_names_passes_what_its_callee_takes() {
    use crate::saves::{Assume, Passing, Sets, summarize_passing};
    let (d, t) = (0x0800_0001u32, 0x0800_0015u32);
    for unset in [false, true] {
        let (mut rom, disc) = pointer_call_to_a_reader(unset);
        // A pointer to d too: `Regs::call` zeroes what it comes with.
        rom.extend(d.to_le_bytes());
        let ctx = Ctx::new(&rom, &disc);
        let funcs = build_all(&ctx).funcs;
        // Unnamed, the pointer call zeroes t's r5: t's hole.
        let sums = crate::saves::summarize(&ctx, &funcs);
        assert_eq!(
            hard_holes(&sums, t),
            vec!["0x08000014: reads r4-r11 at entry: r5".to_string()]
        );
        assert_eq!(hard_holes(&sums, d), Vec::<String>::new());
        let sites = &sums.by_entry[&d].indirect_sites;
        assert_eq!(sites.len(), 1, "{sites:?}");
        let at = *sites.iter().next().unwrap();
        // Named, d's call passes its r5: exact where d set it, d's own
        // entry read where it didn't.
        let mut passing = Passing::default();
        passing.sites.entry(at).or_default().insert(t);
        let sums = summarize_passing(&ctx, &funcs, Assume::default(), &Sets::new(), &passing);
        assert_eq!(hard_holes(&sums, t), Vec::<String>::new());
        assert_eq!(sums.by_entry[&t].passed(), 1 << 1);
        let want: Vec<String> = if unset {
            vec![format!(
                "0x{at:08x}: reads r4-r11 at entry: r5, which 0x08000015 takes"
            )]
        } else {
            vec![]
        };
        assert_eq!(hard_holes(&sums, d), want);
        // A stand-in's code takes nothing, whatever the function it
        // replaces read.
        passing.replaced.insert(t);
        let sums = summarize_passing(&ctx, &funcs, Assume::default(), &Sets::new(), &passing);
        assert_eq!(hard_holes(&sums, d), Vec::<String>::new());
    }
}

/// agbcc's lowest-of-a-list loop, `best = -1; for (i = 0; i < 4; i++) {
/// call(); if (best == -1) { best = i; low = i; } else if (low > i) low =
/// i; }`, with `low` in r6, which the prologue left holding the r8 it
/// saved: `f` (called through a pointer), a leaf at 0x38 and, at 0x3c, a
/// function that calls through a pointer itself. The loop's call goes to
/// `callee`.
fn lowest_of_a_list(callee: u32) -> (Vec<u8>, Discovery) {
    let bl = ((callee - 0x10) / 2) as u16;
    let (mut rom, d) = thumb_program(
        &[
            0xB570, // 0x00 push {r4, r5, r6, lr}
            0x4646, // 0x02 mov r6, r8
            0xB440, // 0x04 push {r6}
            0x2501, // 0x06 movs r5, #1
            0x426D, // 0x08 negs r5, r5
            0x2400, // 0x0a movs r4, #0
            0xF000,
            0xF800 | bl, // 0x0c bl callee
            0x2001,      // 0x10 movs r0, #1
            0x4240,      // 0x12 negs r0, r0
            0x4285,      // 0x14 cmp r5, r0
            0xD102,      // 0x16 bne 0x1e
            0x1C25,      // 0x18 adds r5, r4, #0
            0x1C26,      // 0x1a adds r6, r4, #0
            0xE002,      // 0x1c b 0x24
            0x42A6,      // 0x1e cmp r6, r4
            0xDD00,      // 0x20 ble 0x24
            0x1C26,      // 0x22 adds r6, r4, #0
            0x3401,      // 0x24 adds r4, #1
            0x2C04,      // 0x26 cmp r4, #4
            0xDBF0,      // 0x28 blt 0x0c
            0x1C28,      // 0x2a adds r0, r5, #0
            0xBC08,      // 0x2c pop {r3}
            0x4698,      // 0x2e mov r8, r3
            0xBC70,      // 0x30 pop {r4, r5, r6}
            0xBC02,      // 0x32 pop {r1}
            0x4708,      // 0x34 bx r1
            0x46C0,      // 0x36 nop
            0x4770,      // 0x38 bx lr
            0x46C0,      // 0x3a nop
            0xB500,      // 0x3c push {lr}
            0xF000,
            0xF803, // 0x3e bl 0x48
            0xBC02, // 0x42 pop {r1}
            0x4708, // 0x44 bx r1
            0x46C0, // 0x46 nop
            0x4700, // 0x48 bx r0
            0x46C0, // 0x4a nop
        ],
        &[
            2, 2, 2, 2, 2, 2, 4, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 4, 2, 2, 2, 2, 2,
        ],
        &[0x0800_0001, 0x0800_0039, 0x0800_003d],
    );
    rom.extend(0x0800_0001u32.to_le_bytes());
    (rom, d)
}

#[test]
fn a_branch_on_a_certain_constant_is_taken_one_way() {
    // The first pass of the loop has best = -1 for certain (made of
    // constants, kept by the leaf), so `bne` isn't taken and `cmp r6, r4`
    // never runs on the r8 the prologue left in r6; from the second pass
    // on r6 is set.
    let (rom, d) = lowest_of_a_list(0x38);
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert!(sums.by_entry[&0x0800_0039].keeps);
    assert_eq!(hard_holes(&sums, 0x0800_0001), Vec::<String>::new());
    assert_eq!(sums.by_entry[&0x0800_0001].takes, 0);
    // A callee not proven to keep r5 (it calls through a pointer): best
    // isn't certain after it, both ways are taken, and the read stays.
    let (rom, d) = lowest_of_a_list(0x3c);
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    let callee = &sums.by_entry[&0x0800_003d];
    assert!(!callee.keeps);
    assert!(
        callee.holes.contains("0x08000040: an indirect call"),
        "{:?}",
        callee.holes
    );
    assert_eq!(
        hard_holes(&sums, 0x0800_0001),
        vec!["0x08000020: reads r4-r11 at entry: r8".to_string()]
    );
}

#[test]
fn a_certain_constant_is_not_certain_after_an_msr() {
    // ARM `f: mov r0, #0; b 1f; 1: cmp r0, #0; bne 2f; msr cpsr_f, r1;
    // bxeq lr; ldr r0, [r5]; bx lr; 2: bx lr`. The `b` ends the block, so
    // the 0 reaches the `cmp` as the register's certain constant, not
    // folded by the IR, and the `bne` keeps the `cmp`'s flags written. The
    // msr then sets Z from r1, so the `bxeq` may fall through to the read
    // of r5.
    let words: [u32; 9] = [
        0xE3A0_0000, // 0x00 mov r0, #0
        0xEAFF_FFFF, // 0x04 b 0x08
        0xE350_0000, // 0x08 cmp r0, #0
        0x1A00_0003, // 0x0c bne 0x20
        0xE128_F001, // 0x10 msr cpsr_f, r1
        0x012F_FF1E, // 0x14 bxeq lr
        0xE595_0000, // 0x18 ldr r0, [r5]
        0xE12F_FF1E, // 0x1c bx lr
        0xE12F_FF1E, // 0x20 bx lr
    ];
    let mut rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut d = Discovery::default();
    for i in 0..9 {
        let code = Code {
            size: 4,
            thumb: false,
            evidence: Evidence::Confirmed,
        };
        d.code.insert(ROM_BASE + 4 * i, code);
    }
    d.funcs.insert(ROM_BASE, Evidence::Confirmed);
    // A pointer to f: it is entered by a call.
    rom.extend(ROM_BASE.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(
        hard_holes(&sums, ROM_BASE),
        vec!["0x08000018: reads r4-r11 at entry: r5".to_string()]
    );
}

#[test]
fn an_exit_always_taken_still_reads_its_target() {
    // ARM `f: mov r0, #0; b 1f; 1: cmp r0, #0; bxeq r8; bx lr` (the `b`
    // as in the test above): Z is certain, so the `bxeq` is always taken,
    // to the r8 f came with.
    let words: [u32; 5] = [
        0xE3A0_0000, // 0x00 mov r0, #0
        0xEAFF_FFFF, // 0x04 b 0x08
        0xE350_0000, // 0x08 cmp r0, #0
        0x012F_FF18, // 0x0c bxeq r8
        0xE12F_FF1E, // 0x10 bx lr
    ];
    let mut rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut d = Discovery::default();
    for i in 0..5 {
        let code = Code {
            size: 4,
            thumb: false,
            evidence: Evidence::Confirmed,
        };
        d.code.insert(ROM_BASE + 4 * i, code);
    }
    d.funcs.insert(ROM_BASE, Evidence::Confirmed);
    rom.extend(ROM_BASE.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(
        hard_holes(&sums, ROM_BASE),
        vec!["0x0800000c: reads r4-r11 at entry: r8".to_string()]
    );
}

#[test]
fn a_constant_loaded_from_the_stack_is_not_certain() {
    // `f: push {lr}; sub sp, #4; movs r0, #0; str r0, [sp]; mov r0, sp;
    // bl g; ldr r0, [sp]; cmp r0, #0; beq 1f; ldr r0, [r5, #0]; 1: add
    // sp, #4; pop {r1}; bx r1`, `g: bx lr`: g has the local's address, so
    // the word read back may not be the 0 stored, and the read of r5
    // stays f's.
    let (mut rom, d) = thumb_program(
        &[
            0xB500, // 0x00 push {lr}
            0xB081, // 0x02 sub sp, #4
            0x2000, // 0x04 movs r0, #0
            0x9000, // 0x06 str r0, [sp, #0]
            0x4668, // 0x08 mov r0, sp
            0xF000, 0xF807, // 0x0a bl 0x1c
            0x9800, // 0x0e ldr r0, [sp, #0]
            0x2800, // 0x10 cmp r0, #0
            0xD000, // 0x12 beq 0x16
            0x6828, // 0x14 ldr r0, [r5, #0]
            0xB001, // 0x16 add sp, #4
            0xBC02, // 0x18 pop {r1}
            0x4708, // 0x1a bx r1
            0x4770, // 0x1c bx lr
            0x46C0, // 0x1e nop
        ],
        &[2, 2, 2, 2, 2, 4, 2, 2, 2, 2, 2, 2, 2, 2, 2],
        &[0x0800_0001, 0x0800_001d],
    );
    rom.extend(0x0800_0001u32.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert_eq!(
        hard_holes(&sums, 0x0800_0001),
        vec!["0x08000014: reads r4-r11 at entry: r5".to_string()]
    );
}

#[test]
fn a_takes_with_no_sites_names_a_function_only_jumped_into() {
    use crate::saves::{Assume, Passing, Sets, summarize_passing};
    // `t: ldr r0, [r5, #0]; bx lr`, its address a word in the ROM: a
    // pointer may call it, which zeroes r5, unless the holes file says
    // none does. Then its plain entry stops the program.
    let (mut rom, d) = thumb_program(&[0x6828, 0x4770], &[2, 2], &[0x0800_0001]);
    rom.extend(0x0800_0001u32.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let funcs = build_all(&ctx).funcs;
    let sums = crate::saves::summarize(&ctx, &funcs);
    assert_eq!(
        hard_holes(&sums, 0x0800_0001),
        vec!["0x08000000: reads r4-r11 at entry: r5".to_string()]
    );
    assert_eq!(sums.by_entry[&0x0800_0001].passed(), 0);
    let mut passing = Passing::default();
    passing.named.insert(0x0800_0001);
    let sums = summarize_passing(&ctx, &funcs, Assume::default(), &Sets::new(), &passing);
    assert_eq!(hard_holes(&sums, 0x0800_0001), Vec::<String>::new());
    assert_eq!(sums.by_entry[&0x0800_0001].passed(), 1 << 1);
}

#[test]
fn the_code_after_a_call_that_never_returns_is_not_the_callers() {
    // `f: push {lr}; bl p; ldr r0, [r5, #0]; pop {r1}; bx r1`, `p: b p`:
    // p never returns, so the read of r5 after the call runs only from
    // jumps into it (agb's panic paths), never from f's own entry.
    let (rom, d) = thumb_program(
        &[
            0xB500, // 0x00 push {lr}
            0xF000, 0xF803, // 0x02 bl 0x0c
            0x6828, // 0x06 ldr r0, [r5, #0]
            0xBC02, // 0x08 pop {r1}
            0x4708, // 0x0a bx r1
            0xE7FE, // 0x0c b 0x0c
        ],
        &[2, 4, 2, 2, 2, 2],
        &[0x0800_0001, 0x0800_000d],
    );
    // A pointer to f: it is entered by a call.
    let mut rom = rom;
    rom.extend(0x0800_0001u32.to_le_bytes());
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    assert!(!sums.by_entry[&0x0800_000d].returns);
    assert!(!sums.by_entry[&0x0800_0001].returns);
    assert_eq!(hard_holes(&sums, 0x0800_0001), Vec::<String>::new());
}

/// `c: push {lr}; movs r1, #5; bl f; adds r2, r0, #0; pop {r0}; bx r0`,
/// `f: adds r0, r1, #0; bx lr` and `g: push {lr}; bl f; pop {r0}; bx r0`,
/// in Thumb.
fn summary_calls() -> (Vec<u8>, Discovery) {
    let h = [
        0xB500, 0x2105, 0xF000, 0xF804, 0x1C02, 0xBC01, 0x4700, 0x46C0, // c
        0x1C08, 0x4770, // f, 0x10
        0xB500, 0xF7FF, 0xFFFB, 0xBC01, 0x4700, // g, 0x14
    ];
    let sizes = [2, 2, 4, 2, 2, 2, 2, 2, 2, 2, 4, 2, 2];
    thumb_program(&h, &sizes, &[0x0800_0001, 0x0800_0011, 0x0800_0015])
}

#[test]
fn summaries_find_parameters_through_calls_and_results_callers_read() {
    use crate::saves::{Category, Cause, compare, proto_regs};
    let (rom, d) = summary_calls();
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    let s = |k: u32| &sums.by_entry[&k];
    // f reads r1 and leaves r0, which c reads.
    assert_eq!(s(0x0800_0011).params, 0b10);
    assert_eq!(s(0x0800_0011).causes[&1], Cause::Reads(0x0800_0010));
    assert_eq!(s(0x0800_0011).results, 0b1);
    assert_eq!(s(0x0800_0011).read_after, 0b1);
    assert!(s(0x0800_0011).keeps);
    // c sets r1 first; g passes its own r1 on (a `bl`'s address is its
    // second half's).
    assert_eq!(s(0x0800_0001).params, 0);
    assert_eq!(s(0x0800_0015).params, 0b10);
    assert_eq!(
        s(0x0800_0015).causes[&1],
        Cause::Callee {
            callee: 0x0800_0011,
            at: 0x0800_0018
        }
    );
    // Against prototypes: f with one parameter reads past it; with two
    // it agrees.
    let sha = crate::names::sha1_hex(&rom);
    for (proto, agrees) in [("u32 f(u8 a)", false), ("u32 f(u8 a, u8 b)", true)] {
        let map = format!(
            "[map]\nrom_sha1 = \"{sha}\"\n[functions]\n0x08000011 = {{ name = \"f\", proto = \"{proto}\" }}\n"
        );
        let m = crate::names::SourceMap::from_texts(&sha, &[&map]).unwrap();
        let c = compare(&sums, &m);
        assert_eq!(c.no_proto, 2);
        assert_eq!(c.agree.len(), agrees as usize);
        if !agrees {
            assert_eq!(c.disagree[0].why[0].0, Category::ParamRead);
        }
        assert_eq!(proto_regs(&m, proto, "f").unwrap().1, 1);
    }
    let map = format!(
        "[map]\nrom_sha1 = \"{sha}\"\n[types.\"struct Big\"]\nkind = \"struct\"\nsize = 8\nfields = []\n"
    );
    let m = crate::names::SourceMap::from_texts(&sha, &[&map]).unwrap();
    let r = |p: &str, n: &str| proto_regs(&m, p, n);
    assert_eq!(r("u32 F(u8 a, u64 b, u8 c)", "F"), Some((0b1111, 1)));
    assert_eq!(r("u64 F(void)", "F"), Some((0, 0b11)));
    // A struct returned by value: a hidden first argument.
    assert_eq!(r("struct Big G(u8 id)", "G"), Some((0b11, 0)));
    assert_eq!(r("void H(struct Big b)", "H"), Some((0b11, 0)));
    assert_eq!(r("int V(const char *f, ...)", "V"), Some((0b1111, 1)));
    assert_eq!(r("void U(struct Unknown u)", "U"), None);
}

/// `a: push {lr}; bl b; adds r1, r0, #0; pop {r0}; bx r0`,
/// `b: push {lr}; bl c; pop {pc}` (`return c();`) and `c: movs r0, #1;
/// bx lr`, in Thumb.
#[test]
fn a_result_returned_unread_is_read_where_its_callers_read_it() {
    let h = [
        0xB500, 0xF000, 0xF803, 0x1C01, 0xBC01, 0x4700, // a
        0xB500, 0xF000, 0xF801, 0xBD00, // b, 0x0c
        0x2001, 0x4770, // c, 0x14
    ];
    let sizes = [2, 4, 2, 2, 2, 2, 4, 2, 2, 2];
    let (rom, d) = thumb_program(&h, &sizes, &[0x0800_0001, 0x0800_000d, 0x0800_0015]);
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    let (b, c) = (&sums.by_entry[&0x0800_000d], &sums.by_entry[&0x0800_0015]);
    assert_eq!((b.read_after, b.results), (1, 1));
    assert_eq!((c.read_after, c.forwarded_after, c.results), (0, 1, 1));
}

/// `c: push {lr}; bl stub; adds r2, r1, #0; pop {r0}; bx r0`, `stub:
/// bx r3` (a call through r3) and `f: adds r1, r0, #0; bx lr`, in Thumb.
/// Taken as unknown, the call reads r0-r3 and leaves r1 undefined; with
/// its callee set {f}, it reads r0 and leaves f's r1, which f returns.
#[test]
fn an_indirect_call_with_a_complete_callee_set_reads_its_callees() {
    use crate::saves::{Assume, Cause, Sets, summarize_sets};
    let h = [
        0xB500, 0xF000, 0xF803, 0x1C0A, 0xBC01, 0x4700, // c
        0x4718, // stub, 0x0c
        0x1C01, 0x4770, // f, 0x0e
    ];
    let sizes = [2, 4, 2, 2, 2, 2, 2, 2];
    let (c, f) = (0x0800_0001, 0x0800_000f);
    let (rom, d) = thumb_program(&h, &sizes, &[c, f]);
    let ctx = Ctx::new(&rom, &d);
    let funcs = build_all(&ctx).funcs;
    let plain = crate::saves::summarize(&ctx, &funcs);
    let at = *plain.by_entry[&c]
        .indirect_sites
        .first()
        .expect("c's call through r3");
    assert_eq!(plain.by_entry[&c].params, 0xf);
    assert_eq!(plain.by_entry[&c].causes[&3], Cause::Indirect(at));
    assert!(!plain.by_entry[&c].undefined_reads.is_empty());
    assert_eq!(plain.by_entry[&f].results, 0);
    let sets = Sets::from([(at, [f].into())]);
    let sums = summarize_sets(&ctx, &funcs, Assume::default(), &sets);
    let (sc, sf) = (&sums.by_entry[&c], &sums.by_entry[&f]);
    assert_eq!(sc.params, 0b1);
    assert_eq!(sc.causes[&0], Cause::Callee { callee: f, at });
    assert!(sc.undefined_reads.is_empty(), "{:x?}", sc.undefined_reads);
    assert!(sc.callees.contains(&f));
    assert_eq!((sf.params, sf.read_after, sf.results), (0b1, 0b10, 0b10));
}

#[test]
fn a_typedef_struct_returned_by_value_takes_a_hidden_argument() {
    let sha = crate::names::sha1_hex(b"");
    let map = format!(
        "[map]\nrom_sha1 = \"{sha}\"\n[types.Vec2]\nkind = \"struct\"\nsize = 8\nfields = []\n"
    );
    let m = crate::names::SourceMap::from_texts(&sha, &[&map]).unwrap();
    assert_eq!(
        crate::saves::proto_regs(&m, "Vec2 F(void)", "F"),
        Some((1, 0))
    );
}

#[test]
fn only_the_readable_list_prints_readable() {
    use crate::lower::{fn_name, lower_func_checked};
    use crate::names::{SourceMap, sha1_hex};
    let (rom, d) = pool_calls();
    let sha = sha1_hex(&rom);
    let map = format!(
        "[map]\nrom_sha1 = \"{sha}\"\n[functions]\n0x08000000 = {{ name = \"CallBoth\" }}\n\
         [data]\n0x02000000 = {{ name = \"sBuf\", size = 16 }}\n"
    );
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let list = std::collections::BTreeSet::from([0x0800_0000]);
    let mut ctx = Ctx::new(&rom, &d);
    ctx.readable = Some(&list);
    let r = build_all(&ctx);
    // The listed function: no `insn!` markers, its registers in `Machine` (ARM
    // code keeps the play view's statements).
    let (t, why) = lower_func_checked(&ctx, &r.funcs[0], &fn_name(0x0800_0000));
    assert_eq!(why, None);
    let text = pretty(t);
    assert!(!text.contains("let mut r"), "{text}");
    assert!(!text.contains("insn!"), "{text}");
    // The rest as the play view prints it.
    let callee = pretty(lower_func(&ctx, &r.funcs[1]));
    assert!(!callee.contains("let mut r"), "{callee}");
    assert!(callee.contains("insn!("), "{callee}");
    assert!(!callee.contains("\"bx lr\""), "{callee}");
    // Named, the pool word is `addr!`.
    ctx.names = Some(&names);
    let (t, why) = lower_func_checked(&ctx, &r.funcs[0], &ctx.fn_ident(0x0800_0000));
    assert_eq!(why, None, "{}", pretty(t.clone()));
    assert!(pretty(t).contains("addr!(sBuf + 8)"));
}

/// `push {lr}; ldr r0, =0x2000000; ldrh r1, [r0]; adds r1, #1; strh r1,
/// [r0]; bl f; pop {r0}; bx r0` and `f: bx lr`, in Thumb.
fn thumb_readable() -> (Vec<u8>, Discovery) {
    thumb_readable_at(0x0200_0000)
}

/// [`thumb_readable`], its halfword at `at` rather than 0x2000000.
fn thumb_readable_at(at: u32) -> (Vec<u8>, Discovery) {
    let (lo, hi) = (at as u16, (at >> 16) as u16);
    let (rom, mut d) = thumb_program(
        &[
            0xB500, 0x4804, 0x8801, 0x3101, 0x8001, 0xF000, 0xF805, 0xBC01, 0x4700, 0x46C0, lo, hi,
            0x4770,
        ],
        &[2, 2, 2, 2, 2, 4, 2, 2],
        &[0x0800_0001, 0x0800_0019],
    );
    let code = Code {
        size: 2,
        thumb: true,
        evidence: Evidence::Confirmed,
    };
    d.code.insert(0x0800_0018, code);
    (rom, d)
}

#[test]
fn the_readable_form_moves_writes_to_the_exits() {
    use crate::lower::fn_name;
    let (rom, d) = thumb_readable();
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut ctx = Ctx::new(&rom, &d);
    ctx.readable = Some(&list);
    let r = build_all(&ctx);
    let f = r.funcs.iter().find(|f| f.entry == 0x0800_0001).unwrap();
    let text = pretty(lower_func_checked(&ctx, f, &fn_name(f.entry)).0);
    // The machine's registers are written only in the exits' sync blocks.
    assert!(!text.contains("m.cpu.r[0] ="), "{text}");
    assert!(!text.contains("m.bus."), "{text}");
    for s in [
        "push!(m, 0x0800_0000,",
        // The pool load folded into the access that uses it.
        "m.get_via(0x0800_0002, 0x0800_0014, 0x0800_0004, Ptr::<u16>::at(0x200_0000))",
        "m.set(0x0800_0008, Ptr::<u16>::at(0x200_0000),",
        "call!(m, pc, f_08000019 -> 0x0800_000f,",
        "pop!(m, 0x0800_000e,",
        "ret!(m, pc @ 0x0800_0010, Exit::Exchange(",
        "regs!(m; 0 =",
    ] {
        let flat = |t: &str| t.split_whitespace().collect::<String>();
        assert!(flat(&text).contains(&flat(s)), "{s} in {text}");
    }
    assert!(!text.contains("m.pool("), "{text}");
    // `call!` writes lr from its return site; the sync doesn't repeat it.
    assert!(!text.contains("14 = 0x800_000f"), "{text}");
    // The call goes on only at its return site: no `match pc`.
    assert!(!text.contains("match pc"), "{text}");
}

/// `thumb_readable`'s function printed with `clock`, readable or not.
fn clocked(clock: crate::func::Clock, readable: bool) -> String {
    use crate::lower::fn_name;
    let (rom, d) = thumb_readable();
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut ctx = Ctx::new(&rom, &d);
    ctx.cfg.clock = clock;
    if readable {
        ctx.readable = Some(&list);
    }
    let r = build_all(&ctx);
    let f = r.funcs.iter().find(|f| f.entry == 0x0800_0001).unwrap();
    let text = pretty(lower_func_checked(&ctx, f, &fn_name(f.entry)).0);
    text.split_whitespace().collect()
}

#[test]
fn without_the_timing_model_no_access_carries_an_address() {
    use crate::func::Clock;
    for clock in [Clock::Static, Clock::Instant] {
        let text = clocked(clock, true);
        for s in [
            "push!(m,",
            // No pool load: only the access through the word it loads.
            "m.get(Ptr::<u16>::at(0x200_0000))",
            "m.set(Ptr::<u16>::at(0x200_0000),",
            "pop!(m,",
            // The exit's own address stays: it is where the CPU goes on.
            "ret!(m,pc@0x0800_0010,Exit::Exchange(",
        ] {
            assert!(text.contains(s), "{clock:?}: {s} in {text}");
        }
        for s in [
            "rt::",
            "0x0800_0000",
            "0x0800_0002",
            "0x0800_0004",
            "0x0800_0008",
            "0x0800_0014",
        ] {
            assert!(!text.contains(s), "{clock:?}: {s} in {text}");
        }
    }
    // Exact keeps both.
    let text = clocked(Clock::Exact, true);
    assert!(text.contains("rt::fetch(m,8,true,"), "{text}");
    assert!(text.contains("push!(m,0x0800_0000,"), "{text}");
}

#[test]
fn each_clock_prices_a_block_by_its_own_rule() {
    use crate::func::Clock;
    // Up to the `bl`: push, ldr, ldrh, adds, strh and the `bl`'s two
    // halves. Instant: their listed cycles, no wait states; static: the
    // same plus WAITCNT 0x4317's 3/1 cart waits.
    let call = |c| {
        let t = clocked(c, true);
        let k = "call!(m,pc,f_08000019->0x0800_000f,";
        let at = t.find(k).unwrap_or_else(|| panic!("{t}")) + k.len();
        t[at..].split(',').next().unwrap().to_string()
    };
    let ideal: u32 = call(Clock::Instant).parse().unwrap();
    let stat: u32 = call(Clock::Static).parse().unwrap();
    assert!(ideal < stat, "{ideal} {stat}");
    assert!(call(Clock::Exact).contains("rt::fetch"));
    // The play view: one constant, no stalls, and the tick that drops the
    // data accesses' cycles.
    for clock in [Clock::Static, Clock::Instant] {
        let t = clocked(clock, false);
        assert!(t.contains("rt::tick_flat(m,"), "{t}");
        for s in ["rt::stall", "rt::fetch", "rt::block", "rt::tick(m,"] {
            assert!(!t.contains(s), "{clock:?}: {s} in {t}");
        }
    }
}

#[test]
fn a_typed_datum_prints_as_its_field() {
    use crate::names::{SourceMap, sha1_hex};
    let (rom, d) = thumb_readable();
    let sha = sha1_hex(&rom);
    let map = format!(
        r#"[map]
rom_sha1 = "{sha}"
[functions]
0x08000001 = {{ name = "Bump" }}
[data]
0x02000000 = {{ name = "gPair", size = 4, type = "struct Pair" }}
[types."struct Pair"]
kind = "struct"
size = 4
fields = [
  {{ name = "count", off = 0, size = 2, type = "u16" }},
  {{ name = "other", off = 2, size = 2, type = "s16" }},
]
"#
    );
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut ctx = Ctx::new(&rom, &d);
    ctx.readable = Some(&list);
    ctx.names = Some(&names);
    let r = build_all(&ctx);
    let f = r.funcs.iter().find(|f| f.entry == 0x0800_0001).unwrap();
    let text = pretty(lower_func_checked(&ctx, f, &ctx.fn_ident(f.entry)).0);
    for s in [
        "m.get_via(0x0800_0002, 0x0800_0014, 0x0800_0004, gPair.count())",
        "m.set(0x0800_0008, gPair.count(),",
    ] {
        let flat = |t: &str| t.split_whitespace().collect::<String>();
        assert!(flat(&text).contains(&flat(s)), "{s} in {text}");
    }
}

/// `push {lr}; ldr r0, =0x2000000; ldrb r1, [r0, #1]; movs r2, #0x38;
/// bics r1, r2; movs r2, #0x10; orrs r1, r2; strb r1, [r0, #1]; bl f;
/// pop {r0}; bx r0` and `f: bx lr`, in Thumb, named from a map whose
/// `struct Opts` has a bitfield `frame` of `w` bits from bit 11: the
/// function's print.
fn bit_write_print(w: u32) -> String {
    bit_write_print_of(
        &[
            0xB500, 0x4805, 0x7841, 0x2238, 0x4391, 0x2210, 0x4311, 0x7041, 0xF000, 0xF804, 0xBC01,
            0x4700, 0x0000, 0x0200, 0x4770,
        ],
        &[2, 2, 2, 2, 2, 2, 2, 2, 4, 2, 2],
        0x0800_001c,
        (3, w),
    )
}

/// The print of `SetFrame`, Thumb `halves` of instructions `sizes` long
/// and `f: bx lr` at `f`, named as [`bit_write_print`]'s but with `speed`
/// and `frame` `w.0` and `w.1` bits.
fn bit_write_print_of(halves: &[u16], sizes: &[u32], f: u32, w: (u32, u32)) -> String {
    use crate::names::{SourceMap, sha1_hex};
    let (rom, mut d) = thumb_program(halves, sizes, &[0x0800_0001, f | 1]);
    let code = Code {
        size: 2,
        thumb: true,
        evidence: Evidence::Confirmed,
    };
    d.code.insert(f, code);
    let sha = sha1_hex(&rom);
    let map = format!(
        r#"[map]
rom_sha1 = "{sha}"
[functions]
0x08000001 = {{ name = "SetFrame" }}
[data]
0x02000000 = {{ name = "gOpts", size = 4, type = "struct Opts" }}
[types."struct Opts"]
kind = "struct"
size = 4
fields = [
  {{ name = "speed", off = 0, size = 4, type = "u32", bits = [8, {}] }},
  {{ name = "frame", off = 0, size = 4, type = "u32", bits = [11, {}] }},
]
"#,
        w.0, w.1
    );
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut ctx = Ctx::new(&rom, &d);
    ctx.readable = Some(&list);
    ctx.names = Some(&names);
    let r = build_all(&ctx);
    let f = r.funcs.iter().find(|f| f.entry == 0x0800_0001).unwrap();
    pretty(lower_func_checked(&ctx, f, &ctx.fn_ident(f.entry)).0)
}

#[test]
fn a_bitfield_write_prints_as_a_field_set() {
    // (Without the trailing comma a long call gets.)
    let flat = |t: &str| t.split_whitespace().collect::<String>().replace(",)", ")");
    // Bits 3-5 of byte 1 cleared and 2 put in: `gOpts.frame = 2`, the
    // value stored synced as r1.
    let text = bit_write_print(3);
    let want = "= m.set_bits_via(0x0800_0002, 0x0800_0018, 0x0800_0004, 0x0800_000e, \
                gOpts.frame(), 2);";
    assert!(flat(&text).contains(&flat(want)), "{want} in {text}");
    assert!(!text.contains("get_via"), "{text}");
    // Bits that are no one field's: the load and the store.
    let text = bit_write_print(2);
    assert!(!text.contains("set_bits"), "{text}");
    for s in ["m.get_via(0x0800_0002,", "m.set(0x0800_000e,"] {
        assert!(flat(&text).contains(&flat(s)), "{s} in {text}");
    }
}

/// `push {lr}; ldr r0, =0x2000000; ldrb r1, [r0, #1]; movs r2, #7;
/// bics r1, r2; ands r2, r3; orrs r1, r2; strb r1, [r0, #1]; movs r2, #0;
/// bl f; pop {r0}; bx r0`: [`bit_write_print_of`] with `speed` `w` bits.
fn computed_bit_write_print(w: u32) -> String {
    bit_write_print_of(
        &[
            0xB500, 0x4806, 0x7841, 0x2207, 0x4391, 0x401A, 0x4311, 0x7041, 0x2200, 0xF000, 0xF805,
            0xBC01, 0x4700, 0x0000, 0x0000, 0x0200, 0x4770,
        ],
        &[2, 2, 2, 2, 2, 2, 2, 2, 2, 4, 2, 2],
        0x0800_0020,
        (w, 5),
    )
}

#[test]
fn a_computed_bitfield_write_prints_as_a_field_set() {
    let flat = |t: &str| t.split_whitespace().collect::<String>().replace(",)", ")");
    // Bits 0-2 of byte 1 cleared and r3's low three put in: `gOpts.speed
    // = r3`.
    let text = computed_bit_write_print(3);
    let want = "m.set_bits_via(0x0800_0002, 0x0800_001c, 0x0800_0004, 0x0800_000e, \
                gOpts.speed(),";
    assert!(flat(&text).contains(&flat(want)), "{want} in {text}");
    assert!(!text.contains("get_via"), "{text}");
    // Bits that are no one field's: the load and the store.
    let text = computed_bit_write_print(2);
    assert!(!text.contains("set_bits"), "{text}");
    for s in ["m.get_via(0x0800_0002,", "m.set(0x0800_000e,"] {
        assert!(flat(&text).contains(&flat(s)), "{s} in {text}");
    }
}

#[test]
fn an_io_register_prints_by_name() {
    use crate::names::{SourceMap, sha1_hex};
    let (rom, d) = thumb_readable_at(0x0400_0104);
    let sha = sha1_hex(&rom);
    let map = format!("[map]\nrom_sha1 = \"{sha}\"\n");
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut ctx = Ctx::new(&rom, &d);
    ctx.readable = Some(&list);
    ctx.names = Some(&names);
    let r = build_all(&ctx);
    let f = r.funcs.iter().find(|f| f.entry == 0x0800_0001).unwrap();
    let text = pretty(lower_func_checked(&ctx, f, &ctx.fn_ident(f.entry)).0);
    for s in [
        "m.get_via(0x0800_0002, 0x0800_0014, 0x0800_0004, REG_TM1CNT_L)",
        "m.set(0x0800_0008, REG_TM1CNT_L,",
    ] {
        let flat = |t: &str| t.split_whitespace().collect::<String>();
        assert!(flat(&text).contains(&flat(s)), "{s} in {text}");
    }
    let globals = names.typed.module(&names).to_string();
    assert!(
        globals.contains("pub const REG_TM1CNT_L : Ptr < u16 >"),
        "{globals}"
    );
}

#[test]
fn a_function_key_in_the_pool_prints_as_its_name() {
    use crate::names::check::check_func;
    use crate::names::{SourceMap, sha1_hex};
    let (rom, d) = pool_calls();
    let sha = sha1_hex(&rom);
    let map = format!(
        "[map]\nrom_sha1 = \"{sha}\"\n[functions]\n0x08000000 = {{ name = \"CallBoth\" }}\n\
         0x03001000 = {{ name = \"IwramFn\" }}\n[data]\n0x02000000 = {{ name = \"sBuf\", size = 16 }}\n"
    );
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let plain = Ctx::new(&rom, &d);
    let mut named = Ctx::new(&rom, &d);
    named.names = Some(&names);
    let r = build_all(&plain);
    let n = check_func(&plain, &named, &r.funcs[0]).unwrap();
    assert_eq!(n.addrs, 2);
    let text = pretty(lower_func_checked(&named, &r.funcs[0], &named.fn_ident(0x0800_0000)).0);
    assert!(text.contains("addr!(IwramFn)"), "{text}");
}

/// `ldr r0, =0x8000010; ldr r1, =0x8000014; bx lr`, in Thumb, then 44
/// bytes of data: with no `[text]`, each word's bytes print, the first up
/// to where the second points, the second cut at the cap
/// (`crates/codegen/data-preview.toml`).
#[test]
fn rom_data_with_no_table_prints_its_bytes() {
    use crate::lower::fn_name;
    use crate::names::check::check_func;
    use crate::names::{SourceMap, sha1_hex};
    let mut halves = vec![
        0x4801, 0x4902, 0x4770, 0x46C0, 0x0010, 0x0800, 0x0014, 0x0800,
    ];
    halves.extend((0..22u16).map(|i| (2 * i + 2) << 8 | (2 * i + 1)));
    let (rom, mut d) = thumb_program(&halves, &[2, 2, 2, 2], &[0x0800_0001]);
    d.data.push((0x0800_0008, 0x0800_0010));
    let sha = sha1_hex(&rom);
    let map =
        format!("[map]\nrom_sha1 = \"{sha}\"\n[data]\n0x08000014 = {{ name = \"sTable\" }}\n");
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut plain = Ctx::new(&rom, &d);
    plain.readable = Some(&list);
    let mut named = Ctx::new(&rom, &d);
    named.names = Some(&names);
    named.readable = Some(&list);
    assert_eq!(
        named.pointed().iter().copied().collect::<Vec<_>>(),
        [0x0800_0010, 0x0800_0014]
    );
    let r = build_all(&plain);
    let n = check_func(&plain, &named, &r.funcs[0]).unwrap();
    assert_eq!(n.previews, 2);
    let text = pretty(lower_func_checked(&named, &r.funcs[0], &named.fn_ident(0x0800_0001)).0);
    let cap = crate::text::Preview::get().max_bytes;
    let rest: Vec<String> = (5..5 + cap).map(|b| format!("0x{b:02x}")).collect();
    for s in [
        "0x08000010: [u8; 4] = [0x01, 0x02, 0x03, 0x04]; encoding unknown".to_string(),
        format!(
            "sTable: [u8; {cap}] = [{}]; encoding unknown, cut at {cap}",
            rest.join(", ")
        ),
    ] {
        assert!(text.contains(&s), "{s} in {text}");
    }
    // The play view, and the unnamed readable print, show no bytes.
    let unnamed = pretty(lower_func_checked(&plain, &r.funcs[0], &fn_name(0x0800_0001)).0);
    assert!(!unnamed.contains("encoding unknown"), "{unnamed}");
}

/// `ClearBattleMonForms`' shape: `push {lr}; ldr r1, =0x2000000; movs r2,
/// #0; adds r0, r1, #3; 1: strb r2, [r0]; subs r0, #1; cmp r0, r1; bge 1b;
/// pop {r0}; bx r0`, in Thumb.
#[test]
fn a_counted_loop_prints_as_for() {
    use crate::lower::fn_name;
    let (rom, d) = thumb_program(
        &[
            0xB500, 0x4904, 0x2200, 0x1CC8, 0x7002, 0x3801, 0x4288, 0xDAFB, 0xBC01, 0x4700, 0x0000,
            0x0200,
        ],
        &[2; 10],
        &[0x0800_0001],
    );
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut ctx = Ctx::new(&rom, &d);
    ctx.readable = Some(&list);
    let r = build_all(&ctx);
    let text = pretty(lower_func_checked(&ctx, &r.funcs[0], &fn_name(0x0800_0001)).0);
    let flat = |t: &str| t.split_whitespace().collect::<String>();
    for s in [
        // The bound and the zero stored are carried in locals.
        "let mut r1: u32;",
        "(r1, r2, r13) = (0x200_0000, 0, ",
        // The entry's tick checks the variable's start after a yield.
        "; 0 = 0x200_0003, 1 = r1, 2 = r2, 13 = r13",
        "'l1: for r0 in (0x200_0000_u32..=0x200_0003).rev() {",
        "m.set(0x0800_0008, Ptr::<u8>::at(r0), r2);",
        // The branch back checks the value the variable steps to.
        "; 0 = v7, 1 = r1, 2 = r2, 13 = r13",
        "unreachable!();",
    ] {
        assert!(flat(&text).contains(&flat(s)), "{s} in {text}");
    }
    // The loop's block reads no register from `Machine`.
    assert!(!text.contains("m.cpu.r[0]"), "{text}");
    assert!(!text.contains("let mut r0"), "{text}");
}

#[test]
fn dwarf_names_values_and_comments_lines_and_calls() {
    use crate::names::check::check_func;
    use crate::names::{SourceMap, sha1_hex};
    let (rom, d) = thumb_readable();
    let sha = sha1_hex(&rom);
    // `count` in r1 and `ptr` in r0 over the whole function: the pop's
    // r0 (the return address) isn't `ptr`.
    let map = format!(
        r#"[map]
rom_sha1 = "{sha}"
[functions]
0x08000001 = {{ name = "Bump", src = "src/main.c", proto = "void Bump(void)" }}
0x08000019 = {{ name = "f", proto = "u16 f(u16 *p)" }}
[locals]
0x08000001 = [
  {{ name = "count", reg = 1, from = 0x08000000, to = 0x08000014, type = "u16" }},
  {{ name = "ptr", reg = 0, from = 0x08000000, to = 0x08000014, type = "u16 *" }},
]
[lines]
0x08000000 = 10
0x08000004 = 11
0x0800000e = "src/other.c:12"
"#
    );
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut plain = Ctx::new(&rom, &d);
    plain.readable = Some(&list);
    plain.cfg.insn = Insn::Asm;
    let mut named = Ctx::new(&rom, &d);
    named.readable = Some(&list);
    named.names = Some(&names);
    named.cfg.insn = Insn::Asm;
    let r = build_all(&plain);
    let f = r.funcs.iter().find(|f| f.entry == 0x0800_0001).unwrap();
    let text = pretty(lower_func_checked(&named, f, &named.fn_ident(f.entry)).0);
    for s in [
        "    // src/main.c:10\n    insn!(0x0800_0000,",
        "    // src/main.c:11\n    insn!(0x0800_0004,",
        "    // src/other.c:12\n    insn!(0x0800_000e,",
        "    // r0 = f(p: r0): u16\n    call!(",
        "1 = count",
    ] {
        assert!(text.contains(s), "{s} in {text}");
    }
    assert!(!text.contains("ptr"), "{text}");
    assert!(!text.contains("note!"), "{text}");
    let n = check_func(&plain, &named, f).unwrap();
    assert_eq!((n.vars_named, n.lines, n.calls), (1, 3, 1), "{n:?}");
    // The play view, and the unnamed readable print, name nothing.
    let unnamed = pretty(lower_func_checked(&plain, f, &crate::lower::fn_name(f.entry)).0);
    assert!(
        !unnamed.contains("count") && !unnamed.contains("//"),
        "{unnamed}"
    );
}

/// `0: movs r0, #1; b 8; f: cmp r0, #0; beq 0; 8: bx lr`, in Thumb: a
/// block below the entry, as on tmc 0x08000ee1 and 0x080af7d5, katam
/// 0x0814f909 and Rogue's build 0x0800c719, where the line in effect at
/// the block's start was looked up from the entry up to it, a backwards
/// range, and the readable print panicked (mod/readable-print-btree-panic).
#[test]
fn a_block_below_the_entry_prints_readable_and_named() {
    use crate::names::{SourceMap, sha1_hex};
    let (rom, d) = thumb_program(
        &[0x2001, 0xE001, 0x2800, 0xD0FB, 0x4770],
        &[4, 4, 2],
        &[0x0800_0005],
    );
    let sha = sha1_hex(&rom);
    let map = format!(
        "[map]\nrom_sha1 = \"{sha}\"\n[functions]\n\
         0x08000005 = {{ name = \"Low\", src = \"low.o\" }}\n[lines]\n0x08000004 = 3\n"
    );
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let list = std::collections::BTreeSet::from([0x0800_0005]);
    let mut ctx = Ctx::new(&rom, &d);
    ctx.readable = Some(&list);
    ctx.names = Some(&names);
    ctx.cfg.insn = Insn::Asm;
    let r = build_all(&ctx);
    let f = r.funcs.iter().find(|f| f.entry == 0x0800_0005).unwrap();
    let text = pretty(lower_func_checked(&ctx, f, &ctx.fn_ident(f.entry)).0);
    // The row in effect at the entry and at the block after it; none for
    // the block below it.
    let low = text.find("insn!(0x0800_0000,").expect(&text);
    assert!(!text[..low].trim_end().ends_with("low.o:3"), "{text}");
    assert!(
        text.contains("// low.o:3\n    insn!(0x0800_0008,"),
        "{text}"
    );
}

#[test]
fn a_note_prints_as_a_comment() {
    let t = pretty(quote::quote!(
        fn f() {
            note!("src/a.c:1");
            let x = 1;
        }
    ));
    assert_eq!(t, "fn f() {\n    // src/a.c:1\n    let x = 1;\n}\n");
}

#[test]
fn a_long_note_prints_as_one_comment() {
    let long = "r0 = HasTrainerLeftPartnersList(trainerId: r0, name: r1): bool32 and more and more";
    let t = pretty(quote::quote!(
        fn f() {
            if a {
                if b {
                    if c {
                        if d {
                            note!(#long);
                            let x = 1;
                        }
                    }
                }
            }
        }
    ));
    assert!(t.contains(&format!("// {long}\n")), "{t}");
    assert!(!t.contains("note!"), "{t}");
}

/// The ROM ranges const-prop folds loads from (`Code::folded`): the pool
/// word, and the byte read through the address the pool gave.
#[test]
fn folded_lists_the_rom_loads_const_prop_reads() {
    let (rom, d) = program(
        &[
            0xE59F_0004, // 0x08000000 ldr r0, [pc, #4] (0x0800000C)
            0xE5D0_1000, // 0x08000004 ldrb r1, [r0]
            0xE12F_FF1E, // 0x08000008 bx lr
            0x0800_0011, // 0x0800000C the pool word
            0x0000_0000,
        ],
        &[0x0800_0000],
    );
    let ctx = Ctx::new(&rom, &d);
    let r = build_all(&ctx);
    assert_eq!(
        crate::emit::folded(&ctx, &r.funcs),
        [(0x0800_000C, 0x0800_0010), (0x0800_0011, 0x0800_0012)]
    );
}

/// `ldr r0, =0x2000000; ldrh r1, [r0]; cmp r1, #2; movs r1, #1; strh r1,
/// [r0]; bx lr`, in Thumb: a compare and a store of a field.
fn thumb_field_values() -> (Vec<u8>, Discovery) {
    thumb_program(
        &[
            0x4802, 0x8801, 0x2902, 0x2101, 0x8001, 0x4770, 0x0000, 0x0200,
        ],
        &[2, 2, 2, 2, 2, 2],
        &[0x0800_0001],
    )
}

#[test]
fn a_field_s_values_print_by_name() {
    use crate::names::{SourceMap, sha1_hex};
    let (rom, d) = thumb_field_values();
    let sha = sha1_hex(&rom);
    let map = format!(
        r#"[map]
rom_sha1 = "{sha}"
[functions]
0x08000001 = {{ name = "SetMode" }}
[data]
0x02000000 = {{ name = "gPair", size = 4, type = "struct Pair" }}
[types."struct Pair"]
kind = "struct"
size = 4
fields = [
  {{ name = "mode", off = 0, size = 2, type = "u16" }},
  {{ name = "other", off = 2, size = 2, type = "u16" }},
]
[values."PAIR_MODE"]
file = "include/constants/pair.h"
fields = ["struct Pair.mode"]
names = [
  {{ name = "PAIR_MODE_OFF", value = 0 }},
  {{ name = "PAIR_MODE_ON", value = 1 }},
  {{ name = "PAIR_MODE_AUTO", value = 2 }},
]
"#
    );
    let names = SourceMap::from_texts(&sha, &[&map]).unwrap();
    let list = std::collections::BTreeSet::from([0x0800_0001]);
    let mut named = Ctx::new(&rom, &d);
    named.readable = Some(&list);
    named.names = Some(&names);
    let r = build_all(&named);
    let f = r.funcs.iter().find(|f| f.entry == 0x0800_0001).unwrap();
    let text = pretty(lower_func_checked(&named, f, &named.fn_ident(f.entry)).0);
    let flat = |t: &str| t.split_whitespace().collect::<String>();
    for s in [
        "m.set(0x0800_0008, gPair.mode(), PAIR_MODE_ON)",
        "prim::add_with_carry(v",
        "!PAIR_MODE_AUTO, true)",
    ] {
        assert!(flat(&text).contains(&flat(s)), "{s} in {text}");
    }
    let globals = names.typed.module(&names).to_string();
    assert!(
        globals.contains("pub const PAIR_MODE_ON : u32 = 1 ;"),
        "{globals}"
    );
    // A local can't take a value's name (a `let` would match the const).
    assert_eq!(
        names.local_ident("PAIR_MODE_ON").as_deref(),
        Some("PAIR_MODE_ON_")
    );
}

#[test]
fn a_function_that_never_returns_stays_v1() {
    // `calls()`'s caller only calls and loops (`b .`): a main loop, which a
    // rollback's restore would leave block-timed for good if structured.
    let (rom, d) = calls();
    let ctx = Ctx::new(&rom, &d);
    let (main, callee) = (build_func(&ctx, 0x0800_0000), build_func(&ctx, 0x0800_0010));
    assert!(crate::lower::never_returns(&ctx, &main));
    assert!(!crate::lower::never_returns(&ctx, &callee));
    assert_eq!(
        crate::lower::plan_or_why(&ctx, &main).err(),
        Some(crate::lower::V1::NeverReturns)
    );
    assert!(crate::lower::plan_or_why(&ctx, &callee).is_ok());
    // Every block of it is a key, so a restore resumes it compiled.
    let r = build_all(&ctx);
    assert!(main.blocks.keys().all(|k| r.table.contains_key(k)));
}

#[test]
fn an_arm_ldr_pop_of_r4_restores_it() {
    // newlib's strcmp: `str r4, [sp, #-4]!` ... `ldr r4, [sp], #4`. A
    // single `ldr` rotates by its address's low bits, which are 0 at sp.
    let (rom, d) = program(
        &[
            0xE52D_4004, // str r4, [sp, #-4]!
            0xE3A0_4001, // mov r4, #1
            0xE49D_4004, // ldr r4, [sp], #4
            0xE12F_FF1E, // bx lr
        ],
        &[ROM_BASE],
    );
    let ctx = Ctx::new(&rom, &d);
    let sums = crate::saves::summarize(&ctx, &build_all(&ctx).funcs);
    let s = &sums.by_entry[&ROM_BASE];
    assert_eq!(s.holes, std::collections::BTreeSet::new());
    assert!(s.keeps);
}

#[test]
fn an_idle_loops_exit_syncs_in_native_code() {
    // `b L2; L1: ldr r0, [r1, #8]; L2: ands r0, r4; cmp r0, #0; bne L1;
    // nop; bx lr`: a DMA wait entered in its middle (Metroid's).
    let (rom, d) = thumb_program(
        &[0xE000, 0x6888, 0x4020, 0x2800, 0xD1FB, 0x46C0, 0x4770],
        &[2; 7],
        &[0x0800_0001],
    );
    let mut ctx = Ctx::new(&rom, &d);
    ctx.cfg.clock = crate::func::Clock::Native;
    let f = build_func(&ctx, 0x0800_0001);
    // The `bne` ends its block, so where it isn't taken the loop's exit
    // syncs as the interpreter does (native/play-native-metroid-interp).
    let x = exits(&ctx, &f.blocks[&0x0800_0005]);
    let idle: Vec<Idle> = x.iter().map(|x| x.idle).collect();
    assert_eq!(idle, [Idle::Edge, Idle::Exit], "{x:x?}");
    let text = pretty(lower_func(&ctx, &f));
    assert!(text.contains("rt::idle_edge(m)"), "{text}");
    assert!(text.contains("rt::idle_exit(m)"), "{text}");
}
