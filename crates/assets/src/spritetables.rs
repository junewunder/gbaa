//! The Sonic Advance sprite engine's tables (Kirby & the Amazing Mirror's
//! `gSpriteTables`, katam `include/sprite.h`), read from the ROM: each
//! animation's scripts, its frames as the OAM pieces the engine draws, and
//! the per-frame hitboxes the scripts set (`AnimCmd_6`). A frame is drawn
//! whole from its pieces, so frames built from several sprites of mixed
//! sizes come out as one picture, which the OAM recording can't give
//! ([`crate::sprites`]). docs/research/kirby-controller.md.
//!
//! The tables: `anims[animId]` is an array of variant pointers, each a
//! script of commands (negative ids) and `ShowFrame`s (delay, frame);
//! `attrs[animId]` the frames (12 bytes each: first OAM piece, piece count,
//! width, height, the pivot's offset); `oamData[animId]` the pieces (OAM
//! attributes 0 to 2, positions inside the frame); `tiles4bpp` and
//! `palette` what `GetTiles` and `GetPalette` copy from.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::graphics;

const ROM: u32 = 0x0800_0000;
const FRAME_MS: f64 = 1000.0 / 59.7275;
/// A `ShowFrame` of -1: nothing drawn.
pub const HIDDEN: u32 = u32::MAX;

fn word(rom: &[u8], a: u32) -> Result<u32, String> {
    let o = a.wrapping_sub(ROM) as usize;
    rom.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("{a:#010x}: outside the ROM"))
}

fn half(rom: &[u8], a: u32) -> Result<u16, String> {
    let o = a.wrapping_sub(ROM) as usize;
    rom.get(o..o + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("{a:#010x}: outside the ROM"))
}

fn in_rom(rom: &[u8], a: u32) -> bool {
    a >= ROM && ((a - ROM) as usize) < rom.len()
}

/// `struct SpriteTables`.
#[derive(Clone, Copy, Debug)]
pub struct Tables {
    pub at: u32,
    pub anims: u32,
    pub attrs: u32,
    pub oam: u32,
    pub palette: u32,
    pub tiles4: u32,
    pub tiles8: u32,
}

pub fn tables(rom: &[u8], at: u32) -> Result<Tables, String> {
    let f = |i: u32| word(rom, at + 4 * i);
    let t = Tables {
        at,
        anims: f(0)?,
        attrs: f(1)?,
        oam: f(2)?,
        palette: f(3)?,
        tiles4: f(4)?,
        tiles8: f(5)?,
    };
    for (name, a) in [("anims", t.anims), ("attrs", t.attrs), ("oamData", t.oam)] {
        if !in_rom(rom, a) {
            return Err(format!("{at:#010x}: {name} {a:#010x} isn't in the ROM"));
        }
    }
    Ok(t)
}

/// How many animations the table has: entries of `anims` that point into
/// the ROM.
pub fn count(rom: &[u8], t: &Tables) -> u32 {
    (0..0x4000)
        .take_while(|&i| word(rom, t.anims + 4 * i).is_ok_and(|a| in_rom(rom, a)))
        .count() as u32
}

/// A hitbox an `AnimCmd_6` sets: slot (the low 4 bits of its first word),
/// the word itself, and left, top, right, bottom from the sprite's pivot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hitbox {
    pub slot: u8,
    pub word: u32,
    pub rect: [i8; 4],
}

/// One `ShowFrame` and what the commands before it set.
#[derive(Clone, Debug)]
pub struct Step {
    pub frame: u32,
    pub delay: i32,
    /// The last `GetTiles` (first tile, count; negative means 8bpp).
    pub tiles: Option<(i32, u32)>,
    /// The last `GetPalette` (palette, colours, offset).
    pub palette: Option<(i32, u16, u16)>,
    /// The hitboxes set at this frame, slot by slot.
    pub boxes: Vec<Hitbox>,
    /// Other commands since the last frame (sound, move, the unknowns).
    pub events: Vec<String>,
}

/// How a variant's script ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// `AnimCmd_4`: the last frame stays.
    Stop,
    /// `JumpBack` to this step.
    Loop(usize),
    /// `SetIdAndVariant`: goes on to another animation.
    Goto(u16, u16),
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub addr: u32,
    pub steps: Vec<Step>,
    pub end: End,
}

/// One OAM piece of a frame, placed in the frame's box (frame flips
/// applied).
#[derive(Clone, Copy, Debug)]
pub struct Piece {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// In 32-byte units from the frame's first tile.
    pub tile: u32,
    pub bpp8: bool,
    pub hflip: bool,
    pub vflip: bool,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub w: u16,
    pub h: u16,
    /// The pivot (the sprite's x, y) inside the frame's box.
    pub pivot: (i16, i16),
    pub pieces: Vec<Piece>,
}

#[derive(Clone, Debug)]
pub struct Anim {
    pub id: u32,
    pub variants: Vec<Variant>,
    /// Frames by index; `None` for one no script shows, or one in the
    /// engine's other (streamed) frame format, which this doesn't read.
    pub frames: Vec<Option<Frame>>,
    pub streamed: bool,
}

/// OAM shape and size to pixels.
fn obj_size(shape: u16, size: u16) -> (u32, u32) {
    const S: [[(u32, u32); 4]; 3] = [
        [(8, 8), (16, 16), (32, 32), (64, 64)],
        [(16, 8), (32, 8), (32, 16), (64, 32)],
        [(8, 16), (8, 32), (16, 32), (32, 64)],
    ];
    S.get(shape as usize)
        .map_or((8, 8), |r| r[size as usize & 3])
}

fn variant(rom: &[u8], start: u32) -> Result<Variant, String> {
    let mut steps = Vec::new();
    let mut shown_at: Vec<u32> = Vec::new();
    let (mut tiles, mut palette) = (None, None);
    let mut boxes: BTreeMap<u8, Hitbox> = BTreeMap::new();
    let mut events = Vec::new();
    let mut pos = 0u32;
    for _ in 0..4096 {
        let a = start + 4 * pos;
        let w = |i: u32| word(rom, a + 4 * i);
        let cmd = w(0)? as i32;
        if cmd >= 0 {
            steps.push(Step {
                frame: w(1)?,
                delay: cmd,
                tiles,
                palette,
                boxes: boxes.values().copied().collect(),
                events: std::mem::take(&mut events),
            });
            shown_at.push(pos);
            pos += 2;
            continue;
        }
        let (len, end) = match cmd {
            -1 => {
                tiles = Some((w(1)? as i32, w(2)?));
                (3, None)
            }
            -2 => {
                let c = w(2)?;
                palette = Some((w(1)? as i32, c as u16, (c >> 16) as u16));
                (3, None)
            }
            -3 => {
                let to = pos.wrapping_sub(w(1)?);
                let k = shown_at.iter().position(|&p| p >= to).unwrap_or(0);
                (2, Some(End::Loop(k)))
            }
            -4 => (1, Some(End::Stop)),
            -5 => {
                events.push(format!("sound {:#x}", w(1)? & 0xFFFF));
                (2, None)
            }
            -6 => {
                let (b, r) = (w(1)?, w(2)?);
                let slot = (b & 0xF) as u8;
                if r == 0 {
                    boxes.remove(&slot);
                } else {
                    boxes.insert(
                        slot,
                        Hitbox {
                            slot,
                            word: b,
                            rect: r.to_le_bytes().map(|x| x as i8),
                        },
                    );
                }
                (3, None)
            }
            -7 => {
                let v = w(1)?;
                events.push(format!("move {},{}", v as i16, (v >> 16) as i16));
                (2, None)
            }
            -8 => {
                events.push(format!("cmd8 {:#x} {:#x}", w(1)?, w(2)?));
                (3, None)
            }
            -9 => {
                let v = w(1)?;
                (2, Some(End::Goto(v as u16, (v >> 16) as u16)))
            }
            -10 => {
                events.push(format!("cmd10 {:#x} {:#x} {:#x}", w(1)?, w(2)?, w(3)?));
                (4, None)
            }
            -11 => {
                events.push(format!("priority {}", w(1)?));
                (2, None)
            }
            -12 => {
                events.push(format!("cmd12 {:#x}", w(1)?));
                (2, None)
            }
            _ => return Err(format!("{a:#010x}: unknown animation command {cmd}")),
        };
        if let Some(end) = end {
            return Ok(Variant {
                addr: start,
                steps,
                end,
            });
        }
        pos += len;
    }
    Err(format!("{start:#010x}: no end in 4096 commands"))
}

/// Animation `id`: its variants (the pointers before the array's own
/// address, as the scripts sit just before it) and the frames they show.
pub fn anim(rom: &[u8], t: &Tables, id: u32) -> Result<Anim, String> {
    let base = word(rom, t.anims + 4 * id)?;
    let mut variants = Vec::new();
    for k in 0..256 {
        // The last animation's array runs into the anims table itself.
        if base < t.anims && base + 4 * k >= t.anims {
            break;
        }
        let v = word(rom, base + 4 * k)?;
        if !in_rom(rom, v) || v >= base {
            break;
        }
        variants.push(variant(rom, v).map_err(|e| format!("anim {id} variant {k}: {e}"))?);
    }
    let shown = variants
        .iter()
        .flat_map(|v| &v.steps)
        .map(|s| s.frame)
        .filter(|&f| f != HIDDEN);
    let streamed = shown.clone().any(|f| f >> 28 != 0);
    let n = shown.map(|f| (f & 0x0FFF_FFFF) + 1).max().unwrap_or(0);
    let mut frames = vec![None; n as usize];
    if !streamed {
        let (attrs, oam) = (word(rom, t.attrs + 4 * id)?, word(rom, t.oam + 4 * id)?);
        for s in variants
            .iter()
            .flat_map(|v| &v.steps)
            .filter(|s| s.frame != HIDDEN)
        {
            let i = s.frame as usize;
            if frames[i].is_none() {
                frames[i] = Some(frame(rom, attrs + 12 * s.frame, oam)?);
            }
        }
    }
    Ok(Anim {
        id,
        variants,
        frames,
        streamed,
    })
}

fn frame(rom: &[u8], e: u32, oam: u32) -> Result<Frame, String> {
    let h16 = |o: u32| half(rom, e + o);
    let (bits, n, w, h) = (h16(0)?, h16(2)?, h16(4)?, h16(6)?);
    let pivot = (h16(8)? as i16, h16(10)? as i16);
    let (fh, fv) = (bits & 0x4000 != 0, bits & 0x8000 != 0);
    let mut pieces = Vec::new();
    for i in 0..n as u32 {
        let p = oam + 6 * ((bits as u32 & 0x3FFF) + i);
        let (a0, a1, a2) = (half(rom, p)?, half(rom, p + 2)?, half(rom, p + 4)?);
        let (pw, ph) = obj_size(a0 >> 14, a1 >> 14);
        let bpp8 = a0 & 0x2000 != 0;
        let mut x = (a1 & 0x1FF) as i32;
        let mut y = (a0 & 0xFF) as i32;
        if x >= 256 {
            x -= 512;
        }
        if y >= 128 {
            y -= 256;
        }
        if fh {
            x = w as i32 - pw as i32 - x;
        }
        if fv {
            y = h as i32 - ph as i32 - y;
        }
        let tile = (a2 & 0x3FF) as u32;
        pieces.push(Piece {
            x,
            y,
            w: pw,
            h: ph,
            tile: if bpp8 { tile * 2 } else { tile },
            bpp8,
            hflip: (a1 & 0x1000 != 0) != fh,
            vflip: (a1 & 0x2000 != 0) != fv,
        });
    }
    Ok(Frame {
        w,
        h,
        pivot,
        pieces,
    })
}

/// The box around every frame of `a`, from the pivot: (left, top, right,
/// bottom), exclusive at the right and bottom.
pub fn bounds(a: &Anim) -> (i32, i32, i32, i32) {
    let mut b = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for f in a.frames.iter().flatten() {
        for p in &f.pieces {
            let (x, y) = (p.x - f.pivot.0 as i32, p.y - f.pivot.1 as i32);
            b = (
                b.0.min(x),
                b.1.min(y),
                b.2.max(x + p.w as i32),
                b.3.max(y + p.h as i32),
            );
        }
    }
    if b.0 > b.2 { (0, 0, 1, 1) } else { b }
}

/// Frame `f` drawn whole with the tiles `GetTiles` gave it, on a canvas
/// `bounds` wide with the pivot at `-bounds.0, -bounds.1`: pixel indices.
pub fn draw(
    rom: &[u8],
    t: &Tables,
    f: &Frame,
    tiles: (i32, u32),
    b: (i32, i32, i32, i32),
) -> Result<Vec<u8>, String> {
    let (cw, ch) = ((b.2 - b.0) as usize, (b.3 - b.1) as usize);
    let mut px = vec![0u8; cw * ch];
    let first = if tiles.0 < 0 {
        t.tiles8.wrapping_add((tiles.0 as u32).wrapping_mul(64))
    } else {
        t.tiles4 + 32 * tiles.0 as u32
    };
    for p in &f.pieces {
        let bpp = if p.bpp8 { 8u8 } else { 4 };
        let len = (p.w * p.h) as usize * bpp as usize / 8;
        let o = (first + 32 * p.tile).wrapping_sub(ROM) as usize;
        let bytes = rom
            .get(o..o + len)
            .ok_or_else(|| format!("tiles at {:#010x} outside the ROM", first + 32 * p.tile))?;
        let (pw, ph, img) = graphics::tile_pixels(bytes, bpp, p.w as usize / 8);
        let (x0, y0) = (p.x - f.pivot.0 as i32 - b.0, p.y - f.pivot.1 as i32 - b.1);
        for y in 0..ph {
            for x in 0..pw {
                let sx = if p.hflip { pw - 1 - x } else { x };
                let sy = if p.vflip { ph - 1 - y } else { y };
                let v = img[sy * pw + sx];
                let (cx, cy) = (x0 + x as i32, y0 + y as i32);
                if v != 0 && cx >= 0 && cy >= 0 && (cx as usize) < cw && (cy as usize) < ch {
                    px[cy as usize * cw + cx as usize] = v;
                }
            }
        }
    }
    Ok(px)
}

/// Names for animations and variants: `[anims]` `"1" = "walk"`,
/// `[variants]` `"6/5" = "float-inflate"`.
#[derive(Default, Debug)]
pub struct Labels {
    pub anims: BTreeMap<u32, String>,
    pub variants: BTreeMap<(u32, usize), String>,
}

pub fn labels(text: &str) -> Result<Labels, String> {
    let v: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut l = Labels::default();
    let num = |s: &str| {
        s.trim()
            .parse::<u32>()
            .map_err(|_| format!("{s:?}: not a number"))
    };
    let name = |v: &toml::Value| v.as_str().map(str::to_string).ok_or("names are strings");
    for (k, v) in v
        .get("anims")
        .and_then(|t| t.as_table())
        .into_iter()
        .flatten()
    {
        l.anims.insert(num(k)?, name(v)?);
    }
    for (k, v) in v
        .get("variants")
        .and_then(|t| t.as_table())
        .into_iter()
        .flatten()
    {
        let (a, b) = k
            .split_once('/')
            .ok_or(format!("{k:?}: not anim/variant"))?;
        l.variants.insert((num(a)?, num(b)? as usize), name(v)?);
    }
    Ok(l)
}

fn ms(frames: i32) -> u16 {
    ((frames.max(1) as f64 * FRAME_MS).round() as u64).clamp(1, u16::MAX as u64) as u16
}

fn end_text(e: End) -> String {
    match e {
        End::Stop => "once".into(),
        End::Loop(k) => format!("loop from step {k}"),
        End::Goto(a, v) => format!("then anim {a}/{v}"),
    }
}

fn box_text(b: &[Hitbox]) -> String {
    b.iter()
        .map(|h| {
            let [l, t, r, bo] = h.rect;
            format!("box{}=({l},{t})-({r},{bo})", h.slot)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pixels for each frame of `a` with the tiles the first step showing it
/// loaded, `None` where no step loads tiles.
fn frame_pixels(
    rom: &[u8],
    t: &Tables,
    a: &Anim,
    b: (i32, i32, i32, i32),
) -> Result<Vec<Option<Vec<u8>>>, String> {
    let mut out = vec![None; a.frames.len()];
    for s in a
        .variants
        .iter()
        .flat_map(|v| &v.steps)
        .filter(|s| s.frame != HIDDEN)
    {
        let i = (s.frame & 0x0FFF_FFFF) as usize;
        if let (None, Some(f), Some(tiles)) = (&out[i], &a.frames[i], s.tiles) {
            out[i] = Some(draw(rom, t, f, tiles, b)?);
        }
    }
    Ok(out)
}

/// The Aseprite file for `a`: indexed with `pal`, one layer, the frames
/// first (tag `frames`), then each variant's steps as linked cels (a tag a
/// variant, named from `labels`), each step's delay and hitboxes in its
/// cel's user data; the pivot in the sprite's.
pub fn aseprite(
    rom: &[u8],
    t: &Tables,
    a: &Anim,
    pal: &[u16],
    labels: &Labels,
) -> Result<Vec<u8>, String> {
    use aseprite::{AsepriteFile, Color, ColorMode, LoopDirection, Pixels, UserData};
    let e = |x: aseprite::AsepriteError| x.to_string();
    let b = bounds(a);
    let (cw, ch) = ((b.2 - b.0) as u16, (b.3 - b.1) as u16);
    let pixels = frame_pixels(rom, t, a, b)?;
    let mut f = AsepriteFile::new(cw, ch, ColorMode::Indexed);
    let colours: Vec<Color> = (0..16)
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
    let layer = f.add_layer("frame");
    let ud = |s: String| UserData {
        text: Some(s),
        ..Default::default()
    };
    let blank = vec![0u8; cw as usize * ch as usize];
    for (i, p) in pixels.iter().enumerate() {
        let k = f.add_frame(100);
        let cel = Pixels::new(
            p.clone().unwrap_or_else(|| blank.clone()),
            cw,
            ch,
            ColorMode::Indexed,
        )
        .map_err(e)?;
        f.set_cel(layer, k, cel, 0, 0).map_err(e)?;
        f.set_cel_user_data(layer, k, ud(format!("frame={i}")));
    }
    let n = pixels.len().max(1);
    if pixels.is_empty() {
        let k = f.add_frame(100);
        let cel = Pixels::new(blank.clone(), cw, ch, ColorMode::Indexed).map_err(e)?;
        f.set_cel(layer, k, cel, 0, 0).map_err(e)?;
    }
    f.add_tag("frames", 0..=n - 1, LoopDirection::Forward)
        .map_err(e)?;
    let mut at = n;
    for (vi, v) in a.variants.iter().enumerate() {
        if v.steps.is_empty() {
            continue;
        }
        for s in &v.steps {
            let k = f.add_frame(ms(s.delay));
            if s.frame == HIDDEN {
                let cel = Pixels::new(blank.clone(), cw, ch, ColorMode::Indexed).map_err(e)?;
                f.set_cel(layer, k, cel, 0, 0).map_err(e)?;
            } else {
                f.set_linked_cel(layer, k, (s.frame & 0x0FFF_FFFF) as usize)
                    .map_err(e)?;
            }
            f.set_cel_user_data(
                layer,
                k,
                ud(format!(
                    "frame={} video_frames={} {}",
                    s.frame as i32,
                    s.delay,
                    box_text(&s.boxes)
                )
                .trim_end()
                .to_string()),
            );
        }
        let name = labels
            .variants
            .get(&(a.id, vi))
            .cloned()
            .unwrap_or_else(|| format!("v{vi}"));
        let tag = f
            .add_tag(&name, at..=at + v.steps.len() - 1, LoopDirection::Forward)
            .map_err(e)?;
        f.set_tag_user_data(
            tag,
            ud(format!(
                "variant {vi} at {:#010x}; {}",
                v.addr,
                end_text(v.end)
            )),
        );
        at += v.steps.len();
    }
    f.set_sprite_user_data(ud(format!(
        "anim={} tables={:#010x} pivot={},{}",
        a.id, t.at, -b.0, -b.1
    )));
    let mut out = Vec::new();
    f.write_to(&mut out).map_err(e)?;
    Ok(out)
}

/// Every frame of `a` side by side, indexed PNG.
pub fn strip(rom: &[u8], t: &Tables, a: &Anim, pal: &[u16]) -> Result<Vec<u8>, String> {
    let b = bounds(a);
    let (fw, fh) = ((b.2 - b.0) as usize, (b.3 - b.1) as usize);
    let pixels = frame_pixels(rom, t, a, b)?;
    let n = pixels.len().max(1);
    let w = fw * n;
    let mut px = vec![0u8; w * fh];
    for (i, p) in pixels.iter().enumerate() {
        if let Some(p) = p {
            for y in 0..fh {
                px[y * w + i * fw..y * w + i * fw + fw].copy_from_slice(&p[y * fw..y * fw + fw]);
            }
        }
    }
    graphics::indexed_png(w, fh, 4, &px, pal)
}

/// The text summary of `a`: frames, canvas, and each variant's steps.
pub fn describe(a: &Anim, labels: &Labels) -> String {
    let b = bounds(a);
    let mut s = String::new();
    let label = labels
        .anims
        .get(&a.id)
        .map_or(String::new(), |l| format!(" {l}"));
    let _ = writeln!(
        s,
        "anim {}{label}: {} frames{}, {} variants, canvas {}x{} pivot {},{}",
        a.id,
        a.frames.len(),
        if a.streamed {
            " (streamed, not drawn)"
        } else {
            ""
        },
        a.variants.len(),
        b.2 - b.0,
        b.3 - b.1,
        -b.0,
        -b.1
    );
    for (vi, v) in a.variants.iter().enumerate() {
        let vl = labels
            .variants
            .get(&(a.id, vi))
            .map_or(String::new(), |l| format!(" {l}"));
        let steps: Vec<String> = v
            .steps
            .iter()
            .map(|st| {
                let mut x = format!("{}x{}", st.frame as i32, st.delay);
                for ev in &st.events {
                    x += &format!(" [{ev}]");
                }
                x
            })
            .collect();
        let total: i32 = v.steps.iter().map(|s| s.delay).sum();
        let _ = writeln!(
            s,
            "  v{vi}{vl} {:#010x}: {} ({total} video frames; {})",
            v.addr,
            steps.join(" "),
            end_text(v.end)
        );
    }
    s
}

/// Hitbox lines for `a`, tab separated: anim, variant, step, frame, slot,
/// left, top, right, bottom, the slot word. Only steps with a box set.
pub fn hitbox_lines(a: &Anim) -> String {
    let mut s = String::new();
    for (vi, v) in a.variants.iter().enumerate() {
        for (k, st) in v.steps.iter().enumerate() {
            for h in &st.boxes {
                let [l, t, r, b] = h.rect;
                let _ = writeln!(
                    s,
                    "{}\t{vi}\t{k}\t{}\t{}\t{l}\t{t}\t{r}\t{b}\t{:#x}",
                    a.id, st.frame as i32, h.slot, h.word
                );
            }
        }
    }
    s
}

/// Animation ids an `AnimInfo` table (4 bytes an entry: animId, variant)
/// of `n` entries at `at` uses, in order, each once.
pub fn anim_info_ids(rom: &[u8], at: u32, n: u32) -> Result<Vec<u32>, String> {
    let mut ids = Vec::new();
    for i in 0..n {
        let id = half(rom, at + 4 * i)? as u32;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(ids)
}

/// 16 colours at `at`.
pub fn palette(rom: &[u8], at: u32) -> Result<Vec<u16>, String> {
    (0..16).map(|i| half(rom, at + 2 * i)).collect()
}
