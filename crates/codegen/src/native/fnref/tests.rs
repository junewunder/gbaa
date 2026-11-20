//! The function-address flow on hand-built IR: each use section 4
//! allows (a call, a store and its load, an equality compare, the halves
//! split and joined, `| 1`, a vtable word, an argument), and the ones it
//! doesn't (arithmetic, an address, an ordered compare), each a hole.

use super::*;
use armv4t::sem::{Flag, Reg};

/// A function's key, Thumb.
const F: u32 = 0x0800_0201;
/// Another.
const H: u32 = 0x0800_0301;
/// An ARM function's key.
const A: u32 = 0x0800_0600;
/// A literal pool word holding `A`.
const POOL_A: u32 = 0x0800_040c;
/// A literal pool word holding `F`.
const POOL: u32 = 0x0800_0400;
/// A vtable in ROM data: `[0, 0, 0, F]`.
const VT: u32 = 0x0800_0500;
/// A RAM word.
const CELL: u32 = 0x0300_0100;

struct Img {
    words: BTreeMap<u32, u32>,
    /// Where ROM, and its data, ends.
    top: u32,
    /// Code in ROM, where a run of data ends (none: it runs to `top`).
    code: BTreeSet<u32>,
    /// More functions' keys.
    fns: BTreeSet<u32>,
    /// Instructions' bytes, `lo..hi` (none: no address is inside one).
    insts: Vec<(u32, u32)>,
}

impl Img {
    fn new() -> Img {
        Img {
            words: BTreeMap::from([(POOL, F), (VT + 12, F), (0x0800_0404, H), (POOL_A, A)]),
            top: 0x0800_1000,
            code: BTreeSet::new(),
            fns: BTreeSet::new(),
            insts: Vec::new(),
        }
    }
}

impl Image for Img {
    fn is_fn(&self, v: u32) -> bool {
        v == F || v == H || v == A || self.fns.contains(&v)
    }
    fn word(&self, a: u32) -> Option<u32> {
        (0x0800_0000..self.top)
            .contains(&a)
            .then(|| self.words.get(&(a & !3)).copied().unwrap_or(0))
    }
    fn bound(&self, a: u32) -> u32 {
        self.code.range(a + 1..).next().copied().unwrap_or(self.top)
    }
    fn code_at(&self, a: u32) -> bool {
        self.insts.iter().any(|&(lo, hi)| lo <= a && a < hi)
    }
}

/// A block's IR, built by hand.
#[derive(Default)]
struct B {
    insts: Vec<Inst>,
    pools: BTreeMap<usize, u32>,
}

impl B {
    fn op(&mut self, op: Op) -> ValueId {
        self.insts.push(Inst { op, guard: None });
        ValueId(self.insts.len() as u32 - 1)
    }
    fn k(&mut self, c: u32) -> ValueId {
        self.op(Op::Const(c))
    }
    /// `ldr rN, =F` from the pool word.
    fn pool(&mut self, w: u32, c: u32) -> ValueId {
        let v = self.op(Op::Pool(c));
        self.pools.insert(v.index(), w);
        v
    }
    fn get(&mut self, r: u8) -> ValueId {
        self.op(Op::GetReg(Reg(r)))
    }
    fn set(&mut self, r: u8, v: ValueId) {
        self.op(Op::SetReg(Reg(r), v));
    }
    fn bin(&mut self, op: BinOp, a: ValueId, b: ValueId) -> ValueId {
        self.op(Op::Bin(op, a, b))
    }
    fn shift(&mut self, kind: ShiftType, v: ValueId, n: u32) -> ValueId {
        let amount = self.k(n);
        self.op(Op::Shift { kind, v, amount })
    }
    /// Sets flag `f` to `v` and reads it (a branch on it would).
    fn flag(&mut self, f: Flag, v: ValueId) {
        self.op(Op::SetFlag(f, v));
        self.read_flag(f);
    }
    /// Reads flag `f` into r2.
    fn read_flag(&mut self, f: Flag) {
        let x = self.op(Op::GetFlag(f));
        let w = self.op(Op::BitToWord(x));
        self.set(2, w);
    }
    fn ld(&mut self, width: Width, addr: ValueId) -> ValueId {
        self.op(Op::Load { width, addr })
    }
    fn st(&mut self, width: Width, addr: ValueId, value: ValueId) {
        self.op(Op::Store { width, addr, value });
    }
    /// Ends the block with `edge`, jumping to `t` (a call's or jump's
    /// target, if the edge takes one).
    fn end(mut self, t: Option<ValueId>, edge: Edge) -> (Vec<Inst>, Out, BTreeMap<usize, u32>) {
        let t = match t {
            Some(t) => t,
            None => self.k(0),
        };
        self.op(Op::Exit(Exit::JumpExchange(t)));
        let out = Out {
            edge,
            via: None,
            at: 0x0800_0100,
            bios: None,
            takes: false,
        };
        (self.insts, out, self.pools)
    }
}

type Built = (u32, Vec<(u32, (Vec<Inst>, Out, BTreeMap<usize, u32>))>);

fn run(fns: &[Built]) -> Flow {
    run_with(fns, &BTreeMap::new())
}

thread_local! {
    /// [`run_ram`]'s: the RAM pointers' rules on whatever
    /// `GBAA_FNREF_RAM` says.
    static FORCE_RAM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`run`] with the RAM pointers' rules on ([`ram_on`],
/// `native/fnref-ram-switch`). The rest of the tests run as
/// `GBAA_FNREF_RAM` says: off, main's flow; on, 7a29e22e's.
fn run_ram(fns: &[Built]) -> Flow {
    FORCE_RAM.with(|c| c.set(true));
    let f = run(fns);
    FORCE_RAM.with(|c| c.set(false));
    f
}

/// With frames: function entry -> its frame.
fn run_with(fns: &[Built], frames: &BTreeMap<u32, Frame>) -> Flow {
    run_img(fns, frames, &Img::new())
}

fn run_img(fns: &[Built], frames: &BTreeMap<u32, Frame>, img: &Img) -> Flow {
    run_bounded(fns, frames, img, &BTreeMap::new())
}

/// The value-set analysis's bounds on a load, as [`FBlock::hulls`] has
/// them, and the stride if they start at a literal the function loads
/// ([`FBlock::bases`]).
type Bounds = ((u32, u32), Option<u32>);

/// [`run_img`] with the value-set bounds of some loads, by block and inst.
fn run_bounded(
    fns: &[Built],
    frames: &BTreeMap<u32, Frame>,
    img: &Img,
    bounds: &BTreeMap<(u32, usize), Bounds>,
) -> Flow {
    let of = |k: u32| bounds.range((k, 0)..=(k, usize::MAX));
    let ins: Vec<FnIn> = fns
        .iter()
        .map(|(e, bs)| FnIn {
            entry: *e,
            blocks: bs
                .iter()
                .map(|(k, (insts, out, pools))| {
                    (
                        *k,
                        FBlock {
                            insts,
                            exits: BTreeMap::from([(insts.len() - 1, out.clone())]),
                            pools: pools.clone(),
                            hulls: of(*k).map(|(&(_, i), &(h, _))| (i, h)).collect(),
                            bases: of(*k)
                                .filter_map(|(&(_, i), &(_, s))| Some((i, s?)))
                                .collect(),
                        },
                    )
                })
                .collect(),
            frame: frames.get(e),
            leaves: LEAVES.with(|c| c.borrow().get(e).map_or(0, |m| m.0)),
            reads: LEAVES.with(|c| c.borrow().get(e).map_or(0xf, |m| m.1)),
        })
        .collect();
    let ram = ram_env() || FORCE_RAM.with(|c| c.get());
    let allocs = ALLOCS.with(|c| c.borrow().clone());
    let live = LEAVES.with(|c| !c.borrow().is_empty());
    follow_live(&ins, img, &[], ram || !allocs.is_empty(), &allocs, live)
}

thread_local! {
    /// [`run_live`]'s: what each function leaves as it came.
    static LEAVES: std::cell::RefCell<BTreeMap<u32, (u8, u8)>> = const { std::cell::RefCell::new(BTreeMap::new()) };
}

/// [`run`] with a call taking back only what its callee may change
/// (`native/fnref-live-registers`): `leaves`, by function, the registers
/// of r0-r3 (bit `n` for rN) it leaves as they came, and those it reads.
fn run_live(fns: &[Built], leaves: &[(u32, u8, u8)]) -> Flow {
    LEAVES.with(|c| *c.borrow_mut() = leaves.iter().map(|&(k, l, r)| (k, (l, r))).collect());
    let f = run(fns);
    LEAVES.with(|c| c.borrow_mut().clear());
    f
}

thread_local! {
    /// [`run_heap`]'s allocators.
    static ALLOCS: std::cell::RefCell<BTreeSet<u32>> = const { std::cell::RefCell::new(BTreeSet::new()) };
}

/// [`run`] with `allocs` named as allocators, each direct call of one an
/// allocation of its own (`native/fnref-heap-sites`), on the RAM
/// pointers' rules.
fn run_heap(fns: &[Built], allocs: &[u32]) -> Flow {
    ALLOCS.with(|c| *c.borrow_mut() = allocs.iter().copied().collect());
    let f = run(fns);
    ALLOCS.with(|c| c.borrow_mut().clear());
    f
}

/// A function at `e` of two blocks: `b` ending in an indirect call
/// through `t` (if given) and a return.
fn calling(e: u32, b: B, t: ValueId) -> Built {
    let ret = e + 0x10;
    (
        e,
        vec![
            (e, b.end(Some(t), Edge::Call { target: None, ret })),
            (ret, B::default().end(None, Edge::Return)),
        ],
    )
}

fn site(f: &Flow) -> &Site {
    f.sites.get(&0x0800_0100).expect("the call's site")
}

#[test]
fn a_call_through_a_pool_word_is_proven_with_its_callee() {
    let mut b = B::default();
    let t = b.pool(POOL, F);
    let f = run(&[calling(0x0800_1001, b, t)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    assert!(site(&f).complete);
}

#[test]
fn a_store_and_its_load_are_followed() {
    // One function stores F to RAM; another loads it and calls it.
    let mut a = B::default();
    let p = a.pool(POOL, F);
    let c = a.k(CELL);
    a.st(Width::Word, c, p);
    let a = (0x0800_1001, vec![(0x0800_1001, a.end(None, Edge::Return))]);
    let mut b = B::default();
    let c = b.k(CELL);
    let t = b.ld(Width::Word, c);
    let f = run(&[a, calling(0x0800_2001, b, t)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    // RAM holds what was stored there and nothing else the run knows of.
    assert!(site(&f).complete);
}

#[test]
fn an_equality_compare_is_allowed() {
    // cmp r0, r1 (a + !b + 1), then Z.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let x = b.get(1);
    let n = b.op(Op::Not(x));
    let one = b.op(Op::Bit(true));
    let d = b.op(Op::Adc { a: p, b: n, c: one });
    let z = b.op(Op::IsZero(d));
    b.flag(Flag::Z, z);
    let f = run(&[(0x0800_1001, vec![(0x0800_1001, b.end(None, Edge::Return))])]);
    assert!(f.proven(POOL), "{f:?}");
}

#[test]
fn an_ordered_compare_is_a_hole() {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let x = b.get(1);
    let n = b.op(Op::Not(x));
    let one = b.op(Op::Bit(true));
    let c = b.op(Op::AdcCarry { a: p, b: n, c: one });
    b.flag(Flag::C, c);
    let f = run(&[(0x0800_1001, vec![(0x0800_1001, b.end(None, Edge::Return))])]);
    assert!(!f.proven(POOL));
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Ordered);
}

#[test]
fn halves_split_stored_and_joined_are_the_function_again() {
    // data[14] = func >> 16; data[15] = func; ... (data[14] << 16) | data[15]
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let hi = b.shift(ShiftType::Lsr, p, 16);
    let c = b.k(CELL);
    b.st(Width::Half, c, hi);
    let c2 = b.k(CELL + 2);
    b.st(Width::Half, c2, p);
    let c = b.k(CELL);
    let hi = b.ld(Width::Half, c);
    let c2 = b.k(CELL + 2);
    let lo = b.ld(Width::Half, c2);
    let up = b.shift(ShiftType::Lsl, hi, 16);
    let t = b.bin(BinOp::Or, up, lo);
    let f = run(&[calling(0x0800_1001, b, t)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    // `& 0xffff` is the bottom half too.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let m = b.k(0xffff);
    let lo = b.bin(BinOp::And, p, m);
    let c = b.k(CELL);
    b.st(Width::Half, c, lo);
    let f = run(&[(0x0800_1001, vec![(0x0800_1001, b.end(None, Edge::Return))])]);
    assert!(f.proven(POOL), "{f:?}");
}

#[test]
fn or_one_is_folded() {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let one = b.k(1);
    let t = b.bin(BinOp::Or, p, one);
    let f = run(&[calling(0x0800_1001, b, t)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
}

#[test]
fn a_vtable_word_read_through_a_rom_pointer_is_proven() {
    // ldr r0, =vtable; ldr r3, [r0, #12]; blx r3
    let mut b = B::default();
    let vt = b.pool(0x0800_0408, VT);
    let o = b.k(12);
    let a = b.bin(BinOp::Add, vt, o);
    let t = b.ld(Width::Word, a);
    let f = run(&[calling(0x0800_1001, b, t)]);
    assert!(f.proven(VT + 12), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    assert!(site(&f).complete);
}

#[test]
fn an_argument_reaches_the_callee_that_calls_it() {
    // f(g): r0 = F; bl g; and g calls r0.
    let mut a = B::default();
    let p = a.pool(POOL, F);
    a.set(0, p);
    let caller = (
        0x0800_1001,
        vec![
            (
                0x0800_1001,
                a.end(
                    None,
                    Edge::Call {
                        target: Some(0x0800_2001),
                        ret: 0x0800_1011,
                    },
                ),
            ),
            (0x0800_1011, B::default().end(None, Edge::Return)),
        ],
    );
    let mut b = B::default();
    let t = b.get(0);
    let f = run(&[caller, calling(0x0800_2001, b, t)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    assert!(site(&f).complete);
}

/// f: r8 = F; a call to g, which may take r8 (`Summary::may_take`); g
/// calls r8. `takes`: whether the call passes every register.
fn r8_to_a_taker(takes: bool) -> Flow {
    let mut a = B::default();
    let p = a.pool(POOL, F);
    a.set(8, p);
    let mut call = a.end(
        None,
        Edge::Call {
            target: Some(0x0800_2001),
            ret: 0x0800_1011,
        },
    );
    call.1.takes = takes;
    let caller = (
        0x0800_1001,
        vec![
            (0x0800_1001, call),
            (0x0800_1011, B::default().end(None, Edge::Return)),
        ],
    );
    let mut b = B::default();
    let t = b.get(8);
    run(&[caller, calling(0x0800_2001, b, t)])
}

#[test]
fn a_pointer_passed_in_r8_reaches_the_taker_that_calls_it() {
    let f = r8_to_a_taker(true);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    assert!(site(&f).complete);
    // A call that passes r0-r3 only leaves the taker's r8 empty.
    let f = r8_to_a_taker(false);
    assert!(!site(&f).callees.contains(&F), "{f:?}");
}

#[test]
fn arithmetic_on_a_function_address_is_a_hole() {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let four = b.k(4);
    let t = b.bin(BinOp::Add, p, four);
    let f = run(&[calling(0x0800_1001, b, t)]);
    assert!(!f.proven(POOL));
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Arithmetic);
    // The sum isn't a function: the call's set is empty, so not complete.
    assert!(site(&f).callees.is_empty());
    assert!(!site(&f).complete);
}

#[test]
fn a_function_address_used_as_an_address_is_a_hole() {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    b.ld(Width::Byte, p);
    let f = run(&[(0x0800_1001, vec![(0x0800_1001, b.end(None, Edge::Return))])]);
    assert!(!f.proven(POOL));
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Address);
}

#[test]
fn a_hole_after_ram_reaches_the_word_that_was_stored() {
    // F stored to RAM in one function, loaded and incremented in another.
    let mut a = B::default();
    let p = a.pool(POOL, F);
    let c = a.k(CELL);
    a.st(Width::Word, c, p);
    let a = (0x0800_1001, vec![(0x0800_1001, a.end(None, Edge::Return))]);
    let mut b = B::default();
    let c = b.k(CELL);
    let x = b.ld(Width::Word, c);
    let one = b.k(2);
    b.bin(BinOp::Add, x, one);
    let b = (0x0800_2001, vec![(0x0800_2001, b.end(None, Edge::Return))]);
    let f = run(&[a, b]);
    assert!(!f.proven(POOL));
}

#[test]
fn a_word_nothing_reads_is_not_proven() {
    let f = run(&[(
        0x0800_1001,
        vec![(0x0800_1001, B::default().end(None, Edge::Return))],
    )]);
    assert!(!f.proven(POOL));
    assert!(!f.read.contains(&POOL));
}

#[test]
fn a_target_loaded_from_an_unknown_place_is_incomplete() {
    // A parameter of a root function (nothing calls it directly).
    let mut b = B::default();
    let t = b.get(0);
    let f = run(&[calling(0x0800_1001, b, t)]);
    assert!(!site(&f).complete);
}

#[test]
fn a_table_read_past_what_the_run_reads_is_incomplete() {
    // ldr r0, =VT; ldr r3, [r0, r1] with r1 unknown, the data running on
    // past what one read reads to another function's word.
    let mut img = Img::new();
    img.top = VT + ROM_READ + 0x1000;
    img.words.insert(VT + ROM_READ + 0x800, H);
    let mut b = B::default();
    let vt = b.pool(0x0800_0408, VT);
    let i = b.get(1);
    let a = b.bin(BinOp::Add, vt, i);
    let t = b.ld(Width::Word, a);
    let f = run_img(&[calling(0x0800_1001, b, t)], &BTreeMap::new(), &img);
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    assert!(!site(&f).complete, "{f:?}");
}

/// `ldr r0, =VT; ldr r3, [r0, r1]` with r1 unknown, calling r3.
fn calling_vt_entry() -> Built {
    let mut b = B::default();
    let vt = b.pool(0x0800_0408, VT);
    let i = b.get(1);
    let a = b.bin(BinOp::Add, vt, i);
    let t = b.ld(Width::Word, a);
    calling(0x0800_1001, b, t)
}

#[test]
fn a_table_read_stops_at_the_next_object() {
    // A second object at VT + 0x40 (a ROM word points at it) holds H: a
    // read by an unknown index from VT on stays in VT's object
    // (native/fnref-rom-read-bound).
    let mut img = Img::new();
    img.words.insert(0x0800_0420, VT + 0x40);
    img.words.insert(VT + 0x44, H);
    let f = run_img(&[calling_vt_entry()], &BTreeMap::new(), &img);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
    // Without the pointer it runs to the next code, taking in H.
    img.words.remove(&0x0800_0420);
    let f = run_img(&[calling_vt_entry()], &BTreeMap::new(), &img);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

#[test]
fn a_pointer_into_a_table_of_handlers_doesnt_cut_it() {
    // VT's handlers F, H, F at 12, 16 and 20, and a ROM word pointing at
    // VT + 16 (as `&TABLE[4..]` would): not an object's start.
    let mut img = Img::new();
    img.words.insert(VT + 16, H);
    img.words.insert(VT + 20, F);
    img.words.insert(0x0800_0420, VT + 16);
    let f = run_img(&[calling_vt_entry()], &BTreeMap::new(), &img);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

/// A function at `e` that reads the table at `t` by an unknown index
/// (`ldr r0, =t` from the pool word `w`; `ldr r0, [r0, r1]`) and returns,
/// and its load's bounds: 256 entries of `stride` bytes from `t`, a table
/// read from its start.
fn reading(e: u32, w: u32, t: u32, stride: u32) -> (Built, ((u32, usize), Bounds)) {
    let mut b = B::default();
    let p = b.pool(w, t);
    let i = b.get(1);
    let a = b.bin(BinOp::Add, p, i);
    let x = b.ld(Width::Word, a);
    b.set(0, x);
    let bounds = ((e, x.index()), ((t, t + 255 * stride), Some(stride)));
    ((e, vec![(e, b.end(None, Edge::Return))]), bounds)
}

/// `ldr r0, =VT; ldr r3, [r0, r1]` with r1 unknown, calling r3, and that
/// load's bounds: 256 entries of `stride` bytes from VT.
fn calling_vt_bounded(stride: u32) -> (Built, ((u32, usize), Bounds)) {
    let (e, b) = (0x0800_1001, calling_vt_entry());
    let i = b.1[0]
        .1
        .0
        .iter()
        .position(|x| matches!(x.op, Op::Load { .. }));
    let bounds = ((e, i.unwrap()), ((VT, VT + 255 * stride), Some(stride)));
    (b, bounds)
}

#[test]
fn a_table_ends_where_one_the_code_indexes_from_starts() {
    // Two tables of handlers back to back, as C lays them: F, F, F, F
    // from VT and H, H from VT + 16. A run of function words, so no
    // object start ends the first; but other code indexes from VT + 16
    // (native/fnref-table-ends).
    let mut img = Img::new();
    for k in 0..4 {
        img.words.insert(VT + 4 * k, F);
    }
    img.words.insert(VT + 16, H);
    img.words.insert(VT + 20, H);
    img.words.insert(0x0800_0420, VT + 16);
    let (call, b1) = calling_vt_bounded(4);
    let (other, b2) = reading(0x0800_2001, 0x0800_0420, VT + 16, 4);
    let f = run_bounded(
        &[call.clone(), other.clone()],
        &BTreeMap::new(),
        &img,
        &BTreeMap::from([b1, b2]),
    );
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
    // A pointer to it that nothing indexes from doesn't end the first.
    let f = run_bounded(
        &[call, other],
        &BTreeMap::new(),
        &img,
        &BTreeMap::from([b1]),
    );
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

#[test]
fn a_pointer_into_the_middle_of_a_table_still_reads_to_its_end() {
    // Records of 8 bytes from VT, a handler and a number each: F, 7 and
    // H, 7 (and VT + 12's F). Code reads the handlers from VT, and the
    // numbers from VT + 4 (`ldr r0, =VT+4`, a struct table read by one
    // field): VT + 4 is in the first record, not a table of its own.
    let mut img = Img::new();
    img.words.insert(VT, F);
    img.words.insert(VT + 4, 7);
    img.words.insert(VT + 8, H);
    img.words.insert(0x0800_0420, VT + 4);
    let (call, b1) = calling_vt_bounded(8);
    let (other, b2) = reading(0x0800_2001, 0x0800_0420, VT + 4, 8);
    let f = run_bounded(
        &[call, other],
        &BTreeMap::new(),
        &img,
        &BTreeMap::from([b1, b2]),
    );
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

#[test]
fn bounds_from_a_table_s_start_end_with_its_object() {
    // A load whose address the run can't place at all (r1, unknown),
    // which the value sets bound to 2 KB from VT, a literal its function
    // loads: VT's bytes, not the 2 KB. VT + 12's F is in it; H at
    // VT + 0x40 is past VT + 0x20, where other code indexes from.
    let mut img = Img::new();
    img.words.insert(VT + 0x40, H);
    let mut b = B::default();
    b.pool(0x0800_0408, VT);
    let a = b.get(1);
    let t = b.ld(Width::Word, a);
    let e = 0x0800_1001;
    let call = calling(e, b, t);
    let hull = ((e, t.index()), ((VT, VT + 0x7ff), Some(1)));
    let (other, b2) = reading(0x0800_2001, 0x0800_0424, VT + 0x20, 4);
    let f = run_bounded(
        &[call.clone(), other.clone()],
        &BTreeMap::new(),
        &img,
        &BTreeMap::from([hull, b2]),
    );
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
    // Bounds that don't start at a literal it loads read all 2 KB.
    let hull = ((e, t.index()), ((VT, VT + 0x7ff), None));
    let f = run_bounded(
        &[call, other],
        &BTreeMap::new(),
        &img,
        &BTreeMap::from([hull, b2]),
    );
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

/// `ldr rN, =base` (a RAM literal) plus `rI << sh`, plus `k`: a pointer
/// into `base`'s object ([`ram_indexed`] finds `base` a start).
fn indexed(b: &mut B, w: u32, base: u32, i: u8, sh: u32, k: u32) -> ValueId {
    let lit = b.pool(w, base);
    let r = b.get(i);
    let x = b.shift(ShiftType::Lsl, r, sh);
    let a = b.bin(BinOp::Add, x, lit);
    let k = b.k(k);
    b.bin(BinOp::Add, a, k)
}

/// Two arrays of records in RAM, each indexed from its start: A1 of 24
/// bytes, A2 of 8 (`native/fnref-ram-pointers`).
const A1: u32 = 0x0300_0200;
const A2: u32 = 0x0300_0400;

/// One function makes `&A1[r1]` and stores it to CELL (a scheduler's
/// current task), and stores H in A2[r2]; another loads CELL back and
/// stores F through it, 4 on.
fn make_and_set() -> [Built; 2] {
    let e = 0x0800_1001;
    let mut a = B::default();
    let lit = a.pool(0x0800_0410, A1);
    let i = a.get(1);
    let n = a.k(24);
    let x = a.bin(BinOp::Mul, i, n);
    let t = a.bin(BinOp::Add, lit, x);
    let c = a.k(CELL);
    a.st(Width::Word, c, t);
    let y = indexed(&mut a, 0x0800_0414, A2, 2, 3, 0);
    let h = a.pool(0x0800_0404, H);
    a.st(Width::Word, y, h);
    let e2 = 0x0800_3001;
    let mut b = B::default();
    let c = b.k(CELL);
    let t = b.ld(Width::Word, c);
    let four = b.k(4);
    let x = b.bin(BinOp::Add, t, four);
    let p = b.pool(POOL, F);
    b.st(Width::Word, x, p);
    [
        (e, vec![(e, a.end(None, Edge::Return))]),
        (e2, vec![(e2, b.end(None, Edge::Return))]),
    ]
}

#[test]
fn a_pointer_into_a_ram_array_stays_in_it() {
    // A call through CELL's pointer, 4 on: F alone, H kept in A2.
    let mut b = B::default();
    let c = b.k(CELL);
    let t = b.ld(Width::Word, c);
    let four = b.k(4);
    let x = b.bin(BinOp::Add, t, four);
    let g = b.ld(Width::Word, x);
    let [make, set] = make_and_set();
    let f = run_ram(&[make, set, calling(0x0800_2001, b, g)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
    assert!(!f.piled.contains(&Origin::Word(POOL)), "{f:?}");
    // And through A1[r3] + 4, from its literal: F alone too.
    let mut b = B::default();
    let x = indexed(&mut b, 0x0800_0418, A1, 3, 3, 4);
    let g = b.ld(Width::Word, x);
    let [make, set] = make_and_set();
    let f = run_ram(&[make, set, calling(0x0800_2001, b, g)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    // And through A2[r3]: H alone.
    let mut b = B::default();
    let x = indexed(&mut b, 0x0800_0418, A2, 3, 3, 0);
    let g = b.ld(Width::Word, x);
    let [make, set] = make_and_set();
    let f = run_ram(&[make, set, calling(0x0800_2001, b, g)]);
    assert_eq!(site(&f).callees, BTreeSet::from([H]), "{f:?}");
}

#[test]
fn a_loose_function_s_parameter_reads_the_cells() {
    // F stored to CELL, H in A2[r2]; a function nothing calls directly
    // (an `&mut dyn` closure's method, through a vtable the run never
    // read) calls through what its r0 points at
    // (`native/fnref-ram-loose-params`): every cell, the stacks' and the
    // pile, but no RAM object's, so F alone, complete. Before, its r0 was
    // an unknown, no RAM address: the load read nothing, the call
    // incomplete.
    let e = 0x0800_1001;
    let mut a = B::default();
    let p = a.pool(POOL, F);
    let c = a.k(CELL);
    a.st(Width::Word, c, p);
    let y = indexed(&mut a, 0x0800_0414, A2, 2, 3, 0);
    let h = a.pool(0x0800_0404, H);
    a.st(Width::Word, y, h);
    let make = (e, vec![(e, a.end(None, Edge::Return))]);
    let mut b = B::default();
    let r0 = b.get(0);
    let t = b.ld(Width::Word, r0);
    let f = run_ram(&[make, calling(0x0800_2001, b, t)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
}

#[test]
fn a_ram_pointer_dropped_is_read_everywhere() {
    // H stored in A2[r2]; F through a pointer the run dropped (which
    // may be anywhere): a load through A2[r3] reads both.
    let e = 0x0800_1001;
    let mut a = B::default();
    let y = indexed(&mut a, 0x0800_0414, A2, 2, 3, 0);
    let h = a.pool(0x0800_0404, H);
    a.st(Width::Word, y, h);
    let (x, _) = dropped(&mut a);
    let p = a.pool(POOL, F);
    a.st(Width::Word, x, p);
    let store = (e, vec![(e, a.end(None, Edge::Return))]);
    let mut b = B::default();
    let x = indexed(&mut b, 0x0800_0418, A2, 3, 3, 0);
    let g = b.ld(Width::Word, x);
    let f = run_ram(&[store, calling(0x0800_2001, b, g)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

#[test]
fn a_bios_copy_reads_its_length() {
    let n = |m: u32| Val::of(Tok::Num(m));
    // CpuSet: halfwords, words with bit 26, one unit with the fill bit.
    assert_eq!(copy_len(false, &n(92)), Some(184));
    assert_eq!(copy_len(false, &n(1 << 26 | 92)), Some(368));
    assert_eq!(copy_len(false, &n(1 << 24 | 1 << 26 | 92)), Some(4));
    // CpuFastSet: words, in blocks of eight.
    assert_eq!(copy_len(true, &n(9)), Some(64));
    // The longest of several; none for an unknown.
    let mut v = n(8);
    v.join(&n(16));
    assert_eq!(copy_len(false, &v), Some(32));
    assert_eq!(copy_len(false, &Val::unk()), None);
}

#[test]
fn a_rom_pointer_past_the_next_code_is_not_taken_in_by_one_from_below() {
    // r0 is VT + r1 (r1 unknown: "from VT on", read up to the code at
    // 0x0800_0580) or a second vtable past that code, whose word is H.
    // Widening must keep the second: a read from VT on never reaches it.
    let vt2 = 0x0800_0700;
    let mut img = Img::new();
    img.code.insert(0x0800_0580);
    img.words.insert(vt2, H);
    let mut b = B::default();
    let vt = b.pool(0x0800_0408, VT);
    let i = b.get(1);
    let a = b.bin(BinOp::Add, vt, i);
    let p2 = b.pool(0x0800_0410, vt2);
    let sel = b.op(Op::Select { c: i, a, b: p2 });
    let t = b.ld(Width::Word, sel);
    let f = run_img(&[calling(0x0800_1001, b, t)], &BTreeMap::new(), &img);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
}

#[test]
fn widening_takes_in_rom_pointers_only_up_to_the_next_code() {
    let mut img = Img::new();
    img.code.insert(0x0800_0580);
    let _code = CodeGuard::set(&img);
    // From VT on takes in a pointer above it before the code, not past it.
    let mut v = Val::of(Tok::RomFrom(VT));
    v.join(&Val::of(Tok::Rom(VT + 0x10)));
    v.join(&Val::of(Tok::Rom(0x0800_0700)));
    let want: Toks = [Tok::RomFrom(VT), Tok::Rom(0x0800_0700)]
        .into_iter()
        .collect();
    assert_eq!(v.toks, want);
    // One from past the code takes in those above it there.
    v.join(&Val::of(Tok::RomFrom(0x0800_0600)));
    let want: Toks = [Tok::RomFrom(VT), Tok::RomFrom(0x0800_0600)]
        .into_iter()
        .collect();
    assert_eq!(v.toks, want);
    // Too many exact pointers: one "from the lowest on" per run of data.
    let mut v = Val::default();
    v.join(&Val::of(Tok::Rom(0x0800_0640)));
    v.join(&Val::of(Tok::Rom(0x0800_0700)));
    for k in 0..=ROMS as u32 {
        v.join(&Val::of(Tok::Rom(VT + 4 * k)));
    }
    let want: Toks = [Tok::RomFrom(VT), Tok::RomFrom(0x0800_0640)]
        .into_iter()
        .collect();
    assert_eq!(v.toks, want);
}

#[test]
fn a_rom_pointer_minus_an_unknown_is_incomplete() {
    // A table walked down from its end: the run gives the pointer up, so
    // a call through what it loads can't be complete, though it holds F.
    let mut b = B::default();
    let vt = b.pool(0x0800_0408, VT + 0x10);
    let i = b.get(1);
    let a = b.bin(BinOp::Sub, vt, i);
    let x = b.ld(Width::Word, a);
    let p = b.pool(POOL, F);
    let t = b.op(Op::Select { c: x, a: x, b: p });
    let f = run(&[calling(0x0800_1001, b, t)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    assert!(!site(&f).complete, "{f:?}");
}

#[test]
fn a_lost_value_through_a_local_slot_by_halves_is_incomplete() {
    use super::super::frame::{Place, Slot};
    // The pointer-less-unknown value of the test above, split, its halves
    // kept in a local (sp - 8, never escaping), loaded and joined.
    let mut b = B::default();
    let vt = b.pool(0x0800_0408, VT + 0x10);
    let i = b.get(1);
    let a = b.bin(BinOp::Sub, vt, i);
    let x = b.ld(Width::Word, a);
    let p = b.pool(POOL, F);
    let v = b.op(Op::Select { c: x, a: x, b: p });
    let hi = b.shift(ShiftType::Lsr, v, 16);
    let sp = b.get(13);
    b.st(Width::Half, sp, hi);
    let st_hi = b.insts.len() - 1;
    b.st(Width::Half, sp, v);
    let st_lo = b.insts.len() - 1;
    let hi = b.ld(Width::Half, sp);
    let ld_hi = b.insts.len() - 1;
    let lo = b.ld(Width::Half, sp);
    let ld_lo = b.insts.len() - 1;
    let up = b.shift(ShiftType::Lsl, hi, 16);
    let t = b.bin(BinOp::Or, up, lo);
    let e = 0x0800_1001;
    let frames = BTreeMap::from([(
        e,
        Frame {
            slots: vec![Slot {
                lo: -8,
                hi: -4,
                saved: false,
                escapes: 0,
            }],
            places: BTreeMap::from([
                ((e, st_hi), Place::Exact(-8, 2)),
                ((e, st_lo), Place::Exact(-6, 2)),
                ((e, ld_hi), Place::Exact(-8, 2)),
                ((e, ld_lo), Place::Exact(-6, 2)),
            ]),
            ..Frame::default()
        },
    )]);
    let f = run_with(&[calling(e, b, t)], &frames);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(!site(&f).complete, "{f:?}");
}

/// A one-block function returning at once.
fn just(e: u32, b: B) -> Built {
    (e, vec![(e, b.end(None, Edge::Return))])
}

#[test]
fn or_one_on_an_arm_address_is_a_hole() {
    // `| 1` makes an ARM function's address another one's (a Thumb one).
    let mut b = B::default();
    let p = b.pool(POOL_A, A);
    let one = b.k(1);
    b.bin(BinOp::Or, p, one);
    let f = run(&[just(0x0800_1001, b)]);
    assert!(!f.proven(POOL_A));
    assert_eq!(f.holes[&Origin::Word(POOL_A)].1, Why::Arithmetic);
}

#[test]
fn a_half_joined_with_an_unknown_is_a_hole() {
    // (F & 0xffff) | r5: r5 isn't F's top half.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let m = b.k(0xffff);
    let lo = b.bin(BinOp::And, p, m);
    let r5 = b.get(5);
    b.bin(BinOp::Or, lo, r5);
    let f = run(&[just(0x0800_1001, b)]);
    assert!(!f.proven(POOL), "{f:?}");
}

#[test]
fn a_half_compared_is_a_hole() {
    // (F >> 16) == 0: the top half of a dense ID is always 0.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let hi = b.shift(ShiftType::Lsr, p, 16);
    b.op(Op::IsZero(hi));
    let f = run(&[just(0x0800_1001, b)]);
    assert!(!f.proven(POOL), "{f:?}");
    // And so is a whole address compared with a small constant (it could
    // be a dense ID).
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let three = b.k(3);
    let d = b.bin(BinOp::Sub, p, three);
    b.op(Op::IsZero(d));
    let f = run(&[just(0x0800_1001, b)]);
    assert!(!f.proven(POOL), "{f:?}");
}

#[test]
fn a_stack_argument_reaches_the_callee() {
    use super::super::frame::{Kind, Place, Slot};
    // The caller stores F to its outgoing argument slot (sp - 8, the
    // callee's sp + 0) and calls g; g loads it and adds 4.
    let mut a = B::default();
    let p = a.pool(POOL, F);
    let sp = a.get(13);
    a.st(Width::Word, sp, p);
    let store_at = a.insts.len() - 1;
    let caller = (
        0x0800_1001,
        vec![
            (
                0x0800_1001,
                a.end(
                    None,
                    Edge::Call {
                        target: Some(0x0800_2001),
                        ret: 0x0800_1011,
                    },
                ),
            ),
            (0x0800_1011, B::default().end(None, Edge::Return)),
        ],
    );
    let mut b = B::default();
    let sp = b.get(13);
    let x = b.ld(Width::Word, sp);
    let load_at = b.insts.len() - 1;
    let four = b.k(4);
    b.bin(BinOp::Add, x, four);
    let callee = just(0x0800_2001, b);
    let frames = BTreeMap::from([
        (
            0x0800_1001,
            Frame {
                slots: vec![Slot {
                    lo: -8,
                    hi: -4,
                    saved: false,
                    escapes: Kind::Args as u8,
                }],
                places: BTreeMap::from([((0x0800_1001, store_at), Place::Exact(-8, 4))]),
                ..Frame::default()
            },
        ),
        (
            0x0800_2001,
            Frame {
                places: BTreeMap::from([((0x0800_2001, load_at), Place::Exact(0, 4))]),
                ..Frame::default()
            },
        ),
    ]);
    let f = run_with(&[caller, callee], &frames);
    assert!(!f.proven(POOL), "{f:?}");
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Arithmetic);
}

#[test]
fn a_store_outside_ram_is_lost() {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let io = b.k(0x0400_00d4);
    b.st(Width::Word, io, p);
    let f = run(&[just(0x0800_1001, b)]);
    assert!(!f.proven(POOL));
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Outside);
}

#[test]
fn callees_that_shared_an_entry_share_what_a_call_by_value_gives() {
    let fn_tok = |key| Tok::Fn {
        key,
        origin: Origin::Word(key),
        form: Form::Full,
    };
    let shared = Val {
        toks: [fn_tok(1), fn_tok(2)].into_iter().collect(),
        ..Val::default()
    };
    let own = Val::of(fn_tok(3));
    let mut g = G::default();
    for k in [10, 11] {
        g.entry.insert(k, vec![shared.clone(); 16]);
    }
    g.entry.insert(12, vec![own; 16]);
    let args = vec![Val::of(fn_tok(4)); 4];
    g.give_all(&BTreeSet::from([10, 11, 12, 13]), &args);
    let r0 = |k: u32| g.entry[&k][0].clone();
    assert!(std::rc::Rc::ptr_eq(&r0(10).toks.0, &r0(11).toks.0));
    let toks = |v: Val| v.toks.iter().copied().collect::<Vec<_>>();
    assert_eq!(toks(r0(10)), [fn_tok(1), fn_tok(2), fn_tok(4)]);
    assert_eq!(toks(r0(12)), [fn_tok(3), fn_tok(4)]);
    assert_eq!(toks(r0(13)), [fn_tok(4)]);
    // Registers past the arguments are as they were.
    assert_eq!(toks(g.entry[&10][4].clone()), [fn_tok(1), fn_tok(2)]);
    assert_eq!(g.dirty_entry, BTreeSet::from([10, 11, 12, 13]));
    assert!(g.changed);
    // The same again changes nothing.
    g.changed = false;
    g.give_all(&BTreeSet::from([10, 11, 12, 13]), &args);
    assert!(!g.changed);
}

/// A kept expansion holds the cap of all it has read, whatever order
/// memory gained it in: capped a store at a time, the pointers below a
/// widened one stayed exact, while the capped read of memory, which a
/// function is rerun by, already took a later one in.
#[test]
fn a_kept_expansion_caps_all_it_read_together() {
    let rom = |n: u32| Tok::Rom(0x0800_0800 + 4 * n);
    let fn_tok = |key| Tok::Fn {
        key,
        origin: Origin::Code(key, 0),
        form: Form::Full,
    };
    let of = |ts: &[Tok]| Val {
        toks: ts.iter().copied().collect(),
        ..Val::default()
    };
    // Seventeen pointers widen to "from the lowest on"; then two below it.
    let first: Vec<Tok> = (5..22).map(rom).chain([fn_tok(1)]).collect();
    let mut e = Exp::default();
    exp_merge(&mut e, of(&first));
    assert!(e.v.toks.contains(&Tok::RomFrom(0x0800_0800 + 4 * 5)));
    exp_merge(&mut e, of(&[rom(1)]));
    exp_merge(&mut e, of(&[rom(3), fn_tok(2)]));
    let mut whole = of(&first);
    whole.union(&of(&[rom(1), rom(3), fn_tok(2)]));
    whole.cap();
    assert_eq!(e.v, whole);
    let toks: Vec<Tok> = e.v.toks.iter().copied().collect();
    assert_eq!(toks, [fn_tok(1), fn_tok(2), Tok::RomFrom(0x0800_0800 + 4)]);
}

/// Where [`swi_then`]'s BIOS call is.
const SWI_AT: u32 = 0x0800_0080;

/// A function at `e`: `b`, BIOS call `n`, then `then` ending in an
/// indirect call through `t` (the call [`site`] finds) and a return.
fn swi_then(e: u32, b: B, n: u8, then: B, t: ValueId) -> Built {
    let (next, ret) = (e + 0x10, e + 0x20);
    let mut first = b.end(None, Edge::Swi(next));
    (first.1.bios, first.1.at) = (Some(n), SWI_AT);
    (
        e,
        vec![
            (e, first),
            (next, then.end(Some(t), Edge::Call { target: None, ret })),
            (ret, B::default().end(None, Edge::Return)),
        ],
    )
}

/// F from its pool word in register `r`, BIOS call `n`, then a call
/// through `r`.
fn in_reg_at_swi(r: u8, n: u8) -> Flow {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    b.set(r, p);
    let mut then = B::default();
    let t = then.get(r);
    run(&[swi_then(0x0800_1001, b, n, then, t)])
}

#[test]
fn an_address_left_where_a_bios_call_doesnt_read_stays_proven_and_followed() {
    // VBlankIntrWait takes no arguments and leaves r2 and r3 as they came.
    for r in [2, 3] {
        let f = in_reg_at_swi(r, 0x05);
        assert!(f.proven(POOL), "r{r}: {f:?}");
        assert_eq!(site(&f).callees, BTreeSet::from([F]), "r{r}");
        assert!(site(&f).complete, "r{r}");
    }
    // It leaves its flags in r0 and 1 in r1: no hole, and nothing of F
    // after it.
    for r in [0, 1] {
        let f = in_reg_at_swi(r, 0x05);
        assert!(f.proven(POOL), "r{r}: {f:?}");
        assert!(site(&f).callees.is_empty(), "r{r}");
        assert!(!site(&f).complete, "r{r}");
    }
    // Halt reads and sets none.
    for r in 0..4 {
        let f = in_reg_at_swi(r, 0x02);
        assert!(f.proven(POOL), "r{r}: {f:?}");
        assert_eq!(site(&f).callees, BTreeSet::from([F]), "r{r}");
    }
}

#[test]
fn an_address_where_a_bios_call_reads_is_a_hole() {
    // CpuSet: source, destination, length and mode.
    for r in 0..3 {
        let f = in_reg_at_swi(r, 0x0b);
        assert!(!f.proven(POOL), "r{r}");
        assert_eq!(f.holes[&Origin::Word(POOL)], (SWI_AT, Why::Bios), "r{r}");
    }
    // r3 it doesn't read, and leaves 0x170 there, or r3 as it came (a
    // source in the BIOS): F may still be called through it.
    let f = in_reg_at_swi(3, 0x0b);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    // Div reads r0 and r1, and sets r3 too: r2 alone is as it came.
    for (r, hole, kept) in [(0, true, false), (2, false, true), (3, false, false)] {
        let f = in_reg_at_swi(r, 0x06);
        assert_eq!(f.proven(POOL), !hole, "r{r}");
        assert_eq!(site(&f).callees.contains(&F), kept, "r{r}");
    }
}

#[test]
fn a_bios_call_with_no_rule_holes_all_four() {
    // ArcTan (the play build has it, a native build doesn't), a number
    // that is no call, and an exit that names none.
    for n in [Some(0x09), Some(0xff), None] {
        for r in 0..4 {
            let mut b = B::default();
            let p = b.pool(POOL, F);
            b.set(r, p);
            let mut then = B::default();
            let t = then.get(r);
            let mut fun = swi_then(0x0800_1001, b, 0, then, t);
            fun.1[0].1.1.bios = n;
            let f = run(&[fun]);
            assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Bios, "{n:?} r{r}");
            assert!(site(&f).callees.is_empty(), "{n:?} r{r}");
        }
    }
}

#[test]
fn every_call_a_native_build_makes_has_a_rule_and_no_other() {
    let none = bios_regs(None);
    assert_eq!((none.reads, none.sets), (0xf, 0xf));
    for n in 0..=0xff {
        let has = bios_regs(Some(n)) != none;
        assert_eq!(has, analysis::holes::NATIVE_BIOS.contains(&n), "{n:#04x}");
        // Set or maybe set, not both.
        let r = bios_regs(Some(n));
        assert_eq!(r.sets & r.may_set, 0, "{n:#04x}");
    }
}

#[test]
fn cpu_fast_set_leaves_a_word_it_copied_in_r3() {
    // Eight words from VT (its fourth is F) to RAM: r3 is the last
    // block's first word as the BIOS leaves it, so one of the eight.
    let copy = |b: &mut B, from: ValueId| {
        b.set(0, from);
        let to = b.k(0x0200_0000);
        b.set(1, to);
        let n = b.k(8);
        b.set(2, n);
    };
    let mut b = B::default();
    let vt = b.pool(0x0800_0408, VT);
    copy(&mut b, vt);
    let mut then = B::default();
    let t = then.get(3);
    let f = run(&[swi_then(0x0800_1001, b, 0x0c, then, t)]);
    assert!(f.proven(VT + 12), "{f:?}");
    assert!(site(&f).callees.contains(&F), "{f:?}");
    // Arithmetic on it is arithmetic on F.
    let mut b = B::default();
    let vt = b.pool(0x0800_0408, VT);
    copy(&mut b, vt);
    let mut then = B::default();
    let x = then.get(3);
    let four = then.k(4);
    let t = then.bin(BinOp::Add, x, four);
    let f = run(&[swi_then(0x0800_1001, b, 0x0c, then, t)]);
    assert_eq!(f.holes[&Origin::Word(VT + 12)].1, Why::Arithmetic, "{f:?}");
    // From RAM, a word some store left there.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let c = b.k(CELL);
    b.st(Width::Word, c, p);
    copy(&mut b, c);
    let mut then = B::default();
    let t = then.get(3);
    let f = run(&[swi_then(0x0800_1001, b, 0x0c, then, t)]);
    assert!(site(&f).callees.contains(&F), "{f:?}");
}

/// F stored at `CELL`, then BIOS call `n` with `src` in r0, `CELL2` in
/// r1 and `len` in r2; after it, `then` makes the target from the word
/// loaded at `CELL2`.
fn copied(n: u8, src: Option<u32>, len: u32, then: impl Fn(&mut B, ValueId) -> ValueId) -> Flow {
    const CELL2: u32 = 0x0200_0400;
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let c = b.k(CELL);
    b.st(Width::Word, c, p);
    if let Some(src) = src {
        let s = b.k(src);
        b.set(0, s);
    }
    let to = b.k(CELL2);
    b.set(1, to);
    let len = b.k(len);
    b.set(2, len);
    let mut after = B::default();
    let c2 = after.k(CELL2);
    let x = after.ld(Width::Word, c2);
    let t = then(&mut after, x);
    run(&[swi_then(0x0800_1001, b, n, after, t)])
}

#[test]
fn a_bios_copy_out_of_ram_carries_its_addresses() {
    let word = 1 << 26 | 1;
    let add = |b: &mut B, x: ValueId| {
        let two = b.k(2);
        b.bin(BinOp::Add, x, two)
    };
    // CpuSet of the cell, one word: F loaded from the copy and added to.
    let f = copied(0x0b, Some(CELL), word, add);
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Arithmetic, "{f:?}");
    // Called through instead: followed and proven.
    let f = copied(0x0b, Some(CELL), word, |_, x| x);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    // From the cell's mirror, by CpuFastSet; and from a source the run
    // can't place (r0 as the function came: unknown).
    for (n, src) in [(0x0c, Some(CELL + 0x8000)), (0x0b, None)] {
        let f = copied(n, src, word, add);
        assert_eq!(
            f.holes[&Origin::Word(POOL)].1,
            Why::Arithmetic,
            "{n:#x} {src:x?}"
        );
    }
    // Its length unknown: every cell.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let c = b.k(CELL);
    b.st(Width::Word, c, p);
    let s = b.k(CELL - 0x40);
    b.set(0, s);
    let mut then = B::default();
    let c2 = then.k(0x0200_0400);
    let x = then.ld(Width::Word, c2);
    let t = add(&mut then, x);
    let f = run(&[swi_then(0x0800_1001, b, 0x0b, then, t)]);
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Arithmetic, "{f:?}");
    // A copy that ends before the cell doesn't move F.
    let f = copied(0x0b, Some(CELL - 0x40), word, |_, x| x);
    assert!(site(&f).callees.is_empty(), "{f:?}");
}

#[test]
fn a_function_address_a_bios_call_computes_with_is_lost() {
    // ObjAffineSet's parameters, BgAffineSet's, MidiKey2Freq's wave
    // header (F its frequency), LZ77's data: through r0, from the cell.
    for (n, src) in [
        (0x0f, CELL),
        (0x0e, CELL),
        (0x1f, CELL - 4),
        (0x11, CELL - 0x40),
    ] {
        let f = copied(n, Some(src), 1, |_, x| x);
        assert_eq!(f.holes[&Origin::Word(POOL)], (SWI_AT, Why::Bios), "{n:#x}");
    }
    // Past what it reads, nothing: one entry of ObjAffineSet ends before.
    let f = copied(0x0f, Some(CELL - 8), 1, |_, x| x);
    assert!(!f.holes.contains_key(&Origin::Word(POOL)), "{f:?}");
}

/// The play build's BIOS, call `n` from an ARM `swi` with `r` in r0 to
/// r3, and any stall it leaves: the registers after, and EWRAM and IWRAM.
fn play_bios(n: u8, r: [u32; 4]) -> ([u32; 4], Vec<u8>) {
    use runtime::bios;
    let rom: &'static [u8] = Box::leak(vec![0u8; 0x200].into_boxed_slice());
    let mut m = Box::new(runtime::Machine::new(rom, runtime::platform::Headless));
    m.cpu.pc = 0x0800_0104;
    m.cpu.r[..4].copy_from_slice(&r);
    // RAM a pattern, so a copy moves something and a clear clears it;
    // the sources' first word 0, a decompressor's empty input.
    let bus = &mut m.bus;
    for (i, b) in bus.ewram.iter_mut().chain(&mut bus.iwram).enumerate() {
        *b = (i as u8).wrapping_mul(31) | 1;
    }
    bus.ewram[0x100..0x104].fill(0);
    bios::swi(&mut m, (n as u32) << 16).unwrap();
    while m.cpu.pc == bios::STALL_LOOP {
        m.step();
    }
    let ram = [&m.bus.ewram[..], &m.bus.iwram[..]].concat();
    (m.cpu.r[..4].try_into().unwrap(), ram)
}

#[test]
fn the_play_builds_bios_reads_and_leaves_what_the_rules_say() {
    // Each call from three sets of registers: a source and a destination
    // in EWRAM with a count of 4 (the source's first word 0, every
    // decompressor's empty input, then a pattern); a source in the BIOS,
    // which the copies refuse; and flags that make RegisterRamReset clear
    // memory. The decompressors' and MultiBoot's own work isn't reached
    // (the lift check's native-bios programs hold the lifted calls to the
    // play build's on real data), nor a wait's end, and nothing here tells
    // "sets" from "may set".
    let sets = [
        [0x0200_0100, 0x0200_1000, 4, 2],
        [0, 0x0200_1000, 3, 0x0bad_f00d],
        [0x1f, 0x0200_1000, 0, 7],
    ];
    for &n in analysis::holes::NATIVE_BIOS {
        // SoftReset doesn't return, and zeroes them all.
        if n == 0x00 {
            continue;
        }
        let rule = bios_regs(Some(n));
        for r in sets {
            let (out, ram) = play_bios(n, r);
            for i in 0..4 {
                // One the rule says is as it came, is.
                if (rule.sets | rule.may_set) & 1 << i == 0 {
                    assert_eq!(out[i], r[i], "call {n:#04x} changed r{i}");
                }
                // One it doesn't read changes nothing else.
                if rule.reads & 1 << i == 0 {
                    let mut other = r;
                    other[i] = !other[i];
                    let (o2, ram2) = play_bios(n, other);
                    for j in (0..4).filter(|&j| j != i) {
                        assert_eq!(o2[j], out[j], "call {n:#04x}: r{j} by r{i}");
                    }
                    assert!(ram2 == ram, "call {n:#04x}: RAM by r{i}");
                }
            }
        }
    }
}

#[test]
fn equal_sets_kept_are_one_set() {
    let set = |n: u32| -> Toks { (0..n).map(Tok::Num).collect() };
    let same = |a: &Toks, b: &Toks| std::rc::Rc::ptr_eq(&a.0, &b.0);
    let mut sets = Sets::default();
    let (mut a, mut b, mut c) = (set(100), set(100), set(99));
    for t in [&mut a, &mut b, &mut c] {
        sets.share(t);
    }
    assert!(same(&a, &b) && !same(&a, &c));
    // Changing one leaves the other, which stays the kept set.
    b.insert(Tok::Num(1000));
    assert_eq!((a.len(), b.len()), (100, 101));
    let mut d = set(100);
    sets.share(&mut d);
    assert!(same(&a, &d));
    // A kept set changed while nothing else held it is not found as what
    // it was.
    let mut e = set(50);
    sets.share(&mut e);
    e.insert(Tok::Num(77));
    let mut f = set(50);
    sets.share(&mut f);
    assert_eq!((e.len(), f.len()), (51, 50));
    // One nothing holds is gone: the next of its tokens is kept instead.
    drop((a, d));
    let mut g = set(100);
    let p = std::rc::Rc::as_ptr(&g.0);
    sets.share(&mut g);
    assert_eq!(p, std::rc::Rc::as_ptr(&g.0));
}

/// Entries that grow to the same tokens by different joins hold one set,
/// and the bound counts it once.
#[test]
fn entries_joined_to_the_same_tokens_share_a_set() {
    let fn_tok = |key| Tok::Fn {
        key,
        origin: Origin::Word(key),
        form: Form::Full,
    };
    let of = |ks: std::ops::Range<u32>| Val {
        toks: ks.map(fn_tok).collect(),
        ..Val::default()
    };
    let mut g = G::default();
    let (x, y) = (vec![of(0..80)], vec![of(40..120)]);
    g.give(10, &x);
    g.give(10, &y);
    g.give(11, &y);
    g.give(11, &x);
    g.give_ret(12, &y);
    g.give_ret(12, &x);
    let (a, b, c) = (&g.entry[&10][0], &g.entry[&11][0], &g.ret[&12][0]);
    assert_eq!(a.toks.len(), 120);
    assert!(std::rc::Rc::ptr_eq(&a.toks.0, &b.toks.0));
    assert!(std::rc::Rc::ptr_eq(&a.toks.0, &c.toks.0));
    assert_eq!(g.held().iter().map(|h| h.2).sum::<usize>(), 120);
}

/// The bound counts what the kept reads and expansions hold too.
#[test]
fn the_bound_counts_the_kept_expansions() {
    let set = |lo: u32| -> Toks { (lo..lo + 10).map(Tok::Num).collect() };
    let val = |lo: u32| Val {
        toks: set(lo),
        ..Val::default()
    };
    let mut g = G::default();
    let r = mem_ref(false, 4);
    g.ref_exp.insert(
        r,
        Exp {
            v: val(0),
            raw: set(0).iter().copied().collect(),
            ..Exp::default()
        },
    );
    g.ref_rom.insert(mem_ref(true, 4), val(100));
    let key = (0, None, [(std::ptr::null(), false, false, false); 2]);
    g.expanded
        .insert(key, ([set(200), Toks::default()], vec![val(300)], val(400)));
    g.split_rest.insert((r, Xf::ID), (set(500), set(600)));
    g.pile_raw[4] = val(700);
    g.cell_reads[4] = Some(val(800));
    g.loose_args = vec![val(900)];
    g.loose_rets = Some(vec![val(1000)]);
    g.mem_log.entry(PILE).or_default().push((r, 4));
    // Eleven sets of ten, the expansion's raw tokens and the log's one.
    assert_eq!(g.held().iter().map(|h| h.2).sum::<usize>(), 110 + 10 + 1);
}

/// A table of `n` functions' addresses at 0x0800_0800, the functions'
/// keys, and the table's words.
fn table(n: u32) -> (Img, BTreeSet<u32>, Vec<u32>) {
    let mut img = Img::new();
    img.top = 0x0801_0000;
    let mut words = Vec::new();
    for i in 0..n {
        let k = 0x0800_4001 + 16 * i;
        img.fns.insert(k);
        img.words.insert(0x0800_0800 + 4 * i, k);
        words.push(0x0800_0800 + 4 * i);
    }
    let fns = img.fns.clone();
    (img, fns, words)
}

/// What a use of a table's entry comes to: whether the call's callees are
/// the table's functions, the set complete, each `Why` with whether every
/// word has that hole, and whether every word is proven.
type Shape = (bool, bool, Vec<(Why, bool)>, bool);

/// Runs `body` (given the entry loaded by an unknown index at width `w`,
/// it gives the call's target), twice over, on a table of `n` functions.
fn table_use(n: u32, w: Width, body: &dyn Fn(&mut B, ValueId) -> ValueId) -> Shape {
    let (img, fns, words) = table(n);
    let mut b = B::default();
    let tab = b.pool(0x0800_0408, 0x0800_0800);
    let i = b.get(1);
    let a = b.bin(BinOp::Add, tab, i);
    let x = b.ld(w, a);
    // Twice: the second time a large value's ops are found as kept.
    body(&mut b, x);
    let t = body(&mut b, x);
    let f = run_img(&[calling(0x0800_1001, b, t)], &BTreeMap::new(), &img);
    assert!(!f.unsettled);
    let mut whys: BTreeMap<Why, usize> = BTreeMap::new();
    for wd in &words {
        if let Some((_, why)) = f.holes.get(&Origin::Word(*wd)) {
            *whys.entry(*why).or_default() += 1;
        }
    }
    (
        site(&f).callees == fns,
        site(&f).complete,
        whys.into_iter()
            .map(|(y, c)| (y, c == words.len()))
            .collect(),
        words.iter().all(|wd| f.proven(*wd)),
    )
}

/// Two RAM pointers the run drops: another mode's registers
/// (`native/fnref-ram-pointers`), each plus an unknown.
fn dropped(b: &mut B) -> (ValueId, ValueId) {
    let (p, q) = (b.op(Op::GetUserReg(Reg(13))), b.op(Op::GetUserReg(Reg(14))));
    let (n, m) = (b.get(2), b.get(3));
    (b.bin(BinOp::Add, p, n), b.bin(BinOp::Add, q, m))
}

/// A read of a hundred functions' addresses is one [`Tok::Ref`], where
/// one of three is its tokens: every use comes to the same.
#[test]
fn a_large_read_is_used_as_a_small_one_is() {
    type Body = (
        &'static str,
        Width,
        Box<dyn Fn(&mut B, ValueId) -> ValueId>,
        Shape,
    );
    let proven: Shape = (true, true, vec![], true);
    let holed = |why| (true, true, vec![(why, true)], false);
    let bodies: Vec<Body> = vec![
        ("called", Width::Word, Box::new(|_, x| x), proven.clone()),
        (
            "stored to a cell and loaded",
            Width::Word,
            Box::new(|b, x| {
                let c = b.k(CELL);
                b.st(Width::Word, c, x);
                b.ld(Width::Word, c)
            }),
            proven.clone(),
        ),
        (
            "stored and loaded through RAM pointers the run dropped",
            Width::Word,
            Box::new(|b, x| {
                let (p, q) = dropped(b);
                b.st(Width::Word, p, x);
                b.ld(Width::Word, q)
            }),
            proven.clone(),
        ),
        (
            "compared with 0",
            Width::Word,
            Box::new(|b, x| {
                let z = b.k(0);
                let d = b.bin(BinOp::Sub, x, z);
                b.op(Op::IsZero(d));
                x
            }),
            proven.clone(),
        ),
        (
            "added to",
            Width::Word,
            Box::new(|b, x| {
                let four = b.k(4);
                b.bin(BinOp::Add, x, four);
                x
            }),
            holed(Why::Arithmetic),
        ),
        (
            "its top half used as an address",
            Width::Word,
            Box::new(|b, x| {
                let h = b.shift(ShiftType::Lsr, x, 16);
                b.ld(Width::Word, h);
                x
            }),
            holed(Why::Address),
        ),
        (
            "its compare's result stored",
            Width::Word,
            Box::new(|b, x| {
                let z = b.k(0);
                let d = b.bin(BinOp::Sub, x, z);
                let c = b.k(CELL);
                b.st(Width::Word, c, d);
                x
            }),
            holed(Why::CmpUsed),
        ),
        (
            "copied by bytes and loaded as a word",
            Width::Byte,
            Box::new(|b, x| {
                let (p, q) = dropped(b);
                b.st(Width::Byte, p, x);
                b.ld(Width::Word, q)
            }),
            proven.clone(),
        ),
        (
            "its bytes called",
            Width::Byte,
            Box::new(|_, x| x),
            (false, false, vec![(Why::CalledInPart, true)], false),
        ),
    ];
    for (what, w, body, want) in bodies {
        let small = table_use(3, w, &*body);
        let large = table_use(100, w, &*body);
        assert_eq!(small, want, "{what}, 3 functions");
        assert_eq!(large, want, "{what}, 100 functions");
    }
}

/// A join into an entry that the cap undoes (a pointer one "from here
/// on" takes in, a number into an unknown) leaves the set itself, not a
/// copy of it; one that changes it says so whatever its length after.
/// Either way it is what the plain join makes.
#[test]
fn a_join_the_cap_undoes_leaves_the_set_itself() {
    let mut img = Img::new();
    img.code = (1..16).map(|k| 0x0800_0000 + 0x100 * k).collect();
    let _code = CodeGuard::set(&img);
    // `join_capped` of `o` into `a`: as `join`, and whether it changed.
    let both = |a: &mut Val, o: Val| {
        let (was, mut plain) = (a.clone(), a.clone());
        plain.join(&o);
        let p = std::rc::Rc::as_ptr(&a.toks.0);
        let grew = a.join_capped(&o);
        assert_eq!(*a, plain, "{was:?} joined with {o:?}");
        assert_eq!(grew, *a != was, "{was:?} joined with {o:?}");
        // Unchanged, it is the set it was.
        assert!(grew || p == std::rc::Rc::as_ptr(&a.toks.0));
        grew
    };
    let rom = |x: u32| Val::of(Tok::Rom(x));
    let mut a = Val::of(Tok::RomFrom(0x0800_0810));
    a.join(&Val::unk());
    assert!(!both(&mut a, rom(0x0800_0820)));
    assert!(!both(&mut a, Val::of(Tok::RomFrom(0x0800_0840))));
    assert!(!both(&mut a, Val::of(Tok::Num(7))));
    // Past the next code, it is its own.
    assert!(both(&mut a, rom(0x0800_0910)));
    // Sixteen pointers, one in each run of data, and a seventeenth:
    // sixteen "from here on".
    let mut b = Val::default();
    for k in 0..16 {
        both(&mut b, rom(0x0800_0010 + 0x100 * k));
    }
    assert_eq!(b.toks.len(), 16);
    assert!(both(&mut b, rom(0x0800_0020)));
    assert_eq!(b.toks.len(), 16);
    assert!(b.toks.contains(&Tok::RomFrom(0x0800_0010)));
    // Pointers a "from here on" takes in still count toward the cap's
    // sixteen: with the ones held below it, they widen those.
    let mut c = Val::of(Tok::RomFrom(0x0800_0080));
    for k in 0..16 {
        both(&mut c, rom(0x0800_0010 + 4 * k));
    }
    assert_eq!(c.toks.len(), 17);
    assert!(both(&mut c, rom(0x0800_0090)));
    assert_eq!(
        c.toks.iter().copied().collect::<Vec<_>>(),
        [Tok::RomFrom(0x0800_0010)]
    );
    // With none held, they don't.
    let mut d = Val::of(Tok::RomFrom(0x0800_0080));
    let many = Val {
        toks: (0..20).map(|k| Tok::Rom(0x0800_0084 + 4 * k)).collect(),
        ..Val::default()
    };
    assert!(!both(&mut d, many));
}

/// A table of 12-byte records in ROM data: a byte flag at +0, a function
/// at +4 and another at +8 (`native/fnref-table-fields`).
const TB: u32 = 0x0800_0700;

fn records_img() -> Img {
    let mut img = Img::new();
    // A pool word points at the table: an object's start.
    img.words.insert(0x0800_0410, TB);
    for (r, (flag, a, b)) in [(1, F, H), (0, H, F)].into_iter().enumerate() {
        let at = TB + 12 * r as u32;
        img.words.insert(at, flag);
        img.words.insert(at + 4, a);
        img.words.insert(at + 8, b);
    }
    img
}

/// `ldrb r1, [CELL]`, scaled to a record (`lsl r0, r1, #1; add r0, r1;
/// lsl r0, #2`, 12 times r1, when `scaled`; else r1 as it is), added to
/// the table's start and `off`.
fn record_field(b: &mut B, scaled: bool, off: u32) -> ValueId {
    let c = b.k(CELL);
    let i = b.ld(Width::Byte, c);
    b.set(1, i);
    let i = b.get(1);
    let x = if scaled {
        let two = b.shift(ShiftType::Lsl, i, 1);
        b.set(0, two);
        let (r0, r1) = (b.get(0), b.get(1));
        let three = b.bin(BinOp::Add, r0, r1);
        b.shift(ShiftType::Lsl, three, 2)
    } else {
        i
    };
    let tb = b.pool(0x0800_0410, TB);
    let p = b.bin(BinOp::Add, x, tb);
    let o = b.k(off);
    b.bin(BinOp::Add, p, o)
}

/// A function that reads the records' flag byte by index and shifts it
/// (a use no function's address is allowed), stores the field at +8 to
/// RAM, and calls the one at +4.
fn reading_records(scaled: bool) -> Flow {
    let mut b = B::default();
    let a = record_field(&mut b, scaled, 0);
    let flag = b.ld(Width::Byte, a);
    let up = b.shift(ShiftType::Lsl, flag, 28);
    b.set(0, up);
    let a = record_field(&mut b, scaled, 8);
    let h = b.ld(Width::Word, a);
    let c = b.k(CELL + 4);
    b.st(Width::Word, c, h);
    let a = record_field(&mut b, scaled, 4);
    let t = b.ld(Width::Word, a);
    run_img(
        &[calling(0x0800_1001, b, t)],
        &BTreeMap::new(),
        &records_img(),
    )
}

#[test]
fn a_table_of_records_read_by_one_field_reads_only_that_field() {
    let f = reading_records(true);
    // The flag's reads give no function's bytes, so the shift holes none.
    for w in [TB + 4, TB + 8, TB + 16, TB + 20] {
        assert!(f.proven(w), "{w:#x}: {f:?}");
    }
    // The call through the field at +4 calls only what that field holds.
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]));
    assert!(site(&f).complete);
    // By an index the run doesn't know the scale of, a byte read "from
    // here on" reads every byte of the records: the shift holes them.
    let f = reading_records(false);
    for w in [TB + 4, TB + 8, TB + 16, TB + 20] {
        assert!(!f.proven(w), "{w:#x}: {f:?}");
        assert_eq!(f.holes[&Origin::Word(w)].1, Why::Arithmetic);
    }
}

#[test]
fn strides_follow_a_scaled_index_through_its_registers() {
    let mut b = B::default();
    let x = record_field(&mut b, true, 4);
    let s = strides(&b.insts);
    // 12 times a byte: 12; the table's start plus it, unknown: 1.
    let Op::Bin(_, p, _) = b.insts[x.index()].op else {
        panic!()
    };
    let Op::Bin(_, scaled, _) = b.insts[p.index()].op else {
        panic!()
    };
    assert_eq!(s[scaled.index()], 12);
    assert_eq!(s[x.index()], 1);
    // A word's index scaled by 12 may wrap: only its 4 is kept.
    let mut b = B::default();
    let i = b.get(1);
    let two = b.shift(ShiftType::Lsl, i, 1);
    let three = b.bin(BinOp::Add, two, i);
    let twelve = b.shift(ShiftType::Lsl, three, 2);
    assert_eq!(strides(&b.insts)[twelve.index()], 4);
}

/// One block of one function at 0x0800_1001, ending in `edge`.
fn alone(b: B, edge: Edge) -> Built {
    (0x0800_1001, vec![(0x0800_1001, b.end(None, edge))])
}

/// `cmp p, #0` (`p + !0 + 1`): its result, carry and overflow.
fn cmp_zero(b: &mut B, p: ValueId) -> [ValueId; 3] {
    let n = b.k(!0);
    let one = b.op(Op::Bit(true));
    [
        b.op(Op::Adc { a: p, b: n, c: one }),
        b.op(Op::AdcCarry { a: p, b: n, c: one }),
        b.op(Op::AdcOverflow { a: p, b: n, c: one }),
    ]
}

#[test]
fn an_add_of_zero_is_a_move() {
    // adds r1, r0, #0 (old GCC's move), its C and V read; then a call
    // through r1.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let (z, z2) = (b.k(0), b.k(0));
    let c = b.op(Op::Bit(false));
    let m = b.op(Op::Adc { a: p, b: z, c });
    let carry = b.op(Op::AdcCarry { a: p, b: z, c });
    let over = b.op(Op::AdcOverflow { a: z2, b: p, c });
    b.flag(Flag::C, carry);
    b.flag(Flag::V, over);
    let f = run(&[calling(0x0800_1001, b, m)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    assert!(site(&f).complete);
}

/// `negs r1, x; orrs r1, y` (`0 + !x + 1`, or'd with `y`), then `lsrs
/// r5, r1, #31` into r5.
fn ne_bit(b: &mut B, x: ValueId, y: ValueId) {
    let z = b.k(0);
    let n = b.op(Op::Not(x));
    let one = b.op(Op::Bit(true));
    let neg = b.op(Op::Adc { a: z, b: n, c: one });
    let or = b.bin(BinOp::Or, neg, y);
    let bit = b.shift(ShiftType::Lsr, or, 31);
    b.set(5, bit);
}

#[test]
fn old_gcc_s_not_zero_bit_is_no_arithmetic() {
    // (r0 | -r0) >> 31 on a callback, which is then called.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    ne_bit(&mut b, p, p);
    let f = run(&[calling(0x0800_1001, b, p)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    // With another value or'd in it is arithmetic.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let r = b.get(2);
    ne_bit(&mut b, p, r);
    let f = run(&[calling(0x0800_1001, b, p)]);
    assert!(!f.proven(POOL));
    // So is `p | -r`.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let r = b.get(2);
    ne_bit(&mut b, r, p);
    let f = run(&[calling(0x0800_1001, b, p)]);
    assert!(!f.proven(POOL));
}

/// `ands r1, p, #m; cmp r1, #0`, Z read and r1 stored to RAM; then a
/// call through p.
fn bit_test(m: u32) -> Flow {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let m = b.k(m);
    let t = b.bin(BinOp::And, m, p);
    let [d, carry, _] = cmp_zero(&mut b, t);
    let z = b.op(Op::IsZero(d));
    b.flag(Flag::Z, z);
    b.flag(Flag::C, carry);
    let c = b.k(CELL);
    b.st(Width::Word, c, t);
    run(&[calling(0x0800_1001, b, p)])
}

#[test]
fn a_test_of_high_bits_an_address_has_clear_is_zero() {
    // Bit 24 (Metroid's `movs r1, #128; lsls r1, #17; ands r1, r0`) and
    // a few top bits: 0 for F, and for any dense ID.
    for m in [0x0100_0000, 0xf000_0000, 0x0001_0000] {
        let f = bit_test(m);
        assert!(f.proven(POOL), "{m:#x}: {f:?}");
        assert_eq!(site(&f).callees, BTreeSet::from([F]));
    }
    // A bit F has set (dense, it would be clear), or a mask reaching
    // into the bottom half (where a dense ID is), is arithmetic.
    for m in [0x0800_0000, 0x0100_0001, 0x0000_0200, 0xffff_0000] {
        let f = bit_test(m);
        assert!(!f.proven(POOL), "{m:#x}");
        assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Arithmetic);
    }
}

#[test]
fn an_add_of_one_is_still_arithmetic() {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let z = b.k(0);
    let c = b.op(Op::Bit(true));
    let m = b.op(Op::Adc { a: p, b: z, c });
    let f = run(&[calling(0x0800_1001, b, m)]);
    assert!(!f.proven(POOL));
}

#[test]
fn a_compare_with_zero_carries_and_never_overflows() {
    // cmp r0, #0 on a callback, C and V read: no ordered compare.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let [d, carry, over] = cmp_zero(&mut b, p);
    let z = b.op(Op::IsZero(d));
    b.flag(Flag::Z, z);
    b.flag(Flag::C, carry);
    b.flag(Flag::V, over);
    let f = run(&[alone(b, Edge::Return)]);
    assert!(f.proven(POOL), "{f:?}");
}

#[test]
fn the_carry_of_a_compare_with_a_number_is_still_ordered() {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let n = b.k(!5);
    let one = b.op(Op::Bit(true));
    let carry = b.op(Op::AdcCarry { a: p, b: n, c: one });
    b.flag(Flag::C, carry);
    let f = run(&[alone(b, Edge::Return)]);
    assert!(!f.proven(POOL));
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Ordered);
}

#[test]
fn a_function_or_a_number_compared_with_zero_is_allowed() {
    // r0 is F or 5 (a callback or a default), compared with 0.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let five = b.k(5);
    let g = b.op(Op::GetFlag(Flag::Z));
    let x = b.op(Op::Select {
        c: g,
        a: p,
        b: five,
    });
    let [d, ..] = cmp_zero(&mut b, x);
    let z = b.op(Op::IsZero(d));
    b.flag(Flag::Z, z);
    let f = run(&[alone(b, Edge::Return)]);
    assert!(f.proven(POOL), "{f:?}");
    // With 5 in the other operand it is a compare with 5.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let five = b.k(!5);
    let g = b.op(Op::GetFlag(Flag::Z));
    let n = b.k(!0);
    let y = b.op(Op::Select {
        c: g,
        a: n,
        b: five,
    });
    let one = b.op(Op::Bit(true));
    let d = b.op(Op::Adc { a: p, b: y, c: one });
    let z = b.op(Op::IsZero(d));
    b.flag(Flag::Z, z);
    let f = run(&[alone(b, Edge::Return)]);
    assert!(!f.proven(POOL));
}

/// `cmp r0, #0` on F setting N (bit 31 of the result), then `edge`;
/// with `read`, N is read after it.
fn n_flag_then(edge: Edge, read: bool) -> Flow {
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let [d, ..] = cmp_zero(&mut b, p);
    let n = b.op(Op::TestBit { v: d, n: 31 });
    b.op(Op::SetFlag(Flag::N, n));
    // Returned (r0 as it came).
    b.set(0, p);
    if read {
        b.read_flag(Flag::N);
    }
    run(&[alone(b, edge)])
}

#[test]
fn a_flag_nothing_reads_before_the_return_holes_nothing() {
    let f = n_flag_then(Edge::Return, false);
    assert!(f.proven(POOL), "{f:?}");
    // Read, it is a bit test.
    let f = n_flag_then(Edge::Return, true);
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::BitTest);
    // A jump into another function passes it the flags.
    let f = n_flag_then(Edge::Tail(None), false);
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::BitTest);
}

#[test]
fn a_flag_read_after_a_call_is_live_across_it() {
    // N set, a call, N read in the block the call returns to.
    let mut b = B::default();
    let p = b.pool(POOL, F);
    let [d, ..] = cmp_zero(&mut b, p);
    let n = b.op(Op::TestBit { v: d, n: 31 });
    b.op(Op::SetFlag(Flag::N, n));
    let t = b.k(H);
    let e = 0x0800_1001;
    let ret = e + 0x10;
    let mut r = B::default();
    r.read_flag(Flag::N);
    let f = run(&[(
        e,
        vec![
            (e, b.end(Some(t), Edge::Call { target: None, ret })),
            (ret, r.end(None, Edge::Return)),
        ],
    )]);
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::BitTest);
}

/// An image with Thumb code at 0x08000800..0x08000900 and at
/// 0x08000a00..0x08000a10, numbers between them but for a function's
/// address at 0x08000980.
fn img_with_code() -> Img {
    let mut img = Img::new();
    img.insts = vec![(0x0800_0800, 0x0800_0900), (0x0800_0a00, 0x0800_0a10)];
    for &(lo, hi) in &img.insts {
        img.code.extend((lo..hi).step_by(2));
    }
    img.words.insert(0x0800_0980, F);
    img
}

#[test]
fn bytes_that_hold_nothing_the_run_follows_are_dull() {
    let img = img_with_code();
    let _code = CodeGuard::set(&img);
    // Instructions, and the numbers before them back to the last
    // function's address (VT + 12's F).
    assert!(dull(0x0800_0810));
    assert!(dull(0x0800_0811));
    assert!(dull(0x0800_0700));
    assert!(dull(VT + 16));
    assert!(!dull(VT + 12));
    assert!(!dull(VT));
    // A run's last instruction reads on through the data after it,
    // which holds a function's address: not dull. The numbers after
    // that address, up to the next code, are.
    assert!(dull(0x0800_08fc));
    assert!(!dull(0x0800_08fe));
    assert!(!dull(0x0800_0900));
    assert!(!dull(0x0800_0980));
    assert!(dull(0x0800_0984));
    assert!(dull(0x0800_0a00));
    // The last code of all reads to the end of ROM.
    assert!(!dull(0x0800_0a0e));
    drop(_code);
    // No image, nothing is.
    assert!(!dull(0x0800_0810));
}

#[test]
fn pointers_into_dull_bytes_are_one_token_past_sixteen() {
    let img = img_with_code();
    let _code = CodeGuard::set(&img);
    // Sixteen, each from its own instruction on, are kept as they are.
    let mut v = Val::of(Tok::RomFrom(VT));
    for k in 0..16 {
        v.join(&Val::of(Tok::RomFrom(0x0800_0810 + 2 * k)));
    }
    assert_eq!(v.toks.len(), 17);
    // One more: all of them are the one token, beside the pointer into
    // the table.
    assert!(v.join_capped(&Val::of(Tok::RomFrom(0x0800_0880))));
    let want: Toks = [Tok::RomFrom(VT), Tok::RomDull].into_iter().collect();
    assert_eq!(v.toks, want);
    assert!(!v.lost_rom);
    // It takes in every pointer "from here on" there, and says nothing
    // changed, leaving the set it was.
    let p = std::rc::Rc::as_ptr(&v.toks.0);
    assert!(!v.join_capped(&Val::of(Tok::RomFrom(0x0800_0700))));
    assert!(!v.join_capped(&Val::of(Tok::RomStep(0x0800_0820, 4))));
    assert_eq!(v.toks, want);
    assert_eq!(p, std::rc::Rc::as_ptr(&v.toks.0));
    // An exact pointer there stays its own (one more may be a function's
    // key), as does one into followed data.
    assert!(v.join_capped(&Val::of(Tok::Rom(0x0800_0820))));
    assert!(v.join_capped(&Val::of(Tok::Rom(0x0800_0980))));
    assert_eq!(v.toks.len(), 4);
}

#[test]
fn ops_on_the_dull_token() {
    let img = img_with_code();
    let _code = CodeGuard::set(&img);
    let on = |op, x, y| match bin_tok(&img, op, x, y, [1, 1]) {
        R::T(t) => Some(t),
        _ => None,
    };
    let d = Some(Tok::RomDull);
    // An index added, or aligned down to its word: still there.
    assert_eq!(on(BinOp::Add, d, None), d);
    assert_eq!(on(BinOp::Add, d, Some(Tok::Num(8))), d);
    assert_eq!(on(BinOp::And, d, Some(Tok::Num(!3))), d);
    // Less a number, or masked to a page: no pointer the run knows.
    assert_eq!(on(BinOp::Sub, d, Some(Tok::Num(8))), None);
    assert_eq!(on(BinOp::And, d, Some(Tok::Num(0xffff_0000))), None);
    // Somewhere in ROM stays that, wherever it is moved.
    let any = Some(Tok::RomAny);
    assert_eq!(on(BinOp::Add, any, None), any);
    assert_eq!(on(BinOp::Sub, any, Some(Tok::Num(8))), any);
    assert_eq!(on(BinOp::And, any, Some(Tok::Num(0xffff_0000))), any);
}

#[test]
fn more_pointers_from_here_on_than_the_bound_are_dropped() {
    let mut img = Img::new();
    img.top = 0x0801_0000;
    img.code = (1..0x100).map(|k| 0x0800_0000 + 0x100 * k).collect();
    let _code = CodeGuard::set(&img);
    let mut v = Val::default();
    for k in 0..FROMS as u32 {
        v.join(&Val::of(Tok::RomFrom(0x0800_0010 + 0x100 * k)));
    }
    assert_eq!(v.toks.len(), FROMS);
    assert!(!v.lost_rom);
    v.join(&Val::of(Tok::RomFrom(0x0800_0010 + 0x100 * FROMS as u32)));
    let any: Toks = [Tok::RomAny].into_iter().collect();
    assert_eq!(v.toks, any, "{v:?}");
    assert!(v.lost_rom);
    // For good: it takes in every pointer after, and doesn't fill again.
    assert!(!v.join_capped(&Val::of(Tok::RomFrom(0x0800_0010))));
    assert!(!v.join_capped(&Val::of(Tok::Rom(0x0800_0020))));
    assert_eq!(v.toks, any);
    // A value made of memory's words is dropped the same way.
    let mut e = Exp::default();
    let mut d = Val::default();
    d.toks
        .extend((0..=FROMS as u32).map(|k| Tok::RomFrom(0x0800_0010 + 0x100 * k)));
    exp_merge(&mut e, d);
    assert!(e.v.lost_rom, "{:?}", e.v);
}

/// `n` functions' addresses, whole, each from its own word.
fn many_fns(n: usize) -> Val {
    Val {
        toks: (0..n as u32)
            .map(|k| Tok::Fn {
                key: 0x0800_2001 + 4 * k,
                origin: Origin::Word(0x0810_0000 + 4 * k),
                form: Form::Full,
            })
            .collect(),
        ..Val::default()
    }
}

#[test]
fn a_large_value_s_functions_are_the_large_set_s() {
    let mut g = G::default();
    // At the bound, a value is kept as it is.
    let mut small = many_fns(LARGE);
    g.take_large(&mut small);
    assert_eq!(small, many_fns(LARGE));
    assert!(!g.large.changed);
    // Past it, one reference to the set, which expands to what it held.
    let big = many_fns(LARGE + 1);
    let mut v = big.clone();
    g.take_large(&mut v);
    assert_eq!(v.toks.len(), 1, "{v:?}");
    assert!(v.marked());
    assert!(g.large.changed);
    assert_eq!(g.expand(&v), big);
    assert!(g.rd.large, "whoever looks at it reads the set");
    // A second value with the reference and a half of another function:
    // the half goes to the set, and the value's reference holds both
    // forms. The first value's, whole addresses only, doesn't gain it.
    let lo = Tok::Fn {
        key: F,
        origin: Origin::Word(POOL),
        form: Form::Lo,
    };
    let mut w = v.clone();
    w.join(&Val::of(lo));
    g.take_large(&mut w);
    assert_eq!(w.toks.len(), 1, "{w:?}");
    assert_ne!(w, v);
    let mut both = big.clone();
    both.join(&Val::of(lo));
    assert_eq!(g.expand(&w), both);
    assert_eq!(g.expand(&v), big);
    // Joined again with what it holds, it is as it was.
    let mut again = w.clone();
    again.join(&Val::of(lo));
    g.take_large(&mut again);
    assert_eq!(again, w);
    // A large value that may be a ROM pointer too: the run gives the
    // pointer up.
    let mut p = v.clone();
    p.join(&Val::of(Tok::RomFrom(VT)));
    g.take_large(&mut p);
    assert_eq!(p.toks, v.toks);
    assert!(p.lost_rom && p.unk);
}

#[test]
fn a_large_value_s_memory_reads_go_to_the_set() {
    let mut g = G::default();
    // The pile holds F, whole.
    let f = Tok::Fn {
        key: F,
        origin: Origin::Word(POOL),
        form: Form::Full,
    };
    g.store(Store::Pile, &[(f, 4)]);
    let mut v = many_fns(LARGE + 1);
    g.take_large(&mut v);
    // A large value that may also be a word of the pile: the read's
    // functions are the set's, and the value keeps the read's flags and
    // pointers by a reference with every form dropped.
    let mut w = v.clone();
    w.join(&Val::of(mem_ref(false, 4)));
    g.take_large(&mut w);
    assert_eq!(w.toks.len(), 2, "{w:?}");
    let e = g.expand(&w);
    assert!(e.toks.contains(&f));
    assert_eq!(e.toks.len(), LARGE + 2);
    // What the pile gains later, the value's expansion has.
    let h = Tok::Fn {
        key: H,
        origin: Origin::Word(0x0800_0404),
        form: Form::Full,
    };
    g.store(Store::Pile, &[(h, 4)]);
    assert!(g.expand(&w).toks.contains(&h));
}

#[test]
fn a_reference_that_stands_for_nothing_is_no_token() {
    let none = Xf::of(|_| None);
    assert_eq!(ref_tok(Src::Pile(4), none), None);
    // Forms the read never holds don't count: a byte read's are bytes.
    let full = Xf::of(|f| (f == Form::Full).then_some(Form::Full));
    assert_eq!(ref_tok(Src::Pile(1), full), None);
    assert!(ref_tok(Src::Pile(4), full).is_some());
    // Its flags or its ROM pointers alone are something.
    assert!(
        ref_tok(
            Src::Pile(4),
            Xf {
                flags: true,
                ..none
            }
        )
        .is_some()
    );
    assert!(ref_tok(Src::Pile(4), Xf { roms: true, ..none }).is_some());
}

/// An allocator: returns a pointer into the heap's RAM.
const ALLOC: u32 = 0x0800_3001;
/// Where it says the memory is.
const HEAP: u32 = 0x0200_1000;
/// A second RAM word.
const CELL2: u32 = 0x0300_0104;

fn allocator() -> Built {
    let mut a = B::default();
    let p = a.k(HEAP);
    a.set(0, p);
    (ALLOC, vec![(ALLOC, a.end(None, Edge::Return))])
}

/// `b` ended with a direct call of [`ALLOC`] at `at`, returning to `ret`.
fn alloc_call(b: B, at: u32, ret: u32) -> (Vec<Inst>, Out, BTreeMap<usize, u32>) {
    call_at(b, ALLOC, at, ret)
}

/// `b` ended with a direct call of `target` at `at`, returning to `ret`.
fn call_at(b: B, target: u32, at: u32, ret: u32) -> (Vec<Inst>, Out, BTreeMap<usize, u32>) {
    let mut x = b.end(
        None,
        Edge::Call {
            target: Some(target),
            ret,
        },
    );
    x.1.at = at;
    x
}

/// `p - k` as a `sub` instruction is lifted: `p + !k + 1`.
fn sub(b: &mut B, p: ValueId, k: u32) -> ValueId {
    let k = b.k(k);
    let n = b.op(Op::Not(k));
    let one = b.op(Op::Bit(true));
    b.op(Op::Adc { a: p, b: n, c: one })
}

/// A function that allocates twice, at two call sites: F stored at
/// `off_f` of the first allocation, whose pointer goes to [`CELL`]; H at
/// `off_h` of the second (`second`: else of the first again), whose
/// pointer goes to [`CELL2`].
fn two_allocations(off_f: u32, off_h: u32, second: bool) -> Built {
    two_allocations_by(ALLOC, off_f, off_h, second)
}

/// [`two_allocations`], by calls of `alloc`.
fn two_allocations_by(alloc: u32, off_f: u32, off_h: u32, second: bool) -> Built {
    let e = 0x0800_1001;
    let mut b1 = B::default();
    let p = b1.get(0);
    let o = b1.k(off_f);
    let x = b1.bin(BinOp::Add, p, o);
    let f = b1.pool(POOL, F);
    b1.st(Width::Word, x, f);
    let c = b1.k(CELL);
    b1.st(Width::Word, c, p);
    b1.set(4, p);
    let mut b2 = B::default();
    let p = if second { b2.get(0) } else { b2.get(4) };
    let o = b2.k(off_h);
    let x = b2.bin(BinOp::Add, p, o);
    let h = b2.pool(0x0800_0404, H);
    b2.st(Width::Word, x, h);
    let c = b2.k(CELL2);
    b2.st(Width::Word, c, p);
    (
        e,
        vec![
            (e, call_at(B::default(), alloc, 0x0800_0110, e + 0x10)),
            (e + 0x10, call_at(b1, alloc, 0x0800_0120, e + 0x20)),
            (e + 0x20, b2.end(None, Edge::Return)),
        ],
    )
}

/// A call through the word at `off` of the allocation [`CELL`] (or
/// `cell`) points to.
fn call_field(cell: u32, off: u32) -> Built {
    let mut b = B::default();
    let c = b.k(cell);
    let p = b.ld(Width::Word, c);
    let o = b.k(off);
    let x = b.bin(BinOp::Add, p, o);
    let t = b.ld(Width::Word, x);
    calling(0x0800_2001, b, t)
}

#[test]
fn two_allocations_are_kept_apart() {
    // Each allocation's word 0 holds its own function: a call through
    // the first's is F alone, through the second's H alone.
    let fns = |cell| {
        [
            allocator(),
            two_allocations(0, 0, true),
            call_field(cell, 0),
        ]
    };
    let f = run_heap(&fns(CELL), &[ALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
    assert!(f.proven(POOL) && f.proven(0x0800_0404), "{f:?}");
    assert!(!f.piled.contains(&Origin::Word(POOL)), "{f:?}");
    let f = run_heap(&fns(CELL2), &[ALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([H]), "{f:?}");
    // With no allocator named, both are one RAM address's.
    let f = run_ram(&fns(CELL));
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

#[test]
fn two_fields_of_an_allocation_are_kept_apart() {
    // F at +0 and H at +4 of one allocation: a call through each word.
    let fns = |off| {
        [
            allocator(),
            two_allocations(0, 4, false),
            call_field(CELL, off),
        ]
    };
    let f = run_heap(&fns(0), &[ALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    let f = run_heap(&fns(4), &[ALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([H]), "{f:?}");
    // A byte of the word at +4 is H's: its use as a number holes H alone.
    let mut b = B::default();
    let c = b.k(CELL);
    let p = b.ld(Width::Word, c);
    let o = b.k(5);
    let x = b.bin(BinOp::Add, p, o);
    let v = b.ld(Width::Byte, x);
    let one = b.k(1);
    let n = b.bin(BinOp::Add, v, one);
    b.set(0, n);
    let user = (0x0800_2001, vec![(0x0800_2001, b.end(None, Edge::Return))]);
    let f = run_heap(&[allocator(), two_allocations(0, 4, false), user], &[ALLOC]);
    assert!(f.proven(POOL), "{f:?}");
    assert!(!f.proven(0x0800_0404), "{f:?}");
}

#[test]
fn an_index_into_an_allocation_reads_all_of_it() {
    // p + r1, r1 unknown: somewhere in the allocation, both its words;
    // but not the other allocation's.
    let user = || {
        let mut b = B::default();
        let c = b.k(CELL);
        let p = b.ld(Width::Word, c);
        let i = b.get(1);
        let x = b.bin(BinOp::Add, p, i);
        let t = b.ld(Width::Word, x);
        calling(0x0800_2001, b, t)
    };
    let f = run_heap(
        &[allocator(), two_allocations(0, 4, false), user()],
        &[ALLOC],
    );
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
    let f = run_heap(
        &[allocator(), two_allocations(0, 0, true), user()],
        &[ALLOC],
    );
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
}

#[test]
fn an_allocations_header_is_the_heaps_ram() {
    // `p - 8` is the allocator's header, in the RAM it returned: F
    // stored there isn't in the allocation, and a load at the heap's
    // address reads it.
    let make = || {
        let e = 0x0800_1001;
        let mut b1 = B::default();
        let p = b1.get(0);
        let o = b1.k(8);
        let x = b1.bin(BinOp::Sub, p, o);
        let f = b1.pool(POOL, F);
        b1.st(Width::Word, x, f);
        let c = b1.k(CELL);
        b1.st(Width::Word, c, p);
        (
            e,
            vec![
                (e, alloc_call(B::default(), 0x0800_0110, e + 0x10)),
                (e + 0x10, b1.end(None, Edge::Return)),
            ],
        )
    };
    let f = run_heap(&[allocator(), make(), call_field(CELL, 0)], &[ALLOC]);
    assert!(site(&f).callees.is_empty(), "{f:?}");
    let mut b = B::default();
    let a = b.k(HEAP - 8);
    let t = b.ld(Width::Word, a);
    let f = run_heap(&[allocator(), make(), calling(0x0800_2001, b, t)], &[ALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
}

#[test]
fn pointers_into_many_sites_are_any_allocations() {
    let mut v = Val::default();
    for s in 1..=HEAP_SITES as u32 {
        v.toks.insert(Tok::Heap(s, 0));
    }
    v.cap();
    assert_eq!(v.toks.len(), HEAP_SITES);
    // One site more: offset 0 of any allocation, which takes each in.
    v.toks.insert(Tok::Heap(99, 0));
    v.cap();
    assert_eq!(v.toks.iter().collect::<Vec<_>>(), [&Tok::Heap(0, 0)]);
    assert!(v.takes_in(Tok::Heap(7, 0)) && !v.takes_in(Tok::Heap(7, 4)));
    // Its header is still a header (`free`'s `ptr - 16`).
    assert_eq!(heap_off(0, 0, -16), Tok::Heap(0, HEAP_UNDER));
    // Offsets: somewhere in a site's takes in its exact ones.
    let mut v = Val::default();
    v.toks
        .extend([Tok::Heap(3, 8), Tok::Heap(3, HEAP_ANY), Tok::Heap(4, 8)]);
    v.cap();
    assert_eq!(
        v.toks.iter().collect::<Vec<_>>(),
        [&Tok::Heap(3, HEAP_ANY), &Tok::Heap(4, 8)]
    );
}

#[test]
fn a_sub_back_to_an_allocations_start_is_in_it() {
    // `q = p + 8; subs q, #8` (an add of `!8`, then of 1) is the
    // allocation's start again, not its header on the way.
    let mut b = B::default();
    let c = b.k(CELL);
    let p = b.ld(Width::Word, c);
    let eight = b.k(8);
    let q = b.bin(BinOp::Add, p, eight);
    let r = sub(&mut b, q, 8);
    let t = b.ld(Width::Word, r);
    let fns = [
        allocator(),
        two_allocations(0, 4, false),
        calling(0x0800_2001, b, t),
    ];
    let f = run_heap(&fns, &[ALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    // And `subs p, #8` from the start is the header: F stored there is
    // the heap's RAM's, read at its address and not through the
    // allocation.
    let make = || {
        let e = 0x0800_1001;
        let mut b1 = B::default();
        let p = b1.get(0);
        let x = sub(&mut b1, p, 8);
        let f = b1.pool(POOL, F);
        b1.st(Width::Word, x, f);
        let c = b1.k(CELL);
        b1.st(Width::Word, c, p);
        (
            e,
            vec![
                (e, alloc_call(B::default(), 0x0800_0110, e + 0x10)),
                (e + 0x10, b1.end(None, Edge::Return)),
            ],
        )
    };
    let f = run_heap(&[allocator(), make(), call_field(CELL, 0)], &[ALLOC]);
    assert!(site(&f).callees.is_empty(), "{f:?}");
    let mut b = B::default();
    let a = b.k(HEAP - 8);
    let t = b.ld(Width::Word, a);
    let f = run_heap(&[allocator(), make(), calling(0x0800_2001, b, t)], &[ALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
}

#[test]
fn a_pointer_into_an_allocation_aligned_down() {
    let heap = |k| Some(Tok::Heap(3, k));
    let and = |k, m| match bin_tok(&Img::new(), BinOp::And, heap(k), Some(Tok::Num(m)), [1, 1]) {
        R::T(t) => t,
        _ => panic!("no token"),
    };
    RAM_ON.with(|c| c.set(true));
    // To a word: that word of it (an allocation starts on one).
    assert_eq!(and(7, !3), Tok::Heap(3, 4));
    // To eight bytes, from 8 or more on: at or past its start, somewhere.
    assert_eq!(and(12, !7), Tok::Heap(3, HEAP_ANY));
    // From less: either side of its start, a pointer the run drops.
    assert_eq!(and(4, !7), RAM_ANY);
    RAM_ON.with(|c| c.set(false));
}

#[test]
fn an_allocator_that_returns_anothers_result_has_its_own_sites() {
    // ZALLOC calls ALLOC and returns its pointer; both are named. Its two
    // callers' allocations are apart, and each one's header is where
    // ALLOC said the memory is.
    const ZALLOC: u32 = 0x0800_3101;
    let zalloc = || {
        (
            ZALLOC,
            vec![
                (ZALLOC, alloc_call(B::default(), 0x0800_0130, ZALLOC + 0x10)),
                (ZALLOC + 0x10, B::default().end(None, Edge::Return)),
            ],
        )
    };
    let fns = |cell| {
        [
            allocator(),
            zalloc(),
            two_allocations_by(ZALLOC, 0, 0, true),
            call_field(cell, 0),
        ]
    };
    let f = run_heap(&fns(CELL), &[ALLOC, ZALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    let f = run_heap(&fns(CELL2), &[ALLOC, ZALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([H]), "{f:?}");
    // With ALLOC alone named, ZALLOC's one call of it is one allocation.
    let f = run_heap(&fns(CELL), &[ALLOC]);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
    // The header, through ZALLOC's site: a load at the heap's address
    // reads what was stored 8 below the pointer.
    let e = 0x0800_1001;
    let mut b1 = B::default();
    let p = b1.get(0);
    let x = sub(&mut b1, p, 8);
    let fp = b1.pool(POOL, F);
    b1.st(Width::Word, x, fp);
    let make = (
        e,
        vec![
            (e, call_at(B::default(), ZALLOC, 0x0800_0110, e + 0x10)),
            (e + 0x10, b1.end(None, Edge::Return)),
        ],
    );
    let mut b = B::default();
    let a = b.k(HEAP - 8);
    let t = b.ld(Width::Word, a);
    let f = run_heap(
        &[allocator(), zalloc(), make, calling(0x0800_2001, b, t)],
        &[ALLOC, ZALLOC],
    );
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
}

#[test]
fn any_allocations_pointer_reads_and_reaches_every_site() {
    let tok = |key, w| Tok::Fn {
        key,
        origin: Origin::Word(w),
        form: Form::Full,
    };
    let (f, h) = (tok(F, POOL), tok(H, 0x0800_0404));
    let mut g = G::default();
    // F stored through site 0 (any allocation), H at +4 of site 3.
    g.store(Store::Cell(heap_cell(0, HEAP_ANY)), &[(f, 4)]);
    g.store(Store::Cell(heap_cell(3, 4)), &[(h, 4)]);
    let read = |g: &mut G, s, k| {
        let mut v = Val::default();
        g.read_heap(s, k, 4, &mut v);
        g.expand(&v)
    };
    // A load of site 3's word 0 reads F (it may be there), not H.
    let v = read(&mut g, 3, 0);
    assert!(v.toks.contains(&f) && !v.toks.contains(&h), "{v:?}");
    // Of its word 4, and of somewhere in it, both.
    for k in [4, HEAP_ANY] {
        let v = read(&mut g, 3, k);
        assert!(v.toks.contains(&f) && v.toks.contains(&h), "{v:?}");
    }
    // Site 5 holds F alone; any allocation's pointer reads both.
    let v = read(&mut g, 5, HEAP_ANY);
    assert!(v.toks.contains(&f) && !v.toks.contains(&h), "{v:?}");
    let v = read(&mut g, 0, 4);
    assert!(v.toks.contains(&f) && v.toks.contains(&h), "{v:?}");
    // A word load across two words reads the whole site.
    let v = read(&mut g, 3, 2);
    assert!(v.toks.contains(&h), "{v:?}");
}

#[test]
fn pointers_into_many_ram_objects_are_some_objects() {
    // With no objects found, each RAM is one object: use distinct starts.
    RAM_ON.with(|c| c.set(true));
    let mut v = Val::default();
    for i in 0..OBJS_MAX as u32 {
        v.toks.insert(Tok::Ram(0x0300_0100 + i * 0x100));
    }
    v.cap();
    assert_eq!(v.toks.len(), OBJS_MAX);
    // One more: somewhere in some object, which takes each in, and an
    // exact RAM address too; the stacks' pointer stays its own.
    v.toks
        .extend([Tok::Ram(0x0300_4000), STACK, Tok::Num(0x0300_0004)]);
    v.cap();
    assert_eq!(v.toks.iter().collect::<Vec<_>>(), [&STACK, &ANYOBJ]);
    assert!(v.takes_in(Tok::Ram(0x0300_0100)) && v.takes_in(Tok::Num(0x0200_0000)));
    assert!(!v.takes_in(LOOSE));
    // Capped again it is the same, and one more object beside it is it.
    let was = v.clone();
    v.cap();
    assert_eq!(v, was);
    v.toks.insert(Tok::Ram(0x0300_7000));
    v.cap();
    assert_eq!(v, was);
    // Exact addresses in more objects than the bound, with no pointer
    // into an object yet: the same in one widening.
    let mut v = Val::default();
    v.toks
        .extend((0..=OBJS_MAX as u32).map(|i| Tok::Num(0x0200_0000 + i * 0x1_0000)));
    v.cap();
    let once = v.clone();
    v.cap();
    assert_eq!(v, once);
    RAM_ON.with(|c| c.set(false));
}

#[test]
fn many_reads_of_single_objects_are_one_of_every_object() {
    RAM_ON.with(|c| c.set(true));
    let obj = |k: u32| Tok::Ref {
        src: Src::Obj(obj_cell(0x0300_0000 + k * 0x100), 4),
        xf: Xf::ID,
    };
    let all = Tok::Ref {
        src: Src::Objs(4),
        xf: Xf::ID,
    };
    let mut v = Val::default();
    v.toks.extend((0..OBJ_REFS as u32).map(obj));
    v.cap();
    assert_eq!(v.toks.len(), OBJ_REFS);
    assert!(!v.takes_in(obj(9)));
    // One more: the read of every object's, which takes each in.
    v.toks.insert(obj(9));
    v.cap();
    assert_eq!(v.toks.iter().collect::<Vec<_>>(), [&all]);
    assert!(v.takes_in(obj(3)));
    // At another width it is another read.
    let bytes = Tok::Ref {
        src: Src::Obj(obj_cell(0x0300_0000), 1),
        xf: Xf::ID,
    };
    assert!(!v.takes_in(bytes));
    RAM_ON.with(|c| c.set(false));
}

#[test]
fn a_store_through_some_objects_pointer_is_read_in_any_object() {
    // `n` arrays, each indexed from its start and so an object, and a
    // pointer that may be into any of them, stored through; then a load
    // at a constant address in one more array, which the pointer isn't
    // into.
    let arr = |k: u32| 0x0300_0200 + k * 0x400;
    let flow = |n: u32| {
        let e = 0x0800_1001;
        let mut a = B::default();
        let five = a.k(5);
        let mut p = None;
        for k in 0..n {
            let x = indexed(&mut a, 0x0800_0420 + 4 * k, arr(k), 2, 3, 0);
            a.st(Width::Word, x, five);
            p = Some(match p {
                None => x,
                Some(q) => {
                    let c = a.op(Op::GetFlag(Flag::Z));
                    a.op(Op::Select { c, a: q, b: x })
                }
            });
        }
        let f = a.pool(POOL, F);
        a.st(Width::Word, p.unwrap(), f);
        let y = indexed(&mut a, 0x0800_0480, arr(12), 2, 3, 0);
        a.st(Width::Word, y, five);
        let store = (e, vec![(e, a.end(None, Edge::Return))]);
        let mut b = B::default();
        let c = b.k(arr(12) + 0x10);
        let t = b.ld(Width::Word, c);
        run_ram(&[store, calling(0x0800_2001, b, t)])
    };
    // Into as many objects as the bound keeps: each one's own cell, which
    // a load in another object doesn't read.
    let f = flow(OBJS_MAX as u32);
    assert!(site(&f).callees.is_empty(), "{f:?}");
    // Into one more: somewhere in some object, read in every object.
    let f = flow(OBJS_MAX as u32 + 1);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
}

#[test]
fn a_function_s_read_ranges_are_kept_once() {
    let mut rd = Rd::default();
    for _ in 0..3 {
        rd.range(8, 11);
        rd.range(0, 100);
        rd.range(200, 200);
        rd.range(4, 4);
    }
    assert_eq!(rd.ranges.len(), 4);
    rd.settle();
    // Those inside another are left out.
    assert_eq!(rd.ranges, [(0, 100), (200, 200)]);
    // Two from one start: the longer; and one to the top stays.
    let mut rd = Rd::default();
    for r in [(8, 9), (8, 20), (8, 12), (30, u32::MAX), (40, u32::MAX)] {
        rd.range(r.0, r.1);
    }
    rd.settle();
    assert_eq!(rd.ranges, [(8, 20), (30, u32::MAX)]);
}

/// A callee that returns at once: r0-r3 as they came.
const PASS: u32 = 0x0800_3201;

fn pass_through() -> Built {
    (PASS, vec![(PASS, B::default().end(None, Edge::Return))])
}

/// Two callers of [`PASS`]: one with F in r1, which calls through r1
/// after; one with H in r1.
fn two_callers_of_pass() -> [Built; 2] {
    let e = 0x0800_1001;
    let mut b = B::default();
    let f = b.pool(POOL, F);
    b.set(1, f);
    let mut after = B::default();
    let t = after.get(1);
    let a = (
        e,
        vec![
            (e, call_at(b, PASS, 0x0800_0110, e + 0x10)),
            (
                e + 0x10,
                after.end(
                    Some(t),
                    Edge::Call {
                        target: None,
                        ret: e + 0x20,
                    },
                ),
            ),
            (e + 0x20, B::default().end(None, Edge::Return)),
        ],
    );
    let e2 = 0x0800_2001;
    let mut b = B::default();
    let h = b.pool(0x0800_0404, H);
    b.set(1, h);
    let c = (
        e2,
        vec![
            (e2, call_at(b, PASS, 0x0800_0120, e2 + 0x10)),
            (e2 + 0x10, B::default().end(None, Edge::Return)),
        ],
    );
    [a, c]
}

#[test]
fn a_register_a_callee_leaves_is_the_caller_s_own_after_the_call() {
    // As the callee's returns hold it, r1 after the call is every
    // caller's r1: F and H.
    let [a, c] = two_callers_of_pass();
    let f = run(&[pass_through(), a, c]);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
    // Left as it came (its summary's word), it is the caller's own: F.
    let [a, c] = two_callers_of_pass();
    let f = run_live(&[pass_through(), a, c], &[(PASS, 0b1111, 0b1111)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
    // One it may change is still its return's.
    let [a, c] = two_callers_of_pass();
    let f = run_live(&[pass_through(), a, c], &[(PASS, 0b1101, 0b1111)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

#[test]
fn a_callee_is_given_only_the_registers_it_reads() {
    // The callee calls through its r1; a caller passes F there.
    let callee = || {
        let mut b = B::default();
        let t = b.get(1);
        calling(0x0800_3001, b, t)
    };
    let caller = || {
        let e = 0x0800_1001;
        let mut b = B::default();
        let f = b.pool(POOL, F);
        b.set(1, f);
        (
            e,
            vec![
                (e, call_at(b, 0x0800_3001, 0x0800_0110, e + 0x10)),
                (e + 0x10, B::default().end(None, Edge::Return)),
            ],
        )
    };
    let f = run_live(&[callee(), caller()], &[(0x0800_3001, 0, 0b0010)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    // Said not to read r1 (it reads r0 alone), it isn't given it.
    let f = run_live(&[callee(), caller()], &[(0x0800_3001, 0, 0b0001)]);
    assert!(site(&f).callees.is_empty(), "{f:?}");
}

#[test]
fn a_call_with_no_callee_yet_waits_for_one() {
    // T calls through its r1. U calls T with its own r1, which comes
    // from V, and with H left in r3. The functions run in address order,
    // so H reaches T a round before F does; a loose function does
    // arithmetic on its r3.
    let fns = || {
        let mut b = B::default();
        let t = b.get(1);
        let t_fn = calling(0x0800_1001, b, t);
        let u = 0x0800_2001;
        let mut b = B::default();
        let h = b.pool(0x0800_0404, H);
        b.set(3, h);
        let u_fn = (
            u,
            vec![
                (u, call_at(b, 0x0800_1001, 0x0800_0110, u + 0x10)),
                (u + 0x10, B::default().end(None, Edge::Return)),
            ],
        );
        let v = 0x0800_5001;
        let mut b = B::default();
        let f = b.pool(POOL, F);
        b.set(1, f);
        let v_fn = (
            v,
            vec![
                (v, call_at(b, u, 0x0800_0120, v + 0x10)),
                (v + 0x10, B::default().end(None, Edge::Return)),
            ],
        );
        let l = 0x0800_4001;
        let mut b = B::default();
        let x = b.get(3);
        let one = b.k(1);
        let y = b.bin(BinOp::Add, x, one);
        b.set(0, y);
        let l_fn = (l, vec![(l, b.end(None, Edge::Return))]);
        [t_fn, u_fn, l_fn, v_fn]
    };
    // Taken as an incomplete set while it has no callee, the call hands
    // H to the loose function, for good.
    let f = run(&fns());
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(!f.proven(0x0800_0404), "{f:?}");
    // Waiting, it calls F alone, and H goes nowhere. (The entry for no
    // function only turns the switch on: every function leaves nothing
    // and is given all four.)
    let f = run_live(&fns(), &[(1, 0, 0b1111)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
    assert!(f.proven(0x0800_0404), "{f:?}");
}

#[test]
fn a_call_that_never_gets_a_callee_is_incomplete_in_the_end() {
    // A loose function calls through its r1 with F in r0: no function's
    // address ever reaches the target, so once the run has settled F goes
    // to every loose function, one of which does arithmetic on its r0.
    let mut b = B::default();
    let f = b.pool(POOL, F);
    b.set(0, f);
    let t = b.get(1);
    let caller = calling(0x0800_1001, b, t);
    let l = 0x0800_4001;
    let mut b = B::default();
    let x = b.get(0);
    let one = b.k(1);
    let y = b.bin(BinOp::Add, x, one);
    b.set(0, y);
    let l_fn = (l, vec![(l, b.end(None, Edge::Return))]);
    let f = run_live(&[caller, l_fn], &[(1, 0, 0b1111)]);
    assert!(!site(&f).complete, "{f:?}");
    assert!(!f.proven(POOL), "{f:?}");
}

#[test]
fn a_call_by_value_takes_back_what_each_callee_may_change() {
    // Two callees a pointer may call: one leaves r1 as it came, one sets
    // it to H. The caller has F in r1, calls one of them, and then calls
    // through r1: F (after the one that leaves it) or H.
    const LEAVE: u32 = 0x0800_3201;
    const SET: u32 = 0x0800_3301;
    let fns = || {
        let leave = (LEAVE, vec![(LEAVE, B::default().end(None, Edge::Return))]);
        let mut b = B::default();
        let h = b.pool(0x0800_0404, H);
        b.set(1, h);
        let set = (SET, vec![(SET, b.end(None, Edge::Return))]);
        // Another caller of LEAVE, with a third function in r1.
        let o = 0x0800_6001;
        let mut b = B::default();
        let a = b.pool(POOL_A, A);
        b.set(1, a);
        let other = (
            o,
            vec![
                (o, call_at(b, LEAVE, 0x0800_0130, o + 0x10)),
                (o + 0x10, B::default().end(None, Edge::Return)),
            ],
        );
        let e = 0x0800_1001;
        let mut b = B::default();
        let f = b.pool(POOL, F);
        b.set(1, f);
        let c = b.op(Op::GetFlag(Flag::Z));
        let l = b.pool(0x0800_0440, LEAVE);
        let s = b.pool(0x0800_0444, SET);
        let t = b.op(Op::Select { c, a: l, b: s });
        let mut first = b.end(
            Some(t),
            Edge::Call {
                target: None,
                ret: e + 0x10,
            },
        );
        first.1.at = 0x0800_0110;
        let mut after = B::default();
        let t = after.get(1);
        let caller = (
            e,
            vec![
                (e, first),
                (
                    e + 0x10,
                    after.end(
                        Some(t),
                        Edge::Call {
                            target: None,
                            ret: e + 0x20,
                        },
                    ),
                ),
                (e + 0x20, B::default().end(None, Edge::Return)),
            ],
        );
        [leave, set, other, caller]
    };
    let img = || {
        let mut img = Img::new();
        img.fns.extend([LEAVE, SET]);
        img.words.extend([(0x0800_0440, LEAVE), (0x0800_0444, SET)]);
        img
    };
    let run_on = |leaves: &[(u32, u8, u8)]| {
        LEAVES.with(|c| *c.borrow_mut() = leaves.iter().map(|&(k, l, r)| (k, (l, r))).collect());
        let f = run_img(&fns(), &BTreeMap::new(), &img());
        LEAVES.with(|c| c.borrow_mut().clear());
        f
    };
    // As the returns hold it: LEAVE's r1 is every caller's (F and A), and
    // SET's is H.
    let f = run_on(&[]);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H, A]), "{f:?}");
    // LEAVE leaves r1: the caller's own F after it, H after SET, and not
    // what LEAVE's other caller passed.
    let f = run_on(&[(LEAVE, 0b1111, 0b1111)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
}

/// A function that stores H in A1[r2] (so A1 and A2 are objects: A2[r2]
/// gets a number) and F at a constant address outside both, [`CELL`].
fn h_in_a1_f_in_a_global() -> Built {
    let e = 0x0800_1001;
    let mut a = B::default();
    let x = indexed(&mut a, 0x0800_0414, A1, 2, 3, 0);
    let h = a.pool(0x0800_0404, H);
    a.st(Width::Word, x, h);
    let y = indexed(&mut a, 0x0800_0418, A2, 2, 3, 0);
    let five = a.k(5);
    a.st(Width::Word, y, five);
    let f = a.pool(POOL, F);
    let c = a.k(CELL);
    a.st(Width::Word, c, f);
    (e, vec![(e, a.end(None, Edge::Return))])
}

#[test]
fn a_load_through_a_pointer_into_an_array_reads_that_array_alone() {
    // Through A1[r3]: H, stored there, and not F, which is in a global
    // of its own.
    let mut b = B::default();
    let x = indexed(&mut b, 0x0800_041c, A1, 3, 3, 0);
    let t = b.ld(Width::Word, x);
    let f = run_ram(&[h_in_a1_f_in_a_global(), calling(0x0800_2001, b, t)]);
    assert_eq!(site(&f).callees, BTreeSet::from([H]), "{f:?}");
    // And F stored at a constant address inside A1 is read through it.
    let e = 0x0800_3001;
    let mut a = B::default();
    let f = a.pool(POOL, F);
    let c = a.k(A1 + 0x30);
    a.st(Width::Word, c, f);
    let inside = (e, vec![(e, a.end(None, Edge::Return))]);
    let mut b = B::default();
    let x = indexed(&mut b, 0x0800_041c, A1, 3, 3, 0);
    let t = b.ld(Width::Word, x);
    let f = run_ram(&[h_in_a1_f_in_a_global(), inside, calling(0x0800_2001, b, t)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
}

#[test]
fn an_amount_that_varies_keeps_a_ram_address_in_its_object() {
    // `A1 + (0 or 0x200)`: the second lands on A2's start, but an index
    // stays in its array: F stored through it is A1's, and a load through
    // A2[r3] doesn't read it.
    let store = || {
        let e = 0x0800_3001;
        let mut a = B::default();
        let lit = a.pool(0x0800_0420, A1);
        let zero = a.k(0);
        let far = a.k(A2 - A1);
        let c = a.op(Op::GetFlag(Flag::Z));
        let k = a.op(Op::Select { c, a: zero, b: far });
        let p = a.bin(BinOp::Add, lit, k);
        let f = a.pool(POOL, F);
        a.st(Width::Word, p, f);
        (e, vec![(e, a.end(None, Edge::Return))])
    };
    let through = |base: u32| {
        let mut b = B::default();
        let x = indexed(&mut b, 0x0800_041c, base, 3, 3, 0);
        let t = b.ld(Width::Word, x);
        calling(0x0800_2001, b, t)
    };
    let f = run_ram(&[h_in_a1_f_in_a_global(), store(), through(A2)]);
    assert!(site(&f).callees.is_empty(), "{f:?}");
    let f = run_ram(&[h_in_a1_f_in_a_global(), store(), through(A1)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
    // One constant offset is exact: a field's address, wherever it is.
    let mut a = B::default();
    let lit = a.pool(0x0800_0420, A1);
    let far = a.k(A2 - A1);
    let p = a.bin(BinOp::Add, lit, far);
    let fp = a.pool(POOL, F);
    a.st(Width::Word, p, fp);
    let exact = (0x0800_3001, vec![(0x0800_3001, a.end(None, Edge::Return))]);
    let f = run_ram(&[h_in_a1_f_in_a_global(), exact, through(A2)]);
    assert_eq!(site(&f).callees, BTreeSet::from([F]), "{f:?}");
}

#[test]
fn a_callback_s_pointer_is_its_callers_once_every_call_is_seen() {
    // CB's address is taken (a pool word) and called through that word
    // with `&A1[r3]` in r0; CB calls through the word r0 points at.
    const CB: u32 = 0x0800_3201;
    let fns = || {
        let mut b = B::default();
        let p = b.get(0);
        let t = b.ld(Width::Word, p);
        let cb = calling(CB, b, t);
        let e = 0x0800_5001;
        let mut b = B::default();
        let x = indexed(&mut b, 0x0800_041c, A1, 3, 3, 0);
        b.set(0, x);
        let w = b.pool(0x0800_0440, CB);
        let mut call = b.end(
            Some(w),
            Edge::Call {
                target: None,
                ret: e + 0x10,
            },
        );
        call.1.at = 0x0800_0110;
        let caller = (
            e,
            vec![(e, call), (e + 0x10, B::default().end(None, Edge::Return))],
        );
        [h_in_a1_f_in_a_global(), cb, caller]
    };
    let img = || {
        let mut img = Img::new();
        img.fns.insert(CB);
        img.words.insert(0x0800_0440, CB);
        img
    };
    // With the RAM pointers alone, CB's r0 may be anything a caller the
    // run can't see passes: its load reads every cell, F's too.
    FORCE_RAM.with(|c| c.set(true));
    let f = run_img(&fns(), &BTreeMap::new(), &img());
    assert_eq!(site(&f).callees, BTreeSet::from([F, H]), "{f:?}");
    // With calls that wait, every call of CB is seen: r0 is `&A1[i]`,
    // and the load reads A1 alone.
    LEAVES.with(|c| c.borrow_mut().insert(1, (0, 0b1111)));
    let f = run_img(&fns(), &BTreeMap::new(), &img());
    LEAVES.with(|c| c.borrow_mut().clear());
    FORCE_RAM.with(|c| c.set(false));
    assert_eq!(site(&f).callees, BTreeSet::from([H]), "{f:?}");
    assert!(site(&f).complete, "{f:?}");
}

#[test]
fn the_last_array_ends_below_the_bios_s_ram() {
    // H is stored in A2[r2], the last array the code indexes. F stored at
    // the IRQ vector isn't in it: a load through A2[r3] reads H alone.
    let e = 0x0800_1001;
    let mut a = B::default();
    let y = indexed(&mut a, 0x0800_0414, A2, 2, 3, 0);
    let h = a.pool(0x0800_0404, H);
    a.st(Width::Word, y, h);
    let f = a.pool(POOL, F);
    let c = a.k(IRQ_VECTOR);
    a.st(Width::Word, c, f);
    let store = (e, vec![(e, a.end(None, Edge::Return))]);
    let mut b = B::default();
    let x = indexed(&mut b, 0x0800_041c, A2, 3, 3, 0);
    let t = b.ld(Width::Word, x);
    let f = run_ram(&[store, calling(0x0800_2001, b, t)]);
    assert_eq!(site(&f).callees, BTreeSet::from([H]), "{f:?}");
}

#[test]
fn one_pointer_of_several_plus_a_constant_is_exact() {
    // `p = A1 or 0x03000a00; p + (A2 - A1)`: a field's address through a
    // pointer with two targets, not a walk. F stored through it at A2 is
    // read through A2[r3] (and through the other target's field too).
    let e = 0x0800_3001;
    let mut a = B::default();
    let l1 = a.pool(0x0800_0420, A1);
    let l2 = a.k(0x0300_0a00);
    let c = a.op(Op::GetFlag(Flag::Z));
    let p = a.op(Op::Select { c, a: l1, b: l2 });
    let far = a.k(A2 - A1);
    let q = a.bin(BinOp::Add, p, far);
    let fp = a.pool(POOL, F);
    a.st(Width::Word, q, fp);
    let store = (e, vec![(e, a.end(None, Edge::Return))]);
    let mut b = B::default();
    let x = indexed(&mut b, 0x0800_041c, A2, 3, 3, 0);
    let t = b.ld(Width::Word, x);
    let f = run_ram(&[h_in_a1_f_in_a_global(), store, calling(0x0800_2001, b, t)]);
    assert!(site(&f).callees.contains(&F), "{f:?}");
}
