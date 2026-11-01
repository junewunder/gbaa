//! Breakpoints and watchpoints, in the interpreter only: compiled code has
//! no instruction boundary to stop at, and its stores go straight to the
//! [`Bus`] (docs/research/lua-scripting.md, "Can't, or costs"). A front end
//! that has a point set runs frames with [`Machine::run_frame_debug`]
//! instead, as mGBA's debugger does: a [`Debugger`] sees each instruction
//! before it runs and, while it is [`Debugger::watching`], every load and
//! store the CPU makes ([`Access`]). DMA and the BIOS calls (`swi`, run
//! as Rust) aren't seen.
//!
//! [`Machine::step`] is [`Machine::step_watched`] with [`NoWatch`], whose
//! checks compile away.

use crate::bus::Bus;
use crate::machine::Machine;
use crate::platform::Platform;
use armv4t::sem::Width;

/// One CPU load or store, told before it happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub write: bool,
    /// As the instruction gave it (unaligned too).
    pub addr: u32,
    /// In bytes: 1, 2 or 4.
    pub width: u32,
    /// What memory holds there now (for a load, what it loads).
    pub old: u32,
    /// What a store writes, its low `width` bytes; 0 for a load.
    pub new: u32,
}

/// What [`Machine::step_watched`] tells of an instruction's accesses.
pub trait Watch {
    /// Whether to tell at all: [`NoWatch`]'s `false` compiles the checks
    /// out of [`Machine::step`].
    const ON: bool = true;
    fn access(&mut self, a: Access);
}

/// Tells nothing.
pub struct NoWatch;

impl Watch for NoWatch {
    const ON: bool = false;
    #[inline(always)]
    fn access(&mut self, _: Access) {}
}

/// The bus as the CPU sees it, telling `w` of each access first.
pub struct Watched<'a, W: Watch> {
    pub bus: &'a mut Bus,
    pub w: &'a mut W,
}

fn bytes(w: Width) -> u32 {
    match w {
        Width::Byte => 1,
        Width::Half => 2,
        Width::Word => 4,
    }
}

impl<W: Watch> Watched<'_, W> {
    fn peek(&self, w: Width, addr: u32) -> u32 {
        match w {
            Width::Byte => self.bus.read8(addr) as u32,
            Width::Half => self.bus.read16(addr) as u32,
            Width::Word => self.bus.read32(addr),
        }
    }
}

impl<W: Watch> armv4t::interp::Bus for Watched<'_, W> {
    fn load(&mut self, w: Width, addr: u32) -> u32 {
        let old = self.peek(w, addr);
        self.w.access(Access {
            write: false,
            addr,
            width: bytes(w),
            old,
            new: 0,
        });
        self.bus.load(w, addr)
    }

    fn store(&mut self, w: Width, addr: u32, v: u32) {
        let width = bytes(w);
        let old = self.peek(w, addr);
        let new = if width == 4 {
            v
        } else {
            v & ((1 << (8 * width)) - 1)
        };
        self.w.access(Access {
            write: true,
            addr,
            width,
            old,
            new,
        });
        self.bus.store(w, addr, v)
    }
}

/// What a front end with breakpoints or watchpoints set hands
/// [`Machine::run_frame_debug`].
pub trait Debugger<P: Platform>: Watch {
    /// Before the instruction at `m.cpu.pc` runs: fire the breakpoints
    /// there. The machine is the debugger's to read and change.
    fn before(&mut self, m: &mut Machine<P>);
    /// Whether to watch the next instruction's accesses ([`Watch::access`]).
    fn watching(&self) -> bool;
    /// After an instruction: fire what its accesses hit.
    fn after(&mut self, m: &mut Machine<P>);
}

impl<P: Platform> Machine<P> {
    /// [`Machine::run_frame`], with `d` told of each instruction.
    pub fn run_frame_debug(&mut self, d: &mut impl Debugger<P>) {
        self.sched.frame_ended = false;
        while !self.sched.frame_ended && self.fault.is_none() {
            if self.sched.halted {
                self.idle();
                continue;
            }
            self.take_irq();
            // A call a mod added, whose frame the debugger's interpreter
            // doesn't have: on where its block began (`dispatch::modcall`).
            let k = crate::dispatch::key(&self.cpu);
            if crate::dispatch::modcall::is_call_key(k) {
                if crate::dispatch::modcall::resume(self, k).is_none() {
                    self.fault = Some(crate::machine::Fault::ModCall(k));
                }
                continue;
            }
            d.before(self);
            if d.watching() {
                self.step_watched(d);
            } else {
                self.step();
            }
            d.after(self);
        }
    }
}
