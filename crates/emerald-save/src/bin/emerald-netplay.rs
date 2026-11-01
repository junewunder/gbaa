//! `emerald-netplay ROM SAVE_DIR [--frames N] [--delay D] [--latency MS]
//! [--jitter MS] [--loss P] [--seed N]`: the netplay session core's check
//! (multiplayer/session-core). Two linked Emeralds, `alice.sav` in seat 0
//! (the cable's parent) and `bob.sav` in seat 1 (`just emerald-saves`),
//! play emerald-link's walk into the Trade Center twice:
//!
//! - straight: one process, both GBAs on one cable ([`netplay::gba::Gba`]),
//!   each frame on what each player held D frames before;
//! - as a session: two peers by rollback ([`netplay::Rollback`]) on a
//!   simulated network ([`netplay::sim`]), each running both GBAs, each
//!   starting with only its own save and getting the other's state from
//!   its owner, inputs crossing with the latency, jitter and loss given
//!   (default 40 ms, up to 30 ms more, 10%), input delay D (default 0).
//!
//! It passes (exit 0) when both peers' hashes of both consoles, every 60
//! frames and at the end, are the straight run's, there was no desync, and
//! both players reached `LINK_STATE_CONN_ESTABLISHED` and the Trade Center
//! (so the cable was used). About two minutes.

use std::path::Path;
use std::process::ExitCode;

use emerald_save::{
    CONN_ESTABLISHED, G_LINK, Presses, SB1_PTR, TRADE_CENTER, TRADE_CENTER_PRESSES,
};
use netplay::gba::Gba;
use netplay::session::HASH_EVERY;
use netplay::sim::{Conditions, Net, RETRY_US, play};
use netplay::{Config, Console, Netplay, Rollback, rollback};

const USAGE: &str = "usage: emerald-netplay ROM SAVE_DIR [--frames N] [--delay D] \
                     [--latency MS] [--jitter MS] [--loss P] [--seed N]";

struct Options {
    rom: &'static [u8],
    saves: [Vec<u8>; 2],
    frames: u32,
    delay: u8,
    cond: Conditions,
    seed: u64,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut pos = Vec::new();
    let (mut frames, mut delay, mut seed) = (4200, 0, 1);
    let mut cond = Conditions {
        latency: 40_000,
        jitter: 30_000,
        loss: 0.1,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| USAGE.to_string());
        let ms = |v: &String| v.parse::<u64>().map(|m| m * 1000).map_err(|_| USAGE);
        match a.as_str() {
            "--frames" => frames = val()?.parse().map_err(|_| USAGE)?,
            "--delay" => delay = val()?.parse().map_err(|_| USAGE)?,
            "--latency" => cond.latency = ms(val()?)?,
            "--jitter" => cond.jitter = ms(val()?)?,
            "--loss" => cond.loss = val()?.parse().map_err(|_| USAGE)?,
            "--seed" => seed = val()?.parse().map_err(|_| USAGE)?,
            _ if !a.starts_with('-') => pos.push(a.clone()),
            _ => return Err(USAGE.into()),
        }
    }
    let [rom, saves] = &pos[..] else {
        return Err(USAGE.into());
    };
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let rom = read(Path::new(rom))?;
    if rom.get(0xAC..0xB0) != Some(b"BPEE") {
        return Err("not Pokémon Emerald (US): its game code isn't BPEE".into());
    }
    let saves = Path::new(saves);
    Ok(Options {
        rom: Box::leak(rom.into_boxed_slice()),
        saves: [
            read(&saves.join("alice.sav"))?,
            read(&saves.join("bob.sav"))?,
        ],
        frames,
        delay,
        cond,
        seed,
    })
}

/// Player `p`'s link state and whether it stands in the Trade Center.
fn where_is(g: &Gba, p: usize) -> (u8, bool) {
    let m = &g.machines()[p];
    let sb1 = m.bus.read32(SB1_PTR);
    let map = [m.bus.read8(sb1 + 4), m.bus.read8(sb1 + 5)];
    (m.bus.read8(G_LINK + 1), map == TRADE_CENTER)
}

fn run(o: &Options) -> Result<bool, String> {
    let presses = Presses::parse(TRADE_CENTER_PRESSES)?;
    let delay = o.delay as u32;
    let config = Config {
        delay: o.delay,
        ..Config::default()
    };

    // Straight.
    let mut g = Gba::new(o.rom, &[Some(&o.saves[0]), Some(&o.saves[1])]);
    g.set_clock(config.clock);
    let mut want = Vec::new();
    let (mut established, mut entered) = ([None; 2], [None; 2]);
    for f in 0..o.frames {
        if f.is_multiple_of(HASH_EVERY) {
            want.push((f, g.hash()));
        }
        let k = if f < delay {
            0
        } else {
            presses.keys((f - delay) as u64)
        };
        g.step(&[k, k])?;
        for p in 0..2 {
            let (state, there) = where_is(&g, p);
            if state == CONN_ESTABLISHED && established[p].is_none() {
                established[p] = Some(f + 1);
            }
            if there && entered[p].is_none() {
                entered[p] = Some(f + 1);
            }
        }
    }
    let end = g.hash();
    println!(
        "straight: {} frames, {} transfers, established at {established:?}, \
         in the Trade Center at {entered:?}, end {end:016x}",
        o.frames,
        g.link().transfers
    );

    // As a session.
    let net = Net::new(2, o.cond, o.seed);
    let mut peers: Vec<_> = (0..2)
        .map(|s| {
            let own: Vec<Option<&[u8]>> = (0..2)
                .map(|p| (p == s).then_some(&o.saves[p][..]))
                .collect();
            let host = (s == 0).then_some(config);
            let end = Net::endpoint(&net, s);
            Rollback::new(Gba::new(o.rom, &own), end, s, host, rollback::WINDOW)
        })
        .collect();
    let played = play(
        &mut peers,
        &net,
        o.frames,
        |_, f| presses.keys(f as u64),
        |_, _| {},
    )
    .map_err(|e| e.to_string())?;
    // A simulated second more, the peers answering, so every input arrives
    // and every guess is put right.
    let t0 = net.borrow().now();
    for t in (t0..t0 + 1_000_000).step_by(RETRY_US as usize) {
        net.borrow_mut().advance_to(t);
        for p in &mut peers {
            p.pump().map_err(|e| e.to_string())?;
        }
    }
    println!(
        "session: {:?} one way +{:?} jitter, {}% loss, delay {}: {:.1} s simulated for {:.1} s of \
         frames, {} waiting turns, {} held back; {} messages ({} lost, {} resent), {} KB",
        std::time::Duration::from_micros(o.cond.latency),
        std::time::Duration::from_micros(o.cond.jitter),
        o.cond.loss * 100.0,
        o.delay,
        played.time as f64 / 1e6,
        o.frames as f64 * netplay::sim::FRAME_US as f64 / 1e6,
        played.waits,
        played.skips,
        played.net.sent,
        played.net.lost,
        played.net.resent,
        played.net.bytes / 1024
    );
    let mut same = true;
    for p in &mut peers {
        let end_here = p.console_mut().hash();
        let got: Vec<_> = p.hashes().iter().filter(|(f, _)| *f < o.frames).collect();
        let want: Vec<_> = want.iter().collect();
        let first_off = got
            .iter()
            .zip(&want)
            .find(|(a, b)| a != b)
            .map(|(a, _)| a.0);
        println!(
            "seat {}: {} hashes, end {end_here:016x}, desyncs {:?}, first mismatch {first_off:?}, {}",
            p.seat(),
            got.len(),
            p.desyncs(),
            p.summary()
        );
        same &= got == want && end_here == end && p.desyncs().is_empty();
    }
    let linked = established.iter().all(Option::is_some) && entered.iter().all(Option::is_some);
    Ok(same && linked)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = match options(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("emerald-netplay: {e}");
            return ExitCode::from(2);
        }
    };
    match run(&o) {
        Ok(true) => {
            println!(
                "pass: both peers ran the straight run's frames, linked into the Trade Center"
            );
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("fail");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("emerald-netplay: {e}");
            ExitCode::FAILURE
        }
    }
}
