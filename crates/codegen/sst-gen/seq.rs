//! Written by `gbaa-recompile --sst` from `crates/codegen/sst-gen/seq.rs`.
//! Don't edit the copy.
//!
//! The sequence cases: random blocks of SingleStepTests opcodes and short
//! functions of them with branches, built by codegen's own pipeline from an
//! image at `0x0800_0000`, each run through its generated fn and through the
//! interpreter with `armv4t::cycles` counted beside it, as
//! `runtime::Machine::step` counts. Compared: the CPU, the bus accesses
//! interleaved with the timing calls (multiplies, `ldm`/`stm` counts,
//! prefetch stalls), and every tick's cycles.

use armv4t::cycles::{self, Cost, SKIPPED};
use armv4t::decode::{Instr, decode_arm, decode_thumb};
use armv4t::interp::{Bus, Cpu};
use armv4t::sem::Width;
use gbaa_sst_gen::rt::{self, Ev, Seq, Sst};
use gbaa_sst_gen::SEQS;
use std::collections::{BTreeMap, HashMap};

/// The code's image, mapped at the start of the cartridge.
static IMAGE: &[u8] = include_bytes!("../image.bin");
const ROM_BASE: u32 = 0x0800_0000;

/// Ticks a case runs before it yields: where its loops end.
const BUDGET: usize = 24;

/// Memory: the image in the cartridge's regions (stores there are
/// dropped), elsewhere bytes stored stay and the rest read as a hash of
/// their address. Every access goes in the log.
#[derive(Default)]
struct SeqBus {
    bytes: HashMap<u32, u8>,
}

fn rom_offset(a: u32) -> Option<usize> {
    let o = (a.checked_sub(ROM_BASE)? as usize) % (32 << 20);
    (a < 0x0E00_0000 && o < IMAGE.len()).then_some(o)
}

impl Bus for SeqBus {
    fn load(&mut self, w: Width, addr: u32) -> u32 {
        let v = (0..w.bytes()).fold(0, |acc, i| {
            let a = addr.wrapping_add(i);
            let b = match rom_offset(a) {
                Some(o) => IMAGE[o],
                None => self.bytes.get(&a).copied().unwrap_or_else(|| {
                    (a.wrapping_mul(0x9E37_79B9).rotate_left(13) ^ 0x5A5A_5A5A) as u8
                }),
            };
            acc | (b as u32) << (8 * i)
        });
        rt::log(Ev::Load {
            bytes: w.bytes(),
            addr,
            value: v,
        });
        v
    }
    fn store(&mut self, w: Width, addr: u32, v: u32) {
        for i in 0..w.bytes() {
            let a = addr.wrapping_add(i);
            if rom_offset(a).is_none() {
                self.bytes.insert(a, (v >> (8 * i)) as u8);
            }
        }
        rt::log(Ev::Store {
            bytes: w.bytes(),
            addr,
            value: v,
        });
    }
}

fn key(cpu: &Cpu) -> u32 {
    if cpu.thumb() { cpu.pc | 1 } else { cpu.pc & !3 }
}

/// The memory region of code at `addr`, as codegen and the runtime index
/// wait tables.
fn region(addr: u32) -> usize {
    let r = (addr >> 24) as usize;
    if r < 16 { r } else { 0 }
}

struct Run {
    cpu: Cpu,
    events: Vec<Ev>,
    ticks: Vec<i32>,
}

/// The generated fn, until it returns.
fn run_gen(s: &Seq) -> Option<Run> {
    rt::take_log();
    let mut bus = SeqBus::default();
    let mut m = Sst::new(s.cpu.clone(), &mut bus, BUDGET);
    let k = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (s.f)(&mut m, s.entry)));
    let events = rt::take_log();
    let runtime::dispatch::Flow::Jump(k) = k.ok()?;
    let (mut cpu, ticks) = (m.cpu, m.ticks);
    // A static exit leaves the CPU's pc behind; a dynamic one set it, low
    // bits and all.
    if key(&cpu) != k {
        cpu.pc = k & !1;
    }
    Some(Run { cpu, events, ticks })
}

/// The interpreter, an instruction at a time, counting each one's cycles
/// and what hangs on values as `Machine::step` does, and ending a tick
/// where the case's blocks end: until `ticks` ticks, or it leaves the
/// blocks. `None` if it flips the T bit inside a block (the block's set
/// is fixed).
fn run_interp(s: &Seq, ticks: usize) -> Option<Run> {
    rt::take_log();
    let mut bus = SeqBus::default();
    let mut cpu = s.cpu.clone();
    let thumb = cpu.thumb();
    let size = if thumb { 2 } else { 4 };
    let mut out = Vec::new();
    'blocks: while out.len() < ticks {
        let Some(&(_, end)) = s.blocks.iter().find(|b| b.0 == cpu.pc) else {
            break;
        };
        let mut c = 0i32;
        loop {
            let at = cpu.pc;
            let o = (at - ROM_BASE) as usize;
            let instr = if thumb {
                decode_thumb(u16::from_le_bytes([IMAGE[o], IMAGE[o + 1]]), at)
            } else {
                decode_arm(u32::from_le_bytes(IMAGE[o..o + 4].try_into().unwrap()), at)
            };
            let pass = cpu.cond_passes(instr.cond());
            let cost: Cost = if pass { cycles::cost(&instr) } else { SKIPPED };
            let r = region(at);
            c += (cost.base + rt::waits(r, thumb, cost.seq, cost.nonseq)) as i32;
            if pass {
                if let Instr::Mul { rs, .. } | Instr::MulLong { rs, .. } = instr {
                    let v = cpu.r.get(rs as usize).copied().unwrap_or(at + 2 * size);
                    rt::log(Ev::Mul(cycles::mul_wait(&instr, v).unwrap()));
                }
                if let Some(n) = cycles::block_accesses(&instr) {
                    rt::log(Ev::Block(n));
                }
            }
            cpu.execute_decoded(&mut bus, instr);
            if r >= 8
                && let Some(load) = cycles::stalls(&instr)
            {
                rt::log(Ev::Stall {
                    r,
                    pc: at + 2 * size,
                    load,
                });
            }
            let next = at + size;
            let ends = cpu.pc != next || next == end;
            if cpu.thumb() != thumb {
                if !ends {
                    return None;
                }
                // A `bx` to the other set (or a block-ending `msr`): it
                // leaves the case's blocks.
                out.push(c);
                break 'blocks;
            }
            if ends {
                out.push(c);
                continue 'blocks;
            }
        }
    }
    Some(Run {
        cpu,
        events: rt::take_log(),
        ticks: out,
    })
}

/// The first way the generated run differs from the interpreter's.
/// `excused`: whether the CPSR's flags may differ (a run that ended in a
/// yield, `stale_flags`).
fn differ(g: &Run, r: &Run, excused: bool) -> Option<String> {
    let mut cpu = g.cpu.clone();
    if excused {
        cpu.cpsr = cpu.cpsr & 0x0FFF_FFFF | r.cpu.cpsr & 0xF000_0000;
    }
    if cpu != r.cpu {
        return Some(format!("cpu {:x?}\n    interpreter {:x?}", g.cpu, r.cpu));
    }
    if g.events != r.events {
        let i = g.events.iter().zip(&r.events).take_while(|(a, b)| a == b).count();
        return Some(format!(
            "event {i}: {:x?}, interpreter {:x?} (of {} and {})",
            g.events.get(i),
            r.events.get(i),
            g.events.len(),
            r.events.len()
        ));
    }
    (g.ticks != r.ticks).then(|| format!("ticks {:?}, interpreter {:?}", g.ticks, r.ticks))
}

/// Whether a run's CPSR flags differ at a yield: flag liveness leaves
/// them stale on a static edge whose target writes them before reading,
/// until the yield path's fix (step 2 of
/// `docs/structured-codegen-design.md`), as `fn-diff` excuses them.
fn stale_flags(g: &Run, r: &Run) -> bool {
    g.ticks.len() >= BUDGET && (g.cpu.cpsr ^ r.cpu.cpsr) >> 28 != 0
}

/// Every sequence case; the number that differ.
pub fn run() -> usize {
    // kind: (run, skipped, ticks, flags excused at a yield, differ).
    let mut table: BTreeMap<&str, (usize, usize, usize, usize, Vec<String>)> = BTreeMap::new();
    for s in SEQS.iter() {
        let row = table.entry(s.kind).or_default();
        let Some(g) = run_gen(s) else {
            row.0 += 1;
            row.4.push(format!("{:#x}: generated code panicked", s.entry));
            continue;
        };
        let Some(r) = run_interp(s, g.ticks.len().max(1)) else {
            row.1 += 1;
            continue;
        };
        row.0 += 1;
        row.2 += g.ticks.len();
        let excused = stale_flags(&g, &r);
        row.3 += excused as usize;
        if let Some(d) = differ(&g, &r, excused) {
            row.4.push(format!("{:#x} ({} blocks): {d}", s.entry, s.blocks.len()));
        }
    }
    println!(
        "{:<34} {:>6} {:>6} {:>6} {:>6} {:>6}",
        "sequences", "run", "ticks", "T flip", "flags", "≠"
    );
    let mut bad = 0;
    for (kind, (run, skipped, ticks, flags, diff)) in &table {
        println!(
            "{kind:<34} {run:>6} {ticks:>6} {skipped:>6} {flags:>6} {:>6}",
            diff.len()
        );
        for d in diff.iter().take(5) {
            println!("    {d}");
        }
        bad += diff.len();
    }
    bad
}
