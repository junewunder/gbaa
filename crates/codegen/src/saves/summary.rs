//! Per-function summaries (`docs/native-port-design.md` section 5,
//! "Registers"; `docs/research/registers-and-types.md`, "Van Emmerik's
//! preservation analysis"): which of r0–r3 a function reads on entry (its
//! parameters), which registers it leaves defined on every return that a
//! caller reads (its results), and whether r4–r11 and sp come back as they
//! came (the callee-save proof, [`super::prove`]).
//!
//! The summary's run is the proof's abstract run with one more fact per
//! register r0–r3 and r12: where its value came from ([`Org`]). The entry
//! value, a write of the function's own, a value a BIOS call or an
//! indirect call clobbered, or the value a direct call left there (by call
//! site). A read of a register that may still hold its entry value makes
//! it a parameter; a call reads its callee's parameters there. What the
//! proof fails on is a hole in the summary instead ([`Summary::holes`]),
//! and the run goes on where it can.
//!
//! Summaries are a least fixed point over the call graph (Van Emmerik's
//! premises for recursion): every function starts reading nothing and
//! never returning, and each is run again when a callee's summary grows.
//! An unknown callee (an indirect call) reads r0–r3 and returns a value in
//! r0, clobbering r1–r3 and r12; a BIOS call reads and writes what GBATEK
//! says it does. An indirect call whose callee set is known complete
//! ([`summarize_sets`]; the lifter's `native::fnref`) reads the union of
//! its callees' parameters and leaves what any of them may leave, like a
//! direct call to each (decision "Indirect calls pass r0-r3 until their
//! callee sets are known").
//!
//! [`compare`] checks summaries against DWARF prototypes ([`proto_regs`]):
//! the gate the stage-A lifter's parameter lists and return values rest
//! on.

use super::{State, V, Walk, prove, walk, width_mask};
use crate::func::{Ctx, Func};
use crate::names::SourceMap;
use analysis::ir::{Inst, Op};
use armv4t::sem::Width;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The registers a summary tracks, by slot: r0–r3 and r12.
const SLOTS: [u8; 5] = [0, 1, 2, 3, 12];

fn slot(r: usize) -> Option<usize> {
    match r {
        0..=3 => Some(r),
        12 => Some(4),
        _ => None,
    }
}

/// Where a register's value may have come from, in a summary's run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Org {
    /// Its value at the function's entry.
    entry: bool,
    /// A write of the function's own.
    own: bool,
    /// A BIOS call or an indirect call that doesn't define it.
    clob: bool,
    /// The direct calls (by call site) whose return left it.
    sites: BTreeSet<u32>,
}

impl Org {
    pub(super) fn entry() -> Org {
        Org {
            entry: true,
            ..Org::default()
        }
    }

    pub(super) fn join(&self, o: &Org) -> Org {
        Org {
            entry: self.entry || o.entry,
            own: self.own || o.own,
            clob: self.clob || o.clob,
            sites: self.sites.union(&o.sites).copied().collect(),
        }
    }
}

/// What a register may hold when a function returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bits {
    /// Its value at entry.
    pub entry: bool,
    /// A value the function (or a callee that defines it) wrote.
    pub def: bool,
    /// A value nothing defines (a BIOS call's or an indirect call's
    /// leftovers).
    pub clob: bool,
}

/// Why a register is a parameter: the first read of it found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The function's own instruction at this address reads it.
    Reads(u32),
    /// The call at `at` passes it on to `callee`, which reads it.
    Callee { callee: u32, at: u32 },
    /// The indirect call at this address, taken to read r0–r3.
    Indirect(u32),
    /// The BIOS call `swi n` at `at` reads it.
    Bios { n: u8, at: u32 },
}

impl Cause {
    pub fn at(self) -> u32 {
        match self {
            Cause::Reads(at)
            | Cause::Callee { at, .. }
            | Cause::Indirect(at)
            | Cause::Bios { at, .. } => at,
        }
    }
}

/// A function's summary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// The parameters: bit `n` for rN, r0–r3, read on entry.
    pub params: u8,
    /// Why each parameter is one.
    pub causes: BTreeMap<u8, Cause>,
    /// What r0–r3 and r12 may hold on return (by [`SLOTS`]); all clear if
    /// it never returns.
    pub exit: [Bits; 5],
    /// Some path returns to the caller.
    pub returns: bool,
    /// The results: bit `n` for rN, r0–r3, defined on every return and
    /// read by a caller after a direct call (or an indirect one whose
    /// complete callee set has it).
    pub results: u16,
    /// The registers callers read after a direct call to it, or an
    /// indirect call whose complete callee set has it (bit `n` for
    /// rN, r0–r3 and r12), defined or not: with their own instructions,
    /// and by passing them on to a callee that reads them.
    pub read_after: u16,
    pub passed_after: u16,
    /// For each register in `passed_after`, a caller that passes it on
    /// and how.
    pub passed_by: BTreeMap<u8, (u32, Cause)>,
    /// The registers callers return as their own result without reading
    /// (`return f(x);`), where a caller of theirs reads it with its own
    /// instruction or forwards it again.
    pub forwarded_after: u16,
    /// The direct callees whose value it returns, by register.
    pub forwards: BTreeMap<u8, BTreeSet<u32>>,
    /// r4–r11 and sp proven kept ([`super::prove`]).
    pub keeps: bool,
    /// The registers of r4–r11 (bit `n` for rN) some return may leave
    /// other than they came: hand-written code's scratch registers. The
    /// lift keeps its callers' r4–r11 in their own locals, so that is
    /// sound only where no caller reads one before writing it (a hole at
    /// the reader), and nothing calls it through a pointer, where callers
    /// aren't known. A function's own read of one of r4–r11 as it came is
    /// a hole whether it changes them or not ([`Sum::entry_uses`]).
    pub changes: u16,
    /// The registers of r4–r11 (bit `n - 4` for rN) it reads as they came,
    /// some bit reaching a use: its own, or a callee's it passes them on
    /// to ([`Sum::call`]). Parameters by register: the lift passes them at
    /// each direct call and tail call, and at the indirect calls a holes
    /// file names ([`Passing`]); any other way in zeroes them, a hole.
    /// One only returned in r0–r3 isn't here ([`Summary::entry_returns`]).
    pub takes: u8,
    /// The calls its run saw pass a callee what it takes: (the call's
    /// address, the callee). The lift passes at no other: a call the run
    /// never reached (code past a hole) had its registers unread.
    pub passes: BTreeSet<(u32, u32)>,
    /// Why the summary may be incomplete: what the proof fails on, and
    /// each read of a register a callee (`Some`) or a BIOS or indirect
    /// call (`None`) left undefined, by the reading instruction.
    pub holes: BTreeSet<String>,
    /// A path's run ended at one of the holes (a jump into code it
    /// doesn't follow, to an unknown place, a mode change, a run that
    /// didn't settle): reads past it are unseen, so its parameters, and
    /// what it reads after its calls, may be incomplete.
    pub stopped: bool,
    /// The functions whose summaries its run used: its direct calls'
    /// callees, and the functions it tail-calls (a jump into another's
    /// entry the run doesn't go on through).
    pub callees: BTreeSet<u32>,
    /// Its indirect calls, by address (with a complete callee set or not).
    pub indirect_sites: BTreeSet<u32>,
    pub undefined_reads: BTreeMap<(u32, u8), Option<u32>>,
    /// The holes of entry values of r4–r11 returned in r0–r3, by the
    /// register they're in: holes where a caller's own instruction reads
    /// it (`read_after`, `forwarded_after`; agbcc's epilogue `pop {r3};
    /// mov r8, r3` leaves the entry r8 in r3, unread).
    pub entry_returns: BTreeMap<u8, BTreeSet<String>>,
}

impl Summary {
    /// The registers (bit `n` for rN, r0–r3 and r12) defined on every
    /// return: never a leftover, and the entry value only where it's a
    /// parameter.
    pub fn defines(&self) -> u16 {
        defines(self.params, &self.exit)
    }

    /// [`Summary::takes`] where its callers pass them (a `takes r4-r11
    /// from its callers` hole): 0 for one whose read is a hard hole, or
    /// that nothing calls.
    pub fn passed(&self) -> u8 {
        if self.holes.iter().any(|h| h.contains(TAKES_READ)) {
            self.takes
        } else {
            0
        }
    }

    /// [`Summary::takes`] where a call may pass them: [`Summary::passed`],
    /// or a hard hole a holes file may yet name the callers of. 0 for one
    /// nothing calls.
    pub fn may_take(&self) -> u8 {
        let called = |h: &String| h.contains(TAKES_READ) || h.contains(ENTRY_READ);
        if self.holes.iter().any(called) {
            self.takes
        } else {
            0
        }
    }
}

/// What the lift passes of r4–r11 besides a direct call's: the holes
/// file's part of a summary's run.
#[derive(Clone, Debug, Default)]
pub struct Passing {
    /// By an indirect call's address: the functions it passes what they
    /// take (`[[takes]]`). The lift prints the call to one of them with
    /// the caller's registers, and their plain entries stop the program,
    /// so no other pointer call reaches one with zeroes.
    pub sites: BTreeMap<u32, BTreeSet<u32>>,
    /// Every function a `[[takes]]` names, one with no `sites` too: no
    /// pointer call reaches that one (code only jumped into, with the
    /// jumper's registers: m4a's mixer in IWRAM, from `SoundMain`'s
    /// constant `bx r3`), and its plain entry stops the program all the
    /// same (unit `native/nightmare-entry-reads`).
    pub named: BTreeSet<u32>,
    /// The functions a stand-in replaces: they take nothing.
    pub replaced: BTreeSet<u32>,
}

fn defines(params: u8, exit: &[Bits; 5]) -> u16 {
    let mut m = 0;
    for (i, b) in exit.iter().enumerate() {
        let r = SLOTS[i];
        let param = r < 4 && params & 1 << r != 0;
        if !b.clob && (!b.entry || param) {
            m |= 1 << r;
        }
    }
    m
}

/// What a summary assumes of an indirect call's callee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assume {
    /// The registers of r0–r3 it reads (bit `n` for rN): all four is
    /// sound; none trusts the caller to have set up what it passes, as C
    /// does.
    pub indirect_params: u8,
}

impl Default for Assume {
    fn default() -> Assume {
        Assume {
            indirect_params: 0xf,
        }
    }
}

/// An indirect call's callee: reads r0–r3 (as [`Assume`] says), defines
/// r0, clobbers the rest.
const UNKNOWN: (u8, [Bits; 5]) = {
    let def = Bits {
        entry: false,
        def: true,
        clob: false,
    };
    let clob = Bits {
        entry: false,
        def: false,
        clob: true,
    };
    (0xf, [def, clob, clob, clob, clob])
};

/// What BIOS call `n` reads and defines (bit `n` for rN), per GBATEK; the
/// rest of r0–r3 it may clobber. One this doesn't know reads r0–r3 and
/// defines r0.
fn bios(n: u8) -> (u8, u8) {
    match n {
        0x01 | 0x19 | 0x1A | 0x1B => (0b0001, 0),
        0x02 | 0x03 | 0x05 | 0x1C | 0x1D | 0x1E | 0x28 | 0x29 => (0, 0),
        0x04 => (0b0011, 0),
        0x06 | 0x07 => (0b0011, 0b1011),
        0x08 | 0x09 => (0b0001, 0b0001),
        0x0A => (0b0011, 0b0001),
        0x0B | 0x0C | 0x0E | 0x10 => (0b0111, 0),
        0x0D => (0, 0b0001),
        0x0F => (0b1111, 0),
        0x11..=0x18 => (0b0011, 0),
        0x1F => (0b0111, 0b0001),
        0x25 => (0b0011, 0b0001),
        _ => (0b1111, 0b0001),
    }
}

/// Each indirect call's complete callee set, by the call's address.
pub type Sets = BTreeMap<u32, BTreeSet<u32>>;

/// A summary's run: the callees' summaries so far, and what it finds.
pub(super) struct Sum<'a> {
    sums: &'a BTreeMap<u32, Summary>,
    assume: Assume,
    /// The indirect calls whose callee sets are complete.
    sets: &'a Sets,
    passing: &'a Passing,
    /// The functions proven to keep r4–r11 and sp ([`super::Proof`]).
    kept: &'a BTreeSet<u32>,
    params: BTreeMap<u8, Cause>,
    /// r0–r3 and r12's origins, joined over every return.
    exit: Option<Vec<Org>>,
    /// (callee, register): a read after a direct call (or one of a
    /// complete callee set) of what it left,
    /// the first found.
    used: BTreeMap<(u32, u8), Cause>,
    undefined: BTreeMap<(u32, u8), Option<u32>>,
    pub(super) holes: BTreeSet<String>,
    /// A path's run ended at a hole ([`Summary::stopped`]).
    pub(super) stopped: bool,
    /// Each call site's callee, `None` for an indirect one.
    sites: BTreeMap<u32, Option<u32>>,
    /// The functions it falls or jumps into and goes on through, with
    /// its own registers ([`super::Walk::flow`]).
    pub(super) falls: BTreeSet<u32>,
    /// The functions whose summaries this run used.
    refs: BTreeSet<u32>,
    /// r4–r11 not as they came, over every return ([`Summary::changes`]).
    changes: u16,
    /// Reads of r4–r11 as they came whose bits reach a use: (the
    /// instruction, the register, how).
    entry_reads: BTreeSet<(u32, u8, Via)>,
    /// [`Summary::passes`].
    passes: BTreeSet<(u32, u32)>,
    /// Reads of r4–r11 as they came whose every bit an op masks away:
    /// (the op's instruction, the register).
    masked: BTreeSet<(u32, u8)>,
    /// The frame pointers (bit `n` for rN, r7 or r11) it points at the
    /// stack word it saved them in: a frame record, whose entry value is
    /// the caller's frame pointer ([`Sum::frame_record`]).
    frames: u16,
}

/// How bits of an entry value of r4–r11 reach a use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Via {
    /// The instruction's own: an address, a value stored, a condition.
    Own,
    /// In this register, to a callee (or BIOS call) that reads it.
    Arg(u8),
    /// In this register, returned.
    Ret(u8),
    /// As it came, to this callee, which takes it ([`Summary::takes`]).
    Takes(u32),
}

impl Via {
    fn text(self) -> String {
        match self {
            Via::Own => String::new(),
            Via::Arg(r) => format!(", passed in r{r}"),
            Via::Ret(r) => format!(", returned in r{r}"),
            Via::Takes(c) => format!(", which 0x{c:08x} takes"),
        }
    }
}

/// Each register of r4–r11 in `regs` (bit `n - 4` for rN).
fn entry_regs(regs: u8) -> impl Iterator<Item = u8> {
    (4..12u8).filter(move |r| regs & 1 << (r - 4) != 0)
}

impl<'a> Sum<'a> {
    fn new(
        sums: &'a BTreeMap<u32, Summary>,
        assume: Assume,
        sets: &'a Sets,
        passing: &'a Passing,
        kept: &'a BTreeSet<u32>,
    ) -> Sum<'a> {
        Sum {
            sums,
            assume,
            sets,
            passing,
            kept,
            params: BTreeMap::new(),
            exit: None,
            used: BTreeMap::new(),
            undefined: BTreeMap::new(),
            holes: BTreeSet::new(),
            stopped: false,
            sites: BTreeMap::new(),
            falls: BTreeSet::new(),
            refs: BTreeSet::new(),
            changes: 0,
            entry_reads: BTreeSet::new(),
            passes: BTreeSet::new(),
            masked: BTreeSet::new(),
            frames: 0,
        }
    }

    /// `callee`'s parameters and return state so far (`None`: unknown).
    fn callee(&self, callee: Option<u32>) -> (u8, [Bits; 5]) {
        match callee.and_then(|c| self.sums.get(&c)) {
            // A callee whose run lost its returns to a hole: what it
            // leaves is unknown, not nothing.
            Some(s) if Self::lost(s) => (s.params, UNKNOWN.1),
            Some(s) => (s.params, s.exit),
            None if callee.is_none() => (self.assume.indirect_params, UNKNOWN.1),
            None => UNKNOWN,
        }
    }

    /// What the call at `site` to `callee` reads and leaves: an indirect
    /// one with a complete callee set, the union of its callees'.
    fn at_site(&self, site: u32, callee: Option<u32>) -> (u8, [Bits; 5]) {
        let set = match callee {
            None => self.sets.get(&site),
            Some(_) => None,
        };
        let Some(set) = set else {
            return self.callee(callee);
        };
        let (mut p, mut x) = (0, [Bits::default(); 5]);
        for &c in set {
            let (cp, cx) = self.callee(Some(c));
            p |= cp;
            for (a, b) in x.iter_mut().zip(cx) {
                a.entry |= b.entry;
                a.def |= b.def;
                a.clob |= b.clob;
            }
        }
        (p, x)
    }

    /// The functions the call at `site` to `callee` may reach whose
    /// summaries it uses: its callee, or an indirect call's complete set.
    fn targets(&self, site: u32, callee: Option<u32>) -> Vec<u32> {
        match callee {
            Some(c) => vec![c],
            None => self
                .sets
                .get(&site)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default(),
        }
    }

    /// `callee`'s summary so far finds no return, and no path of its run
    /// ended at a hole, which may have hidden one ([`Summary::stopped`]):
    /// a call to it doesn't come back.
    pub(super) fn never_returns(&self, callee: u32) -> bool {
        self.sums
            .get(&callee)
            .is_some_and(|s| !s.returns && !s.stopped)
    }

    /// `callee` is proven to keep r4–r11: a caller's constant in one is
    /// the same after a call to it ([`super::State::called`]).
    pub(super) fn keeps(&self, callee: u32) -> bool {
        self.kept.contains(&callee)
    }

    /// No return found, and a hole that may be why.
    fn lost(s: &Summary) -> bool {
        !s.returns && !s.holes.is_empty()
    }

    /// Register `r` read in state `s`, for `cause`.
    pub(super) fn read(&mut self, s: &State, r: usize, cause: Cause) {
        // A callee's or a BIOS call's argument holding bits of r4–r11's
        // entry values (`mov r0, r8; bl f`).
        if !matches!(cause, Cause::Reads(_))
            && let Some((regs, _)) = s.regs[r].taint()
        {
            for e in entry_regs(regs) {
                self.entry_reads.insert((cause.at(), e, Via::Arg(r as u8)));
            }
        }
        if let Some(&c) = s.chg.get(r.wrapping_sub(4))
            && c != 0
        {
            self.holes.insert(format!(
                "0x{:08x}: reads r4-r11 a callee changed: r{r}, by 0x{c:08x}",
                cause.at()
            ));
        }
        let Some(i) = slot(r) else { return };
        let o = &s.org[i];
        if o.entry && r < 4 {
            self.params.entry(r as u8).or_insert(cause);
        }
        for site in &o.sites {
            let callee = self.sites[site];
            for c in self.targets(*site, callee) {
                self.used.entry((c, r as u8)).or_insert(cause);
            }
            let (p, x) = self.at_site(*site, callee);
            if defines(p, &x) & 1 << r == 0 {
                self.undefined.insert((cause.at(), r as u8), callee);
            }
        }
        if o.clob {
            self.undefined.insert((cause.at(), r as u8), None);
        }
    }

    /// Register `r` written, in every case or (`guarded`) maybe.
    pub(super) fn set(&self, s: &mut State, r: usize, guarded: bool) {
        let Some(i) = slot(r) else { return };
        if guarded {
            s.org[i].own = true;
        } else {
            s.org[i] = Org {
                own: true,
                ..Org::default()
            };
        }
    }

    /// A call at `at` to `callee` (`None`: an indirect one), in `s`.
    pub(super) fn call(&mut self, s: &mut State, callee: Option<u32>, at: u32) {
        let targets = self.targets(at, callee);
        self.refs.extend(targets.iter().copied());
        for &c in &targets {
            if self.sums.get(&c).is_some_and(Self::lost) {
                self.holes.insert(format!(
                    "calls 0x{c:08x}, whose summary has holes and no return"
                ));
            }
        }
        let (params, exit) = self.at_site(at, callee);
        for r in 0..4 {
            if params & 1 << r != 0 {
                // An indirect call with a complete callee set passes it
                // to the first callee that reads it.
                let reader = targets
                    .iter()
                    .copied()
                    .find(|&c| self.callee(Some(c)).0 & 1 << r != 0);
                let cause = match reader {
                    Some(c) => Cause::Callee { callee: c, at },
                    None => Cause::Indirect(at),
                };
                self.read(s, r, cause);
            }
        }
        // What a callee takes of r4–r11 is this call's own registers (the
        // lift passes them): one still as it came is taken in turn.
        let takers: Vec<u32> = match callee {
            Some(c) => vec![c],
            None => self
                .passing
                .sites
                .get(&at)
                .into_iter()
                .flatten()
                .copied()
                .collect(),
        };
        for c in takers {
            self.refs.insert(c);
            let takes = match self.sums.get(&c) {
                Some(x) if !self.passing.replaced.contains(&c) => x.takes,
                _ => 0,
            };
            // A stack address through a named pointer call: the frames
            // (`native::frame`, made before the holes file is read) don't
            // see it escape, so the call isn't one the lift passes at.
            let frame = callee.is_none()
                && entry_regs(takes).any(|n| matches!(s.regs[n as usize], V::Sp(_)));
            if takes != 0 && !frame {
                self.passes.insert((at, c));
            }
            for n in entry_regs(takes) {
                if let Some((regs, _)) = s.regs[n as usize].taint() {
                    for e in entry_regs(regs) {
                        self.entry_reads.insert((at, e, Via::Takes(c)));
                    }
                }
                // The lift's local is as it was before an earlier callee
                // changed the register: not what the machine passes.
                let by = s.chg[n as usize - 4];
                if by != 0 {
                    self.holes.insert(format!(
                        "0x{at:08x}: reads r4-r11 a callee changed: r{n}, by 0x{by:08x}"
                    ));
                }
            }
        }
        self.sites.insert(at, callee);
        let changes = callee
            .and_then(|c| self.sums.get(&c))
            .map_or(0, |c| c.changes);
        for r in (4..12).filter(|r| changes & 1 << r != 0) {
            s.regs[r] = V::Other;
            s.chg[r - 4] = callee.expect("a direct call");
        }
        for (o, x) in s.org.iter_mut().zip(exit) {
            let mut n = if x.entry { o.clone() } else { Org::default() };
            if x.def || x.clob {
                n.sites.insert(at);
            }
            *o = n;
        }
    }

    /// `swi n` at `at`, in `s`.
    pub(super) fn swi(&mut self, s: &mut State, n: u8, at: u32) {
        let (reads, defs) = bios(n);
        for r in 0..4 {
            if reads & 1 << r != 0 {
                self.read(s, r, Cause::Bios { n, at });
            }
        }
        for (r, o) in s.org[..4].iter_mut().enumerate() {
            *o = Org {
                own: defs & 1 << r != 0,
                clob: defs & 1 << r == 0,
                ..Org::default()
            };
        }
    }

    /// Inst `i` (of instruction `at`), with the values up to its own:
    /// where bits of r4–r11's entry values reach a use. The run follows
    /// which bits of each value may hold them ([`V::Part`]): a pure op
    /// passes on the bits that survive it, a copy to a register or a flag
    /// or a word stored on the stack keeps them, and the rest is a use:
    /// an address, a value stored elsewhere, a condition (a guard, a
    /// select's), a jump's target. A use is a register parameter the lift
    /// doesn't pass (`Regs::call` zeroes r4–r12), a hard hole: TMC's
    /// `sub_080086D8` takes r8 and r11 from its hand-written callers. So
    /// is passing them to a callee ([`Sum::read`]) or returning them
    /// ([`Sum::ret`]). A pure op whose operand holds some and whose value
    /// holds none masked every bit away (agbcc builds a word byte by byte
    /// in a register it never set, `ands r4, r0; orrs r4, r1`): soft,
    /// recorded as `masked`.
    pub(super) fn entry_uses(&mut self, insts: &[Inst], i: usize, at: u32, vals: &[V]) {
        let op = insts[i].op;
        let taint = |v: analysis::ir::ValueId| vals[v.index()].taint();
        let mut uses = insts[i].guard.and_then(taint).map_or(0, |t| t.0);
        let pure = op.is_pure_bit()
            || matches!(
                op,
                Op::Bin(..)
                    | Op::Not(_)
                    | Op::MulLongLo { .. }
                    | Op::MulLongHi { .. }
                    | Op::Adc { .. }
                    | Op::Shift { .. }
                    | Op::Rrx { .. }
                    | Op::Sext { .. }
                    | Op::Select { .. }
                    | Op::BitToWord(_)
            );
        let mut held = 0;
        for v in op.operands() {
            let Some((regs, bits)) = taint(v) else {
                continue;
            };
            let kept = match op {
                Op::Store { addr, value, width } => {
                    v == value
                        && v != addr
                        && match vals[addr.index()] {
                            // A word on the stack keeps them (a save).
                            V::Sp(_) if width == Width::Word => true,
                            _ => bits & width_mask(width) == 0,
                        }
                }
                Op::SetReg(..) | Op::SetFlag(..) | Op::YieldFlag(..) => true,
                Op::Select { c, .. } | Op::BSelect { c, .. } => v != c,
                _ => pure,
            };
            if kept {
                held |= regs;
            } else {
                uses |= regs;
            }
        }
        for r in entry_regs(uses) {
            self.entry_reads.insert((at, r, Via::Own));
        }
        // A select of one that holds them and one that doesn't is a
        // merge of paths, unknown as [`V::join`] takes it, not a mask.
        let merge = matches!(op, Op::Select { .. } | Op::BSelect { .. });
        if pure && !merge && vals[i].taint().is_none() {
            for r in entry_regs(held & !uses) {
                self.masked.insert((at, r));
            }
        }
    }

    /// Register `r` set to `x` in `s`: `add r7, sp, #12` after `push {r4,
    /// r5, r6, r7, lr}` points r7 at the word it saved r7 in, the frame
    /// record of code built with frame pointers (agb's: r7 in Thumb, r11
    /// in ARM). There every caller's r7 is its frame pointer, never a
    /// parameter, and a read of the entry r7 walks the chain of frames (a
    /// backtrace, agb's on a panic): natively r7 comes in as 0 (`Regs::call`),
    /// so the walk finds no frames. A hole of its own, soft.
    pub(super) fn frame_record(&mut self, s: &State, r: usize, x: V) {
        if let V::Sp(k) = x
            && matches!(r, 7 | 11)
            && s.slots.get(&k) == Some(&V::Entry(r as u8))
        {
            self.frames |= 1 << r;
        }
    }

    /// A return to the caller with `s`, at `at`. A value returned in
    /// r0–r3 with bits of r4–r11's entry values is a use of them where a
    /// caller reads it ([`Summary::entry_returns`]).
    pub(super) fn ret(&mut self, s: &State, at: u32) {
        for (r, v) in s.regs[..4].iter().enumerate() {
            if let Some((regs, _)) = v.taint() {
                for e in entry_regs(regs) {
                    self.entry_reads.insert((at, e, Via::Ret(r as u8)));
                }
            }
        }
        for (r, v) in s.regs[..12].iter().enumerate().skip(4) {
            if *v != V::Entry(r as u8) {
                self.changes |= 1 << r;
            }
        }
        self.exit = Some(match self.exit.take() {
            Some(e) => e.iter().zip(&s.org).map(|(a, b)| a.join(b)).collect(),
            None => s.org.clone(),
        });
    }

    /// The direct callees whose value it returns, by register.
    fn forwards(&self) -> BTreeMap<u8, BTreeSet<u32>> {
        let mut out: BTreeMap<u8, BTreeSet<u32>> = BTreeMap::new();
        for (i, o) in self.exit.iter().flatten().enumerate() {
            for site in &o.sites {
                for c in self.targets(*site, self.sites[site]) {
                    out.entry(SLOTS[i]).or_default().insert(c);
                }
            }
        }
        out
    }

    /// The return state as [`Bits`].
    fn exit_bits(&self) -> [Bits; 5] {
        let mut out = [Bits::default(); 5];
        let Some(e) = &self.exit else { return out };
        for (b, o) in out.iter_mut().zip(e) {
            b.entry = o.entry;
            b.def = o.own;
            b.clob = o.clob;
        }
        for (i, o) in e.iter().enumerate() {
            for site in &o.sites {
                let (_, x) = self.at_site(*site, self.sites[site]);
                out[i].def |= x[i].def;
                out[i].clob |= x[i].clob;
            }
        }
        out
    }
}

/// An entry read's hole, after its address: hard
/// (`analysis::holes::SUMMARY_HARD`).
const ENTRY_READ: &str = ": reads r4-r11 at entry: ";
/// [`ENTRY_READ`] in a function nothing enters by `Regs::call`, nor by
/// a tail call or fall from one so entered: soft.
const JUMPED_READ: &str = ": reads r4-r11 at entry, only jumped into: ";
/// [`ENTRY_READ`] where every way in passes the register
/// ([`Summary::takes`]): soft.
const TAKES_READ: &str = ": takes r4-r11 from its callers: ";

/// Every function's summary, by entry.
#[derive(Clone, Debug, Default)]
pub struct Summaries {
    pub by_entry: BTreeMap<u32, Summary>,
    /// Runs it took to settle, for the report.
    pub runs: usize,
}

impl Summaries {
    pub fn get(&self, entry: u32) -> Option<&Summary> {
        self.by_entry.get(&entry)
    }

    /// Where `entry`'s parameter `r` comes from at the end of its chain of
    /// callees that pass it on: the function and the read that isn't a
    /// pass-on.
    pub fn root(&self, entry: u32, r: u8) -> Option<(u32, Cause)> {
        let (mut f, mut seen) = (entry, BTreeSet::new());
        loop {
            let c = *self.get(f)?.causes.get(&r)?;
            match c {
                Cause::Callee { callee, .. } if seen.insert(f) => f = callee,
                _ => return Some((f, c)),
            }
        }
    }
}

/// A library routine's summary (`Ctx::library`: libgcc's `__muldf3`,
/// `__divdf3`): two doubles in r0-r3, one back in r0 and r1, r2, r3 and
/// r12 left as nothing defines; r4-r11 and sp kept.
fn library_summary(k: u32) -> Summary {
    let def = Bits {
        entry: false,
        def: true,
        clob: false,
    };
    let clob = Bits {
        entry: false,
        def: false,
        clob: true,
    };
    Summary {
        params: 0xf,
        causes: (0..4).map(|r| (r, Cause::Reads(k))).collect(),
        exit: [def, def, clob, clob, clob],
        returns: true,
        keeps: true,
        ..Summary::default()
    }
}

/// The summaries of `funcs`, which should be every function `ctx` has.
pub fn summarize(ctx: &Ctx, funcs: &[Func]) -> Summaries {
    summarize_with(ctx, funcs, Assume::default())
}

/// [`summarize`], assuming `assume` of indirect calls.
pub fn summarize_with(ctx: &Ctx, funcs: &[Func], assume: Assume) -> Summaries {
    summarize_sets(ctx, funcs, assume, &Sets::new())
}

/// [`summarize_with`], an indirect call in `sets` (by its address) taken
/// to call exactly the functions there: it reads their parameters and
/// leaves what they leave, and what is read after it is read after each.
/// `assume` holds for the rest.
pub fn summarize_sets(ctx: &Ctx, funcs: &[Func], assume: Assume, sets: &Sets) -> Summaries {
    summarize_passing(ctx, funcs, assume, sets, &Passing::default())
}

/// [`summarize_sets`], with what a holes file says of r4–r11's passing
/// ([`Passing`]).
pub fn summarize_passing(
    ctx: &Ctx,
    funcs: &[Func],
    assume: Assume,
    sets: &Sets,
    passing: &Passing,
) -> Summaries {
    let proof = prove(ctx, funcs);
    let by_entry: BTreeMap<u32, &Func> = funcs.iter().map(|f| (f.entry, f)).collect();
    let mut sums: BTreeMap<u32, Summary> =
        by_entry.keys().map(|&k| (k, Summary::default())).collect();
    // The library routines the runtime stands in for are leaves of a
    // known convention, never run; the functions inside their code aren't
    // either (a call into one is a hole of its caller's, `Walk`).
    let lib = ctx.library();
    for &k in lib.routines.keys() {
        sums.insert(k, library_summary(k));
    }
    let mut used: BTreeMap<u32, BTreeMap<(u32, u8), Cause>> = BTreeMap::new();
    // Each function's `bl` callees (a tail call's site is its callee).
    let mut bl: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    // Each function's tail callees and the functions it falls into: they
    // take its own r4-r11.
    let mut jumps: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut callers: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut work: VecDeque<u32> = by_entry
        .keys()
        .copied()
        .filter(|&k| !lib.inside(k))
        .collect();
    let mut queued: BTreeSet<u32> = work.iter().copied().collect();
    let mut runs = 0;
    // A run cut short ("didn't settle") isn't monotone in its callees, so
    // in principle two could re-queue each other: a guard.
    let cap = 64 * by_entry.len() + 64;
    while let Some(k) = work.pop_front() {
        queued.remove(&k);
        runs += 1;
        if runs > cap {
            for k in std::mem::take(&mut work) {
                if let Some(s) = sums.get_mut(&k) {
                    s.holes.insert("summaries didn't settle".into());
                    s.stopped = true;
                }
            }
            break;
        }
        let w: Walk = walk(
            ctx,
            &by_entry,
            by_entry[&k],
            Some(Sum::new(&sums, assume, sets, passing, &proof.kept)),
        )
        .expect("a summary's run records what the proof fails on");
        let mut sum = w.sum.expect("a summary's run");
        let mut returned: BTreeMap<u8, BTreeSet<String>> = BTreeMap::new();
        for &(at, r, via) in &sum.entry_reads {
            let hole = if sum.frames & 1 << r != 0 {
                format!("0x{at:08x}: reads the caller's frame pointer: r{r}")
            } else {
                format!("0x{at:08x}{ENTRY_READ}r{r}{}", via.text())
            };
            match via {
                Via::Ret(x) => {
                    returned.entry(x).or_default().insert(hole);
                }
                _ => {
                    sum.holes.insert(hole);
                }
            }
        }
        for &(at, r) in &sum.masked {
            sum.holes
                .insert(format!("0x{at:08x}: masks away r4-r11's entry bits: r{r}"));
        }
        let exit = sum.exit_bits();
        let params = sum.params.keys().fold(0, |m, &r| m | 1 << r);
        let takes = sum
            .entry_reads
            .iter()
            .filter(|&&(_, r, via)| !matches!(via, Via::Ret(_)) && sum.frames & 1 << r == 0)
            .fold(0, |m, &(_, r, _)| m | 1 << (r - 4));
        let new = Summary {
            params,
            causes: sum.params.clone(),
            exit,
            returns: sum.exit.is_some(),
            results: 0,
            read_after: 0,
            passed_after: 0,
            passed_by: BTreeMap::new(),
            forwarded_after: 0,
            forwards: sum.forwards(),
            keeps: proof.kept.contains(&k),
            changes: sum.changes,
            takes,
            passes: sum.passes.clone(),
            holes: sum.holes.clone(),
            stopped: sum.stopped,
            callees: sum.refs.clone(),
            indirect_sites: sum
                .sites
                .iter()
                .filter(|(_, c)| c.is_none())
                .map(|(&a, _)| a)
                .collect(),
            undefined_reads: sum.undefined.clone(),
            entry_returns: returned,
        };
        for &c in &sum.refs {
            callers.entry(c).or_default().insert(k);
        }
        bl.insert(
            k,
            sum.sites
                .iter()
                .filter_map(|(&at, &c)| c.filter(|&c| c != at))
                .collect(),
        );
        jumps.insert(
            k,
            sum.sites
                .iter()
                .filter_map(|(&at, &c)| c.filter(|&c| c == at))
                .chain(sum.falls.iter().copied())
                .collect(),
        );
        used.insert(k, sum.used);
        let old = &sums[&k];
        let grew = old.params != new.params
            || old.exit != new.exit
            || old.changes != new.changes
            || old.takes != new.takes
            || old.returns != new.returns
            || Sum::lost(old) != Sum::lost(&new);
        sums.insert(k, new);
        if grew {
            for &c in callers.get(&k).into_iter().flatten() {
                if queued.insert(c) {
                    work.push_back(c);
                }
            }
        }
    }
    for (caller, u) in used {
        for ((c, r), cause) in u {
            if let Some(s) = sums.get_mut(&c) {
                if let Cause::Reads(_) = cause {
                    s.read_after |= 1 << r;
                } else {
                    s.passed_after |= 1 << r;
                    s.passed_by.entry(r).or_insert((caller, cause));
                }
            }
        }
    }
    // A value a caller returns unread is read where its callers read it,
    // from a chain that starts at a caller's own instruction (a pass-on
    // read is only as good as the callee's parameter it feeds).
    let mut todo: Vec<(u32, u8)> = sums
        .iter()
        .flat_map(|(&k, s)| {
            let m = s.read_after;
            (0..4u8)
                .filter(move |r| m & 1 << r != 0)
                .map(move |r| (k, r))
        })
        .collect();
    while let Some((f, r)) = todo.pop() {
        let cs: Vec<u32> = sums[&f]
            .forwards
            .get(&r)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        for c in cs {
            if let Some(s) = sums.get_mut(&c)
                && (s.read_after | s.forwarded_after) & 1 << r == 0
            {
                s.forwarded_after |= 1 << r;
                todo.push((c, r));
            }
        }
    }
    for s in sums.values_mut() {
        let read = s.read_after | s.passed_after | s.forwarded_after;
        s.results = s.defines() & read & 0xf;
        // Passed on unset to the next callee isn't enough: a callee's
        // parameters may be more than it uses (Emerald's 0x080e24cc passes
        // the r3 a callee's `pop {r3}; mov r8, r3` left).
        let own = s.read_after | s.forwarded_after;
        for (&r, hs) in &s.entry_returns {
            if own & 1 << r != 0 {
                s.holes.extend(hs.iter().cloned());
            }
        }
    }
    // A function that changes r4–r11 and may be called through a pointer:
    // its callers aren't all known, so none can be trusted not to read
    // them. A pointer is the key as a word anywhere in the ROM.
    let words: BTreeSet<u32> = sums
        .iter()
        .filter(|(_, s)| s.changes != 0)
        .map(|(&k, _)| k)
        .collect();
    if !words.is_empty() {
        // Code not in the ROM (a RAM copy's context): any pointer may.
        let found: BTreeSet<u32> = if ctx.base == analysis::program::ROM_BASE {
            ctx.rom
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&w| u32::from_le_bytes(w))
                .filter(|w| words.contains(w))
                .collect()
        } else {
            words
        };
        for k in found {
            sums.get_mut(&k).expect("a summary").holes.insert(format!(
                "0x{k:08x}: changes r4-r11 and may be called through a pointer"
            ));
        }
    }
    // An entry read of r4–r11 is a parameter by register. The lift passes
    // it at a `bl` and a tail call or fall (the extended entry, with the
    // caller's registers; one the caller still has as it came is the
    // caller's read too, `Sum::call`), and at the indirect calls a holes
    // file names: soft, `takes r4-r11 from its callers`. Any other way in
    // is `Regs::call`, which zeroes them: an indirect call (one of a
    // complete callee set, or through a pointer: the key as a word in
    // the ROM), hard. One nothing calls is only jumped into, by functions
    // themselves only jumped into (agb's panic paths, discovery's entries
    // inside another function): soft. An entry value returned in r0–r3
    // isn't passed: hard wherever the function is called, or jumped into
    // from one that is.
    let readers: BTreeSet<u32> = sums
        .iter()
        .filter(|(_, s)| s.holes.iter().any(|h| h.contains(ENTRY_READ)))
        .map(|(&k, _)| k)
        .collect();
    if !readers.is_empty() {
        let mut named: BTreeSet<u32> = passing.sites.values().flatten().copied().collect();
        named.extend(passing.named.iter().copied());
        let mut zeroed: BTreeSet<u32> = sets.values().flatten().copied().collect();
        if ctx.base == analysis::program::ROM_BASE {
            zeroed.extend(
                ctx.rom
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|&w| u32::from_le_bytes(w))
                    .filter(|w| sums.contains_key(w)),
            );
        } else {
            zeroed.extend(sums.keys().copied());
        }
        let mut called: BTreeSet<u32> = bl.values().flatten().copied().collect();
        called.extend(zeroed.iter().copied());
        let mut todo: Vec<u32> = called.iter().copied().collect();
        while let Some(f) = todo.pop() {
            for &c in jumps.get(&f).into_iter().flatten() {
                if called.insert(c) {
                    todo.push(c);
                }
            }
        }
        for k in &readers {
            let s = sums.get_mut(k).expect("a summary");
            s.holes = std::mem::take(&mut s.holes)
                .into_iter()
                .filter_map(|h| {
                    let to = if !called.contains(k) && !named.contains(k) {
                        // Its jumpers' pass-on reads say no more than its own.
                        if h.contains(ENTRY_READ) && h.contains(", which 0x") {
                            return None;
                        }
                        JUMPED_READ
                    } else if h.contains(", returned in r")
                        || zeroed.contains(k) && !named.contains(k)
                    {
                        ENTRY_READ
                    } else {
                        TAKES_READ
                    };
                    Some(h.replace(ENTRY_READ, to))
                })
                .collect();
        }
    }
    for (k, why) in &proof.why {
        if let Some(s) = sums.get_mut(k) {
            s.holes.insert(format!("callee-saves: {why}"));
        }
    }
    Summaries {
        by_entry: sums,
        runs,
    }
}

/// What a prototype gives the registers: its parameters (bit `n` for rN,
/// r0–r3) and its results (r0, or r0 and r1 for 64 bits; none for a
/// struct returned through a hidden first argument). `None` where a
/// parameter's size can't be read (a struct the map doesn't give).
pub fn proto_regs(m: &SourceMap, proto: &str, name: &str) -> Option<(u8, u16)> {
    let (ret, params) = crate::dwarf::proto_params(proto, name)?;
    let words = |t: &str| -> Option<u32> {
        let t = t
            .split_whitespace()
            .filter(|w| !matches!(*w, "const" | "volatile"))
            .collect::<Vec<_>>()
            .join(" ");
        if t.contains('*') || t.contains('[') || t.contains('(') {
            return Some(1);
        }
        match t.as_str() {
            "void" => return Some(0),
            "u64"
            | "s64"
            | "f64"
            | "double"
            | "long long"
            | "unsigned long long"
            | "long long int"
            | "long long unsigned int" => return Some(2),
            _ => {}
        }
        match m.types.get(&t) {
            Some(d) if d.kind != "enum" => Some(d.size.div_ceil(4)),
            None if t.starts_with("struct ") || t.starts_with("union ") => None,
            _ => Some(1),
        }
    };
    // A struct or union, by tag or typedef.
    let aggregate = |t: &str| {
        let t = t.replace("const ", "").replace("volatile ", "");
        let t = t.trim();
        t.starts_with("struct ")
            || t.starts_with("union ")
            || m.types.get(t).is_some_and(|d| d.kind != "enum")
    };
    let (mut at, mut results) = (0u32, 0u16);
    match words(&ret)? {
        0 => {}
        1 => results = 1,
        2 if !aggregate(&ret) => results = 0b11,
        // Returned in memory, through a hidden first argument.
        _ => at = 1,
    }
    let mut mask = (1u8 << at) - 1;
    for (_, ty) in &params {
        let n = if ty == "..." { 4 } else { words(ty)? };
        for _ in 0..n {
            if at < 4 {
                mask |= 1 << at;
            }
            at += 1;
        }
    }
    Some((mask, results))
}

/// A way a summary disagrees with its prototype.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// It reads a register past the prototype's parameters itself.
    ParamRead,
    /// It passes one on to a callee whose summary reads it (at the end of
    /// the chain of callees that pass it on, an instruction reads it).
    ParamCallee,
    /// An indirect call, taken to read r0–r3, reads one (here, or at the
    /// end of a chain of callees that pass it on).
    ParamIndirect,
    /// A BIOS call reads one.
    ParamBios,
    /// A caller's own instruction reads a register after it that the
    /// prototype doesn't return.
    ResultReadNotReturned,
    /// A caller passes a register it left, that the prototype doesn't
    /// return, on to a callee whose summary reads it.
    ResultPassedNotReturned,
    /// The prototype returns a register a return path leaves undefined.
    ResultUndefined,
    /// The prototype returns a value and no path returns.
    NeverReturns,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::ParamRead,
        Category::ParamCallee,
        Category::ParamIndirect,
        Category::ParamBios,
        Category::ResultReadNotReturned,
        Category::ResultPassedNotReturned,
        Category::ResultUndefined,
        Category::NeverReturns,
    ];

    pub fn text(self) -> &'static str {
        match self {
            Category::ParamRead => "reads a register past its parameters itself",
            Category::ParamCallee => "passes a register past its parameters to a callee",
            Category::ParamIndirect => "an indirect call reads r0-r3 (maybe through callees)",
            Category::ParamBios => "a BIOS call reads a register past its parameters",
            Category::ResultReadNotReturned => "a caller reads a register it doesn't return",
            Category::ResultPassedNotReturned => {
                "a caller passes on or returns a register it doesn't return"
            }
            Category::ResultUndefined => "a return leaves its result undefined",
            Category::NeverReturns => "returns a value but never returns",
        }
    }
}

/// One function's summary against its prototype.
#[derive(Clone, Debug)]
pub struct Verdict {
    pub entry: u32,
    pub name: String,
    pub proto: String,
    /// The prototype's parameters and results ([`proto_regs`]).
    pub dwarf: (u8, u16),
    /// Each disagreement and what it is, in words.
    pub why: Vec<(Category, String)>,
}

/// Summaries against prototypes, over the functions both have.
#[derive(Clone, Debug, Default)]
pub struct Comparison {
    pub agree: Vec<Verdict>,
    pub disagree: Vec<Verdict>,
    /// Summarized functions the map gives no prototype for.
    pub no_proto: usize,
    /// Prototypes [`proto_regs`] can't read.
    pub unreadable: usize,
}

fn regs(m: u16) -> String {
    let v: Vec<String> = (0..16)
        .filter(|r| m & 1 << r != 0)
        .map(|r| format!("r{r}"))
        .collect();
    if v.is_empty() {
        "none".into()
    } else {
        v.join(",")
    }
}

/// Agree means the summary's parameters are among the prototype's, what
/// callers read after it is among what it returns, and it defines what
/// the prototype returns on every return.
pub fn compare(sums: &Summaries, m: &SourceMap) -> Comparison {
    let mut out = Comparison::default();
    for (&k, s) in &sums.by_entry {
        let Some(item) = m.functions.get(&k) else {
            out.no_proto += 1;
            continue;
        };
        let Some(proto) = &item.proto else {
            out.no_proto += 1;
            continue;
        };
        let Some((dp, dr)) = proto_regs(m, proto, &item.name) else {
            out.unreadable += 1;
            continue;
        };
        let mut why = Vec::new();
        for (&r, &c) in &s.causes {
            if dp & 1 << r != 0 {
                continue;
            }
            let (cat, text) = match c {
                Cause::Reads(at) => (Category::ParamRead, format!("r{r} read at 0x{at:08x}")),
                Cause::Callee { callee, at } => {
                    let root = sums.root(callee, r);
                    let cat = match root {
                        Some((_, Cause::Indirect(_))) => Category::ParamIndirect,
                        Some((_, Cause::Bios { .. })) => Category::ParamBios,
                        _ => Category::ParamCallee,
                    };
                    let text = match root {
                        Some((f, c)) => format!(
                            "r{r} passed to 0x{callee:08x} at 0x{at:08x}; from 0x{f:08x}'s {c:x?}"
                        ),
                        None => format!("r{r} passed to 0x{callee:08x} at 0x{at:08x}"),
                    };
                    (cat, text)
                }
                Cause::Indirect(at) => (
                    Category::ParamIndirect,
                    format!("r{r} at the indirect call at 0x{at:08x}"),
                ),
                Cause::Bios { n, at } => (
                    Category::ParamBios,
                    format!("r{r} read by swi 0x{n:02x} at 0x{at:08x}"),
                ),
            };
            why.push((cat, text));
        }
        let extra = s.read_after & !dr;
        if extra != 0 {
            why.push((
                Category::ResultReadNotReturned,
                format!("callers read {} after it", regs(extra)),
            ));
        }
        let extra = s.forwarded_after & !(s.read_after | s.passed_after) & !dr;
        if extra != 0 {
            why.push((
                Category::ResultPassedNotReturned,
                format!("callers return {} from it as their own result", regs(extra)),
            ));
        }
        let extra = s.passed_after & !s.read_after & !dr;
        if extra != 0 {
            let how: Vec<String> = s
                .passed_by
                .iter()
                .filter(|&(&r, _)| extra & 1 << r != 0)
                .map(|(&r, &(caller, c))| {
                    let root = match c {
                        Cause::Callee { callee, .. } => sums.root(callee, r),
                        _ => Some((caller, c)),
                    };
                    match root {
                        Some((f, rc)) if rc != c => {
                            format!("r{r} by 0x{caller:08x}'s {c:x?}, from 0x{f:08x}'s {rc:x?}")
                        }
                        _ => format!("r{r} by 0x{caller:08x}'s {c:x?}"),
                    }
                })
                .collect();
            why.push((
                Category::ResultPassedNotReturned,
                format!(
                    "callers pass {} on after it: {}",
                    regs(extra),
                    how.join("; ")
                ),
            ));
        }
        if dr != 0 && !s.returns {
            why.push((Category::NeverReturns, "no path returns".into()));
        } else if dr & !s.defines() != 0 {
            why.push((
                Category::ResultUndefined,
                format!("{} not defined on every return", regs(dr & !s.defines())),
            ));
        }
        let v = Verdict {
            entry: k,
            name: item.name.clone(),
            proto: proto.clone(),
            dwarf: (dp, dr),
            why,
        };
        if v.why.is_empty() {
            out.agree.push(v);
        } else {
            out.disagree.push(v);
        }
    }
    out
}
