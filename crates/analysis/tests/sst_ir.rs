//! SingleStepTests ARM7TDMI through the IR: each case's instruction is
//! decoded, built into a one-instruction block (`ir::build`), cleaned by
//! the block-local passes (`Block::optimize`: const-prop, DCE, dead
//! register writes, flag liveness), and run by the IR evaluator (`ir::eval`) on the interpreter's CPU
//! state and bus. The result is compared with the test's expected state, as
//! `just sst` does, and with the interpreter's own run of the same case:
//! final state, r15, and every bus access in order.
//!
//! `just sst-ir` runs this in release mode and prints the per-file table,
//! which must match `just sst`'s, with no case diverging from the
//! interpreter. The harness is `armv4t/tests/common/sst.rs`.

#[path = "../../armv4t/tests/common/sst.rs"]
#[allow(dead_code)]
mod sst;

use analysis::ir::{build::Builder, constprop::Rom, eval};
use armv4t::decode::{decode_arm, decode_thumb};
use armv4t::interp::{Bus, Cpu};
use armv4t::sem::Width;
use armv4t::sem::arm::execute;

/// Build the instruction at `cpu.pc` into a block and run it.
fn run_ir(cpu: &mut Cpu, bus: &mut impl Bus, raw: u32) -> bool {
    let (at, thumb) = (cpu.pc, cpu.thumb());
    let instr = if thumb {
        decode_thumb(raw as u16, at)
    } else {
        decode_arm(raw, at)
    };
    let mut b = Builder::new(at, thumb);
    b.begin_insn(at);
    execute(&mut b, at, thumb, instr);
    let mut block = b.finish(at.wrapping_add(if thumb { 2 } else { 4 }));
    block.optimize(Rom::NONE);
    eval::run(&block, cpu, bus).is_some()
}

/// The first way the IR run differs from the interpreter's.
fn diverged(ir: &sst::Run, interp: &sst::Run) -> Option<(String, String)> {
    let failed = |r: &sst::Run| match &r.outcome {
        sst::Outcome::Fail { field, .. } => Some(field.clone()),
        _ => None,
    };
    if let (Some((ir_state, ir_txs)), Some((in_state, in_txs))) = (&ir.end, &interp.end) {
        if let Some(d) = sst::compare_states(ir_state, in_state) {
            return Some(d);
        }
        if ir_txs != in_txs {
            return Some((
                "bus".into(),
                format!("IR accesses {ir_txs:x?}, interpreter {in_txs:x?}"),
            ));
        }
    }
    // One ran and the other didn't (a panic), or they failed differently.
    let (a, b) = (failed(ir), failed(interp));
    (a != b).then(|| {
        (
            "outcome".into(),
            format!("IR failed on {a:?}, interpreter on {b:?}"),
        )
    })
}

#[test]
#[cfg_attr(debug_assertions, ignore = "slow in debug builds: run `just sst-ir`")]
fn single_step_tests_through_ir() {
    sst::run_suite("≠interp", |c| {
        let ir = sst::run_case(c, |cpu, bus, raw| run_ir(cpu, bus, raw));
        let interp = sst::run_case(c, |cpu, bus, raw| cpu.execute(bus, raw));
        sst::Checked {
            diverged: diverged(&ir, &interp),
            outcome: ir.outcome,
        }
    });
}

/// Memory for the block test: bytes stored stay, the rest read as a hash
/// of their address. Every access is recorded.
#[derive(Default)]
struct HashMem {
    bytes: std::collections::HashMap<u32, u8>,
    seen: Vec<(bool, u32, u32, u32)>,
}

impl Bus for HashMem {
    fn load(&mut self, w: Width, addr: u32) -> u32 {
        let v = (0..w.bytes()).fold(0, |acc, i| {
            let a = addr.wrapping_add(i);
            let b = self.bytes.get(&a).copied().unwrap_or_else(|| {
                (a.wrapping_mul(0x9E37_79B9).rotate_left(13) ^ 0x5A5A_5A5A) as u8
            });
            acc | (b as u32) << (8 * i)
        });
        self.seen.push((false, w.bytes(), addr, v));
        v
    }
    fn store(&mut self, w: Width, addr: u32, v: u32) {
        for i in 0..w.bytes() {
            self.bytes
                .insert(addr.wrapping_add(i), (v >> (8 * i)) as u8);
        }
        self.seen.push((true, w.bytes(), addr, v));
    }
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

/// Opcodes and initial states from every file, sampled, for one state.
#[derive(Default)]
struct Pool {
    opcodes: Vec<u32>,
    states: Vec<sst::State>,
}

const BLOCKS: usize = 1_000_000;
const MAX_LEN: usize = 6;

/// Blocks of several SingleStepTests opcodes in a row, from one case's
/// initial state, through the IR and through the interpreter, on the same
/// memory. There's no expected state to compare with, only the
/// interpreter. This is what exercises the builder's forwarding of
/// registers and flags from one instruction to the next, and its cache
/// flushes after mode switches, `msr` and user-bank writes, which a
/// one-instruction case never reaches. A block whose run flips the T bit
/// without an exit (`msr`, unpredictable) is skipped: the block's
/// instruction set is fixed.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in debug builds: run `just sst-ir`")]
fn sst_opcodes_in_blocks_through_ir() {
    let Ok(entries) = std::fs::read_dir(sst::data_dir()) else {
        return; // `single_step_tests_through_ir` says why.
    };
    let mut pools = [Pool::default(), Pool::default()];
    let mut paths: Vec<_> = entries.map(|e| e.unwrap().path()).collect();
    paths.sort();
    for path in paths {
        if !path.to_string_lossy().ends_with(".json.bin") {
            continue;
        }
        for (i, c) in sst::read_file(&path).into_iter().enumerate() {
            let thumb = sst::to_cpu(&c.initial).thumb();
            let pool = &mut pools[thumb as usize];
            if i % 8 == 0 {
                pool.opcodes
                    .push(if thumb { c.opcode & 0xFFFF } else { c.opcode });
            }
            if i % 500 == 0 {
                pool.states.push(c.initial);
            }
        }
    }

    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let (mut run, mut skipped, mut diverged) = (0, 0, Vec::new());
    for _ in 0..BLOCKS {
        let thumb = rng.below(2) == 1;
        let pool = &pools[thumb as usize];
        let start = sst::to_cpu(&pool.states[rng.below(pool.states.len())]);
        let len = 2 + rng.below(MAX_LEN - 1);
        let raws: Vec<u32> = (0..len)
            .map(|_| pool.opcodes[rng.below(pool.opcodes.len())])
            .collect();
        let size = if thumb { 2 } else { 4 };
        let decode = |i: usize| {
            let at = start.pc.wrapping_add(size * i as u32);
            if thumb {
                decode_thumb(raws[i] as u16, at)
            } else {
                decode_arm(raws[i], at)
            }
        };

        // The interpreter, one instruction at a time until one exits.
        let (mut cpu, mut mem) = (start.clone(), HashMem::default());
        let mut flipped = false;
        for i in 0..len {
            if cpu.execute_decoded(&mut mem, decode(i)) {
                break;
            }
            if cpu.thumb() != thumb {
                flipped = true;
                break;
            }
        }
        if flipped {
            skipped += 1;
            continue;
        }

        // The same instructions as one block.
        let (mut ir_cpu, mut ir_mem) = (start.clone(), HashMem::default());
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut b = Builder::new(start.pc, thumb);
            let mut at = start.pc;
            for i in 0..len {
                b.begin_insn(at);
                execute(&mut b, at, thumb, decode(i));
                at = at.wrapping_add(size);
                if b.ended() {
                    break;
                }
            }
            let mut block = b.finish(at);
            block.optimize(Rom::NONE);
            eval::run(&block, &mut ir_cpu, &mut ir_mem);
            block
        }));
        run += 1;
        let problem = match &built {
            Err(_) => Some("panicked".to_string()),
            Ok(_) if ir_cpu != cpu => Some(format!("cpu {ir_cpu:x?}, interpreter {cpu:x?}")),
            Ok(_) if ir_mem.seen != mem.seen => Some(format!(
                "bus {:x?}, interpreter {:x?}",
                ir_mem.seen, mem.seen
            )),
            Ok(_) => None,
        };
        if let Some(p) = problem {
            let block = built.map(|b| b.to_string()).unwrap_or_default();
            diverged.push(format!(
                "{} block at {:#x}: {raws:08x?}\n  {p}\n{block}",
                if thumb { "thumb" } else { "arm" },
                start.pc
            ));
        }
    }
    println!(
        "blocks of 2 to {MAX_LEN} opcodes: {run} run, {} ≠interp, {skipped} skipped (T flipped)",
        diverged.len()
    );
    for d in diverged.iter().take(5) {
        println!("{d}");
    }
    assert!(diverged.is_empty(), "{} blocks ≠interp", diverged.len());
}
