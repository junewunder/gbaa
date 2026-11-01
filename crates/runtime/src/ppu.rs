//! The picture processing unit: turns VRAM, palette, OAM and the display
//! registers into a [`Frame`], one scanline at a time.
//!
//! Each line is drawn in two steps:
//!
//! 1. **Layers.** Every background the mode has and `DISPCNT` enables is
//!    drawn into its own [`Line`] by [`Ppu::bg_line`], the one place that
//!    knows which kind of background BG `n` is in mode `m`. Pixels nothing
//!    covers are [`TRANSPARENT`].
//!    Sprites ([`obj`]) are drawn into an [`ObjLine`], which also keeps
//!    each pixel's priority.
//! 2. **Compose.** [`compose`] picks, for each pixel, the opaque layer with
//!    the lowest `BGxCNT` priority (then the lowest BG number), or the
//!    backdrop (palette entry 0). An opaque sprite pixel goes in front of
//!    any background with the same or a higher priority value. The
//!    windows ([`window`]) say which layers each pixel may show and
//!    whether colour effects ([`blend`]) apply there; an effect works on
//!    the top two of what's left.
//!
//! What's drawn so far (decision 13's order): the bitmap modes 3, 4 and 5
//! ([`bitmap`]), BG2 only, with its affine transform, the text
//! backgrounds of modes 0 and 1 ([`bg_text`]), the affine backgrounds of
//! modes 1 and 2 ([`bg_affine`]), and regular and affine sprites
//! ([`obj`]), with windows and blending, and mosaic ([`mosaic`]) for all
//! of them.
//!
//! Drawing per line means a runner that calls [`Ppu::render_line`] at each
//! line's H-blank sees mid-frame register changes the way hardware does;
//! [`Ppu::render_frame`] draws all 160 lines from the current state, which
//! is what a runner that renders only at V-blank calls.

mod bg_affine;
mod bg_text;
mod bitmap;
mod blend;
mod mosaic;
mod obj;
mod window;

pub use obj::ObjLine;

use crate::platform::{Frame, HEIGHT, WIDTH};
use crate::video::Video;
use blend::Blend;
use mosaic::Mosaic;

/// A pixel no layer covers. GBA colours use bits 0-14, so bit 15 is free.
pub const TRANSPARENT: u16 = 0x8000;
/// `BGxCNT` bit 6: the background is drawn in mosaic blocks.
const BG_MOSAIC: u16 = 1 << 6;
/// What a forced-blank line shows.
pub const WHITE: u16 = 0x7FFF;

/// One layer's pixels for one scanline.
pub type Line = [u16; WIDTH];

/// `DISPCNT` bits.
pub mod dispcnt {
    /// Bits 0-2: the background mode.
    pub const MODE: u16 = 7;
    /// Bit 4: which page modes 4 and 5 show.
    pub const PAGE: u16 = 1 << 4;
    /// Bit 5: H-blank is free for the CPU, leaving sprites fewer cycles.
    pub const HBLANK_FREE: u16 = 1 << 5;
    /// Bit 6: sprite tiles are mapped 1D rather than as a 2D sheet.
    pub const OBJ_1D: u16 = 1 << 6;
    /// Bit 7: forced blank, every line white.
    pub const FORCED_BLANK: u16 = 1 << 7;
    /// Bit 8 + `n`: BG `n` is on.
    pub const BG0: u16 = 1 << 8;
    /// Bit 12: sprites are on.
    pub const OBJ: u16 = 1 << 12;
    /// Bits 13 and 14: windows 0 and 1 are on.
    pub const WIN0: u16 = 1 << 13;
    pub const WIN1: u16 = 1 << 14;
    /// Bit 15: the OBJ window is on.
    pub const OBJ_WIN: u16 = 1 << 15;
}

/// What kind of background BG `n` is in a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BgKind {
    /// A tiled, scrolling background (modes 0 and 1).
    Text,
    /// A rotated and scaled tiled background (modes 1 and 2).
    Affine,
    /// The full-screen bitmap of modes 3-5 (BG2 only).
    Bitmap,
}

/// BG `n`'s kind in `mode`, or `None` if the mode has no such background
/// (modes 6 and 7 have none).
pub fn bg_kind(mode: u16, n: usize) -> Option<BgKind> {
    match (mode, n) {
        (0, 0..=3) | (1, 0 | 1) => Some(BgKind::Text),
        (1, 2) | (2, 2 | 3) => Some(BgKind::Affine),
        (3..=5, 2) => Some(BgKind::Bitmap),
        _ => None,
    }
}

/// The PPU: the frame being drawn, and the state it carries between
/// lines, the affine backgrounds' reference points.
pub struct Ppu {
    pub frame: Frame,
    refs: bg_affine::Refs,
    /// Off: lines aren't drawn, [`Ppu::frame`] keeps what it had, and the
    /// reference points still move. Nothing the guest sees changes:
    /// rollback's frames run again, which nobody is shown. Not saved.
    pub draw: bool,
}

impl Default for Ppu {
    fn default() -> Ppu {
        Ppu::new()
    }
}

impl Ppu {
    pub fn new() -> Ppu {
        Ppu {
            frame: [0; WIDTH * HEIGHT],
            refs: bg_affine::Refs::default(),
            draw: true,
        }
    }

    /// Back to [`Ppu::new`]'s state, in place (`HardReset`).
    pub fn reset(&mut self) {
        self.frame.fill(0);
        self.refs = bg_affine::Refs::default();
    }

    /// Draw all 160 lines from the display's current state.
    pub fn render_frame(&mut self, v: &Video) {
        for y in 0..HEIGHT {
            self.render_line(v, y);
        }
    }

    /// Draw line `y` (0-159) into [`Ppu::frame`]. The play build passes
    /// [`crate::Bus::video_for_draw`]; a native program its own
    /// [`Video`] ([`crate::hw`]).
    pub fn render_line(&mut self, v: &Video, y: usize) {
        // The reference points move on every line, drawn or not.
        let refs = self.refs.line(v, y);
        if !self.draw {
            return;
        }
        let row: &mut Line = (&mut self.frame[y * WIDTH..(y + 1) * WIDTH])
            .try_into()
            .unwrap();
        let cnt = v.dispcnt.0;
        if cnt & dispcnt::FORCED_BLANK != 0 {
            row.fill(WHITE);
            return;
        }
        let mode = cnt & dispcnt::MODE;
        let mut lines = [[TRANSPARENT; WIDTH]; 4];
        let mut layers = [Layer::default(); 4];
        let mut count = 0;
        let mosaic = Mosaic::new(v);
        for (n, line) in lines.iter_mut().enumerate() {
            if cnt & (dispcnt::BG0 << n) == 0 {
                continue;
            }
            let Some(kind) = bg_kind(mode, n) else {
                continue;
            };
            if Self::bg_line(v, n, kind, y, refs, &mosaic, line) {
                layers[count] = Layer {
                    bg: n,
                    priority: v.bg[n].cnt.get_priority(),
                };
                count += 1;
            }
        }
        let layers = &mut layers[..count];
        layers.sort_unstable_by_key(|l| (l.priority, l.bg));
        let mut objs = ObjLine::new();
        if cnt & dispcnt::OBJ != 0 {
            obj::line(v, cnt, y, &mosaic, &mut objs);
        }
        let mut enables = [0; WIDTH];
        window::line(v, cnt, y, &objs, &mut enables);
        let blend = Blend::new(v);
        compose(row, &lines, layers, &objs, &enables, &blend, backdrop(v));
    }

    /// Draw BG `n`, a background of `kind`, for line `y` into `out` (which
    /// starts all [`TRANSPARENT`]); `refs` are BG2's and BG3's reference
    /// points for the line, and `mosaic` the block sizes if `BGxCNT` asks
    /// for them. Returns false if nothing was drawn, so the compose step
    /// can skip the layer.
    fn bg_line(
        v: &Video,
        n: usize,
        kind: BgKind,
        y: usize,
        refs: [(i32, i32); 2],
        mosaic: &Mosaic,
        out: &mut Line,
    ) -> bool {
        debug_assert!(kind != BgKind::Bitmap || n == 2);
        let on = v.bg[n].cnt.0 & BG_MOSAIC != 0;
        // How many lines back the mosaic block starts.
        let back = if on { y % mosaic.bg_h } else { 0 };
        match kind {
            BgKind::Bitmap => {
                bitmap::line(v, bg_affine::step_back(v, 2, refs[0], back), out);
            }
            BgKind::Text => {
                bg_text::line(v, n, y - back, out);
            }
            BgKind::Affine => {
                let start = bg_affine::step_back(v, n, refs[n - 2], back);
                bg_affine::line(v, n, start, out);
            }
        }
        if on {
            mosaic::hold(out, mosaic.bg_w);
        }
        true
    }
}

/// A background drawn on this line, with its priority.
#[derive(Clone, Copy, Debug, Default)]
struct Layer {
    bg: usize,
    priority: u16,
}

/// The backdrop: BG palette entry 0.
fn backdrop(v: &Video) -> u16 {
    v.color(0) & 0x7FFF
}

/// For each pixel, of the layers `enables` lets it show: the first opaque
/// background in `order` (front to back), with an opaque sprite pixel in
/// front of it unless its priority value is lower than the sprite's, then
/// the backdrop. `blend` mixes the top two, where `enables` allows effects
/// (and for semi-transparent sprites, everywhere).
fn compose(
    row: &mut Line,
    lines: &[Line; 4],
    order: &[Layer],
    objs: &ObjLine,
    enables: &[u8; WIDTH],
    blend: &Blend,
    backdrop: u16,
) {
    for (x, px) in row.iter_mut().enumerate() {
        let en = enables[x];
        // The top two (colour, target bit) pairs; what no layer fills is
        // the backdrop.
        let mut top = [(backdrop, blend::BACKDROP); 2];
        let mut n = 0;
        let obj = objs.colour[x];
        let mut obj_left = obj != TRANSPARENT && en & window::OBJ != 0;
        for l in order {
            if n == 2 {
                break;
            }
            let c = lines[l.bg][x];
            if c == TRANSPARENT || en & (1 << l.bg) == 0 {
                continue;
            }
            if obj_left && l.priority >= objs.priority[x] as u16 {
                top[n] = (obj, blend::OBJ);
                n += 1;
                obj_left = false;
                if n == 2 {
                    break;
                }
            }
            top[n] = (c, l.bg as u8);
            n += 1;
        }
        if obj_left && n < 2 {
            top[n] = (obj, blend::OBJ);
        }
        let [(c, layer), (under, under_layer)] = top;
        let semi = layer == blend::OBJ && objs.semi[x];
        let effects = en & window::EFFECTS != 0;
        *px = blend.apply(c, layer, semi, under, under_layer, effects);
    }
}

crate::snap::snap!(Ppu { frame, refs } ; skip draw);

impl Ppu {
    /// The state without the picture ([`Machine::save_unseen`]).
    ///
    /// [`Machine::save_unseen`]: crate::Machine::save_unseen
    pub(crate) fn snap_unseen(&mut self, v: &mut dyn crate::snap::Visitor) {
        crate::snap::Snap::snap(&mut self.refs, v);
    }
}

#[cfg(test)]
mod tests;
