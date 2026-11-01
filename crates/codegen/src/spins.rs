//! The per-game list of spin loops the rule misses
//! ([`runtime::spins`], `crates/runtime/spins.toml`), for the code
//! generator: it marks their exits and carries them in the build's
//! `Code::spins`.

/// The listed spin edges of the ROM `rom` (by its SHA-1), sorted; empty
/// for a game not listed.
pub fn for_rom(rom: &[u8]) -> Vec<u32> {
    let mut v = Vec::new();
    runtime::spins::listed(&runtime::sha1::sha1(rom), |e| v.push(e));
    v.sort_unstable();
    v.dedup();
    v
}
