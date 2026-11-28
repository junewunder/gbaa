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
//! - what its callers read after it (`read_after`, `passed_after`,
//!   `forwarded_after`: its direct callers, and indirect ones whose
//!   complete callee set has it; the summary's `results` are these it
//!   defines);
//! - r0, where a pointer can reach it ([`Node::addressed`], `FNS`'s
//!   functions; native/lift-returns-read): an indirect call's caller reads
//!   it where the callee set is incomplete (the summaries take such a
//!   callee to define r0 and leave r1-r3 undefined), and a call through
//!   `call_fn` gets it back from the function's adapter. One no pointer
//!   reaches gives back only what its callers read (a C `void` nothing);
//! - what a direct caller whose run stopped at a hole may read past it:
//!   the callee's results its paths still hold in r0-r3 there
//!   (`hidden_after`; native/lift-returns-past-holes), as a path that
//!   wrote over one first can't read it and a read before the hole is
//!   seen;
//! - all of r0-r3 where a caller's reads aren't known: one whose run was
//!   cut short ([`Summary::blind`]), one in another code context (the ROM
//!   and an IWRAM copy are summarised apart), or one with no summary;
//! - what may be read after a function that returns its value unread
//!   (`return f(x);`, the summary's `forwards`), wherever that is;
//! - all four after a call by value whose target the lifter knows (the
//!   summaries take it as an indirect call, so its caller's reads reach
//!   no callee);
//! - what may be read after a function that falls or jumps into its
//!   entry, since its return goes there ([`Shape`]'s demand is a fixed
//!   point over those jumps), and after one that jumps to a function by
//!   value (a tail call through `call_fn`) where it may go: any function
//!   a pointer reaches (a jump after a context restore, `longjmp`'s, goes
//!   back into its save's caller by unwinding, no function's return).
//!   What reaches a function only by a jump whose jumper's run went on
//!   through its code and proved that jump a return (m4a's mixer: a
//!   Thumb half pushing lr, jumping into its ARM half, which pops it) is
//!   no tail call's (native/lift-returns-past-holes); nor is anything a
//!   stand-in that only forwards to a lifted function reads, a jump into
//!   that function ([`forwarded_to`]: Metroid's IWRAM copy whose switch
//!   goes back into the ROM function's cases).
//!
//! A result it may leave as it came on some paths (its entry value) is a
//! parameter too, so that path returns what the caller passed. A register
//! it never changes isn't a result: the caller's value stands; nor is one
//! that holds only its entry lr on every return ([`Summary::lr_left`]:
//! agbcc's `pop {r0}; bx r0`), the return address, no value. One it may
//! change but no caller reads isn't a result either ([`changed`]), and a
//! caller's copy of it is dead across the call (`FnPrinter::clobbers`):
//! a scratch register clobbered prints as nothing. Hand-written code
//! returning in r1-r3 (`__aeabi_uidivmod`'s remainder) keeps them, as
//! its callers read them.
//!
//! **Prototypes** (unit `native/lift-signature-holes`; the owner's call,
//! 2026-11-20). Where the names files give a function's prototype
//! ([`Node::proto`]: `crate::saves::proto_regs`), its shape is the
//! prototype's, hole or not: its parameters and its one value or none.
//! The summaries stay the safety check: the prototype is refused, and the
//! function keeps the summaries' shape (four registers where they give
//! none), where they prove it wrong ([`refusal`]): the function reads a
//! register past the prototype's parameters (an instruction of its own, a
//! callee's or a BIOS call's at the end of a chain it passes it on
//! through; not an indirect call, which the summaries take to read all
//! four), a caller reads a register after it that the prototype doesn't
//! return ([`Node::proven_after`], carried over `return f(x);` and falls
//! as the demand is), it returns a result as it came that the prototype
//! doesn't pass, or it takes or gives r4-r11. What the summaries can't
//! prove (a read past a hole, a caller's past its own) the prototype
//! settles: the native check and fn-check are what catch a wrong one, and
//! `gbaa-lift --plant-proto=KEY` ([`Node::forced`]) plants one the check
//! must catch. A stand-in keeps four registers.

use crate::func::{Ctx, Func, Plan, exits};
use crate::saves::{Cause, Summaries, Summary};
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
    /// Those made through a register whose target is known (the lifter
    /// calls them directly; the summaries take them as indirect, so what
    /// is read after them reaches no callee's summary).
    pub calls_by_value: BTreeSet<u32>,
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
                    e.calls_by_value.extend(target.filter(|t| all.contains(t)));
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
                Plan::Undefined { .. } | Plan::Unwind => {}
            }
        }
    }
    e
}

/// The function a stand-in only forwards to, if its code is that and no
/// more: a body `a_<key>(g, sp, a)` or `f_<key>(g, sp, a)` (Metroid's
/// IWRAM copy of a ROM function, whose switch jumps back into the ROM
/// one's cases). Its edges are a jump into that function, which returns
/// to the stand-in's caller: what is read after the stand-in is read
/// after it, and the copy's own jumps (an unknown one, as a tail call by
/// value) never run.
pub fn forwarded_to(code: &str) -> Option<u32> {
    let body = code.get(code.find('{')? + 1..code.rfind('}')?)?.trim();
    let body = body.strip_suffix(';').unwrap_or(body).trim();
    let rest = body
        .strip_prefix("a_")
        .or_else(|| body.strip_prefix("f_"))?;
    let (key, args) = rest.split_at_checked(8)?;
    let args: String = args.chars().filter(|c| !c.is_whitespace()).collect();
    if args != "(g,sp,a)" || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(key, 16).ok()
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
    /// The stand-in is the lift's own code: the per-function check's
    /// replacement of a function by its lift or a mod's cut of it
    /// (native/cut-callee-shapes). It reads its callees' results as the
    /// lift does, so they keep the lift's shapes, which a cut was printed
    /// against.
    pub lifted: bool,
    /// Its address may be taken (`FNS`'s: a pointer can reach it), so
    /// an indirect call whose callee set is incomplete, or a tail call
    /// by value, may reach it.
    pub addressed: bool,
    /// Its prototype's shape, from the names files ([`crate::saves::proto_regs`]).
    pub proto: Option<Shape>,
    /// The prototype is taken unchecked (the planted wrong prototype).
    pub forced: bool,
    /// Its parameters past what the prototype could leave out: the
    /// registers the summary's causes prove read ([`proven_params`]).
    pub proven_params: u8,
    /// What its callers' runs prove read after it: their own
    /// instructions' reads, and passing on to a callee whose chain ends in
    /// one ([`proven_after`]).
    pub proven_after: u8,
    /// Where each proof is, for the report: by register, the reading
    /// instruction and its function (params first, then reads after).
    pub proofs: BTreeMap<u8, String>,
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
    /// The shaped functions whose shape is their prototype's.
    pub from_proto: BTreeSet<u32>,
    /// The functions whose prototype the summaries prove wrong, and why
    /// ([`refusal`]): they keep the summaries' shape, or four registers.
    pub refused: BTreeMap<u32, String>,
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
                Some(s) => {
                    (s.read_after | s.passed_after | s.forwarded_after | s.hidden_after) as u8 & 0xf
                        | u8::from(n.addressed)
                }
                None => 0xf,
            };
            (k, d)
        })
        .collect();
    for n in nodes.values() {
        // A caller whose run stopped at a hole reads past it what its
        // paths still held of the callee's results there (the callee's
        // `hidden_after`); one cut short, or with no summary, any.
        let blind = n.sum.is_none_or(|s| s.blind) || n.standin && !n.lifted;
        for c in &n.edges.calls {
            if let Some(m) = nodes.get(c)
                && (blind || m.ctx != n.ctx || n.edges.calls_by_value.contains(c))
            {
                *demand.get_mut(c).expect("every node has a demand") = 0xf;
            }
        }
    }
    // What may be read after each returns on the paths where its jumps
    // by value are its own: not those of a function jumping into it whose
    // run went on through its code and proved them (a Thumb half pushing
    // lr and jumping into its ARM half, which pops it: m4a's mixer), as
    // there they return to that one's caller.
    let mut tail = demand.clone();
    let mut by_value = 0u8;
    loop {
        let mut grew = false;
        for (&k, n) in nodes {
            let d = demand[&k];
            // A jump the run proved a return (to the entry lr) isn't a
            // tail call.
            let unknown = |s: Option<&Summary>, at: &u32| {
                s.is_none_or(|s| {
                    s.holes
                        .contains(&format!("0x{at:08x}: a jump to an unknown place"))
                })
            };
            if n.edges.by_value.iter().any(|at| unknown(n.sum, at)) {
                let t = tail[&k];
                if by_value | t != by_value {
                    by_value |= t;
                    grew = true;
                }
            }
            // A value it returns from a callee unread (`return f(x);`)
            // is read where its own is.
            let forwards = n.sum.map(|s| &s.forwards).into_iter().flatten();
            for (&r, cs) in forwards {
                if r > 3 || d & 1 << r == 0 {
                    continue;
                }
                for c in cs {
                    for m in [&mut demand, &mut tail] {
                        if let Some(t) = m.get_mut(c)
                            && *t & 1 << r == 0
                        {
                            *t |= 1 << r;
                            grew = true;
                        }
                    }
                }
            }
            for j in &n.edges.jumps {
                if let Some(t) = demand.get_mut(j)
                    && *t | d != *t
                {
                    *t |= d;
                    grew = true;
                }
                // Its run went on through j's code (j is no callee of its
                // summary), unstopped, and proved j's jumps by value.
                let through = n.sum.is_some_and(|s| {
                    !s.stopped
                        && !s.blind
                        && !s.callees.contains(j)
                        && nodes.get(j).is_some_and(|m| {
                            m.ctx == n.ctx
                                && !m.edges.by_value.iter().any(|at| unknown(Some(s), at))
                        })
                });
                if !through
                    && let Some(t) = tail.get_mut(j)
                    && *t | d != *t
                {
                    *t |= d;
                    grew = true;
                }
            }
        }
        // A tail call by value goes only where a pointer can.
        for (k, n) in nodes {
            if n.addressed {
                for m in [&mut demand, &mut tail] {
                    let t = m.get_mut(k).expect("every node has a demand");
                    if *t | by_value != *t {
                        *t |= by_value;
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }
    // What callers are proven to read after each returns, carried as
    // the demand is: over `return f(x);` and into what it jumps into.
    let mut proven: BTreeMap<u32, u8> = nodes.iter().map(|(&k, n)| (k, n.proven_after)).collect();
    loop {
        let mut grew = false;
        for (&k, n) in nodes {
            let d = proven[&k];
            let forwards = n.sum.map(|s| &s.forwards).into_iter().flatten();
            for (&r, cs) in forwards {
                if r > 3 || d & 1 << r == 0 {
                    continue;
                }
                for c in cs {
                    if let Some(t) = proven.get_mut(c)
                        && *t & 1 << r == 0
                    {
                        *t |= 1 << r;
                        grew = true;
                    }
                }
            }
            for j in &n.edges.jumps {
                if let Some(t) = proven.get_mut(j)
                    && *t | d != *t
                {
                    *t |= d;
                    grew = true;
                }
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
        if n.standin {
            continue;
        }
        if let Some(p) = n.proto {
            match refusal(n, p, proven[&k]) {
                Some(why) if !n.forced => {
                    out.refused.insert(k, why);
                }
                _ => {
                    out.shaped.insert(k, p);
                    out.from_proto.insert(k);
                    continue;
                }
            }
        }
        let Some(s) = n.sum else { continue };
        if !s.holes.is_empty() {
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

/// Why the summaries prove prototype shape `p` wrong for `n`, if they
/// do; `proven` is what its callers are proven to read after it.
fn refusal(n: &Node, p: Shape, proven: u8) -> Option<String> {
    let list = |m: u8| {
        regs(m)
            .map(|r| format!("r{r}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // Where the proof is, for the first register of `m`.
    let at = |m: u8, after: bool| {
        regs(m)
            .next()
            .and_then(|r| n.proofs.get(&(r as u8 + if after { 4 } else { 0 })))
            .map_or(String::new(), |w| format!(" ({w})"))
    };
    let extra = n.proven_params & !p.params;
    if extra != 0 {
        return Some(format!(
            "reads {}, which its prototype doesn't pass{}",
            list(extra),
            at(extra, false)
        ));
    }
    let extra = proven & 0xf & !p.results;
    if extra != 0 {
        return Some(format!(
            "a caller reads {} after it, which its prototype doesn't return{}",
            list(extra),
            at(extra, true)
        ));
    }
    let s = n.sum?;
    let entry = (0..4)
        .filter(|&r| s.exit[r].entry)
        .fold(0u8, |m, r| m | 1 << r);
    let extra = p.results & entry & !p.params;
    if extra != 0 && s.returns {
        return Some(format!(
            "may return {} as it came, which its prototype doesn't pass",
            list(extra)
        ));
    }
    if p.results & s.lr_left != 0 {
        return Some(format!(
            "returns its return address in {}",
            list(p.results & s.lr_left)
        ));
    }
    if s.passed() != 0 {
        return Some("takes r4-r11 from its callers".into());
    }
    if s.gives != 0 {
        return Some("gives r4-r11 back to its callers".into());
    }
    None
}

/// Where [`proven_params`] and [`proven_after`] found each register (by
/// register; a read after it at the register plus 4), as the report
/// words it.
pub fn proofs(sums: &Summaries, k: u32) -> BTreeMap<u8, String> {
    let mut out = BTreeMap::new();
    let Some(s) = sums.get(k) else { return out };
    for &r in s.causes.keys().filter(|&&r| r < 4) {
        match sums.root(k, r) {
            Some((f, Cause::Reads(at))) => {
                out.insert(r, format!("read at 0x{at:08x}, in 0x{f:08x}"));
            }
            Some((f, Cause::Bios { n, at })) => {
                out.insert(r, format!("swi 0x{n:02x} at 0x{at:08x}, in 0x{f:08x}"));
            }
            _ => {}
        }
    }
    for (&r, &(caller, at)) in &s.read_by {
        if r < 4 {
            out.insert(r + 4, format!("read at 0x{at:08x}, in 0x{caller:08x}"));
        }
    }
    for (&r, &(caller, c)) in &s.passed_by {
        if r > 3 || out.contains_key(&(r + 4)) {
            continue;
        }
        let root = match c {
            Cause::Callee { callee, .. } => sums.root(callee, r),
            _ => Some((caller, c)),
        };
        if let Some((f, Cause::Reads(at) | Cause::Bios { at, .. })) = root {
            out.insert(
                r + 4,
                format!("passed on by 0x{caller:08x}, read at 0x{at:08x}, in 0x{f:08x}"),
            );
        }
    }
    out
}

/// The registers of r0-r3 `k`'s summary proves read at entry: those whose
/// cause, followed through the callees it is passed on to, is an
/// instruction's read or a BIOS call's ([`Summaries::root`]); not an
/// indirect call's, which the summaries take to read all four.
pub fn proven_params(sums: &Summaries, k: u32) -> u8 {
    let Some(s) = sums.get(k) else { return 0 };
    s.causes
        .keys()
        .filter(|&&r| r < 4)
        .filter(|&&r| {
            matches!(
                sums.root(k, r),
                Some((_, Cause::Reads(_) | Cause::Bios { .. }))
            )
        })
        .fold(0, |m, &r| m | 1 << r)
}

/// What `k`'s callers' runs prove read after it (r0-r3): their own
/// instructions' reads (`read_after`), and the registers they pass on to
/// a callee whose chain ends in such a read (`passed_after`'s, by
/// [`Summary::passed_by`]).
pub fn proven_after(sums: &Summaries, k: u32) -> u8 {
    let Some(s) = sums.get(k) else { return 0 };
    let mut m = s.read_after as u8 & 0xf;
    for (&r, &(caller, c)) in &s.passed_by {
        if r > 3 || s.passed_after & 1 << r == 0 {
            continue;
        }
        let root = match c {
            Cause::Callee { callee, .. } => sums.root(callee, r),
            _ => Some((caller, c)),
        };
        if matches!(root, Some((_, Cause::Reads(_) | Cause::Bios { .. }))) {
            m |= 1 << r;
        }
    }
    m
}

/// A clean summary's shape, given what may be read after it returns.
fn shape(s: &Summary, demand: u8) -> Shape {
    if !s.returns {
        return Shape {
            params: s.params,
            results: 0,
        };
    }
    let changed = changed(s);
    let entry = (0..4)
        .filter(|&r| s.exit[r].entry)
        .fold(0u8, |m, r| m | 1 << r);
    // The entry lr left in a register (agbcc's `pop {r0}; bx r0`) is
    // no value a caller reads (`native/lift-frame-gone`).
    let results = (demand & changed | s.results as u8 & 0xf) & !s.lr_left;
    Shape {
        params: s.params | results & entry,
        results,
    }
}

/// The registers of r0-r3 a returning function's summary says it may
/// leave other than they came (bit `n` for rN). Those of them that aren't
/// its results no caller reads after it: a caller's copy is dead there.
pub fn changed(s: &Summary) -> u8 {
    (0..4)
        .filter(|&r| {
            let b = s.exit[r];
            b.def || b.clob || !b.entry
        })
        .fold(0, |m, r| m | 1 << r)
}

/// The summaries of one context's functions, by key.
/// `proto` gives a function's prototype shape and whether it is forced.
pub fn nodes_of<'a>(
    ctx_index: usize,
    funcs: &[Func],
    sums: &'a Summaries,
    edges: impl Fn(&Func) -> Edges,
    standin: impl Fn(u32) -> bool,
    lifted: impl Fn(u32) -> bool,
    addressed: impl Fn(u32) -> bool,
    proto: impl Fn(u32) -> Option<(Shape, bool)>,
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
                    lifted: lifted(f.entry),
                    addressed: addressed(f.entry),
                    proto: proto(f.entry).map(|(p, _)| p),
                    forced: proto(f.entry).is_some_and(|(_, f)| f),
                    proven_params: proven_params(sums, f.entry),
                    proven_after: proven_after(sums, f.entry),
                    proofs: proofs(sums, f.entry),
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
                calls_by_value: BTreeSet::new(),
            },
            standin: false,
            lifted: false,
            addressed: true,
            proto: None,
            forced: false,
            proven_params: 0,
            proven_after: s.read_after as u8 & 0xf,
            proofs: BTreeMap::new(),
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
    fn a_caller_whose_run_was_cut_short_reads_everything_it_changes() {
        let f = sum(0b0001, 0b0001, [DEF, DEF, KEEP, DEF]);
        let mut a = sum(0, 0, [DEF; 4]);
        a.callees.insert(1);
        a.holes.insert("didn't settle".into());
        a.stopped = true;
        a.blind = true;
        let nodes = BTreeMap::from([(1, node(&f, &[], &[])), (2, node(&a, &[1], &[]))]);
        let s = shapes(&nodes);
        assert_eq!(s.shaped[&1].results, 0b1011);
        assert!(!s.shaped.contains_key(&2), "a holey function keeps four");
    }

    /// A stand-in reads all its callee changes; one that is the lift's own
    /// code (fn-check's replacement by the lift or a mod's cut) only what
    /// the lift reads, so a cut's calls keep the shapes it was cut with.
    #[test]
    fn a_lifted_stand_in_reads_its_callees_as_the_lift_does() {
        let f = sum(0b0001, 0b0001, [DEF, DEF, KEEP, DEF]);
        let mut a = sum(0, 0, [DEF; 4]);
        a.callees.insert(1);
        let results = |lifted: bool| {
            let mut n = node(&a, &[1], &[]);
            n.standin = true;
            n.lifted = lifted;
            shapes(&BTreeMap::from([(1, node(&f, &[], &[])), (2, n)])).shaped[&1].results
        };
        assert_eq!(results(false), 0b1011);
        assert_eq!(results(true), 0b0001);
    }

    #[test]
    fn a_caller_whose_run_stopped_reads_what_it_held_at_the_hole() {
        // a calls f, reads r1, writes r0 and r3, and jumps to code it has
        // none of with f's r2 still in r2: f gives back r1 and r2 (r0 is
        // its pointer reach's, not a's).
        let mut f = sum(0b0001, 0b0010, [DEF, DEF, DEF, DEF]);
        f.read_after = 0b0010;
        f.hidden_after = 0b0100;
        let mut a = sum(0, 0, [DEF; 4]);
        a.callees.insert(1);
        a.holes
            .insert("jumps to 0x08000100, which has no code here".into());
        a.stopped = true;
        let mut fnode = node(&f, &[], &[]);
        fnode.addressed = false;
        let nodes = BTreeMap::from([(1, fnode), (2, node(&a, &[1], &[]))]);
        let s = shapes(&nodes);
        assert_eq!(s.shaped[&1].results, 0b0110);
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
    fn a_value_returned_unread_is_read_where_its_returner_is() {
        // c: `bl x; pop {pc}`, returning x's r1; c's caller stopped with
        // c's r1 in r1, so x's demand must take r1 from it.
        let mut x = sum(0, 0, [DEF, DEF, KEEP, KEEP]);
        x.read_after = 0;
        let mut c = sum(0, 0, [DEF, DEF, KEEP, KEEP]);
        c.callees.insert(1);
        c.forwards.insert(1, BTreeSet::from([1]));
        c.hidden_after = 0b0010;
        let mut b = sum(0, 0, [DEF; 4]);
        b.callees.insert(2);
        b.holes
            .insert("jumps to 0x08000100, which has no code here".into());
        b.stopped = true;
        let nodes = BTreeMap::from([
            (1, node(&x, &[], &[])),
            (2, node(&c, &[1], &[])),
            (3, node(&b, &[2], &[])),
        ]);
        let s = shapes(&nodes);
        assert_eq!(s.shaped[&2].results, 0b0011);
        assert_eq!(s.shaped[&1].results, 0b0011);
    }

    fn proto(n: &mut Node, params: u8, results: u8) {
        n.proto = Some(Shape { params, results });
    }

    #[test]
    fn a_prototype_shapes_a_function_with_a_hole() {
        // f(a) -> void, its run stopped: four registers from the
        // summaries, its prototype's shape with one.
        let mut f = sum(0b0001, 0, [DEF; 4]);
        f.holes
            .insert("0x08000010: a jump to an unknown place".into());
        f.stopped = true;
        let mut n = node(&f, &[], &[]);
        proto(&mut n, 0b0001, 0);
        let nodes = BTreeMap::from([(1, n)]);
        let s = shapes(&nodes);
        assert_eq!(
            s.shaped[&1],
            Shape {
                params: 0b0001,
                results: 0
            }
        );
        assert!(s.from_proto.contains(&1));
    }

    #[test]
    fn a_prototype_the_summary_proves_short_is_refused() {
        // The summary proves r1 read; the prototype passes r0 only: the
        // summary's shape stands.
        let f = sum(0b0011, 0b0001, [DEF, KEEP, KEEP, KEEP]);
        let mut n = node(&f, &[], &[]);
        n.proven_params = 0b0011;
        proto(&mut n, 0b0001, 1);
        let nodes = BTreeMap::from([(1, n)]);
        let s = shapes(&nodes);
        assert!(s.refused[&1].contains("reads r1"));
        assert_eq!(s.shaped[&1].params, 0b0011);
        assert!(!s.from_proto.contains(&1));
    }

    #[test]
    fn a_result_a_caller_reads_refuses_a_void_prototype() {
        // A caller's own instruction reads r0 after it: void is wrong.
        let f = sum(0, 0b0001, [DEF, KEEP, KEEP, KEEP]);
        let mut n = node(&f, &[], &[]);
        proto(&mut n, 0, 0);
        let s = shapes(&BTreeMap::from([(1, n)]));
        assert!(s.refused[&1].contains("a caller reads r0"));
    }

    #[test]
    fn a_read_after_a_forwarder_is_a_read_after_its_callee() {
        // c returns x's r0 unread (`return x();`) and its caller reads
        // r0: x's void prototype is refused.
        let x = sum(0, 0, [DEF, KEEP, KEEP, KEEP]);
        let mut c = sum(0, 0b0001, [DEF, KEEP, KEEP, KEEP]);
        c.callees.insert(1);
        c.forwards.insert(0, BTreeSet::from([1]));
        let mut nx = node(&x, &[], &[]);
        nx.proven_after = 0;
        proto(&mut nx, 0, 0);
        let nodes = BTreeMap::from([(1, nx), (2, node(&c, &[1], &[]))]);
        let s = shapes(&nodes);
        assert!(s.refused.contains_key(&1));
    }

    #[test]
    fn a_planted_prototype_is_taken_unchecked() {
        let f = sum(0, 0b0001, [DEF, KEEP, KEEP, KEEP]);
        let mut n = node(&f, &[], &[]);
        proto(&mut n, 0, 0);
        n.forced = true;
        let s = shapes(&BTreeMap::from([(1, n)]));
        assert!(s.refused.is_empty());
        assert_eq!(s.shaped[&1].results, 0);
    }

    #[test]
    fn a_stand_in_keeps_four_registers_whatever_its_prototype() {
        let f = sum(0b0001, 0, [DEF; 4]);
        let mut n = node(&f, &[], &[]);
        proto(&mut n, 0b0001, 0);
        n.standin = true;
        let s = shapes(&BTreeMap::from([(1, n)]));
        assert!(s.shaped.is_empty());
    }

    #[test]
    fn a_function_no_pointer_reaches_gives_only_what_its_callers_read() {
        // f changes r0-r2; its one caller reads r1 after it. Not
        // addressed: no r0 for an indirect caller, nor a tail call by
        // value's demand (t jumps by value to an unknown place, its caller
        // reading r0-r3).
        let f = sum(0b0001, 0b0010, [DEF, DEF, DEF, KEEP]);
        let mut main = sum(0, 0, [DEF; 4]);
        main.callees.insert(1);
        let mut t = sum(0, 0, [DEF; 4]);
        t.read_after = 0xf;
        t.holes
            .insert("0x08000300: a jump to an unknown place".into());
        let mut tn = node(&t, &[], &[]);
        tn.edges.by_value.insert(0x0800_0300);
        let mut fnode = node(&f, &[], &[]);
        fnode.addressed = false;
        let g = sum(0b0001, 0, [DEF, DEF, DEF, KEEP]);
        let nodes = BTreeMap::from([
            (1, fnode),
            (2, node(&main, &[1], &[])),
            (3, tn),
            (4, node(&g, &[], &[])),
        ]);
        let s = shapes(&nodes);
        assert_eq!(s.by_value, 0xf);
        assert_eq!(s.shaped[&1].results, 0b0010);
        // g's address is taken: what the tail call's caller reads.
        assert_eq!(s.shaped[&4].results, 0b0111);
    }

    #[test]
    fn a_jump_by_value_the_jumpers_run_proved_is_no_tail_call() {
        // p (Thumb half: push lr, jump into j) has callers reading r0-r3;
        // j (ARM half: pop lr, jump by value) can't prove its jump alone,
        // but p's run went on through j's code and proved it a return.
        // Only j's own entries (a pointer's: r0) reach the tail call's
        // demand; g, addressed, gives back r0 only.
        let mut j = sum(0b0001, 0, [DEF; 4]);
        j.holes
            .insert("0x08000300: a jump to an unknown place".into());
        let mut p = sum(0b0001, 0, [DEF; 4]);
        p.read_after = 0xf;
        let mut jn = node(&j, &[], &[]);
        jn.edges.by_value.insert(0x0800_0300);
        let g = sum(0b0001, 0, [DEF, DEF, DEF, KEEP]);
        fn mk(p: &Summary) -> Node<'_> {
            let mut pn = node(p, &[], &[1]);
            pn.edges.by_value.insert(0x0800_0200);
            pn
        }
        let nodes = BTreeMap::from([(1, jn), (2, mk(&p)), (4, node(&g, &[], &[]))]);
        let s = shapes(&nodes);
        assert_eq!(s.by_value, 0b0001);
        assert_eq!(s.shaped[&4].results, 0b0001);
        // A jumper whose run didn't go through (j among its callees) or
        // didn't prove the jump leaves it a tail call of p's callers'.
        let mut q = p.clone();
        q.callees.insert(1);
        let mut r = p.clone();
        r.holes
            .insert("0x08000300: a jump to an unknown place".into());
        for x in [&q, &r] {
            let mut jn = node(&j, &[], &[]);
            jn.edges.by_value.insert(0x0800_0300);
            let nodes = BTreeMap::from([(1, jn), (2, mk(x)), (4, node(&g, &[], &[]))]);
            assert_eq!(shapes(&nodes).by_value, 0xf);
        }
    }

    #[test]
    fn a_stand_in_forwarding_to_a_function_is_a_jump_into_it() {
        let code = "pub fn f_030016c5(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {\n    a_08057f7d(g, sp, a)\n}\n";
        assert_eq!(forwarded_to(code), Some(0x0805_7f7d));
        let more = "pub fn f_1(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> { st8(g, 0, 0); a_08057f7d(g, sp, a) }";
        assert_eq!(forwarded_to(more), None);
        let other = "pub fn f_1(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> { a_08057f7d(g, sp, [0, a[1], a[2], a[3]]) }";
        assert_eq!(forwarded_to(other), None);
        // The stand-in (3), copied code with an unknown jump, its caller
        // reading r0-r3: as a jump into f (1), the demand is f's, and no
        // tail call by value's (g, 4, addressed, gives back only r0).
        let f = sum(0b0001, 0b0001, [DEF, DEF, DEF, DEF]);
        let mut t = sum(0, 0, [DEF; 4]);
        t.read_after = 0b0110;
        t.holes
            .insert("0x08000300: a jump to an unknown place".into());
        let mut tn = node(&t, &[], &[1]);
        tn.standin = true;
        let g = sum(0b0001, 0, [DEF, DEF, DEF, KEEP]);
        let nodes = BTreeMap::from([(1, node(&f, &[], &[])), (3, tn), (4, node(&g, &[], &[]))]);
        let s = shapes(&nodes);
        assert_eq!(s.by_value, 0);
        assert_eq!(s.shaped[&1].results, 0b0111);
        assert_eq!(s.shaped[&4].results, 0b0001);
    }

    #[test]
    fn a_function_that_never_returns_gives_nothing() {
        let mut f = sum(0b0011, 0, [DEF; 4]);
        f.returns = false;
        let nodes = BTreeMap::from([(1, node(&f, &[], &[]))]);
        assert_eq!(shapes(&nodes).shaped[&1].results, 0);
    }
}
