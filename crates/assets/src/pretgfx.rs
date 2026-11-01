//! `check-pret graphics` (docs/asset-extraction-design.md, Checking against
//! pret): each exported blob joined to pret's file by the source map's
//! symbol (pret's ELF) and the `INCGFX` that defines it, then checked at
//! four levels:
//!
//! - **bytes**: our unpacked bytes equal pret's file through gbagfx with its
//!   flags (the right blob, whole);
//! - **rebuilds**: our file through gbagfx with pret's flags, then
//!   compressed as pret does, gives the ROM's bytes;
//! - **palette** (tile sheets): our paired palette equals pret's PNG's
//!   (`x >> 3`) over the colours the sheet's depth uses;
//! - **image** (tile sheets): our PNG's size and pixel indices equal pret's.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::graphics::{Kind, ROM_BASE, lz77, read_indexed_png, rl};

/// One `INCGFX_U*("source", ".ext", "args")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incgfx {
    pub symbol: String,
    /// The C file it's in, relative to pret's root.
    pub file: String,
    pub source: String,
    pub ext: String,
    pub args: String,
}

fn quoted(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start().strip_prefix('"')?;
    let end = s.find('"')?;
    Some((&s[..end], &s[end + 1..]))
}

/// Every `INCGFX` in one C source's text.
pub fn parse_incgfx(text: &str, file: &str) -> Vec<Incgfx> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("INCGFX_U") {
        let before = &rest[..i];
        let after = &rest[i..];
        rest = &rest[i + 8..];
        // `name[] = INCGFX_U32(`: the identifier before the last `[` (or
        // `=`) on the statement.
        let stmt = before.rsplit([';', '}', '{']).next().unwrap_or("");
        let Some(eq) = stmt.rfind('=') else { continue };
        let lhs = stmt[..eq].trim_end();
        let lhs = lhs.find('[').map_or(lhs, |b| &lhs[..b]).trim_end();
        let symbol: String = lhs
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let Some(open) = after.find('(') else {
            continue;
        };
        let args_text = &after[open + 1..];
        let Some((source, r)) = quoted(args_text) else {
            continue;
        };
        let Some((ext, r)) = r.trim_start().strip_prefix(',').and_then(quoted) else {
            continue;
        };
        let args = r
            .trim_start()
            .strip_prefix(',')
            .and_then(quoted)
            .map_or("", |a| a.0);
        if symbol.is_empty() {
            continue;
        }
        out.push(Incgfx {
            symbol,
            file: file.into(),
            source: source.into(),
            ext: ext.into(),
            args: args.into(),
        });
    }
    out
}

/// Every `INCGFX` under pret's `src/`, by symbol (statics of two files can
/// share a name).
pub fn scan_pret(pret: &Path) -> Result<BTreeMap<String, Vec<Incgfx>>, String> {
    let mut out: BTreeMap<String, Vec<Incgfx>> = BTreeMap::new();
    let mut stack = vec![pret.join("src")];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "c" || x == "h") {
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let rel = p.strip_prefix(pret).unwrap_or(&p).display().to_string();
                for g in parse_incgfx(&text, &rel) {
                    out.entry(g.symbol.clone()).or_default().push(g);
                }
            }
        }
    }
    Ok(out)
}

/// One manifest entry the check reads.
#[derive(Debug, Clone)]
pub struct Entry {
    pub addr: u32,
    pub kind: Kind,
    pub store: String,
    pub size: u32,
    pub bpp: u8,
    /// A tile sheet drawn with a palette on the route (else greyscale).
    pub paired: bool,
    pub file: String,
}

/// The manifest's `[tiles]`, `[palettes]` and `[tilemaps]`.
pub fn manifest_entries(text: &str) -> Result<Vec<Entry>, String> {
    let doc: toml::Table = text.parse().map_err(|e| format!("manifest.toml: {e}"))?;
    let mut out = Vec::new();
    for (sec, kind) in [
        ("tiles", Kind::Tiles),
        ("palettes", Kind::Palette),
        ("tilemaps", Kind::Tilemap),
    ] {
        let Some(t) = doc.get(sec).and_then(|v| v.as_table()) else {
            continue;
        };
        for (k, v) in t {
            let addr = u32::from_str_radix(k.trim_start_matches("0x"), 16)
                .map_err(|_| format!("[{sec}] {k}: not an address"))?;
            let get = |f: &str| v.get(f).and_then(|x| x.as_integer());
            out.push(Entry {
                addr,
                kind,
                store: v
                    .get("store")
                    .and_then(|x| x.as_str())
                    .unwrap_or("raw")
                    .into(),
                size: get("size").unwrap_or(0) as u32,
                bpp: get("bpp").unwrap_or(4) as u8,
                paired: v.get("palette").is_some_and(|p| p.as_str() != Some("grey")),
                file: v
                    .get("files")
                    .and_then(|x| x.as_array())
                    .and_then(|a| a.first())
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .into(),
            });
        }
    }
    Ok(out)
}

/// The levels one blob reached, and why it missed the rest.
#[derive(Debug, Default)]
pub struct Levels {
    pub bytes: bool,
    pub rebuilds: bool,
    pub palette: Option<bool>,
    pub image: Option<bool>,
    pub why: Vec<String>,
}

pub struct Report {
    pub text: String,
    pub joined: usize,
    pub bytes: usize,
    pub rebuilds: usize,
    pub palette: (usize, usize),
    pub image: (usize, usize),
}

struct Gbagfx {
    tool: PathBuf,
    tmp: PathBuf,
}

impl Gbagfx {
    fn run(&self, input: &Path, out: &str, args: &str) -> Result<Vec<u8>, String> {
        let o = self.tmp.join(out);
        let r = Command::new(&self.tool)
            .arg(input)
            .arg(&o)
            .args(args.split_whitespace())
            .output()
            .map_err(|e| format!("gbagfx: {e}"))?;
        if !r.status.success() {
            return Err(format!(
                "gbagfx {}: {}",
                input.display(),
                String::from_utf8_lossy(&r.stderr).trim()
            ));
        }
        std::fs::read(&o).map_err(|e| format!("{}: {e}", o.display()))
    }
}

/// pret's file's unpacked bytes in the ROM's layout.
fn pret_unpacked(g: &Gbagfx, pret: &Path, inc: &Incgfx) -> Result<Vec<u8>, String> {
    let src = pret.join(&inc.source);
    if !src.exists() {
        return Err(format!("{} isn't in pret's tree", inc.source));
    }
    let unpacked_ext = inc
        .ext
        .strip_suffix(".lz")
        .or(inc.ext.strip_suffix(".rl"))
        .unwrap_or(&inc.ext);
    if unpacked_ext.is_empty() {
        return std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()));
    }
    g.run(&src, &format!("pret{unpacked_ext}"), &inc.args)
}

/// Our file back to the ROM's bytes as pret builds them.
fn rebuild(g: &Gbagfx, ours: &Path, inc: &Incgfx) -> Result<Vec<u8>, String> {
    let (unpacked_ext, pack) = match (inc.ext.strip_suffix(".lz"), inc.ext.strip_suffix(".rl")) {
        (Some(u), _) => (u, Some(".lz")),
        (_, Some(u)) => (u, Some(".rl")),
        _ => (inc.ext.as_str(), None),
    };
    let (unpacked, upath) = if unpacked_ext.is_empty() {
        (
            std::fs::read(ours).map_err(|e| format!("{}: {e}", ours.display()))?,
            ours.to_path_buf(),
        )
    } else {
        let name = format!("ours{unpacked_ext}");
        (g.run(ours, &name, &inc.args)?, g.tmp.join(name))
    };
    match pack {
        None => Ok(unpacked),
        Some(p) => g.run(&upath, &format!("ours{unpacked_ext}{p}"), ""),
    }
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(i) => format!("differ first at byte {i}"),
        None => format!("{} bytes against {}", a.len(), b.len()),
    }
}

fn check_one(
    g: &Gbagfx,
    rom: &[u8],
    out: &Path,
    pret: &Path,
    e: &Entry,
    inc: &Incgfx,
    ours_unpacked: &[u8],
) -> Levels {
    let mut lv = Levels::default();
    let expect = match (e.kind, inc.ext.as_str()) {
        (Kind::Tiles, x) => x.contains("bpp"),
        (Kind::Palette, x) => x.starts_with(".gbapal"),
        (Kind::Tilemap, x) => !x.contains("bpp") && !x.contains("gbapal"),
    };
    if !expect {
        lv.why
            .push(format!("kind: ours {:?}, pret's {}", e.kind, inc.ext));
        return lv;
    }
    match pret_unpacked(g, pret, inc) {
        Ok(p) if p == ours_unpacked => lv.bytes = true,
        Ok(p) => lv
            .why
            .push(format!("bytes: {}", first_diff(ours_unpacked, &p))),
        Err(err) => lv.why.push(format!("bytes: {err}")),
    }
    let ours = out.join(&e.file);
    let start = (e.addr - ROM_BASE) as usize;
    match rebuild(g, &ours, inc) {
        Ok(r) => {
            let at = rom.get(start..start + r.len());
            // A packed blob's size is the padded length the game read; the
            // rebuilt stream must fit it. A raw blob's size is only the span
            // the game copied, which needn't cover the whole symbol.
            let pad = r.len().div_ceil(4) * 4;
            let fits = e.store == "raw" || pad as u32 <= e.size;
            if at == Some(&r[..]) && fits {
                lv.rebuilds = true;
            } else {
                lv.why.push(format!(
                    "rebuilds: {}",
                    at.map_or("runs off the ROM".into(), |a| first_diff(&r, a))
                ));
            }
        }
        Err(err) => lv.why.push(format!("rebuilds: {err}")),
    }
    if e.kind == Kind::Tiles {
        let theirs = std::fs::read(pret.join(&inc.source))
            .map_err(|x| x.to_string())
            .and_then(|b| read_indexed_png(&b));
        let mine = std::fs::read(&ours)
            .map_err(|x| x.to_string())
            .and_then(|b| read_indexed_png(&b));
        match (mine, theirs) {
            (Ok(m), Ok(t)) => {
                // A 4bpp sheet's colour 0 is transparent: pret's PNGs hold
                // any colour there.
                let (from, n) = if e.bpp == 4 { (1, 16) } else { (0, 256) };
                let (mp, tp) = (
                    m.3.get(from..m.3.len().min(n)).unwrap_or(&[]),
                    t.3.get(from..t.3.len().min(n)).unwrap_or(&[]),
                );
                let pal_ok = mp == tp;
                lv.palette = e.paired.then_some(pal_ok);
                if !e.paired {
                    lv.why
                        .push("palette: never drawn on the route, written greyscale".into());
                } else if !pal_ok {
                    let i = from
                        + mp.iter()
                            .zip(tp)
                            .position(|(a, b)| a != b)
                            .unwrap_or(mp.len().min(tp.len()));
                    lv.why.push(format!(
                        "palette: colour {i} differs ({} against pret's {} colours)",
                        m.3.len(),
                        t.3.len()
                    ));
                }
                let img_ok = (m.0, m.1) == (t.0, t.1) && m.2 == t.2;
                lv.image = Some(img_ok);
                if !img_ok {
                    lv.why.push(if (m.0, m.1) != (t.0, t.1) {
                        format!("image: {}x{} against pret's {}x{}", m.0, m.1, t.0, t.1)
                    } else {
                        "image: pixels differ".into()
                    });
                }
            }
            (Err(x), _) => lv.why.push(format!("image: ours: {x}")),
            (_, Err(x)) => lv.why.push(format!("image: pret's: {x}")),
        }
    }
    lv
}

/// Checks every graphics blob in `out`'s manifest against pret.
pub fn check(
    rom: &[u8],
    out: &Path,
    pret: &Path,
    names: &BTreeMap<u32, (String, Option<String>)>,
) -> Result<Report, String> {
    let text = std::fs::read_to_string(out.join("manifest.toml"))
        .map_err(|e| format!("{}/manifest.toml: {e}", out.display()))?;
    let entries = manifest_entries(&text)?;
    let incs = scan_pret(pret)?;
    let tmp = std::env::temp_dir().join(format!("gbaa-gfx-check-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let g = Gbagfx {
        tool: pret.join("tools/gbagfx/gbagfx"),
        tmp: tmp.clone(),
    };
    let mut lines = String::new();
    let mut unjoined = String::new();
    let (mut joined, mut bytes, mut rebuilds) = (0, 0, 0);
    let (mut pal, mut img) = ((0, 0), (0, 0));
    for e in &entries {
        let Some((sym, file)) = names.get(&e.addr) else {
            let _ = writeln!(
                unjoined,
                "  {:#010x} {:?}: no pret symbol starts here",
                e.addr, e.kind
            );
            continue;
        };
        let cands = incs.get(sym).cloned().unwrap_or_default();
        // Statics of two files: the one in the symbol's object file.
        let inc = cands
            .iter()
            .find(|c| {
                file.as_deref()
                    .is_some_and(|f| Path::new(&c.file).file_stem() == Path::new(f).file_stem())
            })
            .or(cands.first());
        let Some(inc) = inc else {
            let _ = writeln!(
                unjoined,
                "  {:#010x} {sym} {:?}: no INCGFX defines it",
                e.addr, e.kind
            );
            continue;
        };
        joined += 1;
        let at = rom.get((e.addr - ROM_BASE) as usize..).unwrap_or(&[]);
        let data = match e.store.as_str() {
            "lz77" => lz77(at).map(|d| d.0),
            "rl" => rl(at).map(|d| d.0),
            _ => at.get(..e.size as usize).map(|d| d.to_vec()),
        }
        .unwrap_or_default();
        let lv = check_one(&g, rom, out, pret, e, inc, &data);
        bytes += lv.bytes as usize;
        rebuilds += lv.rebuilds as usize;
        if let Some(p) = lv.palette {
            pal.1 += 1;
            pal.0 += p as usize;
        }
        if let Some(i) = lv.image {
            img.1 += 1;
            img.0 += i as usize;
        }
        let mark = |b: bool| if b { "ok" } else { "--" };
        let _ = writeln!(
            lines,
            "  {:#010x} {sym} ({}{} {}) bytes {} rebuilds {} palette {} image {}{}",
            e.addr,
            inc.source,
            inc.ext,
            if inc.args.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", inc.args)
            },
            mark(lv.bytes),
            mark(lv.rebuilds),
            lv.palette.map_or("n/a", |p| mark(p)),
            lv.image.map_or("n/a", |p| mark(p)),
            if lv.why.is_empty() {
                String::new()
            } else {
                format!(": {}", lv.why.join("; "))
            },
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let total_pret: usize = incs.values().map(|v| v.len()).sum();
    let by_kind = |k: Kind| entries.iter().filter(|e| e.kind == k).count();
    let text = format!(
        "# gbaa-assets check-pret graphics: each exported blob against pret's INCGFX file\n\
         exported: {} tile sheets, {} palettes, {} tilemaps; joined to a pret INCGFX file {joined} of {}\n\
         bytes {bytes}, rebuilds {rebuilds}, palette {} of {}, image {} of {} (of the joined)\n\
         pret's INCGFX files: {total_pret}; shown by the routes and joined: {joined}\n\n\
         per blob:\n{lines}\nnot joined:\n{unjoined}",
        by_kind(Kind::Tiles),
        by_kind(Kind::Palette),
        by_kind(Kind::Tilemap),
        entries.len(),
        pal.0,
        pal.1,
        img.0,
        img.1,
    );
    Ok(Report {
        text,
        joined,
        bytes,
        rebuilds,
        palette: pal,
        image: img,
    })
}
