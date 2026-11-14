//! `gbaa-holes --nightly FILE --roms DIR [--out DIR] [--against DIR] [--only NAME]…
//! [--skip STEP]… [--step-minutes N]` (`just nightly`, unit
//! `native/nightly-corpus`): every game of the corpus file
//! (`holes/corpus.toml`) taken as far as it goes, and what changed since
//! the run before.
//!
//! Per game, in turn, each step its own child with its output in
//! `OUT/<name>/<step>.log`:
//!
//! - `holes`: the hole report (`gbaa-holes ROM --holes H --line …`, its
//!   report in `OUT/<name>/holes.txt`), whose ranking line holds the
//!   lift's hard holes by kind;
//! - `play`: the play build, `just recompile ROM out/nightly/play-<name>
//!   --name <name> --timing instant` then `just build-recompiled` on it;
//! - `readable`: the same with every function readable (`--readable` a
//!   list of `*`), into `out/nightly/readable-<name>`;
//! - `lift`: `just lift ROM out/nightly/lift-<name> --name <lift>` (the
//!   corpus's `lift`, the name its holes file is found by), which builds the
//!   lift and runs it headless (`just first-stop`): the frames it ran and
//!   where it stopped come from the lift's `metrics.toml`.
//!
//! A step that fails is recorded and the rest go on; a step running longer
//! than `--step-minutes` (default 120) is killed, with its process group,
//! and recorded as timed out. The generated crates are kept under
//! `out/nightly/` between nights (the next night rebuilds them); the dated
//! folder keeps the logs and the summaries.
//!
//! OUT defaults to `out/nightly/<date>` (`<date>-2`, `-3`… when that has a
//! summary already). It gets `summary.toml` ([`Night`]) and `summary.txt`,
//! and each game's own `OUT/<name>/summary.txt`: hard holes by kind, which
//! builds built, the frames run and the stop, and the changes against the
//! run before ([`changes`]): `--against DIR`, else the latest other run in
//! `out/nightly/` with a summary. Exits 1 when a game got worse (a hard
//! count rose, a build that built no longer does, fewer frames run): a
//! lifter change that took moonbeam's hard holes from 11 to 503 must not
//! land quietly. Exits 0 otherwise, whatever failed in both runs.

use std::fmt::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::corpus::{CorpusFile, Line};

/// The steps, in the order each game takes them.
pub const STEPS: [&str; 4] = ["holes", "play", "readable", "lift"];

/// One step's outcome.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    /// The child's exit code; None when it was killed (or never started).
    pub exit: Option<i32>,
    pub seconds: f64,
    /// Killed at the time limit.
    pub timed_out: bool,
    /// For a build step (play, readable, lift): the crate built. False
    /// for the hole report, which builds nothing.
    pub built: bool,
    /// What went wrong, short: the step that failed, the log's last line.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// The lift's headless run (`codegen::native::first_stop::Run`), as
/// much of it as the diff needs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunLine {
    pub frames_run: u64,
    pub frames_asked: u64,
    pub stop: String,
    pub what: String,
    /// The run in a few words (`Run::summary`).
    pub summary: String,
}

/// One game's night.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Game {
    pub name: String,
    /// The hole report's ranking line; None when it stopped first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holes: Option<Line>,
    /// Each step run, by name ([`STEPS`]); a skipped one is absent.
    pub step: std::collections::BTreeMap<String, Step>,
    /// The lift's run; None when the lift wasn't run or recorded none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunLine>,
    /// What changed against the run before, a line each ([`changes`]).
    pub changes: Vec<String>,
    /// Some change made it worse.
    pub worse: bool,
}

/// `summary.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Night {
    pub commit: String,
    pub date: String,
    /// The run compared with (its folder), empty for none.
    pub against: String,
    pub against_commit: String,
    pub game: Vec<Game>,
}

/// The hard holes by kind, in a few words: `3 (jumps 2, BIOS 1)`.
pub fn hard_text(l: &Line) -> String {
    let kinds = [
        ("jumps", l.jumps),
        ("no code", l.no_code),
        ("undefined", l.undefined),
        ("CPU state", l.cpu_state),
        ("BIOS", l.bios),
        ("summaries", l.summaries),
        ("r4-r11", l.takes),
        ("holes file", l.holes_file),
    ];
    let k: Vec<String> = kinds
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    if k.is_empty() {
        l.hard.to_string()
    } else {
        format!("{} ({})", l.hard, k.join(", "))
    }
}

fn built_text(s: Option<&Step>) -> String {
    match s {
        None => "skipped".into(),
        Some(s) if s.built => "built".into(),
        Some(s) if s.timed_out => "timed out".into(),
        Some(s) => format!("not built ({})", s.why),
    }
}

/// What changed from `was` to `now`, a line each, and whether any of it
/// is worse: a hard count up, a build lost, fewer frames run. A step
/// skipped on either side isn't compared.
pub fn changes(now: &Game, was: &Game) -> (Vec<String>, bool) {
    let (mut out, mut worse) = (Vec::new(), false);
    match (&now.holes, &was.holes) {
        (Some(n), Some(w)) => {
            if n.hard != w.hard {
                worse |= n.hard > w.hard;
                out.push(format!(
                    "hard holes {} -> {}{}",
                    hard_text(w),
                    hard_text(n),
                    if n.hard > w.hard { " WORSE" } else { "" }
                ));
            } else if hard_text(n) != hard_text(w) {
                out.push(format!(
                    "hard holes by kind {} -> {}",
                    hard_text(w),
                    hard_text(n)
                ));
            }
        }
        (None, Some(_)) if now.step.contains_key("holes") => {
            worse = true;
            out.push("hole report stopped (it ran before) WORSE".into());
        }
        (Some(n), None) => out.push(format!("hole report ran: hard {}", hard_text(n))),
        _ => {}
    }
    for step in ["play", "readable", "lift"] {
        let (Some(n), Some(w)) = (now.step.get(step), was.step.get(step)) else {
            continue;
        };
        if n.built != w.built {
            worse |= w.built;
            out.push(format!(
                "{step} build: {} -> {}{}",
                built_text(Some(w)),
                built_text(Some(n)),
                if w.built { " WORSE" } else { "" }
            ));
        }
    }
    if let (Some(n), Some(w)) = (&now.run, &was.run)
        && (n.frames_run, &n.stop, &n.what) != (w.frames_run, &w.stop, &w.what)
    {
        let fewer = n.frames_run < w.frames_run;
        worse |= fewer;
        out.push(format!(
            "run: {} -> {}{}",
            w.summary,
            n.summary,
            if fewer { " WORSE" } else { "" }
        ));
    }
    (out, worse)
}

/// One game's `summary.txt`.
pub fn game_text(g: &Game, against: &str) -> String {
    let mut s = String::new();
    writeln!(s, "{}", g.name).unwrap();
    let hard = match (&g.holes, g.step.get("holes")) {
        (Some(l), _) => hard_text(l),
        (None, None) => "skipped".into(),
        (None, Some(st)) => format!("hole report stopped ({})", st.why),
    };
    writeln!(s, "  hard holes: {hard}").unwrap();
    writeln!(
        s,
        "  play build: {}; readable build: {}; lift: {}",
        built_text(g.step.get("play")),
        built_text(g.step.get("readable")),
        built_text(g.step.get("lift"))
    )
    .unwrap();
    if let Some(r) = &g.run {
        writeln!(s, "  run: {}", r.summary).unwrap();
    }
    if against.is_empty() {
        writeln!(s, "  changes: nothing to compare with").unwrap();
    } else if g.changes.is_empty() {
        writeln!(s, "  changes against {against}: none").unwrap();
    } else {
        writeln!(s, "  changes against {against}:").unwrap();
        for c in &g.changes {
            writeln!(s, "    {c}").unwrap();
        }
    }
    s
}

/// The whole night's `summary.txt`.
pub fn night_text(n: &Night) -> String {
    let mut s = format!("nightly {} at {}", n.date, n.commit);
    if !n.against.is_empty() {
        write!(s, ", against {} at {}", n.against, n.against_commit).unwrap();
    }
    s.push('\n');
    for g in &n.game {
        s.push('\n');
        s.push_str(&game_text(g, &n.against));
    }
    let worse: Vec<&str> = n
        .game
        .iter()
        .filter(|g| g.worse)
        .map(|g| g.name.as_str())
        .collect();
    if !worse.is_empty() {
        write!(s, "\nWORSE: {}\n", worse.join(", ")).unwrap();
    }
    s
}

/// A run folder's sort key: `2026-11-14` before `2026-11-14-2` before
/// `2026-11-14-10`.
fn key(name: &str) -> Option<(String, u32)> {
    let date = name.get(..10)?;
    let ok = date.len() == 10
        && date.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !ok {
        return None;
    }
    let n = match &name[10..] {
        "" => 1,
        r => r.strip_prefix('-')?.parse().ok()?,
    };
    Some((date.to_string(), n))
}

/// The runs under `root` with a summary, oldest first.
fn runs(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<((String, u32), PathBuf)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let k = key(&e.file_name().to_string_lossy())?;
            e.path()
                .join("summary.toml")
                .exists()
                .then(|| (k, e.path()))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

/// Today's folder: `root/<date>`, or the next `-N` free of a summary.
fn default_out(root: &Path, date: &str) -> PathBuf {
    (1..)
        .map(|n| {
            root.join(if n == 1 {
                date.to_string()
            } else {
                format!("{date}-{n}")
            })
        })
        .find(|p| !p.join("summary.toml").exists())
        .expect("a free folder")
}

fn read_night(dir: &Path) -> Result<Night, String> {
    let p = dir.join("summary.toml");
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    toml::from_str(&t).map_err(|e| format!("{}: {e}", p.display()))
}

/// The last line of `log` that says something, short: not blank, and not
/// just's own `error: recipe … failed`.
fn last_line(log: &Path) -> String {
    let t = std::fs::read_to_string(log).unwrap_or_default();
    let l = t
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.starts_with("error: recipe `"))
        .unwrap_or("");
    let l = l.trim();
    match l.char_indices().nth(160) {
        Some((i, _)) => format!("{}…", &l[..i]),
        None => l.to_string(),
    }
}

/// Runs `c` with its output appended to `log`, killing its process group
/// after `limit`.
fn run(mut c: Command, log: &Path, limit: Duration) -> Step {
    let t = Instant::now();
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .and_then(|f| f.try_clone().map(|g| (f, g)));
    let (out, err) = match f {
        Ok(f) => f,
        Err(e) => {
            return Step {
                why: format!("{}: {e}", log.display()),
                ..Step::default()
            };
        }
    };
    c.stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .process_group(0);
    wait(c, log, limit, t)
}

fn just(args: &[&std::ffi::OsStr]) -> Command {
    let mut c = Command::new("just");
    c.args(args);
    c
}

/// A play or readable build: recompile into `dir`, then build it.
fn build(
    rom: &Path,
    dir: &Path,
    name: &str,
    extra: &[&std::ffi::OsStr],
    log: &Path,
    limit: Duration,
) -> Step {
    let mut a: Vec<&std::ffi::OsStr> = vec![
        "recompile".as_ref(),
        rom.as_os_str(),
        dir.as_os_str(),
        "--name".as_ref(),
        name.as_ref(),
        "--timing".as_ref(),
        "instant".as_ref(),
    ];
    a.extend_from_slice(extra);
    let mut s = run(just(&a), log, limit);
    if s.exit != Some(0) {
        s.why = format!("recompile: {}", s.why);
        return s;
    }
    let left = limit.saturating_sub(Duration::from_secs_f64(s.seconds));
    let b = run(
        just(&["build-recompiled".as_ref(), dir.as_os_str()]),
        log,
        left,
    );
    Step {
        seconds: s.seconds + b.seconds,
        built: b.exit == Some(0),
        why: if b.exit == Some(0) {
            String::new()
        } else {
            format!("build: {}", b.why)
        },
        ..b
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: gbaa-holes --nightly FILE --roms DIR [--out DIR] [--against DIR] [--only NAME]... [--skip holes|play|readable|lift]... [--step-minutes N]"
    );
    ExitCode::from(2)
}

/// `gbaa-holes --nightly …`; `args` are the ones after `--nightly`.
pub fn main(args: &[String]) -> ExitCode {
    let [file, rest @ ..] = args else {
        return usage();
    };
    let (mut roms, mut out, mut against) = (None::<PathBuf>, None::<PathBuf>, None::<PathBuf>);
    let (mut only, mut skip, mut minutes) = (Vec::new(), Vec::new(), 120u64);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let Some(v) = it.next() else { return usage() };
        match a.as_str() {
            "--roms" => roms = Some(v.into()),
            "--out" => out = Some(v.into()),
            "--against" => against = Some(v.into()),
            "--only" => only.push(v.clone()),
            "--skip" if STEPS.contains(&v.as_str()) => skip.push(v.clone()),
            "--step-minutes" => match v.parse() {
                Ok(m) => minutes = m,
                Err(_) => return usage(),
            },
            _ => return usage(),
        }
    }
    let Some(roms) = roms else { return usage() };
    let limit = Duration::from_secs(minutes * 60);
    let corpus: CorpusFile = match std::fs::read_to_string(file)
        .map_err(|e| e.to_string())
        .and_then(|t| toml::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gbaa-holes: {file}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut games = corpus.game;
    if let Some(n) = only.iter().find(|n| !games.iter().any(|g| &g.name == *n)) {
        eprintln!("gbaa-holes: {file}: no game {n}");
        return ExitCode::from(2);
    }
    if !only.is_empty() {
        games.retain(|g| only.contains(&g.name));
    }
    let root = PathBuf::from("out/nightly");
    let date = analysis::numbers_doc::date(std::time::SystemTime::now());
    let out = out.unwrap_or_else(|| default_out(&root, &date));
    let against = against.or_else(|| {
        runs(&root)
            .into_iter()
            .rev()
            .find(|p| p.canonicalize().ok() != out.canonicalize().ok())
    });
    let was = match against.as_deref().map(read_night).transpose() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("gbaa-holes: --against: {e}");
            return ExitCode::from(2);
        }
    };
    let me = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gbaa-holes: {e}");
            return ExitCode::from(2);
        }
    };
    let readable_list = out.join("readable-all.list");
    if let Err(e) = std::fs::create_dir_all(&out)
        .and_then(|()| std::fs::write(&readable_list, "# every function readable\n*\n"))
    {
        eprintln!("gbaa-holes: {}: {e}", out.display());
        return ExitCode::from(2);
    }
    let mut night = Night {
        commit: analysis::numbers_doc::commit(),
        date,
        against: against
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        against_commit: was.as_ref().map(|w| w.commit.clone()).unwrap_or_default(),
        game: Vec::new(),
    };
    eprintln!(
        "gbaa-holes: nightly, {} games into {}{}",
        games.len(),
        out.display(),
        match &against {
            Some(a) => format!(", against {}", a.display()),
            None => String::new(),
        }
    );
    for g in &games {
        let dir = out.join(&g.name);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("gbaa-holes: {}: {e}", dir.display());
            return ExitCode::from(2);
        }
        let rom = roms.join(&g.rom);
        let lift_name = g.lift.clone().unwrap_or_else(|| g.name.clone());
        let mut game = Game {
            name: g.name.clone(),
            ..Game::default()
        };
        let todo = |s: &str| !skip.iter().any(|k| k == s);
        if todo("holes") {
            let line = dir.join("holes.toml");
            let _ = std::fs::remove_file(&line);
            let log = dir.join("holes.log");
            let _ = std::fs::remove_file(&log);
            let mut c = Command::new(&me);
            c.arg(&rom).arg("--line").arg(&line);
            if let Some(h) = crate::corpus::holes_file(&lift_name) {
                c.arg("--holes").arg(h);
            }
            // The report itself to holes.txt, the child's progress to the log.
            let txt = std::fs::File::create(dir.join("holes.txt"));
            let mut st = match txt {
                Ok(f) => {
                    c.stdout(Stdio::from(f));
                    run_keep_stdout(c, &log, limit)
                }
                Err(e) => Step {
                    why: e.to_string(),
                    ..Step::default()
                },
            };
            game.holes = std::fs::read_to_string(&line)
                .ok()
                .and_then(|t| toml::from_str(&t).ok());
            if game.holes.is_none() && st.why.is_empty() {
                st.why = "no ranking line".into();
            }
            game.step.insert("holes".into(), st);
        }
        for (step, extra) in [
            ("play", None),
            ("readable", Some(readable_list.as_os_str())),
        ] {
            if !todo(step) {
                continue;
            }
            let log = dir.join(format!("{step}.log"));
            let _ = std::fs::remove_file(&log);
            let crate_dir = root.join(format!("{step}-{}", g.name));
            let extra: Vec<&std::ffi::OsStr> = match extra {
                Some(l) => vec!["--readable".as_ref(), l],
                None => vec![],
            };
            let st = build(&rom, &crate_dir, &g.name, &extra, &log, limit);
            game.step.insert(step.into(), st);
        }
        if todo("lift") {
            let log = dir.join("lift.log");
            let _ = std::fs::remove_file(&log);
            let lift_dir = root.join(format!("lift-{}", g.name));
            // An old run's record mustn't pass for tonight's.
            let _ = std::fs::remove_file(lift_dir.join("metrics.toml"));
            let mut st = run(
                just(&[
                    "lift".as_ref(),
                    rom.as_os_str(),
                    lift_dir.as_os_str(),
                    "--name".as_ref(),
                    lift_name.as_ref(),
                ]),
                &log,
                limit,
            );
            let r = std::fs::read_to_string(lift_dir.join("metrics.toml"))
                .ok()
                .and_then(|t| codegen::native::first_stop::Run::read(&t).ok().flatten());
            if let Some(r) = r {
                st.built = r.stop != codegen::native::first_stop::kind::NOT_BUILT;
                game.run = Some(RunLine {
                    frames_run: r.frames_run,
                    frames_asked: r.frames_asked,
                    summary: r.summary(),
                    stop: r.stop,
                    what: r.what,
                });
            }
            if !st.built && st.why.is_empty() {
                st.why = "no run recorded".into();
            }
            game.step.insert("lift".into(), st);
        }
        if let Some(w) = was
            .as_ref()
            .and_then(|w| w.game.iter().find(|x| x.name == g.name))
        {
            (game.changes, game.worse) = changes(&game, w);
        }
        let text = game_text(&game, &night.against);
        if let Err(e) = std::fs::write(dir.join("summary.txt"), &text) {
            eprintln!("gbaa-holes: {}: {e}", dir.display());
        }
        eprint!("{text}");
        night.game.push(game);
        // Written after every game, so a run cut off keeps what it did.
        if let Err(e) = write_night(&out, &night) {
            eprintln!("gbaa-holes: {e}");
            return ExitCode::from(2);
        }
    }
    print!("{}", night_text(&night));
    println!("summaries in {}", out.display());
    if night.game.iter().any(|g| g.worse) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// [`run`] for a command whose stdout is already set: only its stderr goes
/// to `log`.
fn run_keep_stdout(mut c: Command, log: &Path, limit: Duration) -> Step {
    let t = Instant::now();
    let err = match std::fs::File::create(log) {
        Ok(f) => f,
        Err(e) => {
            return Step {
                why: format!("{}: {e}", log.display()),
                ..Step::default()
            };
        }
    };
    c.stdin(Stdio::null())
        .stderr(Stdio::from(err))
        .process_group(0);
    wait(c, log, limit, t)
}

/// Spawns `c` (in its own process group) and waits for it, killing the
/// group once `limit` has passed since `t`.
fn wait(mut c: Command, log: &Path, limit: Duration, t: Instant) -> Step {
    let mut step = Step::default();
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            step.why = format!("{:?}: {e}", c.get_program());
            return step;
        }
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if t.elapsed() > limit => {
                let pg = format!("-{}", child.id());
                let _ = Command::new("kill").args(["-TERM", "--", &pg]).status();
                std::thread::sleep(Duration::from_secs(10));
                let _ = Command::new("kill").args(["-KILL", "--", &pg]).status();
                let _ = child.wait();
                step.timed_out = true;
                step.why = format!("timed out after {} min", limit.as_secs() / 60);
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(e) => {
                step.why = e.to_string();
                break None;
            }
        }
    };
    step.seconds = t.elapsed().as_secs_f64();
    step.exit = status.and_then(|s| s.code());
    if step.exit.is_some_and(|x| x != 0) {
        step.why = last_line(log);
    } else if status.is_some() && step.exit.is_none() {
        step.why = "killed by a signal".into();
    }
    step
}

fn write_night(out: &Path, n: &Night) -> Result<(), String> {
    let p = out.join("summary.toml");
    let t = toml::to_string(n).map_err(|e| e.to_string())?;
    std::fs::write(&p, t).map_err(|e| format!("{}: {e}", p.display()))?;
    let p = out.join("summary.txt");
    std::fs::write(&p, night_text(n)).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(hard: usize, built: bool, frames: u64) -> Game {
        let mut g = Game {
            name: "g".into(),
            holes: Some(Line {
                hard,
                jumps: hard,
                ..Line::default()
            }),
            run: Some(RunLine {
                frames_run: frames,
                frames_asked: 600,
                stop: "panic".into(),
                summary: format!("{frames} of 600 frames, panic"),
                ..RunLine::default()
            }),
            ..Game::default()
        };
        for s in ["holes", "play", "readable", "lift"] {
            g.step.insert(
                s.into(),
                Step {
                    built,
                    exit: Some(if built { 0 } else { 1 }),
                    why: if built {
                        String::new()
                    } else {
                        "build: error".into()
                    },
                    ..Step::default()
                },
            );
        }
        g
    }

    #[test]
    fn a_hard_count_rising_is_worse() {
        let (c, worse) = changes(&game(503, true, 10), &game(11, true, 10));
        assert!(worse);
        assert_eq!(c, ["hard holes 11 (jumps 11) -> 503 (jumps 503) WORSE"]);
        let (c, worse) = changes(&game(3, true, 10), &game(11, true, 10));
        assert!(!worse, "{c:?}");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn a_lost_build_and_fewer_frames_are_worse() {
        let (c, worse) = changes(&game(0, false, 10), &game(0, true, 10));
        assert!(worse);
        assert_eq!(c.len(), 3, "{c:?}");
        assert!(c[0].starts_with("play build: built -> not built"), "{c:?}");
        let (c, worse) = changes(&game(0, true, 5), &game(0, true, 10));
        assert!(worse);
        assert_eq!(
            c,
            ["run: 10 of 600 frames, panic -> 5 of 600 frames, panic WORSE"]
        );
        let (c, worse) = changes(&game(0, true, 600), &game(0, false, 10));
        assert!(!worse, "{c:?}");
        let (c, worse) = changes(&game(0, true, 10), &game(0, true, 10));
        assert!(c.is_empty() && !worse);
    }

    #[test]
    fn a_skipped_step_isnt_compared() {
        let mut now = game(0, false, 10);
        now.step.remove("play");
        now.step.remove("readable");
        now.step.remove("lift");
        now.run = None;
        let (c, worse) = changes(&now, &game(0, true, 600));
        assert!(c.is_empty() && !worse, "{c:?}");
    }

    #[test]
    fn the_summary_says_it_short() {
        let mut g = game(2, true, 600);
        g.holes.as_mut().unwrap().jumps = 1;
        g.holes.as_mut().unwrap().bios = 1;
        g.changes = vec!["hard holes 3 (jumps 3) -> 2 (jumps 1, BIOS 1)".into()];
        let t = game_text(&g, "out/nightly/2026-11-13");
        assert_eq!(
            t,
            "g\n  hard holes: 2 (jumps 1, BIOS 1)\n  play build: built; readable build: built; lift: built\n  run: 600 of 600 frames, panic\n  changes against out/nightly/2026-11-13:\n    hard holes 3 (jumps 3) -> 2 (jumps 1, BIOS 1)\n"
        );
        let n = Night {
            game: vec![g],
            ..Night::default()
        };
        let s = toml::to_string(&n).unwrap();
        assert_eq!(toml::from_str::<Night>(&s).unwrap(), n);
    }

    #[test]
    fn runs_sort_by_date_then_number() {
        assert_eq!(key("2026-11-14"), Some(("2026-11-14".into(), 1)));
        assert_eq!(key("2026-11-14-10"), Some(("2026-11-14".into(), 10)));
        assert_eq!(key("play-metroid"), None);
        assert_eq!(key("2026-11-14x"), None);
        let root = std::env::temp_dir().join(format!("gbaa-nightly-{}", std::process::id()));
        for d in [
            "2026-11-14-10",
            "2026-11-14-2",
            "2026-11-13",
            "lift-x",
            "2026-11-14",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            std::fs::write(root.join(d).join("summary.toml"), "").unwrap();
        }
        let names: Vec<String> = runs(&root)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["2026-11-13", "2026-11-14", "2026-11-14-2", "2026-11-14-10"]
        );
        assert_eq!(default_out(&root, "2026-11-14"), root.join("2026-11-14-3"));
        assert_eq!(default_out(&root, "2026-11-15"), root.join("2026-11-15"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
