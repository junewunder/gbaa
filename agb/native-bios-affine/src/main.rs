//! gbaa test ROM for the affine BIOS calls the native lifter prints
//! (native/lift-bios-affine): `BgAffineSet` (0x0E) and `ObjAffineSet`
//! (0x0F).
//!
//! Each frame `f` (from 0) the main loop makes six calls, on arguments
//! that change with `f`, and keeps r0-r3 as each leaves them in `OUT`, a
//! global in RAM, with what the calls wrote into `OUT`'s buffers:
//!
//! 0. `BgAffineSet`, three entries from RAM (`OUT.bg_src`) into
//!    `OUT.bg`;
//! 1. `BgAffineSet`, two entries from ROM (`BG_ROM`) into `OUT.bg_rom`;
//! 2. `BgAffineSet`, one identity entry into BG2's own registers
//!    (`BG2PA`-`BG2Y`, 0x0400_0020), as a game sets a background;
//! 3. `ObjAffineSet`, four entries from RAM, stride 2, into `OUT.obj`;
//! 4. `ObjAffineSet`, two entries from RAM, stride 8, into OAM's
//!    parameters (0x0700_0006), as a game sets its sprites';
//! 5. `BgAffineSet` with a count of `f % 2` (none on even frames).
//!
//! Each call's registers, folded with what it wrote (but call 2's, whose
//! registers can't be read back), are drawn as a pixel
//! in mode 3, its own row, column `f % 240`. A lift that gets a call's
//! registers or output wrong draws a different frame or leaves different
//! RAM, I/O or OAM. What it draws: `SCENE.md`.
//!
//! mGBA draws a different frame: its HLE BIOS computes the matrices in
//! floats and leaves r0-r2 as given, where the play build follows the
//! real BIOS's sine table and registers (`runtime::bios::math`).
#![no_std]
#![no_main]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const BG2PA: u32 = 0x0400_0020;
const OAM_PARAMS: u32 = 0x0700_0006;
const VRAM: u32 = 0x0600_0000;

/// A call's r0-r3 in and out.
type Regs = [u32; 4];

/// `swi N` with r0-r3 in and out. r12 may change in the BIOS.
macro_rules! swi {
    ($n:literal, $r:expr) => {{
        let r: &mut Regs = $r;
        // SAFETY: each call's arguments are set up by its caller below
        // (addresses of sources and of buffers that hold what it writes).
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

/// A `BgAffineSet` source: texture centre (24.8), screen centre, scales
/// (8.8) and angle; 20 bytes.
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct BgSrc {
    ox: i32,
    oy: i32,
    cx: i16,
    cy: i16,
    sx: i16,
    sy: i16,
    angle: u16,
    pad: u16,
}

/// A `BgAffineSet` destination: pa-pd and the start point; 16 bytes.
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct BgDst {
    p: [i16; 4],
    x: i32,
    y: i32,
}

/// An `ObjAffineSet` source: scales (8.8) and angle; 8 bytes.
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct ObjSrc {
    sx: i16,
    sy: i16,
    angle: u16,
    pad: u16,
}

const BG0: BgSrc = BgSrc {
    ox: 0,
    oy: 0,
    cx: 0,
    cy: 0,
    sx: 0x100,
    sy: 0x100,
    angle: 0,
    pad: 0,
};
const BD0: BgDst = BgDst {
    p: [0; 4],
    x: 0,
    y: 0,
};
const OS0: ObjSrc = ObjSrc {
    sx: 0,
    sy: 0,
    angle: 0,
    pad: 0,
};

/// Two fixed sources in ROM: a shrink turned an eighth, a negative
/// stretch turned most of the way round.
static BG_ROM: [BgSrc; 2] = [
    BgSrc {
        ox: 120 << 8,
        oy: 80 << 8,
        cx: 120,
        cy: 80,
        sx: 0x80,
        sy: 0xC0,
        angle: 0x2000,
        pad: 0,
    },
    BgSrc {
        ox: -(37 << 8) + 3,
        oy: 1000 << 8,
        cx: -16,
        cy: 200,
        sx: -0x180,
        sy: 0x7FFF,
        angle: 0xE123,
        pad: 0,
    },
];

/// The calls, one row each (the header's list).
const CALLS: usize = 6;

/// The last frame's results, for the native check to compare.
#[repr(C, align(4))]
struct Out {
    /// Each call's r0-r3 after it.
    regs: [Regs; CALLS],
    bg_src: [BgSrc; 3],
    bg: [BgDst; 3],
    bg_rom: [BgDst; 2],
    obj_src: [ObjSrc; 4],
    obj: [[i16; 4]; 4],
    /// Call 5's destination.
    bg_odd: BgDst,
}

struct Global(UnsafeCell<Out>);

// SAFETY: only the main loop touches it; agb's handler doesn't.
unsafe impl Sync for Global {}

static OUT: Global = Global(UnsafeCell::new(Out {
    regs: [[0; 4]; CALLS],
    bg_src: [BG0; 3],
    bg: [BD0; 3],
    bg_rom: [BD0; 2],
    obj_src: [OS0; 4],
    obj: [[0; 4]; 4],
    bg_odd: BD0,
}));

fn addr<T>(p: *const T) -> u32 {
    p as u32
}

/// A value of `f` that wanders over the whole of a 16-bit range.
fn mix(f: u32, k: u32) -> u32 {
    let v = f.wrapping_add(k).wrapping_mul(0x9E37_79B9);
    v ^ v >> 16
}

/// One frame's calls, into `o`.
fn calls(f: u32, o: &mut Out) {
    for (k, s) in o.bg_src.iter_mut().enumerate() {
        let k = k as u32;
        *s = BgSrc {
            ox: (mix(f, k) as i32) >> 4,
            oy: (mix(f, k + 10) as i32) >> 6,
            cx: (f * 3 + k * 40) as i16 % 240,
            cy: (f * 7 + k * 20) as i16 % 160 - 40,
            sx: mix(f, k + 20) as i16 >> 4,
            sy: mix(f, k + 30) as i16 >> 3,
            angle: (f * 0x1234 + k * 0x5555) as u16,
            pad: 0,
        };
    }
    for (k, s) in o.obj_src.iter_mut().enumerate() {
        let k = k as u32;
        *s = ObjSrc {
            sx: mix(f, k + 40) as i16 >> 5,
            sy: (0x100 + f * 3 + k * 50) as i16,
            angle: (f * 0x0321 + k * 0x4000) as u16,
            pad: 0,
        };
    }
    let mut rs = [[0u32; 4]; CALLS];
    rs[0] = [addr(&o.bg_src), addr(&o.bg), 3, f];
    swi!("0x0E", &mut rs[0]);
    rs[1] = [addr(&BG_ROM), addr(&o.bg_rom), 2, 1];
    swi!("0x0E", &mut rs[1]);
    // The identity, from the stack: anything else would turn the picture.
    let id = [BG0];
    rs[2] = [addr(&id), BG2PA, 1, 2];
    swi!("0x0E", &mut rs[2]);
    rs[3] = [addr(&o.obj_src), addr(&o.obj), 4, 2];
    swi!("0x0F", &mut rs[3]);
    rs[4] = [addr(&o.obj_src[1]), OAM_PARAMS + (f % 8) * 32, 2, 8];
    swi!("0x0F", &mut rs[4]);
    rs[5] = [addr(&o.bg_src[2]), addr(&o.bg_odd), f % 2, 3];
    swi!("0x0E", &mut rs[5]);
    o.regs = rs;
}

/// What a call wrote, as words: call `k`'s destination.
fn written(k: usize, o: &Out, f: u32) -> u32 {
    let words = |p: *const u32, n: usize| {
        (0..n).fold(0u32, |a, i| {
            // SAFETY: `p` is a destination of `n` words the call wrote.
            a.rotate_left(7) ^ unsafe { read_volatile(p.add(i)) }
        })
    };
    match k {
        0 => words(addr(&o.bg) as *const u32, 12),
        1 => words(addr(&o.bg_rom) as *const u32, 8),
        // BG2's registers are write-only: its registers alone.
        2 => 0,
        3 => words(addr(&o.obj) as *const u32, 8),
        4 => {
            let base = OAM_PARAMS + (f % 8) * 32;
            (0..8).fold(0u32, |a, i| {
                // SAFETY: OAM's affine halfwords the call wrote.
                a.rotate_left(5) ^ unsafe { read_volatile((base + i * 8) as *const u16) } as u32
            })
        }
        _ => words(addr(&o.bg_odd) as *const u32, 4),
    }
}

/// A call's registers and output folded into a BGR555 colour.
fn colour(r: &Regs, w: u32) -> u16 {
    let v = r[0] ^ r[1].rotate_left(5) ^ r[2].rotate_left(11) ^ r[3].rotate_left(17) ^ w;
    ((v ^ v >> 15 ^ v >> 30) & 0x7FFF) as u16
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let mut f: u32 = 0;
    loop {
        // SAFETY: OUT is ours (see its Sync).
        let o = unsafe { &mut *OUT.0.get() };
        calls(f, o);
        for k in 0..CALLS {
            let c = colour(&o.regs[k], written(k, o, f));
            let px = VRAM + ((k as u32 * 4) * 240 + f % 240) * 2;
            // SAFETY: a pixel of mode 3's frame buffer, rows 0-20.
            unsafe { write_volatile(px as *mut u16, c) };
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
