//! Every doc in `docs/budgets.toml` is within its line budget
//! (src/budgets.rs says why).

use std::path::Path;

#[test]
fn docs_are_within_their_line_budgets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("docs/budgets.toml")).unwrap();
    let budgets = leak_check::budgets::parse(&text).unwrap();
    let problems = leak_check::budgets::check(&root, &budgets);
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}
