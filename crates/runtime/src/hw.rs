//! The GBA's hardware as a library for native programs
//! (docs/native-port-design.md, sections 2, 3 and 8): a game lifted to
//! plain Rust keeps its state in its own struct and talks to [`Hw`]
//! through typed fields and calls, not addresses.
//!
//! ```ignore
//! g.hw.ppu.dispcnt = DispCnt::MODE_0 | DispCnt::BG0 | DispCnt::OBJ | DispCnt::OBJ_1D;
//! g.hw.ppu.bg[0].cnt = BgCnt::new().priority(1).screen_base(31);
//! g.hw.ppu.bg[0].scroll_x = x;
//! g.hw.irq.on(irq::VBLANK, vblank);
//! loop {
//!     hw::wait_vblank(g)?;
//!     let keys = g.hw.keys.held();
//!     …
//! }
//! ```
//!
//! The picture comes from the play build's own renderer ([`Ppu`]) reading
//! the [`Video`] the game writes, line by line, so mid-frame changes show
//! as on hardware and the two builds can't differ by renderer.
//!
//! **Time** moves only where the game waits or looks at it (section 3).
//! The beam is a line number; each step ([`step`]) draws the current line
//! if it is visible, runs its H-blank (H-blank DMA, then the H-blank
//! handler), and moves to the next line, where a V-count match and, at
//! line 160, V-blank happen. At V-blank, V-blank DMA runs, the frame is
//! presented to the [`Host`] and the keys latched, then the handlers run.
//! The waits step until what they wait for:
//!
//! | wait | runs the beam until |
//! |------|---------------------|
//! | [`wait_vblank`] | line 160 has started and its handlers have run |
//! | [`wait_irq`]`(mask)` | a handler for a source in `mask` has run (I set: one is requested) |
//! | [`halt`] | any handler has run, or V-blank (I set: any is requested) |
//! | [`idle`] (a store-free busy loop's back edge, a poll on a flag or the beam) | half a line has passed ([`half_step`]) |
//! | [`spin`] (a loop that calls, past its budget) | half a line has passed |
//! | [`spend`]`(cycles)` (a BIOS copy's time, [`bios::cpu_set_cycles`]) | `cycles` have passed |
//!
//! A read of the beam (`VCOUNT`, `DISPSTAT`: [`Hw::line`], [`Hw::dispstat`])
//! moves nothing; a poll on it moves at its [`idle`]. These are the play
//! build's `--timing native` steps (docs/decisions.md 11).
//!
//! [`wait_vblank`] returns at line 160, after the V-blank handler, as
//! `VBlankIntrWait` does: code after it sees `VCOUNT` 160, and the next
//! wait runs lines 161-227 (their H-blank and V-count handlers), then
//! draws the next frame from line 0.
//!
//! **Handlers** are callbacks over the whole game, `fn(&mut G)`, run to
//! completion at those points, never nested. Two ways to install them:
//! the game's own master handler ([`Irq::set_handler`], what `IntrMain` at
//! `0x0300_7FFC` becomes), which reads [`Irq::flags`] and acknowledges
//! itself; or one per source ([`Irq::on`]), which the library calls
//! lowest-numbered source first and acknowledges for it. A source raises
//! its request only when enabled as on hardware: its `DISPSTAT` bit
//! ([`DispStat`]) for the display's three, and `IE` and `IME` for any
//! handler to run.
//!
//! **The CPSR's I bit** ([`Native::irq_masked`]) holds them as the
//! hardware does: while it is set, a request waits in `IF` and the waits
//! wake without running a handler ([`wait_irq`] and [`halt`] return when
//! one they wait for is requested); the program's write that clears I
//! calls [`irq_release`], which runs them at once.
//!
//! **Stopping.** [`Host::present`] may return [`Exit`]; every call that
//! can reach a V-blank returns it, and the game passes it up with `?`
//! (the design's `Reset` shape).
//!
//! **Sound** is its registers only ([`Sound`]): stored as written, so a
//! check can compare them, but not played (the play build's
//! [`crate::sound`] still reads the bus).
//!
//! What isn't here yet: timers, the sound mixer and its FIFOs, DMA by
//! address, serial, saves.

pub mod bios;

use crate::io::{self, irq, reg};
use crate::platform::{Frame, HEIGHT};
use crate::ppu::Ppu;
use crate::video::Video;

pub use crate::io::key;
pub use crate::video::{Affine, Bg, BgCnt, DispCnt, OamEntry, Window};

/// Cycles a line takes, and where in it H-blank starts (the play build's
/// scheduler's).
pub const LINE_DOTS: u32 = crate::sched::LINE_CYCLES as u32;
pub const HBLANK_DOT: u32 = crate::sched::HBLANK_START as u32;

/// Lines a frame, visible and V-blank.
pub const LINES: u16 = 228;
/// The first line of V-blank.
pub const VBLANK_LINE: u16 = HEIGHT as u16;

/// The program was told to stop (the host's frames ran out, the window
/// closed); `?` carries it up the stack from the wait it happened in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit;

/// The front end: where frames go and keys come from.
pub trait Host {
    /// A finished frame, at the start of V-blank. `Err(Exit)` stops the
    /// program.
    fn present(&mut self, frame: &Frame) -> Result<(), Exit>;
    /// The keys held now, as [`key`] bits (1 = pressed), latched right
    /// after [`Host::present`] for the frame that follows.
    fn keys(&mut self) -> u16;
}

/// A [`crate::Platform`] as a [`Host`] that never stops.
pub struct PlatformHost<P>(pub P);

impl<P: crate::Platform> Host for PlatformHost<P> {
    fn present(&mut self, frame: &Frame) -> Result<(), Exit> {
        self.0.present(frame);
        Ok(())
    }
    fn keys(&mut self) -> u16 {
        self.0.keys()
    }
}

/// A native program's state: whatever it is, it holds an [`Hw`] the
/// library's free functions reach through [`Native::hw`], and the host
/// type the library presents to.
pub trait Native: Sized {
    type Host: Host;
    fn hw(&mut self) -> &mut Hw<Self>;
    /// The CPSR's I bit: set, an IRQ that comes due is held in `IF` (the
    /// waits wake but run no handler) until the program clears it and
    /// calls [`irq_release`]. A program with no CPSR (one written by hand
    /// over the library) returns `false`.
    fn irq_masked(&self) -> bool;
}

/// The keys as the game reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    held: u16,
}

impl Keys {
    /// Held now, 1 = pressed (`KEYINPUT` inverted), latched at each
    /// V-blank.
    pub fn held(&self) -> u16 {
        self.held
    }

    /// `KEYINPUT` itself: active low.
    pub fn keyinput(&self) -> u16 {
        !self.held & key::ALL
    }
}

/// `DISPSTAT`'s writable half: which display interrupts are raised, and
/// the V-count line that matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DispStat {
    pub vblank_irq: bool,
    pub hblank_irq: bool,
    pub vcount_irq: bool,
    pub vcount_line: u8,
}

/// The interrupt controller and the game's handlers.
pub struct Irq<G> {
    /// `IE`: the sources whose requests reach a handler.
    pub enable: u16,
    /// `IME`.
    pub master_enable: bool,
    /// `IF`: requested and not yet acknowledged.
    pub flags: u16,
    master: Option<fn(&mut G)>,
    each: [Option<fn(&mut G)>; 14],
    /// Sources whose handler ran since the waits last looked.
    ran: u16,
}

impl<G> Default for Irq<G> {
    fn default() -> Irq<G> {
        Irq {
            enable: 0,
            master_enable: false,
            flags: 0,
            master: None,
            each: [None; 14],
            ran: 0,
        }
    }
}

impl<G> Irq<G> {
    /// The game's master handler (stored at `0x0300_7FFC`): called with
    /// [`Irq::flags`] set, it acknowledges what it handles
    /// ([`Irq::acknowledge`]). Replaces any per-source handlers, as
    /// [`Irq::on`] replaces a master handler.
    pub fn set_handler(&mut self, f: fn(&mut G)) {
        self.master = Some(f);
        self.each = [None; 14];
    }

    /// A handler for one source (an [`irq`] bit), as agb's
    /// `add_interrupt_handler` or a stock dispatch table installs it: also
    /// enables the source (`IE`, `IME`).
    pub fn on(&mut self, source: u16, f: fn(&mut G)) {
        debug_assert_eq!(source.count_ones(), 1, "one source at a time");
        self.master = None;
        self.each[source.trailing_zeros() as usize] = Some(f);
        self.enable |= source;
        self.master_enable = true;
    }

    /// Clear requests in `IF` (writing 1s to it).
    pub fn acknowledge(&mut self, bits: u16) {
        self.flags &= !bits;
    }
}

/// An H-blank DMA to a display register: at each visible line's H-blank
/// the library asks `src` for the next value (its index counts transfers
/// since [`Dma::hblank_to`]) and writes it to the register at `dst`.
/// `src` borrows the game when it runs, as the design's `Ref` would.
pub struct HblankDma<G> {
    pub dst: u32,
    pub src: fn(&G, usize) -> u16,
    pub next: usize,
}

/// The DMA channels' repeating kinds (section 8). Immediate copies are
/// plain copies into [`Video`] ([`Video::copy_vram16`] and friends).
pub struct Dma<G> {
    hblank: [Option<HblankDma<G>>; 4],
    vblank: [Option<fn(&mut G)>; 4],
}

impl<G> Default for Dma<G> {
    fn default() -> Dma<G> {
        Dma {
            hblank: [None, None, None, None],
            vblank: [None; 4],
        }
    }
}

impl<G> Dma<G> {
    /// Channel `n` repeats at each visible line's H-blank, writing
    /// `src(g, i)` to the display register at offset `dst` for the `i`th.
    pub fn hblank_to(&mut self, n: usize, dst: u32, src: fn(&G, usize) -> u16) {
        self.hblank[n] = Some(HblankDma { dst, src, next: 0 });
        self.vblank[n] = None;
    }

    /// Channel `n` repeats at each V-blank, before the frame is presented
    /// (an OAM copy from a shadow buffer).
    pub fn vblank(&mut self, n: usize, f: fn(&mut G)) {
        self.vblank[n] = Some(f);
        self.hblank[n] = None;
    }

    /// Channel `n` off.
    pub fn stop(&mut self, n: usize) {
        self.hblank[n] = None;
        self.vblank[n] = None;
    }
}

/// The sound registers, `SOUND1CNT_L` (offset `0x60`) to `SOUNDBIAS` and
/// wave RAM (`0x9E`), stored as written. Nothing plays them yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sound {
    regs: [u16; Sound::LEN],
}

impl Sound {
    /// The first register's offset.
    pub const START: u32 = 0x060;
    /// Past the last (wave RAM's end; the FIFOs at `0xA0` aren't here).
    pub const END: u32 = 0x0A0;
    const LEN: usize = (Sound::END - Sound::START) as usize / 2;

    /// As the BIOS leaves them: `SOUNDBIAS` 0x200, the rest zero.
    pub fn new() -> Sound {
        let mut s = Sound {
            regs: [0; Sound::LEN],
        };
        s.write16(reg::SOUNDBIAS, 0x200);
        s
    }

    /// The register at offset `o` (even, `START..END`) as written.
    pub fn read16(&self, o: u32) -> u16 {
        self.regs[(o - Sound::START) as usize / 2]
    }

    pub fn write16(&mut self, o: u32, v: u16) {
        self.regs[(o - Sound::START) as usize / 2] = v;
    }
}

impl Default for Sound {
    fn default() -> Sound {
        Sound::new()
    }
}

/// The hardware a native program drives.
pub struct Hw<G: Native> {
    /// The display's registers and memories, as fields.
    pub ppu: Video,
    pub stat: DispStat,
    pub keys: Keys,
    pub irq: Irq<G>,
    pub dma: Dma<G>,
    pub sound: Sound,
    pub host: G::Host,
    /// Run at each V-blank's start just before the present, after V-blank
    /// DMA ([`Hw::on_frame_end`]).
    frame_end: Option<fn(&mut G)>,
    /// The line the beam is on, 0-227.
    line: u16,
    /// Frames presented since power-on.
    frames: u64,
    /// The beam is in the current line's H-blank, as a `DISPSTAT` poll
    /// last saw it ([`dispstat`]).
    in_hblank: bool,
    /// Cycles into the current line, for [`spend`]: set to the event's own
    /// time at each half step.
    dot: u32,
    /// `WAITCNT`, stored: what [`bios::cpu_set_cycles`] prices a copy by.
    pub waitcnt: u16,
    /// The play build's renderer, and the frame it is drawing.
    renderer: Ppu,
    /// Each [`spin`] loop's passes since the beam last moved, by its call
    /// site (direct-mapped, as the play build's `Sched::spins`).
    spins: [(usize, u32); crate::sched::SPIN_SLOTS],
    /// The beam is moving for a stuck [`spin`]: the counts stay.
    spinning: bool,
}

impl<G: Native> Hw<G> {
    /// Power-on: memories zero, registers as the BIOS leaves them, the
    /// beam at line 0.
    pub fn new(host: G::Host) -> Hw<G> {
        Hw {
            ppu: Video::new(),
            stat: DispStat::default(),
            keys: Keys::default(),
            irq: Irq::default(),
            dma: Dma::default(),
            sound: Sound::new(),
            host,
            frame_end: None,
            line: 0,
            frames: 0,
            in_hblank: false,
            dot: 0,
            waitcnt: 0,
            renderer: Ppu::new(),
            spins: [(0, 0); crate::sched::SPIN_SLOTS],
            spinning: false,
        }
    }

    /// The beam's line, without moving it (the library's own view; a game
    /// reading `VCOUNT` calls [`vcount`]).
    pub fn line(&self) -> u16 {
        self.line
    }

    /// Put the beam on `line` (0-227), in its H-blank or not, without
    /// drawing or raising anything: for a check that starts the library
    /// from a state the play build recorded (`native-fn-diff`).
    pub fn set_beam(&mut self, line: u16, in_hblank: bool) {
        debug_assert!(line < LINES);
        self.line = line;
        self.in_hblank = in_hblank;
        self.dot = if in_hblank { HBLANK_DOT } else { 0 };
    }

    /// Call `f` at each V-blank's start, after V-blank DMA and just
    /// before the frame is presented ([`Hw::frames`] already counts it):
    /// what the play build holds when it presents, for a check build to record (the native check,
    /// `route-check --native`). Not for game code.
    pub fn on_frame_end(&mut self, f: fn(&mut G)) {
        self.frame_end = Some(f);
    }

    /// Frames presented since power-on.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The frame drawn so far: complete at a present.
    pub fn frame(&self) -> &Frame {
        &self.renderer.frame
    }

    /// Off: lines aren't drawn (rollback's or a fast-forward's frames).
    pub fn set_drawing(&mut self, on: bool) {
        self.renderer.draw = on;
    }

    /// `DISPSTAT` as the CPU reads it at the beam's line.
    pub fn dispstat(&self) -> u16 {
        let s = &self.stat;
        let y = self.line;
        ((VBLANK_LINE..LINES - 1).contains(&y) as u16)
            | (self.in_hblank as u16) << 1
            | ((y == s.vcount_line as u16) as u16) << 2
            | (s.vblank_irq as u16) << 3
            | (s.hblank_irq as u16) << 4
            | (s.vcount_irq as u16) << 5
            | (s.vcount_line as u16) << 8
    }

    /// The escape hatch (section 2): write the I/O register at offset `o`
    /// (even) as a store to `0x0400_0000 + o` would, for code that
    /// addresses registers through a table or a loop. The display
    /// registers, `DISPSTAT`, `IE`, `IF` (1s acknowledge) and `IME`;
    /// panics on a register the library doesn't model yet (timers, sound,
    /// DMA by address, serial), which a lifted program must not reach
    /// silently.
    pub fn write16(&mut self, o: u32, v: u16) {
        match o {
            reg::DISPSTAT => {
                self.stat = DispStat {
                    vblank_irq: v & (1 << 3) != 0,
                    hblank_irq: v & (1 << 4) != 0,
                    vcount_irq: v & (1 << 5) != 0,
                    vcount_line: (v >> 8) as u8,
                }
            }
            reg::VCOUNT | reg::KEYINPUT => {}
            _ if o < crate::video::REGS_END => self.ppu.write16(o, v),
            Sound::START..Sound::END => self.sound.write16(o, v),
            // Bit 15 is the cartridge type: read-only, 0 (GBA).
            reg::WAITCNT => self.waitcnt = v & 0x7FFF,
            reg::IE => self.irq.enable = v & 0x3FFF,
            reg::IF => self.irq.acknowledge(v),
            reg::IME => self.irq.master_enable = v & 1 != 0,
            _ => panic!("hw: I/O register {o:#05x} isn't modelled"),
        }
    }

    /// Read the I/O register at offset `o` (even) as a load would, without
    /// moving the beam (a game's `VCOUNT` poll calls [`vcount`]).
    pub fn read16(&self, o: u32) -> u16 {
        match o {
            reg::DISPSTAT => self.dispstat(),
            reg::VCOUNT => self.line,
            reg::KEYINPUT => self.keys.keyinput(),
            _ if o < crate::video::REGS_END => self.ppu.read16(o),
            Sound::START..Sound::END => self.sound.read16(o),
            reg::WAITCNT => self.waitcnt,
            reg::IE => self.irq.enable,
            reg::IF => self.irq.flags,
            reg::IME => self.irq.master_enable as u16,
            _ => panic!("hw: I/O register {o:#05x} isn't modelled"),
        }
    }

    /// The I/O halfword at offset `o` (even) as stored, write-only bits
    /// included (the play build's `Io::raw16`), for a check: `DISPSTAT`
    /// its writable bits, `VCOUNT` the beam's line, and 0 for the
    /// registers the library doesn't model.
    pub fn raw16(&self, o: u32) -> u16 {
        match o {
            reg::DISPSTAT => self.dispstat() & 0xFF38,
            reg::VCOUNT => self.line,
            _ if o < crate::video::REGS_END => self.ppu.raw16(o),
            Sound::START..Sound::END => self.sound.read16(o),
            reg::KEYINPUT => self.keys.keyinput(),
            reg::WAITCNT => self.waitcnt,
            reg::IE => self.irq.enable,
            reg::IF => self.irq.flags,
            reg::IME => self.irq.master_enable as u16,
            _ => 0,
        }
    }

    /// Request `bits` in `IF`, as the hardware does.
    fn request(&mut self, bits: u16) {
        self.irq.flags |= bits;
    }
}

/// Run the handlers of what's requested and enabled: the master handler
/// once, or each source's lowest-numbered first, acknowledged for it.
/// Nothing while the CPSR's I bit is set ([`Native::irq_masked`]): the
/// requests wait in `IF`, as on the hardware.
fn dispatch<G: Native>(g: &mut G) {
    if g.irq_masked() {
        return;
    }
    let irq = &mut g.hw().irq;
    let due = irq.enable & irq.flags;
    if !irq.master_enable || due == 0 {
        return;
    }
    irq.ran |= due;
    if let Some(f) = irq.master {
        f(g);
        return;
    }
    for n in 0..14 {
        let bit = 1 << n;
        let irq = &mut g.hw().irq;
        // An earlier handler may have disabled or acknowledged this one.
        if due & bit == 0 || irq.enable & irq.flags & bit == 0 || !irq.master_enable {
            continue;
        }
        irq.flags &= !bit;
        if let Some(f) = irq.each[n] {
            f(g);
        }
    }
}

/// The program cleared the CPSR's I bit (an `msr`, a critical section's
/// end): run the handlers of what came due while it was set, as the
/// hardware takes a held IRQ at once. Moves no time.
pub fn irq_release<G: Native>(g: &mut G) {
    dispatch(g);
}

/// Whether a source in `mask` is requested and enabled while the CPSR's I
/// bit holds it: what wakes a wait that can't run its handler.
fn woke_masked<G: Native>(g: &mut G, mask: u16) -> bool {
    if !g.irq_masked() {
        return false;
    }
    let irq = &g.hw().irq;
    irq.enable & irq.flags & mask != 0
}

/// Move the beam one line: draw the current line if visible, its H-blank
/// (H-blank DMA, the H-blank handler), then the next line's start (its
/// V-count match; at line 160, V-blank: V-blank DMA, the present, the
/// keys, then the handlers). Returns whether V-blank started.
pub fn step<G: Native>(g: &mut G) -> Result<bool, Exit> {
    if !g.hw().in_hblank {
        hblank_half(g);
    }
    line_half(g)
}

/// Move the beam half a line, to the next event the play build's
/// scheduler has: from a line's start to its H-blank ([`hblank_half`]),
/// or from H-blank to the next line's start ([`line_half`]). Returns
/// whether V-blank started.
pub fn half_step<G: Native>(g: &mut G) -> Result<bool, Exit> {
    if g.hw().in_hblank {
        return line_half(g);
    }
    hblank_half(g);
    Ok(false)
}

/// The beam moved: every [`spin`] loop counts again, but while a stuck
/// one moves it.
fn moved<G: Native>(hw: &mut Hw<G>) {
    if !hw.spinning {
        hw.spins = [(0, 0); crate::sched::SPIN_SLOTS];
    }
}

/// The current line's H-blank: draw it if visible, H-blank DMA, then the
/// H-blank handler.
fn hblank_half<G: Native>(g: &mut G) {
    let hw = g.hw();
    moved(hw);
    hw.in_hblank = true;
    hw.dot = HBLANK_DOT;
    let y = hw.line;
    if y < VBLANK_LINE {
        hw.renderer.render_line(&hw.ppu, y as usize);
        for n in 0..4 {
            let Some(d) = g.hw().dma.hblank[n].as_ref() else {
                continue;
            };
            let (dst, src, i) = (d.dst, d.src, d.next);
            let v = src(g, i);
            let hw = g.hw();
            hw.write16(dst, v);
            if let Some(d) = hw.dma.hblank[n].as_mut() {
                d.next += 1;
            }
        }
    }
    let hw = g.hw();
    if hw.stat.hblank_irq {
        hw.request(irq::HBLANK);
        dispatch(g);
    }
}

/// The next line's start: its V-count match; at line 160, V-blank
/// (V-blank DMA, the present, the keys, then the handlers). Returns
/// whether V-blank started.
fn line_half<G: Native>(g: &mut G) -> Result<bool, Exit> {
    let hw = g.hw();
    moved(hw);
    hw.in_hblank = false;
    hw.dot = 0;
    let y = (hw.line + 1) % LINES;
    hw.line = y;
    let mut due = 0;
    if hw.stat.vcount_irq && y == hw.stat.vcount_line as u16 {
        due |= irq::VCOUNT;
    }
    let vblank = y == VBLANK_LINE;
    if vblank && hw.stat.vblank_irq {
        due |= irq::VBLANK;
    }
    // Requested as the line starts, as the hardware does (a record at
    // the present sees `IF` as the play build has it); handled after.
    hw.request(due);
    if vblank {
        for n in 0..4 {
            if let Some(f) = g.hw().dma.vblank[n] {
                f(g);
            }
        }
        g.hw().frames += 1;
        if let Some(f) = g.hw().frame_end {
            f(g);
        }
        let hw = g.hw();
        let shown = hw.host.present(&hw.renderer.frame);
        hw.keys.held = hw.host.keys() & key::ALL;
        shown?;
    }
    if due != 0 {
        dispatch(g);
    }
    Ok(vblank)
}

/// `VBlankIntrWait`: the rest of the frame drawn from the beam's line,
/// presented, the keys latched, the V-blank handler run; returns at line
/// 160.
pub fn wait_vblank<G: Native>(g: &mut G) -> Result<(), Exit> {
    while !step(g)? {}
    Ok(())
}

/// `IntrWait` for the sources in `mask`: the beam runs until a handler
/// for one of them has run. With the CPSR's I bit set no handler can run,
/// so it returns when one of them is requested (the CPU wakes), its
/// handler still to come at [`irq_release`]. That departs from the
/// hardware on purpose: the BIOS's `IntrWait` keeps the caller's I bit and
/// loops on its flags, which no handler sets, so it would spin for ever.
pub fn wait_irq<G: Native>(g: &mut G, mask: u16) -> Result<(), Exit> {
    g.hw().irq.ran = 0;
    loop {
        step(g)?;
        if g.hw().irq.ran & mask != 0 || woke_masked(g, mask) {
            return Ok(());
        }
    }
}

/// The back edge of a busy loop with no stores and no calls (a poll on a
/// flag a handler sets, or on the beam: `while g.hw.line() != 100 {
/// hw::idle(g)?; }`): the beam moves half a line, as the play build's
/// `--timing native` runs to its next event, then the loop tests again.
/// A read of the beam itself moves nothing (`Hw::line`, `Hw::dispstat`).
pub fn idle<G: Native>(g: &mut G) -> Result<(), Exit> {
    half_step(g).map(|_| ())
}

/// The back edge of a loop that calls functions and may be waiting through
/// them for an interrupt (Emerald's `DoMapLoadLoop`,
/// `while !load_map_in_steps_local(g, state, false) { hw::spin(g)?; }`;
/// docs/research/busy-wait-loops.md): counted by call site. Past
/// `SPIN_BUDGET` passes with no time passing, each further pass moves the
/// beam half a line, until it moves some other way or the frame ends.
/// The play build's `--timing native` does the same at the same loops
/// (`armv4t::idle::is_spin_edge`).
#[track_caller]
pub fn spin<G: Native>(g: &mut G) -> Result<(), Exit> {
    let site = core::panic::Location::caller() as *const _ as usize;
    let hw = g.hw();
    let slot = &mut hw.spins[(site >> 3) % crate::sched::SPIN_SLOTS];
    if slot.0 != site {
        *slot = (site, 0);
    }
    slot.1 += 1;
    if slot.1 <= crate::sched::SPIN_BUDGET {
        return Ok(());
    }
    hw.spinning = true;
    let r = half_step(g);
    let hw = g.hw();
    hw.spinning = false;
    // A frame's end starts every count again.
    if matches!(r, Ok(true)) {
        hw.spins = [(0, 0); crate::sched::SPIN_SLOTS];
    }
    r.map(|_| ())
}

/// Work that takes time (a BIOS copy's, as the play build's `--timing
/// native` charges it, [`bios::cpu_set_cycles`]): the beam moves `cycles`
/// on, through every event it passes (drawing, H-blank, V-blank and their
/// handlers, as a wait would), and stops between them. The only time a
/// native program's work takes (docs/native-port-design.md, section 7).
pub fn spend<G: Native>(g: &mut G, cycles: u32) -> Result<(), Exit> {
    let mut left = cycles;
    loop {
        let hw = g.hw();
        let next = if hw.in_hblank { LINE_DOTS } else { HBLANK_DOT };
        let room = next - hw.dot;
        if left < room {
            hw.dot += left;
            return Ok(());
        }
        left -= room;
        half_step(g)?;
    }
}

/// `Halt`: the beam runs until a handler has run, or to V-blank if none
/// is enabled. With the CPSR's I bit set, until an enabled source is
/// requested (the CPU wakes; its handler waits for [`irq_release`]).
pub fn halt<G: Native>(g: &mut G) -> Result<(), Exit> {
    g.hw().irq.ran = 0;
    loop {
        let vblank = step(g)?;
        if vblank || g.hw().irq.ran != 0 || woke_masked(g, !0) {
            return Ok(());
        }
    }
}

/// `io::reg` re-exported for [`Hw::write16`]'s callers.
pub use io::reg as regs;

#[cfg(test)]
mod tests;
