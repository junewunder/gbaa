//! The radio over a network (multiplayer/wireless-radio-netplay;
//! `docs/research/multiplayer.md`, option F): each player runs only their
//! own console and adapter ([`super::Wireless::networked`]), and the radio
//! (who hosts, with which broadcast, who is whose child in which slot,
//! and the packets between them) is an [`Air`] on the server
//! (`relay::rooms`), which every adapter tells what it does ([`Up`]) and
//! hears from ([`Down`]).
//!
//! The adapter answers its GBA at once from its own view of the radio, as
//! a real one does from what it last heard: a `Connect` is pending
//! (`IsConnectionComplete` says so) until the server's [`Down::Joined`] or
//! [`Down::Refused`] comes; a search lists the hosts the server last told
//! of. Host IDs are the adapter's own (from its seat, so no two collide);
//! a child's comes from the server. A host's packet goes to every child,
//! a child's to its host, each as it's sent; each side reads them in
//! order, one per `ReceiveData`, so none is lost to the network's
//! jitter (Emerald numbers its packets and calls five out of turn a
//! link error).
//!
//! Every message is a tag byte, then its fields, little-endian. The
//! transport (`relay::radio`'s channel) delivers them reliably, in order.

use super::{MAX_PLAYERS, PACKET, Packet};

/// The most hosts a [`Down::Hosts`] lists (`BroadcastRead` gives four).
pub const MAX_HOSTS: usize = 4;

/// What an adapter tells the radio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Up {
    /// Hosting as `id`: open to children or not, how many it takes, and
    /// its broadcast. Sent again whenever one of them changes.
    Host {
        id: u16,
        open: bool,
        max: u8,
        broadcast: [u32; 6],
    },
    /// Out of the radio: a host drops its children, a child leaves its
    /// host, a pending connection is given up.
    Leave,
    /// Join the host whose ID is `id`.
    Connect { id: u16 },
    /// A host sends the children in these slots away.
    Kick { mask: u8 },
    /// A host's packet for every child, or a child's for its host.
    Data(Packet),
}

/// One host as a search sees it: its ID, the slot the next child would
/// take (`0xFF`: none), and its broadcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heard {
    pub id: u16,
    pub slot: u8,
    pub broadcast: [u32; 6],
}

impl Heard {
    /// `BroadcastRead`'s entry: the metadata word, then the broadcast.
    pub fn entry(&self) -> [u32; 7] {
        let mut e = [self.id as u32 | (self.slot as u32) << 16; 7];
        e[1..].copy_from_slice(&self.broadcast);
        e
    }
}

/// What the radio tells an adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Down {
    /// Every open host, as of now (sent whenever it changes).
    Hosts { n: u8, hosts: [Heard; MAX_HOSTS] },
    /// The connection to host `host` is made: this adapter is `id`, in
    /// slot `slot`.
    Joined { host: u16, id: u16, slot: u8 },
    /// Host `host` didn't take this adapter (gone, closed or full).
    Refused { host: u16 },
    /// A host's children: each slot's child ID, 0 for none.
    Children { ids: [u16; 4] },
    /// This child is out of its host's room: sent away (0) or the host
    /// gone (1).
    Dropped { reason: u8 },
    /// A packet: from the host (`from` `0xFF`), or from the child in slot
    /// `from`.
    Data { from: u8, packet: Packet },
}

/// From the host, in [`Down::Data`].
pub const FROM_HOST: u8 = 0xFF;

/// The most bytes an encoded message takes.
pub const MAX_MSG: usize = 2 + MAX_HOSTS * 27 + 8;

const HOST: u8 = 1;
const LEAVE: u8 = 2;
const CONNECT: u8 = 3;
const KICK: u8 = 4;
const DATA: u8 = 5;
const HOSTS: u8 = 6;
const JOINED: u8 = 7;
const REFUSED: u8 = 8;
const CHILDREN: u8 = 9;
const DROPPED: u8 = 10;

struct W<'a> {
    b: &'a mut [u8; MAX_MSG],
    n: usize,
}

impl W<'_> {
    fn u8(&mut self, v: u8) {
        self.b[self.n] = v;
        self.n += 1;
    }
    fn u16(&mut self, v: u16) {
        for x in v.to_le_bytes() {
            self.u8(x);
        }
    }
    fn u32(&mut self, v: u32) {
        for x in v.to_le_bytes() {
            self.u8(x);
        }
    }
    fn packet(&mut self, p: &Packet) {
        self.u8(p.n as u8);
        for &x in p.bytes() {
            self.u8(x);
        }
    }
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn u8(&mut self) -> Option<u8> {
        let (&v, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes([self.u8()?, self.u8()?]))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes([
            self.u8()?,
            self.u8()?,
            self.u8()?,
            self.u8()?,
        ]))
    }
    fn packet(&mut self) -> Option<Packet> {
        let n = self.u8()? as usize;
        if n > PACKET || self.0.len() < n {
            return None;
        }
        let (b, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(Packet::of(b))
    }
    fn broadcast(&mut self) -> Option<[u32; 6]> {
        let mut b = [0; 6];
        for w in &mut b {
            *w = self.u32()?;
        }
        Some(b)
    }
}

impl Up {
    /// The message's bytes in `b`; how many.
    pub fn encode(&self, b: &mut [u8; MAX_MSG]) -> usize {
        let mut w = W { b, n: 0 };
        match self {
            Up::Host {
                id,
                open,
                max,
                broadcast,
            } => {
                w.u8(HOST);
                w.u16(*id);
                w.u8(*open as u8);
                w.u8(*max);
                for &x in broadcast {
                    w.u32(x);
                }
            }
            Up::Leave => w.u8(LEAVE),
            Up::Connect { id } => {
                w.u8(CONNECT);
                w.u16(*id);
            }
            Up::Kick { mask } => {
                w.u8(KICK);
                w.u8(*mask);
            }
            Up::Data(p) => {
                w.u8(DATA);
                w.packet(p);
            }
        }
        w.n
    }

    /// `None` if `b` isn't one whole message.
    pub fn decode(b: &[u8]) -> Option<Up> {
        let mut r = R(b);
        let m = match r.u8()? {
            HOST => Up::Host {
                id: r.u16()?,
                open: r.u8()? != 0,
                max: r.u8()?,
                broadcast: r.broadcast()?,
            },
            LEAVE => Up::Leave,
            CONNECT => Up::Connect { id: r.u16()? },
            KICK => Up::Kick { mask: r.u8()? },
            DATA => Up::Data(r.packet()?),
            _ => return None,
        };
        r.0.is_empty().then_some(m)
    }
}

impl Down {
    /// The message's bytes in `b`; how many.
    pub fn encode(&self, b: &mut [u8; MAX_MSG]) -> usize {
        let mut w = W { b, n: 0 };
        match self {
            Down::Hosts { n, hosts } => {
                w.u8(HOSTS);
                w.u8(*n);
                for h in &hosts[..(*n as usize).min(MAX_HOSTS)] {
                    w.u16(h.id);
                    w.u8(h.slot);
                    for &x in &h.broadcast {
                        w.u32(x);
                    }
                }
            }
            Down::Joined { host, id, slot } => {
                w.u8(JOINED);
                w.u16(*host);
                w.u16(*id);
                w.u8(*slot);
            }
            Down::Refused { host } => {
                w.u8(REFUSED);
                w.u16(*host);
            }
            Down::Children { ids } => {
                w.u8(CHILDREN);
                for &id in ids {
                    w.u16(id);
                }
            }
            Down::Dropped { reason } => {
                w.u8(DROPPED);
                w.u8(*reason);
            }
            Down::Data { from, packet } => {
                w.u8(DATA);
                w.u8(*from);
                w.packet(packet);
            }
        }
        w.n
    }

    /// `None` if `b` isn't one whole message.
    pub fn decode(b: &[u8]) -> Option<Down> {
        let mut r = R(b);
        let m = match r.u8()? {
            HOSTS => {
                let n = r.u8()?;
                if n as usize > MAX_HOSTS {
                    return None;
                }
                let mut hosts = [Heard {
                    id: 0,
                    slot: 0,
                    broadcast: [0; 6],
                }; MAX_HOSTS];
                for h in &mut hosts[..n as usize] {
                    *h = Heard {
                        id: r.u16()?,
                        slot: r.u8()?,
                        broadcast: r.broadcast()?,
                    };
                }
                Down::Hosts { n, hosts }
            }
            JOINED => Down::Joined {
                host: r.u16()?,
                id: r.u16()?,
                slot: r.u8()?,
            },
            REFUSED => Down::Refused { host: r.u16()? },
            CHILDREN => {
                let mut ids = [0; 4];
                for id in &mut ids {
                    *id = r.u16()?;
                }
                Down::Children { ids }
            }
            DROPPED => Down::Dropped { reason: r.u8()? },
            DATA => Down::Data {
                from: r.u8()?,
                packet: r.packet()?,
            },
            _ => return None,
        };
        r.0.is_empty().then_some(m)
    }
}

/// A seat's place in the radio, as the server keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seat {
    Idle,
    Host {
        id: u16,
        open: bool,
        max: u8,
        broadcast: [u32; 6],
        /// Each slot's child: its seat and ID.
        children: [Option<(u8, u16)>; 4],
    },
    Child {
        host: u8,
        slot: u8,
    },
}

/// The radio between up to [`MAX_PLAYERS`] seats' adapters, on the
/// server: the one place that says who hosts and who is whose child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Air {
    seats: [Seat; MAX_PLAYERS],
    next_id: u16,
    /// The open hosts last told of.
    told: Option<Down>,
}

impl Default for Air {
    fn default() -> Air {
        Air::new()
    }
}

impl Air {
    pub const fn new() -> Air {
        Air {
            seats: [Seat::Idle; MAX_PLAYERS],
            next_id: 0x8001,
            told: None,
        }
    }

    /// A child's ID: from the top half, so it never meets a host's
    /// ([`super::Wireless::networked`]).
    fn new_id(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id = if id == 0xFFFF { 0x8001 } else { id + 1 };
        id
    }

    fn next_slot(&self, h: usize) -> u8 {
        match self.seats[h] {
            Seat::Host {
                open: true,
                max,
                children,
                ..
            } => children[..(max as usize).min(4)]
                .iter()
                .position(Option::is_none)
                .map_or(0xFF, |s| s as u8),
            _ => 0xFF,
        }
    }

    fn hosts(&self) -> Down {
        let mut hosts = [Heard {
            id: 0,
            slot: 0,
            broadcast: [0; 6],
        }; MAX_HOSTS];
        let mut n = 0;
        for (s, seat) in self.seats.iter().enumerate() {
            if let Seat::Host {
                id,
                open: true,
                broadcast,
                ..
            } = *seat
                && n < MAX_HOSTS
            {
                hosts[n] = Heard {
                    id,
                    slot: self.next_slot(s),
                    broadcast,
                };
                n += 1;
            }
        }
        Down::Hosts { n: n as u8, hosts }
    }

    fn children(&self, h: usize) -> Down {
        let mut ids = [0; 4];
        if let Seat::Host { children, .. } = self.seats[h] {
            for (id, c) in ids.iter_mut().zip(children) {
                *id = c.map_or(0, |(_, id)| id);
            }
        }
        Down::Children { ids }
    }

    /// Seat `seat` (below [`MAX_PLAYERS`]) says `m`; `out` gets what to
    /// tell whom.
    pub fn take(&mut self, seat: usize, m: Up, out: &mut impl FnMut(usize, Down)) {
        if seat >= MAX_PLAYERS {
            return;
        }
        match m {
            Up::Host {
                id,
                open,
                max,
                broadcast,
            } => {
                let children = match self.seats[seat] {
                    Seat::Host { children, .. } => children,
                    _ => {
                        self.leave(seat, out);
                        [None; 4]
                    }
                };
                self.seats[seat] = Seat::Host {
                    id,
                    open,
                    max: max.min(4),
                    broadcast,
                    children,
                };
            }
            Up::Leave => self.leave(seat, out),
            Up::Connect { id } => {
                self.leave(seat, out);
                let host = (0..MAX_PLAYERS).find(|&h| {
                    h != seat && matches!(self.seats[h], Seat::Host { id: i, .. } if i == id)
                });
                let slot = host.map_or(0xFF, |h| self.next_slot(h));
                match host {
                    Some(h) if slot != 0xFF => {
                        let child = self.new_id();
                        if let Seat::Host { children, .. } = &mut self.seats[h] {
                            children[slot as usize] = Some((seat as u8, child));
                        }
                        self.seats[seat] = Seat::Child {
                            host: h as u8,
                            slot,
                        };
                        out(
                            seat,
                            Down::Joined {
                                host: id,
                                id: child,
                                slot,
                            },
                        );
                        out(h, self.children(h));
                    }
                    _ => out(seat, Down::Refused { host: id }),
                }
            }
            Up::Kick { mask } => {
                if let Seat::Host { children, .. } = self.seats[seat] {
                    for (s, c) in children.iter().enumerate() {
                        if mask >> s & 1 != 0
                            && let Some((c, _)) = *c
                        {
                            self.drop_child(seat, s, c as usize, 0, out);
                        }
                    }
                    out(seat, self.children(seat));
                }
            }
            Up::Data(packet) => match self.seats[seat] {
                Seat::Host { children, .. } => {
                    for (c, _) in children.iter().flatten() {
                        out(
                            *c as usize,
                            Down::Data {
                                from: FROM_HOST,
                                packet,
                            },
                        );
                    }
                }
                Seat::Child { host, slot } => out(host as usize, Down::Data { from: slot, packet }),
                Seat::Idle => {}
            },
        }
        self.tell_hosts(out);
    }

    /// Seat `seat`'s player has gone: out of the radio.
    pub fn gone(&mut self, seat: usize, out: &mut impl FnMut(usize, Down)) {
        if seat < MAX_PLAYERS {
            self.leave(seat, out);
            self.tell_hosts(out);
        }
    }

    /// Seat `seat` has just come: what it must hear first.
    pub fn hello(&self, seat: usize, out: &mut impl FnMut(usize, Down)) {
        out(seat, self.hosts());
    }

    /// Everyone hears the open hosts when they change.
    fn tell_hosts(&mut self, out: &mut impl FnMut(usize, Down)) {
        let h = self.hosts();
        if self.told != Some(h) {
            self.told = Some(h);
            for s in 0..MAX_PLAYERS {
                out(s, h);
            }
        }
    }

    fn drop_child(
        &mut self,
        host: usize,
        slot: usize,
        child: usize,
        reason: u8,
        out: &mut impl FnMut(usize, Down),
    ) {
        if let Seat::Host { children, .. } = &mut self.seats[host] {
            children[slot] = None;
        }
        self.seats[child] = Seat::Idle;
        out(child, Down::Dropped { reason });
    }

    fn leave(&mut self, seat: usize, out: &mut impl FnMut(usize, Down)) {
        match self.seats[seat] {
            Seat::Host { children, .. } => {
                for (s, c) in children.iter().enumerate() {
                    if let Some((c, _)) = *c {
                        self.drop_child(seat, s, c as usize, 1, out);
                    }
                }
            }
            Seat::Child { host, slot } => {
                if let Seat::Host { children, .. } = &mut self.seats[host as usize] {
                    children[slot as usize] = None;
                }
                out(host as usize, self.children(host as usize));
            }
            Seat::Idle => {}
        }
        self.seats[seat] = Seat::Idle;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    fn say(air: &mut Air, seat: usize, m: Up) -> Vec<(usize, Down)> {
        let mut out = Vec::new();
        air.take(seat, m, &mut |s, d| out.push((s, d)));
        out
    }

    fn host(id: u16) -> Up {
        Up::Host {
            id,
            open: true,
            max: 4,
            broadcast: [id as u32; 6],
        }
    }

    #[test]
    fn every_message_round_trips() {
        let p = Packet::of(&[1, 2, 3, 4, 5]);
        let ups = [
            host(0x1001),
            Up::Leave,
            Up::Connect { id: 0x2003 },
            Up::Kick { mask: 5 },
            Up::Data(p),
            Up::Data(Packet::of(&[0xAA; PACKET])),
        ];
        let mut b = [0; MAX_MSG];
        for u in ups {
            let n = u.encode(&mut b);
            assert_eq!(Up::decode(&b[..n]), Some(u));
            assert_eq!(Up::decode(&b[..n - 1]), None, "{u:?} cut short");
        }
        let heard = Heard {
            id: 0x1001,
            slot: 0,
            broadcast: [7; 6],
        };
        let downs = [
            Down::Hosts {
                n: 4,
                hosts: [heard; MAX_HOSTS],
            },
            Down::Hosts {
                n: 0,
                hosts: [heard; MAX_HOSTS],
            },
            Down::Joined {
                host: 0x1001,
                id: 0x8001,
                slot: 2,
            },
            Down::Refused { host: 0x1001 },
            Down::Children {
                ids: [0x8001, 0, 0x8002, 0],
            },
            Down::Dropped { reason: 1 },
            Down::Data {
                from: FROM_HOST,
                packet: p,
            },
        ];
        for d in downs {
            let n = d.encode(&mut b);
            let back = Down::decode(&b[..n]).unwrap();
            // An empty list's unused entries aren't sent.
            if let (Down::Hosts { n: 0, .. }, Down::Hosts { n: k, .. }) = (d, back) {
                assert_eq!(k, 0);
            } else {
                assert_eq!(back, d);
            }
        }
    }

    /// A host opens, a child joins, packets pass both ways, the host
    /// sends the child away; a child of a host that leaves is dropped.
    #[test]
    fn hosts_children_and_packets() {
        let mut air = Air::new();
        let out = say(&mut air, 0, host(0x1001));
        // Everyone hears the host.
        assert_eq!(out.len(), MAX_PLAYERS);
        assert!(
            matches!(out[1].1, Down::Hosts { n: 1, hosts } if hosts[0].id == 0x1001 && hosts[0].slot == 0)
        );
        // No such host.
        let out = say(&mut air, 1, Up::Connect { id: 0x1999 });
        assert!(out.contains(&(1, Down::Refused { host: 0x1999 })));
        let out = say(&mut air, 1, Up::Connect { id: 0x1001 });
        assert!(out.contains(&(
            1,
            Down::Joined {
                host: 0x1001,
                id: 0x8001,
                slot: 0
            }
        )));
        assert!(out.contains(&(
            0,
            Down::Children {
                ids: [0x8001, 0, 0, 0]
            }
        )));
        // The next child would take slot 1.
        assert!(
            out.iter()
                .any(|(s, d)| *s == 2
                    && matches!(d, Down::Hosts { hosts, .. } if hosts[0].slot == 1))
        );
        let p = Packet::of(&[9, 8, 7]);
        assert_eq!(
            say(&mut air, 0, Up::Data(p)),
            [(
                1,
                Down::Data {
                    from: FROM_HOST,
                    packet: p
                }
            )]
        );
        assert_eq!(
            say(&mut air, 1, Up::Data(p)),
            [(0, Down::Data { from: 0, packet: p })]
        );
        let out = say(&mut air, 0, Up::Kick { mask: 1 });
        assert!(out.contains(&(1, Down::Dropped { reason: 0 })));
        assert!(out.contains(&(0, Down::Children { ids: [0; 4] })));
        // A closed room takes nobody.
        say(
            &mut air,
            0,
            Up::Host {
                id: 0x1001,
                open: false,
                max: 4,
                broadcast: [0; 6],
            },
        );
        let out = say(&mut air, 1, Up::Connect { id: 0x1001 });
        assert!(out.contains(&(1, Down::Refused { host: 0x1001 })));
        // Open again: seat 1 joins, then seat 0's player goes.
        say(&mut air, 0, host(0x1001));
        say(&mut air, 1, Up::Connect { id: 0x1001 });
        let mut out = Vec::new();
        air.gone(0, &mut |s, d| out.push((s, d)));
        assert!(out.contains(&(1, Down::Dropped { reason: 1 })));
        assert!(
            out.iter()
                .any(|(_, d)| matches!(d, Down::Hosts { n: 0, .. }))
        );
        // A child that leaves frees its slot.
        say(&mut air, 2, host(0x3001));
        say(&mut air, 3, Up::Connect { id: 0x3001 });
        let out = say(&mut air, 3, Up::Leave);
        assert!(out.contains(&(2, Down::Children { ids: [0; 4] })));
    }
}
