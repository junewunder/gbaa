//! agb/native-spike's `src/main.rs`, lifted by hand to what the lifter
//! should print (docs/native-port-design.md): no Machine, no registers,
//! no GBA addresses. State is Rust values, the hardware is `hw`'s fields,
//! the frame loop waits with `hw::wait_vblank`, and the V-blank handler is
//! a callback. Names and shapes follow the agb source, which a lifter
//! wouldn't have; what it would print instead is in
//! docs/research/native-agb-spike.md.

use std::convert::Infallible;

use runtime::hw::{self, BgCnt, DispCnt, Exit, Hw, Native, OamEntry};
use runtime::io::irq;
use runtime::video::Video;

/// The whole program's state.
pub struct Game {
    pub ram: Box<Ram>,
    pub hw: Hw<Game>,
}

impl Native for Game {
    type Host = crate::AnyHost;
    fn hw(&mut self) -> &mut Hw<Game> {
        &mut self.hw
    }
    /// No CPSR: written over the library, never in a critical section.
    fn irq_masked(&self) -> bool {
        false
    }
}

/// The program's globals (its `.bss`): one, the V-blank handler's count.
/// The play layout (the default, the design's `--layout packed`): fields
/// for Rust.
#[cfg(not(feature = "guest-layout"))]
#[derive(Default)]
pub struct Ram {
    pub frames: u32,
}

#[cfg(not(feature = "guest-layout"))]
impl Ram {
    pub fn new() -> Box<Ram> {
        Box::default()
    }
}

/// The check layout (`--features guest-layout`, the design's `--layout
/// guest`): the ROM's EWRAM then IWRAM, each global at its link address
/// (agb/native-spike's ELF: `FRAMES` at `0x0200_00C0`) and padding
/// between, so its bytes are the play build's RAM. The code above reads
/// and writes the same field names in either layout. Assumes a
/// little-endian host, as the GBA is.
///
/// agb's own globals (`GLOBAL_ALLOC`, `INTERRUPT_TABLE`, `NUM_VBLANKS`
/// and its heap) aren't here: the program replaced agb's allocator and
/// interrupt list with the library's (docs/research/native-agb-spike.md,
/// rule 1), so they are the padding and the check skips them by name.
#[cfg(feature = "guest-layout")]
#[derive(zerocopy::IntoBytes, zerocopy::FromZeros, zerocopy::Immutable)]
#[repr(C)]
pub struct Ram {
    _below_frames: [u8; 0xC0],
    pub frames: u32,
    _ewram: [u8; 0x4_0000 - 0xC4],
    _iwram: [u8; 0x8000],
}

#[cfg(feature = "guest-layout")]
impl Ram {
    pub fn new() -> Box<Ram> {
        <Ram as zerocopy::FromZeros>::new_box_zeroed().expect("RAM's 288 KB")
    }
}

#[cfg(feature = "guest-layout")]
const _: () = {
    assert!(core::mem::offset_of!(Ram, frames) == 0xC0);
    assert!(core::mem::size_of::<Ram>() == 0x4_8000);
};

/// The check build's record of each frame ([`crate::host::record`]).
pub fn record(g: &mut Game) {
    #[cfg(feature = "guest-layout")]
    crate::host::record(&g.hw, zerocopy::IntoBytes::as_bytes(&*g.ram));
    #[cfg(not(feature = "guest-layout"))]
    let _ = g;
}

const KEY_A: u16 = 1 << 0;
const KEY_B: u16 = 1 << 1;
const KEY_RIGHT: u16 = 1 << 4;
const KEY_LEFT: u16 = 1 << 5;
const KEY_UP: u16 = 1 << 6;
const KEY_DOWN: u16 = 1 << 7;

const WORLD: i32 = 1024;
const SIZE: i32 = 16;
const ZONE_X: (i32, i32) = (64, 160);
const ZONE_Y: (i32, i32) = (40, 104);
const BACKDROP: u16 = 0x2108;
/// The OBJ palette's first colour in [`Video::color`]'s numbering.
const OBJ_PALETTE: usize = 256;
const BLINK: [u16; 4] = [0x7FFF, 0x5294, 0x318C, 0x5294];

// The ROM's constant data, as the lifter would print it: the bytes it
// copies, typed by how they're copied (halfwords), not the `const fn`s
// that made them at compile time.
const BG_PALETTE_0: [u16; 6] = [0x001F, 0x03E0, 0x7C00, 0x7FFF, 0x1084, 0x03FF];
const BG_PALETTE_1: [u16; 6] = [0x7C1F, 0x7FE0, 0x0210, 0x4210, 0x0C63, 0x2A0A];
const RING: [u16; 3] = [0x03FF, 0x7C1F, 0x7FE0];
const BG_TILES: [u16; 48] = tables::bg_tiles();
const OBJ_TILES: [u16; 64] = tables::obj_tiles();
const MAP: [u16; 1024] = tables::map();

/// `main`, from agb's entry on.
pub fn main(g: &mut Game) -> Result<Infallible, Exit> {
    crate::host::agb_crt0(
        g,
        &crate::host::Sections {
            ewram: (0x0802_58D4, 0xB8),
            iwram: (0x0802_56A8, 0x22C),
        },
    )?;
    setup(&mut g.hw.ppu);
    // agb's `VBlank::get` and `add_interrupt_handler`.
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, vblank);
    let mut s = State::new();
    loop {
        hw::wait_vblank(g)?;
        let keys = g.hw.keys.held();
        s.update(keys);
        s.commit(&mut g.hw.ppu, g.ram.frames);
    }
}

/// The V-blank handler.
fn vblank(g: &mut Game) {
    g.ram.frames += 1;
}

fn setup(v: &mut Video) {
    v.copy_colors(1, &BG_PALETTE_0);
    v.copy_colors(17, &BG_PALETTE_1);
    for (bank, &c) in RING.iter().enumerate() {
        v.set_color(OBJ_PALETTE + bank * 16 + 1, c);
        v.set_color(OBJ_PALETTE + bank * 16 + 3, 0x0421);
    }
    v.copy_vram16(Video::tile4(0, 1), &BG_TILES);
    v.copy_vram16(Video::screen_block(31), &MAP);
    v.copy_vram16(Video::tile4(4, 1), &OBJ_TILES);
    for i in 0..128 {
        let hidden = OamEntry {
            attr0: 1 << 9,
            ..v.obj(i)
        };
        v.set_obj(i, hidden);
    }
}

struct State {
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    left: bool,
    bank: u16,
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

    fn commit(&self, v: &mut Video, frames: u32) {
        v.set_color(0, BACKDROP);
        v.bg[0].cnt = BgCnt::new().screen_base(31);
        v.bg[0].scroll_x = self.cx as u16;
        v.bg[0].scroll_y = self.cy as u16;
        let sx = (self.x - self.cx) as u16 & 0x1FF;
        let sy = (self.y - self.cy) as u16 & 0xFF;
        let flip = if self.left { 1 << 12 } else { 0 };
        let mut o = v.obj(0);
        o.attr0 = sy;
        o.attr1 = sx | flip | 1 << 14;
        // The planted wrong store (`--features plant`): one pixel off
        // while facing left. The check must catch it.
        if cfg!(feature = "plant") && self.left {
            o.attr1 = (sx + 1) & 0x1FF | flip | 1 << 14;
        }
        o.attr2 = 1 | self.bank << 12;
        v.set_obj(0, o);
        v.set_color(
            OBJ_PALETTE + self.bank as usize * 16 + 2,
            BLINK[(frames / 16 % 4) as usize],
        );
        v.dispcnt = DispCnt::MODE_0 | DispCnt::BG0 | DispCnt::OBJ | DispCnt::OBJ_1D;
    }
}

fn follow(c: i32, p: i32, zone: (i32, i32)) -> i32 {
    if p - c > zone.1 {
        p - zone.1
    } else if p - c < zone.0 {
        p - zone.0
    } else {
        c
    }
}

/// The ROM's tables. A lifter reads these bytes out of the ROM; written
/// by hand here, they are agb/native-spike's `const fn`s, run at compile
/// time, so they are the same bytes.
mod tables {
    const fn quad(p: [u16; 4]) -> u16 {
        p[0] | p[1] << 4 | p[2] << 8 | p[3] << 12
    }

    const fn bg_pixel(t: usize, x: usize, y: usize) -> u16 {
        match t {
            1 => 1 + ((x + y) / 2 % 3) as u16,
            2 => {
                if x == 0 || y == 0 {
                    4
                } else {
                    5
                }
            }
            _ => {
                if x == 5 && y == 2 {
                    4
                } else {
                    6
                }
            }
        }
    }

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

    const fn map_entry(tx: usize, ty: usize) -> u16 {
        let tile = match (tx * 7 + ty * 3) % 5 {
            0 | 1 => 1,
            2 | 3 => 2,
            _ => 3,
        };
        let hflip = if (tx + ty).is_multiple_of(4) {
            1 << 10
        } else {
            0
        };
        let bank = ((tx / 8 + ty / 8) % 2) as u16;
        tile | hflip | bank << 12
    }

    pub const fn bg_tiles() -> [u16; 48] {
        let mut out = [0; 48];
        let mut i = 0;
        while i < 48 {
            let (t, y, x) = (1 + i / 16, i % 16 / 2, i % 2 * 4);
            out[i] = quad([
                bg_pixel(t, x, y),
                bg_pixel(t, x + 1, y),
                bg_pixel(t, x + 2, y),
                bg_pixel(t, x + 3, y),
            ]);
            i += 1;
        }
        out
    }

    pub const fn obj_tiles() -> [u16; 64] {
        let mut out = [0; 64];
        let mut i = 0;
        while i < 64 {
            let t = i / 16;
            let x = t % 2 * 8 + i % 2 * 4;
            let y = t / 2 * 8 + i % 16 / 2;
            out[i] = quad([
                obj_pixel(x, y),
                obj_pixel(x + 1, y),
                obj_pixel(x + 2, y),
                obj_pixel(x + 3, y),
            ]);
            i += 1;
        }
        out
    }

    pub const fn map() -> [u16; 1024] {
        let mut out = [0; 1024];
        let mut i = 0;
        while i < 1024 {
            out[i] = map_entry(i % 32, i / 32);
            i += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::hw::Host;
    use runtime::platform::Frame;

    /// Right held from the start, for `frames` frames.
    struct Right {
        left: u32,
    }

    impl Host for Right {
        fn present(&mut self, _frame: &Frame) -> Result<(), Exit> {
            self.left -= 1;
            if self.left > 0 { Ok(()) } else { Err(Exit) }
        }

        fn keys(&mut self) -> u16 {
            KEY_RIGHT
        }
    }

    #[test]
    fn right_moves_the_sprite_then_the_camera() {
        let mut g = Box::new(Game {
            ram: Ram::new(),
            hw: Hw::new(crate::AnyHost(Box::new(Right { left: 100 }))),
        });
        let Err(Exit) = main(&mut g);
        // 99 commits, each a pixel right: x 112 + 99 = 211; the camera
        // keeps it at screen x 160, the dead zone's right edge.
        assert_eq!(g.hw.ppu.obj(0).attr1 & 0x1FF, 160);
        assert_eq!(g.hw.ppu.bg[0].scroll_x, 211 - 160);
        // The handler ran after each present but the last.
        assert_eq!(g.ram.frames, 99);
    }
}
