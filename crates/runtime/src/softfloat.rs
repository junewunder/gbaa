//! libgcc's soft-float double multiply and divide (`__muldf3`, `__divdf3`
//! in `libgcc/config/arm/ieee754-df.S`), bit-exact on its results: what a
//! native program calls where the game has them
//! (`analysis::softfloat`, which recognises them by their code).
//!
//! A double is its bits, the low word in r0 (r2) and the high word in r1
//! (r3), as the routines take them. Finite operands round to nearest even
//! as IEEE 754 says, which Rust's `f64` arithmetic does on every target,
//! denormals included; the special cases are written out as libgcc does
//! them, since IEEE leaves a NaN's bits open: a NaN result is an operand
//! with its quiet bit (and the exponent's bits) set, payload and sign
//! kept, never a fresh default NaN. `0 * inf` is the infinity quieted,
//! `inf / inf` and `0 / 0` the dividend quieted. Pinned against the
//! interpreter running a game's own copy (`gbaa-softfloat-check ROM`,
//! crates/hole-report).

/// The exponent's bits.
const EXP: u64 = 0x7ff << 52;
/// The sign bit.
const SIGN: u64 = 1 << 63;

/// libgcc's `Lml_n`: `x` with its high word's exponent and quiet bit set.
fn quiet(x: u64) -> u64 {
    x | 0x7ff8 << 48
}

fn is_zero(x: u64) -> bool {
    x << 1 == 0
}

fn is_special(x: u64) -> bool {
    x & EXP == EXP
}

/// A NaN: the exponent all ones and a mantissa.
fn is_nan(x: u64) -> bool {
    is_special(x) && x << 12 != 0
}

/// Infinity (`Lml_i`) or zero (`Lml_z`) with the sign of `a` times `b`.
fn inf(a: u64, b: u64) -> u64 {
    (a ^ b) & SIGN | EXP
}

fn zero(a: u64, b: u64) -> u64 {
    (a ^ b) & SIGN
}

/// `__muldf3`: `a * b`.
pub fn muldf3(a: u64, b: u64) -> u64 {
    if is_special(a) || is_special(b) {
        // Lml_s's infinities and NaNs, in its order.
        return if is_zero(a) {
            quiet(b)
        } else if is_zero(b) || is_nan(a) {
            quiet(a)
        } else if is_nan(b) {
            quiet(b)
        } else {
            inf(a, b)
        };
    }
    if is_zero(a) || is_zero(b) {
        return zero(a, b);
    }
    (f64::from_bits(a) * f64::from_bits(b)).to_bits()
}

/// `__divdf3`: `a / b`.
pub fn divdf3(a: u64, b: u64) -> u64 {
    if is_special(a) && is_special(b) {
        // Ldv_s: inf or NaN over inf or NaN is the dividend quieted.
        return quiet(a);
    }
    if is_special(a) {
        return if is_nan(a) { quiet(a) } else { inf(a, b) };
    }
    if is_special(b) {
        return if is_nan(b) { quiet(b) } else { zero(a, b) };
    }
    match (is_zero(a), is_zero(b)) {
        (true, true) => quiet(a),
        (false, true) => inf(a, b),
        (true, false) => zero(a, b),
        (false, false) => (f64::from_bits(a) / f64::from_bits(b)).to_bits(),
    }
}

#[cfg(test)]
mod tests;
