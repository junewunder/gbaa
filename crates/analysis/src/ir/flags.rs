//! Flag liveness (decision 9; `docs/sem-design.md`, "What flag liveness
//! and const-prop need"): delete `SetFlag`s whose flag nothing reads before
//! the next write, so DCE can drop the carry, overflow and test ops that
//! fed them.
//!
//! Within a block the walk is backwards: an unguarded `SetFlag(f)` kills
//! `f`, a guarded one doesn't (the old value survives its false path),
//! `GetFlag(f)` makes `f` live, `GetCpsr` and `GetSpsr` (the CPSR in user
//! and system mode) read all four, and an unguarded
//! `SetCpsr` with the flags mask writes all four. At an exit the live set is
//! whatever its successor needs, which [`remove_dead`] solves over a
//! function's blocks; an exit with no known successor keeps all four live.
//! Mid-block sync points don't change this ("Sync points inside a block").
//!
//! An IRQ, though, can be taken at any exit's tick, and its handler may
//! save the CPSR (Emerald's `IntrMain` does), so each exit's yield path
//! writes the flags whose last write before it was deleted
//! (`docs/structured-codegen-design.md`, step 2). It takes their values
//! from the block's [`Op::YieldFlag`]s, and only a block's own writes have
//! one: so a flag stays live on an edge into a block unless that block
//! writes it, unguarded, before its first exit. A guarded `SetCpsr` with
//! the flags mask reads all four, since the builder forwards no flag
//! through it.

use super::{Block, Op};
use armv4t::sem::{Flag, PsrMask};

/// A set of flags, one bit per [`Flag::index`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags(pub u8);

impl Flags {
    pub const NONE: Flags = Flags(0);
    pub const ALL: Flags = Flags(0b1111);

    pub fn has(self, f: Flag) -> bool {
        self.0 & 1 << f.index() != 0
    }

    fn with(self, f: Flag) -> Flags {
        Flags(self.0 | 1 << f.index())
    }

    fn without(self, f: Flag) -> Flags {
        Flags(self.0 & !(1 << f.index()))
    }
}

impl Block {
    /// Walk the block backwards with `out(i)` live after the exit at inst
    /// `i`. Calls `dead(i)` for each `SetFlag` whose flag is dead after it,
    /// and returns the flags live at entry.
    fn walk_flags(&self, out: impl Fn(usize) -> Flags, mut dead: impl FnMut(usize)) -> Flags {
        let mut live = Flags::NONE;
        for (i, inst) in self.insts.iter().enumerate().rev() {
            let guarded = inst.guard.is_some();
            match inst.op {
                Op::Exit(_) if guarded => live = Flags(live.0 | out(i).0),
                Op::Exit(_) => live = out(i),
                Op::SetFlag(f, _) if !live.has(f) => dead(i),
                Op::SetFlag(f, _) if !guarded => live = live.without(f),
                Op::GetFlag(f) => live = live.with(f),
                // In user and system mode, which have no SPSR, `GetSpsr` reads
                // the CPSR (`Cpu::spsr`).
                Op::GetCpsr | Op::GetSpsr => live = Flags::ALL,
                Op::SetCpsr { mask, .. } if mask.contains(PsrMask::FLAGS) => {
                    live = if guarded { Flags::ALL } else { Flags::NONE }
                }
                _ => {}
            }
        }
        live
    }

    /// The flags the block writes, unguarded, before its first exit: those
    /// whose value at every exit is the block's own.
    pub fn written_before_exit(&self) -> Flags {
        let mut w = Flags::NONE;
        for inst in self.insts.iter().filter(|i| i.guard.is_none()) {
            match inst.op {
                Op::Exit(_) => break,
                Op::SetFlag(f, _) => w = w.with(f),
                Op::SetCpsr { mask, .. } if mask.contains(PsrMask::FLAGS) => w = Flags::ALL,
                _ => {}
            }
        }
        w
    }

    /// The flags live at entry, with `out(i)` live after the exit at inst
    /// `i`.
    pub fn live_flags(&self, out: impl Fn(usize) -> Flags) -> Flags {
        self.walk_flags(out, |_| {})
    }

    /// Delete the `SetFlag`s whose flag is dead, with `out(i)` live after
    /// the exit at inst `i`, and the `YieldFlag`s of flags whose last
    /// write before their exit is kept, then DCE. Returns how many writes
    /// it deleted.
    pub fn remove_dead_flags(&mut self, out: impl Fn(usize) -> Flags) -> usize {
        let mut dead = vec![false; self.insts.len()];
        self.walk_flags(out, |i| dead[i] = true);
        // Forwards: the flags whose last write so far was deleted, which
        // the CPU holds stale.
        let mut stale = Flags::NONE;
        let mut drop = Vec::new();
        for (i, inst) in self.insts.iter().enumerate() {
            let guarded = inst.guard.is_some();
            match inst.op {
                Op::SetFlag(f, _) if dead[i] => stale = stale.with(f),
                Op::SetFlag(f, _) if !guarded => stale = stale.without(f),
                Op::SetCpsr { mask, .. } if !guarded && mask.contains(PsrMask::FLAGS) => {
                    stale = Flags::NONE
                }
                Op::YieldFlag(f, _) if !stale.has(f) => drop.push(i),
                _ => {}
            }
        }
        let removed = dead.iter().filter(|&&d| d).count();
        for (i, _) in dead.iter().enumerate().filter(|(_, d)| **d) {
            self.nop(i);
        }
        for &i in &drop {
            self.nop(i);
        }
        if removed + drop.len() > 0 {
            self.dce();
        }
        removed
    }
}

/// Solve flag liveness over a function's blocks and delete each block's
/// dead `SetFlag`s, then DCE them. `succ(b, i)` is the block the exit at
/// inst `i` of block `b` continues in, or `None` if it isn't a block of
/// this function (then all four flags are live after it). A flag the
/// successor doesn't write before its first exit is live on the edge too
/// (see the module's doc). Returns how many writes it deleted.
pub fn remove_dead(
    blocks: &mut [&mut Block],
    succ: impl Fn(usize, usize) -> Option<usize>,
) -> usize {
    let n = blocks.len();
    // The least fixpoint: start with nothing live and grow.
    let mut live_in = vec![Flags::NONE; n];
    let kept: Vec<Flags> = blocks
        .iter()
        .map(|b| Flags(!b.written_before_exit().0 & Flags::ALL.0))
        .collect();
    // An exit leaving the function (a `swi`, a dynamic jump, a call or
    // return, a jump elsewhere) may reach code that reads any flag.
    let out = |live_in: &[Flags], b: usize, i: usize| {
        succ(b, i).map_or(Flags::ALL, |s| Flags(live_in[s].0 | kept[s].0))
    };
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..n).rev() {
            let l = blocks[b].live_flags(|i| out(&live_in, b, i));
            if l != live_in[b] {
                live_in[b] = l;
                changed = true;
            }
        }
    }
    let mut removed = 0;
    for (b, block) in blocks.iter_mut().enumerate() {
        removed += block.remove_dead_flags(|i| out(&live_in, b, i));
    }
    removed
}
