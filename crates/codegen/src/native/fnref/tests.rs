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
    /// Code in ROM, where a run of data ends (none: it runs to `top`).
    code: BTreeSet<u32>,
    /// More functions' keys.
    fns: BTreeSet<u32>,
}

impl Img {
    fn new() -> Img {
        Img {
            words: BTreeMap::from([(POOL, F), (VT + 12, F), (0x0800_0404, H), (POOL_A, A)]),
            top: 0x0800_1000,
            code: BTreeSet::new(),
            fns: BTreeSet::new(),
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
    follow(&ins, img, &[])
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
    g.mem_log.push((false, r, 4));
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
            "stored and loaded through unknown addresses",
            Width::Word,
            Box::new(|b, x| {
                let (p, q) = (b.get(2), b.get(3));
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
                let (p, q) = (b.get(2), b.get(3));
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
