//! Lifted code indented by its nesting (unit `native/lift-indent`):
//! `gbaa-lift` on a C program of the lift-check file (c-loops, agbcc's
//! build: nested counted loops), and every line of each module of lifted
//! functions starts with 4 spaces for each brace it sits inside. Skipped,
//! saying so, where `just c-tests` hasn't built it into `$GBAA_DATA`, so
//! `just check` works on a machine without it.

use std::path::{Path, PathBuf};
use std::process::Command;

use codegen::native::{indent, modules};

const ROM: &str = "c/c-loops-agbcc.gba";

fn data() -> PathBuf {
    std::env::var_os("GBAA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".local/share/gbaa")
        })
}

#[test]
fn every_lifted_line_is_indented_by_its_brace_depth() {
    let rom = data().join(ROM);
    if !rom.exists() {
        eprintln!("skipped: no {ROM} in $GBAA_DATA (just c-tests)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("gbaa-lift-indent-{}", std::process::id()));
    let out = tmp.join("lift");
    // A small C program: no memory cap needed (analysis::cap).
    let o = Command::new(env!("CARGO_BIN_EXE_gbaa-lift"))
        .env("GBAA_UNCAPPED", "1")
        .arg(&rom)
        .arg(&out)
        .output()
        .expect("gbaa-lift runs");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let src = out.join("src");
    let mut seen = 0;
    let mut nested = 0;
    for e in std::fs::read_dir(&src).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !modules::is_module(&name) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let bad = indent::misindented(&text);
        assert!(
            bad.is_empty(),
            "{}: lines not at 4 spaces a brace: {:?}",
            Path::new(&name).display(),
            &bad[..bad.len().min(10)]
        );
        seen += text.lines().count();
        // The nested loops are there to be seen: lines three blocks deep.
        nested += text
            .lines()
            .filter(|l| l.starts_with("            "))
            .count();
    }
    assert!(
        seen > 100,
        "only {seen} lines of lifted modules in {}",
        src.display()
    );
    assert!(nested > 0, "no line three blocks deep in {}", src.display());
    std::fs::remove_dir_all(&tmp).ok();
}
