//! `naming prepare ROM PRET OUT [--seed N]`: draw the sample from ROM's
//! functions that the source map PRET names (`docs/mod-view-design.md`,
//! "The naming loop") and write `OUT/sample.toml`: each function's size,
//! callees, callers and pool data, and pret's names, which only `score`
//! reads.
//!
//! `naming sibling ROM PRET SIB_ROM SIB_PRET OUT [--seed N] [--count N]`:
//! the sibling leak control's sample instead (`docs/mod-view-design.md`,
//! "Measuring the leak"): COUNT (30) of ROM's functions whose code is the
//! same as one of SIB_ROM's under another pret name, each with that name
//! as `sibling`, which `score` counts answers giving.
//!
//! `naming ask ROM OUT ROUND [--jobs N] [--model M] [--effort E] [--limit N]`:
//! ask an agent (`claude -p` with no tools, at effort E if given) for each sampled function's
//! name, in ROUND: `0` (code alone), `1` (bottom-up, callees' round-1
//! names), `2` (callers' names too), or a leak control, `nocode` (address
//! and size only) or `recog` (which game is this?). Each answer, with its
//! tokens and seconds, goes to `OUT/<round>/<key>.toml` and its prompt
//! beside it; a rerun asks only what has no answer yet.
//!
//! `naming score OUT`: score every round's answers against pret's names
//! and write `OUT/report.md`, and the agent's names as a source map,
//! `OUT/names.toml`.

use analysis::discovery::discover;
use analysis::ir::Op;
use codegen::emit::pretty;
use codegen::func::{Ctx, build_func};
use codegen::lower::lower_func_checked;
use codegen::names::{SourceMap, sha1_hex};
use naming::{Answer, Class, Graph, Plan, Score, data_subtokens, draw, exact, levels};
use naming::{parse_answer, subtokens, vote};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use toml::{Table, Value};

/// The objects whose functions aren't sampled: libgcc, m4a and the SDK's
/// libraries (pret names them from asm or vendor symbols, and a matcher
/// would name them in any game). Their names are given in every print.
fn library(file: &str) -> bool {
    file.starts_with("m4a")
        || file.starts_with("lib")
        || file.starts_with('_')
        || file.starts_with("agb_flash")
        || matches!(file, "crt0.o" | "rom_header.o" | "Div.o" | "siirtc.o")
}

fn in_ram(v: u32) -> bool {
    (0x0200_0000..0x0204_0000).contains(&v) || (0x0300_0000..0x0300_8000).contains(&v)
}

/// A sampled function, as `sample.toml` holds it.
#[derive(Clone, Debug, Default)]
struct Fun {
    insns: usize,
    class: Option<Class>,
    pret: String,
    /// The sibling control's other game's name for the same code.
    sibling: Option<String>,
    /// The roots of the subtrees it was drawn in.
    roots: Vec<u32>,
    callees: Vec<u32>,
    callers: Vec<u32>,
    data: Vec<u32>,
}

/// A datum a sampled function's pool points at.
#[derive(Clone, Debug, Default)]
struct Datum {
    /// The gap to the next pool target in the ROM: our size estimate.
    size: u32,
    pret: Option<String>,
}

#[derive(Default)]
struct SampleFile {
    rom_sha1: String,
    seed: u64,
    funcs: BTreeMap<u32, Fun>,
    data: BTreeMap<u32, Datum>,
    library: BTreeMap<u32, String>,
}

fn hex(k: u32) -> String {
    format!("0x{k:08x}")
}

fn parse_hex(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s.trim_start_matches("0x").trim_start_matches("f_"), 16)
        .map_err(|e| format!("{s}: {e}"))
}

fn keys_value(ks: &[u32]) -> Value {
    Value::Array(ks.iter().map(|&k| Value::String(hex(k))).collect())
}

fn keys_of(v: Option<&Value>) -> Vec<u32> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().and_then(|s| parse_hex(s).ok()))
        .collect()
}

impl SampleFile {
    fn write(&self, path: &Path) -> Result<(), String> {
        let mut head = Table::new();
        head.insert("rom_sha1".into(), Value::String(self.rom_sha1.clone()));
        head.insert("seed".into(), Value::Integer(self.seed as i64));
        let mut funcs = Table::new();
        for (&k, f) in &self.funcs {
            let mut t = Table::new();
            t.insert("insns".into(), Value::Integer(f.insns as i64));
            let class = f.class.map_or("", Class::tag);
            t.insert("class".into(), Value::String(class.into()));
            t.insert("pret".into(), Value::String(f.pret.clone()));
            if let Some(n) = &f.sibling {
                t.insert("sibling".into(), Value::String(n.clone()));
            }
            t.insert("roots".into(), keys_value(&f.roots));
            t.insert("callees".into(), keys_value(&f.callees));
            t.insert("callers".into(), keys_value(&f.callers));
            t.insert("data".into(), keys_value(&f.data));
            funcs.insert(hex(k), Value::Table(t));
        }
        let mut data = Table::new();
        for (&k, d) in &self.data {
            let mut t = Table::new();
            t.insert("size".into(), Value::Integer(i64::from(d.size)));
            if let Some(p) = &d.pret {
                t.insert("pret".into(), Value::String(p.clone()));
            }
            data.insert(hex(k), Value::Table(t));
        }
        let lib: Table = self
            .library
            .iter()
            .map(|(&k, n)| (hex(k), Value::String(n.clone())))
            .collect();
        let mut doc = Table::new();
        doc.insert("sample".into(), Value::Table(head));
        doc.insert("functions".into(), Value::Table(funcs));
        doc.insert("data".into(), Value::Table(data));
        doc.insert("library".into(), Value::Table(lib));
        let text = format!(
            "# The naming loop's sample (`naming prepare`, docs/mod-view-design.md).\n\
             # `pret` is the truth `naming score` reads; no prompt shows it.\n\n{}",
            toml::to_string(&doc).map_err(|e| e.to_string())?
        );
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn read(path: &Path) -> Result<SampleFile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: Table = text
            .parse()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let tab = |k: &str| {
            doc.get(k)
                .and_then(Value::as_table)
                .cloned()
                .unwrap_or_default()
        };
        let head = tab("sample");
        let mut s = SampleFile {
            rom_sha1: head
                .get("rom_sha1")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            seed: head.get("seed").and_then(Value::as_integer).unwrap_or(0) as u64,
            ..SampleFile::default()
        };
        for (k, v) in tab("functions") {
            let st = |f: &str| v.get(f).and_then(Value::as_str).unwrap_or("").to_string();
            s.funcs.insert(
                parse_hex(&k)?,
                Fun {
                    insns: v.get("insns").and_then(Value::as_integer).unwrap_or(0) as usize,
                    class: Class::from_tag(&st("class")),
                    pret: st("pret"),
                    sibling: v.get("sibling").and_then(Value::as_str).map(String::from),
                    roots: keys_of(v.get("roots")),
                    callees: keys_of(v.get("callees")),
                    callers: keys_of(v.get("callers")),
                    data: keys_of(v.get("data")),
                },
            );
        }
        for (k, v) in tab("data") {
            s.data.insert(
                parse_hex(&k)?,
                Datum {
                    size: v.get("size").and_then(Value::as_integer).unwrap_or(1) as u32,
                    pret: v.get("pret").and_then(Value::as_str).map(String::from),
                },
            );
        }
        for (k, v) in tab("library") {
            s.library
                .insert(parse_hex(&k)?, v.as_str().unwrap_or("").into());
        }
        Ok(s)
    }
}

fn read_rom(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

/// What `prepare` and `sibling` read from a ROM and its pret map: every
/// function's size, calls, callers and pool data, the library's names, the
/// named candidates' call graph, and each candidate's masked code.
struct Scan {
    sha: String,
    map: SourceMap,
    graph: Graph,
    lib: BTreeMap<u32, String>,
    insns: BTreeMap<u32, usize>,
    calls: BTreeMap<u32, BTreeSet<u32>>,
    callers: BTreeMap<u32, BTreeSet<u32>>,
    pools: BTreeMap<u32, BTreeSet<u32>>,
    all_pool: Vec<u32>,
    code: BTreeMap<u32, Vec<u8>>,
}

fn scan(rom_path: &str, pret: &str) -> Result<Scan, String> {
    let rom = read_rom(rom_path)?;
    let sha = sha1_hex(&rom);
    let map = SourceMap::load(&sha, &[pret])?;
    let disc = discover(rom.clone());
    let ctx = Ctx::new(&rom, &disc);
    let rom_fn = |k: u32| (0x0800_0000..0x0a00_0000).contains(&k);
    let mut graph = Graph {
        insns: BTreeMap::new(),
        calls: BTreeMap::new(),
    };
    let mut lib = BTreeMap::new();
    let mut callers: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut pools: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut all_pool: BTreeSet<u32> = BTreeSet::new();
    let mut all_calls = BTreeMap::new();
    let mut insns = BTreeMap::new();
    let mut code = BTreeMap::new();
    for &k in ctx.entries.iter().filter(|&&k| rom_fn(k)) {
        let f = build_func(&ctx, k);
        let n: usize = f.blocks.values().map(|b| b.insns.len()).sum();
        let mut ps = BTreeSet::new();
        for b in f.blocks.values() {
            for i in &b.block.insts {
                if let Op::Pool(x) = i.op {
                    let rom_data = disc_rom_data(&disc, x);
                    if in_ram(x) || rom_data {
                        ps.insert(x);
                    }
                }
            }
        }
        all_pool.extend(&ps);
        pools.insert(k, ps);
        for &c in &f.calls {
            callers.entry(c).or_default().insert(k);
        }
        insns.insert(k, n);
        all_calls.insert(k, f.calls.clone());
        match map.functions.get(&k) {
            Some(it) if it.file.as_deref().is_some_and(library) => {
                lib.insert(k, it.name.clone());
            }
            Some(it) if n > 0 && it.size != Some(0) => {
                graph.insns.insert(k, n);
                code.insert(k, masked_code(&rom, &f));
            }
            _ => {}
        }
    }
    graph.calls = all_calls.clone();
    Ok(Scan {
        sha,
        map,
        graph,
        lib,
        insns,
        calls: all_calls,
        callers,
        pools,
        all_pool: all_pool.into_iter().collect(),
        code,
    })
}

/// A function's instructions in address order, as bytes with call offsets
/// masked ([`naming::mask_insn`]): the same code in another build gives the
/// same bytes.
fn masked_code(rom: &[u8], f: &codegen::func::Func) -> Vec<u8> {
    let mut at: BTreeMap<u32, bool> = BTreeMap::new();
    for b in f.blocks.values() {
        for &(a, _) in &b.insns {
            at.insert(a & !1, b.key & 1 != 0);
        }
    }
    let mut out = Vec::new();
    for (a, thumb) in at {
        let Some(o) = a.checked_sub(0x0800_0000).map(|o| o as usize) else {
            continue;
        };
        let w = if thumb { 2 } else { 4 };
        let Some(bytes) = rom.get(o..o + w) else {
            continue;
        };
        let raw = bytes
            .iter()
            .rev()
            .fold(0u32, |v, &b| (v << 8) | u32::from(b));
        out.extend_from_slice(&naming::mask_insn(raw, thumb).to_le_bytes()[..w]);
    }
    out
}

impl Scan {
    /// `k`'s entry in a sample file, its pool data added to `s.data`.
    fn fun(&self, s: &mut SampleFile, k: u32, roots: Vec<u32>) -> Fun {
        let n = self.insns[&k];
        let data: Vec<u32> = self.pools[&k].iter().copied().collect();
        for &d in &data {
            let i = self.all_pool.partition_point(|&p| p <= d);
            let size = self.all_pool.get(i).map_or(1, |&next| next - d);
            let pret = self.map.data.get(&d).map(|it| it.name.clone());
            s.data.insert(d, Datum { size, pret });
        }
        Fun {
            insns: n,
            class: Some(Class::of(n)),
            pret: self.map.functions[&k].name.clone(),
            sibling: None,
            roots,
            callees: self.calls[&k].iter().copied().collect(),
            callers: self
                .callers
                .get(&k)
                .into_iter()
                .flatten()
                .copied()
                .collect(),
            data,
        }
    }
}

fn prepare(rom_path: &str, pret: &str, out: &Path, seed: u64) -> Result<(), String> {
    let sc = scan(rom_path, pret)?;
    let plan = Plan {
        seed,
        ..Plan::default()
    };
    let sample = draw(&sc.graph, &plan);
    let mut s = SampleFile {
        rom_sha1: sc.sha.clone(),
        seed,
        library: sc.lib.clone(),
        ..SampleFile::default()
    };
    for &k in &sample.keys {
        let roots = sample
            .roots
            .iter()
            .filter(|(_, t)| t.contains(&k))
            .map(|(&r, _)| r)
            .collect();
        let f = sc.fun(&mut s, k, roots);
        s.funcs.insert(k, f);
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    s.write(&out.join("sample.toml"))?;
    let count = |c| s.funcs.values().filter(|f| f.class == Some(c)).count();
    let in_trees: BTreeSet<u32> = sample.roots.values().flatten().copied().collect();
    println!(
        "sample of {} from {} candidates (seed {seed}): {} small, {} mid, {} large; \
         {} subtrees holding {}; {} data ({} pret-named); {} library names given",
        s.funcs.len(),
        sc.graph.insns.len(),
        count(Class::Small),
        count(Class::Mid),
        count(Class::Large),
        sample.roots.len(),
        in_trees.len(),
        s.data.len(),
        s.data.values().filter(|d| d.pret.is_some()).count(),
        s.library.len(),
    );
    Ok(())
}

/// Names that say nothing: pret's placeholders.
fn placeholder(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.starts_with("sub_") || l.starts_with("nullsub") || l.starts_with("unk")
}

/// The sibling leak control (docs/mod-view-design.md, "Measuring the
/// leak"): `count` functions of ROM whose code is the same as one of
/// SIB_ROM's (masked bytes, unique in each game, at least `MIN_INSNS`
/// instructions) under a different, non-placeholder pret name; written as
/// a sample with each sibling's name beside pret's, for `ask` and `score`.
fn sibling(
    rom_path: &str,
    pret: &str,
    sib_rom: &str,
    sib_pret: &str,
    out: &Path,
    seed: u64,
    count: usize,
) -> Result<(), String> {
    const MIN_INSNS: usize = 8;
    let a = scan(rom_path, pret)?;
    let b = scan(sib_rom, sib_pret)?;
    let named = |sc: &Scan| -> BTreeMap<u32, (Vec<u8>, String)> {
        sc.code
            .iter()
            .filter(|(k, _)| sc.insns[k] >= MIN_INSNS)
            .map(|(&k, c)| (k, (c.clone(), sc.map.functions[&k].name.clone())))
            .collect()
    };
    let (na, nb) = (named(&a), named(&b));
    let all = naming::sibling_pairs(&na, &nb);
    let same = {
        let mut u: BTreeMap<&[u8], usize> = BTreeMap::new();
        for (fp, _) in na.values() {
            *u.entry(fp).or_default() += 1;
        }
        let ub: BTreeSet<&[u8]> = nb.values().map(|(fp, _)| fp.as_slice()).collect();
        u.iter()
            .filter(|(fp, n)| **n == 1 && ub.contains(*fp))
            .count()
    };
    let mut pairs: Vec<(u32, u32)> = all
        .iter()
        .copied()
        .filter(|(ka, kb)| !placeholder(&na[ka].1) && !placeholder(&nb[kb].1))
        .collect();
    naming::Rng::new(seed).shuffle(&mut pairs);
    pairs.truncate(count);
    let mut s = SampleFile {
        rom_sha1: a.sha.clone(),
        seed,
        library: a.lib.clone(),
        ..SampleFile::default()
    };
    for &(ka, kb) in &pairs {
        let mut f = a.fun(&mut s, ka, Vec::new());
        f.sibling = Some(nb[&kb].1.clone());
        s.funcs.insert(ka, f);
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    s.write(&out.join("sample.toml"))?;
    let class = |c| s.funcs.values().filter(|f| f.class == Some(c)).count();
    println!(
        "{} of {} candidates with {MIN_INSNS}+ instructions have the same code in the sibling; \
         {} of those have another name, {} not placeholders; drew {} (seed {seed}): \
         {} small, {} mid, {} large",
        same,
        na.len(),
        all.len(),
        all.iter()
            .filter(|(ka, kb)| !placeholder(&na[ka].1) && !placeholder(&nb[kb].1))
            .count(),
        s.funcs.len(),
        class(Class::Small),
        class(Class::Mid),
        class(Class::Large),
    );
    Ok(())
}

/// Whether `x` is in ROM but not code: a pointer to ROM data.
fn disc_rom_data(disc: &analysis::discovery::Discovery, x: u32) -> bool {
    (0x0800_0000..0x0a00_0000).contains(&x) && !disc.code.contains_key(&(x & !1))
}

/// The rounds `ask` knows.
const ROUNDS: [&str; 5] = ["0", "1", "2", "nocode", "recog"];

const SYSTEM: &str = "You are an expert reverse engineer naming the functions and \
    global data of a Game Boy Advance game that has no symbols, as a C decompilation \
    project would name them. Answer only with the TOML asked for, no prose.";

const RECOG_TRIALS: usize = 10;

/// What one asking session holds: the ROM's bytes, the sample, the output
/// folder its answers go to, and the model it asks (at `effort`, else the
/// model's default).
struct Asker {
    rom: Vec<u8>,
    sample: SampleFile,
    out: PathBuf,
    model: String,
    effort: Option<String>,
}

fn answer_path(out: &Path, round: &str, k: u32) -> PathBuf {
    out.join(round).join(format!("{}.toml", hex(k)))
}

/// The answers of `round` so far.
fn answers(out: &Path, round: &str) -> BTreeMap<u32, (Answer, Table)> {
    let mut m = BTreeMap::new();
    let Ok(dir) = std::fs::read_dir(out.join(round)) else {
        return m;
    };
    for e in dir.flatten() {
        let p = e.path();
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if p.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let (Ok(k), Ok(text)) = (parse_hex(stem), std::fs::read_to_string(&p)) else {
            continue;
        };
        let Ok(t) = text.parse::<Table>() else {
            continue;
        };
        let s = |f: &str| t.get(f).and_then(Value::as_str).unwrap_or("").to_string();
        let data = t
            .get("data")
            .and_then(Value::as_table)
            .into_iter()
            .flatten()
            .filter_map(|(d, n)| {
                Some((
                    parse_hex(d.trim_start_matches("d_")).ok()?,
                    n.as_str()?.into(),
                ))
            })
            .collect();
        let stored = Answer {
            name: s("name"),
            summary: s("summary"),
            data,
        };
        // The raw answer read again, so a better parser applies to old
        // answers too.
        let a = match t.get("raw").and_then(Value::as_str).map(parse_answer) {
            Some(Ok(a)) => a,
            _ => stored,
        };
        m.insert(k, (a, t));
    }
    m
}

impl Asker {
    /// The source map a print in this state uses: the library's names, the
    /// agent's function names in `names` but `own`'s, and every sampled
    /// datum as its voted name or `d_<hex>`, with our size.
    fn map(
        &self,
        names: &BTreeMap<u32, String>,
        data: &BTreeMap<u32, String>,
        own: u32,
    ) -> SourceMap {
        let funcs = self
            .sample
            .library
            .iter()
            .chain(names)
            .filter(|(k, _)| **k != own)
            .map(|(&k, n)| (k, (n.clone(), None)))
            .collect();
        let text = map_text(&self.sample, "agent", &funcs, data);
        SourceMap::from_texts(&self.sample.rom_sha1, &[&text]).expect("the agent's map loads")
    }

    fn print(&self, disc: &analysis::discovery::Discovery, k: u32, map: &SourceMap) -> String {
        let mut ctx = Ctx::new(&self.rom, disc);
        let readable = BTreeSet::from([k]);
        ctx.names = Some(map);
        ctx.readable = Some(&readable);
        let f = build_func(&ctx, k);
        let (body, _) = lower_func_checked(&ctx, &f, &ctx.fn_ident(k));
        pretty(body)
    }

    /// One function's prompt in a naming round: its print and its callers'
    /// and callees' keys, with names and summaries from `known`.
    fn prompt(
        &self,
        disc: &analysis::discovery::Discovery,
        k: u32,
        known: &BTreeMap<u32, Answer>,
        show_callers: bool,
        data: &BTreeMap<u32, String>,
    ) -> String {
        let f = &self.sample.funcs[&k];
        let names: BTreeMap<u32, String> =
            known.iter().map(|(&k, a)| (k, a.name.clone())).collect();
        let map = self.map(&names, data, k);
        let code = self.print(disc, k, &map);
        let mode = if k & 1 != 0 { "Thumb" } else { "ARM" };
        let mut p = format!(
            "Name this function, f_{k:08x} ({mode}, {} instructions), and the data it touches.\n\n\
             Its code was recompiled from ARM7TDMI machine code to Rust: each `insn!` line is one \
             guest instruction as assembly, followed by what it does; `m.bus` is memory, `r0`..`r15` \
             registers, `rt::` timing. Calls print as the callee's name if it has one, else \
             `f_<address>`; pool words pointing at data print as `addr!(name)` or `addr!(name + offset)`.\n\n",
            f.insns
        );
        let line = |p: &mut String, c: u32| {
            let lib = self.sample.library.get(&c);
            match (known.get(&c), lib) {
                (_, Some(n)) => {
                    let _ = writeln!(p, "- f_{c:08x}: {n} (a standard library routine)");
                }
                (Some(a), _) if !a.name.is_empty() => {
                    let _ = writeln!(p, "- f_{c:08x}: {}: {}", a.name, a.summary);
                }
                _ => {
                    let _ = writeln!(p, "- f_{c:08x} (not named yet)");
                }
            }
        };
        let list = |p: &mut String, title: &str, ks: &[u32], named: bool| {
            if ks.is_empty() {
                return;
            }
            let _ = writeln!(p, "{title}:");
            let mut ks: Vec<u32> = ks.to_vec();
            ks.sort_by_key(|c| (!known.contains_key(c), *c));
            for &c in ks.iter().take(15) {
                if named {
                    line(p, c);
                } else {
                    let _ = writeln!(p, "- f_{c:08x}");
                }
            }
            if ks.len() > 15 {
                let _ = writeln!(p, "- and {} more", ks.len() - 15);
            }
            p.push('\n');
        };
        list(&mut p, "It calls", &f.callees, true);
        list(&mut p, "It is called by", &f.callers, show_callers);
        if !f.data.is_empty() {
            p.push_str("Data its pool words point at (size: the gap to the next address any code's pool points at):\n");
            for &d in &f.data {
                let where_ = match d >> 24 {
                    2 => "EWRAM",
                    3 => "IWRAM",
                    _ => "ROM",
                };
                let size = self.sample.data.get(&d).map_or(1, |x| x.size);
                let shown = data
                    .get(&d)
                    .map_or(format!("d_{d:08x}"), |n| format!("{n} (d_{d:08x})"));
                let _ = writeln!(p, "- {shown}: {where_}, up to {size} bytes");
            }
            p.push('\n');
        }
        let _ = write!(
            p,
            "```rust\n{code}```\n\n\
             Answer with TOML only, in this form (functions PascalCase, globals gCamelCase, \
             file-local statics sCamelCase; one entry under [data] for each datum above, keyed \
             by its d_ address):\n\n\
             name = \"FunctionName\"\nsummary = \"one line: what it does\"\n[data]\n"
        );
        for &d in &f.data {
            let _ = writeln!(p, "d_{d:08x} = \"gDataName\"");
        }
        p
    }

    fn nocode_prompt(&self, k: u32) -> String {
        let f = &self.sample.funcs[&k];
        format!(
            "Pokémon Emerald (U), function at 0x{k:08x}, {} instructions. What is it called in \
             pret's pokeemerald decompilation?\n\nAnswer with TOML only:\n\n\
             name = \"FunctionName\"\nsummary = \"one line: what it does\"\n",
            f.insns
        )
    }

    /// Trial `t` of the recognition control: three sampled functions'
    /// round-0 prints (small and mid only, to keep it short).
    fn recog_prompt(&self, disc: &analysis::discovery::Discovery, t: usize) -> (u32, String) {
        let mut pool: Vec<u32> = self
            .sample
            .funcs
            .iter()
            .filter(|(_, f)| f.class != Some(Class::Large))
            .map(|(&k, _)| k)
            .collect();
        let mut rng = naming::Rng::new(self.sample.seed ^ (t as u64 + 1));
        rng.shuffle(&mut pool);
        let map = self.map(&BTreeMap::new(), &BTreeMap::new(), 0);
        let mut p = String::from(
            "Here are three functions from one Game Boy Advance game, recompiled from ARM7TDMI \
             machine code to Rust (each `insn!` line is one guest instruction). Which game is it?\n\n",
        );
        for &k in pool.iter().take(3) {
            let _ = write!(p, "```rust\n{}```\n\n", self.print(disc, k, &map));
        }
        p.push_str(
            "Answer with TOML only:\n\ngame = \"title, or unknown\"\nconfidence = 0.5\nwhy = \"one line\"\n",
        );
        (t as u32, p)
    }
}

/// A source map's text: `funcs` (each name with an optional summary) and
/// every sampled datum as its name in `data` or `d_<hex>`, with our size.
/// A name given twice gets `_2`, `_3`, … so that the map loads.
fn map_text(
    sample: &SampleFile,
    source: &str,
    funcs: &BTreeMap<u32, (String, Option<String>)>,
    data: &BTreeMap<u32, String>,
) -> String {
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut unique = |n: &str| {
        let mut id = n.to_string();
        let mut i = 2;
        while !used.insert(id.to_lowercase()) {
            id = format!("{n}_{i}");
            i += 1;
        }
        Value::String(id).to_string()
    };
    let q = |s: &str| Value::String(s.into()).to_string();
    let mut text = format!(
        "[map]\nrom_sha1 = \"{}\"\nsource = {}\nformat = 1\n\n[functions]\n",
        sample.rom_sha1,
        q(source)
    );
    for (&k, (n, summary)) in funcs {
        if n.is_empty() {
            continue;
        }
        let summary = summary
            .as_deref()
            .map_or(String::new(), |s| format!(", summary = {}", q(s)));
        let _ = writeln!(text, "0x{k:08x} = {{ name = {}{summary} }}", unique(n));
    }
    text.push_str("\n[data]\n");
    for (&d, it) in &sample.data {
        let n = data
            .get(&d)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or(format!("d_{d:08x}"));
        let _ = writeln!(
            text,
            "0x{d:08x} = {{ name = {}, size = {} }}",
            unique(&n),
            it.size.max(1)
        );
    }
    text
}

/// Asks the agent `prompt` and returns its answer file's table, or why
/// the call failed (no file is written then, so a rerun asks again).
fn call_agent(
    model: &str,
    effort: Option<&str>,
    prompt: &str,
    recog: bool,
) -> Result<Table, String> {
    let (result, mut t) = naming::claude(model, effort, SYSTEM, prompt)?;
    if recog {
        match toml::from_str::<Table>(result.trim().trim_matches('`').trim_start_matches("toml")) {
            Ok(a) => {
                for f in ["game", "why"] {
                    let v = a.get(f).and_then(Value::as_str).unwrap_or("");
                    t.insert(f.into(), Value::String(v.into()));
                }
            }
            Err(e) => {
                t.insert("error".into(), Value::String(format!("{e}")));
            }
        }
    } else {
        match parse_answer(&result) {
            Ok(a) => {
                t.insert("name".into(), Value::String(a.name));
                t.insert("summary".into(), Value::String(a.summary));
                let d: Table = a
                    .data
                    .iter()
                    .map(|(&d, n)| (format!("d_{d:08x}"), Value::String(n.clone())))
                    .collect();
                t.insert("data".into(), Value::Table(d));
            }
            Err(e) => {
                t.insert("error".into(), Value::String(e));
            }
        }
    }
    t.insert("raw".into(), Value::String(result));
    Ok(t)
}

/// Asks every prompt, `jobs` at a time, writing each answer as it comes.
fn ask_all(
    model: &str,
    effort: Option<&str>,
    out: &Path,
    round: &str,
    prompts: Vec<(u32, String)>,
    jobs: usize,
) -> usize {
    let todo = Mutex::new(prompts.into_iter());
    let failed = Mutex::new(0);
    let dir = out.join(round);
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let next = todo.lock().unwrap().next();
                    let Some((k, prompt)) = next else { break };
                    let _ = std::fs::write(dir.join(format!("{}.prompt", hex(k))), &prompt);
                    match call_agent(model, effort, &prompt, round == "recog") {
                        Ok(mut t) => {
                            t.insert("key".into(), Value::String(hex(k)));
                            t.insert("round".into(), Value::String(round.into()));
                            let text = toml::to_string(&t).unwrap();
                            let _ = std::fs::write(answer_path(out, round, k), text);
                            let name = t.get("name").and_then(Value::as_str).unwrap_or("?");
                            eprintln!("{round} {}: {name}", hex(k));
                        }
                        Err(e) => {
                            *failed.lock().unwrap() += 1;
                            eprintln!("{round} {}: failed: {e}", hex(k));
                        }
                    }
                }
            });
        }
    });
    failed.into_inner().unwrap()
}

struct AskOpts {
    jobs: usize,
    model: String,
    effort: Option<String>,
    limit: Option<usize>,
}

fn ask(rom_path: &str, out: &Path, round: &str, o: &AskOpts) -> Result<(), String> {
    if !ROUNDS.contains(&round) {
        return Err(format!("round {round}: one of {ROUNDS:?}"));
    }
    let sample = SampleFile::read(&out.join("sample.toml"))?;
    let rom = read_rom(rom_path)?;
    if sha1_hex(&rom) != sample.rom_sha1 {
        return Err(format!("{rom_path} isn't the sample's ROM"));
    }
    std::fs::create_dir_all(out.join(round)).map_err(|e| e.to_string())?;
    let disc = discover(rom.clone());
    let a = Asker {
        rom,
        sample,
        out: out.to_path_buf(),
        model: o.model.clone(),
        effort: o.effort.clone(),
    };
    let keys: BTreeSet<u32> = a.sample.funcs.keys().copied().collect();
    let mut keys_limited: BTreeSet<u32> = keys.clone();
    if let Some(n) = o.limit {
        // A trial run: the first n of the bottom-up order, so a limited
        // round 1 still sees callees named.
        let calls: BTreeMap<u32, BTreeSet<u32>> = a
            .sample
            .funcs
            .iter()
            .map(|(&k, f)| (k, f.callees.iter().copied().collect()))
            .collect();
        keys_limited = levels(&keys, &calls).concat().into_iter().take(n).collect();
    }
    let done = answers(&a.out, round);
    let mut failed = 0;
    match round {
        "nocode" => {
            let ps = keys_limited
                .iter()
                .filter(|k| !done.contains_key(k))
                .map(|&k| (k, a.nocode_prompt(k)))
                .collect();
            failed += ask_all(&a.model, a.effort.as_deref(), &a.out, round, ps, o.jobs);
        }
        "recog" => {
            let ps = (0..RECOG_TRIALS)
                .filter(|&t| !done.contains_key(&(t as u32)))
                .map(|t| a.recog_prompt(&disc, t))
                .collect();
            failed += ask_all(&a.model, a.effort.as_deref(), &a.out, round, ps, o.jobs);
        }
        "0" => {
            let ps = keys_limited
                .iter()
                .filter(|k| !done.contains_key(k))
                .map(|&k| {
                    (
                        k,
                        a.prompt(&disc, k, &BTreeMap::new(), false, &BTreeMap::new()),
                    )
                })
                .collect();
            failed += ask_all(&a.model, a.effort.as_deref(), &a.out, round, ps, o.jobs);
        }
        _ => {
            // Rounds 1 and 2: bottom-up by level. Round 1 sees its callees'
            // round-1 names and round 0's data vote; round 2 every sampled
            // neighbour's latest name (round 2's, else round 1's) and round
            // 1's data vote.
            let prev = if round == "1" { "0" } else { "1" };
            let prev_answers = answers(&a.out, prev);
            if prev_answers.is_empty() {
                return Err(format!("round {round} needs round {prev}'s answers"));
            }
            let data = vote(prev_answers.values().map(|(a, _)| a));
            let calls: BTreeMap<u32, BTreeSet<u32>> = a
                .sample
                .funcs
                .iter()
                .map(|(&k, f)| (k, f.callees.iter().copied().collect()))
                .collect();
            for level in levels(&keys, &calls) {
                let mut known: BTreeMap<u32, Answer> = BTreeMap::new();
                if round == "2" {
                    known.extend(prev_answers.iter().map(|(&k, (a, _))| (k, a.clone())));
                }
                known.extend(answers(&a.out, round).into_iter().map(|(k, (a, _))| (k, a)));
                let ps: Vec<(u32, String)> = level
                    .iter()
                    .filter(|k| keys_limited.contains(k) && !known_in_round(&a.out, round, **k))
                    .map(|&k| (k, a.prompt(&disc, k, &known, round == "2", &data)))
                    .collect();
                if ps.is_empty() {
                    continue;
                }
                failed += ask_all(&a.model, a.effort.as_deref(), &a.out, round, ps, o.jobs);
            }
        }
    }
    let have = answers(&a.out, round).len();
    println!(
        "round {round}: {have} answers in {}, {failed} calls failed",
        out.join(round).display()
    );
    if failed > 0 {
        return Err(format!("{failed} calls failed; rerun to ask them again"));
    }
    Ok(())
}

fn known_in_round(out: &Path, round: &str, k: u32) -> bool {
    answer_path(out, round, k).exists()
}

/// The cost of a round's answers: calls, mean tokens in (all input,
/// cached included) and out, mean seconds, and dollars in all.
fn cost(ans: &BTreeMap<u32, (Answer, Table)>) -> (usize, f64, f64, f64, f64) {
    let n = ans.len().max(1) as f64;
    let f = |t: &Table, k: &str| match t.get(k) {
        Some(Value::Integer(i)) => *i as f64,
        Some(Value::Float(x)) => *x,
        _ => 0.0,
    };
    let sum = |g: &dyn Fn(&Table) -> f64| ans.values().map(|(_, t)| g(t)).sum::<f64>();
    let tin =
        sum(&|t| f(t, "input_tokens") + f(t, "cache_creation_tokens") + f(t, "cache_read_tokens"));
    (
        ans.len(),
        tin / n,
        sum(&|t| f(t, "output_tokens")) / n,
        sum(&|t| f(t, "seconds")) / n,
        sum(&|t| f(t, "cost_usd")),
    )
}

fn score(out: &Path) -> Result<(), String> {
    let s = SampleFile::read(&out.join("sample.toml"))?;
    let mut r = String::new();
    let _ = writeln!(
        r,
        "# Naming sample: agents' names against pret's\n\n\
         Written by `naming score` (crates/naming) from `{}`. Seed {}, {} functions \
         (docs/mod-view-design.md, \"The naming loop\").\n",
        out.display(),
        s.seed,
        s.funcs.len()
    );
    let rounds = ["0", "1", "2", "nocode"];
    let all: BTreeMap<&str, BTreeMap<u32, (Answer, Table)>> =
        rounds.iter().map(|&rd| (rd, answers(out, rd))).collect();
    let _ = writeln!(
        r,
        "## Functions\n\nSubtoken precision, recall and F1 (macro, per function; a missing or \
         unparsed answer scores 0) and exact match, case-insensitive. REBENCH's leak-free \
         token F1, 0.1-0.4, is the lower reference.\n\n\
         | round | class | n | precision | recall | F1 | exact |\n|---|---|---|---|---|---|---|"
    );
    for rd in rounds {
        let ans = &all[rd];
        if ans.is_empty() {
            continue;
        }
        let mut by: BTreeMap<Option<Class>, Score> = BTreeMap::new();
        for (k, f) in &s.funcs {
            let pred = ans.get(k).map_or("", |(a, _)| a.name.as_str());
            let truth = subtokens(&f.pret);
            for c in [f.class, None] {
                by.entry(c)
                    .or_default()
                    .add(&subtokens(pred), &truth, exact(pred, &f.pret));
            }
        }
        for (c, sc) in &by {
            let _ = writeln!(
                r,
                "| {rd} | {} | {} | {:.3} | {:.3} | {:.3} | {:.1}% |",
                c.map_or("all", Class::label),
                sc.n,
                sc.precision(),
                sc.recall(),
                sc.f1(),
                100.0 * sc.exact_share()
            );
        }
    }
    if s.funcs.values().any(|f| f.sibling.is_some()) {
        sibling_report(&mut r, &s, &all);
    }
    // The bottom-up gain: functions in a subtree with sampled callees.
    let with_callees: Vec<u32> = s
        .funcs
        .iter()
        .filter(|(_, f)| !f.roots.is_empty() && f.callees.iter().any(|c| s.funcs.contains_key(c)))
        .map(|(&k, _)| k)
        .collect();
    let _ = writeln!(
        r,
        "\n### Subtree functions with sampled callees ({})\n\nWhere bottom-up rounds can pass \
         names upward.\n\n| round | F1 | exact |\n|---|---|---|",
        with_callees.len()
    );
    for rd in ["0", "1", "2"] {
        let ans = &all[rd];
        if ans.is_empty() {
            continue;
        }
        let mut sc = Score::default();
        for k in &with_callees {
            let pret = &s.funcs[k].pret;
            let pred = ans.get(k).map_or("", |(a, _)| a.name.as_str());
            sc.add(&subtokens(pred), &subtokens(pret), exact(pred, pret));
        }
        let _ = writeln!(
            r,
            "| {rd} | {:.3} | {:.1}% |",
            sc.f1(),
            100.0 * sc.exact_share()
        );
    }
    let _ = writeln!(
        r,
        "\n## Data\n\nEach datum's name is the vote of the round's answers that touch it; scored \
         where pret names that exact address, scope prefixes (g, s) dropped.\n\n\
         | round | n | precision | recall | F1 | exact |\n|---|---|---|---|---|---|"
    );
    for rd in ["0", "1", "2"] {
        let ans = &all[rd];
        if ans.is_empty() {
            continue;
        }
        let v = vote(ans.values().map(|(a, _)| a));
        let mut sc = Score::default();
        for (d, it) in &s.data {
            let Some(pret) = &it.pret else { continue };
            let pred = v.get(d).map_or("", String::as_str);
            sc.add(
                &data_subtokens(pred),
                &data_subtokens(pret),
                exact(pred, pret),
            );
        }
        let _ = writeln!(
            r,
            "| {rd} | {} | {:.3} | {:.3} | {:.3} | {:.1}% |",
            sc.n,
            sc.precision(),
            sc.recall(),
            sc.f1(),
            100.0 * sc.exact_share()
        );
    }
    let _ = writeln!(
        r,
        "\n## Cost\n\nPer call, from each call's usage; tokens in count the cached prompt.\n\n\
         | round | calls | tokens in | tokens out | seconds | dollars (list) |\n|---|---|---|---|---|---|"
    );
    let recog = answers(out, "recog");
    for (rd, ans) in all.iter().map(|(k, v)| (*k, v)).chain([("recog", &recog)]) {
        if ans.is_empty() {
            continue;
        }
        let (n, tin, tout, secs, usd) = cost(ans);
        let _ = writeln!(
            r,
            "| {rd} | {n} | {tin:.0} | {tout:.0} | {secs:.1} | {usd:.2} |"
        );
    }
    let unparsed: Vec<String> = all
        .iter()
        .filter(|(_, a)| !a.is_empty())
        .map(|(rd, a)| {
            format!(
                "{rd}: {}",
                a.values().filter(|(a, _)| a.name.is_empty()).count()
            )
        })
        .collect();
    let _ = writeln!(r, "\nAnswers that didn't parse: {}.", unparsed.join(", "));
    if !recog.is_empty() {
        let _ = writeln!(
            r,
            "\n## Recognition control\n\nThree round-0 prints a trial; which game?\n\n| trial | game | why |\n|---|---|---|"
        );
        for (t, (_, tb)) in &recog {
            let g = |f: &str| {
                tb.get(f)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .replace('|', "/")
            };
            let _ = writeln!(r, "| {t} | {} | {} |", g("game"), g("why"));
        }
    }
    let _ = writeln!(
        r,
        "\n## Side by side\n\n20 functions, drawn with the sample's seed.\n\n\
         | key | class | pret | round 0 | round 1 | round 2 | no code |\n|---|---|---|---|---|---|---|"
    );
    let mut ks: Vec<u32> = s.funcs.keys().copied().collect();
    naming::Rng::new(s.seed ^ 0x5eed).shuffle(&mut ks);
    ks.truncate(20);
    ks.sort();
    for k in ks {
        let f = &s.funcs[&k];
        let n = |rd: &str| {
            all[rd]
                .get(&k)
                .map_or("-".to_string(), |(a, _)| a.name.clone())
        };
        let _ = writeln!(
            r,
            "| {} | {} | {} | {} | {} | {} | {} |",
            hex(k),
            f.class.map_or("", Class::tag),
            f.pret,
            n("0"),
            n("1"),
            n("2"),
            n("nocode")
        );
    }
    // The agent's map (the design's `names.toml`): each function's latest
    // round's name, and that round's data vote.
    if let Some(rd) = ["2", "1", "0"].into_iter().find(|rd| !all[rd].is_empty()) {
        let funcs = all[rd]
            .iter()
            .map(|(&k, (a, _))| (k, (a.name.clone(), Some(a.summary.clone()))))
            .collect();
        let data = vote(all[rd].values().map(|(a, _)| a));
        let run = out
            .file_name()
            .map_or("run".into(), |n| n.to_string_lossy());
        let text = format!(
            "# The agent's names for the naming sample (round {rd}), from `naming score`.\n\
             # Scored only; never a build's source map (docs/mod-view-design.md).\n\n{}",
            map_text(&s, &format!("agent:{run}"), &funcs, &data)
        );
        SourceMap::from_texts(&s.rom_sha1, &[&text])?;
        let p = out.join("names.toml");
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let path = out.join("report.md");
    std::fs::write(&path, &r).map_err(|e| format!("{}: {e}", path.display()))?;
    print!("{r}");
    Ok(())
}

/// The sibling control's part of the report: how often an answer is the
/// other game's name for the same code, against how often it is this
/// game's.
fn sibling_report(
    r: &mut String,
    s: &SampleFile,
    all: &BTreeMap<&str, BTreeMap<u32, (Answer, Table)>>,
) {
    let _ = writeln!(
        r,
        "\n## Sibling control\n\nEach function's code is the same as a function of the sibling \
         game, under another pret name. An answer with the sibling's name is remembered, not \
         read. F1 is subtoken F1 against each name.\n\n\
         | round | n | sibling's name | own name | F1 own | F1 sibling | closer to sibling |\n\
         |---|---|---|---|---|---|---|"
    );
    for (rd, ans) in all.iter().filter(|(_, a)| !a.is_empty()) {
        let (mut own, mut sib) = (Score::default(), Score::default());
        let mut closer = 0;
        for (k, f) in &s.funcs {
            let Some(sn) = &f.sibling else { continue };
            let pred = ans.get(k).map_or("", |(a, _)| a.name.as_str());
            let p = subtokens(pred);
            let mut o1 = Score::default();
            o1.add(&p, &subtokens(&f.pret), false);
            let mut s1 = Score::default();
            s1.add(&p, &subtokens(sn), false);
            closer += usize::from(s1.f1() > o1.f1());
            own.add(&p, &subtokens(&f.pret), exact(pred, &f.pret));
            sib.add(&p, &subtokens(sn), exact(pred, sn));
        }
        let _ = writeln!(
            r,
            "| {rd} | {} | {} | {} | {:.3} | {:.3} | {} |",
            own.n,
            sib.exact,
            own.exact,
            own.f1(),
            sib.f1(),
            closer
        );
    }
    let _ = writeln!(
        r,
        "\n| key | class | own (pret) | sibling (pret) | round 0 |\n|---|---|---|---|---|"
    );
    for (k, f) in &s.funcs {
        let n0 = all
            .get("0")
            .and_then(|a| a.get(k))
            .map_or("-".to_string(), |(a, _)| a.name.clone());
        let _ = writeln!(
            r,
            "| {} | {} | {} | {} | {n0} |",
            hex(*k),
            f.class.map_or("", Class::tag),
            f.pret,
            f.sibling.as_deref().unwrap_or("")
        );
    }
}

const USAGE: &str = "usage: naming prepare ROM PRET OUT [--seed N] | \
    naming sibling ROM PRET SIB_ROM SIB_PRET OUT [--seed N] [--count N] | \
    naming ask ROM OUT ROUND [--jobs N] [--model M] [--effort E] [--limit N] | naming score OUT";

fn run(args: &[String]) -> Result<(), String> {
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let num = |name: &str| -> Result<Option<u64>, String> {
        flag(name)
            .map(|v| v.parse().map_err(|e| format!("{name} {v}: {e}")))
            .transpose()
    };
    match args {
        [c, rom, pret, out, ..] if c == "prepare" => prepare(
            rom,
            pret,
            Path::new(out),
            num("--seed")?.unwrap_or(Plan::default().seed),
        ),
        [c, rom, pret, sib_rom, sib_pret, out, ..] if c == "sibling" => sibling(
            rom,
            pret,
            sib_rom,
            sib_pret,
            Path::new(out),
            num("--seed")?.unwrap_or(Plan::default().seed),
            num("--count")?.unwrap_or(30) as usize,
        ),
        [c, rom, out, round, ..] if c == "ask" => {
            let o = AskOpts {
                jobs: num("--jobs")?.unwrap_or(6) as usize,
                model: flag("--model").unwrap_or("claude-sonnet-5-5".into()),
                effort: flag("--effort"),
                limit: num("--limit")?.map(|n| n as usize),
            };
            ask(rom, Path::new(out), round, &o)
        }
        [c, out] if c == "score" => score(Path::new(out)),
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("naming: {e}");
            ExitCode::FAILURE
        }
    }
}
