//! `--rogue`: Emerald Rogue's co-op ("Multiplayer Adventures") without
//! Rogue Assistant: its other end, built in (docs/research/multiplayer.md,
//! "Rogue Assistant").
//!
//! Rogue's co-op isn't link-cable play. Each player runs their own game;
//! the game keeps a multiplayer blob in RAM (request and current state, a
//! handshake, the host's game state, player profiles, each player's
//! state), and Rogue Assistant copies parts of it between the host's game
//! and its clients over ENet (UDP, port 30025), reading and writing guest
//! memory once a frame. This does the same, the way its
//! `MultiplayerBehaviour.cpp` does (compat version 3), over rusty_enet, a
//! Rust transpile of the same ENet: a gbaa player and an mGBA player with
//! Rogue Assistant can play together.

use rusty_enet as enet;
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

/// Rogue Assistant's default port.
pub const DEFAULT_PORT: u16 = 30025;
/// The compat version this end speaks (the assistant's
/// `ROGUE_ASSISTANT_COMPAT_VERSION`); a ROM with another refuses.
pub const COMPAT_VERSION: u32 = 3;

/// The ROM offset of the GF header's three Rogue words: a handshake, the
/// pointer to the `RogueAssistantHeader`, another handshake.
const GF_ROGUE: usize = 0x178;
const HANDSHAKE_1: u32 = 20012;
const HANDSHAKE_2: u32 = 30035;

// The game's `netRequestState` and `netCurrentState` bits.
const NET_STATE_HOST: u8 = 2;
// `pendingHandshake.state`.
const HANDSHAKE_SEND_TO_HOST: u8 = 1;
const HANDSHAKE_SEND_TO_CLIENT: u8 = 2;

// The assistant's ENet channels.
const CH_HANDSHAKE: u8 = 0;
const CH_GAME_STATE: u8 = 1;
const CH_PLAYER_STATE: u8 = 2;
const CH_PLAYER_PROFILES: u8 = 3;
const CHANNELS: usize = 4;

/// Frames between the host's game-state sends (the assistant's 5 a
/// second) and between player-state sends (30 a second).
const GAME_STATE_EVERY: u64 = 12;
const PLAYER_STATE_EVERY: u64 = 2;
/// Frames a client waits for the host to answer before trying again (the
/// assistant blocks 5 s).
const CONNECT_FRAMES: u64 = 300;

/// Guest memory, as the assistant reaches it through mGBA's scripting.
pub trait Guest {
    fn read8(&self, addr: u32) -> u8;
    fn write8(&mut self, addr: u32, v: u8);

    fn read32(&self, addr: u32) -> u32 {
        u32::from_le_bytes(std::array::from_fn(|i| self.read8(addr + i as u32)))
    }
    fn read(&self, addr: u32, out: &mut [u8]) {
        for (i, b) in out.iter_mut().enumerate() {
            *b = self.read8(addr + i as u32);
        }
    }
    fn write(&mut self, addr: u32, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            self.write8(addr + i as u32, b);
        }
    }
}

impl Guest for runtime::bus::Bus {
    fn read8(&self, addr: u32) -> u8 {
        runtime::bus::Bus::read8(self, addr)
    }
    fn write8(&mut self, addr: u32, v: u8) {
        runtime::bus::Bus::write8(self, addr, v)
    }
}

/// The `RogueAssistantHeader` fields multiplayer uses: offsets and sizes
/// into the multiplayer blob, and where the game keeps the blob's pointer
/// and the assistant's "connected" word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub confirm_size: u32,
    pub confirm_offset: u32,
    pub size: u32,
    pub handshake: u32,
    pub handshake_size: u32,
    pub handshake_state: u32,
    pub handshake_player: u32,
    pub game_state: u32,
    pub game_state_size: u32,
    pub profiles: u32,
    pub profile_size: u32,
    pub players: u32,
    pub player_size: u32,
    pub player_count: u32,
    pub request: u32,
    pub current: u32,
    /// Where `gRogueAssistantState` lives (the confirm word is in it).
    pub assistant_state: u32,
    /// Where the pointer to the multiplayer blob lives (null with no
    /// session).
    pub multiplayer_ptr: u32,
}

impl Layout {
    /// Read from Rogue's ROM: the GF header's handshakes, then the header
    /// they point at, which must be of compat version 3 and lay the blob
    /// out within itself.
    pub fn from_rom(rom: &[u8]) -> Result<Layout, String> {
        let word = |o: usize| {
            rom.get(o..o + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or("ROM too short for Rogue's header")
        };
        if word(GF_ROGUE)? != HANDSHAKE_1 || word(GF_ROGUE + 8)? != HANDSHAKE_2 {
            return Err("not Emerald Rogue: no Rogue Assistant header".into());
        }
        let h = word(GF_ROGUE + 4)?.wrapping_sub(0x0800_0000) as usize;
        // Two bytes (version, debug), two of padding, then words.
        let f = |i: usize| word(h + 4 + 4 * i);
        let compat = f(0)?;
        if compat != COMPAT_VERSION {
            return Err(format!(
                "Rogue Assistant compat version {compat}, this speaks {COMPAT_VERSION}"
            ));
        }
        let l = Layout {
            confirm_size: f(1)?,
            confirm_offset: f(2)?,
            size: f(3)?,
            handshake: f(4)?,
            handshake_size: f(5)?,
            handshake_state: f(6)?,
            handshake_player: f(7)?,
            game_state: f(8)?,
            game_state_size: f(9)?,
            profiles: f(10)?,
            profile_size: f(11)?,
            players: f(12)?,
            player_size: f(13)?,
            player_count: f(14)?,
            request: f(15)?,
            current: f(16)?,
            assistant_state: f(30)?,
            multiplayer_ptr: f(31)?,
        };
        l.check()?;
        Ok(l)
    }

    /// Every span inside the blob, as the assistant's
    /// `ValidateMultiplayerLayout` has it.
    fn check(&self) -> Result<(), String> {
        let spans = [
            ("request", self.request, 1),
            ("current", self.current, 1),
            ("handshake", self.handshake, self.handshake_size),
            ("game state", self.game_state, self.game_state_size),
            (
                "profiles",
                self.profiles,
                self.profile_size * self.player_count,
            ),
            (
                "players",
                self.players,
                self.player_size * self.player_count,
            ),
        ];
        for (name, at, n) in spans {
            if at > self.size || n > self.size - at {
                return Err(format!("Rogue's {name} lies outside its blob"));
            }
        }
        if self.player_count == 0
            || self.handshake_state >= self.handshake_size
            || self.handshake_player >= self.handshake_size
            || self.confirm_size > 4
        {
            return Err("Rogue's multiplayer header doesn't add up".into());
        }
        Ok(())
    }

    fn player_id_ok(&self, id: u8) -> bool {
        id != 0 && (id as u32) < self.player_count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conn {
    /// A client waiting for ENet's connect, since frame N.
    Connecting(u64),
    AwaitingHandshake,
    AwaitingResponse,
    Confirmed,
    Connected,
}

/// An open session: the game's request it answers, and the ENet host.
struct Session {
    request: u8,
    net: enet::Host<UdpSocket>,
    /// A client's link to the host.
    host_peer: Option<enet::PeerID>,
    conn: Conn,
    /// The client's seat, from the host's answer; the host's is 0.
    player_id: u8,
    /// The host: the client whose handshake the game is answering.
    pending: Option<enet::PeerID>,
    /// The host: each client's seat.
    seats: HashMap<enet::PeerID, u8>,
    /// The host: the profiles last sent.
    profiles: Vec<u8>,
}

/// The assistant's multiplayer side, run once a frame.
pub struct Assistant {
    layout: Layout,
    /// Where a client connects; a host listens on its port.
    addr: SocketAddr,
    session: Option<Session>,
    frame: u64,
    /// What it did, for the terminal (taken by [`Assistant::take_log`]).
    log: Vec<String>,
}

impl Assistant {
    /// For the game in `rom`, connecting to (or, as host, listening on the
    /// port of) `addr`, `host[:port]`, the port 30025 if left out.
    pub fn new(rom: &[u8], addr: &str) -> Result<Assistant, String> {
        let layout = Layout::from_rom(rom)?;
        Ok(Assistant::with_layout(layout, parse_addr(addr)?))
    }

    pub fn with_layout(layout: Layout, addr: SocketAddr) -> Assistant {
        Assistant {
            layout,
            addr,
            session: None,
            frame: 0,
            log: Vec::new(),
        }
    }

    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// Whether a session is open and past its handshake.
    pub fn connected(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.conn == Conn::Connected)
    }

    /// The frame's work, after the guest's frame: tell the game an
    /// assistant is there, open or close the session the game asks for,
    /// and swap the blob's parts with the other players.
    pub fn frame(&mut self, g: &mut impl Guest) {
        self.frame += 1;
        let l = self.layout.clone();
        // The game counts frames in the confirm word; the assistant keeps
        // zeroing it, and the game takes a stale count for "gone".
        let confirm = l.assistant_state + l.confirm_offset;
        g.write(confirm, &[0; 4][..l.confirm_size as usize]);
        let ptr = g.read32(l.multiplayer_ptr);
        if ptr == 0 {
            if self.session.take().is_some() {
                self.log.push("session closed by the game".into());
            }
            return;
        }
        let mut blob = vec![0; l.size as usize];
        g.read(ptr, &mut blob);
        let request = blob[l.request as usize];
        match &self.session {
            None if request != 0 => self.open(request),
            None => {}
            Some(s) if s.request != request => {
                // The game changed its mind: start again next frame.
                self.session = None;
                self.log
                    .push("session closed: the game's request changed".into());
            }
            Some(_) => self.update(g, ptr, &blob),
        }
    }

    fn open(&mut self, request: u8) {
        let l = &self.layout;
        let host = request & NET_STATE_HOST != 0;
        let bind: SocketAddr = match host {
            true => ([0, 0, 0, 0], self.addr.port()).into(),
            false if self.addr.is_ipv6() => "[::]:0".parse().expect("an address"),
            false => ([0, 0, 0, 0], 0).into(),
        };
        let socket = match UdpSocket::bind(bind) {
            Ok(s) => s,
            Err(e) => {
                // Retried next frame: say so once.
                if self.frame % 300 == 1 {
                    self.log.push(format!("can't open UDP {bind}: {e}"));
                }
                return;
            }
        };
        let settings = enet::HostSettings {
            peer_limit: match host {
                true => (l.player_count as usize - 1).max(1),
                false => 1,
            },
            channel_limit: CHANNELS,
            ..Default::default()
        };
        let mut net = match enet::Host::new(socket, settings) {
            Ok(n) => n,
            Err(e) => {
                self.log.push(format!("ENet: {e:?}"));
                return;
            }
        };
        let (host_peer, conn) = match host {
            true => {
                self.log
                    .push(format!("hosting on UDP port {}", self.addr.port()));
                (None, Conn::Confirmed)
            }
            false => match net.connect(self.addr, CHANNELS, 0) {
                Ok(p) => {
                    self.log.push(format!("joining {}", self.addr));
                    (Some(p.id()), Conn::Connecting(self.frame))
                }
                Err(e) => {
                    self.log.push(format!("ENet: {e:?}"));
                    return;
                }
            },
        };
        self.session = Some(Session {
            request,
            net,
            host_peer,
            conn,
            player_id: 0,
            pending: None,
            seats: HashMap::new(),
            profiles: Vec::new(),
        });
    }

    fn update(&mut self, g: &mut impl Guest, ptr: u32, blob: &[u8]) {
        let l = self.layout.clone();
        let s = self.session.as_mut().expect("a session");
        let hs = l.handshake as usize;
        let hs_state = blob[hs + l.handshake_state as usize];
        let handshake = &blob[hs..hs + l.handshake_size as usize];
        if let Some(peer) = s.pending {
            // The host's game is answering a client: wait for it, then
            // send its answer back.
            if hs_state == HANDSHAKE_SEND_TO_CLIENT {
                s.pending = None;
                let id = blob[hs + l.handshake_player as usize];
                if !l.player_id_ok(id) {
                    self.log.push(format!("the game gave seat {id}; dropped"));
                    return;
                }
                s.seats.insert(peer, id);
                if let Some(p) = s.net.get_peer_mut(peer) {
                    let _ = p.send(CH_HANDSHAKE, &enet::Packet::reliable(handshake));
                }
                self.log.push(format!("player {id} joined"));
                // Send the profiles to everyone again.
                s.profiles.clear();
                s.net.flush();
                return;
            }
        } else if self.poll(g, ptr).is_err() {
            self.session = None;
            return;
        }
        let s = self.session.as_mut().expect("a session");
        match s.conn {
            Conn::Connecting(since) => {
                if self.frame - since > CONNECT_FRAMES {
                    self.log
                        .push(format!("no answer from {}; trying again", self.addr));
                    self.session = None;
                }
            }
            Conn::AwaitingHandshake => {
                if hs_state == HANDSHAKE_SEND_TO_HOST {
                    send(s, CH_HANDSHAKE, enet::Packet::reliable(handshake));
                    s.conn = Conn::AwaitingResponse;
                }
            }
            Conn::AwaitingResponse => {}
            Conn::Confirmed => {
                // Tell the game it's on: its current state is its request.
                g.write8(ptr + l.current, s.request);
                s.conn = Conn::Connected;
                self.log.push("connected".into());
            }
            Conn::Connected => self.connected_update(blob),
        }
        if let Some(s) = &mut self.session {
            s.net.flush();
        }
    }

    fn connected_update(&mut self, blob: &[u8]) {
        let l = &self.layout;
        let frame = self.frame;
        let s = self.session.as_mut().expect("a session");
        let span = |at: u32, n: u32| &blob[at as usize..(at + n) as usize];
        let players = span(l.players, l.player_size * l.player_count);
        if s.host_peer.is_none() {
            let profiles = span(l.profiles, l.profile_size * l.player_count);
            if s.profiles != profiles {
                s.profiles = profiles.to_vec();
                let p = enet::Packet::reliable(profiles);
                s.net.broadcast(CH_PLAYER_PROFILES, &p);
            }
            if frame.is_multiple_of(GAME_STATE_EVERY) {
                let p = enet::Packet::reliable(span(l.game_state, l.game_state_size));
                s.net.broadcast(CH_GAME_STATE, &p);
            }
            if frame.is_multiple_of(PLAYER_STATE_EVERY) {
                s.net
                    .broadcast(CH_PLAYER_STATE, &enet::Packet::unreliable(players));
            }
        } else if frame.is_multiple_of(PLAYER_STATE_EVERY) && l.player_id_ok(s.player_id) {
            let at = (l.player_size * s.player_id as u32) as usize;
            let mine = &players[at..at + l.player_size as usize];
            send(s, CH_PLAYER_STATE, enet::Packet::unreliable(mine));
        }
    }

    /// Take what came in and write it into the game.
    fn poll(&mut self, g: &mut impl Guest, ptr: u32) -> Result<(), ()> {
        let l = self.layout.clone();
        let s = self.session.as_mut().expect("a session");
        let is_host = s.host_peer.is_none();
        loop {
            let ev = match s.net.service() {
                Ok(Some(ev)) => ev.no_ref(),
                Ok(None) => return Ok(()),
                Err(e) => {
                    self.log.push(format!("UDP: {e}"));
                    return Err(());
                }
            };
            match ev {
                enet::EventNoRef::Connect { peer, .. } => {
                    if let Conn::Connecting(_) = s.conn
                        && Some(peer) == s.host_peer
                    {
                        s.conn = Conn::AwaitingHandshake;
                        self.log.push("reached the host".into());
                    }
                }
                enet::EventNoRef::Disconnect { peer, .. } => {
                    s.seats.remove(&peer);
                    if Some(peer) == s.host_peer {
                        self.log.push("the host went away".into());
                        return Err(());
                    }
                    self.log.push("a player went away".into());
                }
                enet::EventNoRef::Receive {
                    peer,
                    channel_id,
                    packet,
                } => {
                    let data = packet.data();
                    let n = data.len() as u32;
                    match channel_id {
                        CH_HANDSHAKE if n == l.handshake_size => {
                            g.write(ptr + l.handshake, data);
                            if is_host {
                                // The game answers it; then the rest waits.
                                s.pending = Some(peer);
                                return Ok(());
                            } else if s.conn == Conn::AwaitingResponse {
                                let id = data[l.handshake_player as usize];
                                if !l.player_id_ok(id) {
                                    self.log.push(format!("the host gave seat {id}"));
                                    return Err(());
                                }
                                s.player_id = id;
                                s.conn = Conn::Confirmed;
                            }
                        }
                        CH_PLAYER_PROFILES if !is_host && n == l.profile_size * l.player_count => {
                            g.write(ptr + l.profiles, data);
                        }
                        CH_GAME_STATE if !is_host && n == l.game_state_size => {
                            g.write(ptr + l.game_state, data);
                        }
                        CH_PLAYER_STATE if is_host && n == l.player_size => {
                            if let Some(&id) = s.seats.get(&peer) {
                                g.write(ptr + l.players + l.player_size * id as u32, data);
                            }
                        }
                        CH_PLAYER_STATE if !is_host && n == l.player_size * l.player_count => {
                            // Everyone's but our own.
                            for (id, d) in data.chunks(l.player_size as usize).enumerate() {
                                if id != s.player_id as usize {
                                    g.write(ptr + l.players + l.player_size * id as u32, d);
                                }
                            }
                        }
                        _ => self
                            .log
                            .push(format!("dropped {n} bytes on channel {channel_id}")),
                    }
                }
            }
        }
    }
}

fn send(s: &mut Session, channel: u8, p: enet::Packet) {
    if let Some(p2) = s.host_peer.and_then(|id| s.net.get_peer_mut(id)) {
        let _ = p2.send(channel, &p);
    }
}

/// `host[:port]`, the port 30025 if left out.
pub fn parse_addr(s: &str) -> Result<SocketAddr, String> {
    let with_port = match s.rsplit_once(':') {
        Some((_, p)) if p.parse::<u16>().is_ok() && !s.ends_with(']') => s.to_string(),
        _ => format!("{s}:{DEFAULT_PORT}"),
    };
    with_port
        .to_socket_addrs()
        .map_err(|e| format!("--rogue {s}: {e}"))?
        .next()
        .ok_or_else(|| format!("--rogue {s}: no address"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emerald Rogue 2.x's layout (the header of compat version 3).
    fn layout() -> Layout {
        Layout {
            confirm_size: 2,
            confirm_offset: 54,
            size: 1440,
            handshake: 1388,
            handshake_size: 48,
            handshake_state: 42,
            handshake_player: 43,
            game_state: 520,
            game_state_size: 868,
            profiles: 0,
            profile_size: 40,
            players: 80,
            player_size: 220,
            player_count: 2,
            request: 1436,
            current: 1437,
            assistant_state: 0x0203_8270,
            multiplayer_ptr: 0x0203_8e90,
        }
    }

    const BLOB: u32 = 0x0201_0000;

    /// The game's side, as `rogue_multiplayer.c` has it: one EWRAM.
    struct Game {
        ram: Vec<u8>,
        host: bool,
        mark: u8,
    }

    impl Guest for Game {
        fn read8(&self, a: u32) -> u8 {
            self.ram[(a & 0x3_ffff) as usize]
        }
        fn write8(&mut self, a: u32, v: u8) {
            self.ram[(a & 0x3_ffff) as usize] = v;
        }
    }

    impl Game {
        fn new(host: bool, mark: u8) -> Game {
            let mut g = Game {
                ram: vec![0; 0x4_0000],
                host,
                mark,
            };
            let l = layout();
            g.write(l.multiplayer_ptr, &BLOB.to_le_bytes());
            let at = |o: u32| BLOB + o;
            // RogueMP_OpenHost / OpenClient.
            g.write8(at(l.request), if host { 3 } else { 1 });
            g.write8(at(l.profiles), mark);
            if !host {
                g.write8(at(l.handshake), mark);
                g.write8(at(l.handshake + l.handshake_state), HANDSHAKE_SEND_TO_HOST);
            }
            g
        }

        /// RogueMP_MainCB, roughly: answer handshakes, write our state.
        fn frame(&mut self) {
            let l = layout();
            let at = |o: u32| BLOB + o;
            let hs = at(l.handshake + l.handshake_state);
            let active = self.read8(at(l.current)) & 1 != 0;
            if self.host && active && self.read8(hs) == HANDSHAKE_SEND_TO_HOST {
                let profile = self.read8(at(l.handshake));
                self.write8(at(l.profiles + l.profile_size), profile);
                self.write8(at(l.handshake + l.handshake_player), 1);
                self.write8(hs, HANDSHAKE_SEND_TO_CLIENT);
            }
            if !self.host && active && self.read8(hs) == HANDSHAKE_SEND_TO_CLIENT {
                self.write8(hs, 0);
            }
            let me = if self.host { 0 } else { 1 };
            self.write8(at(l.players + l.player_size * me), self.mark);
        }

        fn player(&self, id: u32) -> u8 {
            let l = layout();
            self.read8(BLOB + l.players + l.player_size * id)
        }
    }

    #[test]
    fn rom_header_reads() {
        let mut rom = vec![0u8; 0x1000];
        let w =
            |rom: &mut Vec<u8>, o: usize, v: u32| rom[o..o + 4].copy_from_slice(&v.to_le_bytes());
        w(&mut rom, GF_ROGUE, HANDSHAKE_1);
        w(&mut rom, GF_ROGUE + 4, 0x0800_0400);
        w(&mut rom, GF_ROGUE + 8, HANDSHAKE_2);
        let l = layout();
        let words = [
            COMPAT_VERSION,
            l.confirm_size,
            l.confirm_offset,
            l.size,
            l.handshake,
            l.handshake_size,
            l.handshake_state,
            l.handshake_player,
            l.game_state,
            l.game_state_size,
            l.profiles,
            l.profile_size,
            l.players,
            l.player_size,
            l.player_count,
            l.request,
            l.current,
        ];
        for (i, v) in words.iter().enumerate() {
            w(&mut rom, 0x404 + 4 * i, *v);
        }
        w(&mut rom, 0x404 + 4 * 30, l.assistant_state);
        w(&mut rom, 0x404 + 4 * 31, l.multiplayer_ptr);
        assert_eq!(Layout::from_rom(&rom), Ok(l));
        w(&mut rom, 0x404, 2);
        assert!(
            Layout::from_rom(&rom)
                .unwrap_err()
                .contains("compat version 2")
        );
    }

    #[test]
    fn two_games_see_each_other() {
        // A port of its own, so tests running together don't collide.
        let port = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        let (mut a, mut b) = (Game::new(true, 0xA1), Game::new(false, 0xB2));
        let mut host = Assistant::with_layout(layout(), addr);
        let mut client = Assistant::with_layout(layout(), addr);
        for _ in 0..600 {
            a.frame();
            b.frame();
            host.frame(&mut a);
            client.frame(&mut b);
            if a.player(1) == 0xB2 && b.player(0) == 0xA1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let log = format!("{:?} / {:?}", host.take_log(), client.take_log());
        assert!(host.connected() && client.connected(), "{log}");
        assert_eq!(a.player(1), 0xB2, "the host sees the client: {log}");
        assert_eq!(b.player(0), 0xA1, "the client sees the host: {log}");
        // The host's profiles reached the client, its own included.
        let l = layout();
        assert_eq!(b.read8(BLOB + l.profiles), 0xA1);
        assert_eq!(b.read8(BLOB + l.profiles + l.profile_size), 0xB2);
        // The assistant keeps zeroing the confirm word.
        assert_eq!(a.read8(l.assistant_state + l.confirm_offset), 0);
    }

    #[test]
    fn addresses() {
        assert_eq!(parse_addr("127.0.0.1").unwrap().port(), DEFAULT_PORT);
        assert_eq!(parse_addr("127.0.0.1:4000").unwrap().port(), 4000);
    }
}
