//! Runtime support linked into generated code. `no_std`, so the same crate
//! serves the desktop and embedded front ends.
//!
//! [`Machine`] is the GBA: the [`armv4t`] CPU state, the [`Bus`] (memory
//! map and I/O registers), and a [`Platform`] front end. [`sched`] runs
//! time (scanlines, IRQs, halt) with [`timer`], [`dma`] and [`sound`]; the interpreter's
//! runner is on [`Machine`]. BIOS calls (#28, `bios*`) and the PPU (#29,
//! `ppu*`) build on the hooks in [`io`].

#![no_std]

pub mod bios;
pub mod bus;
pub mod debug;
pub mod dispatch;
pub mod dma;
pub mod gpio;
pub mod hw;
pub mod io;
pub mod link;
pub mod machine;
pub mod name;
pub mod overlay;
pub mod platform;
pub mod ppu;
#[cfg(feature = "provenance")]
pub mod prov;
pub mod save;
pub mod sched;
pub mod sha1;
pub mod snap;
pub mod sound;
pub mod spins;
pub mod state;
pub mod timer;
#[cfg(feature = "timing-lab")]
pub mod timing_lab;
pub mod trace;
pub mod video;
pub mod waits;

#[cfg(test)]
mod test_util;

pub use bus::Bus;
pub use machine::{Machine, Timing};
pub use platform::{AudioChunk, Frame, Platform};
pub use ppu::Ppu;
pub use video::Video;
