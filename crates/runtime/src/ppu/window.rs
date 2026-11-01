//! Windows (GBATEK "LCD I/O Window Feature").
//!
//! Each pixel gets a set of enable bits: bits 0-3 show BG0-BG3, bit 4
//! sprites, bit 5 colour effects ([`super::blend`]). With no window on in
//! `DISPCNT` every bit is set. Otherwise the first window the pixel is in
//! decides, in the order WIN0, WIN1, the OBJ window, and outside:
//!
//! | register | bits  | enables for          |
//! |----------|-------|----------------------|
//! | `WININ`  | 0-5   | inside WIN0          |
//! | `WININ`  | 8-13  | inside WIN1          |
//! | `WINOUT` | 0-5   | outside every window |
//! | `WINOUT` | 8-13  | inside the OBJ window |
//!
//! WIN0 and WIN1 are rectangles: `WINnH` holds the left edge in its high
//! byte and the right edge (exclusive) in its low byte, `WINnV` the top and
//! bottom the same way. A start past the end wraps: the window covers from
//! the start to the screen's edge and from 0 up to the end. Edges past the
//! screen are clamped as mGBA clamps them (an end past the screen is the
//! screen's edge; a start past it and past the end is 0). The OBJ window
//! is every opaque pixel of a sprite in OBJ-window mode
//! ([`ObjLine::window`]).

use super::{ObjLine, dispcnt};
use crate::platform::{HEIGHT, WIDTH};
use crate::video::Video;

/// Every layer and the effects.
pub const ALL: u8 = 0x3F;
/// Bit 4: sprites.
pub const OBJ: u8 = 1 << 4;
/// Bit 5: colour effects.
pub const EFFECTS: u8 = 1 << 5;

/// The enable bits for each pixel of line `y`, given `DISPCNT` as `cnt`.
pub fn line(v: &Video, cnt: u16, y: usize, objs: &ObjLine, out: &mut [u8; WIDTH]) {
    let on = |bit: u16| cnt & bit != 0;
    if !on(dispcnt::WIN0) && !on(dispcnt::WIN1) && !on(dispcnt::OBJ_WIN) {
        out.fill(ALL);
        return;
    }
    let winin = v.winin;
    let winout = v.winout;
    out.fill(winout as u8 & ALL);
    // Drawn back to front, so WIN0 ends up on top.
    if on(dispcnt::OBJ_WIN) && on(dispcnt::OBJ) {
        for (e, &w) in out.iter_mut().zip(objs.window.iter()) {
            if w {
                *e = (winout >> 8) as u8 & ALL;
            }
        }
    }
    for n in [1, 0] {
        if !on(dispcnt::WIN0 << n) {
            continue;
        }
        let w = &v.win[n];
        let vs = span((w.top as u16) << 8 | w.bottom as u16, HEIGHT);
        if !inside(vs, y) {
            continue;
        }
        let h = span((w.left as u16) << 8 | w.right as u16, WIDTH);
        let bits = (winin >> (8 * n)) as u8 & ALL;
        for (x, e) in out.iter_mut().enumerate() {
            if inside(h, x) {
                *e = bits;
            }
        }
    }
}

/// A window edge register's start and end, clamped to a screen side of
/// `size` pixels.
fn span(r: u16, size: usize) -> (usize, usize) {
    let (mut start, mut end) = ((r >> 8) as usize, (r & 0xFF) as usize);
    if start > size && start > end {
        start = 0;
    }
    if end > size {
        end = size;
        start = start.min(size);
    }
    (start, end)
}

/// Whether `p` is in `[start, end)`, wrapping when the start is past the end.
fn inside((start, end): (usize, usize), p: usize) -> bool {
    if start <= end {
        start <= p && p < end
    } else {
        p >= start || p < end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_wrap_and_clamp() {
        assert!(inside(span(0x1020, WIDTH), 0x10));
        assert!(!inside(span(0x1020, WIDTH), 0x20));
        assert!(!inside(span(0x1010, WIDTH), 0x10));
        // Wrapping: 200 to the right edge, and 0 to 10.
        let s = span(200 << 8 | 10, WIDTH);
        assert!(inside(s, 239) && inside(s, 0) && inside(s, 9));
        assert!(!inside(s, 10) && !inside(s, 199));
        // An end past the screen is the edge.
        assert_eq!(span(100 << 8 | 250, WIDTH), (100, 240));
        assert_eq!(span(250 << 8 | 20, WIDTH), (0, 20));
        assert_eq!(span(170 << 8 | 200, HEIGHT), (160, 160));
    }
}
