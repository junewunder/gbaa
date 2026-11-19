//! `emu:loadFile`'s check (scripting/ironmon): `crates/script/fixtures/
//! load-file/load.lua` run headless with `--save` must see the first ROM's
//! save, load the next ROM (its title from then on), and find that ROM's
//! save blank; on exit the first save holds what the script wrote before
//! the load, and the next ROM's goes beside it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A ROM whose game is `b .`, with a header and SRAM's tag.
fn rom(dir: &Path, name: &str, title: &[u8; 12]) -> PathBuf {
    let mut r = vec![0u8; 0x200];
    r[..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
    r[0xA0..0xAC].copy_from_slice(title);
    r[0xAC..0xB0].copy_from_slice(b"BTST");
    r[0x100..0x10C].copy_from_slice(b"SRAM_V113\0\0\0");
    let p = dir.join(name);
    std::fs::write(&p, r).unwrap();
    p
}

#[test]
fn a_script_loads_another_rom_and_its_save() {
    let dir = std::env::temp_dir().join(format!("gbaa-load-file-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let first = rom(&dir, "first.gba", b"FIRSTROM\0\0\0\0");
    rom(&dir, "next.gba", b"NEXTROM\0\0\0\0\0");
    std::fs::write(dir.join("first.sav"), vec![0x11u8; 0x8000]).unwrap();
    // A sandboxed script loads only from its own folder: the ROMs'.
    let script = dir.join("load.lua");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../script/fixtures/load-file/load.lua"
        ),
        &script,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_gbaa-desktop"))
        .arg(&first)
        .args(["--headless", "--timing", "exact", "--frames", "6", "--save"])
        .arg(dir.join("first.sav"))
        .arg("--script")
        .arg(&script)
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
            "log: FIRSTROM save 11",
            "log: loaded: true",
            "log: NEXTROM save FF",
        ],
        "{stdout}"
    );
    let first_sav = std::fs::read(dir.join("first.sav")).unwrap();
    assert_eq!(first_sav[..2], [0x22, 0x11]);
    let next_sav = std::fs::read(dir.join("next.sav")).unwrap();
    assert_eq!(next_sav[..2], [0x33, 0xFF]);
    let _ = std::fs::remove_dir_all(&dir);
}
