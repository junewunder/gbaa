//! The arithmetic calls: `Div`, `Sqrt`, `ArcTan`, `ArcTan2`, the affine
//! matrix sets and `MidiKey2Freq`. All but the last are integer, as in the
//! real BIOS, so results match it bit for bit rather than through floats.
//! `MidiKey2Freq` follows mGBA's float formula instead, with its `exp2f`
//! ported from glibc's so the rounding is the same.

use crate::bus::Bus;

/// `Div`/`DivArm`: r0 = quotient, r1 = remainder, r3 = |quotient|, both
/// rounded toward zero (GBATEK). The real BIOS hangs dividing by zero
/// unless the numerator is -1, 0 or 1; like mGBA we return its results for
/// those (r0 = ±1, r1 = the numerator, r3 = 1) rather than hang.
/// `i32::MIN / -1` gives mGBA's `i32::MIN`, 0, `i32::MIN`.
pub fn div(r: &mut [u32; 15], num: i32, den: i32) {
    let (q, rem, abs) = quot_rem(num, den);
    r[0] = q as u32;
    r[1] = rem as u32;
    r[3] = abs;
}

/// `Div` over values: (quotient, remainder, |quotient|), as [`div`]
/// leaves them in r0, r1 and r3 (a native program's `bios::div`).
pub fn quot_rem(num: i32, den: i32) -> (i32, i32, u32) {
    let (q, rem) = if den == 0 {
        (if num < 0 { -1 } else { 1 }, num)
    } else {
        (num.wrapping_div(den), num.wrapping_rem(den))
    };
    (q, rem, if den == 0 { 1 } else { q.unsigned_abs() })
}

/// `Sqrt`: the integer square root of r0, unsigned, rounded down. Only r0
/// changes (mGBA).
pub fn sqrt(x: u32) -> u32 {
    // Digit by digit, two bits at a time.
    let (mut x, mut root) = (x, 0u32);
    let mut bit = 1u32 << 30;
    while bit > x {
        bit >>= 2;
    }
    while bit != 0 {
        if x >= root + bit {
            x -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

/// The real BIOS's `ArcTan` polynomial, as disassembled in the
/// Cult-of-GBA BIOS: `tan` in 1.14 fixed point in, the angle (0x4000 =
/// π/2) out, plus the r1 and r3 it leaves behind.
fn arc_tan_raw(i: i32) -> (i32, i32, i32) {
    let a = -(i.wrapping_mul(i) >> 14);
    let mut b = (0xA9i32.wrapping_mul(a) >> 14) + 0x390;
    for k in [0x91C, 0xFB6, 0x16AA, 0x2081, 0x3651, 0xA2F9] {
        b = (b.wrapping_mul(a) >> 14) + k;
    }
    (i.wrapping_mul(b) >> 16, a, b)
}

/// `ArcTan`: r0 = atan(r0), r1 and r3 the polynomial's intermediates.
pub fn arc_tan(r: &mut [u32; 15]) {
    let (angle, a, b) = arc_tan_raw(r[0] as i32);
    r[0] = angle as u32;
    r[1] = a as u32;
    r[3] = b as u32;
}

/// `ArcTan2` (r0 = x, r1 = y): the angle of (x, y) in 0..0xFFFF for 0..2π,
/// reduced to an octant so `ArcTan` sees |tan| <= 1, as mGBA does. r1
/// keeps `ArcTan`'s intermediate when it ran; r3 = 0x170 (GBATEK).
pub fn arc_tan2(r: &mut [u32; 15]) {
    let (angle, r1) = arc_tan2_of(r[0] as i32, r[1] as i32);
    if let Some(a) = r1 {
        r[1] = a as u32;
    }
    r[0] = angle as u32;
    r[3] = 0x170;
}

/// `ArcTan2` over values: the angle of (x, y), and the r1 `ArcTan` left
/// if it ran (a native program's `bios::arctan2` takes the angle).
pub fn arc_tan2_of(x: i32, y: i32) -> (u16, Option<i32>) {
    let mut r1 = None;
    let mut atan = |t: i32| {
        let (angle, a, _) = arc_tan_raw(t);
        r1 = Some(a);
        angle
    };
    let yx = || y.wrapping_shl(14).wrapping_div(x);
    let xy = || x.wrapping_shl(14).wrapping_div(y);
    let angle = if y == 0 {
        if x >= 0 { 0 } else { 0x8000 }
    } else if x == 0 {
        if y >= 0 { 0x4000 } else { 0xC000 }
    } else if y >= 0 {
        if x >= 0 && x >= y {
            atan(yx())
        } else if x < 0 && x.wrapping_neg() >= y {
            atan(yx()) + 0x8000
        } else {
            0x4000 - atan(xy())
        }
    } else if x <= 0 && x.wrapping_neg() > y.wrapping_neg() {
        atan(yx()) + 0x8000
    } else if x > 0 && x >= y.wrapping_neg() {
        atan(yx()) + 0x10000
    } else {
        0xC000 - atan(xy())
    };
    (angle as u16, r1)
}

/// The real BIOS's sine table: 256 steps around the circle, 1.14 fixed
/// point (as transcribed in the Cult-of-GBA BIOS).
#[rustfmt::skip]
const SIN: [u16; 256] = [
    0x0000, 0x0192, 0x0323, 0x04B5, 0x0645, 0x07D5, 0x0964, 0x0AF1,
    0x0C7C, 0x0E05, 0x0F8C, 0x1111, 0x1294, 0x1413, 0x158F, 0x1708,
    0x187D, 0x19EF, 0x1B5D, 0x1CC6, 0x1E2B, 0x1F8B, 0x20E7, 0x223D,
    0x238E, 0x24DA, 0x261F, 0x275F, 0x2899, 0x29CD, 0x2AFA, 0x2C21,
    0x2D41, 0x2E5A, 0x2F6B, 0x3076, 0x3179, 0x3274, 0x3367, 0x3453,
    0x3536, 0x3612, 0x36E5, 0x37AF, 0x3871, 0x392A, 0x39DA, 0x3A82,
    0x3B20, 0x3BB6, 0x3C42, 0x3CC5, 0x3D3E, 0x3DAE, 0x3E14, 0x3E71,
    0x3EC5, 0x3F0E, 0x3F4E, 0x3F84, 0x3FB1, 0x3FD3, 0x3FEC, 0x3FFB,
    0x4000, 0x3FFB, 0x3FEC, 0x3FD3, 0x3FB1, 0x3F84, 0x3F4E, 0x3F0E,
    0x3EC5, 0x3E71, 0x3E14, 0x3DAE, 0x3D3E, 0x3CC5, 0x3C42, 0x3BB6,
    0x3B20, 0x3A82, 0x39DA, 0x392A, 0x3871, 0x37AF, 0x36E5, 0x3612,
    0x3536, 0x3453, 0x3367, 0x3274, 0x3179, 0x3076, 0x2F6B, 0x2E5A,
    0x2D41, 0x2C21, 0x2AFA, 0x29CD, 0x2899, 0x275F, 0x261F, 0x24DA,
    0x238E, 0x223D, 0x20E7, 0x1F8B, 0x1E2B, 0x1CC6, 0x1B5D, 0x19EF,
    0x187D, 0x1708, 0x158F, 0x1413, 0x1294, 0x1111, 0x0F8C, 0x0E05,
    0x0C7C, 0x0AF1, 0x0964, 0x07D5, 0x0645, 0x04B5, 0x0323, 0x0192,
    0x0000, 0xFE6E, 0xFCDD, 0xFB4B, 0xF9BB, 0xF82B, 0xF69C, 0xF50F,
    0xF384, 0xF1FB, 0xF074, 0xEEEF, 0xED6C, 0xEBED, 0xEA71, 0xE8F8,
    0xE783, 0xE611, 0xE4A3, 0xE33A, 0xE1D5, 0xE075, 0xDF19, 0xDDC3,
    0xDC72, 0xDB26, 0xD9E1, 0xD8A1, 0xD767, 0xD633, 0xD506, 0xD3DF,
    0xD2BF, 0xD1A6, 0xD095, 0xCF8A, 0xCE87, 0xCD8C, 0xCC99, 0xCBAD,
    0xCACA, 0xC9EE, 0xC91B, 0xC851, 0xC78F, 0xC6D6, 0xC626, 0xC57E,
    0xC4E0, 0xC44A, 0xC3BE, 0xC33B, 0xC2C2, 0xC252, 0xC1EC, 0xC18F,
    0xC13B, 0xC0F2, 0xC0B2, 0xC07C, 0xC04F, 0xC02D, 0xC014, 0xC005,
    0xC000, 0xC005, 0xC014, 0xC02D, 0xC04F, 0xC07C, 0xC0B2, 0xC0F2,
    0xC13B, 0xC18F, 0xC1EC, 0xC252, 0xC2C2, 0xC33B, 0xC3BE, 0xC44A,
    0xC4E0, 0xC57E, 0xC626, 0xC6D6, 0xC78F, 0xC851, 0xC91B, 0xC9EE,
    0xCACA, 0xCBAD, 0xCC99, 0xCD8C, 0xCE87, 0xCF8A, 0xD095, 0xD1A6,
    0xD2BF, 0xD3DF, 0xD506, 0xD633, 0xD767, 0xD8A1, 0xD9E1, 0xDB26,
    0xDC72, 0xDDC3, 0xDF19, 0xE075, 0xE1D5, 0xE33A, 0xE4A3, 0xE611,
    0xE783, 0xE8F8, 0xEA71, 0xEBED, 0xED6C, 0xEEEF, 0xF074, 0xF1FB,
    0xF384, 0xF50F, 0xF69C, 0xF82B, 0xF9BB, 0xFB4B, 0xFCDD, 0xFE6E,
];

/// The matrix for scales `sx`, `sy` (8.8) and `angle` (only its top byte
/// counts): `(pa, pb, pc, pd)`, with pb negated after the shift, as the
/// real BIOS does.
fn matrix(sx: i32, sy: i32, angle: u16) -> (i32, i32, i32, i32) {
    let t = (angle >> 8) as usize;
    let sin = SIN[t] as i16 as i32;
    let cos = SIN[(t + 64) & 0xFF] as i16 as i32;
    (
        sx.wrapping_mul(cos) >> 14,
        -(sx.wrapping_mul(sin) >> 14),
        sy.wrapping_mul(sin) >> 14,
        sy.wrapping_mul(cos) >> 14,
    )
}

/// One `BgAffineSet` entry over values: texture centre (`ox`, `oy`,
/// 24.8), screen centre (`cx`, `cy`), scales (8.8) and angle, to `PA`-`PD`
/// and the start point (`BGxX`, `BGxY`) as the call writes them.
#[allow(clippy::too_many_arguments)]
pub fn bg_affine(
    ox: i32,
    oy: i32,
    cx: i32,
    cy: i32,
    sx: i32,
    sy: i32,
    angle: u16,
) -> ([i16; 4], i32, i32) {
    let (pa, pb, pc, pd) = matrix(sx, sy, angle);
    let x = ox
        .wrapping_sub(cx.wrapping_mul(pa))
        .wrapping_sub(cy.wrapping_mul(pb));
    let y = oy
        .wrapping_sub(cx.wrapping_mul(pc))
        .wrapping_sub(cy.wrapping_mul(pd));
    ([pa as i16, pb as i16, pc as i16, pd as i16], x, y)
}

/// One `ObjAffineSet` entry over values: scales (8.8) and angle to
/// `PA`-`PD`.
pub fn obj_affine(sx: i32, sy: i32, angle: u16) -> [i16; 4] {
    let (pa, pb, pc, pd) = matrix(sx, sy, angle);
    [pa as i16, pb as i16, pc as i16, pd as i16]
}

/// What the affine sets' `subs r2, #1; blt` loop leaves in r2: -1, or one
/// less than a negative count.
fn end_count(count: u32) -> u32 {
    (count as i32).min(0).wrapping_sub(1) as u32
}

/// `BgAffineSet` (r0: 20-byte sources, r1: 16-byte destinations, r2:
/// count). Each source is the texture centre (two 24.8 words), the screen
/// centre (two halfwords), the scales (two 8.8 halfwords) and the angle;
/// each destination is pa-pd and the start point. Registers end as in the
/// Cult-of-GBA BIOS: r0 and r1 past the last entry, r2 = -1 (see
/// [`end_count`]).
pub fn bg_affine_set(bus: &mut Bus, r: &mut [u32; 15]) {
    let (mut src, mut dst) = (r[0], r[1]);
    for _ in 0..(r[2] as i32).max(0) {
        let ox = bus.read32(src) as i32;
        let oy = bus.read32(src.wrapping_add(4)) as i32;
        let cx = bus.read16(src.wrapping_add(8)) as i16 as i32;
        let cy = bus.read16(src.wrapping_add(10)) as i16 as i32;
        let sx = bus.read16(src.wrapping_add(12)) as i16 as i32;
        let sy = bus.read16(src.wrapping_add(14)) as i16 as i32;
        let ([pa, pb, pc, pd], x, y) =
            bg_affine(ox, oy, cx, cy, sx, sy, bus.read16(src.wrapping_add(16)));
        for (k, v) in [pa, pb, pc, pd].into_iter().enumerate() {
            bus.write16(dst.wrapping_add(2 * k as u32), v as u16);
        }
        bus.write32(dst.wrapping_add(8), x as u32);
        bus.write32(dst.wrapping_add(12), y as u32);
        src = src.wrapping_add(20);
        dst = dst.wrapping_add(16);
    }
    r[0] = src;
    r[1] = dst;
    r[2] = end_count(r[2]);
}

/// `ObjAffineSet` (r0: 8-byte sources, r1: destination, r2: count, r3: the
/// stride between pa, pb, pc and pd: 2 for a packed matrix, 8 for OAM).
/// Each source is the scales (two 8.8 halfwords) and the angle. Registers
/// end as `BgAffineSet`'s.
pub fn obj_affine_set(bus: &mut Bus, r: &mut [u32; 15]) {
    let (mut src, mut dst, stride) = (r[0], r[1], r[3]);
    for _ in 0..(r[2] as i32).max(0) {
        let sx = bus.read16(src) as i16 as i32;
        let sy = bus.read16(src.wrapping_add(2)) as i16 as i32;
        for v in obj_affine(sx, sy, bus.read16(src.wrapping_add(4))) {
            bus.write16(dst, v as u16);
            dst = dst.wrapping_add(stride);
        }
        src = src.wrapping_add(8);
    }
    r[0] = src;
    r[1] = dst;
    r[2] = end_count(r[2]);
}

/// glibc's `exp2f` table: `2^(i/32)` as a double's bits, less `i << 47`.
#[rustfmt::skip]
const EXP2F_TABLE: [u64; 32] = [
    0x3FF0000000000000, 0x3FEFD9B0D3158574, 0x3FEFB5586CF9890F, 0x3FEF9301D0125B51,
    0x3FEF72B83C7D517B, 0x3FEF54873168B9AA, 0x3FEF387A6E756238, 0x3FEF1E9DF51FDEE1,
    0x3FEF06FE0A31B715, 0x3FEEF1A7373AA9CB, 0x3FEEDEA64C123422, 0x3FEECE086061892D,
    0x3FEEBFDAD5362A27, 0x3FEEB42B569D4F82, 0x3FEEAB07DD485429, 0x3FEEA47EB03A5585,
    0x3FEEA09E667F3BCD, 0x3FEE9F75E8EC5F74, 0x3FEEA11473EB0187, 0x3FEEA589994CCE13,
    0x3FEEACE5422AA0DB, 0x3FEEB737B0CDC5E5, 0x3FEEC49182A3F090, 0x3FEED503B23E255D,
    0x3FEEE89F995AD3AD, 0x3FEEFF76F2FB5E47, 0x3FEF199BDD85529C, 0x3FEF3720DCEF9069,
    0x3FEF5818DCFBA487, 0x3FEF7C97337B9B5F, 0x3FEFA4AFA2A490DA, 0x3FEFD0765B6E4540,
];

/// `2^x` rounded as glibc's `exp2f` rounds it (sysdeps/ieee754/flt-32,
/// the same in musl): `x = k/32 + r`, a table entry for `2^(k/32)` and a
/// cubic in double for `2^r`.
pub fn exp2f(x: f32) -> f32 {
    let abstop = (x.to_bits() >> 20) & 0x7FF;
    if abstop >= (128f32.to_bits() >> 20) {
        if x == f32::NEG_INFINITY {
            return 0.0;
        }
        if abstop >= (f32::INFINITY.to_bits() >> 20) {
            return x + x;
        }
        if x > 0.0 {
            return f32::INFINITY;
        }
        if x <= -150.0 {
            return 0.0;
        }
    }
    let shift = f64::from_bits(0x42E8_0000_0000_0000); // 0x1.8p52 / 32
    let c = [
        f64::from_bits(0x3FAC_6AF8_4B91_2394),
        f64::from_bits(0x3FCE_BFCE_50FA_C4F3),
        f64::from_bits(0x3FE6_2E42_FF0C_52D6),
    ];
    let xd = x as f64;
    let kd = xd + shift;
    let ki = kd.to_bits();
    let r = xd - (kd - shift);
    let s = f64::from_bits(EXP2F_TABLE[(ki % 32) as usize].wrapping_add(ki << 47));
    let z = c[0] * r + c[1];
    let y = c[2] * r + 1.0;
    ((z * (r * r) + y) * s) as f32
}

/// `MidiKey2Freq` (r0: a WaveData, its frequency word at +4; r1: the MIDI
/// key; r2: the fine adjust, 1/256 key): r0 = freq / 2^((180 - key -
/// fine/256) / 12), in floats as mGBA's HLE BIOS does it (the real BIOS
/// uses a table). mGBA's registers are `int32_t`, so r1 and r2 count as
/// signed, and the quotient goes back as x86-64 converts a float to one:
/// truncated, with 2^31 and up, infinity and NaN all 0x8000_0000. Only r0
/// changes.
pub fn midi_key2freq(bus: &mut Bus, r: &mut [u32; 15]) {
    let freq = bus.read32(r[0].wrapping_add(4));
    let e = (180.0 - r[1] as i32 as f32 - r[2] as i32 as f32 / 256.0) / 12.0;
    let q = freq as f32 / exp2f(e);
    r[0] = if q < 2_147_483_648.0 {
        q as i32 as u32
    } else {
        0x8000_0000
    };
}
