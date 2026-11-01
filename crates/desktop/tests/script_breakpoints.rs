//! The breakpoints check (`just script-check`): the fixture in
//! `crates/script/fixtures/breakpoints/`, run headless on Emerald (USA)
//! from power-on with no save, must print what `expected.txt` there says:
//! each breakpoint and watchpoint at its frame, as a once-a-frame look at
//! the same memory sees it. Ignored by `cargo test`: it needs the ROM
//! (`GBAA_SCRIPT_ROM`). `GBAA_SCRIPT_BIN`, if set and not empty, is the binary to run
//! instead of gbaa-desktop: a recompiled build, which runs frames in the
//! interpreter while a point is set and compiled once they're cleared.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Past the title screen's set-up (frames 242-279) and 100 frames after
/// the last point is cleared (frame 500).
const FRAMES: u32 = 600;

#[test]
#[ignore = "needs Emerald: just script-check"]
fn breakpoints_and_watchpoints_on_emerald() {
    let rom: PathBuf = std::env::var_os("GBAA_SCRIPT_ROM")
        .expect("GBAA_SCRIPT_ROM is unset (just script-check sets it)")
        .into();
    let bin = std::env::var_os("GBAA_SCRIPT_BIN")
        .filter(|b| !b.is_empty())
        .map_or_else(|| env!("CARGO_BIN_EXE_gbaa-desktop").into(), PathBuf::from);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/fixtures/breakpoints");
    let out = Command::new(&bin)
        .arg(&rom)
        .args([
            "--headless",
            "--timing",
            "exact",
            "--frames",
            &FRAMES.to_string(),
        ])
        .arg("--script")
        .arg(fixtures.join("main.lua"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{} failed: {}\n{stdout}",
        bin.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let got = &stdout[stdout.find("script console:").expect("a script report")..];
    let want = std::fs::read_to_string(fixtures.join("expected.txt")).unwrap();
    if got != want {
        let actual = std::env::temp_dir().join("gbaa-script-breakpoints.actual.txt");
        std::fs::write(&actual, got).unwrap();
        panic!(
            "script output differs from expected.txt (got it in {}):\n{got}",
            actual.display()
        );
    }
}
