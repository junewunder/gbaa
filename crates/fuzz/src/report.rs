//! `gbaa-fuzz report`: the workers' first sightings joined, as the spike's
//! answer: graphics found (compressed blobs plus copied ROM ranges that
//! reached palette, VRAM or OAM, the route's way of counting, and the
//! source map's symbols they fall in), code run (ROM instructions, and
//! pret's functions they fall in, against code discovery's starts), the
//! screens (`gMain.callback2`) and maps reached, CPU time, and how each
//! count grew over time (`curve.tsv`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::Path;

/// Copied runs closer than this are one range (as `gbaa-assets
/// check-load-log` joins them).
const GAP: u32 = 16;

/// The first sighting of a thing across workers: when (seconds into its
/// worker's run) and whether the route saw it.
#[derive(Clone, Copy)]
struct Seen {
    t: f64,
    route: bool,
}

fn see(map: &mut HashMap<u32, Seen>, k: u32, s: Seen) {
    map.entry(k)
        .and_modify(|o| {
            o.t = o.t.min(s.t);
            o.route |= s.route;
        })
        .or_insert(s);
}

#[derive(Default)]
struct Union {
    pcs: HashMap<u32, Seen>,
    loads: HashMap<u32, Seen>,
    packed: HashMap<u32, Seen>,
    copied: HashMap<u32, Seen>,
    screens: HashMap<u32, Seen>,
}

/// `0xADDR = { name = "N", size = S, … }` lines of one section of a source
/// map TOML: address, name, size.
fn section(toml: &str, name: &str) -> Vec<(u32, String, u32)> {
    let head = format!("[{name}]");
    let mut on = false;
    let mut v = Vec::new();
    for line in toml.lines() {
        if line.starts_with('[') {
            on = line == head;
            continue;
        }
        let Some(rest) = line.strip_prefix("0x").filter(|_| on) else {
            continue;
        };
        let Some(addr) = rest
            .split_whitespace()
            .next()
            .and_then(|a| u32::from_str_radix(a, 16).ok())
        else {
            continue;
        };
        let field = |f: &str| {
            let at = line.find(f)? + f.len();
            Some(&line[at..])
        };
        let n = field("name = \"").and_then(|s| s.split('"').next());
        let size = field("size = ")
            .and_then(|s| s.split([',', ' ', '}']).next())
            .and_then(|s| s.parse().ok());
        if let Some(n) = n {
            v.push((addr, n.to_string(), size.unwrap_or(0)));
        }
    }
    v
}

/// Ranges of `bytes` (sorted) joined across gaps under [`GAP`].
fn ranges(bytes: &[u32]) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = Vec::new();
    for &b in bytes {
        match v.last_mut() {
            Some((_, end)) if b < *end + GAP => *end = b + 1,
            _ => v.push((b, b + 1)),
        }
    }
    v
}

/// The ROM data symbol each of `addrs` (start, end) falls in.
fn symbols<'a>(syms: &'a [(u32, String, u32)], spans: &[(u32, u32)]) -> BTreeSet<&'a str> {
    let mut out = BTreeSet::new();
    for &(s, e) in spans {
        let i = syms.partition_point(|x| x.0 <= s);
        // The symbol holding the start, then any starting inside the span.
        if i > 0 {
            let (a, n, size) = &syms[i - 1];
            if s < a + size.max(&1) {
                out.insert(n.as_str());
            }
        }
        for (a, n, _) in &syms[i..] {
            if *a >= e {
                break;
            }
            out.insert(n.as_str());
        }
    }
    out
}

/// Whether a data symbol's name says it's an image or palette in pret's
/// naming (`gMonFrontPic_`, `gItemIconPalette_`, `gTilesetTiles_`, `…Gfx`),
/// not a layout, metatile or table.
fn graphic(name: &str) -> bool {
    const IS: [&str; 8] = [
        "Gfx", "gfx", "Pal", "Tiles", "Tilemap", "Pic", "Icon", "Image",
    ];
    const NOT: [&str; 4] = ["Template", "Layout", "Metatile", "Table"];
    IS.iter().any(|p| name.contains(p)) && !NOT.iter().any(|p| name.contains(p))
}

struct Counts {
    blobs: usize,
    ranges: usize,
    symbols: usize,
    graphic_symbols: usize,
    loads: usize,
    pcs: usize,
    funcs: usize,
    funcs_undiscovered: usize,
    screens: usize,
}

pub fn run(out: &Path, names: &Path, truth: &Path, disc: &Path) -> Result<(), String> {
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let mut u = Union::default();
    let mut workers = 0;
    let (mut secs, mut frames, mut runs, mut boots, mut faults) = (0.0, 0u64, 0u64, 0u64, 0u64);
    let mut maps = BTreeMap::<String, f64>::new();
    let mut dirs: Vec<_> = std::fs::read_dir(out)
        .map_err(|e| format!("{}: {e}", out.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("events.tsv").exists())
        .collect();
    dirs.sort();
    for d in &dirs {
        workers += 1;
        for line in read(&d.join("events.tsv"))?.lines().skip(1) {
            let f: Vec<&str> = line.split('\t').collect();
            let [t, phase, kind, addr, len, cb2] = f[..] else {
                continue;
            };
            let (Ok(t), Ok(addr), Ok(len), Ok(cb2)) = (
                t.parse::<f64>(),
                u32::from_str_radix(addr.trim_start_matches("0x"), 16),
                len.parse::<u32>(),
                u32::from_str_radix(cb2.trim_start_matches("0x"), 16),
            ) else {
                continue;
            };
            let s = Seen {
                t,
                route: phase == "route",
            };
            see(&mut u.screens, cb2, s);
            match kind {
                "pc" => see(&mut u.pcs, addr, s),
                "load" => see(&mut u.loads, addr, s),
                "packed" => see(&mut u.packed, addr, s),
                "copied" => {
                    for b in addr..addr + len {
                        see(&mut u.copied, b, s);
                    }
                }
                _ => {}
            }
        }
        if let Ok(text) = read(&d.join("stats.tsv"))
            && let Some(last) = text.lines().last()
        {
            let f: Vec<&str> = last.split('\t').collect();
            if f.len() >= 6 {
                secs += f[0].parse::<f64>().unwrap_or(0.0);
                frames += f[1].parse::<u64>().unwrap_or(0);
                runs += f[2].parse::<u64>().unwrap_or(0);
                boots += f[3].parse::<u64>().unwrap_or(0);
                faults += f[5].parse::<u64>().unwrap_or(0);
            }
        }
        if let Ok(text) = read(&d.join("kept.tsv")) {
            for line in text.lines().skip(1) {
                let f: Vec<&str> = line.split('\t').collect();
                if let (Some(t), Some(m)) = (f.first(), f.get(6))
                    && *m != "255.255"
                {
                    let t = t.parse::<f64>().unwrap_or(0.0);
                    let e = maps.entry(m.to_string()).or_insert(t);
                    *e = e.min(t);
                }
            }
        }
    }
    if workers == 0 {
        return Err(format!("{}: no worker's events.tsv", out.display()));
    }

    let names = read(names)?;
    let mut data: Vec<_> = section(&names, "data")
        .into_iter()
        .filter(|d| d.0 >= 0x0800_0000)
        .collect();
    data.sort_by_key(|d| d.0);
    let funcs_named: HashMap<u32, String> = section(&names, "functions")
        .into_iter()
        .map(|(a, n, _)| (a, n))
        .collect();
    // pret's functions: start (bit 0 clear) and end.
    let mut truth_fns: Vec<(u32, u32, String)> = Vec::new();
    for line in read(truth)?.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if let [a, size, name, ..] = f[..]
            && let (Ok(a), Ok(size)) = (
                u32::from_str_radix(a.trim_start_matches("0x"), 16),
                size.parse::<u32>(),
            )
        {
            truth_fns.push((a & !1, size, name.to_string()));
        }
    }
    truth_fns.sort_by_key(|f| f.0);
    truth_fns.dedup_by_key(|f| f.0);
    let ends: Vec<u32> = (0..truth_fns.len())
        .map(|i| {
            let (a, size, _) = &truth_fns[i];
            let next = truth_fns.get(i + 1).map_or(u32::MAX, |n| n.0);
            if *size > 0 { a + size } else { next }
        })
        .collect();
    let disc: BTreeSet<u32> = read(disc)?
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter_map(|a| u32::from_str_radix(a.strip_prefix("0x")?, 16).ok())
        .map(|a| a & !1)
        .collect();
    let undiscovered = truth_fns.iter().filter(|f| !disc.contains(&f.0)).count();
    let func_of = |pc: u32| -> Option<usize> {
        let i = truth_fns.partition_point(|f| f.0 <= pc).checked_sub(1)?;
        (pc < ends[i]).then_some(i)
    };

    // Counts of what was seen by `t` (and by the route if `route`).
    let counts = |keep: &dyn Fn(&Seen) -> bool| -> (Counts, Vec<String>, BTreeSet<usize>) {
        let blobs: Vec<u32> = u
            .packed
            .iter()
            .filter(|e| keep(e.1))
            .map(|e| *e.0)
            .collect();
        let mut bytes: Vec<u32> = u
            .copied
            .iter()
            .filter(|e| keep(e.1))
            .map(|e| *e.0)
            .collect();
        bytes.sort_unstable();
        let rs = ranges(&bytes);
        let mut spans: Vec<(u32, u32)> = blobs.iter().map(|&b| (b, b + 1)).collect();
        spans.extend(&rs);
        let syms = symbols(&data, &spans);
        let mut fns = BTreeSet::new();
        let mut pcs = 0;
        for (pc, s) in &u.pcs {
            if keep(s) {
                pcs += 1;
                if let Some(i) = func_of(*pc) {
                    fns.insert(i);
                }
            }
        }
        let c = Counts {
            blobs: blobs.len(),
            ranges: rs.len(),
            symbols: syms.len(),
            graphic_symbols: syms.iter().filter(|s| graphic(s)).count(),
            loads: u.loads.values().filter(|s| keep(s)).count(),
            pcs,
            funcs: fns.len(),
            funcs_undiscovered: fns
                .iter()
                .filter(|&&i| !disc.contains(&truth_fns[i].0))
                .count(),
            screens: u.screens.values().filter(|s| keep(s)).count(),
        };
        (c, syms.into_iter().map(String::from).collect(), fns)
    };
    let (route, route_syms, route_fns) = counts(&|s| s.route);
    let (all, all_syms, all_fns) = counts(&|_| true);

    let wall = secs / workers as f64;
    let mut curve = String::from(
        "secs\tcpu_hours\tgraphics\tblobs\tranges\tsymbols\tgraphic_symbols\tloads\tpcs\tfuncs\tfuncs_undiscovered\tscreens\n",
    );
    let mut t = 0.0;
    let step = (wall / 40.0).max(30.0);
    let mut rows = Vec::new();
    while t < wall + step {
        let (c, _, _) = counts(&|s: &Seen| s.t <= t);
        let _ = writeln!(
            curve,
            "{t:.0}\t{:.2}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            t * workers as f64 / 3600.0,
            c.blobs + c.ranges,
            c.blobs,
            c.ranges,
            c.symbols,
            c.graphic_symbols,
            c.loads,
            c.pcs,
            c.funcs,
            c.funcs_undiscovered,
            c.screens
        );
        rows.push((t, c));
        t += step;
    }
    std::fs::write(out.join("curve.tsv"), &curve).map_err(|e| format!("curve.tsv: {e}"))?;

    let mut r = String::new();
    let _ = writeln!(
        r,
        "# gbaa-fuzz: Emerald in the interpreter on random presses from mutated saves and RAM\n\
         workers {workers}, {wall:.0} s each: {:.2} CPU-hours, {frames} frames ({:.0} a CPU-second), \
         {runs} runs, {boots} boots of a save, {faults} runs ended by a fault\n",
        secs / 3600.0,
        frames as f64 / secs.max(1.0),
    );
    let line = |r: &mut String, what: &str, a: usize, b: usize| {
        let _ = writeln!(r, "{what:<48} {a:>8} {b:>8} {:>+8}", b as i64 - a as i64);
    };
    let _ = writeln!(r, "{:<48} {:>8} {:>8} {:>8}", "", "route", "fuzz", "new");
    line(
        &mut r,
        "graphics (blobs + copied ranges reaching screen)",
        route.blobs + route.ranges,
        all.blobs + all.ranges,
    );
    line(&mut r, "  compressed blobs", route.blobs, all.blobs);
    line(&mut r, "  copied ROM ranges", route.ranges, all.ranges);
    line(
        &mut r,
        "  source map data symbols they fall in",
        route.symbols,
        all.symbols,
    );
    line(
        &mut r,
        "    named as images or palettes",
        route.graphic_symbols,
        all.graphic_symbols,
    );
    line(
        &mut r,
        "ROM loads (op, source) in the load log",
        route.loads,
        all.loads,
    );
    line(&mut r, "ROM instructions run", route.pcs, all.pcs);
    line(&mut r, "pret functions run", route.funcs, all.funcs);
    line(
        &mut r,
        "  of them, not among discovery's starts",
        route.funcs_undiscovered,
        all.funcs_undiscovered,
    );
    line(
        &mut r,
        "screens (gMain.callback2)",
        route.screens,
        all.screens,
    );
    let _ = writeln!(
        r,
        "\npret's graphics: ~7,608 INCGFX files; source map ROM data symbols: {}. pret's functions: {}, \
         discovery's starts: {} ({undiscovered} of pret's not among them).\nmaps kept states stood in: {}",
        data.len(),
        truth_fns.len(),
        disc.len(),
        maps.len()
    );
    let _ = writeln!(
        r,
        "\n## How it grew (union of workers; t is each worker's seconds)\n"
    );
    let _ = writeln!(
        r,
        "{:>6} {:>9} {:>9} {:>8} {:>7} {:>9} {:>7} {:>8}",
        "secs", "cpu-hours", "graphics", "symbols", "images", "pcs", "funcs", "screens"
    );
    let every = (rows.len() / 12).max(1);
    for (i, (t, c)) in rows.iter().enumerate() {
        if i % every == 0 || i + 1 == rows.len() {
            let _ = writeln!(
                r,
                "{t:>6.0} {:>9.2} {:>9} {:>8} {:>7} {:>9} {:>7} {:>8}",
                t * workers as f64 / 3600.0,
                c.blobs + c.ranges,
                c.symbols,
                c.graphic_symbols,
                c.pcs,
                c.funcs,
                c.screens
            );
        }
    }
    // The last quarter's share of what was found.
    if let Some((_, first)) = rows.iter().find(|(t, _)| *t >= wall * 0.75) {
        let g = (all.blobs + all.ranges) - (first.blobs + first.ranges);
        let _ = writeln!(
            r,
            "\nthe last quarter of the run added {g} graphics ({:.1}%), {} instructions ({:.1}%)",
            100.0 * g as f64 / (all.blobs + all.ranges).max(1) as f64,
            all.pcs - first.pcs,
            100.0 * (all.pcs - first.pcs) as f64 / all.pcs.max(1) as f64
        );
    }
    let new_screens: Vec<String> = {
        let mut v: Vec<(f64, u32)> = u
            .screens
            .iter()
            .filter(|e| !e.1.route)
            .map(|e| (e.1.t, *e.0))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter()
            .map(|(t, a)| {
                let n = funcs_named.get(&a).map_or("?", String::as_str);
                format!("{t:>7.0}s {a:#010x} {n}")
            })
            .collect()
    };
    let _ = writeln!(
        r,
        "\n## Screens the route never shows ({})\n",
        new_screens.len()
    );
    for s in &new_screens {
        let _ = writeln!(r, "{s}");
    }
    let new_syms: Vec<&String> = all_syms
        .iter()
        .filter(|s| !route_syms.contains(s))
        .collect();
    let _ = writeln!(
        r,
        "\n## Data symbols reached past the route ({})\n",
        new_syms.len()
    );
    for s in &new_syms {
        let _ = writeln!(r, "{s}");
    }
    let undisc: Vec<&str> = all_fns
        .iter()
        .filter(|&&i| !disc.contains(&truth_fns[i].0) && !route_fns.contains(&i))
        .map(|&i| truth_fns[i].2.as_str())
        .collect();
    let _ = writeln!(
        r,
        "\n## Functions run past the route that discovery doesn't start ({})\n",
        undisc.len()
    );
    for f in &undisc {
        let _ = writeln!(r, "{f}");
    }
    let _ = writeln!(r, "\n## Maps\n");
    for m in maps.keys() {
        let _ = write!(r, "{m} ");
    }
    let _ = writeln!(r);
    std::fs::write(out.join("report.txt"), &r).map_err(|e| format!("report.txt: {e}"))?;
    print!("{}", r.split("\n## Screens").next().unwrap_or(&r));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_join_across_small_gaps() {
        let bytes = [10, 11, 12, 20, 40, 41];
        assert_eq!(ranges(&bytes), vec![(10, 21), (40, 42)]);
    }

    #[test]
    fn spans_fall_in_symbols() {
        let toml = "[functions]\n0x080003a5 = { name = \"AgbMain\", size = 288 }\n\
                    [data]\n0x08100000 = { name = \"gA\", size = 64, type = \"u8 [64]\" }\n\
                    0x08100040 = { name = \"gB\", size = 32 }\n0x08100100 = { name = \"gC\" }\n";
        let data = section(toml, "data");
        assert_eq!(data[0], (0x0810_0000, "gA".to_string(), 64));
        assert_eq!(data[2], (0x0810_0100, "gC".to_string(), 0));
        assert_eq!(section(toml, "functions")[0].1, "AgbMain");
        let got = symbols(
            &data,
            &[(0x0810_0010, 0x0810_0050), (0x0810_0080, 0x0810_0081)],
        );
        assert_eq!(got.into_iter().collect::<Vec<_>>(), ["gA", "gB"]);
    }

    #[test]
    fn graphics_by_name() {
        assert!(graphic("gMonFrontPic_Mudkip"));
        assert!(graphic("gItemIconPalette_Potion"));
        assert!(!graphic("gMetatiles_General"));
        assert!(!graphic("OldaleTown_Layout"));
    }
}
