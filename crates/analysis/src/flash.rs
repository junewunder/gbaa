//! The flash save library's stack routines (Nintendo's `AgbFlash`, pret's
//! `agb_flash.c`), recognised by the code a game copies onto the stack.
//!
//! To read the flash chip the library can't run from ROM (the cartridge
//! bus is busy with the chip), so it copies three tiny routines into a
//! buffer in its caller's stack frame and calls them there: `ReadFlash1`
//! (one byte, for the chip's ID and status), `ReadFlash_Core` (a sector's
//! bytes into RAM) and `VerifyFlashSector_Core` (RAM against a sector).
//! The buffer lands wherever the caller's frame is, and the next copy
//! puts another routine at the same addresses, so a native build sees
//! code in RAM that changes (`docs/research/native-holes.md`). Each is
//! one call of the hardware library instead (`runtime::hw::flash`, the
//! [`Routine`]'s [`Routine::call`]): a call into RAM whose target holds a
//! recognised routine's bytes is that call, and the bytes are never run.
//!
//! The routines are recognised by their whole code, as compiled: agbcc's
//! (the SDK's own object, in Emerald, both Kirbys and Metroid: Zero
//! Mission) and modern GCC's (pret's `modern` build, Emerald Rogue). Only
//! each code's length and FNV-1a hash are kept here, not the bytes (they
//! are the ROMs'). A compiler's version not listed shows in the hole
//! report as before, RAM code that changed; listing it is a line in
//! [`SIGNATURES`].

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
/// symbols) and checked against the five games' copies by `just holes`;
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

#[cfg(test)]
mod tests;
