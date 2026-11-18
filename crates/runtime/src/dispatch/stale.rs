//! Which compiled functions a changed ROM leaves stale
//! (`docs/research/randomized-roms.md`, option a): each compiled function
//! records the ROM bytes it was compiled from ([`FnBytes`]): its
//! instructions, the `bx rN` stubs it calls through, and every ROM load
//! const-prop baked into it. A ROM whose bytes there differ makes it
//! stale, and with it every function that calls it directly, transitively
//! (a direct call is a Rust call, which the dispatcher never sees:
//! decision 8). [`Bus::stale`](crate::bus::Bus::stale) then holds their
//! keys, and the dispatcher treats them as keys with no compiled code: the
//! interpreter runs them. On the exact ROM nothing is stale and the
//! dispatcher pays one empty-slice check per lookup.
//!
//! No allocation: the caller lends the per-function flags
//! ([`Code::mark_stale`]) and keeps the sorted keys
//! ([`Code::stale_keys`]) where it likes.

use super::Code;
use crate::platform::Platform;

/// The ROM bytes one compiled function was made from.
#[derive(Clone, Copy, Debug)]
pub struct FnBytes {
    /// Its entry key.
    pub entry: u32,
    /// [`fn_hash`] of `ranges` in the ROM it was compiled from.
    pub hash: u64,
    /// The ROM byte ranges `[start, end)` it depends on, at `0x08`
    /// addresses, sorted and merged: its instructions, the stubs it calls
    /// through and its baked loads.
    pub ranges: &'static [(u32, u32)],
    /// The indices in [`Code::fns`] of the functions it calls directly.
    pub calls: &'static [u32],
    /// Its block starts, sorted: every key of [`Code::blocks`],
    /// [`Code::inner`] or [`Code::ram_blocks`] that runs its code.
    pub keys: &'static [u32],
}

/// How many functions [`Code::mark_stale`] found stale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Staleness {
    /// Whose own bytes differ.
    pub changed: usize,
    /// Unchanged, but calling a stale one directly, transitively.
    pub callers: usize,
}

impl Staleness {
    pub fn total(&self) -> usize {
        self.changed + self.callers
    }
}

/// FNV-1a, 64-bit ([`super::rom_hash`]'s), over the bytes of `ranges` in
/// `rom` (mapped at `0x0800_0000`), in order; `None` if one runs past the
/// ROM's end.
pub fn fn_hash(rom: &[u8], ranges: &[(u32, u32)]) -> Option<u64> {
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    for &(a, b) in ranges {
        let (a, b) = ((a & 0x01FF_FFFF) as usize, (b - a) as usize);
        for &x in rom.get(a..a.checked_add(b)?)? {
            h = (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    Some(h)
}

impl<P: Platform> Code<P> {
    /// Marks in `stale` (one flag per [`fns`](Code::fns)) the functions
    /// `rom` leaves stale: those whose bytes differ, then every function
    /// that calls a stale one directly, until none is added.
    pub fn mark_stale(&self, rom: &[u8], stale: &mut [bool]) -> Staleness {
        assert_eq!(stale.len(), self.fns.len(), "one flag per function");
        let mut n = Staleness::default();
        for (s, f) in stale.iter_mut().zip(self.fns) {
            *s = fn_hash(rom, f.ranges) != Some(f.hash);
            n.changed += *s as usize;
        }
        loop {
            let before = n.callers;
            for (i, f) in self.fns.iter().enumerate() {
                if !stale[i] && f.calls.iter().any(|&c| stale[c as usize]) {
                    stale[i] = true;
                    n.callers += 1;
                }
            }
            if n.callers == before {
                return n;
            }
        }
    }

    /// The keys of the functions `stale` marks, unsorted, a key shared by
    /// two of them twice. Sorted and deduplicated, they are
    /// [`Bus::stale`](crate::bus::Bus::stale).
    pub fn stale_keys<'a>(&'a self, stale: &'a [bool]) -> impl Iterator<Item = u32> + 'a {
        self.fns
            .iter()
            .zip(stale)
            .filter(|(_, s)| **s)
            .flat_map(|(f, _)| f.keys.iter().copied())
    }
}
