//! `route-check --lift-mod-run STOCK MOD [--sides N] ROUTE.toml`: a lift
//! mod's frames beside the stock lift's (`just lift-mod-run`, unit
//! `native/lift-mod-run`; the retired mod view's `--mod-run`, for the
//! lift). STOCK and MOD are native programs (each a binary, optionally
//! followed by its flags), run headless along the route as the native
//! check runs them ([`crate::native::runner`]), every frame hashed. It
//! lists the frames whose pictures differ, as runs (`31-600`), then runs
//! both again dumping up to N of them (`--sides`, default
//! [`SIDES`]: the first of each run, then spread over the rest) and writes
//! `out/lift-mod-run/<route>/side-N.png` for each: stock, the mod, and
//! stock dimmed with the differing pixels in red. For each it prints the
//! differing pixels' bounding box and writes `zoom-N.png`: that box (and
//! a margin of [`MARGIN`]) cut from stock and from the mod, scaled up
//! (native/lift-mod-run-zoom; a 4-digit HUD edit is a few dozen pixels).
//! A view, not a check: it fails only when a program can't run.

use crate::native::runner;
use crate::route::{Route, Vars};
use crate::{Stop, Stop::*};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

/// The gap between panels, in pixels, and its grey.
const GAP: usize = 4;
const GAP_GREY: u8 = 0x40;

/// How many side-by-side PNGs to write unless `--sides` says.
pub const SIDES: usize = 16;

/// Pixels of context kept around the differing pixels' box in a zoom.
const MARGIN: u32 = 4;

/// A zoom's panels are scaled up to about this many pixels on their
/// longer side, by a whole factor of 1 to [`ZOOM_MAX`].
const ZOOM_TO: u32 = 240;
const ZOOM_MAX: u32 = 16;

pub fn run(
    stock: &str,
    modded: &str,
    sides: usize,
    route: &str,
    vars: &Vars,
) -> Result<String, Stop> {
    let r = Route::read(Path::new(route), vars).map_err(Bad)?;
    if r.state.is_some() {
        return Err(Bad(format!(
            "{route} starts from a save state, which a native program can't load"
        )));
    }
    let name = Path::new(route)
        .file_stem()
        .map_or(String::new(), |s| s.to_string_lossy().into_owned());
    let out = PathBuf::from("out/lift-mod-run").join(&name);
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    }
    std::fs::create_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    runner(&out, "stock", stock, &r, &[], false)?;
    runner(&out, "mod", modded, &r, &[], false)?;
    let hs = hashes(&out.join("stock-hashes.txt"))?;
    let hm = hashes(&out.join("mod-hashes.txt"))?;
    let frames = r.frames();
    let differ: Vec<u64> = (1..=frames)
        .filter(|f| !hs.contains_key(f) || hs.get(f) != hm.get(f))
        .collect();
    if let Some(f) = differ
        .iter()
        .find(|f| !hs.contains_key(f) || !hm.contains_key(f))
    {
        return Err(Fail(format!(
            "frame {f}: a program presented no such frame ({} hashes)",
            out.display()
        )));
    }
    if differ.is_empty() {
        return Ok(format!("no frame differs, of {frames}"));
    }
    let runs = ranges(&differ);
    println!(
        "{name}: {} of {frames} frames differ: {}",
        differ.len(),
        runs.join(", ")
    );
    let pick = picks(&differ, sides);
    let at: Vec<String> = pick.iter().map(u64::to_string).collect();
    let (ds, dm) = (out.join("stock"), out.join("mod"));
    let with_dump = |cmd: &str, dir: &Path| {
        format!(
            "{cmd} --dump-at {} --dump-dir {}",
            at.join(","),
            dir.display()
        )
    };
    if !pick.is_empty() {
        runner(&out, "stock-dump", &with_dump(stock, &ds), &r, &[], false)?;
        runner(&out, "mod-dump", &with_dump(modded, &dm), &r, &[], false)?;
    }
    for &f in &pick {
        let png = format!("frame-{f}.png");
        let (ws, hgt, ps) = read(&ds.join(&png))?;
        let (wm, hm, pm) = read(&dm.join(&png))?;
        if (ws, hgt) != (wm, hm) {
            return Err(Unrun(format!(
                "frame {f}: stock is {ws}x{hgt} but the mod {wm}x{hm}"
            )));
        }
        let n = ps
            .chunks(3)
            .zip(pm.chunks(3))
            .filter(|(a, b)| a != b)
            .count();
        let side = out.join(format!("side-{f}.png"));
        let (w, h, px) = side_by_side(ws, hgt, &ps, &pm);
        write(&side, w, h, &px)?;
        println!("{name} frame {f}: {n} pixels differ: {}", side.display());
        if let Some(b) = bbox(ws, hgt, &ps, &pm) {
            let z = out.join(format!("zoom-{f}.png"));
            let (zw, zh, zx, k) = zoom(ws, hgt, &ps, &pm, b);
            write(&z, zw, zh, &zx)?;
            let (x0, y0, x1, y1) = b;
            println!(
                "{name} frame {f}: they differ in x {x0}-{x1}, y {y0}-{y1} ({}x{}): {} at {k}x",
                x1 - x0 + 1,
                y1 - y0 + 1,
                z.display()
            );
        }
    }
    Ok(format!(
        "{} of {frames} frames differ: {}; side-by-side PNGs of {} in {}",
        differ.len(),
        runs.join(", "),
        pick.len(),
        out.display()
    ))
}

/// The box holding every pixel where two `w`x`h` RGB images differ:
/// `(x0, y0, x1, y1)`, inclusive. None when they are the same.
fn bbox(w: u32, h: u32, a: &[u8], b: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let mut bx: Option<(u32, u32, u32, u32)> = None;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 3) as usize;
            if a[i..i + 3] != b[i..i + 3] {
                bx = Some(match bx {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    bx
}

/// Box `b` grown by [`MARGIN`] (inside the frame), cut from stock and the
/// mod and scaled up by a whole factor, side by side with a grey gap: the
/// zoom's width, height, bytes and factor.
fn zoom(
    w: u32,
    h: u32,
    stock: &[u8],
    modded: &[u8],
    b: (u32, u32, u32, u32),
) -> (u32, u32, Vec<u8>, u32) {
    let x0 = b.0.saturating_sub(MARGIN);
    let y0 = b.1.saturating_sub(MARGIN);
    let x1 = (b.2 + MARGIN).min(w - 1);
    let y1 = (b.3 + MARGIN).min(h - 1);
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let k = (ZOOM_TO / cw.max(ch)).clamp(1, ZOOM_MAX);
    let (pw, ph) = (cw * k, ch * k);
    let wide = 2 * pw + GAP as u32;
    let mut px = Vec::with_capacity((wide * ph * 3) as usize);
    for y in 0..ph {
        let sy = y0 + y / k;
        for (n, img) in [stock, modded].into_iter().enumerate() {
            if n == 1 {
                px.extend([GAP_GREY; 3 * GAP]);
            }
            for x in 0..pw {
                let i = ((sy * w + x0 + x / k) * 3) as usize;
                px.extend(&img[i..i + 3]);
            }
        }
    }
    (wide, ph, px, k)
}

/// A `--frame-hashes` file's hashes by frame.
fn hashes(path: &Path) -> Result<BTreeMap<u64, String>, Stop> {
    let text =
        std::fs::read_to_string(path).map_err(|e| Unrun(format!("{}: {e}", path.display())))?;
    let mut m = BTreeMap::new();
    for l in text.lines() {
        let mut w = l.split_whitespace();
        if let (Some("frame"), Some(n), Some("hash"), Some(h)) =
            (w.next(), w.next(), w.next(), w.next())
            && let Ok(n) = n.parse()
        {
            m.insert(n, h.to_string());
        }
    }
    Ok(m)
}

/// Sorted frames as runs: `31-40`, `52`.
fn ranges(frames: &[u64]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < frames.len() {
        let mut j = i;
        while j + 1 < frames.len() && frames[j + 1] == frames[j] + 1 {
            j += 1;
        }
        out.push(if i == j {
            frames[i].to_string()
        } else {
            format!("{}-{}", frames[i], frames[j])
        });
        i = j + 1;
    }
    out
}

/// Up to `n` of the sorted frames `differ` to draw: the first of each run,
/// then the rest spread evenly over what's left; sorted.
fn picks(differ: &[u64], n: usize) -> Vec<u64> {
    let mut pick: Vec<u64> = differ
        .iter()
        .enumerate()
        .filter(|&(i, &f)| i == 0 || differ[i - 1] + 1 != f)
        .map(|(_, &f)| f)
        .take(n)
        .collect();
    let rest: Vec<u64> = differ
        .iter()
        .copied()
        .filter(|f| !pick.contains(f))
        .collect();
    let room = n - pick.len();
    if rest.len() <= room {
        pick.extend(&rest);
    } else {
        pick.extend((1..=room).map(|k| rest[k * rest.len() / room - 1]));
    }
    pick.sort_unstable();
    pick
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
    use super::{GAP, GAP_GREY, MARGIN, bbox, picks, ranges, side_by_side, zoom};

    #[test]
    fn the_box_holds_every_differing_pixel() {
        // 4x3 images: (1, 0) and (2, 2) differ.
        let a = vec![0u8; 4 * 3 * 3];
        let mut b = a.clone();
        b[3] = 1;
        b[(2 * 4 + 2) * 3 + 2] = 9;
        assert_eq!(bbox(4, 3, &a, &b), Some((1, 0, 2, 2)));
        assert_eq!(bbox(4, 3, &a, &a), None);
    }

    #[test]
    fn the_zoom_scales_the_box_and_its_margin() {
        // A 40x30 frame differing at one pixel, (20, 10): the box grows
        // to 9x9, scaled by 240 / 9 = 26, held to 16.
        let (w, h) = (40u32, 30u32);
        let a = vec![0u8; (w * h * 3) as usize];
        let mut b = a.clone();
        let i = ((10 * w + 20) * 3) as usize;
        b[i..i + 3].copy_from_slice(&[255, 255, 255]);
        let (zw, zh, px, k) = zoom(w, h, &a, &b, (20, 10, 20, 10));
        let side = (2 * MARGIN + 1) * 16;
        assert_eq!((zw, zh, k), (2 * side + GAP as u32, side, 16));
        assert_eq!(px.len(), (zw * zh * 3) as usize);
        // The mod's panel has the white pixel in its middle, 16x16.
        let at = |x: u32, y: u32| {
            let i = ((y * zw + x) * 3) as usize;
            px[i]
        };
        let m = side + GAP as u32;
        assert_eq!(at(m + MARGIN * 16, MARGIN * 16), 255);
        assert_eq!(at(m + MARGIN * 16 + 15, MARGIN * 16 + 15), 255);
        assert_eq!(at(m + MARGIN * 16 + 16, MARGIN * 16), 0);
        assert_eq!(at(MARGIN * 16, MARGIN * 16), 0);
        // A box at the frame's corner keeps the margin inside it.
        let (zw, zh, _, k) = zoom(w, h, &a, &a, (0, 0, 0, 0));
        assert_eq!((zw, zh, k), (2 * 5 * 16 + GAP as u32, 5 * 16, 16));
    }

    #[test]
    fn frames_list_as_runs() {
        assert_eq!(ranges(&[3, 4, 5, 9, 11, 12]), ["3-5", "9", "11-12"]);
        assert!(ranges(&[]).is_empty());
    }

    #[test]
    fn picks_take_each_run_first_then_spread() {
        // One run of 570: its first, then 15 more spread over the rest.
        let all: Vec<u64> = (31..=600).collect();
        let p = picks(&all, 16);
        assert_eq!(p.len(), 16);
        assert_eq!(p[0], 31);
        assert!(*p.last().unwrap() > 560);
        // Fewer than asked: all of them.
        assert_eq!(picks(&[5, 9], 16), [5, 9]);
        // More runs than room: the first runs' starts.
        assert_eq!(picks(&[1, 3, 5, 7], 2), [1, 3]);
        assert!(picks(&[1, 2], 0).is_empty());
    }

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
