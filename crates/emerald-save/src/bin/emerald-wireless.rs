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

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use emerald_save::{G_LINK_STATUS, Presses, SB1_PARTY, SB1_PTR};
use runtime::Machine;
use runtime::link::rfu::Wireless;
use runtime::platform::{Frame, HEIGHT, Platform, WIDTH};
use runtime::sched::FRAME_CYCLES;

const USAGE: &str = "usage: emerald-wireless ROM SAVE_DIR [--frames N] [--once] \
                     [--dump DIR --dump-at N,N...] [--input0 PRESSES] [--input1 PRESSES]";

/// `MAP_UNION_ROOM`: group 25, number 60.
const UNION_ROOM: [u8; 2] = [25, 60];
/// `gMain.callback2`, `gRfu`'s state byte and `gWirelessCommType`.
const CALLBACK2: u32 = 0x0300_22C4;
const G_WIRELESS_COMM_TYPE: u32 = 0x0300_30FC;
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
    for p in 0..2 {
        let used: Vec<String> = radio.commands[p]
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| USAGE.to_string());
        match a.as_str() {
            "--frames" => frames = val()?.parse().map_err(|_| USAGE)?,
            "--once" => once = true,
            "--input0" => presses[0] = val()?.clone(),
            "--input1" => presses[1] = val()?.clone(),
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
