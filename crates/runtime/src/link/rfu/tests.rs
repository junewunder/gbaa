extern crate std;

use super::*;
use crate::platform::Headless;
use crate::test_util::{IO, machine};
use std::boxed::Box;
use std::vec::Vec;

type M = Box<Machine<Headless>>;

fn radio(n: usize) -> (Vec<M>, Wireless) {
    let mut ms: Vec<M> = (0..n).map(|_| machine()).collect();
    let mut refs: Vec<&mut Machine<Headless>> = ms.iter_mut().map(|m| &mut **m).collect();
    let w = Wireless::new(&mut refs);
    (ms, w)
}

/// Command `cmd` with `args` on adapter `i`, straight to the device: its
/// acknowledgement's words, header first.
fn cmd(w: &mut Wireless, i: usize, cmd: u8, args: &[u32]) -> Vec<u32> {
    w.adapters[i].req = Words::new();
    for &a in args {
        w.adapters[i].req.push(a);
    }
    w.execute(i, cmd, 0);
    w.adapters[i].ack.get().to_vec()
}

#[test]
fn handshake_trades_nintendo_as_afska_logged_it() {
    let (_ms, mut w) = radio(1);
    // gba-link-connection's wireless_adapter.md, "Nintendo Exchange".
    let log = [
        (0x7FFF_494E, 0x0000_0000),
        (0xFFFF_494E, 0x494E_B6B1),
        (0xB6B1_494E, 0x494E_B6B1),
        (0xB6B1_544E, 0x544E_B6B1),
        (0xABB1_544E, 0x544E_ABB1),
        (0xABB1_4E45, 0x4E45_ABB1),
        (0xB1BA_4E45, 0x4E45_B1BA),
        (0xB1BA_4F44, 0x4F44_B1BA),
        (0xB0BB_4F44, 0x4F44_B0BB),
        (0xB0BB_8001, 0x8001_B0BB),
    ];
    for (gba, adapter) in log {
        assert_eq!(
            w.exchange(0, gba, false, 0),
            adapter,
            "GBA sent {gba:#010x}"
        );
    }
}

fn siocnt(m: &M) -> u16 {
    m.bus.read16(IO + reg::SIOCNT)
}

fn data32(m: &M) -> u32 {
    m.bus.read32(IO + SIODATA32)
}

/// The GBA clocks `word` at 2 MHz and gets the adapter's.
fn clock(w: &mut Wireless, m: &mut M, word: u32) -> u32 {
    const BASE: u16 = cnt::NORMAL32 | cnt::IRQ | cnt::FAST | cnt::INTERNAL;
    m.bus.write32(IO + SIODATA32, word);
    m.bus.write16(IO + reg::SIOCNT, BASE);
    m.bus.write16(IO + reg::SIOCNT, BASE | cnt::START);
    w.run(&mut [&mut **m], FAST_CYCLES + 64, |_, m| m.run_frame());
    assert_eq!(siocnt(m) & cnt::START, 0, "the transfer ended");
    assert_ne!(siocnt(m) & cnt::SI, 0, "the adapter raised SI");
    m.bus.write16(IO + reg::SIOCNT, BASE | cnt::SO);
    w.run(&mut [&mut **m], 16, |_, m| m.run_frame());
    assert_eq!(siocnt(m) & cnt::SI, 0, "the adapter dropped SI for SO");
    data32(m)
}

#[test]
fn a_command_on_the_port_gets_its_acknowledgement() {
    let (mut ms, mut w) = radio(1);
    let m = &mut ms[0];
    m.bus.write16(IO + reg::RCNT, 0x100);
    // VersionStatus: no data; the acknowledgement and one word.
    assert_eq!(clock(&mut w, m, 0x9966_0012), 0x8000_0000);
    assert_eq!(clock(&mut w, m, 0x8000_0000), 0x9966_0192);
    assert_eq!(clock(&mut w, m, 0x8000_0000), VERSION);
    // Setup: one word of data.
    assert_eq!(clock(&mut w, m, 0x9966_0117), 0x8000_0000);
    assert_eq!(clock(&mut w, m, 0x003C_0420), 0x8000_0000);
    assert_eq!(clock(&mut w, m, 0x8000_0000), 0x9966_0097);
    assert_eq!(w.adapters[0].config, 0x003C_0420);
    // An unknown command.
    clock(&mut w, m, 0x9966_0001);
    assert_eq!(clock(&mut w, m, 0x8000_0000), 0x9966_01EE);
    assert_eq!(clock(&mut w, m, 0x8000_0000), 2);
}

#[test]
fn a_waiting_adapter_clocks_its_event_to_the_armed_gba() {
    let (mut ms, mut w) = radio(1);
    let m = &mut ms[0];
    m.bus.write16(IO + reg::RCNT, 0x100);
    // Setup with a 2-frame wait timeout, then Wait.
    clock(&mut w, m, 0x9966_0117);
    clock(&mut w, m, 0x0000_0002);
    clock(&mut w, m, 0x8000_0000);
    clock(&mut w, m, 0x9966_0027);
    assert_eq!(clock(&mut w, m, 0x8000_0000), 0x9966_00A7);
    // The GBA arms an external-clock transfer; nothing comes for 2 frames.
    const SLAVE: u16 = cnt::NORMAL32 | cnt::IRQ | cnt::FAST;
    let arm = |m: &mut M, word: u32| {
        m.bus.write32(IO + SIODATA32, word);
        m.bus.write16(IO + reg::SIOCNT, SLAVE);
        m.bus.write16(IO + reg::SIOCNT, SLAVE | cnt::START);
    };
    arm(m, 0x8000_0000);
    w.run(&mut [&mut **m], FRAME_CYCLES, |_, m| m.run_frame());
    assert_ne!(siocnt(m) & cnt::START, 0, "still waiting");
    w.run(&mut [&mut **m], FRAME_CYCLES + 2000, |_, m| m.run_frame());
    assert_eq!(siocnt(m) & cnt::START, 0);
    assert_eq!(data32(m), 0x9966_0027, "the wait timed out");
    assert_eq!(siocnt(m) & cnt::SI, 0);
    // The GBA's handshake: SO up, SI follows; then its acknowledgement.
    m.bus.write16(IO + reg::SIOCNT, SLAVE | cnt::SO);
    w.run(&mut [&mut **m], 16, |_, m| m.run_frame());
    assert_ne!(siocnt(m) & cnt::SI, 0);
    arm(m, 0x9966_00A7);
    w.run(&mut [&mut **m], 2000, |_, m| m.run_frame());
    assert_eq!(data32(m), 0x8000_0000);
    assert_eq!(w.adapters[0].spi, Spi::Idle, "the clock is the GBA's again");
}

#[test]
fn a_child_finds_a_host_joins_and_they_trade_packets() {
    let (_ms, mut w) = radio(2);
    let (host, child) = (0, 1);
    cmd(&mut w, host, 0x17, &[0x003C_0420]);
    cmd(&mut w, host, 0x16, &[1, 2, 3, 4, 5, 6]);
    cmd(&mut w, host, 0x19, &[]);
    let hid = w.adapters[host].id;
    assert_eq!(cmd(&mut w, child, 0x1C, &[]), [0x9966_009C]);
    let found = cmd(&mut w, child, 0x1D, &[]);
    assert_eq!(found, [0x9966_079D, hid as u32, 1, 2, 3, 4, 5, 6]);
    cmd(&mut w, child, 0x1E, &[]);
    cmd(&mut w, child, 0x1F, &[hid as u32]);
    let done = cmd(&mut w, child, 0x20, &[]);
    let cid = w.adapters[child].id;
    assert_eq!(done, [0x9966_01A0, cid as u32]);
    assert_eq!(cmd(&mut w, host, 0x1A, &[]), [0x9966_019A, cid as u32]);
    assert_eq!(cmd(&mut w, host, 0x13, &[])[1] >> 24, 2);
    assert_eq!(
        cmd(&mut w, child, 0x13, &[])[1],
        5 << 24 | 1 << 16 | cid as u32
    );
    // The child schedules 3 bytes; the host's SendDataWait carries its
    // own 5 and picks them up.
    cmd(&mut w, child, 0x24, &[3 << 8, 0x00CC_BBAA]);
    w.adapters[child].spi = Spi::Waiting { since: 0 };
    cmd(&mut w, host, 0x25, &[5, 0x4433_2211, 0x0000_0055]);
    assert_eq!(w.adapters[child].event, Some((event::DATA, None)));
    assert_eq!(w.adapters[host].event, Some((event::DATA, None)));
    assert_eq!(
        cmd(&mut w, child, 0x26, &[]),
        [0x9966_03A6, 5, 0x4433_2211, 0x55]
    );
    assert_eq!(cmd(&mut w, child, 0x26, &[]), [0x9966_00A6], "read once");
    assert_eq!(
        cmd(&mut w, host, 0x26, &[]),
        [0x9966_02A6, 3 << 8, 0x00CC_BBAA]
    );
    // The host sends the child away.
    w.adapters[child].event = None;
    w.adapters[child].spi = Spi::Waiting { since: 0 };
    cmd(&mut w, host, 0x30, &[1]);
    assert_eq!(w.adapters[child].role, Role::Idle);
    assert_eq!(
        w.adapters[child].event,
        Some((event::DISCONNECTED, Some(1)))
    );
}
