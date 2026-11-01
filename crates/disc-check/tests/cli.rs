//! The binary's exit status, which the unit tests can't see.

const TRUTH: &str = "0x08000101\t64\tThumbFn\n0x08000200\t16\tArmFn\n";

fn score(disc: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!(
        "disc-check-cli-{}-{}",
        std::process::id(),
        disc.len()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (t, d) = (dir.join("truth.tsv"), dir.join("disc.txt"));
    std::fs::write(&t, TRUTH).unwrap();
    std::fs::write(&d, disc).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_disc-check"))
        .args(["score".as_ref(), t.as_os_str(), d.as_os_str()])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    out
}

#[test]
fn misaligned_arm_start_exits_non_zero() {
    let out = score("0x08000101\n0x08000206\n");
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "scored anyway");
    assert!(String::from_utf8_lossy(&out.stderr).contains("0x08000206"));
}

#[test]
fn good_file_exits_zero() {
    let out = score("0x08000101\n");
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .starts_with("precision 1.0000 (1/1)\nrecall 0.5000 (1/2)\n")
    );
}
