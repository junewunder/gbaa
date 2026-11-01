extern crate std;

use super::*;
use std::boxed::Box;

fn flash() -> Box<Save> {
    Box::new(Save::new(Kind::Flash1M))
}

/// The JEDEC prefix and a command to `5555`.
fn command(s: &mut Save, c: u8) {
    s.write(0x5555, 0xAA);
    s.write(0x2AAA, 0x55);
    s.write(0x5555, c);
}

#[test]
fn the_tag_picks_the_kind() {
    assert_eq!(Kind::detect(b"..FLASH1M_V103.."), Kind::Flash1M);
    assert_eq!(Kind::detect(b"FLASH512_V131"), Kind::Flash512);
    assert_eq!(Kind::detect(b"xFLASH_V126"), Kind::Flash512);
    assert_eq!(Kind::detect(b"SRAM_V113"), Kind::Sram);
    assert_eq!(Kind::detect(b""), Kind::Sram);
}

#[test]
fn id_mode_reads_the_chip_id() {
    let mut s = flash();
    s.data[0] = 0x12;
    command(&mut s, 0x90);
    assert_eq!(s.read(0), 0x62);
    assert_eq!(s.read(1), 0x13);
    assert_eq!(s.read(2), 0xFF);
    command(&mut s, 0xF0);
    assert_eq!(s.read(0), 0x12);

    let mut s = Box::new(Save::new(Kind::Flash512));
    command(&mut s, 0x90);
    assert_eq!([s.read(0), s.read(1)], [0x32, 0x1B]);
}

#[test]
fn program_writes_one_byte_then_stores_are_ignored() {
    let mut s = flash();
    command(&mut s, 0xA0);
    s.write(0x1234, 0x56);
    assert_eq!(s.read(0x1234), 0x56);
    s.write(0x1235, 0x78);
    assert_eq!(s.read(0x1235), 0xFF);
}

#[test]
fn a_broken_sequence_does_nothing() {
    let mut s = flash();
    s.write(0x5555, 0xAA);
    s.write(0x2AAB, 0x55);
    s.write(0x5555, 0xA0);
    s.write(0x0000, 0x00);
    assert_eq!(s.read(0), 0xFF);
}

#[test]
fn sector_erase_clears_4_kb_of_the_current_bank() {
    let mut s = flash();
    s.data.fill(0);
    command(&mut s, 0x80);
    s.write(0x5555, 0xAA);
    s.write(0x2AAA, 0x55);
    s.write(0x3456, 0x30);
    assert_eq!(s.read(0x2FFF), 0);
    assert_eq!(s.read(0x3000), 0xFF);
    assert_eq!(s.read(0x3FFF), 0xFF);
    assert_eq!(s.read(0x4000), 0);
    assert_eq!(s.data[BANK_SIZE + 0x3000], 0);
}

#[test]
fn chip_erase_clears_both_banks() {
    let mut s = flash();
    s.data.fill(0);
    command(&mut s, 0x80);
    command(&mut s, 0x10);
    assert!(s.data.iter().all(|&b| b == 0xFF));
}

#[test]
fn bank_switch_moves_the_window() {
    let mut s = flash();
    command(&mut s, 0xB0);
    s.write(0, 1);
    assert_eq!(s.bank, 1);
    command(&mut s, 0xA0);
    s.write(0x10, 0x42);
    assert_eq!(s.data[BANK_SIZE + 0x10], 0x42);
    assert_eq!(s.read(0x10), 0x42);
    command(&mut s, 0xB0);
    s.write(0, 0);
    assert_eq!(s.read(0x10), 0xFF);

    // The 512 Kbit chip has one bank.
    let mut s = Box::new(Save::new(Kind::Flash512));
    command(&mut s, 0xB0);
    s.write(0, 1);
    assert_eq!(s.bank, 0);
}

#[test]
fn sram_stores_bytes_directly() {
    let mut s = Box::new(Save::new(Kind::Sram));
    s.write(0x5555, 0xAA);
    assert_eq!(s.read(0x5555), 0xAA);
}

#[test]
fn changes_mark_the_save_dirty() {
    let mut s = Box::new(Save::new(Kind::Sram));
    s.write(0x10, 0xFF);
    assert!(!s.dirty, "storing what is there changes nothing");
    s.write(0x10, 0x01);
    assert!(s.dirty);

    let mut s = flash();
    command(&mut s, 0x90);
    command(&mut s, 0xF0);
    assert!(!s.dirty, "ID mode writes no data");
    command(&mut s, 0xA0);
    s.write(0x20, 0x42);
    assert!(s.dirty);
    s.dirty = false;
    command(&mut s, 0x80);
    s.write(0x5555, 0xAA);
    s.write(0x2AAA, 0x55);
    s.write(0x1000, 0x30);
    assert!(s.dirty);
}

fn eeprom() -> Box<Save> {
    Box::new(Save::new(Kind::Eeprom))
}

/// A request's bits, as DMA 3 sends them: each with the units left.
fn send(s: &mut Save, bits: &[u16]) {
    for (i, &b) in bits.iter().enumerate() {
        s.eeprom_write(b, (bits.len() - i) as u32);
    }
}

/// `n` address bits of `block`, most significant first.
fn addr(block: u16, n: u32) -> impl Iterator<Item = u16> {
    (0..n).rev().map(move |i| block >> i & 1)
}

fn write_block(s: &mut Save, block: u16, n: u32, data: u64) {
    let mut bits = std::vec![1, 0];
    bits.extend(addr(block, n));
    bits.extend((0..64).rev().map(|i| (data >> i) as u16 & 1));
    bits.push(0);
    send(s, &bits);
}

fn read_block(s: &mut Save, block: u16, n: u32) -> u64 {
    let mut bits = std::vec![1, 1];
    bits.extend(addr(block, n));
    bits.push(0);
    send(s, &bits);
    let got: std::vec::Vec<u16> = (0..68).map(|_| s.eeprom_read()).collect();
    assert_eq!(got[..4], [0, 0, 0, 0]);
    got[4..].iter().fold(0, |a, &b| a << 1 | b as u64)
}

#[test]
fn the_eeprom_tag_picks_the_eeprom() {
    assert_eq!(Kind::detect(b"..EEPROM_V124.."), Kind::Eeprom);
    assert_eq!(Kind::Eeprom.file_size(), 0x2000);
}

#[test]
fn eeprom_blocks_round_trip_with_either_address_width() {
    let mut s = eeprom();
    // 14 bits (64 Kbit): block 0x3FF is the last 8 bytes.
    write_block(&mut s, 0x3FF, 14, 0x0123_4567_89AB_CDEF);
    assert!(s.dirty);
    assert_eq!(
        s.data[0x1FF8..0x2000],
        [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]
    );
    assert_eq!(read_block(&mut s, 0x3FF, 14), 0x0123_4567_89AB_CDEF);
    // Ready again after the read; an unwritten block reads as erased.
    assert_eq!(s.eeprom_read(), 1);
    assert_eq!(read_block(&mut s, 2, 14), u64::MAX);
    // 6 bits (4 Kbit).
    write_block(&mut s, 5, 6, 0xFEED_FACE_0000_0001);
    assert_eq!(s.data[40..48], [0xFE, 0xED, 0xFA, 0xCE, 0, 0, 0, 1]);
    assert_eq!(read_block(&mut s, 5, 6), 0xFEED_FACE_0000_0001);
    assert_eq!(s.eeprom_status(), 1);
}
