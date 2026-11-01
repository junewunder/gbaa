extern crate std;

use super::*;
use crate::bus::Bus;
use crate::platform::WIDTH;
use crate::test_util::IO;
use crate::test_util::ppu::*;
use std::boxed::Box;

const BG2: u16 = dispcnt::BG0 << 2;

#[test]
fn kinds_by_mode() {
    assert_eq!(bg_kind(0, 3), Some(BgKind::Text));
    assert_eq!(bg_kind(1, 1), Some(BgKind::Text));
    assert_eq!(bg_kind(1, 2), Some(BgKind::Affine));
    assert_eq!(bg_kind(1, 3), None);
    assert_eq!(bg_kind(2, 0), None);
    assert_eq!(bg_kind(2, 3), Some(BgKind::Affine));
    for m in 3..=5 {
        assert_eq!(bg_kind(m, 2), Some(BgKind::Bitmap));
        assert_eq!(bg_kind(m, 0), None);
    }
    assert_eq!(bg_kind(6, 2), None);
}

#[test]
fn mode3_pixels() {
    let mut b = bus();
    b.write16(IO, 3 | BG2);
    b.write16(VRAM, 0x001F);
    // Bit 15 isn't part of the colour.
    b.write16(VRAM + ((159 * 240 + 239) * 2), 0xFC00);
    let p = render(&b);
    assert_eq!(px(&p, 0, 0), 0x001F);
    assert_eq!(px(&p, 239, 159), 0x7C00);
    assert_eq!(px(&p, 1, 0), 0);
}

#[test]
fn mode4_palette_page_and_backdrop() {
    let mut b = bus();
    b.write16(PAL, 0x1234);
    b.write16(PAL + 2 * 7, 0x03E0);
    // Page 0: index 7 at (1, 0); page 1: index 7 at (3, 0).
    b.write8(VRAM + 1, 7);
    b.write16(VRAM + bitmap::PAGE_1 as u32 + 2, 7 << 8);
    b.write16(IO, 4 | BG2);
    let p = render(&b);
    assert_eq!(px(&p, 1, 0), 0x03E0);
    // Index 0 is transparent: the backdrop shows.
    assert_eq!(px(&p, 3, 0), 0x1234);
    b.write16(IO, 4 | BG2 | dispcnt::PAGE);
    let p = render(&b);
    assert_eq!(px(&p, 1, 0), 0x1234);
    assert_eq!(px(&p, 3, 0), 0x03E0);
}

#[test]
fn mode5_size_and_page() {
    let mut b = bus();
    b.write16(PAL, 0x0042);
    let last = ((127 * 160 + 159) * 2) as u32;
    b.write16(VRAM + last, 0x7FFF);
    b.write16(VRAM + bitmap::PAGE_1 as u32, 0x0001);
    b.write16(IO, 5 | BG2);
    let p = render(&b);
    assert_eq!(px(&p, 159, 127), 0x7FFF);
    assert_eq!(px(&p, 160, 127), 0x0042);
    assert_eq!(px(&p, 0, 128), 0x0042);
    b.write16(IO, 5 | BG2 | dispcnt::PAGE);
    let p = render(&b);
    assert_eq!(px(&p, 0, 0), 0x0001);
}

/// Write BG2's matrix and 28-bit reference point.
fn bg2_affine(b: &mut Bus, m: [i16; 4], x: i32, y: i32) {
    for (i, v) in m.into_iter().enumerate() {
        b.write16(IO + 0x20 + 2 * i as u32, v as u16);
    }
    for (o, v) in [(0x28, x), (0x2C, y)] {
        b.write16(IO + o, v as u16);
        b.write16(IO + o + 2, (v >> 16) as u16 & 0x0FFF);
    }
}

#[test]
fn mode3_scaled_through_bg2s_matrix() {
    let mut b = bus();
    b.write16(PAL, 0x0042);
    b.write16(VRAM + (10 * 240 + 20) * 2, 0x001F);
    // Half a bitmap pixel per screen pixel, from bitmap (10, 5).
    bg2_affine(&mut b, [0x80, 0, 0, 0x80], 10 << 8, 5 << 8);
    b.write16(IO, 3 | BG2);
    let p = render(&b);
    // Screen (20..22, 10..12) is bitmap (20, 10).
    assert_eq!(px(&p, 20, 10), 0x001F);
    assert_eq!(px(&p, 21, 11), 0x001F);
    assert_eq!(px(&p, 22, 10), 0);
    assert_eq!(px(&p, 19, 10), 0);
    // Bitmap x 240 and up is outside: transparent, even with the wrap bit
    // (which would show bitmap (0, 0) there).
    b.write16(VRAM, 0x7C00);
    b.write16(VRAM + 239 * 2, 0x03E0);
    bg2_affine(&mut b, [0x100, 0, 0, 0x100], 200 << 8, 0);
    b.write16(IO + 0xC, 1 << 13);
    let p = render(&b);
    assert_eq!(px(&p, 39, 0), 0x03E0);
    assert_eq!(px(&p, 40, 0), 0x0042);
}

#[test]
fn mode4_reference_point_and_bounds() {
    let mut b = bus();
    b.write16(PAL, 0x0042);
    b.write16(PAL + 2, 0x03E0);
    b.write16(VRAM + bitmap::PAGE_1 as u32, 0x0101);
    // Bitmap (0, 0) at screen (5, 3): the point starts at (-5, -3).
    bg2_affine(&mut b, [0x100, 0, 0, 0x100], -5 << 8, -3 << 8);
    b.write16(IO, 4 | BG2 | dispcnt::PAGE);
    let p = render(&b);
    assert_eq!(px(&p, 5, 3), 0x03E0);
    assert_eq!(px(&p, 6, 3), 0x03E0);
    assert_eq!(px(&p, 4, 3), 0x0042);
    assert_eq!(px(&p, 5, 2), 0x0042);
}

#[test]
fn mode5_rotated_and_clipped_to_its_size() {
    let mut b = bus();
    b.write16(PAL, 0x0042);
    b.write16(VRAM + (2 * 160 + 159) * 2, 0x7FFF);
    b.write16(VRAM + (127 * 160 + 3) * 2, 0x001F);
    // 90 degrees: bitmap x follows screen y, bitmap y follows screen -x,
    // starting at bitmap (0, 127.x).
    bg2_affine(&mut b, [0, 0x100, -0x100, 0], 0, (127 << 8) | 0x80);
    b.write16(IO, 5 | BG2);
    let p = render(&b);
    // Screen (x, y) is bitmap (y, 127 - x).
    assert_eq!(px(&p, 125, 159), 0x7FFF);
    assert_eq!(px(&p, 0, 3), 0x001F);
    assert_eq!(px(&p, 1, 3), 0);
    // Screen x 128 is bitmap y -1: outside, so the backdrop.
    assert_eq!(px(&p, 128, 3), 0x0042);
}

#[test]
fn bg2_off_and_forced_blank() {
    let mut b = bus();
    b.write16(PAL, 0x0042);
    b.write16(VRAM, 0x001F);
    b.write16(IO, 3);
    assert_eq!(px(&render(&b), 0, 0), 0x0042);
    b.write16(IO, 3 | BG2 | dispcnt::FORCED_BLANK);
    assert!(render(&b).frame.iter().all(|&c| c == WHITE));
}

#[test]
fn single_line() {
    let mut b = bus();
    b.write16(IO, 3 | BG2);
    b.write16(VRAM + 5 * 240 * 2, 0x001F);
    let mut p = Box::new(Ppu::new());
    // BG2's reference point counts lines from line 0, so draw up to 5.
    for y in 0..=5 {
        p.render_line(&video(&b), y);
    }
    assert_eq!(px(&p, 0, 5), 0x001F);
    assert_eq!(px(&p, 0, 4), 0);
    assert_eq!(px(&p, 0, 6), 0);
}

#[test]
fn compose_by_priority_then_bg_number() {
    let mut lines = [[TRANSPARENT; WIDTH]; 4];
    lines[0][0] = 1;
    lines[1][0] = 2;
    lines[1][1] = 2;
    let mut order = [Layer { bg: 1, priority: 0 }, Layer { bg: 0, priority: 1 }];
    let mut row = [0; WIDTH];
    let blend = Blend::new(&video(&bus()));
    compose(
        &mut row,
        &lines,
        &order,
        &ObjLine::new(),
        &[window::ALL; WIDTH],
        &blend,
        9,
    );
    assert_eq!(&row[..3], &[2, 2, 9]);
    order[0].priority = 1;
    order.sort_unstable_by_key(|l| (l.priority, l.bg));
    compose(
        &mut row,
        &lines,
        &order,
        &ObjLine::new(),
        &[window::ALL; WIDTH],
        &blend,
        9,
    );
    assert_eq!(&row[..3], &[1, 2, 9]);
}

/// BG0 a red 4bpp square over the top-left 64x64 pixels and BG1 blue over
/// the whole screen (priority 1 each, so BG0 is in front), backdrop grey.
fn two_bgs() -> Box<Bus> {
    let mut b = bus();
    hide_objs(&mut b);
    b.write16(PAL, 0x4210);
    b.write16(PAL + 2, 0x001F);
    b.write16(PAL + 4, 0x7C00);
    tile4(&mut b, VRAM + 32, 1);
    tile4(&mut b, VRAM + 64, 2);
    for ty in 0..8 {
        fill16(&mut b, VRAM + 30 * 0x800 + ty * 64, 8, 1);
    }
    fill16(&mut b, VRAM + 31 * 0x800, 32 * 32, 2);
    b.write16(IO + 0x8, 30 << 8 | 1);
    b.write16(IO + 0xA, 31 << 8 | 1);
    b.write16(
        IO,
        dispcnt::BG0 | dispcnt::BG0 << 1 | dispcnt::OBJ | dispcnt::OBJ_1D,
    );
    b
}

#[test]
fn windows_pick_layers_win0_first() {
    let mut b = two_bgs();
    let cnt = b.io.dispcnt();
    // WIN0 (10..20, 10..20) shows BG1 only; WIN1 (15..40, 15..40) BG0
    // only; outside shows nothing but the backdrop.
    b.write16(IO + 0x40, 10 << 8 | 20);
    b.write16(IO + 0x44, 10 << 8 | 20);
    b.write16(IO + 0x42, 15 << 8 | 40);
    b.write16(IO + 0x46, 15 << 8 | 40);
    b.write16(IO + 0x48, 1 << 8 | 2);
    b.write16(IO + 0x4A, 0);
    b.write16(IO, cnt | dispcnt::WIN0 | dispcnt::WIN1);
    let p = render(&b);
    assert_eq!(px(&p, 10, 10), 0x7C00);
    assert_eq!(px(&p, 19, 19), 0x7C00);
    assert_eq!(px(&p, 20, 19), 0x001F);
    assert_eq!(px(&p, 39, 39), 0x001F);
    assert_eq!(px(&p, 40, 39), 0x4210);
    assert_eq!(px(&p, 5, 5), 0x4210);
    // WIN0 off: WIN1 alone.
    b.write16(IO, cnt | dispcnt::WIN1);
    assert_eq!(px(&render(&b), 15, 15), 0x001F);
}

#[test]
fn obj_window_and_sprites_hidden_by_windows() {
    let mut b = two_bgs();
    let cnt = b.io.dispcnt();
    b.write16(OBJ_PAL + 2, 0x03E0);
    tile4(&mut b, OBJ_VRAM + 32, 1);
    // Sprite 0 an OBJ-window 8x8 at (100, 100), sprite 1 a normal one at
    // (0, 0) under WIN0 (0..4, 0..4).
    b.write16(OAM, 100 | 2 << 10);
    b.write16(OAM + 2, 100);
    b.write16(OAM + 4, 1);
    b.write16(OAM + 8, 0);
    b.write16(OAM + 10, 0);
    b.write16(OAM + 12, 1);
    b.write16(IO + 0x40, 4);
    b.write16(IO + 0x44, 4);
    // WIN0: BG0 only. Outside: BG1 and sprites. OBJ window: backdrop.
    b.write16(IO + 0x48, 1);
    b.write16(IO + 0x4A, 2 | 0x10);
    b.write16(IO, cnt | dispcnt::WIN0 | dispcnt::OBJ_WIN);
    let p = render(&b);
    assert_eq!(px(&p, 0, 0), 0x001F);
    assert_eq!(px(&p, 4, 4), 0x03E0);
    assert_eq!(px(&p, 100, 100), 0x4210);
    assert_eq!(px(&p, 108, 100), 0x7C00);
    // The OBJ window needs sprites on.
    b.write16(IO, (cnt & !dispcnt::OBJ) | dispcnt::OBJ_WIN);
    assert_eq!(px(&render(&b), 100, 100), 0x7C00);
}

#[test]
fn blending_takes_the_top_two_the_window_shows() {
    let mut b = two_bgs();
    let cnt = b.io.dispcnt();
    // Alpha, BG0 first, BG1 and backdrop second, 8/8.
    b.write16(IO + 0x50, 1 | 1 << 6 | 2 << 8 | 0x20 << 8);
    b.write16(IO + 0x52, 0x0808);
    let p = render(&b);
    assert_eq!(px(&p, 0, 0), 0x3C0F);
    assert_eq!(px(&p, 64, 0), 0x7C00);
    // WIN0 hides BG1 at (0..8, 0..8): BG0 over the backdrop.
    b.write16(IO + 0x40, 8);
    b.write16(IO + 0x44, 8);
    b.write16(IO + 0x48, 0x21);
    b.write16(IO + 0x4A, 0x3F);
    b.write16(IO, cnt | dispcnt::WIN0);
    let p = render(&b);
    assert_eq!(px(&p, 0, 0), 0x001F / 2 + (0x4210 & 0x7BDE) / 2);
    assert_eq!(px(&p, 8, 8), 0x3C0F);
    // Effects off in WIN0.
    b.write16(IO + 0x48, 0x01);
    assert_eq!(px(&render(&b), 0, 0), 0x001F);
}

#[test]
fn semi_transparent_sprite_blends_over_second_target() {
    let mut b = two_bgs();
    b.write16(OBJ_PAL + 2, 0x03E0);
    tile4(&mut b, OBJ_VRAM + 32, 1);
    // Semi-transparent 8x8 at (100, 0), priority 0, over BG1.
    b.write16(OAM, 1 << 10);
    b.write16(OAM + 2, 100);
    b.write16(OAM + 4, 1);
    // Brighten BG0 fully; BG1 second; EVA 4, EVB 12.
    b.write16(IO + 0x50, 1 | 2 << 6 | 2 << 8);
    b.write16(IO + 0x52, 12 << 8 | 4);
    b.write16(IO + 0x54, 16);
    let p = render(&b);
    assert_eq!(px(&p, 0, 0), 0x7FFF);
    assert_eq!(px(&p, 100, 0), (31 * 4 / 16) << 5 | (31 * 12 / 16) << 10);
}
