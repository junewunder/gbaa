//! A lifted game's text as strings an editor can change (unit
//! `native/lift-text-editable`; docs/native-port-design.md, section 8,
//! "Text"). The lifter writes each string the code reaches whose bytes
//! its game's table decodes as a [`Text`] in the crate's `src/text.rs`:
//! where it lies, its slot (the bytes it had, its end included), the ROM
//! words that point at it, and the string as a literal. The crate's ROM is
//! [`patch`]ed with each literal, encoded at build time by the game's
//! table ([`Charset`]), so editing the literal changes the text on
//! screen.
//!
//! - A string that fits its slot is written over the old one in place.
//! - A longer one moves past the ROM's end ([`extra`], read where the
//!   ROM's bytes end: the prelude's `rom16`) and every word that pointed
//!   at it points at its new place, where the lifter knows every reader of
//!   the string ([`Text::grow`]); else the build fails naming it.
//!
//! All `const fn`: the crate's statics are views of the patched ROM.

/// The most bytes a string encodes to.
pub const MAX: usize = 2048;

/// A game's table, for encoding: each spelling (a character, or a code
/// as the decoder writes it, `{Color:Red}`) with its bytes, by its first
/// byte and the longest first among those, and the bytes that end a
/// string. `first[b]..first[b + 1]` are the spellings whose first byte is
/// `b` (257 entries).
pub struct Charset<'a> {
    pub spell: &'a [(&'a str, &'a [u8])],
    pub first: &'a [u16],
    pub end: &'a [u8],
}

impl<'a> Charset<'a> {
    /// `first` for `spell` as [`Charset`] orders it.
    pub fn index(spell: &[(&str, &[u8])]) -> [u16; 257] {
        let mut first = [0u16; 257];
        for (b, f) in first.iter_mut().enumerate() {
            *f = spell
                .iter()
                .take_while(|(s, _)| s.as_bytes().first().is_some_and(|&f| (f as usize) < b))
                .count() as u16;
        }
        first
    }
}

/// One string of the game's text.
pub struct Text<'a> {
    /// Its guest address.
    pub at: u32,
    /// The bytes it had, its end included: what an edit may fill in place.
    pub slot: u32,
    /// Whether a longer edit may move it: every reader of the string is a
    /// word in `refs`.
    pub grow: bool,
    /// The ROM's words (by offset) that hold its address.
    pub refs: &'a [u32],
    /// The text.
    pub s: &'a str,
}

/// A string's bytes: `len` of `bytes`.
pub struct Encoded {
    pub bytes: [u8; MAX],
    pub len: usize,
}

const fn starts_with(s: &[u8], i: usize, p: &[u8]) -> bool {
    if i + p.len() > s.len() {
        return false;
    }
    let mut k = 0;
    while k < p.len() {
        if s[i + k] != p[k] {
            return false;
        }
        k += 1;
    }
    true
}

/// `s` in the game's bytes, its end included: at each place the longest
/// spelling that starts there. Panics on a character the table has no
/// bytes for, or past [`MAX`] bytes.
pub const fn encode(cs: &Charset, s: &str) -> Encoded {
    let b = s.as_bytes();
    let mut out = Encoded {
        bytes: [0; MAX],
        len: 0,
    };
    let mut i = 0;
    while i < b.len() {
        let mut j = cs.first[b[i] as usize] as usize;
        let to = cs.first[b[i] as usize + 1] as usize;
        let mut found = false;
        while j < to {
            let (sp, bytes) = cs.spell[j];
            let sp = sp.as_bytes();
            if !sp.is_empty() && starts_with(b, i, sp) {
                out = push(out, bytes);
                i += sp.len();
                found = true;
                break;
            }
            j += 1;
        }
        if !found {
            panic!(
                "a text holds a character, or a {{code}}, its game's table has no bytes for (src/text.rs: CHARSET lists them)"
            );
        }
    }
    push(out, cs.end)
}

const fn push(mut e: Encoded, b: &[u8]) -> Encoded {
    if e.len + b.len() > MAX {
        panic!("a text encodes to more than runtime::text::MAX bytes");
    }
    let mut k = 0;
    while k < b.len() {
        e.bytes[e.len + k] = b[k];
        k += 1;
    }
    e.len += b.len();
    e
}

/// Where the moved strings start: the ROM's end, word-aligned, as a ROM
/// offset.
pub const fn extra_base(rom_len: usize) -> usize {
    rom_len.next_multiple_of(4)
}

/// The bytes the moved strings take past the ROM's end, each from a word.
pub const fn extra_len(cs: &Charset, texts: &[&Text]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < texts.len() {
        let e = encode(cs, texts[i].s);
        if e.len > texts[i].slot as usize {
            n += e.len.next_multiple_of(4);
        }
        i += 1;
    }
    n
}

/// The ROM with every text written in: in place where it fits its slot,
/// else its words pointing past the ROM's end, where [`extra`] has it.
/// Panics on a longer text that may not move.
pub const fn patch<const N: usize>(mut rom: [u8; N], cs: &Charset, texts: &[&Text]) -> [u8; N] {
    let mut next = extra_base(N);
    let mut i = 0;
    while i < texts.len() {
        let t = texts[i];
        let e = encode(cs, t.s);
        if e.len <= t.slot as usize {
            let o = (t.at - 0x0800_0000) as usize;
            let mut k = 0;
            while k < e.len {
                rom[o + k] = e.bytes[k];
                k += 1;
            }
        } else {
            if !t.grow {
                panic!(
                    "a text is longer than its slot, and a reader of it the lift can't place reads it where it is (src/text.rs: grow: false)"
                );
            }
            let a = (0x0800_0000 + next as u32).to_le_bytes();
            let mut r = 0;
            while r < t.refs.len() {
                let o = t.refs[r] as usize;
                let mut k = 0;
                while k < 4 {
                    rom[o + k] = a[k];
                    k += 1;
                }
                r += 1;
            }
            next += e.len.next_multiple_of(4);
        }
        i += 1;
    }
    if next > 32 << 20 {
        panic!("the moved texts run past the ROM's 32 MB window");
    }
    rom
}

/// The moved strings, from [`extra_base`] on: `M` is [`extra_len`]'s.
pub const fn extra<const M: usize>(cs: &Charset, texts: &[&Text]) -> [u8; M] {
    let mut out = [0u8; M];
    let mut at = 0;
    let mut i = 0;
    while i < texts.len() {
        let e = encode(cs, texts[i].s);
        if e.len > texts[i].slot as usize {
            let mut k = 0;
            while k < e.len {
                out[at + k] = e.bytes[k];
                k += 1;
            }
            at += e.len.next_multiple_of(4);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPELL: &[(&str, &[u8])] = &[
        (" ", &[0x20, 0]),
        ("A", &[0x41, 0]),
        ("B", &[0x42, 0]),
        ("{End}", &[0xfe, 0xff]),
    ];
    #[test]
    fn encodes_longest_first_and_patches() {
        let first = Charset::index(SPELL);
        assert_eq!(
            (
                first[0x20],
                first[0x21],
                first[0x41],
                first[0x42],
                first[0x7c]
            ),
            (0, 1, 1, 2, 4)
        );
        let cs = Charset {
            spell: SPELL,
            first: &first,
            end: &[0xff, 0xff],
        };
        #[allow(non_snake_case)]
        let CS = &cs;
        let e = encode(CS, "AB {End}");
        assert_eq!(
            &e.bytes[..e.len],
            &[0x41, 0, 0x42, 0, 0x20, 0, 0xfe, 0xff, 0xff, 0xff]
        );
        // In place, and moved: the word at 0 points at the ROM's end.
        let fits = Text {
            at: 0x0800_0008,
            slot: 6,
            grow: false,
            refs: &[],
            s: "B",
        };
        let moves = Text {
            at: 0x0800_0010,
            slot: 4,
            grow: true,
            refs: &[0],
            s: "AA",
        };
        let texts: &[&Text] = &[&fits, &moves];
        let rom = patch([0xEEu8; 22], CS, texts);
        assert_eq!(&rom[8..12], &[0x42, 0, 0xff, 0xff]);
        assert_eq!(rom[12], 0xEE);
        assert_eq!(&rom[0..4], &0x0800_0018u32.to_le_bytes());
        assert_eq!(extra_len(CS, texts), 8);
        assert_eq!(extra::<8>(CS, texts), [0x41, 0, 0x41, 0, 0xff, 0xff, 0, 0]);
    }
}
