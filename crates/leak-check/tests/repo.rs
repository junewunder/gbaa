//! The repo itself has no leaks (src/lib.rs says what that means).

use std::path::Path;

#[test]
fn tracked_files_have_no_private_strings() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut words: Vec<String> = leak_check::PUBLIC_WORDS
        .iter()
        .map(|w| w.to_string())
        .collect();
    match leak_check::private_words() {
        Some(private) => words.extend(private),
        None => {
            eprintln!("leak-check: no $GBAA_DATA/leak-words.txt; checking the public list only")
        }
    }
    let hits = leak_check::scan_repo(&root, &words);
    let report: Vec<String> = hits
        .iter()
        .map(|h| format!("{}: {:?}", h.place, h.word))
        .collect();
    assert!(
        hits.is_empty(),
        "private strings in tracked files (fix at the source, or move the fact to an ignored file):\n{}",
        report.join("\n")
    );
}
