//! An assembler for the `.s` songs mid2agb writes (and pret keeps built):
//! `.equ`, labels, `.byte` and `.word` of expressions over `MPlayDef.s`'s
//! symbols, and `.align 2`. Enough to turn a song's text into the bytes GNU
//! as would, so `check-pret` compares bytes, not text.

use std::collections::HashMap;

/// An assembled song: its bytes from offset 0, `.word`s of its own labels
/// resolved to their offsets and of outside symbols (the voicegroup) to 0,
/// those named in `externs` by offset. `relocs` are the offsets of the
/// words holding a label's offset (an address once placed).
#[derive(Clone, Debug, Default)]
pub struct Asm {
    pub bytes: Vec<u8>,
    pub labels: HashMap<String, u32>,
    pub externs: Vec<(u32, String)>,
    pub relocs: Vec<u32>,
}

/// The `.equ`s of a file (`MPlayDef.s`): name to expression.
pub fn equs(src: &str, into: &mut HashMap<String, String>) {
    for line in src.lines() {
        let line = line.split('@').next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix(".equ") else {
            continue;
        };
        if let Some((name, expr)) = rest.split_once(',') {
            into.insert(name.trim().to_string(), expr.trim().to_string());
        }
    }
}

enum Item<'a> {
    Label(&'a str),
    Bytes(Vec<&'a str>),
    Word(&'a str),
    Align,
}

fn items(src: &str) -> Result<Vec<Item<'_>>, String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.split('@').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(l) = line.strip_suffix(':') {
            out.push(Item::Label(l.trim()));
        } else if let Some(r) = line.strip_prefix(".byte") {
            out.push(Item::Bytes(r.split(',').map(str::trim).collect()));
        } else if let Some(r) = line.strip_prefix(".word") {
            out.push(Item::Word(r.trim()));
        } else if let Some(r) = line.strip_prefix(".align") {
            if r.trim() != "2" {
                return Err(format!("unsupported `{line}`"));
            }
            out.push(Item::Align);
        } else if [".include", ".section", ".global", ".end", ".equ"]
            .iter()
            .any(|d| line.starts_with(d))
        {
        } else {
            return Err(format!("unsupported `{line}`"));
        }
    }
    Ok(out)
}

/// Assembles `src` with `defs` (`MPlayDef.s`'s `.equ`s) and its own.
pub fn assemble(src: &str, defs: &HashMap<String, String>) -> Result<Asm, String> {
    let mut syms = defs.clone();
    equs(src, &mut syms);
    let items = items(src)?;
    let mut a = Asm::default();
    let mut at = 0u32;
    for it in &items {
        match it {
            Item::Label(l) => {
                a.labels.insert(l.to_string(), at);
            }
            Item::Bytes(b) => at += b.len() as u32,
            Item::Word(_) => at += 4,
            Item::Align => at = (at + 3) & !3,
        }
    }
    let labels = a.labels.clone();
    let ev = Eval {
        syms: &syms,
        labels: &labels,
    };
    for it in &items {
        match it {
            Item::Label(_) => {}
            Item::Bytes(b) => {
                for e in b {
                    let v = ev.eval(e, 0)?;
                    if !(-128..=255).contains(&v) {
                        return Err(format!("`{e}` = {v}, not a byte"));
                    }
                    a.bytes.push(v as u8);
                }
            }
            Item::Word(w) => {
                if labels.contains_key(*w) {
                    a.relocs.push(a.bytes.len() as u32);
                }
                let v = match ev.eval(w, 0) {
                    Ok(v) => v as u32,
                    Err(_) => {
                        a.externs.push((a.bytes.len() as u32, ev.name(w)));
                        0
                    }
                };
                a.bytes.extend(v.to_le_bytes());
            }
            Item::Align => {
                while a.bytes.len() % 4 != 0 {
                    a.bytes.push(0);
                }
            }
        }
    }
    Ok(a)
}

struct Eval<'a> {
    syms: &'a HashMap<String, String>,
    labels: &'a HashMap<String, u32>,
}

impl Eval<'_> {
    /// What an outside symbol finally names (`x_grp` → `voicegroup_x`).
    fn name(&self, s: &str) -> String {
        let mut s = s.trim();
        for _ in 0..8 {
            match self.syms.get(s) {
                Some(e) => s = e.trim(),
                None => break,
            }
        }
        s.to_string()
    }

    /// `+ - * /` left to right, `*` and `/` first, over numbers and symbols.
    fn eval(&self, e: &str, depth: u32) -> Result<i64, String> {
        if depth > 16 {
            return Err(format!("`{e}` nests too deep"));
        }
        let toks = tokens(e)?;
        let mut sum = 0i64;
        let (mut sign, mut i) = match toks.first() {
            Some(&"-") => (-1i64, 1),
            _ => (1, 0),
        };
        loop {
            let mut term = self.atom(toks.get(i).ok_or(format!("bad `{e}`"))?, depth)?;
            i += 1;
            while let Some(&op @ ("*" | "/")) = toks.get(i) {
                let r = self.atom(toks.get(i + 1).ok_or(format!("bad `{e}`"))?, depth)?;
                term = if op == "*" {
                    term * r
                } else if r == 0 {
                    return Err(format!("`{e}` divides by 0"));
                } else {
                    term / r
                };
                i += 2;
            }
            sum += sign * term;
            match toks.get(i) {
                None => return Ok(sum),
                Some(&"+") => sign = 1,
                Some(&"-") => sign = -1,
                Some(t) => return Err(format!("bad `{t}` in `{e}`")),
            }
            i += 1;
        }
    }

    fn atom(&self, t: &str, depth: u32) -> Result<i64, String> {
        if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            return i64::from_str_radix(h, 16).map_err(|e| format!("`{t}`: {e}"));
        }
        if t.as_bytes()[0].is_ascii_digit() {
            return t.parse().map_err(|e| format!("`{t}`: {e}"));
        }
        if let Some(&l) = self.labels.get(t) {
            return Ok(i64::from(l));
        }
        match self.syms.get(t) {
            Some(e) => self.eval(e, depth + 1),
            None => Err(format!("unknown symbol `{t}`")),
        }
    }
}

fn tokens(e: &str) -> Result<Vec<&str>, String> {
    let mut out = Vec::new();
    let b = e.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if b"+-*/".contains(&c) {
            out.push(&e[i..i + 1]);
            i += 1;
        } else if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            out.push(&e[s..i]);
        } else {
            return Err(format!("bad character in `{e}`"));
        }
    }
    if out.is_empty() {
        return Err("empty expression".into());
    }
    Ok(out)
}
