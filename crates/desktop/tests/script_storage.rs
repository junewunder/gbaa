//! The storage API's check (scripting/storage): `crates/script/fixtures/
//! storage/storage.lua` run headless twice with one config folder; the
//! first run writes a bucket, the second must read it back.

use std::path::PathBuf;
use std::process::Command;

/// A ROM whose game is `b .`, with a header.
fn rom(dir: &std::path::Path) -> PathBuf {
    let mut r = vec![0u8; 0x200];
    r[..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
    r[0xA0..0xAC].copy_from_slice(b"STORAGETEST\0");
    r[0xAC..0xB0].copy_from_slice(b"BTST");
    let p = dir.join("loop.gba");
    std::fs::write(&p, r).unwrap();
    p
}

fn run(rom: &std::path::Path, config: &std::path::Path) -> String {
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../script/fixtures/storage/storage.lua"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_gbaa-desktop"))
        .arg(rom)
        .args([
            "--headless",
            "--timing",
            "exact",
            "--frames",
            "10",
            "--script",
            script,
        ])
        .env("GBAA_CONFIG_DIR", config)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "gbaa-desktop failed: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

#[test]
fn a_bucket_written_in_one_run_is_read_in_the_next() {
    let dir = std::env::temp_dir().join(format!("gbaa-storage-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let rom = rom(&dir);
    let config = dir.join("config");
    let first = run(&rom, &config);
    assert!(first.contains("log: run 1: wrote the bucket"), "{first}");
    let file = config.join("storage/gbaa_storage_check.json");
    let json = std::fs::read_to_string(&file).unwrap();
    assert!(json.contains("\"runs\":1"), "{json}");
    let second = run(&rom, &config);
    assert!(
        second.contains("log: run 2: gbaa 1,2,3,4 0.5 true integer"),
        "{second}"
    );
    let third = run(&rom, &config);
    assert!(third.contains("log: run 3: "), "{third}");
    let _ = std::fs::remove_dir_all(&dir);
}
