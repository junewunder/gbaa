//! Helpers shared by the runtime's tests.

extern crate std;

use crate::Machine;
use crate::platform::Headless;
use std::boxed::Box;
use std::vec::Vec;

pub const IO: u32 = 0x0400_0000;

/// A machine whose ROM is `code` (ARM words), at the cartridge's entry.
pub fn machine_with(code: &[u32]) -> Box<Machine<Headless>> {
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let rom: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    Box::new(Machine::new(rom, Headless))
}

/// A machine spinning on `b .`.
pub fn machine() -> Box<Machine<Headless>> {
    machine_with(&[0xEAFF_FFFE])
}

/// PPU unit tests: a bare bus, a rendered frame, and fills that store
/// halfwords only. A byte store to OAM is dropped, one to OBJ VRAM (and to
/// BG VRAM past its end in bitmap modes) is dropped, and one to palette or
/// BG VRAM writes the byte to both halves; so set up VRAM, palette and OAM
/// through these, or `write16`, never `write8`.
pub mod ppu {
    use super::Box;
    use crate::bus::Bus;
    use crate::platform::WIDTH;
    use crate::ppu::Ppu;
    use crate::video::Video;

    pub const PAL: u32 = 0x0500_0000;
    pub const OBJ_PAL: u32 = PAL + 0x200;
    pub const VRAM: u32 = 0x0600_0000;
    pub const OBJ_VRAM: u32 = VRAM + 0x1_0000;
    pub const OAM: u32 = 0x0700_0000;

    /// A bus with no ROM; VRAM, palette and OAM zeroed.
    pub fn bus() -> Box<Bus> {
        Box::new(Bus::new(&[]))
    }

    /// The frame `b`'s current state draws.
    pub fn render(b: &Bus) -> Box<Ppu> {
        let mut p = Box::new(Ppu::new());
        p.render_frame(&video(b));
        p
    }

    /// `b`'s display as the PPU sees it: its memories, its registers
    /// decoded from its I/O block.
    pub fn video(b: &Bus) -> Box<Video> {
        let mut v = Box::new(b.video.clone());
        v.load_regs(&b.io);
        v
    }

    /// The pixel at `(x, y)`, BGR555.
    pub fn px(p: &Ppu, x: usize, y: usize) -> u16 {
        p.frame[y * WIDTH + x]
    }

    /// `n` halfwords of `v` from `addr` (even) up.
    pub fn fill16(b: &mut Bus, addr: u32, n: u32, v: u16) {
        assert_eq!(addr & 1, 0, "halfword fill at odd address {addr:#x}");
        for i in 0..n {
            b.write16(addr + i * 2, v);
        }
    }

    /// `bytes` from `addr` (even) up, stored as little-endian halfwords; an
    /// odd count's last byte is paired with a zero.
    pub fn store_bytes(b: &mut Bus, addr: u32, bytes: &[u8]) {
        assert_eq!(addr & 1, 0, "halfword store at odd address {addr:#x}");
        for (i, pair) in bytes.chunks(2).enumerate() {
            let hi = pair.get(1).copied().unwrap_or(0);
            b.write16(addr + i as u32 * 2, u16::from_le_bytes([pair[0], hi]));
        }
    }

    /// The 4bpp tile (32 bytes) at `addr`, every pixel colour `c` (0-15).
    pub fn tile4(b: &mut Bus, addr: u32, c: u8) {
        fill16(b, addr, 16, (c & 0xF) as u16 * 0x1111);
    }

    /// The 8bpp tile (64 bytes) at `addr`, every pixel colour `c`.
    pub fn tile8(b: &mut Bus, addr: u32, c: u8) {
        fill16(b, addr, 32, c as u16 * 0x0101);
    }

    /// All 128 sprites hidden (OAM starts zeroed: 128 8x8 sprites at 0, 0).
    pub fn hide_objs(b: &mut Bus) {
        for n in 0..128 {
            b.write16(OAM + n * 8, 1 << 9);
        }
    }

    #[test]
    fn halfword_fills_reach_obj_vram_and_oam() {
        let mut b = bus();
        tile4(&mut b, OBJ_VRAM, 3);
        tile8(&mut b, OBJ_VRAM + 32, 0xAB);
        store_bytes(&mut b, OAM + 8, &[0x12, 0x34, 0x56]);
        assert_eq!(b.read16(OBJ_VRAM), 0x3333);
        assert_eq!(b.read16(OBJ_VRAM + 30), 0x3333);
        assert_eq!(b.read16(OBJ_VRAM + 32 + 62), 0xABAB);
        assert_eq!(b.read16(OAM + 8), 0x3412);
        assert_eq!(b.read16(OAM + 10), 0x0056);
        // The trap these exist for: a byte store there is dropped.
        b.write8(OBJ_VRAM + 128, 0x11);
        b.write8(OAM + 16, 0x11);
        assert_eq!(b.read16(OBJ_VRAM + 128), 0);
        assert_eq!(b.read16(OAM + 16), 0);
    }
}
