//! The lift scored against pret (native/lift-pret-score): for each game of
//! `games.toml`, its lift's numbers (`metrics.toml`, `report.txt`, the
//! printed source) beside what the pret names map it was lifted with
//! offers (`[functions]`, `[lines]`, `[locals]`, `[values]`) and the
//! decomp's C lines; one table, a row per measure, a column per game.
//! `just pret-score`; docs/tools/commands.md.
//!
//! The rows, each the lift's number(s) / what the map offers:
//! functions named (`report.txt`'s count) of the map's `[functions]`;
//! `lines` of the decomp's C lines (every `src/**/*.c`); `line_notes` of
//! `[lines]`' rows; `locals_named` + `locals_noted` of `[locals]`'
//! variables; `values_named` of `[values]`' names; `named_field_accesses`
//! of the computed accesses (it plus `computed_unknown_unnamed`);
//! `text_statics` and `texts_left`; `loops_for` and `loops_not_for`;
//! `fn_words_proven` of the fn words; the built lift's run (`[run]`, which
//! `just lift` writes; `not run` without it). And one direct measure from
//! the printed source ([`fn_lines`]): each function's printed lines (not
//! blank, not a comment) per distinct `// file:line` note in it; the
//! median over the functions with a note, and under the table the
//! [`WORST`] by name (an `x_<key>` entry named by its one-line wrapper).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One game of `games.toml`.
#[derive(Clone, Debug, Deserialize)]
pub struct Game {
    pub name: String,
    pub lift: String,
    pub map: String,
    pub decomp: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct Games {
    pub game: Vec<Game>,
}

/// What the names map offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    /// `[functions]` rows.
    pub functions: u64,
    /// `[lines]` rows.
    pub lines: u64,
    /// Variables of `[locals]`, every function's.
    pub locals: u64,
    /// Names of `[values]`, every family's.
    pub values: u64,
    /// The decomp's C lines (every `src/**/*.c`).
    pub c_lines: u64,
}

/// A function's printed lines against the source lines its notes name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FnLines {
    pub name: String,
    /// Its printed lines: not blank, not a comment.
    pub lines: u64,
    /// The distinct `// file:line` notes in it.
    pub source_lines: u64,
}

impl FnLines {
    pub fn ratio(&self) -> f64 {
        self.lines as f64 / self.source_lines as f64
    }
}

/// How many worst functions the summary keeps.
pub const WORST: usize = 20;

/// One game's score, as `summary.toml` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub name: String,
    pub note: String,
    /// The lift's metrics (`metrics.toml`'s top level).
    pub lift: BTreeMap<String, u64>,
    /// `report.txt`'s "N of M functions named".
    pub functions_named: u64,
    pub functions: u64,
    pub offer: Offer,
    /// The built lift's run ([`codegen::native::first_stop::Run::summary`]),
    /// empty when it wasn't run.
    pub run: String,
    /// Functions with a line note.
    pub fns_noted: u64,
    /// The median of their lines per source line.
    pub median: f64,
    /// The [`WORST`] highest, worst first.
    pub worst: Vec<FnLines>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub commit: String,
    pub game: Vec<Score>,
}

/// The names map's offer (its TOML text) and the decomp's C lines.
pub fn offer(map: &str, c_lines: u64) -> Result<Offer, String> {
    let t: toml::Table = map.parse().map_err(|e| format!("{e}"))?;
    let len = |k: &str| {
        t.get(k)
            .and_then(|v| v.as_table())
            .map_or(0, |t| t.len() as u64)
    };
    let locals = t.get("locals").and_then(|v| v.as_table()).map_or(0, |l| {
        l.values()
            .filter_map(|v| v.as_array())
            .map(|a| a.len() as u64)
            .sum()
    });
    let values = t.get("values").and_then(|v| v.as_table()).map_or(0, |vs| {
        vs.values()
            .filter_map(|f| f.get("names").and_then(|n| n.as_array()))
            .map(|a| a.len() as u64)
            .sum()
    });
    Ok(Offer {
        functions: len("functions"),
        lines: len("lines"),
        locals,
        values,
        c_lines,
    })
}

/// The lines of every `.c` file under `dir`, recursively.
pub fn c_lines(dir: &Path) -> Result<u64, String> {
    let mut n = 0;
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in rd {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = e.path();
        let ty = e.file_type().map_err(|e| format!("{}: {e}", p.display()))?;
        if ty.is_dir() {
            n += c_lines(&p)?;
        } else if ty.is_file() && p.extension().is_some_and(|x| x == "c") {
            let b = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            n += b.iter().filter(|&&c| c == b'\n').count() as u64;
        }
    }
    Ok(n)
}

/// The top level of a lift's `metrics.toml`: its whole-number metrics.
pub fn metrics(text: &str) -> Result<BTreeMap<String, u64>, String> {
    let t: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
    Ok(t.iter()
        .filter_map(|(k, v)| Some((k.clone(), u64::try_from(v.as_integer()?).ok()?)))
        .collect())
}

/// `report.txt`'s `names (gbaa-lift --names): N of M functions named`.
pub fn functions_named(report: &str) -> Option<(u64, u64)> {
    let l = report
        .lines()
        .find_map(|l| l.strip_prefix("names (gbaa-lift --names): "))?;
    let (n, rest) = l.split_once(" of ")?;
    let m = rest.split_once(" functions named")?.0;
    Some((n.parse().ok()?, m.parse().ok()?))
}

/// The identifier `s` starts with.
fn ident(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

/// A top-level `fn`'s name, if `l` starts one.
fn fn_start(l: &str) -> Option<&str> {
    let r = l
        .strip_prefix("pub fn ")
        .or_else(|| l.strip_prefix("fn "))?;
    Some(ident(r)).filter(|n| !n.is_empty())
}

/// An `x_<key>` or `a_<key>` entry's name.
fn is_entry(n: &str) -> bool {
    (n.starts_with("x_") || n.starts_with("a_"))
        && n.len() == 10
        && n[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// The printed functions of a lift's source files, each named as a reader
/// sees it (an `x_<key>` entry by the function its one-line wrapper calls
/// or is called by), with its printed lines and the source lines its notes
/// name; those with no note left out.
pub fn fn_lines<'a>(files: impl IntoIterator<Item = &'a str> + Clone) -> Vec<FnLines> {
    // One-line wrappers tie an entry to its name, either way round.
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for t in files.clone() {
        for l in t.lines() {
            let Some(a) = fn_start(l) else { continue };
            let Some(body) = l.trim_end().strip_suffix(" }") else {
                continue;
            };
            let Some(b) = body.rsplit_once("{ ").map(|(_, b)| ident(b)) else {
                continue;
            };
            match (is_entry(a), is_entry(b)) {
                (true, false) if !b.is_empty() => names.insert(a.to_string(), b.to_string()),
                (false, true) => names.insert(b.to_string(), a.to_string()),
                _ => None,
            };
        }
    }
    let mut out = Vec::new();
    for t in files {
        let mut cur: Option<(String, u64, std::collections::BTreeSet<&str>)> = None;
        let mut close = |c: Option<(String, u64, std::collections::BTreeSet<&str>)>| {
            if let Some((name, lines, notes)) = c
                && !notes.is_empty()
            {
                out.push(FnLines {
                    name: names.get(&name).cloned().unwrap_or(name),
                    lines,
                    source_lines: notes.len() as u64,
                });
            }
        };
        for l in t.lines() {
            if let Some(n) = fn_start(l) {
                close(cur.take());
                cur = Some((n.to_string(), 0, Default::default()));
                if !l.trim_end().ends_with('{') {
                    // A one-line function.
                    close(cur.take());
                }
                continue;
            }
            let Some((_, lines, notes)) = cur.as_mut() else {
                continue;
            };
            if l == "}" {
                close(cur.take());
                continue;
            }
            let s = l.trim();
            if codegen::native::is_line_note(l) {
                notes.insert(&s[3..]);
            } else if !s.is_empty() && !s.starts_with("//") {
                *lines += 1;
            }
        }
        close(cur.take());
    }
    out
}

/// The median ratio and the [`WORST`] functions, worst first (ties by name).
pub fn worst(mut fns: Vec<FnLines>) -> (f64, Vec<FnLines>) {
    if fns.is_empty() {
        return (0.0, vec![]);
    }
    fns.sort_by(|a, b| {
        b.ratio()
            .total_cmp(&a.ratio())
            .then_with(|| a.name.cmp(&b.name))
    });
    let n = fns.len();
    let median = if n % 2 == 1 {
        fns[n / 2].ratio()
    } else {
        (fns[n / 2 - 1].ratio() + fns[n / 2].ratio()) / 2.0
    };
    fns.truncate(WORST);
    (median, fns)
}

/// `n` with its change from `was` in brackets, when there is a `was`.
fn with(n: u64, was: Option<u64>) -> String {
    match was {
        Some(w) if w == n => format!("{n} (0)"),
        Some(w) => format!("{n} ({:+})", n as i64 - w as i64),
        None => n.to_string(),
    }
}

fn pct(n: u64, of: u64) -> String {
    if of == 0 {
        String::new()
    } else {
        format!(" ({:.1}%)", 100.0 * n as f64 / of as f64)
    }
}

/// The table: a row per measure, a column per game, each cell the lift's
/// number(s) / what the map offers; `against`'s numbers' changes in
/// brackets. Then each game's note and its worst functions.
pub fn table(now: &Summary, against: Option<&Summary>) -> String {
    type Row = (&'static str, fn(&Score, Option<&Score>) -> String);
    let rows: [Row; 11] = [
        ("functions named / map's functions", |s, w| {
            format!(
                "{} / {}{}",
                with(s.functions_named, w.map(|w| w.functions_named)),
                s.offer.functions,
                pct(s.functions_named, s.offer.functions)
            )
        }),
        ("lines / decomp's C lines", |s, w| {
            let l = get(s, "lines");
            format!(
                "{} / {} ({:.2}x)",
                with(l, w.map(|w| get(w, "lines"))),
                s.offer.c_lines,
                l as f64 / s.offer.c_lines.max(1) as f64
            )
        }),
        ("line notes / [lines]", |s, w| {
            let n = get(s, "line_notes");
            format!(
                "{} / {}{}",
                with(n, w.map(|w| get(w, "line_notes"))),
                s.offer.lines,
                pct(n, s.offer.lines)
            )
        }),
        ("locals named + noted / [locals]", |s, w| {
            let (a, b) = (get(s, "locals_named"), get(s, "locals_noted"));
            format!(
                "{} + {} / {}{}",
                with(a, w.map(|w| get(w, "locals_named"))),
                with(b, w.map(|w| get(w, "locals_noted"))),
                s.offer.locals,
                pct(a, s.offer.locals)
            )
        }),
        ("values_named / [values] names", |s, w| {
            let n = get(s, "values_named");
            format!(
                "{} / {}",
                with(n, w.map(|w| get(w, "values_named"))),
                s.offer.values
            )
        }),
        ("named fields / computed accesses", |s, w| {
            let (a, b) = (
                get(s, "named_field_accesses"),
                get(s, "computed_unknown_unnamed"),
            );
            format!(
                "{} / {}{}",
                with(a, w.map(|w| get(w, "named_field_accesses"))),
                a + b,
                pct(a, a + b)
            )
        }),
        ("texts: statics, left", |s, w| {
            format!(
                "{}, {}",
                with(get(s, "text_statics"), w.map(|w| get(w, "text_statics"))),
                with(get(s, "texts_left"), w.map(|w| get(w, "texts_left")))
            )
        }),
        ("loops: for, loop", |s, w| {
            format!(
                "{}, {}",
                with(get(s, "loops_for"), w.map(|w| get(w, "loops_for"))),
                with(get(s, "loops_not_for"), w.map(|w| get(w, "loops_not_for")))
            )
        }),
        ("fn words proven / all", |s, w| {
            let (a, b) = (get(s, "fn_words_proven"), get(s, "fn_words_not_proven"));
            format!(
                "{} / {}{}",
                with(a, w.map(|w| get(w, "fn_words_proven"))),
                a + b,
                pct(a, a + b)
            )
        }),
        ("lines per source line (median)", |s, w| {
            let mut t = format!("{:.2} of {} fns", s.median, s.fns_noted);
            if let Some(w) = w {
                t += &format!(" ({:+.2})", s.median - w.median);
            }
            t
        }),
        ("built and run", |s, _| {
            if s.run.is_empty() {
                "not run".into()
            } else {
                s.run.clone()
            }
        }),
    ];
    let mut cells: Vec<Vec<String>> = vec![
        std::iter::once("measure".to_string())
            .chain(now.game.iter().map(|g| g.name.clone()))
            .collect(),
    ];
    for (label, f) in rows {
        let mut r = vec![label.to_string()];
        for g in &now.game {
            let was = against.and_then(|a| a.game.iter().find(|w| w.name == g.name));
            r.push(f(g, was));
        }
        cells.push(r);
    }
    let cols = cells[0].len();
    let width: Vec<usize> = (0..cols)
        .map(|c| {
            cells
                .iter()
                .map(|r| r[c].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut t = String::new();
    for r in &cells {
        let line: Vec<String> = r
            .iter()
            .zip(&width)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        t += line.join("  ").trim_end();
        t += "\n";
    }
    for g in &now.game {
        if !g.note.is_empty() {
            t += &format!("\n{}: {}\n", g.name, g.note);
        } else {
            t += &format!("\n{}:\n", g.name);
        }
        t += &format!(
            "  the {} with the most lines per source line:\n",
            g.worst.len()
        );
        for f in &g.worst {
            t += &format!(
                "    {:.1}  {} ({} lines, {} source lines)\n",
                f.ratio(),
                f.name,
                f.lines,
                f.source_lines
            );
        }
    }
    t
}

fn get(s: &Score, k: &str) -> u64 {
    s.lift.get(k).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests;
