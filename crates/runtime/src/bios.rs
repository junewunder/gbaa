//! High-level emulation of the BIOS (decision 12): each `swi` routine in
//! Rust, run straight from the call site. No BIOS image is executed.
//!
//! **One entry point**, [`swi`], for the interpreter and generated code
//! alike. The caller sets `cpu.pc` to the instruction after the `swi` and
//! passes the raw comment field. The call runs in the caller's mode, as if
//! the `swi` exception had been taken and returned from, and leaves
//! `cpu.pc` wherever execution continues: after the `swi`, or the reset
//! entry for `SoftReset` and `HardReset`.
//!
//! **Waits.** `Halt`, `Stop`, `CustomHalt`, `IntrWait` and
//! `VBlankIntrWait` halt the CPU the way the real BIOS does: by writing
//! `HALTCNT`, which raises [`event::HALT`](crate::io::event::HALT) or
//! `STOP`. The runner acts on events before it runs the next instruction.
//! `IntrWait` can't finish inside the call, because the game's IRQ handler
//! has to run first and set the BIOS's interrupt flags at [`BIOS_IF`]. So
//! the call records what's [`Pending`] and returns. The runner halts until
//! an interrupt is requested and takes the IRQ, and calls two hooks:
//!
//! - [`irq_enter`] when it enters the BIOS's IRQ stub, before calling the
//!   handler at `0x03007FFC`;
//! - [`irq_return`] after the stub has popped its registers and returned
//!   (`ldmfd sp!, {r0-r3, r12, lr}; subs pc, lr, #4`), back in the
//!   interrupted mode with `cpu.pc` where the IRQ struck.
//!
//! If that return lands back where a pending call left off, at the same
//! handler depth, [`irq_return`] finishes the call: `IntrWait` checks its
//! flags and either ends (setting r0) or halts again. Execution resumes
//! after the `swi` either way, so the wait is invisible to the guest apart
//! from the halt. A return that lands anywhere else at that depth or
//! shallower means the CPU left the wait without the BIOS finishing it (on
//! hardware, IRQs disabled in the CPSR would hang it instead), so the
//! pending call is dropped. Pending calls stack, one per handler depth,
//! like the real BIOS's frames: a handler that waits inside a wait it
//! interrupted leaves the outer one to finish later. These two hooks are the whole interface with
//! the runner's halt and IRQ code (#27); they also move the BIOS open-bus
//! latch.
//!
//! **BIOS reads.** The BIOS region reads [`Bus::bios_latch`], the last
//! opcode the BIOS fetched. Each call sets it as the real BIOS would leave
//! it: [`BIOS_LATCH_AFTER_SWI`] after a call, [`BIOS_LATCH_IN_IRQ`] inside
//! an IRQ handler, [`BIOS_LATCH_AFTER_IRQ`] after one, and
//! [`BIOS_LATCH_AFTER_BOOT`] after `SoftReset`.
//!
//! Results and clobbered registers follow GBATEK, with mGBA's HLE BIOS and
//! the Cult-of-GBA BIOS (which mirrors the original's algorithms) for what
//! GBATEK leaves out. Each routine says which it follows. Calls not
//! implemented here return [`Unimplemented`].

use crate::bus::{
    BIOS_LATCH_AFTER_BOOT, BIOS_LATCH_AFTER_IRQ, BIOS_LATCH_AFTER_SWI, BIOS_LATCH_IN_IRQ, Bus,
};
use crate::io::reg;
use crate::machine::{ROM_ENTRY, SP_IRQ, SP_SVC, SP_SYS, boot_cpu};
use crate::{Machine, Platform};
use armv4t::interp::{Bank, Cpu, T_BIT, mode};

pub(crate) mod decomp;
pub(crate) mod math;
pub mod multiboot;

/// Where the game's IRQ handler records the interrupts it serviced, for
/// `IntrWait` (the BIOS's `IF` mirror). The handler's own address is the
/// word after it.
pub const BIOS_IF: u32 = 0x0300_7FF8;
/// The `swi` exception vector.
pub const SWI_VECTOR: u32 = 0x08;
/// Where the CPU sits while a call's time runs as a stall (mGBA's
/// `StallCall` loop, in its HLE BIOS): see [`swi`].
pub const STALL_LOOP: u32 = 0x34C;
/// The shortest time mGBA runs as a stall rather than at once.
pub const MIN_STALL: u32 = 18;
/// A `bx lr` in mGBA's HLE BIOS (its `NopCall`), which the stubbed sound
/// driver's calls and `SoundDriverGetJumpList`'s table point at. The runner
/// returns from it ([`nop_call`]).
pub const NOP_CALL: u32 = 0x1B0;
/// How many words `SoundDriverGetJumpList` fills.
const JUMP_LIST_WORDS: u32 = 36;
/// The byte `SoftReset` reads to choose where to restart: 0 for the
/// cartridge, anything else for EWRAM.
const RESET_FLAG: u32 = 0x0300_7FFA;
/// `HALTCNT`, the byte above `POSTFLG`.
const HALTCNT: u32 = reg::POSTFLG + 1;

/// The `swi` numbers, as in GBATEK.
pub mod call {
    pub const SOFT_RESET: u8 = 0x00;
    pub const REGISTER_RAM_RESET: u8 = 0x01;
    pub const HALT: u8 = 0x02;
    pub const STOP: u8 = 0x03;
    pub const INTR_WAIT: u8 = 0x04;
    pub const VBLANK_INTR_WAIT: u8 = 0x05;
    pub const DIV: u8 = 0x06;
    pub const DIV_ARM: u8 = 0x07;
    pub const SQRT: u8 = 0x08;
    pub const ARC_TAN: u8 = 0x09;
    pub const ARC_TAN2: u8 = 0x0A;
    pub const CPU_SET: u8 = 0x0B;
    pub const CPU_FAST_SET: u8 = 0x0C;
    pub const GET_BIOS_CHECKSUM: u8 = 0x0D;
    pub const BG_AFFINE_SET: u8 = 0x0E;
    pub const OBJ_AFFINE_SET: u8 = 0x0F;
    pub const BIT_UNPACK: u8 = 0x10;
    pub const LZ77_UNCOMP_WRAM: u8 = 0x11;
    pub const LZ77_UNCOMP_VRAM: u8 = 0x12;
    pub const HUFF_UNCOMP: u8 = 0x13;
    pub const RL_UNCOMP_WRAM: u8 = 0x14;
    pub const RL_UNCOMP_VRAM: u8 = 0x15;
    pub const DIFF8_UNFILTER_WRAM: u8 = 0x16;
    pub const DIFF8_UNFILTER_VRAM: u8 = 0x17;
    pub const DIFF16_UNFILTER: u8 = 0x18;
    pub const SOUND_BIAS: u8 = 0x19;
    pub const SOUND_DRIVER_INIT: u8 = 0x1A;
    pub const SOUND_DRIVER_MODE: u8 = 0x1B;
    pub const SOUND_DRIVER_MAIN: u8 = 0x1C;
    pub const SOUND_DRIVER_VSYNC: u8 = 0x1D;
    pub const SOUND_CHANNEL_CLEAR: u8 = 0x1E;
    pub const MIDI_KEY2FREQ: u8 = 0x1F;
    pub const MUSIC_PLAYER_OPEN: u8 = 0x20;
    pub const MUSIC_PLAYER_START: u8 = 0x21;
    pub const MUSIC_PLAYER_STOP: u8 = 0x22;
    pub const MUSIC_PLAYER_CONTINUE: u8 = 0x23;
    pub const MUSIC_PLAYER_FADE_OUT: u8 = 0x24;
    pub const MULTI_BOOT: u8 = 0x25;
    pub const HARD_RESET: u8 = 0x26;
    pub const CUSTOM_HALT: u8 = 0x27;
    pub const SOUND_DRIVER_VSYNC_OFF: u8 = 0x28;
    pub const SOUND_DRIVER_VSYNC_ON: u8 = 0x29;
    pub const SOUND_DRIVER_GET_JUMP_LIST: u8 = 0x2A;

    /// Each call's name, by number: pret's `libagbsyscall` and GBATEK's,
    /// for the readable form's comments.
    pub const NAMES: [&str; 0x2B] = [
        "SoftReset",
        "RegisterRamReset",
        "Halt",
        "Stop",
        "IntrWait",
        "VBlankIntrWait",
        "Div",
        "DivArm",
        "Sqrt",
        "ArcTan",
        "ArcTan2",
        "CpuSet",
        "CpuFastSet",
        "GetBiosChecksum",
        "BgAffineSet",
        "ObjAffineSet",
        "BitUnPack",
        "LZ77UnCompWram",
        "LZ77UnCompVram",
        "HuffUnComp",
        "RLUnCompWram",
        "RLUnCompVram",
        "Diff8bitUnFilterWram",
        "Diff8bitUnFilterVram",
        "Diff16bitUnFilter",
        "SoundBias",
        "SoundDriverInit",
        "SoundDriverMode",
        "SoundDriverMain",
        "SoundDriverVSync",
        "SoundChannelClear",
        "MidiKey2Freq",
        "MusicPlayerOpen",
        "MusicPlayerStart",
        "MusicPlayerStop",
        "MusicPlayerContinue",
        "MusicPlayerFadeOut",
        "MultiBoot",
        "HardReset",
        "CustomHalt",
        "SoundDriverVSyncOff",
        "SoundDriverVSyncOn",
        "SoundGetJumpList",
    ];

    /// Call `n`'s name, if it is a call.
    pub fn name(n: u8) -> Option<&'static str> {
        NAMES.get(n as usize).copied()
    }

    /// The sound driver's calls, which return at once as in mGBA's HLE
    /// BIOS: a game that plays music through the BIOS's driver is silent.
    pub const SOUND_DRIVER: &[u8] = &[
        SOUND_DRIVER_INIT,
        SOUND_DRIVER_MODE,
        SOUND_DRIVER_MAIN,
        SOUND_DRIVER_VSYNC,
        SOUND_CHANNEL_CLEAR,
        MUSIC_PLAYER_OPEN,
        MUSIC_PLAYER_START,
        MUSIC_PLAYER_STOP,
        MUSIC_PLAYER_CONTINUE,
        MUSIC_PLAYER_FADE_OUT,
        SOUND_DRIVER_VSYNC_OFF,
        SOUND_DRIVER_VSYNC_ON,
    ];

    /// The calls [`swi`](super::swi) runs; any other number is
    /// [`Unimplemented`](super::Unimplemented).
    pub const IMPLEMENTED: &[u8] = &[
        SOFT_RESET,
        REGISTER_RAM_RESET,
        HALT,
        STOP,
        INTR_WAIT,
        VBLANK_INTR_WAIT,
        DIV,
        DIV_ARM,
        SQRT,
        ARC_TAN,
        ARC_TAN2,
        CPU_SET,
        CPU_FAST_SET,
        GET_BIOS_CHECKSUM,
        BG_AFFINE_SET,
        OBJ_AFFINE_SET,
        BIT_UNPACK,
        LZ77_UNCOMP_WRAM,
        LZ77_UNCOMP_VRAM,
        HUFF_UNCOMP,
        RL_UNCOMP_WRAM,
        RL_UNCOMP_VRAM,
        DIFF8_UNFILTER_WRAM,
        DIFF8_UNFILTER_VRAM,
        DIFF16_UNFILTER,
        SOUND_BIAS,
        SOUND_DRIVER_INIT,
        SOUND_DRIVER_MODE,
        SOUND_DRIVER_MAIN,
        SOUND_DRIVER_VSYNC,
        SOUND_CHANNEL_CLEAR,
        MIDI_KEY2FREQ,
        MUSIC_PLAYER_OPEN,
        MUSIC_PLAYER_START,
        MUSIC_PLAYER_STOP,
        MUSIC_PLAYER_CONTINUE,
        MUSIC_PLAYER_FADE_OUT,
        MULTI_BOOT,
        HARD_RESET,
        CUSTOM_HALT,
        SOUND_DRIVER_VSYNC_OFF,
        SOUND_DRIVER_VSYNC_ON,
        SOUND_DRIVER_GET_JUMP_LIST,
    ];
}

/// A `swi` this BIOS doesn't implement. Running on would silently skip
/// whatever the game asked for, so callers stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unimplemented {
    /// The BIOS call number.
    pub number: u8,
    /// The address of the `swi` instruction.
    pub pc: u32,
}

impl core::fmt::Display for Unimplemented {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "unimplemented swi {:#04x} at {:#010x}",
            self.number, self.pc
        )
    }
}

/// How many nested pending calls are kept; past that the oldest is
/// dropped.
const MAX_PENDING: usize = 4;

/// BIOS state that outlives a call: the calls waiting on an IRQ, and how
/// deep in IRQ handlers the CPU is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bios {
    /// The pending calls, innermost last, each deeper than the one before.
    pending: [Pending; MAX_PENDING],
    len: usize,
    /// IRQ handlers entered and not yet returned (handlers that re-enable
    /// IRQs can nest).
    pub irq_depth: u32,
    /// The cycles left of a call's stall ([`STALL_LOOP`]), and where it
    /// returns to.
    stall: u32,
    stall_ret: u32,
    /// A `MultiBoot` exchange in progress, as the parent or as a machine
    /// with no cartridge waiting for its program ([`multiboot`]).
    pub multiboot: multiboot::MultiBoot,
}

impl Bios {
    /// The calls waiting on an IRQ, innermost last.
    pub fn pending(&self) -> &[Pending] {
        &self.pending[..self.len]
    }

    fn top(&self) -> Option<Pending> {
        self.pending().last().copied()
    }

    fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    /// Record a call made at `p.depth`. Any record at that depth or deeper
    /// is dead: the CPU is running a new `swi` there.
    pub(crate) fn push(&mut self, p: Pending) {
        while self.top().is_some_and(|t| t.depth >= p.depth) {
            self.pop();
        }
        if self.len == MAX_PENDING {
            self.pending.copy_within(1.., 0);
            self.len -= 1;
        }
        self.pending[self.len] = p;
        self.len += 1;
    }
}

/// A call that halted and finishes when an IRQ returns to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    /// Where the call returns to: the address after the `swi`.
    pub pc: u32,
    /// [`Bios::irq_depth`] when it was called.
    pub depth: u32,
    /// The interrupt flags an `IntrWait` waits for; 0 for `Halt`, which
    /// ends at the first IRQ.
    pub mask: u16,
}

/// Run the BIOS call a `swi` asks for. `comment` is the instruction's raw
/// comment field (24 bits in ARM state, 8 in Thumb; the CPSR's T bit says
/// which), and `cpu.pc` is the address after the `swi`. On return `cpu.pc`
/// is where execution continues, and a wait has raised an event. An
/// unimplemented call changes nothing.
///
/// The call's time is ticked as mGBA's HLE BIOS takes it (see [`call`]).
/// Most of it is ticked here, and an IRQ it raises is taken after the call
/// returns. The time of LZ77, `CpuSet` and `CpuFastSet` is a stall, as in
/// mGBA: the call returns with `cpu.pc` at [`STALL_LOOP`], in the caller's
/// mode, and the runner burns the time there ([`stall_step`]), taking IRQs
/// and ending frames inside it; the handlers return to it. A long
/// decompression into VRAM can last frames, and run at once it merged
/// their V-blank IRQs into one; a `CpuSet` clearing VRAM takes most of a
/// frame, and run at once it starved Emerald's link of serial IRQs (the
/// trade screen's Summary).
pub fn swi<P: Platform>(m: &mut Machine<P>, comment: u32) -> Result<(), Unimplemented> {
    match number(comment, m.cpu.thumb()) {
        call::HARD_RESET => {
            hard_reset(m);
            return Ok(());
        }
        call::MULTI_BOOT => {
            // Sending runs at `multiboot::LOOP`; the call's entry is mGBA's
            // 45 cycles, as every call's.
            multiboot::start(m);
            m.bus.bios_latch = BIOS_LATCH_AFTER_SWI;
            if m.timing != crate::Timing::Native {
                m.tick(45);
            }
            return Ok(());
        }
        _ => {}
    }
    let (mut cycles, stall) = call(&mut m.cpu, &mut m.bus, &mut m.bios, comment)?;
    // A stall inside a handler that interrupted one runs at once.
    let at_once = if stall >= MIN_STALL && m.bios.stall == 0 {
        m.bios.stall = stall;
        m.bios.stall_ret = m.cpu.pc;
        m.cpu.pc = STALL_LOOP;
        0
    } else {
        stall
    };
    if m.timing == crate::Timing::Native {
        // Native timing: the call's work (the copies' and LZ77's stall)
        // takes its time, the rest none (docs/native-port-design.md,
        // section 7).
        if at_once > 0 {
            m.spend(at_once);
            // The tick after the `swi` says what the sync found.
            m.sched.fresh = true;
        }
        return Ok(());
    }
    cycles += at_once;
    if cycles > 0 {
        m.tick(cycles);
    }
    Ok(())
}

/// Run some of a stall at [`STALL_LOOP`]: up to the next event, so an IRQ
/// or the frame's end comes on time. Returns the cycles it took; when the
/// stall is over, `cpu.pc` is back after the `swi`. None when no stall is
/// running.
pub fn stall_step<P: Platform>(m: &mut Machine<P>) -> Option<u32> {
    if m.bios.stall == 0 {
        return None;
    }
    let room = m.sched.next_event.saturating_sub(m.now()).max(1);
    let n = (m.bios.stall as u64).min(room) as u32;
    m.bios.stall -= n;
    if m.bios.stall == 0 {
        m.cpu.pc = m.bios.stall_ret;
        m.bus.bios_latch = BIOS_LATCH_AFTER_SWI;
    }
    Some(n)
}

/// The CPU is at [`NOP_CALL`], called through the sound driver's jump
/// list: `bx lr`. The latch is left at the last opcode it fetched, the
/// word two after the `bx` in mGBA's HLE BIOS.
pub fn nop_call<P: Platform>(m: &mut Machine<P>) {
    let lr = m.cpu.r[14];
    m.cpu
        .apply_exit(NOP_CALL, false, Some(armv4t::interp::Exit::Exchange(lr)));
    m.bus.bios_latch = NOP_CALL_LATCH;
}

/// The opcode at `NOP_CALL + 8` in mGBA's HLE BIOS (`mov r12, #0x04000000`).
const NOP_CALL_LATCH: u32 = 0xE3A0_C301;

/// [`swi`] for a `swi` that took the hardware exception: SVC mode, `pc`
/// at [`SWI_VECTOR`] (the interpreter normally calls [`swi`] directly).
/// Reads the comment field from the instruction before `lr_svc`, returns
/// to the caller's mode and state, and runs the call. `lr_svc` and
/// `spsr_svc` keep what the exception wrote, as on hardware.
pub fn swi_from_exception<P: Platform>(m: &mut Machine<P>) -> Result<(), Unimplemented> {
    let spsr = m.cpu.spsr();
    let ret = m.cpu.r[14];
    let comment = if spsr & T_BIT != 0 {
        m.bus.read16(ret.wrapping_sub(2)) as u32 & 0xFF
    } else {
        m.bus.read32(ret.wrapping_sub(4)) & 0xFF_FFFF
    };
    m.cpu.write_cpsr(spsr, !0);
    m.cpu.pc = ret;
    swi(m, comment)
}

/// The runner is entering the BIOS's IRQ stub, about to call the game's
/// handler.
pub fn irq_enter<P: Platform>(m: &mut Machine<P>) {
    m.bios.irq_depth += 1;
    m.bus.bios_latch = BIOS_LATCH_IN_IRQ;
}

/// The BIOS's IRQ stub has returned to the interrupted code (`cpu.pc` is
/// where the IRQ struck). Finishes a call pending there, or drops one the
/// CPU has left.
pub fn irq_return<P: Platform>(m: &mut Machine<P>) {
    let bios = &mut m.bios;
    let depth = bios.irq_depth.saturating_sub(1);
    bios.irq_depth = depth;
    m.bus.bios_latch = BIOS_LATCH_AFTER_IRQ;
    // Calls made inside handlers that have now returned are dead.
    while bios.top().is_some_and(|p| p.depth > depth) {
        bios.pop();
    }
    // One made here finishes if the IRQ struck it, and is dead otherwise.
    // One made shallower waits for its own handler level.
    let Some(p) = bios.top().filter(|p| p.depth == depth) else {
        return;
    };
    if m.cpu.pc != p.pc {
        bios.pop();
    } else if p.mask == 0 {
        bios.pop();
        m.bus.bios_latch = BIOS_LATCH_AFTER_SWI;
    } else {
        intr_wait_check(&mut m.cpu, &mut m.bus, bios, p.mask);
    }
}

/// The call number in a `swi`'s comment field: the low byte in Thumb, bits
/// 16-23 in ARM (the BIOS reads the byte at `lr - 2` either way).
pub fn number(comment: u32, thumb: bool) -> u8 {
    if thumb {
        comment as u8
    } else {
        (comment >> 16) as u8
    }
}

/// Runs call `comment` and returns the cycles it takes besides the `swi`
/// itself: mGBA's 45 for entering and leaving the BIOS, plus the routine's
/// own time where mGBA counts one (`Div`'s loop), and apart, the time of
/// the copies and LZ77, which [`swi`] may run as a stall. The waits and
/// `SoftReset` take none here.
fn call(
    cpu: &mut Cpu,
    bus: &mut Bus,
    bios: &mut Bios,
    comment: u32,
) -> Result<(u32, u32), Unimplemented> {
    const ENTRY: u32 = 45;
    let thumb = cpu.thumb();
    let n = number(comment, thumb);
    match n {
        call::SOFT_RESET => {
            soft_reset(cpu, bus, bios);
            return Ok((0, 0));
        }
        call::INTR_WAIT | call::VBLANK_INTR_WAIT => {
            if n == call::VBLANK_INTR_WAIT {
                cpu.r[0] = 1;
                cpu.r[1] = 1;
            }
            intr_wait(cpu, bus, bios);
            return Ok((0, 0));
        }
        _ => {}
    }
    let r = &mut cpu.r;
    #[cfg(feature = "provenance")]
    let (_writing, entry) = (crate::prov::enter_bios(n, r[0], r[1]), (r[0], r[1], r[2]));
    let mut cycles = ENTRY;
    let mut stall = 0;
    match n {
        call::REGISTER_RAM_RESET => register_ram_reset(bus, r[0]),
        call::HALT | call::STOP | call::CUSTOM_HALT => {
            let v = match n {
                call::HALT => 0,
                call::STOP => 0x80,
                _ => r[2] as u8,
            };
            // The BIOS finishes the call after the IRQ that wakes it.
            bios.push(Pending {
                pc: cpu.pc,
                depth: bios.irq_depth,
                mask: 0,
            });
            bus.write8(io_addr(HALTCNT), v);
            cycles = 0;
        }
        call::DIV | call::DIV_ARM => {
            let (num, den) = if n == call::DIV {
                (r[0], r[1])
            } else {
                (r[1], r[0])
            };
            // mGBA: a prologue, 13 a quotient bit, an epilogue.
            let loops = (den.leading_zeros() as i32 - num.leading_zeros() as i32).max(1);
            cycles += 4 + 13 * loops as u32 + 7;
            math::div(r, num as i32, den as i32);
        }
        call::SQRT => r[0] = math::sqrt(r[0]),
        call::ARC_TAN => math::arc_tan(r),
        call::ARC_TAN2 => math::arc_tan2(r),
        // mGBA runs these in its BIOS's own code, where IRQs land.
        call::CPU_SET => stall = decomp::cpu_set(bus, r),
        call::CPU_FAST_SET => stall = decomp::cpu_fast_set(bus, r),
        // mGBA's values for the real BIOS.
        call::GET_BIOS_CHECKSUM => {
            r[0] = 0xBAAE_187F;
            r[1] = 1;
            r[3] = 0x4000;
        }
        call::BG_AFFINE_SET => math::bg_affine_set(bus, r),
        call::OBJ_AFFINE_SET => math::obj_affine_set(bus, r),
        call::BIT_UNPACK => decomp::bit_unpack(bus, r),
        call::LZ77_UNCOMP_WRAM | call::LZ77_UNCOMP_VRAM => {
            // The `swi`'s address plus two fetches, as mGBA's pc reads.
            let pc = cpu.pc.wrapping_add(if thumb { 2 } else { 4 });
            stall = decomp::lz77(bus, r, n == call::LZ77_UNCOMP_VRAM, pc);
        }
        call::HUFF_UNCOMP => decomp::huffman(bus, r),
        call::RL_UNCOMP_WRAM => decomp::rl(bus, r, false),
        call::RL_UNCOMP_VRAM => decomp::rl(bus, r, true),
        call::SOUND_BIAS => sound_bias(bus, r[0]),
        call::DIFF8_UNFILTER_WRAM => decomp::unfilter(bus, r, false, false),
        call::DIFF8_UNFILTER_VRAM => decomp::unfilter(bus, r, false, true),
        call::DIFF16_UNFILTER => decomp::unfilter(bus, r, true, true),
        call::MIDI_KEY2FREQ => math::midi_key2freq(bus, r),
        n if call::SOUND_DRIVER.contains(&n) => {}
        // mGBA's HLE routine: 36 words of NopCall at r0, which ends past
        // them; r1 is 0 and r3 NopCall. mGBA runs its BIOS code for this
        // one, so its time differs from the 45 cycles taken here.
        call::SOUND_DRIVER_GET_JUMP_LIST => {
            for i in 0..JUMP_LIST_WORDS {
                bus.write32(r[0].wrapping_add(4 * i), NOP_CALL);
            }
            r[0] = r[0].wrapping_add(4 * JUMP_LIST_WORDS);
            r[1] = 0;
            r[3] = NOP_CALL;
        }
        // Run by `swi` (`multiboot::start`), which needs the machine.
        call::MULTI_BOOT => unreachable!("MultiBoot runs in swi"),
        number => {
            let size = if thumb { 2 } else { 4 };
            return Err(Unimplemented {
                number,
                pc: cpu.pc.wrapping_sub(size),
            });
        }
    }
    #[cfg(feature = "provenance")]
    crate::prov::bios_loaded(n, entry.0, entry.1, entry.2, &cpu.r);
    bus.bios_latch = BIOS_LATCH_AFTER_SWI;
    Ok((cycles, stall))
}

fn io_addr(o: u32) -> u32 {
    0x0400_0000 + o
}

/// `IntrWait` (r0: discard old flags, r1: the interrupts to wait for).
/// GBATEK: forces `IME` on, and with r0 = 0 returns at once if a flag was
/// already set. Like mGBA, it checks before halting only then.
fn intr_wait(cpu: &mut Cpu, bus: &mut Bus, bios: &mut Bios) {
    let mask = cpu.r[1] as u16;
    bios.push(Pending {
        pc: cpu.pc,
        depth: bios.irq_depth,
        mask,
    });
    if cpu.r[0] != 0 {
        let f = bus.read16(BIOS_IF);
        bus.write16(BIOS_IF, f & !mask);
        bus.write16(io_addr(reg::IME), 1);
        bus.write8(io_addr(HALTCNT), 0);
    } else {
        intr_wait_check(cpu, bus, bios, mask);
    }
}

/// One turn of `IntrWait`'s loop: with `IME` off, take the flags it waits
/// for out of [`BIOS_IF`], then turn `IME` back on. Ends the wait (the
/// innermost pending call) if any
/// were set, with r0 = those flags (the Cult-of-GBA BIOS's `ands r0, r1,
/// r4`; mGBA leaves 0), and halts again otherwise.
fn intr_wait_check(cpu: &mut Cpu, bus: &mut Bus, bios: &mut Bios, mask: u16) {
    bus.write16(io_addr(reg::IME), 0);
    let f = bus.read16(BIOS_IF);
    let hit = f & mask;
    if hit != 0 {
        bus.write16(BIOS_IF, f ^ hit);
    }
    bus.write16(io_addr(reg::IME), 1);
    if hit != 0 {
        cpu.r[0] = hit as u32;
        bios.pop();
        bus.bios_latch = BIOS_LATCH_AFTER_SWI;
    } else {
        bus.write8(io_addr(HALTCNT), 0);
    }
}

/// `SoftReset` (GBATEK): clear the top 0x200 bytes of IWRAM (stacks and
/// the IRQ vector), reset the stacks, zero r0-r12, `lr_svc`, `spsr_svc`,
/// `lr_irq` and `spsr_irq`, and restart in system mode at the cartridge,
/// or EWRAM if the byte at `0x03007FFA` was non-zero.
fn soft_reset(cpu: &mut Cpu, bus: &mut Bus, bios: &mut Bios) {
    let to_ram = bus.read8(RESET_FLAG) != 0;
    for a in (0x0300_7E00..0x0300_8000).step_by(4) {
        bus.write32(a, 0);
    }
    cpu.switch_mode(mode::SYS);
    for (bank, sp) in [(Bank::Svc, SP_SVC), (Bank::Irq, SP_IRQ)] {
        cpu.banks[bank as usize][5] = sp;
        cpu.banks[bank as usize][6] = 0;
        cpu.spsr[bank as usize] = 0;
    }
    cpu.r = [0; 15];
    cpu.r[13] = SP_SYS;
    cpu.cpsr = mode::SYS;
    cpu.pc = if to_ram { 0x0200_0000 } else { ROM_ENTRY };
    *bios = Bios::default();
    bus.bios_latch = BIOS_LATCH_AFTER_BOOT;
}

/// `SoundBias` (r0: 0 for level 0, anything else for 0x200), from GBATEK:
/// moves `SOUNDBIAS`'s level (bits 0-9) to the new one and keeps the upper
/// bits. The BIOS steps the level there with a short delay between steps;
/// this lands it at once, as one store, and takes only the entry time.
/// mGBA's HLE BIOS leaves the register alone.
fn sound_bias(bus: &mut Bus, level: u32) {
    let to = if level == 0 { 0 } else { 0x200 };
    let old = bus.read16(io_addr(reg::SOUNDBIAS));
    bus.write16(io_addr(reg::SOUNDBIAS), old & !0x3FF | to);
}

/// `HardReset`: restart as at power-on, the state [`Machine::new`] makes
/// (which is the desktop's reset), keeping the cartridge (its save and
/// RTC). The real BIOS jumps to its own reset code, which clears memory
/// and the registers and boots the cartridge again; the LCD and the clock
/// run on through it. So this resets the machine in place rather than
/// building a new one: the clock never goes back under a run in progress.
/// The boot logo isn't shown, as `Machine::new` doesn't show it. With no
/// cartridge, the BIOS waits for a program over the cable again
/// ([`multiboot::boot_cartless`]).
fn hard_reset<P: Platform>(m: &mut Machine<P>) {
    let bus = &mut m.bus;
    register_ram_reset(bus, 0xFF);
    bus.iwram[0x7E00..].fill(0);
    // Empty both sound FIFOs.
    bus.write16(io_addr(reg::SOUNDCNT_H), 0x8800);
    bus.write16(io_addr(reg::SOUNDCNT_H), 0);
    // What `RegisterRamReset` leaves unlike power-on.
    bus.io.write16(reg::DISPCNT, 0);
    bus.io.write16(reg::KEYCNT, 0);
    bus.waitcnt_written();
    bus.bios_latch = BIOS_LATCH_AFTER_BOOT;
    m.cpu = boot_cpu();
    m.bios = Bios::default();
    m.ppu.reset();
    m.sched.halted = false;
    m.prefetch.at = u64::MAX;
    if m.bus.rom.is_empty() {
        // No cartridge: the BIOS waits for a program over the cable again.
        multiboot::boot_cartless(m);
    }
}

/// `RegisterRamReset` (r0: what to reset), with the register values mGBA
/// uses. `DISPCNT` is always set to forced blank. IWRAM's top 0x200 bytes
/// are kept. Registers are written as the CPU would write them, so DMA,
/// timer and IRQ writes raise their events.
fn register_ram_reset(bus: &mut Bus, flags: u32) {
    bus.io.write16(reg::DISPCNT, 0x80);
    if flags & 0x01 != 0 {
        bus.ewram.fill(0);
    }
    if flags & 0x02 != 0 {
        bus.iwram[..0x7E00].fill(0);
        bus.overlays.touch_all();
    }
    if flags & 0x04 != 0 {
        bus.video.palette.fill(0);
    }
    if flags & 0x08 != 0 {
        bus.video.vram.fill(0);
    }
    if flags & 0x10 != 0 {
        bus.video.oam.fill(0);
    }
    // Walk the sound FIFOs' overdue samples on the old settings first, as a
    // store would (crate::sound::before_io_write).
    crate::sound::run_timers(bus);
    if flags & 0x40 != 0 {
        // The sound channels, SOUNDCNT_L/H/X and wave RAM; SOUNDBIAS. As
        // stores, so the PSG sees them.
        for o in [
            0x60, 0x62, 0x64, 0x68, 0x6C, 0x70, 0x72, 0x74, 0x78, 0x7C, 0x80, 0x82, 0x84,
        ] {
            bus.write16(0x0400_0000 + o, 0);
        }
        for o in (reg::WAVE_RAM..reg::WAVE_RAM + 0x10).step_by(2) {
            bus.write16(0x0400_0000 + o, 0);
        }
        bus.write16(0x0400_0000 + reg::SOUNDBIAS, 0x200);
    }
    let io = &mut bus.io;
    if flags & 0x20 != 0 {
        // SIOCNT, SIOMLT_SEND, JOYCNT, JOY_RECV, JOY_TRANS; RCNT.
        for o in [0x128, 0x12A, 0x140, 0x150, 0x152, 0x154, 0x156] {
            io.write16(o, 0);
        }
        io.write16(reg::RCNT, 0x8000);
    }
    if flags & 0x80 != 0 {
        // Everything else: display, DMA, timers, interrupts, WAITCNT. The
        // affine backgrounds get the identity matrix.
        let display = (reg::DISPSTAT..=reg::BLDY).step_by(2);
        let dma = (reg::DMA0SAD..0x0E0).step_by(2);
        let timers = (reg::TM0CNT_L..0x110).step_by(2);
        for o in display.chain(dma).chain(timers) {
            let identity = matches!(o, 0x20 | 0x26 | 0x30 | 0x36);
            io.write16(o, if identity { 0x100 } else { 0 });
        }
        io.write16(reg::IE, 0);
        io.write16(reg::IF, 0xFFFF);
        io.write16(reg::WAITCNT, 0);
        io.write16(reg::IME, 0);
    }
}

crate::snap::snap!(Bios {
    pending,
    len,
    irq_depth,
    stall,
    stall_ret,
    multiboot
});
crate::snap::snap!(Pending { pc, depth, mask });

#[cfg(test)]
mod tests;
