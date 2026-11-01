//! mGBA 0.11-dev's `canvas` and `image` (`src/script/canvas.c`,
//! `src/script/image.c`, `src/util/image.c` and `src/util/image/font.c`
//! at fd9dc5c), for overlays on the game picture.
//!
//! `canvas:newLayer(w, h)` gives one of [`LAYERS`] layers, each an image
//! drawn at its position over the picture, newest on top. What a layer
//! shows is what its image held at the last `canvas` update after
//! `layer:update()`, as in mGBA, whose canvas updates in a `frame` callback
//! added before any script's: the host updates at each frame, before the
//! scripts' `frame` callbacks ([`crate::Host::frame`]), and the front end
//! draws [`Canvas::compose`] over the picture.
//!
//! `image.new`, `image.load` (PNG) and `image.newPainter` make images and
//! painters; images have `getPixel`, `setPixel` (ARGB), `drawImageOpaque`,
//! `drawImage`, `save`; painters `setBlend`, `setFill`, `setFillColor`,
//! `setStrokeWidth`, `setStrokeColor`, `drawRectangle`, `drawLine`,
//! `drawCircle`, `drawMask`, and the text calls `loadFont`, `setFontSize`,
//! `drawText`, `textRunMetrics`, `textBoxSize`, with `C.ALIGN`. Pixels
//! follow mGBA's integer blending exactly; text is drawn by `ab_glyph`
//! where mGBA has FreeType, so glyph edges differ slightly, the size is
//! FreeType's (points at 100 dpi), and a stroke is the glyph's coverage
//! widened by the stroke width rather than FreeType's stroked outline.
//!
//! Unlike mGBA: `width` and `height` are `screenWidth` and
//! `screenHeight`, the game picture's 240×160. A layer placed past the
//! picture's left or right edge widens the canvas there into a sidebar
//! (up to [`SIDEBAR_MAX`] a side, the picture's height), which the window
//! draws beside the game at its scale, as mGBA 0.11-dev's canvas reaches
//! past the picture into the window; above and below it is clipped. A
//! headless run has a canvas too, so its dumps show what the window
//! would, sidebars included; and unloading a script frees its layers.

use ab_glyph::{Font as _, FontVec, GlyphId, PxScale, point};
use mlua::{AnyUserData, FromLua, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};
use runtime::platform::{HEIGHT, WIDTH};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// The widest a sidebar gets, each side of the picture, in its pixels.
pub const SIDEBAR_MAX: usize = WIDTH;

/// mGBA's `VIDEO_LAYER_OVERLAY_COUNT`.
pub const LAYERS: usize = 10;
/// `image.new`'s limit, as mGBA's (either side under 10000).
const IMAGE_MAX: u32 = 10_000;

/// `C.ALIGN`'s (`enum mAlignment`).
const ALIGN_HCENTER: i64 = 0x02;
const ALIGN_RIGHT: i64 = 0x03;
const ALIGN_TOP: i64 = 0x10;
const ALIGN_VCENTER: i64 = 0x20;
const ALIGN_BASELINE: i64 = 0x40;
const ALIGN_HORIZONTAL: i64 = 0x03;
const ALIGN_VERTICAL: i64 = 0x70;
pub(crate) const ALIGN: &[(&str, i64)] = &[
    ("LEFT", 0x01),
    ("HCENTER", ALIGN_HCENTER),
    ("RIGHT", ALIGN_RIGHT),
    ("TOP", ALIGN_TOP),
    ("VCENTER", ALIGN_VCENTER),
    ("BOTTOM", 0x30),
    ("BASELINE", ALIGN_BASELINE),
];

/// An image: ARGB pixels, row by row.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub px: Vec<u32>,
    /// Loaded from a greyscale PNG (mGBA's `mCOLOR_L8`): as a mask, its
    /// grey is the coverage.
    grey: bool,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Image {
        Image {
            width,
            height,
            px: vec![0; width as usize * height as usize],
            grey: false,
        }
    }

    fn at(&self, x: i64, y: i64) -> Option<usize> {
        (x >= 0 && y >= 0 && x < self.width as i64 && y < self.height as i64)
            .then(|| y as usize * self.width as usize + x as usize)
    }

    /// The pixel at `x`, `y`; 0 outside (`mImageGetPixel`).
    pub fn get(&self, x: i64, y: i64) -> u32 {
        self.at(x, y).map_or(0, |i| self.px[i])
    }

    /// Set the pixel at `x`, `y`; nothing outside.
    pub fn set(&mut self, x: i64, y: i64, c: u32) {
        if let Some(i) = self.at(x, y) {
            self.px[i] = c;
        }
    }

    /// `src`'s pixels at `x`, `y` in this image, clipped: `f(src, dst)`
    /// gives each new pixel (`COMPOSITE_BOUNDS_INIT`'s walk).
    fn each(&mut self, src: &Image, x: i64, y: i64, mut f: impl FnMut(u32, u32) -> u32) {
        for sy in 0..src.height as i64 {
            for sx in 0..src.width as i64 {
                if let Some(i) = self.at(x + sx, y + sy) {
                    self.px[i] = f(src.get(sx, sy), self.px[i]);
                }
            }
        }
    }

    /// `drawImageOpaque` (`mImageBlit`).
    pub fn blit(&mut self, src: &Image, x: i64, y: i64) {
        self.each(src, x, y, |s, _| s);
    }

    /// `drawImage` (`mImageCompositeWithAlpha`).
    pub fn composite(&mut self, src: &Image, x: i64, y: i64, alpha: f32) {
        if (1.0..257.0 / 256.0).contains(&alpha) {
            self.each(
                src,
                x,
                y,
                |s, d| if s < 0xFF00_0000 { mix(s, d) } else { s },
            );
            return;
        }
        if alpha <= 0.0 {
            return;
        }
        let fixed = (alpha.min(256.0) * 512.0) as u32;
        self.each(src, x, y, |s, d| {
            let a = (((s >> 24) * fixed) >> 9).min(0xFF);
            mix((s & 0x00FF_FFFF) | (a << 24), d)
        });
    }

    /// Load the PNG at `path` (`mImageLoad`), or a GIF's first frame (for
    /// the BizHawk shim's `gui.drawImage`).
    pub fn load(path: &Path) -> Result<Image, String> {
        let head = std::fs::read(path).map_err(|e| e.to_string())?;
        if head.starts_with(b"GIF8") {
            return Image::gif(&head);
        }
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(file));
        dec.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = dec.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; r.output_buffer_size().ok_or("too big")?];
        let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let (w, h) = (info.width, info.height);
        if w >= IMAGE_MAX || h >= IMAGE_MAX {
            return Err("too big".into());
        }
        let argb = |r: u8, g: u8, b: u8, a: u8| u32::from_be_bytes([a, r, g, b]);
        let data = &buf[..info.buffer_size()];
        use png::ColorType::*;
        let px: Vec<u32> = match info.color_type {
            Rgba => data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| argb(p[0], p[1], p[2], p[3]))
                .collect(),
            Rgb => data
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| argb(p[0], p[1], p[2], 0xFF))
                .collect(),
            GrayscaleAlpha => data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| argb(p[0], p[0], p[0], p[1]))
                .collect(),
            Grayscale => data.iter().map(|&l| argb(l, l, l, 0xFF)).collect(),
            Indexed => return Err("a palette left after expanding".into()),
        };
        Ok(Image {
            width: w,
            height: h,
            px,
            grey: info.color_type == Grayscale,
        })
    }

    fn gif(data: &[u8]) -> Result<Image, String> {
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut d = opts.read_info(data).map_err(|e| e.to_string())?;
        let (w, h) = (d.width() as u32, d.height() as u32);
        if w >= IMAGE_MAX || h >= IMAGE_MAX {
            return Err("too big".into());
        }
        let mut img = Image::new(w, h);
        let f = d
            .read_next_frame()
            .map_err(|e| e.to_string())?
            .ok_or("no frame")?;
        for (i, p) in f.buffer.as_chunks::<4>().0.iter().enumerate() {
            let x = f.left as i64 + (i % f.width as usize) as i64;
            let y = f.top as i64 + (i / f.width as usize) as i64;
            img.set(x, y, u32::from_be_bytes([p[3], p[0], p[1], p[2]]));
        }
        Ok(img)
    }

    /// Write it as an RGBA PNG at `path` (`mImageSave`).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let data: Vec<u8> = self
            .px
            .iter()
            .flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8])
            .collect();
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&data).map_err(|e| e.to_string())?;
        w.finish().map_err(|e| e.to_string())
    }
}

/// `a` over `b`, ARGB (`mColorMixARGB8`, its integer form).
pub fn mix(a: u32, b: u32) -> u32 {
    let aa = a >> 24;
    if aa == 0 {
        return b;
    }
    let ab = b >> 24;
    let alpha = aa * 0xFF + ab * (0xFF - aa);
    let mut c = 0;
    for shift in [0, 8, 16] {
        let ch = |v: u32| (v >> shift) & 0xFF;
        let v = (ch(a) * aa * 0xFF + ch(b) * ab * (0xFF - aa)) / alpha;
        c |= v.min(0xFF) << shift;
    }
    c | ((alpha / 0xFF).min(0xFF) << 24)
}

/// Each channel of `a` times `b`'s, over 255 (`_mColorMultiply`).
fn multiply(a: u32, b: u32) -> u32 {
    [0, 8, 16, 24].iter().fold(0, |c, &shift| {
        let v = ((a >> shift) & 0xFF) * ((b >> shift) & 0xFF) / 0xFF;
        c | (v.min(0xFF) << shift)
    })
}

/// A painter's font, at its size.
struct Font {
    f: FontVec,
    /// Points, at FreeType's 100 dpi as mGBA sets it.
    pt: f32,
}

impl Font {
    /// Pixels per font unit.
    fn k(&self) -> f32 {
        self.pt * 100.0 / 72.0 / self.f.units_per_em().unwrap_or(1000.0)
    }

    /// FreeType's size metrics: ascender rounded up, descender down
    /// (negative), line height rounded.
    fn ascender(&self) -> f32 {
        (self.f.ascent_unscaled() * self.k()).ceil()
    }

    fn descender(&self) -> f32 {
        (self.f.descent_unscaled() * self.k()).floor()
    }

    fn line_height(&self) -> f32 {
        ((self.f.height_unscaled() + self.f.line_gap_unscaled()) * self.k()).round()
    }

    fn scale(&self) -> PxScale {
        PxScale::from(self.f.height_unscaled() * self.k())
    }

    /// Glyph `id`'s advance, in whole pixels as FreeType's hinted load gives.
    fn advance(&self, id: GlyphId) -> f32 {
        (self.f.h_advance_unscaled(id) * self.k()).round()
    }

    fn kern(&self, a: Option<GlyphId>, b: GlyphId) -> f32 {
        a.map_or(0.0, |a| self.f.kern_unscaled(a, b) * self.k())
    }

    /// The width of `line` (`mFontRunMetrics`).
    fn width(&self, line: &str) -> f32 {
        let mut last = None;
        let mut w = 0.0;
        for c in line.chars() {
            let id = self.f.glyph_id(c);
            w += self.kern(last, id) + self.advance(id);
            last = Some(id);
        }
        w
    }

    /// Glyph `id`'s coverage as a greyscale mask, and its left and top
    /// offsets from the pen, widened by `stroke` pixels if any.
    fn glyph(&self, id: GlyphId, stroke: u32) -> Option<(Image, i64, i64)> {
        let g = self
            .f
            .outline_glyph(id.with_scale_and_position(self.scale(), point(0.0, 0.0)))?;
        let b = g.px_bounds();
        let (w, h) = (b.width() as u32, b.height() as u32);
        let mut cov = vec![0u8; (w * h) as usize];
        g.draw(|x, y, c| {
            if x < w && y < h {
                cov[(y * w + x) as usize] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        let r = stroke as i64;
        let (mw, mh) = (w as i64 + 2 * r, h as i64 + 2 * r);
        let mut m = Image::new(mw as u32, mh as u32);
        m.grey = true;
        let get = |x: i64, y: i64| {
            if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                0
            } else {
                cov[(y * w as i64 + x) as usize]
            }
        };
        for y in 0..mh {
            for x in 0..mw {
                let mut v = 0;
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy <= r * r + r {
                            v = v.max(get(x - r + dx, y - r + dy));
                        }
                    }
                }
                m.set(x, y, v as u32);
            }
        }
        Some((m, b.min.x as i64 - r, b.min.y as i64 - r))
    }
}

/// A painter: drawing state over an image (`struct mPainter`).
struct Painter {
    image: Rc<RefCell<Image>>,
    blend: bool,
    fill: bool,
    fill_color: u32,
    stroke_width: u32,
    stroke_color: u32,
    font: Option<Font>,
}

impl Painter {
    fn pixel(&self, img: &mut Image, x: i64, y: i64, c: u32) {
        // As mGBA's `mPainterDrawPixel`: blending mixes the stroke colour,
        // whichever colour is drawn.
        let c = if self.blend {
            mix(self.stroke_color, img.get(x, y))
        } else {
            c
        };
        img.set(x, y, c);
    }

    fn fill_rect(&self, img: &mut Image, x: i64, y: i64, w: i64, h: i64, c: u32) {
        for yy in y.max(0)..(y + h).min(img.height as i64) {
            for xx in x.max(0)..(x + w).min(img.width as i64) {
                let d = img.get(xx, yy);
                let v = if !self.blend || c >= 0xFF00_0000 {
                    c
                } else {
                    mix(c, d)
                };
                img.set(xx, yy, v);
            }
        }
    }

    fn rectangle(&self, x: i64, y: i64, w: i64, h: i64) {
        let img = &mut self.image.borrow_mut();
        let s = self.stroke_width as i64;
        let (iw, ih) = (w - s * 2, h - s * 2);
        if self.fill && iw > 0 && ih > 0 {
            self.fill_rect(img, x + s, y + s, iw, ih, self.fill_color);
        }
        if s == 0 || w < 0 || h < 0 {
            return;
        }
        let c = self.stroke_color;
        if w <= s * 2 || h <= s * 2 {
            self.fill_rect(img, x, y, w, h, c);
        } else {
            let (lr, tb) = (h - s, w - s);
            self.fill_rect(img, x, y, tb, s, c);
            self.fill_rect(img, x, y + s, s, lr, c);
            self.fill_rect(img, x + s, y + h - s, tb, s, c);
            self.fill_rect(img, x + w - s, y, s, lr, c);
        }
    }

    fn line(&self, x1: i64, y1: i64, x2: i64, y2: i64) {
        let s = self.stroke_width as i64;
        if s == 0 {
            return;
        }
        let img = &mut self.image.borrow_mut();
        let (dx, dy) = ((x2 - x1).abs(), (y2 - y1).abs());
        let xi = if x2 < x1 { -1 } else { 1 };
        let yi = if y2 < y1 { -1 } else { 1 };
        let c = self.stroke_color;
        if dx > dy {
            let mut r = 2 * dy - dx;
            let mut y = y1;
            let mut x = x1;
            while x != x2 + xi {
                for i in 0..s {
                    self.pixel(img, x, y - s / 2 + i, c);
                }
                if r > 0 {
                    y += yi;
                    r -= 2 * dx;
                }
                r += 2 * dy;
                x += xi;
            }
        } else {
            let mut r = 2 * dx - dy;
            let mut x = x1;
            let mut y = y1;
            while y != y2 + yi {
                for i in 0..s {
                    self.pixel(img, x - s / 2 + i, y, c);
                }
                if r > 0 {
                    x += xi;
                    r -= 2 * dy;
                }
                r += 2 * dx;
                y += yi;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn octants(&self, img: &mut Image, x: i64, y: i64, ox: i64, oy: i64, off: i64, c: u32) {
        self.pixel(img, x + oy - off, y + ox - off, c);
        self.pixel(img, x - oy, y + ox - off, c);
        self.pixel(img, x + oy - off, y - ox, c);
        self.pixel(img, x - oy, y - ox, c);
        if ox < oy {
            self.pixel(img, x + ox - off, y + oy - off, c);
            self.pixel(img, x - ox, y + oy - off, c);
            self.pixel(img, x + ox - off, y - oy, c);
            self.pixel(img, x - ox, y - oy, c);
        }
    }

    fn dot2(&self, img: &mut Image, x: i64, y: i64, c: u32) {
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            self.pixel(img, x - dx, y - dy, c);
        }
    }

    /// `mPainterDrawCircle`, step for step.
    fn circle(&self, x: i64, y: i64, diameter: i64) {
        if diameter < 1 {
            return;
        }
        let img = &mut self.image.borrow_mut();
        let (fc, sc) = (self.fill_color, self.stroke_color);
        let radius = diameter / 2;
        let offset = (diameter ^ 1) & 1;
        let mut stroke = self.stroke_width as i64;
        let mut dx = 1;
        let mut residual0 = 1 - radius;
        let mut dy0 = -2 * radius;
        let mut offx = 0;
        let mut y0 = radius;
        if stroke > radius {
            stroke = radius;
            if offset == 0 {
                self.pixel(img, x + radius, y + radius, sc);
            }
        } else if offset == 0 && self.fill {
            self.pixel(img, x + radius, y + radius, fc);
        }
        let mut residual1 = 1 - radius + stroke;
        let mut dy1 = -2 * (radius - stroke);
        let mut y1 = radius - stroke;
        if offset == 0 {
            for i in 0..stroke {
                self.pixel(img, x + radius, y + radius * 2 - i, sc);
                self.pixel(img, x + radius, y + i, sc);
                self.pixel(img, x + radius * 2 - i, y + radius, sc);
                self.pixel(img, x + i, y + radius, sc);
            }
            if self.fill {
                for i in 1..y1 + 1 {
                    self.pixel(img, x + radius, y + radius - i, fc);
                    self.pixel(img, x + radius, y + radius + i, fc);
                    self.pixel(img, x + radius - i, y + radius, fc);
                    self.pixel(img, x + radius + i, y + radius, fc);
                }
            }
        }
        let (cx, cy) = (x + radius, y + radius);
        while offx < y0 {
            if residual0 >= 0 {
                y0 -= 1;
                dy0 += 2;
                residual0 += dy0;
            }
            if residual1 >= 0 {
                y1 -= 1;
                dy1 += 2;
                residual1 += dy1;
            }
            offx += 1;
            dx += 2;
            residual0 += dx;
            residual1 += dx;
            if stroke != 0 {
                if self.fill {
                    if offx == 1 && y1 == 0 && offset != 0 {
                        self.dot2(img, cx, cy, fc);
                    } else {
                        for offy in offx..y1 + 1 {
                            self.octants(img, cx, cy, offx, offy, offset, fc);
                        }
                    }
                }
                if radius == 1 && offset != 0 {
                    self.dot2(img, cx, cy, sc);
                } else {
                    for offy in y1 + 1..y0 + 1 {
                        if offx == 1 && offy == 1 && y1 == 0 && offset != 0 || offx > offy {
                            continue;
                        }
                        self.octants(img, cx, cy, offx, offy, offset, sc);
                    }
                }
            } else if self.fill {
                if offx == 1 && y0 == 0 && offset != 0 {
                    self.dot2(img, x, y, fc);
                } else {
                    for offy in offx..y0 + 1 {
                        self.octants(img, cx, cy, offx, offy, offset, fc);
                    }
                }
            }
        }
    }

    /// `mPainterDrawMask`: `mask` at `x`, `y`, each channel times the
    /// fill colour; a greyscale mask's grey is its coverage.
    fn mask_on(&self, img: &mut Image, mask: &Image, x: i64, y: i64, fill: u32) {
        if !self.fill {
            return;
        }
        img.each(mask, x, y, |m, d| {
            let m = if mask.grey { (m << 24) | 0xFF_FFFF } else { m };
            let c = multiply(fill, m);
            if self.blend || fill < 0xFF00_0000 {
                mix(c, d)
            } else {
                c
            }
        });
    }

    /// `mPainterDrawText`: lines of `text` at `x`, `y`, placed by `align`;
    /// a stroke first, if it has a width, then the fill.
    fn text(&self, text: &str, x: i64, y: i64, align: i64) {
        let Some(f) = &self.font else {
            return;
        };
        let lines: Vec<&str> = text.split('\n').collect();
        let run = f.ascender() - f.descender();
        let box_h = run * lines.len() as f32;
        let y = y as f32
            + match align & ALIGN_VERTICAL {
                ALIGN_TOP => f.ascender(),
                ALIGN_BASELINE => 0.0,
                ALIGN_VCENTER => f.ascender() - (box_h / 2.0).floor(),
                _ => f.ascender() - box_h,
            };
        let img = &mut self.image.borrow_mut();
        let passes = [
            (self.stroke_width > 0).then_some((self.stroke_color, self.stroke_width)),
            Some((self.fill_color, 0)),
        ];
        for (color, stroke) in passes.into_iter().flatten() {
            let mut yy = y;
            for line in &lines {
                let w = f.width(line);
                let mut pen = x as f32
                    - match align & ALIGN_HORIZONTAL {
                        ALIGN_HCENTER => (w / 2.0).floor(),
                        ALIGN_RIGHT => w,
                        _ => 0.0,
                    };
                let mut last = None;
                for c in line.chars() {
                    let id = f.f.glyph_id(c);
                    pen += f.kern(last, id);
                    if let Some((m, left, top)) = f.glyph(id, stroke) {
                        let gx = pen.floor() as i64 + left;
                        let gy = yy.floor() as i64 + top;
                        self.mask_on(img, &m, gx, gy, color);
                    }
                    pen += f.advance(id);
                    last = Some(id);
                }
                yy += f.line_height();
            }
        }
    }
}

/// What the canvas shows: ARGB, `width` by the picture's height, the
/// picture `left` pixels in (a sidebar each side of it, maybe none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    pub px: Vec<u32>,
    pub width: usize,
    pub left: usize,
}

impl Overlay {
    /// The sidebar right of the picture, in pixels.
    pub fn right(&self) -> usize {
        self.width - self.left - WIDTH
    }
}

/// A layer: its image, where it is, and what it shows.
struct Layer {
    /// Unique to this layer, so a script's handle to a layer unloaded
    /// doesn't reach the one made in its slot after.
    id: u64,
    image: Rc<RefCell<Image>>,
    x: i32,
    y: i32,
    /// The script that made it (0 the prompt).
    script: u32,
    /// `layer:update()` or `setPosition` since the last canvas update.
    dirty: bool,
    /// The pixels and place shown: as at the last update after `dirty`.
    shown: Image,
    at: (i32, i32),
}

/// The `canvas` global's state: its layers, by slot.
#[derive(Default)]
pub struct Canvas {
    layers: RefCell<Vec<Option<Layer>>>,
    made: std::cell::Cell<u64>,
}

impl Canvas {
    /// Show what each layer marked changed holds now (`canvas:update`).
    pub fn update(&self) {
        for l in self.layers.borrow_mut().iter_mut().flatten() {
            if std::mem::take(&mut l.dirty) {
                l.shown.clone_from(&l.image.borrow());
                l.at = (l.x, l.y);
            }
        }
    }

    /// Free the layers script `id` made.
    pub fn unload(&self, id: u32) {
        for slot in self.layers.borrow_mut().iter_mut() {
            if slot.as_ref().is_some_and(|l| l.script == id) {
                *slot = None;
            }
        }
    }

    /// The layers over a transparent picture and its sidebars, oldest at
    /// the bottom; `None` while there are none.
    pub fn compose(&self) -> Option<Overlay> {
        let layers = self.layers.borrow();
        let mut shown = layers.iter().flatten().peekable();
        shown.peek()?;
        // The sidebars: as far as a layer on the picture's rows reaches.
        let (mut left, mut right) = (0, 0);
        for l in layers.iter().flatten() {
            let (x, y) = (l.at.0 as i64, l.at.1 as i64);
            let (w, h) = (l.shown.width as i64, l.shown.height as i64);
            if y >= HEIGHT as i64 || y + h <= 0 || w == 0 {
                continue;
            }
            left = left.max(-x);
            right = right.max(x + w - WIDTH as i64);
        }
        let left = left.clamp(0, SIDEBAR_MAX as i64) as usize;
        let right = right.clamp(0, SIDEBAR_MAX as i64) as usize;
        let width = left + WIDTH + right;
        let mut o = Image::new(width as u32, HEIGHT as u32);
        for l in shown {
            o.composite(&l.shown, l.at.0 as i64 + left as i64, l.at.1 as i64, 1.0);
        }
        Some(Overlay {
            px: o.px,
            width,
            left,
        })
    }

    fn new_layer(&self, w: i64, h: i64, script: u32) -> Option<(usize, u64)> {
        if w <= 0 || h <= 0 || w >= IMAGE_MAX as i64 || h >= IMAGE_MAX as i64 {
            return None;
        }
        let mut layers = self.layers.borrow_mut();
        if layers.is_empty() {
            layers.resize_with(LAYERS, || None);
        }
        let i = layers.iter().position(Option::is_none)?;
        let image = Image::new(w as u32, h as u32);
        let id = self.made.get() + 1;
        self.made.set(id);
        layers[i] = Some(Layer {
            id,
            image: Rc::new(RefCell::new(image.clone())),
            x: 0,
            y: 0,
            script,
            dirty: false,
            shown: image,
            at: (0, 0),
        });
        Some((i, id))
    }
}

/// A number as mGBA's casts take it: an integer, or a float truncated.
struct Num(i64);

impl FromLua for Num {
    fn from_lua(v: Value, lua: &Lua) -> mlua::Result<Num> {
        match v {
            Value::Integer(i) => Ok(Num(i)),
            Value::Number(f) => Ok(Num(f as i64)),
            v => Ok(Num(i64::from_lua(v, lua)?)),
        }
    }
}

/// [`Ctx::allowed`]'s type.
pub(crate) type Allowed = Box<dyn Fn(&Lua, &Path) -> bool>;

/// What `canvas`, `image` and painters share: which files a script may
/// read and write.
pub(crate) struct Ctx {
    /// Whether the running script may use the file at a path.
    pub allowed: Allowed,
}

fn check(ctx: &Ctx, lua: &Lua, name: &str, p: &str) -> mlua::Result<()> {
    if (ctx.allowed)(lua, Path::new(p)) {
        return Ok(());
    }
    Err(mlua::Error::runtime(format!(
        "{name}: {p} is outside the script's folder (the script is sandboxed; \
         give it full access to use it)"
    )))
}

#[derive(Clone)]
struct ImageRef(Rc<RefCell<Image>>, Rc<Ctx>);

fn image_of(ud: &AnyUserData) -> mlua::Result<Rc<RefCell<Image>>> {
    Ok(ud.borrow::<ImageRef>()?.0.clone())
}

impl UserData for ImageRef {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("width", |_, i| Ok(i.0.borrow().width));
        f.add_field_method_get("height", |_, i| Ok(i.0.borrow().height));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("getPixel", |_, i, (x, y): (Num, Num)| {
            Ok(i.0.borrow().get(x.0 as u32 as i64, y.0 as u32 as i64))
        });
        m.add_method("setPixel", |_, i, (x, y, c): (Num, Num, Num)| {
            i.0.borrow_mut()
                .set(x.0 as u32 as i64, y.0 as u32 as i64, c.0 as u32);
            Ok(())
        });
        // A copy of the source first: an image may be drawn onto itself.
        m.add_method(
            "drawImageOpaque",
            |_, i, (s, x, y): (AnyUserData, Num, Num)| {
                let src = image_of(&s)?.borrow().clone();
                i.0.borrow_mut()
                    .blit(&src, x.0 as i32 as i64, y.0 as i32 as i64);
                Ok(())
            },
        );
        m.add_method(
            "drawImage",
            |_, i, (s, x, y, a): (AnyUserData, Num, Num, Option<f32>)| {
                let src = image_of(&s)?.borrow().clone();
                i.0.borrow_mut().composite(
                    &src,
                    x.0 as i32 as i64,
                    y.0 as i32 as i64,
                    a.unwrap_or(1.0),
                );
                Ok(())
            },
        );
        m.add_method("save", |lua, i, (p, fmt): (String, Option<String>)| {
            if fmt.is_some_and(|f| !f.eq_ignore_ascii_case("png")) {
                return Ok(false);
            }
            check(&i.1, lua, "image:save", &p)?;
            Ok(i.0.borrow().save(Path::new(&p)).is_ok())
        });
    }
}

struct PainterRef(RefCell<Painter>, Rc<Ctx>);

/// `textRunMetrics`'s answer (`struct mTextRunMetrics`), in pixels.
struct Metrics {
    width: f32,
    height: f32,
    baseline: f32,
}

impl UserData for Metrics {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("width", |_, r, ()| Ok(r.width));
        m.add_method("height", |_, r, ()| Ok(r.height));
        m.add_method("descender", |_, r, ()| Ok(r.baseline));
        m.add_method("ascender", |_, r, ()| Ok(r.height - r.baseline));
    }
}

impl UserData for PainterRef {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("setBlend", |_, p, on: bool| {
            p.0.borrow_mut().blend = on;
            Ok(())
        });
        m.add_method("setFill", |_, p, on: bool| {
            p.0.borrow_mut().fill = on;
            Ok(())
        });
        m.add_method("setFillColor", |_, p, c: Num| {
            p.0.borrow_mut().fill_color = c.0 as u32;
            Ok(())
        });
        m.add_method("setStrokeWidth", |_, p, w: Num| {
            p.0.borrow_mut().stroke_width = w.0 as u32;
            Ok(())
        });
        m.add_method("setStrokeColor", |_, p, c: Num| {
            p.0.borrow_mut().stroke_color = c.0 as u32;
            Ok(())
        });
        let s32 = |n: Num| n.0 as i32 as i64;
        m.add_method(
            "drawRectangle",
            move |_, p, (x, y, w, h): (Num, Num, Num, Num)| {
                p.0.borrow().rectangle(s32(x), s32(y), s32(w), s32(h));
                Ok(())
            },
        );
        m.add_method(
            "drawLine",
            move |_, p, (a, b, c, d): (Num, Num, Num, Num)| {
                p.0.borrow().line(s32(a), s32(b), s32(c), s32(d));
                Ok(())
            },
        );
        m.add_method("drawCircle", move |_, p, (x, y, d): (Num, Num, Num)| {
            p.0.borrow().circle(s32(x), s32(y), s32(d));
            Ok(())
        });
        m.add_method(
            "drawMask",
            move |_, p, (mask, x, y): (AnyUserData, Num, Num)| {
                let mask = image_of(&mask)?.borrow().clone();
                let p = p.0.borrow();
                let fill = p.fill_color;
                p.mask_on(&mut p.image.borrow_mut(), &mask, s32(x), s32(y), fill);
                Ok(())
            },
        );
        m.add_method(
            "drawText",
            move |_, p, (t, x, y, a): (String, Num, Num, Option<Num>)| {
                let a = a.map_or(ALIGN_TOP | 0x01, |a| a.0 as i32 as i64);
                p.0.borrow().text(&t, s32(x), s32(y), a);
                Ok(())
            },
        );
        m.add_method("loadFont", |lua, p, path: String| {
            check(&p.1, lua, "painter:loadFont", &path)?;
            // mGBA keeps the old font when the new one won't open.
            if let Some(f) = std::fs::read(&path)
                .ok()
                .and_then(|b| FontVec::try_from_vec(b).ok())
            {
                p.0.borrow_mut().font = Some(Font { f, pt: 8.0 });
            }
            Ok(())
        });
        m.add_method("setFontSize", |_, p, pt: f32| {
            if let Some(f) = &mut p.0.borrow_mut().font {
                f.pt = pt;
            }
            Ok(())
        });
        m.add_method("textRunMetrics", |_, p, t: String| {
            let p = p.0.borrow();
            Ok(p.font.as_ref().map(|f| Metrics {
                width: f.width(t.split('\n').next().unwrap_or("")),
                height: f.ascender() - f.descender(),
                baseline: -f.descender(),
            }))
        });
        m.add_method("textBoxSize", |lua, p, t: String| {
            let p = p.0.borrow();
            let Some(f) = &p.font else {
                return Ok(Value::Nil);
            };
            let lines: Vec<&str> = t.split('\n').collect();
            let w = lines.iter().map(|l| f.width(l)).fold(0.0, f32::max);
            let size = lua.create_table()?;
            size.set("width", w)?;
            size.set(
                "height",
                (f.ascender() - f.descender()) * lines.len() as f32,
            )?;
            Ok(Value::Table(size))
        });
    }
}

#[derive(Clone)]
struct CanvasRef(Rc<Canvas>, Rc<Ctx>);

/// A layer: the canvas, its slot and its [`Layer::id`].
struct LayerRef(Rc<Canvas>, usize, u64, Rc<Ctx>);

impl LayerRef {
    fn with<T>(&self, f: impl FnOnce(&mut Layer) -> T) -> mlua::Result<T> {
        let mut layers = self.0.layers.borrow_mut();
        match layers.get_mut(self.1).and_then(Option::as_mut) {
            Some(l) if l.id == self.2 => Ok(f(l)),
            _ => Err(mlua::Error::runtime("the layer's script was unloaded")),
        }
    }
}

impl UserData for LayerRef {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("image", |_, l| {
            Ok(ImageRef(l.with(|l| l.image.clone())?, l.3.clone()))
        });
        f.add_field_method_get("x", |_, l| l.with(|l| l.x));
        f.add_field_method_get("y", |_, l| l.with(|l| l.y));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("update", |_, l, ()| l.with(|l| l.dirty = true));
        m.add_method("setPosition", |_, l, (x, y): (Num, Num)| {
            l.with(|l| {
                (l.x, l.y) = (x.0 as i32, y.0 as i32);
                l.dirty = true;
            })
        });
    }
}

impl UserData for CanvasRef {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        // The BizHawk shim's: free a layer (its padding changed), which
        // mGBA's canvas can't.
        f.add_field_method_get("_free", |lua, c| {
            let c = c.0.clone();
            lua.create_function(move |_, l: AnyUserData| {
                let l = l.borrow::<LayerRef>()?;
                let mut layers = c.layers.borrow_mut();
                if let Some(slot) = layers.get_mut(l.1)
                    && slot.as_ref().is_some_and(|x| x.id == l.2)
                {
                    *slot = None;
                }
                Ok(())
            })
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("newLayer", |lua, c, (w, h): (Num, Num)| {
            let script: u32 = lua.globals().get("__script").unwrap_or(0);
            let (w, h) = (w.0 as i32 as i64, h.0 as i32 as i64);
            Ok(c.0
                .new_layer(w, h, script)
                .map(|(i, id)| LayerRef(c.0.clone(), i, id, c.1.clone())))
        });
        m.add_method("update", |_, c, ()| {
            c.0.update();
            Ok(())
        });
        m.add_method("width", |_, _, ()| Ok(WIDTH));
        m.add_method("height", |_, _, ()| Ok(HEIGHT));
        m.add_method("screenWidth", |_, _, ()| Ok(WIDTH));
        m.add_method("screenHeight", |_, _, ()| Ok(HEIGHT));
    }
}

/// Set the `canvas` and `image` globals up over `canvas`.
pub(crate) fn install(lua: &Lua, canvas: Rc<Canvas>, ctx: Ctx) -> mlua::Result<()> {
    let ctx = Rc::new(ctx);
    let g = lua.globals();
    g.set("canvas", CanvasRef(canvas, ctx.clone()))?;
    let image: Table = lua.create_table()?;
    let c = ctx.clone();
    image.set(
        "new",
        lua.create_function(move |_, (w, h): (Num, Num)| {
            let (w, h) = (w.0 as u32, h.0 as u32);
            Ok((w < IMAGE_MAX && h < IMAGE_MAX)
                .then(|| ImageRef(Rc::new(RefCell::new(Image::new(w, h))), c.clone())))
        })?,
    )?;
    let c = ctx.clone();
    image.set(
        "load",
        lua.create_function(move |lua, p: String| {
            check(&c, lua, "image.load", &p)?;
            Ok(Image::load(Path::new(&p))
                .ok()
                .map(|i| ImageRef(Rc::new(RefCell::new(i)), c.clone())))
        })?,
    )?;
    let c = ctx;
    image.set(
        "newPainter",
        lua.create_function(move |_, i: AnyUserData| {
            let painter = Painter {
                image: image_of(&i)?,
                blend: false,
                fill: false,
                fill_color: 0,
                stroke_width: 0,
                stroke_color: 0,
                font: None,
            };
            Ok(PainterRef(RefCell::new(painter), c.clone()))
        })?,
    )?;
    g.set("image", image)?;
    // The BizHawk shim's: give a painter the built-in font (egui's Ubuntu
    // Light), as BizHawk's `gui.drawText` needs none loaded.
    g.set(
        "__builtinFont",
        lua.create_function(|_, p: AnyUserData| {
            let p = p.borrow::<PainterRef>()?;
            let f = FontVec::try_from_vec(epaint_default_fonts::UBUNTU_LIGHT.to_vec())
                .map_err(mlua::Error::external)?;
            p.0.borrow_mut().font = Some(Font { f, pt: 8.0 });
            Ok(())
        })?,
    )?;
    Ok(())
}
