//! The lifted crate's files (unit `native/lift-modules`;
//! docs/native-stage-b.md, "The first game"). A literal lift of a real game
//! is about a million lines, and rustc gives one module one codegen unit:
//! a lift in one `main.rs` builds on one core, and a game's on one core
//! with its whole LLVM module in memory at once. So the functions go in
//! modules of about [`MODULE_LINES`] lines each, in the order they're
//! printed (by ROM address, then each IWRAM copy's), which rustc builds as
//! separate codegen units in parallel.
//!
//! - `src/main.rs`: the prelude, RAM's struct, `NAME` and `ENTRY`, then a
//!   `mod` (`lifted_mod!` for a module of functions, which a `guest-pc`
//!   build takes from the build script's copy: [`super::pc_marks`]) and a
//!   glob `use` for every other file;
//! - `src/fns.rs`: the `FNS` table (and `COPIES`, below);
//! - `src/<source>.rs` (unit `native/lift-modules-by-source`): the
//!   functions the names files give the source file `<source>`
//!   (`script_pokemon_util.rs`, from `src/script_pokemon_util.c`;
//!   [`module_names`]), its first line naming that file; a file longer
//!   than [`MODULE_LINES`] is split as below, `<source>_1.rs` on;
//! - `src/m_<key>.rs`: of the functions with no source file, those from
//!   the one keyed `<key>` on.
//!
//! Each module starts `use crate::*;` (after its source line), so it sees
//! the prelude and, through the root's glob uses, every other module's
//! functions.
//!
//! `--copies N` (`gbaa-lift`) adds the modules N - 1 more times (`m<i>_<key>`,
//! `m<i>_<source>`), each function renamed `f<i>_<key>` (and its `x_`, `a_` entries likewise):
//! a synthetic large lift for measuring the build, not a program. A
//! `#[used]` static (`fns::COPIES`) takes every copy's plain entry, so
//! rustc builds them all though nothing calls them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// About how many lines a module holds: it closes at the first function
/// that takes it past this (one longer function is a module of its own).
pub const MODULE_LINES: usize = 4000;

/// The lines each module file starts with, besides its functions: not
/// lifted code, so the `lines` metric leaves them out.
const MODULE_HEAD: &str = "use crate::*;\nuse armv4t::sem::ShiftType;\nuse armv4t::sem::prim;\nuse armv4t::sem::prim::MulKind;\n\n";

/// The macro that declares a module of lifted functions (in main.rs, after
/// the prelude): `src/<m>.rs`, or in a build with the `guest-pc` feature the
/// copy the crate's build script wrote with each block's `pc` put back
/// ([`super::pc_marks`]).
const LIFTED_MOD: &str =
    "// A module of lifted functions: with `guest-pc`, build.rs's copy with each block's `pc`.\n\
macro_rules! lifted_mod {
    ($m:ident) => {
        #[cfg(not(feature = \"guest-pc\"))]
        mod $m;
        #[cfg(feature = \"guest-pc\")]
        mod $m {
            include!(concat!(env!(\"OUT_DIR\"), \"/\", stringify!($m), \".rs\"));
        }
    };
}
";

/// A lifted crate's sources: `(path under src/, text)`, `main.rs` first.
pub struct Files {
    pub files: Vec<(String, String)>,
    /// The lines of [`Files::files`] that are module wiring (`mod`, `use`,
    /// each module's head), not lifted code.
    pub wiring: usize,
}

/// Renames copy `i`'s functions: `f_`, `x_` and `a_` followed by eight hex
/// digits, as a whole word, take `i` after the letter.
pub fn rename(text: &str, i: usize) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    let mut last = 0;
    let mut j = 0;
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while j + 10 <= b.len() {
        if matches!(b[j], b'f' | b'x' | b'a')
            && b[j + 1] == b'_'
            && (j == 0 || !word(b[j - 1]))
            && b[j + 2..j + 10].iter().all(u8::is_ascii_hexdigit)
            && b.get(j + 10).is_none_or(|&c| !word(c))
        {
            out.push_str(&text[last..=j]);
            write!(out, "{i}").unwrap();
            last = j + 1;
            j += 10;
        } else {
            j += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// The files of `src/` that aren't modules of lifted functions: the root,
/// `FNS`, the game's text and the per-function check's driver. A source
/// file's module never takes one of these names ([`module_names`]).
pub const NOT_MODULES: [&str; 4] = ["main", "fns", "text", "fn_check"];

/// Whether `f` (a name under src/) is a module of lifted functions.
pub fn is_module(f: &str) -> bool {
    f.strip_suffix(".rs")
        .is_some_and(|m| !NOT_MODULES.contains(&m))
}

/// Names a source module can't take besides [`NOT_MODULES`]: Rust's
/// keywords, the crates and paths the code names, and `lib` (Cargo takes
/// `src/lib.rs` for a library's root).
const RESERVED: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
    "where", "while", "abstract", "become", "box", "do", "final", "gen", "macro", "override",
    "priv", "try", "typeof", "unsized", "virtual", "yield", "union", "std", "core", "alloc",
    "armv4t", "runtime", "prim", "sem", "hw", "lib",
];

/// The type-namespace names `head` declares (`struct X`, `type X`, `mod
/// X`, …): a module of the same name would clash with one.
fn declared(head: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut words = head
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty());
    while let Some(w) = words.next() {
        if matches!(
            w,
            "struct" | "enum" | "union" | "type" | "trait" | "mod" | "crate" | "as"
        ) && let Some(n) = words.next()
        {
            out.insert(n.to_string());
        }
    }
    out
}

/// A source file's path as a module name's words: lowercase, each run of
/// other characters one `_`, never a leading digit.
fn ident(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "c_");
    }
    out
}

/// Each source file's module name (unit `native/lift-modules-by-source`):
/// its file name without the extension (`src/script_pokemon_util.c`,
/// `script_pokemon_util`); where two files share one, each takes its whole
/// path's words but a leading `src/` (`sprites_ai_ridley`); a name the
/// crate has already (`main`, a declared type, a keyword) takes its
/// extension (`main_c`), and an address module's form (`m_0800…`) or a
/// name still taken gets `_` until it's free.
pub fn module_names<'a>(
    srcs: impl IntoIterator<Item = &'a str>,
    head: &str,
) -> BTreeMap<String, String> {
    let srcs: BTreeSet<&str> = srcs.into_iter().collect();
    let stem = |s: &str| {
        let file = s.rsplit('/').next().unwrap_or(s);
        ident(file.rsplit_once('.').map_or(file, |(b, _)| b))
    };
    let mut by_stem: BTreeMap<String, usize> = BTreeMap::new();
    for s in &srcs {
        *by_stem.entry(stem(s)).or_default() += 1;
    }
    let mut taken: BTreeSet<String> = declared(head);
    taken.extend(NOT_MODULES.iter().chain(RESERVED).map(|s| s.to_string()));
    let addressy = |n: &str| {
        n.strip_prefix('m').is_some_and(|r| {
            let r = r.trim_start_matches(|c: char| c.is_ascii_digit());
            r.strip_prefix('_')
                .is_some_and(|h| h.len() == 8 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        })
    };
    let mut out = BTreeMap::new();
    for s in srcs {
        let mut n = stem(s);
        if by_stem[&n] > 1 {
            let path = s.strip_prefix("src/").unwrap_or(s);
            n = ident(path.rsplit_once('.').map_or(path, |(b, _)| b));
        }
        if taken.contains(&n) {
            let ext = s.rsplit('/').next().unwrap_or(s).rsplit_once('.');
            n = format!("{n}_{}", ext.map_or("c".into(), |(_, e)| ident(e)));
        }
        // Not an address module's name, nor (with `--copies`, which
        // prefixes `m<i>_`) a key's.
        let key = n.len() == 8 && n.bytes().all(|b| b.is_ascii_hexdigit());
        while taken.contains(&n) || addressy(&n) || key {
            n.push('_');
        }
        taken.insert(n.clone());
        out.insert(s.to_string(), n);
    }
    out
}

/// The crate's files from the root's text (`head`, prelude to `ENTRY`), the
/// `FNS` table, the plain entry of every function in it (`plains`, for
/// `COPIES`), each function's text by key in print order, and the source
/// file the names files give each function, by key (`srcs`). A function
/// with a source file goes in that file's module ([`module_names`]),
/// split as the address modules are where it's longer than
/// [`MODULE_LINES`] and numbered from 1 then; the rest in modules named
/// for their first function's key, as before.
pub fn split(
    head: String,
    fns: String,
    plains: &[String],
    funcs: &[(u32, String)],
    srcs: &BTreeMap<u32, String>,
    copies: usize,
) -> Files {
    let names = module_names(
        funcs
            .iter()
            .filter_map(|(k, _)| srcs.get(k).map(String::as_str)),
        &head,
    );
    // Group the functions: each source file's, and the runs of the rest,
    // in print order, at the line budget. A group is (its module's name,
    // its source file, its text), its name settled once every group is.
    let mut groups: Vec<(String, Option<String>, String, usize)> = Vec::new();
    // Each source file's open group, and the address run's.
    let mut open: BTreeMap<Option<&str>, usize> = BTreeMap::new();
    for (k, text) in funcs {
        let src = srcs.get(k).map(String::as_str);
        let n = text.lines().count();
        match open.get(&src).map(|&i| &mut groups[i]) {
            Some((_, _, g, lines)) if *lines + n <= MODULE_LINES || *lines == 0 => {
                g.push_str(text);
                *lines += n;
            }
            _ => {
                let m = match src {
                    Some(s) => names[s].clone(),
                    None => format!("m_{k:08x}"),
                };
                open.insert(src, groups.len());
                groups.push((m, src.map(String::from), text.clone(), n));
            }
        }
    }
    // A source file in more than one module: numbered from 1.
    let mut parts: BTreeMap<String, usize> = BTreeMap::new();
    for (m, s, _, _) in &groups {
        if s.is_some() {
            *parts.entry(m.clone()).or_default() += 1;
        }
    }
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut taken: BTreeSet<String> = groups.iter().map(|g| g.0.clone()).collect();
    // (module, its head line, its functions)
    let groups: Vec<(String, String, String)> = groups
        .into_iter()
        .map(|(m, s, g, _)| {
            let Some(s) = s else {
                return (m, String::new(), g);
            };
            let i = seen.entry(m.clone()).or_default();
            *i += 1;
            if parts[&m] > 1 {
                let mut n = format!("{m}_{i}");
                while taken.contains(&n) {
                    n.push('_');
                }
                taken.insert(n.clone());
                let head = format!("// From the decomp's {s} (part {i} of {}).\n", parts[&m]);
                (n, head, g)
            } else {
                (m, format!("// From the decomp's {s}.\n"), g)
            }
        })
        .collect();
    let mut files = Vec::new();
    let mut wiring = 0;
    let mut root = head;
    root.push('\n');
    let mut table = fns;
    let mut uses = vec!["fns".to_string()];
    for i in 0..copies.max(1) {
        for (m, h, g) in &groups {
            let (m, body) = match i {
                0 => (m.clone(), g.clone()),
                // A copy: `m<i>_<key>`, or `m<i>_<source module>`.
                _ => (
                    format!("m{i}_{}", m.strip_prefix("m_").unwrap_or(m)),
                    rename(g, i),
                ),
            };
            wiring += h.lines().count() + MODULE_HEAD.lines().count();
            files.push((format!("{m}.rs"), format!("{h}{MODULE_HEAD}{body}")));
            uses.push(m);
        }
    }
    if copies > 1 {
        let n = plains.len() * (copies - 1);
        writeln!(
            table,
            "\n/// `gbaa-lift --copies {copies}`: every copy's plain entry, so they're built.\n#[used]\npub static COPIES: [Fn; {n}] = ["
        )
        .unwrap();
        for i in 1..copies {
            for p in plains {
                writeln!(table, "    {},", rename(p, i)).unwrap();
            }
        }
        table.push_str("];\n");
        // `COPIES` isn't lifted code.
        wiring += 4 + n + 1;
    }
    root.push_str(LIFTED_MOD);
    wiring += LIFTED_MOD.lines().count();
    for m in &uses {
        let decl = if m == "fns" {
            "mod fns;".to_string()
        } else {
            format!("lifted_mod!({m});")
        };
        writeln!(root, "{decl}\nuse {m}::*;").unwrap();
        wiring += 2;
    }
    wiring += 1;
    files.insert(0, ("fns.rs".into(), format!("use crate::*;\n\n{table}")));
    wiring += 2;
    files.insert(0, ("main.rs".into(), root));
    Files { files, wiring }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_whole_keys_only() {
        assert_eq!(
            rename(
                "f_08000100(g); x_03000000(g, s); a_0800abcd, ff_08000100 f_0800010 f_08000100x",
                3
            ),
            "f3_08000100(g); x3_03000000(g, s); a3_0800abcd, ff_08000100 f_0800010 f_08000100x"
        );
    }

    #[test]
    fn splits_at_the_line_budget() {
        let big = "x\n".repeat(MODULE_LINES - 10);
        let funcs = vec![
            (0x0800_0000, big.clone()),
            (0x0800_0100, "y\n".repeat(20)),
            (0x0800_0200, "z\n".repeat(5)),
            (0x0800_0300, "w\n".repeat(MODULE_LINES * 2)),
        ];
        let f = split(
            "head\n".into(),
            "fns\n".into(),
            &[],
            &funcs,
            &BTreeMap::new(),
            1,
        );
        let names: Vec<&str> = f.files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "main.rs",
                "fns.rs",
                "m_08000000.rs",
                "m_08000100.rs",
                "m_08000300.rs"
            ]
        );
        assert!(
            f.files[0]
                .1
                .contains("lifted_mod!(m_08000100);\nuse m_08000100::*;")
        );
        let total: usize = f.files.iter().map(|(_, t)| t.lines().count()).sum();
        // What's left once the wiring is out: the head, FNS and the functions.
        assert_eq!(
            total - f.wiring,
            1 + 1 + MODULE_LINES - 10 + 20 + 5 + MODULE_LINES * 2
        );
    }

    #[test]
    fn groups_by_source_file() {
        let big = "x\n".repeat(MODULE_LINES - 10);
        let funcs = vec![
            (0x0800_0001, "a\n".to_string()),
            (0x0800_0101, "b\n".to_string()),
            (0x0800_0201, big.clone()),
            (0x0800_0301, "c\n".to_string()),
            (0x0800_0401, "d\n".repeat(20)),
            (0x0800_0501, "e\n".to_string()),
        ];
        let srcs = BTreeMap::from([
            (0x0800_0001, "src/script_util.c".to_string()),
            (0x0800_0201, "src/main.c".to_string()),
            (0x0800_0401, "src/main.c".to_string()),
            (0x0800_0501, "src/script_util.c".to_string()),
        ]);
        let f = split(
            "struct Task;\n".into(),
            "fns\n".into(),
            &[],
            &funcs,
            &srcs,
            1,
        );
        let names: Vec<&str> = f.files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "main.rs",
                "fns.rs",
                "script_util.rs",
                "m_08000101.rs",
                "main_c_1.rs",
                "main_c_2.rs"
            ]
        );
        // A file's functions together, wherever they are; the rest by address.
        assert!(
            f.files[2]
                .1
                .starts_with("// From the decomp's src/script_util.c.\nuse crate::*;")
        );
        assert!(f.files[2].1.ends_with("a\ne\n"));
        assert!(f.files[3].1.ends_with("b\nc\n"));
        assert!(
            f.files[4]
                .1
                .starts_with("// From the decomp's src/main.c (part 1 of 2).\n")
        );
        assert!(
            f.files[0]
                .1
                .contains("lifted_mod!(main_c_2);\nuse main_c_2::*;")
        );
        assert!(
            f.files
                .iter()
                .all(|(n, _)| n == "main.rs" || n == "fns.rs" || is_module(n))
        );
        let total: usize = f.files.iter().map(|(_, t)| t.lines().count()).sum();
        assert_eq!(total - f.wiring, 1 + 1 + 4 + MODULE_LINES - 10 + 20);
    }

    #[test]
    fn names_modules_apart() {
        let n = module_names(
            [
                "src/sprites_AI/ridley.c",
                "src/ridley.c",
                "src/m4a.c",
                "src/core.c",
                "src/Task.c",
                "src/3d.c",
                "src/text.c",
                "c/chain/lib.c",
            ],
            "pub struct task;\n",
        );
        let get = |s: &str| n[s].as_str();
        assert_eq!(get("src/sprites_AI/ridley.c"), "sprites_ai_ridley");
        assert_eq!(get("src/ridley.c"), "ridley");
        assert_eq!(get("src/m4a.c"), "m4a");
        assert_eq!(get("src/core.c"), "core_c");
        assert_eq!(get("src/Task.c"), "task_c");
        assert_eq!(get("src/3d.c"), "c_3d");
        assert_eq!(get("src/text.c"), "text_c");
        assert_eq!(get("c/chain/lib.c"), "lib_c");
        assert!(!is_module("text.rs") && is_module("text_c.rs") && is_module("m_08000000.rs"));
    }
}
