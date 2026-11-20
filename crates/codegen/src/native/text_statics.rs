//! The game's text as strings an editor can change (unit
//! `native/lift-text-editable`; docs/native-port-design.md, section 8,
//! "Text"). Each string the code's constants lead to that the names
//! files' text table decodes ([`crate::lower::word_texts`]: the string a
//! constant points at, or each one a table of pointers at it leads to)
//! becomes a `runtime::text::Text` in the crate's `src/text.rs`, its text a
//! string literal; the prelude's ROM is `runtime::text::patch`ed with
//! them, the table ([`Texts::module`]'s `CHARSET`) encoding each literal
//! at build time. An unedited lift's ROM is the cartridge's, byte for
//! byte: a string is only taken where its literal encodes back to its
//! bytes.
//!
//! A string may grow (`grow: true`: moved past the ROM's end, the words
//! that point at it pointed there) where every reader of it is known:
//! the ROM words that hold its address. One a code constant leads to
//! directly is read at that constant (the lift prints it as a number), so
//! it is edited in place only, at most its slot (`grow: false`), as is
//! one a ROM word or a constant points inside of. Left in
//! the ROM and counted (`texts_left`), each with why: a string some
//! computed load reads (a ROM static the value sets carved over it: a
//! reader the lift can't place), one inside another's bytes, and one whose
//! literal doesn't encode back to its bytes (a spelling two byte
//! sequences share).

use crate::func::Ctx;
use crate::names::SourceMap;
use crate::native::rom_data::RomData;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// One string taken.
#[derive(Clone, Debug)]
pub struct TextStatic {
    pub at: u32,
    pub slot: u32,
    pub grow: bool,
    pub refs: Vec<u32>,
    pub s: String,
    /// Where it was found: the datum, or the table and index.
    pub found: String,
}

impl TextStatic {
    pub fn name(&self) -> String {
        format!("TEXT_{:08X}", self.at)
    }
}

/// The lift's text: the strings taken, those left (by address, why), and
/// the table's spellings for encoding, the longest first.
#[derive(Clone, Debug, Default)]
pub struct Texts {
    pub texts: Vec<TextStatic>,
    pub left: Vec<(u32, String)>,
    spell: Vec<(String, Vec<u8>)>,
    end: Vec<u8>,
}

/// The spellings in the order the encoder tries them
/// (`runtime::text::Charset`): by first byte, the longest first among
/// those, then by text.
fn order(spell: &mut [(String, Vec<u8>)]) {
    spell.sort_by(|a, b| {
        (a.0.as_bytes().first(), b.0.len(), &a.0).cmp(&(b.0.as_bytes().first(), a.0.len(), &b.0))
    });
}

/// `s` encoded by `spell` (ordered), its end included; `None` where a
/// character has no spelling.
fn encode(spell: &[(String, Vec<u8>)], end: &[u8], s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let (sp, b) = spell
            .iter()
            .find(|(sp, _)| !sp.is_empty() && rest.starts_with(sp.as_str()))?;
        out.extend(b);
        rest = &rest[sp.len()..];
    }
    out.extend(end);
    Some(out)
}

/// Take the text. `consts`: the values of the constants and pool words in
/// the lifted functions (every code context's); `edits`: literals to print in place
/// of the decoded ones, by static name (`gbaa-lift --text-edit`).
pub fn collect(
    ctx: &Ctx,
    names: &SourceMap,
    consts: &BTreeSet<u32>,
    rom_data: &RomData,
    edits: &BTreeMap<String, String>,
) -> Result<Texts, String> {
    let Some(cm) = names.text.as_ref() else {
        return match edits.keys().next() {
            Some(n) => Err(format!("--text-edit: {n}: the lift has no text table")),
            None => Ok(Texts::default()),
        };
    };
    let rom = ctx.rom;
    // Each string, by address: where it was first found, and whether a
    // code constant points at it directly.
    let mut found: BTreeMap<u32, (String, bool)> = BTreeMap::new();
    for &x in consts
        .iter()
        .filter(|&&x| (0x0800_0000..0x0a00_0000).contains(&x))
    {
        for (via, at, label) in crate::lower::word_texts(ctx, names, x) {
            let e = found.entry(at).or_insert((label, false));
            e.1 |= via.is_none() || consts.contains(&at);
        }
    }
    // The tokens of each: spellings by bytes, the most used first.
    let mut toks: BTreeMap<u32, (Vec<crate::text::Token>, usize)> = BTreeMap::new();
    let mut uses: BTreeMap<(String, Vec<u8>), usize> = BTreeMap::new();
    let mut left = Vec::new();
    for &at in found.keys() {
        let o = (at - 0x0800_0000) as usize;
        match rom.get(o..).and_then(|b| cm.tokens(b)) {
            Some((t, n)) => {
                for k in &t {
                    *uses.entry(k.clone()).or_default() += 1;
                }
                toks.insert(at, (t, n));
            }
            None => left.push((at, "its bytes don't decode whole".to_string())),
        }
    }
    let mut by: BTreeMap<String, (usize, Vec<u8>)> = BTreeMap::new();
    for ((sp, b), n) in uses {
        let e = by.entry(sp).or_insert((0, b.clone()));
        if n > e.0 {
            *e = (n, b);
        }
    }
    for (sp, b) in cm.spellings() {
        by.entry(sp).or_insert((0, b));
    }
    let mut spell: Vec<(String, Vec<u8>)> = by.into_iter().map(|(s, (_, b))| (s, b)).collect();
    order(&mut spell);
    let end = cm.end().to_vec();
    // Every word of the ROM that points at a string, by value; and the
    // strings something points inside of (a ROM word or a constant), which
    // can't move.
    let ends: BTreeMap<u32, u32> = toks.iter().map(|(&a, (_, n))| (a, a + *n as u32)).collect();
    let inside = |w: u32| {
        ends.range(..w)
            .next_back()
            .and_then(|(&a, &e)| (w < e).then_some(a))
    };
    let mut words: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut middle: BTreeSet<u32> = consts.iter().filter_map(|&c| inside(c)).collect();
    for (k, w) in rom.as_chunks::<4>().0.iter().enumerate() {
        let w = u32::from_le_bytes(*w);
        if toks.contains_key(&w) {
            words.entry(w).or_default().push(4 * k as u32);
        } else if let Some(a) = inside(w) {
            middle.insert(a);
        }
    }
    // The crate's own encoder, as the build will run it.
    let refs: Vec<(&str, &[u8])> = spell
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_slice()))
        .collect();
    let first = runtime::text::Charset::index(&refs);
    let cs = runtime::text::Charset {
        spell: &refs,
        first: &first,
        end: &end,
    };
    let mut texts: Vec<TextStatic> = Vec::new();
    let mut taken_end = 0u32;
    for (at, (t, n)) in toks {
        let s: String = t.iter().map(|(sp, _)| sp.as_str()).collect();
        let slot = n as u32;
        let bytes = &rom[(at - 0x0800_0000) as usize..][..n];
        let e = encode(&spell, &end, &s)
            .filter(|l| l.len() <= runtime::text::MAX)
            .map(|_| runtime::text::encode(&cs, &s));
        if e.as_ref().map(|e| &e.bytes[..e.len]) != Some(bytes) {
            left.push((at, "its literal encodes to other bytes".into()));
            continue;
        }
        if at < taken_end {
            left.push((at, "inside another text's bytes".into()));
            continue;
        }
        if let Some(o) = rom_data.overlapping(at, at + slot) {
            left.push((at, format!("a computed load reads it ({})", o.name)));
            continue;
        }
        taken_end = at + slot;
        let (label, direct) = &found[&at];
        texts.push(TextStatic {
            at,
            slot,
            grow: !direct && !middle.contains(&at),
            refs: words.get(&at).cloned().unwrap_or_default(),
            s,
            found: label.clone(),
        });
    }
    for (n, s) in edits {
        let Some(t) = texts.iter_mut().find(|t| &t.name() == n) else {
            return Err(format!("--text-edit: {n}: no text static of that name"));
        };
        if encode(&spell, &end, s).is_none_or(|l| l.len() > runtime::text::MAX) {
            return Err(format!(
                "--text-edit: {n}: {s:?} has a character the game's table has no bytes for"
            ));
        }
        t.s = s.clone();
    }
    left.sort();
    Ok(Texts {
        texts,
        left,
        spell,
        end,
    })
}

/// A byte string as a Rust slice literal.
fn bytes(b: &[u8]) -> String {
    let each: Vec<String> = b.iter().map(|x| format!("{x:#04x}")).collect();
    format!("&[{}]", each.join(", "))
}

impl Texts {
    /// The crate's `src/text.rs`: the table, each string's static, and the
    /// moved strings' bytes.
    pub fn module(&self) -> String {
        let mut s = String::new();
        s.push_str(
            "// The game's text (native/lift-text-editable; runtime::text). Generated by gbaa-lift.\n\
             // Edit a literal and rebuild: the table below encodes it into the ROM at build time.\n\
             // A text longer than its slot moves past the ROM's end where `grow` is true (every\n\
             // reader of it is a ROM word in `refs`); else the build fails naming it.\n\
             use runtime::text::{Charset, Text};\n\n",
        );
        s.push_str("/// The game's table: each spelling's bytes, by first byte and the longest\n/// first; `first` indexes them by first byte.\n");
        writeln!(
            s,
            "pub static CHARSET: Charset = Charset {{\n    end: {},\n    spell: &[",
            bytes(&self.end)
        )
        .unwrap();
        for (sp, b) in &self.spell {
            writeln!(s, "        ({sp:?}, {}),", bytes(b)).unwrap();
        }
        let refs: Vec<(&str, &[u8])> = self
            .spell
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_slice()))
            .collect();
        let first: Vec<String> = runtime::text::Charset::index(&refs)
            .iter()
            .map(|n| n.to_string())
            .collect();
        writeln!(s, "    ],\n    first: &[{}],\n}};\n", first.join(", ")).unwrap();
        for t in &self.texts {
            let refs: Vec<String> = t.refs.iter().map(|r| format!("{r:#x}")).collect();
            writeln!(s, "/// {}", t.found).unwrap();
            writeln!(
                s,
                "pub static {}: Text = Text {{ at: {:#010x}, slot: {}, grow: {}, refs: &[{}], s: {:?} }};",
                t.name(),
                t.at,
                t.slot,
                t.grow,
                refs.join(", "),
                t.s
            )
            .unwrap();
        }
        s.push_str(
            "\n/// Every text, in the order the ROM is patched.\npub static TEXTS: &[&Text] = &[",
        );
        for t in &self.texts {
            write!(s, "&{}, ", t.name()).unwrap();
        }
        s.push_str("];\n\n");
        s.push_str(
            "/// The texts moved past the ROM's end, from `runtime::text::extra_base` on.\n\
             pub const EXTRA_LEN: usize = runtime::text::extra_len(&CHARSET, TEXTS);\n\
             pub static EXTRA: [u8; EXTRA_LEN] = runtime::text::extra(&CHARSET, TEXTS);\n",
        );
        s
    }

    /// The report's line.
    pub fn report(&self) -> String {
        let grow = self.texts.iter().filter(|t| t.grow).count();
        let mut s = format!(
            "texts as statics (src/text.rs): {}, of which {grow} may grow; left in the ROM: {}",
            self.texts.len(),
            self.left.len()
        );
        for (at, why) in &self.left {
            write!(s, "\n  {at:#010x}: {why}").unwrap();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_by_the_longest_spelling() {
        let mut spell = vec![
            ("A".to_string(), vec![1]),
            ("{AB}".to_string(), vec![9]),
            ("B".to_string(), vec![2]),
        ];
        order(&mut spell);
        assert_eq!(encode(&spell, &[0xff], "A{AB}B"), Some(vec![1, 9, 2, 0xff]));
        assert_eq!(encode(&spell, &[0xff], "C"), None);
        let t = Texts {
            texts: vec![TextStatic {
                at: 0x0800_0010,
                slot: 3,
                grow: true,
                refs: vec![0x40],
                s: "AB".into(),
                found: "sText".into(),
            }],
            left: vec![],
            spell,
            end: vec![0xff],
        };
        let m = t.module();
        assert!(m.contains("pub static TEXT_08000010: Text = Text { at: 0x08000010, slot: 3, grow: true, refs: &[0x40], s: \"AB\" };"));
        assert!(m.contains("(\"{AB}\", &[0x09]),"));
    }
}
