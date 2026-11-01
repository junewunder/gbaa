//! `emerald-link ROM SAVE_DIR [--frames N] [--dump DIR --dump-at N,N…]
//! [--input0 PRESSES] [--input1 PRESSES]`: the link hub's check
//! (multiplayer/link-hub). Two Emeralds, `alice.sav` as the parent and
//! `bob.sav` as the child (`just emerald-saves`), run in the interpreter on
//! one cable (`runtime::link`). Each continues its game, talks to Oldale's
//! cable club attendant, picks the Trade Center and saves, and the game
//! opens the link; the parent's A confirms it, and both walk into the
//! Trade Center. It prints each player's `gLink.state` and `gLinkStatus`
//! when they change, runs the whole thing twice, and passes (exit 0) when
//! both players reached `LINK_STATE_CONN_ESTABLISHED` and the Trade Center
//! and the two runs printed the same and ended in the same machine states.
//! About a minute.
//!
//! `--input0`/`--input1` replace a player's presses, as
//! `button@frame[+held]` separated by commas (frames from 0, as
//! `gbaa-desktop --input` counts them), or `button@from-to/every[+held]`
//! for a press every `every` frames; `--dump-at` writes each player's
//! frame N as DIR/pP-N.png after the first run's frame N.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use emerald_save::{
    CONN_ESTABLISHED, G_LINK, G_LINK_STATUS, Presses, SB1_PTR, TRADE_CENTER,
    TRADE_CENTER_PRESSES as PRESSES,
};
use runtime::Machine;
use runtime::link::Link;
use runtime::platform::{Frame, HEIGHT, Platform, WIDTH};
use runtime::sched::FRAME_CYCLES;

const USAGE: &str = "usage: emerald-link ROM SAVE_DIR [--frames N] [--dump DIR --dump-at N,N...] \
                     [--input0 PRESSES] [--input1 PRESSES]";

const FRAMES: u64 = 4000;

/// A player's front end: counts frames, keeps the last one, and holds the
/// presses for the frame it is about to run (latched at the V-blank).
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
    presses: [Presses; 2],
    dump: Option<(PathBuf, Vec<u64>)>,
}

/// What a run printed, and a hash of each machine's whole state at its end.
struct Outcome {
    log: Vec<String>,
    established: [Option<u64>; 2],
    /// The first frame each player stood in the Trade Center.
    entered: [Option<u64>; 2],
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
    let mut link = Link::new(&mut [&mut **a, &mut **b]);
    let mut out = Outcome {
        log: Vec::new(),
        established: [None; 2],
        entered: [None; 2],
        hashes: [0; 2],
        transfers: 0,
    };
    let mut last = [(u8::MAX, u32::MAX); 2];
    for f in 1..=o.frames {
        link.run(&mut [&mut **a, &mut **b], FRAME_CYCLES, |_, m| {
            m.run_frame()
        });
        for (p, m) in [&a, &b].into_iter().enumerate() {
            if let Some(fault) = &m.fault {
                return Err(format!("player {p}, frame {f}: {fault}"));
            }
            // Less `LINK_STAT_RECEIVED_NOTHING`, which flickers frame to frame.
            let now = (
                m.bus.read8(G_LINK + 1),
                m.bus.read32(G_LINK_STATUS) & !0x100,
            );
            if now != last[p] {
                out.log.push(format!(
                    "frame {f}: player {p} gLink.state {} gLinkStatus {:#010x} (bit 8 aside)",
                    now.0, now.1
                ));
                last[p] = now;
            }
            if now.0 == CONN_ESTABLISHED && out.established[p].is_none() {
                out.established[p] = Some(f);
            }
            let sb1 = m.bus.read32(SB1_PTR);
            let map = [m.bus.read8(sb1 + 4), m.bus.read8(sb1 + 5)];
            if map == TRADE_CENTER && out.entered[p].is_none() {
                out.entered[p] = Some(f);
                out.log
                    .push(format!("frame {f}: player {p} in the Trade Center"));
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
    out.transfers = link.transfers;
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
    let mut presses = [PRESSES.to_string(), PRESSES.to_string()];
    let (mut dir, mut at) = (None, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| USAGE.to_string());
        match a.as_str() {
            "--frames" => frames = val()?.parse().map_err(|_| USAGE)?,
            "--input0" => presses[0] = val()?.clone(),
            "--input1" => presses[1] = val()?.clone(),
            "--dump" => dir = Some(PathBuf::from(val()?)),
            "--dump-at" => {
                at = val()?
                    .split(',')
                    .map(|n| n.parse().map_err(|_| USAGE.to_string()))
                    .collect::<Result<_, _>>()?
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
        presses: [Presses::parse(&presses[0])?, Presses::parse(&presses[1])?],
        dump: dir.map(|d| (d, at)),
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = match options(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("emerald-link: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some((dir, _)) = &o.dump
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("emerald-link: {}: {e}", dir.display());
        return ExitCode::from(2);
    }
    let runs = match run(&o, true).and_then(|a| Ok((a, run(&o, false)?))) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("emerald-link: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (a, b) = &runs;
    for l in &a.log {
        println!("{l}");
    }
    println!(
        "{} frames, {} transfers; states {:016x} {:016x}",
        o.frames, a.transfers, a.hashes[0], a.hashes[1]
    );
    let same = a.log == b.log && a.hashes == b.hashes && a.transfers == b.transfers;
    println!(
        "second run: {}",
        if same { "identical" } else { "DIFFERENT" }
    );
    match (a.established, a.entered) {
        ([Some(p), Some(c)], [Some(_), Some(_)]) if same => {
            println!(
                "pass: LINK_STATE_CONN_ESTABLISHED at frame {p} (parent), {c} (child); \
                 both in the Trade Center"
            );
            ExitCode::SUCCESS
        }
        (e, t) => {
            println!("fail: established at {e:?}, in the Trade Center at {t:?}");
            ExitCode::FAILURE
        }
    }
}
