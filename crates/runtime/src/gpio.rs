//! The cartridge's GPIO port and the real-time clock behind it, as mGBA
//! 0.10 emulates them (`src/gba/cart/gpio.c`).
//!
//! The port is three halfword registers in ROM space: data (the four pins)
//! at `0x080000C4`, direction (a set bit is a pin the GBA drives) at `C6`
//! and control (whether the registers can be read back) at `C8`. Only
//! halfword stores in the `0x08` window reach them, as in mGBA. Until the
//! first store the addresses read the ROM's own bytes; after it they read
//! the pins, direction and control while control is set, and 0 while it
//! isn't (mGBA writes them into its copy of the ROM, so every ROM mirror
//! sees them).
//!
//! The clock is a Seiko S-3511, spoken to over the pins serially: pin 0 is
//! the clock (SCK), 1 the data (SIO), 2 chip select (CS). A transfer
//! raises CS with SCK high, then sends a command byte (bits LSB first,
//! each latched on SCK's rising edge), then reads or writes the bytes the
//! command takes. mGBA's model is followed step for step, quirks included:
//! a command byte is taken when its low nibble is 6, the 12-hour mode drops
//! the PM flag, the IRQ command does nothing.
//!
//! Which cartridges have a clock comes from the game code, as in mGBA's
//! override table ([`HAS_RTC`]). mGBA also gives one to unknown Pokémon ROM
//! hacks, by checksum; that heuristic isn't copied.
//!
//! **Time.** The clock reads [`Gpio::clock`], Unix seconds taken as UTC.
//! It starts at [`FIXED_CLOCK`] and stays there unless the front end moves
//! it, so headless runs are reproducible; `mgba-dump` pins mGBA's clock to
//! the same second (its `RTC_FIXED` source, in UTC). The desktop window
//! sets it to the wall clock every frame.

/// The second every headless run's clock reads: 2004-09-16 12:00:00 UTC,
/// Emerald's Japanese release day.
pub const FIXED_CLOCK: i64 = 1_095_336_000;

/// The registers' offsets in ROM space.
pub const DATA: u32 = 0xC4;
pub const DIRECTION: u32 = 0xC6;
pub const CONTROL: u32 = 0xC8;

/// Game codes (the header's 4 bytes at `0xAC`) that mGBA 0.10's override
/// table gives a real-time clock.
pub const HAS_RTC: [&[u8; 4]; 33] = [
    b"U3IJ", b"U3IE", b"U3IP", b"U32J", b"U32E", b"U32P", b"BLJJ", b"BLJK", b"BLVJ", b"AXVJ",
    b"AXVE", b"AXVP", b"AXVI", b"AXVS", b"AXVD", b"AXVF", b"AXPJ", b"AXPE", b"AXPP", b"AXPI",
    b"AXPS", b"AXPD", b"AXPF", b"BPEJ", b"BPEE", b"BPEP", b"BPEI", b"BPES", b"BPED", b"BPEF",
    b"BR4J", b"BKAJ", b"U33J",
];

/// The bytes each RTC command transfers after its command byte, by the
/// command's 3 bits.
const RTC_BYTES: [i32; 8] = [0, 0, 7, 0, 1, 0, 3, 0];

/// RTC commands (the command byte's bits 4–6).
mod cmd {
    pub const RESET: u8 = 0;
    pub const DATETIME: u8 = 2;
    pub const FORCE_IRQ: u8 = 3;
    pub const CONTROL: u8 = 4;
    pub const TIME: u8 = 6;
}

/// The control register's 24-hour bit.
const HOUR24: u8 = 0x40;

/// The GPIO port and its devices.
#[derive(Clone, Debug)]
pub struct Gpio {
    /// Whether a clock is wired to the pins. Without one (no devices, in
    /// mGBA's terms) stores to the port are ignored.
    pub has_rtc: bool,
    pub pins: u16,
    pub direction: u16,
    /// The control register: nonzero when the port reads back.
    pub read_write: u16,
    /// What `C4`, `C6` and `C8` read, once a register has been stored to.
    pub shown: Option<[u16; 3]>,
    pub rtc: Rtc,
    /// The clock's time, Unix seconds in UTC.
    pub clock: i64,
    /// The ROM's own halfword at `C4`, which the clock's first output
    /// reads back as the old pins (mGBA reads its ROM copy).
    pub rom_data: u16,
}

/// The S-3511's state, field for field mGBA's `struct GBARTC`.
#[derive(Clone, Debug, Default)]
pub struct Rtc {
    pub bytes_remaining: i32,
    pub transfer_step: i32,
    pub bits_read: i32,
    pub bits: u32,
    pub command_active: bool,
    pub command: u32,
    pub control: u8,
    /// The latched date and time, BCD: year, month, day, weekday, hour,
    /// minute, second.
    pub time: [u8; 7],
}

impl Gpio {
    /// The port of the cartridge `rom`, with a clock if its game code is
    /// one of [`HAS_RTC`].
    pub fn for_rom(rom: &[u8]) -> Gpio {
        let has_rtc = rom
            .get(0xAC..0xB0)
            .is_some_and(|code| HAS_RTC.iter().any(|c| c[..] == *code));
        Gpio {
            has_rtc,
            pins: 0,
            direction: 0,
            read_write: 0,
            shown: None,
            rtc: Rtc {
                control: HOUR24,
                ..Rtc::default()
            },
            clock: FIXED_CLOCK,
            rom_data: rom
                .get(DATA as usize..DATA as usize + 2)
                .map_or(0, |b| u16::from_le_bytes([b[0], b[1]])),
        }
    }

    /// What the port reads at ROM offset `o` (halfword aligned), if it
    /// has taken a store; `None` means the ROM's own halfword.
    #[inline(always)]
    pub fn read(&self, o: usize) -> Option<u16> {
        let i = o.wrapping_sub(DATA as usize);
        if i < 6 {
            self.shown.map(|s| s[i >> 1])
        } else {
            None
        }
    }

    /// A halfword store to ROM offset `o` in the `0x08` window: taken if
    /// `o` is one of the registers and a device is wired.
    pub fn write(&mut self, o: u32, v: u16) {
        if !self.has_rtc {
            return;
        }
        match o {
            DATA => {
                self.pins = (self.pins & !self.direction) | (v & self.direction);
                self.rtc_read_pins();
            }
            DIRECTION => self.direction = v,
            CONTROL => self.read_write = v,
            _ => return,
        }
        self.show();
    }

    /// Update what the registers read (mGBA's store into its ROM copy).
    pub fn show(&mut self) {
        self.shown = Some(if self.read_write != 0 {
            [self.pins, self.direction, self.read_write]
        } else {
            [0; 3]
        });
    }

    /// The clock drives the pins it doesn't leave to the GBA. The GBA's
    /// own pins come from what the data register read before this store,
    /// as in mGBA.
    fn output_pins(&mut self, pins: u16) {
        if self.read_write != 0 {
            let old = self.shown.map_or(self.rom_data, |s| s[0]) & self.direction;
            self.pins = old | (pins & !self.direction & 0xF);
        }
    }

    /// The clock sees the pins change.
    fn rtc_read_pins(&mut self) {
        let p = self.pins;
        match self.rtc.transfer_step {
            // Waiting for SCK high with CS low, then CS high.
            0 => {
                if p & 5 == 1 {
                    self.rtc.transfer_step = 1;
                }
            }
            1 => {
                if p & 5 == 5 {
                    self.rtc.transfer_step = 2;
                } else if p & 5 != 1 {
                    self.rtc.transfer_step = 0;
                }
            }
            2 => {
                let r = &mut self.rtc;
                if p & 1 == 0 {
                    // SCK low: the GBA sets the next bit up.
                    r.bits &= !(1 << r.bits_read);
                    r.bits |= (((p & 2) >> 1) as u32) << r.bits_read;
                } else if p & 4 != 0 {
                    // SCK high with CS: a bit moves.
                    if r.command & 0x80 == 0 {
                        r.bits_read += 1;
                        if r.bits_read == 8 {
                            self.rtc_process_byte();
                        }
                    } else {
                        let bit = self.rtc_output();
                        self.output_pins(5 | (bit << 1));
                        let r = &mut self.rtc;
                        r.bits_read += 1;
                        if r.bits_read == 8 {
                            r.bytes_remaining -= 1;
                            if r.bytes_remaining <= 0 {
                                r.command_active = false;
                                r.command = 0;
                            }
                            r.bits_read = 0;
                        }
                    }
                } else {
                    // CS dropped: the transfer ends.
                    r.bits_read = 0;
                    r.bytes_remaining = 0;
                    r.command_active = false;
                    r.command = 0;
                    r.transfer_step = (p & 1) as i32;
                    self.output_pins(1);
                }
            }
            _ => {}
        }
    }

    /// A byte from the GBA: a command, or the data of the active one.
    fn rtc_process_byte(&mut self) {
        let r = &mut self.rtc;
        r.bytes_remaining -= 1;
        if !r.command_active {
            let command = r.bits;
            if command & 0xF == 6 {
                let c = ((command >> 4) & 7) as u8;
                r.command = command;
                r.bytes_remaining = RTC_BYTES[c as usize];
                r.command_active = r.bytes_remaining > 0;
                match c {
                    cmd::RESET => r.control = 0,
                    cmd::DATETIME | cmd::TIME => self.rtc_update_clock(),
                    _ => {}
                }
            }
        } else if (r.command >> 4) & 7 == cmd::CONTROL as u32 {
            r.control = r.bits as u8;
        }
        let r = &mut self.rtc;
        r.bits = 0;
        r.bits_read = 0;
        if r.bytes_remaining == 0 {
            r.command_active = false;
            r.command = 0;
        }
    }

    /// The next bit the clock sends.
    fn rtc_output(&self) -> u16 {
        let r = &self.rtc;
        if !r.command_active {
            return 0;
        }
        let byte = match ((r.command >> 4) & 7) as u8 {
            cmd::CONTROL => r.control,
            cmd::DATETIME | cmd::TIME => {
                let i = 7 - r.bytes_remaining;
                usize::try_from(i)
                    .ok()
                    .and_then(|i| r.time.get(i))
                    .copied()
                    .unwrap_or(0)
            }
            cmd::FORCE_IRQ | cmd::RESET => 0,
            _ => 0,
        };
        ((byte >> r.bits_read) & 1) as u16
    }

    /// Latch [`Gpio::clock`] into the date and time registers.
    fn rtc_update_clock(&mut self) {
        let t = self.clock;
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        // 1970-01-01 was a Thursday; Sunday is 0.
        let wday = (days + 4).rem_euclid(7);
        let hour = secs / 3600;
        let hour = if self.rtc.control & HOUR24 != 0 {
            hour
        } else {
            hour % 12
        };
        // `tm_year - 100`, as mGBA computes it, wrapping as its BCD does.
        let bcd = |v: i64| {
            let v = v as u32;
            ((v % 10) + (((v / 10) % 10) << 4)) as u8
        };
        self.rtc.time = [
            bcd(year - 2000),
            bcd(month),
            bcd(day),
            bcd(wday),
            bcd(hour),
            bcd(secs / 60 % 60),
            bcd(secs % 60),
        ];
    }
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard
/// Hinnant's `civil_from_days`): year, month 1–12, day 1–31.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

crate::snap::snap!(Gpio {
    has_rtc,
    pins,
    direction,
    read_write,
    shown,
    rtc,
    clock,
    rom_data
});
crate::snap::snap!(Rtc {
    bytes_remaining,
    transfer_step,
    bits_read,
    bits,
    command_active,
    command,
    control,
    time
});

#[cfg(test)]
mod tests;
