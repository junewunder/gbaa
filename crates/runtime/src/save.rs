//! The cartridge's save memory: battery SRAM or a flash chip driven by
//! commands at `0E`–`0F`, or a serial EEPROM at `0D`, as mGBA emulates
//! them.
//!
//! The kind comes from the save library's tag in the ROM (`SRAM_V`,
//! `FLASH_V`, `FLASH512_V`, `FLASH1M_V`, `EEPROM_V`), the strings mGBA's
//! override table and autodetection stand in for; a ROM with none gets
//! SRAM. Flash speaks
//! the JEDEC sequence (`AA` to `5555`, `55` to `2AAA`, then the command to
//! `5555`): ID mode (`90`, left with `F0`), erase (`80`, then `10` for the
//! chip or `30` to a 4 KB sector), program one byte (`A0`) and, on the
//! 1 Mbit chip, the bank switch (`B0`, then the bank to address 0). The
//! chip IDs are mGBA's: Panasonic `1B32` for 512 Kbit, Sanyo `1362` for
//! 1 Mbit. Erases and programs finish at once: mGBA's settling time (reads
//! of `5F` just after) isn't modelled.
//!
//! The EEPROM is mGBA's: one bit a halfword, sent and read by DMA 3. A
//! request is `10` (write) or `11` (read), the block's address (6 bits on
//! the 4 Kbit chip, 14 on the 64 Kbit one), then for a write 64 data bits,
//! and a closing bit. How many units are left in the DMA tells the address
//! bits from the data, so one model serves both sizes; it keeps 8 KB, as
//! mGBA's `.sav` of the 64 Kbit chip. A read request is followed by a read
//! of 68 bits: 4 zeros, then the block's 64. Writes finish at once (bit 0
//! of a read is 1, ready, outside a read); mGBA's settling time isn't
//! modelled.
//!
//! Every store that changes the data sets [`Save::dirty`], so a front end
//! can keep the save in a file ([`Kind::file_size`] bytes, mGBA's `.sav`).

/// Bytes of the largest save, the 1 Mbit flash chip.
pub const SAVE_SIZE: usize = 0x2_0000;
/// SRAM's size, and a flash bank's.
pub const BANK_SIZE: usize = 0x1_0000;
/// The 64 Kbit EEPROM's bytes.
pub const EEPROM_SIZE: usize = 0x2000;

const FLASH_ID_512K: u16 = 0x1B32;
const FLASH_ID_1M: u16 = 0x1362;

const BASE_HI: u16 = 0x5555;
const BASE_LO: u16 = 0x2AAA;

mod cmd {
    pub const START: u8 = 0xAA;
    pub const CONTINUE: u8 = 0x55;
    pub const ID: u8 = 0x90;
    pub const TERMINATE: u8 = 0xF0;
    pub const ERASE: u8 = 0x80;
    pub const ERASE_CHIP: u8 = 0x10;
    pub const ERASE_SECTOR: u8 = 0x30;
    pub const PROGRAM: u8 = 0xA0;
    pub const SWITCH_BANK: u8 = 0xB0;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Sram,
    Flash512,
    Flash1M,
    Eeprom,
}

impl Kind {
    /// The save kind a ROM's library tag names (SRAM if none).
    pub fn detect(rom: &[u8]) -> Kind {
        let tag = |t: &[u8]| rom.windows(t.len()).any(|w| w == t);
        if tag(b"FLASH1M_V") {
            Kind::Flash1M
        } else if tag(b"FLASH_V") || tag(b"FLASH512_V") {
            Kind::Flash512
        } else if tag(b"EEPROM_V") {
            Kind::Eeprom
        } else {
            Kind::Sram
        }
    }

    /// The bytes of a `.sav` of this kind, as mGBA writes it: 32 KB SRAM,
    /// 64 KB or 128 KB flash, 8 KB EEPROM.
    pub fn file_size(self) -> usize {
        match self {
            Kind::Sram => 0x8000,
            Kind::Eeprom => EEPROM_SIZE,
            Kind::Flash512 => BANK_SIZE,
            Kind::Flash1M => SAVE_SIZE,
        }
    }
}

/// Where the chip is in the three-write command sequence.
/// The EEPROM's state between bits (mGBA's `EEPROM_COMMAND_*`).
mod ee {
    pub const IDLE: u8 = 0;
    /// Seen the request's first bit.
    pub const PENDING: u8 = 1;
    /// `10`: taking the address, then the data.
    pub const WRITE: u8 = 2;
    /// `11`: taking the address.
    pub const READ_PENDING: u8 = 3;
    /// Sending the block.
    pub const READ: u8 = 4;
}

/// The EEPROM's serial state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Eeprom {
    command: u8,
    /// The next bit a write stores, as a bit offset into the data.
    write_addr: u32,
    /// The block a read sends, as a bit offset.
    read_addr: u32,
    /// Bits of the read still to send, of 68.
    read_left: u8,
}

crate::snap::snap!(Eeprom {
    command,
    write_addr,
    read_addr,
    read_left
});

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Seq {
    Raw,
    Start,
    Continue,
}

pub struct Save {
    pub kind: Kind,
    /// The whole save. SRAM and the 512 Kbit chip use the first 64 KB.
    pub data: [u8; SAVE_SIZE],
    /// The flash bank `0E000000`–`0E00FFFF` shows (0 or 1).
    pub bank: u8,
    /// Set by a store that changes the data; the front end clears it.
    pub dirty: bool,
    seq: Seq,
    /// The command waiting for its operand (0 for none).
    command: u8,
    eeprom: Eeprom,
}

impl crate::snap::Snap for Save {
    fn snap(&mut self, v: &mut dyn crate::snap::Visitor) {
        let Save {
            kind,
            data,
            bank,
            dirty,
            seq,
            command,
            eeprom,
        } = self;
        let mut k = *kind as u8;
        k.snap(v);
        *kind = [Kind::Sram, Kind::Flash512, Kind::Flash1M, Kind::Eeprom][k as usize % 4];
        crate::snap::bytes(data, v);
        bank.snap(v);
        dirty.snap(v);
        let mut q = *seq as u8;
        q.snap(v);
        *seq = [Seq::Raw, Seq::Start, Seq::Continue][q as usize % 3];
        command.snap(v);
        // Version 3 on ([`crate::snap::OLDEST`]); older states lack it.
        if v.version() >= 3 {
            eeprom.snap(v);
        }
    }
}

impl Save {
    /// The EEPROM's serial state back to idle: what a state saved before
    /// it was kept (version 2) loads with.
    pub(crate) fn reset_eeprom(&mut self) {
        self.eeprom = Eeprom::default();
    }

    pub fn new(kind: Kind) -> Save {
        Save {
            kind,
            data: [0xFF; SAVE_SIZE],
            bank: 0,
            dirty: false,
            seq: Seq::Raw,
            command: 0,
            eeprom: Eeprom::default(),
        }
    }

    #[inline(always)]
    fn offset(&self, addr: u16) -> usize {
        self.bank as usize * BANK_SIZE + addr as usize
    }

    /// A byte at `addr`, the low 16 bits of a bus address.
    #[inline(always)]
    pub fn read(&self, addr: u16) -> u8 {
        if self.command == cmd::ID && addr < 2 {
            let id = match self.kind {
                Kind::Flash1M => FLASH_ID_1M,
                Kind::Flash512 => FLASH_ID_512K,
                Kind::Sram | Kind::Eeprom => return self.data[addr as usize],
            };
            return (id >> (addr * 8)) as u8;
        }
        self.data[self.offset(addr)]
    }

    /// A byte store to `addr`, the low 16 bits of a bus address.
    #[inline(always)]
    pub fn write(&mut self, addr: u16, v: u8) {
        match self.kind {
            Kind::Sram | Kind::Eeprom => {
                let d = &mut self.data[addr as usize];
                self.dirty |= *d != v;
                *d = v;
            }
            _ => self.flash_write(addr, v),
        }
    }

    /// mGBA's `GBASavedataWriteFlash`.
    #[inline(never)]
    fn flash_write(&mut self, addr: u16, v: u8) {
        match self.seq {
            Seq::Raw => match self.command {
                cmd::PROGRAM => {
                    let o = self.offset(addr);
                    self.data[o] = v;
                    self.dirty = true;
                    self.command = 0;
                }
                cmd::SWITCH_BANK => {
                    if addr == 0 && v < 2 && self.kind == Kind::Flash1M {
                        self.bank = v;
                    }
                    self.command = 0;
                }
                _ => {
                    if addr == BASE_HI && v == cmd::START {
                        self.seq = Seq::Start;
                    }
                }
            },
            Seq::Start => {
                self.seq = if addr == BASE_LO && v == cmd::CONTINUE {
                    Seq::Continue
                } else {
                    Seq::Raw
                };
            }
            Seq::Continue => {
                self.seq = Seq::Raw;
                if addr == BASE_HI {
                    match self.command {
                        0 => {
                            if matches!(v, cmd::ERASE | cmd::ID | cmd::PROGRAM | cmd::SWITCH_BANK) {
                                self.command = v;
                            }
                        }
                        cmd::ERASE => {
                            if v == cmd::ERASE_CHIP {
                                self.data.fill(0xFF);
                                self.dirty = true;
                            }
                            self.command = 0;
                        }
                        cmd::ID => {
                            if v == cmd::TERMINATE {
                                self.command = 0;
                            }
                        }
                        _ => self.command = 0,
                    }
                } else if self.command == cmd::ERASE && v == cmd::ERASE_SECTOR {
                    let o = self.offset(addr & 0xF000);
                    self.data[o..o + 0x1000].fill(0xFF);
                    self.dirty = true;
                    self.command = 0;
                }
            }
        }
    }
}

impl Save {
    /// A halfword to the EEPROM, `left` units before the end of its DMA
    /// (counting this one; 1 for a CPU store). mGBA's
    /// `GBASavedataWriteEEPROM`.
    pub fn eeprom_write(&mut self, v: u16, left: u32) {
        let bit = (v & 1) as u32;
        let e = &mut self.eeprom;
        match e.command {
            ee::PENDING => {
                e.command = ee::WRITE + bit as u8;
                if e.command == ee::WRITE {
                    e.write_addr = 0;
                } else {
                    e.read_addr = 0;
                }
            }
            ee::WRITE => {
                if left > 65 {
                    e.write_addr = e.write_addr << 1 | bit << 6;
                } else if left == 1 {
                    e.command = ee::IDLE;
                } else {
                    // The 64 Kbit chip ignores the address's top 4 bits.
                    let a = e.write_addr as usize % (EEPROM_SIZE * 8);
                    let mask = 0x80 >> (a & 7);
                    let d = &mut self.data[a >> 3];
                    let new = if bit != 0 { *d | mask } else { *d & !mask };
                    self.dirty |= *d != new;
                    *d = new;
                    e.write_addr += 1;
                }
            }
            ee::READ_PENDING => {
                if left > 1 {
                    e.read_addr = e.read_addr << 1 | bit << 6;
                } else {
                    e.read_left = 68;
                    e.command = ee::READ;
                }
            }
            _ => e.command = bit as u8,
        }
    }

    /// The next bit a load from the EEPROM sees: mGBA's
    /// `GBASavedataReadEEPROM`. Outside a read, 1 (ready).
    pub fn eeprom_read(&mut self) -> u16 {
        let e = &mut self.eeprom;
        if e.command != ee::READ {
            return 1;
        }
        e.read_left -= 1;
        if e.read_left == 0 {
            e.command = ee::IDLE;
        }
        if e.read_left >= 64 {
            return 0;
        }
        let a = (e.read_addr + 63 - e.read_left as u32) as usize % (EEPROM_SIZE * 8);
        (self.data[a >> 3] >> (7 - (a & 7))) as u16 & 1
    }

    /// What a CPU load from the EEPROM sees, without moving a read on:
    /// 1 (ready) outside a read. Games poll it after a write.
    pub fn eeprom_status(&self) -> u16 {
        (self.eeprom.command != ee::READ) as u16
    }
}

#[cfg(test)]
mod tests;
