//! Plain-UDP rooms (`gbaa-server`), for rollback netplay: no QUIC,
//! no TLS, no streams; the server only seats players and passes their
//! datagrams on, so a datagram costs one trip through it and nothing more.
//!
//! Every datagram is a tag byte, then:
//!
//! - `H` seats build[32] version[u32 LE], player to server: make a room
//!   of `seats` (2-4) for this build and take seat 0. Sent again until
//!   answered; the same address asking again gets the same room.
//! - `J` code[6] build[32] version[u32 LE]: take the next free seat in
//!   room `code`.
//! - `R` code[6] seat seats filled, server to player: the room, this
//!   player's seat, and how many seats are taken; sent at each `H`/`J`,
//!   and to everyone each time a seat fills.
//! - `E` text: why the server said no (no such room, full, another
//!   build, too old a version).
//! - `D` seat payload: player to server, `seat` the one it's for; server
//!   to player, `seat` the one it's from. Passed on as it is.
//! - `K`: keep-alive, player to server, so the room and the player's NAT
//!   mapping stay open while it waits.
//! - `L`: player to server: this player has left; its seat is freed.
//! - `G` seat, server to player: the answer to a `D` for a seat whose
//!   player has left. A goodbye sent while this player wasn't reading
//!   (its machine stopped, its buffer full) may be lost; this isn't, as
//!   the next datagram for that seat asks again.
//! - `Q`, player to server: how many rooms are open; the answer is `Q`
//!   rooms[u32 LE] players[u32 LE] (`gbaa-server --rooms ADDR` asks). The
//!   codes aren't given: a code is what lets a player in.
//!
//! `version` is the client's netplay version (`netplay::VERSION`), a
//! number its builds raise by hand whenever a change can split peers or
//! change the wire. A server started with a minimum ([`Rooms::new`],
//! `gbaa-server --min-version N`) refuses a `H` or `J` from an older one,
//! with an `E` naming both numbers, so a player on a stale build is told
//! to update rather than joining and desyncing. A build from before
//! versions sends none and counts as version 0; it reads the `E` as any
//! other refusal. The number means nothing else to the server.
//!
//! A player who says nothing for [`IDLE`] has left too (a crash, a lost
//! connection, a machine stopped). A room closes when its last player
//! leaves, and once every seat has filled, a freed seat isn't offered
//! again: the game began without whoever would take it.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::{CODE_ALPHABET, CODE_LEN};

pub const HOST: u8 = b'H';
pub const JOIN: u8 = b'J';
pub const ROOM: u8 = b'R';
pub const ERROR: u8 = b'E';
pub const DATA: u8 = b'D';
pub const KEEP: u8 = b'K';
pub const LEAVE: u8 = b'L';
pub const QUERY: u8 = b'Q';
pub const GONE: u8 = b'G';

/// How long a player may send nothing before its seat is freed. Seated
/// players send every frame, and waiting ones ask again twice a second.
pub const IDLE: Duration = Duration::from_secs(10);

type Code = [u8; CODE_LEN];

struct Room {
    build: [u8; 32],
    /// Each seat's player and when it was last heard from.
    seats: Vec<Option<(SocketAddr, Instant)>>,
    /// Every seat has filled: the game is on, and a freed seat stays empty.
    began: bool,
}

impl Room {
    fn filled(&self) -> u8 {
        self.seats.iter().filter(|s| s.is_some()).count() as u8
    }

    fn seat_of(&self, who: SocketAddr) -> Option<usize> {
        self.seats
            .iter()
            .position(|s| s.is_some_and(|(a, _)| a == who))
    }
}

#[derive(Default)]
pub struct Rooms {
    /// The oldest client version let in ([`Rooms::new`]); 0 lets every
    /// one in.
    min_version: u32,
    rooms: HashMap<Code, Room>,
    /// Each player's room and seat.
    players: HashMap<SocketAddr, (Code, u8)>,
    seed: u64,
    /// Datagrams passed on.
    pub passed: u64,
    /// What happened to rooms since the server last printed it (rooms
    /// opened and closed, players leaving): [`Rooms::take_log`].
    log: Vec<String>,
}

/// What the server says to a client of `version` when it needs `min`.
pub fn too_old(version: u32, min: u32) -> String {
    format!(
        "this game is too old for the server (version {version}, needs {min}): get the new build"
    )
}

/// The version after a `H`'s or `J`'s build ID (`rest`), 0 if there's
/// none (a build from before versions).
fn version_of(rest: &[u8]) -> u32 {
    rest.get(..4)
        .map_or(0, |v| u32::from_le_bytes(v.try_into().unwrap()))
}

impl Rooms {
    /// Rooms that refuse clients older than netplay version `min_version`.
    pub fn new(min_version: u32) -> Rooms {
        Rooms {
            min_version,
            ..Rooms::default()
        }
    }

    /// Whether a client of `version` is too old: the refusal to send it,
    /// noted in the log.
    fn refuse(&mut self, version: u32) -> Option<String> {
        (version < self.min_version).then(|| {
            self.log.push(format!(
                "refused a client of version {version} (needs {})",
                self.min_version
            ));
            too_old(version, self.min_version)
        })
    }

    fn new_code(&mut self) -> Code {
        if self.seed == 0 {
            self.seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as u64)
                | 1;
        }
        loop {
            let mut code = [0u8; CODE_LEN];
            for c in &mut code {
                let x = &mut self.seed;
                *x ^= *x << 13;
                *x ^= *x >> 7;
                *x ^= *x << 17;
                *c = CODE_ALPHABET[(*x % CODE_ALPHABET.len() as u64) as usize];
            }
            if !self.rooms.contains_key(&code) {
                return code;
            }
        }
    }

    fn room_msg(code: &Code, seat: u8, room: &Room) -> Vec<u8> {
        let mut m = vec![ROOM];
        m.extend_from_slice(code);
        m.extend_from_slice(&[seat, room.seats.len() as u8, room.filled()]);
        m
    }

    /// A seat's player was heard from at `now`.
    fn heard(&mut self, from: SocketAddr, now: Instant) {
        if let Some(&(code, seat)) = self.players.get(&from)
            && let Some(room) = self.rooms.get_mut(&code)
            && let Some(Some((a, t))) = room.seats.get_mut(seat as usize)
            && *a == from
        {
            *t = now;
        }
    }

    /// What to send where, given datagram `d` from `from` at `now`.
    pub fn take(&mut self, from: SocketAddr, d: &[u8], now: Instant) -> Vec<(SocketAddr, Vec<u8>)> {
        let no = |why: &str| {
            let mut m = vec![ERROR];
            m.extend_from_slice(why.as_bytes());
            vec![(from, m)]
        };
        let Some((&tag, rest)) = d.split_first() else {
            return Vec::new();
        };
        match tag {
            HOST => {
                let (Some(&seats), Some(build)) = (rest.first(), rest.get(1..33)) else {
                    return no("bad request");
                };
                if let Some(why) = self.refuse(version_of(&rest[33..])) {
                    return no(&why);
                }
                if !(2..=4).contains(&seats) {
                    return no("a room has 2 to 4 seats");
                }
                let code = match self.players.get(&from) {
                    Some(&(code, 0)) if self.rooms.contains_key(&code) => code,
                    _ => {
                        self.leave(from, "hosted again");
                        let code = self.new_code();
                        let mut s = vec![None; seats as usize];
                        s[0] = Some((from, now));
                        self.rooms.insert(
                            code,
                            Room {
                                build: build.try_into().unwrap(),
                                seats: s,
                                began: false,
                            },
                        );
                        self.players.insert(from, (code, 0));
                        self.log
                            .push(format!("room {} opened, {seats} seats", show(&code)));
                        code
                    }
                };
                self.heard(from, now);
                vec![(from, Self::room_msg(&code, 0, &self.rooms[&code]))]
            }
            JOIN => {
                let (Some(code), Some(build)) =
                    (rest.get(..CODE_LEN), rest.get(CODE_LEN..CODE_LEN + 32))
                else {
                    return no("bad request");
                };
                if let Some(why) = self.refuse(version_of(&rest[CODE_LEN + 32..])) {
                    return no(&why);
                }
                let code: Code = code.try_into().unwrap();
                let Some(room) = self.rooms.get_mut(&code) else {
                    return no("no such room");
                };
                if room.build[..] != *build {
                    return no("the room runs another build or game");
                }
                if let Some(seat) = room.seat_of(from) {
                    room.seats[seat] = Some((from, now));
                    return vec![(from, Self::room_msg(&code, seat as u8, room))];
                }
                if room.began {
                    return no("the room's game has begun");
                }
                let Some(seat) = room.seats.iter().position(Option::is_none) else {
                    return no("the room is full (the host sets its seats)");
                };
                // A player in another room leaves it for this one.
                self.leave(from, "joined another room");
                let room = self.rooms.get_mut(&code).unwrap();
                room.seats[seat] = Some((from, now));
                room.began = room.filled() as usize == room.seats.len();
                let room = &self.rooms[&code];
                let out = room
                    .seats
                    .iter()
                    .enumerate()
                    .filter_map(|(s, a)| a.map(|(a, _)| (a, Self::room_msg(&code, s as u8, room))))
                    .collect();
                self.players.insert(from, (code, seat as u8));
                out
            }
            DATA => {
                let Some(&(code, seat)) = self.players.get(&from) else {
                    return Vec::new();
                };
                self.heard(from, now);
                let Some(room) = self.rooms.get(&code) else {
                    return Vec::new();
                };
                let Some(&to) = rest.first() else {
                    return Vec::new();
                };
                let addr = match room.seats.get(to as usize) {
                    Some(Some((addr, _))) => *addr,
                    Some(None) if room.began => return vec![(from, vec![GONE, to])],
                    _ => return Vec::new(),
                };
                let mut m = Vec::with_capacity(d.len());
                m.extend_from_slice(&[DATA, seat]);
                m.extend_from_slice(&rest[1..]);
                self.passed += 1;
                vec![(addr, m)]
            }
            KEEP => {
                self.heard(from, now);
                Vec::new()
            }
            LEAVE => {
                self.leave(from, "left");
                Vec::new()
            }
            QUERY => {
                let mut m = vec![QUERY];
                m.extend_from_slice(&(self.rooms.len() as u32).to_le_bytes());
                m.extend_from_slice(&(self.players.len() as u32).to_le_bytes());
                vec![(from, m)]
            }
            _ => Vec::new(),
        }
    }

    /// Free `who`'s seat (it left, `why`), and close its room if that was
    /// the last player in it.
    fn leave(&mut self, who: SocketAddr, why: &str) {
        let Some((code, seat)) = self.players.remove(&who) else {
            return;
        };
        let Some(room) = self.rooms.get_mut(&code) else {
            return;
        };
        if room.seats[seat as usize].is_some_and(|(a, _)| a == who) {
            room.seats[seat as usize] = None;
        }
        self.log.push(format!(
            "room {}: player {} {why}",
            show(&code),
            seat as usize + 1
        ));
        if room.filled() == 0 {
            self.rooms.remove(&code);
            self.log
                .push(format!("room {} closed: every player gone", show(&code)));
        }
    }

    /// Free the seats of players who have sent nothing for [`IDLE`], and
    /// close the rooms that leaves empty.
    pub fn expire(&mut self, now: Instant) {
        let quiet: Vec<SocketAddr> = self
            .rooms
            .values()
            .flat_map(|r| r.seats.iter().flatten())
            .filter(|(_, t)| now.saturating_duration_since(*t) >= IDLE)
            .map(|(a, _)| *a)
            .collect();
        for a in quiet {
            self.leave(a, "went quiet");
        }
        let rooms = &self.rooms;
        self.players.retain(|_, (code, _)| rooms.contains_key(code));
    }

    /// What happened since the last call, a line each.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}

fn show(code: &Code) -> String {
    String::from_utf8_lossy(code).into_owned()
}

/// Ask the rooms server at `server` how many rooms are open and how many
/// players are in them.
pub fn query(server: SocketAddr) -> std::io::Result<(u32, u32)> {
    let bind = if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = UdpSocket::bind(bind)?;
    sock.set_read_timeout(Some(Duration::from_millis(300)))?;
    let mut buf = [0u8; 64];
    for _ in 0..10 {
        sock.send_to(&[QUERY], server)?;
        match sock.recv_from(&mut buf) {
            Ok((9, from)) if from == server && buf[0] == QUERY => {
                let n = |i: usize| u32::from_le_bytes(buf[i..i + 4].try_into().unwrap());
                return Ok((n(1), n(5)));
            }
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::TimedOut,
        format!("no answer from {server}"),
    ))
}

/// Serve rooms on `sock` until it fails, refusing clients older than
/// netplay version `min_version` (0 refuses none), saying on stderr as
/// rooms open and close.
pub fn serve(sock: UdpSocket, min_version: u32) -> std::io::Result<()> {
    sock.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut rooms = Rooms::new(min_version);
    let mut buf = vec![0u8; 65536];
    let mut swept = Instant::now();
    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                for (to, m) in rooms.take(from, &buf[..n], Instant::now()) {
                    // A full buffer or a gone player is a lost datagram.
                    let _ = sock.send_to(&m, to);
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            // A bounce from a player that's gone (some systems report one).
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset
                ) => {}
            Err(e) => return Err(e),
        }
        if swept.elapsed() >= Duration::from_secs(1) {
            swept = Instant::now();
            rooms.expire(swept);
        }
        for l in rooms.take_log() {
            eprintln!("gbaa-server: {l}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(p: u16) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, 1], p))
    }

    #[test]
    fn two_players_meet_and_pass_datagrams() {
        let mut r = Rooms::default();
        let now = Instant::now();
        let mut host = vec![HOST, 2];
        host.extend_from_slice(&[7; 32]);
        let out = r.take(a(1), &host, now);
        assert_eq!(out.len(), 1);
        let m = &out[0].1;
        assert_eq!((m[0], m[7], m[8], m[9]), (ROOM, 0, 2, 1));
        let code: Code = m[1..7].try_into().unwrap();
        // Asking again: the same room.
        assert_eq!(r.take(a(1), &host, now)[0].1[1..7], code);
        let mut join = vec![JOIN];
        join.extend_from_slice(&code);
        join.extend_from_slice(&[8; 32]);
        assert_eq!(r.take(a(2), &join, now)[0].1[0], ERROR);
        join[7..].fill(7);
        let out = r.take(a(2), &join, now);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|(_, m)| m[9] == 2));
        assert_eq!(r.take(a(3), &join, now)[0].1[0], ERROR);
        let out = r.take(a(2), &[DATA, 0, 42, 43], now);
        assert_eq!(out, [(a(1), vec![DATA, 1, 42, 43])]);
        let out = r.take(a(1), &[DATA, 1, 44], now);
        assert_eq!(out, [(a(2), vec![DATA, 0, 44])]);
        // The game has begun: a seat freed isn't offered again.
        let later = now + IDLE / 2;
        r.take(a(2), &[LEAVE], later);
        assert_eq!(r.len(), 1);
        assert_eq!(r.take(a(3), &join, later)[0].1[0], ERROR);
        // Seat 1's player has gone: the server says so to who sends to it.
        assert_eq!(r.take(a(1), &[DATA, 1, 1], later), [(a(1), vec![GONE, 1])]);
        // The host goes quiet: the room closes.
        r.expire(later + IDLE);
        assert!(r.is_empty());
        assert!(r.take(a(2), &[DATA, 0, 1], later).is_empty());
        let log = r.take_log();
        assert!(log.iter().any(|l| l.ends_with("player 2 left")), "{log:?}");
        assert!(
            log.iter().any(|l| l.ends_with("player 1 went quiet")),
            "{log:?}"
        );
        assert!(log.last().unwrap().ends_with("closed: every player gone"));
    }

    #[test]
    fn rooms_close_as_their_players_leave() {
        let mut r = Rooms::default();
        let now = Instant::now();
        let mut host = vec![HOST, 2];
        host.extend_from_slice(&[7; 32]);
        let code: Code = r.take(a(1), &host, now)[0].1[1..7].try_into().unwrap();
        let count = |r: &mut Rooms| {
            let m = &r.take(a(9), &[QUERY], now)[0].1;
            let n = |i: usize| u32::from_le_bytes(m[i..i + 4].try_into().unwrap());
            (n(1), n(5))
        };
        assert_eq!(count(&mut r), (1, 1));
        // Waiting, the host asks again; still there after IDLE's worth.
        r.take(a(1), &host, now + IDLE / 2);
        r.expire(now + IDLE);
        assert_eq!(count(&mut r), (1, 1));
        let mut join = vec![JOIN];
        join.extend_from_slice(&code);
        join.extend_from_slice(&[7; 32]);
        r.take(a(2), &join, now + IDLE);
        assert_eq!(count(&mut r), (1, 2));
        // Hosting again leaves the old room for a new one.
        let out = r.take(a(1), &host, now + IDLE);
        assert_eq!(out[0].1[1..7], code);
        r.take(a(1), &[LEAVE], now + IDLE);
        let code2: Code = r.take(a(1), &host, now + IDLE)[0].1[1..7]
            .try_into()
            .unwrap();
        assert_ne!(code, code2);
        assert_eq!(count(&mut r), (2, 2));
        r.take(a(2), &[LEAVE], now + IDLE);
        assert_eq!(count(&mut r), (1, 1));
        r.take(a(1), &[LEAVE], now + IDLE);
        assert_eq!(count(&mut r), (0, 0));
        assert!(r.is_empty());
    }

    fn host(version: Option<u32>) -> Vec<u8> {
        let mut m = vec![HOST, 2];
        m.extend_from_slice(&[7; 32]);
        m.extend(version.map(u32::to_le_bytes).into_iter().flatten());
        m
    }

    fn join(code: &Code, version: Option<u32>) -> Vec<u8> {
        let mut m = vec![JOIN];
        m.extend_from_slice(code);
        m.extend_from_slice(&[7; 32]);
        m.extend(version.map(u32::to_le_bytes).into_iter().flatten());
        m
    }

    fn said(out: &[(SocketAddr, Vec<u8>)]) -> String {
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].1[0], ERROR, "{out:?}");
        String::from_utf8_lossy(&out[0].1[1..]).into_owned()
    }

    #[test]
    fn old_versions_are_refused() {
        let now = Instant::now();
        let mut r = Rooms::new(3);
        // Older, and from before versions: told why, with both numbers.
        assert_eq!(
            said(&r.take(a(1), &host(Some(2)), now)),
            "this game is too old for the server (version 2, needs 3): get the new build"
        );
        assert_eq!(said(&r.take(a(1), &host(None), now)), too_old(0, 3));
        assert!(r.is_empty());
        // Equal and newer are let in.
        let out = r.take(a(1), &host(Some(3)), now);
        assert_eq!(out[0].1[0], ROOM);
        let code: Code = out[0].1[1..7].try_into().unwrap();
        assert_eq!(
            said(&r.take(a(2), &join(&code, Some(1)), now)),
            too_old(1, 3)
        );
        assert_eq!(said(&r.take(a(2), &join(&code, None), now)), too_old(0, 3));
        let out = r.take(a(2), &join(&code, Some(4)), now);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|(_, m)| m[0] == ROOM && m[9] == 2));
        let mut r2 = Rooms::new(3);
        assert_eq!(r2.take(a(1), &host(Some(9)), now)[0].1[0], ROOM);
        let log = r.take_log();
        assert!(
            log.iter()
                .any(|l| l == "refused a client of version 0 (needs 3)"),
            "{log:?}"
        );
        // No minimum: every version, and none, is let in.
        let mut r = Rooms::default();
        let code: Code = r.take(a(1), &host(None), now)[0].1[1..7]
            .try_into()
            .unwrap();
        assert_eq!(r.take(a(2), &join(&code, Some(0)), now).len(), 2);
    }

    #[test]
    fn four_players_take_seats_in_order() {
        let mut r = Rooms::default();
        let now = Instant::now();
        let mut host = vec![HOST, 4];
        host.extend_from_slice(&[7; 32]);
        let code: Code = r.take(a(1), &host, now)[0].1[1..7].try_into().unwrap();
        let mut join = vec![JOIN];
        join.extend_from_slice(&code);
        join.extend_from_slice(&[7; 32]);
        for p in 2..=4u16 {
            let out = r.take(a(p), &join, now);
            // Everyone seated hears the count; the newcomer its seat.
            assert_eq!(out.len(), p as usize);
            assert!(out.iter().all(|(_, m)| m[8] == 4 && m[9] == p as u8));
            let mine = out.iter().find(|(to, _)| *to == a(p)).unwrap();
            assert_eq!(mine.1[7], p as u8 - 1);
        }
        assert_eq!(r.take(a(5), &join, now)[0].1[0], ERROR);
        // Seat 3 to seat 1, and back.
        assert_eq!(r.take(a(4), &[DATA, 1, 9], now), [(a(2), vec![DATA, 3, 9])]);
        assert_eq!(r.take(a(2), &[DATA, 3, 8], now), [(a(4), vec![DATA, 1, 8])]);
        let mut five = host.clone();
        five[1] = 5;
        assert_eq!(r.take(a(6), &five, now)[0].1[0], ERROR);
    }
}
