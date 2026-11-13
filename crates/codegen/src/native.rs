//! The stage-A lifter (`docs/native-port-design.md`, sections 1-6, stage
//! A; unit `native/lift-stage-a`): a whole program printed from the IR as
//! plain Rust over byte RAM and the hardware library, with no `Machine`,
//! no CPU, no cycles.
//!
//! **Map of this file**, in order (search for the names; how a concern
//! owned elsewhere works is that module's doc):
//!
//! 1. The prelude: [`PRELUDE`] (`native/prelude.rs`, hand-written, in
//!    every lifted crate) and [`prelude`], which swaps in
//!    `native/prelude_packed.rs` at `--layout packed`; `BIOS`, the
//!    `swi`s it has.
//! 2. The holes file and the lift's output: [`Holes`] (stand-ins, jump
//!    targets, `[[copy]]`s as [`SeenCopy`], names, the planted controls'
//!    and old behaviours' flags), [`Hole`], [`Lifted`].
//! 3. [`lift`], the driver, in its order: discovery's gaps decoded
//!    (`missing`, `force_code`); the overlays and the holes file's copies;
//!    one `Code` per context (`code`: functions, summaries, frames); RAM
//!    globals carved (`const_access`, `computed_ram`, then
//!    [`globals::Globals::carve`]); shapes (`shapes_of`); the
//!    function-address flow (`fn_ins`, `fn_flow`, `FnImage`, `literal`)
//!    and the heap's ([`heap::follow`]); each
//!    function printed (a stand-in's text, else `FnPrinter::print`);
//!    `FNS`, `ENTRY`, `Ram` and the split into files
//!    ([`modules::split`]); the report (hard holes, `Soft`, frames) and
//!    the [`metrics`].
//! 4. `FnPrinter`, one function's Rust: `print` (signature, locals, the
//!    block loop); `block` (a block's IR as statements: memory accesses,
//!    typed fields via `typed_as`, `g.sp = r13` before I/O stores; `count`
//!    tallies each access for the metrics; `used` drops what nothing
//!    printed needs); stack locals (`local`, `declare_locals`, `flush`);
//!    control flow (`goto`, `fall_into`, `call`, `call_then`, `tail`,
//!    `ret`, and `exit`: branches, calls by value, `swi`, idle and spin
//!    loops).
//! 5. Free helpers: `dyn_targets` (a jump by value's known targets);
//!    signatures from a [`Shape`] (`plain`, `ret_ty`, `ret_expr`, `tuple`,
//!    `lhs`, `args`); locals' text (`local_addr`, `local_read`,
//!    `local_write`); IR to Rust (`expr`, `konst`, `pure`,
//!    `dead_addresses`, `v`, `flag`, `shift`). Then the tests.
//!
//! Elsewhere: what each function's parameters and results are,
//! [`shape`]; frames, escaping slots and which become locals, [`frame`];
//! function addresses and callee sets, [`fnref`]; RAM's globals, `pin`
//! and `ix`, both layouts, [`globals`]; heap pointers as `Ref<T>`, the
//! prelude's `Game::at`, [`heap`]; metric names and bars,
//! [`metrics`]; the crate's files, [`modules`]; `lift-check.toml`'s
//! programs, [`lift_check`]. A function's blocks and IR come from
//! [`crate::func`], its summary from [`crate::saves`].
//!
//! What each function becomes:
//!
//! - `fn f_<key>(g: &mut Game, sp: u32, a0: u32, a1: u32) -> Result<u32, Exit>`:
//!   its parameters and results from the summaries
//!   ([`crate::saves::summarize`]; [`shape`], unit `native/lift-params`),
//!   the stack pointer as a parameter, `Exit` passed up from the waits
//!   with `?`; `a_<key>`, its adapter, takes and gives all of r0-r3, for
//!   `call_fn`. A function whose summary has a hole (or rests on a
//!   callee's that stopped) keeps stage A's four-register form,
//!   `f_<key>(g, sp, a: [u32; 4]) -> Result<[u32; 4], Exit>`, with the
//!   summary printed above it.
//! - Registers and NZCV are Rust locals; each block's IR values are `let`s.
//!   r4-r11 outlive a call because callees keep them, or change only
//!   what no caller reads (the summaries' `keeps` and `changes`; a
//!   function that breaks that, `analysis::holes::SUMMARY_HARD`, is a
//!   hard hole).
//! - Two entries: `x_<key>(g, Regs)` takes every register and flag, and
//!   `f_<key>` above is a call into it with r4-r12 and the flags zero and
//!   `lr` [`RET`]. A fall or jump into another function's entry (or a
//!   call's return into one) runs on in its `x_` with this one's registers
//!   and flags, as the hardware does (unit `native/lift-fallthrough`).
//! - Blocks are the arms of a `loop { match b }` over local labels
//!   (0 the entry); calls are Rust calls; a return is a jump to the value
//!   `lr` held on entry, [`RET`] (prelude), told by value since compiled
//!   code moves it through the stack.
//! - Memory is `ld8/16/32` and `st8/16/32` over `Game::ram` (EWRAM then
//!   IWRAM, as bytes), the library's I/O registers (`Hw::read16` and
//!   `write16`), its palette, VRAM and OAM, and the ROM's bytes. A RAM
//!   access at a constant address is a typed global's field instead
//!   (`g.ram.d_02000100`, [`globals`]): `Game::ram` is a `Ram` struct of
//!   them at the guest's offsets, which derefs to the bytes (`--layout
//!   guest`), or in the lifter's order with the rest of RAM as bytes,
//!   each byte found by its offset (`--layout packed`, the play layout).
//! - Stack slots whose address doesn't escape ([`frame`]) are Rust locals,
//!   `l<bytes below the entry sp>`: a `u32` for an aligned word only ever
//!   accessed whole, else bytes (unit `native/frame-locals`); escaping
//!   slots stay IWRAM bytes, and sp is still passed. A value nothing
//!   printed uses (the address of an access to a local) isn't printed.
//! - BIOS calls are the library's ([`BIOS`] lists them): the waits
//!   (`IntrWait`, `VBlankIntrWait` as `hw::halt` until the game's handler
//!   sets the flag; `Halt`, `Stop`), `CpuSet`, `CpuFastSet` and LZ77 with
//!   their time (`hw::spend`), RL, `Div`, `Sqrt`, `ArcTan2`, `SoundBias`,
//!   `MidiKey2Freq`, `MultiBoot` (unit `native/lift-bios-calls`),
//!   `BgAffineSet`, `ObjAffineSet` (unit `native/lift-bios-affine`).
//! - An idle loop's back edge calls `hw::idle`, a spin loop's `hw::spin`
//!   (`analysis::idle`, `runtime::spins`, as the play build's
//!   `--timing native` marks them).
//! - Function pointers are their keys (ROM or IWRAM address, Thumb bit):
//!   `call_fn` (prelude) looks them up in `FNS`; a RAM key that isn't
//!   there but holds one of the flash save library's stack routines is
//!   that routine's `hw::flash` call (unit `native/lift-flash-routines`).
//!   Stage A has no dense IDs
//!   (`--fn-ids rom` only): a literal lift loads pointers from ROM bytes
//!   and RAM, so it can't renumber them. [`fnref`] follows every function
//!   address to its uses and reports the ROM words dense IDs could rewrite
//!   and each indirect call's callees (unit `native/fnref-flow`). An
//!   indirect call whose callee set is complete is summarised as calling
//!   exactly those (the summaries run again with the sets,
//!   [`crate::saves::summarize_sets`]), and passes `call_fn` only what
//!   they read, taking back what they may change ([`call_fn_args`]; unit
//!   `native/indirect-call-args`); an incomplete one passes all four.
//!
//! **Holes.** A function the lift can't print is a hard hole, named, and
//! the lift fails (and the crate, which gets a `compile_error!` for it),
//! unless the game's holes file gives a stand-in or, for an unresolved
//! jump, its targets ([`Holes`]):
//!
//! - CPU state: `mrs`, `msr`, user-bank transfers, exception returns,
//!   stack switches ([`Frame::switches`]: a task switcher's, which a stand-in
//!   runs on the prelude's host coroutines, `task_resume`);
//! - an indirect jump Recovery didn't resolve;
//! - a BIOS call the prelude doesn't have;
//! - an undefined instruction, a jump to no code (but a compiler's trap,
//!   `udf #255` and the like, which the lift prints as a `panic!`: modern
//!   GCC ends a path it proved dereferences null with one);
//! - a function that returns without keeping r4-r11 and sp.
//!
//! Soft holes (counted in the report): every RAM access but the globals'
//! and the stack locals' is bytes, every pointer a guest address, every
//! ROM read at run time.

use crate::func::{Ctx, ExitInfo, Func, Idle, Plan, build_func, exits, key};
use crate::saves::{Assume, Sets, Summaries, Summary, summarize, summarize_sets};
use analysis::discovery::{self, Discovery};
use analysis::ir::{BinOp, BitOp, Exit, Inst, Op, ValueId};
use analysis::vsa::func::JumpRes;
use armv4t::sem::{Flag, ShiftType, Width};
use frame::{Frame, Kind, Local};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub mod fnref;
pub mod frame;
pub mod globals;
pub mod heap;
pub mod lift_check;
pub mod metrics;
pub mod modules;
pub mod shape;

use globals::{Globals, Layout, Obj, Place};
use metrics::{Metrics, Where};
use shape::{Shape, Shapes};

/// The prelude every lifted crate starts with, at the guest layout.
pub const PRELUDE: &str = include_str!("native/prelude.rs");

/// What the play layout (`--layout packed`) puts in place of the
/// prelude's guest-layout `Ram` code ([`GUEST_RAM`] to [`GUEST_RAM_END`]).
const PACKED_RAM: &str = include_str!("native/prelude_packed.rs");
const GUEST_RAM: &str = "// `Ram` (printed after the prelude) is EWRAM and IWRAM byte for byte:";
const GUEST_RAM_END: &str = "/// The handler the game stored at";

/// The prelude for `layout`.
pub fn prelude(layout: Layout) -> String {
    match layout {
        Layout::Guest => PRELUDE.to_string(),
        Layout::Packed => {
            let (a, b) = (
                PRELUDE
                    .find(GUEST_RAM)
                    .expect("the guest layout's Ram code"),
                PRELUDE.find(GUEST_RAM_END).expect("irq_entry"),
            );
            format!("{}{PACKED_RAM}{}", &PRELUDE[..a], &PRELUDE[b..])
        }
    }
}

/// The BIOS calls the prelude has, the list `gbaa-holes` counts the rest
/// by.
use analysis::holes::NATIVE_BIOS as BIOS;

/// What the game's holes file (`holes/<game>.toml`) gives: hand-written
/// stand-ins for functions, and targets for unresolved jumps.
#[derive(Clone, Debug, Default)]
pub struct Holes {
    /// By function key: the Rust that replaces it (a `pub fn f_<key>` of
    /// the lifted signature), and why.
    pub standins: BTreeMap<u32, (String, String)>,
    /// The stand-ins that read their caller's return address (`lr =
    /// true`): each call sets `Game::lr` to it (the `bl`'s `r14`) first,
    /// as a task switcher's yield stores it for the task's resume.
    pub lr: BTreeSet<u32>,
    /// By the jump's instruction address: its targets (keys), and why.
    pub jumps: BTreeMap<u32, (Vec<u32>, String)>,
    /// Code the game copies to RAM itself that discovery's static copies
    /// don't have (by DMA, by its own loop, through a pointer):
    /// `[[copy]] src = 0x…, dst = 0x…, len = 0x…, roots = [0x…], why = "…"`
    /// in the holes file, as `gbaa-holes` prints its run's copies
    /// (`analysis::holes::ram_copies`, native/lift-ram-copies). Each is
    /// discovered at `dst` from `roots` (keys) and lifted as a static copy
    /// is, its functions at their RAM keys. One that overlaps a static copy
    /// or an earlier one, leaves the ROM or its RAM region, or has a root
    /// outside it is a hard hole.
    pub copies_seen: Vec<SeenCopy>,
    /// A check's planted wrong store (`gbaa-lift --plant AT`): the store
    /// of the instruction at this address stores its value plus one, which
    /// the native check must catch.
    pub plant: Option<u32>,
    /// Stage A's old fall into another function (`gbaa-lift
    /// --fall-as-tail-call`): a tail call through its plain entry, which
    /// starts it with r4-r12 and the flags zeroed. The fallthrough check's
    /// planted control (`native/lift-fallthrough`): the native check must
    /// catch it on `agb/native-fallthrough`.
    pub fall_as_tail_call: bool,
    /// Stage A's old IRQs (`gbaa-lift --irq-at-waits`): `Game::cpsr` as
    /// bookkeeping only, so an IRQ due at a wait inside a critical section
    /// runs there, where the hardware holds it until the I bit clears. The
    /// IRQ-hold check's planted control (`native/lift-irq-hold`): the
    /// native check must catch it on `agb/native-irq-hold`.
    pub irq_at_waits: bool,
    /// The old `IME` (`gbaa-lift --ime-at-waits`): a store that sets `IME`
    /// again with an IRQ pending leaves it to the next wait, where the
    /// hardware takes it at once. The IME-release check's planted control
    /// (`native/ime-release`): the native check must catch it on
    /// `agb/native-ime-release`.
    pub ime_at_waits: bool,
    /// The stack-locals check's planted control (`gbaa-lift --plant-slot
    /// AT`, `native/frame-locals`): the store of the instruction at this
    /// address, which must be to a slot printed as a Rust local, stores
    /// its value plus one. The native check skips the guest stack, so it
    /// must catch this where the slot's value reaches a global.
    pub plant_slot: Option<u32>,
    /// Shadow every stack local in the frame's bytes (`gbaa-lift
    /// --shadow-locals`): a local starts as its bytes, every store to it
    /// writes them too, and every load reads them and prints a line to
    /// stderr (`shadow local:`) where they differ from the local, which
    /// is where something the escape analysis didn't see wrote the frame.
    pub shadow_locals: bool,
    /// The synthetic large lift (`gbaa-lift --copies N`, [`modules`]): the
    /// functions' modules N times over under different names. 0 or 1: once.
    pub copies: usize,
    /// Data names and sizes (0: not known) by guest address, from the
    /// game's names files (`gbaa-lift --names DIR`): a RAM global that
    /// starts at one is named so, else `d_<address>`, and spans a sized
    /// datum an access reaches ([`globals`]).
    pub names: BTreeMap<u32, (String, u32)>,
    /// Where `Ram`'s globals are (`gbaa-lift --layout`): the guest's
    /// offsets, or the play layout's order ([`globals::Layout`]).
    pub layout: Layout,
    /// Why the function addresses aren't followed (`skip_fn_flow = "…"`
    /// in the holes file): [`fnref::follow`] only feeds the report and
    /// stage B's metrics, and on a game it doesn't settle in a session
    /// (Metroid: 68 s a round, rounds growing). `None`: followed.
    pub skip_fn_flow: Option<String>,
}

impl Holes {
    /// Parse a holes file: `[[standin]] entry = 0x…, why = "…", code =
    /// '''…'''` (and `lr = true` for [`Holes::lr`]), `[[jump]] at = 0x…, targets = [0x…], why = "…"`, and at
    /// the top, `skip_fn_flow = "why"`.
    pub fn parse(text: &str) -> Result<Holes, String> {
        let t: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
        let mut h = Holes::default();
        let int = |v: &toml::Value, what: &str| -> Result<u32, String> {
            v.as_integer()
                .map(|i| i as u32)
                .ok_or(format!("{what}: not an integer"))
        };
        let str_of = |e: &toml::Table, k: &str| -> Result<String, String> {
            e.get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or(format!("missing {k}"))
        };
        for (k, v) in &t {
            if k == "skip_fn_flow" {
                let why = v.as_str().ok_or("skip_fn_flow: not a string (why)")?;
                h.skip_fn_flow = Some(why.to_string());
                continue;
            }
            let list = v.as_array().ok_or(format!("{k}: not a list of tables"))?;
            for e in list {
                let e = e.as_table().ok_or(format!("{k}: not a table"))?;
                match k.as_str() {
                    "standin" => {
                        let entry = int(e.get("entry").ok_or("standin: missing entry")?, "entry")?;
                        h.standins
                            .insert(entry, (str_of(e, "code")?, str_of(e, "why")?));
                        match e.get("lr") {
                            None => {}
                            Some(v) => match v.as_bool() {
                                Some(true) => {
                                    h.lr.insert(entry);
                                }
                                Some(false) => {}
                                None => return Err("standin: lr: not a bool".into()),
                            },
                        }
                    }
                    "jump" => {
                        let at = int(e.get("at").ok_or("jump: missing at")?, "at")?;
                        let targets = e
                            .get("targets")
                            .and_then(|v| v.as_array())
                            .ok_or("jump: missing targets")?
                            .iter()
                            .map(|v| int(v, "targets"))
                            .collect::<Result<Vec<_>, _>>()?;
                        h.jumps.insert(at, (targets, str_of(e, "why")?));
                    }
                    "copy" => {
                        let field = |f: &str| -> Result<u32, String> {
                            int(e.get(f).ok_or(format!("copy: missing {f}"))?, f)
                        };
                        let roots = e
                            .get("roots")
                            .and_then(|v| v.as_array())
                            .ok_or("copy: missing roots")?
                            .iter()
                            .map(|v| int(v, "roots"))
                            .collect::<Result<Vec<_>, _>>()?;
                        h.copies_seen.push(SeenCopy {
                            src: field("src")?,
                            dst: field("dst")?,
                            len: field("len")?,
                            roots,
                            why: str_of(e, "why")?,
                        });
                    }
                    _ => return Err(format!("unknown table {k}")),
                }
            }
        }
        Ok(h)
    }
}

/// One `[[copy]]` of the holes file ([`Holes::copies_seen`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenCopy {
    pub src: u32,
    pub dst: u32,
    pub len: u32,
    pub roots: Vec<u32>,
    pub why: String,
}

/// A hard hole: the function, the instruction, and what it is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hole {
    pub func: u32,
    pub at: u32,
    pub what: String,
}

impl std::fmt::Display for Hole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "hard hole in f_{:08x} at {:#010x}: {}",
            self.func, self.at, self.what
        )
    }
}

/// A lifted program.
pub struct Lifted {
    /// The crate's sources: `(path under src/, text)`, `main.rs` first
    /// ([`modules`]).
    pub files: Vec<(String, String)>,
    /// The hard holes no stand-in covers: the crate won't build.
    pub holes: Vec<Hole>,
    /// What was lifted, and the soft holes counted: ends with
    /// [`Lifted::metrics`]' lines.
    pub report: String,
    /// The counts the bars hold ([`metrics`]).
    pub metrics: Metrics,
}

/// One code context: the ROM, or an IWRAM copy at its destination.
struct Code<'a> {
    ctx: Ctx<'a>,
    funcs: Vec<Func>,
    sums: Summaries,
    /// Each function's stack frame ([`frame`]).
    frames: BTreeMap<u32, Frame>,
}

/// Lift `rom` whole. `name` is the program's name for its front end.
pub fn lift(rom: &[u8], name: &str, holes: &Holes) -> Lifted {
    let mut disc = discovery::discover(rom.to_vec());
    // Discovery's gaps: a jump into ROM where it found no code (a scanned
    // fragment it stopped short in, a `bl`'s second half it took for a
    // literal pool). Each such target is decoded as code from there, and
    // again from what that reaches, so the lift has the code the play
    // build would hand to its interpreter. Listed in the report.
    // The holes file's targets for jumps Recovery left unresolved are
    // followed as a table's cases, and decoded the same way if discovery
    // has no code there.
    let given = || holes.jumps.iter().map(|(&a, (ts, _))| (a, ts.as_slice()));
    let mut gaps = BTreeSet::new();
    for _ in 0..64 {
        let ctx = Ctx::new(rom, &disc).with_library().with_jumps(given());
        let funcs: Vec<Func> = ctx.entries.iter().map(|&e| build_func(&ctx, e)).collect();
        let mut missing = missing(&ctx, &funcs);
        missing.extend(
            given()
                .flat_map(|(_, ts)| ts.iter().copied())
                .filter(|&k| (0x0800_0000..0x0A00_0000).contains(&(k & !1)) && !ctx.is_code(k)),
        );
        if missing.is_empty() {
            break;
        }
        gaps.extend(missing.iter().copied());
        let before = disc.code.len();
        for &k in &missing {
            force_code(rom, &mut disc, k);
        }
        if disc.code.len() == before {
            break;
        }
    }
    let disc = disc;
    let mut overlays = discovery::overlays(rom, &disc);
    // The holes file's copies, which only a run shows.
    let mut bad_copies = Vec::new();
    for c in &holes.copies_seen {
        let ok = c.len > 0
            && c.src >= 0x0800_0000
            && (c.src - 0x0800_0000) as usize + c.len as usize <= rom.len()
            && [0x0200_0000..0x0204_0000, 0x0300_0000..0x0300_8000]
                .iter()
                .any(|r| r.contains(&c.dst) && c.dst + c.len <= r.end)
            && c.roots
                .iter()
                .all(|k| (c.dst..c.dst + c.len).contains(&(k & !1)))
            && !overlays
                .iter()
                .any(|o| c.dst < o.dst + o.len && o.dst < c.dst + c.len);
        if !ok {
            bad_copies.push(Hole {
                func: c.dst,
                at: c.dst,
                what: "the holes file's copy is outside the ROM, EWRAM or IWRAM, has a root outside it, or overlaps another copy"
                    .into(),
            });
            continue;
        }
        let image = rom[(c.src - 0x0800_0000) as usize..][..c.len as usize].to_vec();
        overlays.push(discovery::Overlay {
            src: c.src,
            dst: c.dst,
            len: c.len,
            disc: discovery::discover_at(c.dst, image, &c.roots),
        });
    }
    let overlays = overlays;
    let mut codes = vec![code(
        Ctx::new(rom, &disc).with_library().with_jumps(given()),
    )];
    for o in &overlays {
        let o_rom = &rom[(o.src - 0x0800_0000) as usize..][..o.len as usize];
        codes.push(code(
            Ctx::at(o.dst, o_rom, &o.disc)
                .with_library()
                .with_jumps(given()),
        ));
    }
    let holes = &library_standins(holes, &codes);
    // Every function, by key, over all contexts.
    let mut all: BTreeSet<u32> = BTreeSet::new();
    for c in &codes {
        all.extend(c.funcs.iter().map(|f| f.entry));
    }
    // RAM's globals: what the constant-address accesses of the functions
    // printed (a stand-in's code is its own) reach.
    // And what their computed ones reach, where the value sets bound it
    // (native/ref-in-object): one address is an access like a constant
    // one; a range, an object's extent.
    let mut ram = Vec::new();
    let mut ranges = Vec::new();
    for c in &codes {
        for f in c
            .funcs
            .iter()
            .filter(|f| !holes.standins.contains_key(&f.entry))
        {
            for gb in f.blocks.values() {
                let insts = &gb.block.insts;
                let mut at = 0;
                for inst in insts {
                    if let Op::Insn(a) = inst.op {
                        at = a;
                    }
                    if let Some((a, w)) = const_access(insts, &inst.op) {
                        ram.push((a, w));
                    } else if let Some((lo, hi, w)) = computed_ram(&c.ctx, insts, &inst.op, at) {
                        if lo == hi {
                            ram.push((lo, w));
                        } else {
                            ranges.push((lo, hi + w, w));
                        }
                    }
                }
            }
        }
    }
    let globals = Globals::carve(&ram, &ranges, &holes.names);
    let flow_ins: Vec<FlowIn> = codes
        .iter()
        .enumerate()
        .map(|(i, c)| FlowIn {
            ctx: &c.ctx,
            src: if i == 0 {
                0x0800_0000
            } else {
                overlays[i - 1].src
            },
            funcs: &c.funcs,
            frames: &c.frames,
        })
        .collect();
    let (ins, pools, call_fn) = fn_ins(&flow_ins, &all);
    let fnrefs = match &holes.skip_fn_flow {
        None => {
            let (flow, words) = fn_flow(rom, &disc, &overlays, &ins, &pools, &all);
            Some((flow, words, call_fn))
        }
        Some(_) => None,
    };
    // Heap pointers as `Ref<T>` (native/ref-heap): RAM at the start is
    // the program start's copies, else 0.
    let ram_init = |a: u32| -> u32 {
        disc.copies
            .iter()
            .find(|&&(_, dst, len)| (dst..dst + len).contains(&a) && a + 4 <= dst + len)
            .and_then(|&(src, dst, _)| {
                let o = (src + (a - dst)).checked_sub(0x0800_0000)? as usize;
                rom.get(o..o + 4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            })
            .unwrap_or(0)
    };
    // A flow that didn't settle (its rounds or its token bound ran out:
    // native/whole-lift-memory) gives the heap what a skipped one does:
    // its sites and taken functions may be missing some.
    let settled = fnrefs.as_ref().filter(|(f, _, _)| !f.unsettled);
    let no_sites = BTreeMap::new();
    let taken: Option<BTreeSet<u32>> = settled.map(|(f, words, _)| {
        f.consts
            .values()
            .chain(words.values().map(|(k, _)| k))
            .copied()
            .collect()
    });
    // Whether a global ends past a guest RAM address in its region: an
    // object an unknown index makes from there may share bytes with it
    // (native/ref-heap-static-arrays).
    let global_from = |a: u32| -> bool {
        let Some(o) = globals::ram_at(a) else {
            return false;
        };
        let end = if o < 0x4_0000 { 0x4_0000 } else { 0x4_8000 };
        globals.0.range(..end).any(|(_, g)| g.lo + g.len > o)
    };
    let heap = heap::follow(
        &ins,
        settled.map_or(&no_sites, |(f, _, _)| &f.sites),
        taken.as_ref(),
        &ram_init,
        &global_from,
    );
    drop(ins);
    drop(flow_ins);
    // The summaries again, each indirect call with a complete callee set
    // taken as calling exactly those (native/indirect-call-args). The
    // frames, which the flows read, stay the first summaries'. With the
    // flow skipped there are no sets, and the first summaries stand.
    let sets = fnrefs
        .as_ref()
        .map(|f| complete_sets(&f.0))
        .unwrap_or_default();
    if !sets.is_empty() {
        for c in &mut codes {
            c.sums = summarize_sets(&c.ctx, &c.funcs, Assume::default(), &sets);
        }
    }
    let shapes = shapes_of(&codes, &all, holes);
    let args = call_fn_args(&sets, &codes, &shapes.shaped);
    // Each function's text by key, in print order: the modules' contents.
    let mut out: Vec<(u32, String)> = Vec::new();
    let mut report = String::new();
    let mut hard = bad_copies;
    let mut soft = Soft::default();
    let mut fns = Vec::new();
    let entry = key(0x0800_0000, false);
    for c in &codes {
        for f in &c.funcs {
            fns.push(f.entry);
            if let Some((code, why)) = holes.standins.get(&f.entry) {
                let mut t = format!("// f_{:08x}: a stand-in: {why}\n", f.entry);
                t.push_str(code);
                if !t.ends_with('\n') {
                    t.push('\n');
                }
                t.push('\n');
                out.push((f.entry, t));
                soft.standins += 1;
                continue;
            }
            if shapes.callee_hole.contains_key(&f.entry) {
                soft.callee_holes += 1;
            }
            let mut p = FnPrinter {
                ctx: &c.ctx,
                f,
                all: &all,
                shapes: &shapes.shaped,
                me: shapes.shaped.get(&f.entry).copied(),
                holes,
                found: Vec::new(),
                soft: &mut soft,
                labels: f.blocks.keys().enumerate().map(|(i, &k)| (k, i)).collect(),
                frame: c.frames.get(&f.entry),
                locals: c
                    .frames
                    .get(&f.entry)
                    .map(Frame::locals)
                    .unwrap_or_default(),
                cur: f.entry,
                globals: &globals,
                args: &args,
                heap: &heap,
            };
            let s = c.sums.get(f.entry);
            let text = p.print(s);
            if p.found.is_empty() {
                out.push((f.entry, text));
            } else {
                let mut t = String::new();
                for h in &p.found {
                    writeln!(
                        t,
                        "compile_error!(\"{}\");",
                        h.to_string().replace('"', "'")
                    )
                    .unwrap();
                }
                out.push((f.entry, t));
                hard.extend(p.found);
            }
        }
    }
    for k in holes.standins.keys().filter(|k| !all.contains(k)) {
        hard.push(Hole {
            func: *k,
            at: *k,
            what: "the holes file's stand-in matches no function".into(),
        });
    }
    fns.sort_unstable();
    let mut table = String::new();
    writeln!(table, "pub static FNS: [(u32, Fn); {}] = [", fns.len()).unwrap();
    for k in &fns {
        writeln!(table, "    ({k:#010x}, {}),", plain(&shapes.shaped, *k)).unwrap();
    }
    table.push_str("];\n");
    let n_funcs = fns.len();
    let mut main = String::new();
    main.push_str("// Generated by gbaa-lift (crates/codegen/src/native.rs). Don't edit; rerun `just lift`.\n");
    main.push_str("#![allow(unused_mut, unused_parens, unused_variables, unused_imports, unused_assignments, unreachable_code, non_snake_case, dead_code, clippy::all)]\n\n");
    let mut pre = prelude(holes.layout);
    for (on, flag) in [
        (holes.irq_at_waits, "IRQ_HOLD"),
        (holes.ime_at_waits, "IME_RELEASE"),
    ] {
        let set = format!("pub const {flag}: bool = true;");
        debug_assert!(pre.contains(&set));
        if on {
            pre = pre.replace(&set, &format!("pub const {flag}: bool = false;"));
        }
    }
    main.push_str(&pre);
    main.push('\n');
    main.push_str(&globals.ram_struct(holes.layout));
    for &t in &soft.ref_tys {
        main.push_str(&heap.types[t].decl());
    }
    writeln!(main, "\npub const NAME: &str = {name:?};").unwrap();
    writeln!(
        main,
        "pub const ENTRY: Fn = {};\n",
        plain(&shapes.shaped, entry)
    )
    .unwrap();
    let plains: Vec<String> = fns.iter().map(|&k| plain(&shapes.shaped, k)).collect();
    let files = modules::split(main, table, &plains, &out, holes.copies);
    let blocks: usize = codes
        .iter()
        .flat_map(|c| &c.funcs)
        .map(|f| f.blocks.len())
        .sum();
    writeln!(
        report,
        "gbaa-lift: {name}: {n_funcs} functions ({} from IWRAM copies), {blocks} blocks; {} hard holes, {} stand-ins; code discovery missed, descended from {} targets: {}",
        codes[1..].iter().map(|c| c.funcs.len()).sum::<usize>(),
        hard.len(),
        soft.standins,
        gaps.len(),
        gaps.iter().map(|k| format!("{k:#010x}")).collect::<Vec<_>>().join(" ")
    )
    .unwrap();
    for h in &hard {
        writeln!(report, "  {h}").unwrap();
    }
    writeln!(
        report,
        "falls and jumps into another function's entry, run on with the caller's registers and flags: {}{}",
        soft.falls,
        if holes.fall_as_tail_call {
            " (--fall-as-tail-call: every one a tail call, r4-r12 and the flags zeroed)"
        } else {
            ""
        }
    )
    .unwrap();
    if holes.irq_at_waits {
        writeln!(
            report,
            "--irq-at-waits: IRQs run at waits whatever the CPSR's I bit (stage A's old Game::cpsr)"
        )
        .unwrap();
    }
    if holes.ime_at_waits {
        writeln!(
            report,
            "--ime-at-waits: an IRQ pending when IME is set again waits for the next wait (the old IME)"
        )
        .unwrap();
    }
    writeln!(
        report,
        "parameters and results from the summaries: {} functions shaped; four registers for the {} whose summary has a hole and the {} whose parameters rest on a callee's run that stopped at a hole; read after a tail call by value: {{{}}}",
        shapes.shaped.len(),
        soft.summary_holes,
        soft.callee_holes,
        reg_list(shapes.by_value as u16)
    )
    .unwrap();
    for (f, c) in &shapes.callee_hole {
        writeln!(report, "  f_{f:08x}: through f_{c:08x}").unwrap();
    }
    writeln!(
        report,
        "callee-saves not proven (soft: each function's r4-r11 and sp are its own locals): {}",
        soft.unproven.len()
    )
    .unwrap();
    for (f, why) in &soft.unproven {
        writeln!(report, "  f_{f:08x}: {why}").unwrap();
    }
    // The stack frames of the functions printed (a stand-in's isn't).
    let frames: Vec<&Frame> = codes
        .iter()
        .flat_map(|c| {
            c.funcs
                .iter()
                .filter(|f| !holes.standins.contains_key(&f.entry))
                .filter_map(|f| c.frames.get(&f.entry))
        })
        .collect();
    let slots: usize = frames.iter().map(|f| f.slots.len()).sum();
    let escaping: usize = frames.iter().map(|f| f.escaping()).sum();
    let by_kind = Kind::ALL
        .iter()
        .map(|&k| {
            let n: usize = frames
                .iter()
                .flat_map(|f| &f.slots)
                .filter(|s| s.escapes & k as u8 != 0)
                .count();
            format!("{} {n}", k.what())
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        report,
        "stack frames (native/frame.rs): {slots} slots in {} functions with any, {escaping} of them escaping ({by_kind}); {} functions lose sp's offset; sp-relative accesses left in memory to slots that don't escape {}, that do {}, in the caller's frame {}",
        frames.iter().filter(|f| !f.slots.is_empty()).count(),
        frames.iter().filter(|f| f.lost).count(),
        soft.sp_slots[0],
        soft.sp_slots[1],
        soft.sp_slots[2],
    )
    .unwrap();
    match (&fnrefs, &holes.skip_fn_flow) {
        (Some(f), _) => report.push_str(&f.0.report(&f.1, &f.2)),
        (None, why) => writeln!(
            report,
            "function addresses (native/fnref-flow): not followed, as the holes file says: {}",
            why.as_deref().unwrap_or("")
        )
        .unwrap(),
    }
    writeln!(
        report,
        "indirect call arguments (native/indirect-call-args): {} of {} call_fn calls have a complete callee set, {} of them passing all of r0-r3; of the {} complete sets, {} have callees whose summaries read all four between them, {} a four-register callee (a summary hole, a stand-in)",
        soft.indirect_complete,
        soft.indirect,
        soft.indirect_four_complete,
        sets.len(),
        args.union_four,
        args.four_reg_callee,
    )
    .unwrap();
    writeln!(
        report,
        "stack locals (native/frame-locals): {} slots in {} functions printed as Rust locals, {} loads and stores to them; {} calls write the locals to the frame first",
        soft.local_slots,
        soft.local_fns,
        soft.locals,
        frames.iter().map(|f| f.flush.len()).sum::<usize>(),
    )
    .unwrap();
    let objs = |st| soft.access.get(&(st, Where::Object)).copied().unwrap_or(0);
    writeln!(
        report,
        "pointers into one global (native/ref-in-object): {} loads and {} stores through a computed address the value sets bound to one typed global; {} computed RAM accesses no one global explains (pointer arithmetic the types don't explain)",
        objs(false),
        objs(true),
        soft.unexplained,
    )
    .unwrap();
    let refs = |st| soft.access.get(&(st, Where::Ref)).copied().unwrap_or(0);
    writeln!(
        report,
        "heap pointers (native/ref-heap): {} loads and {} stores through a Ref<T>, {} types; {} accesses through an object's pointer no type explains (left as bytes; {} sites' objects may be in a static array); the run took {} rounds, {:.1} s{}",
        refs(false),
        refs(true),
        soft.ref_tys.len(),
        soft.ref_unexplained,
        heap.statics.len(),
        heap.rounds,
        heap.secs,
        if heap.unsettled { " and didn't settle: nothing typed" } else { "" },
    )
    .unwrap();
    if let Some(at) = holes.plant_slot
        && !soft.planted_slot
    {
        let h = Hole {
            func: at,
            at,
            what: "--plant-slot: no store to a stack local there".into(),
        };
        writeln!(report, "  {h}").unwrap();
        hard.push(h);
    }
    let mut m = Metrics::default();
    let n = |x: usize| x as u64;
    // The prelude is hand-written, not lifted: its own metric, which may
    // rise with the evidence of why; `lines` (the lifted code, stand-ins
    // included) only goes down.
    let prelude = pre.lines().count();
    let all_lines: usize = files.files.iter().map(|(_, t)| t.lines().count()).sum();
    m.push("lines", n(all_lines - files.wiring - prelude));
    m.push("lines_prelude", n(prelude));
    m.push("functions", n(n_funcs));
    m.push(
        "functions_iwram",
        n(codes[1..].iter().map(|c| c.funcs.len()).sum::<usize>()),
    );
    m.push("functions_lifted", n(n_funcs - soft.standins));
    // Library recognition (docs/native-stage-b.md, piece 2) is parked:
    // nothing is recognised yet, and nothing replaced but the stand-ins.
    m.push("functions_recognised", 0);
    m.push("functions_unrecognised", n(n_funcs));
    m.push("functions_replaced", n(soft.standins));
    m.push("standins", n(soft.standins));
    m.push("functions_summary_hole", n(soft.summary_holes));
    m.push(
        "four_register_functions_without_summary_hole",
        n(soft.four_reg_clean),
    );
    m.push("callee_saves_unproven", n(soft.unproven.len()));
    m.push("hard_holes", n(hard.len()));
    m.push("blocks", n(blocks));
    m.push_accesses(&soft.access);
    m.push("ram_globals", n(globals.0.len()));
    m.push("pointer_unexplained", soft.unexplained);
    m.push(
        "ref_accesses",
        soft.access.get(&(false, Where::Ref)).copied().unwrap_or(0)
            + soft.access.get(&(true, Where::Ref)).copied().unwrap_or(0),
    );
    m.push("ref_types", n(soft.ref_tys.len()));
    m.push("ref_unexplained", soft.ref_unexplained);
    m.push("sp_accesses_local_slot", soft.sp_slots[0]);
    m.push("sp_accesses_escaping_slot", soft.sp_slots[1]);
    m.push("sp_accesses_callers_frame", soft.sp_slots[2]);
    m.push("call_fn_calls", n(soft.indirect));
    m.push("call_fn_complete", n(soft.indirect_complete));
    m.push("call_fn_four_complete", n(soft.indirect_four_complete));
    m.push("jumps_by_value", n(soft.dyn_jumps));
    if let Some(f) = &fnrefs {
        let fn_proven = f.1.keys().filter(|w| f.0.proven(**w)).count();
        m.push("fn_words_proven", n(fn_proven));
        m.push("fn_words_not_proven", n(f.1.len() - fn_proven));
    }
    m.push("gap_targets", n(gaps.len()));
    writeln!(
        report,
        "modules (native/modules.rs): {} files under src/, the largest {} lines{}",
        files.files.len(),
        files
            .files
            .iter()
            .map(|(_, t)| t.lines().count())
            .max()
            .unwrap_or(0),
        if holes.copies > 1 {
            format!(
                " (--copies {}: a synthetic lift, not the program)",
                holes.copies
            )
        } else {
            String::new()
        }
    )
    .unwrap();
    writeln!(
        report,
        "metrics (`name value`; bars: crates/codegen/lift-check.toml):"
    )
    .unwrap();
    report.push_str(&m.lines());
    Lifted {
        files: files.files,
        holes: hard,
        report,
        metrics: m,
    }
}

/// The ROM targets `funcs` jump to (not call: a call's return site with
/// no code follows a call that never returns) where discovery found no
/// code.
fn missing(ctx: &Ctx, funcs: &[Func]) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for gb in funcs.iter().flat_map(|f| f.blocks.values()) {
        for x in exits(ctx, gb) {
            if let Plan::Static(k) | Plan::Rekey(k) = x.plan
                && (0x0800_0000..0x0A00_0000).contains(&(k & !1))
                && !ctx.is_code(k)
            {
                out.insert(k);
            }
        }
    }
    out
}

/// Take the ROM from `k` (a key) as code, up to the next instruction
/// discovery has or 64 instructions on: the block builder stops at the
/// first exit, so what lies past it is used only if something reaches it.
/// A Thumb `bl`'s halves are joined, the prefix's entry too if discovery
/// split them.
fn force_code(rom: &[u8], disc: &mut Discovery, k: u32) {
    use analysis::discovery::{Code, Evidence};
    use armv4t::decode::{Instr, decode_thumb};
    let thumb = k & 1 != 0;
    let mut a = k & !1;
    let half = |a: u32| -> Option<u16> {
        let o = (a - 0x0800_0000) as usize;
        rom.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let code = |size| Code {
        size,
        thumb,
        evidence: Evidence::Scanned,
    };
    // A `bl` whose return site (past any `nop`s, `mov r8, r8`) is data
    // discovery knows (a literal pool) never returns: what follows it
    // isn't code. Decoding on would take the pool for instructions
    // (agb's qrcodegen: a panic call, a `nop`, then the pool).
    let pools = disc.data.clone();
    let data = |a: u32| {
        let i = pools.partition_point(|&(_, end)| end <= a);
        pools.get(i).is_some_and(|&(start, _)| start <= a)
    };
    let no_return = |mut a: u32| {
        while half(a) == Some(0x46C0) {
            a += 2;
        }
        data(a)
    };
    for _ in 0..64 {
        if disc.code.get(&a).is_some_and(|c| c.thumb == thumb) {
            return;
        }
        if !thumb {
            if (a - 0x0800_0000) as usize + 4 > rom.len() {
                return;
            }
            disc.code.insert(a, code(4));
            a += 4;
            continue;
        }
        let Some(h) = half(a) else { return };
        match decode_thumb(h, a) {
            Instr::ThumbBlHi { .. }
                if half(a + 2)
                    .is_some_and(|l| matches!(decode_thumb(l, a + 2), Instr::ThumbBlLo { .. })) =>
            {
                disc.code.insert(a, code(4));
                a += 4;
                if no_return(a) {
                    return;
                }
            }
            Instr::ThumbBlLo { .. }
                if disc
                    .code
                    .get(&(a - 2))
                    .is_some_and(|c| c.thumb && c.size == 2)
                    && half(a - 2).is_some_and(|p| {
                        matches!(decode_thumb(p, a - 2), Instr::ThumbBlHi { .. })
                    }) =>
            {
                disc.code.insert(a - 2, code(4));
                a += 2;
                if no_return(a) {
                    return;
                }
            }
            _ => {
                disc.code.insert(a, code(2));
                a += 2;
            }
        }
    }
}

/// The program's bytes, for [`fnref`].
struct FnImage<'a> {
    rom: &'a [u8],
    all: &'a BTreeSet<u32>,
    disc: &'a Discovery,
}

impl fnref::Image for FnImage<'_> {
    fn is_fn(&self, v: u32) -> bool {
        self.all.contains(&v)
    }
    fn word(&self, a: u32) -> Option<u32> {
        let o = a.checked_sub(0x0800_0000)? as usize & !3;
        let b = self.rom.get(o..o + 4)?;
        Some(u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn bound(&self, a: u32) -> u32 {
        self.disc
            .code
            .range(a + 1..)
            .next()
            .map_or(0x0800_0000 + self.rom.len() as u32, |(k, _)| *k)
    }
}

/// The ROM address of the literal an `ldr rN, [pc, #k]` at `at` reads, if
/// it is one and the word there is `c`. `src`: where the context's bytes
/// are in ROM (an IWRAM copy's source).
fn literal(ctx: &Ctx, src: u32, at: u32, thumb: bool, c: u32) -> Option<u32> {
    let rd = |a: u32, n: usize| -> Option<u32> {
        let o = a.checked_sub(ctx.base)? as usize;
        let b = ctx.rom.get(o..o + n)?;
        Some(b.iter().rev().fold(0, |x, &y| x << 8 | y as u32))
    };
    let a = if thumb {
        let h = rd(at, 2)?;
        (h & 0xf800 == 0x4800).then(|| ((at + 4) & !3) + (h & 0xff) * 4)?
    } else {
        let w = rd(at, 4)?;
        let off = w & 0xfff;
        match w & 0x0fff_0000 {
            0x059f_0000 => at + 8 + off,
            0x051f_0000 => (at + 8).wrapping_sub(off),
            _ => return None,
        }
    };
    (rd(a, 4)? == c).then(|| a - ctx.base + src)
}

/// One code context, for [`fn_flow`]: its functions and their frames, and
/// where its bytes are in ROM (an IWRAM copy's source).
struct FlowIn<'a> {
    ctx: &'a Ctx<'a>,
    src: u32,
    funcs: &'a [Func],
    frames: &'a BTreeMap<u32, Frame>,
}

/// [`fnref::follow`] over every context's functions: the flow, every
/// function word in ROM outside code (its key, and whether a literal pool
/// read it), and the addresses of the calls printed through `call_fn`.
#[allow(clippy::type_complexity)]
fn fn_flow(
    rom: &[u8],
    disc: &Discovery,
    overlays: &[discovery::Overlay],
    ins: &[fnref::FnIn],
    pools: &BTreeSet<u32>,
    all: &BTreeSet<u32>,
) -> (fnref::Flow, BTreeMap<u32, (u32, bool)>) {
    // Every function word in ROM that isn't an instruction (of the ROM's
    // code, or an IWRAM copy's).
    let in_code = |d: &Discovery, a: u32| {
        d.code
            .range(a.saturating_sub(3)..a + 4)
            .any(|(&k, c)| k + c.size > a)
    };
    let mut words = BTreeMap::new();
    for (i, b) in rom.as_chunks::<4>().0.iter().enumerate() {
        let w = 0x0800_0000 + 4 * i as u32;
        let v = u32::from_le_bytes(*b);
        if !all.contains(&v) || in_code(disc, w) {
            continue;
        }
        let copied = overlays
            .iter()
            .any(|o| (o.src..o.src + o.len).contains(&w) && in_code(&o.disc, o.dst + (w - o.src)));
        if !copied {
            words.insert(w, (v, pools.contains(&w)));
        }
    }
    // The start's copies of ROM data (not code) to RAM.
    let copies: Vec<(u32, u32, u32)> = disc
        .copies
        .iter()
        .copied()
        .filter(|(src, _, _)| !overlays.iter().any(|o| o.src == *src))
        .collect();
    let img = FnImage { rom, all, disc };
    (fnref::follow(ins, &img, &copies), words)
}

/// Every context's functions as the flows take them ([`fnref::FnIn`]),
/// the ROM words literal pools read that hold a function's key, and the
/// addresses of the calls printed through `call_fn`.
#[allow(clippy::type_complexity)]
fn fn_ins<'a>(
    codes: &[FlowIn<'a>],
    all: &BTreeSet<u32>,
) -> (Vec<fnref::FnIn<'a>>, BTreeSet<u32>, BTreeSet<u32>) {
    let mut ins = Vec::new();
    let mut pools = BTreeSet::new();
    let mut call_fn = BTreeSet::new();
    for c in codes {
        let src = c.src;
        for f in c.funcs {
            let mut blocks = BTreeMap::new();
            for (&k, gb) in &f.blocks {
                let insts = &gb.block.insts;
                let mut exits_of = BTreeMap::new();
                for x in exits(c.ctx, gb) {
                    let e = frame::edge(c.ctx, f, all, gb, &x);
                    if matches!(e, frame::Edge::Call { target: None, .. }) {
                        call_fn.insert(x.at);
                    }
                    let via = match x.plan {
                        Plan::CallVia { reg, .. } => Some(reg.index() as u8),
                        _ => None,
                    };
                    let bios = match insts[x.index].op {
                        Op::Exit(Exit::Swi { comment, .. }) => {
                            Some(runtime::bios::number(comment, x.thumb))
                        }
                        _ => None,
                    };
                    exits_of.insert(
                        x.index,
                        fnref::Out {
                            edge: e,
                            via,
                            at: x.at,
                            bios,
                        },
                    );
                }
                let mut at = k & !1;
                let mut lits = BTreeMap::new();
                let mut hulls = BTreeMap::new();
                for (i, inst) in insts.iter().enumerate() {
                    match inst.op {
                        Op::Insn(a) => at = a,
                        Op::Load { .. } | Op::Store { .. } => {
                            if let Some(x) = c.ctx.disc.rec.accesses.get(&at)
                                && let (false, false, Some(h)) = (x.sp, x.top, x.hull)
                            {
                                hulls.insert(i, h);
                            }
                        }
                        Op::Pool(v) => {
                            if let Some(w) = literal(c.ctx, src, at, gb.block.thumb, v) {
                                lits.insert(i, w);
                                if all.contains(&v) {
                                    pools.insert(w);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                blocks.insert(
                    k,
                    fnref::FBlock {
                        insts,
                        exits: exits_of,
                        pools: lits,
                        hulls,
                    },
                );
            }
            ins.push(fnref::FnIn {
                entry: f.entry,
                blocks,
                frame: c.frames.get(&f.entry),
            });
        }
    }
    (ins, pools, call_fn)
}

/// `holes` with a stand-in for each library routine the code has
/// (`Ctx::library`: libgcc's `__muldf3` and `__divdf3`, a call of
/// `runtime::softfloat`) and for each function inside one's code (its
/// special-case paths, which only it calls: never run), unless the holes
/// file has its own.
fn library_standins(holes: &Holes, codes: &[Code]) -> Holes {
    let mut h = holes.clone();
    for c in codes {
        let lib = c.ctx.library();
        for f in c.funcs.iter().filter(|f| lib.inside(f.entry)) {
            let k = f.entry;
            let (code, why) = match lib.routines.get(&k) {
                Some(r) => (
                    format!(
                        "pub fn f_{k:08x}(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {{\n    \
                         let r = {}(a[0] as u64 | (a[1] as u64) << 32, a[2] as u64 | (a[3] as u64) << 32);\n    \
                         Ok([r as u32, (r >> 32) as u32, a[2], a[3]])\n}}\n",
                        r.call()
                    ),
                    format!(
                        "libgcc's {} (analysis::softfloat), found by its code: {}, bit-exact",
                        r.name(),
                        r.call()
                    ),
                ),
                None => (
                    format!(
                        "pub fn f_{k:08x}(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {{\n    \
                         unreachable!(\"0x{k:08x} is inside a libgcc routine that runtime::softfloat stands in for\")\n}}\n"
                    ),
                    "a path inside a libgcc routine the runtime stands in for (analysis::softfloat); only the routine calls it".into(),
                ),
            };
            h.standins.entry(k).or_insert((code, why));
        }
    }
    h
}

fn code(ctx: Ctx<'_>) -> Code<'_> {
    let funcs: Vec<Func> = ctx.entries.iter().map(|&e| build_func(&ctx, e)).collect();
    let sums = summarize(&ctx, &funcs);
    let frames = frame::of_code(&ctx, &funcs, &sums);
    Code {
        ctx,
        funcs,
        sums,
        frames,
    }
}

#[derive(Default)]
struct Soft {
    /// Loads (false) and stores (true) by where their address is.
    access: BTreeMap<(bool, Where), u64>,
    /// The sp-relative ones left in memory: to slots that don't escape
    /// (none should be: they are locals), to slots that do, and to the
    /// caller's frame.
    sp_slots: [u64; 3],
    /// Loads and stores printed as a stack local's.
    locals: u64,
    /// Computed RAM accesses no one global explains (native/ref-in-object).
    unexplained: u64,
    /// Accesses through a heap object's pointer no type explains
    /// (native/ref-heap).
    ref_unexplained: u64,
    /// The heap's types the printed accesses use.
    ref_tys: BTreeSet<usize>,
    /// Stack locals, and the functions with any.
    local_slots: usize,
    local_fns: usize,
    /// The `--plant-slot` store printed.
    planted_slot: bool,
    /// Lifted functions whose summary has a hole, or that have none.
    summary_holes: usize,
    /// Lifted functions taking and returning all of r0-r3 whose summary
    /// has no hole (stage A printed every function so).
    four_reg_clean: usize,
    /// Lifted functions with a clean summary whose parameters rest on a
    /// callee's run that stopped at a hole ([`Shapes::callee_hole`]).
    callee_holes: usize,
    indirect: usize,
    /// Those with a complete callee set, and of them those passing all
    /// of r0-r3 still.
    indirect_complete: usize,
    indirect_four_complete: usize,
    dyn_jumps: usize,
    /// Falls and jumps into another function's entry, printed through its
    /// extended entry.
    falls: usize,
    standins: usize,
    /// Functions whose callee-saves the summaries don't prove, and why.
    unproven: Vec<(u32, String)>,
}

struct FnPrinter<'a, 'b> {
    ctx: &'a Ctx<'a>,
    f: &'a Func,
    all: &'a BTreeSet<u32>,
    /// Every shaped function's parameters and results.
    shapes: &'a BTreeMap<u32, Shape>,
    /// This one's, `None` for the four-register form.
    me: Option<Shape>,
    holes: &'a Holes,
    found: Vec<Hole>,
    soft: &'b mut Soft,
    labels: BTreeMap<u32, usize>,
    /// The function's stack frame.
    frame: Option<&'a Frame>,
    /// Its slots printed as Rust locals ([`Frame::locals`]).
    locals: Vec<Local>,
    /// The block being printed.
    cur: u32,
    /// RAM's typed globals.
    globals: &'a Globals,
    /// What each `call_fn` call with a complete callee set passes and
    /// takes back ([`call_fn_args`]), by its address.
    args: &'a Args,
    /// The heap's types and the accesses through them.
    heap: &'a heap::Heap,
}

fn v(i: ValueId) -> String {
    format!("v{}", i.0)
}

fn flag(f: Flag) -> &'static str {
    match f {
        Flag::N => "nf",
        Flag::Z => "zf",
        Flag::C => "cf",
        Flag::V => "vf",
    }
}

fn shift(k: ShiftType) -> &'static str {
    match k {
        ShiftType::Lsl => "Lsl",
        ShiftType::Lsr => "Lsr",
        ShiftType::Asr => "Asr",
        ShiftType::Ror => "Ror",
    }
}

fn is_bit(op: &Op) -> bool {
    crate::print::is_bit(op)
}

impl FnPrinter<'_, '_> {
    fn hole(&mut self, at: u32, what: impl Into<String>) {
        self.found.push(Hole {
            func: self.f.entry,
            at,
            what: what.into(),
        });
    }

    fn print(&mut self, sum: Option<&Summary>) -> String {
        let f = self.f;
        let mut s = String::new();
        match sum {
            Some(sum) if sum.holes.is_empty() => {
                if self.me.is_none() {
                    self.soft.four_reg_clean += 1;
                }
            }
            _ => self.soft.summary_holes += 1,
        }
        // The summary, as a comment: what a four-register function reads
        // and defines (a shaped one's signature says it).
        if let Some(sum) = sum {
            let regs = |m: u16| {
                (0..4)
                    .filter(|r| m & 1 << r != 0)
                    .map(|r| format!("r{r}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            if self.me.is_none() {
                writeln!(
                    s,
                    "/// Reads {{{}}}; returns {{{}}}{}.",
                    regs(sum.params as u16),
                    regs(sum.results),
                    if sum.returns { "" } else { "; never returns" }
                )
                .unwrap();
            } else if !sum.returns {
                s.push_str("/// Never returns.\n");
            }
            // Callers keep r4-r11 and sp in their own locals, so a callee
            // can't change them; only a function that returns sp changed,
            // or reads r4-r11 a callee changed or as they came
            // (`analysis::holes::SUMMARY_HARD`), is lifted wrongly.
            if let Some(h) = sum
                .holes
                .iter()
                .find(|h| analysis::holes::summary_hard(&analysis::holes::summary_kind(h)))
            {
                self.hole(f.entry, format!("summary: {h}"));
            } else if sum.returns && !sum.keeps {
                self.soft.unproven.push((
                    f.entry,
                    sum.holes.iter().next().cloned().unwrap_or_default(),
                ));
            }
        }
        // A stack switch (a task switcher's): sp is the frame's own
        // natively, so the frames the guest left on the stack it leaves
        // would be lost. A stand-in runs such code on host coroutines
        // (the prelude's `task_resume`, `task_yield`, `task_exit`).
        for &at in self.frame.map(|fr| &fr.switches).into_iter().flatten() {
            self.hole(
                at,
                "CPU state: a stack switch (sp set to a value not made from the frame's)",
            );
        }
        // sp set to a constant (a fresh stack, the SDK's `SoftReset`) is
        // the guest's bytes from there on, as lifted; only a return could
        // reach the frames left behind.
        if sum.is_none_or(|s| s.returns) {
            for &at in self.frame.map(|fr| &fr.fresh).into_iter().flatten() {
                self.hole(
                    at,
                    "CPU state: a stack switch (sp set to a constant in a function that returns)",
                );
            }
        }
        // The plain entry, for calls, and the extended one, for a fall or
        // jump from another function, which passes its registers and flags.
        // A shaped function's plain entry takes its parameters and gives
        // its results; its adapter, `a_<key>`, is the four-register form
        // `call_fn` takes it by (`FNS`), and gives back the registers that
        // aren't results as they came.
        let k = f.entry;
        let ty = ret_ty(self.me);
        match self.me {
            None => writeln!(
                s,
                "pub fn f_{k:08x}(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {{ x_{k:08x}(g, Regs::call(sp, a)) }}"
            )
            .unwrap(),
            Some(sh) => {
                let ps: Vec<String> = shape::regs(sh.params)
                    .map(|r| format!("a{r}: u32"))
                    .collect();
                let a: Vec<String> = (0..4)
                    .map(|r| {
                        if sh.params & 1 << r != 0 {
                            format!("a{r}")
                        } else {
                            "0".into()
                        }
                    })
                    .collect();
                writeln!(
                    s,
                    "pub fn f_{k:08x}(g: &mut Game, sp: u32{}) -> Result<{ty}, Exit> {{ x_{k:08x}(g, Regs::call(sp, [{}])) }}",
                    ps.iter().map(|p| format!(", {p}")).collect::<String>(),
                    a.join(", ")
                )
                .unwrap();
                writeln!(
                    s,
                    "pub fn a_{k:08x}(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {{ let [mut r0, mut r1, mut r2, mut r3] = a; {}f_{k:08x}(g, sp{})?; Ok([r0, r1, r2, r3]) }}",
                    lhs(sh.results),
                    args(sh.params)
                )
                .unwrap();
            }
        }
        writeln!(
            s,
            "pub fn x_{k:08x}(g: &mut Game, s: Regs) -> Result<{ty}, Exit> {{"
        )
        .unwrap();
        s.push_str("    let Regs { r: [mut r0, mut r1, mut r2, mut r3, mut r4, mut r5, mut r6, mut r7, mut r8, mut r9, mut r10, mut r11, mut r12, mut r13, mut r14], f: [mut nf, mut zf, mut cf, mut vf] } = s;\n");
        s.push_str(&self.declare_locals());
        if f.blocks.len() == 1 {
            s.push_str("    let b = 0usize;\n    loop {\n        match b {\n");
        } else {
            s.push_str("    let mut b = 0usize;\n    loop {\n        match b {\n");
        }
        // The entry first: label 0.
        let mut order: Vec<u32> = vec![f.entry];
        order.extend(f.blocks.keys().copied().filter(|&k| k != f.entry));
        self.labels = order.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        for k in &order {
            let gb = &f.blocks[k];
            let label = self.labels[k];
            writeln!(s, "            {label} => {{ // {:#010x}", gb.key).unwrap();
            let xs = exits(self.ctx, gb);
            self.cur = *k;
            let body = self.block(&gb.block.insts, &xs, gb.block.thumb);
            s.push_str(&body);
            s.push_str("            }\n");
        }
        s.push_str("            _ => unreachable!(),\n        }\n    }\n}\n\n");
        s
    }

    fn block(&mut self, insts: &[Inst], xs: &[ExitInfo], thumb: bool) -> String {
        let mut s = String::new();
        let mut at = 0u32;
        let ind = "                ";
        // The accesses printed as a typed global's field, and the address
        // arithmetic only they used, which isn't printed; and the values
        // nothing printed uses (a stack local's address among them).
        let typed: Vec<Option<Place>> = insts
            .iter()
            .map(|inst| {
                let (a, w) = const_access(insts, &inst.op)?;
                self.globals.place(a, w)
            })
            .collect();
        // The computed accesses into one global (native/ref-in-object),
        // which print their address: the pointer finds or checks the field.
        let mut pc = 0;
        let objs: Vec<Option<Obj>> = insts
            .iter()
            .map(|inst| {
                if let Op::Insn(a) = inst.op {
                    pc = a;
                }
                if const_access(insts, &inst.op).is_some() {
                    return None;
                }
                let (lo, hi, w) = computed_ram(self.ctx, insts, &inst.op, pc)?;
                self.globals
                    .object(globals::guest(lo), globals::guest(hi), w)
            })
            .collect();
        // The accesses the value sets don't bound that the heap's run
        // types (native/ref-heap): a field through a `Ref<T>`, which
        // prints its address too.
        let mut pc = 0;
        let refs: Vec<Option<HeapRef>> = insts
            .iter()
            .enumerate()
            .map(|(i, inst)| {
                if let Op::Insn(a) = inst.op {
                    pc = a;
                }
                let (Op::Load { addr, .. } | Op::Store { addr, .. }) = inst.op else {
                    return None;
                };
                if typed[i].is_some()
                    || objs[i].is_some()
                    || self.vsa_where(insts, addr, pc) != Where::Unknown
                {
                    return None;
                }
                let f = *self.heap.access.get(&(self.f.entry, self.cur, i))?;
                // Through the pointer the address adds a constant to, if
                // that's in the object: the addition isn't printed.
                let base = match insts[addr.index()].op {
                    Op::Bin(BinOp::Add, x, y) if konst(insts, addr).is_none() => {
                        match (konst(insts, x), konst(insts, y)) {
                            (None, Some(c)) if c <= f.off => Some((x, c)),
                            (Some(c), None) if c <= f.off => Some((y, c)),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                Some(HeapRef { f, base })
            })
            .collect();
        self.soft
            .ref_tys
            .extend(refs.iter().flatten().map(|r| r.f.ty));
        let dead = dead_addresses(insts, &typed);
        let used = self.used(insts, &typed, &refs);
        for (i, inst) in insts.iter().enumerate() {
            let id = ValueId(i as u32);
            if dead[i] || (pure(&inst.op) && !used[i]) {
                continue;
            }
            let guard = inst.guard.map(v);
            let stmt = |s: &mut String, body: String| match &guard {
                Some(g) => writeln!(s, "{ind}if {g} {{ {body} }}").unwrap(),
                None => writeln!(s, "{ind}{body}").unwrap(),
            };
            match inst.op {
                Op::Insn(a) => at = a,
                Op::Timing(_) | Op::YieldFlag(..) => {}
                Op::SetReg(r, x) => stmt(&mut s, format!("r{} = {};", r.index(), v(x))),
                Op::SetFlag(fl, x) => stmt(&mut s, format!("{} = {};", flag(fl), v(x))),
                Op::GetReg(r) => writeln!(s, "{ind}let {}: u32 = r{};", v(id), r.index()).unwrap(),
                Op::GetFlag(fl) => writeln!(s, "{ind}let {}: bool = {};", v(id), flag(fl)).unwrap(),
                Op::GetUserReg(_) | Op::SetUserReg(..) => {
                    self.hole(at, "CPU state: a user-bank register transfer")
                }
                Op::GetCpsr | Op::GetSpsr => self.hole(at, "CPU state: mrs (reads a PSR)"),
                Op::SetCpsr { .. } | Op::SetSpsr { .. } => {
                    self.hole(at, "CPU state: msr (writes a PSR)")
                }
                Op::Load { width, addr } => {
                    let (f, m) = match width {
                        Width::Byte => ("ld8", ""),
                        Width::Half => ("ld16", " & !1"),
                        Width::Word => ("ld32", " & !3"),
                    };
                    let mem = match (&typed[i], &objs[i], refs[i]) {
                        (Some(p), _, _) => globals::load(p, bytes(width)),
                        (None, Some(o), _) => o.load(&format!("{}{m}", v(addr)), bytes(width)),
                        (None, None, Some(r)) => match r.base {
                            Some((b, c)) => self.heap.load(r.f, &v(b), r.f.off - c),
                            None => self.heap.load(r.f, &v(addr), r.f.off),
                        },
                        (None, None, None) => format!("{f}(g, {}{m})", v(addr)),
                    };
                    let e = match self.local(i) {
                        Some((l, d, n)) => {
                            let x = local_read(&l, d, n);
                            if self.holes.shadow_locals {
                                format!(
                                    "{{ let x: u32 = {x}; let m: u32 = {mem}; if x != m {{ eprintln!(\"shadow local: {at:#010x} loads {{x:#x}} from {}, the frame's bytes hold {{m:#x}}\"); }} x }}",
                                    l.name()
                                )
                            } else {
                                x
                            }
                        }
                        None => {
                            self.count(
                                insts,
                                addr,
                                at,
                                i,
                                false,
                                typed_as(&typed[i], &objs[i], refs[i].is_some()),
                            );
                            mem
                        }
                    };
                    match &guard {
                        Some(gv) => writeln!(
                            s,
                            "{ind}let {}: u32 = if {gv} {{ {e} }} else {{ 0 }};",
                            v(id)
                        )
                        .unwrap(),
                        None => writeln!(s, "{ind}let {}: u32 = {e};", v(id)).unwrap(),
                    }
                }
                Op::Store { width, addr, value } => {
                    let (f, m) = match width {
                        Width::Byte => ("st8", ""),
                        Width::Half => ("st16", " & !1"),
                        Width::Word => ("st32", " & !3"),
                    };
                    let local = self.local(i);
                    let plant = if self.holes.plant == Some(at)
                        || (local.is_some() && self.holes.plant_slot == Some(at))
                    {
                        self.soft.planted_slot |= local.is_some();
                        ".wrapping_add(1)"
                    } else {
                        ""
                    };
                    let val = format!("{}{plant}", v(value));
                    let mem = match (&typed[i], &objs[i], refs[i]) {
                        (Some(p), _, _) => globals::store(p, bytes(width), &val),
                        (None, Some(o), _) => {
                            o.store(&format!("{}{m}", v(addr)), bytes(width), &val)
                        }
                        (None, None, Some(r)) => match r.base {
                            Some((b, c)) => self.heap.store(r.f, &v(b), r.f.off - c, &val),
                            None => self.heap.store(r.f, &v(addr), r.f.off, &val),
                        },
                        (None, None, None) => format!("{f}(g, {}{m}, {val});", v(addr)),
                    };
                    let body = match local {
                        Some((l, d, n)) => {
                            let w = local_write(&l, d, n, &val);
                            if self.holes.shadow_locals {
                                format!("{w} {mem}")
                            } else {
                                w
                            }
                        }
                        None => {
                            let w = self.count(
                                insts,
                                addr,
                                at,
                                i,
                                true,
                                typed_as(&typed[i], &objs[i], refs[i].is_some()),
                            );
                            // A store that may reach I/O may set `IME` again,
                            // which runs a pending IRQ's handler on the
                            // current stack.
                            let io = typed[i].is_none()
                                && objs[i].is_none()
                                && refs[i].is_none()
                                && match w {
                                    Where::Sp => false,
                                    Where::Const(r) => metrics::REGIONS[r] == "io",
                                    _ => true,
                                };
                            if io {
                                format!("g.sp = r13; {mem}")
                            } else {
                                mem
                            }
                        }
                    };
                    stmt(&mut s, body);
                }
                Op::Exit(e) => {
                    let x = xs
                        .iter()
                        .find(|x| x.index == i)
                        .copied()
                        .expect("every exit planned");
                    let body = format!("{}{}", self.flush(i), self.exit(e, &x, thumb));
                    match &guard {
                        Some(gv) => writeln!(s, "{ind}if {gv} {{\n{body}{ind}}}").unwrap(),
                        None => s.push_str(&body),
                    }
                }
                ref op => {
                    let ty = if is_bit(op) { "bool" } else { "u32" };
                    let e = expr(op);
                    writeln!(s, "{ind}let {}: {ty} = {e};", v(id)).unwrap();
                }
            }
        }
        s
    }

    /// Count an access by the instruction at `at`: one printed `typed` as
    /// a field as that; a constant address by
    /// its region; else as the value-set analysis knows the instruction's
    /// addresses (code it didn't reach, such as discovery's gaps, is
    /// unknown). An sp-relative one also by whether the slot it reaches
    /// escapes ([`frame`]; one the frame's run didn't place, as escaping).
    fn count(
        &mut self,
        insts: &[Inst],
        addr: ValueId,
        at: u32,
        i: usize,
        store: bool,
        typed: Option<Where>,
    ) -> Where {
        let w = typed.unwrap_or_else(|| self.vsa_where(insts, addr, at));
        *self.soft.access.entry((store, w)).or_default() += 1;
        self.soft.ref_unexplained += (w == Where::Unknown
            && self.heap.unexplained.contains(&(self.f.entry, self.cur, i)))
            as u64;
        self.soft.unexplained += (w == Where::Computed(0)) as u64;
        if w == Where::Sp {
            let esc = self
                .frame
                .and_then(|f| f.escapes_at(self.cur, i))
                .unwrap_or(true);
            let callers = self.frame.is_some_and(|f| f.in_callers(self.cur, i));
            let k = match (esc, callers) {
                (true, _) => 1,
                (false, true) => 2,
                (false, false) => 0,
            };
            self.soft.sp_slots[k] += 1;
        }
        w
    }

    /// Where an access by the instruction at `at` is, untyped: a constant
    /// address by its region; else as the value-set analysis knows the
    /// instruction's addresses.
    fn vsa_where(&self, insts: &[Inst], addr: ValueId, at: u32) -> Where {
        match konst(insts, addr) {
            Some(a) => Where::Const(metrics::region(a)),
            None => match self.ctx.disc.rec.accesses.get(&at) {
                Some(x) if x.stack_only() => Where::Sp,
                Some(x) if !x.sp && !x.top => x
                    .hull
                    .and_then(|(lo, hi)| metrics::region_of(lo, hi))
                    .map_or(Where::Unknown, Where::Computed),
                _ => Where::Unknown,
            },
        }
    }

    /// Which of a block's values something printed uses. An access
    /// printed as a stack local's (unless `--shadow-locals` reads the
    /// frame's bytes too) or a typed global's (`typed`) doesn't use its
    /// address, so the arithmetic only the address needed isn't printed
    /// ([`pure`]).
    fn used(&self, insts: &[Inst], typed: &[Option<Place>], refs: &[Option<HeapRef>]) -> Vec<bool> {
        let mut used = vec![false; insts.len()];
        for (i, inst) in insts.iter().enumerate().rev() {
            if pure(&inst.op) && !used[i] {
                continue;
            }
            let local = !self.holes.shadow_locals
                && self
                    .frame
                    .is_some_and(|f| f.local_at(&self.locals, self.cur, i).is_some());
            let field = typed[i].is_some();
            let base = refs[i].and_then(|r| r.base).map(|(b, _)| b);
            let ops = match inst.op {
                Op::Load { .. } if local || field => Vec::new(),
                Op::Store { value, .. } if local || field => vec![value],
                Op::Load { .. } if base.is_some() => base.into_iter().collect(),
                Op::Store { value, .. } if base.is_some() => {
                    base.into_iter().chain([value]).collect()
                }
                ref op => op.operands(),
            };
            for x in ops.into_iter().chain(inst.guard) {
                used[x.index()] = true;
            }
        }
        used
    }

    /// The stack local the access at inst `i` of the current block lands
    /// in, if any ([`Frame::local_at`]), counted.
    fn local(&mut self, i: usize) -> Option<(Local, u32, u8)> {
        let r = self.frame?.local_at(&self.locals, self.cur, i);
        self.soft.locals += r.is_some() as u64;
        r
    }

    /// The stack locals' declarations, after the entry's registers: each
    /// zero, or its frame bytes with `--shadow-locals`; and the entry sp,
    /// `sp0`, where a flush or the shadow needs the frame's address.
    fn declare_locals(&mut self) -> String {
        let mut s = String::new();
        if self.locals.is_empty() {
            return s;
        }
        self.soft.local_slots += self.locals.len();
        self.soft.local_fns += 1;
        let shadow = self.holes.shadow_locals;
        if shadow || self.frame.is_some_and(|f| !f.flush.is_empty()) {
            s.push_str("    let sp0: u32 = r13;\n");
        }
        for l in &self.locals {
            let name = l.name();
            let (ty, init) = if l.word {
                let init = if shadow {
                    format!("ld32(g, {})", local_addr(l, 0))
                } else {
                    "0".into()
                };
                ("u32".to_string(), init)
            } else {
                let init = if shadow {
                    let bs: Vec<String> = (0..l.len)
                        .map(|j| format!("ld8(g, {}) as u8", local_addr(l, j)))
                        .collect();
                    format!("[{}]", bs.join(", "))
                } else {
                    format!("[0; {}]", l.len)
                };
                (format!("[u8; {}]", l.len), init)
            };
            writeln!(s, "    let mut {name}: {ty} = {init};").unwrap();
        }
        s
    }

    /// Before a call the frame is done with after, whose callee may reach
    /// up into it ([`Frame::flush`]): every stack local to its bytes.
    fn flush(&self, i: usize) -> String {
        let ind = "                ";
        if !self.frame.is_some_and(|f| f.flush.contains(&(self.cur, i))) {
            return String::new();
        }
        let mut s = String::new();
        for l in &self.locals {
            let name = l.name();
            if l.word {
                writeln!(s, "{ind}st32(g, {}, {name});", local_addr(l, 0)).unwrap();
            } else {
                writeln!(
                    s,
                    "{ind}for (j, x) in {name}.iter().enumerate() {{ st8(g, {}.wrapping_add(j as u32), *x as u32); }}",
                    local_addr(l, 0)
                )
                .unwrap();
            }
        }
        s
    }

    /// A jump to block or function `k`: a label, or a fall into another
    /// function, which runs on with this one's registers and flags (its
    /// extended entry; a stand-in has only the plain one).
    fn goto(&mut self, k: u32, at: u32) -> String {
        if let Some(l) = self.labels.get(&k) {
            format!("b = {l}; continue;")
        } else if self.all.contains(&k) {
            self.fall_into(k)
        } else {
            self.hole(at, format!("a jump to {k:#010x}, where there's no code"));
            String::new()
        }
    }

    /// A fall or jump into function `k`'s entry, as the last thing this
    /// one does.
    fn fall_into(&mut self, k: u32) -> String {
        let callee = self.shape(k);
        let (lhs, call) = if self.holes.fall_as_tail_call || self.holes.standins.contains_key(&k) {
            self.call(k)
        } else {
            self.soft.falls += 1;
            (
                callee.map_or("[r0, r1, r2, r3] = ".into(), |c| lhs(c.results)),
                format!(
                    "x_{k:08x}(g, Regs {{ r: [r0, r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11, r12, r13, r14], f: [nf, zf, cf, vf] }})"
                ),
            )
        };
        self.tail(callee.map(|c| c.results), &lhs, &call)
    }

    /// `k`'s shape, `None` for a four-register function or a stand-in.
    fn shape(&self, k: u32) -> Option<Shape> {
        if self.holes.standins.contains_key(&k) {
            return None;
        }
        self.shapes.get(&k).copied()
    }

    /// A plain call of `k` with this function's registers: what it
    /// assigns its results to, and the call (without `?`).
    fn call(&self, k: u32) -> (String, String) {
        match self.shape(k) {
            None if self.holes.lr.contains(&k) => (
                "[r0, r1, r2, r3] = ".into(),
                format!("{{ g.lr = r14; f_{k:08x}(g, r13, [r0, r1, r2, r3]) }}"),
            ),
            None => (
                "[r0, r1, r2, r3] = ".into(),
                format!("f_{k:08x}(g, r13, [r0, r1, r2, r3])"),
            ),
            Some(c) => (
                lhs(c.results),
                format!("f_{k:08x}(g, r13{})", args(c.params)),
            ),
        }
    }

    /// The call statement: `lhs call?;`.
    fn call_stmt(&self, k: u32) -> String {
        let (lhs, call) = self.call(k);
        format!("{lhs}{call}?;")
    }

    /// The return statement.
    fn ret(&self) -> String {
        format!("return Ok({});", ret_expr(self.me))
    }

    /// A tail call, `call` giving `results` (`None`: all of r0-r3) into
    /// `lhs`: returned as it is where this function gives the same.
    fn tail(&self, results: Option<u8>, lhs: &str, call: &str) -> String {
        if results == self.me.map(|m| m.results) {
            format!("return {call};")
        } else {
            format!("{lhs}{call}?; {}", self.ret())
        }
    }

    fn call_then(&mut self, call: String, ret: u32, at: u32) -> String {
        // A return site that is another function's entry: the call
        // returns into it, as a fall (a callee that never returns never
        // gets there). One with no code: the callee never returns.
        let then = if self.labels.contains_key(&ret) {
            self.goto(ret, at)
        } else if self.all.contains(&ret) && !self.holes.fall_as_tail_call {
            self.fall_into(ret)
        } else {
            "unreachable!(\"a call that never returns returned\")".to_string()
        };
        format!("{call}\n                    {then}")
    }

    /// An indirect call at `at` to key `k` through `call_fn`: all of
    /// r0-r3 in and out, or, where its callee set is complete, what its
    /// callees read in and what they may change out.
    fn call_fn(&mut self, at: u32, k: &str) -> String {
        self.soft.indirect += 1;
        let (pass, back) = self.args.by_site.get(&at).copied().unwrap_or((0xf, 0xf));
        if self.args.by_site.contains_key(&at) {
            self.soft.indirect_complete += 1;
            if pass == 0xf {
                self.soft.indirect_four_complete += 1;
            }
        }
        call_fn_text(k, pass, back)
    }

    fn exit(&mut self, e: Exit, x: &ExitInfo, thumb: bool) -> String {
        let ind = "                    ";
        let at = x.at;
        let regs = "[r0, r1, r2, r3]";
        let mut s = String::new();
        let wait = match x.idle {
            Idle::Edge => "g.sp = r13; hw::idle(g)?; ",
            Idle::Spin => "g.sp = r13; hw::spin(g)?; ",
            _ => "",
        };
        match x.plan {
            Plan::Static(k) | Plan::Rekey(k) => {
                let j = self.goto(k, at);
                writeln!(s, "{ind}{wait}{j}").unwrap();
            }
            Plan::Call { target, ret } => {
                let c = self.call_stmt(target);
                let c = self.call_then(c, ret, at);
                writeln!(s, "{ind}{wait}{c}").unwrap();
            }
            Plan::CallVia { reg, ret, .. } => {
                let r = reg.index();
                let c = self.call_fn(
                    at,
                    &format!("if r{r} & 1 != 0 {{ r{r} }} else {{ r{r} & !3 }}"),
                );
                let c = self.call_then(c, ret, at);
                writeln!(s, "{ind}{wait}{c}").unwrap();
            }
            Plan::Dynamic { ret, target, .. } => {
                let t = match e {
                    Exit::Jump(t) | Exit::JumpExchange(t) => v(t),
                    Exit::ExceptionReturn(_) => {
                        self.hole(at, "CPU state: an exception return (CPSR = SPSR)");
                        return s;
                    }
                    _ => unreachable!("a dynamic exit jumps"),
                };
                // The key of the target value, as the jump takes it.
                let k = match e {
                    Exit::JumpExchange(_) => {
                        format!("(if {t} & 1 != 0 {{ {t} }} else {{ {t} & !3 }})")
                    }
                    _ if thumb => format!("({t} | 1)"),
                    _ => format!("({t} & !3)"),
                };
                if let Some(ret) = ret {
                    // An indirect call.
                    let c = match target {
                        Some(tk) if self.all.contains(&tk) => self.call_stmt(tk),
                        _ => self.call_fn(at, &k),
                    };
                    let c = self.call_then(c, ret, at);
                    writeln!(s, "{ind}{c}").unwrap();
                    return s;
                }
                // A return, a jump through a table or a known target, or a
                // tail call through a register.
                let targets = match dyn_targets(self.ctx, self.holes, at, thumb, target) {
                    Ok(ts) => ts,
                    Err(a) => {
                        self.hole(
                            a,
                            "an indirect jump Recovery didn't resolve (give its targets in the holes file)",
                        );
                        target.into_iter().collect()
                    }
                };
                writeln!(s, "{ind}let t: u32 = {t};").unwrap();
                writeln!(s, "{ind}if t & !3 == RET {{ {} }}", self.ret()).unwrap();
                if !targets.is_empty() {
                    self.soft.dyn_jumps += 1;
                    writeln!(s, "{ind}match {k} {{").unwrap();
                    for tk in targets {
                        let j = self.goto(tk, at);
                        writeln!(s, "{ind}    {tk:#010x} => {{ {j} }}").unwrap();
                    }
                    writeln!(s, "{ind}    _ => {{}}").unwrap();
                    writeln!(s, "{ind}}}").unwrap();
                }
                // Anything else is a function's entry, taken as a tail call
                // (`call_fn` panics on any other value).
                let t = self.tail(
                    None,
                    "[r0, r1, r2, r3] = ",
                    &format!("call_fn(g, {k}, r13, {regs})"),
                );
                writeln!(s, "{ind}{t}").unwrap();
            }
            Plan::Swi { next } => {
                let Exit::Swi { comment, .. } = e else {
                    unreachable!("a swi plan is a swi exit")
                };
                let n = runtime::bios::number(comment, thumb);
                let call = match n {
                    // The restart is `main`'s (the prelude's `soft_reset`).
                    0x00 => "soft_reset(g)?;".to_string(),
                    0x01 => "register_ram_reset(g, r0);".to_string(),
                    // `Stop` is a halt, as in the play build.
                    0x02 | 0x03 => "g.sp = r13; hw::halt(g)?;".to_string(),
                    0x04 => "r0 = intr_wait(g, r13, r0 != 0, r1)?;".to_string(),
                    0x05 => "r0 = intr_wait(g, r13, true, 1)?; r1 = 1;".to_string(),
                    0x06 => {
                        "let mut r = [r0, r1, r2, r3]; div(&mut r); [r0, r1, r2, r3] = r;".to_string()
                    }
                    0x08 => "r0 = hw::bios::sqrt(r0);".to_string(),
                    0x0A => "let (a, t) = hw::bios::arc_tan2_of(r0 as i32, r1 as i32); \
                        r0 = a as u32; r1 = t.map_or(r1, |t| t as u32); r3 = 0x170;"
                        .to_string(),
                    0x0B => "let mut r = [r0, r1, r2, r3]; cpu_set(g, r13, &mut r)?; [r0, r1, r2, r3] = r;".to_string(),
                    0x0C => "let mut r = [r0, r1, r2, r3]; cpu_fast_set(g, r13, &mut r)?; [r0, r1, r2, r3] = r;".to_string(),
                    0x0E | 0x0F => format!(
                        "let mut r = [r0, r1, r2, r3]; affine_set(g, &mut r, {}); [r0, r1, r2, r3] = r;",
                        n == 0x0E
                    ),
                    0x11 | 0x12 => format!(
                        "let mut r = [r0, r1, r2, r3]; lz77(g, r13, &mut r, {}, {at:#010x}, {thumb})?; [r0, r1, r2, r3] = r;",
                        n == 0x12
                    ),
                    0x13 => "let mut r = [r0, r1, r2, r3]; huff(g, &mut r); [r0, r1, r2, r3] = r;"
                        .to_string(),
                    0x14 | 0x15 => format!(
                        "let mut r = [r0, r1, r2, r3]; rl(g, &mut r, {}); [r0, r1, r2, r3] = r;",
                        n == 0x15
                    ),
                    0x19 => "sound_bias(g, r0);".to_string(),
                    0x1F => "r0 = hw::bios::midi_key2freq(ld32(g, r0.wrapping_add(4) & !3), r1, r2);"
                        .to_string(),
                    // No link cable: no client answers (the play build's).
                    0x25 => "r0 = 1;".to_string(),
                    // `SoundDriverVSyncOff`: the play build's BIOS has no
                    // sound driver, so does nothing.
                    0x28 => String::new(),
                    _ => {
                        debug_assert!(!BIOS.contains(&n));
                        self.hole(at, format!("BIOS call {n:#04x}, which the library doesn't have"));
                        return s;
                    }
                };
                // The BIOS region then reads the opcode after the call's
                // return, as the play build's (`swi_done`). After a halt too:
                // the IRQ that wakes it returns into the BIOS's Halt, which
                // then returns from the call (the play build finishes its
                // pending call there; mGBA agrees, agb/native-bios-region's
                // slot 10).
                let c = self.call_then(format!("{call} swi_done(g);"), next, at);
                writeln!(s, "{ind}{c}").unwrap();
            }
            // A compiler's trap: the game stops there, as `call_fn` does
            // on a call to no function.
            Plan::Undefined { trap: true } => writeln!(
                s,
                "{ind}panic!(\"{at:#010x}: a compiler's trap (undefined instruction), which stops the game\");"
            )
            .unwrap(),
            Plan::Undefined { trap: false } => self.hole(at, "an undefined instruction"),
        }
        s
    }
}

/// How an access printed typed is counted: a constant address's field
/// (`Global`), a computed one's (`Object`), or a heap object's through
/// its `Ref<T>` (`Ref`).
fn typed_as(field: &Option<Place>, obj: &Option<Obj>, r: bool) -> Option<Where> {
    match (field, obj, r) {
        (Some(_), _, _) => Some(Where::Global),
        (None, Some(_), _) => Some(Where::Object),
        (None, None, true) => Some(Where::Ref),
        (None, None, false) => None,
    }
}

/// An access printed as a heap object's field (native/ref-heap): the
/// field, and the pointer it goes through if not its address: a value
/// the address adds a constant to.
#[derive(Clone, Copy)]
struct HeapRef {
    f: heap::Field,
    base: Option<(ValueId, u32)>,
}

fn bytes(w: Width) -> u32 {
    match w {
        Width::Byte => 1,
        Width::Half => 2,
        Width::Word => 4,
    }
}

/// A load or store whose address is a constant in RAM: its offset in
/// `Game::ram` (aligned as the access aligns it) and width in bytes.
fn const_access(insts: &[Inst], op: &Op) -> Option<(u32, u32)> {
    let (Op::Load { width, addr } | Op::Store { width, addr, .. }) = *op else {
        return None;
    };
    let w = bytes(width);
    Some((globals::ram_at(konst(insts, addr)? & !(w - 1))?, w))
}

/// A load or store whose address isn't a constant in its block but which
/// the value sets bound to RAM (`analysis::vsa::func::Access`, by the
/// instruction at `at`): the offsets in `Game::ram` of its lowest and
/// highest address (aligned as the access aligns them, in one mirror) and
/// its width in bytes. A whole instruction's accesses share a bound (an
/// `ldm`'s words).
fn computed_ram(ctx: &Ctx, insts: &[Inst], op: &Op, at: u32) -> Option<(u32, u32, u32)> {
    let (Op::Load { width, addr } | Op::Store { width, addr, .. }) = *op else {
        return None;
    };
    if konst(insts, addr).is_some() {
        return None;
    }
    let x = ctx.disc.rec.accesses.get(&at)?;
    let (lo, hi) = x.hull.filter(|_| !x.sp && !x.top)?;
    if metrics::region_of(lo, hi) != Some(0) {
        return None;
    }
    let w = bytes(width);
    let (lo, hi) = (lo & !(w - 1), hi & !(w - 1));
    let (o_lo, o_hi) = (globals::ram_at(lo)?, globals::ram_at(hi)?);
    (o_hi >= o_lo && o_hi - o_lo == hi - lo).then_some((o_lo, o_hi, w))
}

/// The values of a block's address arithmetic (constants and the ops
/// [`konst`] folds) that nothing uses but the accesses `typed` prints as
/// fields, which don't print their address; and the loads from constant
/// ROM addresses nothing uses (a literal pool's, whose word the IR has as
/// a `Pool` constant), with their addresses.
fn dead_addresses(insts: &[Inst], typed: &[Option<Place>]) -> Vec<bool> {
    let mut uses = vec![0u32; insts.len()];
    for (i, inst) in insts.iter().enumerate() {
        let skip = match inst.op {
            Op::Load { addr, .. } | Op::Store { addr, .. } if typed[i].is_some() => Some(addr),
            _ => None,
        };
        let mut ops = inst.op.operands();
        if let Some(a) = skip {
            let k = ops
                .iter()
                .position(|&o| o == a)
                .expect("the access's address");
            ops.remove(k);
        }
        for o in ops.into_iter().chain(inst.guard) {
            uses[o.index()] += 1;
        }
    }
    let mut dead = vec![false; insts.len()];
    for i in (0..insts.len()).rev() {
        let folds = matches!(
            insts[i].op,
            Op::Const(_) | Op::Pool(_) | Op::Bin(..) | Op::Not(_)
        ) && konst(insts, ValueId(i as u32)).is_some();
        // A literal pool's load whose word the IR has as a `Pool` constant:
        // ROM reads change nothing.
        let pool = match insts[i].op {
            Op::Load { addr, .. } => konst(insts, addr).is_some_and(|a| metrics::region(a) == 2),
            _ => false,
        };
        if (folds && insts[i].guard.is_none() || pool) && uses[i] == 0 {
            dead[i] = true;
            for o in insts[i].op.operands() {
                uses[o.index()] -= 1;
            }
        }
    }
    dead
}

/// The callee-set run ([`fnref`]) over one code context, the ROM's,
/// without a lift: the flow, and each indirect call's complete callee set
/// ([`complete_sets`]), for [`crate::saves::summarize_sets`]. `sums` are
/// the summaries the frames need ([`frame::of_code`]); `gbaa-recompile
/// --summaries` runs it on Emerald. The lift runs it over every context
/// with the stand-ins' code, so its sets can differ.
pub fn callee_sets(
    rom: &[u8],
    disc: &Discovery,
    ctx: &Ctx,
    funcs: &[Func],
    sums: &Summaries,
) -> (fnref::Flow, Sets) {
    let frames = frame::of_code(ctx, funcs, sums);
    let all: BTreeSet<u32> = funcs.iter().map(|f| f.entry).collect();
    let overlays = discovery::overlays(rom, disc);
    let ins = [FlowIn {
        ctx,
        src: 0x0800_0000,
        funcs,
        frames: &frames,
    }];
    let (ins, pools, _) = fn_ins(&ins, &all);
    let (flow, _) = fn_flow(rom, disc, &overlays, &ins, &pools, &all);
    let sets = complete_sets(&flow);
    (flow, sets)
}

/// Each indirect call's complete callee set, from the flow: the calls (not
/// the jumps by value) whose target can hold no other function. A flow
/// that didn't settle (a step cap, the rounds run out) may be missing
/// callees, so it gives no sets, as it proves no words.
fn complete_sets(flow: &fnref::Flow) -> Sets {
    if flow.unsettled {
        return Sets::new();
    }
    flow.sites
        .iter()
        .filter(|(_, s)| s.complete && !s.tail)
        .map(|(&a, s)| (a, s.callees.clone()))
        .collect()
}

/// A `call_fn` call to key `k` passing the registers of `pass` (0 for the
/// rest) and taking back those of `back` (bits for r0-r3).
fn call_fn_text(k: &str, pass: u8, back: u8) -> String {
    let a: Vec<String> = (0..4)
        .map(|r| {
            if pass & 1 << r != 0 {
                format!("r{r}")
            } else {
                "0".into()
            }
        })
        .collect();
    let call = format!("call_fn(g, {k}, r13, [{}])?", a.join(", "));
    if back == 0xf {
        return format!("[r0, r1, r2, r3] = {call};");
    }
    let set: Vec<String> = shape::regs(back)
        .map(|r| format!(" r{r} = o[{r}];"))
        .collect();
    format!("{{ let o = {call};{} }}", set.concat())
}

/// [`call_fn_args`] for one callee set: what the call passes and takes
/// back, given each callee's shape (`None`: four registers).
fn site_args(cs: &BTreeSet<u32>, shape: impl Fn(u32) -> Option<Shape>) -> (u8, u8) {
    let (mut reads, mut back, mut kept) = (0u8, 0u8, 0u8);
    for &c in cs {
        let (p, r) = shape(c).map_or((0xf, 0xf), |sh| (sh.params, sh.results));
        reads |= p;
        back |= r;
        kept |= !r & 0xf;
    }
    (reads | (back & kept), back)
}

struct Args {
    by_site: BTreeMap<u32, (u8, u8)>,
    /// The sites whose callees' summaries (holes or not) read all four
    /// between them, and those with a four-register callee.
    union_four: usize,
    four_reg_callee: usize,
}

/// What each indirect call with a complete callee set passes `call_fn`
/// and takes back from it (bits for r0-r3), by its address. A shaped
/// callee's adapter reads its parameters and gives back its results and
/// the rest as they went in; a four-register one (a summary hole, a
/// stand-in) reads and gives back all four. So the call takes back what
/// any callee may change, and passes what any reads, and what one may
/// change while another leaves it as it came (that one gives back what
/// was passed).
fn call_fn_args(sets: &Sets, codes: &[Code], shaped: &BTreeMap<u32, Shape>) -> Args {
    let sums: BTreeMap<u32, &Summary> = codes
        .iter()
        .flat_map(|c| c.sums.by_entry.iter().map(|(&k, s)| (k, s)))
        .collect();
    let mut a = Args {
        by_site: BTreeMap::new(),
        union_four: 0,
        four_reg_callee: 0,
    };
    for (&at, cs) in sets {
        let raw = cs
            .iter()
            .fold(0, |m, c| m | sums.get(c).map_or(0xf, |s| s.params));
        a.union_four += usize::from(raw == 0xf);
        a.four_reg_callee += usize::from(cs.iter().any(|c| !shaped.contains_key(c)));
        a.by_site
            .insert(at, site_args(cs, |c| shaped.get(&c).copied()));
    }
    a
}

/// Every function's shape ([`shape::shapes`]), over all code contexts.
fn shapes_of(codes: &[Code], all: &BTreeSet<u32>, holes: &Holes) -> Shapes {
    let mut nodes = BTreeMap::new();
    for (i, c) in codes.iter().enumerate() {
        let edges = |f: &Func| {
            shape::edges(&c.ctx, f, all, |at, thumb, target| {
                dyn_targets(&c.ctx, holes, at, thumb, target).unwrap_or_default()
            })
        };
        nodes.extend(shape::nodes_of(i, &c.funcs, &c.sums, edges, |k| {
            holes.standins.contains_key(&k)
        }));
    }
    shape::shapes(&nodes)
}

/// The known targets of a jump by value at `at` (not a call): its own
/// (`target`), a resolved table's cases, the constant RAM address of a
/// jump into a copy's code ([`JumpRes::Ram`]: a function of an overlay,
/// or a hole where the jump goes), the holes file's for one Recovery
/// didn't resolve. `Err` with the jump's address where it's unresolved
/// and the holes file gives none.
fn dyn_targets(
    ctx: &Ctx,
    holes: &Holes,
    at: u32,
    thumb: bool,
    target: Option<u32>,
) -> Result<Vec<u32>, u32> {
    let jumps = &ctx.disc.rec.jumps;
    let mut targets: Vec<u32> = target.into_iter().collect();
    let jump_at = [at, at.wrapping_sub(2)]
        .into_iter()
        .find(|a| jumps.contains_key(a));
    match jump_at.map(|a| (a, &jumps[&a])) {
        Some((_, JumpRes::Resolved(_))) => {
            targets.extend(ctx.table_targets(at, thumb).unwrap_or_default());
        }
        Some((a, JumpRes::Ram(t))) => match holes.jumps.get(&a) {
            Some((ts, _)) => targets.extend(ts),
            None => targets.push(*t),
        },
        Some((a, JumpRes::Unresolved | JumpRes::Rejected(_))) => match holes.jumps.get(&a) {
            Some((ts, _)) => targets.extend(ts),
            None => return Err(a),
        },
        None => {}
    }
    targets.sort_unstable();
    targets.dedup();
    Ok(targets)
}

/// The plain four-register entry of `k`, as `FNS` holds it: a shaped
/// function's adapter.
fn plain(shapes: &BTreeMap<u32, Shape>, k: u32) -> String {
    if shapes.contains_key(&k) {
        format!("a_{k:08x}")
    } else {
        format!("f_{k:08x}")
    }
}

/// A function's return type: all of r0-r3 (`None`), or its results.
fn ret_ty(s: Option<Shape>) -> String {
    match s.map(|s| s.results.count_ones()) {
        None => "[u32; 4]".into(),
        Some(0) => "()".into(),
        Some(1) => "u32".into(),
        Some(n) => format!("({})", vec!["u32"; n as usize].join(", ")),
    }
}

/// Its return value, from the registers.
fn ret_expr(s: Option<Shape>) -> String {
    match s {
        None => "[r0, r1, r2, r3]".into(),
        Some(s) => tuple(s.results),
    }
}

/// The registers of `m` as a value: `()`, `r0`, `(r0, r1)`.
fn tuple(m: u8) -> String {
    let rs: Vec<String> = shape::regs(m).map(|r| format!("r{r}")).collect();
    match rs.len() {
        1 => rs[0].clone(),
        _ => format!("({})", rs.join(", ")),
    }
}

/// What a call's results are assigned to: nothing, `r0 = `, `(r0, r1) = `.
fn lhs(m: u8) -> String {
    if m == 0 {
        String::new()
    } else {
        format!("{} = ", tuple(m))
    }
}

/// A call's arguments after `sp`: `, r0, r2`.
fn args(m: u8) -> String {
    shape::regs(m).map(|r| format!(", r{r}")).collect()
}

/// `{r0, r1}`.
fn reg_list(m: u16) -> String {
    (0..4)
        .filter(|r| m & 1 << r != 0)
        .map(|r| format!("r{r}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `op` is printed as a `let` of a value and does nothing else, so
/// it needn't be printed where nothing uses it.
fn pure(op: &Op) -> bool {
    !matches!(
        op,
        Op::Insn(_)
            | Op::Timing(_)
            | Op::YieldFlag(..)
            | Op::SetReg(..)
            | Op::SetFlag(..)
            | Op::GetUserReg(_)
            | Op::SetUserReg(..)
            | Op::GetCpsr
            | Op::GetSpsr
            | Op::SetCpsr { .. }
            | Op::SetSpsr { .. }
            | Op::Load { .. }
            | Op::Store { .. }
            | Op::Exit(_)
    )
}

/// Byte `d` of stack local `l`'s frame bytes: `sp0` less how far below
/// it the byte is.
fn local_addr(l: &Local, d: u32) -> String {
    format!("sp0.wrapping_sub({:#x})", (-l.lo) as u32 - d)
}

/// A load of `n` bytes at byte `d` of stack local `l`, as a `u32`.
fn local_read(l: &Local, d: u32, n: u8) -> String {
    let name = l.name();
    if l.word {
        return name;
    }
    let b = |j: u32| format!("{name}[{}]", d + j);
    match n {
        1 => format!("{} as u32", b(0)),
        2 => format!("u16::from_le_bytes([{}, {}]) as u32", b(0), b(1)),
        _ => format!(
            "u32::from_le_bytes([{}, {}, {}, {}])",
            b(0),
            b(1),
            b(2),
            b(3)
        ),
    }
}

/// A store of `val`'s low `n` bytes at byte `d` of stack local `l`.
fn local_write(l: &Local, d: u32, n: u8, val: &str) -> String {
    let name = l.name();
    match (l.word, n) {
        (true, _) => format!("{name} = {val};"),
        (_, 1) => format!("{name}[{d}] = ({val}) as u8;"),
        (_, 2) => format!(
            "{name}[{d}..{}].copy_from_slice(&(({val}) as u16).to_le_bytes());",
            d + 2
        ),
        _ => format!(
            "{name}[{d}..{}].copy_from_slice(&({val}).to_le_bytes());",
            d + 4
        ),
    }
}

/// The value of `x` if it is a constant within its block: a literal, a
/// pool word, or arithmetic on them.
fn konst(insts: &[Inst], x: ValueId) -> Option<u32> {
    match insts[x.index()].op {
        Op::Const(c) | Op::Pool(c) => Some(c),
        Op::Bin(o, a, b) => {
            let (a, b) = (konst(insts, a)?, konst(insts, b)?);
            Some(match o {
                BinOp::Add => a.wrapping_add(b),
                BinOp::Sub => a.wrapping_sub(b),
                BinOp::Mul => a.wrapping_mul(b),
                BinOp::And => a & b,
                BinOp::Or => a | b,
                BinOp::Xor => a ^ b,
            })
        }
        Op::Not(a) => konst(insts, a).map(|a| !a),
        _ => None,
    }
}

/// A pure op as a Rust expression over earlier values.
fn expr(op: &Op) -> String {
    use Op::*;
    match *op {
        Const(c) | Pool(c) => format!("{c:#x}u32"),
        Bit(b) => format!("{b}"),
        Bin(o, a, b) => {
            let (a, b) = (v(a), v(b));
            match o {
                BinOp::Add => format!("{a}.wrapping_add({b})"),
                BinOp::Sub => format!("{a}.wrapping_sub({b})"),
                BinOp::Mul => format!("{a}.wrapping_mul({b})"),
                BinOp::And => format!("{a} & {b}"),
                BinOp::Or => format!("{a} | {b}"),
                BinOp::Xor => format!("{a} ^ {b}"),
            }
        }
        Not(a) | BNot(a) => format!("!{}", v(a)),
        MulLongLo { signed, a, b } => format!("prim::mul_long({signed}, {}, {}).0", v(a), v(b)),
        MulLongHi { signed, a, b } => format!("prim::mul_long({signed}, {}, {}).1", v(a), v(b)),
        Adc { a, b, c } => format!("prim::add_with_carry({}, {}, {}).0", v(a), v(b), v(c)),
        AdcCarry { a, b, c } => format!("prim::add_with_carry({}, {}, {}).1", v(a), v(b), v(c)),
        AdcOverflow { a, b, c } => {
            format!("prim::add_with_carry({}, {}, {}).2", v(a), v(b), v(c))
        }
        MulCarry {
            kind,
            a,
            b,
            acc_lo,
            acc_hi,
        } => format!(
            "prim::mul_carry(MulKind::{kind:?}, {}, {}, ({} as u64) << 32 | {} as u64)",
            v(a),
            v(b),
            v(acc_hi),
            v(acc_lo)
        ),
        Shift { kind, v: x, amount } => format!(
            "prim::shift(ShiftType::{}, {}, {}, false).0",
            shift(kind),
            v(x),
            v(amount)
        ),
        ShiftCarry {
            kind,
            v: x,
            amount,
            carry_in,
        } => format!(
            "prim::shift(ShiftType::{}, {}, {}, {}).1",
            shift(kind),
            v(x),
            v(amount),
            v(carry_in)
        ),
        Rrx { v: x, carry_in } => format!("prim::rrx({}, {}).0", v(x), v(carry_in)),
        Sext { v: x, bits } => format!("prim::sext({}, {bits})", v(x)),
        Select { c, a, b } | BSelect { c, a, b } => {
            format!("if {} {{ {} }} else {{ {} }}", v(c), v(a), v(b))
        }
        BitToWord(b) => format!("{} as u32", v(b)),
        TestBit { v: x, n } => format!("({} >> {n}) & 1 != 0", v(x)),
        IsZero(x) => format!("{} == 0", v(x)),
        BBin(o, a, b) => {
            let (a, b) = (v(a), v(b));
            match o {
                BitOp::And => format!("{a} & {b}"),
                BitOp::Or => format!("{a} | {b}"),
                BitOp::Xor => format!("{a} ^ {b}"),
            }
        }
        _ => unreachable!("not a pure op: {op:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flow_that_did_not_settle_gives_no_callee_sets() {
        let mut flow = fnref::Flow::default();
        flow.sites.insert(
            0x0800_0100,
            fnref::Site {
                callees: [0x0800_0201].into(),
                complete: true,
                tail: false,
            },
        );
        assert_eq!(complete_sets(&flow).len(), 1);
        flow.unsettled = true;
        assert!(complete_sets(&flow).is_empty());
    }

    #[test]
    fn a_complete_callee_set_passes_what_its_callees_read() {
        let sh = |params, results| Some(Shape { params, results });
        // f reads r0 and gives r0; g reads r1 and gives r0 and r2. The
        // call passes r0, r1, and r2, which f leaves as it came while g
        // changes it; it takes back r0 and r2, and r3 stays the caller's.
        let shapes = |c: u32| match c {
            1 => sh(0b0001, 0b0001),
            2 => sh(0b0010, 0b0101),
            _ => None,
        };
        assert_eq!(site_args(&[1, 2].into(), shapes), (0b0111, 0b0101));
        // Every callee gives r0 back: it isn't passed for that.
        assert_eq!(site_args(&[1].into(), shapes), (0b0001, 0b0001));
        // A four-register callee: all four, both ways.
        assert_eq!(site_args(&[1, 3].into(), shapes), (0xf, 0xf));
        assert_eq!(
            call_fn_text("k", 0b0011, 0b0001),
            "{ let o = call_fn(g, k, r13, [r0, r1, 0, 0])?; r0 = o[0]; }"
        );
        assert_eq!(
            call_fn_text("k", 0xf, 0xf),
            "[r0, r1, r2, r3] = call_fn(g, k, r13, [r0, r1, r2, r3])?;"
        );
    }

    #[test]
    fn holes_file_parses_standins_and_jumps() {
        let h = Holes::parse(
            r#"
[[standin]]
entry = 0x03000000
why = "the IRQ wrapper"
code = '''
pub fn f_03000000() {}
'''

[[jump]]
at = 0x08001234
targets = [0x08001240, 0x08001251]
why = "a table"
"#,
        )
        .unwrap();
        assert_eq!(h.standins[&0x0300_0000].1, "the IRQ wrapper");
        assert!(h.standins[&0x0300_0000].0.contains("f_03000000"));
        assert!(h.lr.is_empty());
        assert_eq!(h.jumps[&0x0800_1234].0, vec![0x0800_1240, 0x0800_1251]);
        let yield_ = "[[standin]]\nentry = 0x080001a1\nwhy = \"a yield\"\ncode = \"\"\nlr = ";
        let h = Holes::parse(&format!("{yield_}true\n")).unwrap();
        assert_eq!(h.lr, BTreeSet::from([0x0800_01a1]));
        assert!(Holes::parse(&format!("{yield_}1\n")).is_err());
        let h = Holes::parse(
            "[[copy]]\nsrc = 0x08000104\ndst = 0x03000c7c\nlen = 0x230\nroots = [0x03000c7c, 0x03000d84]\nwhy = \"seen\"\n",
        )
        .unwrap();
        assert_eq!(
            h.copies_seen,
            vec![SeenCopy {
                src: 0x0800_0104,
                dst: 0x0300_0c7c,
                len: 0x230,
                roots: vec![0x0300_0c7c, 0x0300_0d84],
                why: "seen".into(),
            }]
        );
        assert!(Holes::parse("[[copy]]\nsrc = 0x08000104\nwhy = \"x\"\n").is_err());
        assert!(Holes::parse("[[nothing]]\nx = 1\n").is_err());
        assert!(Holes::parse("[[standin]]\nwhy = \"no entry\"\n").is_err());
        assert_eq!(h.skip_fn_flow, None);
        let h = Holes::parse("skip_fn_flow = \"too slow\"\n").unwrap();
        assert_eq!(h.skip_fn_flow.as_deref(), Some("too slow"));
        assert!(Holes::parse("skip_fn_flow = true\n").is_err());
    }

    #[test]
    fn pure_ops_print_as_rust() {
        let v = ValueId;
        assert_eq!(expr(&Op::Const(0x1234)), "0x1234u32");
        assert_eq!(
            expr(&Op::Bin(BinOp::Sub, v(1), v(2))),
            "v1.wrapping_sub(v2)"
        );
        assert_eq!(expr(&Op::TestBit { v: v(3), n: 31 }), "(v3 >> 31) & 1 != 0");
        assert_eq!(
            expr(&Op::Shift {
                kind: ShiftType::Asr,
                v: v(4),
                amount: v(5)
            }),
            "prim::shift(ShiftType::Asr, v4, v5, false).0"
        );
    }
}
