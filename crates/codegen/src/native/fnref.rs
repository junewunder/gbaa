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
//!   ([`Image::bound`]), the next ROM data address a ROM word holds
//!   ([`objects`]) outside a run of function words, or the next table
//!   the code indexes from ([`tables`], `native/fnref-table-ends`), and
//!   only those the value-set analysis bounds the load to
//!   (`native/fnref-rom-read-bound`); through a table's start plus an
//!   index scaled by a record's size ([`Tok::RomStep`], the scale from
//!   the block's own shifts and adds: [`strides`]) and a field's offset,
//!   it reads only that field of each record (`native/fnref-table-fields`:
//!   a byte flag beside two handlers gives the flag's bytes, not the
//!   handlers');
//! - the program start's copies of ROM data to RAM (`Discovery::copies`
//!   that aren't code) and `CpuSet` and `CpuFastSet` from ROM; from
//!   anywhere else they copy what a load of their source reads, to the
//!   pile ([`Run::bios_read`], `native/fnref-bios-ram-copies`).
//!
//! Memory: each function's own frame words, exactly where the frame's run
//! placed the access ([`super::frame`]) and the slot doesn't escape (a
//! saved register's slot reads as unknown, the register as it came; one
//! at or above the entry sp, a stack argument, reads the stacks' cell
//! too); RAM words at constant addresses, one cell each; for each RAM
//! object ([`ram_parts`]: RAM cut at the literals the code indexes from),
//! one cell of what was stored somewhere in it, through a pointer
//! [`Tok::Ram`]; one for the stacks ([`STACK`]: sp and what is made of
//! it, frame slots that escape); and one pile for every other store (a
//! pointer the run dropped, [`RAM_ANY`]). A load at a constant address
//! reads its cell, its object's and the pile; through a pointer somewhere
//! in an object, the cells at the constant addresses in that object
//! ([`in_cell`]), that object's and the pile; through a
//! dropped one, the pile, every cell and every object's; through a value
//! that holds no RAM pointer (an unknown, a number read from memory: the
//! closed world, below), nothing in RAM, but where the value-set analysis
//! bounds it to ROM, the words there (to the object's end, where the
//! bounds start at a literal the function loads: a table read from its
//! start), and where it bounds it to RAM, those cells and objects.
//! (`native/fnref-ram-pointers`; with `GBAA_FNREF_RAM` set, [`ram_on`].
//! Off, the default, there are no objects and no stacks' cell: an
//! escaping slot or a stack argument is the pile's, and a load through a
//! value the run can't place, or a ROM pointer "from here on" stored
//! through, reads or writes the pile and every cell, or the cells the
//! value sets bound it to.) Three bounds keep the objects from costing
//! more than they tell (`native/fnref-ram-cost`): a value with pointers
//! into more than [`OBJS_MAX`] objects holds "somewhere in some object"
//! ([`ANYOBJ`]), a load through pointers into more than [`OBJS_READ`]
//! reads every object's cell as one read, and more than [`OBJ_REFS`]
//! reads of single objects' cells in a value are reads of every
//! object's. Interprocedural: a call passes r0-r3 to its callee's entry
//! and takes back what its returns hold; a tail call or a fall passes
//! every register, and so does a call to a function that may take r4-r11
//! from its callers or through a pointer the holes file's `[[takes]]`
//! names ([`Out::takes`]: the lift passes those the caller's own). An
//! indirect call passes them to each function whose
//! whole address its target may hold. A call whose target holds none is
//! an incomplete callee set: its arguments go to every function that may
//! be called that way (one with no direct caller, or whose address is
//! taken), and its results come back from all of them. Functions rerun
//! (a worklist, by rounds) until nothing they read changes: their entry,
//! a callee's results, and of memory only what their loads gave (the pile
//! or every cell at a width, compared with what it gave at the round's
//! start; the cells at the addresses they read). With the RAM pointers
//! on, by the values those reads gave (`native/fnref-ram-reruns`): a
//! function that looked into a kept read reruns when that read holds
//! another value ([`Rd::refs`], [`Exp::ver`]), one that only loaded it
//! when the read's flags change, and a read of an object's cell is always
//! such a read ([`G::read_obj`]). By the memory read, a store of a
//! callback's address to one more global reran every function that had
//! looked at the pile, which holds a read of every object's cell; the
//! `GBAA_FNREF_TRACE` round lines say what reran how many. And a
//! function a run changed the entry of, or a callee's results for, runs
//! in the same round if it comes later in the order (the pickup). The memory
//! reads are kept and grown store by store, and a call through a large callee set
//! gives only what it hadn't before, the callees that held the same set
//! sharing what it joins to. On Metroid (2,616 functions) the run
//! settles in 12 rounds, about 150 s on quack
//! (`native/fnref-flow-scale`), holding about 30 million tokens at a
//! peak of 1.35 GB. A run that holds more than [`TOKENS`] stops,
//! unsettled (Emerald once did, below). Kirby - Nightmare in Dream Land and Minish
//! Cap once stopped so, at about 3,000 functions run
//! (`native/whole-lift-memory`); with the changes below, Nightmare (5,177
//! functions) settles: 31 to 35 minutes at 2.0 GB with the roads
//! (`native/fnref-c-game-words`), 8 minutes at 0.45 GB in its lift
//! (`native/nightmare-fn-flow-again`). Its callee sets are pile-wide
//! (about 2,000 functions each), which once made spurious hard holes;
//! since such a set is a large value whose calls count as incomplete
//! (below), its lift runs the flow with none (`native/nightmare-flow-on`).
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
//! never finished before, ends at 177 s. That alone didn't settle it:
//! after seven rounds 12,000 entries changed in each.
//!
//! **Values the run gives up on** (`native/fnref-flow-emerald`). Three
//! kinds of value grew a few tokens a round with no end in sight, each
//! with nothing in it the run could use, and each is now one token or a
//! flag. None of them comes up on the agb or C test programs or on
//! Metroid, whose flows come out as before. (1) Pointers into bytes that
//! hold nothing the run follows ([`dull`]: instructions, and data with
//! only numbers before the next code): past [`DULLS`] of them "from here
//! on" in a value they are one [`Tok::RomDull`]. Return addresses and
//! copies of code gave Emerald's values 67,000 different ones, none
//! taking another in. (2) Pointers "from here on" into more than
//! [`FROMS`] runs of data: the value is a dropped pointer
//! ([`Val::lost_rom`]). A wide read of ROM takes every word that looks
//! like a ROM address for one. (3) More than [`LARGE`] functions'
//! addresses in a value kept between functions: they go to one set all
//! such values share, and the value keeps a reference to it
//! ([`Large::take`]); Emerald's tasks and callbacks are thousands, and
//! 40,000 registers each held their own copy, 900 million tokens between
//! them. A reference that stands for nothing is no token
//! ([`ref_tok`]). With these Emerald (15,158 functions) settles in 24
//! rounds, about 12 minutes on quack at a peak under 2 GB
//! (`memory/fnref-flow-emerald-memory.md`).
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
//! RAM addresses too (`native/fnref-ram-pointers`, `GBAA_FNREF_RAM` set;
//! off by default until its pieces land, `native/fnref-ram-switch`):
//! memory keeps each
//! RAM pointer stored as a word (an exact one, or one somewhere in an
//! object), and the program start's copies of RAM addresses, so a number
//! read from memory is none; a RAM address plus or less an index the run
//! doesn't know is somewhere in its object (`&tasks[i]`), as a ROM
//! pointer "from here on" is; any other op on one (a shift, a difference,
//! its bytes) makes a number, as on a ROM pointer. What that takes on
//! trust, as the ROM rule does: an index stays in its object (so a RAM
//! address moved out of its object by an amount that varies is still
//! somewhere in it, [`left_object`]; one constant offset is exact, a
//! field's address), and an
//! object runs to the next literal the code indexes from that isn't a
//! field of the one below, or to the BIOS's part of IWRAM
//! ([`BIOS_RAM`], [`ram_parts`]); a global the names files name is an
//! object, whole, whatever literals lie in it (GCC's globals, addressed
//! from one anchor, have none of their own: `native/fnref-ram-anchors`),
//! and an access the function's own code makes at a literal plus a
//! constant plus a scaled index is in the object at literal plus
//! constant ([`BASED`], [`G::rebased`]: GCC adds the index to the anchor
//! first) (a `memcpy` through `&s` past such a literal
//! inside `s` leaves those stores in `s`'s object, and a load there at a
//! constant address misses them); a pointer isn't made of its bytes in
//! registers (a byte copy through memory is a number); a function called
//! from where the run can't see (`loose`) is passed, besides what the
//! incomplete callee sets pass, a pointer into a stack, at a constant
//! address or one the run dropped, never one somewhere in an object
//! ([`LOOSE`] in r0-r3, `native/fnref-ram-loose-params`: a closure's
//! environment called through a vtable the run never read); with the
//! registers' rules on too ([`live_env`]), the callers the run can't see
//! are the BIOS's and nothing's (the reset, a function no code names,
//! the IRQ vector's function at 0x03007ffc) once the run has otherwise
//! settled, and every address-taken function's only from when some
//! call's callee set is incomplete, where before each such function had
//! them from the first round; and no load at a constant address reads a
//! stack.
//!
//! **The heap by allocation site** (`native/fnref-heap-sites`;
//! `GBAA_FNREF_HEAP` set, [`heap_env`], off by default, and on only over
//! the RAM pointers' rules above). A direct call of a function the holes
//! file names as an allocator (`allocators = [0x…]`) returns a pointer to
//! an allocation of its own, by call site ([`Tok::Heap`],
//! [`G::heap_alloc`]): plus or less a constant it is that offset of it,
//! plus an index the run doesn't know somewhere in it, and any other op
//! makes a number, as on a RAM pointer. A store through it goes to the
//! site's cell for that word, or to the site's any-cell; a load reads
//! that word's cell, the any-cell and the pile ([`G::read_heap`]), and no
//! global's cell: two allocations, and two fields of one, are apart. A
//! value with pointers into more than [`HEAP_SITES`] sites holds "that
//! offset of any allocation" (`free`'s parameter). What that takes on
//! trust: an index from an allocation's pointer stays at or past it, and
//! a pointer somewhere in it less a constant is still in it; no global
//! lies in an allocation, so no load at a constant address reads one; a
//! named allocator returns memory nothing else points to yet (a wrapper
//! named as one keeps no copy of the pointer it returns); the allocator's
//! own code, through its own pointers into the heap's RAM, stores only
//! numbers in an allocation (it zeroes it) and reads nothing a user
//! stored there (no `realloc` that copies: such a function isn't one to
//! name); and memory freed and allocated again at another site isn't
//! read before it is written. A
//! pointer below the one returned (`free`'s `ptr - 12`) is the
//! allocator's header: a pointer into the RAM the allocator returned
//! ([`G::heap_lower`]), read and written as that RAM is. On `c/heap`,
//! with the registers' rules on too ([`live_env`]), it proves all 8
//! function words on both compilers, where the RAM rules prove 2 and
//! main 1; alone it narrows the callee sets and proves no more
//! (`docs/research/fnref-held-words.md`, "The heap by allocation site").
//!
//! **Registers at a call** (`native/fnref-live-registers`;
//! `GBAA_FNREF_LIVE` set, [`live_env`], off by default). As above, a call
//! passes r0-r3 and takes all four back as the callee's returns hold
//! them, so a register a callee never writes comes back as every
//! caller's value of it joined, and a leftover in a scratch register
//! travels the program. On, three rules, each from the callee's summary
//! ([`crate::saves::Summary`]) where its run and its callees' didn't stop
//! at a hole: a register the callee leaves as it came on every return
//! ([`FnIn::leaves`]) is the caller's own value after the call
//! ([`G::kept`]), which is what the machine does and what the lifted
//! program has; a callee is given only the registers it reads
//! ([`FnIn::reads`]: its parameters, and those it may return as they
//! came on some path or that a BIOS or indirect call in it may leave);
//! and a call through a value no function's address has reached yet
//! waits until the run has otherwise settled before it counts as an
//! incomplete callee set ([`Run::call_by_value`]), where at once its
//! arguments went to every loose function for good and what that held
//! depended on the order the functions ran in.
//!
//! **What isn't allowed** ([`Why`]): a function's address used as an
//! address, in arithmetic but the split and join, in an ordered compare or
//! a bit test, its halves or bytes used as anything but stored or joined,
//! called in part. Lost (not followed, so not proven): given to the BIOS
//! (in a register the call reads: [`bios_regs`], or in memory a call
//! computes with through a pointer: [`bios_mem`]), moved to a user-bank
//! register. A word is **proven** when the run read
//! it and no value it entered met either. A function word in ROM the run
//! never read (no tracked load reaches it) isn't proven either.
//!
//! Nothing printed changes: the report lists the words, the holes and the
//! callee sets ([`Flow::report`]), and the metrics count the words not
//! proven (barred) and proven. `GBAA_FNREF_TRACE=1` prints each round of
//! the run (with what reran functions for memory: each kind of read, the
//! functions it reran and those nothing else did) and each use not
//! allowed, with its operands; with
//! `GBAA_FNREF_GAINS=1` too, what entries and results gained each round,
//! by kind ([`G::gain`]), and what all of them hold between them: the
//! tokens by kind, the set with the most pointers "from here on" (what
//! found each cause of Emerald's run not settling).
//! `GBAA_FNREF_WATCH=WORD[,WORD…]` prints each place those ROM words'
//! function addresses go ([`G::watch`]), and (`roads` alone turns only
//! this on) at each instruction that makes a use not allowed, what its
//! operands held each address by ([`G::roads`]):
//! `docs/research/fnref-held-words.md` is made with both, and with
//! `GBAA_FNREF_OFF` ([`off_env`]), which measures what the words would be
//! without one kind of hole. `GBAA_FNREF_RAM=1` turns the closed world's
//! RAM pointers on ([`ram_on`]), `GBAA_FNREF_HEAP=1` those and the
//! allocation sites ([`heap_env`]), `GBAA_FNREF_LIVE=1` the registers'
//! rules at a call ([`live_env`]).
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
    /// Somewhere in ROM from here on, a whole number of `step` bytes on
    /// (`native/fnref-table-fields`): one field of a table of records,
    /// the table's start plus a field's offset plus an index scaled by
    /// the record's size ([`strides`]). A read through it reads that
    /// field of each record, not the bytes between.
    RomStep(u32, u16),
    /// Somewhere in ROM bytes that hold nothing the run follows
    /// ([`dull`]): the address of an instruction, or of data with only
    /// numbers from there to the next code, that the run no longer knows
    /// exactly, or from one on (a return address a callee was passed, the
    /// source of a copy of code to RAM, a function discovery didn't find,
    /// a pool of constants, each with an unknown added). A load through
    /// it reads a number. One token for them all: each such run of bytes
    /// is its own run of data, so their pointers "from here on" never
    /// took each other in, and Emerald's values held thousands of them,
    /// 67,000 different ones (`native/fnref-flow-emerald`).
    RomDull,
    /// Somewhere in ROM, the run doesn't say where: a value that held
    /// pointers "from here on" into more runs of data than [`FROMS`]. It
    /// takes in every ROM pointer, and its value is one the run dropped
    /// ([`Val::lost_rom`], which [`Val::cap`] sets with it): a load
    /// through it reads nothing, and says so the same way. It stays, so
    /// that the value doesn't fill with pointers again.
    RomAny,
    /// Somewhere in the RAM object that starts here ([`ram_parts`]): a
    /// RAM address plus an index the run doesn't know (`&tasks[i]`), as
    /// [`Tok::RomFrom`] is in ROM (`native/fnref-ram-pointers`). Memory
    /// keeps it, a load through it reads that object, a store through it
    /// stays in it. [`RAM_ANY`] (start 0): a pointer into RAM the run
    /// dropped, anywhere.
    Ram(u32),
    /// A pointer into what one call of an allocator returned
    /// (`native/fnref-heap-sites`; `GBAA_FNREF_HEAP`, [`heap_on`]): the
    /// call site ([`G::heap_site`]'s number, from 1) and the offset from
    /// the pointer returned, exact below [`HEAP_WIN`] or [`HEAP_ANY`],
    /// somewhere at or past it (an index the run doesn't know). A store
    /// through it goes to that site's cell for that word ([`heap_cell`]),
    /// a load reads it: two allocations' fields, and two fields of one,
    /// are kept apart, where one pile held them all. Site 0: that offset
    /// of any allocation, in a value that held pointers into more sites
    /// than [`HEAP_SITES`] (`free`'s parameter).
    Heap(u32, u32),
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
/// of every cell, of the ROM in `lo..end` a read from `lo` on gives
/// ([`Run::rom_from`]: every word, or with a step, the field at `lo` of
/// each record), or of the ROM words in `lo..=hi` (a read the value-set
/// analysis bounds there); or the functions' addresses of every large
/// value ([`Large::take`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Src {
    Pile(u8),
    Cells(u8),
    RomFrom(u32, u32, u8, u16),
    Hull(u32, u32, u8),
    /// What a load at width `w` reads of what was stored somewhere in a
    /// RAM object, by its cell ([`obj_cell`]; [`STACK_CELL`]: in a
    /// stack): [`G::read_obj`].
    Obj(u32, u8),
    /// Of what was stored somewhere in any object or stack.
    Objs(u8),
    Large,
}

impl Src {
    fn w(self) -> u8 {
        match self {
            Src::Pile(w)
            | Src::Cells(w)
            | Src::RomFrom(_, _, w, _)
            | Src::Hull(_, _, w)
            | Src::Obj(_, w)
            | Src::Objs(w) => w,
            Src::Large => 4,
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

    /// The forms in `mask` (bit `f as u8`) as they are, the rest dropped:
    /// a large value's ([`Large::take`]).
    fn only(mask: u8) -> Xf {
        Xf::of(|f| (mask & 1 << f as u8 != 0).then_some(f))
    }

    /// [`Xf::only`]'s mask, if this is one.
    fn only_mask(self) -> Option<u8> {
        let mut mask = 0;
        for (i, &m) in self.map.iter().enumerate() {
            if m == i as u8 {
                mask |= 1 << i;
            } else if m != Xf::DROP {
                return None;
            }
        }
        (!self.roms && !self.rom_mem && !self.flags).then_some(mask)
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
}

thread_local! {
    /// The code addresses of the run's image, sorted, for [`bound`]
    /// (empty outside [`follow`]: every address is then one run of data).
    static CODE: std::cell::RefCell<std::rc::Rc<Vec<u32>>> = std::cell::RefCell::default();
    /// The image's dull bytes ([`dull`]), as sorted ranges.
    static DULL: std::cell::RefCell<std::rc::Rc<Vec<(u32, u32)>>> = std::cell::RefCell::default();
}

/// Whether a read from ROM address `a` on, to the next code, reads
/// nothing the run follows: `a` is inside an instruction of the run's
/// image ([`Image::code_at`]: compiled code doesn't keep a function's
/// address in one), or in data before the next instruction whose words
/// from `a` on are neither a function's address nor a pointer to ROM
/// data (a pointer into an instruction is as good as a number). Nothing
/// is, outside [`follow`] or where the image names no instruction.
fn dull(a: u32) -> bool {
    DULL.with(|c| {
        let c = c.borrow();
        let i = c.partition_point(|r| r.0 <= a);
        i > 0 && a < c[i - 1].1
    })
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

/// Sets [`CODE`] and [`DULL`] for a run; the ones before come back when
/// it's dropped.
struct CodeGuard(std::rc::Rc<Vec<u32>>, std::rc::Rc<Vec<(u32, u32)>>);

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
        // Each instruction's bytes (at most four from its start), joined
        // where they touch.
        let mut insts: Vec<(u32, u32)> = Vec::new();
        for &k in std::iter::once(&0x0800_0000).chain(&v) {
            let n = (0..4).take_while(|&i| img.code_at(k + i)).count() as u32;
            if n == 0 {
                continue;
            }
            match insts.last_mut() {
                Some(r) if r.1 >= k => r.1 = r.1.max(k + n),
                _ => insts.push((k, k + n)),
            }
        }
        // And the end of the data before each run of instructions, after
        // its last word the run follows: a function's address, or a
        // pointer into ROM that isn't into an instruction.
        // (Not into a run's last instruction, from which a read goes on
        // through the data after it.)
        let in_inst = |a: u32| {
            let i = insts.partition_point(|r| r.0 <= a);
            i > 0 && a < insts[i - 1].1 && {
                let k = v.partition_point(|&k| k < insts[i - 1].1);
                k > 0 && a < v[k - 1]
            }
        };
        let mut froms: Vec<u32> = Vec::with_capacity(insts.len());
        let mut data = 0x0800_0000u32;
        for &(lo, hi) in &insts {
            let mut from = lo;
            while from > data {
                let p = (from - 1) & !3;
                match img.word(p) {
                    Some(w) if !img.is_fn(w) && (img.word(w).is_none() || in_inst(w)) => {
                        from = p.max(data)
                    }
                    // (A read at a run's first instruction, not on a
                    // word's start, reads this word too.)
                    _ => {
                        from = from.max(p + 4);
                        break;
                    }
                }
            }
            froms.push(from);
            data = hi;
        }
        // A read from a run's last instruction on goes through the data
        // after it, to the next code: that instruction is dull only if
        // all of that data is.
        let mut dull: Vec<(u32, u32)> = Vec::with_capacity(insts.len());
        for (i, &(lo, hi)) in insts.iter().enumerate() {
            let through = froms.get(i + 1).is_some_and(|&f| f == hi);
            let end = if through {
                hi
            } else {
                // The last instruction's start: the last code address
                // below `hi`.
                let k = v.partition_point(|&k| k < hi);
                if k > 0 && v[k - 1] > lo { v[k - 1] } else { lo }
            };
            if froms[i] >= end {
                continue;
            }
            match dull.last_mut() {
                Some(r) if r.1 >= froms[i] => r.1 = end,
                _ => dull.push((froms[i], end)),
            }
        }
        CodeGuard(
            CODE.with(|c| c.replace(std::rc::Rc::new(v))),
            DULL.with(|c| c.replace(std::rc::Rc::new(dull))),
        )
    }
}

impl Drop for CodeGuard {
    fn drop(&mut self) {
        CODE.with(|c| *c.borrow_mut() = self.0.clone());
        DULL.with(|c| *c.borrow_mut() = self.1.clone());
    }
}

/// The most tokens the run holds (each distinct set once, the kept reads'
/// and expansions' too, and memory's: [`G::held`]) before it stops,
/// unsettled, as when its rounds run out: proving
/// nothing then, as a lift without the flow (`skip_fn_flow`). Metroid
/// settles holding about 30 million at a peak of 1.4 GB, and Kirby -
/// Nightmare in Dream Land at 0.45 GB to 2.0 GB. An earlier flow held over 100
/// million on Nightmare and Minish Cap in a round and a half, past 15 GB
/// (`native/whole-lift-memory`).
const TOKENS: usize = 48_000_000;

/// [`G::mem_log`]'s tag of a store to the pile, and to a cell; one to an
/// object's or a stack's cell is tagged by it.
const PILE: u32 = 0;
const CELLS: u32 = 1;
/// And to any object's or stack's cell.
const OBJS: u32 = 2;

/// A pointer into RAM the run dropped ([`Tok::Ram`]): a load through it
/// reads the pile and every cell, a store goes to the pile.
const RAM_ANY: Tok = Tok::Ram(0);

/// sp and what is made of it ([`Tok::Ram`]): somewhere in a stack. The
/// frames' slots a function's run can't keep apart ([`super::frame`]:
/// one whose address escapes, a caller's) are kept together in their own
/// cell, [`STACK_CELL`], which a load through such a pointer reads with
/// the pile (no load at a constant address does: no global is in a
/// stack).
const STACK: Tok = Tok::Ram(1);
const STACK_CELL: u32 = 0x0fff_fff0;

/// A loose function's parameter ([`Tok::Ram`],
/// `native/fnref-ram-loose-params`): a pointer from a caller the run
/// can't see (an `&mut dyn` closure's environment, called through a
/// vtable the run never read), so somewhere in RAM it doesn't place. A
/// load through it reads every cell, the stacks' cell and the pile, as an
/// unknown address did with the switch off ([`ram_on`]), but no RAM
/// object's (what [`RAM_ANY`] adds); a store through it goes to the pile.
const LOOSE: Tok = Tok::Ram(2);

/// A [`Tok::Heap`]'s offset: somewhere in the allocation, at or past the
/// pointer its call returned (an index stays in its object, as C has it).
const HEAP_ANY: u32 = 0xffff_fff0;
/// A [`Tok::Heap`]'s offset below the pointer the call returned: the
/// allocator's own header, which is the heap's RAM, not the allocation.
/// Never kept: the op that makes it gives pointers into the RAM the
/// allocator returned instead ([`G::heap_lower`]).
const HEAP_UNDER: u32 = 0xffff_fff1;
/// Exact offsets into an allocation are kept below this.
const HEAP_WIN: u32 = 0xff00;
/// More exact offsets into one allocation than this in a value: somewhere
/// in it ([`HEAP_ANY`]).
const HEAP_OFFS: usize = 8;
/// Pointers into more allocation sites than this in a value: somewhere in
/// any allocation ([`HEAP_ALL`]; `free` and a `memset` take every site's).
const HEAP_SITES: usize = 16;
/// Somewhere in any allocation ([`Tok::Heap`] of site 0, which is any
/// site): a store through a pointer into site 0 goes to one cell every
/// allocation's loads read, a load reads every site's cells.
const HEAP_ALL: Tok = Tok::Heap(0, HEAP_ANY);
/// Where the allocations' cells are kept, outside the address space and
/// past the RAM objects' ([`obj_cell`]): 64 KB a site, a cell a word.
const HEAP_BASE: u32 = 0x2000_0000;
/// The most allocation sites: a call past it returns what the allocator
/// did, a pointer into the heap's RAM.
const HEAP_MAX: u32 = 0x3fff;

/// The cell of the word at `off` of allocation site `s` ([`Tok::Heap`]);
/// [`HEAP_ANY`]: of what was stored somewhere in it.
fn heap_cell(s: u32, off: u32) -> u32 {
    let at = if off >= HEAP_WIN { 0xfff8 } else { off & !3 };
    HEAP_BASE + (s << 16) + at
}

/// A pointer `d` bytes on from offset `k` of site `s`'s allocation.
fn heap_off(s: u32, k: u32, d: i64) -> Tok {
    if k == HEAP_ANY || k == HEAP_UNDER {
        return Tok::Heap(s, k);
    }
    match k as i64 + d {
        n if n < 0 => Tok::Heap(s, HEAP_UNDER),
        n if n >= HEAP_WIN as i64 => Tok::Heap(s, HEAP_ANY),
        n => Tok::Heap(s, n as u32),
    }
}

/// Somewhere in some RAM object ([`Tok::Ram`]): a value that held
/// pointers into more objects than [`OBJS_MAX`]. A load through it reads
/// every cell, every object's cell and the pile; a store goes to one cell
/// of its own, [`ANYOBJ_CELL`], which a load at a constant address or
/// through a pointer into an object reads with that object's
/// (`native/fnref-ram-cost`).
const ANYOBJ: Tok = Tok::Ram(3);
const ANYOBJ_CELL: u32 = 0x0fff_fff8;
/// More RAM objects than this pointed into by one value: somewhere in
/// some object ([`ANYOBJ`]). A parameter every caller passes its own
/// global to held dozens, each stored through and read through in turn.
const OBJS_MAX: usize = 8;
/// A load through a pointer into more RAM objects than this reads every
/// object's cell as one read ([`Src::Objs`]), not each object's as its
/// own.
const OBJS_READ: usize = 4;
/// More reads of single objects' cells ([`Src::Obj`]) than this in a
/// value: each is the read of every object's cell at its width and forms
/// ([`Src::Objs`]), which holds what it held and more. With the RAM
/// pointers on, Metroid's registers held hundreds of such reads between
/// them (one an object, width and form), and every shift, compare and
/// call looked through each: its run took five times main's
/// (`native/fnref-ram-cost`). Eight: a load through a pointer into an
/// object is three such reads ([`G::read_in_obj`]), each a `Ref` however
/// little it holds ([`G::read_obj`]), and two loads joined stay apart (at
/// four, c/fnptr built by GCC read every global for its sprite's field
/// and lost its IRQ handler's word).
const OBJ_REFS: usize = 8;

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

/// The [`Tok::Ref`] of `src` seen through `xf`, if it stands for
/// anything: one with every form `src` holds dropped, no ROM pointers
/// and no flags is no token (kept, each was one more token to hand from
/// value to value, a call deeper a round).
fn ref_tok(src: Src, xf: Xf) -> Option<Tok> {
    (xf.roms || xf.rom_mem || xf.flags || read_forms(src).iter().any(|&f| xf.form(f).is_some()))
        .then_some(Tok::Ref { src, xf })
}

/// A load at width `w` of the pile's, or every cell's, words.
fn mem_ref(cells: bool, w: u8) -> Tok {
    Tok::Ref {
        src: if cells { Src::Cells(w) } else { Src::Pile(w) },
        xf: Xf::ID,
    }
}

/// More functions' tokens than this in a value kept between functions
/// or stored: they are the large set's ([`Large::take`]). Metroid's largest
/// value holds 664.
const LARGE: usize = 1024;
/// More numbers than this in a value: it's unknown.
const NUMS: usize = 8;
/// More exact RAM addresses than this in a value: each somewhere in its
/// object ([`Tok::Ram`]). Low, as a loop's pointer stored and loaded
/// back grows by one a round (Metroid's run took 26 rounds at 8, each
/// changing most entries); a load through a pointer into an object reads
/// no more than one through a value the run couldn't place used to.
const RAMS: usize = 2;
/// More exact ROM pointers than this: "from the lowest on".
const ROMS: usize = 16;
/// More pointers "from here on" than this in a value, each into its own
/// run of data: the value is a ROM pointer the run dropped
/// ([`Val::lost_rom`]), and holds none. A wide read of ROM data takes
/// every word that looks like a ROM address for a pointer, so on a large
/// game a value read from a table by an unknown index held over a
/// thousand, one into each literal pool some word of graphics or sound
/// pointed at by chance, and every load through it read them all
/// (`native/fnref-flow-emerald`). Metroid's flow comes out the same with
/// the bound as without.
const FROMS: usize = 64;
/// More pointers "from here on" into dull bytes ([`dull`]) than this in a
/// value: they are [`Tok::RomDull`]. Up to it each is kept, as exact as
/// any other (one less a constant is still a pointer; [`Tok::RomDull`]
/// less one is dropped, [`Val::lost_rom`]).
const DULLS: usize = 16;
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
/// wrong, so [`Run::object_end`] doesn't stop at these between function
/// words, only at a table the code indexes from ([`tables`]).
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

/// The tables in ROM the code reads by an index from their start: the
/// low bound of every access [`FBlock::bases`] names, with its stride (the
/// largest, where several read one table).
///
/// Such a start ends the object below it even inside a run of function
/// words, where [`objects`]' starts don't: C lays its tables of handlers
/// back to back, and indexing one past its end is undefined, so a read
/// "from here on" from below stays below. Not every such start is a new
/// object, though: a struct table read by one field is indexed from the
/// field's address (`ldr r1, =TABLE+4`), inside the table's first record.
/// So a start less than one entry past another (by either one's stride)
/// is a field of that one's records, not an end ([`Run::object_end`]).
///
/// What this takes on trust: that a literal the code indexes from at an
/// entry's stride or more past another table's start is not `&TABLE[k]`
/// of that table with both indexed. C allows it; it is rare, and a read
/// it cuts short would leave the callee set it fed incomplete without
/// saying so. Bounds the code proves with a compare need nothing here:
/// the value-set analysis narrows the index on the branch, and `hulls`
/// already holds the narrowed bounds.
fn tables(fns: &[FnIn], img: &dyn Image) -> BTreeMap<u32, u32> {
    let mut t: BTreeMap<u32, u32> = BTreeMap::new();
    for f in fns {
        for b in f.blocks.values() {
            for (i, &stride) in &b.bases {
                let (lo, _) = b.hulls[i];
                if img.word(lo).is_some() && stride > 0 {
                    let e = t.entry(lo).or_default();
                    *e = (*e).max(stride);
                }
            }
        }
    }
    t
}

/// What [`ram_indexed`] knows of a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RamIx {
    /// A constant.
    K(u32),
    /// A RAM literal, plus an offset, plus indexes of this step (0: none
    /// yet).
    B(u32, u32, u32),
    Other,
}

impl RamIx {
    /// Either, where two paths meet.
    fn join(self, o: RamIx) -> RamIx {
        match (self, o) {
            (a, b) if a == b => a,
            (RamIx::B(l, k, s), RamIx::B(m, j, t)) if l == m && k == j => RamIx::B(l, k, gcd(s, t)),
            _ => RamIx::Other,
        }
    }
}

/// The loads and stores of a function whose address is a RAM literal it
/// loads, a constant offset, and an index it scales
/// (`native/fnref-ram-array-bases`): `ldr r2, =ARRAY; lsls r3, r1, #2;
/// adds r2, r3, r2; str r0, [r2]`. By block and inst index: the literal
/// and the index's step (the record's size: the factor [`factors`]
/// finds, taken as exact, as an index into the array can't wrap). The
/// offset is less than one step, a field of a record; one more (a
/// struct's array field indexed from the struct's address, `s.a[i]` from
/// `&s`) isn't kept. Followed through the registers from block to block
/// of the function, where every path in agrees; not across a call or a
/// BIOS call (a callee needn't keep a register).
fn ram_indexed(f: &FnIn) -> BTreeMap<(u32, usize), (u32, u32)> {
    let mut ins: BTreeMap<u32, [RamIx; 16]> = BTreeMap::new();
    let mut out = BTreeMap::new();
    let first = f.blocks.keys().next().copied();
    ins.insert(f.entry, [RamIx::Other; 16]);
    if let Some(k) = first {
        ins.entry(k).or_insert([RamIx::Other; 16]);
    }
    let mut work: BTreeSet<u32> = ins.keys().copied().collect();
    while let Some(k) = work.pop_first() {
        let Some(b) = f.blocks.get(&k) else { continue };
        let (found, exits) = ram_indexed_block(b, ins[&k]);
        // (Run again with more paths in, a block's sums may no longer be
        // what they were: those of its first run go.)
        out.retain(|&(blk, key), _| blk != k || key & BASED == 0);
        for (i, x) in found {
            out.insert((k, i), x);
        }
        for (to, regs) in exits {
            if !f.blocks.contains_key(&to) {
                continue;
            }
            let new = match ins.get(&to) {
                Some(old) => {
                    let mut j = *old;
                    for (a, b) in j.iter_mut().zip(regs) {
                        *a = a.join(b);
                    }
                    (j != *old).then_some(j)
                }
                None => Some(regs),
            };
            if let Some(j) = new {
                ins.insert(to, j);
                work.insert(to);
            }
        }
    }
    out
}

/// [`ram_indexed_block`]'s key of a value it finds stored or passed, not
/// an address (or'd with the inst's index).
const VALUE: usize = 1 << 30;
/// And of a sum that is a RAM literal, a constant and a scaled index
/// (`anchor + i * 4 + 8`: GCC's globals, addressed from one anchor,
/// `native/fnref-ram-anchors`), at the instruction that makes it: the
/// literal and the constant, whatever the step. The run takes the sum's
/// object from that ([`G::rebased`]), where its own arithmetic, literal
/// plus index first, is in the object at the literal; every use of the
/// sum (an access, a call's argument, a result, a spill) then has it.
const BASED: usize = 1 << 29;

/// One block of [`ram_indexed`], from the registers `regs` in: what it
/// finds, and the registers at each block of the function it goes on to.
#[allow(clippy::type_complexity)]
fn ram_indexed_block(
    b: &FBlock,
    mut regs: [RamIx; 16],
) -> (BTreeMap<usize, (u32, u32)>, Vec<(u32, [RamIx; 16])>) {
    use RamIx::*;
    let insts = b.insts;
    let fac = factors(insts);
    // The step an index adds: its factor, 1 if nothing is known.
    let step = |x: ValueId| match fac[x.index()].1 {
        f @ 1..=0xffff_ffff => f as u32,
        _ => 1,
    };
    let mut v: Vec<RamIx> = Vec::with_capacity(insts.len());
    let mut found = BTreeMap::new();
    let mut exits = Vec::new();
    let add = |a: RamIx, b: RamIx, bx: ValueId| match (a, b) {
        (K(x), K(y)) => K(x.wrapping_add(y)),
        (B(l, k, s), K(c)) => B(l, k.wrapping_add(c), s),
        (B(l, k, s), Other) => B(l, k, gcd(s, step(bx))),
        _ => Other,
    };
    for (i, inst) in insts.iter().enumerate() {
        let r = match inst.op {
            Op::Const(c) => K(c),
            Op::Bit(b) => K(b as u32),
            Op::Pool(c) if is_ram(c) => B(c, 0, 0),
            Op::Pool(c) => K(c),
            Op::GetReg(r) => regs[r.index()],
            Op::SetReg(r, x) => {
                let x = v[x.index()];
                let r = &mut regs[r.index()];
                *r = if inst.guard.is_none() { x } else { r.join(x) };
                Other
            }
            Op::SetCpsr { .. } | Op::SetSpsr { .. } | Op::SetUserReg(..) => {
                regs = [Other; 16];
                Other
            }
            Op::Bin(BinOp::Add, a, b) => match (v[a.index()], v[b.index()]) {
                (y, x @ B(..)) => add(x, y, a),
                (x, y) => add(x, y, b),
            },
            Op::Adc { a, b, c } if v[c.index()] == K(0) => match (v[a.index()], v[b.index()]) {
                (y, x @ B(..)) => add(x, y, a),
                (x, y) => add(x, y, b),
            },
            Op::Bin(BinOp::Sub, a, b) => match (v[a.index()], v[b.index()]) {
                (K(x), K(y)) => K(x.wrapping_sub(y)),
                (B(l, k, s), K(c)) => B(l, k.wrapping_sub(c), s),
                _ => Other,
            },
            Op::Load { addr, .. } | Op::Store { addr, .. } => {
                if let B(l, k, s) = v[addr.index()]
                    && s > 0
                    && k < s
                {
                    found.insert(i, (l, s));
                }
                // A pointer made so and stored (`cur = &tasks[i]`): its
                // literal starts an object as well (`native/fnref-ram-pointers`).
                if let Op::Store { value, .. } = inst.op
                    && let B(l, k, s) = v[value.index()]
                    && s > 0
                    && k < s
                {
                    found.insert(i | VALUE, (l, s));
                }
                Other
            }
            Op::Exit(_) => {
                // Or passed to a call.
                if matches!(
                    b.exits.get(&i).map(|o| &o.edge),
                    Some(Edge::Call { .. } | Edge::Tail(_))
                ) {
                    for (r, x) in regs[..4].iter().enumerate() {
                        if let B(l, k, s) = *x
                            && s > 0
                            && k < s
                        {
                            found.insert(i | VALUE | (r + 1) << 24, (l, s));
                        }
                    }
                }
                match b.exits.get(&i).map(|o| &o.edge) {
                    Some(Edge::Goto(ks) | Edge::TailOr(ks)) => {
                        exits.extend(ks.iter().map(|&k| (k, regs)));
                    }
                    Some(Edge::Call { ret: k, .. } | Edge::Swi(k)) => exits.push((*k, [Other; 16])),
                    _ => {}
                }
                Other
            }
            _ => Other,
        };
        // A sum that is a literal, a constant and a scaled index: where
        // the run takes its object from ([`BASED`]).
        if let (B(l, k, s), Op::Bin(..) | Op::Adc { .. }) = (r, inst.op)
            && s > 0
        {
            found.insert(i | BASED, (l, k));
        }
        v.push(r);
    }
    (found, exits)
}

/// The objects in RAM the code indexes from: the start of each, its
/// largest step, and where it ends, by start. A start is a literal
/// [`ram_indexed`] finds, or the low bound of an access [`FBlock::bases`]
/// names in RAM. An object ends at the next start that isn't a field of
/// one below (less than one entry past it, by either one's step, as
/// [`Run::object_end`] has it in ROM), or at its RAM's end.
///
/// What this takes on trust is the ROM rule's: that a literal the code
/// indexes from at an entry or more past another start is an object of
/// its own, not `&ARRAY[k]` of the array below indexed both ways; and
/// that an index stays in its array, as C has it. A store the run
/// can't place stays in its object ([`Store::Bounded`]), so a read of
/// that array from below it, or past its object's end, misses it.
#[allow(clippy::type_complexity)]
fn ram_objects(
    fns: &[FnIn],
    indexed: &[BTreeMap<(u32, usize), (u32, u32)>],
) -> BTreeMap<u32, (u32, u32)> {
    let mut steps: BTreeMap<u32, u32> = BTreeMap::new();
    let mut add = |lo: u32, s: u32| {
        let e = steps.entry(ram_canon(lo)).or_default();
        *e = (*e).max(s);
    };
    for (f, ix) in fns.iter().zip(indexed) {
        for (_, &(l, s)) in ix.iter().filter(|(k, _)| k.1 & BASED == 0) {
            add(l, s);
        }
        for b in f.blocks.values() {
            for (i, &s) in &b.bases {
                let (lo, _) = b.hulls[i];
                if is_ram(lo) && s > 0 {
                    add(lo, s);
                }
            }
        }
    }
    // Records longer than this aren't looked for (as in ROM).
    const RECORD: u32 = 0x1000;
    let field = |c: u32| {
        let own = steps[&c];
        steps
            .range(c.saturating_sub(RECORD)..c)
            .any(|(x, s)| c - x < (*s).max(own))
    };
    steps
        .iter()
        .map(|(&lo, &s)| {
            let top = if lo >> 24 == 2 {
                0x0204_0000
            } else {
                0x0300_8000
            };
            let end = steps
                .range(lo + 1..top)
                .map(|(c, _)| *c)
                .find(|&c| !field(c))
                .unwrap_or(top);
            (lo, (s, end))
        })
        .collect()
}

/// RAM cut into objects for [`Tok::Ram`] (`native/fnref-ram-pointers`):
/// by start, where each ends. The starts [`ram_objects`] finds that
/// aren't a field of one below, each RAM's first byte, the BIOS's
/// part of IWRAM ([`BIOS_RAM`]), and each global the names files name
/// (`named`: start and size), whole; each object runs to the next start,
/// so every RAM address is in one.
fn ram_parts(fns: &[FnIn], named: &BTreeMap<u32, u32>) -> BTreeMap<u32, u32> {
    let indexed: Vec<_> = fns.iter().map(ram_indexed).collect();
    let objs = ram_objects(fns, &indexed);
    let mut starts: BTreeSet<u32> = [0x0200_0000, 0x0300_0000].into_iter().collect();
    // A field's start lies inside the object below it, which ends where
    // the next start that isn't a field begins.
    let ends: BTreeSet<u32> = objs.values().map(|&(_, e)| e).collect();
    starts.extend(objs.keys().filter(|&&o| {
        objs.range(..o)
            .next_back()
            .is_none_or(|(_, &(_, e))| e <= o)
            || ends.contains(&o)
    }));
    // The top of IWRAM is the BIOS's: the IRQ and supervisor stacks and
    // its own words (the IRQ flags at 0x03007ff8, the vector at
    // 0x03007ffc), from 0x03007f00 up, where the user stack starts and
    // grows down. No array of the program's lies there, so the last one
    // it indexes doesn't run through them.
    starts.insert(BIOS_RAM);
    // The globals the names files name, each an object, whole: a start
    // the code's literals put inside one is no start (a struct's array
    // member indexed from its own literal: a pointer through `&s` covers
    // it), and what follows one is another object. With these, GCC's
    // globals, which it addresses from one anchor and so have no literal
    // of their own, are objects too (`native/fnref-ram-anchors`).
    let mut spans: Vec<(u32, u32)> = named
        .iter()
        .filter(|&(&a, &n)| is_ram(a) && n > 0)
        .map(|(&a, &n)| (ram_canon(a), n))
        .filter(|&(a, n)| a.checked_add(n).is_some_and(|e| e <= ram_top(a)))
        .map(|(a, n)| (a, a + n))
        .collect();
    // (One inside or across another, a member with a symbol of its own:
    // one object between them.)
    spans.sort_unstable();
    let mut named: Vec<(u32, u32)> = Vec::new();
    for (lo, hi) in spans {
        match named.last_mut() {
            Some(l) if lo < l.1 => l.1 = l.1.max(hi),
            _ => named.push((lo, hi)),
        }
    }
    starts.retain(|s| !named.iter().any(|&(lo, hi)| lo < *s && *s < hi));
    for &(lo, hi) in &named {
        starts.insert(lo);
        if hi < ram_top(lo) {
            starts.insert(hi);
        }
    }
    let v: Vec<u32> = starts.into_iter().collect();
    v.iter()
        .enumerate()
        .map(|(i, &o)| {
            let top = ram_top(o);
            let end = v.get(i + 1).copied().filter(|&n| n < top).unwrap_or(top);
            (o, end)
        })
        .collect()
}

/// The word the BIOS's IRQ stub calls through (0x03fffffc, as the
/// program writes it, is its mirror): a function stored there has a
/// caller the run doesn't see. One put there through a pointer the run
/// can't place, or by a BIOS copy, isn't found so; its registers at
/// entry are the interrupted code's, which a handler doesn't read.
const IRQ_VECTOR: u32 = 0x0300_7ffc;

/// Where the BIOS's part of IWRAM starts: its stacks and its words, the
/// IRQ flags and vector among them. The user stack starts here and grows
/// down.
const BIOS_RAM: u32 = 0x0300_7f00;

/// The end of the RAM `a` is in.
fn ram_top(a: u32) -> u32 {
    if a >> 24 == 2 {
        0x0204_0000
    } else {
        0x0300_8000
    }
}

thread_local! {
    /// The run's [`ram_parts`] (empty outside [`follow`]: each RAM is one
    /// object).
    static RAM_OBJS: std::cell::RefCell<std::rc::Rc<BTreeMap<u32, u32>>> = std::cell::RefCell::default();
    /// Whether the run keeps RAM pointers ([`ram_on`]).
    static RAM_ON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `GBAA_FNREF_LIVE` as set: whether a call takes back only the registers
/// its callee may change (`native/fnref-live-registers`), off by default.
/// The flow took all of r0-r3 back as the callee's returns hold them, so
/// a register a callee never writes came back as every caller's value of
/// it joined: a handler called through r0 that returns nothing "returned"
/// its own address, and a call with an empty callee set took those back
/// from every address-taken function. On, a register the callee leaves as
/// it came on every return ([`FnIn::leaves`]) keeps the caller's own
/// value after the call, which is what the machine does and what the
/// lifted program has (a shaped callee doesn't return it). Off, the flow
/// is main's.
pub fn live_env() -> bool {
    std::env::var_os("GBAA_FNREF_LIVE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// `GBAA_FNREF_HEAP` as set: whether the run splits the heap by
/// allocation site (`native/fnref-heap-sites`), off by default. On, the
/// RAM pointers' rules are on too ([`ram_on`]: without the closed world a
/// load through an unknown address would read every allocation), and a
/// direct call of an allocator the holes file names (`allocators =
/// [0x…]`) returns a pointer to an object of its own ([`Tok::Heap`]).
/// Off, the list is not looked at and no such token is made.
pub fn heap_env() -> bool {
    std::env::var_os("GBAA_FNREF_HEAP").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Whether the run follows RAM pointers in a closed world
/// (`native/fnref-ram-pointers`): `GBAA_FNREF_RAM` set, off by default
/// (`native/fnref-ram-switch`), so its rules land one by one while the
/// flow stays main's. Off, no [`Tok::Ram`] is made, a RAM address is a
/// number as any other, and memory is the frames, the cells and the pile.
fn ram_on() -> bool {
    RAM_ON.with(|c| c.get())
}

/// `GBAA_FNREF_RAM` as set ([`ram_on`]).
fn ram_env() -> bool {
    std::env::var_os("GBAA_FNREF_RAM").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Sets [`RAM_ON`] and [`RAM_OBJS`] for a run, and clears them after.
struct ObjsGuard;

impl ObjsGuard {
    fn set(ram: bool, fns: &[FnIn], named: &BTreeMap<u32, u32>) -> ObjsGuard {
        RAM_ON.with(|c| c.set(ram));
        if ram {
            let objs = ram_parts(fns, named);
            RAM_OBJS.with(|c| *c.borrow_mut() = std::rc::Rc::new(objs));
        }
        ObjsGuard
    }
}

impl Drop for ObjsGuard {
    fn drop(&mut self) {
        RAM_ON.with(|c| c.set(false));
        RAM_OBJS.with(|c| *c.borrow_mut() = std::rc::Rc::default());
    }
}

/// The object RAM address `a` is in ([`ram_parts`]): its start and end.
fn ram_obj(a: u32) -> (u32, u32) {
    let a = ram_canon(a);
    let base = a & 0xff00_0000;
    RAM_OBJS.with(|c| {
        c.borrow()
            .range(base..=a)
            .next_back()
            .map(|(&o, &e)| (o, e))
            .unwrap_or((base, ram_top(a)))
    })
}

/// Where what is stored somewhere in object `o` is kept: a cell of its
/// own outside the address space (8 bytes apart, so no read of one
/// overlaps another's), which a read of every cell reads too.
fn obj_cell(o: u32) -> u32 {
    let i = if o >> 24 == 2 {
        o & 0x3_ffff
    } else {
        0x4_0000 | (o & 0x7fff)
    };
    0x1000_0000 + (i << 3)
}

/// Where what was stored at the constant addresses inside object `o` is
/// kept together, beside each address's own cell: what a load through a
/// pointer somewhere in `o` reads of them ([`G::read_in_obj`]). A cell of
/// its own, as [`obj_cell`] is, past those.
fn in_cell(o: u32) -> u32 {
    obj_cell(o) + 0x0800_0000
}

/// Whether cell `k` is an [`in_cell`]: a copy of constant addresses'
/// cells, which a read of every cell holds already, and so no part of
/// "what was stored somewhere in an object" ([`G::objs_all`]). (Counted
/// there, every store to a global reran each function that read every
/// object's cell: 13,000 of Emerald's 15,000 a round.)
fn is_in_cell(k: u32) -> bool {
    (0x1800_0000..HEAP_BASE).contains(&k)
}

/// The RAM object whose [`obj_cell`] `c` is.
fn obj_of_cell(c: u32) -> Option<u32> {
    let i = c.checked_sub(0x1000_0000)? >> 3;
    if c & 7 != 0 || i >= 0x4_8000 {
        return None;
    }
    Some(if i < 0x4_0000 {
        0x0200_0000 | i
    } else {
        0x0300_0000 | (i & 0x7fff)
    })
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
        // (Whether it did: a pointer into code that [`Tok::RomDull`]
        // takes in, counted with the exact pointers above, changes
        // nothing, and an entry said to change each round never settles.)
        let was = (self.toks.clone(), self.mem, self.unk, self.lost_rom);
        self.join(o);
        if was == (self.toks.clone(), self.mem, self.unk, self.lost_rom) {
            // The set it was, not an equal copy.
            self.toks = was.0;
            return false;
        }
        true
    }

    /// Whether this, as the cap leaves it, takes token `t` in with no more
    /// than the cap dropping `t` again: it holds `t`, or `t` is a number
    /// and it is unknown, or a ROM pointer one of its "from here on"
    /// takes in (which still counts toward the cap's ROMS:
    /// [`Val::join_capped`]).
    fn takes_in(&self, t: Tok) -> bool {
        self.toks.contains(&t)
            || match t {
                Tok::Num(c) if is_ram(c) && ram_on() => {
                    self.toks.contains(&RAM_ANY)
                        || self.toks.contains(&ANYOBJ)
                        || self.toks.contains(&Tok::Ram(ram_obj(c).0))
                }
                Tok::Ram(o) => {
                    o != 0 && self.toks.contains(&RAM_ANY) || o > 3 && self.toks.contains(&ANYOBJ)
                }
                Tok::Heap(s, k) => {
                    self.toks.contains(&HEAP_ALL)
                        || self.toks.contains(&Tok::Heap(0, k))
                        || k != HEAP_ANY && self.toks.contains(&Tok::Heap(s, HEAP_ANY))
                }
                Tok::Num(_) => self.unk,
                Tok::Ref {
                    src: Src::Obj(k, w),
                    xf,
                } => self.toks.contains(&Tok::Ref {
                    src: if is_in_cell(k) {
                        Src::Cells(w)
                    } else {
                        Src::Objs(w)
                    },
                    xf,
                }),
                Tok::Rom(_) | Tok::RomFrom(_) | Tok::RomStep(..) | Tok::RomDull
                    if self.toks.contains(&Tok::RomAny) =>
                {
                    true
                }
                Tok::RomFrom(x) | Tok::RomStep(x, _)
                    if dull(x) && self.toks.contains(&Tok::RomDull) =>
                {
                    true
                }
                Tok::Rom(x) if ram_on() && dull(x) && self.toks.contains(&Tok::RomDull) => true,
                Tok::Rom(x) | Tok::RomFrom(x) | Tok::RomStep(x, _) => self
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
        let (mut nums, mut roms, mut froms, mut steps) = (0, 0, false, 0);
        let (mut rams, mut objs) = (0, false);
        let mut heaps = false;
        let ram = ram_on();
        // Whether it holds [`Tok::RomDull`], or [`Tok::RomAny`].
        let (mut has_dull, mut has_any) = (false, false);
        // Functions' tokens sort first: only those after them count.
        for t in self.toks.range(Tok::Num(0)..REF0) {
            match *t {
                Tok::Num(c) if ram && is_ram(c) => rams += 1,
                Tok::Ram(_) => objs = true,
                Tok::Heap(..) => heaps = true,
                Tok::Num(_) => nums += 1,
                Tok::Rom(_) => roms += 1,
                Tok::RomFrom(_) => froms = true,
                Tok::RomStep(..) => steps += 1,
                Tok::RomDull => has_dull = true,
                Tok::RomAny => has_any = true,
                _ => {}
            }
        }
        // Too many fields' pointers (a loop moving one by less than a
        // record): each every word from its start on.
        if steps > ROMS {
            let los: Vec<u32> = self
                .toks
                .range(Tok::RomStep(0, 0)..=Tok::RomStep(u32::MAX, u16::MAX))
                .filter_map(|t| match *t {
                    Tok::RomStep(a, _) => Some(a),
                    _ => None,
                })
                .collect();
            self.drop_range(Tok::RomStep(0, 0)..=Tok::RomStep(u32::MAX, u16::MAX));
            for lo in los {
                self.toks.insert(Tok::RomFrom(lo));
            }
            froms = true;
        }
        if nums > NUMS {
            self.unk = true;
        }
        // Unknown takes in every number but RAM's (the closed world: an
        // unknown is no RAM address), so the cap stays monotone.
        if self.unk && nums > 0 {
            if ram {
                self.drop_range(Tok::Num(0)..Tok::Num(0x0200_0000));
                self.drop_range(Tok::Num(0x0400_0000)..=Tok::Num(u32::MAX));
            } else {
                self.drop_range(Tok::Num(0)..=Tok::Num(u32::MAX));
            }
        }
        // Too many RAM addresses: each somewhere in its object; a pointer
        // into an object, or one dropped, takes in those it holds.
        if ram && (rams > RAMS || (objs && rams > 0) || self.toks.contains(&RAM_ANY)) {
            let cs: Vec<u32> = self
                .toks
                .range(Tok::Num(0x0200_0000)..Tok::Num(0x0400_0000))
                .filter_map(|t| match *t {
                    Tok::Num(c) => Some(c),
                    _ => None,
                })
                .collect();
            let any = self.toks.contains(&RAM_ANY);
            let mut gone = Vec::new();
            let mut new = Vec::new();
            for c in cs {
                let o = Tok::Ram(ram_obj(c).0);
                if any || rams > RAMS || self.toks.contains(&o) {
                    gone.push(Tok::Num(c));
                    if !any && rams > RAMS {
                        new.push(o);
                    }
                }
            }
            if any {
                gone.extend(self.toks.range(Tok::Ram(1)..=Tok::Ram(u32::MAX)).copied());
            }
            for t in gone {
                self.toks.remove(&t);
            }
            for t in new {
                self.toks.insert(t);
            }
        }
        // Pointers into more objects than [`OBJS_MAX`], or any beside
        // [`ANYOBJ`]: somewhere in some object, which takes in every
        // exact RAM address too.
        // (Also those the block above just made of exact addresses.)
        if ram && (objs || rams > RAMS) {
            let n = self.toks.range(Tok::Ram(4)..=Tok::Ram(u32::MAX)).count();
            let any = self.toks.contains(&ANYOBJ);
            if n > OBJS_MAX || any && n > 0 {
                self.drop_range(Tok::Ram(4)..=Tok::Ram(u32::MAX));
                self.toks.insert(ANYOBJ);
            }
            if self.toks.contains(&ANYOBJ) && !self.toks.contains(&RAM_ANY) {
                self.drop_range(Tok::Num(0x0200_0000)..Tok::Num(0x0400_0000));
            }
        }
        if heaps {
            self.cap_heap();
        }
        // More reads of single objects' cells than [`OBJ_REFS`], or any
        // beside the read of every object's at its width and forms: that
        // read of every object's.
        if ram && self.toks.range(REF0..).next().is_some() {
            let objs: Vec<Tok> = self
                .toks
                .range(REF0..)
                .filter(|t| {
                    matches!(
                        t,
                        Tok::Ref {
                            src: Src::Obj(..),
                            ..
                        }
                    )
                })
                .copied()
                .collect();
            if !objs.is_empty() {
                let all = |t: Tok| match t {
                    // (A copy of constant addresses' cells: every cell's.)
                    Tok::Ref {
                        src: Src::Obj(k, w),
                        xf,
                    } if is_in_cell(k) => Tok::Ref {
                        src: Src::Cells(w),
                        xf,
                    },
                    Tok::Ref {
                        src: Src::Obj(_, w),
                        xf,
                    } => Tok::Ref {
                        src: Src::Objs(w),
                        xf,
                    },
                    t => t,
                };
                let many = objs.len() > OBJ_REFS;
                for t in objs {
                    if many || self.toks.contains(&all(t)) {
                        self.toks.remove(&t);
                        if many {
                            self.toks.insert(all(t));
                        }
                    }
                }
            }
        }
        // Somewhere in ROM takes in every pointer there.
        if has_any {
            self.drop_range(Tok::Rom(0)..Tok::RomAny);
            self.lost_rom = true;
            return;
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
        if !froms && !has_dull {
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
        let fields: Vec<Tok> = self
            .toks
            .range(Tok::RomStep(0, 0)..=Tok::RomStep(u32::MAX, u16::MAX))
            .copied()
            .collect();
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
        let (mut covered, mut ri, mut fi) = (0, 0, 0);
        let field_at = |t: Tok| match t {
            Tok::RomStep(a, _) => a,
            _ => unreachable!(),
        };
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
            while fi < fields.len() && field_at(fields[fi]) < lo {
                fi += 1;
            }
            while fi < fields.len() && field_at(fields[fi]) < end {
                gone.push(fields[fi]);
                fi += 1;
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
        // More than [`DULLS`] of them into dull bytes ([`dull`]), or one
        // beside [`Tok::RomDull`]: all of them are that one token. (An
        // exact pointer there stays: one more is a function's key where
        // hand-written code makes a Thumb address of a label.)
        let gone: Vec<Tok> = self
            .toks
            .range(Tok::RomFrom(0)..Tok::RomDull)
            .filter(|t| matches!(**t, Tok::RomFrom(a) | Tok::RomStep(a, _) if dull(a)))
            .copied()
            .collect();
        let dull_now = has_dull || gone.len() > DULLS;
        if gone.len() > DULLS || has_dull && !gone.is_empty() {
            let s = &mut self.toks;
            for t in gone {
                s.remove(&t);
            }
            s.insert(Tok::RomDull);
        }
        // With the RAM pointers' rules: an exact pointer into dull bytes
        // beside [`Tok::RomDull`] is that token too. Kept exact, a join
        // that passed [`ROMS`] folded it in, and the next join of the
        // value it came from put it back: on native-tasks three entries
        // changed so every round, and the run never settled
        // (`native/fnref-ram-pointers`). [`Val::takes_in`] agrees.
        // (An exact one there, one more, is a function's key where
        // hand-written code makes a Thumb address of a label. Past
        // [`ROMS`] of them the widening above folds them all the same,
        // with the switch off too; marking the value as holding a dropped
        // address here left 8 of Metroid's 70 complete callee sets
        // complete.)
        if dull_now && ram {
            let exact: Vec<Tok> = self
                .toks
                .range(Tok::Rom(0)..=Tok::Rom(u32::MAX))
                .filter(|t| matches!(**t, Tok::Rom(a) if dull(a)))
                .copied()
                .collect();
            for t in exact {
                self.toks.remove(&t);
            }
        }
        // Still pointers "from here on" into more runs of data than
        // [`FROMS`]: the run gives them up, for good ([`Tok::RomAny`]).
        if self.toks.range(Tok::RomFrom(0)..Tok::RomDull).count() > FROMS {
            self.drop_range(Tok::Rom(0)..Tok::RomAny);
            self.toks.insert(Tok::RomAny);
            self.lost_rom = true;
        }
    }

    /// [`Val::cap`]'s widening of pointers into allocations
    /// ([`Tok::Heap`]): somewhere in a site's takes in its exact offsets,
    /// more exact ones than [`HEAP_OFFS`] are somewhere in it, and more
    /// sites than [`HEAP_SITES`], or any beside site 0's, are site 0's:
    /// any allocation's, at the same offsets.
    fn cap_heap(&mut self) {
        let all = Tok::Heap(0, 0)..=Tok::Heap(u32::MAX, u32::MAX);
        let hs: Vec<(u32, u32)> = self
            .toks
            .range(all.clone())
            .filter_map(|t| match *t {
                Tok::Heap(s, k) => Some((s, k)),
                _ => None,
            })
            .collect();
        let mut sites: Vec<(u32, usize, bool)> = Vec::new();
        for &(s, k) in &hs {
            match sites.last_mut() {
                Some(l) if l.0 == s => {
                    l.1 += 1;
                    l.2 |= k == HEAP_ANY;
                }
                _ => sites.push((s, 1, k == HEAP_ANY)),
            }
        }
        if sites.len() > HEAP_SITES || sites[0].0 == 0 && sites.len() > 1 {
            self.drop_range(all);
            let ks: BTreeSet<u32> = hs.iter().map(|h| h.1).collect();
            if ks.len() > 1 && (ks.contains(&HEAP_ANY) || ks.len() > HEAP_OFFS) {
                self.toks.insert(HEAP_ALL);
            } else {
                self.toks.extend(ks.into_iter().map(|k| Tok::Heap(0, k)));
            }
            return;
        }
        for (s, n, any) in sites {
            if n > 1 && (any || n > HEAP_OFFS) {
                self.drop_range(Tok::Heap(s, 0)..=Tok::Heap(s, u32::MAX));
                self.toks.insert(Tok::Heap(s, HEAP_ANY));
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
    /// Whether `a` is inside an instruction: no data a load could read a
    /// function's address from ([`num`]).
    fn code_at(&self, _a: u32) -> bool {
        false
    }
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
    /// A call that passes every register, not r0-r3 only: to a callee
    /// that may take r4-r11 from its callers (`Summary::may_take`), or a
    /// pointer call the holes file's `[[takes]]` names. The lift passes
    /// such a callee the caller's own r4-r11.
    pub takes: bool,
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
    /// Of those, the ones whose bounds start at a literal the function
    /// loads (so they read a table from its start on, by an index), with
    /// the bounds' stride: the size of the table's entries.
    pub bases: BTreeMap<usize, u32>,
}

/// One function.
pub struct FnIn<'a> {
    pub entry: u32,
    pub blocks: BTreeMap<u32, FBlock<'a>>,
    /// Its frame ([`super::frame`]): where its sp-relative accesses go.
    pub frame: Option<&'a Frame>,
    /// Of r0-r3 (bit `n` for rN), those it leaves as they came on every
    /// return, where its summary says so ([`crate::saves::Summary::exit`];
    /// 0: none, or not known). After a call of it such a register holds
    /// the caller's own value still (`native/fnref-live-registers`,
    /// [`live_env`]).
    pub leaves: u8,
    /// Of r0-r3, those a call gives it: its parameters
    /// ([`crate::saves::Summary::params`]) and each register it may
    /// return as it came on one path and changed on another; all four
    /// (0xf) where its summary doesn't say. A caller's leftover in a
    /// register it doesn't read isn't passed on through it.
    pub reads: u8,
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
    /// Why it isn't complete (`None` when it is), for the counts.
    pub lost: Option<Lost>,
}

/// What left a callee set incomplete, as the target's value shows it at
/// the call: the first that applies, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lost {
    /// No whole function's address reaches the target.
    NoCallee,
    /// It holds the large shared set ([`LARGE`] functions or more), whose
    /// ROM pointers are given up ([`Large::take`]).
    Large,
    /// A ROM pointer given up past [`FROMS`] runs ([`Tok::RomAny`]).
    RomAny,
    /// A ROM read cut short or too wide to make, or arithmetic on a ROM
    /// pointer the run gave up on.
    RomRead,
}

impl Lost {
    pub const ALL: [Lost; 4] = [Lost::NoCallee, Lost::Large, Lost::RomAny, Lost::RomRead];

    pub fn text(self) -> &'static str {
        match self {
            Lost::NoCallee => "no function's address reaches the target",
            Lost::Large => "the target holds the large shared set (1,024 functions or more)",
            Lost::RomAny => "a ROM pointer given up (past 64 runs of data)",
            Lost::RomRead => {
                "a ROM read cut short or too wide, or arithmetic on a lost ROM pointer"
            }
        }
    }
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
    /// Each token stored new to the pile ([`PILE`]), a cell ([`CELLS`]),
    /// an object's or a stack's cell (that cell: [`G::obj_reads`]), or
    /// to any of those ([`OBJS`]), with the width stored, in
    /// order, by that tag: what a kept [`Exp`] hasn't read (a log of
    /// each, so that a Ref reads what its own memory gained, not every
    /// store's: `native/fnref-ram-pointers`).
    /// A cell's the first time any cell gets it (`cells_all`: a read of
    /// every cell holds it once, and Emerald's four thousand cells get
    /// the same tokens).
    mem_log: std::collections::HashMap<u32, Vec<(Tok, u8)>, Fx>,
    /// How many entries all of it has (an [`Exp`] that read to here has
    /// read all of its own).
    log_total: usize,
    cells_all: Mem,
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
    spill_reads: std::collections::HashMap<(u32, u8), Val, Fx>,
    cell_at_reads: std::collections::HashMap<(u32, u8), Val, Fx>,
    /// What a load at a width reads of what was stored somewhere in each
    /// RAM object or a stack, by its cell ([`G::read_obj`]): capped, kept
    /// until it changes; and uncapped, grown at each store
    /// ([`G::obj_raw_read`]). Those cells are no RAM's: a read of every
    /// cell doesn't read them, a read of every object does
    /// ([`G::objs_all`]).
    obj_reads: std::collections::HashMap<(u32, u8), Val, Fx>,
    /// What each of those gave before this round's first store to its
    /// cell: the functions that read it rerun if it gives something else
    /// at the round's end ([`Rd::at`], [`G::objs_changed`], which keeps
    /// the read of every cell stored to, so only a cell first stored to
    /// this round has none here).
    obj_was: std::collections::HashMap<(u32, u8), Val, Fx>,
    /// The count [`Exp::ver`] is taken from.
    exp_ver: u64,
    /// Whether a function reruns by the values its reads gave
    /// ([`Rd::refs`]) and not by which memory they were of: with the RAM
    /// pointers ([`ram_on`]).
    by_value_read: bool,
    obj_raw_vals: std::collections::HashMap<(u32, u8), Val, Fx>,
    /// What every object's and stack's cell holds, and its reads by
    /// width, as [`G::cells_all`] and [`G::cell_raw`] are the cells'.
    objs_all: Mem,
    objs_raw: [Val; 5],
    objs_reads: [Option<Val>; 5],
    /// The origins in [`Flow::holes`], to look up fast.
    holed: std::collections::HashSet<Origin, Fx>,
    /// Large token sets whose every origin is in [`Flow::holes`], by
    /// address: a hole of one again adds nothing. Each held weakly, which
    /// keeps its address from being reused and none of its tokens (held
    /// whole, they were 845 sets of 9 million tokens on Emerald, each a
    /// function's own; a set that is changed moves, `Rc::make_mut`).
    all_holed: std::collections::HashMap<*const BTreeSet<Tok>, std::rc::Weak<BTreeSet<Tok>>, Fx>,
    /// The functions' tokens of every large value ([`Large`]).
    large: Large,
    /// What incomplete callee sets pass.
    loose_args: Vec<Val>,
    /// Functions whose address is taken.
    taken: BTreeSet<u32>,
    /// What a read from ROM "from here on" gives, by (start, end, width,
    /// step: 1, every word).
    rom_from: BTreeMap<(u32, u32, u8, u16), Val>,
    /// The objects in ROM data ([`objects`]).
    objects: BTreeSet<u32>,
    /// The tables in ROM the code reads by an index from their start
    /// ([`FBlock::bases`]), by start: the size of their entries (the
    /// largest stride any such read steps by). Where one starts, the
    /// table below it ends ([`tables`]).
    tables: BTreeMap<u32, u32>,
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
    /// The sets kept, each once ([`Sets`]).
    sets: Sets,
    /// `GBAA_FNREF_WATCH`: the words watched (key, origin), whether the
    /// roads are printed, the lines said and how many of each word (its
    /// places, and its holes: [`WATCH_LINES`]), the function and
    /// instruction running, and, when the roads are printed, each origin
    /// as it gets its first hole ([`G::watch`], [`G::roads`]).
    watch: Vec<(u32, Origin)>,
    roads: bool,
    watch_said: BTreeSet<String>,
    watch_lines: BTreeMap<(u32, bool), usize>,
    now: (u32, u32),
    hole_log: Vec<Origin>,
    /// `GBAA_FNREF_OFF`'s kinds ([`off_env`]).
    off: u8,
    /// `GBAA_FNREF_GAINS`: what entries and results gained this round, by
    /// kind ([`G::gain`]), and the samples said of each kind.
    gains: Option<BTreeMap<String, (usize, usize)>>,
    /// The allocators (`native/fnref-heap-sites`: the holes file's
    /// `allocators`, with `GBAA_FNREF_HEAP` set; else none): a direct
    /// call of one returns a pointer to an allocation of its own
    /// ([`G::heap_alloc`]).
    allocs: BTreeSet<u32>,
    /// The allocation sites, by the call's address: their numbers in
    /// [`Tok::Heap`], from 1.
    heap_ix: BTreeMap<u32, u32>,
    /// By number less 1: the call's address, and the pointers into RAM
    /// the allocator returned there, which is where the allocation is
    /// (its header, just below it, is that RAM's: [`G::heap_lower`]).
    heap_sites: Vec<(u32, BTreeSet<Tok>)>,
    /// One of those sets grew this round.
    heap_changed: bool,
    /// By function, the registers of r0-r3 it leaves as they came
    /// ([`FnIn::leaves`]; empty unless [`live_env`]): after a call they
    /// are the caller's own, not what the callee's returns hold.
    leaves: std::collections::HashMap<u32, u8, Fx>,
    /// Those of the functions incomplete callee sets reach, between
    /// them: kept with [`G::loose_rets`].
    loose_leaves: u8,
    /// By function, the registers of r0-r3 a call gives it
    /// ([`FnIn::reads`]; empty unless [`live_env`], and a function not
    /// in it is given all four).
    reads: std::collections::HashMap<u32, u8, Fx>,
    /// A call through a value nothing has reached yet waits
    /// ([`live_env`]): the functions whose last run had one, and whether
    /// the run has otherwise settled, when they are taken as the
    /// incomplete sets they are ([`Run::call_by_value`]).
    defer: bool,
    deferred: BTreeSet<u32>,
    deferred_due: bool,
    /// Some call has had an incomplete callee set: an address-taken
    /// function may be called from it with anything.
    unseen_calls: bool,
    /// By function, block and instruction ([`ram_indexed`]'s [`BASED`]
    /// keys): the literal and the constant of a sum that is a RAM literal
    /// plus a constant plus a scaled index (with the RAM pointers on).
    based: std::collections::HashMap<(u32, u32, usize), (u32, u32), Fx>,
}

/// A kept op's key ([`G::op_kept`]): the op, a shift's amount, each
/// operand's set (null: empty, or no operand) and flags.
type OpKey = (
    u8,
    Option<u32>,
    [(*const BTreeSet<Tok>, bool, bool, bool); 2],
);

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

/// The functions' tokens of every large value ([`Large::take`]): the
/// set, each token in order as it came (an [`Exp`] reads on from where it
/// was), and whether any came this round.
#[derive(Default)]
struct Large {
    set: Toks,
    log: Vec<Tok>,
    changed: bool,
}

impl Large {
    /// Makes a large value's functions' tokens the large set's
    /// (`native/fnref-flow-emerald`): if `v` holds more than [`LARGE`] of
    /// them, or some beside a reference to the set, they go to the set
    /// and `v` keeps one [`Tok::Ref`] to it, with the forms `v` held
    /// ([`Xf::only`]). So `v` then stands for every large value's
    /// addresses in those forms: more than it held, never less.
    ///
    /// Emerald's task and callback functions are thousands, every
    /// function that takes one holds them all, and each such value
    /// gained a few tokens a round by its own road: after two rounds
    /// 40,000 registers held a set of 4,000 to 29,000 tokens each, 900
    /// million between them, and a round took twenty minutes. One set
    /// they all refer to grows once. Whoever looks at the tokens expands
    /// the reference ([`G::expand`]) and reruns when the set grows
    /// (`Rd::large`). Called where a value is kept between functions
    /// or stored: entries, results, call arguments, stores, loads.
    fn take(&mut self, v: &mut Val, rom: &std::collections::HashMap<Tok, Val, Fx>) {
        if v.toks.is_empty() {
            return;
        }
        // Its references to the set as it is, and its other references
        // that hold functions' addresses: the set's through an op's
        // forms, reads of memory, and reads of ROM (their values in
        // `rom`, [`G::ref_rom`]).
        let mut mask = 0u8;
        let mut old: Vec<Tok> = Vec::new();
        let mut refs: Vec<Tok> = Vec::new();
        for t in v.toks.range(REF0..) {
            let Tok::Ref { src, xf } = *t else { continue };
            match (src, xf.only_mask()) {
                (Src::Large, Some(m)) => {
                    mask |= m;
                    old.push(*t);
                }
                _ if read_forms(src).iter().any(|&f| xf.form(f).is_some()) => refs.push(*t),
                _ => {}
            }
        }
        if old.is_empty() && v.toks.len() <= LARGE {
            return;
        }
        let fns = v.toks.range(..Tok::Num(0)).count();
        let roms = v.toks.range(Tok::Rom(0)..Tok::Ram(0)).next().is_some();
        if old.is_empty() && fns <= LARGE || fns == 0 && old.len() == 1 && refs.is_empty() && !roms
        {
            return;
        }
        let mut new = Vec::new();
        for t in v.toks.range(..Tok::Num(0)) {
            if let Tok::Fn { form, .. } = *t {
                mask |= 1 << form as u8;
                if !self.set.contains(t) {
                    new.push(*t);
                }
            }
        }
        v.drop_range(..Tok::Num(0));
        // The other references go to the set as they are (its expansion
        // reads through them), and `v` keeps what they hold besides
        // functions' addresses: a ROM read's numbers, and of a memory
        // read a reference with every form dropped (its ROM pointers and
        // flags, as they are when someone looks).
        for r in refs {
            let Tok::Ref { src, xf } = r else { continue };
            for &f in read_forms(src) {
                if let Some(g) = xf.form(f) {
                    mask |= 1 << g as u8;
                }
            }
            if !self.set.contains(&r) {
                new.push(r);
            }
            v.toks.remove(&r);
            match (src, rom.get(&r)) {
                (_, Some(val)) => {
                    let rest: Vec<Tok> = val.toks.range(Tok::Num(0)..REF0).copied().collect();
                    v.toks.add_all(rest);
                    v.mem |= val.mem;
                    v.unk |= val.unk;
                    v.lost_rom |= val.lost_rom;
                }
                (Src::Pile(_) | Src::Cells(_), None) => {
                    let rest = Xf {
                        map: [Xf::DROP; 8],
                        ..xf
                    };
                    v.toks.extend(ref_tok(src, rest));
                }
                // The set's own, through forms: functions' addresses only.
                _ => {}
            }
        }
        if !new.is_empty() {
            self.log.extend(new.iter().copied());
            self.set.add_all(new);
            self.changed = true;
        }
        for t in old {
            v.toks.remove(&t);
        }
        v.toks.extend(ref_tok(Src::Large, Xf::only(mask)));
        // A value that may be any of a thousand functions and a pointer
        // into ROM besides is no pointer the run can follow: it gives
        // them up ([`Val::lost_rom`]). Each such value otherwise held up
        // to [`FROMS`] of them, a few more each round.
        // (Its pointers into RAM stay: an unknown is none, in the closed
        // world, so dropped with these a load through it read nothing.)
        if v.toks.range(Tok::Rom(0)..Tok::Ram(0)).next().is_some() {
            v.drop_range(Tok::Rom(0)..Tok::Ram(0));
            v.lost_rom = true;
            // (And, shifted or masked, the pointer was a number.)
            v.unk = true;
        }
        v.cap();
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
    /// It looked at the large set's tokens ([`Large::take`]).
    large: bool,
    ranges: Vec<(u32, u32)>,
    /// Those of [`Rd::ranges`], while the function runs: each is kept
    /// once ([`Rd::range`]).
    seen: std::collections::HashSet<(u32, u32), Fx>,
    /// It made a call whose callee set is incomplete (it reads every
    /// loose function's results).
    incomplete: bool,
    /// It looked at where an allocation is ([`G::heap_lower`]).
    heap: bool,
    /// Every object's and stack's cell at some widths (bit `w`,
    /// [`G::read_objs`]), and single ones' at a width ([`G::read_obj`]):
    /// kept by what each load took of the read (its flags, with its
    /// `Ref`), not by which cell was stored to (`native/fnref-ram-reruns`:
    /// a store to any object's cell reran every function that had read
    /// every object's, 13,000 of Emerald's 15,000 a round).
    objs: u8,
    at: Vec<(u32, u8)>,
    at_seen: std::collections::HashSet<(u32, u8), Fx>,
    /// The kept reads it looked into ([`G::ref_val`]), each with which
    /// value it held ([`Exp::ver`]): with the RAM pointers on, what it
    /// reruns by in place of the memory each is made of. A read of the
    /// pile is of every object's cell and the large set too, through the
    /// reads the pile holds, and a store to any of them that the read
    /// already held (a callback's address stored to one more global)
    /// reran every function that had looked at the pile: 13,000 of
    /// Emerald's 15,000 in each of its last twenty rounds.
    refs: Vec<(Tok, u64)>,
    refs_seen: std::collections::HashMap<Tok, u64, Fx>,
}

impl Rd {
    /// The run read object cell `k` at width `w`. Once each, as
    /// [`Rd::range`].
    fn cell(&mut self, k: u32, w: u8) {
        if self.at_seen.insert((k, w)) {
            self.at.push((k, w));
        }
    }

    /// The run looked at `r`'s kept value `e`. By that value
    /// (`by_value`, [`G::by_value_read`]): it reruns when `e` holds
    /// another. Else by the memory `e` is made of.
    fn exp(&mut self, r: Tok, e: &Exp, by_value: bool) {
        if by_value {
            // (The first it saw: one that changed under it reruns it.)
            self.refs_seen.entry(r).or_insert(e.ver);
            return;
        }
        self.pile |= e.pile;
        self.all_cells |= e.cells;
        for &(k, w) in &e.objs {
            if k == OBJS {
                self.objs |= 1 << w;
            } else {
                self.cell(k, w);
            }
        }
        self.large |= e.large;
    }

    /// The run read the cells in `lo..=hi`. Once each: a load says so at
    /// every pass over its block, and a kept read each time it is looked
    /// at, which on Metroid was 175 million entries for 213 thousand
    /// ranges with the RAM pointers on (`native/fnref-ram-cost`), a
    /// gigabyte the run held and walked each round.
    fn range(&mut self, lo: u32, hi: u32) {
        if self.seen.insert((lo, hi)) {
            self.ranges.push((lo, hi));
        }
    }

    /// The ranges read, sorted, those inside another left out; what the
    /// run is kept by between rounds.
    fn settle(&mut self) {
        self.seen = Default::default();
        self.at_seen = Default::default();
        self.at.shrink_to_fit();
        self.refs = std::mem::take(&mut self.refs_seen).into_iter().collect();
        self.ranges
            .sort_unstable_by_key(|&(lo, hi)| (lo, std::cmp::Reverse(hi)));
        let mut top: Option<u32> = None;
        self.ranges.retain(|&(_, hi)| match top {
            Some(t) if hi <= t => false,
            _ => {
                top = Some(hi);
                true
            }
        });
        self.ranges.shrink_to_fit();
    }
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
        if self.watch.iter().any(|w| w.1 == o) {
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
    fn read_spill(&mut self, f: u32, w: u8, into: &mut Val) {
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
            self.watch(&v, || format!("read w{w} from f_{f:08x}'s spills"));
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
            self.watch(&v, || format!("read w{w} from the cell at {c:#010x}"));
        }
    }

    /// What a load at width `w` through a pointer into a stack reads
    /// ([`STACK`]): the stacks' cell, and the pile.
    fn read_stack(&mut self, w: u8, into: &mut Val) {
        self.read_obj(STACK_CELL, w, into);
        self.read_pile(w, into);
    }

    /// What a load at width `w` through a loose function's parameter
    /// reads ([`LOOSE`]): every cell, the stacks' cell, and the pile.
    /// And, where the run has allocations ([`Tok::Heap`]), theirs: the
    /// caller it can't see may pass one's pointer. (With every object's
    /// cell, which is the read that holds them.)
    fn read_loose(&mut self, w: u8, into: &mut Val) {
        self.read_cells(w, into);
        self.read_stack(w, into);
        // (What was stored through a pointer into some object may be in
        // a cell: an exact address beside [`ANYOBJ`] is that.)
        self.read_obj(ANYOBJ_CELL, w, into);
        if !self.heap_sites.is_empty() {
            self.read_objs(w, into);
        }
    }

    /// What a load at width `w` through a pointer somewhere in RAM object
    /// `o` reads ([`Tok::Ram`]): what was stored at the constant
    /// addresses in `o` ([`in_cell`]: one cell of them all, so one read),
    /// what was stored somewhere in `o` or in some object, and the pile.
    /// No other object's cells: `obj->color` isn't the callback in the
    /// global beside the array (`native/fnref-ram-pointers`; every cell,
    /// before, as a load the run couldn't place read).
    fn read_in_obj(&mut self, o: u32, w: u8, into: &mut Val) {
        // (Somewhere in some object, [`ANYOBJ`]: every cell and every
        // object's; else this one's cells, what was stored somewhere in
        // it, and what was stored through such a pointer.)
        if o == 3 {
            self.read_cells(w, into);
            self.read_objs(w, into);
        } else {
            let o = ram_obj(o).0;
            self.read_obj(in_cell(o), w, into);
            self.read_obj(obj_cell(o), w, into);
            self.read_obj(ANYOBJ_CELL, w, into);
        }
        self.read_pile(w, into);
    }

    /// What a load at width `w` reads of what was stored somewhere in a
    /// RAM object or a stack, by its cell `k` ([`obj_cell`],
    /// [`STACK_CELL`]): a [`Tok::Ref`] whenever it holds anything, with
    /// the read's flags. Never its tokens themselves, as a small ROM read
    /// gives ([`rom_ref`]): a cell thousands of functions read (the
    /// stacks', [`ANYOBJ_CELL`]) reran them all at each store to it, and
    /// one that grew past the bound gave every value that held its tokens
    /// a `Ref` besides, which then went from results to callers a round a
    /// call (`native/fnref-ram-reruns`: Emerald's rounds 4 to 15, a third
    /// of its run). A function that looks into the `Ref` reruns when its
    /// value changes ([`Rd::refs`]); one that only loaded it, when the
    /// read's flags do or it first holds something ([`Rd::at`]).
    fn read_obj(&mut self, k: u32, w: u8, into: &mut Val) {
        self.rd.cell(k, w);
        let r = self.obj_read(k, w);
        if r.toks.is_empty() {
            let r = r.clone();
            into.union(&r);
        } else {
            let (mem, unk, lost_rom) = (r.mem, r.unk, r.lost_rom);
            into.join(&Val {
                toks: [Tok::Ref {
                    src: Src::Obj(k, w),
                    xf: Xf::ID,
                }]
                .into_iter()
                .collect(),
                mem,
                unk,
                lost_rom,
            });
        }
        if !self.watch.is_empty() {
            let v = self.obj_read(k, w).clone();
            self.watch(&v, || {
                format!("read w{w} from the RAM object cell {k:#010x}")
            });
        }
    }

    /// What a load at width `w` reads of object cell `k`, capped: kept
    /// until it changes.
    fn obj_read(&mut self, k: u32, w: u8) -> &Val {
        let cells = &self.cells;
        self.obj_reads.entry((k, w)).or_insert_with(|| {
            let mut v = Val::default();
            if let Some(m) = cells.get(&k) {
                from_mem(m, w, &mut v);
            }
            v.cap();
            v
        })
    }

    /// Its raw read, uncapped ([`G::exp_add`]): kept, and grown at each
    /// store.
    fn obj_raw_read(&mut self, k: u32, w: u8) -> Val {
        let cells = &self.cells;
        self.obj_raw_vals
            .entry((k, w))
            .or_insert_with(|| {
                let mut v = Val::default();
                if let Some(m) = cells.get(&k) {
                    from_mem(m, w, &mut v);
                }
                v
            })
            .clone()
    }

    /// The object cells of `cells` (those stored to this round) whose
    /// read at a width gives a load something else than at the round's
    /// start ([`G::read_obj`]: its flags, and whether it holds anything),
    /// with that width. One first stored to this round counts at every
    /// width.
    fn objs_changed(&mut self, cells: &BTreeSet<u32>) -> std::collections::HashSet<(u32, u8), Fx> {
        let mut was = std::mem::take(&mut self.obj_was);
        let mut out: std::collections::HashSet<(u32, u8), Fx> = Default::default();
        for &k in cells.iter().filter(|k| !is_ram(**k)) {
            for w in [1u8, 2, 4] {
                let now = self.obj_read(k, w);
                let gives = |v: &Val| (v.toks.is_empty(), v.mem, v.unk, v.lost_rom);
                if was.remove(&(k, w)).is_none_or(|v| gives(&v) != gives(now)) {
                    out.insert((k, w));
                }
            }
        }
        debug_assert!(was.is_empty());
        out
    }

    /// What a load at width `w` reads of every object's and stack's
    /// cell, capped: kept until one changes.
    fn objs_read(&mut self, w: u8) -> &Val {
        let i = w as usize;
        if self.objs_reads[i].is_none() {
            let mut v = self.objs_raw[i].clone();
            v.cap();
            self.objs_reads[i] = Some(v);
        }
        self.objs_reads[i].as_ref().unwrap()
    }

    /// A load at width `w` from anywhere reads every object's and
    /// stack's cell, as it reads every cell: the [`Tok::Ref`] and the
    /// read's flags.
    fn read_objs(&mut self, w: u8, into: &mut Val) {
        self.rd.objs |= 1 << w;
        let r = self.objs_read(w);
        let (mem, unk, lost_rom) = (r.mem, r.unk, r.lost_rom);
        into.join(&Val {
            toks: [Tok::Ref {
                src: Src::Objs(w),
                xf: Xf::ID,
            }]
            .into_iter()
            .collect(),
            mem,
            unk,
            lost_rom,
        });
    }

    /// What a direct call at `at` of an allocator returns, given what the
    /// allocator did (`ret`): a pointer to that call site's own
    /// allocation ([`Tok::Heap`], offset 0) in place of each pointer into
    /// RAM, and whatever else it returned (0 for no memory). The pointers
    /// taken out say where the allocation is: the site's, kept for its
    /// header ([`G::heap_lower`]). An allocator that returns another's
    /// result (a wrapper that zeroes) passes that one's on.
    fn heap_alloc(&mut self, at: u32, ret: &Val) -> Val {
        let n = match self.heap_ix.get(&at) {
            Some(&n) => n,
            None if self.heap_sites.len() as u32 >= HEAP_MAX => return ret.clone(),
            None => {
                self.heap_sites.push((at, BTreeSet::new()));
                let n = self.heap_sites.len() as u32;
                self.heap_ix.insert(at, n);
                n
            }
        };
        let mut out = self.expand(ret);
        let mut under: Vec<Tok> = Vec::new();
        let ptrs: Vec<Tok> = out
            .toks
            .range(Tok::Num(0x0200_0000)..REF0)
            .filter(|t| match **t {
                Tok::Num(c) => is_ram(c),
                Tok::Ram(_) | Tok::Heap(..) => true,
                _ => false,
            })
            .copied()
            .collect();
        for t in ptrs {
            match t {
                Tok::Num(c) => under.push(Tok::Ram(ram_obj(c).0)),
                // (Another site's, which may grow: read again then.)
                Tok::Heap(s, _) => {
                    self.rd.heap = true;
                    under.extend(self.heap_under(s));
                }
                t => under.push(t),
            }
            out.toks.remove(&t);
        }
        let site = &mut self.heap_sites[n as usize - 1].1;
        let was = site.len();
        site.extend(under);
        if site.len() != was {
            self.heap_changed = true;
            self.mark(true);
        }
        out.toks.insert(Tok::Heap(n, 0));
        out.cap();
        out
    }

    /// Where site `s`'s allocation is: the pointers into RAM its
    /// allocator returned (site 0: any site's).
    fn heap_under(&self, s: u32) -> BTreeSet<Tok> {
        match s {
            0 => self
                .heap_sites
                .iter()
                .flat_map(|h| h.1.iter().copied())
                .collect(),
            s => self.heap_sites[s as usize - 1].1.clone(),
        }
    }

    /// Takes the pointers below an allocation's start out of `v`
    /// ([`HEAP_UNDER`]: `free`'s `ptr - 12`), each for the pointers into
    /// RAM its allocator returned: a header is the allocator's own, read
    /// and written through the heap's RAM, not the allocation's cells.
    fn heap_lower(&mut self, v: &mut Val) {
        let low: Vec<Tok> = v
            .toks
            .range(Tok::Heap(0, 0)..=Tok::Heap(u32::MAX, u32::MAX))
            .filter(|t| matches!(t, Tok::Heap(_, HEAP_UNDER)))
            .copied()
            .collect();
        if low.is_empty() {
            return;
        }
        self.rd.heap = true;
        for t in low {
            v.toks.remove(&t);
            if let Tok::Heap(s, _) = t {
                let under = self.heap_under(s);
                v.toks.extend(under);
            }
        }
        v.cap();
    }

    /// What a load at width `w` through a pointer at `k` into site `s`'s
    /// allocation reads ([`Tok::Heap`]): the cell of that word and the
    /// site's any-cell, or, somewhere in it, every cell of the site's;
    /// what was stored somewhere in any allocation; and the pile (a
    /// pointer the run dropped may be into it). No cell at a constant
    /// address (no global is in an allocation) and none of the heap's
    /// RAM: what the allocator's own code stored in an allocation through
    /// its own pointers (zeroes) is a number, which the read's mark
    /// stands for. Site 0, any allocation: every site's and object's.
    fn read_heap(&mut self, s: u32, k: u32, w: u8, into: &mut Val) {
        into.mem = true;
        self.read_pile(w, into);
        if k == HEAP_UNDER {
            debug_assert!(false, "a header's pointer not lowered");
            self.read_cells(w, into);
            self.read_objs(w, into);
            return;
        }
        if s == 0 {
            self.read_objs(w, into);
            return;
        }
        if k == HEAP_ANY || (k & 3) + w as u32 > 4 {
            let (lo, hi) = (heap_cell(s, 0), heap_cell(s, HEAP_ANY));
            self.rd.range(lo, hi);
            let cs: Vec<u32> = self.cells.range(lo..=hi).map(|(&c, _)| c).collect();
            for c in cs {
                self.read_obj(c, w, into);
            }
        } else {
            self.read_obj(heap_cell(s, k), w, into);
            self.read_obj(heap_cell(s, HEAP_ANY), w, into);
        }
        self.read_obj(heap_cell(0, HEAP_ANY), w, into);
    }

    fn store(&mut self, to: Store, toks: &[(Tok, u8)]) {
        if !self.watch.is_empty() {
            let v = Val {
                toks: toks.iter().map(|t| t.0).collect(),
                ..Val::default()
            };
            self.watch(&v, || match to {
                Store::Pile => "stored to the pile".to_string(),
                Store::Cell(a) => format!("stored to the cell at {a:#010x}"),
                Store::Spill(f) => format!("stored to f_{f:08x}'s spills"),
            });
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
                    self.log_total += new.len();
                    self.mem_log
                        .entry(PILE)
                        .or_default()
                        .extend(new.iter().copied());
                    self.pile_changed = true;
                    Some(&mut self.pile_raw)
                }
                // An object's or a stack's cell: no RAM's.
                Store::Cell(a) if !is_ram(a) => {
                    self.cells_changed.insert(a);
                    for w in [1u8, 2, 4] {
                        if let Some(v) = self.obj_reads.remove(&(a, w)) {
                            self.obj_was.entry((a, w)).or_insert(v);
                        }
                        if let Some(v) = self.obj_raw_vals.get_mut(&(a, w)) {
                            for &(t, sw) in &new {
                                from_mem_one(t, sw, w, v);
                            }
                        }
                    }
                    self.log_total += new.len();
                    self.mem_log
                        .entry(a)
                        .or_default()
                        .extend(new.iter().copied());
                    for &(t, sw) in &new {
                        if !is_in_cell(a) && self.objs_all.insert((t, sw)) {
                            self.log_total += 1;
                            self.mem_log.entry(OBJS).or_default().push((t, sw));
                            self.objs_reads = Default::default();
                            for w in [1u8, 2, 4] {
                                from_mem_one(t, sw, w, &mut self.objs_raw[w as usize]);
                            }
                        }
                    }
                    None
                }
                Store::Cell(a) => {
                    self.cell_reads = Default::default();
                    for &(t, sw) in &new {
                        if self.cells_all.insert((t, sw)) {
                            self.log_total += 1;
                            self.mem_log.entry(CELLS).or_default().push((t, sw));
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
            // And, at a constant RAM address, to its object's cell of
            // those ([`in_cell`]).
            if let Store::Cell(a) = to
                && is_ram(a)
                && ram_on()
            {
                self.store(Store::Cell(in_cell(ram_obj(a).0)), &new);
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
        if self.off & (if cells { OFF_CELLS } else { OFF_PILE }) != 0 {
            return;
        }
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
        if let Some(e) = self.ref_exp.get(&r)
            && self.exp_current(e)
        {
            self.rd.exp(r, e, self.by_value_read);
            return e.v.clone();
        }
        let mut e = match self.ref_exp.remove(&r) {
            Some(e) => e,
            None => {
                let Tok::Ref { src, xf } = r else {
                    unreachable!("not a Ref: {r:?}")
                };
                self.exp_ver += 1;
                let mut e = Exp {
                    large_at: self.large.log.len(),
                    ver: self.exp_ver,
                    ..Exp::default()
                };
                self.exp_add(&mut e, vec![(src, xf)]);
                e.total = self.log_total;
                self.sets.share(&mut e.v.toks);
                // Of ROM alone: it never changes.
                if e.pile | e.cells == 0 && e.objs.is_empty() && !e.large {
                    self.ref_rom.insert(r, e.v.clone());
                    return e.v;
                }
                e
            }
        };
        if !self.exp_current(&e) {
            let was = (std::rc::Rc::as_ptr(&e.v.toks.0), e.v.toks.len());
            if self.exp_update(&mut e) {
                self.exp_ver += 1;
                e.ver = self.exp_ver;
            }
            if was != (std::rc::Rc::as_ptr(&e.v.toks.0), e.v.toks.len()) {
                self.sets.share(&mut e.v.toks);
            }
        }
        self.rd.exp(r, &e, self.by_value_read);
        let v = e.v.clone();
        self.ref_exp.insert(r, e);
        v
    }

    /// Whether `e` has read all its memory's log.
    fn exp_current(&self, e: &Exp) -> bool {
        e.large_at == self.large.log.len()
            && (e.total == self.log_total
                || e.at
                    .iter()
                    .all(|(t, &n)| self.mem_log.get(t).map_or(0, |l| l.len()) == n))
    }

    /// Reads `work` (and the reads it reaches through the `Ref`s memory
    /// holds, each with the forms the `Ref`s on the way give) into `e`,
    /// those it hasn't read yet, whole. Memory is read as stored, uncapped,
    /// so that what it gains later can be added alone ([`exp_merge`] caps).
    fn exp_add(&mut self, e: &mut Exp, mut work: Vec<(Src, Xf)>) -> bool {
        let mut d = Val::default();
        while let Some((src, xf)) = work.pop() {
            // More single objects' cells than a value keeps apart
            // ([`OBJ_REFS`]): the rest are every object's, as in a value
            // ([`Val::cap`]). The pile holds a `Ref` for each object's
            // cell stored to it, hundreds, and every kept read that
            // reaches the pile took each in by itself, form by form: on
            // Metroid 100,000 such reads in the first round, 11 million
            // tokens, and half of Emerald's first two rounds
            // (`native/fnref-ram-reruns`).
            let src = match src {
                Src::Obj(k, w)
                    if !e.objs.contains(&(k, w))
                        && e.objs.iter().filter(|o| o.0 != OBJS).count() >= OBJ_REFS =>
                {
                    if is_in_cell(k) {
                        Src::Cells(w)
                    } else {
                        Src::Objs(w)
                    }
                }
                s => s,
            };
            if !e.seen.insert((src, xf)) {
                continue;
            }
            if let Some(t) = src_tag(src) {
                let n = self.mem_log.get(&t).map_or(0, |l| l.len());
                e.at.entry(t).or_insert(n);
                e.mems.push((src, xf));
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
                Src::RomFrom(lo, end, w, k) => self.rom_from[&(lo, end, w, k)].clone(),
                Src::Hull(lo, hi, w) => self.rom_hull[&(lo, hi, w)].clone(),
                Src::Obj(k, w) => {
                    e.objs.insert((k, w));
                    self.obj_raw_read(k, w)
                }
                Src::Objs(w) => {
                    e.objs.insert((OBJS, w));
                    self.objs_raw[w as usize].clone()
                }
                Src::Large => {
                    e.large = true;
                    Val {
                        toks: self.large.set.clone(),
                        ..Val::default()
                    }
                }
            };
            exp_take(&mut d, &read, xf, &mut work);
        }
        exp_merge(e, d)
    }

    /// Brings `e` up to date: what was stored since it was made, read
    /// through each memory read it holds, and whatever new reads that
    /// reaches, whole. Whether that made its value another.
    fn exp_update(&mut self, e: &mut Exp) -> bool {
        let mut work = Vec::new();
        let mut d = Val::default();
        let mems = e.mems.clone();

        // What each read gained (the pile's or every cell's, at a width),
        // made once and taken through each of the forms it is seen by: a
        // value a token and form at a time was most of Emerald's third
        // round.
        let mut gained: BTreeMap<Src, Val> = BTreeMap::new();
        for &(src, xf) in &mems {
            let read = gained.entry(src).or_insert_with(|| {
                let mut read = Val::default();
                let tag = src_tag(src).expect("memory");
                let from = e.at.get(&tag).copied().unwrap_or(0);
                if let Some(l) = self.mem_log.get(&tag) {
                    for &(t, sw) in &l[from.min(l.len())..] {
                        from_mem_one(t, sw, src.w(), &mut read);
                    }
                }
                read
            });
            exp_take(&mut d, read, xf, &mut work);
        }
        for (t, n) in e.at.iter_mut() {
            *n = self.mem_log.get(t).map_or(0, |l| l.len());
        }
        e.total = self.log_total;
        // And the large set's, the same way.
        if e.large && e.large_at < self.large.log.len() {
            let read = Val {
                toks: self.large.log[e.large_at..].iter().copied().collect(),
                ..Val::default()
            };
            for &(_, xf) in e.seen.iter().filter(|(s, _)| *s == Src::Large) {
                exp_take(&mut d, &read, xf, &mut work);
            }
        }
        e.large_at = self.large.log.len();
        let mut changed = exp_merge(e, d);
        work.retain(|x| !e.seen.contains(x));
        if !work.is_empty() {
            changed |= self.exp_add(e, work);
        }
        changed
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
        let same = |a: &Val, b: &Val| {
            std::rc::Rc::ptr_eq(&a.toks.0, &b.toks.0)
                && (a.mem, a.unk, a.lost_rom) == (b.mem, b.unk, b.lost_rom)
        };
        let key = |ts: &[Toks; 2]| {
            let of = |i: usize| {
                let p = if ts[i].is_empty() {
                    std::ptr::null()
                } else {
                    std::rc::Rc::as_ptr(&ts[i].0)
                };
                (p, vs[i].mem, vs[i].unk, vs[i].lost_rom)
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
                ..Xf::of(|f| store_form(f, w))
            };
            out.toks.extend(ref_tok(src, xf.then(to)));
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

    /// [`Large::take`] of `v`, with the values of the ROM reads it holds
    /// made first.
    fn take_large(&mut self, v: &mut Val) {
        if v.toks.len() <= LARGE
            && !v.toks.range(REF0..).any(|t| {
                matches!(
                    t,
                    Tok::Ref {
                        src: Src::Large,
                        ..
                    }
                )
            })
        {
            return;
        }
        let roms: Vec<Tok> = v
            .refs()
            .filter(|t| {
                matches!(
                    t,
                    Tok::Ref {
                        src: Src::RomFrom(..) | Src::Hull(..),
                        ..
                    }
                ) && !self.ref_rom.contains_key(t)
            })
            .collect();
        for r in roms {
            self.ref_val(r);
        }
        self.large.take(v, &self.ref_rom);
    }

    fn give(&mut self, f: u32, regs: &[Val]) {
        let mut e = self
            .entry
            .remove(&f)
            .unwrap_or_else(|| vec![Val::default(); 16]);
        let mut c = false;
        let mut gained = Vec::new();
        let reads = self.reads.get(&f).copied().unwrap_or(0xf);
        for (i, (a, b)) in e.iter_mut().zip(regs).enumerate() {
            // (A register of r0-r3 it doesn't read: not its to take.)
            if i < 4 && reads & 1 << i == 0 {
                continue;
            }
            let old = a.clone();
            if a.join_capped(b) {
                self.take_large(a);
                if *a == old {
                    *a = old;
                    continue;
                }
                self.sets.share(&mut a.toks);
                gained.push((i, old, a.clone()));
                c = true;
            }
        }
        self.entry.insert(f, e);
        for (i, old, a) in gained {
            self.gain("entry", f, i, &old, &a);
        }
        if c {
            self.dirty_entry.insert(f);
        }
        self.mark(c);
        self.watch_regs(regs, |r| format!("passed in r{r} to f_{f:08x}"));
    }

    /// [`G::give`] the same registers to each of `ks` (a call by value's
    /// callees). Callees whose entry register held the same set before
    /// share the set it joins to: a join per callee made each its own copy
    /// of a large set, thousands of them on a large game (Minish Cap and
    /// Nightmare ran past 15 GB before this; `native/whole-lift-memory`).
    fn give_all(&mut self, ks: &BTreeSet<u32>, regs: &[Val]) {
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
            (usize, *const BTreeSet<Tok>, bool, bool, bool),
            (Toks, Val, bool),
            Fx,
        > = Default::default();
        let mut c_any = false;
        let mut gained = Vec::new();
        for &f in ks {
            let mut e = self
                .entry
                .remove(&f)
                .unwrap_or_else(|| vec![Val::default(); 16]);
            let mut c = false;
            let reads = self.reads.get(&f).copied().unwrap_or(0xf);
            for (i, (a, b)) in e.iter_mut().zip(regs).enumerate() {
                if i < 4 && reads & 1 << i == 0 {
                    continue;
                }
                if a.toks.is_empty() {
                    if a.join_capped(b) {
                        self.take_large(a);
                        self.sets.share(&mut a.toks);
                        c = true;
                    }
                    continue;
                }
                let key = (i, std::rc::Rc::as_ptr(&a.toks.0), a.mem, a.unk, a.lost_rom);
                match memo.get(&key) {
                    Some((_, v, grew)) => {
                        *a = v.clone();
                        c |= grew;
                    }
                    None => {
                        let old = a.toks.clone();
                        let was = a.clone();
                        let mut grew = a.join_capped(b);
                        if grew {
                            self.take_large(a);
                            grew = *a != was;
                            if !grew {
                                *a = was.clone();
                            }
                        }
                        if grew {
                            self.sets.share(&mut a.toks);
                            if self.gains.is_some() {
                                gained.push((f, i, was, a.clone()));
                            }
                        }
                        memo.insert(key, (old, a.clone(), grew));
                        c |= grew;
                    }
                }
            }
            self.entry.insert(f, e);
            if c {
                self.dirty_entry.insert(f);
            }
            c_any |= c;
        }
        for (f, i, old, new) in gained {
            self.gain("entry (set)", f, i, &old, &new);
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
                &mut self
                    .cell_at_reads
                    .values()
                    .chain(self.obj_reads.values())
                    .chain(self.obj_raw_vals.values())
                    .map(|v| &v.toks),
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
                    .chain(self.pile_reads.iter().flatten())
                    .chain(self.cell_reads.iter().flatten())
                    .chain(&self.loose_args)
                    .chain(self.loose_rets.iter().flatten())
                    .map(|v| &v.toks),
            ),
            ("large", 1, self.large.set.len() + self.large.log.len()),
            (
                "memory",
                0,
                self.pile.len()
                    + mems
                    + self.mem_log.values().map(|l| l.len()).sum::<usize>()
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

    fn give_ret(&mut self, f: u32, regs: &[Val]) {
        self.watch_regs(regs, |r| format!("returned in r{r} by f_{f:08x}"));
        let mut e = self
            .ret
            .remove(&f)
            .unwrap_or_else(|| vec![Val::default(); 4]);
        let mut c = false;
        let mut gained = Vec::new();
        for (i, (a, b)) in e.iter_mut().zip(regs).enumerate() {
            let old = a.clone();
            if a.join_capped(b) {
                self.take_large(a);
                if *a == old {
                    *a = old;
                    continue;
                }
                self.sets.share(&mut a.toks);
                gained.push((i, old, a.clone()));
                c = true;
            }
        }
        self.ret.insert(f, e);
        for (i, old, a) in gained {
            self.gain("result", f, i, &old, &a);
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

    /// `GBAA_FNREF_GAINS`: count what register `i` of `f`'s entry or
    /// result gained (`old` to `new`) by the kinds of tokens added and
    /// taken out and the flags set, and print a few of each kind.
    fn gain(&mut self, what: &str, f: u32, i: usize, old: &Val, new: &Val) {
        let Some(gains) = &mut self.gains else {
            return;
        };
        let kind = |t: &Tok| match t {
            Tok::Fn { .. } => "fn",
            Tok::Num(_) => "num",
            Tok::Rom(_) => "rom",
            Tok::RomFrom(_) => "from",
            Tok::RomStep(..) => "step",
            Tok::RomDull => "dull",
            Tok::RomAny => "any",
            Tok::Ram(_) => "ram",
            Tok::Heap(..) => "heap",
            Tok::Ref { src, .. } => match src {
                Src::Pile(_) => "ref-pile",
                Src::Cells(_) => "ref-cells",
                Src::Obj(..) | Src::Objs(_) => "ref-obj",
                Src::RomFrom(..) => "ref-romfrom",
                Src::Hull(..) => "ref-hull",
                Src::Large => "ref-large",
            },
        };
        let added: Vec<Tok> = new.toks.difference(&old.toks).copied().collect();
        let gone: Vec<Tok> = old.toks.difference(&new.toks).copied().collect();
        let kinds = |ts: &[Tok]| -> String {
            let k: BTreeSet<&str> = ts.iter().map(kind).collect();
            k.into_iter().collect::<Vec<_>>().join(",")
        };
        let mut key = format!("{what} r{i} +[{}] -[{}]", kinds(&added), kinds(&gone));
        for (on, was, name) in [
            (new.mem, old.mem, " mem"),
            (new.unk, old.unk, " unk"),
            (new.lost_rom, old.lost_rom, " lost"),
        ] {
            if on && !was {
                key.push_str(name);
            }
        }
        let n = gains.entry(key.clone()).or_default();
        n.0 += 1;
        if n.1 >= 4 {
            return;
        }
        n.1 += 1;
        let show = |ts: &[Tok]| -> String {
            ts.iter()
                .take(4)
                .map(|t| match *t {
                    Tok::Rom(a) => format!("rom {a:#x}"),
                    Tok::RomFrom(a) => format!("from {a:#x}"),
                    Tok::Num(n) => format!("num {n:#x}"),
                    t => format!("{t:?}"),
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        eprintln!(
            "gain f_{f:08x} {key}: {} added: {}; {} gone: {}; now {} tokens",
            added.len(),
            show(&added),
            gone.len(),
            show(&gone),
            new.toks.len()
        );
    }

    /// `v`, the sum instruction `i` of block `block` of function `f`
    /// makes, with its pointer somewhere in its literal's object made one
    /// somewhere in the object at the literal plus its constant
    /// ([`BASED`]): `anchor + i * 4 + 8` is in the global at `anchor +
    /// 8`, though the sum's first step, anchor plus index, was in the one
    /// at the anchor. (An array's own literal with `arr[i + 1]` is in the
    /// same object either way.) What this takes on trust: the index is
    /// not negative from the literal plus the constant (`gNext[i - 3]`
    /// from an anchor, with the `- 3` folded into the constant, would be
    /// the global three entries below).
    fn rebased(&self, f: u32, block: u32, i: usize, mut v: Val) -> Val {
        let Some(&(l, k)) = self.based.get(&(f, block, i | BASED)) else {
            return v;
        };
        let l = ram_canon(l);
        if k >= 0x8000_0000 || l.checked_add(k).is_none_or(|at| at >= ram_top(l)) {
            return v;
        }
        let (from, to) = (Tok::Ram(ram_obj(l).0), Tok::Ram(ram_obj(l + k).0));
        if from != to && v.toks.contains(&from) {
            v.toks.remove(&from);
            v.toks.insert(to);
        }
        v
    }

    /// After a call of `k` with the caller's registers `own`: each of
    /// `rets` that `k` leaves as it came ([`G::leaves`]) is the caller's
    /// own value, not what `k`'s returns hold (every caller's, joined).
    fn kept(&self, k: u32, rets: &mut [Val], own: &[Val]) {
        let Some(&m) = self.leaves.get(&k) else {
            return;
        };
        for (i, (r, o)) in rets.iter_mut().zip(own).enumerate().take(4) {
            if m & 1 << i != 0 {
                *r = o.clone();
            }
        }
    }

    /// The registers of r0-r3 any of `ks` leaves as it came.
    fn left_by<'a>(&self, ks: impl IntoIterator<Item = &'a u32>) -> u8 {
        if self.leaves.is_empty() {
            return 0;
        }
        ks.into_iter()
            .fold(0, |m, k| m | self.leaves.get(k).copied().unwrap_or(0))
    }

    /// Whether `k` may change r`i` (of r0-r3): its return's value of it
    /// is a result.
    fn sets(&self, k: u32, i: usize) -> bool {
        self.leaves.get(&k).is_none_or(|m| m & 1 << i == 0)
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

/// Per block of `f`, the insts whose value only feeds flags nothing reads
/// before they are written again or the function's are gone (old GCC's
/// `cmp r0, #0` before a return sets N of a function's address). Flags
/// as the native lift has them: a call passes none and leaves the
/// caller's as they were, a return gives none back, a fall into another
/// function passes them all.
fn unused(f: &FnIn) -> BTreeMap<u32, Vec<bool>> {
    use analysis::ir::flags::{Flags, dead_writes};
    let keys: Vec<u32> = f.blocks.keys().copied().collect();
    let index: BTreeMap<u32, usize> = keys.iter().enumerate().map(|(i, &k)| (k, i)).collect();
    let insts: Vec<&[Inst]> = f.blocks.values().map(|b| b.insts).collect();
    let dead = dead_writes(&insts, |b, i| {
        let to = |ks: &[u32]| match ks.iter().map(|k| index.get(k).copied()).collect() {
            Some(v) => (v, Flags::NONE),
            None => (Vec::new(), Flags::ALL),
        };
        let insts = f.blocks[&keys[b]].insts;
        // A jump by value may also go where a holes file's jumps send it,
        // into another function, which the edge doesn't list.
        let by_value = match insts[i].op {
            Op::Exit(Exit::Jump(t) | Exit::JumpExchange(t)) => {
                !matches!(insts[t.index()].op, Op::Const(_) | Op::Pool(_))
            }
            _ => false,
        };
        match f.blocks[&keys[b]].exits.get(&i).map(|o| &o.edge) {
            Some(Edge::Goto(ks)) if !by_value => to(ks),
            Some(Edge::Goto(ks)) => (to(ks).0, Flags::ALL),
            Some(Edge::Call { ret, .. }) => to(&[*ret]),
            Some(Edge::Swi(next)) => to(&[*next]),
            Some(Edge::Return) => (Vec::new(), Flags::NONE),
            _ => (Vec::new(), Flags::ALL),
        }
    });
    keys.iter()
        .zip(insts.iter().zip(dead))
        .map(|(&k, (insts, dead))| {
            // What DCE keeps: `root(i)` and what it takes.
            let used = |root: &dyn Fn(usize, &Inst) -> bool| {
                let mut used = vec![false; insts.len()];
                for (i, inst) in insts.iter().enumerate().rev() {
                    if !(used[i] || root(i, inst)) {
                        continue;
                    }
                    used[i] = true;
                    for v in inst.op.operands().into_iter().chain(inst.guard) {
                        used[v.index()] = true;
                    }
                }
                used
            };
            // An op nothing takes at all is run as before (its holes are
            // made): only what the dead flag writes, the yields and the
            // timing alone take, which the native lift drops, is skipped.
            let all = used(&|_, inst| inst.op.is_root());
            let live = used(&|i, inst| {
                !all[i]
                    || inst.op.is_root()
                        && !dead[i]
                        && !matches!(inst.op, Op::YieldFlag(..) | Op::Timing(_))
            });
            (k, live.into_iter().map(|l| !l).collect())
        })
        .collect()
}

/// `a + b + c` that is a move (`x + 0 + 0`): the operand it moves.
fn moves(vals: &[Val], a: ValueId, b: ValueId, c: ValueId) -> Option<ValueId> {
    let zero = |x: ValueId| is_num(&vals[x.index()], 0);
    if !zero(c) {
        None
    } else if zero(b) {
        Some(a)
    } else if zero(a) {
        Some(b)
    } else {
        None
    }
}

/// `a | b` that is `x | -x`, one operand `0 + !x + 1` (`negs`) of the
/// other: that `x`. Old GCC's `x != 0` as a bit, `negs r1, r0; orrs r1,
/// r0; lsrs r5, r1, #31` (`native/fnref-gcc-ne-bit`).
fn ne_or(insts: &[Inst], vals: &[Val], op: BinOp, a: ValueId, b: ValueId) -> Option<ValueId> {
    if op != BinOp::Or {
        return None;
    }
    let neg_of = |n: ValueId, x: ValueId| match insts[n.index()].op {
        Op::Adc { a: p, b: q, c } if is_num(&vals[c.index()], 1) => {
            [(p, q), (q, p)].into_iter().any(|(z, m)| {
                is_num(&vals[z.index()], 0) && matches!(insts[m.index()].op, Op::Not(y) if y == x)
            })
        }
        _ => false,
    };
    if neg_of(a, b) {
        Some(b)
    } else if neg_of(b, a) {
        Some(a)
    } else {
        None
    }
}

/// An `a + b + c` whose carry and overflow are known whatever the other
/// operand holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Known {
    /// `x + 0 + 0`: neither.
    Move,
    /// `x + !0 + 1`, `cmp x, #0`: it carries and never overflows (the sum
    /// is `x + 2^32`, and as signed `x - 1 + 1`).
    CmpZero,
}

fn flags_of(vals: &[Val], a: ValueId, b: ValueId, c: ValueId) -> Option<Known> {
    let num = |x: ValueId, n: u32| is_num(&vals[x.index()], n);
    if moves(vals, a, b, c).is_some() {
        Some(Known::Move)
    } else if num(c, 1) && (num(a, !0) || num(b, !0)) {
        Some(Known::CmpZero)
    } else {
        None
    }
}

/// `v` is the number `n` and nothing else.
fn is_num(v: &Val, n: u32) -> bool {
    !v.unk && !v.mem && !v.lost_rom && v.toks.len() == 1 && v.toks.contains(&Tok::Num(n))
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
            Tok::Rom(_) | Tok::RomFrom(_) | Tok::RomStep(..) | Tok::RomDull | Tok::RomAny
                if w == 4 =>
            {
                out.push((t, 4))
            }
            // A pointer into RAM as it is (the closed world: memory keeps
            // every one, so a number read from it is none); its pieces, as
            // a ROM pointer's, a number.
            Tok::Num(c) if is_ram(c) && w == 4 && ram_on() => out.push((Tok::Num(ram_canon(c)), 4)),
            Tok::Ram(_) | Tok::Heap(..) if w == 4 => out.push((t, 4)),
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
        (Src::Large, _) => &FORMS,
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
        Tok::Rom(_) | Tok::RomFrom(_) | Tok::RomStep(..) | Tok::RomDull | Tok::RomAny if w == 4 => {
            t
        }
        // A pointer into RAM read as stored (`native/fnref-ram-pointers`);
        // its bytes, as a ROM pointer's, a number read from memory.
        Tok::Num(c) if is_ram(c) && w == 4 && sw == 4 && ram_on() => t,
        Tok::Ram(_) | Tok::Heap(..) if w == 4 && sw == 4 => t,
        // Read at the width it was stored at, it is what was stored; a
        // word from pieces, reassembled; narrower, its bytes.
        Tok::Ref { src, xf } => {
            let xf = if sw == w {
                xf
            } else if w == 4 {
                xf.then(Xf::all(Form::Full, true))
            } else {
                xf.then(Xf::all(Form::Part, true))
            };
            match ref_tok(src, xf) {
                Some(t) => t,
                None => return,
            }
        }
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
    /// Every number and ROM pointer read into it, as read ([`exp_merge`]).
    raw: BTreeSet<Tok>,
    seen: BTreeSet<(Src, Xf)>,
    pile: u8,
    cells: u8,
    /// The object cells it reads, each at a width ([`Src::Obj`];
    /// [`OBJS`]: every one).
    objs: BTreeSet<(u32, u8)>,
    /// How much of each memory's log ([`G::mem_log`]) it has read, and
    /// of all of it ([`G::log_total`]) when it last read to the end.
    at: BTreeMap<u32, usize>,
    total: usize,
    /// Its memory reads (of [`Exp::seen`]), for [`G::exp_update`].
    mems: Vec<(Src, Xf)>,
    /// It reads the large set ([`Large::take`]), and how much of
    /// [`Large::log`] it has read.
    large: bool,
    large_at: usize,
    /// Which value `v` is, of all the kept reads' ([`G::exp_ver`]):
    /// another each time it changes.
    ver: u64,
}

/// [`G::mem_log`]'s tag of a memory read.
fn src_tag(src: Src) -> Option<u32> {
    match src {
        Src::Pile(_) => Some(PILE),
        Src::Cells(_) => Some(CELLS),
        Src::Obj(k, _) => Some(k),
        Src::Objs(_) => Some(OBJS),
        Src::RomFrom(..) | Src::Hull(..) | Src::Large => None,
    }
}

/// Takes `d`, read uncapped, into `e`, and says whether `e.v` is another
/// value for it. Its numbers and ROM pointers are
/// kept as read (`e.raw`), and `e.v` holds the cap of them all: capping
/// what each store adds onto the capped value would widen by the order of
/// the stores, and leave a pointer out that the capped read of memory
/// takes in, which is what [`follow`] reruns a function by (`snap`).
fn exp_merge(e: &mut Exp, mut d: Val) -> bool {
    let nums = Tok::Num(0)..REF0;
    let was = (e.v.toks.len(), e.v.mem, e.v.unk, e.v.lost_rom);
    d.drop_range(REF0..);
    let new: Vec<Tok> = d
        .toks
        .range(nums.clone())
        .filter(|t| !e.raw.contains(t))
        .copied()
        .collect();
    let unk = d.unk && !e.v.unk;
    // Functions' tokens and the flags: the cap leaves them as they are.
    d.drop_range(nums.clone());
    e.v.union(&d);
    if new.is_empty() && !unk {
        return was != (e.v.toks.len(), e.v.mem, e.v.unk, e.v.lost_rom);
    }
    e.raw.extend(new);
    let mut p = Val {
        toks: e.raw.iter().copied().collect(),
        unk: e.v.unk,
        ..Val::default()
    };
    p.cap();
    e.v.unk |= p.unk;
    // (The cap gives up pointers into too many runs: [`FROMS`].)
    e.v.lost_rom |= p.lost_rom;
    let mut changed = false;
    if !p.toks.iter().eq(e.v.toks.range(nums.clone())) {
        e.v.drop_range(nums);
        e.v.toks.add_all(p.toks.iter().copied().collect());
        changed = true;
    }
    changed || was != (e.v.toks.len(), e.v.mem, e.v.unk, e.v.lost_rom)
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
            // A ROM or RAM pointer's bytes: a number read from memory.
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
    /// Per block, the insts whose value only a flag nothing reads takes
    /// ([`unused`]): they make no holes.
    unused: BTreeMap<u32, Vec<bool>>,
    /// The instruction's op read where an allocation is
    /// ([`G::heap_lower`]): its result isn't kept by its operands
    /// ([`G::op_kept`]), which don't say where that was.
    lowered: bool,
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
            if self.g.watch.iter().any(|x| x.1 == o) {
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
    /// it and the 16 from it on), or the next table start ([`tables`])
    /// past `a`, wherever it is; or `hi`. Neither ends it where it is a
    /// field of a table below: less than one entry past a table's start.
    fn object_end(&self, a: u32, hi: u32) -> u32 {
        let img = self.img;
        let fns_in = |lo: u32, hi: u32| {
            (lo & !3..hi)
                .step_by(4)
                .any(|p| img.word(p).is_some_and(|x| img.is_fn(x)))
        };
        let tables = &self.g.tables;
        // Records longer than this aren't looked for.
        const RECORD: u32 = 0x1000;
        let field = |c: u32| {
            let own = tables.get(&c).copied().unwrap_or(0);
            tables
                .range(c.saturating_sub(RECORD)..c)
                .any(|(x, s)| c - x < (*s).max(own))
        };
        let mut run = self
            .g
            .objects
            .range(a.saturating_add(1)..hi)
            .copied()
            .peekable();
        let mut starts = tables
            .range(a.saturating_add(1)..hi)
            .map(|(c, _)| *c)
            .peekable();
        // Both, in order, once each.
        let cuts = std::iter::from_fn(|| match (run.peek().copied(), starts.peek().copied()) {
            (Some(o), Some(t)) if o == t => {
                run.next();
                starts.next()
            }
            (Some(o), Some(t)) if t < o => starts.next(),
            (Some(_), _) => run.next(),
            (None, _) => starts.next(),
        });
        cuts.into_iter()
            .find(|&c| {
                self.g.off & OFF_TABLES != 0
                    || !field(c)
                        && (tables.contains_key(&c)
                            || !(fns_in(c.saturating_sub(16), c)
                                && fns_in(c, c.saturating_add(16))))
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
        let v = self.rom_from_val(a, 1, w, hull, len);
        into.join(&v);
    }

    /// What [`Self::rom_from`] reads; with a `step` over 1 (a
    /// [`Tok::RomStep`]), only the field at `a` of each record: `w`
    /// bytes at `a`, `a + step`, and so on.
    fn rom_from_val(
        &mut self,
        a: u32,
        step: u16,
        w: u8,
        hull: Option<(u32, u32)>,
        len: Option<u32>,
    ) -> Val {
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
            // A step keeps its records' field: the first at or past `l`.
            let k = step as u32;
            lo = if l > lo && k > 1 {
                lo.saturating_add((l - lo).div_ceil(k) * k)
            } else {
                l.max(lo)
            };
            hi = hi.min(h.saturating_add(1));
        }
        let end = hi.min(lo.saturating_add(ROM_READ));
        if let Some(v) = self.g.rom_from.get(&(lo, end, w, step)) {
            return rom_ref(v, Src::RomFrom(lo, end, w, step));
        }
        let mut v = Val {
            // Cut short: the words past the cut go unread.
            lost_rom: end < hi,
            ..Val::default()
        };
        if step > 1 {
            let mut p = lo;
            while p < end {
                self.rom_read_capped(p, w.min(4), &mut v);
                p = p.saturating_add(step as u32);
            }
        } else {
            let mut p = lo & !3;
            while p < end {
                self.rom_read_capped(p, w.min(4), &mut v);
                p += 4;
            }
        }
        let r = rom_ref(&v, Src::RomFrom(lo, end, w, step));
        self.g.rom_from.insert((lo, end, w, step), v);
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
        // What each large Ref leaves, gathered and added once: a set built
        // again at each of a value's Refs was a third of Emerald's rounds
        // with the RAM pointers on (`native/fnref-ram-cost`).
        let mut rests: Vec<Toks> = Vec::new();
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
            if !rest.is_empty() && !rests.iter().any(|t| std::rc::Rc::ptr_eq(&t.0, &rest.0)) {
                rests.push(rest);
            }
            let Tok::Ref { src, xf: rx } = r else {
                continue;
            };
            refs.extend(ref_tok(src, rx.then(xf)));
        }
        match rests.len() {
            0 => {}
            1 if conc.toks.is_empty() => conc.toks = rests.pop().expect("one"),
            _ => conc
                .toks
                .add_all(rests.iter().flat_map(|t| t.iter().copied()).collect()),
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
            let Some(c) = ref_tok(src, xf.then(Xf::all(Form::Cmp, false))) else {
                continue;
            };
            let e = self.g.ref_val(c);
            if !e.toks.is_empty() {
                let rest: Toks = out.toks.difference(&e.toks).copied().collect();
                out.toks = rest;
            }
            out.toks.insert(c);
        }
    }

    /// `steps`: what each operand's unknowns are a whole number of
    /// ([`strides`]; 1, nothing known).
    fn bin(&mut self, op: BinOp, a: &Val, b: &Val, steps: [u32; 2], site: Origin, at: u32) -> Val {
        let mut out = Val::default();
        // Every token with every token, but for large sets: each with an
        // unknown partner (which only adds holes and unknowns).
        let (xs, ys) = (a.parts(), b.parts());
        // (Not token by token with a single token either: on
        // native-tasks the run then never settles, a set growing by its
        // step each round.)
        // But an `&` with one exact constant (a mask, whose results are
        // bounded by it) is token by token, the mask kept.
        let mask = |v: &Val| {
            !v.mem
                && !v.unk
                && v.toks.len() == 1
                && matches!(v.toks.iter().next(), Some(Tok::Num(_)))
        };
        let pairs: Vec<(Option<Tok>, Option<Tok>)> =
            if xs.len() * ys.len() > 64 && !(op == BinOp::And && (mask(a) || mask(b))) {
                xs.iter()
                    .map(|&x| (x, None))
                    .chain(ys.iter().map(|&y| (None, y)))
                    .collect()
            } else {
                xs.iter()
                    .flat_map(|&x| ys.iter().map(move |&y| (x, y)))
                    .collect()
            };
        // Whether a side's amount varies (more than one value: a loop's
        // index), or its RAM addresses are a walk through one object
        // (several, all in it). One pointer of several plus one constant
        // is a field's address, and stays exact.
        let walk = |ps: &[Option<Tok>]| {
            let mut os = ps.iter().filter_map(|t| match t {
                Some(Tok::Num(c)) if is_ram(*c) => Some(ram_obj(*c).0),
                _ => None,
            });
            let first = os.next();
            let mut n = 1;
            first.is_some_and(|o| {
                os.all(|x| {
                    n += 1;
                    x == o
                }) && n > 1
            })
        };
        let ram = ram_on();
        let varies = [
            ram && (ys.len() > 1 || walk(&xs)),
            ram && (xs.len() > 1 || walk(&ys)),
        ];
        // Gathered, and added at the end (a large set's one at a time was
        // most of a large game's run), as the holes of functions' tokens.
        let mut toks = Vec::new();
        let mut holes = Vec::new();
        for (x, y) in pairs {
            {
                match bin_tok(self.img, op, x, y, steps) {
                    // A RAM address moved out of its object by an amount
                    // that varies (a loop's index or walked pointer, not
                    // one field's offset): somewhere in its object, as an
                    // index stays in its array.
                    R::T(Tok::Num(c)) => match left_object(self.img, op, x, y) {
                        Some((t, on_x)) if varies[usize::from(!on_x)] => toks.push(t),
                        _ => {
                            let t = self.classify(c, site);
                            toks.push(t);
                        }
                    },
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
                                (
                                    Some(
                                        Tok::Rom(_)
                                            | Tok::RomFrom(_)
                                            | Tok::RomStep(..)
                                            | Tok::RomDull
                                    ),
                                    None
                                ) | (
                                    Some(Tok::RomFrom(_) | Tok::RomStep(..) | Tok::RomDull),
                                    Some(Tok::Num(_))
                                )
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
        let under = toks.iter().any(|t| matches!(t, Tok::Heap(_, HEAP_UNDER)));
        out.toks.add_all(toks);
        if under {
            self.g.heap_lower(&mut out);
            self.lowered = true;
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
        // Unless the run dropped one: a ROM read it cut short or couldn't
        // make, or a ROM pointer it lost.
        let complete = !callees.is_empty() && !t.lost_rom;
        let lost = if complete {
            None
        } else if callees.is_empty() {
            Some(Lost::NoCallee)
        } else if callees.len() >= LARGE {
            Some(Lost::Large)
        } else if t.toks.contains(&Tok::RomAny) {
            Some(Lost::RomAny)
        } else {
            Some(Lost::RomRead)
        };
        let site = Site {
            callees: callees.clone(),
            complete,
            tail,
            lost,
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
        self.g.now = (self.f.entry, at);
        let mut vals: Vec<Val> = vec![Val::default(); b.insts.len()];
        let strides = strides(b.insts);
        let b_insts = b.insts;
        let none = Val::default();
        let unused = self.unused[&key].clone();
        for (i, inst) in b.insts.iter().enumerate() {
            if unused[i] {
                continue;
            }
            let site = Origin::Code(key, i as u32);
            self.lowered = false;
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
                // (Nor `x | -x`: a number, whatever `x` is.)
                Op::Bin(op, a, b)
                    if (large(a) || large(b)) && ne_or(b_insts, &vals, op, a, b).is_none() =>
                {
                    Some((6 + op as u8, None, a, Some(b)))
                }
                // (A move isn't kept: it is its operand.)
                Op::Adc { a, b, c }
                    if (large(a) || large(b)) && moves(&vals, a, b, c).is_none() =>
                {
                    Some((12, None, a, Some(b)))
                }
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
                    // (Kept by its operands, as made: its object here is
                    // this instruction's, [`BASED`].)
                    vals[i] = self.g.rebased(self.f.entry, key, i, out);
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
                // Another mode's register: maybe a pointer into RAM.
                Op::GetUserReg(_) if ram_on() => {
                    let mut v = Val::unk();
                    v.toks.insert(RAM_ANY);
                    v
                }
                Op::Bin(op, ra, rb) if ne_or(b_insts, &vals, op, ra, rb).is_some() => {
                    // `x | -x` (old GCC's `x != 0` as bit 31: `negs r1, r0;
                    // orrs r1, r0; lsrs r5, r1, #31`) is 0 or has bit 31
                    // set, so it is never a function's address, whatever
                    // `x` holds: a number, exact where `x` is.
                    let x = ne_or(b_insts, &vals, op, ra, rb).expect("matched");
                    let x = xv(self.g, x);
                    let mut out = Val {
                        unk: x.unk || x.mem || x.lost_rom,
                        ..Val::default()
                    };
                    let mut toks = Vec::new();
                    for &t in &x.toks {
                        match t {
                            Tok::Num(n) | Tok::Rom(n) => toks.push(Tok::Num(n | n.wrapping_neg())),
                            _ => out.unk = true,
                        }
                    }
                    out.toks.add_all(toks);
                    out.cap();
                    out
                }
                Op::Bin(op, ra, rb) => {
                    let (a, b) = (xv(self.g, ra), xv(self.g, rb));
                    // (Not for a kept op: its key has no steps.)
                    let st = match kept {
                        None => [strides[ra.index()], strides[rb.index()]],
                        Some(_) => [1, 1],
                    };
                    let mut out = self.bin(op, &a, &b, st, site, at);
                    if op == BinOp::Sub {
                        self.cmp_refs(&mut out, &[&vals[ra.index()], &vals[rb.index()]]);
                    }
                    out
                }
                Op::Adc { a: ra, b: rb, c } => {
                    if let Some(x) = moves(&vals, ra, rb, c) {
                        // `x + 0 + 0` is `x` (old GCC's `adds rd, rs, #0`,
                        // a move between low registers).
                        val(x)
                    } else {
                        let (a, b) = (xv(self.g, ra), xv(self.g, rb));
                        if a.fns().next().is_some() || b.fns().next().is_some() {
                            // A compare `a + !b + 1` (or an add, a hole when
                            // used): of whole addresses, with each other, 0 or
                            // an unknown.
                            let mut out = Val::unk();
                            let other_num = |v: &Val, zero: u32| {
                                v.toks.iter().any(|t| {
                                    // (A RAM address, or its `!`, is no
                                    // function's nor a dense ID.)
                                    matches!(t, Tok::Num(n) if *n != zero
                                        && !(ram_on() && (is_ram(*n) || is_ram(!*n))))
                                        || matches!(
                                            t,
                                            Tok::Rom(_)
                                                | Tok::RomFrom(_)
                                                | Tok::RomStep(..)
                                                | Tok::RomDull
                                                | Tok::RomAny
                                        )
                                })
                            };
                            // An address is compared with 0 or an address, or
                            // added to an unknown: the other operand's numbers
                            // say (`a + !b + 1`: a's 0 is b's `!0`). A value that
                            // may be a function or a number is still compared
                            // with 0.
                            let bad = [other_num(&b, 0xffff_ffff), other_num(&a, 0)];
                            let mut cmps = Vec::new();
                            for (v, bad) in [&a, &b].into_iter().zip(bad) {
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
                            kept = None;
                            let st = [strides[ra.index()], strides[rb.index()]];
                            let c = xv(self.g, c);
                            // A pointer into an allocation less a constant
                            // (`subs r0, #8` is `r0 + !8 + 1`): the
                            // constant first, so that `p + 8 - 8` is the
                            // allocation's start, not one byte below it on
                            // the way (its header, [`HEAP_UNDER`]).
                            let heap = |v: &Val| {
                                v.toks
                                    .range(Tok::Heap(0, 0)..=Tok::Heap(u32::MAX, u32::MAX))
                                    .next()
                                    .is_some()
                            };
                            let nums = |v: &Val| {
                                !v.unk
                                    && !v.mem
                                    && !v.lost_rom
                                    && !v.toks.is_empty()
                                    && v.toks.iter().all(|t| matches!(t, Tok::Num(_)))
                            };
                            if heap(&a) && nums(&b) && nums(&c) {
                                let bc = self.bin(BinOp::Add, &b, &c, [1, 1], site, at);
                                self.bin(BinOp::Add, &a, &bc, st, site, at)
                            } else if heap(&b) && nums(&a) && nums(&c) {
                                let ac = self.bin(BinOp::Add, &a, &c, [1, 1], site, at);
                                self.bin(BinOp::Add, &ac, &b, st, site, at)
                            } else {
                                let ab = self.bin(BinOp::Add, &a, &b, st, site, at);
                                self.bin(BinOp::Add, &ab, &c, [1, 1], site, at)
                            }
                        }
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
                            Tok::RomFrom(_)
                            | Tok::RomStep(..)
                            | Tok::RomDull
                            | Tok::RomAny
                            | Tok::Ram(_)
                            | Tok::Heap(..) => out.unk = true,
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
                            | Tok::RomStep(..)
                            | Tok::RomDull
                            | Tok::RomAny
                            | Tok::Ref { .. }
                                if keep =>
                            {
                                toks.push(t)
                            }
                            // A RAM pointer too (`native/fnref-ram-pointers`).
                            Tok::Num(c) if keep && is_ram(c) && ram_on() => toks.push(t),
                            Tok::Num(c) | Tok::Rom(c) => match n {
                                Some(n) => toks.push(num_tok(self.img, shift(kind, c, n))),
                                None => out.unk = true,
                            },
                            Tok::Ram(_) | Tok::Heap(..) if keep => toks.push(t),
                            // Shifted, it's no pointer (the closed world).
                            Tok::RomFrom(_)
                            | Tok::RomStep(..)
                            | Tok::RomDull
                            | Tok::RomAny
                            | Tok::Ram(_)
                            | Tok::Heap(..) => out.unk = true,
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
                // A move neither carries nor overflows; a compare with 0
                // (`x + !0 + 1`) always carries and never overflows.
                Op::AdcCarry { a, b, c } | Op::AdcOverflow { a, b, c }
                    if let Some(k) = flags_of(&vals, a, b, c) =>
                {
                    let carry = matches!(inst.op, Op::AdcCarry { .. });
                    Val::of(Tok::Num(u32::from(carry && k == Known::CmpZero)))
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
                    let (a, mut x) = (xv(self.g, addr), val(value));
                    self.g.take_large(&mut x);
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
                && !self.lowered
                && !v
                    .toks
                    .iter()
                    .any(|t| matches!(t, Tok::Fn { origin, .. } if *origin == site))
            {
                self.g.op_keep(k, &v);
            }
            // A literal, a constant and an index: its object ([`BASED`];
            // after it is kept by its operands, as made: another
            // instruction's sum of the same operands has its own).
            if !self.g.based.is_empty() && matches!(inst.op, Op::Bin(..) | Op::Adc { .. }) {
                v = self.g.rebased(self.f.entry, key, i, v);
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
        self.g.take_large(&mut v);
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
                    // argument), which its stores reached through the
                    // stacks' cell.
                    if esc || *o >= 0 {
                        self.read_stack_or_pile(w, &mut out);
                        out.mem = true;
                    }
                    return out;
                }
                Some(super::frame::Place::Through(_)) => {
                    self.g.read_spill(self.f.entry, w, &mut out);
                    self.read_stack_or_pile(w, &mut out);
                    return out;
                }
                _ => {}
            }
        }
        // An unknown or a number read from memory is no RAM address (the
        // closed world: memory keeps every RAM pointer, and an op the run
        // doesn't follow on one drops it, [`RAM_ANY`]): an I/O read, or
        // one the value-set analysis bounds.
        let mut anywhere = a.unk || a.mem;
        // A RAM pointer dropped: every cell and the pile.
        let mut lost = false;
        let ram = ram_on();
        // `out` as the cap leaves it (each ROM read caps it; a read
        // through thousands of function addresses reads their code).
        let mut capped = false;
        // Into more objects than [`OBJS_READ`], or somewhere in some
        // object ([`ANYOBJ`]): every cell, every object's cell and the
        // pile, once (each object's own was a `Ref` of its own in the
        // value).
        let many = ram
            && a.toks
                .range(Tok::Ram(3)..=Tok::Ram(u32::MAX))
                .nth(OBJS_READ)
                .is_some()
            || a.toks.contains(&ANYOBJ);
        if many {
            self.g.read_cells(w, &mut out);
            self.g.read_objs(w, &mut out);
            self.g.read_pile(w, &mut out);
        }
        for &t in &a.toks {
            match t {
                Tok::Rom(r) => {
                    self.rom_read_or_capped(r, w, &mut out, capped);
                    capped = true;
                }
                Tok::Ram(o) if o > 2 && many => {}
                Tok::Ram(0) => lost = true,
                Tok::Ram(1) => {
                    capped = false;
                    self.g.read_stack(w, &mut out);
                }
                Tok::Ram(2) => {
                    capped = false;
                    self.g.read_loose(w, &mut out);
                }
                // Somewhere in an object: what was stored in it, its
                // cells, and the pile.
                Tok::Ram(o) => {
                    capped = false;
                    self.g.read_in_obj(o, w, &mut out);
                }
                // In an allocation: its site's cells.
                Tok::Heap(s, k) => {
                    capped = false;
                    self.g.read_heap(s, k, w, &mut out);
                }
                Tok::RomFrom(_) | Tok::RomStep(..) => {
                    let (r, k) = match t {
                        Tok::RomStep(r, k) => (r, k),
                        Tok::RomFrom(r) => (r, 1),
                        _ => unreachable!(),
                    };
                    // As the cap leaves it, and taking in all the read
                    // holds, `out` is the join already (a load through
                    // hundreds of pointers "from here on" capped it at
                    // each: a fifth of Emerald's second round).
                    let v = self.rom_from_val(r, k, w, hull, None);
                    if capped {
                        out.join_capped(&v);
                    } else {
                        out.join(&v);
                    }
                    capped = true;
                }
                Tok::Num(c) | Tok::Fn { key: c, .. } => {
                    if is_ram(c) {
                        capped = false;
                        let c = ram_canon(c);
                        self.g.rd.range(c.saturating_sub(3), c + w as u32 - 1);
                        // The cells the access overlaps, what was stored
                        // somewhere in its object.
                        self.g.read_cell(c, w, &mut out);
                        if ram {
                            self.g.read_obj(obj_cell(ram_obj(c).0), w, &mut out);
                            self.g.read_obj(ANYOBJ_CELL, w, &mut out);
                        }
                        self.g.read_pile(w, &mut out);
                    } else if self.img.word(c).is_some() {
                        self.rom_read_or_capped(c, w, &mut out, capped);
                        capped = true;
                    } else {
                        capped = false;
                        out.unk = true;
                    }
                }
                // An instruction's bytes: a number.
                Tok::RomDull => {
                    capped = false;
                    out.unk = true;
                }
                // Anywhere in ROM: nothing the run reads.
                Tok::RomAny => {
                    capped = false;
                    out.unk = true;
                    out.lost_rom = true;
                }
                Tok::Ref { .. } => {
                    debug_assert!(false, "a Ref not expanded");
                    if ram {
                        lost = true;
                    } else {
                        anywhere = true;
                    }
                }
            }
        }
        // Nothing yet (a pointer loaded from a global no store has
        // reached so far): nothing, until something does (the closed
        // world: no RAM address hides in it). Else an early round's empty
        // read made the call through it incomplete, and its arguments went
        // to every loose function for good.
        if ram && a.toks.is_empty() && !a.unk && !a.mem && !a.lost_rom {
            return out;
        }
        if a.toks.is_empty() {
            anywhere = true;
        }
        // Bounded to ROM by the value sets, but placed in RAM: a pointer
        // the closed world missed (one copied where the run doesn't see),
        // so ROM's words there too.
        if ram
            && !anywhere
            && !lost
            && let Some((lo, hi)) = hull
            && self.img.word(lo).is_some()
            && self.img.word(hi).is_some()
            && !a
                .toks
                .iter()
                .any(|t| matches!(t, Tok::Rom(_) | Tok::RomFrom(_) | Tok::RomStep(..)))
        {
            anywhere = true;
        }
        if lost {
            self.g.read_pile(w, &mut out);
            self.g.read_cells(w, &mut out);
            self.g.read_objs(w, &mut out);
            out.unk = true;
        }
        if anywhere {
            // Bounds that start at a table the code indexes from its
            // start (`FBlock::bases`): only that table, to where its object
            // ends. (`GBAA_FNREF_OFF`'s `tables`: any bounds, and they end
            // at every object start.)
            let based = self
                .f
                .blocks
                .get(&key)
                .is_some_and(|b| b.bases.contains_key(&i));
            let hull = match hull {
                Some((lo, hi))
                    if (based || self.g.off & OFF_TABLES != 0)
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
                    // Bounded to RAM: no pointer the run follows, so a
                    // pointer dropped somewhere (the closed world broken,
                    // as by a copy the run doesn't see): those cells, the
                    // objects they are in, and the pile.
                    if !ram {
                        // Off (`native/fnref-ram-switch`): any address,
                        // the pile and the cells in the bounds.
                        self.g.read_pile(w, &mut out);
                        match hull {
                            Some((lo, hi)) => {
                                self.g.rd.range(lo.saturating_sub(3), hi);
                                for (_, c) in self.g.cells.range(lo.saturating_sub(3)..=hi) {
                                    from_mem(c, w, &mut out);
                                }
                            }
                            None => self.g.read_cells(w, &mut out),
                        }
                    } else if let Some((lo, hi)) = hull
                        && (is_ram(lo) || is_ram(hi))
                    {
                        let (lo, hi) = (ram_canon(lo), ram_canon(hi).max(ram_canon(lo)));
                        self.g.read_pile(w, &mut out);
                        self.g.rd.range(lo.saturating_sub(3), hi);
                        for (_, c) in self.g.cells.range(lo.saturating_sub(3)..=hi) {
                            from_mem(c, w, &mut out);
                        }
                        let (o, _) = ram_obj(lo);
                        let os: Vec<u32> =
                            RAM_OBJS.with(|c| c.borrow().range(o..=hi).map(|(&o, _)| o).collect());
                        for o in std::iter::once(o).chain(os) {
                            self.g.read_obj(obj_cell(o), w, &mut out);
                        }
                        self.g.read_obj(ANYOBJ_CELL, w, &mut out);
                    }
                    // An I/O read, too.
                    out.unk = true;
                }
            }
        }
        out
    }

    /// A read of the caller's frame or an escaping slot: the stacks' cell
    /// ([`G::read_stack`]), or, off ([`ram_on`]), the pile.
    fn read_stack_or_pile(&mut self, w: u8, into: &mut Val) {
        if ram_on() {
            self.g.read_stack(w, into);
        } else {
            self.g.read_pile(w, into);
        }
    }

    /// Where a store to the caller's frame or an escaping slot goes: the
    /// stacks' cell, or, off ([`ram_on`]), the pile.
    fn stack_or_pile() -> Store {
        if ram_on() {
            Store::Cell(STACK_CELL)
        } else {
            Store::Pile
        }
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
                        self.g.store(Self::stack_or_pile(), toks);
                    }
                    return;
                }
                Some(super::frame::Place::Through(_)) => {
                    self.g.store(Store::Spill(self.f.entry), toks);
                    self.g.store(Self::stack_or_pile(), toks);
                    return;
                }
                _ => {}
            }
        }
        if toks.is_empty() {
            return;
        }
        // An unknown address, or a number read from memory, is none in
        // RAM (the closed world, as for loads): what it stores reaches no
        // load there, but functions' and ROM addresses go to the pile all
        // the same (an I/O register's or VRAM's: not followed).
        // (Not where the address is a pointer into an allocation that may
        // also be a number, as one loaded from a global that was 0 is: the
        // allocation is where the store goes, and its site's cells are
        // all the pile would have been split into.)
        let ram = ram_on();
        let heap = a
            .toks
            .range(Tok::Heap(0, 0)..=Tok::Heap(u32::MAX, u32::MAX))
            .next()
            .is_some();
        if ram && !heap && (a.unk || a.mem || a.toks.is_empty()) {
            let kept: Vec<(Tok, u8)> = toks
                .iter()
                .copied()
                .filter(|(t, _)| {
                    matches!(
                        t,
                        Tok::Fn { .. }
                            | Tok::Rom(_)
                            | Tok::RomFrom(_)
                            | Tok::RomStep(..)
                            | Tok::Ref { .. }
                    )
                })
                .collect();
            if !kept.is_empty() {
                self.g.store(Store::Pile, &kept);
            }
        }
        // Off ([`ram_on`]): an unknown address may be any, its store goes
        // to the pile, and so does one through a ROM pointer from here on.
        let mut anywhere = !ram && (a.unk || a.mem || a.toks.is_empty());
        for &t in &a.toks {
            match t {
                Tok::Num(c) | Tok::Fn { key: c, .. } if is_ram(c) => {
                    self.g.store(Store::Cell(ram_canon(c)), toks)
                }
                // Somewhere in an object: stays in it, as an index stays in
                // its array (`native/fnref-ram-pointers`).
                Tok::Ram(0) | Tok::Ram(2) => anywhere = true,
                Tok::Ram(1) => self.g.store(Store::Cell(STACK_CELL), toks),
                Tok::Ram(3) => self.g.store(Store::Cell(ANYOBJ_CELL), toks),
                Tok::Ram(o) => self.g.store(Store::Cell(obj_cell(o)), toks),
                // In an allocation: its word's cell, or, somewhere in it
                // (or in any allocation), the site's any-cell.
                Tok::Heap(_, HEAP_UNDER) => {
                    debug_assert!(false, "a header's pointer not lowered");
                    anywhere = true;
                }
                Tok::Heap(s, k) => {
                    let k = if s == 0 || (k & 3) + w as u32 > 4 {
                        HEAP_ANY
                    } else {
                        k
                    };
                    self.g.store(Store::Cell(heap_cell(s, k)), toks)
                }
                // I/O, the display memories, SRAM: not followed.
                Tok::Num(c) | Tok::Fn { key: c, .. } if self.img.word(c).is_none() => {
                    self.g.hole(x, self.at, Why::Outside, |_| true)
                }
                Tok::Num(_)
                | Tok::Rom(_)
                | Tok::RomFrom(_)
                | Tok::RomStep(..)
                | Tok::RomDull
                | Tok::RomAny
                | Tok::Fn { .. } => {}
                Tok::Ref { .. } => debug_assert!(false, "a Ref not expanded"),
            }
            if matches!(t, Tok::Ref { .. })
                || !ram
                    && matches!(
                        t,
                        Tok::RomFrom(_) | Tok::RomStep(..) | Tok::RomDull | Tok::RomAny
                    )
            {
                anywhere = true;
            }
        }
        // (A ROM pointer, exact or from here on, is no RAM address: by
        // the closed world, a store through it reaches no load.)
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
                let args = if out.takes { &s.regs[..] } else { &s.regs[..4] };
                let rets = match target {
                    Some(k) => {
                        self.g.give(*k, args);
                        let mut r = self.ret_of(*k);
                        if self.g.allocs.contains(k) {
                            r[0] = self.g.heap_alloc(out.at, &r[0]);
                        }
                        self.g.kept(*k, &mut r, &s.regs);
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
                self.g.give(*k, &s.regs);
                let mut r = self.ret_of(*k);
                if self.g.allocs.contains(k) {
                    r[0] = self.g.heap_alloc(out.at, &r[0]);
                }
                self.g.kept(*k, &mut r, &s.regs);
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
                // CpuSet and CpuFastSet copy their source's words to RAM:
                // what a load of it reads, to the pile.
                if matches!(out.bios, Some(0x0B | 0x0C)) {
                    let fast = out.bios == Some(0x0C);
                    let n = copy_len(fast, &s.regs[2]);
                    let mut got = Val::default();
                    self.bios_read(&s.regs[0], n, &mut got);
                    self.g.take_large(&mut got);
                    let (toks, _) = to_mem(&got, 4);
                    self.g.store(Store::Pile, &toks);
                    // CpuFastSet leaves in r3 a word it read: the fill's, or
                    // its last block's first.
                    if fast {
                        after.regs[3].join(&got);
                    }
                }
                // The calls that compute with what they read through a
                // pointer (the affine sets' parameters, the decompressors'
                // data, MidiKey2Freq's wave header): a function's address
                // there is lost, as one in a register they read is.
                if let Some((r, n)) = bios_mem(out.bios, &s.regs[2]) {
                    let mut got = Val::default();
                    self.bios_read(&s.regs[r], n, &mut got);
                    self.g.hole(&got, out.at, Why::Bios, |_| true);
                }
                self.flow(*next, &after);
            }
            Edge::Stop => {}
        }
    }

    /// What a BIOS call reads of memory at `src`, `n` bytes of it if
    /// the run knows how many (`native/fnref-bios-ram-copies`): a load's
    /// words, as a load at width 4 reads them. In ROM, its words to
    /// there, or to its object's end; in RAM at a constant address, the
    /// cells in those bytes (every cell, `n` unknown or past the region's
    /// end) and the pile; anywhere else (a frame slot whose address was
    /// taken, the heap, an unknown), the pile and every cell.
    fn bios_read(&mut self, src: &Val, n: Option<u32>, into: &mut Val) {
        let src = self.g.expand(src);
        into.lost_rom |= src.lost_rom;
        let mut pile = false;
        let mut anywhere = src.unk || src.mem || src.toks.is_empty();
        for &t in &src.toks {
            match t {
                Tok::Rom(a) => self.rom_from(a, 4, None, n, into),
                Tok::RomFrom(a) | Tok::RomStep(a, _) => self.rom_from(a, 4, None, None, into),
                // Instructions' bytes: numbers.
                Tok::RomDull => into.unk = true,
                Tok::RomAny => {
                    into.unk = true;
                    into.lost_rom = true;
                }
                Tok::Num(c) | Tok::Fn { key: c, .. } if is_ram(c) => {
                    pile = true;
                    let lo = ram_canon(c);
                    let end = if lo >> 24 == 2 {
                        0x0204_0000
                    } else {
                        0x0300_8000
                    };
                    match n.map(|n| lo.saturating_add(n.max(1))) {
                        Some(hi) if hi <= end => {
                            self.g.rd.range(lo.saturating_sub(3), hi - 1);
                            for (_, m) in self.g.cells.range(lo.saturating_sub(3)..hi) {
                                from_mem(m, 4, into);
                            }
                            // And what was stored somewhere in the objects
                            // those bytes are in.
                            let (o, _) = ram_obj(lo);
                            let os: Vec<u32> = RAM_OBJS
                                .with(|c| c.borrow().range(o..hi).map(|(&o, _)| o).collect());
                            for o in std::iter::once(o).chain(os).filter(|_| ram_on()) {
                                self.g.read_obj(obj_cell(o), 4, into);
                            }
                            if ram_on() {
                                self.g.read_obj(ANYOBJ_CELL, 4, into);
                            }
                        }
                        _ => anywhere = true,
                    }
                }
                Tok::Num(c) | Tok::Fn { key: c, .. } if self.img.word(c).is_some() => {
                    self.rom_from(c, 4, None, n, into)
                }
                Tok::Ram(1) => {
                    pile = true;
                    self.g.read_obj(STACK_CELL, 4, into);
                }
                Tok::Ram(2) => {
                    pile = true;
                    self.g.read_cells(4, into);
                    self.g.read_obj(STACK_CELL, 4, into);
                }
                Tok::Ram(o) if o != 0 => self.g.read_in_obj(o, 4, into),
                Tok::Heap(s, k) if k != HEAP_UNDER => self.g.read_heap(s, HEAP_ANY, 4, into),
                // I/O, VRAM, OAM: nothing the run stores a function's
                // address to but through the pile.
                _ => anywhere = true,
            }
        }
        if pile || anywhere {
            self.g.read_pile(4, into);
        }
        if anywhere {
            self.g.read_cells(4, into);
            if ram_on() {
                self.g.read_objs(4, into);
            }
        }
        into.cap();
    }

    /// A call or tail call through `t`: give `args` to each callee, and
    /// what comes back.
    fn call_by_value(&mut self, t: &Val, at: u32, args: &[Val], tail: bool) -> Vec<Val> {
        let (callees, complete) = self.callees(t, at, tail);
        // A target no function's address has reached yet (the function
        // that stores or passes it hasn't run, or not with its own
        // values; an unknown is none, in the closed world) is no call
        // yet. Taken as an incomplete set at once, its arguments went to
        // every loose function for good, though a round later it called
        // one function: which words that un-proved was the order the
        // functions ran in. It waits until the run has otherwise settled;
        // one still without a callee then is the incomplete set it
        // always was. (One that may hold an address the run dropped
        // doesn't wait: it is incomplete whatever comes.)
        if self.g.defer && !self.g.deferred_due && callees.is_empty() && !t.lost_rom {
            self.g.deferred.insert(self.f.entry);
            return vec![Val::unk(); 4];
        }
        // Large arguments as the entries keep them ([`Large::take`]), so
        // that what a callee set was given before is told from what's new.
        let args: Vec<Val> = args
            .iter()
            .map(|a| {
                let mut a = a.clone();
                self.g.take_large(&mut a);
                a
            })
            .collect();
        let args = &args[..];
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
                        self.g.sets.share(&mut o.toks);
                    }
                }
                let mut grew = false;
                let mut once = Once::default();
                for &k in &callees {
                    self.rets_read.insert(k);
                    if self.g.ret_stamp.get(&k).is_some_and(|&s| s > m.stamp) {
                        for (i, (a, b)) in m.rets.iter_mut().zip(&self.g.ret[&k]).enumerate() {
                            // (What it may change: [`G::kept`].)
                            grew |= self.g.sets(k, i) && once.first(i, b) && a.union_new(b);
                        }
                    }
                }
                if grew {
                    for a in &mut m.rets {
                        a.cap();
                        self.g.take_large(a);
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
                self.g.give_all(&callees, args);
                let mut once = Once::default();
                for &k in &callees {
                    for (i, (a, b)) in rets.iter_mut().zip(self.ret_of(k)).enumerate() {
                        if self.g.sets(k, i) && once.first(i, &b) {
                            a.union_new(&b);
                        }
                    }
                }
                for a in &mut rets {
                    a.cap();
                    self.g.take_large(a);
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
        // A register some callee leaves as it came is, after that one,
        // the caller's own ([`G::kept`]).
        let left = self.g.left_by(&callees);
        for (i, r) in rets.iter_mut().enumerate() {
            if left & 1 << i != 0 {
                r.join(&args[i]);
                self.g.take_large(r);
            }
        }
        if !complete {
            self.g.rd.incomplete = true;
            // (With the RAM pointers and calls that wait: from now on
            // every address-taken function has callers the run can't
            // see, [`follow_live`].)
            if !self.g.unseen_calls && self.g.defer && ram_on() {
                self.g.unseen_calls = true;
                self.g.dirty_loose = true;
            }
            self.g
                .watch_regs(args, |r| format!("passed in r{r} to every loose function"));
            let mut c = false;
            let mut loose = std::mem::take(&mut self.g.loose_args);
            for (a, b) in loose.iter_mut().zip(args) {
                let old = a.clone();
                if a.join_capped(b) {
                    self.g.take_large(a);
                    if *a == old {
                        *a = old;
                        continue;
                    }
                    self.g.sets.share(&mut a.toks);
                    c = true;
                }
            }
            self.g.loose_args = loose;
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
                                if self.g.sets(*k, i) && once.first(i, b) {
                                    a.join_capped(b);
                                }
                            }
                        }
                    }
                    self.g.loose_leaves = self.g.left_by(self.loose);
                    for a in &mut v {
                        self.g.take_large(a);
                        self.g.sets.share(&mut a.toks);
                    }
                    self.g.loose_rets = Some(v.clone());
                    v
                }
            };
            self.rets_read.extend(self.loose.iter().copied());
            for (i, (a, b)) in rets.iter_mut().zip(&lr).enumerate() {
                a.join_capped(b);
                // (And as some loose function leaves it: the caller's.)
                if self.g.loose_leaves & 1 << i != 0 {
                    a.join(&args[i]);
                    self.g.take_large(a);
                }
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
struct Once(std::collections::HashSet<(usize, *const BTreeSet<Tok>, bool, bool, bool), Fx>);

impl Once {
    /// Whether `v` is new to register `i`.
    fn first(&mut self, i: usize, v: &Val) -> bool {
        v.toks.is_empty()
            || self
                .0
                .insert((i, std::rc::Rc::as_ptr(&v.toks.0), v.mem, v.unk, v.lost_rom))
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

/// Whether mask `m` holds bits of the top half only and function `key`
/// in form `f` has every one of them clear: then `& m` gives 0 for it,
/// and for any dense ID, whose top half is 0 (a byte or half mask, which
/// may split an address to be joined again, isn't one).
fn high_clear(f: Form, key: u32, m: u32) -> bool {
    let v = match f {
        Form::Full => key,
        Form::Lo => key & 0xffff,
        Form::Hi => key >> 16,
        Form::HiUp => key & 0xffff_0000,
        _ => return false,
    };
    m != 0 && m & 0xffff == 0 && v & m == 0
}

/// A token of `op` on tokens `x` and `y` (`None`: an unknown, a whole
/// number of `steps[0]` for `x`, of `steps[1]` for `y`).
fn bin_tok(img: &dyn Image, op: BinOp, x: Option<Tok>, y: Option<Tok>, steps: [u32; 2]) -> R {
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
            // (A RAM address is no function's nor a dense ID: the
            // closed world keeps it a pointer, `native/fnref-ram-pointers`.)
            (
                BinOp::Sub,
                Full,
                None | Some(Num(0)) | Some(Fn { form: Full, .. }) | Some(Ram(_)) | Some(Heap(..)),
            ) => fnt(Cmp),
            (BinOp::Sub, Full, Some(Num(c))) if is_ram(c) && ram_on() => fnt(Cmp),
            (BinOp::Or, Full, Some(Num(1))) if kx & 1 == 1 => fnt(Full),
            (BinOp::Or | BinOp::Add, _, Some(Num(0))) | (BinOp::And, _, Some(Num(0xffff_ffff))) => {
                fnt(fx)
            }
            (BinOp::And, Full | Lo, Some(Num(0xffff))) => fnt(Lo),
            // A test of high bits the address has clear (`movs r1, #128;
            // lsls r1, #17; ands r1, r0`: bit 24): 0, and 0 when dense too,
            // a dense ID being below 2^16 (`native/fnref-bit-test-mask`).
            (BinOp::And, Full | Lo | Hi | HiUp, Some(Num(m))) if high_clear(fx, kx, m) => {
                R::T(Num(0))
            }
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
    // A pointer "from here on" every `k` bytes, if `k` is more than 1.
    let step = |a: u32, k: u32| match u16::try_from(k) {
        Ok(k) if k > 1 => RomStep(a, k),
        _ => RomFrom(a),
    };
    // A RAM address plus or less an index the run doesn't know:
    // somewhere in its object (`&tasks[i]`); any other op on one that
    // isn't exact (less a pointer, an alignment test) makes a number, by
    // the closed world, as on a ROM pointer. (Off, [`ram_on`]: none is
    // made, and a RAM address is a number as any other.)
    match (op, x, y) {
        _ if !ram_on() => {}
        (BinOp::Add | BinOp::Sub, Some(Num(c)), None) if is_ram(c) => {
            return R::T(Ram(ram_obj(c).0));
        }
        (BinOp::Add, None, Some(Num(c))) if is_ram(c) => return R::T(Ram(ram_obj(c).0)),
        (BinOp::Add | BinOp::Sub, Some(Ram(o)), None) | (BinOp::Add, None, Some(Ram(o))) => {
            return R::T(Ram(o));
        }
        (BinOp::Add | BinOp::Sub, Some(Ram(o)), Some(Num(_)))
        | (BinOp::Add, Some(Num(_)), Some(Ram(o))) => return R::T(Ram(o)),
        (BinOp::And, Some(Ram(o)), Some(Num(m))) | (BinOp::And, Some(Num(m)), Some(Ram(o)))
            if m >= 0xffff_0000 =>
        {
            return R::T(Ram(o));
        }
        // A pointer into an allocation (`native/fnref-heap-sites`): plus
        // or less a constant, that offset; an index the run doesn't
        // know, somewhere in it; aligned down, the word it is in. Below
        // the pointer the call returned it is the allocator's header.
        (BinOp::Add | BinOp::Sub, Some(Heap(s, k)), None)
        | (BinOp::Add, None, Some(Heap(s, k))) => {
            return R::T(Heap(s, if k == HEAP_UNDER { k } else { HEAP_ANY }));
        }
        (BinOp::Add, Some(Heap(s, k)), Some(Num(c)))
        | (BinOp::Add, Some(Num(c)), Some(Heap(s, k))) => {
            return R::T(heap_off(s, k, c as i32 as i64));
        }
        (BinOp::Sub, Some(Heap(s, k)), Some(Num(c))) => {
            return R::T(heap_off(s, k, -(c as i32 as i64)));
        }
        (BinOp::And, Some(Heap(s, k)), Some(Num(m)))
        | (BinOp::And, Some(Num(m)), Some(Heap(s, k)))
            if m >= 0xffff_0000 =>
        {
            // An allocation starts on a word, so aligned down to one the
            // offset is; to more, it is at or past the start if the
            // offset was past the mask's reach, and else on either side
            // of it: a pointer the run drops.
            return R::T(match k {
                HEAP_ANY | HEAP_UNDER => Heap(s, k),
                k if m >= 0xffff_fffc => Heap(s, k & m),
                k if k >= !m => Heap(s, HEAP_ANY),
                _ => RAM_ANY,
            });
        }
        (_, Some(Ram(_) | Heap(..)), _) | (_, _, Some(Ram(_) | Heap(..))) => return R::Unk,
        _ => {}
    }
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        // An index scaled by a record's size: that field of each record.
        (Some(RomDull), None) | (None, Some(RomDull)) if op == BinOp::Add => {
            return R::T(RomDull);
        }
        // Somewhere in ROM, moved: somewhere in ROM (dropped already).
        (Some(RomAny), None) if matches!(op, BinOp::Add | BinOp::Sub) => return R::T(RomAny),
        (None, Some(RomAny)) if op == BinOp::Add => return R::T(RomAny),
        (Some(Rom(a)), None) if op == BinOp::Add => return R::T(step(a, steps[1])),
        (None, Some(Rom(a))) if op == BinOp::Add => return R::T(step(a, steps[0])),
        (Some(RomStep(a, k)), None) if op == BinOp::Add => {
            return R::T(step(a, gcd(k as u32, steps[1])));
        }
        (None, Some(RomStep(a, k))) if op == BinOp::Add => {
            return R::T(step(a, gcd(k as u32, steps[0])));
        }
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
        (RomDull, Num(_)) | (Num(_), RomDull) if op == BinOp::Add => R::T(RomDull),
        // Aligned down, it reads the same words ([`dull`] is by words).
        // Under a coarser mask it is a number, as a pointer through any
        // other op the run doesn't follow: what this takes on trust is
        // that code doesn't mask a pointer into an instruction or a pool
        // of constants down to a page and read a table of functions
        // there. (Metroid masks words of flags that hold such pointers
        // only by the pile's width, with 0xffff0000 and !0x20; dropped,
        // `Val::lost_rom`, they left 7 of its 70 callee sets complete.)
        (RomDull, Num(m)) | (Num(m), RomDull) if op == BinOp::And && m >= 0xffff_fffc => {
            R::T(RomDull)
        }
        (RomAny, Num(_)) if matches!(op, BinOp::Add | BinOp::Sub) => R::T(RomAny),
        (Num(_), RomAny) if op == BinOp::Add => R::T(RomAny),
        (RomAny, Num(m)) | (Num(m), RomAny) if op == BinOp::And && m >= 0xffff_0000 => R::T(RomAny),
        (RomFrom(a), Num(b)) if op == BinOp::Sub => {
            let p = a.wrapping_sub(b);
            if img.word(p).is_some() {
                R::T(RomFrom(p))
            } else {
                R::Unk
            }
        }
        (RomFrom(a), Num(m)) if op == BinOp::And && m >= 0xffff_0000 => R::T(RomFrom(a & m)),
        // Whole records on or back: the same field (a loop walking the
        // table keeps it). Else a field's offset: the field `b` past this
        // one; and back, as one "from here on" never goes back past
        // where it is (an index from the table's start less one record
        // is a record of the table), the field `b` modulo the record on.
        (RomStep(a, k), Num(b)) | (Num(b), RomStep(a, k)) if op == BinOp::Add => {
            let k = k as i32;
            let b = b as i32;
            let on = if b % k == 0 {
                0
            } else if b > 0 {
                b
            } else {
                b.rem_euclid(k)
            };
            R::T(RomStep(a.wrapping_add(on as u32), k as u16))
        }
        (RomStep(a, k), Num(b)) if op == BinOp::Sub => {
            let p = a.wrapping_sub(b);
            if img.word(p).is_some() {
                R::T(RomStep(p, k))
            } else {
                R::Unk
            }
        }
        (RomStep(a, _), Num(m)) if op == BinOp::And && m >= 0xffff_0000 => R::T(RomFrom(a & m)),
        _ => R::Unk,
    }
}

/// `x op y` of a RAM address and a number that leaves the address's RAM
/// object ([`ram_parts`]) for another's: a pointer somewhere in the
/// address's object, and whether the address was `x`. (A result outside
/// RAM is two values' tokens paired, not a pointer moved: a number.)
fn left_object(img: &dyn Image, op: BinOp, x: Option<Tok>, y: Option<Tok>) -> Option<(Tok, bool)> {
    if !ram_on() {
        return None;
    }
    let (p, r, on_x) = match (op, x, y) {
        (BinOp::Add, Some(Tok::Num(p)), Some(Tok::Num(k))) if is_ram(p) && !is_ram(k) => {
            (p, p.wrapping_add(k), true)
        }
        (BinOp::Add, Some(Tok::Num(k)), Some(Tok::Num(p))) if is_ram(p) && !is_ram(k) => {
            (p, p.wrapping_add(k), false)
        }
        (BinOp::Sub, Some(Tok::Num(p)), Some(Tok::Num(k))) if is_ram(p) && !is_ram(k) => {
            (p, p.wrapping_sub(k), true)
        }
        _ => return None,
    };
    let o = ram_obj(p).0;
    (is_ram(r) && img.word(r).is_none() && ram_obj(r).0 != o).then_some((Tok::Ram(o), on_x))
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// What each value of a block is a whole number of, by its index
/// (`native/fnref-table-fields`): 1, nothing known, or the stride of an
/// index scaled by a record's size, as the block's own shifts, adds and
/// multiplies by constants make it (`lsl r0, r1, #1; add r0, r1; lsl r0,
/// #3` is 24 times r1). Each value is followed as `factor * root`, a
/// value the block reads or makes otherwise, through its registers. The
/// factor itself is the stride when the root is under 2^16 (a byte or
/// halfword load, a mask, a shift right by 16 or more) and the factor
/// too, so the product doesn't wrap; otherwise only the factor's power
/// of two divides what wraps.
fn strides(insts: &[Inst]) -> Vec<u32> {
    factors(insts)
        .iter()
        .map(|&(_, f, small)| {
            if f == 0 || f > u32::MAX as u64 {
                // (A product that saturated: nothing known.)
                return 1;
            }
            let f = f as u32;
            if small && f <= 0xffff {
                f
            } else {
                // Its power of two, as a step keeps it.
                f.isolate_lowest_one().min(0x8000)
            }
        })
        .collect()
}

/// Each value of a block as `factor * root` ([`strides`]): (root,
/// factor, root under 2^16), a factor over 2^32 kept as 0.
fn factors(insts: &[Inst]) -> Vec<(usize, u64, bool)> {
    // (root, factor, root under 2^16), a factor over 2^32 kept as 0.
    let mut lin: Vec<(usize, u64, bool)> = Vec::with_capacity(insts.len());
    let mut regs: [Option<usize>; 16] = [None; 16];
    let konst = |x: ValueId| match insts[x.index()].op {
        Op::Const(c) => Some(c),
        Op::Bit(b) => Some(b as u32),
        _ => None,
    };
    let times = |l: (usize, u64, bool), c: u64| (l.0, l.1.saturating_mul(c), l.2);
    for (i, inst) in insts.iter().enumerate() {
        let me = (i, 1, false);
        let small = (i, 1, true);
        let l = match inst.op {
            Op::GetReg(r) => match regs[r.index()] {
                Some(v) => lin[v],
                None => {
                    regs[r.index()] = Some(i);
                    me
                }
            },
            Op::SetReg(r, x) => {
                regs[r.index()] = inst.guard.is_none().then_some(x.index());
                me
            }
            // A mode switch banks registers: none read before is known
            // to be the one read after.
            Op::SetCpsr { .. } | Op::SetSpsr { .. } | Op::SetUserReg(..) => {
                regs = [None; 16];
                me
            }
            Op::Load {
                width: Width::Byte | Width::Half,
                ..
            } => small,
            Op::Shift {
                kind: ShiftType::Lsl,
                v,
                amount,
            } => match konst(amount) {
                Some(n) if n < 32 => times(lin[v.index()], 1 << n),
                _ => me,
            },
            Op::Shift {
                kind: ShiftType::Lsr,
                amount,
                ..
            } if konst(amount).is_some_and(|n| n >= 16) => small,
            Op::Bin(BinOp::And, a, b) if konst(a).or(konst(b)).is_some_and(|m| m <= 0xffff) => {
                small
            }
            Op::Bin(BinOp::Mul, a, b) => match (konst(a), konst(b)) {
                (Some(c), None) => times(lin[b.index()], c as u64),
                (None, Some(c)) => times(lin[a.index()], c as u64),
                _ => me,
            },
            Op::Bin(BinOp::Add, a, b) | Op::Adc { a, b, c: _ }
                if matches!(inst.op, Op::Bin(..))
                    || matches!(inst.op, Op::Adc { c, .. } if konst(c) == Some(0)) =>
            {
                let ((ra, fa, sa), (rb, fb, _)) = (lin[a.index()], lin[b.index()]);
                if ra == rb {
                    (ra, fa.saturating_add(fb), sa)
                } else {
                    me
                }
            }
            _ => me,
        };
        lin.push(l);
    }
    lin
}

/// `GBAA_FNREF_OFF=tables,pile,cells`: kinds of hole made not to
/// happen, to measure what the words would be without them
/// (`docs/research/fnref-held-words.md`). **For measuring only**, never
/// for a lift that is built (the report's first line says which were
/// off). All unsound, to see what an exact rule could prove at most:
/// `tables`: a ROM read by index, or one the value sets bound, stops at
/// the next object's start even between function words (it reads fewer
/// words, so more are never read); `pile`, `cells`: a pile read, an
/// every-cell read, gives nothing. (Its `bios` is the rule now:
/// [`bios_regs`]; so are `move`, old GCC's `adds rd, rs, #0`, and of
/// `flags` what is sound: [`flags_of`], [`unused`].)
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
        eprintln!("GBAA_FNREF_OFF={v}: the function flow is unsound, for measuring only");
    }
    off
}

const OFF_KINDS: [&str; 3] = ["tables", "pile", "cells"];
const OFF_TABLES: u8 = 1;
const OFF_PILE: u8 = 2;
const OFF_CELLS: u8 = 4;

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

/// Which register BIOS call `n` reads memory through, besides the
/// copies, and how many bytes from it if `r2` (the count) says
/// (`native/fnref-bios-ram-copies`). The affine sets read `r2` entries
/// of 20 or 8 bytes; the decompressors their header's length, which the
/// run doesn't know; `MidiKey2Freq` the wave header's frequency at
/// `r0 + 4`. The other calls a native build makes read no memory the
/// program wrote: the waits read the BIOS's IRQ flags, and `MultiBoot`'s
/// lifted form reads nothing.
fn bios_mem(n: Option<u8>, r2: &Val) -> Option<(usize, Option<u32>)> {
    let count = |unit: u32| {
        if r2.unk || r2.mem || r2.toks.is_empty() {
            return None;
        }
        let mut most = 0;
        for &t in &r2.toks {
            let Tok::Num(m) = t else { return None };
            most = most.max(m.saturating_mul(unit));
        }
        Some(most)
    };
    match n? {
        0x0e => Some((0, count(20))),
        0x0f => Some((0, count(8))),
        0x11..=0x15 => Some((0, None)),
        0x1f => Some((0, Some(8))),
        _ => None,
    }
}

/// `GBAA_FNREF_WATCH`'s words: `0x080008c8[,…]`, the ROM words that hold a
/// function's address, each with its key (`roads`, or a word that holds
/// none, watches nothing).
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
        "{{{fns} function tokens, {} others: {}{}{}{}}}",
        v.toks.len() - fns,
        rest.join(" "),
        if v.unk { " unk" } else { "" },
        if v.mem { " mem" } else { "" },
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
                Src::RomFrom(lo, end, w, 1) => {
                    format!("a ROM read by index w{w} of {lo:#010x}..{end:#010x}")
                }
                Src::RomFrom(lo, end, w, k) => {
                    format!("a ROM read by index w{w} of {lo:#010x}..{end:#010x} every {k}")
                }
                Src::Hull(lo, hi, w) => {
                    format!("a bounded ROM read w{w} of {lo:#010x}..={hi:#010x}")
                }
                Src::Obj(STACK_CELL, w) => format!("a read w{w} of the stacks' cell"),
                Src::Obj(ANYOBJ_CELL, w) => {
                    format!("a read w{w} of what was stored somewhere in some RAM object")
                }
                Src::Obj(k, w) if k >= in_cell(0x0200_0000) && k < HEAP_BASE => format!(
                    "a read w{w} of the cells in the RAM object at {:#010x}",
                    obj_of_cell(k - 0x0800_0000).unwrap_or(k)
                ),
                Src::Obj(k, w) => format!(
                    "a read w{w} of what was stored somewhere in the RAM object at {:#010x}",
                    obj_of_cell(k).unwrap_or(k)
                ),
                Src::Objs(w) => format!("a read w{w} of every RAM object's cell"),
                Src::Large => "the large values' set".to_string(),
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
/// `allocs`: the functions the holes file names as allocators, looked at
/// only with `GBAA_FNREF_HEAP` set ([`heap_env`]).
/// What the lift knows of a program besides its code, for [`follow`].
#[derive(Clone, Copy, Debug)]
pub struct Program<'a> {
    /// The functions the holes file names as allocators, looked at only
    /// with `GBAA_FNREF_HEAP` set ([`heap_env`]).
    pub allocs: &'a BTreeSet<u32>,
    /// Its named data, start and size in bytes, as its names files give
    /// them: with the RAM pointers on ([`ram_on`]) each such global in
    /// RAM is an object ([`ram_parts`]).
    pub named: &'a BTreeMap<u32, u32>,
}

pub fn follow(fns: &[FnIn], img: &dyn Image, copies: &[(u32, u32, u32)], known: Program) -> Flow {
    let none = BTreeSet::new();
    let heap = heap_env();
    follow_live(
        fns,
        img,
        copies,
        ram_env() || heap,
        if heap { known.allocs } else { &none },
        live_env(),
        known.named,
    )
}

/// Whether `f` has a jump by value the run takes for a tail call.
fn jumps_by_value(f: &FnIn) -> bool {
    f.blocks
        .values()
        .flat_map(|b| b.exits.values())
        .any(|o| matches!(o.edge, Edge::Tail(None) | Edge::TailOr(_)))
}

/// [`follow`] as told: RAM pointers kept or not ([`ram_on`]); the
/// allocators whose calls return an allocation of their own
/// (`native/fnref-heap-sites`; none: no [`Tok::Heap`] is made); and
/// whether a call takes back only what its callee may change
/// ([`live_env`]).
fn follow_live(
    fns: &[FnIn],
    img: &dyn Image,
    copies: &[(u32, u32, u32)],
    ram: bool,
    allocs: &BTreeSet<u32>,
    live: bool,
    named: &BTreeMap<u32, u32>,
) -> Flow {
    debug_assert!(
        ram || allocs.is_empty(),
        "allocation sites rest on the RAM rules"
    );
    let _code = CodeGuard::set(img);
    let _objs = ObjsGuard::set(ram, fns, named);
    let mut g = G {
        loose_args: vec![Val::default(); 4],
        objects: objects(img),
        tables: tables(fns, img),
        watch: watch_env(img),
        off: off_env(),
        roads: std::env::var_os("GBAA_FNREF_WATCH").is_some(),
        gains: std::env::var_os("GBAA_FNREF_GAINS").map(|_| BTreeMap::new()),
        allocs: allocs.clone(),
        // (Neither for a function that jumps by value: its registers go
        // on to wherever its target's value says, which may be more
        // functions than its summary followed.)
        leaves: fns
            .iter()
            .filter(|f| live && f.leaves != 0 && !jumps_by_value(f))
            .map(|f| (f.entry, f.leaves))
            .collect(),
        reads: fns
            .iter()
            .filter(|f| live && f.reads & 0xf != 0xf && !jumps_by_value(f))
            .map(|f| (f.entry, f.reads))
            .collect(),
        defer: live,
        by_value_read: ram,
        based: fns
            .iter()
            .filter(|_| ram)
            .flat_map(|f| {
                ram_indexed(f)
                    .into_iter()
                    .filter(|(k, _)| k.1 & BASED != 0)
                    .map(|((b, k), x)| ((f.entry, b, k), x))
            })
            .collect(),
        ..G::default()
    };
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
            } else if ram && let Some(w) = img.word(p).filter(|&w| is_ram(w)) {
                // A RAM address copied: memory keeps every one (the
                // closed world).
                g.store(Store::Cell(dst + (p - src)), &[(Tok::Num(ram_canon(w)), 4)]);
            } else if ram && let Some(w) = img.word(p).filter(|&w| img.word(w).is_some()) {
                // And a ROM one (a vtable's address in initialized data).
                g.store(Store::Cell(dst + (p - src)), &[(Tok::Rom(w), 4)]);
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
            v.push(g.objs_read(w).clone());
        }
        v
    };
    let mut snapped = snap(&mut g);
    g.cells_changed.clear();
    g.pile_changed = false;
    let mut loose: BTreeSet<u32> = BTreeSet::new();
    let mut ran = 0usize;
    // Each function's place in the order they run in. With the RAM
    // pointers on, a function whose entry or whose callee's results a run
    // changed runs in the same round where it comes later in that order
    // (the pickup, `native/fnref-ram-reruns`), not a round later: a
    // change went one call a round (Metroid's flow 41 rounds to 30,
    // Emerald's 21 to 15, the results the same).
    let pos: std::collections::HashMap<u32, usize, Fx> =
        fns.iter().enumerate().map(|(i, f)| (f.entry, i)).collect();
    debug_assert_eq!(pos.len(), fns.len(), "a function listed twice");
    let pickup = g.by_value_read;
    let mut readers: std::collections::HashMap<u32, BTreeSet<u32>, Fx> = Default::default();
    let mut read_fwd: std::collections::HashMap<u32, BTreeSet<u32>, Fx> = Default::default();
    let mut incomplete: BTreeSet<u32> = BTreeSet::new();
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
        // Settled but for the calls that waited on a target nothing
        // reached: they are incomplete sets now, and the run goes on.
        if dirty.is_empty() && g.defer && !g.deferred_due {
            g.deferred_due = true;
            dirty = std::mem::take(&mut g.deferred);
            // And the functions a caller the run can't see may call are
            // run as that: until now none was.
            if ram {
                dirty.extend(loose.iter().copied());
            }
        }
        if dirty.is_empty() {
            return finish(g);
        }
        if round == 256 {
            break;
        }
        let mut queue: BTreeSet<usize> = dirty.iter().filter_map(|k| pos.get(k).copied()).collect();
        // (With the pickup: what changed this round, and what reruns in
        // the next.)
        let mut next: BTreeSet<u32> = BTreeSet::new();
        let mut seen_entries: BTreeSet<u32> = BTreeSet::new();
        let mut seen_rets: BTreeSet<u32> = BTreeSet::new();
        let mut loose_ret_at: Option<usize> = None;
        while let Some(i) = queue.pop_first() {
            let f = &fns[i];
            let mut entry = g
                .entry
                .get(&f.entry)
                .cloned()
                .unwrap_or_else(|| vec![Val::default(); 16]);
            if loose.contains(&f.entry) {
                for (a, b) in entry.iter_mut().zip(&g.loose_args) {
                    a.join(b);
                }
                // Whether a caller the run can't see may call it. With
                // the RAM pointers and the registers' rules (the closed
                // world, and calls that wait: [`live_env`]): once the
                // run has otherwise settled, the reset and what no code
                // names, and the IRQ vector's function (the BIOS calls
                // it); and from when some call's callee set is
                // incomplete, every function whose address is taken.
                // Before, every such function from the first round: its
                // parameters were unknowns and pointers anywhere, so a
                // callback given `&gObjs[i]` read every global.
                let vector = |g: &G| {
                    g.cells.get(&IRQ_VECTOR).is_some_and(|m| {
                        m.iter()
                            .any(|(t, _)| matches!(t, Tok::Fn { key, .. } if *key == f.entry))
                    })
                };
                let seen = ram
                    && g.defer
                    && !g.unseen_calls
                    && !(g.deferred_due && (!g.taken.contains(&f.entry) || vector(&g)));
                if !seen {
                    // r0-r3 from a caller the run can't see: somewhere in
                    // RAM it doesn't place ([`LOOSE`]).
                    if ram {
                        for r in &mut entry[..4] {
                            r.toks.insert(LOOSE);
                        }
                    }
                    for r in &mut entry {
                        r.unk = true;
                    }
                }
            }
            for r in &mut entry[4..] {
                r.unk = true;
            }
            // sp: somewhere in a stack (its frame's accesses the run
            // places apart, [`super::frame`]).
            if ram {
                entry[13].toks.insert(STACK);
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
                unused: unused(f),
                lowered: false,
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
            run.g.deferred.remove(&f.entry);
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
            let mut rd = std::mem::take(&mut g.rd);
            rd.settle();
            if pickup {
                // Whose results it read, the other way round; an
                // incomplete call's (every loose function's) by its flag.
                let new: BTreeSet<u32> = if rd.incomplete {
                    incomplete.insert(f.entry);
                    r.difference(&loose).copied().collect()
                } else {
                    incomplete.remove(&f.entry);
                    r.clone()
                };
                if let Some(old) = read_fwd.get(&f.entry) {
                    for k in old.difference(&new) {
                        if let Some(s) = readers.get_mut(k) {
                            s.remove(&f.entry);
                        }
                    }
                }
                for &k in &new {
                    readers.entry(k).or_default().insert(f.entry);
                }
                read_fwd.insert(f.entry, new);
                // What this run changed for a function later in the
                // order runs it this round; for one at or before it, the
                // next.
                let mut push = |x: u32, queue: &mut BTreeSet<usize>| match pos.get(&x) {
                    Some(&p) if p > i => {
                        queue.insert(p);
                    }
                    Some(_) => {
                        next.insert(x);
                    }
                    None => {}
                };
                for e in std::mem::take(&mut g.dirty_entry) {
                    push(e, &mut queue);
                    seen_entries.insert(e);
                }
                for k in std::mem::take(&mut g.dirty_ret) {
                    if let Some(s) = readers.get(&k) {
                        for &x in s {
                            push(x, &mut queue);
                        }
                    }
                    if loose.contains(&k) {
                        // (What their results join to is kept: made
                        // again for the incomplete calls still to run.
                        // [`G::give_ret`] does that by `G::loose`, which
                        // is empty in the first round.)
                        g.loose_rets = None;
                        if loose_ret_at.is_none() {
                            for &x in &incomplete {
                                push(x, &mut queue);
                            }
                        }
                        loose_ret_at = Some(i);
                    }
                    seen_rets.insert(k);
                }
            }
            reads.insert(f.entry, r);
            rds.insert(f.entry, rd);
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
        let mut n_entry = dirty.len();
        let mut n_rets = rets.len();
        if pickup {
            // (An incomplete call that ran before the last loose
            // function's results changed.)
            if let Some(at) = loose_ret_at {
                next.extend(incomplete.iter().copied().filter(|x| pos[x] <= at));
            }
            dirty.extend(next);
            n_entry = seen_entries.len();
            n_rets = seen_rets.len();
        }
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
        let at = g.objs_changed(&cells);
        // The IRQ vector's function is called by the BIOS: one stored
        // there now runs again, as that ([`IRQ_VECTOR`]).
        if cells.contains(&IRQ_VECTOR)
            && let Some(m) = g.cells.get(&IRQ_VECTOR)
        {
            dirty.extend(m.iter().filter_map(|(t, _)| match t {
                Tok::Fn { key, .. } => Some(*key),
                _ => None,
            }));
        }
        if std::mem::take(&mut g.pile_changed) || !cells.is_empty() {
            let now = snap(&mut g);
            for (i, w) in [1u8, 2, 4].into_iter().enumerate() {
                // By the values the reads gave, a load takes only the
                // read's flags with its `Ref` ([`G::read_mem`]): what
                // looks into it is kept by that ([`Rd::refs`]).
                let other = |a: &Val, b: &Val| {
                    if g.by_value_read {
                        (a.mem, a.unk, a.lost_rom) != (b.mem, b.unk, b.lost_rom)
                    } else {
                        a != b
                    }
                };
                if other(&now[3 * i], &snapped[3 * i]) {
                    pile_w |= 1 << w;
                }
                if other(&now[3 * i + 1], &snapped[3 * i + 1]) {
                    cells_w |= 1 << w;
                }
                if other(&now[3 * i + 2], &snapped[3 * i + 2]) {
                    objs_w |= 1 << w;
                }
            }
            snapped = now;
        }
        let large = std::mem::take(&mut g.large.changed);
        let heap = std::mem::take(&mut g.heap_changed);
        // Every kept read brought up to date, so that one a function
        // looked into and none has since says whether it changed.
        if g.by_value_read {
            let kept: Vec<Tok> = g.ref_exp.keys().copied().collect();
            for r in kept {
                g.ref_val(r);
            }
        }
        let stale = |g: &G, r: &Rd| {
            r.refs
                .iter()
                .any(|(t, v)| g.ref_exp.get(t).is_none_or(|e| e.ver != *v))
        };
        if trace {
            // Why functions rerun for memory: each reason's functions, and
            // those of them nothing else reruns.
            let kind = |c: u32| match c {
                c if is_ram(c) => 4,
                STACK_CELL => 5,
                ANYOBJ_CELL => 6,
                c if is_in_cell(c) => 8,
                c if c >= HEAP_BASE => 9,
                _ => 7,
            };
            let names = [
                "pile",
                "every cell",
                "large",
                "heap",
                "a RAM cell",
                "the stacks' cell",
                "the any-object cell",
                "an object's cell",
                "an object's constant cells",
                "an allocation's cell",
                "every object's cell",
                "a kept read's value",
            ];
            let mut n = [(0usize, 0usize); 12];
            for (f, r) in &rds {
                let mut why = [false; 12];
                why[11] = stale(&g, r);
                why[0] = r.pile & pile_w != 0;
                why[1] = r.all_cells & cells_w != 0;
                why[2] = r.large && large;
                why[3] = r.heap && heap;
                why[10] = r.objs & objs_w != 0;
                for &(lo, hi) in &r.ranges {
                    for &c in cells.range(lo..=hi) {
                        why[kind(c)] = true;
                    }
                }
                for x in r.at.iter().filter(|x| at.contains(x)) {
                    why[kind(x.0)] = true;
                }
                let only = !dirty.contains(f) && why.iter().filter(|w| **w).count() == 1;
                for (i, w) in why.iter().enumerate() {
                    if *w {
                        n[i].0 += 1;
                        n[i].1 += only as usize;
                    }
                }
            }
            let mut by = [0usize; 10];
            for &c in &cells {
                by[kind(c)] += 1;
            }
            let line: Vec<String> = names
                .iter()
                .zip(n)
                .enumerate()
                .filter(|(_, (_, n))| n.0 > 0)
                .map(|(i, (k, n))| match i {
                    0..4 | 10 | 11 => format!("{k} {}/{}", n.0, n.1),
                    _ => format!("{k} {}/{} ({} stored to)", n.0, n.1, by[i]),
                })
                .collect();
            eprintln!(
                "fnref round {round} reruns for memory (functions/by that alone): {}",
                line.join(", ")
            );
        }
        dirty.extend(
            rds.iter()
                .filter(|(_, r)| {
                    r.pile & pile_w != 0
                        || r.all_cells & cells_w != 0
                        || r.objs & objs_w != 0
                        || r.large && large
                        || r.heap && heap
                        || r.at.iter().any(|x| at.contains(x))
                        || stale(&g, r)
                        || r.ranges
                            .iter()
                            .any(|&(lo, hi)| cells.range(lo..=hi).next().is_some())
                })
                .map(|(f, _)| *f),
        );
        if g.gains.is_some() {
            // The function tokens all entries and results hold between
            // them, their origins, and the largest set: does what they
            // hold grow, or only who holds it?
            let mut seen = std::collections::HashSet::new();
            let mut all: std::collections::HashSet<Tok, Fx> = Default::default();
            let mut big = 0usize;
            let mut nbig = 0usize;
            for v in g.entry.values().chain(g.ret.values()).flatten() {
                big = big.max(v.toks.len());
                if v.toks.len() > 4096 {
                    nbig += 1;
                }
                if seen.insert(std::rc::Rc::as_ptr(&v.toks.0)) {
                    all.extend(
                        v.toks
                            .iter()
                            .filter(|t| matches!(t, Tok::Fn { .. }))
                            .copied(),
                    );
                }
            }
            let origins: std::collections::HashSet<Origin> = all
                .iter()
                .filter_map(|t| match *t {
                    Tok::Fn { origin, .. } => Some(origin),
                    _ => None,
                })
                .collect();
            eprintln!(
                "gains round {round}: entries and results hold {} function tokens of {} origins between them; the largest set {big} tokens, {nbig} over 4096",
                all.len(),
                origins.len()
            );
            // By kind: the distinct tokens over all sets, and the tokens
            // summed over every register (what a round's joins walk).
            let kind = |t: &Tok| match t {
                Tok::Fn { .. } => 0,
                // (A pointer into a RAM object with the numbers.)
                Tok::Num(_) | Tok::Ram(_) | Tok::Heap(..) => 1,
                Tok::Rom(_) => 2,
                Tok::RomFrom(_) => 3,
                Tok::RomStep(..) | Tok::RomDull | Tok::RomAny => 4,
                Tok::Ref { src, .. } => match src {
                    Src::Pile(_) => 5,
                    // (An object's cells with every cell's.)
                    Src::Cells(_) | Src::Obj(..) | Src::Objs(_) => 6,
                    Src::RomFrom(..) => 7,
                    Src::Hull(..) | Src::Large => 8,
                },
            };
            let names = [
                "fn",
                "num",
                "rom",
                "from",
                "step",
                "ref-pile",
                "ref-cells",
                "ref-romfrom",
                "ref-hull",
            ];
            let mut distinct: Vec<std::collections::HashSet<Tok, Fx>> =
                (0..9).map(|_| Default::default()).collect();
            let mut summed = [0usize; 9];
            let mut xfs: std::collections::HashSet<Xf> = Default::default();
            let mut srcs: std::collections::HashSet<Src> = Default::default();
            let mut per: std::collections::HashMap<*const BTreeSet<Tok>, [usize; 9]> =
                Default::default();
            // Registers by how many non-function tokens they hold.
            let mut rest_hist = [0usize; 6];
            for v in g.entry.values().chain(g.ret.values()).flatten() {
                let p = std::rc::Rc::as_ptr(&v.toks.0);
                let c = per.entry(p).or_insert_with(|| {
                    let mut c = [0usize; 9];
                    for t in v.toks.iter() {
                        let k = kind(t);
                        c[k] += 1;
                        if k != 0 {
                            distinct[k].insert(*t);
                        }
                        if let Tok::Ref { src, xf } = t {
                            xfs.insert(*xf);
                            srcs.insert(*src);
                        }
                    }
                    c
                });
                for k in 0..9 {
                    summed[k] += c[k];
                }
                let rest: usize = c[1..].iter().sum();
                let b = match rest {
                    0 => 0,
                    1..=15 => 1,
                    16..=63 => 2,
                    64..=255 => 3,
                    256..=1023 => 4,
                    _ => 5,
                };
                rest_hist[b] += 1;
            }
            // The set with the most pointers "from here on": a sample of
            // them, each with how far its run of data goes.
            if let Some(v) = g
                .entry
                .values()
                .chain(g.ret.values())
                .flatten()
                .max_by_key(|v| v.toks.range(Tok::RomFrom(0)..Tok::RomDull).count())
            {
                let fr: Vec<u32> = v
                    .toks
                    .range(Tok::RomFrom(0)..Tok::RomDull)
                    .filter_map(|t| match *t {
                        Tok::RomFrom(a) | Tok::RomStep(a, _) => Some(a),
                        _ => None,
                    })
                    .collect();
                let step = (fr.len() / 60).max(1);
                let sample: Vec<String> = fr
                    .iter()
                    .step_by(step)
                    .map(|&a| format!("{a:#x}+{:#x}", bound(a) - a))
                    .collect();
                eprintln!(
                    "gains round {round}: the most pointers from here on in a set, {}: {}",
                    fr.len(),
                    sample.join(" ")
                );
            }
            let line: Vec<String> = (1..9)
                .map(|k| format!("{} {}/{}", names[k], distinct[k].len(), summed[k]))
                .collect();
            eprintln!(
                "gains round {round}: {} distinct sets; fn tokens summed {}; distinct/summed by kind: {}; {} Xfs, {} Srcs in Refs; registers by non-fn tokens (0, <16, <64, <256, <1024, more): {rest_hist:?}",
                per.len(),
                summed[0],
                line.join(", "),
                xfs.len(),
                srcs.len()
            );
        }
        if let Some(gains) = &mut g.gains {
            let mut by: Vec<_> = gains.iter().map(|(k, n)| (n.0, k.clone())).collect();
            by.sort_by(|a, b| b.cmp(a));
            for (n, k) in by.iter().take(40) {
                eprintln!("gains round {round}: {n} {k}");
            }
            gains.clear();
        }
        if trace {
            let kinds = |m: &Mem| {
                let mut k = [0usize; 5];
                for (t, _) in m {
                    k[match t {
                        Tok::Fn { .. } => 0,
                        Tok::Num(c) if is_ram(*c) => 1,
                        Tok::Ram(_) | Tok::Heap(..) => 2,
                        Tok::Ref { .. } => 3,
                        _ => 4,
                    }] += 1;
                }
                k
            };
            eprintln!(
                "fnref pile by kind (functions, RAM addresses, RAM objects, Refs, other): {:?}; kept Refs {}",
                kinds(&g.pile),
                g.ref_exp.len()
            );
            eprintln!(
                "fnref round {round}: {:?}; {} to rerun ({n_entry} entries changed, {n_rets} results; pile widths {pile_w:#x}, every-cell widths {cells_w:#x}, {} cells changed); pile {}, cells {}, taken {}, holes {}, large {}",
                t0.elapsed(),
                dirty.len(),
                cells.len(),
                g.pile.len(),
                g.cells.len(),
                g.taken.len(),
                g.flow.holes.len(),
                g.large.set.len()
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
        let widths: Vec<usize> = calls
            .iter()
            .filter(|(_, s)| s.complete)
            .map(|(_, s)| s.callees.len())
            .collect();
        let width = match widths.iter().max() {
            Some(m) => format!(
                " (on average {:.1} callees, the widest {m})",
                widths.iter().sum::<usize>() as f64 / widths.len() as f64
            ),
            None => String::new(),
        };
        writeln!(
            s,
            "function addresses (native/fnref.rs){}: {} function words in ROM ({pools} in literal pools, {} other data), {proven} proven, {} not: {unread} never read{} ({piled} of the {} not proven stored where the run can't place them, the heap's pile); {} code constants, {} proven; {} call_fn calls with a callee set, {complete} of them complete{width}; {} other jumps by value to a function",
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
