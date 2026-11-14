//! `gbaa-first-stop OUT [--frames N] [--time-limit SECONDS] [--bin FILE]
//! [--not-built]`: where a built lift stops (`codegen::native::first_stop`;
//! unit `native/native-first-stop`). The last step of `just lift`; `just
//! first-stop OUT` runs it alone. Run from the repo root.
//!
//! Runs OUT's program (`target/recompiled/release/<the crate's name>`, or
//! FILE) `--headless --frames N --time-limit SECONDS` in a systemd scope
//! of its own capped at 3 GB with no swap (off Linux, as it is), its
//! stderr in `OUT/first-stop.log`, and writes how far it got to
//! `OUT/metrics.toml` (the `[run]` table) and the end of `OUT/report.txt`,
//! printing it. N defaults to 600, SECONDS to 120; a program still running
//! 15 s after its time limit is killed.
//!
//! A lift with hard holes can't build (each is a `compile_error!`), so
//! nothing is run: the stop is `not-built`, the holes counted from
//! `metrics.toml`, whatever binary an older lift left. `--not-built` says
//! the build failed (`just lift` passes it): with hard holes that is the
//! same stop; with none the build may only have been refused for RAM or
//! killed at its cap, which is no finding about the game, so no run is
//! recorded and an earlier one is removed. Otherwise the program must be
//! there and no older than the crate's `Cargo.toml`, `rom.gba` and
//! `src/`: an older build's binary never stands in for one that failed.
//!
//! The run keeps the commit it was made at (`[run]`'s `commit`).
//!
//! Exits 0 when the run is recorded (or, with `--not-built`, there is
//! none to record), whether or not the program stopped: a stop is a
//! finding, not a failure of the lift. Exits 2 when it couldn't run or
//! record.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use codegen::native::first_stop::{Ended, FRAMES, Run, TIME_LIMIT};
use codegen::native::metrics::Metrics;
use native_record::stop::{Stop, Why};

/// The memory cap of the program's scope.
const CAP: &str = "MemoryMax=3G";

/// Seconds past its own time limit before the program is killed.
const GRACE: u64 = 15;

/// As much of the end of the program's stderr as is read.
const TAIL: u64 = 64 << 10;

struct Args {
    out: PathBuf,
    frames: u64,
    time_limit: u64,
    bin: Option<PathBuf>,
    not_built: bool,
}

fn parse(args: &[String]) -> Option<Args> {
    let [out, rest @ ..] = args else {
        return None;
    };
    let mut a = Args {
        out: out.into(),
        frames: FRAMES,
        time_limit: TIME_LIMIT,
        bin: None,
        not_built: false,
    };
    let mut it = rest.iter();
    while let Some(f) = it.next() {
        match (f.as_str(), it.clone().next()) {
            ("--not-built", _) => {
                a.not_built = true;
                continue;
            }
            ("--frames", Some(n)) => a.frames = n.parse().ok().filter(|&n| n > 0)?,
            ("--time-limit", Some(n)) => a.time_limit = n.parse().ok().filter(|&n| n > 0)?,
            ("--bin", Some(f)) => a.bin = Some(f.into()),
            _ => return None,
        }
        it.next();
    }
    Some(a)
}

/// The crate's name in OUT's `Cargo.toml`.
fn crate_name(out: &Path) -> Result<String, String> {
    let p = out.join("Cargo.toml");
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let t: toml::Table = t.parse().map_err(|e| format!("{}: {e}", p.display()))?;
    t.get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string)
        .ok_or(format!("{}: no package name", p.display()))
}

#[cfg(unix)]
fn signal_of(s: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(s)
}

#[cfg(not(unix))]
fn signal_of(_: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// The last [`TAIL`] bytes of `log`.
fn tail(log: &Path) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(log)?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL)))?;
    let mut b = Vec::new();
    f.read_to_end(&mut b)?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

/// Run `bin` for `a`'s frames in its capped scope, stderr to `log`.
fn run(bin: &Path, a: &Args, log: &Path) -> Result<Ended, String> {
    let mut c = if cfg!(target_os = "linux") {
        let mut c = Command::new("systemd-run");
        c.args([
            "--user",
            "--scope",
            "-q",
            "-p",
            CAP,
            "-p",
            "MemorySwapMax=0",
        ])
        .arg(bin);
        c
    } else {
        Command::new(bin)
    };
    let err = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    c.args(["--headless", "--frames", &a.frames.to_string()])
        .args(["--time-limit", &a.time_limit.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(err);
    let mut child = c
        .spawn()
        .map_err(|e| format!("{}: {e}", c.get_program().to_string_lossy()))?;
    // The program stops itself at its time limit; past that, it is killed.
    let end = Instant::now() + Duration::from_secs(a.time_limit + GRACE);
    let mut killed = false;
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break s,
            None if Instant::now() >= end => {
                let _ = child.kill();
                killed = true;
                break child.wait().map_err(|e| e.to_string())?;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let mut stderr = tail(log).map_err(|e| format!("{}: {e}", log.display()))?;
    if killed {
        // It didn't stop itself: the stop line it should have printed,
        // its frames unknown.
        let s = Stop {
            frames: 0,
            asked: a.frames,
            functions: Vec::new(),
            at: String::new(),
            why: Why::TimeLimit(a.time_limit + GRACE),
        };
        stderr = format!("{stderr}\n{}\n", s.line());
    }
    Ok(Ended {
        code: status.code(),
        signal: signal_of(&status),
        stderr,
    })
}

/// The newest change time among the files of OUT a build reads:
/// `Cargo.toml`, `rom.gba` and `src/`'s.
fn newest_source(out: &Path) -> Result<std::time::SystemTime, String> {
    let time = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .map_err(|e| format!("{}: {e}", p.display()))
    };
    let mut newest = time(&out.join("Cargo.toml"))?.max(time(&out.join("rom.gba"))?);
    let src = out.join("src");
    for e in std::fs::read_dir(&src).map_err(|e| format!("{}: {e}", src.display()))? {
        let p = e.map_err(|e| format!("{}: {e}", src.display()))?.path();
        newest = newest.max(time(&p)?);
    }
    Ok(newest)
}

fn first_stop(a: &Args) -> Result<(), String> {
    let name = crate_name(&a.out)?;
    let metrics_path = a.out.join("metrics.toml");
    let report_path = a.out.join("report.txt");
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let write =
        |p: &Path, t: String| std::fs::write(p, t).map_err(|e| format!("{}: {e}", p.display()));
    let metrics = read(&metrics_path)?;
    let report = read(&report_path)?;
    let bad_metrics = |e: String| format!("{}: {e}", metrics_path.display());
    let hard_holes = Metrics::from_toml(&metrics)
        .map_err(bad_metrics)?
        .get("hard_holes")
        .ok_or_else(|| bad_metrics("no hard_holes: not a lift's metrics".into()))?;
    let mut r = if hard_holes > 0 {
        Run::not_built(a.frames, hard_holes, &report)
    } else if a.not_built {
        // Refused, killed, or code that doesn't compile: the build's own
        // output says which. Nothing about where the game stops.
        write(
            &metrics_path,
            Run::clear_metrics(&metrics).map_err(bad_metrics)?,
        )?;
        write(&report_path, Run::clear_report(&report))?;
        println!(
            "gbaa-first-stop: {name}: no run recorded: the build failed with no hard hole in the lift (refused for RAM, killed at its cap, or code that doesn't compile: see the build's output)"
        );
        return Ok(());
    } else {
        let bin = a
            .bin
            .clone()
            .unwrap_or_else(|| Path::new("target/recompiled/release").join(&name));
        let built = std::fs::metadata(&bin)
            .and_then(|m| m.modified())
            .map_err(|e| {
                format!(
                    "{}: {e} (`just build-recompiled {}`; a build that failed is `--not-built`)",
                    bin.display(),
                    a.out.display()
                )
            })?;
        if built < newest_source(&a.out)? {
            return Err(format!(
                "{}: older than {}'s sources, so not this lift's build (`just build-recompiled {}`; a build that failed is `--not-built`)",
                bin.display(),
                a.out.display(),
                a.out.display()
            ));
        }
        Run::of(a.frames, &run(&bin, a, &a.out.join("first-stop.log"))?)
    };
    r.commit = analysis::numbers_doc::commit();
    let with_run = r.write_metrics(&metrics).map_err(bad_metrics)?;
    write(&metrics_path, with_run)?;
    write(&report_path, r.write_report(&report))?;
    let mut lines = r.lines().into_iter();
    println!(
        "gbaa-first-stop: {name}: {}",
        lines.next().unwrap_or_default()
    );
    for l in lines {
        println!("  {l}");
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(a) = parse(&args) else {
        eprintln!(
            "usage: gbaa-first-stop OUT [--frames N] [--time-limit SECONDS] [--bin FILE] [--not-built]"
        );
        return ExitCode::from(2);
    };
    match first_stop(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gbaa-first-stop: {e}");
            ExitCode::from(2)
        }
    }
}
