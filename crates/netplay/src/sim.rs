//! An in-memory network with a variable ping, for running a whole session
//! in one process (the owner's way to test netplay locally): every message
//! takes the latency plus a random jitter; unreliable ones are lost at the
//! loss rate and may overtake each other; reliable ones stay in order and
//! a "lost" one costs a resend, a round trip later. Time is simulated, in
//! microseconds, and the randomness is seeded, so a run repeats exactly.
//!
//! [`play`] runs peers' sessions on it, each on its own frame clock, as
//! front ends on separate machines would.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::rc::Rc;

use crate::{Console, Delivery, Error, Netplay, Status, To, Transport};

/// The network's behaviour, the same on every path.
#[derive(Clone, Copy, Debug)]
pub struct Conditions {
    /// One-way delay, µs.
    pub latency: u64,
    /// Up to this much more, uniformly, µs.
    pub jitter: u64,
    /// Chance a message is lost (unreliable) or must be resent (reliable).
    pub loss: f64,
}

impl Conditions {
    /// No delay at all.
    pub const LOCAL: Conditions = Conditions {
        latency: 0,
        jitter: 0,
        loss: 0.0,
    };
}

/// Counts over a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub sent: u64,
    pub lost: u64,
    pub resent: u64,
    pub bytes: u64,
}

type InFlight = Reverse<(u64, u64, usize, usize, Vec<u8>)>;

pub struct Net {
    cond: Conditions,
    now: u64,
    rng: u64,
    seq: u64,
    queue: BinaryHeap<InFlight>,
    inbox: Vec<VecDeque<(usize, Vec<u8>)>>,
    /// When the last reliable message from one seat to another arrives:
    /// the next can't arrive before it.
    last: Vec<Vec<u64>>,
    pub stats: Stats,
}

impl Net {
    pub fn new(seats: usize, cond: Conditions, seed: u64) -> Rc<RefCell<Net>> {
        Rc::new(RefCell::new(Net {
            cond,
            now: 0,
            rng: seed | 1,
            seq: 0,
            queue: BinaryHeap::new(),
            inbox: vec![VecDeque::new(); seats],
            last: vec![vec![0; seats]; seats],
            stats: Stats::default(),
        }))
    }

    /// Seat `seat`'s connection.
    pub fn endpoint(net: &Rc<RefCell<Net>>, seat: usize) -> Endpoint {
        Endpoint {
            net: net.clone(),
            seat,
        }
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    /// Move time to `t` and deliver what has arrived by then.
    pub fn advance_to(&mut self, t: u64) {
        self.now = self.now.max(t);
        while let Some(Reverse((at, _, _, _, _))) = self.queue.peek() {
            if *at > self.now {
                break;
            }
            let Reverse((_, _, from, to, payload)) = self.queue.pop().unwrap();
            self.inbox[to].push_back((from, payload));
        }
    }

    /// xorshift64*: a number in `0..1`.
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn delay(&mut self) -> u64 {
        self.cond.latency + (self.random() * (self.cond.jitter + 1) as f64) as u64
    }

    fn send(&mut self, from: usize, to: usize, how: Delivery, payload: Vec<u8>) {
        self.stats.sent += 1;
        self.stats.bytes += payload.len() as u64;
        let mut at = self.now + self.delay();
        match how {
            Delivery::Unreliable => {
                if self.random() < self.cond.loss {
                    self.stats.lost += 1;
                    return;
                }
            }
            Delivery::Reliable => {
                while self.random() < self.cond.loss {
                    self.stats.resent += 1;
                    at += 2 * self.cond.latency + self.delay();
                }
                at = at.max(self.last[from][to]);
                self.last[from][to] = at;
            }
        }
        self.seq += 1;
        self.queue.push(Reverse((at, self.seq, from, to, payload)));
    }
}

/// A seat's end of the [`Net`].
pub struct Endpoint {
    net: Rc<RefCell<Net>>,
    seat: usize,
}

impl Transport for Endpoint {
    fn send(&mut self, to: To, how: Delivery, payload: Vec<u8>) {
        let mut net = self.net.borrow_mut();
        match to {
            To::Seat(s) => net.send(self.seat, s, how, payload),
            To::Everyone => {
                for s in (0..net.inbox.len()).filter(|&s| s != self.seat) {
                    net.send(self.seat, s, how, payload.clone());
                }
            }
        }
    }

    fn recv(&mut self) -> Option<(usize, Vec<u8>)> {
        self.net.borrow_mut().inbox[self.seat].pop_front()
    }
}

/// How [`play`] went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Played {
    /// Simulated time, µs.
    pub time: u64,
    /// Turns each peer spent waiting or starting, all peers together.
    pub waits: u64,
    /// Frames rollback's peers held back for time sync, all together.
    pub skips: u64,
    pub net: Stats,
}

/// A frame at 60 Hz, µs.
pub const FRAME_US: u64 = 16_667;
/// How often a waiting peer tries again, µs.
pub const RETRY_US: u64 = 2_000;

/// Run `peers` (seat order) on `net` until each has run `frames` frames.
/// Each peer runs a frame every [`FRAME_US`] when it can, and tries again
/// every [`RETRY_US`] while it waits; `input(seat, frame)` is what the
/// seat's player holds when the peer is about to run `frame`.
/// `meddle(seat, session)` sees each peer after each frame it runs (to
/// break one on purpose). Gives up after a simulated hour.
pub fn play<P: Netplay<Net = Endpoint>>(
    peers: &mut [P],
    net: &Rc<RefCell<Net>>,
    frames: u32,
    mut input: impl FnMut(usize, u32) -> <P::Console as Console>::Input,
    mut meddle: impl FnMut(usize, &mut P),
) -> Result<Played, Error> {
    let mut next = vec![0u64; peers.len()];
    let mut played = Played::default();
    let mut t = 0;
    while peers.iter().any(|p| p.frame() < frames) {
        if t > 3_600_000_000 {
            return Err(Error::Console(format!(
                "stalled: frames {:?}",
                peers.iter().map(|p| p.frame()).collect::<Vec<_>>()
            )));
        }
        net.borrow_mut().advance_to(t);
        for (s, p) in peers.iter_mut().enumerate() {
            if p.frame() >= frames {
                // Done, but others may still need its inputs again.
                p.pump()?;
                continue;
            }
            if t < next[s] {
                continue;
            }
            let i = input(p.seat(), p.frame());
            match p.advance(i)? {
                Status::Stepped => {
                    next[s] = t + FRAME_US;
                    meddle(s, p);
                }
                Status::Skipped => {
                    played.skips += 1;
                    next[s] = t + FRAME_US;
                }
                Status::Waiting | Status::Starting => {
                    played.waits += 1;
                    next[s] = t + RETRY_US;
                }
            }
        }
        t = next
            .iter()
            .zip(peers.iter())
            .filter(|(_, p)| p.frame() < frames)
            .map(|(&n, _)| n)
            .min()
            .unwrap_or(t)
            .max(t + 1)
            .min(t + RETRY_US);
    }
    played.time = t;
    played.net = net.borrow().stats;
    Ok(played)
}
