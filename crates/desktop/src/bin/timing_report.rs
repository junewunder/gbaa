//! `gbaa-timing-report DIR VARIANT…`: what `just timing-lab` found
//! (docs/research/why-timing-is-runtime.md). Reads DIR/base.tsv (the
//! timing-lab build's stall log of the real run; `baked` and `table` were
//! priced from DIR/train.tsv, the same route's first frames) and prints how each
//! memory instruction's cost varies, and from what, and how far a static
//! table of costs gets. Then compares each variant's run with the real
//! one: DIR/NAME.out (`scripts/kirby/timing.lua`'s lines), NAME.hashes
//! (`--frame-hashes`), NAME.wav, NAME.stats (the timing-lab's misses) and
//! NAME/frame-N.png (`--dump-at`), writing
//! a picture beside each frame that differs (DIR/NAME/diff-N.png: real,
//! variant, difference).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// One row of the stall log ([`desktop`'s `timing_lab`]).
#[derive(Clone, Copy)]
struct Row {
    pc: u32,
    data: i32,
    cart: bool,
    waitcnt: u16,
    ahead: u32,
    prev: u32,
    cost: i32,
    count: u64,
}

fn read_log(path: &Path) -> Result<Vec<Row>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rows = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let bad = || format!("{}: bad row {line:?}", path.display());
        let hex = |s: &str| u32::from_str_radix(s, 16).map_err(|_| bad());
        let num = |s: &str| s.parse::<i64>().map_err(|_| bad());
        if f.len() != 9 {
            return Err(bad());
        }
        rows.push(Row {
            pc: hex(f[0])?,
            data: num(f[2])? as i32,
            cart: f[3] == "1",
            waitcnt: hex(f[4])? as u16,
            ahead: num(f[5])? as u32,
            prev: hex(f[6])?,
            cost: num(f[7])? as i32,
            count: num(f[8])? as u64,
        });
    }
    Ok(rows)
}

/// How a table keyed by `key` prices the log: the executions and cycles
/// it gets wrong, each key's cost being its most common one.
fn mispriced<K: std::hash::Hash + Eq>(rows: &[Row], key: impl Fn(&Row) -> K) -> (u64, u64) {
    let mut groups: HashMap<K, BTreeMap<i32, u64>> = HashMap::new();
    for r in rows {
        *groups.entry(key(r)).or_default().entry(r.cost).or_default() += r.count;
    }
    let (mut runs, mut cycles) = (0, 0);
    for costs in groups.values() {
        let (&best, _) = costs.iter().max_by_key(|&(c, n)| (*n, -c)).unwrap();
        for (&c, &n) in costs {
            if c != best {
                runs += n;
                cycles += n * c.abs_diff(best) as u64;
            }
        }
    }
    (runs, cycles)
}

fn hist<K: Ord + Copy>(rows: &[&Row], key: impl Fn(&Row) -> K) -> BTreeMap<K, u64> {
    let mut h = BTreeMap::new();
    for r in rows {
        *h.entry(key(r)).or_default() += r.count;
    }
    h
}

fn show<K: std::fmt::Display>(h: &BTreeMap<K, u64>) -> String {
    let total: u64 = h.values().sum();
    h.iter()
        .map(|(k, n)| format!("{k}: {:.0}%", 100.0 * *n as f64 / total as f64))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A frame's cycles: 228 lines of 1232.
const FRAME: i64 = 228 * 1232;

fn sites(rows: &[Row], frames: Option<usize>) {
    let total: u64 = rows.iter().map(|r| r.count).sum();
    let mut by_site: BTreeMap<u32, Vec<&Row>> = BTreeMap::new();
    for r in rows {
        by_site.entry(r.pc).or_default().push(r);
    }
    let waitcnts = hist(&rows.iter().collect::<Vec<_>>(), |r| r.waitcnt);
    println!("## The stall log\n");
    println!(
        "{} memory instructions run from ROM ({} sites), {} cases; WAITCNT values seen: {}",
        total,
        by_site.len(),
        rows.len(),
        waitcnts
            .iter()
            .map(|(w, n)| format!("{w:04x} ({n} runs)"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let cycles: i64 = rows.iter().map(|r| r.cost as i64 * r.count as i64).sum();
    let data: i64 = rows.iter().map(|r| r.data as i64 * r.count as i64).sum();
    println!(
        "their data accesses: {data} cycles before the buffer's overlap, {cycles} after (the stalls)"
    );
    if let Some(frames) = frames {
        let per = |c: i64| {
            let c = c as f64 / frames as f64;
            format!(
                "{c:.0} cycles a frame ({:.1}% of its {FRAME})",
                100.0 * c / FRAME as f64
            )
        };
        println!(
            "over {frames} frames: the stalls {}; with the buffer off, the accesses {}; so the buffer saves {}",
            per(cycles),
            per(data),
            per(data - cycles)
        );
    }
    // How many costs each site has.
    let mut ncosts: BTreeMap<usize, (usize, u64)> = BTreeMap::new();
    let mut why: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    let mut varying = Vec::new();
    for (pc, rs) in &by_site {
        let costs = hist(rs, |r| r.cost);
        let runs: u64 = rs.iter().map(|r| r.count).sum();
        let e = ncosts.entry(costs.len()).or_default();
        e.0 += 1;
        e.1 += runs;
        if costs.len() == 1 {
            continue;
        }
        let owned: Vec<Row> = rs.iter().map(|r| **r).collect();
        let exact = |k: &dyn Fn(&Row) -> (i64, i64)| mispriced(&owned, k).0 == 0;
        let w = hist(rs, |r| r.waitcnt).len() > 1;
        let reason = if w && exact(&|r| (r.waitcnt as i64, 0)) {
            "WAITCNT alone"
        } else if exact(&|r| (r.data as i64, r.waitcnt as i64)) {
            "the address it touched"
        } else if exact(&|r| (r.ahead as i64, r.waitcnt as i64)) {
            "the buffer's fill"
        } else if exact(&|r| (r.prev as i64, r.data as i64 * 65536 + r.waitcnt as i64)) {
            "the path in, and the address"
        } else {
            "the buffer's fill and the address"
        };
        let e = why.entry(reason).or_default();
        e.0 += 1;
        e.1 += runs;
        varying.push((runs, *pc, reason, costs));
    }
    println!("\nsites by how many different costs they had:");
    for (n, (s, runs)) in &ncosts {
        println!(
            "  {n} cost{}: {s} sites, {:.1}% of the runs",
            if *n == 1 { "" } else { "s" },
            100.0 * *runs as f64 / total as f64
        );
    }
    println!("\nwhat a varying site's cost hangs on (the first that settles it):");
    for (r, (s, runs)) in &why {
        println!(
            "  {r}: {s} sites, {:.1}% of all runs",
            100.0 * *runs as f64 / total as f64
        );
    }
    varying.sort_by_key(|a| std::cmp::Reverse(a.0));
    println!("\nthe busiest varying sites (address = pc - 4, Thumb):");
    for (runs, pc, reason, costs) in varying.iter().take(10) {
        let rs = &by_site[pc];
        println!(
            "  {:08x}: {runs} runs, {reason}; cost {}; data cycles {}; buffer lead {}; came from {} sites",
            pc.wrapping_sub(4),
            show(costs),
            show(&hist(rs, |r| r.data)),
            show(&hist(rs, |r| r.ahead)),
            hist(rs, |r| r.prev).len()
        );
    }
    println!("\nhow each static table prices the log (most common cost per key):");
    type Key = fn(&Row) -> (u32, i64, i64, i64);
    let tables: [(&str, Key); 7] = [
        ("site", |r| (r.pc, 0, 0, 0)),
        ("site x WAITCNT", |r| (r.pc, r.waitcnt as i64, 0, 0)),
        ("site x buffer x WAITCNT", |r| {
            (r.pc, r.waitcnt as i64, r.ahead as i64, 0)
        }),
        ("site x path in x WAITCNT", |r| {
            (r.pc, r.waitcnt as i64, r.prev as i64, 0)
        }),
        ("site x address x WAITCNT", |r| {
            (r.pc, r.waitcnt as i64, r.data as i64, r.cart as i64)
        }),
        ("site x path in x address x WAITCNT", |r| {
            (r.pc, r.waitcnt as i64, r.data as i64, r.prev as i64)
        }),
        ("site x buffer x address x WAITCNT", |r| {
            (
                r.pc,
                r.waitcnt as i64,
                r.data as i64 * 32 + r.ahead as i64,
                r.cart as i64,
            )
        }),
    ];
    for (name, key) in tables {
        let (runs, cyc) = mispriced(rows, key);
        println!(
            "  {name}: {runs} runs mispriced ({:.2}%), {cyc} cycles off{}",
            100.0 * runs as f64 / total as f64,
            frames.map_or(String::new(), |f| format!(
                " ({:.0} a frame)",
                cyc as f64 / f as f64
            ))
        );
    }
}

/// A run's `timing.lua` lines: frame -> (fc, rng, wait, x, y).
fn read_out(path: &Path) -> Result<BTreeMap<u32, [u32; 5]>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("log: ") else {
            continue;
        };
        let mut it = rest.split(' ');
        let Some(f) = it.next().and_then(|f| f.parse().ok()) else {
            continue;
        };
        let mut v = [0; 5];
        for kv in it {
            let Some((k, val)) = kv.split_once('=') else {
                continue;
            };
            let i = match k {
                "fc" => 0,
                "rng" => 1,
                "wait" => 2,
                "x" => 3,
                "y" => 4,
                _ => continue,
            };
            v[i] = if k == "rng" {
                u32::from_str_radix(val, 16).unwrap_or(0)
            } else {
                val.parse().unwrap_or(0)
            };
        }
        m.insert(f, v);
    }
    Ok(m)
}

fn read_hashes(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|l| l.split_whitespace().nth(3).map(str::to_string))
        .collect())
}

/// A 16-bit PCM WAV's samples (both channels, interleaved).
fn read_wav(path: &Path) -> Result<(u32, Vec<i16>), String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut i = 12;
    let mut rate = 0;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        if id == b"fmt " {
            rate = u32::from_le_bytes(b[i + 12..i + 16].try_into().unwrap());
        } else if id == b"data" {
            let d = &b[i + 8..(i + 8 + n).min(b.len())];
            return Ok((
                rate,
                d.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| i16::from_le_bytes(c))
                    .collect(),
            ));
        }
        i += 8 + n + (n & 1);
    }
    Err(format!("{}: no data chunk", path.display()))
}

fn read_png(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut r = d.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let ch = info.color_type.samples();
    let rgb = buf[..info.buffer_size()]
        .chunks_exact(ch)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    Some((info.width, info.height, rgb))
}

/// Compare two dumps of one frame: the pixels and rows that differ, and a
/// picture of it: the real frame, the variant's, and the real one dimmed
/// with the differing pixels in magenta, side by side.
fn diff_png(a: &Path, b: &Path, out: &Path) -> Option<(usize, Vec<u32>)> {
    let (w, h, pa) = read_png(a)?;
    let (_, _, pb) = read_png(b)?;
    let mut img = Vec::with_capacity(3 * pa.len());
    let (mut n, mut rows) = (0, Vec::new());
    for y in 0..h {
        let line = 3 * (y * w) as usize..3 * ((y + 1) * w) as usize;
        img.extend(&pa[line.clone()]);
        img.extend(&pb[line]);
        let mut row = false;
        for x in 0..w {
            let i = 3 * (y * w + x) as usize;
            if pa[i..i + 3] != pb[i..i + 3] {
                n += 1;
                row = true;
                img.extend([255, 0, 255]);
            } else {
                img.extend(pa[i..i + 3].iter().map(|v| v / 3));
            }
        }
        if row {
            rows.push(y);
        }
    }
    if n > 0 {
        let f = std::fs::File::create(out).ok()?;
        let mut e = png::Encoder::new(std::io::BufWriter::new(f), 3 * w, h);
        e.set_color(png::ColorType::Rgb);
        e.write_header().ok()?.write_image_data(&img).ok()?;
    }
    Some((n, rows))
}

fn spans(rows: &[u32]) -> String {
    let mut s: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let mut j = i;
        while j + 1 < rows.len() && rows[j + 1] == rows[j] + 1 {
            j += 1;
        }
        s.push(if i == j {
            rows[i].to_string()
        } else {
            format!("{}-{}", rows[i], rows[j])
        });
        i = j + 1;
    }
    s.join(",")
}

fn compare(dir: &Path, name: &str) -> Result<(), String> {
    let base = read_out(&dir.join("base.out"))?;
    let run = read_out(&dir.join(format!("{name}.out")))?;
    let bh = read_hashes(&dir.join("base.hashes"))?;
    let rh = read_hashes(&dir.join(format!("{name}.hashes")))?;
    println!("\n## {name}\n");
    // The first frame any of fields `is` differs.
    let first_of = |is: &[usize]| {
        base.iter()
            .find(|(f, v)| run.get(f).is_some_and(|w| is.iter().any(|&i| w[i] != v[i])))
            .map(|(f, _)| f.to_string())
            .unwrap_or("never".into())
    };
    let first = |i: usize| first_of(&[i]);
    let lag = |m: &BTreeMap<u32, [u32; 5]>| m.values().filter(|v| v[2] == 0).count();
    let differ = bh.iter().zip(&rh).filter(|(a, b)| a != b).count();
    let first_frame = bh
        .iter()
        .zip(&rh)
        .position(|(a, b)| a != b)
        .map(|i| (i + 1).to_string())
        .unwrap_or("never".into());
    println!(
        "first frame that looks different: {first_frame} ({differ} of {} frames differ)",
        bh.len()
    );
    println!("first frame the RNG differs: {}", first(1));
    println!(
        "first frame Kirby's position differs: {}",
        first_of(&[3, 4])
    );
    println!(
        "lag frames: {} (the real run: {}); first frame a lag differs: {}",
        lag(&run),
        lag(&base),
        first(2)
    );
    let lagged: Vec<String> = run
        .iter()
        .filter(|(f, v)| v[2] == 0 && base.get(f).is_some_and(|b| b[2] != 0))
        .map(|(f, _)| f.to_string())
        .take(12)
        .collect();
    if !lagged.is_empty() {
        println!("  lags the real run hasn't: {}", lagged.join(" "));
    }
    if let (Some((f, b)), Some(r)) = (base.iter().next_back(), run.values().next_back()) {
        println!(
            "at frame {f}: rng {:08x} (real {:08x}), Kirby at x={} y={} (real {} {})",
            r[1],
            b[1],
            r[3] / 256,
            r[4] / 256,
            b[3] / 256,
            b[4] / 256
        );
    }
    if let Ok(t) = std::fs::read_to_string(dir.join(format!("{name}.stats"))) {
        let v: Vec<u64> = t
            .lines()
            .filter_map(|l| l.split(' ').nth(1)?.parse().ok())
            .collect();
        if let [runs, unseen] = v[..]
            && runs > 0
        {
            println!(
                "stalls its table had no entry for (so their real cost): {unseen} of {runs} ({:.2}%)",
                100.0 * unseen as f64 / runs as f64
            );
        }
    }
    let (wa, wb) = (dir.join("base.wav"), dir.join(format!("{name}.wav")));
    if let (Ok((rate, a)), Ok((_, b))) = (read_wav(&wa), read_wav(&wb)) {
        let n = a.len().min(b.len());
        match (0..n).find(|&i| a[i] != b[i]) {
            None => println!("sound: identical ({n} samples)"),
            Some(i) => {
                let differ = (0..n).filter(|&i| a[i] != b[i]).count();
                // A frame's samples, both channels: is it the real sound late?
                let k = 2 * (rate as f64 / 59.7275).round() as usize;
                let late = (k..n).filter(|&i| a[i - k] == b[i] && a[i] != b[i]).count();
                println!(
                    "sound: first differs at {:.3} s (frame {:.0}); {differ} of {n} samples differ ({:.2}%), {:.1}% of those the real sound one frame late",
                    i as f64 / 2.0 / rate as f64,
                    i as f64 / 2.0 / rate as f64 * 59.7275,
                    100.0 * differ as f64 / n as f64,
                    100.0 * late as f64 / differ as f64
                );
            }
        }
    }
    let mut dumps: Vec<u32> = std::fs::read_dir(dir.join("base"))
        .map(|d| {
            d.filter_map(|e| {
                let n = e.ok()?.file_name().into_string().ok()?;
                n.strip_prefix("frame-")?.strip_suffix(".png")?.parse().ok()
            })
            .collect()
        })
        .unwrap_or_default();
    dumps.sort();
    for f in dumps {
        let file = format!("frame-{f}.png");
        let out = dir.join(name).join(format!("diff-{f}.png"));
        match diff_png(
            &dir.join("base").join(&file),
            &dir.join(name).join(&file),
            &out,
        ) {
            Some((0, _)) => println!("frame {f}: same picture"),
            Some((n, rows)) => println!(
                "frame {f}: {n} pixels differ, on lines {} ({})",
                spans(&rows),
                out.display()
            ),
            None => println!("frame {f}: no dump"),
        }
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first().map(Path::new) else {
        eprintln!("usage: gbaa-timing-report DIR VARIANT…");
        return std::process::ExitCode::from(2);
    };
    let frames = read_hashes(&dir.join("base.hashes")).ok().map(|h| h.len());
    let r = read_log(&dir.join("base.tsv"))
        .map(|rows| sites(&rows, frames))
        .and_then(|()| args[1..].iter().try_for_each(|v| compare(dir, v)));
    match r {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gbaa-timing-report: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
