//! The leak check. gbaa is developed in public, so nothing tracked may name
//! june's machines, network, accounts or where ROMs come from.
//!
//! Two lists of strings, matched case-insensitively as plain substrings:
//!
//! - [`PUBLIC_WORDS`], here: shapes of secrets that are fine to name.
//! - `$GBAA_DATA/leak-words.txt` (default `~/.local/share/gbaa/`): the
//!   private ones (the tailnet name, IP prefixes, host and account names),
//!   one per line, `#` for comments. It can't be in git, since it names
//!   what it hides. Without it only the public list is checked, which is
//!   what a stranger's clone gets.
//!
//! A hit is fixed at its source: say it another way, or move the fact to an
//! ignored file (`docs/machines.private.md`, `mods/`).

pub mod budgets;

use std::path::{Path, PathBuf};
use std::process::Command;

/// Strings that are never in a tracked file, whoever's clone it is. Each is
/// split with `concat!` so this file doesn't match itself.
pub const PUBLIC_WORDS: &[&str] = &[
    concat!("-----BEGIN ", "RSA PRIVATE KEY"),
    concat!("-----BEGIN ", "OPENSSH PRIVATE KEY"),
    concat!("-----BEGIN ", "EC PRIVATE KEY"),
    concat!("-----BEGIN ", "PRIVATE KEY"),
    concat!(".ts", ".net"),
];

/// One hit: where, and which word.
#[derive(Debug, PartialEq, Eq)]
pub struct Hit {
    pub place: String,
    pub word: String,
}

/// The private words from `$GBAA_DATA/leak-words.txt`, or `None` if the
/// file isn't there.
pub fn private_words() -> Option<Vec<String>> {
    let data = std::env::var_os("GBAA_DATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/gbaa")))?;
    let text = std::fs::read_to_string(data.join("leak-words.txt")).ok()?;
    Some(parse_words(&text))
}

/// A word list's lines, trimmed, without blanks and `#` comments.
pub fn parse_words(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The hits of `words` in `text`, one per line and word, named `place:line`.
pub fn scan(place: &str, text: &str, words: &[String]) -> Vec<Hit> {
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let l = line.to_lowercase();
        for (w, lw) in words.iter().zip(&lower) {
            if l.contains(lw.as_str()) {
                hits.push(Hit {
                    place: format!("{place}:{}", i + 1),
                    word: w.clone(),
                });
            }
        }
    }
    hits
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Every hit in the tracked files of the repo at `root` (as they are on
/// disk) and in the messages of commits not yet on the upstream.
pub fn scan_repo(root: &Path, words: &[String]) -> Vec<Hit> {
    let mut hits = Vec::new();
    let files = git(root, &["ls-files", "-z"]).unwrap_or_default();
    for name in files.split(|&b| b == 0).filter(|n| !n.is_empty()) {
        let name = String::from_utf8_lossy(name);
        let Ok(bytes) = std::fs::read(root.join(name.as_ref())) else {
            continue; // deleted in the working tree
        };
        hits.extend(scan(&name, &String::from_utf8_lossy(&bytes), words));
    }
    if let Some(log) = git(
        root,
        &["log", "--format=%H%n%an <%ae>%n%B", "@{upstream}..HEAD"],
    ) {
        hits.extend(scan(
            "unpushed commit messages",
            &String::from_utf8_lossy(&log),
            words,
        ));
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_any_case_and_names_the_line() {
        let words = parse_words("# comment\n\nSecret-Host\n");
        let hits = scan("f.md", "fine\nssh secret-host.example\n", &words);
        assert_eq!(
            hits,
            vec![Hit {
                place: "f.md:2".into(),
                word: "Secret-Host".into()
            }]
        );
    }
}
