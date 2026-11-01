//! Order-sensitive digests of the bus accesses and ticks a run makes, for
//! the per-function differential runs (`docs/structured-codegen-design.md`,
//! "Validation"; `fn-diff` in `docs/tools/commands.md`). Two runs whose
//! digests match made the same accesses and ticks in the same order.
//!
//! The digests are always here; only the `trace` feature feeds them, so a
//! build without it pays nothing. A build that wants them (a generated
//! `fn-diff` crate) turns the feature on for its `runtime`.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// Whether this build feeds the digests.
pub const ENABLED: bool = cfg!(feature = "trace");

/// A `u64` as two 32-bit halves: the no_std target (`check-nostd`'s
/// riscv32) has no 64-bit atomics. Runs are single-threaded.
struct Word([AtomicU32; 2]);

impl Word {
    const fn new() -> Word {
        Word([AtomicU32::new(0), AtomicU32::new(0)])
    }

    #[inline(always)]
    fn get(&self) -> u64 {
        (self.0[1].load(Relaxed) as u64) << 32 | self.0[0].load(Relaxed) as u64
    }

    #[inline(always)]
    fn set(&self, v: u64) {
        self.0[0].store(v as u32, Relaxed);
        self.0[1].store((v >> 32) as u32, Relaxed);
    }
}

/// One stream of events: a running hash and a count.
pub struct Stream {
    hash: Word,
    count: Word,
}

impl Stream {
    const fn new() -> Stream {
        Stream {
            hash: Word::new(),
            count: Word::new(),
        }
    }

    /// Add an event.
    #[inline(always)]
    pub fn add(&self, x: u64) {
        self.hash.set(mix(self.hash.get() ^ x));
        self.count.set(self.count.get() + 1);
    }

    /// The digest so far: (hash, events).
    pub fn get(&self) -> (u64, u64) {
        (self.hash.get(), self.count.get())
    }

    pub fn reset(&self) {
        self.hash.set(0);
        self.count.set(0);
    }
}

/// Every data access through [`armv4t::interp::Bus`]: its kind, width,
/// address and value.
pub static BUS: Stream = Stream::new();
/// Every [`crate::Machine::tick`]: the cycles it was given and the clock
/// after it.
pub static TICKS: Stream = Stream::new();

/// A data access, for [`BUS`].
#[inline(always)]
pub fn access(store: bool, bytes: u32, addr: u32, v: u32) {
    BUS.add(((store as u64) << 63 | (bytes as u64) << 32 | addr as u64) ^ mix(v as u64));
}

/// SplitMix64's finaliser: each bit of `x` reaches every bit.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}
