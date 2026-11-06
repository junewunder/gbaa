//! The code games copy into RAM themselves, found with a coverage run.
//!
//! Discovery folds a BIOS `CpuSet`/`CpuFastSet` out of ROM with constant
//! arguments into a copy ([`Discovery::copies`]), and
//! [`crate::discovery::overlays`] discovers one at its destination from the
//! constant RAM addresses the ROM's code reaches. Games copy code other
//! ways too: by DMA (pret's `IntrMain`), by their own loops, by a `CpuSet`
//! whose arguments come from a caller; and a copy discovery did fold is
//! often entered only through a pointer it can't follow (m4a's
//! `SoundMainRAM`, called through `SoundInfo`). [`ram_copies`] takes the
//! run as the evidence:
//!
//! 1. each BIOS copy into RAM gets, besides the ROM's constant entries
//!    into it (IWRAM only, as [`crate::discovery::overlays`]), the run's
//!    call targets and IRQ handlers inside it as roots;
//! 2. RAM code the run executed outside those copies whose bytes are found
//!    in the ROM (at the same alignment) is a copy of that ROM, the place
//!    that agrees with the most of it ([`sources`]); its image runs
//!    [`TAIL`] bytes past the last executed instruction (for its literal
//!    pools and the end of its last function);
//! 3. in both, where the run executed code (that agrees with the ROM
//!    source) that discovery from those roots doesn't reach, the lowest
//!    such address becomes a root too, until none is left ([`RamCopy::filled`]).
//!
//! Code that changed while it ran and the flash library's stack routines
//! ([`super::flash_calls`]) are left out: the first is a hole of its own,
//! the second the hardware library's. Code whose bytes aren't in the ROM
//! (built at run time) is found by neither and stays undiscovered.

use std::collections::{BTreeMap, BTreeSet};

use crate::discovery::{Discovery, Overlay, discover_at};
use crate::program::ROM_BASE;

use super::{Run, Seen, flash_calls, in_flash, is_ram, rom_read};

/// Bytes past a found copy's last executed instruction its image takes.
pub const TAIL: u32 = 0x100;

/// The fewest bytes from an instruction on (contiguous) that must match
/// the ROM for a copy to start there.
const SEED: usize = 8;

/// The most bytes from a copy's first instruction looked for in the ROM.
const MATCH: usize = 64;

/// Code at one offset from its source with gaps this long, or shorter,
/// can be one copy.
const JOIN: u32 = 0x400;

/// The most roots [`ram_copies`] adds to one copy where the run went
/// further than discovery.
const FILL_MAX: usize = 64;

/// One copy of ROM code into RAM and what discovery finds in it.
#[derive(Debug)]
pub struct RamCopy {
    pub overlay: Overlay,
    /// Found from the run's bytes, not a BIOS copy discovery folded.
    pub seen: bool,
    /// Roots where code enters it (interworking form): the ROM's constant
    /// RAM entries and the run's call targets and IRQ handlers.
    pub entered: Vec<u32>,
    /// Roots added where the run executed code the others don't reach.
    pub filled: Vec<u32>,
}

/// The RAM copies of `rom`'s code (discovered as `disc`) that the ROM's
/// entries or `run` show running, each discovered at its destination. A
/// copy whose destination overlaps an earlier one's is left out: one
/// address, one overlay.
pub fn ram_copies(rom: &[u8], disc: &Discovery, run: &Run) -> Vec<RamCopy> {
    let flash = flash_calls(run);
    // What a copy can hold: RAM code that ran unchanged, outside the flash
    // routines.
    let exec: BTreeMap<u32, Seen> = run
        .code
        .range(0x0200_0000..0x0400_0000)
        .filter(|(a, s)| !s.changed && !in_flash(&flash, **a))
        .map(|(a, s)| (*a, *s))
        .collect();
    let entries: BTreeSet<u32> = run
        .call_targets
        .iter()
        .chain(&run.irq_handlers)
        .copied()
        .filter(|a| is_ram(a & !1) && !in_flash(&flash, a & !1))
        .collect();

    let mut out: Vec<RamCopy> = Vec::new();
    let overlaps = |out: &[RamCopy], dst: u32, len: u32| {
        out.iter()
            .any(|c| dst < c.overlay.dst + c.overlay.len && c.overlay.dst < dst + len)
    };
    for &(src, dst, len) in &disc.copies {
        if !is_ram(dst) || !is_ram(dst + len - 1) || overlaps(&out, dst, len) {
            continue;
        }
        let iwram = (0x0300_0000..0x0300_8000).contains(&dst) && dst + len <= 0x0300_8000;
        let mut roots: Vec<u32> = Vec::new();
        if iwram {
            roots.extend(
                disc.ram_entries
                    .iter()
                    .filter(|&&k| (dst..dst + len).contains(&(k & !1))),
            );
        }
        if let Some(c) = build(rom, src, dst, len, roots, &entries, &exec, false) {
            out.push(c);
        }
    }

    // The RAM code outside those copies, and the copies it is of.
    let ins: Vec<(u32, Seen)> = exec
        .iter()
        .filter(|(a, _)| {
            !out.iter()
                .any(|c| (c.overlay.dst..c.overlay.dst + c.overlay.len).contains(a))
        })
        .map(|(a, s)| (*a, *s))
        .collect();
    let groups = sources(rom, &ins);
    for i in 0..groups.len() {
        let (dst, end, src) = groups[i];
        let region_end = if dst >> 24 == 0x02 {
            0x0204_0000
        } else {
            0x0300_8000
        };
        let mut stop = (end + TAIL)
            .min(region_end)
            .min(ROM_BASE + rom.len() as u32 - src + dst);
        if let Some(n) = groups.get(i + 1) {
            stop = stop.min(n.0);
        }
        for c in &out {
            // A BIOS copy past this one's start ends it, even one the
            // gaps it bridges run over.
            if c.overlay.dst > dst {
                stop = stop.min(c.overlay.dst);
            }
        }
        if let Some(c) = build(rom, src, dst, stop - dst, Vec::new(), &entries, &exec, true) {
            out.push(c);
        }
    }
    out.sort_by_key(|c| c.overlay.dst);
    out
}

/// Discover `rom`'s `len` bytes at `src` at `dst` from `roots`, the run's
/// `entries` inside, and roots added where `exec` went further. None if
/// it has no root at all.
#[allow(clippy::too_many_arguments)]
fn build(
    rom: &[u8],
    src: u32,
    dst: u32,
    len: u32,
    mut roots: Vec<u32>,
    entries: &BTreeSet<u32>,
    exec: &BTreeMap<u32, Seen>,
    seen: bool,
) -> Option<RamCopy> {
    let o = src.checked_sub(ROM_BASE)? as usize;
    let image = rom.get(o..o + len as usize)?.to_vec();
    let end = dst + len;
    // The run's code here that is the copy's: its bytes are the ROM's.
    let ours = |a: u32, s: &Seen| rom_read(rom, src + (a - dst), s.thumb) == Some(s.raw);
    for &k in entries.range(dst..end) {
        let a = k & !1;
        if exec
            .get(&a)
            .is_some_and(|s| ours(a, s) && s.thumb == (k & 1 != 0))
        {
            roots.push(k);
        }
    }
    roots.sort_unstable();
    roots.dedup();
    let entered = roots.clone();
    // The lowest address the run executed here that `d` doesn't reach and
    // isn't a root already.
    let next = |d: Option<&Discovery>, roots: &[u32]| {
        exec.range(dst..end)
            .map(|(a, s)| (*a, *a | s.thumb as u32, s))
            .find(|&(a, k, s)| {
                let covered = d.is_some_and(|d| {
                    d.code
                        .range(..=a)
                        .next_back()
                        .is_some_and(|(&c, i)| a < c + i.size)
                });
                ours(a, s) && !covered && !roots.contains(&k)
            })
            .map(|(_, k, _)| k)
    };
    let mut filled = Vec::new();
    if roots.is_empty() {
        let k = next(None, &roots)?;
        roots.push(k);
        filled.push(k);
    }
    let disc = loop {
        let d = discover_at(dst, image.clone(), &roots);
        match next(Some(&d), &roots) {
            Some(k) if filled.len() < FILL_MAX => {
                roots.push(k);
                filled.push(k);
            }
            _ => break d,
        }
    };
    Some(RamCopy {
        overlay: Overlay {
            src,
            dst,
            len,
            disc,
        },
        seen,
        entered,
        filled,
    })
}

/// The copies `ins` (RAM code, sorted) is of: (dst, end, src) for each run
/// of it at one offset from the ROM. At each instruction not yet placed,
/// of the places in the ROM holding its first bytes (up to [`MATCH`]
/// contiguous, at least [`SEED`]), the one that agrees with the most
/// instructions from there ([`agree`]) is its source, the lowest of a tie.
/// A copy ends where that place stops agreeing, or at a stretch (code
/// after a gap) another place agrees with further: the start of another
/// copy whose first bytes happen to match.
fn sources(rom: &[u8], ins: &[(u32, Seen)]) -> Vec<(u32, u32, u32)> {
    let mut groups: Vec<(u32, u32, u32)> = Vec::new();
    let mut i = 0;
    while i < ins.len() {
        let Some((src, mut end)) = best(rom, ins, i) else {
            i += 1;
            continue;
        };
        for k in i + 1..end {
            if ins[k].0 != next(&ins[k - 1]) && best(rom, ins, k).is_some_and(|(_, e)| e > end) {
                end = k;
                break;
            }
        }
        let (dst, last) = (ins[i].0, next(&ins[end - 1]));
        match groups.last_mut() {
            Some(g)
                if g.2.wrapping_sub(g.0) == src.wrapping_sub(dst)
                    && dst.saturating_sub(g.1) <= JOIN =>
            {
                g.1 = last
            }
            _ => groups.push((dst, last, src)),
        }
        i = end;
    }
    groups
}

/// The address after instruction `i`.
fn next(i: &(u32, Seen)) -> u32 {
    i.0 + if i.1.thumb { 2 } else { 4 }
}

/// The source of `ins[k]` onward and the index its agreement ends at: of
/// the places in the ROM at `ins[k]`'s alignment (mod 4) holding its first
/// bytes, the one [`agree`] goes furthest with, the lowest of a tie. None
/// if its stretch has under [`SEED`] bytes.
fn best(rom: &[u8], ins: &[(u32, Seen)], k: usize) -> Option<(u32, usize)> {
    let mut bytes: Vec<u8> = Vec::new();
    for j in k..ins.len() {
        if j > k && ins[j].0 != next(&ins[j - 1]) {
            break;
        }
        let size = if ins[j].1.thumb { 2 } else { 4 };
        bytes.extend_from_slice(&ins[j].1.raw.to_le_bytes()[..size]);
        if bytes.len() >= MATCH {
            break;
        }
    }
    if bytes.len() < SEED {
        return None;
    }
    let first = u16::from_le_bytes([bytes[0], bytes[1]]);
    let start = ins[k].0;
    let mut out: Option<(u32, usize)> = None;
    for o in ((start & 3) as usize..(rom.len() + 1).saturating_sub(bytes.len())).step_by(4) {
        if u16::from_le_bytes([rom[o], rom[o + 1]]) != first || rom[o..o + bytes.len()] != bytes[..]
        {
            continue;
        }
        let src = ROM_BASE + o as u32;
        let end = agree(rom, ins, k, src);
        if out.is_none_or(|b| end > b.1) {
            out = Some((src, end));
        }
    }
    out
}

/// How far `ins` from `k` is a copy of the ROM at `src`: the index of the
/// first instruction whose bytes differ, or that is more than [`JOIN`]
/// bytes past the one before.
fn agree(rom: &[u8], ins: &[(u32, Seen)], k: usize, src: u32) -> usize {
    let dst = ins[k].0;
    let mut j = k;
    while j < ins.len()
        && (j == k || ins[j].0.saturating_sub(next(&ins[j - 1])) <= JOIN)
        && rom_read(rom, src + (ins[j].0 - dst), ins[j].1.thumb) == Some(ins[j].1.raw)
    {
        j += 1;
    }
    j
}
