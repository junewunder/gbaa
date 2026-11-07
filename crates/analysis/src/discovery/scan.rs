//! The ROM pointer scan (decision 5d): every word-aligned ROM word that
//! points into ROM, outside code and tables found already, is a candidate
//! function start, Thumb if the word is odd and ARM if it is 4-aligned.
//!
//! **Validation** ([`validate`]) decodes forward from the candidate,
//! following fallthrough and direct branches, as Ghidra's
//! `checkValidSubroutine` does (`docs/research/discovery-prior-art.md`). It
//! must reach a return (a pop through sp, `bx lr`, …), an indirect jump
//! after a few instructions (a switch whose cases hold the returns), or,
//! after a `push {…, lr}`, loop forever with every path closed (a task run
//! as a coroutine),
//! without running into a known literal pool or table, out of ROM, or into
//! code found already other than at one of its instruction starts in the
//! same instruction set. A body that only runs into a function start
//! found already validates if code loads the candidate. Undefined instructions, lone halves of a Thumb
//! `bl`, BIOS calls that don't exist, long runs of one word, instructions
//! no game runs (user-bank `ldm`), and a first instruction that is
//! conditional or pops fail it. An ARM body must be mostly unconditional.
//! Then the value-set analysis from the candidate must return with sp
//! where it started: data decoded as code usually pops its caller's frame.
//!
//! **Ranking.** A `push {…, lr}` prologue only ranks candidates: it scores
//! 4, being loaded from a literal pool by code found, sitting next to a
//! known function start in a table or being a constant getter (pool
//! loads, then `bx lr`: [`getter`]) scores 2, and more than one word
//! pointing there scores 1. A candidate needs [`MIN_SCORE`]. A batch takes
//! candidates best first and drops any whose body overlaps one taken
//! already, other than at a shared instruction start.
//!
//! A candidate at an instruction start of code found already (reached by
//! `b`, never as a function) is taken only if code loads its address from
//! a pool and the code before it doesn't run into it: other words pointing
//! into a function's middle are usually unresolved jump tables or data.

use std::collections::{BTreeMap, HashMap, HashSet};

use armv4t::decode::{Cond, Instr, Reg};

use super::View;
use crate::program::{Insn, Program, ROM_BASE};
use crate::vsa::exec::writes_pc;
use crate::vsa::func::{Ctl, classify};

/// The most instructions a candidate's body may have.
pub const BODY_MAX: usize = 16_384;

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Scan batches run.
    pub batches: usize,
    /// Distinct candidate targets, over all batches.
    pub candidates: usize,
    /// Candidates that passed validation, over all batches.
    pub valid: usize,
    /// Candidates taken as function starts.
    pub accepted: usize,
}

/// One candidate start.
#[derive(Clone, Copy, Debug, Default)]
struct Cand {
    /// Words pointing at it.
    refs: u32,
    /// One of them is a literal-pool word that code loads.
    pooled: bool,
    /// One of them is next to a word holding a function start found
    /// already: an entry of a table of function pointers.
    table: bool,
}

/// A candidate's instructions, as (address, size).
type Body = Vec<(u32, u32)>;

/// The lowest score a candidate needs to be taken.
pub const MIN_SCORE: u32 = 2;

/// The candidates to take next, in interworking form; empty when the scan
/// has nothing more. A candidate that failed in an earlier batch is tried
/// again: code found since can change the answer.
pub fn batch(
    prog: &Program,
    view: &View,
    far: &dyn Fn(u32, bool) -> crate::vsa::func::Bl,
    stats: &mut Stats,
) -> Vec<u32> {
    stats.batches += 1;
    let mut cands: BTreeMap<u32, Cand> = BTreeMap::new();
    let len = prog.rom().len() as u32;
    for at in (ROM_BASE..ROM_BASE + (len & !3)).step_by(4) {
        if view.insn_at(at).is_some() || view.insn_at(at + 2).is_some() || view.in_table(at) {
            continue;
        }
        let Some(w) = prog.read(at, 4) else { continue };
        let thumb = w & 1 != 0;
        if !prog.in_rom(w & !1) || (!thumb && w % 4 != 0) || view.roots.contains(&w) {
            continue;
        }
        let c = cands.entry(w).or_default();
        c.refs += 1;
        c.pooled |= view.pools.contains(&at);
        let start_at = |b: u32| prog.read(b, 4).is_some_and(|v| view.funcs.contains(&v));
        c.table |= start_at(at.wrapping_sub(4)) || start_at(at + 4);
    }
    stats.candidates += cands.len();

    // (score, candidate, body)
    let mut ranked: Vec<(u32, u32, Body)> = Vec::new();
    for (&w, c) in &cands {
        let (a, thumb) = (w & !1, w & 1 != 0);
        let push = prog.decode(a, thumb).is_some_and(|i| is_push_lr(&i));
        let score = 4 * push as u32
            + 2 * (c.pooled || c.table || getter(prog, a, thumb)) as u32
            + (c.refs > 1) as u32;
        if score < MIN_SCORE {
            continue;
        }
        let body = match view.insn_at(a) {
            Some((s, _, t)) if s == a && t == thumb && c.pooled && !falls_into(prog, view, a) => {
                vec![]
            }
            Some(_) => continue,
            None => match validate(prog, view, a, thumb) {
                Some((b, own)) if own || c.pooled => b,
                _ => continue,
            },
        };
        if !balanced(prog, a, thumb, far) {
            continue;
        }
        ranked.push((score, w, body));
    }
    stats.valid += ranked.len();
    ranked.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)));

    // Bodies taken so far, by halfword: (instruction set, instruction
    // start). Two bodies may share instructions (a tail branch into the
    // other), but nothing else, and a start inside a body taken already
    // isn't taken.
    let mut taken: HashMap<u32, (bool, bool)> = HashMap::new();
    let mut out = Vec::new();
    let log = std::env::var_os("GBAA_DISC_LOG").is_some();
    for (score, w, body) in ranked {
        let (start, thumb) = (w & !1, w & 1 != 0);
        if taken.contains_key(&start) {
            continue;
        }
        let clash = body.iter().any(|&(a, n)| {
            (a..a + n).step_by(2).any(|h| {
                taken
                    .get(&h)
                    .is_some_and(|&(t, s)| t != thumb || s != (h == a))
            })
        });
        if clash {
            continue;
        }
        for &(a, n) in &body {
            for h in (a..a + n).step_by(2) {
                taken.insert(h, (thumb, h == a));
            }
        }
        if log {
            eprintln!("scan: take 0x{w:08x}, score {score}, body {}", body.len());
        }
        out.push(w);
    }
    stats.accepted += out.len();
    out
}

/// The highest BIOS call number.
const BIOS_MAX: u32 = 0x2a;
/// More identical instructions in a row than this is data.
const REPEAT_MAX: u32 = 16;

/// Whether `i` ends a run of more than [`REPEAT_MAX`] identical words.
fn repeated(prog: &Program, i: &Insn) -> bool {
    let n = if i.thumb { 2 } else { 4 };
    let Some(w) = prog.read(i.addr, n) else {
        return false;
    };
    (1..=REPEAT_MAX).all(|k| prog.read(i.addr.wrapping_sub(k * n), n) == Some(w))
}

/// Instructions games don't run outside the BIOS and exception handlers,
/// or that make no sense: user-bank or SPSR-restoring `ldm`/`stm`, pc
/// written by an ALU op other than `mov`, `add` or `sub`, and writeback
/// to pc.
fn implausible(i: &Instr) -> bool {
    use armv4t::decode::{AluOp, Index};
    match *i {
        Instr::Block {
            psr, rn, writeback, ..
        } => psr || (rn == Reg::Pc && writeback),
        Instr::Alu {
            op, rd: Reg::Pc, ..
        } => !op.is_compare() && !matches!(op, AluOp::Mov | AluOp::Add | AluOp::Sub),
        Instr::Load { addr, .. } | Instr::Store { addr, .. } => {
            addr.rn == Reg::Pc
                && matches!(
                    addr.index,
                    Index::Post { .. } | Index::Pre { writeback: true }
                )
        }
        _ => false,
    }
}

/// Whether code found already runs straight into `a`: the instruction
/// ending there continues to the next one. A function reached before
/// only by `b` starts after an unconditional branch, a return or data.
fn falls_into(prog: &Program, view: &View, a: u32) -> bool {
    let Some((p, n, thumb)) = view.insn_at(a.wrapping_sub(2)) else {
        return false;
    };
    if p + n != a {
        return false;
    }
    let Some(i) = prog.decode(p, thumb) else {
        return false;
    };
    let ends = i.instr.cond() == Cond::Al
        && (matches!(i.instr, Instr::Branch { link: false, .. })
            || (i.bl.is_none() && writes_pc(&i.instr) && classify(prog, &i) != Ctl::Call));
    !ends
}

/// Whether the value-set analysis from `a` returns with sp where it was
/// on entry, wherever it knows sp: a function doesn't pop its caller's
/// frame (the far-jump rule's test, turned around), and data decoded as
/// code usually does. An analysis that hits the step cap fails.
fn balanced(
    prog: &Program,
    a: u32,
    thumb: bool,
    far: &dyn Fn(u32, bool) -> crate::vsa::func::Bl,
) -> bool {
    let r =
        crate::vsa::func::analyze_capped(prog, a, thumb, &mut |t, th| far(t, th), super::STEP_CAP);
    !r.capped && r.ret_sp.iter().all(|o| o.is_none_or(|o| o == 0))
}

/// The most instructions a getter has before its `bx lr`.
const GETTER_MAX: usize = 3;

/// Whether `a` starts a constant getter: up to [`GETTER_MAX`] word loads
/// from a pool after it and moves or shifts of an immediate or of a
/// register set already, into r0 to r3 and setting r0, then `bx lr`.
/// Rust leaves many (`FnOnce::call_once` shims, agb's section-end
/// getters) reached only through a vtable or a static in RAM. One pool
/// load at least: `movs r0, #0; bx lr` ends many a function.
fn getter(prog: &Program, a: u32, thumb: bool) -> bool {
    use armv4t::decode::{AluOp, MemSize, Operand2, Shift};
    let low = |r: Reg| (r as u32) < 4;
    // r0..r3 written so far, as bits.
    let mut set = 0u32;
    let mut at = a;
    let mut pool = u32::MAX;
    for _ in 0..=GETTER_MAX {
        let Some(i) = prog.decode(at, thumb) else {
            return false;
        };
        if i.instr.cond() != Cond::Al {
            return false;
        }
        let rd = match i.instr {
            Instr::Bx { rm: Reg::Lr, .. } => {
                return set & 1 != 0 && pool != u32::MAX && pool >= i.next();
            }
            Instr::Load {
                size: MemSize::Word,
                rd,
                ..
            } => {
                let Some((p, _)) = crate::vsa::func::pool_read(&i) else {
                    return false;
                };
                pool = pool.min(p);
                rd
            }
            Instr::Alu {
                op: AluOp::Mov,
                rd,
                op2,
                ..
            } => {
                let ok = match op2 {
                    Operand2::Imm { .. } => true,
                    Operand2::Reg { rm, shift } => {
                        low(rm) && set & 1 << rm as u32 != 0 && !matches!(shift, Shift::Rrx)
                    }
                    Operand2::RegShiftReg { .. } => false,
                };
                if !ok {
                    return false;
                }
                rd
            }
            _ => return false,
        };
        if !low(rd) {
            return false;
        }
        set |= 1 << rd as u32;
        at = i.next();
    }
    false
}

/// Thumb `push {…, lr}` or ARM `stmdb sp!, {…, lr}`.
fn is_push_lr(i: &Insn) -> bool {
    matches!(
        i.instr,
        Instr::Block { load: false, rn: Reg::Sp, writeback: true, regs, cond: Cond::Al, .. }
            if regs & 1 << 14 != 0
    )
}

/// Decode forward from `start`: the body's instructions as (address,
/// size), if it validates, and whether it ends on its own: a body that
/// reaches no return or jump of its own but runs straight into a function
/// start found already validates too, and the caller decides (modern GCC
/// schedules the `push` a few instructions in, so a stray word pointing
/// at the push becomes a start inside the true function).
///
/// A path that continues after a `bl` is soft: the `bl` may be a far jump
/// or a call that never returns, so what follows may be a literal pool. A
/// conflict on a soft path only ends that path, and soft instructions are
/// left out of the body.
pub fn validate(prog: &Program, view: &View, start: u32, thumb: bool) -> Option<(Body, bool)> {
    // address -> (size, soft)
    let mut seen: BTreeMap<u32, (u32, bool)> = BTreeMap::new();
    let mut work = vec![(start, false)];
    let mut ends = false;
    let mut jumps = false;
    // A hard path ran into a function start found already.
    let mut into = false;
    let mut halves: HashSet<u32> = HashSet::new();
    // Pool words the body's own pc-relative loads read.
    let mut pools: HashSet<u32> = HashSet::new();
    // Every path, soft ones too, ended by coming back to an instruction
    // of the body: no return, jump, bad decode or code found already.
    let mut closed = true;
    while let Some((a, soft)) = work.pop() {
        if let Some(&(_, s)) = seen.get(&a) {
            if s && !soft {
                // Reached on a hard path too: decode it again as hard.
                seen.remove(&a);
            } else {
                continue;
            }
        }
        if seen.len() >= BODY_MAX {
            return None;
        }
        match decode(prog, view, a, thumb, a == start, &halves) {
            // Running into code found already ends this path, but doesn't
            // count as reaching a return.
            Step::Known => {
                into |= !soft && view.funcs.contains(&(a | thumb as u32));
                closed = false;
                continue;
            }
            Step::Bad if soft => {
                closed = false;
                continue;
            }
            Step::Bad => return None,
            Step::Insn(i) => {
                seen.insert(a, (i.size, soft));
                if i.size == 4 && thumb {
                    halves.insert(a + 2);
                }
                if let Some((p, e)) = crate::vsa::func::pool_read(&i) {
                    pools.extend((p & !1..e).step_by(2));
                }
                let next = i.next();
                let cond = i.instr.cond() != Cond::Al;
                if let Some(t) = i.bl {
                    if !prog.in_rom(t) {
                        if !soft {
                            return None;
                        }
                        closed = false;
                    }
                    work.push((next, true));
                } else if let Instr::Branch { target, .. } = i.instr {
                    if !prog.in_rom(target) {
                        if soft {
                            closed = false;
                            continue;
                        }
                        return None;
                    }
                    work.push((target, soft));
                    if cond {
                        work.push((next, soft));
                    }
                } else if writes_pc(&i.instr) {
                    closed = false;
                    // Only a pop through sp counts as returning: `ldm rN,
                    // {…, pc}` is how ARM-decoded text often ends.
                    let pop = !matches!(i.instr, Instr::Block { rn, .. } if rn != Reg::Sp);
                    // An indirect jump ends the body too: a switch whose
                    // cases hold every return.
                    match classify(prog, &i) {
                        Ctl::Return if pop => ends = true,
                        Ctl::Jump if !soft => jumps = true,
                        Ctl::Return | Ctl::Jump => {}
                        Ctl::Call => work.push((next, true)),
                    }
                    if cond {
                        work.push((next, soft));
                    }
                } else {
                    work.push((next, soft));
                }
            }
        }
    }
    let hard: Vec<(u32, u32)> = seen
        .iter()
        .filter(|(_, (_, soft))| !soft)
        .map(|(&a, &(n, _))| (a, n))
        .collect();
    // A body entered mid-instruction, or running through its own pool, is
    // broken.
    let broken = |&(a, n): &(u32, u32)| {
        halves.contains(&a) || (a..a + n).step_by(2).any(|h| pools.contains(&h))
    };
    if hard.iter().any(broken) {
        return None;
    }
    // ARM code is mostly unconditional; ARM-decoded data mostly isn't.
    if !thumb {
        let al = hard
            .iter()
            .filter(|&&(a, _)| {
                prog.decode(a, false)
                    .is_some_and(|i| i.instr.cond() == Cond::Al)
            })
            .count();
        if 2 * al < hard.len() {
            return None;
        }
    }
    // An indirect jump stands in for a return only after a few
    // instructions: data has short runs ending in one. So does a body
    // that never leaves itself, if it opens with `push {…, lr}`: a task
    // run as a coroutine (Kirby: Nightmare in Dream Land's game states,
    // each an endless loop that yields every frame, listed in a table of
    // pointers).
    let endless = closed
        && hard.len() >= JUMP_BODY_MIN
        && prog.decode(start, thumb).is_some_and(|i| is_push_lr(&i));
    let own = ends || (jumps && hard.len() >= JUMP_BODY_MIN) || endless;
    (own || into).then_some((hard, own))
}

/// The fewest instructions a body ending in an indirect jump needs.
const JUMP_BODY_MIN: usize = 4;

enum Step {
    Insn(Insn),
    /// Code found already, at one of its instruction starts.
    Known,
    /// Not code.
    Bad,
}

/// Decode one instruction of a candidate's body at `a`.
fn decode(
    prog: &Program,
    view: &View,
    a: u32,
    thumb: bool,
    first: bool,
    halves: &HashSet<u32>,
) -> Step {
    let known = |a: u32| match view.insn_at(a) {
        Some((s, _, t)) if s == a && t == thumb => Some(true),
        Some(_) => None,
        None if prog.is_data(a) || !prog.in_rom(a) => None,
        None => Some(false),
    };
    match known(a) {
        Some(true) => return Step::Known,
        None => return Step::Bad,
        Some(false) if halves.contains(&a) => return Step::Bad,
        Some(false) => {}
    }
    let Some(i) = prog.decode(a, thumb) else {
        return Step::Bad;
    };
    if i.size == 4 && thumb && known(a + 2) != Some(false) {
        return Step::Bad;
    }
    let bad = match i.instr {
        Instr::Undefined { .. } => true,
        Instr::ThumbBlHi { .. } | Instr::ThumbBlLo { .. } => i.bl.is_none(),
        // The BIOS has calls 0 to 0x2a.
        Instr::Swi { comment, .. } => {
            (if thumb { comment } else { comment >> 16 }) & 0xff > BIOS_MAX
        }
        _ => false,
    };
    // Code starts unconditionally and doesn't start by popping its
    // caller's frame, and data is often a run of one value (Ghidra's
    // repeated-instruction rule).
    let pops = matches!(
        i.instr,
        Instr::Block {
            load: true,
            rn: Reg::Sp,
            ..
        }
    );
    if bad
        || (first && (i.instr.cond() != Cond::Al || pops))
        || repeated(prog, &i)
        || implausible(&i.instr)
    {
        return Step::Bad;
    }
    Step::Insn(i)
}
