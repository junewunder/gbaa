//! A static cycle cost per instruction (decision 11): what the interpreter
//! ticks after each instruction and what codegen sums over a block at
//! recompile time, so both count time the same way.
//!
//! It follows mGBA's count: the ARM7TDMI's S/N/I cycles from the TRM. A
//! [`Cost`] leaves out what only the running machine knows: the wait states
//! of the code fetches (the region's, from `WAITCNT`), which it counts
//! instead; the data accesses, which the bus counts as they happen (with
//! mGBA's prefetch stall applied after each memory instruction or multiply
//! run from ROM, [`stalls`]); and three things that hang on values
//! (emerald/intro-rayquaza): an instruction whose condition fails costs
//! [`SKIPPED`] instead; a multiply's internal cycles come from its operand
//! ([`mul_wait`]); and an `ldm`/`stm`'s accesses after the first are
//! sequential (the bus's block count).

use crate::decode::{Instr, Operand2, Reg};

/// An instruction's time apart from its data accesses: `base` cycles, plus
/// the wait states of `seq` sequential and `nonseq` nonsequential code
/// fetches from the region it runs in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub base: u32,
    pub seq: u32,
    pub nonseq: u32,
}

impl Cost {
    /// `self` and `o` in a row.
    pub const fn plus(self, o: Cost) -> Cost {
        Cost {
            base: self.base + o.base,
            seq: self.seq + o.seq,
            nonseq: self.nonseq + o.nonseq,
        }
    }
}

/// The cost of `instr`: at least one cycle.
pub const fn cost(instr: &Instr) -> Cost {
    // Fetching the next instruction: 1S.
    const NEXT: Cost = Cost {
        base: 1,
        seq: 1,
        nonseq: 0,
    };
    // After a data access the next fetch is 1N.
    const NEXT_N: Cost = Cost {
        base: 1,
        seq: 0,
        nonseq: 1,
    };
    // Refilling the pipeline after a jump: 1N + 1S more.
    const JUMP: Cost = Cost {
        base: 2,
        seq: 1,
        nonseq: 1,
    };
    const fn i(n: u32) -> Cost {
        Cost {
            base: n,
            seq: 0,
            nonseq: 0,
        }
    }
    const NONE: Cost = i(0);
    match *instr {
        Instr::Alu { rd, op2, op, .. } => {
            let shift_by_reg = matches!(op2, Operand2::RegShiftReg { .. }) as u32;
            let writes_pc = matches!(rd, Reg::Pc) && !op.is_compare();
            NEXT.plus(i(shift_by_reg))
                .plus(if writes_pc { JUMP } else { NONE })
        }
        // The multiplier's cycles hang on its operand ([`mul_wait`]); as in
        // mGBA, the next fetch is N.
        Instr::Mul { .. } | Instr::MulLong { .. } => NEXT_N,
        // 1N data + 1I.
        Instr::Load { rd, .. } => {
            NEXT_N
                .plus(i(1))
                .plus(if matches!(rd, Reg::Pc) { JUMP } else { NONE })
        }
        // 1N data.
        Instr::Store { .. } => NEXT_N,
        Instr::Block { load, regs, .. } => {
            // An empty list transfers r15 alone.
            if load {
                NEXT_N.plus(i(1)).plus(if regs & 0x8000 != 0 || regs == 0 {
                    JUMP
                } else {
                    NONE
                })
            } else {
                NEXT_N
            }
        }
        // 2N data + 1I.
        Instr::Swap { .. } => NEXT_N.plus(i(1)),
        Instr::Mrs { .. } | Instr::Msr { .. } | Instr::ThumbBlHi { .. } => NEXT,
        Instr::Branch { .. }
        | Instr::Bx { .. }
        | Instr::ThumbBlLo { .. }
        | Instr::Swi { .. }
        | Instr::Undefined { .. } => NEXT.plus(JUMP),
    }
}

/// What an instruction whose condition fails costs: one fetch, 1S (mGBA's
/// `ARM_PREFETCH_CYCLES`), whatever it would have cost.
pub const SKIPPED: Cost = Cost {
    base: 1,
    seq: 1,
    nonseq: 0,
};

/// For an instruction that mGBA's prefetch stall follows (one that accesses
/// data memory, or a multiply, whose internal cycles it overlaps too),
/// whether it loads (it has an I cycle, which the stall counts in with the
/// accesses).
pub const fn stalls(instr: &Instr) -> Option<bool> {
    match *instr {
        Instr::Load { .. } | Instr::Swap { .. } => Some(true),
        Instr::Store { .. } | Instr::Mul { .. } | Instr::MulLong { .. } => Some(false),
        Instr::Block { load, .. } => Some(load),
        _ => None,
    }
}

/// For a multiply, how mGBA's `ARM_WAIT_MUL` counts it: whether it checks
/// its operand signed (`smull`/`smlal` and the 32-bit forms; `umull`/`umlal`
/// don't), and the cycles it adds to [`mul_cycles`] (one for an
/// accumulate, one more for the long forms). `None` for any other
/// instruction.
pub const fn mul_form(instr: &Instr) -> Option<(bool, u32)> {
    match *instr {
        Instr::Mul { acc, .. } => Some((true, acc.is_some() as u32)),
        Instr::MulLong { signed, acc, .. } => Some((signed, 1 + acc as u32)),
        _ => None,
    }
}

/// The multiplier's cycles for operand `rs` (`Mul`'s `rs`, before the
/// instruction runs): 1 to 4 by how many of its top bytes are all zeros
/// (or, `signed`, all ones).
pub const fn mul_cycles(rs: u32, signed: bool) -> u32 {
    let mut m = 1;
    let mut mask = 0xFFFF_FF00;
    while m < 4 && !(rs & mask == 0 || (signed && rs & mask == mask)) {
        m += 1;
        mask <<= 8;
    }
    m
}

/// A multiply's internal cycles with `rs` its operand's value:
/// [`mul_cycles`] plus [`mul_form`]'s extra. `None` for any other
/// instruction.
pub const fn mul_wait(instr: &Instr, rs: u32) -> Option<u32> {
    match mul_form(instr) {
        Some((signed, extra)) => Some(mul_cycles(rs, signed) + extra),
        None => None,
    }
}

/// The number of data accesses of an `ldm`/`stm`: one per listed register,
/// and one (r15) for an empty list. `None` for any other instruction.
pub const fn block_accesses(instr: &Instr) -> Option<u32> {
    match *instr {
        Instr::Block { regs, .. } => Some(if regs == 0 { 1 } else { regs.count_ones() }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Cost, cost};
    use crate::decode::{decode_arm, decode_thumb};

    /// With no wait states: cycles, data accesses left out.
    fn plain(c: Cost) -> u32 {
        c.base
    }

    #[test]
    fn costs() {
        let arm = |w| plain(cost(&decode_arm(w, 0x0800_0000)));
        assert_eq!(arm(0xE3A0_0042), 1); // mov r0, #0x42
        assert_eq!(arm(0xE081_0312), 2); // add r0, r1, r2, lsl r3
        assert_eq!(arm(0xE1A0_F00E), 3); // mov pc, lr
        assert_eq!(arm(0xE150_000F), 1); // cmp r0, pc
        assert_eq!(arm(0xE591_0000), 2); // ldr r0, [r1]
        assert_eq!(arm(0xE581_0000), 1); // str r0, [r1]
        assert_eq!(arm(0xE8BD_800F), 4); // pop {r0-r3, pc}
        assert_eq!(arm(0xE92D_000F), 1); // push {r0-r3}
        assert_eq!(arm(0xEAFF_FFFE), 3); // b .
        assert_eq!(arm(0xE000_0291), 1); // mul r0, r1, r2: and its mul_wait
        assert_eq!(arm(0xEF00_0005), 3); // swi 5
        let thumb = |h| plain(cost(&decode_thumb(h, 0x0800_0000)));
        assert_eq!(thumb(0x2042), 1); // movs r0, #0x42
        assert_eq!(thumb(0xF000), 1); // bl, first half
        assert_eq!(thumb(0xF800), 3); // bl, second half
        // b .: 1S, then 1N + 1S to refill.
        let b = cost(&decode_arm(0xEAFF_FFFE, 0x0800_0000));
        assert_eq!((b.seq, b.nonseq), (2, 1));
        // ldr r0, [r1]: the next fetch is N.
        let l = cost(&decode_arm(0xE591_0000, 0x0800_0000));
        assert_eq!((l.seq, l.nonseq), (0, 1));
    }

    #[test]
    fn mul_waits() {
        use super::mul_wait;
        let mul = decode_arm(0xE000_0291, 0x0800_0000); // mul r0, r1, r2
        let mla = decode_arm(0xE020_3291, 0x0800_0000); // mla r0, r1, r2, r3
        let umull = decode_arm(0xE081_0392, 0x0800_0000); // umull r0, r1, r2, r3
        let smlal = decode_arm(0xE0E1_0392, 0x0800_0000); // smlal r0, r1, r2, r3
        assert_eq!(mul_wait(&mul, 0x7F), Some(1));
        assert_eq!(mul_wait(&mul, 0xFFFF_FF80), Some(1));
        assert_eq!(mul_wait(&mul, 0x1234), Some(2));
        assert_eq!(mul_wait(&mul, 0x0012_3456), Some(3));
        assert_eq!(mul_wait(&mul, 0x1234_5678), Some(4));
        assert_eq!(mul_wait(&mla, 0x7F), Some(2));
        assert_eq!(mul_wait(&umull, 0xFFFF_FF80), Some(5));
        assert_eq!(mul_wait(&smlal, 0xFFFF_FF80), Some(3));
        let mov = decode_arm(0xE3A0_0042, 0x0800_0000);
        assert_eq!(mul_wait(&mov, 0), None);
    }
}
