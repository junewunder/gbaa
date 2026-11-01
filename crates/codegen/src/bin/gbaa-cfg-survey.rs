//! `gbaa-cfg-survey ROM [TRUTH]`: the shape of every recompiled ROM
//! function's control flow, for the structured-codegen review
//! (`docs/research/structured-control-flow.md`): how many functions are
//! reducible, their sizes, their loops and loop exits, the irreducible
//! regions a structurer would have to split or dispatch, jump tables, and
//! how deep Ramsey's translation would nest, and what the structuring
//! plan (`codegen::plan`) makes of each function. TRUTH (a `disc-truth` file)
//! names the functions it lists.
//!
//! The CFG and its shape are codegen's own (`codegen::cfg`). Code copied
//! into IWRAM isn't included.

use analysis::discovery::discover;
use codegen::cfg::{Cfg, Region, Shape};
use codegen::emit::build_all;
use codegen::func::{Ctx, Func, exits};
use codegen::plan::{self, Fail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 {
        eprintln!("usage: gbaa-cfg-survey ROM [TRUTH]");
        std::process::exit(2);
    }
    let (rom, truth) = (args[0].clone(), args.get(1).cloned());
    survey(Path::new(&rom), truth.as_deref().map(Path::new));
}

/// What the survey counts besides the CFG's shape.
struct Extra {
    insns: Vec<usize>,
    /// Blocks that are a call's return site.
    returns: usize,
    /// Jump tables: their targets inside the function.
    tables: Vec<usize>,
}

fn extra(ctx: &Ctx, f: &Func, g: &Cfg) -> Extra {
    let index: BTreeMap<u32, usize> = g.keys.iter().enumerate().map(|(i, &k)| (k, i)).collect();
    let mut rets = BTreeSet::new();
    let mut tables = Vec::new();
    for gb in f.blocks.values() {
        for x in exits(ctx, gb) {
            if let Some(r) = x.ret.and_then(|r| index.get(&r)) {
                rets.insert(*r);
            }
            if let Some(t) = ctx.table_targets(x.at, x.thumb) {
                let inside: BTreeSet<u32> =
                    t.into_iter().filter(|t| f.blocks.contains_key(t)).collect();
                tables.push(inside.len());
            }
        }
    }
    Extra {
        insns: g.keys.iter().map(|k| f.blocks[k].insns.len()).collect(),
        returns: rets.len(),
        tables,
    }
}

fn bucket(x: usize) -> &'static str {
    match x {
        0..=1 => "1",
        2..=4 => "2-4",
        5..=16 => "5-16",
        17..=64 => "17-64",
        65..=256 => "65-256",
        _ => "257+",
    }
}
const BUCKETS: [&str; 6] = ["1", "2-4", "5-16", "17-64", "65-256", "257+"];

fn names(path: Option<&Path>) -> BTreeMap<u32, String> {
    let Some(path) = path else {
        return BTreeMap::new();
    };
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let a = u32::from_str_radix(it.next()?.trim_start_matches("0x"), 16).ok()?;
            let name = it.nth(1)?.to_string();
            Some((a, name))
        })
        .collect()
}

fn pct(a: usize, b: usize) -> String {
    format!("{:.1}%", 100.0 * a as f64 / b.max(1) as f64)
}

fn survey(rom_path: &Path, truth: Option<&Path>) {
    let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{}: {e}", rom_path.display()));
    let names = names(truth);
    let name = |k: u32| {
        names
            .get(&k)
            .or_else(|| names.get(&(k & !1)))
            .or_else(|| names.get(&(k | 1)))
            .cloned()
            .unwrap_or_default()
    };
    let disc = discover(rom.clone());
    let ctx = Ctx::new(&rom, &disc);
    let code = build_all(&ctx);

    let (mut funcs, mut red, mut blocks, mut insns) = (0, 0, 0, 0);
    let (mut irr_insns, mut irr_blocks, mut split_insns) = (0, 0, 0);
    let mut hist: BTreeMap<&str, [usize; 4]> = BTreeMap::new(); // fns, irreducible, insns, loops
    let (mut loops, mut multi_exit, mut irr_regions, mut max_depth) = (0, 0, 0, 0);
    let mut loop_fns = 0;
    let mut depth_hist: BTreeMap<usize, usize> = BTreeMap::new();
    let mut nest_hist: BTreeMap<usize, usize> = BTreeMap::new();
    let (mut returns, mut tables, mut table_targets, mut max_table) = (0, 0, 0, 0);
    let mut irr_list = Vec::new();
    let mut big = Vec::new();
    let mut unreach: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut plans = Plans::default();
    for f in &code.funcs {
        let g = Cfg::of(&ctx, f);
        let s = Shape::of(&g);
        let x = extra(&ctx, f, &g);
        let region_insns = |r: &Region| r.blocks.iter().map(|&v| x.insns[v]).sum::<usize>();
        let n_insns: usize = x.insns.iter().sum();
        if !g.unreachable.is_empty() {
            unreach.push((f.entry, g.unreachable.clone()));
        }
        funcs += 1;
        blocks += g.keys.len();
        insns += n_insns;
        returns += x.returns;
        tables += x.tables.len();
        table_targets += x.tables.iter().sum::<usize>();
        max_table = max_table.max(x.tables.iter().copied().max().unwrap_or(0));
        let h = hist.entry(bucket(g.keys.len())).or_default();
        h[0] += 1;
        h[2] += n_insns;
        h[3] += s.regions.len();
        if s.reducible {
            red += 1;
            *nest_hist.entry(s.nesting).or_default() += 1;
        } else {
            h[1] += 1;
        }
        loop_fns += !s.regions.is_empty() as usize;
        for r in &s.regions {
            loops += 1;
            multi_exit += (r.exits.len() > 1) as usize;
            max_depth = max_depth.max(r.depth + 1);
            *depth_hist.entry(r.depth + 1).or_default() += 1;
            if !r.is_loop() {
                irr_regions += 1;
                // Regions nested in an irreducible one are counted with it.
                irr_list.push((
                    f.entry,
                    r.blocks.len(),
                    region_insns(r),
                    r.entries.len(),
                    r.exits.len(),
                    g.keys.len(),
                    n_insns,
                ));
            }
        }
        // The outermost irreducible regions: what a dispatch fallback
        // would keep as `loop { match }`, and what splitting copies (one
        // copy of the region per extra entry, a lower bound).
        for r in s.irreducible() {
            let r_insns = region_insns(r);
            irr_blocks += r.blocks.len();
            irr_insns += r_insns;
            split_insns += (r.entries.len() - 1) * r_insns;
        }
        plans.add(f.entry, &s, plan::plan(&g, &plan::exits_of(&ctx, f, &g)));
        big.push((
            n_insns,
            g.keys.len(),
            f.entry,
            s.reducible,
            s.nesting,
            s.regions.len(),
        ));
    }

    println!("gbaa-cfg-survey: {}", rom_path.display());
    println!(
        "functions {funcs}, blocks {blocks}, instructions {insns} (ROM code; IWRAM copies left out)"
    );
    println!(
        "reducible {red} ({}), irreducible {} ({})",
        pct(red, funcs),
        funcs - red,
        pct(funcs - red, funcs)
    );
    println!(
        "irreducible regions {irr_regions}; their outermost ones hold {irr_blocks} blocks, {irr_insns} instructions ({} of all); splitting them copies at least {split_insns} instructions",
        pct(irr_insns, insns)
    );
    println!(
        "loops and regions {loops} in {loop_fns} functions; with exits to two or more blocks {multi_exit} ({}); deepest nest {max_depth}",
        pct(multi_exit, loops)
    );
    println!(
        "loop depth: {}",
        depth_hist
            .iter()
            .map(|(d, n)| format!("{d}: {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "return sites {returns} ({} of blocks); jump tables {tables}, {table_targets} targets inside their function, largest {max_table}",
        pct(returns, blocks)
    );
    println!("by blocks per function: functions, irreducible, instructions, loops");
    for b in BUCKETS {
        if let Some(h) = hist.get(b) {
            println!("  {b:>7}: {:>6} {:>4} {:>8} {:>6}", h[0], h[1], h[2], h[3]);
        }
    }
    let mut nests: Vec<usize> = Vec::new();
    for (&d, &n) in &nest_hist {
        nests.extend(std::iter::repeat_n(d, n));
    }
    let q = |p: f64| {
        nests
            .get(((nests.len() as f64 - 1.0) * p) as usize)
            .copied()
            .unwrap_or(0)
    };
    println!(
        "Ramsey nesting (reducible functions): median {}, p90 {}, p99 {}, max {}",
        q(0.5),
        q(0.9),
        q(0.99),
        q(1.0)
    );
    println!(
        "irreducible regions (function, region blocks/insns, entries, exits, function blocks/insns, name):"
    );
    for (e, b, i, en, ex, fb, fi) in &irr_list {
        println!(
            "  0x{e:08x} {b:>4}/{i:<5} {en:>2} {ex:>3} {fb:>5}/{fi:<6} {}",
            name(*e)
        );
    }
    println!(
        "functions with blocks their CFG doesn't reach from the entry (left out): {}",
        unreach.len()
    );
    for (e, ks) in unreach.iter().take(10) {
        let ks: Vec<String> = ks.iter().map(|k| format!("0x{k:08x}")).collect();
        println!("  0x{e:08x} {}: {}", name(*e), ks.join(" "));
    }
    plans.print(&name);
    big.sort_unstable_by(|a, b| b.cmp(a));
    println!("largest functions (insns, blocks, reducible, nesting, loops, name):");
    for (i, b, e, r, nest, l) in big.iter().take(12) {
        println!(
            "  0x{e:08x} {i:>6} {b:>5} {r:>5} {nest:>4} {l:>4} {}",
            name(*e)
        );
    }
}

/// What the structuring plan made of the functions.
#[derive(Default)]
struct Plans {
    planned: usize,
    /// Planned with collapsed regions: functions, regions, their blocks.
    collapsed: [usize; 3],
    /// Still irreducible with their regions collapsed, and failing the
    /// structure check (with the first message).
    irreducible: Vec<u32>,
    failed: Vec<(u32, String)>,
    /// Planned reducible functions nesting less and more than the shape's
    /// estimate.
    unlike_shape: [usize; 2],
    nesting: Vec<usize>,
    indent: Vec<(usize, u32)>,
}

impl Plans {
    fn add(&mut self, entry: u32, s: &Shape, p: Result<plan::Planned, Fail>) {
        let p = match p {
            Ok(p) => p,
            Err(Fail::Irreducible) => return self.irreducible.push(entry),
            Err(Fail::Check(m)) => return self.failed.push((entry, m)),
        };
        self.planned += 1;
        if p.regions > 0 {
            self.collapsed[0] += 1;
            self.collapsed[1] += p.regions;
            self.collapsed[2] += p.region_blocks;
        }
        let d = p.depth();
        if s.reducible && d.nesting != s.nesting {
            self.unlike_shape[(d.nesting > s.nesting) as usize] += 1;
        }
        self.nesting.push(d.nesting);
        self.indent.push((d.indent, entry));
    }

    fn print(&mut self, name: &dyn Fn(u32) -> String) {
        let funcs = self.planned + self.irreducible.len() + self.failed.len();
        println!(
            "structuring plan: planned {} of {funcs} ({}); with collapsed regions {} ({} regions, {} blocks); still irreducible {}; failed the check {}",
            self.planned,
            pct(self.planned, funcs),
            self.collapsed[0],
            self.collapsed[1],
            self.collapsed[2],
            self.irreducible.len(),
            self.failed.len(),
        );
        self.nesting.sort_unstable();
        self.indent.sort_unstable();
        let q = |v: &[usize], p: f64| {
            v.get(((v.len() as f64 - 1.0) * p) as usize)
                .copied()
                .unwrap_or(0)
        };
        let indent: Vec<usize> = self.indent.iter().map(|x| x.0).collect();
        println!(
            "plan nesting (loops, labeled blocks, regions): median {}, p90 {}, p99 {}, max {}; reducible ones below the estimate {}, above it {}",
            q(&self.nesting, 0.5),
            q(&self.nesting, 0.9),
            q(&self.nesting, 0.99),
            q(&self.nesting, 1.0),
            self.unlike_shape[0],
            self.unlike_shape[1]
        );
        println!(
            "plan indent (and inlined if/match arms): median {}, p90 {}, p99 {}, max {}",
            q(&indent, 0.5),
            q(&indent, 0.9),
            q(&indent, 0.99),
            q(&indent, 1.0)
        );
        for (d, e) in self.indent.iter().rev().take(3) {
            println!("  indent {d}: 0x{e:08x} {}", name(*e));
        }
        for e in &self.irreducible {
            println!("  still irreducible: 0x{e:08x} {}", name(*e));
        }
        for (e, m) in self.failed.iter().take(10) {
            println!("  failed: 0x{e:08x} {}: {m}", name(*e));
        }
    }
}
