//! The bars hold (unit `native/lift-bars`): `gbaa-lift --bars` on a
//! program of the committed lift-check file, with one bar lowered below
//! the measured value (to 0), fails naming the metric and both numbers,
//! and nothing else: its other bars hold. Lifts agb/affine (the smallest
//! program) from `$GBAA_DATA`; skipped, saying so, where it isn't built
//! (`just agb-tests`), so `just check` works on a machine without
//! `$GBAA_DATA`.

use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAM: &str = "affine";
/// The bar lowered: the computed-address loads no analysis bounds, the
/// family most stage-B units work on.
const METRIC: &str = "load_computed_unknown";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn data() -> PathBuf {
    std::env::var_os("GBAA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".local/share/gbaa")
        })
}

fn lift(bars: &Path, out: &Path) -> (bool, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_gbaa-lift"))
        .arg(data().join(format!("agb/{PROGRAM}.gba")))
        .arg(out)
        .arg("--holes")
        .arg(root().join(format!("holes/{PROGRAM}.toml")))
        .arg("--bars")
        .arg(bars)
        .output()
        .expect("gbaa-lift runs");
    (
        o.status.success(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn a_lowered_bar_fails_the_lift_naming_it() {
    if !data().join(format!("agb/{PROGRAM}.gba")).exists() {
        eprintln!("skipped: no agb/{PROGRAM}.gba in $GBAA_DATA (just agb-tests)");
        return;
    }
    let file = root().join("crates/codegen/lift-check.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    let tmp = std::env::temp_dir().join(format!("gbaa-lift-bars-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    // The bar lowered to 0, below any value there is some of; every other
    // bar as committed.
    let mut lowered = String::new();
    let mut in_bars = false;
    let mut found = false;
    for l in text.lines() {
        if l.starts_with('[') {
            in_bars = l == format!("[program.{PROGRAM}.bars]");
        }
        if in_bars && l.split('=').next().map(str::trim) == Some(METRIC) {
            lowered.push_str(&format!("{METRIC} = 0\n"));
            found = true;
        } else {
            lowered.push_str(l);
            lowered.push('\n');
        }
    }
    assert!(found, "no bar {METRIC} for {PROGRAM}");
    let bars = tmp.join("lowered.toml");
    std::fs::write(&bars, lowered).unwrap();
    let (ok, out, err) = lift(&bars, &tmp.join("lift"));
    // The value the report measured.
    let value: u64 = out
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{METRIC} ")))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no `{METRIC} N` line in the report:\n{out}"));
    assert!(value > 0, "{METRIC} is 0: pick a metric there is some of");
    assert!(!ok, "the lift passed with {METRIC}'s bar lowered");
    let want = format!("gbaa-lift: {PROGRAM}: {METRIC} is {value}, over its bar of 0");
    assert!(err.contains(&want), "wanted {want:?} in:\n{err}");
    // Only that bar fails: the committed ones hold.
    assert_eq!(err.matches("over its bar").count(), 1, "{err}");
    assert_eq!(
        err.lines().filter(|l| l.contains("bar")).count(),
        1,
        "{err}"
    );
    std::fs::remove_dir_all(&tmp).ok();
}
