//! Values printed in the width they have (`native/lift-cast-types`;
//! `docs/research/mirror-machine-count.md`, row 8). Stage A computes in
//! `u32`, so a value the lift already printed at a width (a narrowing
//! `x as u16 as u32`, a field's `g.ram.f as u32`) went back through `u32`
//! wherever it was compared, narrowed or shifted: `((x as i16 as u32) as
//! i32) < 0`. Where the printer builds those, it reads the outermost cast
//! of the operand's text ([`top_cast`]) and uses the width it shows:
//!
//! - a cast to `i32`, or to a type no wider than the operand's last cast,
//!   leaves out an intermediate 32-bit cast (`A as u32 as i32` is `A as
//!   i32`, `A as u32 as u8` is `A as u8`: Rust's integer casts compose so
//!   for every integer `A`);
//! - a compare of a value extended from `T` (`u8`, `u16`, `i8`, `i16`)
//!   with a constant `T` holds, or with another value extended from `T`,
//!   compares in `T` where extending keeps the order ([`narrow_cmp`]);
//! - an arithmetic shift of a value extended from `T` by less than `T`'s
//!   width shifts in `T`; of a value whose sign bit is a constant `&`'s 0,
//!   it is the logical shift ([`asr`]).
//!
//! Each rewrite is an identity of Rust's integer semantics on the printed
//! text, whatever the value: the lift's behaviour is unchanged.

/// How many integer casts `text` holds: ` as ` and an integer type, a
/// whole word (the `casts` metric; `mirror-machine-count.md` row 8's
/// count).
pub fn count(text: &str) -> usize {
    text.match_indices(" as ")
        .filter(|&(i, _)| {
            let rest = &text[i + 4..];
            TYPES.iter().any(|t| {
                rest.strip_prefix(t).is_some_and(|r| {
                    !r.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                })
            })
        })
        .count()
}

/// The integer types a printed cast names, with their widths and signs.
fn int(ty: &str) -> Option<(u32, bool)> {
    Some(match ty {
        "u8" => (8, false),
        "i8" => (8, true),
        "u16" => (16, false),
        "i16" => (16, true),
        "u32" => (32, false),
        "i32" => (32, true),
        "u64" => (64, false),
        "i64" => (64, true),
        _ => return None,
    })
}

/// `e` without parentheses around the whole of it.
fn unwrap(mut e: &str) -> &str {
    loop {
        let t = e.trim();
        let Some(inner) = t.strip_prefix('(').and_then(|t| t.strip_suffix(')')) else {
            return t;
        };
        // `(a) + (b)` starts and ends with parentheses that don't match.
        let mut depth = 0i32;
        for c in inner.chars() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => return t,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
        }
        e = inner;
    }
}

/// The cast `e` ends in, where it is the whole expression (in
/// parentheses or not): `(a, ty)` for `a as ty`, `ty` an integer type.
pub fn top_cast(e: &str) -> Option<(&str, &'static str)> {
    let t = unwrap(e);
    let (a, ty) = t.rsplit_once(" as ")?;
    let ty = *TYPES.iter().find(|n| **n == ty)?;
    // The text before ` as ` must be what the cast applies to: Rust's
    // own parse says whether the top of `t` is that cast.
    match syn::parse_str::<syn::Expr>(t).ok()? {
        syn::Expr::Cast(_) => Some((a.trim(), ty)),
        _ => None,
    }
}

const TYPES: [&str; 8] = ["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64"];

/// `e` cast to `ty` (an integer type no wider than 32 bits), leaving out
/// casts at the top of `e` to a type at least as wide as `ty` (they
/// change no bit the outer cast keeps), with parentheses where `e` needs
/// them. `e` is an operand already (one term or in parentheses).
pub fn cast(e: &str, ty: &str) -> String {
    format!("{} as {ty}", source(e, ty))
}

/// What [`cast`] casts: `e`, or what its outer casts to a type at least
/// as wide as `ty` apply to, as an operand of `as`.
fn source<'a>(mut e: &'a str, ty: &str) -> &'a str {
    let (w, _) = int(ty).expect("an integer type");
    while let Some((a, t)) = top_cast(e) {
        match int(t) {
            Some((tw, _)) if tw >= w => e = a,
            _ => break,
        }
    }
    e
}

/// The type `e` is extended from where it is `a as T as u32` or `a as T
/// as i32` (`T` `u8`, `u16`, `i8` or `i16`, either outer cast left out
/// too): `a as T` and `T`.
pub fn extended(e: &str) -> Option<(String, &'static str)> {
    let mut e = e;
    while let Some((a, t)) = top_cast(e) {
        match int(t)? {
            (32, _) => e = a,
            (8 | 16, _) => return Some((format!("{a} as {t}"), t)),
            _ => return None,
        }
    }
    None
}

/// Whether `k` (as the compare reads it: an `i32` if `signed`) is a
/// value of `T`, and its literal there.
fn fits(k: u32, signed: bool, ty: &str) -> Option<String> {
    let (w, ts) = int(ty)?;
    let v = if signed { (k as i32) as i64 } else { k as i64 };
    let (lo, hi) = if ts {
        (-(1i64 << (w - 1)), (1i64 << (w - 1)) - 1)
    } else {
        (0, (1i64 << w) - 1)
    };
    // A signed compare's constant prints in decimal, as `cond` prints it.
    let t = match signed {
        true if v.abs() <= 0x10000 => format!("{v}"),
        _ => literal(v),
    };
    (lo..=hi).contains(&v).then_some(t)
}

/// An integer literal: decimal to 255 in size, else hex.
pub fn literal(v: i64) -> String {
    match v {
        -255..=255 => format!("{v}"),
        v if v < 0 => format!("-{:#x}", -v),
        v => format!("{v:#x}"),
    }
}

/// A compare `l rel r` (an order or an equality; `signed`: as `i32`s)
/// in the type both sides are extended from, where that gives the same
/// answer: `l` is extended from `T`, and `r` is a constant `T` holds or
/// another value extended from `T`. Extending keeps every value of `T`
/// and, read as an `i32`, their order; read as a `u32` too when `T` is
/// unsigned. A signed `T` read as `u32` keeps only equality.
pub fn narrow_cmp(l: &str, rel: &str, r: Result<u32, &str>, signed: bool) -> Option<String> {
    let order = !matches!(rel, "==" | "!=");
    let (a, ty) = extended(l)?;
    let (_, ts) = int(ty)?;
    if order && ts && !signed {
        return None;
    }
    let b = match r {
        // A signed `T` extended to `u32` equals the constant where that
        // is a `T` sign-extended: the constant read as an `i32`.
        Ok(k) => fits(k, signed || ts, ty)?,
        Err(r) => match extended(r) {
            Some((b, t)) if t == ty => format!("({b})"),
            _ => return None,
        },
    };
    Some(format!("({a}) {rel} {b}"))
}

/// An arithmetic shift right of `x` (an operand) by `c` (1 to 31), as
/// `u32`: in `T` where `x` is extended from `T` and `c` is less than
/// `T`'s width; a logical shift where `x` is an `&` with a constant whose
/// bit 31 is 0; else through `i32`.
pub fn asr(x: &str, c: u32) -> String {
    if let Some((a, ty)) = extended(x)
        && int(ty).is_some_and(|(w, _)| c < w)
    {
        return format!("(({a}) >> {c}) as u32");
    }
    if sign_clear(x) {
        return format!("{x} >> {c}");
    }
    format!("({} >> {c}) as u32", cast_paren(x, "i32"))
}

/// [`cast`] in parentheses.
fn cast_paren(x: &str, ty: &str) -> String {
    format!("({})", cast(x, ty))
}

/// Whether `x` is `p & k` (either side the constant) with a constant `k`
/// whose bit 31 is 0, so `x` is too.
fn sign_clear(x: &str) -> bool {
    let t = unwrap(x);
    if !t.contains(" & ") {
        return false;
    }
    let Ok(syn::Expr::Binary(b)) = syn::parse_str::<syn::Expr>(t) else {
        return false;
    };
    if !matches!(b.op, syn::BinOp::BitAnd(_)) {
        return false;
    }
    let small = |e: &syn::Expr| match e {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(i),
            ..
        }) => i.base10_parse::<u32>().is_ok_and(|k| k < 0x8000_0000),
        _ => false,
    };
    small(&b.left) || small(&b.right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(
            count("(v1 as i16 as u32) as u8; x as u32x; y as usize; z as u64"),
            4
        );
    }

    #[test]
    fn top_casts() {
        assert_eq!(top_cast("(v1 as i16 as u32)"), Some(("v1 as i16", "u32")));
        assert_eq!(top_cast("g.ram.x as u32"), Some(("g.ram.x", "u32")));
        assert_eq!(top_cast("a | b as u32"), None);
        assert_eq!(top_cast("(a) + (b as u32)"), None);
        assert_eq!(top_cast("v1"), None);
        assert_eq!(top_cast("(a | b) as u32"), Some(("(a | b)", "u32")));
    }

    #[test]
    fn casts_skip_wider() {
        assert_eq!(cast("(v1 as i16 as u32)", "i32"), "v1 as i16 as i32");
        assert_eq!(cast("(v1 as u16 as u32)", "u8"), "v1 as u8");
        assert_eq!(cast("(v1 as u8 as u32)", "u16"), "v1 as u8 as u16");
        assert_eq!(cast("v3", "i32"), "v3 as i32");
    }

    #[test]
    fn narrow_compares() {
        let n = |l, rel, r, s| narrow_cmp(l, rel, r, s);
        assert_eq!(
            n("(v1 as u8 as u32)", ">", Ok(15), false).as_deref(),
            Some("(v1 as u8) > 15")
        );
        assert_eq!(n("(v1 as u8 as u32)", ">", Ok(256), false), None);
        assert_eq!(
            n("(v1 as i16 as u32)", "<=", Ok(9), true).as_deref(),
            Some("(v1 as i16) <= 9")
        );
        assert_eq!(
            n("(v1 as i16 as u32)", ">=", Ok((-320i32) as u32), true).as_deref(),
            Some("(v1 as i16) >= -320")
        );
        // A signed T read as u32 keeps only equality.
        assert_eq!(n("(v1 as i8 as u32)", "<", Ok(5), false), None);
        assert_eq!(
            n("(v1 as i8 as u32)", "==", Ok(0xffff_ffff), false).as_deref(),
            Some("(v1 as i8) == -1")
        );
        assert_eq!(n("(v1 as i8 as u32)", "==", Ok(0xff), false), None);
        assert_eq!(
            n("(v1 as u16 as u32)", "<", Err("(v2 as u16 as u32)"), true).as_deref(),
            Some("(v1 as u16) < (v2 as u16)")
        );
        assert_eq!(
            n("(v1 as u16 as u32)", "<", Err("(v2 as i16 as u32)"), true),
            None
        );
        assert_eq!(n("v1", "<", Ok(1), true), None);
    }

    #[test]
    fn arithmetic_shifts() {
        assert_eq!(asr("(v1 as i16 as u32)", 8), "((v1 as i16) >> 8) as u32");
        assert_eq!(
            asr("(v1 as i16 as u32)", 16),
            "((v1 as i16 as i32) >> 16) as u32"
        );
        assert_eq!(asr("(v8 & 0xf00u32)", 8), "(v8 & 0xf00u32) >> 8");
        assert_eq!(
            asr("(v8 & 0x80000000u32)", 8),
            "(((v8 & 0x80000000u32) as i32) >> 8) as u32"
        );
        assert_eq!(asr("v4", 8), "((v4 as i32) >> 8) as u32");
    }

    /// Each rewrite is the same value as what it replaces, over every
    /// 16-bit pattern and its sign-extended and high-bit variants.
    #[test]
    fn rewrites_are_identities() {
        for i in 0..=0xffffu32 {
            for v in [i, i | 0x8000_0000, i.wrapping_mul(0x9e37_79b9)] {
                let s16 = (v as i16 as u32) as i32;
                assert_eq!(s16, v as i16 as i32);
                for c in 1..16 {
                    assert_eq!(((s16 >> c) as u32), ((v as i16) >> c) as u32);
                    assert_eq!((((v & 0xf00) as i32) >> c) as u32, (v & 0xf00) >> c);
                }
                for k in [0i32, 9, 255, -320, 0x7fff, -0x8000] {
                    assert_eq!(s16 <= k, (v as i16) <= k as i16);
                    assert_eq!((v as i16 as u32) == k as u32, (v as i16) == k as i16);
                }
                for k in [0u32, 15, 255] {
                    assert_eq!((v as u8 as u32) > k, (v as u8) > k as u8);
                    assert_eq!(((v as u8 as u32) as i32) < k as i32, (v as u8) < k as u8);
                }
                assert_eq!(((v as u16 as u32) as u8), v as u8);
            }
        }
    }
}
