extern crate std;

use super::*;
use crate::io::irq;
use crate::machine::Machine;
use crate::platform::Headless;
use crate::sched::{LINE_CYCLES, VISIBLE_LINES};
use crate::test_util::{IO, machine};

const EWRAM: u32 = 0x0200_0000;
const IWRAM: u32 = 0x0300_0000;

/// Program channel `n` and write its control (with the enable bit), as a
/// game would, then let the machine see the write.
fn start(m: &mut Machine<Headless>, n: u32, src: u32, dst: u32, count: u16, control: u16) {
    let b = IO + reg::DMA0SAD + 12 * n;
    m.bus.write32(b, src);
    m.bus.write32(b + 4, dst);
    m.bus.write16(b + 8, count);
    m.bus.write16(b + 10, control | ctl::ENABLE);
    m.tick(0);
}

fn control(m: &Machine<Headless>, n: u32) -> u16 {
    m.bus.io.raw16(reg::DMA0SAD + 12 * n + 10)
}

fn fill(m: &mut Machine<Headless>, at: u32, words: u32) {
    for i in 0..words {
        m.bus.write32(at + 4 * i, 0x1111_1111 * (i + 1));
    }
}

#[test]
fn immediate_word_copy() {
    let mut m = machine();
    fill(&mut m, EWRAM, 4);
    let before = m.now();
    start(&mut m, 3, EWRAM, IWRAM, 4, ctl::WORD | ctl::IRQ);
    for i in 0..4 {
        assert_eq!(m.bus.read32(IWRAM + 4 * i), 0x1111_1111 * (i + 1));
    }
    assert_eq!(m.bus.read32(IWRAM + 16), 0);
    // Done: the enable bit drops, the IRQ is requested, the CPU waited:
    // 2 a unit plus 2, and EWRAM's 5 wait states a word.
    assert_eq!(control(&m, 3) & ctl::ENABLE, 0);
    assert_eq!(m.bus.read16(IO + reg::IF), irq::DMA0 << 3);
    assert_eq!(m.now() - before, 2 * 4 + 2 + 4 * 5);
}

#[test]
fn halfwords_with_address_control() {
    let mut m = machine();
    fill(&mut m, EWRAM, 2);
    // Source decrements from the top halfword; destination fixed.
    start(
        &mut m,
        0,
        EWRAM + 6,
        IWRAM,
        4,
        1 << ctl::SRC_SHIFT | 2 << ctl::DST_SHIFT,
    );
    // Last written is the lowest halfword.
    assert_eq!(m.bus.read32(IWRAM), 0x1111);
    // Destination decrements.
    start(&mut m, 1, EWRAM, IWRAM + 0x16, 4, 1 << ctl::DST_SHIFT);
    assert_eq!(m.bus.read16(IWRAM + 0x16), 0x1111);
    assert_eq!(m.bus.read16(IWRAM + 0x14), 0x1111);
    assert_eq!(m.bus.read16(IWRAM + 0x12), 0x2222);
    assert_eq!(m.bus.read16(IWRAM + 0x10), 0x2222);
    assert_eq!(m.bus.read16(IO + reg::IF), 0);
}

/// Count 0 is the maximum: 0x4000 units, or 0x10000 for DMA 3.
#[test]
fn count_zero_is_the_maximum() {
    let mut m = machine();
    m.bus.write16(EWRAM + 2 * 0x4000 - 2, 0xBEEF);
    start(&mut m, 2, EWRAM, IWRAM, 0, 0);
    assert_eq!(m.bus.read16(IWRAM + 0x7FFE), 0xBEEF);
    let before = m.now();
    start(&mut m, 3, EWRAM, EWRAM + 0x2_0000, 0, 0);
    // EWRAM to EWRAM: 2 wait states a halfword at each end.
    assert_eq!(m.now() - before, 2 * 0x1_0000 + 2 + 0x1_0000 * 4);
}

/// A V-blank channel waits for V-blank; with repeat it stays on and, with
/// destination control 3, starts each time from the destination register.
#[test]
fn vblank_repeat_reloads() {
    let mut m = machine();
    fill(&mut m, EWRAM, 8);
    start(
        &mut m,
        1,
        EWRAM,
        IWRAM,
        2,
        ctl::WORD | ctl::REPEAT | 1 << ctl::TIMING_SHIFT | 3 << ctl::DST_SHIFT,
    );
    assert_eq!(m.bus.read32(IWRAM), 0);
    m.run_frame();
    assert_eq!(m.bus.read32(IWRAM), 0x1111_1111);
    assert_eq!(m.bus.read32(IWRAM + 4), 0x2222_2222);
    assert_ne!(control(&m, 1) & ctl::ENABLE, 0);
    // The next V-blank continues the source but restarts the destination.
    m.run_frame();
    assert_eq!(m.bus.read32(IWRAM), 0x3333_3333);
    assert_eq!(m.bus.read32(IWRAM + 8), 0);
}

/// An H-blank channel runs once in each visible line's H-blank.
#[test]
fn hblank_runs_each_visible_line() {
    let mut m = machine();
    // One word a line from a fixed source into an incrementing destination.
    m.bus.write32(EWRAM, 0xCAFE_F00D);
    start(
        &mut m,
        0,
        EWRAM,
        IWRAM,
        1,
        ctl::WORD | ctl::REPEAT | 2 << ctl::TIMING_SHIFT | 2 << ctl::SRC_SHIFT,
    );
    m.tick(LINE_CYCLES as u32);
    assert_eq!(m.bus.read32(IWRAM), 0xCAFE_F00D);
    assert_eq!(m.bus.read32(IWRAM + 4), 0);
    m.run_frame();
    m.run_frame();
    let n = VISIBLE_LINES as u32;
    // Lines 0-159 twice; none in V-blank.
    assert_eq!(m.bus.read32(IWRAM + 4 * (2 * n - 1)), 0xCAFE_F00D);
    assert_eq!(m.bus.read32(IWRAM + 4 * (2 * n)), 0);
}

/// A channel without repeat runs once and stops.
#[test]
fn without_repeat_runs_once() {
    let mut m = machine();
    m.bus.write32(EWRAM, 7);
    start(
        &mut m,
        2,
        EWRAM,
        IWRAM,
        1,
        ctl::WORD | 1 << ctl::TIMING_SHIFT,
    );
    m.run_frame();
    assert_eq!(m.bus.read32(IWRAM), 7);
    assert_eq!(control(&m, 2) & ctl::ENABLE, 0);
    m.bus.write32(EWRAM, 8);
    m.run_frame();
    assert_eq!(m.bus.read32(IWRAM), 7);
}

/// Program and enable channel `n` with bus stores only, no tick: what a
/// compiled block does.
fn store_only(m: &mut Machine<Headless>, n: u32, src: u32, dst: u32, count: u16, control: u16) {
    let b = IO + reg::DMA0SAD + 12 * n;
    m.bus.write32(b, src);
    m.bus.write32(b + 4, dst);
    m.bus
        .write32(b + 8, count as u32 | ((control | ctl::ENABLE) as u32) << 16);
}

/// Two copies on one channel inside a block both happen, each in its store.
#[test]
fn immediate_copies_run_in_the_store() {
    let mut m = machine();
    fill(&mut m, EWRAM, 2);
    store_only(&mut m, 3, EWRAM, IWRAM, 1, ctl::WORD);
    assert_eq!(m.bus.read32(IWRAM), 0x1111_1111);
    store_only(&mut m, 3, EWRAM + 4, IWRAM + 0x100, 1, ctl::WORD);
    assert_eq!(m.bus.read32(IWRAM + 0x100), 0x2222_2222);
}

/// Stopping a repeating channel and starting it again in one block
/// relatches its addresses.
#[test]
fn stop_and_restart_relatches() {
    let mut m = machine();
    fill(&mut m, EWRAM, 2);
    let hblank = ctl::WORD | ctl::REPEAT | 2 << ctl::TIMING_SHIFT | 2 << ctl::SRC_SHIFT;
    store_only(&mut m, 0, EWRAM, IWRAM, 1, hblank);
    m.bus.write16(IO + reg::DMA0SAD + 10, 0);
    store_only(&mut m, 0, EWRAM + 4, IWRAM + 0x100, 1, hblank);
    m.tick(LINE_CYCLES as u32);
    assert_eq!(m.bus.read32(IWRAM), 0);
    assert_eq!(m.bus.read32(IWRAM + 0x100), 0x2222_2222);
}

/// A transfer that switches its own channel off and on again: the re-armed
/// latch stands. (An immediate re-arm would run again, forever here, as on
/// hardware; it loops rather than recursing.)
#[test]
fn transfer_rearming_its_own_channel() {
    // DMA 3 copies two halfwords into its own control register (fixed
    // destination): 0 switches it off, then the second switches it on.
    let cnt_h = IO + reg::DMA0SAD + 12 * 3 + 10;
    let rearm = |m: &mut Machine<Headless>, second: u16| {
        m.bus.write16(EWRAM, 0);
        m.bus.write16(EWRAM + 2, second);
        start(m, 3, EWRAM, cnt_h, 2, 2 << ctl::DST_SHIFT);
    };
    // V-blank timing, re-latched from the registers: it stays armed.
    let mut m = machine();
    rearm(&mut m, ctl::ENABLE | 1 << ctl::TIMING_SHIFT);
    assert_eq!(control(&m, 3), ctl::ENABLE | 1 << ctl::TIMING_SHIFT);
    assert_eq!(m.bus.dma[3].src, EWRAM);
    assert_eq!(m.bus.dma[3].count, 2);
}

/// Game libraries drive the EEPROM with DMA 3, a bit a halfword: a write
/// request, then a read request and a 68-unit read back.
#[test]
fn dma_reaches_the_eeprom() {
    let mut m = machine();
    m.bus.save.kind = crate::save::Kind::Eeprom;
    const EEPROM: u32 = 0x0D00_0000;
    let data: u64 = 0xA5A5_0000_FFFF_1234;
    // `10`, block 3 in 14 bits, 64 data bits, the closing 0.
    let mut bits = std::vec![1u16, 0];
    bits.extend((0..14).rev().map(|i| 3u16 >> i & 1));
    bits.extend((0..64).rev().map(|i| (data >> i) as u16 & 1));
    bits.push(0);
    for (i, b) in bits.iter().enumerate() {
        m.bus.write16(EWRAM + 2 * i as u32, *b);
    }
    start(&mut m, 3, EWRAM, EEPROM, bits.len() as u16, 0);
    assert_eq!(m.bus.save.data[24..32], data.to_be_bytes());
    assert_eq!(m.bus.read16(EEPROM), 1, "ready");
    // `11`, block 3, the closing 0; then 68 bits back.
    bits.truncate(16);
    bits[1] = 1;
    bits.push(0);
    for (i, b) in bits.iter().enumerate() {
        m.bus.write16(EWRAM + 2 * i as u32, *b);
    }
    start(&mut m, 3, EWRAM, EEPROM, bits.len() as u16, 0);
    start(&mut m, 3, EEPROM, IWRAM, 68, 0);
    let got = (4..68).fold(0u64, |a, i| a << 1 | m.bus.read16(IWRAM + 2 * i) as u64);
    assert_eq!(got, data);
}
