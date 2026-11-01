//! Written by `gbaa-recompile --sst` from `crates/codegen/sst-gen/rt.rs`.
//! Don't edit the copy.
//!
//! `runtime::dispatch`'s helpers, for [`Sst`]. Instead of a scheduler and
//! a bus that counts, the timing helpers record what they're told: each
//! tick's cycles in [`Sst::ticks`], the rest ([`Ev`]) in the log the test
//! bus writes its accesses to, so the sequence check can compare both with
//! `armv4t::cycles` run alongside the interpreter.

use armv4t::interp::{Bus, Cpu, Exit};
pub use runtime::dispatch::Flow;
use std::cell::RefCell;

/// The machine a case runs on: the interpreter's CPU and the harness's bus.
pub struct Sst<'a> {
    pub cpu: Cpu,
    pub bus: &'a mut dyn Bus,
    /// An exit went through `apply_exit`.
    pub jumped: bool,
    /// Each tick's cycles, less what `skipped` took off since the last.
    pub ticks: Vec<i32>,
    /// Ticks to run before yielding: 1 for a one-instruction case (an
    /// instruction that jumps to itself would otherwise loop forever), a
    /// few dozen for a sequence (its loops end there).
    pub budget: usize,
    skipped: i32,
}

impl<'a> Sst<'a> {
    pub fn new(cpu: Cpu, bus: &'a mut dyn Bus, budget: usize) -> Self {
        Sst {
            cpu,
            bus,
            jumped: false,
            ticks: Vec::new(),
            budget,
            skipped: 0,
        }
    }
}

pub type CaseFn = for<'a, 'b> fn(&'a mut Sst<'b>, u32) -> Flow;

/// A sequence case: a block or short function at `entry` (a key), run from
/// `cpu`. `blocks`: its blocks' addresses and ends, as codegen built them.
pub struct Seq {
    pub kind: &'static str,
    pub entry: u32,
    pub cpu: Cpu,
    pub blocks: &'static [(u32, u32)],
    pub f: CaseFn,
}

/// What the generated code (or the reference beside the interpreter) did
/// besides ticking, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    Load { bytes: u32, addr: u32, value: u32 },
    Store { bytes: u32, addr: u32, value: u32 },
    /// A multiply's internal cycles.
    Mul(u32),
    /// An `ldm`/`stm`'s access count.
    Block(u32),
    Stall { r: usize, pc: u32, load: bool },
}

thread_local! {
    static LOG: RefCell<Vec<Ev>> = const { RefCell::new(Vec::new()) };
}

pub fn log(e: Ev) {
    LOG.with_borrow_mut(|l| l.push(e));
}

/// The log so far, emptying it.
pub fn take_log() -> Vec<Ev> {
    LOG.with_borrow_mut(std::mem::take)
}

/// The wait states the test machine charges for `seq` sequential and
/// `nonseq` nonsequential fetches from region `r`: made up, but different
/// for S and N, ARM and Thumb, so a count in the wrong one shows.
pub fn waits(r: usize, thumb: bool, seq: u32, nonseq: u32) -> u32 {
    let (r, t) = (r as u32, thumb as u32);
    seq * (1 + r % 4 + t) + nonseq * (3 + r % 3 + 2 * t)
}

pub fn tick(m: &mut Sst, cycles: u32) -> bool {
    let c = cycles as i32 - std::mem::take(&mut m.skipped);
    m.ticks.push(c);
    m.ticks.len() >= m.budget
}

pub fn fetch(_m: &Sst, r: usize, thumb: bool, seq: u32, nonseq: u32) -> u32 {
    waits(r, thumb, seq, nonseq)
}

pub fn stall(_m: &mut Sst, r: usize, pc: u32, load: bool) {
    log(Ev::Stall { r, pc, load });
}

/// As `runtime::dispatch::skipped`: the block's count has the
/// instruction's full cost; the next tick takes off what it costs beyond
/// `armv4t::cycles::SKIPPED`.
pub fn skipped(m: &mut Sst, r: usize, thumb: bool, base: u32, seq: u32, nonseq: u32) {
    let full = base + waits(r, thumb, seq, nonseq);
    let s = armv4t::cycles::SKIPPED;
    let one = s.base + waits(r, thumb, s.seq, s.nonseq);
    m.skipped += full as i32 - one as i32;
}

pub fn multiply(_m: &mut Sst, rs: u32, signed: bool, extra: u32) {
    log(Ev::Mul(armv4t::cycles::mul_cycles(rs, signed) + extra));
}

pub fn block(_m: &mut Sst, n: u32) {
    log(Ev::Block(n));
}

pub fn exit(m: &mut Sst, at: u32, thumb: bool, e: Exit) -> u32 {
    m.jumped = true;
    m.cpu.apply_exit(at, thumb, Some(e));
    runtime::dispatch::key(&m.cpu)
}

pub fn yielding(m: &Sst) -> bool {
    m.ticks.len() >= m.budget
}

pub fn rekey(m: &mut Sst, addr: u32) -> u32 {
    m.cpu.pc = addr & !1;
    runtime::dispatch::key(&m.cpu)
}

pub fn swi(m: &mut Sst, at: u32, thumb: bool, _comment: u32) -> u32 {
    exit(m, at, thumb, Exit::Swi)
}
