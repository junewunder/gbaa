//! What a session's peers share (`docs/research/multiplayer.md`, "A
//! session"): seat 0's settings, the errors, what a turn did, and how
//! often the consoles are hashed. The session itself is [`crate::Rollback`].
//!
//! **Start.** Every peer holds the whole [`crate::Console`] but has only
//! its own seat's state; it sends that ([`crate::msg::Msg::State`] at
//! frame 0) with its build ID, and seat 0 sends the settings
//! ([`crate::msg::Msg::Start`]: input delay, clock). Once a peer has every
//! state and the settings it loads them all, so every peer starts from the
//! same consoles at frame 0.

/// Frames between hash checks.
pub const HASH_EVERY: u32 = 60;

/// Seat 0's settings for the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Input delay, in frames.
    pub delay: u8,
    /// The consoles' clock at the start, Unix seconds (the host's).
    pub clock: i64,
}

impl Default for Config {
    /// No input delay (june, 2026-10-27: a press reaches your own console
    /// on the next frame) and the runtime's fixed headless clock.
    fn default() -> Config {
        Config {
            delay: 0,
            clock: runtime::gpio::FIXED_CLOCK,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Seat `seat` runs another build: the session can't stay in sync.
    Build { seat: usize },
    /// Seat `seat` sent a message that doesn't decode, or one not its own
    /// to send.
    Bad { seat: usize },
    /// The console failed: a fault, or a state that didn't load.
    Console(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Build { seat } => write!(f, "seat {seat} runs another build"),
            Error::Bad { seat } => write!(f, "seat {seat} sent a bad message"),
            Error::Console(e) => write!(f, "{e}"),
        }
    }
}

/// What [`crate::Netplay::advance`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not every seat's state or the settings yet.
    Starting,
    /// Ran a frame.
    Stepped,
    /// Waits on another seat: at the window's edge, or its state.
    Waiting,
    /// Held a frame back, this peer running ahead of
    /// another. Pace it as a frame (show the last one again).
    Skipped,
}
