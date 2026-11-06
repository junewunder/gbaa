//! Function addresses followed to every use (`native/fnref-flow`;
//! `docs/native-stage-b.md`, piece 3; `docs/native-port-design.md`,
//! section 4, "Function pointers").
//!
//! Dense `FnRef` IDs (`native/fnref-dense`) rewrite each word that holds a
//! function's address. That is only right where every value the word
//! enters is used as section 4 allows: called, stored (and loaded back),
//! compared for equality, split into halves and joined again (`>> 16`,
//! `& 0xffff`, `<< 16`, `|`), and `| 1` on a Thumb address, folded. This
//! module follows them, over the whole program, and says which words it
//! proved, and each indirect call's callee set.
//!
//! **What it follows.** A whole-program abstract run of every function's
//! IR (stand-ins' original code too: a stand-in does what its code did).
//! A value is a set of tokens ([`Tok`]): a function's address in some
//! form ([`Form`], with the [`Origin`] it came from: a ROM word or a
//! constant the code made), a number, a pointer into ROM (exact, or "from
//! here on" after an index the run doesn't know), and two flags: read
//! from RAM, and unknown (an I/O read, a root's parameter, arithmetic the
//! run doesn't follow). Sources:
//!
//! - a constant or pool word equal to a function's key (the IR's `Pool`
//!   op is placed back at its literal's address by decoding the `ldr
//!   [pc]`);
//! - a load through a ROM pointer, of a word holding a function's key
//!   (agb's vtables, tables of handlers); a load "from here on" reads
//!   every word up to the next code ([`Image::bound`]);
//! - the program start's copies of ROM data to RAM (`Discovery::copies`
//!   that aren't code) and `CpuSet` from ROM.
//!
//! Memory: each function's own frame words, exactly where the frame's run
//! placed the access ([`super::frame`]) and the slot doesn't escape (a
//! saved register's slot gives back the entry value); RAM words at
//! constant addresses, one cell each; and one pile for every other store
//! (the heap, escaping frame slots, anything computed). A load reads its
//! cell and the pile, or, at an address the run can't place, the pile and
//! every cell, or, where the value-set analysis bounds it to ROM, the
//! words there. Interprocedural: a call passes r0-r3 to its callee's entry
//! and takes back what its returns hold; a tail call or a fall passes
//! every register. An indirect call passes them to each function whose
//! whole address its target may hold. A call whose target holds none is
//! an incomplete callee set: its arguments go to every function that may
//! be called that way (one with no direct caller, or whose address is
//! taken), and its results come back from all of them. Functions rerun
//! (a worklist) until nothing they read changes.
//!
//! **The closed world.** Function and ROM addresses enter values only
//! from the sources above; a number made by arithmetic the run doesn't
//! follow, or read from I/O, is never one. So an unknown value is never a
//! function's address, and the whole addresses a target may hold are its
//! complete callee set (a piece of one called is a hole of its origin).
//! Compiled code keeps that; hand-written code that builds a code address
//! from parts wouldn't, and its constants would show as not proven (they
//! are never read as a whole word).
//!
//! **What isn't allowed** ([`Why`]): a function's address used as an
//! address, in arithmetic but the split and join, in an ordered compare or
//! a bit test, its halves or bytes used as anything but stored or joined,
//! called in part. Lost (not followed, so not proven): given to the BIOS,
//! moved to a user-bank register. A word is **proven** when the run read
//! it and no value it entered met either. A function word in ROM the run
//! never read (no tracked load reaches it) isn't proven either.
//!
//! Nothing printed changes: the report lists the words, the holes and the
//! callee sets ([`Flow::report`]), and the metrics count the words not
//! proven (barred) and proven. `GBAA_FNREF_TRACE=1` prints each round of
//! the run and each use not allowed, with its operands.
//!
//! Precision: the one pile is the limit (`docs/native-stage-b.md`, piece
//! 3, as built). agb keeps closures and trait objects on its heap, so a
//! load through any heap pointer may give every function address stored
//! there, and one such load used as a number marks them all.

use super::frame::{Edge, Frame};
use analysis::ir::{BinOp, Exit, Inst, Op, ValueId};
use armv4t::sem::{ShiftType, Width};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Where a function's address came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// A word of ROM: a literal pool's, a vtable's, a table's.
    Word(u32),
    /// A constant the code makes (an immediate, pc-relative arithmetic, a
    /// pool load not placed back at its word): its block and inst index.
    Code(u32, u32),
}

/// The form a function's address is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Form {
    /// The address itself (`| 1` folded).
    Full,
    /// `>> 16`: its top half, at the bottom.
    Hi,
    /// `& 0xffff` (or stored as a halfword): its bottom half.
    Lo,
    /// `Hi << 16`: its top half in place, waiting to be joined.
    HiUp,
    /// `<< 16` of the whole or the bottom half.
    LoUp,
    /// Bytes of it, read or stored narrower than it was (a byte copy).
    Part,
    /// `!` of it: one side of a compare (`a + !b + 1`).
    Not,
    /// A difference with it: a compare's result, for `== 0` only.
    Cmp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Tok {
    Fn {
        key: u32,
        origin: Origin,
        form: Form,
    },
    Num(u32),
    /// A pointer into ROM.
    Rom(u32),
    /// Somewhere in ROM from here on.
    RomFrom(u32),
}

/// A token set, shared until changed (values are copied at every
/// register read).
#[derive(Clone, Debug, Default, Eq)]
struct Toks(std::rc::Rc<BTreeSet<Tok>>);

impl PartialEq for Toks {
    fn eq(&self, o: &Toks) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &o.0) || self.0 == o.0
    }
}

impl std::ops::Deref for Toks {
    type Target = BTreeSet<Tok>;
    fn deref(&self) -> &BTreeSet<Tok> {
        &self.0
    }
}

impl std::ops::DerefMut for Toks {
    fn deref_mut(&mut self) -> &mut BTreeSet<Tok> {
        std::rc::Rc::make_mut(&mut self.0)
    }
}

impl<'a> IntoIterator for &'a Toks {
    type Item = &'a Tok;
    type IntoIter = std::collections::btree_set::Iter<'a, Tok>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl FromIterator<Tok> for Toks {
    fn from_iter<I: IntoIterator<Item = Tok>>(i: I) -> Toks {
        Toks(std::rc::Rc::new(i.into_iter().collect()))
    }
}

/// What the run knows of a value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Val {
    toks: Toks,
    /// Maybe read from RAM (where every function address stored is
    /// followed).
    mem: bool,
    /// Maybe anything else.
    unk: bool,
}

/// More numbers than this in a value: it's unknown.
const NUMS: usize = 8;
/// More exact ROM pointers than this: "from the lowest on".
const ROMS: usize = 16;

impl Val {
    fn unk() -> Val {
        Val {
            unk: true,
            ..Val::default()
        }
    }

    fn of(t: Tok) -> Val {
        Val {
            toks: [t].into_iter().collect(),
            ..Val::default()
        }
    }

    fn join(&mut self, o: &Val) -> bool {
        let before = (self.toks.len(), self.mem, self.unk);
        if self.toks.is_empty() {
            self.toks = o.toks.clone();
        } else if !o.toks.is_subset(&self.toks) {
            self.toks.extend(o.toks.iter().copied());
        }
        self.mem |= o.mem;
        self.unk |= o.unk;
        self.cap();
        before != (self.toks.len(), self.mem, self.unk)
    }

    /// Widen: too many numbers are unknown, too many ROM pointers are
    /// one "from the lowest on" (a loop walking a table), and a pointer
    /// "from here on" takes in those above it.
    fn cap(&mut self) {
        if self
            .toks
            .iter()
            .filter(|t| matches!(t, Tok::Num(_)))
            .count()
            > NUMS
        {
            self.unk = true;
        }
        // Unknown takes in every number (so the cap stays monotone).
        if self.unk {
            self.toks.retain(|t| !matches!(t, Tok::Num(_)));
        }
        let roms: Vec<u32> = self
            .toks
            .iter()
            .filter_map(|t| match t {
                Tok::Rom(a) => Some(*a),
                _ => None,
            })
            .collect();
        if roms.len() > ROMS {
            self.toks.retain(|t| !matches!(t, Tok::Rom(_)));
            self.toks.insert(Tok::RomFrom(roms[0]));
        }
        let froms: Vec<u32> = self
            .toks
            .iter()
            .filter_map(|t| match t {
                Tok::RomFrom(a) => Some(*a),
                _ => None,
            })
            .collect();
        if let Some(&lo) = froms.first() {
            self.toks.retain(|t| match t {
                Tok::Rom(a) | Tok::RomFrom(a) => *a < lo,
                _ => true,
            });
            self.toks.insert(Tok::RomFrom(lo));
        }
    }

    fn fns(&self) -> impl Iterator<Item = (u32, Origin, Form)> + '_ {
        self.toks.iter().filter_map(|t| match *t {
            Tok::Fn { key, origin, form } => Some((key, origin, form)),
            _ => None,
        })
    }

    /// Each token, and `None` for the flags' unknown part.
    fn parts(&self) -> Vec<Option<Tok>> {
        let mut v: Vec<Option<Tok>> = self.toks.iter().copied().map(Some).collect();
        if self.mem || self.unk || v.is_empty() {
            v.push(None);
        }
        v
    }
}

/// Why a function's address wasn't proven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Why {
    Address,
    Arithmetic,
    Ordered,
    BitTest,
    PartUsed,
    CmpUsed,
    CalledInPart,
    Psr,
    // Lost: not followed.
    Bios,
    UserBank,
}

impl Why {
    /// Not a forbidden use: the value went where the run doesn't follow.
    pub fn lost(self) -> bool {
        matches!(self, Why::Bios | Why::UserBank)
    }

    pub fn what(self) -> &'static str {
        match self {
            Why::Address => "used as an address",
            Why::Arithmetic => "arithmetic but the split, join and | 1",
            Why::Ordered => "an ordered compare (<, >=)",
            Why::BitTest => "a bit test or shift's carry",
            Why::PartUsed => "its bytes or a half used as a value",
            Why::CmpUsed => "a compare's result used as a value",
            Why::CalledInPart => "called in part",
            Why::Psr => "written to a PSR",
            Why::Bios => "given to the BIOS (lost)",
            Why::UserBank => "moved to a user-bank register (lost)",
        }
    }
}

/// What the run needs of the program's bytes.
pub trait Image {
    /// Whether `v` is a function's key.
    fn is_fn(&self, v: u32) -> bool;
    /// The aligned ROM word at `a & !3`, if `a` is in ROM.
    fn word(&self, a: u32) -> Option<u32>;
    /// Where data a variable index from `a` may reach ends (the next code
    /// in ROM).
    fn bound(&self, a: u32) -> u32;
}

/// How an exit leaves, for the run.
#[derive(Clone, Debug)]
pub struct Out {
    pub edge: Edge,
    /// A call through a `bx rN` stub: the register.
    pub via: Option<u8>,
    /// The instruction's address.
    pub at: u32,
    /// A BIOS call's number.
    pub bios: Option<u8>,
}

/// One block: its IR, its exits by inst index, and the ROM words its
/// `Pool` ops read, by inst index.
#[derive(Clone, Debug)]
pub struct FBlock<'a> {
    pub insts: &'a [Inst],
    pub exits: BTreeMap<usize, Out>,
    pub pools: BTreeMap<usize, u32>,
    /// The value-set analysis's bounds on the addresses of the loads and
    /// stores it bounds, by inst index.
    pub hulls: BTreeMap<usize, (u32, u32)>,
}

/// One function.
pub struct FnIn<'a> {
    pub entry: u32,
    pub blocks: BTreeMap<u32, FBlock<'a>>,
    /// Its frame ([`super::frame`]): where its sp-relative accesses go.
    pub frame: Option<&'a Frame>,
}

/// An indirect call's or a tail jump's callees.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    pub callees: BTreeSet<u32>,
    /// The target can hold no other function's address (the closed
    /// world): the set isn't empty.
    pub complete: bool,
    /// A jump by value (a tail call), not a call.
    pub tail: bool,
}

/// What the run found.
#[derive(Debug, Default)]
pub struct Flow {
    /// The ROM words the run read a function's address from.
    pub read: BTreeSet<u32>,
    /// The constants the code makes that are a function's address, with
    /// their keys.
    pub consts: BTreeMap<Origin, u32>,
    /// The first use not allowed (or lost) of each origin: where, and why.
    pub holes: BTreeMap<Origin, (u32, Why)>,
    /// Each indirect call's (by its instruction's address) and each jump
    /// by value's that may take a function's address.
    pub sites: BTreeMap<u32, Site>,
    /// The origins stored where the run can't place them (the pile).
    pub piled: BTreeSet<Origin>,
    /// The run didn't settle: nothing is proven.
    pub unsettled: bool,
}

impl Flow {
    /// Whether ROM word `w`'s function address is proven.
    pub fn proven(&self, w: u32) -> bool {
        !self.unsettled && self.read.contains(&w) && !self.holes.contains_key(&Origin::Word(w))
    }
}

type Mem = BTreeSet<(Tok, u8)>;

#[derive(Default)]
struct G {
    entry: BTreeMap<u32, Vec<Val>>,
    ret: BTreeMap<u32, Vec<Val>>,
    /// Every store the run can't place (the heap, escaping frame slots).
    pile: Mem,
    /// [`G::read_pile`]'s, by width.
    pile_reads: [Option<Val>; 5],
    cells: BTreeMap<u32, Mem>,
    spills: BTreeMap<u32, Mem>,
    /// What incomplete callee sets pass.
    loose_args: Vec<Val>,
    /// Functions whose address is taken.
    taken: BTreeSet<u32>,
    /// What a read from ROM "from here on" gives, by (address, width).
    rom_from: BTreeMap<(u32, u8), Val>,
    /// What a read the value-set analysis bounds to ROM gives, by bounds
    /// and width.
    rom_hull: BTreeMap<(u32, u32, u8), Val>,
    flow: Flow,
    changed: bool,
    /// What changed since the functions last ran, to rerun what reads it.
    dirty_entry: BTreeSet<u32>,
    dirty_ret: BTreeSet<u32>,
    dirty_mem: bool,
    dirty_loose: bool,
}

impl G {
    fn mark(&mut self, c: bool) {
        self.changed |= c;
    }

    fn hole(&mut self, v: &Val, at: u32, why: Why, only: impl Fn(Form) -> bool) {
        let os: Vec<Origin> = v.fns().filter(|f| only(f.2)).map(|f| f.1).collect();
        for o in os {
            // An output only: nothing the run reads.
            self.flow.holes.entry(o).or_insert((at, why));
        }
    }

    fn store(&mut self, to: Store, toks: &[(Tok, u8)]) {
        let m = match to {
            // One pile: splitting it by the value-set analysis's bounds on
            // the stores' addresses proved nothing more on the agb programs
            // (their heap stores aren't bounded) and cost nine times the
            // time.
            Store::Pile => {
                for (t, _) in toks {
                    if let Tok::Fn { origin, .. } = t {
                        self.flow.piled.insert(*origin);
                    }
                }
                &mut self.pile
            }
            Store::Cell(a) => self.cells.entry(a).or_default(),
            Store::Spill(f) => self.spills.entry(f).or_default(),
        };
        let n = m.len();
        m.extend(toks.iter().copied());
        let c = m.len() != n;
        if c {
            self.pile_reads = Default::default();
            match to {
                Store::Spill(f) => {
                    self.dirty_entry.insert(f);
                }
                _ => self.dirty_mem = true,
            }
        }
        self.mark(c);
    }

    /// What a load at width `w` reads of the pile (kept until the pile
    /// changes).
    fn read_pile(&mut self, w: u8, into: &mut Val) {
        let i = w as usize;
        if self.pile_reads[i].is_none() {
            let mut v = Val::default();
            from_mem(&self.pile, w, &mut v);
            self.pile_reads[i] = Some(v);
        }
        into.join(self.pile_reads[i].as_ref().unwrap());
    }

    fn give(&mut self, f: u32, regs: &[Val]) {
        let e = self
            .entry
            .entry(f)
            .or_insert_with(|| vec![Val::default(); 16]);
        let mut c = false;
        for (a, b) in e.iter_mut().zip(regs) {
            c |= a.join(b);
        }
        if c {
            self.dirty_entry.insert(f);
        }
        self.mark(c);
    }

    fn give_ret(&mut self, f: u32, regs: &[Val]) {
        let e = self.ret.entry(f).or_insert_with(|| vec![Val::default(); 4]);
        let mut c = false;
        for (a, b) in e.iter_mut().zip(regs) {
            c |= a.join(b);
        }
        if c {
            self.dirty_ret.insert(f);
        }
        self.mark(c);
    }

    fn taken_add(&mut self, v: &Val) {
        let ks: Vec<u32> = v.fns().filter(|f| f.2 == Form::Full).map(|f| f.0).collect();
        for k in ks {
            let c = self.taken.insert(k);
            self.mark(c);
        }
    }
}

#[derive(Clone, Copy)]
enum Store {
    Pile,
    Cell(u32),
    Spill(u32),
}

fn bytes(w: Width) -> u8 {
    match w {
        Width::Byte => 1,
        Width::Half => 2,
        Width::Word => 4,
    }
}

fn is_ram(a: u32) -> bool {
    (0x0200_0000..0x0400_0000).contains(&a)
}

/// What a value stored at width `w` leaves in memory, and the forms that
/// can't be stored.
fn to_mem(v: &Val, w: u8) -> (Vec<(Tok, u8)>, Vec<Form>) {
    let mut out = Vec::new();
    let mut bad = Vec::new();
    for &t in &v.toks {
        match t {
            Tok::Fn { key, origin, form } => {
                let f = match (form, w) {
                    (Form::Full, 4) => Some(Form::Full),
                    (Form::Full, 2) => Some(Form::Lo),
                    (Form::Full | Form::Part, _) => Some(Form::Part),
                    (Form::Lo | Form::Hi, 2 | 4) => Some(form),
                    (Form::HiUp | Form::LoUp, 4) => Some(form),
                    _ => None,
                };
                match f {
                    Some(form) => out.push((Tok::Fn { key, origin, form }, w)),
                    None => bad.push(form),
                }
            }
            Tok::Rom(_) | Tok::RomFrom(_) if w == 4 => out.push((t, 4)),
            _ => {}
        }
    }
    (out, bad)
}

/// What a load at width `w` reads of memory `m`.
fn from_mem(m: &Mem, w: u8, into: &mut Val) {
    into.mem = true;
    for &(t, sw) in m {
        let t = match t {
            Tok::Fn { key, origin, form } => {
                // Read at the width it was stored at, it is what was
                // stored; pieces read as a word are a copy reassembled
                // (`memcpy` by bytes or halves); anything else, a piece.
                let form = if sw == w {
                    form
                } else if w == 4 && sw < 4 {
                    Form::Full
                } else {
                    Form::Part
                };
                Tok::Fn { key, origin, form }
            }
            t if w == 4 => t,
            _ => continue,
        };
        into.toks.insert(t);
    }
    into.cap();
}

struct Run<'a, 'g> {
    f: &'a FnIn<'a>,
    img: &'a dyn Image,
    g: &'g mut G,
    /// Functions any incomplete callee set may reach.
    loose: &'a BTreeSet<u32>,
    ins: BTreeMap<u32, St>,
    /// Blocks to run, lowest address first (near enough reverse
    /// postorder for compiled code, so loops settle in fewer passes).
    work: BTreeSet<u32>,
    trace: bool,
    /// The functions whose results the run read.
    rets_read: BTreeSet<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct St {
    regs: Vec<Val>,
    /// The function's own frame words (offset, width) the run knows.
    frame: BTreeMap<i32, (Val, u8)>,
}

impl St {
    fn join(&self, o: &St) -> St {
        let mut regs = self.regs.clone();
        for (a, b) in regs.iter_mut().zip(&o.regs) {
            a.join(b);
        }
        // A word below the entry sp one side never stored holds nothing
        // of this run's yet (garbage no one reads): the other side's. One
        // at or above it is the caller's (a stack argument), and two
        // widths clash: width 0, unknown (kept, so the join only grows).
        let mut frame = BTreeMap::new();
        for k in self.frame.keys().chain(o.frame.keys()) {
            let v = match (self.frame.get(k), o.frame.get(k)) {
                (Some((a, aw)), Some((b, bw))) if aw == bw => {
                    let mut v = a.clone();
                    v.join(b);
                    (v, *aw)
                }
                (Some(x), None) | (None, Some(x)) if *k < 0 => x.clone(),
                _ => (Val::unk(), 0),
            };
            frame.insert(*k, v);
        }
        St { regs, frame }
    }
}

/// The result of `op` on one token of each side (`None`: the unknown
/// part). `Err` with the forms of `x` and `y` that make it a hole.
enum R {
    T(Tok),
    Unk,
    Hole(Why, Option<u32>),
}

impl Run<'_, '_> {
    fn classify(&mut self, c: u32, origin: Origin) -> Tok {
        if self.img.is_fn(c) {
            if let Origin::Code(..) = origin {
                self.g.flow.consts.insert(origin, c);
            }
            Tok::Fn {
                key: c,
                origin,
                form: Form::Full,
            }
        } else {
            num(self.img, c)
        }
    }

    fn rom_read(&mut self, a: u32, w: u8, into: &mut Val) {
        let Some(word) = self.img.word(a) else {
            into.unk = true;
            return;
        };
        if self.img.is_fn(word) {
            let o = Origin::Word(a & !3);
            self.g.flow.read.insert(a & !3);
            let form = if w == 4 && a & 3 == 0 {
                Form::Full
            } else {
                Form::Part
            };
            into.toks.insert(Tok::Fn {
                key: word,
                origin: o,
                form,
            });
            return;
        }
        let sh = (a & 3) * 8;
        let v = match w {
            4 => word,
            2 => (word >> (sh & 16)) & 0xffff,
            _ => (word >> sh) & 0xff,
        };
        into.toks.insert(num(self.img, v));
        into.cap();
    }

    fn rom_from(&mut self, a: u32, w: u8, into: &mut Val) {
        if let Some(v) = self.g.rom_from.get(&(a, w)) {
            into.join(v);
            return;
        }
        let end = self.img.bound(a).min(a.saturating_add(0x4000));
        let mut p = a & !3;
        let mut v = Val::default();
        while p < end {
            self.rom_read(p, w.min(4), &mut v);
            p += 4;
        }
        into.join(&v);
        self.g.rom_from.insert((a, w), v);
    }

    fn bin(&mut self, op: BinOp, a: &Val, b: &Val, site: Origin, at: u32) -> Val {
        let mut out = Val::default();
        // Every token with every token, but for large sets: each with an
        // unknown partner (which only adds holes and unknowns).
        let (xs, ys) = (a.parts(), b.parts());
        let pairs: Vec<(Option<Tok>, Option<Tok>)> = if xs.len() * ys.len() > 64 {
            xs.iter()
                .map(|&x| (x, None))
                .chain(ys.iter().map(|&y| (None, y)))
                .collect()
        } else {
            xs.iter()
                .flat_map(|&x| ys.iter().map(move |&y| (x, y)))
                .collect()
        };
        for (x, y) in pairs {
            {
                match bin_tok(self.img, op, x, y) {
                    R::T(Tok::Num(c)) => {
                        let t = self.classify(c, site);
                        out.toks.insert(t);
                    }
                    R::T(
                        t @ Tok::Fn {
                            form: Form::Cmp, ..
                        },
                    ) => {
                        out.toks.insert(t);
                        // `a - b` with both functions: b is compared too.
                        if let Some(Tok::Fn {
                            key,
                            origin,
                            form: Form::Full | Form::Lo | Form::Hi,
                        }) = y
                        {
                            out.toks.insert(Tok::Fn {
                                key,
                                origin,
                                form: Form::Cmp,
                            });
                        }
                    }
                    R::T(t) => {
                        out.toks.insert(t);
                    }
                    R::Unk => out.unk = true,
                    R::Hole(why, num) => {
                        for t in [x, y].into_iter().flatten() {
                            if let Tok::Fn { .. } = t {
                                self.g.hole(&Val::of(t), at, why, |_| true);
                            }
                        }
                        match num {
                            Some(c) => {
                                out.toks.insert(num_tok(self.img, c));
                            }
                            None => out.unk = true,
                        }
                    }
                }
            }
        }
        out.cap();
        out
    }

    fn flow(&mut self, to: u32, s: &St) {
        if !self.f.blocks.contains_key(&to) {
            return;
        }
        let next = match self.ins.get(&to) {
            Some(old) => old.join(s),
            None => s.clone(),
        };
        if self.ins.get(&to) != Some(&next) {
            self.ins.insert(to, next);
            self.work.insert(to);
        }
    }

    /// What a call to a known function gives back in r0-r3.
    fn ret_of(&mut self, k: u32) -> Vec<Val> {
        self.rets_read.insert(k);
        self.g
            .ret
            .get(&k)
            .cloned()
            .unwrap_or_else(|| vec![Val::default(); 4])
    }

    /// The callees of a call or jump through `t` at `at`.
    fn callees(&mut self, t: &Val, at: u32, tail: bool) -> (BTreeSet<u32>, bool) {
        self.g.hole(t, at, Why::CalledInPart, |f| f != Form::Full);
        let callees: BTreeSet<u32> = t.fns().filter(|f| f.2 == Form::Full).map(|f| f.0).collect();
        // In the closed world nothing unknown is a function's address, so
        // the whole addresses the target may hold are all it can call (a
        // piece of one, called, is a hole of its origin's, above).
        let complete = !callees.is_empty();
        let site = Site {
            callees: callees.clone(),
            complete,
            tail,
        };
        if !tail || !callees.is_empty() {
            self.g.flow.sites.insert(at, site);
        }
        (callees, complete)
    }

    fn block(&mut self, key: u32) {
        let b = &self.f.blocks[&key];
        let mut s = self.ins[&key].clone();
        let mut at = key & !1;
        let mut vals: Vec<Val> = vec![Val::default(); b.insts.len()];
        for (i, inst) in b.insts.iter().enumerate() {
            let site = Origin::Code(key, i as u32);
            let holes_before = self.g.flow.holes.len();
            let val = |x: ValueId| vals[x.index()].clone();
            let guarded = inst.guard.is_some();
            let v = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    Val::default()
                }
                Op::Const(c) => Val::of(self.classify(c, site)),
                Op::Pool(c) => {
                    let o = match b.pools.get(&i) {
                        Some(&w) => {
                            if self.img.is_fn(c) {
                                self.g.flow.read.insert(w);
                            }
                            Origin::Word(w)
                        }
                        None => site,
                    };
                    Val::of(self.classify(c, o))
                }
                Op::Bit(x) => Val::of(Tok::Num(x as u32)),
                Op::GetReg(r) => s.regs[r.index()].clone(),
                Op::Bin(op, a, b) => self.bin(op, &val(a), &val(b), site, at),
                Op::Adc { a, b, c } => {
                    let (a, b) = (val(a), val(b));
                    if a.fns().next().is_some() || b.fns().next().is_some() {
                        // A compare (or an add, which is a hole when used).
                        let mut out = Val::unk();
                        for v in [&a, &b] {
                            self.g.hole(v, at, Why::Arithmetic, |f| {
                                !matches!(f, Form::Full | Form::Lo | Form::Hi | Form::Not)
                            });
                            for (key, origin, _) in v.fns() {
                                out.toks.insert(Tok::Fn {
                                    key,
                                    origin,
                                    form: Form::Cmp,
                                });
                            }
                        }
                        out
                    } else {
                        let ab = self.bin(BinOp::Add, &a, &b, site, at);
                        self.bin(BinOp::Add, &ab, &val(c), site, at)
                    }
                }
                Op::Not(a) => {
                    let a = val(a);
                    let mut out = Val {
                        unk: a.unk || a.mem,
                        ..Val::default()
                    };
                    self.g.hole(&a, at, Why::Arithmetic, |f| {
                        !matches!(f, Form::Full | Form::Lo | Form::Hi)
                    });
                    for &t in &a.toks {
                        match t {
                            Tok::Fn { key, origin, form } => {
                                if matches!(form, Form::Full | Form::Lo | Form::Hi) {
                                    out.toks.insert(Tok::Fn {
                                        key,
                                        origin,
                                        form: Form::Not,
                                    });
                                } else {
                                    out.unk = true;
                                }
                            }
                            Tok::Num(c) | Tok::Rom(c) => {
                                out.toks.insert(num_tok(self.img, !c));
                            }
                            Tok::RomFrom(_) => out.unk = true,
                        }
                    }
                    out.cap();
                    out
                }
                Op::Shift { kind, v, amount } => {
                    let (x, n) = (val(v), val(amount));
                    let n = match (n.toks.iter().next(), n.toks.len(), n.unk || n.mem) {
                        (Some(Tok::Num(n)), 1, false) => Some(*n),
                        _ => None,
                    };
                    let mut out = Val {
                        unk: x.unk || x.mem,
                        ..Val::default()
                    };
                    for &t in &x.toks {
                        match t {
                            Tok::Fn { key, origin, form } => {
                                use Form::*;
                                use ShiftType::*;
                                let f = match (form, kind, n) {
                                    (f, _, Some(0)) => Some(f),
                                    // A word load's rotation by its
                                    // address's low bits: 0 for the
                                    // aligned words compiled code keeps
                                    // function addresses in.
                                    (f, Ror, None) => Some(f),
                                    (Full, Lsr, Some(16)) | (HiUp, Lsr, Some(16)) => Some(Hi),
                                    (Full | Lo, Lsl, Some(16)) => Some(LoUp),
                                    (LoUp, Lsr, Some(16)) => Some(Lo),
                                    (Hi, Lsl, Some(16)) => Some(HiUp),
                                    _ => None,
                                };
                                match f {
                                    Some(form) => {
                                        out.toks.insert(Tok::Fn { key, origin, form });
                                    }
                                    None => {
                                        self.g.hole(&Val::of(t), at, Why::Arithmetic, |_| true);
                                        match n {
                                            Some(n) => {
                                                out.toks
                                                    .insert(num_tok(self.img, shift(kind, key, n)));
                                            }
                                            None => out.unk = true,
                                        }
                                    }
                                }
                            }
                            Tok::Num(c) | Tok::Rom(c) => match n {
                                Some(n) => {
                                    out.toks.insert(num_tok(self.img, shift(kind, c, n)));
                                }
                                None => out.unk = true,
                            },
                            Tok::RomFrom(_) => out.unk = true,
                        }
                    }
                    out.cap();
                    out
                }
                Op::Select { a, b, .. } => {
                    let mut x = val(a);
                    x.join(&val(b));
                    x
                }
                Op::Rrx { v: a, .. } | Op::Sext { v: a, .. } => {
                    self.g.hole(&val(a), at, Why::Arithmetic, |_| true);
                    Val::unk()
                }
                Op::MulLongLo { a, b, .. } | Op::MulLongHi { a, b, .. } => {
                    self.g.hole(&val(a), at, Why::Arithmetic, |_| true);
                    self.g.hole(&val(b), at, Why::Arithmetic, |_| true);
                    Val::unk()
                }
                Op::IsZero(a) => {
                    self.g.hole(&val(a), at, Why::Arithmetic, |f| {
                        !matches!(f, Form::Full | Form::Cmp | Form::Lo | Form::Hi)
                    });
                    Val::unk()
                }
                Op::AdcCarry { a, b, .. } | Op::AdcOverflow { a, b, .. } => {
                    self.g.hole(&val(a), at, Why::Ordered, |_| true);
                    self.g.hole(&val(b), at, Why::Ordered, |_| true);
                    Val::unk()
                }
                Op::MulCarry { a, b, .. } => {
                    self.g.hole(&val(a), at, Why::Arithmetic, |_| true);
                    self.g.hole(&val(b), at, Why::Arithmetic, |_| true);
                    Val::unk()
                }
                Op::ShiftCarry { v: a, .. } | Op::TestBit { v: a, .. } => {
                    self.g.hole(&val(a), at, Why::BitTest, |_| true);
                    Val::unk()
                }
                Op::SetReg(r, x) => {
                    let r = r.index();
                    let x = val(x);
                    if guarded {
                        s.regs[r].join(&x);
                    } else {
                        s.regs[r] = x;
                    }
                    Val::default()
                }
                Op::SetUserReg(_, x) => {
                    self.g.hole(&val(x), at, Why::UserBank, |_| true);
                    Val::default()
                }
                Op::SetCpsr { v: x, .. } | Op::SetSpsr { v: x, .. } => {
                    self.g.hole(&val(x), at, Why::Psr, |_| true);
                    Val::default()
                }
                Op::Load { width, addr } => {
                    let w = bytes(width);
                    let a = val(addr);
                    self.g.hole(&a, at, Why::Address, |_| true);
                    let mut out = self.load(&s, key, i, &a, w, b.hulls.get(&i).copied());
                    if guarded {
                        out.unk = true;
                    }
                    out
                }
                Op::Store { width, addr, value } => {
                    let w = bytes(width);
                    let (a, x) = (val(addr), val(value));
                    self.g.hole(&a, at, Why::Address, |_| true);
                    let (toks, bad) = to_mem(&x, w);
                    for f in bad {
                        let why = match f {
                            Form::Cmp | Form::Not => Why::CmpUsed,
                            _ => Why::PartUsed,
                        };
                        self.g.hole(&x, at, why, |g| g == f);
                    }
                    self.store(&mut s, key, i, &a, w, &x, &toks, guarded);
                    Val::default()
                }
                Op::Exit(e) => {
                    let out = &b.exits[&i];
                    let t = match e {
                        Exit::Jump(t) | Exit::JumpExchange(t) | Exit::ExceptionReturn(t) => {
                            Some(val(t))
                        }
                        _ => None,
                    };
                    self.exit(out, t, &s);
                    Val::default()
                }
                _ => Val::unk(),
            };
            if self.trace && self.g.flow.holes.len() != holes_before {
                let ops: Vec<String> = inst_operands(&inst.op)
                    .into_iter()
                    .map(|x| format!("{:?}", vals[x.index()]))
                    .collect();
                eprintln!("  hole at {at:#010x}: {:?} on {}", inst.op, ops.join(" / "));
            }
            vals[i] = v;
        }
    }

    fn load(
        &mut self,
        s: &St,
        key: u32,
        i: usize,
        a: &Val,
        w: u8,
        hull: Option<(u32, u32)>,
    ) -> Val {
        let mut out = Val::default();
        if let Some(fr) = self.f.frame {
            match fr.places.get(&(key, i)) {
                Some(super::frame::Place::Exact(o, _)) => {
                    // A saved register's slot gives it back as it came.
                    if fr
                        .slots
                        .iter()
                        .any(|sl| sl.saved && sl.lo <= *o && *o < sl.hi)
                    {
                        return Val::unk();
                    }
                    let esc = fr.escapes_at(key, i).unwrap_or(true);
                    match s.frame.get(o) {
                        Some((v, sw)) if *sw == w => {
                            if !esc {
                                return v.clone();
                            }
                            out.join(v);
                        }
                        _ => {
                            let sp = self
                                .g
                                .spills
                                .get(&self.f.entry)
                                .cloned()
                                .unwrap_or_default();
                            from_mem(&sp, w, &mut out);
                        }
                    }
                    if esc {
                        self.g.read_pile(w, &mut out);
                    }
                    return out;
                }
                Some(super::frame::Place::Through(_)) => {
                    let sp = self
                        .g
                        .spills
                        .get(&self.f.entry)
                        .cloned()
                        .unwrap_or_default();
                    from_mem(&sp, w, &mut out);
                    self.g.read_pile(w, &mut out);
                    return out;
                }
                _ => {}
            }
        }
        let mut anywhere = a.unk || a.mem;
        for &t in &a.toks {
            match t {
                Tok::Rom(r) => self.rom_read(r, w, &mut out),
                Tok::RomFrom(r) => self.rom_from(r, w, &mut out),
                Tok::Num(c) | Tok::Fn { key: c, .. } => {
                    if is_ram(c) {
                        // The cells the access overlaps.
                        let cell: Mem = self
                            .g
                            .cells
                            .range(c.saturating_sub(3)..c + w as u32)
                            .flat_map(|(&a, m)| {
                                m.iter().filter(move |(_, sw)| c < a + *sw as u32).copied()
                            })
                            .collect();
                        from_mem(&cell, w, &mut out);
                        self.g.read_pile(w, &mut out);
                    } else if self.img.word(c).is_some() {
                        self.rom_read(c, w, &mut out);
                    } else {
                        out.unk = true;
                    }
                }
            }
        }
        if a.toks.is_empty() {
            anywhere = true;
        }
        if anywhere {
            match hull {
                // Bounded to ROM: its words there.
                Some((lo, hi))
                    if self.img.word(lo).is_some()
                        && self.img.word(hi).is_some()
                        && hi - lo < 0x4000 =>
                {
                    match self.g.rom_hull.get(&(lo, hi, w)) {
                        Some(v) => {
                            out.join(v);
                        }
                        None => {
                            let mut v = Val::default();
                            let mut p = lo & !3;
                            while p <= hi {
                                self.rom_read(p, w, &mut v);
                                p += 4;
                            }
                            out.join(&v);
                            self.g.rom_hull.insert((lo, hi, w), v);
                        }
                    }
                }
                _ => {
                    self.g.read_pile(w, &mut out);
                    let (lo, hi) = hull.unwrap_or((0, u32::MAX));
                    for (_, c) in self.g.cells.range(lo.saturating_sub(3)..=hi) {
                        from_mem(c, w, &mut out);
                    }
                    // An I/O read, too.
                    out.unk = true;
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn store(
        &mut self,
        s: &mut St,
        key: u32,
        i: usize,
        a: &Val,
        w: u8,
        x: &Val,
        toks: &[(Tok, u8)],
        guarded: bool,
    ) {
        if let Some(fr) = self.f.frame {
            match fr.places.get(&(key, i)) {
                Some(super::frame::Place::Exact(o, n)) => {
                    let o = *o;
                    let old = s.frame.get(&o).cloned();
                    s.frame
                        .retain(|&k, (_, kw)| k + *kw as i32 <= o || o + *n as i32 <= k);
                    let new = match (guarded, old) {
                        (false, _) => Some((x.clone(), w)),
                        (true, Some((mut ov, ow))) if ow == w => {
                            ov.join(x);
                            Some((ov, w))
                        }
                        _ => None,
                    };
                    s.frame.extend(new.map(|v| (o, v)));
                    self.g.store(Store::Spill(self.f.entry), toks);
                    if fr.escapes_at(key, i).unwrap_or(true) {
                        self.g.store(Store::Pile, toks);
                    }
                    return;
                }
                Some(super::frame::Place::Through(_)) => {
                    self.g.store(Store::Spill(self.f.entry), toks);
                    self.g.store(Store::Pile, toks);
                    return;
                }
                _ => {}
            }
        }
        if toks.is_empty() {
            return;
        }
        let mut anywhere = a.unk || a.mem || a.toks.is_empty();
        for &t in &a.toks {
            match t {
                Tok::Num(c) | Tok::Fn { key: c, .. } if is_ram(c) => {
                    self.g.store(Store::Cell(c), toks)
                }
                Tok::Num(_) | Tok::Rom(_) | Tok::RomFrom(_) | Tok::Fn { .. } => {}
            }
            if matches!(t, Tok::RomFrom(_)) {
                anywhere = true;
            }
        }
        if anywhere {
            self.g.store(Store::Pile, toks);
        }
    }

    fn exit(&mut self, out: &Out, t: Option<Val>, s: &St) {
        let me = self.f.entry;
        match &out.edge {
            Edge::Goto(ks) => {
                for &k in ks {
                    self.flow(k, s);
                }
            }
            Edge::Call { target, ret } => {
                let args = &s.regs[..4];
                let rets = match target {
                    Some(k) => {
                        self.g.give(*k, args);
                        self.ret_of(*k)
                    }
                    None => {
                        let t = match out.via {
                            Some(r) => s.regs[r as usize].clone(),
                            None => t.unwrap_or_else(Val::unk),
                        };
                        self.call_by_value(&t, out.at, args, false)
                    }
                };
                let mut after = s.clone();
                after.regs[..4].clone_from_slice(&rets);
                for r in [12, 14] {
                    after.regs[r] = Val::unk();
                }
                self.flow(*ret, &after);
            }
            Edge::Return => {
                self.g.give_ret(me, &s.regs[..4]);
                // `bx rN` to a function's address is a tail call.
                if let Some(t) = t.filter(|t| t.fns().next().is_some()) {
                    let rets = self.call_by_value(&t, out.at, &s.regs, true);
                    self.g.give_ret(me, &rets);
                }
            }
            Edge::Tail(Some(k)) => {
                self.g.give(*k, &s.regs);
                let r = self.ret_of(*k);
                self.g.give_ret(me, &r);
            }
            Edge::Tail(None) | Edge::TailOr(_) => {
                let t = t.unwrap_or_else(Val::unk);
                let rets = self.call_by_value(&t, out.at, &s.regs, true);
                self.g.give_ret(me, &rets);
                if let Edge::TailOr(ks) = &out.edge {
                    for &k in ks {
                        self.flow(k, s);
                    }
                }
            }
            Edge::Swi(next) => {
                for r in &s.regs[..4] {
                    self.g.hole(r, out.at, Why::Bios, |_| true);
                }
                // CpuSet and CpuFastSet from ROM copy its words to RAM.
                if matches!(out.bios, Some(0x0B | 0x0C)) {
                    let mut got = Val::default();
                    for &t in &s.regs[0].toks {
                        match t {
                            Tok::Rom(a) | Tok::RomFrom(a) => self.rom_from(a, 4, &mut got),
                            _ => {}
                        }
                    }
                    let (toks, _) = to_mem(&got, 4);
                    self.g.store(Store::Pile, &toks);
                }
                let mut after = s.clone();
                for r in &mut after.regs[..4] {
                    *r = Val::unk();
                }
                self.flow(*next, &after);
            }
            Edge::Stop => {}
        }
    }

    /// A call or tail call through `t`: give `args` to each callee, and
    /// what comes back.
    fn call_by_value(&mut self, t: &Val, at: u32, args: &[Val], tail: bool) -> Vec<Val> {
        let (callees, complete) = self.callees(t, at, tail);
        let mut rets = vec![Val::default(); 4];
        for &k in &callees {
            self.g.give(k, args);
            for (a, b) in rets.iter_mut().zip(self.ret_of(k)) {
                a.join(&b);
            }
        }
        if !complete {
            let mut c = false;
            for (a, b) in self.g.loose_args.iter_mut().zip(args) {
                c |= a.join(b);
            }
            self.g.dirty_loose |= c;
            self.g.mark(c);
            let loose: Vec<u32> = self.loose.iter().copied().collect();
            for k in loose {
                for (a, b) in rets.iter_mut().zip(self.ret_of(k)) {
                    a.join(&b);
                }
            }
            for r in &mut rets {
                r.unk = true;
            }
        }
        rets
    }
}

fn inst_operands(op: &Op) -> Vec<ValueId> {
    match *op {
        Op::Bin(_, a, b)
        | Op::Adc { a, b, .. }
        | Op::AdcCarry { a, b, .. }
        | Op::AdcOverflow { a, b, .. } => vec![a, b],
        Op::Not(a)
        | Op::IsZero(a)
        | Op::Shift { v: a, .. }
        | Op::ShiftCarry { v: a, .. }
        | Op::TestBit { v: a, .. } => vec![a],
        Op::Load { addr, .. } => vec![addr],
        Op::Store { addr, value, .. } => vec![addr, value],
        _ => vec![],
    }
}

fn shift(kind: ShiftType, x: u32, n: u32) -> u32 {
    match kind {
        ShiftType::Lsl => x.checked_shl(n).unwrap_or(0),
        ShiftType::Lsr => x.checked_shr(n).unwrap_or(0),
        ShiftType::Asr => ((x as i32) >> n.min(31)) as u32,
        ShiftType::Ror => x.rotate_right(n),
    }
}

/// A number: a ROM pointer if it is in ROM.
fn num(img: &dyn Image, c: u32) -> Tok {
    if img.word(c).is_some() && !img.is_fn(c) {
        Tok::Rom(c)
    } else {
        Tok::Num(c)
    }
}

/// A number made by an op on a function's address (a hole already).
fn num_tok(img: &dyn Image, c: u32) -> Tok {
    if img.word(c).is_some() {
        Tok::Rom(c)
    } else {
        Tok::Num(c)
    }
}

fn bin_tok(img: &dyn Image, op: BinOp, x: Option<Tok>, y: Option<Tok>) -> R {
    use Form::*;
    use Tok::*;
    let arith = |a: u32, b: u32| match op {
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::And => a & b,
        BinOp::Or => a | b,
        BinOp::Xor => a ^ b,
        BinOp::Mul => a.wrapping_mul(b),
    };
    let val = |t: Option<Tok>| match t {
        Some(Num(c) | Rom(c)) => Some(c),
        Some(Fn { key, .. }) => Some(key),
        _ => None,
    };
    // The forbidden case's numeric result (the hole is recorded by the
    // caller).
    let hole = |why| R::Hole(why, val(x).zip(val(y)).map(|(a, b)| arith(a, b)));
    // Function forms first.
    if let (
        Some(Fn {
            form: fx,
            key: kx,
            origin: ox,
        }),
        y,
    )
    | (
        y,
        Some(Fn {
            form: fx,
            key: kx,
            origin: ox,
        }),
    ) = (x, y)
    {
        let on_left =
            matches!(x, Some(Fn { key, origin, form }) if key == kx && origin == ox && form == fx);
        let other = if on_left { y } else { x };
        let fnt = |form| {
            R::T(Fn {
                key: kx,
                origin: ox,
                form,
            })
        };
        return match (op, fx, other) {
            (BinOp::Sub, Full | Lo | Hi, _) => fnt(Cmp),
            (BinOp::Or, Full, Some(Num(1))) if kx & 1 == 1 => fnt(Full),
            (BinOp::Or | BinOp::Add, _, Some(Num(0))) | (BinOp::And, _, Some(Num(0xffff_ffff))) => {
                fnt(fx)
            }
            (BinOp::And, Full | Lo, Some(Num(0xffff))) => fnt(Lo),
            (
                BinOp::Or | BinOp::Add,
                HiUp,
                Some(Fn {
                    form: Lo,
                    key,
                    origin,
                }),
            ) => R::T(Fn {
                key,
                origin,
                form: Full,
            }),
            (BinOp::Or | BinOp::Add, Lo, Some(Fn { form: HiUp, .. })) => fnt(Full),
            // The join's shape with a side the run doesn't know (both
            // halves read back from RAM): the join, of whatever is there.
            (BinOp::Or | BinOp::Add, HiUp, None) => R::Unk,
            (BinOp::Or | BinOp::Add, Lo, None) => fnt(Full),
            _ => hole(Why::Arithmetic),
        };
    }
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        (Some(Rom(a)), None) | (None, Some(Rom(a))) if op == BinOp::Add => return R::T(RomFrom(a)),
        (Some(RomFrom(a)), None) | (None, Some(RomFrom(a))) if op == BinOp::Add => {
            return R::T(RomFrom(a));
        }
        _ => return R::Unk,
    };
    match (x, y) {
        (Num(a) | Rom(a), Num(b)) | (Num(a), Rom(b)) => R::T(Num(arith(a, b))),
        (Rom(a), Rom(b)) if op == BinOp::Sub => R::T(Num(a.wrapping_sub(b))),
        (RomFrom(a), Num(b)) | (Num(b), RomFrom(a)) if op == BinOp::Add => {
            let _ = b;
            R::T(RomFrom(a))
        }
        (RomFrom(a), Num(b)) if op == BinOp::Sub => {
            let p = a.wrapping_sub(b);
            if img.word(p).is_some() {
                R::T(RomFrom(p))
            } else {
                R::Unk
            }
        }
        (RomFrom(a), Num(m)) if op == BinOp::And && m >= 0xffff_0000 => R::T(RomFrom(a & m)),
        _ => R::Unk,
    }
}

/// Follow every function address in `fns` (all the program's functions,
/// over every code context) from its sources to its uses. `copies`: the
/// program start's copies of ROM data to RAM (source, destination, bytes).
pub fn follow(fns: &[FnIn], img: &dyn Image, copies: &[(u32, u32, u32)]) -> Flow {
    let mut g = G {
        loose_args: vec![Val::default(); 4],
        ..G::default()
    };
    // Functions some exit enters directly; the rest have callers the run
    // can't see (the reset, the IRQ handler, `call_fn`).
    let mut direct = BTreeSet::new();
    for f in fns {
        for b in f.blocks.values() {
            for o in b.exits.values() {
                match o.edge {
                    Edge::Call {
                        target: Some(k), ..
                    }
                    | Edge::Tail(Some(k)) => {
                        direct.insert(k);
                    }
                    _ => {}
                }
            }
        }
    }
    // The start's copies.
    for &(src, dst, len) in copies {
        let mut p = src & !3;
        while p + 4 <= src + len {
            if let Some(w) = img.word(p).filter(|&w| img.is_fn(w)) {
                g.flow.read.insert(p);
                let t = Tok::Fn {
                    key: w,
                    origin: Origin::Word(p),
                    form: Form::Full,
                };
                g.store(Store::Cell(dst + (p - src)), &[(t, 4)]);
                g.store(Store::Pile, &[(t, 4)]);
            }
            p += 4;
        }
    }
    let t0 = std::time::Instant::now();
    // A worklist of functions: each reruns when what it reads changed
    // (its entry, a callee's results, memory, what incomplete sets pass).
    let loads: BTreeSet<u32> = fns
        .iter()
        .filter(|f| {
            f.blocks
                .values()
                .any(|b| b.insts.iter().any(|i| matches!(i.op, Op::Load { .. })))
        })
        .map(|f| f.entry)
        .collect();
    let mut reads: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut dirty: BTreeSet<u32> = fns.iter().map(|f| f.entry).collect();
    let trace = std::env::var_os("GBAA_FNREF_TRACE").is_some();
    for round in 0..256 {
        g.changed = false;
        let loose: BTreeSet<u32> = fns
            .iter()
            .map(|f| f.entry)
            .filter(|k| !direct.contains(k) || g.taken.contains(k))
            .collect();
        for f in fns.iter().filter(|f| dirty.contains(&f.entry)) {
            let mut entry = g
                .entry
                .get(&f.entry)
                .cloned()
                .unwrap_or_else(|| vec![Val::default(); 16]);
            if loose.contains(&f.entry) {
                for (a, b) in entry.iter_mut().zip(&g.loose_args) {
                    a.join(b);
                }
                for r in &mut entry {
                    r.unk = true;
                }
            }
            for r in &mut entry[4..] {
                r.unk = true;
            }
            let mut run = Run {
                f,
                img,
                g: &mut g,
                loose: &loose,
                ins: BTreeMap::new(),
                work: BTreeSet::new(),
                trace,
                rets_read: BTreeSet::new(),
            };
            if f.blocks.contains_key(&f.entry) {
                run.ins.insert(
                    f.entry,
                    St {
                        regs: entry,
                        frame: BTreeMap::new(),
                    },
                );
                run.work.insert(f.entry);
            }
            let mut steps = 0;
            while let Some(k) = run.work.pop_first() {
                steps += 1;
                if steps > 64 * f.blocks.len() + 64 {
                    run.g.flow.unsettled = true;
                    break;
                }
                run.block(k);
            }
            let r = std::mem::take(&mut run.rets_read);
            reads.insert(f.entry, r);
        }
        // What every value held as a function's whole address: taken.
        let before = g.taken.len();
        let held: Vec<Val> = g
            .entry
            .values()
            .chain(g.ret.values())
            .flatten()
            .cloned()
            .chain(std::iter::once(Val {
                toks: g
                    .pile
                    .iter()
                    .chain(g.cells.values().flatten())
                    .map(|(t, _)| *t)
                    .collect(),
                ..Val::default()
            }))
            .collect();
        for v in &held {
            g.taken_add(v);
        }
        let called: Vec<u32> = g
            .flow
            .sites
            .values()
            .flat_map(|s| s.callees.iter().copied())
            .collect();
        for k in called {
            g.taken.insert(k);
        }
        let taken_changed = g.taken.len() != before;
        dirty = std::mem::take(&mut g.dirty_entry);
        let rets = std::mem::take(&mut g.dirty_ret);
        dirty.extend(
            reads
                .iter()
                .filter(|(_, r)| !r.is_disjoint(&rets))
                .map(|(f, _)| *f),
        );
        if std::mem::take(&mut g.dirty_mem) {
            dirty.extend(loads.iter().copied());
        }
        if std::mem::take(&mut g.dirty_loose) || taken_changed {
            dirty.extend(fns.iter().map(|f| f.entry).filter(|k| {
                loose.contains(k) || reads.get(k).is_some_and(|r| !r.is_disjoint(&loose))
            }));
        }
        if taken_changed {
            // The loose set grew: every function may see new callees.
            dirty.extend(fns.iter().map(|f| f.entry));
        }
        if trace {
            eprintln!(
                "fnref round {round}: {:?}; {} to rerun; pile {}, cells {}, taken {}, holes {}",
                t0.elapsed(),
                dirty.len(),
                g.pile.len(),
                g.cells.len(),
                g.taken.len(),
                g.flow.holes.len()
            );
        }
        if dirty.is_empty() {
            return g.flow;
        }
    }
    g.flow.unsettled = true;
    g.flow
}

impl Flow {
    /// The report's lines: the words (`words`: every function word in ROM,
    /// each with whether it is in a literal pool), the holes, the sites;
    /// `call_fn` the addresses of the calls printed through `call_fn`.
    pub fn report(&self, words: &BTreeMap<u32, (u32, bool)>, call_fn: &BTreeSet<u32>) -> String {
        let mut s = String::new();
        let proven = words.keys().filter(|w| self.proven(**w)).count();
        let pools = words.values().filter(|w| w.1).count();
        let unread = words.keys().filter(|w| !self.read.contains(w)).count();
        let mut by: BTreeMap<Why, usize> = BTreeMap::new();
        for w in words.keys() {
            if let Some((_, why)) = self.holes.get(&Origin::Word(*w)) {
                *by.entry(*why).or_default() += 1;
            }
        }
        let piled = words
            .keys()
            .filter(|w| !self.proven(**w) && self.piled.contains(&Origin::Word(**w)))
            .count();
        let calls: Vec<(&u32, &Site)> = self
            .sites
            .iter()
            .filter(|(a, _)| call_fn.contains(a))
            .collect();
        let complete = calls.iter().filter(|(_, s)| s.complete).count();
        writeln!(
            s,
            "function addresses (native/fnref.rs){}: {} function words in ROM ({pools} in literal pools, {} other data), {proven} proven, {} not: {unread} never read{} ({piled} of the {} not proven stored where the run can't place them, the heap's pile); {} code constants, {} proven; {} call_fn calls with a callee set, {complete} of them complete; {} other jumps by value to a function",
            if self.unsettled { " (didn't settle: nothing proven)" } else { "" },
            words.len(),
            words.len() - pools,
            words.len() - proven,
            by.iter()
                .map(|(w, n)| format!(", {n} {}", w.what()))
                .collect::<String>(),
            words.len() - proven,
            self.consts.len(),
            self.consts.keys().filter(|o| !self.holes.contains_key(o)).count(),
            calls.len(),
            self.sites.len() - calls.len(),
        )
        .unwrap();
        for (w, (k, pool)) in words {
            let kind = if *pool { "pool" } else { "data" };
            let piled = if self.piled.contains(&Origin::Word(*w)) {
                " (stored to the pile)"
            } else {
                ""
            };
            let what = if self.proven(*w) {
                "proven".to_string()
            } else if let Some((at, why)) = self.holes.get(&Origin::Word(*w)) {
                format!("not proven: {} at {at:#010x}{piled}", why.what())
            } else {
                "not proven: never read".to_string()
            };
            writeln!(s, "  word {w:#010x} ({kind}) = f_{k:08x}: {what}").unwrap();
        }
        for (o, k) in &self.consts {
            let Origin::Code(b, i) = o else { continue };
            let what = match self.holes.get(o) {
                Some((at, why)) => format!("not proven: {} at {at:#010x}", why.what()),
                None => "proven".into(),
            };
            writeln!(
                s,
                "  constant in block {b:#010x} inst {i} = f_{k:08x}: {what}"
            )
            .unwrap();
        }
        for (at, site) in &self.sites {
            let kind = if call_fn.contains(at) {
                "call_fn call"
            } else if site.tail {
                "jump by value"
            } else {
                "call by value"
            };
            writeln!(
                s,
                "  {kind} at {at:#010x}: {} {{{}}}",
                if site.complete {
                    "complete"
                } else {
                    "incomplete"
                },
                site.callees
                    .iter()
                    .map(|k| format!("f_{k:08x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .unwrap();
        }
        let missing: Vec<String> = call_fn
            .iter()
            .filter(|a| !self.sites.contains_key(a))
            .map(|a| format!("{a:#010x}"))
            .collect();
        if !missing.is_empty() {
            writeln!(
                s,
                "  call_fn calls the run didn't reach (no callee set): {}",
                missing.join(" ")
            )
            .unwrap();
        }
        s
    }
}

#[cfg(test)]
mod tests;
