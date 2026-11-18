//! `emerald-trade DESKTOP ROM SAVE_DIR [--keep DIR] [--netplay [--profile
//! FILE]]`: linked play's check (multiplayer/local-link-window). It copies
//! `alice.sav` and `bob.sav` (`just emerald-saves`) to a scratch folder and
//! runs them as two linked Emeralds through the desktop (`DESKTOP
//! --headless --link`, the same run as its window's, with no window): each
//! continues, takes the cable club's Trade Center, sits down, offers its
//! first Pokémon and says yes. Then it reads the two saves the desktop
//! wrote back and passes (exit 0) when the trade is in them: Alice's first
//! Pokémon is the one Bob had (personality and original trainer), Bob's the
//! one Alice had, and the second ones stayed. About a minute. `--keep DIR`
//! writes the scratch saves and the last frame (both screens) there and
//! leaves them.
//!
//! `--netplay` makes it netplay's check (multiplayer/desktop-netplay): the
//! same trade between two desktop processes, one a player, through a
//! server's plain-UDP rooms it starts in-process (`relay::rooms::serve`, as
//! `gbaa-pingsim desktop` does). Alice's (`--server ADDR --host`) prints
//! the room's code, Bob's joins it
//! (`--join CODE`); each runs both GBAs and keeps only its own save, and
//! `--keep` gets each one's own screen. `--profile FILE` puts each
//! player's path to the server through the variable-ping simulator
//! (`netplay::proxy`, a profile from crates/netplay/profiles/), so they
//! see two hops of it. It passes only if, besides the trade, both ended on
//! the same hash of both consoles, with no desync and neither player
//! taken to have left. It plays by rollback at no input delay (the
//! desktop's default), so the presses land on the frames they name, as in
//! the linked run. `--stop SEAT@SECS+DUR` stops player SEAT's desktop
//! (`SIGSTOP`) SECS seconds after it starts, for DUR seconds, then lets it
//! go on (`SIGCONT`): a machine that stalls, which must catch up rather
//! than end the session (multiplayer/wander-split-under-load).

use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use emerald_save::{party, read};
use netplay::proxy::{Profile, Proxy};

const USAGE: &str = "usage: emerald-trade DESKTOP ROM SAVE_DIR [--keep DIR] \
                     [--netplay [--profile FILE] [--stop SEAT@SECS+DUR]]";

/// `--stop`'s player (from 0), when (after it starts) and for how long.
#[derive(Clone, Copy)]
struct Stop {
    seat: usize,
    at: Duration,
    hold: Duration,
}

fn parse_stop(s: &str) -> Option<Stop> {
    let (seat, rest) = s.split_once('@')?;
    let (at, hold) = rest.split_once('+')?;
    let seat: usize = seat.parse().ok()?;
    let secs = |t: &str| {
        t.parse::<f64>()
            .ok()
            .filter(|t| t.is_finite() && *t >= 0.0)
            .map(Duration::from_secs_f64)
    };
    (1..=2).contains(&seat).then_some(Stop {
        seat: seat - 1,
        at: secs(at)?,
        hold: secs(hold)?,
    })
}

/// Both players' presses: continue (Start, A, A), Up at the attendant and A
/// every 60 frames through her lines, the Trade Center, the save's two
/// Yeses and (the parent's) the link's confirmation; then, in the Trade
/// Center, up to the table and round to a chair; then A on the first
/// Pokémon and A on Summary, B back to the trade screen (its CpuSet
/// clearing VRAM once starved the link: the BIOS's stall), then A on the
/// first Pokémon, Down to Trade, A, and A on "Is this trade okay?"'s Yes.
const COMMON: &str = "start@300,a@900,a@1100,up@1350,a@1400-3400/60,up@3800+10";
const TO_CHAIR: [&str; 2] = ["left@3840+10", "right@3840+10"];
const TRADE: &str = "up@3880+10,up@3920+10,a@4500,a@4560,b@5000,\
                     a@5300,down@5360,a@5400,a@5650";
/// The trade's animation, the link save and the return to the trade
/// screen are done by about frame 8200.
const FRAMES: u64 = 8400;

/// How long a netplay player may take: about a minute headless, more
/// through a slow profile.
const NETPLAY_LIMIT: Duration = Duration::from_secs(900);

const NAMES: [&str; 2] = ["alice", "bob"];

fn input(p: usize) -> String {
    format!("{COMMON},{},{TRADE}", TO_CHAIR[p])
}

fn run() -> Result<bool, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let (mut keep, mut netplay, mut profile, mut stop) = (None, false, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--keep" => keep = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
            "--netplay" => netplay = true,
            "--profile" => profile = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
            "--stop" => {
                let a = it.next().ok_or(USAGE)?;
                stop = Some(parse_stop(a).ok_or(format!("--stop {a}: not SEAT@SECS+DUR"))?);
            }
            s if s.starts_with('-') => return Err(USAGE.into()),
            _ => pos.push(Path::new(a)),
        }
    }
    let [desktop, rom, saves] = pos[..] else {
        return Err(USAGE.into());
    };
    if (profile.is_some() || stop.is_some()) && !netplay {
        return Err("--profile and --stop are --netplay's".into());
    }
    let dir = match &keep {
        Some(d) => d.clone(),
        None => std::env::temp_dir().join(format!("emerald-trade-{}", std::process::id())),
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut before = Vec::new();
    for n in NAMES {
        let from = saves.join(format!("{n}.sav"));
        let bytes = std::fs::read(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        let b = read(&bytes).ok_or(format!("{}: no whole save in it", from.display()))?;
        before.push(party(&b));
        let to = dir.join(format!("{n}.sav"));
        std::fs::write(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    if netplay {
        let profile = match &profile {
            Some(p) => Some(Profile::load(p)?),
            None => None,
        };
        play_netplay(desktop, rom, &dir, profile, stop, keep.is_some())?;
    } else {
        play_linked(desktop, rom, &dir, keep.is_some())?;
    }
    let mut after = Vec::new();
    for n in NAMES {
        let path = dir.join(format!("{n}.sav"));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        after.push(party(&read(&bytes).ok_or(format!(
            "{}: no whole save after the run",
            path.display()
        ))?));
    }
    if keep.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let show = |p: &[(u32, u32)]| {
        p.iter()
            .map(|(pid, ot)| format!("{pid:08x}/OT {ot:08x}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for (n, (b, a)) in NAMES.iter().zip(before.iter().zip(&after)) {
        println!("{n}: before [{}], after [{}]", show(b), show(a));
    }
    let want = |me: usize| {
        let mut p = before[me].clone();
        p[0] = before[1 - me][0];
        p
    };
    Ok(before.iter().all(|p| p.len() >= 2) && after[0] == want(0) && after[1] == want(1))
}

/// Both players in one desktop, `--link`.
fn play_linked(desktop: &Path, rom: &Path, dir: &Path, keep: bool) -> Result<(), String> {
    let mut cmd = Command::new(desktop);
    cmd.arg(rom)
        .args([
            "--headless",
            "--timing",
            "exact",
            "--frames",
            &FRAMES.to_string(),
        ])
        .arg("--save")
        .arg(dir.join("alice.sav"))
        .arg("--link")
        .arg(dir.join("bob.sav"))
        .args(["--input", &input(0), "--input2", &input(1), "--hash"]);
    if keep {
        cmd.arg("--dump").arg(dir.join("last.png"));
    }
    let out = cmd
        .output()
        .map_err(|e| format!("{}: {e}", desktop.display()))?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        return Err(format!("the desktop failed: {}", out.status));
    }
    Ok(())
}

/// Each player in a desktop of their own, through a local server's
/// plain-UDP rooms (and the ping simulator's proxies, given a profile).
fn play_netplay(
    desktop: &Path,
    rom: &Path,
    dir: &Path,
    profile: Option<Profile>,
    stop: Option<Stop>,
    keep: bool,
) -> Result<(), String> {
    let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let sock = UdpSocket::bind(any).map_err(|e| format!("server: {e}"))?;
    let rooms = sock.local_addr().map_err(|e| format!("server: {e}"))?;
    std::thread::spawn(move || relay::rooms::serve(sock, 0));
    let mut proxies = Vec::new();
    let mut to = [rooms; 2];
    if let Some(p) = profile {
        for (seat, t) in to.iter_mut().enumerate() {
            // Each path its own randomness.
            let p = Profile {
                seed: p.seed.wrapping_add(seat as u64),
                ..p.clone()
            };
            let proxy = Proxy::to(p, any, rooms).map_err(|e| format!("proxy: {e}"))?;
            *t = proxy.addrs()[0];
            proxies.push(proxy);
        }
    }
    let player = |seat: usize, role: &[&str]| -> Result<Child, String> {
        let mut cmd = Command::new(desktop);
        cmd.arg(rom)
            .args([
                "--headless",
                "--timing",
                "exact",
                "--frames",
                &FRAMES.to_string(),
            ])
            .arg("--save")
            .arg(dir.join(format!("{}.sav", NAMES[seat])))
            .arg("--server")
            .arg(to[seat].to_string())
            .args(role)
            .args(["--input", &input(seat), "--hash"])
            .stdout(Stdio::piped());
        if keep {
            cmd.arg("--dump")
                .arg(dir.join(format!("{}.png", NAMES[seat])));
        }
        cmd.spawn()
            .map_err(|e| format!("{}: {e}", desktop.display()))
    };
    let began = Instant::now();
    let mut host = player(0, &["--host"])?;
    let mut host_out = BufReader::new(host.stdout.take().unwrap());
    let mut line = String::new();
    host_out
        .read_line(&mut line)
        .map_err(|e| format!("player 1's output: {e}"))?;
    let Some(code) = line.trim().strip_prefix("room ").map(str::to_string) else {
        let _ = host.kill();
        let _ = host.wait();
        return Err(format!("player 1 gave no room code: {line:?}"));
    };
    println!("room {code}");
    let mut guest = match player(1, &["--join", &code]) {
        Ok(g) => g,
        Err(e) => {
            let _ = host.kill();
            let _ = host.wait();
            return Err(e);
        }
    };
    if let Some(st) = stop {
        let pid = [&host, &guest][st.seat].id() as libc::pid_t;
        let seat = st.seat + 1;
        std::thread::spawn(move || {
            std::thread::sleep(st.at);
            // SAFETY: plain signals to a child this process started (and
            // hasn't reaped: the waits below come after its end).
            if unsafe { libc::kill(pid, libc::SIGSTOP) } != 0 {
                return;
            }
            println!("stopped player {seat} for {:.1} s", st.hold.as_secs_f64());
            std::thread::sleep(st.hold);
            unsafe { libc::kill(pid, libc::SIGCONT) };
        });
    }
    let guest_out = BufReader::new(guest.stdout.take().unwrap());
    let outs: [Box<dyn Read + Send>; 2] = [Box::new(host_out), Box::new(guest_out)];
    let readers = outs.map(|mut r| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = r.read_to_string(&mut s);
            s
        })
    });
    let mut failed = Vec::new();
    for (seat, k) in [&mut host, &mut guest].into_iter().enumerate() {
        loop {
            match k.try_wait().map_err(|e| e.to_string())? {
                Some(st) if st.success() => break,
                Some(st) => {
                    failed.push(format!("player {}'s desktop ended: {st}", seat + 1));
                    break;
                }
                None if began.elapsed() > NETPLAY_LIMIT => {
                    let _ = k.kill();
                    let _ = k.wait();
                    failed.push(format!("player {}'s desktop stalled; killed", seat + 1));
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }
    println!("played in {:.1} s", began.elapsed().as_secs_f64());
    let outs = readers.map(|r| r.join().unwrap_or_default());
    for proxy in proxies {
        let st = proxy.stop();
        println!(
            "proxy: {} received, {} forwarded, {} lost, {} spikes, delay {:.1} to {:.1} ms",
            st.received,
            st.forwarded,
            st.lost,
            st.spikes,
            st.min_delay_us as f64 / 1000.0,
            st.max_delay_us as f64 / 1000.0
        );
    }
    let mut ends = Vec::new();
    for (seat, out) in outs.iter().enumerate() {
        for l in out.lines() {
            println!("player {}: {l}", seat + 1);
            if l.starts_with("desync") {
                failed.push(format!("player {} went apart: {l}", seat + 1));
            }
            // The other player was taken to have left: each played on alone.
            if l.starts_with("left player") {
                failed.push(format!("player {}: {l}", seat + 1));
            }
        }
        // The desktop's last line: `netplay: room …, end HASH, …`.
        let end = out
            .lines()
            .find_map(|l| l.split(" end ").nth(1)?.split(',').next())
            .map(str::to_string);
        ends.push(end);
    }
    if !failed.is_empty() {
        return Err(failed.join("; "));
    }
    match &ends[..] {
        [Some(a), Some(b)] if a == b => Ok(()),
        _ => Err(format!("the players' consoles ended apart: {ends:?}")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => {
            println!("pass: the first Pokémon traded places, in both saves");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("fail: the saves don't hold the trade");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("emerald-trade: {e}");
            ExitCode::from(2)
        }
    }
}
