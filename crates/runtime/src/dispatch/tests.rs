extern crate std;

use super::*;
use crate::overlay::Overlay;
use crate::platform::Headless;
use std::boxed::Box;
use std::vec::Vec;

type M = Machine<Headless>;

/// ARM code at the ROM's start: a block recompiled by hand at 0x0800_0000,
/// then `mov r1, #5; b 0x08000000` for the interpreter.
fn machine() -> Box<M> {
    let words: [u32; 3] = [0xE1A0_0000, 0xE3A0_1005, 0xEAFF_FFFC];
    let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    Box::new(Machine::new(Box::leak(rom.into_boxed_slice()), Headless))
}

/// What codegen would emit for one block that adds 1 to r0 and falls
/// through to 0x0800_0004, which it doesn't own.
fn f_08000000(m: &mut M, mut pc: u32) -> Flow {
    loop {
        match pc {
            0x0800_0000 => {
                m.cpu.r[0] = m.cpu.r[0].wrapping_add(1);
                pc = 0x0800_0004;
                if tick(m, 1) {
                    return Flow::Jump(pc);
                }
                continue;
            }
            _ => return Flow::Jump(pc),
        }
    }
}

static CODE: Code<Headless> = Code {
    blocks: &[(0x0800_0000, f_08000000)],
    ram_blocks: &[],
    extents: &[],
    ram_extents: &[],
    inner: &[],
    inner_extents: &[],
    modded: &[],
    folded: &[],
    overlays: &[],
    rom_hash: 0,
    timing: crate::Timing::Exact,
    seeds_file: None,
    spins: &[],
};

#[test]
fn compiled_and_interpreted_take_turns() {
    let mut m = machine();
    let mut stats = Stats::default();
    let stop = run(&mut m, Some(&CODE), None, &mut stats);
    assert_eq!(stop, Stop::FrameEnd);
    // Each round trip is one compiled block and two interpreted
    // instructions.
    assert!(m.cpu.r[0] > 1000, "r0 = {}", m.cpu.r[0]);
    assert_eq!(m.cpu.r[1], 5);
    assert!(
        stats.calls.abs_diff(stats.interpreted / 2) <= 1,
        "{stats:?}"
    );
    assert_eq!(&stats.seeds[..stats.n_seeds], &[0x0800_0004]);
    // The hand-written block ticks one cycle; the rest of the frame ran
    // in the interpreter, nothing halted.
    assert_eq!(stats.compiled_cycles, stats.calls, "{stats:?}");
    assert_eq!(stats.halted_cycles, 0);
    assert_eq!(stats.interpreted_ram_cycles, 0);
    assert_eq!(stats.compiled_cycles + stats.interpreted_cycles, m.now());
    // Every call is at the function's entry, and the frame ended in the
    // interpreter, so no compiled call yielded.
    assert_eq!((stats.resumes, stats.yields), (0, 0), "{stats:?}");
}

fn f_08000100(_: &mut M, pc: u32) -> Flow {
    Flow::Jump(pc)
}

#[test]
fn entries_are_each_functions_first_block() {
    static TWO: Code<Headless> = Code {
        blocks: &[
            (0x0800_0000, f_08000000),
            (0x0800_0010, f_08000000),
            (0x0800_0100, f_08000100),
            (0x0800_0104, f_08000100),
        ],
        ram_blocks: &[],
        extents: &[],
        ram_extents: &[],
        inner: &[],
        inner_extents: &[],
        modded: &[],
        folded: &[],
        overlays: &[],
        rom_hash: 0,
        timing: crate::Timing::Exact,
        seeds_file: None,
        spins: &[],
    };
    let entries: Vec<bool> = [
        0x0800_0000,
        0x0800_0010,
        0x0800_0100,
        0x0800_0104,
        0x0800_0200,
    ]
    .map(|k| TWO.is_entry(k))
    .to_vec();
    assert_eq!(entries, [true, false, true, false, false]);
}

#[test]
fn fallback_in_ram_is_counted() {
    // `b .` in EWRAM: the whole frame runs in the fallback, in RAM.
    let mut m = machine();
    m.bus.write32(0x0200_0000, 0xEAFF_FFFE);
    m.cpu.pc = 0x0200_0000;
    let mut stats = Stats::default();
    assert_eq!(run(&mut m, Some(&CODE), None, &mut stats), Stop::FrameEnd);
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.interpreted_ram_cycles, m.now());
    assert_eq!(stats.interpreted_cycles, m.now());
    assert_eq!(stats.n_seeds, 0);
}

#[test]
fn interpreter_alone_and_stop_at() {
    let mut m = machine();
    let mut stats = Stats::default();
    let stop = run(&mut m, None, Some(0x0800_0008), &mut stats);
    assert_eq!(stop, Stop::At(0x0800_0008));
    assert_eq!((m.cpu.pc, m.cpu.r[1]), (0x0800_0008, 5));
    assert_eq!(stats.interpreted, 2);
}

/// ARM code for [`settle`]: a compiled caller at 0x0800_0000 whose call
/// to 0x0800_0010 returns at 0x0800_0004; the callee tail-jumps to
/// `mov r1, #5; bx lr` at 0x0800_0020, which nothing compiled.
fn settle_machine() -> Box<M> {
    let mut words = [0xE1A0_0000u32; 10];
    words[8] = 0xE3A0_1005;
    words[9] = 0xE12F_FF1E;
    let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    Box::new(Machine::new(Box::leak(rom.into_boxed_slice()), Headless))
}

/// The caller as codegen emits a call: settled when it comes back
/// elsewhere, then on to its return site's block, which counts in r0 and
/// calls again.
fn f_caller(m: &mut M, mut pc: u32) -> Flow {
    loop {
        match pc {
            0x0800_0000 => {
                m.cpu.r[14] = 0x0800_0004;
                pc = 0x0800_0010;
                if tick(m, 1) {
                    return Flow::Jump(pc);
                }
                let sp0 = m.cpu.r[13];
                let Flow::Jump(a) = f_tail(m, 0x0800_0010);
                pc = settle(m, &SETTLE, a, 0x0800_0004, sp0);
                if yielding(m) {
                    return Flow::Jump(pc);
                }
                continue;
            }
            0x0800_0004 => {
                m.cpu.r[0] = m.cpu.r[0].wrapping_add(1);
                pc = 0x0800_0000;
                if tick(m, 1) {
                    return Flow::Jump(pc);
                }
                continue;
            }
            _ => return Flow::Jump(pc),
        }
    }
}

/// A tail call (`b 0x08000020`), to code that isn't compiled.
fn f_tail(m: &mut M, _: u32) -> Flow {
    let _ = tick(m, 1);
    Flow::Jump(0x0800_0020)
}

static SETTLE: Code<Headless> = Code {
    blocks: &[
        (0x0800_0000, f_caller),
        (0x0800_0004, f_caller),
        (0x0800_0010, f_tail),
    ],
    ram_blocks: &[],
    extents: &[],
    ram_extents: &[],
    inner: &[],
    inner_extents: &[],
    modded: &[],
    folded: &[],
    overlays: &[],
    rom_hash: 0,
    timing: crate::Timing::Exact,
    seeds_file: None,
    spins: &[],
};

#[test]
fn a_call_that_comes_back_elsewhere_is_settled_in_place() {
    let mut m = settle_machine();
    let mut stats = Stats::default();
    assert_eq!(run(&mut m, Some(&SETTLE), None, &mut stats), Stop::FrameEnd);
    // Every call came back through the interpreter, and the caller went
    // on without unwinding: the dispatcher called it once, and the frame
    // end (in a tick of the caller's or a nested run's) is the only
    // unwind.
    assert!(m.cpu.r[0] > 1000, "r0 = {}", m.cpu.r[0]);
    assert_eq!(m.cpu.r[1], 5);
    assert!(stats.calls <= 2, "{stats:?}");
    assert_eq!(stats.resumes, 0, "{stats:?}");
    assert!(stats.settles >= m.cpu.r[0] as u64, "{stats:?}");
    assert!(stats.unsettled <= 1, "{stats:?}");
    assert_eq!(stats.max_depth, 1);
    assert_eq!(m.nest.depth, 0);
    // Nested runs count their own cycles, and only once: two instructions
    // a settle (one, if the frame ended between them).
    assert_eq!(stats.compiled_cycles + stats.interpreted_cycles, m.now());
    assert!(
        stats.interpreted.abs_diff(2 * stats.settles) <= 1,
        "{stats:?}"
    );
}

#[test]
fn settle_stops_only_with_the_callers_sp() {
    // The guest reaches the return site with another sp: not the caller's
    // return, so the nested run goes on to the next one that is.
    let mut m = settle_machine();
    let mut stats = Stats::default();
    m.nest.stats = &mut stats;
    let sp = m.cpu.r[13];
    m.cpu.r[14] = 0x0800_0004;
    let _ = settle(
        &mut m,
        &SETTLE,
        0x0800_0020,
        0x0800_0004,
        sp.wrapping_sub(8),
    );
    m.nest.stats = core::ptr::null_mut();
    // It ran on, through the caller's own settled calls a level deeper,
    // until the frame ended.
    assert!(m.sched.frame_ended);
    assert!(stats.unsettled >= 1 && stats.settles > 1000, "{stats:?}");
    assert_eq!((stats.resumes, stats.max_depth), (1, 2), "{stats:?}");
}

/// A compiled loop at 0x0800_0000 counting in r0, under an H-blank IRQ
/// whose handler (in IWRAM, interpreted) acknowledges it and counts in
/// IWRAM.
fn irq_machine() -> Box<M> {
    use crate::io::{irq, reg};
    let mut m = machine();
    let handler = [
        0xE280_0C02u32, // add r0, r0, #0x200
        0xE3A0_1002,    // mov r1, #2
        0xE1C0_10B2,    // strh r1, [r0, #2]: IF
        0xE59F_100C,    // ldr r1, [pc, #12]
        0xE591_2000,    // ldr r2, [r1]
        0xE282_2001,    // add r2, r2, #1
        0xE581_2000,    // str r2, [r1]
        0xE12F_FF1E,    // bx lr
        0x0300_0100,    // the count
    ];
    for (i, w) in handler.iter().enumerate() {
        m.bus.write32(0x0300_0000 + 4 * i as u32, *w);
    }
    m.bus.write32(crate::sched::IRQ_HANDLER_PTR, 0x0300_0000);
    m.bus.write16(0x0400_0000 + reg::DISPSTAT, 1 << 4);
    m.bus.write16(0x0400_0000 + reg::IE, irq::HBLANK);
    m.bus.write16(0x0400_0000 + reg::IME, 1);
    m
}

/// The loop as v1 emitted it: a tick that finds an IRQ unwinds.
fn f_loop_unwinds(m: &mut M, mut pc: u32) -> Flow {
    loop {
        match pc {
            0x0800_0000 => {
                m.cpu.r[0] = m.cpu.r[0].wrapping_add(1);
                pc = 0x0800_0000;
                if tick(m, 3) {
                    return Flow::Jump(pc);
                }
                continue;
            }
            _ => return Flow::Jump(pc),
        }
    }
}

/// The loop as codegen emits it: the IRQ is taken in the tick.
fn f_loop(m: &mut M, mut pc: u32) -> Flow {
    loop {
        match pc {
            0x0800_0000 => {
                m.cpu.r[0] = m.cpu.r[0].wrapping_add(1);
                pc = 0x0800_0000;
                if tick(m, 3)
                    && let Some(k) = yield_at(m, &LOOP, pc)
                {
                    return Flow::Jump(k);
                }
                continue;
            }
            _ => return Flow::Jump(pc),
        }
    }
}

static LOOP_UNWINDS: Code<Headless> = Code {
    blocks: &[(0x0800_0000, f_loop_unwinds)],
    ..LOOP
};

static LOOP: Code<Headless> = Code {
    blocks: &[(0x0800_0000, f_loop)],
    ram_blocks: &[],
    extents: &[],
    ram_extents: &[],
    inner: &[],
    inner_extents: &[],
    modded: &[],
    folded: &[],
    overlays: &[],
    rom_hash: 0,
    timing: crate::Timing::Exact,
    seeds_file: None,
    spins: &[],
};

#[test]
fn irqs_taken_in_the_tick_match_unwinding() {
    let (mut a, mut b) = (irq_machine(), irq_machine());
    let (mut sa, mut sb) = (Stats::default(), Stats::default());
    for _ in 0..3 {
        assert_eq!(
            run(&mut a, Some(&LOOP_UNWINDS), None, &mut sa),
            Stop::FrameEnd
        );
        assert_eq!(run(&mut b, Some(&LOOP), None, &mut sb), Stop::FrameEnd);
    }
    let irqs = b.bus.read32(0x0300_0100) as u64;
    assert!(irqs > 400, "{irqs} IRQs");
    // The same state, the same IRQs at the same ticks.
    assert_eq!(a.bus.read32(0x0300_0100) as u64, irqs);
    assert_eq!(
        (a.cpu.r, a.cpu.cpsr, a.now()),
        (b.cpu.r, b.cpu.cpsr, b.now())
    );
    // Unwinding, the dispatcher resumed the loop after each IRQ; taken
    // in the tick, it called the loop once a frame.
    assert!(sa.yields >= irqs && sa.calls > irqs, "{sa:?}");
    assert_eq!((sb.calls, sb.yields), (3, 3), "{sb:?}");
    assert_eq!((sb.irqs_in_place, sb.irqs_unwound), (irqs, 0), "{sb:?}");
    assert_eq!(sb.max_depth, 1);
    assert_eq!(b.nest.depth, 0);
    assert_eq!(sb.compiled_cycles + sb.interpreted_cycles, b.now());
    assert_eq!(sa.interpreted_cycles, sb.interpreted_cycles);
    assert!(sb.max_stack > 0 && sb.max_stack < 1 << 20, "{sb:?}");
}

/// The registers, pc, CPSR and clock.
type State = ([u32; 15], u32, u32, u64);

std::thread_local! {
    /// The state at each frame end [`frame_end_switch`] saw.
    static FRAME_ENDS: core::cell::RefCell<Vec<State>> =
        const { core::cell::RefCell::new(Vec::new()) };
}

/// A front end's [`Nest::frame_end`] that doesn't switch stacks: it notes
/// the state, and stops the run with a fault at the third frame end.
fn frame_end_switch(m: *mut ()) {
    // SAFETY: the run hands out its machine.
    let m = unsafe { &mut *(m as *mut M) };
    assert!(m.sched.frame_ended);
    FRAME_ENDS.with_borrow_mut(|v| {
        v.push((m.cpu.r, m.cpu.pc, m.cpu.cpsr, m.now()));
        if v.len() == 3 {
            m.fault = Some(crate::machine::Fault::Bios(0));
        }
    });
}

#[test]
fn a_guest_run_switches_at_the_frame_end_as_unwinding_stops() {
    let (mut a, mut b) = (irq_machine(), irq_machine());
    let (mut sa, mut sb) = (Stats::default(), Stats::default());
    let mut want = Vec::new();
    for _ in 0..3 {
        assert_eq!(run(&mut a, Some(&LOOP), None, &mut sa), Stop::FrameEnd);
        want.push((a.cpu.r, a.cpu.pc, a.cpu.cpsr, a.now()));
    }
    b.nest.frame_end = Some(frame_end_switch);
    assert_eq!(run_guest(&mut b, &LOOP, &mut sb), Stop::Fault);
    // The same state at every frame end, and the loop called once.
    assert_eq!(FRAME_ENDS.take(), want);
    assert_eq!((sb.calls, sb.resumes, sb.switches), (1, 0, 3), "{sb:?}");
    assert_eq!((sa.calls, sa.switches), (3, 0), "{sa:?}");
    assert!(sb.max_stack > 0 && sb.max_stack < 1 << 20, "{sb:?}");
    b.nest.forget_run();
    assert!(b.nest.frame_end.is_none() && b.nest.depth == 0);
}

#[test]
fn keys_and_lookup() {
    let mut cpu = Cpu {
        pc: 0x0800_0102,
        cpsr: armv4t::interp::T_BIT,
        ..Cpu::default()
    };
    assert_eq!(key(&cpu), 0x0800_0103);
    set_pc(&mut cpu, 0x0800_0201);
    assert_eq!(cpu.pc, 0x0800_0200);
    let mut bus = crate::Bus::new(&[]);
    assert!(CODE.lookup(&mut bus, 0x0800_0000).is_some());
    assert!(CODE.lookup(&mut bus, 0x0800_0001).is_none());
    assert_ne!(rom_hash(b"a"), rom_hash(b"b"));
}

/// ROM for the overlay tests: `b .` at the entry, then at 0x0800_0010 the
/// code crt0 would copy to 0x0300_0000: `add r0, r0, #1; b 0x03000000`, a
/// data word, and a second function, `bx lr`.
fn overlay_machine() -> Box<M> {
    let words: [u32; 8] = [
        0xEAFF_FFFE,
        0,
        0,
        0,
        0xE280_0001,
        0xEAFF_FFFD,
        0xDEAD_BEEF,
        0xE12F_FF1E,
    ];
    let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut m = Box::new(Machine::new(Box::leak(rom.into_boxed_slice()), Headless));
    for i in 0..4 {
        let v = m.bus.read32(0x0800_0010 + 4 * i);
        m.bus.write32(0x0300_0000 + 4 * i, v);
    }
    m.cpu.pc = 0x0300_0000;
    m
}

/// What codegen would emit for the overlay at 0x0300_0000: its loop,
/// guarded, with its two instructions as one block.
fn f_03000000(m: &mut M, mut pc: u32) -> Flow {
    loop {
        if !overlay_valid(m, 0) {
            return Flow::Jump(pc);
        }
        match pc {
            0x0300_0000 => {
                m.cpu.r[0] = m.cpu.r[0].wrapping_add(1);
                pc = 0x0300_0000;
                if tick_ram(m, 4) {
                    return Flow::Jump(pc);
                }
                continue;
            }
            _ => return Flow::Jump(pc),
        }
    }
}

fn f_0300000c(_m: &mut M, pc: u32) -> Flow {
    Flow::Jump(pc)
}

static OVERLAY_CODE: Code<Headless> = Code {
    blocks: &[],
    ram_blocks: &[
        (0x0300_0000, f_03000000, 0, 1),
        (0x0300_000C, f_0300000c, 1, 1),
    ],
    extents: &[],
    ram_extents: &[],
    inner: &[],
    inner_extents: &[],
    modded: &[],
    folded: &[],
    overlays: &[
        Overlay {
            ranges: &[(0x0300_0000, 8, 0x0800_0010)],
            image: None,
        },
        Overlay {
            ranges: &[(0x0300_000C, 4, 0x0800_001C)],
            image: None,
        },
    ],
    rom_hash: 0,
    timing: crate::Timing::Exact,
    seeds_file: None,
    spins: &[],
};

#[test]
fn overlay_runs_until_its_bytes_change() {
    let mut m = overlay_machine();
    let mut stats = Stats::default();
    assert_eq!(
        run(&mut m, Some(&OVERLAY_CODE), None, &mut stats),
        Stop::FrameEnd
    );
    // All of the frame in the overlay: one call, nothing interpreted.
    assert_eq!((stats.calls, stats.interpreted), (1, 0), "{stats:?}");
    assert!(m.cpu.r[0] > 1000);
    assert_eq!(stats.compiled_ram_cycles, stats.compiled_cycles);

    // A data word between the overlays: still valid.
    m.bus.write32(0x0300_0008, 7);
    let mut stats = Stats::default();
    run(&mut m, Some(&OVERLAY_CODE), None, &mut stats);
    assert_eq!(stats.interpreted, 0, "{stats:?}");
    assert_eq!(m.bus.overlays.invalidations, 0);

    // Its first instruction becomes `add r1, r1, #1`: the fallback runs
    // that, and r0 stays put.
    let r0 = m.cpu.r[0];
    m.bus.write32(0x0300_0000, 0xE281_1001);
    let mut stats = Stats::default();
    run(&mut m, Some(&OVERLAY_CODE), None, &mut stats);
    assert_eq!(stats.calls, 0, "{stats:?}");
    assert!(stats.interpreted > 1000, "{stats:?}");
    assert_eq!(m.cpu.r[0], r0);
    assert!(m.cpu.r[1] > 500);
    assert_eq!(m.bus.overlays.invalidations, 1);
    // The other overlay wasn't touched.
    assert!(m.bus.overlay_valid(1));

    // Writing the old bytes back makes it valid again.
    m.bus.write32(0x0300_0000, 0xE280_0001);
    let mut stats = Stats::default();
    run(&mut m, Some(&OVERLAY_CODE), None, &mut stats);
    assert_eq!(stats.interpreted, 0, "{stats:?}");
    assert!(m.cpu.r[0] > r0);
}

#[test]
fn overlay_waits_for_its_copy() {
    // Before crt0's copy IWRAM is zero: the key falls back.
    let mut m = overlay_machine();
    m.bus.write32(0x0300_0000, 0);
    m.bus.write32(0x0300_0004, 0);
    m.bus.overlays.install(OVERLAY_CODE.overlays);
    assert!(OVERLAY_CODE.lookup(&mut m.bus, 0x0300_0000).is_none());
    // Copied (the way CpuSet and DMA write: through the bus): found.
    for i in 0..2 {
        let v = m.bus.read32(0x0800_0010 + 4 * i);
        m.bus.write32(0x0300_0000 + 4 * i, v);
    }
    assert!(OVERLAY_CODE.lookup(&mut m.bus, 0x0300_0000).is_some());
    // A mirror isn't the overlay's key, but a store through one is a store
    // to it.
    assert!(OVERLAY_CODE.lookup(&mut m.bus, 0x0300_8000).is_none());
    m.bus.write8(0x0300_8001, 0xFF);
    assert!(OVERLAY_CODE.lookup(&mut m.bus, 0x0300_0000).is_none());
    assert_eq!(m.bus.overlays.invalidations, 1);
}

#[test]
fn call_into_a_stale_overlay_hands_the_key_back() {
    let mut m = overlay_machine();
    m.bus.overlays.install(OVERLAY_CODE.overlays);
    m.cpu.r[0] = 0;
    // Valid: `call` runs it (until the frame ends).
    assert_eq!(
        call(&mut m, &OVERLAY_CODE, 0x0300_0000),
        Flow::Jump(0x0300_0000)
    );
    assert!(m.cpu.r[0] > 0);
    // Rewritten: `call` runs nothing and returns the key for the
    // dispatcher to interpret.
    let r0 = m.cpu.r[0];
    m.bus.write32(0x0300_0004, 0xE1A0_0000);
    assert_eq!(
        call(&mut m, &OVERLAY_CODE, 0x0300_0000),
        Flow::Jump(0x0300_0000)
    );
    assert_eq!(m.cpu.r[0], r0);
}

#[test]
fn register_ram_reset_checks_the_overlays_again() {
    let mut m = overlay_machine();
    m.bus.overlays.install(OVERLAY_CODE.overlays);
    assert!(OVERLAY_CODE.lookup(&mut m.bus, 0x0300_0000).is_some());
    // RegisterRamReset (swi 0x01, ARM state) with r0 bit 1: clear IWRAM.
    m.cpu.r[0] = 2;
    m.swi(0x01_0000);
    assert!(m.fault.is_none());
    assert!(OVERLAY_CODE.lookup(&mut m.bus, 0x0300_0000).is_none());
    assert_eq!(m.bus.overlays.invalidations, 1);
}

/// `add r0, r0, #1; add r2, r2, #3; mov r1, #5; b 0x08000000`: the first
/// two a compiled block, the rest for the interpreter fallback.
const BLOCK_WORDS: [u32; 4] = [0xE280_0001, 0xE282_2003, 0xE3A0_1005, 0xEAFF_FFFB];

fn block_machine() -> Box<M> {
    let rom: Vec<u8> = BLOCK_WORDS.iter().flat_map(|w| w.to_le_bytes()).collect();
    Box::new(Machine::new(Box::leak(rom.into_boxed_slice()), Headless))
}

/// What codegen would emit for the two-instruction block: both run, one
/// tick with their summed cost at the exit to 0x0800_0008.
fn f_two(m: &mut M, mut pc: u32) -> Flow {
    loop {
        match pc {
            0x0800_0000 => {
                m.cpu.r[0] = m.cpu.r[0].wrapping_add(1);
                m.cpu.r[2] = m.cpu.r[2].wrapping_add(3);
                let c = BLOCK_WORDS[..2]
                    .iter()
                    .map(|&w| armv4t::cycles::cost(&armv4t::decode::decode_arm(w, 0)))
                    .fold(armv4t::cycles::Cost::default(), |a, c| a.plus(c));
                pc = 0x0800_0008;
                if tick(m, c.base + fetch(m, 8, false, c.seq, c.nonseq)) {
                    return Flow::Jump(pc);
                }
                continue;
            }
            _ => return Flow::Jump(pc),
        }
    }
}

static TWO: Code<Headless> = Code {
    blocks: &[(0x0800_0000, f_two)],
    ram_blocks: &[],
    extents: &[Extent { len: 2, stub: 0 }],
    ram_extents: &[],
    inner: &[],
    inner_extents: &[],
    modded: &[],
    folded: &[],
    overlays: &[],
    rom_hash: 0,
    timing: crate::Timing::Exact,
    seeds_file: None,
    spins: &[],
};

#[test]
fn block_timed_interpreter_matches_compiled_code() {
    let (mut a, mut b) = (block_machine(), block_machine());
    let (mut sa, mut sb) = (Stats::default(), Stats::default());
    for _ in 0..3 {
        run(&mut a, Some(&TWO), None, &mut sa);
        run_blocks(&mut b, &TWO, None, &mut sb);
    }
    assert!(a.cpu.r[0] > 1000, "r0 = {}", a.cpu.r[0]);
    assert_eq!(a.cpu, b.cpu);
    assert_eq!(a.now(), b.now());
    // Compiled code moves neither the prefetch queue nor open bus; nor
    // does the block-timed interpreter inside a block.
    assert_eq!(a.prefetch, b.prefetch);
    assert_eq!(a.bus.open_bus, b.bus.open_bus);
    assert_eq!(sa.compiled_cycles, sb.compiled_cycles);
    assert_eq!(sa.interpreted, sb.interpreted);
}

#[test]
fn a_block_ends_at_its_length_or_a_taken_branch() {
    let mut m = block_machine();
    let step = |m: &mut M, at: u32| {
        m.cpu.pc = at;
        let t = m.now();
        interp_block(m, Extent { len: 2, stub: 0 }, false);
        m.now() - t
    };
    // Two instructions, then the tick: one block's cost in one go.
    let two = step(&mut m, 0x0800_0000);
    assert_eq!((m.cpu.pc, m.cpu.r[0], m.cpu.r[2]), (0x0800_0008, 1, 3));
    // From the `mov`: the extent allows two, but the branch leaves after
    // it, so the block ends there.
    let _ = step(&mut m, 0x0800_0008);
    assert_eq!((m.cpu.pc, m.cpu.r[1]), (0x0800_0000, 5));
    assert!(two > 0);
}

/// A mod's patched range against the ROM ranges the code folded in.
#[test]
fn folded_in_finds_an_overlap() {
    let code: Code<Headless> = Code {
        folded: &[(0x0800_0100, 0x0800_0108), (0x0800_0200, 0x0800_0204)],
        ..CODE
    };
    assert_eq!(code.folded_in(0x0800_0000, 0x0800_0100), None);
    assert_eq!(
        code.folded_in(0x0800_0104, 0x0800_0110),
        Some((0x0800_0100, 0x0800_0108))
    );
    assert_eq!(code.folded_in(0x0800_0108, 0x0800_0200), None);
    assert_eq!(
        code.folded_in(0x0800_0000, 0x0800_1000),
        Some((0x0800_0100, 0x0800_0108))
    );
    assert_eq!(
        code.folded_in(0x0800_0203, 0x0800_0300),
        Some((0x0800_0200, 0x0800_0204))
    );
}

/// Thumb code for [`modcall`]: a compiled mod function at 0x0800_0000
/// whose added call goes to `adds r2, #1; bx lr` at 0x0800_0010, which
/// nothing compiled.
fn mod_call_machine() -> Box<M> {
    let mut hw = [0x46C0u16; 16];
    hw[8] = 0x3201;
    hw[9] = 0x4770;
    let rom: Vec<u8> = hw.iter().flat_map(|h| h.to_le_bytes()).collect();
    let mut m = Box::new(Machine::new(Box::leak(rom.into_boxed_slice()), Headless));
    m.cpu.cpsr |= armv4t::interp::T_BIT;
    m.cpu.pc = 0x0800_0000;
    m.cpu.r[13] = 0x0300_7f00;
    m.cpu.r[14] = 0x0800_0123;
    m
}

const MOD_KEY: u32 = modcall::call_key("tests.rs", 1, 1);

/// The mod function as `call!(m, pc, f)` expands in it, then a count in
/// r0 and a tick back to its entry.
fn f_mod(m: &mut M, mut pc: u32) -> Flow {
    loop {
        let (at, regs) = (pc, modcall::regs(m));
        let sp0 = modcall::enter(m, &regs, at, MOD_KEY);
        let Flow::Jump(a) = f_stub(m, 0x0800_0011);
        pc = settle(m, &MOD_CALL, a, MOD_KEY, sp0);
        if pc != MOD_KEY {
            return Flow::Jump(pc);
        }
        if yielding(m)
            && let Some(k) = yield_at(m, &MOD_CALL, MOD_KEY)
        {
            return Flow::Jump(k);
        }
        // The callee's r2 is its value; the registers are the block's.
        let r2 = m.cpu.r[2];
        modcall::leave(m, &regs);
        assert_eq!((m.cpu.r[14], m.cpu.r[2] + 1), (0x0800_0123, r2));
        m.cpu.r[2] = r2;
        pc = at;
        m.cpu.r[0] = m.cpu.r[0].wrapping_add(1);
        if tick(m, 1)
            && let Some(k) = yield_at(m, &MOD_CALL, pc)
        {
            return Flow::Jump(k);
        }
    }
}

/// A call through to code that isn't compiled.
fn f_stub(_: &mut M, key: u32) -> Flow {
    Flow::Jump(key)
}

static MOD_CALL: Code<Headless> = Code {
    blocks: &[(0x0800_0001, f_mod)],
    ..LOOP
};

#[test]
fn a_mod_call_returns_to_its_reserved_key() {
    assert!(modcall::is_call_key(MOD_KEY) && MOD_KEY & 3 == 1);
    assert_ne!(MOD_KEY, modcall::call_key("tests.rs", 2, 1));
    let mut m = mod_call_machine();
    let mut stats = Stats::default();
    m.cpu.pc = 0x0800_0000;
    assert_eq!(
        run(&mut m, Some(&MOD_CALL), None, &mut stats),
        Stop::FrameEnd
    );
    // Each call came back to the mod: one count a call, none lost.
    assert!(m.cpu.r[0] > 1000, "r0 = {}", m.cpu.r[0]);
    assert!(m.cpu.r[2].abs_diff(m.cpu.r[0]) <= 1, "{:?}", m.cpu.r);
    assert_eq!(
        (stats.mod_call_resumes, stats.mod_resumes),
        (0, 0),
        "{stats:?}"
    );
    assert!(m.fault.is_none());
}

#[test]
fn a_mod_call_whose_frame_is_lost_goes_on_where_its_block_began() {
    // Inside the call as a state load leaves it: the record below sp, the
    // CPU in the callee, no mod frame to come back to.
    let mut m = mod_call_machine();
    m.cpu.r[5] = 77;
    m.cpu.r[2] = 40;
    let regs = modcall::regs(&m);
    let sp = m.cpu.r[13];
    modcall::enter(&mut m, &regs, 0x0800_0001, MOD_KEY);
    m.cpu.r[5] = 0;
    m.cpu.pc = 0x0800_0010;
    let mut stats = Stats::default();
    m.nest.stats = &mut stats;
    let mut steps = 0;
    // The interpreter runs the callee to the key; the dispatcher puts the
    // record back and calls the mod again at its entry.
    while key(&m.cpu) != MOD_KEY {
        m.step();
        steps += 1;
    }
    assert_eq!((steps, m.cpu.r[2]), (2, 41));
    m.nest.stats = core::ptr::null_mut();
    let stop = run(&mut m, Some(&MOD_CALL), Some(0x0800_0001), &mut stats);
    assert_eq!(stop, Stop::At(0x0800_0001));
    assert_eq!((m.cpu.r[5], m.cpu.r[2], m.cpu.r[13]), (77, 40, sp));
    assert!(m.cpu.thumb());
    assert_eq!(
        (stats.mod_call_resumes, stats.mod_resumes, stats.resumes),
        (1, 1, 1),
        "{stats:?}"
    );
    // Then the mod runs on as before.
    assert_eq!(
        run(&mut m, Some(&MOD_CALL), None, &mut stats),
        Stop::FrameEnd
    );
    assert!(m.cpu.r[0] > 1000 && m.fault.is_none(), "{:?}", m.cpu.r);
}

#[test]
fn a_mod_call_key_with_no_record_faults() {
    let mut m = mod_call_machine();
    m.cpu.pc = MOD_KEY & !1;
    let mut stats = Stats::default();
    assert_eq!(run(&mut m, Some(&MOD_CALL), None, &mut stats), Stop::Fault);
    assert_eq!(m.fault, Some(crate::machine::Fault::ModCall(MOD_KEY)));
}
