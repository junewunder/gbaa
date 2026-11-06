use super::*;
use crate::save::{Kind, Save};

/// A 512 Kbit chip with `bytes` programmed at `at` by its own commands.
fn chip(at: u16, bytes: &[u8]) -> Save {
    let mut s = Save::new(Kind::Flash512);
    for (i, &b) in bytes.iter().enumerate() {
        s.write(0x5555, 0xAA);
        s.write(0x2AAA, 0x55);
        s.write(0x5555, 0xA0);
        s.write(at + i as u16, b);
    }
    s
}

#[test]
fn reads_bytes_and_sectors() {
    let s = chip(0x1000, &[1, 2, 3, 4]);
    assert_eq!(read_byte(&s, 0x0E00_1002), 3);
    assert_eq!(read_byte(&s, 0x0E00_2000), 0xFF, "erased");
    let mut dest = [0u8; 5];
    read(&s, 0x0E00_1000, &mut dest);
    assert_eq!(dest, [1, 2, 3, 4, 0xFF]);
}

#[test]
fn read_byte_sees_id_mode() {
    let mut s = Save::new(Kind::Flash512);
    for (a, v) in [(0x5555, 0xAA), (0x2AAA, 0x55), (0x5555, 0x90)] {
        s.write(a, v);
    }
    let id = read_byte(&s, 0x0E00_0000) as u16 | (read_byte(&s, 0x0E00_0001) as u16) << 8;
    assert_eq!(id, 0x1B32, "Panasonic's 512 Kbit ID, as the bus reads it");
}

#[test]
fn verify_gives_the_first_difference() {
    let s = chip(0x3000, &[9, 8, 7]);
    assert_eq!(verify(&s, &[9, 8, 7], 0x0E00_3000), 0);
    assert_eq!(verify(&s, &[9, 0, 7], 0x0E00_3000), 0x0E00_3001);
    assert_eq!(verify(&s, &[], 0x0E00_3000), 0);
}
