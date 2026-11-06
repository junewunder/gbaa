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
//! It is the play build's clock ([`Hw::now`], cycles since power-on) and
//! moves as the play build's `--timing native` moves it: at each event,
//! [`sync`]'s work is the play build's `Machine::sync`'s. The events are
//! the beam's (a line's H-blank: the line drawn if visible, H-blank DMA;
//! the next line's start: its V-count match and, at line 160, V-blank:
//! V-blank DMA and its price, the frame's sound to [`Host::audio`], the
//! present, the keys) and the overflows of a timer something listens to
//! (its IRQ, a cascade, a sound FIFO). After each, the handlers of what's
//! requested run. The waits run event by event until what they wait for,
//! each starting with a sync where it is (the play build's halt store's):
//!
//! | wait | runs the clock until |
//! |------|---------------------|
//! | [`wait_vblank`] | line 160 has started and its handlers have run |
//! | [`wait_irq`]`(mask)` | a handler for a source in `mask` has run (I set: one is requested) |
//! | [`halt`] | any handler has run, or V-blank (I set: any is requested) |
//! | [`idle`] (a store-free busy loop's back edge, a poll on a flag or the beam) | the next event ([`half_step`]) |
//! | [`spin`] (a loop that calls, past its budget) | the next event |
//! | [`spend`]`(cycles)` (a BIOS copy's time, [`bios::cpu_set_cycles`]) | `cycles` have passed; what fell due runs at the new time |
//!
//! [`step`] runs the clock to the next line's start.
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
//! **Sound** is the play build's own sound path ([`Sound`],
//! [`crate::sound`]): the sound registers and timers stored and read as
//! the play build stores them, the PSG, the FIFOs and their DMA
//! ([`Hw::fifo_dma`], reading the program's memory), and the mixer, whose
//! samples go to [`Host::audio`]. A timer's counter reads as it stands at
//! the clock.
//!
//! **Saves** are the play build's chip ([`Hw::save`], a 512 Kbit flash
//! chip until the program sets its kind), driven by the chip's commands
//! or, for the flash library's stack routines, by [`flash`]'s calls.
//!
//! What isn't here yet: DMA by address (the escape hatch panics on its
//! registers), H-blank DMA's price on the clock, serial, keeping the save
//! in a `.sav`.

pub mod bios;
pub mod flash;

use crate::io::{self, Io, irq, reg};
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
    /// The sound made since the last chunk: at each V-blank before the
    /// present, and whenever a buffer is half full (the play build's
    /// `Platform::audio`). A host that plays none leaves it.
    fn audio(&mut self, _chunk: &crate::sound::AudioChunk<'_>) {}
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
    fn audio(&mut self, chunk: &crate::sound::AudioChunk<'_>) {
        self.0.audio(chunk);
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
    vblank: [Option<VblankCopy<G>>; 4],
}

/// A V-blank DMA's copy: it runs over the game and returns its price in
/// cycles ([`dma_cycles`]).
pub type VblankCopy<G> = fn(&mut G) -> u32;

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
    /// (an OAM copy from a shadow buffer). `f` copies and returns what the
    /// copy costs ([`dma_cycles`], or 0): the play build adds it to the
    /// clock after the V-blank's channels have run, which moves where its
    /// sound's frame ends.
    pub fn vblank(&mut self, n: usize, f: VblankCopy<G>) {
        self.vblank[n] = Some(f);
        self.hblank[n] = None;
    }

    /// Channel `n` off.
    pub fn stop(&mut self, n: usize) {
        self.hblank[n] = None;
        self.vblank[n] = None;
    }
}

/// A sound FIFO's DMA: channel 1 or 2 with the special start, aimed at
/// FIFO A or B. Each time the FIFO pops down to its refill level it moves
/// 4 words from `src` (as the play build's [`crate::dma::fifo_refill`]),
/// read from the program's memory by `read`.
pub struct FifoDma<G> {
    /// FIFO A (0) or B (1).
    pub fifo: usize,
    /// The next word's address, as the channel's latched source.
    pub src: u32,
    /// `DMAxCNT_H`: its source control, repeat and IRQ apply.
    pub control: u16,
    /// The word at an address, from the program's memory (ROM data, or a
    /// buffer it mixes into).
    pub read: fn(&mut G, u32) -> u32,
}

impl<G> Clone for FifoDma<G> {
    fn clone(&self) -> FifoDma<G> {
        *self
    }
}

impl<G> Copy for FifoDma<G> {}

/// The sound: the play build's own sound path ([`crate::sound`]), so the
/// two builds mix with one mixer. The sound and timer registers live in
/// an I/O block of the play build's ([`Io`]: `SOUND1CNT_L` at `0x60` to
/// the FIFOs at `0xA4`, the timers at `0x100`-`0x10F`; nothing else of it
/// is used), stored and read as the play build stores and reads them, with
/// the clock ([`Hw::now`]); the PSG, the FIFOs and the mixer are
/// [`crate::sound::Sound`], handed to [`Host::audio`] at each V-blank;
/// the FIFOs' DMA is [`FifoDma`] ([`Hw::fifo_dma`]).
pub struct Sound<G> {
    io: Io,
    /// The PSG, the FIFOs, and the samples made since the last hand-off.
    pub mixer: crate::sound::Sound,
    /// DMA channels 1 and 2, when they feed a FIFO.
    dma: [Option<FifoDma<G>>; 2],
}

impl<G> Sound<G> {
    /// Power-on: `SOUNDBIAS` 0x200, the rest zero, as the BIOS leaves it.
    pub fn new() -> Sound<G> {
        Sound {
            io: Io::new(),
            mixer: crate::sound::Sound::default(),
            dma: [None, None],
        }
    }

    /// Whether `o` is a register the sound path keeps: the sound
    /// registers, wave RAM and the FIFOs, and the timers.
    pub fn has(o: u32) -> bool {
        (SOUND_START..SOUND_END).contains(&o) || (TIMERS_START..TIMERS_END).contains(&o)
    }

    fn store(&mut self) -> Store<'_> {
        Store {
            io: &mut self.io,
            mixer: &mut self.mixer,
        }
    }
}

impl<G> Default for Sound<G> {
    fn default() -> Sound<G> {
        Sound::new()
    }
}

/// The sound registers' offsets, `SOUND1CNT_L` to past the FIFOs.
pub const SOUND_START: u32 = 0x060;
pub const SOUND_END: u32 = 0x0A8;
/// The timers' offsets.
pub const TIMERS_START: u32 = 0x100;
pub const TIMERS_END: u32 = 0x110;

/// The sound path over the registers and mixer alone: for a store, which
/// never refills a FIFO (every overflow up to the clock was walked when
/// the clock last moved, in [`sync`], which can refill).
struct Store<'a> {
    io: &'a mut Io,
    mixer: &'a mut crate::sound::Sound,
}

impl crate::sound::Audio for Store<'_> {
    fn parts(&mut self) -> (&mut Io, &mut crate::sound::Sound) {
        (self.io, self.mixer)
    }

    fn fifo_refill(&mut self, _: usize) -> bool {
        panic!("hw: a sound FIFO came due inside a store: the clock moved outside a sync")
    }
}

/// The sound path over the whole program, whose FIFO DMA reads its
/// memory: what [`sync`] runs.
struct Run<'a, G: Native>(&'a mut G);

impl<G: Native> crate::sound::Audio for Run<'_, G> {
    fn parts(&mut self) -> (&mut Io, &mut crate::sound::Sound) {
        let s = &mut self.0.hw().sound;
        (&mut s.io, &mut s.mixer)
    }

    fn fifo_refill(&mut self, fifo: usize) -> bool {
        let s = &self.0.hw().sound;
        let Some(n) = (0..2).find(|&i| s.dma[i].is_some_and(|d| d.fifo == fifo)) else {
            return false;
        };
        let Some(d) = s.dma[n] else { return false };
        let step = crate::dma::fifo_step(d.control);
        let mut src = d.src;
        let mut words = [0; 4];
        for w in &mut words {
            *w = (d.read)(self.0, src & !3);
            src = src.wrapping_add(step);
        }
        let s = &mut self.0.hw().sound;
        for w in words {
            for b in w.to_le_bytes() {
                s.mixer.fifo[fifo].push(b);
            }
        }
        if d.control & crate::dma::ctl::IRQ != 0 {
            s.io.request_irq(irq::DMA0 << (n + 1));
        }
        s.dma[n] = (d.control & crate::dma::ctl::REPEAT != 0).then_some(FifoDma { src, ..d });
        s.io.now += crate::dma::FIFO_REFILL_CYCLES;
        true
    }
}

/// What a DMA transfer of `count` units of `bytes` (2 or 4) from `src` to
/// `dst` costs under `WAITCNT` `waitcnt`: the play build's price, which
/// its V-blank DMA adds to the clock (a [`Dma::vblank`] callback returns
/// it). The addresses only say which memories it touches.
pub fn dma_cycles(waitcnt: u16, bytes: u32, src: u32, dst: u32, count: u32) -> u32 {
    let mut w = crate::waits::Waits::default();
    w.set_waitcnt(waitcnt);
    w.dma(bytes, src, dst, count) + 2
}

/// The hardware a native program drives.
pub struct Hw<G: Native> {
    /// The display's registers and memories, as fields.
    pub ppu: Video,
    pub stat: DispStat,
    pub keys: Keys,
    pub irq: Irq<G>,
    pub dma: Dma<G>,
    /// The sound and timers, and the clock.
    pub sound: Sound<G>,
    /// The cartridge's save chip, as the play build emulates it: the
    /// program sets its kind ([`crate::save::Kind::detect`] on its ROM).
    pub save: crate::save::Save,
    pub host: G::Host,
    /// Run at each V-blank's start just before the present, after V-blank
    /// DMA ([`Hw::on_frame_end`]).
    frame_end: Option<fn(&mut G)>,
    /// The line the beam is on, 0-227.
    line: u16,
    /// Frames presented since power-on.
    frames: u64,
    /// The beam is in the current line's H-blank.
    in_hblank: bool,
    /// When the beam's next event is due (H-blank's start, or the next
    /// line's), on the clock ([`Hw::now`]).
    next_line_event: u64,
    /// The earliest event due as of the last [`sync`]: the beam's, or a
    /// timer overflow something listens to (its IRQ, a cascade above it, a
    /// FIFO it clocks). Where an [`idle`] runs to.
    next_event: u64,
    /// `WAITCNT`, stored: what [`bios::cpu_set_cycles`] prices a copy by.
    pub waitcnt: u16,
    /// The play build's renderer, and the frame it is drawing.
    renderer: Ppu,
    /// Each [`spin`] loop's passes since time last passed, by its call
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
            save: crate::save::Save::new(crate::save::Kind::Flash512),
            host,
            frame_end: None,
            line: 0,
            frames: 0,
            in_hblank: false,
            next_line_event: HBLANK_DOT as u64,
            next_event: HBLANK_DOT as u64,
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

    /// Cycles since power-on: the play build's clock (`Io::now`), which
    /// moves where the program waits or spends ([`spend`]), and with its
    /// DMA (a V-blank copy's price, a FIFO refill's).
    pub fn now(&self) -> u64 {
        self.sound.io.now
    }

    /// Put the beam on `line` (0-227), in its H-blank or not, without
    /// drawing or raising anything: for a check that starts the library
    /// from a state the play build recorded (`native-fn-diff`).
    pub fn set_beam(&mut self, line: u16, in_hblank: bool) {
        debug_assert!(line < LINES);
        self.line = line;
        self.in_hblank = in_hblank;
        let start = line as u64 * LINE_DOTS as u64;
        let now = start + if in_hblank { HBLANK_DOT as u64 } else { 0 };
        self.sound.io.now = now;
        self.next_line_event = start + if in_hblank { LINE_DOTS } else { HBLANK_DOT } as u64;
        self.next_event = self.next_line_event;
    }

    /// Call `f` at each V-blank's start, after V-blank DMA and just
    /// before the frame is presented ([`Hw::frames`] already counts it, and
    /// the frame's sound has gone to [`Host::audio`]): what the play build
    /// holds when it presents, for a check build to record (the native
    /// check, `route-check --native`). Not for game code.
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

    /// DMA channel `n` (1 or 2) feeds FIFO `fifo` (0 for A, 1 for B) from
    /// `src` under `control` (`DMAxCNT_H`, enabled, special start), each
    /// word read by `read`: as the play build latches a channel switched on
    /// aimed at `FIFO_A` or `FIFO_B` (and marks the restart in the sound's
    /// chunk). The FIFO asks for its first 4 words at its first pop.
    pub fn fifo_dma(
        &mut self,
        n: usize,
        fifo: usize,
        src: u32,
        control: u16,
        read: fn(&mut G, u32) -> u32,
    ) {
        assert!((1..=2).contains(&n), "hw: only DMA1 and DMA2 feed a FIFO");
        let s = &mut self.sound;
        // As a store to the channel's registers: the overflows due first.
        crate::sound::run_timers(&mut s.store());
        s.dma[n - 1] = Some(FifoDma {
            fifo,
            src,
            control,
            read,
        });
        let now = s.io.now;
        s.mixer.restarted(fifo, now);
    }

    /// DMA channel `n` (1 or 2) no longer feeds a FIFO.
    pub fn fifo_dma_stop(&mut self, n: usize) {
        self.sound.dma[n - 1] = None;
    }

    /// The escape hatch (section 2): write the I/O register at offset `o`
    /// (even) as a store to `0x0400_0000 + o` would, for code that
    /// addresses registers through a table or a loop. The display
    /// registers, `DISPSTAT`, the sound registers and FIFOs, the timers,
    /// `IE`, `IF` (1s acknowledge) and `IME`; panics on a register the
    /// library doesn't model yet (DMA by address, serial), which a lifted
    /// program must not reach silently.
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
            _ if Sound::<G>::has(o) => self.sound_store(o, 2, v as u32),
            // Bit 15 is the cartridge type: read-only, 0 (GBA).
            reg::WAITCNT => self.waitcnt = v & 0x7FFF,
            reg::IE => self.irq.enable = v & 0x3FFF,
            reg::IF => self.irq.acknowledge(v),
            reg::IME => self.irq.master_enable = v & 1 != 0,
            _ => panic!("hw: I/O register {o:#05x} isn't modelled"),
        }
    }

    /// A word store at offset `o` (a multiple of 4): one store to a sound
    /// or timer register pair, as the play build's bus makes it (a word
    /// reaches the PSG at once, as DMA's 32-bit units do), else two
    /// [`Hw::write16`]s.
    pub fn write32(&mut self, o: u32, v: u32) {
        if Sound::<G>::has(o) && Sound::<G>::has(o + 2) {
            self.sound_store(o, 4, v);
        } else {
            self.write16(o, v as u16);
            self.write16(o + 2, (v >> 16) as u16);
        }
    }

    /// A store of `bytes` to a sound or timer register, through the play
    /// build's sound path ([`crate::sound::store`]).
    fn sound_store(&mut self, o: u32, bytes: u32, v: u32) {
        crate::sound::store(&mut self.sound.store(), o, bytes, v);
        self.take_sound_irqs();
    }

    /// Move the IRQs the timers and FIFO DMA requested (in the sound's
    /// `IF`) into [`Irq::flags`], and drop the sound's events, which only
    /// the play build's scheduler reads.
    fn take_sound_irqs(&mut self) {
        let io = &mut self.sound.io;
        let f = io.raw16(reg::IF);
        if f != 0 {
            io.set_raw16(reg::IF, 0);
            self.irq.flags |= f;
        }
        io.events = 0;
    }

    /// Read the I/O register at offset `o` (even) as a load would, without
    /// moving the beam (a game's `VCOUNT` poll calls [`vcount`]). A timer's
    /// counter is its value at the clock, as it stands.
    pub fn read16(&self, o: u32) -> u16 {
        match o {
            reg::DISPSTAT => self.dispstat(),
            reg::VCOUNT => self.line,
            reg::KEYINPUT => self.keys.keyinput(),
            _ if o < crate::video::REGS_END => self.ppu.read16(o),
            _ if Sound::<G>::has(o) => self.sound.io.read16(o),
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
            _ if Sound::<G>::has(o) => self.sound.io.raw16(o),
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

/// The sound brought up to the clock: each overflow of a timer that
/// clocks a FIFO walked (popping, refilling from the program's memory,
/// recording the sample), the other timers counted, the PSG run.
fn run_sound<G: Native>(g: &mut G) {
    crate::sound::run_timers(&mut Run(g));
    g.hw().take_sound_irqs();
}

/// Bring everything up to the clock moved on by `cycles`, as the play
/// build's `Machine::sync` does: each beam event due ([`line_event`]: a
/// line drawn, H-blank DMA, V-blank's DMA and present), with the sound
/// brought up to the clock before each and after; then the next event is
/// found ([`Hw::next_event`]), and the handlers of what's requested run.
/// Returns whether V-blank started.
fn sync<G: Native>(g: &mut G, cycles: u32) -> Result<bool, Exit> {
    g.hw().sound.io.now += cycles as u64;
    let mut vblank = false;
    while g.hw().sound.io.now >= g.hw().next_line_event {
        run_sound(g);
        vblank |= line_event(g)?;
    }
    run_sound(g);
    let hw = g.hw();
    if crate::sound::half_full(&hw.sound.mixer) {
        hand_off(hw);
    }
    let io = &hw.sound.io;
    hw.next_event = crate::timer::next_event(io, crate::sound::clocks(io))
        .map_or(hw.next_line_event, |t| t.min(hw.next_line_event));
    // Time passed: every spin loop counts again, but while a stuck one
    // moves it; a frame's end starts them all again.
    if !hw.spinning || vblank {
        hw.spins = [(0, 0); crate::sched::SPIN_SLOTS];
    }
    dispatch(g);
    Ok(vblank)
}

/// The sound since the last hand-off, to [`Host::audio`].
fn hand_off<G: Native>(hw: &mut Hw<G>) {
    let host = &mut hw.host;
    crate::sound::hand_off(&mut hw.sound.store(), |c| host.audio(c));
}

/// The beam's next event, at the clock: H-blank's start (the line drawn if
/// visible, H-blank DMA, its IRQ requested), or the next line's (its
/// V-count match; at line 160, V-blank: V-blank DMA and its price, the
/// sound handed off, the present, the keys). Returns whether V-blank
/// started.
fn line_event<G: Native>(g: &mut G) -> Result<bool, Exit> {
    let hw = g.hw();
    if !hw.in_hblank {
        hw.in_hblank = true;
        hw.next_line_event += (LINE_DOTS - HBLANK_DOT) as u64;
        if hw.stat.hblank_irq {
            hw.request(irq::HBLANK);
        }
        let y = hw.line;
        if y < VBLANK_LINE {
            // Drawn before H-blank DMA changes the registers for the next
            // line, as the play build does.
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
        return Ok(false);
    }
    hw.in_hblank = false;
    hw.next_line_event += HBLANK_DOT as u64;
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
    if !vblank {
        return Ok(false);
    }
    // The channels run at the line's cycle; what they cost goes on the
    // clock after them all, as the play build's DMA trigger adds it.
    let mut cycles = 0;
    for n in 0..4 {
        if let Some(f) = g.hw().dma.vblank[n] {
            cycles += f(g) as u64;
        }
    }
    let hw = g.hw();
    hw.sound.io.now += cycles;
    hand_off(hw);
    hw.frames += 1;
    if let Some(f) = hw.frame_end {
        f(g);
    }
    let hw = g.hw();
    let shown = hw.host.present(&hw.renderer.frame);
    hw.keys.held = hw.host.keys() & key::ALL;
    shown?;
    Ok(true)
}

/// Run the clock to the next event ([`Hw::next_event`]: the beam's half
/// line, or a timer overflow something listens to) unless it is past it
/// already, and [`sync`] there: the play build's idle edge. Returns
/// whether V-blank started.
pub fn half_step<G: Native>(g: &mut G) -> Result<bool, Exit> {
    let hw = g.hw();
    hw.sound.io.now = hw.sound.io.now.max(hw.next_event);
    sync(g, 0)
}

/// Move the beam one line: events ([`half_step`]) until the next line has
/// started. Returns whether V-blank started.
pub fn step<G: Native>(g: &mut G) -> Result<bool, Exit> {
    let line = g.hw().line;
    let mut vblank = false;
    while g.hw().line == line {
        vblank |= half_step(g)?;
    }
    Ok(vblank)
}

/// `VBlankIntrWait`: the rest of the frame drawn from the beam's line,
/// presented, the keys latched, the V-blank handler run; returns at line
/// 160. Like every wait it starts with a [`sync`] where it is (the play
/// build's halt store's), which finds the next event.
pub fn wait_vblank<G: Native>(g: &mut G) -> Result<(), Exit> {
    if sync(g, 0)? {
        return Ok(());
    }
    while !half_step(g)? {}
    Ok(())
}

/// `IntrWait` for the sources in `mask`: the clock runs event by event
/// until a handler for one of them has run. With the CPSR's I bit set no
/// handler can run, so it returns when one of them is requested (the CPU
/// wakes), its handler still to come at [`irq_release`]. That departs from
/// the hardware on purpose: the BIOS's `IntrWait` keeps the caller's I bit
/// and loops on its flags, which no handler sets, so it would spin for
/// ever.
pub fn wait_irq<G: Native>(g: &mut G, mask: u16) -> Result<(), Exit> {
    g.hw().irq.ran = 0;
    sync(g, 0)?;
    loop {
        if g.hw().irq.ran & mask != 0 || woke_masked(g, mask) {
            return Ok(());
        }
        half_step(g)?;
    }
}

/// The back edge of a busy loop with no stores and no calls (a poll on a
/// flag a handler sets, or on the beam: `while g.hw.line() != 100 {
/// hw::idle(g)?; }`): the clock runs to the next event ([`half_step`]),
/// as the play build's `--timing native` does, then the loop tests again.
/// A read of the beam itself moves nothing (`Hw::line`, `Hw::dispstat`).
pub fn idle<G: Native>(g: &mut G) -> Result<(), Exit> {
    half_step(g).map(|_| ())
}

/// The back edge of a loop that calls functions and may be waiting through
/// them for an interrupt (Emerald's `DoMapLoadLoop`,
/// `while !load_map_in_steps_local(g, state, false) { hw::spin(g)?; }`;
/// docs/research/busy-wait-loops.md): counted by call site. Past
/// `SPIN_BUDGET` passes with no time passing, each further pass runs the
/// clock to the next event, until time passes some other way or the frame
/// ends. The play build's `--timing native` does the same at the same
/// loops (`armv4t::idle::is_spin_edge`).
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
    g.hw().spinning = false;
    r.map(|_| ())
}

/// Work that takes time (a BIOS copy's, as the play build's `--timing
/// native` charges it, [`bios::cpu_set_cycles`]): the clock moves
/// `cycles` on and everything due by then is brought up to it ([`sync`]:
/// each event passed runs at the new time, then the handlers), as the
/// play build's `Machine::spend`. The only time a native program's work
/// takes (docs/native-port-design.md, section 7).
pub fn spend<G: Native>(g: &mut G, cycles: u32) -> Result<(), Exit> {
    sync(g, cycles).map(|_| ())
}

/// `Halt`: the clock runs event by event until a handler has run, or to
/// V-blank if none is enabled. With the CPSR's I bit set, until an enabled
/// source is requested (the CPU wakes; its handler waits for
/// [`irq_release`]).
pub fn halt<G: Native>(g: &mut G) -> Result<(), Exit> {
    g.hw().irq.ran = 0;
    let mut vblank = sync(g, 0)?;
    loop {
        if vblank || g.hw().irq.ran != 0 || woke_masked(g, !0) {
            return Ok(());
        }
        vblank = half_step(g)?;
    }
}

/// `io::reg` re-exported for [`Hw::write16`]'s callers.
pub use io::reg as regs;

#[cfg(test)]
mod tests;
