//! The escape analysis on hand-built IR: one case per [`Kind`] and a slot
//! that doesn't escape.

use super::*;
use armv4t::sem::Reg;

/// A block's IR, built by hand.
#[derive(Default)]
struct B {
    insts: Vec<Inst>,
}

impl B {
    fn op(&mut self, op: Op) -> ValueId {
        self.insts.push(Inst { op, guard: None });
        ValueId(self.insts.len() as u32 - 1)
    }
    fn k(&mut self, c: u32) -> ValueId {
        self.op(Op::Const(c))
    }
    fn get(&mut self, r: u8) -> ValueId {
        self.op(Op::GetReg(Reg(r)))
    }
    fn set(&mut self, r: u8, v: ValueId) {
        self.op(Op::SetReg(Reg(r), v));
    }
    fn add(&mut self, a: ValueId, b: ValueId) -> ValueId {
        self.op(Op::Bin(BinOp::Add, a, b))
    }
    /// sp + c (c may be negative), as a value.
    fn sp(&mut self, c: i32) -> ValueId {
        let sp = self.get(13);
        let k = self.k(c as u32);
        self.add(sp, k)
    }
    fn ld(&mut self, addr: ValueId) -> ValueId {
        self.op(Op::Load {
            width: Width::Word,
            addr,
        })
    }
    fn st(&mut self, addr: ValueId, value: ValueId) -> ValueId {
        self.op(Op::Store {
            width: Width::Word,
            addr,
            value,
        })
    }
    /// `push {r4, lr}; sub sp, #8`: sp is the entry's -16.
    fn prologue(&mut self) {
        for (o, r) in [(-8, 4), (-4, 14)] {
            let a = self.sp(o);
            let v = self.get(r);
            self.st(a, v);
        }
        let sp = self.sp(-16);
        self.set(13, sp);
    }
    fn exit(&mut self) -> usize {
        let t = self.k(0);
        self.op(Op::Exit(Exit::Jump(t))).index()
    }
}

/// Function `entry`'s blocks: each (key, IR, the edge its last inst takes).
fn func(entry: u32, bs: Vec<(u32, B, Edge)>) -> (u32, Vec<(u32, Vec<Inst>, Edge)>) {
    let bs = bs
        .into_iter()
        .map(|(k, mut b, e)| {
            b.exit();
            (k, b.insts, e)
        })
        .collect();
    (entry, bs)
}

type Fns = Vec<(u32, Vec<(u32, Vec<Inst>, Edge)>)>;

fn frames_of(fns: &Fns) -> BTreeMap<u32, Frame> {
    let fs: Vec<(u32, BTreeMap<u32, FBlock>)> = fns
        .iter()
        .map(|(e, bs)| {
            let m = bs
                .iter()
                .map(|(k, insts, edge)| {
                    (
                        *k,
                        FBlock {
                            insts,
                            edges: BTreeMap::from([(insts.len() - 1, edge.clone())]),
                        },
                    )
                })
                .collect();
            (*e, m)
        })
        .collect();
    frames(&fs, &AllFour)
}

/// [`frames_of`] where a call to `9` never returns.
fn frames_9_never_returns(fns: &Fns) -> BTreeMap<u32, Frame> {
    struct NeverReturns;
    impl Env for NeverReturns {
        fn params(&self, _: Option<u32>) -> u16 {
            0xf
        }
        fn returns(&self, t: Option<u32>) -> bool {
            t != Some(9)
        }
    }
    let fs: Vec<(u32, BTreeMap<u32, FBlock>)> = fns
        .iter()
        .map(|(e, bs)| {
            let m = bs
                .iter()
                .map(|(k, insts, edge)| {
                    (
                        *k,
                        FBlock {
                            insts,
                            edges: BTreeMap::from([(insts.len() - 1, edge.clone())]),
                        },
                    )
                })
                .collect();
            (*e, m)
        })
        .collect();
    frames(&fs, &NeverReturns)
}

/// The frame of a one-block function `b`, ending with `edge`.
fn one(b: B, edge: Edge) -> Frame {
    frames_of(&vec![func(1, vec![(1, b, edge)])])
        .remove(&1)
        .unwrap()
}

/// The slot at `lo` and how it escapes.
fn at(f: &Frame, lo: i32) -> &Slot {
    f.slots
        .iter()
        .find(|s| s.lo == lo)
        .unwrap_or_else(|| panic!("no slot at {lo}: {:?}", f.slots))
}

fn kinds(f: &Frame, lo: i32) -> Vec<Kind> {
    let e = at(f, lo).escapes;
    Kind::ALL
        .into_iter()
        .filter(|k| e & *k as u8 != 0)
        .collect()
}

#[test]
fn a_local_only_loaded_and_stored_through_sp_doesnt_escape() {
    let mut b = B::default();
    b.prologue();
    let a = b.sp(0); // the new sp: the entry's -16
    let x = b.get(0);
    let st = b.st(a, x).index();
    let a2 = b.sp(4);
    let ld = b.ld(a2).index();
    let r = b.k(0);
    b.set(0, r);
    let f = one(b, Edge::Return);
    assert_eq!(
        f.slots.iter().map(|s| (s.lo, s.hi)).collect::<Vec<_>>(),
        vec![(-16, -12), (-12, -8), (-8, -4), (-4, 0)]
    );
    assert!(f.slots.iter().all(|s| !s.escapes()), "{:?}", f.slots);
    assert!(at(&f, -8).saved && at(&f, -4).saved && !at(&f, -16).saved);
    assert_eq!(f.escapes_at(1, st), Some(false));
    assert_eq!(f.escapes_at(1, ld), Some(false));
    assert!(!f.lost);
}

#[test]
fn an_address_passed_to_a_callee_escapes_with_what_is_above_it() {
    let mut b = B::default();
    b.prologue();
    // A local at -12 the code writes; a pointer to -16 passed in r0.
    let a = b.sp(4);
    let x = b.k(7);
    b.st(a, x);
    let p = b.sp(0);
    b.set(0, p);
    let f = one(
        b,
        Edge::Call {
            target: Some(9),
            ret: 2,
        },
    );
    // -16 has no access of its own: only the callee writes it.
    assert_eq!((at(&f, -16).lo, at(&f, -16).hi), (-16, -16));
    assert_eq!(kinds(&f, -16), vec![Kind::Call]);
    // The object may reach -12, but not the saved registers.
    assert_eq!(kinds(&f, -12), vec![Kind::Call]);
    assert!(!at(&f, -8).escapes() && !at(&f, -4).escapes());
    assert_eq!(f.escaping(), 2);
}

#[test]
fn an_address_stored_to_memory_escapes() {
    let mut b = B::default();
    b.prologue();
    let a = b.sp(0);
    let x = b.k(1);
    b.st(a, x);
    let g = b.k(0x0300_0000);
    let p = b.sp(0);
    b.st(g, p);
    let f = one(b, Edge::Return);
    assert_eq!(kinds(&f, -16), vec![Kind::Stored]);
}

#[test]
fn an_address_stored_in_a_slot_escapes_when_that_slot_does() {
    // -12 holds a pointer to -16, and -12's address goes to a callee.
    let mut b = B::default();
    b.prologue();
    let a = b.sp(4);
    let p = b.sp(0);
    b.st(a, p);
    let q = b.sp(4);
    b.set(1, q);
    let f = one(
        b,
        Edge::Call {
            target: None,
            ret: 2,
        },
    );
    assert_eq!(kinds(&f, -16), vec![Kind::Stored]);
    // The object at -16 may reach -12 too.
    assert_eq!(kinds(&f, -12), vec![Kind::Call, Kind::Stored]);
    // A slot holding a pointer, its own address kept, keeps it.
    let mut b = B::default();
    b.prologue();
    let a = b.sp(4);
    let p = b.sp(0);
    b.st(a, p);
    let f = one(b, Edge::Return);
    assert!(f.slots.iter().all(|s| !s.escapes()), "{:?}", f.slots);
}

#[test]
fn a_pointer_spilled_and_reloaded_is_still_followed() {
    let mut b = B::default();
    b.prologue();
    let a = b.sp(4);
    let p = b.sp(0);
    b.st(a, p);
    let a2 = b.sp(4);
    let back = b.ld(a2);
    b.set(0, back);
    let f = one(
        b,
        Edge::Call {
            target: Some(9),
            ret: 2,
        },
    );
    // -12 holds the pointer; its own address didn't go anywhere, but it
    // sits above -16, inside the object the callee may reach, so what it
    // holds escapes again.
    assert_eq!(kinds(&f, -16), vec![Kind::Call, Kind::Stored]);
    assert_eq!(kinds(&f, -12), vec![Kind::Call, Kind::Stored]);
}

#[test]
fn an_address_returned_escapes() {
    let mut b = B::default();
    b.prologue();
    let p = b.sp(8); // the entry's -8: a saved register's slot
    b.set(0, p);
    let f = one(b, Edge::Return);
    assert_eq!(kinds(&f, -8), vec![Kind::Returned]);
    assert!(!at(&f, -4).escapes());
}

#[test]
fn arithmetic_the_run_cant_follow_escapes() {
    // A local array at -16..-8, indexed by r1.
    let mut b = B::default();
    b.prologue();
    let a = b.sp(0);
    let x = b.k(0);
    b.st(a, x);
    let base = b.sp(0);
    let i = b.get(1);
    let four = b.k(2);
    let off = b.op(Op::Shift {
        kind: armv4t::sem::ShiftType::Lsl,
        v: i,
        amount: four,
    });
    let p = b.add(base, off);
    let ld = b.ld(p).index();
    let f = one(b, Edge::Return);
    assert_eq!(kinds(&f, -16), vec![Kind::Arithmetic]);
    assert_eq!(f.places[&(1, ld)], Place::Through(BTreeSet::from([-16])));
    assert_eq!(f.escapes_at(1, ld), Some(true));
}

#[test]
fn a_callees_stack_arguments_escape() {
    // The caller writes an argument at its sp, the callee reads it at its
    // entry sp.
    let mut b = B::default();
    b.prologue();
    let a = b.sp(0);
    let x = b.k(5);
    b.st(a, x);
    let a = b.sp(4);
    b.st(a, x);
    let caller = func(
        1,
        vec![(
            1,
            b,
            Edge::Call {
                target: Some(9),
                ret: 2,
            },
        )],
    );
    let mut c = B::default();
    let a = c.sp(0);
    c.ld(a);
    let callee = func(9, vec![(9, c, Edge::Return)]);
    let fs = frames_of(&vec![caller, callee]);
    assert_eq!(fs[&9].args, 4);
    assert_eq!(kinds(&fs[&1], -16), vec![Kind::Args]);
    assert!(!at(&fs[&1], -12).escapes());
}

#[test]
fn an_address_passed_as_a_stack_argument_escapes() {
    // `f(1, 2, 3, 4, &x)`: the argument word at sp (-16) holds &x (-12),
    // and nothing else escapes.
    let mut b = B::default();
    b.prologue();
    let x = b.sp(4);
    let k = b.k(9);
    b.st(x, k);
    let a = b.sp(0);
    let p = b.sp(4);
    b.st(a, p);
    let caller = func(
        1,
        vec![(
            1,
            b,
            Edge::Call {
                target: Some(9),
                ret: 2,
            },
        )],
    );
    let mut c = B::default();
    let a = c.sp(0);
    c.ld(a);
    let fs = frames_of(&vec![caller, func(9, vec![(9, c, Edge::Return)])]);
    assert_eq!(kinds(&fs[&1], -16), vec![Kind::Args]);
    assert_eq!(kinds(&fs[&1], -12), vec![Kind::Stored]);
}

#[test]
fn sp_lost_escapes_every_slot() {
    let mut b = B::default();
    b.prologue();
    let a = b.sp(0);
    let x = b.k(5);
    b.st(a, x);
    let r = b.get(0);
    b.set(13, r);
    let f = one(b, Edge::Return);
    assert!(f.lost);
    assert!(f.slots.iter().all(|s| s.escapes & Kind::Lost as u8 != 0));
    // sp set from a parameter: a stack switch, at the block's start (the
    // hand-built block has no instruction marks).
    assert_eq!(f.switches, BTreeSet::from([0]));
}

#[test]
fn a_stack_switch_is_named_at_the_instruction_its_sp_came_from() {
    // A task switcher's resume: `push {r4-r11, lr}` (a store and sp
    // moved), `mov sp, r2`, `pop {r4-r11}`, which the IR folds into one
    // write of sp: r2 + 32, made at the `mov`.
    let mut b = B::default();
    b.op(Op::Insn(0x100));
    let sp = b.sp(-36);
    b.set(13, sp);
    b.op(Op::Insn(0x104));
    let r2 = b.get(2);
    b.op(Op::Insn(0x108));
    let k = b.k(32);
    let popped = b.add(r2, k);
    b.set(13, popped);
    // Arithmetic on the new sp after it is no second switch.
    b.op(Op::Insn(0x10c));
    let k = b.k(4);
    let more = b.add(popped, k);
    b.set(13, more);
    let f = one(b, Edge::Return);
    assert_eq!(f.switches, BTreeSet::from([0x104]));

    // A yield's `ldr sp, [r3]`, then its pop: at the load.
    let mut b = B::default();
    b.op(Op::Insn(0x200));
    let g = b.k(0x0300_4c94);
    let main = b.ld(g);
    b.op(Op::Insn(0x204));
    let k = b.k(36);
    let popped = b.add(main, k);
    b.set(13, popped);
    assert_eq!(one(b, Edge::Return).switches, BTreeSet::from([0x200]));
}

#[test]
fn sp_from_the_frame_is_no_stack_switch() {
    // `sub sp, #16`, a frame pointer's `mov sp, r7` with r7 = sp + a run-time
    // amount (a variable-length array), and `add sp, #16`.
    let mut b = B::default();
    b.prologue();
    let sp = b.get(13);
    let n = b.get(0);
    let r7 = b.add(sp, n);
    b.set(7, r7);
    let r7 = b.get(7);
    b.set(13, r7);
    let up = b.sp(16);
    b.set(13, up);
    let f = one(b, Edge::Return);
    assert!(f.switches.is_empty(), "{:?}", f.switches);
    assert!(f.lost);

    // APCS's epilogue, `ldmdb r11, {r11, sp, lr}`, with r11 a copy of sp
    // the run lost the offset of (a conditional epilogue before it): the
    // sp loaded through it is a restore.
    let mut b = B::default();
    b.prologue();
    let sp = b.get(13);
    let n = b.get(0);
    let r11 = b.add(sp, n);
    let k = b.k(4);
    let slot = b.add(r11, k);
    let saved = b.ld(slot);
    b.set(13, saved);
    assert!(one(b, Edge::Return).switches.is_empty());
}

#[test]
fn two_paths_with_different_pointers_join_to_both() {
    // Block 1 puts sp-16 or sp-12 in r0 (two predecessors of block 3), and
    // block 3 passes it.
    let mut b1 = B::default();
    b1.prologue();
    let p = b1.sp(0);
    b1.set(0, p);
    let a = b1.sp(4);
    let x = b1.k(0);
    b1.st(a, x);
    let mut b2 = B::default();
    let p = b2.sp(4);
    b2.set(0, p);
    let b3 = B::default();
    let fs = frames_of(&vec![func(
        1,
        vec![
            (1, b1, Edge::Goto(vec![2, 3])),
            (2, b2, Edge::Goto(vec![3])),
            (
                3,
                b3,
                Edge::Call {
                    target: None,
                    ret: 4,
                },
            ),
        ],
    )]);
    let f = &fs[&1];
    assert_eq!(kinds(f, -16), vec![Kind::Call]);
    assert_eq!(kinds(f, -12), vec![Kind::Call]);
}

/// A caller that keeps words at -16 and -12 and calls 9, which passes on
/// the address of its stack argument at its sp + 4 (the caller's -12).
fn reaching_up() -> Fns {
    let mut b = B::default();
    b.prologue();
    let x = b.k(5);
    let a = b.sp(0);
    b.st(a, x);
    let a = b.sp(4);
    b.st(a, x);
    let caller = func(
        1,
        vec![(
            1,
            b,
            Edge::Call {
                target: Some(9),
                ret: 2,
            },
        )],
    );
    let mut c = B::default();
    let p = c.sp(4);
    c.set(0, p);
    let callee = func(
        9,
        vec![(
            9,
            c,
            Edge::Call {
                target: Some(10),
                ret: 11,
            },
        )],
    );
    vec![caller, callee]
}

#[test]
fn a_callee_passing_on_a_stack_arguments_address_reaches_up_into_the_frame() {
    let fs = frames_of(&reaching_up());
    // It reads nothing exactly, but from its sp + 4 up the object is of
    // unknown size: up to the saved registers.
    assert_eq!(fs[&9].args, 0);
    assert_eq!(kinds(&fs[&1], -12), vec![Kind::Args]);
    assert!(!at(&fs[&1], -16).escapes() && !at(&fs[&1], -8).escapes());
    assert!(fs[&1].flush.is_empty());
}

#[test]
fn before_a_callee_that_reaches_up_and_never_returns_the_locals_go_to_memory() {
    let fns = reaching_up();
    let fs = frames_9_never_returns(&fns);
    assert!(
        fs[&1].slots.iter().all(|s| !s.escapes()),
        "{:?}",
        fs[&1].slots
    );
    let exit = fns[0].1[0].1.len() - 1;
    assert_eq!(fs[&1].flush, BTreeSet::from([(1, exit)]));
}

#[test]
fn locals_are_the_frames_own_slots_that_dont_escape() {
    // Words at -16 and -12, a byte at -20 and a halfword at -18 (one slot
    // of bytes, as they don't overlap: two), a word read at the entry sp
    // (the caller's), and a word stored at -30 (unaligned).
    let mut b = B::default();
    b.prologue();
    let x = b.k(5);
    let w0 = b.sp(0);
    let st0 = b.st(w0, x).index();
    let w4 = b.sp(4);
    b.st(w4, x);
    let by = b.sp(-4);
    let stb = b
        .op(Op::Store {
            width: Width::Byte,
            addr: by,
            value: x,
        })
        .index();
    let h = b.sp(-2);
    let ldh = b
        .op(Op::Load {
            width: Width::Half,
            addr: h,
        })
        .index();
    let up = b.sp(16);
    let ldu = b.ld(up).index();
    let odd = b.sp(-14);
    b.st(odd, x);
    let f = one(b, Edge::Return);
    let locals = f.locals();
    let at_lo = |lo: i32| locals.iter().find(|l| l.lo == lo).copied();
    assert_eq!(
        at_lo(-16),
        Some(Local {
            lo: -16,
            len: 4,
            word: true
        })
    );
    assert!(at_lo(-12).is_some_and(|l| l.word));
    assert!(at_lo(-20).is_some_and(|l| !l.word && l.len == 1));
    assert!(at_lo(-18).is_some_and(|l| !l.word && l.len == 2));
    // The saved registers' slots are the function's own too.
    assert!(at_lo(-8).is_some() && at_lo(-4).is_some());
    assert!(at_lo(0).is_none() && at_lo(-30).is_none());
    assert_eq!(
        f.local_at(&locals, 1, st0).map(|(l, d, n)| (l.lo, d, n)),
        Some((-16, 0, 4))
    );
    assert_eq!(
        f.local_at(&locals, 1, stb).map(|(l, d, n)| (l.lo, d, n)),
        Some((-20, 0, 1))
    );
    assert_eq!(
        f.local_at(&locals, 1, ldh).map(|(l, d, n)| (l.lo, d, n)),
        Some((-18, 0, 2))
    );
    assert_eq!(f.local_at(&locals, 1, ldu), None);
    assert!(f.in_callers(1, ldu) && !f.in_callers(1, st0));
    assert_eq!(
        Local {
            lo: -16,
            len: 4,
            word: true
        }
        .name(),
        "l16"
    );
}
