//! The Lua host's check (`just script-check`): the fixture in
//! `crates/script/fixtures/emerald/` and mGBA 0.10.5's `pokemon.lua`, run
//! headless on Emerald (USA) from power-on with no save, must print what
//! `expected.txt` there says. Ignored by `cargo test`: it needs the ROM
//! (`GBAA_SCRIPT_ROM`) and the pinned mGBA checkout (`GBAA_SCRIPT_MGBA`,
//! from `scripts/fetch-mgba.sh`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Into Birch's speech, after the fixture writes the party (frame 1200).
const FRAMES: u32 = 1500;

fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .unwrap_or_else(|| panic!("{name} is unset (just script-check sets it)"))
        .into()
}

#[test]
#[ignore = "needs Emerald and mGBA's checkout: just script-check"]
fn fixture_and_pokemon_lua_on_emerald() {
    let rom = env_path("GBAA_SCRIPT_ROM");
    let mgba = env_path("GBAA_SCRIPT_MGBA");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/fixtures/emerald");
    let out = Command::new(env!("CARGO_BIN_EXE_gbaa-desktop"))
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
        .arg("--script")
        .arg(mgba.join("res/scripts/pokemon.lua"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "gbaa-desktop failed: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    // From the script report on, with the paths made relative.
    let got = &stdout[stdout.find("script console:").expect("a script report")..];
    let got = got
        .replace(&format!("{}/", fixtures.display()), "")
        .replace(&format!("{}/", mgba.display()), "");
    let want = std::fs::read_to_string(fixtures.join("expected.txt")).unwrap();
    if got != want {
        let actual = std::env::temp_dir().join("gbaa-script-check.actual.txt");
        std::fs::write(&actual, &got).unwrap();
        panic!(
            "script output differs from expected.txt (got it in {}):\n{got}",
            actual.display()
        );
    }
}
