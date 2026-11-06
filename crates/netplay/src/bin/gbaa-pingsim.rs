//! `gbaa-pingsim`: the variable-ping simulator (multiplayer/ping-sim). Two
//! peers, each its own process, play a netplay session over UDP through a
//! proxy ([`netplay::proxy`]) that delays, loses and reorders datagrams as
//! a TOML profile says (`crates/netplay/profiles/`).
//!
//! A session here is rollback's ([`netplay::Rollback`]), the only kind.
//!
//! - `gbaa-pingsim run PROFILE [OPTIONS]`: the check. Starts the proxy,
//!   runs seat 0 and seat 1 as two processes of this program, and passes
//!   (exit 0) when both peers' hashes, every 60 frames and at the end, are
//!   a straight run's, with no desync.
//! - `gbaa-pingsim proxy PROFILE A_ADDR B_ADDR`: the proxy alone, until
//!   killed (peer A sends to A_ADDR, peer B to B_ADDR).
//! - `gbaa-pingsim peer SEAT ADDR [OPTIONS]`: one peer, seat 0 hosting,
//!   sending to ADDR (its side of the proxy, or the other peer itself).
//!   Prints `hash FRAME HASH` lines, `desync FRAME`, `end HASH` and a
//!   `stats` line.
//! - `gbaa-pingsim room PROFILE [--server ADDR] [OPTIONS]`: the same
//!   through `gbaa-server`'s rooms: a local one (in this process), or the
//!   one at ADDR (fly's, say), and a proxy for each peer between it and the
//!   server, each following the profile; seat 0 makes a room of
//!   `--players` seats, the others join it by its code. Passes as `run`
//!   does, on every seat.
//! - `gbaa-pingsim room-peer SEAT ADDR [CODE] [OPTIONS]`: one peer through
//!   the server at ADDR; seat 0 makes a room of `--players` and prints
//!   `code CODE` first, any other joins CODE (taking the next free seat).
//! - `gbaa-pingsim straight [OPTIONS]`: the same game without a network.
//! - `gbaa-pingsim desktop PROFILE ROM [-- DESKTOP_ARGS]`: two (or
//!   `--players N`, up to four) `gbaa-desktop` netplay peers (beside this
//!   program) on ROM through a local server, each joiner's path through a
//!   proxy following the profile (the host's direct, so the profile is the
//!   whole trip between host and joiner). DESKTOP_ARGS go to all
//!   (`--headless --paced --frames N`, ...); `--input1 S` to `--input4 S`
//!   among them go to one each as `--input`, `--save1 F`.. as `--save`,
//!   `--menu1 S`.. as `--menu` (when the seat's menu is open; `--menu S`
//!   goes to all), `--dump-dir1 D`.. as `--dump-dir`, `--desync-at1 N`..
//!   as `--desync-at` (a forced desync); `--delay D` and `--players N` to
//!   the host; `--exe BIN` runs BIN for all instead of `gbaa-desktop` (a
//!   recompiled game's binary, `gbaa-<game>`). The joiners start a second
//!   apart, so each takes the seat its options name. Prints each peer's
//!   end lines; fails if their end hashes differ. Without `--headless`,
//!   windows: how it
//!   feels.
//!
//! OPTIONS: `--console toy|gba` (the toy console, or GBAs on a cable
//! running a tiny ROM that reads the keys; toy), `--players N` (seats, 2
//! to 4, `room` only; 2), `--frames N`
//! (600), `--delay D` (input delay, 0), `--fps F` (each peer's frame rate,
//! 60), `--linger-ms M` (the most a peer keeps answering after its last
//! frame, waiting for the other's last inputs and acks; 3000).

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use netplay::demo::{self, Toy};
use netplay::proxy::{Profile, Proxy};
use netplay::session::HASH_EVERY;
use netplay::udp::Udp;
use netplay::{Config, Console, Netplay, Rollback, Snapshot, Status, rollback};

const USAGE: &str = "usage: gbaa-pingsim run PROFILE [OPTIONS]
       gbaa-pingsim proxy PROFILE A_ADDR B_ADDR
       gbaa-pingsim peer SEAT ADDR [OPTIONS]
       gbaa-pingsim room PROFILE [--server ADDR] [OPTIONS]
       gbaa-pingsim room-peer SEAT ADDR [CODE] [OPTIONS]
       gbaa-pingsim straight [OPTIONS]
       gbaa-pingsim desktop PROFILE ROM [-- DESKTOP_ARGS]
OPTIONS: --console toy|gba, --players N, --frames N, --delay D, --fps F, --linger-ms M";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Toy,
    Gba,
}

#[derive(Clone, Debug)]
struct Options {
    console: Kind,
    frames: u32,
    delay: u8,
    fps: f64,
    linger_ms: u64,
    /// `room`'s server, when not a local one.
    server: Option<String>,
    /// `room`'s seats, 2-4.
    players: usize,
}

impl Options {
    /// Back to arguments, for the peers `run` starts.
    fn args(&self) -> Vec<String> {
        let console = match self.console {
            Kind::Toy => "toy",
            Kind::Gba => "gba",
        };
        [
            ("--console", console.to_string()),
            ("--frames", self.frames.to_string()),
            ("--delay", self.delay.to_string()),
            ("--fps", self.fps.to_string()),
            ("--linger-ms", self.linger_ms.to_string()),
            ("--players", self.players.to_string()),
        ]
        .into_iter()
        .flat_map(|(k, v)| [k.to_string(), v])
        .collect()
    }
}

/// The options among `args`, and what's left.
fn options(args: &[String]) -> Result<(Options, Vec<String>), String> {
    let mut o = Options {
        console: Kind::Toy,
        frames: 600,
        delay: 0,
        fps: 60.0,
        linger_ms: 3000,
        server: None,
        players: 2,
    };
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !a.starts_with("--") {
            rest.push(a.clone());
            continue;
        }
        let v = it.next().ok_or(format!("{a} needs a value"))?;
        let bad = |_: std::num::ParseIntError| format!("{a} {v}?");
        match a.as_str() {
            "--console" => {
                o.console = match v.as_str() {
                    "toy" => Kind::Toy,
                    "gba" => Kind::Gba,
                    _ => return Err(format!("--console toy or gba, not {v}")),
                }
            }
            "--frames" => o.frames = v.parse().map_err(bad)?,
            "--delay" => o.delay = v.parse().map_err(bad)?,
            "--fps" => o.fps = v.parse().map_err(|_| format!("{a} {v}?"))?,
            "--linger-ms" => o.linger_ms = v.parse().map_err(bad)?,
            "--server" => o.server = Some(v.clone()),
            "--players" => o.players = v.parse().map_err(bad)?,
            _ => return Err(format!("unknown option {a}")),
        }
    }
    if !o.fps.is_finite() || o.fps <= 0.0 {
        return Err("--fps must be above 0".into());
    }
    if !(2..=runtime::link::MAX_PLAYERS).contains(&o.players) {
        return Err(format!(
            "--players 2 to {}, not {}",
            runtime::link::MAX_PLAYERS,
            o.players
        ));
    }
    if o.frames < HASH_EVERY {
        return Err(format!("--frames at least {HASH_EVERY}, for a hash check"));
    }
    Ok((o, rest))
}

fn config(o: &Options) -> Config {
    Config {
        delay: o.delay,
        ..Config::default()
    }
}

/// Seat `seat`'s own console of `seats`, or (`None`) the whole session's.
fn console_toy(seats: usize, seat: Option<usize>) -> Toy {
    seat.map_or_else(|| Toy::new(seats), |s| Toy::peer(seats, s))
}

fn console_gba(seats: usize, seat: Option<usize>) -> netplay::gba::Gba {
    let mut g = demo::gba(demo::key_folder(), seats, seat);
    if seat.is_none() {
        // A session sets the host's clock at its start; so does the
        // straight run.
        g.set_clock(Config::default().clock);
    }
    g
}

fn straight(o: &Options) -> Vec<(u32, u64)> {
    match o.console {
        Kind::Toy => demo::straight(console_toy(o.players, None), o.delay as u32, o.frames),
        Kind::Gba => demo::straight(console_gba(o.players, None), o.delay as u32, o.frames),
    }
}

fn peer(seat: usize, to: SocketAddr, o: &Options) -> Result<(), String> {
    if seat > 1 || o.players != 2 {
        return Err("two seats: 0 or 1 (`room` plays more)".into());
    }
    let bind: SocketAddr = if to.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }
        .parse()
        .unwrap();
    let sock = UdpSocket::bind(bind).map_err(|e| format!("bind: {e}"))?;
    let udp = Udp::new(sock, seat, &[to, to]).map_err(|e| e.to_string())?;
    match o.console {
        Kind::Toy => play(console_toy(2, Some(seat)), udp, seat, o),
        Kind::Gba => play(console_gba(2, Some(seat)), udp, seat, o),
    }
}

fn udp_said(u: &Udp) -> String {
    let st = u.stats;
    format!(
        "datagrams={} resent={} damaged={}",
        st.datagrams, st.resent, st.damaged
    )
}

fn room_peer(
    seat: usize,
    server: SocketAddr,
    code: Option<&str>,
    o: &Options,
) -> Result<(), String> {
    let n = o.players;
    match o.console {
        Kind::Toy => room_play(|s| console_toy(n, Some(s)), server, seat, code, o),
        Kind::Gba => room_play(|s| console_gba(n, Some(s)), server, seat, code, o),
    }
}

/// Seat 0 makes the room; a joiner is seated as it arrives (any seat from
/// 1), so its console is made (`make`) once the room says which.
fn room_play<C: Snapshot<Input = u16>>(
    make: impl Fn(usize) -> C,
    server: SocketAddr,
    seat: usize,
    code: Option<&str>,
    o: &Options,
) -> Result<(), String> {
    let id = make(seat).build_id();
    let until = Instant::now() + Duration::from_secs(60);
    let go_on = || Instant::now() < until;
    let (net, _) = match (seat, code) {
        (0, None) => {
            let mut said = false;
            let say = |code: [u8; relay::CODE_LEN], _, _| {
                if !std::mem::replace(&mut said, true) {
                    println!("code {}", String::from_utf8_lossy(&code));
                    let _ = std::io::stdout().flush();
                }
            };
            Udp::host(server, o.players, &id, say, go_on)?
        }
        (1.., Some(code)) => {
            let code = code
                .as_bytes()
                .try_into()
                .map_err(|_| format!("code {code}?"))?;
            Udp::join(server, code, &id, go_on)?
        }
        _ => return Err("seat 0 hosts (no code), the others join a code".into()),
    };
    let seat = net.seat_of();
    if net.seats() != o.players {
        return Err(format!("the room has {} seats", net.seats()));
    }
    play(make(seat), net, seat, o)
}

/// Run the session at `o.fps`, printing what `run` reads.
fn play<C: Snapshot<Input = u16>>(c: C, net: Udp, seat: usize, o: &Options) -> Result<(), String> {
    let host = (seat == 0).then(|| config(o));
    let mut s = Rollback::new(c, net, seat, host, rollback::WINDOW);
    let frame = Duration::from_secs_f64(1.0 / o.fps);
    let retry = Duration::from_millis(1);
    let start = Instant::now();
    let mut next = start;
    let mut waits = 0u64;
    let e = |e: netplay::Error| e.to_string();
    while s.frame() < o.frames {
        let now = Instant::now();
        if now < next {
            std::thread::sleep((next - now).min(retry));
            s.pump().map_err(e)?;
            continue;
        }
        match s.advance(demo::script(seat, s.frame())).map_err(e)? {
            Status::Stepped | Status::Skipped => next = now + frame,
            Status::Waiting | Status::Starting => {
                waits += 1;
                next = now + retry;
            }
        }
    }
    let played = start.elapsed();
    // The other peer may still lack inputs or acks of ours, and this one
    // theirs: the last guesses are put right while it lingers, until
    // nothing more can change (a peer slowed by a busy machine may finish
    // well after this one), then a little longer so the last acks get
    // through the losses.
    let linger = Instant::now() + Duration::from_millis(o.linger_ms);
    let mut done = None;
    while Instant::now() < linger {
        s.pump().map_err(e)?;
        let now = Instant::now();
        if s.settled() && now >= *done.get_or_insert(now + Duration::from_millis(250)) {
            break;
        }
        std::thread::sleep(retry);
    }
    for (f, h) in s.hashes().iter().filter(|(f, _)| *f < o.frames) {
        println!("hash {f} {h:016x}");
    }
    for f in s.desyncs() {
        println!("desync {f}");
    }
    println!("end {:016x}", s.console_mut().hash());
    println!(
        "stats waits={waits} secs={:.2} {} {}",
        played.as_secs_f64(),
        udp_said(s.transport()),
        s.summary()
    );
    Ok(())
}

/// What one peer printed.
#[derive(Default, Debug)]
struct Report {
    hashes: Vec<(u32, u64)>,
    desyncs: Vec<u32>,
    end: Option<u64>,
    stats: String,
}

fn report(out: &str) -> Result<Report, String> {
    let mut r = Report::default();
    let hex = |h: &str| u64::from_str_radix(h, 16).map_err(|_| format!("hash {h}?"));
    for line in out.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["hash", f, h] => r.hashes.push((f.parse().map_err(|_| line)?, hex(h)?)),
            ["desync", f] => r.desyncs.push(f.parse().map_err(|_| line)?),
            ["end", h] => r.end = Some(hex(h)?),
            ["code", _] => {}
            ["stats", ..] => r.stats = line.to_string(),
            _ => return Err(format!("peer said {line:?}")),
        }
    }
    Ok(r)
}

fn run(profile_path: &Path, o: &Options) -> Result<(), String> {
    let profile = Profile::load(profile_path)?;
    let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let proxy = Proxy::start(profile, any, any).map_err(|e| format!("proxy: {e}"))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut kids = Vec::new();
    for (seat, addr) in proxy.addrs().iter().enumerate() {
        let args = ["peer".to_string(), seat.to_string(), addr.to_string()];
        kids.push(spawn(&exe, &args, o, seat)?);
    }
    judge(kids, vec![proxy], o)
}

fn spawn(exe: &Path, args: &[String], o: &Options, seat: usize) -> Result<Peer, String> {
    let mut kid = Command::new(exe)
        .args(args)
        .args(o.args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("seat {seat}: {e}"))?;
    let out = BufReader::new(kid.stdout.take().unwrap());
    Ok((kid, out))
}

type Peer = (Child, BufReader<ChildStdout>);

/// Two peers through a server's rooms (a local one, or `o.server`), each
/// on its own proxy to it.
fn run_room(profile_path: &Path, o: &Options) -> Result<(), String> {
    let profile = Profile::load(profile_path)?;
    let server = match &o.server {
        Some(a) => address(a)?,
        None => {
            let sock = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
            let addr = sock.local_addr().map_err(|e| e.to_string())?;
            std::thread::spawn(move || relay::rooms::serve(sock, 0));
            addr
        }
    };
    let local: SocketAddr = if server.ip().is_loopback() {
        "127.0.0.1:0"
    } else {
        "0.0.0.0:0"
    }
    .parse()
    .unwrap();
    let mut proxies = Vec::new();
    for seat in 0..o.players as u64 {
        // Each path its own randomness.
        let p = Profile {
            seed: profile.seed.wrapping_add(seat),
            ..profile.clone()
        };
        proxies.push(Proxy::to(p, local, server).map_err(|e| format!("proxy: {e}"))?);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // The peers reach their proxies on this machine.
    let to: Vec<SocketAddr> = proxies
        .iter()
        .map(|p| SocketAddr::from(([127, 0, 0, 1], p.addrs()[0].port())))
        .collect();
    let peer = |seat: usize, code: Option<&str>| {
        let mut args = vec![
            "room-peer".to_string(),
            seat.to_string(),
            to[seat].to_string(),
        ];
        args.extend(code.map(str::to_string));
        spawn(&exe, &args, o, seat)
    };
    let mut host = peer(0, None)?;
    let mut line = String::new();
    host.1.read_line(&mut line).map_err(|e| e.to_string())?;
    let Some(code) = line.trim().strip_prefix("code ").map(str::to_string) else {
        let _ = host.0.kill();
        let _ = host.0.wait();
        return Err(format!("seat 0 gave no room code: {line:?}"));
    };
    println!("room {code} on {server}");
    let mut peers = vec![host];
    for seat in 1..o.players {
        peers.push(peer(seat, Some(&code))?);
    }
    judge(peers, proxies, o)
}

/// A server's address: a host name or IP, and a port (default
/// [`relay::PORT`]).
fn address(v: &str) -> Result<SocketAddr, String> {
    let with_port;
    let a = if v.contains(':') {
        v
    } else {
        with_port = format!("{v}:{}", relay::PORT);
        &with_port
    };
    use std::net::ToSocketAddrs;
    a.to_socket_addrs()
        .map_err(|e| format!("server {v}: {e}"))?
        .find(SocketAddr::is_ipv4)
        .ok_or(format!("server {v}: no IPv4 address"))
}

/// Waits for the peers, then compares what they printed with a straight
/// run.
fn judge(peers: Vec<Peer>, proxies: Vec<Proxy>, o: &Options) -> Result<(), String> {
    let want = straight(o);
    // Generous: four times the frames' own time, plus the start and the
    // linger.
    let limit = Duration::from_secs_f64(o.frames as f64 / o.fps * 4.0 + 30.0)
        + Duration::from_millis(o.linger_ms);
    let began = Instant::now();
    let (mut kids, pipes): (Vec<Child>, Vec<_>) = peers.into_iter().unzip();
    let mut outs = vec![String::new(); kids.len()];
    let readers: Vec<_> = pipes
        .into_iter()
        .map(|out| {
            std::thread::spawn(move || {
                out.lines()
                    .map_while(Result::ok)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
        .collect();
    let mut failed = Vec::new();
    for (seat, k) in kids.iter_mut().enumerate() {
        loop {
            match k.try_wait().map_err(|e| e.to_string())? {
                Some(st) if st.success() => break,
                Some(st) => {
                    failed.push(format!("seat {seat}'s peer ended: {st}"));
                    break;
                }
                None if began.elapsed() > limit => {
                    let _ = k.kill();
                    let _ = k.wait();
                    failed.push(format!("seat {seat}'s peer stalled; killed"));
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
    for (o, r) in outs.iter_mut().zip(readers) {
        *o = r.join().unwrap_or_default();
    }
    for proxy in proxies {
        let st = proxy.stop();
        println!(
            "proxy: {} received, {} forwarded, {} lost, {} reordered, {} duplicated, \
             {} spikes, delay {:.1} to {:.1} ms",
            st.received,
            st.forwarded,
            st.lost,
            st.reordered,
            st.duplicated,
            st.spikes,
            st.min_delay_us as f64 / 1000.0,
            st.max_delay_us as f64 / 1000.0
        );
    }
    if !failed.is_empty() {
        return Err(failed.join("; "));
    }
    let (_, want_end) = *want.last().unwrap();
    let want: Vec<_> = want.into_iter().filter(|(f, _)| *f < o.frames).collect();
    for (seat, out) in outs.iter().enumerate() {
        let r = report(out)?;
        println!("seat {seat}: {}", r.stats);
        if r.hashes != want {
            return Err(format!(
                "seat {seat}'s hashes {:x?}, the straight run's {want:x?}",
                r.hashes
            ));
        }
        if !r.desyncs.is_empty() {
            return Err(format!("seat {seat} desynced at {:?}", r.desyncs));
        }
        let end = r.end.ok_or(format!("seat {seat} gave no end hash"))?;
        if end != want_end {
            return Err(format!(
                "seat {seat} ended at {end:016x}, straight {want_end:016x}"
            ));
        }
    }
    println!(
        "pass: {} hashes and the end match the straight run on all {} seats",
        want.len(),
        o.players
    );
    Ok(())
}

/// `gbaa-desktop` peers through a local server, each joiner behind a
/// proxy of its own (`desktop` above).
fn desktop(profile_path: &Path, rom: &str, extra: &[&str]) -> Result<(), String> {
    const MAX: usize = runtime::link::MAX_PLAYERS;
    let profile = Profile::load(profile_path)?;
    let mut exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("gbaa-desktop");
    let mut both = Vec::new();
    let mut own: [Vec<String>; MAX] = Default::default();
    let mut players = 2;
    let mut it = extra.iter();
    while let Some(&a) = it.next() {
        let mut one = |seat: usize, flag: &str| -> Result<(), String> {
            let v = it.next().ok_or(format!("{a} needs a value"))?;
            own[seat].extend([flag.to_string(), v.to_string()]);
            Ok(())
        };
        // `--input3`: seat 3's (from 1) `--input`, and so on.
        let numbered = ["--input", "--save", "--menu", "--dump-dir", "--desync-at"]
            .into_iter()
            .find_map(|f| {
                let n: usize = a.strip_prefix(f)?.parse().ok()?;
                (1..=MAX).contains(&n).then_some((f, n - 1))
            });
        match a {
            _ if numbered.is_some() => {
                let (flag, seat) = numbered.unwrap();
                one(seat, flag)?
            }
            // The host's to set.
            "--delay" => one(0, "--delay")?,
            "--players" => {
                let v = it.next().ok_or("--players needs a value")?;
                players = v
                    .parse()
                    .ok()
                    .filter(|n| (2..=MAX).contains(n))
                    .ok_or(format!("--players 2 to {MAX}, not {v}"))?;
                own[0].extend(["--players".to_string(), v.to_string()]);
            }
            "--exe" => exe = it.next().ok_or("--exe needs a value")?.into(),
            a => both.push(a.to_string()),
        }
    }
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    // The host goes to the server itself; each joiner to a proxy in front
    // of it.
    let sock = UdpSocket::bind(local).map_err(|e| e.to_string())?;
    let server = sock.local_addr().map_err(|e| e.to_string())?;
    std::thread::spawn(move || relay::rooms::serve(sock, 0));
    let mut proxies = Vec::new();
    for seat in 1..players as u64 {
        // Each path its own randomness.
        let p = Profile {
            seed: profile.seed.wrapping_add(seat),
            ..profile.clone()
        };
        proxies.push(Proxy::to(p, local, server).map_err(|e| format!("proxy: {e}"))?);
    }
    let spawn = |seat: usize, addr: SocketAddr, role: &[String]| -> Result<Child, String> {
        let mut c = Command::new(&exe);
        c.arg(rom).arg("--server").arg(addr.to_string()).args(role);
        c.args(&both).args(&own[seat]).stdout(Stdio::piped());
        c.spawn().map_err(|e| format!("{}: {e}", exe.display()))
    };
    let mut host = spawn(0, server, &["--host".into()])?;
    let mut out = BufReader::new(host.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).map_err(|e| e.to_string())?;
    let Some(code) = line.trim().strip_prefix("room ").map(str::to_string) else {
        let _ = host.kill();
        return Err(format!("the host gave no room code: {line:?}"));
    };
    println!("room {code}");
    // A joiner takes the next free seat as its ask reaches the server: one
    // at a time, so each gets its own options' seat (a proxy's spike could
    // still swap two, which changes nothing the check compares).
    let mut guests = Vec::new();
    for (seat, proxy) in (1..players).zip(&proxies) {
        if seat > 1 {
            std::thread::sleep(Duration::from_secs(1));
        }
        guests.push(spawn(
            seat,
            proxy.addrs()[0],
            &["--join".into(), code.clone()],
        )?);
    }
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).map_err(|e| e.to_string())?;
    let _ = host.wait();
    let mut outs = vec![rest];
    for g in guests {
        let g = g.wait_with_output().map_err(|e| e.to_string())?;
        outs.push(String::from_utf8_lossy(&g.stdout).into_owned());
    }
    for p in proxies {
        let _ = p.stop();
    }
    let mut ends = Vec::new();
    for (seat, o) in outs.iter().enumerate() {
        for l in o
            .lines()
            .filter(|l| l.starts_with("netplay") || l.starts_with("desync"))
        {
            println!("seat {}: {l}", seat + 1);
        }
        ends.extend(
            o.lines()
                .filter_map(|l| l.split(", end ").nth(1))
                .map(|e| e.split(',').next().unwrap_or("").to_string()),
        );
    }
    match ends.as_slice() {
        // Windows closed early say nothing to compare.
        e if e.len() < players => Ok(()),
        [a, rest @ ..] if rest.iter().all(|e| e == a) => Ok(()),
        e => Err(format!("the peers ended apart: {}", e.join(", "))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = (|| {
        // `desktop`'s arguments are the desktop's own, after `--`.
        let (args, desk) = match args.iter().position(|a| a == "--") {
            Some(i) if args.first().is_some_and(|a| a == "desktop") => {
                (args[..i].to_vec(), args[i..].to_vec())
            }
            _ => (args.clone(), Vec::new()),
        };
        let (o, mut rest) = options(&args)?;
        rest.extend(desk);
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
        match rest.as_slice() {
            ["run", profile] => run(Path::new(profile), &o),
            ["proxy", profile, a, b] => {
                let p = Profile::load(Path::new(profile))?;
                let addr = |s: &str| s.parse::<SocketAddr>().map_err(|_| format!("{s}?"));
                let proxy = Proxy::start(p, addr(a)?, addr(b)?).map_err(|e| e.to_string())?;
                let [a, b] = proxy.addrs();
                println!("proxy: A sends to {a}, B to {b}");
                loop {
                    std::thread::park();
                }
            }
            ["room", profile] => run_room(Path::new(profile), &o),
            ["room-peer", seat, to, code @ ..] if code.len() <= 1 => {
                let seat = seat.parse().map_err(|_| format!("seat {seat}?"))?;
                let to = to.parse().map_err(|_| format!("{to}?"))?;
                room_peer(seat, to, code.first().copied(), &o)
            }
            ["peer", seat, to] => {
                let seat = seat.parse().map_err(|_| format!("seat {seat}?"))?;
                let to = to.parse().map_err(|_| format!("{to}?"))?;
                peer(seat, to, &o)
            }
            ["desktop", profile, rom, extra @ ..] => {
                let extra = match extra {
                    ["--", e @ ..] => e,
                    e => e,
                };
                desktop(Path::new(profile), rom, extra)
            }
            ["straight"] => {
                for (f, h) in straight(&o) {
                    println!("hash {f} {h:016x}");
                }
                Ok(())
            }
            _ => Err(USAGE.to_string()),
        }
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gbaa-pingsim: {e}");
            ExitCode::FAILURE
        }
    }
}
