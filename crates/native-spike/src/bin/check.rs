//! `native-spike-check` (`just native-spike`): the native program against
//! its exact play build, frame by frame.
//!
//! ```text
//! native-spike-check CHECK.toml OUT PLAY ROM NATIVE PLANTED
//! ```
//!
//! Runs PLAY (the recompiled build of ROM, `--timing exact`), NATIVE and
//! PLANTED (the native program built with its one planted wrong store)
//! headless on CHECK.toml's script for its frames, each capped at 3 GB,
//! writing OUT/{play,native,planted}.hashes. Passes when the script holds
//! a D-pad key in every `move_every` frames, each run writes every
//! frame's hash, play and native match on every frame, and planted
//! differs somewhere.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use desktop::Script;
use runtime::io::key;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    frames: u64,
    input: String,
    move_every: u64,
}

const DPAD: u16 = key::UP | key::DOWN | key::LEFT | key::RIGHT;

/// One run's hashes file: `cmd` with the run's flags, under a 3 GB cap.
fn run(cmd: &[&str], c: &Check, hashes: &Path) -> Result<String, String> {
    // Not last run's file, if this one writes none.
    let _ = std::fs::remove_file(hashes);
    let status = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "-q",
            "-p",
            "MemoryMax=3G",
            "-p",
            "MemorySwapMax=0",
        ])
        .args(cmd)
        .args(["--headless", "--frames", &c.frames.to_string()])
        .args(["--input", &c.input])
        .arg("--frame-hashes")
        .arg(hashes)
        .status()
        .map_err(|e| format!("{}: {e}", cmd[0]))?;
    if !status.success() {
        return Err(format!("{}: {status}", cmd[0]));
    }
    let text = std::fs::read_to_string(hashes).map_err(|e| format!("{}: {e}", hashes.display()))?;
    let lines = text.lines().count() as u64;
    if lines != c.frames {
        return Err(format!("{}: {lines} frames, not {}", cmd[0], c.frames));
    }
    Ok(text)
}

/// The first line where `a` and `b` differ, or `None`.
fn first_diff<'a>(a: &'a str, b: &'a str) -> Option<(&'a str, &'a str)> {
    let mut a = a.lines();
    let mut b = b.lines();
    loop {
        match (a.next(), b.next()) {
            (None, None) => return None,
            (x, y) if x == y => {}
            (x, y) => return Some((x.unwrap_or("(ended)"), y.unwrap_or("(ended)"))),
        }
    }
}

/// FNV-1a 64 of a hashes file, to quote as one number.
fn digest(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn check(args: &[String]) -> Result<(), String> {
    let [toml, out, play, rom, native, planted] = args else {
        return Err("usage: native-spike-check CHECK.toml OUT PLAY ROM NATIVE PLANTED".into());
    };
    let text = std::fs::read_to_string(toml).map_err(|e| format!("{toml}: {e}"))?;
    let c: Check = toml::from_str(&text).map_err(|e| format!("{toml}: {e}"))?;
    let script = Script::parse(&c.input)?;
    let mut idle = 0;
    for f in 0..c.frames {
        idle = if script.keys(f) & DPAD != 0 {
            0
        } else {
            idle + 1
        };
        if idle >= c.move_every {
            return Err(format!(
                "the script holds no D-pad key in the {} frames to frame {f}",
                c.move_every
            ));
        }
    }
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let p = run(
        &[play, rom, "--timing", "exact"],
        &c,
        &out.join("play.hashes"),
    )?;
    let n = run(&[native], &c, &out.join("native.hashes"))?;
    let w = run(&[planted], &c, &out.join("planted.hashes"))?;
    let distinct = p
        .lines()
        .map(|l| l.rsplit(' ').next())
        .collect::<std::collections::HashSet<_>>()
        .len();
    if let Some((a, b)) = first_diff(&p, &n) {
        return Err(format!(
            "native differs from play: play {a:?}, native {b:?}"
        ));
    }
    println!(
        "native = play: {} frames identical ({distinct} distinct pictures), hashes file {:016x}, last {}",
        c.frames,
        digest(&p),
        p.lines().last().unwrap_or("")
    );
    match first_diff(&p, &w) {
        Some((a, b)) => println!("planted wrong store caught: play {a:?}, planted {b:?}"),
        None => {
            return Err(
                "the planted wrong store drew the same frames: the check can't see it".into(),
            );
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match check(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("native-spike-check: {e}");
            ExitCode::FAILURE
        }
    }
}
