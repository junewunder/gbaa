//! Function addresses followed to every use (`native/fnref-flow`;
//! `docs/native-stage-b.md`, piece 3; `docs/native-port-design.md`,
//! section 4, "Function pointers").
//!
//! Dense `FnRef` IDs (`native/fnref-dense`) rewrite each word that holds a
//! function's address. That is only right where every value the word
//! enters is used as section 4 allows: called, stored (and loaded back),
//! compared for equality as a whole (with another, 0, or a value the
//! closed world says is none: never a half, whose top is 0 when dense,
//! nor a small constant, which may be a dense ID), split into halves and
//! joined again, each half only with the other (`>> 16`,
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
//! saved register's slot reads as unknown, the register as it came);
//! RAM words at constant addresses, one cell each; **objects**
//! (`native/fnref-heap-sites`): each function's frame (its sp at entry
//! is a pointer to it, [`Tok::Obj`]) and what each call of a named
//! allocator returns (`allocators` in the holes file), each with a cell
//! per offset and one for anywhere in it; and one pile for every store
//! the run can't place. An escaping slot is its frame's cell; a stack
//! argument is the caller's, through the sp it called with. A pointer
//! into an object stays one through `+`/`-` (an unknown index: anywhere
//! in it), `|`, `^`, a mask down to an alignment and its pieces (bytes,
//! halves); any other op makes a number of it (the closed world, as for
//! ROM pointers), so an address the run doesn't know is no object's,
//! unless a pointer into one was dropped ([`Val::lost_obj`]: then every
//! object). A load through a pointer into an object reads its cell there,
//! its anywhere cell and what lost pointers stored; one at a constant
//! address its cell and the pile; one the run can't place, the pile and
//! every RAM cell (every object's too, if lost), or, where the value-set
//! analysis bounds it to ROM, the words there. An object doesn't overlap
//! a constant address the code reads (the stack, the heap). Interprocedural: a call passes r0-r3 to its callee's entry
//! and takes back what its returns hold; a tail call or a fall passes
//! every register. An indirect call passes them to each function whose
//! whole address its target may hold. A call whose target holds none is
//! an incomplete callee set: its arguments go to every function that may
//! be called that way (one with no direct caller, or whose address is
//! taken), and its results come back from all of them. Functions rerun
//! (a worklist, by rounds) until nothing they read changes: their entry,
//! a callee's results, and of memory only what their loads gave (the pile
//! or every cell at a width, compared with what it gave at the round's
//! start; the cells at the addresses they read). The memory reads are
//! kept and grown store by store, and a call through a large callee set
//! gives only what it hadn't before, the callees that held the same set
//! sharing what it joins to. On Metroid (2,616 functions) the run
//! settles in 12 rounds, about 150 s on quack
//! (`native/fnref-flow-scale`), holding about 30 million tokens at a
//! peak of 1.35 GB. A run that holds more than [`TOKENS`] stops,
//! unsettled (Kirby - Nightmare in Dream Land and Minish Cap, at about
//! 3,000 functions run: `native/whole-lift-memory`).
//!
//! **The closed world.** Function and ROM addresses enter values only
//! from the sources above; a number made by arithmetic the run doesn't
//! follow, or read from I/O, is never one. So an unknown value is never a
//! function's address, and the whole addresses a target may hold are its
//! complete callee set (a piece of one called is a hole of its origin),
//! unless the run dropped one on the way ([`Val::lost_rom`]): a "from here
//! on" ROM read cut short at 256 KB, a ROM-bounded read too wide to make,
//! or a ROM pointer less an unknown. A target that may hold such a value
//! has an incomplete callee set.
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
//! Precision (`docs/native-stage-b.md`, "Known gaps"): splitting the
//! pile into objects proved no more words on the agb programs (measured
//! with no pile at all, too), nor did running each function once per
//! call site (`native/fnref-context`, k = 1, [`Ix`]; on affine 5 s
//! became 21 s with direct calls only, 445 s with each `call_fn`
//! callee's too). What holds them: a load whose address may be unknown
//! (affine: about 640 load sites, the same with the allocator named, the
//! pile left out of constant-address loads, or the I/O flag left off)
//! reads the pile and every RAM cell, so the IRQ vector's handler
//! address, among others, reaches almost every hole. Pointers the agb
//! allocator makes by arithmetic (its free list in `GLOBAL_ALLOC`) are
//! unknown.

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
    /// A pointer into an object ([`ObjId`], by its index in
    /// [`G::objs`]) at this offset, or anywhere in it ([`OBJ_ANY`]).
    Obj(u32, i32),
}

/// What an [`Tok::Obj`] points into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ObjId {
    /// A function instance's frame ([`Ix`]): offsets from its entry sp.
    Frame(Ix),
    /// What an allocator's call at this instruction returns.
    Heap(u32),
}

/// An offset somewhere in the object.
const OBJ_ANY: i32 = i32::MAX;
/// Offsets kept exact: further from 0 is anywhere in the object.
const OBJ_WIN: i32 = 0x7ff0;
/// More exact offsets than this into one object: anywhere in it.
const OBJ_OFFS: usize = 8;
/// Where the objects' cells are kept, among RAM's: each object a 64 KB
/// window above here (no RAM address, nor any the program makes).
const OBJ_BASE: u32 = 0x1000_0000;
/// The cell of stores through a lost pointer into an object
/// ([`Val::lost_obj`]): every object's load reads it, as does every load
/// from RAM the run can't place ([`G::read_cells`]).
const LOST_CELL: u32 = OBJ_BASE - 0x10;
/// The most objects: past it, a new one's pointers are lost.
const OBJ_MAX: u32 = 0x6000;

/// The cell an object's offset is kept at.
fn obj_addr(i: u32, off: i32) -> u32 {
    let at = if off == OBJ_ANY {
        0xfff8
    } else {
        (off + 0x8000) as u32
    };
    OBJ_BASE + (i << 16) + at
}

/// An exact offset, or anywhere if out of the window.
fn obj_off(off: i64) -> i32 {
    if off.abs() < OBJ_WIN as i64 {
        off as i32
    } else {
        OBJ_ANY
    }
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
    /// Maybe a function address, or a ROM pointer to some, that the run
    /// dropped: a ROM read cut short or too wide to make, or arithmetic on
    /// a ROM pointer it gave up on. A call through it has an incomplete
    /// callee set, whatever whole addresses it holds besides.
    lost_rom: bool,
    /// Maybe a pointer into an object the run dropped (arithmetic it
    /// doesn't follow, a pointer stored in pieces): used as an address,
    /// it may reach every object. Set only with [`Val::unk`].
    lost_obj: bool,
}

thread_local! {
    /// The code addresses of the run's image, sorted, for [`bound`]
    /// (empty outside [`follow`]: every address is then one run of data).
    static CODE: std::cell::RefCell<std::rc::Rc<Vec<u32>>> = std::cell::RefCell::default();
}

/// Where a read "from here on" at `a` stops: [`Image::bound`] of the run's
/// image, for [`Val::cap`], which has no image at hand.
fn bound(a: u32) -> u32 {
    CODE.with(|c| {
        let c = c.borrow();
        if c.is_empty() {
            return u32::MAX;
        }
        c.get(c.partition_point(|&k| k <= a))
            .copied()
            .unwrap_or(u32::MAX)
    })
}

/// Sets [`CODE`] for a run; the one before comes back when it's dropped.
struct CodeGuard(std::rc::Rc<Vec<u32>>);

impl CodeGuard {
    fn set(img: &dyn Image) -> CodeGuard {
        // Every code address after the start of ROM, by stepping the
        // image's bound (up to its last, past which `bound` is the end).
        let mut v = Vec::new();
        let mut a = 0x0800_0000u32;
        loop {
            let b = img.bound(a);
            if b <= a || img.word(b).is_none() {
                v.push(b);
                break;
            }
            v.push(b);
            a = b;
        }
        CodeGuard(CODE.with(|c| c.replace(std::rc::Rc::new(v))))
    }
}

impl Drop for CodeGuard {
    fn drop(&mut self) {
        CODE.with(|c| *c.borrow_mut() = self.0.clone());
    }
}

/// The most tokens the run holds (each distinct set once, and memory's)
/// before it stops, unsettled, as when its rounds run out: proving
/// nothing then, as a lift without the flow (`skip_fn_flow`). Metroid
/// settles holding about 30 million at a peak of 1.4 GB; Kirby -
/// Nightmare in Dream Land and Minish Cap held over 100 million in a
/// round and a half, past 15 GB, before (`native/whole-lift-memory`).
const TOKENS: usize = 48_000_000;

/// More numbers than this in a value: it's unknown.
const NUMS: usize = 8;
/// More exact ROM pointers than this: "from the lowest on".
const ROMS: usize = 16;
/// The most ROM bytes one read by an unknown index reads (a "from here
/// on" pointer up to the next code, or a ROM range the value-set analysis
/// bounds); past it the read is cut short and the value marked
/// [`Val::lost_rom`]. The agb programs' data runs to about 110 KB; reading
/// it all costs about a second of each lift.
const ROM_READ: u32 = 0x4_0000;

impl Val {
    fn unk() -> Val {
        Val {
            unk: true,
            ..Val::default()
        }
    }

    /// Unknown, maybe a pointer into any object.
    fn lost() -> Val {
        Val {
            unk: true,
            lost_obj: true,
            ..Val::default()
        }
    }

    fn has_obj(&self) -> bool {
        self.lost_obj || self.toks.range(Tok::Obj(0, i32::MIN)..).next().is_some()
    }

    fn of(t: Tok) -> Val {
        Val {
            toks: [t].into_iter().collect(),
            ..Val::default()
        }
    }

    /// Join without widening (as tokens read from memory one by one).
    fn union(&mut self, o: &Val) {
        if self.toks.is_empty() {
            self.toks = o.toks.clone();
        } else if !std::rc::Rc::ptr_eq(&self.toks.0, &o.toks.0) && !o.toks.is_subset(&self.toks) {
            self.toks.extend(o.toks.iter().copied());
        }
        self.mem |= o.mem;
        self.unk |= o.unk;
        self.lost_rom |= o.lost_rom;
        self.lost_obj |= o.lost_obj;
    }

    fn join(&mut self, o: &Val) -> bool {
        let before = (
            self.toks.len(),
            self.mem,
            self.unk,
            self.lost_rom,
            self.lost_obj,
        );
        if self.toks.is_empty() {
            self.toks = o.toks.clone();
        } else if !std::rc::Rc::ptr_eq(&self.toks.0, &o.toks.0) && !o.toks.is_subset(&self.toks) {
            self.toks.extend(o.toks.iter().copied());
        }
        self.mem |= o.mem;
        self.unk |= o.unk;
        self.lost_rom |= o.lost_rom;
        self.lost_obj |= o.lost_obj;
        self.cap();
        before
            != (
                self.toks.len(),
                self.mem,
                self.unk,
                self.lost_rom,
                self.lost_obj,
            )
    }

    /// Widen: too many numbers are unknown, too many ROM pointers are
    /// one "from the lowest on" per run of data (a loop walking a table),
    /// and a pointer "from here on" takes in those above it in its run of
    /// data. A read "from here on" stops at the next code
    /// ([`Image::bound`]), so a pointer past it stays its own (else the
    /// words it reads would go unread: `native/fnref-romfrom-absorb`).
    fn cap(&mut self) {
        // One pass to see what needs widening; the set (shared) is only
        // changed when something does.
        let (mut nums, mut roms, mut froms) = (0, 0, false);
        // Objects with an offset anywhere and more exact ones, or too many.
        let mut wide: Vec<u32> = Vec::new();
        let mut obj: Option<(u32, usize, bool)> = None;
        // Functions' tokens sort first: only those after them count.
        for t in self.toks.range(Tok::Num(0)..) {
            match *t {
                Tok::Num(_) => nums += 1,
                Tok::Rom(_) => roms += 1,
                Tok::RomFrom(_) => froms = true,
                Tok::Obj(i, off) => {
                    let (n, any) = match obj {
                        Some((j, n, any)) if j == i => (n + 1, any || off == OBJ_ANY),
                        _ => (1, off == OBJ_ANY),
                    };
                    // Sorted by offset, anywhere last.
                    if ((any && n > 1) || n > OBJ_OFFS) && wide.last() != Some(&i) {
                        wide.push(i);
                    }
                    obj = Some((i, n, any));
                }
                _ => {}
            }
        }
        for i in wide {
            self.drop_range(Tok::Obj(i, i32::MIN)..=Tok::Obj(i, OBJ_ANY));
            self.toks.insert(Tok::Obj(i, OBJ_ANY));
        }
        if nums > NUMS {
            self.unk = true;
        }
        // Unknown takes in every number (so the cap stays monotone).
        if self.unk && nums > 0 {
            self.drop_range(Tok::Num(0)..=Tok::Num(u32::MAX));
        }
        if roms > ROMS {
            // The lowest of each run of data.
            let mut los: Vec<u32> = Vec::new();
            let mut end = 0;
            for t in self.toks.range(Tok::Rom(0)..=Tok::Rom(u32::MAX)) {
                if let Tok::Rom(a) = *t
                    && a >= end
                {
                    los.push(a);
                    end = bound(a);
                }
            }
            self.drop_range(Tok::Rom(0)..=Tok::Rom(u32::MAX));
            for lo in los {
                self.toks.insert(Tok::RomFrom(lo));
            }
            froms = true;
        }
        if !froms {
            return;
        }
        // A pointer "from here on" takes in those above it, up to the end
        // of its run of data.
        let los: Vec<u32> = self
            .toks
            .range(Tok::RomFrom(0)..)
            .filter_map(|t| match *t {
                Tok::RomFrom(a) => Some(a),
                _ => None,
            })
            .collect();
        let mut covered = 0;
        for lo in los {
            if lo < covered {
                continue;
            }
            let end = bound(lo);
            covered = end;
            let hi = Tok::Rom(lo)..Tok::Rom(end);
            let hf = Tok::RomFrom(lo.saturating_add(1))..Tok::RomFrom(end);
            if end > lo
                && (self.toks.range(hi.clone()).next().is_some()
                    || self.toks.range(hf.clone()).next().is_some())
            {
                self.drop_range(hi);
                self.drop_range(hf);
            }
        }
    }

    /// Take out the tokens in `r` (the functions' tokens, sorted first,
    /// are most of a large set: a `retain` over them all at each cap was
    /// most of a large game's run).
    fn drop_range(&mut self, r: impl std::ops::RangeBounds<Tok>) {
        let ts: Vec<Tok> = self.toks.range(r).copied().collect();
        if !ts.is_empty() {
            for t in ts {
                self.toks.remove(&t);
            }
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
    Outside,
    UserBank,
}

impl Why {
    /// Not a forbidden use: the value went where the run doesn't follow.
    pub fn lost(self) -> bool {
        matches!(self, Why::Bios | Why::UserBank | Why::Outside)
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
            Why::Outside => "stored outside RAM: I/O, display memory, SRAM (lost)",
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
    /// The origins stored where the run can't place them (the pile: not a
    /// constant address, nor a pointer into an object).
    pub piled: BTreeSet<Origin>,
    /// The run didn't settle: nothing is proven.
    pub unsettled: bool,
    /// It stopped at [`TOKENS`]: the tokens it held, after how many
    /// functions run.
    pub over: Option<(usize, usize)>,
}

impl Flow {
    /// Whether ROM word `w`'s function address is proven.
    pub fn proven(&self, w: u32) -> bool {
        !self.unsettled && self.read.contains(&w) && !self.holes.contains_key(&Origin::Word(w))
    }
}

type Mem = BTreeSet<(Tok, u8)>;

/// A function instance (`native/fnref-context`): its entry in the low
/// half, its context in the high: the call (direct, or each callee of an
/// indirect one) it is that call's callee for (k = 1), or 0 for every
/// other caller (an incomplete set, a function the run can't see called).
/// A fall or a tail jump keeps its caller's context. `k as Ix` is `k`'s
/// instance 0. Each instance has its own frame object and spills.
type Ix = u64;

fn ix(entry: u32, ctx: u32) -> Ix {
    (ctx as Ix) << 32 | entry as Ix
}

fn entry_of(i: Ix) -> u32 {
    i as u32
}

fn ctx_of(i: Ix) -> u32 {
    (i >> 32) as u32
}

#[derive(Default)]
struct G {
    /// Each instance's entry registers and results.
    entry: BTreeMap<Ix, Vec<Val>>,
    ret: BTreeMap<Ix, Vec<Val>>,
    /// Every store the run can't place (the heap, escaping frame slots).
    pile: Mem,
    /// [`G::read_pile`]'s, by width: uncapped (grown at each store), and
    /// capped (made again when the first changes).
    pile_raw: [Val; 5],
    pile_reads: [Option<Val>; 5],
    /// RAM words at constant addresses, and the objects' cells (above
    /// [`OBJ_BASE`]).
    cells: BTreeMap<u32, Mem>,
    /// [`G::read_cells`]'s, by width, the same way (RAM's cells only).
    cell_raw: [Val; 5],
    cell_reads: [Option<Val>; 5],
    /// [`G::read_objs`]'s, by width, the same way (the objects' cells).
    obj_raw: [Val; 5],
    obj_reads: [Option<Val>; 5],
    /// The objects, by index in [`Tok::Obj`], and back.
    objs: Vec<ObjId>,
    obj_ix: std::collections::HashMap<ObjId, u32, Fx>,
    /// The allocators: a direct call to one returns a new object.
    allocs: BTreeSet<u32>,
    /// The functions run once per call site ([`Ix`]).
    by_site: BTreeSet<u32>,
    spills: BTreeMap<Ix, Mem>,
    /// What a load at a width reads of a function's spills, and of the
    /// cells at an address (kept until they change).
    spill_reads: std::collections::HashMap<(Ix, u8), Val, Fx>,
    cell_at_reads: std::collections::HashMap<(u32, u8), Val, Fx>,
    /// The origins in [`Flow::holes`], to look up fast.
    holed: std::collections::HashSet<Origin, Fx>,
    /// Large token sets whose every origin is in [`Flow::holes`], by
    /// address (each kept, so the address isn't reused): a hole of one
    /// again adds nothing.
    all_holed: std::collections::HashMap<*const BTreeSet<Tok>, Toks, Fx>,
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
    dirty_entry: BTreeSet<Ix>,
    dirty_ret: BTreeSet<Ix>,
    dirty_loose: bool,
    /// The functions incomplete callee sets reach this round, and what
    /// their results join to (kept until one of them changes).
    loose: BTreeSet<Ix>,
    loose_rets: Option<Vec<Val>>,
    /// When each function's results last changed (a count of changes).
    ret_stamp: BTreeMap<Ix, u64>,
    stamp: u64,
    /// Each call by value's last callees, arguments and results, by
    /// function, address and whether a tail call: a large callee set given
    /// the same arguments again changes nothing, and its results only
    /// when a callee's did.
    by_value: std::collections::HashMap<(Ix, u32, bool), ByValue>,
    /// The cells changed this round, and whether the pile did.
    cells_changed: BTreeSet<u32>,
    pile_changed: bool,
    /// What the running function read of memory.
    rd: Rd,
}

/// A fast hash for the run's small keys (the standard one was a tenth of
/// a large game's run).
#[derive(Clone, Copy, Default)]
struct Fx;

struct FxHasher(u64);

impl std::hash::BuildHasher for Fx {
    type Hasher = FxHasher;
    fn build_hasher(&self) -> FxHasher {
        FxHasher(0)
    }
}

impl std::hash::Hasher for FxHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }
    fn write_u8(&mut self, n: u8) {
        self.write_u64(n as u64);
    }
    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }
    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
    fn write_isize(&mut self, n: isize) {
        self.write_u64(n as u64);
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

struct ByValue {
    callees: BTreeSet<u32>,
    args: Vec<Val>,
    rets: Vec<Val>,
    stamp: u64,
}

/// What a function's run read of memory, to rerun it only when that
/// changed: the pile and every cell at some widths (bit `w`), and cells
/// in ranges (inclusive).
#[derive(Default)]
struct Rd {
    pile: u8,
    all_cells: u8,
    all_objs: u8,
    ranges: Vec<(u32, u32)>,
    /// It made a call whose callee set is incomplete (it reads every
    /// loose function's results).
    incomplete: bool,
}

impl G {
    /// The instance of `k` a direct call (or a fall) with context `ctx`
    /// enters.
    fn ix_call(&self, k: u32, ctx: u32) -> Ix {
        if self.by_site.contains(&k) {
            ix(k, ctx)
        } else {
            k as Ix
        }
    }

    fn mark(&mut self, c: bool) {
        self.changed |= c;
    }

    /// A pointer to object `o` at `off`; past [`OBJ_MAX`] objects, lost.
    fn obj(&mut self, o: ObjId, off: i32) -> Val {
        let i = match self.obj_ix.get(&o) {
            Some(&i) => i,
            None if (self.objs.len() as u32) < OBJ_MAX => {
                let i = self.objs.len() as u32;
                self.objs.push(o);
                self.obj_ix.insert(o, i);
                i
            }
            None => return Val::lost(),
        };
        Val::of(Tok::Obj(i, off))
    }

    /// A pointer the run follows into the frame of the function it is
    /// in at or above the entry sp: the caller's frame, where it can't
    /// say which (only a function's own sp-relative accesses there are
    /// followed, [`Run::sp_in`]).
    fn obj_up(&self, i: u32, off: i32) -> bool {
        matches!(self.objs[i as usize], ObjId::Frame(_)) && off != OBJ_ANY && off >= 0
    }

    /// What a load at width `w` through object `i` at `off` reads: its
    /// cell there and its cell for anywhere (or, anywhere, all its
    /// cells), and what stores through lost pointers left. Not the pile:
    /// by the closed world, an address the run doesn't know is no
    /// object's unless it is a lost pointer into one.
    fn read_obj(&mut self, i: u32, off: i32, w: u8, into: &mut Val) {
        if off == OBJ_ANY {
            let (lo, hi) = (obj_addr(i, -OBJ_WIN), obj_addr(i, OBJ_ANY) + 3);
            self.rd.ranges.push((lo, hi));
            for (_, c) in self.cells.range(lo..=hi) {
                from_mem(c, w, into);
            }
        } else {
            let c = obj_addr(i, off);
            self.rd.ranges.push((c.saturating_sub(3), c + w as u32 - 1));
            self.read_cell(c, w, into);
            let a = obj_addr(i, OBJ_ANY);
            self.rd.ranges.push((a, a));
            self.read_cell(a, w, into);
        }
        self.rd.ranges.push((LOST_CELL, LOST_CELL));
        self.read_cell(LOST_CELL, w, into);
    }

    /// What a load at width `w` reads of every object's cells (kept as
    /// [`G::read_cells`] keeps RAM's).
    fn read_objs(&mut self, w: u8, into: &mut Val) {
        let i = w as usize;
        self.rd.all_objs |= 1 << w;
        if self.obj_reads[i].is_none() {
            let mut v = self.obj_raw[i].clone();
            v.cap();
            self.obj_reads[i] = Some(v);
        }
        into.join(self.obj_reads[i].as_ref().unwrap());
    }

    fn hole(&mut self, v: &Val, at: u32, why: Why, only: impl Fn(Form) -> bool) {
        let big = v.toks.len() >= 64;
        let p = std::rc::Rc::as_ptr(&v.toks.0);
        if big && self.all_holed.contains_key(&p) {
            return;
        }
        let mut all = true;
        for (_, o, form) in v.fns() {
            if self.holed.contains(&o) {
                continue;
            }
            // An output only: nothing the run reads.
            if only(form) {
                self.holed.insert(o);
                self.flow.holes.insert(o, (at, why));
            } else {
                all = false;
            }
        }
        if big && all {
            if self.all_holed.len() >= 1024 {
                self.all_holed.clear();
            }
            self.all_holed.insert(p, v.toks.clone());
        }
    }

    /// What a load at width `w` reads of function `f`'s spills.
    fn read_spill(&mut self, f: Ix, w: u8, into: &mut Val) {
        let v = self.spill_reads.entry((f, w)).or_insert_with(|| {
            let mut v = Val::default();
            if let Some(m) = self.spills.get(&f) {
                from_mem(m, w, &mut v);
            }
            v
        });
        into.union(v);
    }

    /// What a load at width `w` from RAM address `c` reads of the cells
    /// it overlaps.
    fn read_cell(&mut self, c: u32, w: u8, into: &mut Val) {
        let v = self.cell_at_reads.entry((c, w)).or_insert_with(|| {
            let mut v = Val::default();
            for (&a, m) in self.cells.range(c.saturating_sub(3)..c + w as u32) {
                for &(t, sw) in m {
                    if c < a + sw as u32 {
                        from_mem_one(t, sw, w, &mut v);
                    }
                }
            }
            v
        });
        into.union(v);
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
        let new: Vec<(Tok, u8)> = toks.iter().copied().filter(|t| m.insert(*t)).collect();
        let c = !new.is_empty();
        if c {
            let raw = match to {
                Store::Pile => {
                    self.pile_reads = Default::default();
                    self.pile_changed = true;
                    Some(&mut self.pile_raw)
                }
                Store::Cell(a) => {
                    let objs = a >= OBJ_BASE;
                    if objs {
                        self.obj_reads = Default::default();
                    } else {
                        self.cell_reads = Default::default();
                    }
                    // The kept reads of the addresses this cell overlaps.
                    for c in a.saturating_sub(4)..=a + 3 {
                        for w in [1u8, 2, 4] {
                            if let Some(v) = self.cell_at_reads.get_mut(&(c, w)) {
                                for &(t, sw) in &new {
                                    if a + 3 >= c && a < c + w as u32 && c < a + sw as u32 {
                                        from_mem_one(t, sw, w, v);
                                    }
                                }
                            }
                        }
                    }
                    self.cells_changed.insert(a);
                    Some(if objs {
                        &mut self.obj_raw
                    } else {
                        &mut self.cell_raw
                    })
                }
                Store::Spill(f) => {
                    self.dirty_entry.insert(f);
                    for w in [1, 2, 4] {
                        if let Some(v) = self.spill_reads.get_mut(&(f, w)) {
                            for &(t, sw) in &new {
                                from_mem_one(t, sw, w, v);
                            }
                        }
                    }
                    None
                }
            };
            if let Some(raw) = raw {
                for w in [1, 2, 4] {
                    for &(t, sw) in &new {
                        from_mem_one(t, sw, w, &mut raw[w as usize]);
                    }
                }
            }
        }
        self.mark(c);
    }

    /// What a load at width `w` reads of the pile (kept until the pile
    /// changes).
    fn read_pile(&mut self, w: u8, into: &mut Val) {
        let i = w as usize;
        self.rd.pile |= 1 << w;
        if self.pile_reads[i].is_none() {
            let mut v = self.pile_raw[i].clone();
            v.cap();
            self.pile_reads[i] = Some(v);
        }
        into.join(self.pile_reads[i].as_ref().unwrap());
    }

    /// What a load at width `w` from anywhere reads of the cells (kept
    /// until a cell changes: reading three thousand of them at each such
    /// load was most of a large game's run).
    fn read_cells(&mut self, w: u8, into: &mut Val) {
        let i = w as usize;
        self.rd.all_cells |= 1 << w;
        if self.cell_reads[i].is_none() {
            let mut v = self.cell_raw[i].clone();
            v.cap();
            self.cell_reads[i] = Some(v);
        }
        into.join(self.cell_reads[i].as_ref().unwrap());
    }

    fn give(&mut self, f: Ix, regs: &[Val]) {
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

    /// [`G::give`] the same registers to each of `ks` (a call by value's
    /// callees). Callees whose entry register held the same set before
    /// share the set it joins to: a join per callee made each its own copy
    /// of a large set, thousands of them on a large game (Minish Cap and
    /// Nightmare ran past 15 GB; `native/whole-lift-memory`).
    fn give_all(&mut self, ks: &BTreeSet<Ix>, regs: &[Val]) {
        if ks.len() < 2 {
            for &k in ks {
                self.give(k, regs);
            }
            return;
        }
        // By register and the set it held (kept alive here, so its
        // address isn't reused) with its flags: what it joined to.
        #[allow(clippy::type_complexity)]
        let mut memo: std::collections::HashMap<
            (usize, *const BTreeSet<Tok>, bool, bool, bool, bool),
            (Toks, Val),
            Fx,
        > = Default::default();
        let mut c_any = false;
        for &f in ks {
            let e = self
                .entry
                .entry(f)
                .or_insert_with(|| vec![Val::default(); 16]);
            let mut c = false;
            for (i, (a, b)) in e.iter_mut().zip(regs).enumerate() {
                if a.toks.is_empty() {
                    c |= a.join(b);
                    continue;
                }
                let key = (
                    i,
                    std::rc::Rc::as_ptr(&a.toks.0),
                    a.mem,
                    a.unk,
                    a.lost_rom,
                    a.lost_obj,
                );
                let before = (a.toks.len(), a.mem, a.unk, a.lost_rom, a.lost_obj);
                match memo.get(&key) {
                    Some((_, v)) => *a = v.clone(),
                    None => {
                        let old = a.toks.clone();
                        a.join(b);
                        memo.insert(key, (old, a.clone()));
                    }
                }
                c |= before != (a.toks.len(), a.mem, a.unk, a.lost_rom, a.lost_obj);
            }
            if c {
                self.dirty_entry.insert(f);
            }
            c_any |= c;
        }
        self.mark(c_any);
    }

    /// The tokens the run holds, by store: each distinct set once (as the
    /// trace prints them, and [`TOKENS`] bounds their sum), and the
    /// memory's (the pile, cells and spills).
    fn held(&self) -> Vec<(&'static str, usize, usize)> {
        let mut seen = std::collections::HashSet::new();
        let mut count = |name, vs: &mut dyn Iterator<Item = &Toks>| {
            let (mut n, mut t) = (0usize, 0usize);
            for v in vs {
                if seen.insert(std::rc::Rc::as_ptr(&v.0)) {
                    n += 1;
                    t += v.len();
                }
            }
            (name, n, t)
        };
        let mems: usize = self
            .cells
            .values()
            .chain(self.spills.values())
            .map(|m| m.len())
            .sum();
        vec![
            count("entry", &mut self.entry.values().flatten().map(|v| &v.toks)),
            count("ret", &mut self.ret.values().flatten().map(|v| &v.toks)),
            count("rom_from", &mut self.rom_from.values().map(|v| &v.toks)),
            count("rom_hull", &mut self.rom_hull.values().map(|v| &v.toks)),
            count(
                "by_value",
                &mut self
                    .by_value
                    .values()
                    .flat_map(|b| b.args.iter().chain(&b.rets))
                    .map(|v| &v.toks),
            ),
            count(
                "spill_reads",
                &mut self.spill_reads.values().map(|v| &v.toks),
            ),
            count(
                "cell_at_reads",
                &mut self.cell_at_reads.values().map(|v| &v.toks),
            ),
            count("all_holed", &mut self.all_holed.values()),
            ("memory", 0, self.pile.len() + mems),
        ]
    }

    fn give_ret(&mut self, f: Ix, regs: &[Val]) {
        let e = self.ret.entry(f).or_insert_with(|| vec![Val::default(); 4]);
        let mut c = false;
        for (a, b) in e.iter_mut().zip(regs) {
            c |= a.join(b);
        }
        if c {
            self.dirty_ret.insert(f);
            self.stamp += 1;
            self.ret_stamp.insert(f, self.stamp);
            if self.loose.contains(&f) {
                self.loose_rets = None;
            }
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
    Spill(Ix),
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

/// A RAM address without its mirror: EWRAM repeats every 256 KB, IWRAM
/// every 32 KB.
fn ram_canon(a: u32) -> u32 {
    match a >> 24 {
        2 => 0x0200_0000 | (a & 0x3_ffff),
        _ => 0x0300_0000 | (a & 0x7fff),
    }
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
            Tok::Rom(_) | Tok::RomFrom(_) | Tok::Obj(..) if w == 4 => out.push((t, 4)),
            // A piece of a pointer into an object: of the object still.
            Tok::Obj(i, _) => out.push((Tok::Obj(i, OBJ_ANY), w)),
            _ => out.push((OTHER, w)),
        }
    }
    // Anything else stored there, marked (so a load of memory nothing
    // else was stored to is exactly what was).
    if v.unk || v.mem || v.toks.is_empty() {
        out.push((OTHER, w));
    }
    if v.lost_rom {
        out.push((LOST, w));
    }
    if v.lost_obj {
        out.push((LOST_OBJ, w));
    }
    (out, bad)
}

/// In memory: something stored that isn't a function's or ROM address.
const OTHER: Tok = Tok::Num(0);
/// In memory: a value with [`Val::lost_rom`] stored (no other number is
/// kept in memory: [`to_mem`] makes each [`OTHER`]).
const LOST: Tok = Tok::Num(1);
/// In memory: a value with [`Val::lost_obj`] stored, or a piece of a
/// pointer into an object.
const LOST_OBJ: Tok = Tok::Num(2);

/// What a load at width `w` reads of memory `m`.
fn from_mem(m: &Mem, w: u8, into: &mut Val) {
    for &(t, sw) in m {
        from_mem_one(t, sw, w, into);
    }
}

/// What a load at width `w` reads of token `t` stored at width `sw`.
fn from_mem_one(t: Tok, sw: u8, w: u8, into: &mut Val) {
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
        Tok::Rom(_) | Tok::RomFrom(_) | Tok::Obj(..) if w == 4 && sw == 4 => t,
        // A piece of a pointer into an object, or pieces of one read as a
        // word: of the object still.
        Tok::Obj(i, _) => {
            into.unk = true;
            Tok::Obj(i, OBJ_ANY)
        }
        _ => {
            // A mark, not a value: nothing else came of it.
            if t == LOST {
                into.lost_rom = true;
                return;
            }
            if t == LOST_OBJ {
                into.lost_obj = true;
                into.unk = true;
                return;
            }
            into.mem = true;
            return;
        }
    };
    into.toks.insert(t);
}

struct Run<'a, 'g> {
    f: &'a FnIn<'a>,
    img: &'a dyn Image,
    g: &'g mut G,
    /// Functions any incomplete callee set may reach.
    loose: &'a BTreeSet<Ix>,
    ins: BTreeMap<u32, St>,
    /// Blocks to run, lowest address first (near enough reverse
    /// postorder for compiled code, so loops settle in fewer passes).
    work: BTreeSet<u32>,
    trace: bool,
    /// The functions whose results the run read.
    rets_read: BTreeSet<Ix>,
    /// The instance being run.
    ix: Ix,
    /// The instruction being run.
    at: u32,
    /// Its own frame's object (`None`: past [`OBJ_MAX`], the pile).
    me: Option<u32>,
    /// The sp its callers called it with (unknown for one the run can't
    /// see called).
    sp_in: Val,
}

/// What a call passes: r0-r3 and sp (where its stack arguments are).
fn call_args(s: &St) -> Vec<Val> {
    let mut v = s.regs[..4].to_vec();
    v.resize(16, Val::default());
    v[13] = s.regs[13].clone();
    v
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
        let bound = self.img.bound(a);
        let end = bound.min(a.saturating_add(ROM_READ));
        let mut p = a & !3;
        let mut v = Val {
            // Cut short: the words past the cut go unread.
            lost_rom: end < bound,
            ..Val::default()
        };
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
            // A pointer into an object stays one, anywhere in it, through
            // anything but a difference with another (a number).
            let r = bin_tok(self.img, op, x, y);
            // A pointer into an object stays one, anywhere in it, joined
            // or tagged (`|`, `^`, `+` with another), masked down to an
            // alignment, or cut to a byte or half (a piece copied); any
            // other op makes a number of it (the closed world: an
            // alignment test, a difference, a hash).
            let keeps = match (op, x, y) {
                (BinOp::Or | BinOp::Xor | BinOp::Add, _, _) => true,
                (BinOp::And, _, Some(Tok::Num(m))) | (BinOp::And, Some(Tok::Num(m)), _) => {
                    piece_mask(m)
                }
                _ => false,
            };
            if keeps && !matches!(r, R::T(Tok::Obj(..))) {
                for t in [x, y] {
                    if let Some(Tok::Obj(i, _)) = t {
                        out.toks.insert(Tok::Obj(i, OBJ_ANY));
                    }
                }
            }
            {
                match r {
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
                    R::Unk => {
                        out.unk = true;
                        // A ROM pointer less an unknown, or one "from here
                        // on" taken below ROM: the run gives it up (other
                        // ops on a pointer make a number, by the closed
                        // world: an alignment test, a difference).
                        if op == BinOp::Sub
                            && matches!(
                                (x, y),
                                (Some(Tok::Rom(_) | Tok::RomFrom(_)), None)
                                    | (Some(Tok::RomFrom(_)), Some(Tok::Num(_)))
                            )
                        {
                            out.lost_rom = true;
                        }
                    }
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
    fn ret_of(&mut self, k: Ix) -> Vec<Val> {
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
        // Unless the run dropped one: a ROM read it cut short or couldn't
        // make, or a ROM pointer it lost.
        let complete = !callees.is_empty() && !t.lost_rom;
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
        self.at = at;
        let mut vals: Vec<Val> = vec![Val::default(); b.insts.len()];
        for (i, inst) in b.insts.iter().enumerate() {
            let site = Origin::Code(key, i as u32);
            let holes_before = self.g.flow.holes.len();
            let val = |x: ValueId| vals[x.index()].clone();
            let guarded = inst.guard.is_some();
            let v = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    self.at = a;
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
                // What a user-bank register holds isn't followed.
                Op::GetUserReg(_) => Val::lost(),
                Op::Bin(op, a, b) => self.bin(op, &val(a), &val(b), site, at),
                Op::Adc { a, b, c } => {
                    let (a, b) = (val(a), val(b));
                    if a.fns().next().is_some() || b.fns().next().is_some() {
                        // A compare `a + !b + 1` (or an add, a hole when
                        // used): of whole addresses, with each other, 0 or
                        // an unknown.
                        let mut out = Val::unk();
                        let other_num = |v: &Val, zero: u32| {
                            v.toks.iter().any(|t| {
                                matches!(t, Tok::Num(n) if *n != zero)
                                    || matches!(t, Tok::Rom(_) | Tok::RomFrom(_))
                            })
                        };
                        let bad = other_num(&b, 0xffff_ffff) || other_num(&a, 0);
                        for v in [&a, &b] {
                            self.g.hole(v, at, Why::Arithmetic, |f| {
                                bad || !matches!(f, Form::Full | Form::Not)
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
                    self.g.hole(&a, at, Why::Arithmetic, |f| f != Form::Full);
                    for &t in &a.toks {
                        match t {
                            Tok::Fn { key, origin, form } => {
                                if form == Form::Full {
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
                            // `!` of a pointer is a number (the closed world).
                            Tok::Obj(..) => out.unk = true,
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
                            // A ROM pointer through a word load's
                            // rotation stays one, as a function does.
                            Tok::Rom(_) | Tok::RomFrom(_) | Tok::Obj(..)
                                if n == Some(0) || (kind == ShiftType::Ror && n.is_none()) =>
                            {
                                out.toks.insert(t);
                            }
                            // Shifted by bytes, a pointer into an object is a
                            // piece of one (of the object still); by
                            // anything else, a number.
                            Tok::Obj(i, _) => {
                                out.unk = true;
                                if matches!(n, Some(8 | 16 | 24)) {
                                    out.toks.insert(Tok::Obj(i, OBJ_ANY));
                                }
                            }
                            Tok::Num(c) | Tok::Rom(c) => match n {
                                Some(n) => {
                                    out.toks.insert(num_tok(self.img, shift(kind, c, n)));
                                }
                                None => out.unk = true,
                            },
                            // Shifted, it's no pointer (the closed world).
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
                        !matches!(f, Form::Full | Form::Cmp)
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
                    if self.trace && out.lost_obj && !a.lost_obj {
                        eprintln!("  obj lost by load at {at:#010x} w{w} through {a:?}");
                    }
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
            let mut v = v;
            if !v.lost_rom && lost_from(&inst.op).iter().any(|x| vals[x.index()].lost_rom) {
                v.lost_rom = true;
            }
            // What a word op makes of a lost pointer is lost.
            if !v.lost_obj
                && !matches!(inst.op, Op::Load { .. })
                && lost_from(&inst.op).iter().any(|x| vals[x.index()].lost_obj)
            {
                v.unk = true;
                v.lost_obj = true;
            }
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

    /// What a load at width `w` through `a` reads, widened once
    /// ([`from_mem`] doesn't: a load may read thousands of cells).
    fn load(
        &mut self,
        s: &St,
        key: u32,
        i: usize,
        a: &Val,
        w: u8,
        hull: Option<(u32, u32)>,
    ) -> Val {
        let mut v = self.load_raw(s, key, i, a, w, hull);
        v.cap();
        v
    }

    fn load_raw(
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
                    let o = *o;
                    // A saved register's slot: the register as it came, unknown.
                    if fr
                        .slots
                        .iter()
                        .any(|sl| sl.saved && sl.lo <= o && o < sl.hi)
                    {
                        return Val::unk();
                    }
                    let esc = fr.escapes_at(key, i).unwrap_or(true);
                    match s.frame.get(&o) {
                        Some((v, sw)) if *sw == w => {
                            if !esc && o < 0 {
                                return v.clone();
                            }
                            out.join(v);
                        }
                        _ => self.g.read_spill(self.ix, w, &mut out),
                    }
                    if o >= 0 {
                        // At or above the entry sp: the caller's frame (a
                        // stack argument), through the sp it called with.
                        let at = offset(&self.sp_in, Some(o as i64));
                        let v = self.load_at(&at, w, None);
                        out.join(&v);
                    } else if esc {
                        // An escaping slot: its cell, and what stores
                        // through pointers to it left.
                        self.own_read(o, w, &mut out);
                    }
                    return out;
                }
                Some(super::frame::Place::Through(os)) => {
                    self.g.read_spill(self.ix, w, &mut out);
                    self.own_read(OBJ_ANY, w, &mut out);
                    if os.iter().any(|o| *o >= 0) {
                        let at = offset(&self.sp_in, None);
                        let v = self.load_at(&at, w, None);
                        out.join(&v);
                    }
                    return out;
                }
                _ => {}
            }
        }
        let v = self.load_at(a, w, hull);
        out.join(&v);
        out
    }

    /// What a load at width `w` reads of the function's own frame at
    /// `off` (its cells, or the pile if it has no object).
    fn own_read(&mut self, off: i32, w: u8, into: &mut Val) {
        match self.me {
            Some(me) => self.g.read_obj(me, off, w, into),
            None => {
                self.g.read_pile(w, into);
                into.mem = true;
            }
        }
    }

    /// Where a store to the function's own frame at `off` goes.
    fn own_cell(&self, off: i32) -> Store {
        match self.me {
            Some(me) => Store::Cell(obj_addr(me, off)),
            None => Store::Pile,
        }
    }

    /// What a load at width `w` through address `a` reads (the access
    /// not in the function's own frame, or not placed there).
    fn load_at(&mut self, a: &Val, w: u8, hull: Option<(u32, u32)>) -> Val {
        let mut out = Val::default();
        let mut anywhere = a.unk || a.mem;
        // Every object too: a pointer into one may be among the unknown.
        let mut objs = a.lost_obj || a.toks.is_empty();
        for &t in &a.toks {
            match t {
                Tok::Rom(r) => self.rom_read(r, w, &mut out),
                Tok::RomFrom(r) => self.rom_from(r, w, &mut out),
                Tok::Obj(o, off) if self.g.obj_up(o, off) => {
                    anywhere = true;
                    objs = true;
                }
                Tok::Obj(o, off) => self.g.read_obj(o, off, w, &mut out),
                Tok::Num(c) | Tok::Fn { key: c, .. } => {
                    if is_ram(c) {
                        let c = ram_canon(c);
                        self.g
                            .rd
                            .ranges
                            .push((c.saturating_sub(3), c + w as u32 - 1));
                        // The cells the access overlaps.
                        self.g.read_cell(c, w, &mut out);
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
                        && hi - lo < ROM_READ =>
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
                    // Bounded to ROM but too wide to read: its words go
                    // unread.
                    if let Some((lo, hi)) = hull
                        && self.img.word(lo).is_some()
                        && self.img.word(hi).is_some()
                    {
                        out.lost_rom = true;
                    }
                    self.g.read_pile(w, &mut out);
                    match hull {
                        Some((lo, hi)) => {
                            self.g.rd.ranges.push((lo.saturating_sub(3), hi));
                            for (_, c) in self.g.cells.range(lo.saturating_sub(3)..=hi) {
                                from_mem(c, w, &mut out);
                            }
                        }
                        None => self.g.read_cells(w, &mut out),
                    }
                    if objs {
                        self.g.read_objs(w, &mut out);
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
                    // Narrower than a word, what memory would hold (`strh`
                    // of a function's address is its bottom half).
                    let x = &if w == 4 {
                        x.clone()
                    } else {
                        let mut v = Val::unk();
                        v.toks.extend(
                            toks.iter()
                                .filter(|(t, _)| *t != OTHER && *t != LOST && *t != LOST_OBJ)
                                .map(|(t, _)| *t),
                        );
                        v.lost_rom = x.lost_rom;
                        v.lost_obj = x.lost_obj;
                        v
                    };
                    let new = match (guarded, old) {
                        (false, _) => Some((x.clone(), w)),
                        (true, Some((mut ov, ow))) if ow == w => {
                            ov.join(x);
                            Some((ov, w))
                        }
                        _ => None,
                    };
                    s.frame.extend(new.map(|v| (o, v)));
                    self.g.store(Store::Spill(self.ix), toks);
                    if o >= 0 {
                        // The caller's frame, through the sp it called with.
                        let at = offset(&self.sp_in, Some(o as i64));
                        self.store_at(&at, toks);
                    } else if fr.escapes_at(key, i).unwrap_or(true) {
                        let c = self.own_cell(o);
                        self.g.store(c, toks);
                    }
                    return;
                }
                Some(super::frame::Place::Through(os)) => {
                    self.g.store(Store::Spill(self.ix), toks);
                    let c = self.own_cell(OBJ_ANY);
                    self.g.store(c, toks);
                    if os.iter().any(|o| *o >= 0) {
                        let at = offset(&self.sp_in, None);
                        self.store_at(&at, toks);
                    }
                    return;
                }
                _ => {}
            }
        }
        if toks.is_empty() {
            return;
        }
        for &t in &a.toks {
            // I/O, the display memories, SRAM: not followed.
            if let Tok::Num(c) | Tok::Fn { key: c, .. } = t
                && !is_ram(c)
                && self.img.word(c).is_none()
            {
                self.g.hole(x, self.at, Why::Outside, |_| true)
            }
        }
        self.store_at(a, toks);
    }

    /// A store of `toks` through address `a` (not to a place in the
    /// function's own frame).
    fn store_at(&mut self, a: &Val, toks: &[(Tok, u8)]) {
        let mut anywhere = a.unk || a.mem || a.toks.is_empty();
        for &t in &a.toks {
            match t {
                Tok::Num(c) | Tok::Fn { key: c, .. } if is_ram(c) => {
                    self.g.store(Store::Cell(ram_canon(c)), toks)
                }
                Tok::Obj(o, off) if self.g.obj_up(o, off) => anywhere = true,
                Tok::Obj(o, off) => self.g.store(Store::Cell(obj_addr(o, off)), toks),
                Tok::RomFrom(_) => anywhere = true,
                Tok::Num(_) | Tok::Rom(_) | Tok::Fn { .. } => {}
            }
        }
        // A lost pointer into an object: every object.
        if a.lost_obj {
            self.g.store(Store::Cell(LOST_CELL), toks);
        }
        // Somewhere the run can't place: the pile, which every load from
        // RAM reads.
        if anywhere {
            if self.trace && toks.iter().any(|(t, _)| matches!(t, Tok::Fn { .. })) {
                eprintln!("  piled at {:#010x} through {a:?}", self.at);
            }
            self.g.store(Store::Pile, toks);
        }
    }

    fn exit(&mut self, out: &Out, t: Option<Val>, s: &St) {
        let me = self.ix;
        match &out.edge {
            Edge::Goto(ks) => {
                for &k in ks {
                    self.flow(k, s);
                }
            }
            Edge::Call { target, ret } => {
                let args = &call_args(s);
                let rets = match target {
                    Some(k) => {
                        let ki = self.g.ix_call(*k, out.at);
                        self.g.give(ki, args);
                        let mut r = self.ret_of(ki);
                        // An allocator's call: a new object.
                        if self.g.allocs.contains(k) {
                            r[0] = self.g.obj(ObjId::Heap(out.at), 0);
                        }
                        r
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
                // A fall or tail jump keeps the caller's context.
                let ki = self.g.ix_call(*k, ctx_of(me));
                self.g.give(ki, &s.regs);
                let mut r = self.ret_of(ki);
                if self.g.allocs.contains(k) {
                    r[0] = self.g.obj(ObjId::Heap(out.at), 0);
                }
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
                    let mut got = Val {
                        lost_rom: s.regs[0].lost_rom,
                        ..Val::default()
                    };
                    for &t in &s.regs[0].toks {
                        match t {
                            Tok::Rom(a) | Tok::RomFrom(a) => self.rom_from(a, 4, &mut got),
                            _ => {}
                        }
                    }
                    let (toks, _) = to_mem(&got, 4);
                    self.g.store(Store::Pile, &toks);
                    let dst = offset(&s.regs[1], None);
                    self.store_at(&dst, &toks);
                }
                // The BIOS may write numbers through a pointer into an
                // object.
                for r in &s.regs[..4] {
                    if r.has_obj() {
                        let a = offset(r, None);
                        self.store_at(&a, &[(OTHER, 1), (OTHER, 2), (OTHER, 4)]);
                    }
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
        // A large callee set is given only what it wasn't given before
        // (the entries hold that already: they only grow), and its results
        // are joined again only from the callees whose results changed.
        let key = (self.ix, at, tail);
        // Each callee's instance: this call's, or for a jump, the caller's
        // context.
        let ctx = if tail { ctx_of(self.ix) } else { at };
        let memo = self
            .g
            .by_value
            .remove(&key)
            .filter(|m| m.callees == callees && callees.len() > 1);
        let rets = match memo {
            Some(mut m) => {
                let delta: Vec<Val> = args
                    .iter()
                    .zip(&m.args)
                    .map(|(a, o)| {
                        let mut d = Val {
                            mem: a.mem && !o.mem,
                            unk: a.unk && !o.unk,
                            lost_rom: a.lost_rom && !o.lost_rom,
                            lost_obj: a.lost_obj && !o.lost_obj,
                            ..Val::default()
                        };
                        if !std::rc::Rc::ptr_eq(&a.toks.0, &o.toks.0) {
                            d.toks = a.toks.difference(&o.toks).copied().collect();
                        }
                        d
                    })
                    .collect();
                if delta
                    .iter()
                    .any(|d| !d.toks.is_empty() || d.mem || d.unk || d.lost_rom || d.lost_obj)
                {
                    let ixs: BTreeSet<Ix> = callees.iter().map(|&k| self.g.ix_call(k, ctx)).collect();
                    self.g.give_all(&ixs, &delta);
                    for (o, a) in m.args.iter_mut().zip(args) {
                        o.union(a);
                    }
                }
                for &k in &callees {
                    let k = self.g.ix_call(k, ctx);
                    self.rets_read.insert(k);
                    if self.g.ret_stamp.get(&k).is_some_and(|&s| s > m.stamp) {
                        for (a, b) in m.rets.iter_mut().zip(&self.g.ret[&k]) {
                            a.join(b);
                        }
                    }
                }
                m.stamp = self.g.stamp;
                let rets = m.rets.clone();
                self.g.by_value.insert(key, m);
                rets
            }
            None => {
                let mut rets = vec![Val::default(); 4];
                let ixs: BTreeSet<Ix> = callees.iter().map(|&k| self.g.ix_call(k, ctx)).collect();
                self.g.give_all(&ixs, args);
                for &k in &ixs {
                    for (a, b) in rets.iter_mut().zip(self.ret_of(k)) {
                        a.join(&b);
                    }
                }
                if callees.len() > 1 {
                    self.g.by_value.insert(
                        key,
                        ByValue {
                            callees: callees.clone(),
                            args: args.to_vec(),
                            rets: rets.clone(),
                            stamp: self.g.stamp,
                        },
                    );
                }
                rets
            }
        };
        let mut rets = rets;
        if !complete {
            self.g.rd.incomplete = true;
            let mut c = false;
            for (a, b) in self.g.loose_args.iter_mut().zip(args) {
                c |= a.join(b);
            }
            self.g.dirty_loose |= c;
            self.g.mark(c);
            let lr = match &self.g.loose_rets {
                Some(v) => v.clone(),
                None => {
                    let mut v = vec![Val::default(); 4];
                    for k in self.loose {
                        if let Some(r) = self.g.ret.get(k) {
                            for (a, b) in v.iter_mut().zip(r) {
                                a.join(b);
                            }
                        }
                    }
                    self.g.loose_rets = Some(v.clone());
                    v
                }
            };
            self.rets_read.extend(self.loose.iter().copied());
            for (a, b) in rets.iter_mut().zip(&lr) {
                a.join(b);
            }
            for r in &mut rets {
                r.unk = true;
            }
        }
        rets
    }
}

/// The operands whose [`Val::lost_rom`] a word-valued op's result takes
/// (a dropped address through a load or arithmetic may still be one).
fn lost_from(op: &Op) -> Vec<ValueId> {
    match *op {
        Op::Bin(_, a, b)
        | Op::Adc { a, b, .. }
        | Op::MulLongLo { a, b, .. }
        | Op::MulLongHi { a, b, .. }
        | Op::Select { a, b, .. } => vec![a, b],
        Op::Not(a) | Op::Shift { v: a, .. } | Op::Rrx { v: a, .. } | Op::Sext { v: a, .. } => {
            vec![a]
        }
        Op::Load { addr, .. } => vec![addr],
        _ => vec![],
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

/// `v` plus `d` (`None`: an offset not known), as an address: a pointer
/// into an object moved in it, a number by it, anything else unknown.
fn offset(v: &Val, d: Option<i64>) -> Val {
    let mut out = Val {
        unk: v.unk,
        mem: v.mem,
        lost_obj: v.lost_obj,
        ..Val::default()
    };
    for &t in &v.toks {
        match (t, d) {
            (Tok::Obj(i, k), Some(d)) => {
                out.toks.insert(Tok::Obj(i, obj_move(k, d)));
            }
            (Tok::Obj(i, _), None) => {
                out.toks.insert(Tok::Obj(i, OBJ_ANY));
            }
            (Tok::Num(c), Some(d)) => {
                out.toks.insert(Tok::Num(c.wrapping_add(d as u32)));
            }
            _ => out.unk = true,
        }
    }
    out.cap();
    out
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
/// A mask that keeps a pointer one (down to an alignment) or a piece of
/// one (a byte or half of it).
fn piece_mask(m: u32) -> bool {
    m >= 0xffff_0000 || [0xff, 0xff00, 0xff_0000, 0xff00_0000, 0xffff].contains(&m)
}

/// An object's offset `k` moved by `d`.
fn obj_move(k: i32, d: i64) -> i32 {
    if k == OBJ_ANY {
        OBJ_ANY
    } else {
        obj_off(k as i64 + d)
    }
}

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
            // A compare: of a whole address, with another, 0 or a value
            // the closed world says is no function's (a small constant
            // could be a dense ID; a half's top is 0 when dense).
            (BinOp::Sub, Full, None | Some(Num(0)) | Some(Fn { form: Full, .. })) => fnt(Cmp),
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
            _ => hole(Why::Arithmetic),
        };
    }
    // A pointer into an object plus or less a number stays in it (the
    // offset known or not); two pointers' difference is a number. Any
    // other op keeps it anywhere in the object ([`Run::bin`]).
    match (op, x, y) {
        (BinOp::Add, Some(Obj(i, k)), Some(Num(c)))
        | (BinOp::Add, Some(Num(c)), Some(Obj(i, k))) => {
            return R::T(Obj(i, obj_move(k, c as i32 as i64)));
        }
        (BinOp::Sub, Some(Obj(i, k)), Some(Num(c))) => {
            return R::T(Obj(i, obj_move(k, -(c as i32 as i64))));
        }
        (BinOp::Add | BinOp::Sub, Some(Obj(i, _)), None) | (BinOp::Add, None, Some(Obj(i, _))) => {
            return R::T(Obj(i, OBJ_ANY));
        }
        _ => {}
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
pub fn follow(
    fns: &[FnIn],
    img: &dyn Image,
    copies: &[(u32, u32, u32)],
    allocs: &BTreeSet<u32>,
    ctx_blocks: usize,
) -> Flow {
    let _code = CodeGuard::set(img);
    let mut g = G {
        loose_args: vec![Val::default(); 16],
        allocs: allocs.clone(),
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
    // The functions run once per call site: those of at most
    // `ctx_blocks` blocks (an allocator's calls are objects already).
    g.by_site = fns
        .iter()
        .filter(|f| !allocs.contains(&f.entry))
        .filter(|f| f.blocks.len() <= ctx_blocks)
        .map(|f| f.entry)
        .collect();
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
    // (its entry, a callee's results, the memory it read, what incomplete
    // sets pass, the functions they reach). Memory by what a load gives:
    // the pile and every cell by width (compared with what they gave at
    // the round's start), the cells by address.
    let mut reads: BTreeMap<Ix, BTreeSet<Ix>> = BTreeMap::new();
    let mut rds: BTreeMap<Ix, Rd> = BTreeMap::new();
    let mut dirty: BTreeSet<Ix> = fns.iter().map(|f| f.entry as Ix).collect();
    let by_entry: BTreeMap<u32, &FnIn> = fns.iter().map(|f| (f.entry, f)).collect();
    let trace = std::env::var_os("GBAA_FNREF_TRACE").is_some();
    let snap = |g: &mut G| -> Vec<Val> {
        let mut v = Vec::new();
        for w in [1, 2, 4] {
            let (mut p, mut c, mut o) = (Val::default(), Val::default(), Val::default());
            g.read_pile(w, &mut p);
            g.read_cells(w, &mut c);
            g.read_objs(w, &mut o);
            v.push(p);
            v.push(c);
            v.push(o);
        }
        v
    };
    let mut snapped = snap(&mut g);
    g.cells_changed.clear();
    g.pile_changed = false;
    let mut loose: BTreeSet<Ix> = BTreeSet::new();
    let mut ran = 0usize;
    for round in 0..=256 {
        g.changed = false;
        let was = std::mem::replace(
            &mut loose,
            fns.iter()
                .map(|f| f.entry)
                .filter(|k| !direct.contains(k) || g.taken.contains(k))
                .map(|k| k as Ix)
                .collect(),
        );
        if round > 0 {
            // New loose functions take what incomplete sets pass, and each
            // incomplete set reads their results.
            dirty.extend(loose.difference(&was).copied());
            g.loose = loose.clone();
            g.loose_rets = None;
            if loose != was {
                dirty.extend(rds.iter().filter(|(_, r)| r.incomplete).map(|(f, _)| *f));
            }
        }
        if dirty.is_empty() {
            if trace {
                for (i, o) in g.objs.iter().enumerate() {
                    eprintln!("obj {i}: {o:x?}");
                }
            }
            return g.flow;
        }
        if round == 256 {
            break;
        }
        for &i in &dirty {
            let Some(&f) = by_entry.get(&entry_of(i)) else {
                continue;
            };
            let mut entry = g
                .entry
                .get(&i)
                .cloned()
                .unwrap_or_else(|| vec![Val::default(); 16]);
            if loose.contains(&i) {
                for (a, b) in entry.iter_mut().zip(&g.loose_args) {
                    a.join(b);
                }
                for r in &mut entry {
                    r.unk = true;
                }
            }
            // The sp it was called with; its own is its frame's start.
            let mut sp_in = std::mem::take(&mut entry[13]);
            if sp_in.toks.is_empty() {
                sp_in.unk = true;
            }
            for r in &mut entry[4..] {
                r.unk = true;
            }
            let me = g.obj(ObjId::Frame(i), 0);
            let me = me.toks.iter().next().map(|t| match *t {
                Tok::Obj(i, _) => i,
                _ => unreachable!(),
            });
            entry[13] = match me {
                Some(i) => Val::of(Tok::Obj(i, 0)),
                None => Val::lost(),
            };
            let mut run = Run {
                f,
                img,
                g: &mut g,
                loose: &loose,
                ins: BTreeMap::new(),
                work: BTreeSet::new(),
                trace,
                rets_read: BTreeSet::new(),
                ix: i,
                at: 0,
                me,
                sp_in,
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
            run.g.rd = Rd::default();
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
            reads.insert(i, r);
            rds.insert(i, std::mem::take(&mut g.rd));
            // The memory bound, every 64 functions run.
            ran += 1;
            if ran.is_multiple_of(64) {
                let held = g.held();
                let total: usize = held.iter().map(|h| h.2).sum();
                if trace && ran.is_multiple_of(256) {
                    let by: Vec<String> = held
                        .iter()
                        .map(|(k, n, t)| format!("{k} {n}/{t}"))
                        .collect();
                    eprintln!(
                        "fnref {ran} functions run, {:?}: {total} tokens held (sets/tokens: {})",
                        t0.elapsed(),
                        by.join(", ")
                    );
                }
                if total > TOKENS {
                    g.flow.unsettled = true;
                    g.flow.over = Some((total, ran));
                    return g.flow;
                }
            }
        }
        // What every value held as a function's whole address: taken
        // (each shared set once).
        let mut seen = std::collections::HashSet::new();
        let held: Vec<Val> = g
            .entry
            .values()
            .chain(g.ret.values())
            .flatten()
            .filter(|v| seen.insert(std::rc::Rc::as_ptr(&v.toks.0)))
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
        dirty = std::mem::take(&mut g.dirty_entry);
        let rets = std::mem::take(&mut g.dirty_ret);
        let n_entry = dirty.len();
        let n_rets = rets.len();
        dirty.extend(
            reads
                .iter()
                .filter(|(_, r)| !r.is_disjoint(&rets))
                .map(|(f, _)| *f),
        );
        if std::mem::take(&mut g.dirty_loose) {
            dirty.extend(loose.iter().copied());
        }
        // Memory: the widths whose pile or every-cell read changed, and
        // the cells that did.
        let mut pile_w = 0u8;
        let mut cells_w = 0u8;
        let mut objs_w = 0u8;
        let cells = std::mem::take(&mut g.cells_changed);
        if std::mem::take(&mut g.pile_changed) || !cells.is_empty() {
            let now = snap(&mut g);
            for (i, w) in [1u8, 2, 4].into_iter().enumerate() {
                if now[3 * i] != snapped[3 * i] {
                    pile_w |= 1 << w;
                }
                if now[3 * i + 1] != snapped[3 * i + 1] {
                    cells_w |= 1 << w;
                }
                if now[3 * i + 2] != snapped[3 * i + 2] {
                    objs_w |= 1 << w;
                }
            }
            snapped = now;
        }
        dirty.extend(
            rds.iter()
                .filter(|(_, r)| {
                    r.pile & pile_w != 0
                        || r.all_cells & cells_w != 0
                        || r.all_objs & objs_w != 0
                        || r.ranges
                            .iter()
                            .any(|&(lo, hi)| cells.range(lo..=hi).next().is_some())
                })
                .map(|(f, _)| *f),
        );
        if trace {
            eprintln!(
                "fnref round {round}: {:?}; {} to rerun ({n_entry} entries changed, {n_rets} results; pile widths {pile_w:#x}, every-cell widths {cells_w:#x}, {} cells changed); pile {}, cells {}, taken {}, holes {}",
                t0.elapsed(),
                dirty.len(),
                cells.len(),
                g.pile.len(),
                g.cells.len(),
                g.taken.len(),
                g.flow.holes.len()
            );
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
            "function addresses (native/fnref.rs){}: {} function words in ROM ({pools} in literal pools, {} other data), {proven} proven, {} not: {unread} never read{} ({piled} of the {} not proven stored where the run can't place them, the pile); {} code constants, {} proven; {} call_fn calls with a callee set, {complete} of them complete; {} other jumps by value to a function",
            match (self.unsettled, self.over) {
                (true, Some((t, n))) => format!(
                    " (stopped holding {t} tokens, over its bound of {TOKENS}, after {n} functions run: nothing proven)"
                ),
                (true, None) => " (didn't settle: nothing proven)".to_string(),
                _ => String::new(),
            },
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
