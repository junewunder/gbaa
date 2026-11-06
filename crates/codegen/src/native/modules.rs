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
//!   `mod` and a glob `use` for every other file;
//! - `src/fns.rs`: the `FNS` table (and `COPIES`, below);
//! - `src/m_<key>.rs`: the functions from the one keyed `<key>` on. Each
//!   starts `use crate::*;`, so it sees the prelude and, through the root's
//!   glob uses, every other module's functions.
//!
//! `--copies N` (`gbaa-lift`) adds the modules N - 1 more times, each
//! function renamed `f<i>_<key>` (and its `x_`, `a_` entries likewise):
//! a synthetic large lift for measuring the build, not a program. A
//! `#[used]` static (`fns::COPIES`) takes every copy's plain entry, so
//! rustc builds them all though nothing calls them.

use std::fmt::Write;

/// About how many lines a module holds: it closes at the first function
/// that takes it past this (one longer function is a module of its own).
pub const MODULE_LINES: usize = 4000;

/// The lines each module file starts with, besides its functions: not
/// lifted code, so the `lines` metric leaves them out.
const MODULE_HEAD: &str = "use crate::*;\nuse armv4t::sem::ShiftType;\nuse armv4t::sem::prim;\nuse armv4t::sem::prim::MulKind;\n\n";

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

/// The crate's files from the root's text (`head`, prelude to `ENTRY`), the
/// `FNS` table, the plain entry of every function in it (`plains`, for
/// `COPIES`), and each function's text by key in print order.
pub fn split(
    head: String,
    fns: String,
    plains: &[String],
    funcs: &[(u32, String)],
    copies: usize,
) -> Files {
    // Group the functions.
    let mut groups: Vec<(u32, String)> = Vec::new();
    let mut lines = 0;
    for (k, text) in funcs {
        let n = text.lines().count();
        match groups.last_mut() {
            Some((_, g)) if lines + n <= MODULE_LINES || lines == 0 => {
                g.push_str(text);
                lines += n;
            }
            _ => {
                groups.push((*k, text.clone()));
                lines = n;
            }
        }
    }
    let mut files = Vec::new();
    let mut wiring = 0;
    let mut root = head;
    root.push('\n');
    let mut table = fns;
    let mut uses = vec!["fns".to_string()];
    for i in 0..copies.max(1) {
        for (k, g) in &groups {
            let m = if i == 0 {
                format!("m_{k:08x}")
            } else {
                format!("m{i}_{k:08x}")
            };
            let body = if i == 0 { g.clone() } else { rename(g, i) };
            wiring += MODULE_HEAD.lines().count();
            files.push((format!("{m}.rs"), format!("{MODULE_HEAD}{body}")));
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
    for m in &uses {
        writeln!(root, "mod {m};\nuse {m}::*;").unwrap();
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
        let f = split("head\n".into(), "fns\n".into(), &[], &funcs, 1);
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
        assert!(f.files[0].1.contains("mod m_08000100;\nuse m_08000100::*;"));
        let total: usize = f.files.iter().map(|(_, t)| t.lines().count()).sum();
        // What's left once the wiring is out: the head, FNS and the functions.
        assert_eq!(
            total - f.wiring,
            1 + 1 + MODULE_LINES - 10 + 20 + 5 + MODULE_LINES * 2
        );
    }
}
