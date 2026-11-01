extern crate std;

use super::*;
use crate::dma::ctl as dctl;
use crate::io::irq;
use crate::machine::Machine;
use crate::platform::Headless;
use crate::test_util::{IO, machine};

const EWRAM: u32 = 0x0200_0000;
/// Cycles per sample in these tests (timer 0's reload is `0x10000 - P`):
/// more than the 10 cycles a refill adds to the clock, so the refills
/// within a test never add up to an extra sample.
const P: u32 = 64;
/// agb's sound DMA control: fixed destination, repeat, words, special.
const AGB_DMA: u16 = (2 << dctl::DST_SHIFT) | dctl::REPEAT | dctl::WORD | (3 << dctl::TIMING_SHIFT);

/// Bytes 1, 2, 3, … at `EWRAM`.
fn fill(m: &mut Machine<Headless>, n: u32) {
    for i in 0..n {
        m.bus.write8(EWRAM + i, (i + 1) as u8);
    }
}

/// Set up as agb's `mixer.enable()` and `swap()` do: FIFO A on timer 0 to
/// both speakers at 100%, fed by DMA1 from `EWRAM`, with `DMA1CNT_L` set
/// to a count that must be ignored. Timer 1 cascades every `buffer`
/// samples with its IRQ if `buffer` isn't 0.
fn agb(m: &mut Machine<Headless>, buffer: u16) {
    m.bus.write16(IO + reg::TM0CNT_L, (0x1_0000 - P) as u16);
    m.bus.write16(IO + reg::TM0CNT_L + 2, timer::ctl::ENABLE);
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0B04);
    m.bus.write16(IO + reg::SOUNDCNT_X, 0x80);
    if buffer != 0 {
        m.bus
            .write16(IO + reg::TM0CNT_L + 4, 0u16.wrapping_sub(buffer));
        m.bus.write16(
            IO + reg::TM0CNT_L + 6,
            timer::ctl::ENABLE | timer::ctl::CASCADE | timer::ctl::IRQ,
        );
    }
    let d = IO + reg::DMA0SAD + 12;
    m.bus.write32(d, EWRAM);
    m.bus.write32(d + 4, FIFO_ADDR[0]);
    m.bus.write16(d + 8, 100);
    m.bus.write16(d + 10, AGB_DMA | dctl::ENABLE);
    m.tick(0);
}

#[test]
fn fifo_push_pop_full_empty_reset() {
    let mut f = Fifo::default();
    assert!(f.is_empty());
    assert_eq!(f.pop(), None);
    for i in 0..FIFO_SIZE {
        assert!(f.push(i as u8));
    }
    // Full: the push is dropped.
    assert!(!f.push(0xEE));
    assert_eq!(f.len(), FIFO_SIZE);
    assert_eq!(f.pop(), Some(0));
    assert!(f.push(0x80));
    for i in 1..FIFO_SIZE {
        assert_eq!(f.pop(), Some(i as i8));
    }
    // Wrapped around the ring, and signed.
    assert_eq!(f.pop(), Some(-128));
    assert_eq!(f.pop(), None);
    f.push(1);
    f.push(2);
    f.reset();
    assert!(f.is_empty());
    assert_eq!(f.pop(), None);
}

#[test]
fn stores_push_their_bytes_low_first() {
    let mut m = machine();
    // A 4-word burst, as an `stm` or a CPU-run copy does it.
    for i in 0..4u32 {
        m.bus
            .write32(IO + reg::FIFO_A, 0x0403_0201 + 0x0404_0404 * i);
    }
    assert_eq!(m.bus.sound.fifo[0].len(), 16);
    for i in 1..=16 {
        assert_eq!(m.bus.sound.fifo[0].pop(), Some(i));
    }
    m.bus.write16(IO + reg::FIFO_B + 2, 0x0201);
    m.bus.write8(IO + reg::FIFO_B + 1, 3);
    let b = &mut m.bus.sound.fifo[1];
    assert_eq!(
        (b.pop(), b.pop(), b.pop(), b.pop()),
        (Some(1), Some(2), Some(3), None)
    );
    // Reading back gives nothing: write-only.
    assert_eq!(m.bus.read32(IO + reg::FIFO_A), 0);
    // A reset empties only the FIFO it names.
    m.bus.write32(IO + reg::FIFO_A, 1);
    m.bus.write32(IO + reg::FIFO_B, 1);
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0800);
    assert_eq!(
        (m.bus.sound.fifo[0].len(), m.bus.sound.fifo[1].len()),
        (0, 4)
    );
}

#[test]
fn fifo_dma_refills_four_words_at_the_threshold() {
    let mut m = machine();
    fill(&mut m, 256);
    agb(&mut m, 0);
    let dma = |m: &Machine<Headless>| m.bus.dma[1].src - EWRAM;
    // Enabling doesn't run it.
    assert_eq!((m.bus.sound.fifo[0].len(), dma(&m)), (0, 0));
    // The first pop finds the FIFO empty (sample 0), then refills 16.
    m.tick(P);
    assert_eq!(m.bus.sound.out[0], 0);
    assert_eq!((m.bus.sound.fifo[0].len(), dma(&m)), (16, 16));
    // 15 left: refill to 31.
    m.tick(P);
    assert_eq!(m.bus.sound.out[0], 1);
    assert_eq!((m.bus.sound.fifo[0].len(), dma(&m)), (31, 32));
    // Then a refill every 16 pops, exactly 4 words each, never the count
    // register's 100 units.
    m.tick(14 * P);
    assert_eq!((m.bus.sound.fifo[0].len(), dma(&m)), (17, 32));
    m.tick(P);
    assert_eq!((m.bus.sound.fifo[0].len(), dma(&m)), (32, 48));
    assert_eq!(m.bus.sound.out[0], 16);
    // Still enabled (repeat), the destination untouched.
    assert_ne!(m.bus.dma[1].control & dctl::ENABLE, 0);
    assert_eq!(m.bus.dma[1].dst, FIFO_ADDR[0]);
    // The records are the pops in order, one per sample; B was never
    // routed, so it stays 0.
    let s = &m.bus.sound;
    assert_eq!(s.len, 17);
    assert_eq!(s.period, P);
    let a: std::vec::Vec<i8> = s.records[..s.len].iter().map(|r| r.0).collect();
    assert_eq!(a, (0..=16).collect::<std::vec::Vec<i8>>());
    assert!(s.records[..s.len].iter().all(|r| r.1 == 0));
}

#[test]
fn without_repeat_a_refill_switches_the_channel_off() {
    let mut m = machine();
    fill(&mut m, 64);
    agb(&mut m, 0);
    let o = reg::DMA0SAD + 12 + 10;
    m.bus
        .write16(IO + o, (AGB_DMA & !dctl::REPEAT) | dctl::ENABLE | dctl::IRQ);
    m.tick(P);
    assert_eq!(m.bus.sound.fifo[0].len(), 16);
    assert_eq!(m.bus.io.raw16(o) & dctl::ENABLE, 0);
    assert_ne!(m.bus.read16(IO + reg::IF) & (irq::DMA0 << 1), 0);
    m.tick(P);
    assert_eq!(m.bus.sound.fifo[0].len(), 15);
}

#[test]
fn timer_pops_with_a_cascade_and_its_irq() {
    let mut m = machine();
    fill(&mut m, 256);
    agb(&mut m, 4);
    m.tick(3 * P);
    assert_eq!(m.bus.read16(IO + reg::IF) & (irq::TIMER0 << 1), 0);
    assert_eq!(m.bus.sound.swap.swaps, 0);
    // The fourth sample overflows timer 1: its IRQ, and a swap to time.
    m.tick(P);
    assert_ne!(m.bus.read16(IO + reg::IF) & (irq::TIMER0 << 1), 0);
    assert_eq!(m.bus.sound.swap.swaps, 1);
    assert_eq!(m.bus.sound.len, 4);
    // Re-latching DMA1 half a sample later counts as that swap's re-latch.
    m.tick(P / 2);
    let o = IO + reg::DMA0SAD + 12 + 10;
    m.bus.write16(o, 0);
    m.bus.write16(o, AGB_DMA | dctl::ENABLE);
    // The clock ran 20 cycles ahead of the timer: the first two refills'
    // DMA time.
    let s = m.bus.sound.swap;
    assert_eq!(
        (s.relatches, s.max_cycles, s.max_pops, s.max_refills),
        (1, (P / 2 + 20) as u64, 0, 0)
    );
    assert_eq!(&m.bus.sound.restarts[..m.bus.sound.restarts_len], &[0, 4]);
}

#[test]
fn many_samples_in_one_go_are_walked_one_by_one() {
    let mut m = machine();
    fill(&mut m, 256);
    agb(&mut m, 0);
    // One sync, 40 samples late: the same pops and refills as one a tick.
    m.tick(40 * P);
    let s = &m.bus.sound;
    assert_eq!(s.len, 40);
    let a: std::vec::Vec<i8> = s.records[..40].iter().map(|r| r.0).collect();
    assert_eq!(a, (0..40).collect::<std::vec::Vec<i8>>());
}

#[test]
fn a_full_buffer_drops_and_counts() {
    let mut m = machine();
    // Timer 0 at reload 0xFFFF overflows every cycle.
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0300);
    m.bus.write16(IO + reg::SOUNDCNT_X, 0x80);
    m.bus.write16(IO + reg::TM0CNT_L, 0xFFFF);
    m.bus.write16(IO + reg::TM0CNT_L + 2, timer::ctl::ENABLE);
    m.bus.io.now += CHUNK as u64 + 100;
    run_timers(&mut m.bus);
    assert_eq!((m.bus.sound.len, m.bus.sound.dropped), (CHUNK, 100));
    m.hand_off_audio();
    assert_eq!((m.bus.sound.len, m.bus.sound.dropped), (0, 0));
}

#[test]
fn mapping_to_i16() {
    let mut io = Io::new();
    // Master off: the bias alone, 0x200, is silence.
    io.write16(reg::SOUNDCNT_H, 0x330C);
    assert_eq!(mix(&io, 127, -128, (0, 0)), (0, 0));
    io.write16(reg::SOUNDCNT_X, 0x80);
    // Both at 100% to both sides: 4·s each, times 64.
    assert_eq!(mix(&io, 10, -3, (0, 0)), ((40 - 12) * 64, (40 - 12) * 64));
    // A to the left only, B to the right only, as agb sets it.
    io.write16(reg::SOUNDCNT_H, 0x120C);
    assert_eq!(mix(&io, 127, -128, (0, 0)), (127 * 256, -128 * 256));
    // 50%: 2·s.
    io.write16(reg::SOUNDCNT_H, 0x1200);
    assert_eq!(mix(&io, 127, -128, (0, 0)), (127 * 128, -128 * 128));
    // The bias shifts the level, and the sum clips at 0 and 0x3FF.
    io.write16(reg::SOUNDCNT_H, 0x120C);
    io.write16(reg::SOUNDBIAS, 0x4100);
    assert_eq!(mix(&io, 0, 0, (0, 0)), (-0x100 * 64, -0x100 * 64));
    assert_eq!(
        mix(&io, -128, 127, (0, 0)),
        (-0x200 * 64, (0x100 + 508 - 0x200) * 64)
    );
    io.write16(reg::SOUNDBIAS, 0x3FE);
    assert_eq!(
        mix(&io, 127, -128, (0, 0)),
        ((0x3FF - 0x200) * 64, (0x3FE - 512 - 0x200) * 64)
    );
}

#[test]
fn a_dma_burst_into_a_fifo_pushes_sixteen_bytes() {
    let mut m = machine();
    fill(&mut m, 16);
    // An ordinary immediate DMA3, 4 words to a fixed FIFO_B.
    let d = IO + reg::DMA0SAD + 36;
    m.bus.write32(d, EWRAM);
    m.bus.write32(d + 4, FIFO_ADDR[1]);
    m.bus.write16(d + 8, 4);
    m.bus
        .write16(d + 10, (2 << dctl::DST_SHIFT) | dctl::WORD | dctl::ENABLE);
    let b = &mut m.bus.sound.fifo[1];
    assert_eq!(b.len(), 16);
    for i in 1..=16 {
        assert_eq!(b.pop(), Some(i));
    }
}

#[test]
fn a_store_walks_the_samples_due_before_it() {
    let mut m = machine();
    fill(&mut m, 256);
    agb(&mut m, 0);
    // Three samples are due, but no tick has synced them; a FIFO reset
    // store must see them happen first, refills included.
    m.bus.io.now += 3 * P as u64;
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0B04);
    assert_eq!(m.bus.sound.len, 3);
    assert_eq!(m.bus.dma[1].src - EWRAM, 32);
    assert!(m.bus.sound.fifo[0].is_empty());
    // So must a DMA1 control store: the sample due before it pops (empty,
    // so it refills from the old source), then the re-latch lands.
    m.bus.io.now += P as u64;
    let d = IO + reg::DMA0SAD + 12;
    m.bus.write16(d + 10, 0);
    assert_eq!((m.bus.sound.len, m.bus.dma[1].src - EWRAM), (4, 48));
}

#[test]
fn a_cascaded_timer_can_clock_a_fifo() {
    let mut m = machine();
    for i in 0..4 {
        m.bus
            .write32(IO + reg::FIFO_A, 0x0403_0201 + 0x0404_0404 * i);
    }
    // Timer 0 overflows every P/2 cycles; timer 1 every second of those,
    // and clocks FIFO A.
    m.bus.write16(IO + reg::TM0CNT_L, (0x1_0000 - P / 2) as u16);
    m.bus.write16(IO + reg::TM0CNT_L + 4, 0xFFFE);
    m.bus.write16(
        IO + reg::TM0CNT_L + 6,
        timer::ctl::ENABLE | timer::ctl::CASCADE,
    );
    m.bus.write16(IO + reg::TM0CNT_L + 2, timer::ctl::ENABLE);
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0704);
    m.bus.write16(IO + reg::SOUNDCNT_X, 0x80);
    m.tick(0);
    m.tick(5 * P / 2);
    let s = &m.bus.sound;
    assert_eq!(s.len, 2);
    assert_eq!((s.records[0].0, s.records[1].0), (1, 2));
    assert_eq!(s.period, P);
}

#[test]
fn sync_hands_off_a_half_full_buffer() {
    let mut m = machine();
    m.bus.write16(IO + reg::SOUNDCNT_H, 0x0300);
    m.bus.write16(IO + reg::SOUNDCNT_X, 0x80);
    m.bus.write16(IO + reg::TM0CNT_L, 0xFFF0);
    m.bus.write16(IO + reg::TM0CNT_L + 2, timer::ctl::ENABLE);
    m.tick(0);
    // Not half full yet: kept.
    m.tick(16 * (CHUNK as u32 / 2 - 1));
    assert_eq!(m.bus.sound.len, CHUNK / 2 - 1);
    // Half full at a sync: handed to the platform (Headless drops it).
    m.tick(16);
    assert_eq!(m.bus.sound.len, 0);
}
