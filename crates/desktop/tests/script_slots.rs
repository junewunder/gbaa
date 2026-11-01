//! The scripts' state slots' check (scripting/state-slots):
//! `crates/script/fixtures/slots/slots.lua` run headless must save the
//! window's slot 7 (`<rom>.state7`) and load it back, and an empty slot
//! or a negative one must answer false.

use std::path::PathBuf;
use std::process::Command;

/// A ROM whose game is `b .`, with a header.
fn rom(dir: &std::path::Path) -> PathBuf {
    let mut r = vec![0u8; 0x200];
    r[..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
    r[0xA0..0xAC].copy_from_slice(b"SLOTTEST\0\0\0\0");
    r[0xAC..0xB0].copy_from_slice(b"BTST");
    let p = dir.join("loop.gba");
    std::fs::write(&p, r).unwrap();
    p
}

#[test]
fn a_script_saves_a_slot_and_loads_it_back() {
    let dir = std::env::temp_dir().join(format!("gbaa-slots-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../script/fixtures/slots/slots.lua"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_gbaa-desktop"))
        .arg(rom(&dir))
        .args([
            "--headless",
            "--timing",
            "exact",
            "--frames",
            "6",
            "--script",
            script,
        ])
        .env("GBAA_CONFIG_DIR", dir.join("config"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "gbaa-desktop failed: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    let console: Vec<&str> = stdout
        .lines()
        .skip_while(|l| *l != "script console:")
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .map(str::trim)
        .collect();
    assert_eq!(
        console,
        [
            "log: saved 7: true",
            "log: saved -1: false",
            "log: loaded 8: false",
            "log: loaded 7: true",
            "log: wram 12345678",
        ],
        "{stdout}"
    );
    assert!(dir.join("loop.state7").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
