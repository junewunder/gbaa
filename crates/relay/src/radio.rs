//! The wireless adapter's radio on the server (multiplayer/
//! wireless-radio-netplay): a room's players each run their own console
//! and adapter, and the room's [`runtime::link::rfu::air::Air`] is the
//! radio between them ([`crate::rooms`]' `W` datagrams). What crosses is
//! the radio's messages ([`runtime::link::rfu::air`]'s `Up` and `Down`),
//! each way over a [`Channel`]: reliable and in order, so a lost
//! `Connect` or packet costs a resend, never a radio out of step.
//!
//! A channel datagram is the number of the first message its sender
//! hasn't received from the other side (`u32`; every one before it has
//! come), then any number of messages, each its number (`u32`, counting
//! from 0), length (`u8`) and bytes. A message unacknowledged after the
//! resend timeout goes again; the timeout is twice the smoothed round
//! trip, 30 ms to 1 s.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// The most message bytes one datagram carries.
pub const DATAGRAM: usize = 1200;

const RTO_MIN: Duration = Duration::from_millis(30);
const RTO_MAX: Duration = Duration::from_secs(1);

/// One side of a reliable, ordered stream of small messages (up to 255
/// bytes each) over datagrams.
#[derive(Debug)]
pub struct Channel {
    next_out: u32,
    /// Sent and not yet acknowledged: number, bytes, when last sent, and
    /// whether more than once.
    unacked: VecDeque<(u32, Vec<u8>, Option<Instant>, bool)>,
    next_in: u32,
    early: BTreeMap<u32, Vec<u8>>,
    /// Something came in: the other side wants to hear it did.
    ack_due: bool,
    srtt: Option<Duration>,
    /// When this side last sent anything.
    pub last_sent: Option<Instant>,
    /// Messages sent again.
    pub resent: u64,
}

impl Default for Channel {
    fn default() -> Channel {
        Channel::new()
    }
}

impl Channel {
    pub fn new() -> Channel {
        Channel {
            next_out: 0,
            unacked: VecDeque::new(),
            next_in: 0,
            early: BTreeMap::new(),
            ack_due: false,
            srtt: None,
            last_sent: None,
            resent: 0,
        }
    }

    /// Queue `msg` (at most 255 bytes) to go at the next [`Channel::flush`].
    pub fn send(&mut self, msg: &[u8]) {
        assert!(msg.len() <= 255, "a channel message is at most 255 bytes");
        self.unacked
            .push_back((self.next_out, msg.to_vec(), None, false));
        self.next_out = self.next_out.wrapping_add(1);
    }

    /// Messages sent and not yet acknowledged.
    pub fn unacked(&self) -> usize {
        self.unacked.len()
    }

    fn rto(&self) -> Duration {
        self.srtt.map_or(Duration::from_millis(200), |s| {
            (s * 2).clamp(RTO_MIN, RTO_MAX)
        })
    }

    /// The datagrams to send now: new messages, those due again, and an
    /// acknowledgement if one is owed.
    pub fn flush(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let rto = self.rto();
        let mut out = Vec::new();
        let mut d = self.next_in.to_le_bytes().to_vec();
        for (n, msg, sent, again) in self.unacked.iter_mut() {
            let due = match sent {
                None => true,
                Some(t) => now.saturating_duration_since(*t) >= rto,
            };
            if !due {
                continue;
            }
            if sent.is_some() {
                *again = true;
                self.resent += 1;
            }
            *sent = Some(now);
            if d.len() + 5 + msg.len() > DATAGRAM {
                out.push(std::mem::replace(
                    &mut d,
                    self.next_in.to_le_bytes().to_vec(),
                ));
            }
            d.extend_from_slice(&n.to_le_bytes());
            d.push(msg.len() as u8);
            d.extend_from_slice(msg);
        }
        if d.len() > 4 || self.ack_due {
            out.push(d);
        }
        self.ack_due = false;
        if !out.is_empty() {
            self.last_sent = Some(now);
        }
        out
    }

    /// Take datagram `d` from the other side: the messages it lets
    /// through, in order. A damaged one is dropped whole.
    pub fn take(&mut self, d: &[u8], now: Instant) -> Vec<Vec<u8>> {
        let Some((ack, mut rest)) = d.split_first_chunk::<4>() else {
            return Vec::new();
        };
        let ack = u32::from_le_bytes(*ack);
        let mut msgs = Vec::new();
        while !rest.is_empty() {
            let Some((n, r)) = rest.split_first_chunk::<4>() else {
                return Vec::new();
            };
            let Some((&len, r)) = r.split_first() else {
                return Vec::new();
            };
            if r.len() < len as usize {
                return Vec::new();
            }
            let (msg, r) = r.split_at(len as usize);
            msgs.push((u32::from_le_bytes(*n), msg));
            rest = r;
        }
        // Acknowledged: everything before `ack`.
        while let Some((n, _, sent, again)) = self.unacked.front() {
            if ack.wrapping_sub(*n) as i32 <= 0 {
                break;
            }
            if let (Some(t), false) = (sent, again) {
                let rtt = now.saturating_duration_since(*t);
                self.srtt = Some(self.srtt.map_or(rtt, |s| (s * 7 + rtt) / 8));
            }
            self.unacked.pop_front();
        }
        let mut got = Vec::new();
        for (n, msg) in msgs {
            self.ack_due = true;
            if n.wrapping_sub(self.next_in) as i32 >= 0 {
                self.early.entry(n).or_insert_with(|| msg.to_vec());
            }
        }
        while let Some(msg) = self.early.remove(&self.next_in) {
            got.push(msg);
            self.next_in = self.next_in.wrapping_add(1);
        }
        got
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through a line that loses every third datagram, and reorders: every
    /// message arrives once, in order.
    #[test]
    fn messages_arrive_once_in_order_through_loss() {
        let (mut a, mut b) = (Channel::new(), Channel::new());
        let t0 = Instant::now();
        let mut got = Vec::new();
        let mut k = 0;
        for i in 0..100u8 {
            a.send(&[i, i ^ 0x55]);
            let now = t0 + Duration::from_millis(10 * i as u64);
            let mut ds = a.flush(now);
            ds.reverse();
            for d in ds {
                k += 1;
                if k % 3 == 0 {
                    continue;
                }
                got.extend(b.take(&d, now));
            }
            for d in b.flush(now) {
                a.take(&d, now);
            }
        }
        for i in 0..200 {
            let now = t0 + Duration::from_millis(1000 + 50 * i);
            for d in a.flush(now) {
                got.extend(b.take(&d, now));
            }
            for d in b.flush(now) {
                a.take(&d, now);
            }
        }
        let want: Vec<Vec<u8>> = (0..100u8).map(|i| vec![i, i ^ 0x55]).collect();
        assert_eq!(got, want);
        assert_eq!(a.unacked(), 0);
        assert!(a.resent > 0);
    }
}
