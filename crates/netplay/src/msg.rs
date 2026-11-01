//! The session's messages, which ride inside the transport's payloads: a
//! tag byte, then the fields, little-endian; byte strings and lists are a
//! `u32` count first. The seat a message is from is the transport's to say.

/// One session message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// The sender's own seat's state as of `frame` (0, the start), with
    /// its build ID. Reliable.
    State {
        frame: u32,
        build: [u8; 32],
        data: Vec<u8>,
    },
    /// Seat 0's settings for everyone. Reliable.
    Start { delay: u8, clock: i64 },
    /// The sender's inputs for frames `first..`, [`crate::Input::SIZE`]
    /// bytes each, and how many frames of each seat's inputs it holds
    /// (`have[s]`), so seat `s` knows what to send it again. Unreliable.
    Input {
        first: u32,
        inputs: Vec<u8>,
        have: Vec<u32>,
    },
    /// A hash of every console after `frame` frames. Reliable.
    Hash { frame: u32, hash: u64 },
}

const STATE: u8 = 1;
const START: u8 = 2;
const INPUT: u8 = 3;
const HASH: u8 = 4;

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        match self {
            Msg::State { frame, build, data } => {
                o.push(STATE);
                o.extend_from_slice(&frame.to_le_bytes());
                o.extend_from_slice(build);
                bytes(&mut o, data);
            }
            Msg::Start { delay, clock } => {
                o.push(START);
                o.push(*delay);
                o.extend_from_slice(&clock.to_le_bytes());
            }
            Msg::Input {
                first,
                inputs,
                have,
            } => {
                o.push(INPUT);
                o.extend_from_slice(&first.to_le_bytes());
                bytes(&mut o, inputs);
                o.extend_from_slice(&(have.len() as u32).to_le_bytes());
                for h in have {
                    o.extend_from_slice(&h.to_le_bytes());
                }
            }
            Msg::Hash { frame, hash } => {
                o.push(HASH);
                o.extend_from_slice(&frame.to_le_bytes());
                o.extend_from_slice(&hash.to_le_bytes());
            }
        }
        o
    }

    /// `None` if `b` isn't a whole message.
    pub fn decode(b: &[u8]) -> Option<Msg> {
        let mut r = Reader(b);
        let m = match r.u8()? {
            STATE => Msg::State {
                frame: r.u32()?,
                build: r.take(32)?.try_into().ok()?,
                data: r.bytes()?.to_vec(),
            },
            START => Msg::Start {
                delay: r.u8()?,
                clock: i64::from_le_bytes(r.take(8)?.try_into().ok()?),
            },
            INPUT => Msg::Input {
                first: r.u32()?,
                inputs: r.bytes()?.to_vec(),
                have: {
                    let n = r.u32()? as usize;
                    // Each is 4 bytes: a count past what's left is damage.
                    if n > r.0.len() / 4 {
                        return None;
                    }
                    (0..n).map(|_| r.u32()).collect::<Option<_>>()?
                },
            },
            HASH => Msg::Hash {
                frame: r.u32()?,
                hash: u64::from_le_bytes(r.take(8)?.try_into().ok()?),
            },
            _ => return None,
        };
        r.0.is_empty().then_some(m)
    }
}

fn bytes(o: &mut Vec<u8>, b: &[u8]) {
    o.extend_from_slice(&(b.len() as u32).to_le_bytes());
    o.extend_from_slice(b);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}
