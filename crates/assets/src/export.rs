//! The graphics export as a library call, for `gbaa-assets graphics` and
//! the desktop's extract-while-playing (`gbaa-desktop --extract DIR`,
//! docs/asset-extraction-design.md, Extract while playing): every route
//! recorded under a folder, its tile sheets, palettes, tilemaps and
//! sprites written there, and the manifest's sections for them merged in.

use crate::graphics::{self, Found};
use std::collections::BTreeMap;
use std::path::Path;

/// What [`graphics`] wrote.
pub struct Export {
    pub found: Found,
    pub routes: Vec<String>,
    /// What it wrote, a line or two.
    pub summary: String,
}

/// Exports the graphics every route folder under `dir` (one with a
/// `loads.tsv`) recorded for `rom`. `named`: names by ROM address (a
/// source map's, or [`manifest_names`]); `sizes`: the source map's data
/// symbols and sizes, bounding sprite sheets.
pub fn graphics(
    rom: &[u8],
    dir: &Path,
    named: &BTreeMap<u32, String>,
    sizes: &BTreeMap<u32, u64>,
) -> Result<Export, String> {
    let sha = sha1_hex(rom);
    let mpath = dir.join("manifest.toml");
    let old = std::fs::read_to_string(&mpath).unwrap_or_default();
    if let Some(theirs) = old
        .lines()
        .find_map(|l| l.strip_prefix("rom_sha1 = \""))
        .and_then(|l| l.strip_suffix('"'))
        && theirs != sha
    {
        return Err(format!(
            "{}: exported from another ROM (sha1 {theirs}, this one's {sha})",
            mpath.display()
        ));
    }
    let mut routes = Vec::new();
    let mut logs = Vec::new();
    let mut oams = Vec::new();
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut subdirs: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        // A session cut off before its logs were written has no uses.tsv.
        .filter(|p| p.join("loads.tsv").exists() && p.join("uses.tsv").exists())
        .collect();
    subdirs.sort();
    for d in subdirs {
        let read = |n: &str| {
            let p = d.join(n);
            std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
        };
        logs.push((
            graphics::parse_loads(&read("loads.tsv")?)?,
            graphics::parse_uses(&read("uses.tsv")?)?,
        ));
        // A route recorded before the OAM log shows no sprites.
        if d.join("oam.tsv").exists() {
            let last = crate::sprites::last_frame(&read("shown.tsv")?)
                .ok_or_else(|| format!("{}: shown.tsv has no frame", d.display()))?;
            oams.push((crate::sprites::parse_oam(&read("oam.tsv")?)?, last));
        }
        routes.push(
            d.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        );
    }
    if logs.is_empty() {
        return Err(format!(
            "{}: no route folder with a loads.tsv (`just assets-record`)",
            dir.display()
        ));
    }
    let mut found = graphics::find(rom, &logs);
    let sprites = crate::sprites::find(rom, &mut found, &oams, sizes);
    let w = graphics::write(&found, named, dir)?;
    let sw = crate::sprites::write(&found, &sprites, named, dir)?;
    let rpath = dir.join("sprites.txt");
    std::fs::write(&rpath, &sw.report).map_err(|e| format!("{}: {e}", rpath.display()))?;
    let header = format!(
        "# Written by `gbaa-assets` (crates/assets). Don't edit.\n\n[manifest]\nrom_sha1 = \"{sha}\"\nformat = 1\n"
    );
    let mut text = graphics::merge_manifest(
        &old,
        &["tiles", "palettes", "tilemaps", "sprites", "graphics"],
        &format!("{}\n{}", w.manifest, sw.manifest),
        &header,
    );
    let routes_q: Vec<String> = routes.iter().map(|r| format!("\"{r}\"")).collect();
    text.push_str(&format!(
        "\n[graphics]\nroutes = [{}]\n",
        routes_q.join(", ")
    ));
    std::fs::write(&mpath, text).map_err(|e| format!("{}: {e}", mpath.display()))?;
    let summary = format!(
        "graphics from {} route(s): {} tile sheets, {} palettes, {} tilemaps written to {}\nsprites: {} sheets, {} animations: sprites/*.aseprite ({})",
        routes.len(),
        w.counts[0],
        w.counts[1],
        w.counts[2],
        dir.display(),
        sw.sheets,
        sw.anims,
        rpath.display()
    );
    Ok(Export {
        found,
        routes,
        summary,
    })
}

/// The names and sizes an earlier export under `dir` gave its blobs
/// (`manifest.toml`'s `[tiles]`, `[palettes]`, `[tilemaps]` and
/// `[sprites]`), so a later export without the source map keeps its file
/// names and sprite sheets; empty with no manifest.
pub fn manifest_names(dir: &Path) -> (BTreeMap<u32, String>, BTreeMap<u32, u64>) {
    let mut named = BTreeMap::new();
    let mut sizes = BTreeMap::new();
    let Some(t) = std::fs::read_to_string(dir.join("manifest.toml"))
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
    else {
        return (named, sizes);
    };
    for section in ["tiles", "palettes", "tilemaps", "sprites"] {
        let Some(entries) = t.get(section).and_then(|v| v.as_table()) else {
            continue;
        };
        for (k, v) in entries {
            let Some(addr) = k
                .strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
            else {
                continue;
            };
            if let Some(n) = v.get("name").and_then(|n| n.as_str()) {
                named.entry(addr).or_insert_with(|| n.to_string());
            }
            if section == "tiles"
                && let Some(n) = v.get("size").and_then(|n| n.as_integer())
            {
                sizes.insert(addr, n as u64);
            }
        }
    }
    (named, sizes)
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

/// The SHA-1 of `data`, in hex, as manifests name their ROM.
pub fn sha1_hex(data: &[u8]) -> String {
    runtime::sha1::sha1(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What [`check`] found.
pub struct Check {
    pub text: String,
    /// Tile and palette blobs the session showed that the manifest misses.
    pub missed: usize,
    /// Entries of the earlier manifest gone or renamed.
    pub lost: usize,
    /// Entries it lacked.
    pub new: usize,
}

/// `--extract`'s check (`gbaa-assets check-extract`): every tile blob and
/// palette blob the session at `session` showed (its `uses.tsv`) that it
/// loaded (a source in its `loads.tsv`: the export's blobs are loads) is
/// in `dir`'s manifest, an entry with files on disk holding its address;
/// and every entry of `before` (the manifest before the session, if any)
/// is still there, under the same name. Bytes the game's own code copied
/// from ROM (a font's glyphs) aren't loads and aren't exported (the
/// design's Recording): they are listed, not failed.
pub fn check(dir: &Path, session: &Path, before: Option<&str>) -> Result<Check, String> {
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let uses = graphics::parse_uses(&read(&session.join("uses.tsv"))?)?;
    // Loaded: a compressed blob's first byte, inside a copy's ROM
    // source, or inside a RAM source's ROM run that the CPU didn't copy
    // there (a run is `rom/offset/bytes/steps`).
    let mut loaded_at: Vec<(u32, u32)> = Vec::new();
    let log = read(&session.join("loads.tsv"))?;
    for l in log.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let f: Vec<&str> = l.split('\t').collect();
        let (Some(op), Some(src), Some(len), Some(runs)) = (f.get(1), f.get(2), f.get(3), f.get(6))
        else {
            continue;
        };
        let (Some(src), Ok(len)) = (hex(src), len.parse::<u32>()) else {
            continue;
        };
        match *op {
            _ if src >> 24 != 0x08 => {}
            "dma" | "cpuset" | "fastset" => loaded_at.push((src, len)),
            _ => loaded_at.push((src, 1)),
        }
        for r in runs.split(',') {
            let p: Vec<&str> = r.split('/').collect();
            if let [rom, _, n, steps] = p[..]
                && steps != "cpu"
                && let (Some(rom), Ok(n)) = (hex(rom), n.parse::<u32>())
            {
                loaded_at.push((rom, n));
            }
        }
    }
    let loaded = |a: u32| loaded_at.iter().any(|&(s, n)| a >= s && a - s < n);
    let now = entries(&read(&dir.join("manifest.toml"))?)?;
    let mut shown: BTreeMap<u32, (&str, u64)> = BTreeMap::new();
    for u in &uses {
        shown.entry(u.blob).or_insert(("tiles", u.first));
        if let Some((p, _)) = u.pal {
            shown.entry(p).or_insert(("palettes", u.first));
        }
    }
    let mut text = String::new();
    let (mut missed, mut cpu) = (0, 0);
    for (&a, &(kind, first)) in &shown {
        // The entry holding it: a joined sprite sheet starts before a
        // frame; tile data decompressed beside a map may be filed as one.
        let kinds: &[&str] = if kind == "tiles" {
            &["tiles", "tilemaps"]
        } else {
            &[kind]
        };
        let holder = kinds.iter().find_map(|&k| {
            now.range(..=(k, a))
                .next_back()
                .filter(|((hk, s), e)| *hk == k && (a as u64) < *s as u64 + e.size.max(1) as u64)
        });
        if holder.is_none() && !loaded(a) {
            cpu += 1;
            text.push_str(&format!(
                "not loaded\t{kind}\t{a:#010x}\tshown from frame {first}, in no load the export takes (a CPU copy)\n"
            ));
            continue;
        }
        match holder {
            Some(((_, s), e)) if e.files.iter().all(|f| dir.join(f).exists()) => {
                let at = if *s == a {
                    String::new()
                } else {
                    format!(" (in {s:#010x})")
                };
                text.push_str(&format!("ok\t{kind}\t{a:#010x}\t{}{at}\n", e.name));
            }
            Some(((_, s), _)) => {
                missed += 1;
                text.push_str(&format!(
                    "no file\t{kind}\t{a:#010x}\t{s:#010x}'s files aren't all there\n"
                ));
            }
            None => {
                missed += 1;
                text.push_str(&format!(
                    "missed\t{kind}\t{a:#010x}\tshown from frame {first}\n"
                ));
            }
        }
    }
    let mut lost = 0;
    let mut kept = 0;
    if let Some(b) = before {
        for (key, e) in entries(b)? {
            match now.get(&key) {
                Some(n) if n.name == e.name => kept += 1,
                Some(n) => {
                    lost += 1;
                    text.push_str(&format!(
                        "renamed\t{}\t{:#010x}\t{} to {}\n",
                        key.0, key.1, e.name, n.name
                    ));
                }
                None => {
                    lost += 1;
                    text.push_str(&format!("lost\t{}\t{:#010x}\t{}\n", key.0, key.1, e.name));
                }
            }
        }
    }
    let new = before.map_or(Ok(now.len()), |b| {
        entries(b).map(|old| now.keys().filter(|k| !old.contains_key(k)).count())
    })?;
    let head = format!(
        "{} blobs shown ({} missed, {cpu} in no load, not exported); {} entries now, {new} new; {kept} earlier entries kept, {lost} lost or renamed\n",
        shown.len(),
        missed,
        now.len()
    );
    Ok(Check {
        text: head + &text,
        missed,
        lost,
        new,
    })
}

/// A manifest entry, for [`check`].
struct Entry {
    name: String,
    size: u32,
    files: Vec<String>,
}

/// A manifest's `[tiles]`, `[palettes]`, `[tilemaps]` and `[sprites]` by
/// (section, address); sprites count as tiles, which they are.
fn entries(text: &str) -> Result<BTreeMap<(&'static str, u32), Entry>, String> {
    let t: toml::Table = text.parse().map_err(|e| format!("manifest: {e}"))?;
    let mut out = BTreeMap::new();
    for (section, as_kind) in [
        ("tiles", "tiles"),
        ("palettes", "palettes"),
        ("tilemaps", "tilemaps"),
        ("sprites", "sprites"),
    ] {
        let Some(es) = t.get(section).and_then(|v| v.as_table()) else {
            continue;
        };
        for (k, v) in es {
            let Some(addr) = k
                .strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
            else {
                continue;
            };
            let get_str = |n: &str| v.get(n).and_then(|x| x.as_str()).unwrap_or("").to_string();
            out.insert(
                (as_kind, addr),
                Entry {
                    name: get_str("name"),
                    size: v.get("size").and_then(|x| x.as_integer()).unwrap_or(0) as u32,
                    files: v
                        .get("files")
                        .and_then(|x| x.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|f| f.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                },
            );
        }
    }
    Ok(out)
}
