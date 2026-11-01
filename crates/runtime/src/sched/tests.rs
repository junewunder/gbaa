extern crate std;

use super::*;
use crate::io::key;
use crate::machine::{Fault, ROM_ENTRY, SP_IRQ, SP_SYS};
use crate::platform::{Frame, Headless};
use crate::test_util::{IO, machine, machine_with};
use armv4t::interp::{Bank, T_BIT};

const IWRAM: u32 = 0x0300_0000;
const B_SELF: u32 = 0xEAFF_FFFE;
const BX_LR: u32 = 0xE12F_FF1E;

fn stat(m: &Machine<Headless>) -> u16 {
    m.bus.read16(IO + reg::DISPSTAT)
}

fn if_(m: &Machine<Headless>) -> u16 {
    m.bus.read16(IO + reg::IF)
}

/// Tick to `cycles` after power-on.
fn tick_to(m: &mut Machine<Headless>, cycles: u64) {
    m.tick((cycles - m.now()) as u32);
}

#[test]
fn scanline_flags_and_vcount() {
    let mut m = machine();
    tick_to(&mut m, HBLANK_START - 1);
    assert_eq!((m.bus.io.vcount(), stat(&m) & 3), (0, 0));
    tick_to(&mut m, HBLANK_START);
    assert_eq!(stat(&m) & 3, 2);
    tick_to(&mut m, LINE_CYCLES);
    assert_eq!((m.bus.io.vcount(), stat(&m) & 3), (1, 0));
    tick_to(&mut m, LINE_CYCLES * 160);
    assert_eq!((m.bus.io.vcount(), stat(&m) & 3), (160, 1));
    tick_to(&mut m, LINE_CYCLES * 160 + HBLANK_START);
    assert_eq!(stat(&m) & 3, 3);
    tick_to(&mut m, LINE_CYCLES * 227);
    assert_eq!((m.bus.io.vcount(), stat(&m) & 3), (227, 0));
    tick_to(&mut m, FRAME_CYCLES);
    assert_eq!((m.bus.io.vcount(), stat(&m) & 3), (0, 0));
}

#[test]
fn dispstat_irqs() {
    let mut m = machine();
    // V-blank, H-blank and V-count IRQs on, V-count target 5.
    m.bus.write16(IO + reg::DISPSTAT, 5 << 8 | 7 << 3);
    tick_to(&mut m, HBLANK_START - 1);
    assert_eq!(if_(&m), 0);
    tick_to(&mut m, HBLANK_START);
    assert_eq!(if_(&m), irq::HBLANK);
    m.bus.write16(IO + reg::IF, 0xFFFF);
    tick_to(&mut m, LINE_CYCLES * 5 - 1);
    m.bus.write16(IO + reg::IF, 0xFFFF);
    tick_to(&mut m, LINE_CYCLES * 5);
    assert_eq!(if_(&m), irq::VCOUNT);
    assert_ne!(stat(&m) & 4, 0);
    tick_to(&mut m, LINE_CYCLES * 6);
    assert_eq!(stat(&m) & 4, 0);
    // A new target is compared at once.
    m.bus.write16(IO + reg::DISPSTAT, 6 << 8 | 7 << 3);
    assert_ne!(stat(&m) & 4, 0);
    m.bus.write16(IO + reg::IF, 0xFFFF);
    tick_to(&mut m, LINE_CYCLES * 160);
    assert_eq!(if_(&m), irq::VBLANK | irq::HBLANK);
}

/// A platform holding A down.
struct HoldA;

impl Platform for HoldA {
    fn present(&mut self, _frame: &Frame) {}
    fn keys(&mut self) -> u16 {
        key::A
    }
}

#[test]
fn run_frame_ends_at_vblank_and_polls_keys() {
    let rom: &'static [u8] = &[0xFE, 0xFF, 0xFF, 0xEA];
    let mut m = std::boxed::Box::new(Machine::new(rom, HoldA));
    // A `b .` from ROM at power-on's wait states: 1S + 1N + 1S, 3 + 5 + 7 + 5.
    const B: u64 = 20;
    m.run_frame();
    let first = m.now();
    assert!((LINE_CYCLES * 160..LINE_CYCLES * 160 + B).contains(&first));
    assert_eq!(m.bus.io.vcount(), 160);
    assert_eq!(m.bus.read16(IO + reg::KEYINPUT), key::ALL & !key::A);
    m.run_frame();
    assert!((first + FRAME_CYCLES - B..first + FRAME_CYCLES + B).contains(&m.now()));
}

fn enable_vblank_irq(m: &mut Machine<Headless>, ime: bool) {
    m.bus.write16(IO + reg::DISPSTAT, 1 << 3);
    m.bus.write16(IO + reg::IE, irq::VBLANK);
    m.bus.write16(IO + reg::IME, ime as u16);
}

#[test]
fn tick_yields_for_a_takeable_irq() {
    let mut m = machine();
    assert!(!m.tick(10));
    enable_vblank_irq(&mut m, true);
    assert!(!m.tick(10));
    m.bus.io.request_irq(irq::VBLANK);
    assert!(m.tick(1));
    // The CPSR's I bit holds it back; clearing it is seen at the next tick.
    m.cpu.cpsr |= I_BIT;
    assert!(!m.tick(1));
    assert!(!m.take_irq());
    m.cpu.cpsr &= !I_BIT;
    assert!(m.tick(1));
}

/// IRQ entry and the BIOS stub's return, for ARM and Thumb code.
#[test]
fn irq_entry_and_return() {
    for thumb in [false, true] {
        let mut m = machine();
        m.bus.write32(IWRAM, BX_LR);
        m.bus.write32(IRQ_HANDLER_PTR, IWRAM);
        enable_vblank_irq(&mut m, true);
        m.bus.io.request_irq(irq::VBLANK);
        for r in [0, 1, 2, 3, 12, 14] {
            m.cpu.r[r] = 0x100 + r as u32;
        }
        let pc = ROM_ENTRY + 2 * thumb as u32;
        m.cpu.pc = pc;
        if thumb {
            m.cpu.cpsr |= T_BIT;
        }
        let cpsr = m.cpu.cpsr;

        assert!(m.take_irq());
        assert_eq!(m.cpu.cpsr & 0x1F, mode::IRQ);
        assert_eq!(m.cpu.cpsr & (I_BIT | T_BIT), I_BIT);
        assert_eq!(m.cpu.spsr[Bank::Irq as usize], cpsr);
        assert_eq!((m.cpu.pc, m.cpu.r[0], m.cpu.r[14]), (IWRAM, IO, IRQ_RETURN));
        assert_eq!(m.cpu.r[13], SP_IRQ - 24);
        // The pushed lr is the interrupted instruction + 4.
        assert_eq!(m.bus.read32(SP_IRQ - 4), pc + 4);
        assert_eq!(m.bus.read32(SP_IRQ - 24), 0x100);

        m.step(); // bx lr
        assert_eq!(m.cpu.pc, IRQ_RETURN);
        m.step(); // the BIOS's return path
        assert_eq!((m.cpu.pc, m.cpu.cpsr), (pc, cpsr));
        for r in [0, 1, 2, 3, 12, 14] {
            assert_eq!(m.cpu.r[r], 0x100 + r as u32);
        }
        assert_eq!(m.cpu.r[13], SP_SYS);
        assert_eq!(m.cpu.banks[Bank::Irq as usize][5], SP_IRQ);
        assert_eq!(m.bus.bios_latch, crate::bus::BIOS_LATCH_AFTER_IRQ);
        assert_eq!(m.fault, None);
    }
}

/// `strb` to `HALTCNT`, then `b .`.
const HALT_CODE: [u32; 4] = [
    0xE3A0_0301, // mov r0, #0x04000000
    0xE3A0_1000, // mov r1, #0
    0xE5C0_1301, // strb r1, [r0, #0x301]
    B_SELF,
];

/// Halt wakes on `IE & IF` whatever `IME` says, and time skips ahead.
#[test]
fn halt_wakes_on_an_enabled_request() {
    let mut m = machine_with(&HALT_CODE);
    enable_vblank_irq(&mut m, false);
    m.step();
    m.step();
    assert!(m.step());
    assert!(m.sched.halted);
    assert_eq!(m.cpu.pc, ROM_ENTRY + 12);
    m.run_frame();
    assert!(!m.sched.halted);
    assert_eq!(m.now(), LINE_CYCLES * 160);
    // Not taken: IME is off.
    assert_eq!(m.cpu.cpsr & 0x1F, mode::SYS);
}

#[test]
fn halt_with_nothing_enabled_stays_halted() {
    let mut m = machine_with(&HALT_CODE);
    m.run_frame();
    m.run_frame();
    assert!(m.sched.halted);
    assert_eq!(m.cpu.pc, ROM_ENTRY + 12);
}

/// An IRQ wakes a halted CPU and its handler runs, as `VBlankIntrWait`
/// relies on.
#[test]
fn halt_then_irq() {
    let mut m = machine_with(&HALT_CODE);
    m.bus.write32(IWRAM, BX_LR);
    m.bus.write32(IRQ_HANDLER_PTR, IWRAM);
    enable_vblank_irq(&mut m, true);
    m.run_frame();
    assert!(m.take_irq());
    assert_eq!(m.cpu.pc, IWRAM);
    m.step();
    m.step();
    assert_eq!((m.cpu.pc, m.cpu.cpsr & 0x1F), (ROM_ENTRY + 12, mode::SYS));
}

/// The two instructions after the one executing are already fetched, so a
/// store over them doesn't change what runs (jsmolka's `nes` test 1).
#[test]
fn prefetched_code_survives_a_store() {
    let mut m = machine();
    let code = [
        0xE3A0_1000, // mov r1, #0
        0xE50F_1004, // str r1, [pc, #-4]: overwrites the next instruction
        0xE3A0_2005, // mov r2, #5
        B_SELF,
    ];
    for (i, w) in code.iter().enumerate() {
        m.bus.write32(IWRAM + 4 * i as u32, *w);
    }
    m.cpu.pc = IWRAM;
    for _ in 0..3 {
        m.step();
    }
    assert_eq!(m.bus.read32(IWRAM + 8), 0);
    assert_eq!(m.cpu.r[2], 5);
    // Coming back to it cold runs what's there now (`andeq r0, r0, r0`).
    m.cpu.r[2] = 0;
    m.cpu.pc = IWRAM + 8;
    m.tick(1);
    m.step();
    assert_eq!((m.cpu.r[2], m.cpu.pc), (0, IWRAM + 12));
    // Open bus reads the last prefetched opcode.
    assert_eq!(m.bus.open_bus, m.bus.read32(IWRAM + 20));
}

/// A `swi` runs the BIOS call in Rust and carries on after it.
#[test]
fn swi_runs_the_bios_call() {
    let mut m = machine_with(&[0xE3A0_0007, 0xE3A0_1002, 0xEF06_0000, B_SELF]);
    m.run_frame();
    assert_eq!(m.fault, None);
    assert_eq!(m.cpu.r[..2], [3, 1]);
    assert_eq!((m.cpu.pc, m.cpu.cpsr & 0x1F), (ROM_ENTRY + 12, mode::SYS));
}

/// A `swi` that took the hardware exception runs from the vector too.
#[test]
fn swi_through_the_vector() {
    let mut m = machine_with(&[0xEF06_0000, B_SELF]);
    m.cpu.r[..2].copy_from_slice(&[9, 2]);
    m.cpu.enter_exception(mode::SVC, 0x08, ROM_ENTRY + 4);
    m.step();
    assert_eq!(m.fault, None);
    assert_eq!(m.cpu.r[..2], [4, 1]);
    assert_eq!((m.cpu.pc, m.cpu.cpsr & 0x1F), (ROM_ENTRY + 4, mode::SYS));
}

/// An unimplemented `swi` stops the runner with its number and address.
#[test]
fn unimplemented_swi_faults() {
    let mut m = machine_with(&[0xEF2B_0000]);
    m.run_frame();
    assert_eq!(
        m.fault,
        Some(Fault::Swi(crate::bios::Unimplemented {
            number: 0x2B,
            pc: ROM_ENTRY
        }))
    );
}

/// `VBlankIntrWait` halts, the V-blank IRQ's handler flags it, and the wait
/// ends after the IRQ returns, back after the `swi`.
#[test]
fn vblank_intr_wait() {
    let mut m = machine_with(&[0xEF05_0000, B_SELF]);
    // Flag V-blank for the BIOS and acknowledge it, as agb's handler does
    // (r0 = 0x04000000 from the BIOS stub).
    let handler = [
        0xE3A0_1001, // mov r1, #1
        0xE140_10B8, // strh r1, [r0, #-8]: 0x03FFFFF8, the BIOS flags
        0xE280_2C02, // add r2, r0, #0x200
        0xE1C2_10B2, // strh r1, [r2, #2]: IF
        BX_LR,
    ];
    for (i, w) in handler.iter().enumerate() {
        m.bus.write32(IWRAM + 4 * i as u32, *w);
    }
    m.bus.write32(IRQ_HANDLER_PTR, IWRAM);
    enable_vblank_irq(&mut m, true);
    m.run_frame();
    assert!(!m.bios.pending().is_empty(), "waiting");
    m.run_frame();
    assert_eq!(m.fault, None);
    assert!(m.bios.pending().is_empty());
    assert_eq!((m.cpu.pc, m.cpu.cpsr & 0x1F), (ROM_ENTRY + 4, mode::SYS));
    assert_eq!(m.cpu.r[0], 1);
    assert_eq!(m.bus.read16(crate::bios::BIOS_IF), 0);
    assert_eq!(m.bus.read32(0), crate::bus::BIOS_LATCH_AFTER_SWI);
}

/// [`Machine::timing`]: from ROM at power-on's wait states, an `ldr` from
/// EWRAM and a `b .` cost mGBA's cycles exact, and only their listed ones
/// instant (no fetch waits, the load's access free).
#[test]
fn instant_timing_drops_fetch_waits_and_data_costs() {
    use crate::machine::Timing;
    use armv4t::cycles::cost;
    use armv4t::decode::decode_arm;
    const LDR: u32 = 0xE591_0000; // ldr r0, [r1]
    static ROM: [u8; 8] = {
        let (l, b) = (LDR.to_le_bytes(), B_SELF.to_le_bytes());
        [l[0], l[1], l[2], l[3], b[0], b[1], b[2], b[3]]
    };
    let run = |timing| {
        let mut m = std::boxed::Box::new(Machine::new(&ROM, Headless));
        m.timing = timing;
        m.cpu.r[1] = 0x0200_0000;
        m.step();
        let ldr = m.now();
        m.step();
        (ldr, m.now() - ldr)
    };
    let base = |raw, at| cost(&decode_arm(raw, at)).base as u64;
    let listed = (base(LDR, ROM_ENTRY), base(B_SELF, ROM_ENTRY + 4));
    assert_eq!(run(Timing::Instant), listed);
    let (ldr, b) = run(Timing::Exact);
    // The `b .`'s 3 + 5 + 7 + 5 (as above); the `ldr`'s fetches and its
    // EWRAM access on top of what's listed.
    assert_eq!(b, 20);
    assert!(ldr > listed.0 + 2, "{ldr}");
}

/// Instant charges a failed condition the whole instruction, as generated
/// code without `rt::skipped` does; exact charges mGBA's one cycle.
#[test]
fn instant_timing_charges_a_failed_condition_in_full() {
    use crate::machine::Timing;
    use armv4t::cycles::{SKIPPED, cost};
    use armv4t::decode::decode_arm;
    const LDRNE: u32 = 0x1591_0000; // ldrne r0, [r1]
    static ROM: [u8; 4] = LDRNE.to_le_bytes();
    let run = |timing| {
        let mut m = std::boxed::Box::new(Machine::new(&ROM, Headless));
        m.timing = timing;
        m.cpu.cpsr |= 1 << 30; // Z: `ne` fails.
        m.step();
        m.now()
    };
    assert_eq!(
        run(Timing::Instant),
        cost(&decode_arm(LDRNE, ROM_ENTRY)).base as u64
    );
    assert!(run(Timing::Exact) >= SKIPPED.base as u64);
    assert_ne!(run(Timing::Instant), SKIPPED.base as u64);
}
