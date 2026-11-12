//! gbaa test ROM for the BIOS calls the native lifter prints beyond the
//! waits, `Div` and `CpuSet` (native/lift-bios-calls): `Stop` (0x03),
//! `Sqrt` (0x08), `ArcTan2` (0x0A), `CpuFastSet` (0x0C), the LZ77 (0x11,
//! 0x12) and RL (0x14, 0x15) decompressors, `SoundBias` (0x19),
//! `MidiKey2Freq` (0x1F) and `MultiBoot` (0x25).
//!
//! Each frame `f` (from 0) the main loop makes every call once, on
//! arguments that change with `f`, and keeps r0-r3 as each leaves them in
//! `OUT`, a global in RAM, with what the memory calls wrote (EWRAM-bound
//! decompressions and copies into `OUT`'s buffers, VRAM-bound ones into
//! the frame buffer). Each call's registers are also drawn as a pixel in
//! mode 3, its own row, column `f % 240`. A lift that gets a call's
//! registers, its output or its time wrong draws a different frame or
//! leaves different RAM. On frames with `f % 4 == 1` it waits with
//! `Stop` instead of agb's V-blank wait; the play build, like mGBA,
//! wakes from it at the next IRQ. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const SOUNDBIAS: *mut u16 = 0x0400_0088 as *mut u16;
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

/// Word-aligned bytes (the decompressors read a header word).
#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

/// 96 bytes LZ77-compressed: 8 literals, then back-references of 3 to 18
/// bytes up to 33 back between literals. No distance is under 2, so the
/// VRAM call (which stores halfwords) reads only bytes it has stored.
static LZ: Aligned<36> = Aligned([
    0x10, 96, 0, 0, // type 0x10, 96 bytes
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, // 8 literals: 8
    0xAA, // ref, lit, ref, lit, ref, lit, ref, lit
    0x50, 0x07, 0x99, // 8 from 8 back, 0x99: 17
    0xF0, 0x08, 0xAB, // 18 from 9 back, 0xAB: 36
    0xC0, 0x01, 0xCD, // 15 from 2 back, 0xCD: 52
    0xF0, 0x13, 0xEF, // 18 from 20 back, 0xEF: 71
    0xC0, // ref, ref
    0xF0, 0x20, // 18 from 33 back: 89
    0x40, 0x01, // 7 from 2 back: 96
    0, 0, 0, 0, 0, // to a word
]);

/// 78 bytes RL-compressed (padded to 80 by the call): a run, literals, a
/// run, literals.
static RL: Aligned<24> = Aligned([
    0x30, 78, 0, 0, // type 0x30, 78 bytes
    0x91, 0x3C, // 20 of 0x3C: 20
    0x04, 1, 2, 3, 4, 5, // 5 literals: 25
    0xAF, 0xE7, // 50 of 0xE7: 75
    0x02, 9, 8, 7, // 3 literals: 78
    0, 0, 0, 0, 0, 0, // to a word
]);

/// What `MidiKey2Freq` reads of a WaveData: its frequency, at +4.
#[repr(C, align(4))]
struct Wave {
    header: u32,
    freq: u32,
}

static WAVE: Wave = Wave {
    header: 0,
    freq: 0x2B11_0000,
};

/// `CpuFastSet`'s source in ROM.
static TABLE: [u32; 32] = {
    let mut t = [0; 32];
    let mut i = 0;
    while i < 32 {
        t[i] = (i as u32 + 1).wrapping_mul(0x9E37_79B9);
        i += 1;
    }
    t
};

/// The last frame's results, for the native check to compare.
#[repr(C, align(4))]
struct Out {
    /// Each call's r0-r3 after it, in `CALLS`'s order.
    regs: [Regs; CALLS],
    /// `LZ77UnCompWram` from ROM, then from `lz_src`.
    lz: [u8; 96],
    lz_ram: [u8; 96],
    /// `LZ`, copied to RAM by `CpuFastSet`.
    lz_src: [u32; 16],
    /// `RLUnCompWram`'s output, padded.
    rl: [u8; 80],
    /// `CpuFastSet`'s copy of `TABLE` and its fill.
    fast: [u32; 32],
    fill: [u32; 16],
    /// The fill's word.
    word: u32,
    /// `SOUNDBIAS` after `SoundBias`.
    bias: u32,
}

/// The calls, one row each: sqrt, arctan2, key2freq, fast copy, fast
/// fill, lz77 wram, lz77 from ram, lz77 vram, rl wram, rl vram, sound
/// bias, multiboot, and the frame's last copy (of `LZ` to RAM).
const CALLS: usize = 13;

struct Global(UnsafeCell<Out>);

// SAFETY: only the main loop touches it; agb's handler doesn't.
unsafe impl Sync for Global {}

static OUT: Global = Global(UnsafeCell::new(Out {
    regs: [[0; 4]; CALLS],
    lz: [0; 96],
    lz_ram: [0; 96],
    lz_src: [0; 16],
    rl: [0; 80],
    fast: [0; 32],
    fill: [0; 16],
    word: 0,
    bias: 0,
}));

fn addr<T>(p: *const T) -> u32 {
    p as u32
}

/// One frame's calls, into `o`.
fn calls(f: u32, o: &mut Out) {
    let mut rs = [[0u32; 4]; CALLS];
    // Sqrt: r0.
    rs[0] = [f.wrapping_mul(0x0009_E377).wrapping_add(f * f), 1, 2, 3];
    swi!("0x08", &mut rs[0]);
    // ArcTan2 (x, y): every quadrant over 512 frames.
    let x = (f * 37 % 512) as i32 - 256;
    let y = (f * 53 % 512) as i32 - 256;
    rs[1] = [x as u32, y as u32, 5, 6];
    swi!("0x0A", &mut rs[1]);
    // MidiKey2Freq (wave, key, fine).
    rs[2] = [addr(&WAVE), f % 128, f * 29 % 256, 7];
    swi!("0x1F", &mut rs[2]);
    // CpuFastSet: a copy of 1-32 words (rounded up to 8s), then a fill.
    rs[3] = [addr(&TABLE), addr(&o.fast), 1 + f % 32, 0];
    swi!("0x0C", &mut rs[3]);
    o.word = f.wrapping_mul(0x0101_0101);
    rs[4] = [addr(&o.word), addr(&o.fill), 16 | 1 << 24, 0];
    swi!("0x0C", &mut rs[4]);
    // LZ77: into RAM from ROM and from RAM (last frame's copy), and into
    // a row of the frame buffer.
    rs[5] = [addr(&LZ), addr(&o.lz), 0, 9];
    swi!("0x11", &mut rs[5]);
    rs[6] = [addr(&o.lz_src), addr(&o.lz_ram), 0, 9];
    swi!("0x11", &mut rs[6]);
    rs[7] = [addr(&LZ), VRAM + (40 + f % 40) * 480 + (f / 40 % 4) * 96, 0, 9];
    swi!("0x12", &mut rs[7]);
    // RL: into RAM and the frame buffer.
    rs[8] = [addr(&RL), addr(&o.rl), 10, 11];
    swi!("0x14", &mut rs[8]);
    rs[9] = [addr(&RL), VRAM + (100 + f % 40) * 480 + (f / 40 % 4) * 80, 10, 11];
    swi!("0x15", &mut rs[9]);
    // SoundBias: level 0 on odd frames, 0x200 on even ones.
    rs[10] = [(f + 1) & 1, 12, 13, 14];
    swi!("0x19", &mut rs[10]);
    // SAFETY: SOUNDBIAS is a halfword I/O register.
    o.bias = unsafe { read_volatile(SOUNDBIAS) } as u32;
    // MultiBoot: no cable, so it fails.
    rs[11] = [addr(&o.fast), 1, 15, 16];
    swi!("0x25", &mut rs[11]);
    // Next frame's LZ77 source in RAM: `LZ` (9 words) by CpuFastSet.
    rs[12] = [addr(&LZ), addr(&o.lz_src), 9, 0];
    swi!("0x0C", &mut rs[12]);
    o.regs = rs;
}

/// A call's registers folded into a BGR555 colour.
fn colour(r: &Regs) -> u16 {
    let v = r[0] ^ r[1].rotate_left(5) ^ r[2].rotate_left(11) ^ r[3].rotate_left(17);
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
        // An index into a static array the program also reaches by
        // constant address (`o.regs = rs`): its element stays bytes in
        // the lift (native/ref-heap-static-arrays).
        for (k, r) in o.regs.iter().enumerate() {
            let px = VRAM + ((k as u32) * 240 + f % 240) * 2;
            // SAFETY: a pixel of mode 3's frame buffer, rows 0-12.
            unsafe { write_volatile(px as *mut u16, colour(r)) };
        }
        if f % 4 == 1 {
            let mut r = [0; 4];
            swi!("0x03", &mut r);
        } else {
            vblank.wait_for_vblank();
        }
        f += 1;
    }
}
