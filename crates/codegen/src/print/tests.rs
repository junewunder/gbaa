//! The printed shape of small hand-written blocks: what is bound, what is
//! inlined, and what must stay where it was.

use super::Printer;
use crate::emit::pretty;
use analysis::ir::{BinOp, Inst, Op, ValueId};
use armv4t::sem::{Flag, Reg, ShiftType, Width};
use quote::quote;

/// A block under construction; each push returns the inst's value.
#[derive(Default)]
struct B(Vec<Inst>);

impl B {
    fn op(&mut self, op: Op) -> ValueId {
        self.guarded(op, None)
    }

    fn guarded(&mut self, op: Op, guard: Option<ValueId>) -> ValueId {
        self.0.push(Inst { op, guard });
        ValueId(self.0.len() as u32 - 1)
    }

    fn c(&mut self, x: u32) -> ValueId {
        self.op(Op::Const(x))
    }

    fn reg(&mut self, r: u8) -> ValueId {
        self.op(Op::GetReg(Reg(r)))
    }

    fn set(&mut self, r: u8, x: ValueId) {
        self.op(Op::SetReg(Reg(r), x));
    }

    /// The block's statements as `lower_block` prints them (no stalls,
    /// guards as plain `if`s), pretty-printed with one space between
    /// tokens' lines collapsed, so tests can match on one line.
    fn print(&self) -> String {
        self.print_stalled(&[])
    }

    /// [`Self::print`], with a `rt::stall` before each inst in `stalls`
    /// (printed as `stall();`).
    fn print_stalled(&self, stalls: &[usize]) -> String {
        let stall_before: Vec<bool> = (0..self.0.len()).map(|i| stalls.contains(&i)).collect();
        let p = Printer::new(&self.0, &stall_before, &crate::locals::Locals::OFF);
        let mut out = quote!();
        for (i, inst) in self.0.iter().enumerate() {
            if stall_before[i] {
                out.extend(quote!(stall();));
            }
            out.extend(p.tuple_before(i));
            if let Op::Insn(_) = inst.op {
                continue;
            }
            let s = p.value(i).unwrap_or_else(|| p.effect(i));
            out.extend(match inst.guard {
                Some(g) if p.value(i).is_none() => {
                    let g = p.guard(g);
                    quote!(if #g { #s })
                }
                _ => s,
            });
        }
        let text = pretty(quote!(fn f(m: &mut M) { #out }));
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

#[test]
fn constants_are_literals() {
    let mut b = B::default();
    let x = b.c(5);
    b.set(0, x);
    let y = b.c(0x0800_1234);
    b.set(1, y);
    let t = b.print();
    assert!(t.contains("m.cpu.r[0] = 5;"), "{t}");
    assert!(t.contains("m.cpu.r[1] = 0x800_1234;"), "{t}");
    assert!(!t.contains("let"), "{t}");
}

#[test]
fn a_value_used_once_is_printed_at_its_use() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let three = b.c(3);
    let sum = b.op(Op::Bin(BinOp::Add, r1, three));
    let mask = b.c(0xFF);
    let and = b.op(Op::Bin(BinOp::And, sum, mask));
    b.set(2, and);
    let t = b.print();
    assert!(
        t.contains("m.cpu.r[2] = m.cpu.r[1].wrapping_add(3) & 255;"),
        "{t}"
    );
    assert!(!t.contains("let"), "{t}");
}

#[test]
fn a_value_used_twice_is_bound() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let r2 = b.reg(2);
    let sum = b.op(Op::Bin(BinOp::Add, r1, r2));
    b.set(3, sum);
    b.set(4, sum);
    let t = b.print();
    assert!(
        t.contains("let v2: u32 = m.cpu.r[1].wrapping_add(m.cpu.r[2]);"),
        "{t}"
    );
    assert!(t.contains("m.cpu.r[3] = v2; m.cpu.r[4] = v2;"), "{t}");
}

#[test]
fn operators_keep_their_operands_grouped() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let r2 = b.reg(2);
    let or = b.op(Op::Bin(BinOp::Or, r1, r2));
    let not = b.op(Op::Not(or));
    let one = b.c(1);
    let sum = b.op(Op::Bin(BinOp::Add, not, one));
    b.set(0, sum);
    let t = b.print();
    assert!(
        t.contains("m.cpu.r[0] = (!(m.cpu.r[1] | m.cpu.r[2])).wrapping_add(1);"),
        "{t}"
    );
}

#[test]
fn a_literal_gets_a_type_where_nothing_gives_it_one() {
    let mut b = B::default();
    let pc = b.c(0x0800_0008);
    let off = b.c(4);
    let sum = b.op(Op::Bin(BinOp::Add, pc, off));
    b.set(0, sum);
    let x = b.c(0x8000_0000);
    let z = b.op(Op::IsZero(x));
    b.op(Op::SetFlag(Flag::Z, z));
    let t = b.print();
    assert!(
        t.contains("m.cpu.r[0] = 0x800_0008_u32.wrapping_add(4);"),
        "{t}"
    );
    assert!(t.contains("0x8000_0000_u32 == 0"), "{t}");
}

#[test]
fn a_read_does_not_move_past_a_write() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let zero = b.c(0);
    b.set(1, zero);
    b.set(2, r1);
    let t = b.print();
    assert!(t.contains("let v0: u32 = m.cpu.r[1];"), "{t}");
    assert!(t.contains("m.cpu.r[2] = v0;"), "{t}");
}

#[test]
fn a_read_moves_past_other_writes() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let c = b.op(Op::GetFlag(Flag::C));
    let zero = b.c(0);
    b.set(2, zero);
    b.op(Op::SetFlag(Flag::Z, c));
    b.set(3, r1);
    let t = b.print();
    assert!(
        t.contains("m.cpu.set_flag(Flag::Z, m.cpu.flag(Flag::C));"),
        "{t}"
    );
    assert!(t.contains("m.cpu.r[3] = m.cpu.r[1];"), "{t}");
    assert!(!t.contains("let"), "{t}");
}

#[test]
fn a_read_does_not_move_past_a_mode_change() {
    let mut b = B::default();
    let sp = b.reg(13);
    let x = b.c(0x12);
    b.op(Op::SetCpsr {
        v: x,
        mask: armv4t::sem::PsrMask(1),
    });
    b.set(0, sp);
    let t = b.print();
    assert!(t.contains("let v0: u32 = m.cpu.r[13];"), "{t}");
}

#[test]
fn a_read_moves_past_a_store() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let a = b.c(0x0200_0000);
    let zero = b.c(0);
    b.op(Op::Store {
        width: Width::Word,
        addr: a,
        value: zero,
    });
    b.set(2, r1);
    let t = b.print();
    assert!(t.contains("m.cpu.r[2] = m.cpu.r[1];"), "{t}");
}

#[test]
fn a_load_keeps_its_place_among_accesses() {
    let mut b = B::default();
    let a = b.reg(0);
    let x = b.op(Op::Load {
        width: Width::Word,
        addr: a,
    });
    let zero = b.c(0);
    b.op(Op::Store {
        width: Width::Byte,
        addr: a,
        value: zero,
    });
    b.set(1, x);
    let t = b.print();
    let load = t.find("let v1: u32 = m.bus.load(Width::Word, v0 & !3);");
    let store = t.find("m.bus.store(Width::Byte, v0, 0);");
    assert!(load.is_some() && load < store, "{t}");
}

#[test]
fn a_load_moves_into_its_use_past_cpu_writes() {
    let mut b = B::default();
    let a = b.reg(0);
    let x = b.op(Op::Load {
        width: Width::Half,
        addr: a,
    });
    let zero = b.c(0);
    b.set(5, zero);
    b.set(1, x);
    let t = b.print();
    assert!(
        t.contains("m.cpu.r[5] = 0; m.cpu.r[1] = m.bus.load(Width::Half, m.cpu.r[0] & !1);"),
        "{t}"
    );
}

#[test]
fn a_load_does_not_move_under_a_guard_or_into_a_select_arm() {
    let mut b = B::default();
    let z = b.op(Op::GetFlag(Flag::Z));
    let a = b.reg(0);
    let x = b.op(Op::Load {
        width: Width::Byte,
        addr: a,
    });
    b.guarded(Op::SetReg(Reg(1), x), Some(z));
    let y = b.op(Op::Load {
        width: Width::Byte,
        addr: a,
    });
    let r2 = b.reg(2);
    let s = b.op(Op::Select { c: z, a: y, b: r2 });
    b.set(2, s);
    let t = b.print();
    assert!(
        t.contains("let v2: u32 = m.bus.load(Width::Byte, v1);"),
        "{t}"
    );
    assert!(t.contains("if v0 { m.cpu.r[1] = v2; }"), "{t}");
    assert!(
        t.contains("let v4: u32 = m.bus.load(Width::Byte, v1);"),
        "{t}"
    );
    assert!(
        t.contains("m.cpu.r[2] = if v0 { v4 } else { m.cpu.r[2] };"),
        "{t}"
    );
}

#[test]
fn a_load_nothing_uses_still_runs() {
    let mut b = B::default();
    let a = b.c(0x0400_0130);
    b.op(Op::Load {
        width: Width::Half,
        addr: a,
    });
    let t = b.print();
    assert!(
        t.contains("m.bus.load(Width::Half, 0x400_0130 & !1);"),
        "{t}"
    );
}

#[test]
fn results_of_one_call_share_it() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let r2 = b.reg(2);
    let c = b.op(Op::Bit(false));
    let (a, bb) = (r1, r2);
    let sum = b.op(Op::Adc { a, b: bb, c });
    let carry = b.op(Op::AdcCarry { a, b: bb, c });
    let ovf = b.op(Op::AdcOverflow { a, b: bb, c });
    b.set(0, sum);
    b.op(Op::SetFlag(Flag::C, carry));
    b.op(Op::SetFlag(Flag::V, ovf));
    let t = b.print();
    assert!(
        t.contains("let (v3, v4, v5) = prim::add_with_carry(m.cpu.r[1], m.cpu.r[2], false);"),
        "{t}"
    );
    assert_eq!(t.matches("add_with_carry").count(), 1, "{t}");
}

#[test]
fn a_dead_result_is_left_out_of_the_tuple() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let r2 = b.reg(2);
    let c = b.op(Op::Bit(true));
    let sum = b.op(Op::Adc { a: r1, b: r2, c });
    b.op(Op::AdcCarry { a: r1, b: r2, c });
    let ovf = b.op(Op::AdcOverflow { a: r1, b: r2, c });
    b.set(0, sum);
    b.op(Op::SetFlag(Flag::V, ovf));
    let t = b.print();
    assert!(
        t.contains("let (v3, _, v5) = prim::add_with_carry(m.cpu.r[1], m.cpu.r[2], true);"),
        "{t}"
    );
}

#[test]
fn a_lone_result_is_a_field_of_the_call() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let r2 = b.reg(2);
    let c = b.op(Op::Bit(true));
    let sum = b.op(Op::Adc { a: r1, b: r2, c });
    b.set(0, sum);
    let t = b.print();
    assert!(
        t.contains("m.cpu.r[0] = prim::add_with_carry(m.cpu.r[1], m.cpu.r[2], true).0;"),
        "{t}"
    );
}

#[test]
fn a_shift_by_a_register_with_its_carry_is_one_call() {
    let mut b = B::default();
    let x = b.reg(1);
    let n = b.reg(2);
    let c = b.op(Op::GetFlag(Flag::C));
    let r = b.op(Op::Shift {
        kind: ShiftType::Lsl,
        v: x,
        amount: n,
    });
    let carry = b.op(Op::ShiftCarry {
        kind: ShiftType::Lsl,
        v: x,
        amount: n,
        carry_in: c,
    });
    b.set(0, r);
    b.op(Op::SetFlag(Flag::C, carry));
    let t = b.print();
    // (Pretty-printed over several lines, so with a trailing comma.)
    assert!(
        t.contains(
            "let (v3, v4) = prim::shift( ShiftType::Lsl, m.cpu.r[1], m.cpu.r[2], m.cpu.flag(Flag::C), );"
        ),
        "{t}"
    );
    assert_eq!(t.matches("prim::shift").count(), 1, "{t}");
}

#[test]
fn a_rotate_by_a_register_is_rotate_right() {
    let mut b = B::default();
    let x = b.reg(1);
    let n = b.reg(2);
    let r = b.op(Op::Shift {
        kind: ShiftType::Ror,
        v: x,
        amount: n,
    });
    b.set(0, r);
    let t = b.print();
    assert!(
        t.contains("m.cpu.r[0] = m.cpu.r[1].rotate_right(m.cpu.r[2]);"),
        "{t}"
    );
}

/// `movs r0, r1, <kind> #n` as the builder makes it (the amount as the
/// shifter reads it), printed.
fn shift_by(kind: ShiftType, n: u32) -> String {
    let mut b = B::default();
    let x = b.reg(1);
    let amount = b.c(n);
    let c = b.op(Op::GetFlag(Flag::C));
    let r = b.op(Op::Shift { kind, v: x, amount });
    let carry = b.op(Op::ShiftCarry {
        kind,
        v: x,
        amount,
        carry_in: c,
    });
    b.set(0, r);
    b.op(Op::SetFlag(Flag::C, carry));
    b.print()
}

#[test]
fn a_shift_by_a_constant_is_an_operator() {
    use ShiftType::*;
    let cases = [
        (Lsl, 24, "m.cpu.r[0] = v0 << 24;", "(v0 >> 8) & 1 != 0"),
        (Lsr, 1, "m.cpu.r[0] = v0 >> 1;", "v0 & 1 != 0"),
        // The value is dropped, so r1 has one use left.
        (Lsr, 32, "m.cpu.r[0] = 0;", "(m.cpu.r[1] >> 31) & 1 != 0"),
        (Lsl, 33, "m.cpu.r[0] = 0;", "Flag::C, false"),
        (
            Asr,
            4,
            "m.cpu.r[0] = ((v0 as i32) >> 4) as u32;",
            "(v0 >> 3) & 1 != 0",
        ),
        (
            Asr,
            40,
            "m.cpu.r[0] = ((v0 as i32) >> 31) as u32;",
            "(v0 >> 31) & 1 != 0",
        ),
        (
            Ror,
            8,
            "m.cpu.r[0] = v0.rotate_right(8);",
            "(v0 >> 7) & 1 != 0",
        ),
        (Ror, 32, "m.cpu.r[0] = v0;", "(v0 >> 31) & 1 != 0"),
        // Only the low byte counts: 0x100 is a shift by 0.
        (
            Lsl,
            0x100,
            "m.cpu.r[0] = m.cpu.r[1];",
            "Flag::C, m.cpu.flag(Flag::C)",
        ),
    ];
    for (kind, n, value, carry) in cases {
        let t = shift_by(kind, n);
        assert!(t.contains(value), "{kind:?} {n}: {t}");
        assert!(t.contains(carry), "{kind:?} {n}: {t}");
        assert!(!t.contains("prim::shift"), "{kind:?} {n}: {t}");
    }
}

#[test]
fn flag_writes_are_one_cpsr_write() {
    let mut b = B::default();
    let x = b.reg(1);
    let n = b.op(Op::TestBit { v: x, n: 31 });
    let z = b.op(Op::IsZero(x));
    let t = b.op(Op::Bit(true));
    let f = b.op(Op::Bit(false));
    b.op(Op::SetFlag(Flag::N, n));
    b.op(Op::SetFlag(Flag::Z, z));
    b.op(Op::SetFlag(Flag::C, t));
    b.op(Op::SetFlag(Flag::V, f));
    let t = b.print();
    assert!(
        t.contains(
            "m.cpu.cpsr = m.cpu.cpsr & 0xfff_ffff | 0x2000_0000 | (((v0 >> 31) & 1 != 0) as u32) << 31 | ((v0 == 0) as u32) << 30;"
        ),
        "{t}"
    );
    assert!(!t.contains("set_flag"), "{t}");
}

#[test]
fn a_read_between_flag_writes_ends_the_run() {
    let mut b = B::default();
    let x = b.reg(1);
    let z = b.op(Op::IsZero(x));
    b.op(Op::SetFlag(Flag::Z, z));
    let c = b.op(Op::GetFlag(Flag::Z));
    b.op(Op::SetFlag(Flag::C, c));
    let t = b.print();
    assert!(
        t.contains("m.cpu.set_flag(Flag::Z, m.cpu.r[1] == 0);"),
        "{t}"
    );
    assert!(
        t.contains("m.cpu.set_flag(Flag::C, m.cpu.flag(Flag::Z));"),
        "{t}"
    );
}

#[test]
fn nothing_moves_past_a_stall() {
    let mut b = B::default();
    let r1 = b.reg(1);
    let a = b.reg(2);
    let x = b.op(Op::Load {
        width: Width::Byte,
        addr: a,
    });
    b.op(Op::Insn(0x0800_0004));
    b.set(3, r1);
    b.set(4, x);
    let t = b.print_stalled(&[3]);
    assert!(t.contains("let v0: u32 = m.cpu.r[1];"), "{t}");
    assert!(
        t.contains("let v2: u32 = m.bus.load(Width::Byte, m.cpu.r[2]);"),
        "{t}"
    );
    assert!(
        t.contains("stall(); m.cpu.r[3] = v0; m.cpu.r[4] = v2;"),
        "{t}"
    );
}

#[test]
fn a_load_does_not_move_into_another_access() {
    // `m.bus.store(.., m.bus.load(..))` borrows `m.bus` twice.
    let mut b = B::default();
    let a = b.c(0x0300_0000);
    let x = b.op(Op::Load {
        width: Width::Word,
        addr: a,
    });
    b.op(Op::Store {
        width: Width::Word,
        addr: a,
        value: x,
    });
    let y = b.op(Op::Load {
        width: Width::Word,
        addr: a,
    });
    let z = b.op(Op::Load {
        width: Width::Word,
        addr: y,
    });
    b.set(0, z);
    let t = b.print();
    assert!(
        t.contains("let v1: u32 = m.bus.load(Width::Word, 0x300_0000 & !3);"),
        "{t}"
    );
    assert!(
        t.contains("m.bus.store(Width::Word, 0x300_0000 & !3, v1);"),
        "{t}"
    );
    assert!(
        t.contains("let v3: u32 = m.bus.load(Width::Word, 0x300_0000 & !3);"),
        "{t}"
    );
    assert!(
        t.contains("m.cpu.r[0] = m.bus.load(Width::Word, v3 & !3);"),
        "{t}"
    );
}

#[test]
fn a_flag_write_changes_the_spsr_read() {
    // In user mode `Cpu::spsr` is the CPSR.
    let mut b = B::default();
    let s = b.op(Op::GetSpsr);
    let t1 = b.op(Op::Bit(true));
    b.op(Op::SetFlag(Flag::Z, t1));
    b.set(0, s);
    let t = b.print();
    assert!(t.contains("let v0: u32 = m.cpu.spsr();"), "{t}");
}

#[test]
fn a_result_whose_operand_comes_later_is_its_own_call() {
    // The carry in is read after the shift, so the carry can't join the
    // shift's call.
    let mut b = B::default();
    let x = b.reg(1);
    let n = b.reg(2);
    let r = b.op(Op::Shift {
        kind: ShiftType::Lsr,
        v: x,
        amount: n,
    });
    let c = b.op(Op::GetFlag(Flag::C));
    let carry = b.op(Op::ShiftCarry {
        kind: ShiftType::Lsr,
        v: x,
        amount: n,
        carry_in: c,
    });
    b.set(0, r);
    b.op(Op::SetFlag(Flag::C, carry));
    let t = b.print();
    assert!(!t.contains("let ("), "{t}");
    assert!(
        t.contains("prim::shift(ShiftType::Lsr, v0, v1, false).0"),
        "{t}"
    );
    assert!(
        t.contains("prim::shift(ShiftType::Lsr, v0, v1, m.cpu.flag(Flag::C)).1"),
        "{t}"
    );
}

#[test]
fn a_strength_reduced_index_is_a_scale_of_its_value() {
    use super::linear;
    let mut b = B::default();
    let i = b.reg(4);
    let base = b.reg(6);
    let (two, three, f) = (b.c(2), b.c(3), b.op(Op::Bit(false)));
    let shl = |b: &mut B, v, n| {
        b.op(Op::Shift {
            kind: ShiftType::Lsl,
            v,
            amount: n,
        })
    };
    // ((i << 2) + i) << 3, plus r6, plus 4: agbcc's &gTasks[i].isActive.
    let x4 = shl(&mut b, i, two);
    let x5 = b.op(Op::Adc { a: x4, b: i, c: f });
    let x40 = shl(&mut b, x5, three);
    let sum = b.op(Op::Adc {
        a: x40,
        b: base,
        c: f,
    });
    let four = b.c(4);
    let at = b.op(Op::Bin(BinOp::Add, sum, four));
    let r6 = |r: usize| (r == 6).then_some(0x0300_5e00);
    assert_eq!(linear(&b.0, &r6, at, 8), (0x0300_5e04, Some((i, 40))));
    // r6 not known: the sum of two values is a leaf.
    assert_eq!(linear(&b.0, &|_| None, at, 8), (4, Some((sum, 1))));
    // A multiply by a constant scales too.
    let k = b.c(36);
    let m = b.op(Op::Bin(BinOp::Mul, k, i));
    assert_eq!(linear(&b.0, &|_| None, m, 8), (0, Some((i, 36))));
}
