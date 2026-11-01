use super::*;
use crate::io::{irq, reg};
use crate::machine::Machine;
use crate::platform::Headless;
use crate::test_util::{IO, machine};

fn start(m: &mut Machine<Headless>, n: u32, reload: u16, control: u16) {
    m.bus.write16(IO + reg::TM0CNT_L + 4 * n, reload);
    m.bus
        .write16(IO + reg::TM0CNT_L + 4 * n + 2, control | ctl::ENABLE);
    m.tick(0);
}

fn counter(m: &Machine<Headless>, n: u32) -> u16 {
    m.bus.read16(IO + reg::TM0CNT_L + 4 * n)
}

fn if_(m: &Machine<Headless>) -> u16 {
    m.bus.read16(IO + reg::IF)
}

#[test]
fn advance_wraps_to_the_reload() {
    assert_eq!(advance(0xFFF0, 0xFF00, 0x0F), (0xFFFF, 0));
    assert_eq!(advance(0xFFF0, 0xFF00, 0x10), (0xFF00, 1));
    assert_eq!(advance(0xFFF0, 0xFF00, 0x10 + 0x100 * 3 + 5), (0xFF05, 4));
}

#[test]
fn prescalers() {
    for (sel, shift) in [(0, 0), (1, 6), (2, 8), (3, 10)] {
        let mut m = machine();
        start(&mut m, 2, 0, sel);
        m.tick((10 << shift) + (1 << shift) - 1);
        assert_eq!(counter(&m, 2), 10, "prescaler {sel}");
        // Writing the reload doesn't touch a running counter.
        m.bus.write16(IO + reg::TM0CNT_L + 8, 0x1234);
        m.tick(1 << shift);
        assert_eq!(counter(&m, 2), 11, "prescaler {sel}");
    }
}

/// Reads see the current time, even between syncs.
#[test]
fn reads_between_syncs() {
    let mut m = machine();
    start(&mut m, 0, 0, 0);
    assert!(!m.tick(100));
    assert_eq!(counter(&m, 0), 100);
}

#[test]
fn overflow_raises_the_irq_on_time() {
    let mut m = machine();
    start(&mut m, 1, 0xFF00, ctl::IRQ);
    m.tick(0xFF);
    assert_eq!(if_(&m), 0);
    m.tick(1);
    assert_eq!(if_(&m), irq::TIMER0 << 1);
    assert_eq!(counter(&m, 1), 0xFF00);
    // Without the IRQ bit, no request (and no event either).
    let mut m = machine();
    start(&mut m, 1, 0xFF00, 0);
    m.tick(0x1000);
    assert_eq!(if_(&m), 0);
}

#[test]
fn cascade_counts_overflows() {
    let mut m = machine();
    start(&mut m, 1, 0, ctl::CASCADE | ctl::IRQ);
    start(&mut m, 0, 0xFFFC, 0);
    m.tick(4 * 10);
    assert_eq!(counter(&m, 1), 10);
    // A cascaded overflow's IRQ comes with the overflow below it.
    let mut m = machine();
    start(&mut m, 1, 0xFFFF, ctl::CASCADE | ctl::IRQ);
    start(&mut m, 0, 0xFFFC, 0);
    m.tick(3);
    assert_eq!(if_(&m) & irq::TIMER0 << 1, 0);
    m.tick(1);
    assert_eq!(if_(&m) & irq::TIMER0 << 1, irq::TIMER0 << 1);
}

/// Timer 0's count-up bit is ignored: it counts the clock.
#[test]
fn timer0_cannot_cascade() {
    let mut m = machine();
    start(&mut m, 0, 0, ctl::CASCADE);
    m.tick(7);
    assert_eq!(counter(&m, 0), 7);
}

#[test]
fn stopping_freezes_and_restarting_reloads() {
    let mut m = machine();
    start(&mut m, 3, 0x100, 0);
    m.tick(5);
    m.bus.write16(IO + reg::TM0CNT_L + 14, 0);
    m.tick(50);
    assert_eq!(counter(&m, 3), 0x105);
    start(&mut m, 3, 0x200, 0);
    assert_eq!(counter(&m, 3), 0x200);
}

/// A cascading timer switched to the clock counts from the switch, not from
/// when it was first enabled.
#[test]
fn cascade_to_clocked_counts_from_the_switch() {
    let mut m = machine();
    start(&mut m, 1, 0, ctl::CASCADE);
    m.tick(1000);
    m.bus.write16(IO + reg::TM0CNT_L + 6, ctl::ENABLE);
    m.tick(5);
    assert_eq!(counter(&m, 1), 5);
}

/// A new reload applies from the next overflow; counting up to the write
/// uses the old one.
#[test]
fn reload_written_while_running() {
    let mut m = machine();
    start(&mut m, 0, 0xFFF0, 0);
    m.tick(0x20);
    // Two overflows so far, each reloading 0xFFF0.
    m.bus.write16(IO + reg::TM0CNT_L, 0xFF00);
    assert_eq!(counter(&m, 0), 0xFFF0);
    m.tick(0x10);
    assert_eq!(counter(&m, 0), 0xFF00);
}

/// A timer started by a store inside a block gets its overflow scheduled at
/// the block's tick, so the IRQ still comes on time.
#[test]
fn started_in_a_block_overflows_on_time() {
    let mut m = machine();
    m.bus.write16(IO + reg::TM0CNT_L, 0xFF00);
    m.bus
        .write16(IO + reg::TM0CNT_L + 2, ctl::ENABLE | ctl::IRQ);
    m.tick(0x80);
    assert_eq!(if_(&m), 0);
    m.tick(0x7F);
    assert_eq!(if_(&m), 0);
    m.tick(1);
    assert_eq!(if_(&m), irq::TIMER0);
}
