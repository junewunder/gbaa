//! `gbaa-interp ROM [--frames N] [--test] [--peek ADDR]… [--trace ADDR]…` runs a ROM
//! headless in the interpreter for N frames (default 60), then prints where it
//! stopped and the registers. Once a frame it checks for an idle loop (`b .`)
//! and stops there. `--peek ADDR` (hex) also prints the word at ADDR.
//! `--trace ADDR` (hex) prints the frame, line, cycle in the line and r0-r3
//! each time an instruction at ADDR is about to run.
//!
//! `--m4a-log FILE` writes, once a frame, every m4a music player whose song
//! header changed (see [`M4aLog`]): the songs the run played, by address.
//!
//! `--timing instant|exact|static` sets how the interpreter counts cycles
//! ([`runtime::Timing`]). Exact by default: this runner is for checks, which
//! run against mGBA's model; `gbaa-desktop` plays instant.
//!
//! `--test` is for jsmolka's gba-tests, which leave the failing test's
//! number in r12 (0 for a pass) and then spin in `idle: b idle`: it prints
//! `pass` or `fail N` and exits 0 only on a pass at the idle loop.
//!
//! No frame dump yet: that needs the PPU (#29).

use std::process::ExitCode;

use runtime::Machine;
use runtime::platform::Headless;

const USAGE: &str = "usage: gbaa-interp ROM [--frames N] [--test] [--peek ADDR]... [--trace ADDR]... [--m4a-log FILE] [--timing T]";

/// The m4a sound driver's state, found the way the driver finds it: the word
/// at 0x03007FF0 (`SOUND_INFO_PTR`) points to its `SoundInfo`, whose
/// `ident` is "Smsh" (one more mid critical section), and whose
/// `musicPlayerHead` (+0x24) heads the list of players (`musicPlayerNext`,
/// +0x3C). A player's `songHeader` (+0x00) is the song it last started. No
/// game's addresses.
struct M4aLog {
    out: std::io::BufWriter<std::fs::File>,
    /// Each player seen and the header it had.
    players: Vec<(u32, u32)>,
}

const M4A_IDENT: u32 = 0x6873_6D53;

impl M4aLog {
    fn new(path: &str) -> std::io::Result<M4aLog> {
        use std::io::Write;
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(
            out,
            "# m4a song starts: frame, player (its MusicPlayerInfo), song header"
        )?;
        Ok(M4aLog {
            out,
            players: Vec::new(),
        })
    }

    fn poll(&mut self, m: &Machine<Headless>, frame: u32) -> std::io::Result<()> {
        use std::io::Write;
        let info = m.bus.read32(0x0300_7FF0);
        if !(0x0300_0000..0x0300_8000).contains(&info)
            && !(0x0200_0000..0x0204_0000).contains(&info)
        {
            return Ok(());
        }
        let ident = m.bus.read32(info);
        if ident != M4A_IDENT && ident != M4A_IDENT + 1 {
            return Ok(());
        }
        let mut player = m.bus.read32(info + 0x24);
        // A list longer than this is garbage, not players.
        for _ in 0..32 {
            if player & 3 != 0 || !(0x0200_0000..0x0400_0000).contains(&player) {
                break;
            }
            let header = m.bus.read32(player);
            match self.players.iter_mut().find(|(p, _)| *p == player) {
                Some((_, h)) if *h == header => {}
                Some((_, h)) => {
                    *h = header;
                    writeln!(self.out, "{frame} {player:#010x} {header:#010x}")?;
                }
                None => {
                    self.players.push((player, header));
                    if header != 0 {
                        writeln!(self.out, "{frame} {player:#010x} {header:#010x}")?;
                    }
                }
            }
            player = m.bus.read32(player + 0x3C);
        }
        Ok(())
    }
}

/// Whether the instruction at the CPU's pc branches to itself.
fn at_idle_loop(m: &Machine<Headless>) -> bool {
    let pc = m.cpu.pc;
    if m.cpu.thumb() {
        m.bus.read16(pc & !1) == 0xE7FE
    } else {
        m.bus.read32(pc & !3) == 0xEAFF_FFFE
    }
}

fn print_state(m: &Machine<Headless>) {
    let c = &m.cpu;
    for (i, r) in c.r.iter().enumerate() {
        print!("r{i:<2} = {r:08x}{}", if i % 4 == 3 { "\n" } else { "  " });
    }
    println!("pc  = {:08x}  cpsr = {:08x}", c.pc, c.cpsr);
    println!("vcount = {}  cycles = {}", m.bus.io.vcount(), m.now());
}

/// [`Machine::run_frame`], printing a line before each instruction at an
/// address in `traces`.
fn run_frame_traced(m: &mut Machine<Headless>, frame: u32, traces: &[u32]) {
    m.sched.frame_ended = false;
    while !m.sched.frame_ended && m.fault.is_none() {
        if m.sched.halted {
            m.idle();
            continue;
        }
        m.take_irq();
        if traces.contains(&(m.cpu.pc & !1)) {
            let r = &m.cpu.r;
            println!(
                "trace {:08x} frame {frame} line {} cycle {} r0 {:08x} r1 {:08x} r2 {:08x} r3 {:08x} lr {:08x}",
                m.cpu.pc,
                m.bus.io.vcount(),
                m.now() % 1232,
                r[0],
                r[1],
                r[2],
                r[3],
                r[14]
            );
        }
        m.step();
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut rom_path, mut frames, mut test) = (None, 60u32, false);
    let (mut peeks, mut traces) = (Vec::new(), Vec::new());
    let mut m4a_path = None;
    let mut timing = runtime::Timing::Exact;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--frames" => match it.next().and_then(|n| n.parse().ok()) {
                Some(n) => frames = n,
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--test" => test = true,
            "--timing" => match it.next().and_then(|t| runtime::Timing::parse(t)) {
                Some(t) => timing = t,
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--m4a-log" => match it.next() {
                Some(p) => m4a_path = Some(p.clone()),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--peek" | "--trace" => match it
                .next()
                .and_then(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            {
                Some(addr) if a == "--peek" => peeks.push(addr),
                Some(addr) => traces.push(addr & !1),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ if rom_path.is_none() && !a.starts_with('-') => rom_path = Some(a.clone()),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(rom_path) = rom_path else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let rom = match std::fs::read(&rom_path) {
        Ok(r) if r.len() <= runtime::bus::ROM_MAX => r,
        Ok(_) => {
            eprintln!("{rom_path}: larger than 32 MB");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("{rom_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let rom: &'static [u8] = Box::leak(rom.into_boxed_slice());
    let mut m = Box::new(Machine::new(rom, Headless));
    m.set_timing(timing);
    // The game's listed spin loops (`crates/runtime/spins.toml`).
    let mut spins = Vec::new();
    runtime::spins::listed(&runtime::sha1::sha1(rom), |e| spins.push(e));
    spins.sort_unstable();
    m.spin_list = Box::leak(spins.into_boxed_slice());
    let mut m4a = match m4a_path.as_deref().map(M4aLog::new).transpose() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("--m4a-log: {e}");
            return ExitCode::from(2);
        }
    };

    let mut ran = 0;
    let mut idle = false;
    while ran < frames {
        if traces.is_empty() {
            m.run_frame();
        } else {
            run_frame_traced(&mut m, ran, &traces);
        }
        if let Some(log) = m4a.as_mut()
            && let Err(e) = log.poll(&m, ran)
        {
            eprintln!("--m4a-log: {e}");
            return ExitCode::FAILURE;
        }
        ran += 1;
        if m.fault.is_some() {
            break;
        }
        if at_idle_loop(&m) {
            idle = true;
            break;
        }
    }
    match m.fault {
        Some(fault) => println!("stopped after {ran} frames: {fault}"),
        None if idle => println!(
            "stopped after {ran} frames: idle loop at {:#010x}",
            m.cpu.pc
        ),
        None => println!("ran {ran} frames"),
    }
    if let Some(mut log) = m4a {
        use std::io::Write;
        if let Err(e) = log.out.flush() {
            eprintln!("--m4a-log: {e}");
            return ExitCode::FAILURE;
        }
    }
    print_state(&m);
    for a in peeks {
        println!("[{a:08x}] = {:08x}", m.bus.read32(a));
    }
    if !test {
        return ExitCode::SUCCESS;
    }
    let r12 = m.cpu.r[12];
    match (idle, r12) {
        (true, 0) => {
            println!("pass");
            ExitCode::SUCCESS
        }
        // A test that stops on a fault has failed; r12 says which.
        _ if idle || m.fault.is_some() => {
            println!("fail {r12}");
            ExitCode::FAILURE
        }
        _ => {
            println!("no idle loop after {ran} frames");
            ExitCode::FAILURE
        }
    }
}
