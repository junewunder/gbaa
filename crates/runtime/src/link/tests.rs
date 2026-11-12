extern crate std;

use super::*;
use crate::platform::Headless;
use crate::test_util::{IO, machine};
use std::boxed::Box;

type M = Box<Machine<Headless>>;

/// Multiplayer mode, 115200 bps, IRQ on; `send` in `SIOMLT_SEND`.
fn multi(m: &mut M, send: u16) {
    m.bus.write16(IO + reg::RCNT, 0);
    m.bus.write16(IO + reg::SIOCNT, sio::MULTI | sio::IRQ | 3);
    m.bus.write16(IO + reg::SIOMLT_SEND, send);
}

fn siocnt(m: &M) -> u16 {
    m.bus.read16(IO + reg::SIOCNT)
}

fn multis(m: &M) -> [u16; 4] {
    core::array::from_fn(|p| m.bus.read16(IO + reg::SIOMULTI0 + 2 * p as u32))
}

fn run(l: &mut Link, a: &mut M, b: &mut M, cycles: u64) {
    l.run(&mut [&mut **a, &mut **b], cycles, |_, m| m.run_frame());
}

fn pair() -> (M, M, Link) {
    let (mut a, mut b) = (machine(), machine());
    multi(&mut a, 0x1111);
    multi(&mut b, 0x2222);
    let l = Link::new(&mut [&mut *a, &mut *b]);
    (a, b, l)
}

#[test]
fn terminals_show_parent_and_ready() {
    let (mut a, mut b, mut l) = pair();
    assert_eq!(siocnt(&a) & (sio::SI | sio::SD), sio::SD);
    assert_eq!(siocnt(&b) & (sio::SI | sio::SD), sio::SI | sio::SD);
    // The CPU can't write them.
    a.bus.write16(IO + reg::SIOCNT, sio::MULTI | sio::SI | 3);
    assert_eq!(siocnt(&a) & (sio::SI | sio::SD), sio::SD);
    // A child leaving multiplayer mode: not ready, from the next turn.
    b.bus.write16(IO + reg::SIOCNT, 0);
    run(&mut l, &mut a, &mut b, QUANTUM);
    assert_eq!(siocnt(&a) & sio::SD, 0);
    assert_eq!(l.now(), QUANTUM);
    // Each overshoots by less than a `b .` from the cartridge.
    for m in [&a, &b] {
        assert!((QUANTUM..QUANTUM + 16).contains(&m.now()), "{}", m.now());
    }
}

#[test]
fn a_transfer_reaches_every_player_at_one_cycle() {
    let (mut a, mut b, mut l) = pair();
    run(&mut l, &mut a, &mut b, 100);
    let start = a.now();
    a.bus.write16(IO + reg::SIOCNT, siocnt(&a) | sio::START);
    // Busy on both while it's in flight; the parent can't stop it.
    run(&mut l, &mut a, &mut b, 1000);
    assert_ne!(siocnt(&a) & sio::START, 0);
    assert_ne!(siocnt(&b) & sio::START, 0);
    a.bus.write16(IO + reg::SIOCNT, sio::MULTI | sio::IRQ | 3);
    assert_ne!(siocnt(&a) & sio::START, 0);
    assert_eq!(multis(&a), [0xFFFF; 4]);
    let end = start + TRANSFER_CYCLES[3][1];
    let n = end - 1 - l.now();
    run(&mut l, &mut a, &mut b, n);
    assert_eq!(l.transfers, 0);
    run(&mut l, &mut a, &mut b, 1);
    assert_eq!(l.transfers, 1);
    for (m, id) in [(&a, 0), (&b, 1)] {
        assert_eq!(multis(m), [0x1111, 0x2222, 0xFFFF, 0xFFFF]);
        assert_eq!(siocnt(m) & (sio::START | sio::ID), id << sio::ID_SHIFT);
        assert_ne!(m.bus.read16(IO + reg::IF) & irq::SERIAL, 0);
    }
}

#[test]
fn only_a_ready_parent_starts_one() {
    let (mut a, mut b, mut l) = pair();
    b.bus.write16(IO + reg::SIOCNT, siocnt(&b) | sio::START);
    assert_eq!(
        siocnt(&b) & sio::START,
        0,
        "a child's busy bit is the hub's"
    );
    run(&mut l, &mut a, &mut b, 20_000);
    assert_eq!(l.transfers, 0);
    // A child out of multiplayer mode: the start bit doesn't stick.
    b.bus.write16(IO + reg::SIOCNT, 0);
    run(&mut l, &mut a, &mut b, QUANTUM);
    a.bus.write16(IO + reg::SIOCNT, siocnt(&a) | sio::START);
    run(&mut l, &mut a, &mut b, 20_000);
    assert_eq!(l.transfers, 0);
    assert_eq!(siocnt(&a) & sio::START, 0);
    assert_eq!(multis(&a), [0; 4]);
}

#[test]
fn unlinked_machines_store_siocnt_as_written() {
    let mut m = machine();
    m.bus
        .write16(IO + reg::SIOCNT, sio::MULTI | sio::SD | sio::START);
    assert_eq!(siocnt(&m), sio::MULTI | sio::SD | sio::START);
    assert_eq!(m.bus.io.events, 0);
}

#[test]
fn a_deadline_ends_the_run() {
    let mut m = machine();
    m.set_deadline(5000);
    m.run_frame();
    assert!((5000..5016).contains(&m.now()), "{}", m.now());
    m.set_deadline(u64::MAX);
    m.run_frame();
    assert_eq!(m.bus.io.vcount(), crate::sched::VISIBLE_LINES);
}

#[test]
fn runs_are_deterministic() {
    let go = || {
        let (mut a, mut b, mut l) = pair();
        for k in 0..5u64 {
            run(&mut l, &mut a, &mut b, 17_000 + k * 333);
            a.bus.write16(IO + reg::SIOCNT, siocnt(&a) | sio::START);
            b.bus.write16(IO + reg::SIOMLT_SEND, k as u16);
        }
        (l, a.now(), b.now(), multis(&a), multis(&b))
    };
    let first = go();
    // The last start has had no time to finish.
    assert_eq!(first.0.transfers, 4);
    assert_eq!(first, go());
}

fn refs(ms: &mut [M]) -> std::vec::Vec<&mut Machine<Headless>> {
    ms.iter_mut().map(|m| &mut **m).collect()
}

#[test]
fn four_players_share_one_bus() {
    let mut ms: std::vec::Vec<M> = (0..MAX_PLAYERS).map(|_| machine()).collect();
    for (p, m) in ms.iter_mut().enumerate() {
        multi(m, 0x1111 * (p as u16 + 1));
    }
    let mut l = Link::new(&mut refs(&mut ms));
    // The parent alone has SI low; every one sees SD, all four ready.
    for (p, m) in ms.iter().enumerate() {
        let si = if p == 0 { 0 } else { sio::SI };
        assert_eq!(siocnt(m) & (sio::SI | sio::SD), si | sio::SD);
    }
    l.run(&mut refs(&mut ms), 100, |_, m| m.run_frame());
    let start = ms[0].now();
    let w = siocnt(&ms[0]) | sio::START;
    ms[0].bus.write16(IO + reg::SIOCNT, w);
    // Four players: the slowest of the baud rate's transfers.
    let end = start + TRANSFER_CYCLES[3][3];
    let n = end - 1 - l.now();
    l.run(&mut refs(&mut ms), n, |_, m| m.run_frame());
    assert_eq!(l.transfers, 0);
    l.run(&mut refs(&mut ms), 1, |_, m| m.run_frame());
    assert_eq!(l.transfers, 1);
    for (p, m) in ms.iter().enumerate() {
        assert_eq!(multis(m), [0x1111, 0x2222, 0x3333, 0x4444]);
        assert_eq!(
            siocnt(m) & (sio::START | sio::ID),
            (p as u16) << sio::ID_SHIFT
        );
        assert_ne!(m.bus.read16(IO + reg::IF) & irq::SERIAL, 0);
    }
}

/// `MultiBoot`'s parameters at IWRAM's start: seat 1 the one child, the
/// program the `len` bytes from the cartridge's start.
fn multi_boot(m: &mut M, len: u32) {
    use crate::machine::ROM_ENTRY;
    m.bus.write8(0x0300_001E, 2);
    m.bus.write32(0x0300_0020, ROM_ENTRY);
    m.bus.write32(0x0300_0024, ROM_ENTRY + len);
    m.cpu.r[0] = 0x0300_0000;
    m.cpu.r[1] = 1;
    // Returning to the `b .` at the entry.
    m.cpu.pc = ROM_ENTRY;
    crate::bios::swi(m, 0x25 << 16).unwrap();
}

/// A child with a cartridge never answers as a cartless one's BIOS does:
/// `MultiBoot` sends, waits for its `0075`, gives up and fails, with IRQs
/// and `SIOCNT` as they were.
#[test]
fn multi_boot_with_no_child_taking_it_fails() {
    use crate::bios::multiboot::LOOP;
    let (mut a, mut b, mut l) = pair();
    multi_boot(&mut a, 0x100);
    assert_eq!(a.cpu.pc, LOOP, "sending");
    assert!(siocnt(&a) & sio::IRQ != 0);
    for _ in 0..30 {
        run(&mut l, &mut a, &mut b, crate::sched::FRAME_CYCLES);
        if a.cpu.pc != LOOP {
            break;
        }
    }
    assert_eq!(a.cpu.pc, 0x0800_0000, "returned");
    assert_eq!(a.cpu.r[0], 1, "failed");
    assert_eq!(a.cpu.cpsr & 0x80, 0, "IRQs as they were");
    assert!(l.transfers > 128, "the program went out: {}", l.transfers);
    assert_eq!(a.bios.multiboot, Default::default());
}

/// A length `MultiBoot` can't send (not a multiple of 16) fails at once.
#[test]
fn multi_boot_of_a_bad_length_fails_at_once() {
    let (mut a, _b, _l) = pair();
    multi_boot(&mut a, 0x108);
    assert_eq!((a.cpu.pc, a.cpu.r[0]), (0x0800_0000, 1));
}

/// A GBA with no cartridge waits for a program in multiplayer mode, and
/// `HardReset` there waits again.
#[test]
fn a_cartless_gba_waits_for_a_program() {
    use crate::bios::multiboot::{LOOP, boot_cartless, role};
    let mut m: M = Box::new(Machine::new(&[], Headless));
    boot_cartless(&mut m);
    assert_eq!(m.cpu.pc, LOOP);
    assert!(multiplayer(&m.bus.io));
    m.run_frame();
    assert_eq!(m.cpu.pc, LOOP);
    assert!(m.fault.is_none());
    m.cpu.pc = 0x0200_0100;
    crate::bios::swi(&mut m, 0x26 << 16).unwrap();
    assert_eq!(m.cpu.pc, LOOP);
    assert_eq!(m.bios.multiboot.role, role::CHILD);
}
