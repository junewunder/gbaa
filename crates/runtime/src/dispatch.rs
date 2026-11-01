//! Running recompiled code (`docs/codegen-design.md`): the type every
//! generated function returns, the table of block starts, the helpers
//! generated code calls, and the dispatcher loop with its interpreter
//! fallback.
//!
//! A **key** is a guest address in interworking form: bit 0 set for Thumb.
//! Its bit 0 always equals the CPSR's T bit. Inside generated code
//! `cpu.pc` is stale; every [`Flow`] carries the key to continue at, and
//! the dispatcher writes it back, so `cpu.pc` is exact whenever control is
//! outside generated code.
//!
//! Code copied into IWRAM is recompiled too, as overlays
//! ([`crate::overlay`]): its blocks are in [`Code::ram_blocks`], and a key
//! there is only found while the overlay's bytes are still in IWRAM.

use crate::bus::Bus;
use crate::machine::Machine;
use crate::overlay::Overlay;
use crate::platform::Platform;
use armv4t::interp::{Cpu, Exit};

pub mod modcall;

/// How a generated function was left.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Continue at this key.
    Jump(u32),
}

/// A recompiled guest function. The second argument is the key of the
/// block to start at: the entry, or, in a function printed in v1 form, any
/// other block start of the function (a return site, a jump-table target,
/// an IRQ's return point). A structured function
/// (`docs/structured-codegen-design.md`, step 8) is only called at its
/// entry.
pub type GuestFn<P> = fn(&mut Machine<P>, u32) -> Flow;

/// A recompiled ROM: its dispatch keys, and the ROM it was made from.
pub struct Code<P: Platform + 'static> {
    /// The ROM's dispatch keys: every function's entry, and every block
    /// start of the functions printed in v1 form. Sorted by key, no
    /// duplicates.
    pub blocks: &'static [(u32, GuestFn<P>)],
    /// The blocks of the code recompiled into IWRAM, with the index of
    /// their first overlay in [`overlays`](Code::overlays) and how many
    /// versions of the block follow it there. Sorted by key, no duplicates.
    pub ram_blocks: &'static [(u32, GuestFn<P>, u16, u16)],
    /// The overlays' guards.
    pub overlays: &'static [Overlay],
    /// Where each of [`blocks`](Code::blocks) ticks, for [`run_blocks`]
    /// (empty in a build that has none).
    pub extents: &'static [Extent],
    /// The same for [`ram_blocks`](Code::ram_blocks).
    pub ram_extents: &'static [Extent],
    /// The block starts of the structured functions that aren't keys:
    /// compiled code the dispatcher can't enter there, so the interpreter
    /// runs from one to the next key (a resume, not a seed). Sorted.
    pub inner: &'static [u32],
    /// Where each of [`inner`](Code::inner) ticks, for [`run_blocks`],
    /// which runs them as blocks too (empty in a build without extents).
    pub inner_extents: &'static [Extent],
    /// The block starts past the entry of the functions a mod replaces
    /// (`gbaa-recompile --mod`, `docs/mod-view-design.md`, "A mod copy"),
    /// sorted: a resume at one is counted in [`Stats::mod_resumes`].
    pub modded: &'static [u32],
    /// The ROM ranges (start, end) const-prop read loads from and folded
    /// into the code (`analysis::ir::constprop`), sorted and merged: a
    /// mod's patched ROM copy mustn't change these bytes, since the code
    /// keeps the original's (docs/asset-extraction-design.md, "Overrides").
    pub folded: &'static [(u32, u32)],
    /// [`rom_hash`] of the ROM it was recompiled from.
    pub rom_hash: u64,
    /// The timing it was generated with (`gbaa-recompile --timing`): what
    /// a front end runs its interpreter fallback with unless told otherwise
    /// ([`crate::Machine::timing`]).
    pub timing: crate::Timing,
    /// Where a front end with a file system writes the fallback's ROM entry
    /// addresses (decision 6's seeds), one per line.
    pub seeds_file: Option<&'static str>,
    /// The game's own spin loops for `--timing native`, sorted
    /// ([`Machine::spin_list`], `runtime::spins`): a front end installs
    /// them in the machine, for its interpreter.
    pub spins: &'static [u32],
}

/// How far a compiled block runs before the tick at its exit: `len`
/// instructions, or fewer if one leaves the straight line (a taken branch
/// or exception); then, if that took it to `stub` (a `bx rN` its call goes
/// through, `codegen::func::Plan::CallVia`; 0 for none), that one too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub len: u16,
    pub stub: u32,
}

impl<P: Platform> Code<P> {
    /// The first of [`folded`](Code::folded) that overlaps `start..end`.
    pub fn folded_in(&self, start: u32, end: u32) -> Option<(u32, u32)> {
        let i = self.folded.partition_point(|&(_, e)| e <= start);
        self.folded.get(i).copied().filter(|&(s, _)| s < end)
    }

    /// The function holding the block at `key`. A key in RAM is only found
    /// while one of its versions' bytes are in IWRAM.
    #[inline]
    pub fn lookup(&self, bus: &mut Bus, key: u32) -> Option<GuestFn<P>> {
        self.find(bus, key).map(|(f, _)| f)
    }

    /// [`Code::lookup`], with the block's place: its index in
    /// [`ram_blocks`](Code::ram_blocks) (`true`) or
    /// [`blocks`](Code::blocks).
    #[inline]
    fn find(&self, bus: &mut Bus, key: u32) -> Option<(GuestFn<P>, (bool, usize))> {
        if is_ram(key) {
            let i = self.ram_blocks.binary_search_by_key(&key, |e| e.0).ok()?;
            let (_, f, o, n) = self.ram_blocks[i];
            let o = o as usize;
            return (o..o + n as usize)
                .any(|v| bus.overlay_valid(v))
                .then_some((f, (true, i)));
        }
        self.blocks
            .binary_search_by_key(&key, |&(k, _)| k)
            .ok()
            .map(|i| (self.blocks[i].1, (false, i)))
    }

    /// The index of `key` in [`inner`](Code::inner).
    #[inline]
    fn find_inner(&self, key: u32) -> Option<usize> {
        self.inner.binary_search(&key).ok()
    }

    /// Whether `key` is a block of a modded function past its entry.
    #[inline]
    fn is_modded(&self, key: u32) -> bool {
        !self.modded.is_empty() && self.modded.binary_search(&key).is_ok()
    }

    /// Whether `key` is the first block of its function in
    /// [`blocks`](Code::blocks): its entry, for every function whose entry
    /// is its lowest address (the dispatcher's resume counts,
    /// `docs/structured-codegen-design.md`). RAM keys are never.
    pub fn is_entry(&self, key: u32) -> bool {
        let Ok(i) = self.blocks.binary_search_by_key(&key, |&(k, _)| k) else {
            return false;
        };
        i == 0 || self.blocks[i - 1].1 as usize != self.blocks[i].1 as usize
    }
}

fn is_ram(addr: u32) -> bool {
    (0x0200_0000..0x0400_0000).contains(&addr)
}

/// FNV-1a, 64-bit: checks that a recompiled build runs the ROM it was made
/// from.
pub fn rom_hash(rom: &[u8]) -> u64 {
    rom.iter().fold(0xCBF2_9CE4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// The key of where `cpu` is about to run. ARM keys are word-aligned (the
/// interpreter keeps a misaligned ARM jump target's low bits; no compiler
/// makes one).
#[inline(always)]
pub fn key(cpu: &Cpu) -> u32 {
    if cpu.thumb() { cpu.pc | 1 } else { cpu.pc & !3 }
}

/// Point `cpu` at `key`. Its bit 0 must agree with the T bit.
#[inline(always)]
pub fn set_pc(cpu: &mut Cpu, key: u32) {
    debug_assert_eq!(key & 1 != 0, cpu.thumb(), "key {key:#x} vs T bit");
    cpu.pc = key & !1;
}

/// Take a dynamic exit of the instruction at `at` the way the interpreter
/// does (`Cpu::apply_exit`: alignment, the T bit, exception entry and
/// return), and return the key to continue at.
#[inline(always)]
pub fn exit<P: Platform>(m: &mut Machine<P>, at: u32, thumb: bool, e: Exit) -> u32 {
    m.cpu.apply_exit(at, thumb, Some(e));
    key(&m.cpu)
}

/// Advance time by `cycles` ([`Machine::tick`]). True when the run loop
/// must get control back: an IRQ can be taken, the CPU halted, or the
/// frame ended.
#[inline(always)]
pub fn tick<P: Platform>(m: &mut Machine<P>, cycles: u32) -> bool {
    m.tick(cycles)
}

/// [`tick`] for code generated without mGBA's model (`gbaa-recompile
/// --timing instant|static`): the data accesses since the last tick cost
/// nothing, so only `cycles` passes.
#[inline(always)]
pub fn tick_flat<P: Platform>(m: &mut Machine<P>, cycles: u32) -> bool {
    m.bus.take_data_cycles();
    m.tick(cycles)
}

/// [`tick`] for code generated with `--timing native`: no time passes
/// ([`Machine::tick_native`]).
#[inline(always)]
pub fn tick_native<P: Platform>(m: &mut Machine<P>) -> bool {
    m.tick_native()
}

/// The exit along an idle loop's back edge (`--timing native`,
/// [`armv4t::idle`]): the clock runs to the next event
/// ([`Machine::idle_edge`]). True as [`tick`].
#[inline(always)]
pub fn idle_edge<P: Platform>(m: &mut Machine<P>) -> bool {
    m.idle_edge()
}

/// The exit out of an idle loop, past its branch (`--timing native`):
/// everything brought up to a clock a DMA moved
/// ([`Machine::idle_exit`]). True as [`tick`].
#[inline(always)]
pub fn idle_exit<P: Platform>(m: &mut Machine<P>) -> bool {
    m.idle_exit()
}

/// The back edge of a spin loop at `at` (`--timing native`): counted,
/// and past its budget the clock runs to the next event
/// ([`Machine::spin_edge`]). True as [`tick`].
#[inline(always)]
pub fn spin_edge<P: Platform>(m: &mut Machine<P>, at: u32) -> bool {
    m.spin_edge(at)
}

/// [`tick_ram`] as [`tick_flat`] counts.
#[inline(always)]
pub fn tick_ram_flat<P: Platform>(m: &mut Machine<P>, cycles: u32) -> bool {
    m.bus.take_data_cycles();
    tick_ram(m, cycles)
}

/// The wait states of fetching `seq` sequential and `nonseq`
/// nonsequential instructions from region `r` ([`crate::waits::Waits::fetch`]).
#[inline(always)]
pub fn fetch<P: Platform>(m: &Machine<P>, r: usize, thumb: bool, seq: u32, nonseq: u32) -> u32 {
    m.bus.waits.fetch(r, thumb, seq, nonseq)
}

/// After a memory instruction run from ROM region `r`: the prefetch
/// buffer's overlap with its data accesses ([`crate::bus::Bus::stall`]).
/// `pc` is its address plus two fetches; `load`, whether it loads.
#[inline(always)]
pub fn stall<P: Platform>(m: &mut Machine<P>, r: usize, pc: u32, load: bool) {
    m.bus.stall(r, pc, load)
}

/// An instruction whose condition failed, with `base`, `seq` and `nonseq`
/// its [`armv4t::cycles::Cost`], run from region `r`: the block's count
/// has its cost, so take off what it costs beyond
/// [`armv4t::cycles::SKIPPED`] (`Machine::step` counts the same).
#[inline(always)]
pub fn skipped<P: Platform>(
    m: &mut Machine<P>,
    r: usize,
    thumb: bool,
    base: u32,
    seq: u32,
    nonseq: u32,
) {
    let w = &m.bus.waits;
    let full = base + w.fetch(r, thumb, seq, nonseq);
    let skipped = armv4t::cycles::SKIPPED;
    let one = skipped.base + w.fetch(r, thumb, skipped.seq, skipped.nonseq);
    m.bus.stall_cycles -= full as i32 - one as i32;
}

/// A multiply with operand `rs`: its internal cycles
/// ([`armv4t::cycles::mul_cycles`] plus `extra`), which the prefetch stall
/// may overlap, as data.
#[inline(always)]
pub fn multiply<P: Platform>(m: &mut Machine<P>, rs: u32, signed: bool, extra: u32) {
    m.bus.data_cycles += (armv4t::cycles::mul_cycles(rs, signed) + extra) as i32;
}

/// The next `n` data accesses are an `ldm`'s or `stm`'s ([`Bus::block`]).
#[inline(always)]
pub fn block<P: Platform>(m: &mut Machine<P>, n: u32) {
    m.bus.block(n)
}

/// [`tick`] from an overlay (code recompiled into IWRAM), counting its
/// cycles in [`crate::overlay::Overlays::cycles`].
#[inline(always)]
pub fn tick_ram<P: Platform>(m: &mut Machine<P>, cycles: u32) -> bool {
    m.bus.overlays.cycles += cycles as u64;
    m.tick(cycles)
}

/// Whether overlay `i` still matches IWRAM. Generated overlay code checks
/// it before each block: a store may have rewritten its own code.
#[inline(always)]
pub fn overlay_valid<P: Platform>(m: &mut Machine<P>, i: usize) -> bool {
    m.bus.overlay_valid(i)
}

/// An indirect call to `key` with its return address already in lr (a
/// `bl` to a `bx rN` stub): run the function holding that block, if
/// `code` has one, and return where it continues. Without one, `key`
/// itself, which the caller's `match` takes or passes up to the
/// dispatcher.
#[inline]
pub fn call<P: Platform>(m: &mut Machine<P>, code: &Code<P>, key: u32) -> Flow {
    match code.lookup(&mut m.bus, key) {
        Some(f) => f(m, key),
        None => Flow::Jump(key),
    }
}

/// Whether the run loop must get control back, as a state rather than an
/// event: checked after a call returns, since the callee's `tick` may have
/// said so already and `tick` doesn't repeat itself.
#[inline(always)]
pub fn yielding<P: Platform>(m: &Machine<P>) -> bool {
    m.sched.frame_ended || m.sched.halted || m.fault.is_some() || m.irq_takeable()
}

/// Continue at `addr` with whatever T bit the CPSR now has (the block
/// wrote its control byte). Returns the key.
#[inline(always)]
pub fn rekey<P: Platform>(m: &mut Machine<P>, addr: u32) -> u32 {
    m.cpu.pc = addr & !1;
    key(&m.cpu)
}

/// `swi` with comment field `comment`, the instruction at `at`: the same
/// [`Machine::swi`] the interpreter calls, with `cpu.pc` at the next
/// instruction. Returns the key execution continues at.
pub fn swi<P: Platform>(m: &mut Machine<P>, at: u32, thumb: bool, comment: u32) -> u32 {
    m.cpu.pc = at.wrapping_add(if thumb { 2 } else { 4 });
    m.swi(comment);
    key(&m.cpu)
}

/// Where the time went, for decision 1's "would change our minds" and
/// decision 6's seeds.
#[derive(Clone, Debug)]
pub struct Stats {
    /// Calls from the dispatcher into generated code.
    pub calls: u64,
    /// Resumes, after an unwind: the part of `calls` made at a ROM key
    /// that isn't a function's entry ([`Code::is_entry`]), and the runs of
    /// the interpreter fallback that started at, or ran on into, a block of
    /// a structured function that isn't a key ([`Code::inner`]), where v1
    /// would have called compiled code.
    pub resumes: u64,
    /// The part of `resumes` that landed inside a modded function
    /// ([`Code::modded`]): the interpreter runs the ROM's code there, or
    /// the mod's copy is entered past its start, so the mod may be skipped
    /// for that call.
    pub mod_resumes: u64,
    /// Returns to a call a mod added ([`modcall`]) that found the call's
    /// frame gone (a state load, a rewind or an unwind inside it) and went
    /// on at the key its block began at. Each is one of `mod_resumes` too.
    pub mod_call_resumes: u64,
    /// Calls into generated code that came back with the run loop needing
    /// control ([`yielding`]: an IRQ, a halt or the frame end), unwinding
    /// every compiled frame.
    pub yields: u64,
    /// Instructions the interpreter fallback ran.
    pub interpreted: u64,
    /// Guest cycles spent in generated code (with the `swi`s it made).
    pub compiled_cycles: u64,
    /// The part of `compiled_cycles` that overlays (code recompiled into
    /// IWRAM) ticked. Ticks only: a `swi` or DMA from an overlay counts as
    /// ROM code's.
    pub compiled_ram_cycles: u64,
    /// Guest cycles spent in the interpreter fallback, and taking IRQs
    /// (the BIOS stub runs in Rust, its return through the fallback).
    pub interpreted_cycles: u64,
    /// The part of `interpreted_cycles` spent running code in EWRAM or
    /// IWRAM (copied there, decision 7).
    pub interpreted_ram_cycles: u64,
    /// Guest cycles the CPU spent halted (`Halt`, `VBlankIntrWait`).
    pub halted_cycles: u64,
    /// Distinct ROM keys the fallback was entered at, the first
    /// [`MAX_SEEDS`] of them.
    pub seeds: [u32; MAX_SEEDS],
    pub n_seeds: usize,
    /// Calls that came back somewhere other than their return site and
    /// were settled at the call site ([`settle`]), and how many of those
    /// the nested run left elsewhere (a fault, the frame end, the depth
    /// cap), which then unwound.
    pub settles: u64,
    pub unsettled: u64,
    /// Ticks that found an IRQ takeable or the CPU halted and ran it in
    /// place ([`yield_at`]), and how many of those the nested run left
    /// elsewhere (a fault, the frame end), which then unwound.
    pub irqs_in_place: u64,
    pub irqs_unwound: u64,
    /// The deepest nesting of [`settle`]'s and [`yield_at`]'s runs.
    pub max_depth: u32,
    /// The deepest host stack below a run's start seen at a tick's slow
    /// path ([`Machine::sync`]), in bytes: a sample, not a bound.
    pub max_stack: usize,
    /// Frame ends that switched to the front end without unwinding
    /// ([`Nest::frame_end`]).
    pub switches: u64,
}

pub const MAX_SEEDS: usize = 256;

impl Default for Stats {
    fn default() -> Stats {
        Stats {
            calls: 0,
            resumes: 0,
            mod_resumes: 0,
            mod_call_resumes: 0,
            yields: 0,
            interpreted: 0,
            compiled_cycles: 0,
            compiled_ram_cycles: 0,
            interpreted_cycles: 0,
            interpreted_ram_cycles: 0,
            halted_cycles: 0,
            seeds: [0; MAX_SEEDS],
            n_seeds: 0,
            settles: 0,
            unsettled: 0,
            irqs_in_place: 0,
            irqs_unwound: 0,
            max_depth: 0,
            max_stack: 0,
            switches: 0,
        }
    }
}

impl Stats {
    fn seed(&mut self, key: u32) {
        if is_rom(key) && self.n_seeds < MAX_SEEDS && !self.seeds[..self.n_seeds].contains(&key) {
            self.seeds[self.n_seeds] = key;
            self.n_seeds += 1;
        }
    }
}

fn is_rom(addr: u32) -> bool {
    (0x0800_0000..0x0E00_0000).contains(&addr)
}

/// Why [`run`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The frame ended (V-blank started).
    FrameEnd,
    /// [`Machine::fault`] is set.
    Fault,
    /// Reached the key the caller asked to stop at.
    At(u32),
}

/// Run until the next V-blank starts, through `code` where it has the block
/// and the interpreter where it doesn't: [`Machine::run_frame`] with
/// recompiled code.
pub fn run_frame<P: Platform>(m: &mut Machine<P>, code: &Code<P>, stats: &mut Stats) {
    run(m, Some(code), None, stats);
}

/// Run until the frame ends, a fault, or `stop_at` is reached, through
/// `code` (or only the interpreter, with none). `stop_at` is checked
/// whenever control is in the dispatcher or the interpreter: a compiled
/// function returning to it unwinds here.
pub fn run<P: Platform>(
    m: &mut Machine<P>,
    code: Option<&Code<P>>,
    stop_at: Option<u32>,
    stats: &mut Stats,
) -> Stop {
    run_with(m, code, stop_at, false, stats)
}

/// [`run`] with the block-timed interpreter (validation item 1 in
/// `docs/structured-codegen-design.md`): wherever `code` has a block, the
/// interpreter runs that block's instructions without ticking
/// ([`Machine::step_untimed`]) and ticks once with their sum at its exit,
/// as the generated code does, so I/O sees the same clock and IRQs land
/// at the same instructions. Its full state is the generated code's at
/// every tick. The keys `code` doesn't have go to the interpreter fallback
/// as in [`run`]. `code` must have its [`extents`](Code::extents).
pub fn run_blocks<P: Platform>(
    m: &mut Machine<P>,
    code: &Code<P>,
    stop_at: Option<u32>,
    stats: &mut Stats,
) -> Stop {
    assert!(
        code.extents.len() == code.blocks.len() && code.ram_extents.len() == code.ram_blocks.len(),
        "a build without block extents"
    );
    run_with(m, Some(code), stop_at, true, stats)
}

/// Run the block at the CPU's key, whose extent is `e`, in the interpreter
/// and tick its cycles (counted as an overlay's with `ram`). Like
/// generated code, it prices the block's fetches at the exit, with the
/// wait states then (a store to `WAITCNT` reprices the block it is in),
/// and a call's stub's apart, in the stub's region.
fn interp_block<P: Platform>(m: &mut Machine<P>, e: Extent, ram: bool) {
    let (start, thumb) = (m.cpu.pc, m.cpu.thumb());
    let size = if thumb { 2 } else { 4 };
    let mut cost = armv4t::cycles::Cost::default();
    let mut stub = None;
    for _ in 0..e.len {
        let next = key(&m.cpu).wrapping_add(size);
        cost = cost.plus(m.step_untimed());
        let k = key(&m.cpu);
        if k != next {
            if e.stub != 0 && k == e.stub {
                stub = Some(m.step_untimed());
            }
            break;
        }
    }
    cost.base = cost.base.max(1);
    let price = |m: &Machine<P>, c: armv4t::cycles::Cost, at: u32| {
        c.base
            + m.bus
                .waits
                .fetch(crate::waits::region(at), thumb, c.seq, c.nonseq)
    };
    let mut cycles = price(m, cost, start);
    if let Some(c) = stub {
        cycles += price(m, c, e.stub);
    }
    if ram {
        m.bus.overlays.cycles += cycles as u64;
    }
    let _ = m.tick(cycles);
}

fn run_with<P: Platform>(
    m: &mut Machine<P>,
    code: Option<&Code<P>>,
    stop_at: Option<u32>,
    blocks: bool,
    stats: &mut Stats,
) -> Stop {
    run_raw(m, code, stop_at, blocks, stats)
}

/// The guest on its own stack (`docs/structured-codegen-design.md`, step
/// 7): [`run`] with [`Nest::frame_end`] set, so a frame end switches to
/// the front end and the run goes on when it switches back. Returns only
/// on a fault. `stats` is a pointer, not a `&mut`, because the front end
/// reads the counts while the run is suspended; it must stay valid, and
/// untouched but between switches, for the run's length.
pub fn run_guest<P: Platform>(m: &mut Machine<P>, code: &Code<P>, stats: *mut Stats) -> Stop {
    assert!(m.nest.frame_end.is_some(), "a guest run needs its switch");
    run_raw(m, Some(code), None, false, stats)
}

fn run_raw<P: Platform>(
    m: &mut Machine<P>,
    code: Option<&Code<P>>,
    stop_at: Option<u32>,
    blocks: bool,
    stats: *mut Stats,
) -> Stop {
    if let Some(c) = code {
        m.bus.overlays.install(c.overlays);
    }
    m.sched.frame_ended = false;
    // A nested run ([`settle`]) reaches the counts through the machine.
    let outer = core::mem::replace(&mut m.nest.stats, stats);
    let (top, low) = (m.nest.stack_top, m.nest.stack_low);
    if outer.is_null() {
        m.nest.stack_top = stack_here();
        m.nest.stack_low = m.nest.stack_top;
    }
    let stop = run_inner(m, code, stop_at.map(|k| (k, None)), blocks);
    if outer.is_null() {
        let deepest = m.nest.stack_top - m.nest.stack_low;
        // SAFETY: the caller's, for the run.
        let stats = unsafe { &mut *stats };
        stats.max_stack = stats.max_stack.max(deepest);
        (m.nest.stack_top, m.nest.stack_low) = (top, low);
    }
    m.nest.stats = outer;
    stop
}

/// The address of a local here: where the host stack is.
#[inline(always)]
fn stack_here() -> usize {
    let here = 0u8;
    core::hint::black_box(&here) as *const u8 as usize
}

/// How deep [`settle`]'s and [`yield_at`]'s runs nest before a call or an
/// IRQ unwinds instead. A guest that never comes back to the return site
/// (it returns somewhere else) keeps a nested run going, stranding host
/// frames until the frame ends; the cap bounds the host stack that can
/// take.
pub const MAX_DEPTH: u32 = 64;

/// The dispatcher's state for a nested run, in [`Machine::nest`].
#[derive(Debug)]
pub struct Nest {
    /// The counts of the run in progress: its caller's `&mut Stats`, held
    /// for the run's length (null outside one). A pointer, not a copy:
    /// [`Machine`] is already as big as a debug build's test stack allows.
    stats: *mut Stats,
    /// How many [`settle`] and [`yield_at`] runs are in progress.
    pub depth: u32,
    /// Where the outermost run's host stack starts, and the lowest it has
    /// been seen since ([`Nest::sample_stack`]); 0 outside a run.
    stack_top: usize,
    stack_low: usize,
    /// The guest on its own stack (step 7; [`run_guest`]): called at the
    /// frame end with the machine, it switches to the front end's stack
    /// and returns when the front end switches back for the next frame.
    /// The front end uses the machine only through that pointer, and only
    /// until it switches back. Set, the frame isn't presented at the
    /// V-blank either: the front end calls [`Machine::show_frame`] after
    /// the switch, so the platform's drawing runs on its own stack. None:
    /// the frame end unwinds to the outermost run, which returns.
    pub frame_end: Option<fn(*mut ())>,
    /// With [`Nest::frame_end`], V-blank started since the front end last
    /// cleared it: the switch is a frame's end, whose frame the front end
    /// shows ([`Machine::show_frame`]). A link hub's deadline switches too
    /// ([`crate::link`]), with no frame to show.
    pub vblank: bool,
    /// Run a structured function resumed past its entry block-timed
    /// ([`run_blocks`]'s way: each inner block's instructions, then one
    /// tick with their cost) rather than ticking each instruction, so the
    /// run computes what the compiled function would have. Netplay's
    /// guests set it: one started afresh after a rollback's restore
    /// computes what the guest that ran on did.
    pub timed_resumes: bool,
}

impl Default for Nest {
    fn default() -> Nest {
        Nest {
            stats: core::ptr::null_mut(),
            depth: 0,
            stack_top: 0,
            stack_low: 0,
            frame_end: None,
            vblank: false,
            timed_resumes: false,
        }
    }
}

// SAFETY: `stats` is only set while a run, on the thread that owns the
// machine, holds the `&mut Stats` it points to.
unsafe impl Send for Nest {}

impl Nest {
    /// Note how deep the host stack is here, for [`Stats::max_stack`].
    #[inline(always)]
    pub fn sample_stack(&mut self) {
        self.stack_low = self.stack_low.min(stack_here());
    }

    /// Forget a guest run ([`run_guest`]) the front end dropped, suspended,
    /// without letting it return (after a state load or rewind): no run is
    /// in progress now, and there is no switch.
    pub fn forget_run(&mut self) {
        *self = Nest::default();
    }

    /// The counts of the run in progress.
    fn stats(&mut self) -> &mut Stats {
        // SAFETY: see `stats`: the run's caller lent it for the run.
        unsafe { self.stats.as_mut() }.expect("inside a run")
    }
}

/// The dispatcher: [`run_with`]'s loop, with its counts in
/// [`Machine::nest`]. `stop_at`'s guard, if any, is the sp and mode the
/// CPU must also have there.
fn run_inner<P: Platform>(
    m: &mut Machine<P>,
    code: Option<&Code<P>>,
    stop_at: Option<(u32, Option<(u32, u32)>)>,
    blocks: bool,
) -> Stop {
    let lookup = |m: &mut Machine<P>, k| code.and_then(|c| c.lookup(&mut m.bus, k));
    let stops = |m: &Machine<P>, k: u32| match stop_at {
        Some((at, guard)) if at == k => {
            guard.is_none_or(|(sp, cpsr)| m.cpu.r[13] == sp && (m.cpu.cpsr ^ cpsr) & 0x1f == 0)
        }
        _ => false,
    };
    loop {
        if m.fault.is_some() {
            return Stop::Fault;
        }
        if m.sched.frame_ended {
            if !switch(m) {
                return Stop::FrameEnd;
            }
            continue;
        }
        if m.sched.halted {
            let t = m.now();
            m.idle();
            m.nest.stats().halted_cycles += m.now() - t;
            continue;
        }
        let t = m.now();
        m.take_irq();
        m.nest.stats().interpreted_cycles += m.now() - t;
        let k = key(&m.cpu);
        if stops(m, k) {
            return Stop::At(k);
        }
        // A call a mod added came back with nobody waiting there: go on
        // where its block began. A resume at an inner key counts itself
        // as a mod resume below; one at the modded function's entry (the
        // mod runs again from its top) is counted here.
        if modcall::is_call_key(k) {
            let Some(at) = modcall::resume(m, k) else {
                m.fault = Some(crate::machine::Fault::ModCall(k));
                return Stop::Fault;
            };
            let s = m.nest.stats();
            s.mod_call_resumes += 1;
            if code.is_some_and(|c| c.is_entry(at)) {
                s.resumes += 1;
                s.mod_resumes += 1;
            }
            continue;
        }
        let inner = code.and_then(|c| c.find_inner(k));
        // The block-timed interpreter runs a structured function's inner
        // blocks as blocks too.
        if blocks && let (Some(i), Some(c)) = (inner, code) {
            let t = m.now();
            interp_block(m, c.inner_extents[i], false);
            m.nest.stats().compiled_cycles += m.now() - t;
            continue;
        }
        // A resume with [`Nest::timed_resumes`]: the block as compiled
        // code would tick it.
        if let (Some(i), Some(c)) = (inner, code)
            && m.nest.timed_resumes
            && c.inner_extents.len() == c.inner.len()
        {
            let t = m.now();
            interp_block(m, c.inner_extents[i], false);
            let ran = m.now() - t;
            let s = m.nest.stats();
            s.resumes += 1;
            s.interpreted_cycles += ran;
            continue;
        }
        if let Some((f, (ram, i))) = code.and_then(|c| c.find(&mut m.bus, k)) {
            // What runs nested in the call ([`settle`]) counts itself.
            let counted = |s: &Stats| {
                let all = s.compiled_cycles + s.interpreted_cycles + s.halted_cycles;
                (all, s.compiled_ram_cycles)
            };
            let (t, r, n) = (m.now(), m.bus.overlays.cycles, counted(m.nest.stats()));
            if blocks {
                let c = code.expect("found in it");
                interp_block(m, if ram { c.ram_extents[i] } else { c.extents[i] }, ram);
            } else {
                m.nest.stats().calls += 1;
                if is_rom(k) && code.is_some_and(|c| !c.is_entry(k)) {
                    m.nest.stats().resumes += 1;
                    if code.is_some_and(|c| c.is_modded(k)) {
                        m.nest.stats().mod_resumes += 1;
                    }
                }
                let Flow::Jump(next) = f(m, k);
                if yielding(m) {
                    m.nest.stats().yields += 1;
                }
                set_pc(&mut m.cpu, next);
            }
            let (now, ram_now, (all, ram)) =
                (m.now(), m.bus.overlays.cycles, counted(m.nest.stats()));
            let s = m.nest.stats();
            s.compiled_cycles += now - t - (all - n.0);
            s.compiled_ram_cycles += ram_now - r - (ram - n.1);
            continue;
        }
        // The interpreter fallback, until a key has a recompiled block or
        // the run loop must take control. Started inside a structured
        // function, it isn't a seed for the next recompile (decision 6);
        // started or run on into one, where v1 would have stopped at the
        // block's key, it is a resume.
        let mut resumed = inner.is_some();
        let mut modded = resumed && code.is_some_and(|c| c.is_modded(k));
        if !resumed {
            m.nest.stats().seed(k);
        }
        let t = m.now();
        loop {
            let (pc, before) = (m.cpu.pc, m.now());
            let yield_ = m.step();
            m.nest.stats().interpreted += 1;
            if (0x0200_0000..0x0400_0000).contains(&pc) {
                m.nest.stats().interpreted_ram_cycles += m.now() - before;
            }
            let k = key(&m.cpu);
            let inside = |k| is_rom(k) && code.is_some_and(|c| c.find_inner(k).is_some());
            if !resumed && inside(k) {
                // A timed resume runs it as a block.
                if m.nest.timed_resumes {
                    break;
                }
                resumed = true;
                modded = code.is_some_and(|c| c.is_modded(k));
                // The block-timed interpreter runs it as a block.
                if blocks {
                    break;
                }
            }
            if yield_ || stops(m, k) || lookup(m, k).is_some() || modcall::is_call_key(k) {
                break;
            }
        }
        if resumed {
            m.nest.stats().resumes += 1;
            if modded {
                m.nest.stats().mod_resumes += 1;
            }
        }
        m.nest.stats().interpreted_cycles += m.now() - t;
    }
}

/// A call to a function returned `at`, not its return site `ret` (a tail
/// call, an unresolved indirect call, a call through a stub, IWRAM overlay
/// code, code the interpreter ran): run the dispatcher here, nested, until
/// the guest reaches `ret` with the caller's sp (`sp0`, taken before the
/// call) and mode, and return the key it stopped at. The same blocks run in
/// the same order with the same ticks as when the call unwinds to the
/// dispatcher and the caller resumes at `ret`; only the host stack differs.
/// A nested run takes IRQs and halts as the outer one does; the frame end
/// or a fault stops it elsewhere, and the caller unwinds with that key, as
/// it does past [`MAX_DEPTH`].
#[inline(never)]
pub fn settle<P: Platform>(m: &mut Machine<P>, code: &Code<P>, at: u32, ret: u32, sp0: u32) -> u32 {
    // The frame end, a halt or a fault: the outer run takes it.
    if at == ret || m.nest.depth >= MAX_DEPTH || yielding(m) && !m.irq_takeable() {
        return at;
    }
    set_pc(&mut m.cpu, at);
    let mode = m.cpu.cpsr;
    nested(m, code, ret, sp0, mode, |s| {
        (&mut s.settles, &mut s.unsettled)
    });
    key(&m.cpu)
}

/// A tick at the edge to `at` found an IRQ takeable or the CPU halted (its
/// yield path's flags are written): take it here, as the dispatcher would,
/// and run the dispatcher nested until the handler returns to `at` with
/// the same sp and mode. Then `None`: the IRQ was taken at the same tick
/// with the same state, and compiled code goes on at `at`. `Some(key)`
/// when it must unwind with `key` instead: a fault is set (before or
/// during the run), the frame ended with no [`Nest::frame_end`] to switch
/// to the front end, or past [`MAX_DEPTH`].
#[inline(never)]
#[cold]
pub fn yield_at<P: Platform>(m: &mut Machine<P>, code: &Code<P>, at: u32) -> Option<u32> {
    if m.fault.is_some() {
        return Some(at);
    }
    set_pc(&mut m.cpu, at);
    // With the guest on its own stack, the frame end switches to the
    // front end here, and the IRQ the V-blank raised is taken below.
    if m.sched.frame_ended && !switch(m) || m.nest.depth >= MAX_DEPTH {
        return Some(at);
    }
    if !m.sched.halted && !m.irq_takeable() {
        return None;
    }
    let (sp, mode) = (m.cpu.r[13], m.cpu.cpsr);
    if nested(m, code, at, sp, mode, |s| {
        (&mut s.irqs_in_place, &mut s.irqs_unwound)
    }) {
        return None;
    }
    Some(key(&m.cpu))
}

/// The frame ended (`cpu.pc` is exact): with [`Nest::frame_end`], switch
/// to the front end and, once it switches back, go on with the next frame.
/// Whether it did; without it the run unwinds to return.
fn switch<P: Platform>(m: &mut Machine<P>) -> bool {
    let Some(f) = m.nest.frame_end else {
        return false;
    };
    // Compiled code called outside a run has no counts to keep.
    // SAFETY: see `Nest::stats`.
    if let Some(s) = unsafe { m.nest.stats.as_mut() } {
        s.switches += 1;
        // A guest's outermost run doesn't return to note it.
        s.max_stack = s.max_stack.max(m.nest.stack_top - m.nest.stack_low);
    }
    f(m as *mut Machine<P> as *mut ());
    m.sched.frame_ended = false;
    true
}

/// [`settle`]'s and [`yield_at`]'s run: the dispatcher from the CPU's key,
/// nested, until it reaches `stop` with sp `sp` and `mode`'s mode bits.
/// Whether it got there. `counts` names the counts of runs and of runs
/// that stopped elsewhere.
fn nested<P: Platform>(
    m: &mut Machine<P>,
    code: &Code<P>,
    stop: u32,
    sp: u32,
    mode: u32,
    counts: fn(&mut Stats) -> (&mut u64, &mut u64),
) -> bool {
    // Compiled code called outside a run (a test harness's own loop)
    // counts into a scratch copy.
    let mut scratch = None;
    if m.nest.stats.is_null() {
        m.nest.stats = scratch.insert(Stats::default());
    }
    m.nest.depth += 1;
    *counts(m.nest.stats()).0 += 1;
    m.nest.stats().max_depth = m.nest.stats().max_depth.max(m.nest.depth);
    let got = run_inner(m, Some(code), Some((stop, Some((sp, mode)))), false) == Stop::At(stop);
    m.nest.depth -= 1;
    if !got {
        *counts(m.nest.stats()).1 += 1;
    }
    if scratch.is_some() {
        m.nest.stats = core::ptr::null_mut();
    }
    got
}

#[cfg(test)]
mod tests;
