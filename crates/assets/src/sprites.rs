//! Sprites and animations (docs/asset-extraction-design.md, Sprites and
//! animations): sprites assembled from what OAM showed along a route
//! (`oam.tsv`, a line each time an entry changes), the frames of each
//! sheet, each entry's sequence of frames and how long each stayed, and an
//! Aseprite file a sheet: indexed, its paired palette, one layer, the
//! sheet's frames then each animation's (linked cels), a tag an
//! animation, the ROM addresses in user data.
//!
//! A sheet is a tile blob shown by OAM at one size. Its frames are found
//! two ways, both through the tags of an entry's first tile: a whole sheet
//! in VRAM and the entry's tile number stepping through it (the frame is
//! the tile's offset in the blob), or one VRAM slot refilled from a
//! different ROM offset each time (Emerald's object events): raw copies
//! one frame long that follow each other in one entry are frames of one
//! sheet, joined to the source map's symbol when one holds them all. A
//! sheet refilled that way is laid out a frame across, pret's
//! `-mwidth`/`-mheight` strips; one in VRAM whole, a frame down, as
//! gbagfx reads it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use crate::graphics::{self, Blob, Found, Kind, Pairing, ROM_BASE, Store, TileInfo};

/// Where an entry's first tile came from: a compressed blob and the offset
/// in its unpacked bytes, or a ROM address copied as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Origin {
    Packed(u32, u32),
    Raw(u32),
}

/// An OAM entry as shown from `frame` on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obj {
    pub x: i32,
    pub y: i32,
    pub w: u8,
    pub h: u8,
    pub bpp: u8,
    pub one_d: bool,
    pub tile: u16,
    pub bank: u8,
    pub hflip: bool,
    pub vflip: bool,
    pub affine: bool,
    pub origin: Option<Origin>,
    /// The bank's colours: their source and offset, as in `uses.tsv`.
    pub pal: Option<(u32, u32)>,
}

/// One line of `oam.tsv`: (frame, entry, what it shows from then, `None`
/// once hidden).
pub type Line = (u64, u8, Option<Obj>);

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

pub fn parse_oam(text: &str) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    for l in text.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let f: Vec<&str> = l.split('\t').collect();
        let bad = || format!("oam.tsv: bad line {l:?}");
        let frame: u64 = f.first().and_then(|v| v.parse().ok()).ok_or_else(bad)?;
        let index: u8 = f.get(1).and_then(|v| v.parse().ok()).ok_or_else(bad)?;
        if f.get(2) == Some(&"hidden") {
            out.push((frame, index, None));
            continue;
        }
        let [
            _,
            _,
            x,
            y,
            size,
            bpp,
            mapping,
            tile,
            bank,
            flip,
            tile_src,
            tile_via,
            tile_off,
            _,
            _,
            pal_src,
            _,
            pal_off,
        ] = f[..]
        else {
            return Err(bad());
        };
        let (w, h) = size.split_once('x').ok_or_else(bad)?;
        let num = |s: &str| s.parse::<i64>().map_err(|_| bad());
        let origin = hex(tile_src)
            .filter(|_| !tile_via.contains("derived"))
            .map(|a| {
                if tile_via.contains("lz77") || tile_via.contains("unpack") {
                    Origin::Packed(a, tile_off.parse().unwrap_or(0))
                } else {
                    Origin::Raw(a)
                }
            });
        out.push((
            frame,
            index,
            Some(Obj {
                x: num(x)? as i32,
                y: num(y)? as i32,
                w: num(w)? as u8,
                h: num(h)? as u8,
                bpp: num(bpp)? as u8,
                one_d: mapping == "1d",
                tile: num(tile)? as u16,
                bank: num(bank)? as u8,
                hflip: flip.contains('h'),
                vflip: flip.contains('v'),
                affine: flip == "affine",
                origin,
                pal: hex(pal_src).zip(pal_off.parse().ok()),
            }),
        ));
    }
    Ok(out)
}

/// The last frame of a route: `shown.tsv`'s.
pub fn last_frame(shown: &str) -> Option<u64> {
    shown
        .lines()
        .next()?
        .strip_prefix("# every tile shown on frame ")?
        .trim()
        .parse()
        .ok()
}

/// An entry showing one thing over frames `[start, end)`.
#[derive(Clone, Debug)]
struct Span {
    start: u64,
    end: u64,
    index: u8,
    obj: Obj,
}

/// Each route's spans, a route's in time order a list an entry, and
/// whether the route ended while each list's last span showed.
fn spans(routes: &[(Vec<Line>, u64)]) -> Vec<Vec<(Vec<Span>, bool)>> {
    routes
        .iter()
        .map(|(lines, last)| {
            let mut by: BTreeMap<u8, Vec<(u64, Option<Obj>)>> = BTreeMap::new();
            for (f, i, o) in lines {
                by.entry(*i).or_default().push((*f, o.clone()));
            }
            by.into_iter()
                .map(|(i, ls)| {
                    let mut out = Vec::new();
                    let mut cut = false;
                    for (k, (f, o)) in ls.iter().enumerate() {
                        let Some(o) = o else { continue };
                        let end = ls.get(k + 1).map_or(last + 1, |n| n.0);
                        cut = k + 1 == ls.len();
                        out.push(Span {
                            start: *f,
                            end: end.max(*f + 1),
                            index: i,
                            obj: o.clone(),
                        });
                    }
                    (out, cut)
                })
                .collect()
        })
        .collect()
}

/// Bytes in one frame `w×h` tiles at `bpp`.
fn frame_bytes(w: u8, h: u8, bpp: u8) -> u32 {
    w as u32 * h as u32 * 8 * bpp as u32
}

/// How a sheet's frames are laid out in its PNG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub w: u8,
    pub h: u8,
    /// A frame across (pret's strips) or a frame down.
    pub across: bool,
}

/// One animation: a sequence of (frame, hflip, vflip) and how many video
/// frames each stayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anim {
    pub steps: Vec<((u32, bool, bool), u64)>,
    /// Repeats (seen twice or more in a row), else played once.
    pub looped: bool,
    /// Cut off by the route's end, or a loop seen less than twice.
    pub partial: bool,
    /// Where it was first seen: route, entry, frame.
    pub seen: (usize, u8, u64),
}

/// A sprite sheet.
#[derive(Clone, Debug)]
pub struct Sheet {
    pub addr: u32,
    pub w: u8,
    pub h: u8,
    pub bpp: u8,
    pub one_d: bool,
    pub across: bool,
    pub frames: u32,
    /// Frames shown, with the video frames each was shown.
    pub shown: BTreeMap<u32, u64>,
    pub bank: u8,
    pub palette: Option<Pairing>,
    pub anims: Vec<Anim>,
    /// Shown by two entries at once that kept their places: a sprite of
    /// several entries, composed on one canvas.
    pub composed: Option<Composed>,
}

/// One entry of a composed sprite: its place on the canvas in pixels and
/// the frame it shows.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Part {
    pub x: i32,
    pub y: i32,
    pub frame: u32,
    pub hflip: bool,
    pub vflip: bool,
}

/// One arrangement of a composed sprite's entries: its parts, the video
/// frames it stayed the first time, and where that was (route, frame).
pub type Arrangement = (Vec<Part>, u64, (usize, u64));

/// A sprite of several entries on one canvas.
#[derive(Clone, Debug, Default)]
pub struct Composed {
    /// The canvas in pixels: every arrangement fits.
    pub w: u16,
    pub h: u16,
    /// Each distinct arrangement the sheet's entries were shown in, first
    /// seen first, placed from its top-left part.
    pub arrangements: Vec<Arrangement>,
    /// One arrangement shows each of the sheet's frames once, in order, a
    /// frame across (`Some(true)`) or down: the sheet's PNG is laid out
    /// as that sprite (pret's `-mwidth` sheets of one sprite).
    pub grid: Option<bool>,
}

/// What [`find`] makes of the OAM logs.
#[derive(Debug, Default)]
pub struct Sprites {
    pub sheets: BTreeMap<u32, Sheet>,
    /// Tile blobs OAM showed that aren't a sheet of one frame size, and
    /// why.
    pub skipped: BTreeMap<u32, String>,
}

/// The blob in `found` holding ROM or blob address `a`.
fn blob_at(found: &Found, a: u32) -> Option<u32> {
    let (&s, b) = found.blobs.range(..=a).next_back()?;
    (a < s + b.size.max(1) || a == s).then_some(s)
}

struct Dsu(HashMap<u32, u32>);

impl Dsu {
    fn root(&mut self, a: u32) -> u32 {
        let p = *self.0.entry(a).or_insert(a);
        if p == a {
            return a;
        }
        let r = self.root(p);
        self.0.insert(a, r);
        r
    }
    fn join(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra != rb {
            self.0.insert(ra.max(rb), ra.min(rb));
        }
    }
}

/// The sheet a span's origin is in, as (sheet address, byte offset).
type Placed = (u32, u32);

/// An entry's size in tiles, bpp and 1D mapping.
type Shape = (u8, u8, u8, bool);

/// One entry's spans on one sheet in a row, each with its frame: (sheet,
/// spans, cut by the route's end, route).
type Track<'a> = (u32, Vec<(&'a Span, u32)>, bool, usize);

/// Two entries' offsets apart while shown together, and for how many
/// frames.
type Apart = (BTreeSet<(i32, i32)>, u64);

/// Finds the sheets and animations OAM showed, joins raw frames copied one
/// at a time into their sheet's blob in `found`, and sets the sheets'
/// layouts there. `sizes`: the source map's data symbols and sizes.
pub fn find(
    rom: &[u8],
    found: &mut Found,
    routes: &[(Vec<Line>, u64)],
    sizes: &BTreeMap<u32, u64>,
) -> Sprites {
    let all = spans(routes);
    // The source map's symbol holding an address: frames of two symbols
    // are two sheets.
    let symbol = |a: u32| {
        sizes
            .range(..=a)
            .next_back()
            .filter(|(s, n)| (a as u64) < **s as u64 + **n)
            .map(|(s, _)| *s)
    };
    // Raw frames that follow each other in one entry, same size, joined.
    let mut dsu = Dsu(HashMap::new());
    let mut raw_size: BTreeMap<u32, u32> = BTreeMap::new();
    for route in &all {
        for (list, _) in route {
            let mut prev: Option<(u32, u32)> = None;
            for s in list {
                let fb = frame_bytes(s.obj.w, s.obj.h, s.obj.bpp);
                let Some(Origin::Raw(a)) = s.obj.origin else {
                    prev = None;
                    continue;
                };
                dsu.root(a);
                let e = raw_size.entry(a).or_insert(fb);
                *e = (*e).max(fb);
                if let Some((p, pfb)) = prev
                    && pfb == fb
                    && p != a
                    && p.abs_diff(a).is_multiple_of(fb)
                    && p.abs_diff(a) <= 64 * fb
                    && symbol(p) == symbol(a)
                {
                    dsu.join(p, a);
                }
                prev = Some((a, fb));
            }
        }
    }
    // Each group's range: the blobs holding its frames, joined, widened to
    // the source map's symbol when that holds them all in whole frames.
    let mut groups: BTreeMap<u32, (u32, u32, u32)> = BTreeMap::new();
    let addrs: Vec<u32> = raw_size.keys().copied().collect();
    for a in addrs {
        let fb = raw_size[&a];
        let (s, e) = match blob_at(found, a) {
            Some(b) if found.blobs[&b].kind == Kind::Tiles => {
                let blob = &found.blobs[&b];
                (b, (b + blob.size).max(a + fb))
            }
            _ => (a, a + fb),
        };
        let r = dsu.root(a);
        let g = groups.entry(r).or_insert((s, e, fb));
        g.0 = g.0.min(s);
        g.1 = g.1.max(e);
    }
    let mut raw_sheet: BTreeMap<u32, u32> = BTreeMap::new();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for (&r, &(mut s, mut e, fb)) in &groups {
        if let Some((&sym, &len)) = sizes.range(..=s).next_back() {
            let end = sym as u64 + len;
            if end >= e as u64 && len.is_multiple_of(fb as u64) && (s - sym).is_multiple_of(fb) {
                (s, e) = (sym, end as u32);
            }
        }
        raw_sheet.insert(r, s);
        ranges.push((s, e));
    }
    // Join each range's blobs into one.
    ranges.sort();
    ranges.dedup();
    for &(s, e) in &ranges {
        let inside: Vec<u32> = found
            .blobs
            .range(s..e)
            .filter(|(_, b)| b.kind == Kind::Tiles && b.store == Store::Raw)
            .map(|(&a, _)| a)
            .collect();
        if inside == [s] && found.blobs[&s].size == e - s {
            continue;
        }
        let Some(data) = rom.get((s - ROM_BASE) as usize..(e - ROM_BASE) as usize) else {
            continue;
        };
        let mut info = TileInfo::default();
        let (mut first, mut vram) = (u64::MAX, None);
        for a in &inside {
            let b = found.blobs.remove(a).expect("listed");
            first = first.min(b.first_frame);
            vram = vram.or(b.vram);
            if let Some(t) = found.tiles.remove(a) {
                info.bpp = info.bpp.max(t.bpp);
                info.bg |= t.bg;
                info.obj |= t.obj;
                for (k, n) in t.pairings {
                    *info.pairings.entry(k).or_default() += n;
                }
                info.bg_tiles
                    .extend(t.bg_tiles.into_iter().map(|(o, b)| (o + a - s, b)));
            }
        }
        found.blobs.insert(
            s,
            Blob {
                addr: s,
                store: Store::Raw,
                size: e - s,
                data: data.to_vec(),
                kind: Kind::Tiles,
                first_frame: if first == u64::MAX { 0 } else { first },
                vram,
            },
        );
        found.tiles.insert(s, info);
    }
    // Each span placed in its sheet.
    let mut dsu_roots: HashMap<u32, u32> = HashMap::new();
    for &a in raw_size.keys() {
        dsu_roots.insert(a, dsu.root(a));
    }
    let place = |o: Origin| -> Option<Placed> {
        match o {
            Origin::Packed(b, off) => found
                .blobs
                .get(&b)
                .filter(|bl| bl.kind == Kind::Tiles)
                .map(|_| (b, off)),
            Origin::Raw(a) => {
                let s = raw_sheet[&dsu_roots[&a]];
                found
                    .blobs
                    .get(&s)
                    .filter(|bl| bl.kind == Kind::Tiles)
                    .map(|_| (s, a - s))
            }
        }
    };
    // Per sheet: the sizes shown and for how long.
    let mut sizes_shown: BTreeMap<u32, BTreeMap<Shape, u64>> = BTreeMap::new();
    for route in &all {
        for (list, _) in route {
            for s in list {
                if let Some((sheet, _)) = s.obj.origin.and_then(place) {
                    *sizes_shown
                        .entry(sheet)
                        .or_default()
                        .entry((s.obj.w, s.obj.h, s.obj.bpp, s.obj.one_d))
                        .or_default() += s.end - s.start;
                }
            }
        }
    }
    let mut out = Sprites::default();
    for (&addr, by) in &sizes_shown {
        let blob = &found.blobs[&addr];
        let info = found.tiles.get(&addr);
        let why = if by.len() > 1 {
            Some(format!(
                "shown at {} sizes ({})",
                by.len(),
                by.keys()
                    .map(|k| format!("{}x{}", k.0, k.1))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        } else if info.is_some_and(|t| t.bg) {
            Some("also a BG's tiles".into())
        } else {
            None
        };
        let &(w, h, bpp, one_d) = by.keys().next().expect("non-empty");
        let fb = frame_bytes(w, h, bpp);
        let why = why
            .or_else(|| (!one_d).then(|| "2D mapping: its frames aren't runs of the blob".into()));
        let why = why.or_else(|| {
            (!(blob.data.len() as u32).is_multiple_of(fb)).then(|| {
                format!(
                    "{} bytes aren't whole {w}x{h} frames of {fb}",
                    blob.data.len()
                )
            })
        });
        if let Some(w) = why {
            out.skipped.insert(addr, w);
            continue;
        }
        out.sheets.insert(
            addr,
            Sheet {
                addr,
                w,
                h,
                bpp,
                one_d,
                across: false,
                frames: blob.data.len() as u32 / fb,
                shown: BTreeMap::new(),
                bank: 0,
                palette: info.and_then(|t| t.palette()),
                anims: Vec::new(),
                composed: None,
            },
        );
    }
    // Tracks: one entry's spans on one sheet in a row, each span with its
    // frame.
    let mut banks: BTreeMap<u32, BTreeMap<u8, u64>> = BTreeMap::new();
    let mut tracks: Vec<Track> = Vec::new();
    for (ri, route) in all.iter().enumerate() {
        for (list, cut) in route {
            let mut track: Vec<(&Span, u32)> = Vec::new();
            let mut on: Option<u32> = None;
            for (k, s) in list.iter().enumerate() {
                let at = s.obj.origin.and_then(place).and_then(|(sheet, off)| {
                    let sh = out.sheets.get(&sheet)?;
                    Some((sheet, off, frame_bytes(sh.w, sh.h, sh.bpp)))
                });
                let joins = track.last().is_some_and(|&(p, _)| p.end == s.start);
                let same = at.is_some_and(|x| Some(x.0) == on);
                if let Some(sheet) = on
                    && !(same && joins)
                {
                    tracks.push((sheet, std::mem::take(&mut track), false, ri));
                    on = None;
                }
                let Some((sheet, off, fb)) = at else { continue };
                if !off.is_multiple_of(fb) {
                    out.sheets.remove(&sheet);
                    out.skipped
                        .insert(sheet, format!("shown from byte {off}, not a frame's start"));
                    track.clear();
                    on = None;
                    continue;
                }
                let sh = out.sheets.get_mut(&sheet).expect("kept");
                *sh.shown.entry(off / fb).or_default() += s.end - s.start;
                *banks
                    .entry(sheet)
                    .or_default()
                    .entry(s.obj.bank)
                    .or_default() += s.end - s.start;
                // The slot refilled from ROM: the frame changed, the tile
                // didn't. (A frame refilled from a decompressed buffer, a
                // Pokémon's pic, comes from a sheet gbagfx lays down.)
                if let Some(&(p, pf)) = track.last()
                    && pf != off / fb
                    && p.obj.tile == s.obj.tile
                    && matches!(s.obj.origin, Some(Origin::Raw(_)))
                {
                    sh.across = true;
                }
                on = Some(sheet);
                track.push((s, off / fb));
                if k + 1 == list.len() {
                    tracks.push((sheet, std::mem::take(&mut track), *cut, ri));
                    on = None;
                }
            }
            if let Some(sheet) = on {
                tracks.push((sheet, track, false, ri));
            }
        }
    }
    for (sheet, track, cut, ri) in tracks {
        let Some(sh) = out.sheets.get_mut(&sheet) else {
            continue;
        };
        let first = track[0].0;
        if let Some(a) = animation(&track, cut, (ri, first.index, first.start)) {
            sh.anims.push(a);
        }
    }
    for sh in out.sheets.values_mut() {
        sh.anims = distinct(std::mem::take(&mut sh.anims));
    }
    for (a, by) in banks {
        if let Some(sh) = out.sheets.get_mut(&a) {
            sh.bank = by
                .into_iter()
                .max_by_key(|&(b, n)| (n, b))
                .map_or(0, |x| x.0);
        }
    }
    // A sprite of several entries: two entries on one sheet over the same
    // frames, the same distance apart throughout.
    let mut composed = BTreeSet::new();
    for route in &all {
        let mut on: BTreeMap<u32, Vec<&Span>> = BTreeMap::new();
        for (list, _) in route {
            for s in list {
                if let Some((sheet, _)) = s.obj.origin.and_then(place)
                    && out.sheets.contains_key(&sheet)
                {
                    on.entry(sheet).or_default().push(s);
                }
            }
        }
        for (sheet, ss) in on {
            // Each pair's offsets apart and the frames they showed together.
            let mut rel: BTreeMap<(u8, u8), Apart> = BTreeMap::new();
            for a in &ss {
                for b in &ss {
                    if a.index < b.index && a.start < b.end && b.start < a.end {
                        let r = rel.entry((a.index, b.index)).or_default();
                        r.0.insert((b.obj.x - a.obj.x, b.obj.y - a.obj.y));
                        r.1 += a.end.min(b.end) - a.start.max(b.start);
                    }
                }
            }
            // Half a second together, never apart: one sprite, not two
            // that crossed.
            if rel.values().any(|(d, n)| d.len() == 1 && *n >= 30) {
                composed.insert(sheet);
            }
        }
    }
    for sheet in composed {
        let sh = out.sheets.get_mut(&sheet).expect("kept");
        let c = compose(&all, sh, |o| o.and_then(place));
        // The entries' own sequences follow OAM slots, which the game
        // reassigns under a sprite of several entries: its arrangements
        // are its animation.
        sh.anims.clear();
        if let Some(across) = c.grid {
            sh.across = across;
        }
        sh.composed = Some(c);
    }
    // A sprite of several entries isn't one entry's frames across or
    // down: its sheet keeps the plain layout unless one arrangement lays
    // out the whole sheet.
    found.layouts = out
        .sheets
        .values()
        .filter(|s| s.composed.as_ref().is_none_or(|c| c.grid.is_some()))
        .map(|s| {
            (
                s.addr,
                Layout {
                    w: s.w,
                    h: s.h,
                    across: s.across,
                },
            )
        })
        .collect();
    out
}

/// A composed sheet's arrangements: at each moment, every entry showing
/// one of its frames, placed from the top-left one; the same arrangement
/// kept on is one run, and each distinct one is a frame of the canvas.
fn compose(
    all: &[Vec<(Vec<Span>, bool)>],
    sh: &Sheet,
    place: impl Fn(Option<Origin>) -> Option<Placed>,
) -> Composed {
    let fb = frame_bytes(sh.w, sh.h, sh.bpp);
    let (fw, fh) = (sh.w as i32 * 8, sh.h as i32 * 8);
    let mut c = Composed::default();
    for (ri, route) in all.iter().enumerate() {
        let on: Vec<(&Span, u32)> = route
            .iter()
            .flat_map(|(list, _)| list)
            .filter_map(|s| {
                let (sheet, off) = place(s.obj.origin)?;
                (sheet == sh.addr && off.is_multiple_of(fb)).then_some((s, off / fb))
            })
            .collect();
        let mut times: Vec<u64> = on.iter().flat_map(|(s, _)| [s.start, s.end]).collect();
        times.sort_unstable();
        times.dedup();
        let mut runs: Vec<(Vec<Part>, u64, u64)> = Vec::new();
        for w in times.windows(2) {
            let (t0, t1) = (w[0], w[1]);
            let mut parts: Vec<Part> = on
                .iter()
                .filter(|(s, _)| s.start <= t0 && t0 < s.end)
                .map(|&(s, frame)| Part {
                    x: s.obj.x,
                    y: s.obj.y,
                    frame,
                    hflip: s.obj.hflip,
                    vflip: s.obj.vflip,
                })
                .collect();
            let (Some(x0), Some(y0)) = (
                parts.iter().map(|p| p.x).min(),
                parts.iter().map(|p| p.y).min(),
            ) else {
                continue;
            };
            for p in &mut parts {
                p.x -= x0;
                p.y -= y0;
            }
            parts.sort();
            match runs.last_mut() {
                Some(r) if r.0 == parts && r.2 + r.1 == t0 => r.1 += t1 - t0,
                _ => runs.push((parts, t1 - t0, t0)),
            }
        }
        for (parts, hold, start) in runs {
            if !c.arrangements.iter().any(|a| a.0 == parts) {
                c.arrangements.push((parts, hold, (ri, start)));
            }
        }
    }
    for (parts, _, _) in &c.arrangements {
        for p in parts {
            c.w = c.w.max((p.x + fw) as u16);
            c.h = c.h.max((p.y + fh) as u16);
        }
    }
    let n = sh.frames as usize;
    c.grid = c.arrangements.iter().find_map(|(parts, _, _)| {
        let whole = n > 1
            && parts.len() == n
            && parts.iter().map(|p| p.frame).collect::<BTreeSet<_>>().len() == n
            && parts.iter().all(|p| !p.hflip && !p.vflip);
        let k = |p: &Part| p.frame as i32;
        if whole && parts.iter().all(|p| p.x == k(p) * fw && p.y == 0) {
            Some(true)
        } else if whole && parts.iter().all(|p| p.y == k(p) * fh && p.x == 0) {
            Some(false)
        } else {
            None
        }
    });
    c
}

/// One track's animation: its frames' sequence folded to its loop if it
/// repeats. `None` for a track that shows one frame.
fn animation(track: &[(&Span, u32)], cut: bool, seen: (usize, u8, u64)) -> Option<Anim> {
    // Runs of one frame (an entry moving shows the same frame on).
    let mut runs: Vec<((u32, bool, bool), u64)> = Vec::new();
    for (s, f) in track {
        let key = (*f, s.obj.hflip, s.obj.vflip);
        match runs.last_mut() {
            Some(r) if r.0 == key => r.1 += s.end - s.start,
            _ => runs.push((key, s.end - s.start)),
        }
    }
    if runs.len() < 2 {
        return None;
    }
    let keys: Vec<_> = runs.iter().map(|r| r.0).collect();
    let n = keys.len();
    // The shortest period seen twice in a row that the whole run follows:
    // each step one or two along the loop (a step dropped on a lag frame).
    let period = (1..=n / 2).find(|&p| {
        (0..=n - 2 * p)
            .find(|&i| (0..p).all(|k| keys[i + k] == keys[i + k + p]))
            .is_some_and(|i| follows(&keys[i..i + p], &keys) * 5 >= (n - 1) * 4)
    });
    let Some(p) = period else {
        return Some(Anim {
            steps: runs,
            looped: false,
            partial: cut,
            seen,
        });
    };
    // The loop's steps from the first window the next period repeats, each
    // held its most common hold (the first and last runs may be cut).
    let at = (0..n - 2 * p + 1)
        .find(|&i| (0..p).all(|k| keys[i + k] == keys[i + k + p]))
        .unwrap_or(0);
    let steps = (0..p)
        .map(|j| {
            let key = keys[at + j];
            let mut c: BTreeMap<u64, usize> = BTreeMap::new();
            for (i, r) in runs.iter().enumerate() {
                if r.0 == key && i != 0 && i != n - 1 {
                    *c.entry(r.1).or_default() += 1;
                }
            }
            let hold = c
                .into_iter()
                .max_by_key(|&(h, k)| (k, std::cmp::Reverse(h)))
                .map_or(runs[at + j].1, |x| x.0);
            (key, hold)
        })
        .collect::<Vec<_>>();
    Some(Anim {
        steps: rotate_min(steps),
        looped: true,
        // Seen whole twice: two full periods between the first and last run.
        partial: n < 2 * p + 2,
        seen,
    })
}

/// A loop rotated to start at its least step, so one loop seen from
/// different starts compares equal.
fn rotate_min<T: Ord + Clone>(v: Vec<T>) -> Vec<T> {
    let best = (0..v.len())
        .min_by(|&a, &b| {
            let ra = v[a..].iter().chain(&v[..a]);
            let rb = v[b..].iter().chain(&v[..b]);
            ra.cmp(rb)
        })
        .unwrap_or(0);
    let mut out = v[best..].to_vec();
    out.extend_from_slice(&v[..best]);
    out
}

/// How many of `seq`'s steps follow loop `lp`: the next frame one or two
/// places on from where the last could be.
fn follows<K: PartialEq>(lp: &[K], seq: &[K]) -> usize {
    let p = lp.len();
    let at = |k: &K| (0..p).filter(|&i| lp[i] == *k).collect::<Vec<_>>();
    let mut now = at(&seq[0]);
    let mut ok = 0;
    for k in &seq[1..] {
        let next: Vec<usize> = at(k)
            .into_iter()
            .filter(|&j| now.iter().any(|&i| j == (i + 1) % p || j == (i + 2) % p))
            .collect();
        if next.is_empty() {
            now = at(k);
        } else {
            ok += 1;
            now = next;
        }
    }
    ok
}

/// `small`'s steps appear in a row in `big`'s (going round, for a loop),
/// or follow `big`'s loop over most of them.
fn covered(small: &Anim, big: &Anim) -> bool {
    let ks: Vec<_> = small.steps.iter().map(|s| s.0).collect();
    if big.looped && ks.len() > 2 {
        let lp: Vec<_> = big.steps.iter().map(|s| s.0).collect();
        if follows(&lp, &ks) * 5 >= (ks.len() - 1) * 4 {
            return true;
        }
    }
    let mut kb: Vec<_> = big.steps.iter().map(|s| s.0).collect();
    if big.looped {
        let once = kb.clone();
        while kb.len() < ks.len() + once.len() {
            kb.extend_from_slice(&once);
        }
    }
    ks.len() <= kb.len() && kb.windows(ks.len()).any(|w| w == ks)
}

/// The distinct animations: loops once each, and a sequence played once
/// only if no loop or longer sequence holds it (the start or end of a
/// loop seen cut off).
fn distinct(all: Vec<Anim>) -> Vec<Anim> {
    // Longest first: a loop seen with a step dropped follows the whole one.
    let mut loops: Vec<Anim> = Vec::new();
    let mut by_len: Vec<&Anim> = all.iter().filter(|a| a.looped).collect();
    by_len.sort_by_key(|a| std::cmp::Reverse(a.steps.len()));
    for a in by_len {
        let keys = |x: &Anim| x.steps.iter().map(|s| s.0).collect::<Vec<_>>();
        match loops
            .iter_mut()
            .find(|b| keys(b) == keys(a) || covered(a, b))
        {
            Some(b) => b.partial &= a.partial,
            None => loops.push(a.clone()),
        }
    }
    loops.sort_by_key(|a| a.seen);
    let mut onces: Vec<Anim> = all.into_iter().filter(|a| !a.looped).collect();
    onces.sort_by_key(|a| std::cmp::Reverse(a.steps.len()));
    let mut kept: Vec<Anim> = Vec::new();
    for a in onces {
        if !loops.iter().chain(&kept).any(|b| covered(&a, b)) {
            kept.push(a);
        }
    }
    kept.sort_by_key(|a| a.seen);
    loops.extend(kept);
    loops
}

/// Milliseconds a video frame: the GBA's 59.7275 Hz.
const FRAME_MS: f64 = 1000.0 / 59.7275;

fn ms(frames: u64) -> u16 {
    ((frames as f64 * FRAME_MS).round() as u64).clamp(1, u16::MAX as u64) as u16
}

/// One frame's pixel indices, `w×h` tiles, 1D, flipped as asked.
pub fn frame_pixels(data: &[u8], sh: &Sheet, f: u32, hflip: bool, vflip: bool) -> Vec<u8> {
    let fb = frame_bytes(sh.w, sh.h, sh.bpp) as usize;
    let bytes = &data[f as usize * fb..(f as usize + 1) * fb];
    let (pw, ph, px) = graphics::tile_pixels(bytes, sh.bpp, sh.w as usize);
    let mut out = vec![0u8; pw * ph];
    for y in 0..ph {
        for x in 0..pw {
            let (sx, sy) = (
                if hflip { pw - 1 - x } else { x },
                if vflip { ph - 1 - y } else { y },
            );
            out[y * pw + x] = px[sy * pw + sx];
        }
    }
    out
}

/// A sheet's frames as one image, laid out as [`Layout`] says: (width,
/// height, pixels).
pub fn sheet_pixels(data: &[u8], bpp: u8, l: Layout) -> (usize, usize, Vec<u8>) {
    let fb = frame_bytes(l.w, l.h, bpp) as usize;
    let n = data.len() / fb;
    if !l.across {
        return graphics::tile_pixels(data, bpp, l.w as usize);
    }
    let (fw, fh) = (l.w as usize * 8, l.h as usize * 8);
    let (pw, ph) = (fw * n.max(1), fh);
    let mut px = vec![0u8; pw * ph];
    for f in 0..n {
        let (w, _, fpx) = graphics::tile_pixels(&data[f * fb..(f + 1) * fb], bpp, l.w as usize);
        for y in 0..fh {
            px[y * pw + f * fw..y * pw + f * fw + fw].copy_from_slice(&fpx[y * w..y * w + fw]);
        }
    }
    (pw, ph, px)
}

/// The frames an Aseprite file gets: the sheet's, then each animation's
/// steps (frame, flips, hold in video frames).
fn file_frames(sh: &Sheet) -> Vec<((u32, bool, bool), Option<u64>)> {
    let mut v: Vec<_> = (0..sh.frames).map(|f| ((f, false, false), None)).collect();
    for a in &sh.anims {
        v.extend(a.steps.iter().map(|&(k, n)| (k, Some(n))));
    }
    v
}

/// The sprite's user data text.
fn user_text(sh: &Sheet) -> String {
    format!(
        "rom={:#010x} palette={} bank={} size={}x{} mapping={} bpp={}{}",
        sh.addr,
        sh.palette
            .map_or("none".into(), |p| format!("{:#010x}+{}", p.0, p.1)),
        sh.bank,
        sh.w,
        sh.h,
        if sh.one_d { "1d" } else { "2d" },
        sh.bpp,
        sh.composed
            .as_ref()
            .map_or(String::new(), |c| format!(" composed={}x{}", c.w, c.h))
    )
}

fn anim_name(i: usize) -> String {
    format!("anim{}", i + 1)
}

/// The tags an Aseprite file gets: `sheet`, each animation's, and
/// `composed` over a composed sprite's arrangements.
fn tag_names(sh: &Sheet) -> Vec<String> {
    std::iter::once("sheet".to_string())
        .chain((0..sh.anims.len()).map(anim_name))
        .chain(
            sh.composed
                .as_ref()
                .filter(|c| !c.arrangements.is_empty())
                .map(|_| "composed".to_string()),
        )
        .collect()
}

/// The canvas in pixels: a frame, or a composed sprite's.
fn canvas(sh: &Sheet) -> (u16, u16) {
    sh.composed
        .as_ref()
        .map_or((sh.w as u16 * 8, sh.h as u16 * 8), |c| (c.w, c.h))
}

/// A part's cel user data: the sheet frame it shows, flipped or not, and
/// its place, which an override reads its edits back by.
fn part_text(p: &Part, hold: u64) -> String {
    format!(
        "frame={}{}{} at={},{} video_frames={hold}",
        p.frame,
        if p.hflip { " hflip" } else { "" },
        if p.vflip { " vflip" } else { "" },
        p.x,
        p.y
    )
}

/// Reads [`part_text`] back: (frame, hflip, vflip, x, y).
pub fn parse_part(text: &str) -> Option<(u32, bool, bool, i32, i32)> {
    let mut frame = None;
    let mut at = None;
    let (mut h, mut v) = (false, false);
    for w in text.split_whitespace() {
        if let Some(f) = w.strip_prefix("frame=") {
            frame = f.parse().ok();
        } else if let Some(a) = w.strip_prefix("at=") {
            let (x, y) = a.split_once(',')?;
            at = Some((x.parse().ok()?, y.parse().ok()?));
        }
        h |= w == "hflip";
        v |= w == "vflip";
    }
    let (x, y) = at?;
    Some((frame?, h, v, x, y))
}

/// Each frame of the Aseprite file as its canvas shows it (index 0
/// clear), with its duration in ms: the sheet's frames and the
/// animations' at the top left, then each arrangement's parts in place.
fn canvas_frames(data: &[u8], sh: &Sheet) -> Vec<(Vec<u8>, u16)> {
    let (cw, ch) = canvas(sh);
    let (cw, ch) = (cw as usize, ch as usize);
    let pw = sh.w as usize * 8;
    let draw = |px: &mut [u8], p: &Part| {
        let fp = frame_pixels(data, sh, p.frame, p.hflip, p.vflip);
        for (k, &v) in fp.iter().enumerate() {
            let (x, y) = (p.x as usize + k % pw, p.y as usize + k / pw);
            if v != 0 && x < cw && y < ch {
                px[y * cw + x] = v;
            }
        }
    };
    let mut out = Vec::new();
    for ((frame, hflip, vflip), hold) in file_frames(sh) {
        let mut px = vec![0; cw * ch];
        let p = Part {
            x: 0,
            y: 0,
            frame,
            hflip,
            vflip,
        };
        draw(&mut px, &p);
        out.push((px, hold.map_or(100, ms)));
    }
    for (parts, hold, _) in sh.composed.iter().flat_map(|c| &c.arrangements) {
        let mut px = vec![0; cw * ch];
        for p in parts {
            draw(&mut px, p);
        }
        out.push((px, ms(*hold)));
    }
    out
}

/// The Aseprite file for one sheet: indexed with its palette (index 0
/// transparent), layer `sprite` with a cel a frame (animation frames
/// linked to the sheet's), a tag `sheet` over the sheet's frames and one
/// an animation, user data on the sprite with its ROM addresses and on
/// each animation cel with its exact video frames. A composed sprite's
/// canvas holds its every arrangement: the sheet's frames at its top
/// left, then a frame an arrangement (tag `composed`), each part a cel on
/// its own layer (`part 1`, …) with the frame it shows in user data.
pub fn aseprite(data: &[u8], sh: &Sheet, pal: &[u16]) -> Result<Vec<u8>, String> {
    use aseprite::{AsepriteFile, Color, ColorMode, LoopDirection, Pixels, UserData};
    let e = |x: aseprite::AsepriteError| x.to_string();
    let (pw, ph) = (sh.w as u16 * 8, sh.h as u16 * 8);
    let (cw, ch) = canvas(sh);
    let mut f = AsepriteFile::new(cw, ch, ColorMode::Indexed);
    let n = 1usize << sh.bpp;
    let colours: Vec<Color> = (0..n)
        .map(|i| {
            let [r, g, b] = graphics::rgb(pal.get(i).copied().unwrap_or(0));
            Color {
                r,
                g,
                b,
                a: 255,
                name: None,
            }
        })
        .collect();
    f.set_palette(&colours).map_err(e)?;
    f.set_transparent_index(0);
    let layer = f.add_layer("sprite");
    let arrangements: &[Arrangement] = sh.composed.as_ref().map_or(&[], |c| &c.arrangements[..]);
    let most = arrangements.iter().map(|a| a.0.len()).max().unwrap_or(0);
    let parts: Vec<_> = (0..most)
        .map(|j| f.add_layer(&format!("part {}", j + 1)))
        .collect();
    let frames = file_frames(sh);
    for (i, &((fr, hf, vf), hold)) in frames.iter().enumerate() {
        let k = f.add_frame(hold.map_or(100, ms));
        debug_assert_eq!(k, i);
        if i >= sh.frames as usize && !hf && !vf {
            f.set_linked_cel(layer, k, fr as usize).map_err(e)?;
        } else {
            let px = frame_pixels(data, sh, fr, hf, vf);
            let p = Pixels::new(px, pw, ph, ColorMode::Indexed).map_err(e)?;
            f.set_cel(layer, k, p, 0, 0).map_err(e)?;
        }
        if let Some(h) = hold {
            let text = format!(
                "frame={fr}{}{} video_frames={h}",
                if hf { " hflip" } else { "" },
                if vf { " vflip" } else { "" }
            );
            f.set_cel_user_data(
                layer,
                k,
                UserData {
                    text: Some(text),
                    ..Default::default()
                },
            );
        }
    }
    for (ps, hold, _) in arrangements {
        let k = f.add_frame(ms(*hold));
        for (p, &l) in ps.iter().zip(&parts) {
            let px = frame_pixels(data, sh, p.frame, p.hflip, p.vflip);
            let cel = Pixels::new(px, pw, ph, ColorMode::Indexed).map_err(e)?;
            f.set_cel(l, k, cel, p.x as i16, p.y as i16).map_err(e)?;
            f.set_cel_user_data(
                l,
                k,
                UserData {
                    text: Some(part_text(p, *hold)),
                    ..Default::default()
                },
            );
        }
    }
    f.add_tag("sheet", 0..=sh.frames as usize - 1, LoopDirection::Forward)
        .map_err(e)?;
    let mut at = sh.frames as usize;
    for (i, a) in sh.anims.iter().enumerate() {
        let t = f
            .add_tag(
                &anim_name(i),
                at..=at + a.steps.len() - 1,
                LoopDirection::Forward,
            )
            .map_err(e)?;
        f.set_tag_user_data(
            t,
            UserData {
                text: Some(format!(
                    "{}{}; seen on route {} entry {} from frame {}",
                    if a.looped { "loop" } else { "once" },
                    if a.partial { ", partial" } else { "" },
                    a.seen.0,
                    a.seen.1,
                    a.seen.2
                )),
                ..Default::default()
            },
        );
        at += a.steps.len();
    }
    if let Some((_, _, (route, frame))) = arrangements.first() {
        let t = f
            .add_tag(
                "composed",
                at..=at + arrangements.len() - 1,
                LoopDirection::Forward,
            )
            .map_err(e)?;
        f.set_tag_user_data(
            t,
            UserData {
                text: Some(format!(
                    "{} arrangements of up to {most} entries; first seen on route {route} from frame {frame}",
                    arrangements.len()
                )),
                ..Default::default()
            },
        );
    }
    f.set_sprite_user_data(UserData {
        text: Some(user_text(sh)),
        ..Default::default()
    });
    let mut out = Vec::new();
    f.write_to(&mut out).map_err(e)?;
    Ok(out)
}

/// Reads a written file back through aseprite-io and asefile and checks
/// both give what was meant: size, frames and durations, tags, user data,
/// and each frame's pixels as the canvas shows them (asefile's as RGBA,
/// index 0 clear).
pub fn round_trip(bytes: &[u8], data: &[u8], sh: &Sheet, pal: &[u16]) -> Result<(), String> {
    let want = canvas_frames(data, sh);
    let want_tags = tag_names(sh);
    let (pw, ph) = canvas(sh);
    let (pw, ph) = (pw as u32, ph as u32);
    // aseprite-io.
    let a = aseprite::AsepriteFile::from_reader(bytes).map_err(|e| format!("aseprite-io: {e}"))?;
    if (a.width() as u32, a.height() as u32) != (pw, ph) || a.frames().len() != want.len() {
        return Err(format!(
            "aseprite-io: {}x{}, {} frames, against {pw}x{ph}, {}",
            a.width(),
            a.height(),
            a.frames().len(),
            want.len()
        ));
    }
    let tags: Vec<String> = a.tags().iter().map(|t| t.name.clone()).collect();
    if tags != want_tags {
        return Err(format!("aseprite-io: tags {tags:?}, against {want_tags:?}"));
    }
    if a.sprite_user_data().as_ref().and_then(|u| u.text.clone()) != Some(user_text(sh)) {
        return Err("aseprite-io: the sprite's user data differs".into());
    }
    for (i, (px, dur)) in want.iter().enumerate() {
        if a.frames()[i].duration_ms != *dur {
            return Err(format!("aseprite-io: frame {i}'s duration differs"));
        }
        let got = crate::overrides::ase_frame(&a, i).map_err(|e| format!("aseprite-io: {e}"))?;
        if &got != px {
            return Err(format!("aseprite-io: frame {i}'s pixels differ"));
        }
    }
    // asefile.
    let b = asefile::AsepriteFile::read(bytes).map_err(|e| format!("asefile: {e}"))?;
    if (b.width() as u32, b.height() as u32) != (pw, ph) || b.num_frames() as usize != want.len() {
        return Err("asefile: size or frame count differs".into());
    }
    let tags: Vec<String> = (0..b.num_tags())
        .map(|i| b.tag(i).name().to_string())
        .collect();
    if tags != want_tags {
        return Err(format!("asefile: tags {tags:?}, against {want_tags:?}"));
    }
    for (i, (px, dur)) in want.iter().enumerate() {
        let fr = b.frame(i as u32);
        if fr.duration() != *dur as u32 {
            return Err(format!("asefile: frame {i}'s duration differs"));
        }
        let img = fr.image();
        for (k, &ix) in px.iter().enumerate() {
            let p = img.get_pixel(k as u32 % pw, k as u32 / pw).0;
            let ok = if ix == 0 {
                p[3] == 0
            } else {
                let [r, g, bl] = graphics::rgb(pal.get(ix as usize).copied().unwrap_or(0));
                p == [r, g, bl, 255]
            };
            if !ok {
                return Err(format!("asefile: frame {i}'s pixel {k} differs"));
            }
        }
    }
    Ok(())
}

/// What [`write`] wrote.
pub struct Written {
    pub manifest: String,
    pub report: String,
    pub sheets: usize,
    pub anims: usize,
}

/// Writes `sprites/<name>.aseprite` a sheet, each round-tripped, and
/// returns the manifest's `[sprites]` and the report.
pub fn write(
    found: &Found,
    sprites: &Sprites,
    names: &BTreeMap<u32, String>,
    dir: &Path,
) -> Result<Written, String> {
    std::fs::create_dir_all(dir.join("sprites")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut manifest = String::new();
    let mut report = String::new();
    let mut anims = 0;
    let mut composed = 0;
    for sh in sprites.sheets.values() {
        let blob = &found.blobs[&sh.addr];
        let name = graphics::blob_name(names, blob);
        let pal = sh.palette.map_or_else(
            || graphics::grey(sh.bpp),
            |p| graphics::pairing_colours(found, p, 1 << sh.bpp),
        );
        let bytes = aseprite(&blob.data, sh, &pal)?;
        round_trip(&bytes, &blob.data, sh, &pal).map_err(|e| format!("{name}.aseprite: {e}"))?;
        let file = format!("sprites/{name}.aseprite");
        std::fs::write(dir.join(&file), &bytes).map_err(|e| format!("{file}: {e}"))?;
        anims += sh.anims.len();
        composed += sh.composed.is_some() as usize;
        let layout = match &sh.composed {
            Some(c) if c.grid.is_none() => "plain",
            _ if sh.across => "across",
            _ => "down",
        };
        let shown: Vec<String> = sh.shown.keys().map(|f| f.to_string()).collect();
        let _ = writeln!(
            manifest,
            "{:#010x} = {{ name = \"{name}\", size = \"{}x{}\", bpp = {}, mapping = \"{}\", frames = {}, shown = [{}], layout = \"{layout}\", bank = {}, palette = {}, animations = {}, composed = {}{}, files = [\"{file}\", \"tiles/{name}.png\"] }}",
            sh.addr,
            sh.w,
            sh.h,
            sh.bpp,
            if sh.one_d { "1d" } else { "2d" },
            sh.frames,
            shown.join(", "),
            sh.bank,
            sh.palette
                .map_or("\"grey\"".into(), |p| format!("\"{:#010x}+{}\"", p.0, p.1)),
            sh.anims.len(),
            sh.composed.is_some(),
            sh.composed.as_ref().map_or(String::new(), |c| format!(
                ", canvas = \"{}x{}\", arrangements = {}",
                c.w,
                c.h,
                c.arrangements.len()
            )),
        );
        let _ = writeln!(
            report,
            "  {:#010x} {name}: {}x{} tiles, {} frames ({} shown), laid {layout}{}",
            sh.addr,
            sh.w,
            sh.h,
            sh.frames,
            sh.shown.len(),
            sh.composed.as_ref().map_or(String::new(), |c| format!(
                ", composed: a {}x{} canvas, {} arrangements",
                c.w,
                c.h,
                c.arrangements.len()
            ))
        );
        for (i, (parts, hold, seen)) in sh.composed.iter().flat_map(|c| &c.arrangements).enumerate()
        {
            let ps: Vec<String> = parts
                .iter()
                .map(|p| {
                    format!(
                        "{}{}{}@{},{}",
                        p.frame,
                        if p.hflip { "h" } else { "" },
                        if p.vflip { "v" } else { "" },
                        p.x,
                        p.y
                    )
                })
                .collect();
            let _ = writeln!(
                report,
                "    arrangement {}: [{}] for {hold} (frame@x,y; route {} from frame {})",
                i + 1,
                ps.join(" "),
                seen.0,
                seen.1
            );
        }
        for (i, a) in sh.anims.iter().enumerate() {
            let steps: Vec<String> = a
                .steps
                .iter()
                .map(|&((f, h, v), n)| {
                    format!(
                        "{f}{}{}:{n}",
                        if h { "h" } else { "" },
                        if v { "v" } else { "" }
                    )
                })
                .collect();
            let _ = writeln!(
                report,
                "    {}: {}{} [{}] (frame:video frames; route {} entry {} from frame {})",
                anim_name(i),
                if a.looped { "loop" } else { "once" },
                if a.partial { ", partial" } else { "" },
                steps.join(" "),
                a.seen.0,
                a.seen.1,
                a.seen.2
            );
        }
    }
    let mut skipped = String::new();
    for (a, why) in &sprites.skipped {
        let n = found
            .blobs
            .get(a)
            .map_or_else(|| format!("{a:#010x}"), |b| graphics::blob_name(names, b));
        let _ = writeln!(skipped, "  {a:#010x} {n}: {why}");
    }
    let report = format!(
        "# gbaa-assets graphics: sprites from OAM\nsheets {}, animations {anims}, composed {composed}; OAM tile blobs not a sheet: {}\n\nsheets:\n{report}\nnot a sheet:\n{skipped}",
        sprites.sheets.len(),
        sprites.skipped.len()
    );
    Ok(Written {
        manifest: format!("[sprites]\n{manifest}"),
        report,
        sheets: sprites.sheets.len(),
        anims,
    })
}

/// One `[sprites]` entry the check reads.
#[derive(Debug, Clone)]
struct Entry {
    addr: u32,
    name: String,
    w: u8,
    h: u8,
    anims: usize,
    composed: bool,
}

fn manifest_sprites(text: &str) -> Result<Vec<Entry>, String> {
    let doc: toml::Table = text.parse().map_err(|e| format!("manifest.toml: {e}"))?;
    let Some(t) = doc.get("sprites").and_then(|v| v.as_table()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (k, v) in t {
        let bad = || format!("[sprites] {k}: bad entry");
        let addr = hex(k).ok_or_else(bad)?;
        let s = |f: &str| v.get(f).and_then(|x| x.as_str()).ok_or_else(bad);
        let (w, h) = s("size")?.split_once('x').ok_or_else(bad)?;
        out.push(Entry {
            addr,
            name: s("name")?.into(),
            w: w.parse().map_err(|_| bad())?,
            h: h.parse().map_err(|_| bad())?,
            anims: v
                .get("animations")
                .and_then(|x| x.as_integer())
                .unwrap_or(0) as usize,
            composed: v.get("composed").and_then(|x| x.as_bool()).unwrap_or(false),
        });
    }
    Ok(out)
}

/// What [`check`] found.
pub struct Report {
    pub text: String,
    /// Object-event and Pokémon strips: matched at the image level, of.
    pub strips: (usize, usize),
}

/// pret's kind of sheet, by where its PNG lives.
fn strip_kind(source: &str) -> Option<&'static str> {
    if source.starts_with("graphics/object_events/") {
        Some("object event")
    } else if source.starts_with("graphics/pokemon/") {
        Some("pokémon")
    } else {
        None
    }
}

/// `check-pret sprites`: each sprite joined to pret's PNG by its symbol;
/// its tile sheet's size and pixels against pret's, and each frame of its
/// Aseprite file (the `sheet` tag) against pret's PNG cut into frames of
/// the sprite's size in reading order, the way gbagfx's `-mwidth` and
/// `-mheight` read it. Object-event and Pokémon strips count apart; the
/// animations found are listed from `sprites.txt`.
pub fn check(
    out: &Path,
    pret: &Path,
    names: &BTreeMap<u32, (String, Option<String>)>,
) -> Result<Report, String> {
    let text = std::fs::read_to_string(out.join("manifest.toml"))
        .map_err(|e| format!("{}/manifest.toml: {e}", out.display()))?;
    let entries = manifest_sprites(&text)?;
    let incs = crate::pretgfx::scan_pret(pret)?;
    let mut lines = String::new();
    let (mut strips, mut others) = ((0, 0), (0, 0));
    for e in &entries {
        let inc = names.get(&e.addr).and_then(|(sym, file)| {
            let cands = incs.get(sym)?;
            cands
                .iter()
                .find(|c| {
                    file.as_deref()
                        .is_some_and(|f| Path::new(&c.file).file_stem() == Path::new(f).file_stem())
                })
                .or(cands.first())
                .cloned()
        });
        let Some(inc) = inc else {
            let _ = writeln!(lines, "  {:#010x} {}: no pret file", e.addr, e.name);
            continue;
        };
        let kind = strip_kind(&inc.source);
        let mut why = Vec::new();
        let theirs = std::fs::read(pret.join(&inc.source))
            .map_err(|x| x.to_string())
            .and_then(|b| graphics::read_indexed_png(&b));
        let sheet = std::fs::read(out.join(format!("tiles/{}.png", e.name)))
            .map_err(|x| x.to_string())
            .and_then(|b| graphics::read_indexed_png(&b));
        let ase = std::fs::read(out.join(format!("sprites/{}.aseprite", e.name)))
            .map_err(|x| x.to_string())
            .and_then(|b| aseprite::AsepriteFile::from_reader(&b[..]).map_err(|x| x.to_string()));
        let mut ok = false;
        match (&theirs, &sheet, &ase) {
            (Ok(t), Ok(m), Ok(a)) => {
                let sheet_ok = (m.0, m.1) == (t.0, t.1) && m.2 == t.2;
                if !sheet_ok {
                    why.push(if (m.0, m.1) != (t.0, t.1) {
                        format!("sheet: {}x{} against pret's {}x{}", m.0, m.1, t.0, t.1)
                    } else {
                        "sheet: pixels differ".into()
                    });
                }
                // pret's PNG in frames of the sprite's size, reading order.
                let (fw, fh) = (e.w as usize * 8, e.h as usize * 8);
                let across = t.0 / fw.max(1);
                let n = across * (t.1 / fh.max(1));
                let sheet_tag = a.tags().iter().find(|g| g.name == "sheet");
                let ours_n = sheet_tag.map_or(0, |g| g.to_frame + 1 - g.from_frame);
                let mut frames_ok = ours_n == n && t.0 % fw == 0 && t.1 % fh == 0;
                if !frames_ok {
                    why.push(if t.0 % fw != 0 || t.1 % fh != 0 {
                        format!("pret's {}x{} isn't whole {fw}x{fh} frames", t.0, t.1)
                    } else {
                        format!("frames: {ours_n} against pret's {n}")
                    });
                }
                let layer = a.layer_ref(0);
                for f in 0..ours_n.min(n) {
                    let want: Vec<u8> = (0..fh)
                        .flat_map(|y| {
                            let (ox, oy) = (f % across * fw, f / across * fh);
                            t.2[(oy + y) * t.0 + ox..(oy + y) * t.0 + ox + fw].to_vec()
                        })
                        .collect();
                    let got = layer
                        .and_then(|l| a.resolve_cel(l, f))
                        .and_then(|c| match &c.kind {
                            aseprite::CelKind::Raw { pixels, .. }
                            | aseprite::CelKind::Compressed { pixels, .. } => Some(&pixels.data),
                            _ => None,
                        });
                    if got != Some(&want) {
                        frames_ok = false;
                        why.push(format!("frame {f}: pixels differ"));
                        break;
                    }
                }
                // The palette over the colours a 4bpp sheet can use, 0 aside.
                let pal: Vec<u16> = a
                    .palette()
                    .iter()
                    .map(|c| (c.r as u16 >> 3) | (c.g as u16 >> 3) << 5 | (c.b as u16 >> 3) << 10)
                    .collect();
                let k = pal.len().min(t.3.len());
                if pal.get(1..k) != t.3.get(1..k) {
                    why.push("palette differs".into());
                }
                ok = sheet_ok && frames_ok;
            }
            (Err(x), _, _) => why.push(format!("pret's: {x}")),
            (_, Err(x), _) => why.push(format!("sheet: {x}")),
            (_, _, Err(x)) => why.push(format!("aseprite: {x}")),
        }
        let c = if kind.is_some() {
            &mut strips
        } else {
            &mut others
        };
        c.1 += 1;
        c.0 += ok as usize;
        let _ = writeln!(
            lines,
            "  {:#010x} {} ({}, {}): {}x{} frames, {} animations{}: image {}{}",
            e.addr,
            e.name,
            inc.source,
            kind.unwrap_or("other"),
            e.w,
            e.h,
            e.anims,
            if e.composed { ", composed" } else { "" },
            if ok { "ok" } else { "--" },
            if why.is_empty() {
                String::new()
            } else {
                format!(": {}", why.join("; "))
            }
        );
    }
    let anims = std::fs::read_to_string(out.join("sprites.txt")).unwrap_or_default();
    let found: String = anims
        .lines()
        .filter(|l| l.starts_with("  0x") || l.starts_with("    anim"))
        .map(|l| format!("{l}\n"))
        .collect();
    let n_anims = anims.lines().filter(|l| l.starts_with("    anim")).count();
    let text = format!(
        "# gbaa-assets check-pret sprites: each sprite sheet against pret's PNG\n\
         object-event and Pokémon strips at the image level: {} of {}; other sheets: {} of {}\n\
         animations found: {n_anims}\n\n\
         per sprite:\n{lines}\nanimations (sprites.txt):\n{found}",
        strips.0, strips.1, others.0, others.1
    );
    Ok(Report { text, strips })
}
