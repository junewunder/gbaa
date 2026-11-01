//! `gbaa-lift-check FILE --data DIR [--only NAME]`: the stage-A lifter's
//! check (`just lift-check`; units `native/lift-stage-a`,
//! `native/lift-bars`), driven by FILE (`crates/codegen/lift-check.toml`),
//! one table per program ([`codegen::native::lift_check`]). Run from the
//! repo root, with `target/release/gbaa-lift` and `route-check` built.
//!
//! 1. **Bars.** Each program is lifted (`gbaa-lift --bars FILE`, into
//!    `out/lift-<name>`, its report in `out/lift-<name>.txt`); any metric
//!    over its bar fails the check here, before anything is built, naming
//!    the metric and both numbers.
//! 2. **The native check.** Each lift and its play build (`just recompile
//!    --timing native`) are built and `route-check --native` compares them
//!    on the program's route, capped at 3 GB. A program with
//!    `planted_store` is lifted again with the store planted, and one with
//!    `planted_flags` again with each flag (an old behaviour brought back,
//!    such as `--fall-as-tail-call`); the native check must catch each
//!    (`--planted`).
//! 3. **The planted hard hole.** A program with `planted_hole` is lifted
//!    with that stand-in dropped, from a fresh OUT: the lift and the
//!    crate's build must both fail, the build naming the hole.
//!
//! `--only NAME` runs one program's steps.

use codegen::native::lift_check::{Program, programs};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const LIFT: &str = "target/release/gbaa-lift";
const ROUTE_CHECK: &str = "target/release/route-check";

/// Run `cmd` with its output (both streams) in `log`. Whether it passed,
/// and the output.
fn run(cmd: &mut Command, log: &Path) -> Result<(bool, String), String> {
    let f = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let f2 = f.try_clone().map_err(|e| e.to_string())?;
    let st = cmd
        .stdout(Stdio::from(f))
        .stderr(Stdio::from(f2))
        .status()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    let out = std::fs::read_to_string(log).unwrap_or_default();
    Ok((st.success(), out))
}

/// Run `cmd`, failing with the tail of its log if it fails.
fn must(cmd: &mut Command, log: &Path, what: &str) -> Result<String, String> {
    let (ok, out) = run(cmd, log)?;
    if ok {
        return Ok(out);
    }
    let tail: Vec<&str> = out.lines().rev().take(20).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(format!(
        "{what} failed ({}):\n{}",
        log.display(),
        tail.join("\n")
    ))
}

fn just(args: &[&str]) -> Command {
    let mut c = Command::new("just");
    c.args(args);
    c
}

/// `gbaa-lift ROM OUT` with the program's holes file, if it has one.
fn lift(rom: &Path, name: &str, out: &str) -> Command {
    let mut c = Command::new(LIFT);
    c.arg(rom).arg(out);
    let holes = format!("holes/{name}.toml");
    if Path::new(&holes).exists() {
        c.args(["--holes", &holes]);
    }
    c
}

struct Run {
    file: String,
    data: PathBuf,
}

impl Run {
    fn rom(&self, p: &Program) -> PathBuf {
        self.data.join(&p.rom)
    }

    /// Step 1: the lift, held to its bars. The failures, if any.
    fn bars(&self, p: &Program) -> Result<Vec<String>, String> {
        let rom = self.rom(p);
        let out = format!("out/lift-{}", p.name);
        let mut c = lift(&rom, &p.name, &out);
        c.args(["--bars", &self.file]).args(&p.lift_flags);
        let (ok, text) = run(&mut c, Path::new(&format!("out/lift-{}.txt", p.name)))?;
        for l in text.lines().filter(|l| l.contains("under its bar")) {
            println!("{l}");
        }
        if ok {
            return Ok(Vec::new());
        }
        let fails: Vec<String> = text
            .lines()
            .filter(|l| {
                l.starts_with("gbaa-lift:")
                    && ((l.contains(" bar") && !l.contains("under its bar"))
                        || l.contains("hard holes;")
                        || l.contains(".toml:"))
            })
            .map(str::to_string)
            .collect();
        Ok(if fails.is_empty() {
            vec![format!(
                "gbaa-lift: {}: failed (out/lift-{}.txt)",
                p.name, p.name
            )]
        } else {
            fails
        })
    }

    /// Steps 2 and 3.
    fn native(&self, p: &Program) -> Result<(), String> {
        let rom = self.rom(p);
        let rom_s = rom.to_string_lossy().to_string();
        let n = &p.name;
        let log = |s: &str| PathBuf::from(format!("out/{s}"));
        let mut route = Command::new("systemd-run");
        route.args([
            "--user",
            "--scope",
            "-q",
            "-p",
            "MemoryMax=3G",
            "-p",
            "MemorySwapMax=0",
            ROUTE_CHECK,
            "--native",
        ]);
        route
            .arg(format!("target/recompiled/release/gbaa-native-{n}"))
            .arg(format!("target/recompiled/release/gbaa-{n}"))
            .arg(&p.route);
        if let Some(at) = p.planted_store {
            let out = format!("out/lift-{n}-planted");
            let mut c = lift(&rom, n, &out);
            c.args([
                "--name",
                &format!("{n}-planted"),
                "--plant",
                &format!("{at:#010x}"),
            ])
            .args(&p.lift_flags);
            must(
                &mut c,
                &log(&format!("lift-{n}-planted.txt")),
                "the planted store's lift",
            )?;
            must(
                &mut just(&["build-recompiled", &out]),
                &log(&format!("lift-{n}-planted.build.txt")),
                "the planted store's build",
            )?;
            route
                .arg("--planted")
                .arg(format!("target/recompiled/release/gbaa-native-{n}-planted"));
        }
        for flag in &p.planted_flags {
            let tag = flag.trim_start_matches('-');
            let out = format!("out/lift-{n}-{tag}");
            let mut c = lift(&rom, n, &out);
            c.args(["--name", &format!("{n}-{tag}"), flag])
                .args(&p.lift_flags);
            must(
                &mut c,
                &log(&format!("lift-{n}-{tag}.txt")),
                &format!("the {flag} lift"),
            )?;
            must(
                &mut just(&["build-recompiled", &out]),
                &log(&format!("lift-{n}-{tag}.build.txt")),
                &format!("the {flag} build"),
            )?;
            route
                .arg("--planted")
                .arg(format!("target/recompiled/release/gbaa-native-{n}-{tag}"));
        }
        route.args(&p.route_flags);
        let out = format!("out/lift-{n}");
        must(
            &mut just(&["build-recompiled", &out]),
            &log(&format!("lift-{n}.build.txt")),
            "the lift's build",
        )?;
        let play = format!("out/{n}");
        must(
            &mut just(&["recompile", &rom_s, &play, "--timing", "native"]),
            &log(&format!("{n}.recompile.txt")),
            "the play build's recompile",
        )?;
        must(
            &mut just(&["build-recompiled", &play]),
            &log(&format!("{n}.build.txt")),
            "the play build's build",
        )?;
        println!("lift-check: {n}: route-check --native on {}", p.route);
        let st = route.status().map_err(|e| format!("route-check: {e}"))?;
        if !st.success() {
            return Err(format!("{n}: the native check failed"));
        }
        if let Some((key, names)) = &p.planted_hole {
            // From a fresh OUT, so an old run's crate can't answer for it.
            let out = format!("out/lift-{n}-planted-hole");
            let _ = std::fs::remove_dir_all(&out);
            let mut c = lift(&rom, n, &out);
            c.args([
                "--name",
                &format!("{n}-planted-hole"),
                "--drop-standin",
                &format!("{key:#010x}"),
            ])
            .args(&p.lift_flags);
            let (lifted, _) = run(&mut c, &log(&format!("lift-{n}-planted-hole.txt")))?;
            if lifted {
                return Err(format!("{n}: the planted hard hole's lift passed"));
            }
            let blog = log(&format!("lift-{n}-planted-hole.build.txt"));
            let (built, text) = run(&mut just(&["build-recompiled", &out]), &blog)?;
            if built {
                return Err(format!("{n}: the planted hard hole's crate built"));
            }
            match text.lines().find(|l| l.contains(names.as_str())) {
                Some(l) => println!("lift-check: {n}: planted hard hole caught: {l}"),
                None => {
                    return Err(format!(
                        "{n}: the planted hard hole's build didn't name it ({names:?}; {})",
                        blog.display()
                    ));
                }
            }
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!("usage: gbaa-lift-check FILE --data DIR [--only NAME]");
        ExitCode::FAILURE
    };
    let [file, rest @ ..] = &args[..] else {
        return usage();
    };
    let (mut data, mut only) = (None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--data", Some(d)) => data = Some(PathBuf::from(d)),
            ("--only", Some(n)) => only = Some(n.clone()),
            _ => return usage(),
        }
    }
    let Some(data) = data else { return usage() };
    let mut ps = match std::fs::read_to_string(file)
        .map_err(|e| e.to_string())
        .and_then(|t| programs(&t))
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gbaa-lift-check: {file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(n) = &only {
        ps.retain(|p| &p.name == n);
        if ps.is_empty() {
            eprintln!("gbaa-lift-check: {file}: no program {n}");
            return ExitCode::FAILURE;
        }
    }
    let r = Run {
        file: file.clone(),
        data,
    };
    let missing: Vec<String> = ps
        .iter()
        .map(|p| r.rom(p))
        .filter(|p| !p.exists())
        .map(|p| p.display().to_string())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "gbaa-lift-check: no {}: run just agb-tests",
            missing.join(", ")
        );
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::create_dir_all("out") {
        eprintln!("gbaa-lift-check: out: {e}");
        return ExitCode::FAILURE;
    }
    // 1. Every program's bars first: they take seconds, the builds minutes.
    let mut over = Vec::new();
    for p in &ps {
        match r.bars(p) {
            Ok(f) => over.extend(f),
            Err(e) => over.push(format!("gbaa-lift-check: {}: {e}", p.name)),
        }
    }
    if !over.is_empty() {
        for l in &over {
            eprintln!("{l}");
        }
        eprintln!("lift-check: failed: a lift over its bars ({file})");
        return ExitCode::FAILURE;
    }
    println!(
        "lift-check: every metric within its bar for {}",
        ps.iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    // 2 and 3.
    for p in &ps {
        if let Err(e) = r.native(p) {
            eprintln!("lift-check: failed: {e}");
            return ExitCode::FAILURE;
        }
    }
    println!("lift-check: passed");
    ExitCode::SUCCESS
}
