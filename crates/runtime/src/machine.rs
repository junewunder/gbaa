//! The whole machine: CPU state, the bus, and the front end. Generated code
//! and the interpreter both run against a `&mut Machine`.
//!
//! This file has the interpreter's runner: [`Machine::step`] runs one
//! instruction and ticks, [`Machine::run_frame`] runs to the next V-blank.
//! Time, IRQs and halt are in [`crate::sched`].

use crate::bios::{self, Bios, Unimplemented};
use crate::bus::{BIOS_SIZE, Bus};
use crate::platform::Platform;
use crate::ppu::Ppu;
use crate::sched::{IRQ_RETURN, MGBA_IRQ_RETURN, Sched};
use crate::waits;
use armv4t::cycles;
use armv4t::decode::{Instr, decode_arm, decode_thumb};
use armv4t::interp::{Bank, Cpu, mode};
use armv4t::sem::Flag;

/// The stack pointers the BIOS leaves for each mode.
pub const SP_SYS: u32 = 0x0300_7F00;
pub const SP_IRQ: u32 = 0x0300_7FA0;
pub const SP_SVC: u32 = 0x0300_7FE0;
/// Where the cartridge's entry point is.
pub const ROM_ENTRY: u32 = 0x0800_0000;

/// A GBA: CPU state, memory and I/O, the PPU, and the front end it draws
/// to.
pub struct Machine<P: Platform> {
    pub cpu: Cpu,
    pub bus: Bus,
    /// BIOS state that outlives a call ([`crate::bios`]).
    pub bios: Bios,
    pub ppu: Ppu,
    pub platform: P,
    pub sched: Sched,
    /// Set when the runner hit something nothing emulates; it stops there.
    pub fault: Option<Fault>,
    /// The interpreter's prefetch queue.
    pub prefetch: Prefetch,
    /// The dispatcher's state while it runs, for a run nested inside
    /// compiled code ([`crate::dispatch::settle`]). Not guest state.
    pub nest: crate::dispatch::Nest,
    /// How the interpreter counts its cycles. [`Machine::new`] starts it
    /// exact; a front end sets it (`gbaa-desktop --timing`, or a
    /// recompiled build's own, [`crate::dispatch::Code::timing`]). Not
    /// guest state: a state loads into either.
    pub timing: Timing,
    /// The game's own spin loops (their branches' addresses, sorted),
    /// beyond what [`armv4t::idle::is_spin_edge`] finds: a per-game list
    /// (`runtime::spins`) a recompiled build carries
    /// ([`crate::dispatch::Code::spins`]). Native timing only. Not guest
    /// state.
    pub spin_list: &'static [u32],
    /// The lowest `sp` the interpreter has seen after an instruction since
    /// power-on ([`Machine::note_sp`]). Compiled code doesn't keep it:
    /// its pushes and pops may never write the lowest value to r13 (a
    /// block's balanced push and pop leave r13 as it was), so a run
    /// wanting it is interpreted (`gbaa-desktop --interp`). The native
    /// check leaves the guest stack, from here to IWRAM's end, out of its
    /// RAM compare (docs/native-port-design.md, section 7). Not guest
    /// state.
    pub low_sp: u32,
}

/// How cycles are counted (`--timing`, `docs/decisions.md` 11), the
/// runtime's copy of `gbaa-recompile`'s modes: what a recompiled build was
/// generated with ([`crate::dispatch::Code::timing`]) and what the
/// interpreter follows ([`Machine::step`]). Only the cycle count differs,
/// never what an instruction does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Timing {
    /// mGBA's model: fetches priced by `WAITCNT`, each data access by the
    /// memory it touches, the prefetch buffer's overlap. What every check
    /// runs, against mGBA.
    #[default]
    Exact = 0,
    /// Fetches priced by `WAITCNT`, data accesses free. Compiled code
    /// prices fetches at the `WAITCNT` games set instead; the interpreter
    /// uses the game's own.
    Static = 1,
    /// As fast as the CPU goes: every fetch as if the prefetch buffer had
    /// it (no wait states), data accesses free. What play runs. A failed
    /// condition costs the whole instruction.
    Instant = 2,
    /// As a native program keeps time (docs/native-port-design.md,
    /// sections 3 and 7): code takes none. The clock moves only at the
    /// BIOS's waits and its copies' and decompressions' stalls, at an idle
    /// loop's back edge ([`armv4t::idle`], [`Machine::idle_edge`]), at a
    /// stuck spin loop's ([`Machine::spin_edge`]) and by DMA's cost; a
    /// read of `VCOUNT`, `DISPSTAT` or a timer moves none (a poll on one
    /// is an idle loop). IRQs are taken only where it moved. What the
    /// native port's check runs.
    Native = 3,
}

impl Timing {
    /// `--timing`'s word for it.
    pub fn name(self) -> &'static str {
        match self {
            Timing::Exact => "exact",
            Timing::Static => "static",
            Timing::Instant => "instant",
            Timing::Native => "native",
        }
    }

    /// [`Timing::name`]'s inverse.
    pub fn parse(s: &str) -> Option<Timing> {
        match s {
            "exact" => Some(Timing::Exact),
            "static" => Some(Timing::Static),
            "instant" => Some(Timing::Instant),
            "native" => Some(Timing::Native),
            _ => None,
        }
    }
}

/// Why the runner stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The CPU reached a BIOS address nothing emulates.
    Bios(u32),
    /// A `swi` the BIOS HLE doesn't implement.
    Swi(Unimplemented),
    /// Code returned to a call a mod added (this reserved key,
    /// `dispatch::modcall`) whose record isn't at sp.
    ModCall(u32),
}

impl core::fmt::Display for Fault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Fault::Bios(at) => write!(f, "reached BIOS address {at:#010x}, which isn't emulated"),
            Fault::Swi(e) => write!(f, "{e}"),
            Fault::ModCall(k) => write!(
                f,
                "returned to a call a mod added ({k:#010x}) with no record of it at sp"
            ),
        }
    }
}

/// The two opcodes the ARM7TDMI has fetched past the one it executes. A
/// store over them doesn't change what runs next (jsmolka's `nes` test 1);
/// the second is also what open bus reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefetch {
    /// The address of `raw[0]`, in the state `thumb`.
    pub addr: u32,
    pub thumb: bool,
    pub raw: [u32; 2],
    /// The clock when it was filled. Only the very next step may use it:
    /// anything else that ran since (compiled code, an IRQ entry, a halt)
    /// moved the clock and empties the queue.
    pub at: u64,
}

/// Whether `addr` is in the cartridge's ROM, where idle loops are looked
/// for (code copied to RAM is the overlays', which the code generator
/// plans apart).
pub fn is_rom(addr: u32) -> bool {
    (0x0800_0000..0x0E00_0000).contains(&addr)
}

/// The CPU as the BIOS hands over: see [`Machine::new`].
pub(crate) fn boot_cpu() -> Cpu {
    let mut cpu = Cpu {
        pc: ROM_ENTRY,
        cpsr: mode::SYS,
        ..Cpu::default()
    };
    cpu.r[13] = SP_SYS;
    cpu.r[14] = ROM_ENTRY;
    cpu.set_flag(Flag::C, true);
    // r13 is the sixth of each bank's r8-r14.
    cpu.banks[Bank::Irq as usize][5] = SP_IRQ;
    cpu.banks[Bank::Svc as usize][5] = SP_SVC;
    cpu
}

impl<P: Platform> Machine<P> {
    /// A machine in the state the BIOS hands over in: system mode, ARM
    /// state, the BIOS's stacks, about to run the cartridge's entry point.
    /// The rest is as mGBA's HLE BIOS leaves it, so register dumps from
    /// reset compare equal: its reset code (`resetBase` in hle-bios.s)
    /// jumps with `bx lr` and lr = the entry point, after a `cmp r1, #0`
    /// that sets the carry flag, and leaves every other register zero.
    pub fn new(rom: &'static [u8], platform: P) -> Machine<P> {
        Machine {
            cpu: boot_cpu(),
            bus: Bus::new(rom),
            bios: Bios::default(),
            ppu: Ppu::new(),
            platform,
            sched: Sched::default(),
            fault: None,
            prefetch: Prefetch {
                at: u64::MAX,
                ..Prefetch::default()
            },
            nest: crate::dispatch::Nest::default(),
            timing: Timing::Exact,
            spin_list: &[],
            low_sp: u32::MAX,
        }
    }

    /// `sp` was set to `sp`: keep the lowest ([`Machine::low_sp`]).
    #[inline(always)]
    pub fn note_sp(&mut self, sp: u32) {
        self.low_sp = self.low_sp.min(sp);
    }

    /// Set how cycles are counted ([`Machine::timing`]).
    pub fn set_timing(&mut self, timing: Timing) {
        self.timing = timing;
    }

    /// Run until the next V-blank starts (the frame is presented there), or
    /// until [`Machine::fault`] is set.
    pub fn run_frame(&mut self) {
        self.sched.frame_ended = false;
        while !self.sched.frame_ended && self.fault.is_none() {
            if self.sched.halted {
                self.idle();
                continue;
            }
            self.take_irq();
            self.step();
        }
    }

    /// Run one instruction in the interpreter and tick its cycles. Returns
    /// what [`Machine::tick`] returns: whether the run loop must take
    /// control (to take an IRQ, idle in halt, or end the frame).
    ///
    /// `swi` goes to [`Machine::swi`] rather than the BIOS's vector, and
    /// code in the BIOS region runs as Rust: the IRQ handler's return path
    /// ([`IRQ_RETURN`], or mGBA's [`MGBA_IRQ_RETURN`]), a BIOS call's stall
    /// ([`bios::STALL_LOOP`]), the sound driver's stub ([`bios::NOP_CALL`])
    /// and the `swi` vector. Any other BIOS address sets
    /// [`Machine::fault`].
    pub fn step(&mut self) -> bool {
        self.step_watched(&mut crate::debug::NoWatch)
    }

    /// [`Machine::step`], telling `w` of the instruction's loads and
    /// stores ([`crate::debug`]); `swi`'s BIOS calls aren't told.
    pub fn step_watched<W: crate::debug::Watch>(&mut self, w: &mut W) -> bool {
        let r = self.step_inner(w);
        self.note_sp(self.cpu.r[13]);
        r
    }

    fn step_inner<W: crate::debug::Watch>(&mut self, w: &mut W) -> bool {
        let pc = self.cpu.pc;
        if (pc as usize) < BIOS_SIZE {
            return self.bios_step(pc);
        }
        if self.timing == Timing::Native {
            return self.step_native(w);
        }
        let thumb = self.cpu.thumb();
        let (pc, size) = if thumb { (pc & !1, 2) } else { (pc & !3, 4) };
        let fetch = |bus: &Bus, at: u32| {
            if thumb {
                bus.read16(at) as u32
            } else {
                bus.read32(at)
            }
        };
        let q = self.prefetch;
        let queued = q.at == self.now() && q.thumb == thumb;
        let (raw, next) = if queued && q.addr == pc {
            (q.raw[0], q.raw[1])
        } else {
            (
                fetch(&self.bus, pc),
                fetch(&self.bus, pc.wrapping_add(size)),
            )
        };
        let after = fetch(&self.bus, pc.wrapping_add(2 * size));
        // Open bus is the last prefetched opcode; a Thumb one shows on both
        // halves.
        self.bus.open_bus = if thumb { after * 0x1_0001 } else { after };
        let (instr, pass) = self.decode(pc, thumb, raw);
        // Without mGBA's model a failed condition costs the whole
        // instruction, as generated code charges it (no `rt::skipped`).
        let cost = if pass || self.timing != Timing::Exact {
            cycles::cost(&instr)
        } else {
            cycles::SKIPPED
        };
        let r = waits::region(pc);
        let cycles = match self.timing {
            Timing::Instant => cost.base,
            _ => cost.base + self.bus.waits.fetch(r, thumb, cost.seq, cost.nonseq),
        };
        self.execute(pc, thumb, instr, pass, w);
        if self.timing != Timing::Exact {
            // Data accesses are free, as `dispatch::tick_flat` counts them.
            self.bus.take_data_cycles();
        }
        let yield_ = self.tick(cycles);
        self.prefetch = Prefetch {
            addr: pc.wrapping_add(size),
            thumb,
            raw: [next, after],
            at: self.now(),
        };
        yield_
    }

    /// [`Machine::step`] under [`Timing::Native`]: the instruction as
    /// generated code runs it (neither the prefetch queue nor open bus
    /// changes, no data access is priced), then a tick that moves no time,
    /// or, at an idle loop's branch ([`armv4t::idle`]), the clock run to
    /// the next event ([`Machine::idle_edge`]) or, the loop left, synced
    /// ([`Machine::idle_exit`]). Generated code does the
    /// same at the same instructions, so the two take IRQs at the same
    /// places.
    fn step_native<W: crate::debug::Watch>(&mut self, w: &mut W) -> bool {
        let thumb = self.cpu.thumb();
        let pc = self.cpu.pc & if thumb { !1 } else { !3 };
        let fetch = |bus: &Bus, at: u32| {
            if thumb {
                bus.read16(at) as u32
            } else {
                bus.read32(at)
            }
        };
        let (instr, pass) = self.decode(pc, thumb, fetch(&self.bus, pc));
        self.execute(pc, thumb, instr, pass, w);
        if let Instr::Branch {
            link: false,
            target,
            ..
        } = instr
            && target <= pc
            && is_rom(pc)
            && armv4t::idle::is_idle_edge(|a| fetch(&self.bus, a), pc, target, thumb)
        {
            return if pass && self.cpu.pc == target {
                self.idle_edge()
            } else {
                self.idle_exit()
            };
        }
        if pass
            && let Instr::Branch {
                link: false,
                target,
                ..
            } = instr
            && target <= pc
            && self.cpu.pc == target
            && is_rom(pc)
            && (self.spin_list.binary_search(&pc).is_ok()
                || armv4t::idle::is_spin_edge(|a| fetch(&self.bus, a), pc, target, thumb))
        {
            return self.spin_edge(pc);
        }
        self.tick(0)
    }

    /// Run the instruction at the CPU's `pc` without ticking, as generated
    /// code runs one inside a block: the fetch is from memory, neither the
    /// prefetch queue nor open bus changes, and a failed condition's
    /// saving comes off the bus's count as `dispatch::skipped` takes it.
    /// Returns its full [`cycles::cost`], which
    /// [`crate::dispatch::run_blocks`] sums and prices at the block's exit,
    /// with the wait states of then, as generated code does.
    pub fn step_untimed(&mut self) -> cycles::Cost {
        let thumb = self.cpu.thumb();
        let pc = self.cpu.pc & if thumb { !1 } else { !3 };
        let raw = if thumb {
            self.bus.read16(pc) as u32
        } else {
            self.bus.read32(pc)
        };
        let (instr, pass) = self.decode(pc, thumb, raw);
        let full = cycles::cost(&instr);
        if !pass {
            let r = waits::region(pc);
            let w = &self.bus.waits;
            let s = cycles::SKIPPED;
            let one = s.base + w.fetch(r, thumb, s.seq, s.nonseq);
            self.bus.stall_cycles -=
                (full.base + w.fetch(r, thumb, full.seq, full.nonseq)) as i32 - one as i32;
        }
        self.execute(pc, thumb, instr, pass, &mut crate::debug::NoWatch);
        full
    }

    /// The instruction `raw` at `pc`, and whether its condition passes.
    fn decode(&self, pc: u32, thumb: bool, raw: u32) -> (Instr, bool) {
        let instr = if thumb {
            decode_thumb(raw as u16, pc)
        } else {
            decode_arm(raw, pc)
        };
        (instr, self.cpu.cond_passes(instr.cond()))
    }

    /// [`Machine::step`]'s instruction at `pc` once its cost is counted:
    /// the timing that hangs on values, and its effect, without the tick.
    fn execute<W: crate::debug::Watch>(
        &mut self,
        pc: u32,
        thumb: bool,
        instr: Instr,
        pass: bool,
        w: &mut W,
    ) {
        let size = if thumb { 2 } else { 4 };
        // Native timing prices nothing, as its generated code doesn't.
        let priced = self.timing != Timing::Native;
        if pass && priced {
            // What hangs on values (`armv4t::cycles`), from the registers
            // before the instruction runs.
            if let Instr::Mul { rs, .. } | Instr::MulLong { rs, .. } = instr {
                let v = match self.cpu.r.get(rs as usize) {
                    Some(&v) => v,
                    None => pc.wrapping_add(2 * size),
                };
                self.bus.data_cycles += cycles::mul_wait(&instr, v).unwrap_or(0) as i32;
            }
            if let Some(n) = cycles::block_accesses(&instr) {
                self.bus.block(n);
            }
        }
        match instr {
            Instr::Swi { comment, .. } if pass => {
                self.cpu.pc = pc.wrapping_add(size);
                self.swi(comment);
            }
            _ if W::ON => {
                let mut bus = crate::debug::Watched {
                    bus: &mut self.bus,
                    w,
                };
                self.cpu.execute_decoded(&mut bus, instr);
            }
            _ => {
                self.cpu.execute_decoded(&mut self.bus, instr);
            }
        }
        let r = waits::region(pc);
        if r >= 8
            && priced
            && let Some(load) = cycles::stalls(&instr)
        {
            self.bus.stall(r, pc.wrapping_add(2 * size), load);
        }
    }

    /// Run the BIOS code at `pc` as Rust.
    fn bios_step(&mut self, pc: u32) -> bool {
        match pc {
            IRQ_RETURN | MGBA_IRQ_RETURN => {
                self.irq_return();
                self.tick(5)
            }
            bios::NOP_CALL => {
                bios::nop_call(self);
                self.tick(3)
            }
            bios::STALL_LOOP if let Some(n) = bios::stall_step(self) => self.spend(n),
            // A `swi` that took the hardware exception (an exit applied
            // with `Cpu::apply_exit`) rather than coming through `swi`.
            bios::SWI_VECTOR if self.cpu.cpsr & 0x1F == mode::SVC => {
                if let Err(e) = bios::swi_from_exception(self) {
                    self.fault = Some(Fault::Swi(e));
                }
                self.tick(3)
            }
            _ => {
                self.fault = Some(Fault::Bios(pc));
                true
            }
        }
    }

    /// A `swi` with `comment` (ARM's 24-bit field or Thumb's 8-bit one),
    /// called with `cpu.pc` at the next instruction: the BIOS call, in Rust
    /// ([`bios::swi`]). One it doesn't implement sets [`Machine::fault`].
    pub fn swi(&mut self, comment: u32) {
        if let Err(e) = bios::swi(self, comment) {
            self.fault = Some(Fault::Swi(e));
        }
    }

    /// Give the platform the sound since the last hand-off, if any
    /// ([`Platform::audio`]).
    pub fn hand_off_audio(&mut self) {
        let now = self.bus.io.now as i64;
        crate::sound::advance_psg(&mut self.bus, now);
        let s = &self.bus.sound;
        if !s.pending() {
            return;
        }
        self.platform.audio(&s.chunk());
        self.bus.sound.clear();
    }

    /// Hand the frame, drawn line by line at each H-blank, to the platform
    /// and read the keys back into `KEYINPUT`. The runner calls this at the
    /// start of V-blank.
    pub fn present_frame(&mut self) {
        self.hand_off_audio();
        self.show_frame();
    }

    /// [`Machine::present_frame`] but the sound: what a front end running
    /// the guest on its own stack calls after the frame end's switch
    /// ([`crate::dispatch::Nest::frame_end`]).
    pub fn show_frame(&mut self) {
        self.platform.present(&self.ppu.frame);
        let keys = self.platform.keys();
        self.bus.io.set_keys(keys);
    }
}

crate::snap::snap!(Prefetch {
    addr,
    thumb,
    raw,
    at
});
