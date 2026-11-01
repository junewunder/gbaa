//! The netplay session core (`docs/research/multiplayer.md`, "Recommended:
//! the layers"): every peer runs every console in the session, and only
//! inputs cross the network, so whatever links the consoles (the GBA's
//! cable) stays exact inside each peer's process.
//!
//! - [`Console`]: what the core needs of a console (all of a session's
//!   seats, linked): step a frame on every seat's input, save one seat's
//!   state, load every seat's, hash, and a build ID every peer must share.
//!   [`Snapshot`]: one rollback can take back.
//! - [`Transport`]: messages to the other seats, reliable or not. [`udp`]
//!   carries them for real; [`sim`] is an in-memory network with latency,
//!   jitter and loss for tests.
//! - [`Rollback`]: Slippi-style rollback, the only session: run on guessed
//!   inputs, go back and run again when a guess was wrong; the start from
//!   every seat's own state, hash checks. [`Netplay`] is what a front end
//!   drives. [`session`]: what its peers share (settings, errors).
//! - [`gba`]: the GBA's adapter, the link hub's machines as a [`Console`].
//!   The only module that knows the console.
//! - [`udp`]: a [`Transport`] over UDP, for peers in separate processes,
//!   directly or through `gbaa-server`'s rooms; [`proxy`]: the
//!   variable-ping simulator between two of them, from a TOML profile
//!   (`gbaa-pingsim`). [`demo`]: what the tests play.

pub mod demo;
pub mod gba;
pub mod msg;
pub mod proxy;
pub mod rollback;
pub mod session;
pub mod sim;
pub mod udp;

pub use rollback::Rollback;
pub use session::{Config, Error, Status};

/// One seat's input for one frame: a button bitmask or a pad's whole
/// reading, fixed-size on the wire.
pub trait Input: Copy + Eq + Default + core::fmt::Debug {
    /// Bytes on the wire.
    const SIZE: usize;
    fn write(&self, out: &mut Vec<u8>);
    /// From exactly [`Input::SIZE`] bytes.
    fn read(b: &[u8]) -> Self;
}

macro_rules! int_input {
    ($($t:ty),*) => {$(
        impl Input for $t {
            const SIZE: usize = size_of::<$t>();
            fn write(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn read(b: &[u8]) -> Self {
                <$t>::from_le_bytes(b.try_into().unwrap())
            }
        }
    )*};
}
int_input!(u8, u16, u32, u64);

/// A session's consoles, one a seat, linked however the console links
/// them. Every peer holds one and steps it identically.
pub trait Console {
    type Input: Input;
    fn seats(&self) -> usize;
    /// Run one frame, every seat's input given.
    fn step(&mut self, inputs: &[Self::Input]) -> Result<(), String>;
    /// Seat `seat`'s console state, between frames: what its owner sends at
    /// the start.
    fn save(&mut self, seat: usize, out: &mut Vec<u8>);
    /// Replace every seat's state with `states` (one a seat) and restart
    /// whatever links them, between frames. Every peer does it at the same
    /// frame with the same states, so it stays deterministic.
    fn load(&mut self, states: &[Vec<u8>]) -> Result<(), String>;
    /// Set the wall clock the consoles read (Unix seconds) at the session's
    /// start; from there the console advances it by emulated time, never
    /// from the peer's own clock.
    fn set_clock(&mut self, _unix: i64) {}
    /// A hash of every seat's state, for desync checks.
    fn hash(&mut self) -> u64;
    /// What every peer must agree on: the console, the build, the games.
    fn build_id(&self) -> [u8; 32];
}

/// A console rollback can take back: a snapshot of every seat in memory,
/// cheap enough to take every frame, never leaving this peer.
pub trait Snapshot: Console {
    type Snap: Default;
    /// The consoles as they are, between frames, into `into` (whose
    /// buffers are reused).
    fn snapshot(&mut self, into: &mut Self::Snap);
    fn restore(&mut self, from: &Self::Snap) -> Result<(), String>;
    /// What [`Console::hash`] gives on the consoles `s` holds.
    fn snap_hash(&self, s: &Self::Snap) -> u64;
    /// While on, the frames being run again: nothing a player hears or
    /// sees should come of them. Never changes what the consoles compute.
    fn quiet(&mut self, _on: bool) {}
}

/// A session as a front end drives it: a [`Rollback`], or a stand-in for
/// one.
pub trait Netplay {
    type Console: Console;
    type Net: Transport;
    /// One turn of the front end's frame loop, `local` being what this
    /// seat's player holds now.
    fn advance(&mut self, local: <Self::Console as Console>::Input) -> Result<Status, Error>;
    /// Take in what has arrived and answer, without running.
    fn pump(&mut self) -> Result<(), Error>;
    /// Frames run.
    fn frame(&self) -> u32;
    fn seat(&self) -> usize;
    fn console(&self) -> &Self::Console;
    fn console_mut(&mut self) -> &mut Self::Console;
    fn transport(&self) -> &Self::Net;
    /// The hashes this peer took, (frame, hash).
    fn hashes(&self) -> &[(u32, u64)];
    /// The frames whose hashes differed between peers.
    fn desyncs(&self) -> &[u32];
    /// How many frames this peer runs ahead of the others (behind, if
    /// negative), if the session keeps track.
    fn lead(&self) -> Option<f32> {
        None
    }
    /// The session's own counts, for a front end's end line.
    fn summary(&self) -> String {
        String::new()
    }
}

/// Who a message is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    Seat(usize),
    Everyone,
}

/// How a message must arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Whole and in order ([`udp`]'s numbered, acknowledged chunks).
    Reliable,
    /// Maybe late, out of order or never (one datagram): inputs only,
    /// each repeated until the others have it.
    Unreliable,
}

/// The session's connection to the other seats.
pub trait Transport {
    fn send(&mut self, to: To, how: Delivery, payload: Vec<u8>);
    /// The next message that has arrived, with the seat it came from.
    fn recv(&mut self) -> Option<(usize, Vec<u8>)>;
}

/// FNV-1a, 64-bit: a hash every peer computes alike, for [`Console::hash`].
#[derive(Clone, Copy, Debug)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv(0xCBF2_9CE4_8422_2325)
    }
}

impl Fnv {
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ x as u64).wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
}

#[cfg(test)]
mod tests;
