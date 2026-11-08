//! The memory calls: `CpuSet`, `CpuFastSet` and the decompressors. They go
//! through the bus like the BIOS's own loads and stores, so mirrors, VRAM's
//! byte-store rule and I/O events all apply. Like mGBA, a source in the
//! BIOS region (below `0x02000000`) does nothing: the real BIOS refuses to
//! read itself.
//!
//! `CpuSet`, `CpuFastSet` and `LZ77` return the cycles they take, as mGBA
//! counts them: its HLE BIOS's loops for the two copies, its own tally for
//! LZ77. Each memory access costs [`access`]: the region's wait states.

use crate::bus::Bus;
use crate::waits;

/// The cycles a data access to `addr` takes: 1 plus the region's wait
/// states ([`crate::waits::Waits`]). `seq` for the later words of an
/// `ldm`/`stm` burst.
pub(super) fn access(bus: &Bus, addr: u32, word: bool, seq: bool) -> u32 {
    access_in(&bus.waits, addr, word, seq)
}

/// [`access`] under `w`'s wait states.
pub(crate) fn access_in(w: &waits::Waits, addr: u32, word: bool, seq: bool) -> u32 {
    let r = waits::region(addr);
    let wait = match (word, seq) {
        (false, false) => w.nonseq16[r],
        (false, true) => w.seq16[r],
        (true, false) => w.nonseq32[r],
        (true, true) => w.seq32[r],
    };
    1 + wait as u32
}

/// Whether `addr` is a source the BIOS will read.
fn readable(addr: u32) -> bool {
    addr >= 0x0200_0000
}

/// `CpuSet` (r0: source, r1: destination, r2: count in bits 0-20, bit 24
/// fill, bit 26 words rather than halfwords). Registers end as in mGBA's
/// HLE BIOS, which matches the original's: word copies leave r0 and r1
/// past the data (a word fill moves r0 by one word), halfword ones leave
/// them alone, and r3 = 0x170.
///
/// Time: mGBA's loop, `cmp; ldmlt; stmlt; blt` (no `ldm` for a fill), and
/// about 25 cycles around it.
pub fn cpu_set(bus: &mut Bus, r: &mut [u32; 15]) -> u32 {
    if !readable(r[0]) {
        return 0;
    }
    let count = r[2] & 0x1F_FFFF;
    let fill = r[2] & 1 << 24 != 0;
    let word = r[2] & 1 << 26 != 0;
    let cycles = cpu_set_cycles(&bus.waits, r[0], r[1], r[2]);
    if word {
        let fill_word = if fill {
            let v = bus.read32(r[0]);
            r[0] = r[0].wrapping_add(4);
            Some(v)
        } else {
            None
        };
        for _ in 0..count {
            let v = fill_word.unwrap_or_else(|| {
                let v = bus.read32(r[0]);
                r[0] = r[0].wrapping_add(4);
                v
            });
            bus.write32(r[1], v);
            r[1] = r[1].wrapping_add(4);
        }
    } else {
        let (src, dst) = (r[0] & !1, r[1] & !1);
        for i in 0..count {
            let v = bus.read16(if fill { src } else { src.wrapping_add(2 * i) });
            bus.write16(dst.wrapping_add(2 * i), v);
        }
    }
    r[3] = 0x170;
    cycles
}

/// The cycles `CpuSet` from `src` to `dst` with control word `ctl` (r2)
/// takes under `w`'s wait states, as [`cpu_set`] counts them: the play
/// build's and the native library's (`hw::bios::cpu_set_cycles`) price.
pub fn cpu_set_cycles(w: &waits::Waits, src: u32, dst: u32, ctl: u32) -> u32 {
    let count = ctl & 0x1F_FFFF;
    let fill = ctl & 1 << 24 != 0;
    let word = ctl & 1 << 26 != 0;
    // cmp, the branch back, and an I cycle for each load.
    let mut cycles = 25 + count * (1 + 3 + !fill as u32);
    if count > 0 {
        cycles += count * access_in(w, dst, word, false);
        if !fill {
            cycles += count * access_in(w, src, word, false);
        }
    }
    cycles
}

/// `CpuFastSet` (r0: source, r1: destination, r2: word count in bits 0-20,
/// bit 24 fill). Words move eight at a time, so the count rounds up to a
/// multiple of 8. Registers end as in mGBA's HLE BIOS: r1 (and r0 for a
/// copy) past the data, r2 = r1 + count * 4 as it was on entry, r3 the
/// last word loaded.
///
/// Time: mGBA's loop, `cmp; ldmltia {r3-r10}; stmltia {r3-r10}; blt` (no
/// `ldm` for a fill), and about 30 cycles around it.
pub fn cpu_fast_set(bus: &mut Bus, r: &mut [u32; 15]) -> u32 {
    if !readable(r[0]) {
        return 0;
    }
    let fill = r[2] & 1 << 24 != 0;
    let cycles = cpu_fast_set_cycles(&bus.waits, r[0], r[1], r[2]);
    r[3] = r[2] << 12;
    r[2] = r[1].wrapping_add((r[2] & 0x1F_FFFF) * 4);
    if fill {
        r[3] = bus.read32(r[0]);
    }
    while (r[1] as i32) < (r[2] as i32) {
        for k in 0..8 {
            if !fill {
                let v = bus.read32(r[0]);
                r[0] = r[0].wrapping_add(4);
                if k == 0 {
                    r[3] = v;
                }
                bus.write32(r[1], v);
            } else {
                bus.write32(r[1], r[3]);
            }
            r[1] = r[1].wrapping_add(4);
        }
    }
    cycles
}

/// The cycles `CpuFastSet` from `src` to `dst` with control word `ctl`
/// (r2) takes under `w`'s wait states, as [`cpu_fast_set`] counts them:
/// the play build's and the native library's (`hw::bios`) price.
pub fn cpu_fast_set_cycles(w: &waits::Waits, src: u32, dst: u32, ctl: u32) -> u32 {
    let fill = ctl & 1 << 24 != 0;
    let bursts = (ctl & 0x1F_FFFF).div_ceil(8);
    let burst = |addr: u32| access_in(w, addr, true, false) + 7 * access_in(w, addr, true, true);
    let mut cycles = 30 + bursts * (1 + 3 + !fill as u32);
    if bursts > 0 {
        cycles += bursts * burst(dst);
        if !fill {
            cycles += bursts * burst(src);
        }
    }
    cycles
}

/// Writes decompressed bytes either one at a time (the `Wram` variants)
/// or in halfwords (`Vram`, which ignores byte stores): each even byte is
/// held until its odd partner arrives.
struct Out {
    dst: u32,
    vram: bool,
    half: u16,
}

impl Out {
    /// Returns the cycles the store took (none for a held byte).
    fn put(&mut self, bus: &mut Bus, b: u8) -> u32 {
        let mut cycles = 0;
        if !self.vram {
            cycles = access(bus, self.dst, false, false);
            bus.write8(self.dst, b);
        } else if self.dst & 1 != 0 {
            self.half |= (b as u16) << 8;
            cycles = access(bus, self.dst, false, false);
            bus.write16(self.dst ^ 1, self.half);
        } else {
            self.half = b as u16;
        }
        self.dst = self.dst.wrapping_add(1);
        cycles
    }
}

/// A mod's replacement for the compressed blob at `src`
/// ([`Bus::asset_overrides`]), if it has one: one branch without a mod.
fn asset_override(bus: &Bus, src: u32) -> Option<&'static [u8]> {
    let o = bus.asset_overrides;
    if o.is_empty() {
        return None;
    }
    o.binary_search_by_key(&src, |e| e.0).ok().map(|i| o[i].1)
}

/// Write an override's bytes at `dst` as the call would have written its
/// own (bytes, or halfwords for VRAM). The decoder has run first, so
/// registers, timing and the reads of its back-references are as without
/// the mod; the override is the header's size (checked when the mod
/// loads), so it covers exactly what the decoder wrote.
fn write_override(bus: &mut Bus, dst: u32, vram: bool, bytes: &[u8]) {
    let mut out = Out { dst, vram, half: 0 };
    for &b in bytes {
        out.put(bus, b);
    }
}

/// `LZ77UnCompWram`/`Vram` (r0: source, r1: destination; `pc` is the
/// caller's, the `swi`'s address plus two fetches). The header word
/// is type 0x10 and the decompressed size in bits 8-31; then a flag byte
/// before each 8 blocks, a set bit meaning a back-reference (4 bits of
/// length - 3, 12 bits of distance - 1, big-endian) and a clear one a
/// literal byte. A back-reference reads the destination as it stands,
/// like the BIOS, and finishes even past the size (data that does that
/// overruns on hardware too). Ends with r0 and r1 past the data and
/// r3 = 0 (mGBA).
pub fn lz77(bus: &mut Bus, r: &mut [u32; 15], vram: bool, pc: u32) -> u32 {
    let mut src = r[0];
    if src & 0x0E00_0000 == 0 {
        return 0;
    }
    // mGBA's tally: 14 a step, 18 more a block, 10 more a copied byte (and 4
    // for VRAM's halfword handling); loads cost an I cycle too.
    let load = |bus: &mut Bus, addr: u32, word: bool| {
        let wait = 1 + access(bus, addr, word, false);
        stalled(bus, pc, addr, wait)
    };
    let over = asset_override(bus, src).map(|b| (b, r[1]));
    let mut cycles = 20 + load(bus, src, true);
    let mut remaining = bus.read32(src) >> 8;
    src = src.wrapping_add(4);
    let mut out = Out {
        dst: r[1],
        vram,
        half: 0,
    };
    let put = |bus: &mut Bus, out: &mut Out, b: u8| {
        let at = out.dst;
        match out.put(bus, b) {
            0 => 0,
            wait => stalled(bus, pc, at, wait),
        }
    };
    'blocks: while remaining > 0 {
        cycles += 14 + load(bus, src, false);
        let flags = bus.read8(src);
        src = src.wrapping_add(1);
        for bit in (0..8).rev() {
            if remaining == 0 {
                break 'blocks;
            }
            cycles += 14 + 18;
            if flags >> bit & 1 != 0 {
                cycles += load(bus, src.wrapping_add(1), false) + load(bus, src, false);
                let block = (bus.read8(src) as u32) << 8 | bus.read8(src.wrapping_add(1)) as u32;
                src = src.wrapping_add(2);
                let mut from = out.dst.wrapping_sub((block & 0xFFF) + 1);
                for _ in 0..(block >> 12) + 3 {
                    cycles += 10 + load(bus, from, false) + if vram { 4 } else { 0 };
                    let b = bus.read8(from);
                    cycles += put(bus, &mut out, b);
                    from = from.wrapping_add(1);
                    remaining = remaining.saturating_sub(1);
                }
            } else {
                cycles += load(bus, src, false);
                let b = bus.read8(src);
                src = src.wrapping_add(1);
                cycles += put(bus, &mut out, b);
                remaining -= 1;
            }
        }
    }
    r[0] = src;
    r[1] = out.dst;
    r[3] = 0;
    if let Some((bytes, dst)) = over {
        write_override(bus, dst, vram, bytes);
    }
    cycles.max(0) as u32
}

/// What one of the BIOS's own accesses (`wait` cycles, as [`lz77`] counts
/// them) costs in mGBA: called from ROM with the prefetch buffer on, an
/// access below the cartridge goes through its prefetch stall, as if the
/// `swi` at `pc` made it ([`crate::waits::Waits::stall`]). That can be
/// negative: mGBA's HLE LZ77 into VRAM runs faster from ROM than from RAM.
fn stalled(bus: &mut Bus, pc: u32, addr: u32, wait: u32) -> i32 {
    let r = waits::region(pc);
    if !bus.waits.prefetch || r < 8 || addr >= 0x0800_0000 {
        return wait as i32;
    }
    bus.waits.stall(r, pc, &mut bus.prefetched, wait as i32)
}

/// `RLUnCompWram`/`Vram` (r0: source, r1: destination). The header word is
/// type 0x30 and the size; then runs, each a flag byte: bit 7 set means
/// the next byte repeated (flag & 0x7F) + 3 times, clear means the next
/// flag + 1 bytes copied. Like mGBA, the output is padded with zeros to a
/// multiple of 4 bytes. Ends with r0 and r1 past the data.
pub fn rl(bus: &mut Bus, r: &mut [u32; 15], vram: bool) {
    let mut src = r[0];
    if src & 0x0E00_0000 == 0 {
        return;
    }
    let over = asset_override(bus, src).map(|b| (b, r[1]));
    let mut remaining = bus.read32(src & !3) >> 8;
    let mut padding = (4u32.wrapping_sub(remaining) & 3) as i32;
    src = src.wrapping_add(4);
    let mut out = Out {
        dst: r[1],
        vram,
        half: 0,
    };
    while remaining > 0 {
        let flag = bus.read8(src);
        src = src.wrapping_add(1);
        if flag & 0x80 != 0 {
            let b = bus.read8(src);
            src = src.wrapping_add(1);
            for _ in 0..((flag & 0x7F) as u32 + 3).min(remaining) {
                out.put(bus, b);
                remaining -= 1;
            }
        } else {
            for _ in 0..(flag as u32 + 1).min(remaining) {
                let b = bus.read8(src);
                src = src.wrapping_add(1);
                out.put(bus, b);
                remaining -= 1;
            }
        }
    }
    let mut dst = out.dst;
    if vram {
        if dst & 1 != 0 {
            padding -= 1;
            dst = dst.wrapping_add(1);
        }
        while padding > 0 {
            bus.write16(dst, 0);
            padding -= 2;
            dst = dst.wrapping_add(2);
        }
    } else {
        for _ in 0..padding {
            bus.write8(dst, 0);
            dst = dst.wrapping_add(1);
        }
    }
    r[0] = src;
    r[1] = dst;
    if let Some((bytes, dst)) = over {
        write_override(bus, dst, vram, bytes);
    }
}

/// `HuffUnComp` (r0: source, word-aligned; r1: destination). The header
/// word is type 0x20 | the data's unit width in bits (bits 0-3; 4 or 8)
/// and the decompressed size in bits 8-31; then the tree, a size byte
/// (`(size + 1) * 2` bytes of tree in all) and the nodes from the root.
/// A node's bits 0-5 are its children's offset (they sit at
/// `(node & !1) + offset * 2 + 2`, left then right) and bits 7 and 6 say
/// the left or right child is a leaf, whose byte is a unit. After the tree
/// come 32-bit words of path bits, highest first: 0 goes left, 1 right.
/// Units fill each output word from its low bits and words go out whole.
/// As in mGBA, widths that don't divide 32 (and 1) do nothing, and 0 is
/// taken as 8. Ends with r0 and r1 past the data.
pub fn huffman(bus: &mut Bus, r: &mut [u32; 15]) {
    let mut src = r[0] & !3;
    if src & 0x0E00_0000 == 0 {
        return;
    }
    let header = bus.read32(src);
    let mut remaining = (header >> 8) as i32;
    let width = match header & 0xF {
        0 => 8,
        w => w,
    };
    if 32 % width != 0 || width == 1 {
        return;
    }
    let root = src.wrapping_add(5);
    src = root.wrapping_add((bus.read8(src.wrapping_add(4)) as u32) << 1 | 1);
    let mut dst = r[1];
    let (mut at, mut node) = (root, bus.read8(root));
    let (mut word, mut filled) = (0u32, 0);
    while remaining > 0 {
        let bits = bus.read32(src);
        src = src.wrapping_add(4);
        for i in 0..32 {
            if remaining <= 0 {
                break;
            }
            let right = bits << i & 0x8000_0000 != 0;
            let child = (at & !1).wrapping_add((node as u32 & 0x3F) * 2 + 2) + right as u32;
            if node & if right { 0x40 } else { 0x80 } == 0 {
                at = child;
                node = bus.read8(at);
                continue;
            }
            word |= (bus.read8(child) as u32 & ((1 << width) - 1)) << filled;
            filled += width;
            (at, node) = (root, bus.read8(root));
            if filled == 32 {
                bus.write32(dst, word);
                dst = dst.wrapping_add(4);
                remaining -= 4;
                (word, filled) = (0, 0);
            }
        }
    }
    r[0] = src;
    r[1] = dst;
}

/// `BitUnPack` (r0: source, r1: destination, r2: the parameters: source
/// length in bytes (halfword), source and destination widths in bits
/// (bytes), and an offset word). Each source unit, low bits first, is
/// widened; the offset (bits 0-30) is added to non-zero units, or to all of
/// them if bit 31 is set. Output goes out a word at a time, so a final
/// partial word isn't written. Bad widths do nothing (mGBA). Ends with r0
/// and r1 past the data.
pub fn bit_unpack(bus: &mut Bus, r: &mut [u32; 15]) {
    let (mut src, mut dst, info) = (r[0], r[1], r[2]);
    if !readable(src) {
        return;
    }
    let mut len = bus.read16(info) as u32;
    let src_width = bus.read8(info.wrapping_add(2)) as u32;
    let dst_width = bus.read8(info.wrapping_add(3)) as u32;
    if !matches!(src_width, 1 | 2 | 4 | 8) || !matches!(dst_width, 1 | 2 | 4 | 8 | 16 | 32) {
        return;
    }
    let bias = bus.read32(info.wrapping_add(4));
    let (mut byte, mut bits_left) = (0u32, 0);
    let (mut word, mut filled) = (0u32, 0);
    while len > 0 || bits_left > 0 {
        if bits_left == 0 {
            byte = bus.read8(src) as u32;
            src = src.wrapping_add(1);
            len -= 1;
            bits_left = 8;
        }
        let mut unit = byte & ((1 << src_width) - 1);
        byte >>= src_width;
        bits_left -= src_width;
        if unit != 0 || bias & 0x8000_0000 != 0 {
            unit = unit.wrapping_add(bias & 0x7FFF_FFFF);
        }
        word |= unit << filled;
        filled += dst_width;
        if filled == 32 {
            bus.write32(dst, word);
            dst = dst.wrapping_add(4);
            word = 0;
            filled = 0;
        }
    }
    r[0] = src;
    r[1] = dst;
}

/// `Diff8bitUnFilterWram`/`Vram` and `Diff16bitUnFilter` (r0: source,
/// word-aligned; r1: destination). The header word is type 0x80 | the unit
/// size in bytes and the output size in bits 8-31; then the deltas, each
/// unit the running sum of those so far. `in16` reads halfword deltas,
/// `out16` writes halfwords: the 8-bit VRAM variant pairs its bytes, low
/// one first, and an odd size's last byte has no partner (mGBA). Ends with
/// r0 and r1 past the data.
pub fn unfilter(bus: &mut Bus, r: &mut [u32; 15], in16: bool, out16: bool) {
    let mut src = r[0] & !3;
    if src & 0x0E00_0000 == 0 {
        return;
    }
    let mut dst = r[1];
    let mut remaining = (bus.read32(src) >> 8) as i32;
    src = src.wrapping_add(4);
    let (mut sum, mut half) = (0u16, 0u16);
    while remaining > 0 {
        let delta = if in16 {
            bus.read16(src)
        } else {
            bus.read8(src) as u16
        };
        sum = sum.wrapping_add(delta);
        if out16 && !in16 {
            half = half >> 8 | sum << 8;
            if src & 1 != 0 {
                bus.write16(dst, half);
                dst = dst.wrapping_add(2);
                remaining -= 2;
            }
        } else if out16 {
            bus.write16(dst, sum);
            dst = dst.wrapping_add(2);
            remaining -= 2;
        } else {
            bus.write8(dst, sum as u8);
            dst = dst.wrapping_add(1);
            remaining -= 1;
        }
        src = src.wrapping_add(1 + in16 as u32);
    }
    r[0] = src;
    r[1] = dst;
}
