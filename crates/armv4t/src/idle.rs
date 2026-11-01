//! Idle loops: the busy loops where a native program's time passes
//! (docs/native-port-design.md, section 3). A loop with no stores and no
//! calls can only be waiting for something outside it (a flag a handler
//! sets, the keys, the beam), so at its back edge `--timing native` runs
//! the clock to the next event, and the native lifter prints `hw::idle`.
//!
//! The test is over the bytes alone, so the interpreter (at run time),
//! the code generator and the lifter (`analysis::idle`, at build time)
//! agree on every edge without passing a list between them: a backward
//! branch, without link, whose loop (the straight range from its target
//! to it) is at most [`MAX_LEN`] instructions none of which stores, calls
//! (`bl`, `swi`, anything that writes `lr`) or is undefined, and whose
//! registers carry nothing from one pass to the next (none is read before
//! the pass writes it and also written in it): a loop that walks a
//! pointer or counts (`strlen`, a checksum, a delay) does work, not
//! waiting. Branches out of the range are the loop's exits and allowed.

use crate::decode::{AluOp, Index, Instr, MsrSrc, Offset, Operand2, Reg, decode_arm, decode_thumb};

/// The longest loop, in instructions, the test looks at: an idle poll is a
/// load, a test and the branch, and a few more for a mask or a second flag.
pub const MAX_LEN: u32 = 32;

/// Whether the branch at `at` to `target` (both without the Thumb bit) is
/// an idle loop's back edge. `fetch` reads the instruction word (Thumb:
/// the halfword in its low bits) at an address.
pub fn is_idle_edge(fetch: impl Fn(u32) -> u32, at: u32, target: u32, thumb: bool) -> bool {
    let size = if thumb { 2 } else { 4 };
    if target > at || (at - target) / size >= MAX_LEN {
        return false;
    }
    let decode = |a: u32| {
        if thumb {
            decode_thumb(fetch(a) as u16, a)
        } else {
            decode_arm(fetch(a), a)
        }
    };
    match decode(at) {
        Instr::Branch {
            link: false,
            target: t,
            ..
        } if t == target => {}
        _ => return false,
    }
    let body: [Option<Instr>; MAX_LEN as usize] = core::array::from_fn(|i| {
        let a = target + i as u32 * size;
        (a <= at).then(|| decode(a))
    });
    let body = body.iter().flatten();
    if !body.clone().all(quiet) {
        return false;
    }
    // A register the loop writes and reads before writing carries a value
    // around it. A conditional write may not happen, so it doesn't count
    // as written for what follows.
    let defs = body.clone().fold(0, |d, i| d | regs(i).1);
    let mut written = 0u16;
    for i in body {
        let (uses, d) = regs(i);
        if uses & !written & defs != 0 {
            return false;
        }
        if i.cond() == crate::decode::Cond::Al {
            written |= d;
        }
    }
    true
}

/// The longest spin loop, in instructions, [`is_spin_edge`] looks at.
pub const MAX_SPIN_LEN: u32 = 64;

/// Whether the branch at `at` to `target` is a spin loop's back edge
/// (docs/research/busy-wait-loops.md, "(a′)"): a backward branch without
/// link, not an idle edge, whose straight range is at most
/// [`MAX_SPIN_LEN`] instructions and holds a call (`bl`, or a write of
/// `lr` before a `bx`). Such a loop may be waiting through the functions
/// it calls (Emerald's `DoMapLoadLoop`, `while (!LoadMapInStepsLocal(…));`),
/// so `--timing native` counts its passes and, past a budget with no
/// time passing, lets each pass take half a line.
pub fn is_spin_edge(fetch: impl Fn(u32) -> u32, at: u32, target: u32, thumb: bool) -> bool {
    let size = if thumb { 2 } else { 4 };
    if target > at || (at - target) / size >= MAX_SPIN_LEN {
        return false;
    }
    let decode = |a: u32| {
        if thumb {
            decode_thumb(fetch(a) as u16, a)
        } else {
            decode_arm(fetch(a), a)
        }
    };
    match decode(at) {
        Instr::Branch {
            link: false,
            target: t,
            ..
        } if t == target => {}
        _ => return false,
    }
    // `bl`, or `lr` set from `pc` (`mov lr, pc` / `add lr, pc, #n` before
    // a `bx`): not `lr` used as scratch in a copy loop.
    let calls = (0..=(at - target) / size).any(|i| match decode(target + i * size) {
        Instr::ThumbBlHi { .. } | Instr::ThumbBlLo { .. } | Instr::Branch { link: true, .. } => {
            true
        }
        Instr::Alu {
            rd: Reg::Lr,
            rn,
            op2,
            ..
        } => rn == Reg::Pc || matches!(op2, Operand2::Reg { rm: Reg::Pc, .. }),
        _ => false,
    });
    calls && !is_idle_edge(fetch, at, target, thumb)
}

/// The registers `i` reads and writes, as masks (bit n = rn; pc left out).
fn regs(i: &Instr) -> (u16, u16) {
    let r = |x: Reg| 1u16 << x as u8;
    let op2 = |o: &Operand2| match *o {
        Operand2::Imm { .. } => 0,
        Operand2::Reg { rm, .. } => r(rm),
        Operand2::RegShiftReg { rm, rs, .. } => r(rm) | r(rs),
    };
    let wb = |a: &crate::decode::Addr| match a.index {
        Index::Pre { writeback: true } | Index::Post { .. } => r(a.rn),
        _ => 0,
    };
    let addr = |a: &crate::decode::Addr| {
        r(a.rn)
            | match a.offset {
                Offset::Reg { rm, .. } => r(rm),
                Offset::Imm(_) => 0,
            }
    };
    let (uses, defs) = match i {
        Instr::Alu {
            op, rd, rn, op2: o, ..
        } => {
            let reads_rn = !matches!(op, AluOp::Mov | AluOp::Mvn);
            let writes = !matches!(op, AluOp::Tst | AluOp::Teq | AluOp::Cmp | AluOp::Cmn);
            (
                op2(o) | if reads_rn { r(*rn) } else { 0 },
                if writes { r(*rd) } else { 0 },
            )
        }
        Instr::Mul {
            rd, rm, rs, acc, ..
        } => (r(*rm) | r(*rs) | acc.map_or(0, r), r(*rd)),
        Instr::MulLong {
            rd_lo,
            rd_hi,
            rm,
            rs,
            acc,
            ..
        } => {
            let d = r(*rd_lo) | r(*rd_hi);
            (r(*rm) | r(*rs) | if *acc { d } else { 0 }, d)
        }
        Instr::Load { rd, addr: a, .. } => (addr(a), r(*rd) | wb(a)),
        Instr::Block {
            rn,
            writeback,
            regs,
            ..
        } => (r(*rn), *regs | if *writeback { r(*rn) } else { 0 }),
        Instr::Mrs { rd, .. } => (0, r(*rd)),
        Instr::Msr {
            src: MsrSrc::Reg(x),
            ..
        } => (r(*x), 0),
        Instr::Bx { rm, .. } => (r(*rm), 0),
        _ => (0, 0),
    };
    let pc = !(1u16 << 15);
    (uses & pc, defs & pc)
}

/// Neither a store nor a call.
fn quiet(i: &Instr) -> bool {
    match *i {
        Instr::Store { .. }
        | Instr::Swap { .. }
        | Instr::Swi { .. }
        | Instr::ThumbBlHi { .. }
        | Instr::ThumbBlLo { .. }
        | Instr::Undefined { .. }
        | Instr::Block { load: false, .. }
        | Instr::Branch { link: true, .. } => false,
        // `mov lr, pc` before a `bx`: an indirect call.
        Instr::Alu { rd: Reg::Lr, .. } | Instr::Load { rd: Reg::Lr, .. } => false,
        Instr::Block {
            load: true, regs, ..
        } => regs & 1 << 14 == 0,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thumb halfwords at 0x0800_0000 on.
    fn thumb(code: &[u16]) -> impl Fn(u32) -> u32 + '_ {
        |a| code[(a - 0x0800_0000) as usize / 2] as u32
    }

    #[test]
    fn a_flag_poll_is_idle() {
        // ldrh r1, [r0]; cmp r1, #0; beq 0x0800_0000
        let code = [0x8801, 0x2900, 0xD0FC];
        assert!(is_idle_edge(thumb(&code), 0x0800_0004, 0x0800_0000, true));
    }

    #[test]
    fn a_loop_that_stores_or_calls_is_not() {
        // strh r1, [r0]; cmp r1, #0; beq 0x0800_0000
        let stores = [0x8001, 0x2900, 0xD0FC];
        assert!(!is_idle_edge(
            thumb(&stores),
            0x0800_0004,
            0x0800_0000,
            true
        ));
        // bl (two halves); b 0x0800_0000
        let calls = [0xF000, 0xF800, 0xE7FC];
        assert!(!is_idle_edge(thumb(&calls), 0x0800_0004, 0x0800_0000, true));
    }

    #[test]
    fn a_loop_that_walks_or_counts_is_not() {
        // strlen: ldrb r2, [r1]; cmp r2, #0; beq out; adds r1, #1; b 0x0800_0000
        let walks = [0x780A, 0x2A00, 0xD001, 0x3101, 0xE7FA];
        assert!(!is_idle_edge(thumb(&walks), 0x0800_0008, 0x0800_0000, true));
        // A delay: subs r0, #1; bne 0x0800_0000
        let counts = [0x3801, 0xD1FD];
        assert!(!is_idle_edge(
            thumb(&counts),
            0x0800_0002,
            0x0800_0000,
            true
        ));
    }

    #[test]
    fn a_loop_that_calls_is_a_spin() {
        // movs r0, #0; bl (two halves); cmp r0, #0; beq 0x0800_0000
        let spins = [0x2000, 0xF000, 0xF800, 0x2800, 0xD0FA];
        assert!(is_spin_edge(thumb(&spins), 0x0800_0008, 0x0800_0000, true));
        // A poll is idle, not a spin; a loop with no call is neither.
        let poll = [0x8801, 0x2900, 0xD0FC];
        assert!(!is_spin_edge(thumb(&poll), 0x0800_0004, 0x0800_0000, true));
        let counts = [0x3801, 0xD1FD];
        assert!(!is_spin_edge(
            thumb(&counts),
            0x0800_0002,
            0x0800_0000,
            true
        ));
    }

    #[test]
    fn forward_and_mismatched_branches_are_not() {
        let code = [0x8801, 0x2900, 0xD0FC];
        assert!(!is_idle_edge(thumb(&code), 0x0800_0000, 0x0800_0004, true));
        assert!(!is_idle_edge(thumb(&code), 0x0800_0004, 0x0800_0002, true));
    }
}
