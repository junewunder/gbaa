//! `route-check --lift-mod-run STOCK MOD [--sides N] ROUTE.toml`: a lift
//! mod's frames beside the stock lift's (`just lift-mod-run`, unit
//! `native/lift-mod-run`; the mod view's [`crate::mod_run`], for the
//! lift). STOCK and MOD are native programs (each a binary, optionally
//! followed by its flags), run headless along the route as the native
//! check runs them ([`crate::native::runner`]), every frame hashed. It
//! lists the frames whose pictures differ, as runs (`31-600`), then runs
//! both again dumping up to N of them (`--sides`, default
//! [`SIDES`]: the first of each run, then spread over the rest) and writes
//! `out/lift-mod-run/<route>/side-N.png` for each: stock, the mod, and
//! stock dimmed with the differing pixels in red. A view, not a check: it
//! fails only when a program can't run.

use crate::mod_run::{read, side_by_side, write};
use crate::native::runner;
use crate::route::{Route, Vars};
use crate::{Stop, Stop::*};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How many side-by-side PNGs to write unless `--sides` says.
pub const SIDES: usize = 16;

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
    }
    Ok(format!(
        "{} of {frames} frames differ: {}; side-by-side PNGs of {} in {}",
        differ.len(),
        runs.join(", "),
        pick.len(),
        out.display()
    ))
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

#[cfg(test)]
mod tests {
    use super::{picks, ranges};

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
}
