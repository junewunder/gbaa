//! `gbaa-present-sim [--scale X]… TIMES…`: what a 60 Hz display shows of a
//! single-player game's frames (deck/even-presents; `desktop::present`).
//! Each TIMES is a `gbaa-desktop --headless --frame-times` file; each
//! frame's cost is scaled by every `--scale` given (1 if none: e.g. 0.7
//! for a recompiled build, 2.2 for the Deck). For each, paced by the sound
//! and by the muted clock, prints a minute's doubled, skipped and late
//! frames and the display lag for frames handed over when done, 8 ms into
//! their turn (netplay's) and after the slowest recent frame (the
//! window's), averaged over 16 display phases.

use desktop::present::{Pacer, Present, Shown, read_times, sim_phases};

fn main() {
    let mut scales = Vec::new();
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--scale" => match args.next().and_then(|v| v.parse::<f64>().ok()) {
                Some(s) if s > 0.0 => scales.push(s),
                _ => fail("--scale takes a number over 0"),
            },
            "-h" | "--help" => {
                println!("usage: gbaa-present-sim [--scale X]… TIMES…");
                return;
            }
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        fail("usage: gbaa-present-sim [--scale X]… TIMES…");
    }
    if scales.is_empty() {
        scales.push(1.0);
    }
    println!(
        "| trace | scale | pacer | present | doubled/min | skipped/min | late/min | beat/min | lag ms |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| fail(&format!("{f}: {e}")));
        let costs = read_times(&text).unwrap_or_else(|e| fail(&format!("{f}: {e}")));
        let name = std::path::Path::new(f)
            .file_stem()
            .map_or(f.clone(), |s| s.to_string_lossy().into_owned());
        for &scale in &scales {
            let c: Vec<f64> = costs.iter().map(|x| x * scale).collect();
            for pacer in [Pacer::Sound, Pacer::Clock] {
                for (label, p) in [
                    ("when done", Present::Done),
                    ("8 ms", Present::Fixed(8_000.0)),
                    ("slowest", Present::Slowest),
                ] {
                    let s = sim_phases(&c, pacer, p, 16);
                    row(&name, scale, pacer, label, &s);
                }
            }
        }
    }
}

fn row(name: &str, scale: f64, pacer: Pacer, label: &str, s: &Shown) {
    let m = |n| format!("{:.0}", s.per_minute(n));
    println!(
        "| {name} | {scale} | {} | {label} | {} | {} | {} | {} | {:.1} |",
        match pacer {
            Pacer::Sound => "sound",
            Pacer::Clock => "muted",
        },
        m(s.doubled),
        m(s.skipped),
        m(s.late),
        m(s.beat),
        s.lag_us / 1e3
    );
}

fn fail(msg: &str) -> ! {
    eprintln!("gbaa-present-sim: {msg}");
    std::process::exit(2);
}
