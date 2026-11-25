//! Context saves and restores, `setjmp` and `longjmp` (native/lift-longjmp).
//!
//! newlib's `setjmp` stores r4–r12, sp and lr through r0 (`stm r0!,
//! {r4-r12, sp, lr}`) and returns 0; `longjmp` loads them back (`ldm r0!`,
//! the same registers), so its return goes into the frame of the function
//! that called `setjmp`, at that call's return site, with r0 the value
//! passed. Natively that frame's r4–r11 and sp are Rust locals, out of the
//! restore's reach, so the lift does it the Rust way:
//!
//! - The restore's function returns by unwinding: its `ldm` keeps the
//!   buffer's address (`ctx_buf`), and each of its returns after it is the
//!   prelude's `restore(ctx_buf, [r0, r1, r2, r3])`, a panic carrying them.
//! - A function that calls a save directly (`setjmp`'s caller) is a catch
//!   point: it prints in the `match` form inside the prelude's
//!   `catch_restore`; each call of the save keeps the buffer it passes and
//!   its own r4–r11 and sp (what the save stored, natively its locals), and
//!   a restore of that buffer sets them back and goes on at the call's
//!   return site with r0–r3 as the restore returned. Any other unwinding
//!   (another buffer's, a real panic) goes on up.
//!
//! The save itself prints as it is: it stores the zeroes its r4–r11 are
//! natively (`saves::context_saves`: it takes nothing), and what it stores
//! no restore reads but into its own locals.
//!
//! A restore of another shape loads sp and pc from a saved block itself
//! (native/lift-restore-saved-block): `ldr sp, [rb, #o]` and then, ending
//! its block, `ldr pc, [rb, #o2]` or a constant jump into another function.
//! A save to match it stores sp at the same offset (`str sp, [rb, #o]`),
//! with, for a loaded pc, a code address it made (`adr`) at `#o2` beside
//! it; a constant target must be code of the saving function's, or of one
//! it jumps or returns into. Such a restore goes on at a block of the saving
//! function, on the saved sp, with the restorer's registers but the
//! callee-saved ones it never wrote ([`Unwind`]), so the lift does it the
//! Rust way too:
//!
//! - The restore's jump is a [`Plan::Unwind`]: the prelude's
//!   `resume(key, sp, [r0, …, r12], sets)`, a panic carrying the target's
//!   key, the loaded sp and the registers.
//! - A function with such a target among its blocks, or that goes on into
//!   it as another function's entry (by a jump, or a call returning
//!   there), is a resume point: it prints in the `match` form inside
//!   `catch_restore`, and an unwinding whose sp is its own (the frame that
//!   saved it: its sp at the call under way, as stored) and whose key is
//!   one of its targets goes on there, r0–r3 as the restorer had them, and
//!   of r4–r12 those the restorer writes. Any other goes on up.

use super::Code;
use crate::func::{Ctx, Func, Plan, exits, key};
use armv4t::decode::{Addr, AluOp, Cond, Index, Instr, MemSize, Offset, Operand2, Reg};
use std::collections::{BTreeMap, BTreeSet};

/// The program's context saves and restores, as the lift prints them.
#[derive(Clone, Debug, Default)]
pub struct Jumps {
    /// The functions a direct call of which is a save's (`setjmp`'s): those
    /// with a save in them, and those that jump into one (newlib's Thumb
    /// entry, `bx pc`, into its ARM code).
    pub save_fns: BTreeSet<u32>,
    /// The restores a save matches, by address, with their base register.
    pub restores: BTreeMap<u32, u8>,
    /// The catch points: functions that call a save directly, with the
    /// return site of each such call.
    pub catches: BTreeMap<u32, BTreeSet<u32>>,
    /// The jumps of the restores that load sp and pc from a saved block
    /// ([`Unwind`]), by address, with the registers of r4–r12 each
    /// restorer writes (bit `n` for rN): those it passes on.
    pub unwinds: BTreeMap<u32, u16>,
    /// Their sp loads: no stack switch.
    pub sp_loads: BTreeSet<u32>,
    /// The resume points: functions with such a restore's target among
    /// their blocks, or that jump or return into it as another function's
    /// entry, with those targets. Not a catch point's: a function that is
    /// both takes `setjmp`'s restores only.
    pub resumes: BTreeMap<u32, BTreeSet<u32>>,
}

impl Jumps {
    /// The return sites of `f`'s calls of a save, if it is a catch point.
    pub fn catch(&self, f: u32) -> Option<&BTreeSet<u32>> {
        self.catches.get(&f)
    }

    /// Whether `f` has a restore in it.
    pub fn restores_in(&self, f: &crate::func::Func) -> bool {
        f.blocks
            .values()
            .flat_map(|b| &b.insns)
            .any(|(a, _)| self.restores.contains_key(a))
    }
}

/// The context saves and restores of `codes` ([`Jumps`]), and the
/// restores that load sp and pc from a saved block, `unwinds`
/// ([`find_unwinds`]).
pub(super) fn find(codes: &[Code], unwinds: &[Unwind]) -> Jumps {
    let mut j = find_setjmp(codes);
    for u in unwinds {
        j.unwinds.insert(u.at, u.sets);
        j.sp_loads.insert(u.sp_at);
        let Some(c) = codes.get(u.saver_ctx) else {
            continue;
        };
        for &l in &u.labels {
            // Those with the target among their blocks, and those that go
            // on into its function, a jump or a call's return into its
            // entry (a loop's first pass, before it jumps into the copy
            // of itself that the restore's jump made a function).
            let into = |f: &Func| {
                !f.blocks.contains_key(&l)
                    && f.blocks
                        .values()
                        .any(|gb| exits(&c.ctx, gb).iter().any(|x| goes_on(x).any(|k| k == l)))
            };
            for f in c
                .funcs
                .iter()
                .filter(|f| f.blocks.contains_key(&l) || into(f))
            {
                if !j.catches.contains_key(&f.entry) {
                    j.resumes.entry(f.entry).or_default().insert(l);
                }
            }
        }
    }
    j
}

/// `setjmp`'s and `longjmp`'s ([`Jumps`]).
fn find_setjmp(codes: &[Code]) -> Jumps {
    let (saves, restores) = crate::saves::context_pairs(codes.iter().flat_map(|c| &c.funcs));
    if saves.is_empty() {
        return Jumps::default();
    }
    let mut save_fns: BTreeSet<u32> = codes
        .iter()
        .flat_map(|c| &c.funcs)
        .filter(|f| {
            f.blocks
                .values()
                .flat_map(|b| &b.insns)
                .any(|(a, _)| saves.contains(a))
        })
        .map(|f| f.entry)
        .collect();
    // And those that jump into one, but call nothing on the way: a
    // function's own code that doesn't return before the save.
    loop {
        let before = save_fns.len();
        for c in codes {
            for f in &c.funcs {
                if save_fns.contains(&f.entry) || !f.calls.is_empty() {
                    continue;
                }
                let into = f.blocks.values().any(|gb| {
                    exits(&c.ctx, gb).iter().any(|x| match x.plan {
                        Plan::Static(k) | Plan::Rekey(k) => {
                            !f.blocks.contains_key(&k) && save_fns.contains(&k)
                        }
                        Plan::Dynamic {
                            ret: None,
                            target: Some(k),
                            ..
                        } => !f.blocks.contains_key(&k) && save_fns.contains(&k),
                        _ => false,
                    })
                });
                if into {
                    save_fns.insert(f.entry);
                }
            }
        }
        if save_fns.len() == before {
            break;
        }
    }
    let mut catches: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for c in codes {
        for f in &c.funcs {
            for gb in f.blocks.values() {
                for x in exits(&c.ctx, gb) {
                    if let Plan::Call { target, ret } = x.plan
                        && save_fns.contains(&target)
                    {
                        catches.entry(f.entry).or_default().insert(ret);
                    }
                }
            }
        }
    }
    Jumps {
        save_fns,
        restores,
        catches,
        ..Jumps::default()
    }
}

/// A context restore that loads sp and pc from a saved block (the module's
/// doc): its jump is a [`Plan::Unwind`].
#[derive(Clone, Debug)]
pub struct Unwind {
    /// Its code context (an index of the lift's).
    pub ctx: usize,
    /// Its `ldr sp`.
    pub sp_at: u32,
    /// Its jump.
    pub at: u32,
    /// The code context of the saves it matches, and the keys it may go
    /// on at there.
    pub saver_ctx: usize,
    pub labels: BTreeSet<u32>,
    /// The registers of r4–r12 its function writes (bit `n` for rN): what
    /// it passes on, as the guest's are its own there; the others are the
    /// resumed function's, as natively.
    pub sets: u16,
}

/// A plain word access `[rn, #o]` of `rd`, unconditional, no writeback:
/// `(rd, rn, o)`.
fn word_at(i: &Instr, load: bool) -> Option<(Reg, Reg, i32)> {
    let (rd, addr) = match *i {
        Instr::Load {
            cond: Cond::Al,
            size: MemSize::Word,
            rd,
            addr,
        } if load => (rd, addr),
        Instr::Store {
            cond: Cond::Al,
            size: MemSize::Word,
            rd,
            addr,
        } if !load => (rd, addr),
        _ => return None,
    };
    match addr {
        Addr {
            rn,
            offset: Offset::Imm(o),
            index: Index::Pre { writeback: false },
        } if rn != Reg::Sp && rn != Reg::Pc => Some((rd, rn, o)),
        _ => None,
    }
}

/// Whether `i` may write sp.
fn writes_sp(i: &Instr) -> bool {
    match *i {
        Instr::Load { rd, addr, .. } => rd == Reg::Sp || addr.rn == Reg::Sp,
        Instr::Store { addr, .. } => addr.rn == Reg::Sp,
        Instr::Alu { rd, .. } | Instr::Mul { rd, .. } => rd == Reg::Sp,
        Instr::Block {
            rn,
            regs,
            load,
            writeback,
            ..
        } => (load && regs & 1 << 13 != 0) || (writeback && rn == Reg::Sp),
        Instr::MulLong { rd_lo, rd_hi, .. } => rd_lo == Reg::Sp || rd_hi == Reg::Sp,
        Instr::Swap { rd, .. } | Instr::Mrs { rd, .. } => rd == Reg::Sp,
        Instr::Msr { .. } => true,
        _ => false,
    }
}

/// The code address `adr rX` made at `at` (`add rX, pc, #n`, or `sub`).
fn adr(at: u32, i: &Instr, thumb: bool) -> Option<(Reg, u32)> {
    let Instr::Alu {
        cond: Cond::Al,
        op,
        s: false,
        rd,
        rn: Reg::Pc,
        op2: Operand2::Imm { value, .. },
    } = *i
    else {
        return None;
    };
    let pc = if thumb { (at + 4) & !3 } else { at + 8 };
    match op {
        AluOp::Add => Some((rd, pc.wrapping_add(value))),
        AluOp::Sub => Some((rd, pc.wrapping_sub(value))),
        _ => None,
    }
}

/// How a restore's block ends ([`find_unwinds`]).
enum Restore {
    /// `ldr pc, [rb, #o2]`.
    Loaded(i32),
    /// A constant jump, to this key.
    To(u32),
}

/// The restores of `codes` (each context with its functions) that load sp
/// and pc from a saved block a save matches ([`Unwind`]).
pub(super) fn find_unwinds(codes: &[(&Ctx, &[Func])]) -> Vec<Unwind> {
    // The saves of sp: (context, function, offset), and those with a code
    // address beside them: (context, offset, its offset, the key).
    let mut sp_saves: Vec<(usize, u32, i32)> = Vec::new();
    let mut pc_saves: Vec<(usize, i32, i32, u32)> = Vec::new();
    for (ci, (_, funcs)) in codes.iter().enumerate() {
        for f in funcs.iter() {
            for gb in f.blocks.values() {
                let thumb = gb.block.thumb;
                for &(_, i) in &gb.insns {
                    let Some((Reg::Sp, rb, o)) = word_at(&i, false) else {
                        continue;
                    };
                    sp_saves.push((ci, f.entry, o));
                    for (m, &(_, j)) in gb.insns.iter().enumerate() {
                        let Some((rx, rb2, o2)) = word_at(&j, false) else {
                            continue;
                        };
                        if rb2 != rb || rx == Reg::Sp || rx == Reg::Pc || o2 == o {
                            continue;
                        }
                        // The last write of rX before the store, an `adr`.
                        let made = gb.insns[..m]
                            .iter()
                            .rev()
                            .find(|(_, x)| writes(x, rx))
                            .and_then(|&(a, x)| adr(a, &x, thumb))
                            .filter(|&(r, _)| r == rx);
                        if let Some((_, l)) = made {
                            pc_saves.push((ci, o, o2, key(l & !3, false)));
                        }
                    }
                }
            }
        }
    }
    if sp_saves.is_empty() {
        return Vec::new();
    }
    // The keys each function's code may go on at: its blocks, and those of
    // the functions it jumps or a call of its returns into (not those it
    // calls), as far as they go.
    let reach = |ci: usize, entry: u32| -> BTreeSet<u32> {
        let (ctx, funcs) = codes[ci];
        let by: BTreeMap<u32, &Func> = funcs.iter().map(|f| (f.entry, f)).collect();
        let mut seen: BTreeSet<u32> = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut todo = vec![entry];
        while let Some(e) = todo.pop() {
            if !seen.insert(e) || seen.len() > 64 {
                continue;
            }
            let Some(f) = by.get(&e) else { continue };
            keys.extend(f.blocks.keys().copied());
            for gb in f.blocks.values() {
                for x in exits(ctx, gb) {
                    todo.extend(goes_on(&x).filter(|k| !f.blocks.contains_key(k)));
                }
            }
        }
        keys
    };
    let mut out = Vec::new();
    for (ci, (ctx, funcs)) in codes.iter().enumerate() {
        for f in funcs.iter() {
            for gb in f.blocks.values() {
                let Some(n) = gb
                    .insns
                    .iter()
                    .position(|(_, i)| matches!(word_at(i, true), Some((Reg::Sp, ..))))
                else {
                    continue;
                };
                let (sp_at, i) = gb.insns[n];
                let Some((_, rb, o)) = word_at(&i, true) else {
                    continue;
                };
                let Some(&(at, last)) = gb.insns.last() else {
                    continue;
                };
                if gb.insns[n + 1..gb.insns.len() - 1]
                    .iter()
                    .any(|(_, x)| writes_sp(x))
                    || at == sp_at
                {
                    continue;
                }
                let Some(x) = exits(ctx, gb).into_iter().find(|x| x.at == at) else {
                    continue;
                };
                let how = match (word_at(&last, true), x.plan) {
                    (Some((Reg::Pc, rb2, o2)), _) if rb2 == rb => Restore::Loaded(o2),
                    // (The target may be among its own blocks till the
                    // jump is an unwind: a block no other function's
                    // entry starts, copied.)
                    (_, Plan::Static(k) | Plan::Rekey(k)) if last.cond() == Cond::Al => {
                        Restore::To(k)
                    }
                    _ => continue,
                };
                // The saves it matches, in one context (the first).
                let mut found: Option<(usize, BTreeSet<u32>)> = None;
                match how {
                    Restore::Loaded(o2) => {
                        for &(sc, so, so2, l) in &pc_saves {
                            if (so, so2) == (o, o2) && found.as_ref().is_none_or(|x| x.0 == sc) {
                                found.get_or_insert((sc, BTreeSet::new())).1.insert(l);
                            }
                        }
                    }
                    Restore::To(k) => {
                        for &(sc, se, so) in &sp_saves {
                            if so == o && found.is_none() && reach(sc, se).contains(&k) {
                                found = Some((sc, [k].into()));
                            }
                        }
                    }
                }
                let Some((saver_ctx, labels)) = found else {
                    continue;
                };
                out.push(Unwind {
                    ctx: ci,
                    sp_at,
                    at,
                    saver_ctx,
                    labels,
                    sets: writes_high(f),
                });
            }
        }
    }
    out
}

/// The registers of r4–r12 `f` writes (bit `n` for rN).
fn writes_high(f: &Func) -> u16 {
    let mut sets = 0u16;
    for b in f.blocks.values() {
        for inst in &b.block.insts {
            if let analysis::ir::Op::SetReg(r, _) = inst.op
                && (4..=12).contains(&r.index())
            {
                sets |= 1 << r.index();
            }
        }
    }
    sets
}

/// Each of `unwinds`' [`Unwind::sets`] again, from `codes` built with
/// their jumps as unwinds: a restorer's function then has no copy of the
/// code its jump went on into.
pub(super) fn rewrite_sets(unwinds: &mut [Unwind], codes: &[(&Ctx, &[Func])]) {
    for u in unwinds {
        let Some((_, funcs)) = codes.get(u.ctx) else {
            continue;
        };
        u.sets = funcs
            .iter()
            .filter(|f| {
                f.blocks
                    .values()
                    .any(|b| b.insns.iter().any(|&(a, _)| a == u.at))
            })
            .fold(0, |m, f| m | writes_high(f));
    }
}

/// Where a function goes on after exit `x` but by its own returns: a
/// jump's target, a call's return site.
fn goes_on(x: &crate::func::ExitInfo) -> impl Iterator<Item = u32> {
    let jump = match x.plan {
        Plan::Static(k) | Plan::Rekey(k) => Some(k),
        _ => None,
    };
    jump.into_iter().chain(x.ret)
}

/// Whether `i` may write `r` (a data register).
fn writes(i: &Instr, r: Reg) -> bool {
    match *i {
        Instr::Load { rd, addr, .. } => {
            rd == r || (addr.rn == r && !matches!(addr.index, Index::Pre { writeback: false }))
        }
        Instr::Store { addr, .. } => {
            addr.rn == r && !matches!(addr.index, Index::Pre { writeback: false })
        }
        Instr::Alu { op, rd, .. } => {
            rd == r && !matches!(op, AluOp::Tst | AluOp::Teq | AluOp::Cmp | AluOp::Cmn)
        }
        Instr::Mul { rd, .. } | Instr::Swap { rd, .. } | Instr::Mrs { rd, .. } => rd == r,
        Instr::MulLong { rd_lo, rd_hi, .. } => rd_lo == r || rd_hi == r,
        Instr::Block {
            rn,
            regs,
            load,
            writeback,
            ..
        } => (load && regs & 1 << r as u16 != 0) || (writeback && rn == r),
        Instr::Branch { link: true, .. } | Instr::ThumbBlHi { .. } | Instr::ThumbBlLo { .. } => {
            r == Reg::Lr
        }
        _ => false,
    }
}
