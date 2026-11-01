//! `route-check --windows units/NAME.toml [--through CHECKPOINT]`: the
//! Windows build's check (`just windows-check NAME`, stretch/windows).
//! Builds the route's game for Windows (`just windows-exe`) and for Linux
//! (`just build-recompiled`), runs both headless to the route's first
//! checkpoint (or CHECKPOINT), the .exe under Wine in the `gbaa-windows`
//! container, and fails unless every frame's hash, each checkpoint's PNG,
//! the last registers and the cartridge save match, and each checkpoint
//! draws the route's hash. Outputs go in out/units/NAME-windows/.

use crate::route::Route;
use crate::{Run, Stop, build_recompiled, game_name, read, route_args, run_as, runner};
use Stop::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// The image `just windows-image` builds.
const IMAGE: &str = "gbaa-windows";

pub fn check(name: &str, r: &Route) -> Result<String, Stop> {
    for f in std::iter::once(&r.rom).chain(&r.state) {
        if !f.exists() {
            return Err(Bad(format!("{} is missing", f.display())));
        }
    }
    let out = PathBuf::from("out/units").join(format!("{name}-windows"));
    // Never read a stale dump.
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    }
    std::fs::create_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    let game = game_name(&r.rom);
    let built = Command::new("just")
        .arg("windows-exe")
        .arg(&r.rom)
        .status()
        .map_err(|e| Unrun(format!("just windows-exe: {e}")))?;
    if !built.success() {
        return Err(Unrun("windows-exe refused or failed (above)".into()));
    }
    let exe = format!("out/windows/{game}/gbaa-{game}.exe");
    let bin = build_recompiled(r, &out)?;
    let args = route_args(r);
    let last = r.frames();
    let sav = |who: &str| out.join(format!("{who}.sav")).display().to_string();

    let linux = runner(
        &out,
        "linux",
        &bin,
        &args,
        &[
            "--headless",
            "--regs",
            "--timing",
            "exact",
            "--save",
            &sav("linux"),
        ],
    )?;
    let t = Instant::now();
    let windows = run_as(
        &out,
        "windows",
        wine(r, &exe)?,
        &exe,
        &args,
        &[
            "--headless",
            "--regs",
            "--timing",
            "exact",
            "--save",
            &sav("windows"),
        ],
    )?;
    let wine_ms = t.elapsed().as_millis();
    compare(r, &out, &linux, &windows)?;
    let saved = match std::fs::read(sav("windows")) {
        Ok(b) => format!("the same {} KB .sav", b.len() / 1024),
        Err(_) => "no .sav written by either".into(),
    };
    let names: Vec<&str> = r.checkpoints.iter().map(|c| c.name.as_str()).collect();
    Ok(format!(
        "{} to frame {last}: gbaa-{game}.exe under Wine draws the Linux build's {last} frames \
         and the route's hash, same registers, {saved}; Wine run {wine_ms} ms",
        names.join(", ")
    ))
}

/// Where the two runs must agree, and the route's hashes.
fn compare(r: &Route, out: &Path, linux: &Run, windows: &Run) -> Result<(), Stop> {
    if let Some(f) = windows.first_difference(linux, &[]) {
        return Err(Fail(format!(
            "the Windows build's frame {f} differs from the Linux build's: {} and {}",
            out.join("linux-hashes.txt").display(),
            out.join("windows-hashes.txt").display()
        )));
    }
    for c in &r.checkpoints {
        if linux.hash(c.frame) != c.hash {
            return Err(Fail(format!(
                "checkpoint {} (frame {}): the Linux build drew {}, want {} ({})",
                c.name,
                c.frame,
                linux.hash(c.frame),
                c.hash,
                linux.png(c.frame).display()
            )));
        }
        if std::fs::read(linux.png(c.frame)).ok() != std::fs::read(windows.png(c.frame)).ok() {
            return Err(Fail(format!(
                "checkpoint {} (frame {}): the Windows PNG differs from the Linux one ({} and {})",
                c.name,
                c.frame,
                linux.png(c.frame).display(),
                windows.png(c.frame).display()
            )));
        }
    }
    let (ours, theirs) = (
        read(&out.join("linux.txt"))?,
        read(&out.join("windows.txt"))?,
    );
    if ours != theirs {
        return Err(Fail(format!(
            "at the last frame, the Windows build's registers or hash differ from Linux's:\n\
             linux:\n{ours}windows:\n{theirs}"
        )));
    }
    let sav = |who: &str| std::fs::read(out.join(format!("{who}.sav"))).ok();
    if sav("linux") != sav("windows") {
        return Err(Fail(format!(
            "the cartridge saves differ ({} and {}; a missing one wasn't written)",
            out.join("linux.sav").display(),
            out.join("windows.sav").display()
        )));
    }
    Ok(())
}

/// `wine EXE` in the container, capped at 3 GB as [`runner`] is, with
/// this checkout (read-write, for the outputs) and the route's ROM and
/// state (read-only) at the same paths as here, so every path means the
/// same under Wine (its drive Z: is the container's root).
fn wine(r: &Route, exe: &str) -> Result<Command, Stop> {
    let here = std::env::current_dir().map_err(|e| Unrun(format!("the current folder: {e}")))?;
    let mut cmd = Command::new("podman");
    cmd.args([
        "run",
        "--rm",
        "--security-opt",
        "label=disable",
        "--memory",
        "3g",
        "--memory-swap",
        "3g",
    ]);
    cmd.arg("-v").arg(format!("{0}:{0}", here.display()));
    for f in std::iter::once(&r.rom).chain(&r.state) {
        let dir = absolute(f, &here)
            .parent()
            .map(Path::to_path_buf)
            .ok_or(Bad(format!("{}: no folder", f.display())))?;
        if !dir.starts_with(&here) {
            cmd.arg("-v").arg(format!("{0}:{0}:ro", dir.display()));
        }
    }
    cmd.arg("-w").arg(&here).args([IMAGE, "wine", exe]);
    Ok(cmd)
}

fn absolute(f: &Path, here: &Path) -> PathBuf {
    if f.is_absolute() {
        f.to_path_buf()
    } else {
        here.join(f)
    }
}
