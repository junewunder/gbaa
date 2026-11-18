//! A recompiled build running a ROM other than the one it was made from
//! (a randomized copy, a patched one, or one a script writes to): the
//! functions whose bytes the ROM changed, and their direct callers, run
//! interpreted ([`runtime::dispatch::stale`]); the rest stays compiled.
//!
//! Whole-ROM identity is the loaded ROM's own: save states and rewind
//! compare its CRC ([`runtime::Bus::rom_crc`]), netplay's build ID holds
//! it, and scripts' `emu:checksum` gives it.

use crate::Code;
use runtime::dispatch::Staleness;

/// What [`stale`] found in a ROM that differs from the build's.
#[derive(Clone, Copy, Debug)]
pub struct Changed {
    /// The stale functions' keys, sorted, for [`runtime::Bus::stale`].
    pub keys: &'static [u32],
    pub n: Staleness,
    /// How many functions the build has.
    pub fns: usize,
}

impl Changed {
    /// The line the desktop prints: how many functions went stale.
    pub fn report(&self) -> String {
        format!(
            "{} of {} compiled functions stale ({} changed, {} calling them), run interpreted",
            self.n.total(),
            self.fns,
            self.n.changed,
            self.n.callers
        )
    }
}

/// The functions of `code` that `rom` leaves stale: `None` for the ROM
/// the build was made from. A build without per-function hashes
/// ([`Code::fns`] empty) refuses any other ROM.
pub fn stale(rom: &[u8], code: &Code) -> Result<Option<Changed>, String> {
    if runtime::dispatch::rom_hash(rom) == code.rom_hash {
        return Ok(None);
    }
    if code.fns.is_empty() {
        return Err(
            "not the ROM this build was recompiled from, and the build has no \
             per-function hashes to run a changed one (recompile it)"
                .into(),
        );
    }
    let mut flags = vec![false; code.fns.len()];
    let n = code.mark_stale(rom, &mut flags);
    let mut keys: Vec<u32> = code.stale_keys(&flags).collect();
    keys.sort_unstable();
    keys.dedup();
    Ok(Some(Changed {
        keys: keys.leak(),
        n,
        fns: code.fns.len(),
    }))
}
