extern crate std;

use super::*;
use crate::test_util::machine;
use std::vec;
use std::vec::Vec;

/// A state for `m`'s ROM, in system mode at `pc` (ARM), everything else 0.
fn state_for<P: Platform>(m: &Machine<P>, pc: u32) -> Vec<u8> {
    let mut s = vec![0u8; STATE_SIZE];
    let mut put = |o: usize, v: u32| s[o..o + 4].copy_from_slice(&v.to_le_bytes());
    put(at::MAGIC, MAGIC + MAX_VERSION);
    put(at::ROM_CRC32, crc32(m.bus.rom));
    put(at::CPSR, mode::SYS);
    put(at::GPRS + 60, pc + 4);
    s
}

fn put(s: &mut [u8], o: usize, v: u32) {
    s[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn crc32_is_zlibs() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn refuses_what_it_cannot_load() {
    let mut m = machine();
    let good = state_for(&m, 0x0800_0000);
    assert_eq!(m.load_mgba_state(&good[..100]), Err(StateError::Short(100)));
    assert_eq!(m.load_mgba_state(b"\x89PNG\r\n"), Err(StateError::Png));
    let mut s = good.clone();
    put(&mut s, at::MAGIC, MAGIC + MAX_VERSION + 1);
    assert!(matches!(m.load_mgba_state(&s), Err(StateError::Magic(_))));
    let mut s = good.clone();
    put(&mut s, at::ROM_CRC32, 1);
    assert!(matches!(
        m.load_mgba_state(&s),
        Err(StateError::OtherRom { .. })
    ));
    // In mGBA's BIOS, not halted in a wait.
    let s = state_for(&m, hle::INTR_WAIT);
    assert_eq!(
        m.load_mgba_state(&s),
        Err(StateError::InBios(hle::INTR_WAIT))
    );
    // Nothing was changed.
    assert_eq!(m.cpu.pc, crate::machine::ROM_ENTRY);
}

#[test]
fn loads_registers_memory_and_time() {
    let mut m = machine();
    let mut s = state_for(&m, 0x0800_0100);
    // Thumb, IRQ mode: r15 is one halfword past.
    put(&mut s, at::CPSR, mode::IRQ | T_BIT);
    put(&mut s, at::GPRS + 60, 0x0800_0102);
    put(&mut s, at::GPRS + 4 * 13, 0x0300_7FA0);
    put(&mut s, at::SPSR, 0x1F);
    // The user bank's sp and r8; the SVC bank's lr.
    put(&mut s, at::BANKED, 0x0300_7F00);
    put(&mut s, at::BANKED + 8, 0x88);
    put(&mut s, at::BANKED + 4 * (7 * 3 + 1), 0x0800_0042);
    s[at::EWRAM + 5] = 0xAB;
    s[at::IWRAM + 0x7FFF] = 0xCD;
    s[at::VRAM] = 0xEF;
    // VCOUNT 160, timer 1 on at counter 0x1234 with reload 0xFF00, last
    // ticking 10 cycles ago.
    s[at::IO + 6] = 160;
    s[at::IO + 0x104..at::IO + 0x108].copy_from_slice(&[0x34, 0x12, 0x80, 0]);
    s[at::TIMERS + 0x14..at::TIMERS + 0x16].copy_from_slice(&[0x00, 0xFF]);
    put(&mut s, at::TIMERS + 0x14 + 4, -10i32 as u32);
    put(&mut s, at::GLOBAL_CYCLES, 1000);
    put(&mut s, at::CYCLES, 20);
    put(&mut s, at::VIDEO_NEXT_EVENT, 500);
    put(&mut s, at::VIDEO_FLAGS, 2);
    m.load_mgba_state(&s).unwrap();
    let c = &m.cpu;
    assert_eq!((c.pc, c.r[13], c.spsr()), (0x0800_0100, 0x0300_7FA0, 0x1F));
    assert_eq!(c.banks[Bank::User as usize][5], 0x0300_7F00);
    assert_eq!(c.banks[Bank::User as usize][0], 0x88);
    assert_eq!(c.banks[Bank::Svc as usize][6], 0x0800_0042);
    assert_eq!(
        (m.bus.ewram[5], m.bus.iwram[0x7FFF], m.bus.video.vram[0]),
        (0xAB, 0xCD, 0xEF)
    );
    assert_eq!(m.bus.io.vcount(), 160);
    // mGBA's clock plus 2^32, so earlier times stay positive.
    let base = 1 << 32;
    assert_eq!(m.now(), base + 1020);
    let t = m.bus.io.timers[1];
    assert_eq!((t.counter, t.since, t.ticking), (0x1234, base + 1010, true));
    assert_eq!(m.bus.io.raw16(reg::TM0CNT_L + 4), 0xFF00);
    assert_eq!(
        (m.sched.next_line_event, m.sched.hblank),
        (base + 1520, false)
    );
}

#[test]
fn unwinds_a_wait_in_mgbas_intr_wait() {
    let mut m = machine();
    let mut s = state_for(&m, hle::INTR_WAIT);
    put(&mut s, at::MISC_FLAGS, 1);
    // r1: the flags waited for. System sp, and the SVC bank's sp.
    put(&mut s, at::GPRS + 4, 1);
    put(&mut s, at::GPRS + 4 * 13, 0x0300_7EEC);
    put(&mut s, at::BANKED + 4 * (7 * 3), 0x0300_7FD0);
    // IntrWait's r2, r3, lr; the handler's r2, lr (system mode)...
    for (i, v) in [2, 3, 0xA4, 2, 0x0800_1235].into_iter().enumerate() {
        put(&mut s, at::IWRAM + 0x7EEC + 4 * i, v);
    }
    // ... and its SPSR (the caller's CPSR: Thumb, system), r11, r12, lr.
    for (i, v) in [mode::SYS | T_BIT, 11, 12, 0x0800_0202]
        .into_iter()
        .enumerate()
    {
        put(&mut s, at::IWRAM + 0x7FD0 + 4 * i, v);
    }
    m.load_mgba_state(&s).unwrap();
    let c = &m.cpu;
    assert_eq!((c.pc, c.cpsr), (0x0800_0202, mode::SYS | T_BIT));
    assert_eq!((c.r[2], c.r[3], c.r[11], c.r[12]), (2, 3, 11, 12));
    assert_eq!((c.r[13], c.r[14]), (0x0300_7F00, 0x0800_1235));
    assert_eq!(c.banks[Bank::Svc as usize][5..], [0x0300_7FE0, 0x0800_0202]);
    assert!(m.sched.halted);
    assert_eq!(
        m.bios.pending(),
        [Pending {
            pc: 0x0800_0202,
            depth: 0,
            mask: 1
        }]
    );
}
