//! `gbaa-recompile ROM [OUT] [--name NAME] [--no-flag-liveness]
//! [--no-structured] [--no-dead-reg-writes] [--locals] [--callee-saves]
//! [--asm-text | --asm-comments]`: discover a ROM's code and write it as a
//! Rust crate to OUT (`just recompile`), by default `out/<name>`. The name
//! is the game's, the key its saves are filed under, baked into the build:
//! NAME, or else the ROM's file name, in the one form
//! ([`runtime::name::GameName`]: "Kirby & The Amazing Mirror" is
//! `kirby-the-amazing-mirror`). With `--no-flag-liveness`,
//! every flag write is kept (`Config::flag_liveness` off), for comparing
//! the two builds' exact states; with `--no-structured`, every function is
//! printed in v1 form (`Config::structured` off); `--no-dead-reg-writes`
//! turns off `Config::dead_reg_writes`, `--locals` turns on
//! `Config::locals`, and `--callee-saves` (with it) `Config::callee_saves`.
//! `--asm-text` prints each `insn!` marker's instruction as assembly, and
//! `--asm-comments` as a `// 0x0805_9810 push {r4, lr}` comment instead
//! (`Config::insn`); a readable function prints none without one. Each `--names FILE` layers a source map
//! ([`codegen::names`]) over the ones before: the crate is the named
//! print, the mod view (`docs/mod-view-design.md`). `--readable FILE`
//! prints the functions FILE lists (by key or name, `*` for every one, `#`
//! comments; [`codegen::names::readable_list`]) in the readable form, the rest as
//! the flags say ([`codegen::func::Ctx::cfg_for`]). `--mod DIR` builds a
//! mod copy in: its functions leave the parts for a `modded` part crate
//! compiling the mod's `src/*.rs` ([`codegen::modcopy`]). With no
//! `--names`, `--names-dir DIR` names the readable form and a mod from the
//! map files in DIR that name the ROM by sha1
//! ([`codegen::names::files_for`]), as fn-diff does; a plain recompile, the
//! play build, stays unnamed unless `--names` names it.
//!
//! `gbaa-recompile --messages-check ROM JSON`: decode every message of an
//! index game's table (`crates/codegen/text-tables.toml`, `[game.index]`)
//! and compare each with JSON, its decomp's text by bank and message
//! (tmc's `translations/USA.json`); exits 1 if any differ.
//!
//! `gbaa-recompile ROM --names FILE... --cut DIR FUNC...`: cut FUNC (keys
//! or names) into the mod copy at DIR, printed readable and named
//! (`docs/mod-view-design.md`, "A mod copy"). `--names-dir DIR` in place
//! of `--names` too, here and below.
//!
//! `gbaa-recompile --find ROM FIELD --names FILE...`: the functions whose
//! readable print reads or writes the struct field FIELD, writers and
//! readers apart, by pret line ([`codegen::find`]; `just mod-find`).
//!
//! `gbaa-recompile --names-check ROM --names FILE... [--readable LIST]`:
//! print every function unnamed and named and check the two token by token
//! ([`codegen::names::check`]); LIST's functions readable, with their
//! DWARF names and comments counted.
//!
//! `gbaa-recompile --fn-diff ROM OUT [--hand FILE KEYS] [--names-dir DIR] [FUNCS]`: write the
//! crate of the per-function differential runs (`just fn-diff`,
//! [`codegen::fn_diff`]): FUNCS functions (200) in every configuration.
//! With `--hand`, only KEYS (comma-separated hex) are picked, and a `hand`
//! form compiles FILE's fns in place of their generated code
//! ([`codegen::fn_diff::Opts::hand`]). With `--names-dir`, the map files in
//! DIR that name this ROM ([`codegen::names::files_for`]) name and type the
//! readable form ([`codegen::fn_diff::Opts::names`]).
//!
//! `gbaa-recompile --print ROM KEY...`: print those functions to stdout
//! with registers in locals, no reloads after calls to functions that keep
//! r4-r11 ([`codegen::saves`]) and assembly text, as the readable form
//! starts from.
//!
//! `gbaa-recompile --saves ROM [KEY...]`: how many functions keep r4-r11
//! and sp ([`codegen::saves`]), why the others don't, and each KEY's
//! answer.
//!
//! `gbaa-recompile --summaries ROM [--names FILE]... [--names-dir DIR]
//! [--show N] [--callee-sets] [KEY...]`: each function's parameters,
//! results and callee-saves ([`codegen::saves::summarize`]), checked
//! against the map's DWARF prototypes ([`codegen::saves::compare`]):
//! agree, disagree and no-prototype counts, the disagreements by category
//! with N of each (default 10, a sample spread over the list), and each
//! KEY's summary. With `--callee-sets`, an indirect call with a complete
//! callee set reads and leaves what its callees do
//! ([`codegen::native::callee_sets`], [`codegen::saves::summarize_sets`];
//! the function-address flow, which doesn't settle on a whole game yet).
//!
//! `gbaa-recompile --snapshots ROM FILE`: write to FILE the smallest
//! structured function of each shape ([`codegen::snapshot`], `just
//! snapshots`). With `--names FILE... --readable LIST`, the functions LIST
//! names instead, printed readable (`just snapshots-readable`).
//!
//! `gbaa-recompile --sst OUT [PER_FILE [SEQS]]`: write a crate with one
//! generated fn per sampled SingleStepTests case, which checks each against
//! the expected state and the interpreter, and SEQS sequence cases (random
//! blocks and short functions of the suite's opcodes), checked against the
//! interpreter with their timing (`just sst-gen`). With `--readable`, each
//! is printed in the readable form.

#[path = "../../../armv4t/tests/common/sst.rs"]
#[allow(dead_code)]
mod sst;

use analysis::discovery::{Code, Discovery, Evidence, discover};
use analysis::ir::build::Builder;
use analysis::ir::constprop::Rom;
use analysis::program::ROM_BASE;
use armv4t::decode::{decode_arm, decode_thumb};
use armv4t::interp::Cpu;
use armv4t::sem::arm::execute;
use codegen::emit::{lib_head, pretty, write_crate, write_if_changed};
use codegen::func::{Clock, Config, Ctx, Func, GBlock, Insn, build_func, key};
use codegen::lower::{fn_name, hex, lower_func, lower_func_named};
use codegen::names::SourceMap;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
}

fn main() {
    analysis::cap::require_cap("gbaa-recompile");
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, out, rest @ ..] if flag == "--sst" => {
            let readable = rest.iter().any(|a| a == "--readable");
            let rest: Vec<_> = rest.iter().filter(|a| *a != "--readable").collect();
            let per_file = rest.first().map_or(100, |n| n.parse().expect("PER_FILE"));
            let seqs = rest.get(1).map_or(2000, |n| n.parse().expect("SEQS"));
            sst_crate(Path::new(out), per_file, seqs, readable);
        }
        [flag, rom, out, rest @ ..] if flag == "--fn-diff" => {
            let mut opts = codegen::fn_diff::Opts::default();
            let mut rest = rest;
            if let [flag, file, keys, more @ ..] = rest
                && flag == "--hand"
            {
                let keys = keys
                    .split(',')
                    .map(|k| u32::from_str_radix(k.trim_start_matches("0x"), 16).expect("KEY"))
                    .collect();
                opts.hand = Some((PathBuf::from(file), keys));
                rest = more;
            }
            let mut names_dir = None;
            if let [flag, dir, more @ ..] = rest
                && flag == "--names-dir"
            {
                names_dir = Some(PathBuf::from(dir));
                rest = more;
            }
            if let Some(n) = rest.first() {
                opts.funcs = n.parse().expect("FUNCS");
            }
            fn_diff(Path::new(rom), Path::new(out), names_dir.as_deref(), opts);
        }
        [flag, rom_path, file, rest @ ..] if flag == "--snapshots" && !rest.is_empty() => {
            // The readable snapshot: `--names FILE... --readable LIST`.
            let Opts {
                names,
                names_dir,
                readable: list,
                ..
            } = options(rest, Clock::Exact).unwrap_or_else(|| usage());
            let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{rom_path}: {e}"));
            let names = mod_view_names(&rom, &names, names_dir.as_deref());
            let disc = discover(rom.clone());
            let all: Vec<u32> = disc.funcs.keys().copied().collect();
            let list = read_list(&list.unwrap_or_else(|| usage()), names.as_ref(), &all);
            let mut ctx = Ctx::new(&rom, &disc);
            ctx.names = names.as_ref();
            ctx.readable = Some(&list);
            ctx.prove_saves(&codegen::emit::build_all(&ctx).funcs);
            let name = Path::new(rom_path).file_stem().unwrap().to_string_lossy();
            let text = codegen::snapshot::readable(&ctx, &name);
            write_if_changed(Path::new(file), text).expect("writing the snapshot");
        }
        [flag, rom_path, file] if flag == "--snapshots" => {
            let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{rom_path}: {e}"));
            let disc = discover(rom.clone());
            let ctx = Ctx::new(&rom, &disc);
            let r = codegen::emit::build_all(&ctx);
            let name = Path::new(rom_path).file_stem().unwrap().to_string_lossy();
            let text = codegen::snapshot::snapshots(&ctx, &r, &name);
            write_if_changed(Path::new(file), text).expect("writing the snapshots");
        }
        [flag, rom_path, keys @ ..] if flag == "--print" => {
            let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{rom_path}: {e}"));
            let disc = discover(rom.clone());
            let mut ctx = Ctx::new(&rom, &disc);
            ctx.cfg.locals = true;
            ctx.cfg.insn = Insn::Asm;
            ctx.cfg.callee_saves = true;
            ctx.prove_saves(&codegen::emit::build_all(&ctx).funcs);
            for k in keys {
                let k = u32::from_str_radix(k.trim_start_matches("0x"), 16).expect("KEY");
                let f = build_func(&ctx, k);
                println!("// 0x{k:08x}\n{}", pretty(lower_func(&ctx, &f)));
            }
        }
        [flag, rom_path, rest @ ..] if flag == "--summaries" => {
            let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{rom_path}: {e}"));
            summaries(&rom, rest);
        }
        [flag, rom_path, keys @ ..] if flag == "--saves" => {
            let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{rom_path}: {e}"));
            let disc = discover(rom.clone());
            saves(&rom, &disc, keys);
        }
        [flag, rom, json] if flag == "--messages-check" => {
            messages_check(Path::new(rom), Path::new(json))
        }
        [flag, rom, rest @ ..] if flag == "--names-check" => {
            let o = options(rest, Clock::Exact).unwrap_or_else(|| usage());
            names_check(
                Path::new(rom),
                o.cfg,
                &o.names,
                o.names_dir.as_deref(),
                o.readable.as_deref(),
            )
        }
        [flag, rom, field, rest @ ..] if flag == "--find" => {
            let o = options(rest, Clock::Exact).unwrap_or_else(|| usage());
            find(Path::new(rom), o, field)
        }
        [rom, rest @ ..] if !rom.starts_with("--") && rest.iter().any(|a| a == "--cut") => {
            let i = rest.iter().position(|a| a == "--cut").unwrap();
            let o = options(&rest[..i], Clock::Instant).unwrap_or_else(|| usage());
            let [dir, funcs @ ..] = &rest[i + 1..] else {
                usage()
            };
            if funcs.is_empty() || o.modded.is_some() {
                usage()
            }
            cut(Path::new(rom), o, Path::new(dir), funcs)
        }
        [rom, rest @ ..] if !rom.starts_with("--") => {
            // OUT is optional: the first argument, if it isn't a switch.
            let (out, rest) = match rest {
                [out, rest @ ..] if !out.starts_with("--") => (Some(Path::new(out)), rest),
                _ => (None, rest),
            };
            let o = options(rest, Clock::Instant).unwrap_or_else(|| usage());
            recompile(Path::new(rom), out, o)
        }
        _ => usage(),
    }
}

/// The switches of a recompile.
struct Opts {
    /// [`FLAGS`].
    cfg: Config,
    /// Each `--names FILE`, in order.
    names: Vec<PathBuf>,
    /// `--names-dir DIR`.
    names_dir: Option<PathBuf>,
    /// `--readable FILE`.
    readable: Option<PathBuf>,
    /// `--mod DIR`.
    modded: Option<PathBuf>,
    /// `--name NAME`: the game's name, in place of the ROM's file name's.
    name: Option<String>,
}

/// The switches of a recompile: [`FLAGS`], each `--names FILE` in order,
/// the `--names-dir DIR`, the `--readable FILE`, the `--mod DIR`, the
/// `--name NAME` and the `--timing` clock, `clock` if not given: instant for
/// a game's build and a cut (what june plays and mods, 2026-10-30), exact
/// for the printer's checks. `None` if one isn't known.
fn options(args: &[String], clock: Clock) -> Option<Opts> {
    let mut names = Vec::new();
    let mut names_dir = None;
    let mut readable = None;
    let mut modded = None;
    let mut name = None;
    let mut clock = clock;
    let mut flags = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--names" => names.push(PathBuf::from(it.next()?)),
            "--names-dir" => names_dir = Some(PathBuf::from(it.next()?)),
            "--readable" => readable = Some(PathBuf::from(it.next()?)),
            "--mod" => modded = Some(PathBuf::from(it.next()?)),
            "--name" => name = Some(it.next()?.clone()),
            "--timing" => {
                clock = match it.next()?.as_str() {
                    "exact" => Clock::Exact,
                    "static" => Clock::Static,
                    "instant" => Clock::Instant,
                    "native" => Clock::Native,
                    _ => return None,
                }
            }
            f if FLAGS.contains(&f) => flags.push(f),
            _ => return None,
        }
    }
    let has = |f: &str| flags.contains(&f);
    let cfg = Config {
        flag_liveness: !has("--no-flag-liveness"),
        structured: !has("--no-structured"),
        dead_reg_writes: !has("--no-dead-reg-writes"),
        locals: has("--locals"),
        insn: if has("--asm-comments") {
            Insn::Comment
        } else if has("--asm-text") {
            Insn::Asm
        } else {
            Insn::Debug
        },
        callee_saves: has("--callee-saves"),
        readable: false,
        clock,
        idle_exits: false,
    };
    Some(Opts {
        cfg,
        names,
        names_dir,
        readable,
        modded,
        name,
    })
}

fn usage() -> ! {
    eprintln!(
        "usage: gbaa-recompile ROM [OUT] [--name NAME] [--no-flag-liveness] [--no-structured] [--no-dead-reg-writes] [--locals] [--asm-text | --asm-comments] [--callee-saves] [--timing instant|static|exact|native] [--names FILE]... [--names-dir DIR] [--readable FILE] [--mod DIR] | \
         gbaa-recompile ROM [--names FILE]... [--names-dir DIR] [--readable FILE] [flags] --cut DIR FUNC... | \
         gbaa-recompile --find ROM FIELD (--names FILE... | --names-dir DIR) | \
         gbaa-recompile --messages-check ROM JSON | \
         gbaa-recompile --names-check ROM (--names FILE [--names FILE]... | --names-dir DIR) [--readable LIST] [flags] | \
         gbaa-recompile --fn-diff ROM OUT [--hand FILE KEYS] [--names-dir DIR] [FUNCS] | \
         gbaa-recompile --print ROM KEY... | \
         gbaa-recompile --saves ROM [KEY...] | \
         gbaa-recompile --summaries ROM [--names FILE]... [--names-dir DIR] [--show N] [--callee-sets] [KEY...] | \
         gbaa-recompile --snapshots ROM FILE [--names FILE... | --names-dir DIR] [--readable LIST] | \
         gbaa-recompile --sst OUT [PER_FILE [SEQS]] [--readable]"
    );
    std::process::exit(2);
}

/// The switches that turn one transformation off ([`Config`]).
const FLAGS: [&str; 7] = [
    "--no-flag-liveness",
    "--no-structured",
    "--no-dead-reg-writes",
    "--locals",
    "--asm-text",
    "--asm-comments",
    "--callee-saves",
];

/// `--messages-check ROM JSON`: every message the ROM's `[game.index]`
/// table holds against JSON's (`codegen::text::Messages::check`).
fn messages_check(rom: &Path, json: &Path) {
    let fail = |e: String| -> ! {
        eprintln!("gbaa-recompile: {e}");
        std::process::exit(2);
    };
    let rom = std::fs::read(rom).unwrap_or_else(|e| fail(format!("{}: {e}", rom.display())));
    let text =
        std::fs::read_to_string(json).unwrap_or_else(|e| fail(format!("{}: {e}", json.display())));
    let want = codegen::text::parse_json_banks(&text).unwrap_or_else(|e| fail(e));
    let sha = codegen::names::sha1_hex(&rom);
    let Some(m) = codegen::text::Table::messages_for_rom(&sha).unwrap_or_else(|e| fail(e)) else {
        fail(format!("no index table (or its file) for ROM sha1 {sha}"))
    };
    let (ok, bad) = m.check(&rom, &want);
    for (n, got) in bad.iter().take(20) {
        let (b, i) = ((n >> 8) as usize, (n & 0xff) as usize);
        println!("message 0x{n:04x}: want {:?}", want[b][i]);
        println!("               got  {got:?}");
    }
    println!("messages: {ok} match, {} differ", bad.len());
    if !bad.is_empty() {
        std::process::exit(1);
    }
}

/// The source map `paths` layer, for `rom`; `None` with no paths.
fn load_names(rom: &[u8], paths: &[PathBuf]) -> Option<SourceMap> {
    if paths.is_empty() {
        return None;
    }
    let sha = codegen::names::sha1_hex(rom);
    match SourceMap::load(&sha, paths) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("gbaa-recompile: {e}");
            std::process::exit(2);
        }
    }
}

/// The source map of a readable or mod-view print: `paths` layered, or
/// with none, the map files in `dir` that name `rom` by sha1
/// ([`codegen::names::files_for`], the order they layer in). `None` if
/// neither gives a file.
fn mod_view_names(rom: &[u8], paths: &[PathBuf], dir: Option<&Path>) -> Option<SourceMap> {
    if !paths.is_empty() {
        return load_names(rom, paths);
    }
    let dir = dir?;
    let files =
        codegen::names::files_for(dir, &codegen::names::sha1_hex(rom)).unwrap_or_else(|e| {
            eprintln!("gbaa-recompile: {}: {e}", dir.display());
            std::process::exit(2);
        });
    if !files.is_empty() {
        eprintln!(
            "gbaa-recompile: named from {}",
            files
                .iter()
                .map(|f| f.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    load_names(rom, &files)
}

/// `--saves`: how many functions keep r4-r11 and sp ([`codegen::saves`]),
/// why the rest don't, and for each of `keys`, its own answer.
fn saves(rom: &[u8], disc: &Discovery, keys: &[String]) {
    let ctx = Ctx::new(rom, disc);
    let funcs = codegen::emit::build_all(&ctx).funcs;
    let proof = codegen::saves::prove(&ctx, &funcs);
    // The reasons with their addresses taken out, most common first.
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for w in proof.why.values() {
        let kind = w.split_once(": ").map_or(w.as_str(), |(_, k)| k);
        let kind = kind.split(" 0x").next().unwrap_or(kind);
        *kinds.entry(kind.to_string()).or_default() += 1;
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by_key(|(k, n)| (std::cmp::Reverse(*n), k.clone()));
    let called = funcs
        .iter()
        .filter(|f| f.calls.iter().any(|c| proof.kept.contains(c)));
    println!(
        "saves: {} of {} functions keep r4-r11 and sp; {} make a call to one",
        proof.kept.len(),
        funcs.len(),
        called.count()
    );
    for (k, n) in kinds {
        println!("  {n:6}  {k}");
    }
    for k in keys {
        let k = u32::from_str_radix(k.trim_start_matches("0x"), 16).expect("KEY");
        match proof.why.get(&k) {
            Some(w) => println!("0x{k:08x}: {w}"),
            None if proof.kept.contains(&k) => println!("0x{k:08x}: keeps them"),
            None => println!("0x{k:08x}: not a function"),
        }
    }
}

/// `--summaries`: every function's summary against its prototype
/// ([`codegen::saves::compare`]).
fn summaries(rom: &[u8], rest: &[String]) {
    let (mut paths, mut dir, mut show, mut keys) = (Vec::new(), None, 10, Vec::new());
    let mut with_sets = false;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--callee-sets" => with_sets = true,
            "--names" => paths.push(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--names-dir" => dir = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--show" => {
                show = it
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            k => keys.push(
                u32::from_str_radix(k.trim_start_matches("0x"), 16).unwrap_or_else(|_| usage()),
            ),
        }
    }
    let names = mod_view_names(rom, &paths, dir.as_deref());
    let disc = discover(rom.to_vec());
    let ctx = Ctx::new(rom, &disc);
    let funcs = codegen::emit::build_all(&ctx).funcs;
    // Indirect calls with a complete callee set (native/indirect-call-args)
    // read and leave what their callees do: the function addresses
    // followed (codegen::native::fnref), which needs the frames, which
    // need summaries without them.
    let plain = codegen::saves::summarize(&ctx, &funcs);
    let t = std::time::Instant::now();
    let (flow, sets) = if with_sets {
        codegen::native::callee_sets(rom, &disc, &ctx, &funcs, &plain)
    } else {
        Default::default()
    };
    let indirect: std::collections::BTreeSet<u32> = plain
        .by_entry
        .values()
        .flat_map(|s| s.indirect_sites.iter().copied())
        .collect();
    let with_set = indirect
        .iter()
        .filter(|a| flow.sites.contains_key(a))
        .count();
    let complete = indirect.iter().filter(|a| sets.contains_key(a)).count();
    let callees: usize = indirect
        .iter()
        .filter_map(|a| sets.get(a))
        .map(|s| s.len())
        .sum();
    if with_sets {
        println!(
            "callee sets (codegen::native::fnref, {:.1} s{}): {} indirect calls, {with_set} with a callee set, {complete} of them complete (on average {:.1} callees)",
            t.elapsed().as_secs_f64(),
            match (flow.unsettled, flow.over) {
                (true, Some((t, n))) => {
                    format!(", didn't settle: {t} tokens held, over its bound, after {n} runs")
                }
                (true, None) => ", didn't settle".to_string(),
                _ => String::new(),
            },
            indirect.len(),
            callees as f64 / complete.max(1) as f64,
        );
    } else {
        println!(
            "callee sets: not followed (--callee-sets); {} indirect calls, each taken to read r0-r3",
            indirect.len()
        );
    }
    let sums = if with_sets {
        codegen::saves::summarize_sets(&ctx, &funcs, codegen::saves::Assume::default(), &sets)
    } else {
        plain.clone()
    };
    let all = &sums.by_entry;
    let n = |f: &dyn Fn(&codegen::saves::Summary) -> bool| all.values().filter(|s| f(s)).count();
    println!(
        "summaries: {} functions in {} runs; {} keep r4-r11 and sp, {} with no hole; \
         {} read a parameter, {} return a result a caller reads",
        all.len(),
        sums.runs,
        n(&|s| s.keeps),
        n(&|s| s.holes.is_empty() && s.undefined_reads.is_empty()),
        n(&|s| s.params != 0),
        n(&|s| s.results != 0),
    );
    let regs = |m: u16| {
        let v: Vec<String> = (0..16)
            .filter(|r| m & 1 << r != 0)
            .map(|r| format!("r{r}"))
            .collect();
        if v.is_empty() {
            "-".to_string()
        } else {
            v.join(",")
        }
    };
    if let Some(m) = &names {
        // Trusting indirect calls to read only what C passes them.
        let trusting = codegen::saves::Assume { indirect_params: 0 };
        let t = codegen::saves::summarize_with(&ctx, &funcs, trusting);
        let t = codegen::saves::compare(&t, m);
        println!(
            "dwarf, trusting indirect calls to read nothing: {} agree, {} disagree",
            t.agree.len(),
            t.disagree.len()
        );
        for cat in codegen::saves::Category::ALL {
            let n = t
                .disagree
                .iter()
                .filter(|v| v.why.iter().any(|(c, _)| *c == cat));
            println!("  {:6}  {}", n.count(), cat.text());
        }
        if with_sets {
            let p = codegen::saves::compare(&plain, m);
            println!(
                "dwarf, without callee sets (every indirect call reading r0-r3): {} agree, {} disagree",
                p.agree.len(),
                p.disagree.len()
            );
        }
        let c = codegen::saves::compare(&sums, m);
        println!(
            "dwarf: {} agree, {} disagree, {} no prototype, {} prototype unreadable",
            c.agree.len(),
            c.disagree.len(),
            c.no_proto,
            c.unreadable
        );
        for cat in codegen::saves::Category::ALL {
            let hit: Vec<_> = c
                .disagree
                .iter()
                .filter_map(|v| Some((v, v.why.iter().find(|(c, _)| *c == cat)?)))
                .collect();
            println!("  {:6}  {}", hit.len(), cat.text());
            // A sample spread over the list, for checking by hand.
            let step = (hit.len() / show.max(1)).max(1);
            for (v, (_, why)) in hit.iter().step_by(step).take(show) {
                println!(
                    "          0x{:08x} {}: {why}  [{}]",
                    v.entry, v.name, v.proto
                );
            }
        }
    }
    for k in keys {
        let Some(s) = sums.get(k) else {
            println!("0x{k:08x}: not a function");
            continue;
        };
        println!(
            "0x{k:08x}: params {} results {} defines {} read after {} passed after {} keeps {} returns {}",
            regs(s.params as u16),
            regs(s.results),
            regs(s.defines()),
            regs(s.read_after),
            regs(s.passed_after),
            s.keeps,
            s.returns
        );
        for (r, c) in &s.causes {
            match sums.root(k, *r) {
                Some((f, root)) if root != *c => {
                    println!("  r{r}: {c:x?}, from 0x{f:08x}'s {root:x?}")
                }
                _ => println!("  r{r}: {c:x?}"),
            }
        }
        for ((at, r), c) in &s.undefined_reads {
            println!("  reads r{r} at 0x{at:08x}, undefined by {c:x?}");
        }
        for h in &s.holes {
            println!("  hole: {h}");
        }
        if let Some(m) = &names
            && let Some(p) = m.functions.get(&k).and_then(|i| i.proto.as_deref())
        {
            println!("  proto: {p}");
        }
    }
}

/// The readable list at `path` ([`codegen::names::readable_list`]), `*`
/// standing for every function in `all`.
fn read_list(path: &Path, names: Option<&SourceMap>, all: &[u32]) -> BTreeSet<u32> {
    std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|t| codegen::names::readable_list(&t, names, all))
        .unwrap_or_else(|e| {
            eprintln!("gbaa-recompile: {}: {e}", path.display());
            std::process::exit(2);
        })
}

/// `--names-check`: every function printed unnamed and named, compared
/// token by token ([`codegen::names::check`]); with `--readable LIST`,
/// LIST's functions printed readable, their DWARF names and comments too
/// (with `[types]` and prototypes off).
fn names_check(
    rom_path: &Path,
    cfg: Config,
    paths: &[PathBuf],
    dir: Option<&Path>,
    list: Option<&Path>,
) {
    let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{}: {e}", rom_path.display()));
    if paths.is_empty() && dir.is_none() {
        usage()
    }
    let Some(mut names) = mod_view_names(&rom, paths, dir) else {
        eprintln!(
            "gbaa-recompile: --names-check: no map file names {}",
            rom_path.display()
        );
        std::process::exit(2);
    };
    // A typed access (`gTasks.idx(15).next()`) restructures what it
    // folds in, which no token walk inverts: the readable functions are
    // checked as `[types]` would leave them untyped, and so without
    // their prototypes' `arg!`s and `return`s (`codegen::types::Proto`).
    if list.is_some() {
        names.typed = Default::default();
    }
    let t = std::time::Instant::now();
    let disc = discover(rom.clone());
    let all: Vec<u32> = match list {
        Some(_) => codegen::emit::all_entries(&rom, &disc, codegen::variants::FRAMES, cfg)
            .into_iter()
            .collect(),
        None => Vec::new(),
    };
    let readable = list.map(|p| read_list(p, Some(&names), &all));
    let rep = codegen::names::check::check_rom(
        &rom,
        &disc,
        codegen::variants::FRAMES,
        cfg,
        &names,
        readable.as_ref(),
    );
    for (k, e) in rep.failed.iter().take(20) {
        eprintln!("{}: {e}", fn_name(*k));
    }
    println!(
        "names-check: {} functions, {} failed; {} function names and {} addr! constants printed, in {:.1?}",
        rep.funcs,
        rep.failed.len(),
        rep.named.idents,
        rep.named.addrs,
        t.elapsed()
    );
    if readable.is_some() {
        let n = rep.named;
        println!(
            "names-check: readable: of {} DWARF register variables, {} printed by name, {} only as comments, {} neither; comments: {} lines, {} stack slots, {} calls, {} texts, {} text tables, {} byte previews",
            rep.vars,
            n.vars_named,
            n.vars_commented,
            rep.vars.saturating_sub(n.vars_named + n.vars_commented),
            n.lines,
            n.slots,
            n.calls,
            n.texts,
            n.text_tables,
            n.previews
        );
    }
    if !rep.failed.is_empty() {
        std::process::exit(1);
    }
}

/// `--find`: every function printed readable and named, and those whose
/// print reads or writes `field` ([`codegen::find`]).
fn find(rom_path: &Path, o: Opts, field: &str) {
    let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{}: {e}", rom_path.display()));
    let Some(names) = mod_view_names(&rom, &o.names, o.names_dir.as_deref()) else {
        eprintln!("gbaa-recompile: --find: no source map names the ROM");
        std::process::exit(2);
    };
    let disc = discover(rom.clone());
    let all: BTreeSet<u32> = disc.funcs.keys().copied().collect();
    let mut ctx = Ctx::new(&rom, &disc);
    ctx.cfg = o.cfg;
    ctx.names = Some(&names);
    ctx.readable = Some(&all);
    let r = codegen::emit::build_all(&ctx);
    ctx.prove_saves(&r.funcs);
    let hits = codegen::find::find(&ctx, &r.funcs, field);
    print!("{}", codegen::find::report(field, &hits, r.funcs.len()));
    if hits.is_empty() {
        std::process::exit(1);
    }
}

/// `--cut DIR FUNCS`: print FUNCS (keys or names, as `--readable` takes
/// them) readable and named into the mod at DIR
/// ([`codegen::modcopy::cut`]).
fn cut(rom_path: &Path, o: Opts, dir: &Path, funcs: &[String]) {
    let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{}: {e}", rom_path.display()));
    let names = mod_view_names(&rom, &o.names, o.names_dir.as_deref());
    let keys = codegen::names::readable_list(&funcs.join("\n"), names.as_ref(), &[])
        .unwrap_or_else(|e| {
            eprintln!("gbaa-recompile: --cut: {e}");
            std::process::exit(2);
        });
    let disc = discover(rom.clone());
    let all: Vec<u32> = disc.funcs.keys().copied().collect();
    let mut readable = o
        .readable
        .map(|p| read_list(&p, names.as_ref(), &all))
        .unwrap_or_default();
    readable.extend(&keys);
    let mut ctx = Ctx::new(&rom, &disc);
    ctx.cfg = o.cfg;
    ctx.names = names.as_ref();
    ctx.readable = Some(&readable);
    let r = codegen::emit::build_all(&ctx);
    ctx.prove_saves(&r.funcs);
    let picked: Vec<&Func> = r.funcs.iter().filter(|f| keys.contains(&f.entry)).collect();
    if let Some(k) = keys.iter().find(|k| !picked.iter().any(|f| f.entry == **k)) {
        eprintln!("gbaa-recompile: --cut: 0x{k:08x} isn't a function of the ROM");
        std::process::exit(2);
    }
    let commit = std::process::Command::new("git")
        .args([
            "-C",
            &crates_dir().display().to_string(),
            "rev-parse",
            "--short=12",
            "HEAD",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let sha = codegen::names::sha1_hex(&rom);
    let c = codegen::modcopy::cut(&ctx, &picked, dir, &sha, &commit).unwrap_or_else(|e| {
        eprintln!("gbaa-recompile: --cut: {e}");
        std::process::exit(1);
    });
    println!(
        "cut {} functions into {} ({} fresh, {} merged)",
        picked.len(),
        dir.display(),
        c.fresh.len(),
        c.merged.len()
    );
    for (n, conflicts) in &c.merged {
        println!("  merged {n}: {conflicts} conflicts");
    }
    for (k, w) in &c.unchecked {
        println!("  0x{k:08x}: registers stay in Machine: {w}");
    }
    if c.merged.iter().any(|(_, n)| *n > 0) {
        std::process::exit(1);
    }
}

/// Recompile the ROM at `rom_path` into `out`, by default `out/<name>`,
/// the game's name: `--name`'s, or else the ROM's file name's, in the one
/// form ([`runtime::name::GameName`]). The build carries that name.
fn recompile(rom_path: &Path, out: Option<&Path>, o: Opts) {
    let Opts {
        cfg,
        names,
        names_dir,
        readable,
        modded,
        name: game,
    } = o;
    let game = match &game {
        Some(n) => n.clone(),
        None => rom_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let game = runtime::name::GameName(&game).to_string();
    let default_out = Path::new("out").join(&game);
    let out = out.unwrap_or(&default_out);
    let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{}: {e}", rom_path.display()));
    // `--names-dir` names only a readable or mod-view print: the play build
    // stays as it was.
    let names = if readable.is_some() || modded.is_some() {
        mod_view_names(&rom, &names, names_dir.as_deref())
    } else {
        load_names(&rom, &names)
    };
    let t = std::time::Instant::now();
    let disc = discover(rom.clone());
    eprintln!(
        "discovery: {} functions, {} instructions in {:.1?}",
        disc.funcs.len(),
        disc.code.len(),
        t.elapsed()
    );
    // `*` lists every function printed, the IWRAM copies' and the
    // continuation roots' too.
    let all: Vec<u32> = match readable {
        Some(_) => codegen::emit::all_entries(&rom, &disc, codegen::variants::FRAMES, cfg)
            .into_iter()
            .collect(),
        None => Vec::new(),
    };
    let mut readable = readable.map(|p| read_list(&p, names.as_ref(), &all));
    // A mod's functions print readable, as cut (for the check that they
    // still print as cut; they aren't in the parts).
    let modded = modded.map(|d| {
        let m = codegen::modcopy::Mod::load(&d).unwrap_or_else(|e| {
            eprintln!("gbaa-recompile: --mod: {e}");
            std::process::exit(2);
        });
        let sha = codegen::names::sha1_hex(&rom);
        if !m.rom_sha1.eq_ignore_ascii_case(&sha) {
            eprintln!(
                "gbaa-recompile: --mod: {} was cut from ROM {}, not this one",
                d.display(),
                m.rom_sha1
            );
            std::process::exit(2);
        }
        readable
            .get_or_insert_with(Default::default)
            .extend(m.funcs.keys());
        m
    });
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rom".into());
    let frames = codegen::variants::FRAMES;
    let r = write_crate(
        out,
        &name,
        &game,
        &crates_dir(),
        &rom,
        &disc,
        frames,
        cfg,
        names.as_ref(),
        readable.as_ref(),
        modded.as_ref(),
    )
    .unwrap_or_else(|e| {
        eprintln!("gbaa-recompile: {}: {e}", out.display());
        std::process::exit(1);
    });
    let report = std::fs::read_to_string(out.join("report.txt")).unwrap();
    print!("{report}");
    eprintln!(
        "wrote {} ({} functions) in {:.1?}",
        out.display(),
        r.funcs.len(),
        t.elapsed()
    );
}

fn fn_diff(
    rom_path: &Path,
    out: &Path,
    names_dir: Option<&Path>,
    mut opts: codegen::fn_diff::Opts,
) {
    let rom = std::fs::read(rom_path).unwrap_or_else(|e| panic!("{}: {e}", rom_path.display()));
    let rom: &'static [u8] = rom.leak();
    opts.names = mod_view_names(rom, &[], names_dir);
    let t = std::time::Instant::now();
    let disc = discover(rom.to_vec());
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rom".into());
    let what = codegen::fn_diff::write(out, &name, &crates_dir(), rom, &disc, &opts)
        .expect("writing the crate");
    println!("{what}");
    eprintln!("wrote {} in {:.1?}", out.display(), t.elapsed());
}

/// The SingleStepTests crate: `per_file` cases spread evenly over each
/// file, each built as a one-instruction block and lowered as its own fn,
/// and `seqs` sequence cases ([`seq_cases`]).
/// `readable`: each fn printed in the readable form, on the list alone.
fn sst_crate(out: &Path, per_file: usize, seqs: usize, readable: bool) {
    let dir = sst::data_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e} (run `just fetch-tests`)", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".json.bin"))
        .collect();
    files.sort();
    let disc = Discovery::default();
    let src = out.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let (mut mods, mut cases) = (Vec::new(), Vec::new());
    let mut pools = [Pool::default(), Pool::default()];
    for (fi, path) in files.iter().enumerate() {
        let name = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".json.bin")
            .to_string();
        let all = sst::read_file(path);
        for (i, c) in all.iter().enumerate() {
            let thumb = sst::to_cpu(&c.initial).thumb();
            let pool = &mut pools[thumb as usize];
            if i % 8 == 0 {
                pool.opcodes
                    .push(if thumb { c.opcode & 0xFFFF } else { c.opcode });
            }
            if i % 500 == 0 {
                pool.states.push(sst::to_cpu(&c.initial));
            }
        }
        let step = (all.len() / per_file).max(1);
        let mut fns = Vec::new();
        for (ci, c) in all.iter().enumerate().step_by(step).take(per_file) {
            let cpu = sst::to_cpu(&c.initial);
            let (at, thumb) = (c.base_addr, cpu.thumb());
            let instr = if thumb {
                decode_thumb(c.opcode as u16, at)
            } else {
                decode_arm(c.opcode, at)
            };
            let mut b = Builder::new(at, thumb);
            b.begin_insn(at);
            execute(&mut b, at, thumb, instr);
            let next = at.wrapping_add(if thumb { 2 } else { 4 });
            let mut block = b.finish(next);
            block.optimize(Rom::NONE);
            let k = key(at, thumb);
            let gb = GBlock {
                key: k,
                block,
                insns: vec![(at, instr)],
                end: next,
            };
            let f = Func {
                entry: k,
                blocks: BTreeMap::from([(k, gb)]),
                overlay: None,
                variants: Default::default(),
                stubs: Default::default(),
                calls: Default::default(),
            };
            let fname = format_ident!("c{}", ci);
            let one = BTreeSet::from([k]);
            let mut ctx = Ctx::new(&[], &disc);
            ctx.readable = readable.then_some(&one);
            fns.push(lower_func_named(&ctx, &f, &fname));
            let m = format_ident!("f{:02}", fi);
            cases.push(quote!((#name, #ci, #m::#fname)));
        }
        let m = format_ident!("f{:02}", fi);
        let file = quote! {
            use crate::prelude::*;
            #(#fns)*
        };
        write_if_changed(&src.join(format!("{m}.rs")), pretty(file)).unwrap();
        mods.push(m);
    }
    let (seq_mods, seq_list) = seq_cases(&src, &out.join("image.bin"), &pools, seqs, readable);
    mods.extend(seq_mods);
    let head = lib_head(
        quote!(crate::rt),
        {
            let game = codegen::emit::readable_impl(quote!(M<'_>), Clock::Exact);
            quote!(
                pub type M<'a> = crate::Sst<'a>;
                #game
                /// No table: an indirect call always unwinds.
                pub fn call(_m: &mut M<'_>, key: u32) -> Flow {
                    Flow::Jump(key)
                }
                /// Nor a dispatcher: a call that came back elsewhere unwinds.
                pub fn settle(_m: &mut M<'_>, key: u32, _ret: u32, _sp0: u32) -> u32 {
                    key
                }
                /// Nor IRQs taken in the tick: a yield unwinds.
                pub fn yield_at(_m: &mut M<'_>, key: u32) -> Option<u32> {
                    Some(key)
                }
            )
        },
        Clock::Exact,
    );
    let (n, n_seqs) = (cases.len(), seq_list.len());
    let lib = quote! {
        //! Generated by `gbaa-recompile --sst`. Don't edit.
        #![allow(unused, clippy::all, non_snake_case)]
        #head
        pub mod rt;
        pub use rt::{CaseFn, Seq, Sst};
        #(mod #mods;)*
        /// (file, case index, fn).
        pub static CASES: [(&str, usize, CaseFn); #n] = [#(#cases),*];
        pub static SEQS: [Seq; #n_seqs] = [#(#seq_list),*];
    };
    write_if_changed(&src.join("lib.rs"), pretty(lib)).unwrap();
    let harness = crates_dir().join("armv4t/tests/common/sst.rs");
    write_if_changed(
        &src.join("main.rs"),
        include_str!("../../sst-gen/main.rs").replace("HARNESS", &harness.display().to_string()),
    )
    .unwrap();
    write_if_changed(&src.join("rt.rs"), include_str!("../../sst-gen/rt.rs")).unwrap();
    write_if_changed(&src.join("seq.rs"), include_str!("../../sst-gen/seq.rs")).unwrap();
    let path = |c: &str| crates_dir().join(c).display().to_string();
    write_if_changed(
        &out.join("Cargo.toml"),
        format!(
            r#"# Generated by gbaa-recompile --sst. Don't edit.
[package]
name = "gbaa-sst-gen"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
armv4t = {{ path = "{}" }}
runtime = {{ path = "{}" }}

[profile.release]
debug = false
incremental = false
"#,
            path("armv4t"),
            path("runtime")
        ),
    )
    .unwrap();
    eprintln!(
        "wrote {} ({n} cases from {} files, {n_seqs} sequences)",
        out.display(),
        files.len()
    );
}

/// Opcodes and initial states from every file, sampled, for one state
/// (as `sst_ir`'s block test samples them).
#[derive(Default)]
struct Pool {
    opcodes: Vec<u32>,
    states: Vec<Cpu>,
}

/// xorshift64, for reproducible picks.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Where the sequences' code starts in the image, and the room each has.
const SEQ_BASE: u32 = 0x0800_1000;
const SEQ_ROOM: u32 = 0x100;
/// Sequence cases per generated module.
const SEQS_PER_MOD: usize = 250;

/// `b<cond>` from `from` to `to`.
fn branch(thumb: bool, cond: u32, from: u32, to: u32) -> u32 {
    if thumb {
        0xD000 | cond << 8 | (to.wrapping_sub(from + 4) >> 1 & 0xFF)
    } else {
        cond << 28 | 0x0A00_0000 | (to.wrapping_sub(from + 8) >> 2 & 0x00FF_FFFF)
    }
}

/// `cpu` as a constant expression.
fn cpu_expr(c: &Cpu) -> proc_macro2::TokenStream {
    let words = |w: &[u32]| {
        let w = w.iter().map(|&x| hex(x));
        quote!([#(#w),*])
    };
    let (r, spsr) = (words(&c.r), words(&c.spsr));
    let banks = c.banks.iter().map(|b| words(b));
    let (pc, cpsr) = (hex(c.pc), hex(c.cpsr));
    quote!(armv4t::interp::Cpu {
        r: #r,
        pc: #pc,
        cpsr: #cpsr,
        banks: [#(#banks),*],
        spsr: #spsr,
    })
}

/// The sequence cases (`docs/structured-codegen-design.md`, "Validation"):
/// `n` of them, alternately a block of 2 to 6 random opcodes from `pools`
/// and a function of 2 to 4 blocks of 1 to 4, each ending in a `b<cond>` to
/// one of them or to the `bx lr` after the last. Their code goes in an
/// image at the cartridge's start, written to `image_path` for the crate's
/// bus to read; each is built by `build_func` (so with the timing ops,
/// const-prop from the image and flag liveness, as a ROM's code is) and
/// lowered under its entry's name into modules in `src`. The modules and
/// the `Seq` list.
fn seq_cases(
    src: &Path,
    image_path: &Path,
    pools: &[Pool; 2],
    n: usize,
    readable: bool,
) -> (Vec<proc_macro2::Ident>, Vec<proc_macro2::TokenStream>) {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut image = vec![0u8; (SEQ_BASE - ROM_BASE) as usize + n * SEQ_ROOM as usize];
    let mut cases = Vec::new();
    for i in 0..n {
        let thumb = rng.below(2) == 1;
        let pool = &pools[thumb as usize];
        let size = if thumb { 2 } else { 4 };
        let at = SEQ_BASE + i as u32 * SEQ_ROOM;
        let op = |rng: &mut Rng| pool.opcodes[rng.below(pool.opcodes.len())];
        let mut code = Vec::new();
        let kind = if i % 2 == 0 {
            let len = 2 + rng.below(5);
            code.extend((0..len).map(|_| op(&mut rng)));
            "blocks of 2 to 6"
        } else {
            let bodies: Vec<Vec<u32>> = (0..2 + rng.below(3))
                .map(|_| (0..1 + rng.below(4)).map(|_| op(&mut rng)).collect())
                .collect();
            let mut starts = Vec::new();
            let mut a = at;
            for b in &bodies {
                starts.push(a);
                a += size * (b.len() as u32 + 1);
            }
            starts.push(a);
            for b in &bodies {
                code.extend(b);
                let from = at + size * code.len() as u32;
                let to = starts[rng.below(starts.len())];
                // Thumb's cond 1110 is undefined, 1111 a `swi`.
                let cond = rng.below(if thumb { 14 } else { 15 }) as u32;
                code.push(branch(thumb, cond, from, to));
            }
            code.push(if thumb { 0x4770 } else { 0xE12F_FF1E });
            "functions of 2 to 4 blocks"
        };
        let o = (at - ROM_BASE) as usize;
        for (j, w) in code.iter().enumerate() {
            let s = size as usize;
            image[o + j * s..o + (j + 1) * s].copy_from_slice(&w.to_le_bytes()[..s]);
        }
        let mut cpu = pool.states[rng.below(pool.states.len())].clone();
        cpu.pc = at;
        cases.push((kind, at, thumb, cpu, code.len() as u32));
    }
    write_if_changed(image_path, &image).unwrap();

    let (mut mods, mut list) = (Vec::new(), Vec::new());
    for (mi, chunk) in cases.chunks(SEQS_PER_MOD).enumerate() {
        let m = format_ident!("s{:02}", mi);
        let mut fns = Vec::new();
        for &(kind, at, thumb, ref cpu, len) in chunk {
            let size = if thumb { 2 } else { 4 };
            let entry = key(at, thumb);
            let mut disc = Discovery::default();
            disc.funcs.insert(entry, Evidence::Confirmed);
            for j in 0..len {
                let code = Code {
                    size,
                    thumb,
                    evidence: Evidence::Confirmed,
                };
                disc.code.insert(at + size * j, code);
            }
            let one = BTreeSet::from([entry]);
            let mut ctx = Ctx::new(&image, &disc);
            ctx.readable = readable.then_some(&one);
            let f = build_func(&ctx, entry);
            fns.push(lower_func(&ctx, &f));
            let blocks = f.blocks.values().map(|gb| {
                let (a, e) = (hex(gb.key & !1), hex(gb.end));
                quote!((#a, #e))
            });
            let (e, name, cpu) = (hex(entry), fn_name(entry), cpu_expr(cpu));
            list.push(quote!(Seq {
                kind: #kind,
                entry: #e,
                cpu: #cpu,
                blocks: &[#(#blocks),*],
                f: #m::#name,
            }));
        }
        let file = quote! {
            use crate::prelude::*;
            #(#fns)*
        };
        write_if_changed(&src.join(format!("{m}.rs")), pretty(file)).unwrap();
        mods.push(m);
    }
    (mods, list)
}
