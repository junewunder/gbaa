//! What the tests and `gbaa-pingsim` play: a toy console, a tiny ROM that
//! reads the keys, a scripted player, and the same game run straight,
//! without a network, to compare a session's hashes with.

use crate::gba::Gba;
use crate::session::HASH_EVERY;
use crate::{Console, Fnv, Snapshot};

/// A console whose seats are linked: each seat's state mixes in its
/// neighbour's every frame, so a wrong input or state anywhere spreads.
#[derive(Clone)]
pub struct Toy {
    pub st: Vec<u64>,
    pub build: u8,
}

impl Toy {
    /// The true start: seat `s` at `1000 + s`.
    pub fn new(seats: usize) -> Toy {
        Toy {
            st: (0..seats as u64).map(|s| 1000 + s).collect(),
            build: 1,
        }
    }

    /// What seat `seat`'s peer holds before the start: its own seat right,
    /// the others garbage.
    pub fn peer(seats: usize, seat: usize) -> Toy {
        let mut t = Toy::new(seats);
        for (s, v) in t.st.iter_mut().enumerate() {
            if s != seat {
                *v = 0xBAD0 + s as u64;
            }
        }
        t
    }
}

impl Console for Toy {
    type Input = u16;

    fn seats(&self) -> usize {
        self.st.len()
    }

    fn step(&mut self, inputs: &[u16]) -> Result<(), String> {
        let prev = self.st.clone();
        let n = prev.len();
        for (s, v) in self.st.iter_mut().enumerate() {
            let x = prev[s]
                ^ (inputs[s] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ prev[(s + 1) % n].rotate_left(7);
            *v = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD) ^ x >> 29;
        }
        Ok(())
    }

    fn save(&mut self, seat: usize, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.st[seat].to_le_bytes());
    }

    fn load(&mut self, states: &[Vec<u8>]) -> Result<(), String> {
        for (v, s) in self.st.iter_mut().zip(states) {
            *v = u64::from_le_bytes(s[..].try_into().map_err(|_| "toy: bad state")?);
        }
        Ok(())
    }

    fn hash(&mut self) -> u64 {
        let mut h = Fnv::default();
        for v in &self.st {
            h.bytes(&v.to_le_bytes());
        }
        h.0
    }

    fn build_id(&self) -> [u8; 32] {
        [self.build; 32]
    }
}

impl Snapshot for Toy {
    type Snap = Vec<u64>;

    fn snapshot(&mut self, into: &mut Vec<u64>) {
        into.clone_from(&self.st);
    }

    fn restore(&mut self, from: &Vec<u64>) -> Result<(), String> {
        self.st.clone_from(from);
        Ok(())
    }

    fn snap_hash(&self, s: &Vec<u64>) -> u64 {
        let mut h = Fnv::default();
        for v in s {
            h.bytes(&v.to_le_bytes());
        }
        h.0
    }
}

/// A player's presses: changing every few frames, different per seat.
pub fn script(seat: usize, frame: u32) -> u16 {
    let x = (seat as u32 * 7919 + frame / 5).wrapping_mul(2_654_435_761);
    (x >> 16) as u16 & 0x3FF
}

/// The same game without a network: frame `f` runs on what each player
/// held `delay` frames earlier. Its hash every [`HASH_EVERY`] frames and
/// after the last.
pub fn straight<C: Console<Input = u16>>(mut c: C, delay: u32, frames: u32) -> Vec<(u32, u64)> {
    let mut hs = Vec::new();
    for f in 0..=frames {
        if f.is_multiple_of(HASH_EVERY) || f == frames {
            hs.push((f, c.hash()));
        }
        if f == frames {
            break;
        }
        let inputs: Vec<u16> = (0..c.seats())
            .map(|s| if f < delay { 0 } else { script(s, f - delay) })
            .collect();
        c.step(&inputs).unwrap();
    }
    hs
}

/// A cartridge that folds `KEYINPUT` into a word of EWRAM as fast as it
/// can, so every frame's state depends on every frame's keys:
///
/// ```text
///     mov r0, #0x04000000 ; add r0, r0, #0x130   @ KEYINPUT
///     mov r1, #0x02000000
/// 1:  ldrh r2, [r0] ; ldr r3, [r1]
///     add r3, r2, r3, ror #7 ; str r3, [r1]
///     b 1b
/// ```
pub fn key_folder() -> &'static [u8] {
    let code: [u32; 8] = [
        0xE3A0_0301,
        0xE280_0E13,
        0xE3A0_1402,
        0xE1D0_20B0,
        0xE591_3000,
        0xE082_33E3,
        0xE581_3000,
        0xEAFF_FFFA,
    ];
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    Box::leak(bytes.into_boxed_slice())
}

/// GBAs on `rom`, seat `s`'s save 64 bytes of `s + 1`: seat `seat`'s
/// own console, its own save given and the others' blank (every save with
/// `None`).
pub fn gba(rom: &'static [u8], seats: usize, seat: Option<usize>) -> Gba {
    let saves: Vec<Vec<u8>> = (0..seats).map(|s| vec![s as u8 + 1; 64]).collect();
    let pick: Vec<Option<&[u8]>> = (0..seats)
        .map(|s| (seat.is_none_or(|own| own == s)).then_some(&saves[s][..]))
        .collect();
    Gba::new(rom, &pick)
}
