//! Which compiled functions a randomizer's changes would make stale.
//!
//! Reads a generated crate's parts (`out/<game>/parts/*/src/*.rs`): each
//! `pub fn f_XXXXXXXX` is a compiled function, each `insn!(ADDR, …)` an
//! instruction it was compiled from, each `m.bus.load(Width::W, 0x8…)` with a
//! constant ROM address a load whose value const-prop baked into the code
//! (`docs/sem-design.md`), and each `f_XXXXXXXX(m, …)` call a direct call.
//! Then takes the changed ROM bytes, from a list of ranges (`--writes`) or
//! by comparing two ROMs (`--base` and `--other`), and counts the functions
//! whose instructions or baked loads touch them, and the functions that call
//! those directly.
//!
//! Build and run (std only, outside the workspace):
//!   rustc -O --edition 2021 docs/research/randomized-roms/probe.rs -o /tmp/rr-probe
//!   /tmp/rr-probe --parts out/pokemon-emerald-version/parts \
//!     --sym $GBAA_DATA/symbols/pokeemerald.sym \
//!     --writes docs/research/randomized-roms/kaizo-emerald.txt
//!   /tmp/rr-probe --parts … --sym … --base base.gba --other randomized.gba
//!   /tmp/rr-probe --parts … --sym … --baked   (the data the code bakes in)

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

struct Func {
    /// Byte ranges of its instructions, `[start, end)`.
    code: Vec<(u32, u32)>,
    /// Constant ROM loads: address and width in bytes.
    loads: Vec<(u32, u32)>,
    /// Entries of the functions it calls directly.
    calls: BTreeSet<u32>,
}

fn hex(s: &str) -> Option<u32> {
    let s = s.trim_start_matches("0x").replace('_', "");
    u32::from_str_radix(&s, 16).ok()
}

/// The hex number at the start of `s` (after optional spaces), with `_`s.
fn lead_hex(s: &str) -> Option<u32> {
    let s = s.trim_start();
    let s = s.strip_prefix("0x")?;
    let end = s
        .find(|c: char| !(c.is_ascii_hexdigit() || c == '_'))
        .unwrap_or(s.len());
    hex(&s[..end])
}

fn parse(text: &str, funcs: &mut BTreeMap<u32, Func>) {
    let mut cur: Option<u32> = None;
    let mut rest = text;
    while !rest.is_empty() {
        let line_end = rest.find('\n').map_or(rest.len(), |i| i + 1);
        let line = &rest[..line_end];
        if let Some(i) = line.find("pub fn f_") {
            let k = &line[i + 9..];
            if k.contains("(m: &mut M, mut pc: u32) -> Flow {")
                || k.contains("(m: &mut M, pc: u32) -> Flow {")
            {
                if let Some(e) = hex(&k[..8]) {
                    cur = Some(e);
                    funcs.entry(e).or_insert(Func {
                        code: vec![],
                        loads: vec![],
                        calls: BTreeSet::new(),
                    });
                }
            }
        } else if let Some(e) = cur {
            let f = funcs.get_mut(&e).unwrap();
            if let Some(i) = line.find("insn!(") {
                // The address is on this line or the next.
                let after = &rest[i + 6..];
                if let Some(a) = lead_hex(after.trim_start_matches(['\n', ' '])) {
                    let w = if e & 1 != 0 { 2 } else { 4 };
                    f.code.push((a, a + w));
                }
            }
            let mut s = line;
            while let Some(i) = s.find("m.bus.load(Width::") {
                let t = &s[i + 18..];
                let w = if t.starts_with("Byte") {
                    1
                } else if t.starts_with("Half") {
                    2
                } else {
                    4
                };
                if let Some(c) = t.find(", ") {
                    if let Some(a) = lead_hex(&t[c + 2..]) {
                        if (0x0800_0000..0x0e00_0000).contains(&a) {
                            let a = a & !(w - 1);
                            f.loads.push((a, w));
                        }
                    }
                }
                s = t;
            }
            let mut s = line;
            while let Some(i) = s.find("f_") {
                let t = &s[i + 2..];
                if t.len() > 12 && t.as_bytes()[8] == b'(' && t[9..].starts_with("m,") {
                    if let Some(c) = hex(&t[..8]) {
                        if c != e {
                            f.calls.insert(c);
                        }
                    }
                }
                s = t;
            }
        }
        rest = &rest[line_end..];
    }
}

fn load_funcs(parts: &Path) -> BTreeMap<u32, Func> {
    let mut funcs = BTreeMap::new();
    let mut files = vec![];
    for p in fs::read_dir(parts).expect("parts dir") {
        let src = p.unwrap().path().join("src");
        if let Ok(rd) = fs::read_dir(&src) {
            for f in rd {
                let f = f.unwrap().path();
                if f.extension().is_some_and(|x| x == "rs") {
                    files.push(f);
                }
            }
        }
    }
    files.sort();
    for f in files {
        parse(&fs::read_to_string(&f).unwrap(), &mut funcs);
    }
    // Keys in RAM (IWRAM copies) aren't compiled from ROM addresses.
    funcs.retain(|&k, _| k >= 0x0800_0000);
    funcs
}

/// pokeemerald.sym: `ADDR g|l SIZE NAME`.
fn load_sym(p: &Path) -> Vec<(u32, u32, String)> {
    let mut v: Vec<_> = fs::read_to_string(p)
        .unwrap()
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let a = hex(w.next()?)?;
            w.next()?;
            let n = hex(w.next()?)?;
            Some((a, n, w.next()?.to_string()))
        })
        .filter(|s| s.0 >= 0x0800_0000 && s.1 > 0)
        .collect();
    v.sort();
    v
}

fn name(sym: &[(u32, u32, String)], a: u32) -> String {
    let i = sym.partition_point(|s| s.0 <= a);
    match i.checked_sub(1).map(|i| &sym[i]) {
        Some((s, n, nm)) if a < s + n => format!("{nm}+0x{:x}", a - s),
        _ => format!("{a:08x}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .map(|i| args[i + 1].clone())
    };
    let funcs = load_funcs(Path::new(&get("--parts").expect("--parts DIR")));
    let sym = get("--sym")
        .map(|p| load_sym(Path::new(&p)))
        .unwrap_or_default();
    let code_bytes: u64 = funcs
        .values()
        .flat_map(|f| f.code.iter())
        .map(|(a, b)| (b - a) as u64)
        .sum();
    println!(
        "compiled functions {}, instruction bytes {}, baked ROM loads {}",
        funcs.len(),
        code_bytes,
        funcs.values().map(|f| f.loads.len()).sum::<usize>()
    );

    // Byte -> owner: every instruction byte's function.
    let mut owner: BTreeMap<u32, u32> = BTreeMap::new();
    for (&e, f) in &funcs {
        for &(a, b) in &f.code {
            for x in a..b {
                owner.insert(x, e);
            }
        }
    }

    if args.iter().any(|a| a == "--baked") {
        // Baked loads outside every function's instructions: literal pools
        // sit between and after code, so count only those that aren't
        // within 1 KiB of the loading function's own instructions.
        let mut by: BTreeMap<String, (usize, BTreeSet<u32>)> = BTreeMap::new();
        for (&e, f) in &funcs {
            let lo = f.code.iter().map(|c| c.0).min().unwrap_or(e);
            let hi = f.code.iter().map(|c| c.1).max().unwrap_or(e);
            for &(a, _) in &f.loads {
                if a + 1024 >= lo && a <= hi + 1024 {
                    continue;
                }
                let n = name(&sym, a);
                let n = n.split('+').next().unwrap().to_string();
                let s = by.entry(n).or_default();
                s.0 += 1;
                s.1.insert(e);
            }
        }
        let mut v: Vec<_> = by.into_iter().collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1 .1.len()));
        println!("baked loads away from their function: by symbol (loads, functions)");
        for (n, (c, fs)) in v.iter() {
            println!("  {n}: {c} loads, {} functions", fs.len());
        }
        println!("  ({} symbols)", v.len());
        return;
    }

    // The changed bytes, each with its label.
    let mut changed: BTreeMap<u32, String> = BTreeMap::new();
    if let Some(w) = get("--writes") {
        for l in fs::read_to_string(&w).unwrap().lines() {
            let l = l.split('#').next().unwrap().trim();
            if l.is_empty() {
                continue;
            }
            let mut p = l.splitn(3, ' ');
            let a = hex(p.next().unwrap()).expect("address");
            let n = p
                .next()
                .and_then(|n| n.trim().parse::<u32>().ok().or_else(|| hex(n)))
                .expect("length");
            let label = p.next().unwrap_or("").trim().to_string();
            for x in a..a + n {
                changed.insert(x | 0x0800_0000, label.clone());
            }
        }
    }
    if let (Some(b), Some(o)) = (get("--base"), get("--other")) {
        let (b, o) = (fs::read(b).unwrap(), fs::read(o).unwrap());
        let n = b.len().max(o.len());
        let mut diff = 0;
        for i in 0..n {
            if b.get(i) != o.get(i) {
                changed.insert(0x0800_0000 + i as u32, String::new());
                diff += 1;
            }
        }
        println!(
            "ROM sizes {} and {}, bytes that differ {diff}",
            b.len(),
            o.len()
        );
    }

    let mut code_hit: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    let mut load_hit: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for (&x, label) in &changed {
        if let Some(&e) = owner.get(&x) {
            code_hit.entry(e).or_default().insert(label.clone());
        }
    }
    for (&e, f) in &funcs {
        for &(a, w) in &f.loads {
            for x in a..a + w {
                if let Some(label) = changed.get(&(x | 0x0800_0000)) {
                    load_hit.entry(e).or_default().insert(label.clone());
                }
            }
        }
    }
    let stale: BTreeSet<u32> = code_hit.keys().chain(load_hit.keys()).copied().collect();
    let callers: BTreeSet<u32> = funcs
        .iter()
        .filter(|(e, f)| !stale.contains(e) && f.calls.iter().any(|c| stale.contains(c)))
        .map(|(&e, _)| e)
        .collect();
    let in_code = changed.keys().filter(|x| owner.contains_key(x)).count();
    println!(
        "changed bytes {}, in compiled instructions {in_code}",
        changed.len()
    );
    println!(
        "functions whose instructions changed {}, whose baked loads changed {}, stale in all {} of {}",
        code_hit.len(),
        load_hit.len(),
        stale.len(),
        funcs.len()
    );
    println!(
        "unchanged functions that call a stale one directly {}",
        callers.len()
    );
    // If a stale function's callers must stop calling it directly, without
    // a guard on each call, they go stale too, and their callers in turn.
    let mut closed = stale.clone();
    loop {
        let more: Vec<u32> = funcs
            .iter()
            .filter(|(e, f)| !closed.contains(e) && f.calls.iter().any(|c| closed.contains(c)))
            .map(|(&e, _)| e)
            .collect();
        if more.is_empty() {
            break;
        }
        closed.extend(more);
    }
    println!(
        "stale with every direct caller, transitively {}",
        closed.len()
    );
    if args.iter().any(|a| a == "--closure") {
        for &e in closed.difference(&stale) {
            println!("  caller {e:08x} {}", name(&sym, e & !1));
        }
    }
    for &e in &stale {
        let why: BTreeSet<_> = code_hit
            .get(&e)
            .into_iter()
            .flatten()
            .map(|l| format!("code: {l}"))
            .chain(
                load_hit
                    .get(&e)
                    .into_iter()
                    .flatten()
                    .map(|l| format!("baked load: {l}")),
            )
            .collect();
        let why: Vec<_> = why.into_iter().collect();
        println!("  {e:08x} {} ({})", name(&sym, e & !1), why.join("; "));
    }
}
