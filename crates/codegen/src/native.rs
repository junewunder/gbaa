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
//!    block loop, and `unpack`: the extended entry's registers its code
//!    reads as they came); `dead_writes` (liveness: the writes left out,
//!    and those only their block reads, printed as `let`s that `reg`
//!    reads); `block` (a block's IR as statements: memory accesses,
//!    typed fields via `typed_as`, `bank_sp(g, r13)` before I/O stores
//!    but where the bank already holds it (`redundant_banks`); `count`
//!    tallies each access for the metrics; `used` drops what nothing
//!    printed needs); stack locals (`local`, `declare_locals`, `flush`);
//!    control flow (`goto`, `fall_into`, `call`, `call_then`, `tail`,
//!    `ret`, and `exit`: branches, calls by value, `swi`, idle and spin
//!    loops).
//! 5. Free helpers: `dyn_targets` (a jump by value's known targets);
//!    signatures from a [`Shape`] (`plain`, `ret_ty`, `tuple`,
//!    `lhs`, `args`); locals' text (`local_addr`, `local_read`,
//!    `local_write`); IR to Rust (`expr`, `konst`, `pure`,
//!    `dead_addresses`, `v`, `flag`, `shift`). Then the tests.
//!
//! Elsewhere: what each function's parameters and results are,
//! [`shape`]; frames, escaping slots and which become locals, [`frame`];
//! function addresses and callee sets, [`fnref`]; RAM's globals, `pin`
//! and `ix`, both layouts, [`globals`]; heap pointers as `Ref<T>`, the
//! prelude's `Game::at`, [`heap`]; accesses as the names files' named
//! fields, [`fields`]; metric names and bars,
//! [`metrics`]; the crate's files, [`modules`]; functions and data by the
//! names file's names, [`named`]; `lift-check.toml`'s
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
//!   A write of one nothing reads after it is left out ([`live`], unit
//!   `native/lift-dead-writes`: a return reads its results, a call its
//!   parameters, a fall into another function everything). A pure value
//!   used once, or a constant, prints inside its use where nothing
//!   between writes what it reads (unit `native/lift-inline-temps`).
//!   r4-r11 outlive a call because callees keep them, or change only
//!   what no caller reads (the summaries' `keeps` and `changes`; a
//!   function that breaks that, `analysis::holes::SUMMARY_HARD`, is a
//!   hard hole).
//! - Two entries: `x_<key>(g, Regs)` takes every register and flag, and
//!   `f_<key>` above is a call into it with r4-r12 and the flags zero and
//!   `lr` [`RET`]. A fall or jump into another function's entry (or a
//!   call's return into one) runs on in its `x_` with this one's registers
//!   and flags, as the hardware does (unit `native/lift-fallthrough`).
//! - A function that reads one of r4-r11 as it came (a parameter by
//!   register: hand-written code's, or agbcc's word built in an unset
//!   register and stored) takes it from its callers
//!   ([`crate::saves::Summary::takes`], unit `native/metroid-entry-reads`):
//!   a direct call is its `x_` with r0-r3 and the caller's own of those
//!   registers ([`taking`]), and a caller that still has one as it came
//!   takes it in turn. A pointer call can't (`call_fn` passes r0-r3): the
//!   read its zeroes reach is a hard hole unless the holes file names the
//!   indirect calls that reach the function (`[[takes]]`,
//!   [`Holes::takes`]), which then pass it theirs (none, `sites = []`,
//!   for code only jumped into: a jump passes every register). Its `f_`
//!   stops the program, so no call runs it on zeroes. A RAM copy's
//!   function whose key the ROM holds only as a constant jump's literal
//!   (m4a's mixer, from `SoundMain`'s `bx r3`) needs no `[[takes]]`: no
//!   pointer reaches it ([`jump_literals`]), and a jumper that hands on a
//!   register it takes as it came is a hard hole (unit
//!   `native/ram-copy-jump-only`). A context save
//!   (`setjmp`'s `stm`, `saves::summary`'s `context_saves`) is no read.
//! - A function's blocks are printed from the structuring plan
//!   ([`crate::plan`], as the mod view's structured codegen has it; unit
//!   `native/lift-structured`): labeled `loop`s and blocks, a jump to a
//!   block a `continue`, a `break`, a fall-through or the block's code
//!   inline, an irreducible region a `loop { match b }` of its own.
//!   Joins print as the code after an `if`, a `match` or a loop (unit
//!   `native/lift-join-shape`): a labelled block nothing breaks out of
//!   prints bare; a `break` to where falling off goes is left out (an
//!   arm's last), takes a loop's label (out of the loop), or makes the
//!   guarded exit an `if` whose `else` is the rest of its block; arms of
//!   the same code are one; a labelled block whose body falls off at one
//!   arm alone takes the code after it into that arm; a compare tree's
//!   later blocks print nothing in the arms (else they print once, as a
//!   `match` of their own). A function the plan doesn't take (a block no printed jump reaches, one
//!   nested too deep) stays a whole `loop { match b }` over local labels
//!   (0 the entry). Each block starts with `runtime::guest_pc::set` of its
//!   guest address, which a stop names (unit `native/first-stop-guest-pc`:
//!   compiled code keeps no other pc); calls are Rust calls; a return is a jump to the value
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
//! - An idle loop's back edge calls `hw::idle`, its way out (the branch
//!   not taken, an exit of its own: `Ctx::with_idle_exits`)
//!   `hw::idle_exit`, a spin loop's back edge `hw::spin` (`analysis::idle`,
//!   `runtime::spins`, as the play build's `--timing native` marks them).
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
//! - CPU state: `mrs` and `msr` outside the reset and the IRQ wrappers
//!   (theirs are printed: [`PsrFns`]), user-bank transfers, exception
//!   returns, stack switches ([`Frame::switches`]: a task switcher's, which a stand-in
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

use crate::func::{Ctx, ExitInfo, Func, Idle, Plan, build_func, exits, key, successors};
use crate::plan;
use crate::saves::{Assume, Passing, Sets, Summaries, Summary, summarize_passing};
use analysis::discovery::{self, Discovery};
use analysis::ir::{BinOp, BitOp, Exit, Inst, Op, ValueId};
use analysis::vsa::func::JumpRes;
use armv4t::sem::{Flag, PsrMask, ShiftType, Width};
use frame::{Frame, Kind, Local};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub mod banks;
pub mod cond;
pub mod counted;
pub mod fields;
pub mod first_stop;
pub mod fnref;
pub mod frame;
pub mod globals;
pub mod heap;
pub mod lift_check;
pub mod live;
pub mod metrics;
pub mod modules;
pub mod named;
pub mod narrow;
pub mod shape;
pub mod switch;
pub mod unread;

use globals::{Globals, Layout, Obj, Place};
use metrics::{Metrics, Where};
use shape::{Shape, Shapes};

/// The prelude every lifted crate starts with, at the guest layout.
pub const PRELUDE: &str = include_str!("native/prelude.rs");

/// The per-function check's driver (`gbaa-lift --fn-check`), written into
/// a checked lift as `src/fn_check.rs`.
pub const FN_CHECK_DRIVER: &str = include_str!("native/fn_check.rs");

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
    /// By function key: the indirect calls (their addresses) that reach a
    /// function reading one of r4-r11 as it came, and why (`[[takes]]
    /// entry = 0x…, sites = [0x…], why = "…"`): each passes it the
    /// caller's ([`crate::saves::Passing`]), and its plain entry stops the
    /// program, so a pointer call the file doesn't name can't run it on
    /// zeroes. With no sites (`sites = []`) no pointer call reaches it:
    /// code only jumped into, which gets the jumper's registers.
    pub takes: BTreeMap<u32, (Vec<u32>, String)>,
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
    /// RAM the game keeps stacks in outside IWRAM (`[[stack]] start =
    /// 0x…, end = 0x…, why = "…"`): a task switcher's stacks (Kirby -
    /// Nightmare's in EWRAM). The lift doesn't use them; the native check
    /// (`route-check --native`) skips them as it skips IWRAM's stack, and
    /// fails unless every `sp` outside IWRAM the play build's interpreter
    /// saw lies in one (`start..=end`, an empty stack's `sp` its end).
    pub stacks: Vec<SeenStack>,
    /// A check's planted wrong store (`gbaa-lift --plant AT`): the store
    /// of the instruction at this address stores its value plus one, which
    /// the native check must catch.
    pub plant: Option<u32>,
    /// `--plant`'s store in a checked function's reference too
    /// (`gbaa-lift --plant-reference`): a planted wrong lift, which only
    /// the interpreter, the check's second reference, can catch.
    pub plant_reference: bool,
    /// The per-function check runs the interpreter as a second reference
    /// every this many compared runs (`gbaa-lift --interp-every N`; 0,
    /// never; unit `native/fn-check-interp`).
    pub fn_check_interp: u32,
    /// Stage A's old fall into another function (`gbaa-lift
    /// --fall-as-tail-call`): a tail call through its plain entry, which
    /// starts it with r4-r12 and the flags zeroed. The fallthrough check's
    /// planted control (`native/lift-fallthrough`): the native check must
    /// catch it on `agb/native-fallthrough`.
    pub fall_as_tail_call: bool,
    /// Stage A's old dead writes (`gbaa-lift --keep-dead-writes`): every
    /// write of a register or flag printed, read or not, those nothing
    /// reads counted in `dead_flag_writes` (`native/lift-dead-writes`).
    pub keep_dead_writes: bool,
    /// Stage A's old temporaries (`gbaa-lift --keep-temps`): every IR
    /// value its own `let`, constants included, none printed inside its
    /// use (`native/lift-inline-temps`).
    pub keep_temps: bool,
    /// Stage A's old counted loops (`gbaa-lift --no-for-loops`): every
    /// loop a `loop`, none a `for` (`native/lift-for-loops`).
    pub no_for_loops: bool,
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
    /// Stage A's old call of a function that takes r4-r11 from its
    /// callers (`gbaa-lift --takes-zeroed`): each call that passes them
    /// (a direct call, a `[[takes]]` site) passes zeroes instead, as the
    /// plain entry did. The taker check's planted control
    /// (`native/taker-test-program`): the native check must catch it on
    /// `agb/native-takes`.
    pub takes_zeroed: bool,
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
    /// Function names by key, from the same files: each lifted function's
    /// entry is printed so, and a constant address in a named datum as
    /// its name ([`named`]; unit `native/lift-fn-names`). Not with
    /// `--copies`, whose copies would repeat them.
    pub fn_names: BTreeMap<u32, String>,
    /// The same files' types (`codegen::types`), when they give any: the
    /// report counts the constant RAM accesses that land on a named field
    /// of a typed global ([`typed_fields`]; unit
    /// `native/mirror-katam-names`).
    pub types: Option<std::sync::Arc<crate::types::Types>>,
    /// The same files' types, prototypes and DWARF variables: accesses
    /// through a pointer they type print as its named fields ([`fields`];
    /// unit `native/lift-typed-access`). `None`: the files give no types,
    /// or `--no-field-types`.
    pub fields: Option<std::sync::Arc<fields::Maps>>,
    /// Where `Ram`'s globals are (`gbaa-lift --layout`): the guest's
    /// offsets, or the play layout's order ([`globals::Layout`]).
    pub layout: Layout,
    /// Why the function addresses aren't followed (`skip_fn_flow = "…"`
    /// in the holes file): [`fnref::follow`] only feeds the report and
    /// stage B's metrics, and on a game it doesn't settle in a session
    /// (Metroid: 68 s a round, rounds growing). `None`: followed.
    pub skip_fn_flow: Option<String>,
    /// Find the hard holes only (`gbaa-lift --holes-only`; the hole
    /// report's lift, native/holes-match-lift): no heap pointers, no
    /// modules, and unless [`Holes::holes_flow`], no function-address
    /// flow. The heap changes no hole. The flow's complete callee sets
    /// can: the second summaries take each such indirect call as calling
    /// exactly those, so a summary hole may rest on one (one each in
    /// Minish Cap and Kirby & the Amazing Mirror, of the corpus). Without
    /// it this is the lift's [`Lifted::holes`] but those, in seconds to a
    /// few minutes; [`Lifted::files`] is empty and the report one line.
    pub holes_only: bool,
    /// The per-function check (`gbaa-lift --fn-check KEY`, unit
    /// `native/fn-check-any`): the functions checked, by key. Each keeps
    /// its replacement (its stand-in; with none, the lift itself, which a
    /// `--plant` in it makes wrong) and gets the lift printed again as the
    /// reference, `ref_f_<key>`; its `f_<key>` runs the check's driver
    /// (`native/fn_check.rs`, the crate's `fn_check.rs`), whose `main`
    /// runs the program headless under the checks.
    pub fn_check: BTreeMap<u32, FnCheck>,
    /// The budgets the holes file records for its stand-ins (`check = {…}`),
    /// which `--fn-check` uses for a function it names.
    pub checks: BTreeMap<u32, FnCheck>,
    /// With [`Holes::holes_only`], follow the function addresses too
    /// (`--flow`): the whole lift's holes exactly, at the flow's cost (up
    /// to about 20 minutes a game).
    pub holes_flow: bool,
}

/// A checked function's budget (`[[standin]] check = { seed = …, runs =
/// …, digest = "…" }` in the holes file, or `gbaa-lift --fn-check`'s
/// defaults): the seed of its variants, the runs to compare, and the
/// digest of the reference's ends that seed and budget gave, if recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnCheck {
    pub seed: u64,
    pub runs: u32,
    pub digest: Option<u64>,
}

impl Default for FnCheck {
    fn default() -> FnCheck {
        FnCheck {
            seed: 1,
            runs: 2000,
            digest: None,
        }
    }
}

impl Holes {
    /// Parse a holes file: `[[standin]] entry = 0x…, why = "…", code =
    /// '''…'''` (and `lr = true` for [`Holes::lr`], `check = { seed, runs,
    /// digest }` for [`Holes::checks`]), `[[jump]] at = 0x…, targets = [0x…], why = "…"`,
    /// `[[takes]]` ([`Holes::takes`]), `[[copy]]` ([`Holes::copies_seen`]),
    /// `[[stack]]` ([`Holes::stacks`]), and at the top, `skip_fn_flow =
    /// "why"`.
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
                        if let Some(c) = e.get("check") {
                            let c = c.as_table().ok_or("standin: check: not a table")?;
                            let d = FnCheck::default();
                            let num = |k: &str| -> Result<Option<i64>, String> {
                                c.get(k)
                                    .map(|v| {
                                        v.as_integer()
                                            .ok_or(format!("standin: check: {k}: not an integer"))
                                    })
                                    .transpose()
                            };
                            let digest = match c.get("digest") {
                                None => None,
                                Some(v) => Some(
                                    v.as_str()
                                        .and_then(|t| u64::from_str_radix(t, 16).ok())
                                        .ok_or("standin: check: digest: not a hex string")?,
                                ),
                            };
                            h.checks.insert(
                                entry,
                                FnCheck {
                                    seed: num("seed")?.map_or(d.seed, |v| v as u64),
                                    runs: num("runs")?.map_or(d.runs, |v| v as u32),
                                    digest,
                                },
                            );
                        }
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
                    "takes" => {
                        let entry = int(e.get("entry").ok_or("takes: missing entry")?, "entry")?;
                        let sites = e
                            .get("sites")
                            .and_then(|v| v.as_array())
                            .ok_or("takes: missing sites")?
                            .iter()
                            .map(|v| int(v, "sites"))
                            .collect::<Result<Vec<_>, _>>()?;
                        if h.takes.insert(entry, (sites, str_of(e, "why")?)).is_some() {
                            return Err(format!("takes: {entry:#010x} twice"));
                        }
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
                    "stack" => {
                        let field = |f: &str| -> Result<u32, String> {
                            int(e.get(f).ok_or(format!("stack: missing {f}"))?, f)
                        };
                        let (start, end) = (field("start")?, field("end")?);
                        if start >= end {
                            return Err(format!("stack: {start:#010x} not below {end:#010x}"));
                        }
                        h.stacks.push(SeenStack {
                            start,
                            end,
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

/// One `[[stack]]` of the holes file ([`Holes::stacks`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenStack {
    pub start: u32,
    pub end: u32,
    pub why: String,
}

/// A hard hole: the function, the instruction, its kind, and what it is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hole {
    pub func: u32,
    pub at: u32,
    pub kind: HoleKind,
    pub what: String,
}

/// What kind of hard hole: the lift's count by kind, which the hole
/// report (`gbaa-holes`, native/holes-match-lift) prints as its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HoleKind {
    /// An indirect jump Recovery left unresolved and the holes file
    /// gives no targets for.
    Jump,
    /// A jump to ROM where discovery (and the gap decoding) found no code.
    NoCode,
    /// An undefined instruction that isn't a compiler's trap.
    Undefined,
    /// `msr`, `mrs`, a user-bank transfer, an exception return or a stack
    /// switch outside the reset and the IRQ wrappers.
    Cpu,
    /// A BIOS call the library doesn't have.
    Bios,
    /// A function whose summary has a hard hole
    /// (`analysis::holes::SUMMARY_HARD`), once.
    Summary,
    /// A call to a function that takes r4-r11 from its callers that
    /// can't pass them.
    Takes,
    /// The holes file names something that isn't there (a stand-in, a
    /// `[[takes]]`, a `[[copy]]`), or a check's plant missed.
    HolesFile,
}

impl HoleKind {
    pub const ALL: [HoleKind; 8] = [
        HoleKind::Jump,
        HoleKind::NoCode,
        HoleKind::Undefined,
        HoleKind::Cpu,
        HoleKind::Bios,
        HoleKind::Summary,
        HoleKind::Takes,
        HoleKind::HolesFile,
    ];

    /// The kind's short name: a column of the hole report's table.
    pub fn name(self) -> &'static str {
        match self {
            HoleKind::Jump => "jumps",
            HoleKind::NoCode => "no code",
            HoleKind::Undefined => "undefined",
            HoleKind::Cpu => "CPU state",
            HoleKind::Bios => "BIOS",
            HoleKind::Summary => "summaries",
            HoleKind::Takes => "r4-r11 not passed",
            HoleKind::HolesFile => "holes file",
        }
    }
}

/// `holes`' count of each kind, in [`HoleKind::ALL`]'s order.
pub fn holes_by_kind(holes: &[Hole]) -> [usize; 8] {
    let mut n = [0; 8];
    for h in holes {
        n[HoleKind::ALL.iter().position(|&k| k == h.kind).unwrap()] += 1;
    }
    n
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

/// The literals of `funcs`' constant jumps (not calls) to a RAM
/// address, by their address in `ctx`: a pc-relative load of the target
/// in the jump's block, which no store there writes anywhere. Such a word
/// is a way in by a jump, which passes every register, not a pointer a
/// call may take ([`Passing::pointed`]; m4a's `SoundMain`, `ldr r3, =…;
/// bx r3` into its IWRAM mixer). Returned with the bytes every other
/// pc-relative load in `funcs` reads: a compiler shares one pool entry
/// between all the loads of a constant in a function, so a literal one
/// of those loads also reads may be stored, passed or called through,
/// and is not only a jump's ([`literals_only_jumped`]).
fn jump_literals(ctx: &Ctx, funcs: &[Func]) -> (BTreeSet<u32>, BTreeSet<u32>) {
    // Each pc-relative load, by the instruction's address: the word it
    // reads, and whether some block has it as a qualifying jump's load.
    let mut loads: BTreeMap<u32, (u32, u32, bool)> = BTreeMap::new();
    for gb in funcs.iter().flat_map(|f| f.blocks.values()) {
        let b = &gb.block;
        let read = |a: u32, instr| {
            let insn = analysis::program::Insn {
                addr: a,
                thumb: b.thumb,
                size: if b.thumb { 2 } else { 4 },
                instr,
                bl: None,
            };
            analysis::vsa::func::pool_read(&insn)
        };
        for &(a, instr) in &gb.insns {
            if let Some((p, e)) = read(a, instr) {
                loads.entry(a).or_insert((p, e, false));
            }
        }
        for x in exits(ctx, gb) {
            let t = match x.plan {
                Plan::Static(t)
                | Plan::Dynamic {
                    ret: None,
                    target: Some(t),
                    ..
                } if x.ret.is_none() => t,
                _ => continue,
            };
            if !analysis::vsa::func::is_ram(t & !1) {
                continue;
            }
            let stored = b.insts.iter().any(
                |i| matches!(i.op, Op::Store { value, .. } if crate::func::fold(b, value) == Some(t)),
            );
            if stored {
                continue;
            }
            for &(a, instr) in &gb.insns {
                // A word's load, of the target.
                if let Some((p, e)) = read(a, instr)
                    && e.wrapping_sub(p) == 4
                    && p.checked_sub(ctx.base)
                        .is_some_and(|o| (o as usize) + 4 <= ctx.rom.len())
                    && ctx.read(p, 4) == t
                {
                    loads.insert(a, (p, e, true));
                }
            }
        }
    }
    let mut jumps = BTreeSet::new();
    let mut other = BTreeSet::new();
    for (p, e, jump) in loads.into_values() {
        if jump {
            jumps.insert(p);
        } else {
            let mut q = p;
            while q != e {
                other.insert(q);
                q = q.wrapping_add(1);
            }
        }
    }
    (jumps, other)
}

/// The jump literals ([`jump_literals`]' first set, ROM addresses) no
/// other load reads a byte of (its second): only those words are a jump's
/// alone, and dropped from the words a pointer may call through.
fn literals_only_jumped(jumps: BTreeSet<u32>, other: &BTreeSet<u32>) -> BTreeSet<u32> {
    jumps
        .into_iter()
        .filter(|&p| !(0..4).any(|i| other.contains(&p.wrapping_add(i))))
        .collect()
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
        let ctx = Ctx::new(rom, &disc)
            .with_library()
            .with_idle_exits()
            .with_jumps(given());
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
                kind: HoleKind::HolesFile,
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
    // The indirect calls the holes file's `[[takes]]` names: they pass
    // their callees the caller's r4-r11.
    let named_sites: BTreeSet<u32> = holes
        .takes
        .values()
        .flat_map(|(sites, _)| sites.iter().copied())
        .collect();
    let mut ctxs = vec![(
        Ctx::new(rom, &disc)
            .with_library()
            .with_idle_exits()
            .with_jumps(given()),
        0x0800_0000,
    )];
    for o in &overlays {
        let o_rom = &rom[(o.src - 0x0800_0000) as usize..][..o.len as usize];
        ctxs.push((
            Ctx::at(o.dst, o_rom, &o.disc)
                .with_library()
                .with_idle_exits()
                .with_jumps(given()),
            o.src,
        ));
    }
    let built: Vec<Vec<Func>> = ctxs
        .iter()
        .map(|(ctx, _)| ctx.entries.iter().map(|&e| build_func(ctx, e)).collect())
        .collect();
    // A RAM copy's functions a pointer may call: the ROM words that hold
    // a RAM address, but a constant jump's literal.
    let pointed = {
        let (mut jumps, mut other) = (BTreeSet::new(), BTreeSet::new());
        for ((ctx, src), funcs) in ctxs.iter().zip(&built) {
            let to_rom = |p: u32| p.wrapping_sub(ctx.base).wrapping_add(*src);
            let (j, o) = jump_literals(ctx, funcs);
            jumps.extend(j.into_iter().map(to_rom));
            other.extend(o.into_iter().map(to_rom));
        }
        let literals = literals_only_jumped(jumps, &other);
        rom.as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .map(|(i, &w)| (0x0800_0000 + 4 * i as u32, u32::from_le_bytes(w)))
            .filter(|&(p, w)| analysis::vsa::func::is_ram(w & !1) && !literals.contains(&p))
            .map(|(_, w)| w)
            .collect()
    };
    let first = Passing {
        pointed: Some(pointed),
        ..Passing::default()
    };
    let mut codes: Vec<Code> = ctxs
        .into_iter()
        .zip(built)
        .map(|((ctx, _), funcs)| code(ctx, funcs, &named_sites, &first))
        .collect();
    let holes = &library_standins(holes, &codes);
    // A function checked with no stand-in (`--fn-check`) is its own
    // replacement: a stand-in in name, so its callers take it as one.
    let synthetic: BTreeSet<u32> = holes
        .fn_check
        .keys()
        .copied()
        .filter(|k| !holes.standins.contains_key(k))
        .collect();
    let holes = &{
        let mut h = holes.clone();
        for &k in &synthetic {
            h.standins
                .insert(k, (String::new(), "the lift itself (--fn-check)".into()));
        }
        h
    };
    // Every function, by key, over all contexts.
    let mut all: BTreeSet<u32> = BTreeSet::new();
    for c in &codes {
        all.extend(c.funcs.iter().map(|f| f.entry));
    }
    // The reset (the ROM's entry) and the IRQ wrappers the code installs,
    // whose `msr` and `mrs` are printed ([`PsrFns`]).
    let psr_fns = PsrFns::of(&disc, &overlays);
    // RAM's globals: what the constant-address accesses of the functions
    // printed (a stand-in's code is its own) reach.
    // And what their computed ones reach, where the value sets bound it
    // (native/ref-in-object): one address is an access like a constant
    // one; a range, an object's extent.
    let mut ram = Vec::new();
    let mut konst = Vec::new();
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
                        konst.push((a, w));
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
    let typed_line = holes.types.as_deref().map(|t| typed_fields(t, &konst));
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
            sums: &c.sums,
        })
        .collect();
    let no_flow = holes.holes_only && !holes.holes_flow;
    let (ins, pools, call_fn) = if no_flow {
        Default::default()
    } else {
        fn_ins(&flow_ins, &all, &named_sites)
    };
    let fnrefs = match &holes.skip_fn_flow {
        None if !no_flow => {
            let (flow, words) = fn_flow(rom, &disc, &overlays, &ins, &pools, &all);
            Some((flow, words, call_fn))
        }
        _ => None,
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
    let heap = if holes.holes_only {
        heap::Heap::default()
    } else {
        heap::follow(
            &ins,
            settled.map_or(&no_sites, |(f, _, _)| &f.sites),
            taken.as_ref(),
            &ram_init,
            &global_from,
        )
    };
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
    // And with what the holes file says of r4-r11's passing: its
    // `[[takes]]`, and a stand-in takes nothing.
    let mut passing = Passing {
        replaced: holes.standins.keys().copied().collect(),
        ..first
    };
    for (&k, (sites, _)) in &holes.takes {
        passing.named.insert(k);
        for &at in sites {
            passing.sites.entry(at).or_default().insert(k);
        }
    }
    let replaced_takes = codes.iter().any(|c| {
        let takes = |k| c.sums.get(k).is_some_and(|s| s.takes != 0);
        passing.replaced.iter().any(|&k| takes(k))
    });
    if !sets.is_empty() || !passing.named.is_empty() || replaced_takes {
        for c in &mut codes {
            c.sums = summarize_passing(&c.ctx, &c.funcs, Assume::default(), &sets, &passing);
        }
    }
    let shapes = shapes_of(&codes, &all, holes);
    // A checked function's shape as if it weren't replaced: what its
    // callers would read, which the check compares (`--fn-check`).
    let check_shapes = (!holes.fn_check.is_empty()).then(|| {
        let mut h = holes.clone();
        h.standins.retain(|k, _| !holes.fn_check.contains_key(k));
        shapes_of(&codes, &all, &h).shaped
    });
    let args = call_fn_args(&sets, &codes, &shapes.shaped);
    // The functions that take r4-r11 from their callers, over all
    // contexts; a stand-in that calls one by its plain entry would run
    // into its stop.
    let takers: BTreeSet<u32> = codes
        .iter()
        .flat_map(|c| c.sums.by_entry.iter())
        .filter(|(k, s)| s.passed() != 0 && !holes.standins.contains_key(k))
        .map(|(&k, _)| k)
        .collect();
    let lr_ret = lr_entries(&codes, &all, holes);
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
            let check = holes.fn_check.get(&f.entry);
            if let Some((code, why)) = holes.standins.get(&f.entry)
                && check.is_none()
            {
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
            let print = |holes: &Holes, soft: &mut Soft| {
                let mut p = FnPrinter {
                    ctx: &c.ctx,
                    f,
                    all: &all,
                    sums: &c.sums,
                    takers: &takers,
                    shapes: &shapes.shaped,
                    me: shapes
                        .shaped
                        .get(&f.entry)
                        .copied()
                        .filter(|_| check.is_none()),
                    holes,
                    found: Vec::new(),
                    soft,
                    labels: f.blocks.keys().enumerate().map(|(i, &k)| (k, i)).collect(),
                    in_region: Vec::new(),
                    rest_from: 0,
                    unguard: Vec::new(),
                    blind: Vec::new(),
                    quiet: Vec::new(),
                    scrutinee: None,
                    region: None,
                    outs: None,
                    edges: None,
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
                    psr: psr_fns.place(f.entry),
                    starts: narrow::Starts::new(),
                    lr_ret: lr_ret.contains(&f.entry),
                    dead: BTreeMap::new(),
                    live_in: BTreeMap::new(),
                    local: BTreeMap::new(),
                    bound: Default::default(),
                    fors: BTreeMap::new(),
                    steps: BTreeMap::new(),
                    tail_for: false,
                    banked: BTreeMap::new(),
                    field_at: BTreeMap::new(),
                    inl: Vec::new(),
                    printed: BTreeSet::new(),
                    fall: None,
                    tail_edge: false,
                    loops: Vec::new(),
                    direct: None,
                    block_direct: None,
                    mark: None,
                    open_mark: None,
                };
                let text = p.print(c.sums.get(f.entry));
                (text, p.found)
            };
            let (text, found) = match check {
                None => print(holes, &mut soft),
                Some(chk) => {
                    let (t, found) = fn_check_texts(
                        f.entry,
                        chk,
                        holes,
                        &synthetic,
                        check_shapes.as_ref().and_then(|s| s.get(&f.entry)).copied(),
                        c.sums.get(f.entry),
                        &print,
                        &mut soft,
                    );
                    soft.standins += 1;
                    (t, found)
                }
            };
            if found.is_empty() {
                out.push((f.entry, text));
            } else {
                let mut t = String::new();
                for h in &found {
                    writeln!(
                        t,
                        "compile_error!(\"{}\");",
                        h.to_string().replace('"', "'")
                    )
                    .unwrap();
                }
                out.push((f.entry, t));
                hard.extend(found);
            }
        }
    }
    for k in holes.standins.keys().filter(|k| !all.contains(k)) {
        hard.push(Hole {
            func: *k,
            at: *k,
            kind: HoleKind::HolesFile,
            what: "the holes file's stand-in matches no function".into(),
        });
    }
    for (&f, (code, _)) in &holes.standins {
        for &k in takers
            .iter()
            .filter(|k| code.contains(&format!("f_{k:08x}(")))
        {
            hard.push(Hole {
                func: f,
                at: f,
                kind: HoleKind::HolesFile,
                what: format!("the stand-in calls f_{k:08x}, which takes r4-r11 from its callers and stops the program when called so"),
            });
        }
    }
    for (&k, (sites, _)) in &holes.takes {
        for &at in sites.iter().filter(|&&at| !soft.passes.contains(&(at, k))) {
            hard.push(Hole {
                func: k,
                at,
                kind: HoleKind::HolesFile,
                what: "the holes file's takes matches no indirect call (printed through call_fn) to a function that takes one of r4-r11 from its callers".into(),
            });
        }
        // One with no sites names a function only jumped into: stale if
        // the function takes nothing.
        if sites.is_empty() && !takers.contains(&k) {
            hard.push(Hole {
                func: k,
                at: k,
                kind: HoleKind::HolesFile,
                what: "the holes file's takes, with no sites, names a function that takes none of r4-r11 from its callers".into(),
            });
        }
    }
    // A jump into another context's function passes every register, but
    // its summary there can't see what this one hands on as it came
    // (`Summary::foreign`): a register the target reads at entry must be
    // the jumper's own (m4a's SoundMain sets r4, r5, r6 and r8 before its
    // jump into the IWRAM mixer), else it is the jumper's caller's, which
    // no summary passes.
    for c in &codes {
        for (&f, s) in &c.sums.by_entry {
            if holes.standins.contains_key(&f) {
                continue;
            }
            for (&k, &m) in &s.foreign {
                let Some(t) = codes
                    .iter()
                    .find_map(|o| o.sums.get(k))
                    .filter(|_| !holes.standins.contains_key(&k))
                else {
                    continue;
                };
                if t.takes & m != 0 {
                    let regs: Vec<String> = (4..12)
                        .filter(|r| t.takes & m & 1 << (r - 4) != 0)
                        .map(|r| format!("r{r}"))
                        .collect();
                    hard.push(Hole {
                        func: f,
                        at: f,
                        kind: HoleKind::Takes,
                        what: format!(
                            "a jump into {k:#010x}, another code context's, with {} as they came, which {k:#010x} reads at entry",
                            regs.join(", ")
                        ),
                    });
                }
            }
        }
    }
    if holes.holes_only {
        return Lifted {
            files: Vec::new(),
            report: format!(
                "gbaa-lift: {name}: {} functions; {} hard holes (--holes-only)\n",
                fns.len(),
                hard.len()
            ),
            holes: hard,
            metrics: Metrics::default(),
        };
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
    main.push_str("#![allow(unused_mut, unused_parens, unused_variables, unused_imports, unused_assignments, unreachable_code, unused_labels, non_snake_case, non_upper_case_globals, dead_code, clippy::all)]\n\n");
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
    if !holes.fn_check.is_empty() {
        pre = fn_check_main(&pre, holes.fn_check.keys(), holes.fn_check_interp);
    }
    main.push_str(&pre);
    main.push('\n');
    let ram_struct = globals.ram_struct(holes.layout);
    main.push_str(&ram_struct);
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
    let named =
        (holes.copies <= 1 && !(holes.fn_names.is_empty() && holes.names.is_empty())).then(|| {
            named::apply(
                &mut main,
                &ram_struct,
                &mut table,
                &mut out,
                &fns,
                &holes.fn_names,
                &holes.names,
            )
        });
    // The names files' types, declared: like the names' constants, not
    // lifted code, so not in `lines`.
    let mut type_lines = 0;
    if let Some(maps) = &holes.fields {
        let text = fields::module(&maps.types);
        type_lines = text.lines().count();
        main.push_str(&text);
    }
    let plains: Vec<String> = fns.iter().map(|&k| plain(&shapes.shaped, k)).collect();
    let mut files = modules::split(main, table, &plains, &out, holes.copies);
    if !holes.fn_check.is_empty() {
        files
            .files
            .push(("fn_check.rs".into(), FN_CHECK_DRIVER.into()));
    }
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
    writeln!(
        report,
        "bank_sp calls printed: {} ({} stores' left out, the bank already holding their sp)",
        soft.bank_sp_calls, soft.banks_left_out
    )
    .unwrap();
    writeln!(
        report,
        "flag writes printed: {}, of which nothing reads {}{}",
        soft.flag_writes,
        soft.dead_flag_writes,
        if holes.keep_dead_writes {
            " (--keep-dead-writes: every write of a register or flag printed)"
        } else {
            ""
        }
    )
    .unwrap();
    writeln!(
        report,
        "register locals: {} writes printed, {} register values as lets of their own; \
         {} registers and flags taken at entries, {} functions taking one never read as it came; \
         {} functions' bodies in their plain entry, parameters by name, {} packing them into Regs::call",
        soft.reg_writes, soft.reg_lets, soft.unpacked, soft.unpacked_unread, soft.body_in_plain, soft.regs_call
    )
    .unwrap();
    writeln!(
        report,
        "value lets printed: {}, values printed inside their use: {}{}",
        soft.value_lets,
        soft.inlined,
        if holes.keep_temps {
            " (--keep-temps: every value its own let)"
        } else {
            ""
        }
    )
    .unwrap();
    writeln!(
        report,
        "loops printed as for: {}, as loop: {}{}",
        soft.loops_for,
        soft.loops_plain,
        if holes.no_for_loops {
            " (--no-for-loops: every loop a loop)"
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
        "registers by caller (native/metroid-entry-reads): {} functions take one of r4-r11 as it came, passed by each direct call and tail call{}; {} indirect calls pass them as the holes file says",
        soft.takers.len(),
        soft.takers
            .iter()
            .map(|&(k, m)| {
                let regs: Vec<String> = (4..12)
                    .filter(|r| m & 1 << (r - 4) != 0)
                    .map(|r| format!("r{r}"))
                    .collect();
                format!(" f_{k:08x} {}", regs.join(" "))
            })
            .collect::<Vec<_>>()
            .concat(),
        soft.passes.len(),
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
    if !soft.traps.is_empty() {
        let at: BTreeSet<u32> = soft.traps.iter().copied().collect();
        let at: Vec<String> = at.iter().map(|a| format!("{a:#010x}")).collect();
        writeln!(
            report,
            "compilers' traps (native/lift-undefined-insns): {} `udf`s the compiler ends a path with, known by their encoding alone; each is a panic naming it, not a hard hole (printed where its function has no hard hole, which is a `compile_error!` whole): {}",
            at.len(),
            at.join(" "),
        )
        .unwrap();
    }
    let left: Vec<String> = soft
        .pairs_left
        .iter()
        .map(|a| format!("{a:#010x}"))
        .collect();
    writeln!(
        report,
        "narrowing idioms (native/lift-narrowing-casts): {} printed as casts; {} of the guest's {} shift pairs still printed as two shifts, by their second shift{}{}",
        soft.narrowings.len(),
        left.len(),
        soft.shift_pairs.len(),
        if left.is_empty() { "" } else { ": " },
        left.join(" "),
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
    if holes.fields.is_some() {
        writeln!(
            report,
            "named fields (native/lift-typed-access): {} accesses through a pointer the names files type print as a named field, {} as bytes inside one",
            soft.named_fields, soft.named_inside,
        )
        .unwrap();
    }
    if let Some(at) = holes.plant_slot
        && !soft.planted_slot
    {
        let h = Hole {
            func: at,
            at,
            kind: HoleKind::HolesFile,
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
    let const_lines = named.map_or(0, |n| n.const_lines);
    m.push(
        "lines",
        n(all_lines - files.wiring - prelude - const_lines - type_lines),
    );
    m.push("lines_prelude", n(prelude));
    m.push("lines_types", n(type_lines));
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
    // Structured code (native/lift-structured): the functions and blocks
    // printed from the structuring plan, and their share in percent (no
    // bars: they should rise); those left in a `match b` (barred).
    let pct = |x: usize, of: usize| (x * 100).checked_div(of).unwrap_or(100) as u64;
    let (fs, fm) = (soft.fns_structured, soft.fns_match);
    let (bs, bm) = (soft.blocks_structured, soft.blocks_match);
    m.push("functions_structured", n(fs));
    m.push("functions_structured_pct", pct(fs, fs + fm));
    m.push("functions_in_match", n(fm));
    m.push("blocks_structured", n(bs));
    m.push("blocks_structured_pct", pct(bs, bs + bm));
    m.push("blocks_in_match", n(bm));
    // Compare trees on one value printed as a `match` (native/lift-switch-
    // match; no bar), the blocks they took in besides their first, and the
    // blocks with two or more such compares left as `if`s (barred).
    m.push("compare_trees_matched", n(soft.compare_trees));
    m.push("compare_tree_blocks", n(soft.compare_tree_blocks));
    m.push("compare_trees_left", n(soft.compare_trees_left));
    // Narrowing idioms (native/lift-narrowing-casts): the guest's shift
    // pairs still printed as two shifts (barred), and the narrowings
    // printed (no bar: they should rise).
    m.push("shift_pairs_left", n(soft.pairs_left.len()));
    m.push("narrowing_casts", n(soft.narrowings.len()));
    // Flag writes printed that nothing reads (native/lift-dead-writes):
    // none, but with --keep-dead-writes.
    m.push("dead_flag_writes", soft.dead_flag_writes);
    // `bank_sp` calls printed: one per run of stores at one sp
    // (native/lift-bank-sp-once).
    m.push("bank_sp_calls", soft.bank_sp_calls);
    // The `let`s of IR values printed (native/lift-inline-temps): a pure
    // value used once, or a constant, prints inside its use instead.
    m.push("value_lets", soft.value_lets);
    // Register locals (native/lift-register-locals): writes of r0-r12's
    // mut locals printed (a value only its block reads is a `let`), and
    // functions whose entry unpacks a register or flag never read as it
    // came (bar 0).
    m.push("reg_local_writes", soft.reg_writes);
    m.push("unpacked_unread", n(soft.unpacked_unread));
    m.push("plain_entry_bodies", n(soft.body_in_plain));
    m.push("regs_call_entries", n(soft.regs_call));
    // The `let`s nothing reads, and the sums still printed as
    // `add_with_carry(..).0` (native/lift-plain-arith): none.
    let text = || files.files.iter().map(|(_, t)| t.as_str());
    m.push("unread_lets", text().map(unread::unread_lets).sum());
    m.push("adc_sums", text().map(unread::adc_sums).sum());
    // Structured loops (native/lift-for-loops): those printed as `for`
    // (no bar: it should rise) and the rest, as `loop` (barred).
    m.push("loops_for", soft.loops_for);
    m.push("loops_not_for", soft.loops_plain);
    // Joins (native/lift-join-shape): the labelled blocks printed, where
    // the flow isn't an `if`, a `match` or a loop's, and the guest blocks
    // whose code printed more than once (both barred).
    m.push("labelled_blocks", soft.labelled_blocks);
    m.push("blocks_printed_again", soft.blocks_printed_again);
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
    m.push("named_field_accesses", soft.named_fields);
    m.push("named_inside_accesses", soft.named_inside);
    // Only a lift with types has it, and its bar: what the types leave.
    if holes.fields.is_some() {
        let unknown = |st| soft.access.get(&(st, Where::Unknown)).copied().unwrap_or(0);
        m.push(
            "computed_unknown_unnamed",
            unknown(false) + unknown(true) - soft.named_unknown,
        );
    }
    m.push("sp_accesses_local_slot", soft.sp_slots[0]);
    m.push("sp_accesses_escaping_slot", soft.sp_slots[1]);
    m.push("sp_accesses_callers_frame", soft.sp_slots[2]);
    m.push("call_fn_calls", n(soft.indirect));
    m.push("call_fn_complete", n(soft.indirect_complete));
    m.push("call_fn_four_complete", n(soft.indirect_four_complete));
    m.push("jumps_by_value", n(soft.dyn_jumps));
    m.push("branches_on_raw_flags", soft.raw_flag_branches);
    m.push("entry_lr_returns", n(soft.lr_returns));
    m.push("entry_lr_returns_call_fn", n(soft.lr_returns_call_fn));
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
    if let Some(n) = named {
        writeln!(report, "{}", n.line()).unwrap();
    }
    if let Some(l) = typed_line {
        writeln!(report, "{l}").unwrap();
    }
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
    /// The summaries the frames were made from: a direct call to one that
    /// may take r4-r11 passes every register ([`fnref::Out::takes`]).
    sums: &'a Summaries,
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
/// addresses of the calls printed through `call_fn`. `named`: the
/// indirect calls a holes file's `[[takes]]` names, which pass every
/// register.
#[allow(clippy::type_complexity)]
fn fn_ins<'a>(
    codes: &[FlowIn<'a>],
    all: &BTreeSet<u32>,
    named: &BTreeSet<u32>,
) -> (Vec<fnref::FnIn<'a>>, BTreeSet<u32>, BTreeSet<u32>) {
    let mut ins = Vec::new();
    let mut pools = BTreeSet::new();
    let mut call_fn = BTreeSet::new();
    for c in codes {
        let src = c.src;
        for f in c.funcs {
            let mut blocks = BTreeMap::new();
            let mut strides_of = BTreeMap::new();
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
                    let takes = match e {
                        frame::Edge::Call {
                            target: Some(k), ..
                        } => c.sums.get(k).is_some_and(|s| s.may_take() != 0),
                        frame::Edge::Call { target: None, .. } => named.contains(&x.at),
                        _ => false,
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
                            takes,
                        },
                    );
                }
                let mut at = k & !1;
                let mut lits = BTreeMap::new();
                let mut hulls = BTreeMap::new();
                let mut strides = BTreeMap::new();
                for (i, inst) in insts.iter().enumerate() {
                    match inst.op {
                        Op::Insn(a) => at = a,
                        Op::Load { .. } | Op::Store { .. } => {
                            if let Some(x) = c.ctx.disc.rec.accesses.get(&at)
                                && let (false, false, Some(h)) = (x.sp, x.top, x.hull)
                            {
                                hulls.insert(i, h);
                                strides.insert(i, x.stride);
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
                        bases: BTreeMap::new(),
                    },
                );
                strides_of.insert(k, strides);
            }
            // The accesses whose bounds start at a literal this function
            // loads, read by an index: tables it reads from their start.
            let lits: BTreeSet<u32> = blocks
                .values()
                .flat_map(|b: &fnref::FBlock| b.insts.iter())
                .filter_map(|inst| match inst.op {
                    Op::Pool(v) => Some(v),
                    _ => None,
                })
                .collect();
            for (k, b) in blocks.iter_mut() {
                b.bases = b
                    .hulls
                    .iter()
                    .filter(|(_, (lo, hi))| hi > lo && lits.contains(lo))
                    .map(|(&i, _)| (i, strides_of[k][&i]))
                    .collect();
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

/// The prelude of a checked lift (`--fn-check`): its RAM accesses noted
/// (the prelude's `FN_CHECK`), the driver's module, the checks' list, and
/// `main` the driver's, over the program's own (`play`).
fn fn_check_main<'a>(pre: &str, keys: impl Iterator<Item = &'a u32>, interp: u32) -> String {
    let on = "pub const FN_CHECK: bool = false;";
    let main = "fn main() -> std::process::ExitCode {\n    native_host::main(";
    assert!(
        pre.contains(on) && pre.contains(main),
        "the prelude's FN_CHECK and main"
    );
    let mut pre = pre.replace(on, "pub const FN_CHECK: bool = true;").replace(
        main,
        "fn play() -> std::process::ExitCode {\n    native_host::main(",
    );
    pre.push_str("\n// The per-function check (gbaa-lift --fn-check): the functions it runs.\nmod fn_check;\npub static FN_CHECKS: &[&fn_check::Spec] = &[");
    for k in keys {
        write!(pre, "&CHECK_{k:08x}, ").unwrap();
    }
    write!(
        pre,
        "];\n/// The interpreter checks every this many compared runs (0: never).\npub const FN_CHECK_INTERP: u32 = {interp};\n\nfn main() -> std::process::ExitCode {{\n    fn_check::main(play)\n}}\n"
    )
    .unwrap();
    pre
}

/// `x_<key>` as a whole word, and `pub fn f_<key>(`, prefixed with `p`: a
/// second copy of a function's text beside the first.
fn prefix_fn(text: &str, k: u32, p: &str) -> String {
    let x = format!("x_{k:08x}");
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 64);
    let mut last = 0;
    for (i, _) in text.match_indices(&x) {
        if (i == 0 || !word(b[i - 1])) && b.get(i + x.len()).is_none_or(|&c| !word(c)) {
            out.push_str(&text[last..i]);
            out.push_str(p);
            last = i;
        }
    }
    out.push_str(&text[last..]);
    out.replace(
        &format!("pub fn f_{k:08x}("),
        &format!("pub fn {p}f_{k:08x}("),
    )
}

/// A function's lift printed with the holes given: its text and holes.
type PrintFn<'a> = dyn Fn(&Holes, &mut Soft) -> (String, Vec<Hole>) + 'a;

/// A function under the per-function check (`--fn-check`, [`Holes::fn_check`]):
/// the lift as the reference (`ref_f_<key>`, no planted store but with
/// `--plant-reference`), the
/// replacement (`new_f_<key>`: the stand-in, or for a key with none, the
/// lift again with any `--plant`), its [`FnCheck`] as the driver's spec,
/// and `f_<key>` the driver's wrapper. `print` prints the function's lift
/// with the holes given.
#[allow(clippy::too_many_arguments)]
fn fn_check_texts(
    k: u32,
    chk: &FnCheck,
    holes: &Holes,
    synthetic: &BTreeSet<u32>,
    shape: Option<Shape>,
    sum: Option<&Summary>,
    print: &PrintFn,
    soft: &mut Soft,
) -> (String, Vec<Hole>) {
    // The reference has no planted store, unless --plant-reference.
    let clean = if holes.plant_reference {
        holes.clone()
    } else {
        Holes {
            plant: None,
            plant_slot: None,
            ..holes.clone()
        }
    };
    let (rt, mut found) = print(&clean, soft);
    let mut t = format!(
        "// f_{k:08x}: under the per-function check (gbaa-lift --fn-check).\n// The reference: the lift.\n"
    );
    t.push_str(&prefix_fn(&rt, k, "ref_"));
    let replacement = if synthetic.contains(&k) {
        let (nt, nf) = print(holes, soft);
        found.extend(nf);
        t.push_str("// The replacement: the lift again (with --plant's store, if one is in it).\n");
        t.push_str(&prefix_fn(&nt, k, "new_"));
        if holes.plant_reference && (holes.plant.is_some() || holes.plant_slot.is_some()) {
            "the lift with --plant's store, as the reference is (--plant-reference)".to_string()
        } else if holes.plant.is_some() || holes.plant_slot.is_some() {
            "the lift with --plant's store (a planted wrong replacement)".to_string()
        } else {
            "the lift itself".to_string()
        }
    } else {
        let (code, why) = &holes.standins[&k];
        writeln!(t, "// The replacement: the stand-in: {why}").unwrap();
        t.push_str(&code.replace(
            &format!("pub fn f_{k:08x}("),
            &format!("pub fn new_f_{k:08x}("),
        ));
        if !t.ends_with('\n') {
            t.push('\n');
        }
        "the holes file's stand-in".to_string()
    };
    let regs = |m: u8| -> String {
        let v: Vec<String> = (0..4)
            .filter(|r| m & 1 << r != 0)
            .map(|r| format!("r{r}"))
            .collect();
        if v.is_empty() {
            "none".into()
        } else {
            v.join(",")
        }
    };
    // What callers read: the shape's results, as if not replaced; a
    // function with no shape (a summary hole) all four registers.
    let (params, results, sig) = match (shape, sum) {
        (Some(sh), _) => (
            sh.params,
            sh.results,
            format!(
                "parameters {}; compared on return: {} (its shape: what its callers read)",
                regs(sh.params),
                regs(sh.results)
            ),
        ),
        (None, Some(s)) => (
            s.params & 0xF,
            0xF,
            format!(
                "parameters {} (its summary, which has a hole); compared on return: r0-r3",
                regs(s.params & 0xF)
            ),
        ),
        (None, None) => (
            0xF,
            0xF,
            "no summary: r0-r3 in, r0-r3 compared on return".to_string(),
        ),
    };
    // The typed globals the lift reads by field (not through `ld*`, so
    // not in the run's notes): inputs the variants change too.
    let mut fields: Vec<&str> = Vec::new();
    for (i, _) in rt.match_indices("g.ram.") {
        let rest = &rt[i + 6..];
        let n = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if n > 0 && !fields.contains(&&rest[..n]) {
            fields.push(&rest[..n]);
        }
    }
    let globals: Vec<String> = fields
        .iter()
        .map(|f| format!("(std::mem::offset_of!(Ram, {f}), |r: &Ram| size_of_val(&r.{f}))"))
        .collect();
    writeln!(
        t,
        "pub static CHECK_{k:08x}: fn_check::Spec = fn_check::Spec {{ key: {k:#010x}, name: \"f_{k:08x}\", params: {params:#x}, results: {results:#x}, sig: {sig:?}, replacement: {replacement:?}, globals: &[{}], seed: {}, runs: {}, digest: {} }};",
        globals.join(", "),
        chk.seed,
        chk.runs,
        chk.digest.map_or("None".into(), |d| format!("Some({d:#018x})"))
    )
    .unwrap();
    writeln!(
        t,
        "pub fn f_{k:08x}(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {{ fn_check::call(g, sp, a, &CHECK_{k:08x}, ref_f_{k:08x}, new_f_{k:08x}) }}"
    )
    .unwrap();
    // The wrapper already has the adapter's shape; a stand-in elsewhere
    // that calls `a_KEY` by name (Metroid's IWRAM copy of a ROM function)
    // reaches the check through it.
    writeln!(
        t,
        "pub fn a_{k:08x}(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {{ f_{k:08x}(g, sp, a) }}\n"
    )
    .unwrap();
    (t, found)
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

/// The functions whose entry lr (`r14` of their extended entry, `x_`) is
/// [`RET`] however they're entered (`native/lift-pop-return`). A call
/// (`f_`, `a_`, `call_fn`, an IRQ) passes `RET`; a fall or jump into
/// another function's entry (and the code after a call, where it is one)
/// passes the jumper's r14, which is `RET` where the summary's run saw r14
/// still the jumper's entry lr there ([`Summary::lr_into`]) and the
/// jumper is one of these, or where the run never reached the jump and
/// no path of it was cut short at a hole (the jump can't run: the code
/// after a call that never returns). The greatest such set: a cycle of
/// falls is entered from outside it. A jumper with no settled summary,
/// or a stand-in (its code is the holes file's), proves nothing of what
/// it jumps into.
///
/// A return the summary's run proved a jump to the entry lr
/// ([`Summary::lr_returns`]) is then a plain return: the word it pops is
/// the one its prologue pushed, under the run's assumptions, which the
/// lift's callers already rest on ([`crate::saves`]: a store through an
/// address it can't place doesn't hit a saved register's slot; a local
/// slot is a Rust local, which none can).
fn lr_entries(codes: &[Code], all: &BTreeSet<u32>, holes: &Holes) -> BTreeSet<u32> {
    // (jumper, the function it jumps into, r14 proven its entry lr there)
    let mut pairs: Vec<(u32, u32, bool)> = Vec::new();
    for c in codes {
        for f in &c.funcs {
            let sum = c
                .sums
                .get(f.entry)
                .filter(|_| !holes.standins.contains_key(&f.entry));
            for gb in f.blocks.values() {
                for x in exits(&c.ctx, gb) {
                    for k in successors(&c.ctx, &x) {
                        if f.blocks.contains_key(&k) || !all.contains(&k) {
                            continue;
                        }
                        // A jump the run never reached, with no path of
                        // it cut short, can't run (the code after a call
                        // that never returns).
                        let ok =
                            sum.is_some_and(|s| s.lr_into.get(&k).copied().unwrap_or(!s.stopped));
                        pairs.push((f.entry, k, ok));
                    }
                }
            }
        }
    }
    let mut set = all.clone();
    loop {
        let out: Vec<u32> = pairs
            .iter()
            .filter(|&&(j, k, ok)| set.contains(&k) && (!ok || !set.contains(&j)))
            .map(|&(_, k, _)| k)
            .collect();
        if out.is_empty() {
            return set;
        }
        for k in out {
            set.remove(&k);
        }
    }
}

/// One context's functions, summaries and frames. `named`: the indirect
/// calls a holes file's `[[takes]]` names, where a stack address in a
/// register the callees take escapes.
fn code<'a>(ctx: Ctx<'a>, funcs: Vec<Func>, named: &BTreeSet<u32>, passing: &Passing) -> Code<'a> {
    let sums = summarize_passing(&ctx, &funcs, Assume::default(), &Sets::new(), passing);
    let frames = frame::of_code(&ctx, &funcs, &sums, named);
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
    /// Accesses printed through a pointer the names files type
    /// (native/lift-typed-access): as a named field, and as bytes inside
    /// one (`.field::<uN>(off)`).
    named_fields: u64,
    named_inside: u64,
    /// Those of them the value sets don't bound (`computed_unknown`).
    named_unknown: u64,
    /// Stack locals, and the functions with any.
    local_slots: usize,
    local_fns: usize,
    /// The `--plant-slot` store printed.
    planted_slot: bool,
    /// Conditions resting on a raw flag ([`cond::raw`]).
    raw_flag_branches: u64,
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
    /// The holes file's `[[takes]]` printed: (the indirect call, the
    /// function it passes r4-r11 to).
    passes: BTreeSet<(u32, u32)>,
    /// The functions that take one of r4-r11 from their callers.
    takers: Vec<(u32, u8)>,
    dyn_jumps: usize,
    /// Jumps by value to the entry lr printed as a plain return, and those
    /// still through `call_fn`, the entry lr not proven `RET`
    /// ([`lr_entries`]).
    lr_returns: usize,
    lr_returns_call_fn: usize,
    /// Falls and jumps into another function's entry, printed through its
    /// extended entry.
    falls: usize,
    standins: usize,
    /// The compilers' traps planned as a stop of the game, by address
    /// (printed so unless the function has a hard hole).
    traps: Vec<u32>,
    /// Functions whose callee-saves the summaries don't prove, and why.
    unproven: Vec<(u32, String)>,
    /// Functions printed from the structuring plan ([`crate::plan`]), and
    /// the rest (a whole `loop { match b }`); blocks printed as structured
    /// code, and those left as a match's arms (a v1 function's, or an
    /// irreducible region's).
    fns_structured: usize,
    fns_match: usize,
    blocks_structured: usize,
    blocks_match: usize,
    /// The guest's shift pairs in printed blocks, by their second
    /// shift's address ([`narrow::guest_pairs`]), and of them those whose
    /// second shift still printed as one (not as a narrowing, nor folded
    /// to a constant or left unused); every narrowing printed, by
    /// (function, block, inst).
    shift_pairs: BTreeSet<u32>,
    pairs_left: BTreeSet<u32>,
    narrowings: BTreeSet<(u32, u32, usize)>,
    /// Flag writes printed, and of them those nothing reads
    /// ([`FnPrinter::dead_writes`]: none but with `--keep-dead-writes`).
    flag_writes: u64,
    dead_flag_writes: u64,
    /// Compare trees printed as a `match` ([`switch`]), the blocks they
    /// took in besides their first, and the blocks with two or more
    /// compares of one value against a constant left as `if`s.
    compare_trees: usize,
    compare_tree_blocks: usize,
    compare_trees_left: usize,
    /// `bank_sp` calls printed (before a store, a wait, a halt), and the
    /// stores' left out as the bank already held sp
    /// ([`FnPrinter::redundant_banks`]).
    bank_sp_calls: u64,
    banks_left_out: u64,
    /// The `let`s of IR values printed (`value_lets`), and the values
    /// printed inside their use instead ([`FnPrinter::inline`]).
    value_lets: u64,
    inlined: u64,
    /// Writes of r0-r12's locals printed (`reg_local_writes`), and the
    /// register values printed as `let`s of their own instead
    /// ([`live::block_local`]; native/lift-register-locals).
    reg_writes: u64,
    reg_lets: u64,
    /// Functions whose entry unpacks a register or flag their code never
    /// reads as it came (`unpacked_unread`; bar 0), and the registers and
    /// flags unpacked in all.
    unpacked_unread: usize,
    unpacked: u64,
    /// Functions whose body is in their plain entry, its parameters
    /// arguments by name ([`FnPrinter::entries`]).
    body_in_plain: usize,
    /// And those whose plain entry still packs its arguments into
    /// [`Regs::call`] for `x_` (barred): what they take as they came is
    /// more than those (a flag, r4-r12, an lr not proven `RET`).
    regs_call: usize,
    /// The structured loops printed as `for` ([`FnPrinter::find_fors`])
    /// and as `loop`.
    loops_for: u64,
    loops_plain: u64,
    /// Labelled blocks printed (a `break` leaves each), and the times a
    /// guest block's code printed again where it had printed already
    /// (unit `native/lift-join-shape`).
    labelled_blocks: u64,
    blocks_printed_again: u64,
}

struct FnPrinter<'a, 'b> {
    ctx: &'a Ctx<'a>,
    f: &'a Func,
    all: &'a BTreeSet<u32>,
    /// This context's summaries.
    sums: &'a Summaries,
    /// Every context's functions that take r4-r11 from their callers.
    takers: &'a BTreeSet<u32>,
    /// Every shaped function's parameters and results.
    shapes: &'a BTreeMap<u32, Shape>,
    /// This one's, `None` for the four-register form.
    me: Option<Shape>,
    holes: &'a Holes,
    found: Vec<Hole>,
    soft: &'b mut Soft,
    labels: BTreeMap<u32, usize>,
    /// Printing structured ([`FnPrinter::plan`]): the nodes in a region,
    /// by label; the region whose blocks are being printed; and the planned
    /// edges of the exit being printed, which its jumps to its own blocks
    /// take ([`FnPrinter::goto`]). `None` in v1 form.
    in_region: Vec<bool>,
    /// The next block printed starts at this inst: the rest of a block
    /// whose compare run a `match` arm printed ([`FnPrinter::switch_bodies`]).
    rest_from: usize,
    /// The guest blocks whose code printed so far, to count one printed
    /// again ([`Soft::blocks_printed_again`]).
    printed: BTreeSet<u32>,
    /// Joins (unit `native/lift-join-shape`): the labelled block whose
    /// code follows where the node being printed ends, if falling off its
    /// end goes there; whether the exit being printed ends its block's
    /// code (its last, or a `match` arm), so an edge inlined there falls
    /// there too; and the loops printing, innermost last, each with the
    /// labelled block falling out of it goes to, which a `break` to that
    /// block takes the loop's label for.
    fall: Option<usize>,
    tail_edge: bool,
    loops: Vec<(usize, Option<usize>)>,
    /// The labelled block whose body's last node is the one printing
    /// (`direct`), and so whose every way of falling off its body is that
    /// block's; and, while that block's last exit prints, the label its
    /// `match`'s or jump table's one arm that falls off marks
    /// ([`fall_mark`]), where the code after the labelled block goes.
    direct: Option<usize>,
    block_direct: Option<usize>,
    mark: Option<usize>,
    /// An open `match`'s mark and its arms before the rest that fall off.
    open_mark: Option<(usize, usize)>,
    /// The insts whose guards print nothing while a block's uses are
    /// counted: a compare tree's tests ([`FnPrinter::operands`]).
    /// Empty but then.
    unguard: Vec<bool>,
    /// The exits whose printed code doesn't read their target value
    /// ([`FnPrinter::reads_target`]), by inst, while a block's uses are
    /// counted. Empty but then.
    blind: Vec<bool>,
    /// The loads a block may leave out where nothing uses their value
    /// ([`FnPrinter::quiet_loads`]), by inst, while its uses are counted.
    /// Empty but then.
    quiet: Vec<bool>,
    /// And the value its `match` reads, used once more ([`FnPrinter::uses`]).
    scrutinee: Option<ValueId>,
    region: Option<usize>,
    outs: Option<Vec<plan::Out>>,
    edges: Option<Vec<plan::Edge>>,
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
    /// Whether it is the reset or an IRQ wrapper, whose `msr` and `mrs`
    /// are printed ([`PsrFns`]); elsewhere they are hard holes.
    psr: Option<PsrPlace>,
    /// The registers each block starts with a known constant in
    /// ([`narrow::starts`]).
    starts: narrow::Starts,
    /// Its entry lr is [`RET`] on every way in ([`lr_entries`]): a jump by
    /// value to the entry lr is a plain return.
    lr_ret: bool,
    /// The writes of registers and flags nothing printed reads, by block
    /// ([`FnPrinter::dead_writes`]): left out.
    dead: BTreeMap<u32, Vec<bool>>,
    /// What is live at each block's start, as `dead` has it.
    live_in: BTreeMap<u32, live::Live>,
    /// The writes of r0-r12 only their own block reads, by block
    /// ([`FnPrinter::dead_writes`]): each prints as a `let`.
    local: BTreeMap<u32, Vec<Option<live::Local>>>,
    /// What each of r0-r12 holds as the block printing now let it
    /// (`native/lift-register-locals`): a name or a constant its reads
    /// print instead of the register's local.
    bound: [Option<String>; 13],
    /// The loops printed as `for`, by label ([`FnPrinter::find_fors`]),
    /// and the write of each one's step, left out: its block and inst.
    fors: BTreeMap<usize, counted::For>,
    steps: BTreeMap<u32, usize>,
    /// Whether the code printed last may end in a `for`: unlike a `loop`,
    /// it has type `()`, so at the function's end `unreachable!()` follows.
    tail_for: bool,
    /// The stores whose `bank_sp` the bank already holds, by block
    /// ([`FnPrinter::redundant_banks`]): printed without it.
    banked: BTreeMap<u32, Vec<bool>>,
    /// The accesses whose address the names files type, by block and
    /// inst, and where in its object it is ([`FnPrinter::field_types`]).
    field_at: BTreeMap<(u32, usize), fields::At>,
    /// The current block's values printed inside their use, by value:
    /// their expressions ([`FnPrinter::inline`]; `None`, a `let`).
    inl: Vec<Option<String>>,
}

/// Whether an untyped store at `w` ([`FnPrinter::vsa_where`]) may reach
/// I/O, where it may set `IME` again and run a handler on the current
/// stack: so the lift banks sp before it. Not a stack store, nor one at a
/// constant address outside I/O.
fn store_may_reach_io(w: Where) -> bool {
    match w {
        Where::Sp => false,
        Where::Const(r) => metrics::REGIONS[r] == "io",
        _ => true,
    }
}

/// Where an `msr` or `mrs` the lifter prints is: the hole report's
/// `CpuWhere::Boot` and `CpuWhere::Wrapper`, known without a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PsrPlace {
    /// The reset, the ROM's entry: its mode switches set each mode's sp,
    /// the program's start.
    Boot,
    /// An IRQ handler the code installs (a constant stored to the BIOS's
    /// IRQ vector): its PSR saves and its switch to system mode and back
    /// around the call.
    Wrapper,
}

/// The functions whose `msr` and `mrs` the lifter prints (the
/// `native/lift-psr-boot-irq` unit; `docs/research/native-holes.md`, "What
/// the Stage-A lifter should know"): the reset at the ROM's entry, and the
/// IRQ handlers the code installs, ROM or RAM (discovery's
/// `irq_handlers`). Their `msr cpsr` is the prelude's `msr_cpsr`, which
/// banks sp by mode as the hardware does and runs the IRQs a cleared I bit
/// releases; `mrs cpsr` is `mrs_cpsr` (the flags from the locals); the
/// SPSR is `Game::spsr`, which the prelude's IRQ entry sets. Anywhere else
/// they stay hard holes: a critical section's or a mode switch elsewhere
/// needs a stand-in.
struct PsrFns {
    wrappers: BTreeSet<u32>,
}

/// The reset's key: ARM code at the ROM's start.
const RESET: u32 = 0x0800_0000;

impl PsrFns {
    fn of(disc: &Discovery, overlays: &[discovery::Overlay]) -> PsrFns {
        let mut wrappers = disc.irq_handlers.clone();
        for o in overlays {
            wrappers.extend(&o.disc.irq_handlers);
        }
        PsrFns { wrappers }
    }

    fn place(&self, k: u32) -> Option<PsrPlace> {
        if self.wrappers.contains(&k) {
            Some(PsrPlace::Wrapper)
        } else if k == RESET {
            Some(PsrPlace::Boot)
        } else {
            None
        }
    }
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
    /// The registers each block starts with a constant in, over the
    /// edges between its blocks its exits print ([`FnPrinter::jumps`]).
    fn starts(&self) -> narrow::Starts {
        use narrow::{After, Edge};
        let mut edges = Vec::new();
        for (&k, gb) in &self.f.blocks {
            for x in exits(self.ctx, gb) {
                let after = match x.plan {
                    Plan::Static(_) | Plan::Rekey(_) | Plan::Dynamic { ret: None, .. } => {
                        After::Jump
                    }
                    Plan::Call { .. }
                    | Plan::CallVia { .. }
                    | Plan::Dynamic { ret: Some(_), .. } => After::KeptCall,
                    Plan::Swi { .. } | Plan::Undefined { .. } => After::Unknown,
                };
                for to in self.jumps(&gb.block.insts[x.index].op, &x) {
                    if self.f.blocks.contains_key(&to) {
                        edges.push(Edge {
                            from: k,
                            exit: x.index,
                            to,
                            after,
                        });
                    }
                }
            }
        }
        let f = self.f;
        narrow::starts(f.entry, &edges, |k| &f.blocks[&k].block.insts)
    }

    fn hole(&mut self, at: u32, kind: HoleKind, what: impl Into<String>) {
        self.found.push(Hole {
            func: self.f.entry,
            at,
            kind,
            what: what.into(),
        });
    }

    fn print(&mut self, sum: Option<&Summary>) -> String {
        let f = self.f;
        self.starts = self.starts();
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
                self.hole(f.entry, HoleKind::Summary, format!("summary: {h}"));
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
        // A context restore (`longjmp`'s `ldm`) is one too, named: its
        // return goes into the frame of its save's caller (`setjmp`'s),
        // whose r4-r11 and sp are that function's locals natively.
        for &at in self.frame.map(|fr| &fr.switches).into_iter().flatten() {
            let restore = f
                .blocks
                .values()
                .flat_map(|b| &b.insns)
                .any(|(a, i)| *a == at && crate::saves::context_restore(i));
            self.hole(
                at,
                HoleKind::Cpu,
                if restore {
                    "CPU state: a context restore (longjmp: a return into its save's caller's frame, \
                     whose r4-r11 and sp are that function's locals)"
                } else {
                    "CPU state: a stack switch (sp set to a value not made from the frame's)"
                },
            );
        }
        // sp set to a constant (a fresh stack, the SDK's `SoftReset`) is
        // the guest's bytes from there on, as lifted; only a return could
        // reach the frames left behind.
        if sum.is_none_or(|s| s.returns) {
            for &at in self.frame.map(|fr| &fr.fresh).into_iter().flatten() {
                self.hole(
                    at,
                    HoleKind::Cpu,
                    "CPU state: a stack switch (sp set to a constant in a function that returns)",
                );
            }
        }
        // The plain entry, for calls, and the extended one, for a fall or
        // jump from another function, which passes its registers and flags.
        // They print once the body has: whether the body can go in `f_`,
        // and what `x_` unpacks, depend on what it reads as it came
        // ([`FnPrinter::entries`]).
        let ty = ret_ty(self.me);
        let head = s.len();
        s.push_str(&self.declare_locals());
        // The entry first: label 0.
        let mut order: Vec<u32> = vec![f.entry];
        order.extend(f.blocks.keys().copied().filter(|&k| k != f.entry));
        self.labels = order.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        (self.dead, self.live_in, self.local) = self.dead_writes();
        self.field_at = self.field_types();
        self.banked = self.redundant_banks();
        if let Some(p) = self.plan(&order) {
            if !self.holes.no_for_loops {
                self.find_fors(&p.tree);
            }
            self.soft.fns_structured += 1;
            self.soft.blocks_match += p.region_blocks;
            self.soft.blocks_structured += order.len() - p.region_blocks;
            if p.regions > 0 {
                s.push_str("    let mut b = 0usize;\n");
            }
            // A jump that falls through prints nothing on its line.
            let mut body = self.seq(&p.tree);
            assert!(
                !body.contains(['\u{1}', '\u{2}']),
                "{:#010x}: a fall mark left in the printed code",
                f.entry
            );
            if self.tail_for {
                body.push_str("    unreachable!()\n");
            }
            for l in body.lines().filter(|l| !l.trim().is_empty()) {
                writeln!(s, "{l}").unwrap();
            }
            s.push_str("}\n\n");
            let e = self.entries(&s[head..], &ty);
            s.insert_str(head, &e);
            return s;
        }
        self.soft.fns_match += 1;
        self.soft.blocks_match += order.len();
        if f.blocks.len() == 1 {
            s.push_str("    let b = 0usize;\n    loop {\n        match b {\n");
        } else {
            s.push_str("    let mut b = 0usize;\n    loop {\n        match b {\n");
        }
        for k in &order {
            let label = self.labels[k];
            s.push_str(&self.arm(label));
        }
        s.push_str("            _ => unreachable!(),\n        }\n    }\n}\n\n");
        let e = self.entries(&s[head..], &ty);
        s.insert_str(head, &e);
        s
    }

    /// The function's entries and the start of its body
    /// (native/lift-register-locals), given the `body` printed. Of the
    /// registers and flags `body` names, each it reads as it came (live at
    /// the entry block's start, [`FnPrinter::dead_writes`]) is taken, the
    /// rest declared (zero or false: nothing reads that before writing
    /// it). sp is always taken: the printed code passes it by name (calls,
    /// `bank_sp`), which liveness doesn't see. A `mut` only for a register
    /// the body writes.
    ///
    /// Where all it takes is sp, its parameters (r0-r3 of a four-register
    /// function's array), an entry lr proven [`RET`] ([`lr_entries`]) and
    /// r4-r11 its summary shows reach no use (saved and restored for a
    /// caller that keeps its own), the body goes in the plain entry, `f_`, those its arguments by name,
    /// and the extended entry, `x_`, calls it with its `Regs`' values.
    /// Otherwise `f_` calls `x_` with [`Regs::call`]'s, and `x_` holds the
    /// body. A function that takes r4-r11 ([`taking`]) has a plain entry
    /// that panics: the plain one would zero them.
    ///
    /// A shaped function's plain entry takes its parameters and gives
    /// its results; its adapter, `a_<key>`, is the four-register form
    /// `call_fn` takes it by (`FNS`), and gives back the registers that
    /// aren't results as they came.
    fn entries(&mut self, body: &str, ty: &str) -> String {
        let k = self.f.entry;
        let words = rust_words(body);
        let live = self
            .live_in
            .get(&self.f.entry)
            .copied()
            .unwrap_or(live::ALL);
        let written = written_words(body);
        // (register number or 15 + flag index, `mut `, name)
        let (mut taken, mut zeros) = (Vec::new(), Vec::new());
        let regs = (0..15).map(|r| (r, format!("r{r}"), live::reg(r), "0u32"));
        let flags = Flag::ALL.into_iter().map(|fl| {
            (
                15 + fl.index(),
                flag(fl).to_string(),
                live::flag(fl),
                "false",
            )
        });
        for (i, name, bit, zero) in regs.chain(flags) {
            if !words.contains(&name) {
                continue;
            }
            let m = if written.contains(&name) { "mut " } else { "" };
            if live & bit != 0 || name == "r13" {
                self.soft.unpacked += 1;
                taken.push((i, m, name));
            } else {
                zeros.push((format!("{m}{name}"), zero.to_string()));
            }
        }
        // The check of that, on the text alone: a register taken that the
        // printed code never reads.
        self.soft.unpacked_unread += taken_unread(body, taken.iter().map(|t| t.2.as_str()));
        let let_line = |names: Vec<String>, from: Vec<String>| match &names[..] {
            [] => String::new(),
            [n] => format!("    let {n} = {};\n", from[0]),
            ns => format!("    let ({}) = ({});\n", ns.join(", "), from.join(", ")),
        };
        let taker = self.me.is_none() && self.passed(k) != 0;
        let params: u8 = self.me.map_or(0xf, |sh| sh.params);
        // r4-r11 as they came reach no use (a save, its restore unread:
        // callers keep their own) and come back in no result it gives
        // (agbcc's `pop {r3}; mov r8, r3` leaves the entry r8 in r3, which
        // a shaped function doesn't return unless r3 is a result): `x_`'s
        // and `Regs::call`'s zeroes alike.
        let returned = self.me.map_or(0xf, |sh| sh.results);
        let unused_high = self.sums.get(k).is_some_and(|s| {
            s.takes == 0
                && s.echoes.keys().all(|&r| returned & 1 << r == 0)
                && s.entry_returns.keys().all(|&r| returned & 1 << r == 0)
        });
        let in_f = !taker
            && taken.iter().all(|&(i, _, _)| {
                i == 13
                    || (i < 4 && params & 1 << i != 0)
                    || (i == 14 && self.lr_ret)
                    || ((4..12).contains(&i) && unused_high)
            });
        let mut s = String::new();
        if in_f {
            self.soft.body_in_plain += 1;
            // Its arguments by name; `_` for one the body doesn't take.
            let arg = |r: usize| {
                taken
                    .iter()
                    .find(|&&(i, _, _)| i == r)
                    .map_or("_".to_string(), |(_, m, name)| format!("{m}{name}"))
            };
            let sig = match self.me {
                None => format!(
                    "{}: u32, [{}]: [u32; 4]",
                    arg(13),
                    (0..4).map(arg).collect::<Vec<_>>().join(", ")
                ),
                Some(sh) => std::iter::once(13)
                    .chain(shape::regs(sh.params))
                    .map(|r| format!("{}: u32", arg(r)))
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            let call = match self.me {
                None => "s.r[13], [s.r[0], s.r[1], s.r[2], s.r[3]]".to_string(),
                Some(sh) => std::iter::once(13)
                    .chain(shape::regs(sh.params))
                    .map(|r| format!("s.r[{r}]"))
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            if let Some(sh) = self.me {
                writeln!(
                    s,
                    "pub fn a_{k:08x}(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {{ let [mut r0, mut r1, mut r2, mut r3] = a; {}f_{k:08x}(g, sp{})?; Ok([r0, r1, r2, r3]) }}",
                    lhs(sh.results),
                    args(sh.params)
                )
                .unwrap();
            }
            writeln!(
                s,
                "pub fn x_{k:08x}(g: &mut Game, s: Regs) -> Result<{ty}, Exit> {{ f_{k:08x}(g, {call}) }}"
            )
            .unwrap();
            writeln!(
                s,
                "pub fn f_{k:08x}(g: &mut Game, {sig}) -> Result<{ty}, Exit> {{"
            )
            .unwrap();
            // The entry lr, proven `RET`, r4-r11 (unused, as `Regs::call`
            // gives them), then what it declares.
            let (mut names, mut from): (Vec<String>, Vec<String>) = taken
                .iter()
                .filter(|&&(i, _, _)| (4..15).contains(&i) && i != 13)
                .map(|&(i, m, ref name)| {
                    let v = if i == 14 { "RET" } else { "0u32" };
                    (format!("{m}{name}"), v.to_string())
                })
                .unzip();
            for (n, z) in zeros {
                names.push(n);
                from.push(z);
            }
            s.push_str(&let_line(names, from));
            return s;
        }
        if !taker {
            self.soft.regs_call += 1;
        }
        match self.me {
            None if taker => {
                let m = self.passed(k);
                self.soft.takers.push((k, m));
                let regs: Vec<String> = (4..12)
                    .filter(|r| m & 1 << (r - 4) != 0)
                    .map(|r| format!("r{r}"))
                    .collect();
                writeln!(
                    s,
                    "pub fn f_{k:08x}(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {{ let _ = (g, sp, a); panic!(\"0x{k:08x} takes {} from its caller, and this call (through a pointer the holes file's takes doesn't name, or a stand-in's) passes none\") }}",
                    regs.join(", ")
                )
                .unwrap()
            }
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
        // One line: `let (mut r0, r13, mut r1) = (s.r[0], s.r[13], 0u32);`.
        let (mut names, mut from): (Vec<String>, Vec<String>) = taken
            .iter()
            .map(|&(i, m, ref name)| {
                let src = if i < 15 {
                    format!("s.r[{i}]")
                } else {
                    format!("s.f[{}]", i - 15)
                };
                (format!("{m}{name}"), src)
            })
            .unzip();
        for (n, z) in zeros {
            names.push(n);
            from.push(z);
        }
        s.push_str(&let_line(names, from));
        s
    }

    /// Block `label`'s code as an arm of a `match b`. The block's guest
    /// address is kept for a stop to name (`runtime::guest_pc`), on the
    /// arm's own line.
    fn arm(&mut self, label: usize) -> String {
        let k = self.key_of(label);
        format!(
            "            {label} => {{ runtime::guest_pc::set({k:#010x});\n{}            }}\n",
            self.block_at(k)
        )
    }

    fn key_of(&self, label: usize) -> u32 {
        *self
            .labels
            .iter()
            .find(|&(_, &l)| l == label)
            .expect("a label of the function's")
            .0
    }

    /// Block `k`'s code.
    fn block_at(&mut self, k: u32) -> String {
        self.printing(k);
        let gb = &self.f.blocks[&k];
        let xs = exits(self.ctx, gb);
        self.cur = k;
        self.block(&gb.block.insts, &xs, gb.block.thumb)
    }

    /// Block `k`'s code printing: counted if it printed already.
    fn printing(&mut self, k: u32) {
        if !self.printed.insert(k) {
            self.soft.blocks_printed_again += 1;
        }
    }

    /// The accesses of this function whose address the names files type
    /// ([`fields::flow`]; unit `native/lift-typed-access`), by block and
    /// inst. An exit goes on where [`FnPrinter::dead_writes`] has it: a
    /// call at its return site, with the callee's r0.
    fn field_types(&self) -> BTreeMap<(u32, usize), fields::At> {
        let Some(maps) = self.holes.fields.as_deref() else {
            return BTreeMap::new();
        };
        let mut keys: Vec<u32> = vec![self.f.entry];
        keys.extend(self.f.blocks.keys().copied().filter(|&k| k != self.f.entry));
        let index: BTreeMap<u32, usize> = keys.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let xs: Vec<Vec<ExitInfo>> = keys
            .iter()
            .map(|k| exits(self.ctx, &self.f.blocks[k]))
            .collect();
        let insts: Vec<&[Inst]> = keys
            .iter()
            .map(|k| &self.f.blocks[k].block.insts[..])
            .collect();
        let at = fields::flow(
            maps,
            self.f.entry,
            &insts,
            |b, i| {
                let Some(x) = xs[b].iter().find(|x| x.index == i) else {
                    return Vec::new();
                };
                let to = |k: u32, after| index.get(&k).map(|&s| (s, after)).into_iter().collect();
                match x.plan {
                    Plan::Static(k) | Plan::Rekey(k) => to(k, fields::After::Jump),
                    Plan::Call { target, ret } => to(ret, fields::After::Call(Some(target))),
                    Plan::CallVia { ret, .. }
                    | Plan::Dynamic { ret: Some(ret), .. }
                    | Plan::Swi { next: ret } => to(ret, fields::After::Call(None)),
                    Plan::Dynamic {
                        ret: None, target, ..
                    } => {
                        let thumb = self.f.blocks[&keys[b]].block.thumb;
                        dyn_targets(self.ctx, self.holes, x.at, thumb, target)
                            .map(|ts| {
                                ts.into_iter()
                                    .filter_map(|t| Some((*index.get(&t)?, fields::After::Jump)))
                                    .collect()
                            })
                            .unwrap_or_default()
                    }
                    Plan::Undefined { .. } => Vec::new(),
                }
            },
            |b, i| {
                let (l, d, n) = self.frame?.local_at(&self.locals, keys[b], i)?;
                (l.word && d == 0 && n == 4).then_some(l.lo)
            },
        );
        at.into_iter()
            .map(|((b, i), a)| ((keys[b], i), a))
            .collect()
    }

    /// The writes of r0-r12 and the flags nothing printed reads after
    /// them, by block ([`live::dead`]; unit `native/lift-dead-writes`), as
    /// [`FnPrinter::exit`] prints each exit: a jump to a block reads what
    /// it does; a return its results; a direct call its parameters (a
    /// shaped callee's; all of r0-r3 for a four-register one) and the
    /// r4-r11 its callee takes, then what its return site reads; a call through
    /// `call_fn` r0-r11 (a `[[takes]]` site passes some of r4-r11); a
    /// `swi` r0-r3; anything into another function's extended entry, or
    /// that this function can't follow, everything. `--keep-dead-writes`
    /// prints them all the same, counted (`dead_flag_writes`).
    ///
    /// Also the writes of r0-r12 only their own block reads
    /// ([`live::block_local`]), which print as `let`s.
    #[allow(clippy::type_complexity)]
    fn dead_writes(
        &self,
    ) -> (
        BTreeMap<u32, Vec<bool>>,
        BTreeMap<u32, live::Live>,
        BTreeMap<u32, Vec<Option<live::Local>>>,
    ) {
        let keys: Vec<u32> = self.f.blocks.keys().copied().collect();
        let index: BTreeMap<u32, usize> = keys.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let xs: Vec<Vec<ExitInfo>> = self
            .f
            .blocks
            .values()
            .map(|gb| exits(self.ctx, gb))
            .collect();
        let insts: Vec<&[Inst]> = self
            .f
            .blocks
            .values()
            .map(|gb| &gb.block.insts[..])
            .collect();
        let results = live::low(self.me.map_or(0xf, |m| m.results));
        let out = |b: usize, i: usize| {
            let Some(x) = xs[b].iter().find(|x| x.index == i) else {
                return (Vec::new(), live::ALL);
            };
            // Going on at `k`: its block, or (into another function, a
            // hole) everything.
            let to = |k: u32| match index.get(&k) {
                Some(&s) => (vec![s], 0),
                None => (Vec::new(), live::ALL),
            };
            let then = |k: u32, reads: live::Live| {
                let (s, l) = to(k);
                (s, l | reads)
            };
            match x.plan {
                Plan::Static(k) | Plan::Rekey(k) => to(k),
                // A call whose return site is no function's code (it never
                // returns): the `unreachable!` after it reads nothing
                // ([`FnPrinter::call_then`]).
                Plan::Call { target, ret }
                    if !index.contains_key(&ret)
                        && (!self.all.contains(&ret) || self.holes.fall_as_tail_call) =>
                {
                    let reads = match self.shape(target) {
                        Some(c) if self.passed(target) | self.echoes(target) == 0 => {
                            live::low(c.params)
                        }
                        _ => live::low(0xf) | live::high(self.passed(target) | self.echoes(target)),
                    };
                    (Vec::new(), reads)
                }
                // A shaped callee's call prints its parameters only.
                Plan::Call { target, ret } => match self.shape(target) {
                    Some(c) if self.passed(target) | self.echoes(target) == 0 => {
                        then(ret, live::low(c.params))
                    }
                    _ => then(
                        ret,
                        live::low(0xf) | live::high(self.passed(target) | self.echoes(target)),
                    ),
                },
                Plan::CallVia { reg, ret, .. } => then(
                    ret,
                    live::low(0xf) | live::high(0xff) | live::reg(reg.index()),
                ),
                Plan::Dynamic { ret: Some(ret), .. } => {
                    then(ret, live::low(0xf) | live::high(0xff))
                }
                // A plain return ([`FnPrinter::exit`]): its results.
                Plan::Dynamic { ret: None, .. } if !self.reads_target(x) => (Vec::new(), results),
                Plan::Dynamic {
                    ret: None, target, ..
                } => {
                    let thumb = self.f.blocks[&keys[b]].block.thumb;
                    let Ok(ts) = dyn_targets(self.ctx, self.holes, x.at, thumb, target) else {
                        return (Vec::new(), live::ALL);
                    };
                    // The return, or a tail call through `call_fn`.
                    let mut out = (Vec::new(), results | live::low(0xf));
                    for t in ts {
                        let (s, l) = to(t);
                        out.0.extend(s);
                        out.1 |= l;
                    }
                    out
                }
                Plan::Swi { next } => {
                    let reads = match self.f.blocks[&keys[b]].block.insts[x.index].op {
                        Op::Exit(Exit::Swi { comment, .. }) => {
                            swi_reads(runtime::bios::number(comment, x.thumb))
                        }
                        _ => 0xf,
                    };
                    then(next, live::low(reads))
                }
                Plan::Undefined { trap: true } => (Vec::new(), 0),
                Plan::Undefined { trap: false } => (Vec::new(), live::ALL),
            }
        };
        // The exits that print their reads through the block's names
        // ([`FnPrinter::reg`]): a direct call but a taker's (its
        // arguments; it sets its results) returning to a block of this
        // function's, and a plain return.
        let by_name = |b: usize, i: usize| {
            let x = xs[b].iter().find(|x| x.index == i)?;
            match x.plan {
                Plan::Call { target, ret }
                    if self.passed(target) | self.echoes(target) == 0
                        && index.contains_key(&ret) =>
                {
                    Some(live::low(self.shape(target).map_or(0xf, |c| c.results)))
                }
                Plan::Dynamic { ret: None, .. } if !self.reads_target(x) => Some(0),
                _ => None,
            }
        };
        // What an exit sets as printed: such a call's results, and a BIOS
        // call's ([`swi_writes`]).
        let kills = |b: usize, i: usize| {
            let Some(x) = xs[b].iter().find(|x| x.index == i) else {
                return 0;
            };
            match (x.plan, &insts[b][i].op) {
                (Plan::Swi { .. }, Op::Exit(Exit::Swi { comment, .. })) => {
                    live::low(swi_writes(runtime::bios::number(*comment, x.thumb)))
                }
                _ => by_name(b, i).unwrap_or(0),
            }
        };
        let (dead, live_in) = live::dead_and_live_killing(&insts, out, kills);
        let local = live::block_local(&insts, &live_in, out, by_name);
        (
            keys.iter().copied().zip(dead).collect(),
            keys.iter().copied().zip(live_in).collect(),
            keys.into_iter().zip(local).collect(),
        )
    }

    /// Which stores of a block print a `bank_sp` before them (`cur` the
    /// block; `typed`, `objs` and `refs` from [`FnPrinter::accesses`]):
    /// those that may reach I/O ([`store_may_reach_io`]), as
    /// [`FnPrinter::block`] prints them, but a stack local's.
    fn store_banks(
        &self,
        insts: &[Inst],
        typed: &[Option<Place>],
        objs: &[Option<Obj>],
        refs: &[Option<HeapRef>],
    ) -> Vec<bool> {
        let mut pc = 0;
        insts
            .iter()
            .enumerate()
            .map(|(i, inst)| {
                if let Op::Insn(a) = inst.op {
                    pc = a;
                }
                let Op::Store { addr, .. } = inst.op else {
                    return false;
                };
                let local = self
                    .frame
                    .is_some_and(|f| f.local_at(&self.locals, self.cur, i).is_some());
                !local
                    && typed[i].is_none()
                    && objs[i].is_none()
                    && refs[i].is_none_or(|r| r.named)
                    && store_may_reach_io(self.vsa_where(insts, addr, pc))
            })
            .collect()
    }

    /// The stores whose `bank_sp` is redundant, by block ([`banks::redundant`];
    /// unit `native/lift-bank-sp-once`): on every way to it the bank holds
    /// the same `r13`. Only a plain jump to one of the function's blocks
    /// keeps the bank; a call, a `swi`, a wait or anything leaving the
    /// function doesn't.
    fn redundant_banks(&mut self) -> BTreeMap<u32, Vec<bool>> {
        let keys: Vec<u32> = self.f.blocks.keys().copied().collect();
        // Block 0 is the entry.
        let mut order = vec![self.f.entry];
        order.extend(keys.iter().copied().filter(|&k| k != self.f.entry));
        let index: BTreeMap<u32, usize> = order.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let cur = self.cur;
        let mut banks = Vec::new();
        for &k in &order {
            self.cur = k;
            let insts = &self.f.blocks[&k].block.insts;
            let (typed, objs, refs) = self.accesses(insts);
            banks.push(self.store_banks(insts, &typed, &objs, &refs));
        }
        self.cur = cur;
        let xs: Vec<Vec<ExitInfo>> = order
            .iter()
            .map(|k| exits(self.ctx, &self.f.blocks[k]))
            .collect();
        let insts: Vec<&[Inst]> = order
            .iter()
            .map(|k| &self.f.blocks[k].block.insts[..])
            .collect();
        let redundant = banks::redundant(&insts, &banks, |b, i| {
            let Some(x) = xs[b].iter().find(|x| x.index == i) else {
                return Vec::new();
            };
            let at = |k: u32, keeps: bool| index.get(&k).map(|&s| (s, keeps));
            match x.plan {
                Plan::Static(k) | Plan::Rekey(k) => at(k, x.idle == Idle::No).into_iter().collect(),
                Plan::Call { ret, .. }
                | Plan::CallVia { ret, .. }
                | Plan::Dynamic { ret: Some(ret), .. }
                | Plan::Swi { next: ret } => at(ret, false).into_iter().collect(),
                Plan::Dynamic {
                    ret: None, target, ..
                } => {
                    let thumb = self.f.blocks[&order[b]].block.thumb;
                    dyn_targets(self.ctx, self.holes, x.at, thumb, target)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|t| at(t, false))
                        .collect()
                }
                Plan::Undefined { .. } => Vec::new(),
            }
        });
        order.into_iter().zip(redundant).collect()
    }

    /// The function's structuring plan ([`plan::plan`], as the mod view's
    /// structured codegen has it: `docs/structured-codegen-design.md`)
    /// over the jumps this printer prints between its blocks (`order`,
    /// by label), if every block ends in its one unconditional exit, they
    /// all reach from the entry and it nests at most [`MAX_INDENT`] deep. Else it is printed as v1's
    /// `loop { match b }`.
    fn plan(&mut self, order: &[u32]) -> Option<plan::Planned> {
        let last_exit = |gb: &crate::func::GBlock| {
            let insts = &gb.block.insts;
            matches!(insts.last(), Some(i) if matches!(i.op, Op::Exit(_)) && i.guard.is_none())
        };
        if !self.f.blocks.values().all(last_exit) {
            return None;
        }
        let exits: Vec<Vec<plan::Exit>> = order
            .iter()
            .map(|k| {
                let gb = &self.f.blocks[k];
                exits(self.ctx, gb)
                    .iter()
                    .map(|x| {
                        let mut targets = Vec::new();
                        for t in self.jumps(&gb.block.insts[x.index].op, x) {
                            if let Some(&l) = self.labels.get(&t)
                                && !targets.contains(&l)
                            {
                                targets.push(l);
                            }
                        }
                        plan::Exit {
                            targets,
                            leaves: true,
                        }
                    })
                    .collect()
            })
            .collect();
        let succ = exits
            .iter()
            .map(|xs| {
                let set: BTreeSet<usize> =
                    xs.iter().flat_map(|x| x.targets.iter().copied()).collect();
                set.into_iter().collect()
            })
            .collect();
        let g = crate::cfg::Cfg::new(order.to_vec(), succ);
        if !g.unreachable.is_empty() {
            return None;
        }
        let p = plan::plan(&g, &exits).ok()?;
        if p.depth().indent > MAX_INDENT {
            return None;
        }
        let mut in_region = vec![false; order.len()];
        mark_regions(&p.tree, &mut in_region);
        self.in_region = in_region;
        Some(p)
    }

    /// The plan's loops printed as `for` ([`counted`], unit
    /// `native/lift-for-loops`), into `fors`, each one's step into
    /// `steps`. A loop of one block `h`, not the function's entry, whose
    /// first exit is its guarded jump back to itself; one other block
    /// jumps to `h`, by one plain jump, leaving the variable a constant.
    /// The variable, one of r0-r12, is written once in `h` before the
    /// jump back (the step, under a guard too, as an ARM `strcc r3, [r1],
    /// #4` steps) and not read after it in the round; the run from what
    /// the jump in leaves known gives its range, round by round, guards
    /// and all.
    ///
    /// The `for` binding (`mut` where anything printed inside the loop
    /// writes it) shadows the register, and the step is left out. So
    /// after the last round's jump back isn't taken, `h` must write the
    /// register before reading it, or go only by plain jumps to blocks
    /// that don't read it before writing it ([`live`]); and no edge out
    /// of what is printed inside the loop goes to a block that reads it
    /// before writing it, as there the function's own local, which the
    /// loop didn't write, is read.
    fn find_fors(&mut self, seq: &[plan::Node]) {
        for n in seq {
            match n {
                plan::Node::Block { exits, .. } => {
                    for e in exits.iter().flat_map(|o| &o.edges) {
                        if let plan::How::Inline(s) = &e.how {
                            self.find_fors(s);
                        }
                    }
                }
                plan::Node::Loop { label, body } => {
                    if let Some((f, h, step)) = self.counted_loop(*label, body) {
                        self.fors.insert(*label, f);
                        self.steps.insert(h, step);
                    }
                    self.find_fors(body);
                }
                plan::Node::Labeled { body, .. } => self.find_fors(body),
                plan::Node::Region { .. } => {}
            }
        }
    }

    /// Loop `label` (`body`) as a `for` ([`FnPrinter::find_fors`]), with
    /// its block and the inst of its step.
    fn counted_loop(
        &self,
        label: usize,
        body: &[plan::Node],
    ) -> Option<(counted::For, u32, usize)> {
        let [plan::Node::Block { node, exits: outs }] = body else {
            return None;
        };
        let h = self.key_of(*node);
        if h == self.f.entry {
            return None;
        }
        let gb = &self.f.blocks[&h];
        let insts = &gb.block.insts;
        let xs = exits(self.ctx, gb);
        if xs.len() < 2 || xs.len() != outs.len() {
            return None;
        }
        let back = &xs[0];
        if !matches!(back.plan, Plan::Static(k) | Plan::Rekey(k) if k == h)
            || insts[back.index].guard.is_none()
            || !matches!(&outs[0].edges[..], [plan::Edge { to, how: plan::How::Continue(l) }] if to == node && *l == label)
        {
            return None;
        }
        // The one way in.
        let mut ins = Vec::new();
        for (&k, b) in &self.f.blocks {
            if k == h {
                continue;
            }
            for x in exits(self.ctx, b) {
                if self.jumps(&b.block.insts[x.index].op, &x).contains(&h) {
                    ins.push((k, x));
                }
            }
        }
        let [(p, ref px)] = ins[..] else { return None };
        if !matches!(px.plan, Plan::Static(_) | Plan::Rekey(_)) {
            return None;
        }
        let mut start = counted::State::unknown();
        counted::run(&self.f.blocks[&p].block.insts, &mut start, px.index);
        // The blocks printed inside the loop: those its exits inline; and
        // the edges out of them, to blocks printed outside it.
        let mut inside: Vec<u32> = Vec::new();
        let mut out_to: Vec<usize> = Vec::new();
        let mut todo: Vec<&plan::Node> = Vec::new();
        for e in outs[1..].iter().flat_map(|o| &o.edges) {
            match &e.how {
                plan::How::Inline(s) => todo.extend(s),
                plan::How::Continue(l) if *l == label => return None,
                _ => out_to.push(e.to),
            }
        }
        while let Some(n) = todo.pop() {
            match n {
                plan::Node::Block { node, exits } => {
                    inside.push(self.key_of(*node));
                    for e in exits.iter().flat_map(|o| &o.edges) {
                        match &e.how {
                            plan::How::Inline(s) => todo.extend(s),
                            // Back to `h` (not a way in, as it has one),
                            // or to an inner loop's start.
                            plan::How::Continue(l) if *l == label => return None,
                            plan::How::Continue(l) if inside.contains(&self.key_of(*l)) => {}
                            _ => out_to.push(e.to),
                        }
                    }
                }
                plan::Node::Loop { body, .. } | plan::Node::Labeled { body, .. } => {
                    todo.extend(body)
                }
                plan::Node::Region { .. } => return None,
            }
        }
        let others = |k: &u32| exits(self.ctx, &self.f.blocks[k]);
        for r in 0..13 {
            let bit = live::reg(r);
            let set = |i: &Inst| matches!(i.op, Op::SetReg(q, _) if q.index() == r);
            let get = |i: &Inst| matches!(i.op, Op::GetReg(q) if q.index() == r);
            let writes: Vec<usize> = (0..back.index).filter(|&i| set(&insts[i])).collect();
            let [step] = writes[..] else { continue };
            if insts[step..back.index].iter().any(get) {
                continue;
            }
            // After the jump back, until `h` writes it: no read, and only
            // plain jumps to blocks it isn't live in.
            let mut ok = true;
            for (i, inst) in insts.iter().enumerate().skip(back.index + 1) {
                if get(inst) {
                    ok = false;
                    break;
                }
                if set(inst) && inst.guard.is_none() {
                    break;
                }
                if let Some(x) = xs.iter().find(|x| x.index == i) {
                    let plain = matches!(x.plan, Plan::Static(t) | Plan::Rekey(t)
                        if self.live_in.get(&t).is_some_and(|l| l & bit == 0));
                    if !plain {
                        ok = false;
                        break;
                    }
                }
            }
            let live_out = out_to.iter().any(|&l| {
                self.live_in
                    .get(&self.key_of(l))
                    .is_none_or(|v| v & bit != 0)
            });
            if !ok || live_out {
                continue;
            }
            let Some(vs) = counted::values(insts, back.index, r, start.clone()) else {
                continue;
            };
            let Some(mut f) = counted::For::of(r, &vs) else {
                continue;
            };
            // Whether anything printed inside writes it: a write in `h`
            // or a block inside, or a call's results or a fall's.
            let calls = |xs: &[ExitInfo]| {
                xs.iter().any(|x| {
                    !matches!(x.plan, Plan::Static(t) | Plan::Rekey(t) if self.f.blocks.contains_key(&t))
                })
            };
            f.mutable = writes.len() < insts.iter().filter(|i| set(i)).count()
                || inside
                    .iter()
                    .any(|k| self.f.blocks[k].block.insts.iter().any(set))
                || (r < 4 && (calls(&xs) || inside.iter().any(|k| calls(&others(k)))));
            return Some((f, h, step));
        }
        None
    }

    /// The keys an exit's printed code may jump to ([`FnPrinter::exit`]):
    /// those in this function are its edges.
    fn jumps(&self, op: &Op, x: &ExitInfo) -> Vec<u32> {
        match x.plan {
            Plan::Static(k) | Plan::Rekey(k) => vec![k],
            Plan::Call { ret, .. } | Plan::CallVia { ret, .. } => vec![ret],
            Plan::Dynamic { ret: Some(r), .. } => vec![r],
            Plan::Dynamic { ret: None, .. } if matches!(op, Op::Exit(Exit::ExceptionReturn(_))) => {
                vec![]
            }
            Plan::Dynamic {
                ret: None, target, ..
            } => dyn_targets(self.ctx, self.holes, x.at, x.thumb, target)
                .unwrap_or_else(|_| target.into_iter().collect()),
            Plan::Swi { next } => vec![next],
            Plan::Undefined { .. } => vec![],
        }
    }

    /// A sequence of nodes: falling off one but the last goes to the
    /// next (the code after a labelled block, which its node says);
    /// falling off the last where falling off the sequence goes.
    ///
    /// A labelled block nothing breaks out of, whose body falls off at
    /// one `match` or jump table arm alone (its last block's, which
    /// marks it: [`fall_mark`]), takes the code after it into that arm
    /// (unit `native/lift-join-shape`).
    fn seq(&mut self, seq: &[plan::Node]) -> String {
        let (outer, direct) = (self.fall, self.direct);
        let mut s = String::new();
        let mut i = 0;
        while i < seq.len() {
            let last = i + 1 == seq.len();
            self.fall = if last { outer } else { None };
            self.direct = if last { direct } else { None };
            let n = self.node(&seq[i]);
            i += 1;
            let plan::Node::Labeled { label, .. } = &seq[i - 1] else {
                s.push_str(&n);
                continue;
            };
            let (mark, end) = (fall_mark(*label), arm_end(*label));
            if n.matches(&mark).count() == 1
                && !n.contains(&format!("break 'b{label};"))
                && i < seq.len()
            {
                self.fall = outer;
                self.direct = direct;
                let rest = self.seq(&seq[i..]);
                let n = n.replacen(&mark, &format!("\n{rest}"), 1);
                // Its `match` now ends the sequence: an arm's `break` to
                // where falling off it goes is its end.
                let mut parts: Vec<String> = n.split(&end).map(str::to_string).collect();
                let n = match outer {
                    Some(o) => {
                        let k = parts.len();
                        let mut fell = false;
                        for p in &mut parts[..k - 1] {
                            let q = fall_off(p, o);
                            fell |= q != *p;
                            *p = q;
                        }
                        let n = parts.concat();
                        // Those arms now fall off it too: the code after
                        // it can't go into the one arm the code just
                        // moved in marked.
                        if fell {
                            n.replace(&fall_mark(o), "").replace(&arm_end(o), "")
                        } else {
                            n
                        }
                    }
                    None => parts.concat(),
                };
                s.push_str(&n);
                break;
            }
            s.push_str(&n.replace(&mark, "").replace(&end, ""));
        }
        (self.fall, self.direct) = (outer, direct);
        s
    }

    fn node(&mut self, n: &plan::Node) -> String {
        match n {
            plan::Node::Block { node, exits } => {
                let k = self.key_of(*node);
                let outer = self.edges.take();
                self.block_direct = self.direct.take();
                let outs = exits.clone();
                let body = self.block_with(k, outs);
                self.edges = outer;
                format!("    runtime::guest_pc::set({k:#010x});\n{body}")
            }
            plan::Node::Loop { label, body } => {
                let is_for = self.fors.contains_key(label);
                let head = match self.fors.get(label) {
                    Some(f) => {
                        self.soft.loops_for += 1;
                        let m = if f.mutable { "mut " } else { "" };
                        format!("for {m}r{} in {}", f.reg, f.range())
                    }
                    None => {
                        self.soft.loops_plain += 1;
                        "loop".to_string()
                    }
                };
                // Falling out of the loop goes where falling off it does;
                // falling off its body goes round again.
                let after = self.fall.take();
                let direct = self.direct.take();
                self.loops.push((*label, after));
                let b = self.seq(body);
                self.loops.pop();
                (self.fall, self.direct) = (after, direct);
                let s = format!("    'l{label}: {head} {{\n{b}    }}\n");
                self.tail_for = is_for;
                s
            }
            // Printed bare where nothing breaks out of it: its body
            // falls to the code after it at every way out (an `if`'s,
            // a `match`'s arms').
            plan::Node::Labeled { label, body } => {
                let outer = (self.fall.replace(*label), self.direct.replace(*label));
                let b = self.seq(body);
                (self.fall, self.direct) = outer;
                self.tail_for = false;
                if b.contains(&format!("break 'b{label};")) {
                    self.soft.labelled_blocks += 1;
                    format!("    'b{label}: {{\n{b}    }}\n")
                } else {
                    b
                }
            }
            // v1's `loop { match b }` over the region's blocks, entered at
            // the label the edge into it set; leaving it, `match b` picks
            // the edge on.
            plan::Node::Region {
                label,
                blocks,
                exits,
                ..
            } => {
                let mut s = format!("    'r{label}: loop {{\n        match b {{\n");
                let outer = (self.region.replace(*label), self.edges.take());
                let fall = self.fall.take();
                let direct = self.direct.take();
                for b in blocks {
                    let plan::Node::Block { node, exits } = b else {
                        unreachable!("a region holds blocks")
                    };
                    let k = self.key_of(*node);
                    let body = self.block_with(k, exits.clone());
                    writeln!(
                        s,
                        "            {node} => {{ runtime::guest_pc::set({k:#010x});\n{body}            }}"
                    )
                    .unwrap();
                }
                (self.region, self.edges) = outer;
                (self.fall, self.direct) = (fall, direct);
                s.push_str("            _ => unreachable!(),\n        }\n    }\n");
                let mut tail_for = false;
                if !exits.is_empty() {
                    s.push_str("    match b {\n");
                    for e in exits {
                        self.tail_for = false;
                        let t = self.edge(e);
                        tail_for |= self.tail_for;
                        writeln!(s, "        {} => {{ {t} }}", e.to).unwrap();
                    }
                    s.push_str("        _ => unreachable!(),\n    }\n");
                }
                self.tail_for = tail_for;
                s
            }
        }
    }

    /// Block `k`'s code, its exits' jumps taking `outs`, by exit.
    fn block_with(&mut self, k: u32, outs: Vec<plan::Out>) -> String {
        let saved = self.outs.replace(outs);
        let cur = self.cur;
        let s = self.block_at(k);
        self.outs = saved;
        self.cur = cur;
        s
    }

    /// A planned edge's code: what follows the jump to its target. A jump
    /// into a region, or out of one, sets `b` to its target's label, which
    /// the region's `match b` reads.
    fn edge(&mut self, e: &plan::Edge) -> String {
        let set = if self.in_region[e.to] || matches!(e.how, plan::How::Leave) {
            format!("b = {}; ", e.to)
        } else {
            String::new()
        };
        let then = match &e.how {
            // Where falling out of a loop goes, out of the loop.
            plan::How::Break(l) => match self.loops.iter().rev().find(|(_, a)| *a == Some(*l)) {
                Some((x, _)) => format!("break 'l{x};"),
                None => format!("break 'b{l};"),
            },
            plan::How::Continue(l) => format!("continue 'l{l};"),
            plan::How::Next => String::new(),
            plan::How::Inline(seq) => {
                let seq = seq.clone();
                let region = self.region.take();
                let tail = std::mem::take(&mut self.tail_edge);
                let fall = if tail { self.fall } else { None };
                let outer = std::mem::replace(&mut self.fall, fall);
                let direct = self.direct.take();
                let s = format!("\n{}", self.seq(&seq));
                (self.fall, self.direct) = (outer, direct);
                self.tail_edge = tail;
                self.region = region;
                s
            }
            plan::How::Goto => format!(
                "continue 'r{};",
                self.region.expect("a goto inside a region")
            ),
            plan::How::Leave => {
                format!("break 'r{};", self.region.expect("a leave inside a region"))
            }
        };
        format!("{set}{then}")
    }

    /// A block's accesses printed as a typed global's field, and the
    /// address arithmetic only they used, which isn't printed; the
    /// computed accesses into one global; and those through a heap
    /// `Ref<T>` ([`FnPrinter::block`]; `cur` the block).
    #[allow(clippy::type_complexity)]
    fn accesses(
        &self,
        insts: &[Inst],
    ) -> (Vec<Option<Place>>, Vec<Option<Obj>>, Vec<Option<HeapRef>>) {
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
                if typed[i].is_some() || objs[i].is_some() {
                    return None;
                }
                let heap = (self.vsa_where(insts, addr, pc) == Where::Unknown)
                    .then(|| self.heap.access.get(&(self.f.entry, self.cur, i)).copied())
                    .flatten();
                // Else through a pointer the names files type
                // (native/lift-typed-access), where it lands on a field:
                // not a frame's local, which prints as one.
                let (f, named) = match heap {
                    Some(f) => (f, false),
                    None => {
                        let at = self.field_at.get(&(self.cur, i))?;
                        let maps = self.holes.fields.as_deref()?;
                        let width = match inst.op {
                            Op::Load { width, .. } | Op::Store { width, .. } => bytes(width),
                            _ => unreachable!(),
                        };
                        let local = self
                            .frame
                            .is_some_and(|f| f.local_at(&self.locals, self.cur, i).is_some());
                        if local || fields::pointer(&maps.types, at, width, "", 0).is_none() {
                            return None;
                        }
                        let f = heap::Field {
                            ty: 0,
                            off: at.off,
                            width: width as u8,
                        };
                        (f, true)
                    }
                };
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
                    // Thumb's `adds rd, rn, #k` (agbcc's address of a
                    // field past a load's reach), for a named field.
                    Op::Adc { a, b, c }
                        if named && matches!(insts[c.index()].op, Op::Bit(false)) =>
                    {
                        match (konst(insts, a), konst(insts, b)) {
                            (None, Some(c)) if c <= f.off => Some((a, c)),
                            (Some(c), None) if c <= f.off => Some((b, c)),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                Some(HeapRef { f, base, named })
            })
            .collect();
        (typed, objs, refs)
    }

    fn block(&mut self, insts: &[Inst], xs: &[ExitInfo], thumb: bool) -> String {
        let mut s = String::new();
        let mut at = 0u32;
        let ind = "                ";
        let from = std::mem::take(&mut self.rest_from);
        let (typed, objs, refs) = self.accesses(insts);
        let banks = self.store_banks(insts, &typed, &objs, &refs);
        let banked = match self.banked.get(&self.cur) {
            Some(r) if r.len() == insts.len() => r.clone(),
            _ => vec![false; insts.len()],
        };
        self.soft
            .ref_tys
            .extend(refs.iter().flatten().filter(|r| !r.named).map(|r| r.f.ty));
        let dead = dead_addresses(insts, &typed);
        // Conditions as comparisons and bit tests (native/lift-branch-conditions).
        let conds = cond::forms(insts);
        self.soft.raw_flag_branches += cond::raw(insts, &conds);
        // The writes of registers and flags nothing reads after them
        // (`dead_writes`), left out but with `--keep-dead-writes`.
        let unread: Vec<bool> = match self.dead.get(&self.cur) {
            Some(d) if d.len() == insts.len() => d.clone(),
            _ => vec![false; insts.len()],
        };
        let keep = self.holes.keep_dead_writes;
        // A `for`'s step, which its range takes the place of.
        let step = self.steps.get(&self.cur).copied();
        let skip: Vec<bool> = unread
            .iter()
            .enumerate()
            .map(|(i, &u)| (u && !keep) || step == Some(i))
            .collect();
        // A compare tree at the block's end prints as a `match`: its
        // tests' guards aren't printed, its register and flag writes are
        // its arms'.
        let cur = self.cur;
        let outs = self.outs.clone();
        // A run of two tests or more; else one test whose block goes on
        // comparing, two or more in all.
        let tree = self
            .switch_tree(cur, insts, xs, outs.as_deref(), 0, 2)
            .or_else(
                |n| match self.switch_tree(cur, insts, xs, outs.as_deref(), 0, 1) {
                    Ok(t) if t.tests() >= 2 => Ok(t),
                    _ => Err(n),
                },
            );
        let tree = match tree {
            // The rest of a block whose run another's arm printed.
            _ if from > 0 => None,
            // A `for`'s block: its range stands for the jump back's test.
            _ if step.is_some() => None,
            Ok(t) => match switch::arms(&t) {
                Some(a) => Some((t, a)),
                None => {
                    self.soft.compare_trees_left += 1;
                    None
                }
            },
            Err(n) => {
                self.soft.compare_trees_left += usize::from(n >= 2);
                None
            }
        };
        // The tests' guards print nothing; the value compared prints as a
        // `let` the `match` reads.
        self.unguard = (0..insts.len())
            .map(|i| {
                tree.as_ref()
                    .is_some_and(|(t, _)| t.run.tests.iter().any(|&(j, _)| j == i))
            })
            .collect();
        let scrutinee = tree.as_ref().map(|(t, _)| t.run.value);
        self.scrutinee = scrutinee;
        // The narrowing idioms and shifts by a constant, printed over the
        // value they narrow or shift ([`narrow`]).
        let none = BTreeMap::new();
        let known = narrow::values(insts, self.starts.get(&self.cur).unwrap_or(&none));
        let forms: Vec<_> = (0..insts.len())
            .map(|i| narrow::form(insts, &known, i))
            .collect();
        let pairs = narrow::guest_pairs(&self.f.blocks[&self.cur].insns);
        self.soft.shift_pairs.extend(&pairs);
        self.blind = self.blind_exits(insts, xs);
        let quiet = self.quiet_loads(insts, &typed, &objs, &refs);
        self.quiet = quiet.clone();
        let uses = self.uses(insts, &typed, &refs, &skip, &dead, &conds, &forms);
        let used: Vec<bool> = uses.iter().map(|&n| n > 0).collect();
        // The values printed inside their use, not as `let`s (unit
        // `native/lift-inline-temps`); the block's own, a nested block's
        // (an inlined edge's) set aside while that prints.
        let mut inline = self.inline(insts, &typed, &refs, &skip, &dead, &uses, &conds, &forms);
        if let Some(x) = scrutinee {
            inline[x.index()] = false;
        }
        self.unguard = Vec::new();
        self.blind = Vec::new();
        self.quiet = Vec::new();
        self.scrutinee = None;
        let outer = std::mem::replace(&mut self.inl, vec![None; insts.len()]);
        // A register value only this block reads prints as a `let`
        // ([`live::block_local`]), but in a block printed in pieces (a
        // compare tree's, a `for`'s): its reads go by `bound`.
        let outer_bound = std::mem::take(&mut self.bound);
        // Only where something printed reads it: a call or return by
        // name, or a read whose value is used; else it is the write it
        // was (a value a call's result overwrites, say).
        let local: Vec<bool> = match self.local.get(&self.cur) {
            Some(l) if l.len() == insts.len() && from == 0 && tree.is_none() && step.is_none() => l
                .iter()
                .map(|l| {
                    l.as_ref()
                        .is_some_and(|l| l.by_exit || l.reads.iter().any(|&j| used[j]))
                })
                .collect(),
            _ => vec![false; insts.len()],
        };
        // Where falling off the block goes: the labelled block its last
        // exit falls to the end of. A guarded exit that breaks to it
        // prints as an `if` whose `else` is the rest of the block (unit
        // `native/lift-join-shape`); what closes each, innermost last.
        let fall = self.fall.filter(|_| self.outs.is_some());
        // Whether its ways of falling off are its labelled block's.
        let direct = self.block_direct.take().filter(|_| from == 0);
        let mut closers: Vec<String> = Vec::new();
        // An open `match`'s marks ([`FnPrinter::print_switch`]): its
        // label, how many arms before the rest fall off, and where the
        // rest starts.
        let mut open_mark: Option<(usize, usize, usize)> = None;
        for (i, inst) in insts.iter().enumerate() {
            let id = ValueId(i as u32);
            if i < from {
                if let Op::Insn(a) = inst.op {
                    at = a;
                }
                continue;
            }
            if dead[i] || skip[i] || ((pure(&inst.op) || quiet[i]) && !used[i]) {
                continue;
            }
            // A read of a register this block let: the `let`'s name.
            if let Op::GetReg(r) = inst.op
                && let Some(Some(b)) = self.bound.get(r.index())
            {
                self.inl[i] = Some(b.clone());
                continue;
            }
            // A narrowing idiom counts printed inline or as a `let`; so
            // does a guest shift pair left a shift.
            if forms[i].is_some_and(|(_, f)| f.narrows()) {
                self.soft.narrowings.insert((self.f.entry, self.cur, i));
            } else if matches!(inst.op, Op::Shift { .. }) && pairs.contains(&at) {
                self.soft.pairs_left.insert(at);
            }
            if inline[i] {
                let e = self.value(&inst.op, &conds[i], forms[i]);
                self.inl[i] = Some(e);
                self.soft.inlined += 1;
                continue;
            }
            if let Some((t, arms)) = &tree {
                // The writes before the first test are every arm's.
                if t.run.deferred(insts, i) && i > t.run.tests[0].0 {
                    if let Op::SetFlag(..) = inst.op {
                        self.soft.flag_writes += 1;
                        self.soft.dead_flag_writes += u64::from(unread[i]);
                    }
                    continue;
                }
                if i == t.run.at() {
                    self.mark = direct.filter(|_| closers.is_empty());
                    let m = self.print_switch(t, arms, insts, xs, thumb, &skip);
                    self.mark = None;
                    s.push_str(&m);
                    open_mark = self.open_mark.take().map(|(l, n)| (l, n, s.len()));
                    // The rest of the block is the `match`'s last arm.
                    if let switch::End::Rest(_) = t.run.end {
                        closers.push(format!("{ind}    }}\n{ind}}}\n"));
                    }
                    continue;
                }
                if t.run.exit(i) {
                    continue;
                }
            }
            // A load of a stack local nothing uses (a callee-saved
            // register's restore before a return) reads a Rust local:
            // nothing to print.
            if matches!(inst.op, Op::Load { .. })
                && !used[i]
                && !self.holes.shadow_locals
                && self
                    .frame
                    .is_some_and(|f| f.local_at(&self.locals, self.cur, i).is_some())
            {
                continue;
            }
            if let Op::SetFlag(..) = inst.op {
                self.soft.flag_writes += 1;
                self.soft.dead_flag_writes += u64::from(unread[i]);
            }
            let guard = inst.guard.map(|g| self.whole(g));
            let stmt = |s: &mut String, body: String| match &guard {
                Some(g) => writeln!(s, "{ind}if {g} {{ {body} }}").unwrap(),
                None => writeln!(s, "{ind}{body}").unwrap(),
            };
            match inst.op {
                Op::Insn(a) => at = a,
                Op::Timing(_) | Op::YieldFlag(..) => {}
                Op::SetReg(r, x) if local[i] => {
                    // Its own `let`, or (a name or a constant) none.
                    let e = self.whole(x);
                    let b = if plain_value(&e) {
                        e
                    } else {
                        writeln!(s, "{ind}let {}: u32 = {e};", v(id)).unwrap();
                        v(id)
                    };
                    self.bound[r.index()] = Some(b);
                    self.soft.reg_lets += 1;
                }
                Op::SetReg(r, x) => {
                    if let Some(b) = self.bound.get_mut(r.index()) {
                        *b = None;
                        self.soft.reg_writes += 1;
                    }
                    stmt(&mut s, format!("r{} = {};", r.index(), self.whole(x)))
                }
                Op::SetFlag(fl, x) => stmt(&mut s, format!("{} = {};", flag(fl), self.whole(x))),
                Op::GetReg(r) => {
                    self.soft.value_lets += 1;
                    writeln!(s, "{ind}let {}: u32 = r{};", v(id), r.index()).unwrap()
                }
                Op::GetFlag(fl) => {
                    self.soft.value_lets += 1;
                    writeln!(s, "{ind}let {}: bool = {};", v(id), flag(fl)).unwrap()
                }
                Op::GetUserReg(_) | Op::SetUserReg(..) => self.hole(
                    at,
                    HoleKind::Cpu,
                    "CPU state: a user-bank register transfer",
                ),
                Op::GetCpsr | Op::GetSpsr if self.psr.is_none() => {
                    self.hole(at, HoleKind::Cpu, "CPU state: mrs (reads a PSR)")
                }
                Op::SetCpsr { .. } | Op::SetSpsr { .. } if self.psr.is_none() => {
                    self.hole(at, HoleKind::Cpu, "CPU state: msr (writes a PSR)")
                }
                // The reset's and the IRQ wrapper's ([`PsrFns`]).
                Op::GetCpsr => {
                    writeln!(s, "{ind}let {}: u32 = mrs_cpsr(g, nf, zf, cf, vf);", v(id)).unwrap()
                }
                Op::GetSpsr => writeln!(s, "{ind}let {}: u32 = g.spsr;", v(id)).unwrap(),
                Op::SetCpsr { v: x, mask } => {
                    let x = self.val(x);
                    let mut body = String::new();
                    if mask.contains(PsrMask::FLAGS) {
                        for (fl, bit) in Flag::ALL.into_iter().zip([31, 30, 29, 28]) {
                            write!(body, "{} = {x} & 1 << {bit} != 0; ", flag(fl)).unwrap();
                        }
                    }
                    if mask.contains(PsrMask::CONTROL) {
                        write!(body, "r13 = msr_cpsr(g, r13, {x});").unwrap();
                    }
                    if !body.is_empty() {
                        stmt(&mut s, body.trim_end().to_string());
                    }
                }
                Op::SetSpsr { v: x, mask } => {
                    let m = mask.bits();
                    stmt(
                        &mut s,
                        format!("g.spsr = g.spsr & !{m:#x} | {} & {m:#x};", self.val(x)),
                    );
                }
                Op::Load { width, addr } => {
                    let (f, m) = match width {
                        Width::Byte => ("ld8", ""),
                        Width::Half => ("ld16", " & !1"),
                        Width::Word => ("ld32", " & !3"),
                    };
                    let mem = match (&typed[i], &objs[i], refs[i]) {
                        (Some(p), _, _) => globals::load(p, bytes(width)),
                        (None, Some(o), _) => {
                            o.load(&format!("{}{m}", self.val(addr)), bytes(width))
                        }
                        (None, None, Some(r)) if r.named => {
                            format!("g.get({})", self.field_ptr(i, r, addr))
                        }
                        (None, None, Some(r)) => match r.base {
                            Some((b, c)) => self.heap.load(r.f, &self.val(b), r.f.off - c),
                            None => self.heap.load(r.f, &self.val(addr), r.f.off),
                        },
                        (None, None, None) => format!("{f}(g, {}{m})", self.val(addr)),
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
                            let w = self.count(
                                insts,
                                addr,
                                at,
                                i,
                                false,
                                typed_as(&typed[i], &objs[i], refs[i].is_some_and(|r| !r.named)),
                            );
                            self.soft.named_unknown +=
                                u64::from(w == Where::Unknown && refs[i].is_some_and(|r| r.named));
                            mem
                        }
                    };
                    // One nothing reads is still a bus access.
                    if !used[i] {
                        stmt(&mut s, format!("{e};"));
                        continue;
                    }
                    self.soft.value_lets += 1;
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
                    // One term: a field's store casts it (`{val} as u16`).
                    let val = format!("{}{plant}", self.val(value));
                    let mem = match (&typed[i], &objs[i], refs[i]) {
                        (Some(p), _, _) => globals::store(p, bytes(width), &val),
                        (None, Some(o), _) => {
                            o.store(&format!("{}{m}", self.val(addr)), bytes(width), &val)
                        }
                        (None, None, Some(r)) if r.named => {
                            format!("g.set({}, {val});", self.field_ptr(i, r, addr))
                        }
                        (None, None, Some(r)) => match r.base {
                            Some((b, c)) => self.heap.store(r.f, &self.val(b), r.f.off - c, &val),
                            None => self.heap.store(r.f, &self.val(addr), r.f.off, &val),
                        },
                        (None, None, None) => {
                            format!("{f}(g, {}{m}, {val});", self.val(addr))
                        }
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
                                typed_as(&typed[i], &objs[i], refs[i].is_some_and(|r| !r.named)),
                            );
                            self.soft.named_unknown +=
                                u64::from(w == Where::Unknown && refs[i].is_some_and(|r| r.named));
                            assert_eq!(
                                banks[i],
                                typed[i].is_none()
                                    && objs[i].is_none()
                                    && refs[i].is_none_or(|r| r.named)
                                    && store_may_reach_io(w),
                                "{at:#010x}: the bank plan's store isn't the printed one"
                            );
                            if banks[i] && !banked[i] {
                                // sp goes to the current mode's bank: the
                                // reset, an IRQ wrapper or what they call
                                // may be in another mode than system.
                                self.soft.bank_sp_calls += 1;
                                format!("bank_sp(g, r13); {mem}")
                            } else {
                                self.soft.banks_left_out += u64::from(banks[i]);
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
                    // Structured, the exit's jumps take its planned edges.
                    if let Some(outs) = &self.outs {
                        let j = xs
                            .iter()
                            .position(|y| y.index == i)
                            .expect("an exit of the block's");
                        self.edges = Some(outs[j].edges.clone());
                    }
                    // The last exit ends the block's code; so does a
                    // guarded one where the rest of the block can be its
                    // `else` (below).
                    self.tail_edge = guard.is_none() || fall.is_some();
                    self.mark = direct.filter(|_| guard.is_none() && closers.is_empty());
                    let body = format!("{}{}", self.flush(i), self.exit(e, &x, thumb));
                    self.mark = None;
                    self.tail_edge = false;
                    self.edges = None;
                    // One that may fall off (to where falling off the
                    // block goes) has the rest of the block as its `else`.
                    let then = fall
                        .filter(|_| guard.is_some())
                        .map(|l| fall_off(&body, l))
                        .filter(|t| !diverges(t));
                    match (&guard, then) {
                        (Some(_), Some(t)) if t.trim().is_empty() => {
                            let g = inst.guard.expect("a guarded exit");
                            let not = match (&self.inl[g.index()], &conds[g.index()]) {
                                (Some(_), Some(c)) if c.takes_inlined() => {
                                    c.not().text_with(&|x| self.val(x))
                                }
                                _ => format!("!{}", self.val(g)),
                            };
                            writeln!(s, "{ind}if {not} {{").unwrap();
                            closers.push(format!("{ind}}}\n"));
                        }
                        (Some(gv), Some(t)) => {
                            writeln!(s, "{ind}if {gv} {{\n{t}{ind}}} else {{").unwrap();
                            closers.push(format!("{ind}}}\n"));
                        }
                        (Some(gv), None) => writeln!(s, "{ind}if {gv} {{\n{body}{ind}}}").unwrap(),
                        (None, _) => s.push_str(&body),
                    }
                }
                ref op => {
                    let ty = if is_bit(op) { "bool" } else { "u32" };
                    let e = self.value(op, &conds[i], forms[i]);
                    self.soft.value_lets += 1;
                    writeln!(s, "{ind}let {}: {ty} = {e};", v(id)).unwrap();
                }
            }
        }
        // The rest is the open `match`'s last arm: its one way of falling
        // off, or none and the arm before it.
        if let Some((l, falls, at)) = open_mark {
            let rest_falls = closers.len() > 1 || !diverges(&s[at..]);
            match (falls, rest_falls) {
                (1, false) => s.push_str(&arm_end(l)),
                (0, true) if closers.len() == 1 => {
                    s.push_str(&fall_mark(l));
                    s.push_str(&arm_end(l));
                }
                _ => s = s.replace(&fall_mark(l), "").replace(&arm_end(l), ""),
            }
        }
        for c in closers.iter().rev() {
            s.push_str(c);
        }
        self.inl = outer;
        self.bound = outer_bound;
        s
    }

    /// The printed pointer of the access at inst `i` through a pointer
    /// the names files type ([`fields::pointer`]), counted.
    fn field_ptr(&mut self, i: usize, r: HeapRef, addr: ValueId) -> String {
        let maps = self
            .holes
            .fields
            .as_deref()
            .expect("a named access has types");
        let at = &self.field_at[&(self.cur, i)];
        let (p, k) = match r.base {
            Some((b, c)) => (self.val(b), at.off - c),
            None => (self.val(addr), at.off),
        };
        let (e, whole) = fields::pointer(&maps.types, at, r.f.width as u32, &p, k)
            .expect("checked when the access was found");
        if whole {
            self.soft.named_fields += 1;
        } else {
            self.soft.named_inside += 1;
        }
        e
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

    /// How many times something printed uses each of a block's values.
    /// An access printed as a stack local's (unless `--shadow-locals`
    /// reads the frame's bytes too) or a typed global's (`typed`) doesn't
    /// use its address, so the arithmetic only the address needed isn't
    /// printed ([`pure`]), and neither does a write `skip` leaves out (a
    /// dead one's, [`FnPrinter::dead_writes`]) or an address `dead`
    /// leaves out ([`dead_addresses`]); a value printed over another
    /// ([`narrow::form`]) uses only that one, and a condition with a
    /// closed form ([`cond::Form`]) what the form reads.
    #[allow(clippy::too_many_arguments)]
    fn uses(
        &self,
        insts: &[Inst],
        typed: &[Option<Place>],
        refs: &[Option<HeapRef>],
        skip: &[bool],
        dead: &[bool],
        conds: &[Option<cond::Form>],
        forms: &[Option<(ValueId, narrow::Form)>],
    ) -> Vec<u32> {
        let mut uses = vec![0u32; insts.len()];
        if let Some(x) = self.scrutinee {
            uses[x.index()] = 1;
        }
        for (i, inst) in insts.iter().enumerate().rev() {
            if skip[i] || dead[i] || (self.gone_if_unused(i, &inst.op) && uses[i] == 0) {
                continue;
            }
            for x in self.operands(i, inst, typed, refs, conds, forms) {
                uses[x.index()] += 1;
            }
        }
        uses
    }

    /// The values inst `i`'s printed code uses, its guard last ([`uses`]):
    /// a timing mark or a yielded flag uses none, printed as nothing (nor
    /// its guard); a value printed over another ([`narrow::form`]) uses
    /// only that one; and a compare tree's test none for its guard, which
    /// the `match` replaces ([`FnPrinter::unguard`]).
    ///
    /// [`uses`]: FnPrinter::uses
    fn operands(
        &self,
        i: usize,
        inst: &Inst,
        typed: &[Option<Place>],
        refs: &[Option<HeapRef>],
        conds: &[Option<cond::Form>],
        forms: &[Option<(ValueId, narrow::Form)>],
    ) -> Vec<ValueId> {
        let local = !self.holes.shadow_locals
            && self
                .frame
                .is_some_and(|f| f.local_at(&self.locals, self.cur, i).is_some());
        let field = typed[i].is_some();
        let base = refs[i].and_then(|r| r.base).map(|(b, _)| b);
        let ops = match inst.op {
            // Printed as nothing.
            Op::Timing(_) | Op::YieldFlag(..) => return Vec::new(),
            Op::Load { .. } if local || field => Vec::new(),
            Op::Store { value, .. } if local || field => vec![value],
            Op::Load { .. } if base.is_some() => base.into_iter().collect(),
            Op::Store { value, .. } if base.is_some() => base.into_iter().chain([value]).collect(),
            _ if let Some(f) = &conds[i] => f.uses.clone(),
            _ if let Some((x, f)) = forms[i] => [x].into_iter().chain(f.others()).collect(),
            Op::Exit(_) if self.blind.get(i).copied().unwrap_or(false) => Vec::new(),
            ref op => op.operands(),
        };
        let guard = inst
            .guard
            .filter(|_| !self.unguard.get(i).copied().unwrap_or(false));
        ops.into_iter().chain(guard).collect()
    }

    /// Which of a block's values print inside their one use rather than
    /// as a `let` (unit `native/lift-inline-temps`): a pure value used
    /// once, and a constant at every use, where nothing between the value
    /// and its use writes a register or flag it reads (through the values
    /// inlined into it too). Loads stay `let`s, and so do the operands of
    /// an exit and of a PSR write, which print theirs more than once
    /// (constants but). Inlined into a value that is itself inlined, a
    /// value is read where that one's use is. A condition's closed form
    /// ([`cond::Form`]) that is a comparison takes its operands inlined;
    /// any other (a join of two, an overflow test) names their `let`s, so
    /// those stay `let`s, whoever else uses them (a constant too).
    #[allow(clippy::too_many_arguments)]
    fn inline(
        &self,
        insts: &[Inst],
        typed: &[Option<Place>],
        refs: &[Option<HeapRef>],
        skip: &[bool],
        dead: &[bool],
        uses: &[u32],
        conds: &[Option<cond::Form>],
        forms: &[Option<(ValueId, narrow::Form)>],
    ) -> Vec<bool> {
        let n = insts.len();
        let mut inline = vec![false; n];
        if self.holes.keep_temps {
            return inline;
        }
        let konst = |op: &Op| matches!(op, Op::Const(_) | Op::Pool(_) | Op::Bit(_));
        // What each pending value reads: r0-r15 as bits 0-15, the flags
        // 16-19; `None` once it is printed as a `let` or used.
        let mut reads: Vec<Option<u32>> = vec![None; n];
        // The values a printed form names.
        let mut named = vec![false; n];
        for (i, f) in conds.iter().enumerate() {
            if let Some(f) = f
                && !f.takes_inlined()
                && !(dead[i] || skip[i] || uses[i] == 0)
            {
                for x in &f.uses {
                    named[x.index()] = true;
                }
            }
        }
        let flag_bit = |f: Flag| 1u32 << (16 + f as u32);
        for (i, inst) in insts.iter().enumerate() {
            let op = &inst.op;
            if dead[i] || skip[i] || (self.gone_if_unused(i, op) && uses[i] == 0) {
                continue;
            }
            let mut r = match *op {
                Op::GetReg(g) => 1 << g.index(),
                Op::GetFlag(f) => flag_bit(f),
                _ => 0,
            };
            let whole = !matches!(op, Op::Exit(_) | Op::SetCpsr { .. } | Op::SetSpsr { .. });
            let ops = match &conds[i] {
                Some(f) if !f.takes_inlined() => Vec::new(),
                _ => self.operands(i, inst, typed, refs, conds, forms),
            };
            for x in ops {
                let x = x.index();
                if named[x] {
                    reads[x] = None;
                } else if konst(&insts[x].op) {
                    inline[x] = true;
                } else if whole && let Some(rx) = reads[x].take() {
                    inline[x] = true;
                    r |= rx;
                }
            }
            // A write ends every pending value that reads what it writes.
            let w = match *op {
                Op::SetReg(g, _) => 1 << g.index(),
                Op::SetFlag(f, _) => flag_bit(f),
                Op::SetCpsr { .. } | Op::SetUserReg(..) => u32::MAX,
                _ => 0,
            };
            if w != 0 {
                for p in reads.iter_mut() {
                    if p.is_some_and(|p| p & w != 0) {
                        *p = None;
                    }
                }
            }
            if pure(op) && uses[i] == 1 && !konst(op) {
                reads[i] = Some(r);
            }
        }
        inline
    }

    /// The expression of a value `op`: a condition's closed form if it
    /// has one (`cond`, [`cond::Form`]); over the value it narrows or
    /// shifts if it has a `form` ([`narrow::form`]); else of its operands.
    fn value(
        &self,
        op: &Op,
        cond: &Option<cond::Form>,
        form: Option<(ValueId, narrow::Form)>,
    ) -> String {
        if let Some(c) = cond {
            return c.text_with(&|x| self.val(x));
        }
        match form {
            Some((x, form)) => form.print_with(&self.val(x), &|y| self.whole(y)),
            None => expr(op, &|x| self.val(x), &|x| self.whole(x)),
        }
    }

    /// Value `x` where an operand goes: its name, or its inlined
    /// expression ([`FnPrinter::inline`]), in parentheses unless it is
    /// one term.
    fn val(&self, x: ValueId) -> String {
        match self.inl.get(x.index()) {
            Some(Some(e)) if !one_term(e) => format!("({e})"),
            Some(Some(e)) => e.clone(),
            _ => v(x),
        }
    }

    /// Value `x` where a whole expression goes (an assignment's right
    /// side, a condition): its name or its inlined expression as it is.
    fn whole(&self, x: ValueId) -> String {
        match self.inl.get(x.index()) {
            Some(Some(e)) => e.clone(),
            _ => v(x),
        }
    }

    /// The writes of block `k`'s insts nothing reads ([`FnPrinter::dead_writes`]),
    /// and those left out.
    fn unread(&self, k: u32, n: usize) -> (Vec<bool>, Vec<bool>) {
        let unread: Vec<bool> = match self.dead.get(&k) {
            Some(d) if d.len() == n => d.clone(),
            _ => vec![false; n],
        };
        let keep = self.holes.keep_dead_writes;
        let skip = unread.iter().map(|&u| u && !keep).collect();
        (unread, skip)
    }

    /// The compare tree at the end of block `cur` ([`switch::run`], its
    /// first run of at least `min` tests), with
    /// the trees of the blocks its plain jumps go on to: a block of the
    /// plan's reached by that edge alone ([`plan::How::Inline`]), that
    /// does nothing before its run, and compares the register holding
    /// the same value. Else how many of its exits compare one value.
    fn switch_tree(
        &self,
        cur: u32,
        insts: &[Inst],
        xs: &[ExitInfo],
        outs: Option<&[plan::Out]>,
        depth: usize,
        min: usize,
    ) -> Result<switch::Tree, usize> {
        let run = switch::run(insts, min)?;
        let mut next: Vec<_> = run
            .exits()
            .map(|i| self.switch_next(cur, insts, xs, outs, &run, i, depth))
            .collect();
        next.resize(run.tests.len() + 1, None);
        Ok(switch::Tree { run, next })
    }

    #[allow(clippy::too_many_arguments)]
    fn switch_next(
        &self,
        cur: u32,
        insts: &[Inst],
        xs: &[ExitInfo],
        outs: Option<&[plan::Out]>,
        run: &switch::Run,
        i: usize,
        depth: usize,
    ) -> Option<switch::Tree> {
        let (outs, j) = (outs?, xs.iter().position(|y| y.index == i)?);
        let x = &xs[j];
        if depth >= 8
            || !switch::plain(&insts[i].op)
            || !matches!(x.plan, Plan::Static(_))
            || x.idle != Idle::No
            || self.frame.is_some_and(|f| f.flush.contains(&(cur, i)))
        {
            return None;
        }
        let (node, exits) = inline_block(outs.get(j)?)?;
        if self.in_region[node] {
            return None;
        }
        let reg = switch::holder(insts, run.value, i)?;
        let k = self.key_of(node);
        let gb = &self.f.blocks[&k];
        let nxs = exits_of_block(self.ctx, gb);
        let n = &gb.block.insts;
        let t = self
            .switch_tree(k, n, &nxs, Some(exits), depth + 1, 1)
            .ok()?;
        // Its block prints nothing in the arms but its leaves (unit
        // `native/lift-join-shape`): no write the run defers that is
        // read, and no value its exits take. Else it prints once, as a
        // `match` of its own inside this one's arm.
        let (_, nskip) = self.unread(k, n.len());
        let silent = (0..n.len()).all(|d| !t.run.deferred(n, d) || nskip[d])
            && tree_lets(n, &t.run, &nskip, &self.blind_exits(n, &nxs), false).is_empty();
        (silent
            && switch::quiet_before(n, t.run.start)
            && n[t.run.value.index()].op == Op::GetReg(reg))
        .then_some(t)
    }

    /// A compare tree as a `match` on its value, each leaf an arm
    /// ([`switch::arms`]), the last `_`: when the run ends at its last
    /// test, that arm is left open for the rest of the block, which
    /// [`FnPrinter::block`] closes.
    fn print_switch(
        &mut self,
        t: &switch::Tree,
        arms: &[Vec<(u32, u32)>],
        insts: &[Inst],
        xs: &[ExitInfo],
        thumb: bool,
        skip: &[bool],
    ) -> String {
        let ind = "                ";
        let (cur, outs) = (self.cur, self.outs.clone());
        // (The arms' own blocks print with marks of their own.)
        let mark = self.mark.take();
        let bodies = self.switch_bodies(t, insts, xs, thumb, skip, cur, outs.as_deref(), true);
        self.cur = cur;
        self.soft.compare_trees += 1;
        let open = matches!(t.run.end, switch::End::Rest(_));
        // Where falling off the `match` goes, the end of its block's
        // code: the labelled block falling off that goes to. An arm's
        // `break` to it is the same as its end (unit
        // `native/lift-join-shape`).
        let fall = self.fall.filter(|_| outs.is_some());
        let bodies: Vec<String> = bodies
            .into_iter()
            .map(|b| match fall {
                Some(l) => fall_off(&b, l),
                None => b,
            })
            .collect();
        let mut bodies = merge_arms(bodies, arms, open);
        // Its one arm that falls off takes the code after its labelled
        // block, if its block's ways of falling off are that block's.
        // An open one's last arm is the rest of the block, which
        // [`FnPrinter::block`] marks, or takes these marks back from.
        if let Some(l) = mark {
            let k = bodies.len() - usize::from(open);
            let falls: Vec<usize> = (0..k).filter(|&n| !diverges(&bodies[n].0)).collect();
            if falls.len() == 1 || (open && falls.is_empty()) {
                if let [n] = falls[..] {
                    bodies[n].0.push_str(&fall_mark(l));
                }
                for b in &mut bodies[..k] {
                    b.0.push_str(&arm_end(l));
                }
                if open {
                    self.open_mark = Some((l, falls.len()));
                }
            }
        }
        let mut s = format!("{ind}match {} {{\n", v(t.run.value));
        for (n, (b, a)) in bodies.iter().enumerate() {
            let pat = if n + 1 == bodies.len() {
                "_".to_string()
            } else {
                switch::pattern(a)
            };
            let closed = n + 1 < bodies.len() || !open;
            // A one-line arm on one line, an empty one as `{}`.
            if closed && b.trim().is_empty() {
                writeln!(s, "{ind}    {pat} => {{}}").unwrap();
                continue;
            }
            if closed && b.lines().count() == 1 {
                writeln!(s, "{ind}    {pat} => {{ {} }}", b.trim()).unwrap();
                continue;
            }
            write!(s, "{ind}    {pat} => {{\n{b}").unwrap();
            if closed {
                writeln!(s, "{ind}    }}").unwrap();
            }
        }
        if !open {
            writeln!(s, "{ind}}}").unwrap();
        }
        s
    }

    /// Each leaf's code: the run's writes before its exit, then the
    /// exit, or the next block's run's leaves; a run ending at its last
    /// test, the writes before the rest. At the `top` the writes before
    /// the first test are printed before the `match`.
    #[allow(clippy::too_many_arguments)]
    fn switch_bodies(
        &mut self,
        t: &switch::Tree,
        insts: &[Inst],
        xs: &[ExitInfo],
        thumb: bool,
        skip: &[bool],
        cur: u32,
        outs: Option<&[plan::Out]>,
        top: bool,
    ) -> Vec<String> {
        let ind = "                ";
        let from = if top { t.run.tests[0].0 } else { t.run.start };
        let rest = match t.run.end {
            switch::End::Rest(r) => Some(r),
            switch::End::Exit(_) => None,
        };
        let mut out = Vec::new();
        for (n, i) in t.run.exits().chain(rest).enumerate() {
            let mut pre = String::new();
            let unread = self.arm_unread(insts, &t.run, from, i, xs);
            for d in from..i {
                if !t.run.deferred(insts, d) || skip[d] || unread[d - from] {
                    continue;
                }
                let body = match insts[d].op {
                    Op::SetReg(r, x) => {
                        self.soft.reg_writes += u64::from(r.index() < 13);
                        format!("r{} = {};", r.index(), self.whole(x))
                    }
                    Op::SetFlag(fl, x) => format!("{} = {};", flag(fl), self.whole(x)),
                    _ => unreachable!("a run defers register and flag writes"),
                };
                match insts[d].guard {
                    Some(g) => writeln!(pre, "{ind}if {} {{ {body} }}", self.whole(g)).unwrap(),
                    None => writeln!(pre, "{ind}{body}").unwrap(),
                }
            }
            // The rest of the block: at the top the open arm's, which the
            // block goes on printing; else printed here from its inst.
            if Some(i) == rest {
                if !top {
                    // Its block's only code, with what it reads.
                    self.printing(cur);
                    let lets = tree_lets(insts, &t.run, skip, &self.blind_exits(insts, xs), true);
                    self.rest_from = i;
                    self.cur = cur;
                    let saved = self.outs.replace(outs.expect("a structured tree").to_vec());
                    let body = self.block(insts, xs, thumb);
                    self.outs = saved;
                    let lets = unread::read_lets(&lets, &body);
                    self.soft.value_lets += lets.lines().count() as u64;
                    writeln!(pre, "{ind}runtime::guest_pc::set({cur:#010x});").unwrap();
                    pre.push_str(&lets);
                    pre.push_str(&body);
                }
                out.push(pre);
                continue;
            }
            let j = xs
                .iter()
                .position(|y| y.index == i)
                .expect("an exit of the block's");
            match &t.next[n] {
                None => {
                    self.cur = cur;
                    self.edges = outs.map(|o| o[j].edges.clone());
                    let Op::Exit(e) = insts[i].op else {
                        unreachable!("a run's exits are exits")
                    };
                    // Every arm ends the block's code.
                    self.tail_edge = true;
                    let body = format!("{}{}", self.flush(i), self.exit(e, &xs[j], thumb));
                    self.tail_edge = false;
                    self.edges = None;
                    out.push(format!("{pre}{body}"));
                }
                Some(nt) => {
                    let (node, exits) = inline_block(&outs.expect("a structured tree")[j])
                        .expect("a tree's next block is inline");
                    let k = self.key_of(node);
                    let gb = &self.f.blocks[&k];
                    let (n, nthumb) = (&gb.block.insts, gb.block.thumb);
                    let nxs = exits_of_block(self.ctx, gb);
                    let (unread, nskip) = self.unread(k, n.len());
                    // The rest of the block, if any, counts its own.
                    let stop = match nt.run.end {
                        switch::End::Rest(r) => r,
                        switch::End::Exit(_) => n.len(),
                    };
                    for (d, inst) in n[..stop].iter().enumerate() {
                        if let Op::SetFlag(..) = inst.op
                            && !nskip[d]
                        {
                            self.soft.flag_writes += 1;
                            self.soft.dead_flag_writes += u64::from(unread[d]);
                        }
                    }
                    self.soft.compare_tree_blocks += 1;
                    // It prints nothing of its own in the arms
                    // ([`FnPrinter::switch_next`]) but the rest of its
                    // block, if any, in its arm; its values print as their
                    // `let`s there.
                    let outer = std::mem::replace(&mut self.inl, vec![None; n.len()]);
                    for b in self.switch_bodies(nt, n, &nxs, nthumb, &nskip, k, Some(exits), false)
                    {
                        out.push(format!("{pre}{b}"));
                    }
                    self.inl = outer;
                }
            }
        }
        out
    }

    /// Which of a run's writes from inst `from` to its exit at `i` (or
    /// the rest of its block, there) an arm leaves out, by inst from
    /// `from`: those the arm's code doesn't read (unit
    /// `native/lift-plain-arith`). The run's tests, which read its flags,
    /// print as the `match`; so a write is read only if it is the last of
    /// its register or flag before the exit (no unguarded one after it)
    /// and live where the exit goes on (a block of this function's,
    /// [`FnPrinter::dead_writes`]; anywhere else, and the rest of a block,
    /// everything). A write also stays where a read of the same register
    /// or flag follows it within the run. sp, lr and pc always stay.
    fn arm_unread(
        &self,
        insts: &[Inst],
        run: &switch::Run,
        from: usize,
        i: usize,
        xs: &[ExitInfo],
    ) -> Vec<bool> {
        let mut out = vec![false; i.saturating_sub(from)];
        if self.holes.keep_dead_writes {
            return out;
        }
        let x = xs.iter().find(|x| x.index == i);
        let mut need = match x.map(|x| x.plan) {
            Some(Plan::Static(k) | Plan::Rekey(k)) => {
                self.live_in.get(&k).copied().unwrap_or(live::ALL)
            }
            _ => live::ALL,
        };
        need |= live::reg(13) | live::reg(14) | live::reg(15);
        for d in (from..i).rev() {
            let bit = match insts[d].op {
                Op::SetReg(r, _) if run.deferred(insts, d) => live::reg(r.index()),
                Op::SetFlag(f, _) if run.deferred(insts, d) => live::flag(f),
                Op::GetReg(r) => {
                    need |= live::reg(r.index());
                    continue;
                }
                Op::GetFlag(f) => {
                    need |= live::flag(f);
                    continue;
                }
                _ => continue,
            };
            out[d - from] = need & bit == 0;
            if insts[d].guard.is_none() {
                need &= !bit;
            }
        }
        out
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
        if let (Some(&l), Some(edges)) = (self.labels.get(&k), &self.edges) {
            let e = edges
                .iter()
                .find(|e| e.to == l)
                .cloned()
                .unwrap_or_else(|| {
                    panic!("{at:#010x}: a jump to {k:#010x} the plan has no edge for")
                });
            self.edge(&e)
        } else if let Some(l) = self.labels.get(&k) {
            format!("b = {l}; continue;")
        } else if self.all.contains(&k) {
            self.fall_into(k)
        } else {
            self.hole(
                at,
                HoleKind::NoCode,
                format!("a jump to {k:#010x}, where there's no code"),
            );
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

    /// What `k` takes of r4-r11 from its callers (bit `n - 4` for rN,
    /// [`Summary::passed`]): 0 for a stand-in, or a function of another
    /// context (its summary isn't this one's, so this caller's reads of
    /// what it passes went unseen; the plain entry stops the program).
    fn passed(&self, k: u32) -> u8 {
        if self.holes.standins.contains_key(&k) {
            return 0;
        }
        self.sums.get(k).map_or(0, Summary::passed)
    }

    /// The registers of r4-r11 (bit `n - 4` for rN) `k` returns in r0-r3
    /// as they came ([`Summary::echoes`]): a call passes them, so it
    /// returns the caller's, as the machine does. 0 for a stand-in.
    fn echoes(&self, k: u32) -> u8 {
        if self.holes.standins.contains_key(&k) {
            return 0;
        }
        self.sums
            .get(k)
            .map_or(0, |s| s.echoes.values().fold(0, |m, &n| m | 1 << (n - 4)))
    }

    /// What a call passes of `m` (bits of r4-r11 its callee takes): all
    /// of them, or none with `--takes-zeroed` ([`Holes::takes_zeroed`]).
    fn taken(&self, m: u8) -> u8 {
        if self.holes.takes_zeroed { 0 } else { m }
    }

    /// A direct call at `at` to `k`: a hole where `k` takes r4-r11 from
    /// its callers and this call can't be shown to pass them (the
    /// summary's run never reached it, or `k` is another context's).
    fn passes_at(&mut self, at: u32, k: u32) {
        if !self.takers.contains(&k) {
            return;
        }
        let seen = self
            .sums
            .get(self.f.entry)
            .is_some_and(|s| s.passes.contains(&(at, k)));
        if self.passed(k) == 0 || !seen {
            self.hole(at, HoleKind::Takes, format!("a call to {k:#010x}, which takes r4-r11 from its callers, that the summaries didn't see pass them (past a hole in this function, or from another code context)"));
        }
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
        let (m, e) = (self.passed(k), self.echoes(k));
        if m | e != 0 {
            return (self.results_lhs(k), taking(k, self.taken(m) | e));
        }
        let four = (0..4).map(|r| self.reg(r)).collect::<Vec<_>>().join(", ");
        match self.shape(k) {
            None if self.holes.lr.contains(&k) => (
                "[r0, r1, r2, r3] = ".into(),
                format!("{{ g.lr = r14; f_{k:08x}(g, r13, [{four}]) }}"),
            ),
            None => (
                "[r0, r1, r2, r3] = ".into(),
                format!("f_{k:08x}(g, r13, [{four}])"),
            ),
            Some(c) => (
                lhs(c.results),
                format!(
                    "f_{k:08x}(g, r13{})",
                    shape::regs(c.params)
                        .map(|r| format!(", {}", self.reg(r)))
                        .collect::<String>()
                ),
            ),
        }
    }

    /// What a call of `k`'s `x_` entry ([`taking`]) assigns its results
    /// to: its shape's, or all of r0-r3 for a four-register function.
    fn results_lhs(&self, k: u32) -> String {
        match self.shape(k) {
            Some(c) => lhs(c.results),
            None => "[r0, r1, r2, r3] = ".into(),
        }
    }

    /// The call statement: `lhs call?;`.
    fn call_stmt(&self, k: u32) -> String {
        let (lhs, call) = self.call(k);
        format!("{lhs}{call}?;")
    }

    /// Register `r` as the code printing now reads it: what this block
    /// let it be ([`FnPrinter::bound`]), or its local.
    fn reg(&self, r: usize) -> String {
        match self.bound.get(r) {
            Some(Some(b)) => b.clone(),
            _ => format!("r{r}"),
        }
    }

    /// The registers of `m` (bit `n` for rN of r0-r3), each as
    /// [`FnPrinter::reg`] reads it, as a value: `()`, `r0`, `(r0, v3)`.
    fn regs_value(&self, m: u8) -> String {
        let rs: Vec<String> = shape::regs(m).map(|r| self.reg(r)).collect();
        match &rs[..] {
            [] => "()".into(),
            [r] => r.clone(),
            _ => format!("({})", rs.join(", ")),
        }
    }

    /// The return statement.
    fn ret(&self) -> String {
        let e = match self.me {
            None => format!(
                "[{}]",
                (0..4).map(|r| self.reg(r)).collect::<Vec<_>>().join(", ")
            ),
            Some(s) => self.regs_value(s.results),
        };
        format!("return Ok({e});")
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
        // The functions the holes file says this call reaches that take
        // one of r4-r11 (`[[takes]]`): to those, with this function's.
        let mut named: Vec<(u32, u8, bool)> = self
            .holes
            .takes
            .iter()
            .filter(|(_, (sites, _))| sites.contains(&at))
            .map(|(&e, _)| (e, self.passed(e)))
            .filter(|&(e, m)| {
                let seen = |s: &Summary| s.passes.contains(&(at, e));
                m != 0 && self.sums.get(self.f.entry).is_some_and(seen)
            })
            .map(|(e, m)| (e, self.taken(m), true))
            .collect();
        // And those of its callee set that return one of r4-r11 as it
        // came: with that register ([`FnPrinter::echoes`]).
        for &e in self.args.echoes.get(&at).into_iter().flatten() {
            if let Some(n) = named.iter_mut().find(|n| n.0 == e) {
                n.1 |= self.echoes(e);
            } else if self.echoes(e) != 0 {
                named.push((e, self.echoes(e), false));
            }
        }
        if !named.is_empty() {
            let mut t = format!("match {k} {{ ");
            for (e, m, takes) in named {
                if takes {
                    self.soft.passes.insert((at, e));
                }
                write!(
                    t,
                    "{e:#010x} => {{ {}{}?; }} ",
                    self.results_lhs(e),
                    taking(e, m)
                )
                .unwrap();
            }
            write!(t, "k => {{ {} }} }}", call_fn_text("k", pass, back)).unwrap();
            return t;
        }
        call_fn_text(k, pass, back)
    }

    /// Whether exit `x`'s printed code ([`FnPrinter::exit`]) reads its
    /// target value: only a dynamic jump's does, unless it is a plain
    /// return to the entry lr or a call through a register whose one
    /// callee is called directly.
    fn reads_target(&self, x: &ExitInfo) -> bool {
        let Plan::Dynamic { ret, target, .. } = x.plan else {
            return false;
        };
        if ret.is_some() {
            return !target.is_some_and(|tk| self.all.contains(&tk) && !self.takers.contains(&tk));
        }
        let to_lr = self
            .sums
            .get(self.f.entry)
            .is_some_and(|s| s.lr_returns.contains(&x.at));
        !(to_lr && self.lr_ret)
    }

    /// The loads of a block's `insts` whose read has no effect, so that
    /// one nothing uses prints as nothing (unit `native/lift-plain-arith`):
    /// a stack slot's, a typed global's or heap object's, or one the
    /// value-set analysis bounds to RAM, ROM or the display memories. One
    /// that may reach I/O, SRAM or anywhere still prints, as a statement.
    fn quiet_loads(
        &self,
        insts: &[Inst],
        typed: &[Option<Place>],
        objs: &[Option<Obj>],
        refs: &[Option<HeapRef>],
    ) -> Vec<bool> {
        let quiet_region = |r: usize| matches!(metrics::REGIONS[r], "ram" | "rom" | "display");
        let mut at = 0;
        let mut q = vec![false; insts.len()];
        for (i, inst) in insts.iter().enumerate() {
            match inst.op {
                Op::Insn(a) => at = a,
                Op::Load { addr, .. } => {
                    let w = typed_as(&typed[i], &objs[i], refs[i].is_some())
                        .unwrap_or_else(|| self.vsa_where(insts, addr, at));
                    q[i] = match w {
                        Where::Global | Where::Object | Where::Ref | Where::Sp => true,
                        Where::Const(r) | Where::Computed(r) => quiet_region(r),
                        Where::Unknown => false,
                    };
                }
                _ => {}
            }
        }
        q
    }

    /// Whether inst `i`, op `op`, of the block whose uses are counted
    /// prints as nothing where nothing uses it: a pure op or a quiet load
    /// ([`FnPrinter::quiet_loads`]).
    fn gone_if_unused(&self, i: usize, op: &Op) -> bool {
        pure(op) || self.quiet.get(i).copied().unwrap_or(false)
    }

    /// The exits of a block's `insts` whose printed code doesn't read
    /// their target ([`FnPrinter::reads_target`]), by inst.
    fn blind_exits(&self, insts: &[Inst], xs: &[ExitInfo]) -> Vec<bool> {
        let mut b = vec![false; insts.len()];
        for x in xs {
            if x.index < insts.len() && matches!(insts[x.index].op, Op::Exit(_)) {
                b[x.index] = !self.reads_target(x);
            }
        }
        b
    }

    fn exit(&mut self, e: Exit, x: &ExitInfo, thumb: bool) -> String {
        self.tail_for = false;
        let ind = "                    ";
        let at = x.at;
        let regs = "[r0, r1, r2, r3]";
        let mut s = String::new();
        // sp goes to the current mode's bank, as at a store.
        let wait = match x.idle {
            Idle::Edge => "bank_sp(g, r13); hw::idle(g)?; ",
            Idle::Spin => "bank_sp(g, r13); hw::spin(g)?; ",
            Idle::Exit => "bank_sp(g, r13); hw::idle_exit(g)?; ",
            _ => "",
        };
        self.soft.bank_sp_calls += u64::from(!wait.is_empty());
        match x.plan {
            Plan::Static(k) | Plan::Rekey(k) => {
                let j = self.goto(k, at);
                writeln!(s, "{ind}{wait}{j}").unwrap();
            }
            Plan::Call { target, ret } => {
                self.passes_at(at, target);
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
                    Exit::Jump(t) | Exit::JumpExchange(t) => self.val(t),
                    Exit::ExceptionReturn(_) => {
                        self.hole(
                            at,
                            HoleKind::Cpu,
                            "CPU state: an exception return (CPSR = SPSR)",
                        );
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
                    // (One that takes r4-r11 from its callers goes by
                    // `call_fn` all the same: the summaries took this for
                    // an indirect call, so what it would pass went unread.)
                    let c = match target {
                        Some(tk) if self.all.contains(&tk) && !self.takers.contains(&tk) => {
                            self.call_stmt(tk)
                        }
                        Some(tk) if self.all.contains(&tk) => {
                            if !self
                                .sums
                                .get(self.f.entry)
                                .is_some_and(|s| s.passes.contains(&(at, tk)))
                            {
                                self.hole(at, HoleKind::Takes, format!("a call through a register to {tk:#010x}, which takes r4-r11 from its callers: not passed (the holes file's takes can name the call)"));
                            }
                            self.call_fn(at, &k)
                        }
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
                            a, HoleKind::Jump,
                            "an indirect jump Recovery didn't resolve (give its targets in the holes file)",
                        );
                        target.into_iter().collect()
                    }
                };
                // A jump to the entry lr, which is `RET`: a plain return,
                // the popped word left in its register
                // (`native/lift-pop-return`).
                let to_lr = self
                    .sums
                    .get(self.f.entry)
                    .is_some_and(|s| s.lr_returns.contains(&at));
                if to_lr && self.lr_ret {
                    self.soft.lr_returns += 1;
                    writeln!(s, "{ind}{}", self.ret()).unwrap();
                    return s;
                }
                if to_lr {
                    self.soft.lr_returns_call_fn += 1;
                }
                writeln!(s, "{ind}let t: u32 = {t};").unwrap();
                writeln!(s, "{ind}if t & !3 == RET {{ {} }}", self.ret()).unwrap();
                // Anything else is a function's entry, taken as a tail call
                // (`call_fn` panics on any other value): the match's last
                // arm, as a target's jump may fall through (structured).
                let t = self.tail(
                    None,
                    "[r0, r1, r2, r3] = ",
                    &format!("call_fn(g, {k}, r13, {regs})"),
                );
                if targets.is_empty() {
                    writeln!(s, "{ind}{t}").unwrap();
                } else {
                    self.soft.dyn_jumps += 1;
                    writeln!(s, "{ind}match {k} {{").unwrap();
                    let mut tail_for = false;
                    // Ending the block's code, an arm's `break` to where
                    // falling off it goes is its end; arms of the same
                    // code are one (unit `native/lift-join-shape`).
                    let fall = self.fall.filter(|_| self.tail_edge);
                    let mark = self.mark.take();
                    let mut arms: Vec<(Vec<u32>, String)> = Vec::new();
                    for tk in targets {
                        let j = self.goto(tk, at);
                        let j = fall.map_or(j.clone(), |l| fall_off(&j, l));
                        tail_for |= std::mem::take(&mut self.tail_for);
                        match arms.iter_mut().find(|(_, b)| b.trim() == j.trim()) {
                            Some((ks, _)) => ks.push(tk),
                            None => arms.push((vec![tk], j)),
                        }
                    }
                    // Its one arm that falls off takes the code after its
                    // labelled block ([`FnPrinter::seq`]).
                    if let Some(l) = mark {
                        let falls: Vec<usize> =
                            (0..arms.len()).filter(|&n| !diverges(&arms[n].1)).collect();
                        if let [n] = falls[..] {
                            arms[n].1.push_str(&fall_mark(l));
                            for a in &mut arms {
                                a.1.push_str(&arm_end(l));
                            }
                        }
                    }
                    for (ks, j) in arms {
                        let pat: Vec<String> = ks.iter().map(|k| format!("{k:#010x}")).collect();
                        writeln!(s, "{ind}    {} => {{ {j} }}", pat.join(" | ")).unwrap();
                    }
                    self.tail_for = tail_for;
                    writeln!(s, "{ind}    _ => {{ {t} }}").unwrap();
                    writeln!(s, "{ind}}}").unwrap();
                }
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
                    0x02 | 0x03 => {
                        self.soft.bank_sp_calls += 1;
                        "bank_sp(g, r13); hw::halt(g)?;".to_string()
                    }
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
                        self.hole(at, HoleKind::Bios, format!("BIOS call {n:#04x}, which the library doesn't have"));
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
            Plan::Undefined { trap: true } => {
                self.soft.traps.push(at);
                writeln!(
                    s,
                    "{ind}panic!(\"{at:#010x}: a compiler's trap (undefined instruction), which stops the game\");"
                )
                .unwrap()
            }
            Plan::Undefined { trap: false } => {
                self.hole(at, HoleKind::Undefined, "an undefined instruction")
            }
        }
        s
    }
}

/// The deepest a structured function's printed Rust may nest
/// ([`plan::Depth::indent`]); a deeper one stays a `loop { match b }`.
/// Deeper than the mod view's ([`crate::lower::MAX_INDENT`], whose tokens
/// go through `syn`): rustc parses this text itself (agb's `core::fmt`
/// code nests 93 deep).
const MAX_INDENT: usize = 256;

/// Mark the nodes of `seq`'s regions in `in_region`, by label.
fn mark_regions(seq: &[plan::Node], in_region: &mut [bool]) {
    let edges = |es: &[plan::Edge], in_region: &mut [bool]| {
        for e in es {
            if let plan::How::Inline(s) = &e.how {
                mark_regions(s, in_region);
            }
        }
    };
    for n in seq {
        match n {
            plan::Node::Block { exits, .. } => {
                for o in exits {
                    edges(&o.edges, in_region);
                }
            }
            plan::Node::Loop { body, .. } | plan::Node::Labeled { body, .. } => {
                mark_regions(body, in_region)
            }
            plan::Node::Region { blocks, exits, .. } => {
                for b in blocks {
                    if let plan::Node::Block { node, .. } = b {
                        in_region[*node] = true;
                    }
                }
                edges(exits, in_region);
            }
        }
    }
}

/// The report's line on the names files' types (unit
/// `native/mirror-katam-names`): of the constant accesses into EWRAM or
/// IWRAM (`ram`: their offsets in `Game::ram` and widths), those in a
/// typed global, those exactly one of its scalars (an element or field,
/// [`crate::types::Types::walk`]), and those of these inside a struct or
/// union field (`gKirbys[1].base.x`). Counted only: the printer still
/// prints them as `Ram`'s fields ([`globals`]).
fn typed_fields(t: &crate::types::Types, ram: &[(u32, u32)]) -> String {
    let (mut all, mut typed, mut whole, mut field) = (0, 0, 0, 0);
    for &(o, w) in ram {
        let a = globals::guest(o);
        all += 1;
        let Some((k, ty)) = t.global_at(a) else {
            continue;
        };
        typed += 1;
        let Some((p, _)) = t.walk(ty, a - k, w, None) else {
            continue;
        };
        whole += 1;
        if p.iter().any(|s| matches!(s, crate::types::Step::Field(_))) {
            field += 1;
        }
    }
    format!(
        "types (gbaa-lift --names): of {all} constant RAM accesses, {typed} in a typed global, \
         {whole} exactly one of its scalars, {field} a named struct field"
    )
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
    /// Not the heap's: a field of a type the names files give
    /// ([`fields`]), printed over the same `ld*`/`st*` (`f.ty` unused).
    named: bool,
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
    let none = BTreeSet::new();
    let frames = frame::of_code(ctx, funcs, sums, &none);
    let all: BTreeSet<u32> = funcs.iter().map(|f| f.entry).collect();
    let overlays = discovery::overlays(rom, disc);
    let ins = [FlowIn {
        ctx,
        src: 0x0800_0000,
        funcs,
        frames: &frames,
        sums,
    }];
    let (ins, pools, _) = fn_ins(&ins, &all, &none);
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

/// A call of `k`, which takes the registers of `m` (bit `n - 4` for rN,
/// of r4-r11) from its callers: its extended entry with r0-r3 and those
/// of the caller's, the rest as `Regs::call` gives them (without `?`).
fn taking(k: u32, m: u8) -> String {
    let hi: Vec<String> = (4..12)
        .map(|r| {
            if m & 1 << (r - 4) != 0 {
                format!("r{r}")
            } else {
                "0".into()
            }
        })
        .collect();
    format!(
        "x_{k:08x}(g, Regs {{ r: [r0, r1, r2, r3, {}, 0, r13, RET], f: [false; 4] }})",
        hi.join(", ")
    )
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
    /// The callees of each call's set that return one of r4-r11 as it
    /// came ([`Summary::echoes`]): it calls them with that register.
    echoes: BTreeMap<u32, Vec<u32>>,
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
        echoes: BTreeMap::new(),
        union_four: 0,
        four_reg_callee: 0,
    };
    for (&at, cs) in sets {
        let echo: Vec<u32> = cs
            .iter()
            .copied()
            .filter(|c| sums.get(c).is_some_and(|s| !s.echoes.is_empty()))
            .collect();
        if !echo.is_empty() {
            a.echoes.insert(at, echo);
        }
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

/// Whether expression `e` is one term, needing no parentheses as an
/// operand: no space outside brackets, and no prefix operator.
fn one_term(e: &str) -> bool {
    if e.starts_with(['!', '-', '&', '*']) {
        return false;
    }
    let mut depth = 0i32;
    for c in e.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ' ' if depth == 0 => return false,
            _ => {}
        }
    }
    true
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

/// The single block an exit's one planned edge brings inline: its node
/// and its exits' plan.
fn inline_block(o: &plan::Out) -> Option<(usize, &[plan::Out])> {
    let [e] = o.edges.as_slice() else { return None };
    let plan::How::Inline(seq) = &e.how else {
        return None;
    };
    match seq.as_slice() {
        [plan::Node::Block { node, exits }] if *node == e.to => Some((*node, exits.as_slice())),
        _ => None,
    }
}

/// The mark in a `match` arm's code where the code after labelled block
/// `l` goes ([`FnPrinter::seq`]), at its end: gone from every function
/// printed.
fn fall_mark(l: usize) -> String {
    format!("\u{1}fall {l}\u{1}")
}

/// The mark at the end of each arm of the `match` that took a
/// [`fall_mark`], so that a `break` ending one can go once the code after
/// its labelled block is in its arm ([`FnPrinter::seq`]).
fn arm_end(l: usize) -> String {
    format!("\u{2}end {l}\u{2}")
}

/// Whether an arm's code `b` ends in a jump away (`break`, `continue`,
/// `return`), as printed; one that may fall off the end of the arm
/// doesn't (one ending in a block is taken to).
fn diverges(b: &str) -> bool {
    let Some(t) = b.trim_end().strip_suffix(';') else {
        return false;
    };
    let start = t.rfind(['\n', ';', '{', '}']).map_or(0, |i| i + 1);
    let last = t[start..].trim();
    ["break", "continue", "return"]
        .iter()
        .any(|w| last == *w || last.starts_with(&format!("{w} ")))
}

/// An arm's code `b` with the `break` to labelled block `l` that ends it
/// left out: the arm falls to the end of the `match` there.
fn fall_off(b: &str, l: usize) -> String {
    let brk = format!("break 'b{l};");
    let t = b.trim_end();
    match t.strip_suffix(&brk) {
        Some(rest) => {
            let rest = rest.trim_end();
            if rest.is_empty() {
                String::new()
            } else {
                format!("{rest}\n")
            }
        }
        None => b.to_string(),
    }
}

/// A `match`'s arms with those of the same code as one, its patterns
/// theirs in order (unit `native/lift-join-shape`); the last stays last,
/// with those of its code, as the `_` arm. An `open` match's last arm
/// goes on in its block, so it takes in none.
fn merge_arms(
    bodies: Vec<String>,
    arms: &[Vec<(u32, u32)>],
    open: bool,
) -> Vec<(String, Vec<(u32, u32)>)> {
    let n = bodies.len();
    let mut out: Vec<(String, Vec<(u32, u32)>)> = Vec::new();
    let last = bodies.last().cloned();
    for (i, (b, a)) in bodies.into_iter().zip(arms).enumerate() {
        let to_last = i + 1 < n && !open && last.as_ref().is_some_and(|l| l.trim() == b.trim());
        if to_last {
            continue;
        }
        match out
            .iter_mut()
            .take(if i + 1 == n { 0 } else { usize::MAX })
            .find(|(o, _)| o.trim() == b.trim())
        {
            Some((_, p)) => p.extend(a),
            None => out.push((b, a.clone())),
        }
    }
    // The ranges of each arm sorted and joined.
    for (_, p) in &mut out {
        p.sort_unstable();
        let mut joined: Vec<(u32, u32)> = Vec::new();
        for &(lo, hi) in p.iter() {
            match joined.last_mut() {
                Some((_, h)) if h.wrapping_add(1) == lo && *h != u32::MAX => *h = hi,
                _ => joined.push((lo, hi)),
            }
        }
        *p = joined;
    }
    out
}

/// A block's exits ([`exits`]), named apart from the planned `exits`.
fn exits_of_block(ctx: &Ctx, gb: &crate::func::GBlock) -> Vec<ExitInfo> {
    exits(ctx, gb)
}

/// The reads and pure values a compare run printed inside another's arm
/// needs ([`FnPrinter::switch_bodies`]), in order: with `rest`, those the
/// rest of its block takes (the block's printer prints the rest itself);
/// else those its printed writes and guards and its exits (those `blind`
/// doesn't mark, [`FnPrinter::blind_exits`]) take.
fn tree_lets(
    insts: &[Inst],
    run: &switch::Run,
    skip: &[bool],
    blind: &[bool],
    rest: bool,
) -> String {
    let ind = "                ";
    let mut need = vec![false; insts.len()];
    let stop = match run.end {
        switch::End::Rest(r) => r,
        switch::End::Exit(_) => insts.len(),
    };
    if rest {
        for inst in &insts[stop..] {
            for x in inst.op.operands().into_iter().chain(inst.guard) {
                if x.index() < stop {
                    need[x.index()] = true;
                }
            }
        }
    }
    for (i, inst) in insts.iter().enumerate().take(stop).rev() {
        let root = !rest
            && ((run.deferred(insts, i) && !skip[i])
                || (i >= run.start && matches!(inst.op, Op::Exit(_))));
        if !(root || need[i]) {
            continue;
        }
        if root {
            let guard = inst.guard.filter(|_| !run.exit(i));
            // An exit whose printed code doesn't read its target.
            let ops = match inst.op {
                Op::Exit(_) if blind.get(i).copied().unwrap_or(false) => Vec::new(),
                op => op.operands(),
            };
            for x in ops.into_iter().chain(guard) {
                need[x.index()] = true;
            }
        } else {
            for x in inst.op.operands() {
                need[x.index()] = true;
            }
        }
    }
    let mut s = String::new();
    for (i, inst) in insts.iter().enumerate().take(stop) {
        if !need[i] || matches!(inst.op, Op::Exit(_)) {
            continue;
        }
        let id = v(ValueId(i as u32));
        match inst.op {
            Op::GetReg(r) => writeln!(s, "{ind}let {id}: u32 = r{};", r.index()).unwrap(),
            Op::GetFlag(fl) => writeln!(s, "{ind}let {id}: bool = {};", flag(fl)).unwrap(),
            ref op => {
                let ty = if is_bit(op) { "bool" } else { "u32" };
                writeln!(s, "{ind}let {id}: {ty} = {};", expr(op, &v, &v)).unwrap();
            }
        }
    }
    s
}

/// The words of `text` that may name a local: identifiers not after a `.`
/// and not starting with a digit.
fn rust_words(text: &str) -> BTreeSet<String> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < b.len() {
        if !word(b[i]) {
            i += 1;
            continue;
        }
        let s = i;
        while i < b.len() && word(b[i]) {
            i += 1;
        }
        if !b[s].is_ascii_digit() && (s == 0 || b[s - 1] != b'.') {
            out.insert(text[s..i].to_string());
        }
    }
    out
}

/// The registers of r0-r3 (bit `n` for rN) BIOS call `n`'s printed code
/// ([`FnPrinter::exit`]'s `Plan::Swi`) reads: `VBlankIntrWait` none, as
/// it sets its own. All four for one the library doesn't have (a hole).
fn swi_reads(n: u8) -> u8 {
    match n {
        0x00 | 0x02 | 0x03 | 0x05 | 0x25 | 0x28 => 0,
        0x01 | 0x08 | 0x19 => 0b1,
        0x04 | 0x0A => 0b11,
        0x1F => 0b111,
        _ => 0xf,
    }
}

/// The registers of r0-r3 BIOS call `n`'s printed code ([`swi_reads`])
/// sets whatever it reads: its results.
fn swi_writes(n: u8) -> u8 {
    match n {
        0x04 | 0x08 | 0x1F | 0x25 => 0b1,
        0x05 => 0b11,
        0x0A => 0b1011,
        0x06 | 0x0B | 0x0C | 0x0E | 0x0F | 0x11..=0x15 => 0xf,
        _ => 0,
    }
}

/// 1 if printed code `body` reads one of the registers and flags `taken`
/// nowhere ([`reads_name`]), else 0: `unpacked_unread`'s count of one
/// function.
fn taken_unread<'a>(body: &str, mut taken: impl Iterator<Item = &'a str>) -> usize {
    usize::from(taken.any(|name| !reads_name(body, name)))
}

/// Whether printed code `body` reads `name` anywhere: the check behind
/// `unpacked_unread`, made on the text alone, apart from the liveness that
/// decided what an entry takes. Each statement (split at `;`, `{`, `}` and
/// line ends; a match arm's pattern apart from its code) reads the names
/// in it but the one a `let`, a plain `x = ` or a tuple's or array's
/// assignment (`[r0, r1] = `) writes; `r13 = r13.wrapping_add(4)` and
/// `r1 += 1` read theirs. Text order is not control order (a loop's
/// write comes before the read at its exit the loop may skip), so a read
/// after a write still counts.
fn reads_name(body: &str, name: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"^\s*(?:let\s+)?(?:mut\s+)?(?:([A-Za-z_][A-Za-z0-9_]*)(?:\s*:\s*[^=]+)?|[\[(]([^\[\]()=]*)[\])])\s*=(?:[^=]|$)",
        )
        .expect("a valid pattern")
    });
    let names = |t: &str| rust_words(t).contains(name);
    for line in body.split(['\n', ';', '{', '}']) {
        // A match arm's pattern, then its code.
        let (pat, st) = line.split_once("=>").unwrap_or(("", line));
        if names(pat) {
            return true;
        }
        let written = re.captures(st).and_then(|c| {
            let whole = c.get(0)?;
            let hit = match (c.get(1), c.get(2)) {
                (Some(w), _) => w.as_str() == name,
                (_, Some(ws)) => rust_words(ws.as_str()).contains(name),
                _ => false,
            };
            hit.then(|| whole.end() - 1)
        });
        let rest = written.map_or(st, |eq| &st[eq..]);
        if names(rest) {
            return true;
        }
    }
    false
}

/// The locals `text` assigns: `x = `, `x += `, and each name of a tuple's
/// or array's assignment (`[r0, r1] = `), or of a `for mut x`.
fn written_words(text: &str) -> BTreeSet<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"(?:^|[^A-Za-z0-9_.])(?:([A-Za-z_][A-Za-z0-9_]*)|[\[(]([^\[\]()=;]*)[\])]) *(?:[-+*/%^&|]|<<|>>)?= *[^=]",
        )
        .expect("a valid pattern")
    });
    let mut out = BTreeSet::new();
    for c in re.captures_iter(text) {
        if let Some(w) = c.get(1) {
            out.insert(w.as_str().to_string());
        }
        if let Some(ws) = c.get(2) {
            out.extend(rust_words(ws.as_str()));
        }
    }
    out
}

/// Whether `e` is a value's name (`v7`) or a constant (`0x10u32`): what
/// a register's `let` may stand for in its reads unprinted.
fn plain_value(e: &str) -> bool {
    let name = e
        .strip_prefix('v')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    let constant = e.strip_suffix("u32").is_some_and(|d| {
        d.strip_prefix("0x").map_or_else(
            || !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()),
            |h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()),
        )
    });
    name || constant
}

/// A pure op as a Rust expression over earlier values, each printed by
/// `v` as an operand and `w` as a call's argument or a branch (a `vN`, or
/// its expression where it is inlined: [`FnPrinter::val`],
/// [`FnPrinter::whole`]).
fn expr(op: &Op, v: &dyn Fn(ValueId) -> String, w: &dyn Fn(ValueId) -> String) -> String {
    use Op::*;
    match *op {
        Const(c) | Pool(c) => format!("{c:#x}u32"),
        Bit(b) => format!("{b}"),
        GetReg(r) => format!("r{}", r.index()),
        GetFlag(f) => flag(f).to_string(),
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
        MulLongLo { signed, a, b } => format!("prim::mul_long({signed}, {}, {}).0", w(a), w(b)),
        MulLongHi { signed, a, b } => format!("prim::mul_long({signed}, {}, {}).1", w(a), w(b)),
        // Where no form names it ([`narrow::form`]), a sum as it is.
        Adc { a, b, c } => format!(
            "{}.wrapping_add({}).wrapping_add(u32::from({}))",
            v(a),
            w(b),
            w(c)
        ),
        AdcCarry { a, b, c } => format!("prim::add_with_carry({}, {}, {}).1", w(a), w(b), w(c)),
        AdcOverflow { a, b, c } => {
            format!("prim::add_with_carry({}, {}, {}).2", w(a), w(b), w(c))
        }
        MulCarry {
            kind,
            a,
            b,
            acc_lo,
            acc_hi,
        } => format!(
            "prim::mul_carry(MulKind::{kind:?}, {}, {}, ({} as u64) << 32 | {} as u64)",
            w(a),
            w(b),
            v(acc_hi),
            v(acc_lo)
        ),
        Shift { kind, v: x, amount } => format!(
            "prim::shift(ShiftType::{}, {}, {}, false).0",
            shift(kind),
            w(x),
            w(amount)
        ),
        ShiftCarry {
            kind,
            v: x,
            amount,
            carry_in,
        } => format!(
            "prim::shift(ShiftType::{}, {}, {}, {}).1",
            shift(kind),
            w(x),
            w(amount),
            w(carry_in)
        ),
        Rrx { v: x, carry_in } => format!("prim::rrx({}, {}).0", w(x), w(carry_in)),
        Sext { v: x, bits } => format!("prim::sext({}, {bits})", w(x)),
        Select { c, a, b } | BSelect { c, a, b } => {
            format!("if {} {{ {} }} else {{ {} }}", w(c), w(a), w(b))
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
    fn a_taken_register_the_printed_code_never_reads_is_counted() {
        // r1 is only written; r0 is read after its write, as a loop's
        // exit may read the value it came with; r13's write reads it.
        let body = "    r1 = 5;\n    r0 = ld32(g, r1);\n    r13 = r13.wrapping_add(4);\n    return Ok(r0);\n";
        assert!(reads_name(body, "r0"));
        assert!(reads_name(body, "r13"));
        assert_eq!(taken_unread(body, ["r0", "r13"].into_iter()), 0);
        let only_written =
            "    r1 = 5;\n    [r2, r3] = f(g);\n    let r4 = 0;\n    return Ok(r0);\n";
        for r in ["r1", "r2", "r3", "r4"] {
            assert!(!reads_name(only_written, r), "{r}");
        }
        assert_eq!(taken_unread(only_written, ["r0", "r1"].into_iter()), 1);
        assert_eq!(taken_unread(only_written, ["r0"].into_iter()), 0);
        // A compound write, a match arm's pattern and a condition read.
        assert!(reads_name("    r1 += 1;\n", "r1"));
        assert!(reads_name("    match r2 { 0 => { r2 = 1; } }\n", "r2"));
        assert!(reads_name("    5 => r3 = r3 | 1,\n", "r3"));
        assert!(reads_name("    if cf { r0 = 1; }\n", "cf"));
        assert!(!reads_name("    cf = v7 >= 100;\n", "cf"));
    }

    #[test]
    fn a_break_ending_an_arm_where_it_falls_anyway_goes() {
        assert_eq!(fall_off("    x = 1;\n    break 'b6;\n", 6), "    x = 1;\n");
        assert_eq!(fall_off("break 'b6;", 6), "");
        // Another label's, or one not at the end, stays.
        assert_eq!(fall_off("break 'b16;\n", 6), "break 'b16;\n");
        assert_eq!(fall_off("break 'b6; x = 1;\n", 6), "break 'b6; x = 1;\n");
    }

    #[test]
    fn an_arm_diverges_only_when_its_last_statement_jumps_away() {
        assert!(diverges("    x = 1;\n    break 'b3;\n"));
        assert!(diverges("f(g)?; return Ok(r0);"));
        assert!(diverges("continue 'l2;"));
        assert!(!diverges(""));
        assert!(!diverges("x = 1;"));
        // A block may fall off its end: taken to.
        assert!(!diverges("if v { return Ok(r0); }"));
        assert!(!diverges("let returned: u32 = 1;"));
    }

    #[test]
    fn arms_of_the_same_code_are_one_and_the_last_takes_its_own() {
        let arms = vec![
            vec![(1, 1)],
            vec![(2, 4)],
            vec![(5, 5)],
            vec![(6, u32::MAX), (0, 0)],
        ];
        let bodies: Vec<String> = ["a", "b", "a", "b"].map(String::from).to_vec();
        let m = merge_arms(bodies.clone(), &arms, false);
        // 2..=4 is the last's code: `_` takes it.
        assert_eq!(
            m,
            vec![
                ("a".to_string(), vec![(1, 1), (5, 5)]),
                ("b".to_string(), vec![(0, 0), (6, u32::MAX)]),
            ]
        );
        // An open `match`'s last arm goes on in its block: none join it.
        let m = merge_arms(bodies, &arms, true);
        assert_eq!(m.len(), 3);
        assert_eq!(m[1], ("b".to_string(), vec![(2, 4)]));
    }

    /// A ROM whose reset jumps by `ldr r3, =0x03000000; bx r3` into code
    /// a `[[copy]]` puts in IWRAM, `ldr r0, [r4]; bx lr`: it reads r4 as
    /// it came. `first` and `before_jump` are the reset's first
    /// instruction and the one before its `bx`; `pointer`, whether the
    /// copy's address is also a word of the ROM besides the jump's literal.
    fn jump_into_a_copy(first: u32, before_jump: u32, pointer: bool) -> Vec<Hole> {
        let mut words = vec![0xE1A0_0000u32; 0x48];
        words[0] = first;
        if first == BL_SHARER {
            // The called function loads the jump's literal too (one pool
            // entry shared) and stores it: `ldr r1, [pc, #-0x14]` (the word
            // at 0x14); `str r1, [r0]`; `bx lr`.
            words[8] = 0xE51F_1014;
            words[9] = 0xE580_1000;
            words[10] = 0xE12F_FF1E;
        }
        words[1] = 0xE59F_3008; // ldr r3, [pc, #8]: the word at 0x14
        words[2] = before_jump;
        words[3] = 0xE12F_FF13; // bx r3
        words[4] = 0xEAFF_FFFE; // b .
        words[5] = 0x0300_0000;
        words[6] = if pointer { 0x0300_0000 } else { 0 };
        words[0x40] = 0xE594_0000; // ldr r0, [r4]
        words[0x41] = 0xE12F_FF1E; // bx lr
        let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut holes = Holes::parse(
            "[[copy]]\nsrc = 0x08000100\ndst = 0x03000000\nlen = 8\nroots = [0x03000000]\nwhy = \"t\"\n",
        )
        .unwrap();
        holes.holes_only = true;
        lift(&rom, "t", &holes).holes
    }

    /// A shift pair prints as a cast, and an `and` with a register a block
    /// before set to 0xff as `as u8` (native/lift-narrowing-casts).
    #[test]
    fn narrowing_idioms_print_as_casts() {
        let mut words = [0xE1A0_0000u32; 0x10];
        words[0] = 0xE1A0_0C01; // mov r0, r1, lsl #24
        words[1] = 0xE1A0_0C40; // mov r0, r0, asr #24
        words[2] = 0xE581_0000; // str r0, [r1]
        words[3] = 0xE3A0_20FF; // mov r2, #0xff
        words[4] = 0xEAFF_FFFF; // b 0x08000014
        words[5] = 0xE001_3002; // and r3, r1, r2
        words[6] = 0xE581_3000; // str r3, [r1]
        words[7] = 0xEAFF_FFFE; // b .
        let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let l = lift(&rom, "t", &Holes::parse("").unwrap());
        let text: String = l.files.iter().map(|(_, t)| t.as_str()).collect();
        let reset = &text[text.find("pub fn x_08000000").expect("the reset")..];
        let reset = &reset[..reset.find("\n}\n").unwrap()];
        assert!(reset.contains(" as i8 as u32"), "{reset}");
        assert!(reset.contains(" as u8 as u32"), "{reset}");
        assert!(!reset.contains("prim::shift"), "{reset}");
        assert_eq!(l.metrics.get("shift_pairs_left"), Some(0));
        assert_eq!(l.metrics.get("narrowing_casts"), Some(2));
        assert!(
            l.report
                .contains("0 of the guest's 1 shift pairs still printed"),
            "{}",
            l.report
        );
    }

    /// The reset of [`a_loop_lifts_as_a_rust_loop`], then `words` from
    /// 0x0800000c, lifted with `holes`: its text and metrics.
    fn countdown(after: &[u32], holes: &Holes) -> (String, Metrics) {
        let mut words = [0xE1A0_0000u32; 0x10];
        words[0] = 0xE3A0_000A; // mov r0, #10
        words[1] = 0xE250_0001; // subs r0, r0, #1
        words[2] = 0x1AFF_FFFD; // bne 0x08000004
        words[3..3 + after.len()].copy_from_slice(after);
        let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let l = lift(&rom, "t", holes);
        let text: String = l.files.iter().map(|(_, t)| t.as_str()).collect();
        let reset = &text[text.find("pub fn x_08000000").expect("the reset")..];
        (reset[..reset.find("\n}\n").unwrap()].to_string(), l.metrics)
    }

    /// A reset that counts r0 down from 10 in a loop and then spins
    /// prints from its structuring plan: a labeled loop and no `match b`,
    /// the countdown a `for` over its range (`native/lift-for-loops`), its
    /// step left out; with `--no-for-loops`, a `loop`.
    #[test]
    fn a_loop_lifts_as_a_rust_loop() {
        const SPIN: u32 = 0xEAFF_FFFE; // b .
        let (reset, m) = countdown(&[SPIN], &Holes::parse("").unwrap());
        assert!(
            reset.contains("'l1: for r0 in (1_u32..=10).rev() {"),
            "{reset}"
        );
        let body = &reset[reset.find("'l1:").unwrap()..];
        assert!(!body.contains("r0 = "), "the step is left out: {reset}");
        assert!(!reset.contains("match b"), "{reset}");
        assert_eq!(m.get("functions_in_match"), Some(0));
        assert_eq!(m.get("blocks_structured_pct"), Some(100));
        assert_eq!(m.get("loops_for"), Some(1));
        let old = Holes {
            no_for_loops: true,
            ..Holes::parse("").unwrap()
        };
        let (reset, m) = countdown(&[SPIN], &old);
        assert!(reset.contains("'l1: loop {"), "{reset}");
        assert_eq!(m.get("loops_for"), Some(0));
    }

    /// A countdown whose register a block after the loop reads stays a
    /// `loop`: the `for` binding would hide its last value.
    #[test]
    fn a_counter_read_after_its_loop_stays_a_loop() {
        let after = [
            0xEAFF_FFFF, // b 0x08000010, a block of its own
            0xE3A0_1403, // mov r1, #0x03000000
            0xE581_0000, // str r0, [r1]
            0xEAFF_FFFE, // b .
        ];
        let (reset, m) = countdown(&after, &Holes::parse("").unwrap());
        assert!(reset.contains("'l1: loop {"), "{reset}");
        assert_eq!(m.get("loops_for"), Some(0));
    }

    /// A value read from r0 and used once prints inside its use, but not
    /// past a write of r0 between them: there it stays a `let`.
    #[test]
    fn a_single_use_value_prints_inside_its_use_unless_a_write_comes_between() {
        let mut words = [0xE1A0_0000u32; 0x10];
        words[0] = 0xE280_1001; // add r1, r0, #1
        words[1] = 0xE3A0_0000; // mov r0, #0
        words[2] = 0xE3A0_2403; // mov r2, #0x03000000
        words[3] = 0xE582_1000; // str r1, [r2]
        words[4] = 0xE3A0_1000; // mov r1, #0
        words[5] = 0xEB00_0001; // bl 0x08000020 (reads r0)
        words[6] = 0xEAFF_FFFE; // b .
        words[8] = 0xE280_0001; // add r0, r0, #1
        words[9] = 0xE12F_FF1E; // bx lr
        let rom: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let print = |holes: &Holes| {
            let l = lift(&rom, "t", holes);
            let text: String = l.files.iter().map(|(_, t)| t.as_str()).collect();
            let reset = &text[text.find("pub fn x_08000000").expect("the reset")..];
            (reset[..reset.find("\n}\n").unwrap()].to_string(), l.metrics)
        };
        let (reset, m) = print(&Holes::parse("").unwrap());
        let sum = reset
            .find(": u32 = r0.wrapping_add(0x1);")
            .unwrap_or_else(|| panic!("r0 + 1 a let: {reset}"));
        // r0's new value, only the call reads, is its argument
        // (native/lift-register-locals).
        let write = reset
            .find("(g, r13, 0x0u32)")
            .unwrap_or_else(|| panic!("r0's new value the call's argument: {reset}"));
        assert!(sum < write, "{reset}");
        // Constants print at their use.
        assert!(!reset.contains(": u32 = 0x"), "{reset}");
        let (old, old_m) = print(&Holes {
            keep_temps: true,
            ..Holes::parse("").unwrap()
        });
        assert!(old.contains(": u32 = 0x"), "{old}");
        assert!(m.get("value_lets") < old_m.get("value_lets"));
    }

    /// `bl 0x08000020`, the start of a function [`jump_into_a_copy`] gives
    /// a load and store of the jump's literal.
    const BL_SHARER: u32 = 0xEB00_0006;

    #[test]
    fn code_in_a_copy_only_jumped_into_takes_the_jumpers_registers() {
        const NOP: u32 = 0xE1A0_0000;
        const SET_R4: u32 = 0xE3A0_4402; // mov r4, #0x02000000
        let text = |hs: Vec<Hole>| hs.iter().map(|h| h.to_string()).collect::<Vec<_>>();
        // The jumper sets r4, and no word but the jump's literal holds the
        // copy's address: no pointer call reaches it.
        assert_eq!(
            text(jump_into_a_copy(SET_R4, NOP, false)),
            Vec::<String>::new()
        );
        // A pointer may call it: r4 zeroed.
        let hs = text(jump_into_a_copy(SET_R4, NOP, true));
        assert!(
            hs.iter().any(|h| h.contains("reads r4-r11 at entry: r4")),
            "{hs:?}"
        );
        // The literal stored too (`str r3, [r0]`): the address escapes.
        let hs = text(jump_into_a_copy(SET_R4, 0xE580_3000, false));
        assert!(
            hs.iter().any(|h| h.contains("reads r4-r11 at entry: r4")),
            "{hs:?}"
        );
        // Another function loads the same pool entry and stores it: the
        // literal isn't the jump's alone (the jumper sets r4 just before
        // the jump, so only the pointer call zeroes it).
        let hs = text(jump_into_a_copy(BL_SHARER, SET_R4, false));
        assert!(
            hs.iter().any(|h| h.contains("reads r4-r11 at entry: r4")),
            "{hs:?}"
        );
        // The jumper hands on the r4 it came with: its callers', which no
        // summary passes.
        let hs = text(jump_into_a_copy(NOP, NOP, false));
        assert!(
            hs.iter().any(|h| h
                .contains("a jump into 0x03000000, another code context's, with r4 as they came")),
            "{hs:?}"
        );
    }

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
    fn the_reset_and_installed_irq_handlers_print_their_psr_ops() {
        let p = PsrFns {
            wrappers: BTreeSet::from([0x0300_0c7c, 0x0800_0104]),
        };
        assert_eq!(p.place(RESET), Some(PsrPlace::Boot));
        assert_eq!(p.place(0x0800_0104), Some(PsrPlace::Wrapper));
        assert_eq!(p.place(0x0300_0c7c), Some(PsrPlace::Wrapper));
        // A critical section's elsewhere stays a hole.
        assert_eq!(p.place(0x0800_2728), None);
    }

    #[test]
    fn holes_file_parses_a_standins_check() {
        let t = "[[standin]]\nentry = 0x0800017d\nwhy = \"w\"\ncode = \"\"\ncheck = { seed = 7, digest = \"00ff\" }\n";
        let h = Holes::parse(t).unwrap();
        assert_eq!(
            h.checks[&0x0800_017d],
            FnCheck {
                seed: 7,
                runs: 2000,
                digest: Some(0xFF)
            }
        );
        assert!(Holes::parse(&t.replace("\"00ff\"", "\"zz\"")).is_err());
        assert!(Holes::parse(&t.replace("seed = 7", "seed = \"7\"")).is_err());
    }

    #[test]
    fn prefix_fn_renames_one_functions_entries() {
        let t = "pub fn f_08000100(g: &mut Game) { x_08000100(g) }\npub fn x_08000100(g: &mut Game) { x_08000100(g); ax_08000100(g); x_081000100; f_08000100(g) }\n";
        assert_eq!(
            prefix_fn(t, 0x0800_0100, "ref_"),
            "pub fn ref_f_08000100(g: &mut Game) { ref_x_08000100(g) }\npub fn ref_x_08000100(g: &mut Game) { ref_x_08000100(g); ax_08000100(g); x_081000100; f_08000100(g) }\n"
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
        let h = Holes::parse(
            "[[stack]]\nstart = 0x0203bee0\nend = 0x0203ffe0\nwhy = \"task stacks\"\n",
        )
        .unwrap();
        assert_eq!(
            h.stacks,
            vec![SeenStack {
                start: 0x0203_bee0,
                end: 0x0203_ffe0,
                why: "task stacks".into(),
            }]
        );
        assert!(Holes::parse("[[stack]]\nstart = 0x10\nend = 0x10\nwhy = \"x\"\n").is_err());
        assert!(Holes::parse("[[stack]]\nstart = 0x10\nwhy = \"x\"\n").is_err());
        let h = Holes::parse(
            "[[takes]]\nentry = 0x08000015\nsites = [0x08000006, 0x08000020]\nwhy = \"its caller's r5\"\n",
        )
        .unwrap();
        assert_eq!(h.takes[&0x0800_0015].0, vec![0x0800_0006, 0x0800_0020]);
        assert!(Holes::parse("[[takes]]\nentry = 0x08000015\nwhy = \"no sites\"\n").is_err());
        // No sites: code only jumped into.
        let h = Holes::parse("[[takes]]\nentry = 0x08000015\nsites = []\nwhy = \"jumped into\"\n")
            .unwrap();
        assert!(h.takes[&0x0800_0015].0.is_empty());
        assert_eq!(
            taking(0x0800_0015, 0b10_0010),
            "x_08000015(g, Regs { r: [r0, r1, r2, r3, 0, r5, 0, 0, 0, r9, 0, 0, 0, r13, RET], f: [false; 4] })"
        );
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
        let expr = |op: &Op| super::expr(op, &super::v, &super::v);
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
