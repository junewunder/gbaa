//! A mod copy (`docs/mod-view-design.md`, "A mod copy"): functions cut out
//! of the generator into `mods/<game>/<mod>/` as ordinary Rust, and the
//! `modded` part crate a build with `gbaa-recompile --mod` compiles them in.
//!
//! The mod's directory holds `mod.toml` (the ROM's sha1, the gbaa commit
//! and the source map's `source` at the cut, and the functions by key and
//! name), `src/<Name>.rs` (one per function, printed readable and named,
//! exported under its function's symbol, edited freely) and `orig/<Name>.rs`
//! (the same prints, never edited, for a three-way merge when a re-cut
//! prints them differently).

use crate::emit::{Files, GENERATED, PACKAGE, part_lib, pretty, print_fn};
use crate::func::{Ctx, Func, Insn};
use crate::names::{Kind, SourceMap};
use quote::quote;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A mod's `mod.toml`.
#[derive(Debug, Default, PartialEq)]
pub struct Mod {
    /// Its directory, `mods/<game>/<mod>`.
    pub dir: PathBuf,
    /// The ROM it was cut from.
    pub rom_sha1: String,
    /// The gbaa commit at the last cut.
    pub gbaa: String,
    /// The source map's `[map] source` lines at the last cut.
    pub map_source: Vec<String>,
    /// The functions it replaces: key, and the name its file has.
    pub funcs: BTreeMap<u32, String>,
}

/// The first lines of every file a cut writes: what the numbers an editor
/// meets are, and what to do with them (`docs/mod-view-design.md`,
/// "Editing a cut", says why).
const HEAD: &str = "// Cut by `gbaa-recompile --cut` (docs/mod-view-design.md, \"A mod copy\"):\n\
// edit freely. orig/ keeps the print as cut, for a re-cut's merge.\n\
//\n\
// The numbers in it (\"Editing a cut\" there says why):\n\
// - `m.get(at, ..)`, `m.set(at, ..)`, `m.get_via(pat, lit, at, ..)`: `at` is\n\
//   the guest instruction's address, which prices the prefetch stall after\n\
//   the access; `lit` is the pool word that holds the address. An access you\n\
//   add takes its neighbour's `at` (and `pat` and `lit`).\n\
// - `ret!(m, pc @ at, exit, cost, { regs!(..) })`, `tick!` and `call!`: `at`\n\
//   is the exit's address, `cost` the cycles of the code before it as cut.\n\
//   Leave both as cut: an edit isn't expected to keep exact timing.\n\
// - `call!(m, pc, f -> ret, ..)`: `ret` is the return site, which the call\n\
//   writes to lr; `f`'s entry key is `keys::f`. Keep `ret` as cut.\n\
// - A call you add is `call!(m, pc, f, { regs!(m; 0 = arg, ..) })`, with no\n\
//   return site. Its value is the callee's r0 (`let v = call!(..);`); it\n\
//   leaves the registers as it found them. Put it first in its block (at\n\
//   the top, or just after a `tick!` or `call!`): a state loaded inside the\n\
//   call runs the block again from there.\n\
// - `regs!(m; 0 = v, ..; n = ..)`: the registers and flags left for the\n\
//   caller. r0 is a return value and r4-r11 and sp must stay as cut; the\n\
//   rest only keep states identical to stock, so if you delete a value they\n\
//   name, give them any value (0).\n\n";

impl Mod {
    /// `dir`'s `mod.toml`.
    pub fn load(dir: &Path) -> Result<Mod, String> {
        let path = dir.join("mod.toml");
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Mod::parse(dir, &text).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn parse(dir: &Path, text: &str) -> Result<Mod, String> {
        let doc: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
        let head = doc
            .get("mod")
            .and_then(|v| v.as_table())
            .ok_or("no [mod]")?;
        let s = |k: &str| head.get(k).and_then(|v| v.as_str()).map(String::from);
        let mut funcs = BTreeMap::new();
        if let Some(t) = doc.get("functions").and_then(|v| v.as_table()) {
            for (k, v) in t {
                let key = k
                    .strip_prefix("0x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .ok_or_else(|| format!("[functions] {k}: not a hex key"))?;
                let name = v
                    .as_str()
                    .ok_or_else(|| format!("[functions] {k}: not a name"))?;
                funcs.insert(key, name.to_string());
            }
        }
        let map_source = head
            .get("map_source")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        Ok(Mod {
            dir: dir.to_path_buf(),
            rom_sha1: s("rom_sha1").ok_or("[mod] has no rom_sha1")?,
            gbaa: s("gbaa").unwrap_or_default(),
            map_source,
            funcs,
        })
    }

    fn text(&self) -> String {
        let mut t = String::from(
            "# A mod copy (docs/mod-view-design.md, \"A mod copy\"). Written by\n\
             # `gbaa-recompile --cut`; build it with `gbaa-recompile ROM OUT --mod DIR`.\n\n[mod]\n",
        );
        let q = |s: &str| toml::Value::String(s.into()).to_string();
        t += &format!("rom_sha1 = {}\n", q(&self.rom_sha1));
        t += &format!("gbaa = {}\n", q(&self.gbaa));
        let srcs: Vec<String> = self.map_source.iter().map(|s| q(s)).collect();
        t += &format!("map_source = [{}]\n\n", srcs.join(", "));
        t += "# The functions it replaces, by key: each is src/<name>.rs.\n[functions]\n";
        for (k, n) in &self.funcs {
            t += &format!("0x{k:08x} = {}\n", q(n));
        }
        t
    }
}

/// `f` as a cut prints it: its own file, exported by symbol, with why its
/// registers stay in `Machine` if they do.
pub fn fn_file(ctx: &Ctx, f: &Func) -> (String, Option<String>) {
    let (tokens, why) = print_fn(ctx, f, true);
    // A named print's typed accesses name `base`'s globals, which the mod's
    // `lib.rs` leaves out: its own files may define the same names.
    let globals = ctx.names.map(|_| {
        quote!(
            use base::globals::*;
        )
    });
    let file = quote! {
        #[allow(unused_imports)]
        use super::*;
        use crate::prelude::*;
        #globals
        #tokens
    };
    (format!("{HEAD}{}", pretty(file)), why)
}

/// What a cut did.
#[derive(Debug, Default)]
pub struct Cut {
    /// The functions written fresh (no edit to keep), by name.
    pub fresh: Vec<String>,
    /// The edited functions whose new print was merged in, by name, with
    /// the conflicts `git merge-file` left (0: none).
    pub merged: Vec<(String, usize)>,
    /// The functions whose registers stay in `Machine`, with why.
    pub unchecked: Vec<(u32, String)>,
}

/// Cut `funcs`, printed in `ctx`, into the mod at `dir`, adding them to
/// its `mod.toml` (made if new). A function whose `src/` file was edited
/// since its last cut gets the new print three-way merged in (`git
/// merge-file src orig new`); one renamed in the map has its files renamed
/// first. `gbaa` is the commit cut at.
pub fn cut(
    ctx: &Ctx,
    funcs: &[&Func],
    dir: &Path,
    rom_sha1: &str,
    gbaa: &str,
) -> Result<Cut, String> {
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let mut m = if dir.join("mod.toml").exists() {
        let m = Mod::load(dir)?;
        if !m.rom_sha1.eq_ignore_ascii_case(rom_sha1) {
            return Err(format!(
                "{}: cut from ROM {}, not this one ({rom_sha1})",
                dir.display(),
                m.rom_sha1
            ));
        }
        m
    } else {
        Mod {
            dir: dir.to_path_buf(),
            rom_sha1: rom_sha1.to_string(),
            ..Mod::default()
        }
    };
    let (src, orig) = (dir.join("src"), dir.join("orig"));
    for d in [&src, &orig] {
        std::fs::create_dir_all(d).map_err(|e| io(d, e))?;
    }
    let mut out = Cut::default();
    for f in funcs {
        let name = ctx.fn_ident(f.entry).to_string();
        let file = format!("{name}.rs");
        if let Some(old) = m.funcs.get(&f.entry).filter(|o| **o != name) {
            for d in [&src, &orig] {
                let p = d.join(format!("{old}.rs"));
                if p.exists() {
                    std::fs::rename(&p, d.join(&file)).map_err(|e| io(&p, e))?;
                }
            }
        }
        let (text, why) = fn_file(ctx, f);
        out.unchecked.extend(why.map(|w| (f.entry, w)));
        let (s, o) = (src.join(&file), orig.join(&file));
        let edited = match (std::fs::read(&s), std::fs::read(&o)) {
            (Ok(a), Ok(b)) => a != b,
            _ => false,
        };
        if edited {
            // A cut from before `insn!` left the readable form: its markers
            // in `src/` and `orig/` become the new print's (none, or
            // comments) first, or an edit next to one would conflict.
            let insn = ctx.cfg_for(f.entry).insn;
            for p in [&s, &o] {
                let t = std::fs::read_to_string(p).map_err(|e| io(p, e))?;
                let r = recast_insns(&t, insn);
                if r != t {
                    std::fs::write(p, r).map_err(|e| io(p, e))?;
                }
            }
            let new = dir.join(format!(".new-{file}"));
            std::fs::write(&new, &text).map_err(|e| io(&new, e))?;
            let c = merge_file(&s, &o, &new);
            let _ = std::fs::remove_file(&new);
            out.merged.push((name.clone(), c?));
        } else {
            std::fs::write(&s, &text).map_err(|e| io(&s, e))?;
            out.fresh.push(name.clone());
        }
        std::fs::write(&o, &text).map_err(|e| io(&o, e))?;
        m.funcs.insert(f.entry, name);
    }
    m.gbaa = gbaa.to_string();
    m.map_source = ctx.names.map(|n| n.sources.clone()).unwrap_or_default();
    let p = dir.join("mod.toml");
    std::fs::write(&p, m.text()).map_err(|e| io(&p, e))?;
    Ok(out)
}

/// `git merge-file src orig new`: the change from `orig` to `new` merged
/// into `src` in place. The conflicts it left.
fn merge_file(src: &Path, orig: &Path, new: &Path) -> Result<usize, String> {
    let st = std::process::Command::new("git")
        .args(["merge-file", "-L", "src", "-L", "orig", "-L", "new"])
        .args([src, orig, new])
        .status()
        .map_err(|e| format!("git merge-file: {e}"))?;
    match st.code() {
        Some(c) if c >= 0 => Ok(c as usize),
        _ => Err(format!("git merge-file {}: {st}", src.display())),
    }
}

/// `text` with each `insn!(0x0805_9810, "push {r4, lr}");` statement,
/// alone on its line or wrapped over several, printed as `insn` says:
/// left out (`Off`), as `// 0x0805_9810 push {r4, lr}` (`Comment`, a `bl`'s
/// second half left out), or kept.
fn recast_insns(text: &str, insn: Insn) -> String {
    if matches!(insn, Insn::Asm | Insn::Debug) || !text.contains("insn!(") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let t = line.trim();
        if !t.starts_with("insn!(") {
            out.push_str(line);
            continue;
        }
        let mut stmt = t.to_string();
        while !stmt.ends_with(");") {
            match lines.next() {
                Some(l) => stmt.push_str(l.trim()),
                None => break,
            }
        }
        let (Some(q0), Some(q1)) = (stmt.find('"'), stmt.rfind('"')) else {
            continue;
        };
        let pc = stmt["insn!(".len()..]
            .split(',')
            .next()
            .unwrap_or("")
            .trim();
        let asm = stmt.get(q0 + 1..q1).unwrap_or("");
        if insn == Insn::Comment && asm != "(bl, second half)" {
            let indent = &line[..line.len() - line.trim_start().len()];
            out.push_str(format!("{indent}// {pc} {asm}").trim_end());
            out.push('\n');
        }
    }
    out
}

/// The callees of the `call!`s and `call_any!`s in the mod's `src/*.rs`
/// (their third argument, by name from `names` or as `f_<key>`), by key:
/// a call the mod added may reach a function the cut never called, which
/// the `modded` crate then has to link. Lines that are comments are
/// skipped.
pub fn called(m: &Mod, names: Option<&SourceMap>) -> Result<BTreeSet<u32>, String> {
    let src = m.dir.join("src");
    let rd = std::fs::read_dir(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut out = BTreeSet::new();
    for e in rd {
        let p = e.map_err(|e| format!("{}: {e}", src.display()))?.path();
        if p.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        for id in callees(&text) {
            let key = names
                .and_then(|n| n.key_of(&id))
                .and_then(|(k, key)| (k == Kind::Func).then_some(key))
                .or_else(|| {
                    id.strip_prefix("f_")
                        .filter(|h| h.len() == 8)
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                });
            match key {
                Some(k) => {
                    out.insert(k);
                }
                None => {
                    return Err(format!(
                        "{}: calls {id}, which names no function",
                        p.display()
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// The third arguments of the `call!(` and `call_any!(` in `text`, outside
/// line comments: `call!(m, pc, f -> ret, ..)` and `call!(m, pc, f)` give `f`.
fn callees(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Statements span lines, so a comment ends at its line's end.
    let code: String = text
        .lines()
        .map(|l| l.find("//").map_or(l, |i| &l[..i]))
        .collect::<Vec<_>>()
        .join("\n");
    for pat in ["call!(", "call_any!("] {
        let mut at = 0;
        while let Some(i) = code[at..].find(pat) {
            let start = at + i + pat.len();
            at = start;
            // Only the macro itself, not a longer name ending in it.
            let before = code[..start - pat.len()].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let ident = |t: &str| {
                let t = t.trim();
                t.starts_with(|c: char| c.is_alphabetic() || c == '_')
                    && t.chars().all(|c| c.is_alphanumeric() || c == '_')
            };
            // The generated macro's shape, `m, pc, f`; a mod's own `call!`
            // of another shape (the spike's `call!(g, f @ key -> ret, ..)`)
            // isn't one.
            let mut args = code[start..].splitn(3, ',');
            let (Some(m), Some(pc), Some(rest)) = (args.next(), args.next(), args.next()) else {
                continue;
            };
            let rest = rest.trim_start();
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if ident(m) && ident(pc) && ident(&rest[..end]) {
                out.push(rest[..end].to_string());
            }
        }
    }
    out
}

/// Write the `modded` part crate into `crate_dir/modded`: a part whose
/// modules are the mod's `src/*.rs`, by path, holding `funcs` (the
/// functions the mod replaces, as the generator would print them, for the
/// calls they make into other parts), and linking `added` too ([`called`]).
/// `deps`: the parts' dependencies.
/// Returns the functions whose `orig/` print differs from this
/// generator's (a re-cut would merge a change in).
pub fn write_modded(
    files: &mut Files,
    crate_dir: &Path,
    name: &str,
    m: &Mod,
    funcs: &[(&Ctx, &Func)],
    added: &BTreeSet<u32>,
    deps: &str,
) -> std::io::Result<Vec<String>> {
    let dir = crate_dir.join("modded");
    let src = std::path::absolute(m.dir.join("src"))?;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&src)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    paths.retain(|p| p.extension().is_some_and(|x| x == "rs"));
    paths.sort();
    let mods = paths.iter().enumerate().map(|(i, p)| {
        let id = quote::format_ident!("m{:03}", i);
        let p = p.display().to_string();
        quote! {
            #[path = #p]
            mod #id;
            pub use #id::*;
        }
    });
    let mods = quote!(#(#mods)*);
    files.write(
        &dir.join("src/lib.rs"),
        pretty(part_lib(funcs, added, mods, false)),
    )?;
    files.write(
        &dir.join("Cargo.toml"),
        format!(
            "{GENERATED}[package]\nname = \"gbaa-{name}-modded\"\n{PACKAGE}\n[dependencies]\n{deps}\
             base = {{ package = \"gbaa-{name}-base\", path = \"../base\" }}\n"
        ),
    )?;
    let mut stale = Vec::new();
    for (ctx, f) in funcs {
        let n = &m.funcs[&f.entry];
        let orig = std::fs::read_to_string(m.dir.join("orig").join(format!("{n}.rs")));
        if orig.ok() != Some(fn_file(ctx, f).0) {
            stale.push(n.clone());
        }
    }
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callees_of_a_mods_calls() {
        let t = "    call!(\n        m, pc, InputRecorderRead -> 0x0815_26fd, 8, { regs!(m;; n = a); }\n    );\n\
                 // call!(m, pc, Commented);\n    call!(m, pc, VBlankIntrWait);\n\
                 \x20   call_any!(m, pc, f_08001234 -> 0x0800_0003, 2);\n    my_call!(m, pc, Other);\n\
                 \x20   call!(g, SeedRng @ 0x0806_f5f9 -> 0x0800_056d, 10 + (4, 4), {});\n";
        assert_eq!(
            callees(t),
            ["InputRecorderRead", "VBlankIntrWait", "f_08001234"]
        );
    }

    #[test]
    fn insn_markers_recast_as_the_new_print_has_them() {
        let t = "    // a.c:1\n    insn!(0x0800_0000, \"bl 0x08000008\");\n    \
                 insn!(0x0800_0002, \"(bl, second half)\");\n    insn!(\n        \
                 0x0800_0004,\n        \"ldr r0, [pc, #4]\"\n    );\n    let x = 1;\n";
        assert_eq!(recast_insns(t, Insn::Off), "    // a.c:1\n    let x = 1;\n");
        assert_eq!(
            recast_insns(t, Insn::Comment),
            "    // a.c:1\n    // 0x0800_0000 bl 0x08000008\n    \
             // 0x0800_0004 ldr r0, [pc, #4]\n    let x = 1;\n"
        );
        assert_eq!(recast_insns(t, Insn::Asm), t);
    }

    #[test]
    fn mod_toml_round_trips() {
        let m = Mod {
            dir: PathBuf::from("mods/emerald/x"),
            rom_sha1: "f3ae0881".into(),
            gbaa: "abc123".into(),
            map_source: vec!["pret/pokeemerald@731ad5bf".into()],
            funcs: BTreeMap::from([(0x0800_0561, "SeedRngAndSetTrainerId".into())]),
        };
        assert_eq!(Mod::parse(&m.dir, &m.text()), Ok(m));
    }

    /// A cut from before the editor's guide in `HEAD`, with an edit just
    /// below the header (as `mods/emerald/text-speed` has), takes the new
    /// header on a re-cut with no conflict.
    #[test]
    fn a_re_cut_brings_in_a_new_header() {
        let old = "// Cut by `gbaa-recompile --cut` (docs/mod-view-design.md, \"A mod copy\"):\n\
                   // edit freely. orig/ keeps the print as cut, for a re-cut's merge.\n\n";
        let body = "#[allow(unused_imports)]\nuse super::*;\nuse crate::prelude::*;\n\
                    use base::globals::*;\n/// F, a.o\npub fn F(m: &mut M, mut pc: u32) -> Flow {\n    \
                    m.set(0x0808_434c, Ptr::<u8>::at(0x0200_0000), 1);\n}\n";
        let edited = body
            .replace(
                "use base::globals::*;\n",
                "use base::globals::*;\n\nconst X: u32 = 2;\n\n",
            )
            .replace(", 1);", ", X);");
        let dir = std::env::temp_dir().join(format!("gbaa-recut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (s, o, n) = (dir.join("src"), dir.join("orig"), dir.join("new"));
        std::fs::write(&s, format!("{old}{edited}")).unwrap();
        std::fs::write(&o, format!("{old}{body}")).unwrap();
        std::fs::write(&n, format!("{HEAD}{body}")).unwrap();
        let c = merge_file(&s, &o, &n);
        let got = std::fs::read_to_string(&s).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(c, Ok(0));
        assert_eq!(got, format!("{HEAD}{edited}"));
    }
}
