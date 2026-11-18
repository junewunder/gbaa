//! Finding a ROM's strings with a guessed table, and scoring the guess
//! against the game's real table (`crates/codegen/text-tables.toml`),
//! which the guess never saw.

use std::collections::{BTreeMap, HashMap};

use codegen::text::{Entry, Table};

use crate::code_at;

/// A table as code to what it prints: `Some(c)` a character (`"\n"` for a
/// new line), `None` a code with a name (a control code).
pub type Codes = HashMap<u32, Option<String>>;

/// A table's entries as [`Codes`]; entries longer than one unit (none in
/// the tables this scores) are left out, and the first entry for a code
/// wins, as [`codegen::text::Charmap`] has it.
pub fn codes(entries: &[(Vec<u8>, Entry)], unit: usize) -> Codes {
    let mut out = Codes::new();
    for (b, e) in entries {
        if b.len() != unit {
            continue;
        }
        let v = code_at(b, 0, unit);
        let what = match e {
            Entry::Char(c) => Some(c.replace("\\n", "\n")),
            Entry::Name(_) => None,
        };
        out.entry(v).or_insert(what);
    }
    out
}

/// What `t` prints for `c`: its own entry, or, for a 2-byte code `XXnn`
/// with no entry of its own, a code if `XX00` is one (a code whose low
/// byte is its value, as codegen's 2-byte tables spell `{GAP(24)}`).
pub fn lookup(t: &Codes, c: u32) -> Option<&Option<String>> {
    match t.get(&c) {
        Some(e) => Some(e),
        None if c > 0xff => t.get(&(c & 0xff00)).filter(|e| e.is_none()),
        None => None,
    }
}

/// The ROM's strings, by a table: each `unit`-aligned run of codes ending
/// in `end`, at most 1024 units, cut back to its last stretch of codes
/// that `known` holds (or `extra` allows: codes not in the table but let
/// into a string, at most a third of it), and kept when it holds at least
/// `min` characters. Offsets of its first unit, and its codes, end left
/// out.
pub fn strings(
    rom: &[u8],
    unit: usize,
    end: u32,
    known: &Codes,
    extra: &dyn Fn(u32) -> bool,
    min: usize,
) -> Vec<(usize, Vec<u32>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + unit <= rom.len() {
        if code_at(rom, at, unit) != end {
            at += unit;
            continue;
        }
        // Back from the end while codes are known or allowed.
        let mut start = at;
        let (mut chars, mut others) = (0usize, 0usize);
        while start >= unit && (at - start) / unit < 1024 {
            let c = code_at(rom, start - unit, unit);
            if c == end {
                break;
            }
            if lookup(known, c).is_some() {
                chars += 1;
            } else if extra(c) {
                others += 1;
            } else {
                break;
            }
            start -= unit;
        }
        // Allowed codes at the string's head belong to what came before.
        while start < at && lookup(known, code_at(rom, start, unit)).is_none() {
            others -= 1;
            start += unit;
        }
        if chars >= min && others * 3 <= chars + others {
            let cs = (start..at)
                .step_by(unit)
                .map(|i| code_at(rom, i, unit))
                .collect();
            out.push((start, cs));
        }
        at += unit;
    }
    out
}

/// A code a guess got wrong: what the real table has (`None` a code),
/// what the guess has (`None` nothing), and how often the strings use it.
pub type Wrong = (Option<String>, Option<Option<String>>, usize);

/// How a guess fared.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Score {
    /// Strings the real table finds, and those the guess decodes the same.
    pub strings: usize,
    pub strings_right: usize,
    /// Characters in those strings (a control code counts as one), and
    /// those the guess gives right: the same character, or a code for a
    /// code (names aren't guessed).
    pub chars: usize,
    pub chars_right: usize,
    /// The real table's distinct codes those strings use, and those the
    /// guess gives right.
    pub codes: usize,
    pub codes_right: usize,
    /// Wrong or missing codes: code, the real, the guess, how often.
    pub wrong: BTreeMap<u32, Wrong>,
}

/// Scores `guess` against `truth` over the strings the real table finds in
/// `rom` (at least `min` characters each).
pub fn score(rom: &[u8], unit: usize, end: u32, truth: &Codes, guess: &Codes, min: usize) -> Score {
    let mut s = Score::default();
    let mut used: HashMap<u32, bool> = HashMap::new();
    for (_, cs) in strings(rom, unit, end, truth, &|_| false, min) {
        // A string of codes alone isn't text.
        let want = |c: &u32| lookup(truth, *c).cloned().flatten();
        if cs.iter().all(|c| want(c).is_none()) {
            continue;
        }
        s.strings += 1;
        let mut all = true;
        for c in &cs {
            let want = &want(c);
            let got = lookup(guess, *c);
            let right = match (want, got) {
                (Some(w), Some(Some(g))) => w == g,
                (None, Some(None)) => true,
                _ => false,
            };
            s.chars += 1;
            if right {
                s.chars_right += 1;
            } else {
                all = false;
                let e = s.wrong.entry(*c).or_insert((want.clone(), got.cloned(), 0));
                e.2 += 1;
            }
            used.insert(*c, right);
        }
        if all {
            s.strings_right += 1;
        }
    }
    s.codes = used.len();
    s.codes_right = used.values().filter(|r| **r).count();
    s
}

/// A character of a Japanese script, as `codegen::text` tells one:
/// hiragana, katakana, CJK punctuation and ideographs, full-width forms.
pub fn japanese(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c as u32, 0x3000..=0x30ff | 0x3400..=0x9fff | 0xff00..=0xffef))
}

/// The real table for the ROM whose sha1 is `sha1`, from
/// `crates/codegen/text-tables.toml` and `$GBAA_DATA`: its unit, its end
/// and its codes.
pub fn real_table(sha1: &str) -> Result<(usize, u32, Codes), String> {
    let t = Table::all()
        .iter()
        .find(|t| t.rom_sha1 == sha1)
        .ok_or(format!("text-tables.toml lists no table for sha1 {sha1}"))?;
    let data = match std::env::var_os("GBAA_DATA") {
        Some(d) if !d.is_empty() => std::path::PathBuf::from(d),
        _ => std::path::PathBuf::from(std::env::var_os("HOME").ok_or("no HOME")?)
            .join(".local/share/gbaa"),
    };
    let p = data.join(&t.table);
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let entries = match t.format.as_str() {
        "tbl" => codegen::text::parse_tbl(&text, t.enc.unit)?,
        _ => codegen::text::parse_charmap(&text, t.enc.unit)?,
    };
    let end = code_at(&t.enc.end, 0, t.enc.unit);
    let mut cs = codes(&entries, t.enc.unit);
    // As the readable print has it (`codegen::text::Charmap::with`): a
    // Japanese character the English text doesn't use isn't text.
    cs.retain(|_, e| match e {
        Some(ch) => !japanese(ch) || ch.chars().all(|c| t.enc.extra.contains(c)),
        None => true,
    });
    Ok((t.enc.unit, end, cs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom_of(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn strings_are_cut_at_the_end_and_unknown_codes() {
        let t: Codes = [(0x81, Some("A".to_string())), (0x82, Some("B".into()))].into();
        // Junk, "ABA" end, "BB" end (too short at min 3).
        let rom = rom_of(&[0x1234, 0x81, 0x82, 0x81, 0xff00, 0x82, 0x82, 0xff00]);
        let s = strings(&rom, 2, 0xff00, &t, &|_| false, 3);
        assert_eq!(s, vec![(2, vec![0x81, 0x82, 0x81])]);
    }

    #[test]
    fn a_guess_is_scored_by_character_and_string() {
        let truth: Codes = [
            (0x81, Some("A".to_string())),
            (0x82, Some("B".into())),
            (0x400, None),
        ]
        .into();
        let guess: Codes = [(0x81, Some("A".to_string())), (0x400, None)].into();
        let rom = rom_of(&[0x81, 0x81, 0x400, 0x81, 0xff00, 0x81, 0x82, 0x81, 0xff00]);
        let s = score(&rom, 2, 0xff00, &truth, &guess, 3);
        assert_eq!((s.strings, s.strings_right), (2, 1));
        assert_eq!((s.chars, s.chars_right), (7, 6));
        assert_eq!((s.codes, s.codes_right), (3, 2));
        assert_eq!(s.wrong[&0x82], (Some("B".into()), None, 1));
    }

    #[test]
    fn a_valued_code_counts_as_its_code() {
        let t: Codes = [(0x8000, None), (0x81, Some("A".to_string()))].into();
        assert_eq!(lookup(&t, 0x8026), Some(&None));
        assert_eq!(lookup(&t, 0x8126), None);
        assert_eq!(lookup(&t, 0x26), None);
    }
}
