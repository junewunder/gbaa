//! Provenance tags (assets/provenance, docs/decisions.md decision 2): for
//! every byte of EWRAM, IWRAM, palette RAM, VRAM and OAM, where its value
//! came from, a ROM address and the transforms on the way. Off unless the
//! `provenance` feature is on; a build without it pays nothing.
//!
//! A tag is a `u32`: the low 25 bits a ROM offset ([`NO_ROM`] when the byte
//! traces to no ROM byte), the top 7 the [`via`] bits of every step it took.
//! 0 is a byte never written.
//!
//! - **CPU stores** match the value stored against the last [`RING`] loads
//!   of the same width, newest first, and take that load's tags: a copy.
//!   No match is a computed value, [`via::DERIVED`]: with the origin of
//!   the newest remembered load that traces to ROM and isn't a word that
//!   looks like a pointer (a literal pool's), as a guess at the table it
//!   was built from; else from [`NO_ROM`].
//! - **DMA, `CpuSet` and `CpuFastSet`** copy each unit's tags from the read
//!   before it ([`enter_copy`]).
//! - **BIOS decompression** (LZ77, RL, Huffman, BitUnPack, the diff
//!   unfilters) tags every byte it writes with the blob's first byte:
//!   its origin is the compressed blob's address ([`enter_fixed`]).
//!
//! Bytes the sound FIFOs take, and every tagged store to palette, VRAM and
//! OAM, are counted by [`Region`] and class ([`written`]), and the ROM bytes
//! that reached the screen's memory are kept in two bitmaps ([`reached`]).
//!
//! A decompressed byte also keeps its offset in the unpacked data
//! ([`offset_at`]), so a tile knows where in its blob it sits; every other
//! byte's is 0.
//!
//! **The load log** (assets/load-log, docs/asset-extraction-design.md,
//! Recording): each BIOS decompression, `CpuSet`, `CpuFastSet` and DMA whose
//! source is ROM, or RAM with bytes copied from ROM, is a [`Load`], taken by
//! the host with [`take_loads`].
//! Process-wide state, for one machine a process (the headless runner's
//! case); state loads and rewinds don't restore it.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// Whether this build tags.
pub const ENABLED: bool = cfg!(feature = "provenance");

/// The steps a byte took, a tag's top bits.
pub mod via {
    /// A CPU store.
    pub const CPU: u32 = 1;
    /// BIOS `CpuSet`.
    pub const CPU_SET: u32 = 2;
    /// BIOS `CpuFastSet`.
    pub const FAST_SET: u32 = 4;
    /// A DMA transfer.
    pub const DMA: u32 = 8;
    /// BIOS LZ77 decompression.
    pub const LZ77: u32 = 16;
    /// BIOS RL, Huffman, BitUnPack or a diff unfilter.
    pub const UNPACK: u32 = 32;
    /// A CPU store of a value no recent load had: computed.
    pub const DERIVED: u32 = 64;
    /// Names, lowest bit first.
    pub const NAMES: [&str; 7] = [
        "cpu", "cpuset", "fastset", "dma", "lz77", "unpack", "derived",
    ];
}

pub const VIA_SHIFT: u32 = 25;
/// The origin of a byte that traces to no ROM byte.
pub const NO_ROM: u32 = (1 << VIA_SHIFT) - 1;
const ROM_BYTES: usize = 1 << VIA_SHIFT;

/// A tag's ROM offset, if it has one.
pub fn origin(tag: u32) -> Option<u32> {
    let o = tag & NO_ROM;
    (tag != 0 && o != NO_ROM).then_some(o)
}

/// A tag's [`via`] bits.
pub fn steps(tag: u32) -> u32 {
    tag >> VIA_SHIFT
}

const EWRAM: usize = 0;
const IWRAM: usize = EWRAM + 0x4_0000;
const PALETTE: usize = IWRAM + 0x8000;
const VRAM: usize = PALETTE + 0x400;
const OAM: usize = VRAM + 0x1_8000;
const SHADOW_SIZE: usize = OAM + 0x400;

static SHADOW: [AtomicU32; SHADOW_SIZE] = [const { AtomicU32::new(0) }; SHADOW_SIZE];
/// Each byte's offset in the unpacked data of the blob its tag names (0
/// unless decompressed).
static OFFSET: [AtomicU32; SHADOW_SIZE] = [const { AtomicU32::new(0) }; SHADOW_SIZE];

/// Where a tag is counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Vram = 0,
    Palette = 1,
    Oam = 2,
    Fifo = 3,
}

/// Classes: a tag's via bits, plus 128 when it has a ROM origin.
const CLASSES: usize = 256;
static WRITTEN: [AtomicU32; 4 * CLASSES] = [const { AtomicU32::new(0) }; 4 * CLASSES];
/// ROM bytes copied (no decompression on the way) into palette, VRAM or
/// OAM, the first bytes of blobs decompressed into them, and the guessed
/// tables of computed bytes: bitmaps.
static COPIED: [AtomicU32; ROM_BYTES / 32] = [const { AtomicU32::new(0) }; ROM_BYTES / 32];
static PACKED: [AtomicU32; ROM_BYTES / 32] = [const { AtomicU32::new(0) }; ROM_BYTES / 32];
static TABLES: [AtomicU32; ROM_BYTES / 32] = [const { AtomicU32::new(0) }; ROM_BYTES / 32];

/// What the writes in progress are.
const CPU: u32 = 0;
const COPY: u32 = 1;
const FIXED: u32 = 2;
static MODE: AtomicU32 = AtomicU32::new(CPU);
/// [`COPY`]: the via bit added; [`FIXED`]: the tag written.
static ARG: AtomicU32 = AtomicU32::new(0);
/// [`COPY`]: the last read's tags, a byte each, and their offsets.
static LAST: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static LAST_OFF: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
/// [`FIXED`]: the destination's start, where unpacked offset 0 goes; and
/// the end of the source read so far and the span written, for the load
/// log's calls whose registers don't say.
static FIXED_DST: AtomicU32 = AtomicU32::new(0);
static SRC_START: AtomicU32 = AtomicU32::new(0);
static SRC_END: AtomicU32 = AtomicU32::new(0);
static DST_LO: AtomicU32 = AtomicU32::new(u32::MAX);
static DST_HI: AtomicU32 = AtomicU32::new(0);

/// The CPU loads remembered, newest at `RING_POS - 1`.
const RING: usize = 8;
static RING_WIDTH: [AtomicU32; RING] = [const { AtomicU32::new(0) }; RING];
static RING_VALUE: [AtomicU32; RING] = [const { AtomicU32::new(0) }; RING];
static RING_TAGS: [AtomicU32; 4 * RING] = [const { AtomicU32::new(0) }; 4 * RING];
static RING_OFF: [AtomicU32; 4 * RING] = [const { AtomicU32::new(0) }; 4 * RING];
static RING_POS: AtomicU32 = AtomicU32::new(0);

/// The shadow index of `addr`, in the memories tagged.
fn slot(addr: u32) -> Option<usize> {
    let a = addr as usize;
    Some(match addr >> 24 {
        0x02 => EWRAM + (a & 0x3_FFFF),
        0x03 => IWRAM + (a & 0x7FFF),
        0x05 => PALETTE + (a & 0x3FF),
        0x06 => {
            let o = a & 0x1_FFFF;
            VRAM + if o >= 0x1_8000 { o - 0x8000 } else { o }
        }
        0x07 => OAM + (a & 0x3FF),
        _ => return None,
    })
}

/// Where the byte at `addr` came from: itself for ROM, else its tag, with
/// "never written" and untagged memory as [`NO_ROM`].
fn source(addr: u32) -> u32 {
    if (0x08..=0x0D).contains(&(addr >> 24)) {
        return addr & NO_ROM;
    }
    match slot(addr).map(|s| SHADOW[s].load(Relaxed)) {
        Some(t) if t != 0 => t,
        _ => NO_ROM,
    }
}

/// The unpacked offset of the byte at `addr` (0 for ROM and untagged
/// memory).
fn source_offset(addr: u32) -> u32 {
    slot(addr).map_or(0, |s| OFFSET[s].load(Relaxed))
}

/// `tag` having taken step `bits` too.
fn add(tag: u32, bits: u32) -> u32 {
    tag | bits << VIA_SHIFT
}

/// Forgets every byte's tag and offset. For a state load or a rewind: the
/// memory the tags described is gone, so its bytes trace to nothing until
/// the game loads them again, rather than to what was there before.
pub fn forget() {
    for t in SHADOW.iter().chain(OFFSET.iter()) {
        t.store(0, Relaxed);
    }
}

/// The tag at `addr` (0 if never written or not tagged).
pub fn tag_at(addr: u32) -> u32 {
    slot(addr).map_or(0, |s| SHADOW[s].load(Relaxed))
}

/// The offset in its blob's unpacked data of the byte at `addr`, for a
/// decompressed byte (0 otherwise).
pub fn offset_at(addr: u32) -> u32 {
    source_offset(addr)
}

/// Bytes stored to `region` with tags of `class` (via bits, plus 128 for
/// a ROM origin).
pub fn written(region: Region, class: usize) -> u32 {
    WRITTEN[region as usize * CLASSES + class].load(Relaxed)
}

/// The bitmaps of ROM bytes that reached palette, VRAM or OAM, word `i`
/// (bit `b` is offset `32 i + b`): copied, decompressed blobs' first
/// bytes, and computed bytes' guessed tables.
pub fn reached(i: usize) -> [u32; 3] {
    [
        COPIED[i].load(Relaxed),
        PACKED[i].load(Relaxed),
        TABLES[i].load(Relaxed),
    ]
}

pub const REACHED_WORDS: usize = ROM_BYTES / 32;

fn count(region: Region, tag: u32, bytes: u32) {
    let class = steps(tag) as usize | if origin(tag).is_some() { 128 } else { 0 };
    WRITTEN[region as usize * CLASSES + class].fetch_add(bytes, Relaxed);
    if region == Region::Fifo {
        return;
    }
    if let Some(o) = origin(tag) {
        let map = if steps(tag) & via::DERIVED != 0 {
            &TABLES
        } else if steps(tag) & (via::LZ77 | via::UNPACK) != 0 {
            &PACKED
        } else {
            &COPIED
        };
        map[o as usize / 32].fetch_or(1 << (o % 32), Relaxed);
    }
}

/// A CPU data load of `bytes` bytes from `addr` that read `v`.
pub fn cpu_load(bytes: u32, addr: u32, v: u32) {
    let addr = addr & !(bytes - 1);
    let i = RING_POS.load(Relaxed) as usize % RING;
    RING_WIDTH[i].store(bytes, Relaxed);
    RING_VALUE[i].store(v, Relaxed);
    for b in 0..bytes {
        RING_TAGS[4 * i + b as usize].store(source(addr + b), Relaxed);
        RING_OFF[4 * i + b as usize].store(source_offset(addr + b), Relaxed);
    }
    RING_POS.store((i + 1) as u32, Relaxed);
}

/// A read by DMA or a BIOS routine; remembered for the write after it.
pub fn read(bytes: u32, addr: u32) {
    let addr = addr & !(bytes - 1);
    match MODE.load(Relaxed) {
        COPY => {
            for b in 0..bytes {
                LAST[b as usize].store(source(addr + b), Relaxed);
                LAST_OFF[b as usize].store(source_offset(addr + b), Relaxed);
            }
        }
        FIXED => {
            // The source read so far: from its start, in its region (a
            // back-reference reads the destination).
            let start = SRC_START.load(Relaxed);
            if addr >= start && addr >> 24 == start >> 24 {
                SRC_END.fetch_max(addr + bytes, Relaxed);
            }
        }
        _ => {}
    }
}

/// The tags and offsets of a CPU store of `v`, `bytes` wide.
fn cpu_store(bytes: u32, v: u32, tags: &mut [u32; 4], offs: &mut [u32; 4]) {
    let newest = RING_POS.load(Relaxed) as usize + RING - 1;
    for k in 0..RING {
        let i = (newest - k) % RING;
        if RING_WIDTH[i].load(Relaxed) == bytes && RING_VALUE[i].load(Relaxed) == v {
            for b in 0..bytes as usize {
                tags[b] = add(RING_TAGS[4 * i + b].load(Relaxed), via::CPU);
                offs[b] = RING_OFF[4 * i + b].load(Relaxed);
            }
            return;
        }
    }
    // The newest load from tagged data that isn't a pointer: a table entry
    // the value may be built from.
    let fed = (0..RING)
        .map(|k| (newest - k) % RING)
        .filter(|&i| {
            let v = RING_VALUE[i].load(Relaxed);
            !(RING_WIDTH[i].load(Relaxed) == 4 && (0x02..0x0E).contains(&(v >> 24)))
        })
        .find_map(|i| origin(RING_TAGS[4 * i].load(Relaxed)));
    *tags = [add(fed.unwrap_or(NO_ROM), via::CPU | via::DERIVED); 4];
}

/// A store of `v`, `bytes` wide, to `addr`, by whoever is writing now.
pub fn write(bytes: u32, addr: u32, v: u32) {
    let addr = addr & !(bytes - 1);
    let fifo = (0x0400_00A0..0x0400_00A8).contains(&addr);
    let Some(s) = slot(addr) else {
        if !fifo {
            return;
        }
        let (mut tags, mut offs) = ([0; 4], [0; 4]);
        tags_now(bytes, addr, v, &mut tags, &mut offs);
        for &t in &tags[..bytes as usize] {
            count(Region::Fifo, t, 1);
        }
        return;
    };
    let (mut tags, mut offs) = ([0; 4], [0; 4]);
    tags_now(bytes, addr, v, &mut tags, &mut offs);
    if MODE.load(Relaxed) == FIXED {
        DST_LO.fetch_min(addr, Relaxed);
        DST_HI.fetch_max(addr + bytes, Relaxed);
    }
    let region = match s {
        PALETTE..VRAM => Some(Region::Palette),
        VRAM..OAM => Some(Region::Vram),
        OAM.. => Some(Region::Oam),
        _ => None,
    };
    // Palette and VRAM take a byte store as a halfword of it; OAM ignores it.
    let (s, tags, offs, n) = match (bytes, region) {
        (1, Some(Region::Oam)) => return,
        (1, Some(_)) => (s & !1, [tags[0]; 4], [offs[0]; 4], 2),
        _ => (s, tags, offs, bytes as usize),
    };
    for (b, &t) in tags[..n].iter().enumerate() {
        SHADOW[s + b].store(t, Relaxed);
        OFFSET[s + b].store(offs[b], Relaxed);
        if let Some(r) = region {
            count(r, t, 1);
        }
    }
}

fn tags_now(bytes: u32, addr: u32, v: u32, tags: &mut [u32; 4], offs: &mut [u32; 4]) {
    match MODE.load(Relaxed) {
        COPY => {
            let bit = ARG.load(Relaxed);
            for (b, t) in tags[..bytes as usize].iter_mut().enumerate() {
                *t = add(LAST[b].load(Relaxed), bit);
                offs[b] = LAST_OFF[b].load(Relaxed);
            }
        }
        FIXED => {
            let tag = ARG.load(Relaxed);
            *tags = [tag; 4];
            if origin(tag).is_some() {
                let at = addr.wrapping_sub(FIXED_DST.load(Relaxed));
                for (b, o) in offs.iter_mut().enumerate() {
                    *o = at.wrapping_add(b as u32);
                }
            }
        }
        _ => cpu_store(bytes, v, tags, offs),
    }
}

/// Bytes the sound FIFO took from a DMA's read: the last read's tags.
pub fn fifo_dma_word() {
    for t in &LAST {
        count(Region::Fifo, add(t.load(Relaxed), via::DMA), 1);
    }
}

/// What was writing before an `enter_*`; dropping it goes back to that.
pub struct Saved(u32, u32, u32);

impl Drop for Saved {
    fn drop(&mut self) {
        MODE.store(self.0, Relaxed);
        ARG.store(self.1, Relaxed);
        FIXED_DST.store(self.2, Relaxed);
    }
}

fn enter(mode: u32, arg: u32) -> Saved {
    let s = Saved(
        MODE.load(Relaxed),
        ARG.load(Relaxed),
        FIXED_DST.load(Relaxed),
    );
    MODE.store(mode, Relaxed);
    ARG.store(arg, Relaxed);
    s
}

/// Until the guard drops, writes copy the read before them, adding `bit`
/// ([`via`]).
pub fn enter_copy(bit: u32) -> Saved {
    enter(COPY, bit)
}

/// Until the guard drops, writes are of a blob decompressed from `src` (a
/// BIOS routine's source address) with `bit`; or, `src` `None`, values a
/// BIOS routine computed.
pub fn enter_fixed(src: Option<u32>, bit: u32) -> Saved {
    enter(
        FIXED,
        match src {
            Some(a) => add(source(a), bit),
            None => add(NO_ROM, bit),
        },
    )
}

/// Until the guard drops, writes are BIOS call `n`'s, with `src` and `dst`
/// its r0 and r1.
pub fn enter_bios(n: u8, src: u32, dst: u32) -> Saved {
    use crate::bios::call::*;
    let unpack = |bit| {
        let s = enter_fixed(Some(src), bit);
        FIXED_DST.store(dst, Relaxed);
        SRC_START.store(src, Relaxed);
        SRC_END.store(src, Relaxed);
        DST_LO.store(u32::MAX, Relaxed);
        DST_HI.store(0, Relaxed);
        s
    };
    match n {
        CPU_SET => enter_copy(via::CPU_SET),
        CPU_FAST_SET => enter_copy(via::FAST_SET),
        LZ77_UNCOMP_WRAM | LZ77_UNCOMP_VRAM => unpack(via::LZ77),
        BIT_UNPACK | HUFF_UNCOMP | RL_UNCOMP_WRAM | RL_UNCOMP_VRAM | DIFF8_UNFILTER_WRAM
        | DIFF8_UNFILTER_VRAM | DIFF16_UNFILTER => unpack(via::UNPACK),
        _ => enter_fixed(None, via::DERIVED),
    }
}

/// What a load was, a [`Load`]'s `op`.
pub mod op {
    pub const LZ77: u32 = 0;
    pub const RL: u32 = 1;
    pub const HUFF: u32 = 2;
    pub const BIT_UNPACK: u32 = 3;
    pub const DIFF: u32 = 4;
    pub const DMA: u32 = 5;
    pub const CPU_SET: u32 = 6;
    pub const FAST_SET: u32 = 7;
    /// Names, by op.
    pub const NAMES: [&str; 8] = [
        "lz77",
        "rl",
        "huff",
        "bitunpack",
        "diff",
        "dma",
        "cpuset",
        "fastset",
    ];
}

/// A run of a RAM source's bytes copied from ROM: `len` bytes from `tag`'s
/// origin on (for a decompressed run, from unpacked offset `offset` of the
/// blob at the origin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub tag: u32,
    pub offset: u32,
    pub len: u32,
}

/// One load: `src_len` bytes read from `src` (a decompression's, header
/// included and rounded up to 4; a copy's, its count times its unit) and
/// `dst_len` written from `dst`. A RAM source's ROM bytes are its `runs`
/// (none for a ROM source); `truncated` when there were more than
/// [`MAX_RUNS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Load<'a> {
    pub op: u32,
    pub src: u32,
    pub src_len: u32,
    pub dst: u32,
    pub dst_len: u32,
    pub truncated: bool,
    pub runs: &'a [Run],
}

/// A load's runs kept at most.
pub const MAX_RUNS: usize = 1024;
const HEAD: usize = 6;
/// Loads since the host last took them: a header of [`HEAD`] words, then
/// three words a run.
const LOG_WORDS: usize = 1 << 18;
static LOG: [AtomicU32; LOG_WORDS] = [const { AtomicU32::new(0) }; LOG_WORDS];
static LOG_LEN: AtomicU32 = AtomicU32::new(0);
static LOG_DROPPED: AtomicU32 = AtomicU32::new(0);

fn in_rom(addr: u32) -> bool {
    (0x08..=0x0D).contains(&(addr >> 24))
}

/// Logs a load whose source traces to ROM; drops any other.
fn log_load(op: u32, src: u32, src_len: u32, dst: u32, dst_len: u32) {
    if src_len == 0 {
        return;
    }
    // The runs go straight into the log after the header's room; the
    // header goes in last.
    let at = LOG_LEN.load(Relaxed) as usize;
    if at + HEAD > LOG_WORDS {
        LOG_DROPPED.fetch_add(1, Relaxed);
        return;
    }
    let run = |k: usize, i: usize| &LOG[at + HEAD + 3 * k + i];
    let (mut n, mut truncated) = (0, false);
    if !in_rom(src) {
        let mut prev: Option<(u32, u32)> = None;
        for a in src..src.wrapping_add(src_len) {
            let (t, off) = (source(a), source_offset(a));
            let copied = origin(t).is_some() && steps(t) & via::DERIVED == 0;
            if !copied {
                prev = None;
                continue;
            }
            let packed = steps(t) & (via::LZ77 | via::UNPACK) != 0;
            let joins = prev.is_some_and(|(pt, poff)| {
                let pp = steps(pt) & (via::LZ77 | via::UNPACK) != 0;
                pp == packed
                    && if packed {
                        origin(pt) == origin(t) && off == poff + 1
                    } else {
                        origin(t) == origin(pt).map(|o| o + 1)
                    }
            });
            if joins {
                run(n - 1, 2).fetch_add(1, Relaxed);
            } else if n == MAX_RUNS || at + HEAD + 3 * (n + 1) > LOG_WORDS {
                truncated = true;
            } else {
                run(n, 0).store(t, Relaxed);
                run(n, 1).store(off, Relaxed);
                run(n, 2).store(1, Relaxed);
                n += 1;
            }
            prev = Some((t, off));
        }
        if n == 0 {
            return;
        }
    }
    let head = [
        op | (truncated as u32) << 8 | (n as u32) << 16,
        src,
        src_len,
        dst,
        dst_len,
        0,
    ];
    for (k, w) in head.into_iter().enumerate() {
        LOG[at + k].store(w, Relaxed);
    }
    LOG_LEN.store((at + HEAD + 3 * n) as u32, Relaxed);
}

/// BIOS call `n` ended: `src`, `dst` and `info` its r0, r1 and r2 on entry,
/// `r` its registers now. Logs it if it loads.
pub fn bios_loaded(n: u8, src: u32, dst: u32, info: u32, r: &[u32; 15]) {
    use crate::bios::call::*;
    let count = info & 0x1F_FFFF;
    let fill = info & 1 << 24 != 0;
    // How far a decoder that leaves r0 and r1 past the data read and wrote.
    let past = || {
        (
            r[0].wrapping_sub(src).next_multiple_of(4),
            r[1].wrapping_sub(dst),
        )
    };
    // A decoder that doesn't: the spans its reads and writes covered.
    let spans = || {
        let (lo, hi) = (DST_LO.load(Relaxed), DST_HI.load(Relaxed));
        let end = SRC_END.load(Relaxed);
        (
            end.wrapping_sub(src).next_multiple_of(4),
            dst.min(lo),
            hi.saturating_sub(dst.min(lo)),
        )
    };
    let (op, src_len, dst, dst_len) = match n {
        CPU_SET => {
            let unit = if info & 1 << 26 != 0 { 4 } else { 2 };
            (
                op::CPU_SET,
                if fill { unit } else { count * unit },
                dst,
                count * unit,
            )
        }
        CPU_FAST_SET => {
            let bytes = count.div_ceil(8) * 32;
            (op::FAST_SET, if fill { 4 } else { bytes }, dst, bytes)
        }
        LZ77_UNCOMP_WRAM | LZ77_UNCOMP_VRAM | RL_UNCOMP_WRAM | RL_UNCOMP_VRAM | HUFF_UNCOMP => {
            let o = match n {
                HUFF_UNCOMP => op::HUFF,
                LZ77_UNCOMP_WRAM | LZ77_UNCOMP_VRAM => op::LZ77,
                _ => op::RL,
            };
            let (s, d) = past();
            (o, s, dst, d)
        }
        BIT_UNPACK | DIFF8_UNFILTER_WRAM | DIFF8_UNFILTER_VRAM | DIFF16_UNFILTER => {
            let (s, d, dl) = spans();
            let o = if n == BIT_UNPACK {
                op::BIT_UNPACK
            } else {
                op::DIFF
            };
            (o, s, d, dl)
        }
        _ => return,
    };
    log_load(op, src, src_len, dst, dst_len);
}

/// A DMA transfer ended: `count` units of `unit` bytes from `src` (stepping
/// by `src_step`) to `dst`. Logs it if its source traces to ROM.
pub fn dma_loaded(src: u32, dst: u32, count: u32, unit: u32, src_step: u32) {
    let span = count.saturating_sub(1).wrapping_mul(unit);
    let (lo, len) = match src_step {
        0 => (src, unit),
        s if s == unit => (src, count * unit),
        _ => (src.wrapping_sub(span), count * unit),
    };
    log_load(op::DMA, lo & !(unit - 1), len, dst, count * unit);
}

/// Hands every load logged since the last call to `f`, oldest first, and
/// forgets them. Returns how many were dropped for want of room.
pub fn take_loads(mut f: impl FnMut(&Load)) -> u32 {
    let len = LOG_LEN.load(Relaxed) as usize;
    let mut at = 0;
    let mut runs = [Run {
        tag: 0,
        offset: 0,
        len: 0,
    }; MAX_RUNS];
    while at < len {
        let w = |k: usize| LOG[at + k].load(Relaxed);
        let head = w(0);
        let n = (head >> 16) as usize;
        for (k, r) in runs[..n].iter_mut().enumerate() {
            *r = Run {
                tag: w(HEAD + 3 * k),
                offset: w(HEAD + 3 * k + 1),
                len: w(HEAD + 3 * k + 2),
            };
        }
        f(&Load {
            op: head & 0xFF,
            src: w(1),
            src_len: w(2),
            dst: w(3),
            dst_len: w(4),
            truncated: head >> 8 & 1 != 0,
            runs: &runs[..n],
        });
        at += HEAD + 3 * n;
    }
    LOG_LEN.store(0, Relaxed);
    LOG_DROPPED.swap(0, Relaxed)
}
