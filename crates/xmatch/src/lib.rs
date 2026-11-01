//! Recognising the same function across a compiler change
//! (`docs/research/cross-compiler-matching.md`): each function of one ROM
//! is matched to one of another's, or to none, from what should survive a
//! new compiler (hardware registers, BIOS calls, the data it points at,
//! constants, the shape of its loads and stores, its size), then spread
//! through the call graph and the RAM variables the matched pairs share.
//!
//! This part needs no ROM: [`Fun`] is a function's fingerprint, made from
//! codegen's IR by `main.rs`, and [`Matcher`] matches two sets of them.
//! Names never enter it; [`score`] compares its pairs with the truth after.

use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(test)]
mod tests;

/// One thing a function does that a compiler change should keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tok {
    /// A hardware address it names (I/O registers, palette, VRAM, OAM).
    Hw(u32),
    /// A load (`false`) or store of `width` bytes at a hardware address.
    HwAccess(bool, u8, u32),
    /// A BIOS call.
    Swi(u32),
    /// ROM data it points at, by a hash of its first bytes (pointers in it
    /// masked to their region), not its address: data moves when code does.
    Data(u64),
    /// A constant that isn't an address, 16 or more.
    Const(u32),
    /// How many functions it calls: 0, 1, 2-3, 4-7, 8+.
    Calls(u8),
    /// Loads (`false`) and stores of `width` bytes, by the region their
    /// address is based in, counted.
    Mem(bool, u8, Region),
    /// Calls through a register (function pointers), counted.
    Indirect,
    /// Spreading: a callee, caller or function pointer matched as pair N,
    /// and a RAM variable mapped as pair N.
    Callee(u32),
    Caller(u32),
    Ref(u32),
    Ram(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Region {
    Ewram,
    Iwram,
    Hw,
    Rom,
    Stack,
    Other,
}

impl Region {
    pub fn of(a: u32) -> Option<Region> {
        match a >> 24 {
            0x02 if a < 0x0204_0000 => Some(Region::Ewram),
            0x03 if a < 0x0300_8000 => Some(Region::Iwram),
            0x04..=0x07 => Some(Region::Hw),
            0x08 | 0x09 => Some(Region::Rom),
            _ => None,
        }
    }
}

/// A function's fingerprint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fun {
    pub key: u32,
    /// Guest instructions.
    pub insns: usize,
    /// Its instruction bytes hashed, call offsets and PC-relative offsets
    /// masked: equal for the same code from the same compiler.
    pub bytes: u64,
    /// Functions it calls or tail-calls, by key.
    pub calls: BTreeSet<u32>,
    /// Functions whose address it loads (callbacks, tasks).
    pub refs: BTreeSet<u32>,
    /// RAM addresses it loads from its literal pool.
    pub ram: BTreeSet<u32>,
    pub toks: BTreeMap<Tok, u32>,
}

/// One side's functions, with callers.
#[derive(Clone)]
pub struct Side {
    pub funs: Vec<Fun>,
    pub at: HashMap<u32, usize>,
    pub callers: Vec<Vec<usize>>,
    /// RAM address → functions loading it.
    pub ram_users: BTreeMap<u32, Vec<usize>>,
}

impl Side {
    pub fn new(funs: Vec<Fun>) -> Side {
        let at: HashMap<u32, usize> = funs.iter().enumerate().map(|(i, f)| (f.key, i)).collect();
        let mut callers = vec![Vec::new(); funs.len()];
        let mut ram_users: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (i, f) in funs.iter().enumerate() {
            for c in &f.calls {
                if let Some(&j) = at.get(c) {
                    callers[j].push(i);
                }
            }
            for &r in &f.ram {
                ram_users.entry(r).or_default().push(i);
            }
        }
        Side {
            funs,
            at,
            callers,
            ram_users,
        }
    }
}

/// How a pair was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// The same bytes, unique on both sides.
    Bytes,
    /// Fingerprints alone (round 0).
    Print,
    /// A later round, with matched neighbours and RAM variables as tokens.
    Spread,
    /// A later round, among one matched pair's unmatched neighbours.
    Local,
}

/// The matcher's settings: which stages run, and the acceptance bar.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub bytes: bool,
    pub spread: bool,
    /// The least similarity a pair needs.
    pub min_score: f64,
    /// How far above both sides' second-best candidates it must be.
    pub margin: f64,
    /// The bar and margin while only a function's own tokens count:
    /// round 0, and every round without spreading.
    pub first_min: f64,
    pub first_margin: f64,
    /// After the rounds, drop pairs more of whose matched neighbours
    /// disagree than agree ([`agreement`]), then run the rounds again.
    pub prune: bool,
    /// The least guest instructions a byte anchor needs.
    pub min_bytes_insns: usize,
    /// Rounds at most.
    pub rounds: usize,
    /// A token on more functions than this (both sides) doesn't propose
    /// candidates (it still scores them).
    pub df_cap: usize,
    /// With spreading, the bar for a pair among one matched pair's
    /// unmatched callees or callers, where there are few candidates
    /// (BinDiff's neighbourhood step); `None` skips the step.
    pub local_min: Option<f64>,
}

impl Config {
    /// For matching only a reference's libraries (`--library`): a game
    /// function calling the same helpers looks like one, so the spreading
    /// bar is higher and the neighbourhood step is off
    /// (docs/research/cross-compiler-matching.md, "Library names").
    pub fn library() -> Config {
        Config {
            min_score: 0.6,
            margin: 0.15,
            local_min: None,
            ..Config::default()
        }
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            bytes: true,
            spread: true,
            min_score: 0.5,
            margin: 0.1,
            first_min: 0.7,
            first_margin: 0.2,
            prune: true,
            min_bytes_insns: 6,
            rounds: 12,
            df_cap: 60,
            local_min: Some(0.5),
        }
    }
}

/// A matched pair: indices into side A and side B.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub a: usize,
    pub b: usize,
    pub score: f64,
    pub stage: Stage,
    pub round: usize,
}

pub struct Matcher<'s> {
    pub a: &'s Side,
    pub b: &'s Side,
    pub cfg: Config,
    pub pairs: Vec<Pair>,
    pa: Vec<Option<u32>>,
    pb: Vec<Option<u32>>,
    /// RAM address pairs mapped by spreading: A address → (B address, id).
    ram_a: BTreeMap<u32, u32>,
    ram_b: BTreeMap<u32, u32>,
}

/// Tokens with their weights, sorted by token.
type Vec_ = Vec<(Tok, f64)>;

impl<'s> Matcher<'s> {
    pub fn new(a: &'s Side, b: &'s Side, cfg: Config) -> Matcher<'s> {
        Matcher {
            a,
            b,
            cfg,
            pairs: Vec::new(),
            pa: vec![None; a.funs.len()],
            pb: vec![None; b.funs.len()],
            ram_a: BTreeMap::new(),
            ram_b: BTreeMap::new(),
        }
    }

    fn accept(&mut self, a: usize, b: usize, score: f64, stage: Stage, round: usize) {
        let id = self.pairs.len() as u32;
        self.pa[a] = Some(id);
        self.pb[b] = Some(id);
        self.pairs.push(Pair {
            a,
            b,
            score,
            stage,
            round,
        });
    }

    /// Runs every stage the config asks for.
    pub fn run(mut self) -> Vec<Pair> {
        if self.cfg.bytes {
            self.bytes();
        }
        self.rounds(0);
        if self.cfg.prune {
            let all = self.pairs.clone();
            let of_a: HashMap<usize, usize> = all.iter().map(|q| (q.a, q.b)).collect();
            let keep: Vec<Pair> = all
                .iter()
                .copied()
                .filter(|p| {
                    let (ag, dis) = agreement_in(self.a, self.b, &of_a, p);
                    dis <= ag
                })
                .collect();
            self.pairs.clear();
            self.pa.iter_mut().for_each(|x| *x = None);
            self.pb.iter_mut().for_each(|x| *x = None);
            for p in keep {
                self.accept(p.a, p.b, p.score, p.stage, p.round);
            }
            self.rounds(1);
        }
        self.pairs
    }

    fn rounds(&mut self, from: usize) {
        for round in from..from + self.cfg.rounds {
            if round > 0 && self.cfg.spread {
                self.map_ram();
            }
            let stage = if round > 0 && self.cfg.spread {
                Stage::Spread
            } else {
                Stage::Print
            };
            if self.round(round, stage) == 0 && round > 0 {
                break;
            }
        }
    }

    /// Byte-identical functions unique on both sides.
    fn bytes(&mut self) {
        let count = |s: &Side| {
            let mut m: HashMap<u64, Vec<usize>> = HashMap::new();
            for (i, f) in s.funs.iter().enumerate() {
                if f.insns >= self.cfg.min_bytes_insns {
                    m.entry(f.bytes).or_default().push(i);
                }
            }
            m
        };
        let (ma, mb) = (count(self.a), count(self.b));
        let mut found: Vec<(usize, usize)> = ma
            .iter()
            .filter_map(
                |(h, va)| match (va.as_slice(), mb.get(h).map(Vec::as_slice)) {
                    ([a], Some([b])) => Some((*a, *b)),
                    _ => None,
                },
            )
            .collect();
        found.sort_unstable();
        for (a, b) in found {
            self.accept(a, b, 1.0, Stage::Bytes, 0);
        }
    }

    /// The tokens of side `s`'s function `i` now: its own, and with
    /// spreading its matched neighbours' and mapped RAM variables.
    fn toks(&self, side_a: bool, i: usize) -> BTreeMap<Tok, u32> {
        let (s, pairs, ram) = if side_a {
            (self.a, &self.pa, &self.ram_a)
        } else {
            (self.b, &self.pb, &self.ram_b)
        };
        let f = &s.funs[i];
        let mut t = f.toks.clone();
        if !self.cfg.spread {
            return t;
        }
        let mut add = |k: Tok| *t.entry(k).or_insert(0) += 1;
        for c in &f.calls {
            if let Some(p) = s.at.get(c).and_then(|&j| pairs[j]) {
                add(Tok::Callee(p));
            }
        }
        for r in &f.refs {
            if let Some(p) = s.at.get(r).and_then(|&j| pairs[j]) {
                add(Tok::Ref(p));
            }
        }
        for &j in &s.callers[i] {
            if let Some(p) = pairs[j] {
                add(Tok::Caller(p));
            }
        }
        for r in &f.ram {
            if let Some(&id) = ram.get(r) {
                add(Tok::Ram(id));
            }
        }
        t
    }

    /// RAM addresses whose users are matched pairs: each A address to the
    /// B address most of its matched users' partners load, when that is
    /// mutual and at least two pairs agree.
    fn map_ram(&mut self) {
        let votes = |from: &Side, to: &Side, mine: &[Option<u32>], other: bool| {
            let mut best: BTreeMap<u32, (u32, usize, usize)> = BTreeMap::new();
            for (&r, users) in &from.ram_users {
                let mut v: BTreeMap<u32, usize> = BTreeMap::new();
                for &u in users {
                    let Some(p) = mine[u] else { continue };
                    let pr = self.pairs[p as usize];
                    let j = if other { pr.b } else { pr.a };
                    for &r2 in &to.funs[j].ram {
                        *v.entry(r2).or_insert(0) += 1;
                    }
                }
                let mut v: Vec<(usize, u32)> = v.into_iter().map(|(r2, n)| (n, r2)).collect();
                v.sort_unstable_by(|x, y| y.cmp(x));
                if let Some(&(n, r2)) = v.first() {
                    let second = v.get(1).map_or(0, |x| x.0);
                    best.insert(r, (r2, n, second));
                }
            }
            best
        };
        let ab = votes(self.a, self.b, &self.pa, true);
        let ba = votes(self.b, self.a, &self.pb, false);
        self.ram_a.clear();
        self.ram_b.clear();
        let mut id = 0;
        for (&ra, &(rb, n, second)) in &ab {
            if n >= 2
                && n > second
                && ba
                    .get(&rb)
                    .is_some_and(|&(back, m, s2)| back == ra && m > s2)
            {
                self.ram_a.insert(ra, id);
                self.ram_b.insert(rb, id);
                id += 1;
            }
        }
    }

    /// One round: every unmatched pair sharing a rare token is scored, and
    /// mutual best pairs over the bar, clear of the runners-up, are taken.
    /// Returns how many.
    fn round(&mut self, round: usize, stage: Stage) -> usize {
        // Without neighbours a pair has only its own tokens to go on.
        let (min_score, margin) = if stage == Stage::Print {
            (self.cfg.first_min, self.cfg.first_margin)
        } else {
            (self.cfg.min_score, self.cfg.margin)
        };
        let free_a: Vec<usize> = (0..self.a.funs.len())
            .filter(|&i| self.pa[i].is_none())
            .collect();
        let free_b: Vec<usize> = (0..self.b.funs.len())
            .filter(|&i| self.pb[i].is_none())
            .collect();
        let ta: Vec<BTreeMap<Tok, u32>> = free_a.iter().map(|&i| self.toks(true, i)).collect();
        let tb: Vec<BTreeMap<Tok, u32>> = free_b.iter().map(|&i| self.toks(false, i)).collect();
        // Document frequency over both sides' unmatched functions.
        let mut df: HashMap<Tok, usize> = HashMap::new();
        for t in ta.iter().chain(&tb) {
            for k in t.keys() {
                *df.entry(*k).or_insert(0) += 1;
            }
        }
        let n = (ta.len() + tb.len()).max(1) as f64;
        let idf = |k: &Tok| (n / df[k] as f64).ln();
        let weigh = |t: &BTreeMap<Tok, u32>| -> (Vec_, f64) {
            let v: Vec_ = t
                .iter()
                .map(|(k, &c)| (*k, (1.0 + (c as f64).ln()) * idf(k)))
                .filter(|(_, w)| *w > 0.0)
                .collect();
            let norm = v.iter().map(|(_, w)| w * w).sum::<f64>().sqrt();
            (v, norm)
        };
        let va: Vec<(Vec_, f64)> = ta.iter().map(weigh).collect();
        let vb: Vec<(Vec_, f64)> = tb.iter().map(weigh).collect();
        // Rare tokens propose candidates.
        let mut index: HashMap<Tok, Vec<usize>> = HashMap::new();
        for (x, t) in ta.iter().enumerate() {
            for k in t.keys() {
                if df[k] <= self.cfg.df_cap {
                    index.entry(*k).or_default().push(x);
                }
            }
        }
        let mut best_a: Vec<[(f64, usize); 2]> = vec![[(0.0, usize::MAX); 2]; ta.len()];
        let mut best_b: Vec<[(f64, usize); 2]> = vec![[(0.0, usize::MAX); 2]; tb.len()];
        let push = |b: &mut [(f64, usize); 2], s: f64, i: usize| {
            if s > b[0].0 {
                b[1] = b[0];
                b[0] = (s, i);
            } else if s > b[1].0 {
                b[1] = (s, i);
            }
        };
        for (y, t) in tb.iter().enumerate() {
            let mut cands: BTreeSet<usize> = BTreeSet::new();
            for k in t.keys() {
                if let Some(xs) = index.get(k) {
                    cands.extend(xs);
                }
            }
            for x in cands {
                let s = similarity(
                    &va[x],
                    &vb[y],
                    self.a.funs[free_a[x]].insns,
                    self.b.funs[free_b[y]].insns,
                );
                push(&mut best_b[y], s, x);
                push(&mut best_a[x], s, y);
            }
        }
        let mut took = Vec::new();
        for (y, bb) in best_b.iter().enumerate() {
            let (s, x) = bb[0];
            if x == usize::MAX || s < min_score {
                continue;
            }
            let ba = &best_a[x];
            if ba[0].1 != y {
                continue;
            }
            if s - bb[1].0 < margin || s - ba[1].0 < margin {
                continue;
            }
            took.push((free_a[x], free_b[y], s));
        }
        for &(a, b, s) in &took {
            self.accept(a, b, s, stage, round);
        }
        let mut n = took.len();
        if let (Some(min), Stage::Spread) = (self.cfg.local_min, stage) {
            let xa: HashMap<usize, usize> =
                free_a.iter().enumerate().map(|(x, &i)| (i, x)).collect();
            let xb: HashMap<usize, usize> =
                free_b.iter().enumerate().map(|(y, &i)| (i, y)).collect();
            let mut local = Vec::new();
            for p in self.pairs.clone() {
                let near =
                    |s: &Side, i: usize, pairs: &[Option<u32>], x: &HashMap<usize, usize>| {
                        let f = &s.funs[i];
                        let mut out: BTreeSet<usize> = f
                            .calls
                            .iter()
                            .filter_map(|c| s.at.get(c).copied())
                            .collect();
                        out.extend(&s.callers[i]);
                        out.into_iter()
                            .filter(|&j| pairs[j].is_none())
                            .filter_map(|j| x.get(&j).copied())
                            .collect::<Vec<usize>>()
                    };
                let na = near(self.a, p.a, &self.pa, &xa);
                let nb = near(self.b, p.b, &self.pb, &xb);
                if na.is_empty() || nb.is_empty() {
                    continue;
                }
                let mut ba: Vec<[(f64, usize); 2]> = vec![[(0.0, usize::MAX); 2]; na.len()];
                let mut bb: Vec<[(f64, usize); 2]> = vec![[(0.0, usize::MAX); 2]; nb.len()];
                for (i, &x) in na.iter().enumerate() {
                    for (j, &y) in nb.iter().enumerate() {
                        let s = similarity(
                            &va[x],
                            &vb[y],
                            self.a.funs[free_a[x]].insns,
                            self.b.funs[free_b[y]].insns,
                        );
                        push(&mut ba[i], s, j);
                        push(&mut bb[j], s, i);
                    }
                }
                for (j, b2) in bb.iter().enumerate() {
                    let (s, i) = b2[0];
                    if i == usize::MAX || s < min || ba[i][0].1 != j {
                        continue;
                    }
                    if s - b2[1].0 < self.cfg.margin || s - ba[i][1].0 < self.cfg.margin {
                        continue;
                    }
                    local.push((free_a[na[i]], free_b[nb[j]], s));
                }
            }
            local.sort_by(|x, y| y.2.total_cmp(&x.2));
            for (a, b, s) in local {
                if self.pa[a].is_none() && self.pb[b].is_none() {
                    self.accept(a, b, s, Stage::Local, round);
                    n += 1;
                }
            }
        }
        n
    }
}

/// For pair `p` among `pairs`: how many of its A function's matched
/// callees and callers are matched to the B function's (agree), and how
/// many to something else (disagree).
pub fn agreement(a: &Side, b: &Side, pairs: &[Pair], p: &Pair) -> (usize, usize) {
    let of_a: HashMap<usize, usize> = pairs.iter().map(|q| (q.a, q.b)).collect();
    agreement_in(a, b, &of_a, p)
}

/// [`agreement`], with the pairs as a map from A index to B index.
pub fn agreement_in(a: &Side, b: &Side, of_a: &HashMap<usize, usize>, p: &Pair) -> (usize, usize) {
    let near = |s: &Side, i: usize| -> BTreeSet<usize> {
        let mut out: BTreeSet<usize> = s.funs[i]
            .calls
            .iter()
            .filter_map(|c| s.at.get(c).copied())
            .filter(|&j| j != i)
            .collect();
        out.extend(s.callers[i].iter().filter(|&&j| j != i));
        out
    };
    let nb = near(b, p.b);
    let (mut agree, mut disagree) = (0, 0);
    for x in near(a, p.a) {
        if let Some(y) = of_a.get(&x) {
            if nb.contains(y) {
                agree += 1;
            } else {
                disagree += 1;
            }
        }
    }
    (agree, disagree)
}

/// Cosine of two weighted token vectors, times how close the sizes are.
pub fn similarity(a: &(Vec_, f64), b: &(Vec_, f64), na: usize, nb: usize) -> f64 {
    if a.1 == 0.0 || b.1 == 0.0 {
        return 0.0;
    }
    let (mut i, mut j, mut dot) = (0, 0, 0.0);
    while i < a.0.len() && j < b.0.len() {
        match a.0[i].0.cmp(&b.0[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                dot += a.0[i].1 * b.0[j].1;
                i += 1;
                j += 1;
            }
        }
    }
    let (lo, hi) = (na.min(nb).max(1) as f64, na.max(nb).max(1) as f64);
    dot / (a.1 * b.1) * (lo / hi).powf(0.25)
}

/// For each of B's functions `ys`, A's `n` functions most like it by
/// their own tokens (round 0's cosine, weighted over every function of
/// both sides, nothing matched), best first: the candidates a judge is
/// shown (`xmatch judge`).
pub fn nearest(a: &Side, b: &Side, ys: &[usize], n: usize) -> Vec<Vec<(usize, f64)>> {
    let mut df: HashMap<Tok, usize> = HashMap::new();
    for f in a.funs.iter().chain(&b.funs) {
        for k in f.toks.keys() {
            *df.entry(*k).or_insert(0) += 1;
        }
    }
    let total = (a.funs.len() + b.funs.len()).max(1) as f64;
    let weigh = |t: &BTreeMap<Tok, u32>| -> (Vec_, f64) {
        let v: Vec_ = t
            .iter()
            .map(|(k, &c)| (*k, (1.0 + (c as f64).ln()) * (total / df[k] as f64).ln()))
            .filter(|(_, w)| *w > 0.0)
            .collect();
        let norm = v.iter().map(|(_, w)| w * w).sum::<f64>().sqrt();
        (v, norm)
    };
    let va: Vec<(Vec_, f64)> = a.funs.iter().map(|f| weigh(&f.toks)).collect();
    ys.iter()
        .map(|&y| {
            let vb = weigh(&b.funs[y].toks);
            let mut s: Vec<(usize, f64)> = va
                .iter()
                .enumerate()
                .map(|(x, v)| (x, similarity(v, &vb, a.funs[x].insns, b.funs[y].insns)))
                .filter(|&(_, s)| s > 0.0)
                .collect();
            s.sort_by(|p, q| q.1.total_cmp(&p.1).then(p.0.cmp(&q.0)));
            s.truncate(n);
            s
        })
        .collect()
}

/// A function's size class, by guest instructions (the naming sample's).
pub fn class(insns: usize) -> usize {
    match insns {
        0..16 => 0,
        16..=100 => 1,
        _ => 2,
    }
}

pub const CLASSES: [&str; 3] = ["small (<16)", "mid (16-100)", "large (>100)"];

/// What a pair is, against the truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// B's counterpart in A.
    Right,
    /// Part of it: a piece the new compiler split out (`Foo.part.0`
    /// for `Foo`), or the body inlined into it.
    Partial,
    /// Neither side has a name (discovery took the same data for code).
    Unnamed,
    Wrong,
}

/// Pairs against the truth, by size class of the B function.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    /// Per class: B functions with a counterpart in A, pairs made, and of
    /// those, right, partial and unnamed.
    pub matchable: [usize; 3],
    pub made: [usize; 3],
    pub right: [usize; 3],
    pub partial: [usize; 3],
    pub unnamed: [usize; 3],
    /// Right pairs whose sizes differ by half or more (inlining).
    pub resized: usize,
}

impl Score {
    pub fn total(x: &[usize; 3]) -> usize {
        x.iter().sum()
    }
    /// Pairs judged: the unnamed are neither right nor wrong.
    pub fn judged(&self, c: usize) -> usize {
        self.made[c] - self.unnamed[c]
    }
    pub fn judged_all(&self) -> usize {
        (0..3).map(|c| self.judged(c)).sum()
    }
    /// Right over judged, a partial pair counted wrong.
    pub fn precision(&self) -> f64 {
        Self::total(&self.right) as f64 / self.judged_all().max(1) as f64
    }
    /// Right or partial over judged.
    pub fn lenient(&self) -> f64 {
        (Self::total(&self.right) + Self::total(&self.partial)) as f64
            / self.judged_all().max(1) as f64
    }
    pub fn recall(&self) -> f64 {
        Self::total(&self.right) as f64 / Self::total(&self.matchable).max(1) as f64
    }
}

/// Scores `pairs`: `truth` gives each B index with a counterpart its A
/// index, `judge` says what a pair is.
pub fn score(
    a: &Side,
    b: &Side,
    pairs: &[Pair],
    truth: &BTreeMap<usize, usize>,
    judge: impl Fn(usize, usize) -> Verdict,
) -> Score {
    let mut s = Score::default();
    for &y in truth.keys() {
        s.matchable[class(b.funs[y].insns)] += 1;
    }
    for p in pairs {
        let c = class(b.funs[p.b].insns);
        s.made[c] += 1;
        match judge(p.a, p.b) {
            Verdict::Right => {
                s.right[c] += 1;
                let (na, nb) = (a.funs[p.a].insns as f64, b.funs[p.b].insns as f64);
                if na.max(nb) >= 1.5 * na.min(nb) {
                    s.resized += 1;
                }
            }
            Verdict::Partial => s.partial[c] += 1,
            Verdict::Unnamed => s.unnamed[c] += 1,
            Verdict::Wrong => {}
        }
    }
    s
}
