//! Asset extraction (docs/asset-extraction-design.md): the exporter reads a
//! ROM and what a recording run logged, and writes files and a manifest. No
//! emulation here, so an export and its checks rerun in seconds.
//!
//! - [`m4a`]: the sound driver's structures, the song table scan and the
//!   bytecode decoder.
//! - [`midi`]: a standard MIDI file writer and reader.
//! - [`songs`]: songs to format-1 MIDI the way pret's mid2agb reads them.
//! - [`mid2agb`]: which bars mid2agb makes patterns of, for [`songs`].
//! - [`mplay`]: an assembler for mid2agb's `.s` output, for the check.
//! - [`pret`]: `check-pret` for songs and samples.
//! - [`graphics`]: tile sheets, palettes and tilemaps from the logs.
//! - [`export`]: the graphics export of every route under a folder, for
//!   the CLI and the desktop's extract-while-playing.
//! - [`pretgfx`]: `check-pret` for graphics.
//! - [`samples`]: voicegroups, samples to WAV and `.pcm`, soundfonts.
//! - [`sf2`]: a SoundFont 2 writer.
//! - [`sprites`]: sprite sheets and animations from OAM, to Aseprite.
//! - [`spritetables`]: the Sonic Advance engine's sprite tables (Kirby),
//!   whole frames and per-frame hitboxes read from the ROM.
//! - [`overrides`]: a mod's edited files back to their blobs' bytes, for
//!   the runtime to serve (asset overrides).
//! - [`coverage`]: pret's used graphics files as the denominator, and a
//!   fuzz or route run scored against them.
//! - [`scan`]: every LZ77 blob in a ROM found statically, and the ones no
//!   run reached as greyscale sheets.
//! - [`loadlog`]: `check-load-log`, a recording run's load and use logs
//!   against what it showed.

pub mod coverage;
pub mod export;
pub mod graphics;
pub mod loadlog;
pub mod m4a;
pub mod mid2agb;
pub mod midi;
pub mod mplay;
pub mod overrides;
pub mod pret;
pub mod pretgfx;
pub mod samples;
pub mod scan;
pub mod sf2;
pub mod songs;
pub mod sprites;
pub mod spritetables;

#[cfg(test)]
mod tests;
