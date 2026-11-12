//! The wireless adapter, one per console, and the radio between them, in
//! one process (`docs/research/multiplayer.md`, "Wireless, later (option
//! F)"; multiplayer/wireless-adapter-local).
//!
//! The adapter sits on the link port in normal 32-bit mode. Its protocol
//! is librfu's "STWI" (pret's `librfu_stwi.c`, `librfu_intr.c`, the game
//! side) and afska's `gba-link-connection` docs (`wireless_adapter.md`,
//! the adapter side):
//!
//! - **Handshake.** After a reset (`RCNT` in GPIO mode pulls SD high), the
//!   GBA clocks words at 256 kHz and the two trade "NINTENDO" a halfword
//!   at a time, each echoing the other's last halfword inverted, until
//!   the adapter sends its ID, `0x8001`.
//! - **Commands.** At 2 MHz the GBA sends `0x9966LLCC` (`LL` words of
//!   data follow, `CC` the command), then clocks `0x80000000` and gets
//!   the acknowledgement `0x9966LL(CC|0x80)` and its `LL` words. Between
//!   words the two handshake on SI and SO: after a word, the adapter
//!   raises SI; the GBA raises SO; the adapter drops SI.
//! - **Waiting.** After `0x25`, `0x27`, `0x35` or `0x37` the clock turns
//!   round: the GBA arms an external-clock transfer and the adapter clocks
//!   an event to it when it has one (`0x28` data ready, `0x29`
//!   disconnected, `0x27` the wait timed out), then the GBA's
//!   acknowledgement, and the GBA is master again. Here SI follows SO.
//!
//! The radio is the hub: an adapter's broadcast, its room and its packets
//! reach the others at once. A host's `SendData` carries its packet to
//! every child and picks up what each child scheduled, as the adapter
//! does (afska: "data is only sent when the host calls SendData").
//!
//! A [`Wireless`] runs the machines in turns as [`super::Link`] does, each
//! up to a shared deadline a scanline ahead; a machine's turn also ends
//! at each `SIOCNT` write ([`crate::io::Io::sio_wireless`]), where the hub
//! answers the terminals and starts a transfer, and at the end of one in
//! flight. Each console's transfers are its own (the adapter is on its
//! port alone); only the radio is shared, and the order of turns is
//! fixed, so a run is deterministic. IDs are handed out in order from
//! `0x2001`, not at random. The hub's state isn't in the machines' save
//! states.

pub mod air;

use super::{MAX_PLAYERS, QUANTUM};
use crate::io::{Io, irq, reg};
use crate::machine::Machine;
use crate::platform::Platform;
use crate::sched::FRAME_CYCLES;
use air::{Down, FROM_HOST, MAX_HOSTS, Up};

/// `SIOCNT`'s bits in normal mode (GBATEK).
pub mod cnt {
    /// The GBA drives the clock.
    pub const INTERNAL: u16 = 1;
    /// 2 MHz, not 256 kHz, when internal.
    pub const FAST: u16 = 1 << 1;
    /// The SI terminal (read only).
    pub const SI: u16 = 1 << 2;
    /// SO while no transfer runs.
    pub const SO: u16 = 1 << 3;
    pub const START: u16 = 1 << 7;
    pub const MODE: u16 = 3 << 12;
    pub const NORMAL32: u16 = 1 << 12;
    pub const IRQ: u16 = 1 << 14;
}

/// `SIODATA32` (`SIOMULTI0`-`1`).
const SIODATA32: u32 = reg::SIOMULTI0;

/// A 32-bit transfer at 2 MHz and at 256 kHz.
pub const FAST_CYCLES: u64 = 32 * 8;
pub const SLOW_CYCLES: u64 = 32 * 64;
/// How long after the GBA arms a transfer the adapter starts clocking it
/// (afska: a GBA must wait at least 40 µs; the adapter answers at about
/// that).
pub const ARM_GAP: u64 = 700;

/// The adapter's version word (afska: "always returns 8585495").
pub const VERSION: u32 = 8_585_495;
/// The words the handshake trades, then the adapter's ID.
const NINTENDO: [u16; 5] = [0x494E, 0x544E, 0x4E45, 0x4F44, 0x8001];

/// A waiting adapter's events.
pub mod event {
    pub const TIMEOUT: u8 = 0x27;
    pub const DATA: u8 = 0x28;
    pub const DISCONNECTED: u8 = 0x29;
}

/// How long a searching adapter lists a host it no longer hears (afska:
/// "it disappears after 3 seconds"), in cycles.
pub const HEARD_FOR: u64 = 180 * FRAME_CYCLES;

/// The most words a command or acknowledgement carries here (a host's
/// `SendData` is 23, four broadcasts 28).
const WORDS: usize = 32;
/// The most bytes in a packet (a host's 87).
const PACKET: usize = 90;

/// Whether a machine's serial port is in normal 32-bit mode.
pub fn normal32(io: &Io) -> bool {
    io.raw16(reg::RCNT) & 0x8000 == 0 && io.raw16(reg::SIOCNT) & cnt::MODE == cnt::NORMAL32
}

fn siodata32(io: &Io) -> u32 {
    io.raw16(SIODATA32) as u32 | (io.raw16(SIODATA32 + 2) as u32) << 16
}

fn set_siodata32(io: &mut Io, v: u32) {
    io.set_raw16(SIODATA32, v as u16);
    io.set_raw16(SIODATA32 + 2, (v >> 16) as u16);
}

/// Up to [`WORDS`] words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Words {
    w: [u32; WORDS],
    n: usize,
}

impl Words {
    const fn new() -> Words {
        Words {
            w: [0; WORDS],
            n: 0,
        }
    }

    fn push(&mut self, v: u32) {
        if self.n < WORDS {
            self.w[self.n] = v;
            self.n += 1;
        }
    }

    fn get(&self) -> &[u32] {
        &self.w[..self.n]
    }
}

/// A packet's bytes, as the radio carries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    b: [u8; PACKET],
    n: usize,
}

impl Packet {
    const fn new() -> Packet {
        Packet {
            b: [0; PACKET],
            n: 0,
        }
    }

    /// The first `n` bytes of `words`, low byte first.
    fn from_words(words: &[u32], n: usize) -> Packet {
        let mut p = Packet::new();
        for (i, w) in words.iter().enumerate() {
            for k in 0..4 {
                if p.n < n.min(PACKET) && 4 * i + k < n {
                    p.b[p.n] = (w >> (8 * k)) as u8;
                    p.n += 1;
                }
            }
        }
        p
    }

    /// Up to [`PACKET`] bytes.
    pub fn of(bytes: &[u8]) -> Packet {
        let mut p = Packet::new();
        p.n = bytes.len().min(PACKET);
        p.b[..p.n].copy_from_slice(&bytes[..p.n]);
        p
    }

    pub fn bytes(&self) -> &[u8] {
        &self.b[..self.n]
    }
}

/// Packets that came in and wait for the adapter's slot to empty (a
/// networked adapter's: the network may bring two at once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Queue {
    p: [Packet; QUEUE],
    head: usize,
    len: usize,
}

/// The most packets a [`Queue`] holds; past it the oldest go.
const QUEUE: usize = 8;

impl Queue {
    const fn new() -> Queue {
        Queue {
            p: [Packet::new(); QUEUE],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, p: Packet) {
        if self.len == QUEUE {
            self.head = (self.head + 1) % QUEUE;
            self.len -= 1;
        }
        self.p[(self.head + self.len) % QUEUE] = p;
        self.len += 1;
    }

    fn pop(&mut self) -> Option<Packet> {
        (self.len > 0).then(|| {
            let p = self.p[self.head];
            self.head = (self.head + 1) % QUEUE;
            self.len -= 1;
            p
        })
    }
}

/// What a networked adapter ([`Wireless::networked`]) keeps of the radio
/// beyond what a local one does.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Net {
    seat: u8,
    /// Host IDs this adapter has used, counted.
    ids: u16,
    /// What it has to tell the radio, oldest first.
    out: [Up; OUTBOX],
    out_head: usize,
    out_len: usize,
    /// What it last told of its hosting: open, max children, broadcast.
    told: Option<(bool, u8, [u32; 6])>,
    /// The open hosts the radio last told of.
    hosts: Down,
    /// Hosts heard while searching, by ID: the entry and when.
    heard: [Option<([u32; 7], u64)>; HEARD],
    /// The host a pending `Connect` is for.
    target: u16,
    /// Each slot's child's ID (a host).
    kids: [u16; 4],
    from_host: Queue,
    from_child: [Queue; 4],
}

/// Messages a networked adapter holds for the radio between pumps; past
/// it the oldest go.
const OUTBOX: usize = 64;
/// Hosts a networked adapter remembers hearing.
const HEARD: usize = 8;

/// Where an adapter is in the radio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Idle,
    /// Hosting: children by slot (the adapter's index), and whether the
    /// room takes more.
    Host {
        open: bool,
        children: [Option<u8>; 4],
    },
    Searching,
    /// `Connect` sent to the host at this index (`None`: no such host).
    Connecting(Option<u8>),
    Child {
        host: u8,
        slot: u8,
    },
}

/// The adapter's side of the SPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spi {
    /// Waiting for a command's first word.
    Idle,
    /// Taking a command's data.
    Request { cmd: u8, len: usize },
    /// Sending the acknowledgement: `ack`'s words from `sent`.
    Ack { sent: usize },
    /// The clock is the adapter's: waiting for something to tell.
    Waiting { since: u64 },
    /// Clocking an event: `ack`'s words from `sent` (the last is
    /// `0x80000000`, for the GBA's acknowledgement).
    Event { sent: usize, at: u64 },
}

/// One adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Adapter {
    /// The handshake: how far the adapter is, and its next word; `None`
    /// in command mode.
    hello: Option<(usize, u32)>,
    /// SD went high: the next 256 kHz word starts a handshake.
    reset: bool,
    spi: Spi,
    /// The command's data, then the words to send.
    req: Words,
    ack: Words,
    /// The command whose acknowledgement turns the clock round.
    turn_after: bool,
    /// The next word the adapter sends.
    out: u32,
    /// An event waiting for the GBA's clock: its first word, and its
    /// parameter if any.
    event: Option<(u8, Option<u32>)>,
    /// `Setup`'s word and the broadcast.
    config: u32,
    broadcast: [u32; 6],
    id: u16,
    role: Role,
    /// Received and not yet read: from the host (a child), from each
    /// child by slot (a host).
    from_host: Packet,
    from_child: [Packet; 4],
    /// A child's packet, until the host next sends; a host's last.
    sending: Packet,
    /// The hosts this adapter heard while searching, by index: the
    /// `BroadcastRead` entry (metadata word, then the broadcast) and when.
    heard: [Option<([u32; 7], u64)>; MAX_PLAYERS],
}

impl Adapter {
    const fn new() -> Adapter {
        Adapter {
            hello: None,
            reset: true,
            spi: Spi::Idle,
            req: Words::new(),
            ack: Words::new(),
            turn_after: false,
            out: 0x8000_0000,
            event: None,
            config: 0,
            broadcast: [0; 6],
            id: 0,
            role: Role::Idle,
            from_host: Packet::new(),
            from_child: [Packet::new(); 4],
            sending: Packet::new(),
            heard: [None; MAX_PLAYERS],
        }
    }

    /// The children a host's room holds, by `Setup`'s bits 16-17.
    fn max_children(&self) -> usize {
        4 - (self.config >> 16 & 3) as usize
    }

    /// The slot the next child takes, or `0xFF`.
    fn next_slot(&self) -> u8 {
        match self.role {
            Role::Host {
                open: true,
                children,
            } => children[..self.max_children()]
                .iter()
                .position(|c| c.is_none())
                .map_or(0xFF, |s| s as u8),
            _ => 0xFF,
        }
    }

    fn waiting(&self) -> bool {
        matches!(self.spi, Spi::Waiting { .. })
    }
}

/// A console's port: the transfer in flight and the terminals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Port {
    /// When the transfer ends (hub time), the adapter's word, and whether
    /// the GBA drove the clock.
    flight: Option<(u64, u32, bool)>,
    /// When the GBA armed an external-clock transfer.
    armed: Option<u64>,
    si: bool,
    so: bool,
    /// Whether the GBA drove the clock of the last transfer.
    master: bool,
}

const PORT: Port = Port {
    flight: None,
    armed: None,
    si: false,
    so: false,
    master: true,
};

/// The radio and every console's adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wireless {
    players: usize,
    /// Hub time: every machine has reached it.
    now: u64,
    /// Each machine's clock at hub time 0.
    base: [u64; MAX_PLAYERS],
    ports: [Port; MAX_PLAYERS],
    adapters: [Adapter; MAX_PLAYERS],
    next_id: u16,
    /// 32-bit words exchanged, every console's.
    pub transfers: u64,
    /// Commands each adapter took, by command (a histogram, for logs).
    pub commands: [[u32; 64]; MAX_PLAYERS],
    /// How long a search keeps listing a host it no longer hears.
    heard_for: u64,
    /// The radio is elsewhere ([`Wireless::networked`]).
    net: Option<Net>,
}

impl Wireless {
    /// Put an adapter on each of `ms` (1 to [`MAX_PLAYERS`]), all in one
    /// radio, at hub time 0.
    pub fn new<P: Platform>(ms: &mut [&mut Machine<P>]) -> Wireless {
        assert!(
            (1..=MAX_PLAYERS).contains(&ms.len()),
            "a radio here has 1 to {MAX_PLAYERS} adapters"
        );
        let mut base = [0; MAX_PLAYERS];
        for (b, m) in base.iter_mut().zip(ms.iter_mut()) {
            *b = m.now();
            m.bus.io.sio_linked = true;
            m.bus.io.sio_wireless = true;
        }
        Wireless {
            players: ms.len(),
            now: 0,
            base,
            ports: [PORT; MAX_PLAYERS],
            adapters: [Adapter::new(); MAX_PLAYERS],
            next_id: 0x2001,
            transfers: 0,
            commands: [[0; 64]; MAX_PLAYERS],
            heard_for: HEARD_FOR,
            net: None,
        }
    }

    /// Put an adapter on `m`, the console of seat `seat` (below
    /// [`MAX_PLAYERS`]), whose radio is an [`air::Air`] elsewhere: what
    /// the adapter does comes out of [`Wireless::take_up`], what the radio
    /// says goes in through [`Wireless::deliver`]. Host IDs are
    /// `0x1000 * (seat + 1)` counting up, below the server's children's.
    /// A search lists only the hosts the radio last said were open (no
    /// [`HEARD_FOR`]): kept, a host's entry from before it reopened under
    /// a new ID sent the first `Connect` to the old one
    /// (docs/research/multiplayer.md, "The radio on the server").
    pub fn networked<P: Platform>(m: &mut Machine<P>, seat: u8) -> Wireless {
        let mut w = Wireless::new(&mut [m]);
        w.heard_for = 0;
        w.net = Some(Net {
            seat,
            ids: 0,
            out: [Up::Leave; OUTBOX],
            out_head: 0,
            out_len: 0,
            told: None,
            hosts: Down::Hosts {
                n: 0,
                hosts: [air::Heard {
                    id: 0,
                    slot: 0,
                    broadcast: [0; 6],
                }; MAX_HOSTS],
            },
            heard: [None; HEARD],
            target: 0,
            kids: [0; 4],
            from_host: Queue::new(),
            from_child: [Queue::new(); 4],
        });
        w
    }

    /// How long a search keeps listing a host it no longer hears
    /// ([`HEARD_FOR`], or 0 for a networked adapter, unless set; 0 lists
    /// only those heard now).
    pub fn set_heard_for(&mut self, cycles: u64) {
        self.heard_for = cycles;
    }

    /// The next thing a networked adapter has to tell the radio.
    pub fn take_up(&mut self) -> Option<Up> {
        let n = self.net.as_mut()?;
        (n.out_len > 0).then(|| {
            let m = n.out[n.out_head];
            n.out_head = (n.out_head + 1) % OUTBOX;
            n.out_len -= 1;
            m
        })
    }

    fn tell(&mut self, m: Up) {
        if let Some(n) = &mut self.net {
            if n.out_len == OUTBOX {
                n.out_head = (n.out_head + 1) % OUTBOX;
                n.out_len -= 1;
            }
            n.out[(n.out_head + n.out_len) % OUTBOX] = m;
            n.out_len += 1;
        }
    }

    /// What the radio says to a networked adapter, at the hub's time.
    pub fn deliver(&mut self, d: Down) {
        let t = self.now;
        let Some(n) = &mut self.net else { return };
        let a = &mut self.adapters[0];
        match d {
            Down::Hosts { n: k, hosts } => {
                n.hosts = d;
                for h in &hosts[..k as usize] {
                    remember(&mut n.heard, h.entry(), t);
                }
            }
            Down::Joined { host, id, slot } => {
                if a.role == Role::Connecting(Some(0)) && n.target == host {
                    a.id = id;
                    a.role = Role::Child { host: 0, slot };
                    a.from_host = Packet::new();
                    a.sending = Packet::new();
                    n.from_host = Queue::new();
                } else {
                    // Given up on before the radio answered.
                    self.tell(Up::Leave);
                }
            }
            Down::Refused { host } => {
                if a.role == Role::Connecting(Some(0)) && n.target == host {
                    a.role = Role::Connecting(None);
                }
            }
            Down::Children { ids } => {
                if let Role::Host { open, .. } = a.role {
                    let mut children = [None; 4];
                    for s in 0..4 {
                        if ids[s] != 0 {
                            children[s] = Some(s as u8);
                        } else {
                            a.from_child[s] = Packet::new();
                            n.from_child[s] = Queue::new();
                        }
                    }
                    n.kids = ids;
                    a.role = Role::Host { open, children };
                }
            }
            Down::Dropped { reason } => {
                if let Role::Child { slot, .. } = a.role {
                    n.from_host = Queue::new();
                    self.drop_child(0, slot as usize, reason as u32);
                }
            }
            Down::Data { from, packet } => match a.role {
                Role::Child { .. } if from == FROM_HOST => {
                    n.from_host.push(packet);
                    self.refill(0);
                }
                Role::Host { children, .. }
                    if (from as usize) < 4 && children[from as usize].is_some() =>
                {
                    n.from_child[from as usize].push(packet);
                    self.refill(0);
                }
                _ => {}
            },
        }
    }

    /// A networked adapter's empty receive slots take the next packet
    /// waiting; a waiting child hears there's one.
    fn refill(&mut self, i: usize) {
        let Some(n) = &mut self.net else { return };
        let a = &mut self.adapters[i];
        if a.from_host.n == 0
            && let Some(p) = n.from_host.pop()
        {
            a.from_host = p;
            if a.waiting() && a.event.is_none() {
                a.event = Some((event::DATA, None));
            }
        }
        for s in 0..4 {
            if a.from_child[s].n == 0
                && let Some(p) = n.from_child[s].pop()
            {
                a.from_child[s] = p;
            }
        }
    }

    /// Take the adapters off: the machines run on their own again.
    pub fn unplug<P: Platform>(self, ms: &mut [&mut Machine<P>]) {
        for m in ms.iter_mut() {
            m.bus.io.sio_linked = false;
            m.bus.io.sio_wireless = false;
            m.bus.io.sio_owned = 0;
            m.set_deadline(u64::MAX);
        }
    }

    /// Hub time: cycles since [`Wireless::new`].
    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn players(&self) -> usize {
        self.players
    }

    /// Adapter `i`'s ID in the radio (0 when it has none), and its state
    /// as `SystemStatus` gives it (0 idle, 1 and 2 hosting, 3 searching,
    /// 4 connecting, 5 a child).
    pub fn status(&self, i: usize) -> (u16, u8) {
        let a = &self.adapters[i];
        (a.id, state_byte(a))
    }

    /// Adapter `i`'s broadcast (`Broadcast`'s six words).
    pub fn broadcast(&self, i: usize) -> [u32; 6] {
        self.adapters[i].broadcast
    }

    /// Run every machine `cycles` further, in turns, as
    /// [`super::Link::run`] does.
    pub fn run<P: Platform>(
        &mut self,
        ms: &mut [&mut Machine<P>],
        cycles: u64,
        mut run: impl FnMut(usize, &mut Machine<P>),
    ) {
        assert_eq!(
            ms.len(),
            self.players,
            "the machines the radio was made with"
        );
        let end = self.now + cycles;
        while self.now < end {
            if ms.iter().any(|m| m.fault.is_some()) {
                return;
            }
            let d = (self.now + QUANTUM).min(end);
            for (i, m) in ms.iter_mut().enumerate() {
                self.turn(m, i, d, &mut run);
            }
            self.now = d;
        }
    }

    /// Run machine `i` until it reaches hub time `d`, stopping at each
    /// `SIOCNT` write and transfer end to answer it.
    fn turn<P: Platform>(
        &mut self,
        m: &mut Machine<P>,
        i: usize,
        d: u64,
        run: &mut impl FnMut(usize, &mut Machine<P>),
    ) {
        loop {
            let t = m.now() - self.base[i];
            self.service(i, &mut m.bus.io, t);
            if t >= d || m.fault.is_some() {
                break;
            }
            let mut stop = d;
            if let Some((end, ..)) = self.ports[i].flight {
                stop = stop.min(end);
            }
            if let Some(at) = self.clock_at(i) {
                stop = stop.min(at);
            }
            m.set_deadline(self.base[i] + stop.max(t + 1));
            run(i, m);
        }
        m.set_deadline(u64::MAX);
    }

    /// When adapter `i` next has something to do on its own: clock an
    /// event to an armed GBA, or time its wait out.
    fn clock_at(&self, i: usize) -> Option<u64> {
        let a = &self.adapters[i];
        let p = &self.ports[i];
        match a.spi {
            Spi::Event { at, .. } => p.armed.map(|t| at.max(t + ARM_GAP)),
            Spi::Waiting { since } if a.event.is_none() => {
                let frames = (a.config & 0xFF) as u64;
                (frames != 0).then(|| since + frames * FRAME_CYCLES)
            }
            _ => None,
        }
    }

    /// Bring console `i`'s port up to hub time `t`: end a transfer that's
    /// due, answer the terminals the CPU set, start a transfer.
    fn service(&mut self, i: usize, io: &mut Io, t: u64) {
        if let Some((end, word, internal)) = self.ports[i].flight
            && end <= t
        {
            self.ports[i].flight = None;
            set_siodata32(io, word);
            let v = io.raw16(reg::SIOCNT) & !cnt::START;
            io.set_raw16(reg::SIOCNT, v);
            if v & cnt::IRQ != 0 {
                io.request_irq(irq::SERIAL);
            }
            let p = &mut self.ports[i];
            p.master = internal;
            p.si = internal;
            p.armed = None;
            self.transfers += 1;
        }
        if io.raw16(reg::RCNT) & 0x8000 != 0 {
            // GPIO mode: the reset line.
            self.adapters[i].reset = true;
        }
        if !normal32(io) {
            self.ports[i].flight = None;
            self.ports[i].armed = None;
            io.sio_owned = 0;
            return;
        }
        // The adapter's clock: time a wait out, or turn an event into
        // words once its time comes.
        if let Spi::Waiting { since } = self.adapters[i].spi {
            let frames = (self.adapters[i].config & 0xFF) as u64;
            if self.adapters[i].event.is_none() && frames != 0 && t >= since + frames * FRAME_CYCLES
            {
                self.adapters[i].event = Some((event::TIMEOUT, None));
            }
            if let Some((ev, param)) = self.adapters[i].event.take() {
                let a = &mut self.adapters[i];
                a.ack = Words::new();
                a.ack
                    .push(0x9966_0000 | (param.is_some() as u32) << 8 | ev as u32);
                if let Some(p) = param {
                    a.ack.push(p);
                }
                a.ack.push(0x8000_0000);
                a.spi = Spi::Event { sent: 0, at: t };
            }
        }
        let v = io.raw16(reg::SIOCNT);
        let so = v & cnt::SO != 0;
        let p = &mut self.ports[i];
        if so != p.so {
            p.so = so;
            p.si = so && !p.master;
        }
        if p.flight.is_none() && v & cnt::START != 0 {
            if v & cnt::INTERNAL != 0 {
                let fast = v & cnt::FAST != 0;
                let word = self.exchange(i, siodata32(io), fast, t);
                let len = if fast { FAST_CYCLES } else { SLOW_CYCLES };
                self.ports[i].flight = Some((t + len, word, true));
            } else if !so {
                let p = &mut self.ports[i];
                p.armed.get_or_insert(t);
                if let Some(at) = self.clock_at(i)
                    && at <= t
                    && let Spi::Event { sent, .. } = self.adapters[i].spi
                {
                    let a = &mut self.adapters[i];
                    let word = a.ack.w[sent];
                    if sent + 1 < a.ack.n {
                        a.spi = Spi::Event {
                            sent: sent + 1,
                            at: t,
                        };
                    } else {
                        // The GBA's acknowledgement: the clock is its own again.
                        a.spi = Spi::Idle;
                        a.out = 0x8000_0000;
                    }
                    self.ports[i].flight = Some((t + FAST_CYCLES, word, false));
                }
            }
        } else if v & cnt::START == 0 {
            self.ports[i].armed = None;
        }
        let p = &self.ports[i];
        let v = io.raw16(reg::SIOCNT) & !cnt::SI | if p.si { cnt::SI } else { 0 };
        io.set_raw16(reg::SIOCNT, v);
        io.sio_owned = cnt::SI | if p.flight.is_some() { cnt::START } else { 0 };
    }

    /// A word the GBA clocks: adapter `i` takes `gba` and gives its own.
    fn exchange(&mut self, i: usize, gba: u32, fast: bool, t: u64) -> u32 {
        if !fast {
            let a = &mut self.adapters[i];
            if a.reset || a.hello.is_none() {
                self.power_on(i);
            }
            let a = &mut self.adapters[i];
            let (mut step, out) = a.hello.unwrap_or((0, 0));
            if step + 1 < NINTENDO.len() && (gba >> 16) as u16 == !NINTENDO[step] {
                step += 1;
            }
            let next = (NINTENDO[step] as u32) << 16 | (!gba & 0xFFFF);
            a.hello = Some((step, next));
            return out;
        }
        let a = &mut self.adapters[i];
        a.hello = None;
        let out = a.out;
        match a.spi {
            Spi::Idle | Spi::Waiting { .. } | Spi::Event { .. } => {
                a.spi = Spi::Idle;
                if gba >> 16 == 0x9966 {
                    let (cmd, len) = (gba as u8, (gba >> 8 & 0xFF) as usize);
                    a.req = Words::new();
                    if len == 0 {
                        self.execute(i, cmd, t);
                    } else {
                        a.spi = Spi::Request { cmd, len };
                    }
                }
            }
            Spi::Request { cmd, len } => {
                a.req.push(gba);
                if a.req.n >= len {
                    self.execute(i, cmd, t);
                }
            }
            Spi::Ack { sent } => {
                if sent + 1 < a.ack.n {
                    a.spi = Spi::Ack { sent: sent + 1 };
                    a.out = a.ack.w[sent + 1];
                } else {
                    a.out = 0x8000_0000;
                    a.spi = if a.turn_after {
                        Spi::Waiting { since: t }
                    } else {
                        Spi::Idle
                    };
                    a.turn_after = false;
                    if a.waiting()
                        && a.event.is_none()
                        && matches!(a.role, Role::Child { .. })
                        && a.from_host.n > 0
                    {
                        // The host's packet came while the clock was the GBA's.
                        a.event = Some((event::DATA, None));
                    }
                }
            }
        }
        out
    }

    /// Adapter `i` powers on (SD went high): out of the radio, every
    /// setting back to its default.
    fn power_on(&mut self, i: usize) {
        self.leave(i, 1);
        self.adapters[i] = Adapter {
            reset: false,
            hello: Some((0, 0)),
            ..Adapter::new()
        };
    }

    fn new_id(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Adapter `i` leaves the radio: a host drops its children, a child
    /// its host. A child that loses its host hears `reason` (0 sent away,
    /// 1 the link lost).
    fn leave(&mut self, i: usize, reason: u32) {
        if let Some(n) = &mut self.net {
            // The radio drops the children, or tells the host.
            n.told = None;
            n.kids = [0; 4];
            n.from_host = Queue::new();
            n.from_child = [Queue::new(); 4];
            let a = &mut self.adapters[i];
            let was = core::mem::replace(&mut a.role, Role::Idle);
            a.from_host = Packet::new();
            a.from_child = [Packet::new(); 4];
            if matches!(
                was,
                Role::Host { .. } | Role::Child { .. } | Role::Connecting(Some(_))
            ) {
                self.tell(Up::Leave);
            }
            return;
        }
        match self.adapters[i].role {
            Role::Host { children, .. } => {
                for (s, c) in children.iter().enumerate() {
                    if let Some(c) = *c {
                        self.drop_child(c as usize, s, reason);
                    }
                }
            }
            Role::Child { host, slot } => {
                if let Role::Host { children, .. } = &mut self.adapters[host as usize].role {
                    children[slot as usize] = None;
                }
                self.adapters[host as usize].from_child[slot as usize] = Packet::new();
            }
            _ => {}
        }
        self.adapters[i].role = Role::Idle;
    }

    /// The child at index `c` (slot `slot`) is out of its host's room.
    fn drop_child(&mut self, c: usize, slot: usize, reason: u32) {
        let a = &mut self.adapters[c];
        a.role = Role::Idle;
        a.from_host = Packet::new();
        a.sending = Packet::new();
        if a.waiting() {
            a.event = Some((event::DISCONNECTED, Some(1 << slot | reason << 8)));
        }
    }

    /// Adapter `i` takes command `cmd` (its data in `req`) at hub time
    /// `t`, and gets its acknowledgement ready.
    fn execute(&mut self, i: usize, cmd: u8, t: u64) {
        self.commands[i][cmd as usize & 63] += 1;
        let req = self.adapters[i].req;
        let args = req.get();
        let arg = |k: usize| args.get(k).copied().unwrap_or(0);
        let mut ack = Words::new();
        let mut ok = true;
        let mut turn = false;
        match cmd {
            // Reset, StopMode (Bye).
            0x10 | 0x3D => {
                self.leave(i, 1);
                let a = &mut self.adapters[i];
                *a = Adapter {
                    reset: false,
                    hello: None,
                    ..Adapter::new()
                };
            }
            // SignalLevel: a byte per slot.
            0x11 => {
                let mut v = 0;
                match self.adapters[i].role {
                    Role::Host { children, .. } => {
                        for (s, c) in children.iter().enumerate() {
                            if c.is_some() {
                                v |= 0xFF << (8 * s);
                            }
                        }
                    }
                    Role::Child { slot, .. } => v = 0xFF << (8 * slot),
                    _ => {}
                }
                ack.push(v);
            }
            0x12 => ack.push(VERSION),
            0x13 => {
                let a = &self.adapters[i];
                let slots = match a.role {
                    Role::Child { slot, .. } => 1 << slot,
                    _ => 0,
                };
                ack.push(a.id as u32 | slots << 16 | (state_byte(a) as u32) << 24);
            }
            // SlotStatus.
            0x14 => {
                let a = &self.adapters[i];
                ack.push(a.next_slot() as u32);
                self.children(i, &mut ack);
            }
            // ConfigStatus.
            0x15 => {
                let a = &self.adapters[i];
                if matches!(a.role, Role::Host { .. }) {
                    for w in a.broadcast {
                        ack.push(w);
                    }
                    ack.push(a.config);
                } else {
                    for _ in 0..6 {
                        ack.push(0);
                    }
                }
                ack.push(257);
            }
            // Broadcast, Setup.
            0x16 => {
                for (k, w) in self.adapters[i].broadcast.iter_mut().enumerate() {
                    *w = arg(k);
                }
            }
            0x17 => self.adapters[i].config = arg(0),
            // StartHost.
            0x19 => match self.adapters[i].role {
                Role::Host { children, .. } => {
                    self.adapters[i].role = Role::Host {
                        open: true,
                        children,
                    }
                }
                _ => {
                    self.leave(i, 1);
                    let id = match &mut self.net {
                        Some(n) => {
                            n.ids = (n.ids + 1) & 0xFFF;
                            0x1000 * (n.seat as u16 + 1) + n.ids
                        }
                        None => self.new_id(),
                    };
                    let a = &mut self.adapters[i];
                    a.id = id;
                    a.role = Role::Host {
                        open: true,
                        children: [None; 4],
                    };
                }
            },
            // PollConnections, EndHost.
            0x1A | 0x1B => {
                if let Role::Host { children, .. } = self.adapters[i].role {
                    self.children(i, &mut ack);
                    if cmd == 0x1B {
                        self.adapters[i].role = if children.iter().any(Option::is_some) {
                            Role::Host {
                                open: false,
                                children,
                            }
                        } else {
                            Role::Idle
                        };
                    }
                } else if cmd == 0x1A {
                    ok = false;
                }
            }
            // BroadcastRead: start, poll, end.
            0x1C => {
                self.leave(i, 1);
                self.adapters[i].role = Role::Searching;
            }
            0x1D | 0x1E => {
                if self.adapters[i].role != Role::Searching {
                    ok = false;
                } else if let Some(n) = &mut self.net {
                    if let Down::Hosts { n: k, hosts } = n.hosts {
                        for h in &hosts[..k as usize] {
                            remember(&mut n.heard, h.entry(), t);
                        }
                    }
                    let mut found = 0;
                    let own = n.seat as u32 + 1;
                    for (e, when) in n.heard.iter().flatten() {
                        // Not this adapter's own room, while the radio
                        // hasn't heard it's gone.
                        if found < 4 && t - when <= self.heard_for && e[0] >> 12 & 0xF != own {
                            for &w in e {
                                ack.push(w);
                            }
                            found += 1;
                        }
                    }
                    if cmd == 0x1E {
                        self.adapters[i].role = Role::Idle;
                    }
                } else {
                    let mut found = 0;
                    for j in 0..self.players {
                        let h = &self.adapters[j];
                        if j != i && matches!(h.role, Role::Host { open: true, .. }) {
                            let mut e = [h.id as u32 | (h.next_slot() as u32) << 16; 7];
                            e[1..].copy_from_slice(&h.broadcast);
                            self.adapters[i].heard[j] = Some((e, t));
                        }
                        match self.adapters[i].heard[j] {
                            Some((e, when)) if found < 4 && t - when <= self.heard_for => {
                                for w in e {
                                    ack.push(w);
                                }
                                found += 1;
                            }
                            _ => {}
                        }
                    }
                    if cmd == 0x1E {
                        self.adapters[i].role = Role::Idle;
                    }
                }
            }
            // Connect: to the host with this ID.
            0x1F => {
                let id = arg(0) as u16;
                self.leave(i, 1);
                if let Some(n) = &mut self.net {
                    // Pending until the radio answers.
                    n.target = id;
                    self.adapters[i].role = Role::Connecting(Some(0));
                    self.tell(Up::Connect { id });
                    self.finish(i, cmd, ack, ok, turn);
                    return;
                }
                let host = (0..self.players).find(|&j| {
                    j != i
                        && self.adapters[j].id == id
                        && matches!(self.adapters[j].role, Role::Host { .. })
                });
                self.adapters[i].role = Role::Connecting(host.map(|j| j as u8));
            }
            // IsConnectionComplete, FinishConnection.
            0x20 | 0x21 => match self.adapters[i].role {
                Role::Connecting(Some(_)) if self.net.is_some() => {
                    // Still waiting for the radio: status 1, or given up.
                    if cmd == 0x21 {
                        self.leave(i, 1);
                        ack.push(self.adapters[i].id as u32 | 4 << 16 | 2 << 24);
                    } else {
                        ack.push(self.adapters[i].id as u32 | 1 << 24);
                    }
                }
                Role::Connecting(host) => {
                    let slot = host.map_or(0xFF, |h| self.adapters[h as usize].next_slot());
                    match host {
                        Some(h) if slot != 0xFF => {
                            let id = self.new_id();
                            if let Role::Host { children, .. } = &mut self.adapters[h as usize].role
                            {
                                children[slot as usize] = Some(i as u8);
                            }
                            let h = h as usize;
                            self.adapters[h].from_child[slot as usize] = Packet::new();
                            let a = &mut self.adapters[i];
                            a.id = id;
                            a.role = Role::Child {
                                host: h as u8,
                                slot,
                            };
                            a.from_host = Packet::new();
                            a.sending = Packet::new();
                            ack.push(id as u32 | (slot as u32) << 16);
                        }
                        // Failed: a slot number past 3.
                        _ => {
                            ack.push(self.adapters[i].id as u32 | 4 << 16 | 2 << 24);
                            if cmd == 0x21 {
                                self.adapters[i].role = Role::Idle;
                            }
                        }
                    }
                }
                Role::Child { slot, .. } => {
                    ack.push(self.adapters[i].id as u32 | (slot as u32) << 16);
                }
                _ => ok = false,
            },
            // SendData, SendDataWait.
            0x24 | 0x25 => {
                self.send(i, args, t);
                turn = cmd == 0x25;
            }
            // ReceiveData.
            0x26 => self.receive(i, &mut ack),
            // Wait.
            0x27 | 0x35 => turn = true,
            // DisconnectClient.
            0x30 => {
                let mask = arg(0);
                match self.adapters[i].role {
                    Role::Host { children, open } if self.net.is_some() => {
                        let mut left = children;
                        for (s, c) in left.iter_mut().enumerate() {
                            if mask >> s & 1 != 0 {
                                *c = None;
                                self.adapters[i].from_child[s] = Packet::new();
                            }
                        }
                        self.adapters[i].role = Role::Host {
                            open,
                            children: left,
                        };
                        if let Some(n) = &mut self.net {
                            for s in 0..4 {
                                if mask >> s & 1 != 0 {
                                    n.kids[s] = 0;
                                    n.from_child[s] = Queue::new();
                                }
                            }
                        }
                        self.tell(Up::Kick { mask: mask as u8 });
                    }
                    Role::Host { children, .. } => {
                        for (s, c) in children.iter().enumerate() {
                            if mask >> s & 1 != 0
                                && let Some(c) = *c
                            {
                                if let Role::Host { children, .. } = &mut self.adapters[i].role {
                                    children[s] = None;
                                }
                                self.adapters[i].from_child[s] = Packet::new();
                                self.drop_child(c as usize, s, 0);
                            }
                        }
                    }
                    Role::Child { slot, .. } if mask >> slot & 1 != 0 => self.leave(i, 0),
                    _ => {}
                }
            }
            // RetransmitAndWait: the host's last packet again.
            0x37 => {
                if let Role::Host { .. } = self.adapters[i].role {
                    let p = self.adapters[i].sending;
                    self.host_send(i, p, t);
                }
                turn = true;
            }
            // The rest librfu knows (test mode, reconnection, 0x36):
            // accepted, with nothing to say.
            0x18 | 0x31..=0x34 | 0x36 | 0x38 | 0x39 => {}
            _ => {
                ok = false;
                ack.push(2);
            }
        }
        self.finish(i, cmd, ack, ok, turn);
    }

    /// Command `cmd`'s acknowledgement: `ack`'s words, or an error; then
    /// the clock turns round if `turn`. A networked host tells the radio
    /// what changed of its room.
    fn finish(&mut self, i: usize, cmd: u8, ack: Words, ok: bool, turn: bool) {
        if let Some(n) = &mut self.net {
            let a = &self.adapters[i];
            let now = match a.role {
                Role::Host { open, .. } => Some((open, a.max_children() as u8, a.broadcast)),
                _ => None,
            };
            if now != n.told {
                n.told = now;
                let id = a.id;
                self.tell(match now {
                    Some((open, max, broadcast)) => Up::Host {
                        id,
                        open,
                        max,
                        broadcast,
                    },
                    None => Up::Leave,
                });
            }
        }
        let a = &mut self.adapters[i];
        a.ack = Words::new();
        if ok {
            a.ack
                .push(0x9966_0000 | (ack.n as u32) << 8 | (cmd as u32 | 0x80));
            for &w in ack.get() {
                a.ack.push(w);
            }
        } else {
            a.ack.push(0x9966_01EE);
            a.ack.push(if ack.n > 0 { 2 } else { 1 });
        }
        a.out = a.ack.w[0];
        a.spi = Spi::Ack { sent: 0 };
        a.turn_after = ok && turn;
        if a.turn_after && matches!(a.role, Role::Host { .. }) && cmd != 0x27 && cmd != 0x35 {
            // A host hears at once that its packet went out.
            a.event = Some((event::DATA, None));
        }
    }

    /// A host's children, a word each: ID, slot in bits 16-23.
    fn children(&self, i: usize, ack: &mut Words) {
        if let Role::Host { children, .. } = self.adapters[i].role {
            for (s, c) in children.iter().enumerate() {
                if let Some(c) = *c {
                    let id = match &self.net {
                        Some(n) => n.kids[s],
                        None => self.adapters[c as usize].id,
                    };
                    ack.push(id as u32 | (s as u32) << 16);
                }
            }
        }
    }

    /// `SendData`: a host's packet goes out now; a child's waits for the
    /// host's next.
    fn send(&mut self, i: usize, args: &[u32], t: u64) {
        let Some((&header, data)) = args.split_first() else {
            return;
        };
        match self.adapters[i].role {
            Role::Host { .. } => {
                let n = (header & 0x7F) as usize;
                let p = if data.is_empty() {
                    // A "ghost send": the last bytes again.
                    let mut p = self.adapters[i].sending;
                    p.n = p.n.min(n);
                    p
                } else {
                    Packet::from_words(data, n)
                };
                self.adapters[i].sending = p;
                self.host_send(i, p, t);
            }
            Role::Child { slot, .. } => {
                let n = (header >> (8 + 5 * slot as u32) & 0x1F) as usize;
                let p = Packet::from_words(data, n);
                if self.net.is_some() {
                    // At once: a child sending twice before the host's next
                    // packet came would otherwise lose the first, and the
                    // game numbers them (link_rfu_2.c's childRecvIds).
                    if p.n > 0 {
                        self.tell(Up::Data(p));
                    }
                } else {
                    self.adapters[i].sending = p;
                }
            }
            _ => {}
        }
    }

    /// Host `i`'s packet reaches every child, and each child's scheduled
    /// packet reaches it.
    fn host_send(&mut self, i: usize, p: Packet, _t: u64) {
        let Role::Host { children, .. } = self.adapters[i].role else {
            return;
        };
        if self.net.is_some() {
            self.tell(Up::Data(p));
            return;
        }
        for (s, c) in children.iter().enumerate() {
            let Some(c) = *c else { continue };
            let c = &mut self.adapters[c as usize];
            c.from_host = p;
            if c.waiting() && c.event.is_none() {
                c.event = Some((event::DATA, None));
            }
            let sent = core::mem::replace(&mut c.sending, Packet::new());
            if sent.n > 0 {
                self.adapters[i].from_child[s] = sent;
            }
        }
    }

    /// `ReceiveData`: what came in since the last, behind a word of sizes.
    fn receive(&mut self, i: usize, ack: &mut Words) {
        let a = &mut self.adapters[i];
        let mut header = 0;
        let mut bytes = [0u8; 4 * (WORDS - 1)];
        let mut n = 0;
        let mut put = |p: &Packet| {
            for &b in p.bytes() {
                if n < bytes.len() {
                    bytes[n] = b;
                    n += 1;
                }
            }
        };
        match a.role {
            Role::Host { .. } => {
                for s in 0..4 {
                    let p = core::mem::replace(&mut a.from_child[s], Packet::new());
                    header |= (p.n as u32) << (8 + 5 * s);
                    put(&p);
                }
            }
            Role::Child { .. } => {
                let p = core::mem::replace(&mut a.from_host, Packet::new());
                header = p.n as u32;
                put(&p);
            }
            _ => {}
        }
        self.refill(i);
        if n == 0 {
            return;
        }
        ack.push(header);
        for w in bytes[..n].chunks(4) {
            ack.push(w.iter().rev().fold(0, |v, &b| v << 8 | b as u32));
        }
    }
}

/// Note host `entry` as heard at `t`, in place of the last from the
/// same adapter (a host ID's top four bits are its seat's,
/// [`Wireless::networked`]: a host reopening takes a new ID, and the old
/// one is gone), or of the one heard longest ago.
fn remember(heard: &mut [Option<([u32; 7], u64)>; HEARD], entry: [u32; 7], t: u64) {
    let seat = entry[0] >> 12 & 0xF;
    let k = heard
        .iter()
        .position(|h| h.is_some_and(|(e, _)| e[0] >> 12 & 0xF == seat))
        .or_else(|| heard.iter().position(Option::is_none))
        .unwrap_or_else(|| {
            (0..HEARD)
                .min_by_key(|&k| heard[k].map_or(0, |(_, w)| w))
                .unwrap_or(0)
        });
    heard[k] = Some((entry, t));
}

/// `SystemStatus`'s state byte.
fn state_byte(a: &Adapter) -> u8 {
    match a.role {
        Role::Idle => 0,
        Role::Host { open: false, .. } => 1,
        Role::Host { open: true, .. } => 2,
        Role::Searching => 3,
        Role::Connecting(_) => 4,
        Role::Child { .. } => 5,
    }
}

#[cfg(test)]
mod tests;
