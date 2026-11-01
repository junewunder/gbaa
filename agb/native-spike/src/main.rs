//! gbaa's native-port spike (native/agb-spike): a sprite the D-pad moves
//! over a background a dead-zone camera scrolls, set up and driven by
//! writing the display registers directly, as `agb/template` does.
//!
//! `crates/native-spike` is this program lifted by hand to a native Rust
//! program (docs/native-port-design.md); the two must draw the same frames
//! for the same presses (`just native-spike`). What it draws: `SCENE.md`.
//!
//! The first frame is drawn before anything is set up, so set-up leaves
//! DISPCNT at 0 (no layer on) and the backdrop colour at 0: that frame is
//! black however long set-up takes. The display goes on at the first
//! commit, in the first V-blank.
//!
//! VRAM, palette and OAM take halfword stores only (`store16`; see
//! `agb/template`).
#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use core::cell::UnsafeCell;

const IO: usize = 0x0400_0000;
const DISPCNT: usize = IO;
const BG0CNT: usize = IO + 0x8;
const BG0HOFS: usize = IO + 0x10;
const BG0VOFS: usize = IO + 0x12;
const KEYINPUT: usize = IO + 0x130;
const PAL: usize = 0x0500_0000;
const OBJ_PAL: usize = PAL + 0x200;
const VRAM: usize = 0x0600_0000;
const OBJ_VRAM: usize = VRAM + 0x1_0000;
const OAM: usize = 0x0700_0000;

const BG0_ON: u16 = 1 << 8;
const OBJ_ON: u16 = 1 << 12;
const OBJ_1D: u16 = 1 << 6;

const KEY_A: u16 = 1 << 0;
const KEY_B: u16 = 1 << 1;
const KEY_RIGHT: u16 = 1 << 4;
const KEY_LEFT: u16 = 1 << 5;
const KEY_UP: u16 = 1 << 6;
const KEY_DOWN: u16 = 1 << 7;

/// The world the player walks: 1024 pixels square (BG0's 256-pixel map
/// repeats over it).
const WORLD: i32 = 1024;
/// The sprite's size.
const SIZE: i32 = 16;
/// The dead zone: where on screen the sprite's top-left may be before the
/// camera follows it.
const ZONE_X: (i32, i32) = (64, 160);
const ZONE_Y: (i32, i32) = (40, 104);
/// The backdrop.
const BACKDROP: u16 = 0x2108;
/// The sprite's colour 2, a step every 16 frames.
const BLINK: [u16; 4] = [0x7FFF, 0x5294, 0x318C, 0x5294];

/// Frames counted by the V-blank handler. (ARMv4T has no atomics.)
struct Counter(UnsafeCell<u32>);

// SAFETY: only the V-blank handler writes it, and the main loop reads it
// only between handlers, so no access overlaps a write.
unsafe impl Sync for Counter {}

impl Counter {
    fn get(&self) -> u32 {
        // SAFETY: see the `Sync` impl.
        unsafe { read_volatile(self.0.get()) }
    }

    fn step(&self) {
        // SAFETY: see the `Sync` impl.
        unsafe { write_volatile(self.0.get(), self.get() + 1) }
    }
}

static FRAMES: Counter = Counter(UnsafeCell::new(0));

fn store16(addr: usize, v: u16) {
    // SAFETY: callers pass addresses in IO, palette, VRAM or OAM, which
    // take halfword stores.
    unsafe { write_volatile(addr as *mut u16, v) }
}

fn load16(addr: usize) -> u16 {
    // SAFETY: an I/O register.
    unsafe { read_volatile(addr as *const u16) }
}

/// Four 4bpp pixels as one halfword, the leftmost in the low nibble.
const fn quad(p: [u16; 4]) -> u16 {
    p[0] | p[1] << 4 | p[2] << 8 | p[3] << 12
}

/// BG tile `t`'s pixel (x, y).
const fn bg_pixel(t: usize, x: usize, y: usize) -> u16 {
    match t {
        // Diagonal stripes in colours 1-3.
        1 => 1 + ((x + y) / 2 % 3) as u16,
        // A box: colour 4 on the top and left edges, 5 inside.
        2 => {
            if x == 0 || y == 0 {
                4
            } else {
                5
            }
        }
        // Solid 6 with one dot of 4.
        _ => {
            if x == 5 && y == 2 {
                4
            } else {
                6
            }
        }
    }
}

/// The sprite's pixel (x, y), 0-15 each: a ring in colour 1, a centre in
/// colour 2 (the one that blinks), an eye in colour 3 on the right so a
/// flip shows.
const fn obj_pixel(x: usize, y: usize) -> u16 {
    let dx = 2 * x as i32 - 15;
    let dy = 2 * y as i32 - 15;
    let r2 = dx * dx + dy * dy;
    if x >= 10 && x <= 11 && y >= 5 && y <= 6 {
        3
    } else if r2 < 36 {
        2
    } else if r2 >= 100 && r2 < 196 {
        1
    } else {
        0
    }
}

/// The map entry at tile (tx, ty).
const fn map_entry(tx: usize, ty: usize) -> u16 {
    let tile = match (tx * 7 + ty * 3) % 5 {
        0 | 1 => 1,
        2 | 3 => 2,
        _ => 3,
    };
    let hflip = if (tx + ty) % 4 == 0 { 1 << 10 } else { 0 };
    let bank = ((tx / 8 + ty / 8) % 2) as u16;
    tile | hflip | bank << 12
}

/// BG tiles 1-3, 16 halfwords each. Built at compile time: computed at
/// run time, set-up took longer than the first frame (each pixel's `%` a
/// call into the division routine), so the display came on a frame late.
const BG_TILES: [u16; 48] = {
    let mut out = [0; 48];
    let mut i = 0;
    while i < 48 {
        let (t, y, h) = (1 + i / 16, i % 16 / 2, i % 2);
        let x = h * 4;
        out[i] = quad([
            bg_pixel(t, x, y),
            bg_pixel(t, x + 1, y),
            bg_pixel(t, x + 2, y),
            bg_pixel(t, x + 3, y),
        ]);
        i += 1;
    }
    out
};

/// The sprite's four tiles, 16x16 in 1D order, 16 halfwords each.
const OBJ_TILES: [u16; 64] = {
    let mut out = [0; 64];
    let mut i = 0;
    while i < 64 {
        let (t, y, h) = (i / 16, i % 16 / 2, i % 2);
        let x = t % 2 * 8 + h * 4;
        let y = t / 2 * 8 + y;
        out[i] = quad([
            obj_pixel(x, y),
            obj_pixel(x + 1, y),
            obj_pixel(x + 2, y),
            obj_pixel(x + 3, y),
        ]);
        i += 1;
    }
    out
};

/// BG0's map, 32x32 entries.
const MAP: [u16; 1024] = {
    let mut out = [0; 1024];
    let mut i = 0;
    while i < 1024 {
        out[i] = map_entry(i % 32, i / 32);
        i += 1;
    }
    out
};

/// `src` to the halfwords from `addr` up.
fn copy16(addr: usize, src: &[u16]) {
    for (i, &v) in src.iter().enumerate() {
        store16(addr + i * 2, v);
    }
}

/// Tiles, map, palettes and OAM; nothing shown yet (DISPCNT stays 0).
fn setup() {
    // BG palettes 0 and 1, colours 1-6 (colour 0 of bank 0 is the backdrop,
    // written at the first commit).
    copy16(PAL + 2, &[0x001F, 0x03E0, 0x7C00, 0x7FFF, 0x1084, 0x03FF]);
    copy16(PAL + 34, &[0x7C1F, 0x7FE0, 0x0210, 0x4210, 0x0C63, 0x2A0A]);
    // OBJ palettes 0-2: colour 1 the ring's, colour 3 the eye's.
    const RING: [u16; 3] = [0x03FF, 0x7C1F, 0x7FE0];
    for (bank, &c) in RING.iter().enumerate() {
        store16(OBJ_PAL + (bank * 16 + 1) * 2, c);
        store16(OBJ_PAL + (bank * 16 + 3) * 2, 0x0421);
    }
    // BG tiles 1-3 at character base 0, the map at screen block 31, the
    // sprite at OBJ tiles 1-4.
    copy16(VRAM + 32, &BG_TILES);
    copy16(VRAM + 31 * 0x800, &MAP);
    copy16(OBJ_VRAM + 32, &OBJ_TILES);
    // Every sprite hidden; sprite 0 is set at each commit.
    for n in 0..128 {
        store16(OAM + n * 8, 1 << 9);
        store16(OAM + n * 8 + 2, 0);
        store16(OAM + n * 8 + 4, 0);
    }
}

/// What the game keeps from frame to frame.
struct State {
    /// The sprite's top-left in the world.
    x: i32,
    y: i32,
    /// The camera's top-left in the world.
    cx: i32,
    cy: i32,
    /// Facing left (the sprite drawn flipped).
    left: bool,
    /// The sprite's palette bank, 0-2; A steps it.
    bank: u16,
    /// Last frame's keys, for A's press.
    last: u16,
}

impl State {
    fn new() -> State {
        State {
            x: 112,
            y: 72,
            cx: 0,
            cy: 0,
            left: false,
            bank: 0,
            last: 0,
        }
    }

    /// One frame's move, from the keys held (1 = pressed).
    fn update(&mut self, keys: u16) {
        let speed = if keys & KEY_B != 0 { 2 } else { 1 };
        if keys & KEY_RIGHT != 0 {
            self.x += speed;
            self.left = false;
        }
        if keys & KEY_LEFT != 0 {
            self.x -= speed;
            self.left = true;
        }
        if keys & KEY_DOWN != 0 {
            self.y += speed;
        }
        if keys & KEY_UP != 0 {
            self.y -= speed;
        }
        self.x = self.x.clamp(0, WORLD - SIZE);
        self.y = self.y.clamp(0, WORLD - SIZE);
        self.cx = follow(self.cx, self.x, ZONE_X).clamp(0, WORLD - 240);
        self.cy = follow(self.cy, self.y, ZONE_Y).clamp(0, WORLD - 160);
        if keys & KEY_A != 0 && self.last & KEY_A == 0 {
            self.bank = (self.bank + 1) % 3;
        }
        self.last = keys;
    }

    /// The display registers, the sprite and its blink, from the state.
    fn commit(&self, frames: u32) {
        store16(PAL, BACKDROP);
        store16(BG0CNT, 31 << 8);
        store16(BG0HOFS, self.cx as u16);
        store16(BG0VOFS, self.cy as u16);
        let sx = (self.x - self.cx) as u16 & 0x1FF;
        let sy = (self.y - self.cy) as u16 & 0xFF;
        let flip = if self.left { 1 << 12 } else { 0 };
        store16(OAM, sy);
        store16(OAM + 2, sx | flip | 1 << 14);
        store16(OAM + 4, 1 | self.bank << 12);
        let blink = BLINK[(frames / 16 % 4) as usize];
        store16(OBJ_PAL + (self.bank as usize * 16 + 2) * 2, blink);
        store16(DISPCNT, BG0_ON | OBJ_ON | OBJ_1D);
    }
}

/// The camera's new edge on one axis: moved just enough to keep `p`'s
/// screen position inside `zone`.
fn follow(c: i32, p: i32, zone: (i32, i32)) -> i32 {
    if p - c > zone.1 {
        p - zone.1
    } else if p - c < zone.0 {
        p - zone.0
    } else {
        c
    }
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    setup();
    // SAFETY: the handler is kept for the whole program.
    let _counter = unsafe {
        agb::interrupt::add_interrupt_handler(agb::interrupt::Interrupt::VBlank, |_| FRAMES.step())
    };
    let vblank = agb::interrupt::VBlank::get();
    let mut s = State::new();
    loop {
        vblank.wait_for_vblank();
        let keys = !load16(KEYINPUT) & 0x3FF;
        s.update(keys);
        s.commit(FRAMES.get());
    }
}
