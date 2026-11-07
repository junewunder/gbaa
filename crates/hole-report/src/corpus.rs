//! `gbaa-holes --corpus FILE --roms DIR [--out DIR] [--against DIR]
//! [--only NAME]… [--cover] [--write-doc FILE] [FLAGS…]` (`just holes-all`): the hole report
//! over every game of FILE (`holes/corpus.toml`), one table at the end.
//!
//! Each game is its own `gbaa-holes ROM FLAGS… --line OUT/<name>.toml`
//! child, one at a time (so one memory cap holds the whole run), its
//! report in `OUT/<name>.txt` and its timings in `OUT/<name>.log`. With
//! `--cover` each game with a `cover` file gets `--cover FILE` too (10 to
//! 30 minutes a game). The child writes its ranking line's numbers
//! ([`Line`], from the report's own counts) to `--line`; the table is
//! printed from those, and saved, one `[[game]]` each, in
//! `OUT/summary.toml`. `--against DIR2` reads DIR2's `summary.toml` and
//! puts each change beside its number, `4 (-3)`.
//!
//! OUT defaults to `out/holes/<git short hash>` (`-dirty` after it when
//! tracked files have changed). FLAGS (`--frames N`, `--no-summaries`,
//! `--press …`, …) go to every child as they are.
//!
//! `--write-doc FILE` also writes the table as a generated numbers file
//! ([`analysis::numbers_doc`]; `docs/research/native-holes.numbers.md`).
//! `--from DIR` runs nothing: it prints (and with `--write-doc` writes)
//! the table of DIR's `summary.toml`, so a run already made at the
//! commit wanted needn't be made again (`--roms` isn't needed then). A
//! summary keeps the commit and date it was made at; one from before it
//! did takes DIR's name as its commit and the file's date.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use analysis::holes::Report;
use serde::{Deserialize, Serialize};

/// The ranking line's numbers ([`Report::line`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub hard: usize,
    /// Indirect jumps Recovery leaves unresolved: ROM, then the overlays.
    pub jumps: usize,
    pub overlay_jumps: usize,
    pub return_arithmetic: usize,
    pub ram_changed: usize,
    pub cpu_state: usize,
    pub bios: usize,
    pub non_entries: usize,
    pub summaries: usize,
    /// Bytes reached but not discovered, ROM and RAM.
    pub undiscovered: u32,
    pub undiscovered_rom: u32,
    pub undiscovered_ram: u32,
    pub reached: u32,
    pub busy_loops: usize,
    pub flash: usize,
}

impl Line {
    pub fn of(r: &Report) -> Line {
        Line {
            hard: r.hard(),
            jumps: r.unresolved.len(),
            overlay_jumps: r.overlay_unresolved.len(),
            return_arithmetic: r.hard_ret(),
            ram_changed: r.ram_changed.len(),
            cpu_state: r.hard_cpu(),
            bios: r.bios.len(),
            non_entries: r.indirect_non_entry.len(),
            summaries: r.hard_summary(),
            undiscovered: r.undiscovered_bytes(),
            undiscovered_rom: r.rom_undiscovered_bytes,
            undiscovered_ram: r.ram_undiscovered_bytes,
            reached: r.reached_bytes,
            busy_loops: r.busy_loops.len(),
            flash: r.flash_routines(),
        }
    }

    /// The table's columns: (heading, value).
    fn columns(&self) -> [(&'static str, i64); 10] {
        [
            ("hard", self.hard as i64),
            ("jumps", self.jumps as i64),
            ("ovl jumps", self.overlay_jumps as i64),
            ("ret arith", self.return_arithmetic as i64),
            ("RAM changed", self.ram_changed as i64),
            ("CPU state", self.cpu_state as i64),
            ("BIOS", self.bios as i64),
            ("non-entries", self.non_entries as i64),
            ("summaries", self.summaries as i64),
            ("undiscovered", self.undiscovered as i64),
        ]
    }
}

/// Write `r`'s line to `path` (`gbaa-holes --line FILE`).
pub fn write_line(path: &str, r: &Report) -> Result<(), String> {
    let t = toml::to_string(&Line::of(r)).map_err(|e| e.to_string())?;
    std::fs::write(path, t).map_err(|e| format!("{path}: {e}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusFile {
    game: Vec<Game>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Game {
    name: String,
    rom: String,
    cover: Option<String>,
}

/// One game's run, as `summary.toml` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    /// The child's exit code (0 passed; with `--cover`, 1 is a coverage
    /// difference, the report still written).
    pub exit: i32,
    pub seconds: f64,
    /// None when the child stopped before its report.
    pub line: Option<Line>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// The commit the run was made at ([`analysis::numbers_doc::commit`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The run's date, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default)]
    pub game: Vec<Entry>,
}

/// The numbers file's text for `s`, read from `from` (or just run).
pub fn numbers_doc(s: &Summary, path: &str, from: &Path) -> String {
    let date = s.date.clone().unwrap_or_else(|| {
        std::fs::metadata(from.join("summary.toml"))
            .and_then(|m| m.modified())
            .map(analysis::numbers_doc::date)
            .unwrap_or_else(|_| "on an unknown date".into())
    });
    let commit = s.commit.clone().unwrap_or_else(|| {
        from.file_name()
            .map_or("unknown".into(), |n| n.to_string_lossy().into_owned())
    });
    analysis::numbers_doc::render(
        "The hole report over the corpus",
        &format!("just holes-all --write-doc {path}"),
        &date,
        &commit,
        &[
            "Each game of `holes/corpus.toml` through `gbaa-holes` (its ranking line's \
             numbers, a row a game); `s` is the game's run time in seconds."
                .into(),
        ],
        &table(s, None),
    )
}

/// `n` with its change from `was`: `4 (-3)`, `4 (0)`; no bracket when
/// there's nothing to compare with.
fn cell(n: i64, was: Option<i64>) -> String {
    match was {
        None => n.to_string(),
        Some(w) if n > w => format!("{n} (+{})", n - w),
        Some(w) => format!("{n} ({})", n - w),
    }
}

/// The table: a row per game, `against`'s numbers in brackets.
pub fn table(now: &Summary, against: Option<&Summary>) -> String {
    let heads: Vec<&str> = Line::default().columns().iter().map(|c| c.0).collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut head = vec!["game".to_string()];
    head.extend(heads.iter().map(|h| h.to_string()));
    head.push("s".into());
    rows.push(head);
    let mut total = Line::default();
    let mut total_was = against.map(|_| Line::default());
    let mut all = true;
    for e in &now.game {
        let was = against.and_then(|a| a.game.iter().find(|g| g.name == e.name));
        let was_line = was.and_then(|w| w.line.as_ref());
        let mut row = vec![e.name.clone()];
        match &e.line {
            Some(l) => {
                let wc = was_line.map(Line::columns);
                for (i, (_, v)) in l.columns().iter().enumerate() {
                    row.push(cell(*v, wc.map(|w| w[i].1)));
                }
                add(&mut total, l);
                match (&mut total_was, was_line) {
                    (Some(t), Some(w)) => add(t, w),
                    (Some(_), None) => all = false,
                    _ => {}
                }
            }
            None => {
                all = false;
                row.push(format!("stopped (exit {})", e.exit));
                row.extend((1..heads.len()).map(|_| String::new()));
            }
        }
        row.push(format!("{:.0}", e.seconds));
        rows.push(row);
    }
    let mut row = vec!["total".to_string()];
    // A total with a game missing on either side isn't comparable.
    let tw = total_was.filter(|_| all).map(|t| t.columns());
    for (i, (_, v)) in total.columns().iter().enumerate() {
        row.push(cell(*v, tw.map(|w| w[i].1)));
    }
    row.push(format!(
        "{:.0}",
        now.game.iter().map(|e| e.seconds).sum::<f64>()
    ));
    rows.push(row);
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    let mut s = String::new();
    for r in &rows {
        let cells: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(c, v)| {
                if c == 0 {
                    format!("{v:<w$}", w = widths[c])
                } else {
                    format!("{v:>w$}", w = widths[c])
                }
            })
            .collect();
        writeln!(s, "{}", cells.join("  ").trim_end()).unwrap();
    }
    for e in now.game.iter().filter(|e| e.exit != 0 && e.line.is_some()) {
        writeln!(s, "{}: exit {} (see its .txt)", e.name, e.exit).unwrap();
    }
    s
}

fn add(t: &mut Line, l: &Line) {
    t.hard += l.hard;
    t.jumps += l.jumps;
    t.overlay_jumps += l.overlay_jumps;
    t.return_arithmetic += l.return_arithmetic;
    t.ram_changed += l.ram_changed;
    t.cpu_state += l.cpu_state;
    t.bios += l.bios;
    t.non_entries += l.non_entries;
    t.summaries += l.summaries;
    t.undiscovered += l.undiscovered;
    t.undiscovered_rom += l.undiscovered_rom;
    t.undiscovered_ram += l.undiscovered_ram;
    t.reached += l.reached;
    t.busy_loops += l.busy_loops;
    t.flash += l.flash;
}

/// `out/holes/<short hash>`, `-dirty` when tracked files have changed.
fn default_out() -> Result<PathBuf, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let o = Command::new("git")
            .args(args)
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if !o.status.success() {
            return Err(format!("git {}: failed", args.join(" ")));
        }
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let hash = git(&["rev-parse", "--short", "HEAD"])?;
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    Ok(PathBuf::from("out/holes").join(if dirty { format!("{hash}-dirty") } else { hash }))
}

fn read_summary(dir: &Path) -> Result<Summary, String> {
    let p = dir.join("summary.toml");
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    toml::from_str(&t).map_err(|e| format!("{}: {e}", p.display()))
}

fn write_doc_file(s: &Summary, path: &str, from: &Path) -> ExitCode {
    match analysis::numbers_doc::write(path, &numbers_doc(s, path, from)) {
        Ok(()) => {
            println!("wrote {path}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("gbaa-holes: --write-doc: {e}");
            ExitCode::from(2)
        }
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: gbaa-holes --corpus FILE --roms DIR [--out DIR] [--against DIR] [--only NAME]... [--cover] [--write-doc FILE] [gbaa-holes flags...]\n       gbaa-holes --corpus FILE --from DIR [--write-doc FILE]"
    );
    ExitCode::from(2)
}

/// `gbaa-holes --corpus …`; `args` are the ones after `--corpus`.
pub fn main(args: &[String]) -> ExitCode {
    let [file, rest @ ..] = args else {
        return usage();
    };
    let (mut roms, mut out, mut against) = (None, None, None);
    let (mut write_doc, mut from) = (None::<String>, None::<PathBuf>);
    let (mut only, mut cover, mut pass) = (Vec::new(), false, Vec::new());
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--roms" | "--out" | "--against" | "--only" | "--write-doc" | "--from" => {
                let Some(v) = it.next() else { return usage() };
                match a.as_str() {
                    "--roms" => roms = Some(PathBuf::from(v)),
                    "--write-doc" => write_doc = Some(v.clone()),
                    "--from" => from = Some(PathBuf::from(v)),
                    "--out" => out = Some(PathBuf::from(v)),
                    "--against" => against = Some(PathBuf::from(v)),
                    _ => only.push(v.clone()),
                }
            }
            "--cover" => cover = true,
            "--line" | "--update" => return usage(),
            _ => pass.push(a.clone()),
        }
    }
    if let Some(dir) = &from {
        let s = match read_summary(dir) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("gbaa-holes: --from: {e}");
                return ExitCode::from(2);
            }
        };
        print!("{}", table(&s, None));
        return match &write_doc {
            Some(p) => write_doc_file(&s, p, dir),
            None => ExitCode::SUCCESS,
        };
    }
    let Some(roms) = roms else { return usage() };
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
    let was = match against.as_deref().map(read_summary).transpose() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("gbaa-holes: --against: {e}");
            return ExitCode::from(2);
        }
    };
    let out = match out.map_or_else(default_out, Ok) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("gbaa-holes: {e}; give --out DIR");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("gbaa-holes: {}: {e}", out.display());
        return ExitCode::from(2);
    }
    let me = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gbaa-holes: {e}");
            return ExitCode::from(2);
        }
    };
    eprintln!("gbaa-holes: {} games into {}", games.len(), out.display());
    let mut summary = Summary {
        commit: Some(analysis::numbers_doc::commit()),
        date: Some(analysis::numbers_doc::date(std::time::SystemTime::now())),
        ..Summary::default()
    };
    for g in &games {
        let t = std::time::Instant::now();
        let line_path = out.join(format!("{}.toml", g.name));
        let _ = std::fs::remove_file(&line_path);
        let files = std::fs::File::create(out.join(format!("{}.txt", g.name))).and_then(|o| {
            std::fs::File::create(out.join(format!("{}.log", g.name))).map(|l| (o, l))
        });
        let (txt, log) = match files {
            Ok(f) => f,
            Err(e) => {
                eprintln!("gbaa-holes: {}: {e}", out.display());
                return ExitCode::from(2);
            }
        };
        let mut c = Command::new(&me);
        c.arg(roms.join(&g.rom))
            .args(&pass)
            .arg("--line")
            .arg(&line_path);
        if let (true, Some(f)) = (cover, &g.cover) {
            c.args(["--cover", f]);
        }
        let exit = match c.stdout(Stdio::from(txt)).stderr(Stdio::from(log)).status() {
            Ok(s) => s.code().unwrap_or(-1),
            Err(e) => {
                eprintln!("gbaa-holes: {}: {e}", me.display());
                return ExitCode::from(2);
            }
        };
        let line = std::fs::read_to_string(&line_path)
            .ok()
            .and_then(|t| toml::from_str::<Line>(&t).ok());
        let seconds = t.elapsed().as_secs_f64();
        eprintln!(
            "gbaa-holes: {}: {}, {seconds:.0} s",
            g.name,
            match (&line, exit) {
                (Some(l), 0) => format!("hard {}", l.hard),
                (Some(l), x) => format!("hard {}, exit {x}", l.hard),
                (None, x) => format!("stopped, exit {x} ({}.log)", g.name),
            }
        );
        summary.game.push(Entry {
            name: g.name.clone(),
            exit,
            seconds,
            line,
        });
    }
    let path = out.join("summary.toml");
    if let Err(e) = toml::to_string(&summary)
        .map_err(|e| e.to_string())
        .and_then(|t| std::fs::write(&path, t).map_err(|e| e.to_string()))
    {
        eprintln!("gbaa-holes: {}: {e}", path.display());
        return ExitCode::from(2);
    }
    if let Some(a) = &against {
        println!("against {}:", a.display());
    }
    print!("{}", table(&summary, was.as_ref()));
    println!("reports in {}, data in {}", out.display(), path.display());
    if let Some(p) = &write_doc
        && write_doc_file(&summary, p, &out) != ExitCode::SUCCESS
    {
        return ExitCode::from(2);
    }
    if summary.game.iter().all(|e| e.exit == 0) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, hard: usize, jumps: usize) -> Entry {
        Entry {
            name: name.into(),
            exit: 0,
            seconds: 60.0,
            line: Some(Line {
                hard,
                jumps,
                ..Line::default()
            }),
        }
    }

    #[test]
    fn cells_show_the_change() {
        assert_eq!(cell(4, Some(7)), "4 (-3)");
        assert_eq!(cell(9, Some(7)), "9 (+2)");
        assert_eq!(cell(7, Some(7)), "7 (0)");
        assert_eq!(cell(7, None), "7");
    }

    #[test]
    fn the_table_against_another_run() {
        let now = Summary {
            game: vec![entry("a", 4, 3), entry("b", 10, 10)],
            ..Summary::default()
        };
        let was = Summary {
            game: vec![entry("a", 7, 6), entry("b", 10, 10)],
            ..Summary::default()
        };
        let t = table(&now, Some(&was));
        let rows: Vec<&str> = t.lines().collect();
        assert!(rows[0].starts_with("game") && rows[0].contains("hard"));
        assert!(
            rows[1].starts_with("a ") && rows[1].contains("4 (-3)"),
            "{t}"
        );
        assert!(rows[2].contains("10 (0)"), "{t}");
        assert!(
            rows[3].starts_with("total") && rows[3].contains("14 (-3)"),
            "{t}"
        );
        // Against itself: every change is zero.
        let t = table(&now, Some(&now));
        assert!(!t.contains("(-") && !t.contains("(+"), "{t}");
        // Alone: no brackets.
        assert!(!table(&now, None).contains('('));
    }

    #[test]
    fn a_game_missing_on_one_side() {
        let now = Summary {
            game: vec![
                entry("a", 4, 3),
                Entry {
                    name: "c".into(),
                    exit: 101,
                    seconds: 1.0,
                    line: None,
                },
            ],
            ..Summary::default()
        };
        let was = Summary {
            game: vec![entry("b", 1, 1)],
            ..Summary::default()
        };
        let t = table(&now, Some(&was));
        assert!(t.contains("stopped (exit 101)"), "{t}");
        // a has nothing to compare with, and neither has the total.
        let rows: Vec<&str> = t.lines().collect();
        assert!(rows[1].starts_with("a ") && !rows[1].contains('('), "{t}");
        assert!(
            rows[3].starts_with("total") && !rows[3].contains('('),
            "{t}"
        );
    }

    #[test]
    fn the_numbers_file_from_an_old_summary() {
        // No commit or date kept: the directory's name stands for it.
        let s = Summary {
            game: vec![entry("a", 4, 3)],
            ..Summary::default()
        };
        let t = numbers_doc(&s, "F", Path::new("out/holes/abc1234"));
        assert!(t.contains("`just holes-all --write-doc F`"), "{t}");
        assert!(t.contains("`abc1234`"), "{t}");
        assert!(t.contains("```text\ngame") && t.contains("\ntotal"), "{t}");
        let s = Summary {
            commit: Some("def5678".into()),
            date: Some("2026-11-06".into()),
            ..s
        };
        let t = numbers_doc(&s, "F", Path::new("out/holes/abc1234"));
        assert!(t.contains("2026-11-06 at commit `def5678`"), "{t}");
    }

    #[test]
    fn the_summary_reads_back() {
        let s = Summary {
            commit: Some("abc1234".into()),
            date: Some("2026-11-06".into()),
            game: vec![entry("a", 4, 3)],
        };
        let t = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<Summary>(&t).unwrap(), s);
    }

    #[test]
    fn the_corpus_file_parses() {
        let t = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../holes/corpus.toml"
        ))
        .unwrap();
        let c: CorpusFile = toml::from_str(&t).unwrap();
        assert_eq!(c.game.len(), 8);
        for g in &c.game {
            if let Some(f) = &g.cover {
                let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../").to_string() + f;
                assert!(Path::new(&p).exists(), "{f}");
            }
        }
    }
}
