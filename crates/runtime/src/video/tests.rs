extern crate std;

use super::*;
use crate::io::Io;
use std::boxed::Box;

/// A few awkward values for every halfword of the display block.
const VALUES: [u16; 6] = [0, 0xFFFF, 0x8000, 0x0800, 0x1234, 0xFEDC];

/// Every offset of the display block but the beam's two.
fn offsets() -> impl Iterator<Item = u32> {
    (0..REGS_END)
        .step_by(2)
        .filter(|&o| o != io::reg::DISPSTAT && o != io::reg::VCOUNT)
}

#[test]
fn load_regs_keeps_every_stored_bit() {
    for v in VALUES {
        let mut io = Io::new();
        for o in offsets() {
            io.write16(o, v ^ o as u16);
        }
        let mut video = Box::new(Video::new());
        video.load_regs(&io);
        for o in offsets() {
            assert_eq!(video.raw16(o), io.raw16(o), "offset {o:#x}, value {v:#x}");
        }
    }
}

#[test]
fn writes_go_through_the_same_masks_as_io() {
    for v in VALUES {
        let mut io = Io::new();
        let mut video = Box::new(Video::new());
        video.load_regs(&io);
        for o in offsets() {
            io.write16(o, v ^ o as u16);
            video.write16(o, v ^ o as u16);
            io.write8(o + 1, v as u8);
            video.write8(o + 1, v as u8);
            assert_eq!(video.raw16(o), io.raw16(o), "offset {o:#x}, value {v:#x}");
            assert_eq!(video.read16(o), io.read16(o), "offset {o:#x}, value {v:#x}");
        }
    }
}

#[test]
fn power_on_matches_io() {
    let video = Box::new(Video::new());
    let io = Io::new();
    for o in offsets() {
        assert_eq!(video.raw16(o), io.raw16(o), "offset {o:#x}");
    }
}

#[test]
fn reference_points_sign_extend_from_bit_27() {
    let mut video = Box::new(Video::new());
    video.write16(0x028, 0x5678);
    video.write16(0x02A, 0x0812);
    assert_eq!(video.affine[0].x, 0xF812_5678_u32 as i32);
    video.write16(0x02A, 0x0712);
    assert_eq!(video.affine[0].x, 0x0712_5678);
    // The high half's top nibble isn't stored.
    video.write16(0x03E, 0xF001);
    assert_eq!(video.affine[1].y, 0x0001_0000);
}

#[test]
fn typed_fields_are_the_registers() {
    let mut video = Box::new(Video::new());
    video.dispcnt = DispCnt::MODE_1 | DispCnt::BG0 | DispCnt::OBJ | DispCnt::OBJ_1D;
    video.bg[2].cnt = BgCnt::new()
        .priority(1)
        .char_base(2)
        .screen_base(31)
        .size(3);
    video.bg[1].scroll_y = 0x1AB;
    video.win[1] = Window {
        left: 8,
        right: 200,
        top: 16,
        bottom: 150,
    };
    assert_eq!(video.raw16(0x000), 0x1141);
    assert_eq!(video.raw16(0x00C), 0xDF09);
    assert_eq!(video.raw16(0x016), 0x1AB);
    assert_eq!(video.raw16(0x042), 0x08C8);
    assert_eq!(video.raw16(0x046), 0x1096);
    assert_eq!(video.dispcnt.mode(), 1);
}

#[test]
fn memories_by_halfword_and_entry() {
    let mut video = Box::new(Video::new());
    video.set_color(257, 0x03E0);
    assert_eq!(video.palette[0x202..0x204], [0xE0, 0x03]);
    video.copy_vram16(Video::tile4(4, 1), &[0x1111; 16]);
    assert_eq!(video.vram16(OBJ_VRAM + 32), 0x1111);
    video.fill_vram16(Video::screen_block(31), 4, 7);
    assert_eq!(video.vram16(31 * 0x800 + 6), 7);
    let e = OamEntry {
        attr0: 1,
        attr1: 2,
        attr2: 3,
        affine: 4,
    };
    video.set_obj(5, e);
    assert_eq!(video.obj(5), e);
    assert_eq!(video.oam[40..48], [1, 0, 2, 0, 3, 0, 4, 0]);
}
