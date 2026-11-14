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

const USAGE: &str = "usage: jt-check ELF [--discover] [--expect N] [--max-subset N] [--expect-far N] [--show N] [--dump FILE] [--trace ADDR]";

fn read(path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}").into())
}

fn run(args: &[String]) -> Result<ExitCode> {
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

fn print_trace(prog: &Program, rec: &analysis::jumptable::Recovery, a: u32) {
    let Some(f) = prog.func_at(a).copied() else {
        println!("0x{a:08x}: in no function");
        return;
    };
    let far_targets: std::collections::HashSet<u32> = rec
        .far_jumps
        .iter()
        .filter_map(|&b| prog.decode(b, f.thumb).and_then(|i| i.bl))
        .collect();
    let states = analysis::vsa::func::states(prog, f.start, f.thumb, &mut |t, _| {
        if far_targets.contains(&t) {
            Bl::Jump
        } else {
            Bl::Call
        }
    });
    println!(
        "trace 0x{a:08x}: function 0x{:08x}..0x{:08x}",
        f.start, f.end
    );
    for (at, st) in states.range(a.saturating_sub(0x40)..=a + 4) {
        let insn = prog.decode(*at, f.thumb).map(|i| format!("{:?}", i.instr));
        println!("0x{at:08x} {}", insn.unwrap_or_default());
        for (r, v) in st.regs.iter().enumerate() {
            println!(
                "    r{r:<2} {:?} sym {:x} off {:#x}",
                v.dom,
                v.sym.0 & 0xffff,
                v.off
            );
        }
        println!("    flags {:?}", st.flags);
        println!(
            "    stack {:?}",
            st.stack
                .iter()
                .map(|(k, v)| (*k, &v.dom))
                .collect::<Vec<_>>()
        );
    }
}
