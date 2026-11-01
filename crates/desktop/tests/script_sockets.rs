//! The socket API's check (`just script-check`): mGBA 0.10.5's
//! `res/scripts/socketserver.lua`, run headless (sandboxed, so on
//! loopback), accepts a client, logs what it sends and sends it the keys
//! when they change. Ignored by `cargo test`: it needs the pinned mGBA
//! checkout (`GBAA_SCRIPT_MGBA`, from `scripts/fetch-mgba.sh`).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// About 10 s headless; A is pressed at frame 15000, well after the client
/// is in.
const FRAMES: &str = "20000";
const PRESS: &str = "a@15000+30";
/// The ports socketserver.lua tries: 8888, then the next while in use.
const PORTS: std::ops::Range<u16> = 8888..8900;

/// A ROM whose game is `b .`, with a header.
fn rom() -> PathBuf {
    let mut r = vec![0u8; 0x200];
    r[..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
    r[0xA0..0xAC].copy_from_slice(b"SOCKETTEST\0\0");
    r[0xAC..0xB0].copy_from_slice(b"BTST");
    let p = std::env::temp_dir().join(format!("gbaa-socket-check-{}.gba", std::process::id()));
    std::fs::write(&p, r).unwrap();
    p
}

fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(200)).is_ok()
}

#[test]
#[ignore = "needs mGBA's checkout: just script-check"]
fn socketserver_lua_answers_a_loopback_client() {
    let mgba: PathBuf = std::env::var_os("GBAA_SCRIPT_MGBA")
        .expect("GBAA_SCRIPT_MGBA is unset (just script-check sets it)")
        .into();
    let rom = rom();
    // Ports someone else holds aren't the server's.
    let taken: Vec<u16> = PORTS.filter(|&p| listening(p)).collect();
    let run = Command::new(env!("CARGO_BIN_EXE_gbaa-desktop"))
        .arg(&rom)
        .args([
            "--headless",
            "--timing",
            "exact",
            "--frames",
            FRAMES,
            "--input",
            PRESS,
        ])
        .arg("--script")
        .arg(mgba.join("res/scripts/socketserver.lua"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let c = loop {
        let free = PORTS.filter(|p| !taken.contains(p));
        if let Some(c) = free
            .into_iter()
            .find_map(|p| TcpStream::connect(("127.0.0.1", p)).ok())
        {
            break c;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "no server came up"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    (&c).write_all(b"hello\n").unwrap();
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut line = String::new();
    BufReader::new(&c).read_line(&mut line).unwrap();
    drop(c);
    let out = run.wait_with_output().unwrap();
    let _ = std::fs::remove_file(&rom);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "gbaa-desktop failed: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    // KEY_NAMES' order: A first, then B, Select, Start, the D-pad, R, L.
    assert_eq!(line, "[A         ]\n", "the keys line; console:\n{stdout}");
    for want in [
        "log: Socket Server Test: Listening on port",
        // Its own wording: every log line says "Received".
        "log: Socket 1 Received: Connected",
        "log: Socket 1 Received: hello",
        "error: Socket 1 Error: disconnected",
    ] {
        assert!(stdout.contains(want), "no {want:?} in:\n{stdout}");
    }
}
