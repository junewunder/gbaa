//! The flash save library's stack routines as library calls
//! (docs/native-port-design.md, section 2, "Saves are the library's").
//!
//! Nintendo's `AgbFlash` copies three tiny routines onto the stack and
//! calls them there, because the cartridge bus can't fetch code from ROM
//! while it reads the chip; `analysis::flash` recognises them by those
//! bytes, at a call into RAM. A native program calls these instead, on
//! [`Hw::save`](super::Hw::save), the play build's own chip emulation
//! ([`crate::save::Save`]), so the chip's state (ID mode, the 1 Mbit
//! chip's bank) answers as it does for the copied routine's loads.
//!
//! Chip addresses are bus addresses (`0x0E00_0000` on); like the bus, the
//! chip sees their low 16 bits.

use crate::save::Save;

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
