//! The idle back edges of a program (docs/native-port-design.md, section
//! 3): the store-free, call-free busy loops where `--timing native` and
//! a native program let time pass. The test is
//! [`armv4t::idle::is_idle_edge`], over the bytes, which the interpreter
//! also runs at each backward branch, so a list made here and the run
//! time agree; the code generator and the native lifter take it from here.

use crate::discovery::Discovery;
use armv4t::decode::{Instr, decode_arm, decode_thumb};
pub use armv4t::idle::{MAX_LEN, MAX_SPIN_LEN, is_idle_edge, is_spin_edge};

/// The address of every idle back edge in `disc`'s code, ascending.
/// `image` is the ROM (or a copy's bytes), loaded at `base`.
pub fn edges(disc: &Discovery, image: &[u8], base: u32) -> Vec<u32> {
    let fetch = |a: u32| word(image, base, a);
    let mut out = Vec::new();
    for (&at, c) in &disc.code {
        let raw = fetch(at);
        let i = if c.thumb {
            decode_thumb(raw as u16, at)
        } else {
            decode_arm(raw, at)
        };
        if let Instr::Branch {
            link: false,
            target,
            ..
        } = i
            && is_idle_edge(fetch, at, target, c.thumb)
        {
            out.push(at);
        }
    }
    out
}

/// The address of every spin loop's back edge in `disc`'s code
/// ([`is_spin_edge`], docs/research/busy-wait-loops.md), with `extra`
/// (the game's list, `runtime::spins`, backward branches only) merged in,
/// ascending: where the native lifter prints `hw::spin`.
pub fn spin_edges(disc: &Discovery, image: &[u8], base: u32, extra: &[u32]) -> Vec<u32> {
    let fetch = |a: u32| word(image, base, a);
    let mut out: Vec<u32> = disc
        .code
        .iter()
        .filter_map(|(&at, c)| {
            let raw = fetch(at);
            let i = if c.thumb {
                decode_thumb(raw as u16, at)
            } else {
                decode_arm(raw, at)
            };
            let Instr::Branch {
                link: false,
                target,
                ..
            } = i
            else {
                return None;
            };
            let listed =
                target <= at && extra.contains(&at) && !is_idle_edge(fetch, at, target, c.thumb);
            (listed || is_spin_edge(fetch, at, target, c.thumb)).then_some(at)
        })
        .collect();
    out.sort_unstable();
    out
}

/// Whether the instruction at `at` in `disc`'s code is a spin loop's back
/// edge, found or listed in `extra`: what the hole report checks each
/// busy-wait loop it saw against (one that isn't wants listing).
pub fn is_spin_at(disc: &Discovery, image: &[u8], base: u32, at: u32, extra: &[u32]) -> bool {
    let Some(c) = disc.code.get(&at) else {
        return false;
    };
    let fetch = |a: u32| word(image, base, a);
    let raw = fetch(at);
    let i = if c.thumb {
        decode_thumb(raw as u16, at)
    } else {
        decode_arm(raw, at)
    };
    matches!(i, Instr::Branch { link: false, target, .. }
        if target <= at && (extra.contains(&at) || is_spin_edge(fetch, at, target, c.thumb)))
}

/// The little-endian word at guest address `a` of `image` (zero past its
/// end): a Thumb fetch uses its low half.
fn word(image: &[u8], base: u32, a: u32) -> u32 {
    let o = a.wrapping_sub(base) as usize;
    let b = |i: usize| image.get(o + i).copied().unwrap_or(0) as u32;
    b(0) | b(1) << 8 | b(2) << 16 | b(3) << 24
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_poll_and_not_the_copy() {
        // 0x0800_0000: ldrh r1, [r0]; cmp r1, #0; beq 0x0800_0000
        // 0x0800_0006: strh r1, [r0]; subs r2, #1; bne 0x0800_0006
        let halves: [u16; 6] = [0x8801, 0x2900, 0xD0FC, 0x8001, 0x3A01, 0xD1FC];
        let image: Vec<u8> = halves.iter().flat_map(|h| h.to_le_bytes()).collect();
        let mut disc = Discovery::default();
        for i in 0..6 {
            disc.code.insert(
                0x0800_0000 + 2 * i,
                crate::discovery::Code {
                    size: 2,
                    thumb: true,
                    evidence: crate::discovery::Evidence::Confirmed,
                },
            );
        }
        assert_eq!(edges(&disc, &image, 0x0800_0000), vec![0x0800_0004]);
        assert!(spin_edges(&disc, &image, 0x0800_0000, &[]).is_empty());
        // Listed, the copy loop counts as a spin.
        assert_eq!(
            spin_edges(&disc, &image, 0x0800_0000, &[0x0800_000a]),
            vec![0x0800_000a]
        );
    }
}
