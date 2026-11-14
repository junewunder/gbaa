//! `emerald-wireless ROM SAVE_DIR [--frames N] [--once] [--dump DIR
//! --dump-at N,N…] [--input0 PRESSES] [--input1 PRESSES]`: the wireless
//! adapter's check (multiplayer/wireless-adapter-local). Two Emeralds,
//! `alice.sav` and `bob.sav` (`just emerald-saves`), each with an emulated
//! wireless adapter, in one radio (`runtime::link::rfu`), in the
//! interpreter. Each continues its game, walks to Oldale's Union Room
//! attendant and goes in; there BOB registers his Torchic at the trading
//! board, ALICE offers her Mudkip for it, BOB accepts, and they trade.
//! It prints each player's map, adapter state and the game's link state as
//! they change, runs the whole thing twice (not with `--once`), and passes
//! (exit 0) when both players were in the Union Room, each ends with the
//! other's Pokémon first in its party, and the two runs printed the same
//! and ended in the same machine states.
//!
//! `--input0`/`--input1` replace a player's presses (as `emerald-link`'s);
//! `--dump-at` writes each player's frame N as DIR/pP-N.png.
//!
//! `--net PROFILE` (multiplayer/wireless-radio-netplay) is the same trade
//! with the radio on the server: a `gbaa-server` room in this process,
//! and two processes of this program (`--seat 0|1 --server ADDR [--code
//! CODE]`), each running only its own Emerald and networked adapter
//! (`netplay::radio`) in real time (59.73 frames a second), each path to
//! the server through the ping simulator's PROFILE (a name in
//! `crates/netplay/profiles/`, or a TOML file). It runs once and passes
//! when both were in the Union Room and the first Pokémon swapped.
//! `--sim-ms N` is the same on simulated time, in this process: both
//! consoles, each with its networked adapter, and the server's radio
//! (`runtime::link::rfu::air::Air`), every message N ms each way between
//! a console and the server (to the nearest quarter frame); deterministic,
//! and as fast as the local check, for measuring how much latency the
//! Union Room allows.
//! `--heard-ms N` sets how long a networked adapter's search keeps
//! listing a host it no longer hears (default 3000, as the local
//! radio's; 0: only those the server last said were open).

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use emerald_save::{G_LINK_STATUS, Presses, SB1_PARTY, SB1_PTR};
use runtime::Machine;
use runtime::link::rfu::Wireless;
use runtime::platform::{Frame, HEIGHT, Platform, WIDTH};
use runtime::sched::FRAME_CYCLES;

const USAGE: &str = "usage: emerald-wireless ROM SAVE_DIR [--frames N] [--once] \
                     [--dump DIR --dump-at N,N...] [--input0 PRESSES] [--input1 PRESSES] \
                     [--net PROFILE | --seat N --server ADDR [--code CODE]] [--heard-ms N]";

/// `MAP_UNION_ROOM`: group 25, number 60.
const UNION_ROOM: [u8; 2] = [25, 60];
/// `gMain.callback2`, `gRfu`'s state byte and `gWirelessCommType`.
const CALLBACK2: u32 = 0x0300_22C4;
const G_WIRELESS_COMM_TYPE: u32 = 0x0300_30FC;
/// `CB2_LinkError` (Thumb) and `sLinkErrorBuffer`: the status word
/// (`gRfu.errorInfo << 16 | errorParam0 << 8 | errorParam1`, the first
/// the link manager's message), then the queue counts.
const CB2_LINK_ERROR: u32 = 0x0800_AF31;
const LINK_ERROR: u32 = 0x0202_2B00;
/// `gObjectEvents` (0x24 bytes each, `currentCoords` at 0x10) and
/// `gPlayerAvatar.objectEventId`; map coordinates are 7 off
/// (`MAP_OFFSET`).
const G_OBJECT_EVENTS: u32 = 0x0203_7350;
const PLAYER_OBJECT_ID: u32 = 0x0203_7595;

/// Where the player stands on the map.
fn position(m: &Machine<Pad>) -> (i16, i16) {
    let o = G_OBJECT_EVENTS + 0x24 * m.bus.read8(PLAYER_OBJECT_ID) as u32 + 0x10;
    (m.bus.read16(o) as i16 - 7, m.bus.read16(o + 2) as i16 - 7)
}

/// The frames the check runs: both are back in the Union Room, the trade
/// saved, by about 10200.
const FRAMES: u64 = 10500;
/// Each player's presses. Both: continue (Start, A, A); down two, left
/// four and up two to Oldale's Union Room attendant (the counter's posts
/// block row 3); A through her lines, Yes, the save and in. Then ALICE
/// (player 0): up one, left five and up eight to the trading board, and
/// later checks it: BOB's offer, Yes, her Mudkip, Trade. BOB (player 1):
/// up one, left four and up seven to the room's attendant: Register, his
/// Torchic, Register, WATER (two downs), then Yes to ALICE's offer. Both
/// press A through the trade and its save.
const PRESSES: [&str; 2] = [
    "start@300,a@900,a@1100,down@1400+32,left@1440+70,up@1520+36,a@1600-3500/50,\
     up@5000+16,left@5020+86,up@5110+136,a@5800,a@5940,a@6000,a@6080,a@6160,\
     a@6300,a@6430,a@6560,a@6620,a@6680,a@9400-10000/60",
    "start@300,a@900,a@1100,down@1400+32,left@1440+70,up@1520+36,a@1600-3500/50,\
     up@3700+16,left@3720+70,up@3800+120,a@3940-4740/60,a@4850,a@4950,\
     down@5300,down@5340,a@5390,a@5580,a@7340-7700/60,a@9400-10000/60",
];

struct Pad {
    presses: Presses,
    frames: u64,
    frame: Box<Frame>,
}

impl Platform for Pad {
    fn present(&mut self, frame: &Frame) {
        self.frames += 1;
        self.frame.copy_from_slice(frame);
    }

    fn keys(&mut self) -> u16 {
        self.presses.keys(self.frames)
    }
}

struct Options {
    rom: &'static [u8],
    saves: [Vec<u8>; 2],
    frames: u64,
    once: bool,
    presses: [Presses; 2],
    dump: Option<(PathBuf, Vec<u64>)>,
    /// `--net PROFILE`, `--seat`, `--server`, `--code`, `--heard-ms`.
    net: Option<String>,
    seat: Option<usize>,
    server: Option<String>,
    code: Option<String>,
    heard_ms: Option<u64>,
    /// `--sim-ms N`: the radio on a server in this process, N ms each way.
    sim_ms: Option<f64>,
    /// Every argument, for the seats' processes.
    args: Vec<String>,
}

struct Outcome {
    log: Vec<String>,
    /// The first frame each player stood in the Union Room.
    entered: [Option<u64>; 2],
    /// Each player's first party Pokémon's personality, at the start and
    /// at the end.
    first: [[u32; 2]; 2],
    hashes: [u64; 2],
    transfers: u64,
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

fn write_png(path: &Path, frame: &Frame) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), WIDTH as u32, HEIGHT as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = frame
        .iter()
        .flat_map(|&c| {
            let ch = |s: u16| ((c >> s & 31) as u8) << 3 | ((c >> s & 31) as u8) >> 2;
            [ch(0), ch(5), ch(10)]
        })
        .collect();
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&data))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The personality of the first Pokémon in the party.
fn first_mon(m: &Machine<Pad>) -> u32 {
    m.bus.read32(m.bus.read32(SB1_PTR) + SB1_PARTY as u32)
}

fn run(o: &Options, dump: bool) -> Result<Outcome, String> {
    let mut ms: Vec<Box<Machine<Pad>>> = (0..2)
        .map(|p| {
            let pad = Pad {
                presses: o.presses[p].clone(),
                frames: 0,
                frame: Box::new([0; WIDTH * HEIGHT]),
            };
            let mut m = Box::new(Machine::new(o.rom, pad));
            let save = &o.saves[p];
            m.bus.save.data[..save.len()].copy_from_slice(save);
            m
        })
        .collect();
    let [a, b] = &mut ms[..] else { unreachable!() };
    let mut radio = Wireless::new(&mut [&mut **a, &mut **b]);
    let mut out = Outcome {
        log: Vec::new(),
        entered: [None; 2],
        first: [[0; 2]; 2],
        hashes: [0; 2],
        transfers: 0,
    };
    let mut last = [None; 2];
    let mut last_bc = [[0; 6]; 2];
    for f in 1..=o.frames {
        radio.run(&mut [&mut **a, &mut **b], FRAME_CYCLES, |_, m| {
            m.run_frame()
        });
        for (p, m) in [&a, &b].into_iter().enumerate() {
            if let Some(fault) = &m.fault {
                return Err(format!("player {p}, frame {f}: {fault}"));
            }
            let sb1 = m.bus.read32(SB1_PTR);
            let map = [m.bus.read8(sb1 + 4), m.bus.read8(sb1 + 5)];
            if f == 400 {
                out.first[p][0] = first_mon(m);
            }
            let (id, state) = radio.status(p);
            let now = (
                map,
                m.bus.read32(CALLBACK2),
                m.bus.read8(G_WIRELESS_COMM_TYPE),
                state,
                id,
                m.bus.read32(G_LINK_STATUS) & !0x100,
                position(m),
            );
            if Some(now) != last[p] {
                out.log.push(format!(
                    "frame {f}: player {p} map {}.{} callback2 {:08x} commType {} \
                     adapter state {} id {:04x} gLinkStatus {:#010x} at {},{}",
                    map[0], map[1], now.1, now.2, now.3, now.4, now.5, now.6.0, now.6.1
                ));
                last[p] = Some(now);
            }
            let bc = radio.broadcast(p);
            if bc != last_bc[p] {
                out.log
                    .push(format!("frame {f}: player {p} broadcast {bc:08x?}"));
                last_bc[p] = bc;
            }
            if map == UNION_ROOM && out.entered[p].is_none() {
                out.entered[p] = Some(f);
                out.log
                    .push(format!("frame {f}: player {p} in the Union Room"));
            }
        }
        if let Some((dir, at)) = o.dump.as_ref().filter(|_| dump)
            && at.contains(&f)
        {
            for (p, m) in [&a, &b].into_iter().enumerate() {
                write_png(&dir.join(format!("p{p}-{f}.png")), &m.platform.frame)?;
            }
        }
    }
    out.transfers = radio.transfers;
    for (p, commands) in radio.commands[..2].iter().enumerate() {
        let used: Vec<String> = commands
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(c, n)| format!("{c:02x}:{n}"))
            .collect();
        out.log
            .push(format!("player {p} adapter commands {}", used.join(" ")));
    }
    for (p, m) in [&a, &b].into_iter().enumerate() {
        out.first[p][1] = first_mon(m);
    }
    for (h, m) in out.hashes.iter_mut().zip([a, b]) {
        let mut acc = 0xCBF2_9CE4_8422_2325;
        m.save_payload(&mut |bytes| acc = fnv(acc, bytes));
        *h = acc;
    }
    Ok(out)
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut pos = Vec::new();
    let mut frames = FRAMES;
    let mut once = false;
    let mut presses = PRESSES.map(String::from);
    let (mut dir, mut at) = (None, Vec::new());
    let (mut net, mut seat, mut server, mut code, mut heard_ms) = (None, None, None, None, None);
    let mut sim_ms = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| USAGE.to_string());
        match a.as_str() {
            "--frames" => frames = val()?.parse().map_err(|_| USAGE)?,
            "--once" => once = true,
            "--input0" => presses[0] = val()?.clone(),
            "--input1" => presses[1] = val()?.clone(),
            "--net" => net = Some(val()?.clone()),
            "--seat" => seat = Some(val()?.parse().map_err(|_| USAGE)?),
            "--server" => server = Some(val()?.clone()),
            "--code" => code = Some(val()?.clone()),
            "--heard-ms" => heard_ms = Some(val()?.parse().map_err(|_| USAGE)?),
            "--sim-ms" => sim_ms = Some(val()?.parse().map_err(|_| USAGE)?),
            "--dump" => dir = Some(PathBuf::from(val()?)),
            "--dump-at" => {
                at = val()?
                    .split(',')
                    .map(|n| {
                        if let Some((a, b)) = n.split_once('-') {
                            let (a, b, every) = match b.split_once('/') {
                                Some((b, e)) => (a, b, e),
                                None => (a, b, "1"),
                            };
                            let p = |s: &str| s.parse::<u64>().map_err(|_| USAGE.to_string());
                            Ok((p(a)?..=p(b)?).step_by(p(every)?.max(1) as usize).collect())
                        } else {
                            n.parse().map(|n| vec![n]).map_err(|_| USAGE.to_string())
                        }
                    })
                    .collect::<Result<Vec<Vec<u64>>, _>>()?
                    .concat()
            }
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
        once,
        presses: [Presses::parse(&presses[0])?, Presses::parse(&presses[1])?],
        dump: dir.map(|d| (d, at)),
        net,
        seat,
        server,
        code,
        heard_ms,
        sim_ms,
        args: args.to_vec(),
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = match options(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("emerald-wireless: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some((dir, _)) = &o.dump
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("emerald-wireless: {}: {e}", dir.display());
        return ExitCode::from(2);
    }
    if let Some(seat) = o.seat {
        return match run_seat(&o, seat) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("emerald-wireless: seat {seat}: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(ms) = o.sim_ms {
        return match run_sim(&o, ms) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("emerald-wireless: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(profile) = &o.net {
        return match run_net(&o, profile) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("emerald-wireless: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let a = match run(&o, true) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("emerald-wireless: {e}");
            return ExitCode::FAILURE;
        }
    };
    for l in &a.log {
        println!("{l}");
    }
    println!(
        "{} frames, {} transfers; states {:016x} {:016x}",
        o.frames, a.transfers, a.hashes[0], a.hashes[1]
    );
    let same = if o.once {
        true
    } else {
        match run(&o, false) {
            Ok(b) => a.log == b.log && a.hashes == b.hashes && a.transfers == b.transfers,
            Err(e) => {
                eprintln!("emerald-wireless: second run: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    if !o.once {
        println!(
            "second run: {}",
            if same { "identical" } else { "DIFFERENT" }
        );
    }
    let [[a0, a1], [b0, b1]] = a.first;
    let traded = a0 != b0 && a1 == b0 && b1 == a0;
    println!(
        "first Pokémon (personality): player 0 {a0:08x} -> {a1:08x}, player 1 {b0:08x} -> {b1:08x}"
    );
    match a.entered {
        [Some(p), Some(c)] if same && traded => {
            println!(
                "pass: both in the Union Room (frames {p}, {c}); they traded their first Pokémon"
            );
            ExitCode::SUCCESS
        }
        e => {
            println!("fail: in the Union Room at {e:?}, traded: {traded}");
            ExitCode::FAILURE
        }
    }
}

/// A GBA frame in real time: 280896 cycles at 2^24 Hz.
const FRAME_TIME: Duration = Duration::from_nanos(16_742_706);

/// The room's build ID: the same for both seats.
const BUILD: [u8; 32] = *b"emerald-wireless radio netplay\0\0";

/// One seat: its own Emerald, its networked adapter, the radio on the
/// server at `--server`, in real time. Prints its log as it goes, then
/// `result ENTERED FIRST_BEFORE FIRST_AFTER`.
fn run_seat(o: &Options, seat: usize) -> Result<(), String> {
    use netplay::radio::Radio;
    let server: SocketAddr = o
        .server
        .as_deref()
        .ok_or("--seat needs --server")?
        .parse()
        .map_err(|e| format!("--server: {e}"))?;
    let mut radio = if seat == 0 {
        let mut said = false;
        Radio::host(
            server,
            2,
            &BUILD,
            |code, _, _| {
                if !said {
                    println!("code {}", String::from_utf8_lossy(&code));
                    let _ = std::io::stdout().flush();
                    said = true;
                }
            },
            || true,
        )?
        .0
    } else {
        let code = o.code.as_deref().ok_or("--seat 1 needs --code")?;
        let code: [u8; relay::CODE_LEN] = code
            .as_bytes()
            .try_into()
            .map_err(|_| format!("--code {code}?"))?;
        Radio::join(server, code, &BUILD, || true)?
    };
    let pad = Pad {
        presses: o.presses[seat].clone(),
        frames: 0,
        frame: Box::new([0; WIDTH * HEIGHT]),
    };
    let mut m = Box::new(Machine::new(o.rom, pad));
    let save = &o.saves[seat];
    m.bus.save.data[..save.len()].copy_from_slice(save);
    let mut w = Wireless::networked(&mut m, seat as u8);
    if let Some(ms) = o.heard_ms {
        w.set_heard_for(ms * FRAME_CYCLES * 60 / 1000);
    }
    let mut last = None;
    let mut last_bc = [0; 6];
    let mut entered = None;
    let (mut first, mut first_now) = (0, 0);
    let start = Instant::now();
    let mut behind = 0u64;
    for f in 1..=o.frames {
        for _ in 0..4 {
            w.run(&mut [&mut *m], FRAME_CYCLES / 4, |_, m| m.run_frame());
            radio.pump(&mut w, Instant::now());
        }
        if let Some(fault) = &m.fault {
            return Err(format!("frame {f}: {fault}"));
        }
        let sb1 = m.bus.read32(SB1_PTR);
        let map = [m.bus.read8(sb1 + 4), m.bus.read8(sb1 + 5)];
        if f == 400 {
            first = first_mon(&m);
            first_now = first;
        }
        let (id, state) = w.status(0);
        let now = (
            map,
            m.bus.read32(CALLBACK2),
            m.bus.read8(G_WIRELESS_COMM_TYPE),
            state,
            id,
            m.bus.read32(G_LINK_STATUS) & !0x100,
            position(&m),
        );
        if Some(now) != last {
            if now.1 == CB2_LINK_ERROR && last.is_none_or(|l| l.1 != CB2_LINK_ERROR) {
                println!(
                    "frame {f}: player {seat} link error {:08x} {:08x}",
                    m.bus.read32(LINK_ERROR),
                    m.bus.read32(LINK_ERROR + 4)
                );
            }
            println!(
                "frame {f}: player {seat} map {}.{} callback2 {:08x} commType {} \
                 adapter state {} id {:04x} gLinkStatus {:#010x} at {},{}",
                map[0], map[1], now.1, now.2, now.3, now.4, now.5, now.6.0, now.6.1
            );
            last = Some(now);
        }
        let bc = w.broadcast(0);
        if bc != last_bc {
            println!("frame {f}: player {seat} broadcast {bc:08x?}");
            last_bc = bc;
        }
        if map == UNION_ROOM && entered.is_none() {
            entered = Some(f);
            println!("frame {f}: player {seat} in the Union Room");
        }
        if f > 400 && first_mon(&m) != first_now {
            first_now = first_mon(&m);
            println!("frame {f}: player {seat} first Pokémon now {first_now:08x}");
        }
        // Real time: wait for the frame's turn, hearing the radio meanwhile.
        let due = start + FRAME_TIME * f as u32;
        if Instant::now() > due + FRAME_TIME {
            behind += 1;
        }
        while Instant::now() < due {
            radio.pump(&mut w, Instant::now());
            std::thread::sleep(Duration::from_micros(500).min(due - Instant::now().min(due)));
        }
    }
    let used: Vec<String> = w.commands[0]
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(c, n)| format!("{c:02x}:{n}"))
        .collect();
    println!("player {seat} adapter commands {}", used.join(" "));
    let s = radio.stats;
    println!(
        "player {seat} radio: {} up, {} down, {} datagrams in, {} out, {} resent; \
         {behind} frames more than a frame late; {:.1} s",
        s.ups,
        s.downs,
        s.datagrams_in,
        s.datagrams_out,
        radio.resent(),
        start.elapsed().as_secs_f64()
    );
    radio.leave();
    println!(
        "result {} {first:08x} {:08x}",
        entered.map_or("-".to_string(), |f| f.to_string()),
        first_mon(&m)
    );
    Ok(())
}

/// A ping simulator profile: a name in `crates/netplay/profiles/`, or a
/// file.
fn profile(name: &str) -> Result<netplay::proxy::Profile, String> {
    let path = if name.ends_with(".toml") || name.contains('/') {
        PathBuf::from(name)
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../netplay/profiles")
            .join(format!("{name}.toml"))
    };
    netplay::proxy::Profile::load(&path)
}

/// The two seats through a server in this process, each path to it
/// through the ping simulator's `profile`: passes when both were in the
/// Union Room and their first Pokémon swapped.
fn run_net(o: &Options, profile_name: &str) -> Result<bool, String> {
    use netplay::proxy::{Profile, Proxy};
    let profile = profile(profile_name)?;
    let sock = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let server = sock.local_addr().map_err(|e| e.to_string())?;
    std::thread::spawn(move || relay::rooms::serve(sock, 0));
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut proxies = Vec::new();
    for seat in 0..2u64 {
        let p = Profile {
            seed: profile.seed.wrapping_add(seat),
            ..profile.clone()
        };
        proxies.push(Proxy::to(p, local, server).map_err(|e| format!("proxy: {e}"))?);
    }
    let to: Vec<SocketAddr> = proxies
        .iter()
        .map(|p| SocketAddr::from(([127, 0, 0, 1], p.addrs()[0].port())))
        .collect();
    println!("radio on {server}, profile {profile_name}: {profile:?}");
    // The seats get every argument but --net.
    let mut args = Vec::new();
    let mut it = o.args.iter();
    while let Some(a) = it.next() {
        if a == "--net" {
            it.next();
        } else {
            args.push(a.clone());
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let spawn = |seat: usize, code: Option<&str>| {
        let mut c = Command::new(&exe);
        c.args(&args).args([
            "--seat",
            &seat.to_string(),
            "--server",
            &to[seat].to_string(),
        ]);
        if let Some(code) = code {
            c.args(["--code", code]);
        }
        c.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("seat {seat}: {e}"))
    };
    let mut host = spawn(0, None)?;
    let mut out0 = BufReader::new(host.stdout.take().unwrap());
    let mut line = String::new();
    out0.read_line(&mut line).map_err(|e| e.to_string())?;
    let Some(code) = line.trim().strip_prefix("code ").map(str::to_string) else {
        let _ = host.kill();
        return Err(format!("seat 0 gave no room code: {line:?}"));
    };
    println!("room {code}");
    let mut guest = spawn(1, Some(&code))?;
    let out1 = BufReader::new(guest.stdout.take().unwrap());
    let read = |out: Box<dyn BufRead + Send>| {
        std::thread::spawn(move || {
            let mut result = None;
            for l in out.lines().map_while(Result::ok) {
                println!("{l}");
                if let Some(r) = l.strip_prefix("result ") {
                    result = Some(r.to_string());
                }
            }
            result
        })
    };
    let r0 = read(Box::new(out0));
    let r1 = read(Box::new(out1));
    let (r0, r1) = (r0.join().ok().flatten(), r1.join().ok().flatten());
    let _ = host.wait();
    let _ = guest.wait();
    for (seat, p) in proxies.into_iter().enumerate() {
        let s = p.stop();
        println!(
            "seat {seat}'s path: {} datagrams, {} lost, delay {:.1}-{:.1} ms",
            s.received,
            s.lost,
            s.min_delay_us as f64 / 1000.0,
            s.max_delay_us as f64 / 1000.0
        );
    }
    let parse = |r: Option<String>| -> Option<(Option<u64>, u32, u32)> {
        let r = r?;
        let w: Vec<&str> = r.split_whitespace().collect();
        let [e, a, b] = w[..] else { return None };
        Some((
            e.parse().ok(),
            u32::from_str_radix(a, 16).ok()?,
            u32::from_str_radix(b, 16).ok()?,
        ))
    };
    let (Some((e0, a0, a1)), Some((e1, b0, b1))) = (parse(r0), parse(r1)) else {
        println!("fail: a seat gave no result");
        return Ok(false);
    };
    let traded = a0 != b0 && a1 == b0 && b1 == a0;
    println!(
        "first Pokémon (personality): player 0 {a0:08x} -> {a1:08x}, player 1 {b0:08x} -> {b1:08x}"
    );
    match (e0, e1) {
        (Some(p), Some(c)) if traded => {
            println!(
                "pass: both in the Union Room (frames {p}, {c}); they traded their first Pokémon \
                 through the radio on the server"
            );
            Ok(true)
        }
        e => {
            println!("fail: in the Union Room at {e:?}, traded: {traded}");
            Ok(false)
        }
    }
}

/// Both seats and the radio's [`runtime::link::rfu::air::Air`] in this
/// process, on simulated time: each message takes `ms` each way between
/// a console and the server, rounded to quarter frames, which is when
/// each console's adapter and the air hear what's due.
fn run_sim(o: &Options, ms: f64) -> Result<bool, String> {
    use runtime::link::rfu::air::{Air, Down, Up};
    use std::collections::VecDeque;
    const SLICES: u64 = 4;
    let slice_ms = 1000.0 / 59.7275 / SLICES as f64;
    let delay = (ms / slice_ms).round() as u64;
    println!("radio in this process, {ms} ms each way ({delay} quarter frames)");
    let mut ms_: Vec<Box<Machine<Pad>>> = (0..2)
        .map(|p| {
            let pad = Pad {
                presses: o.presses[p].clone(),
                frames: 0,
                frame: Box::new([0; WIDTH * HEIGHT]),
            };
            let mut m = Box::new(Machine::new(o.rom, pad));
            let save = &o.saves[p];
            m.bus.save.data[..save.len()].copy_from_slice(save);
            m
        })
        .collect();
    let mut ws: Vec<Wireless> = ms_
        .iter_mut()
        .enumerate()
        .map(|(p, m)| {
            let mut w = Wireless::networked(m, p as u8);
            if let Some(h) = o.heard_ms {
                w.set_heard_for(h * FRAME_CYCLES * 60 / 1000);
            }
            w
        })
        .collect();
    let mut air = Air::new();
    // In flight: (due tick, seat, message).
    let mut ups: VecDeque<(u64, usize, Up)> = VecDeque::new();
    let mut downs: VecDeque<(u64, usize, Down)> = VecDeque::new();
    for s in 0..2 {
        air.hello(s, &mut |to, d| downs.push_back((delay, to, d)));
    }
    let mut last = [None; 2];
    let mut entered = [None; 2];
    let mut first = [[0u32; 2]; 2];
    let mut tick = 0u64;
    for f in 1..=o.frames {
        for _ in 0..SLICES {
            for (p, (m, w)) in ms_.iter_mut().zip(ws.iter_mut()).enumerate() {
                w.run(&mut [&mut **m], FRAME_CYCLES / SLICES, |_, m| m.run_frame());
                while let Some(u) = w.take_up() {
                    ups.push_back((tick + delay, p, u));
                }
            }
            tick += 1;
            while ups.front().is_some_and(|u| u.0 <= tick) {
                let (_, p, u) = ups.pop_front().unwrap();
                air.take(p, u, &mut |to, d| downs.push_back((tick + delay, to, d)));
            }
            while downs.front().is_some_and(|d| d.0 <= tick) {
                let (_, p, d) = downs.pop_front().unwrap();
                if let Some(w) = ws.get_mut(p) {
                    w.deliver(d);
                }
            }
        }
        for (p, m) in ms_.iter().enumerate() {
            if let Some(fault) = &m.fault {
                return Err(format!("player {p}, frame {f}: {fault}"));
            }
            let sb1 = m.bus.read32(SB1_PTR);
            let map = [m.bus.read8(sb1 + 4), m.bus.read8(sb1 + 5)];
            if f == 400 {
                first[p][0] = first_mon(m);
            }
            let (id, state) = ws[p].status(0);
            let now = (
                map,
                m.bus.read32(CALLBACK2),
                m.bus.read8(G_WIRELESS_COMM_TYPE),
                state,
                id,
                m.bus.read32(G_LINK_STATUS) & !0x100,
                position(m),
            );
            if Some(now) != last[p] {
                if now.1 == CB2_LINK_ERROR && last[p].is_none_or(|l| l.1 != CB2_LINK_ERROR) {
                    println!(
                        "frame {f}: player {p} link error {:08x} {:08x}",
                        m.bus.read32(LINK_ERROR),
                        m.bus.read32(LINK_ERROR + 4)
                    );
                }
                println!(
                    "frame {f}: player {p} map {}.{} callback2 {:08x} commType {} \
                     adapter state {} id {:04x} gLinkStatus {:#010x} at {},{}",
                    map[0], map[1], now.1, now.2, now.3, now.4, now.5, now.6.0, now.6.1
                );
                last[p] = Some(now);
            }
            if map == UNION_ROOM && entered[p].is_none() {
                entered[p] = Some(f);
            }
        }
    }
    for p in 0..2 {
        first[p][1] = first_mon(&ms_[p]);
        let used: Vec<String> = ws[p].commands[0]
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(c, n)| format!("{c:02x}:{n}"))
            .collect();
        println!("player {p} adapter commands {}", used.join(" "));
    }
    let [[a0, a1], [b0, b1]] = first;
    let traded = a0 != b0 && a1 == b0 && b1 == a0;
    println!(
        "first Pokémon (personality): player 0 {a0:08x} -> {a1:08x}, player 1 {b0:08x} -> {b1:08x}"
    );
    match entered {
        [Some(p), Some(c)] if traded => {
            println!(
                "pass: both in the Union Room (frames {p}, {c}); they traded, {ms} ms each way"
            );
            Ok(true)
        }
        e => {
            println!("fail: in the Union Room at {e:?}, traded: {traded}, {ms} ms each way");
            Ok(false)
        }
    }
}
