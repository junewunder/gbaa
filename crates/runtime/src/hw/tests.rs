extern crate std;

use super::*;
use crate::platform::WIDTH;
use crate::video::{OBJ_VRAM, Video};
use std::boxed::Box;
use std::vec::Vec;

/// A host that records each frame's hash (or a few pixels) and plays keys
/// from a list, stopping after `frames` presents.
struct Rec {
    frames: Vec<Box<Frame>>,
    stop_after: usize,
    keys: Vec<u16>,
}

impl Host for Rec {
    fn present(&mut self, frame: &Frame) -> Result<(), Exit> {
        self.frames.push(Box::new(*frame));
        if self.frames.len() >= self.stop_after {
            return Err(Exit);
        }
        Ok(())
    }
    fn keys(&mut self) -> u16 {
        self.keys.get(self.frames.len() - 1).copied().unwrap_or(0)
    }
}

/// The smallest native program: its RAM, and the hardware.
struct Game {
    ram: Ram,
    hw: Hw<Game>,
    /// The CPSR's I bit, for the tests of a held IRQ.
    masked: bool,
}

#[derive(Default)]
struct Ram {
    vblanks: u32,
    hblanks: u32,
    vcounts: u32,
    seen_line: u16,
    wave: Vec<u16>,
}

impl Native for Game {
    type Host = Rec;
    fn hw(&mut self) -> &mut Hw<Game> {
        &mut self.hw
    }
    fn irq_masked(&self) -> bool {
        self.masked
    }
}

fn game(stop_after: usize) -> Box<Game> {
    Box::new(Game {
        masked: false,
        ram: Ram::default(),
        hw: Hw::new(Rec {
            frames: Vec::new(),
            stop_after,
            keys: Vec::new(),
        }),
    })
}

fn px(f: &Frame, x: usize, y: usize) -> u16 {
    f[y * WIDTH + x]
}

/// agb/template's `scene()`, as a native program writes it: fields only.
fn template_scene(v: &mut Video) {
    v.dispcnt = DispCnt::FORCED_BLANK;
    v.set_color(0, 0x4210);
    v.set_color(1, 0x001F);
    v.set_color(256 + 1, 0x03E0);
    v.fill_vram16(Video::tile4(0, 1), 16, 0x1111);
    v.fill_vram16(Video::screen_block(31), 32 * 32, 0);
    for ty in 0..4 {
        for tx in 0..4 {
            v.set_vram16(Video::screen_block(31) + (ty * 32 + tx) * 2, 1);
        }
    }
    v.bg[0].cnt = BgCnt::new().screen_base(31);
    for t in 1..5 {
        v.fill_vram16(Video::tile4(4, t), 16, 0x1111);
    }
    for n in 0..128 {
        v.set_obj(
            n,
            OamEntry {
                attr0: 1 << 9,
                ..OamEntry::default()
            },
        );
    }
    let obj = |a0, a1, a2| OamEntry {
        attr0: a0,
        attr1: a1,
        attr2: a2,
        affine: 0,
    };
    v.set_obj(0, obj(16, 16, 1));
    v.set_obj(1, obj(60, 100 | 1 << 14, 1));
    v.dispcnt = DispCnt::MODE_0 | DispCnt::BG0 | DispCnt::OBJ | DispCnt::OBJ_1D;
}

/// agb/template/SCENE.md, every pixel: red inside (0..31, 0..31), green
/// inside sprite 0's and sprite 1's boxes, grey elsewhere; and its table.
#[test]
fn template_scene_from_fields_alone() {
    let mut g = game(10);
    template_scene(&mut g.hw.ppu);
    let stopped = (|| -> Result<(), Exit> {
        loop {
            wait_vblank(&mut *g)?;
        }
    })();
    assert_eq!(stopped, Err(Exit));
    let frames = &g.hw.host.frames;
    assert_eq!(frames.len(), 10);
    let f = &frames[9];
    let (red, green, grey) = (0x001F, 0x03E0, 0x4210);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let want = if (16..24).contains(&x) && (16..24).contains(&y)
                || (100..116).contains(&x) && (60..76).contains(&y)
            {
                green
            } else if x < 32 && y < 32 {
                red
            } else {
                grey
            };
            assert_eq!(px(f, x, y), want, "pixel ({x}, {y})");
        }
    }
    for (x, y, c) in [
        (0, 0, 0x001F),
        (31, 31, 0x001F),
        (32, 0, 0x4210),
        (0, 32, 0x4210),
        (16, 16, 0x03E0),
        (23, 23, 0x03E0),
        (24, 24, 0x001F),
        (100, 60, 0x03E0),
        (115, 75, 0x03E0),
        (116, 76, 0x4210),
        (239, 159, 0x4210),
    ] {
        assert_eq!(px(f, x, y), c, "SCENE.md's ({x}, {y})");
    }
    // The scene never changes: every frame is the same picture.
    assert!(frames.iter().all(|fr| fr[..] == f[..]));
}

/// The play build's renderer, on the same scene stored at addresses
/// through the bus, draws the same frame.
#[test]
fn same_frame_as_the_play_build() {
    use crate::test_util::ppu::{OAM, OBJ_PAL, PAL, VRAM, fill16, render};
    let mut g = game(1);
    template_scene(&mut g.hw.ppu);
    assert_eq!(wait_vblank(&mut *g), Err(Exit));
    let mut b = crate::test_util::ppu::bus();
    b.write16(0x0400_0000, 1 << 7);
    b.write16(PAL, 0x4210);
    b.write16(PAL + 2, 0x001F);
    b.write16(OBJ_PAL + 2, 0x03E0);
    fill16(&mut b, VRAM + 32, 16, 0x1111);
    for ty in 0..4 {
        for tx in 0..4 {
            b.write16(VRAM + 31 * 0x800 + (ty * 32 + tx) * 2, 1);
        }
    }
    b.write16(0x0400_0008, 31 << 8);
    fill16(&mut b, VRAM + OBJ_VRAM as u32 + 32, 64, 0x1111);
    for n in 0..128 {
        b.write16(OAM + n * 8, 1 << 9);
    }
    for (n, a) in [[16, 16, 1], [60, 100 | 1 << 14, 1]].iter().enumerate() {
        for (k, &h) in a.iter().enumerate() {
            b.write16(OAM + n as u32 * 8 + 2 * k as u32, h);
        }
    }
    b.write16(0x0400_0000, 0x1140);
    let p = render(&b);
    assert!(p.frame[..] == g.hw.host.frames[0][..]);
}

fn on_vblank(g: &mut Game) {
    g.ram.vblanks += 1;
    g.ram.seen_line = g.hw.line();
}

fn on_hblank(g: &mut Game) {
    g.ram.hblanks += 1;
}

/// The V-count handler moves BG0 down 8 lines from line 80 on.
fn on_vcount(g: &mut Game) {
    g.ram.vcounts += 1;
    g.hw.ppu.bg[0].scroll_y = 8;
}

#[test]
fn wait_vblank_runs_handlers_and_returns_at_line_160() {
    let mut g = game(100);
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, on_vblank);
    for n in 1..=3 {
        wait_vblank(&mut *g).unwrap();
        assert_eq!(g.hw.line(), 160);
        assert_eq!(g.ram.vblanks, n);
        assert_eq!(g.ram.seen_line, 160);
        assert_eq!(g.hw.host.frames.len(), n as usize);
    }
    // The handler acknowledged for it; nothing stays requested.
    assert_eq!(g.hw.irq.flags, 0);
}

/// With the CPSR's I bit set a wait wakes but runs no handler: V-blank
/// is held in `IF` until the program clears I and calls `irq_release`,
/// which runs it at once, at the line the beam is on (native/lift-irq-hold).
#[test]
fn a_masked_wait_wakes_without_dispatch_and_release_runs_it() {
    let mut g = game(100);
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, on_vblank);
    g.masked = true;
    // A poll past line 160 inside the critical section.
    while g.hw.line() != 170 {
        idle(&mut *g).unwrap();
    }
    assert_eq!(g.ram.vblanks, 0, "no handler while I is set");
    assert_eq!(g.hw.irq.flags, irq::VBLANK, "held in IF");
    // Releasing with I still set runs nothing either.
    irq_release(&mut *g);
    assert_eq!(g.ram.vblanks, 0);
    g.masked = false;
    irq_release(&mut *g);
    assert_eq!((g.ram.vblanks, g.ram.seen_line), (1, 170));
    assert_eq!(g.hw.irq.flags, 0);
    assert_eq!(g.hw.line(), 170, "a release moves no time");
}

/// With `IME` clear a V-blank waits in `IF`; writing `IME` back runs it
/// at once, at the line the beam is on, and a write that leaves `IME` on
/// runs nothing (native/ime-release).
#[test]
fn setting_ime_again_runs_what_is_pending() {
    let mut g = game(100);
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, on_vblank);
    write_ime(&mut *g, 0);
    while g.hw.line() != 170 {
        idle(&mut *g).unwrap();
    }
    assert_eq!((g.ram.vblanks, g.hw.irq.flags), (0, irq::VBLANK));
    // With I set, IME's return runs nothing yet.
    g.masked = true;
    write_ime(&mut *g, 1);
    assert_eq!(g.ram.vblanks, 0);
    g.masked = false;
    write_ime(&mut *g, 0);
    write_ime(&mut *g, 1);
    assert_eq!((g.ram.vblanks, g.ram.seen_line), (1, 170));
    assert_eq!(g.hw.line(), 170, "moves no time");
    // IME already on: nothing new.
    g.hw.irq.flags = irq::VBLANK;
    write_ime(&mut *g, 1);
    assert_eq!(g.ram.vblanks, 1);
}

/// `wait_irq` with I set returns when one of its sources is requested
/// (the CPU wakes), where unmasked it waits for the handler; it must not
/// hang. `halt` likewise.
#[test]
fn wait_irq_and_halt_with_i_set_return_on_wake() {
    let mut g = game(100);
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, on_vblank);
    g.masked = true;
    wait_irq(&mut *g, irq::VBLANK).unwrap();
    assert_eq!(g.hw.line(), 160);
    assert_eq!((g.ram.vblanks, g.hw.irq.flags), (0, irq::VBLANK));
    g.masked = false;
    irq_release(&mut *g);
    assert_eq!(g.ram.vblanks, 1);
    // Masked again: halt wakes at the next request, V-blank's.
    g.masked = true;
    halt(&mut *g).unwrap();
    assert_eq!(g.hw.line(), 160);
    assert_eq!((g.ram.vblanks, g.hw.host.frames.len()), (1, 2));
    // Unmasked, the same wait runs the handler where it comes due.
    g.masked = false;
    irq_release(&mut *g);
    wait_irq(&mut *g, irq::VBLANK).unwrap();
    assert_eq!((g.ram.vblanks, g.hw.line()), (3, 160));
}

#[test]
fn no_handler_without_its_dispstat_bit() {
    let mut g = game(100);
    g.hw.irq.on(irq::VBLANK, on_vblank);
    wait_vblank(&mut *g).unwrap();
    assert_eq!(g.ram.vblanks, 0);
    assert_eq!(g.hw.host.frames.len(), 1);
}

/// A mid-frame register change from a V-count handler shows from the
/// next line on, as the play build draws it.
#[test]
fn lines_are_drawn_from_the_beam() {
    let mut g = game(100);
    let v = &mut g.hw.ppu;
    v.set_color(1, 0x001F);
    v.set_color(2, 0x03E0);
    // Tile 1 red, tile 2 green; map rows alternate every tile row.
    v.fill_vram16(Video::tile4(0, 1), 16, 0x1111);
    v.fill_vram16(Video::tile4(0, 2), 16, 0x2222);
    for ty in 0..32 {
        let t = 1 + (ty % 2) as u16;
        v.fill_vram16(Video::screen_block(31) + ty * 64, 32, t);
    }
    v.bg[0].cnt = BgCnt::new().screen_base(31);
    v.dispcnt = DispCnt::MODE_0 | DispCnt::BG0;
    g.hw.stat.vcount_irq = true;
    g.hw.stat.vcount_line = 80;
    g.hw.irq.on(irq::VCOUNT, on_vcount);
    wait_vblank(&mut *g).unwrap();
    let f = &g.hw.host.frames[0];
    // Lines 0-79: unscrolled (line 64 is in tile row 8, red; 79 in row 9,
    // green); line 80 on: 8 lines down (line 80 shows row 11, green, where
    // unscrolled it would show row 10, red).
    assert_eq!(px(f, 0, 64), 0x001F);
    assert_eq!(px(f, 0, 79), 0x03E0);
    assert_eq!(px(f, 0, 80), 0x03E0);
    assert_eq!(px(f, 0, 88), 0x001F);
    assert_eq!(g.ram.vcounts, 1);
    // The handler's change holds into the next frame, from line 0.
    g.ram.vcounts = 0;
    g.hw.ppu.bg[0].scroll_y = 0;
    wait_vblank(&mut *g).unwrap();
    assert_eq!(px(&g.hw.host.frames[1], 0, 64), 0x001F);
    assert_eq!(px(&g.hw.host.frames[1], 0, 80), 0x03E0);
    assert_eq!(g.ram.vcounts, 1);
}

fn wave(g: &Game, i: usize) -> u16 {
    g.ram.wave[i % 160]
}

/// An H-blank DMA to `BG0HOFS` from a table in the game's RAM: line
/// `y + 1` is drawn with entry `y`.
#[test]
fn hblank_dma_to_a_register() {
    let mut g = game(100);
    let v = &mut g.hw.ppu;
    v.set_color(1, 0x001F);
    // Tile 1's rows: pixels 0 and 1 red, the rest transparent.
    let row: Vec<u16> = (0..16)
        .map(|i| if i % 2 == 0 { 0x0011 } else { 0 })
        .collect();
    v.copy_vram16(Video::tile4(0, 1), &row);
    v.fill_vram16(Video::screen_block(31), 32 * 32, 1);
    v.bg[0].cnt = BgCnt::new().screen_base(31);
    v.dispcnt = DispCnt::MODE_0 | DispCnt::BG0;
    g.ram.wave = (0..160).map(|y| (y % 8) as u16).collect();
    g.hw.dma.hblank_to(0, reg::BG0HOFS, wave);
    wait_vblank(&mut *g).unwrap();
    let f = &g.hw.host.frames[0];
    // Line 0: scroll 0, pixels 0-1 red, 2-7 backdrop.
    assert_eq!(px(f, 0, 0), 0x001F);
    assert_eq!(px(f, 2, 0), 0);
    // Line 2: entry 1, scroll 1: pixel 0 is the tile's pixel 1, red; 1 isn't.
    assert_eq!(px(f, 0, 2), 0x001F);
    assert_eq!(px(f, 1, 2), 0);
    // Line 3: scroll 2: pixel 0 is the tile's pixel 2.
    assert_eq!(px(f, 0, 3), 0);
    assert_eq!(px(f, 6, 3), 0x001F);
}

#[test]
fn idle_and_wait_irq_stop_at_handlers() {
    let mut g = game(100);
    g.hw.stat.hblank_irq = true;
    g.hw.irq.on(irq::HBLANK, on_hblank);
    // Halt wakes at the first event whose handler runs: line 0's H-blank.
    halt(&mut *g).unwrap();
    assert_eq!((g.hw.line(), g.ram.hblanks), (0, 1));
    g.hw.stat.vcount_irq = true;
    g.hw.stat.vcount_line = 100;
    g.hw.irq.on(irq::VCOUNT, on_vcount);
    wait_irq(&mut *g, irq::VCOUNT).unwrap();
    assert_eq!((g.hw.line(), g.ram.vcounts), (100, 1));
    assert_eq!(g.ram.hblanks, 100);
    // Nothing enabled: halt runs to V-blank.
    g.hw.stat = DispStat::default();
    halt(&mut *g).unwrap();
    assert_eq!(g.hw.line(), 160);
    // An idle back edge moves half a line: to H-blank, then the next line.
    idle(&mut *g).unwrap();
    assert_eq!((g.hw.line(), g.hw.dispstat() & 2), (160, 2));
    idle(&mut *g).unwrap();
    assert_eq!((g.hw.line(), g.hw.dispstat() & 2), (161, 0));
}

#[test]
fn a_vcount_poll_finishes() {
    let mut g = game(100);
    let r: Result<(), Exit> = (|| {
        while g.hw.line() != 100 {
            idle(&mut *g)?;
        }
        Ok(())
    })();
    r.unwrap();
    assert_eq!(g.hw.line(), 100);
    assert_eq!(g.hw.read16(reg::VCOUNT), 100);
    assert!(g.hw.host.frames.is_empty());
    // Past 227 the beam wraps through V-blank.
    for _ in 0..2 * 228 {
        idle(&mut *g).unwrap();
    }
    assert_eq!((g.hw.line(), g.hw.host.frames.len()), (100, 1));
}

/// The master handler sees `IF`, acknowledges, and runs once for two
/// sources due together (line 160 with V-count 160).
#[test]
fn master_handler_sees_if() {
    fn intr_main(g: &mut Game) {
        let f = g.hw.irq.flags;
        g.ram.seen_line = f;
        g.ram.vblanks += 1;
        g.hw.irq.acknowledge(f);
    }
    let mut g = game(100);
    g.hw.irq.set_handler(intr_main);
    g.hw.write16(reg::DISPSTAT, 1 << 3 | 1 << 5 | 160 << 8);
    g.hw.write16(reg::IE, irq::VBLANK | irq::VCOUNT);
    g.hw.write16(reg::IME, 1);
    wait_vblank(&mut *g).unwrap();
    assert_eq!(g.ram.seen_line, irq::VBLANK | irq::VCOUNT);
    assert_eq!(g.ram.vblanks, 1);
    assert_eq!(g.hw.read16(reg::IF), 0);
    assert_eq!(
        g.hw.read16(reg::DISPSTAT),
        1 | 1 << 2 | 1 << 3 | 1 << 5 | 160 << 8
    );
}

/// Keys latched at each present are the host's for the frame after.
#[test]
fn keys_latch_at_the_present() {
    let mut g = game(100);
    g.hw.host.keys = std::vec![key::A, key::A | key::LEFT];
    wait_vblank(&mut *g).unwrap();
    assert_eq!(g.hw.keys.held(), key::A);
    assert_eq!(g.hw.read16(reg::KEYINPUT), 0x3FE);
    wait_vblank(&mut *g).unwrap();
    assert_eq!(g.hw.keys.held(), key::A | key::LEFT);
}

#[test]
fn io_writes_by_offset_reach_the_fields() {
    let mut g = game(100);
    g.hw.write16(reg::BG0HOFS + 4, 0xFFFF);
    assert_eq!(g.hw.ppu.bg[1].scroll_x, 0x1FF);
    g.hw.write16(reg::DISPCNT, 0x1140);
    assert_eq!(
        g.hw.ppu.dispcnt,
        DispCnt::MODE_0 | DispCnt::BG0 | DispCnt::OBJ | DispCnt::OBJ_1D
    );
    assert_eq!(g.hw.read16(reg::BG0HOFS), 0);
    assert_eq!(g.hw.read16(reg::DISPCNT), 0x1140);
}

#[test]
#[should_panic(expected = "isn't modelled")]
fn unmodelled_registers_are_loud() {
    let mut g = game(100);
    g.hw.write16(reg::SIOCNT, 1);
}

/// Polls on each of `DISPSTAT`'s flags finish.
#[test]
fn dispstat_polls_finish() {
    let mut g = game(100);
    let r: Result<(), Exit> = (|| {
        while g.hw.dispstat() & 2 == 0 {
            idle(&mut *g)?;
        }
        assert_eq!(g.hw.line(), 0);
        while g.hw.dispstat() & 2 != 0 {
            idle(&mut *g)?;
        }
        assert_eq!(g.hw.line(), 1);
        while g.hw.dispstat() & 1 == 0 {
            idle(&mut *g)?;
        }
        assert_eq!(g.hw.line(), 160);
        Ok(())
    })();
    r.unwrap();
}

/// One kind of handler at a time; a handler that disables another source
/// keeps it from running in the same dispatch.
#[test]
fn handler_kinds_replace_each_other_and_dispatch_rechecks() {
    fn master(g: &mut Game) {
        g.ram.vcounts += 100;
        let f = g.hw.irq.flags;
        g.hw.irq.acknowledge(f);
    }
    fn vblank_disables_vcount(g: &mut Game) {
        g.ram.vblanks += 1;
        g.hw.irq.enable &= !irq::VCOUNT;
    }
    let mut g = game(100);
    g.hw.irq.set_handler(master);
    g.hw.irq.on(irq::VBLANK, vblank_disables_vcount);
    g.hw.irq.on(irq::VCOUNT, on_vcount);
    g.hw.stat = DispStat {
        vblank_irq: true,
        vcount_irq: true,
        vcount_line: 160,
        ..DispStat::default()
    };
    wait_vblank(&mut *g).unwrap();
    assert_eq!((g.ram.vblanks, g.ram.vcounts), (1, 0));
}

/// `hw::spin` matches the play build's spin rule (`Sched::spins`): its
/// first SPIN_BUDGET passes leave the beam, then each moves it half a
/// line, so a loop waiting through calls reaches V-blank after
/// SPIN_BUDGET + 2 × 160 passes from line 0, as
/// `a_spin_loop_takes_time_past_its_budget` counts.
#[test]
fn spin_takes_half_a_line_past_its_budget() {
    // One loop: one call site.
    fn pass(g: &mut Game) {
        spin(g).unwrap();
    }
    let mut g = game(100);
    let mut passes = 0u32;
    while g.hw.frames() == 0 {
        pass(&mut g);
        passes += 1;
        if passes == crate::sched::SPIN_BUDGET {
            assert_eq!((g.hw.line(), g.hw.in_hblank), (0, false));
        }
    }
    assert_eq!(passes, crate::sched::SPIN_BUDGET + 2 * 160);
    assert_eq!(g.hw.line(), VBLANK_LINE);
    // The frame's end starts the count again.
    for _ in 0..crate::sched::SPIN_BUDGET {
        pass(&mut g);
    }
    assert_eq!((g.hw.line(), g.hw.in_hblank), (VBLANK_LINE, false));
}

#[test]
fn spend_moves_the_beam_by_cycles_through_its_events() {
    let mut g = game(10);
    g.hw.stat.hblank_irq = true;
    g.hw.irq.on(irq::HBLANK, |g: &mut Game| g.ram.hblanks += 1);
    // Short of the first H-blank: nothing happens.
    spend(&mut *g, HBLANK_DOT - 1).unwrap();
    assert_eq!((g.hw.line(), g.ram.hblanks), (0, 0));
    // One more cycle reaches it: drawn, its handler run.
    spend(&mut *g, 1).unwrap();
    assert_eq!((g.hw.line(), g.ram.hblanks), (0, 1));
    // Three lines on: their H-blanks' requests merge in `IF`, and one
    // handler runs after them, as the play build takes one IRQ after its
    // sync.
    spend(&mut *g, 3 * LINE_DOTS).unwrap();
    assert_eq!((g.hw.line(), g.ram.hblanks), (3, 2));
    // The play build's CpuSet price: 92 halfwords ROM to EWRAM at reset
    // wait states, as agb's crt0 copies its `.ewram`.
    assert_eq!(bios::cpu_set_cycles(0, 0x0800_0000, 0x0200_0000, 92), 1221);
}

#[test]
fn vblank_is_requested_before_the_present() {
    struct Seen(u16);
    struct G {
        hw: Hw<G>,
        seen: Seen,
    }
    impl Native for G {
        type Host = Rec;
        fn hw(&mut self) -> &mut Hw<G> {
            &mut self.hw
        }
        fn irq_masked(&self) -> bool {
            false
        }
    }
    let mut g = Box::new(G {
        hw: Hw::new(Rec {
            frames: Vec::new(),
            stop_after: 2,
            keys: Vec::new(),
        }),
        seen: Seen(0),
    });
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, |_| {});
    // What a check build records at the present: IF as the play build
    // holds it, V-blank requested, not yet handled.
    g.hw.on_frame_end(|g: &mut G| g.seen.0 = g.hw.irq.flags);
    wait_vblank(&mut *g).unwrap();
    assert_eq!(g.seen.0, irq::VBLANK);
    assert_eq!(g.hw.irq.flags, 0);
}

/// A program whose memory is EWRAM, the I/O registers and the palette:
/// what DMA by its registers moves through.
struct Mem {
    hw: Hw<Mem>,
    ewram: Vec<u8>,
}

impl Native for Mem {
    type Host = Rec;
    fn hw(&mut self) -> &mut Hw<Mem> {
        &mut self.hw
    }
    fn irq_masked(&self) -> bool {
        false
    }
    fn dma_load(&mut self, a: u32, word: bool) -> u32 {
        let half = |g: &mut Mem, a: u32| -> u32 {
            let o = (a & 0xFF_FFFF) as usize;
            match a >> 24 {
                2 => u16::from_le_bytes([g.ewram[o], g.ewram[o + 1]]) as u32,
                4 => g.hw.read16(o as u32) as u32,
                5 => u16::from_le_bytes([g.hw.ppu.palette[o], g.hw.ppu.palette[o + 1]]) as u32,
                _ => panic!("no memory at {a:#x}"),
            }
        };
        if word {
            half(self, a) | half(self, a + 2) << 16
        } else {
            half(self, a)
        }
    }
    fn dma_store(&mut self, a: u32, v: u32, word: bool) {
        let half = |g: &mut Mem, a: u32, v: u16| {
            let o = (a & 0xFF_FFFF) as usize;
            match a >> 24 {
                2 => g.ewram[o..o + 2].copy_from_slice(&v.to_le_bytes()),
                4 => write_dma(g, o as u32, v),
                5 => g.hw.ppu.palette[o..o + 2].copy_from_slice(&v.to_le_bytes()),
                _ => panic!("no memory at {a:#x}"),
            }
        };
        half(self, a, v as u16);
        if word {
            half(self, a + 2, (v >> 16) as u16);
        }
    }
}

fn mem(stop_after: usize) -> Box<Mem> {
    let mut g = Box::new(Mem {
        hw: Hw::new(Rec {
            frames: Vec::new(),
            stop_after,
            keys: Vec::new(),
        }),
        ewram: std::vec![0; 0x4_0000],
    });
    for i in 0..0x400 {
        g.ewram[i] = i as u8;
    }
    g
}

/// Store a channel's four registers as a program does: source, destination,
/// then count and control as one word (the control written last).
fn dma_set(g: &mut Mem, n: u32, src: u32, dst: u32, count: u16, control: u16) {
    let b = reg::DMA0SAD + 12 * n;
    for (o, v) in [
        (b, src),
        (b + 4, dst),
        (b + 8, count as u32 | (control as u32) << 16),
    ] {
        write_dma(g, o, v as u16);
        write_dma(g, o + 2, (v >> 16) as u16);
    }
}

use crate::dma::ctl as dctl;

/// An immediate DMA by its registers: the same bytes, price on the clock,
/// IRQ and registers after as the play build's bus.
#[test]
fn immediate_dma_by_registers_is_the_play_builds() {
    const EW: u32 = 0x0200_0000;
    let mut m = crate::test_util::machine();
    let mut g = mem(100);
    for i in 0..0x400 {
        m.bus.write8(EW + i, i as u8);
    }
    let io = crate::test_util::IO;
    // Words up, IRQ; then halfwords down from a fixed source to the
    // palette (a fill), the count 0 meaning the maximum is not used.
    let runs = [
        (3, EW, EW + 0x800, 16, dctl::ENABLE | dctl::WORD | dctl::IRQ),
        (
            0,
            EW + 6,
            0x0500_0020,
            8,
            dctl::ENABLE | 2 << dctl::SRC_SHIFT,
        ),
        (
            2,
            EW + 0x40,
            EW + 0x900,
            5,
            dctl::ENABLE | 1 << dctl::DST_SHIFT,
        ),
    ];
    for (n, src, dst, count, control) in runs {
        let (m0, g0) = (m.bus.io.now, g.hw.now());
        let b = io + reg::DMA0SAD + 12 * n;
        m.bus.write32(b, src);
        m.bus.write32(b + 4, dst);
        m.bus.write32(b + 8, count as u32 | (control as u32) << 16);
        dma_set(&mut g, n, src, dst, count, control);
        assert_eq!(m.bus.io.now - m0, g.hw.now() - g0, "channel {n}'s price");
        for o in (reg::DMA0SAD..reg::DMA0SAD + 48).step_by(2) {
            assert_eq!(m.bus.io.raw16(o), g.hw.raw16(o), "{o:#x}");
            assert_eq!(m.bus.io.read16(o), g.hw.read16(o), "{o:#x} read");
        }
    }
    assert_eq!(&m.bus.ewram[..0x1000], &g.ewram[..0x1000]);
    assert_eq!(m.bus.video.palette[..0x40], g.hw.ppu.palette[..0x40]);
    assert_eq!(m.bus.io.raw16(reg::IF), irq::DMA0 << 3);
    assert_eq!(g.hw.irq.flags, irq::DMA0 << 3);
    // Done and switched off: the enable bit reads clear.
    assert_eq!(g.hw.read16(reg::DMA0SAD + 36 + 10) & dctl::ENABLE, 0);
    assert_eq!(&g.ewram[0x800..0x840], &g.ewram[..0x40]);
}

/// H-blank and V-blank channels repeat at the beam's events: one unit a
/// visible line to a fixed palette entry, and a block reloaded at each
/// V-blank, each priced on the clock.
#[test]
fn hblank_and_vblank_dma_by_registers_repeat() {
    const EW: u32 = 0x0200_0000;
    let mut g = mem(100);
    let mut plain = mem(100);
    let hb = dctl::ENABLE | 2 << dctl::TIMING_SHIFT | dctl::REPEAT | 2 << dctl::DST_SHIFT;
    dma_set(&mut g, 0, EW, 0x0500_0000, 1, hb);
    let vb =
        dctl::ENABLE | 1 << dctl::TIMING_SHIFT | dctl::REPEAT | dctl::WORD | 3 << dctl::DST_SHIFT;
    dma_set(&mut g, 2, EW + 0x200, EW + 0x1000, 4, vb);
    // Nothing runs at the stores.
    assert_eq!(g.hw.ppu.palette[..2], [0, 0]);
    wait_vblank(&mut *g).unwrap();
    wait_vblank(&mut *plain).unwrap();
    // 160 H-blanks, a halfword each: the last read EWRAM 0x13E.
    assert_eq!(g.hw.ppu.palette[..2], [0x3E, 0x3F]);
    assert_eq!(g.hw.dma.regs.ch[0].src, EW + 320);
    assert_eq!(&g.ewram[0x1000..0x1010], &g.ewram[0x200..0x210]);
    // The destination reloads, the source goes on.
    let ch = g.hw.dma.regs.ch[2];
    assert_eq!((ch.src, ch.dst, ch.count), (EW + 0x210, EW + 0x1000, 4));
    // Each H-blank's price fits in its idle half line; V-blank's comes
    // after the line's events, as the play build adds it.
    let v = dma_cycles(0, 4, EW + 0x200, EW + 0x1000, 4) as u64;
    assert_eq!(g.hw.now() - plain.hw.now(), v);
    // Off: the channels stop.
    write_dma(&mut *g, reg::DMA0SAD + 10, 0);
    wait_vblank(&mut *g).unwrap();
    assert_eq!(g.hw.dma.regs.ch[0].src, EW + 320);
}

/// A special start aimed at a FIFO feeds it from the program's memory;
/// switched off, it stops.
#[test]
fn special_dma_by_registers_feeds_a_fifo() {
    const EW: u32 = 0x0200_0000;
    let mut g = mem(100);
    let c = dctl::ENABLE | 3 << dctl::TIMING_SHIFT | dctl::REPEAT | dctl::WORD | dctl::IRQ;
    dma_set(&mut g, 1, EW, crate::sound::FIFO_ADDR[1], 0, c);
    let d = g.hw.sound.dma[0].expect("DMA1 feeds a FIFO");
    assert_eq!((d.fifo, d.src, d.control), (1, EW, c));
    // Timer 0 clocks FIFO B; its first pop asks for 4 words.
    g.hw.write16(reg::SOUNDCNT_H, 0x3000);
    g.hw.write16(reg::SOUNDCNT_X, 0x80);
    g.hw.write16(reg::TM0CNT_L, 0xFF00);
    g.hw.write16(reg::TM0CNT_L + 2, crate::timer::ctl::ENABLE);
    spend(&mut *g, 0x100).unwrap();
    assert_eq!(g.hw.sound.dma[0].unwrap().src, EW + 16);
    assert_eq!(g.hw.dma.regs.ch[1].src, EW + 16);
    assert_eq!(g.hw.irq.flags & irq::DMA0 << 1, irq::DMA0 << 1);
    // A special start aimed elsewhere feeds nothing.
    write_dma(&mut *g, reg::DMA0SAD + 12 + 10, 0);
    assert!(g.hw.sound.dma[0].is_none());
    dma_set(&mut g, 2, EW, EW + 0x100, 4, c);
    assert!(g.hw.sound.dma[1].is_none());
}
