//! `route-check units/NAME.toml [--through CHECKPOINT]`: one route's check
//! (`just unit NAME`, docs/factory.md). Runs the route's ROM to its last
//! checkpoint (or CHECKPOINT) once each in mGBA, our interpreter and the
//! recompiled build, from empty outputs in out/units/NAME/, and fails at the
//! first checkpoint, by name, where mGBA doesn't draw its hash (or its
//! known-wrong `mgba_hash`), our interpreter doesn't draw it, or the
//! recompiled build's frames, registers or compiled share part from the
//! interpreter's. A failure names the PNGs to look at.
//!
//! `route-check --missing units/*.toml`: the ROMs and states the routes
//! name that aren't on disk, one a line (exit 1 if any; `just rebuildable`).
//!
//! `route-check --same-states A B [--every N] units/NAME.toml…`: two
//! builds' exact states at every checkpoint, byte for byte; A and B are
//! binaries, each optionally followed by its flags (`"BIN
//! --interp-blocks"`) ([`same_states`]).
//!
//! `route-check --mod-run STOCK MOD [--dump-at N,…] units/NAME.toml`: a
//! mod's frames beside stock's, a side-by-side PNG of each pair that
//! differs (`just mod-run`, [`mod_run`]).
//!
//! `route-check --args units/NAME.toml`: the arguments every runner is
//! given for the route (ROM, frames, presses, state, dump frames), then
//! `--timing exact` (gbaa-desktop's and a recompiled build's, not
//! mgba-dump's), one a line, to run another build along it by hand (v1
//! against structured).
//!
//! `route-check --windows units/NAME.toml [--through CHECKPOINT]`: the
//! Windows build against the Linux one, to the first checkpoint
//! ([`windows`]).
//!
//! `route-check --native NATIVE PLAY ROUTE [--replaced FILE] [--packed
//! BIN] [--planted BIN]`: a native program against its play build at
//! `--timing native`, every frame's picture, RAM, I/O, display memories and
//! audio ([`native`], `just native-check`).
//!
//! `--timing T` anywhere on the line runs our builds and the recompile at
//! T instead of `exact` (`native`: docs/native-port-design.md, section
//! 7); mGBA keeps its own.
//!
//! Run from the repo's root, with the binaries built (`just unit` does
//! both): it calls `just recompile` and `just build-recompiled`, which take
//! the build lock themselves. Exits 1 on a failure, 2 on a bad route file,
//! 75 when this machine couldn't run the check (a build refused or broke).

mod mod_run;
mod native;
mod route;
mod same_states;
mod windows;

use route::{Checkpoint, Route, Vars};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const USAGE: &str = "usage: route-check units/NAME.toml [--through CHECKPOINT]\n       \
                     route-check --missing ROUTE.toml…\n       \
                     route-check --same-states A B [--every N] ROUTE.toml…\n       \
                     route-check --mod-run STOCK MOD [--dump-at N,…] ROUTE.toml\n       \
                     route-check --args ROUTE.toml\n       \
                     route-check --windows units/NAME.toml [--through CHECKPOINT]\n       \
                     route-check --native NATIVE PLAY ROUTE [--replaced FILE] [--packed BIN] \
                     [--planted BIN]... [--no-audio WHY]\n       \
                     (any of them with --timing T: our builds at T, not exact)";

/// How a check ended, other than passing.
enum Stop {
    /// The unit fails: what failed.
    Fail(String),
    /// The route file is wrong, or the command line.
    Bad(String),
    /// This machine couldn't run it: not a verdict on the unit.
    Unrun(String),
}

use Stop::*;

/// `--timing`'s word: what our runners and the recompile are given.
static TIMING: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The timing our builds run at: `exact` unless `--timing` said.
fn timing() -> &'static str {
    TIMING.get().map_or("exact", String::as_str)
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--timing") {
        let Some(t) = args.get(i + 1).cloned() else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        };
        args.drain(i..i + 2);
        let _ = TIMING.set(t);
    }
    let vars = Vars::from_env();
    if args.first().map(String::as_str) == Some("--missing") {
        return missing(&args[1..], &vars);
    }
    if let [flag, p] = args.as_slice()
        && flag == "--args"
    {
        return match Route::read(Path::new(p), &vars) {
            Ok(r) => {
                // Our runners' too: a route is a check, at mGBA's timing.
                for a in route_args(&r)
                    .iter()
                    .map(String::as_str)
                    .chain(["--timing", timing()])
                {
                    println!("{a}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        };
    }
    if let [flag, a, b, rest @ ..] = args.as_slice()
        && flag == "--same-states"
    {
        let (every, routes) = match rest {
            [e, n, routes @ ..] if e == "--every" => match n.parse() {
                Ok(n) => (n, routes),
                Err(_) => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            routes => (0, routes),
        };
        if routes.is_empty() {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        return report("same-states", same_states::run(a, b, every, routes, &vars));
    }
    if let [flag, stock, modded, rest @ ..] = args.as_slice()
        && flag == "--mod-run"
    {
        let (at, route) = match rest {
            [d, list, route] if d == "--dump-at" => {
                match list
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<Vec<u64>, _>>()
                {
                    Ok(at) => (at, route),
                    Err(_) => {
                        eprintln!("{USAGE}");
                        return ExitCode::from(2);
                    }
                }
            }
            [route] => (Vec::new(), route),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        };
        return report("mod-run", mod_run::run(stock, modded, &at, route, &vars));
    }
    if let [flag, native, play, route, rest @ ..] = args.as_slice()
        && flag == "--native"
    {
        let extra = match native::parse_extra(rest) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("{e}\n{USAGE}");
                return ExitCode::from(2);
            }
        };
        let name = Path::new(route)
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned());
        return report(
            &format!("native {name}"),
            native::run(native, play, route, &extra, &vars),
        );
    }
    let (windows, args) = match args.split_first() {
        Some((w, rest)) if w == "--windows" => (true, rest),
        _ => (false, args.as_slice()),
    };
    let (path, through) = match args {
        [p] => (p, None),
        [p, t, n] if t == "--through" => (p, Some(n)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let name = Path::new(path)
        .file_stem()
        .map_or(String::new(), |s| s.to_string_lossy().into_owned());
    let route = Route::read(Path::new(path), &vars).and_then(|r| match through {
        Some(t) => r.through(t),
        // The Windows check stops at the first checkpoint unless told.
        None if windows => match r.checkpoints.first() {
            Some(c) => {
                let first = c.name.clone();
                r.through(&first)
            }
            None => Ok(r),
        },
        None => Ok(r),
    });
    if windows {
        let result = route.map_err(Bad).and_then(|r| windows::check(&name, &r));
        return report(&format!("windows {name}"), result);
    }
    let result = route.map_err(Bad).and_then(|r| check(&name, &r));
    report(&format!("unit {name}"), result)
}

/// Print how a check ended, as `WHAT: pass (…)` or `WHAT: FAIL: …`, and
/// exit with its code.
fn report(name: &str, result: Result<String, Stop>) -> ExitCode {
    match result {
        Ok(said) => {
            println!("{name}: pass ({said})");
            ExitCode::SUCCESS
        }
        Err(Fail(e)) => {
            println!("{name}: FAIL: {e}");
            ExitCode::from(1)
        }
        Err(Bad(e)) => {
            eprintln!("{name}: {e}");
            ExitCode::from(2)
        }
        Err(Unrun(e)) => {
            println!("{name}: could not run: {e}");
            ExitCode::from(75)
        }
    }
}

/// `--missing`: print each route's ROM or state that isn't on disk.
fn missing(paths: &[String], vars: &Vars) -> ExitCode {
    let mut n = 0;
    for p in paths {
        match Route::read(Path::new(p), vars) {
            Ok(r) => {
                for (what, f) in [("rom", Some(&r.rom)), ("state", r.state.as_ref())] {
                    if let Some(f) = f
                        && !f.exists()
                    {
                        println!("{p}: {what} {} is missing", f.display());
                        n += 1;
                    }
                }
            }
            Err(e) => {
                println!("{e}");
                n += 1;
            }
        }
    }
    if n == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// A runner's results in `dir`: its frames' hashes, one per frame.
struct Run {
    dir: PathBuf,
    hashes: Vec<String>,
}

impl Run {
    /// The hash drawn at `frame`, as 16 hex digits.
    fn hash(&self, frame: u64) -> &str {
        self.hashes
            .get(frame as usize - 1)
            .and_then(|l| l.rsplit(' ').next())
            .unwrap_or("(none)")
    }

    fn png(&self, frame: u64) -> PathBuf {
        self.dir.join(format!("frame-{frame}.png"))
    }

    /// The first frame (from 1) whose hash differs from `other`'s, but for
    /// the frames in `except`.
    fn first_difference(&self, other: &Run, except: &[u64]) -> Option<u64> {
        let n = self.hashes.len().max(other.hashes.len());
        (0..n)
            .find(|&i| {
                !except.contains(&(i as u64 + 1)) && self.hashes.get(i) != other.hashes.get(i)
            })
            .map(|i| i as u64 + 1)
    }
}

fn check(name: &str, r: &Route) -> Result<String, Stop> {
    for (what, f) in [("ROM", Some(&r.rom)), ("state", r.state.as_ref())] {
        if let Some(f) = f
            && !f.exists()
        {
            return Err(Bad(format!("{what} {} is missing", f.display())));
        }
    }
    let out = PathBuf::from("out/units").join(name);
    // Never read a stale dump.
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    }
    std::fs::create_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    let args = route_args(r);
    let n = r.checkpoints.len();
    let last = r.frames();
    println!(
        "unit {name}: {n} checkpoint{} to frame {last}",
        if n == 1 { "" } else { "s" }
    );

    // 1. mGBA, the oracle.
    let mgba = runner(&out, "mgba", "target/release/mgba-dump", &args, &[])?;
    for c in &r.checkpoints {
        let theirs = mgba.hash(c.frame);
        if theirs == c.mgba_want() {
            continue;
        }
        let at = at_checkpoint(c);
        if c.mgba_hash.is_some() && theirs == c.hash {
            return Err(Fail(format!(
                "{at}: mGBA now draws its hash, so it isn't wrong any more: drop mgba_hash and \
                 mgba_why"
            )));
        }
        return Err(Fail(format!(
            "{at}: mGBA drew {theirs}, want {} ({})",
            c.mgba_want(),
            mgba.png(c.frame).display()
        )));
    }

    // 2. Our interpreter.
    let interp = runner(
        &out,
        "interp",
        "target/release/gbaa-desktop",
        &args,
        &["--headless", "--regs", "--timing", timing()],
    )?;
    for c in &r.checkpoints {
        let ours = interp.hash(c.frame);
        if ours == c.hash {
            continue;
        }
        let mut pngs = format!(
            "{} and {}",
            mgba.png(c.frame).display(),
            interp.png(c.frame).display()
        );
        if c.mgba_hash.is_none() {
            let diff = out.join(format!("diff-{}.png", c.name));
            let said = Command::new("target/release/png-diff")
                .arg(mgba.png(c.frame))
                .arg(interp.png(c.frame))
                .arg(&diff)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            write!(pngs, ", {}: {said}", diff.display()).unwrap();
        }
        let first = interp
            .first_difference(&mgba, &[])
            .map_or(String::new(), |f| {
                format!("; ours first differs from mGBA's at frame {f}")
            });
        return Err(Fail(format!(
            "{}: the interpreter drew {ours}, want {} ({pngs}){first}",
            at_checkpoint(c),
            c.hash
        )));
    }
    println!("unit {name}: mGBA and the interpreter draw every checkpoint");

    // 3. The recompiled build, against the interpreter. Its crate is the
    // game's, in out/<game>/.
    let bin = build_recompiled(r, &out)?;
    let rec = runner(
        &out,
        "recompiled",
        &bin,
        &args,
        &["--headless", "--regs", "--timing", timing()],
    )?;
    let excused: Vec<u64> = r
        .checkpoints
        .iter()
        .flat_map(|c| c.block_timed.iter().copied())
        .collect();
    if let Some(&f) = excused.iter().find(|&&f| rec.hash(f) == interp.hash(f)) {
        return Err(Fail(format!(
            "the recompiled build's frame {f} is the interpreter's now: drop it from block_timed \
             (and block_timed_why, if it was the last)"
        )));
    }
    if let Some(f) = rec.first_difference(&interp, &excused) {
        let before = r
            .checkpoints
            .iter()
            .find(|c| c.frame >= f)
            .map_or(String::new(), |c| format!(" (before {})", at_checkpoint(c)));
        return Err(Fail(format!(
            "the recompiled build's frame {f} differs from the interpreter's{before}: {} and {}",
            out.join("interp-hashes.txt").display(),
            out.join("recompiled-hashes.txt").display()
        )));
    }
    let (ours, theirs) = (
        read(&out.join("interp.txt"))?,
        read(&out.join("recompiled.txt"))?,
    );
    if ours != theirs {
        return Err(Fail(format!(
            "at frame {last}, the recompiled build's registers differ from the interpreter's:\n\
             interpreter:\n{ours}recompiled:\n{theirs}"
        )));
    }
    let log = read(&out.join("recompiled.log"))?;
    let mut shares = Vec::new();
    for c in &r.checkpoints {
        if std::fs::read(interp.png(c.frame)).ok() != std::fs::read(rec.png(c.frame)).ok() {
            return Err(Fail(format!(
                "{}: the recompiled PNG differs from the interpreter's ({} and {})",
                at_checkpoint(c),
                interp.png(c.frame).display(),
                rec.png(c.frame).display()
            )));
        }
        if let Some(min) = c.min_compiled {
            let share = compiled_share(&log, c.frame).ok_or(Fail(format!(
                "{}: min_compiled is set but {} has no cycles line for it",
                at_checkpoint(c),
                out.join("recompiled.log").display()
            )))?;
            if share < min {
                return Err(Fail(format!(
                    "{}: compiled share {share:.4} is under min_compiled {min} (the rest ran in \
                     the interpreter fallback)",
                    at_checkpoint(c)
                )));
            }
            shares.push(format!("{} {share:.4} >= {min}", c.name));
        }
    }
    if let Some(cycles) = log.lines().find(|l| l.starts_with("cycles:")) {
        println!("{cycles}");
    }
    let names: Vec<&str> = r.checkpoints.iter().map(|c| c.name.as_str()).collect();
    let mut said = format!(
        "{} to frame {last}; {}; recompiled = interpreted",
        names.join(", "),
        mgba_said(&r.checkpoints)
    );
    if !excused.is_empty() {
        let frames: Vec<String> = excused.iter().map(u64::to_string).collect();
        write!(said, " but block-timed frame {}", frames.join(", ")).unwrap();
    }
    if !shares.is_empty() {
        write!(said, "; compiled share {}", shares.join(", ")).unwrap();
    }
    Ok(said)
}

/// What every runner is given: the ROM, frames to the last checkpoint,
/// the route's presses and state, and a PNG at each checkpoint.
fn route_args(r: &Route) -> Vec<String> {
    let frames = r.frames().to_string();
    let at: Vec<String> = r.checkpoints.iter().map(|c| c.frame.to_string()).collect();
    let mut args: Vec<String> = vec![r.rom.display().to_string(), "--frames".into(), frames];
    if !r.input.is_empty() {
        args.extend(["--input".into(), r.input.clone()]);
    }
    if let Some(s) = &r.state {
        args.extend(["--state".into(), s.display().to_string()]);
    }
    args.extend(["--dump-at".into(), at.join(",")]);
    args
}

/// Recompile the route's ROM into its game's crate, out/<game>/, and build
/// it (`just recompile`, `just build-recompiled`), logging the first to
/// `out`; returns the binary.
fn build_recompiled(r: &Route, out: &Path) -> Result<String, Stop> {
    let game = game_name(&r.rom);
    let crate_dir = format!("out/{game}");
    let log = out.join("recompile.txt");
    let recompiled = Command::new("just")
        // A check: mGBA's timing, cycle for cycle (play builds are instant).
        .args([
            "recompile",
            &r.rom.display().to_string(),
            &crate_dir,
            "--timing",
            timing(),
        ])
        .stdout(file(&log)?)
        .stderr(file(&log)?)
        .status()
        .map_err(|e| Unrun(format!("just recompile: {e}")))?;
    if !recompiled.success() {
        return Err(Fail(format!(
            "recompiling failed ({}: {})",
            log.display(),
            tail(&log)
        )));
    }
    let built = Command::new("just")
        .args(["build-recompiled", &crate_dir])
        .status()
        .map_err(|e| Unrun(format!("just build-recompiled: {e}")))?;
    if !built.success() {
        return Err(Unrun("build-recompiled refused or failed (above)".into()));
    }
    Ok(format!("target/recompiled/release/gbaa-{game}"))
}

/// Run `bin` with `args` and `extra`, capped at 3 GB, writing its hashes,
/// PNGs, stdout (`NAME.txt`) and stderr (`NAME.log`) into `out`.
fn runner(out: &Path, name: &str, bin: &str, args: &[String], extra: &[&str]) -> Result<Run, Stop> {
    let mut cmd = Command::new("systemd-run");
    cmd.args([
        "--user",
        "--scope",
        "-q",
        "-p",
        "MemoryMax=3G",
        "-p",
        "MemorySwapMax=0",
    ])
    .arg(bin);
    run_as(out, name, cmd, bin, args, extra)
}

/// [`runner`]'s work, with `cmd` (which runs `bin`) as the runner.
fn run_as(
    out: &Path,
    name: &str,
    mut cmd: Command,
    bin: &str,
    args: &[String],
    extra: &[&str],
) -> Result<Run, Stop> {
    let dir = out.join(name);
    std::fs::create_dir_all(&dir).map_err(|e| Unrun(format!("{}: {e}", dir.display())))?;
    let hashes = out.join(format!("{name}-hashes.txt"));
    let (stdout, stderr) = (
        out.join(format!("{name}.txt")),
        out.join(format!("{name}.log")),
    );
    let status = cmd
        .args(args)
        .args(extra)
        .arg("--dump-dir")
        .arg(&dir)
        .arg("--frame-hashes")
        .arg(&hashes)
        .stdin(Stdio::null())
        .stdout(file(&stdout)?)
        .stderr(file(&stderr)?)
        .status()
        .map_err(|e| Unrun(format!("{bin}: {e}")))?;
    if !status.success() {
        return Err(Fail(format!(
            "{bin} exited with an error ({}: {})",
            stderr.display(),
            tail(&stderr)
        )));
    }
    let hashes = read(&hashes)?.lines().map(String::from).collect();
    Ok(Run { dir, hashes })
}

fn at_checkpoint(c: &Checkpoint) -> String {
    format!("checkpoint {} (frame {})", c.name, c.frame)
}

/// What the pass line says of mGBA.
fn mgba_said(cs: &[Checkpoint]) -> String {
    let wrong: Vec<String> = cs
        .iter()
        .filter_map(|c| Some(format!("{}: {}", c.name, c.mgba_why.as_ref()?)))
        .collect();
    if wrong.is_empty() {
        "mGBA matches".into()
    } else {
        format!("mGBA known wrong at {}", wrong.join("; "))
    }
}

/// The share of running guest cycles spent in compiled code up to `frame`,
/// from the recompiled build's `at frame N cycles: C compiled, I in the
/// interpreter` line.
fn compiled_share(log: &str, frame: u64) -> Option<f64> {
    let prefix = format!("at frame {frame} cycles: ");
    let rest = log.lines().find_map(|l| l.strip_prefix(&prefix))?;
    let (c, rest) = rest.split_once(" compiled, ")?;
    let i = rest.strip_suffix(" in the interpreter")?;
    let (c, i): (f64, f64) = (c.parse().ok()?, i.parse().ok()?);
    Some(if c + i > 0.0 { c / (c + i) } else { 0.0 })
}

/// The name `just recompile` gives ROM's crate: its file name, lower case,
/// each run of other characters a '-'.
fn game_name(rom: &Path) -> String {
    let stem = rom
        .file_stem()
        .map_or(String::new(), |s| s.to_string_lossy().to_lowercase());
    let mut name = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            name.push(ch);
        } else if !name.ends_with('-') {
            name.push('-');
        }
    }
    name.trim_matches('-').to_string()
}

fn file(path: &Path) -> Result<std::fs::File, Stop> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Unrun(format!("{}: {e}", path.display())))
}

fn read(path: &Path) -> Result<String, Stop> {
    std::fs::read_to_string(path).map_err(|e| Unrun(format!("{}: {e}", path.display())))
}

/// A file's last line, for a failure message.
fn tail(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.lines().last().map(String::from))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_games_as_just_recompile_does() {
        let n = |s: &str| game_name(Path::new(s));
        assert_eq!(
            n("/r/Pokemon - Emerald Version.gba"),
            "pokemon-emerald-version"
        );
        assert_eq!(n("/r/Emerald Rogue.gba"), "emerald-rogue");
        assert_eq!(n("/d/agb/gba-cart1.gba"), "gba-cart1");
        assert_eq!(n("/d/(x).gba"), "x");
    }

    #[test]
    fn reads_the_compiled_share() {
        let log = "recompiled: 3 calls\nat frame 10 cycles: 75 compiled, 25 in the interpreter\n";
        assert_eq!(compiled_share(log, 10), Some(0.75));
        assert_eq!(compiled_share(log, 11), None);
        let none = "at frame 1 cycles: 0 compiled, 0 in the interpreter";
        assert_eq!(compiled_share(none, 1), Some(0.0));
    }

    #[test]
    fn finds_the_first_differing_frame() {
        let run = |h: &[&str]| Run {
            dir: PathBuf::new(),
            hashes: h
                .iter()
                .enumerate()
                .map(|(i, h)| format!("frame {} hash {h}", i + 1))
                .collect(),
        };
        let (a, b) = (run(&["1", "2", "3"]), run(&["1", "2", "4"]));
        assert_eq!(a.first_difference(&b, &[]), Some(3));
        assert_eq!(a.first_difference(&b, &[3]), None);
        assert_eq!(run(&["0", "2", "4"]).first_difference(&b, &[1]), None);
        assert_eq!(a.first_difference(&a, &[]), None);
        assert_eq!(a.first_difference(&run(&["1", "2"]), &[]), Some(3));
        assert_eq!(a.hash(2), "2");
    }
}
