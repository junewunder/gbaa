//! A game's text, decoded for the readable form's comments
//! (`docs/mod-view-design.md`, "Text"): the source map's `[text]`, a
//! pret character table (`charmap.txt`, which `just pret-names` reads as
//! data), turns the bytes a pool word points at into
//! `// gText_MainMenuNewGame: "NEW GAME"`. A map with no table gets the
//! bytes instead ([`Preview`]; "Bytes where no table is known").
//!
//! - **Characters** are the table's quoted entries; the first entry for a
//!   byte sequence wins, so Latin over the Japanese tables that reuse the
//!   same bytes. An entry in a Japanese script (kana, CJK) is left out: an
//!   English ROM's text never uses them, and a byte only they spell is a
//!   sign the bytes aren't text.
//! - **Codes and placeholders** are its named entries, printed by name in
//!   braces as pret's source writes them (`{PLAYER}`, `{STR_VAR_1}`,
//!   `{FONT_NORMAL}`); the longest sequence matches (`{PKMN}` over
//!   `{PK}`).
//! - **Argument names** are values a code takes, not text: the run of
//!   named entries that starts with one whose first byte an earlier
//!   character already spells (`TRANSPARENT = 00`, the colors, then
//!   `SE_*`, `MUS_*`, `PH_*`), up to the next entry led by a code byte
//!   (`0xF8`–`0xFE`). A color code's argument prints by its one-byte name
//!   (`{COLOR RED}`), a sound code's by its two-byte one (`{PLAY_SE
//!   SE_SELECT}`); every other argument prints in hex (`{PAUSE 0x0f}`).
//! - **Arguments.** pret's engine gives each `0xFC` code a length
//!   (`GetExtCtrlCodeLength`, src/string_util.c), which the table doesn't
//!   say: [`FC_ARGS`]. `0xF7`, `0xF8`, `0xF9` and `0xFD` take one byte.
//! - `0xFF` ends a string.
//!
//! Text is only said where the bytes look like it ([`Charmap::decode`]):
//! they end within [`MAX_BYTES`]; every byte decodes; its characters
//! outside ASCII are ones English text uses ([`COMMON`]); at least
//! [`MIN_PLAIN`] of the characters are ASCII letters, digits or spaces,
//! two letters or digits (one, with a placeholder); and no
//! [`MAX_SPACES`] spaces in a row. A datum DWARF types as anything but a
//! `u8` array isn't text, and nor is one the string would overrun.

use std::collections::{BTreeMap, HashMap};

/// The end of a string.
pub const EOS: u8 = 0xff;
/// The longest string decoded, in bytes.
pub const MAX_BYTES: usize = 1024;
/// The share of a string's characters that must be plain (ASCII letters,
/// digits, spaces).
pub const MIN_PLAIN: f64 = 0.6;
/// The characters outside ASCII that English text uses: Emerald's
/// (POKéMON, ’, …, ¥ for money, the gender signs).
pub const COMMON: &str = "é’‘“”…¥♂♀×·▶";
/// Spaces in a row that text never has (a tile or a table's zeros has).
pub const MAX_SPACES: usize = 8;
/// The longest comment printed, in characters; a longer one is cut with `…`.
pub const MAX_CHARS: usize = 120;

/// The arguments each `0xFC` code takes, by its second byte: pret's
/// `GetExtCtrlCodeLength` (src/string_util.c), less the code's own byte.
/// A code past the end takes none.
pub const FC_ARGS: [u8; 25] = [
    0, 1, 1, 1, 3, 1, 1, 0, 1, 0, 0, 2, 1, 1, 1, 0, 2, 1, 1, 1, 1, 0, 0, 0, 0,
];

/// One `[text]` entry, in the table's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A quoted character (`'A' = BB`), as written between the quotes but
    /// for `\'`: `\n`, `\p` and `\l` stay escapes.
    Char(String),
    /// A named code, placeholder or value (`PLAYER = FD 01`).
    Name(String),
}

/// How a table's text is laid out: pret's Pokémon tables by default
/// ([`Encoding::default`]); another game's from
/// `crates/codegen/text-tables.toml` ([`Table`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoding {
    /// Bytes a character takes (1 or 2), little-endian.
    pub unit: usize,
    /// The unit that ends a string, as bytes.
    pub end: Vec<u8>,
    /// The Pokémon engine's codes: arguments after `0xF7`–`0xFD`, argument
    /// names, `0xFD` placeholders ([`FC_ARGS`]).
    pub pokemon: bool,
    /// Characters outside ASCII the game's English text uses, besides
    /// [`COMMON`].
    pub extra: String,
    /// Control codes whose arguments follow them, by their byte (a 1-byte
    /// table's only): Minish Cap's `03 01 E8`, `{Sound:01:E8}`. They take
    /// the place of any table entry that starts with the same byte.
    pub codes: BTreeMap<u8, Code>,
}

impl Default for Encoding {
    fn default() -> Encoding {
        Encoding {
            unit: 1,
            end: vec![EOS],
            pokemon: true,
            extra: String::new(),
            codes: BTreeMap::new(),
        }
    }
}

/// A control code with arguments, printed as the game's own source writes
/// it: `{name:XX:YY}`, each argument byte in hex, the first by its name
/// where the code names its values (`{Color:Red}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code {
    pub name: String,
    /// The argument bytes after the code.
    pub args: usize,
    /// Where the first argument decides how many there are, that count by
    /// its value (Minish Cap's `04 10 xx` takes two, `05 FF` one).
    pub when: Vec<(u8, usize)>,
    /// The first argument's names, by value (`Color`: `White`, `Red`, …).
    pub values: Vec<String>,
    /// Arguments in hex without leading zeros (`{Var:1}`), not two digits.
    pub short: bool,
    /// Whole spellings of the code by its first argument's value: Minish
    /// Cap's `06 00` is `{Player}`, not `{Var:0}`.
    pub named: Vec<(u8, String)>,
}

/// A character table, ready to decode.
#[derive(Clone, Debug, Default)]
pub struct Charmap {
    /// What each byte sequence prints as: a character, or `{NAME}`'s name.
    seqs: HashMap<Vec<u8>, (bool, String)>,
    /// Argument names, by their bytes.
    args: HashMap<Vec<u8>, String>,
    /// A 2-byte table's codes that carry a value in their low byte
    /// (`GAP = 8000` spells `8005` as `{GAP(5)}`), by their high byte.
    valued: HashMap<u8, String>,
    /// The longest sequence, in bytes.
    longest: usize,
    /// The table's layout.
    enc: Encoding,
}

/// A character of a Japanese script: hiragana, katakana, CJK punctuation
/// and ideographs, and full-width forms.
fn japanese(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c as u32, 0x3000..=0x30ff | 0x3400..=0x9fff | 0xff00..=0xffef))
}

impl Charmap {
    /// A pret Pokémon table from its entries, in order.
    pub fn new(entries: Vec<(Vec<u8>, Entry)>) -> Charmap {
        Charmap::with(entries, Encoding::default())
    }

    /// The table from its entries, in order, laid out as `enc` says. Only
    /// a Pokémon table has argument names; another's named entries are all
    /// codes, and a 2-byte one's `XX00` codes also spell `XXnn` with the
    /// value `nn` ([`Charmap::valued`]).
    pub fn with(entries: Vec<(Vec<u8>, Entry)>, enc: Encoding) -> Charmap {
        let mut m = Charmap::default();
        let mut char_first = [false; 256];
        let mut in_args = false;
        for (bytes, e) in &entries {
            let Some(&first) = bytes.first() else {
                continue;
            };
            if !bytes.len().is_multiple_of(enc.unit) {
                continue;
            }
            if (0xf8..=0xfe).contains(&first) {
                in_args = false;
            }
            match e {
                Entry::Char(c) => {
                    char_first[first as usize] |= bytes.len() == 1;
                    let kept = c.chars().all(|ch| enc.extra.contains(ch));
                    if japanese(c) && !kept || bytes == &enc.end {
                        continue;
                    }
                    m.seqs.entry(bytes.clone()).or_insert((false, c.clone()));
                }
                Entry::Name(n) if enc.pokemon && (in_args || char_first[first as usize]) => {
                    in_args = true;
                    m.args.entry(bytes.clone()).or_insert(n.clone());
                }
                Entry::Name(n) => {
                    if enc.unit == 2 && bytes.len() == 2 && bytes[0] == 0 {
                        m.valued.entry(bytes[1]).or_insert(n.clone());
                    }
                    if bytes != &enc.end {
                        m.seqs.entry(bytes.clone()).or_insert((true, n.clone()));
                    }
                }
            }
        }
        m.longest = m.seqs.keys().map(Vec::len).max().unwrap_or(0);
        m.enc = enc;
        m
    }

    /// How many argument bytes follow the sequence `s`, past those it
    /// already holds (`FONT_NORMAL = FC 06 01` holds `FONT`'s one).
    fn args_after(s: &[u8]) -> usize {
        let (want, head) = match *s {
            [0xfc, code, ..] => (FC_ARGS.get(code as usize).copied().unwrap_or(0), 2),
            [0xf7 | 0xf8 | 0xf9 | 0xfd, ..] => (1, 1),
            _ => (0, 0),
        };
        (want as usize).saturating_sub(s.len().saturating_sub(head))
    }

    /// The arguments `a` of the code `s`, by name where the code takes a
    /// named value: a sound (`PLAY_BGM`, `PLAY_SE`: `SE_SELECT = 05 00`)
    /// whole, a color (`COLOR`, `HIGHLIGHT`, `SHADOW`,
    /// `COLOR_HIGHLIGHT_SHADOW`: `{… DARK_GRAY WHITE LIGHT_GRAY}`) by
    /// byte; else, and for a value with no name, each byte in hex.
    fn arg_text(&self, s: &[u8], a: &[u8]) -> String {
        let hex = |b: &u8| format!("0x{b:02x}");
        let (sound, color) = match *s {
            [0xfc, 0x0b | 0x10] => (true, false),
            [0xfc, 0x01..=0x04, ..] => (false, true),
            _ => (false, false),
        };
        if sound && let Some(n) = self.args.get(a) {
            return n.clone();
        }
        let each: Vec<String> = a
            .iter()
            .map(|b| match self.args.get(&vec![*b]) {
                Some(n) if color => n.clone(),
                _ => hex(b),
            })
            .collect();
        each.join(" ")
    }

    /// The string at the start of `bytes`, as pret's source writes it, if
    /// it decodes whole up to its end (`0xFF` in a Pokémon table) and
    /// looks like text (the module doc); and its length in bytes, the end
    /// included.
    pub fn decode(&self, bytes: &[u8]) -> Option<(String, usize)> {
        self.decode_as(bytes, true)
    }

    /// The string at the start of `bytes`, if it decodes whole up to its
    /// end, whether or not it looks like text: one a game's own message
    /// table leads to ([`Messages`]), which is text whatever it holds.
    pub fn decode_known(&self, bytes: &[u8]) -> Option<(String, usize)> {
        self.decode_as(bytes, false)
    }

    fn decode_as(&self, bytes: &[u8], check: bool) -> Option<(String, usize)> {
        let u = self.enc.unit;
        let mut out = String::new();
        let (mut plain, mut alnum, mut chars, mut holders) = (0usize, 0usize, 0usize, 0usize);
        let mut spaces = 0;
        let mut i = 0;
        loop {
            if i >= MAX_BYTES {
                return None;
            }
            let rest = bytes.get(i..).filter(|r| r.len() >= u)?;
            if rest.starts_with(&self.enc.end) {
                break;
            }
            if u == 1
                && let Some(c) = self.enc.codes.get(&rest[0])
            {
                let n = rest
                    .get(1)
                    .and_then(|f| c.when.iter().find(|w| w.0 == *f))
                    .map_or(c.args, |w| w.1);
                let a = rest.get(1..1 + n)?;
                i += 1 + n;
                if let Some((_, w)) = c.named.iter().find(|w| a.first() == Some(&w.0)) {
                    out.push('{');
                    out.push_str(w);
                    out.push('}');
                    continue;
                }
                out.push('{');
                out.push_str(&c.name);
                for (k, b) in a.iter().enumerate() {
                    out.push(':');
                    match c.values.get(*b as usize).filter(|_| k == 0) {
                        Some(v) => out.push_str(v),
                        None if c.short => out.push_str(&format!("{b:X}")),
                        None => out.push_str(&format!("{b:02X}")),
                    }
                }
                out.push('}');
                continue;
            }
            let max = self.longest.min(rest.len());
            let found = (1..=max)
                .rev()
                .filter(|n| n.is_multiple_of(u))
                .find_map(|n| self.seqs.get(&rest[..n]).map(|t| (n, t)));
            let (len, (name, text)) = match found {
                Some((n, (name, text))) => (n, (*name, text.clone())),
                // A 2-byte code with its value in the low byte.
                None if u == 2 => {
                    let n = self.valued.get(&rest[1])?;
                    (2, (true, format!("{n}({})", rest[0])))
                }
                None => return None,
            };
            let (name, text) = (&name, &text);
            let s = &bytes[i..i + len];
            i += len;
            if *name && !self.enc.pokemon {
                out.push('{');
                out.push_str(text);
                out.push('}');
            } else if *name {
                let k = Charmap::args_after(s);
                let a = bytes.get(i..i + k)?;
                if a.contains(&EOS) {
                    return None;
                }
                i += k;
                out.push('{');
                out.push_str(text);
                if k > 0 {
                    out.push(' ');
                    out.push_str(&self.arg_text(s, a));
                }
                out.push('}');
                // A placeholder (a name, a number) stands for text.
                holders += usize::from(matches!(s.first(), Some(0xfd)));
            } else {
                chars += 1;
                if check
                    && text.chars().any(|c| {
                        !c.is_ascii() && !COMMON.contains(c) && !self.enc.extra.contains(c)
                    })
                {
                    return None;
                }
                spaces = if text == " " { spaces + 1 } else { 0 };
                if check && spaces >= MAX_SPACES {
                    return None;
                }
                let c = text.chars().next().unwrap_or('\0');
                if text.chars().count() == 1 && c.is_ascii_alphanumeric() {
                    alnum += 1;
                    plain += 1;
                } else if text == " " {
                    plain += 1;
                }
                out.push_str(text);
            }
        }
        let looks = !check
            || chars > 0
                && (alnum >= 2 || holders > 0 && alnum >= 1)
                && plain as f64 >= MIN_PLAIN * chars as f64;
        looks.then_some((out, i + self.enc.end.len()))
    }

    /// The comment for a pool word at `x` whose bytes are `bytes`, named
    /// `name` (the datum it lands in, with its offset) if the map names
    /// it: `gText_MainMenuNewGame: "NEW GAME"`. `ty` and `size`: that
    /// datum's DWARF type, and its bytes from `x` on (none, or 0: unsized).
    pub fn note(
        &self,
        x: u32,
        bytes: &[u8],
        name: Option<(&str, u32)>,
        ty: Option<&str>,
        size: Option<u32>,
    ) -> Option<String> {
        let array = |t: &str| match self.enc.unit {
            2 => t.contains("u16 ["),
            _ => t.contains("u8 [") || t.contains("char ["),
        };
        if ty.is_some_and(|t| !array(t)) {
            return None;
        }
        let (s, len) = self.decode(bytes)?;
        if size.is_some_and(|n| n > 0 && (len as u32) > n) {
            return None;
        }
        let s = match s.char_indices().nth(MAX_CHARS) {
            Some((at, _)) => format!("{}…", &s[..at]),
            None => s,
        };
        let at = match name {
            Some((n, 0)) => n.to_string(),
            Some((n, off)) => format!("{n}+{off}"),
            None => format!("0x{x:08x}"),
        };
        Some(format!("{at}: \"{s}\""))
    }
}

/// The most entries a pointer table is read for when the map doesn't say
/// its size.
pub const MAX_ENTRIES: usize = 64;

impl Charmap {
    /// The strings a table of pointers at `x` leads to, for a game that
    /// reaches its text by index (Metroid: Zero Mission's
    /// `sFileScreenTextPointers[language][id]`): `rom` the cartridge from
    /// `0x0800_0000`, `size` a datum's size in bytes where the map names
    /// one starting at an address, `label` its name. Either every entry
    /// of the table at `x` is text, `at: ["Start Game", "Copy", …]`, or
    /// one of its entries is such a table (the first, as a language's
    /// that decodes; a Japanese one doesn't),
    /// `at[2] sEnglishTextPointers_FileScreen: ["Start Game", …]`. A table
    /// the map doesn't size runs to its first word that isn't a pointer
    /// into the cartridge, at most [`MAX_ENTRIES`], and is text as far as
    /// its entries decode, two at least.
    pub fn table_note(
        &self,
        x: u32,
        rom: &[u8],
        at: &str,
        size: &dyn Fn(u32) -> Option<u32>,
        label: &dyn Fn(u32) -> Option<String>,
    ) -> Option<String> {
        let leaf = |t: u32| {
            let sized = size(t).map(|n| n as usize / 4);
            let ptrs = pointers(rom, t, sized.unwrap_or(MAX_ENTRIES))?;
            let mut out = Vec::new();
            for p in &ptrs {
                match self.decode(rom.get((*p - 0x0800_0000) as usize..)?) {
                    Some((s, _)) => out.push(s),
                    None if sized.is_some() => return None,
                    None => break,
                }
            }
            (out.len() >= 2 || sized == Some(out.len()) && !out.is_empty()).then_some(out)
        };
        // The strings, as many as fit in MAX_CHARS, the first at least
        // (cut with `…` itself if it's longer); `…` says more follow.
        let show = |head: String, strings: Vec<String>| {
            let mut o = String::new();
            for (k, st) in strings.iter().enumerate() {
                let used = o.chars().count();
                if k > 0 && used + st.chars().count() + 4 > MAX_CHARS {
                    o.push_str(", …");
                    break;
                }
                if k > 0 {
                    o.push_str(", ");
                }
                o.push('"');
                match st.char_indices().nth(MAX_CHARS) {
                    Some((at, _)) => {
                        o.push_str(&st[..at]);
                        o.push_str("…\"");
                        if k + 1 < strings.len() {
                            o.push_str(", …");
                        }
                        break;
                    }
                    None => o.push_str(st),
                }
                o.push('"');
            }
            format!("{head}: [{o}]")
        };
        if let Some(st) = leaf(x) {
            return Some(show(at.to_string(), st));
        }
        let outer = pointers(rom, x, size(x).map_or(MAX_ENTRIES, |n| n as usize / 4))?;
        outer.iter().enumerate().find_map(|(k, &t)| {
            let st = leaf(t)?;
            let name = label(t).unwrap_or_else(|| format!("0x{t:08x}"));
            Some(show(format!("{at}[{k}] {name}"), st))
        })
    }
}

/// The words at `x` in `rom` (the cartridge from `0x0800_0000`) that
/// point into it, word-aligned, up to `max` or the first that doesn't; none
/// if `x` isn't aligned or its first word doesn't.
fn pointers(rom: &[u8], x: u32, max: usize) -> Option<Vec<u32>> {
    let base = 0x0800_0000u32;
    if !x.is_multiple_of(4) {
        return None;
    }
    let off = x.checked_sub(base)? as usize;
    let mut out = Vec::new();
    for k in 0..max {
        let Some(w) = rom.get(off + 4 * k..off + 4 * k + 4) else {
            break;
        };
        let w = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        if w < base || w - base >= rom.len() as u32 {
            break;
        }
        out.push(w);
    }
    (!out.is_empty()).then_some(out)
}

/// One game's table from `crates/codegen/text-tables.toml`
/// (`docs/mod-view-design.md`, "Other games' tables"): where it is under
/// `$GBAA_DATA`, its syntax, and its [`Encoding`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// The game, for people.
    pub name: String,
    /// The ROM it decodes, by sha1.
    pub rom_sha1: String,
    /// Its path under `$GBAA_DATA`.
    pub table: String,
    /// `"charmap"` (pret's `charmap.txt`) or `"tbl"` (a Thingy table).
    pub format: String,
    /// How its text is laid out.
    pub enc: Encoding,
    /// Whether code reaches its text by address (`"pointer"`), which the
    /// pool-word notes decode ([`Table::for_rom`]), or by a message number
    /// (`"index"`), which a call to one of `index.calls` takes in `r0`
    /// ([`Table::messages_for_rom`]).
    pub reached: String,
    /// Where an `"index"` game's message table is, and which calls take a
    /// message number.
    pub index: Option<Index>,
    /// Characters printed in place of the table's own entry for their
    /// bytes (`chars`): Minish Cap's `22`, which tmc's charmap names
    /// `QT`, is `\"`, the quote its decomp's text writes.
    pub chars: Vec<(Vec<u8>, String)>,
    /// Where it came from.
    pub source: String,
}

/// A game's table of messages by number (`[game.index]`): Minish Cap's
/// `gTranslations[0]`, a word per bank, each its bank's offset from the
/// table's start; a bank the same, a word per message. A number's high
/// byte is its bank, its low byte the message; each table's first word,
/// over 4, counts its entries. A number past either count, or over
/// `0xFFFF` (`ShowTextBox`'s argument may be a string's address), has no
/// message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    /// The table's address.
    pub table: u32,
    /// The functions (by the source map's names) whose first argument,
    /// `r0`, is a message number.
    pub calls: Vec<String>,
}

/// An `"index"` game's messages, ready to decode: the readable print's
/// comment at a call that passes a constant message number,
/// `message 0x0f07: "{Sound:01:E8}Heh heh heh..."`.
#[derive(Clone, Debug)]
pub struct Messages {
    pub cm: Charmap,
    pub index: Index,
}

impl Messages {
    /// Message `n`'s text, from `rom` (the cartridge from `0x0800_0000`).
    pub fn get(&self, rom: &[u8], n: u32) -> Option<String> {
        if n > 0xffff {
            return None;
        }
        let word = |a: u32| {
            let o = a.checked_sub(0x0800_0000)? as usize;
            let b = rom.get(o..o + 4)?;
            Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let entry = |t: u32, k: u32| {
            if k >= word(t)? / 4 {
                return None;
            }
            t.checked_add(word(t + 4 * k)?)
        };
        let bank = entry(self.index.table, n >> 8)?;
        let s = entry(bank, n & 0xff)?;
        let bytes = rom.get(s.checked_sub(0x0800_0000)? as usize..)?;
        Some(self.cm.decode_known(bytes)?.0)
    }

    /// The comment for a call passing `n`, if it's a message with text
    /// (message 0 of a bank is empty: no message).
    pub fn note(&self, rom: &[u8], n: u32) -> Option<String> {
        let s = self.get(rom, n).filter(|s| !s.is_empty())?;
        let s = match s.char_indices().nth(MAX_CHARS) {
            Some((at, _)) => format!("{}…", &s[..at]),
            None => s,
        };
        Some(format!("message 0x{n:04x}: \"{s}\""))
    }

    /// Every message against `want`, the game's text by bank and message
    /// as its decomp writes it (tmc's `translations/USA.json`, read by
    /// [`parse_json_banks`]): how many match, and the numbers of those
    /// that don't, with what each decoded to (`gbaa-recompile
    /// --messages-check`). A message the decomp leaves empty must decode
    /// empty, or to nothing.
    pub fn check(&self, rom: &[u8], want: &[Vec<String>]) -> (usize, Vec<(u32, Option<String>)>) {
        let (mut ok, mut bad) = (0, Vec::new());
        for (b, bank) in want.iter().enumerate() {
            for (m, w) in bank.iter().enumerate() {
                let n = ((b as u32) << 8) | m as u32;
                let got = self.get(rom, n);
                let w = w.replace('"', "\\\"").replace('\n', "\\n");
                let same = match &got {
                    Some(g) => *g == w,
                    None => w.is_empty(),
                };
                if same {
                    ok += 1;
                } else {
                    bad.push((n, got));
                }
            }
        }
        (ok, bad)
    }
}

/// A JSON array of arrays of strings (tmc's `translations/USA.json`):
/// each string unescaped (`\n`, `\"`, `\\`, `\uXXXX`).
pub fn parse_json_banks(text: &str) -> Result<Vec<Vec<String>>, String> {
    type It<'a> = std::iter::Peekable<std::str::Chars<'a>>;
    fn skip(it: &mut It) {
        while it.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            it.next();
        }
    }
    fn expect(it: &mut It, want: char) -> Result<(), String> {
        skip(it);
        match it.next() {
            Some(c) if c == want => Ok(()),
            c => Err(format!("JSON: wanted {want:?}, found {c:?}")),
        }
    }
    let mut it = text.chars().peekable();
    expect(&mut it, '[')?;
    let mut banks = Vec::new();
    loop {
        skip(&mut it);
        if it.peek() == Some(&']') {
            break;
        }
        expect(&mut it, '[')?;
        let mut bank = Vec::new();
        loop {
            skip(&mut it);
            if it.peek() == Some(&']') {
                it.next();
                break;
            }
            expect(&mut it, '"')?;
            let mut s = String::new();
            loop {
                match it.next().ok_or("JSON: unended string")? {
                    '"' => break,
                    '\\' => match it.next().ok_or("JSON: unended escape")? {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        'u' => {
                            let h: String = (0..4).filter_map(|_| it.next()).collect();
                            let c = u32::from_str_radix(&h, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or_else(|| format!("JSON: bad \\u{h}"))?;
                            s.push(c);
                        }
                        c => s.push(c),
                    },
                    c => s.push(c),
                }
            }
            bank.push(s);
        }
        banks.push(bank);
    }
    Ok(banks)
}

impl Table {
    /// The tables in the committed file.
    pub fn all() -> &'static [Table] {
        static T: std::sync::OnceLock<Vec<Table>> = std::sync::OnceLock::new();
        T.get_or_init(|| {
            Table::parse(include_str!("../text-tables.toml"))
                .expect("crates/codegen/text-tables.toml parses")
        })
    }

    /// The `[[game]]` tables in `text`, a `text-tables.toml`.
    pub fn parse(text: &str) -> Result<Vec<Table>, String> {
        let t: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let games = t.get("game").and_then(|v| v.as_array());
        games
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(n, g)| {
                let s = |k: &str| {
                    g.get(k)
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .ok_or_else(|| format!("text-tables.toml: game {n}: no {k}"))
                };
                let unit = match g.get("unit").and_then(|v| v.as_integer()) {
                    Some(u @ (1 | 2)) => u as usize,
                    _ => return Err(format!("text-tables.toml: game {n}: unit wants 1 or 2")),
                };
                let end = units(&s("end")?, unit)
                    .map_err(|e| format!("text-tables.toml: game {n}: end: {e}"))?;
                let reached = s("reached")?;
                if reached != "pointer" && reached != "index" {
                    return Err(format!("text-tables.toml: game {n}: reached {reached:?}"));
                }
                let format = s("format")?;
                if format != "charmap" && format != "tbl" {
                    return Err(format!("text-tables.toml: game {n}: format {format:?}"));
                }
                let err = |e: String| format!("text-tables.toml: game {n}: {e}");
                let codes = match g.get("codes") {
                    Some(c) if unit == 1 => parse_codes(c).map_err(err)?,
                    Some(_) => return Err(err("codes want a 1-byte table".into())),
                    None => BTreeMap::new(),
                };
                let index = match g.get("index") {
                    Some(i) => Some(parse_index(i).map_err(err)?),
                    None => None,
                };
                if (reached == "index") != index.is_some() {
                    return Err(err(
                        "[game.index] is for reached = \"index\", and it wants one".into(),
                    ));
                }
                let mut chars = Vec::new();
                for (k, v) in g
                    .get("chars")
                    .and_then(|c| c.as_table())
                    .into_iter()
                    .flatten()
                {
                    let b = units(k, unit).map_err(|e| err(format!("chars: {e}")))?;
                    let c = v.as_str().ok_or_else(|| err("chars want text".into()))?;
                    chars.push((b, c.to_string()));
                }
                Ok(Table {
                    name: s("name")?,
                    rom_sha1: s("rom_sha1")?,
                    table: s("table")?,
                    format,
                    enc: Encoding {
                        unit,
                        end,
                        pokemon: false,
                        extra: s("extra").unwrap_or_default(),
                        codes,
                    },
                    reached,
                    index,
                    chars,
                    source: s("source")?,
                })
            })
            .collect()
    }

    /// The table for the ROM whose sha1 is `sha1`, read from `$GBAA_DATA`
    /// (default `~/.local/share/gbaa`): none if no game names it, its text
    /// is reached by index (no pool word leads to it: what a pointer scan
    /// finds there is ASCII-looking bytes), or its file isn't there; an
    /// error if the file doesn't parse.
    pub fn for_rom(sha1: &str) -> Result<Option<Charmap>, String> {
        Table::loaded(sha1, "pointer")
    }

    /// The messages of the ROM whose sha1 is `sha1`, if its text is
    /// reached by index (`[game.index]`) and its table's file is there.
    pub fn messages_for_rom(sha1: &str) -> Result<Option<Messages>, String> {
        let Some(cm) = Table::loaded(sha1, "index")? else {
            return Ok(None);
        };
        let t = Table::all().iter().find(|t| t.rom_sha1 == sha1);
        Ok(t.and_then(|t| t.index.clone())
            .map(|index| Messages { cm, index }))
    }

    /// The table of the ROM whose sha1 is `sha1`, if its text is reached
    /// as `reached` says and its file is there.
    fn loaded(sha1: &str, reached: &str) -> Result<Option<Charmap>, String> {
        let found = Table::all().iter().find(|t| t.rom_sha1 == sha1);
        let Some(t) = found.filter(|t| t.reached == reached) else {
            return Ok(None);
        };
        let data = match std::env::var_os("GBAA_DATA") {
            Some(d) if !d.is_empty() => std::path::PathBuf::from(d),
            _ => match std::env::var_os("HOME") {
                Some(h) => std::path::PathBuf::from(h).join(".local/share/gbaa"),
                None => return Ok(None),
            },
        };
        let p = data.join(&t.table);
        let Ok(text) = std::fs::read_to_string(&p) else {
            return Ok(None);
        };
        t.load(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", p.display()))
    }

    /// The table in `text`, the file's contents.
    pub fn load(&self, text: &str) -> Result<Charmap, String> {
        let table = match self.format.as_str() {
            "tbl" => parse_tbl(text, self.enc.unit)?,
            _ => parse_charmap(text, self.enc.unit)?,
        };
        // The first entry for some bytes wins ([`Charmap::with`]).
        let over = self
            .chars
            .iter()
            .map(|(b, c)| (b.clone(), Entry::Char(c.clone())));
        let entries = over.chain(table).collect();
        Ok(Charmap::with(entries, self.enc.clone()))
    }
}

/// A game's `codes`: `[{ code = "03", name = "Sound", args = 2 }, …]`,
/// with `when = { "10" = 2 }` where the first argument decides how many
/// there are and `values = ["White", …]` naming the first's values.
fn parse_codes(v: &toml::Value) -> Result<BTreeMap<u8, Code>, String> {
    let hex = |h: &str| u8::from_str_radix(h, 16).map_err(|e| format!("codes: {h:?}: {e}"));
    let mut out = BTreeMap::new();
    for c in v.as_array().ok_or("codes want an array")? {
        let s = |k: &str| c.get(k).and_then(|v| v.as_str());
        let code = hex(s("code").ok_or("codes: one has no code")?)?;
        let name = s("name").ok_or("codes: one has no name")?.to_string();
        let count = |v: &toml::Value| match v.as_integer() {
            Some(n @ 0..=8) => Ok(n as usize),
            _ => Err(format!("codes: {name}: a count wants 0 to 8")),
        };
        let args = count(c.get("args").ok_or(format!("codes: {name}: no args"))?)?;
        let mut when = Vec::new();
        for (k, v) in c
            .get("when")
            .and_then(|w| w.as_table())
            .into_iter()
            .flatten()
        {
            when.push((hex(k)?, count(v)?));
        }
        let values = c
            .get("values")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or(format!("codes: {name}: values"))
            })
            .collect::<Result<_, _>>()?;
        let short = c.get("short").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut named = Vec::new();
        for (k, v) in c
            .get("named")
            .and_then(|w| w.as_table())
            .into_iter()
            .flatten()
        {
            let w = v
                .as_str()
                .ok_or(format!("codes: {name}: named wants text"))?;
            named.push((hex(k)?, w.to_string()));
        }
        let new = Code {
            name,
            args,
            when,
            values,
            short,
            named,
        };
        if out.insert(code, new).is_some() {
            return Err(format!("codes: {code:02X} twice"));
        }
    }
    Ok(out)
}

/// A game's `[game.index]`: `table = "0x08109214"`, `calls = [..]`.
fn parse_index(v: &toml::Value) -> Result<Index, String> {
    let table = v
        .get("table")
        .and_then(|t| t.as_str())
        .and_then(|t| t.strip_prefix("0x"))
        .and_then(|t| u32::from_str_radix(&t.replace('_', ""), 16).ok())
        .ok_or("index: table wants an address, \"0x08109214\"")?;
    let calls: Vec<String> = v
        .get("calls")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter_map(|c| c.as_str().map(str::to_string))
        .collect();
    if calls.is_empty() {
        return Err("index: no calls".into());
    }
    Ok(Index { table, calls })
}

/// Hex `units` of `unit` bytes each (`"FF00"`, `"FC 06 01"`), as
/// little-endian bytes.
fn units(hex: &str, unit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for t in hex.split_whitespace() {
        if t.len() != 2 * unit {
            return Err(format!("{t}: wants {} hex digits", 2 * unit));
        }
        let v = u16::from_str_radix(t, 16).map_err(|e| format!("{t}: {e}"))?;
        out.extend_from_slice(&v.to_le_bytes()[..unit]);
    }
    if out.is_empty() {
        return Err("no bytes".into());
    }
    Ok(out)
}

/// The entries of a pret `charmap.txt`: `'A' = BB`, `PLAYER = FD 01`, each
/// value `unit` bytes (a 2-byte one's `8100` is the bytes 00 81).
/// Comments (`@ …`) and blank lines are dropped; a line that is neither
/// is an error, with its number.
pub fn parse_charmap(text: &str, unit: usize) -> Result<Vec<(Vec<u8>, Entry)>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let at = || format!("charmap.txt:{}", n + 1);
        let line = line.trim();
        // A quoted `@` would be a character; keep the quote's contents out
        // of the comment cut.
        let (lhs, rest) = if let Some(q) = line.strip_prefix('\'') {
            let end = quote_end(q).ok_or_else(|| format!("{}: an unclosed quote", at()))?;
            (Some(&q[..end]), &q[end + 1..])
        } else {
            (None, line)
        };
        let rest = rest.split('@').next().unwrap_or("").trim();
        if lhs.is_none() && rest.is_empty() {
            continue;
        }
        let (name, bytes) = match lhs {
            Some(_) => ("", rest.strip_prefix('=').map(str::trim)),
            None => match rest.split_once('=') {
                Some((k, v)) => (k.trim(), Some(v.trim())),
                None => (rest, None),
            },
        };
        let bytes = bytes.ok_or_else(|| format!("{}: no `=`", at()))?;
        let bytes = units(bytes, unit).map_err(|e| format!("{}: {e}", at()))?;
        let entry = match lhs {
            Some(q) => Entry::Char(q.replace("\\'", "'")),
            None if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.is_empty() =>
            {
                Entry::Name(name.to_string())
            }
            None => return Err(format!("{}: {name:?} isn't a name", at())),
        };
        out.push((bytes, entry));
    }
    Ok(out)
}

/// Where the quote that `q` (just past an opening `'`) ends: the first `'`
/// not escaped by a `\`.
fn quote_end(q: &str) -> Option<usize> {
    let mut esc = false;
    for (i, c) in q.char_indices() {
        match c {
            '\\' if !esc => esc = true,
            '\'' if !esc => return Some(i),
            _ => esc = false,
        }
    }
    None
}

/// The entries of a Thingy table (`.tbl`), the ROM-hacking community's
/// format: `41=A`, a run of hex then `=` and the text (`8100=COLOR`
/// prints as a character, so a code in it is written `8100=[COLOR]`, which
/// becomes a name), `/FF=` the end (left out, as `end` says it), `*FE=`
/// a new line (`\n`). Each entry's hex is whole `unit`s. Blank lines and
/// `;` or `#` comments are dropped.
pub fn parse_tbl(text: &str, unit: usize) -> Result<Vec<(Vec<u8>, Entry)>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let at = || format!(".tbl:{}", n + 1);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('/') {
            continue;
        }
        let (newline, line) = match line.strip_prefix('*') {
            Some(l) => (true, l),
            None => (false, line),
        };
        let (hex, txt) = line
            .split_once('=')
            .ok_or_else(|| format!("{}: no `=`", at()))?;
        let digits = 2 * unit;
        if hex.is_empty() || !hex.len().is_multiple_of(digits) {
            return Err(format!("{}: {hex}: wants whole {unit}-byte units", at()));
        }
        let spaced: Vec<&str> = (0..hex.len() / digits)
            .map(|k| &hex[k * digits..(k + 1) * digits])
            .collect();
        let bytes = units(&spaced.join(" "), unit).map_err(|e| format!("{}: {e}", at()))?;
        let entry = if newline {
            Entry::Char("\\n".into())
        } else if let Some(nm) = txt.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            Entry::Name(nm.to_string())
        } else {
            Entry::Char(txt.to_string())
        };
        out.push((bytes, entry));
    }
    Ok(out)
}

/// The byte previews' settings, from `crates/codegen/data-preview.toml`
/// (`docs/mod-view-design.md`, "Bytes where no table is known").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preview {
    /// The most bytes one preview shows.
    pub max_bytes: usize,
}

impl Preview {
    /// The settings in the committed file.
    pub fn get() -> Preview {
        static P: std::sync::OnceLock<Preview> = std::sync::OnceLock::new();
        *P.get_or_init(|| {
            Preview::parse(include_str!("../data-preview.toml"))
                .expect("crates/codegen/data-preview.toml parses")
        })
    }

    /// The settings in `text`, a `data-preview.toml`.
    pub fn parse(text: &str) -> Result<Preview, String> {
        let t: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let n = t
            .get("max_bytes")
            .and_then(|v| v.as_integer())
            .filter(|&n| n > 0)
            .ok_or("data-preview.toml: max_bytes wants a positive integer")?;
        Ok(Preview {
            max_bytes: n as usize,
        })
    }

    /// The comment for a pool word at `x` into ROM data whose encoding
    /// nobody knows: `bytes` from `x` on (to the cartridge's end), `next`
    /// the next address after `x` that other code points at or the map
    /// names, and `name` as for [`Charmap::note`]. The bytes up to `next`,
    /// at most [`Preview::max_bytes`]:
    /// `sData+4: [u8; 3] = [0x01, 0x02, 0x03]; encoding unknown`, and
    /// `, cut at 32` where the cap, not a label, ended it.
    pub fn note(
        &self,
        x: u32,
        bytes: &[u8],
        next: Option<u32>,
        name: Option<(&str, u32)>,
    ) -> Option<String> {
        let to_label = next.map_or(usize::MAX, |n| n.saturating_sub(x) as usize);
        let len = bytes.len().min(to_label);
        let cut = len > self.max_bytes;
        let len = len.min(self.max_bytes);
        if len == 0 {
            return None;
        }
        let at = match name {
            Some((n, 0)) => n.to_string(),
            Some((n, off)) => format!("{n}+{off}"),
            None => format!("0x{x:08x}"),
        };
        let each: Vec<String> = bytes[..len].iter().map(|b| format!("0x{b:02x}")).collect();
        let tail = if cut {
            format!(", cut at {}", self.max_bytes)
        } else {
            String::new()
        };
        Some(format!(
            "{at}: [u8; {len}] = [{}]; encoding unknown{tail}",
            each.join(", ")
        ))
    }
}

/// `[text]`'s `chars`, as the source map writes them: each entry's bytes
/// in hex (`"FC 06 00"`) and its `char` or `name`.
pub fn parse_entries(v: &toml::Value) -> Result<Vec<(Vec<u8>, Entry)>, String> {
    let arr = v
        .get("chars")
        .and_then(|v| v.as_array())
        .ok_or("[text] has no chars array")?;
    arr.iter()
        .enumerate()
        .map(|(n, e)| {
            let at = format!("[text] chars[{n}]");
            let bytes = e
                .get("bytes")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{at}: no bytes"))?
                .split_whitespace()
                .map(|b| u8::from_str_radix(b, 16).map_err(|e| format!("{at}: {b}: {e}")))
                .collect::<Result<Vec<u8>, _>>()?;
            if bytes.is_empty() {
                return Err(format!("{at}: no bytes"));
            }
            let s = |k: &str| e.get(k).and_then(|v| v.as_str()).map(str::to_string);
            let entry = match (s("char"), s("name")) {
                (Some(c), None) => Entry::Char(c),
                (None, Some(n)) => Entry::Name(n),
                _ => return Err(format!("{at}: wants one of char or name")),
            };
            Ok((bytes, entry))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table in charmap.txt's order: characters, a name in the text
    /// (`PKMN`), the Japanese reuse of a byte, codes, colors and a sound.
    fn map() -> Charmap {
        let c = |b: &[u8], s: &str| (b.to_vec(), Entry::Char(s.into()));
        let n = |b: &[u8], s: &str| (b.to_vec(), Entry::Name(s.into()));
        Charmap::new(vec![
            c(&[0x00], " "),
            c(&[0x04], "Ç"),
            c(&[0x1b], "é"),
            n(&[0x53], "PK"),
            n(&[0x53, 0x54], "PKMN"),
            c(&[0xbb], "A"),
            c(&[0xbc], "B"),
            c(&[0xc8], "N"),
            c(&[0xc9], "O"),
            c(&[0xff], "$"),
            c(&[0x01], "あ"),
            c(&[0xfe], "\\n"),
            n(&[0xfd], "STRING"),
            n(&[0xfd, 0x01], "PLAYER"),
            n(&[0xfd, 0x01], "B_BUFF2"),
            n(&[0xfc, 0x01], "COLOR"),
            n(&[0xfc, 0x06], "FONT"),
            n(&[0xfc, 0x08], "PAUSE"),
            n(&[0xfc, 0x10], "PLAY_SE"),
            n(&[0xfc, 0x06, 0x01], "FONT_NORMAL"),
            n(&[0x00], "TRANSPARENT"),
            n(&[0x04], "RED"),
            n(&[0x05, 0x00], "SE_SELECT"),
            n(&[0xf7, 0x00], "SE_M_HYPER_BEAM2"),
            n(&[0xf8, 0x00], "A_BUTTON"),
        ])
    }

    #[test]
    fn decodes_codes_placeholders_and_arguments() {
        let m = map();
        let s = [
            0xbb, 0xbc, 0x00, 0xfd, 0x01, 0xfe, 0xfc, 0x01, 0x04, 0xfc, 0x06, 0x01, 0xc8, 0xc9,
            0xfc, 0x08, 0x0f, 0x53, 0x54, 0x1b, 0xfc, 0x10, 0x05, 0x00, 0xf8, 0x00, 0xff, 0xbb,
        ];
        let (text, len) = m.decode(&s).unwrap();
        assert_eq!(
            text,
            "AB {PLAYER}\\n{COLOR RED}{FONT_NORMAL}NO{PAUSE 0x0f}{PKMN}é{PLAY_SE SE_SELECT}{A_BUTTON}"
        );
        assert_eq!(len, s.len() - 1);
        // The placeholder carries a lone letter.
        assert_eq!(
            m.decode(&[0xfd, 0x01, 0x00, 0xbb, 0xff]).unwrap().0,
            "{PLAYER} A"
        );
    }

    #[test]
    fn says_no_text_where_the_bytes_arent() {
        let m = map();
        // No end; a Japanese-only byte; a byte nothing spells; a code's
        // argument cut by the end; one letter; a tile's run of zeros; an
        // accented capital English text never has.
        for b in [
            &[0xbb, 0xbc][..],
            &[0xbb, 0x01, 0xbc, 0xff],
            &[0xbb, 0x99, 0xbc, 0xff],
            &[0xbb, 0xbc, 0xfc, 0x01, 0xff],
            &[0xbb, 0xff],
            &[0xbb, 0xbc, 0, 0, 0, 0, 0, 0, 0, 0, 0xff],
            &[0xbb, 0xbc, 0x04, 0xff],
        ] {
            assert_eq!(m.decode(b), None, "{b:02x?}");
        }
        let ab = [0xbb, 0xbc, 0xff];
        assert_eq!(
            m.note(
                0x0800_0000,
                &ab,
                Some(("gText_AB", 0)),
                Some("const u8 [3]"),
                Some(3)
            ),
            Some("gText_AB: \"AB\"".into())
        );
        assert_eq!(
            m.note(0x0800_0004, &ab, Some(("gText_X", 4)), None, None),
            Some("gText_X+4: \"AB\"".into())
        );
        assert_eq!(
            m.note(0x0800_0010, &ab, None, None, None),
            Some("0x08000010: \"AB\"".into())
        );
        // A struct isn't text, nor a string longer than its datum.
        assert_eq!(
            m.note(
                0,
                &ab,
                Some(("gTrainers", 0)),
                Some("struct Trainer [855]"),
                None
            ),
            None
        );
        assert_eq!(
            m.note(0, &ab, Some(("gText_A", 0)), Some("const u8 [2]"), Some(2)),
            None
        );
    }

    #[test]
    fn previews_stop_at_the_next_label_or_the_cap() {
        let p = Preview::parse("max_bytes = 4\n").unwrap();
        let b = [1, 2, 3, 4, 5, 6];
        // The next label, two bytes on, ends it.
        assert_eq!(
            p.note(0x0800_0010, &b, Some(0x0800_0012), Some(("sData", 0))),
            Some("sData: [u8; 2] = [0x01, 0x02]; encoding unknown".into())
        );
        // No label within the cap: the cap ends it, and says so.
        assert_eq!(
            p.note(0x0800_0010, &b, Some(0x0800_0020), None),
            Some(
                "0x08000010: [u8; 4] = [0x01, 0x02, 0x03, 0x04]; encoding unknown, cut at 4".into()
            )
        );
        // The cartridge's end; a label at the word itself shows nothing.
        assert_eq!(
            p.note(0x0800_0010, &b[..3], None, Some(("sData", 4))),
            Some("sData+4: [u8; 3] = [0x01, 0x02, 0x03]; encoding unknown".into())
        );
        assert_eq!(p.note(0x0800_0010, &b, Some(0x0800_0010), None), None);
        assert!(Preview::parse("max_bytes = 0\n").is_err());
        assert_eq!(Preview::get().max_bytes, 32);
    }

    /// A 2-byte table as Metroid: Zero Mission's charmap.txt writes one.
    fn wide() -> Charmap {
        let src = "' ' = 0040\n'A' = 0081\n'B' = 0082\n'N' = 008E\n'O' = 008F\n\
                   '？' = 0102\n'あ' = 0103\nSelect_button = 02C9 02CA\n\
                   GAP = 8000\nCOLOR = 8100\nAWAIT_INPUT = FC00\n'\\n' = FE00\n";
        let enc = Encoding {
            unit: 2,
            end: vec![0x00, 0xff],
            pokemon: false,
            extra: "？".into(),
            codes: BTreeMap::new(),
        };
        Charmap::with(parse_charmap(src, 2).unwrap(), enc)
    }

    #[test]
    fn decodes_a_two_byte_table_with_valued_codes() {
        let m = wide();
        let s = [
            0x05, 0x80, 0x81, 0x00, 0x82, 0x00, 0x40, 0x00, 0xc9, 0x02, 0xca, 0x02, 0x00, 0xfe,
            0x00, 0x81, 0x8e, 0x00, 0x8f, 0x00, 0x02, 0x01, 0x00, 0xfc, 0x00, 0xff, 0x81, 0x00,
        ];
        let (text, len) = m.decode(&s).unwrap();
        assert_eq!(
            text,
            "{GAP(5)}AB {Select_button}\\n{COLOR}NO？{AWAIT_INPUT}"
        );
        assert_eq!(len, 26);
        // A Japanese character the game's extra doesn't keep; an odd
        // byte left at the end.
        assert_eq!(
            m.decode(&[0x81, 0x00, 0x03, 0x01, 0x82, 0x00, 0x00, 0xff]),
            None
        );
        assert_eq!(m.decode(&[0x81, 0x00, 0x82]), None);
    }

    #[test]
    fn follows_tables_of_pointers_to_text() {
        let m = wide();
        // 0x00: a table of two languages; 0x08: Japanese (not text);
        // 0x10: English, two strings; 0x20, 0x28: the strings; 0x30: one
        // in Japanese.
        let mut rom = vec![0u8; 0x40];
        let mut word = |at: usize, w: u32| rom[at..at + 4].copy_from_slice(&w.to_le_bytes());
        word(0x00, 0x0800_0008);
        word(0x04, 0x0800_0010);
        word(0x08, 0x0800_0030);
        word(0x0c, 0x0800_0030);
        word(0x10, 0x0800_0020);
        word(0x14, 0x0800_0028);
        word(0x18, 0x1234_5678);
        rom[0x20..0x26].copy_from_slice(&[0x81, 0x00, 0x82, 0x00, 0x00, 0xff]);
        rom[0x28..0x2e].copy_from_slice(&[0x8e, 0x00, 0x8f, 0x00, 0x00, 0xff]);
        rom[0x30..0x36].copy_from_slice(&[0x03, 0x01, 0x03, 0x01, 0x00, 0xff]);
        let sizes = |a: u32| match a {
            0x0800_0000 | 0x0800_0008 | 0x0800_0010 => Some(8),
            _ => None,
        };
        let label = |a: u32| (a == 0x0800_0010).then(|| "sEnglish".to_string());
        assert_eq!(
            m.table_note(0x0800_0000, &rom, "sText", &sizes, &label),
            Some("sText[1] sEnglish: [\"AB\", \"NO\"]".into())
        );
        // Unsized, the table runs to its first word that isn't a pointer.
        assert_eq!(
            m.table_note(0x0800_0010, &rom, "x", &|_| None, &|_| None),
            Some("x: [\"AB\", \"NO\"]".into())
        );
        // Not aligned; a table with no text.
        assert_eq!(m.table_note(0x0800_0002, &rom, "x", &sizes, &label), None);
        assert_eq!(m.table_note(0x0800_0008, &rom, "x", &sizes, &label), None);
    }

    #[test]
    fn looks_messages_up_by_number_with_their_codes() {
        let all = Table::all();
        let tmc = all.iter().find(|t| t.name.contains("Minish")).unwrap();
        let entries = (0x20u8..0x7f)
            .map(|b| (vec![b], Entry::Char((b as char).to_string())))
            .chain([(vec![0x0a], Entry::Char("\\n".into()))])
            .collect();
        let cm = Charmap::with(entries, tmc.enc.clone());
        // Two banks; bank 1's message 1 has a sound code (whose argument
        // holds a 00), a color and a choice whose FF takes no second byte.
        let base = 0x0800_0000u32;
        let mut rom = vec![0u8; 0x100];
        let put = |rom: &mut Vec<u8>, at: usize, w: u32| {
            rom[at..at + 4].copy_from_slice(&w.to_le_bytes())
        };
        put(&mut rom, 0x00, 8); // bank 0 at +8
        put(&mut rom, 0x04, 0x10); // bank 1 at +0x10
        put(&mut rom, 0x08, 4); // bank 0: one message, at its own start
        put(&mut rom, 0x10, 8); // bank 1: two messages
        put(&mut rom, 0x14, 0x20);
        let text = b"\x03\x00\xd6Hi \x02\x01Bo\x02\x00!\x05\xffYes\n\x00";
        rom[0x30..0x30 + text.len()].copy_from_slice(text);
        let index = Index {
            table: base,
            calls: vec!["MessageFromTarget".into()],
        };
        let ms = Messages { cm, index };
        assert_eq!(
            ms.note(&rom, 0x0101),
            Some(
                "message 0x0101: \"{Sound:00:D6}Hi {Color:Red}Bo{Color:White}!{Choice:FF}Yes\\n\""
                    .into()
            )
        );
        // Past a bank's count, past the banks, an address.
        assert_eq!(ms.get(&rom, 0x0102), None);
        assert_eq!(ms.get(&rom, 0x0201), None);
        assert_eq!(ms.get(&rom, 0x0800_0030), None);
        assert!(Table::parse("[[game]]\nname = \"x\"\nrom_sha1 = \"0\"\ntable = \"t\"\nformat = \"tbl\"\nunit = 1\nend = \"00\"\nreached = \"index\"\nsource = \"s\"\n").is_err());
    }

    #[test]
    fn spells_messages_as_the_decomp_and_checks_them_against_its_json() {
        let all = Table::all();
        let tmc = all.iter().find(|t| t.name.contains("Minish")).unwrap();
        // tmc's charmap names 22 QT; `chars` makes it the decomp's quote.
        let cm = tmc
            .load("QT = 22\n'H' = 48\n'i' = 69\n'\\n' = 0A\n")
            .unwrap();
        let mut rom = vec![0u8; 0x40];
        rom[0..4].copy_from_slice(&8u32.to_le_bytes()); // bank 0 at +8
        rom[8..12].copy_from_slice(&8u32.to_le_bytes()); // two messages
        rom[12..16].copy_from_slice(&0x10u32.to_le_bytes());
        let text = b"H\x22i\x06\x00\x06\x0b\n\x00";
        rom[0x18..0x18 + text.len()].copy_from_slice(text);
        let index = Index {
            table: 0x0800_0000,
            calls: vec![],
        };
        let ms = Messages { cm, index };
        assert_eq!(ms.get(&rom, 1).as_deref(), Some("H\\\"i{Player}{Var:B}\\n"));
        let json = r#"[ [ "", "H\"i{Player}{Var:B}\n" ] ]"#;
        let want = parse_json_banks(json).unwrap();
        assert_eq!(
            want,
            vec![vec!["".to_string(), "H\"i{Player}{Var:B}\n".into()]]
        );
        assert_eq!(ms.check(&rom, &want), (2, vec![]));
        let bad = parse_json_banks(r#"[["", "Hi"]]"#).unwrap();
        assert_eq!(ms.check(&rom, &bad).1.len(), 1);
        assert!(parse_json_banks("[[\"x]]").is_err());
    }

    #[test]
    fn reads_tbl_files_and_the_committed_tables() {
        let src = "; a comment\n41=A\n42=B\n/FF=\n*FE=\nF0=[COLOR]\n4142=AB\n";
        let e = parse_tbl(src, 1).unwrap();
        assert_eq!(
            e,
            vec![
                (vec![0x41], Entry::Char("A".into())),
                (vec![0x42], Entry::Char("B".into())),
                (vec![0xfe], Entry::Char("\\n".into())),
                (vec![0xf0], Entry::Name("COLOR".into())),
                (vec![0x41, 0x42], Entry::Char("AB".into())),
            ]
        );
        assert!(parse_tbl("4=A\n", 1).is_err());
        assert_eq!(parse_tbl("0081=A\n", 2).unwrap()[0].0, vec![0x81, 0x00]);
        let all = Table::all();
        let mzm = all.iter().find(|t| t.name.starts_with("Metroid")).unwrap();
        assert_eq!((mzm.enc.unit, &mzm.enc.end[..]), (2, &[0x00, 0xff][..]));
        assert!(all.iter().all(|t| !t.enc.pokemon && !t.source.is_empty()));
        assert!(Table::parse("[[game]]\nname = \"x\"\nunit = 3\n").is_err());
    }
}
