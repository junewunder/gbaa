//! The jump-table check (decision 5c, `docs/research/jump-tables.md`):
//! ground truth from a linked pret ELF and its ROM, the code map the
//! analysis runs over, and a score of the recovered tables against the
//! truth.
//!
//! The truth comes from the ELF's symbols and the ROM's bytes, never from
//! instruction patterns ("Definitions" in the research doc):
//!
//! 1. A table is a maximal run of words, all inside one function, each the
//!    address of a `$t`/`$a` instruction in that same function. Inline
//!    tables lie in the function's `$d` regions. Out-of-line tables lie in a
//!    ROM data section, where every entry must be mid-function and the
//!    table's start must be held in the function's literal pool.
//! 2. A run is split wherever a data word of the function points into its
//!    middle.
//! 3. A recovered jump matches the truth table whose range contains the
//!    address its targets were read from: **exact** if the target sets are
//!    equal, **subset** if every target is right but some cases are left
//!    out, **wrong** if it has a target the table lacks.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use analysis::jumptable::Recovery;
use analysis::program::{Func, ROM_BASE, Region};
use analysis::vsa::func::JumpRes;

pub mod elf;

#[cfg(test)]
mod tests;

const ROM_END: u32 = 0x0a00_0000;

#[derive(Clone, Debug)]
pub struct Section {
    pub index: usize,
    pub addr: u32,
    pub size: u32,
    pub exec: bool,
    /// Allocated and with contents in the image (not `NOBITS`).
    pub data: bool,
}

#[derive(Clone, Debug)]
pub struct Sym {
    pub name: String,
    pub value: u32,
    pub size: u32,
    pub func: bool,
    pub global_notype: bool,
    pub section: usize,
}

/// What the truth rules read from an ELF. Symbols are in symbol-table
/// order (the first FUNC symbol at an address names it).
#[derive(Clone, Debug, Default)]
pub struct Elf {
    pub sections: Vec<Section>,
    pub syms: Vec<Sym>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Arm,
    Data,
    Thumb,
}

/// The ELF's code map, functions and jump tables.
#[derive(Debug, Default)]
pub struct Truth {
    /// `$a`/`$t` regions.
    pub code: Vec<Region>,
    /// Function extents, sorted by start.
    pub funcs: Vec<Func>,
    pub names: Vec<String>,
    pub tables: Vec<Table>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// Address of the first entry.
    pub addr: u32,
    /// The entries, in order.
    pub targets: Vec<u32>,
    /// The function's name.
    pub func: String,
}

impl Truth {
    pub fn build(elf: &Elf, rom: &[u8]) -> Truth {
        let sec = |i: usize| elf.sections.iter().find(|s| s.index == i);
        let exec = |i: usize| sec(i).is_some_and(|s| s.exec);
        // Mapping symbols, sorted as (address, kind, section).
        let mut maps: Vec<(u32, Kind, usize)> = Vec::new();
        let mut funcs: BTreeMap<u32, (u32, bool, String)> = BTreeMap::new();
        let mut notypes = Vec::new();
        for s in elf.syms.iter().filter(|s| exec(s.section)) {
            let kind = match s.name.as_str() {
                "$a" => Some(Kind::Arm),
                "$t" => Some(Kind::Thumb),
                "$d" => Some(Kind::Data),
                _ => None,
            };
            if let Some(k) = kind {
                maps.push((s.value, k, s.section));
            } else if s.func && (ROM_BASE..ROM_END).contains(&(s.value & !1)) {
                funcs
                    .entry(s.value & !1)
                    .or_insert((s.size, s.value & 1 != 0, s.name.clone()));
            } else if s.global_notype {
                notypes.push(s);
            }
        }
        maps.sort();
        // Each mapping symbol's region runs to the next one in the same
        // section, or to the section's end.
        let mut regions: Vec<(u32, u32, Kind)> = Vec::new();
        for (i, &(a, k, ndx)) in maps.iter().enumerate() {
            let s = sec(ndx).unwrap();
            let end = match maps.get(i + 1) {
                Some(&(b, _, n)) if n == ndx => b,
                _ => s.addr + s.size,
            };
            if end > a {
                regions.push((a, end, k));
            }
        }
        let starts: Vec<u32> = regions.iter().map(|r| r.0).collect();
        let start_set: HashSet<u32> = starts.iter().copied().collect();
        let kind_at = |a: u32| {
            let i = starts.partition_point(|&s| s <= a);
            let r = regions.get(i.checked_sub(1)?)?;
            (a < r.1).then_some(r.2)
        };
        for s in notypes {
            let k = kind_at(s.value);
            if matches!(k, Some(Kind::Arm | Kind::Thumb))
                && !funcs.contains_key(&(s.value & !1))
                && start_set.contains(&s.value)
            {
                funcs.insert(s.value, (0, k == Some(Kind::Thumb), s.name.clone()));
            }
        }
        let fstarts: Vec<u32> = funcs.keys().copied().collect();
        let ext: Vec<(u32, u32)> = fstarts
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let size = match funcs[&s].0 {
                    0 => fstarts.get(i + 1).map_or(4, |&n| n - s),
                    n => n,
                };
                (s, s + size)
            })
            .collect();
        let func_at = |a: u32| {
            let i = fstarts.partition_point(|&s| s <= a).checked_sub(1)?;
            (a < ext[i].1).then_some(i)
        };
        let word = |a: u32| {
            let o = a.checked_sub(ROM_BASE)? as usize;
            let b = rom.get(o..o + 4)?;
            Some(u32::from_le_bytes(b.try_into().unwrap()))
        };
        let is_code_addr = |w: u32| match kind_at(w) {
            Some(Kind::Arm) => w.is_multiple_of(4),
            Some(Kind::Thumb) => w.is_multiple_of(2),
            _ => false,
        };
        let words = |a: u32, e: u32| ((a + 3) & !3..e.saturating_sub(3)).step_by(4);

        // Every data word of each function, for the literal-pool tests.
        let data_regions: Vec<(u32, u32)> = regions
            .iter()
            .filter(|r| r.2 == Kind::Data && func_at(r.0).is_some())
            .map(|r| (r.0, r.1))
            .collect();
        let mut pool: HashMap<usize, HashSet<u32>> = HashMap::new();
        for &(a, e) in &data_regions {
            let p = pool.entry(func_at(a).unwrap()).or_default();
            p.extend(words(a, e).filter_map(word));
        }
        let in_pool = |fi: usize, a: u32| pool.get(&fi).is_some_and(|p| p.contains(&a));

        // Runs of same-function code addresses: out of line in ROM data
        // sections (entries mid-function, start in the pool), and inline in
        // the function's own data regions.
        let mut runs: Vec<(usize, Vec<(u32, u32)>)> = Vec::new();
        let rodata = elf
            .sections
            .iter()
            .filter(|s| s.data && !s.exec && (ROM_BASE..ROM_END).contains(&s.addr) && s.size > 0);
        for s in rodata {
            let mut run: Vec<(u32, u32)> = Vec::new();
            let mut rfi: Option<usize> = None;
            let mut flush = |run: &mut Vec<(u32, u32)>, rfi: Option<usize>| {
                if let (Some(fi), Some(first)) = (rfi, run.first())
                    && in_pool(fi, first.0)
                {
                    runs.push((fi, std::mem::take(run)));
                }
                run.clear();
            };
            for at in words(s.addr, s.addr + s.size) {
                let w = word(at);
                let fi = w.and_then(func_at);
                let ok = match (w, fi) {
                    (Some(w), Some(fi)) => is_code_addr(w) && w != fstarts[fi],
                    _ => false,
                };
                if ok && (rfi.is_none() || fi == rfi) {
                    run.push((at, w.unwrap()));
                    rfi = fi;
                    continue;
                }
                flush(&mut run, rfi);
                (run, rfi) = if ok {
                    (vec![(at, w.unwrap())], fi)
                } else {
                    (Vec::new(), None)
                };
            }
            flush(&mut run, rfi);
        }
        for &(a, e) in &data_regions {
            let fi = func_at(a).unwrap();
            let mut run = Vec::new();
            for at in words(a, e) {
                match word(at) {
                    Some(w) if func_at(w) == Some(fi) && is_code_addr(w) => run.push((at, w)),
                    _ if !run.is_empty() => runs.push((fi, std::mem::take(&mut run))),
                    _ => {}
                }
            }
            if !run.is_empty() {
                runs.push((fi, run));
            }
        }
        let mut tables = Vec::new();
        for (fi, run) in runs {
            let mut cur: Vec<(u32, u32)> = vec![run[0]];
            for &x in &run[1..] {
                if in_pool(fi, x.0) {
                    tables.push((fi, std::mem::take(&mut cur)));
                }
                cur.push(x);
            }
            tables.push((fi, cur));
        }

        let code = regions
            .iter()
            .filter(|r| r.2 != Kind::Data)
            .map(|r| Region {
                start: r.0,
                end: r.1,
                thumb: r.2 == Kind::Thumb,
            })
            .collect();
        let funcs_out = ext
            .iter()
            .map(|&(s, e)| Func {
                start: s,
                end: e,
                thumb: funcs[&s].1,
            })
            .collect();
        Truth {
            code,
            funcs: funcs_out,
            names: fstarts.iter().map(|s| funcs[s].2.clone()).collect(),
            tables: tables
                .into_iter()
                .map(|(fi, t)| Table {
                    addr: t[0].0,
                    targets: t.iter().map(|x| x.1).collect(),
                    func: funcs[&fstarts[fi]].2.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    Exact,
    Subset,
    Wrong,
}

/// The score of a [`Recovery`] against the truth tables.
#[derive(Debug, Default)]
pub struct Score {
    pub truth: usize,
    /// Resolved jumps per verdict, with (jump, table, function) for the
    /// non-exact ones.
    pub counts: BTreeMap<Verdict, usize>,
    pub notes: Vec<(Verdict, u32, u32, String)>,
    /// Truth tables no resolved jump matched: (table, entries, function).
    pub missed: Vec<(u32, usize, String)>,
    /// Resolved jumps read from ROM outside every truth table (byte or
    /// halfword tables, single literals), and resolved jumps not read
    /// from ROM.
    pub no_truth: usize,
    pub unresolved: usize,
    pub rejected: usize,
}

impl Score {
    pub fn get(&self, v: Verdict) -> usize {
        self.counts.get(&v).copied().unwrap_or(0)
    }

    pub fn new(truth: &Truth, rec: &Recovery) -> Score {
        // Truth tables by start; a later duplicate start replaces an
        // earlier one, as in the research scorer.
        let mut by_start: BTreeMap<u32, &Table> = BTreeMap::new();
        for t in &truth.tables {
            by_start.insert(t.addr, t);
        }
        let table_of = |a: u32| {
            let (_, t) = by_start.range(..=a).next_back()?;
            (a < t.addr + 4 * t.targets.len() as u32).then_some(*t)
        };
        let mut s = Score {
            truth: by_start.len(),
            ..Score::default()
        };
        let mut hit: BTreeSet<u32> = BTreeSet::new();
        for (at, res) in &rec.jumps {
            let j = match res {
                JumpRes::Resolved(j) => j,
                JumpRes::Unresolved => {
                    s.unresolved += 1;
                    continue;
                }
                JumpRes::Rejected(_) => {
                    s.rejected += 1;
                    continue;
                }
            };
            let Some(t) = j.table.and_then(|src| table_of(src.first)) else {
                s.no_truth += 1;
                continue;
            };
            hit.insert(t.addr);
            let want: BTreeSet<u32> = t.targets.iter().map(|x| x & !1).collect();
            let got: BTreeSet<u32> = j.targets.iter().map(|x| x & !1).collect();
            let v = if got == want {
                Verdict::Exact
            } else if got.is_subset(&want) {
                Verdict::Subset
            } else {
                Verdict::Wrong
            };
            *s.counts.entry(v).or_default() += 1;
            if v != Verdict::Exact {
                s.notes.push((v, *at, t.addr, t.func.clone()));
            }
        }
        s.missed = by_start
            .values()
            .filter(|t| !hit.contains(&t.addr))
            .map(|t| (t.addr, t.targets.len(), t.func.clone()))
            .collect();
        s
    }

    /// The report: one summary line, then up to `show` details.
    pub fn report(&self, show: usize) -> String {
        let mut out = String::new();
        writeln!(
            out,
            "exact {}/{}  subset {}  wrong {}  missed {}",
            self.get(Verdict::Exact),
            self.truth,
            self.get(Verdict::Subset),
            self.get(Verdict::Wrong),
            self.missed.len()
        )
        .unwrap();
        writeln!(
            out,
            "  also: {} resolved outside the truth tables, {} unresolved, {} rejected",
            self.no_truth, self.unresolved, self.rejected
        )
        .unwrap();
        for (v, at, t, f) in self.notes.iter().take(show) {
            writeln!(out, "  {v:?}: jump 0x{at:08x}, table 0x{t:08x} in {f}").unwrap();
        }
        for (t, n, f) in self.missed.iter().take(show) {
            writeln!(out, "  missed: table 0x{t:08x} ({n} entries) in {f}").unwrap();
        }
        out
    }
}

/// The far-jump truth, as the research measured it (`farjump.py` in
/// `docs/research/jump-tables.md`): a Thumb `bl` in the code map whose
/// target is at no symbol (it lands mid-function) is a far jump, unless the
/// target is a lone `bx rN` with rN not lr, which is GCC's local
/// call-via-register stub: an indirect call.
#[derive(Debug, Default)]
pub struct FarTruth {
    /// Every Thumb `bl` in the code map: (address, target).
    pub bls: Vec<(u32, u32)>,
    pub far: BTreeSet<u32>,
    /// `bl`s to a mid-function `bx rN` stub.
    pub stubs: usize,
}

impl FarTruth {
    pub fn build(elf: &Elf, prog: &analysis::program::Program) -> FarTruth {
        let exec: HashSet<usize> = elf
            .sections
            .iter()
            .filter(|s| s.exec)
            .map(|s| s.index)
            .collect();
        let at_sym: HashSet<u32> = elf
            .syms
            .iter()
            .filter(|s| exec.contains(&s.section) && !s.name.starts_with('$'))
            .map(|s| s.value & !1)
            .collect();
        let mut t = FarTruth::default();
        for r in prog.regions().iter().filter(|r| r.thumb) {
            let mut a = r.start;
            while a < r.end {
                let Some(i) = prog.decode(a, true) else { break };
                if let Some(to) = i.bl {
                    t.bls.push((a, to));
                    if !at_sym.contains(&to) {
                        let stub = matches!(
                            prog.decode(to, true).map(|i| i.instr),
                            Some(armv4t::decode::Instr::Bx { rm, cond: armv4t::decode::Cond::Al })
                                if rm != armv4t::decode::Reg::Lr
                        );
                        if stub {
                            t.stubs += 1;
                        } else {
                            t.far.insert(a);
                        }
                    }
                }
                a = i.next();
            }
        }
        t
    }

    /// (found, false, missed) for the `bl`s a rule calls far jumps.
    pub fn score(&self, found: &BTreeSet<u32>) -> (usize, usize, usize) {
        let hit = found.intersection(&self.far).count();
        (hit, found.len() - hit, self.far.len() - hit)
    }
}
