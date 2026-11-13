//! The value domain: join, widening, and narrowing on `cmp` and the
//! conditional branches, run through the real semantics.

use armv4t::decode::Cond;
use armv4t::sem::Reg;

use super::domain::{AbsVal, Bit, Dom, ENTRY, MAX_SET, Ranges, Sym};
use super::exec::{State, split, step};
use super::func::Bl;
use crate::program::{Program, ROM_BASE, Region};

const R0: Reg = Reg(0);

/// A Thumb program of `code` at the start of ROM.
fn thumb(code: &[u16]) -> Program {
    let rom: Vec<u8> = code.iter().flat_map(|h| h.to_le_bytes()).collect();
    let end = ROM_BASE + rom.len() as u32;
    Program::new(
        rom,
        vec![Region {
            start: ROM_BASE,
            end,
            thumb: true,
        }],
        vec![],
    )
}

/// Run every instruction of `p` in order on `st`, falling through each.
fn run(p: &Program, mut st: State) -> State {
    let sp0 = State::sp0(ROM_BASE);
    let mut a = ROM_BASE;
    while let Some(i) = p.decode(a, true) {
        st = step(p, sp0, 0, &i, st).fall.expect("falls through");
        a = i.next();
    }
    st
}

fn entry() -> State {
    State::entry(ROM_BASE)
}

/// The states where `c` holds and doesn't after running `p`.
fn branch(p: &Program, st: State, c: Cond) -> (State, State) {
    let at = ROM_BASE + 0x100;
    let (t, f) = split(p, State::sp0(ROM_BASE), 0, at, c, run(p, st));
    (
        t.expect("taken is feasible"),
        f.expect("not taken is feasible"),
    )
}

fn dom(st: &State, r: u8) -> &Dom {
    &st.reg(Reg(r)).dom
}

fn si(s: u32, lo: u32, hi: u32) -> Dom {
    Dom::si(s, lo, hi)
}

/// The members of a small set, in order.
fn members(d: &Dom) -> Vec<u32> {
    d.elems().expect("a small set")
}

// Thumb encodings.
fn cmp_imm(rn: u16, k: u16) -> u16 {
    0x2800 | rn << 8 | k
}
fn movs_imm(rd: u16, k: u16) -> u16 {
    0x2000 | rd << 8 | k
}
fn lsls(rd: u16, rm: u16, n: u16) -> u16 {
    n << 6 | rm << 3 | rd
}
fn subs_imm3(rd: u16, rn: u16, k: u16) -> u16 {
    0x1e00 | k << 6 | rn << 3 | rd
}

#[test]
fn ranges() {
    let r = Ranges::new(8, u32::MAX);
    assert_eq!(r.complement(), Ranges::new(0, 7));
    assert!(Ranges::empty().complement().is_full());
    // Shifting wraps, splitting the range.
    let w = Ranges::new(0, 7).shift(u32::MAX - 3);
    assert!(w.contains(u32::MAX - 3) && w.contains(3) && !w.contains(4));
    assert_eq!(w.shift(4), Ranges::new(0, 7));
    assert_eq!(r.intersect(&Ranges::new(0, 9)), Ranges::new(8, 9));
}

#[test]
fn join_of_sets() {
    let a = Dom::Const(1).join(&Dom::Const(3));
    assert_eq!(a, Dom::Set(vec![1, 3].into()));
    // A small interval joins exactly; a big one by its strided hull.
    assert_eq!(
        members(&si(4, 0, 20).join(&Dom::Const(2))),
        [0, 2, 4, 8, 12, 16, 20]
    );
    assert_eq!(si(4, 0, 1 << 20).join(&Dom::Const(2)), si(2, 0, 1 << 20));
    // Past MAX_SET constants a set becomes their strided hull.
    let many = Dom::from_vals((0..=MAX_SET as u32).map(|x| 8 * x).collect());
    assert_eq!(many, si(8, 0, 8 * MAX_SET as u32));
    assert_eq!(si(1, 0, 5).join(&Dom::Top), Dom::Top);
}

#[test]
fn widening_only_moves_what_changed() {
    let mut a = entry();
    let mut b = entry();
    a.set_reg(R0, AbsVal::konst(1));
    b.set_reg(R0, AbsVal::konst(2));
    a.set_reg(Reg(1), AbsVal::konst(7));
    b.set_reg(Reg(1), AbsVal::konst(7));
    let name = |n| Sym::new(ENTRY, 0, 0, n);
    let j = a.join(&b, name, false);
    assert_eq!(*dom(&j, 0), Dom::Set(vec![1, 2].into()));
    let w = a.join(&b, name, true);
    assert_eq!(*dom(&w, 0), Dom::Top);
    assert_eq!(*dom(&w, 1), Dom::Const(7));
}

#[test]
fn cmp_then_bhi_and_bls() {
    let p = thumb(&[cmp_imm(0, 7)]);
    let (hi, ls) = branch(&p, entry(), Cond::Hi);
    assert_eq!(*dom(&hi, 0), si(1, 8, u32::MAX));
    assert_eq!(*dom(&ls, 0), si(1, 0, 7));
    let (ls, hi) = branch(&p, entry(), Cond::Ls);
    assert_eq!(*dom(&ls, 0), si(1, 0, 7));
    assert_eq!(*dom(&hi, 0), si(1, 8, u32::MAX));
    // Unsigned lower: bcc is r0 < 7.
    let (lo, _) = branch(&p, entry(), Cond::Cc);
    assert_eq!(*dom(&lo, 0), si(1, 0, 6));
}

#[test]
fn a_decided_branch_has_one_side() {
    let p = thumb(&[movs_imm(0, 3), cmp_imm(0, 7)]);
    let (t, f) = split(&p, State::sp0(ROM_BASE), 0, 0, Cond::Hi, run(&p, entry()));
    assert!(t.is_none());
    assert_eq!(*dom(&f.unwrap(), 0), Dom::Const(3));
}

#[test]
fn narrowing_reaches_related_registers() {
    // subs r0, r5, #1 ; cmp r0, #7 ; bls: r0 in 0..=7 and r5 in 1..=8.
    let p = thumb(&[subs_imm3(0, 5, 1), cmp_imm(0, 7)]);
    let (ls, _) = branch(&p, entry(), Cond::Ls);
    assert_eq!(*dom(&ls, 0), si(1, 0, 7));
    assert_eq!(*dom(&ls, 5), si(1, 1, 8));
}

#[test]
fn narrowing_reaches_derived_values() {
    // lsls r4, r6, #2 ; cmp r6, #5 ; bls: the index was scaled before the
    // check, and is still bounded by it.
    let p = thumb(&[lsls(4, 6, 2), cmp_imm(6, 5)]);
    let (ls, _) = branch(&p, entry(), Cond::Ls);
    assert_eq!(members(dom(&ls, 4)), [0, 4, 8, 12, 16, 20]);
}

#[test]
fn relation_and_flags_survive_a_join() {
    // Two paths reach a compare's branch: on one the counter is still the
    // constant 0 (the first trip round a loop), on the other it's unknown.
    let p = thumb(&[cmp_imm(6, 5)]);
    let mut first = entry();
    first.set_reg(Reg(6), AbsVal::konst(0));
    let (a, b) = (run(&p, first), run(&p, entry()));
    assert!(matches!(
        a.flags[2],
        Bit::Pred {
            now: Some(false),
            ..
        }
    ));
    let j = a.join(&b, |n| Sym::new(ENTRY, 1, 0, n), false);
    let (hi, ls) = split(&p, State::sp0(ROM_BASE), 0, 0, Cond::Hi, j);
    assert_eq!(*dom(&hi.unwrap(), 6), si(1, 6, u32::MAX));
    assert_eq!(*dom(&ls.unwrap(), 6), si(1, 0, 5));

    // r0 = r5 - 1 on both sides, over different unknowns: still related.
    let p = thumb(&[subs_imm3(0, 5, 1)]);
    let mut other = entry();
    other.set_reg(Reg(5), AbsVal::top(Sym::new(ENTRY, 9, 0, 5)));
    let j = run(&p, entry()).join(&run(&p, other), |n| Sym::new(ENTRY, 2, 0, n), false);
    let (r0, r5) = (j.reg(R0), j.reg(Reg(5)));
    assert_eq!((r0.sym, r0.off.wrapping_add(1)), (r5.sym, r5.off));
}

#[test]
fn masks_that_change_nothing() {
    // lsrs r0, r1, #28 ; movs r2, #0 ; bics r0, r2: an all-ones mask.
    let st = run(&thumb(&[0x0f08, 0x2200, 0x4390]), entry());
    assert_eq!(*dom(&st, 0), si(1, 0, 15));
}

#[test]
fn reads_note_code_outside_the_function() {
    // push {lr}; bl leaf; pop {pc}; leaf: movs r0, #1; bx lr
    let p = thumb(&[0xb500, 0xf000, 0xf801, 0xbd00, 0x2001, 0x4770]);
    let r = super::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    let insns: Vec<u32> = r.insns.iter().map(|&(a, _)| a).collect();
    assert_eq!(insns, [ROM_BASE, ROM_BASE + 2, ROM_BASE + 6]);
    // The leaf runs inline: new data over it would change the result.
    for a in [ROM_BASE + 8, ROM_BASE + 10] {
        assert!(r.reads.iter().any(|&(lo, hi)| (lo..hi).contains(&a)));
    }
}

/// The BIOS copy `swi n` makes with r2 = `ctl`, copying from the start of
/// ROM to IWRAM.
fn copy(n: u16, ctl: u32) -> Vec<(u32, u32, u32)> {
    let mut code = vec![
        0x4802, // ldr r0, [pc, #8] -> 0x0c
        0x4903, // ldr r1, [pc, #12] -> 0x10
        0x4a03, // ldr r2, [pc, #12] -> 0x14
        0xdf00 | n,
        0x4770,
        0x46c0,
    ];
    for w in [ROM_BASE, 0x0300_0000, ctl] {
        code.extend([w as u16, (w >> 16) as u16]);
    }
    code.resize(0x20, 0x46c0);
    let p = thumb(&code);
    let r = super::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    r.copies.into_iter().collect()
}

#[test]
fn bios_copies_out_of_rom() {
    let to = |n| vec![(ROM_BASE, 0x0300_0000, n)];
    // CpuSet: halfwords, or words with bit 26.
    assert_eq!(copy(0x0b, 4), to(8));
    assert_eq!(copy(0x0b, 1 << 26 | 4), to(16));
    // CpuFastSet copies 8 words at a time.
    assert_eq!(copy(0x0c, 3), to(32));
    // Fills, empty copies, other calls and sources past ROM don't count.
    assert_eq!(copy(0x0b, 1 << 24 | 4), []);
    assert_eq!(copy(0x0b, 0), []);
    assert_eq!(copy(0x05, 4), []);
    assert_eq!(copy(0x0b, 1 << 26 | 0x100), []);
}

#[test]
fn bios_copies_through_a_wrapper() {
    // ldr r0-r2 from the pool; bl cpuset; bx lr; cpuset: swi 0x0b; bx lr.
    let mut code = vec![
        0x4805, 0x4906, 0x4a06, 0xf000, 0xf803, 0x4770, 0x46c0, 0x46c0, 0xdf0b, 0x4770, 0x46c0,
        0x46c0,
    ];
    for w in [ROM_BASE, 0x0300_0000, 1 << 26 | 4] {
        code.extend([w as u16, (w >> 16) as u16]);
    }
    let p = thumb(&code);
    let r = super::func::analyze(&p, ROM_BASE, true, &mut |_, _| Bl::Call);
    assert_eq!(
        r.copies.into_iter().collect::<Vec<_>>(),
        [(ROM_BASE, 0x0300_0000, 16)]
    );
}

/// The compilers' trap encodings are traps; other undefined encodings
/// (Thumb 0xde00, an ARM `push`'s high half read as Thumb) are not.
#[test]
fn compiler_traps_are_known_by_encoding() {
    use super::func::is_trap;
    assert!(is_trap(0xdeff, true));
    assert!(is_trap(0xdefe, true));
    assert!(is_trap(0xe7ff_defe, false));
    assert!(is_trap(0xe7f0_00f0, false));
    assert!(!is_trap(0xde00, true));
    assert!(!is_trap(0xe92d, true));
    assert!(!is_trap(0xdeff, false));
}
