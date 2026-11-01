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
