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
/// A literal pool word holding `F`.
const POOL: u32 = 0x0800_0400;
/// A vtable in ROM data: `[0, 0, 0, F]`.
const VT: u32 = 0x0800_0500;
/// A RAM word.
const CELL: u32 = 0x0300_0100;

struct Img {
    words: BTreeMap<u32, u32>,
}

impl Img {
    fn new() -> Img {
        Img {
            words: BTreeMap::from([(POOL, F), (VT + 12, F), (0x0800_0404, H)]),
        }
    }
}

impl Image for Img {
    fn is_fn(&self, v: u32) -> bool {
        v == F || v == H
    }
    fn word(&self, a: u32) -> Option<u32> {
        (0x0800_0000..0x0800_1000)
            .contains(&a)
            .then(|| self.words.get(&(a & !3)).copied().unwrap_or(0))
    }
    fn bound(&self, _: u32) -> u32 {
        0x0800_1000
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
            frame: None,
        })
        .collect();
    follow(&ins, &Img::new(), &[])
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
