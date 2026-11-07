//! The function-address flow on hand-built IR: each use section 4
//! allows (a call, a store and its load, an equality compare, the halves
//! split and joined, `| 1`, a vtable word, an argument), and the ones it
//! doesn't (arithmetic, an address, an ordered compare), each a hole.

use super::*;
use armv4t::sem::Reg;

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
}

impl Img {
    fn new() -> Img {
        Img {
            words: BTreeMap::from([(POOL, F), (VT + 12, F), (0x0800_0404, H), (POOL_A, A)]),
            top: 0x0800_1000,
        }
    }
}

impl Image for Img {
    fn is_fn(&self, v: u32) -> bool {
        v == F || v == H || v == A
    }
    fn word(&self, a: u32) -> Option<u32> {
        (0x0800_0000..self.top)
            .contains(&a)
            .then(|| self.words.get(&(a & !3)).copied().unwrap_or(0))
    }
    fn bound(&self, _: u32) -> u32 {
        self.top
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
        };
        (self.insts, out, self.pools)
    }
}

type Built = (u32, Vec<(u32, (Vec<Inst>, Out, BTreeMap<usize, u32>))>);

fn run(fns: &[Built]) -> Flow {
    run_with(fns, &BTreeMap::new())
}

/// With frames: function entry -> its frame.
fn run_with(fns: &[Built], frames: &BTreeMap<u32, Frame>) -> Flow {
    run_img(fns, frames, &Img::new())
}

fn run_img(fns: &[Built], frames: &BTreeMap<u32, Frame>, img: &Img) -> Flow {
    run_allocs(fns, frames, img, &BTreeSet::new())
}

/// With allocators named.
fn run_allocs(
    fns: &[Built],
    frames: &BTreeMap<u32, Frame>,
    img: &Img,
    allocs: &BTreeSet<u32>,
) -> Flow {
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
                            hulls: BTreeMap::new(),
                        },
                    )
                })
                .collect(),
            frame: frames.get(e),
        })
        .collect();
    follow(&ins, img, &[], allocs, usize::MAX)
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
    b.op(Op::SetFlag(armv4t::sem::Flag::Z, z));
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
    b.op(Op::SetFlag(armv4t::sem::Flag::C, c));
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

/// An allocator: returns (what it returns is the call's new object).
const ALLOC: u32 = 0x0800_3001;

/// Two objects from two allocator calls: F stored in the first, a number
/// in the second, the second's word read and used as a number, the
/// first's called. With the allocator named the objects are apart, so F
/// is proven and the call's set is it; unnamed, both pointers are
/// unknown, the stores go to the pile, and the number read is F too.
#[test]
fn two_heap_objects_are_kept_apart() {
    let e = 0x0800_1001;
    let call_at = |b: B, ret: u32, at: u32| {
        let mut x = b.end(
            None,
            Edge::Call {
                target: Some(ALLOC),
                ret,
            },
        );
        x.1.at = at;
        x
    };
    let mut keep = B::default();
    let a = keep.get(0);
    keep.set(4, a);
    let mut c = B::default();
    let a = c.get(4);
    let b = c.get(0);
    let p = c.pool(POOL, F);
    c.st(Width::Word, a, p);
    let five = c.k(5);
    c.st(Width::Word, b, five);
    let n = c.ld(Width::Word, b);
    let four = c.k(4);
    c.bin(BinOp::Add, n, four);
    let t = c.ld(Width::Word, a);
    let f = vec![
        (e, call_at(B::default(), e + 0x10, 0x0800_1100)),
        (e + 0x10, call_at(keep, e + 0x20, 0x0800_1110)),
        (
            e + 0x20,
            c.end(
                Some(t),
                Edge::Call {
                    target: None,
                    ret: e + 0x30,
                },
            ),
        ),
        (e + 0x30, B::default().end(None, Edge::Return)),
    ];
    let alloc = (ALLOC, vec![(ALLOC, B::default().end(None, Edge::Return))]);
    let fns = [(e, f), alloc];
    let named = run_allocs(
        &fns,
        &BTreeMap::new(),
        &Img::new(),
        &BTreeSet::from([ALLOC]),
    );
    assert!(named.proven(POOL), "{named:?}");
    assert_eq!(site(&named).callees, BTreeSet::from([F]));
    assert!(site(&named).complete);
    assert!(named.piled.is_empty(), "{named:?}");
    let unnamed = run(&fns);
    assert!(!unnamed.proven(POOL));
    assert_eq!(unnamed.holes[&Origin::Word(POOL)].1, Why::Arithmetic);
}

/// Two words of a function's frame reached through sp-relative
/// addresses the frame's run didn't place: each is its own cell of the
/// frame's object, so the number read from one isn't F stored in the
/// other.
#[test]
fn two_frame_words_are_kept_apart() {
    let mut b = B::default();
    let sp = b.get(13);
    let eight = b.k(8);
    let a = b.bin(BinOp::Sub, sp, eight);
    let sixteen = b.k(16);
    let c = b.bin(BinOp::Sub, sp, sixteen);
    let p = b.pool(POOL, F);
    b.st(Width::Word, a, p);
    let five = b.k(5);
    b.st(Width::Word, c, five);
    let n = b.ld(Width::Word, c);
    let four = b.k(4);
    b.bin(BinOp::Add, n, four);
    let t = b.ld(Width::Word, a);
    let f = run(&[calling(0x0800_1001, b, t)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(site(&f).callees, BTreeSet::from([F]));
    assert!(site(&f).complete);
}

/// A pointer into a frame plus an index the run doesn't know may reach
/// any word of the frame: the load reads F's word too, and the number
/// made of it is F's hole.
#[test]
fn a_pointer_into_an_object_through_an_add_of_an_unknown_reads_all_of_it() {
    let mut b = B::default();
    let sp = b.get(13);
    let eight = b.k(8);
    let a = b.bin(BinOp::Sub, sp, eight);
    let p = b.pool(POOL, F);
    b.st(Width::Word, a, p);
    // sp - 16 + an unknown index: anywhere in the frame, F's word too.
    let sixteen = b.k(16);
    let c = b.bin(BinOp::Sub, sp, sixteen);
    let i = b.get(1);
    let ci = b.bin(BinOp::Add, c, i);
    let n = b.ld(Width::Word, ci);
    let four = b.k(4);
    b.bin(BinOp::Add, n, four);
    let f = run(&[(0x0800_1001, vec![(0x0800_1001, b.end(None, Edge::Return))])]);
    assert!(!f.proven(POOL), "{f:?}");
    assert_eq!(f.holes[&Origin::Word(POOL)].1, Why::Arithmetic);
}

/// Two callers pass one callee a pointer each into their own frames
/// (`native/fnref-context`): one stored F there, the other 5. The callee
/// returns what its pointer reads; run per call site, the first caller
/// gets back only F (and calls it), the second only 5 (and adds to it),
/// so F's word is proven. Joined at the callee's entry, the add would be
/// a hole of F's word.
#[test]
fn a_callee_s_values_are_kept_apart_by_call_site() {
    let (x, y, c) = (0x0800_1001, 0x0800_2001, 0x0800_3001);
    let at = |mut b: (Vec<Inst>, Out, BTreeMap<usize, u32>), at: u32| {
        b.1.at = at;
        b
    };
    let call = |b: B, ret: u32, site: u32| {
        at(
            b.end(
                None,
                Edge::Call {
                    target: Some(c),
                    ret,
                },
            ),
            site,
        )
    };
    // A caller: its frame's word at sp - 8 holds `v`, r0 points to it.
    let pass = |v: &dyn Fn(&mut B) -> ValueId| {
        let mut b = B::default();
        let sp = b.get(13);
        let eight = b.k(8);
        let a = b.bin(BinOp::Sub, sp, eight);
        let v = v(&mut b);
        b.st(Width::Word, a, v);
        b.set(0, a);
        b
    };
    let mut use_x = B::default();
    let r = use_x.get(0);
    let fx = vec![
        (x, call(pass(&|b| b.pool(POOL, F)), x + 0x10, 0x0800_1100)),
        (
            x + 0x10,
            at(
                use_x.end(
                    Some(r),
                    Edge::Call {
                        target: None,
                        ret: x + 0x20,
                    },
                ),
                0x0800_1110,
            ),
        ),
        (x + 0x20, B::default().end(None, Edge::Return)),
    ];
    let mut use_y = B::default();
    let r = use_y.get(0);
    let four = use_y.k(4);
    use_y.bin(BinOp::Add, r, four);
    let fy = vec![
        (y, call(pass(&|b| b.k(5)), y + 0x10, 0x0800_2100)),
        (y + 0x10, use_y.end(None, Edge::Return)),
    ];
    let mut body = B::default();
    let a = body.get(0);
    let n = body.ld(Width::Word, a);
    body.set(0, n);
    let fc = vec![(c, body.end(None, Edge::Return))];
    let f = run(&[(x, fx), (y, fy), (c, fc)]);
    assert!(f.proven(POOL), "{f:?}");
    assert_eq!(f.sites[&0x0800_1110].callees, BTreeSet::from([F]));
    assert!(f.sites[&0x0800_1110].complete);
}
