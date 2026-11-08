//! Whether you hear the other players' Kirbys (multiplayer/other-seat-sound):
//! Kirby & the Amazing Mirror's two-player game, seat 0 idle and seat 1
//! jumping in place, first on a straight run (two GBAs on a cable, no
//! rollback), then by rollback over a simulated wandering network, seat 0's
//! speaker on its peer. Prints what seat 0 hears, frame by frame, against
//! the straight runs. Needs the ROM and its save (`$GBAA_ROMS`, default
//! `~/ROMs/gba`), so it's ignored by default:
//! `cargo test --release -p netplay --test seat_sound -- --ignored --nocapture`.
//! The findings: docs/research/multiplayer.md, "The other players' sound".

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use netplay::gba::{Gba, Speaker};
use netplay::sim::{Conditions, Net, play};
use netplay::{Config, Console, Netplay, Rollback, rollback};
use runtime::AudioChunk;
use runtime::io::key;

/// Frames run: the menus end about frame 2200, then 23 s in Rainbow Route.
const FRAMES: u32 = 3600;
/// Seat 1 jumps from here.
const PLAY: u32 = 2400;

/// Each frame's samples, as heard.
#[derive(Default)]
struct Heard {
    now: Vec<(i16, i16)>,
    frames: Vec<Vec<(i16, i16)>>,
}

struct Ear(Rc<RefCell<Heard>>);

impl Speaker for Ear {
    fn audio(&mut self, chunk: &AudioChunk<'_>) {
        self.0.borrow_mut().now.extend_from_slice(chunk.samples);
    }

    fn end_frame(&mut self) {
        let mut h = self.0.borrow_mut();
        let f = std::mem::take(&mut h.now);
        h.frames.push(f);
    }
}

/// The menu presses of docs/research/multiplayer.md's Kirby runs (both
/// pick a file, Start Game, Multi Player; seat 0, the parent, starts the
/// game), then seat 1 jumps (A for 10 frames every 60) or not.
fn keys(seat: usize, f: u32, jump: bool) -> u16 {
    let at = |p: &[u32], len: u32| p.iter().any(|&s| (s..s + len).contains(&f));
    let mut k = 0;
    if at(&[400, 700], 4) {
        k |= key::START;
    }
    if at(&[800, 950, 1100], 4) || (seat == 0 && at(&[1820, 1950, 2100], 4)) {
        k |= key::A;
    }
    if at(&[1050], 4) {
        k |= key::DOWN;
    }
    if seat == 1 && jump && f >= PLAY && (f - PLAY) % 60 < 10 {
        k |= key::A;
    }
    k
}

fn rom() -> Option<(&'static [u8], &'static [u8])> {
    let dir = std::env::var_os("GBAA_ROMS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join("ROMs/gba"));
    let rom = std::fs::read(dir.join("Kirby & The Amazing Mirror.gba")).ok()?;
    let sav = std::fs::read(dir.join("Kirby & The Amazing Mirror.sav")).ok()?;
    Some((rom.leak(), sav.leak()))
}

fn console(rom: &'static [u8], sav: &'static [u8], ear: Option<&Rc<RefCell<Heard>>>) -> Gba {
    let mut g = Gba::new(rom, &[Some(sav), Some(sav)]);
    g.set_clock(Config::default().clock);
    if let Some(e) = ear {
        g.machines_mut()[0].platform.speaker = Some(Box::new(Ear(e.clone())));
    }
    g
}

/// A straight run: seat 0's and seat 1's sound, and seat 0's picture
/// from [`PLAY`] on.
struct Straight {
    ears: [Vec<Vec<(i16, i16)>>; 2],
    pictures: Vec<Vec<u16>>,
}

fn straight(rom: &'static [u8], sav: &'static [u8], jump: bool) -> Straight {
    let ears = [Rc::default(), Rc::default()];
    let mut g = console(rom, sav, None);
    for (m, e) in g.machines_mut().iter_mut().zip(&ears) {
        m.platform.speaker = Some(Box::new(Ear(Rc::clone(e))));
    }
    let mut pictures = Vec::new();
    for f in 0..FRAMES {
        g.step(&[keys(0, f, jump), keys(1, f, jump)]).unwrap();
        if f >= PLAY {
            pictures.push(g.machines()[0].ppu.frame.to_vec());
        }
    }
    let take = |e: &Rc<RefCell<Heard>>| std::mem::take(&mut e.borrow_mut().frames);
    Straight {
        ears: [take(&ears[0]), take(&ears[1])],
        pictures,
    }
}

/// Frames where `a` and `b` differ, from `start` on.
fn differ_from<T: PartialEq>(start: u32, a: &[T], b: &[T]) -> Vec<u32> {
    (start as usize..a.len().min(b.len()))
        .filter(|&f| a[f] != b[f])
        .map(|f| f as u32)
        .collect()
}

/// Frames from [`PLAY`] on where `a` and `b` (whole runs) differ.
fn differ<T: PartialEq>(a: &[T], b: &[T]) -> Vec<u32> {
    differ_from(PLAY, a, b)
}

/// Runs of consecutive frames: (first, length).
fn runs(fs: &[u32]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &f in fs {
        match out.last_mut() {
            Some((s, n)) if *s + *n == f => *n += 1,
            _ => out.push((f, 1)),
        }
    }
    out
}

#[test]
#[ignore = "needs Kirby & the Amazing Mirror; release build"]
fn kirby_seat_sound() {
    let Some((rom, sav)) = rom() else {
        eprintln!("no Kirby ROM or save: skipped");
        return;
    };
    let idle = straight(rom, sav, false);
    let jump = straight(rom, sav, true);
    let own = differ(&idle.ears[1], &jump.ears[1]);
    let other = differ(&idle.ears[0], &jump.ears[0]);
    // Seat 0's picture: frames it differs on, and pixels 20 frames in
    // (mid-jump: seat 1's Kirby in the air).
    let seen = differ_from(0, &idle.pictures, &jump.pictures).len();
    let pixels = (idle.pictures[20].iter().zip(&jump.pictures[20]))
        .filter(|(a, b)| a != b)
        .count();
    if let Some(dir) = std::env::var_os("SEAT_SOUND_PPM") {
        // Seat 0's picture 20 frames into the jumps, idle and jumping.
        for (name, run) in [("idle", &idle), ("jump", &jump)] {
            let mut ppm = b"P6 240 160 31\n".to_vec();
            for &c in &run.pictures[20] {
                ppm.extend([c & 31, c >> 5 & 31, c >> 10 & 31].map(|v| v as u8));
            }
            std::fs::write(PathBuf::from(&dir).join(format!("{name}.ppm")), ppm).unwrap();
        }
    }
    println!("straight, frames {PLAY}..{FRAMES}, seat 1 jumping vs idle:");
    println!(
        "  seat 1's sound differs on {} frames: {:?}",
        own.len(),
        &runs(&own)[..runs(&own).len().min(12)]
    );
    println!(
        "  seat 0's picture differs on {seen} frames; {pixels} pixels at frame {}",
        PLAY + 20
    );
    println!(
        "  seat 0's sound differs on {} frames: {:?}",
        other.len(),
        &runs(&other)[..runs(&other).len().min(12)]
    );

    // By rollback, seat 0's peer hearing seat 0.
    let wander = Conditions {
        latency: 60_000,
        jitter: 40_000,
        loss: 0.02,
    };
    let ear: Rc<RefCell<Heard>> = Rc::default();
    let consoles = vec![console(rom, sav, Some(&ear)), console(rom, sav, None)];
    let net = Net::new(2, wander, 3);
    let mut peers: Vec<_> = consoles
        .into_iter()
        .enumerate()
        .map(|(s, c)| {
            let host = (s == 0).then_some(Config::default());
            Rollback::new(c, Net::endpoint(&net, s), s, host, rollback::WINDOW)
        })
        .collect();
    play(&mut peers, &net, FRAMES, |s, f| keys(s, f, true), |_, _| {}).unwrap();
    let heard = std::mem::take(&mut ear.borrow_mut().frames);
    let s = peers[0].stats;
    println!(
        "rollback (60 ± 40 ms one way, 2% loss), seat 0's peer: {} rollbacks, {} frames run again",
        s.rollbacks, s.rerun
    );
    println!("  heard {} frames", heard.len());
    let wrong = differ_from(0, &heard, &jump.ears[0]);
    println!(
        "  seat 0's sound heard differs from the straight run's on {} frames: {:?}",
        wrong.len(),
        &runs(&wrong)[..runs(&wrong).len().min(12)]
    );
    let peer_end = peers[0].console_mut().hash();
    let mut g = console(rom, sav, None);
    for f in 0..FRAMES {
        g.step(&[keys(0, f, true), keys(1, f, true)]).unwrap();
    }
    println!(
        "  end hash matches the straight run's: {}",
        peer_end == g.hash()
    );
    // Seat 1's jumps sound on its own console and show on seat 0's, but
    // seat 0's console plays none of them; rollback loses nothing of it.
    assert!(!own.is_empty() && seen > 0 && pixels > 0);
    assert_eq!(other, []);
    assert_eq!(wrong, []);
    assert_eq!(peer_end, g.hash());
}
