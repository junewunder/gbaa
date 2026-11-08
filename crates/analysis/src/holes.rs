//! What blocks a native build of a game (`docs/native-port-design.md`,
//! section 6): the hole classification the hole report prints and the
//! lifter reuses.
//!
//! Inputs are the ROM's [`Discovery`], its RAM overlays
//! ([`crate::discovery::overlays`], or with the run's evidence
//! [`ram_copies`]), what a coverage run saw ([`Run`]:
//! filled by `gbaa-holes`, which runs the interpreter) and, optionally, the
//! holes the function summaries recorded (`codegen::saves::summarize`).
//! [`classify`] sorts them into [`Report`]'s lists.
//!
//! **Hard** holes ([`Report::hard`]) are the ones no printed program can
//! stand in for without a hand-written replacement or a fact added by hand:
//!
//! - an indirect jump [`Recovery`](crate::jumptable::Recovery) leaves
//!   unresolved or rejected (all of them, reached or not: the function
//!   can't be printed either way), in the ROM or in an overlay;
//! - return-address arithmetic whose call site Recovery doesn't resolve:
//!   found in the run (a return that lands near, but not on, the return
//!   site of the call it closes: WarioWare's `sub lr, lr, #24`), or
//!   statically (a function that does arithmetic on its entry lr before
//!   writing lr: libgcc's `__gnu_thumb1_case_*`, resolved where Recovery
//!   resolves the `bl` as a jump);
//! - code in RAM that changed while it ran (run, then rewritten, then run
//!   again);
//! - CPU state with no native meaning: `msr`/`mrs`, user-bank `ldm`/`stm`,
//!   exception returns and stack switches, outside the IRQ wrapper (code
//!   the run executed at the top level of an IRQ, and the rest of its
//!   function) and outside boot (code that ran only in the first frame, and
//!   the rest of its function: crt0 setting up the stacks);
//! - a BIOS call the native lifter doesn't print ([`bios_implemented`]);
//! - a call through a register the run saw land on an address that isn't
//!   a known function start;
//! - a function summary whose hole breaks the lift's calling convention
//!   ([`SUMMARY_HARD`]): a return with sp changed (a run-time sp
//!   adjustment: a variable-length array), a read of r4-r11 a callee
//!   changed or that came in as a parameter.
//!
//! The flash save library's stack routines ([`crate::flash`]) are none of
//! these: a call into RAM whose target held a recognised routine every time
//! it was called is a hardware-library call ([`Report::flash`]), and the
//! RAM it ran (changed or not, discovered or not) isn't counted. The
//! exemption is by address: other code run there without a call into it
//! (a fall-through, a plain `bx`) would go uncounted too; the run keeps
//! only each address's first bytes, so it can't tell.
//!
//! **Not discovered** ([`Report::undiscovered_bytes`]) is the bytes of code
//! the run executed that discovery (or an overlay's) didn't find. It ranks
//! games second, after hard holes. Pointer arithmetic the types don't
//! explain isn't measured until stage B (it needs the typed lift), and
//! cycle-timing dependence is the native-timing play build's to find.

use std::collections::{BTreeMap, BTreeSet};

use armv4t::decode::{AluOp, Instr, Operand2, Reg, decode_arm, decode_thumb};

use crate::discovery::{Discovery, Overlay};
use crate::program::ROM_BASE;
use crate::vsa::func::JumpRes;

/// One instruction address the run executed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub thumb: bool,
    /// The halfword or word first executed there.
    pub raw: u32,
    /// Executed later with a different raw value: the code changed.
    pub changed: bool,
    /// Executed at the top level of an IRQ (in the handler itself, not a
    /// function it calls).
    pub wrapper: bool,
    /// Executed after the first frame.
    pub after_boot: bool,
    /// Changed the CPU mode (outside a `swi`).
    pub mode_change: bool,
    /// Moved sp by [`STACK_SWITCH`] or more in one instruction, in one
    /// mode, other than by sp-relative arithmetic (`sub sp, sp, #…`).
    pub stack_switch: bool,
}

/// A move of sp at least this far in one instruction is a stack switch.
pub const STACK_SWITCH: u32 = 0x1000;

/// A return the run saw land near, but not on, the return site of the
/// call it closes (the first such return from each call site to each
/// place).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetArith {
    /// The call instruction (a Thumb `bl`'s first half).
    pub site: u32,
    /// What it called, in interworking form.
    pub callee: u32,
    /// The instruction that returned.
    pub at: u32,
    /// Where it returned.
    pub to: u32,
    /// Where it returned, minus the return site.
    pub delta: i32,
    pub count: u64,
}

/// What a coverage run saw.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub frames: u32,
    pub instructions: u64,
    /// Every instruction address executed (aligned, no Thumb bit).
    pub code: BTreeMap<u32, Seen>,
    /// The IRQ handlers the run entered, as installed at `0x0300_7ffc`.
    pub irq_handlers: BTreeSet<u32>,
    /// By call site and where it returned.
    pub ret_arith: BTreeMap<(u32, u32), RetArith>,
    /// Targets of calls through a register (interworking form), each with a
    /// call site.
    pub indirect_calls: BTreeMap<u32, u32>,
    /// Every call target seen, in interworking form: function starts for
    /// code discovery didn't find.
    pub call_targets: BTreeSet<u32>,
    /// Busy-wait loops that call functions (Emerald's `DoMapLoadLoop`):
    /// a backward branch outside IRQs taken at least [`BUSY_ITERATIONS`]
    /// times in a row, in one frame, by iterations that each made a call,
    /// didn't halt, loaded RAM an IRQ handler had written, and made the
    /// same stores (addresses and values) as the iteration before: no
    /// progress of their own. By the branch's address: the longest such
    /// streak.
    pub busy_loops: BTreeMap<u32, u32>,
    /// Calls into RAM: by target (interworking form), the distinct
    /// [`RAM_CALL_BYTES`] bytes there when called. The flash library's
    /// stack routines are recognised from these ([`crate::flash`]).
    pub ram_calls: BTreeMap<u32, BTreeSet<Vec<u8>>>,
}

/// How many bytes at a RAM call's target the run keeps: more than the
/// longest routine [`crate::flash`] recognises.
pub const RAM_CALL_BYTES: u32 = 64;

/// How many qualifying iterations in a row make a busy-wait loop. A wait
/// spins until the interrupt comes, up to a frame's worth of iterations
/// (Emerald Rogue's map load: 186); a pure loop over a short list (a
/// `FlagGet` per item: 27) stops sooner.
pub const BUSY_ITERATIONS: u32 = 64;

/// The BIOS calls the native lifter prints (`gbaa-lift`'s prelude,
/// `crates/codegen/src/native/prelude.rs`): `Halt`, `Stop`, `IntrWait`,
/// `VBlankIntrWait`, `Div`, `Sqrt`, `ArcTan2`, `CpuSet`, `CpuFastSet`,
/// the LZ77 and RL decompressors, `SoundBias`, `MidiKey2Freq` and
/// `MultiBoot`. Every other call is a hard hole in a native build, though
/// the play build's BIOS (`runtime::bios`) has nearly all of them.
pub const NATIVE_BIOS: &[u8] = &[
    0x02, 0x03, 0x04, 0x05, 0x06, 0x08, 0x0A, 0x0B, 0x0C, 0x11, 0x12, 0x14, 0x15, 0x19, 0x1F, 0x25,
];

/// Whether a native build can make BIOS call `n`: the lifter prints it
/// ([`NATIVE_BIOS`]).
pub fn bios_implemented(n: u32) -> bool {
    u8::try_from(n).is_ok_and(|n| NATIVE_BIOS.contains(&n))
}

/// An indirect jump Recovery doesn't resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unresolved {
    pub at: u32,
    /// Rejected (a table with a bad target) rather than unresolved.
    pub rejected: bool,
    /// The run executed it.
    pub reached: bool,
}

/// Return-address arithmetic, from the run or found statically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetHole {
    /// The function doing it, in interworking form.
    pub func: u32,
    /// The instruction doing the arithmetic (static), or the return (run).
    pub at: u32,
    /// The run's delta, for one seen in the run.
    pub delta: Option<i32>,
    /// Call sites: discovered `bl`s to `func` (static), or the run's site.
    pub sites: Vec<u32>,
    /// Every site resolves as a jump in Recovery.
    pub resolved: bool,
    /// Not resolved, but every site is a jump Recovery leaves unresolved
    /// or rejected: counted there, not here.
    pub as_jump: bool,
    pub reached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CpuKind {
    /// `msr` that changed the mode in the run, or ran only in the first
    /// frame, or wasn't run.
    Msr,
    /// `msr` that ran without changing the mode (masking IRQs).
    MsrMask,
    Mrs,
    /// `ldm`/`stm` with the S bit: user-bank registers or an exception
    /// return.
    UserBank,
    /// A data-processing `s` write to pc: an exception return.
    ExceptionReturn,
    /// One instruction moved sp by [`STACK_SWITCH`] or more.
    StackSwitch,
}

impl CpuKind {
    pub fn text(self) -> &'static str {
        match self {
            CpuKind::Msr => "msr (mode switch)",
            CpuKind::MsrMask => "msr (IRQ mask only)",
            CpuKind::Mrs => "mrs",
            CpuKind::UserBank => "ldm/stm with S bit",
            CpuKind::ExceptionReturn => "exception return",
            CpuKind::StackSwitch => "stack switch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CpuWhere {
    /// In the IRQ wrapper: recognised, no hole.
    Wrapper,
    /// In boot code (crt0): recognised, no hole.
    Boot,
    /// Anywhere else: a hard hole.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuHole {
    pub at: u32,
    pub kind: CpuKind,
    pub place: CpuWhere,
    pub reached: bool,
}

/// A run of RAM code, `start..end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RamRun {
    pub start: u32,
    pub end: u32,
    /// Where its first-run bytes are in the ROM, if they are.
    pub rom_src: Option<u32>,
}

/// A call into RAM the hardware library stands in for: the flash
/// library's stack routine `routine` at `target` (interworking form),
/// `len` bytes of code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FlashCall {
    pub target: u32,
    pub routine: crate::flash::Routine,
    pub len: u32,
}

/// Summary holes of one kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SummaryKind {
    /// Function entries.
    pub funcs: BTreeSet<u32>,
    pub hard: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    /// The ROM's Recovery: indirect jumps unresolved or rejected.
    pub unresolved: Vec<Unresolved>,
    /// The overlays' Recovery.
    pub overlay_unresolved: Vec<Unresolved>,
    pub ret: Vec<RetHole>,
    /// RAM code that changed while it ran, but for the flash routines'.
    pub ram_changed: Vec<RamRun>,
    /// Calls into RAM that are the flash library's stack routines: not
    /// holes, the hardware library's calls.
    pub flash: Vec<FlashCall>,
    /// RAM code the run executed that no overlay discovered.
    pub ram_undiscovered: Vec<RamRun>,
    /// ROM code the run executed that discovery didn't find, as runs
    /// (`rom_src` is `None`).
    pub rom_undiscovered: Vec<RamRun>,
    pub cpu: Vec<CpuHole>,
    /// `swi`s the library doesn't implement: (address, number).
    pub bios: Vec<(u32, u32)>,
    /// Calls through a register to something not a known function start:
    /// (target, site).
    pub indirect_non_entry: Vec<(u32, u32)>,
    /// By kind (the hole's text with addresses taken out).
    pub summary: BTreeMap<String, SummaryKind>,
    /// The run's busy-wait loops that call functions ([`Run::busy_loops`]):
    /// not counted as hard until the owner decides how a native program
    /// lets time pass in them.
    pub busy_loops: Vec<(u32, u32)>,
    /// CPU-state instructions left out because only the pointer scan
    /// found their code and the run never ran it.
    pub scanned_skipped: usize,
    /// BIOS calls the lifter doesn't print left out the same way: the
    /// lifter lifts that code, so its BIOS holes are [`Report::bios`]
    /// and these.
    pub scanned_bios: usize,
    /// Bytes of ROM code executed that discovery didn't find.
    pub rom_undiscovered_bytes: u32,
    /// Bytes of RAM code executed that no overlay discovered.
    pub ram_undiscovered_bytes: u32,
    /// Bytes of code executed, ROM and RAM.
    pub reached_bytes: u32,
    pub rom_reached_bytes: u32,
}

impl Report {
    pub fn hard_unresolved(&self) -> usize {
        self.unresolved.len() + self.overlay_unresolved.len()
    }

    pub fn hard_ret(&self) -> usize {
        self.ret
            .iter()
            .filter(|r| !r.resolved && !r.as_jump)
            .count()
    }

    pub fn hard_cpu(&self) -> usize {
        self.cpu
            .iter()
            .filter(|c| c.place == CpuWhere::Other)
            .count()
    }

    /// The functions with a hard summary hole, each once whatever kinds
    /// it has.
    pub fn hard_summary(&self) -> usize {
        self.summary
            .values()
            .filter(|k| k.hard)
            .flat_map(|k| &k.funcs)
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Every hard hole, counted once per site of each kind (a function,
    /// for summaries). One place can be two kinds: the flash library's
    /// stack routines are RAM code that changed and, each entry, a call to
    /// a non-entry.
    pub fn hard(&self) -> usize {
        self.hard_unresolved()
            + self.hard_ret()
            + self.ram_changed.len()
            + self.hard_cpu()
            + self.bios.len()
            + self.indirect_non_entry.len()
            + self.hard_summary()
    }

    /// The flash library's routines the run called on the stack, each
    /// once however many places it was copied to.
    pub fn flash_routines(&self) -> usize {
        self.flash
            .iter()
            .map(|f| f.routine)
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn undiscovered_bytes(&self) -> u32 {
        self.rom_undiscovered_bytes + self.ram_undiscovered_bytes
    }

    /// The ranking line: hard holes, then bytes reached but not
    /// discovered.
    pub fn line(&self, game: &str) -> String {
        format!(
            "{game}: hard {} (jumps {}+{}, return arithmetic {}, RAM code changed {}, \
             CPU state {}, BIOS {}, calls to non-entries {}, summaries {}); \
             not discovered {} bytes ({} ROM, {} RAM) of {} reached; \
             busy loops calling functions {}; flash library stand-ins {}",
            self.hard(),
            self.unresolved.len(),
            self.overlay_unresolved.len(),
            self.hard_ret(),
            self.ram_changed.len(),
            self.hard_cpu(),
            self.bios.len(),
            self.indirect_non_entry.len(),
            self.hard_summary(),
            self.undiscovered_bytes(),
            self.rom_undiscovered_bytes,
            self.ram_undiscovered_bytes,
            self.reached_bytes,
            self.busy_loops.len(),
            self.flash_routines(),
        )
    }
}

fn is_ram(a: u32) -> bool {
    (0x0200_0000..0x0400_0000).contains(&a)
}

fn rom_read(rom: &[u8], a: u32, thumb: bool) -> Option<u32> {
    let o = a.checked_sub(ROM_BASE)? as usize;
    if thumb {
        let b = rom.get(o..o + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]) as u32)
    } else {
        let b = rom.get(o..o + 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

fn decode(raw: u32, a: u32, thumb: bool) -> Instr {
    if thumb {
        decode_thumb(raw as u16, a)
    } else {
        decode_arm(raw, a)
    }
}

/// Whether `i` writes lr (as a destination; a Thumb `bl`'s halves both do).
fn writes_lr(i: &Instr) -> bool {
    use Instr::*;
    match *i {
        Alu { op, rd, .. } => {
            !matches!(op, AluOp::Tst | AluOp::Teq | AluOp::Cmp | AluOp::Cmn) && rd == Reg::Lr
        }
        Mul { rd, .. } => rd == Reg::Lr,
        MulLong { rd_lo, rd_hi, .. } => rd_lo == Reg::Lr || rd_hi == Reg::Lr,
        Load { rd, addr, .. } => rd == Reg::Lr || addr.rn == Reg::Lr,
        Block {
            load,
            regs,
            rn,
            writeback,
            ..
        } => (load && regs & 1 << 14 != 0) || (writeback && rn == Reg::Lr),
        Swap { rd, .. } => rd == Reg::Lr,
        Mrs { rd, .. } => rd == Reg::Lr,
        Branch { link, .. } => link,
        ThumbBlHi { .. } | ThumbBlLo { .. } => true,
        _ => false,
    }
}

/// Arithmetic on lr into lr (`add lr, lr, r1`, `sub lr, lr, #24`), or a
/// load through lr: what a function that reads its return address does
/// before it writes lr.
fn lr_arith(i: &Instr) -> bool {
    match *i {
        Instr::Alu {
            op,
            rd: Reg::Lr,
            rn,
            op2,
            ..
        } => {
            let uses = rn == Reg::Lr
                || matches!(
                    op2,
                    Operand2::Reg { rm: Reg::Lr, .. } | Operand2::RegShiftReg { rm: Reg::Lr, .. }
                );
            uses && !matches!(
                op,
                AluOp::Mov | AluOp::Mvn | AluOp::Tst | AluOp::Teq | AluOp::Cmp | AluOp::Cmn
            )
        }
        Instr::Load { addr, .. } => addr.rn == Reg::Lr,
        _ => false,
    }
}

fn cpu_kind(i: &Instr) -> Option<CpuKind> {
    match *i {
        Instr::Msr { spsr: false, .. } => Some(CpuKind::Msr),
        Instr::Msr { spsr: true, .. } | Instr::Mrs { .. } => Some(CpuKind::Mrs),
        Instr::Block { psr: true, .. } => Some(CpuKind::UserBank),
        Instr::Alu {
            s: true,
            rd: Reg::Pc,
            op,
            ..
        } if !matches!(op, AluOp::Tst | AluOp::Teq | AluOp::Cmp | AluOp::Cmn) => {
            Some(CpuKind::ExceptionReturn)
        }
        _ => None,
    }
}

/// The text of a summary hole with its addresses and detail taken out,
/// for grouping: `0x08001234: returns with r4-r11 changed: r4 = unknown`
/// is `returns with r4-r11 changed`. The callee-saves proof's holes
/// (`callee-saves: …`) group with the summary's own of the same kind.
pub fn summary_kind(hole: &str) -> String {
    let hole = hole.strip_prefix("callee-saves: ").unwrap_or(hole);
    let words: Vec<&str> = hole
        .split_whitespace()
        .filter(|w| !w.starts_with("0x"))
        .collect();
    let kind = words.join(" ");
    let kind = kind.split_once(": ").map_or(kind.as_str(), |(k, _)| k);
    kind.trim_end_matches(',').to_string()
}

/// The summary hole kinds that are hard (`codegen::saves`): the lift keeps
/// each caller's r4-r11 and sp in its own locals and passes only r0-r3,
/// so it is wrong where a function returns with sp moved (by a run-time
/// amount, as a variable-length array does), where a caller reads one of
/// r4-r11 that a callee changed, where a function entered by a call reads
/// one of r4-r11 as it came and some of its bits reach a use (a register
/// parameter: an address, a condition, a value stored, passed on or
/// returned), or where a function that changes them may be called
/// through a pointer. A return with r4-r11 changed (hand-written code's
/// scratch registers) is not itself hard: the reads are. Nor is a read
/// whose every bit is masked away (`masks away r4-r11's entry bits`), the
/// caller's frame pointer read through a frame record, or a read in a
/// function only jumped into, which takes every register. The other kinds are counted under other
/// holes (an unknown jump, a mode change, RAM code) or aren't holes for a
/// native build (an indirect call is an `FnRef`).
pub const SUMMARY_HARD: [&str; 4] = [
    "returns with sp changed",
    "reads r4-r11 a callee changed",
    "reads r4-r11 at entry",
    "changes r4-r11 and may be called through a pointer",
];

/// Whether a summary hole of this kind ([`summary_kind`]) is hard
/// ([`SUMMARY_HARD`]).
pub fn summary_hard(kind: &str) -> bool {
    SUMMARY_HARD.contains(&kind)
}

/// One instruction known from any source.
#[derive(Clone, Copy)]
struct Known {
    thumb: bool,
    raw: u32,
}

/// The flash library's stack routines the run called: a RAM target is one
/// only if every version of its bytes the run saw called was recognised.
pub fn flash_calls(run: &Run) -> Vec<FlashCall> {
    let mut flash: BTreeSet<FlashCall> = BTreeSet::new();
    for (&target, versions) in &run.ram_calls {
        let sigs: Option<Vec<_>> = versions
            .iter()
            .map(|b| crate::flash::recognise(b))
            .collect();
        for s in sigs.unwrap_or_default() {
            flash.insert(FlashCall {
                target,
                routine: s.routine,
                len: s.len,
            });
        }
    }
    flash.into_iter().collect()
}

/// Whether `a` is in one of `flash`'s routines.
fn in_flash(flash: &[FlashCall], a: u32) -> bool {
    flash.iter().any(|f| {
        let start = f.target & !1;
        a >= start && a < start + f.len
    })
}

/// Sort what discovery, the overlays, the run and the summaries say into
/// holes. `summaries` is each function entry's summary holes, if measured.
pub fn classify(
    rom: &[u8],
    disc: &Discovery,
    overlays: &[Overlay],
    run: &Run,
    summaries: Option<&BTreeMap<u32, Vec<String>>>,
) -> Report {
    let mut r = Report::default();
    let reached = |a: u32| run.code.contains_key(&(a & !1));

    let flash = flash_calls(run);
    let flash_targets: BTreeSet<u32> = flash.iter().map(|f| f.target).collect();
    let in_flash = |a: u32| in_flash(&flash, a);

    // Indirect jumps.
    let unres = |rec: &crate::jumptable::Recovery| -> Vec<Unresolved> {
        rec.jumps
            .iter()
            .filter_map(|(&at, j)| match j {
                JumpRes::Resolved(_) => None,
                JumpRes::Unresolved => Some(Unresolved {
                    at,
                    rejected: false,
                    reached: reached(at),
                }),
                JumpRes::Rejected(_) => Some(Unresolved {
                    at,
                    rejected: true,
                    reached: reached(at),
                }),
            })
            .collect()
    };
    r.unresolved = unres(&disc.rec);
    for o in overlays {
        r.overlay_unresolved.extend(unres(&o.disc.rec));
        // A ROM source and its RAM copy count once, at the copy: the
        // copy's analysis of the jump is the one that runs (unless the
        // run ran the ROM original too).
        r.unresolved.retain(|u| {
            u.reached
                || !(o.src..o.src + o.len).contains(&u.at)
                || !o.disc.rec.jumps.contains_key(&(o.dst + (u.at - o.src)))
        });
    }

    // Every instruction known: discovered (ROM, overlays) and run.
    let mut known: BTreeMap<u32, Known> = BTreeMap::new();
    let mut covered: BTreeSet<u32> = BTreeSet::new(); // discovered halfwords
    for (&a, c) in &disc.code {
        if let Some(raw) = rom_read(rom, a, c.thumb) {
            known.insert(
                a,
                Known {
                    thumb: c.thumb,
                    raw,
                },
            );
        }
        for h in (a..a + c.size).step_by(2) {
            covered.insert(h);
        }
    }
    for o in overlays {
        for (&a, c) in &o.disc.code {
            if let Some(raw) = rom_read(rom, o.rom_addr(a), c.thumb) {
                known.insert(
                    a,
                    Known {
                        thumb: c.thumb,
                        raw,
                    },
                );
            }
            for h in (a..a + c.size).step_by(2) {
                covered.insert(h);
            }
        }
    }
    let mut rom_missed: Vec<(u32, &Seen)> = Vec::new();
    for (&a, s) in &run.code {
        let size = if s.thumb { 2 } else { 4 };
        let k = known.entry(a).or_insert(Known {
            thumb: s.thumb,
            raw: s.raw,
        });
        if k.thumb != s.thumb {
            // Run as the other instruction set than discovery says: the
            // run's view, since it ran.
            *k = Known {
                thumb: s.thumb,
                raw: s.raw,
            };
        }
        r.reached_bytes += size;
        if !is_ram(a) {
            r.rom_reached_bytes += size;
        }
        if in_flash(a) {
            continue;
        }
        let new = (a..a + size)
            .step_by(2)
            .filter(|h| !covered.contains(h))
            .count() as u32
            * 2;
        if is_ram(a) {
            r.ram_undiscovered_bytes += new;
        } else {
            r.rom_undiscovered_bytes += new;
            if new > 0 {
                rom_missed.push((a, s));
            }
        }
    }
    r.rom_undiscovered = runs(rom_missed.into_iter(), |_| None);

    // Function starts, for grouping: discovered, overlays', run's call
    // targets and IRQ handlers.
    let mut starts: BTreeSet<u32> = disc.funcs.keys().map(|a| a & !1).collect();
    for o in overlays {
        starts.extend(o.disc.funcs.keys().map(|a| a & !1));
    }
    starts.extend(run.call_targets.iter().map(|a| a & !1));
    starts.extend(run.irq_handlers.iter().map(|a| a & !1));
    let func_of = |a: u32| starts.range(..=a).next_back().copied();
    let mut wrapper_funcs = BTreeSet::new();
    let mut after_boot_funcs = BTreeSet::new();
    let mut run_funcs = BTreeSet::new();
    for (&a, s) in &run.code {
        if let Some(f) = func_of(a) {
            run_funcs.insert(f);
            if s.wrapper {
                wrapper_funcs.insert(f);
            }
            if s.after_boot {
                after_boot_funcs.insert(f);
            }
        }
    }

    // RAM code: changed, and not discovered.
    let ram: Vec<(u32, &Seen)> = run
        .code
        .iter()
        .filter(|(a, _)| is_ram(**a) && !in_flash(**a))
        .map(|(a, s)| (*a, s))
        .collect();
    r.ram_changed = runs(
        ram.iter().filter(|(_, s)| s.changed).map(|(a, s)| (*a, *s)),
        |_| None,
    );
    let undiscovered = ram
        .iter()
        .filter(|(a, _)| !covered.contains(a))
        .map(|(a, s)| (*a, *s));
    let rom_src = |run_bytes: &[u8]| -> Option<u32> {
        if run_bytes.len() < 8 {
            return None;
        }
        let n = run_bytes.len().min(64);
        rom.windows(n)
            .position(|w| w == &run_bytes[..n])
            .map(|o| ROM_BASE + o as u32)
    };
    r.ram_undiscovered = runs(undiscovered, |bytes| rom_src(bytes));

    // RAM code copied from the ROM by the game's own loop runs at its RAM
    // address: its ROM source's instructions take the RAM copy's place
    // (the IRQ wrapper, pret's `IntrMain`).
    let mut alias: BTreeMap<u32, u32> = BTreeMap::new();
    for c in &r.ram_undiscovered {
        if let Some(src) = c.rom_src {
            for a in run.code.range(c.start..c.end).map(|(a, _)| *a) {
                alias.insert(src + (a - c.start), a);
            }
        }
    }
    // Code only the pointer scan found and nothing ran: likely data, so
    // what decodes there isn't counted.
    let scanned_only = |a: u32| {
        !run.code.contains_key(&a)
            && !alias.contains_key(&a)
            && disc
                .code
                .get(&a)
                .is_some_and(|c| c.evidence == crate::discovery::Evidence::Scanned)
    };

    // CPU state, BIOS calls and return-address arithmetic, by instruction.
    let mut lr_written: BTreeSet<u32> = BTreeSet::new(); // functions past their first lr write
    let mut static_ret: BTreeMap<u32, u32> = BTreeMap::new(); // function -> first arith insn
    for (&a, k) in &known {
        let i = decode(k.raw, a, k.thumb);
        let ra = alias.get(&a).copied().unwrap_or(a);
        let seen = run.code.get(&ra);
        let at_place = || {
            place(
                seen,
                func_of(ra),
                &wrapper_funcs,
                &after_boot_funcs,
                &run_funcs,
            )
        };
        if ra != a && known.contains_key(&ra) {
            // The ROM source of a RAM copy the run ran: classified at the
            // copy, once.
            continue;
        }
        let skip = scanned_only(a);
        if let Some(mut kind) = cpu_kind(&i) {
            if kind == CpuKind::Msr && seen.is_some_and(|s| !s.mode_change && s.after_boot) {
                kind = CpuKind::MsrMask;
            }
            if skip {
                r.scanned_skipped += 1;
            } else {
                r.cpu.push(CpuHole {
                    at: a,
                    kind,
                    place: at_place(),
                    reached: seen.is_some(),
                });
            }
        }
        if seen.is_some_and(|s| s.stack_switch) && ra == a {
            r.cpu.push(CpuHole {
                at: a,
                kind: CpuKind::StackSwitch,
                place: at_place(),
                reached: true,
            });
        }
        if let Instr::Swi { comment, .. } = i {
            let n = if k.thumb {
                comment & 0xFF
            } else {
                comment >> 16 & 0xFF
            };
            if bios_implemented(n) {
            } else if skip {
                r.scanned_bios += 1;
            } else {
                r.bios.push((a, n));
            }
        }
        // Static return arithmetic: in address order within a function,
        // arithmetic on lr before anything writes it.
        if let Some(f) = func_of(a)
            && !lr_written.contains(&f)
        {
            if lr_arith(&i) {
                static_ret.entry(f).or_insert(a);
            }
            if writes_lr(&i) {
                lr_written.insert(f);
            }
        }
    }

    // Who calls each function by `bl` (the call sites a resolved jump at
    // the `bl` covers).
    let mut bl_sites: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (&a, k) in &known {
        let t = if k.thumb {
            let lo = known
                .get(&(a + 2))
                .map(|n| n.raw)
                .or_else(|| rom_read(rom, a + 2, true));
            lo.and_then(|lo| armv4t::decode::pair_thumb_bl(k.raw as u16, lo as u16, a))
        } else {
            match decode(k.raw, a, false) {
                Instr::Branch {
                    link: true, target, ..
                } => Some(target),
                _ => None,
            }
        };
        if let Some(t) = t {
            bl_sites.entry(t & !1).or_default().push(a);
        }
    }
    let jump_failed = |site: u32| {
        disc.rec
            .jumps
            .get(&site)
            .or_else(|| overlays.iter().find_map(|o| o.disc.rec.jumps.get(&site)))
            .is_some_and(|j| !matches!(j, JumpRes::Resolved(_)))
    };
    let jump_resolved = |site: u32| {
        disc.rec
            .jumps
            .get(&site)
            .or_else(|| overlays.iter().find_map(|o| o.disc.rec.jumps.get(&site)))
            .is_some_and(|j| matches!(j, JumpRes::Resolved(_)))
    };
    // The run's, one hole per function: its first return seen, and every
    // call site.
    let mut dynamic: BTreeMap<u32, RetHole> = BTreeMap::new();
    for ra in run.ret_arith.values() {
        // A jump to a function start is a tail jump (a long-call veneer),
        // and one inside the callee's own function a jump within it (a
        // table whose cases happen to lie near the return site): neither
        // returns.
        let to = ra.to & !1;
        if starts.contains(&to) || func_of(to) == func_of(ra.callee & !1) {
            continue;
        }
        let h = dynamic.entry(ra.callee & !1).or_insert(RetHole {
            func: ra.callee,
            at: ra.at,
            delta: Some(ra.delta),
            sites: Vec::new(),
            resolved: true,
            as_jump: true,
            reached: true,
        });
        if !h.sites.contains(&ra.site) {
            h.sites.push(ra.site);
        }
    }
    let dyn_funcs: BTreeSet<u32> = dynamic.keys().copied().collect();
    for mut h in dynamic.into_values() {
        h.resolved = h.sites.iter().all(|&s| jump_resolved(s));
        h.as_jump = !h.resolved && h.sites.iter().all(|&s| jump_resolved(s) || jump_failed(s));
        r.ret.push(h);
    }
    for (&f, &at) in &static_ret {
        if dyn_funcs.contains(&f) {
            continue;
        }
        let sites = bl_sites.get(&f).cloned().unwrap_or_default();
        if sites.is_empty() {
            // Nothing calls it by `bl`: lr there isn't a return address
            // we know of (hand-written ARM using lr as scratch from its
            // entry, entered by a jump).
            continue;
        }
        let thumb = known.get(&f).is_some_and(|k| k.thumb);
        r.ret.push(RetHole {
            func: f | thumb as u32,
            at,
            delta: None,
            resolved: sites.iter().all(|&s| jump_resolved(s)),
            as_jump: !sites.iter().all(|&s| jump_resolved(s))
                && sites.iter().all(|&s| jump_resolved(s) || jump_failed(s)),
            sites,
            reached: reached(at),
        });
    }

    // Calls through a register to an address that isn't a function start.
    let entries: BTreeSet<u32> = disc
        .funcs
        .keys()
        .chain(overlays.iter().flat_map(|o| o.disc.funcs.keys()))
        .chain(&disc.ram_entries)
        .map(|a| a & !1)
        .collect();
    for (&t, &site) in &run.indirect_calls {
        if !entries.contains(&(t & !1)) && !flash_targets.contains(&t) {
            r.indirect_non_entry.push((t, site));
        }
    }

    r.busy_loops = run.busy_loops.iter().map(|(&a, &n)| (a, n)).collect();
    r.flash = flash;

    if let Some(sums) = summaries {
        for (&f, holes) in sums {
            for h in holes {
                let kind = summary_kind(h);
                let e = r.summary.entry(kind.clone()).or_default();
                e.hard = summary_hard(&kind);
                e.funcs.insert(f);
            }
        }
    }
    r
}

fn place(
    seen: Option<&Seen>,
    func: Option<u32>,
    wrapper_funcs: &BTreeSet<u32>,
    after_boot_funcs: &BTreeSet<u32>,
    run_funcs: &BTreeSet<u32>,
) -> CpuWhere {
    if seen.is_some_and(|s| s.wrapper) || func.is_some_and(|f| wrapper_funcs.contains(&f)) {
        return CpuWhere::Wrapper;
    }
    if seen.is_some_and(|s| !s.after_boot)
        || func.is_some_and(|f| run_funcs.contains(&f) && !after_boot_funcs.contains(&f))
    {
        return CpuWhere::Boot;
    }
    CpuWhere::Other
}

/// Group addresses (sorted) into runs with gaps of at most 16 bytes, each
/// with `src` of its first-run bytes.
fn runs<'a>(
    code: impl Iterator<Item = (u32, &'a Seen)>,
    src: impl Fn(&[u8]) -> Option<u32>,
) -> Vec<RamRun> {
    // (start, end, the first contiguous stretch's bytes, still contiguous)
    let mut out: Vec<(u32, u32, Vec<u8>, bool)> = Vec::new();
    for (a, s) in code {
        let size = if s.thumb { 2 } else { 4 };
        let bytes = &s.raw.to_le_bytes()[..size as usize];
        match out.last_mut() {
            Some((_, end, b, whole)) if a >= *end && a - *end <= 16 => {
                // Past a gap, the bytes in it weren't run: the source is
                // looked up from the first stretch.
                *whole &= a == *end;
                if *whole {
                    b.extend_from_slice(bytes);
                }
                *end = a + size;
            }
            _ => out.push((a, a + size, bytes.to_vec(), true)),
        }
    }
    out.into_iter()
        .map(|(start, end, b, _)| RamRun {
            start,
            end,
            rom_src: src(&b),
        })
        .collect()
}

mod copies;
pub use copies::{RamCopy, ram_copies};

#[cfg(test)]
mod tests;
