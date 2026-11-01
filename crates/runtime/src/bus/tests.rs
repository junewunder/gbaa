extern crate std;

use super::*;
use crate::io::{event, irq, key, reg};
use crate::{Machine, platform::Headless};
use std::boxed::Box;
use std::vec::Vec;

const IO: u32 = 0x0400_0000;

/// A ROM whose halfword at each offset is the offset's low bits plus a tag,
/// so mirrors are distinguishable from out-of-range reads.
fn rom(len: usize) -> &'static [u8] {
    let v: Vec<u8> = (0..len).map(|i| (i as u8) ^ 0xA5).collect();
    Box::leak(v.into_boxed_slice())
}

fn bus() -> Box<Bus> {
    Box::new(Bus::new(rom(0x100)))
}

#[test]
fn ram_mirrors() {
    let mut b = bus();
    b.write32(0x0200_0000, 0x1122_3344);
    assert_eq!(b.read32(0x0204_0000), 0x1122_3344);
    assert_eq!(b.read32(0x02FC_0000), 0x1122_3344);
    b.write32(0x0300_7FFC, 0xCAFE_F00D);
    assert_eq!(b.read32(0x0300_FFFC), 0xCAFE_F00D);
    assert_eq!(b.read32(0x03FF_FFFC), 0xCAFE_F00D);
    b.write16(0x0500_0010, 0x7FFF);
    assert_eq!(b.read16(0x0500_0410), 0x7FFF);
    b.write32(0x0700_0000, 1);
    assert_eq!(b.read32(0x0700_0400), 1);
    // Narrow reads and writes through the mirrors.
    b.write8(0x0204_0005, 0x77);
    assert_eq!(b.read8(0x0200_0005), 0x77);
    assert_eq!(b.read16(0x0208_0004), 0x7700);
    assert_eq!(b.read16(0x0300_FFFE), 0xCAFE);
    assert_eq!(b.read8(0x0700_0400), 1);
}

#[test]
fn vram_mirrors() {
    let mut b = bus();
    // 128 KB mirror.
    b.write32(0x0600_0000, 2);
    assert_eq!(b.read32(0x0602_0000), 2);
    // The last 32 KB mirror 0x10000-0x17FFF in the tiled modes.
    b.write32(0x0601_0000, 3);
    assert_eq!(b.read32(0x0601_8000), 3);
    b.write32(0x0601_C004, 4);
    assert_eq!(b.read32(0x0601_4004), 4);
    // In the bitmap modes 0x18000-0x1BFFF is dead; 0x1C000 still mirrors.
    b.write16(IO + reg::DISPCNT, 3);
    assert_eq!(b.read32(0x0601_8000), 0);
    b.write32(0x0601_8000, 9);
    assert_eq!(b.read32(0x0601_0000), 3);
    assert_eq!(b.read32(0x0601_C004), 4);
    b.write32(0x0601_C008, 5);
    assert_eq!(b.read32(0x0601_4008), 5);
}

#[test]
fn rom_mirrors_and_past_the_end() {
    let b = bus();
    let first = b.read32(0x0800_0000);
    assert_eq!(first, u32::from_le_bytes([0xA5, 0xA4, 0xA7, 0xA6]));
    assert_eq!(b.read32(0x0A00_0000), first);
    assert_eq!(b.read32(0x0C00_0000), first);
    assert_eq!(b.read8(0x0C00_0001), 0xA4);
    // 0x0D is the upper 16 MB, past this image.
    assert_eq!(b.read16(0x0D00_0010), 0x0008);
    // Past the image: the halfword address.
    assert_eq!(b.read16(0x0800_0200), 0x100);
    assert_eq!(b.read32(0x0800_0200), 0x0101_0100);
    assert_eq!(b.read8(0x0800_0201), 0x01);
}

#[test]
fn rom_and_bios_ignore_writes() {
    let mut b = bus();
    let before = b.read32(0x0800_0000);
    b.write32(0x0800_0000, 0);
    b.write8(0x0800_0000, 0);
    assert_eq!(b.read32(0x0800_0000), before);
    b.write32(0, 0);
    assert_eq!(b.read32(0), BIOS_LATCH_AFTER_BOOT);
    b.bios_latch = BIOS_LATCH_AFTER_SWI;
    assert_eq!(b.read32(0x3FFC), BIOS_LATCH_AFTER_SWI);
    assert_eq!(b.read16(2), (BIOS_LATCH_AFTER_SWI >> 16) as u16);
    assert_eq!(b.read8(1), (BIOS_LATCH_AFTER_SWI >> 8) as u8);
}

#[test]
fn widths_are_little_endian() {
    let mut b = bus();
    b.write32(0x0300_0000, 0x4433_2211);
    assert_eq!(b.read8(0x0300_0000), 0x11);
    assert_eq!(b.read8(0x0300_0003), 0x44);
    assert_eq!(b.read16(0x0300_0002), 0x4433);
    b.write8(0x0300_0001, 0xFF);
    b.write16(0x0300_0002, 0xBEEF);
    assert_eq!(b.read32(0x0300_0000), 0xBEEF_FF11);
    // Misaligned addresses are aligned down.
    assert_eq!(b.read32(0x0300_0003), 0xBEEF_FF11);
    assert_eq!(b.read16(0x0300_0003), 0xBEEF);
}

#[test]
fn video_byte_stores() {
    let mut b = bus();
    // Palette: the byte goes to both halves.
    b.write8(0x0500_0021, 1);
    assert_eq!(b.read16(0x0500_0020), 0x0101);
    // OAM: ignored.
    b.write8(0x0700_0010, 1);
    assert_eq!(b.read32(0x0700_0010), 0);
    // VRAM, tiled modes: BG (below 0x10000) as a halfword, OBJ ignored.
    b.write8(0x0600_0010, 2);
    assert_eq!(b.read16(0x0600_0010), 0x0202);
    b.write8(0x0601_0010, 2);
    assert_eq!(b.read32(0x0601_0010), 0);
    // Bitmap modes: BG reaches 0x14000.
    b.write16(IO + reg::DISPCNT, 3);
    b.write8(0x0601_0010, 2);
    assert_eq!(b.read16(0x0601_0010), 0x0202);
    b.write8(0x0601_4010, 2);
    assert_eq!(b.read32(0x0601_4010), 0);
}

#[test]
fn sram_is_an_8_bit_bus() {
    let mut b = bus();
    assert_eq!(b.read8(0x0E00_0000), 0xFF);
    b.write8(0x0E00_0000, 0x12);
    assert_eq!(b.read16(0x0E00_0000), 0x1212);
    assert_eq!(b.read32(0x0E00_0000), 0x1212_1212);
    assert_eq!(b.read8(0x0E01_0000), 0x12);
    assert_eq!(b.read8(0x0F00_0000), 0x12);
    // Wider stores keep the addressed byte lane. (The interpreter's stores
    // arrive aligned, so for it that's always the low byte.)
    b.write32(0x0E00_0001, 0x4433_2211);
    assert_eq!(b.read8(0x0E00_0001), 0x22);
    b.write16(0x0E00_0003, 0x4433);
    assert_eq!(b.read8(0x0E00_0003), 0x44);
}

#[test]
fn unmapped_reads_open_bus() {
    let mut b = bus();
    b.open_bus = 0xDEAD_BEEF;
    assert_eq!(b.read32(0x0100_0000), 0xDEAD_BEEF);
    assert_eq!(b.read16(0x1000_0002), 0xDEAD);
    assert_eq!(b.read8(0x0000_4001), 0xBE);
    assert_eq!(b.read32(IO + 0x400), 0xDEAD_BEEF);
    b.write32(0x0100_0000, 0);
    b.write32(IO + 0x400, 0);
    assert_eq!(b.read32(0x0100_0000), 0xDEAD_BEEF);
    assert_eq!(b.io.events, 0);
}

#[test]
fn io_starts_where_the_bios_leaves_it() {
    let b = bus();
    for (o, v) in [
        (reg::DISPCNT, 0),
        (reg::KEYINPUT, 0x03FF),
        (reg::POSTFLG, 1),
        (reg::SOUNDBIAS, 0x200),
        (reg::RCNT, 0x8000),
    ] {
        assert_eq!(b.read16(IO + o), v, "{o:#x}");
    }
    // The write-only affine matrices are the identity.
    for o in [reg::BG2PA, reg::BG2PA + 6, reg::BG3PA, reg::BG3PA + 6] {
        assert_eq!(b.io.raw16(o), 0x100, "{o:#x}");
    }
    assert_eq!(b.io.raw16(reg::BG2PA + 2), 0);
}

/// Masks of the registers whose layout varies by channel, from GBATEK.
#[test]
fn io_masks_by_channel() {
    let mut b = bus();
    // (offset, readable, writable) per DMA channel: SAD high, DAD high,
    // count, control.
    let dma = [
        [
            (0x0B2, 0, 0x07FF),
            (0x0B6, 0, 0x07FF),
            (0x0B8, 0, 0x3FFF),
            (0x0BA, 0xF7E0, 0xF7E0),
        ],
        [
            (0x0BE, 0, 0x0FFF),
            (0x0C2, 0, 0x07FF),
            (0x0C4, 0, 0x3FFF),
            (0x0C6, 0xF7E0, 0xF7E0),
        ],
        [
            (0x0CA, 0, 0x0FFF),
            (0x0CE, 0, 0x07FF),
            (0x0D0, 0, 0x3FFF),
            (0x0D2, 0xF7E0, 0xF7E0),
        ],
        [
            (0x0D6, 0, 0x0FFF),
            (0x0DA, 0, 0x0FFF),
            (0x0DC, 0, 0xFFFF),
            (0x0DE, 0xFFE0, 0xFFE0),
        ],
    ];
    for (o, r, w) in dma.into_iter().flatten().chain([
        // Timer control, sound channels, POSTFLG/HALTCNT.
        (0x102, 0x00C7, 0x00C7),
        (0x10E, 0x00C7, 0x00C7),
        (0x062, 0xFFC0, 0xFFFF),
        (0x064, 0x4000, 0xC7FF),
        (0x072, 0xE000, 0xE0FF),
        (0x07C, 0x40FF, 0xC0FF),
        (reg::SOUNDCNT_H, 0x770F, 0x770F),
        (reg::POSTFLG, 0x0001, 0xFF01),
    ]) {
        b.write16(IO + o, 0xFFFF);
        assert_eq!(b.read16(IO + o), r, "read {o:#x}");
        assert_eq!(b.io.raw16(o), w, "stored {o:#x}");
        b.io.set_raw16(o, 0);
    }
}

#[test]
fn io_byte_and_odd_accesses() {
    let mut b = bus();
    // The V-count target is DISPSTAT's high byte.
    b.write8(IO + reg::DISPSTAT + 1, 0x9F);
    assert_eq!(b.read8(IO + reg::DISPSTAT + 1), 0x9F);
    assert_eq!(b.read16(IO + reg::DISPSTAT), 0x9F00);
    // A halfword write to an odd address goes to the halfword below.
    b.write16(IO + reg::BLDALPHA + 1, 0x1F1F);
    assert_eq!(b.read16(IO + reg::BLDALPHA), 0x1F1F);
}

#[test]
fn timers_read_the_counter() {
    let mut b = bus();
    b.write16(IO + 0x104, 0xFF00);
    assert_eq!(b.io.raw16(0x104), 0xFF00);
    assert_eq!(b.read16(IO + 0x104), 0);
    b.io.timers[1].counter = 0xFF42;
    assert_eq!(b.read16(IO + 0x104), 0xFF42);
    assert_eq!(b.read32(IO + 0x104), 0xFF42);
}

#[test]
fn io_masks() {
    let mut b = bus();
    // DISPSTAT: status bits are the scheduler's.
    b.write16(IO + reg::DISPSTAT, 0xFFFF);
    assert_eq!(b.read16(IO + reg::DISPSTAT), 0xFF38);
    b.io.set_blanking(true, false);
    assert_eq!(b.read16(IO + reg::DISPSTAT) & 7, 1);
    // VCOUNT is read-only; setting it updates the match flag.
    b.write16(IO + reg::VCOUNT, 5);
    assert_eq!(b.read16(IO + reg::VCOUNT), 0);
    assert!(b.io.set_vcount(0xFF));
    assert_eq!(b.read16(IO + reg::DISPSTAT) & 4, 4);
    assert!(!b.io.set_vcount(3));
    assert_eq!(b.read16(IO + reg::DISPSTAT) & 4, 0);
    assert_eq!(b.read32(IO + reg::DISPSTAT), 0x0003_FF39);
    // DISPCNT's CGB bit.
    b.write16(IO + reg::DISPCNT, 0xFFFF);
    assert_eq!(b.read16(IO + reg::DISPCNT), 0xFFF7);
    // Write-only scroll offsets read 0 but are stored for the PPU.
    b.write16(IO + reg::BG0HOFS, 0xFFFF);
    assert_eq!(b.read16(IO + reg::BG0HOFS), 0);
    assert_eq!(b.io.raw16(reg::BG0HOFS), 0x01FF);
    // WAITCNT: stored, bit 15 read-only.
    b.write16(IO + reg::WAITCNT, 0xFFFF);
    assert_eq!(b.read16(IO + reg::WAITCNT), 0x5FFF);
    // Unused halfwords.
    b.write16(IO + 0x056, 0xFFFF);
    assert_eq!(b.read16(IO + 0x056), 0);
    // Byte writes keep the other byte.
    b.write16(IO + reg::BLDCNT, 0x3FFF);
    b.write8(IO + reg::BLDCNT + 1, 0);
    assert_eq!(b.read16(IO + reg::BLDCNT), 0x00FF);
    assert_eq!(b.read8(IO + reg::BLDCNT), 0xFF);
    // A word write is two halfword writes.
    b.write32(IO + reg::WININ, 0xFFFF_FFFF);
    assert_eq!(b.read32(IO + reg::WININ), 0x3F3F_3F3F);
}

#[test]
fn keyinput_is_active_low_and_read_only() {
    let mut b = bus();
    assert_eq!(b.read16(IO + reg::KEYINPUT), 0x03FF);
    b.io.set_keys(key::A | key::START);
    assert_eq!(
        b.read16(IO + reg::KEYINPUT),
        0x03FF & !(key::A | key::START)
    );
    b.write16(IO + reg::KEYINPUT, 0);
    assert_eq!(
        b.read16(IO + reg::KEYINPUT),
        0x03FF & !(key::A | key::START)
    );
}

#[test]
fn interrupt_registers() {
    let mut b = bus();
    b.write16(IO + reg::IE, irq::VBLANK | irq::TIMER0);
    b.io.request_irq(irq::VBLANK | irq::HBLANK);
    assert!(b.io.irq_requested());
    assert!(!b.io.irq_pending());
    b.write16(IO + reg::IME, 0xFFFF);
    assert_eq!(b.read16(IO + reg::IME), 1);
    assert!(b.io.irq_pending());
    // Writing IF clears the bits written as 1, and only those.
    b.write16(IO + reg::IF, !irq::HBLANK);
    assert_eq!(b.read16(IO + reg::IF), irq::HBLANK);
    assert!(!b.io.irq_requested());
    // A byte write to IF's low byte leaves the high byte alone.
    b.io.request_irq(irq::DMA0);
    b.write8(IO + reg::IF, 0xFF);
    assert_eq!(b.read16(IO + reg::IF), irq::DMA0);
    b.write16(IO + reg::IF, irq::DMA0);
    // And the other way round.
    b.io.request_irq(irq::VBLANK | irq::DMA0);
    b.write8(IO + reg::IF + 1, 0xFF);
    assert_eq!(b.read16(IO + reg::IF), irq::VBLANK);
    // The word at IE writes IE and IF together.
    b.write32(IO + reg::IE, 0xFFFF_0000);
    assert_eq!(b.read32(IO + reg::IE), 0);
    assert!(b.io.take_events() & event::IRQ != 0);
    assert_eq!(b.io.events, 0);
}

#[test]
fn side_effect_writes_raise_events() {
    let mut b = bus();
    b.io.write16(0x0C6, 0x8000);
    assert_eq!(b.io.take_events(), event::DMA0 << 1);
    // Through the bus, the DMA write is acted on in the store, which
    // consumes its event.
    b.write16(IO + 0x0D2, 0x8000);
    assert_eq!(b.io.take_events(), 0);
    b.write16(IO + 0x10E, 0x80);
    assert_eq!(b.io.take_events(), event::TIMER0 << 3);
    b.write8(IO + 0x301, 0);
    assert_eq!(b.io.take_events(), event::HALT);
    b.write8(IO + 0x301, 0x80);
    assert_eq!(b.io.take_events(), event::STOP);
    // POSTFLG alone doesn't halt.
    b.write8(IO + reg::POSTFLG, 1);
    assert_eq!(b.io.take_events(), 0);
    // FIFO stores push in the store, and leave no event.
    b.write32(IO + reg::FIFO_A, 0);
    assert_eq!(b.io.take_events(), 0);
    assert_eq!(b.sound.fifo[0].len(), 4);
    b.write16(IO + reg::SOUNDCNT_X, 0x80);
    assert_eq!(b.io.take_events(), event::SOUND);
    // So do FIFO resets, which aren't stored bits and so don't repeat.
    b.write16(IO + reg::SOUNDCNT_H, 0x8800);
    assert_eq!(b.io.take_events(), event::SOUND);
    assert!(b.sound.fifo[0].is_empty());
    b.write8(IO + reg::SOUNDCNT_H, 0x0F);
    assert_eq!(b.io.take_events(), event::SOUND);
    assert_eq!(b.read16(IO + reg::SOUNDCNT_H), 0x000F);
    // Hardware-side updates raise nothing.
    b.io.set_raw16(0x0DE, 0);
    assert_eq!(b.io.take_events(), 0);
    assert_eq!(b.read16(IO + reg::SOUNDCNT_X), 0x80);
    // Plain registers don't.
    b.write16(IO + reg::DISPCNT, 0x80);
    assert_eq!(b.io.take_events(), 0);
}

/// The interpreter runs on the bus unchanged: an ARM loop storing to IWRAM.
#[test]
fn interpreter_runs_on_the_machine() {
    let code: [u32; 4] = [
        0xE3A0_0042, // mov r0, #0x42
        0xE3A0_1403, // mov r1, #0x03000000
        0xE581_0010, // str r0, [r1, #0x10]
        0xEAFF_FFFE, // b .
    ];
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let rom: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let mut m = Box::new(Machine::new(rom, Headless));
    for _ in 0..4 {
        m.step();
    }
    assert_eq!(m.bus.read32(0x0300_0010), 0x42);
    assert_eq!(m.cpu.pc, 0x0800_000C);
    assert_eq!(m.cpu.r[13], crate::machine::SP_SYS);
}

#[test]
fn a_flash_rom_gets_the_flash_chip() {
    let mut b = Box::new(Bus::new(b"\0\0\0\0FLASH1M_V103"));
    b.write8(0x0E00_5555, 0xAA);
    b.write8(0x0E00_2AAA, 0x55);
    b.write8(0x0E00_5555, 0x90);
    assert_eq!(b.read8(0x0E00_0000), 0x62);
    assert_eq!(b.read8(0x0E00_0001), 0x13);
    assert_eq!(b.read16(0x0E00_0000), 0x6262);
}
