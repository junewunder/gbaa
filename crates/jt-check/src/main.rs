//! `jt-check ELF [--expect N] [--max-subset N] [--expect-far N] [--show N]
//! [--dump FILE] [--trace ADDR]`: build the truth tables from a pret ELF
//! and the ROM beside it (same name, `.gba`), recover jump tables over the
//! ELF's code map, and print the score. With `--expect N`, exits 1 unless
//! at least N tables are exact, none is wrong, and at most `--max-subset`
//! (default 0) are subsets: a subset is right only if the missing cases are
//! dead, which has to be checked by hand, so a new one fails the check.
//!
//! With `--discover`, the tables, far jumps and code come from code
//! discovery on the ROM ([`analysis::discovery`]) instead of the ELF's code
//! map; it also prints how much of the ELF's code discovery reached.
//!
//! With `--rom ROM` in place of the ELF, a debugging aid: discovery runs
//! on the ROM, and for each `--trace ADDR` it prints the jump's result,
//! then the states near it as each root reaching it sees them (the
//! function holding it, and the continuation entries inside that
//! function), where frame addresses escape in them, the function's far
//! jumps, and every root within 32 KiB that reaches the jump. `--back N`
//! (hex, default 0x40) is how far before ADDR the states start.
//!
//! It also scores the far-jump `bl` rule (decision 5a) on every Thumb `bl`
//! in the code map, against [`FarTruth`]. With `--expect-far N`, exits 1
//! unless at least N are found and none is false.

use std::process::ExitCode;
use std::time::Instant;

use analysis::jumptable::recover_with;
use analysis::program::Program;
use analysis::vsa::func::Bl;
use jt_check::{FarTruth, Score, Truth, Verdict, elf};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "usage: jt-check ELF [--discover] [--expect N] [--max-subset N] [--expect-far N] [--show N] [--dump FILE] [--trace ADDR]\n       jt-check --rom ROM [--trace ADDR]... [--back N]";

fn read(path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}").into())
}

fn run(args: &[String]) -> Result<ExitCode> {
    if let [flag, rom, rest @ ..] = args
        && flag == "--rom"
    {
        return rom_mode(rom, rest);
    }
    let [elf_path, rest @ ..] = args else {
        return Err(USAGE.into());
    };
    let (mut expect, mut show, mut trace, mut dump) = (None, 10, Vec::new(), None);
    let mut max_subset = 0;
    let mut expect_far = None;
    let mut it = rest.iter();
    let mut discover = false;
    while let Some(flag) = it.next() {
        if flag == "--discover" {
            discover = true;
            continue;
        }
        let arg = it.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--expect" => expect = Some(arg.parse()?),
            "--show" => show = arg.parse()?,
            "--max-subset" => max_subset = arg.parse()?,
            "--expect-far" => expect_far = Some(arg.parse()?),
            // A debugging aid: print the fixpoint states of the function
            // holding this address.
            "--trace" => trace.push(u32::from_str_radix(arg.trim_start_matches("0x"), 16)?),
            // Every jump's result, as TSV: address, table, targets.
            "--dump" => dump = Some(arg.clone()),
            _ => return Err(USAGE.into()),
        }
    }
    let rom_path = elf_path
        .strip_suffix(".elf")
        .map(|s| format!("{s}.gba"))
        .ok_or("the ELF's name must end in .elf")?;
    let e = elf::read(&read(elf_path)?).map_err(|e| format!("{elf_path}: {e}"))?;
    let rom = read(&rom_path)?;
    let truth = Truth::build(&e, &rom);
    if discover {
        return discovery_mode(elf_path, &e, rom, &truth, show);
    }
    let prog = Program::new(rom, truth.code.clone(), truth.funcs.clone());
    let t0 = Instant::now();
    let (mut eng, rec) = recover_with(&prog);
    let secs = t0.elapsed().as_secs_f64();
    // The rule is a property of the target, so it is asked of every `bl`,
    // reached by the analyses or not.
    let far_truth = FarTruth::build(&e, &prog);
    let ruled: std::collections::BTreeSet<u32> = far_truth
        .bls
        .iter()
        .filter(|&&(_, t)| eng.far(&prog, (t, true)) == Bl::Jump)
        .map(|&(b, _)| b)
        .collect();
    let (far_found, far_false, far_missed) = far_truth.score(&ruled);
    let score = Score::new(&truth, &rec);
    println!("{elf_path}");
    print!("{}", score.report(show));
    println!(
        "  {} functions, {} instructions reached, {} far-jump bls, {} analyses ({} frozen, {} capped), {secs:.1} s",
        truth.funcs.len(),
        rec.reached,
        rec.far_jumps.len(),
        rec.analyses,
        rec.frozen,
        rec.capped
    );
    println!(
        "  far-jump bls: found {far_found}/{}, false {far_false}, missed {far_missed} ({} bls, {} calls via a stub)",
        far_truth.far.len(),
        far_truth.bls.len(),
        far_truth.stubs
    );
    for &b in far_truth.far.difference(&ruled).take(show) {
        println!("  far jump missed: bl 0x{b:08x}");
    }
    for &b in ruled.difference(&far_truth.far).take(show) {
        println!("  false far jump: bl 0x{b:08x}");
    }
    if let Some(path) = dump {
        let mut out = String::new();
        for (a, r) in &rec.jumps {
            let (kind, j) = match r {
                analysis::vsa::func::JumpRes::Resolved(j) => ("resolved", Some(j)),
                analysis::vsa::func::JumpRes::Rejected(j) => ("rejected", Some(j)),
                analysis::vsa::func::JumpRes::Unresolved => ("unresolved", None),
                analysis::vsa::func::JumpRes::Ram(_) => ("unresolved", None),
            };
            let table = j
                .and_then(|j| j.table)
                .map(|t| format!("0x{:08x}", t.first));
            let targets: Vec<String> = j
                .map(|j| j.targets.iter().map(|t| format!("0x{t:08x}")).collect())
                .unwrap_or_default();
            out += &format!(
                "0x{a:08x}\t{kind}\t{}\t{}\n",
                table.unwrap_or_default(),
                targets.join(",")
            );
        }
        std::fs::write(&path, out).map_err(|e| format!("{path}: {e}"))?;
    }
    for &a in &trace {
        print_trace(&prog, &rec, a);
    }
    let mut ok = true;
    if let Some(n) = expect
        && (score.get(Verdict::Exact) < n
            || score.get(Verdict::Wrong) > 0
            || score.get(Verdict::Subset) > max_subset)
    {
        println!("  FAIL: want at least {n} exact, 0 wrong, at most {max_subset} subset");
        ok = false;
    }
    if let Some(n) = expect_far
        && (far_found < n || far_false > 0)
    {
        println!("  FAIL: want at least {n} far-jump bls found, 0 false");
        ok = false;
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The jump tables, far jumps and code that discovery finds on the ROM,
/// scored against the ELF.
fn discovery_mode(
    elf_path: &str,
    e: &jt_check::Elf,
    rom: Vec<u8>,
    truth: &Truth,
    show: usize,
) -> Result<ExitCode> {
    let t0 = Instant::now();
    let d = analysis::discovery::discover(rom.clone());
    let secs = t0.elapsed().as_secs_f64();
    let prog = Program::new(rom, truth.code.clone(), truth.funcs.clone());
    let score = Score::new(truth, &d.rec);
    println!("{elf_path} (discovery)");
    print!("{}", score.report(show));
    let far_truth = FarTruth::build(e, &prog);
    let reached: std::collections::BTreeSet<u32> = far_truth
        .far
        .iter()
        .copied()
        .filter(|b| d.code.contains_key(b))
        .collect();
    let thumb_bls: std::collections::BTreeSet<u32> = far_truth.bls.iter().map(|b| b.0).collect();
    let found = d.rec.far_jumps.intersection(&reached).count();
    let wrong: Vec<u32> = d
        .rec
        .far_jumps
        .iter()
        .copied()
        .filter(|b| thumb_bls.contains(b) && !far_truth.far.contains(b))
        .collect();
    println!(
        "  far-jump bls reached: found {found}/{}, false {}, missed {} (of {} in the code map)",
        reached.len(),
        wrong.len(),
        reached.len() - found,
        far_truth.far.len()
    );
    for b in reached.difference(&d.rec.far_jumps).take(show) {
        println!("  far jump missed: bl 0x{b:08x}");
    }
    for b in wrong.iter().take(show) {
        println!("  false far jump: bl 0x{b:08x}");
    }
    // Coverage: instructions of the ELF's code map that discovery reached.
    let mut total = 0usize;
    for r in prog.regions() {
        let mut a = r.start;
        while a < r.end {
            let Some(i) = prog.decode(a, r.thumb) else {
                break;
            };
            total += 1;
            a = i.next();
        }
    }
    let inside = d
        .code
        .iter()
        .filter(|(a, c)| prog.is_code(**a, c.thumb))
        .count();
    println!(
        "  code: reached {inside} of {total} instructions in the code map ({:.1}%), {} outside it",
        100.0 * inside as f64 / total.max(1) as f64,
        d.code.len() - inside
    );
    println!(
        "  {} functions found, {} far-jump bls, {} analyses, {secs:.1} s",
        d.funcs.len(),
        d.rec.far_jumps.len(),
        d.rec.analyses
    );
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(&args).unwrap_or_else(|e| {
        eprintln!("jt-check: {e}");
        ExitCode::FAILURE
    })
}

fn hex(s: &str) -> Result<u32> {
    Ok(u32::from_str_radix(s.trim_start_matches("0x"), 16)?)
}

/// `--rom ROM [--trace ADDR]... [--back N]` (see the module docs).
fn rom_mode(rom_path: &str, rest: &[String]) -> Result<ExitCode> {
    let (mut trace, mut back) = (Vec::new(), 0x40);
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let arg = it.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--trace" => trace.push(hex(arg)?),
            "--back" => back = hex(arg)?,
            _ => return Err(USAGE.into()),
        }
    }
    let rom = read(rom_path)?;
    let t0 = Instant::now();
    let d = analysis::discovery::discover(rom.clone());
    println!(
        "{rom_path}: {} functions, {} entries, {:.1} s",
        d.funcs.len(),
        d.entries.len(),
        t0.elapsed().as_secs_f64()
    );
    let starts: Vec<u32> = d.funcs.keys().copied().collect();
    let end = analysis::program::ROM_BASE + rom.len() as u32;
    let funcs = starts
        .iter()
        .enumerate()
        .map(|(i, &s)| analysis::program::Func {
            start: s & !1,
            end: starts.get(i + 1).map_or(end, |n| n & !1),
            thumb: s & 1 != 0,
        })
        .collect();
    let mut prog = Program::new(rom, Vec::new(), funcs);
    prog.set_data(d.data.clone());
    for &a in &trace {
        println!("0x{a:08x}: {:?}", d.rec.jumps.get(&a));
        let Some(f) = prog.func_at(a).copied() else {
            println!("  in no function");
            continue;
        };
        let roots = std::iter::once(f.start).chain(
            d.entries
                .keys()
                .map(|&e| e & !1)
                .filter(|&e| f.start < e && e < f.end),
        );
        for root in roots.collect::<Vec<_>>() {
            print_states(&prog, &d.rec, root, f.thumb, a, back);
        }
        let fj: Vec<String> = d
            .rec
            .far_jumps
            .range(f.start..f.end.saturating_add(0x2000))
            .map(|b| format!("0x{b:08x}"))
            .collect();
        println!(
            "  function 0x{:08x}..0x{:08x}, far jumps {fj:?}",
            f.start, f.end
        );
        // Every root near it that reaches the jump, or exits into its
        // function.
        let far = far_answers(&prog, &d.rec, f.thumb);
        let near = d.funcs.keys().chain(d.entries.keys()).map(|&k| k & !1);
        for root in near.filter(|&k| k.abs_diff(a) < 0x8000).collect::<Vec<_>>() {
            let r = analysis::vsa::func::analyze(&prog, root, f.thumb, &mut |t, _| far(t));
            let exits: Vec<_> = r
                .exits
                .iter()
                .filter(|(_, t)| (f.start..f.end).contains(&(t & !1)))
                .map(|(x, t)| format!("0x{x:08x}->0x{t:08x}"))
                .collect();
            if r.jumps.contains_key(&a) || !exits.is_empty() {
                println!(
                    "  root 0x{root:08x}: {:?}, exits into the function {exits:?}",
                    r.jumps
                        .get(&a)
                        .map(|j| matches!(j, analysis::vsa::func::JumpRes::Resolved(_)))
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_trace(prog: &Program, rec: &analysis::jumptable::Recovery, a: u32) {
    let Some(f) = prog.func_at(a).copied() else {
        println!("0x{a:08x}: in no function");
        return;
    };
    println!(
        "trace 0x{a:08x}: function 0x{:08x}..0x{:08x}",
        f.start, f.end
    );
    print_states(prog, rec, f.start, f.thumb, a, 0x40);
}

/// The far-jump answers `rec` took, by `bl` target.
fn far_answers(
    prog: &Program,
    rec: &analysis::jumptable::Recovery,
    thumb: bool,
) -> impl Fn(u32) -> Bl {
    let far_targets: std::collections::HashSet<u32> = rec
        .far_jumps
        .iter()
        .filter_map(|&b| prog.decode(b, thumb).and_then(|i| i.bl))
        .collect();
    move |t| {
        if far_targets.contains(&t) {
            Bl::Jump
        } else {
            Bl::Call
        }
    }
}

/// The fixpoint states from `root` at `a - back..=a + 4`, and the jump
/// result there, the far jumps taken as `rec` says.
fn print_states(
    prog: &Program,
    rec: &analysis::jumptable::Recovery,
    root: u32,
    thumb: bool,
    a: u32,
    back: u32,
) {
    let far = far_answers(prog, rec, thumb);
    let mut far = |t: u32, _| far(t);
    let r = analysis::vsa::func::analyze(prog, root, thumb, &mut far);
    println!(
        "from 0x{root:08x}: {:?}, {} reached, capped {}",
        r.jumps.get(&a),
        r.reached,
        r.capped
    );
    let states = analysis::vsa::func::states(prog, root, thumb, &mut far);
    // Where frame addresses escape: the instructions whose step lowers
    // the escaped offset, and the merge points whose join does. (A `bl`
    // isn't one step: its return site's state is compared.)
    let sp0 = analysis::vsa::exec::State::sp0(root);
    let lower = |a: Option<i32>, b: Option<i32>| match (a, b) {
        (Some(x), Some(y)) => x < y,
        (Some(_), None) => true,
        _ => false,
    };
    let mut lost = Vec::new();
    for (at, st) in &states {
        let Some(i) = prog.decode(*at, thumb) else {
            continue;
        };
        if let Some(_) = i.bl {
            if let Some(n) = states.get(&i.next())
                && lower(n.escaped, st.escaped)
            {
                lost.push(format!("0x{at:08x} bl {:?}", n.escaped));
            }
            continue;
        }
        // What the instruction escapes by itself.
        let mut clean = st.clone();
        clean.escaped = None;
        let c = analysis::vsa::exec::step(prog, sp0, 0, &i, clean);
        let own = c
            .fall
            .iter()
            .chain(c.exits.iter().map(|e| &e.state))
            .filter_map(|x| x.escaped)
            .min();
        if let Some(e) = own {
            lost.push(format!("0x{at:08x} escapes {e}"));
        }
        let s = analysis::vsa::exec::step(prog, sp0, 0, &i, st.clone());
        let mut outs: Vec<(u32, &analysis::vsa::exec::State)> = Vec::new();
        if let Some(f) = &s.fall {
            outs.push((i.next(), f));
        }
        for e in &s.exits {
            if let armv4t::decode::Instr::Branch { target, .. } = i.instr {
                outs.push((target, &e.state));
            }
        }
        for (to, o) in outs {
            if lower(o.escaped, st.escaped) {
                lost.push(format!("0x{at:08x} {:?}", o.escaped));
            } else if let Some(n) = states.get(&to)
                && lower(n.escaped, o.escaped)
                && !lost
                    .iter()
                    .any(|l| l.starts_with(&format!("join 0x{to:08x}")))
            {
                lost.push(format!("join 0x{to:08x} {:?} from 0x{at:08x}", n.escaped));
            }
        }
    }
    lost.truncate(24);
    println!("  frame escapes: {lost:?}");
    for (at, st) in states.range(a.saturating_sub(back)..=a + 4) {
        let insn = prog.decode(*at, thumb).map(|i| format!("{:?}", i.instr));
        println!("0x{at:08x} {}", insn.unwrap_or_default());
        for (r, v) in st.regs.iter().enumerate() {
            println!(
                "    r{r:<2} {:?} sym {:x} off {:#x} der {:?}",
                v.dom,
                v.sym.0,
                v.off,
                v.derived.map(|d| (d.sym.0, d.off, d.op))
            );
        }
        println!("    flags {:?}", st.flags);
        println!(
            "    stack {:?} escaped {:?}",
            st.stack
                .iter()
                .map(|(k, v)| (*k, &v.dom, v.sym.0))
                .collect::<Vec<_>>(),
            st.escaped
        );
    }
}
