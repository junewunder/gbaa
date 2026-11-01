//! The BIOS calls a native program makes, as functions over values and
//! slices (docs/native-port-design.md, section 2): no registers, no
//! addresses. The arithmetic is the play build's own ([`crate::bios`]'s
//! `math`, refactored to values); the decompressors here decode as the
//! play build's do, byte for byte, without its cycle counts (a test runs
//! both on the same data).
//!
//! `CpuSet` and `CpuFastSet` are copies and fills (`copy_from_slice`,
//! `fill`, [`crate::video::Video::copy_vram16`]); the waits are
//! [`super::wait_vblank`] and friends.

pub use crate::bios::math::{arc_tan2_of, bg_affine, obj_affine, quot_rem as div, sqrt};

/// The cycles `CpuSet` from `src` to `dst` with control word `ctl` (its
/// r2: count, fill, words) takes under `WAITCNT` `waitcnt`: the play
/// build's own price (`crate::bios`), for [`super::spend`]. The addresses
/// only say which memories the copy touches.
pub fn cpu_set_cycles(waitcnt: u16, src: u32, dst: u32, ctl: u32) -> u32 {
    let mut w = crate::waits::Waits::default();
    w.set_waitcnt(waitcnt);
    crate::bios::decomp::cpu_set_cycles(&w, src, dst, ctl)
}

/// `ArcTan2`: the angle of (x, y), 0..0xFFFF for 0..2π.
pub fn arctan2(x: i32, y: i32) -> u16 {
    arc_tan2_of(x, y).0
}

/// Where decompressed bytes go: a slice, written a byte at a time (the
/// `Wram` calls) or a halfword at a time (`Vram`, where a byte store
/// would be lost: each even byte waits for its odd partner). Writes past
/// the slice's end are dropped.
struct Out<'a> {
    dst: &'a mut [u8],
    at: usize,
    vram: bool,
    half: u8,
}

impl Out<'_> {
    fn put(&mut self, b: u8) {
        let at = self.at;
        if !self.vram {
            if let Some(d) = self.dst.get_mut(at) {
                *d = b;
            }
        } else if at & 1 != 0 {
            if at < self.dst.len() {
                self.dst[at - 1] = self.half;
                self.dst[at] = b;
            }
        } else {
            self.half = b;
        }
        self.at += 1;
    }

    /// The byte at `i` as the destination holds it now (a held even byte
    /// isn't there yet).
    fn get(&self, i: usize) -> u8 {
        self.dst.get(i).copied().unwrap_or(0)
    }
}

/// The size a compressed blob's header gives (its bits 8-31).
pub fn uncompressed_len(src: &[u8]) -> usize {
    (u32::from_le_bytes([src[0], src[1], src[2], src[3]]) >> 8) as usize
}

/// `LZ77UnCompWram` (`vram` false) or `LZ77UnCompVram` (true): `src`
/// from its header on into `dst`. A back-reference reads the destination
/// as it stands, as the BIOS does. Returns the bytes written.
pub fn lz77(src: &[u8], dst: &mut [u8], vram: bool) -> usize {
    let mut remaining = uncompressed_len(src);
    let mut s = 4;
    let mut out = Out {
        dst,
        at: 0,
        vram,
        half: 0,
    };
    let byte = |i: usize| src.get(i).copied().unwrap_or(0);
    'blocks: while remaining > 0 {
        let flags = byte(s);
        s += 1;
        for bit in (0..8).rev() {
            if remaining == 0 {
                break 'blocks;
            }
            if flags >> bit & 1 != 0 {
                let block = (byte(s) as usize) << 8 | byte(s + 1) as usize;
                s += 2;
                let mut from = out.at.wrapping_sub((block & 0xFFF) + 1);
                for _ in 0..(block >> 12) + 3 {
                    let b = out.get(from);
                    out.put(b);
                    from = from.wrapping_add(1);
                    remaining = remaining.saturating_sub(1);
                }
            } else {
                out.put(byte(s));
                s += 1;
                remaining -= 1;
            }
        }
    }
    out.at
}

/// `RLUnCompWram`/`Vram`: `src` from its header on into `dst`, padded with
/// zeros to a multiple of 4 bytes as the play build (mGBA) pads. Returns
/// where the output ended, the padding included.
pub fn rl(src: &[u8], dst: &mut [u8], vram: bool) -> usize {
    let mut remaining = uncompressed_len(src);
    let padding = 4usize.wrapping_sub(remaining) & 3;
    let mut s = 4;
    let mut out = Out {
        dst,
        at: 0,
        vram,
        half: 0,
    };
    let byte = |i: usize| src.get(i).copied().unwrap_or(0);
    while remaining > 0 {
        let flag = byte(s);
        s += 1;
        if flag & 0x80 != 0 {
            let b = byte(s);
            s += 1;
            for _ in 0..((flag & 0x7F) as usize + 3).min(remaining) {
                out.put(b);
                remaining -= 1;
            }
        } else {
            for _ in 0..(flag as usize + 1).min(remaining) {
                out.put(byte(s));
                s += 1;
                remaining -= 1;
            }
        }
    }
    if !vram {
        for _ in 0..padding {
            out.put(0);
        }
        return out.at;
    }
    // VRAM, as the play build pads it: an odd end drops the held byte and
    // counts it as padding, then whole halfwords of zero.
    let mut at = out.at;
    let mut padding = padding as i32;
    if at & 1 != 0 {
        padding -= 1;
        at += 1;
    }
    while padding > 0 {
        if at + 1 < out.dst.len() {
            out.dst[at..at + 2].fill(0);
        }
        padding -= 2;
        at += 2;
    }
    at
}

#[cfg(test)]
mod tests;
