//! Written by `gbaa-recompile --sst` from `crates/codegen/sst-gen/main.rs`.
//! Don't edit the copy.
//!
//! Each sampled SingleStepTests case through its generated fn, against the
//! expected state and against the interpreter; then the sequence cases
//! (`seq`). Fails on any case where the two differ.

#[path = "HARNESS"]
#[allow(dead_code)]
mod sst;
mod seq;

use gbaa_sst_gen::{CASES, CaseFn, Sst};
use runtime::dispatch::Flow;
use std::collections::BTreeMap;

fn key(a: u32, thumb: bool) -> u32 {
    if thumb { a | 1 } else { a & !3 }
}

fn run_gen(f: CaseFn, cpu: &mut armv4t::interp::Cpu, bus: &mut sst::TxBus) -> bool {
    let (at, thumb) = (cpu.pc, cpu.thumb());
    let next = at.wrapping_add(if thumb { 2 } else { 4 });
    let mut m = Sst::new(cpu.clone(), bus, 1);
    let Flow::Jump(k) = f(&mut m, key(at, thumb));
    let jumped = m.jumped;
    *cpu = m.cpu;
    if jumped {
        // A dynamic exit: `apply_exit` set pc, keeping what it keeps.
        return true;
    }
    // A static exit (or a fallthrough rekeyed after an `msr`).
    cpu.pc = k & !1;
    cpu.pc != next
}

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut by_file: BTreeMap<&str, Vec<(usize, CaseFn)>> = BTreeMap::new();
    for &(file, i, f) in CASES.iter() {
        by_file.entry(file).or_default().push((i, f));
    }
    let dir = sst::data_dir();
    let (mut total, mut bad) = (0, 0);
    println!("{:<34} {:>6} {:>6} {:>6} {:>6}", "file", "cases", "gen", "interp", "≠");
    for (file, list) in &by_file {
        let all = sst::read_file(&dir.join(format!("{file}.json.bin")));
        let (mut gen_pass, mut int_pass, mut diff) = (0, 0, Vec::new());
        for &(i, f) in list {
            let c = &all[i];
            let g = sst::run_case(c, |cpu, bus, _| run_gen(f, cpu, bus));
            let r = sst::run_case(c, |cpu, bus, raw| cpu.execute(bus, raw));
            gen_pass += matches!(g.outcome, sst::Outcome::Pass) as usize;
            int_pass += matches!(r.outcome, sst::Outcome::Pass) as usize;
            let why = match (&g.end, &r.end) {
                (Some(a), Some(b)) => sst::compare_states(&a.0, &b.0)
                    .map(|(f, d)| format!("{f}: {d}"))
                    .or_else(|| (a.1 != b.1).then(|| format!("bus {:x?} vs {:x?}", a.1, b.1))),
                (None, None) => None,
                (None, _) => Some("generated code panicked".into()),
                (_, None) => Some("the interpreter panicked".into()),
            };
            if let Some(why) = why {
                diff.push(format!("case {i} opcode {:#010x} at {:#x}: {why}", c.opcode, c.base_addr));
            }
        }
        total += list.len();
        bad += diff.len();
        println!("{file:<34} {:>6} {gen_pass:>6} {int_pass:>6} {:>6}", list.len(), diff.len());
        for d in diff.iter().take(3) {
            println!("    {d}");
        }
    }
    println!("{total} cases, {bad} differ from the interpreter");
    let seq_bad = seq::run();
    println!("{} sequences differ from the interpreter", seq_bad);
    if bad + seq_bad > 0 {
        std::process::exit(1);
    }
}
