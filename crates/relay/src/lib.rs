//! The netplay server, `gbaa-server`: plain-UDP [`rooms`] with a short
//! code that seat the players and pass their datagrams on, knowing nothing
//! about the game. Build IDs are opaque bytes the server compares; frames,
//! inputs, states and hashes are the clients' business, inside the
//! payloads. See docs/research/multiplayer.md, "Relay server". The one
//! exception is the wireless adapter's [`radio`]: a room's radio is on the
//! server, so each player runs only their own console.

pub mod radio;
pub mod rooms;

/// Room code length; the alphabet leaves out look-alikes (0/O, 1/I).
pub const CODE_LEN: usize = 6;
pub const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// The server's port when an address leaves it out.
pub const PORT: u16 = 7374;
