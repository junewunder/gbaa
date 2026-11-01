//! Compares `armv4t::text` with arm-none-eabi-objdump's text.
//!
//! Both sides go through [`normalise`], which takes out what is objdump's
//! style rather than the instruction: comments and `<symbol>` labels,
//! `.n`/`.w`, APCS register names, the stack names of `ldm`/`stm`, the base
//! and sign of numbers, a rotated immediate's `#imm, rot` form, and a
//! repeated first register (`adds r0, r0, #1` is `adds r0, #1`). What is left
//! has to match exactly.

use armv4t::decode::{
    Instr, canonical_arm, canonical_thumb, decode_arm, decode_thumb, pair_thumb_bl,
};
use armv4t::text::Bl;

/// One instruction line of `objdump -d`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub addr: u32,
    pub enc: Enc,
    /// The text after the encoding, tabs as spaces.
    pub text: String,
}

/// The encoding column, which also says the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enc {
    Arm(u32),
    Thumb(u16),
    /// Two Thumb halfwords, printed as one instruction (`bl`, or a Thumb-2
    /// instruction an ARMv4T doesn't have).
    Pair(u16, u16),
}

/// Parses an instruction line (` 8000000:\tea00002e \tb\t0x80000c0`), or
/// `None` for anything else.
pub fn parse_line(line: &str) -> Option<Line> {
    let mut cols = line.splitn(3, '\t');
    let addr = cols.next()?.trim().strip_suffix(':')?;
    let addr = u32::from_str_radix(addr, 16).ok()?;
    let hex: Vec<&str> = cols.next()?.split_whitespace().collect();
    let text = cols.next()?.replace('\t', " ").trim().to_string();
    let h = |s: &str| u16::from_str_radix(s, 16).ok();
    let enc = match hex[..] {
        [w] if w.len() == 8 => Enc::Arm(u32::from_str_radix(w, 16).ok()?),
        [t] if t.len() == 4 => Enc::Thumb(h(t)?),
        [a, b] if a.len() == 4 && b.len() == 4 => Enc::Pair(h(a)?, h(b)?),
        _ => return None,
    };
    Some(Line { addr, enc, text })
}

/// What [`ours`] makes of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ours {
    Text(String),
    /// Our decoder calls it undefined; objdump may know it from a later
    /// architecture.
    Undefined,
    /// Two halfwords that aren't a `bl`: objdump read a Thumb-2 instruction.
    NotV4t,
}

/// Our text for the line's encoding at its address.
pub fn ours(line: &Line) -> Ours {
    let (instr, thumb) = match line.enc {
        Enc::Arm(w) => (decode_arm(w, line.addr), false),
        Enc::Thumb(t) => (decode_thumb(t, line.addr), true),
        Enc::Pair(hi, lo) => {
            return match pair_thumb_bl(hi, lo, line.addr) {
                Some(target) => Ours::Text(Bl(target).to_string()),
                None => Ours::NotV4t,
            };
        }
    };
    match instr {
        Instr::Undefined { .. } => Ours::Undefined,
        i => Ours::Text(i.text(thumb).to_string()),
    }
}

/// Whether objdump printed data or an instruction it calls undefined.
pub fn theirs_is_data(text: &str) -> bool {
    text.starts_with('.') || text.contains("<UNDEFINED>") || text.is_empty()
}

/// The normal form both texts are compared in (see the crate docs).
pub fn normalise(text: &str) -> String {
    let text = text.to_ascii_lowercase().replace('\t', " ");
    let (mut main, comment) = match text.split_once('@') {
        Some((m, c)) => (m.trim().to_string(), c.trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    };
    // `nop @ (mov r0, r0)`, `pop {r0} @ (ldr r0, [sp], #4)`: the comment is
    // the instruction.
    let mn = main.split(' ').next().unwrap_or("");
    if matches!(mn, "nop" | "push" | "pop")
        && let Some(inner) = comment.strip_prefix('(').and_then(|c| c.strip_suffix(')'))
    {
        main = inner.to_string();
    }
    while let Some(i) = main.find('<') {
        let j = main[i..].find('>').map_or(main.len(), |j| i + j + 1);
        main.replace_range(i..j, "");
    }
    let (mn, ops) = match main.trim().split_once(' ') {
        Some((m, o)) => (m.to_string(), o.trim().to_string()),
        None => (main.trim().to_string(), String::new()),
    };
    let mut mn = mn
        .strip_suffix(".n")
        .or_else(|| mn.strip_suffix(".w"))
        .unwrap_or(&mn)
        .to_string();
    for (from, to) in [
        ("ldmfd", "ldm"),
        ("stmfd", "stmdb"),
        ("ldmia", "ldm"),
        ("stmia", "stm"),
    ] {
        if let Some(rest) = mn.strip_prefix(from) {
            mn = format!("{to}{rest}");
        }
    }
    let mut parts: Vec<String> = if ops.is_empty() {
        Vec::new()
    } else {
        ops.split(',').map(|p| regs(p.trim())).collect()
    };
    // `#88, 14`: an 8-bit immediate and its rotation.
    if parts.len() >= 2 {
        let n = parts.len();
        if let (Some(imm), Some(rot)) = (
            parts[n - 2].strip_prefix('#').and_then(number),
            number(&parts[n - 1]),
        ) {
            let v = (imm as u32).rotate_right(rot as u32);
            parts.truncate(n - 2);
            parts.push(format!("#{v}"));
        }
    }
    if parts.len() >= 3 && parts[0] == parts[1] && is_reg(&parts[0]) {
        parts.remove(1);
    }
    // `objdump -d` of an ELF prints branch targets as bare hex.
    if is_branch(&mn)
        && let [t] = &mut parts[..]
        && let Ok(v) = u32::from_str_radix(t, 16)
    {
        *t = format!("{v:#x}");
    }
    let parts: Vec<String> = parts.iter().map(|p| numbers(p)).collect();
    if parts.is_empty() {
        mn
    } else {
        format!("{mn} {}", parts.join(", "))
    }
}

/// `b`, `bl`, `beq`, `blne` and so on.
fn is_branch(mn: &str) -> bool {
    let Some(rest) = mn.strip_prefix('b') else {
        return false;
    };
    let cond = |c: &str| {
        matches!(
            c,
            "" | "eq"
                | "ne"
                | "cs"
                | "cc"
                | "mi"
                | "pl"
                | "vs"
                | "vc"
                | "hi"
                | "ls"
                | "ge"
                | "lt"
                | "gt"
                | "le"
                | "al"
                | "nv"
                | "hs"
                | "lo"
        )
    };
    cond(rest) || rest.strip_prefix('l').is_some_and(cond)
}

/// A whole decimal or `0x` number, maybe negative.
fn number(s: &str) -> Option<i64> {
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let v = match s.strip_prefix("0x") {
        Some(h) => i64::from_str_radix(h, 16).ok()?,
        None if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => s.parse().ok()?,
        None => return None,
    };
    Some(if neg { -v } else { v })
}

fn is_reg(s: &str) -> bool {
    matches!(s, "sp" | "lr" | "pc")
        || s.strip_prefix('r')
            .is_some_and(|n| n.parse::<u8>().is_ok_and(|n| n < 16))
}

/// APCS names to `r9`-`r12`, and `r13`-`r15` to `sp`, `lr`, `pc`, wherever
/// a whole word.
fn regs(part: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        out.push_str(match word.as_str() {
            "sb" => "r9",
            "sl" => "r10",
            "fp" => "r11",
            "ip" => "r12",
            "r13" => "sp",
            "r14" => "lr",
            "r15" => "pc",
            w => w,
        });
        word.clear();
    };
    for c in part.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Every number not inside a word, as the hex of its 32-bit value.
fn numbers(part: &str) -> String {
    let b = part.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let prev_word = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        let starts =
            b[i].is_ascii_digit() || (b[i] == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit));
        if starts && !prev_word {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric()) {
                j += 1;
            }
            match number(&part[i..j]) {
                Some(v) => out.push_str(&format!("{:#x}", v as u32)),
                None => out.push_str(&part[i..j]),
            }
            i = j;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// The tally of a run.
#[derive(Debug, Default)]
pub struct Tally {
    pub matched: usize,
    pub mismatched: Vec<(Line, String, String)>,
    /// Mismatches in an encoding with should-be-zero/one bits off, which
    /// objdump decodes strictly and the ARM7TDMI (and our decoder) ignores.
    pub should_be: Vec<(Line, String, String)>,
    /// objdump printed data or `<UNDEFINED>`.
    pub data: usize,
    pub undefined: usize,
    /// Thumb-2 as objdump reads it: a 32-bit pair that isn't `bl`, an
    /// `it` and the halfwords under it.
    pub not_v4t: usize,
    /// Discovered addresses objdump has no line starting at.
    pub missing: Vec<u32>,
    /// Halfwords still under an `it` objdump read.
    it_left: usize,
}

impl Tally {
    /// Adds a line; lines come in address order.
    pub fn add(&mut self, line: &Line) {
        // objdump reads `0xbfxx` as Thumb-2's `it` and puts conditions on
        // up to four halfwords after it.
        if self.it_left > 0 {
            self.it_left -= 1;
            self.not_v4t += 1;
            return;
        }
        if let Enc::Thumb(_) = line.enc
            && line.text.starts_with("it")
        {
            self.it_left = line.text.split(' ').next().map_or(1, str::len) - 1;
            self.not_v4t += 1;
            return;
        }
        if theirs_is_data(&line.text) {
            self.data += 1;
            return;
        }
        match ours(line) {
            Ours::Undefined => self.undefined += 1,
            Ours::NotV4t => self.not_v4t += 1,
            Ours::Text(t) => {
                let (a, b) = (normalise(&t), normalise(&line.text));
                let canonical = match line.enc {
                    Enc::Arm(w) => canonical_arm(w) == w,
                    Enc::Thumb(t) => canonical_thumb(t) == t,
                    Enc::Pair(..) => true,
                };
                if a == b {
                    self.matched += 1;
                } else if canonical {
                    self.mismatched.push((line.clone(), a, b));
                } else {
                    self.should_be.push((line.clone(), a, b));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
