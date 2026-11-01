//! The crate `fn-diff` runs (`gbaa-recompile --fn-diff`): a sample of a
//! ROM's functions, each compiled in every configuration
//! ([`crate::func::Config`]) with the functions it calls, and the states
//! they were entered with in an interpreted run. Design:
//! `docs/structured-codegen-design.md`, "Validation"; the runner is the
//! `fn-diff` crate.
//!
//! The forms: the reference (the default configuration), then each
//! transformation switched from its default, then the readable form of the
//! picks, then the reference with one byte of one function's code changed, which the runs must catch
//! (gbarecomp's injected-fault gate, `docs/research/codegen-validation.md`).

use crate::emit::{GENERATED, PACKAGE, build_all, lib_head, pretty, write_if_changed};
use crate::func::{Clock, Config, Ctx, Func, build_func};
use crate::lower::{fn_name, hex, lower_func};
use analysis::discovery::Discovery;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What to pick and record.
pub struct Opts {
    /// Functions to pick, at most.
    pub funcs: usize,
    /// Frames of the interpreted run the states are recorded in.
    pub frames: u32,
    /// States per function, at most: its entries spread over the run.
    pub samples: u64,
    /// Functions a pick may need compiled with it (itself and every
    /// function it calls, directly or not), at most.
    pub max_closure: usize,
    /// Hand-written forms of some functions (the readable-form spike,
    /// `docs/mod-view-design.md`): a file of `f_<key>` fns, and their keys.
    /// Then only these are picked, and a `hand` form compiles the file in
    /// place of their generated code. One never entered in the run
    /// borrows the first recorded state, its entry changed.
    pub hand: Option<(std::path::PathBuf, BTreeSet<u32>)>,
    /// The source map the readable form is printed named and typed from
    /// (`docs/mod-view-design.md`), if any.
    pub names: Option<crate::names::SourceMap>,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            funcs: 200,
            frames: 600,
            samples: 3,
            max_closure: 40,
            hand: None,
            names: None,
        }
    }
}

/// A hand file's code as tokens, each of its `include!("FILE");` lines
/// replaced by FILE (beside it).
fn hand_tokens(p: &Path) -> std::io::Result<proc_macro2::TokenStream> {
    let text = std::fs::read_to_string(p)?;
    let mut out = String::new();
    for line in text.lines() {
        match line
            .trim()
            .strip_prefix("include!(\"")
            .and_then(|r| r.strip_suffix("\");"))
        {
            Some(f) => out.push_str(&std::fs::read_to_string(p.with_file_name(f))?),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out.parse()
        .map_err(|e| std::io::Error::other(format!("doesn't lex: {e}")))
}

/// The configurations compared with the reference, each one
/// transformation switched from its default, named.
pub fn configs() -> Vec<(&'static str, Config)> {
    vec![
        (
            "liveness",
            Config {
                flag_liveness: false,
                ..Config::default()
            },
        ),
        (
            "dead_regs",
            Config {
                dead_reg_writes: false,
                ..Config::default()
            },
        ),
        (
            "v1",
            Config {
                structured: false,
                ..Config::default()
            },
        ),
        // The switches that are off by default, on.
        (
            "locals",
            Config {
                locals: true,
                ..Config::default()
            },
        ),
        // Reloads dropped after calls to functions that keep r4-r11 and
        // sp (`crate::saves`), which only locals have.
        (
            "saves",
            Config {
                locals: true,
                callee_saves: true,
                ..Config::default()
            },
        ),
    ]
}

/// `f` and every function it calls, directly or not, if they number at
/// most `max` and all have code.
fn closure(funcs: &BTreeMap<u32, &Func>, f: u32, max: usize) -> Option<BTreeSet<u32>> {
    let mut seen = BTreeSet::from([f]);
    let mut work = vec![f];
    while let Some(e) = work.pop() {
        for &c in &funcs.get(&e)?.calls {
            if seen.insert(c) {
                if seen.len() > max {
                    return None;
                }
                work.push(c);
            }
        }
    }
    Some(seen)
}

/// Write the crate to `dir`, named for `name`. `crates` holds the repo's
/// crates. Returns what it picked and the function it faulted, if any.
pub fn write(
    dir: &Path,
    name: &str,
    crates: &Path,
    rom: &'static [u8],
    disc: &Discovery,
    opts: &Opts,
) -> std::io::Result<String> {
    let name = &crate::emit::crate_name(name);
    // Only ever replace a crate this wrote: OUT could be anything.
    let toml = std::fs::read_to_string(dir.join("Cargo.toml"));
    if toml.as_ref().is_ok_and(|c| !c.starts_with(GENERATED)) {
        return Err(std::io::Error::other(format!(
            "{} holds a crate gbaa-recompile didn't write",
            dir.display()
        )));
    }
    let ctx = Ctx::new(rom, disc);
    let all = build_all(&ctx);
    let funcs: BTreeMap<u32, &Func> = all.funcs.iter().map(|f| (f.entry, f)).collect();
    // Discovery's functions (not its continuation roots), with their
    // closures.
    let cands: BTreeMap<u32, BTreeSet<u32>> = disc
        .funcs
        .keys()
        .map(|&a| crate::func::key(a & !1, a & 1 != 0))
        .filter_map(|k| Some((k, closure(&funcs, k, opts.max_closure)?)))
        .collect();
    let keys: BTreeSet<u32> = cands.keys().copied().collect();
    let hand_keys = opts.hand.as_ref().map(|h| h.1.clone());
    let mut hits = fn_diff::hits(rom, &keys, opts.frames);
    if let Some(h) = &hand_keys {
        // A borrowed state runs on its lender's stack and lr: eight
        // lenders, Thumb as every hand function is.
        let lenders: BTreeSet<u32> = hits
            .keys()
            .copied()
            .filter(|k| k & 1 != 0 && !h.contains(k))
            .take(8)
            .collect();
        hits.retain(|k, _| h.contains(k) || lenders.contains(k));
    }
    let entered: Vec<(u32, u64)> = hits.into_iter().collect();
    let step = entered.len().div_ceil(opts.funcs.max(1)).max(1);
    let picks: BTreeMap<u32, Vec<u64>> = entered
        .iter()
        .step_by(step)
        .map(|&(k, n)| {
            let mut at: Vec<u64> = (0..opts.samples).map(|j| j * n / opts.samples).collect();
            at.dedup();
            (k, at)
        })
        .collect();
    // Hand mode: every entered function is picked (step 1), the lender
    // too; the hand keys never entered borrow its states below.
    let mut picks = picks;
    if let Some(h) = &hand_keys {
        for &k in h {
            picks.entry(k).or_default();
        }
    }
    let mut states = fn_diff::record(rom, &picks, opts.frames);
    if let Some(h) = &hand_keys {
        let lent: Vec<fn_diff::State> = states
            .iter()
            .filter(|s| !h.contains(&s.entry))
            .cloned()
            .collect();
        states.retain(|s| h.contains(&s.entry));
        for &k in h {
            if !states.iter().any(|s| s.entry == k) {
                states.extend(lent.iter().map(|s| fn_diff::State {
                    entry: k,
                    ..s.clone()
                }));
            }
        }
    }
    let picks: BTreeMap<u32, Vec<u64>> = match &hand_keys {
        Some(h) => picks.into_iter().filter(|(k, _)| h.contains(k)).collect(),
        None => picks,
    };
    std::fs::create_dir_all(dir.join("src"))?;
    let states_path = std::path::absolute(dir.join("states.bin"))?;
    fn_diff::write_states(&states_path, &states)?;
    let needed: BTreeSet<u32> = picks.keys().flat_map(|k| &cands[k]).copied().collect();

    let hash = proc_macro2::Literal::u64_unsuffixed(runtime::dispatch::rom_hash(rom));
    // Every form dispatches at the reference's keys only: a form with more
    // (v1, every block start a key) would run compiled where the reference
    // resumes in the interpreter, which ticks each instruction, not each
    // block, and the runs would differ in that alone.
    let ref_keys: BTreeSet<u32> = {
        let fs: Vec<Func> = needed.iter().map(|&e| build_func(&ctx, e)).collect();
        let structured = |f: &Func| crate::lower::structured(&ctx, f).is_some();
        crate::emit::keys(&fs, structured).0.into_keys().collect()
    };
    let module = |ctx: &Ctx, hand: Option<&Path>| {
        let fs: Vec<Func> = needed.iter().map(|&e| build_func(ctx, e)).collect();
        let structured = |f: &Func| crate::lower::structured(ctx, f).is_some();
        let (mut table, _) = crate::emit::keys(&fs, structured);
        table.retain(|k, _| ref_keys.contains(k));
        assert_eq!(
            table.len(),
            ref_keys.len(),
            "a form without a reference key"
        );
        let n = table.len();
        let entries = table.iter().map(|(&k, &f)| {
            let (k, f) = (hex(k), ctx.fn_ident(f));
            quote!((#k, #f))
        });
        let fns = fs
            .iter()
            .filter(|f| hand.is_none() || !hand_keys.as_ref().unwrap().contains(&f.entry))
            .map(|f| crate::lower::lower_func_named(ctx, f, &ctx.fn_ident(f.entry)));
        // Pasted in, not `include!`d: macros an `include!` defines can't
        // shadow the generated prelude's of the same name.
        let hand = hand.map(|p| hand_tokens(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())));
        let fn_keys = crate::emit::keys_module(fs.iter().map(|f| (ctx.fn_ident(f.entry), f.entry)));
        let head = lib_head(
            quote!(runtime::dispatch),
            {
                let game = crate::emit::readable_impl(quote!(M), Clock::Exact);
                quote!(
                    pub type M = runtime::Machine<runtime::platform::Headless>;
                    #game
                    #fn_keys
                    /// An indirect call through this form's table.
                    #[inline]
                    pub fn call(m: &mut M, key: u32) -> Flow {
                        rt::call(m, &super::CODE, key)
                    }
                    /// A call run on to its return site through this form's table.
                    #[inline]
                    pub fn settle(m: &mut M, key: u32, ret: u32, sp0: u32) -> u32 {
                        rt::settle(m, &super::CODE, key, ret, sp0)
                    }
                    /// A tick found an IRQ takeable or the CPU halted at `key`: take it
                    /// here (`rt::yield_at`); `Some` to unwind with.
                    #[inline]
                    pub fn yield_at(m: &mut M, key: u32) -> Option<u32> {
                        rt::yield_at(m, &super::CODE, key)
                    }
                )
            },
            Clock::Exact,
        );
        let names = crate::emit::names_module(ctx.names, false);
        let globals = ctx.names.map(|_| {
            quote!(
                use globals::*;
            )
        });
        pretty(quote! {
            //! Generated by `gbaa-recompile --fn-diff`. Don't edit.
            #![allow(unused, clippy::all, non_snake_case)]
            #head
            use prelude::*;
            #names
            #globals
            #(#fns)*
            #hand
            /// The reference's keys in these functions, sorted.
            pub static BLOCKS: [(u32, runtime::dispatch::GuestFn<runtime::platform::Headless>); #n] =
                [#(#entries),*];
            pub static CODE: runtime::dispatch::Code<runtime::platform::Headless> =
                runtime::dispatch::Code {
                    blocks: &BLOCKS,
                    ram_blocks: &[],
                    extents: &[],
                    ram_extents: &[],
                    inner: &[],
                    inner_extents: &[],
                    modded: &[],
                    folded: &[],
                    overlays: &[],
                    rom_hash: #hash,
                    timing: runtime::Timing::Exact,
                    seeds_file: None,
                    spins: &[],
                };
        })
    };
    let src = dir.join("src");
    let mut forms = vec![("reference".to_string(), module(&ctx, None))];
    let mut kept = None;
    for (n, cfg) in configs() {
        let mut c = Ctx::new(rom, disc);
        c.cfg = cfg;
        if cfg.callee_saves {
            c.prove_saves(&build_all(&c).funcs);
            let held = needed.iter().filter(|k| c.kept.contains(k)).count();
            let calls = picks
                .keys()
                .filter(|k| funcs[k].calls.iter().any(|t| c.kept.contains(t)));
            kept = Some((c.kept.len(), all.funcs.len(), held, calls.count()));
        }
        forms.push((n.to_string(), module(&c, None)));
    }
    // The readable form (`docs/mod-view-design.md`): the picks on the
    // readable list, their callees as the reference prints them.
    let picked: BTreeSet<u32> = picks.keys().copied().collect();
    let mut c = Ctx::new(rom, disc);
    c.readable = Some(&picked);
    c.names = opts.names.as_ref();
    c.prove_saves(&build_all(&c).funcs);
    forms.push(("readable".into(), module(&c, None)));
    if let Some((p, _)) = &opts.hand {
        forms.push(("hand".into(), module(&ctx, Some(&std::path::absolute(p)?))));
    }
    // The fault: the first pick whose code has one exit, which every run
    // of it takes, its cycle count one byte off.
    const CYCLES: &str = "let cycles: u32 = ";
    let reference = &forms[0].1;
    let fault = picks.keys().find_map(|&k| {
        let text = pretty(lower_func(&ctx, funcs[&k]));
        (text.matches(CYCLES).count() == 1).then_some(k)
    });
    if let Some(k) = fault {
        let at = reference.find(&format!("pub fn {}(", fn_name(k))).unwrap();
        let at = at + reference[at..].find(CYCLES).unwrap() + CYCLES.len();
        let mut bytes = reference.clone().into_bytes();
        bytes[at] = if bytes[at] == b'9' {
            b'8'
        } else {
            bytes[at] + 1
        };
        forms.push(("fault".into(), String::from_utf8(bytes).unwrap()));
    }
    for (n, text) in &forms {
        write_if_changed(&src.join(format!("{n}.rs")), text)?;
    }
    let mods = forms.iter().map(|(n, _)| format_ident!("{}", n));
    let form_list = forms.iter().map(|(n, _)| {
        let m = format_ident!("{}", n);
        quote!(fn_diff::Form { name: #n, code: &#m::CODE })
    });
    let n_forms = forms.len();
    let picked = picks.keys().map(|&k| hex(k));
    let fault_tok = match fault {
        Some(k) => {
            let k = hex(k);
            quote!(Some(#k))
        }
        None => quote!(None),
    };
    let states_str = states_path.display().to_string();
    let main = quote! {
        //! Generated by `gbaa-recompile --fn-diff`. Don't edit.
        //! diff-NAME ROM [--exact]: the per-function differential runs.
        #(mod #mods;)*
        static FORMS: [fn_diff::Form; #n_forms] = [#(#form_list),*];
        fn main() -> std::process::ExitCode {
            fn_diff::main(&fn_diff::Spec {
                forms: &FORMS,
                picked: &[#(#picked),*],
                fault: #fault_tok,
                states: #states_str,
            })
        }
    };
    write_if_changed(&src.join("main.rs"), pretty(main))?;
    let path = |c: &str| crates.join(c).display().to_string();
    let cargo = format!(
        "{GENERATED}[package]\nname = \"gbaa-{name}\"\n{PACKAGE}\
         # Its own workspace: not a member of gbaa's.\n[workspace]\n\n[dependencies]\n\
         armv4t = {{ path = \"{}\" }}\nruntime = {{ path = \"{}\", features = [\"trace\"] }}\n\
         fn-diff = {{ path = \"{}\" }}\n\n[profile.release]\ndebug = false\nincremental = false\n",
        path("armv4t"),
        path("runtime"),
        path("fn-diff"),
    );
    write_if_changed(&dir.join("Cargo.toml"), cargo)?;
    let n_states = states.len();
    let kept = kept.map_or(String::new(), |(k, n, held, calls)| {
        format!(
            "; saves: {k} of {n} functions keep r4-r11 and sp, {held} of those compiled, \
             {calls} picks call one"
        )
    });
    Ok(format!(
        "fn-diff: {} candidates entered, {} picked, {} compiled with them, {n_states} states, \
         forms {}; fault in {}{kept}",
        entered.len(),
        picks.len(),
        needed.len(),
        forms
            .iter()
            .map(|f| f.0.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        fault.map_or("none".into(), |k| fn_name(k).to_string())
    ))
}
