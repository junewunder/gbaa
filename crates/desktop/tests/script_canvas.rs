//! The canvas check (`just script-check`): mGBA's canvas demos
//! (`res/scripts/demos` at fd9dc5c, fetched by `scripts/fetch-mgba.sh`
//! into `GBAA_SCRIPT_DEMOS`) run headless on Emerald (`GBAA_SCRIPT_ROM`)
//! and draw over its picture in `--dump-at`'s PNGs: `text-demo.lua`'s
//! nine labels in their colours, `logo-bounce.lua`'s logo, moving.
//! Ignored by `cargo test`: it needs the ROM and the demos.

use std::path::{Path, PathBuf};
use std::process::Command;

fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .unwrap_or_else(|| panic!("{name} is unset (just script-check sets it)"))
        .into()
}

/// The RGB pixels of frames `at` of a headless run with `script`, if any.
fn frames(script: Option<&Path>, at: &[u32]) -> Vec<Vec<[u8; 3]>> {
    let rom = env_path("GBAA_SCRIPT_ROM");
    // Unique to the call: the tests run at once.
    static RUN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gbaa-canvas-{}-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        script.map_or("none".into(), |s| s.file_stem().unwrap().to_string_lossy())
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let last = at.iter().max().unwrap();
    let list: Vec<String> = at.iter().map(u32::to_string).collect();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gbaa-desktop"));
    cmd.arg(&rom)
        .args([
            "--headless",
            "--timing",
            "exact",
            "--frames",
            &last.to_string(),
        ])
        .args(["--dump-at", &list.join(",")])
        .arg("--dump-dir")
        .arg(&dir);
    if let Some(s) = script {
        cmd.arg("--script").arg(s);
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "gbaa-desktop failed: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !stdout.contains("error:") && !stdout.contains("warn:"),
        "the script complained:\n{stdout}"
    );
    let got = at
        .iter()
        .map(|n| {
            let file = std::fs::File::open(dir.join(format!("frame-{n}.png"))).unwrap();
            let mut r = png::Decoder::new(std::io::BufReader::new(file))
                .read_info()
                .unwrap();
            let mut buf = vec![0; r.output_buffer_size().unwrap()];
            let info = r.next_frame(&mut buf).unwrap();
            buf[..info.buffer_size()].as_chunks::<3>().0.to_vec()
        })
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    got
}

/// Whether `a` is within 60 of `b` in each channel.
fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    (0..3).all(|i| a[i].abs_diff(b[i]) <= 60)
}

/// The pixels that differ, as (x, y).
fn changed(a: &[[u8; 3]], b: &[[u8; 3]]) -> Vec<(usize, usize)> {
    (0..a.len())
        .filter(|&i| a[i] != b[i])
        .map(|i| (i % 240, i / 240))
        .collect()
}

#[test]
#[ignore = "needs Emerald and mGBA's demos: just script-check"]
fn canvas_demos_draw_over_emerald() {
    let demos = env_path("GBAA_SCRIPT_DEMOS");
    let base = frames(None, &[400, 600]);

    // Each label's colour (near it: small glyphs are rarely fully
    // covered), inside its ninth of the screen.
    let text = &frames(Some(&demos.join("text-demo.lua")), &[600])[0];
    let labels: [(&str, [u8; 3], usize, usize); 8] = [
        ("top center", [0, 255, 255], 1, 0),
        ("top right", [255, 0, 255], 2, 0),
        ("center left", [255, 255, 0], 0, 1),
        ("center center", [255, 0, 0], 1, 1),
        ("center right", [0, 255, 0], 2, 1),
        ("bottom left", [0, 0, 255], 0, 2),
        ("bottom center", [128, 128, 128], 1, 2),
        // Black, outlined in white over a dark ground: its outline.
        ("bottom right", [255, 255, 255], 2, 2),
    ];
    for (name, rgb, col, row) in labels {
        let n = (0..text.len())
            .filter(|&i| near(text[i], rgb) && !near(base[1][i], rgb))
            .filter(|&i| (i % 240) / 80 == col && (i / 240) * 3 / 160 == row)
            .count();
        assert!(n >= 20, "text-demo: {name} has {n} pixels of {rgb:?}");
    }
    let n = changed(&base[1], text).len();
    assert!(n > 2000, "text-demo changed only {n} pixels");

    // The logo (40×29, partly transparent) somewhere in each, and moved.
    let logo = frames(Some(&demos.join("logo-bounce.lua")), &[400, 600]);
    let boxes: Vec<_> = (0..2)
        .map(|i| {
            let c = changed(&base[i], &logo[i]);
            assert!(c.len() > 300, "logo-bounce: {} pixels at {i}", c.len());
            let (xs, ys): (Vec<_>, Vec<_>) = c.into_iter().unzip();
            let b = (
                *xs.iter().min().unwrap(),
                *ys.iter().min().unwrap(),
                *xs.iter().max().unwrap(),
                *ys.iter().max().unwrap(),
            );
            assert!(
                b.2 - b.0 < 40 && b.3 - b.1 < 29,
                "logo-bounce: drawn over {b:?}, more than the logo"
            );
            b
        })
        .collect();
    assert_ne!(boxes[0], boxes[1], "logo-bounce: the logo didn't move");
}

/// The PNG of frame `n` of a headless run with `script`: its width and
/// RGB pixels.
fn dump(script: &Path, n: u32) -> (usize, Vec<[u8; 3]>) {
    let rom = env_path("GBAA_SCRIPT_ROM");
    let dir = std::env::temp_dir().join(format!("gbaa-sidebar-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_gbaa-desktop"))
        .arg(&rom)
        .args([
            "--headless",
            "--timing",
            "exact",
            "--frames",
            &n.to_string(),
        ])
        .args(["--dump-at", &n.to_string()])
        .arg("--dump-dir")
        .arg(&dir)
        .arg("--script")
        .arg(script)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let file = std::fs::File::open(dir.join(format!("frame-{n}.png"))).unwrap();
    let mut r = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        info.width as usize,
        buf[..info.buffer_size()].as_chunks::<3>().0.to_vec(),
    )
}

#[test]
#[ignore = "needs Emerald: just script-check"]
fn a_sidebar_panel_is_drawn_right_of_emerald() {
    let panel = Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/fixtures/sidebar/panel.lua");
    let base = &frames(None, &[600])[0];
    let (width, got) = dump(&panel, 600);
    // The picture and an 80-pixel sidebar right of it.
    assert_eq!((width, got.len()), (320, 320 * 160));
    let at = |x: usize, y: usize| got[y * width + x];
    for y in 0..160 {
        assert_eq!(&got[y * width..][..240], &base[y * 240..][..240], "row {y}");
    }
    assert_eq!(at(240, 80), [0xE0; 3], "the border");
    assert_eq!(at(260, 80), [0x20, 0x30, 0x40], "the ground");
    assert_eq!(at(260, 12), [0xE0, 0x30, 0x30], "the bar");
}
