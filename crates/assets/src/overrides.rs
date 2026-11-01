//! Asset overrides (docs/asset-extraction-design.md, "Overrides"): a mod
//! folder's edited files encoded back to the bytes their blob unpacks to,
//! the exporter's inverse, for the runtime to serve at the load.
//!
//! A mod's `mod.toml` maps a manifest address to a file in the mod folder:
//!
//! ```toml
//! [assets]
//! 0x0836b9e0 = "assets/truck.png"           # a tile sheet: PNG
//! 0x084975f8 = "assets/brendan.aseprite"    # a sprite sheet: Aseprite
//! 0x0836bcfc = "assets/truck.pal"           # a palette: JASC
//! ```
//!
//! A tilemap takes its `.bin`. Each file must have the exported file's
//! shape (size, frames, colours): the game's buffers are sized from the
//! blob's header. An edited file only changes the pixels or colours.

use crate::graphics::{self, Store};
use crate::sprites::{self, Layout};
use std::collections::BTreeMap;
use std::path::Path;

/// What the manifest says of one blob: enough to lay its bytes out as the
/// exporter did.
#[derive(Clone, Debug)]
pub struct Entry {
    pub addr: u32,
    pub kind: Kind,
    pub store: Store,
    /// Bytes in the ROM (compressed, header included, for LZ77 and RL).
    pub size: u32,
    /// Tiles: bits a pixel, tiles across the PNG, and the frame layout of
    /// a sprite sheet.
    pub bpp: u8,
    pub width: usize,
    pub layout: Option<Layout>,
    /// A sprite sheet's frames and their size in tiles, from `[sprites]`:
    /// Aseprite's first `frames` frames are the sheet's. (A composed
    /// sprite's PNG keeps the plain layout, so `layout` may be `None`.)
    pub frames: Option<u32>,
    pub frame: Option<(u8, u8)>,
    /// A composed sprite: its Aseprite canvas is wider than a frame, with
    /// the sheet's frames at the top left and its arrangements after.
    pub composed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tiles,
    Palette,
    Tilemap,
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

/// `WxH` tiles.
fn size(s: &str) -> Option<(u8, u8)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// The manifest's `[tiles]`, `[palettes]` and `[tilemaps]`, by address,
/// with `[sprites]`' frame counts.
pub fn manifest(text: &str) -> Result<BTreeMap<u32, Entry>, String> {
    let doc: toml::Table = text.parse().map_err(|e| format!("manifest: {e}"))?;
    let mut out = BTreeMap::new();
    let table = |name: &str| doc.get(name).and_then(|t| t.as_table());
    for (section, kind) in [
        ("tiles", Kind::Tiles),
        ("palettes", Kind::Palette),
        ("tilemaps", Kind::Tilemap),
    ] {
        for (k, v) in table(section).into_iter().flatten() {
            let bad = || format!("manifest [{section}] {k}: malformed");
            let addr = hex(k).ok_or_else(bad)?;
            let get = |f: &str| v.get(f);
            let int = |f: &str| get(f).and_then(|x| x.as_integer());
            let store = match get("store").and_then(|x| x.as_str()) {
                Some("raw") => Store::Raw,
                Some("lz77") => Store::Lz77,
                Some("rl") => Store::Rl,
                _ => return Err(bad()),
            };
            let layout = match (
                get("frame").and_then(|x| x.as_str()),
                get("layout").and_then(|x| x.as_str()),
            ) {
                (Some(f), Some(l)) => {
                    let (w, h) = size(f).ok_or_else(bad)?;
                    Some(Layout {
                        w,
                        h,
                        across: l == "across",
                    })
                }
                _ => None,
            };
            out.insert(
                addr,
                Entry {
                    addr,
                    kind,
                    store,
                    size: int("size").ok_or_else(bad)? as u32,
                    bpp: int("bpp").unwrap_or(4) as u8,
                    width: int("width").unwrap_or(16) as usize,
                    layout,
                    frames: None,
                    frame: None,
                    composed: false,
                },
            );
        }
    }
    for (k, v) in table("sprites").into_iter().flatten() {
        let frame = v.get("size").and_then(|x| x.as_str()).and_then(size);
        if let (Some(a), Some(n)) = (hex(k), v.get("frames").and_then(|x| x.as_integer()))
            && let Some(e) = out.get_mut(&a)
        {
            e.frames = Some(n as u32);
            e.frame = frame;
            e.composed = v.get("composed").and_then(|x| x.as_bool()) == Some(true);
        }
    }
    Ok(out)
}

/// The bytes the blob at `e` unpacks to, from the ROM: what an override
/// replaces, and its exact length.
pub fn original(rom: &[u8], e: &Entry) -> Result<Vec<u8>, String> {
    let o = (e.addr & 0x01FF_FFFF) as usize;
    let at = rom
        .get(o..)
        .ok_or(format!("{:#010x}: past the ROM", e.addr))?;
    let data = match e.store {
        Store::Raw => at.get(..e.size as usize).map(<[u8]>::to_vec),
        Store::Lz77 => graphics::lz77(at).map(|d| d.0),
        Store::Rl => graphics::rl(at).map(|d| d.0),
    };
    data.ok_or(format!(
        "{:#010x}: the ROM's {} blob doesn't decode",
        e.addr,
        e.store.as_str()
    ))
}

/// The pixels a tile blob's exported PNG has: (width, height).
fn sheet_size(len: usize, e: &Entry) -> (usize, usize) {
    let zeros = vec![0u8; len];
    let (w, h, _) = match e.layout {
        Some(l) => sprites::sheet_pixels(&zeros, e.bpp, l),
        None => graphics::tile_pixels(&zeros, e.bpp, e.width),
    };
    (w, h)
}

/// Tiles from pixel indices `w` px across, as many as `data` holds whole,
/// written over `data`: [`graphics::tile_pixels`]' inverse.
fn put_tiles(data: &mut [u8], bpp: u8, w: usize, px: &[u8]) {
    let tsize = 8 * bpp as usize;
    let across = w / 8;
    for t in 0..data.len() / tsize {
        let (tx, ty) = (t % across * 8, t / across * 8);
        let tile = &mut data[t * tsize..(t + 1) * tsize];
        for y in 0..8 {
            for x in 0..8 {
                let v = px[(ty + y) * w + tx + x];
                if bpp == 4 {
                    let b = &mut tile[y * 4 + x / 2];
                    *b = if x % 2 == 0 {
                        *b & 0xF0 | v & 15
                    } else {
                        *b & 15 | v << 4
                    };
                } else {
                    tile[y * 8 + x] = v;
                }
            }
        }
    }
}

/// A sheet's pixels (laid out as the exporter laid them) back to its
/// bytes, over `data`.
fn put_sheet(data: &mut [u8], e: &Entry, w: usize, px: &[u8]) {
    match e.layout {
        Some(l) if l.across => {
            let (fw, fh) = (l.w as usize * 8, l.h as usize * 8);
            let fb = fw * fh * e.bpp as usize / 8;
            for (f, frame) in data.chunks_exact_mut(fb).enumerate() {
                let fpx: Vec<u8> = (0..fh)
                    .flat_map(|y| px[y * w + f * fw..y * w + f * fw + fw].iter().copied())
                    .collect();
                put_tiles(frame, e.bpp, fw, &fpx);
            }
        }
        _ => put_tiles(data, e.bpp, w, px),
    }
}

/// Every index fits `bpp` bits.
fn check_indices(px: &[u8], bpp: u8, what: &str) -> Result<(), String> {
    match px.iter().find(|&&i| (i as u32) >> bpp != 0) {
        Some(i) => Err(format!("{what}: index {i} doesn't fit {bpp} bits a pixel")),
        None => Ok(()),
    }
}

/// An indexed PNG of the exported sheet's size, to the blob's bytes.
pub fn from_png(png: &[u8], e: &Entry, original: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h, px, _) = graphics::read_indexed_png(png)?;
    let want = sheet_size(original.len(), e);
    if (w, h) != want {
        return Err(format!(
            "{w}x{h} px, the exported sheet is {}x{}",
            want.0, want.1
        ));
    }
    check_indices(&px, e.bpp, "the PNG")?;
    let mut data = original.to_vec();
    put_sheet(&mut data, e, w, &px);
    Ok(data)
}

/// One frame of an Aseprite file as indices: every visible layer's cel,
/// later layers over earlier, the file's transparent index showing what's
/// under it.
pub(crate) fn ase_frame(a: &aseprite::AsepriteFile, f: usize) -> Result<Vec<u8>, String> {
    use aseprite::CelKind;
    let (w, h) = (a.width() as usize, a.height() as usize);
    let clear = a.transparent_index();
    let mut px = vec![clear; w * h];
    for (i, l) in a.layers().iter().enumerate() {
        let Some(layer) = a.layer_ref(i).filter(|_| l.visible) else {
            continue;
        };
        let Some(cel) = a.resolve_cel(layer, f) else {
            continue;
        };
        let (p, x0, y0) = match &cel.kind {
            CelKind::Raw { pixels, x, y } | CelKind::Compressed { pixels, x, y, .. } => {
                (pixels, *x as isize, *y as isize)
            }
            _ => return Err(format!("frame {f}: a cel that isn't pixels")),
        };
        // A linked cel's own place wins over its source's.
        let (x0, y0) = match &a.cel(layer, f).map(|c| &c.kind) {
            Some(CelKind::Linked { x, y, .. }) => (*x as isize, *y as isize),
            _ => (x0, y0),
        };
        let pw = p.width as usize;
        for (k, &v) in p.data.iter().enumerate() {
            let (x, y) = (x0 + (k % pw) as isize, y0 + (k / pw) as isize);
            if v != clear && (0..w as isize).contains(&x) && (0..h as isize).contains(&y) {
                px[y as usize * w + x as usize] = v;
            }
        }
    }
    Ok(px)
}

/// An Aseprite sprite sheet to the blob's bytes: its first `frames`
/// frames are the sheet's, in order (the exporter's `sheet` tag); the
/// animation frames after them only show how the game plays them. A
/// composed sprite's sheet frames are the canvas's top left, and a part
/// of an arrangement (a cel on a `part` layer, its user data naming the
/// frame it shows) that differs from the ROM's frame is an edit of that
/// frame, unflipped, over the sheet's.
pub fn from_aseprite(file: &[u8], e: &Entry, original: &[u8]) -> Result<Vec<u8>, String> {
    let a = aseprite::AsepriteFile::from_reader(file).map_err(|x| format!("aseprite: {x}"))?;
    if a.color_mode() != aseprite::ColorMode::Indexed {
        return Err("not an indexed sprite".into());
    }
    let frames = e.frames.ok_or("not a sprite sheet in the manifest")? as usize;
    let (w, h) = e.frame.ok_or("the manifest gives the sprite no size")?;
    let (fw, fh) = (w as usize * 8, h as usize * 8);
    let (cw, ch) = (a.width() as usize, a.height() as usize);
    let fits = if e.composed {
        cw >= fw && ch >= fh
    } else {
        (cw, ch) == (fw, fh)
    };
    if !fits {
        return Err(format!(
            "{cw}x{ch} px, the sprite's frames are {fw}x{fh}{}",
            if e.composed { " (composed)" } else { "" }
        ));
    }
    if a.frames().len() < frames {
        return Err(format!(
            "{} frames, the sheet has {frames}",
            a.frames().len()
        ));
    }
    let fb = fw * fh * e.bpp as usize / 8;
    let crop = |px: &[u8], x0: i32, y0: i32| -> Vec<u8> {
        (0..fw * fh)
            .map(|k| {
                let (x, y) = (x0 + (k % fw) as i32, y0 + (k / fw) as i32);
                if (0..cw as i32).contains(&x) && (0..ch as i32).contains(&y) {
                    px[y as usize * cw + x as usize]
                } else {
                    0
                }
            })
            .collect()
    };
    let mut data = original.to_vec();
    for (f, chunk) in data.chunks_exact_mut(fb).take(frames).enumerate() {
        let px = crop(&ase_frame(&a, f)?, 0, 0);
        check_indices(&px, e.bpp, &format!("frame {f}"))?;
        put_tiles(chunk, e.bpp, fw, &px);
    }
    if !e.composed {
        return Ok(data);
    }
    let clear = a.transparent_index();
    for (i, l) in a.layers().iter().enumerate() {
        let Some(layer) = a
            .layer_ref(i)
            .filter(|_| l.visible && l.name.starts_with("part"))
        else {
            continue;
        };
        for f in frames..a.frames().len() {
            let Some((k, hflip, vflip, x0, y0)) = a
                .cel(layer, f)
                .and_then(|c| c.user_data.as_ref()?.text.as_deref())
                .and_then(sprites::parse_part)
            else {
                continue;
            };
            let Some(chunk) = original.chunks_exact(fb).nth(k as usize) else {
                return Err(format!("frame {f}: a part shows frame {k}, past the sheet"));
            };
            // The part's cel alone, in its frame's place.
            let mut px = vec![clear; cw * ch];
            if let Some(cel) = a.resolve_cel(layer, f) {
                let (p, cx, cy) = match &cel.kind {
                    aseprite::CelKind::Raw { pixels, x, y }
                    | aseprite::CelKind::Compressed { pixels, x, y, .. } => (pixels, *x, *y),
                    _ => return Err(format!("frame {f}: a cel that isn't pixels")),
                };
                let (cx, cy) = match &a.cel(layer, f).map(|c| &c.kind) {
                    Some(aseprite::CelKind::Linked { x, y, .. }) => (*x as i32, *y as i32),
                    _ => (cx as i32, cy as i32),
                };
                let pw = p.width as usize;
                for (n, &v) in p.data.iter().enumerate() {
                    let (x, y) = (cx + (n % pw) as i32, cy + (n / pw) as i32);
                    if (0..cw as i32).contains(&x) && (0..ch as i32).contains(&y) {
                        px[y as usize * cw + x as usize] = v;
                    }
                }
            }
            let shown = crop(&px, x0, y0);
            let mut px = vec![0; fw * fh];
            for (n, &v) in shown.iter().enumerate() {
                let (x, y) = (n % fw, n / fw);
                let (sx, sy) = (
                    if hflip { fw - 1 - x } else { x },
                    if vflip { fh - 1 - y } else { y },
                );
                px[sy * fw + sx] = if v == clear { 0 } else { v };
            }
            let (_, _, rom) = graphics::tile_pixels(chunk, e.bpp, w as usize);
            if px == rom {
                continue;
            }
            check_indices(&px, e.bpp, &format!("frame {f}'s part"))?;
            let at = k as usize * fb;
            put_tiles(&mut data[at..at + fb], e.bpp, fw, &px);
        }
    }
    Ok(data)
}

/// A JASC palette to the blob's bytes: as many colours as the blob holds.
pub fn from_pal(text: &str, original: &[u8]) -> Result<Vec<u8>, String> {
    let cs = graphics::read_jasc(text).ok_or("not a JASC palette")?;
    if cs.len() != original.len() / 2 {
        return Err(format!(
            "{} colours, the palette has {}",
            cs.len(),
            original.len() / 2
        ));
    }
    let mut data = original.to_vec();
    for (i, c) in cs.iter().enumerate() {
        data[2 * i..2 * i + 2].copy_from_slice(&c.to_le_bytes());
    }
    Ok(data)
}

/// One file's override: the bytes its blob unpacks to.
pub fn encode(rom: &[u8], e: &Entry, path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::read(path).map_err(|x| format!("{}: {x}", path.display()))?;
    let orig = original(rom, e)?;
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
    match (e.kind, ext) {
        (Kind::Tiles, "png") => from_png(&file, e, &orig),
        (Kind::Tiles, "aseprite" | "ase") => from_aseprite(&file, e, &orig),
        (Kind::Palette, "pal") => from_pal(&String::from_utf8_lossy(&file), &orig),
        (Kind::Tilemap, "bin") if file.len() == orig.len() => Ok(file),
        (Kind::Tilemap, "bin") => Err(format!(
            "{} bytes, the tilemap has {}",
            file.len(),
            orig.len()
        )),
        (k, _) => Err(format!("a {k:?} blob takes no .{ext} file")),
    }
    .map_err(|x| format!("{}: {x}", path.display()))
}

/// A mod's overrides, encoded.
#[derive(Debug, Default)]
pub struct Overrides {
    /// Compressed blobs: (ROM address, unpacked bytes), sorted, for the
    /// runtime's LZ77 and RL (`runtime::bus::Bus::asset_overrides`).
    pub unpacked: Vec<(u32, Vec<u8>)>,
    /// Raw blobs: (ROM address, bytes), sorted, for a patched ROM copy.
    pub raw: Vec<(u32, Vec<u8>)>,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.unpacked.is_empty() && self.raw.is_empty()
    }

    /// The ROM with the raw overrides written in, same length; `None`
    /// when there are none.
    pub fn patched(&self, rom: &[u8]) -> Option<Vec<u8>> {
        if self.raw.is_empty() {
            return None;
        }
        let mut out = rom.to_vec();
        for (a, b) in &self.raw {
            let o = (a & 0x01FF_FFFF) as usize;
            out[o..o + b.len()].copy_from_slice(b);
        }
        Some(out)
    }
}

/// The `[assets]` table of the mod at `dir` (`dir/mod.toml`), encoded
/// against the manifest's layouts and the ROM's blobs.
pub fn load(dir: &Path, manifest_text: &str, rom: &[u8]) -> Result<Overrides, String> {
    let path = dir.join("mod.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: toml::Table = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let entries = manifest(manifest_text)?;
    let mut out = Overrides::default();
    let Some(assets) = doc.get("assets").and_then(|t| t.as_table()) else {
        return Err(format!("{}: no [assets] table", path.display()));
    };
    for (k, v) in assets {
        let bad = |why: &str| format!("{} [assets] {k}: {why}", path.display());
        let addr = hex(k).ok_or_else(|| bad("not an address"))?;
        let file = v.as_str().ok_or_else(|| bad("not a file name"))?;
        let e = entries
            .get(&addr)
            .ok_or_else(|| bad("not a blob in the manifest"))?;
        let bytes = encode(rom, e, &dir.join(file))?;
        match e.store {
            Store::Raw => out.raw.push((addr, bytes)),
            _ => out.unpacked.push((addr, bytes)),
        }
    }
    out.raw.sort();
    out.unpacked.sort();
    Ok(out)
}

/// The exported file of each blob that an override takes: a sprite
/// sheet's Aseprite file, a tile sheet's PNG, a palette's `.pal`, a
/// tilemap's `.bin` (`files` in the manifest, relative to the export).
pub fn exported_files(manifest_text: &str) -> Result<Vec<(u32, Vec<String>)>, String> {
    let doc: toml::Table = manifest_text
        .parse()
        .map_err(|e| format!("manifest: {e}"))?;
    let mut out: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for section in ["tiles", "palettes", "tilemaps", "sprites"] {
        for (k, v) in doc
            .get(section)
            .and_then(|t| t.as_table())
            .into_iter()
            .flatten()
        {
            let Some(a) = hex(k) else { continue };
            let files = v.get("files").and_then(|f| f.as_array());
            for f in files.into_iter().flatten().filter_map(|f| f.as_str()) {
                let ext = Path::new(f).extension().and_then(|x| x.to_str());
                let takes = match section {
                    "tiles" => ext == Some("png"),
                    "palettes" => ext == Some("pal"),
                    "tilemaps" => ext == Some("bin"),
                    _ => ext == Some("aseprite"),
                };
                let v = out.entry(a).or_default();
                if takes && !v.iter().any(|x| x == f) {
                    v.push(f.to_string());
                }
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// `check-overrides round-trip`: every exported file encoded back
/// ([`encode`]) against its blob's bytes in the ROM, and an unedited mod
/// of them all at `mod_dir` (each blob's Aseprite file if it has one,
/// else its first file), for the in-game check. Returns the report and
/// how many files failed.
pub fn round_trip(rom: &[u8], out: &Path, mod_dir: &Path) -> Result<(String, usize), String> {
    let text = std::fs::read_to_string(out.join("manifest.toml"))
        .map_err(|e| format!("{}: {e}", out.join("manifest.toml").display()))?;
    let entries = manifest(&text)?;
    let mut report = String::new();
    let mut toml = String::from(
        "# Every exported file unedited (gbaa-assets check-overrides round-trip):\n\
         # running with it must draw what the game draws without it.\n[assets]\n",
    );
    let (mut files, mut failed) = (0, 0);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for (a, fs) in exported_files(&text)? {
        let Some(e) = entries.get(&a) else { continue };
        let orig = original(rom, e)?;
        for f in &fs {
            files += 1;
            let ext = f.rsplit('.').next().unwrap_or("").to_string();
            match encode(rom, e, &out.join(f)) {
                Ok(b) if b == orig => *kinds.entry(ext).or_default() += 1,
                Ok(b) => {
                    failed += 1;
                    let at = b.iter().zip(&orig).position(|(x, y)| x != y);
                    report.push_str(&format!("{a:#010x} {f}: differs at byte {at:?}\n"));
                }
                Err(x) => {
                    failed += 1;
                    report.push_str(&format!("{a:#010x} {f}: {x}\n"));
                }
            }
        }
        let pick = fs.iter().find(|f| f.ends_with(".aseprite")).or(fs.first());
        if let Some(f) = pick {
            toml.push_str(&format!("{a:#010x} = \"../../{f}\"\n"));
        }
    }
    std::fs::create_dir_all(mod_dir).map_err(|e| format!("{}: {e}", mod_dir.display()))?;
    let p = mod_dir.join("mod.toml");
    std::fs::write(&p, toml).map_err(|e| format!("{}: {e}", p.display()))?;
    let summary = kinds
        .iter()
        .map(|(k, n)| format!("{n} .{k}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok((
        format!(
            "round trip: {} of {files} exported files encode back to their blob's bytes ({summary})\n{report}",
            files - failed
        ),
        failed,
    ))
}

/// Pixel indices with a mark drawn over them, `w` px across: index 1 on
/// each 8x8 tile's top row and left column (a tile sheet), or across
/// rows `rows` (a sprite), so the edit shows in game.
fn mark(px: &mut [u8], w: usize, rows: Option<std::ops::Range<usize>>) {
    for (k, p) in px.iter_mut().enumerate() {
        let (x, y) = (k % w, k / w);
        let on = match &rows {
            Some(r) => r.contains(&y),
            None => x % 8 == 0 || y % 8 == 0,
        };
        if on {
            *p = 1;
        }
    }
}

/// `check-overrides edit`: a mod at `mod_dir` that edits two exported
/// files as an artist would: the Aseprite sprite at `sprite` gets a band
/// across every sheet frame, the tile sheet PNG at `sheet` a grid line on
/// every tile. Both written with the libraries an editor's file is read
/// with, under `mod_dir/assets/`.
pub fn edited_mod(out: &Path, mod_dir: &Path, sprite: u32, sheet: u32) -> Result<String, String> {
    let text = std::fs::read_to_string(out.join("manifest.toml"))
        .map_err(|e| format!("{}: {e}", out.join("manifest.toml").display()))?;
    let files: BTreeMap<u32, Vec<String>> = exported_files(&text)?.into_iter().collect();
    let find = |a: u32, ext: &str| {
        files
            .get(&a)
            .and_then(|fs| fs.iter().find(|f| f.ends_with(ext)).cloned())
            .ok_or(format!("{a:#010x}: no exported .{ext} file"))
    };
    let dir = mod_dir.join("assets");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let write = |name: &str, b: &[u8]| {
        std::fs::write(dir.join(name), b).map_err(|e| format!("{}: {e}", dir.join(name).display()))
    };
    // The sprite.
    let sf = find(sprite, ".aseprite")?;
    let bytes = std::fs::read(out.join(&sf)).map_err(|e| format!("{sf}: {e}"))?;
    let mut a =
        aseprite::AsepriteFile::from_reader(&bytes[..]).map_err(|e| format!("{sf}: {e}"))?;
    let frames = manifest(&text)?
        .get(&sprite)
        .and_then(|e| e.frames)
        .ok_or(format!("{sprite:#010x}: not a sprite sheet"))? as usize;
    let (w, h) = (a.width(), a.height());
    let layer = a.layer_ref(0).ok_or(format!("{sf}: no layer"))?;
    for f in 0..frames {
        let mut px = ase_frame(&a, f)?;
        let band = h as usize / 3;
        mark(&mut px, w as usize, Some(band..band + 3));
        let p = aseprite::Pixels::new(px, w, h, aseprite::ColorMode::Indexed)
            .map_err(|e| format!("{sf}: {e}"))?;
        a.set_cel(layer, f, p, 0, 0)
            .map_err(|e| format!("{sf}: {e}"))?;
    }
    let mut ase = Vec::new();
    a.write_to(&mut ase).map_err(|e| format!("{sf}: {e}"))?;
    write("sprite.aseprite", &ase)?;
    // The tile sheet.
    let tf = find(sheet, ".png")?;
    let png = std::fs::read(out.join(&tf)).map_err(|e| format!("{tf}: {e}"))?;
    let (pw, ph, mut px, pal) = graphics::read_indexed_png(&png)?;
    mark(&mut px, pw, None);
    let bpp = manifest(&text)?.get(&sheet).map_or(4, |e| e.bpp);
    write("sheet.png", &graphics::indexed_png(pw, ph, bpp, &px, &pal)?)?;
    let toml = format!(
        "# Two exported files edited (gbaa-assets check-overrides edit): a band\n\
         # across {sf}'s frames, a grid on {tf}'s tiles.\n[assets]\n\
         {sprite:#010x} = \"assets/sprite.aseprite\"\n{sheet:#010x} = \"assets/sheet.png\"\n"
    );
    let p = mod_dir.join("mod.toml");
    std::fs::write(&p, toml).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(format!(
        "edited mod: {} ({sf} with a band, {tf} with a grid)",
        mod_dir.display()
    ))
}

/// A `--frame-hashes` file: each frame's hash, by frame.
fn hashes(text: &str) -> BTreeMap<u64, String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            match (w.next(), w.next(), w.next(), w.next()) {
                (Some("frame"), Some(n), Some("hash"), Some(h)) => {
                    Some((n.parse().ok()?, h.into()))
                }
                _ => None,
            }
        })
        .collect()
}

/// `check-overrides frames`: the run with the unedited mod draws every
/// frame the plain run does, and the run with the edited mod differs at
/// each of `differ` (the frames the edited assets are on screen). Returns
/// the report, or why it fails.
pub fn check_frames(
    plain: &str,
    unedited: &str,
    edited: &str,
    differ: &[u64],
) -> Result<String, String> {
    let (p, u, e) = (hashes(plain), hashes(unedited), hashes(edited));
    if p.is_empty() || p.len() != u.len() || p.len() != e.len() {
        return Err(format!(
            "frame counts differ: plain {}, unedited {}, edited {}",
            p.len(),
            u.len(),
            e.len()
        ));
    }
    if let Some((n, _)) = p.iter().find(|(n, h)| u.get(n) != Some(h)) {
        return Err(format!(
            "the unedited mod's frame {n} differs from the plain run's"
        ));
    }
    let first = p.iter().find(|(n, h)| e.get(n) != Some(h)).map(|x| *x.0);
    let same: Vec<u64> = differ
        .iter()
        .copied()
        .filter(|n| p.get(n).is_none_or(|h| e.get(n) == Some(h)))
        .collect();
    if !same.is_empty() {
        return Err(format!(
            "the edited mod draws frames {same:?} as the plain run does"
        ));
    }
    let changed = p.iter().filter(|(n, h)| e.get(n) != Some(h)).count();
    Ok(format!(
        "frames: the unedited mod draws all {} frames as the plain run; the edited mod \
         changes {changed} from frame {}, including {differ:?}\n",
        p.len(),
        first.unwrap_or(0)
    ))
}
