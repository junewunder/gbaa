//! Colour effects (GBATEK "LCD I/O Color Special Effects").
//!
//! `BLDCNT` picks the effect (bits 6-7: none, alpha blending, brighten,
//! darken), the first targets (bits 0-5: BG0-BG3, sprites, backdrop) and
//! the second targets (bits 8-13, the same order). Where the window lets
//! effects through and the top pixel is a first target:
//!
//! - alpha blending mixes it with the pixel beneath it, if that is a
//!   second target: per 5-bit channel, `min(31, (top * EVA + under * EVB)
//!   / 16)`, with EVA and EVB from `BLDALPHA` bits 0-4 and 8-12;
//! - brighten moves it toward white, `c + (31 - c) * EVY / 16`, and darken
//!   toward black, `c - c * EVY / 16`, with EVY from `BLDY` bits 0-4.
//!
//! Every coefficient counts as 16 above 16, and division rounds down.
//!
//! A semi-transparent sprite's pixel over a second target is alpha-blended
//! whatever the effect and the first targets are, and even where the
//! window turns effects off (as mGBA does); over anything else the usual
//! rule applies to it.

use crate::video::Video;

/// Target bit of the sprite layer and of the backdrop (BG `n` is bit `n`).
pub const OBJ: u8 = 4;
pub const BACKDROP: u8 = 5;

/// `BLDCNT`, `BLDALPHA` and `BLDY`, decoded.
#[derive(Clone, Copy, Debug)]
pub struct Blend {
    first: u8,
    second: u8,
    mode: u16,
    eva: u16,
    evb: u16,
    evy: u16,
}

impl Blend {
    pub fn new(v: &Video) -> Blend {
        let cnt = v.bldcnt;
        let alpha = v.bldalpha;
        let coef = |v: u16| (v & 0x1F).min(16);
        Blend {
            first: cnt as u8 & 0x3F,
            second: (cnt >> 8) as u8 & 0x3F,
            mode: (cnt >> 6) & 3,
            eva: coef(alpha),
            evb: coef(alpha >> 8),
            evy: coef(v.bldy),
        }
    }

    /// The colour shown for `top` (from layer `top_layer`, a target bit
    /// number) over `under` (from `under_layer`); `semi` if `top` is a
    /// semi-transparent sprite's, `effects` if the window allows effects.
    pub fn apply(
        &self,
        top: u16,
        top_layer: u8,
        semi: bool,
        under: u16,
        under_layer: u8,
        effects: bool,
    ) -> u16 {
        let second = self.second & (1 << under_layer) != 0;
        if semi && second {
            return mix(top, self.eva, under, self.evb);
        }
        if !effects || self.first & (1 << top_layer) == 0 {
            return top;
        }
        match self.mode {
            1 if second => mix(top, self.eva, under, self.evb),
            2 => channels(top, |c| c + (31 - c) * self.evy / 16),
            3 => channels(top, |c| c - c * self.evy / 16),
            _ => top,
        }
    }
}

/// `a * wa + b * wb`, divided by 16 and capped at 31, per channel.
fn mix(a: u16, wa: u16, b: u16, wb: u16) -> u16 {
    let mut out = 0;
    for shift in [0, 5, 10] {
        let ca = (a >> shift) & 0x1F;
        let cb = (b >> shift) & 0x1F;
        out |= ((ca * wa + cb * wb) / 16).min(31) << shift;
    }
    out
}

/// `f` applied to each 5-bit channel of `c`.
fn channels(c: u16, f: impl Fn(u16) -> u16) -> u16 {
    let mut out = 0;
    for shift in [0, 5, 10] {
        out |= f((c >> shift) & 0x1F) << shift;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::reg;

    fn blend(cnt: u16, alpha: u16, y: u16) -> Blend {
        let mut b = crate::test_util::ppu::bus();
        b.io.set_raw16(reg::BLDCNT, cnt);
        b.io.set_raw16(reg::BLDALPHA, alpha);
        b.io.set_raw16(reg::BLDY, y);
        Blend::new(&crate::test_util::ppu::video(&b))
    }

    #[test]
    fn alpha_needs_both_targets_and_caps_at_31() {
        // BG0 first, BG1 second, alpha, EVA 8, EVB 8.
        let b = blend(1 | 1 << 6 | 2 << 8, 0x0808, 0);
        assert_eq!(b.apply(0x001F, 0, false, 0x7C00, 1, true), 0x3C0F);
        assert_eq!(b.apply(0x001F, 0, false, 0x7C00, 2, true), 0x001F);
        assert_eq!(b.apply(0x001F, 1, false, 0x7C00, 1, true), 0x001F);
        // EVA 16 and EVB 31 (counts as 16): saturates.
        let b = blend(1 | 1 << 6 | 2 << 8, 0x1F10, 0);
        assert_eq!(b.apply(0x0010, 0, false, 0x0018, 1, true), 0x001F);
    }

    #[test]
    fn brighten_and_darken() {
        let b = blend(1 | 2 << 6, 0, 8);
        assert_eq!(
            b.apply(0x0000, 0, false, 0, 5, true),
            15 | 15 << 5 | 15 << 10
        );
        assert_eq!(b.apply(0x0000, 1, false, 0, 5, true), 0);
        let b = blend(1 | 3 << 6, 0, 20);
        assert_eq!(b.apply(0x7FFF, 0, false, 0, 5, true), 0);
        let b = blend(1 | 3 << 6, 0, 4);
        assert_eq!(b.apply(0x001F, 0, false, 0, 5, true), 24);
    }

    #[test]
    fn semi_transparent_sprites_blend_regardless() {
        // No effect, no first targets; BG0 second.
        let b = blend(1 << 8, 0x0808, 0);
        assert_eq!(b.apply(0x001F, OBJ, true, 0x7C00, 0, true), 0x3C0F);
        assert_eq!(b.apply(0x001F, OBJ, true, 0x7C00, 1, true), 0x001F);
        // Effects off in the window: still blended.
        assert_eq!(b.apply(0x001F, OBJ, true, 0x7C00, 0, false), 0x3C0F);
        // Not over a second target: the usual effect.
        let b = blend(1 << OBJ | 2 << 6 | 1 << 8, 0, 16);
        assert_eq!(b.apply(0x0000, OBJ, true, 0, 1, true), 0x7FFF);
    }
}
