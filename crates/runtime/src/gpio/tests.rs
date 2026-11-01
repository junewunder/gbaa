extern crate std;

use super::*;
use crate::Bus;
use std::boxed::Box;
use std::vec::Vec;

const SCK: u16 = 1;
const SIO: u16 = 2;
const CS: u16 = 4;

/// A ROM with game code `code`, its bytes elsewhere `0xA5`.
fn rom(code: &[u8; 4]) -> &'static [u8] {
    let mut v: Vec<u8> = std::vec![0xA5; 0x200];
    v[0xAC..0xB0].copy_from_slice(code);
    Box::leak(v.into_boxed_slice())
}

fn bus(code: &[u8; 4]) -> Box<Bus> {
    Box::new(Bus::new(rom(code)))
}

/// Emerald's `siirtc.c`, store for store, through the bus.
struct Driver<'a>(&'a mut Bus);

impl Driver<'_> {
    fn data(&mut self, v: u16) {
        self.0.write16(0x0800_00C4, v);
    }
    fn direction(&mut self, v: u16) {
        self.0.write16(0x0800_00C6, v);
    }
    fn begin(&mut self) {
        self.data(SCK);
        self.data(SCK | CS);
        self.direction(7);
    }
    fn end(&mut self) {
        self.data(SCK);
        self.data(SCK);
    }
    /// `WriteCommand`: MSB first.
    fn command(&mut self, v: u8) {
        for i in 0..8 {
            let b = ((v >> (7 - i)) & 1) as u16;
            for _ in 0..3 {
                self.data(b << 1 | CS);
            }
            self.data(b << 1 | SCK | CS);
        }
    }
    /// `WriteData`: LSB first.
    fn write(&mut self, v: u8) {
        for i in 0..8 {
            let b = ((v >> i) & 1) as u16;
            for _ in 0..3 {
                self.data(b << 1 | CS);
            }
            self.data(b << 1 | SCK | CS);
        }
    }
    /// `ReadData`.
    fn read(&mut self) -> u8 {
        let mut v = 0u8;
        for _ in 0..8 {
            for _ in 0..5 {
                self.data(CS);
            }
            self.data(SCK | CS);
            let bit = ((self.0.read16(0x0800_00C4) & SIO) >> 1) as u8;
            v = (v >> 1) | (bit << 7);
        }
        v
    }
    fn read_command(&mut self, command: u8, n: usize) -> Vec<u8> {
        self.begin();
        self.command(command);
        self.direction(SCK | CS);
        let out = (0..n).map(|_| self.read()).collect();
        self.end();
        out
    }
    fn status(&mut self) -> u8 {
        self.read_command(0x63, 1)[0]
    }
    fn datetime(&mut self) -> Vec<u8> {
        self.read_command(0x65, 7)
    }
    fn time(&mut self) -> Vec<u8> {
        self.read_command(0x67, 3)
    }
    fn set_status(&mut self, v: u8) {
        self.begin();
        self.command(0x62);
        self.write(v);
        self.end();
    }
    fn reset(&mut self) {
        self.begin();
        self.command(0x60);
        self.end();
    }
}

#[test]
fn detected_by_game_code() {
    assert!(bus(b"BPEE").gpio.has_rtc);
    assert!(bus(b"AXVE").gpio.has_rtc);
    assert!(!bus(b"BPRE").gpio.has_rtc);
    assert!(!Gpio::for_rom(&[0; 0x10]).has_rtc);
}

#[test]
fn registers_read_rom_until_stored() {
    let mut b = bus(b"BPEE");
    assert_eq!(b.read16(0x0800_00C4), 0xA5A5);
    // Read-back off: the registers read 0, in every ROM mirror.
    b.write16(0x0800_00C6, 5);
    assert_eq!(b.read16(0x0800_00C6), 0);
    assert_eq!(b.read32(0x0A00_00C4), 0);
    assert_eq!(b.read8(0x0800_00C8), 0);
    // Read-back on.
    b.write16(0x0800_00C8, 1);
    assert_eq!(b.read32(0x0800_00C4), 5 << 16);
    assert_eq!(b.read16(0x0C00_00C8), 1);
    // The GBA only drives its output pins.
    b.write16(0x0800_00C4, 0xF);
    assert_eq!(b.read16(0x0800_00C4), 5);
    // Neighbours are still ROM.
    assert_eq!(b.read16(0x0800_00C2), 0xA5A5);
    assert_eq!(b.read16(0x0800_00CA), 0xA5A5);
}

#[test]
fn only_halfword_stores_in_the_first_window() {
    let mut b = bus(b"BPEE");
    b.write8(0x0800_00C8, 1);
    b.write32(0x0800_00C8, 1);
    b.write16(0x0900_00C8, 1);
    b.write16(0x0A00_00C8, 1);
    assert_eq!(b.gpio.read_write, 0);
    assert_eq!(b.read16(0x0800_00C8), 0xA5A5);
}

#[test]
fn no_clock_no_port() {
    let mut b = bus(b"BPRE");
    b.write16(0x0800_00C8, 1);
    b.write16(0x0800_00C6, 7);
    assert_eq!(b.read16(0x0800_00C8), 0xA5A5);
    assert_eq!(b.gpio.shown, None);
}

/// Emerald's `SiiRtcProbe` then `RtcGetRawInfo`, from power on.
#[test]
fn emerald_probe_and_read() {
    let mut b = bus(b"BPEE");
    let mut d = Driver(&mut b);
    d.0.write16(0x0800_00C8, 1);
    // The first transfer starts with every pin an input, so the clock
    // never sees it begin, and SIO still holds the command's last bit.
    assert_eq!(d.status(), 0xFF);
    assert_eq!(d.0.gpio.rtc.transfer_step, 1);
    // Now in step, the probe resets it into 24-hour mode.
    d.reset();
    assert_eq!(d.0.gpio.rtc.control, 0);
    d.set_status(0x40);
    assert_eq!(d.0.gpio.rtc.control, 0x40);
    assert_eq!(d.time(), [0x12, 0x00, 0x00]);
    assert_eq!(d.status(), 0x40);
    // 2004-09-16, a Thursday, 12:00:00.
    assert_eq!(d.datetime(), [0x04, 0x09, 0x16, 0x04, 0x12, 0x00, 0x00]);
    // Between transfers the clock is idle.
    assert!(!d.0.gpio.rtc.command_active);
}

#[test]
fn the_clock_reads_the_time_it_is_given() {
    let mut b = bus(b"BPEE");
    let mut d = Driver(&mut b);
    d.0.write16(0x0800_00C8, 1);
    d.direction(5);
    // 2024-02-29 23:59:58 UTC, a Thursday.
    d.0.gpio.clock = 1_709_251_198;
    assert_eq!(d.datetime(), [0x24, 0x02, 0x29, 0x04, 0x23, 0x59, 0x58]);
    // 12-hour mode drops the hour to 0-11 with no PM flag, as in mGBA.
    d.set_status(0);
    assert_eq!(d.time(), [0x11, 0x59, 0x58]);
}

#[test]
fn dates() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(-1), (1969, 12, 31));
    assert_eq!(civil_from_days(FIXED_CLOCK / 86_400), (2004, 9, 16));
    assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    assert_eq!(civil_from_days(10_957), (2000, 1, 1));
}
