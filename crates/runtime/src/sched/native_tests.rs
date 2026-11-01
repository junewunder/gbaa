//! `--timing native` (docs/native-port-design.md, sections 3 and 7): code
//! takes no time; the clock moves at idle back edges, stuck spin loops
//! and the BIOS's work, not at reads of the beam.

extern crate std;

use super::*;
use crate::Timing;
use crate::machine::ROM_ENTRY;
use crate::platform::Headless;
use crate::test_util::{IO, machine_with};

const NOP: u32 = 0xE1A0_0000; // mov r0, r0
const B_SELF: u32 = 0xEAFF_FFFE;

fn native(code: &[u32]) -> std::boxed::Box<Machine<Headless>> {
    let mut m = machine_with(code);
    m.set_timing(Timing::Native);
    m
}

#[test]
fn plain_instructions_move_the_clock_not_at_all() {
    let mut code = std::vec![NOP; 1000];
    code.push(B_SELF);
    let mut m = native(&code);
    for _ in 0..1000 {
        assert!(!m.step());
    }
    assert_eq!((m.now(), m.cpu.pc), (0, ROM_ENTRY + 4000));
    // `b .` is an idle loop: its back edge runs the clock to the next
    // event, H-blank's start.
    m.step();
    assert_eq!(m.now(), HBLANK_START);
    assert_eq!(m.bus.read16(IO + reg::DISPSTAT) & 2, 2);
}

/// `while (REG_VCOUNT != 100) ;` is an idle loop: a read moves no time
/// (the decision from Emerald's ten), each back edge half a line, so the
/// poll sees each line twice and leaves the beam at line 100's start.
#[test]
fn a_vcount_poll_exits_at_its_line() {
    let mut m = native(&[
        0xE3A0_1301, // mov r1, #0x0400_0000
        0xE281_1006, // add r1, r1, #6
        0xE1D1_00B0, // loop: ldrh r0, [r1]
        0xE350_0064, // cmp r0, #100
        0x1AFF_FFFC, // bne loop
        B_SELF,
    ]);
    let mut reads = 0;
    while m.cpu.pc != ROM_ENTRY + 0x14 {
        if m.cpu.pc == ROM_ENTRY + 8 {
            reads += 1;
        }
        m.step();
        assert!(reads <= 2 * 228, "the poll never ended");
    }
    assert_eq!((reads, m.cpu.r[0]), (201, 100));
    assert_eq!(m.now(), 100 * LINE_CYCLES);
    assert_eq!(m.bus.io.vcount(), 100);
}

/// A read of `DISPSTAT` or `VCOUNT` outside a poll moves no time.
#[test]
fn a_beam_read_moves_no_time() {
    let mut m = native(&[B_SELF]);
    for o in [reg::DISPSTAT, reg::VCOUNT] {
        armv4t::interp::Bus::load(&mut m.bus, armv4t::sem::Width::Half, IO + o);
    }
    assert_eq!(m.now(), 0);
}

/// `while (!busy_with_calls()) ;` (Emerald's `DoMapLoadLoop`) is a spin
/// loop: its first SPIN_BUDGET passes take no time, then each takes half
/// a line, until the frame ends, which starts its count again.
#[test]
fn a_spin_loop_takes_time_past_its_budget() {
    let mut m = native(&[
        0xEB00_0002, // loop: bl f
        0xE350_0000, // cmp r0, #0
        0x0AFF_FFFC, // beq loop
        B_SELF,
        0xE3A0_0000, // f: mov r0, #0
        0xE12F_FF1E, // bx lr
    ]);
    let mut passes = 0;
    while m.now() == 0 {
        if m.cpu.pc == ROM_ENTRY + 8 {
            passes += 1;
        }
        m.step();
        assert!(passes <= SPIN_BUDGET + 1);
    }
    assert_eq!((passes, m.now()), (SPIN_BUDGET + 1, HBLANK_START));
    // Half a line a pass from here: line 160's start ends the frame.
    let mut more = 0;
    while !m.sched.frame_ended {
        if m.cpu.pc == ROM_ENTRY + 8 {
            more += 1;
        }
        m.step();
    }
    assert_eq!(more, 2 * 160 - 1);
    assert_eq!(m.now(), 160 * LINE_CYCLES);
    // The frame's end cleared the count: SPIN_BUDGET free passes again.
    m.sched.frame_ended = false;
    for _ in 0..SPIN_BUDGET * 4 {
        m.step();
    }
    assert_eq!(m.now(), 160 * LINE_CYCLES);
}

/// Emerald's 96 KB VRAM fill (decision 12) still spans a V-blank: the
/// copies take their time under native timing, and the V-blank they cross
/// comes inside the call, as in a wait.
#[test]
fn a_96k_cpu_fast_set_crosses_a_vblank() {
    let mut m = native(&[
        0xEF0C_0000, // swi 0x0C (CpuFastSet)
        B_SELF,
    ]);
    // Start at line 150, ten lines before V-blank.
    m.bus.io.now = 150 * LINE_CYCLES;
    m.idle_edge();
    assert_eq!(m.bus.io.vcount(), 150);
    m.cpu.r[0] = 0x0200_0000;
    m.cpu.r[1] = 0x0600_0000;
    m.cpu.r[2] = 1 << 24 | 0x6000; // fill, 24576 words
    let mut ended = false;
    while m.cpu.pc != ROM_ENTRY + 4 {
        m.step();
        ended |= m.sched.frame_ended;
    }
    assert!(ended, "the fill ended at line {}", m.bus.io.vcount());
    assert!(m.bus.io.vcount() > 160 || m.bus.io.vcount() < 150);
    // The `swi` itself (its entry and exit) took none of it.
    let mut exact = machine_with(&[0xEF0C_0000, B_SELF]);
    exact.cpu.r[0] = 0x0200_0000;
    exact.cpu.r[1] = 0x0600_0000;
    exact.cpu.r[2] = 1 << 24 | 0x6000;
    exact.step();
    while exact.cpu.pc != ROM_ENTRY + 4 {
        exact.step();
    }
    let took = m.now() - 150 * LINE_CYCLES;
    assert!(took > 10 * LINE_CYCLES && took < exact.now(), "{took}");
}

/// An IRQ that becomes takeable between time points (`IME` written with
/// one pending) waits for the next one, as the native program's handlers
/// run only inside the library's steps.
#[test]
fn irqs_are_taken_only_where_time_passed() {
    let mut m = native(&[NOP, NOP, B_SELF]);
    m.bus.write16(IO + reg::IE, irq::VBLANK);
    m.bus.write16(IO + reg::IF, 0);
    m.bus.io.request_irq(irq::VBLANK);
    m.bus.write16(IO + reg::IME, 1);
    m.step();
    assert!(!m.irq_takeable());
    m.step();
    m.step(); // the idle edge
    assert!(m.irq_takeable());
}

/// An IRQ that comes due at a time point while the CPSR's I bit is set
/// (a wait inside a critical section) is held, and taken as soon as an
/// `msr` clears I, not at the next time point (native/lift-irq-hold).
#[test]
fn an_irq_held_by_the_i_bit_is_taken_when_msr_clears_it() {
    let mut m = native(&[
        B_SELF,      // the idle edge: half a line passes
        0xE129_F000, // msr cpsr_fc, r0
        NOP,
        B_SELF,
    ]);
    m.bus.write16(IO + reg::IE, irq::HBLANK);
    m.bus.write16(IO + reg::DISPSTAT, 1 << 4);
    m.bus.write16(IO + reg::IME, 1);
    m.cpu.cpsr |= armv4t::interp::I_BIT;
    m.cpu.r[0] = m.cpu.cpsr & !armv4t::interp::I_BIT;
    // H-blank comes due at the idle edge, with I set: held.
    assert!(!m.step());
    assert!(!m.irq_takeable());
    m.cpu.pc = ROM_ENTRY + 4;
    assert!(!m.take_irq());
    // The msr clears I: taken at once.
    assert!(m.step(), "the msr's tick yields to take it");
    assert_eq!(m.now(), HBLANK_START, "no time passed");
    assert!(m.take_irq());
}
