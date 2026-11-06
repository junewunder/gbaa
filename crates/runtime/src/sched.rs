//! Time: scanlines, timers, DMA starts, halt and IRQ delivery (decision 11).
//!
//! The clock is [`Io::now`](crate::io::Io::now). Code advances it through
//! [`Machine::tick`]: the interpreter after every instruction, generated
//! code at every block exit, each with [`armv4t::cycles::cost`]'s count and
//! the code fetches' wait states, plus the data accesses' cycles the bus
//! counted ([`crate::waits`]).
//! `tick`'s fast path is an add and three compares; when an event is due, or
//! an I/O write left something to act on, it calls [`Machine::sync`], which
//! brings everything up to the clock and says whether the run loop must take
//! control: an IRQ can be taken, the CPU is halted, or the frame ended.
//!
//! A scanline is 1232 cycles: H-draw, then H-blank from cycle 1006 (when
//! GBATEK's H-blank flag and IRQ come). 228 lines a frame; 160–227 are
//! V-blank, though the V-blank flag drops on line 227. The frame ends, for
//! [`Machine::run_frame`], at the start of V-blank.
//!
//! IRQ entry follows the real BIOS's handler at `0x18`, run as Rust (we ship
//! no BIOS image, decision 12): it enters IRQ mode, pushes r0–r3, r12 and lr,
//! and calls the handler pointer at `0x0300_7FFC` with lr pointing at
//! [`IRQ_RETURN`]. When the handler returns there, [`Machine::irq_return`]
//! does the BIOS's `ldmfd` and `subs pc, lr, #4`.

use crate::bios;
use crate::dma;
use crate::io::{event, irq, reg};
use crate::machine::Machine;
use crate::platform::Platform;
use crate::sound::psg::PSG_CHUNK;
use crate::sound::{self, CHUNK};
use crate::timer;
use armv4t::interp::{I_BIT, mode};

/// Cycles in a scanline.
pub const LINE_CYCLES: u64 = 1232;
/// Where in the line H-blank starts.
pub const HBLANK_START: u64 = 1006;
/// Scanlines in a frame, and the visible ones.
pub const LINES: u16 = 228;
pub const VISIBLE_LINES: u16 = 160;
/// A spin loop's passes with no time passing before each further pass
/// takes half a line under native timing ([`Machine::spin_edge`];
/// docs/research/busy-wait-loops.md: about 4× the longest wait seen,
/// Emerald Rogue's 186 a frame, rounded up).
pub const SPIN_BUDGET: u32 = 2048;

/// How many spin loops [`Sched::spins`] counts at once: direct-mapped by
/// address, a clash restarting the count.
pub const SPIN_SLOTS: usize = 32;

/// Cycles in a frame.
pub const FRAME_CYCLES: u64 = LINE_CYCLES * LINES as u64;

/// The BIOS's IRQ vector, and the address in the BIOS's IRQ handler that
/// user handlers return to.
pub const IRQ_VECTOR: u32 = 0x18;
pub const IRQ_RETURN: u32 = 0x138;
/// The same point in mGBA's HLE BIOS, which a handler running in a loaded
/// mGBA save state ([`crate::state`]) may return to.
pub const MGBA_IRQ_RETURN: u32 = 0x194;
/// Where the BIOS reads the user IRQ handler's address from (a mirror of
/// `0x0300_7FFC`; the BIOS uses `[0x0400_0000 - 4]`).
pub const IRQ_HANDLER_PTR: u32 = 0x0300_7FFC;

/// The scheduler's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sched {
    /// When the next scanline event (H-blank start or the next line) is due.
    pub next_line_event: u64,
    /// Whether the line is in H-blank.
    pub hblank: bool,
    /// The earliest due event: `tick` calls [`Machine::sync`] at or after it.
    pub next_event: u64,
    /// The CPU is halted (`HALTCNT`), until `IE & IF` is nonzero.
    pub halted: bool,
    /// V-blank started since [`Machine::run_frame`] last cleared it.
    pub frame_ended: bool,
    /// `IME` and `IE & IF` were both set at the last sync. `tick` syncs
    /// while it is, so an IRQ that the CPSR held back is taken as soon as
    /// the I bit clears.
    pub irq_line: bool,
    /// When a link hub's turn ends ([`crate::link`]): reaching it ends the
    /// run as the frame end does ([`Sched::frame_ended`] is set). `u64::MAX`
    /// off a hub. Not guest state: the hub sets it for each turn.
    pub deadline: u64,
    /// Under [`crate::Timing::Native`], IRQs are taken only here: set by
    /// each sync (where time passed), cleared by the next tick. Not saved:
    /// a state is saved at a frame's end, where it is set, and loading one
    /// sets it.
    pub window: bool,
    /// Under native timing, a BIOS call's work synced inside its `swi`
    /// ([`Machine::spend`] there): the tick after the `swi` keeps
    /// [`Sched::window`] open and reports a frame end or an IRQ that sync
    /// found, as one at an idle edge does. Not saved, as `window`.
    pub fresh: bool,
    /// Under native timing, each spin loop's passes since time last
    /// passed (by its branch's address, [`SPIN_SLOTS`] direct-mapped):
    /// cleared at every sync but a stuck loop's own, and at every frame's
    /// end. Not saved: a state is saved at a frame's end, where it is
    /// empty.
    pub spins: [(u32, u32); SPIN_SLOTS],
}

impl Default for Sched {
    fn default() -> Sched {
        Sched {
            next_line_event: HBLANK_START,
            hblank: false,
            next_event: HBLANK_START,
            halted: false,
            frame_ended: false,
            irq_line: false,
            deadline: u64::MAX,
            window: true,
            fresh: false,
            spins: [(0, 0); SPIN_SLOTS],
        }
    }
}

impl<P: Platform> Machine<P> {
    /// Advance the clock by `cycles`. Returns true when the run loop must
    /// take control: an IRQ can be taken, the CPU halted, or the frame
    /// ended. Generated code calls this at every block exit.
    #[inline(always)]
    pub fn tick(&mut self, cycles: u32) -> bool {
        if self.timing == crate::Timing::Native {
            return self.tick_native();
        }
        // The data accesses' cycles since the last tick ([`crate::waits`]);
        // a prefetch stall can make those negative, never the instructions'
        // whole cost.
        #[cfg(feature = "trace")]
        let given = cycles;
        let cycles = (cycles as i32 + self.bus.take_data_cycles()).max(0) as u32;
        #[cfg(feature = "trace")]
        crate::trace::TICKS.add((given as u64) << 32 | cycles as u64);
        let io = &mut self.bus.io;
        let now = io.now + cycles as u64;
        if now < self.sched.next_event && io.events == 0 && !self.sched.irq_line {
            io.now = now;
            return false;
        }
        self.sync(cycles)
    }

    /// [`Machine::tick`] under [`crate::Timing::Native`], where code takes
    /// no time: the cycles are dropped, and only a halt (a `HALTCNT`
    /// store), a serial start, or an IRQ that an `msr` or an `IME` write
    /// let through is acted on. A clock a DMA moved is
    /// brought up to date at the next point where time passes (an idle
    /// back edge, a BIOS call or wait), the same instruction in generated
    /// code and the interpreter, so IRQs are taken at the same places in
    /// both.
    #[inline(never)]
    pub fn tick_native(&mut self) -> bool {
        self.bus.take_data_cycles();
        let ime_on = self.bus.io.events & event::IME_ON != 0;
        self.bus.io.events &= !event::IME_ON;
        if core::mem::take(&mut self.sched.fresh) {
            return self.sched.halted || self.sched.frame_ended || self.irq_takeable();
        }
        // An IRQ the CPSR's I bit held at the last sync (a wait inside a
        // critical section) is taken as soon as an `msr` clears I, as the
        // hardware takes it, not at the next time point (native/lift-irq-
        // hold). One the sync found with I clear was taken in its window,
        // so this is only ever a held one. Likewise one pending when `IME`
        // is set again (a critical section that clears `IME`, as agbcc
        // games' do): taken at this tick (native/ime-release).
        if (self.sched.irq_line || ime_on)
            && self.cpu.cpsr & I_BIT == 0
            && self.bus.io.irq_pending()
        {
            self.sched.window = true;
            return true;
        }
        self.sched.window = false;
        if self.bus.io.events & (event::HALT | event::STOP | event::SERIAL) != 0 {
            return self.sync(0);
        }
        false
    }

    /// An idle loop's back edge was taken under [`crate::Timing::Native`]
    /// ([`armv4t::idle`]): run the clock to the next event (a half line, a
    /// timer's), unless a DMA already moved it past one, and bring
    /// everything up to it; then the loop tests again. Returns what
    /// [`Machine::tick`] does.
    pub fn idle_edge(&mut self) -> bool {
        self.bus.take_data_cycles();
        let io = &mut self.bus.io;
        io.now = io.now.max(self.sched.next_event);
        self.sync(0)
    }

    /// An idle loop's branch wasn't taken under [`crate::Timing::Native`]:
    /// the loop ends, so if a DMA in it moved the clock past an event,
    /// bring everything up to it here; else a tick.
    pub fn idle_exit(&mut self) -> bool {
        if self.bus.io.now >= self.sched.next_event {
            self.bus.take_data_cycles();
            return self.sync(0);
        }
        self.tick_native()
    }

    /// A spin loop's back edge was taken under [`crate::Timing::Native`]
    /// ([`armv4t::idle::is_spin_edge`], or the game's list): count the
    /// pass. Past [`SPIN_BUDGET`] passes with no time passing, the loop is
    /// stuck (it is waiting for an interrupt through the functions it
    /// calls): each further pass runs the clock to the next event, as an
    /// idle edge, until time passes some other way or the frame ends.
    /// Returns what [`Machine::tick`] does.
    pub fn spin_edge(&mut self, at: u32) -> bool {
        let slot = &mut self.sched.spins[(at as usize >> 1) % SPIN_SLOTS];
        if slot.0 != at {
            *slot = (at, 0);
        }
        slot.1 += 1;
        if slot.1 <= SPIN_BUDGET {
            return self.tick_native();
        }
        self.bus.take_data_cycles();
        let io = &mut self.bus.io;
        io.now = io.now.max(self.sched.next_event);
        let spins = self.sched.spins;
        let y = self.sync(0);
        // The stuck loop's own time doesn't unstick it; a frame's end does.
        if !self.sched.frame_ended {
            self.sched.spins = spins;
        }
        y
    }

    /// Time a BIOS routine's work takes (a copy's, a fill's, a
    /// decompression's stall): ticked, or under [`crate::Timing::Native`],
    /// where it is the only time code takes, spent and synced to, so a
    /// V-blank it crosses comes as in a wait.
    pub fn spend(&mut self, cycles: u32) -> bool {
        if self.timing == crate::Timing::Native {
            self.bus.take_data_cycles();
            return self.sync(cycles);
        }
        self.tick(cycles)
    }

    /// Cycles since power-on.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.bus.io.now
    }

    /// [`Machine::tick`]'s slow path: act on pending I/O writes (as of the
    /// clock before `cycles`, when the code that wrote them ran), advance
    /// the clock, and bring scanlines, timers and DMA up to it.
    #[inline(never)]
    pub fn sync(&mut self, cycles: u32) -> bool {
        self.nest.sample_stack();
        loop {
            let events = self.bus.io.take_events();
            if events == 0 {
                break;
            }
            self.handle_events(events);
        }
        self.bus.io.now += cycles as u64;
        loop {
            let events = self.bus.io.take_events();
            if events != 0 {
                self.handle_events(events);
                continue;
            }
            if self.bus.io.now >= self.sched.next_line_event {
                sound::run_timers(&mut self.bus);
                self.line_event();
                continue;
            }
            break;
        }
        sound::run_timers(&mut self.bus);
        if self.bus.sound.len >= CHUNK / 2 || self.bus.sound.psg.len >= PSG_CHUNK / 2 {
            self.hand_off_audio();
        }
        let io = &self.bus.io;
        self.sched.next_event = timer::next_event(io, sound::clocks(io))
            .map_or(self.sched.next_line_event, |t| {
                t.min(self.sched.next_line_event)
            })
            .min(self.sched.deadline);
        if io.now >= self.sched.deadline {
            self.sched.frame_ended = true;
        }
        if self.sched.halted && io.irq_requested() {
            self.sched.halted = false;
        }
        self.sched.irq_line = io.irq_pending();
        self.sched.window = true;
        // Time passed: every spin loop starts its count again.
        self.sched.spins = [(0, 0); SPIN_SLOTS];
        self.sched.halted || self.sched.frame_ended || self.irq_takeable()
    }

    /// Act on the writes [`crate::io::event`] recorded. DMA and timer
    /// writes already took effect in the store ([`crate::Bus`]); a timer
    /// write's event is only here so its overflow gets rescheduled, and
    /// `IRQ` so the IRQ line is recomputed, both of which `sync` does.
    fn handle_events(&mut self, events: u32) {
        if events & (event::HALT | event::STOP) != 0 {
            // STOP is treated as halt; nothing here powers down.
            self.halt();
        }
        if events & event::SERIAL != 0 && crate::link::start_requested(&self.bus.io) {
            // The hub starts the transfer between turns: end this one.
            self.sched.deadline = 0;
        }
    }

    /// End the run (as the frame end does) once the clock reaches
    /// `deadline`: a link hub's turn ([`crate::link`]). `u64::MAX` for none.
    pub fn set_deadline(&mut self, deadline: u64) {
        self.sched.deadline = deadline;
        self.sched.next_event = self.sched.next_event.min(deadline);
    }

    /// The next scanline event: H-blank start, or the next line.
    fn line_event(&mut self) {
        let s = &mut self.sched;
        let line = self.bus.io.vcount();
        let stat = self.bus.io.raw16(reg::DISPSTAT);
        if !s.hblank {
            s.hblank = true;
            s.next_line_event += LINE_CYCLES - HBLANK_START;
            let vblank = stat & 1 != 0;
            self.bus.io.set_blanking(vblank, true);
            if stat & (1 << 4) != 0 {
                self.bus.io.request_irq(irq::HBLANK);
            }
            if line < VISIBLE_LINES {
                // Draw the line before H-blank DMA changes the registers
                // for the next one (mGBA's order), so mid-frame effects show.
                self.ppu
                    .render_line(self.bus.video_for_draw(), line as usize);
                let c = dma::trigger(&mut self.bus, dma::Timing::HBlank);
                self.bus.io.now += c as u64;
            }
            return;
        }
        s.hblank = false;
        s.next_line_event += HBLANK_START;
        let line = (line + 1) % LINES;
        if self.bus.io.set_vcount(line) && stat & (1 << 5) != 0 {
            self.bus.io.request_irq(irq::VCOUNT);
        }
        self.bus
            .io
            .set_blanking((VISIBLE_LINES..LINES - 1).contains(&line), false);
        if line == VISIBLE_LINES {
            self.vblank_start(stat);
        }
    }

    /// V-blank starts: the IRQ, V-blank DMA, then the frame (drawn line by
    /// line at each H-blank) is presented and the keys read
    /// ([`Machine::present_frame`]).
    fn vblank_start(&mut self, stat: u16) {
        if stat & (1 << 3) != 0 {
            self.bus.io.request_irq(irq::VBLANK);
        }
        let c = dma::trigger(&mut self.bus, dma::Timing::VBlank);
        self.bus.io.now += c as u64;
        self.sched.frame_ended = true;
        if self.nest.frame_end.is_some() {
            // The front end shows it after the switch, off the guest's
            // stack; no guest instruction runs in between.
            self.nest.vblank = true;
            self.hand_off_audio();
        } else {
            self.present_frame();
        }
    }

    /// Halt the CPU until an enabled interrupt is requested (`IE & IF`,
    /// whatever `IME` says). For `HALTCNT` writes and the BIOS's waits.
    pub fn halt(&mut self) {
        self.sched.halted = true;
        // Wake now if one already is, as the hardware does.
        if self.bus.io.irq_requested() {
            self.sched.halted = false;
        }
    }

    /// While halted, run time forward event by event until the CPU wakes
    /// or the frame ends.
    pub fn idle(&mut self) {
        while self.sched.halted && !self.sched.frame_ended {
            let io = &mut self.bus.io;
            io.now = io.now.max(self.sched.next_event);
            self.sync(0);
        }
    }

    /// Whether an IRQ would be taken now: `IME`, `IE & IF`, and the CPSR's
    /// I bit clear.
    #[inline]
    pub fn irq_takeable(&self) -> bool {
        self.cpu.cpsr & I_BIT == 0
            && self.bus.io.irq_pending()
            && (self.sched.window || self.timing != crate::Timing::Native)
    }

    /// Take an IRQ if one is takeable, the way the BIOS does: IRQ mode with
    /// lr = the interrupted instruction + 4, push r0–r3, r12, lr, then call
    /// the handler at [`IRQ_HANDLER_PTR`] in ARM state with r0 =
    /// `0x0400_0000` and lr = [`IRQ_RETURN`]. Call it between instructions
    /// (or blocks), with `cpu.pc` the next one to run. Returns whether it
    /// took one.
    pub fn take_irq(&mut self) -> bool {
        if !self.irq_takeable() {
            return false;
        }
        let ret = self.cpu.pc.wrapping_add(4);
        self.cpu.enter_exception(mode::IRQ, IRQ_VECTOR, ret);
        let sp = self.cpu.r[13].wrapping_sub(24);
        for (i, r) in [0, 1, 2, 3, 12, 14].into_iter().enumerate() {
            self.bus.write32(sp + 4 * i as u32, self.cpu.r[r]);
        }
        self.cpu.r[13] = sp;
        self.cpu.r[0] = 0x0400_0000;
        self.cpu.r[14] = IRQ_RETURN;
        self.cpu.pc = self.bus.read32(IRQ_HANDLER_PTR) & !3;
        if self.timing != crate::Timing::Native {
            self.bus.io.now += 3;
        }
        bios::irq_enter(self);
        true
    }

    /// The end of the BIOS's IRQ handler, run when a user handler returns
    /// to [`IRQ_RETURN`]: pop r0–r3, r12, lr, then `subs pc, lr, #4`, then
    /// [`bios::irq_return`].
    pub fn irq_return(&mut self) {
        let sp = self.cpu.r[13];
        for (i, r) in [0, 1, 2, 3, 12, 14].into_iter().enumerate() {
            self.cpu.r[r] = self.bus.read32(sp + 4 * i as u32);
        }
        self.cpu.r[13] = sp.wrapping_add(24);
        let target = self.cpu.r[14].wrapping_sub(4);
        let spsr = self.cpu.spsr();
        self.cpu.write_cpsr(spsr, !0);
        self.cpu.pc = target;
        // Sets the after-IRQ BIOS read, and finishes a BIOS wait this IRQ
        // interrupted.
        bios::irq_return(self);
    }
}

crate::snap::snap!(Sched {
    next_line_event,
    hblank,
    next_event,
    halted,
    frame_ended,
    irq_line
}; skip deadline, window, fresh, spins);

#[cfg(test)]
mod tests;

#[cfg(test)]
mod native_tests;
