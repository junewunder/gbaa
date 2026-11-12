//! A player's wireless adapter on a radio that's on the server
//! (multiplayer/wireless-radio-netplay, `docs/research/multiplayer.md`
//! option F): this player runs only its own console, with a networked
//! adapter ([`Wireless::networked`]), and [`Radio::pump`] carries what the
//! adapter does to the room's radio on `gbaa-server` ([`relay::radio`],
//! the rooms' `W` datagrams) and what the radio says back.
//!
//! Unlike netplay's other sessions, nothing here is rolled back or kept
//! in step: each console runs at its own pace, as two real GBAs with
//! adapters do, and the radio's messages arrive when the network brings
//! them.

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use relay::radio::Channel;
use relay::rooms::{KEEP, LEAVE, RADIO};
use runtime::link::rfu::Wireless;
use runtime::link::rfu::air::{Down, MAX_MSG};

use crate::udp::Udp;

/// How often a player with nothing to say tells the server it's there.
const KEEP_EVERY: Duration = Duration::from_secs(1);

/// Counts over a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub ups: u64,
    pub downs: u64,
    pub datagrams_in: u64,
    pub datagrams_out: u64,
}

pub struct Radio {
    sock: UdpSocket,
    server: SocketAddr,
    seat: usize,
    chan: Channel,
    buf: Vec<u8>,
    pub stats: Stats,
}

impl Radio {
    /// Make a room of `seats` on the rooms server `server` and wait (while
    /// `go_on` says so) for it to fill, as [`Udp::host`] does.
    pub fn host(
        server: SocketAddr,
        seats: usize,
        build: &[u8; 32],
        say: impl FnMut([u8; relay::CODE_LEN], usize, usize),
        go_on: impl FnMut() -> bool,
    ) -> Result<(Radio, [u8; relay::CODE_LEN]), String> {
        let (u, code) = Udp::host(server, seats, build, say, go_on)?;
        Ok((Radio::from(u, server)?, code))
    }

    /// Take a seat in room `code`, as [`Udp::join`] does.
    pub fn join(
        server: SocketAddr,
        code: [u8; relay::CODE_LEN],
        build: &[u8; 32],
        go_on: impl FnMut() -> bool,
    ) -> Result<Radio, String> {
        let (u, _) = Udp::join(server, code, build, go_on)?;
        Radio::from(u, server)
    }

    fn from(u: Udp, server: SocketAddr) -> Result<Radio, String> {
        let (sock, seat, _) = u.into_parts();
        sock.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Radio {
            sock,
            server,
            seat,
            chan: Channel::new(),
            buf: vec![0; 2048],
            stats: Stats::default(),
        })
    }

    /// This player's seat in the room.
    pub fn seat(&self) -> usize {
        self.seat
    }

    /// Send what adapter `w` has to tell the radio, give it what the radio
    /// said, and send again what's due.
    pub fn pump(&mut self, w: &mut Wireless, now: Instant) {
        let mut b = [0u8; MAX_MSG];
        while let Some(up) = w.take_up() {
            let n = up.encode(&mut b);
            self.chan.send(&b[..n]);
            self.stats.ups += 1;
        }
        loop {
            let (n, from) = match self.sock.recv_from(&mut self.buf) {
                Ok(r) => r,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                // A bounce, or anything else: the next pump tries again.
                Err(_) => break,
            };
            if from != self.server || n == 0 || self.buf[0] != RADIO {
                continue;
            }
            self.stats.datagrams_in += 1;
            for m in self.chan.take(&self.buf[1..n], now) {
                if let Some(d) = Down::decode(&m) {
                    self.stats.downs += 1;
                    w.deliver(d);
                }
            }
        }
        let mut ds = self.chan.flush(now);
        if ds.is_empty()
            && self
                .chan
                .last_sent
                .is_none_or(|t| now.saturating_duration_since(t) >= KEEP_EVERY)
        {
            // The first word opens this seat's channel on the server;
            // later, it keeps the seat.
            ds = if self.chan.last_sent.is_none() {
                vec![0u32.to_le_bytes().to_vec()]
            } else {
                let _ = self.sock.send_to(&[KEEP], self.server);
                self.chan.last_sent = Some(now);
                Vec::new()
            };
            self.chan.last_sent = Some(now);
        }
        for d in ds {
            let mut m = Vec::with_capacity(d.len() + 1);
            m.push(RADIO);
            m.extend_from_slice(&d);
            let _ = self.sock.send_to(&m, self.server);
            self.stats.datagrams_out += 1;
        }
    }

    /// Messages resent so far.
    pub fn resent(&self) -> u64 {
        self.chan.resent
    }

    /// Give the seat up (sent a few times; nothing acknowledges it).
    pub fn leave(&mut self) {
        for _ in 0..3 {
            let _ = self.sock.send_to(&[LEAVE], self.server);
        }
    }
}
