//! The stage-A lifter (`docs/native-port-design.md`, sections 1-6, stage
//! A; unit `native/lift-stage-a`): a whole program printed from the IR as
//! plain Rust over byte RAM and the hardware library, with no `Machine`,
//! no CPU, no cycles.
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
//!   r4-r11 outlive a call because callees keep them (the summaries' proof,
//!   `keeps`; a function that returns without it is a hard hole).
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
//!   `write16`), its palette, VRAM and OAM, and the ROM's bytes.
//! - BIOS calls are the library's: the waits (`IntrWait`, `VBlankIntrWait`
//!   as `hw::halt` until the game's handler sets the flag), `CpuSet` with
//!   its time (`hw::spend`), `Div`.
//! - An idle loop's back edge calls `hw::idle`, a spin loop's `hw::spin`
//!   (`analysis::idle`, `runtime::spins`, as the play build's
//!   `--timing native` marks them).
//! - Function pointers are their keys (ROM or IWRAM address, Thumb bit):
//!   `call_fn` (prelude) looks them up in `FNS`. Stage A has no dense IDs
//!   (`--fn-ids rom` only): a literal lift loads pointers from ROM bytes
//!   and RAM, so it can't renumber them.
//!
//! **Holes.** A function the lift can't print is a hard hole, named, and
//! the lift fails (and the crate, which gets a `compile_error!` for it),
//! unless the game's holes file gives a stand-in or, for an unresolved
//! jump, its targets ([`Holes`]):
//!
//! - CPU state: `mrs`, `msr`, user-bank transfers, exception returns;
//! - an indirect jump Recovery didn't resolve;
//! - a BIOS call the prelude doesn't have;
//! - an undefined instruction, a jump to no code;
//! - a function that returns without keeping r4-r11 and sp.
//!
//! Soft holes (counted in the report): every RAM access is bytes, every
//! pointer a guest address, every ROM read at run time.

use crate::func::{Ctx, ExitInfo, Func, Idle, Plan, build_func, exits, key};
use crate::saves::{Summaries, Summary, summarize};
use analysis::discovery::{self, Discovery};
use analysis::ir::{BinOp, BitOp, Exit, Inst, Op, ValueId};
use analysis::vsa::func::JumpRes;
use armv4t::sem::{Flag, ShiftType, Width};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub mod lift_check;
pub mod metrics;
pub mod shape;

use metrics::{Metrics, Where};
use shape::{Shape, Shapes};

/// The prelude every lifted crate starts with.
pub const PRELUDE: &str = include_str!("native/prelude.rs");

/// The BIOS calls the prelude has.
const BIOS: &[u8] = &[0x02, 0x04, 0x05, 0x06, 0x0B];

/// What the game's holes file (`holes/<game>.toml`) gives: hand-written
/// stand-ins for functions, and targets for unresolved jumps.
#[derive(Clone, Debug, Default)]
pub struct Holes {
    /// By function key: the Rust that replaces it (a `pub fn f_<key>` of
    /// the lifted signature), and why.
    pub standins: BTreeMap<u32, (String, String)>,
    /// By the jump's instruction address: its targets (keys), and why.
    pub jumps: BTreeMap<u32, (Vec<u32>, String)>,
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
}

impl Holes {
    /// Parse a holes file: `[[standin]] entry = 0x…, why = "…", code =
    /// '''…'''` and `[[jump]] at = 0x…, targets = [0x…], why = "…"`.
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
            let list = v.as_array().ok_or(format!("{k}: not a list of tables"))?;
            for e in list {
                let e = e.as_table().ok_or(format!("{k}: not a table"))?;
                match k.as_str() {
                    "standin" => {
                        let entry = int(e.get("entry").ok_or("standin: missing entry")?, "entry")?;
                        h.standins
                            .insert(entry, (str_of(e, "code")?, str_of(e, "why")?));
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
                    _ => return Err(format!("unknown table {k}")),
                }
            }
        }
        Ok(h)
    }
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
    /// The crate's `src/main.rs`.
    pub main: String,
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
}

/// Lift `rom` whole. `name` is the program's name for its front end.
pub fn lift(rom: &[u8], name: &str, holes: &Holes) -> Lifted {
    let mut disc = discovery::discover(rom.to_vec());
    // Discovery's gaps: a jump into ROM where it found no code (a scanned
    // fragment it stopped short in, a `bl`'s second half it took for a
    // literal pool). Each such target is decoded as code from there, and
    // again from what that reaches, so the lift has the code the play
    // build would hand to its interpreter. Listed in the report.
    let mut gaps = BTreeSet::new();
    for _ in 0..64 {
        let ctx = Ctx::new(rom, &disc);
        let funcs: Vec<Func> = ctx.entries.iter().map(|&e| build_func(&ctx, e)).collect();
        let missing = missing(&ctx, &funcs);
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
    let overlays = discovery::overlays(rom, &disc);
    let mut codes = vec![code(Ctx::new(rom, &disc))];
    for o in &overlays {
        let o_rom = &rom[(o.src - 0x0800_0000) as usize..][..o.len as usize];
        codes.push(code(Ctx::at(o.dst, o_rom, &o.disc)));
    }
    // Every function, by key, over all contexts.
    let mut all: BTreeSet<u32> = BTreeSet::new();
    for c in &codes {
        all.extend(c.funcs.iter().map(|f| f.entry));
    }
    let shapes = shapes_of(&codes, &all, holes);
    let mut out = String::new();
    let mut report = String::new();
    let mut hard = Vec::new();
    let mut soft = Soft::default();
    let mut fns = Vec::new();
    let entry = key(0x0800_0000, false);
    for c in &codes {
        for f in &c.funcs {
            fns.push(f.entry);
            if let Some((code, why)) = holes.standins.get(&f.entry) {
                writeln!(
                    out,
                    "// f_{:08x}: a stand-in from the holes file: {why}",
                    f.entry
                )
                .unwrap();
                out.push_str(code);
                out.push('\n');
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
            };
            let s = c.sums.get(f.entry);
            let text = p.print(s);
            if p.found.is_empty() {
                out.push_str(&text);
            } else {
                for h in &p.found {
                    writeln!(
                        out,
                        "compile_error!(\"{}\");",
                        h.to_string().replace('"', "'")
                    )
                    .unwrap();
                }
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
    main.push_str("#![allow(unused_mut, unused_parens, unused_variables, unused_assignments, unreachable_code, non_snake_case, clippy::all)]\n\n");
    if holes.irq_at_waits {
        const HOLD: &str = "pub const IRQ_HOLD: bool = true;";
        debug_assert!(PRELUDE.contains(HOLD));
        main.push_str(&PRELUDE.replace(HOLD, "pub const IRQ_HOLD: bool = false;"));
    } else {
        main.push_str(PRELUDE);
    }
    writeln!(main, "\npub const NAME: &str = {name:?};").unwrap();
    writeln!(
        main,
        "pub const ENTRY: Fn = {};\n",
        plain(&shapes.shaped, entry)
    )
    .unwrap();
    main.push_str(&table);
    main.push_str("\nuse armv4t::sem::prim;\nuse armv4t::sem::ShiftType;\nuse armv4t::sem::prim::MulKind;\n\n");
    main.push_str(&out);
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
    let mut m = Metrics::default();
    let n = |x: usize| x as u64;
    // The prelude is hand-written, not lifted: its own metric, which may
    // rise with the evidence of why; `lines` (the lifted code, stand-ins
    // included) only goes down.
    let prelude = PRELUDE.lines().count();
    m.push("lines", n(main.lines().count() - prelude));
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
    m.push("call_fn_calls", n(soft.indirect));
    m.push("jumps_by_value", n(soft.dyn_jumps));
    m.push("gap_targets", n(gaps.len()));
    writeln!(
        report,
        "metrics (`name value`; bars: crates/codegen/lift-check.toml):"
    )
    .unwrap();
    report.push_str(&m.lines());
    Lifted {
        main,
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

fn code(ctx: Ctx<'_>) -> Code<'_> {
    let funcs: Vec<Func> = ctx.entries.iter().map(|&e| build_func(&ctx, e)).collect();
    let sums = summarize(&ctx, &funcs);
    Code { ctx, funcs, sums }
}

#[derive(Default)]
struct Soft {
    /// Loads (false) and stores (true) by where their address is.
    access: BTreeMap<(bool, Where), u64>,
    /// Lifted functions whose summary has a hole, or that have none.
    summary_holes: usize,
    /// Lifted functions taking and returning all of r0-r3 whose summary
    /// has no hole (stage A printed every function so).
    four_reg_clean: usize,
    /// Lifted functions with a clean summary whose parameters rest on a
    /// callee's run that stopped at a hole ([`Shapes::callee_hole`]).
    callee_holes: usize,
    indirect: usize,
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
            // can't change them; only one that really returns them changed
            // (`analysis::holes`' rule) is lifted wrongly.
            if let Some(h) = sum
                .holes
                .iter()
                .find(|h| h.contains("returns with r4-r11 or sp changed"))
            {
                self.hole(f.entry, format!("summary: {h}"));
            } else if sum.returns && !sum.keeps {
                self.soft.unproven.push((
                    f.entry,
                    sum.holes.iter().next().cloned().unwrap_or_default(),
                ));
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
        for (i, inst) in insts.iter().enumerate() {
            let id = ValueId(i as u32);
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
                    self.count(insts, addr, at, false);
                    let e = format!("{f}(g, {}{m})", v(addr));
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
                    self.count(insts, addr, at, true);
                    let plant = if self.holes.plant == Some(at) {
                        ".wrapping_add(1)"
                    } else {
                        ""
                    };
                    stmt(
                        &mut s,
                        format!("{f}(g, {}{m}, {}{plant});", v(addr), v(value)),
                    );
                }
                Op::Exit(e) => {
                    let x = xs
                        .iter()
                        .find(|x| x.index == i)
                        .copied()
                        .expect("every exit planned");
                    let body = self.exit(e, &x, thumb);
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

    /// Count an access by the instruction at `at`: a constant address by
    /// its region; else as the value-set analysis knows the instruction's
    /// addresses (code it didn't reach, such as discovery's gaps, is
    /// unknown).
    fn count(&mut self, insts: &[Inst], addr: ValueId, at: u32, store: bool) {
        let w = match konst(insts, addr) {
            Some(a) => Where::Const(metrics::region(a)),
            None => match self.ctx.disc.rec.accesses.get(&at) {
                Some(x) if x.stack_only() => Where::Sp,
                Some(x) if !x.sp && !x.top => x
                    .hull
                    .and_then(|(lo, hi)| metrics::region_of(lo, hi))
                    .map_or(Where::Unknown, Where::Computed),
                _ => Where::Unknown,
            },
        };
        *self.soft.access.entry((store, w)).or_default() += 1;
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
                self.soft.indirect += 1;
                let r = reg.index();
                let c = format!(
                    "[r0, r1, r2, r3] = call_fn(g, if r{r} & 1 != 0 {{ r{r} }} else {{ r{r} & !3 }}, r13, {regs})?;"
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
                        _ => {
                            self.soft.indirect += 1;
                            format!("[r0, r1, r2, r3] = call_fn(g, {k}, r13, {regs})?;")
                        }
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
                    0x02 => "g.sp = r13; hw::halt(g)?;".to_string(),
                    0x04 => "r0 = intr_wait(g, r13, r0 != 0, r1)?;".to_string(),
                    0x05 => "r0 = intr_wait(g, r13, true, 1)?; r1 = 1;".to_string(),
                    0x06 => {
                        "let mut r = [r0, r1, r2, r3]; div(&mut r); [r0, r1, r2, r3] = r;".to_string()
                    }
                    0x0B => "let mut r = [r0, r1, r2, r3]; cpu_set(g, r13, &mut r)?; [r0, r1, r2, r3] = r;".to_string(),
                    _ => {
                        debug_assert!(!BIOS.contains(&n));
                        self.hole(at, format!("BIOS call {n:#04x}, which the library doesn't have"));
                        return s;
                    }
                };
                let c = self.call_then(call, next, at);
                writeln!(s, "{ind}{c}").unwrap();
            }
            Plan::Undefined => self.hole(at, "an undefined instruction"),
        }
        s
    }
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
/// (`target`), a resolved table's cases, the holes file's for one
/// Recovery didn't resolve. `Err` with the jump's address where it's
/// unresolved and the holes file gives none.
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
        assert_eq!(h.jumps[&0x0800_1234].0, vec![0x0800_1240, 0x0800_1251]);
        assert!(Holes::parse("[[nothing]]\nx = 1\n").is_err());
        assert!(Holes::parse("[[standin]]\nwhy = \"no entry\"\n").is_err());
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
