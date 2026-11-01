//! Jump-table recovery (decision 5c): value-set analysis over every
//! function of a [`Program`], with no per-compiler matchers.
//!
//! Every function is analysed from its entry ([`vsa::func::analyze`]). The
//! far-jump `bl` rule (decision 5a) is a fixpoint over the same analyses: a
//! `bl` to `t` is a jump if every return reached from `t` leaves sp above
//! its entry value (it pops the caller's frame). Analysing from `t` can meet
//! more `bl`s, so the answers depend on each other; an analysis that asked
//! about `u` is redone whenever `u`'s answer changes.
//!
//! A jump counts as resolved only if every function context reaching it
//! resolves it; its targets are then the union over the contexts.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use crate::program::Program;
use crate::vsa::func::{Access, Bl, FuncResult, Jump, JumpRes, STEP_CAP, analyze_capped};

/// How often one far-jump answer may change in one [`Engine::run`] before
/// it is frozen for the rest of that run. The answers aren't monotone (a
/// `bl` turning into a jump changes which returns the probe reaches), so
/// this bounds the work.
pub const MAX_UPDATES: u32 = 8;

#[derive(Debug, Default)]
pub struct Recovery {
    /// Every indirect jump reached from a function entry.
    pub jumps: BTreeMap<u32, JumpRes>,
    /// The `bl`s taken as far jumps, over all function contexts.
    pub far_jumps: BTreeSet<u32>,
    /// Analyses run, including the redone ones.
    pub analyses: usize,
    /// Far-jump answers that hit [`MAX_UPDATES`] in some run.
    pub frozen: usize,
    /// Analyses that hit the step cap.
    pub capped: usize,
    /// `bl` targets answered as never returning: nothing after a call to
    /// one is analysed.
    pub no_returns: usize,
    /// Instructions reached from function entries.
    pub reached: usize,
    /// The loads and stores reached, by instruction address, joined over
    /// every root that reaches them (unknown, from a capped analysis).
    pub accesses: BTreeMap<u32, Access>,
}

impl Recovery {
    /// The resolved jumps.
    pub fn resolved(&self) -> impl Iterator<Item = (u32, &Jump)> {
        self.jumps.iter().filter_map(|(a, r)| match r {
            JumpRes::Resolved(j) => Some((*a, j)),
            _ => None,
        })
    }
}

/// An analysis root: an entry address and its instruction set.
pub type Key = (u32, bool);

/// What a probe from a `bl` target says the `bl` is, given the answer so
/// far. A jump if every return with sp known pops the caller's frame, a
/// balanced call if every one leaves sp where it was on entry. A
/// probe that reaches no return with sp known keeps a jump: cases of one
/// big function that far-jump to each other reach no return once they are
/// taken as jumps, and the epilogue that pops the frame once they are
/// calls, so dropping the answer there would flip it forever. But a probe
/// stopped by calls that never return, or by data, says the target
/// doesn't return either (LLVM's `handle_error`, whose first probe ran
/// past such a call into whatever followed, before the call or the pool
/// after it was known): it reaches no return at all, every indirect jump
/// resolved and none leaving it (an interworking thunk's `bx` into ARM
/// code).
fn probe_says(r: &FuncResult, old: Bl) -> Bl {
    if r.capped {
        return Bl::Call;
    }
    let known: Vec<i32> = r.ret_sp.iter().flatten().copied().collect();
    if !known.is_empty() {
        return if known.iter().all(|&o| o > 0) {
            Bl::Jump
        } else if known.iter().all(|&o| o == 0) {
            Bl::Balanced
        } else {
            Bl::Call
        };
    }
    let resolved = r.jumps.values().all(|j| matches!(j, JumpRes::Resolved(_)));
    let ends = r.ret_sp.is_empty() && r.exits.is_empty() && resolved;
    if old == Bl::Jump && !(ends && (r.stopped || !r.no_returns.is_empty())) {
        Bl::Jump
    } else if ends {
        Bl::NoReturn
    } else {
        Bl::Call
    }
}

/// The analyses of a [`Program`] and the far-jump answers they depend on,
/// kept so that code discovery can add functions and redo only the
/// analyses a change touches.
#[derive(Default)]
pub struct Engine {
    pub results: HashMap<Key, FuncResult>,
    answer: HashMap<Key, Bl>,
    /// Answer changes in the current run.
    updates: HashMap<Key, u32>,
    /// Answers that hit [`MAX_UPDATES`] in some run.
    frozen: HashSet<Key>,
    /// Who asked about each target.
    askers: HashMap<Key, BTreeSet<Key>>,
    /// Analyses run, including the redone ones.
    pub analyses: usize,
    /// Print analyses slower than 0.2 s to stderr.
    pub log: bool,
    /// The step cap of each analysis, if not [`STEP_CAP`].
    pub step_cap: Option<u64>,
    /// Keys whose results are final: once analysed, never analysed again,
    /// so their answers don't change either.
    pub kept: HashSet<Key>,
}

impl Engine {
    /// Analyse `keys`, again if they were analysed before, and every probe
    /// they ask about, until the far-jump answers settle.
    pub fn run(&mut self, prog: &Program, keys: impl IntoIterator<Item = Key>) {
        let mut queue: VecDeque<Key> = VecDeque::new();
        let mut queued: HashSet<Key> = HashSet::new();
        self.updates.clear();
        for k in keys {
            if queued.insert(k) {
                queue.push_back(k);
            }
        }
        while let Some(k) = queue.pop_front() {
            queued.remove(&k);
            if self.kept.contains(&k) && self.results.contains_key(&k) {
                continue;
            }
            let mut asked = Vec::new();
            let answer = &self.answer;
            let t0 = std::time::Instant::now();
            let cap = self.step_cap.unwrap_or(STEP_CAP);
            let r = analyze_capped(
                prog,
                k.0,
                k.1,
                &mut |t, thumb| {
                    asked.push((t, thumb));
                    answer.get(&(t, thumb)).copied().unwrap_or_default()
                },
                cap,
            );
            self.analyses += 1;
            if self.log && t0.elapsed().as_secs_f64() > 0.2 {
                eprintln!(
                    "  slow analysis 0x{:08x} {}: {:.1} s, {} reached, capped {}",
                    k.0,
                    if k.1 { "thumb" } else { "arm" },
                    t0.elapsed().as_secs_f64(),
                    r.reached,
                    r.capped
                );
            }
            for u in asked {
                self.askers.entry(u).or_default().insert(k);
                if !self.results.contains_key(&u) && u != k && queued.insert(u) {
                    queue.push_back(u);
                }
            }
            let old = self.answer.get(&k).copied().unwrap_or_default();
            let far = probe_says(&r, old);
            self.results.insert(k, r);
            if far != self.answer.get(&k).copied().unwrap_or_default() {
                let n = self.updates.entry(k).or_default();
                if *n < MAX_UPDATES {
                    *n += 1;
                    self.answer.insert(k, far);
                    for &a in self.askers.get(&k).into_iter().flatten() {
                        if queued.insert(a) {
                            queue.push_back(a);
                        }
                    }
                } else {
                    self.frozen.insert(k);
                }
            }
        }
    }

    /// The far-jump answer for a `bl` to `k`, probing `k` first if no
    /// analysis has asked about it yet.
    pub fn far(&mut self, prog: &Program, k: Key) -> Bl {
        if !self.results.contains_key(&k) {
            self.run(prog, [k]);
        }
        self.answer.get(&k).copied().unwrap_or_default()
    }

    /// The far-jump answer for a `bl` to `k` so far, without probing.
    pub fn known(&self, k: Key) -> Bl {
        self.answer.get(&k).copied().unwrap_or_default()
    }

    /// The analysed keys, other than [`kept`](Engine::kept) ones, whose
    /// results `stale` says are out of date.
    pub fn stale(&self, stale: impl Fn(&FuncResult) -> bool) -> Vec<Key> {
        let mut v: Vec<Key> = self
            .results
            .iter()
            .filter(|(k, r)| !self.kept.contains(k) && stale(r))
            .map(|(k, _)| *k)
            .collect();
        v.sort();
        v
    }

    /// The recovery over the analyses of `roots` (each analysed already).
    pub fn collect(&self, roots: impl IntoIterator<Item = Key>) -> Recovery {
        let mut rec = Recovery {
            analyses: self.analyses,
            frozen: self.frozen.len(),
            no_returns: self.answer.values().filter(|&&b| b == Bl::NoReturn).count(),
            capped: self.results.values().filter(|r| r.capped).count(),
            ..Recovery::default()
        };
        let mut seen = HashSet::new();
        for k in roots {
            if !seen.insert(k) {
                continue;
            }
            let r = &self.results[&k];
            rec.reached += r.reached;
            rec.far_jumps.extend(&r.far_jumps);
            for (&a, &x) in &r.accesses {
                // Without a fixpoint nothing is known: the address could be
                // anything, as a capped analysis's jumps are unresolved.
                let x = if r.capped {
                    Access {
                        top: true,
                        ..Access::default()
                    }
                } else {
                    x
                };
                let e = rec.accesses.entry(a).or_insert(x);
                *e = e.join(x);
            }
            for (&a, res) in &r.jumps {
                // Without a fixpoint nothing is known.
                let res = if r.capped { &JumpRes::Unresolved } else { res };
                let merged = match (rec.jumps.remove(&a), res) {
                    (None, r) => r.clone(),
                    (Some(JumpRes::Resolved(old)), JumpRes::Resolved(new)) => {
                        JumpRes::Resolved(union(old, new))
                    }
                    // Some context failed: the jump isn't resolved.
                    (Some(JumpRes::Resolved(_)), r) => r.clone(),
                    (Some(failed), _) => failed,
                };
                rec.jumps.insert(a, merged);
            }
        }
        rec
    }
}

/// Recover the jump tables of every function of `prog`.
pub fn recover(prog: &Program) -> Recovery {
    recover_with(prog).1
}

/// [`recover`], also returning the engine, for asking about far jumps.
pub fn recover_with(prog: &Program) -> (Engine, Recovery) {
    let mut e = Engine::default();
    let roots: Vec<Key> = prog.funcs().iter().map(|f| (f.start, f.thumb)).collect();
    e.run(prog, roots.iter().copied());
    let rec = e.collect(roots);
    (e, rec)
}

fn union(mut a: Jump, b: &Jump) -> Jump {
    a.targets.extend(&b.targets);
    a.targets.sort_unstable();
    a.targets.dedup();
    a.table = match (a.table, b.table) {
        (Some(x), Some(y)) if x.size == y.size => Some(crate::vsa::domain::Src {
            first: x.first.min(y.first),
            last: x.last.max(y.last),
            size: x.size,
        }),
        _ => None,
    };
    a
}

#[cfg(test)]
mod tests;
