//! A lift's mod copy (unit `native/lift-mod-cut`; the mod view's,
//! [`crate::modcopy`], for the lift): `gbaa-lift ROM OUT --cut DIR KEY...`
//! writes the chosen functions' printed text, names and all, to DIR as
//! ordinary Rust an editor edits, with a replace file that
//! `gbaa-lift --replace DIR/replace.toml` builds with.
//!
//! DIR holds `replace.toml` (the ROM's sha1, the gbaa commit at the last
//! cut, and a `[[cut]] entry, name, file` per function), `src/<name>.rs`
//! (the function's whole text as the crate prints it: its entries, its
//! body; edited freely) and `orig/<name>.rs` (the same, never edited, for
//! a three-way merge when a re-cut prints it differently: after a
//! printer or names change, `--cut DIR` again merges the new print into
//! the edits with `git merge-file`). A cut takes the place of the
//! function's text in the crate, after the names pass; its callers are
//! printed as for the lift, so it keeps the lifted signature.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The first lines of every file a cut writes: what the text an editor
/// meets is, and what must stay as cut. [`head`] adds a line for each
/// checked helper ([`HELPERS`]) the cut uses, then [`HEAD_END`].
pub const HEAD: &str = "// Cut by `gbaa-lift --cut` (crates/codegen/src/native/cut.rs): edit freely;\n\
// build with `gbaa-lift ROM OUT --replace DIR/replace.toml`. orig/ keeps\n\
// the print as cut, for a re-cut's three-way merge.\n\
//\n\
// - Keep each `pub fn` line's name, parameters and return type as cut:\n\
//   the rest of the lift calls it so. `g` is the game (RAM `g.ram.<global>`,\n\
//   `g.get`/`g.set` through a pointer); `r13` is the guest's sp; a `-> u32`\n\
//   returns r0. The `x_<key>`/`a_<key>` lines are the other entries'.\n\
// - A function that can't stop returns its value bare; one that can,\n\
//   `Result<_, Exit>` (`?` after a call that can). Keep that as cut.\n\
// - `lN` is the stack slot N bytes below the entry sp; `vN` a value\n\
//   (one the lift could tell holds a global, field or variable is named so);\n\
//   `RET` the lr a call leaves (a return's marker); `bank_sp(g, r13)`\n\
//   comes before what may run a handler.\n";

/// The header's last line, after the helpers'.
const HEAD_END: &str = "// - Call another function by its Rust name, as the lift does.\n\n";

/// The helpers an edit may meet that the header explains when the cut
/// uses them (unit `native/lift-cut-header-helpers`): the name called,
/// and its line.
pub const HELPERS: [(&str, &str); 4] = [
    (
        "pin",
        "// - `pin(p, 0x…)` checks that pointer `p` is the global at that RAM offset\n\
//   (the field next to it is read or written by name, not through `p`); it\n\
//   panics if not, and doesn't change with the value. Keep it with its access.\n",
    ),
    (
        "ix",
        "// - `G[ix(p, 0x…, e, n)]` is the element of RAM global `G` (at that offset,\n\
//   `n` of `e` bytes) pointer `p` points to; it panics outside it.\n",
    ),
    (
        "rix",
        "// - `R[rix(p, 0x…, e, n)]` is the same for a ROM static from that guest\n\
//   address: `p`'s element of `R`, checked.\n",
    ),
    (
        "prim::sext",
        "// - `prim::sext(x, n)` sign-extends the low `n` bits of `x` (a signed\n\
//   byte or halfword).\n",
    ),
];

/// Whether `body` calls `name` (as `name(`, not part of a longer name:
/// `hw::spin(` isn't `pin(`, `rix(` isn't `ix(`).
fn calls(body: &str, name: &str) -> bool {
    let pat = format!("{name}(");
    body.match_indices(&pat).any(|(i, _)| {
        !body[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':')
    })
}

/// The header for a cut of `body`: [`HEAD`], a line for each of
/// [`HELPERS`] it calls, then the last line.
pub fn head(body: &str) -> String {
    let mut h = String::from(HEAD);
    for (name, line) in HELPERS {
        if calls(body, name) {
            h += line;
        }
    }
    h + HEAD_END
}

/// `body` as a cut file: its [`head`], then it.
pub fn with_head(body: &str) -> String {
    head(body) + body
}

/// A cut's `replace.toml`.
#[derive(Debug, Default, PartialEq)]
pub struct CutMod {
    /// The ROM it was cut from.
    pub rom_sha1: String,
    /// The gbaa commit at the last cut.
    pub gbaa: String,
    /// The functions it replaces: key, and the name its files have.
    pub funcs: BTreeMap<u32, String>,
}

impl CutMod {
    /// `dir`'s `replace.toml`, if it has one.
    pub fn load(dir: &Path) -> Result<Option<CutMod>, String> {
        let path = dir.join("replace.toml");
        if !path.exists() {
            return Ok(None);
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        CutMod::parse(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    fn parse(text: &str) -> Result<CutMod, String> {
        let doc: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
        let head = doc
            .get("mod")
            .and_then(|v| v.as_table())
            .ok_or("no [mod]")?;
        let s = |k: &str| head.get(k).and_then(|v| v.as_str()).map(String::from);
        let mut funcs = BTreeMap::new();
        for e in doc
            .get("cut")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let entry = e
                .get("entry")
                .and_then(|v| v.as_integer())
                .ok_or("cut: no entry")? as u32;
            let name = e
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("cut: no name")?;
            funcs.insert(entry, name.to_string());
        }
        Ok(CutMod {
            rom_sha1: s("rom_sha1").ok_or("[mod] has no rom_sha1")?,
            gbaa: s("gbaa").unwrap_or_default(),
            funcs,
        })
    }

    /// The file's text: a holes file `gbaa-lift --replace` reads.
    pub fn text(&self) -> String {
        let q = |s: &str| toml::Value::String(s.into()).to_string();
        let mut t = String::from(
            "# A lift's mod copy (crates/codegen/src/native/cut.rs). Written by\n\
             # `gbaa-lift ROM OUT --cut DIR KEY...`; build with `gbaa-lift ROM OUT --replace DIR/replace.toml`.\n\n[mod]\n",
        );
        t += &format!("rom_sha1 = {}\n", q(&self.rom_sha1));
        t += &format!("gbaa = {}\n", q(&self.gbaa));
        for (k, n) in &self.funcs {
            t += &format!(
                "\n[[cut]]\nentry = 0x{k:08x}\nname = {}\nfile = {}\n",
                q(n),
                q(&format!("src/{n}.rs"))
            );
        }
        t
    }
}

/// The name a function's printed text defines it by: its first `pub fn`
/// that isn't an `x_` or `a_` entry (`tally`, or `f_<key>` unnamed).
pub fn name_of(text: &str, key: u32) -> String {
    let other =
        |n: &str| n == format!("x_{key:08x}") || n == format!("a_{key:08x}") || n.is_empty();
    text.lines()
        .filter_map(|l| l.strip_prefix("pub fn "))
        .map(|l| {
            let end = l
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(l.len());
            &l[..end]
        })
        .find(|n| !other(n))
        .map_or_else(|| format!("f_{key:08x}"), str::to_string)
}

/// A cut as the per-function check's replacement (`gbaa-lift --replace
/// DIR/replace.toml --fn-check KEY`, unit `native/lift-mod-run`): the cut's
/// own function (the `pub fn` [`name_of`] names, to its closing `}`; its
/// `x_`/`a_` entries left out, the reference has its own) renamed
/// `cut_<name>`, with what its signature says: that name, how many
/// parameters it takes after `g` (`sp`, then the shape's registers), and
/// whether it returns a `Result`.
#[derive(Debug, PartialEq)]
pub struct Replacement {
    pub name: String,
    pub text: String,
    pub params: usize,
    pub result: bool,
}

impl Replacement {
    pub fn of(text: &str, key: u32) -> Result<Replacement, String> {
        let name = name_of(text, key);
        let head = format!("pub fn {name}(");
        let mut lines = text.lines().skip_while(|l| !l.starts_with(&head));
        let sig = lines
            .next()
            .ok_or_else(|| format!("the cut of {name} has no `{head}` line"))?;
        let mut body = vec![sig.replacen(&head, &format!("pub fn cut_{name}("), 1)];
        let mut closed = false;
        for l in lines {
            body.push(l.to_string());
            if l == "}" {
                closed = true;
                break;
            }
        }
        if !closed {
            return Err(format!("the cut of {name}: no `}}` line ends {head}…"));
        }
        // The parameter list, to its matching `)`.
        let open = head.len();
        let mut depth = 1;
        let close = sig[open..]
            .char_indices()
            .find(|&(_, c)| {
                depth += match c {
                    '(' | '[' | '<' => 1,
                    ')' | ']' | '>' => -1,
                    _ => 0,
                };
                depth == 0
            })
            .map(|(i, _)| open + i)
            .ok_or_else(|| format!("the cut of {name}: `{sig}` has no `)`"))?;
        let mut params = 0usize;
        let mut depth = 0;
        let mut any = false;
        for c in sig[open..close].chars() {
            match c {
                '(' | '[' | '<' => depth += 1,
                ')' | ']' | '>' => depth -= 1,
                ',' if depth == 0 => params += 1,
                c if !c.is_whitespace() => any = true,
                _ => {}
            }
        }
        // `g` is the first; the commas count the rest.
        let params = if any { params } else { 0 };
        let result = sig[close..]
            .trim_start_matches(')')
            .trim_start()
            .strip_prefix("->")
            .is_some_and(|r| r.trim_start().starts_with("Result<"));
        Ok(Replacement {
            name: format!("cut_{name}"),
            text: body.join("\n") + "\n",
            params,
            result,
        })
    }
}

/// What a cut did.
#[derive(Debug, Default)]
pub struct Cut {
    /// The functions written fresh (no edit to keep), by name.
    pub fresh: Vec<String>,
    /// The edited functions whose new print was merged in, by name, with
    /// the conflicts `git merge-file` left (0: none).
    pub merged: Vec<(String, usize)>,
}

/// Cut `texts` (by key, each function's printed text as lifted) into the
/// mod at `dir`, adding them to its `replace.toml` (made if new). A
/// function whose `src/` file was edited since its last cut gets the new
/// print three-way merged in (`git merge-file src orig new`); one renamed
/// has its files renamed first. `gbaa`: the commit cut at.
pub fn cut(
    dir: &Path,
    texts: &BTreeMap<u32, String>,
    rom_sha1: &str,
    gbaa: &str,
) -> Result<Cut, String> {
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let mut m = match CutMod::load(dir)? {
        Some(m) if !m.rom_sha1.eq_ignore_ascii_case(rom_sha1) => {
            return Err(format!(
                "{}: cut from ROM {}, not this one ({rom_sha1})",
                dir.display(),
                m.rom_sha1
            ));
        }
        Some(m) => m,
        None => CutMod {
            rom_sha1: rom_sha1.to_string(),
            ..CutMod::default()
        },
    };
    let (src, orig) = (dir.join("src"), dir.join("orig"));
    for d in [&src, &orig] {
        std::fs::create_dir_all(d).map_err(|e| io(d, e))?;
    }
    let mut out = Cut::default();
    for (&k, body) in texts {
        let name = name_of(body, k);
        let file = format!("{name}.rs");
        if let Some(old) = m.funcs.get(&k).filter(|o| **o != name) {
            for d in [&src, &orig] {
                let p = d.join(format!("{old}.rs"));
                if p.exists() {
                    std::fs::rename(&p, d.join(&file)).map_err(|e| io(&p, e))?;
                }
            }
        }
        let text = with_head(body);
        let (s, o) = (src.join(&file), orig.join(&file));
        let edited = match (std::fs::read(&s), std::fs::read(&o)) {
            (Ok(a), Ok(b)) => a != b,
            _ => false,
        };
        if edited {
            let new = dir.join(format!(".new-{file}"));
            std::fs::write(&new, &text).map_err(|e| io(&new, e))?;
            let c = crate::modcopy::merge_file(&s, &o, &new);
            let _ = std::fs::remove_file(&new);
            out.merged.push((name.clone(), c?));
        } else {
            std::fs::write(&s, &text).map_err(|e| io(&s, e))?;
            out.fresh.push(name.clone());
        }
        std::fs::write(&o, &text).map_err(|e| io(&o, e))?;
        m.funcs.insert(k, name);
    }
    m.gbaa = gbaa.to_string();
    let p = dir.join("replace.toml");
    std::fs::write(&p, m.text()).map_err(|e| io(&p, e))?;
    Ok(out)
}

/// The cuts' `orig/` copies by key: what [`stale`] compares.
pub type Origs = BTreeMap<u32, PathBuf>;

/// The cut files of a replace file at `path` (`[[cut]]`'s `file`, read
/// relative to it), by key, into `holes.cuts`; and the ones whose `orig/`
/// copy is there, by key, with its path, for [`stale`].
pub fn read_files(
    path: &Path,
    files: &BTreeMap<u32, String>,
) -> Result<(BTreeMap<u32, String>, Origs), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut cuts = BTreeMap::new();
    let mut origs = BTreeMap::new();
    for (&k, f) in files {
        let p = dir.join(f);
        let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        cuts.insert(k, t);
        let o = p
            .parent()
            .and_then(Path::parent)
            .zip(p.file_name())
            .map(|(d, n)| d.join("orig").join(n));
        if let Some(o) = o.filter(|o| o.exists()) {
            origs.insert(k, o);
        }
    }
    Ok((cuts, origs))
}

/// The cuts whose `orig/` print isn't what this lift prints (`lifted`, by
/// key, [`super::Lifted::cut_texts`]): a re-cut would merge a change in.
pub fn stale(origs: &Origs, lifted: &BTreeMap<u32, String>) -> Vec<u32> {
    origs
        .iter()
        .filter(|(k, o)| {
            let Some(t) = lifted.get(k) else {
                return false;
            };
            std::fs::read_to_string(o).ok() != Some(with_head(t))
        })
        .map(|(&k, _)| k)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TALLY: &str = "pub fn x_080004fd(g: &mut Game, s: Regs) -> u32 { tally(g, s.r[13], s.r[0]) }\n\
                         pub fn tally(g: &mut Game, mut r13: u32, mut n: u32) -> u32 {\n    \
                         let v4: u32 = n;\n    g.ram.gTally = 0x82u32;\n    \
                         // c/switch/main.c:222\n    let v5: u32 = g.ram.gFrame;\n    return v5;\n}\n\n";

    /// The header explains a checked helper only where the cut calls it
    /// (and `hw::spin(` isn't `pin(`, nor `rix(` `ix(`).
    #[test]
    fn the_header_explains_the_helpers_the_cut_uses() {
        let plain = head(TALLY);
        assert!(plain.starts_with(HEAD) && plain.ends_with(HEAD_END));
        assert_eq!(plain.len(), HEAD.len() + HEAD_END.len());
        let body = "pin(v3, 0x1c0); g.ram.gX.a = v4;\nlet v5 = ROM_T[rix(v2, 0x80001000, 4, 8)];\n\
                    let v6 = prim::sext(v5, 8); hw::spin(g)?;\n";
        let h = head(body);
        assert!(h.contains("// - `pin(p, 0x…)`"));
        assert!(h.contains("// - `R[rix("));
        assert!(h.contains("// - `prim::sext("));
        assert!(!h.contains("// - `G[ix("));
        let h = head("let v1 = g.ram.gT[ix(v0, 0x40, 2, 16)];\n");
        assert!(h.contains("// - `G[ix(") && !h.contains("rix(") && !h.contains("`pin("));
        assert_eq!(with_head("x"), format!("{plain}x"));
    }

    #[test]
    fn a_functions_name_is_its_entrys() {
        assert_eq!(name_of(TALLY, 0x0800_04fd), "tally");
        assert_eq!(
            name_of("pub fn f_08000100(g: &mut Game) {}\n", 0x0800_0100),
            "f_08000100"
        );
        assert_eq!(
            name_of(
                "/// Never returns.\npub fn a_08000575(g) {}\npub fn x_08000575(g) {}\npub fn AgbMain(g) {}\n",
                0x0800_0575
            ),
            "AgbMain"
        );
    }

    #[test]
    fn a_cut_as_a_replacement_is_its_own_function_renamed() {
        let text = "pub fn x_080004fd(g: &mut Game, s: Regs) -> () { tally(g, s.r[13], s.r[0]) }\n\
                    pub fn tally(g: &mut Game, _: u32, mut n: u32) -> () {\n    n += 1;\n}\n";
        let r = super::Replacement::of(text, 0x0800_04fd).unwrap();
        assert_eq!(r.name, "cut_tally");
        assert_eq!(
            r.text,
            "pub fn cut_tally(g: &mut Game, _: u32, mut n: u32) -> () {\n    n += 1;\n}\n"
        );
        assert_eq!((r.params, r.result), (2, false));
        let text = "pub fn f_08000100(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {\n}\n";
        let r = super::Replacement::of(text, 0x0800_0100).unwrap();
        assert_eq!(
            (r.name.as_str(), r.params, r.result),
            ("cut_f_08000100", 2, true)
        );
        assert!(super::Replacement::of("pub fn tally(g: &mut Game) -> () {\n", 1).is_err());
    }

    #[test]
    fn replace_toml_round_trips_and_parses_as_a_holes_file() {
        let m = CutMod {
            rom_sha1: "f3ae0881".into(),
            gbaa: "abc123".into(),
            funcs: BTreeMap::from([(0x0800_04fd, "tally".into())]),
        };
        assert_eq!(CutMod::parse(&m.text()), Ok(m));
        let h = super::super::Holes::parse(&CutMod::default().text()).unwrap();
        assert!(h.cut_files.is_empty());
        let m = CutMod {
            rom_sha1: "f3ae0881".into(),
            gbaa: String::new(),
            funcs: BTreeMap::from([(0x0800_04fd, "tally".into())]),
        };
        let h = super::super::Holes::parse(&m.text()).unwrap();
        assert_eq!(h.cut_files[&0x0800_04fd], "src/tally.rs");
        assert_eq!(h.cut_rom.as_deref(), Some("f3ae0881"));
    }

    /// A cut, an edit of one constant, a re-cut after the printer changed
    /// a line away from it: the edit stays, the change comes in, no
    /// conflict; orig/ is the new print, and a re-cut with no change is
    /// clean.
    #[test]
    fn a_re_cut_merges_a_printer_change_into_an_edit() {
        let dir = std::env::temp_dir().join(format!("gbaa-lift-cut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let k = 0x0800_04fd;
        let c = cut(&dir, &BTreeMap::from([(k, TALLY.to_string())]), "aa", "c1").unwrap();
        assert_eq!(c.fresh, ["tally"]);
        let s = dir.join("src/tally.rs");
        let t = std::fs::read_to_string(&s).unwrap();
        std::fs::write(&s, t.replace("0x82u32", "0x40u32")).unwrap();
        let printed = TALLY.replace(
            "let v5: u32 = g.ram.gFrame;\n    return v5;",
            "return g.ram.gFrame;",
        );
        let c = cut(&dir, &BTreeMap::from([(k, printed.clone())]), "aa", "c2").unwrap();
        assert_eq!(c.merged, [("tally".to_string(), 0)]);
        let got = std::fs::read_to_string(&s).unwrap();
        assert_eq!(got, with_head(&printed.replace("0x82u32", "0x40u32")));
        let orig = std::fs::read_to_string(dir.join("orig/tally.rs")).unwrap();
        assert_eq!(orig, with_head(&printed));
        let m = CutMod::load(&dir).unwrap().unwrap();
        assert_eq!(m.gbaa, "c2");
        let (cuts, origs) = read_files(
            &dir.join("replace.toml"),
            &BTreeMap::from([(k, "src/tally.rs".into())]),
        )
        .unwrap();
        assert_eq!(cuts[&k], got);
        assert!(stale(&origs, &BTreeMap::from([(k, printed)])).is_empty());
        assert_eq!(
            stale(&origs, &BTreeMap::from([(k, TALLY.to_string())])),
            [k]
        );
        assert!(cut(&dir, &BTreeMap::new(), "bb", "c3").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
