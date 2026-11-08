//! libgcc's soft-float double multiply and divide (`__muldf3` and
//! `__divdf3`, from `libgcc/config/arm/ieee754-df.S`), recognised by their
//! code, so every game linked with them gets the same stand-in.
//!
//! The two call their special-case paths (`Lml_s`, `Ldv_s`: a zero, a
//! denormal, an infinity or a NaN) by `bleq`, and those return with r4 and
//! r5 changed (the exponents, renormalised): a real break of the calling
//! convention the lift keeps (`docs/research/native-holes.md`, "Functions
//! that change r4-r11 or sp"). Each is one call of the runtime's soft
//! float instead (`runtime::softfloat`, bit-exact on libgcc's results):
//! the summaries take a recognised entry as a leaf that reads r0-r3 and
//! returns r0-r1 (`codegen::saves`), the lift prints it as that call, and
//! the functions discovery found inside the code (the special-case paths,
//! called only from it) aren't summarised or printed.
//!
//! As in [`crate::flash`], a routine is its code's length and FNV-1a hash
//! ([`crate::flash::fnv1a`]), not its bytes. The code is position
//! independent (every branch in it is relative and lands inside it), so
//! one hash holds wherever the linker put it. `__divdf3` shares
//! `__muldf3`'s tails (underflow, infinity, NaN), so its signature starts
//! at `__muldf3`'s entry, `entry` bytes before its own; `__muldf3` alone
//! is self-contained (Emerald Rogue has a second copy of it with no
//! `__divdf3` after).

use crate::flash::fnv1a;

/// One of the routines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Routine {
    /// `__muldf3(a, b) -> a * b`: a in r0 (low word) and r1, b in r2 and
    /// r3, the result in r0 and r1.
    Mul,
    /// `__divdf3(a, b) -> a / b`, as [`Routine::Mul`].
    Div,
}

impl Routine {
    /// libgcc's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Routine::Mul => "__muldf3",
            Routine::Div => "__divdf3",
        }
    }

    /// The runtime function that stands in for it (`fn(u64, u64) -> u64`
    /// over the doubles' bits).
    pub fn call(self) -> &'static str {
        match self {
            Routine::Mul => "runtime::softfloat::muldf3",
            Routine::Div => "runtime::softfloat::divdf3",
        }
    }
}

/// One compiled form of a routine: the code from `entry` bytes before its
/// entry, `len` bytes long, hashes to `hash`. Always ARM code.
#[derive(Clone, Copy, Debug)]
pub struct Signature {
    pub routine: Routine,
    pub source: &'static str,
    pub entry: u32,
    pub len: u32,
    pub hash: u64,
    /// Its first instruction, which [`scan`] looks for at every word
    /// before it hashes.
    pub first: u32,
}

/// Every compiled form known. Read from moonbeam's and Emerald Rogue's
/// ROMs (and Emerald Imperium's, the same), where the two lie at
/// 0x08036480 and 0x082f8d30: `__muldf3` 0x290 bytes to the end of
/// `Lml_s`, `__divdf3` then 0x20c bytes to the end of `Ldv_s`.
pub const SIGNATURES: &[Signature] = &[
    Signature {
        routine: Routine::Mul,
        source: "libgcc ieee754-df.S, ARMv4T (devkitARM)",
        entry: 0,
        len: 0x290,
        hash: 0x2301_33d9_e3a1_618c,
        // push {r4, r5, r6, lr}
        first: 0xe92d_4070,
    },
    Signature {
        routine: Routine::Div,
        source: "libgcc ieee754-df.S, ARMv4T (devkitARM)",
        entry: 0x290,
        len: 0x49c,
        hash: 0x3e61_6bde_4b8e_c613,
        // push {r4, r5, r6, lr}
        first: 0xe92d_4070,
    },
];

/// A routine found in a ROM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub routine: Routine,
    /// Its entry (ARM: the key is the address).
    pub entry: u32,
    /// The code its signature covers, `start..end`: a function discovery
    /// found in there, other than a routine's entry, is one of its inner
    /// paths.
    pub start: u32,
    pub end: u32,
}

/// The routine whose entry is `entry` in `image`, an image loaded at
/// `base`, or `None`. An odd `entry` (Thumb) is never one.
pub fn recognise(image: &[u8], base: u32, entry: u32) -> Option<Found> {
    recognise_in(image, base, entry, SIGNATURES)
}

/// [`recognise`] over a given table.
pub fn recognise_in(image: &[u8], base: u32, entry: u32, sigs: &[Signature]) -> Option<Found> {
    if entry & 3 != 0 {
        return None;
    }
    sigs.iter().find_map(|s| {
        let start = entry.checked_sub(s.entry)?;
        let off = start.checked_sub(base)? as usize;
        let code = image.get(off..off.checked_add(s.len as usize)?)?;
        (fnv1a(code) == s.hash).then_some(Found {
            routine: s.routine,
            entry,
            start,
            end: start + s.len,
        })
    })
}

/// Every routine in `image` (loaded at `base`), wherever discovery may
/// or may not have found it: each word that is a signature's first
/// instruction is tried whole.
pub fn scan(image: &[u8], base: u32) -> Vec<Found> {
    scan_in(image, base, SIGNATURES)
}

/// [`scan`] over a given table.
pub fn scan_in(image: &[u8], base: u32, sigs: &[Signature]) -> Vec<Found> {
    let mut found = Vec::new();
    for (i, w) in image.as_chunks::<4>().0.iter().enumerate() {
        let w = u32::from_le_bytes(*w);
        let start = base + 4 * i as u32;
        for s in sigs.iter().filter(|s| s.first == w) {
            if let Some(f) = recognise_in(image, base, start + s.entry, std::slice::from_ref(s)) {
                found.push(f);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests;
