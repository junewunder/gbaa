//! IWRAM code no static copy explains, found by running it (decision 7).
//! pret's games copy code to IWRAM in ways discovery can't follow: `IntrMain`
//! by DMA, agb_flash's `ReadFlash_Core` by a CPU loop onto the stack. Codegen
//! runs the ROM in the interpreter (the same run length as
//! [`crate::variants`]) and, the first time code runs at an IWRAM address
//! outside every known copy, looks for the bytes there in the ROM. A unique
//! match, widened while IWRAM and the ROM agree, is a copy like a static
//! one: discovered at its destination from the addresses seen entering it,
//! compiled as overlays, guarded by its bytes. Code with no ROM source
//! (built at run time) stays interpreted, and so does code run in the live
//! stack (at or above `sp`): `ReadFlash_Core` lands wherever the caller's
//! frame is, so the addresses one run sees needn't be the next one's, and
//! it only reads the save once at boot.

use crate::func::key;
use analysis::discovery::{Overlay, discover_at};
use analysis::program::ROM_BASE;
use runtime::Machine;
use runtime::platform::Headless;
use std::collections::HashSet;

const IWRAM: std::ops::Range<u32> = 0x0300_0000..0x0300_8000;

/// Bytes at the entry a place in the ROM must hold to be its source.
const SEED: usize = 8;

/// The fewest bytes a copy found this way may have.
const MIN_LEN: u32 = 12;

/// A copy seen running: its destination, length, ROM source, and the keys
/// it was entered at from outside it, in the order seen.
struct Found {
    dst: u32,
    len: u32,
    src: u32,
    roots: Vec<u32>,
}

/// Run `rom` for `frames` frames and give the IWRAM copies its code runs
/// in that none of `known` covers, each discovered at its destination.
pub fn overlays(rom: &[u8], known: &[Overlay], frames: u32) -> Vec<Overlay> {
    record(rom, known, frames)
        .into_iter()
        .filter_map(|f| {
            let o = (f.src - ROM_BASE) as usize;
            let image = rom[o..o + f.len as usize].to_vec();
            // Later entries are roots only where discovery from the earlier
            // ones didn't reach (a return from a call isn't an entry).
            let mut roots: Vec<u32> = Vec::new();
            let mut disc = None;
            for k in f.roots {
                let reached = disc
                    .as_ref()
                    .is_some_and(|d: &analysis::discovery::Discovery| {
                        d.code.contains_key(&(k & !1))
                    });
                if !reached {
                    roots.push(k);
                    disc = Some(discover_at(f.dst, image.clone(), &roots));
                }
            }
            Some(Overlay {
                src: f.src,
                dst: f.dst,
                len: f.len,
                disc: disc?,
            })
        })
        .collect()
}

fn record(rom: &[u8], known: &[Overlay], frames: u32) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    if frames == 0 {
        return found;
    }
    // Where a copy may lie: IWRAM less the known copies, as sorted ranges.
    let mut taken: Vec<(u32, u32)> = known.iter().map(|o| (o.dst, o.dst + o.len)).collect();
    let mut tried: HashSet<u32> = HashSet::new();
    let rom: &'static [u8] = Box::leak(rom.to_vec().into_boxed_slice());
    let mut m = Box::new(Machine::new(rom, Headless));
    let mut prev = 0u32;
    for _ in 0..frames {
        m.sched.frame_ended = false;
        while !m.sched.frame_ended && m.fault.is_none() {
            if m.sched.halted {
                m.idle();
                continue;
            }
            m.take_irq();
            let pc = m.cpu.pc;
            if IWRAM.contains(&pc) {
                let k = key(pc, m.cpu.thumb());
                let inside = |f: &Found, a: u32| (f.dst..f.dst + f.len).contains(&a);
                if let Some(f) = found.iter_mut().find(|f| inside(f, pc)) {
                    if !inside(f, prev) && !f.roots.contains(&k) {
                        f.roots.push(k);
                    }
                } else if !taken.iter().any(|&(a, b)| (a..b).contains(&pc))
                    && tried.insert(pc)
                    // Code in the live stack moves with the call depth.
                    && pc < m.cpu.r[13]
                    && let Some((dst, len, src)) = locate(&m, rom, pc, &taken)
                {
                    taken.push((dst, dst + len));
                    found.push(Found {
                        dst,
                        len,
                        src,
                        roots: vec![k],
                    });
                }
            }
            prev = pc;
            m.step();
        }
        if m.fault.is_some() {
            break;
        }
    }
    found
}

/// The copy holding `pc`: of the places in the ROM (at `pc`'s alignment)
/// whose first [`SEED`] bytes are IWRAM's at `pc`, the one that agrees
/// with IWRAM longest, widened both ways and stopping at IWRAM's ends and
/// at `taken`; none if two agree as long, or it's under [`MIN_LEN`].
/// (dst, len, src).
fn locate(
    m: &Machine<Headless>,
    rom: &[u8],
    pc: u32,
    taken: &[(u32, u32)],
) -> Option<(u32, u32, u32)> {
    let lo = taken
        .iter()
        .map(|r| r.1)
        .filter(|&b| b <= pc)
        .fold(IWRAM.start, u32::max);
    let hi = taken
        .iter()
        .map(|r| r.0)
        .filter(|&a| a > pc)
        .fold(IWRAM.end, u32::min);
    if pc + SEED as u32 > hi {
        return None;
    }
    let seed: Vec<u8> = (pc..pc + SEED as u32).map(|a| m.bus.read8(a)).collect();
    let align = if pc & 3 == 0 { 4 } else { 2 };
    let extent = |at: usize| {
        let (mut d0, mut s0) = (pc, at);
        while d0 > lo && s0 > 0 && m.bus.read8(d0 - 1) == rom[s0 - 1] {
            d0 -= 1;
            s0 -= 1;
        }
        let (mut d1, mut s1) = (pc + SEED as u32, at + SEED);
        while d1 < hi && s1 < rom.len() && m.bus.read8(d1) == rom[s1] {
            d1 += 1;
            s1 += 1;
        }
        (d0, d1 - d0, ROM_BASE + s0 as u32)
    };
    let mut best: Option<(u32, u32, u32)> = None;
    let mut tie = false;
    for o in (0..rom.len().saturating_sub(SEED)).step_by(2) {
        if o % align != pc as usize % align || rom[o..o + SEED] != seed[..] {
            continue;
        }
        let e = extent(o);
        match best {
            Some(b) if e.1 == b.1 => tie = true,
            Some(b) if e.1 < b.1 => {}
            _ => (best, tie) = (Some(e), false),
        }
    }
    best.filter(|b| !tie && b.1 >= MIN_LEN)
}
