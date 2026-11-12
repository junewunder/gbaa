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
//! saved register's slot reads as unknown, the register as it came; one
//! at or above the entry sp, a stack argument, reads the pile too); RAM words at
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
//! Precision: the one pile is the limit (`docs/native-stage-b.md`, "Known
//! gaps", the pile). agb keeps closures and trait objects on its heap, so a
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

/// What a [`Tok::Ref`] stands for: a load at width `w`'s read of the pile,
/// of every cell, of the ROM words in `lo..end` a read from `lo` on
/// gives ([`Run::rom_from`]), or of the ROM words in `lo..=hi` (a read
/// the value-set analysis bounds there).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Src {
    Pile(u8),
    Cells(u8),
    RomFrom(u32, u32, u8),
    Hull(u32, u32, u8),
}

impl Src {
    fn w(self) -> u8 {
        match self {
            Src::Pile(w) | Src::Cells(w) | Src::RomFrom(_, _, w) | Src::Hull(_, _, w) => w,
        }
    }
}

/// What a [`Tok::Ref`] makes of the tokens it stands for: each
/// function's address's form mapped (`map[form]`, [`Xf::DROP`]: none of
/// it), ROM pointers kept or not (and if not, whether they make it a
/// number read from memory), the read's flags kept or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Xf {
    map: [u8; 8],
    roms: bool,
    rom_mem: bool,
    flags: bool,
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
    /// As memory holds it.
    const ID: Xf = Xf {
        map: [0, 1, 2, 3, 4, 5, 6, 7],
        roms: true,
        rom_mem: false,
        flags: true,
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

/// The lowest [`Tok::Ref`].
const REF0: Tok = Tok::Ref {
    src: Src::Pile(0),
    xf: Xf {
        map: [0; 8],
        roms: false,
        rom_mem: false,
        flags: false,
    },
};

/// A load at width `w` of the pile's, or every cell's, words.
fn mem_ref(cells: bool, w: u8) -> Tok {
    Tok::Ref {
        src: if cells { Src::Cells(w) } else { Src::Pile(w) },
        xf: Xf::ID,
    }
}

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
            self.toks.add_all(o.toks.iter().copied().collect());
        }
        self.mem |= o.mem;
        self.unk |= o.unk;
        self.lost_rom |= o.lost_rom;
    }

    fn join(&mut self, o: &Val) -> bool {
        let before = (self.toks.len(), self.mem, self.unk, self.lost_rom);
        if self.toks.is_empty() {
            self.toks = o.toks.clone();
        } else if !std::rc::Rc::ptr_eq(&self.toks.0, &o.toks.0) && !o.toks.is_subset(&self.toks) {
            self.toks.add_all(o.toks.iter().copied().collect());
        }
        self.mem |= o.mem;
        self.unk |= o.unk;
        self.lost_rom |= o.lost_rom;
        self.cap();
        before != (self.toks.len(), self.mem, self.unk, self.lost_rom)
    }

    /// [`Val::union`], and whether `o` added anything (the callees'
    /// results of a call by value are taken in so, then capped once: a cap
    /// at each of thousands was most of a large game's run).
    fn union_new(&mut self, o: &Val) -> bool {
        let flags = (o.mem && !self.mem) || (o.unk && !self.unk) || (o.lost_rom && !self.lost_rom);
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
        flags || toks
    }

    /// [`Val::join`] into a value as the cap leaves it (one only ever
    /// joined into, from empty: an entry, a result): when `o` adds
    /// nothing, without the cap (a large callee set's results, joined
    /// one by one, capped each time, was most of a large game's run).
    fn join_capped(&mut self, o: &Val) -> bool {
        let adds = (o.mem && !self.mem)
            || (o.unk && !self.unk)
            || (o.lost_rom && !self.lost_rom)
            || !(o.toks.is_empty()
                || std::rc::Rc::ptr_eq(&self.toks.0, &o.toks.0)
                || o.toks.is_subset(&self.toks));
        adds && self.join(o)
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
        // Functions' tokens sort first: only those after them count.
        for t in self.toks.range(Tok::Num(0)..REF0) {
            match *t {
                Tok::Num(_) => nums += 1,
                Tok::Rom(_) => roms += 1,
                Tok::RomFrom(_) => froms = true,
                _ => {}
            }
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
    /// The origins stored where the run can't place them (the pile).
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

#[derive(Default)]
struct G {
    entry: BTreeMap<u32, Vec<Val>>,
    ret: BTreeMap<u32, Vec<Val>>,
    /// Every store the run can't place (the heap, escaping frame slots).
    pile: Mem,
    /// [`G::read_pile`]'s, by width: uncapped (grown at each store), and
    /// capped (made again when the first changes).
    pile_raw: [Val; 5],
    pile_reads: [Option<Val>; 5],
    cells: BTreeMap<u32, Mem>,
    /// [`G::read_cells`]'s, by width, the same way.
    cell_raw: [Val; 5],
    cell_reads: [Option<Val>; 5],
    spills: BTreeMap<u32, Mem>,
    /// [`G::ref_val`]'s, brought up to date with [`G::mem_log`] when
    /// looked at again.
    ref_exp: std::collections::HashMap<Tok, Exp, Fx>,
    /// Each token stored new to the pile (`false`) or a cell (`true`),
    /// with the width stored, in order: what a kept [`Exp`] hasn't read.
    mem_log: Vec<(bool, Tok, u8)>,
    /// Those that read ROM alone, kept for good.
    ref_rom: std::collections::HashMap<Tok, Val, Fx>,
    /// [`G::expand`]'s of a value with several Refs, by its set (kept, so
    /// the address isn't reused) and flags: the Refs' values it was made
    /// of, and it.
    #[allow(clippy::type_complexity)]
    expanded: std::collections::HashMap<
        (*const BTreeSet<Tok>, bool, bool, bool),
        (Toks, Vec<Val>, Val),
        Fx,
    >,
    /// [`Run::split_refs`]' tokens a map leaves of a Ref's value, by the
    /// Ref and map, with the value's set they came of (the same while it
    /// is).
    split_rest: std::collections::HashMap<(Tok, Xf), (Toks, Toks), Fx>,
    /// The [`Tok::Ref`]s stored to the pile (their origins are piled too).
    piled_refs: BTreeSet<Tok>,
    /// What a load at a width reads of a function's spills, and of the
    /// cells at an address (kept until they change).
    spill_reads: std::collections::HashMap<(u32, u8), Val, Fx>,
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
    dirty_entry: BTreeSet<u32>,
    dirty_ret: BTreeSet<u32>,
    dirty_loose: bool,
    /// The functions incomplete callee sets reach this round, and what
    /// their results join to (kept until one of them changes).
    loose: BTreeSet<u32>,
    loose_rets: Option<Vec<Val>>,
    /// When each function's results last changed (a count of changes).
    ret_stamp: BTreeMap<u32, u64>,
    stamp: u64,
    /// Each call by value's last callees, arguments and results, by
    /// function, address and whether a tail call: a large callee set given
    /// the same arguments again changes nothing, and its results only
    /// when a callee's did.
    by_value: std::collections::HashMap<(u32, u32, bool), ByValue>,
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
    ranges: Vec<(u32, u32)>,
    /// It made a call whose callee set is incomplete (it reads every
    /// loose function's results).
    incomplete: bool,
}

impl G {
    fn mark(&mut self, c: bool) {
        self.changed |= c;
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

    /// [`G::hole`] of one function's token, all forms.
    fn hole_one(&mut self, o: Origin, at: u32, why: Why) {
        if self.holed.insert(o) {
            self.flow.holes.insert(o, (at, why));
        }
    }

    /// What a load at width `w` reads of function `f`'s spills.
    fn read_spill(&mut self, f: u32, w: u8, into: &mut Val) {
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
                        .extend(new.iter().map(|&(t, sw)| (false, t, sw)));
                    self.pile_changed = true;
                    Some(&mut self.pile_raw)
                }
                Store::Cell(a) => {
                    self.cell_reads = Default::default();
                    self.mem_log
                        .extend(new.iter().map(|&(t, sw)| (true, t, sw)));
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
                    Some(&mut self.cell_raw)
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

    /// A load at width `w` reads the pile (`cells`: every cell): its
    /// [`Tok::Ref`] and the read's flags.
    fn read_mem(&mut self, cells: bool, w: u8, into: &mut Val) {
        let r = if cells {
            self.cells_read(w)
        } else {
            self.pile_read(w)
        };
        let (mem, unk, lost_rom) = (r.mem, r.unk, r.lost_rom);
        into.join(&Val {
            toks: [mem_ref(cells, w)].into_iter().collect(),
            mem,
            unk,
            lost_rom,
        });
    }

    fn read_pile(&mut self, w: u8, into: &mut Val) {
        self.read_mem(false, w, into);
    }

    fn read_cells(&mut self, w: u8, into: &mut Val) {
        self.read_mem(true, w, into);
    }

    /// What [`Tok::Ref`] `r` holds now, with no `Ref` in it (kept, and
    /// brought up to date with what memory gained since: made again at
    /// each store, a large game's 30k-token reads were most of its run);
    /// the running function reads the memory it does.
    fn ref_val(&mut self, r: Tok) -> Val {
        if let Some(v) = self.ref_rom.get(&r) {
            return v.clone();
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
                // Of ROM alone: it never changes.
                if e.pile | e.cells == 0 {
                    self.ref_rom.insert(r, e.v.clone());
                    return e.v;
                }
                e
            }
        };
        if e.at < self.mem_log.len() {
            self.exp_update(&mut e);
        }
        self.rd.pile |= e.pile;
        self.rd.all_cells |= e.cells;
        let v = e.v.clone();
        self.ref_exp.insert(r, e);
        v
    }

    /// Reads `work` (and the reads it reaches through the `Ref`s memory
    /// holds, each with the forms the `Ref`s on the way give) into `e`,
    /// those it hasn't read yet, whole. Memory is read as stored, uncapped
    /// (it holds no numbers, so the cap at the end widens the same ROM
    /// pointers), so that what it gains later can be added alone.
    fn exp_add(&mut self, e: &mut Exp, mut work: Vec<(Src, Xf)>) {
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
                Src::RomFrom(lo, end, w) => self.rom_from[&(lo, end, w)].clone(),
                Src::Hull(lo, hi, w) => self.rom_hull[&(lo, hi, w)].clone(),
            };
            exp_take(&mut e.v, &read, xf, &mut work);
        }
        e.v.drop_range(REF0..);
        e.v.cap();
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
            .filter(|(s, _)| matches!(s, Src::Pile(_) | Src::Cells(_)))
            .copied()
            .collect();
        for &(cells, t, sw) in &self.mem_log[e.at..] {
            for &(src, xf) in &mems {
                let w = match src {
                    Src::Cells(w) if cells => w,
                    Src::Pile(w) if !cells => w,
                    _ => continue,
                };
                let mut one = Val::default();
                from_mem_one(t, sw, w, &mut one);
                exp_take(&mut d, &one, xf, &mut work);
            }
        }
        e.at = self.mem_log.len();
        d.drop_range(REF0..);
        let v = &mut e.v;
        if (d.mem && !v.mem)
            || (d.unk && !v.unk)
            || (d.lost_rom && !v.lost_rom)
            || d.toks.iter().any(|t| !v.toks.contains(t))
        {
            v.union(&d);
            v.cap();
        }
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
        let refs: Vec<Tok> = v.refs().collect();
        let parts: Vec<Val> = refs.iter().map(|&r| self.ref_val(r)).collect();
        let key = (std::rc::Rc::as_ptr(&v.toks.0), v.mem, v.unk, v.lost_rom);
        let same = |a: &Val, b: &Val| {
            std::rc::Rc::ptr_eq(&a.toks.0, &b.toks.0)
                && (a.mem, a.unk, a.lost_rom) == (b.mem, b.unk, b.lost_rom)
        };
        if parts.len() > 1
            && let Some((_, was, out)) = self.expanded.get(&key)
            && was.len() == parts.len()
            && was.iter().zip(&parts).all(|(a, b)| same(a, b))
        {
            return out.clone();
        }
        let mut out = v.clone();
        out.drop_range(REF0..);
        for x in &parts {
            out.union(x);
        }
        out.cap();
        if parts.len() > 1 {
            if self.expanded.len() >= 4096 {
                self.expanded.clear();
            }
            self.expanded
                .insert(key, (v.toks.clone(), parts, out.clone()));
        }
        out
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

    fn give(&mut self, f: u32, regs: &[Val]) {
        let e = self
            .entry
            .entry(f)
            .or_insert_with(|| vec![Val::default(); 16]);
        let mut c = false;
        for (a, b) in e.iter_mut().zip(regs) {
            c |= a.join_capped(b);
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
    fn give_all(&mut self, ks: &BTreeSet<u32>, regs: &[Val]) {
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
            (usize, *const BTreeSet<Tok>, bool, bool, bool),
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
                    c |= a.join_capped(b);
                    continue;
                }
                let key = (i, std::rc::Rc::as_ptr(&a.toks.0), a.mem, a.unk, a.lost_rom);
                let before = (a.toks.len(), a.mem, a.unk, a.lost_rom);
                match memo.get(&key) {
                    Some((_, v)) => *a = v.clone(),
                    None => {
                        let old = a.toks.clone();
                        a.join_capped(b);
                        memo.insert(key, (old, a.clone()));
                    }
                }
                c |= before != (a.toks.len(), a.mem, a.unk, a.lost_rom);
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

    fn give_ret(&mut self, f: u32, regs: &[Val]) {
        let e = self.ret.entry(f).or_insert_with(|| vec![Val::default(); 4]);
        let mut c = false;
        for (a, b) in e.iter_mut().zip(regs) {
            c |= a.join_capped(b);
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
            Tok::Rom(_) | Tok::RomFrom(_) if w == 4 => out.push((t, 4)),
            // As it is ([`G::for_mem`] expands any other).
            Tok::Ref { .. } => {
                debug_assert!(
                    ref_keeps(t, w),
                    "a Ref memory doesn't keep: {t:?} at width {w}"
                );
                out.push((t, w));
            }
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
    // mark: more exact, as compiled code reads them back.)
    (!xf.roms || w == 4)
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
        Tok::Rom(_) | Tok::RomFrom(_) if w == 4 => t,
        // Read at the width it was stored at, it is what was stored; a
        // word from pieces, reassembled; narrower, its bytes.
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
    seen: BTreeSet<(Src, Xf)>,
    pile: u8,
    cells: u8,
    at: usize,
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
    }
    let mut toks = Vec::new();
    for &t in read.toks.range(..REF0) {
        match t {
            Tok::Fn { key, origin, form } => {
                if let Some(form) = xf.form(form) {
                    toks.push(Tok::Fn { key, origin, form });
                }
            }
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
    loose: &'a BTreeSet<u32>,
    ins: BTreeMap<u32, St>,
    /// Blocks to run, lowest address first (near enough reverse
    /// postorder for compiled code, so loops settle in fewer passes).
    work: BTreeSet<u32>,
    trace: bool,
    /// The functions whose results the run read.
    rets_read: BTreeSet<u32>,
    /// The instruction being run.
    at: u32,
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

    /// [`Run::rom_read`] into `into` as [`Val::cap`] leaves it: a number
    /// or ROM pointer the cap would drop again (one held, one unknown takes
    /// in, one a pointer "from here on" covers) is left out without a cap
    /// (a cap at each of a long read's words was most of a large game's
    /// run).
    fn rom_read_or_capped(&mut self, a: u32, w: u8, into: &mut Val, capped: bool) {
        if capped {
            self.rom_read_capped(a, w, into);
        } else {
            self.rom_read(a, w, into);
        }
    }

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
            .find(|&c| !(fns_in(c.saturating_sub(16), c) && fns_in(c, c.saturating_add(16))))
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
            let v = rom_ref(v, Src::RomFrom(lo, end, w));
            into.join(&v);
            return;
        }
        let mut p = lo & !3;
        let mut v = Val {
            // Cut short: the words past the cut go unread.
            lost_rom: end < hi,
            ..Val::default()
        };
        while p < end {
            self.rom_read_capped(p, w.min(4), &mut v);
            p += 4;
        }
        into.join(&rom_ref(&v, Src::RomFrom(lo, end, w)));
        self.g.rom_from.insert((lo, end, w), v);
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
                    let rest: Toks = e
                        .toks
                        .iter()
                        .filter(|t| !matches!(t, Tok::Fn { form, .. } if m(*form).is_some()))
                        .copied()
                        .collect();
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
        // Gathered, and added at the end (a large set's one at a time was
        // most of a large game's run), as the holes of functions' tokens.
        let mut toks = Vec::new();
        let mut holes = Vec::new();
        for (x, y) in pairs {
            {
                match bin_tok(self.img, op, x, y) {
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
            // With the pile's tokens in, to look at them.
            let xv = |g: &mut G, x: ValueId| g.expand(&vals[x.index()]);
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
                        out
                    } else {
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
                            Tok::RomFrom(_) => out.unk = true,
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
                            Tok::Rom(_) | Tok::RomFrom(_) | Tok::Ref { .. } if keep => toks.push(t),
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
                    // A saved register's slot: the register as it came, unknown.
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
                            if !esc && *o < 0 {
                                return v.clone();
                            }
                            out.join(v);
                        }
                        _ => self.g.read_spill(self.f.entry, w, &mut out),
                    }
                    // At or above the entry sp: the caller's frame (a stack
                    // argument), which its stores reached through the pile.
                    if esc || *o >= 0 {
                        self.g.read_pile(w, &mut out);
                        out.mem = true;
                    }
                    return out;
                }
                Some(super::frame::Place::Through(_)) => {
                    self.g.read_spill(self.f.entry, w, &mut out);
                    self.g.read_pile(w, &mut out);
                    return out;
                }
                _ => {}
            }
        }
        let mut anywhere = a.unk || a.mem;
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
                    self.rom_from(r, w, hull, None, &mut out);
                    capped = true;
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
                    anywhere = true;
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
                            out.join(&rom_ref(v, Src::Hull(lo, hi, w)));
                        }
                        None => {
                            let mut v = Val::default();
                            let mut p = lo & !3;
                            while p <= hi {
                                self.rom_read_capped(p, w, &mut v);
                                p += 4;
                            }
                            out.join(&rom_ref(&v, Src::Hull(lo, hi, w)));
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
                                .filter(|(t, _)| *t != OTHER && *t != LOST)
                                .map(|(t, _)| *t),
                        );
                        v.lost_rom = x.lost_rom;
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
                    self.g.store(Store::Cell(ram_canon(c)), toks)
                }
                // I/O, the display memories, SRAM: not followed.
                Tok::Num(c) | Tok::Fn { key: c, .. } if self.img.word(c).is_none() => {
                    self.g.hole(x, self.at, Why::Outside, |_| true)
                }
                Tok::Num(_) | Tok::Rom(_) | Tok::RomFrom(_) | Tok::Fn { .. } => {}
                Tok::Ref { .. } => debug_assert!(false, "a Ref not expanded"),
            }
            if matches!(t, Tok::RomFrom(_) | Tok::Ref { .. }) {
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
                // CpuSet and CpuFastSet from ROM copy its words to RAM: their
                // length's, or the source object's.
                if matches!(out.bios, Some(0x0B | 0x0C)) {
                    let mut got = Val {
                        lost_rom: s.regs[0].lost_rom,
                        ..Val::default()
                    };
                    let n = copy_len(out.bios == Some(0x0C), &s.regs[2]);
                    for &t in &self.g.expand(&s.regs[0]).toks {
                        match t {
                            Tok::Rom(a) => self.rom_from(a, 4, None, n, &mut got),
                            Tok::RomFrom(a) => self.rom_from(a, 4, None, None, &mut got),
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
        // A large callee set is given only what it wasn't given before
        // (the entries hold that already: they only grow), and its results
        // are joined again only from the callees whose results changed.
        let key = (self.f.entry, at, tail);
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
                    .any(|d| !d.toks.is_empty() || d.mem || d.unk || d.lost_rom || d.marked())
                {
                    self.g.give_all(&callees, &delta);
                    for (o, a) in m.args.iter_mut().zip(args) {
                        o.union(a);
                    }
                }
                let mut grew = false;
                for &k in &callees {
                    self.rets_read.insert(k);
                    if self.g.ret_stamp.get(&k).is_some_and(|&s| s > m.stamp) {
                        for (a, b) in m.rets.iter_mut().zip(&self.g.ret[&k]) {
                            grew |= a.union_new(b);
                        }
                    }
                }
                if grew {
                    for a in &mut m.rets {
                        a.cap();
                    }
                }
                m.stamp = self.g.stamp;
                let rets = m.rets.clone();
                self.g.by_value.insert(key, m);
                rets
            }
            None => {
                let mut rets = vec![Val::default(); 4];
                self.g.give_all(&callees, args);
                for &k in &callees {
                    for (a, b) in rets.iter_mut().zip(self.ret_of(k)) {
                        a.union_new(&b);
                    }
                }
                for a in &mut rets {
                    a.cap();
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
                c |= a.join_capped(b);
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
                                a.join_capped(b);
                            }
                        }
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

fn shift(kind: ShiftType, x: u32, n: u32) -> u32 {
    match kind {
        ShiftType::Lsl => x.checked_shl(n).unwrap_or(0),
        ShiftType::Lsr => x.checked_shr(n).unwrap_or(0),
        ShiftType::Asr => ((x as i32) >> n.min(31)) as u32,
        ShiftType::Ror => x.rotate_right(n),
    }
}

/// A number: a ROM pointer if it is in ROM.
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
    let _code = CodeGuard::set(img);
    let mut g = G {
        loose_args: vec![Val::default(); 4],
        objects: objects(img),
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
    // (its entry, a callee's results, the memory it read, what incomplete
    // sets pass, the functions they reach). Memory by what a load gives:
    // the pile and every cell by width (compared with what they gave at
    // the round's start), the cells by address.
    let mut reads: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut rds: BTreeMap<u32, Rd> = BTreeMap::new();
    let mut dirty: BTreeSet<u32> = fns.iter().map(|f| f.entry).collect();
    let trace = std::env::var_os("GBAA_FNREF_TRACE").is_some();
    let snap = |g: &mut G| -> Vec<Val> {
        let mut v = Vec::new();
        for w in [1, 2, 4] {
            let (p, c) = (g.pile_read(w).clone(), g.cells_read(w).clone());
            v.push(p);
            v.push(c);
        }
        v
    };
    let mut snapped = snap(&mut g);
    g.cells_changed.clear();
    g.pile_changed = false;
    let mut loose: BTreeSet<u32> = BTreeSet::new();
    let mut ran = 0usize;
    for round in 0..=256 {
        g.changed = false;
        let was = std::mem::replace(
            &mut loose,
            fns.iter()
                .map(|f| f.entry)
                .filter(|k| !direct.contains(k) || g.taken.contains(k))
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
            return finish(g);
        }
        if round == 256 {
            break;
        }
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
                at: 0,
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
            reads.insert(f.entry, r);
            rds.insert(f.entry, std::mem::take(&mut g.rd));
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
        let cells = std::mem::take(&mut g.cells_changed);
        if std::mem::take(&mut g.pile_changed) || !cells.is_empty() {
            let now = snap(&mut g);
            for (i, w) in [1u8, 2, 4].into_iter().enumerate() {
                if now[2 * i] != snapped[2 * i] {
                    pile_w |= 1 << w;
                }
                if now[2 * i + 1] != snapped[2 * i + 1] {
                    cells_w |= 1 << w;
                }
            }
            snapped = now;
        }
        dirty.extend(
            rds.iter()
                .filter(|(_, r)| {
                    r.pile & pile_w != 0
                        || r.all_cells & cells_w != 0
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
            "function addresses (native/fnref.rs){}: {} function words in ROM ({pools} in literal pools, {} other data), {proven} proven, {} not: {unread} never read{} ({piled} of the {} not proven stored where the run can't place them, the heap's pile); {} code constants, {} proven; {} call_fn calls with a callee set, {complete} of them complete; {} other jumps by value to a function",
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
