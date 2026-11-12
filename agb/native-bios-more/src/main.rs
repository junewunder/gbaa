//! gbaa test ROM for the BIOS calls the native lifter prints that
//! native/lift-bios-calls left out (native/lift-bios-more): `SoftReset`
//! (0x00), `RegisterRamReset` (0x01), the Huffman decompressor (0x13) and
//! `SoundDriverVSyncOff` (0x28).
//!
//! Each frame `f` (from 0) the main loop writes a palette entry and an OAM
//! halfword, then `RegisterRamReset`s by `f` none of them, the palette,
//! OAM, both, the sound registers, or the display, DMA, timer and
//! interrupt registers, and sets the display and agb's V-blank IRQ back as
//! they were. Then it decompresses two Huffman blobs (8- and 4-bit
//! symbols) from ROM into `OUT`, a global in RAM, and a copy of the 8-bit
//! one, with a leaf that changes with `f`, from RAM into a row of the
//! frame buffer, and makes a `SoundDriverVSyncOff`. Each call's r0-r3 as
//! it leaves them are kept in `OUT` and drawn as a pixel in mode 3, its
//! own row, column `f % 240`. At `f == RESET_AT` it resets EWRAM, VRAM,
//! the palette, OAM, sound and the other registers, then `SoftReset`s (as
//! the SDK's `SoftReset(flags)` does), so it runs from `f = 0` again with
//! everything agb's start-up makes. A lift that gets a call's registers,
//! its output or the restart wrong draws a different frame or leaves
//! different RAM. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

const VRAM: u32 = 0x0600_0000;

/// A call's r0-r3 in and out.
type Regs = [u32; 4];

/// `swi N` with r0-r3 in and out. r12 may change in the BIOS.
macro_rules! swi {
    ($n:literal, $r:expr) => {{
        let r: &mut Regs = $r;
        // SAFETY: each call's arguments are set up by its caller below
        // (addresses of buffers that hold what the call writes).
        unsafe {
            asm!(
                concat!("swi ", $n),
                inout("r0") r[0],
                inout("r1") r[1],
                inout("r2") r[2],
                inout("r3") r[3],
                out("r12") _,
            );
        }
    }};
}

/// Word-aligned bytes (the decompressor reads a header word).
#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

/// Huffman-compressed data (`HuffUnComp`), 8-bit symbols then 4-bit: the
/// same tree of four leaves (codes 0, 10, 110, 111; the tree size byte 3,
/// so the bits start word-aligned at +12) over a run of random symbols,
/// 64 and 48 bytes out.
static HUFF8: Aligned<28> = Aligned([
    0x28, 0x40, 0x00, 0x00, 0x03, 0x80, 0x5A, 0x80, 0xC3, 0xC0, 0x0F, 0xF0, 0xC1, 0xB0, 0x41, 0x98,
    0xFA, 0x20, 0xC4, 0x82, 0x8C, 0xF7, 0x82, 0xFE, 0x00, 0x00, 0x00, 0x9F,
]);
static HUFF4: Aligned<36> = Aligned([
    0x24, 0x30, 0x00, 0x00, 0x03, 0x80, 0x07, 0x80, 0x0C, 0xC0, 0x01, 0x0E, 0x57, 0x99, 0x5F, 0xCF,
    0x6B, 0x36, 0xB6, 0x9C, 0x8E, 0x44, 0xAC, 0x79, 0xF7, 0xA8, 0x78, 0x65, 0x5E, 0x0C, 0x68, 0x9A,
    0x00, 0x00, 0x28, 0x05,
]);

/// The frame whose end restarts the program by `SoftReset`.
const RESET_AT: u32 = 150;

/// What `RegisterRamReset` clears each frame, by `f % 6`.
const FLAGS: [u32; 6] = [0x00, 0x04, 0x10, 0x14, 0x40, 0x80];

/// What it clears before the `SoftReset`: everything but the serial
/// registers (0x20, which the native library doesn't keep) and IWRAM
/// (0x02, where the stack is).
const RESET_FLAGS: u32 = 0xDD;

/// The last frame's results, for the native check to compare.
#[repr(C, align(4))]
struct Out {
    /// Each call's r0-r3 after it, in `CALLS`'s order.
    regs: [Regs; CALLS],
    /// `HuffUnComp`'s outputs from ROM.
    huff8: [u8; 64],
    huff4: [u8; 48],
    /// `HUFF8` copied to RAM, its first leaf the frame's.
    huff_src: Aligned<28>,
    /// `SOUNDBIAS` after the frame's `RegisterRamReset`.
    bias: u32,
}

/// The calls, one row each: register ram reset, huffman 8-bit and 4-bit
/// from ROM, huffman 8-bit from RAM into VRAM, sound driver vsync off.
const CALLS: usize = 5;

struct Global(UnsafeCell<Out>);

// SAFETY: only the main loop touches it; agb's handler doesn't.
unsafe impl Sync for Global {}

static OUT: Global = Global(UnsafeCell::new(Out {
    regs: [[0; 4]; CALLS],
    huff8: [0; 64],
    huff4: [0; 48],
    huff_src: Aligned([0; 28]),
    bias: 0,
}));

fn addr<T>(p: *const T) -> u32 {
    p as u32
}

fn io(o: u32) -> *mut u16 {
    (0x0400_0000 + o) as *mut u16
}

/// One frame's calls, into `o`.
fn calls(f: u32, o: &mut Out) {
    let mut rs = [[0u32; 4]; CALLS];
    // RegisterRamReset, after a palette entry and an OAM halfword.
    // SAFETY: palette and OAM halfwords; I/O registers.
    let (dispcnt, dispstat, ie, waitcnt) = unsafe {
        write_volatile((0x0500_0000 + f % 512 * 2) as *mut u16, f as u16 | 1);
        write_volatile((0x0700_0000 + f * 7 % 512 * 2) as *mut u16, f as u16 | 2);
        let r = |o| read_volatile(io(o));
        (r(0x000), r(0x004), r(0x200), r(0x204))
    };
    rs[0] = [FLAGS[(f % 6) as usize], 1, 2, 3];
    swi!("0x01", &mut rs[0]);
    // SAFETY: as above: the display as `main` set it, and agb's V-blank
    // IRQ as it set it up.
    unsafe {
        o.bias = read_volatile(io(0x088)) as u32;
        write_volatile(io(0x000), dispcnt);
        write_volatile(io(0x004), dispstat);
        write_volatile(io(0x200), ie);
        write_volatile(io(0x204), waitcnt);
        write_volatile(io(0x208), 1);
    }
    // HuffUnComp, 8- and 4-bit, from ROM into RAM.
    rs[1] = [addr(&HUFF8), addr(&o.huff8), 4, 5];
    swi!("0x13", &mut rs[1]);
    rs[2] = [addr(&HUFF4), addr(&o.huff4), 6, 7];
    swi!("0x13", &mut rs[2]);
    // From RAM, its first leaf the frame's, into a row of the frame buffer.
    o.huff_src.0 = HUFF8.0;
    o.huff_src.0[6] = f as u8;
    rs[3] = [addr(&o.huff_src), VRAM + (40 + f % 40) * 480 + (f / 40 % 4) * 64, 8, 9];
    swi!("0x13", &mut rs[3]);
    // SoundDriverVSyncOff: nothing, as the play build has no sound driver.
    rs[4] = [f, 10, 11, 12];
    swi!("0x28", &mut rs[4]);
    o.regs = rs;
}

/// A call's registers folded into a BGR555 colour.
fn colour(r: &Regs) -> u16 {
    let v = r[0] ^ r[1].rotate_left(5) ^ r[2].rotate_left(11) ^ r[3].rotate_left(17);
    ((v ^ v >> 15 ^ v >> 30) & 0x7FFF) as u16
}

/// `RegisterRamReset(RESET_FLAGS)` then `SoftReset`, as the SDK's
/// `SoftReset(flags)`: IME off and the restart flag clear first (back to
/// the cartridge), the two calls in one block so nothing runs between.
fn soft_reset() -> ! {
    // SAFETY: IME, and the BIOS's restart flag at the top of IWRAM; the
    // reset clears no memory the two `swi`s need, and the second doesn't
    // return.
    unsafe {
        write_volatile(io(0x208), 0);
        write_volatile(0x0300_7FFA as *mut u8, 0);
        asm!("swi 0x01", "swi 0x00", in("r0") RESET_FLAGS, options(noreturn));
    }
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(io(0x000), 0x0403) };
    let mut f: u32 = 0;
    loop {
        // SAFETY: OUT is ours (see its Sync).
        let o = unsafe { &mut *OUT.0.get() };
        calls(f, o);
        for (k, r) in o.regs.iter().enumerate() {
            let px = VRAM + ((k as u32) * 240 + f % 240) * 2;
            // SAFETY: a pixel of mode 3's frame buffer, rows 0-4.
            unsafe { write_volatile(px as *mut u16, colour(r)) };
        }
        if f == RESET_AT {
            soft_reset();
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
