//! Function shapes from the summaries (`docs/native-stage-b.md`, piece 3;
//! unit `native/lift-params`): which of r0-r3 each lifted function takes
//! as parameters and gives back as results, or that it keeps stage A's
//! four-register form, `fn(g, sp, [u32; 4]) -> [u32; 4]`.
//!
//! A function is *shaped* when its summary ([`crate::saves::summarize`])
//! has no hole and its parameters are complete: none of the callees whose
//! summaries its run used ([`Summary::callees`]: its calls, its tail
//! calls) has a run that stopped at a hole ([`Summary::stopped`]), since
//! a caller reads its callee's parameters.
//! Its parameters are the summary's `params`; its results are what may be
//! read after it returns and it may change:
//!
//! - what its direct callers read after it (`read_after`, `passed_after`,
//!   `forwarded_after`; the summary's `results` are these it defines);
//! - r0, which an indirect call's caller reads (the summaries take an
//!   indirect callee to define r0 and leave r1-r3 undefined), and which a
//!   call through `call_fn` gets back from the function's adapter;
//! - all of r0-r3 where a caller's reads aren't known: a direct caller
//!   whose run stopped at a hole (what it reads past the hole is unseen),
//!   one in another code context (the ROM and an IWRAM copy are
//!   summarised apart), or one with no summary;
//! - what may be read after a function that falls or jumps into its
//!   entry, since its return goes there ([`Shape`]'s demand is a fixed
//!   point over those jumps), and after one that jumps to a function by
//!   value (a tail call through `call_fn`) wherever it may go.
//!
//! A result it may leave as it came on some paths (its entry value) is a
//! parameter too, so that path returns what the caller passed. A register
//! it never changes isn't a result: the caller's value stands.

use crate::func::{Ctx, Func, Plan, exits};
use crate::saves::{Summaries, Summary};
use std::collections::{BTreeMap, BTreeSet};

/// A shaped function's registers (bit `n` for rN, r0-r3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub params: u8,
    pub results: u8,
}

/// The registers of `m` (bit `n` for rN), in order.
pub fn regs(m: u8) -> impl Iterator<Item = usize> {
    (0..4).filter(move |r| m & 1 << r != 0)
}

/// What one function does that the shapes depend on, from its exits as
/// the lifter prints them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    /// Its direct calls' targets.
    pub calls: BTreeSet<u32>,
    /// The other functions' entries it falls or jumps into (a call's
    /// return site included), which return to its caller.
    pub jumps: BTreeSet<u32>,
    /// Its jumps by value (returns, and tail calls through a register),
    /// by address: one its run didn't prove a return may be a tail call
    /// to any function.
    pub by_value: BTreeSet<u32>,
}

/// `f`'s edges in `ctx`, `all` every function's key. `targets` gives a
/// jump-by-value exit's known targets (the lifter's: a table's cases, the
/// holes file's), by the exit's address.
pub fn edges(
    ctx: &Ctx,
    f: &Func,
    all: &BTreeSet<u32>,
    targets: impl Fn(u32, bool, Option<u32>) -> Vec<u32>,
) -> Edges {
    let mut e = Edges::default();
    let jump = |e: &mut Edges, k: u32| {
        if !f.blocks.contains_key(&k) && all.contains(&k) {
            e.jumps.insert(k);
        }
    };
    for gb in f.blocks.values() {
        for x in exits(ctx, gb) {
            match x.plan {
                Plan::Static(k) | Plan::Rekey(k) => jump(&mut e, k),
                Plan::Call { target, ret } => {
                    e.calls.insert(target);
                    jump(&mut e, ret);
                }
                Plan::CallVia { ret, .. } => jump(&mut e, ret),
                Plan::Dynamic {
                    ret: Some(ret),
                    target,
                    ..
                } => {
                    e.calls.extend(target.filter(|t| all.contains(t)));
                    jump(&mut e, ret);
                }
                Plan::Dynamic {
                    ret: None, target, ..
                } => {
                    for k in targets(x.at, gb.block.thumb, target) {
                        jump(&mut e, k);
                    }
                    e.by_value.insert(x.at);
                }
                Plan::Swi { next } => jump(&mut e, next),
                Plan::Undefined => {}
            }
        }
    }
    e
}

/// One function, as the shapes see it.
pub struct Node<'a> {
    /// Its code context (the ROM, or an IWRAM copy), by index.
    pub ctx: usize,
    pub sum: Option<&'a Summary>,
    pub edges: Edges,
    /// A stand-in from the holes file replaces it: it keeps the
    /// four-register form.
    pub standin: bool,
}

/// The shapes, and why the rest have none.
#[derive(Debug, Default)]
pub struct Shapes {
    pub shaped: BTreeMap<u32, Shape>,
    /// Lifted functions whose summary is clean but whose parameters rest
    /// on a callee's whose run stopped at a hole: the first such callee.
    pub callee_hole: BTreeMap<u32, u32>,
    /// The registers a jump by value's tail call may leave read, added to
    /// every function's results.
    pub by_value: u8,
}

/// Every function's shape. `nodes` are all functions, by key.
pub fn shapes(nodes: &BTreeMap<u32, Node>) -> Shapes {
    // Parameters are incomplete where the run stopped, and in everything
    // that calls or jumps into such a function.
    let mut unsound: BTreeMap<u32, u32> = BTreeMap::new();
    let stopped = |n: &Node| n.sum.is_none_or(|s| s.stopped);
    loop {
        let mut grew = false;
        for (&k, n) in nodes {
            if unsound.contains_key(&k) {
                continue;
            }
            let Some(sum) = n.sum else { continue };
            let bad = sum.callees.iter().find(|c| {
                nodes
                    .get(c)
                    .is_some_and(|m| m.ctx == n.ctx && (stopped(m) || unsound.contains_key(c)))
            });
            if let Some(&c) = bad {
                unsound.insert(k, c);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    // What may be read after each returns.
    let mut demand: BTreeMap<u32, u8> = nodes
        .iter()
        .map(|(&k, n)| {
            let d = match n.sum {
                Some(s) => (s.read_after | s.passed_after | s.forwarded_after) as u8 & 0xf | 1,
                None => 0xf,
            };
            (k, d)
        })
        .collect();
    for n in nodes.values() {
        let blind = stopped(n) || n.standin;
        for c in &n.edges.calls {
            if let Some(m) = nodes.get(c)
                && (blind || m.ctx != n.ctx)
            {
                *demand.get_mut(c).expect("every node has a demand") = 0xf;
            }
        }
    }
    let mut by_value = 0u8;
    loop {
        let mut grew = false;
        for (&k, n) in nodes {
            let d = demand[&k];
            // A jump the run proved a return (to the entry lr) isn't a
            // tail call.
            let unknown = |at: &u32| {
                n.sum.is_none_or(|s| {
                    s.holes
                        .contains(&format!("0x{at:08x}: a jump to an unknown place"))
                })
            };
            if n.edges.by_value.iter().any(unknown) && by_value | d != by_value {
                by_value |= d;
                grew = true;
            }
            for j in &n.edges.jumps {
                if let Some(t) = demand.get_mut(j)
                    && *t | d != *t
                {
                    *t |= d;
                    grew = true;
                }
            }
        }
        for t in demand.values_mut() {
            if *t | by_value != *t {
                *t |= by_value;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let mut out = Shapes {
        by_value,
        ..Shapes::default()
    };
    for (&k, n) in nodes {
        let Some(s) = n.sum else { continue };
        if n.standin || !s.holes.is_empty() {
            continue;
        }
        if let Some(&c) = unsound.get(&k) {
            out.callee_hole.insert(k, c);
            continue;
        }
        out.shaped.insert(k, shape(s, demand[&k]));
    }
    out
}

/// A clean summary's shape, given what may be read after it returns.
fn shape(s: &Summary, demand: u8) -> Shape {
    if !s.returns {
        return Shape {
            params: s.params,
            results: 0,
        };
    }
    let (mut changed, mut entry) = (0u8, 0u8);
    for r in 0..4 {
        let b = s.exit[r];
        if b.entry {
            entry |= 1 << r;
        }
        if b.def || b.clob || !b.entry {
            changed |= 1 << r;
        }
    }
    let results = demand & changed | s.results as u8 & 0xf;
    Shape {
        params: s.params | results & entry,
        results,
    }
}

/// The summaries of one context's functions, by key.
pub fn nodes_of<'a>(
    ctx_index: usize,
    funcs: &[Func],
    sums: &'a Summaries,
    edges: impl Fn(&Func) -> Edges,
    standin: impl Fn(u32) -> bool,
) -> Vec<(u32, Node<'a>)> {
    funcs
        .iter()
        .map(|f| {
            (
                f.entry,
                Node {
                    ctx: ctx_index,
                    sum: sums.get(f.entry),
                    edges: edges(f),
                    standin: standin(f.entry),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saves::Bits;

    fn sum(params: u8, results: u16, exit: [(bool, bool); 4]) -> Summary {
        let mut s = Summary {
            params,
            results,
            read_after: results,
            returns: true,
            keeps: true,
            ..Summary::default()
        };
        for (b, (entry, def)) in s.exit.iter_mut().zip(exit) {
            *b = Bits {
                entry,
                def,
                clob: false,
            };
        }
        s
    }

    fn node<'a>(s: &'a Summary, calls: &[u32], jumps: &[u32]) -> Node<'a> {
        assert!(
            calls.iter().all(|c| s.callees.contains(c)),
            "a test's callees are its summary's"
        );
        Node {
            ctx: 0,
            sum: Some(s),
            edges: Edges {
                calls: calls.iter().copied().collect(),
                jumps: jumps.iter().copied().collect(),
                by_value: BTreeSet::new(),
            },
            standin: false,
        }
    }

    const KEEP: (bool, bool) = (true, false);
    const DEF: (bool, bool) = (false, true);
    const MIXED: (bool, bool) = (true, true);

    #[test]
    fn a_clean_function_takes_its_params_and_gives_what_is_read_and_r0() {
        // f(r0, r1) -> r1 read by a caller; r0 and r2 changed, r3 kept.
        let f = sum(0b0011, 0b0010, [DEF, DEF, DEF, KEEP]);
        let mut main = sum(0, 0, [DEF, DEF, DEF, DEF]);
        main.callees.insert(1);
        let nodes = BTreeMap::from([(1, node(&f, &[], &[])), (2, node(&main, &[1], &[]))]);
        let s = shapes(&nodes);
        assert_eq!(
            s.shaped[&1],
            Shape {
                params: 0b0011,
                results: 0b0011
            }
        );
    }

    #[test]
    fn a_result_it_may_leave_as_it_came_is_a_parameter() {
        // r1 read after it, defined on one path and kept on the other.
        let mut f = sum(0b0001, 0, [DEF, MIXED, KEEP, KEEP]);
        f.read_after = 0b0010;
        let nodes = BTreeMap::from([(1, node(&f, &[], &[]))]);
        assert_eq!(
            shapes(&nodes).shaped[&1],
            Shape {
                params: 0b0011,
                results: 0b0011
            }
        );
    }

    #[test]
    fn a_caller_whose_run_stopped_reads_everything_it_changes() {
        let f = sum(0b0001, 0b0001, [DEF, DEF, KEEP, DEF]);
        let mut a = sum(0, 0, [DEF; 4]);
        a.callees.insert(1);
        a.holes
            .insert("jumps to 0x08000100, which has no code here".into());
        a.stopped = true;
        let nodes = BTreeMap::from([(1, node(&f, &[], &[])), (2, node(&a, &[1], &[]))]);
        let s = shapes(&nodes);
        assert_eq!(s.shaped[&1].results, 0b1011);
        assert!(!s.shaped.contains_key(&2), "a holey function keeps four");
    }

    #[test]
    fn a_function_falling_in_passes_on_what_its_callers_read() {
        // a falls into f; a's caller reads r2 after a.
        let f = sum(0b0001, 0, [DEF, DEF, DEF, DEF]);
        let mut a = sum(0, 0, [DEF; 4]);
        a.read_after = 0b0100;
        let nodes = BTreeMap::from([(1, node(&f, &[], &[])), (2, node(&a, &[], &[1]))]);
        assert_eq!(shapes(&nodes).shaped[&1].results, 0b0101);
    }

    #[test]
    fn a_callee_whose_run_stopped_leaves_its_callers_four_registers() {
        let mut c = sum(0b0001, 0, [DEF; 4]);
        c.holes
            .insert("0x08000010: a jump to an unknown place".into());
        c.stopped = true;
        let mut f = sum(0b0001, 0, [DEF; 4]);
        f.callees.insert(1);
        let mut g = sum(0, 0, [DEF; 4]);
        g.callees.insert(2);
        let nodes = BTreeMap::from([
            (1, node(&c, &[], &[])),
            (2, node(&f, &[1], &[])),
            (3, node(&g, &[2], &[])),
        ]);
        let s = shapes(&nodes);
        assert_eq!(s.callee_hole[&2], 1);
        assert_eq!(s.callee_hole[&3], 2);
        assert!(s.shaped.is_empty());
    }

    #[test]
    fn a_fall_its_run_went_through_rests_on_no_summary_of_the_target() {
        // a falls into b with r4 changed: a's run goes on through b's code
        // (b isn't among its summary's callees), so b's stop doesn't touch
        // a's parameters; b's return goes to a's callers.
        let mut b = sum(0b0001, 0, [DEF; 4]);
        b.holes
            .insert("0x08000192: a jump to an unknown place".into());
        b.stopped = true;
        let mut a = sum(0b0001, 0b0001, [DEF; 4]);
        a.read_after = 0b0010;
        let nodes = BTreeMap::from([(1, node(&b, &[], &[])), (2, node(&a, &[], &[1]))]);
        let s = shapes(&nodes);
        assert_eq!(
            s.shaped[&2],
            Shape {
                params: 0b0001,
                results: 0b0011
            }
        );
        assert!(!s.shaped.contains_key(&1));
    }

    #[test]
    fn a_function_that_never_returns_gives_nothing() {
        let mut f = sum(0b0011, 0, [DEF; 4]);
        f.returns = false;
        let nodes = BTreeMap::from([(1, node(&f, &[], &[]))]);
        assert_eq!(shapes(&nodes).shaped[&1].results, 0);
    }
}
