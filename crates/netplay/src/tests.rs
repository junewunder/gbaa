use crate::demo::{Toy, gba, key_folder, script, straight};
use crate::msg::Msg;
use crate::sim::{Conditions, Endpoint, Net, play};
use crate::*;

const BAD: Conditions = Conditions {
    latency: 40_000,
    jitter: 30_000,
    loss: 0.25,
};

#[test]
fn messages_round_trip() {
    let ms = [
        Msg::State {
            frame: 180,
            build: [7; 32],
            data: vec![1, 2, 3],
        },
        Msg::Start {
            delay: 4,
            clock: -5,
        },
        Msg::Input {
            first: 9,
            inputs: vec![1, 0, 2, 0],
            have: vec![12, 13],
        },
        Msg::Hash {
            frame: 60,
            hash: u64::MAX - 1,
        },
    ];
    for m in ms {
        let b = m.encode();
        assert_eq!(Msg::decode(&b), Some(m.clone()));
        assert_eq!(Msg::decode(&b[..b.len() - 1]), None);
        let mut long = b.clone();
        long.push(0);
        assert_eq!(Msg::decode(&long), None);
    }
    assert_eq!(Msg::decode(&[]), None);
    assert_eq!(Msg::decode(&[9]), None);
    assert_eq!(
        Msg::decode(&[3, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255]),
        None
    );
}

#[test]
fn another_rom_is_another_build() {
    let a = gba(key_folder(), 2, None);
    let b = gba(key_folder()[..28].to_vec().leak(), 2, None);
    assert_ne!(a.build_id(), b.build_id());
}

#[test]
fn a_speaker_hears_its_seat_and_changes_nothing() {
    use crate::gba::Speaker;
    use std::cell::Cell;
    use std::rc::Rc;
    struct Count(Rc<Cell<(u32, u32)>>);
    impl Speaker for Count {
        fn audio(&mut self, _: &runtime::AudioChunk<'_>) {
            let (a, f) = self.0.get();
            self.0.set((a + 1, f));
        }
        fn end_frame(&mut self) {
            let (a, f) = self.0.get();
            self.0.set((a, f + 1));
        }
    }
    let rom = key_folder();
    let heard = Rc::new(Cell::new((0, 0)));
    let mut loud = gba(rom, 2, None);
    loud.machines_mut()[1].platform.speaker = Some(Box::new(Count(heard.clone())));
    let mut quiet = gba(rom, 2, None);
    for f in 0..120 {
        let keys = [script(0, f), script(1, f)];
        loud.step(&keys).unwrap();
        quiet.step(&keys).unwrap();
    }
    assert_eq!(loud.hash(), quiet.hash());
    let (chunks, frames) = heard.get();
    assert_eq!(frames, 120);
    assert!(chunks >= 120);
}

#[test]
fn frames_run_quietly_are_not_drawn_and_change_nothing() {
    let rom = key_folder();
    let mut drawn = gba(rom, 2, None);
    let mut quiet = gba(rom, 2, None);
    let step = |g: &mut crate::gba::Gba, f| g.step(&[script(0, f), script(1, f)]).unwrap();
    for f in 0..30 {
        step(&mut drawn, f);
        step(&mut quiet, f);
    }
    // No colour the PPU draws has bit 15 set.
    quiet.machines_mut()[0].ppu.frame.fill(0xFFFF);
    quiet.quiet(true);
    for f in 30..90 {
        step(&mut drawn, f);
        step(&mut quiet, f);
    }
    assert!(
        quiet.machines()[0].ppu.frame.iter().all(|&c| c == 0xFFFF),
        "a quiet frame drew"
    );
    assert_eq!(drawn.hash(), quiet.hash());
    quiet.quiet(false);
    step(&mut drawn, 90);
    step(&mut quiet, 90);
    assert!(quiet.machines()[0].ppu.frame[..] == drawn.machines()[0].ppu.frame[..]);
}

// Rollback (crate::rollback).

type RollPeers<C> = Vec<Rollback<C, Endpoint>>;

fn rollback_session<C: Snapshot>(
    consoles: Vec<C>,
    cond: Conditions,
    seed: u64,
    config: Config,
) -> (RollPeers<C>, std::rc::Rc<std::cell::RefCell<Net>>) {
    let net = Net::new(consoles.len(), cond, seed);
    let peers = consoles
        .into_iter()
        .enumerate()
        .map(|(s, c)| {
            let host = (s == 0).then_some(config);
            Rollback::new(c, Net::endpoint(&net, s), s, host, rollback::WINDOW)
        })
        .collect();
    (peers, net)
}

/// Keep the finished peers answering for a simulated second, so every
/// input arrives and every guess is put right.
fn settle<C: Snapshot>(peers: &mut RollPeers<C>, net: &std::rc::Rc<std::cell::RefCell<Net>>) {
    let t0 = net.borrow().now();
    for t in (t0..t0 + 1_000_000).step_by(sim::RETRY_US as usize) {
        net.borrow_mut().advance_to(t);
        for p in peers.iter_mut() {
            p.pump().unwrap();
        }
    }
}

/// Every peer's hashes are the straight run's (all but the hash frames
/// not yet certain when it stopped, none at all missing after `settle`),
/// and every peer ends on the straight run's consoles.
fn assert_rolled_straight<C: Snapshot>(peers: &mut RollPeers<C>, want: &[(u32, u64)], frames: u32) {
    let (last, end) = want.last().copied().unwrap();
    assert_eq!(last, frames);
    for p in peers.iter_mut() {
        let got: Vec<_> = p.hashes().iter().filter(|(f, _)| *f < frames).collect();
        let want: Vec<_> = want.iter().filter(|(f, _)| *f < frames).collect();
        assert_eq!(got, want, "seat {}", p.seat());
        assert!(
            p.desyncs().is_empty(),
            "seat {}: {:?}",
            p.seat(),
            p.desyncs()
        );
        assert_eq!(p.console_mut().hash(), end, "seat {}'s end", p.seat());
    }
}

fn toy_rollback(
    seats: usize,
    cond: Conditions,
    delay: u8,
    frames: u32,
    seed: u64,
) -> RollPeers<Toy> {
    let config = Config {
        delay,
        ..Config::default()
    };
    let consoles = (0..seats).map(|s| Toy::peer(seats, s)).collect();
    let (mut peers, net) = rollback_session(consoles, cond, seed, config);
    play(&mut peers, &net, frames, script, |_, _| {}).unwrap();
    settle(&mut peers, &net);
    let want = straight(Toy::new(seats), delay as u32, frames);
    assert_rolled_straight(&mut peers, &want, frames);
    peers
}

#[test]
fn rollback_locally_never_rolls_back() {
    for p in toy_rollback(2, Conditions::LOCAL, 1, 600, 1) {
        assert_eq!(p.stats.rollbacks, 0, "{:?}", p.stats);
    }
}

#[test]
fn rollback_at_delay_zero_over_a_bad_network_is_a_straight_run() {
    for seed in 1..6 {
        let peers = toy_rollback(2, BAD, 0, 1200, seed);
        assert!(peers.iter().all(|p| p.stats.rollbacks > 0));
    }
}

#[test]
fn rollback_with_three_or_four_seats_is_a_straight_run() {
    toy_rollback(3, BAD, 1, 900, 7);
    toy_rollback(4, BAD, 0, 900, 9);
}

#[test]
fn rollback_at_delay_zero_locally_is_a_straight_run() {
    toy_rollback(2, Conditions::LOCAL, 0, 300, 2);
}

#[test]
fn another_build_is_refused() {
    let mut b = Toy::peer(2, 1);
    b.build = 2;
    let (mut peers, net) = rollback_session(
        vec![Toy::peer(2, 0), b],
        Conditions::LOCAL,
        1,
        Config::default(),
    );
    let e = play(&mut peers, &net, 10, script, |_, _| {}).unwrap_err();
    assert!(matches!(e, Error::Build { .. }), "{e:?}");
}

#[test]
fn a_desync_is_seen_by_every_peer() {
    let frames = 600;
    let consoles = (0..2).map(|s| Toy::peer(2, s)).collect();
    // Locally, at delay 1, every input is in before its frame runs.
    let config = Config {
        delay: 1,
        ..Config::default()
    };
    let (mut peers, net) = rollback_session(consoles, Conditions::LOCAL, 5, config);
    // Seat 1's peer gets seat 0's console wrong once frame 100 and all
    // before it are certain, so no rollback can put it right.
    let mut broken = false;
    play(&mut peers, &net, frames, script, |s, p| {
        if s == 1 && !broken && p.frame() >= 100 && p.confirmed() == p.frame() {
            p.console_mut().st[0] ^= 1;
            broken = true;
        }
    })
    .unwrap();
    settle(&mut peers, &net);
    assert!(broken);
    let want = straight(Toy::new(2), 1, frames);
    let (a, b) = (peers[0].hashes(), peers[1].hashes());
    // Right until it broke, apart after, and both peers say so.
    assert_eq!(a[..2], want[..2]);
    assert_eq!(b[..2], want[..2]);
    assert_ne!(a[3], b[3]);
    assert_eq!(peers[0].desyncs(), peers[1].desyncs());
    assert!(peers[0].desyncs().len() >= 5, "{:?}", peers[0].desyncs());
}

#[test]
fn a_skewed_input_is_a_desync_both_peers_see() {
    let frames = 600;
    let consoles = (0..2).map(|s| Toy::peer(2, s)).collect();
    // Over a bad network, so rollbacks run the skewed frames again too.
    let (mut peers, net) = rollback_session(consoles, BAD, 4, Config::default());
    peers[1].skew(200, 0x155);
    play(&mut peers, &net, frames, script, |_, _| {}).unwrap();
    settle(&mut peers, &net);
    let want = straight(Toy::new(2), 0, frames);
    let (a, b) = (peers[0].hashes(), peers[1].hashes());
    // Seat 0's peer ran the real inputs: the straight run.
    let real: Vec<_> = a.iter().filter(|(f, _)| *f < frames).collect();
    let straight: Vec<_> = want.iter().filter(|(f, _)| *f < frames).collect();
    assert_eq!(real, straight);
    // Seat 1's, right up to the skew and apart after it.
    assert_eq!(b[..4], want[..4]);
    assert_ne!(b[5], want[5]);
    assert_eq!(peers[0].desyncs(), peers[1].desyncs());
    assert!(
        peers[0]
            .desyncs()
            .first()
            .is_some_and(|&f| f > 200 && f <= 300)
    );
}

#[test]
fn rollback_keeps_two_peers_level() {
    // 50 ms each way: both peers should guess about as far, ~3 frames.
    let cond = Conditions {
        latency: 50_000,
        jitter: 5_000,
        loss: 0.0,
    };
    for p in toy_rollback(2, cond, 0, 1800, 3) {
        let per = p.stats.rerun as f64 / p.stats.rollbacks.max(1) as f64;
        assert!(per < 5.0, "seat {}: {:?}", p.seat(), p.stats);
    }
}

#[test]
fn rolled_back_linked_gbas_are_a_straight_run() {
    let rom = key_folder();
    let frames = 600;
    let mut idle = gba(rom, 2, None);
    idle.set_clock(Config::default().clock);
    let want = straight(idle, 0, frames);
    // The keys matter: a run without them ends elsewhere.
    let mut idle = gba(rom, 2, None);
    for _ in 0..frames {
        idle.step(&[0, 0]).unwrap();
    }
    assert_ne!(idle.hash(), want.last().unwrap().1);
    let consoles = (0..2).map(|s| gba(rom, 2, Some(s))).collect();
    let (mut peers, net) = rollback_session(consoles, BAD, 5, Config::default());
    play(&mut peers, &net, frames, script, |_, _| {}).unwrap();
    settle(&mut peers, &net);
    assert_rolled_straight(&mut peers, &want, frames);
    assert!(peers.iter().all(|p| p.stats.rollbacks > 0));
    // Each peer's copy of seat 1's cartridge save is seat 1's.
    for p in &peers {
        assert_eq!(p.console().machines()[1].bus.save.data[..64], [2; 64]);
    }
}

#[test]
fn a_gba_snapshot_restores_exactly() {
    let rom = key_folder();
    let mut g = gba(rom, 2, None);
    for f in 0..30 {
        g.step(&[script(0, f), script(1, f)]).unwrap();
    }
    let mut snap = Default::default();
    g.snapshot(&mut snap);
    let at = g.hash();
    assert_eq!(g.snap_hash(&snap), at);
    let run = |g: &mut crate::gba::Gba| {
        for f in 30..90 {
            g.step(&[script(0, f), script(1, f)]).unwrap();
        }
        g.hash()
    };
    let first = run(&mut g);
    g.restore(&snap).unwrap();
    assert_eq!(g.hash(), at);
    assert_eq!(run(&mut g), first);
}

/// A recompiled build with no blocks: every key goes to the dispatcher's
/// interpreter fallback.
static NO_CODE: runtime::dispatch::Code<crate::gba::Pad> = runtime::dispatch::Code {
    blocks: &[],
    ram_blocks: &[],
    overlays: &[],
    extents: &[],
    ram_extents: &[],
    inner: &[],
    inner_extents: &[],
    modded: &[],
    folded: &[],
    rom_hash: 0,
    timing: runtime::Timing::Exact,
    seeds_file: None,
    spins: &[],
};

#[test]
fn recompiled_consoles_are_another_build_and_roll_back() {
    let rom = key_folder();
    let mut i = crate::gba::Gba::new(rom, &[None, None]);
    let mut c =
        crate::gba::Gba::with_code(rom, &[None, None], Some(&NO_CODE), runtime::Timing::Exact);
    assert!(c.recompiled() && !i.recompiled());
    assert_ne!(i.build_id(), c.build_id());
    assert_eq!(i.build_id()[16..], c.build_id()[16..]);
    let mut snap = Default::default();
    let mut at = 0;
    for f in 0..90 {
        if f == 30 {
            c.snapshot(&mut snap);
            at = c.hash();
        }
        c.step(&[script(0, f), script(1, f)]).unwrap();
    }
    let end = c.hash();
    c.restore(&snap).unwrap();
    assert_eq!(c.hash(), at);
    for f in 30..90 {
        c.step(&[script(0, f), script(1, f)]).unwrap();
    }
    assert_eq!(c.hash(), end);
    // The fallback is the interpreter: the same consoles.
    for f in 0..90 {
        i.step(&[script(0, f), script(1, f)]).unwrap();
    }
    assert_eq!(i.hash(), end);
}

/// [`NO_CODE`] generated with `--timing instant`.
static NO_CODE_INSTANT: runtime::dispatch::Code<crate::gba::Pad> = runtime::dispatch::Code {
    timing: runtime::Timing::Instant,
    ..NO_CODE
};

#[test]
fn timing_is_part_of_the_build_id() {
    use crate::gba::Gba;
    use runtime::Timing::{Exact, Instant};
    let rom = key_folder();
    let id = |code, timing| Gba::with_code(rom, &[None, None], code, timing).build_id();
    // The code's own timing, with the fallback's the same.
    assert_ne!(id(Some(&NO_CODE), Exact), id(Some(&NO_CODE_INSTANT), Exact));
    // The interpreter's, on the same code and without any.
    assert_ne!(id(Some(&NO_CODE), Exact), id(Some(&NO_CODE), Instant));
    assert_ne!(id(None, Exact), id(None, Instant));
    assert_eq!(id(None, Exact), Gba::new(rom, &[None, None]).build_id());
}

/// Two seats' transports on this machine, A's datagrams to `a_to` and B's
/// to `b_to` (each other, or a proxy's sides).
fn udp_pair(
    socks: [std::net::UdpSocket; 2],
    a_to: std::net::SocketAddr,
    b_to: std::net::SocketAddr,
) -> (udp::Udp, udp::Udp) {
    let [sa, sb] = socks;
    let a = udp::Udp::new(sa, 0, &[a_to, a_to]).unwrap();
    let b = udp::Udp::new(sb, 1, &[b_to, b_to]).unwrap();
    (a, b)
}

fn local_sock() -> std::net::UdpSocket {
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap()
}

/// Polls both until `done` or `secs` pass; what B received.
fn pump(
    a: &mut udp::Udp,
    b: &mut udp::Udp,
    secs: f64,
    mut done: impl FnMut(&udp::Udp, &[Vec<u8>]) -> bool,
) -> Vec<Vec<u8>> {
    let start = std::time::Instant::now();
    let mut got = Vec::new();
    while start.elapsed().as_secs_f64() < secs && !done(a, &got) {
        while a.recv().is_some() {}
        while let Some((_, m)) = b.recv() {
            got.push(m);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    got
}

#[test]
fn a_silent_peer_backs_the_resends_off() {
    let (sa, sb) = (local_sock(), local_sock());
    let (a_at, b_at) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
    let (mut a, mut b) = udp_pair([sa, sb], b_at, a_at);
    // A round trip of well under a millisecond: the timeout at its floor.
    for i in 0..20u8 {
        a.send(To::Seat(1), Delivery::Reliable, vec![i]);
        pump(&mut a, &mut b, 1.0, |a, _| a.unacked() == 0);
    }
    assert_eq!(a.stats.resent, 0);
    // B stops answering: a chunk resent every 20 ms would go 60 times in
    // 1.3 s; doubling, 6.
    a.send(To::Seat(1), Delivery::Reliable, vec![1; 100]);
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < 1.3 {
        while a.recv().is_some() {}
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        (1..=8).contains(&a.stats.resent),
        "{} resent",
        a.stats.resent
    );
    // B answers again, and the chunk is in.
    let got = pump(&mut a, &mut b, 3.0, |a, _| a.unacked() == 0);
    assert_eq!(got.last(), Some(&vec![1; 100]));
}

#[test]
fn a_big_message_over_a_lossy_path_resends_about_what_was_lost() {
    let profile = proxy::Profile {
        latency_ms: 20.0,
        jitter_ms: 30.0,
        loss: 0.1,
        seed: 7,
        ..Default::default()
    };
    let any: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let p = proxy::Proxy::start(profile, any, any).unwrap();
    let [pa, pb] = p.addrs();
    let (mut a, mut b) = udp_pair([local_sock(), local_sock()], pa, pb);
    // The proxy learns each side from what it sends first.
    for _ in 0..20 {
        a.send(To::Seat(1), Delivery::Unreliable, vec![0]);
        b.send(To::Seat(0), Delivery::Unreliable, vec![0]);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    pump(&mut a, &mut b, 0.5, |_, _| false);
    // A start state's size: 341 chunks, 64 in flight.
    let big: Vec<u8> = (0..409_000u32).map(|i| (i * 7 + i / 251) as u8).collect();
    a.send(To::Seat(1), Delivery::Reliable, big.clone());
    let got = pump(&mut a, &mut b, 30.0, |a, got| {
        a.unacked() == 0 && got.iter().any(|m| m.len() == big.len())
    });
    let stats = p.stop();
    assert!(got.contains(&big), "the message never arrived whole");
    // Each chunk lost (a tenth of 341, and its ack's loss) is resent about
    // once; a jittery round trip over the timeout costs a few more.
    let lost = stats.lost;
    assert!(
        a.stats.resent <= 2 * lost + 40,
        "{} resent for {lost} lost",
        a.stats.resent
    );
}

#[test]
fn an_unplugged_gba_plays_its_seat_alone() {
    let rom = key_folder();
    let mut g = gba(rom, 2, None);
    for _ in 0..10 {
        g.step(&[1, 2]).unwrap();
    }
    assert_eq!(g.alone(), None);
    g.unplug(1);
    assert_eq!(g.alone(), Some(1));
    let at = |g: &crate::gba::Gba| [g.machines()[0].now(), g.machines()[1].now()];
    let before = at(&g);
    let word = |g: &mut crate::gba::Gba| g.machines_mut()[1].bus.read32(0x0200_0000);
    let w = word(&mut g);
    for _ in 0..10 {
        g.step(&[1, 2]).unwrap();
    }
    let after = at(&g);
    // Seat 0's console is left where its player left it; seat 1's runs on.
    assert_eq!(after[0], before[0]);
    assert!(after[1] >= before[1] + 9 * runtime::sched::FRAME_CYCLES);
    assert_ne!(word(&mut g), w);
}

#[test]
fn a_seat_that_says_goodbye_or_goes_quiet_has_left() {
    use std::time::Duration;
    use udp::Gone;
    let (sa, sb) = (local_sock(), local_sock());
    let (a_at, b_at) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
    let (mut a, mut b) = udp_pair([sa, sb], b_at, a_at);
    let long = Duration::from_secs(60);
    b.send(To::Seat(0), Delivery::Unreliable, vec![1]);
    pump(&mut b, &mut a, 0.2, |_, got| !got.is_empty());
    assert_eq!(a.gone(long), None);
    // B sends nothing for a while: quiet, but only past the time given.
    std::thread::sleep(Duration::from_millis(150));
    while a.recv().is_some() {}
    assert_eq!(a.gone(Duration::from_millis(100)), Some((1, Gone::Quiet)));
    assert_eq!(a.gone(long), None);
    // A datagram waiting counts, though nothing has taken it in yet: a
    // peer stopped a while judges by what came meanwhile.
    std::thread::sleep(Duration::from_millis(150));
    b.send(To::Seat(0), Delivery::Unreliable, vec![3]);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(a.gone(Duration::from_millis(100)), None);
    assert_eq!(a.recv(), Some((1, vec![3])));
    // A says goodbye: B hears it at once, and A sends nothing more.
    a.leave();
    assert!(a.has_left());
    let sent = a.stats.datagrams;
    a.send(To::Seat(1), Delivery::Reliable, vec![2]);
    assert_eq!(a.stats.datagrams, sent);
    pump(&mut a, &mut b, 1.0, |_, _| false);
    assert_eq!(b.gone(long), Some((0, Gone::Said)));
}

#[test]
fn a_room_whose_players_leave_closes() {
    use std::time::Duration;
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server = sock.local_addr().unwrap();
    std::thread::spawn(move || relay::rooms::serve(sock, 0));
    let build = [5; 32];
    let (tx, rx) = std::sync::mpsc::channel();
    let host = std::thread::spawn(move || {
        udp::Udp::host(server, 2, &build, |c| tx.send(c).unwrap(), || true).unwrap()
    });
    let code = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let (mut b, _) = udp::Udp::join(server, code, &build, || true).unwrap();
    let (mut a, _) = host.join().unwrap();
    assert_eq!(relay::rooms::query(server).unwrap(), (1, 2));
    a.leave();
    let start = std::time::Instant::now();
    while b.gone(Duration::from_secs(60)).is_none() && start.elapsed() < Duration::from_secs(5) {
        while b.recv().is_some() {}
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(b.gone(Duration::from_secs(60)), Some((0, udp::Gone::Said)));
    // A leave is unacknowledged: a query may overtake it.
    let settles_at = |want| {
        let start = std::time::Instant::now();
        while relay::rooms::query(server).unwrap() != want
            && start.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(relay::rooms::query(server).unwrap(), want);
    };
    settles_at((1, 1));
    b.leave();
    settles_at((0, 0));
}

#[test]
fn a_server_too_new_for_this_build_refuses_it_plainly() {
    use std::time::Duration;
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server = sock.local_addr().unwrap();
    std::thread::spawn(move || relay::rooms::serve(sock, crate::VERSION + 1));
    let start = std::time::Instant::now();
    let Err(e) = udp::Udp::host(server, 2, &[5; 32], |_| {}, || true) else {
        panic!("seated by a server that needs a newer version");
    };
    assert!(start.elapsed() < Duration::from_secs(5), "{e}");
    let v = crate::VERSION;
    assert_eq!(
        e,
        format!(
            "{}this game is too old for the server (version {v}, needs {}): get the new build",
            udp::REFUSED,
            v + 1
        )
    );
}

/// fly's server lets in this build and refuses the one before.
#[test]
fn fly_refuses_versions_before_this_one() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/fly/Dockerfile");
    let docker = std::fs::read_to_string(path).unwrap();
    let want = format!("\"--min-version\", \"{}\"", crate::VERSION);
    assert!(
        docker.contains(&want),
        "deploy/fly/Dockerfile's CMD should pass {want}: netplay::VERSION changed"
    );
}
