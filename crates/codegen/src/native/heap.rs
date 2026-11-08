//! Heap pointers as typed references (`native/ref-heap`;
//! `docs/native-stage-b.md`, piece 5; `docs/native-port-design.md`,
//! section 1, "Pointers into state", form 3).
//!
//! A load or store whose address the value sets don't bound (a pointer
//! the allocator returned, or one loaded from where it was stored) prints
//! as a field of a `Ref<T>` (`g.at(r).f_8`, `g.at_mut(r).f_8 = …`), `T` a
//! struct whose fields are the offsets and widths seen through every
//! pointer of its kind. A pointer the types don't explain stays bytes.
//!
//! **What it follows.** A whole-program abstract run of every function's
//! IR, like [`super::fnref`]'s but of pointers: a value is a set of tokens
//! ([`T`]): a number, a RAM address (a constant in EWRAM or IWRAM), a
//! pointer `off` bytes into an object, and "unknown". An object is made
//! where a pointer is added to an amount the run doesn't know (its site:
//! the instruction's address): that is what an allocator does to its
//! heap's cursor (agb's bump allocator, inlined where `Box::new` is, adds
//! the alignment's padding to the cursor and returns it), and what an
//! index does to an array's base (the element is then the object).
//! Pointer plus a constant moves within the object. Null plus an unknown
//! is nothing (a null check's other path).
//!
//! Memory: RAM words at constant addresses (one cell each, starting as
//! the program start's copies or 0), the objects' word fields by (site,
//! offset), and each function's own frame words that don't escape
//! ([`super::frame`]). A store through an address the run can't place is
//! not followed, so the run isn't sound for memory: the types are checked
//! at run time instead. Interprocedural: a call passes r0-r3 to its
//! callee's entry (a tail call or a fall every register) and takes back
//! r0-r3; an indirect call reaches the callee set [`super::fnref`]
//! found. If any indirect call's set isn't complete, every function
//! starts from unknown registers (memory still flows). Rounds until
//! nothing changes ([`ROUNDS`] at most: past that, nothing is typed).
//!
//! **Types.** An access is typed when every token of its address is a
//! pointer into an object at one offset (nulls left out): the objects'
//! sites are one type (union-find over the sites that meet in an
//! access), the field its offset and width. Fields of one type that
//! overlap at different widths or offsets leave those accesses as bytes,
//! counted (`ref_unexplained`).
//!
//! **Checked at run time.** The printed field reads the same bytes the
//! byte access did (`Ref::to(addr, off)` is the address less the field's
//! offset), so a wrong type changes nothing; the prelude's `Game::at`
//! stops the program, naming the pointer, if it isn't in RAM, if the
//! object runs past its region (or, at the packed layout, isn't
//! contiguous in `Ram`), or isn't aligned in the guest as `T` is.
//!
//! `GBAA_HEAP_TRACE=1` prints what changed each round, then the cells,
//! fields and every access through a pointer; `GBAA_HEAP_CELL=<hex>` each
//! store to that RAM word, with where.

use super::fnref::{FnIn, Site};
use super::frame::{Edge, Place};
use analysis::ir::{BinOp, Op};
use armv4t::sem::{ShiftType, Width};
use std::collections::{BTreeMap, BTreeSet};

/// The most rounds the run takes before giving up (nothing typed).
pub const ROUNDS: usize = 60;

/// A value's token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum T {
    Num(u32),
    /// A RAM address (canonical: no mirrors).
    Ram(u32),
    /// `off` bytes into an object made at a site.
    Obj(u32, i32),
    /// Somewhere in an object made at a site.
    Any(u32),
    /// Somewhere in RAM (too many RAM addresses to keep).
    RamAny,
    /// `!` of a pointer (one side of a difference or compare).
    Neg,
    /// A number made of a constant outside RAM and an unknown amount (an
    /// I/O register's, VRAM's, a ROM table's address, indexed): never a
    /// RAM object's pointer.
    Other,
}

impl T {
    fn ptr(self) -> bool {
        matches!(self, T::Ram(_) | T::Obj(..) | T::Any(_) | T::RamAny)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct V {
    t: BTreeSet<T>,
    unk: bool,
}

/// Most numbers a value keeps; past it, they go to unknown.
const NUMS: usize = 4;
/// Most offsets into one object (or RAM addresses) a value keeps; past
/// it, [`T::Any`] (unknown).
const OFFS: usize = 8;

impl V {
    fn unk() -> V {
        V {
            t: BTreeSet::new(),
            unk: true,
        }
    }

    fn of(t: T) -> V {
        V {
            t: BTreeSet::from([t]),
            unk: false,
        }
    }

    fn join(&mut self, o: &V) -> bool {
        let n = self.t.len();
        let u = self.unk;
        self.t.extend(o.t.iter().copied());
        self.unk |= o.unk;
        self.cap();
        self.t.len() != n || self.unk != u
    }

    fn cap(&mut self) {
        let mut per: BTreeMap<u32, usize> = BTreeMap::new();
        let (mut nums, mut rams) = (0, 0);
        for t in &self.t {
            match t {
                T::Obj(s, _) => *per.entry(*s).or_default() += 1,
                T::Num(_) => nums += 1,
                T::Ram(_) => rams += 1,
                _ => {}
            }
        }
        for (s, n) in per {
            if n > OFFS || self.t.contains(&T::Any(s)) {
                self.t.retain(|t| !matches!(t, T::Obj(x, _) if *x == s));
                self.t.insert(T::Any(s));
            }
        }
        // Unknown is any number already (and keeping one would let a
        // value shrink and grow again, never settling).
        if nums > NUMS || (self.unk && nums > 0) {
            self.t.retain(|t| !matches!(t, T::Num(_)));
            self.unk = true;
        }
        // Kept once made, so the value never shrinks and grows again.
        if rams > OFFS || (rams > 0 && self.t.contains(&T::RamAny)) {
            self.t.retain(|t| !matches!(t, T::Ram(_)));
            self.t.insert(T::RamAny);
        }
    }

    fn num(&self) -> Option<u32> {
        match (self.unk, self.t.len(), self.t.iter().next()) {
            (false, 1, Some(T::Num(n))) => Some(*n),
            _ => None,
        }
    }
}

fn canon(c: u32) -> Option<u32> {
    match c >> 24 {
        2 if c < 0x0204_0000 => Some(c),
        3 if c < 0x0300_8000 => Some(c),
        _ => None,
    }
}

fn tok(c: u32) -> T {
    match canon(c) {
        Some(a) => T::Ram(a),
        None => T::Num(c),
    }
}

/// Numbers small enough to be a null pointer plus a field's offset: left
/// out of an address (the path that dereferences one never runs).
fn nullish(t: T) -> bool {
    matches!(t, T::Num(n) if n < 0x100)
}

fn add(a: &V, b: &V, site: u32) -> V {
    let mut out = V::default();
    if a.t.len() * b.t.len() > 64 {
        return V::unk();
    }
    out.unk = a.unk && b.unk;
    let fresh = |x: &V, y: &V, out: &mut V| {
        if !y.unk {
            return;
        }
        for &t in &x.t {
            match t {
                _ if t.ptr() => {
                    out.t.insert(T::Obj(site, 0));
                }
                T::Num(0) => {}
                T::Num(c) if !nullish(T::Num(c)) => {
                    out.t.insert(T::Other);
                }
                T::Other => {
                    out.t.insert(T::Other);
                }
                _ => out.unk = true,
            }
        }
    };
    fresh(a, b, &mut out);
    fresh(b, a, &mut out);
    for &x in &a.t {
        for &y in &b.t {
            let r = match (x, y) {
                (T::Num(p), T::Num(q)) => Some(tok(p.wrapping_add(q))),
                (T::Ram(c), T::Num(k)) | (T::Num(k), T::Ram(c)) => Some(tok(c.wrapping_add(k))),
                (T::Obj(s, o), T::Num(k)) | (T::Num(k), T::Obj(s, o)) => {
                    let o = o.wrapping_add(k as i32);
                    (o.unsigned_abs() < 0x4000).then_some(T::Obj(s, o))
                }
                (T::Any(s), T::Num(_)) | (T::Num(_), T::Any(s)) => Some(T::Any(s)),
                (T::RamAny, T::Num(_)) | (T::Num(_), T::RamAny) => Some(T::RamAny),
                (T::Neg, T::Num(_)) | (T::Num(_), T::Neg) => Some(T::Neg),
                (T::Other, T::Num(_)) | (T::Num(_), T::Other) => Some(T::Other),
                _ => None,
            };
            match r {
                Some(t) => {
                    out.t.insert(t);
                }
                None => out.unk = true,
            }
        }
    }
    out.cap();
    out
}

fn not(a: &V) -> V {
    let mut out = V {
        unk: a.unk,
        ..V::default()
    };
    for &t in &a.t {
        match t {
            T::Num(c) => {
                out.t.insert(T::Num(!c));
            }
            T::Neg | T::Other => out.unk = true,
            _ => {
                out.t.insert(T::Neg);
            }
        }
    }
    out
}

fn neg(a: &V) -> V {
    let mut out = not(a);
    out.t = out
        .t
        .into_iter()
        .map(|t| match t {
            T::Num(c) => T::Num(c.wrapping_add(1)),
            t => t,
        })
        .collect();
    out
}

fn and(a: &V, b: &V) -> V {
    if let (Some(x), Some(y)) = (a.num(), b.num()) {
        return V::of(T::Num(x & y));
    }
    let (p, m) = match (a.num(), b.num()) {
        (_, Some(m)) => (a, m),
        (Some(m), _) => (b, m),
        _ => return V::unk(),
    };
    let mut out = V {
        unk: p.unk,
        ..V::default()
    };
    for &t in &p.t {
        let r = match t {
            T::Num(c) => Some(T::Num(c & m)),
            T::Ram(c) if m >= 0xFFFF_FF00 => Some(tok(c & m)),
            T::Ram(c) if m <= 3 => Some(T::Num(c & m)),
            T::Obj(s, o) if m >= 0xFFFF_FF00 => Some(T::Obj(s, o & m as i32)),
            T::Other if m >= 0xFFFF_FF00 => Some(T::Other),
            // A word load's rotation: the object is aligned (checked
            // where a field is read).
            T::Obj(_, o) if m <= 3 => Some(T::Num(o as u32 & m)),
            _ => None,
        };
        match r {
            Some(t) => {
                out.t.insert(t);
            }
            None => out.unk = true,
        }
    }
    out
}

fn num2(a: &V, b: &V, f: impl Fn(u32, u32) -> u32) -> V {
    match (a.num(), b.num()) {
        (Some(x), Some(y)) => V::of(tok(f(x, y))),
        _ => V::unk(),
    }
}

fn shift(kind: ShiftType, x: u32, n: u32) -> u32 {
    match kind {
        ShiftType::Lsl => x.checked_shl(n).unwrap_or(0),
        ShiftType::Lsr => x.checked_shr(n).unwrap_or(0),
        ShiftType::Asr => ((x as i32) >> n.min(31)) as u32,
        ShiftType::Ror => x.rotate_right(n & 31),
    }
}

fn bytes(w: Width) -> u8 {
    match w {
        Width::Byte => 1,
        Width::Half => 2,
        Width::Word => 4,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct St {
    regs: Vec<V>,
    /// The frame's words that don't escape, by offset from the entry sp.
    frame: BTreeMap<i32, (V, u8)>,
}

impl St {
    fn join(&mut self, o: &St) -> bool {
        let mut ch = false;
        for (a, b) in self.regs.iter_mut().zip(&o.regs) {
            ch |= a.join(b);
        }
        let n = self.frame.len();
        self.frame
            .retain(|k, (_, w)| matches!(o.frame.get(k), Some((_, ow)) if ow == w));
        ch |= self.frame.len() != n;
        for (k, (v, _)) in self.frame.iter_mut() {
            ch |= v.join(&o.frame[k].0);
        }
        ch
    }
}

/// Each access's address and width, by (function, block, inst index).
type Record = BTreeMap<(u32, u32, usize), (V, u8)>;

#[derive(Default)]
struct G {
    entry: BTreeMap<u32, Vec<V>>,
    ret: BTreeMap<u32, Vec<V>>,
    cells: BTreeMap<u32, V>,
    fields: BTreeMap<(u32, i32), V>,
    any: BTreeMap<u32, V>,
    changed: bool,
    /// What changed this round, for `GBAA_HEAP_TRACE`.
    why: Vec<String>,
    /// The last pass's addresses, by (function, block, inst index).
    record: Option<Record>,
}

impl G {
    fn give(&mut self, k: u32, regs: &[V], n: usize) {
        let e = self
            .entry
            .entry(k)
            .or_insert_with(|| vec![V::default(); 16]);
        let mut ch = false;
        for (r, v) in regs.iter().enumerate().take(16) {
            ch |= e[r].join(if r < n { v } else { &UNK });
        }
        for x in e.iter_mut().skip(regs.len()) {
            ch |= x.join(&UNK);
        }
        if ch && self.why.len() < 8 {
            self.why.push(format!("entry {k:#x}: {:?}", &e[..4]));
        }
        self.changed |= ch;
    }

    fn give_ret(&mut self, k: u32, regs: &[V]) {
        let e = self.ret.entry(k).or_insert_with(|| vec![V::default(); 4]);
        let mut ch = false;
        for (a, b) in e.iter_mut().zip(regs) {
            ch |= a.join(b);
        }
        if ch && self.why.len() < 8 {
            self.why.push(format!("ret {k:#x}: {e:?}"));
        }
        self.changed |= ch;
    }

    fn ret_of(&self, k: u32) -> Vec<V> {
        self.ret
            .get(&k)
            .cloned()
            .unwrap_or_else(|| vec![V::default(); 4])
    }
}

static UNK: V = V {
    t: BTreeSet::new(),
    unk: true,
};

struct Run<'a> {
    f: &'a FnIn<'a>,
    fns: &'a BTreeSet<u32>,
    sites: &'a BTreeMap<u32, Site>,
    init: &'a dyn Fn(u32) -> u32,
    g: &'a mut G,
    /// `GBAA_HEAP_CELL`: the RAM word whose stores to trace.
    trace_cell: Option<u32>,
}

impl Run<'_> {
    fn cell(&mut self, c: u32) -> V {
        let init = self.init;
        self.g
            .cells
            .entry(c)
            .or_insert_with(|| V::of(tok(init(c))))
            .clone()
    }

    fn store_mem(&mut self, a: &V, w: u8, x: &V, at: u32) {
        let x = if w == 4 { x.clone() } else { V::unk() };
        if let Some(c) = self.trace_cell
            && a.t.iter().any(|t| matches!(t, T::Ram(r) if *r & !3 == c))
        {
            eprintln!(
                "heap: {at:#010x} in {:#010x} stores {x:?} to {c:#x}",
                self.f.entry
            );
        }
        for &t in &a.t {
            let ch = match t {
                T::Ram(c) => {
                    let c = c & !3;
                    let mut v = self.cell(c);
                    let ch = v.join(&x);
                    self.g.cells.insert(c, v);
                    ch
                }
                T::Obj(s, o) => self.g.fields.entry((s, o & !3)).or_default().join(&x),
                T::Any(s) => self.g.any.entry(s).or_default().join(&x),
                _ => false,
            };
            if ch && self.g.why.len() < 8 {
                self.g.why.push(format!("store {t:?} {x:?}"));
            }
            self.g.changed |= ch;
        }
    }

    fn load_mem(&mut self, a: &V, w: u8) -> V {
        if a.unk || w != 4 {
            return V::unk();
        }
        let mut out = V::default();
        for &t in &a.t {
            match t {
                T::Ram(c) => {
                    let v = self.cell(c & !3);
                    out.join(&v);
                }
                T::Obj(s, o) => {
                    if let Some(v) = self.g.fields.get(&(s, o & !3)) {
                        out.join(v);
                    }
                    if let Some(v) = self.g.any.get(&s) {
                        out.join(v);
                    }
                }
                T::Any(_) | T::RamAny | T::Neg | T::Other => out.unk = true,
                T::Num(_) if nullish(t) => {}
                T::Num(_) => out.unk = true,
            }
        }
        out
    }

    fn function(&mut self) {
        let f = self.f;
        let entry = self
            .g
            .entry
            .get(&f.entry)
            .cloned()
            .unwrap_or_else(|| vec![V::default(); 16]);
        let mut ins: BTreeMap<u32, St> = BTreeMap::new();
        let Some(&first) = f
            .blocks
            .keys()
            .find(|&&k| k == f.entry)
            .or(f.blocks.keys().next())
        else {
            return;
        };
        ins.insert(
            first,
            St {
                regs: entry,
                frame: BTreeMap::new(),
            },
        );
        let mut work: BTreeSet<u32> = BTreeSet::from([first]);
        let mut steps = 0;
        while let Some(k) = work.pop_first() {
            steps += 1;
            if steps > 64 * f.blocks.len() + 64 {
                break;
            }
            for (to, s) in self.block(k, ins[&k].clone()) {
                if !f.blocks.contains_key(&to) {
                    continue;
                }
                match ins.get_mut(&to) {
                    Some(old) => {
                        if old.join(&s) {
                            work.insert(to);
                        }
                    }
                    None => {
                        ins.insert(to, s);
                        work.insert(to);
                    }
                }
            }
        }
    }

    fn callees(&self, at: u32) -> Option<&BTreeSet<u32>> {
        self.sites
            .get(&at)
            .filter(|s| s.complete)
            .map(|s| &s.callees)
    }

    /// Run block `k` from `s`: the states it passes to the function's
    /// blocks.
    fn block(&mut self, k: u32, mut s: St) -> Vec<(u32, St)> {
        let f = self.f;
        let b = &f.blocks[&k];
        let mut at = k & !1;
        let mut vals: Vec<V> = vec![V::default(); b.insts.len()];
        let mut next = Vec::new();
        for (i, inst) in b.insts.iter().enumerate() {
            let val = |x: analysis::ir::ValueId| vals[x.index()].clone();
            let guarded = inst.guard.is_some();
            let v = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    V::default()
                }
                Op::Const(c) | Op::Pool(c) => V::of(tok(c)),
                Op::Bit(x) => V::of(T::Num(x as u32)),
                Op::GetReg(r) if r.index() == 13 => V::unk(),
                Op::GetReg(r) => s.regs[r.index()].clone(),
                Op::Bin(op, x, y) => {
                    let (x, y) = (val(x), val(y));
                    match op {
                        BinOp::Add => add(&x, &y, at),
                        BinOp::Sub => add(&x, &neg(&y), at),
                        BinOp::And => and(&x, &y),
                        BinOp::Or => num2(&x, &y, |p, q| p | q),
                        BinOp::Xor => num2(&x, &y, |p, q| p ^ q),
                        BinOp::Mul => num2(&x, &y, |p, q| p.wrapping_mul(q)),
                    }
                }
                Op::Adc { a, b, c } => {
                    let ab = add(&val(a), &val(b), at);
                    add(&ab, &val(c), at)
                }
                Op::Not(a) => not(&val(a)),
                Op::Shift { kind, v, amount } => {
                    let x = val(v);
                    match val(amount).num() {
                        Some(0) => x,
                        Some(n) => match x.num() {
                            Some(c) => V::of(tok(shift(kind, c, n))),
                            None => V::unk(),
                        },
                        None => V::unk(),
                    }
                }
                Op::Select { a, b, .. } => {
                    let mut x = val(a);
                    x.join(&val(b));
                    x
                }
                Op::SetReg(r, x) => {
                    let x = val(x);
                    if guarded {
                        s.regs[r.index()].join(&x);
                    } else {
                        s.regs[r.index()] = x;
                    }
                    V::default()
                }
                Op::Load { width, addr } => {
                    let w = bytes(width);
                    let a = val(addr);
                    match self.frame_place(k, i) {
                        Some(Some(o)) => match s.frame.get(&o) {
                            Some((v, sw)) if *sw == w => v.clone(),
                            _ => V::unk(),
                        },
                        Some(None) => V::unk(),
                        None => {
                            if let Some(r) = &mut self.g.record {
                                r.insert((f.entry, k, i), (a.clone(), w));
                            }
                            self.load_mem(&a, w)
                        }
                    }
                }
                Op::Store { width, addr, value } => {
                    let w = bytes(width);
                    let (a, x) = (val(addr), val(value));
                    match self.frame_place(k, i) {
                        Some(place) => {
                            if let Some(Place::Exact(o, n)) =
                                f.frame.and_then(|fr| fr.places.get(&(k, i)))
                            {
                                let (o, n) = (*o, *n as i32);
                                let old = s.frame.get(&o).cloned();
                                s.frame
                                    .retain(|&q, (_, qw)| q + *qw as i32 <= o || o + n <= q);
                                if let Some(o) = place {
                                    let x = if w == 4 { x } else { V::unk() };
                                    let new = match (guarded, old) {
                                        (false, _) => Some((x, w)),
                                        (true, Some((mut ov, ow))) if ow == w => {
                                            ov.join(&x);
                                            Some((ov, w))
                                        }
                                        _ => None,
                                    };
                                    s.frame.extend(new.map(|v| (o, v)));
                                }
                            }
                        }
                        None => {
                            if let Some(r) = &mut self.g.record {
                                r.insert((f.entry, k, i), (a.clone(), w));
                            }
                            self.store_mem(&a, w, &x, at);
                        }
                    }
                    V::default()
                }
                Op::Exit(_) => {
                    let Some(out) = b.exits.get(&i) else {
                        continue;
                    };
                    self.exit(&out.edge, out.at, &s, &mut next);
                    V::default()
                }
                _ => V::unk(),
            };
            vals[i] = v;
        }
        next
    }

    /// Where the access at inst `i` of block `k` is in the frame: `None`
    /// if not into it, `Some(None)` into it but not followed (escaping, a
    /// saved register's, through arithmetic), `Some(Some(o))` a word the
    /// run follows.
    fn frame_place(&self, k: u32, i: usize) -> Option<Option<i32>> {
        let fr = self.f.frame?;
        match fr.places.get(&(k, i))? {
            Place::Exact(o, _) => {
                let saved = fr
                    .slots
                    .iter()
                    .any(|sl| sl.saved && sl.lo <= *o && *o < sl.hi);
                let esc = fr.escapes_at(k, i).unwrap_or(true);
                Some((!saved && !esc && *o < 0).then_some(*o))
            }
            Place::Through(_) => Some(None),
            Place::Elsewhere => None,
        }
    }

    fn exit(&mut self, edge: &Edge, at: u32, s: &St, next: &mut Vec<(u32, St)>) {
        let me = self.f.entry;
        let call = |run: &mut Self, ks: Option<Vec<u32>>, regs: &[V], n: usize| -> Vec<V> {
            match ks {
                Some(ks) if !ks.is_empty() => {
                    let mut rets = vec![V::default(); 4];
                    for k in ks {
                        if !run.fns.contains(&k) {
                            return vec![V::unk(); 4];
                        }
                        run.g.give(k, regs, n);
                        for (a, b) in rets.iter_mut().zip(run.g.ret_of(k)) {
                            a.join(&b);
                        }
                    }
                    rets
                }
                _ => vec![V::unk(); 4],
            }
        };
        match edge {
            Edge::Goto(ks) => {
                for &k in ks {
                    next.push((k, s.clone()));
                }
            }
            Edge::Call { target, ret } => {
                let ks = match target {
                    Some(k) => Some(vec![*k]),
                    None => self.callees(at).map(|c| c.iter().copied().collect()),
                };
                let rets = call(self, ks, &s.regs, 4);
                let mut after = s.clone();
                after.regs[..4].clone_from_slice(&rets);
                for r in [12, 14] {
                    after.regs[r] = V::unk();
                }
                next.push((*ret, after));
            }
            Edge::Return => self.g.give_ret(me, &s.regs[..4]),
            Edge::Tail(t) => {
                let ks = match t {
                    Some(k) => Some(vec![*k]),
                    None => self.callees(at).map(|c| c.iter().copied().collect()),
                };
                let rets = call(self, ks, &s.regs, 16);
                self.g.give_ret(me, &rets);
            }
            Edge::TailOr(ks) => {
                let cs = self.callees(at).map(|c| c.iter().copied().collect());
                let rets = call(self, cs, &s.regs, 16);
                self.g.give_ret(me, &rets);
                for &k in ks {
                    next.push((k, s.clone()));
                }
            }
            Edge::Swi(n) => {
                let mut after = s.clone();
                for r in &mut after.regs[..4] {
                    *r = V::unk();
                }
                next.push((*n, after));
            }
            Edge::Stop => {}
        }
    }
}

/// One type the run found: the sites of its objects, and its fields
/// (offset to width in bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ty {
    pub name: String,
    pub sites: Vec<u32>,
    pub fields: BTreeMap<u32, u8>,
}

impl Ty {
    /// The struct on one line, `#[repr(C, packed)]` (`Ram`'s bytes have
    /// no alignment at the packed layout), its fields at their offsets
    /// with byte padding between, padded to its alignment in the guest:
    /// `Plain::ALIGN`, its widest field's, which `Game::at` checks the
    /// guest address against.
    pub fn decl(&self) -> String {
        let align = self.fields.values().copied().max().unwrap_or(1) as u32;
        let mut f = Vec::new();
        let mut at = 0;
        for (&o, &w) in &self.fields {
            if o > at {
                f.push(format!("_{at:x}: [u8; {}]", o - at));
            }
            f.push(format!("pub f_{o:x}: u{}", 8 * w as u32));
            at = o + w as u32;
        }
        let size = at.div_ceil(align) * align;
        if size > at {
            f.push(format!("_{at:x}: [u8; {}]", size - at));
        }
        let sites: Vec<String> = self.sites.iter().map(|a| format!("{a:#010x}")).collect();
        let n = &self.name;
        format!(
            "#[repr(C, packed)] pub struct {n} {{ {} }} unsafe impl Plain for {n} {{ const ALIGN: u32 = {align}; }} // objects made at {} (native/ref-heap)\n",
            f.join(", "),
            sites.join(", ")
        )
    }
}

/// A typed access: its type (an index into [`Heap::types`]) and field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub ty: usize,
    pub off: u32,
    pub width: u8,
}

/// What the run found.
#[derive(Debug, Default)]
pub struct Heap {
    pub types: Vec<Ty>,
    /// The typed accesses, by (function, block, inst index).
    pub access: BTreeMap<(u32, u32, usize), Field>,
    /// The accesses through an object's pointer that no type explains.
    pub unexplained: BTreeSet<(u32, u32, usize)>,
    pub rounds: usize,
    /// How long the run took, in seconds.
    pub secs: f64,
    /// The run didn't settle in [`ROUNDS`]: nothing is typed.
    pub unsettled: bool,
}

impl Heap {
    /// The `Ref` to field `f`'s object from `p`, a pointer `k` bytes into
    /// it.
    fn r(&self, f: Field, p: &str, k: u32) -> String {
        let t = &self.types[f.ty].name;
        match k {
            0 => format!("Ref::<{t}>::new({p})"),
            k => format!("Ref::<{t}>::to({p}, {k:#x})"),
        }
    }

    /// A field read through `p`, `k` bytes into its object, as the printed
    /// load's value (`u32`).
    pub fn load(&self, f: Field, p: &str, k: u32) -> String {
        let e = format!("g.at({}).f_{:x}", self.r(f, p, k), f.off);
        match f.width {
            4 => e,
            _ => format!("({e} as u32)"),
        }
    }

    /// A field written through `p`, `k` bytes into its object.
    pub fn store(&self, f: Field, p: &str, k: u32, val: &str) -> String {
        let cast = match f.width {
            4 => String::new(),
            w => format!(" as u{}", 8 * w as u32),
        };
        format!("g.at_mut({}).f_{:x} = {val}{cast};", self.r(f, p, k), f.off)
    }
}

fn find(p: &mut BTreeMap<u32, u32>, x: u32) -> u32 {
    let mut r = x;
    while let Some(&q) = p.get(&r) {
        if q == r {
            break;
        }
        r = q;
    }
    p.insert(x, r);
    r
}

/// Follow pointers through `fns` (every function of the program), the
/// indirect calls reaching `sites`' callees, `taken` the functions whose
/// address the program takes (`None`: the function-address flow didn't
/// run, so neither is known); `init` is RAM's word at an address when the
/// program starts (0 where nothing is copied).
pub fn follow(
    fns: &[FnIn],
    sites: &BTreeMap<u32, Site>,
    taken: Option<&BTreeSet<u32>>,
    init: &dyn Fn(u32) -> u32,
) -> Heap {
    let keys: BTreeSet<u32> = fns.iter().map(|f| f.entry).collect();
    let mut direct = BTreeSet::new();
    let mut closed = true;
    for f in fns {
        for b in f.blocks.values() {
            for o in b.exits.values() {
                match &o.edge {
                    Edge::Call {
                        target: Some(k), ..
                    }
                    | Edge::Tail(Some(k)) => {
                        direct.insert(*k);
                    }
                    // A jump by value the function-address flow has no set
                    // for takes no function's address: it stays in the
                    // function.
                    Edge::Call { target: None, .. } | Edge::Tail(None) | Edge::TailOr(_) => {
                        match sites.get(&o.at) {
                            Some(s) if s.complete => direct.extend(s.callees.iter().copied()),
                            None if !matches!(o.edge, Edge::Call { .. }) && taken.is_some() => {}
                            _ => closed = false,
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut g = G::default();
    // Roots (no caller the run sees: the reset, the IRQ handler), and
    // when an indirect call's set isn't complete, every function whose
    // address is taken (all of them, if that isn't known): unknown
    // registers.
    for f in fns {
        let reached = match taken {
            Some(t) => t.contains(&f.entry),
            None => true,
        };
        if !direct.contains(&f.entry) || (!closed && reached) {
            g.entry.insert(f.entry, vec![V::unk(); 16]);
        }
    }
    let mut h = Heap::default();
    let t0 = std::time::Instant::now();
    let trace = std::env::var_os("GBAA_HEAP_TRACE").is_some();
    let trace_cell = std::env::var("GBAA_HEAP_CELL")
        .ok()
        .and_then(|c| u32::from_str_radix(c.trim_start_matches("0x"), 16).ok());
    let run_all = |g: &mut G| {
        for f in fns {
            if !g.entry.contains_key(&f.entry) {
                continue;
            }
            Run {
                f,
                fns: &keys,
                sites,
                init,
                g,
                trace_cell,
            }
            .function();
        }
    };
    loop {
        h.rounds += 1;
        g.changed = false;
        g.why.clear();
        run_all(&mut g);
        if trace {
            eprintln!("heap round {}: {:?}", h.rounds, g.why);
        }
        if !g.changed {
            break;
        }
        if h.rounds >= ROUNDS {
            h.unsettled = true;
            h.secs = t0.elapsed().as_secs_f64();
            return h;
        }
    }
    g.record = Some(BTreeMap::new());
    run_all(&mut g);
    h.secs = t0.elapsed().as_secs_f64();
    let rec = g.record.take().unwrap_or_default();
    if trace {
        eprintln!(
            "heap: {} rounds, {} functions run, {} cells, {} fields",
            h.rounds,
            g.entry.len(),
            g.cells.len(),
            g.fields.len()
        );
        for (c, v) in &g.cells {
            eprintln!("  cell {c:#010x}: {v:?}");
        }
        for (c, v) in &g.fields {
            eprintln!("  field {c:x?}: {v:?}");
        }
        for ((f, b, i), (v, w)) in &rec {
            if v.t.iter().any(|t| t.ptr()) {
                eprintln!("  {f:#010x} {b:#010x}.{i} w{w}: {v:?}");
            }
        }
    }
    // An access through objects' pointers: (site, offset) of each token,
    // nulls and the unknown left out (what the run doesn't follow, which
    // `Game::at` checks); `None` if a token isn't one.
    let objs = |v: &V| -> Option<Vec<(u32, i32)>> {
        let mut out = Vec::new();
        for &t in &v.t {
            match t {
                T::Obj(s, o) => out.push((s, o)),
                _ if nullish(t) => {}
                _ => return None,
            }
        }
        (!out.is_empty()).then_some(out)
    };
    let through = |v: &V| v.t.iter().any(|t| matches!(t, T::Obj(..) | T::Any(_)));
    let mut uf: BTreeMap<u32, u32> = BTreeMap::new();
    for (v, _) in rec.values() {
        if let Some(os) = objs(v) {
            let r = find(&mut uf, os[0].0);
            for &(s, _) in &os[1..] {
                let q = find(&mut uf, s);
                uf.insert(q, r);
            }
        }
    }
    // Each candidate: its class's root, offset and width.
    let mut cand = BTreeMap::new();
    for (&k, (v, w)) in &rec {
        let Some(os) = objs(v) else {
            if through(v) {
                h.unexplained.insert(k);
            }
            continue;
        };
        let o = os[0].1;
        // At one offset, aligned for the access (an unaligned word load
        // rotates, which a field doesn't).
        if os.iter().any(|&(_, x)| x != o) || o < 0 || o % *w as i32 != 0 {
            h.unexplained.insert(k);
            continue;
        }
        let root = find(&mut uf, os[0].0);
        let off = o as u32;
        cand.insert(k, (root, off, *w));
    }
    // Fields by class; an overlap at another offset or width rules both out.
    let mut fields: BTreeMap<u32, BTreeSet<(u32, u8)>> = BTreeMap::new();
    for &(r, o, w) in cand.values() {
        fields.entry(r).or_default().insert((o, w));
    }
    let ok = |fs: &BTreeSet<(u32, u8)>, o: u32, w: u8| {
        fs.iter()
            .all(|&(p, x)| (p, x) == (o, w) || p + x as u32 <= o || o + w as u32 <= p)
    };
    let mut ty_of: BTreeMap<u32, usize> = BTreeMap::new();
    let mut sites_of: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for s in uf.keys().copied().collect::<Vec<_>>() {
        let r = find(&mut uf, s);
        sites_of.entry(r).or_default().insert(s);
    }
    for (k, (r, o, w)) in cand {
        let fs = &fields[&r];
        if !ok(fs, o, w) {
            h.unexplained.insert(k);
            continue;
        }
        let ty = *ty_of.entry(r).or_insert_with(|| {
            let sites: Vec<u32> = sites_of[&r].iter().copied().collect();
            h.types.push(Ty {
                name: format!("H_{:08x}", sites[0]),
                sites,
                fields: BTreeMap::new(),
            });
            h.types.len() - 1
        });
        h.types[ty].fields.insert(o, w);
        h.access.insert(
            k,
            Field {
                ty,
                off: o,
                width: w,
            },
        );
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_arithmetic() {
        let cur = V::of(T::Ram(0x0200_00c8));
        let pad = V::unk();
        // The allocator's cursor plus padding: a new object.
        let p = add(&cur, &pad, 0x0800_0290);
        assert_eq!(p, V::of(T::Obj(0x0800_0290, 0)));
        // Plus a constant: a field.
        let f = add(&p, &V::of(T::Num(8)), 0);
        assert_eq!(f, V::of(T::Obj(0x0800_0290, 8)));
        // Less one: `subs` is `a + !b + 1`.
        let b = add(&add(&f, &not(&V::of(T::Num(4))), 0), &V::of(T::Num(1)), 0);
        assert_eq!(b, V::of(T::Obj(0x0800_0290, 4)));
        // A difference of pointers is a number.
        assert!(add(&f, &not(&p), 0).unk);
        // Null plus an unknown is nothing.
        assert_eq!(add(&V::of(T::Num(0)), &V::unk(), 1), V::default());
        let mut z = V::of(T::Num(0));
        z.join(&cur);
        assert_eq!(add(&z, &V::unk(), 7), V::of(T::Obj(7, 0)));
        // Aligned, and a word load's rotation.
        let m = and(&f, &V::of(T::Num(!3)));
        assert_eq!(m, f);
        assert_eq!(and(&f, &V::of(T::Num(3))), V::of(T::Num(0)));
        // Many offsets: anywhere in it.
        let mut many = V::default();
        for o in 0..10 {
            many.join(&V::of(T::Obj(5, 4 * o)));
        }
        assert_eq!(many, V::of(T::Any(5)));
    }

    #[test]
    fn decl_pads_to_offsets() {
        let t = Ty {
            name: "H_08000290".into(),
            sites: vec![0x0800_0290],
            fields: BTreeMap::from([(0, 4), (8, 2), (12, 4)]),
        };
        let d = t.decl();
        assert_eq!(
            d,
            "#[repr(C, packed)] pub struct H_08000290 { pub f_0: u32, _4: [u8; 4], pub f_8: u16, _a: [u8; 2], pub f_c: u32 } unsafe impl Plain for H_08000290 { const ALIGN: u32 = 4; } // objects made at 0x08000290 (native/ref-heap)\n"
        );
        let h = Heap {
            types: vec![t],
            ..Heap::default()
        };
        let f = Field {
            ty: 0,
            off: 8,
            width: 2,
        };
        assert_eq!(
            h.load(f, "v3", 8),
            "(g.at(Ref::<H_08000290>::to(v3, 0x8)).f_8 as u32)"
        );
        assert_eq!(
            h.store(f, "v2", 0, "v4"),
            "g.at_mut(Ref::<H_08000290>::new(v2)).f_8 = v4 as u16;"
        );
    }
}
