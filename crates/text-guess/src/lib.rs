//! Working out a game's text encoding from what its screens show (unit
//! mod/text-from-screen; the method and its score are in
//! `docs/research/text-from-screen.md`).
//!
//! The steps, each a subcommand of `gbaa-text-guess`:
//!
//! - [`isomorphs`] and [`relative`] (`find`): a line read off a screenshot
//!   is looked for in the ROM, by its pattern of repeats or, where the
//!   screen's font hides case, by the letters' distances in the alphabet.
//!   A fit gives codes for the line's characters, and where its strings are.
//! - [`trace::code_reads`] and [`fit_font`] (`font`): the interpreter runs
//!   a scene with its loads watched; the reads after each code the
//!   renderer takes from a string give the tables it indexes by code, the
//!   font among them.
//! - [`glyph::sheet`] (`glyphs`): every code's glyph drawn in a grid, for
//!   an agent to read into a Thingy table.
//! - [`score::strings`] (`unknown`): the codes the text uses that the
//!   table doesn't hold yet, each with a string it's in.
//! - [`score::score`] (`score`): the table against the game's real one,
//!   hidden till then: share of characters right, and of strings right.

use std::collections::{BTreeMap, HashMap};

pub mod glyph;
pub mod score;
pub mod trace;

/// The `unit`-byte little-endian code at `rom[at..]`.
pub fn code_at(rom: &[u8], at: usize, unit: usize) -> u32 {
    (0..unit).fold(0, |v, k| v | (rom[at + k] as u32) << (8 * k))
}

/// One place a line fits: its offset and the code each character took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fit {
    pub offset: usize,
    pub map: BTreeMap<char, u32>,
    /// [`relative`]'s word-initial letters and the codes they took, in
    /// order (empty for [`isomorphs`]).
    pub initials: Vec<(char, u32)>,
}

/// Every `unit`-aligned offset in `rom` where `line`'s characters stand in
/// the same pattern of repeats as the codes there: two places hold the
/// same code exactly when they hold the same character. A line of a few
/// distinct letters fits by chance, so the caller wants long ones (the
/// pattern of `EXTERMINATE ALL METROID` is met once in a ROM of 8 MB).
pub fn isomorphs(rom: &[u8], line: &str, unit: usize) -> Vec<Fit> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    // Each character's first index: the pattern the codes must repeat.
    let first: Vec<usize> = (0..n)
        .map(|i| chars.iter().position(|&c| c == chars[i]).unwrap())
        .collect();
    let mut out = Vec::new();
    if n == 0 || rom.len() < n * unit {
        return out;
    }
    let mut seen: HashMap<u32, usize> = HashMap::new();
    let mut at = 0;
    'off: while at + n * unit <= rom.len() {
        seen.clear();
        for (i, &f) in first.iter().enumerate() {
            let c = code_at(rom, at + i * unit, unit);
            match seen.get(&c) {
                Some(&j) if f == j => {}
                Some(_) => {
                    at += unit;
                    continue 'off;
                }
                None if f == i => {
                    seen.insert(c, i);
                }
                None => {
                    at += unit;
                    continue 'off;
                }
            }
        }
        let map = chars
            .iter()
            .enumerate()
            .map(|(i, &ch)| (ch, code_at(rom, at + i * unit, unit)))
            .collect();
        out.push(Fit {
            offset: at,
            map,
            initials: Vec::new(),
        });
        at += unit;
    }
    out
}

/// A line in a font that shows small letters as capitals (as Metroid's
/// intro does): every `unit`-aligned offset where the letters inside each
/// word stand as far apart in code as in the alphabet (a relative search,
/// the ROM-hacking community's way), a word's first letter left free as
/// it may be a capital of another run, and the other characters each on
/// one code of their own. A fit's map gives the letters inside words by
/// their capital; [`Fit::initials`] the free first letters.
pub fn relative(rom: &[u8], line: &str, unit: usize) -> Vec<Fit> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let letter = |i: usize| chars[i].is_ascii_alphabetic();
    let initial = |i: usize| letter(i) && (i == 0 || !letter(i - 1));
    let inner: Vec<usize> = (0..n).filter(|&i| letter(i) && !initial(i)).collect();
    let others: Vec<usize> = (0..n).filter(|&i| !letter(i)).collect();
    let mut out = Vec::new();
    if inner.len() < 2 || rom.len() < n * unit {
        return out;
    }
    let up = |i: usize| chars[i].to_ascii_uppercase() as u32;
    let mut at = 0;
    'off: while at + n * unit <= rom.len() {
        let code = |i: usize| code_at(rom, at + i * unit, unit);
        let base = code(inner[0]).wrapping_sub(up(inner[0]));
        for &i in &inner[1..] {
            if code(i).wrapping_sub(up(i)) != base {
                at += unit;
                continue 'off;
            }
        }
        // Other characters: one code each, none a letter's.
        let mut seen: HashMap<char, u32> = HashMap::new();
        for &i in &others {
            let c = code(i);
            let letterish = (b'A' as u32..=b'Z' as u32).contains(&c.wrapping_sub(base));
            match seen.get(&chars[i]) {
                Some(&k) if k != c => {
                    at += unit;
                    continue 'off;
                }
                _ if letterish || seen.iter().any(|(&ch, &k)| k == c && ch != chars[i]) => {
                    at += unit;
                    continue 'off;
                }
                _ => {
                    seen.insert(chars[i], c);
                }
            }
        }
        let mut map: BTreeMap<char, u32> = ('A'..='Z')
            .map(|ch| (ch, base.wrapping_add(ch as u32)))
            .collect();
        map.extend(seen);
        let initials = (0..n)
            .filter(|&i| initial(i))
            .map(|i| (chars[i], code(i)))
            .collect();
        out.push(Fit {
            offset: at,
            map,
            initials,
        });
        at += unit;
    }
    out
}

/// A table the renderer indexes by code: `base + code * stride`, and how
/// many distinct codes' reads it explains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontFit {
    pub base: u32,
    pub stride: u32,
    pub codes: usize,
}

/// The tables `reads` index by code, best first: `reads` pairs each code
/// the renderer took from a string with the cartridge addresses it read
/// before the next code. For each power-of-two stride from 1 to 512, every
/// read proposes `addr - code * stride` as a base; a base that many codes
/// propose is a table indexed by code (a width table at stride 1, the
/// glyphs at their size). Fits under `min` codes are dropped.
pub fn fit_font(reads: &[(u32, Vec<u32>)], min: usize) -> Vec<FontFit> {
    let mut out = Vec::new();
    for shift in 0..10 {
        let stride = 1u32 << shift;
        let mut votes: HashMap<u32, std::collections::BTreeSet<u32>> = HashMap::new();
        for (code, addrs) in reads {
            for &a in addrs {
                let base = a.wrapping_sub(code.wrapping_mul(stride));
                votes.entry(base).or_default().insert(*code);
            }
        }
        for (base, codes) in votes {
            if codes.len() >= min {
                out.push(FontFit {
                    base,
                    stride,
                    codes: codes.len(),
                });
            }
        }
    }
    out.sort_by_key(|f| {
        (
            std::cmp::Reverse(f.codes),
            std::cmp::Reverse(f.stride),
            f.base,
        )
    });
    // A table at stride s also fits at s's divisors with the same base:
    // keep the largest.
    let mut kept: Vec<FontFit> = Vec::new();
    for f in out {
        if !kept.iter().any(|k| k.base == f.base && k.codes == f.codes) {
            kept.push(f);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isomorph_finds_a_line_by_its_repeats() {
        // "ABBA C" in a 2-byte code: A=0x41 B=0x42 space=0x40 C=0x43.
        let mut rom = vec![9u8; 6];
        for c in [0x41u16, 0x42, 0x42, 0x41, 0x40, 0x43] {
            rom.extend_from_slice(&c.to_le_bytes());
        }
        rom.extend_from_slice(&[7, 7, 7, 7]);
        let fits = isomorphs(&rom, "ABBA C", 2);
        assert_eq!(fits.len(), 1);
        assert_eq!(fits[0].offset, 6);
        assert_eq!(fits[0].map[&'B'], 0x42);
        assert_eq!(fits[0].map[&' '], 0x40);
        // Different characters on the same code don't fit.
        assert!(isomorphs(&rom, "ABCA C", 2).is_empty());
    }

    #[test]
    fn relative_search_folds_a_capital_initial() {
        // "Abc de." with small letters at 0xC1 + n, capitals at 0x81 + n,
        // space 0x40, full stop 0x4E.
        let mut rom = vec![0u8; 4];
        for c in [0x81u16, 0xc2, 0xc3, 0x40, 0xc4, 0xc5, 0x4e] {
            rom.extend_from_slice(&c.to_le_bytes());
        }
        let fits = relative(&rom, "ABC DE.", 2);
        assert_eq!(fits.len(), 1);
        assert_eq!(fits[0].offset, 4);
        assert_eq!(fits[0].map[&'A'], 0xc1);
        assert_eq!(fits[0].map[&' '], 0x40);
        assert_eq!(fits[0].initials, vec![('A', 0x81), ('D', 0xc4)]);
    }

    #[test]
    fn font_fit_finds_the_glyph_table() {
        // Glyphs of 64 bytes at 0x1000, a width byte per code at 0x800,
        // and a pool word every code reads.
        let reads: Vec<(u32, Vec<u32>)> = [0x93u32, 0xd4, 0xc1, 0xd2]
            .iter()
            .map(|&c| {
                (
                    c,
                    vec![0x500, 0x800 + c, 0x1000 + 64 * c, 0x1000 + 64 * c + 4],
                )
            })
            .collect();
        let fits = fit_font(&reads, 3);
        assert!(fits.contains(&FontFit {
            base: 0x1000,
            stride: 64,
            codes: 4
        }));
        assert!(fits.contains(&FontFit {
            base: 0x800,
            stride: 1,
            codes: 4
        }));
        // The pool word reads at stride 0 only, which isn't tried.
        assert!(!fits.iter().any(|f| f.base == 0x500));
    }
}
