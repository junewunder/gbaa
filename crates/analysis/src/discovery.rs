//! Code discovery (decision 5): which ROM bytes are code, in which
//! instruction set, and where the functions start.
//!
//! - **Recursive descent** (5a) from the ROM entry. Each root is analysed by
//!   the value-set analysis ([`crate::vsa::func`]), which follows `b`,
//!   `b<cond>`, far-jump `bl`s and resolved jump tables (5c). What a root
//!   calls (`bl` targets, constant targets of indirect calls, rN of a
//!   `bl` to a local `bx rN` stub) becomes a function; a resolved jump the
//!   analysis can't follow (5b: into the other instruction set, or out of
//!   the function) becomes a function, or, inside its own function with no
//!   data between the jump and its target, a continuation root (agb's
//!   `crt0` jumps over its literal pool into `main`).
//! - **BIOS copies.** A `CpuSet`/`CpuFastSet` out of ROM with constant
//!   arguments ([`Discovery::copies`]) makes its source data: agb's `crt0`
//!   copies its IWRAM and EWRAM sections that way, and their code runs at
//!   the destination.
//! - **The far-jump `bl` rule** (5a) is [`Engine`]'s fixpoint, shared by
//!   every round. It also says which calls never return (nothing follows
//!   them). A `bl` from a function with a prologue into the span of its own
//!   code is a jump too ([`crate::vsa::func`]): LLVM's long branches to
//!   cold blocks and to the chunks of a `main` that never returns.
//! - **Run roots.** [`discover_with`] takes more starts known to be code
//!   (a run's calls through a register, from `gbaa-holes`) into the first
//!   descent beside the entry; a BIOS copy's source ends at the first of
//!   them inside it.
//! - **The pointer scan** (5d, [`scan`]) runs once descent has nothing left:
//!   ROM words pointing at ROM become candidates, validated and ranked, and
//!   the accepted ones seed more descent. Functions and code found that way
//!   are [`Evidence::Scanned`], kept apart from what descent from the entry
//!   confirmed. A scanned start that another root's code runs straight
//!   into isn't a start ([`Driver::demote`]): a stray word pointing past
//!   the true start doesn't cut the function. A scanned root that reaches
//!   an undefined instruction (not a trap) and that nothing else calls,
//!   or that jumps to such a root, is data ([`Driver::undef`]) and
//!   dropped, as one that hits the step cap is: otherwise the lifter
//!   decodes on from where it leads, through whole tables (Emerald's
//!   species data, 2026-11-07).
//!
//! Function extents come from the starts found so far (each runs to the
//! next start), and every literal pool and table read is data, which ends
//! descent (a call that never returns would otherwise run into its pool).
//! Both grow as discovery goes, so each round redoes the analyses a new
//! start or new data can change: a new start inside the extents a result
//! consulted, or new data over code it read. Once descent from the entry
//! is done its analyses are kept as they are: the starts and pools the
//! scan finds are guesses, and don't reshape confirmed code.
//!
//! Prior art and measurements: `docs/research/discovery-prior-art.md`,
//! `docs/research/jump-tables.md`.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

use armv4t::decode::{Cond, Instr};

use crate::jumptable::{Engine, Key, Recovery};
use crate::program::{Func, Program, ROM_BASE};
use crate::vsa::exec::writes_pc;
use crate::vsa::func::JumpRes;

pub mod scan;

#[cfg(test)]
mod tests;

/// The step cap of each analysis. The value-set analysis of a real
/// function takes far fewer steps; one that hits this cap has run into data.
pub const STEP_CAP: u64 = 50_000;

/// How a root was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Evidence {
    /// Reached by descent from the ROM entry.
    Confirmed,
    /// Reached only from pointer-scan candidates.
    Scanned,
}

/// What a root is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Func,
    /// Code that a jump inside its own function enters in the other
    /// instruction set (`adr r1, 1f; bx r1`): analysed from there, not a
    /// function start.
    Entry,
}

/// One instruction found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Code {
    pub size: u32,
    pub thumb: bool,
    pub evidence: Evidence,
}

#[derive(Debug, Default)]
pub struct Discovery {
    /// Function starts, in interworking form (bit 0 set for Thumb).
    pub funcs: BTreeMap<u32, Evidence>,
    /// Continuation roots ([`Kind::Entry`]), in interworking form.
    pub entries: BTreeMap<u32, Evidence>,
    /// Every instruction reached, by address.
    pub code: BTreeMap<u32, Code>,
    /// Literal pools, jump tables and the sources of [`copies`](Discovery::copies),
    /// `start..end`, sorted and disjoint.
    pub data: Vec<(u32, u32)>,
    /// What the code copies out of ROM with the BIOS (agb's `crt0`: the
    /// IWRAM and EWRAM sections, code included): (source, destination,
    /// bytes). The source is data here; the code runs at the destination.
    pub copies: BTreeSet<(u32, u32, u32)>,
    /// Constant code addresses in EWRAM or IWRAM the code reaches, in
    /// interworking form: call and jump targets, and the IRQ handler the
    /// code installs. Where [`copies`](Discovery::copies)' code is entered.
    pub ram_entries: BTreeSet<u32>,
    /// The jumps and far-jump `bl`s over every root.
    pub rec: Recovery,
    /// Descent rounds run.
    pub rounds: usize,
    /// Scanned roots dropped because their analysis hit [`STEP_CAP`].
    pub dropped: usize,
    /// Scanned roots dropped as data: they reach an undefined instruction,
    /// or jump to a root that does ([`Driver::undef`]).
    pub undefined: usize,
    pub scan: scan::Stats,
}

impl Discovery {
    /// The resolved jump tables (more than one target).
    pub fn tables(&self) -> usize {
        self.rec
            .resolved()
            .filter(|(_, j)| j.targets.len() > 1)
            .count()
    }

    /// The discovered-starts file of the discovery-accuracy check
    /// (`docs/research/discovery-prior-art.md`), each start commented with
    /// its evidence, and the far-jump `bl`s listed in comments.
    pub fn starts_file(&self) -> String {
        let mut out = String::from("# gbaa code discovery: function starts (bit 0 set = Thumb)\n");
        for (a, e) in &self.funcs {
            let e = match e {
                Evidence::Confirmed => "confirmed",
                Evidence::Scanned => "scanned",
            };
            out += &format!("0x{a:08x}  # {e}\n");
        }
        // For reading, not scoring: the far jumps, as comments.
        for b in &self.rec.far_jumps {
            out += &format!("# far-jump bl at 0x{b:08x}\n");
        }
        out
    }
}

/// Discover the code of a ROM image (loaded at [`ROM_BASE`]).
pub fn discover(rom: Vec<u8>) -> Discovery {
    discover_with(rom, &[])
}

/// [`discover`], with more function starts (interworking form) known to be
/// code: the call targets a run saw (`gbaa-holes`: m4a's routines called
/// through its table in RAM, code reached only through computed
/// pointers). They join the ROM entry in the first descent, so what they
/// lead to is [`Evidence::Confirmed`] and the pointer scan works around
/// them. Starts outside ROM are ignored.
pub fn discover_with(rom: Vec<u8>, roots: &[u32]) -> Discovery {
    // The cartridge header starts with an ARM branch to the entry code.
    let mut entry = vec![ROM_BASE];
    entry.extend(roots.iter().copied().filter(|&a| a != ROM_BASE));
    let mut d = Driver::new(Program::new(rom, Vec::new(), Vec::new()), entry.clone());
    d.extra = roots.iter().map(|a| a & !1).collect();
    d.descend(entry.into_iter().map(|a| (a, Kind::Func)).collect());
    // Descent from the entry is done: what it found is confirmed, and the
    // starts and pools the scan finds from here on don't change it.
    d.eng.kept = d.eng.results.keys().copied().collect();
    let mut stats = scan::Stats::default();
    loop {
        let view = d.view();
        let far = |t: u32, thumb: bool| d.eng.known((t, thumb));
        let seeds = scan::batch(&d.prog, &view, &far, &mut stats);
        if seeds.is_empty() {
            break;
        }
        d.descend(seeds.into_iter().map(|a| (a, Kind::Func)).collect());
    }
    let mut out = d.finish();
    out.scan = stats;
    out
}

/// Discover the code of `image` mapped at `base`, by descent from `roots`
/// (interworking form) alone, with no pointer scan: code a ROM copies to
/// RAM, analysed where it runs. What it calls outside the image isn't
/// followed.
pub fn discover_at(base: u32, image: Vec<u8>, roots: &[u32]) -> Discovery {
    let mut d = Driver::new(Program::at(base, image), roots.to_vec());
    d.descend(roots.iter().map(|&a| (a, Kind::Func)).collect());
    d.finish()
}

/// Code a ROM copies into IWRAM and runs there (decision 7): one BIOS copy
/// ([`Discovery::copies`]) and what discovery finds in it at its
/// destination.
#[derive(Debug)]
pub struct Overlay {
    /// The ROM address of the bytes.
    pub src: u32,
    /// Where they run.
    pub dst: u32,
    pub len: u32,
    /// The copy's code, at `dst`. Its roots are the ROM's
    /// [`ram_entries`](Discovery::ram_entries) inside the copy.
    pub disc: Discovery,
}

impl Overlay {
    /// The ROM address the byte at `a` (in `dst..dst + len`) came from.
    pub fn rom_addr(&self, a: u32) -> u32 {
        self.src + (a - self.dst)
    }
}

/// The IWRAM copies of `disc` (discovered from `rom`) that some
/// [`ram_entries`](Discovery::ram_entries) enter, each discovered at its
/// destination. A copy whose destination overlaps an earlier one's is
/// left out: one address, one overlay.
pub fn overlays(rom: &[u8], disc: &Discovery) -> Vec<Overlay> {
    let mut out: Vec<Overlay> = Vec::new();
    for &(src, dst, len) in &disc.copies {
        let iwram = (0x0300_0000..0x0300_8000).contains(&dst) && dst + len <= 0x0300_8000;
        if !iwram || out.iter().any(|o| dst < o.dst + o.len && o.dst < dst + len) {
            continue;
        }
        let roots: Vec<u32> = disc
            .ram_entries
            .iter()
            .copied()
            .filter(|&k| (dst..dst + len).contains(&(k & !1)))
            .collect();
        let o = (src - ROM_BASE) as usize;
        let Some(image) = rom.get(o..o + len as usize) else {
            continue;
        };
        if roots.is_empty() {
            continue;
        }
        out.push(Overlay {
            src,
            dst,
            len,
            disc: discover_at(dst, image.to_vec(), &roots),
        });
    }
    out
}

struct Driver {
    prog: Program,
    /// What descent starts from: the confirmed code is what they lead to.
    entry: Vec<u32>,
    /// The extra starts of [`discover_with`], aligned: code in ROM even
    /// where a BIOS copy reads them as its source.
    extra: BTreeSet<u32>,
    eng: Engine,
    roots: BTreeMap<Key, Kind>,
    /// Pools and tables found, as `start..end`, possibly overlapping.
    data: BTreeSet<(u32, u32)>,
    /// Roots whose analysis hit the step cap.
    capped: HashSet<Key>,
    /// Scanned roots taken for data ([`Driver::undef`]): treated as a
    /// capped one is.
    bad: HashSet<Key>,
    /// Scanned function roots that another root's code runs straight
    /// into ([`Driver::demote`]): analysed, but not function starts.
    demoted: HashSet<Key>,
    /// The function starts of the current extents.
    starts: BTreeSet<u32>,
    rounds: usize,
    /// Print each round to stderr (`GBAA_DISC_LOG` set).
    log: bool,
    t0: std::time::Instant,
}

/// What the scan needs to know about discovery so far.
pub struct View {
    /// Roots, in interworking form.
    pub roots: HashSet<u32>,
    /// Function starts, in interworking form.
    pub funcs: HashSet<u32>,
    /// Instructions reached: (address, size, thumb), sorted.
    pub code: Vec<(u32, u32, bool)>,
    /// Literal-pool word addresses read by code.
    pub pools: HashSet<u32>,
    /// Tables of resolved jumps, `start..end`.
    pub tables: Vec<(u32, u32)>,
}

impl View {
    /// The instruction covering `a`, if any.
    pub fn insn_at(&self, a: u32) -> Option<(u32, u32, bool)> {
        let i = self.code.partition_point(|c| c.0 <= a);
        let c = *self.code.get(i.checked_sub(1)?)?;
        (a < c.0 + c.1).then_some(c)
    }

    pub fn in_table(&self, a: u32) -> bool {
        let i = self.tables.partition_point(|t| t.0 <= a);
        i > 0 && a < self.tables[i - 1].1
    }
}

fn iw(k: Key) -> u32 {
    k.0 | k.1 as u32
}

fn key(a: u32) -> Key {
    (a & !1, a & 1 != 0)
}

impl Driver {
    fn new(prog: Program, entry: Vec<u32>) -> Driver {
        let log = std::env::var_os("GBAA_DISC_LOG").is_some();
        let mut eng = Engine::default();
        eng.log = log;
        eng.step_cap = Some(STEP_CAP);
        Driver {
            prog,
            entry,
            extra: BTreeSet::new(),
            eng,
            roots: BTreeMap::new(),
            data: BTreeSet::new(),
            capped: HashSet::new(),
            bad: HashSet::new(),
            demoted: HashSet::new(),
            starts: BTreeSet::new(),
            rounds: 0,
            log,
            t0: std::time::Instant::now(),
        }
    }

    /// Analyse new roots, and everything they lead to, until nothing new
    /// turns up.
    fn descend(&mut self, mut new: Vec<(u32, Kind)>) {
        let mut new_data: Vec<(u32, u32)> = Vec::new();
        loop {
            let mut added: Vec<Key> = Vec::new();
            for (a, kind) in new.drain(..) {
                let k = key(a);
                if !self.prog.in_rom(k.0) || (!k.1 && !k.0.is_multiple_of(4)) {
                    continue;
                }
                match self.roots.get_mut(&k) {
                    Some(old) => {
                        if kind == Kind::Func {
                            *old = Kind::Func;
                        }
                    }
                    None => {
                        self.roots.insert(k, kind);
                        added.push(k);
                    }
                }
            }
            // Analyses that hit the cap have run into data: their starts
            // don't bound other functions.
            for (k, r) in &self.eng.results {
                if r.capped && self.roots.contains_key(k) {
                    self.capped.insert(*k);
                }
            }
            self.undef();
            self.demote();
            let funcs = self.extents();
            let now: BTreeSet<u32> = funcs.iter().map(|f| f.start).collect();
            // Starts added or removed since the last round.
            let mut starts: Vec<u32> = now.symmetric_difference(&self.starts).copied().collect();
            if added.is_empty() && starts.is_empty() && new_data.is_empty() {
                break;
            }
            self.rounds += 1;
            if !starts.is_empty() {
                self.prog.set_funcs(funcs);
                self.starts = now;
            }
            if !new_data.is_empty() {
                self.prog.set_data(self.data.iter().copied().collect());
            }
            starts.sort_unstable();
            let covered: HashSet<u32> = new_data
                .iter()
                .flat_map(|&(a, b)| (a & !1..b).step_by(2))
                .collect();
            let dirty = self.eng.stale(|r| {
                r.span.is_some_and(|(lo, hi)| {
                    let i = starts.partition_point(|&s| s < lo);
                    starts.get(i).is_some_and(|&s| s < hi)
                }) || (!covered.is_empty()
                    && (r.insns.iter().any(|&(a, n)| {
                        covered.contains(&a) || (n == 4 && covered.contains(&(a + 2)))
                    }) || r
                        .reads
                        .iter()
                        .any(|&(a, b)| (a..b).step_by(2).any(|h| covered.contains(&h)))))
            });
            if self.log {
                eprintln!(
                    "round {}: {} roots, {} new ({} ARM), {} new starts, {} new data, {} redone, {} analyses, {:.1} s",
                    self.rounds,
                    self.roots.len(),
                    added.len(),
                    added.iter().filter(|k| !k.1).count(),
                    starts.len(),
                    new_data.len(),
                    dirty.len(),
                    self.eng.analyses,
                    self.t0.elapsed().as_secs_f64()
                );
            }
            self.eng.run(&self.prog, added.into_iter().chain(dirty));
            new_data.clear();
            self.gather(&mut new, &mut new_data);
        }
    }

    /// Function extents: each start runs to the next one. Of two roots at
    /// one address (one per instruction set), only the first counts.
    fn extents(&self) -> Vec<Func> {
        let mut starts: Vec<Key> = self
            .roots
            .iter()
            .filter(|(k, kind)| {
                **kind == Kind::Func
                    && !self.capped.contains(k)
                    && !self.bad.contains(k)
                    && !self.demoted.contains(k)
            })
            .map(|(k, _)| *k)
            .collect();
        starts.dedup_by_key(|k| k.0);
        let end = self.prog.base() + self.prog.rom().len() as u32;
        starts
            .iter()
            .enumerate()
            .map(|(i, k)| Func {
                start: k.0,
                end: starts.get(i + 1).map_or(end, |n| n.0),
                thumb: k.1,
            })
            .collect()
    }

    /// Find the function roots, other than ones descent from the entry
    /// confirmed, that another root's code runs straight into: the
    /// instruction before is reached and continues into it (not a branch,
    /// a return or a `bl`, which may be a call that never returns).
    /// Functions don't fall into each other, so such a start is a stray
    /// word pointing into the middle of a function, and doesn't cut it.
    fn demote(&mut self) {
        if self.eng.kept.is_empty() {
            return;
        }
        let funcs: HashSet<Key> = self
            .roots
            .iter()
            .filter(|(k, kind)| **kind == Kind::Func && !self.eng.kept.contains(k))
            .map(|(k, _)| *k)
            .collect();
        for (k, r) in &self.eng.results {
            if self.dead(k) || !self.roots.contains_key(k) {
                continue;
            }
            for &(a, n) in &r.insns {
                let x = (a + n, k.1);
                if x == *k || !funcs.contains(&x) || self.demoted.contains(&x) {
                    continue;
                }
                let Some(i) = self.prog.decode(a, k.1) else {
                    continue;
                };
                let ends = i.bl.is_some()
                    || (i.instr.cond() == Cond::Al
                        && (matches!(i.instr, Instr::Branch { link: false, .. })
                            || writes_pc(&i.instr)));
                if !ends {
                    self.demoted.insert(x);
                }
            }
        }
    }

    /// Whether a root's analysis counts for nothing: it hit the step cap,
    /// or it is [`bad`](Driver::bad).
    fn dead(&self, k: &Key) -> bool {
        self.eng.results[k].capped || self.bad.contains(k)
    }

    /// Find the scanned roots that reach an undefined instruction (not a
    /// trap: [`crate::vsa::func::deliberate`]) and that no other root calls
    /// but one so found, or that jump to one: data the scan took for code
    /// (a word in Emerald's species table pointing at a run of `movs r0,
    /// r0` whose `bx` lands on ARM-decoded data). Descent from the entry
    /// is never judged so. Once found, a root stays found.
    fn undef(&mut self) {
        if self.eng.kept.is_empty() {
            return;
        }
        let res = |k: &Key| self.eng.results.get(k).filter(|r| !r.capped);
        let judged = |k: &Key| !self.eng.kept.contains(k) && res(k).is_some();
        // `set` and every judged root that jumps to one in it. Only a
        // jump: a call's constant target is often the analysis'
        // imprecision, and a real function would go with it.
        let up = |mut set: HashSet<Key>| {
            let mut grew = true;
            while grew {
                grew = false;
                for k in self.roots.keys() {
                    if set.contains(k) || !judged(k) {
                        continue;
                    }
                    if res(k).is_some_and(|r| r.exits.iter().any(|&(_, t)| set.contains(&key(t)))) {
                        set.insert(*k);
                        grew = true;
                    }
                }
            }
            set
        };
        let reach: HashSet<Key> = self
            .roots
            .keys()
            .filter(|k| judged(k) && res(k).is_some_and(|r| r.undefined.is_some()))
            .copied()
            .collect();
        let tainted = up(reach.iter().chain(&self.bad).copied().collect());
        // What an untainted root calls is a function, whatever runs after
        // a call of its that never returns: Metroid starts its embedded NES
        // image at the image's first word, a `bl` before the header.
        let called: HashSet<Key> = self
            .roots
            .keys()
            .filter(|k| !tainted.contains(k))
            .filter_map(res)
            .flat_map(|r| r.calls.iter().map(|&t| key(t)))
            .collect();
        let seeds = reach
            .iter()
            .filter(|k| !called.contains(k))
            .chain(&self.bad)
            .copied()
            .collect();
        let bad = up(seeds);
        if self.log {
            for k in bad.difference(&self.bad) {
                match res(k).and_then(|r| r.undefined) {
                    Some(u) => eprintln!("undefined: drop {:#010x}, reaches {u:#010x}", iw(*k)),
                    None => eprintln!("undefined: drop {:#010x}, jumps to one", iw(*k)),
                }
            }
        }
        self.bad = bad;
    }

    /// The roots the analyses lead to, and data not seen before.
    fn gather(&mut self, new: &mut Vec<(u32, Kind)>, new_data: &mut Vec<(u32, u32)>) {
        for k in self.roots.keys() {
            let r = &self.eng.results[k];
            // A capped or bad analysis has run into data: nothing it found
            // counts. A kept one was gathered against the extents it was
            // analysed with; the current ones may hold scanned starts.
            if self.dead(k) || self.eng.kept.contains(k) {
                continue;
            }
            new.extend(r.calls.iter().map(|&t| (t, Kind::Func)));
            for &(at, t) in &r.exits {
                // A mode switch inside a function (`adr r1, 1f; bx r1`) has
                // no data between the jump and its target; agb's `crt0`
                // jumps over its literal pool into `main`.
                let (lo, hi) = (at.min(t & !1), at.max(t & !1));
                let over = |&(a, b): &(u32, u32)| a < hi && b > lo;
                let inside = self
                    .prog
                    .func_at(at)
                    .is_some_and(|f| (f.start..f.end).contains(&(t & !1)))
                    && !r.pools.iter().any(over)
                    && !self.data.iter().any(over);
                new.push((t, if inside { Kind::Entry } else { Kind::Func }));
            }
            let tables = r.jumps.values().filter_map(|j| match j {
                JumpRes::Resolved(j) => j.table.map(|s| (s.first, s.last + s.size)),
                _ => None,
            });
            // A copy longer than its code (m4a's `SoundMainRAM`, copied
            // 2 KiB at a time) reads the ROM functions after it: its
            // source ends at the first extra start inside it.
            let extra = &self.extra;
            let copies = r.copies.iter().map(|&(s, _, n)| {
                (
                    s,
                    extra.range(s + 1..s + n).next().copied().unwrap_or(s + n),
                )
            });
            for d in r.pools.iter().copied().chain(tables).chain(copies) {
                if self.data.insert(d) {
                    new_data.push(d);
                }
            }
        }
        new.retain(|&(a, kind)| {
            self.roots
                .get(&key(a))
                .is_none_or(|&k| k != kind && kind == Kind::Func)
        });
    }

    fn view(&self) -> View {
        let mut code: Vec<(u32, u32, bool)> = self
            .roots
            .keys()
            .filter(|k| !self.dead(k))
            .flat_map(|k| self.eng.results[k].insns.iter().map(|&(a, n)| (a, n, k.1)))
            .collect();
        code.sort_unstable();
        code.dedup();
        let mut pools = HashSet::new();
        let mut tables = Vec::new();
        for k in self.roots.keys() {
            let r = &self.eng.results[k];
            if self.dead(k) {
                continue;
            }
            for &(a, _) in &r.pools {
                pools.insert(a & !3);
            }
            for j in r.jumps.values() {
                if let JumpRes::Resolved(j) = j
                    && let Some(s) = j.table
                {
                    tables.push((s.first, s.last + s.size));
                }
            }
        }
        tables.sort_unstable();
        View {
            roots: self.roots.keys().map(|&k| iw(k)).collect(),
            funcs: self
                .roots
                .iter()
                .filter(|(k, kind)| {
                    **kind == Kind::Func && !self.dead(k) && !self.demoted.contains(k)
                })
                .map(|(&k, _)| iw(k))
                .collect(),
            code,
            pools,
            tables,
        }
    }

    fn finish(self) -> Discovery {
        // Confirmed: what the ROM entry leads to through calls and exits.
        let mut confirmed: HashSet<Key> = HashSet::new();
        let mut work: VecDeque<Key> = self.entry.iter().map(|&a| key(a)).collect();
        while let Some(k) = work.pop_front() {
            if !self.roots.contains_key(&k) || !confirmed.insert(k) {
                continue;
            }
            let r = &self.eng.results[&k];
            if r.capped {
                continue;
            }
            work.extend(r.calls.iter().map(|&t| key(t)));
            work.extend(r.exits.iter().map(|&(_, t)| key(t)));
        }
        let ev = |k: &Key| {
            if confirmed.contains(k) {
                Evidence::Confirmed
            } else {
                Evidence::Scanned
            }
        };
        // A scanned root whose analysis hit the cap, or that reaches an
        // undefined instruction, has run into data.
        let dropped = |k: &Key| {
            ev(k) == Evidence::Scanned && (self.eng.results[k].capped || self.bad.contains(k))
        };
        let mut d = Discovery {
            rounds: self.rounds,
            rec: self
                .eng
                .collect(self.roots.keys().filter(|k| !dropped(k)).copied()),
            ..Discovery::default()
        };
        for (k, kind) in &self.roots {
            if dropped(k) {
                if self.eng.results[k].capped {
                    d.dropped += 1;
                } else {
                    d.undefined += 1;
                }
                continue;
            }
            let capped = self.eng.results[k].capped;
            let map = match kind {
                Kind::Func if !self.demoted.contains(k) => &mut d.funcs,
                Kind::Func => &mut d.entries,
                Kind::Entry => &mut d.entries,
            };
            map.insert(iw(*k), ev(k));
            if capped {
                continue;
            }
            d.copies.extend(&self.eng.results[k].copies);
            d.ram_entries.extend(&self.eng.results[k].ram_entries);
            for &(a, size) in &self.eng.results[k].insns {
                let c = Code {
                    size,
                    thumb: k.1,
                    evidence: ev(k),
                };
                d.code
                    .entry(a)
                    .and_modify(|o| o.evidence = o.evidence.min(c.evidence))
                    .or_insert(c);
            }
        }
        let mut data: Vec<(u32, u32)> = Vec::new();
        for &(a, b) in &self.data {
            match data.last_mut() {
                Some(l) if a <= l.1 => l.1 = l.1.max(b),
                _ => data.push((a, b)),
            }
        }
        d.data = data;
        d
    }
}
