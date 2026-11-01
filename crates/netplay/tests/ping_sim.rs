//! The variable-ping simulator's check (multiplayer/ping-sim): two peer
//! processes play a session through the proxy under each profile in
//! `profiles/` and end on a straight run's hashes (`gbaa-pingsim run`
//! says how it went; it fails on any difference or desync), by rollback.
//! The server's check (multiplayer/relay-server) is the same through a
//! local `gbaa-server`'s rooms, each peer's path to it through a proxy
//! (`gbaa-pingsim room`).

use std::path::Path;
use std::process::Command;

fn run(profile: &str, args: &[&str]) {
    check("run", profile, args)
}

fn room(profile: &str, args: &[&str]) {
    check("room", profile, args)
}

fn check(how: &str, profile: &str, args: &[&str]) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("profiles")
        .join(profile);
    let out = Command::new(env!("CARGO_BIN_EXE_gbaa-pingsim"))
        .arg(how)
        .arg(&path)
        .args(args)
        .args(["--linger-ms", "20000"])
        .output()
        .unwrap();
    let say = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("{say}");
    assert!(out.status.success(), "{profile}: {say}");
    assert!(say.contains("pass:"), "{say}");
}

#[test]
fn calm() {
    run("calm.toml", &["--frames", "360"]);
}

#[test]
fn wander() {
    run("wander.toml", &["--frames", "360"]);
}

#[test]
fn spiky() {
    run("spiky.toml", &["--frames", "360"]);
}

#[test]
fn rough() {
    run("rough.toml", &["--frames", "360"]);
}

/// Two GBAs at 20 fps: a debug build's frame of both (about 30 ms, a
/// snapshot included) can't keep 60, and a peer that can't keep up rolls
/// back further each frame. A release build plays them at 60.
const GBA: &[&str] = &["--console", "gba", "--frames", "240", "--fps", "20"];

#[test]
fn rough_with_linked_gbas() {
    run("rough.toml", GBA);
}

#[test]
fn room_calm() {
    room("calm.toml", &["--frames", "360"]);
}

#[test]
fn room_spiky() {
    room("spiky.toml", &["--frames", "360"]);
}

#[test]
fn room_rough_with_linked_gbas() {
    room("rough.toml", GBA);
}

#[test]
fn every_profile_parses() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles");
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        netplay::proxy::Profile::load(&p).unwrap();
    }
    assert!(netplay::proxy::Profile::parse("latncy_ms = 3").is_err());
    assert!(netplay::proxy::Profile::parse("loss = 2").is_err());
}
