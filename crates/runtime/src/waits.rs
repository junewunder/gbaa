//! Memory wait states (decision 11), as mGBA counts them.
//!
//! [`Waits`] holds each region's wait states for 16- and 32-bit accesses,
//! nonsequential and sequential, from `WAITCNT` (the cartridge's three ROM
//! windows and its save memory; EWRAM is fixed at 2). An access costs one
//! cycle plus its wait states. Code fetches cost their region's: generated
//! code and the interpreter count them per instruction from
//! [`armv4t::cycles::Cost`]. Data accesses the bus counts as the CPU makes
//! them ([`crate::bus::Bus::data_cycles`]), each at its region's
//! nonsequential count (an `ldm`/`stm`'s as mGBA counts a block,
//! [`crate::bus::Bus::block`]), and a DMA unit at its source's and
//! destination's.
//!
//! With the prefetch buffer on (`WAITCNT` bit 14), code run from ROM keeps
//! fetching while a data access elsewhere takes the bus: [`Waits::stall`]
//! is mGBA's `GBAMemoryStall`, which the bus applies after each memory
//! instruction or multiply run from ROM ([`crate::bus::Bus::stall`]).

/// `WAITCNT` at power-on.
pub const WAITCNT_RESET: u16 = 0;

const BASE16: [u8; 16] = [0, 0, 2, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 0];
const BASE32: [u8; 16] = [0, 0, 5, 0, 0, 1, 1, 0, 7, 7, 9, 9, 13, 13, 9, 0];
const BASE_SEQ16: [u8; 16] = [0, 0, 2, 0, 0, 0, 0, 0, 2, 2, 4, 4, 8, 8, 4, 0];
const BASE_SEQ32: [u8; 16] = [0, 0, 5, 0, 0, 1, 1, 0, 5, 5, 9, 9, 17, 17, 9, 0];
const ROM: [u8; 4] = [4, 3, 2, 8];
const ROM_SEQ: [u8; 6] = [2, 1, 4, 1, 8, 1];

/// The wait states of each region (an address's top byte, below `0x10`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waits {
    pub nonseq16: [u8; 16],
    pub seq16: [u8; 16],
    pub nonseq32: [u8; 16],
    pub seq32: [u8; 16],
    /// The prefetch buffer is on.
    pub prefetch: bool,
}

impl Default for Waits {
    fn default() -> Waits {
        let mut w = Waits {
            nonseq16: BASE16,
            seq16: BASE_SEQ16,
            nonseq32: BASE32,
            seq32: BASE_SEQ32,
            prefetch: false,
        };
        w.set_waitcnt(WAITCNT_RESET);
        w
    }
}

/// The region `addr` is in: its top byte, or 0 (no wait states) past
/// `0x0FFF_FFFF`.
#[inline(always)]
pub const fn region(addr: u32) -> usize {
    let r = (addr >> 24) as usize;
    if r < 16 { r } else { 0 }
}

impl Waits {
    /// Take a store to `WAITCNT` (mGBA's `GBAAdjustWaitstates`).
    pub fn set_waitcnt(&mut self, v: u16) {
        let sram = ROM[(v & 3) as usize];
        for r in [0xE, 0xF] {
            self.nonseq16[r] = sram;
            self.seq16[r] = sram;
            self.nonseq32[r] = 2 * sram + 1;
            self.seq32[r] = 2 * sram + 1;
        }
        for ws in 0..3 {
            let n = ROM[((v >> (2 + 3 * ws)) & 3) as usize];
            let s = ROM_SEQ[((v >> (4 + 3 * ws)) & 1) as usize + 2 * ws as usize];
            for r in [8 + 2 * ws as usize, 9 + 2 * ws as usize] {
                self.nonseq16[r] = n;
                self.seq16[r] = s;
                self.nonseq32[r] = n + 1 + s;
                self.seq32[r] = 2 * s + 1;
            }
        }
        self.prefetch = v & 0x4000 != 0;
    }

    /// The wait states of fetching `seq` sequential and `nonseq`
    /// nonsequential instructions (Thumb or ARM) from region `r`.
    #[inline(always)]
    pub fn fetch(&self, r: usize, thumb: bool, seq: u32, nonseq: u32) -> u32 {
        let (s, n) = if thumb {
            (self.seq16[r], self.nonseq16[r])
        } else {
            (self.seq32[r], self.nonseq32[r])
        };
        seq * s as u32 + nonseq * n as u32
    }

    /// The cycles of one nonsequential data access of `bytes` at `addr`.
    #[inline(always)]
    pub fn data(&self, bytes: u32, addr: u32) -> u32 {
        let r = region(addr);
        1 + if bytes == 4 {
            self.nonseq32[r]
        } else {
            self.nonseq16[r]
        } as u32
    }

    /// The cycles of a DMA of `count` units of `bytes` from `src` to `dst`:
    /// 2 a unit, plus both regions' wait states, nonsequential for the
    /// first unit and sequential after it (mGBA's `GBADMAService`).
    pub fn dma(&self, bytes: u32, src: u32, dst: u32, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        let (s, d) = (region(src), region(dst));
        let (n, q) = if bytes == 4 {
            (&self.nonseq32, &self.seq32)
        } else {
            (&self.nonseq16, &self.seq16)
        };
        let first = n[s] as u32 + n[d] as u32;
        let rest = q[s] as u32 + q[d] as u32;
        2 * count + first + (count - 1) * rest
    }

    /// mGBA's `GBAMemoryStall`: what a memory instruction's data accesses
    /// (`wait` cycles, its I cycle included) cost, run from ROM region
    /// `r` with the prefetch buffer on, at `pc` (the instruction's address
    /// plus two fetches). `last` is the address the buffer last fetched,
    /// which this moves on. The result is relative to the instruction's
    /// [`armv4t::cycles::Cost`], whose next fetch was nonsequential and
    /// which the buffer may make free; it can be negative.
    pub fn stall(&self, r: usize, pc: u32, last: &mut u32, wait: i32) -> i32 {
        let mut previous = 0;
        let mut max_loads = 8;
        let dist = last.wrapping_sub(pc);
        if dist < 16 {
            previous = (dist >> 1) as i32;
            max_loads -= previous;
        }
        let s = self.seq16[r] as i32;
        let n2s = self.nonseq16[r] as i32 - s + 1;
        let mut stall = s + 1;
        let mut loads = 1;
        while stall < wait && loads < max_loads {
            stall += s;
            loads += 1;
        }
        *last = pc.wrapping_add(2 * (loads + previous - 1) as u32);
        let wait = wait.max(stall);
        wait - n2s - (stall - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::Waits;

    #[test]
    fn waitcnt() {
        let mut w = Waits::default();
        // Power-on: ROM 4/2, SRAM 4.
        assert_eq!(
            (w.nonseq16[8], w.seq16[8], w.nonseq32[8], w.seq32[8]),
            (4, 2, 7, 5)
        );
        assert_eq!((w.nonseq16[0xE], w.nonseq32[0xE]), (4, 9));
        assert!(!w.prefetch);
        // Emerald's: SRAM 8, WS0 3/1, prefetch.
        w.set_waitcnt(0x4317);
        assert_eq!(
            (w.nonseq16[8], w.seq16[8], w.nonseq32[8], w.seq32[8]),
            (3, 1, 5, 3)
        );
        assert_eq!(w.nonseq16[0xE], 8);
        assert!(w.prefetch);
        assert_eq!(w.nonseq16[2], 2);
        assert_eq!(w.data(4, 0x0200_0000), 6);
        assert_eq!(w.data(2, 0x0300_0000), 1);
    }

    #[test]
    fn stall() {
        let mut w = Waits::default();
        w.set_waitcnt(0x4317);
        let mut last = 0;
        // An IWRAM load (1 + 1I) from Thumb ROM code: free beyond the
        // load's own fetch, as N turns S.
        assert_eq!(w.stall(8, 0x0800_0104, &mut last, 2), -2);
        assert_eq!(last, 0x0800_0104);
    }
}

crate::snap::snap!(Waits {
    nonseq16,
    seq16,
    nonseq32,
    seq32,
    prefetch
});
