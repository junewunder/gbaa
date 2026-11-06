//! A [`Transport`] over UDP, for peers in separate processes (the ping
//! simulator's, `gbaa-pingsim`): unreliable messages are one datagram each;
//! reliable ones are cut into numbered chunks, each resent until the seat
//! it's for acknowledges it, and put back together in order there.
//!
//! A datagram is a kind byte, the sender's seat, then:
//!
//! - unreliable: the payload;
//! - reliable: the chunk's number (`u32`, counting per destination), 1 if
//!   it's a message's last chunk, the chunk;
//! - an ack: the number of the first chunk the sender still lacks, so
//!   every one before it has arrived, then a `u64` whose bit `i` says
//!   chunk `first + 1 + i` has arrived too (a selective ack), so a resend
//!   is only of what's missing;
//! - a goodbye: nothing more; the sender has left ([`Udp::leave`]).
//!
//! A seat has left when it says goodbye (or the rooms server says it
//! has), or when nothing has come from it for a while ([`Udp::gone`]): a
//! player who quits says so, one whose machine stopped (a crash, a lost
//! connection, `SIGSTOP`) goes quiet.
//!
//! A chunk unacknowledged after the resend timeout is sent again and the
//! timeout doubles (to [`RTO_MAX`]), as TCP's does; it comes back down
//! with the next round trip measured, which (Karn's rule) only a chunk
//! sent once gives. Without the doubling, a timeout under the real round
//! trip had every chunk resent before its ack, so nothing was measured and
//! the timeout never grew.
//!
//! Each seat's address is given, so a seat can sit behind a proxy: the
//! seat a datagram is from is its header's to say, not its source address.
//! Damaged datagrams are dropped and counted.
//!
//! [`Udp::relayed`]: every seat through `gbaa-server`'s plain-UDP rooms
//! ([`relay::rooms`]), each datagram wrapped as the rooms pass it on;
//! [`Udp::host`] and [`Udp::join`] take a seat in a room first.

use std::collections::{BTreeMap, VecDeque};
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::{Delivery, To, Transport};

/// The rooms server's datagram tags, and a room code's length.
mod rooms {
    pub use relay::rooms::{DATA, ERROR, GONE, HOST, JOIN, LEAVE, ROOM};
    pub const CODE: usize = relay::CODE_LEN;
}

/// How [`Udp::host`]'s and [`Udp::join`]'s error starts when the rooms
/// server said no (no such room, full, too old a version); its words
/// follow.
pub const REFUSED: &str = "the rooms server says: ";

/// The most of a reliable message one datagram carries: under any path's
/// MTU, as a real network needs.
pub const CHUNK: usize = 1200;
/// Chunks in flight to one seat before the rest wait for acks.
pub const WINDOW: usize = 64;

/// The resend timeout's floor, and its ceiling however often it doubles.
pub const RTO_MIN: Duration = Duration::from_millis(20);
pub const RTO_MAX: Duration = Duration::from_secs(1);

const UNRELIABLE: u8 = 0;
const RELIABLE: u8 = 1;
const ACK: u8 = 2;
const BYE: u8 = 3;

/// How a seat left ([`Udp::gone`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gone {
    /// It said goodbye: its player quit, or left the room.
    Said,
    /// Nothing came from it for the time given: its machine stopped, or
    /// its connection did.
    Quiet,
}

/// Counts over a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub datagrams: u64,
    pub resent: u64,
    /// Datagrams that didn't parse or claimed a seat they can't be.
    pub damaged: u64,
}

struct Chunk {
    packet: Vec<u8>,
    sent: Option<Instant>,
    resent: bool,
}

/// What this seat sends one other seat reliably.
#[derive(Default)]
struct Out {
    next: u32,
    /// Unacknowledged chunks, by number.
    chunks: BTreeMap<u32, Chunk>,
}

/// What one other seat sends this one reliably.
#[derive(Default)]
struct In {
    /// The first chunk not yet taken in.
    next: u32,
    early: BTreeMap<u32, (bool, Vec<u8>)>,
    partial: Vec<u8>,
}

pub struct Udp {
    sock: UdpSocket,
    seat: usize,
    /// Each seat's address (`None` for this one).
    to: Vec<Option<SocketAddr>>,
    /// The rooms server every datagram goes through, if any.
    via: Option<SocketAddr>,
    out: Vec<Out>,
    inc: Vec<In>,
    ready: VecDeque<(usize, Vec<u8>)>,
    /// Smoothed round trip and its variation (RFC 6298), and the resend
    /// timeout: what they give, doubled on each timeout since.
    srtt: Option<Duration>,
    rttvar: Duration,
    rto: Duration,
    /// The shortest and longest round trip measured.
    rtt_min: Option<Duration>,
    rtt_max: Duration,
    /// When each seat was last heard from (when this seat started, before
    /// its first datagram), and whether it said goodbye.
    heard: Vec<Instant>,
    said_bye: Vec<bool>,
    /// This seat said goodbye: it sends nothing more.
    left: bool,
    pub stats: Stats,
}

impl Udp {
    /// Seat `seat` on `sock`, seat `s` reached at `to[s]` (this seat's own
    /// entry is ignored).
    pub fn new(sock: UdpSocket, seat: usize, to: &[SocketAddr]) -> std::io::Result<Udp> {
        assert!(seat < to.len() && to.len() <= 255);
        sock.set_nonblocking(true)?;
        let seats = to.len();
        Ok(Udp {
            sock,
            seat,
            to: (0..seats).map(|s| (s != seat).then_some(to[s])).collect(),
            via: None,
            out: (0..seats).map(|_| Out::default()).collect(),
            inc: (0..seats).map(|_| In::default()).collect(),
            ready: VecDeque::new(),
            srtt: None,
            rttvar: Duration::ZERO,
            rto: Duration::from_millis(200),
            rtt_min: None,
            rtt_max: Duration::ZERO,
            heard: vec![Instant::now(); seats],
            said_bye: vec![false; seats],
            left: false,
            stats: Stats::default(),
        })
    }

    /// Seat `seat` of `seats` in a room on the rooms server `server`,
    /// every datagram through it.
    pub fn relayed(
        sock: UdpSocket,
        seat: usize,
        seats: usize,
        server: SocketAddr,
    ) -> std::io::Result<Udp> {
        let mut u = Udp::new(sock, seat, &vec![server; seats])?;
        u.via = Some(server);
        Ok(u)
    }

    /// Make a room of `seats` on the rooms server `server` for `build`, say
    /// its code (`say`), and wait while `go_on` says so for every seat to
    /// fill. This peer is seat 0. The server may refuse, as too old a
    /// [`crate::VERSION`]: the error is then a [`REFUSED`] one.
    pub fn host(
        server: SocketAddr,
        seats: usize,
        build: &[u8; 32],
        say: impl FnOnce([u8; rooms::CODE]),
        go_on: impl FnMut() -> bool,
    ) -> Result<(Udp, [u8; rooms::CODE]), String> {
        let mut ask = vec![rooms::HOST, seats as u8];
        ask.extend_from_slice(build);
        ask.extend_from_slice(&crate::VERSION.to_le_bytes());
        Udp::seat(server, &ask, Some(say), go_on)
    }

    /// Take a seat in room `code` on the rooms server `server`, waiting
    /// while `go_on` says so for the room to fill.
    pub fn join(
        server: SocketAddr,
        code: [u8; rooms::CODE],
        build: &[u8; 32],
        go_on: impl FnMut() -> bool,
    ) -> Result<(Udp, [u8; rooms::CODE]), String> {
        let mut ask = vec![rooms::JOIN];
        ask.extend_from_slice(&code);
        ask.extend_from_slice(build);
        ask.extend_from_slice(&crate::VERSION.to_le_bytes());
        Udp::seat(server, &ask, None::<fn(_)>, go_on)
    }

    fn seat(
        server: SocketAddr,
        ask: &[u8],
        mut say: Option<impl FnOnce([u8; rooms::CODE])>,
        mut go_on: impl FnMut() -> bool,
    ) -> Result<(Udp, [u8; rooms::CODE]), String> {
        let bind = if server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let sock = UdpSocket::bind(bind).map_err(|e| format!("bind: {e}"))?;
        sock.set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 2048];
        let mut sent: Option<Instant> = None;
        let mut heard = false;
        let start = Instant::now();
        while go_on() {
            if !heard && start.elapsed() > Duration::from_secs(10) {
                return Err(format!("no answer from the rooms server at {server}"));
            }
            if sent.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
                // Asking again is how a seated player keeps its seat (and
                // its NAT mapping) while the room fills.
                sock.send_to(ask, server)
                    .map_err(|e| format!("{server}: {e}"))?;
                sent = Some(Instant::now());
            }
            let (n, from) = match sock.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    continue;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset
                    ) =>
                {
                    continue;
                }
                Err(e) => {
                    let _ = sock.send_to(&[rooms::LEAVE], server);
                    return Err(format!("{server}: {e}"));
                }
            };
            let d = &buf[..n];
            if from != server {
                continue;
            }
            match d.first() {
                Some(&rooms::ERROR) => {
                    return Err(format!("{REFUSED}{}", String::from_utf8_lossy(&d[1..])));
                }
                Some(&rooms::ROOM) if n == 1 + rooms::CODE + 3 => {
                    heard = true;
                    let code: [u8; rooms::CODE] = d[1..1 + rooms::CODE].try_into().unwrap();
                    let (seat, seats, filled) = (d[7] as usize, d[8] as usize, d[9]);
                    if let Some(say) = say.take() {
                        say(code);
                    }
                    if filled as usize == seats {
                        sock.set_read_timeout(None).map_err(|e| e.to_string())?;
                        let u =
                            Udp::relayed(sock, seat, seats, server).map_err(|e| e.to_string())?;
                        return Ok((u, code));
                    }
                }
                _ => {}
            }
        }
        // The seat held while the room filled is given up, so a room
        // nobody waits in closes.
        if sent.is_some() {
            let _ = sock.send_to(&[rooms::LEAVE], server);
        }
        Err("stopped waiting for the room to fill".into())
    }

    pub fn seat_of(&self) -> usize {
        self.seat
    }

    pub fn seats(&self) -> usize {
        self.to.len()
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// Reliable chunks sent but not yet acknowledged, every seat together:
    /// zero once everything this seat sent reliably has arrived.
    pub fn unacked(&self) -> usize {
        self.out.iter().map(|o| o.chunks.len()).sum()
    }

    /// The first other seat that has left, and how: said goodbye, or sent
    /// nothing for `quiet`. What has arrived is taken in first (and kept
    /// for [`Transport::recv`]), so a peer that was itself stopped a while
    /// judges by what came meanwhile, not by its own silence.
    pub fn gone(&mut self, quiet: Duration) -> Option<(usize, Gone)> {
        if !self.left {
            self.poll();
        }
        let now = Instant::now();
        (0..self.to.len())
            .filter(|&s| s != self.seat)
            .find_map(|s| {
                if self.said_bye[s] {
                    Some((s, Gone::Said))
                } else {
                    (now.duration_since(self.heard[s]) >= quiet).then_some((s, Gone::Quiet))
                }
            })
    }

    /// How long the quietest other seat has sent nothing (as of what has
    /// arrived so far): near zero while every peer answers.
    pub fn quiet_for(&self) -> Duration {
        let now = Instant::now();
        (0..self.to.len())
            .filter(|&s| s != self.seat)
            .map(|s| now.duration_since(self.heard[s]))
            .max()
            .unwrap_or_default()
    }

    /// Say goodbye to every other seat, and give the seat up on the rooms
    /// server: sent a few times, as nothing acknowledges it. Nothing more
    /// is sent after.
    pub fn leave(&mut self) {
        if self.left {
            return;
        }
        let bye = self.header(BYE);
        for _ in 0..3 {
            for s in 0..self.to.len() {
                if s != self.seat {
                    self.send_to(s, &bye);
                }
            }
        }
        if let Some(server) = self.via {
            for _ in 0..3 {
                let _ = self.sock.send_to(&[rooms::LEAVE], server);
            }
        }
        self.left = true;
        for o in &mut self.out {
            o.chunks.clear();
        }
    }

    /// Whether this seat has said goodbye ([`Udp::leave`]).
    pub fn has_left(&self) -> bool {
        self.left
    }

    fn send_to(&mut self, to: usize, packet: &[u8]) {
        if self.left {
            return;
        }
        if let Some(addr) = self.to[to] {
            self.stats.datagrams += 1;
            // A full buffer or an unreachable peer is a lost datagram.
            if self.via.is_some() {
                let mut d = Vec::with_capacity(packet.len() + 2);
                d.extend_from_slice(&[rooms::DATA, to as u8]);
                d.extend_from_slice(packet);
                let _ = self.sock.send_to(&d, addr);
            } else {
                let _ = self.sock.send_to(packet, addr);
            }
        }
    }

    fn header(&self, kind: u8) -> Vec<u8> {
        vec![kind, self.seat as u8]
    }

    fn send_one(&mut self, to: usize, how: Delivery, payload: &[u8]) {
        match how {
            Delivery::Unreliable => {
                let mut p = self.header(UNRELIABLE);
                p.extend_from_slice(payload);
                self.send_to(to, &p);
            }
            Delivery::Reliable => {
                let n = payload.len().div_ceil(CHUNK).max(1);
                for i in 0..n {
                    let part = &payload
                        [(i * CHUNK).min(payload.len())..((i + 1) * CHUNK).min(payload.len())];
                    let o = &mut self.out[to];
                    let num = o.next;
                    o.next += 1;
                    let mut p = vec![RELIABLE, self.seat as u8];
                    p.extend_from_slice(&num.to_le_bytes());
                    p.push((i + 1 == n) as u8);
                    p.extend_from_slice(part);
                    o.chunks.insert(
                        num,
                        Chunk {
                            packet: p,
                            sent: None,
                            resent: false,
                        },
                    );
                }
                self.flush(to);
            }
        }
    }

    /// Send what's in the window to `to` and not yet sent, and again what
    /// has waited longer than the timeout, doubling it if anything had.
    fn flush(&mut self, to: usize) {
        let now = Instant::now();
        let rto = self.rto;
        let mut due = Vec::new();
        for (_, c) in self.out[to].chunks.iter_mut().take(WINDOW) {
            match c.sent {
                Some(t) if now.duration_since(t) < rto => {}
                Some(_) => {
                    c.resent = true;
                    c.sent = Some(now);
                    due.push((true, c.packet.clone()));
                }
                None => {
                    c.sent = Some(now);
                    due.push((false, c.packet.clone()));
                }
            }
        }
        if due.iter().any(|(again, _)| *again) {
            self.rto = (self.rto * 2).min(RTO_MAX);
        }
        for (again, p) in due {
            self.stats.resent += again as u64;
            self.send_to(to, &p);
        }
    }

    /// Take in every datagram waiting, then resend what's due.
    fn poll(&mut self) {
        let mut buf = vec![0u8; 65536];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let d = match self.via {
                        None => buf[..n].to_vec(),
                        // The server says a seat's player has left.
                        Some(v) if from == v && n == 2 && buf[0] == rooms::GONE => {
                            let s = buf[1] as usize;
                            if s < self.to.len() && s != self.seat {
                                self.said_bye[s] = true;
                            }
                            continue;
                        }
                        // The room's own word (a seat filling, again): not ours.
                        Some(v) if from != v || buf[0] != rooms::DATA => continue,
                        Some(_) if n < 2 => continue,
                        Some(_) => buf[2..n].to_vec(),
                    };
                    if self.take(&d).is_none() {
                        self.stats.damaged += 1;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                // An earlier datagram bounced (some systems say so on the
                // next receive): the peer isn't there yet, nothing more.
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset
                    ) => {}
                Err(_) => break,
            }
        }
        for s in 0..self.to.len() {
            if !self.out[s].chunks.is_empty() {
                self.flush(s);
            }
        }
    }

    fn take(&mut self, d: &[u8]) -> Option<()> {
        let (&kind, rest) = d.split_first()?;
        let (&from, rest) = rest.split_first()?;
        let from = from as usize;
        if from >= self.to.len() || from == self.seat {
            return None;
        }
        if kind > BYE {
            return None;
        }
        self.heard[from] = Instant::now();
        match kind {
            BYE => self.said_bye[from] = true,
            UNRELIABLE => self.ready.push_back((from, rest.to_vec())),
            RELIABLE => {
                let num = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
                let last = match *rest.get(4)? {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                let chunk = &rest[5..];
                let inc = &mut self.inc[from];
                if num >= inc.next {
                    inc.early.insert(num, (last, chunk.to_vec()));
                }
                while let Some((last, c)) = inc.early.remove(&inc.next) {
                    inc.next += 1;
                    inc.partial.extend_from_slice(&c);
                    if last {
                        self.ready
                            .push_back((from, std::mem::take(&mut inc.partial)));
                    }
                }
                let inc = &self.inc[from];
                let mut have = 0u64;
                for (&n, _) in inc.early.range(inc.next + 1..inc.next + 65) {
                    have |= 1 << (n - inc.next - 1);
                }
                let mut ack = self.header(ACK);
                ack.extend_from_slice(&inc.next.to_le_bytes());
                ack.extend_from_slice(&have.to_le_bytes());
                self.send_to(from, &ack);
            }
            ACK => {
                if rest.len() != 12 {
                    return None;
                }
                let upto = u32::from_le_bytes(rest[..4].try_into().ok()?);
                let have = u64::from_le_bytes(rest[4..].try_into().ok()?);
                let now = Instant::now();
                let o = &mut self.out[from];
                let mut sample = None;
                let mut acked = |c: Chunk| {
                    // Karn: a resent chunk's ack might be the first send's.
                    if let (false, Some(t)) = (c.resent, c.sent) {
                        sample = Some(now.duration_since(t));
                    }
                };
                while let Some(e) = o.chunks.first_entry() {
                    if *e.key() >= upto {
                        break;
                    }
                    acked(e.remove());
                }
                for i in (0..64).filter(|i| have >> i & 1 == 1) {
                    if let Some(c) = upto.checked_add(1 + i).and_then(|n| o.chunks.remove(&n)) {
                        acked(c);
                    }
                }
                if let Some(r) = sample {
                    self.measured(r);
                }
            }
            _ => return None,
        }
        Some(())
    }

    /// The smoothed round trip to the other seats, measured on reliable
    /// chunks' acks (the hashes, every few frames), and the shortest and
    /// longest one measured; `None` before the first.
    pub fn rtt(&self) -> Option<(Duration, Duration, Duration)> {
        Some((self.srtt?, self.rtt_min?, self.rtt_max))
    }

    fn measured(&mut self, r: Duration) {
        self.rtt_min = Some(self.rtt_min.map_or(r, |m| m.min(r)));
        self.rtt_max = self.rtt_max.max(r);
        let srtt = match self.srtt {
            None => {
                self.rttvar = r / 2;
                r
            }
            Some(s) => {
                let diff = s.abs_diff(r);
                self.rttvar = (self.rttvar * 3 + diff) / 4;
                (s * 7 + r) / 8
            }
        };
        self.srtt = Some(srtt);
        self.rto = (srtt + self.rttvar * 4).clamp(RTO_MIN, RTO_MAX);
    }
}

impl Transport for Udp {
    fn send(&mut self, to: To, how: Delivery, payload: Vec<u8>) {
        match to {
            To::Seat(s) => self.send_one(s, how, &payload),
            To::Everyone => {
                let me = self.seat;
                for s in (0..self.to.len()).filter(|&s| s != me) {
                    self.send_one(s, how, &payload);
                }
            }
        }
    }

    fn recv(&mut self) -> Option<(usize, Vec<u8>)> {
        if self.ready.is_empty() {
            self.poll();
        }
        self.ready.pop_front()
    }
}
