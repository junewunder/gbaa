//! Wireless play (`--server ADDR --wireless` with `--host` or `--join
//! CODE`): this window's GBA with a wireless adapter (the one Emerald's
//! Union Room and FireRed's wireless club use, [`runtime::link::rfu`]),
//! whose radio is a `gbaa-server` room's ([`relay::radio`]; option F of
//! `docs/research/multiplayer.md`), as `emerald-wireless --seat` runs it
//! (multiplayer/wireless-radio-netplay).
//!
//! Unlike netplay on the cable ([`crate::room`]), nothing is rolled back
//! and no peer runs another's console: each window runs only its own GBA,
//! in real time, as two real GBAs with adapters do, and the adapter's
//! messages ([`netplay::radio::Radio`]) reach the server's radio when the
//! network brings them. The host makes a room of `--players` seats (2 by
//! default), prints `room CODE` and waits until every seat is taken; each
//! seat's adapter then hears the others' through the room.
//!
//! The game starts from reset, its save kept as `--save`
//! keeps any game's, in the interpreter (a recompiled build's too). The
//! menu doesn't pause the game (the other players' games would time out
//! waiting on the radio): while it's open the game plays on with this
//! window's pad let go. The sound paces the window, or, muted and
//! headless, a frame clock: real time either way. No fast-forward,
//! rewind, save states or scripts.

use std::io::Write;
use std::time::{Duration, Instant};

use netplay::radio::Radio;
use runtime::io::key;
use runtime::link::rfu::Wireless;
use runtime::sched::FRAME_CYCLES;
use runtime::{Machine, Timing};

use crate::audio::Player;
use crate::dump::{pixels_hash, write_png};
use crate::fast::{self, Pacer};
use crate::link::Seat;
use crate::room::{Role, SEATS};
use crate::window::Screen;
use crate::{Options, dump_at_path, sav_file};

/// How long a headless host waits for the other players.
const WAIT: Duration = Duration::from_secs(600);

/// After the last frame, headless: how long the radio is kept up, so the
/// others hear what this adapter last said before it leaves.
const LINGER: Duration = Duration::from_secs(2);

/// Slices of a frame between which the radio is heard: what the adapter
/// said goes out, and what the radio said comes in, within a quarter
/// frame (as `emerald-wireless --seat`).
const SLICES: u64 = 4;

/// After this many frames (10 s) without a word from the radio, the
/// window says so: a server older than the radio (`W` datagrams) seats
/// players but never answers their adapters.
const SILENT_FOR: u64 = 600;
const SILENT: &str = "the server's radio hasn't answered: is the server older than the radio?";

/// The room's build: the same for every seat that runs this ROM.
fn build(rom: &[u8]) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[..20].copy_from_slice(b"gbaa-desktop radio\0\0");
    b[20..28].copy_from_slice(&runtime::dispatch::rom_hash(rom).to_le_bytes());
    b
}

/// Whether `o`'s other options go with `--wireless`.
pub fn check(o: &Options) -> Result<(), String> {
    if o.delay.is_some() || o.desync_at.is_some() || o.paced || o.menu != Default::default() {
        return Err(
            "--delay, --desync-at, --paced and --menu are rollback netplay's, not --wireless's"
                .into(),
        );
    }
    crate::link::check(o).map_err(|e| e.replace("linked play (--link)", "wireless play"))
}

/// Play this window's GBA with a networked wireless adapter, as `opts`
/// says, in the window `screen` or headless, its interpreter timed by
/// `timing`.
pub fn run(opts: &Options, timing: Timing, screen: Option<Screen>) -> Result<(), String> {
    let (Some(server), Some(role)) = (&opts.server, &opts.role) else {
        return Err("wireless play needs --server and --host or --join".into());
    };
    let rom = opts.load_rom()?;
    let title = |t: &str| {
        if let Some(s) = &screen {
            s.set_title(t);
        }
    };
    let addr = crate::room::address(server)?;
    let seats = opts.players.unwrap_or(SEATS);
    let until = Instant::now() + WAIT;
    let closed = std::cell::Cell::new(false);
    let go_on = || match &screen {
        Some(sc) => {
            let i = sc.input();
            closed.set(!i.open || i.leave);
            !closed.get()
        }
        None => Instant::now() < until,
    };
    title(match role {
        Role::Host => "gbaa: making a wireless room",
        Role::Join(_) => "gbaa: joining a wireless room",
    });
    let mut said = false;
    let r = match role {
        Role::Host => Radio::host(
            addr,
            seats,
            &build(rom),
            |code, filled, seats| {
                let code = String::from_utf8_lossy(&code).into_owned();
                if !said {
                    // What a script reads, and a player tells the others.
                    println!("room {code}");
                    let _ = std::io::stdout().flush();
                    said = true;
                }
                if let Some(s) = &screen {
                    s.set_title(&format!(
                        "gbaa: wireless room {code}, {filled} of {seats} players in, waiting"
                    ));
                }
            },
            go_on,
        ),
        Role::Join(code) => Radio::join(addr, *code, &build(rom), go_on).map(|r| (r, *code)),
    };
    let (mut radio, code) = match r {
        Err(_) if closed.get() => return Ok(()),
        r => r?,
    };
    let seat = radio.seat();
    let code = String::from_utf8_lossy(&code).into_owned();
    eprintln!(
        "gbaa-desktop: wireless room {code}, seat {}, the radio on {addr}",
        seat + 1
    );
    title(&format!("gbaa: wireless room {code}, player {}", seat + 1));

    let sound = (screen.is_some() && !opts.mute)
        .then(|| {
            Player::open()
                .map(|mut p| {
                    p.set_gain(opts.gain());
                    p
                })
                .inspect(|p| eprintln!("gbaa-desktop: audio: {}", p.describe()))
                .inspect_err(|e| eprintln!("gbaa-desktop: no audio ({e}); running muted"))
                .ok()
        })
        .flatten();
    // Without sound, a frame clock keeps real time, headless too: the
    // other players' games run in real time and time out on a silent radio.
    // Frame k is due k frames after the first; a late frame lets the next
    // ones catch up, up to [`Pacer::SLACK`] behind (as netplay's pacer).
    let mut due = sound.is_none().then(Instant::now);
    let mut m = Box::new(Machine::new(rom, Seat::new(opts.input.clone(), sound)));
    m.set_timing(timing);
    let mut sav = match &opts.save {
        Some(path) => Some(sav_file::SavFile::open(path, &mut m.bus.save)?),
        None => None,
    };
    let mut w = Wireless::networked(&mut m, seat as u8);
    let mut frames = 0;
    let mut open = true;
    let mut late = 0u64;
    let start = Instant::now();
    while opts.frames.is_none_or(|n| frames < n) && open {
        if screen.is_some()
            && let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        {
            m.bus.gpio.clock = t.as_secs() as i64;
        }
        for _ in 0..SLICES {
            w.run(&mut [&mut *m], FRAME_CYCLES / SLICES, |_, m| m.run_frame());
            radio.pump(&mut w, Instant::now());
        }
        frames += 1;
        if frames == SILENT_FOR && radio.stats.datagrams_in == 0 {
            eprintln!("gbaa-desktop: {SILENT}");
            title(&format!("gbaa: wireless room {code}: {SILENT}"));
        }
        if let Some(f) = &m.fault {
            return Err(format!("frame {frames}: {f}"));
        }
        if let Some(f) = &mut sav {
            f.frame(&mut m.bus.save)?;
        }
        if let Some(dir) = &opts.dump_dir
            && opts.dump_at.contains(&frames)
        {
            let path = dump_at_path(dir, frames);
            write_png(&path, &m.platform.frame).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if let Some(s) = &screen {
            s.show(&m.platform.frame, false);
        }
        // Real time: the sound's queue or the frame clock, the radio
        // heard while waiting.
        if let Some(p) = &mut m.platform.player {
            p.wait();
        }
        if let Some(d) = &mut due {
            *d += fast::frame();
            let now = Instant::now();
            if now > *d + fast::frame() {
                late += 1;
            }
            if now > *d + Pacer::SLACK {
                *d = now - Pacer::SLACK;
            }
            while let Some(left) = d.checked_duration_since(Instant::now()) {
                radio.pump(&mut w, Instant::now());
                std::thread::sleep(left.min(Duration::from_micros(500)));
            }
        }
        radio.pump(&mut w, Instant::now());
        if let Some(s) = &screen {
            let i = s.input();
            open = i.open && !i.leave;
            // The menu doesn't pause: the game plays on, the pad let go.
            m.platform.held = match s.menu_open() {
                true => 0,
                false => (i.keys | i.pads) & key::ALL,
            };
        }
    }
    if screen.is_none() || open {
        let until = Instant::now() + LINGER;
        while Instant::now() < until {
            radio.pump(&mut w, Instant::now());
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    radio.leave();
    if let Some(f) = &mut sav {
        f.flush(&mut m.bus.save)?;
        if screen.is_some() {
            crate::back_up_save(&f.path);
        }
    }
    if let Some(p) = &m.platform.player {
        eprintln!("gbaa-desktop: {}", p.summary());
    }
    if let Some(path) = &opts.dump {
        write_png(path, &m.platform.frame).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if opts.hash {
        println!(
            "frame {frames} hash {:016x}",
            pixels_hash(&m.platform.frame[..])
        );
    }
    let s = radio.stats;
    println!(
        "wireless: {frames} frames in {:.1} s ({late} more than a frame late), {} transfers; \
         radio {} up, {} down, {} datagrams in, {} out, {} resent",
        start.elapsed().as_secs_f64(),
        w.transfers,
        s.ups,
        s.downs,
        s.datagrams_in,
        s.datagrams_out,
        radio.resent()
    );
    Ok(())
}
