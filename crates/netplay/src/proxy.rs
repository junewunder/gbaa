//! The variable-ping simulator (multiplayer/ping-sim, the owner's way to
//! test netplay locally): a UDP proxy between two local peers that holds
//! each datagram for a delay that changes over time, as a profile in TOML
//! says (`crates/netplay/profiles/`), and loses, reorders and duplicates
//! some.
//!
//! Each side has its own socket: peer A sends to [`Proxy::addrs`]`[0]`,
//! peer B to `[1]`, and each side learns its peer's address from what it
//! receives, so the peers may start in any order (a datagram for a side
//! not heard from yet is dropped, as a network would).
//!
//! A datagram's one-way delay, at `t` seconds since the proxy started:
//!
//! ```text
//! latency + wander * sin(2π t / wander_period) + spike (while one lasts)
//!         + a uniform 0..jitter
//! ```
//!
//! Datagrams in one direction keep their order, as a real path mostly
//! does (a late one holds back those behind it), except the `reorder`
//! share, held `reorder_ms` longer without holding anything back, so later
//! ones overtake it. Spikes come at random, `spike_every_s` apart on
//! average, on both directions at once. The randomness is seeded; the
//! timing is the machine's, so two runs differ in detail.
//!
//! The model itself is [`Line`], on a clock of µs from its start, which
//! the feel sim ([`crate::feel`]) runs on simulated time.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Deserialize;

/// How the path between the peers behaves. Every field may be left out
/// (zero, or none).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    /// One-way delay, ms.
    pub latency_ms: f64,
    /// Up to this much more per datagram, uniformly, ms.
    pub jitter_ms: f64,
    /// The latency drifts by up to this either way, ms, as a sine of
    /// period `wander_period_s`.
    pub wander_ms: f64,
    pub wander_period_s: f64,
    /// Mean time between spikes, s (0: no spikes).
    pub spike_every_s: f64,
    /// A spike's extra delay, ms, and how long it lasts, ms.
    pub spike_ms: f64,
    pub spike_len_ms: f64,
    /// Chance a datagram is lost.
    pub loss: f64,
    /// Chance a datagram is held `reorder_ms` more, letting later ones
    /// overtake it.
    pub reorder: f64,
    pub reorder_ms: f64,
    /// Chance a datagram arrives twice.
    pub duplicate: f64,
    /// The randomness's seed.
    pub seed: u64,
}

impl Profile {
    pub fn load(path: &Path) -> Result<Profile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Profile::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Profile, String> {
        let p: Profile = toml::from_str(text).map_err(|e| e.to_string())?;
        let chances = [p.loss, p.reorder, p.duplicate];
        if chances.iter().any(|c| !(0.0..=1.0).contains(c)) {
            return Err("loss, reorder and duplicate are chances, 0 to 1".into());
        }
        let times = [
            p.latency_ms,
            p.jitter_ms,
            p.wander_ms,
            p.wander_period_s,
            p.spike_every_s,
            p.spike_ms,
            p.spike_len_ms,
            p.reorder_ms,
        ];
        if times.iter().any(|t| !t.is_finite() || *t < 0.0) {
            return Err("times can't be negative".into());
        }
        if p.wander_ms > 0.0 && p.wander_period_s == 0.0 {
            return Err("wander_ms needs wander_period_s".into());
        }
        Ok(p)
    }
}

/// Counts over the proxy's run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Datagrams received from either side.
    pub received: u64,
    pub forwarded: u64,
    pub lost: u64,
    pub reordered: u64,
    pub duplicated: u64,
    /// For a side not heard from yet.
    pub unaddressed: u64,
    /// Spikes begun.
    pub spikes: u64,
    /// The least and most one-way delay given, µs.
    pub min_delay_us: u64,
    pub max_delay_us: u64,
}

/// The proxy, running on its own thread until stopped or dropped.
pub struct Proxy {
    addrs: [SocketAddr; 2],
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Stats>>,
}

impl Proxy {
    /// Listen on `a` (peer A's side) and `b` (peer B's); port 0 picks one.
    pub fn start(profile: Profile, a: SocketAddr, b: SocketAddr) -> std::io::Result<Proxy> {
        Proxy::with_peers(profile, [a, b], [None, None])
    }

    /// Listen on `a` for peer A and pass its datagrams on to `b`, a fixed
    /// address that needn't speak first (a server), from a port of its
    /// own: one client's path to the server.
    pub fn to(profile: Profile, a: SocketAddr, b: SocketAddr) -> std::io::Result<Proxy> {
        let any: SocketAddr = if b.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }
            .parse()
            .unwrap();
        Proxy::with_peers(profile, [a, any], [None, Some(b)])
    }

    fn with_peers(
        profile: Profile,
        bind: [SocketAddr; 2],
        peer: [Option<SocketAddr>; 2],
    ) -> std::io::Result<Proxy> {
        let socks = [UdpSocket::bind(bind[0])?, UdpSocket::bind(bind[1])?];
        for s in &socks {
            s.set_nonblocking(true)?;
        }
        let addrs = [socks[0].local_addr()?, socks[1].local_addr()?];
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut w = Wire::new(profile, socks);
            w.peer = peer;
            w.run(&flag)
        });
        Ok(Proxy {
            addrs,
            stop,
            thread: Some(thread),
        })
    }

    /// Where peer A sends, and where peer B sends.
    pub fn addrs(&self) -> [SocketAddr; 2] {
        self.addrs
    }

    /// Stop, and say what happened.
    pub fn stop(mut self) -> Stats {
        self.finish()
    }

    fn finish(&mut self) -> Stats {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .take()
            .map(|t| t.join().expect("proxy thread"))
            .unwrap_or_default()
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.finish();
    }
}

/// The profile's path, in time counted in µs from its start, whatever
/// clock that is: the proxy's real one, or the feel sim's simulated one
/// ([`crate::feel`]). Both directions share its spikes.
pub struct Line {
    p: Profile,
    rng: u64,
    /// When the last in-order datagram each way is due.
    last: [u64; 2],
    /// The current or next spike's start and end.
    spike: Option<(u64, u64)>,
    pub stats: Stats,
}

impl Line {
    pub fn new(p: Profile) -> Line {
        let seed = p.seed;
        let mut l = Line {
            p,
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            last: [0; 2],
            spike: None,
            stats: Stats {
                min_delay_us: u64::MAX,
                ..Stats::default()
            },
        };
        l.spike = l.next_spike(0);
        l
    }

    /// xorshift64*: a number in `0..1`.
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// The first spike after `after`: exponentially distributed gaps.
    fn next_spike(&mut self, after: u64) -> Option<(u64, u64)> {
        if self.p.spike_every_s <= 0.0 || self.p.spike_ms <= 0.0 {
            return None;
        }
        let gap = -self.p.spike_every_s * (1.0 - self.random()).ln();
        let from = after + (gap * 1e6) as u64;
        Some((from, from + (self.p.spike_len_ms * 1e3) as u64))
    }

    /// Whether a spike lasts at `now`.
    pub fn spiking(&self, now: u64) -> bool {
        self.spike.is_some_and(|(f, t)| now >= f && now < t)
    }

    /// The one-way delay for a datagram arriving at `now`, ms.
    fn delay_ms(&mut self, now: u64) -> f64 {
        let t = now as f64 / 1e6;
        let mut d = self.p.latency_ms;
        if self.p.wander_ms > 0.0 {
            d += self.p.wander_ms * (std::f64::consts::TAU * t / self.p.wander_period_s).sin();
        }
        while let Some((from, to)) = self.spike {
            if now >= to {
                self.spike = self.next_spike(to);
                continue;
            }
            if now >= from {
                d += self.p.spike_ms;
            }
            break;
        }
        d + self.random() * self.p.jitter_ms
    }

    /// A datagram sent at `now` toward side `out`: when each copy of it
    /// arrives there (none, lost; two, duplicated).
    pub fn route(&mut self, now: u64, out: usize) -> Vec<u64> {
        self.stats.received += 1;
        if self.random() < self.p.loss {
            self.stats.lost += 1;
            return Vec::new();
        }
        let copies = if self.random() < self.p.duplicate {
            self.stats.duplicated += 1;
            2
        } else {
            1
        };
        let mut due_at = Vec::with_capacity(copies);
        for _ in 0..copies {
            let ms = self.delay_ms(now).max(0.0);
            let mut due = now + (ms * 1e3) as u64;
            if self.random() < self.p.reorder {
                self.stats.reordered += 1;
                due += (self.p.reorder_ms * 1e3) as u64;
            } else {
                due = due.max(self.last[out]);
                self.last[out] = due;
            }
            let us = due - now;
            self.stats.min_delay_us = self.stats.min_delay_us.min(us);
            self.stats.max_delay_us = self.stats.max_delay_us.max(us);
            due_at.push(due);
        }
        due_at
    }

    /// The counts so far, a delay never given as 0.
    pub fn finish(&self) -> Stats {
        let mut s = self.stats;
        if s.min_delay_us == u64::MAX {
            s.min_delay_us = 0;
        }
        s
    }
}

/// (when it's due, order of arrival, side it goes out of, datagram)
type Held = Reverse<(Instant, u64, usize, Vec<u8>)>;

/// The proxy's state, on its thread.
struct Wire {
    line: Line,
    socks: [UdpSocket; 2],
    peer: [Option<SocketAddr>; 2],
    start: Instant,
    seq: u64,
    held: BinaryHeap<Held>,
    unaddressed: u64,
    spikes: u64,
}

impl Wire {
    fn new(p: Profile, socks: [UdpSocket; 2]) -> Wire {
        Wire {
            line: Line::new(p),
            socks,
            peer: [None; 2],
            start: Instant::now(),
            seq: 0,
            held: BinaryHeap::new(),
            unaddressed: 0,
            spikes: 0,
        }
    }

    fn us(&self, t: Instant) -> u64 {
        t.duration_since(self.start).as_micros() as u64
    }

    /// A datagram from side `side`, for the other.
    fn arrive(&mut self, side: usize, d: Vec<u8>, now: Instant) {
        let out = 1 - side;
        let at = self.us(now);
        for due in self.line.route(at, out) {
            self.seq += 1;
            let due = now + Duration::from_micros(due - at);
            self.held.push(Reverse((due, self.seq, out, d.clone())));
        }
    }

    fn run(mut self, stop: &AtomicBool) -> Stats {
        let mut buf = vec![0u8; 65536];
        let mut spiking = false;
        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            let mut busy = false;
            for side in 0..2 {
                loop {
                    match self.socks[side].recv_from(&mut buf) {
                        Ok((n, from)) => {
                            self.peer[side] = Some(from);
                            self.arrive(side, buf[..n].to_vec(), now);
                            busy = true;
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        // A datagram to a peer that has gone bounced.
                        Err(_) => break,
                    }
                }
            }
            let in_spike = self.line.spiking(self.us(now));
            if in_spike && !spiking {
                self.spikes += 1;
            }
            spiking = in_spike;
            let now = Instant::now();
            while let Some(Reverse((due, ..))) = self.held.peek() {
                if *due > now {
                    break;
                }
                let Reverse((_, _, out, d)) = self.held.pop().unwrap();
                match self.peer[out] {
                    Some(addr) => {
                        self.line.stats.forwarded += 1;
                        let _ = self.socks[out].send_to(&d, addr);
                    }
                    None => self.unaddressed += 1,
                }
            }
            if !busy {
                let wait = self
                    .held
                    .peek()
                    .map_or(Duration::from_micros(500), |Reverse((due, ..))| {
                        due.saturating_duration_since(now)
                    })
                    .min(Duration::from_micros(500));
                std::thread::sleep(wait);
            }
        }
        Stats {
            unaddressed: self.unaddressed,
            spikes: self.spikes,
            ..self.line.finish()
        }
    }
}
