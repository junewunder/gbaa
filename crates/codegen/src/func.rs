//! Which blocks each recompiled function has (`docs/codegen-design.md`,
//! "Which blocks a function has"), and what each block exit does.
//!
//! Every function in discovery's output (and every continuation root) gets
//! its own set of superblocks, built with [`ir::build::Builder`] from its
//! entry and from every block start its exits lead to inside it. Code two
//! functions share is built into both.
//!
//! Code copied into IWRAM (decision 7) is built the same way, from its own
//! discovery ([`analysis::discovery::overlays`]) and a [`Ctx`] that reads
//! the copy's ROM bytes at the addresses it runs at.

use analysis::discovery::Discovery;
use analysis::ir::build::Builder;
use analysis::ir::constprop::Rom;
use analysis::ir::{BinOp, Block, Exit, Op, ValueId, flags};
use analysis::program::ROM_BASE;
use analysis::vsa::func::JumpRes;
use armv4t::cycles::Cost;
use armv4t::decode::{Cond, Instr, Reg as DReg, decode_arm, decode_thumb};
use armv4t::sem::{PsrMask, Reg, arm::execute};
use std::collections::{BTreeMap, BTreeSet};

/// The most instructions in one block, which bounds how late an IRQ that a
/// block's I/O store raises is taken.
pub const MAX_BLOCK: usize = 256;

/// The key of `addr` in `thumb` state: interworking form, ARM aligned.
pub fn key(addr: u32, thumb: bool) -> u32 {
    if thumb { addr | 1 } else { addr & !3 }
}

/// What codegen works from: the code's bytes and discovery's results.
pub struct Ctx<'a> {
    /// The bytes the code runs from, mapped at `base`: the ROM, or a copy
    /// of part of it in IWRAM.
    pub rom: &'a [u8],
    pub base: u32,
    pub disc: &'a Discovery,
    /// Function entries and continuation roots, as keys.
    pub entries: BTreeSet<u32>,
    /// Which transformations to make.
    pub cfg: Config,
    /// The source map a named print takes names from; `None` prints the
    /// play view, with addresses only.
    pub names: Option<&'a crate::names::SourceMap>,
    /// The functions proven to keep r4-r11 and sp ([`crate::saves`]): with
    /// [`Config::callee_saves`], a call to one doesn't reload them; in a
    /// readable function, a constant one of them holds outlasts the call.
    pub kept: BTreeSet<u32>,
    /// The functions printed in the readable form (`gbaa-recompile
    /// --readable`, `docs/mod-view-design.md`): [`Ctx::cfg_for`] turns on
    /// their registers in locals and assembly text. `None`: none.
    pub readable: Option<&'a BTreeSet<u32>>,
    /// The cartridge addresses the code's literal pools hold, made on
    /// first use ([`Ctx::pointed`]).
    pub pointed: std::sync::OnceLock<BTreeSet<u32>>,
    /// The game's listed spin loops (`crates/runtime/spins.toml`,
    /// [`crate::spins`]),
    /// worked out on first use: ROM code only.
    pub spins: std::sync::OnceLock<Vec<u32>>,
    /// libgcc's soft-float routines in the code, found on first use
    /// ([`Ctx::library`]).
    pub library: std::sync::OnceLock<Library>,
    /// The targets the holes file gives jumps Recovery left unresolved
    /// (`[[jump]]`, by the address Recovery records the jump under), as
    /// keys: [`Ctx::table_targets`] takes them as a resolved table's, so
    /// the function's blocks reach them. Only the native lift sets them
    /// ([`Ctx::with_jumps`]).
    pub given_jumps: BTreeMap<u32, Vec<u32>>,
}

/// The library routines the runtime stands in for that a context's code
/// has (`analysis::softfloat`: libgcc's `__muldf3` and `__divdf3`): each
/// is taken as a leaf call of its stand-in, and the functions discovery
/// found inside their code (their special-case paths, called only from
/// them) are never run.
#[derive(Clone, Debug, Default)]
pub struct Library {
    /// By entry (an ARM key).
    pub routines: BTreeMap<u32, analysis::softfloat::Routine>,
    /// The code their signatures cover, `start..end`.
    pub regions: Vec<(u32, u32)>,
}

impl Library {
    /// Whether `k` (a key) is in a routine's code.
    pub fn inside(&self, k: u32) -> bool {
        self.regions
            .iter()
            .any(|&(s, e)| (s..e).contains(&(k & !1)))
    }
}

/// The transformations codegen makes, each of which can be turned off, so
/// that a build with it and one without can be compared (`fn-diff`,
/// `docs/structured-codegen-design.md`, "Validation").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Delete the flag writes no static edge reads (decision 9).
    pub flag_liveness: bool,
    /// Delete register writes overwritten in the same block with nothing
    /// between that reads `cpu.r` (step 1 of the structured design).
    pub dead_reg_writes: bool,
    /// Print each ROM function that has a plan as `if`s and loops rather
    /// than `loop { match pc }` (step 8), its blocks no longer dispatch
    /// keys but its entry.
    pub structured: bool,
    /// Hold the registers and flags each function uses in Rust locals,
    /// in step with `Machine` where anything else may read them (step 9).
    /// Off: on Emerald it cost far more build than it won (the design's
    /// step 9, "The spike").
    pub locals: bool,
    /// How each instruction's `insn!` marker prints (`docs/mod-view-design.md`,
    /// "Quiet instruction text").
    pub insn: Insn,
    /// With `locals`: after a call to a function proven to keep r4-r11 and
    /// sp ([`crate::saves`], [`Ctx::prove_saves`]), reload only the other
    /// registers and the flags (`docs/mod-view-design.md`, gap 2).
    pub callee_saves: bool,
    /// The readable form (`docs/mod-view-design.md`): each block's
    /// register and flag writes moved into its exits' sync blocks, its
    /// accesses as `m.get`/`m.set`, its exits as the prelude's macros.
    /// [`Ctx::cfg_for`] turns it on for the readable list.
    pub readable: bool,
    /// How the generated code keeps time (`--timing instant|static|exact`):
    /// mGBA's model, or one of two without it
    /// (`docs/research/mod-view-without-timing.md`). One for the whole
    /// crate: the prelude's macros and `Game` take its shape.
    pub clock: Clock,
}

/// How generated code counts its cycles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Clock {
    /// mGBA's model, cycle for cycle: each block's fetches priced by
    /// `WAITCNT` at run time (`rt::fetch`), every data access by the
    /// memory it touches, and the prefetch buffer's overlap after each
    /// memory instruction (`rt::stall`, from the instruction's address,
    /// which each access carries).
    #[default]
    Exact,
    /// Each block's instructions at their listed cycles, with the fetch
    /// wait states of the `WAITCNT` games set ([`STATIC_WAITCNT`]), one
    /// constant per exit. Data accesses are free and the buffer's overlap
    /// isn't counted (the two about cancel: the research's `zero`), so no
    /// access carries an address and no `rt::stall` is printed.
    Static,
    /// `instant`, as fast as the CPU goes: every fetch as if the prefetch buffer
    /// always had it ready (no wait states), so each instruction costs
    /// only its listed internal and bus cycles, and data accesses are free,
    /// as in `Static`. Not the research's `no-prefetch` (the buffer off,
    /// every fetch at full cost): the opposite end, like an overclock.
    Instant,
    /// `native`, as a native program keeps time (`runtime::Timing::Native`):
    /// code takes none, each exit ticks nothing (`rt::tick_native`), and
    /// an idle loop's back edge (`analysis::idle`) runs the clock to the
    /// next event (`rt::idle_edge`).
    Native,
}

/// The `WAITCNT` [`Clock::Static`] prices fetches by: cartridge waits 3
/// nonsequential and 1 sequential, the buffer on. Kirby & the Amazing
/// Mirror writes `0x4017` and Emerald `0x4014`, the same for ROM.
pub const STATIC_WAITCNT: u16 = 0x4317;

/// How a function prints each instruction's `insn!` marker, which expands
/// to nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insn {
    /// `insn!(0x0803_b264, "Shift { .. }")`: its `Debug` form, the play
    /// view's default.
    Debug,
    /// `insn!(0x0803_b264, "lsls r0, r0, #17")`: as assembly
    /// (`armv4t::text`; `--asm-text`).
    Asm,
    /// `// 0x0803_b264 lsls r0, r0, #17`: the assembly as a comment
    /// (`--asm-comments`).
    Comment,
    /// Not at all: the readable form's default (june, 2026-10-30: they
    /// were a third of its lines).
    Off,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            flag_liveness: true,
            dead_reg_writes: true,
            structured: true,
            locals: false,
            insn: Insn::Debug,
            callee_saves: false,
            readable: false,
            clock: Clock::Exact,
        }
    }
}

impl<'a> Ctx<'a> {
    /// The game's listed spin loops ([`crate::spins::for_rom`]); none for
    /// code copied to RAM.
    pub fn spin_list(&self) -> &[u32] {
        self.spins.get_or_init(|| {
            if self.base == ROM_BASE {
                crate::spins::for_rom(self.rom)
            } else {
                Vec::new()
            }
        })
    }

    /// With [`Config::callee_saves`] or a readable list (whose constants
    /// in r4-r11 last across such calls), prove which of `funcs` (every
    /// function this has) keep r4-r11 and sp, into [`Ctx::kept`].
    pub fn prove_saves(&mut self, funcs: &[Func]) {
        if self.cfg.callee_saves && self.cfg.locals || self.readable.is_some() {
            self.kept = crate::saves::prove(self, funcs).kept;
        }
    }

    pub fn new(rom: &'a [u8], disc: &'a Discovery) -> Ctx<'a> {
        Ctx::at(ROM_BASE, rom, disc)
    }

    /// The code in `image`, mapped at `base` (an overlay's destination).
    pub fn at(base: u32, image: &'a [u8], disc: &'a Discovery) -> Ctx<'a> {
        let mut ctx = Ctx {
            rom: image,
            base,
            disc,
            entries: BTreeSet::new(),
            cfg: Config::default(),
            names: None,
            kept: BTreeSet::new(),
            readable: None,
            pointed: std::sync::OnceLock::new(),
            spins: std::sync::OnceLock::new(),
            library: std::sync::OnceLock::new(),
            given_jumps: BTreeMap::new(),
        };
        // An entry discovery found no code at in its mode (a root whose
        // analysis hit the step cap, or one also a root in the other mode)
        // gets no function: its block would spin, never reaching the
        // interpreter.
        ctx.entries = disc
            .funcs
            .keys()
            .chain(disc.entries.keys())
            .map(|&a| key(a & !1, a & 1 != 0))
            .filter(|&k| ctx.is_code(k))
            .collect();
        ctx
    }

    /// This context with the library routines' entries as function
    /// entries ([`Library`]), for a build that stands in for them (the
    /// native lift, its hole report): a jump into one (a `bx pc` veneer's
    /// `b __divdf3`) is then a tail call, not a copy of its code in the
    /// jumper. The play build leaves them as discovery found them.
    pub fn with_library(mut self) -> Ctx<'a> {
        let ks: Vec<u32> = (self.library().routines.keys())
            .copied()
            .filter(|&k| self.is_code(k))
            .collect();
        self.entries.extend(ks);
        self
    }

    /// This context with the holes file's targets for the jumps Recovery
    /// left unresolved ([`Ctx::given_jumps`]), `jumps` by the jump's
    /// address.
    pub fn with_jumps<'b>(mut self, jumps: impl IntoIterator<Item = (u32, &'b [u32])>) -> Ctx<'a> {
        self.given_jumps = jumps.into_iter().map(|(a, ts)| (a, ts.to_vec())).collect();
        self
    }

    /// The library routines in the code ([`Library`]).
    pub fn library(&self) -> &Library {
        self.library.get_or_init(|| {
            let mut l = Library::default();
            for f in analysis::softfloat::scan(self.rom, self.base) {
                l.routines.insert(f.entry, f.routine);
                l.regions.push((f.start, f.end));
            }
            l
        })
    }

    /// The addresses in the cartridge that a word of a literal pool holds
    /// (jump tables' included, a BIOS copy's source left out): where one
    /// datum the code points at may end, for the readable print's byte
    /// previews ([`crate::text::Preview`]). Empty unless the code runs
    /// from ROM.
    pub fn pointed(&self) -> &BTreeSet<u32> {
        self.pointed.get_or_init(|| {
            let mut out = BTreeSet::new();
            if self.base != ROM_BASE {
                return out;
            }
            let end = ROM_BASE as u64 + self.rom.len() as u64;
            for &(s, e) in &self.disc.data {
                let copied = self
                    .disc
                    .copies
                    .iter()
                    .any(|&(src, _, n)| s < src.saturating_add(n) && src < e);
                if copied || s < ROM_BASE {
                    continue;
                }
                let mut a = (s + 3) & !3;
                while a + 4 <= e && u64::from(a) + 4 <= end {
                    let w = self.read(a, 4);
                    if w >= ROM_BASE && u64::from(w) < end {
                        out.insert(w);
                    }
                    a += 4;
                }
            }
            out
        })
    }

    /// The configuration function `key` is printed in: [`Ctx::cfg`], with
    /// `readable` on (and `locals` off: its registers stay in `Machine`)
    /// if it is on the readable list, and its `insn!` markers left out
    /// unless `--asm-text` or `--asm-comments` asked for them.
    pub fn cfg_for(&self, key: u32) -> Config {
        match self.readable {
            Some(r) if r.contains(&key) => Config {
                readable: true,
                locals: false,
                insn: match self.cfg.insn {
                    Insn::Debug => Insn::Off,
                    i => i,
                },
                ..self.cfg
            },
            _ => self.cfg,
        }
    }

    /// The targets of the resolved jump at `at` (a Thumb `bl` into a case
    /// helper is recorded under the pair's first half), as keys; for one
    /// Recovery left unresolved, those the holes file gives
    /// ([`Ctx::given_jumps`]), if it gives any.
    pub fn table_targets(&self, at: u32, thumb: bool) -> Option<Vec<u32>> {
        let jumps = &self.disc.rec.jumps;
        let a = std::iter::once(at)
            .chain(thumb.then(|| at.wrapping_sub(2)))
            .find(|a| jumps.contains_key(a))?;
        match &jumps[&a] {
            JumpRes::Resolved(j) => {
                Some(j.targets.iter().map(|&t| key(t & !1, t & 1 != 0)).collect())
            }
            _ => self.given_jumps.get(&a).cloned(),
        }
    }

    /// The size of the instruction discovery found at `a` in `thumb`
    /// state (4 for a Thumb `bl` pair).
    fn code_at(&self, a: u32, thumb: bool) -> Option<u32> {
        self.disc
            .code
            .get(&a)
            .filter(|c| c.thumb == thumb)
            .map(|c| c.size)
    }

    /// The identifier of the fn for the function whose entry is `key`:
    /// its name in the source map, else `f_<key>`.
    pub fn fn_ident(&self, key: u32) -> proc_macro2::Ident {
        match self.names {
            Some(n) => n.fn_ident(key),
            None => crate::lower::fn_name(key),
        }
    }

    /// The cartridge ROM, for const-prop to fold loads from: code run from
    /// ROM has it, an IWRAM copy's context doesn't.
    pub fn cart(&self) -> Rom<'a> {
        if self.base == ROM_BASE {
            Rom(self.rom)
        } else {
            Rom::NONE
        }
    }

    /// Whether a block may start at `k`: discovery found code there.
    pub fn is_code(&self, k: u32) -> bool {
        self.code_at(k & !1, k & 1 != 0).is_some()
    }

    fn read(&self, a: u32, bytes: usize) -> u32 {
        let o = (a - self.base) as usize;
        self.rom[o..o + bytes]
            .iter()
            .rev()
            .fold(0, |acc, &b| acc << 8 | b as u32)
    }

    /// rN, if the code at key `k` is a lone `bx rN` (rN not lr or pc):
    /// GCC's and LLVM's call-via-register stub.
    pub fn stub_reg(&self, k: u32) -> Option<Reg> {
        let (a, thumb) = (k & !1, k & 1 != 0);
        let size = if thumb { 2 } else { 4 };
        let o = a.checked_sub(self.base)? as usize;
        if o + size > self.rom.len() {
            return None;
        }
        let i = if thumb {
            decode_thumb(self.read(a, 2) as u16, a)
        } else {
            decode_arm(self.read(a, 4), a)
        };
        match i {
            Instr::Bx { rm, cond: Cond::Al } if rm != DReg::Lr && rm != DReg::Pc => {
                Some(Reg(rm as u8))
            }
            _ => None,
        }
    }

    /// The instructions at `a` (two for a Thumb `bl` pair), with their
    /// addresses.
    fn decode(&self, a: u32, thumb: bool, size: u32) -> Vec<(u32, Instr)> {
        if !thumb {
            return vec![(a, decode_arm(self.read(a, 4), a))];
        }
        (0..size / 2)
            .map(|i| {
                let at = a + 2 * i;
                (at, decode_thumb(self.read(at, 2) as u16, at))
            })
            .collect()
    }
}

/// One block of a function.
#[derive(Clone, Debug)]
pub struct GBlock {
    pub key: u32,
    /// After const-prop, flag liveness and DCE.
    pub block: Block,
    /// The guest instructions, in order.
    pub insns: Vec<(u32, Instr)>,
    /// The address after the last instruction.
    pub end: u32,
}

/// One recompiled function.
#[derive(Clone, Debug)]
pub struct Func {
    pub entry: u32,
    pub blocks: BTreeMap<u32, GBlock>,
    /// The index of its first block's overlay, if it runs from IWRAM: each
    /// version of each block is an overlay of its own
    /// ([`Func::block_overlay`]), checks its guard before it runs and counts
    /// its cycles as RAM code's.
    pub overlay: Option<u16>,
    /// Versions of its IWRAM blocks built from bytes the code was seen to
    /// patch them to ([`crate::variants`]), by key.
    pub variants: BTreeMap<u32, Vec<Variant>>,
    /// The keys of the `bx rN` stubs its calls go through
    /// ([`Plan::CallVia`]): compiled into the caller, so an overlay's guard
    /// covers them too.
    pub stubs: BTreeSet<u32>,
    /// The entries of the functions it calls directly ([`Plan::Call`]).
    pub calls: BTreeSet<u32>,
}

/// A block of an IWRAM function built from bytes the code patched it to.
#[derive(Clone, Debug)]
pub struct Variant {
    pub block: GBlock,
    /// The bytes of the block's guard ranges, in order, as patched.
    pub bytes: Vec<u8>,
}

impl Func {
    /// How many versions the block at `key` has: itself and its variants.
    pub fn versions(&self, key: u32) -> usize {
        1 + self.variants.get(&key).map_or(0, Vec::len)
    }

    /// The overlays of all its blocks' versions.
    pub fn overlays(&self) -> usize {
        self.blocks.keys().map(|&k| self.versions(k)).sum()
    }

    /// The first overlay of the block at `key` (its own bytes; its
    /// variants follow): blocks are numbered from [`Func::overlay`] in
    /// address order.
    pub fn block_overlay(&self, key: u32) -> Option<u16> {
        let o = self.overlay?;
        let before: usize = self
            .blocks
            .range(..key)
            .map(|(&k, _)| self.versions(k))
            .sum();
        Some(o + before as u16)
    }
}

/// What one exit of a block does, as codegen lowers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Continue at a key known now, with no CPU state to update.
    Static(u32),
    /// Call the recompiled function whose entry is `target`, then continue
    /// at whatever it returns to (normally `ret`).
    Call {
        target: u32,
        ret: u32,
    },
    /// A call through a `bx rN` stub at `stub` (a `bl` to it): take the
    /// `bx` as the stub would, then call whatever block rN names
    /// (`rt::call`) and continue at whatever it returns to (normally
    /// `ret`). `cost` is the stub's.
    CallVia {
        stub: u32,
        reg: Reg,
        ret: u32,
        cost: Cost,
    },
    /// Continue at a key known now, but through the CPU state: the block
    /// wrote the CPSR's control byte, so the T bit may have changed.
    Rekey(u32),
    /// Take the exit through `Cpu::apply_exit` (`dispatch::exit`). `ret` is
    /// the return site if it's a call; `target` the target if it's known.
    /// `local`: the target may be a block of this function (it's known, or
    /// a resolved table's), so the `match` may take it. Otherwise (returns,
    /// unresolved jumps) the function returns it to its caller: a return
    /// that landed in the same function's return site would otherwise keep
    /// running the outer call's code on the inner call's host frame.
    Dynamic {
        ret: Option<u32>,
        target: Option<u32>,
        local: bool,
    },
    /// `swi`; continues at `next` unless the BIOS sends it elsewhere.
    Swi {
        next: u32,
    },
    Undefined,
}

/// An exit of a block, with what codegen needs to lower it.
#[derive(Clone, Copy, Debug)]
pub struct ExitInfo {
    /// Index of the exit inst.
    pub index: usize,
    /// The address of the instruction it belongs to.
    pub at: u32,
    /// Instructions run up to and including that one.
    pub insns: usize,
    /// The block's instruction set.
    pub thumb: bool,
    /// The return site, if the exit is a call (whatever its target: a
    /// `bl` to a `bx rN` stub is a call but its target isn't a function).
    pub ret: Option<u32>,
    pub plan: Plan,
    /// Whether the exit is at an idle loop's branch (`analysis::idle`, ROM
    /// code only, as the interpreter looks for them), and which way.
    pub idle: Idle,
}

/// An exit at an idle loop's branch, for `--timing native`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idle {
    No,
    /// The back edge: `rt::idle_edge` runs the clock to the next event.
    Edge,
    /// Out of the loop (the branch not taken): `rt::idle_exit` brings
    /// everything up to a clock a DMA moved.
    Exit,
    /// A spin loop's back edge (`armv4t::idle::is_spin_edge`, or the
    /// game's list): `rt::spin_edge` counts it.
    Spin,
}

/// The constant `v` is, if it is one: `Const`, and `Bin`/`Not` of
/// constants (a Thumb `bl`'s target is `lr + offset` after forwarding).
pub fn fold(b: &Block, v: ValueId) -> Option<u32> {
    match b.insts[v.index()].op {
        Op::Const(x) | Op::Pool(x) => Some(x),
        Op::Not(a) => fold(b, a).map(|a| !a),
        Op::Bin(op, x, y) => {
            let (x, y) = (fold(b, x)?, fold(b, y)?);
            Some(match op {
                BinOp::Add => x.wrapping_add(y),
                BinOp::Sub => x.wrapping_sub(y),
                BinOp::And => x & y,
                BinOp::Or => x | y,
                BinOp::Xor => x ^ y,
                BinOp::Mul => x.wrapping_mul(y),
            })
        }
        _ => None,
    }
}

/// The exits of `gb`, planned.
pub fn exits(ctx: &Ctx, gb: &GBlock) -> Vec<ExitInfo> {
    let b = &gb.block;
    let thumb = b.thumb;
    let size = if thumb { 2 } else { 4 };
    let (mut at, mut n) = (b.start, 0);
    let mut out = Vec::new();
    let mut writes_control = false;
    for (i, inst) in b.insts.iter().enumerate() {
        let e = match inst.op {
            Op::Insn(a) => {
                at = a;
                n += 1;
                continue;
            }
            Op::Exit(e) => e,
            Op::SetCpsr { mask, .. } => {
                writes_control |= mask.contains(PsrMask::CONTROL);
                continue;
            }
            _ => continue,
        };
        let next = at.wrapping_add(size);
        // A call: the block's last lr write before the exit set lr to the
        // return site, and discovery didn't call it a far jump.
        let ret = key(next, thumb);
        let far = ctx.disc.rec.far_jumps.contains(&at)
            || (thumb && ctx.disc.rec.far_jumps.contains(&at.wrapping_sub(2)));
        let lr = b.insts[..i].iter().rev().find_map(|x| match x.op {
            Op::SetReg(Reg::LR, v) => Some(fold(b, v)),
            _ => None,
        });
        let call =
            !far && matches!(e, Exit::Jump(_) | Exit::JumpExchange(_)) && lr == Some(Some(ret));
        let local =
            |target: Option<u32>| target.is_some() || ctx.table_targets(at, thumb).is_some();
        let plan = match e {
            Exit::Fallthrough(a) => Plan::Static(key(a, thumb)),
            // A misaligned ARM target keeps its low bits in `cpu.pc` (as in
            // the interpreter), which only `apply_exit` does.
            Exit::Jump(t) => match fold(b, t) {
                Some(t) if !thumb && t & 3 != 0 => Plan::Dynamic {
                    ret: call.then_some(ret),
                    target: Some(key(t, thumb)),
                    local: true,
                },
                Some(t) if call && ctx.entries.contains(&key(t, thumb)) => Plan::Call {
                    target: key(t, thumb),
                    ret,
                },
                Some(t) if call && let Some(reg) = ctx.stub_reg(key(t, thumb)) => {
                    let stub = key(t, thumb);
                    Plan::CallVia {
                        stub,
                        reg,
                        ret,
                        cost: armv4t::cycles::cost(&ctx.decode(stub & !1, thumb, 2)[0].1),
                    }
                }
                Some(t) => Plan::Static(key(t, thumb)),
                None => Plan::Dynamic {
                    ret: call.then_some(ret),
                    target: None,
                    local: local(None),
                },
            },
            Exit::JumpExchange(t) => {
                let target = fold(b, t).map(|t| key(t & !1, t & 1 != 0));
                Plan::Dynamic {
                    ret: call.then_some(ret),
                    target,
                    local: local(target),
                }
            }
            Exit::ExceptionReturn(_) => Plan::Dynamic {
                ret: None,
                target: None,
                local: false,
            },
            Exit::Swi { next, .. } => Plan::Swi {
                next: key(next, thumb),
            },
            Exit::Undefined => Plan::Undefined,
        };
        let plan = match plan {
            Plan::Static(k) if writes_control => Plan::Rekey(k),
            p => p,
        };
        let idle_branch = || {
            let fetch = |a: u32| ctx.read(a, size as usize);
            ctx.base == ROM_BASE
                && runtime::machine::is_rom(at)
                && matches!(ctx.decode(at, thumb, size)[0].1, Instr::Branch { link: false, target, .. }
                    if analysis::idle::is_idle_edge(fetch, at, target, thumb))
        };
        let spin_branch = || {
            let fetch = |a: u32| ctx.read(a, size as usize);
            ctx.base == ROM_BASE
                && runtime::machine::is_rom(at)
                && matches!(ctx.decode(at, thumb, size)[0].1, Instr::Branch { link: false, target, .. }
                    if target <= at && (ctx.spin_list().binary_search(&at).is_ok()
                        || armv4t::idle::is_spin_edge(fetch, at, target, thumb)))
        };
        let idle = match (plan, e) {
            (Plan::Static(_) | Plan::Rekey(_), Exit::Jump(_)) if idle_branch() => Idle::Edge,
            (Plan::Static(_) | Plan::Rekey(_), Exit::Fallthrough(_)) if idle_branch() => Idle::Exit,
            // Any plan: a branch whose block set `lr` may be a `Call`.
            (_, Exit::Jump(_)) if spin_branch() => Idle::Spin,
            _ => Idle::No,
        };
        out.push(ExitInfo {
            index: i,
            at,
            insns: n,
            thumb,
            ret: call.then_some(ret),
            plan,
            idle,
        });
    }
    out
}

/// The block starts an exit leads to: where the function may continue.
pub fn successors(ctx: &Ctx, x: &ExitInfo) -> Vec<u32> {
    let mut out = match x.plan {
        Plan::Static(k) | Plan::Rekey(k) => vec![k],
        Plan::Call { ret, .. } | Plan::CallVia { ret, .. } => vec![ret],
        Plan::Dynamic { ret, target, .. } => ret.into_iter().chain(target).collect(),
        Plan::Swi { next } => vec![next],
        Plan::Undefined => vec![],
    };
    out.extend(x.ret);
    out.extend(ctx.table_targets(x.at, x.thumb).unwrap_or_default());
    out
}

/// Build the block starting at `k`, stopping before any key in `stops`.
///
/// Under [`Clock::Native`] a store to `IME` (its address folded) ends the
/// block too: an IRQ it lets through is taken at the tick after it, so the
/// compiled code takes it at the same instruction as the interpreter, which
/// ticks after each one (native/ime-release). Elsewhere the store's tick
/// would only be later by a block, as any I/O write's is.
fn build_block(ctx: &Ctx, k: u32, stops: &BTreeSet<u32>) -> GBlock {
    let gb = build_block_until(ctx, k, stops, None);
    if ctx.cfg.clock == Clock::Native
        && let Some(at) = ime_store(&gb.block)
        && gb.insns.last().is_some_and(|&(last, _)| last != at)
    {
        return build_block_until(ctx, k, stops, Some(at));
    }
    gb
}

/// The address of the first instruction in `b` that stores to `IME`.
fn ime_store(b: &Block) -> Option<u32> {
    let mut at = 0;
    b.insts.iter().find_map(|inst| match inst.op {
        Op::Insn(a) => {
            at = a;
            None
        }
        Op::Store { addr, .. } => fold(b, addr)
            .is_some_and(|a| (0x0400_0208..0x0400_020C).contains(&a))
            .then_some(at),
        _ => None,
    })
}

/// [`build_block`], ending after the instruction at `last` if it gets
/// there.
fn build_block_until(ctx: &Ctx, k: u32, stops: &BTreeSet<u32>, last: Option<u32>) -> GBlock {
    let thumb = k & 1 != 0;
    let mut a = k & !1;
    let mut b = Builder::new(a, thumb);
    if ctx.cfg.flag_liveness {
        // The values of the flag writes liveness deletes, for each exit's
        // yield path.
        b = b.yield_flags();
    }
    let mut insns = Vec::new();
    while let Some(size) = ctx.code_at(a, thumb) {
        for (at, instr) in ctx.decode(a, thumb, size) {
            b.begin_insn(at);
            b.timing(&instr, at);
            execute(&mut b, at, thumb, instr);
            insns.push((at, instr));
            if b.ended() || last == Some(at) {
                break;
            }
        }
        a = a.wrapping_add(size);
        // An `msr` to the CPSR's control byte may change T, after which the
        // next instruction is in the other set: end the block, and the
        // fallthrough rekeys from the CPU (`Plan::Rekey`).
        let control = matches!(
            insns.last(),
            Some((_, Instr::Msr { spsr: false, fields, .. })) if fields.c
        );
        let stop = last.is_some() && insns.last().map(|x| x.0) == last;
        if b.ended()
            || control
            || stop
            || insns.len() >= MAX_BLOCK
            || stops.contains(&key(a, thumb))
        {
            break;
        }
    }
    let mut block = b.finish(a);
    block.const_prop(ctx.cart());
    block.dce();
    if ctx.cfg.dead_reg_writes {
        block.remove_dead_reg_writes();
    }
    GBlock {
        key: k,
        block,
        insns,
        end: a,
    }
}

/// Delete the function's dead flag writes (decision 9). Flags flow along
/// its static edges (`Plan::Static` to one of its blocks); every other exit
/// keeps all four live. An IWRAM copy's blocks may not run as built (a
/// failed guard goes to the interpreter, a variant may run instead), so
/// there every exit keeps all four live.
fn remove_dead_flags(ctx: &Ctx, blocks: &mut BTreeMap<u32, GBlock>) {
    let index: BTreeMap<u32, usize> = blocks.keys().enumerate().map(|(i, &k)| (k, i)).collect();
    let rom = ctx.base == ROM_BASE;
    let succ: Vec<BTreeMap<usize, usize>> = blocks
        .values()
        .map(|gb| {
            exits(ctx, gb)
                .into_iter()
                .filter_map(|x| match x.plan {
                    Plan::Static(k) if rom => Some((x.index, *index.get(&k)?)),
                    _ => None,
                })
                .collect()
        })
        .collect();
    let mut bs: Vec<&mut Block> = blocks.values_mut().map(|gb| &mut gb.block).collect();
    flags::remove_dead(&mut bs, |b, i| succ[b].get(&i).copied());
}

/// Build the function whose entry is `entry` (a key): every block reachable
/// from the entry without leaving through another function's entry.
pub fn build_func(ctx: &Ctx, entry: u32) -> Func {
    let mut stops: BTreeSet<u32> = ctx.entries.clone();
    stops.insert(entry);
    let mut blocks: BTreeMap<u32, GBlock> = BTreeMap::new();
    let mut work = vec![entry];
    while let Some(k) = work.pop() {
        if blocks.contains_key(&k) {
            continue;
        }
        let gb = build_block(ctx, k, &stops);
        let xs = exits(ctx, &gb);
        blocks.insert(k, gb);
        for x in xs {
            for s in successors(ctx, &x) {
                let foreign = s != entry && ctx.entries.contains(&s);
                if foreign || !ctx.is_code(s) || !stops.insert(s) {
                    continue;
                }
                work.push(s);
                // A new start inside a block already built splits it.
                let inside = blocks
                    .range(..s)
                    .rev()
                    .take_while(|(b, _)| s - **b < 4 * MAX_BLOCK as u32 + 4)
                    .find(|(b, g)| **b & 1 == s & 1 && (s & !1) < g.end)
                    .map(|(&b, _)| b);
                if let Some(b) = inside {
                    blocks.remove(&b);
                    work.push(b);
                }
            }
        }
    }
    if ctx.cfg.flag_liveness {
        remove_dead_flags(ctx, &mut blocks);
    }
    let (mut stubs, mut calls) = (BTreeSet::new(), BTreeSet::new());
    for x in blocks.values().flat_map(|gb| exits(ctx, gb)) {
        match x.plan {
            Plan::CallVia { stub, .. } => {
                stubs.insert(stub);
            }
            Plan::Call { target, .. } => {
                calls.insert(target);
            }
            _ => {}
        }
    }
    Func {
        entry,
        blocks,
        overlay: None,
        variants: BTreeMap::new(),
        stubs,
        calls,
    }
}
