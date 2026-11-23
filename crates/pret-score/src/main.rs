//! `pret-score [--games FILE] [--data DIR] [--out DIR] [--against DIR]
//! [--from DIR] [--only NAME]...`: each game's lift scored against what
//! its pret names map offers (crates/pret-score; `just pret-score`). Reads
//! the lifts as they are (lift them first: `just lift`), writes
//! OUT/summary.toml (OUT defaults to `out/pret-score/<git short
//! hash>[-dirty]`) and prints the table, with changes from `--against`
//! DIR's summary in brackets. `--from DIR` reads nothing else and prints
//! DIR's summary.

use pret_score::{Games, Score, Summary, c_lines, fn_lines, functions_named, metrics, offer};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: pret-score [--games FILE] [--data DIR] [--out DIR] [--against DIR] [--from DIR] [--only NAME]..."
    );
    ExitCode::from(2)
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn read_summary(dir: &Path) -> Result<Summary, String> {
    let p = dir.join("summary.toml");
    toml::from_str(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))
}

fn score(g: &pret_score::Game, data: &Path) -> Result<Score, String> {
    let lift = Path::new(&g.lift);
    let mtext = read(&lift.join("metrics.toml"))?;
    let lift_metrics = metrics(&mtext)?;
    let report = read(&lift.join("report.txt"))?;
    let (named, functions) = functions_named(&report).ok_or(format!(
        "{}: no `names (gbaa-lift --names): N of M functions named` line: lifted without a names map?",
        lift.join("report.txt").display()
    ))?;
    let run = codegen::native::first_stop::Run::read(&mtext)?
        .map(|r| r.summary())
        .unwrap_or_default();
    let c = c_lines(&data.join(&g.decomp).join("src"))?;
    let offer = offer(&read(&data.join(&g.map))?, c)?;
    let src = lift.join("src");
    let mut files = Vec::new();
    let mut names: Vec<PathBuf> = std::fs::read_dir(&src)
        .map_err(|e| format!("{}: {e}", src.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    names.sort();
    for p in &names {
        files.push(read(p)?);
    }
    let fns = fn_lines(files.iter().map(String::as_str));
    let noted = fns.len() as u64;
    let (median, worst) = pret_score::worst(fns);
    Ok(Score {
        name: g.name.clone(),
        note: g.note.clone(),
        lift: lift_metrics,
        functions_named: named,
        functions,
        offer,
        run,
        fns_noted: noted,
        median,
        worst,
    })
}

fn main() -> ExitCode {
    let mut games = PathBuf::from("crates/pret-score/games.toml");
    let mut data = std::env::var_os("GBAA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/gbaa")
        });
    let (mut out, mut against, mut from) = (None, None, None);
    let mut only: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let Some(v) = it.next() else {
            return usage();
        };
        match a.as_str() {
            "--games" => games = PathBuf::from(v),
            "--data" => data = PathBuf::from(v),
            "--out" => out = Some(PathBuf::from(v)),
            "--against" => against = Some(PathBuf::from(v)),
            "--from" => from = Some(PathBuf::from(v)),
            "--only" => only.push(v),
            _ => return usage(),
        }
    }
    let was = match against.as_deref().map(read_summary).transpose() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("pret-score: --against: {e}");
            return ExitCode::FAILURE;
        }
    };
    let summary = if let Some(f) = &from {
        match read_summary(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("pret-score: --from: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        let list: Games = match read(&games)
            .and_then(|t| toml::from_str(&t).map_err(|e| format!("{}: {e}", games.display())))
        {
            Ok(g) => g,
            Err(e) => {
                eprintln!("pret-score: {e}");
                return ExitCode::FAILURE;
            }
        };
        let mut s = Summary {
            commit: analysis::numbers_doc::commit(),
            game: vec![],
        };
        for g in list
            .game
            .iter()
            .filter(|g| only.is_empty() || only.contains(&g.name))
        {
            match score(g, &data) {
                Ok(sc) => s.game.push(sc),
                Err(e) => {
                    eprintln!("pret-score: {}: {e}", g.name);
                    return ExitCode::FAILURE;
                }
            }
        }
        let dir = out.unwrap_or_else(|| PathBuf::from("out/pret-score").join(&s.commit));
        let p = dir.join("summary.toml");
        if let Err(e) = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|_| toml::to_string(&s).map_err(|e| e.to_string()))
            .and_then(|t| std::fs::write(&p, t).map_err(|e| e.to_string()))
        {
            eprintln!("pret-score: {}: {e}", p.display());
            return ExitCode::FAILURE;
        }
        println!("pret-score: {} (commit {})", p.display(), s.commit);
        s
    };
    if let Some(a) = &against {
        println!("against {}:", a.display());
    }
    print!("{}", pret_score::table(&summary, was.as_ref()));
    ExitCode::SUCCESS
}
