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
//!   the words of the object it points into: up to the next code
//!   ([`Image::bound`]) or the next ROM data address a ROM word holds
//!   ([`objects`]), and only those the value-set analysis bounds the
//!   load to (`native/fnref-rom-read-bound`);
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
//! unless a pointer into one was dropped ([`Val::lost_ptr`]: then every
//! object). RAM addresses the same way (`native/fnref-unknown-addrs`):
//! a constant RAM address is a pointer, kept in memory as itself, and
//! moved by an index the run doesn't know it is one "from here on"
//! ([`Tok::RamFrom`]: up to the next RAM address the code names; a
//! piece of one too, as an object's); less an unknown, or in more runs
//! than [`RAM_RUNS`], it is lost ([`Val::lost_ptr`]). A load through a pointer into an object reads its
//! cell there, its anywhere cell and what lost pointers stored; one at a
//! constant address its cell and the pile; one from here on in RAM the
//! cells of its run and the pile; one through a lost pointer, the pile
//! and every cell, RAM's and every object's; one through any other
//! unknown nothing in RAM (an I/O read: by the closed world no RAM
//! address; a store through one changes no cell, its function and ROM
//! addresses still go to the pile), or, where the value-set
//! analysis bounds it to ROM, the words there (a ROM pointer "from here
//! on" too reads only the words in those bounds). A store into ROM
//! changes nothing. An object doesn't overlap
//! a constant address the code reads (the stack, the heap). Interprocedural: a call passes r0-r3 to its callee's entry
//! and takes back what its returns hold; a tail call or a fall passes
//! every register. An indirect call passes them to each function whose
//! whole address its target may hold. A call whose target holds none is
//! an incomplete callee set: its arguments go to every function that may
//! be called that way (one with no direct caller, or whose address is
//! taken), and its results come back from all of them. A function that
//! still has no caller in sight when the run settles (its vtable word
//! was never read) is run again: a call of its own through a target that
//! holds no function may be any function whose address is taken (what a
//! load through an unknown address gave before RAM pointers were
//! followed), and is kept complete. Functions rerun
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
//! **Large reads by reference** (`native/fnref-flow-emerald`). On a large
//! game a read of the pile, of every cell, or of a ROM table by an unknown
//! index is tens of thousands of tokens, a new set at each store, and
//! every function entry joined from a different one held its own copy
//! (Emerald's flow passed 10 GB in its first round). So such a read is one
//! token, [`Tok::Ref`]: its source and what was done to each function's
//! address in it since ([`Xf`]: a shift's or `mvn`'s form, a compare's, a
//! narrower load's or store's). Memory stores it as it is; whoever looks
//! at the tokens (an op on them, a load's address, a call's target, a
//! hole) expands it with memory as it is then ([`G::expand`]) and so reads
//! that memory, rerunning when it grows. A small one (under 64 tokens) is
//! expanded where an op makes it, as before.
//!
//! **Equal sets kept once** (`native/fnref-flow-emerald`). Emerald's run
//! stopped at [`TOKENS`] holding copies: values that grew to the same
//! tokens by different joins each had their own set, 44 million tokens of
//! which 3 million were distinct. The sets a value is kept in for long (an
//! entry, a result, a call by value's, a kept read or expansion) are now
//! found by content ([`Sets`]), so equal ones are one set, and the bound
//! counts every one of them ([`G::held`]); where the first run stopped
//! (27,776 functions run) this one holds about 4 million. An op on a
//! large value is kept by its operands' sets ([`G::op_kept`]), and a call
//! by value joins each distinct result set once ([`Once`]): Emerald's
//! first round takes 31 s (88 before), and its second, which the run
//! never finished before, ends at 177 s. It still doesn't settle: after
//! seven rounds 12,000 entries change in each, holding 24 million tokens
//! (`memory/fnref-flow-emerald-memory.md` says where that stands).
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
//! called in part. Lost (not followed, so not proven): given to the BIOS
//! (in a register the call reads: [`bios_regs`]), moved to a user-bank
//! register. A word is **proven** when the run read
//! it and no value it entered met either. A function word in ROM the run
//! never read (no tracked load reaches it) isn't proven either.
//!
//! Nothing printed changes: the report lists the words, the holes and the
//! callee sets ([`Flow::report`]), and the metrics count the words not
//! proven (barred) and proven.
//!
//! Precision (`docs/native-stage-b.md`, "Known gaps", the pile;
//! `docs/research/fnref-held-words.md`): the agb programs moved a word
//! at most from the objects, from call-site context
//! (`native/fnref-context`, k = 1, [`Ix`]: 5 s became 21 s on affine,
//! 445 s with each `call_fn` callee's too), or from RAM pointers
//! followed (`native/fnref-unknown-addrs`: the loads that read
//! everything, [`Flow::everything`], fell from 623 sites to 3 on
//! affine). No one kind of read holds the words; two together do: the
//! loads that read everything left (through a pointer into a caller's
//! frame) and a pointer anywhere in a frame (an array of pairs read by
//! index, a byte loop). Besides those: registers a callee neither reads
//! nor sets, passed in and taken back (a function's address left in r3
//! crosses 26 functions to a `swi`), and the pile, which every load at
//! a constant address reads.
//!
//! Tools, all by environment variable: `GBAA_FNREF_TRACE=1` prints each
//! round, each use not allowed with its operands, where a RAM pointer is
//! lost and why a callee set is incomplete;
//! `GBAA_FNREF_WATCH=WORD[,WORD…]` (or `all`, every ROM word) each place
//! those words' function addresses go ([`G::watch`], through
//! [`Tok::Ref`]s), and (`roads` alone turns only this on) at each
//! instruction that makes a use not allowed, what its operands held each
//! address by ([`G::roads`]); `GBAA_FNREF_OFF` makes kinds of hole not
//! happen or kinds of read give nothing, unsound, to measure
//! ([`off_env`]). `docs/research/fnref-held-words.md` is made with them.

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
    /// Somewhere in RAM from this address (without its mirror) on, up to
    /// the next RAM address the code names ([`ram_bound`]): a constant
    /// RAM address moved by an index the run doesn't know (an array in a
    /// static, a loop's pointer), as [`Tok::RomFrom`] in ROM.
    RamFrom(u32),
    /// A pointer into an object ([`ObjId`], by its index in
    /// [`G::objs`]) at this offset, or anywhere in it ([`OBJ_ANY`]).
    Obj(u32, i32),
    /// What a load at width `w` reads of the pile, or of every cell, as
    /// it is when someone looks ([`G::ref_val`]), not its tokens: a large
    /// game's pile read is tens of thousands of tokens, a new set at each
    /// store, and each function entry joined from a different one held
    /// its own copy (`native/fnref-flow-emerald`). `xf`: with each
    /// function's form made one ([`Xf`]). Memory keeps it where storing
    /// it changes none of its tokens. Sorted last; whoever looks at the
    /// tokens expands it ([`G::expand`]) and so reads that memory (reruns
    /// when it grows).
    Ref {
        src: Src,
        xf: Xf,
    },
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
/// More objects than this in one value: a pointer lost
/// ([`Val::lost_ptr`]), as for RAM runs past [`RAM_RUNS`]. Without it an
/// sp passed in from hundreds of callers held a frame of each, and every
/// stack argument read them all (Emerald's first round 15 times main's).
/// At 64 the agb programs' metrics are the same as with none.
const OBJS: usize = 64;
/// Where the objects' cells are kept, among RAM's: each object a 64 KB
/// window above here (no RAM address, nor any the program makes).
const OBJ_BASE: u32 = 0x1000_0000;
/// The cell of stores through a lost pointer into an object
/// ([`Val::lost_ptr`]): every object's load reads it, as does every load
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

/// What a [`Tok::Ref`] stands for: a load at width `w`'s read of the pile,
/// of every RAM cell, of every object's cells, of the ROM words in `lo..end` a read from `lo` on
/// gives ([`Run::rom_from`]), or of the ROM words in `lo..=hi` (a read
/// the value-set analysis bounds there).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Src {
    Pile(u8),
    Cells(u8),
    Objs(u8),
    RomFrom(u32, u32, u8),
    Hull(u32, u32, u8),
}

impl Src {
    fn w(self) -> u8 {
        match self {
            Src::Pile(w)
            | Src::Cells(w)
            | Src::Objs(w)
            | Src::RomFrom(_, _, w)
            | Src::Hull(_, _, w) => w,
        }
    }
}

/// What a [`Tok::Ref`] makes of the tokens it stands for: each
/// function's address's form mapped (`map[form]`, [`Xf::DROP`]: none of
/// it), ROM pointers kept or not (and if not, whether they make it a
/// number read from memory), the read's flags kept or not, and RAM
/// pointers ([`Xf::ptrs`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Xf {
    map: [u8; 8],
    roms: bool,
    rom_mem: bool,
    flags: bool,
    /// What it makes of a RAM address, a [`Tok::RamFrom`] or a
    /// [`Tok::Obj`] (`native/fnref-unknown-addrs`): [`Xf::PTR_KEEP`]
    /// them; [`Xf::PTR_PIECE`], a piece of one, as [`from_mem_one`]
    /// reads it narrower or reassembled (from here on, anywhere in the
    /// object, and unknown); [`Xf::PTR_DROP`] (an op's, whose other
    /// tokens are run as they are: [`Run::split_refs`]).
    ptrs: u8,
}

const FORMS: [Form; 8] = [
    Form::Full,
    Form::Hi,
    Form::Lo,
    Form::HiUp,
    Form::LoUp,
    Form::Part,
    Form::Not,
    Form::Cmp,
];

impl Xf {
    const DROP: u8 = u8::MAX;
    const PTR_DROP: u8 = 0;
    const PTR_PIECE: u8 = 1;
    const PTR_KEEP: u8 = 2;
    /// As memory holds it.
    const ID: Xf = Xf {
        map: [0, 1, 2, 3, 4, 5, 6, 7],
        roms: true,
        rom_mem: false,
        flags: true,
        ptrs: Xf::PTR_KEEP,
    };

    /// Every form made `f` ([`Form::Part`]: a word's bytes, read narrower
    /// than it was stored; [`Form::Full`]: reassembled, read as a word
    /// stored narrower; [`Form::Cmp`]: compared, [`Run::cmp_refs`]).
    const fn all(f: Form, mem: bool) -> Xf {
        Xf {
            map: [f as u8; 8],
            roms: false,
            rom_mem: mem,
            flags: mem,
            ptrs: if mem { Xf::PTR_PIECE } else { Xf::PTR_DROP },
        }
    }

    /// Each form as `m` makes it (an op's: [`Run::split_refs`]).
    fn of(m: impl Fn(Form) -> Option<Form>) -> Xf {
        let mut map = [Xf::DROP; 8];
        for f in FORMS {
            map[f as usize] = m(f).map_or(Xf::DROP, |g| g as u8);
        }
        Xf {
            map,
            roms: false,
            rom_mem: false,
            flags: false,
            ptrs: Xf::PTR_DROP,
        }
    }

    /// This, then `o`.
    fn then(self, o: Xf) -> Xf {
        let mut map = [Xf::DROP; 8];
        for (i, m) in map.iter_mut().enumerate() {
            if self.map[i] != Xf::DROP {
                *m = o.map[self.map[i] as usize];
            }
        }
        Xf {
            map,
            roms: self.roms && o.roms,
            rom_mem: (self.rom_mem && o.flags) || (self.roms && o.rom_mem),
            flags: self.flags && o.flags,
            // Drop, a piece, keep: in that order, the less kept wins.
            ptrs: self.ptrs.min(o.ptrs),
        }
    }

    fn form(self, f: Form) -> Option<Form> {
        FORMS.get(self.map[f as usize] as usize).copied()
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

impl Toks {
    /// Adds `new` (in any order): one at a time when they're few beside
    /// the set, else by building the set again from both (an insert each
    /// of thousands of tokens was most of a large game's run).
    fn add_all(&mut self, new: Vec<Tok>) {
        if new.len() <= 32 || new.len() * 8 < self.0.len() {
            let s = std::rc::Rc::make_mut(&mut self.0);
            for t in new {
                s.insert(t);
            }
            return;
        }
        let mut v: Vec<Tok> = Vec::with_capacity(self.0.len() + new.len());
        v.extend(self.0.iter().copied());
        v.extend(new);
        self.0 = std::rc::Rc::new(v.into_iter().collect());
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

/// The token sets the run keeps, by content, so that equal sets are one
/// set (`native/fnref-flow-emerald`). Values that grew to the same tokens
/// by different joins each held their own copy: on Emerald 44 million
/// tokens in 86,000 sets of which 3 million, in 9,000 sets, were distinct
/// (one 3,409-token set 685 times over, in entries, results and calls by
/// value), which is what took the run to [`TOKENS`]. A set is found by
/// its hash and held weakly: changing one makes a new set
/// (`Rc::make_mut`), and one nothing holds is gone.
#[derive(Default)]
struct Sets {
    by: std::collections::HashMap<u64, Vec<std::rc::Weak<BTreeSet<Tok>>>, Fx>,
    /// The sets added since the dead were last swept, and when to sweep.
    added: usize,
    sweep: usize,
}

impl Sets {
    /// Makes `t` the kept set of its tokens (the first seen of them).
    fn share(&mut self, t: &mut Toks) {
        use std::hash::{Hash, Hasher};
        if t.is_empty() {
            return;
        }
        let mut h = FxHasher(t.len() as u64);
        for x in t.iter() {
            x.hash(&mut h);
        }
        let slot = self.by.entry(h.finish()).or_default();
        for w in slot.iter() {
            if let Some(s) = w.upgrade()
                && (std::rc::Rc::ptr_eq(&s, &t.0) || *s == *t.0)
            {
                t.0 = s;
                return;
            }
        }
        slot.push(std::rc::Rc::downgrade(&t.0));
        self.added += 1;
        if self.added >= self.sweep.max(4096) {
            self.by.retain(|_, v| {
                v.retain(|w| w.strong_count() > 0);
                !v.is_empty()
            });
            self.added = 0;
            self.sweep = self.by.len();
        }
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
    /// Maybe a pointer into RAM the run dropped: into an object, or a
    /// constant RAM address (arithmetic it doesn't follow, a pointer
    /// stored in pieces, too many addresses to keep). Used as an address,
    /// it may reach every object and every RAM cell; nothing else unknown
    /// is a RAM address (`native/fnref-unknown-addrs`). Set only with
    /// [`Val::unk`].
    lost_ptr: bool,
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

thread_local! {
    /// The RAM addresses the run's code names (without their mirrors),
    /// sorted, for [`ram_bound`] (empty outside [`follow`]).
    static RAMK: std::cell::RefCell<std::rc::Rc<Vec<u32>>> = std::cell::RefCell::default();
}

/// Where a pointer into RAM from `a` (without its mirror) may reach: the
/// next RAM address the code names, or the end of `a`'s RAM.
fn ram_bound(a: u32) -> u32 {
    let end = if a < 0x0300_0000 {
        0x0204_0000
    } else {
        0x0300_8000
    };
    RAMK.with(|k| {
        let k = k.borrow();
        k.get(k.partition_point(|&x| x <= a))
            .copied()
            .map_or(end, |n| n.min(end))
    })
}

/// Sets [`RAMK`] for a run; the one before comes back when it's dropped.
struct RamGuard(std::rc::Rc<Vec<u32>>);

impl RamGuard {
    fn set(fns: &[FnIn], copies: &[(u32, u32, u32)]) -> RamGuard {
        let mut v: Vec<u32> = copies.iter().map(|c| c.1).filter(|&a| is_ram(a)).collect();
        for f in fns {
            for b in f.blocks.values() {
                for inst in b.insts {
                    if let Op::Const(c) | Op::Pool(c) = inst.op
                        && is_ram(c)
                    {
                        v.push(c);
                    }
                }
            }
        }
        let mut v: Vec<u32> = v.into_iter().map(ram_canon).collect();
        v.sort_unstable();
        v.dedup();
        RamGuard(RAMK.with(|k| k.replace(std::rc::Rc::new(v))))
    }
}

impl Drop for RamGuard {
    fn drop(&mut self) {
        RAMK.with(|k| *k.borrow_mut() = self.0.clone());
    }
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

/// The most tokens the run holds (each distinct set once, the kept reads'
/// and expansions' too, and memory's: [`G::held`]) before it stops,
/// unsettled, as when its rounds run out: proving
/// nothing then, as a lift without the flow (`skip_fn_flow`). Metroid
/// settles holding about 30 million at a peak of 1.4 GB; Kirby -
/// Nightmare in Dream Land and Minish Cap held over 100 million in a
/// round and a half, past 15 GB, before (`native/whole-lift-memory`).
const TOKENS: usize = 48_000_000;

/// The lowest [`Tok::Ref`].
const REF0: Tok = Tok::Ref {
    src: Src::Pile(0),
    xf: Xf {
        map: [0; 8],
        roms: false,
        rom_mem: false,
        flags: false,
        ptrs: Xf::PTR_DROP,
    },
};

/// Memory a [`Tok::Ref`] may read ([`G::read_mem`]): the pile, every RAM
/// cell, every object's cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemKind {
    Pile,
    Cells,
    Objs,
}

/// A load at width `w` of the pile's, every RAM cell's, or every
/// object's words.
fn mem_ref(k: MemKind, w: u8) -> Tok {
    Tok::Ref {
        src: match k {
            MemKind::Pile => Src::Pile(w),
            MemKind::Cells => Src::Cells(w),
            MemKind::Objs => Src::Objs(w),
        },
        xf: Xf::ID,
    }
}

/// More numbers than this in a value: it's unknown.
const NUMS: usize = 8;
/// More exact RAM addresses than this in a value: "from the lowest on"
/// ([`Tok::RamFrom`]); more of those, a pointer lost ([`Val::lost_ptr`]).
/// A ROM table read by an unknown index gives every word in it that
/// looks like one (on affine's font and pictures, 100 to 200 of each run
/// of data).
const RAMS: usize = 16;
/// More RAM runs than this in a value (each [`Tok::RamFrom`]): a pointer
/// lost. (256 before the merge with main's `Ref` tokens: Emerald names
/// thousands of globals, and its entries held 125,000 of these; at 64
/// the agb programs' metrics are the same as at 256.)
const RAM_RUNS: usize = 64;
/// More exact ROM pointers than this: "from the lowest on".
const ROMS: usize = 16;
/// The most ROM bytes one read by an unknown index reads (a "from here
/// on" pointer up to the next code, or a ROM range the value-set analysis
/// bounds); past it the read is cut short and the value marked
/// [`Val::lost_rom`]. The agb programs' data runs to about 110 KB; reading
/// it all costs about a second of each lift.
const ROM_READ: u32 = 0x4_0000;

/// The most bytes a `CpuSet` (or, `fast`, `CpuFastSet`) with `r2` as its
/// length and mode reads, if `r2` holds only numbers: a fill reads one
/// unit, a copy its count of halfwords or words (`CpuFastSet` in blocks
/// of eight words).
fn copy_len(fast: bool, r2: &Val) -> Option<u32> {
    if r2.unk || r2.mem || r2.toks.is_empty() {
        return None;
    }
    let mut most = 0;
    for &t in &r2.toks {
        let Tok::Num(m) = t else { return None };
        let unit = if fast || m & 1 << 26 != 0 { 4 } else { 2 };
        let count = if m & 1 << 24 != 0 {
            1
        } else if fast {
            (m & 0x1f_ffff).div_ceil(8) * 8
        } else {
            m & 0x1f_ffff
        };
        most = most.max(count * unit);
    }
    Some(most)
}

/// The objects in ROM data, by start: every ROM data address (not a
/// function's key) that an aligned ROM word holds, a pool literal's or a
/// pointer field's. A read "from here on" stays in the object it starts
/// in, so it stops at the next of these: the next glyph, string, slice
/// or vtable. Without it, affine's reads through its font's glyph
/// records and strings ran 110 KB to the next code, into agb's vtables
/// (`native/fnref-rom-read-bound`). Font bytes that look like pointers
/// add starts too; only cutting a table of handlers short would be
/// wrong, so [`Run::object_end`] doesn't stop between function words.
fn objects(img: &dyn Image) -> BTreeSet<u32> {
    let mut o = BTreeSet::new();
    let mut p = 0x0800_0000u32;
    while let Some(x) = img.word(p) {
        if x & 0xff00_0000 == 0x0800_0000 && img.word(x).is_some() && !img.is_fn(x) {
            o.insert(x);
        }
        p = p.wrapping_add(4);
    }
    o
}

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
            lost_ptr: true,
            ..Val::default()
        }
    }

    fn has_obj(&self) -> bool {
        self.lost_ptr
            || self
                .toks
                .range(Tok::Obj(0, i32::MIN)..REF0)
                .next()
                .is_some()
    }

    fn of(t: Tok) -> Val {
        Val {
            toks: [t].into_iter().collect(),
            ..Val::default()
        }
    }

    fn flags(&self) -> Flags {
        (self.mem, self.unk, self.lost_rom, self.lost_ptr)
    }

    /// Join without widening (as tokens read from memory one by one).
    fn union(&mut self, o: &Val) {
        if self.toks.is_empty() {
            self.toks = o.toks.clone();
        } else if !std::rc::Rc::ptr_eq(&self.toks.0, &o.toks.0) && !o.toks.is_subset(&self.toks) {
            self.toks.add_all(o.toks.iter().copied().collect());
        }
        self.mem |= o.mem;
        self.unk |= o.unk;
        self.lost_rom |= o.lost_rom;
        self.lost_ptr |= o.lost_ptr;
    }

    fn join(&mut self, o: &Val) -> bool {
        let before = (
            self.toks.len(),
            self.mem,
            self.unk,
            self.lost_rom,
            self.lost_ptr,
        );
        if self.toks.is_empty() {
            self.toks = o.toks.clone();
        } else if !std::rc::Rc::ptr_eq(&self.toks.0, &o.toks.0) && !o.toks.is_subset(&self.toks) {
            self.toks.add_all(o.toks.iter().copied().collect());
        }
        self.mem |= o.mem;
        self.unk |= o.unk;
        self.lost_rom |= o.lost_rom;
        self.lost_ptr |= o.lost_ptr;
        self.cap();
        before
            != (
                self.toks.len(),
                self.mem,
                self.unk,
                self.lost_rom,
                self.lost_ptr,
            )
    }

    /// [`Val::union`], and whether `o` added anything (the callees'
    /// results of a call by value are taken in so, then capped once: a cap
    /// at each of thousands was most of a large game's run).
    fn union_new(&mut self, o: &Val) -> bool {
        let flags = (o.mem && !self.mem)
            || (o.unk && !self.unk)
            || (o.lost_rom && !self.lost_rom)
            || (o.lost_ptr && !self.lost_ptr);
        let toks = !(o.toks.is_empty()
            || std::rc::Rc::ptr_eq(&self.toks.0, &o.toks.0)
            || o.toks.is_subset(&self.toks));
        if toks {
            if self.toks.is_empty() {
                self.toks = o.toks.clone();
            } else {
                self.toks.add_all(o.toks.iter().copied().collect());
            }
        }
        self.mem |= o.mem;
        self.unk |= o.unk;
        self.lost_rom |= o.lost_rom;
        self.lost_ptr |= o.lost_ptr;
        flags || toks
    }

    /// [`Val::join`] into a value as the cap leaves it (one only ever
    /// joined into, from empty: an entry, a result): when `o` adds
    /// nothing, without the cap (a large callee set's results, joined
    /// one by one, capped each time, was most of a large game's run).
    ///
    /// Nor when the cap would drop again all it adds ([`Val::takes_in`],
    /// and the pointers it adds don't make the cap widen those it holds):
    /// the join made a copy of the set to add them to, equal to the set
    /// when capped, and the entries held those copies apart
    /// (`native/fnref-flow-emerald`). So what is left does change it,
    /// whatever its length after (seventeen pointers widened to sixteen
    /// "from here on" are as many tokens as the sixteen before).
    fn join_capped(&mut self, o: &Val) -> bool {
        let roms = |v: &Val| v.toks.range(Tok::Rom(0)..=Tok::Rom(u32::MAX)).count();
        let adds = (o.mem && !self.mem)
            || (o.unk && !self.unk)
            || (o.lost_rom && !self.lost_rom)
            || (o.lost_ptr && !self.lost_ptr)
            || !(o.toks.is_empty()
                || std::rc::Rc::ptr_eq(&self.toks.0, &o.toks.0)
                || o.toks.iter().all(|t| self.takes_in(*t))
                    // Past ROMS exact pointers between them, the cap
                    // widens the ones this holds too.
                    && (roms(self) == 0
                        || roms(self)
                            + o.toks
                                .range(Tok::Rom(0)..=Tok::Rom(u32::MAX))
                                .filter(|t| !self.toks.contains(t))
                                .count()
                            <= ROMS));
        if !adds {
            return false;
        }
        // What `takes_in` can't tell (RAM pointers past [`RAMS`], an
        // object's offsets past [`OBJ_OFFS`]) is joined and compared: a
        // join the cap undoes is no change.
        // (Unchanged, it keeps the set it held, not an equal copy.)
        let was = self.clone();
        self.join(o);
        if *self == was {
            *self = was;
            return false;
        }
        true
    }

    /// Whether this, as the cap leaves it, takes token `t` in with no more
    /// than the cap dropping `t` again: it holds `t`, or `t` is a number
    /// and it is unknown, or a ROM pointer one of its "from here on"
    /// takes in (which still counts toward the cap's ROMS:
    /// [`Val::join_capped`]). RAM pointers ([`Val::cap_ram`]): a lost
    /// pointer takes in every RAM address and run, an object's
    /// anywhere every offset into it; the rest are joined.
    fn takes_in(&self, t: Tok) -> bool {
        self.toks.contains(&t)
            || match t {
                // A lost pointer takes in every RAM address, a pointer
                // "from here on" in RAM those in its run ([`Val::cap_ram`]).
                Tok::Num(c) | Tok::RamFrom(c) if is_ram(c) => {
                    self.lost_ptr || {
                        let c = ram_canon(c);
                        self.toks
                            .range(Tok::RamFrom(0)..=Tok::RamFrom(c))
                            .next_back()
                            .is_some_and(|f| matches!(*f, Tok::RamFrom(lo) if c < ram_bound(lo)))
                    }
                }
                Tok::RamFrom(_) => self.lost_ptr,
                Tok::Obj(i, _) => self.lost_ptr || self.toks.contains(&Tok::Obj(i, OBJ_ANY)),
                Tok::Num(_) => self.unk,
                Tok::Rom(x) | Tok::RomFrom(x) => self
                    .toks
                    .range(Tok::RomFrom(0)..=Tok::RomFrom(x))
                    .next_back()
                    .is_some_and(|f| matches!(*f, Tok::RomFrom(lo) if x < bound(lo))),
                _ => false,
            }
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
        let mut rams = 0;
        for t in self.toks.range(Tok::Num(0)..REF0) {
            match *t {
                Tok::Num(c) if is_ram(c) => rams += 1,
                Tok::RamFrom(_) => rams += 1,
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
        let objs = {
            let mut n = 0;
            let mut last = None;
            for t in self.toks.range(Tok::Obj(0, i32::MIN)..REF0) {
                if let Tok::Obj(i, _) = *t
                    && last != Some(i)
                {
                    n += 1;
                    last = Some(i);
                }
            }
            n
        };
        if objs > OBJS {
            lost_said(|| "more objects than OBJS in a value".into());
            self.unk = true;
            self.lost_ptr = true;
        }
        // A lost pointer takes in every object (it reads and stores
        // them all: [`LOST_CELL`]), so the cap stays monotone.
        if self.lost_ptr && objs > 0 {
            self.drop_range(Tok::Obj(0, i32::MIN)..REF0);
        } else {
            for i in wide {
                self.drop_range(Tok::Obj(i, i32::MIN)..=Tok::Obj(i, OBJ_ANY));
                self.toks.insert(Tok::Obj(i, OBJ_ANY));
            }
        }
        if nums > NUMS {
            self.unk = true;
        }
        // Unknown takes in every number but RAM addresses (so the cap
        // stays monotone): those are pointers, kept up to [`RAMS`] of
        // them; past that they are lost.
        if self.unk && nums > 0 {
            self.drop_range(Tok::Num(0)..Tok::Num(RAM.start));
            self.drop_range(Tok::Num(RAM.end)..=Tok::Num(u32::MAX));
        }
        // A lost pointer takes in every RAM address (it reads them all).
        if self.lost_ptr {
            if rams > 0 {
                self.drop_range(Tok::Num(RAM.start)..Tok::Num(RAM.end));
                self.drop_range(Tok::RamFrom(0)..=Tok::RamFrom(u32::MAX));
            }
        } else if rams > 0 {
            self.cap_ram(rams);
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
        // of its run of data: one walk over both sorted lists (a search
        // per pointer was most of a large game's later rounds, whose
        // results hold thousands).
        let addrs = |r: std::ops::RangeInclusive<Tok>| -> Vec<u32> {
            self.toks
                .range(r)
                .filter_map(|t| match *t {
                    Tok::Rom(a) | Tok::RomFrom(a) => Some(a),
                    _ => None,
                })
                .collect()
        };
        let roms = addrs(Tok::Rom(0)..=Tok::Rom(u32::MAX));
        let los = addrs(Tok::RomFrom(0)..=Tok::RomFrom(u32::MAX));
        let code = CODE.with(|c| c.borrow().clone());
        let mut at = 0;
        let mut bound = |a: u32| -> u32 {
            if code.is_empty() {
                return u32::MAX;
            }
            at += code[at..].partition_point(|&k| k <= a);
            code.get(at).copied().unwrap_or(u32::MAX)
        };
        let mut gone: Vec<Tok> = Vec::new();
        let (mut covered, mut ri) = (0, 0);
        for (i, &lo) in los.iter().enumerate() {
            if lo < covered {
                continue;
            }
            let end = bound(lo);
            covered = end;
            while ri < roms.len() && roms[ri] < lo {
                ri += 1;
            }
            while ri < roms.len() && roms[ri] < end {
                gone.push(Tok::Rom(roms[ri]));
                ri += 1;
            }
            gone.extend(
                los[i + 1..]
                    .iter()
                    .take_while(|&&f| f < end)
                    .map(|&f| Tok::RomFrom(f)),
            );
        }
        if !gone.is_empty() {
            let s = &mut self.toks;
            for t in gone {
                s.remove(&t);
            }
        }
    }

    /// Widen RAM addresses as ROM's ([`Val::cap`]): too many are "from
    /// the lowest on" per run between the addresses the code names
    /// ([`ram_bound`]), and one from here on takes in those above it in
    /// its run; too many runs, it is lost.
    fn cap_ram(&mut self, rams: usize) {
        let ram_num = |t: &Tok| matches!(*t, Tok::Num(c) if is_ram(c));
        if rams > RAMS {
            let mut los: Vec<u32> = self
                .toks
                .range(Tok::Num(RAM.start)..Tok::Num(RAM.end))
                .filter(|t| ram_num(t))
                .map(|t| match *t {
                    Tok::Num(c) => ram_canon(c),
                    _ => unreachable!(),
                })
                .collect();
            los.sort_unstable();
            let mut runs: Vec<u32> = Vec::new();
            let mut end = 0;
            for a in los {
                if a >= end {
                    runs.push(a);
                    end = ram_bound(a);
                }
            }
            self.drop_range(Tok::Num(RAM.start)..Tok::Num(RAM.end));
            for lo in runs {
                self.toks.insert(Tok::RamFrom(lo));
            }
        }
        let froms: Vec<u32> = self
            .toks
            .range(Tok::RamFrom(0)..=Tok::RamFrom(u32::MAX))
            .map(|t| match *t {
                Tok::RamFrom(a) => a,
                _ => unreachable!(),
            })
            .collect();
        if froms.is_empty() {
            return;
        }
        if froms.len() > RAM_RUNS {
            self.unk = true;
            lost_said(|| "more RAM runs than RAM_RUNS in a value".into());
            self.lost_ptr = true;
            self.drop_range(Tok::Num(RAM.start)..Tok::Num(RAM.end));
            self.drop_range(Tok::RamFrom(0)..=Tok::RamFrom(u32::MAX));
            return;
        }
        // Each from here on takes in the exact addresses and those from
        // here on above it in its run (exact ones by their canonical
        // address: a mirror is the same cell).
        let exact: Vec<Tok> = self
            .toks
            .range(Tok::Num(RAM.start)..Tok::Num(RAM.end))
            .copied()
            .collect();
        let mut covered = 0;
        let mut gone: Vec<Tok> = Vec::new();
        for lo in froms {
            if lo < covered {
                gone.push(Tok::RamFrom(lo));
                continue;
            }
            let end = ram_bound(lo);
            covered = end;
            for t in &exact {
                if let Tok::Num(c) = *t
                    && (lo..end).contains(&ram_canon(c))
                {
                    gone.push(*t);
                }
            }
        }
        for t in gone {
            self.toks.remove(&t);
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

    /// It holds a [`Tok::Ref`] not yet expanded.
    fn marked(&self) -> bool {
        self.toks.range(REF0..).next().is_some()
    }

    /// Its [`Tok::Ref`]s.
    fn refs(&self) -> impl Iterator<Item = Tok> + '_ {
        self.toks.range(REF0..).copied()
    }

    fn fns(&self) -> impl Iterator<Item = (u32, Origin, Form)> + '_ {
        debug_assert!(!self.marked(), "a memory mark not expanded");
        self.toks.iter().filter_map(|t| match *t {
            Tok::Fn { key, origin, form } => Some((key, origin, form)),
            _ => None,
        })
    }

    /// Each token, and `None` for the flags' unknown part.
    fn parts(&self) -> Vec<Option<Tok>> {
        debug_assert!(!self.marked(), "a memory mark not expanded");
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
    /// The loads that read everything (`native/fnref-unknown-addrs`):
    /// through a pointer into RAM the run dropped ([`Val::lost_ptr`]), so
    /// every RAM cell, every object and the pile. In any run of their
    /// function, an early round's too: what a load read then stays in
    /// what the run holds.
    pub everything: BTreeSet<u32>,
    /// The run didn't settle: nothing is proven.
    pub unsettled: bool,
    /// It stopped at [`TOKENS`]: the tokens it held, after how many
    /// functions run.
    pub over: Option<(usize, usize)>,
    /// `GBAA_FNREF_OFF` as it was set, if any kind was off: the run is
    /// a measurement, not one to build from ([`off_env`]).
    pub off: String,
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
    /// [`G::ref_val`]'s, brought up to date with [`G::mem_log`] when
    /// looked at again.
    ref_exp: std::collections::HashMap<Tok, Exp, Fx>,
    /// Each token stored new to the pile, a RAM cell or an object's cell,
    /// with the width stored, in order: what a kept [`Exp`] hasn't read.
    /// A cell's the first time any cell gets it (`cells_all`, `objs_all`:
    /// a read of every cell holds it once, and Emerald's four thousand
    /// cells get the same tokens).
    mem_log: Vec<(MemKind, Tok, u8)>,
    cells_all: Mem,
    objs_all: Mem,
    /// Those that read ROM alone, kept for good.
    ref_rom: std::collections::HashMap<Tok, Val, Fx>,
    /// [`G::expand`]'s of a value (op 0), and an op's result on large
    /// values (a shift's, `mvn`'s, a binary op's or compare's:
    /// [`G::op_kept`]), by the op and each operand's set (the kept one of
    /// its tokens, [`Sets`]) and flags: the Refs' values it was made of,
    /// and it.
    #[allow(clippy::type_complexity)]
    expanded: std::collections::HashMap<OpKey, ([Toks; 2], Vec<Val>, Val), Fx>,
    /// [`Run::split_refs`]' tokens a map leaves of a Ref's value, by the
    /// Ref and map, with the value's set they came of (the same while it
    /// is).
    split_rest: std::collections::HashMap<(Tok, Xf), (Toks, Toks), Fx>,
    /// The [`Tok::Ref`]s stored to the pile (their origins are piled too).
    piled_refs: BTreeSet<Tok>,
    /// What a load at a width reads of a function's spills, and of the
    /// cells at an address (kept until they change).
    spill_reads: std::collections::HashMap<(Ix, u8), Val, Fx>,
    cell_at_reads: std::collections::HashMap<(u32, u8), Val, Fx>,
    /// What a load at a width through a pointer anywhere in an object
    /// reads of its cells ([`G::read_obj`]), until a store reaches it.
    obj_any_reads: std::collections::HashMap<(u32, u8), Val, Fx>,
    /// The origins in [`Flow::holes`], to look up fast.
    holed: std::collections::HashSet<Origin, Fx>,
    /// Large token sets whose every origin is in [`Flow::holes`], by
    /// address: a hole of one again adds nothing. Each held weakly, which
    /// keeps its address from being reused and none of its tokens (held
    /// whole, they were 845 sets of 9 million tokens on Emerald, each a
    /// function's own; a set that is changed moves, `Rc::make_mut`).
    all_holed: std::collections::HashMap<*const BTreeSet<Tok>, std::rc::Weak<BTreeSet<Tok>>, Fx>,
    /// What incomplete callee sets pass.
    loose_args: Vec<Val>,
    /// Functions whose address is taken.
    taken: BTreeSet<u32>,
    /// What a read from ROM "from here on" gives, by (start, end, width).
    rom_from: BTreeMap<(u32, u32, u8), Val>,
    /// The objects in ROM data ([`objects`]).
    objects: BTreeSet<u32>,
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
    /// The functions nothing the run saw calls, directly or by an address
    /// it read, once it settled without them: their callers are out of
    /// sight (a vtable word no load reached), and so is what they are
    /// passed. A call of theirs through a target that holds no function
    /// may be any whose address is taken ([`Run::callees`]).
    orphans: BTreeSet<u32>,
    /// The sets kept, each once ([`Sets`]).
    sets: Sets,
    /// `GBAA_FNREF_WATCH`: the words watched (key, origin), or every ROM
    /// word (`all`), whether the roads are printed, the lines said and
    /// how many of each word (its places, and its holes:
    /// [`WATCH_LINES`]), the function and instruction running, and, when
    /// the roads are printed, each origin as it gets its first hole
    /// ([`G::watch`], [`G::roads`]).
    watch: Vec<(u32, Origin)>,
    watch_all: bool,
    roads: bool,
    watch_said: BTreeSet<String>,
    watch_lines: BTreeMap<(u32, bool), usize>,
    now: (u32, u32),
    hole_log: Vec<Origin>,
    /// `GBAA_FNREF_OFF`'s kinds ([`off_env`]).
    off: u8,
}

/// A kept op's key ([`G::op_kept`]): the op, a shift's amount, each
/// operand's set (null: empty, or no operand) and flags.
type OpKey = (u8, Option<u32>, [(*const BTreeSet<Tok>, Flags); 2]);

/// A value's flags: [`Val::mem`], [`Val::unk`], [`Val::lost_rom`],
/// [`Val::lost_ptr`].
type Flags = (bool, bool, bool, bool);

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
        if off == OBJ_ANY && self.off & OFF_ANY != 0 {
            self.read_cell(obj_addr(i, OBJ_ANY), w, into);
        } else if off == OBJ_ANY {
            let (lo, hi) = (obj_addr(i, -OBJ_WIN), obj_addr(i, OBJ_ANY) + 3);
            if !self.watch.is_empty() {
                self.read_range(lo, hi, w, "by a pointer anywhere in it", into);
            } else {
                // Kept until a store reaches the object (read afresh at
                // each load, it was most of Emerald's first round).
                self.rd.ranges.push((lo, hi));
                let v = match self.obj_any_reads.get(&(i, w)) {
                    Some(v) => v.clone(),
                    None => {
                        let mut v = Val::default();
                        for (_, m) in self.cells.range(lo..=hi) {
                            from_mem(m, w, &mut v);
                        }
                        self.obj_any_reads.insert((i, w), v.clone());
                        v
                    }
                };
                into.union(&v);
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

    /// What a load at width `w` reads of every object's cells: its
    /// [`Tok::Ref`] ([`G::read_mem`], as [`G::read_cells`] for RAM's).
    fn read_objs(&mut self, w: u8, into: &mut Val) {
        self.read_mem(MemKind::Objs, w, into);
    }

    /// What a load at width `w` reads of the cells from `lo` to `hi`.
    fn read_range(&mut self, lo: u32, hi: u32, w: u8, how: &str, into: &mut Val) {
        self.rd.ranges.push((lo, hi));
        for (_, m) in self.cells.range(lo..=hi) {
            from_mem(m, w, into);
        }
        if !self.watch.is_empty() {
            let cs: Vec<u32> = self.cells.range(lo..=hi).map(|c| *c.0).collect();
            for c in cs {
                let mut v = Val::default();
                from_mem(&self.cells[&c], w, &mut v);
                let name = self.cell_name(c);
                self.watch(&v, || format!("read w{w} from {name}, {how}"));
            }
        }
    }

    fn hole(&mut self, v: &Val, at: u32, why: Why, only: impl Fn(Form) -> bool) {
        let e;
        let v = if v.marked() {
            e = self.expand(v);
            &e
        } else {
            v
        };
        let big = v.toks.len() >= 64;
        let p = std::rc::Rc::as_ptr(&v.toks.0);
        if !self.watch.is_empty() {
            let n = v.toks.len();
            self.watch_if(v, &only, &|| {
                format!("HOLE, {}, in a value of {n} tokens", why.what())
            });
        }
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
                if self.roads {
                    self.hole_log.push(o);
                }
            } else {
                all = false;
            }
        }
        if big && all {
            if self.all_holed.len() >= 1024 {
                self.all_holed.clear();
            }
            self.all_holed.insert(p, std::rc::Rc::downgrade(&v.toks.0));
        }
    }

    /// [`G::hole`] of one function's token, all forms.
    fn hole_one(&mut self, o: Origin, at: u32, why: Why) {
        if self.watching(o) {
            self.watch_line(o, &format!("HOLE, {}, of it alone", why.what()));
        }
        if self.holed.insert(o) {
            self.flow.holes.insert(o, (at, why));
            if self.roads {
                self.hole_log.push(o);
            }
        }
    }

    /// What a load at width `w` reads of function `f`'s spills.
    fn read_spill(&mut self, f: Ix, w: u8, into: &mut Val) {
        if self.off & OFF_SPILL != 0 {
            return;
        }
        let v = self.spill_reads.entry((f, w)).or_insert_with(|| {
            let mut v = Val::default();
            if let Some(m) = self.spills.get(&f) {
                from_mem(m, w, &mut v);
            }
            v
        });
        into.union(v);
        if !self.watch.is_empty() {
            let v = self.spill_reads[&(f, w)].clone();
            self.watch(&v, || {
                format!("read w{w} from f_{:08x}'s spills", entry_of(f))
            });
        }
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
        if !self.watch.is_empty() {
            let v = self.cell_at_reads[&(c, w)].clone();
            let name = self.cell_name(c);
            self.watch(&v, || format!("read w{w} from {name}"));
        }
    }

    /// A cell's name for [`G::watch`]: a RAM address, or an object's
    /// offset.
    fn cell_name(&self, c: u32) -> String {
        if c == LOST_CELL {
            "the lost-pointer cell".into()
        } else if c >= OBJ_BASE {
            let (i, at) = ((c - OBJ_BASE) >> 16, (c - OBJ_BASE) & 0xffff);
            let o = self.objs.get(i as usize);
            if at >= 0xfff8 {
                format!("object {i} ({o:x?}) anywhere")
            } else {
                format!("object {i} ({o:x?}) at {}", at as i32 - 0x8000)
            }
        } else {
            format!("the cell at {c:#010x}")
        }
    }

    fn store(&mut self, to: Store, toks: &[(Tok, u8)]) {
        if !self.watch.is_empty() {
            let name = match to {
                Store::Pile => "the pile".to_string(),
                Store::Cell(a) => self.cell_name(a),
                Store::Spill(f) => format!("f_{:08x}'s spills", entry_of(f)),
            };
            let v = Val {
                toks: toks.iter().map(|t| t.0).collect(),
                ..Val::default()
            };
            self.watch(&v, || format!("stored to {name}"));
        }
        let m = match to {
            // One pile: splitting it by the value-set analysis's bounds on
            // the stores' addresses proved nothing more on the agb programs
            // (their heap stores aren't bounded) and cost nine times the
            // time.
            Store::Pile => {
                for (t, _) in toks {
                    match t {
                        Tok::Fn { origin, .. } => {
                            self.flow.piled.insert(*origin);
                        }
                        Tok::Ref { .. } => {
                            self.piled_refs.insert(*t);
                        }
                        _ => {}
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
                    self.mem_log
                        .extend(new.iter().map(|&(t, sw)| (MemKind::Pile, t, sw)));
                    self.pile_changed = true;
                    Some(&mut self.pile_raw)
                }
                Store::Cell(a) => {
                    let objs = a >= OBJ_BASE;
                    if objs {
                        let i = (a - OBJ_BASE) >> 16;
                        for w in [1u8, 2, 4] {
                            self.obj_any_reads.remove(&(i, w));
                        }
                    }
                    let (kind, all) = if objs {
                        self.obj_reads = Default::default();
                        (MemKind::Objs, &mut self.objs_all)
                    } else {
                        self.cell_reads = Default::default();
                        (MemKind::Cells, &mut self.cells_all)
                    };
                    for &(t, sw) in &new {
                        if all.insert((t, sw)) {
                            self.mem_log.push((kind, t, sw));
                        }
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
    fn pile_read(&mut self, w: u8) -> &Val {
        let i = w as usize;
        self.rd.pile |= 1 << w;
        if self.pile_reads[i].is_none() {
            let mut v = self.pile_raw[i].clone();
            v.cap();
            self.pile_reads[i] = Some(v);
        }
        self.pile_reads[i].as_ref().unwrap()
    }

    /// What a load at width `w` from anywhere reads of the cells (kept
    /// until a cell changes: reading three thousand of them at each such
    /// load was most of a large game's run).
    fn cells_read(&mut self, w: u8) -> &Val {
        let i = w as usize;
        self.rd.all_cells |= 1 << w;
        if self.cell_reads[i].is_none() {
            let mut v = self.cell_raw[i].clone();
            v.cap();
            self.cell_reads[i] = Some(v);
        }
        self.cell_reads[i].as_ref().unwrap()
    }

    /// What a load at width `w` reads of every object's cells, the same
    /// way.
    fn objs_read(&mut self, w: u8) -> &Val {
        let i = w as usize;
        self.rd.all_objs |= 1 << w;
        if self.obj_reads[i].is_none() {
            let mut v = self.obj_raw[i].clone();
            v.cap();
            self.obj_reads[i] = Some(v);
        }
        self.obj_reads[i].as_ref().unwrap()
    }

    /// A load at width `w` reads the pile (or every RAM cell, or every
    /// object's cells): its [`Tok::Ref`] and the read's flags.
    fn read_mem(&mut self, k: MemKind, w: u8, into: &mut Val) {
        let off = match k {
            MemKind::Pile => OFF_PILE,
            MemKind::Cells => OFF_CELLS,
            MemKind::Objs => 0,
        };
        if self.off & off != 0 {
            return;
        }
        let r = match k {
            MemKind::Pile => self.pile_read(w),
            MemKind::Cells => self.cells_read(w),
            MemKind::Objs => self.objs_read(w),
        };
        let (mem, unk, lost_rom, lost_ptr) = (r.mem, r.unk, r.lost_rom, r.lost_ptr);
        into.join(&Val {
            toks: [mem_ref(k, w)].into_iter().collect(),
            mem,
            unk,
            lost_rom,
            lost_ptr,
        });
    }

    fn read_pile(&mut self, w: u8, into: &mut Val) {
        self.read_mem(MemKind::Pile, w, into);
    }

    fn read_cells(&mut self, w: u8, into: &mut Val) {
        self.read_mem(MemKind::Cells, w, into);
    }

    /// What [`Tok::Ref`] `r` holds now, with no `Ref` in it (kept, and
    /// brought up to date with what memory gained since: made again at
    /// each store, a large game's 30k-token reads were most of its run);
    /// the running function reads the memory it does.
    fn ref_val(&mut self, r: Tok) -> Val {
        if let Some(v) = self.ref_rom.get(&r) {
            return v.clone();
        }
        if let Some(e) = self.ref_exp.get(&r)
            && e.at == self.mem_log.len()
        {
            self.rd.pile |= e.pile;
            self.rd.all_cells |= e.cells;
            self.rd.all_objs |= e.objs;
            return e.v.clone();
        }
        let mut e = match self.ref_exp.remove(&r) {
            Some(e) => e,
            None => {
                let Tok::Ref { src, xf } = r else {
                    unreachable!("not a Ref: {r:?}")
                };
                let mut e = Exp {
                    at: self.mem_log.len(),
                    ..Exp::default()
                };
                self.exp_add(&mut e, vec![(src, xf)]);
                self.sets.share(&mut e.v.toks);
                // Of ROM alone: it never changes.
                if e.pile | e.cells | e.objs == 0 {
                    self.ref_rom.insert(r, e.v.clone());
                    return e.v;
                }
                e
            }
        };
        if e.at < self.mem_log.len() {
            let was = (std::rc::Rc::as_ptr(&e.v.toks.0), e.v.toks.len());
            self.exp_update(&mut e);
            if was != (std::rc::Rc::as_ptr(&e.v.toks.0), e.v.toks.len()) {
                self.sets.share(&mut e.v.toks);
            }
        }
        self.rd.pile |= e.pile;
        self.rd.all_cells |= e.cells;
        self.rd.all_objs |= e.objs;
        let v = e.v.clone();
        self.ref_exp.insert(r, e);
        v
    }

    /// Reads `work` (and the reads it reaches through the `Ref`s memory
    /// holds, each with the forms the `Ref`s on the way give) into `e`,
    /// those it hasn't read yet, whole. Memory is read as stored, uncapped,
    /// so that what it gains later can be added alone ([`exp_merge`] caps).
    fn exp_add(&mut self, e: &mut Exp, mut work: Vec<(Src, Xf)>) {
        let mut d = Val::default();
        while let Some((src, xf)) = work.pop() {
            if !e.seen.insert((src, xf)) {
                continue;
            }
            let read = match src {
                Src::Cells(w) => {
                    e.cells |= 1 << w;
                    self.cell_raw[w as usize].clone()
                }
                Src::Pile(w) => {
                    e.pile |= 1 << w;
                    self.pile_raw[w as usize].clone()
                }
                Src::Objs(w) => {
                    e.objs |= 1 << w;
                    self.obj_raw[w as usize].clone()
                }
                Src::RomFrom(lo, end, w) => self.rom_from[&(lo, end, w)].clone(),
                Src::Hull(lo, hi, w) => self.rom_hull[&(lo, hi, w)].clone(),
            };
            exp_take(&mut d, &read, xf, &mut work);
        }
        exp_merge(e, d);
    }

    /// Brings `e` up to date: what was stored since it was made, read
    /// through each memory read it holds, and whatever new reads that
    /// reaches, whole.
    fn exp_update(&mut self, e: &mut Exp) {
        let mut work = Vec::new();
        let mut d = Val::default();
        let mems: Vec<(Src, Xf)> = e
            .seen
            .iter()
            .filter(|(s, _)| matches!(s, Src::Pile(_) | Src::Cells(_) | Src::Objs(_)))
            .copied()
            .collect();
        // What each read gained (the pile's, every cell's or every
        // object's, at a width),
        // made once and taken through each of the forms it is seen by: a
        // value a token and form at a time was most of Emerald's third
        // round.
        let mut gained: BTreeMap<Src, Val> = BTreeMap::new();
        for &(src, xf) in &mems {
            let read = gained.entry(src).or_insert_with(|| {
                let mut read = Val::default();
                let kind = match src {
                    Src::Pile(_) => MemKind::Pile,
                    Src::Cells(_) => MemKind::Cells,
                    _ => MemKind::Objs,
                };
                for &(k, t, sw) in &self.mem_log[e.at..] {
                    if k == kind {
                        from_mem_one(t, sw, src.w(), &mut read);
                    }
                }
                read
            });
            exp_take(&mut d, read, xf, &mut work);
        }
        e.at = self.mem_log.len();
        exp_merge(e, d);
        work.retain(|x| !e.seen.contains(x));
        if !work.is_empty() {
            self.exp_add(e, work);
        }
    }

    /// `v` with its [`Tok::Ref`]s taken in, to look at its tokens. A
    /// `Ref` alone is its kept value itself, shared.
    fn expand(&mut self, v: &Val) -> Val {
        if !v.marked() {
            return v.clone();
        }
        let k = match self.op_kept(0, None, [v, &Val::default()]) {
            Ok(out) => return out,
            Err(k) => k,
        };
        let mut out = v.clone();
        out.drop_range(REF0..);
        for x in &k.2 {
            out.union(x);
        }
        out.cap();
        self.sets.share(&mut out.toks);
        self.op_keep(k, &out);
        out
    }

    /// The kept result of `op` (with `n`) on `vs`, if they have the
    /// tokens and flags it was made from and their Refs the values they
    /// had then; else what [`G::op_keep`] keeps the result by. By the
    /// kept sets of their tokens, so that equal values find it: in
    /// Emerald's second round most functions shift, compare and expand
    /// values equal to another function's (two thirds of that round's
    /// time).
    #[allow(clippy::type_complexity)]
    fn op_kept(
        &mut self,
        op: u8,
        n: Option<u32>,
        vs: [&Val; 2],
    ) -> Result<Val, (OpKey, [Toks; 2], Vec<Val>)> {
        let refs: Vec<Tok> = vs.iter().flat_map(|v| v.refs()).collect();
        let parts: Vec<Val> = refs.iter().map(|&r| self.ref_val(r)).collect();
        let same =
            |a: &Val, b: &Val| std::rc::Rc::ptr_eq(&a.toks.0, &b.toks.0) && a.flags() == b.flags();
        let key = |ts: &[Toks; 2]| {
            let of = |i: usize| {
                let p = if ts[i].is_empty() {
                    std::ptr::null()
                } else {
                    std::rc::Rc::as_ptr(&ts[i].0)
                };
                (p, vs[i].flags())
            };
            (op, n, [of(0), of(1)])
        };
        let mut kept = [vs[0].toks.clone(), vs[1].toks.clone()];
        for shared in [false, true] {
            if shared {
                for t in &mut kept {
                    self.sets.share(t);
                }
            }
            if let Some((_, was, out)) = self.expanded.get(&key(&kept))
                && was.len() == parts.len()
                && was.iter().zip(&parts).all(|(a, b)| same(a, b))
            {
                return Ok(out.clone());
            }
        }
        Err((key(&kept), kept, parts))
    }

    fn op_keep(&mut self, (key, kept, parts): (OpKey, [Toks; 2], Vec<Val>), out: &Val) {
        if self.expanded.len() >= 4096 {
            self.expanded.clear();
        }
        self.expanded.insert(key, (kept, parts, out.clone()));
    }

    /// `v` as memory keeps it at width `w`: each [`Tok::Ref`] with the
    /// forms storing it there makes ([`store_form`]), and the Refs some
    /// of whose functions can't be stored so (holes, [`to_mem`]'s `bad`).
    fn for_mem(&mut self, v: &Val, w: u8) -> (Val, Vec<Tok>) {
        if v.refs().all(|t| ref_keeps(t, w)) {
            return (v.clone(), Vec::new());
        }
        let refs: Vec<Tok> = v.refs().filter(|t| !ref_keeps(*t, w)).collect();
        let mut out = v.clone();
        let mut bad = Vec::new();
        for r in refs {
            out.toks.remove(&r);
            let Tok::Ref { src, xf } = r else {
                continue;
            };
            if read_forms(src)
                .iter()
                .any(|&f| xf.form(f).is_some_and(|f| store_form(f, w).is_none()))
            {
                bad.push(r);
            }
            let to = Xf {
                roms: w == 4,
                rom_mem: w != 4,
                flags: true,
                // A RAM pointer stored narrower: a piece of one.
                ptrs: if w == 4 { Xf::PTR_KEEP } else { Xf::PTR_PIECE },
                ..Xf::of(|f| store_form(f, w))
            };
            out.toks.insert(Tok::Ref {
                src,
                xf: xf.then(to),
            });
        }
        (out, bad)
    }

    /// Whether `v`, its [`Tok::Ref`]s taken in, may hold a dropped
    /// address ([`Val::lost_rom`]).
    fn lost(&mut self, v: &Val) -> bool {
        v.lost_rom || {
            let refs: Vec<Tok> = v.refs().collect();
            refs.into_iter().any(|r| self.ref_val(r).lost_rom)
        }
    }

    /// The same for a RAM pointer dropped ([`Val::lost_ptr`]).
    fn lost_ptr(&mut self, v: &Val) -> bool {
        v.lost_ptr || {
            let refs: Vec<Tok> = v.refs().collect();
            refs.into_iter().any(|r| self.ref_val(r).lost_ptr)
        }
    }

    fn give(&mut self, f: Ix, regs: &[Val]) {
        let e = self
            .entry
            .entry(f)
            .or_insert_with(|| vec![Val::default(); 16]);
        let mut c = false;
        for (a, b) in e.iter_mut().zip(regs) {
            if a.join_capped(b) {
                self.sets.share(&mut a.toks);
                c = true;
            }
        }
        if c {
            self.dirty_entry.insert(f);
        }
        self.mark(c);
        self.watch_regs(regs, |r| format!("passed in r{r} to f_{:08x}", entry_of(f)));
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
        let n = ks.len();
        self.watch_regs(regs, |r| format!("passed in r{r} to a callee set of {n}"));
        // By register and the set it held (kept alive here, so its
        // address isn't reused) with its flags: what it joined to.
        #[allow(clippy::type_complexity)]
        let mut memo: std::collections::HashMap<
            (usize, *const BTreeSet<Tok>, Flags),
            (Toks, Val, bool),
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
                    if a.join_capped(b) {
                        self.sets.share(&mut a.toks);
                        c = true;
                    }
                    continue;
                }
                let key = (i, std::rc::Rc::as_ptr(&a.toks.0), a.flags());
                match memo.get(&key) {
                    Some((_, v, grew)) => {
                        *a = v.clone();
                        c |= grew;
                    }
                    None => {
                        let old = a.toks.clone();
                        let grew = a.join_capped(b);
                        if grew {
                            self.sets.share(&mut a.toks);
                        }
                        memo.insert(key, (old, a.clone(), grew));
                        c |= grew;
                    }
                }
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
            count(
                "obj_any_reads",
                &mut self.obj_any_reads.values().map(|v| &v.toks),
            ),
            // The kept reads and expansions (`native/fnref-flow-emerald`:
            // they held 11 million tokens the bound didn't count).
            count(
                "refs",
                &mut self
                    .ref_exp
                    .values()
                    .map(|e| &e.v.toks)
                    .chain(self.ref_rom.values().map(|v| &v.toks)),
            ),
            count(
                "expanded",
                &mut self.expanded.values().flat_map(|(a, ps, o)| {
                    a.iter()
                        .chain(ps.iter().map(|p| &p.toks))
                        .chain(std::iter::once(&o.toks))
                }),
            ),
            count(
                "split_rest",
                &mut self.split_rest.values().flat_map(|(a, b)| [a, b]),
            ),
            count(
                "reads",
                &mut self
                    .pile_raw
                    .iter()
                    .chain(&self.cell_raw)
                    .chain(&self.obj_raw)
                    .chain(self.pile_reads.iter().flatten())
                    .chain(self.cell_reads.iter().flatten())
                    .chain(self.obj_reads.iter().flatten())
                    .chain(&self.loose_args)
                    .chain(self.loose_rets.iter().flatten())
                    .map(|v| &v.toks),
            ),
            (
                "memory",
                0,
                self.pile.len()
                    + mems
                    + self.mem_log.len()
                    + self.cells_all.len()
                    + self.objs_all.len()
                    + self.ref_exp.values().map(|e| e.raw.len()).sum::<usize>(),
            ),
        ]
    }

    /// For the trace: the kept sets by store, counted by pointer and by
    /// content (equal sets held apart are copies: what took Emerald to
    /// [`TOKENS`] before [`Sets`]), and the sets with the most tokens in
    /// copies.
    /// For the trace: the entries' tokens (each distinct set once) by
    /// kind, and how many entries hold a lost RAM pointer.
    fn entry_kinds(&self) -> String {
        let mut seen = std::collections::HashSet::new();
        let mut n = [0usize; 7];
        let mut lost = 0;
        for v in self.entry.values().flatten() {
            lost += v.lost_ptr as usize;
            if !seen.insert(std::rc::Rc::as_ptr(&v.toks.0)) {
                continue;
            }
            for t in v.toks.iter() {
                n[match t {
                    Tok::Fn { .. } => 0,
                    Tok::Num(c) if is_ram(*c) => 2,
                    Tok::Num(_) => 1,
                    Tok::Rom(_) | Tok::RomFrom(_) => 3,
                    Tok::RamFrom(_) => 4,
                    Tok::Obj(..) => 5,
                    Tok::Ref { .. } => 6,
                }] += 1;
            }
        }
        format!(
            "functions' {}, numbers {}, RAM addresses {}, ROM pointers {}, RAM runs {}, objects {}, Refs {}; {lost} registers a lost pointer",
            n[0], n[1], n[2], n[3], n[4], n[5], n[6]
        )
    }

    fn held_copies(&self) -> String {
        use std::hash::{Hash, Hasher};
        let stores: [(&str, Vec<&Toks>); 5] = [
            (
                "entry",
                self.entry.values().flatten().map(|v| &v.toks).collect(),
            ),
            (
                "ret",
                self.ret.values().flatten().map(|v| &v.toks).collect(),
            ),
            (
                "by_value",
                self.by_value
                    .values()
                    .flat_map(|b| b.args.iter().chain(&b.rets))
                    .map(|v| &v.toks)
                    .collect(),
            ),
            (
                "refs",
                self.ref_exp
                    .values()
                    .map(|e| &e.v.toks)
                    .chain(self.ref_rom.values().map(|v| &v.toks))
                    .collect(),
            ),
            (
                "expanded and split_rest",
                self.expanded
                    .values()
                    .flat_map(|(a, ps, o)| {
                        a.iter()
                            .chain(ps.iter().map(|p| &p.toks))
                            .chain(std::iter::once(&o.toks))
                    })
                    .chain(self.split_rest.values().flat_map(|(a, b)| [a, b]))
                    .collect(),
            ),
        ];
        let mut s = String::new();
        let mut ptrs = std::collections::HashSet::new();
        // By content: the copies, their length, and the stores they're in.
        let mut all: std::collections::HashMap<u64, (usize, usize, Vec<&str>)> = Default::default();
        for (name, sets) in &stores {
            let (mut n, mut t, mut np, mut tp) = (0usize, 0usize, 0usize, 0usize);
            for v in sets {
                if v.is_empty() || !ptrs.insert(std::rc::Rc::as_ptr(&v.0)) {
                    continue;
                }
                np += 1;
                tp += v.len();
                let mut h = std::collections::hash_map::DefaultHasher::new();
                v.len().hash(&mut h);
                for x in v.iter() {
                    x.hash(&mut h);
                }
                let e = all.entry(h.finish()).or_insert((0, v.len(), Vec::new()));
                e.0 += 1;
                if !e.2.contains(name) {
                    e.2.push(name);
                }
                if e.0 == 1 {
                    n += 1;
                    t += v.len();
                }
            }
            writeln!(
                s,
                "  held {name}: {np} sets of {tp} tokens by pointer, {n} of {t} of them not equal to one before"
            )
            .unwrap();
        }
        let mut top: Vec<&(usize, usize, Vec<&str>)> = all.values().filter(|e| e.0 > 1).collect();
        top.sort_by_key(|e| std::cmp::Reverse((e.0 - 1) * e.1));
        for (copies, len, names) in top.into_iter().take(8) {
            writeln!(s, "  held set: {len} tokens, {copies} times, in {names:?}").unwrap();
        }
        s
    }

    fn give_ret(&mut self, f: Ix, regs: &[Val]) {
        self.watch_regs(regs, |r| {
            format!("returned in r{r} by f_{:08x}", entry_of(f))
        });
        let e = self.ret.entry(f).or_insert_with(|| vec![Val::default(); 4]);
        let mut c = false;
        for (a, b) in e.iter_mut().zip(regs) {
            if a.join_capped(b) {
                self.sets.share(&mut a.toks);
                c = true;
            }
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

/// `v` is the number `n` and nothing else.
fn is_num(v: &Val, n: u32) -> bool {
    !v.unk
        && !v.mem
        && !v.lost_rom
        && !v.lost_ptr
        && v.toks.len() == 1
        && v.toks.contains(&Tok::Num(n))
}

fn bytes(w: Width) -> u8 {
    match w {
        Width::Byte => 1,
        Width::Half => 2,
        Width::Word => 4,
    }
}

/// RAM's addresses (EWRAM and IWRAM, with their mirrors).
const RAM: std::ops::Range<u32> = 0x0200_0000..0x0400_0000;

fn is_ram(a: u32) -> bool {
    RAM.contains(&a)
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
            Tok::Fn { key, origin, form } => match store_form(form, w) {
                Some(form) => out.push((Tok::Fn { key, origin, form }, w)),
                None => bad.push(form),
            },
            // As it is ([`G::for_mem`] expands any other).
            Tok::Ref { .. } => {
                debug_assert!(
                    ref_keeps(t, w),
                    "a Ref memory doesn't keep: {t:?} at width {w}"
                );
                out.push((t, w));
            }
            Tok::Rom(_) | Tok::RomFrom(_) | Tok::Obj(..) if w == 4 => out.push((t, 4)),
            // A RAM address: a pointer, kept.
            // A piece of one: from here on, as an object's piece.
            Tok::Num(c) if is_ram(c) => out.push((
                if w == 4 {
                    t
                } else {
                    Tok::RamFrom(ram_canon(c))
                },
                w,
            )),
            Tok::RamFrom(_) => out.push((t, w)),
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
    if v.lost_ptr {
        out.push((LOST_PTR, w));
    }
    (out, bad)
}

/// ROM read `v` (from `src`), or if it is large its [`Tok::Ref`] (with its
/// flags): a large game's tables hold thousands of functions' addresses.
fn rom_ref(v: &Val, src: Src) -> Val {
    if v.toks.len() < 64 {
        return v.clone();
    }
    Val {
        toks: [Tok::Ref { src, xf: Xf::ID }].into_iter().collect(),
        ..v.clone()
    }
}

/// Whether memory keeps [`Tok::Ref`] `t` stored at width `w` as it is:
/// storing it there changes none of its tokens ([`to_mem`]: a read at `w`
/// stored at `w`, bytes at any width, a reassembled word as a word).
fn ref_keeps(t: Tok, w: u8) -> bool {
    let Tok::Ref { src, xf } = t else {
        return false;
    };
    // Each form the read holds, as the Ref makes it, stored at `w` as it
    // is. (A ROM read's numbers come back as themselves, not as memory's
    // mark: more exact, as compiled code reads them back. A RAM pointer
    // stored narrower is a piece of one.)
    (!xf.roms || w == 4)
        && (xf.ptrs != Xf::PTR_KEEP || w == 4)
        && read_forms(src)
            .iter()
            .filter_map(|&f| xf.form(f))
            .all(|f| store_form(f, w) == Some(f))
}

/// The forms of functions' addresses a read from `src` holds
/// ([`from_mem_one`] of what [`to_mem`] stores, or [`Run::rom_read`]'s).
fn read_forms(src: Src) -> &'static [Form] {
    match (src, src.w()) {
        (Src::RomFrom(..) | Src::Hull(..), 4) => &[Form::Full, Form::Part],
        (_, 4) => &[
            Form::Full,
            Form::Hi,
            Form::Lo,
            Form::HiUp,
            Form::LoUp,
            Form::Part,
        ],
        (_, 2) => &[Form::Hi, Form::Lo, Form::Part],
        _ => &[Form::Part],
    }
}

/// The form a function's address of form `form` stored at width `w` has
/// in memory (`None`: it can't be stored, a hole).
fn store_form(form: Form, w: u8) -> Option<Form> {
    match (form, w) {
        (Form::Full, 4) => Some(Form::Full),
        (Form::Full, 2) => Some(Form::Lo),
        (Form::Full | Form::Part, _) => Some(Form::Part),
        (Form::Lo | Form::Hi, 2 | 4) => Some(form),
        (Form::HiUp | Form::LoUp, 4) => Some(form),
        _ => None,
    }
}

/// In memory: something stored that isn't a function's or ROM address.
const OTHER: Tok = Tok::Num(0);
/// In memory: a value with [`Val::lost_rom`] stored (no other number is
/// kept in memory: [`to_mem`] makes each [`OTHER`]).
const LOST: Tok = Tok::Num(1);
/// In memory: a value with [`Val::lost_ptr`] stored, or a piece of a
/// pointer into an object.
const LOST_PTR: Tok = Tok::Num(2);

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
        Tok::Num(c) if is_ram(c) && w == 4 && sw == 4 => t,
        Tok::RamFrom(_) if w == 4 && sw == 4 => t,
        // A piece of a RAM address, or pieces of one read as a word: from
        // here on.
        Tok::Num(c) if is_ram(c) => {
            into.unk = true;
            Tok::RamFrom(ram_canon(c))
        }
        Tok::RamFrom(_) => {
            into.unk = true;
            t
        }
        // A piece of a pointer into an object, or pieces of one read as a
        // word: of the object still.
        Tok::Obj(i, _) => {
            into.unk = true;
            Tok::Obj(i, OBJ_ANY)
        }
        // Read at the width it was stored at, it is what was stored; a
        // word from pieces, reassembled; narrower, its bytes (RAM
        // pointers in it pieces of them, [`Xf::ptrs`]).
        Tok::Ref { src, xf } => Tok::Ref {
            src,
            xf: if sw == w {
                xf
            } else if w == 4 {
                xf.then(Xf::all(Form::Full, true))
            } else {
                xf.then(Xf::all(Form::Part, true))
            },
        },
        _ => {
            // A mark, not a value: nothing else came of it.
            if t == LOST {
                into.lost_rom = true;
                return;
            }
            if t == LOST_PTR {
                into.lost_ptr = true;
                into.unk = true;
                return;
            }
            into.mem = true;
            return;
        }
    };
    into.toks.insert(t);
}

/// A [`Tok::Ref`]'s value kept ([`G::ref_val`]): with the reads it is
/// made of, the widths of the pile and of every cell among them, and how
/// much of [`G::mem_log`] it has read.
#[derive(Default)]
struct Exp {
    v: Val,
    /// Every number and ROM pointer read into it, as read ([`exp_merge`]).
    raw: BTreeSet<Tok>,
    seen: BTreeSet<(Src, Xf)>,
    pile: u8,
    cells: u8,
    objs: u8,
    at: usize,
}

/// Takes `d`, read uncapped, into `e`. Its numbers and ROM pointers are
/// kept as read (`e.raw`), and `e.v` holds the cap of them all: capping
/// what each store adds onto the capped value would widen by the order of
/// the stores, and leave a pointer out that the capped read of memory
/// takes in, which is what [`follow`] reruns a function by (`snap`).
fn exp_merge(e: &mut Exp, mut d: Val) {
    let nums = Tok::Num(0)..REF0;
    d.drop_range(REF0..);
    let new: Vec<Tok> = d
        .toks
        .range(nums.clone())
        .filter(|t| !e.raw.contains(t))
        .copied()
        .collect();
    // A flag the cap widens by, new.
    let widens = (d.unk && !e.v.unk) || (d.lost_ptr && !e.v.lost_ptr);
    // Functions' tokens and the flags: the cap leaves them as they are.
    d.drop_range(nums.clone());
    e.v.union(&d);
    if new.is_empty() && !widens {
        return;
    }
    e.raw.extend(new);
    let mut p = Val {
        toks: e.raw.iter().copied().collect(),
        unk: e.v.unk,
        lost_ptr: e.v.lost_ptr,
        ..Val::default()
    };
    p.cap();
    e.v.unk |= p.unk;
    e.v.lost_ptr |= p.lost_ptr;
    if !p.toks.iter().eq(e.v.toks.range(nums.clone())) {
        e.v.drop_range(nums);
        e.v.toks.add_all(p.toks.iter().copied().collect());
    }
}

/// Takes `read`, a read seen through forms `xf`, into `v`; the `Ref`s it
/// holds go on `work`, with `xf` after theirs.
fn exp_take(v: &mut Val, read: &Val, xf: Xf, work: &mut Vec<(Src, Xf)>) {
    for t in read.refs() {
        if let Tok::Ref { src, xf: x } = t {
            work.push((src, x.then(xf)));
        }
    }
    if xf == Xf::ID {
        v.union(read);
        return;
    }
    if xf.flags {
        v.mem |= read.mem;
        v.unk |= read.unk;
        v.lost_rom |= read.lost_rom;
        v.lost_ptr |= read.lost_ptr;
    }
    let mut toks = Vec::new();
    for &t in read.toks.range(..REF0) {
        match t {
            Tok::Fn { key, origin, form } => {
                if let Some(form) = xf.form(form) {
                    toks.push(Tok::Fn { key, origin, form });
                }
            }
            // A RAM pointer ([`Xf::ptrs`]): kept, a piece of one (as
            // [`from_mem_one`] reads it), or left to the op's own run.
            Tok::Num(c) | Tok::RamFrom(c) if is_ram(c) && xf.ptrs == Xf::PTR_KEEP => toks.push(t),
            Tok::Obj(..) if xf.ptrs == Xf::PTR_KEEP => toks.push(t),
            Tok::Num(c) | Tok::RamFrom(c) if is_ram(c) && xf.ptrs == Xf::PTR_PIECE => {
                v.unk = true;
                toks.push(Tok::RamFrom(ram_canon(c)));
            }
            Tok::Obj(i, _) if xf.ptrs == Xf::PTR_PIECE => {
                v.unk = true;
                toks.push(Tok::Obj(i, OBJ_ANY));
            }
            Tok::Num(c) | Tok::RamFrom(c) if is_ram(c) => {}
            Tok::Obj(..) => {}
            _ if xf.roms => toks.push(t),
            // A ROM pointer's bytes: a number read from memory.
            _ => v.mem |= xf.rom_mem,
        }
    }
    if !toks.is_empty() {
        v.toks.add_all(toks);
    }
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
            let t = Tok::Fn {
                key: word,
                origin: o,
                form,
            };
            into.toks.insert(t);
            if self.g.watching(o) {
                self.g
                    .watch_line(o, &format!("read w{w} from ROM at {a:#010x} ({form:?})"));
            }
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

    /// What a load at width `w` reads of the ROM words from `lo` to `hi`
    /// (kept by bounds and width; a large read as its [`Tok::Ref`]).
    fn rom_range(&mut self, lo: u32, hi: u32, w: u8, into: &mut Val) {
        match self.g.rom_hull.get(&(lo, hi, w)) {
            Some(v) => {
                into.join(&rom_ref(v, Src::Hull(lo, hi, w)));
            }
            None => {
                let mut v = Val::default();
                let mut p = lo & !3;
                while p <= hi {
                    self.rom_read_capped(p, w, &mut v);
                    p += 4;
                }
                into.join(&rom_ref(&v, Src::Hull(lo, hi, w)));
                self.g.rom_hull.insert((lo, hi, w), v);
            }
        }
    }

    /// [`Run::rom_read_capped`] if `into` is as the cap leaves it
    /// (`capped`), else [`Run::rom_read`].
    fn rom_read_or_capped(&mut self, a: u32, w: u8, into: &mut Val, capped: bool) {
        if capped {
            self.rom_read_capped(a, w, into);
        } else {
            self.rom_read(a, w, into);
        }
    }

    /// [`Run::rom_read`] into `into` as [`Val::cap`] leaves it: a number
    /// or ROM pointer the cap would drop again (one held, one unknown takes
    /// in, one a pointer "from here on" covers) is left out without a cap
    /// (a cap at each of a long read's words was most of a large game's
    /// run).
    fn rom_read_capped(&mut self, a: u32, w: u8, into: &mut Val) {
        if let Some(word) = self.img.word(a)
            && !self.img.is_fn(word)
        {
            let sh = (a & 3) * 8;
            let v = match w {
                4 => word,
                2 => (word >> (sh & 16)) & 0xffff,
                _ => (word >> sh) & 0xff,
            };
            let t = num(self.img, v);
            let covered = match t {
                // A RAM address is a pointer: only a lost one takes it in.
                Tok::Num(c) if is_ram(c) => into.lost_ptr,
                Tok::Num(_) => into.unk,
                Tok::Rom(x) => into
                    .toks
                    .range(Tok::RomFrom(0)..=Tok::RomFrom(x))
                    .next_back()
                    .is_some_and(|f| matches!(*f, Tok::RomFrom(lo) if x < bound(lo))),
                _ => false,
            };
            if covered || into.toks.contains(&t) {
                return;
            }
        }
        self.rom_read(a, w, into);
    }

    /// Where the object a read "from here on" at `a` is in ends, before
    /// `hi`: the next object start ([`objects`]) past `a` that isn't
    /// inside a table of handlers (function words in the 16 bytes before
    /// it and the 16 from it on), or `hi`.
    fn object_end(&self, a: u32, hi: u32) -> u32 {
        let img = self.img;
        let fns_in = |lo: u32, hi: u32| {
            (lo & !3..hi)
                .step_by(4)
                .any(|p| img.word(p).is_some_and(|x| img.is_fn(x)))
        };
        self.g
            .objects
            .range(a.saturating_add(1)..hi)
            .copied()
            .find(|&c| {
                self.g.off & OFF_TABLES != 0
                    || !(fns_in(c.saturating_sub(16), c) && fns_in(c, c.saturating_add(16)))
            })
            .unwrap_or(hi)
    }

    /// A read "from here on" at `a` (`native/fnref-rom-read-bound`): the
    /// words of its object ([`Self::object_end`]), or, a BIOS copy's
    /// whose length the run knows, those `len` bytes; and of those only
    /// the ones in the load's value-set bounds (`hull`, in ROM, inclusive)
    /// where they meet.
    fn rom_from(
        &mut self,
        a: u32,
        w: u8,
        hull: Option<(u32, u32)>,
        len: Option<u32>,
        into: &mut Val,
    ) {
        let v = self.rom_from_val(a, w, hull, len);
        into.join(&v);
    }

    /// What [`Self::rom_from`] reads.
    fn rom_from_val(&mut self, a: u32, w: u8, hull: Option<(u32, u32)>, len: Option<u32>) -> Val {
        let bound = self.img.bound(a);
        let (mut lo, mut hi) = match len {
            Some(n) => (a, bound.min(a.saturating_add(n))),
            None => (a, self.object_end(a, bound)),
        };
        if let Some((l, h)) = hull
            && self.img.word(l).is_some()
            && self.img.word(h).is_some()
            && l.max(lo) < hi.min(h.saturating_add(1))
        {
            lo = l.max(lo);
            hi = hi.min(h.saturating_add(1));
        }
        let end = hi.min(lo.saturating_add(ROM_READ));
        if let Some(v) = self.g.rom_from.get(&(lo, end, w)) {
            return rom_ref(v, Src::RomFrom(lo, end, w));
        }
        let mut p = lo & !3;
        let mut v = Val {
            // Cut short: the words past the cut go unread.
            lost_rom: end < hi,
            ..Val::default()
        };
        if self.trace && end < hi {
            let at = self.at;
            trace_at("rom lost", at, || format!("{lo:#x}..{hi:#x} cut short"));
        }
        while p < end {
            self.rom_read_capped(p, w.min(4), &mut v);
            p += 4;
        }
        let r = rom_ref(&v, Src::RomFrom(lo, end, w));
        self.g.rom_from.insert((lo, end, w), v);
        r
    }

    /// `x` to run a per-token op on that makes a function's address of
    /// form `f` one of form `m(f)` (and `None`, something else): the
    /// tokens to run it on, and the Refs that stand for the rest of its
    /// result. A large Ref's functions `m` maps go in a Ref of their own
    /// mapped ([`Xf::then`]), not a copy; a small one is expanded.
    fn split_refs(&mut self, x: &Val, m: impl Fn(Form) -> Option<Form>) -> (Val, Vec<Tok>) {
        if !x.marked() {
            return (x.clone(), Vec::new());
        }
        let mut conc = x.clone();
        conc.drop_range(REF0..);
        let xf = Xf::of(&m);
        let mut refs = Vec::new();
        for r in x.refs().collect::<Vec<_>>() {
            let e = self.g.ref_val(r);
            if e.toks.len() < 64 {
                conc.union(&e);
                continue;
            }
            conc.mem |= e.mem;
            conc.unk |= e.unk;
            conc.lost_rom |= e.lost_rom;
            let rest = match self.g.split_rest.get(&(r, xf)) {
                Some((t, rest)) if std::rc::Rc::ptr_eq(&t.0, &e.toks.0) => rest.clone(),
                _ => {
                    let mut rest: Toks = e
                        .toks
                        .iter()
                        .filter(|t| !matches!(t, Tok::Fn { form, .. } if m(*form).is_some()))
                        .copied()
                        .collect();
                    self.g.sets.share(&mut rest);
                    self.g
                        .split_rest
                        .insert((r, xf), (e.toks.clone(), rest.clone()));
                    rest
                }
            };
            if conc.toks.is_empty() {
                conc.toks = rest;
            } else if !rest.is_empty() {
                conc.toks.add_all(rest.iter().copied().collect());
            }
            let Tok::Ref { src, xf: rx } = r else {
                continue;
            };
            refs.push(Tok::Ref {
                src,
                xf: rx.then(xf),
            });
        }
        conc.cap();
        (conc, refs)
    }

    /// A compare's result `out` of operands `raws` holding [`Tok::Ref`]s:
    /// each Ref's compared tokens as one Ref (`cmp`), not a copy (a large
    /// game compares heap pointers everywhere). Its tokens are each of its
    /// functions' compared, of any form: more than the op made only of
    /// forms the op made a hole of already.
    fn cmp_refs(&mut self, out: &mut Val, raws: &[&Val]) {
        let refs: BTreeSet<Tok> = raws.iter().flat_map(|v| v.refs()).collect();
        if refs.is_empty() || out.toks.len() < 64 {
            return;
        }
        for r in refs {
            let Tok::Ref { src, xf } = r else {
                continue;
            };
            let c = Tok::Ref {
                src,
                xf: xf.then(Xf::all(Form::Cmp, false)),
            };
            let e = self.g.ref_val(c);
            if !e.toks.is_empty() {
                let rest: Toks = out.toks.difference(&e.toks).copied().collect();
                out.toks = rest;
            }
            out.toks.insert(c);
        }
    }

    fn bin(&mut self, op: BinOp, a: &Val, b: &Val, site: Origin, at: u32) -> Val {
        let mut out = Val::default();
        // Every token with every token, but for large sets: each with an
        // unknown partner (which only adds holes and unknowns).
        let (xs, ys) = (a.parts(), b.parts());
        let one = self.g.off & OFF_MOVE != 0 && xs.len().min(ys.len()) == 1;
        let pairs: Vec<(Option<Tok>, Option<Tok>)> = if xs.len() * ys.len() > 64 && !one {
            xs.iter()
                .map(|&x| (x, None))
                .chain(ys.iter().map(|&y| (None, y)))
                .collect()
        } else {
            xs.iter()
                .flat_map(|&x| ys.iter().map(move |&y| (x, y)))
                .collect()
        };
        // Gathered, and added at the end (a large set's one at a time was
        // most of a large game's run), as the holes of functions' tokens.
        let mut toks = Vec::new();
        let mut holes = Vec::new();
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
                        toks.push(t);
                    }
                    R::T(
                        t @ Tok::Fn {
                            form: Form::Cmp, ..
                        },
                    ) => {
                        toks.push(t);
                        // `a - b` with both functions: b is compared too.
                        if let Some(Tok::Fn {
                            key,
                            origin,
                            form: Form::Full | Form::Lo | Form::Hi,
                        }) = y
                        {
                            toks.push(Tok::Fn {
                                key,
                                origin,
                                form: Form::Cmp,
                            });
                        }
                    }
                    R::T(t) => toks.push(t),
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
                            if self.trace {
                                trace_at("rom lost", at, || format!("{x:x?} - {y:x?}"));
                            }
                        }
                        // A RAM address moved by an unknown (an index, a
                        // mask), as for an object: still a pointer, which
                        // the run can't place (any other op makes a
                        // number of it: a difference, a hash).
                        let ram = |t: Option<Tok>| {
                            matches!(t, Some(Tok::Num(c)) if is_ram(c))
                                || matches!(t, Some(Tok::RamFrom(_)))
                        };
                        // A mask the run doesn't know makes a number of
                        // it, as of a pointer into an object.
                        // Another pointer (into ROM, an object, RAM): two
                        // values' tokens paired that never meet (a
                        // pointer plus a pointer is none), or a
                        // difference, a number.
                        let ptr = |t: Option<Tok>| {
                            ram(t)
                                || matches!(t, Some(Tok::Rom(_) | Tok::RomFrom(_) | Tok::Obj(..)))
                        };
                        let moved = match op {
                            BinOp::Add | BinOp::Or | BinOp::Xor => {
                                (ram(x) && !ptr(y)) || (ram(y) && !ptr(x))
                            }
                            // Less a number or an unknown.
                            BinOp::Sub => ram(x) && !ptr(y),
                            BinOp::And | BinOp::Mul => false,
                        };
                        if moved {
                            lost_said(|| format!("{op:?} of {x:x?} and {y:x?} at {at:#010x}"));
                            out.lost_ptr = true;
                        }
                    }
                    R::Hole(why, num) => {
                        for t in [x, y].into_iter().flatten() {
                            if let Tok::Fn { origin, .. } = t {
                                holes.push((origin, why));
                            }
                        }
                        match num {
                            Some(c) => toks.push(num_tok(self.img, c)),
                            None => out.unk = true,
                        }
                    }
                }
            }
        }
        for (o, why) in holes {
            self.g.hole_one(o, at, why);
        }
        out.toks.add_all(toks);
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
        let mut callees: BTreeSet<u32> =
            t.fns().filter(|f| f.2 == Form::Full).map(|f| f.0).collect();
        // A function with no caller in sight, calling through what it was
        // passed: any function whose address is taken (what a load
        // through an unknown address gave before RAM pointers were
        // followed, `native/fnref-unknown-addrs`).
        // A call only: a return that isn't proven one stays a jump with
        // no callee.
        if callees.is_empty() && !tail && !t.lost_rom && self.g.orphans.contains(&entry_of(self.ix))
        {
            callees = self.g.taken.clone();
        }
        // In the closed world nothing unknown is a function's address, so
        // the whole addresses the target may hold are all it can call (a
        // piece of one, called, is a hole of its origin's, above).
        // Unless the run dropped one: a ROM read it cut short or couldn't
        // make, or a ROM pointer it lost.
        let complete = !callees.is_empty() && !t.lost_rom;
        if self.trace && !complete {
            trace_at("incomplete", at, || format!("through {}", brief(t)));
        }
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
        self.g.now = (entry_of(self.ix), at);
        let mut vals: Vec<Val> = vec![Val::default(); b.insts.len()];
        let none = Val::default();
        for (i, inst) in b.insts.iter().enumerate() {
            let site = Origin::Code(key, i as u32);
            let holes_before = self.g.flow.holes.len();
            let logged = self.g.hole_log.len();
            let val = |x: ValueId| vals[x.index()].clone();
            // With the pile's tokens in, to look at them.
            let xv = |g: &mut G, x: ValueId| g.expand(&vals[x.index()]);
            let guarded = inst.guard.is_some();
            // An op on a large value, as kept: its holes are made
            // already. (A shift that keeps the value as it is, below,
            // leaves its Refs unread; an add with carry of no function's
            // address reads its carry.)
            let large = |x: ValueId| {
                let v = &vals[x.index()];
                v.toks.len() >= 64 || v.marked()
            };
            let op = match inst.op {
                Op::Shift { kind, v, amount } if large(v) => {
                    let n = &vals[amount.index()];
                    let n = match (n.toks.iter().next(), n.toks.len(), n.unk || n.mem) {
                        (Some(Tok::Num(n)), 1, false) => Some(*n),
                        _ => None,
                    };
                    let keep = n == Some(0) || (kind == ShiftType::Ror && n.is_none());
                    (!keep).then_some((1 + kind as u8, n, v, None))
                }
                Op::Not(v) if large(v) => Some((5, None, v, None)),
                Op::Bin(op, a, b) if large(a) || large(b) => Some((6 + op as u8, None, a, Some(b))),
                Op::Adc { a, b, .. } if large(a) || large(b) => Some((12, None, a, Some(b))),
                _ => None,
            };
            // (Not under `GBAA_FNREF_WATCH`, which says what each op makes,
            // nor `GBAA_FNREF_OFF`, whose ops read more than these keys.)
            let op = op.filter(|_| self.g.watch.is_empty() && !self.g.roads && self.g.off == 0);
            let mut kept = match op.map(|(op, n, a, b)| {
                let b = b.map_or(&none, |b| &vals[b.index()]);
                self.g.op_kept(op, n, [&vals[a.index()], b])
            }) {
                Some(Ok(out)) => {
                    vals[i] = out;
                    continue;
                }
                Some(Err(k)) => Some(k),
                None => None,
            };
            let v = match inst.op {
                Op::Insn(a) => {
                    at = a;
                    self.at = a;
                    self.g.now.1 = a;
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
                    let v = Val::of(self.classify(c, o));
                    self.g.watch(&v, || "loaded from its pool word".to_string());
                    v
                }
                Op::Bit(x) => Val::of(Tok::Num(x as u32)),
                Op::GetReg(r) => s.regs[r.index()].clone(),
                // What a user-bank register holds isn't followed.
                Op::GetUserReg(_) => {
                    lost_said(|| format!("a user-bank register read at {at:#010x}"));
                    Val::lost()
                }
                Op::Bin(op, ra, rb) => {
                    let (a, b) = (xv(self.g, ra), xv(self.g, rb));
                    let mut out = self.bin(op, &a, &b, site, at);
                    if op == BinOp::Sub {
                        self.cmp_refs(&mut out, &[&vals[ra.index()], &vals[rb.index()]]);
                    }
                    out
                }
                Op::Adc { a: ra, b: rb, c } => {
                    let (a, b) = (xv(self.g, ra), xv(self.g, rb));
                    if self.g.off & OFF_MOVE != 0 && is_num(&b, 0) && is_num(&vals[c.index()], 0) {
                        // `GBAA_FNREF_OFF`'s `move`: `x + 0 + 0` is `x`.
                        val(ra)
                    } else if a.fns().next().is_some() || b.fns().next().is_some() {
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
                        let mut cmps = Vec::new();
                        for v in [&a, &b] {
                            self.g.hole(v, at, Why::Arithmetic, |f| {
                                bad || !matches!(f, Form::Full | Form::Not)
                            });
                            cmps.extend(v.fns().map(|(key, origin, _)| Tok::Fn {
                                key,
                                origin,
                                form: Form::Cmp,
                            }));
                        }
                        out.toks.add_all(cmps);
                        self.cmp_refs(&mut out, &[&vals[ra.index()], &vals[rb.index()]]);
                        // What the other tokens make (a pointer the value
                        // may be instead, moved: dropped, a load through
                        // it would read nothing).
                        let others = |v: &Val| Val {
                            toks: v
                                .toks
                                .iter()
                                .filter(|t| !matches!(t, Tok::Fn { .. }))
                                .copied()
                                .collect(),
                            ..v.clone()
                        };
                        let (a, b) = (others(&a), others(&b));
                        if !a.toks.is_empty() || !b.toks.is_empty() {
                            let ab = self.bin(BinOp::Add, &a, &b, site, at);
                            let c = xv(self.g, c);
                            out.join(&self.bin(BinOp::Add, &ab, &c, site, at));
                        }
                        out
                    } else {
                        kept = None;
                        let ab = self.bin(BinOp::Add, &a, &b, site, at);
                        let c = xv(self.g, c);
                        self.bin(BinOp::Add, &ab, &c, site, at)
                    }
                }
                Op::Not(ra) => {
                    let not = |f| (f == Form::Full).then_some(Form::Not);
                    let (a, refs) = self.split_refs(&vals[ra.index()], not);
                    let mut out = Val {
                        unk: a.unk || a.mem,
                        toks: refs.into_iter().collect(),
                        ..Val::default()
                    };
                    self.g
                        .hole(&vals[ra.index()], at, Why::Arithmetic, |f| f != Form::Full);
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
                            Tok::RomFrom(_) | Tok::RamFrom(_) => out.unk = true,
                            // `!` of a pointer is a number (the closed world).
                            Tok::Obj(..) => out.unk = true,
                            Tok::Ref { .. } => {
                                debug_assert!(false, "a Ref not expanded");
                                out.unk = true;
                            }
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
                    // Memory holds functions' and ROM addresses, which a
                    // shift by 0 or a load's rotation keeps: its Refs
                    // stay.
                    let keep = n == Some(0) || (kind == ShiftType::Ror && n.is_none());
                    // Otherwise each Ref's forms shifted, as one.
                    let (x, refs) = if keep {
                        (x, Vec::new())
                    } else {
                        self.split_refs(&x, |f| shift_form(f, kind, n))
                    };
                    let mut out = Val {
                        unk: x.unk || x.mem,
                        toks: refs.into_iter().collect(),
                        ..Val::default()
                    };
                    // Gathered and added at once (as in `bin`).
                    let mut toks = Vec::new();
                    for &t in &x.toks {
                        match t {
                            Tok::Fn { key, origin, form } => match shift_form(form, kind, n) {
                                Some(form) => toks.push(Tok::Fn { key, origin, form }),
                                None => {
                                    self.g.hole_one(origin, at, Why::Arithmetic);
                                    match n {
                                        Some(n) => {
                                            toks.push(num_tok(self.img, shift(kind, key, n)))
                                        }
                                        None => out.unk = true,
                                    }
                                }
                            },
                            // A ROM pointer through a word load's
                            // rotation stays one, as a function does.
                            Tok::Rom(_)
                            | Tok::RomFrom(_)
                            | Tok::Obj(..)
                            | Tok::RamFrom(_)
                            | Tok::Ref { .. }
                                if keep =>
                            {
                                toks.push(t)
                            }
                            // Shifted by bytes, a pointer into an object is a
                            // piece of one (of the object still); by
                            // anything else, a number.
                            Tok::Obj(i, _) => {
                                out.unk = true;
                                if matches!(n, Some(8 | 16 | 24)) {
                                    toks.push(Tok::Obj(i, OBJ_ANY));
                                }
                            }
                            // A RAM address through a word load's rotation
                            // stays one; cut in bytes (a piece copied) or
                            // by an unknown amount, it is lost.
                            Tok::Num(c) if is_ram(c) && kind == ShiftType::Ror && n.is_none() => {
                                toks.push(t)
                            }
                            // Shifted by bytes, a RAM address is a piece of
                            // one (as of an object, above): from here on.
                            Tok::Num(c) if is_ram(c) && matches!(n, Some(8 | 16 | 24)) => {
                                out.unk = true;
                                toks.push(Tok::RamFrom(ram_canon(c)));
                            }
                            Tok::RamFrom(_) if matches!(n, Some(8 | 16 | 24)) => {
                                out.unk = true;
                                toks.push(t);
                            }
                            Tok::RamFrom(_) => out.unk = true,
                            Tok::Num(c) | Tok::Rom(c) => match n {
                                Some(n) => toks.push(num_tok(self.img, shift(kind, c, n))),
                                None => out.unk = true,
                            },
                            // Shifted, it's no pointer (the closed world).
                            Tok::RomFrom(_) => out.unk = true,
                            Tok::Ref { .. } => unreachable!("expanded"),
                        }
                    }
                    out.toks.add_all(toks);
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
                // `move`: neither carries nor overflows.
                Op::AdcCarry { b, c, .. } | Op::AdcOverflow { b, c, .. }
                    if self.g.off & OFF_MOVE != 0
                        && is_num(&vals[b.index()], 0)
                        && is_num(&vals[c.index()], 0) =>
                {
                    Val::of(Tok::Num(0))
                }
                Op::AdcCarry { .. }
                | Op::AdcOverflow { .. }
                | Op::MulCarry { .. }
                | Op::ShiftCarry { .. }
                | Op::TestBit { .. }
                    if self.g.off & OFF_FLAGS != 0 =>
                {
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
                    let a = xv(self.g, addr);
                    self.g.hole(&a, at, Why::Address, |_| true);
                    let mut out = self.load(&s, key, i, &a, w, b.hulls.get(&i).copied());
                    if self.trace && out.lost_ptr && !a.lost_ptr {
                        trace_at("obj lost by load", at, || {
                            format!("w{w} through {}", brief(&a))
                        });
                    }
                    if !self.g.watch.is_empty() {
                        let hull = b.hulls.get(&i).copied();
                        self.g.watch(&out, || {
                            format!("LOADED w{w} through {}, bounds {hull:x?}", brief(&a))
                        });
                    }
                    if guarded {
                        out.unk = true;
                    }
                    out
                }
                Op::Store { width, addr, value } => {
                    let w = bytes(width);
                    let (a, x) = (xv(self.g, addr), val(value));
                    self.g.hole(&a, at, Why::Address, |_| true);
                    let (xm, bad_refs) = self.g.for_mem(&x, w);
                    let (toks, bad) = to_mem(&xm, w);
                    if !bad.is_empty() {
                        let mut xc = xm.clone();
                        xc.drop_range(REF0..);
                        for f in bad {
                            let why = match f {
                                Form::Cmp | Form::Not => Why::CmpUsed,
                                _ => Why::PartUsed,
                            };
                            self.g.hole(&xc, at, why, |g| g == f);
                        }
                    }
                    for r in bad_refs {
                        let e = self.g.ref_val(r);
                        let no = |f| store_form(f, w).is_none();
                        self.g.hole(&e, at, Why::CmpUsed, |f| {
                            matches!(f, Form::Cmp | Form::Not) && no(f)
                        });
                        self.g.hole(&e, at, Why::PartUsed, |f| {
                            !matches!(f, Form::Cmp | Form::Not) && no(f)
                        });
                    }
                    self.store(&mut s, key, i, &a, w, &x, &toks, guarded);
                    Val::default()
                }
                Op::Exit(e) => {
                    let out = &b.exits[&i];
                    let t = match e {
                        Exit::Jump(t) | Exit::JumpExchange(t) | Exit::ExceptionReturn(t) => {
                            Some(xv(self.g, t))
                        }
                        _ => None,
                    };
                    self.exit(out, t, &s);
                    Val::default()
                }
                _ => Val::unk(),
            };
            let mut v = v;
            if !v.lost_rom
                && lost_from(&inst.op)
                    .iter()
                    .any(|x| self.g.lost(&vals[x.index()]))
            {
                v.lost_rom = true;
            }
            // What a word op makes of a lost pointer is lost.
            if !v.lost_ptr
                && !matches!(inst.op, Op::Load { .. })
                && lost_from(&inst.op)
                    .iter()
                    .any(|x| self.g.lost_ptr(&vals[x.index()]))
            {
                v.unk = true;
                v.lost_ptr = true;
            }
            if self.trace && self.g.flow.holes.len() != holes_before {
                let ops: Vec<String> = inst_operands(&inst.op)
                    .into_iter()
                    .map(|x| format!("{:?}", vals[x.index()]))
                    .collect();
                eprintln!("  hole at {at:#010x}: {:?} on {}", inst.op, ops.join(" / "));
            }
            if self.g.hole_log.len() != logged {
                // A BIOS call's holes are its registers'.
                let mut ops: Vec<Val> = inst
                    .op
                    .operands()
                    .into_iter()
                    .map(|x| vals[x.index()].clone())
                    .collect();
                if matches!(inst.op, Op::Exit(_)) {
                    ops.extend(s.regs[..4].iter().cloned());
                }
                self.g.roads(logged, &ops);
            }
            // Kept, but for a constant this instruction made (its own).
            if let Some(k) = kept
                && !v
                    .toks
                    .iter()
                    .any(|t| matches!(t, Tok::Fn { origin, .. } if *origin == site))
            {
                self.g.op_keep(k, &v);
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
        // A pointer into RAM the run dropped may be any: every cell and
        // object, and the pile. An unknown with none dropped is no RAM
        // address (the closed world): an I/O read, or one the value-set
        // analysis bounds.
        let mut lost = a.lost_ptr;
        let unknown = a.unk || a.mem || a.toks.is_empty();
        // `out` as the cap leaves it (each ROM read caps it; a read
        // through thousands of function addresses reads their code).
        let mut capped = false;
        for &t in &a.toks {
            match t {
                Tok::Rom(r) => {
                    self.rom_read_or_capped(r, w, &mut out, capped);
                    capped = true;
                }
                Tok::RomFrom(r) => {
                    // As the cap leaves it, and taking in all the read
                    // holds, `out` is the join already (a load through
                    // hundreds of pointers "from here on" capped it at
                    // each: a fifth of Emerald's second round).
                    let v = self.rom_from_val(r, w, hull, None);
                    if capped {
                        out.join_capped(&v);
                    } else {
                        out.join(&v);
                    }
                    capped = true;
                }
                Tok::Obj(o, off) if self.g.obj_up(o, off) => {
                    lost_said(|| {
                        format!(
                            "a load at {:#010x} through {:x?} at {off}, a caller's frame",
                            self.at, self.g.objs[o as usize]
                        )
                    });
                    lost = true
                }
                Tok::Obj(o, off) => {
                    capped = false;
                    self.g.read_obj(o, off, w, &mut out);
                }
                // Its run's cells, and the pile.
                Tok::RamFrom(c) => {
                    capped = false;
                    let hi = ram_bound(c) - 1;
                    let lo = c.saturating_sub(3);
                    self.g.read_range(lo, hi, w, "in a RAM run", &mut out);
                    self.g.read_pile(w, &mut out);
                }
                Tok::Num(c) | Tok::Fn { key: c, .. } => {
                    if is_ram(c) {
                        capped = false;
                        let c = ram_canon(c);
                        self.g
                            .rd
                            .ranges
                            .push((c.saturating_sub(3), c + w as u32 - 1));
                        // The cells the access overlaps.
                        self.g.read_cell(c, w, &mut out);
                        self.g.read_pile(w, &mut out);
                    } else if self.img.word(c).is_some() {
                        self.rom_read_or_capped(c, w, &mut out, capped);
                        capped = true;
                    } else {
                        capped = false;
                        out.unk = true;
                    }
                }
                Tok::Ref { .. } => {
                    debug_assert!(false, "a Ref not expanded");
                    lost = true;
                }
            }
        }
        if lost && self.trace {
            trace_at("load anywhere", self.at, || format!("through {}", brief(a)));
        }
        if self.g.off & OFF_LOST != 0 {
            lost = false;
        }
        if lost || unknown {
            // `GBAA_FNREF_OFF`'s `tables`: only the object the bounds start in.
            let hull = match hull {
                Some((lo, hi))
                    if self.g.off & OFF_TABLES != 0
                        && self.img.word(lo).is_some()
                        && self.img.word(hi).is_some() =>
                {
                    let end = self.object_end(lo, hi.saturating_add(1));
                    Some((lo, hi.min(end.saturating_sub(1))))
                }
                h => h,
            };
            match hull {
                // Bounded to ROM: its words there.
                Some((lo, hi))
                    if self.img.word(lo).is_some()
                        && self.img.word(hi).is_some()
                        && hi - lo < ROM_READ =>
                {
                    self.rom_range(lo, hi, w, &mut out);
                }
                _ => {
                    // Bounded to ROM but too wide to read: its words go
                    // unread.
                    if let Some((lo, hi)) = hull
                        && self.img.word(lo).is_some()
                        && self.img.word(hi).is_some()
                    {
                        out.lost_rom = true;
                        if self.trace {
                            trace_at("rom lost", self.at, || {
                                format!("bounds {lo:#x}..={hi:#x} too wide, through {}", brief(a))
                            });
                        }
                    }
                    // Bounds that reach past RAM are none: "not null",
                    // 1 to 0xffff_ffff, took in every cell and every
                    // object's, the frames' among them.
                    let hull = hull.filter(|&(lo, hi)| is_ram(lo) && is_ram(hi));
                    match hull {
                        // Bounded to RAM: the cells there.
                        Some((lo, hi)) => {
                            self.g.read_pile(w, &mut out);
                            let lo = lo.saturating_sub(3);
                            self.g
                                .read_range(lo, hi, w, "in the load's bounds", &mut out);
                        }
                        None if lost => {
                            self.g.flow.everything.insert(self.at);
                            self.g.read_pile(w, &mut out);
                            self.g.read_cells(w, &mut out);
                        }
                        None => {}
                    }
                    if lost {
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
                                .filter(|(t, _)| *t != OTHER && *t != LOST && *t != LOST_PTR)
                                .map(|(t, _)| *t),
                        );
                        v.lost_rom = x.lost_rom;
                        v.lost_ptr = x.lost_ptr;
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
        let mut anywhere = a.lost_ptr;
        // An unknown address with no RAM pointer lost is none in RAM (the
        // closed world, as for loads): what it stores reaches no load,
        // but function and ROM addresses go to the pile all the same (so
        // their flow is as before).
        if !a.lost_ptr && (a.unk || a.mem || a.toks.is_empty()) {
            let kept: Vec<(Tok, u8)> = toks
                .iter()
                .copied()
                .filter(|(t, _)| {
                    matches!(
                        t,
                        Tok::Fn { .. } | Tok::Rom(_) | Tok::RomFrom(_) | Tok::Ref { .. }
                    )
                })
                .collect();
            if !kept.is_empty() {
                self.g.store(Store::Pile, &kept);
            }
        }
        for &t in &a.toks {
            match t {
                Tok::Num(c) | Tok::Fn { key: c, .. } if is_ram(c) => {
                    self.g.store(Store::Cell(ram_canon(c)), toks)
                }
                Tok::Obj(o, off) if self.g.obj_up(o, off) => anywhere = true,
                Tok::Obj(o, off) => self.g.store(Store::Cell(obj_addr(o, off)), toks),
                // Somewhere in a run of RAM: the pile (every load from RAM
                // reads it).
                Tok::RamFrom(_) => anywhere = true,
                // ROM: nothing changes (the closed world: a ROM pointer
                // moved stays in ROM).
                Tok::Num(_) | Tok::Rom(_) | Tok::RomFrom(_) | Tok::Fn { .. } => {}
                Tok::Ref { .. } => {
                    debug_assert!(false, "a Ref not expanded");
                    anywhere = true;
                }
            }
        }
        // A lost pointer into an object: every object.
        if a.lost_ptr {
            self.g.store(Store::Cell(LOST_CELL), toks);
        }
        // Somewhere the run can't place: the pile, which every load from
        // RAM reads.
        if anywhere {
            if self.trace && toks.iter().any(|(t, _)| matches!(t, Tok::Fn { .. })) {
                trace_at("piled", self.at, || format!("through {}", brief(a)));
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
                            Some(r) => self.g.expand(&s.regs[r as usize]),
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
                let rule = bios_regs(out.bios);
                for (i, r) in s.regs[..4].iter().enumerate() {
                    if rule.reads & 1 << i != 0 {
                        self.g.hole(r, out.at, Why::Bios, |_| true);
                    }
                }
                let mut after = s.clone();
                for (i, r) in after.regs[..4].iter_mut().enumerate() {
                    if rule.sets & 1 << i != 0 {
                        *r = Val::unk();
                    } else if rule.may_set & 1 << i != 0 {
                        r.join(&Val::unk());
                    }
                }
                // CpuSet and CpuFastSet from ROM copy its words to RAM: their
                // length's, or the source object's.
                if matches!(out.bios, Some(0x0B | 0x0C)) {
                    let src = self.g.expand(&s.regs[0]);
                    let mut got = Val {
                        lost_rom: src.lost_rom,
                        ..Val::default()
                    };
                    let n = copy_len(out.bios == Some(0x0C), &s.regs[2]);
                    for &t in &src.toks {
                        match t {
                            Tok::Rom(a) => self.rom_from(a, 4, None, n, &mut got),
                            Tok::RomFrom(a) => self.rom_from(a, 4, None, None, &mut got),
                            // From RAM (not followed): the RAM pointers
                            // it may copy are lost.
                            _ => {
                                got.unk = true;
                                lost_said(|| "CpuSet from RAM".into());
                                got.lost_ptr = true;
                            }
                        }
                    }
                    if src.unk || src.mem || src.lost_ptr {
                        got.unk = true;
                        lost_said(|| "CpuSet from an unknown source".into());
                        got.lost_ptr = true;
                    }
                    let (toks, _) = to_mem(&got, 4);
                    self.g.store(Store::Pile, &toks);
                    let dst = self.g.expand(&s.regs[1]);
                    let dst = offset(&dst, None);
                    self.store_at(&dst, &toks);
                    // CpuFastSet leaves in r3 a word it read: the fill's, or
                    // its last block's first. From ROM one of those; from
                    // anywhere else, what a load the run can't place reads.
                    if out.bios == Some(0x0C) {
                        let rom = |t: &Tok| matches!(t, Tok::Rom(_) | Tok::RomFrom(_));
                        if src.unk || src.mem || !src.toks.iter().all(rom) || src.toks.is_empty() {
                            self.g.read_pile(4, &mut got);
                            self.g.read_cells(4, &mut got);
                        }
                        after.regs[3].join(&got);
                    }
                }
                // The BIOS may write numbers through a pointer into an
                // object it is given.
                for (i, r) in s.regs[..4].iter().enumerate() {
                    if rule.reads & 1 << i == 0 {
                        continue;
                    }
                    let r = self.g.expand(r);
                    if r.has_obj() {
                        let a = offset(&r, None);
                        self.store_at(&a, &[(OTHER, 1), (OTHER, 2), (OTHER, 4)]);
                    }
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
                            lost_ptr: a.lost_ptr && !o.lost_ptr,
                            ..Val::default()
                        };
                        if !std::rc::Rc::ptr_eq(&a.toks.0, &o.toks.0) {
                            d.toks = a.toks.difference(&o.toks).copied().collect();
                        }
                        d
                    })
                    .collect();
                if delta.iter().any(|d| {
                    !d.toks.is_empty() || d.mem || d.unk || d.lost_rom || d.lost_ptr || d.marked()
                }) {
                    let ixs: BTreeSet<Ix> =
                        callees.iter().map(|&k| self.g.ix_call(k, ctx)).collect();
                    self.g.give_all(&ixs, &delta);
                    for (o, a) in m.args.iter_mut().zip(args) {
                        o.union(a);
                        self.g.sets.share(&mut o.toks);
                    }
                }
                let mut grew = false;
                let mut once = Once::default();
                for &k in &callees {
                    let k = self.g.ix_call(k, ctx);
                    self.rets_read.insert(k);
                    if self.g.ret_stamp.get(&k).is_some_and(|&s| s > m.stamp) {
                        for (i, (a, b)) in m.rets.iter_mut().zip(&self.g.ret[&k]).enumerate() {
                            grew |= once.first(i, b) && a.union_new(b);
                        }
                    }
                }
                if grew {
                    for a in &mut m.rets {
                        a.cap();
                        self.g.sets.share(&mut a.toks);
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
                let mut once = Once::default();
                for &k in &ixs {
                    for (i, (a, b)) in rets.iter_mut().zip(self.ret_of(k)).enumerate() {
                        if once.first(i, &b) {
                            a.union_new(&b);
                        }
                    }
                }
                for a in &mut rets {
                    a.cap();
                }
                if callees.len() > 1 {
                    let mut args = args.to_vec();
                    for a in args.iter_mut().chain(&mut rets) {
                        self.g.sets.share(&mut a.toks);
                    }
                    self.g.by_value.insert(
                        key,
                        ByValue {
                            callees: callees.clone(),
                            args,
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
            self.g
                .watch_regs(args, |r| format!("passed in r{r} to every loose function"));
            let mut c = false;
            for (a, b) in self.g.loose_args.iter_mut().zip(args) {
                if a.join_capped(b) {
                    self.g.sets.share(&mut a.toks);
                    c = true;
                }
            }
            self.g
                .watch_regs(args, |r| format!("passed in r{r} to every loose function"));
            self.g.dirty_loose |= c;
            self.g.mark(c);
            let lr = match &self.g.loose_rets {
                Some(v) => v.clone(),
                None => {
                    let mut v = vec![Val::default(); 4];
                    let mut once = Once::default();
                    for k in self.loose {
                        if let Some(r) = self.g.ret.get(k) {
                            for (i, (a, b)) in v.iter_mut().zip(r).enumerate() {
                                if once.first(i, b) {
                                    a.join_capped(b);
                                }
                            }
                        }
                    }
                    for a in &mut v {
                        self.g.sets.share(&mut a.toks);
                    }
                    self.g.loose_rets = Some(v.clone());
                    v
                }
            };
            self.rets_read.extend(self.loose.iter().copied());
            for (a, b) in rets.iter_mut().zip(&lr) {
                a.join_capped(b);
            }
            for r in &mut rets {
                r.unk = true;
            }
        }
        rets
    }
}

/// The values joined into each register of a call's results so far, by
/// set and flags: thousands of callees return the same few kept sets
/// ([`Sets`]), and joining one again adds nothing (half of Emerald's third
/// round was those joins).
#[derive(Default)]
struct Once(std::collections::HashSet<(usize, *const BTreeSet<Tok>, Flags), Fx>);

impl Once {
    /// Whether `v` is new to register `i`.
    fn first(&mut self, i: usize, v: &Val) -> bool {
        v.toks.is_empty()
            || self
                .0
                .insert((i, std::rc::Rc::as_ptr(&v.toks.0), v.flags()))
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
        lost_ptr: v.lost_ptr,
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
            // A RAM address by an offset not known: from here on.
            (Tok::Num(c), None) if is_ram(c) => {
                out.toks.insert(Tok::RamFrom(ram_canon(c)));
            }
            (Tok::RamFrom(a), Some(d)) if d >= 0 => {
                out.toks.insert(Tok::RamFrom(a));
            }
            (Tok::RamFrom(a), None) => {
                out.toks.insert(Tok::RamFrom(a));
            }
            (Tok::RamFrom(_), Some(_)) => {
                out.unk = true;
                lost_said(|| "a RAM run moved down".into());
                out.lost_ptr = true;
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

/// The form a shift by `n` (`None`: not a constant) makes of a
/// function's address of form `form` (`None`: no form, a hole).
fn shift_form(form: Form, kind: ShiftType, n: Option<u32>) -> Option<Form> {
    use Form::*;
    use ShiftType::*;
    match (form, kind, n) {
        (f, _, Some(0)) => Some(f),
        // A word load's rotation by its address's low bits: 0 for the
        // aligned words compiled code keeps function addresses in.
        (f, Ror, None) => Some(f),
        (Full, Lsr, Some(16)) | (HiUp, Lsr, Some(16)) => Some(Hi),
        (Full | Lo, Lsl, Some(16)) => Some(LoUp),
        (LoUp, Lsr, Some(16)) => Some(Lo),
        (Hi, Lsl, Some(16)) => Some(HiUp),
        _ => None,
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
    // A RAM address moved by an index the run doesn't know, joined or
    // tagged (`|`, `^`): from here on. Less one, or masked by one, it is
    // lost ([`Run::bin`]).
    let ram = |t: Option<Tok>| match t {
        Some(Num(c)) if is_ram(c) => Some(ram_canon(c)),
        Some(RamFrom(a)) => Some(a),
        _ => None,
    };
    let moves = matches!(op, BinOp::Add | BinOp::Or | BinOp::Xor);
    match (x, y) {
        (Some(_), None) | (None, Some(_)) if moves => {
            if let Some(a) = ram(x).or(ram(y)) {
                return R::T(RamFrom(a));
            }
        }
        (Some(RamFrom(a)), Some(Num(_))) | (Some(Num(_)), Some(RamFrom(a))) if moves => {
            return R::T(RamFrom(a));
        }
        (Some(RamFrom(a)), Some(Num(b))) if op == BinOp::Sub => {
            let p = a.wrapping_sub(b);
            return if is_ram(p) && ram_canon(p) == p && (p >> 24) == (a >> 24) {
                R::T(RamFrom(p))
            } else {
                R::Unk
            };
        }
        (Some(RamFrom(a)), Some(Num(m))) if op == BinOp::And && m >= 0xffff_0000 => {
            return R::T(RamFrom(a & m));
        }
        (Some(RamFrom(_)), _) | (_, Some(RamFrom(_))) => return R::Unk,
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

thread_local! {
    /// [`lost_said`]'s lines, each once.
    static LOST_SAID: std::cell::RefCell<BTreeSet<String>> = std::cell::RefCell::default();
}

thread_local! {
    /// [`trace_at`]'s places, each once.
    static TRACED: std::cell::RefCell<BTreeSet<(&'static str, u32)>> = std::cell::RefCell::default();
}

/// `GBAA_FNREF_TRACE`: print `what` happened at `at`, the first time it
/// does there (a run reruns a function hundreds of times).
fn trace_at(what: &'static str, at: u32, how: impl FnOnce() -> String) {
    if TRACED.with(|s| s.borrow_mut().insert((what, at))) {
        eprintln!("  {what} at {at:#010x}: {}", how());
    }
}

/// `GBAA_FNREF_TRACE`: print where a pointer into RAM is lost
/// ([`Val::lost_ptr`]), each place once.
fn lost_said(what: impl FnOnce() -> String) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("GBAA_FNREF_TRACE").is_some()) {
        return;
    }
    let what = what();
    if LOST_SAID.with(|s| s.borrow_mut().insert(what.clone())) {
        eprintln!("  pointer lost: {what}");
    }
}

/// `GBAA_FNREF_OFF=move,flags,tables,pile,cells,lost,any,spill`: kinds
/// of hole made not to happen, or of memory read made to give nothing,
/// to measure what the words would be without them
/// (`docs/research/fnref-held-words.md`). **For measuring only**, never
/// for a lift that is built (the report's first line says which were
/// off; the lift prints the list it saw). (Its `bios` is the rule now:
/// [`bios_regs`].)
/// `move`: `x + 0` with its flags set (`adds rd, rs, #0`, old GCC's move
/// between low registers) is `x` and its carry and overflow 0, and an op
/// on a large value and a single token runs token by token (both sound;
/// a word followed further may meet a hole it didn't before). The rest
/// are unsound, to see what an exact rule could prove at most: `flags`:
/// a carry, overflow or bit test of a function's address is no hole;
/// `tables`: a ROM read by index, or one the value sets bound, stops at
/// the next object's start even between function words (it reads fewer
/// words, so more are never read); `pile`, `cells`: a pile read, an
/// every-cell read, gives nothing; `lost`: a load through a lost RAM
/// pointer ([`Val::lost_ptr`]) reads nothing; `any`: a pointer anywhere
/// in an object reads only the object's anywhere cell; `spill`: a
/// function's spills read nothing.
fn off_env() -> u8 {
    let v = std::env::var("GBAA_FNREF_OFF").unwrap_or_default();
    let mut off = 0;
    for k in v.split(',').filter(|k| !k.is_empty()) {
        match OFF_KINDS.iter().position(|n| *n == k) {
            Some(i) => off |= 1 << i,
            None => panic!("GBAA_FNREF_OFF: {k} is none of {}", OFF_KINDS.join(", ")),
        }
    }
    if off != 0 {
        let on: Vec<&str> = OFF_KINDS
            .iter()
            .enumerate()
            .filter(|(i, _)| off & 1 << i != 0)
            .map(|(_, k)| *k)
            .collect();
        eprintln!(
            "GBAA_FNREF_OFF={v}: off: {}; the function flow is unsound, for measuring only",
            on.join(",")
        );
    }
    off
}

const OFF_KINDS: [&str; 8] = [
    "move", "flags", "tables", "pile", "cells", "lost", "any", "spill",
];
const OFF_MOVE: u8 = 1;
const OFF_FLAGS: u8 = 2;
const OFF_TABLES: u8 = 4;
const OFF_PILE: u8 = 8;
const OFF_CELLS: u8 = 16;
const OFF_LOST: u8 = 32;
const OFF_ANY: u8 = 64;
const OFF_SPILL: u8 = 128;

/// Where every program starts: the cartridge header's branch.
const RESET: u32 = 0x0800_0000;

/// What a BIOS call does with r0 to r3, a bit each ([`bios_regs`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BiosRegs {
    /// It reads them: a function's address there is given to the BIOS.
    reads: u8,
    /// It always leaves a number of its own there.
    sets: u8,
    /// It may: on some path they are as they came.
    may_set: u8,
}

/// What BIOS call `n` does with r0 to r3 (`native/fnref-bios-reads`). A
/// register it doesn't read is no hole, and one it doesn't set holds after
/// the call what it held before: a function's address left there is
/// followed on, not lost.
///
/// The calls are the lifted program's: what the lifter prints for each
/// (`native.rs`, `Plan::Swi`) and the prelude's routines, which are the
/// play build's (`runtime::bios`), checked against it call by call. What
/// it reads is either's (`MultiBoot`: the lifted one reads nothing). Where
/// the console's BIOS overwrites a register these leave (r1 and r3 are its
/// scratch in most calls; r2 its dispatcher restores), the address is
/// kept: it is followed further than it lives, never less. A register is
/// only `sets` where every path of both writes it: the copies and the
/// decompressors return early on a source in the BIOS, the affine sets
/// leave r0 and r1 on a count of 0, `ArcTan2` r1 on an axis, and the play
/// build's `IntrWait` r0 when the CPU leaves the wait with IRQs off.
/// `SoftReset` doesn't return: the code after it is followed with the
/// registers as they were.
///
/// Any other call (one a native build can't make, a hard hole:
/// [`analysis::holes::NATIVE_BIOS`]), and a number that is no call, reads
/// all four and sets all four, as every call did before.
fn bios_regs(n: Option<u8>) -> BiosRegs {
    const R0: u8 = 1;
    const R1: u8 = 2;
    const R2: u8 = 4;
    const R3: u8 = 8;
    let (reads, sets, may_set) = match n {
        // SoftReset; Halt, Stop; SoundDriverVSyncOff.
        Some(0x00 | 0x02 | 0x03 | 0x28) => (0, 0, 0),
        // RegisterRamReset, SoundBias: r0 the flags, the level.
        Some(0x01 | 0x19) => (R0, 0, 0),
        // IntrWait: r0 the flags it found (the play build's, left with
        // IRQs off in the CPSR, leaves r0).
        Some(0x04) => (R0 | R1, 0, R0),
        // VBlankIntrWait: IntrWait(1, 1).
        Some(0x05) => (0, R0 | R1, 0),
        // Div: quotient, remainder, the quotient's magnitude.
        Some(0x06) => (R0 | R1, R0 | R1 | R3, 0),
        // Sqrt.
        Some(0x08) => (R0, R0, 0),
        // ArcTan2: r0 the angle, r3 0x170, r1 a scratch value off an axis.
        Some(0x0a) => (R0 | R1, R0 | R3, R1),
        // CpuSet: r0 and r1 past a word copy, r3 0x170. CpuFastSet: r3 a
        // word it read (`Run::exit` joins it).
        Some(0x0b | 0x0c) => (R0 | R1 | R2, 0, R0 | R1 | R3),
        // BgAffineSet: r0 and r1 past the entries, r2 the count's end.
        Some(0x0e) => (R0 | R1 | R2, R2, R0 | R1),
        // ObjAffineSet: r3 the stride, too.
        Some(0x0f) => (R0 | R1 | R2 | R3, R2, R0 | R1),
        // LZ77UnComp: r0 and r1 past the data, r3 0.
        Some(0x11 | 0x12) => (R0 | R1, 0, R0 | R1 | R3),
        // HuffUnComp, RLUnComp: r0 and r1 past the data.
        Some(0x13..=0x15) => (R0 | R1, 0, R0 | R1),
        // MidiKey2Freq: r0 the frequency.
        Some(0x1f) => (R0 | R1 | R2, R0, 0),
        // MultiBoot: r0 the result.
        Some(0x25) => (R0 | R1, R0, 0),
        _ => (0xf, 0xf, 0),
    };
    BiosRegs {
        reads,
        sets,
        may_set,
    }
}

/// `GBAA_FNREF_WATCH`'s words: `0x080008c8[,…]`, the ROM words that hold a
/// function's address, each with its key (`roads`, or a word that holds
/// none, watches nothing; `all` is every word: [`G::watch_all`]).
fn watch_env(img: &dyn Image) -> Vec<(u32, Origin)> {
    let v = std::env::var("GBAA_FNREF_WATCH").unwrap_or_default();
    v.split(',')
        .filter_map(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .filter_map(|w| Some((img.word(w).filter(|&k| img.is_fn(k))?, Origin::Word(w))))
        .collect()
}

/// A value for a [`G::watch`] line: its flags and the tokens that aren't
/// functions' (those only counted).
fn brief(v: &Val) -> String {
    let fns = v
        .toks
        .iter()
        .filter(|t| matches!(t, Tok::Fn { .. }))
        .count();
    let rest: Vec<String> = v
        .toks
        .iter()
        .filter(|t| !matches!(t, Tok::Fn { .. }))
        .take(12)
        .map(|t| format!("{t:x?}"))
        .collect();
    format!(
        "{{{fns} function tokens, {} others: {}{}{}{}{}}}",
        v.toks.len() - fns,
        rest.join(" "),
        if v.unk { " unk" } else { "" },
        if v.mem { " mem" } else { "" },
        if v.lost_ptr { " lost_ptr" } else { "" },
        if v.lost_rom { " lost_rom" } else { "" },
    )
}

/// The most places, and the most holes, [`G::watch`] prints of a word.
const WATCH_LINES: usize = 400;

/// What gave a value a function's address, for [`G::roads`] and
/// [`G::watch`]: the value's own token, or one of its [`Tok::Ref`]s'
/// reads.
fn via(r: Option<Tok>) -> String {
    match r {
        None => "its own token".to_string(),
        Some(Tok::Ref { src, xf }) => {
            let s = match src {
                Src::Pile(w) => format!("a pile read w{w}"),
                Src::Cells(w) => format!("an every-cell read w{w}"),
                Src::Objs(w) => format!("an every-object read w{w}"),
                Src::RomFrom(lo, end, w) => {
                    format!("a ROM read by index w{w} of {lo:#010x}..{end:#010x}")
                }
                Src::Hull(lo, hi, w) => {
                    format!("a bounded ROM read w{w} of {lo:#010x}..={hi:#010x}")
                }
            };
            if xf == Xf::ID {
                s
            } else {
                format!("{s}, its forms mapped")
            }
        }
        Some(t) => format!("{t:x?}"),
    }
}

/// The tracing `GBAA_FNREF_WATCH` turns on. It looks into a value's
/// [`Tok::Ref`]s without marking what the running function reads
/// ([`G::rd`] is put back), so the run is the run without it.
impl G {
    /// Says, once, where watched word `o`'s function address is: up to
    /// [`WATCH_LINES`] places a word and as many holes (a word left in a
    /// scratch register of a large game goes to millions).
    fn watch_line(&mut self, o: Origin, what: &str) {
        let Origin::Word(w) = o else { return };
        let n = self.watch_lines.get(&(w, what.starts_with("HOLE")));
        if n.is_some_and(|n| *n >= WATCH_LINES) {
            return;
        }
        let line = if self.now == (0, 0) {
            format!("watch {w:#010x}: {what} [the start's copies]")
        } else {
            format!(
                "watch {w:#010x}: {what} [f_{:08x} at {:#010x}]",
                self.now.0, self.now.1
            )
        };
        if self.watch_said.insert(line.clone()) {
            eprintln!("{line}");
            let n = self
                .watch_lines
                .entry((w, what.starts_with("HOLE")))
                .or_default();
            *n += 1;
            if *n == WATCH_LINES {
                eprintln!("watch {w:#010x}: {WATCH_LINES} such lines, the rest not printed");
            }
        }
    }

    /// The watched words `v` holds in a form `only` takes, each with the
    /// form and the [`Tok::Ref`] that gave it (`None`: its own token).
    fn watched(
        &mut self,
        v: &Val,
        only: &dyn Fn(Form) -> bool,
    ) -> Vec<(Origin, Form, Option<Tok>)> {
        let mut hits = Vec::new();
        let rd = std::mem::take(&mut self.rd);
        let mut parts: Vec<(Option<Tok>, Toks)> = vec![(None, v.toks.clone())];
        for r in v.refs().collect::<Vec<Tok>>() {
            parts.push((Some(r), self.ref_val(r).toks));
        }
        self.rd = rd;
        for (r, toks) in parts {
            if self.watch_all {
                for t in toks.range(..Tok::Num(0)) {
                    if let Tok::Fn {
                        origin: o @ Origin::Word(_),
                        form,
                        ..
                    } = *t
                        && only(form)
                    {
                        hits.push((o, form, r));
                    }
                }
                continue;
            }
            for &(key, origin) in &self.watch {
                let of = |form| Tok::Fn { key, origin, form };
                for t in toks.range(of(Form::Full)..=of(Form::Cmp)) {
                    if let Tok::Fn { form, .. } = *t
                        && only(form)
                    {
                        hits.push((origin, form, r));
                    }
                }
            }
        }
        hits
    }

    /// Whether `o` is a word watched.
    fn watching(&self, o: Origin) -> bool {
        (self.watch_all && matches!(o, Origin::Word(_))) || self.watch.iter().any(|w| w.1 == o)
    }

    fn watch_if(&mut self, v: &Val, only: &dyn Fn(Form) -> bool, what: &dyn Fn() -> String) {
        if self.watch.is_empty() {
            return;
        }
        for (o, form, r) in self.watched(v, only) {
            let by = match r {
                None => String::new(),
                r => format!(", by {}", via(r)),
            };
            self.watch_line(o, &format!("{} ({form:?}{by})", what()));
        }
    }

    /// `GBAA_FNREF_WATCH=0x080008c8[,…]`: print, once each, where the
    /// function address of each of those ROM words goes: `what` says
    /// where, for value `v` that got there. The lines before a word's
    /// first `HOLE` are a road to it: the first the run found. A word can
    /// be in the pile with no line saying it was stored there: a
    /// [`Tok::Ref`] stored before the word was in what it reads.
    fn watch(&mut self, v: &Val, what: impl Fn() -> String) {
        self.watch_if(v, &|_| true, &what);
    }

    /// [`G::watch`] for registers given.
    fn watch_regs(&mut self, regs: &[Val], what: impl Fn(usize) -> String) {
        if !self.watch.is_empty() {
            for (r, v) in regs.iter().enumerate() {
                self.watch(v, || what(r));
            }
        }
    }

    /// The roads' line for the instruction running, which gave the
    /// origins of `hole_log[from..]` their first hole: for each reason and
    /// each thing that gave an operand (`ops`, as they were, their
    /// [`Tok::Ref`]s not taken in) the address ([`via`]), the ROM words
    /// held so. An operand's own token says only that the address came by
    /// registers, a frame word or a cell: watch the word to see from where.
    /// A word is looked for in any form, not only the one holed; the first
    /// source that holds it is named (the value's own token, then its
    /// `Ref`s in order, the pile's first); and one no operand holds (a
    /// call through a register above r3) is "nothing the operands hold".
    fn roads(&mut self, from: usize, ops: &[Val]) {
        let new: BTreeSet<Origin> = self.hole_log[from..].iter().copied().collect();
        let rd = std::mem::take(&mut self.rd);
        let mut by: BTreeMap<Origin, Option<Tok>> = BTreeMap::new();
        let mut parts: Vec<(Option<Tok>, Toks)> = Vec::new();
        for v in ops {
            parts.push((None, v.toks.clone()));
        }
        for v in ops {
            for r in v.refs().collect::<Vec<Tok>>() {
                parts.push((Some(r), self.ref_val(r).toks));
            }
        }
        self.rd = rd;
        for (r, toks) in parts {
            for t in &toks {
                if let Tok::Fn { origin, .. } = *t
                    && new.contains(&origin)
                {
                    by.entry(origin).or_insert(r);
                }
            }
        }
        let mut lines: BTreeMap<(Why, String), Vec<Origin>> = BTreeMap::new();
        for &o in &new {
            let how = match by.get(&o) {
                Some(r) => via(*r),
                None => "nothing the operands hold now".to_string(),
            };
            lines
                .entry((self.flow.holes[&o].1, how))
                .or_default()
                .push(o);
        }
        for ((why, how), os) in lines {
            let words: Vec<String> = os
                .iter()
                .filter_map(|o| match o {
                    Origin::Word(w) => Some(format!("{w:#010x}")),
                    Origin::Code(..) => None,
                })
                .collect();
            eprintln!(
                "road: hole at {:#010x} in f_{:08x}, {}, by {how}: {} words, {} code constants: {}",
                self.now.1,
                self.now.0,
                why.what(),
                words.len(),
                os.len() - words.len(),
                words.join(" ")
            );
        }
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
    let _ram = RamGuard::set(fns, copies);
    let mut g = G {
        loose_args: vec![Val::default(); 16],
        allocs: allocs.clone(),
        objects: objects(img),
        watch: watch_env(img),
        off: off_env(),
        watch_all: std::env::var("GBAA_FNREF_WATCH").is_ok_and(|v| v == "all"),
        roads: std::env::var_os("GBAA_FNREF_WATCH").is_some(),
        ..G::default()
    };
    // `all`: a word for the watch's guards to see (every ROM word is
    // looked for: [`G::watched`]).
    if g.watch_all {
        g.watch = vec![(0, Origin::Word(0))];
    }
    if g.off != 0 {
        g.flow.off = std::env::var("GBAA_FNREF_OFF").unwrap_or_default();
    }
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
            let (p, c, o) = (
                g.pile_read(w).clone(),
                g.cells_read(w).clone(),
                g.objs_read(w).clone(),
            );
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
            // Settled: the functions still without a caller in sight
            // (but the reset, whose registers are nothing) run again,
            // their calls by value any taken function's.
            let new: Vec<u32> = loose
                .iter()
                .map(|&i| entry_of(i))
                .filter(|k| !g.taken.contains(k) && *k != RESET && !g.orphans.contains(k))
                .collect();
            if !new.is_empty() {
                if trace {
                    eprintln!("fnref round {round}: settled; no caller in sight: {new:x?}");
                }
                dirty.extend(new.iter().map(|&k| k as Ix));
                g.orphans.extend(new);
            }
        }
        if dirty.is_empty() {
            if trace {
                for (i, o) in g.objs.iter().enumerate() {
                    eprintln!("obj {i}: {o:x?}");
                }
            }
            return finish(g);
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
            let mut sp_in = g.expand(&std::mem::take(&mut entry[13]));
            if sp_in.toks.is_empty() {
                lost_said(|| "a function no call gave an sp".into());
                sp_in = Val::lost();
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
                    if trace {
                        eprintln!(
                            "fnref {:#010x}: unsettled after {steps} block runs ({} blocks)",
                            f.entry,
                            f.blocks.len()
                        );
                    }
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
                if trace && (total > TOKENS || ran.is_multiple_of(4096)) {
                    eprint!("{}", g.held_copies());
                    eprintln!("  entries' tokens by kind: {}", g.entry_kinds());
                }
                if total > TOKENS {
                    g.flow.unsettled = true;
                    g.flow.over = Some((total, ran));
                    return finish(g);
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
        let taken_before = g.taken.len();
        // The memory reads the values hold by their Refs: whole addresses
        // read as words that were stored in parts.
        let refs: BTreeSet<Tok> = held.iter().flat_map(|v| v.refs()).collect();
        for v in held {
            let mut v = v;
            if v.marked() {
                v.drop_range(REF0..);
            }
            g.taken_add(&v);
        }
        for r in refs {
            let v = g.ref_val(r);
            g.taken_add(&v);
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
        // More functions taken: what a function with no caller in sight
        // may call grew ([`Run::callees`]).
        if g.taken.len() != taken_before {
            dirty.extend(g.orphans.iter().map(|&k| k as Ix));
        }
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
    finish(g)
}

/// The run's findings: with the origins of what the [`Tok::Ref`]s stored
/// to the pile hold.
fn finish(mut g: G) -> Flow {
    for r in std::mem::take(&mut g.piled_refs) {
        let v = g.ref_val(r);
        for (_, o, _) in v.fns() {
            g.flow.piled.insert(o);
        }
    }
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
                _ if !self.off.is_empty() => format!(
                    " (GBAA_FNREF_OFF={}: unsound, a measurement only)",
                    self.off
                ),
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
