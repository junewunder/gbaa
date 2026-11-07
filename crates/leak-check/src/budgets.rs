//! Line budgets for docs (`docs/budgets.toml`): the repo's other text
//! check, beside the leak check. A plan doc that every factory session
//! reads must stay a plan; when a unit appends its results to one, the
//! doc grows into a log that each later session pays to read. Each listed
//! doc has a maximum number of lines; over it, the check names the doc
//! and says where results belong instead.

use serde::Deserialize;
use std::path::Path;

/// `docs/budgets.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub doc: Vec<Budget>,
}

/// One doc's budget: its path from the repo root and its most lines.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub path: String,
    pub max_lines: usize,
}

/// Parse a budgets file.
pub fn parse(text: &str) -> Result<Budgets, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// The message for a doc over its budget.
pub fn over(path: &str, lines: usize, max: usize) -> String {
    format!(
        "{path}: {lines} lines, budget {max}: move results to its numbers file or the tracker, or raise the budget with a reason"
    )
}

/// Every problem with `budgets` against the files under `root`: a doc
/// over its budget, or one that isn't there (a renamed doc must take its
/// budget with it).
pub fn check(root: &Path, budgets: &Budgets) -> Vec<String> {
    let mut problems = Vec::new();
    for b in &budgets.doc {
        match std::fs::read_to_string(root.join(&b.path)) {
            Ok(t) => {
                let n = t.lines().count();
                if n > b.max_lines {
                    problems.push(over(&b.path, n, b.max_lines));
                }
            }
            Err(e) => problems.push(format!(
                "{}: {e} (docs/budgets.toml lists it: fix the path or drop the entry)",
                b.path
            )),
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "gbaa-budgets-{}-{}",
            std::process::id(),
            files.len()
        ));
        let _ = std::fs::remove_dir_all(&d);
        for (p, t) in files {
            let f = d.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        }
        d
    }

    #[test]
    fn a_doc_over_its_budget_is_named() {
        let d = dir(&[("docs/a.md", "1\n2\n3\n"), ("docs/b.md", "1\n2\n")]);
        let b = parse(
            "[[doc]]\npath = \"docs/a.md\"\nmax_lines = 2\n\n\
             [[doc]]\npath = \"docs/b.md\"\nmax_lines = 2\n",
        )
        .unwrap();
        assert_eq!(
            check(&d, &b),
            vec![
                "docs/a.md: 3 lines, budget 2: move results to its numbers file or the tracker, or raise the budget with a reason"
            ]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_doc_is_a_problem() {
        let d = dir(&[]);
        let b = parse("[[doc]]\npath = \"docs/gone.md\"\nmax_lines = 10\n").unwrap();
        let p = check(&d, &b);
        assert_eq!(p.len(), 1);
        assert!(p[0].starts_with("docs/gone.md: "), "{p:?}");
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(parse("[[doc]]\npath = \"a\"\nmax = 1\n").is_err());
    }
}
