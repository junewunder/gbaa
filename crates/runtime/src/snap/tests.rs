extern crate std;

use super::*;
use crate::platform::Headless;
use crate::state::crc32;
use crate::test_util::machine_with;
use std::boxed::Box;
use std::vec::Vec;

/// Counts in r1 and stores it to the backdrop colour, so each frame draws
/// something new, with timer 0 running.
const COUNTER: [u32; 7] = [
    0xE3A0_0405, // mov r0, #0x05000000
    0xE3A0_2301, // mov r2, #0x04000000
    0xE3E0_3000, // mvn r3, #0 (timer 0: reload 0xFFFF, enabled)
    0xE582_3100, // str r3, [r2, #0x100]
    0xE281_1001, // add r1, r1, #1
    0xE1C0_10B0, // strh r1, [r0]
    0xEAFF_FFFC, // b (the add)
];

fn bytes(m: &mut Machine<Headless>) -> Vec<u8> {
    let mut b = Vec::new();
    m.save_state(&mut |s| b.extend_from_slice(s));
    b
}

fn counter() -> Box<Machine<Headless>> {
    let mut m = machine_with(&COUNTER);
    for _ in 0..10 {
        m.run_frame();
    }
    m
}

#[test]
fn a_loaded_state_runs_on_as_the_saved_one_did() {
    let mut m = counter();
    let s = bytes(&mut m);
    assert_eq!(bytes(&mut m), s, "saving changed the machine");
    let mut frames = Vec::new();
    for _ in 0..5 {
        m.run_frame();
        frames.push(m.ppu.frame);
    }
    let mut n = machine_with(&COUNTER);
    n.load_state(&s).unwrap();
    for f in &frames {
        n.run_frame();
        assert!(n.ppu.frame == *f);
    }
    // All of the state, not just the frame, came out the same.
    assert_eq!(n.cpu, m.cpu);
    assert!(bytes(&mut n) == bytes(&mut m));
}

#[test]
fn refuses_what_it_cannot_load() {
    let mut m = counter();
    let good = bytes(&mut m);
    let mut fresh = machine_with(&COUNTER);
    let before = bytes(&mut fresh);
    let mut bad = |f: fn(&mut Vec<u8>), want: SnapError| {
        let mut s = good.clone();
        f(&mut s);
        assert_eq!(fresh.load_state(&s), Err(want));
    };
    bad(|s| s[0] = b'x', SnapError::NotOurs);
    bad(|s| s[8] += 1, SnapError::Version(VERSION + 1));
    bad(|s| s.truncate(s.len() - 1), SnapError::Damaged);
    bad(|s| s.push(0), SnapError::Damaged);
    bad(|s| *s.last_mut().unwrap() ^= 1, SnapError::Damaged);
    bad(|s| s.truncate(10), SnapError::Damaged);
    let mut other = machine_with(&COUNTER[..6]);
    assert!(matches!(
        other.load_state(&good),
        Err(SnapError::OtherRom { .. })
    ));
    assert!(
        bytes(&mut fresh) == before,
        "a refused state changed the machine"
    );
}

/// The payload's length. When it changes, so did the layout: bump
/// [`VERSION`], then this.
#[test]
fn the_layout_is_versioned() {
    let s = bytes(&mut machine_with(&COUNTER));
    assert_eq!(
        (VERSION, s.len() - HEADER),
        (4, 642_753),
        "the payload changed: bump snap::VERSION"
    );
}

/// A state of version 2 (before the EEPROM's serial state), pinned from
/// the build before version 3 (54ad684a^: `counter`'s state, zlib), loads
/// into this build and comes out as the state this build saves.
#[test]
fn an_older_state_loads() {
    let v2 = miniz_oxide::inflate::decompress_to_vec_zlib(include_bytes!("v2-counter.state.zz"))
        .unwrap();
    assert_eq!(v2[8..12], 2u32.to_le_bytes());
    let mut n = machine_with(&COUNTER);
    n.load_state(&v2).unwrap();
    assert!(
        bytes(&mut n) == bytes(&mut counter()),
        "v2 didn't load as v3"
    );
    // Its EEPROM fields are the defaults, not what the machine had.
    let mut busy = machine_with(&COUNTER);
    busy.bus.save.eeprom_write(1, 2);
    assert!(bytes(&mut busy) != bytes(&mut machine_with(&COUNTER)));
    busy.load_state(&v2).unwrap();
    assert!(bytes(&mut busy) == bytes(&mut counter()));
    // Older than OLDEST, or damaged, it's refused as before.
    let mut old = v2.clone();
    old[8] = 1;
    assert_eq!(n.load_state(&old), Err(SnapError::Version(1)));
    let mut cut = v2.clone();
    cut.pop();
    assert_eq!(n.load_state(&cut), Err(SnapError::Damaged));
}

/// A state of version 3 (before `MultiBoot`'s state), pinned from the
/// build before version 4 (559f1d2: `counter`'s state, zlib), loads into
/// this build as the state this build saves, with no exchange running even
/// on a machine that had one.
#[test]
fn a_v3_state_loads() {
    let v3 = miniz_oxide::inflate::decompress_to_vec_zlib(include_bytes!("v3-counter.state.zz"))
        .unwrap();
    assert_eq!(v3[8..12], 3u32.to_le_bytes());
    let mut n = machine_with(&COUNTER);
    n.bios.multiboot.role = crate::bios::multiboot::role::CHILD;
    n.load_state(&v3).unwrap();
    assert!(
        bytes(&mut n) == bytes(&mut counter()),
        "v3 didn't load as v4"
    );
}

/// The ROM's CRC-32 is kept, but a machine given another ROM hashes that.
#[test]
fn a_new_rom_is_hashed_afresh() {
    let mut m = counter();
    let rom = m.bus.rom;
    assert_eq!(m.bus.rom_crc(), crc32(rom));
    let s = bytes(&mut m);
    m.bus.rom = &rom[..rom.len() - 4];
    assert_eq!(m.bus.rom_crc(), crc32(m.bus.rom));
    assert!(matches!(m.load_state(&s), Err(SnapError::OtherRom { .. })));
    assert_eq!(state_rom(&s), Some(crc32(rom)));
    m.load_state_any_rom(&s).unwrap();
    m.bus.rom = rom;
    m.load_state(&s).unwrap();
}
