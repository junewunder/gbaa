//! `xmatch judge OUT ROM MAP ROM MAP… [--limit N] [--model M] [--effort E]
//! [--jobs N] [--gap N] [--min S] [--seed N]`: a model as the judge for the library
//! functions xmatch leaves unnamed (mod/opus-library-judge;
//! docs/research/cross-compiler-matching.md, "A model as judge").
//!
//! Each ROM in turn is the game, named from the others' libraries as
//! `xmatch name --library` would. Of its functions left unnamed, the
//! library-looking ones (in a run of the ROM's library code: at most
//! `--gap` functions, by address, from a named library function on each
//! side; and the nearest reference library function's own-token similarity
//! at least `--min`) with a real name in the game's own map are
//! the trial: the agent (`claude -p` as `naming ask` runs it) sees the
//! function's readable print and the three nearest reference library
//! functions', named, and answers which one it is, or none. At most
//! `--limit` new calls (default 0: write the prompts' plan and score what
//! is there); answers go to `OUT/<game>/<key>.toml`, a rerun asks only what
//! is missing, in the same seeded order. Every answer is scored against the
//! game's own map (strict, as `xmatch matrix` scores) into `OUT/report.md`.

use crate::{Rom, claims, norm, placeholder};
use analysis::discovery::discover;
use codegen::emit::pretty;
use codegen::func::{Ctx, build_func};
use codegen::lower::lower_func_checked;
use codegen::names::SourceMap;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use toml::{Table, Value};
use xmatch::{Config, nearest};

const SYSTEM: &str = "You are an expert reverse engineer of Game Boy Advance games, \
    telling which known library function a function from a game with no symbols is. \
    Answer only with the TOML asked for, no prose.";

/// The candidates shown per function.
const SHOWN: usize = 3;

/// A print longer than this many lines is cut, so one huge function
/// doesn't make the prompt.
const MAX_LINES: usize = 400;

pub struct Opts {
    pub limit: usize,
    pub model: String,
    pub effort: Option<String>,
    pub jobs: usize,
    pub min: f64,
    /// The most functions between two named library functions of a run.
    pub gap: usize,
    pub seed: u64,
}

/// One reference library function shown to the judge.
#[derive(Clone)]
struct Cand {
    r: usize,
    x: usize,
    score: f64,
}

/// One trial function: game `g`'s function `y`, its true name and the
/// candidates.
struct Item {
    g: usize,
    y: usize,
    truth: String,
    /// Whether any reference library has the true name.
    answerable: bool,
    cands: Vec<Cand>,
}

/// A game's name for files and the report: its map's file stem.
fn game_name(r: &Rom) -> String {
    let p = r.map.as_deref().unwrap_or(&r.path);
    Path::new(p)
        .file_stem()
        .map_or("rom".into(), |s| s.to_string_lossy().into_owned())
}

fn answer_path(out: &Path, game: &str, key: u32) -> PathBuf {
    out.join(game).join(format!("{key:08x}.toml"))
}

/// Readable prints of `keys` in the ROM at `path`, named by `map`.
fn prints(
    path: &str,
    map: &SourceMap,
    keys: &BTreeSet<u32>,
) -> Result<BTreeMap<u32, String>, String> {
    let rom = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let disc = discover(rom.clone());
    let mut out = BTreeMap::new();
    let print = |k: u32, readable: bool| {
        std::panic::catch_unwind(|| {
            let mut ctx = Ctx::new(&rom, &disc);
            let only = BTreeSet::from([k]);
            ctx.names = Some(map);
            if readable {
                ctx.readable = Some(&only);
            }
            let f = build_func(&ctx, k);
            let (body, _) = lower_func_checked(&ctx, &f, &ctx.fn_ident(k));
            pretty(body)
        })
        .ok()
    };
    for &k in keys {
        // A function the readable lowering can't print yet is shown plain.
        let text = print(k, true)
            .or_else(|| {
                eprintln!("{path}: 0x{k:08x}: the readable print panicked; printing it plain");
                print(k, false)
            })
            .unwrap_or_else(|| format!("// 0x{k:08x}: no print (codegen panicked)"));
        let lines: Vec<&str> = text.lines().collect();
        let mut s = lines[..lines.len().min(MAX_LINES)].join("\n");
        if lines.len() > MAX_LINES {
            let _ = write!(s, "\n// … {} more lines cut", lines.len() - MAX_LINES);
        }
        s.push('\n');
        out.insert(k, s);
    }
    Ok(out)
}

/// The source map xmatch's library claims give game `b`: what a reader of
/// a ROM with no symbols would have.
fn claimed_map(
    refs: &[Rom],
    b: &Rom,
    best: &BTreeMap<usize, (crate::Claim, usize)>,
) -> Result<SourceMap, String> {
    let mut used = BTreeSet::new();
    let mut t = format!(
        "[map]\nrom_sha1 = \"{}\"\nsource = \"xmatch\"\nformat = 1\n\n[functions]\n",
        b.sha
    );
    for (&y, (c, _)) in best {
        let n = &refs[c.r].names[c.p.a].as_ref().unwrap().0;
        if used.insert(norm(n)) {
            let _ = writeln!(
                t,
                "0x{:08x} = {{ name = {} }}",
                b.side.funs[y].key,
                Value::String(n.clone())
            );
        }
    }
    SourceMap::from_texts(&b.sha, &[&t])
}

fn prompt(
    game_print: &str,
    key: u32,
    insns: usize,
    cands: &[(String, String, usize, String)],
) -> String {
    let mode = if key & 1 != 0 { "Thumb" } else { "ARM" };
    let mut p = format!(
        "A function from a Game Boy Advance game with no symbols, f_{key:08x} ({mode}, {insns} \
         instructions), looks like a library function (libgcc, the m4a sound driver, the BIOS call \
         stubs, the save, multiboot or link libraries). Below are the {} known library functions \
         most like it, from other games' decompilations. Is it one of them (the same function, \
         maybe another version or compiler), or none?\n\n\
         The code was recompiled from ARM7TDMI machine code to Rust: each `insn!` line is one \
         guest instruction as assembly, followed by what it does; `m.bus` is memory, `r0`..`r15` \
         registers, `rt::` timing. Calls print as the callee's name if it has one, else \
         `f_<address>`.\n\n## The function\n\n```rust\n{game_print}```\n\n",
        cands.len()
    );
    for (i, (name, game, n, print)) in cands.iter().enumerate() {
        let letter = (b'A' + i as u8) as char;
        let _ = write!(
            p,
            "## Candidate {letter}: {name} (from {game}, {n} instructions)\n\n```rust\n{print}```\n\n"
        );
    }
    let letters: Vec<String> = (0..cands.len())
        .map(|i| format!("\"{}\"", (b'A' + i as u8) as char))
        .collect();
    let _ = write!(
        p,
        "Answer with TOML only, in this form (choice one of {}, or \"none\"):\n\n\
         choice = \"A\"\nconfidence = 0.8\nwhy = \"one line\"\n",
        letters.join(", ")
    );
    p
}

/// The judge's choice in its answer: `A`.., `none`, or why it didn't parse.
/// The TOML is the first fenced block if there is one (an answer may say
/// a line of prose first), else the whole answer.
fn parse_choice(text: &str) -> Result<String, String> {
    let body = match text.split_once("```") {
        Some((_, rest)) => {
            let rest = rest.strip_prefix("toml").unwrap_or(rest);
            rest.split_once("```").map_or(rest, |(b, _)| b)
        }
        None => text,
    };
    let t: Table = toml::from_str(body).map_err(|e| format!("{e}"))?;
    let c = t
        .get("choice")
        .and_then(Value::as_str)
        .ok_or("no choice")?
        .trim()
        .to_string();
    if c.eq_ignore_ascii_case("none") {
        return Ok("none".into());
    }
    match c.as_bytes() {
        [l] if l.is_ascii_alphabetic() => Ok(c.to_ascii_uppercase()),
        _ => Err(format!("choice {c:?}")),
    }
}

pub fn run(roms: &[Rom], objects: &BTreeSet<String>, out: &Path, o: &Opts) -> Result<(), String> {
    let cfg = Config::library();
    let cut: Vec<Rom> = roms
        .iter()
        .map(|r| crate::library_only(r.clone(), objects))
        .collect();
    let games: Vec<String> = roms.iter().map(game_name).collect();
    let mut items: Vec<Vec<Item>> = Vec::new();
    let mut claimed = Vec::new();
    let mut library_looking = Vec::new();
    for (g, b) in roms.iter().enumerate() {
        let refs: Vec<Rom> = (0..cut.len())
            .filter(|&r| r != g)
            .map(|r| cut[r].clone())
            .collect();
        let best = claims(&refs, b, cfg);
        let ref_names: BTreeSet<String> = refs
            .iter()
            .flat_map(|r| r.names.iter().flatten().map(|(n, _)| norm(n)))
            .collect();
        // Libraries link as runs of objects: a function within `gap`
        // functions of a named library function on both sides is in one.
        let mut by_addr: Vec<usize> = (0..b.side.funs.len()).collect();
        by_addr.sort_by_key(|&y| b.side.funs[y].key & !1);
        let mut in_run = vec![false; b.side.funs.len()];
        let mut prev = None;
        let mut after = vec![None; by_addr.len()];
        for (i, &y) in by_addr.iter().enumerate().rev() {
            if best.contains_key(&y) {
                prev = Some(i);
            }
            after[i] = prev;
        }
        prev = None;
        for (i, &y) in by_addr.iter().enumerate() {
            if best.contains_key(&y) {
                prev = Some(i);
            }
            if let (Some(lo), Some(hi)) = (prev, after[i]) {
                in_run[y] = hi - lo <= o.gap + 1;
            }
        }
        let real = |y: usize| matches!(&b.names[y], Some((n, _)) if !placeholder(n));
        let unnamed: Vec<usize> = (0..b.side.funs.len())
            .filter(|&y| !best.contains_key(&y) && in_run[y])
            .collect();
        let missed = (0..b.side.funs.len())
            .filter(|&y| !best.contains_key(&y) && real(y))
            .filter(|&y| ref_names.contains(&norm(&b.names[y].as_ref().unwrap().0)))
            .count();
        let mut all: Vec<Vec<Cand>> = vec![Vec::new(); unnamed.len()];
        for (r, a) in refs.iter().enumerate() {
            for (i, near) in nearest(&a.side, &b.side, &unnamed, SHOWN * 2)
                .into_iter()
                .enumerate()
            {
                all[i].extend(near.into_iter().map(|(x, score)| Cand { r, x, score }));
            }
        }
        let mut looking = 0;
        let mut list = Vec::new();
        for (i, mut cs) in all.into_iter().enumerate() {
            cs.sort_by(|p, q| q.score.total_cmp(&p.score));
            let mut seen = BTreeSet::new();
            cs.retain(|c| {
                let n = &refs[c.r].names[c.x].as_ref().unwrap().0;
                !placeholder(n) && seen.insert(norm(n))
            });
            cs.truncate(SHOWN);
            if cs.first().is_none_or(|c| c.score < o.min) {
                continue;
            }
            looking += 1;
            let y = unnamed[i];
            let Some((truth, _)) = &b.names[y] else {
                continue;
            };
            if placeholder(truth) {
                continue;
            }
            // Candidates by their index into all of `roms`.
            let cands = cs
                .into_iter()
                .map(|c| Cand {
                    r: if c.r < g { c.r } else { c.r + 1 },
                    ..c
                })
                .collect();
            list.push(Item {
                g,
                y,
                truth: truth.clone(),
                answerable: ref_names.contains(&norm(truth)),
                cands,
            });
        }
        // How the bar sorts them: at each bar, the trial's functions over
        // it, and how many of those a reference library names.
        let bars: Vec<String> = [0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]
            .iter()
            .map(|&bar| {
                let over: Vec<&Item> = list.iter().filter(|it| it.cands[0].score >= bar).collect();
                let n = over.iter().filter(|it| it.answerable).count();
                format!("{bar}: {} ({n})", over.len())
            })
            .collect();
        println!(
            "  {} by bar (in a reference): {}",
            games[g],
            bars.join(", ")
        );
        naming::Rng::new(o.seed ^ g as u64).shuffle(&mut list);
        println!(
            "{}: {} functions, {} named by xmatch --library, {missed} unnamed with a name a reference library has; {} unnamed library-looking (in a run, gap {}, nearest >= {}), {} with a real name ({} of them in a reference library)",
            games[g],
            b.side.funs.len(),
            best.len(),
            looking,
            o.gap,
            o.min,
            list.len(),
            list.iter().filter(|it| it.answerable).count()
        );
        claimed.push(claimed_map(&refs, b, &best)?);
        library_looking.push(looking);
        items.push(list);
    }
    // The trial's order: round robin over the games' shuffled lists.
    let mut order: Vec<&Item> = Vec::new();
    let longest = items.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..longest {
        for list in &items {
            if let Some(it) = list.get(i) {
                order.push(it);
            }
        }
    }
    let key = |it: &Item| roms[it.g].side.funs[it.y].key;
    let todo: Vec<&Item> = order
        .iter()
        .copied()
        .filter(|it| !answer_path(out, &games[it.g], key(it)).exists())
        .take(o.limit)
        .collect();
    if !todo.is_empty() {
        ask(roms, &cut, &games, &claimed, out, &todo, o)?;
    }
    report(roms, &cut, &games, &items, &library_looking, out)
}

/// Prints every function `todo` shows, then asks the judge about each,
/// `jobs` at a time.
fn ask(
    roms: &[Rom],
    cut: &[Rom],
    games: &[String],
    claimed: &[SourceMap],
    out: &Path,
    todo: &[&Item],
    o: &Opts,
) -> Result<(), String> {
    // (rom, key) → print: the games' functions under xmatch's names, the
    // candidates under their own maps.
    let mut want: BTreeMap<(usize, bool), BTreeSet<u32>> = BTreeMap::new();
    for it in todo {
        want.entry((it.g, true))
            .or_default()
            .insert(roms[it.g].side.funs[it.y].key);
        for c in &it.cands {
            want.entry((c.r, false))
                .or_default()
                .insert(cut[c.r].side.funs[c.x].key);
        }
    }
    let mut printed: BTreeMap<(usize, bool, u32), String> = BTreeMap::new();
    for (&(r, game), keys) in &want {
        let own;
        let map = if game {
            &claimed[r]
        } else {
            own = SourceMap::load(&roms[r].sha, &[roms[r].map.as_deref().unwrap()])?;
            &own
        };
        for (k, s) in prints(&roms[r].path, map, keys)? {
            printed.insert((r, game, k), s);
        }
    }
    let jobs: Vec<(PathBuf, String, Table)> = todo
        .iter()
        .map(|it| {
            let b = &roms[it.g];
            let k = b.side.funs[it.y].key;
            let cands: Vec<(String, String, usize, String)> = it
                .cands
                .iter()
                .map(|c| {
                    let a = &cut[c.r];
                    let ck = a.side.funs[c.x].key;
                    (
                        a.names[c.x].as_ref().unwrap().0.clone(),
                        games[c.r].clone(),
                        a.side.funs[c.x].insns,
                        printed[&(c.r, false, ck)].clone(),
                    )
                })
                .collect();
            let p = prompt(
                &printed[&(it.g, true, k)],
                k,
                b.side.funs[it.y].insns,
                &cands,
            );
            let mut t = Table::new();
            t.insert("game".into(), Value::String(games[it.g].clone()));
            t.insert("key".into(), Value::String(format!("{k:08x}")));
            t.insert(
                "insns".into(),
                Value::Integer(b.side.funs[it.y].insns as i64),
            );
            t.insert("truth".into(), Value::String(it.truth.clone()));
            t.insert("answerable".into(), Value::Boolean(it.answerable));
            let cs: Vec<Value> = it
                .cands
                .iter()
                .zip(&cands)
                .map(|(c, (n, g, _, _))| {
                    let mut ct = Table::new();
                    ct.insert("name".into(), Value::String(n.clone()));
                    ct.insert("from".into(), Value::String(g.clone()));
                    ct.insert(
                        "key".into(),
                        Value::String(format!("{:08x}", cut[c.r].side.funs[c.x].key)),
                    );
                    ct.insert(
                        "score".into(),
                        Value::Float((c.score * 1000.0).round() / 1000.0),
                    );
                    Value::Table(ct)
                })
                .collect();
            t.insert("candidates".into(), Value::Array(cs));
            (answer_path(out, &games[it.g], k), p, t)
        })
        .collect();
    for g in games {
        std::fs::create_dir_all(out.join(g)).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    let queue = Mutex::new(jobs.into_iter());
    let failed = Mutex::new(0);
    std::thread::scope(|s| {
        for _ in 0..o.jobs.max(1) {
            s.spawn(|| {
                loop {
                    let next = queue.lock().unwrap().next();
                    let Some((path, p, mut t)) = next else { break };
                    let _ = std::fs::write(path.with_extension("prompt"), &p);
                    match naming::claude(&o.model, o.effort.as_deref(), SYSTEM, &p) {
                        Ok((text, usage)) => {
                            t.extend(usage);
                            match parse_choice(&text) {
                                Ok(c) => {
                                    t.insert("choice".into(), Value::String(c));
                                }
                                Err(e) => {
                                    t.insert("error".into(), Value::String(e));
                                }
                            }
                            t.insert("raw".into(), Value::String(text));
                            let _ = std::fs::write(&path, toml::to_string(&t).unwrap());
                            let c = t.get("choice").and_then(Value::as_str).unwrap_or("?");
                            eprintln!("{}: {c}", path.display());
                        }
                        Err(e) => {
                            *failed.lock().unwrap() += 1;
                            eprintln!("{}: failed: {e}", path.display());
                        }
                    }
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    if failed > 0 {
        return Err(format!("{failed} calls failed; rerun to ask them again"));
    }
    Ok(())
}

/// One game's tally.
#[derive(Default)]
struct Tally {
    asked: usize,
    answerable: usize,
    /// The true name among the candidates shown.
    shown: usize,
    /// Answers naming a candidate, and of them right.
    named: usize,
    right: usize,
    /// Wrong by name, but the pick's code is byte for byte the function's
    /// (masked as xmatch's byte anchors are): another decomp's name.
    same_code: usize,
    /// "none" where no reference library has the name.
    none_right: usize,
    none_wrong: usize,
    unparsed: usize,
    cost: f64,
    secs: f64,
}

fn report(
    roms: &[Rom],
    cut: &[Rom],
    games: &[String],
    items: &[Vec<Item>],
    looking: &[usize],
    out: &Path,
) -> Result<(), String> {
    let mut tallies: Vec<Tally> = games.iter().map(|_| Tally::default()).collect();
    let mut wrong = Vec::new();
    for list in items {
        for it in list {
            let path = answer_path(out, &games[it.g], roms[it.g].side.funs[it.y].key);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let t: Table = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let s = &mut tallies[it.g];
            s.asked += 1;
            s.cost += t.get("cost_usd").and_then(Value::as_float).unwrap_or(0.0);
            s.secs += t.get("seconds").and_then(Value::as_float).unwrap_or(0.0);
            let name = |c: &Cand| norm(&cut[c.r].names[c.x].as_ref().unwrap().0);
            let truth = norm(&it.truth);
            if it.answerable {
                s.answerable += 1;
            }
            if it.cands.iter().any(|c| name(c) == truth) {
                s.shown += 1;
            }
            // Re-read from the raw answer, so a better parser applies to
            // old answers too.
            let choice = t
                .get("raw")
                .and_then(Value::as_str)
                .map(parse_choice)
                .and_then(Result::ok);
            match choice.as_deref() {
                None => s.unparsed += 1,
                Some("none") => {
                    if it.answerable {
                        s.none_wrong += 1;
                    } else {
                        s.none_right += 1;
                    }
                }
                Some(l) => {
                    let i = (l.as_bytes()[0] - b'A') as usize;
                    let Some(c) = it.cands.get(i) else {
                        s.unparsed += 1;
                        continue;
                    };
                    s.named += 1;
                    if name(c) == truth {
                        s.right += 1;
                    } else {
                        let same =
                            cut[c.r].side.funs[c.x].bytes == roms[it.g].side.funs[it.y].bytes;
                        if same {
                            s.same_code += 1;
                        }
                        wrong.push(format!(
                            "| {} | 0x{:08x} | {} | {} ({}) | {} |",
                            games[it.g],
                            roms[it.g].side.funs[it.y].key,
                            it.truth,
                            cut[c.r].names[c.x].as_ref().unwrap().0,
                            games[c.r],
                            if same { "yes" } else { "" }
                        ));
                    }
                }
            }
        }
    }
    let pct = |n: usize, d: usize| crate::pct(n, d);
    let mut r = String::from(
        "# A model as judge for unmatched library functions\n\n\
         Written by `xmatch judge` (mod/opus-library-judge). Each game named from the others' \
         libraries; of its unnamed functions whose nearest reference library function scores \
         over the bar, those with a real name in its own map were asked about. Strict: right \
         means the chosen candidate's name is the game's own (case aside).\n\n\
         | game | library-looking | in the trial | asked | name in a reference | true name shown | named | right | same code | precision | recall | \"none\" right / wrong | unparsed | cost | seconds |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let mut total = Tally::default();
    for (g, s) in tallies.iter().enumerate() {
        let _ = writeln!(
            r,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} | ${:.2} | {:.0} |",
            games[g],
            looking[g],
            items[g].len(),
            s.asked,
            s.answerable,
            s.shown,
            s.named,
            s.right,
            s.same_code,
            pct(s.right, s.named),
            pct(s.right, s.answerable),
            s.none_right,
            s.none_wrong,
            s.unparsed,
            s.cost,
            s.secs
        );
        total.asked += s.asked;
        total.answerable += s.answerable;
        total.shown += s.shown;
        total.named += s.named;
        total.right += s.right;
        total.same_code += s.same_code;
        total.none_right += s.none_right;
        total.none_wrong += s.none_wrong;
        total.unparsed += s.unparsed;
        total.cost += s.cost;
        total.secs += s.secs;
    }
    let _ = writeln!(
        r,
        "| all | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} | ${:.2} | {:.0} |",
        looking.iter().sum::<usize>(),
        items.iter().map(Vec::len).sum::<usize>(),
        total.asked,
        total.answerable,
        total.shown,
        total.named,
        total.right,
        total.same_code,
        pct(total.right, total.named),
        pct(total.right, total.answerable),
        total.none_right,
        total.none_wrong,
        total.unparsed,
        total.cost,
        total.secs
    );
    if !wrong.is_empty() {
        r.push_str("\nWrong picks:\n\n| game | key | its name | picked | same code |\n|---|---|---|---|---|\n");
        for w in &wrong {
            r.push_str(w);
            r.push('\n');
        }
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let path = out.join("report.md");
    std::fs::write(&path, &r).map_err(|e| format!("{}: {e}", path.display()))?;
    print!("{r}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_choice;

    #[test]
    fn choices_parse() {
        assert_eq!(parse_choice("choice = \"b\"\nwhy = \"x\"\n").unwrap(), "B");
        assert_eq!(
            parse_choice("```toml\nchoice = \"None\"\n```").unwrap(),
            "none"
        );
        let prose =
            "Stores a byte. Another compiler.\n\n```toml\nchoice = \"A\"\nconfidence = 0.9\n```";
        assert_eq!(parse_choice(prose).unwrap(), "A");
        assert!(parse_choice("choice = \"AB\"").is_err());
        assert!(parse_choice("I think it's A").is_err());
    }
}
