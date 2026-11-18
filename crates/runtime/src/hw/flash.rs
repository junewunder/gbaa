//! The flash save library's stack routines as library calls
//! (docs/native-port-design.md, section 2, "Saves are the library's").
//!
//! Nintendo's `AgbFlash` copies three tiny routines onto the stack and
//! calls them there, because the cartridge bus can't fetch code from ROM
//! while it reads the chip. [`recognise`] knows them by those bytes, at a
//! call into RAM (the hole report's `analysis::flash`, and a lifted
//! program's `call_fn`, which never runs them). A native program calls
//! these instead, on
//! [`Hw::save`](super::Hw::save), the play build's own chip emulation
//! ([`crate::save::Save`]), so the chip's state (ID mode, the 1 Mbit
//! chip's bank) answers as it does for the copied routine's loads.
//!
//! Chip addresses are bus addresses (`0x0E00_0000` on); like the bus, the
//! chip sees their low 16 bits.

use crate::save::Save;

/// One of the library's stack routines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Routine {
    /// `ReadFlash1(addr) -> u8`: one byte of the chip (its ID, its status
    /// while a program or erase settles).
    ReadByte,
    /// `ReadFlash_Core(src, dest, size)`: `size` bytes of the chip from
    /// `src` into RAM at `dest`.
    Read,
    /// `VerifyFlashSector_Core(src, tgt, size) -> u32`: RAM at `src`
    /// against `size` bytes of the chip at `tgt`; 0, or the chip address of
    /// the first byte that differs.
    Verify,
}

impl Routine {
    /// The library's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Routine::ReadByte => "ReadFlash1",
            Routine::Read => "ReadFlash_Core",
            Routine::Verify => "VerifyFlashSector_Core",
        }
    }

    /// The hardware library call that stands in for it.
    pub fn call(self) -> &'static str {
        match self {
            Routine::ReadByte => "runtime::hw::flash::read_byte",
            Routine::Read => "runtime::hw::flash::read",
            Routine::Verify => "runtime::hw::flash::verify",
        }
    }
}

/// One compiled form of a routine: its code's length in bytes (to the
/// return, without the alignment padding the copy also takes) and the
/// FNV-1a hash of that code.
#[derive(Clone, Copy, Debug)]
pub struct Signature {
    pub routine: Routine,
    pub compiler: &'static str,
    pub len: u32,
    pub hash: u64,
}

/// Every compiled form known. Taken from pret's builds of Emerald
/// (`pokeemerald.gba` and `pokeemerald_modern.gba`, by their map's
/// symbols) and checked against the five games' copies by `just holes`
/// (`analysis::flash`);
/// agbcc's and modern GCC's `ReadFlash1` are the same two instructions.
pub const SIGNATURES: &[Signature] = &[
    Signature {
        routine: Routine::ReadByte,
        compiler: "agbcc, modern GCC",
        len: 4,
        hash: 0x67dc_677e_8ae0_8cd8,
    },
    Signature {
        routine: Routine::Read,
        compiler: "agbcc",
        len: 34,
        hash: 0x8287_6d53_84ae_4e77,
    },
    Signature {
        routine: Routine::Verify,
        compiler: "agbcc",
        len: 46,
        hash: 0x16c5_a8f5_2ff3_819c,
    },
    Signature {
        routine: Routine::Read,
        compiler: "modern GCC",
        len: 22,
        hash: 0x9d26_0215_b562_07da,
    },
    Signature {
        routine: Routine::Verify,
        compiler: "modern GCC",
        len: 34,
        hash: 0x1c5b_f733_7cbc_cf70,
    },
];

/// 64-bit FNV-1a.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The routine `code` (the bytes at a call's target, at least as long as
/// the routine) begins with, and its signature, from [`SIGNATURES`].
pub fn recognise(code: &[u8]) -> Option<&'static Signature> {
    recognise_in(code, SIGNATURES)
}

/// [`recognise`] over a given table. The longest match wins, so a
/// signature can't be taken for a shorter one that is its prefix.
pub fn recognise_in<'a>(code: &[u8], sigs: &'a [Signature]) -> Option<&'a Signature> {
    sigs.iter()
        .filter(|s| {
            code.get(..s.len as usize)
                .is_some_and(|c| fnv1a(c) == s.hash)
        })
        .max_by_key(|s| s.len)
}

/// The most bytes a routine in [`SIGNATURES`] takes: what a caller reads at
/// a call's target to recognise it.
pub const MAX_LEN: usize = 46;

/// `ReadFlash1(addr) -> u8`: one byte of the chip (its ID in ID mode, or
/// the data).
pub fn read_byte(save: &Save, addr: u32) -> u8 {
    save.read(addr as u16)
}

/// `ReadFlash_Core(src, dest, size)`: `dest.len()` bytes of the chip from
/// `src` on.
pub fn read(save: &Save, src: u32, dest: &mut [u8]) {
    for (i, d) in dest.iter_mut().enumerate() {
        *d = save.read(src.wrapping_add(i as u32) as u16);
    }
}

/// `VerifyFlashSector_Core(src, tgt, size) -> u32`: `src` (RAM) against
/// the chip from `tgt` on; 0 if they agree, else the chip address of the
/// first byte that differs.
pub fn verify(save: &Save, src: &[u8], tgt: u32) -> u32 {
    for (i, &b) in src.iter().enumerate() {
        let a = tgt.wrapping_add(i as u32);
        if save.read(a as u16) != b {
            return a;
        }
    }
    0
}

#[cfg(test)]
mod tests;
