//! Tile sheets, palettes and BG tilemaps out of a ROM
//! (docs/asset-extraction-design.md, Graphics), from what recording runs
//! logged (`loads.tsv`, `uses.tsv`, the load-log unit's
//! `gbaa-desktop --assets-log`).
//!
//! The asset is the source blob, keyed by its ROM address: a compressed
//! blob (LZ77 or RL) or a raw range, merged from the copies that touch.
//! A blob is a **tile sheet** when a use drew tiles from it, a **palette**
//! when a shown bank's colours came from it, and a **tilemap** when it
//! reached BG VRAM as neither. Palettes are paired by use: the palette a
//! tile blob was drawn with most.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

pub const ROM_BASE: u32 = 0x0800_0000;

/// Frames within which two loads into VRAM are taken as one scene's.
const SCENE: u64 = 60;

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

fn is_rom(a: u32) -> bool {
    (0x0800_0000..0x0E00_0000).contains(&a)
}

/// BG VRAM (charblocks and screenblocks), where tilemaps go.
fn is_bg_vram(a: u32) -> bool {
    (0x0600_0000..0x0601_0000).contains(&a)
}

/// Decodes the BIOS's LZ77 (type 1) at `src`: the unpacked bytes and how
/// many bytes were read, header included.
pub fn lz77(src: &[u8]) -> Option<(Vec<u8>, usize)> {
    let h = u32::from_le_bytes(src.get(..4)?.try_into().ok()?);
    if h & 0xF0 != 0x10 {
        return None;
    }
    let size = (h >> 8) as usize;
    let mut out = Vec::with_capacity(size);
    let mut i = 4;
    while out.len() < size {
        let flags = *src.get(i)?;
        i += 1;
        for bit in (0..8).rev() {
            if out.len() >= size {
                break;
            }
            if flags >> bit & 1 == 0 {
                out.push(*src.get(i)?);
                i += 1;
            } else {
                let (b0, b1) = (*src.get(i)? as usize, *src.get(i + 1)? as usize);
                i += 2;
                let len = (b0 >> 4) + 3;
                let disp = ((b0 & 0xF) << 8 | b1) + 1;
                if disp > out.len() {
                    return None;
                }
                for _ in 0..len {
                    if out.len() >= size {
                        break;
                    }
                    out.push(out[out.len() - disp]);
                }
            }
        }
    }
    Some((out, i))
}

/// Decodes the BIOS's run-length format (type 3), as [`lz77`].
pub fn rl(src: &[u8]) -> Option<(Vec<u8>, usize)> {
    let h = u32::from_le_bytes(src.get(..4)?.try_into().ok()?);
    if h & 0xF0 != 0x30 {
        return None;
    }
    let size = (h >> 8) as usize;
    let mut out = Vec::with_capacity(size);
    let mut i = 4;
    while out.len() < size {
        let f = *src.get(i)? as usize;
        i += 1;
        if f & 0x80 != 0 {
            let b = *src.get(i)?;
            i += 1;
            out.extend(std::iter::repeat_n(b, (f & 0x7F) + 3));
        } else {
            let n = (f & 0x7F) + 1;
            out.extend_from_slice(src.get(i..i + n)?);
            i += n;
        }
    }
    out.truncate(size);
    Some((out, i))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Store {
    Raw,
    Lz77,
    Rl,
}

impl Store {
    pub fn as_str(self) -> &'static str {
        match self {
            Store::Raw => "raw",
            Store::Lz77 => "lz77",
            Store::Rl => "rl",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Tiles,
    Palette,
    Tilemap,
}

/// One load from the load log, as far as export needs it.
#[derive(Debug, Clone)]
pub struct Load {
    pub frame: u64,
    pub op: String,
    pub src: u32,
    pub src_len: u32,
    pub dst: u32,
    pub dst_len: u32,
    /// For a RAM source, the ROM addresses its runs came from.
    pub origins: Vec<u32>,
}

pub fn parse_loads(text: &str) -> Result<Vec<Load>, String> {
    let mut out = Vec::new();
    for l in text.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let f: Vec<&str> = l.split('\t').collect();
        let bad = || format!("loads.tsv: bad line {l:?}");
        let [frame, op, src, src_len, dst, dst_len, runs] = f[..] else {
            return Err(bad());
        };
        let origins = runs
            .split(',')
            .filter_map(|r| hex(r.split('/').next()?))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        out.push(Load {
            frame: frame.parse().map_err(|_| bad())?,
            op: op.into(),
            src: hex(src).ok_or_else(bad)?,
            src_len: src_len.parse().map_err(|_| bad())?,
            dst: hex(dst).ok_or_else(bad)?,
            dst_len: dst_len.parse().map_err(|_| bad())?,
            origins,
        });
    }
    Ok(out)
}

/// One distinct use from the use log.
#[derive(Debug, Clone)]
pub struct Use {
    pub bg: bool,
    pub bpp: u8,
    pub blob: u32,
    pub blob_off: u32,
    pub bank: u8,
    /// The ROM address the bank's colours came from and their offset in
    /// that blob's unpacked bytes.
    pub pal: Option<(u32, u32)>,
    pub first: u64,
    pub frames: u64,
}

pub fn parse_uses(text: &str) -> Result<Vec<Use>, String> {
    let mut out = Vec::new();
    for l in text.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let f: Vec<&str> = l.split('\t').collect();
        let bad = || format!("uses.tsv: bad line {l:?}");
        let [
            layer,
            bpp,
            _,
            _,
            _,
            _,
            _,
            blob,
            blob_off,
            bank,
            pal_src,
            _,
            pal_off,
            first,
            _,
            frames,
        ] = f[..]
        else {
            return Err(bad());
        };
        let Some(blob) = hex(blob) else { continue };
        out.push(Use {
            bg: layer.starts_with("bg"),
            bpp: bpp.parse().map_err(|_| bad())?,
            blob,
            blob_off: blob_off.parse().map_err(|_| bad())?,
            bank: bank.parse().map_err(|_| bad())?,
            pal: hex(pal_src).zip(pal_off.parse().ok()),
            first: first.parse().map_err(|_| bad())?,
            frames: frames.parse().map_err(|_| bad())?,
        });
    }
    Ok(out)
}

/// An exported blob.
#[derive(Debug, Clone)]
pub struct Blob {
    pub addr: u32,
    pub store: Store,
    /// Bytes in the ROM (compressed: header included, rounded to 4).
    pub size: u32,
    pub data: Vec<u8>,
    pub kind: Kind,
    pub first_frame: u64,
    /// The first VRAM address it reached, if it did.
    pub vram: Option<u32>,
}

/// A palette pairing: (palette blob, byte offset in its unpacked bytes).
pub type Pairing = (u32, u32);

/// Everything export found for one ROM.
#[derive(Debug, Default)]
pub struct Found {
    pub blobs: BTreeMap<u32, Blob>,
    /// Tile blob: its bpp, uses (BG/OBJ), and pairings with frames shown.
    pub tiles: BTreeMap<u32, TileInfo>,
    /// Sprite sheets' frame layouts ([`crate::sprites`]), by tile blob.
    pub layouts: BTreeMap<u32, crate::sprites::Layout>,
}

#[derive(Debug, Default, Clone)]
pub struct TileInfo {
    pub bpp: u8,
    pub bg: bool,
    pub obj: bool,
    /// (bank, pairing) → frames shown.
    pub pairings: BTreeMap<(u8, Pairing), u64>,
    /// BG uses: (byte offset in the blob, bank).
    pub bg_tiles: BTreeSet<(u32, u8)>,
}

impl TileInfo {
    /// The pairing drawn most, over all banks.
    pub fn palette(&self) -> Option<Pairing> {
        let mut by: BTreeMap<Pairing, u64> = BTreeMap::new();
        for (&(_, p), &n) in &self.pairings {
            *by.entry(p).or_default() += n;
        }
        by.into_iter()
            .max_by_key(|&(p, n)| (n, std::cmp::Reverse(p)))
            .map(|(p, _)| p)
    }

    /// Bank → its most drawn pairing.
    pub fn banks(&self) -> BTreeMap<u8, Pairing> {
        let mut best: BTreeMap<u8, (u64, Pairing)> = BTreeMap::new();
        for (&(b, p), &n) in &self.pairings {
            let e = best.entry(b).or_insert((0, p));
            if n > e.0 {
                *e = (n, p);
            }
        }
        best.into_iter().map(|(b, (_, p))| (b, p)).collect()
    }
}

/// Builds the blobs from every route's logs of one ROM.
pub fn find(rom: &[u8], logs: &[(Vec<Load>, Vec<Use>)]) -> Found {
    let at = |a: u32| rom.get((a - ROM_BASE) as usize..);
    let mut found = Found::default();
    // Compressed blobs, by address.
    let mut packed: BTreeMap<u32, (Store, Vec<u8>, u32, u64)> = BTreeMap::new();
    // Raw ROM ranges copied: (start, end, first frame).
    let mut raw: Vec<(u32, u32, u64)> = Vec::new();
    // ROM origins that reached BG VRAM, with the VRAM address.
    let mut to_vram: BTreeMap<u32, (u32, u64)> = BTreeMap::new();
    // Every load into BG VRAM in time order: (route, frame, start, end,
    // origins). Frames restart at 0 in each route, so only loads of one
    // route compare by frame.
    let mut timeline: Vec<(usize, u64, u32, u32, Vec<u32>)> = Vec::new();
    for (route, (loads, _)) in logs.iter().enumerate() {
        for l in loads {
            if is_rom(l.src) {
                let store = match l.op.as_str() {
                    "lz77" => Some(Store::Lz77),
                    "rl" => Some(Store::Rl),
                    _ => None,
                };
                match store {
                    Some(s) => {
                        let dec = at(l.src).and_then(|b| match s {
                            Store::Lz77 => lz77(b),
                            _ => rl(b),
                        });
                        if let Some((data, read)) = dec {
                            let size = (read as u32).div_ceil(4) * 4;
                            packed.entry(l.src).or_insert((s, data, size, l.frame));
                        }
                    }
                    None if matches!(l.op.as_str(), "dma" | "cpuset" | "fastset") => {
                        raw.push((l.src, l.src + l.src_len, l.frame));
                    }
                    None => {}
                }
                if is_bg_vram(l.dst) {
                    to_vram.entry(l.src).or_insert((l.dst, l.frame));
                    timeline.push((route, l.frame, l.dst, l.dst + l.dst_len, vec![l.src]));
                }
            } else if is_bg_vram(l.dst) {
                for &o in &l.origins {
                    to_vram.entry(o).or_insert((l.dst, l.frame));
                }
                timeline.push((route, l.frame, l.dst, l.dst + l.dst_len, l.origins.clone()));
            }
        }
    }
    // What uses say: tile blobs and palette sources.
    let mut tile_srcs: BTreeSet<u32> = BTreeSet::new();
    let mut pal_srcs: BTreeSet<u32> = BTreeSet::new();
    for (_, uses) in logs {
        // A use shown one frame only is a screen mid-change (a map's bytes
        // read as tiles while a scene loads), not a tile sheet.
        for u in uses.iter().filter(|u| u.frames > 1) {
            tile_srcs.insert(u.blob);
            if let Some((p, _)) = u.pal {
                pal_srcs.insert(p);
            }
        }
    }
    // Raw ranges merged where they overlap (copies that only touch are
    // separate symbols as often as not), kept apart by kind: a palette
    // range never merges into a tile range.
    let kind_of = |s: u32, e: u32| {
        if pal_srcs.range(s..e).next().is_some() {
            Kind::Palette
        } else if tile_srcs.range(s..e).next().is_some() {
            Kind::Tiles
        } else {
            Kind::Tilemap
        }
    };
    raw.sort();
    let mut merged: Vec<(u32, u32, u64, Kind)> = Vec::new();
    for (s, e, f) in raw {
        let k = kind_of(s, e);
        match merged.last_mut() {
            Some(m) if s < m.1 && m.3 == k => {
                m.1 = m.1.max(e);
                m.2 = m.2.min(f);
            }
            _ => merged.push((s, e, f, k)),
        }
    }
    // A raw blob that isn't drawn as tiles or a palette must have reached BG
    // VRAM to be a tilemap; the rest (code, tables) isn't graphics.
    for (s, e, f, k) in merged {
        let vram = to_vram.range(s..e).next().map(|(_, v)| v.0);
        if k == Kind::Tilemap && vram.is_none() {
            continue;
        }
        let Some(data) = rom.get((s - ROM_BASE) as usize..(e - ROM_BASE) as usize) else {
            continue;
        };
        found.blobs.insert(
            s,
            Blob {
                addr: s,
                store: Store::Raw,
                size: e - s,
                data: data.to_vec(),
                kind: k,
                first_frame: f,
                vram,
            },
        );
    }
    for (a, (store, data, size, f)) in packed {
        let kind = if pal_srcs.contains(&a) {
            Kind::Palette
        } else if tile_srcs.contains(&a) {
            Kind::Tiles
        } else if to_vram.contains_key(&a) {
            Kind::Tilemap
        } else {
            continue;
        };
        found.blobs.insert(
            a,
            Blob {
                addr: a,
                store,
                size,
                data,
                kind,
                first_frame: f,
                vram: to_vram.get(&a).map(|v| v.0),
            },
        );
    }
    // A blob written over part of a tile sheet's VRAM (a tileset's
    // animation frames) is tiles too, though never drawn on the route.
    timeline.sort_by_key(|t| (t.0, t.1));
    let tilemaps: Vec<u32> = found
        .blobs
        .values()
        .filter(|b| b.kind == Kind::Tilemap)
        .map(|b| b.addr)
        .collect();
    for a in tilemaps {
        let size = found.blobs[&a].size;
        let Some(i) = timeline
            .iter()
            .position(|t| t.4.iter().any(|&o| o >= a && o < a + size.max(1)))
        else {
            continue;
        };
        let (route, frame, dst) = (timeline[i].0, timeline[i].1, timeline[i].2);
        let before = timeline[..i]
            .iter()
            .rev()
            .take_while(|t| t.0 == route)
            .find(|t| t.2 <= dst && dst < t.3)
            .filter(|t| frame - t.1 <= SCENE);
        let over_tiles = before.is_some_and(|t| {
            t.4.iter().any(|o| {
                found
                    .blobs
                    .range(..=o)
                    .next_back()
                    .is_some_and(|(&s, b)| *o < s + b.size.max(1) && b.kind == Kind::Tiles)
            })
        });
        if over_tiles && let Some(b) = found.blobs.get_mut(&a) {
            b.kind = Kind::Tiles;
        }
    }
    // A use's blob and palette to the blobs found.
    let blob_of = |found: &Found, a: u32| -> Option<u32> {
        let (&s, b) = found.blobs.range(..=a).next_back()?;
        (a < s + b.size.max(1) || a == s).then_some(s)
    };
    let mut tiles: BTreeMap<u32, TileInfo> = BTreeMap::new();
    for (_, uses) in logs {
        for u in uses {
            let Some(b) = blob_of(&found, u.blob) else {
                continue;
            };
            if found.blobs[&b].kind != Kind::Tiles {
                continue;
            }
            let t = tiles.entry(b).or_default();
            t.bpp = t.bpp.max(u.bpp);
            t.bg |= u.bg;
            t.obj |= !u.bg;
            if u.bg {
                t.bg_tiles.insert((u.blob - b + u.blob_off, u.bank));
            }
            if let Some((p, off)) = u.pal {
                // Compressed: the source is the blob, the offset is in its
                // unpacked bytes. Raw: the source is the colour's address.
                let pairing = match found.blobs.get(&p) {
                    Some(pb) if pb.store != Store::Raw => Some((p, off)),
                    _ => blob_of(&found, p).map(|pb| (pb, p - pb + off)),
                };
                if let Some(pr) = pairing.filter(|pr| found.blobs[&pr.0].kind == Kind::Palette) {
                    *t.pairings.entry((u.bank, pr)).or_default() += u.frames;
                }
            }
        }
    }
    found.tiles = tiles;
    found
}

/// BGR555 to 8-bit RGB, gbagfx's conversion.
pub fn rgb(c: u16) -> [u8; 3] {
    let ch = |s: u16| ((s & 31) as u32 * 255 / 31) as u8;
    [ch(c), ch(c >> 5), ch(c >> 10)]
}

pub fn colours(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect()
}

/// A JASC palette, pret's `.pal`.
pub fn jasc(cs: &[u16]) -> String {
    let mut s = format!("JASC-PAL\r\n0100\r\n{}\r\n", cs.len());
    for &c in cs {
        let [r, g, b] = rgb(c);
        let _ = write!(s, "{r} {g} {b}\r\n");
    }
    s
}

/// Reads a JASC palette back to BGR555.
pub fn read_jasc(text: &str) -> Option<Vec<u16>> {
    let mut l = text.lines();
    if l.next()?.trim() != "JASC-PAL" {
        return None;
    }
    l.next()?;
    let n: usize = l.next()?.trim().parse().ok()?;
    (0..n)
        .map(|_| {
            let v: Vec<u16> = l
                .next()?
                .split_whitespace()
                .map(|x| x.parse().ok())
                .collect::<Option<_>>()?;
            let [r, g, b] = v[..] else { return None };
            Some(r >> 3 | (g >> 3) << 5 | (b >> 3) << 10)
        })
        .collect()
}

/// Tiles across a sheet: 16 (pret's usual width) when that leaves no gap,
/// else the widest from 16 down to 8 that divides the count, else 16.
pub fn sheet_width(tiles: usize) -> usize {
    if tiles <= 16 {
        return tiles.max(1);
    }
    (8..=16)
        .rev()
        .find(|&w| tiles.is_multiple_of(w))
        .unwrap_or(16)
}

/// A tile blob's pixels as indices, `w` tiles across: (width, height,
/// pixels).
pub fn tile_pixels(data: &[u8], bpp: u8, w: usize) -> (usize, usize, Vec<u8>) {
    let tsize = 8 * bpp as usize;
    let n = data.len() / tsize;
    let rows = n.div_ceil(w).max(1);
    let (pw, ph) = (w * 8, rows * 8);
    let mut px = vec![0u8; pw * ph];
    for t in 0..n {
        let (tx, ty) = (t % w * 8, t / w * 8);
        let tile = &data[t * tsize..(t + 1) * tsize];
        for y in 0..8 {
            for x in 0..8 {
                let v = if bpp == 4 {
                    let b = tile[y * 4 + x / 2];
                    if x % 2 == 0 { b & 15 } else { b >> 4 }
                } else {
                    tile[y * 8 + x]
                };
                px[(ty + y) * pw + tx + x] = v;
            }
        }
    }
    (pw, ph, px)
}

/// An indexed PNG, `bpp` bits a pixel, its palette in PLTE and index 0
/// transparent.
pub fn indexed_png(w: usize, h: usize, bpp: u8, px: &[u8], pal: &[u16]) -> Result<Vec<u8>, String> {
    indexed_png_text(w, h, bpp, px, pal, &[])
}

/// [`indexed_png`] with `(keyword, text)` chunks.
pub fn indexed_png_text(
    w: usize,
    h: usize,
    bpp: u8,
    px: &[u8],
    pal: &[u16],
    text: &[(&str, &str)],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w as u32, h as u32);
        for (k, t) in text {
            e.add_text_chunk(k.to_string(), t.to_string())
                .map_err(|e| e.to_string())?;
        }
        e.set_color(png::ColorType::Indexed);
        e.set_depth(if bpp == 4 {
            png::BitDepth::Four
        } else {
            png::BitDepth::Eight
        });
        let n = 1usize << bpp;
        let mut plte = Vec::with_capacity(n * 3);
        for i in 0..n {
            plte.extend(rgb(pal.get(i).copied().unwrap_or(0)));
        }
        e.set_palette(plte);
        e.set_trns(vec![0u8]);
        let mut wr = e.write_header().map_err(|e| e.to_string())?;
        let data: Vec<u8> = if bpp == 4 {
            px.chunks(w)
                .flat_map(|row| {
                    row.chunks(2)
                        .map(|p| p[0] << 4 | p.get(1).copied().unwrap_or(0))
                        .collect::<Vec<_>>()
                })
                .collect()
        } else {
            px.to_vec()
        };
        wr.write_image_data(&data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// An indexed PNG's size, pixel indices and palette (BGR555, `x >> 3`).
pub fn read_indexed_png(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>, Vec<u16>), String> {
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
    d.set_transformations(png::Transformations::IDENTITY);
    let mut r = d.read_info().map_err(|e| e.to_string())?;
    let info = r.info();
    if info.color_type != png::ColorType::Indexed {
        return Err(format!("not indexed ({:?})", info.color_type));
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let depth = info.bit_depth as usize;
    let pal = info
        .palette
        .as_ref()
        .map(|p| {
            p.as_chunks::<3>()
                .0
                .iter()
                .map(|c| (c[0] as u16 >> 3) | (c[1] as u16 >> 3) << 5 | (c[2] as u16 >> 3) << 10)
                .collect()
        })
        .unwrap_or_default();
    let mut buf = vec![0; r.output_buffer_size().ok_or("png too big")?];
    let fr = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let mut px = Vec::with_capacity(w * h);
    for row in buf[..fr.buffer_size()].chunks(fr.line_size).take(h) {
        for x in 0..w {
            let bit = x * depth;
            let b = row[bit / 8];
            px.push((b >> (8 - depth - bit % 8)) & ((1u16 << depth) - 1) as u8);
        }
    }
    Ok((w, h, px, pal))
}

/// An RGB PNG.
pub fn rgb_png(w: usize, h: usize, rgb: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w as u32, h as u32);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// A name usable as a file name.
fn file_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A blob's kind-based name, for a ROM with no source map entry there.
pub fn kind_name(b: &Blob) -> String {
    let p = match b.kind {
        Kind::Tiles => "gfx",
        Kind::Palette => "pal",
        Kind::Tilemap => "map",
    };
    format!("{p}_{:08x}", b.addr)
}

/// A blob's file name: its source map name, else its kind-based one.
pub fn blob_name(names: &BTreeMap<u32, String>, b: &Blob) -> String {
    names
        .get(&b.addr)
        .map_or_else(|| kind_name(b), |n| file_name(n))
}

/// `n` colours of a pairing.
pub fn pairing_colours(found: &Found, (p, off): Pairing, n: usize) -> Vec<u16> {
    let cs = found
        .blobs
        .get(&p)
        .map(|b| colours(&b.data))
        .unwrap_or_default();
    let s = (off / 2) as usize;
    (0..n)
        .map(|i| cs.get(s + i).copied().unwrap_or(0))
        .collect()
}

/// A greyscale ramp for a sheet never drawn with a palette.
pub fn grey(bpp: u8) -> Vec<u16> {
    let n = 1u32 << bpp;
    (0..n)
        .map(|i| {
            let v = (i * 31 / (n - 1)) as u16;
            v | v << 5 | v << 10
        })
        .collect()
}

/// What [`write`] wrote.
pub struct Written {
    pub manifest: String,
    pub counts: [usize; 3],
}

/// Writes tile sheets, palettes and tilemaps under `dir`, and returns the
/// manifest's `[tiles]`, `[palettes]` and `[tilemaps]`.
pub fn write(found: &Found, names: &BTreeMap<u32, String>, dir: &Path) -> Result<Written, String> {
    for d in ["tiles", "palettes", "tilemaps"] {
        std::fs::create_dir_all(dir.join(d)).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let name = |b: &Blob| blob_name(names, b);
    let put =
        |f: &str, bytes: &[u8]| std::fs::write(dir.join(f), bytes).map_err(|e| format!("{f}: {e}"));
    let pal_colours = |p: Pairing, n: usize| pairing_colours(found, p, n);
    let mut sections = [String::new(), String::new(), String::new()];
    let mut counts = [0; 3];
    let head = |b: &Blob| {
        let unpacked = if b.store == Store::Raw {
            String::new()
        } else {
            format!(", unpacked = {}", b.data.len())
        };
        format!(
            "{:#010x} = {{ store = \"{}\", size = {}{unpacked}, first_frame = {}, name = \"{}\"",
            b.addr,
            b.store.as_str(),
            b.size,
            b.first_frame,
            name(b)
        )
    };
    for b in found.blobs.values() {
        let n = name(b);
        match b.kind {
            Kind::Tiles => {
                let info = found.tiles.get(&b.addr).cloned().unwrap_or_default();
                let bpp = if info.bpp == 8 { 8 } else { 4 };
                let ntiles = b.data.len() / (8 * bpp as usize);
                let layout = found.layouts.get(&b.addr).copied();
                let (pw, ph, px) = match layout {
                    Some(l) => crate::sprites::sheet_pixels(&b.data, bpp, l),
                    None => tile_pixels(&b.data, bpp, sheet_width(ntiles)),
                };
                let w = pw / 8;
                let frames = layout.map_or(String::new(), |l| {
                    format!(
                        ", frame = \"{}x{}\", layout = \"{}\"",
                        l.w,
                        l.h,
                        if l.across { "across" } else { "down" }
                    )
                });
                let pal = info.palette();
                let cs = pal.map_or_else(|| grey(bpp), |p| pal_colours(p, 1 << bpp));
                let file = format!("tiles/{n}.png");
                put(&file, &indexed_png(pw, ph, bpp, &px, &cs)?)?;
                let all: BTreeSet<Pairing> = info.pairings.keys().map(|k| k.1).collect();
                let fmt_p = |p: &Pairing| {
                    if p.1 == 0 {
                        format!("{:#010x}", p.0)
                    } else {
                        format!("\"{:#010x}+{}\"", p.0, p.1)
                    }
                };
                let use_list: Vec<&str> = [(info.bg, "\"bg\""), (info.obj, "\"obj\"")]
                    .iter()
                    .filter(|u| u.0)
                    .map(|u| u.1)
                    .collect();
                let _ = writeln!(
                    sections[0],
                    "{}, bpp = {bpp}, tiles = {ntiles}, width = {w}{frames}, palette = {}, palettes = [{}], use = [{}], files = [\"{file}\"] }}",
                    head(b),
                    pal.as_ref().map_or("\"grey\"".into(), fmt_p),
                    all.iter().map(fmt_p).collect::<Vec<_>>().join(", "),
                    use_list.join(", "),
                );
                counts[0] += 1;
            }
            Kind::Palette => {
                let cs = colours(&b.data);
                let file = format!("palettes/{n}.pal");
                put(&file, jasc(&cs).as_bytes())?;
                let _ = writeln!(
                    sections[1],
                    "{}, colours = {}, files = [\"{file}\"] }}",
                    head(b),
                    cs.len()
                );
                counts[1] += 1;
            }
            Kind::Tilemap => {
                let file = format!("tilemaps/{n}.bin");
                put(&file, &b.data)?;
                let (drawn, tiles) = draw_tilemap(found, b, &pal_colours);
                let mut files = format!("\"{file}\"");
                if let Some((w, h, rgb)) = drawn {
                    let f = format!("tilemaps/{n}.png");
                    put(&f, &rgb_png(w, h, &rgb)?)?;
                    let _ = write!(files, ", \"{f}\"");
                }
                let _ = writeln!(
                    sections[2],
                    "{}, vram = {:#010x}, drawn_with = {}, files = [{files}] }}",
                    head(b),
                    b.vram.unwrap_or(0),
                    tiles.map_or("\"none\"".into(), |t| format!("{t:#010x}")),
                );
                counts[2] += 1;
            }
        }
    }
    let manifest = format!(
        "[tiles]\n{}\n[palettes]\n{}\n[tilemaps]\n{}",
        sections[0], sections[1], sections[2]
    );
    Ok(Written { manifest, counts })
}

/// A tilemap drawn with the tile blob most likely under it: the BG tile
/// blob whose uses show most of the map's (tile, bank) pairs, then the one
/// loaded nearest in time; each bank with that blob's pairing. To look at,
/// not to rebuild.
#[allow(clippy::type_complexity)]
fn draw_tilemap(
    found: &Found,
    map: &Blob,
    pal_colours: &dyn Fn(Pairing, usize) -> Vec<u16>,
) -> (Option<(usize, usize, Vec<u8>)>, Option<u32>) {
    let cands = found.blobs.values().filter(|b| {
        b.kind == Kind::Tiles
            && b.vram.is_some()
            && found.tiles.get(&b.addr).is_some_and(|t| t.bg && t.bpp == 4)
    });
    let entries = colours(&map.data);
    let score = |b: &Blob| {
        let tv = b.vram.unwrap_or(0x0600_0000);
        let used = &found.tiles[&b.addr].bg_tiles;
        entries
            .iter()
            .filter(|&&e| {
                let off = ((tv & !0x3FFF) + (e as u32 & 0x3FF) * 32).wrapping_sub(tv);
                used.contains(&(off, (e >> 12) as u8))
            })
            .count()
    };
    let Some(t) = cands.min_by_key(|b| {
        let d = b.first_frame as i64 - map.first_frame as i64;
        (std::cmp::Reverse(score(b)), d.unsigned_abs(), b.addr)
    }) else {
        return (None, None);
    };
    // No 4bpp text BG's uses show this map's tiles (an affine map's 8-bit
    // entries, say): drawing it would mislead.
    if score(t) == 0 {
        return (None, None);
    }
    let tv = t.vram.unwrap_or(0x0600_0000);
    let base = tv & !0x3FFF;
    let banks = found.tiles[&t.addr].banks();
    if entries.is_empty() {
        return (None, Some(t.addr));
    }
    let w = 32;
    let rows = entries.len().div_ceil(w);
    let (pw, ph) = (w * 8, rows * 8);
    let mut rgbv = vec![0u8; pw * ph * 3];
    for (i, &e) in entries.iter().enumerate() {
        let tile = (e & 0x3FF) as u32;
        let (hf, vf, bank) = (e & 0x400 != 0, e & 0x800 != 0, (e >> 12) as u8);
        let off = (base + tile * 32).wrapping_sub(tv) as usize;
        let Some(td) = t.data.get(off..off + 32) else {
            continue;
        };
        let cs = banks.get(&bank).map(|&p| pal_colours(p, 16));
        let (tx, ty) = (i % w * 8, i / w * 8);
        for y in 0..8 {
            for x in 0..8 {
                let (sx, sy) = (if hf { 7 - x } else { x }, if vf { 7 - y } else { y });
                let b = td[sy * 4 + sx / 2];
                let v = if sx % 2 == 0 { b & 15 } else { b >> 4 };
                let c = match &cs {
                    Some(cs) => rgb(cs[v as usize]),
                    None => [v * 17; 3],
                };
                let p = ((ty + y) * pw + tx + x) * 3;
                rgbv[p..p + 3].copy_from_slice(&c);
            }
        }
    }
    (Some((pw, ph, rgbv)), Some(t.addr))
}

/// The source map layer of kind-based names for blobs the map doesn't
/// name (docs/asset-extraction-design.md, The manifest).
pub fn names_layer(found: &Found, named: &BTreeMap<u32, String>, rom_sha1: &str) -> String {
    let mut s = format!(
        "# Names for asset blobs no other layer names, by kind and ROM address.\n# Written by `gbaa-assets graphics` (crates/assets). Don't edit; a name\n# given in a later layer wins.\n\n[map]\nrom_sha1 = \"{rom_sha1}\"\nsource = \"gbaa-assets\"\nformat = 1\n\n[data]\n"
    );
    for b in found
        .blobs
        .values()
        .filter(|b| !named.contains_key(&b.addr))
    {
        let _ = writeln!(
            s,
            "{:#010x} = {{ name = \"{}\", size = {} }}",
            b.addr,
            kind_name(b),
            b.size
        );
    }
    s
}

/// Replaces `sections` in a TOML manifest's text with `new` (which holds
/// them), keeping every other section. A `[manifest]` header is added if
/// there is none.
pub fn merge_manifest(old: &str, sections: &[&str], new: &str, header: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for l in old.lines() {
        if let Some(h) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            skip = sections.contains(&h);
        }
        // Each writer's own "Written by" line comes back with its sections.
        if !skip && !l.starts_with("# Written by `gbaa-assets") {
            out.push_str(l);
            out.push('\n');
        }
    }
    let mut out = out.trim_start_matches('\n').to_string();
    if !out.contains("[manifest]") {
        out = format!("{header}\n{out}");
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    format!("{out}\n{new}")
}
