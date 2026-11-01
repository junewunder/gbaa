//! `route-check --same-states A B [--every N] ROUTE.toml…`: run two builds
//! of one game (A and B: each a binary, optionally followed by its own
//! `gbaa-desktop` flags, space-separated) along each route, saving our
//! exact state at each checkpoint (and with `--every`, every Nth frame
//! too), and fail unless every pair of states is byte-identical. Two builds
//! with the same tick points must agree on the whole machine, not only the
//! frame: flag liveness on and off (`docs/structured-codegen-design.md`,
//! step 2), or a recompiled build and its block-timed interpreter
//! (`--interp-blocks`, validation item 1). Each build runs each route once,
//! writing a state after each frame compared (`--dump-states`), and a
//! modded build's `mod:` line (its resumes inside modded functions) is
//! printed. A pair that differs is kept in `out/same-states/<route>/`;
//! one that matches is deleted.

use crate::route::{Route, Vars};
use crate::{Stop, Stop::*, file, tail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Compare A's and B's states at every checkpoint of `routes`, and every
/// `every`th frame up to the last (0: none). Ok: what passed.
pub fn run(a: &str, b: &str, every: u64, routes: &[String], vars: &Vars) -> Result<String, Stop> {
    let mut said = Vec::new();
    let mut failed = Vec::new();
    for p in routes {
        let r = Route::read(Path::new(p), vars).map_err(Bad)?;
        let name = Path::new(p)
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned());
        let out = PathBuf::from("out/same-states").join(&name);
        if out.exists() {
            std::fs::remove_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
        }
        let points = points(&r, every);
        let (da, db) = (out.join("a"), out.join("b"));
        states(a, &r, &points, &da, true)?;
        states(b, &r, &points, &db, true)?;
        // A modded build's resume count (`gbaa-desktop`'s `mod:` line).
        for (side, d) in [("a", &da), ("b", &db)] {
            let log = std::fs::read_to_string(d.join("run.log")).unwrap_or_default();
            for l in log.lines().filter(|l| l.starts_with("mod: ")) {
                println!("{name} {side}: {l}");
            }
        }
        for (&frame, at) in &points {
            let file = format!("frame-{frame}.state");
            let read = |d: &Path| {
                std::fs::read(d.join(&file))
                    .map_err(|e| Unrun(format!("{}: {e}", d.join(&file).display())))
            };
            let (sa, sb) = (read(&da)?, read(&db)?);
            match differ(&sa, &sb) {
                None => {
                    println!("{name} {at}: the same ({} bytes)", sa.len());
                    for d in [&da, &db] {
                        let _ = std::fs::remove_file(d.join(&file));
                    }
                }
                Some(d) => {
                    println!("{name} {at}: {d}");
                    failed.push(format!("{name} {at} ({d})"));
                }
            }
        }
        said.push(format!("{name} ({} frames)", points.len()));
    }
    if failed.is_empty() {
        Ok(format!("the same states at {}", said.join(", ")))
    } else {
        Err(Fail(format!(
            "the states differ at {} (states in out/same-states/)",
            failed.join("; ")
        )))
    }
}

/// The frames to compare at, each with its name: the checkpoints, and
/// every `every`th frame up to the last as `frame-N`.
pub fn points(r: &Route, every: u64) -> BTreeMap<u64, String> {
    let mut p = BTreeMap::new();
    if every > 0 {
        for f in (every..=r.frames()).step_by(every as usize) {
            p.insert(f, format!("frame-{f}"));
        }
    }
    for c in &r.checkpoints {
        p.insert(c.frame, c.name.clone());
    }
    p
}

/// Run `build` (a binary and its flags) along `r` to the last of
/// `points`, writing each frame's PNG into `dir` as `frame-N.png` and,
/// with `with_states`, its state beside it as `frame-N.state`.
pub fn states(
    build: &str,
    r: &Route,
    points: &BTreeMap<u64, String>,
    dir: &Path,
    with_states: bool,
) -> Result<(), Stop> {
    std::fs::create_dir_all(dir).map_err(|e| Unrun(format!("{}: {e}", dir.display())))?;
    let mut words = build.split_whitespace();
    let bin = words.next().ok_or(Bad("an empty build".into()))?;
    let last = points.keys().last().copied().unwrap_or(1);
    let at: Vec<String> = points.keys().map(u64::to_string).collect();
    let mut args: Vec<String> = vec![
        r.rom.display().to_string(),
        "--headless".into(),
        "--timing".into(),
        crate::timing().into(),
        "--frames".into(),
        last.to_string(),
        "--dump-at".into(),
        at.join(","),
        "--dump-dir".into(),
        dir.display().to_string(),
    ];
    if with_states {
        args.push("--dump-states".into());
    }
    args.extend(words.map(String::from));
    if !r.input.is_empty() {
        args.extend(["--input".into(), r.input.clone()]);
    }
    if let Some(s) = &r.state {
        args.extend(["--state".into(), s.display().to_string()]);
    }
    let log = dir.join("run.log");
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
        .arg(bin)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(file(&log)?)
        .status()
        .map_err(|e| Unrun(format!("{bin}: {e}")))?;
    if !status.success() {
        return Err(Fail(format!(
            "{bin} exited with an error ({}: {})",
            log.display(),
            tail(&log)
        )));
    }
    Ok(())
}

/// How `a` and `b` differ, if they do: the bytes, and the first few
/// ranges of differing offsets.
fn differ(a: &[u8], b: &[u8]) -> Option<String> {
    if a == b {
        return None;
    }
    if a.len() != b.len() {
        return Some(format!("{} bytes against {}", a.len(), b.len()));
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for i in (0..a.len()).filter(|&i| a[i] != b[i]) {
        match ranges.last_mut() {
            Some((_, end)) if i <= *end + 8 => *end = i,
            _ => ranges.push((i, i)),
        }
    }
    let n = a.iter().zip(b).filter(|(x, y)| x != y).count();
    let shown: Vec<String> = ranges
        .iter()
        .take(6)
        .map(|&(s, e)| format!("{s:#x}..={e:#x}"))
        .collect();
    let more = if ranges.len() > 6 { ", …" } else { "" };
    Some(format!("{n} bytes differ, at {}{more}", shown.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::differ;

    #[test]
    fn names_the_differing_ranges() {
        assert_eq!(differ(&[1, 2, 3], &[1, 2, 3]), None);
        let a = vec![0u8; 64];
        let mut b = a.clone();
        b[3] = 1;
        b[5] = 1;
        b[40] = 1;
        assert_eq!(
            differ(&a, &b).unwrap(),
            "3 bytes differ, at 0x3..=0x5, 0x28..=0x28"
        );
    }
}
