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
//! says it does.
//!
//! [`compare`] checks summaries against DWARF prototypes ([`proto_regs`]):
//! the gate the stage-A lifter's parameter lists and return values rest
//! on.

use super::{State, Walk, prove, walk};
use crate::func::{Ctx, Func};
use crate::names::SourceMap;
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
    /// read by a caller after a direct call.
    pub results: u16,
    /// The registers callers read after a direct call to it (bit `n` for
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
    pub undefined_reads: BTreeMap<(u32, u8), Option<u32>>,
}

impl Summary {
    /// The registers (bit `n` for rN, r0–r3 and r12) defined on every
    /// return: never a leftover, and the entry value only where it's a
    /// parameter.
    pub fn defines(&self) -> u16 {
        defines(self.params, &self.exit)
    }
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

/// A summary's run: the callees' summaries so far, and what it finds.
pub(super) struct Sum<'a> {
    sums: &'a BTreeMap<u32, Summary>,
    assume: Assume,
    params: BTreeMap<u8, Cause>,
    /// r0–r3 and r12's origins, joined over every return.
    exit: Option<Vec<Org>>,
    /// (callee, register): a read after a direct call of what it left,
    /// the first found.
    used: BTreeMap<(u32, u8), Cause>,
    undefined: BTreeMap<(u32, u8), Option<u32>>,
    pub(super) holes: BTreeSet<String>,
    /// A path's run ended at a hole ([`Summary::stopped`]).
    pub(super) stopped: bool,
    /// Each call site's callee, `None` for an indirect one.
    sites: BTreeMap<u32, Option<u32>>,
    /// The functions whose summaries this run used.
    refs: BTreeSet<u32>,
}

impl<'a> Sum<'a> {
    fn new(sums: &'a BTreeMap<u32, Summary>, assume: Assume) -> Sum<'a> {
        Sum {
            sums,
            assume,
            params: BTreeMap::new(),
            exit: None,
            used: BTreeMap::new(),
            undefined: BTreeMap::new(),
            holes: BTreeSet::new(),
            stopped: false,
            sites: BTreeMap::new(),
            refs: BTreeSet::new(),
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

    /// No return found, and a hole that may be why.
    fn lost(s: &Summary) -> bool {
        !s.returns && !s.holes.is_empty()
    }

    /// Register `r` read in state `s`, for `cause`.
    pub(super) fn read(&mut self, s: &State, r: usize, cause: Cause) {
        let Some(i) = slot(r) else { return };
        let o = &s.org[i];
        if o.entry && r < 4 {
            self.params.entry(r as u8).or_insert(cause);
        }
        for site in &o.sites {
            let callee = self.sites[site];
            if let Some(c) = callee {
                self.used.entry((c, r as u8)).or_insert(cause);
            }
            let (p, x) = self.callee(callee);
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
        self.refs.extend(callee);
        if let Some(c) = callee
            && self.sums.get(&c).is_some_and(Self::lost)
        {
            self.holes.insert(format!(
                "calls 0x{c:08x}, whose summary has holes and no return"
            ));
        }
        let (params, exit) = self.callee(callee);
        for r in 0..4 {
            if params & 1 << r != 0 {
                let cause = match callee {
                    Some(c) => Cause::Callee { callee: c, at },
                    None => Cause::Indirect(at),
                };
                self.read(s, r, cause);
            }
        }
        self.sites.insert(at, callee);
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
        for r in 0..4 {
            s.org[r] = Org {
                own: defs & 1 << r != 0,
                clob: defs & 1 << r == 0,
                ..Org::default()
            };
        }
    }

    /// A return to the caller with `s`.
    pub(super) fn ret(&mut self, s: &State) {
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
                if let Some(c) = self.sites[site] {
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
                let (_, x) = self.callee(self.sites[site]);
                out[i].def |= x[i].def;
                out[i].clob |= x[i].clob;
            }
        }
        out
    }
}

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

/// The summaries of `funcs`, which should be every function `ctx` has.
pub fn summarize(ctx: &Ctx, funcs: &[Func]) -> Summaries {
    summarize_with(ctx, funcs, Assume::default())
}

/// [`summarize`], assuming `assume` of indirect calls.
pub fn summarize_with(ctx: &Ctx, funcs: &[Func], assume: Assume) -> Summaries {
    let proof = prove(ctx, funcs);
    let by_entry: BTreeMap<u32, &Func> = funcs.iter().map(|f| (f.entry, f)).collect();
    let mut sums: BTreeMap<u32, Summary> =
        by_entry.keys().map(|&k| (k, Summary::default())).collect();
    let mut used: BTreeMap<u32, BTreeMap<(u32, u8), Cause>> = BTreeMap::new();
    let mut callers: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut work: VecDeque<u32> = by_entry.keys().copied().collect();
    let mut queued: BTreeSet<u32> = by_entry.keys().copied().collect();
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
        let w: Walk = walk(ctx, &by_entry, by_entry[&k], Some(Sum::new(&sums, assume)))
            .expect("a summary's run records what the proof fails on");
        let sum = w.sum.expect("a summary's run");
        let exit = sum.exit_bits();
        let params = sum.params.keys().fold(0, |m, &r| m | 1 << r);
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
            holes: sum.holes.clone(),
            stopped: sum.stopped,
            callees: sum.refs.clone(),
            undefined_reads: sum.undefined.clone(),
        };
        for &c in &sum.refs {
            callers.entry(c).or_default().insert(k);
        }
        used.insert(k, sum.used);
        let old = &sums[&k];
        let grew =
            old.params != new.params || old.exit != new.exit || Sum::lost(old) != Sum::lost(&new);
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
