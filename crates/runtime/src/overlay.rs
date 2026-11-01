//! Code a ROM copies into IWRAM, recompiled where it runs (decision 7).
//!
//! An **overlay** is one block of a recompiled guest function in IWRAM.
//! It is valid only while IWRAM holds the instruction bytes it was
//! compiled from (its [`Overlay::ranges`]: its instructions, not the
//! literal pools or data words beside them, which the code may write all
//! the time). Guarding blocks rather than whole functions keeps a function
//! that patches one of its own loops (Emerald Rogue's sound mixer)
//! compiled everywhere but that loop. A block may also have **versions**
//! compiled from bytes codegen saw it patched to ([`Overlay::image`]), each
//! an overlay of its own; whichever one IWRAM matches runs. Every IWRAM store goes through [`Overlays::wrote`], which tests
//! one bit per IWRAM word and, on a word some overlay was compiled from,
//! marks that overlay dirty. Entering an overlay ([`Overlays::valid`])
//! costs a flag test; only a dirty one compares its bytes with the ROM
//! copy they came from. A mismatch sends that key to the interpreter
//! fallback, which runs whatever IWRAM holds.

use crate::bus::IWRAM_SIZE;

/// One recompiled IWRAM block's guard.
#[derive(Debug)]
pub struct Overlay {
    /// The instruction bytes it was compiled from: (IWRAM address, length,
    /// where the same bytes are), sorted, disjoint. Where is a ROM address,
    /// or with an [`image`](Overlay::image) an offset into it.
    pub ranges: &'static [(u32, u32, u32)],
    /// A version's own bytes, not in the ROM: what codegen saw the code
    /// patched to.
    pub image: Option<&'static [u8]>,
}

/// The most overlays (blocks) one build may have.
pub const MAX_OVERLAYS: usize = 4096;

const WORDS: usize = IWRAM_SIZE / 4;

/// The overlays' state, kept by the bus: which IWRAM words they cover and
/// whether each still matches.
pub struct Overlays {
    table: &'static [Overlay],
    /// One bit per IWRAM word: set where some overlay's instructions are.
    watch: [u32; WORDS / 32],
    /// A word of the overlay was written since its last check.
    dirty: [bool; MAX_OVERLAYS],
    /// At its last check IWRAM held its bytes.
    valid: [bool; MAX_OVERLAYS],
    /// Checks that found a valid overlay's bytes changed.
    pub invalidations: u64,
    /// Guest cycles compiled overlay code ticked ([`crate::dispatch::tick_ram`]).
    pub cycles: u64,
}

impl Default for Overlays {
    fn default() -> Overlays {
        Overlays {
            table: &[],
            watch: [0; WORDS / 32],
            dirty: [false; MAX_OVERLAYS],
            valid: [false; MAX_OVERLAYS],
            invalidations: 0,
            cycles: 0,
        }
    }
}

/// The IWRAM offset of `a`.
fn offset(a: u32) -> usize {
    a as usize & (IWRAM_SIZE - 1)
}

impl Overlays {
    /// Guard `table` from now on (a no-op if it already is). Every overlay
    /// starts dirty, so its first entry checks its bytes.
    pub fn install(&mut self, table: &'static [Overlay]) {
        if core::ptr::eq(self.table, table) {
            return;
        }
        assert!(
            table.len() <= MAX_OVERLAYS,
            "more than {MAX_OVERLAYS} overlays"
        );
        self.table = table;
        self.watch = [0; WORDS / 32];
        for o in table {
            for &(a, n, _) in o.ranges {
                // A range running past the end of IWRAM never matches
                // (`check`), so its words there needn't be watched.
                let last = ((offset(a) + n as usize).max(1) - 1) / 4;
                for w in offset(a) / 4..=last.min(WORDS - 1) {
                    self.watch[w / 32] |= 1 << (w % 32);
                }
            }
        }
        self.dirty = [true; MAX_OVERLAYS];
        self.valid = [false; MAX_OVERLAYS];
    }

    pub fn table(&self) -> &'static [Overlay] {
        self.table
    }

    /// A store wrote the IWRAM word holding offset `o`.
    #[inline(always)]
    pub fn wrote(&mut self, o: usize) {
        let w = (o & (IWRAM_SIZE - 1)) / 4;
        if self.watch[w / 32] & (1 << (w % 32)) != 0 {
            self.hit(w);
        }
    }

    /// Mark every overlay covering word `w` dirty.
    #[inline(never)]
    fn hit(&mut self, w: usize) {
        for (i, o) in self.table.iter().enumerate() {
            if o.ranges.iter().any(|&(a, n, _)| {
                let a = offset(a);
                a / 4 <= w && w <= (a + n as usize - 1) / 4
            }) {
                self.dirty[i] = true;
            }
        }
    }

    /// Something rewrote IWRAM without going through the stores (the
    /// BIOS's `RegisterRamReset`): check every overlay again.
    pub fn touch_all(&mut self) {
        self.dirty = [true; MAX_OVERLAYS];
    }

    /// Whether overlay `i` may run: IWRAM holds the bytes it was compiled
    /// from. O(1) unless a store marked it dirty.
    #[inline(always)]
    pub fn valid(&mut self, i: usize, iwram: &[u8], rom: &[u8]) -> bool {
        if self.dirty[i] {
            self.check(i, iwram, rom);
        }
        self.valid[i]
    }

    #[inline(never)]
    fn check(&mut self, i: usize, iwram: &[u8], rom: &[u8]) {
        let ov = &self.table[i];
        let (bytes, mask) = match ov.image {
            Some(image) => (image, usize::MAX),
            None => (rom, crate::bus::ROM_MAX - 1),
        };
        let same = ov.ranges.iter().all(|&(a, n, src)| {
            let (o, r) = (offset(a), src as usize & mask);
            let n = n as usize;
            matches!((iwram.get(o..o + n), bytes.get(r..r + n)), (Some(x), Some(y)) if x == y)
        });
        if self.valid[i] && !same {
            self.invalidations += 1;
        }
        self.valid[i] = same;
        self.dirty[i] = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bus;

    /// A Thumb overlay of one instruction at 0x0300_0000, from ROM 0x0800_0000.
    static THUMB: [Overlay; 1] = [Overlay {
        ranges: &[(0x0300_0000, 2, 0x0800_0000)],
        image: None,
    }];

    #[test]
    fn a_store_beside_a_thumb_instruction_leaves_it_valid() {
        let rom: &'static [u8] = &[0x70, 0x47, 0, 0]; // bx lr
        let mut bus = Bus::new(rom);
        bus.write16(0x0300_0000, 0x4770);
        bus.overlays.install(&THUMB);
        assert!(bus.overlay_valid(0));
        // The other half of its word: dirty, checked, still valid.
        bus.write16(0x0300_0002, 0x1234);
        assert!(bus.overlays.dirty[0]);
        assert!(bus.overlay_valid(0));
        bus.write8(0x0300_0001, 0);
        assert!(!bus.overlay_valid(0));
        assert_eq!(bus.overlays.invalidations, 1);
    }

    #[test]
    fn a_version_matches_its_own_bytes_not_the_roms() {
        // `bx lr` as in the ROM, and a version patched to `movs r0, #0`.
        static TWO: [Overlay; 2] = [
            Overlay {
                ranges: &[(0x0300_0000, 2, 0x0800_0000)],
                image: None,
            },
            Overlay {
                ranges: &[(0x0300_0000, 2, 0)],
                image: Some(&[0x00, 0x20]),
            },
        ];
        let rom: &'static [u8] = &[0x70, 0x47, 0, 0];
        let mut bus = Bus::new(rom);
        bus.write16(0x0300_0000, 0x4770);
        bus.overlays.install(&TWO);
        assert!(bus.overlay_valid(0) && !bus.overlay_valid(1));
        bus.write16(0x0300_0000, 0x2000);
        assert!(!bus.overlay_valid(0) && bus.overlay_valid(1));
    }

    #[test]
    fn a_range_past_the_end_of_iwram_is_never_valid() {
        static PAST: [Overlay; 1] = [Overlay {
            ranges: &[(0x0300_7FF0, 0x20, 0x0800_0000)],
            image: None,
        }];
        let mut bus = Bus::new(&[0; 0x20]);
        bus.overlays.install(&PAST);
        assert!(!bus.overlay_valid(0));
    }
}
