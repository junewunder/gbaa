//! The scripts' keyboard's check (scripting/input-keys):
//! `crates/script/fixtures/input/keys.lua` run headless with keys held by
//! `--script-keys` must see them through mGBA 0.11-dev's `input`, in the
//! frames they're held, and get their `key` events.

use std::path::PathBuf;
use std::process::Command;

/// A ROM whose game is `b .`, with a header.
fn rom(dir: &std::path::Path) -> PathBuf {
    let mut r = vec![0u8; 0x200];
    r[..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
    r[0xA0..0xAC].copy_from_slice(b"INPUTTEST\0\0\0");
    r[0xAC..0xB0].copy_from_slice(b"BTST");
    let p = dir.join("loop.gba");
    std::fs::write(&p, r).unwrap();
    p
}

#[test]
fn a_script_reads_keys_held_headless() {
    let dir = std::env::temp_dir().join(format!("gbaa-input-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../script/fixtures/input/keys.lua"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_gbaa-desktop"))
        .arg(rom(&dir))
        .args([
            "--headless",
            "--timing",
            "exact",
            "--frames",
            "12",
            "--script",
            script,
        ])
        .args(["--script-keys", "shift@3+4,!@4+2,a@8+2"])
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
            "log: frame 1: shift up",
            "log: frame 1: bang up",
            "log: frame 1: a up",
            "log: key 0: 800032 state 1 mods 3",
            "log: frame 5: shift held",
            "log: key 1: 21 state 1 mods 3",
            "log: frame 6: bang held",
            "log: hotkey Shift+! in frame 6, keys 33 8388658",
            "log: key 2: 21 state 0 mods 3",
            "log: frame 8: bang up",
            "log: key 3: 800032 state 0 mods 0",
            "log: frame 9: shift up",
            "log: key 4: 41 state 1 mods 0",
            "log: frame 10: a held",
            "log: key 5: 41 state 0 mods 0",
            "log: frame 12: a up",
        ],
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
