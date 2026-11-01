//! The `u32` meaning of the [`Sem`](super::Sem) primitives that aren't plain
//! Rust operators. The interpreter calls these, and generated code will too
//! (through the runtime), so the two can't disagree.

use super::ShiftType;

/// The ARM ARM's `AddWithCarry`: `(a + b + c, carry out, signed overflow)`.
pub fn add_with_carry(a: u32, b: u32, c: bool) -> (u32, bool, bool) {
    let wide = a as u64 + b as u64 + c as u64;
    let result = wide as u32;
    let carry = wide > u32::MAX as u64;
    let overflow = ((a ^ result) & (b ^ result)) >> 31 != 0;
    (result, carry, overflow)
}

/// A barrel shift by a register-specified amount (its low byte), as the
/// ARM7TDMI does it. Amount 0 leaves the value and carry alone; amounts of
/// 32 or more follow the ARM ARM's table (LSL/LSR give 0, ASR fills with the
/// sign, ROR rotates by the amount mod 32).
pub fn shift(kind: ShiftType, v: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    let n = amount & 0xFF;
    if n == 0 {
        return (v, carry_in);
    }
    let bit = |i: u32| (v >> i) & 1 != 0;
    match kind {
        ShiftType::Lsl => match n {
            1..=31 => (v << n, bit(32 - n)),
            32 => (0, bit(0)),
            _ => (0, false),
        },
        ShiftType::Lsr => match n {
            1..=31 => (v >> n, bit(n - 1)),
            32 => (0, bit(31)),
            _ => (0, false),
        },
        ShiftType::Asr => match n {
            1..=31 => (((v as i32) >> n) as u32, bit(n - 1)),
            _ => (((v as i32) >> 31) as u32, bit(31)),
        },
        ShiftType::Ror => match n & 31 {
            0 => (v, bit(31)),
            r => (v.rotate_right(r), bit(r - 1)),
        },
    }
}

/// Rotate right by one through carry: `(result, carry out)`.
pub fn rrx(v: u32, carry_in: bool) -> (u32, bool) {
    (((carry_in as u32) << 31) | (v >> 1), v & 1 != 0)
}

/// Sign-extend the low `from_bits` bits of `v`.
pub fn sext(v: u32, from_bits: u32) -> u32 {
    debug_assert!((1..=32).contains(&from_bits));
    let s = 32 - from_bits;
    (((v << s) as i32) >> s) as u32
}

/// The 64-bit product as `(low, high)`.
pub fn mul_long(signed: bool, a: u32, b: u32) -> (u32, u32) {
    let p = if signed {
        (a as i32 as i64).wrapping_mul(b as i32 as i64) as u64
    } else {
        a as u64 * b as u64
    };
    (p as u32, (p >> 32) as u32)
}

/// Which multiplier datapath a flag-setting multiply uses, for
/// [`mul_carry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MulKind {
    /// `mul`, `mla` (and Thumb `mul`): signed early termination.
    Short,
    /// `umull`, `umlal`.
    LongUnsigned,
    /// `smull`, `smlal`.
    LongSigned,
}

/// C after a flag-setting multiply on the ARM7TDMI. ARMv4 calls it
/// unpredictable; the ARM7TDMI sets it from the last carry of its Booth
/// multiplier's carry-save adders. `multiplicand` is rm, `multiplier` rs,
/// `acc` the accumulator (rn for `mla`, rd_hi:rd_lo for `umlal`/`smlal`, 0
/// otherwise).
///
/// Ported from zaydlang's reference model of the multiplier
/// (github.com/zaydlang/multiplication-algorithm, `impl.h`), keeping only
/// what the carry depends on. This is an altered version of that software,
/// whose notice follows:
///
/// > Copyright (c) 2024 zaydlang
/// >
/// > This software is provided 'as-is', without any express or implied
/// > warranty. In no event will the authors be held liable for any damages
/// > arising from the use of this software.
/// >
/// > Permission is granted to anyone to use this software for any purpose,
/// > including commercial applications, and to alter it and redistribute it
/// > freely, subject to the following restrictions:
/// >
/// > 1. The origin of this software must not be misrepresented; you must
/// >    not claim that you wrote the original software. If you use this
/// >    software in a product, an acknowledgment in the product
/// >    documentation would be appreciated but is not required.
/// > 2. Altered source versions must be plainly marked as such, and must
/// >    not be misrepresented as being the original software.
/// > 3. This notice may not be removed or altered from any source
/// >    distribution.
pub fn mul_carry(kind: MulKind, multiplicand: u32, multiplier: u32, acc: u64) -> bool {
    const MASK33: u64 = 0x1_FFFF_FFFF;
    let bit = |x: u64, n: u32| (x >> n) & 1;
    let signed = kind != MulKind::LongUnsigned;
    // Inputs are 34-bit values, sign- or zero-extended.
    let extend = |v: u32| {
        if signed {
            (v as i32 as i64 as u64) & 0x3_FFFF_FFFF
        } else {
            v as u64
        }
    };
    let mut multiplier = extend(multiplier);
    let multiplicand = extend(multiplicand);

    let mut sum = acc;
    let mut carry = if multiplier & 1 != 0 {
        !multiplicand
    } else {
        0
    };
    // The accumulator's bits above 33, fed in two at a time.
    let mut acc_shift = acc >> 34;
    // The carry bits shifted out so far, rotated as the hardware does.
    let mut partial_carry = (carry & 1) as u128;
    sum >>= 1;
    carry >>= 1;
    partial_carry = partial_carry.rotate_right(1);

    let mut iterations = 0;
    loop {
        // One cycle: four Booth addends through the carry-save array, each
        // retiring two bits.
        let (mut out_sum, mut out_carry) = (0u64, 0u64);
        for i in 0..4 {
            sum &= MASK33;
            carry &= MASK33;
            let chunk = (multiplier >> (2 * i)) & 7;
            let (addend, inject) = match chunk {
                1 | 2 => (multiplicand, 0),
                3 => (multiplicand.wrapping_mul(2), 0),
                4 => (!multiplicand.wrapping_mul(2), 1),
                5 | 6 => (!multiplicand, 1),
                _ => (0, 0),
            };
            let addend = addend & 0x3_FFFF_FFFF;
            let a = addend & MASK33;
            let mut s = sum ^ a ^ carry;
            let mut c = ((sum & a) | (a & carry) | (carry & sum)) << 1 | inject;
            out_sum |= (s & 3) << (2 * i);
            out_carry |= (c & 3) << (2 * i);
            s >>= 2;
            c >>= 2;
            let magic = bit(acc_shift, 0) + (bit(carry, 32) ^ 1) + (bit(addend, 33) ^ 1);
            s |= magic << 31;
            c |= (bit(acc_shift, 1) ^ 1) << 32;
            acc_shift >>= 2;
            sum = s;
            carry = c;
        }
        sum = out_sum | sum << 8;
        carry = out_carry | carry << 8;

        partial_carry |= (carry & 0xFF) as u128;
        sum >>= 8;
        carry >>= 8;
        partial_carry = partial_carry.rotate_right(8);
        // Arithmetic shift of the 33-bit multiplier.
        multiplier = (((multiplier << 31) as i64 >> 39) as u64) & MASK33;
        iterations += 1;
        let done = multiplier == 0 || (signed && multiplier == MASK33);
        if done {
            break;
        }
    }
    partial_carry |= carry as u128;
    let correction = [23, 15, 7, 31][iterations - 1];
    let hi = (partial_carry.rotate_right(correction) >> 64) as u64;
    let n = if kind == MulKind::Short && iterations == 4 {
        31
    } else {
        63
    };
    bit(hi, n) != 0
}
