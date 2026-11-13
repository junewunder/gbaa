//! The flash save library's stack routines (Nintendo's `AgbFlash`, pret's
//! `agb_flash.c`), recognised by the code a game copies onto the stack.
//!
//! To read the flash chip the library can't run from ROM (the cartridge
//! bus is busy with the chip), so it copies three tiny routines into a
//! buffer in its caller's stack frame and calls them there: `ReadFlash1`
//! (one byte, for the chip's ID and status), `ReadFlash_Core` (a sector's
//! bytes into RAM) and `VerifyFlashSector_Core` (RAM against a sector).
//! The buffer lands wherever the caller's frame is, and the next copy
//! puts another routine at the same addresses, so a native build sees
//! code in RAM that changes (`docs/research/native-holes.md`). Each is
//! one call of the hardware library instead (`runtime::hw::flash`, the
//! [`Routine`]'s [`Routine::call`]): a call into RAM whose target holds a
//! recognised routine's bytes is that call, and the bytes are never run.
//!
//! The routines are recognised by their whole code, as compiled: agbcc's
//! (the SDK's own object, in Emerald, both Kirbys and Metroid: Zero
//! Mission) and modern GCC's (pret's `modern` build, Emerald Rogue). Only
//! each code's length and FNV-1a hash are kept here, not the bytes (they
//! are the ROMs'). A compiler's version not listed shows in the hole
//! report as before, RAM code that changed; listing it is a line in
//! [`SIGNATURES`], kept in `runtime::hw::flash` so a lifted program
//! recognises the same routines at run time.

pub use runtime::hw::flash::{
    MAX_LEN, Routine, SIGNATURES, Signature, fnv1a, recognise, recognise_in,
};

#[cfg(test)]
mod tests;
