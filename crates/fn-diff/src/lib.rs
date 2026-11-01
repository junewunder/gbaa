//! Per-function differential runs (`docs/structured-codegen-design.md`,
//! "Validation"; VA 4 of `docs/research/codegen-validation.md`): the same
//! guest functions compiled in two configurations, run from the same real
//! entry states, must make the same bus accesses and ticks in the same
//! order and end in the same state.
//!
//! `gbaa-recompile --fn-diff` picks the functions, records their entry
//! states ([`record`], written with [`write_states`]) and writes a crate
//! holding every form of them; its `main` calls [`main`]. Each state is run
//! under [`MUTATIONS`] changes (all 16 NZCV values, randomised scratch
//! state, perturbed arguments) in every form, from the entry to the
//! function's return or its first yield, whichever comes first.
//!
//! A yield stops the run because the IRQ that follows would save the
//! CPSR, and flag liveness leaves the CPSR's flags stale there until the
//! yield-path fix (step 2 of the design): at a yield the flags are excused
//! and counted, unless `--exact`.

use armv4t::interp::Cpu;
use runtime::Machine;
use runtime::dispatch::{self, Code, Flow};
use runtime::machine::Fault;
use runtime::platform::Headless;
use runtime::trace;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;

pub type M = Machine<Headless>;

/// A function's entry state, recorded in the interpreter: its key, which
/// of its entries it was (counting from 0), and the machine's payload
/// ([`Machine::save_payload`]).
#[derive(Clone)]
pub struct State {
    pub entry: u32,
    pub hit: u64,
    pub payload: Vec<u8>,
}

/// What a states file starts with.
const MAGIC: &[u8; 8] = b"GBAAfdif";

/// Write `states` to `path`: [`MAGIC`], then each state's entry, hit and
/// payload length, little-endian, and its payload.
pub fn write_states(path: &Path, states: &[State]) -> io::Result<()> {
    let mut f = io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(MAGIC)?;
    for s in states {
        f.write_all(&s.entry.to_le_bytes())?;
        f.write_all(&s.hit.to_le_bytes())?;
        f.write_all(&(s.payload.len() as u32).to_le_bytes())?;
        f.write_all(&s.payload)?;
    }
    f.flush()
}

/// Read what [`write_states`] wrote.
pub fn read_states(path: &Path) -> io::Result<Vec<State>> {
    let mut b = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut b)?;
    let bad = || io::Error::other(format!("{}: not a states file", path.display()));
    if !b.starts_with(MAGIC) {
        return Err(bad());
    }
    let mut at = MAGIC.len();
    let mut take = |n: usize| -> io::Result<&[u8]> {
        let s = b.get(at..at + n).ok_or_else(bad)?;
        at += n;
        Ok(s)
    };
    let mut out = Vec::new();
    while let Ok(e) = take(4) {
        let entry = u32::from_le_bytes(e.try_into().unwrap());
        let hit = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let n = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
        let payload = take(n)?.to_vec();
        out.push(State {
            entry,
            hit,
            payload,
        });
    }
    Ok(out)
}

/// Run `rom` in the interpreter for `frames` frames with no input, calling
/// `at` with the key at each instruction boundary (after any IRQ is taken).
fn walk(rom: &'static [u8], frames: u32, mut at: impl FnMut(&mut M, u32)) {
    let mut m = Box::new(Machine::new(rom, Headless));
    for _ in 0..frames {
        m.sched.frame_ended = false;
        while !m.sched.frame_ended && m.fault.is_none() {
            if m.sched.halted {
                m.idle();
                continue;
            }
            m.take_irq();
            let k = dispatch::key(&m.cpu);
            at(&mut m, k);
            m.step();
        }
        if m.fault.is_some() {
            break;
        }
    }
}

/// How many times each of `entries` (keys) is entered in `frames` frames.
pub fn hits(rom: &'static [u8], entries: &BTreeSet<u32>, frames: u32) -> BTreeMap<u32, u64> {
    let mut n = BTreeMap::new();
    walk(rom, frames, |_, k| {
        if entries.contains(&k) {
            *n.entry(k).or_insert(0) += 1;
        }
    });
    n
}

/// The states at the entries `picks` names: for each key, which of its
/// entries (as [`hits`] counts them, from 0). Same ROM and frames as the
/// [`hits`] it was picked from.
pub fn record(rom: &'static [u8], picks: &BTreeMap<u32, Vec<u64>>, frames: u32) -> Vec<State> {
    let mut n: BTreeMap<u32, u64> = BTreeMap::new();
    let mut out = Vec::new();
    walk(rom, frames, |m, k| {
        let Some(want) = picks.get(&k) else {
            return;
        };
        let hit = n.entry(k).or_insert(0);
        if want.contains(hit) {
            let mut payload = Vec::new();
            m.save_payload(&mut |b| payload.extend_from_slice(b));
            out.push(State {
                entry: k,
                hit: *hit,
                payload,
            });
        }
        *hit += 1;
    });
    out
}

/// How many ways each state is changed before it runs.
pub const MUTATIONS: usize = 22;

/// What mutation `i` does, for reports.
pub fn mutation_name(i: usize) -> String {
    match i {
        0..16 => format!("nzcv={i:04b}"),
        16 => "random r12 and flags".into(),
        17..21 => format!("r{}+1", i - 17),
        _ => "random r0-r3".into(),
    }
}

/// Change `cpu` by mutation `i`, drawing from `seed`.
fn mutate(cpu: &mut Cpu, i: usize, seed: u64) {
    let mut rng = Rng(seed | 1);
    match i {
        0..16 => cpu.cpsr = (cpu.cpsr & 0x0FFF_FFFF) | (i as u32) << 28,
        16 => {
            cpu.r[12] = rng.next();
            cpu.cpsr = (cpu.cpsr & 0x0FFF_FFFF) | rng.next() & 0xF000_0000;
        }
        17..21 => cpu.r[i - 17] = cpu.r[i - 17].wrapping_add(1),
        _ => (0..4).for_each(|r| cpu.r[r] = rng.next()),
    }
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Reached the return address the entry state's lr held.
    Returned,
    /// Compiled code or the interpreter asked the run loop for control (an
    /// IRQ, a halt, the frame's end).
    Yielded,
    Fault(Fault),
}

/// What a run did.
#[derive(Clone, PartialEq, Eq)]
pub struct Outcome {
    pub end: End,
    /// The CPSR's flags at a yield, taken out of `payload` (excused) unless
    /// exact.
    pub flags: Option<u32>,
    /// The bus accesses' and ticks' digests ([`runtime::trace`]).
    pub bus: (u64, u64),
    pub ticks: (u64, u64),
    pub regs: [u32; 15],
    pub pc: u32,
    pub cpsr: u32,
    pub payload: Vec<u8>,
}

/// Run from `s` changed by mutation `i` through `code` to the function's
/// return or the first yield.
pub fn run(m: &mut M, code: &Code<Headless>, s: &State, i: usize, exact: bool) -> Outcome {
    m.load_payload(&s.payload).expect("a payload of this build");
    // A borrowed state (`Opts::hand`): another function's entry.
    if dispatch::key(&m.cpu) != s.entry {
        dispatch::set_pc(&mut m.cpu, s.entry);
    }
    let seed = (s.entry as u64) << 32 ^ s.hit << 8 ^ i as u64;
    mutate(&mut m.cpu, i, seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let lr = m.cpu.r[14];
    let ret = if lr & 1 != 0 { lr } else { lr & !3 };
    trace::BUS.reset();
    trace::TICKS.reset();
    m.sched.frame_ended = false;
    let end = loop {
        if let Some(f) = m.fault {
            break End::Fault(f);
        }
        let k = dispatch::key(&m.cpu);
        if k == ret {
            break End::Returned;
        }
        if let Some(f) = code.lookup(&mut m.bus, k) {
            let Flow::Jump(next) = f(m, k);
            dispatch::set_pc(&mut m.cpu, next);
            if dispatch::yielding(m) {
                break End::Yielded;
            }
            continue;
        }
        if m.step() {
            break End::Yielded;
        }
    };
    let mut flags = None;
    if end == End::Yielded && !exact {
        flags = Some(m.cpu.cpsr >> 28);
        m.cpu.cpsr &= 0x0FFF_FFFF;
    }
    let mut payload = Vec::new();
    m.save_payload(&mut |b| payload.extend_from_slice(b));
    Outcome {
        end,
        flags,
        bus: trace::BUS.get(),
        ticks: trace::TICKS.get(),
        regs: m.cpu.r,
        pc: m.cpu.pc,
        cpsr: m.cpu.cpsr,
        payload,
    }
}

/// What differs between two outcomes, or `None`; `Some("")` when only the
/// excused flags at a yield do.
pub fn differs(a: &Outcome, b: &Outcome) -> Option<String> {
    if a == b {
        return None;
    }
    let mut w = Vec::new();
    if a.end != b.end {
        w.push(format!("end {:?} vs {:?}", a.end, b.end));
    }
    if a.bus != b.bus {
        w.push(format!("bus trace ({} vs {} accesses)", a.bus.1, b.bus.1));
    }
    if a.ticks != b.ticks {
        w.push(format!("ticks ({} vs {})", a.ticks.1, b.ticks.1));
    }
    if a.payload != b.payload {
        let at = a.payload.iter().zip(&b.payload).position(|(x, y)| x != y);
        w.push(format!("state from payload byte {}", at.unwrap_or(0)));
        if (a.regs, a.pc, a.cpsr) != (b.regs, b.pc, b.cpsr) {
            let regs = |o: &Outcome| {
                let r: Vec<String> = o.regs.iter().map(|r| format!("{r:08x}")).collect();
                format!("pc {:08x} cpsr {:08x} r {}", o.pc, o.cpsr, r.join(" "))
            };
            w.push(format!("\n      {}\n      {}", regs(a), regs(b)));
        }
    }
    Some(w.join(", "))
}

/// One form of the functions: its name and its code.
pub struct Form {
    pub name: &'static str,
    pub code: &'static Code<Headless>,
}

/// What a generated `fn-diff` crate runs.
pub struct Spec {
    /// The reference (every transformation on) first, then one form per
    /// transformation turned off.
    pub forms: &'static [Form],
    /// The functions picked, as keys; their callees are in the forms too.
    pub picked: &'static [u32],
    /// The last form is the reference with one byte of this function's
    /// code changed: it must differ, in this function's runs.
    pub fault: Option<u32>,
    /// The entry states ([`write_states`]).
    pub states: &'static str,
}

/// The crate's `main`: `diff-NAME ROM [--exact]`.
pub fn main(spec: &Spec) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(rom_path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: diff-NAME ROM [--exact]");
        return ExitCode::from(2);
    };
    let exact = args.iter().any(|a| a == "--exact");
    if !trace::ENABLED {
        eprintln!("fn-diff: runtime built without its `trace` feature");
        return ExitCode::from(2);
    }
    let rom: &'static [u8] = std::fs::read(rom_path).expect("ROM").leak();
    let reference = &spec.forms[0];
    assert_eq!(
        dispatch::rom_hash(rom),
        reference.code.rom_hash,
        "not the ROM this was recompiled from"
    );
    let states = read_states(Path::new(spec.states)).expect("the states file");
    let funcs: BTreeSet<u32> = states.iter().map(|s| s.entry).collect();
    let mut m = Box::new(Machine::new(rom, Headless));
    let n_runs = states.len() * MUTATIONS;
    println!(
        "fn-diff: {} functions picked, {} entered, {} states, {n_runs} runs per form",
        spec.picked.len(),
        funcs.len(),
        states.len()
    );
    // Per form: runs that differ, that differ only in the excused flags,
    // and that differ in the faulted function's own runs.
    let mut tally = vec![(0, 0, 0); spec.forms.len()];
    let mut ends = [0; 3];
    let fault_form = spec.fault.map(|_| spec.forms.len() - 1);
    let mut shown = 0;
    for s in &states {
        for i in 0..MUTATIONS {
            let a = run(&mut m, reference.code, s, i, exact);
            ends[match a.end {
                End::Returned => 0,
                End::Yielded => 1,
                End::Fault(_) => 2,
            }] += 1;
            for (fi, form) in spec.forms.iter().enumerate().skip(1) {
                let o = run(&mut m, form.code, s, i, exact);
                let Some(what) = differs(&a, &o) else {
                    continue;
                };
                let t = &mut tally[fi];
                let mut flags_only = o;
                flags_only.flags = a.flags;
                if differs(&a, &flags_only).is_none() {
                    t.1 += 1;
                    continue;
                }
                t.0 += 1;
                if Some(fi) == fault_form {
                    t.2 += usize::from(Some(s.entry) == spec.fault);
                } else if shown < 10 {
                    shown += 1;
                    println!(
                        "    {}: f_{:08x} entry {} {}: {what}",
                        form.name,
                        s.entry,
                        s.hit,
                        mutation_name(i)
                    );
                }
            }
        }
    }
    println!(
        "  {}: returned {}, yielded {}, faulted {}",
        reference.name, ends[0], ends[1], ends[2]
    );
    let mut ok = true;
    for (fi, form) in spec.forms.iter().enumerate().skip(1) {
        let (differ, excused, in_fault) = tally[fi];
        match spec.fault.filter(|_| Some(fi) == fault_form) {
            Some(f) => {
                let caught = in_fault > 0;
                println!(
                    "  {} (one byte of f_{f:08x} changed): {}, {differ} of {n_runs} runs differ, \
                     {in_fault} of them f_{f:08x}'s",
                    form.name,
                    if caught { "caught" } else { "NOT caught" }
                );
                ok &= caught;
            }
            None => {
                print!("  {}: {differ} of {n_runs} runs differ", form.name);
                if excused > 0 {
                    print!(
                        ", {excused} more only in the CPSR's flags at a yield \
                         (excused until the yield-path flags fix; --exact counts them)"
                    );
                }
                println!();
                ok &= differ == 0;
            }
        }
    }
    println!("fn-diff: {}", if ok { "pass" } else { "FAIL" });
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip() {
        let path = std::env::temp_dir().join(format!("fn-diff-{}.bin", std::process::id()));
        let states = vec![
            State {
                entry: 0x0800_0101,
                hit: 3,
                payload: vec![1, 2, 3],
            },
            State {
                entry: 0x0800_0200,
                hit: 0,
                payload: vec![],
            },
        ];
        write_states(&path, &states).unwrap();
        let back = read_states(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(
            (back[0].entry, back[0].hit, &back[0].payload),
            (0x0800_0101, 3, &vec![1, 2, 3])
        );
        assert_eq!(
            (back[1].entry, back[1].hit, back[1].payload.len()),
            (0x0800_0200, 0, 0)
        );
    }

    #[test]
    fn mutations_cover_every_nzcv_and_are_deterministic() {
        let mut seen = BTreeSet::new();
        for i in 0..16 {
            let mut c = Cpu::default();
            mutate(&mut c, i, 7);
            seen.insert(c.cpsr >> 28);
        }
        assert_eq!(seen.len(), 16);
        let (mut a, mut b) = (Cpu::default(), Cpu::default());
        mutate(&mut a, MUTATIONS - 1, 99);
        mutate(&mut b, MUTATIONS - 1, 99);
        assert_eq!(a.r, b.r);
        assert_ne!(a.r[0], 0);
    }
}
