//! `xmatch score A.gba A.toml B.gba B.toml [SHOW]`: matches every function
//! of ROM B to one of ROM A's (or to none) with names hidden, then scores
//! the pairs against both source maps: by stage, by size, and at other
//! bars, with up to SHOW wrong pairs.
//!
//! `xmatch name [--library] [--frames N] A.gba A.toml [A2.gba A2.toml…]
//! B.gba OUT.toml`: matches B (a ROM with no symbols) to each reference and
//! writes the names they give, each function's strongest claim, as a source
//! map for B; with `--frames N`, runs B that long from reset in the
//! interpreter and lists the functions entered, named or not.
//!
//! `xmatch matrix [--library] ROM MAP ROM MAP…`: every ROM's names scored
//! on every other one.
//!
//! `xmatch library-names ROMS NAMES A.gba A.toml [A2.gba A2.toml…]`: for
//! every ROM in the folder ROMS that no full map in NAMES names (one from
//! symbols, not one xmatch wrote), `name --library` from all the
//! references, written to NAMES/<rom>-library.toml, where codegen finds it
//! by the ROM's sha1 (`codegen::names::files_for`).
//!
//! `xmatch judge OUT ROM MAP ROM MAP… [--limit N] [--model M] [--effort E]
//! [--jobs N] [--gap N] [--min S] [--seed N]`: a model judges the library functions
//! `--library` leaves unnamed (`judge.rs`).
//!
//! `--library` cuts each reference to its library functions
//! (`libraries.toml`); `--no-spread` leaves out the call-graph rounds.

use analysis::discovery::discover;
use analysis::ir::{BinOp, Block, Exit, Op, ValueId};
use armv4t::sem::{Reg, Width};
use codegen::func::{Ctx, Func, Plan, build_func, exits, fold};
use codegen::names::{SourceMap, sha1_hex};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::process::ExitCode;
use xmatch::{Config, Fun, Matcher, Pair, Region, Score, Side, Stage, Tok, Verdict, class, score};

mod judge;

const ROM_BASE: u32 = 0x0800_0000;

fn word_at(rom: &[u8], a: u32) -> Option<u32> {
    let o = a.checked_sub(ROM_BASE)? as usize;
    let b = rom.get(o..o + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The first bytes of ROM data at `a`, to the first 0xFF (Pokémon's string
/// end) or 16 bytes, with aligned words that look like pointers replaced
/// by their region: the data's contents, which survive a relink.
fn data_hash(rom: &[u8], a: u32) -> Option<u64> {
    let o = a.checked_sub(ROM_BASE)? as usize;
    let bytes = rom.get(o..(o + 16).min(rom.len()))?;
    let n = bytes
        .iter()
        .position(|&b| b == 0xff)
        .map_or(bytes.len(), |i| i + 1);
    let mut h = DefaultHasher::new();
    let mut i = 0;
    while i < n {
        if (o + i).is_multiple_of(4)
            && i + 4 <= n
            && let Some(r) = Region::of(word_at(rom, a + i as u32)?)
        {
            (0xffu8, r).hash(&mut h);
            i += 4;
        } else {
            bytes[i].hash(&mut h);
            i += 1;
        }
    }
    n.hash(&mut h);
    Some(h.finish())
}

/// The bytes of one instruction, with what moves when code moves masked:
/// call offsets and PC-relative load and address offsets.
fn masked(rom: &[u8], at: u32, thumb: bool) -> u32 {
    let o = (at - ROM_BASE) as usize;
    if thumb {
        let h = u16::from_le_bytes([rom[o], rom[o + 1]]) as u32;
        match h >> 11 {
            0x1e | 0x1f => 0xf000,     // bl, both halves
            0x09 | 0x14 => h & 0xff00, // ldr rd, [pc, #]; add rd, pc, #
            _ => h,
        }
    } else {
        let w = word_at(rom, at).unwrap_or(0);
        if w & 0x0f00_0000 == 0x0b00_0000 {
            w & 0xff00_0000
        } else if w & 0x0f7f_0000 == 0x051f_0000 || w & 0x0fff_0000 == 0x028f_0000 {
            w & 0xffff_f000
        } else {
            w
        }
    }
}

fn width(w: Width) -> u8 {
    match w {
        Width::Byte => 1,
        Width::Half => 2,
        Width::Word => 4,
    }
}

/// The region a load or store address is based in, and the address if it
/// is a constant.
fn base(b: &Block, v: ValueId) -> (Region, Option<u32>) {
    if let Some(c) = fold(b, v) {
        return (Region::of(c).unwrap_or(Region::Other), Some(c));
    }
    match b.insts[v.index()].op {
        Op::Bin(BinOp::Add | BinOp::Sub, x, y) => {
            let r = fold(b, x).or_else(|| fold(b, y)).and_then(Region::of);
            match (r, b.insts[x.index()].op) {
                (Some(r), _) => (r, None),
                (None, Op::GetReg(Reg::SP)) => (Region::Stack, None),
                _ => (Region::Other, None),
            }
        }
        Op::GetReg(Reg::SP) => (Region::Stack, None),
        _ => (Region::Other, None),
    }
}

/// `f`'s fingerprint.
fn fingerprint(ctx: &Ctx, rom: &[u8], f: &Func) -> Fun {
    let mut fun = Fun {
        key: f.entry,
        ..Fun::default()
    };
    let add = |t: &mut BTreeMap<Tok, u32>, k: Tok| *t.entry(k).or_insert(0) += 1;
    let mut h = DefaultHasher::new();
    let mut toks = BTreeMap::new();
    for gb in f.blocks.values() {
        let b = &gb.block;
        fun.insns += gb.insns.len();
        for &(at, _) in &gb.insns {
            masked(rom, at, b.thumb).hash(&mut h);
        }
        for inst in &b.insts {
            match inst.op {
                Op::Pool(v) | Op::Const(v) => {
                    let pool = matches!(inst.op, Op::Pool(_));
                    match Region::of(v) {
                        Some(Region::Hw) => add(&mut toks, Tok::Hw(v)),
                        Some(Region::Rom) if pool => {
                            if ctx.entries.contains(&v) {
                                fun.refs.insert(v);
                            } else if !ctx.is_code(v)
                                && let Some(d) = data_hash(rom, v)
                            {
                                add(&mut toks, Tok::Data(d));
                            }
                        }
                        // Branch targets and return addresses.
                        Some(Region::Rom) => {}
                        Some(Region::Ewram | Region::Iwram) => {
                            if pool {
                                fun.ram.insert(v);
                            }
                        }
                        _ if v >= 16 => add(&mut toks, Tok::Const(v)),
                        _ => {}
                    }
                }
                Op::Load { width: w, addr } | Op::Store { width: w, addr, .. } => {
                    let store = matches!(inst.op, Op::Store { .. });
                    let (r, exact) = base(b, addr);
                    add(&mut toks, Tok::Mem(store, width(w), r));
                    if let (Region::Hw, Some(a)) = (r, exact) {
                        add(&mut toks, Tok::HwAccess(store, width(w), a));
                    }
                }
                Op::Exit(Exit::Swi { comment, .. }) => add(&mut toks, Tok::Swi(comment & 0xff)),
                _ => {}
            }
        }
        for x in exits(ctx, gb) {
            match x.plan {
                Plan::Call { target, .. } => {
                    fun.calls.insert(target);
                }
                Plan::Static(t)
                    if t != f.entry && !f.blocks.contains_key(&t) && ctx.entries.contains(&t) =>
                {
                    fun.calls.insert(t);
                }
                Plan::CallVia { .. } | Plan::Dynamic { ret: Some(_), .. } => {
                    add(&mut toks, Tok::Indirect)
                }
                _ => {}
            }
        }
    }
    let n = fun.calls.len();
    toks.insert(
        Tok::Calls(match n {
            0 => 0,
            1 => 1,
            2..=3 => 2,
            4..=7 => 3,
            _ => 4,
        }),
        1,
    );
    fun.bytes = h.finish();
    fun.toks = toks;
    fun
}

#[derive(Clone)]
struct Rom {
    side: Side,
    /// Each function's (name, file) from the source map, if it has one.
    names: Vec<Option<(String, Option<String>)>>,
    sha: String,
    source: String,
    /// The ROM's path, and its map's if it has one.
    path: String,
    map: Option<String>,
}

fn load(rom_path: &str, map: Option<&str>) -> Result<Rom, String> {
    let rom = std::fs::read(rom_path).map_err(|e| format!("{rom_path}: {e}"))?;
    let sha = sha1_hex(&rom);
    let m = match map {
        Some(p) => Some(SourceMap::load(&sha, &[p])?),
        None => None,
    };
    let disc = discover(rom.clone());
    let ctx = Ctx::new(&rom, &disc);
    let keys: Vec<u32> = ctx
        .entries
        .iter()
        .copied()
        .filter(|&k| k >= ROM_BASE)
        .collect();
    let funs: Vec<Fun> = keys
        .iter()
        .map(|&k| fingerprint(&ctx, &rom, &build_func(&ctx, k)))
        .collect();
    let names = keys
        .iter()
        .map(|k| {
            let i = m.as_ref()?.functions.get(k)?;
            Some((i.name.clone(), i.file.clone()))
        })
        .collect();
    let source = map.unwrap_or(rom_path).to_string();
    Ok(Rom {
        side: Side::new(funs),
        names,
        sha,
        source,
        path: rom_path.to_string(),
        map: map.map(str::to_string),
    })
}

/// Each B function's counterpart in A by name: the same name, and the same
/// file where a name is on more than one function.
/// A decomp's placeholder for a function it hasn't named
/// (`sub_08001234`, `nullsub_12`): it says nothing in another game.
fn placeholder(name: &str) -> bool {
    [
        "sub_", "nullsub_", "unk_", "Unk_", "func_", "FUN_", "thunk_",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// A name as truth compares it: decomps spell some library names in
/// another case (`LZ77UncompVram`), so case doesn't count. Underscores do:
/// `umul3232H32` and `__umul3232H32` are a stub and its body.
fn norm(name: &str) -> String {
    name.to_lowercase()
}

/// Each B function's counterpart in A by name: the same name, and the same
/// file where a name is on more than one function. Placeholders have none.
fn truth(a: &Rom, b: &Rom) -> BTreeMap<usize, usize> {
    fn index(r: &Rom) -> BTreeMap<String, Vec<usize>> {
        let mut by: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, n) in r.names.iter().enumerate() {
            if let Some((n, _)) = n
                && !placeholder(n)
            {
                by.entry(norm(n)).or_default().push(i);
            }
        }
        by
    }
    let (ia, ib) = (index(a), index(b));
    let mut out = BTreeMap::new();
    for (n, ys) in &ib {
        let Some(xs) = ia.get(n) else { continue };
        if let ([x], [y]) = (xs.as_slice(), ys.as_slice()) {
            out.insert(*y, *x);
            continue;
        }
        for &y in ys {
            let fy = &b.names[y].as_ref().unwrap().1;
            let same: Vec<usize> = xs
                .iter()
                .copied()
                .filter(|&x| &a.names[x].as_ref().unwrap().1 == fy)
                .collect();
            if let [x] = same[..] {
                out.insert(y, x);
            }
        }
    }
    out
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        "-".into()
    } else {
        format!("{:.1}%", 100.0 * n as f64 / d as f64)
    }
}

fn table(label: &str, s: &Score) {
    let mut cells = Vec::new();
    for c in 0..3 {
        cells.push(format!(
            "{} / {}",
            pct(s.right[c], s.judged(c)),
            pct(s.right[c], s.matchable[c]),
        ));
    }
    println!(
        "| {label} | {} | {} | {} | {} | {} | {} | {} |",
        pct(Score::total(&s.right), s.judged_all()),
        pct(
            Score::total(&s.right) + Score::total(&s.partial),
            s.judged_all()
        ),
        pct(Score::total(&s.right), Score::total(&s.matchable)),
        Score::total(&s.made),
        cells[0],
        cells[1],
        cells[2]
    );
}

/// What pair (`x`, `y`) is: see [`Verdict`].
fn judge(
    a: &Rom,
    b: &Rom,
    t: &BTreeMap<usize, usize>,
    inb: &BTreeSet<usize>,
    x: usize,
    y: usize,
) -> Verdict {
    if t.get(&y) == Some(&x) {
        return Verdict::Right;
    }
    let (na, nb) = (&a.names[x], &b.names[y]);
    let blank =
        |n: &Option<(String, Option<String>)>| n.as_ref().is_none_or(|(n, _)| placeholder(n));
    // Nothing to judge: either side has no name, or a placeholder (a
    // decomp's unnamed function may well be the library one).
    if blank(na) || blank(nb) {
        return Verdict::Unnamed;
    }
    if let (Some((na, _)), Some((nb, _))) = (na, nb)
        && let Some((base, _)) = nb.split_once('.')
        && base == na
    {
        return Verdict::Partial;
    }
    // A's x was inlined into y's counterpart: it has none of its own in B.
    if !inb.contains(&x)
        && let Some(&ty) = t.get(&y)
        && a.side.funs[ty].calls.contains(&a.side.funs[x].key)
    {
        return Verdict::Partial;
    }
    Verdict::Wrong
}

fn name_of(r: &Rom, i: usize) -> String {
    match &r.names[i] {
        Some((n, _)) => n.clone(),
        None => format!("(no name) 0x{:08x}", r.side.funs[i].key),
    }
}

fn run_score(a: &Rom, b: &Rom, show: usize, full: Config) {
    let t = truth(a, b);
    let (na, nb) = (a.side.funs.len(), b.side.funs.len());
    let named_b = b.names.iter().filter(|n| n.is_some()).count();
    println!(
        "A: {} functions ({}); B: {nb} functions ({}), {named_b} named",
        na, a.source, b.source
    );
    println!(
        "B functions with a counterpart in A: {} (small {}, mid {}, large {}); without: {}",
        t.len(),
        t.keys()
            .filter(|&&y| class(b.side.funs[y].insns) == 0)
            .count(),
        t.keys()
            .filter(|&&y| class(b.side.funs[y].insns) == 1)
            .count(),
        t.keys()
            .filter(|&&y| class(b.side.funs[y].insns) == 2)
            .count(),
        nb - t.len()
    );
    println!(
        "A functions with no counterpart in B (inlined, dropped or not found): {} of {na}",
        na - t.len()
    );
    println!();
    let inb: BTreeSet<usize> = t.values().copied().collect();
    let j = |x: usize, y: usize| judge(a, b, &t, &inb, x, y);
    println!(
        "| stages | precision | with partial | recall | pairs | small: precision / recall | mid | large |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    let runs = [
        (
            "fingerprints",
            Config {
                bytes: false,
                spread: false,
                prune: false,
                ..full
            },
        ),
        (
            "bytes + fingerprints",
            Config {
                spread: false,
                prune: false,
                ..full
            },
        ),
        (
            "fingerprints + spreading",
            Config {
                bytes: false,
                local_min: None,
                prune: false,
                ..full
            },
        ),
        (
            "bytes + fingerprints + spreading",
            Config {
                local_min: None,
                prune: false,
                ..full
            },
        ),
        (
            "+ neighbourhood step",
            Config {
                prune: false,
                ..full
            },
        ),
        ("+ prune (the default)", full),
    ];
    let mut pairs = Vec::new();
    for (label, cfg) in runs {
        let p = Matcher::new(&a.side, &b.side, cfg).run();
        table(label, &score(&a.side, &b.side, &p, &t, j));
        pairs = p;
    }
    println!();
    println!("All, by the stage that found the pair:");
    println!();
    println!("| stage | pairs | right | partial | unnamed | precision |");
    println!("|---|---|---|---|---|---|");
    for st in [Stage::Bytes, Stage::Print, Stage::Spread, Stage::Local] {
        let ps: Vec<Pair> = pairs.iter().copied().filter(|p| p.stage == st).collect();
        let s = score(&a.side, &b.side, &ps, &t, j);
        println!(
            "| {st:?} | {} | {} | {} | {} | {} |",
            ps.len(),
            Score::total(&s.right),
            Score::total(&s.partial),
            Score::total(&s.unnamed),
            pct(Score::total(&s.right), s.judged_all())
        );
    }
    println!();
    println!("All, by call-graph agreement (matched neighbours that agree / disagree):");
    println!();
    println!("| agreement | pairs | precision |");
    println!("|---|---|---|");
    type Bucket = (&'static str, fn(usize, usize) -> bool);
    let buckets: [Bucket; 4] = [
        ("no matched neighbour", |a, d| a + d == 0),
        ("agree only", |a, d| a > 0 && d == 0),
        ("both", |a, d| a > 0 && d > 0),
        ("disagree only", |a, d| a == 0 && d > 0),
    ];
    let of_a: std::collections::HashMap<usize, usize> = pairs.iter().map(|q| (q.a, q.b)).collect();
    for (label, f) in buckets {
        let ps: Vec<Pair> = pairs
            .iter()
            .copied()
            .filter(|p| {
                let (ag, dis) = xmatch::agreement_in(&a.side, &b.side, &of_a, p);
                f(ag, dis)
            })
            .collect();
        let s = score(&a.side, &b.side, &ps, &t, j);
        println!(
            "| {label} | {} | {} |",
            ps.len(),
            pct(Score::total(&s.right), s.judged_all())
        );
    }
    let s = score(&a.side, &b.side, &pairs, &t, j);
    println!();
    println!(
        "Right pairs whose sizes differ by half or more (inlining): {} of {}",
        s.resized,
        Score::total(&s.right)
    );
    println!();
    println!("Wrong pairs (B name <- A name, score, stage, round, B/A instructions):");
    for p in pairs
        .iter()
        .filter(|p| j(p.a, p.b) == Verdict::Wrong)
        .take(show)
    {
        println!(
            "  {} <- {} ({:.2}, {:?}, {}, {}/{})",
            name_of(b, p.b),
            name_of(a, p.a),
            p.score,
            p.stage,
            p.round,
            b.side.funs[p.b].insns,
            a.side.funs[p.a].insns
        );
    }
}

/// Every ROM's names (cut to its libraries with `--library`) scored on
/// every other ROM: one row per ordered pair (`xmatch matrix`).
fn run_matrix(roms: &[Rom], cfg: Config, cut: Option<&BTreeSet<String>>) {
    println!("| reference | named | precision | with partial | recall | pairs |");
    println!("|---|---|---|---|---|---|");
    for (i, a) in roms.iter().enumerate() {
        let a = match cut {
            Some(o) => library_only(a.clone(), o),
            None => a.clone(),
        };
        for (k, b) in roms.iter().enumerate() {
            if i == k {
                continue;
            }
            let t = truth(&a, b);
            let inb: BTreeSet<usize> = t.values().copied().collect();
            let p = Matcher::new(&a.side, &b.side, cfg).run();
            let s = score(&a.side, &b.side, &p, &t, |x, y| {
                judge(&a, b, &t, &inb, x, y)
            });
            println!(
                "| {} | {} | {} | {} | {} of {} | {} |",
                a.source,
                b.source,
                pct(Score::total(&s.right), s.judged_all()),
                pct(
                    Score::total(&s.right) + Score::total(&s.partial),
                    s.judged_all()
                ),
                pct(Score::total(&s.right), Score::total(&s.matchable)),
                Score::total(&s.matchable),
                s.judged_all()
            );
        }
    }
}

/// Settings tried side by side (`xmatch sweep`), for the write-up.
fn run_sweep(a: &Rom, b: &Rom, d: Config) {
    let t = truth(a, b);
    let inb: BTreeSet<usize> = t.values().copied().collect();
    let j = |x: usize, y: usize| judge(a, b, &t, &inb, x, y);
    println!(
        "| first bar | later bar | neighbourhood | prune | precision | with partial | recall | pairs |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for (fm, fg) in [(0.5, 0.1), (0.6, 0.15), (0.7, 0.2)] {
        for (ms, mg) in [(0.5, 0.1), (0.6, 0.15)] {
            for local in [None, Some(0.3), Some(0.5)] {
                for prune in [false, true] {
                    let cfg = Config {
                        first_min: fm,
                        first_margin: fg,
                        min_score: ms,
                        margin: mg,
                        local_min: local,
                        prune,
                        ..d
                    };
                    let p = Matcher::new(&a.side, &b.side, cfg).run();
                    let s = score(&a.side, &b.side, &p, &t, j);
                    println!(
                        "| {fm}/{fg} | {ms}/{mg} | {} | {prune} | {} | {} | {} | {} |",
                        local.map_or("off".into(), |l| l.to_string()),
                        pct(Score::total(&s.right), s.judged_all()),
                        pct(
                            Score::total(&s.right) + Score::total(&s.partial),
                            s.judged_all()
                        ),
                        pct(Score::total(&s.right), Score::total(&s.matchable)),
                        Score::total(&s.made)
                    );
                }
            }
        }
    }
}

fn quoted(s: &str) -> String {
    format!("{s:?}")
}

/// One reference's claim on a B function: which reference, its pair.
#[derive(Clone, Copy)]
struct Claim {
    r: usize,
    p: Pair,
}

impl Claim {
    /// Byte-identical first, then by score.
    fn rank(&self) -> (bool, f64) {
        (self.p.stage == Stage::Bytes, self.p.score)
    }
}

/// Each B function's best claim over `refs`, a real (not placeholder) name
/// on one function only, and how many references named it the same.
fn claims(refs: &[Rom], b: &Rom, cfg: Config) -> BTreeMap<usize, (Claim, usize)> {
    let mut all: BTreeMap<usize, Vec<Claim>> = BTreeMap::new();
    for (r, a) in refs.iter().enumerate() {
        for p in Matcher::new(&a.side, &b.side, cfg).run() {
            if let Some((n, _)) = &a.names[p.a]
                && !placeholder(n)
            {
                all.entry(p.b).or_default().push(Claim { r, p });
            }
        }
    }
    let name = |c: &Claim| norm(&refs[c.r].names[c.p.a].as_ref().unwrap().0);
    let mut best: BTreeMap<usize, (Claim, usize)> = BTreeMap::new();
    for (y, cs) in &all {
        let top = *cs
            .iter()
            .max_by(|x, z| x.rank().partial_cmp(&z.rank()).unwrap())
            .unwrap();
        let agree = cs.iter().filter(|c| name(c) == name(&top)).count();
        best.insert(*y, (top, agree));
    }
    // A name (in one file: statics of two files may share one) on two B
    // functions: the stronger claim keeps it.
    let file = |c: &Claim| refs[c.r].names[c.p.a].as_ref().unwrap().1.clone();
    let mut owner: BTreeMap<(String, Option<String>), usize> = BTreeMap::new();
    for (&y, (c, _)) in &best {
        let n = (name(c), file(c));
        match owner.get(&n) {
            Some(&o) if best[&o].0.rank() >= c.rank() => {}
            _ => {
                owner.insert(n, y);
            }
        }
    }
    let kept: BTreeSet<usize> = owner.values().copied().collect();
    best.retain(|y, _| kept.contains(y));
    best
}

fn run_name(
    refs: &[Rom],
    b: &Rom,
    out: &str,
    frames: Option<u32>,
    rom_b: &str,
    cfg: Config,
) -> Result<(), String> {
    let best = claims(refs, b, cfg);
    let sources: Vec<&str> = refs.iter().map(|r| r.source.as_str()).collect();
    let mut o = String::new();
    o.push_str(&format!(
        "# Names for {} matched by xmatch (crates/xmatch) from:\n{}# Don't edit.\n\n[map]\nrom_sha1 = {}\nsource = {}\nformat = 1\n\n[functions]\n",
        b.source,
        sources
            .iter()
            .enumerate()
            .map(|(i, s)| format!("#   {i}: {s}\n"))
            .collect::<String>(),
        quoted(&b.sha),
        quoted(&format!("xmatch from {}", sources.join(", ")))
    ));
    for (&y, (c, agree)) in &best {
        let a = &refs[c.r];
        let p = c.p;
        let (n, f) = a.names[p.a].as_ref().unwrap();
        let file = f
            .as_ref()
            .map_or(String::new(), |f| format!(", file = {}", quoted(f)));
        o.push_str(&format!(
            "0x{:08x} = {{ name = {}{file} }} # ref {} 0x{:08x}, {:.2} {:?} {}, {}/{} insns, {agree} of {} refs agree\n",
            b.side.funs[y].key,
            quoted(n),
            c.r,
            a.side.funs[p.a].key,
            p.score,
            p.stage,
            p.round,
            b.side.funs[y].insns,
            a.side.funs[p.a].insns,
            refs.len()
        ));
    }
    std::fs::write(out, o).map_err(|e| format!("{out}: {e}"))?;
    let by_stage = |s: Stage| best.values().filter(|(c, _)| c.p.stage == s).count();
    println!(
        "wrote {out}: {} of {} functions named ({} by bytes, {} by fingerprints, {} by spreading, {} by neighbourhood)",
        best.len(),
        b.side.funs.len(),
        by_stage(Stage::Bytes),
        by_stage(Stage::Print),
        by_stage(Stage::Spread),
        by_stage(Stage::Local)
    );
    for (r, s) in sources.iter().enumerate() {
        println!(
            "  from {s}: {}",
            best.values().filter(|(c, _)| c.r == r).count()
        );
    }
    SourceMap::load(&b.sha, &[out])?;
    let Some(frames) = frames else {
        return Ok(());
    };
    let rom = std::fs::read(rom_b).map_err(|e| format!("{rom_b}: {e}"))?;
    let rom: &'static [u8] = Box::leak(rom.into_boxed_slice());
    let keys: BTreeSet<u32> = b.side.funs.iter().map(|f| f.key).collect();
    let hits = fn_diff::hits(rom, &keys, frames);
    let n = hits
        .keys()
        .filter(|k| best.contains_key(&b.side.at[k]))
        .count();
    println!(
        "entered in {frames} frames: {} functions, {n} named",
        hits.len()
    );
    for (k, cnt) in &hits {
        let y = b.side.at[k];
        let f = &b.side.funs[y];
        match best.get(&y) {
            Some((c, _)) => println!(
                "  0x{k:08x} {:>6}x {:>4} insns  {} ({:.2} {:?})",
                cnt,
                f.insns,
                name_of(&refs[c.r], c.p.a),
                c.p.score,
                c.p.stage
            ),
            None => println!("  0x{k:08x} {:>6}x {:>4} insns  -", cnt, f.insns),
        }
    }
    Ok(())
}

/// A file name for `stem`: lower case, runs of other characters one `-`,
/// the game's-name rule ([`runtime::name::GameName`]).
fn slug(stem: &str) -> String {
    runtime::name::GameName(stem).to_string()
}

/// Whether the map at `path` names functions from symbols: it has
/// `[functions]` and its `[map]` source isn't xmatch's.
fn full_map(path: &std::path::Path) -> Result<bool, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let source = text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "source").then(|| v.trim().trim_matches('"').to_string())
    });
    let has_functions = text.lines().any(|l| l.trim() == "[functions]");
    Ok(has_functions && !source.is_some_and(|s| s.starts_with("xmatch")))
}

/// `library-names`: every `.gba` in `roms` with no full map in `names`
/// named from `refs` (cut to their libraries) into
/// `names/<slug>-library.toml`. Fails at the end if one ROM failed.
fn run_library_names(refs: &[Rom], roms: &str, names: &str, cfg: Config) -> Result<(), String> {
    let mut paths: Vec<_> = std::fs::read_dir(roms)
        .map_err(|e| format!("{roms}: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("gba")))
        .collect();
    paths.sort();
    let dir = std::path::Path::new(names);
    std::fs::create_dir_all(dir).map_err(|e| format!("{names}: {e}"))?;
    let mut failed = Vec::new();
    for p in &paths {
        let shown = p.display().to_string();
        let rom = std::fs::read(p).map_err(|e| format!("{shown}: {e}"))?;
        let sha = sha1_hex(&rom);
        let files = codegen::names::files_for(dir, &sha).map_err(|e| format!("{names}: {e}"))?;
        let mut full = None;
        for f in &files {
            if full_map(f)? {
                full = Some(f);
                break;
            }
        }
        if let Some(f) = full {
            println!("{shown}: has a full map, {}", f.display());
            continue;
        }
        let stem = p.file_stem().unwrap_or_default().to_string_lossy();
        let out = dir.join(format!("{}-library.toml", slug(&stem)));
        let r = load(&shown, None).and_then(|b| {
            println!("{shown}:");
            run_name(refs, &b, &out.display().to_string(), None, &shown, cfg)
        });
        if let Err(e) = r {
            eprintln!("xmatch: {shown}: {e}");
            failed.push(shown);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("library-names failed on {}", failed.join(", ")))
    }
}

/// The object files of every library in `libraries.toml`.
fn library_objects() -> Result<BTreeSet<String>, String> {
    let t: toml::Table = include_str!("../libraries.toml")
        .parse()
        .map_err(|e| format!("libraries.toml: {e}"))?;
    let mut out = BTreeSet::new();
    for (lib, v) in &t {
        let objs = v
            .get("objects")
            .and_then(|o| o.as_array())
            .ok_or_else(|| format!("libraries.toml: [{lib}] has no objects"))?;
        for o in objs {
            let o = o
                .as_str()
                .ok_or_else(|| format!("libraries.toml: [{lib}]: {o} isn't a string"))?;
            out.insert(o.to_string());
        }
    }
    Ok(out)
}

/// `r` with only its functions in `objects`: the library side of a
/// reference, so nothing but a library name can be carried over.
fn library_only(r: Rom, objects: &BTreeSet<String>) -> Rom {
    let keep: Vec<bool> = r
        .names
        .iter()
        .map(|n| matches!(n, Some((_, Some(f))) if objects.contains(f)))
        .collect();
    let funs = r
        .side
        .funs
        .into_iter()
        .zip(&keep)
        .filter_map(|(f, &k)| k.then_some(f))
        .collect();
    let names = r
        .names
        .into_iter()
        .zip(&keep)
        .filter_map(|(n, &k)| k.then_some(n))
        .collect();
    Rom {
        side: Side::new(funs),
        names,
        sha: r.sha,
        source: format!("{} (libraries)", r.source),
        path: r.path,
        map: r.map,
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let usage = "usage: xmatch score|sweep [--library] A.gba A.toml B.gba B.toml [SHOW] | xmatch name [--library] [--frames N] A.gba A.toml [A2.gba A2.toml…] B.gba OUT.toml | xmatch matrix [--library] ROM MAP ROM MAP… | xmatch library-names ROMS NAMES A.gba A.toml [A2.gba A2.toml…] | xmatch judge OUT ROM MAP ROM MAP… [--limit N] [--model M] [--effort E] [--jobs N] [--gap N] [--min S] [--seed N]";
    // --library: only the reference's library functions are matched;
    // library-names always is.
    let library = args
        .iter()
        .any(|a| a == "--library" || a == "library-names");
    let mut frames = None;
    let mut jo = judge::Opts {
        limit: 0,
        model: "claude-opus-5-5".into(),
        effort: None,
        jobs: 4,
        min: 0.0,
        gap: 16,
        seed: 1,
    };
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(usage);
        fn num<T: std::str::FromStr>(flag: &str, v: String) -> Result<T, String>
        where
            T::Err: std::fmt::Display,
        {
            v.parse().map_err(|e| format!("{flag} {v}: {e}"))
        }
        match a.as_str() {
            "--library" | "--no-spread" => {}
            "--frames" => frames = Some(num(a, value()?)?),
            "--limit" => jo.limit = num(a, value()?)?,
            "--jobs" => jo.jobs = num(a, value()?)?,
            "--min" => jo.min = num(a, value()?)?,
            "--gap" => jo.gap = num(a, value()?)?,
            "--seed" => jo.seed = num(a, value()?)?,
            "--model" => jo.model = value()?,
            "--effort" => jo.effort = Some(value()?),
            _ => rest.push(a.clone()),
        }
    }
    let objects = if library {
        Some(library_objects()?)
    } else {
        None
    };
    let mut cfg = if library {
        Config::library()
    } else {
        Config::default()
    };
    // --no-spread: fingerprints (and bytes) only, no call-graph rounds.
    if args.iter().any(|a| a == "--no-spread") {
        cfg.spread = false;
    }
    let load_a = |rom: &str, map: &str| -> Result<Rom, String> {
        let a = load(rom, Some(map))?;
        Ok(match &objects {
            Some(o) => library_only(a, o),
            None => a,
        })
    };
    match rest.as_slice() {
        [cmd, ra, ma, rb, mb, more @ ..] if cmd == "score" => {
            let show = match more {
                [] => 20,
                [n] => n.parse().map_err(|e| format!("SHOW {n}: {e}"))?,
                _ => return Err(usage.into()),
            };
            let (a, b) = (load_a(ra, ma)?, load(rb, Some(mb))?);
            run_score(&a, &b, show, cfg);
            Ok(())
        }
        [cmd, ra, ma, rb, mb] if cmd == "sweep" => {
            let (a, b) = (load_a(ra, ma)?, load(rb, Some(mb))?);
            run_sweep(&a, &b, cfg);
            Ok(())
        }
        [cmd, more @ ..] if cmd == "name" && more.len() >= 4 && more.len() % 2 == 0 => {
            let (pairs, tail) = more.split_at(more.len() - 2);
            let refs = pairs
                .chunks(2)
                .map(|c| load_a(&c[0], &c[1]))
                .collect::<Result<Vec<_>, _>>()?;
            let b = load(&tail[0], None)?;
            run_name(&refs, &b, &tail[1], frames, &tail[0], cfg)
        }
        [cmd, roms, names, more @ ..]
            if cmd == "library-names" && !more.is_empty() && more.len() % 2 == 0 =>
        {
            let refs = more
                .chunks(2)
                .map(|c| load_a(&c[0], &c[1]))
                .collect::<Result<Vec<_>, _>>()?;
            run_library_names(&refs, roms, names, cfg)
        }
        [cmd, out, more @ ..] if cmd == "judge" && more.len() >= 4 && more.len() % 2 == 0 => {
            let roms = more
                .chunks(2)
                .map(|c| load(&c[0], Some(&c[1])))
                .collect::<Result<Vec<_>, _>>()?;
            judge::run(&roms, &library_objects()?, std::path::Path::new(out), &jo)
        }
        [cmd, more @ ..] if cmd == "matrix" && more.len() >= 4 && more.len() % 2 == 0 => {
            let roms = more
                .chunks(2)
                .map(|c| load(&c[0], Some(&c[1])))
                .collect::<Result<Vec<_>, _>>()?;
            run_matrix(&roms, cfg, objects.as_ref());
            Ok(())
        }
        _ => Err(usage.into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xmatch: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// libraries.toml parses, names each object once, and every object is
    /// in Emerald's map style (a `.o`).
    #[test]
    fn library_list_loads() {
        let objs = library_objects().unwrap();
        assert!(objs.contains("m4a.o") && objs.contains("_udivsi3.o"));
        assert!(objs.iter().all(|o| o.ends_with(".o")));
        let t: toml::Table = include_str!("../libraries.toml").parse().unwrap();
        let listed: usize = t
            .values()
            .map(|v| v["objects"].as_array().unwrap().len())
            .sum();
        assert_eq!(listed, objs.len(), "an object is in two libraries");
    }

    /// A ROM's file name as the justfile slugs it.
    #[test]
    fn slug_like_the_justfile() {
        assert_eq!(
            slug("WarioWare, Inc. - Mega Microgame$!"),
            "warioware-inc-mega-microgame"
        );
        assert_eq!(
            slug("Kirby & The Amazing Mirror"),
            "kirby-the-amazing-mirror"
        );
    }

    /// A map from symbols is full; one xmatch wrote, or one with no
    /// functions (an assets layer), isn't.
    #[test]
    fn full_map_by_source_and_functions() {
        let dir = std::env::temp_dir().join(format!("xmatch-full-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |n: &str, t: &str| {
            let p = dir.join(n);
            std::fs::write(&p, t).unwrap();
            p
        };
        let head =
            |src: &str| format!("[map]\nrom_sha1 = \"ab\"\nsource = \"{src}\"\nformat = 1\n\n");
        let pret = write("a.toml", &(head("pret/x@1") + "[functions]\n"));
        let lib = write("b.toml", &(head("xmatch from y") + "[functions]\n"));
        let assets = write("c.toml", &(head("gbaa-assets") + "[data]\n"));
        assert!(full_map(&pret).unwrap());
        assert!(!full_map(&lib).unwrap());
        assert!(!full_map(&assets).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn rom(funs: Vec<Fun>, names: Vec<Option<(&str, Option<&str>)>>) -> Rom {
        Rom {
            side: Side::new(funs),
            names: names
                .into_iter()
                .map(|n| n.map(|(n, f)| (n.to_string(), f.map(str::to_string))))
                .collect(),
            sha: String::new(),
            source: "test".into(),
            path: "test.gba".into(),
            map: None,
        }
    }

    /// Byte-identical functions take their reference's names: two statics
    /// of one name in two files both keep it, a placeholder is never
    /// written, and one name in one file goes to one function only.
    #[test]
    fn claims_keep_statics_and_drop_placeholders() {
        let f = |key, bytes| Fun {
            key,
            insns: 10,
            bytes,
            ..Fun::default()
        };
        let a = rom(
            (1..=5)
                .map(|i| f(0x0800_0001 + 0x100 * i, i as u64))
                .collect(),
            vec![
                Some(("Cmd_end", Some("battle.o"))),
                Some(("Cmd_end", Some("anim.o"))),
                Some(("sub_08000301", Some("x.o"))),
                Some(("Bar", Some("bar.o"))),
                Some(("Bar", Some("bar.o"))),
            ],
        );
        let b = rom(
            (1..=5)
                .map(|i| f(0x0880_0001 + 0x100 * i, i as u64))
                .collect(),
            vec![None; 5],
        );
        let best = claims(std::slice::from_ref(&a), &b, Config::default());
        let named: BTreeMap<usize, &str> = best
            .iter()
            .map(|(&y, (c, _))| (y, a.names[c.p.a].as_ref().unwrap().0.as_str()))
            .collect();
        assert_eq!(named.get(&0), Some(&"Cmd_end"));
        assert_eq!(named.get(&1), Some(&"Cmd_end"));
        assert_eq!(named.get(&2), None, "a placeholder is never written");
        assert_eq!(named.values().filter(|n| **n == "Bar").count(), 1);
    }

    /// A reference cut to its libraries keeps only functions whose file
    /// is a library object, names aligned with them.
    #[test]
    fn library_only_keeps_library_functions() {
        let f = |key| Fun {
            key,
            insns: 10,
            ..Fun::default()
        };
        let r = Rom {
            side: Side::new(vec![f(0x0800_0001), f(0x0800_0101), f(0x0800_0201)]),
            names: vec![
                Some(("m4aSoundMain".into(), Some("m4a.o".into()))),
                Some(("CB2_Overworld".into(), Some("overworld.o".into()))),
                None,
            ],
            sha: String::new(),
            source: "test".into(),
            path: "test.gba".into(),
            map: None,
        };
        let objs: BTreeSet<String> = ["m4a.o".to_string()].into();
        let l = library_only(r, &objs);
        assert_eq!(l.side.funs.len(), 1);
        assert_eq!(l.side.funs[0].key, 0x0800_0001);
        assert_eq!(l.names[0].as_ref().unwrap().0, "m4aSoundMain");
        assert_eq!(l.side.at[&0x0800_0001], 0);
    }
}
