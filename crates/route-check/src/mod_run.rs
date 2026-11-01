//! `route-check --mod-run STOCK MOD [--dump-at N,…] ROUTE.toml`: a mod's
//! frames beside stock's (`just mod-run`, docs/research/agent-first-mod.md,
//! item 3). Runs the stock build and the modded one (each a binary,
//! optionally followed by its own flags) along the route, dumping the
//! frames `--dump-at` names (default: the route's checkpoints), and for
//! each pair says whether they are the same; for a pair that differs it
//! writes `out/mod-run/<route>/side-N.png`: stock, the mod, and stock
//! dimmed with the differing pixels in red, left to right. A view, not a
//! check: differing frames are what a mod is for, so it fails only when a
//! build can't run.

use crate::route::{Route, Vars};
use crate::same_states::{points, states};
use crate::{Stop, Stop::*};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

/// The gap between panels, in pixels, and its grey.
const GAP: usize = 4;
const GAP_GREY: u8 = 0x40;

/// Run both builds along `route` and compare their frames at `at` (empty:
/// the route's checkpoints). Ok: how many pairs differ, and where.
pub fn run(
    stock: &str,
    modded: &str,
    at: &[u64],
    route: &str,
    vars: &Vars,
) -> Result<String, Stop> {
    let r = Route::read(Path::new(route), vars).map_err(Bad)?;
    let name = Path::new(route)
        .file_stem()
        .map_or(String::new(), |s| s.to_string_lossy().into_owned());
    let out = PathBuf::from("out/mod-run").join(&name);
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    }
    let mut frames = if at.is_empty() {
        points(&r, 0)
    } else {
        BTreeMap::new()
    };
    for &f in at {
        let named = r.checkpoints.iter().find(|c| c.frame == f);
        frames.insert(f, named.map_or(String::new(), |c| c.name.clone()));
    }
    if frames.is_empty() {
        return Err(Bad(format!("{route} has no checkpoints: give --dump-at")));
    }
    let (ds, dm) = (out.join("stock"), out.join("mod"));
    states(stock, &r, &frames, &ds, false)?;
    states(modded, &r, &frames, &dm, false)?;
    // A modded build's resume count (`gbaa-desktop`'s `mod:` line).
    let log = std::fs::read_to_string(dm.join("run.log")).unwrap_or_default();
    for l in log.lines().filter(|l| l.starts_with("mod: ")) {
        println!("{name}: {l}");
    }
    let (mut same, mut differ) = (Vec::new(), Vec::new());
    for (&f, label) in &frames {
        let at = if label.is_empty() {
            format!("frame {f}")
        } else {
            format!("{label} (frame {f})")
        };
        let png = format!("frame-{f}.png");
        let (ws, hs, ps) = read(&ds.join(&png))?;
        let (wm, hm, pm) = read(&dm.join(&png))?;
        if (ws, hs) != (wm, hm) {
            return Err(Unrun(format!(
                "frame {f}: stock is {ws}x{hs} but the mod {wm}x{hm}"
            )));
        }
        let n = ps
            .chunks(3)
            .zip(pm.chunks(3))
            .filter(|(a, b)| a != b)
            .count();
        if n == 0 {
            println!("{name} {at}: the same");
            same.push(f.to_string());
            continue;
        }
        let side = out.join(format!("side-{f}.png"));
        let (w, h, px) = side_by_side(ws, hs, &ps, &pm);
        write(&side, w, h, &px)?;
        println!("{name} {at}: {n} pixels differ: {}", side.display());
        differ.push(format!("{f} ({n} pixels)"));
    }
    let list = |v: &[String]| {
        if v.is_empty() {
            "none".to_string()
        } else {
            v.join(", ")
        }
    };
    Ok(format!(
        "differ at {}; the same at {}; side-by-side PNGs in {}",
        list(&differ),
        list(&same),
        out.display()
    ))
}

/// Stock, the mod, and stock dimmed with the differing pixels in red, as
/// one RGB image with a grey gap between panels: its width, height and
/// bytes.
fn side_by_side(w: u32, h: u32, stock: &[u8], modded: &[u8]) -> (u32, u32, Vec<u8>) {
    let (w, h) = (w as usize, h as usize);
    let wide = 3 * w + 2 * GAP;
    let mut px = Vec::with_capacity(wide * h * 3);
    let gap = [GAP_GREY; 3 * GAP];
    for y in 0..h {
        let row = |p: &[u8]| -> Vec<u8> { p[y * w * 3..(y + 1) * w * 3].to_vec() };
        let (s, m) = (row(stock), row(modded));
        px.extend(&s);
        px.extend(gap);
        px.extend(&m);
        px.extend(gap);
        for (a, b) in s.chunks(3).zip(m.chunks(3)) {
            if a == b {
                let grey = ((a[0] as u32 + a[1] as u32 + a[2] as u32) / 9 + 40) as u8;
                px.extend([grey; 3]);
            } else {
                px.extend([255, 0, 0]);
            }
        }
    }
    (wide as u32, h as u32, px)
}

/// An RGB PNG's width, height and bytes.
fn read(path: &Path) -> Result<(u32, u32, Vec<u8>), Stop> {
    let err = |e: &dyn std::fmt::Display| Unrun(format!("{}: {e}", path.display()));
    let mut dec = png::Decoder::new(BufReader::new(File::open(path).map_err(|e| err(&e))?));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| err(&e))?;
    let mut buf = vec![0; r.output_buffer_size().ok_or_else(|| err(&"too large"))?];
    let info = r.next_frame(&mut buf).map_err(|e| err(&e))?;
    if info.color_type != png::ColorType::Rgb {
        return Err(err(&"not an RGB image"));
    }
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

fn write(path: &Path, w: u32, h: u32, px: &[u8]) -> Result<(), Stop> {
    let err = |e: &dyn std::fmt::Display| Unrun(format!("{}: {e}", path.display()));
    let mut enc = png::Encoder::new(
        BufWriter::new(File::create(path).map_err(|e| err(&e))?),
        w,
        h,
    );
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| err(&e))?;
    wr.write_image_data(px).map_err(|e| err(&e))?;
    wr.finish().map_err(|e| err(&e))
}

#[cfg(test)]
mod tests {
    use super::{GAP, GAP_GREY, side_by_side};

    #[test]
    fn three_panels_with_the_difference_in_red() {
        // 2x1 images: the second pixel differs.
        let stock = [10, 10, 10, 1, 2, 3];
        let modded = [10, 10, 10, 9, 9, 9];
        let (w, h, px) = side_by_side(2, 1, &stock, &modded);
        assert_eq!((w, h), (6 + 2 * GAP as u32, 1));
        assert_eq!(px.len(), w as usize * 3);
        assert_eq!(&px[..6], &stock);
        assert_eq!(&px[6..6 + 3 * GAP], &[GAP_GREY; 3 * GAP]);
        let m = 6 + 3 * GAP;
        assert_eq!(&px[m..m + 6], &modded);
        let d = m + 6 + 3 * GAP;
        assert_eq!(&px[d..d + 3], &[43; 3]);
        assert_eq!(&px[d + 3..d + 6], &[255, 0, 0]);
    }
}
