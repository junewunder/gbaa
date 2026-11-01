//! The source map (`docs/mod-view-design.md`, "The source map"): names for
//! a ROM's functions and data, keyed by address, loaded from TOML files
//! and layered, and the identifier table every named print uses.
//!
//! A map file starts with a `[map]` header pinning it to one ROM by sha1,
//! then `[functions]` and `[data]` keyed by address (`0x080003a5 = { name =
//! "AgbMain", size = 288, file = "main.o" }`) and `[copies]`, whose
//! `functions` lists name code a BIOS copy runs in RAM. From DWARF,
//! `[locals]` (each function's variables: a register or stack slot over an
//! address range), `[lines]` (address → line) and `[types]` (struct,
//! union and enum layouts by C name, which [`crate::types`] makes the
//! readable form's `Ptr<T>`s) and `[values]` (pret's `#define` families
//! a field takes, by name and value, which the readable form prints
//! stored and compared values with). Other tables (pret-names' report:
//! `[inside]`, `[unnamed]`, `[missed]`) are ignored.
//!
//! Files load in order and a later one wins per key. One file may not give
//! one name to two addresses unless their `file`s differ (statics of two
//! source files). The identifier table then makes every name a Rust
//! identifier that names exactly one key, so a named print inverts exactly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Which table a key is in: a function's key (bit 0 set for Thumb) or a
/// data byte address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Func,
    Data,
}

/// One named function or datum.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub size: Option<u64>,
    /// The object or source file it is in (`main.o`, `src/main.c`).
    pub file: Option<String>,
    /// The C prototype (functions) or type (data), as text, for comments.
    pub proto: Option<String>,
    /// A function's source file, from DWARF (`src/pokemon.c`).
    pub src: Option<String>,
}

/// Where a local lives (`[locals]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    /// A register, r0–r15.
    Reg(u8),
    /// A stack slot at this offset from `sp` after the prologue.
    Slot(i32),
}

/// A function's parameter or variable over `[from, to)`, from `[locals]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub name: String,
    pub home: Home,
    pub from: u32,
    pub to: u32,
    /// Its C type, as text.
    pub ty: Option<String>,
    pub param: bool,
}

/// A line-table row (`[lines]`): its file, `None` for the `src` of the
/// function holding it, and its line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub file: Option<String>,
    pub line: u32,
}

/// A struct's, union's or enum's layout (`[types]`), by its C name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeDef {
    /// `struct`, `union` or `enum`.
    pub kind: String,
    pub size: u32,
    pub fields: Vec<Field>,
}

/// A field of a [`TypeDef`]: its byte offset and size, C type as text,
/// and for a bitfield its bits in the storage unit at `off`, `(lowest,
/// width)`, little-endian.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub off: u32,
    pub size: u32,
    pub ty: String,
    pub bits: Option<(u32, u32)>,
}

/// A family of named values (`[values]`, `OPTIONS_TEXT_SPEED_*`): each
/// value's one name, and the fields that take it, as (type's C name, field).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Values {
    /// Where its names are defined (`include/constants/global.h`).
    pub file: String,
    pub names: Vec<(String, u32)>,
    pub fields: Vec<(String, String)>,
}

/// Every map file, layered, and the identifier of each named key.
#[derive(Debug, Default)]
pub struct SourceMap {
    pub functions: BTreeMap<u32, Item>,
    pub data: BTreeMap<u32, Item>,
    /// Each function's locals, by its key: a later layer's list replaces
    /// the function's whole list.
    pub locals: BTreeMap<u32, Vec<Local>>,
    /// The line table, by address.
    pub lines: BTreeMap<u32, Line>,
    /// `[types]`, by C name (`struct Task`, a typedef's `TextFlags`): a
    /// later layer's replaces a type whole.
    pub types: BTreeMap<String, TypeDef>,
    /// `[values]`, by family (`OPTIONS_TEXT_SPEED`): a later layer's
    /// replaces a family whole.
    pub values: BTreeMap<String, Values>,
    /// `[text]`, the game's character table: a later layer's replaces it
    /// whole.
    pub text: Option<crate::text::Charmap>,
    /// A game that reaches its text by message number: its messages, which
    /// a call passing a constant one prints (`crates/codegen/text-tables.toml`).
    pub messages: Option<crate::text::Messages>,
    /// The readable form's types, made from `types` and `data` at load.
    pub typed: crate::types::Types,
    /// Each layer's `[map] source` (`pret/pokeemerald@731ad5bf`), in
    /// order: what the names came from, for a mod's `mod.toml`.
    pub sources: Vec<String>,
    /// Each key's identifier.
    idents: BTreeMap<(Kind, u32), String>,
    /// Each identifier's key: the table's inverse.
    keys: BTreeMap<String, (Kind, u32)>,
    /// The names an identifier of the map can't take as it is: Rust's
    /// keywords, the prelude's exports and the printer's own.
    reserved: BTreeSet<String>,
}

/// Rust's keywords, strict and reserved, and the weak ones that can't
/// name an item everywhere.
pub(crate) const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "abstract",
    "become",
    "box",
    "do",
    "final",
    "gen",
    "macro",
    "override",
    "priv",
    "try",
    "typeof",
    "unsized",
    "virtual",
    "yield",
    "union",
    "safe",
    "raw",
    "macro_rules",
];

/// Names the printer itself uses in generated code besides the prelude's:
/// locals and parameters, the items of `lib.rs` and `base`, crate names,
/// and the marker macros.
const PRINTER: &[&str] = &[
    "m", "pc", "cycles", "sp0", "a", "k", "t", "x", "key", "ret", "nf", "zf", "cf", "vf", "P",
    "CODE", "BLOCKS", "INNER", "EXTENTS", "base", "prelude", "ext", "names", "table", "runtime",
    "armv4t", "desktop", "core", "std", "alloc", "insn", "tick", "call", "addr", "main", "note",
    "keys", "arg", // The readable macros' locals, and the module of typed globals.
    "vs", "i", "v", "globals",
];

/// The names [`crate::emit`]'s ROM prelude exports, read off its tokens:
/// the last segment (or the `as` name) of each `pub use`, and each `pub
/// type` and `pub fn`. So a new export reserves its name here too.
pub fn prelude_exports() -> BTreeSet<String> {
    use proc_macro2::{TokenStream, TokenTree};
    fn walk(ts: TokenStream, out: &mut BTreeSet<String>) {
        let toks: Vec<TokenTree> = ts.into_iter().collect();
        let mut i = 0;
        while i < toks.len() {
            match &toks[i] {
                TokenTree::Group(g) => walk(g.stream(), out),
                TokenTree::Ident(id) if id == "pub" => {
                    if let Some(TokenTree::Ident(what)) = toks.get(i + 1) {
                        if ["type", "fn", "mod", "struct", "trait", "enum"]
                            .iter()
                            .any(|w| what == w)
                        {
                            if let Some(TokenTree::Ident(n)) = toks.get(i + 2) {
                                out.insert(n.to_string());
                            }
                        } else if what == "use" {
                            let mut j = i + 2;
                            let mut tree = TokenStream::new();
                            while j < toks.len()
                                && !matches!(&toks[j], TokenTree::Punct(p) if p.as_char() == ';')
                            {
                                tree.extend([toks[j].clone()]);
                                j += 1;
                            }
                            use_names(tree, None, out);
                            i = j;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
    // A use tree's leaf names: the ident before each `,` or the end, or
    // after `as`; `self` in a group names the path before it, `_` nothing.
    fn use_names(ts: TokenStream, prefix: Option<&str>, out: &mut BTreeSet<String>) {
        let flush = |last: &mut Option<String>, out: &mut BTreeSet<String>| match last.take() {
            Some(n) if n == "self" => out.extend(prefix.map(str::to_string)),
            Some(n) if n != "_" => {
                out.insert(n);
            }
            _ => {}
        };
        let mut last: Option<String> = None;
        for t in ts {
            match t {
                TokenTree::Group(g) => use_names(g.stream(), last.take().as_deref(), out),
                TokenTree::Ident(id) => last = Some(id.to_string()),
                TokenTree::Punct(p) if p.as_char() == ',' => flush(&mut last, out),
                // `::` before the next segment, or `as`'s name next.
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
        }
        flush(&mut last, out);
    }
    let mut out = BTreeSet::new();
    let exact = crate::func::Clock::Exact;
    let head = crate::emit::lib_head(
        quote::quote!(runtime::dispatch),
        crate::emit::rom_alias(exact),
        exact,
    );
    walk(head, &mut out);
    out
}

/// The names a map's identifier can't take as given: the prelude's, the
/// printer's and Rust's keywords.
pub(crate) fn reserved() -> BTreeSet<String> {
    let mut r: BTreeSet<String> = prelude_exports();
    r.extend(KEYWORDS.iter().chain(PRINTER).map(|s| s.to_string()));
    r.extend(
        crate::io_regs::REGS
            .iter()
            .map(|r| crate::io_regs::ident(r.0)),
    );
    r
}

/// The printer's own numbered names: `r0`, `v12`, `f_0800_0124`'s
/// `f_<hex>`, the `m000` modules, `p00` parts and the like.
pub(crate) fn printer_pattern(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: &[u8]| !r.is_empty() && r.iter().all(u8::is_ascii_digit);
    let hex = |r: &[u8]| !r.is_empty() && r.iter().all(u8::is_ascii_hexdigit);
    match b {
        [b'f', b'_', rest @ ..] => hex(rest),
        [b'r' | b'v' | b'c' | b'f' | b'm' | b'p' | b's', rest @ ..] => digits(rest),
        _ => false,
    }
}

/// `name` as an identifier: a function-local static's `sBgCnt.135`
/// becomes `sBgCnt_135`. `None` if nothing of it is left.
pub(crate) fn as_ident(name: &str) -> Option<String> {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let first = s.chars().next()?;
    let ok = (first == '_' || first.is_ascii_alphabetic()) && s.chars().any(|c| c != '_');
    ok.then_some(s)
}

/// The stem of `file` for a qualified name: `src/scrcmd.c` → `scrcmd`.
fn stem(file: &str) -> String {
    let s = Path::new(file)
        .file_stem()
        .map_or(file.to_string(), |s| s.to_string_lossy().into_owned());
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn key(k: &str) -> Result<u32, String> {
    let h = k
        .strip_prefix("0x")
        .ok_or_else(|| format!("key {k:?} isn't a 0x address"))?;
    u32::from_str_radix(&h.replace('_', ""), 16).map_err(|e| format!("key {k:?}: {e}"))
}

fn item(v: &toml::Value, at: &str) -> Result<Item, String> {
    let t = v
        .as_table()
        .ok_or_else(|| format!("{at}: not an inline table"))?;
    let s = |f: &str| t.get(f).and_then(|v| v.as_str()).map(str::to_string);
    let name = s("name").ok_or_else(|| format!("{at}: no name"))?;
    Ok(Item {
        name,
        size: t.get("size").and_then(|v| v.as_integer()).map(|n| n as u64),
        file: s("file"),
        proto: s("proto").or_else(|| s("type")),
        src: s("src"),
    })
}

fn local(v: &toml::Value, at: &str) -> Result<Local, String> {
    let t = v
        .as_table()
        .ok_or_else(|| format!("{at}: not an inline table"))?;
    let int = |f: &str| t.get(f).and_then(|v| v.as_integer());
    let name = t
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{at}: a local with no name"))?;
    let home = match (int("reg"), int("slot")) {
        (Some(r @ 0..=15), None) => Home::Reg(r as u8),
        (None, Some(s)) => Home::Slot(s as i32),
        _ => return Err(format!("{at}: {name} needs one of reg (0-15) or slot")),
    };
    let (Some(from), Some(to)) = (int("from"), int("to")) else {
        return Err(format!("{at}: {name} needs from and to"));
    };
    Ok(Local {
        name: name.to_string(),
        home,
        from: from as u32,
        to: to as u32,
        ty: t.get("type").and_then(|v| v.as_str()).map(str::to_string),
        param: t.get("param").and_then(|v| v.as_bool()).unwrap_or(false),
    })
}

fn type_def(v: &toml::Value, at: &str) -> Result<TypeDef, String> {
    let t = v.as_table().ok_or_else(|| format!("{at}: not a table"))?;
    let int = |t: &toml::Table, f: &str| t.get(f).and_then(|v| v.as_integer());
    let mut fields = Vec::new();
    for f in t
        .get("fields")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let f = f
            .as_table()
            .ok_or_else(|| format!("{at}: a field isn't an inline table"))?;
        let s = |k: &str| f.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let bits = match f.get("bits").and_then(|v| v.as_array()).map(|b| &b[..]) {
            Some([lo, w]) => Some((
                lo.as_integer().ok_or_else(|| format!("{at}: bad bits"))? as u32,
                w.as_integer().ok_or_else(|| format!("{at}: bad bits"))? as u32,
            )),
            Some(_) => return Err(format!("{at}: bits are [lowest, width]")),
            None => None,
        };
        fields.push(Field {
            name: s("name"),
            off: int(f, "off").ok_or_else(|| format!("{at}: a field with no off"))? as u32,
            size: int(f, "size").unwrap_or(0) as u32,
            ty: s("type"),
            bits,
        });
    }
    Ok(TypeDef {
        kind: t
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("struct")
            .to_string(),
        size: int(t, "size").unwrap_or(0) as u32,
        fields,
    })
}

fn values(v: &toml::Value, at: &str) -> Result<Values, String> {
    let t = v.as_table().ok_or_else(|| format!("{at}: not a table"))?;
    let arr = |k: &str| t.get(k).and_then(|v| v.as_array()).into_iter().flatten();
    let mut fields = Vec::new();
    for f in arr("fields") {
        let (ty, name) = f
            .as_str()
            .and_then(|f| f.rsplit_once('.'))
            .ok_or_else(|| format!("{at}: a field is \"type.field\""))?;
        fields.push((ty.to_string(), name.to_string()));
    }
    let mut names = Vec::new();
    for n in arr("names") {
        let n = n
            .as_table()
            .ok_or_else(|| format!("{at}: a name isn't an inline table"))?;
        let name = n.get("name").and_then(|v| v.as_str());
        let value = n.get("value").and_then(|v| v.as_integer());
        let (Some(name), Some(value)) = (name, value) else {
            return Err(format!("{at}: a name is {{ name, value }}"));
        };
        let value = u32::try_from(value).map_err(|_| format!("{at}: {name} isn't a u32"))?;
        names.push((name.to_string(), value));
    }
    Ok(Values {
        file: t
            .get("file")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        names,
        fields,
    })
}

fn line(v: &toml::Value, at: &str) -> Result<Line, String> {
    let bad = || format!("{at}: a line is a number or \"file:line\"");
    if let Some(n) = v.as_integer() {
        return Ok(Line {
            file: None,
            line: n as u32,
        });
    }
    let (file, n) = v
        .as_str()
        .and_then(|s| s.rsplit_once(':'))
        .ok_or_else(bad)?;
    Ok(Line {
        file: Some(file.to_string()),
        line: n.parse().map_err(|_| bad())?,
    })
}

/// The SHA-1 of `data`, in hex ([`runtime::sha1`]).
pub fn sha1_hex(data: &[u8]) -> String {
    runtime::sha1::sha1(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl SourceMap {
    /// Loads `paths` in order for the ROM whose sha1 is `rom_sha1` and
    /// builds the identifier table.
    pub fn load(rom_sha1: &str, paths: &[impl AsRef<Path>]) -> Result<SourceMap, String> {
        let mut m = SourceMap::default();
        for p in paths {
            let p = p.as_ref();
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            m.layer(rom_sha1, &text)
                .map_err(|e| format!("{}: {e}", p.display()))?;
        }
        // A game with a table of its own (crates/codegen/text-tables.toml)
        // decodes with it, over any [text] its maps carry.
        if let Some(t) = crate::text::Table::for_rom(rom_sha1)? {
            m.text = Some(t);
        }
        m.messages = crate::text::Table::messages_for_rom(rom_sha1)?;
        m.build_idents();
        m.typed = crate::types::Types::new(&m);
        Ok(m)
    }

    /// Like [`SourceMap::load`], from texts.
    pub fn from_texts(rom_sha1: &str, texts: &[&str]) -> Result<SourceMap, String> {
        let mut m = SourceMap::default();
        for (i, t) in texts.iter().enumerate() {
            m.layer(rom_sha1, t).map_err(|e| format!("map {i}: {e}"))?;
        }
        m.build_idents();
        m.typed = crate::types::Types::new(&m);
        Ok(m)
    }

    /// Lays one file over what is loaded: its `[map]` header checked, then
    /// each key it names replaces the entry before.
    fn layer(&mut self, rom_sha1: &str, text: &str) -> Result<(), String> {
        let doc: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
        let head = doc
            .get("map")
            .and_then(|v| v.as_table())
            .ok_or("no [map] header")?;
        let sha = head
            .get("rom_sha1")
            .and_then(|v| v.as_str())
            .ok_or("[map] has no rom_sha1")?;
        if !sha.eq_ignore_ascii_case(rom_sha1) {
            return Err(format!(
                "[map] rom_sha1 {sha} isn't this ROM's ({rom_sha1})"
            ));
        }
        match head.get("format").map(|v| v.as_integer()) {
            None | Some(Some(1)) => {}
            Some(f) => return Err(format!("[map] format {f:?}: only 1 is known")),
        }
        if let Some(src) = head.get("source").and_then(|v| v.as_str()) {
            self.sources.push(src.to_string());
        }
        let mut funcs = BTreeMap::new();
        let mut data = BTreeMap::new();
        let table = |name: &str| doc.get(name).and_then(|v| v.as_table());
        if let Some(t) = table("functions") {
            for (k, v) in t {
                funcs.insert(key(k)?, item(v, &format!("[functions] {k}"))?);
            }
        }
        if let Some(t) = table("copies") {
            for (k, v) in t {
                let fs = v.get("functions").and_then(|v| v.as_array());
                for f in fs.into_iter().flatten() {
                    let at = format!("[copies] {k}");
                    let fk = f
                        .get("key")
                        .and_then(|v| v.as_integer())
                        .ok_or_else(|| format!("{at}: a function with no key"))?;
                    // `[functions]` says more (size, file) where it has the key.
                    funcs.entry(fk as u32).or_insert(item(f, &at)?);
                }
            }
        }
        if let Some(t) = table("data") {
            for (k, v) in t {
                data.insert(key(k)?, item(v, &format!("[data] {k}"))?);
            }
        }
        // One name on two addresses: only statics of two different files.
        let mut seen: BTreeMap<(&str, Option<&str>), (Kind, u32)> = BTreeMap::new();
        let all = funcs
            .iter()
            .map(|(&k, i)| (Kind::Func, k, i))
            .chain(data.iter().map(|(&k, i)| (Kind::Data, k, i)));
        for (kind, k, i) in all {
            if let Some(&(pk, pa)) = seen.get(&(i.name.as_str(), i.file.as_deref())) {
                return Err(format!(
                    "{} is on both {pk:?} 0x{pa:08x} and {kind:?} 0x{k:08x}{}",
                    i.name,
                    i.file
                        .as_deref()
                        .map_or(String::new(), |f| format!(", both in {f}"))
                ));
            }
            seen.insert((i.name.as_str(), i.file.as_deref()), (kind, k));
        }
        if let Some(t) = table("locals") {
            for (k, v) in t {
                let at = format!("[locals] {k}");
                let ls = v
                    .as_array()
                    .ok_or_else(|| format!("{at}: not an array"))?
                    .iter()
                    .map(|l| local(l, &at))
                    .collect::<Result<Vec<_>, _>>()?;
                self.locals.insert(key(k)?, ls);
            }
        }
        if let Some(t) = table("lines") {
            for (k, v) in t {
                self.lines
                    .insert(key(k)?, line(v, &format!("[lines] {k}"))?);
            }
        }
        if let Some(t) = table("types") {
            for (k, v) in t {
                self.types
                    .insert(k.clone(), type_def(v, &format!("[types] {k:?}"))?);
            }
        }
        if let Some(t) = table("values") {
            for (k, v) in t {
                self.values
                    .insert(k.clone(), values(v, &format!("[values] {k:?}"))?);
            }
        }
        if let Some(v) = doc.get("text") {
            self.text = Some(crate::text::Charmap::new(crate::text::parse_entries(v)?));
        }
        self.functions.extend(funcs);
        self.data.extend(data);
        Ok(())
    }

    /// Every name made an identifier naming one key: keywords and the
    /// printer's and prelude's names get a trailing `_`; a name on more than
    /// one key gets `__<file stem>`, and the address too if that's not
    /// enough. Other characters become `_`; a name with nothing left names
    /// nothing.
    fn build_idents(&mut self) {
        let reserved = reserved();
        let items: Vec<(Kind, u32, &Item)> = self
            .functions
            .iter()
            .map(|(&k, i)| (Kind::Func, k, i))
            .chain(self.data.iter().map(|(&k, i)| (Kind::Data, k, i)))
            .collect();
        let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (n, (_, _, i)) in items.iter().enumerate() {
            if let Some(id) = as_ident(&i.name) {
                by_name.entry(id).or_default().push(n);
            }
        }
        let mut idents: BTreeMap<(Kind, u32), String> = BTreeMap::new();
        for (name, ns) in &by_name {
            if let [n] = ns[..] {
                let (kind, k, _) = items[n];
                let id = if reserved.contains(name) || printer_pattern(name) {
                    format!("{name}_")
                } else {
                    name.to_string()
                };
                idents.insert((kind, k), id);
                continue;
            }
            let mut by_stem: BTreeMap<Option<String>, Vec<usize>> = BTreeMap::new();
            for &n in ns {
                by_stem
                    .entry(items[n].2.file.as_deref().map(stem))
                    .or_default()
                    .push(n);
            }
            for (st, group) in by_stem {
                for &n in &group {
                    let (kind, k, _) = items[n];
                    let mut id = format!("{name}__{}", st.as_deref().unwrap_or(""));
                    if st.is_none() || group.len() > 1 {
                        if st.is_some() {
                            id.push('_');
                        }
                        id.push_str(&format!("{k:08x}"));
                    }
                    idents.insert((kind, k), id);
                }
            }
        }
        // A qualified or `_` form can still meet a name as given (`Foo_`
        // given, `Foo` a keyword): the later key by address takes its
        // address too.
        let mut keys: BTreeMap<String, (Kind, u32)> = BTreeMap::new();
        for (&(kind, k), id) in &idents {
            let mut id = id.clone();
            if keys.contains_key(&id) {
                id = format!("{id}__{k:08x}");
            }
            assert!(
                keys.insert(id, (kind, k)).is_none(),
                "identifier collision at 0x{k:08x}"
            );
        }
        self.idents = keys.iter().map(|(id, &kk)| (kk, id.clone())).collect();
        self.keys = keys;
        self.reserved = reserved;
    }

    /// A DWARF local's name as a Rust local of the readable form
    /// (`docs/mod-view-design.md`, "Naming locals"): `name_` where the
    /// name is reserved, a flag's key in `regs!`, a printer's numbered
    /// name, a function's or datum's identifier or a value's name (a call
    /// names its callee bare, and `base::globals::*` brings each typed
    /// datum and named value in as a const, which a `let` would match
    /// instead of binding). `None` if it can't be
    /// made one that is none of those.
    pub fn local_ident(&self, name: &str) -> Option<String> {
        // `regs!`' flag keys (`n = …, c = …`). The readable macros' own
        // locals (`i`, `vs`) are reserved for the map's items, which a
        // macro's pattern would match, but a `let` is hygienic.
        let taken = |s: &str| {
            ["n", "z", "c", "v"].contains(&s)
                || self.reserved.contains(s) && !["i", "vs"].contains(&s)
                || printer_pattern(s)
                || self.key_of(s).is_some()
                || self.typed.value_named(s).is_some()
        };
        let id = as_ident(name)?;
        if !taken(&id) {
            return Some(id);
        }
        let id = format!("{id}_");
        (!taken(&id)).then_some(id)
    }

    /// The identifier for `key` in `kind`'s table, if it has one.
    pub fn ident(&self, kind: Kind, key: u32) -> Option<&str> {
        self.idents.get(&(kind, key)).map(String::as_str)
    }

    /// The key an identifier names: the table's inverse.
    pub fn key_of(&self, ident: &str) -> Option<(Kind, u32)> {
        self.keys.get(ident).copied()
    }

    /// The data `x` lands on or in, as its identifier and `x`'s offset:
    /// the entry at or before `x`, if `x` is inside its size (an entry
    /// with no size holds only its own address).
    pub fn data_at(&self, x: u32) -> Option<(&str, u32)> {
        let (&k, item) = self.data.range(..=x).next_back()?;
        let off = x - k;
        if u64::from(off) >= item.size.unwrap_or(0).max(1) {
            return None;
        }
        Some((self.ident(Kind::Data, k)?, off))
    }

    /// The function `key`'s identifier: its name if it has one, else
    /// [`crate::lower::fn_name`]'s `f_<key>`.
    pub fn fn_ident(&self, key: u32) -> proc_macro2::Ident {
        match self.ident(Kind::Func, key) {
            Some(id) => quote::format_ident!("{id}"),
            None => crate::lower::fn_name(key),
        }
    }

    /// Every identifier and its key.
    pub fn idents(&self) -> impl Iterator<Item = (&str, Kind, u32)> {
        self.keys
            .iter()
            .map(|(id, &(kind, k))| (id.as_str(), kind, k))
    }
}

/// The map files in `dir` (`*.toml`) whose `[map]` header names the ROM
/// whose sha1 is `rom_sha1`, by file name: the order they layer in. A
/// missing `dir` gives none.
pub fn files_for(dir: &Path, rom_sha1: &str) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    // No directory (no `just names` run on this machine) means no names.
    let entries = match std::fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        r => r?,
    };
    for e in entries {
        let p = e?.path();
        if p.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let text = std::fs::read_to_string(&p)?;
        // The header alone: from `[map]` to the next table.
        let Some(at) = text.find("[map]") else {
            continue;
        };
        let head = &text[at + 5..];
        let head = &head[..head.find("\n[").unwrap_or(head.len())];
        let sha = head.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == "rom_sha1").then(|| v.trim().trim_matches('"').to_string())
        });
        if sha.is_some_and(|s| s.eq_ignore_ascii_case(rom_sha1)) {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

/// The readable list (`gbaa-recompile --readable FILE`,
/// `docs/mod-view-design.md`, "The readable form"): one function a line,
/// by key (`0x0806e415`, `f_0806e415`) or by its identifier in `names`;
/// `#` starts a comment; a line `*` lists every function in `all`. The
/// keys, or the first line naming no function.
pub fn readable_list(
    text: &str,
    names: Option<&SourceMap>,
    all: &[u32],
) -> Result<BTreeSet<u32>, String> {
    let mut keys = BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        let w = line.split('#').next().unwrap_or("").trim();
        if w.is_empty() {
            continue;
        }
        if w == "*" {
            keys.extend(all);
            continue;
        }
        let hex = w.strip_prefix("0x").or_else(|| w.strip_prefix("f_"));
        let key = match hex.map(|h| u32::from_str_radix(h, 16)) {
            Some(Ok(k)) => Some(k),
            _ => names.and_then(|m| match m.key_of(w) {
                Some((Kind::Func, k)) => Some(k),
                _ => None,
            }),
        };
        keys.insert(key.ok_or_else(|| format!("line {}: no function {w:?}", n + 1))?);
    }
    Ok(keys)
}

pub mod check;

#[cfg(test)]
mod tests;
