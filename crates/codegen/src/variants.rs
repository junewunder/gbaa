//! Versions of IWRAM blocks the code patches (decision 7). A copied
//! function may rewrite its own instructions: Emerald Rogue's sound mixer
//! stamps one of three templates into its unrolled loop for each channel,
//! so the loop never holds the bytes it was copied with. Codegen runs the
//! ROM in the interpreter for [`FRAMES`] frames with no input, notes each
//! overlay block's bytes whenever it starts running with them changed, and
//! builds the block again from each set of bytes seen. A version is kept
//! only if its control flow is the block's (same extent, same exits), so
//! only what its instructions compute differs; at run time whichever
//! version IWRAM matches runs, and bytes never seen go to the fallback as
//! before.

use crate::emit::{RamCode, guard_ranges};
use crate::func::{Ctx, GBlock, Variant, build_func, exits};
use analysis::program::ROM_BASE;
use runtime::Machine;
use runtime::platform::Headless;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Frames `gbaa-recompile` runs a ROM for to see the blocks it patches.
pub const FRAMES: u32 = 1800;

/// A block of a copy: (copy, function index, key).
type At = (usize, usize, u32);

/// A block start's block, its guard ranges and the ROM's bytes for them.
type Watched = (At, Vec<(u32, u32, u32)>, Vec<u8>);

/// Run `rom` for `frames` frames and give the functions of `ram` the
/// versions of their blocks it patched them to.
pub fn add(rom: &[u8], ram: &mut [RamCode], frames: u32) {
    for (at, seen) in record(rom, ram, frames) {
        let (ci, fi, k) = at;
        let rc = &ram[ci];
        let o = (rc.copy.src - ROM_BASE) as usize;
        let image = &rom[o..o + rc.copy.len as usize];
        let f = &rc.code.funcs[fi];
        let ranges = guard_ranges(&rc.copy, f, k);
        let orig = Ctx::at(rc.copy.dst, image, &rc.copy.disc);
        let mut kept = Vec::new();
        for bytes in seen {
            let mut patched = image.to_vec();
            let mut i = 0;
            for &(a, n, _) in &ranges {
                let (d, n) = ((a - rc.copy.dst) as usize, n as usize);
                patched[d..d + n].copy_from_slice(&bytes[i..i + n]);
                i += n;
            }
            let ctx = Ctx::at(rc.copy.dst, &patched, &rc.copy.disc);
            let g = build_func(&ctx, f.entry);
            let same_blocks = g.blocks.keys().eq(f.blocks.keys());
            if same_blocks && shape(&ctx, &g.blocks[&k]) == shape(&orig, &f.blocks[&k]) {
                let block = g.blocks[&k].clone();
                kept.push(Variant { block, bytes });
            }
        }
        if !kept.is_empty() {
            ram[ci].code.funcs[fi].variants.insert(k, kept);
        }
    }
}

/// What a version of a block must keep: its end, its instruction count and
/// its exits.
fn shape(ctx: &Ctx, gb: &GBlock) -> (u32, usize, Vec<(u32, usize, crate::func::Plan)>) {
    let xs = exits(ctx, gb).into_iter().map(|x| (x.at, x.insns, x.plan));
    (gb.end, gb.insns.len(), xs.collect())
}

/// The bytes of each block of `ram` (its guard ranges, in order) that it
/// started running with, other than the ROM's, over `frames` frames.
fn record(rom: &[u8], ram: &[RamCode], frames: u32) -> BTreeMap<At, BTreeSet<Vec<u8>>> {
    let mut seen: BTreeMap<At, BTreeSet<Vec<u8>>> = BTreeMap::new();
    if ram.is_empty() || frames == 0 {
        return seen;
    }
    // Each block start's address: the block, its guard and the ROM's bytes.
    let mut blocks: HashMap<u32, Watched> = HashMap::new();
    for (ci, rc) in ram.iter().enumerate() {
        for (fi, f) in rc.code.funcs.iter().enumerate() {
            for &k in f.blocks.keys() {
                blocks.entry(k & !1).or_insert_with(|| {
                    let ranges = guard_ranges(&rc.copy, f, k);
                    let ours = ranges.iter().flat_map(|&(_, n, s)| {
                        let o = (s - ROM_BASE) as usize;
                        rom[o..o + n as usize].iter().copied()
                    });
                    ((ci, fi, k), ranges.clone(), ours.collect())
                });
            }
        }
    }
    let rom: &'static [u8] = Box::leak(rom.to_vec().into_boxed_slice());
    let mut m = Box::new(Machine::new(rom, Headless));
    let mut now = Vec::new();
    for _ in 0..frames {
        m.sched.frame_ended = false;
        while !m.sched.frame_ended && m.fault.is_none() {
            if m.sched.halted {
                m.idle();
                continue;
            }
            m.take_irq();
            let pc = m.cpu.pc;
            if let Some((at, ranges, ours)) = (pc >> 24 == 3).then(|| blocks.get(&pc)).flatten() {
                now.clear();
                for &(a, n, _) in ranges {
                    now.extend((a..a + n).map(|a| m.bus.read8(a)));
                }
                if now != *ours {
                    seen.entry(*at).or_default().insert(now.clone());
                }
            }
            m.step();
        }
        if m.fault.is_some() {
            break;
        }
    }
    seen
}
