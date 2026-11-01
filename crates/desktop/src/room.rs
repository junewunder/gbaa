//! Netplay (`--server ADDR` with `--host` or `--join CODE`, or the
//! window menu's Netplay page, which asks for an [`Ask`]): two GBAs
//! running the same ROM on one link cable, as with `--link`, but each
//! player in their own window, maybe on another machine, through the
//! server's rooms (`gbaa-server`, crates/relay) and the netplay session
//! (crates/netplay; `docs/research/multiplayer.md`, "Recommended: the
//! layers").
//!
//! Every peer runs both GBAs, by rollback; only inputs (and,
//! at the start, each player's own console state) cross the network. The
//! host creates a two-seat room, prints its code and waits for the other
//! player; the host is seat 0, the cable's parent. The window shows this
//! player's seat alone. Its cartridge save is kept as `--save` keeps any
//! game's; the other seat's console, built from the state its owner sent,
//! is discarded at the end. This seat's sound plays and paces the window,
//! as the single window's does (or, muted, the frame clock). The menu
//! doesn't pause a session: while it's open the game plays on and this
//! seat's pad reads as let go, so the other player never waits on it.
//! It quits; fast-forward, rewind, save states and scripts don't go with
//! it.
//!
//! A player who quits or leaves the room says goodbye; one whose machine
//! stops answering (a crash, a lost connection) is taken to have left
//! after [`QUIET`]. Either way the other window says so, on the game and
//! on the Netplay page, and its own console plays on with the cable
//! pulled out ([`netplay::gba::Gba::unplug`]): the game shows its own
//! link error, as with a real cable. It leaves the room too, so the
//! server closes it.
//!
//! A recompiled game's binary runs both GBAs on its code (`--interp`: the
//! interpreter), which the build ID names: every peer must run the same
//! build, or every peer the interpreter. The cable ends each machine's
//! turn every scanline; each runs on a guest stack of its own
//! ([`crate::coro::Guests`]), which switches out there and goes on in
//! compiled code at the next turn, where unwinding to the dispatcher would
//! resume a structured function in the interpreter (Kirby's structured
//! build: 23% of cycles compiled that way). A rollback's restore ends
//! them, and new ones resume block-timed, as the old ones would have run.
//!
//! The end lines (the session's counts, frame times and how many ran over
//! a frame, the round trip, how far ahead of the other player this peer
//! ran) print at the end, show in the menu while it's open, and, from the
//! window, are kept in `$GBAA_DATA/netplay/` ([`keep_report`]).

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use netplay::gba::{Seat, Speaker};
use netplay::udp::Udp;
use netplay::{Config, Console, Netplay, Rollback, Status, rollback};
use runtime::io::key;
use runtime::{AudioChunk, Timing};

use crate::audio::Player;
use crate::dump::{pixels_hash, write_png};
use crate::fast::{self, Clock};
use crate::window::Screen;
use crate::{Code, Desktop, Options, dump_at_path, sav_file};

/// The session's GBAs, on the desktop's platform, which recompiled code is
/// built for.
type Gba = netplay::gba::Gba<Desktop>;

/// Seats in a room.
pub const SEATS: usize = 2;

/// While the menu is open, how often (in frames) its report of the
/// session is brought up to date.
const REPORT_EVERY: u64 = 30;

/// How long a headless host waits for the other player.
const WAIT: Duration = Duration::from_secs(600);

/// After the last frame, headless: how long to keep answering, so the
/// other peer gets the inputs and hashes it still lacks.
const LINGER: Duration = Duration::from_secs(2);

/// What `--host` or `--join` asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Host,
    Join([u8; relay::CODE_LEN]),
}

/// The server the menu's Netplay page offers until another is set: fly's
/// (`docs/tools/commands.md`, the server on fly.io).
pub const DEFAULT_SERVER: &str = "37.16.3.41";

/// A session the menu asked for: the server and the seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub server: String,
    pub role: Role,
}

/// Where this window's netplay is, as the menu shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// In no room.
    #[default]
    Off,
    /// Asking the server for a room.
    Making,
    /// The host, in room `code`, waiting for the other player.
    Waiting(String),
    /// Joining room `code`.
    Joining(String),
    /// Playing seat `seat` (from 0) of room `code`; `lines` say how it's
    /// going (the round trip, the lead, the rollbacks), as of the menu's
    /// last opening. `left` says who left, once the other player has: the
    /// game goes on alone, the cable unplugged. `quiet`: the other player
    /// has sent nothing for [`SHAKY`] (they may be about to leave).
    Playing {
        code: String,
        seat: usize,
        lines: Vec<String>,
        left: Option<String>,
        quiet: bool,
    },
    /// The last session ended, and why.
    Ended(String),
}

/// A room code as typed: six letters or digits, any case.
pub fn parse_code(v: &str) -> Result<[u8; relay::CODE_LEN], String> {
    let up = v.trim().to_ascii_uppercase();
    up.as_bytes()
        .try_into()
        .ok()
        .filter(|c: &[u8; relay::CODE_LEN]| c.iter().all(u8::is_ascii_alphanumeric))
        .ok_or(format!(
            "bad room code {v:?} ({} letters and digits)",
            relay::CODE_LEN
        ))
}

/// `$GBAA_DATA`, defaulting to `~/.local/share/gbaa`.
fn data_dir() -> Option<PathBuf> {
    Some(match std::env::var("GBAA_DATA") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/share/gbaa"),
    })
}

/// Whether `o`'s other options go with netplay.
pub fn check(o: &Options) -> Result<(), String> {
    if o.server.is_none() {
        return Err("--host and --join need --server ADDR".into());
    }
    if o.role.is_none() {
        return Err("--server needs --host or --join CODE".into());
    }
    if o.link.is_some() {
        return Err("--link is two players in one window; netplay is one".into());
    }
    if o.delay.is_some() && o.role != Some(Role::Host) {
        return Err("--delay is the host's to set".into());
    }
    crate::link::check(o).map_err(|e| e.replace("linked play (--link)", "netplay"))
}

impl Speaker for Player {
    fn audio(&mut self, chunk: &AudioChunk<'_>) {
        Player::audio(self, chunk);
    }

    fn end_frame(&mut self) {
        Player::end_frame(self);
    }
}

/// This seat's player, if it has one.
fn player(gba: &mut Gba, seat: usize) -> Option<&mut Player> {
    let s: &mut dyn std::any::Any = &mut **gba.machines_mut()[seat]
        .platform
        .pad_mut()
        .speaker
        .as_mut()?;
    s.downcast_mut()
}

/// The server's address: a host name or IP, and a port (default 7374,
/// `gbaa-server`'s).
fn address(v: &str) -> Result<SocketAddr, String> {
    let with_port;
    let a = if v.contains(':') {
        v
    } else {
        with_port = format!("{v}:{}", relay::PORT);
        &with_port
    };
    a.to_socket_addrs()
        .map_err(|e| format!("server {v}: {e}"))?
        .next()
        .ok_or(format!("server {v}: no address"))
}

/// Play this player's seat in a room, as `opts` says, in the window
/// `screen` or headless, on `code` if given, the interpreter timed by
/// `timing`.
pub fn run(
    opts: &Options,
    code: Option<&'static Code>,
    timing: Timing,
    screen: Option<Screen>,
) -> Result<(), String> {
    let (Some(server), Some(role)) = (&opts.server, &opts.role) else {
        return Err("netplay needs --server and --host or --join".into());
    };
    let rom = opts.load_rom()?;
    if let Some(c) = code
        && runtime::dispatch::rom_hash(rom) != c.rom_hash
    {
        return Err(format!(
            "{}: not the ROM this build was recompiled from",
            opts.rom.display()
        ));
    }
    let mut gba = Gba::with_code(rom, &[None; SEATS], code, timing);
    if code.is_some() {
        gba.set_turns(Box::<crate::coro::Guests>::default());
    }
    let title = |t: &str| {
        if let Some(s) = &screen {
            s.set_title(t);
        }
    };
    let say = |code: [u8; relay::CODE_LEN]| {
        let code = String::from_utf8_lossy(&code).into_owned();
        // What a script reads (the check does) and a player tells the
        // other.
        println!("room {code}");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        title(&format!("gbaa: room {code}, waiting for player 2"));
        if let Some(s) = &screen {
            s.set_netplay(State::Waiting(code));
        }
    };
    // Headless, for a while; the window, until its player closes it.
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
    // No input delay by default: your press reaches your console on the
    // next frame (june, 2026-10-27).
    let delay = opts.delay.unwrap_or(Config::default().delay);
    let addr = address(server)?;
    title(match role {
        Role::Host => "gbaa: making a room",
        Role::Join(_) => "gbaa: joining",
    });
    if let Some(s) = &screen {
        s.set_netplay(match role {
            Role::Host => State::Making,
            Role::Join(c) => State::Joining(String::from_utf8_lossy(c).into_owned()),
        });
    }
    let r = match role {
        Role::Host => Udp::host(addr, SEATS, &gba.build_id(), say, go_on),
        Role::Join(code) => Udp::join(addr, *code, &gba.build_id(), go_on),
    };
    let (net, code) = match r {
        Err(_) if closed.get() => return Ok(()),
        r => r?,
    };
    let seat = net.seat_of();
    if net.seats() != SEATS {
        return Err(format!("the room has {} seats, not {SEATS}", net.seats()));
    }
    play(opts, screen, gba, seat, code, delay, net)
}

/// How long the other player may send nothing before this peer takes
/// them to have left: a seated peer sends every frame, and this is under
/// the rollback window (5 s), so the game doesn't stop for someone gone.
pub const QUIET: Duration = Duration::from_secs(3);

/// How long the other player may send nothing before the window's mark
/// says it hears nothing from them (before [`QUIET`] takes them to have
/// left): a frame's inputs come every 17 ms.
pub const SHAKY: Duration = Duration::from_millis(750);

/// What a window says when player `who` (from 0) has left, `how`.
pub fn left_note(who: usize, how: netplay::udp::Gone) -> String {
    match how {
        netplay::udp::Gone::Said => format!("Player {} left the room", who + 1),
        netplay::udp::Gone::Quiet => {
            format!("Player {} stopped answering and left the room", who + 1)
        }
    }
}

/// Play seat `seat` of room `code` over `net`.
fn play(
    opts: &Options,
    screen: Option<Screen>,
    mut gba: Gba,
    seat: usize,
    code: [u8; relay::CODE_LEN],
    delay: u8,
    net: Udp,
) -> Result<(), String> {
    let title = |t: &str| {
        if let Some(s) = &screen {
            s.set_title(t);
        }
    };
    let code = String::from_utf8_lossy(&code).into_owned();
    let delay_said = if seat == 0 {
        format!("delay {delay}")
    } else {
        "the host's delay".into()
    };
    eprintln!(
        "gbaa-desktop: room {code}, seat {}, rollback, {delay_said}, {}",
        seat + 1,
        match gba.recompiled() {
            true => "recompiled",
            false => "interpreted",
        }
    );
    // This seat's cartridge save; the other's arrives in its owner's state.
    let mut sav = match &opts.save {
        Some(path) => Some(sav_file::SavFile::open(
            path,
            &mut gba.machines_mut()[seat].bus.save,
        )?),
        None => None,
    };
    let host = (seat == 0).then(|| Config {
        delay,
        // The window's clock is the host's wall clock; headless, the
        // runtime's fixed one, so runs repeat.
        clock: match (
            &screen,
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH),
        ) {
            (Some(_), Ok(t)) => t.as_secs() as i64,
            _ => runtime::gpio::FIXED_CLOCK,
        },
    });
    // The sound starts with the session, so none queues while the room
    // fills.
    if screen.is_some() && !opts.mute {
        match Player::open() {
            Ok(mut p) => {
                p.set_gain(opts.gain());
                eprintln!("gbaa-desktop: audio: {}", p.describe());
                gba.machines_mut()[seat].platform.pad_mut().speaker = Some(Box::new(p));
            }
            Err(e) => eprintln!("gbaa-desktop: no audio ({e}); running muted"),
        }
    }
    // Muted, the window keeps time by a frame clock.
    let mut clock = (screen.is_some() && player(&mut gba, seat).is_none()).then(Clock::default);
    let mut s = Rollback::new(gba, net, seat, host, rollback::WINDOW);
    title(&format!("gbaa: room {code}, player {}", seat + 1));
    // Who left, once someone has: this seat plays on alone.
    let mut left: Option<String> = None;
    let mut left_at: Option<(usize, u64)> = None;
    let playing = |lines, left: &Option<String>, quiet| State::Playing {
        code: code.clone(),
        seat,
        lines,
        left: left.clone(),
        quiet,
    };
    if let Some(sc) = &screen {
        sc.set_netplay(playing(Vec::new(), &left, false));
    }
    // The Netplay page's lines as last told, and whether the other player
    // had gone quiet: the window's mark says so as it changes.
    let mut glance: Vec<String> = Vec::new();
    let mut quiet = false;
    let mut held = 0u16;
    let mut open = true;
    let mut menu_was_open = false;
    let mut waits = 0u64;
    let mut waiting: Option<Instant> = None;
    // Headless --paced keeps the window's time without one.
    let mut pace = (screen.is_none() && opts.paced).then(Clock::default);
    let mut report = Report::default();
    let e = |e: netplay::Error| e.to_string();
    // Frames run: the session's, then those run alone.
    let mut solo = 0u64;
    let frames = |s: &Rollback<Gba, Udp>, solo: u64| s.frame() as u64 + solo;
    while opts.frames.is_none_or(|n| frames(&s, solo) < n) && open {
        // Headless, the script stands for the pad, which the menu lets go.
        let n = frames(&s, solo);
        let script = match opts.menu.contains(n) {
            true => 0,
            false => opts.input.keys(n),
        };
        let local = (script | held) & key::ALL;
        let status = if left.is_some() {
            let mut keys = [0; SEATS];
            keys[seat] = local;
            s.console_mut().step(&keys)?;
            solo += 1;
            Status::Stepped
        } else {
            let t = Instant::now();
            let status = s.advance(local).map_err(e)?;
            if status == Status::Stepped {
                report.stepped(t.elapsed(), Some(s.lead()));
            }
            if let Some((who, how)) = s.transport_mut().gone(QUIET) {
                let note = left_note(who, how);
                let f = frames(&s, solo);
                eprintln!(
                    "gbaa-desktop: at frame {f}, {note}; playing on alone, the cable unplugged"
                );
                // What a script reads (the check does).
                println!("left player {} at frame {f}", who + 1);
                s.transport_mut().leave();
                s.console_mut().unplug(seat);
                left = Some(note);
                left_at = Some((who, f));
                title(&format!(
                    "gbaa: room {code}, player {}, alone ({})",
                    seat + 1,
                    left.as_deref().unwrap_or_default()
                ));
                quiet = false;
                if let Some(sc) = &screen {
                    glance = report.glance(&mut s, seat, &left);
                    sc.set_netplay(playing(glance.clone(), &left, quiet));
                }
            } else if let Some(sc) = &screen
                && (s.transport_mut().quiet_for() >= SHAKY) != quiet
            {
                quiet = !quiet;
                sc.set_netplay(playing(glance.clone(), &left, quiet));
            }
            status
        };
        let stepped = match status {
            Status::Stepped => true,
            Status::Skipped => false,
            Status::Starting | Status::Waiting => {
                waits += 1;
                waiting.get_or_insert_with(Instant::now);
                std::thread::sleep(Duration::from_millis(1));
                if let Some(sc) = &screen {
                    let i = sc.input();
                    open = i.open && !i.leave;
                    held = i.keys | i.pads;
                }
                continue;
            }
        };
        if let Some(w) = waiting.take()
            && s.frame() > 1
        {
            report.freezes.add(w.elapsed(), s.frame());
        }
        if let Some(c) = &mut pace {
            c.wait(fast::frame());
        }
        let n = frames(&s, solo);
        // The menu doesn't pause a session (the others would wait): while
        // it's open the game plays on, this seat's pad let go, and the menu
        // shows how the session has gone so far, twice a second. Headless,
        // --menu opens it (the netplay check's stand-in for a player).
        let menu = screen.as_ref().is_some_and(Screen::menu_open) || opts.menu.contains(n);
        if menu && (!menu_was_open || n % REPORT_EVERY == 0) {
            let lines = report.lines(&mut s, &code, seat, waits);
            glance = report.glance(&mut s, seat, &left);
            if let Some(sc) = &screen {
                sc.set_report(lines);
                sc.set_netplay(playing(glance.clone(), &left, quiet));
            }
        }
        menu_was_open = menu;
        // A save's change flag is in its state, so the consoles clear every
        // seat's each frame and keep the news for us (Gba::take_changed).
        let gba = s.console_mut();
        let changed = gba.take_changed(seat);
        let ms = gba.machines_mut();
        if stepped && let Some(f) = &mut sav {
            ms[seat].bus.save.dirty = changed;
            f.frame(&mut ms[seat].bus.save)?;
        }
        let m = &ms[seat];
        if stepped
            && let Some(dir) = &opts.dump_dir
            && opts.dump_at.contains(&n)
        {
            let path = dump_at_path(dir, n);
            write_png(&path, &m.ppu.frame).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let Some(sc) = &screen else {
            continue;
        };
        sc.show(&m.ppu.frame, false);
        // A frame held back made no sound to wait on: the clock paces it.
        match player(s.console_mut(), seat) {
            Some(p) if stepped => p.wait(),
            _ => std::thread::sleep(fast::frame()),
        }
        if let Some(c) = &mut clock {
            c.wait(fast::frame());
        }
        let i = sc.input();
        open = i.open && !i.leave;
        held = i.keys | i.pads;
    }
    // Headless, or at --frames' end: the other peer still needs what it
    // lacks to end where this one does.
    if left.is_none() && (screen.is_none() || open) {
        let until = Instant::now() + LINGER;
        while Instant::now() < until {
            s.pump().map_err(e)?;
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    // Goodbye: the other peer plays on alone, and the server closes the
    // room once it's empty.
    s.transport_mut().leave();
    if let Some(p) = player(s.console_mut(), seat) {
        eprintln!("gbaa-desktop: {}", p.summary());
    }
    let n = frames(&s, solo);
    let m = &mut s.console_mut().machines_mut()[seat];
    if let Some(f) = &mut sav {
        f.flush(&mut m.bus.save)?;
        if screen.is_some() {
            crate::back_up_save(&f.path);
        }
    }
    if let Some(path) = &opts.dump {
        write_png(path, &m.ppu.frame).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if opts.hash {
        println!("frame {n} hash {:016x}", pixels_hash(&m.ppu.frame[..]));
    }
    for f in s.desyncs() {
        println!("desync {f}");
    }
    let mut lines = report.lines(&mut s, &code, seat, waits);
    if let Some((who, f)) = left_at {
        lines.push(format!(
            "netplay alone: player {} left at frame {f}; {solo} frames run alone after, the cable unplugged",
            who + 1
        ));
    }
    for l in &lines {
        println!("{l}");
    }
    // A window's player may not see stdout (an app opened from the
    // desktop): the lines stay in a file, so a playtest leaves numbers on
    // both sides.
    if screen.is_some() {
        match keep_report(&lines, &code, seat) {
            Ok(path) => eprintln!("gbaa-desktop: netplay report in {}", path.display()),
            Err(e) => eprintln!("gbaa-desktop: netplay report not kept: {e}"),
        }
    }
    Ok(())
}

/// Write `lines` to `$GBAA_DATA/netplay/<unix seconds>-<room>-p<seat>.txt`.
fn keep_report(lines: &[String], code: &str, seat: usize) -> Result<PathBuf, String> {
    let dir = data_dir().ok_or("no $GBAA_DATA or $HOME")?.join("netplay");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_secs());
    let path = dir.join(format!("{t}-{code}-p{}.txt", seat + 1));
    let mut text = lines.join("\n");
    text.push('\n');
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// What a session's end lines (and the menu) say beyond the session's own
/// counts: how long each frame run took, the freezes, and how far ahead
/// of the other player this peer ran.
#[derive(Default)]
struct Report {
    /// How long each frame run took, re-runs for a rollback with it.
    times: Vec<Duration>,
    /// Frames run that took longer than a frame: a hitch, the CPU behind.
    over: u64,
    freezes: Freezes,
    /// The lead (frames ahead of the other player) over the frames run:
    /// its sum, least and most.
    lead: Option<(f64, f32, f32)>,
}

impl Report {
    fn stepped(&mut self, took: Duration, lead: Option<f32>) {
        self.times.push(took);
        self.over += (took > fast::frame()) as u64;
        if let Some(l) = lead {
            let (sum, lo, hi) = self.lead.get_or_insert((0.0, l, l));
            *sum += l as f64;
            *lo = lo.min(l);
            *hi = hi.max(l);
        }
    }

    /// The menu's few lines: the other player's seat, the round trip, how
    /// far ahead or behind, the rollbacks and the frames run again.
    fn glance(
        &self,
        s: &mut Rollback<Gba, Udp>,
        seat: usize,
        left: &Option<String>,
    ) -> Vec<String> {
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let lead = s.lead();
        let st = s.stats;
        if let Some(l) = left {
            return vec![
                format!("{l}."),
                format!(
                    "You are player {}, playing on alone, the cable unplugged.",
                    seat + 1
                ),
                format!(
                    "{} rollbacks, {} frames run again, {} frozen, while linked",
                    st.rollbacks, st.rerun, self.freezes.count
                ),
            ];
        }
        vec![
            format!(
                "You are player {}, the other player {}",
                seat + 1,
                SEATS - seat
            ),
            match s.transport().rtt() {
                Some((now, lo, hi)) => format!(
                    "Round trip {:.0} ms (least {:.0}, most {:.0})",
                    ms(now),
                    ms(lo),
                    ms(hi)
                ),
                None => "Round trip not measured yet".into(),
            },
            if lead >= 0.0 {
                format!("{lead:.1} frames ahead of the other player")
            } else {
                format!("{:.1} frames behind the other player", -lead)
            },
            format!(
                "{} rollbacks, {} frames run again, {} frozen",
                st.rollbacks, st.rerun, self.freezes.count
            ),
        ]
    }

    /// The end lines so far.
    fn lines(
        &self,
        s: &mut Rollback<Gba, Udp>,
        code: &str,
        seat: usize,
        waits: u64,
    ) -> Vec<String> {
        let n = s.frame();
        let mut times = self.times.clone();
        times.sort();
        let ms = |p: f64| {
            times
                .get(((times.len().max(1) - 1) as f64 * p) as usize)
                .map_or(0.0, |d| d.as_secs_f64() * 1e3)
        };
        let f = &self.freezes;
        let ms_of = |d: Duration| d.as_secs_f64() * 1e3;
        let rtt = match s.transport().rtt() {
            Some((now, lo, hi)) => format!(
                "round trip {:.0} ms (least {:.0}, most {:.0})",
                ms_of(now),
                ms_of(lo),
                ms_of(hi)
            ),
            None => "round trip not measured".into(),
        };
        let lead = match (Some(s.lead()), self.lead) {
            (Some(now), Some((sum, lo, hi))) => format!(
                "; ahead of the other player by {now:.1} frames now, {:.1} on average, {lo:.1} to {hi:.1}",
                sum / self.times.len().max(1) as f64
            ),
            _ => String::new(),
        };
        let mut lines = vec![
            format!(
                "netplay: room {code}, seat {}, {n} frames, end {:016x}, {} transfers, {waits} waits, {} datagrams, {} resent, {}",
                seat + 1,
                s.console_mut().hash(),
                s.console().link().transfers,
                s.transport().stats.datagrams,
                s.transport().stats.resent,
                s.summary()
            ),
            format!(
                "netplay frames: p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms, {} over a frame; frozen {} times, {:.0} ms in all, longest {:.0} ms (at frame {})",
                ms(0.5),
                ms(0.99),
                ms(1.0),
                self.over,
                f.count,
                ms_of(f.total),
                ms_of(f.longest),
                f.at
            ),
            format!("netplay link: {rtt}{lead}"),
        ];
        let gba = s.console();
        if gba.recompiled() {
            let st = gba.stats();
            let running = (st.compiled_cycles + st.interpreted_cycles).max(1);
            lines.insert(
                1,
                format!(
                    "netplay code: {:.1}% of cycles compiled, {} calls ({} resuming), {} instructions interpreted",
                    st.compiled_cycles as f64 * 100.0 / running as f64,
                    st.calls,
                    st.resumes,
                    st.interpreted
                ),
            );
        }
        lines
    }
}

/// The waits on the other player longer than a frame: what a player sees
/// as the game freezing.
#[derive(Default)]
struct Freezes {
    count: u64,
    total: Duration,
    longest: Duration,
    /// The frame the longest ended at.
    at: u32,
}

impl Freezes {
    fn add(&mut self, d: Duration, frame: u32) {
        if d > fast::frame() {
            self.count += 1;
            self.total += d;
            if d > self.longest {
                self.longest = d;
                self.at = frame;
            }
        }
    }
}
