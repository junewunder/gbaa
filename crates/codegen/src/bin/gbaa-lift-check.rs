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
//!    (`--planted`). Each is lifted once more at the play layout
//!    (`--layout packed`, unit `native/lift-packed-layout`), which must
//!    draw the same frames and record the same RAM as the guest layout
//!    (`--packed`).
//! 3. **The planted hard hole.** A program with `planted_hole` is lifted
//!    with that stand-in dropped, from a fresh OUT: the lift and the
//!    crate's build must both fail, the build naming the hole.
//!
//! `--only NAME` (repeatable) runs those programs' steps.
//!
//! `--metrics-only [--metric REGEX] [--lower-bars]` (`just
//! lift-metrics`) does step 1 alone, every program's lift at once, and prints a table
//! of each metric (rows; `--metric` keeps the names REGEX matches) by
//! program (columns), its bar beside it: from each lift's
//! `metrics.toml` and the file's bars, not from the report's text. No
//! crate is built and no route or planted control runs. `--lower-bars`
//! rewrites FILE, keeping its comments and order, with every bar shown
//! that is over its metric lowered to the metric
//! ([`codegen::native::metrics::lower_bars`]); bars are never raised.

use codegen::native::lift_check::{Program, programs};
use codegen::native::metrics::{self, Metrics};
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

    /// `--metrics-only`: every program lifted with `--bars`, then the
    /// table, and with `--lower-bars` the file rewritten.
    fn metrics_only(&self, ps: &[Program], o: &MetricsOnly) -> ExitCode {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("gbaa-lift-check: {}: {e}", self.file);
                return ExitCode::FAILURE;
            }
        };
        // The lifts at once: each is one thread and under 100 MB, and
        // seconds alone.
        let lifts: Vec<Result<Column, String>> = std::thread::scope(|s| {
            let hs: Vec<_> = ps
                .iter()
                .map(|p| s.spawn(|| self.measure(p, &text)))
                .collect();
            hs.into_iter()
                .map(|h| h.join().unwrap_or(Err("the lift's thread panicked".into())))
                .collect()
        });
        let mut cols = Vec::new();
        for (p, l) in ps.iter().zip(lifts) {
            match l {
                Ok(c) => cols.push(c),
                Err(e) => {
                    eprintln!("gbaa-lift-check: {}: {e}", p.name);
                    return ExitCode::FAILURE;
                }
            }
        }
        let keep = |n: &str| o.metric.as_ref().is_none_or(|r| r.is_match(n));
        let (table, over, lowerable) = metrics_table(&cols, &keep);
        print!("{table}");
        println!(
            "lift-metrics: {} over their bars, {} under (`--lower-bars` lowers them)",
            over,
            lowerable.len()
        );
        if o.lower && !lowerable.is_empty() {
            match metrics::lower_bars(&text, &lowerable).and_then(|(t, c)| {
                std::fs::write(&self.file, t)
                    .map(|_| c)
                    .map_err(|e| e.to_string())
            }) {
                Ok(changed) => {
                    for c in &changed {
                        println!("lift-metrics: lowered {c}");
                    }
                    println!(
                        "lift-metrics: {} bars lowered in {}",
                        changed.len(),
                        self.file
                    );
                }
                Err(e) => {
                    eprintln!("gbaa-lift-check: {}: {e}", self.file);
                    return ExitCode::FAILURE;
                }
            }
        }
        let failed: Vec<&Column> = cols.iter().filter(|c| c.failed.is_some()).collect();
        for c in &failed {
            eprintln!(
                "gbaa-lift-check: {}: {}",
                c.name,
                c.failed.as_deref().unwrap_or("")
            );
        }
        if !failed.is_empty() {
            return ExitCode::FAILURE;
        }
        if over == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }

    /// One program lifted with its bars: its column of the table, from
    /// the lift's `metrics.toml`.
    fn measure(&self, p: &Program, text: &str) -> Result<Column, String> {
        let out = format!("out/lift-{}", p.name);
        let file = Path::new(&out).join("metrics.toml");
        // So a lift that stops early can't leave an old run's numbers.
        let _ = std::fs::remove_file(&file);
        let mut c = lift(&self.rom(p), &p.name, &out);
        c.args(["--bars", &self.file]).args(&p.lift_flags);
        let log = format!("out/lift-{}.txt", p.name);
        let (ok, out) = run(&mut c, Path::new(&log))?;
        // A lift fails on more than a metric over its bar (a metric with no
        // bar, a bar naming no metric), and may still have written its
        // metrics: keep the log's tail so the table can't hide it.
        let failed = (!ok).then(|| {
            let tail: Vec<&str> = out.lines().rev().take(8).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            format!("its lift failed ({log}):\n{}", tail.join("\n"))
        });
        let m = std::fs::read_to_string(&file)
            .map_err(|e| format!("{}: {e} (the lift's log: {log})", file.display()))
            .and_then(|t| Metrics::from_toml(&t))?;
        Ok(Column {
            name: p.name.clone(),
            metrics: m,
            bars: metrics::bars(text, &p.name)?,
            failed,
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
            // A crate name: `--plant-slot=0x…` is `plant-slot-0x…`.
            let tag = flag
                .trim_start_matches('-')
                .replace(|c: char| !c.is_ascii_alphanumeric(), "-");
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
        // The play layout: the same program with its globals in the
        // lifter's order must draw the guest layout's frames.
        let packed = format!("out/lift-{n}-packed");
        let mut c = lift(&rom, n, &packed);
        c.args(["--name", &format!("{n}-packed"), "--layout", "packed"])
            .args(&p.lift_flags);
        must(
            &mut c,
            &log(&format!("lift-{n}-packed.txt")),
            "the packed layout's lift",
        )?;
        must(
            &mut just(&["build-recompiled", &packed]),
            &log(&format!("lift-{n}-packed.build.txt")),
            "the packed layout's build",
        )?;
        route
            .arg("--packed")
            .arg(format!("target/recompiled/release/gbaa-native-{n}-packed"));
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

/// `--metrics-only`'s options.
struct MetricsOnly {
    metric: Option<regex::Regex>,
    lower: bool,
}

fn main() -> ExitCode {
    analysis::cap::require_cap("gbaa-lift-check");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gbaa-lift-check FILE --data DIR [--only NAME]... [--metrics-only [--metric REGEX] [--lower-bars]]"
        );
        ExitCode::FAILURE
    };
    let [file, rest @ ..] = &args[..] else {
        return usage();
    };
    let (mut data, mut only, mut metrics_only) = (None, Vec::new(), false);
    let (mut metric, mut lower) = (None, false);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--metrics-only" => metrics_only = true,
            "--lower-bars" => lower = true,
            k => match (k, it.next()) {
                ("--data", Some(d)) => data = Some(PathBuf::from(d)),
                ("--only", Some(n)) => only.push(n.clone()),
                ("--metric", Some(r)) => match regex::Regex::new(r) {
                    Ok(r) => metric = Some(r),
                    Err(e) => {
                        eprintln!("gbaa-lift-check: --metric: {e}");
                        return ExitCode::FAILURE;
                    }
                },
                _ => return usage(),
            },
        }
    }
    let Some(data) = data else { return usage() };
    if (metric.is_some() || lower) && !metrics_only {
        return usage();
    }
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
    if let Some(n) = only.iter().find(|n| !ps.iter().any(|p| &p.name == *n)) {
        eprintln!("gbaa-lift-check: {file}: no program {n}");
        return ExitCode::FAILURE;
    }
    if !only.is_empty() {
        ps.retain(|p| only.contains(&p.name));
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
    if metrics_only {
        return r.metrics_only(&ps, &MetricsOnly { metric, lower });
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

/// A program's column of the metrics table.
struct Column {
    name: String,
    metrics: Metrics,
    bars: std::collections::BTreeMap<String, u64>,
    /// Why its lift exited non-zero, if it did.
    failed: Option<String>,
}

/// The table, a row per metric `keep` keeps (in the first program's
/// order, then any others'), a column per program: `v` at its bar, `v<b`
/// under, `v>b!` over, `v` alone with no bar. With how many are over, and
/// the ones under as (program, metric, value) for `lower_bars`.
fn metrics_table(
    cols: &[Column],
    keep: &dyn Fn(&str) -> bool,
) -> (String, usize, Vec<(String, String, u64)>) {
    let mut names: Vec<&str> = Vec::new();
    for c in cols {
        for (n, _) in &c.metrics.0 {
            if keep(n) && !names.contains(&n.as_str()) {
                names.push(n);
            }
        }
    }
    let (mut over, mut lower) = (0, Vec::new());
    let mut rows = vec![
        std::iter::once("metric".to_string())
            .chain(cols.iter().map(|c| c.name.clone()))
            .collect::<Vec<_>>(),
    ];
    for n in &names {
        let mut row = vec![n.to_string()];
        for c in cols {
            row.push(match (c.metrics.get(n), c.bars.get(*n)) {
                (None, _) => "-".into(),
                (Some(v), None) => v.to_string(),
                (Some(v), Some(&b)) if v > b => {
                    over += 1;
                    format!("{v}>{b}!")
                }
                (Some(v), Some(&b)) if v < b => {
                    lower.push((c.name.clone(), n.to_string(), v));
                    format!("{v}<{b}")
                }
                (Some(v), Some(_)) => v.to_string(),
            });
        }
        rows.push(row);
    }
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|i| rows.iter().map(|r| r[i].len()).max().unwrap_or(0))
        .collect();
    let mut s = String::new();
    for r in &rows {
        let cells: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if i == 0 {
                    format!("{v:<w$}", w = widths[i])
                } else {
                    format!("{v:>w$}", w = widths[i])
                }
            })
            .collect();
        s.push_str(cells.join("  ").trim_end());
        s.push('\n');
    }
    s.push_str("(v: at its bar; v<b: under bar b; v>b!: over it; no bar: v alone)\n");
    (s, over, lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_marks_each_bar() {
        let col = |name: &str, ms: &[(&str, u64)], bs: &[(&str, u64)]| Column {
            name: name.into(),
            metrics: Metrics(ms.iter().map(|(n, v)| (n.to_string(), *v)).collect()),
            bars: bs.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            failed: None,
        };
        let cols = [
            col(
                "a",
                &[("lines", 10), ("blocks", 5), ("ram_globals", 2)],
                &[("lines", 12), ("blocks", 5)],
            ),
            col(
                "b",
                &[("lines", 20), ("blocks", 6)],
                &[("lines", 20), ("blocks", 4)],
            ),
        ];
        let (t, over, lower) = metrics_table(&cols, &|_| true);
        assert_eq!(over, 1);
        assert_eq!(lower, vec![("a".into(), "lines".into(), 10)]);
        let rows: Vec<&str> = t.lines().collect();
        assert!(
            rows[1].starts_with("lines") && rows[1].contains("10<12"),
            "{t}"
        );
        assert!(rows[2].contains("6>4!"), "{t}");
        assert!(
            rows[3].starts_with("ram_globals") && rows[3].ends_with("-"),
            "{t}"
        );
        let re = regex::Regex::new("^bl").unwrap();
        let (t, over, lower) = metrics_table(&cols, &|n| re.is_match(n));
        assert_eq!((t.lines().count(), over, lower.len()), (3, 1, 0), "{t}");
    }
}
