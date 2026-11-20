//! Accesses as named fields, from the source map's types (unit
//! `native/lift-typed-access`): a load or store through a pointer the
//! value sets can't place prints `g.get(Ref::<t::Kirby>::new(r4).hp())`,
//! not `ld16(g, r4.wrapping_add(0x54) & !1)`.
//!
//! Nothing here proves where a pointer points. A pointer's type is taken
//! from the game's names files (`gbaa-lift --names DIR`, a decomp's DWARF
//! through `pret-names`) and carried through its function:
//!
//! - **Seeds.** A parameter the function's prototype makes a pointer
//!   (r0-r3 at entry); a constant address in a typed datum; a pointer
//!   field loaded through a typed pointer; r0 after a call to a function
//!   whose prototype returns a pointer; and, where the flow has nothing,
//!   the DWARF variable a register is the home of (`[locals]`), if it is
//!   a pointer. The last is what types most of a game whose objects come
//!   out of a task's data by arithmetic (the Amazing Mirror's
//!   `TaskGetStructPtr`): agbcc keeps `kirby` in one register for its
//!   scope.
//! - **Steps.** A typed pointer plus or minus a constant is that many
//!   bytes on in its object; plus anything else, an element of the array
//!   there, or the same place in another object of its type (`p + i`).
//!   An access through `p + i * s`, `s` the size of that array's
//!   elements or of `p`'s type, prints the element by its index
//!   (`.idx(i)`, `.add(i)`; [`pointer_indexed`], native/lift-index-names).
//!   A word a frame's local slot holds ([`super::frame`]) is what every
//!   store to the slot stored.
//! - **Joins.** Over the function's blocks, forwards: a register is
//!   typed at a block's start where every way in agrees. A call keeps
//!   r4-r11 (AAPCS) and gives r0 its callee's type.
//!
//! **A wrong type changes no byte read.** The printed access calls the
//! same `ld*`/`st*` on the same address as the untyped one: the accessors
//! ([`crate::types::Types::markers`], the mod view's) only add the
//! field's offset, which is the constant the code added. So nothing is
//! checked at run time, and the type is a label: a temporary in a
//! variable's register that happens to be dereferenced at one of its
//! type's fields prints as that field. Only an access that lands on a
//! field, or inside one, is printed typed.

use std::collections::BTreeMap;

use analysis::ir::{BinOp, Inst, Op, ValueId};
use armv4t::sem::Width;

use crate::names::{Home, Local};
use crate::types::{Step, Ty, Types};

/// What the names files give the lift's types.
#[derive(Clone, Debug, Default)]
pub struct Maps {
    pub types: Types,
    /// Each function's DWARF variables, by its key.
    pub locals: BTreeMap<u32, Vec<Local>>,
}

/// A value that is an address `off` bytes into a `ty`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct At {
    pub ty: Ty,
    pub off: u32,
}

/// What follows an exit, for the flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    /// The registers as they are.
    Jump,
    /// A call returned: r0-r3, r12 and lr are the callee's, r0 its
    /// prototype's (`Some`: the callee's key).
    Call(Option<u32>),
}

type Regs = [Option<At>; 16];

/// A typed access whose address is a typed pointer plus a value scaled
/// by a constant, and maybe a constant on top (`base + i * s + c`;
/// native/lift-index-names): what prints it as an element,
/// `.idx(i)` or `.add(i)` ([`pointer_indexed`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indexed {
    /// The pointer, and where it points.
    pub base: ValueId,
    pub at: At,
    /// The index, and the bytes one step of it is.
    pub i: ValueId,
    pub s: u32,
    /// The constant added after.
    pub c: u32,
}

/// What [`flow`] finds: by block (an index into its `blocks`) and inst,
/// each typed access's place, and those that index an element.
#[derive(Clone, Debug, Default)]
pub struct Flow {
    pub access: BTreeMap<(usize, usize), At>,
    pub index: BTreeMap<(usize, usize), Indexed>,
}

/// The typed accesses of one function: by block (an index into
/// `blocks`) and inst, where the address is in its object, and the ones
/// that are a typed pointer plus a scaled index ([`Flow`]).
///
/// `blocks[0]` is the entry. `out(b, i)` says where the exit at inst `i`
/// of block `b` goes on in the function; `slot(b, i)` names the frame's
/// local slot the access there is of, if it is one.
pub fn flow(
    maps: &Maps,
    entry: u32,
    blocks: &[&[Inst]],
    out: impl Fn(usize, usize) -> Vec<(usize, After)>,
    slot: impl Fn(usize, usize) -> Option<i32>,
) -> Flow {
    let t = &maps.types;
    let locals = maps.locals.get(&entry).map_or(&[][..], Vec::as_slice);
    // What each variable points at, if it is a pointer.
    let pointees: Vec<Option<Ty>> = locals
        .iter()
        .map(|l| match l.home {
            Home::Reg(_) => t.pointee(l.ty.as_deref()?),
            Home::Slot(_) => None,
        })
        .collect();
    // A pointer variable whose home is `r` at `pc`: the narrowest scope.
    let local = |r: usize, pc: u32, next: u32| -> Option<At> {
        let n = (0..locals.len())
            .filter(|&n| {
                let l = &locals[n];
                l.home == Home::Reg(r as u8) && l.from <= next && pc < l.to
            })
            .min_by_key(|&n| locals[n].to - locals[n].from)?;
        pointees[n].clone().map(|ty| At { ty, off: 0 })
    };
    let mut ins: Vec<Option<Regs>> = vec![None; blocks.len()];
    let mut start: Regs = Default::default();
    if let Some(p) = t.proto_of(entry) {
        for (r, a) in p.args.iter().enumerate() {
            if let Some((_, Some(ty))) = a {
                start[r] = Some(At {
                    ty: ty.clone(),
                    off: 0,
                });
            }
        }
    }
    if !blocks.is_empty() {
        ins[0] = Some(start);
    }
    // A slot's word: unset, or what every store to it stored.
    let mut slots: BTreeMap<i32, Option<At>> = BTreeMap::new();
    let mut access = BTreeMap::new();
    let mut index = BTreeMap::new();
    for round in 0.. {
        let mut changed = false;
        access.clear();
        index.clear();
        for b in 0..blocks.len() {
            let Some(mut regs) = ins[b].clone() else {
                continue;
            };
            let insts = blocks[b];
            let mut vt: Vec<Option<At>> = vec![None; insts.len()];
            let mut pc = 0u32;
            let konst = |x: ValueId| match insts[x.index()].op {
                Op::Const(c) | Op::Pool(c) => Some(c),
                _ => None,
            };
            for (i, inst) in insts.iter().enumerate() {
                match inst.op {
                    Op::Insn(a) => pc = a,
                    Op::Const(c) | Op::Pool(c) => {
                        vt[i] = t.global_at(c).map(|(k, ty)| At {
                            ty: ty.clone(),
                            off: c - k,
                        });
                    }
                    Op::GetReg(r) => {
                        vt[i] = regs[r.index()].clone().or_else(|| local(r.index(), pc, pc));
                    }
                    Op::Bin(op @ (BinOp::Add | BinOp::Sub), x, y) => {
                        let (tx, ty) = (&vt[x.index()], &vt[y.index()]);
                        vt[i] = match (tx, ty, konst(x), konst(y), op) {
                            (Some(p), None, _, Some(c), BinOp::Add) => moved(t, p, c),
                            (None, Some(p), Some(c), _, BinOp::Add) => moved(t, p, c),
                            (Some(p), None, _, Some(c), BinOp::Sub) => {
                                moved(t, p, c.wrapping_neg())
                            }
                            (Some(p), None, _, None, BinOp::Add) => Some(indexed(t, p)),
                            (None, Some(p), None, _, BinOp::Add) => Some(indexed(t, p)),
                            _ => None,
                        };
                    }
                    // Thumb's `adds rd, rn, #k` and `mov rd, rn` (`adds
                    // rd, rn, #0`), `subs` as `a + !b + 1`: an add with
                    // a known carry in.
                    Op::Adc { a, b: y, c } => {
                        let carry = match insts[c.index()].op {
                            Op::Bit(c) => Some(u32::from(c)),
                            _ => None,
                        };
                        // The other operand as a constant: itself, or the
                        // complement of one.
                        let k = |x: ValueId| match insts[x.index()].op {
                            Op::Not(z) => konst(z).map(|c| !c),
                            _ => konst(x),
                        };
                        let (ta, tb) = (&vt[a.index()], &vt[y.index()]);
                        vt[i] = match (ta, tb, carry) {
                            (Some(p), None, Some(c)) => match k(y) {
                                Some(n) => moved(t, p, n.wrapping_add(c)),
                                None => Some(indexed(t, p)),
                            },
                            (None, Some(p), Some(c)) => match k(a) {
                                Some(n) => moved(t, p, n.wrapping_add(c)),
                                None => Some(indexed(t, p)),
                            },
                            _ => None,
                        };
                    }
                    Op::Load { width, addr } => {
                        if let Some(s) = slot(b, i) {
                            vt[i] = slots.get(&s).cloned().flatten();
                        } else if let Some(p) = &vt[addr.index()] {
                            access.insert((b, i), p.clone());
                            if let Some(x) = indexed_at(insts, &vt, addr) {
                                index.insert((b, i), x);
                            }
                            if width == Width::Word
                                && let Some((_, Ty::Ptr(to))) = t.walk(&p.ty, p.off, 4, None)
                            {
                                vt[i] = Some(At { ty: *to, off: 0 });
                            }
                        }
                    }
                    Op::Store { addr, value, .. } => {
                        if let Some(s) = slot(b, i) {
                            let v = vt[value.index()].clone();
                            let new = match slots.get(&s) {
                                None => v,
                                Some(old) if *old == v => v,
                                Some(_) => None,
                            };
                            if slots.get(&s) != Some(&new) {
                                slots.insert(s, new);
                                changed = true;
                            }
                        } else if let Some(p) = &vt[addr.index()] {
                            access.insert((b, i), p.clone());
                            if let Some(x) = indexed_at(insts, &vt, addr) {
                                index.insert((b, i), x);
                            }
                        }
                    }
                    Op::SetReg(r, v) => {
                        let r = r.index();
                        // The flow has nothing: the variable whose home
                        // the register is from here, if the value could
                        // be a pointer.
                        if vt[v.index()].is_none() && pointerish(&insts[v.index()].op) {
                            vt[v.index()] = local(r, pc, pc + 4);
                        }
                        let new = vt[v.index()].clone();
                        regs[r] = match inst.guard {
                            Some(_) if regs[r] != new => None,
                            _ => new,
                        };
                    }
                    Op::Exit(_) => {
                        for (s, after) in out(b, i) {
                            let mut next = regs.clone();
                            if let After::Call(callee) = after {
                                for r in [0, 1, 2, 3, 12, 14] {
                                    next[r] = None;
                                }
                                next[0] = callee
                                    .and_then(|k| t.proto_of(k))
                                    .and_then(|p| p.ret.clone())
                                    .map(|ty| At { ty, off: 0 });
                            }
                            let joined = match &ins[s] {
                                None => next,
                                Some(old) => {
                                    let mut j = old.clone();
                                    for r in 0..16 {
                                        if j[r] != next[r] {
                                            j[r] = None;
                                        }
                                    }
                                    j
                                }
                            };
                            if ins[s].as_ref() != Some(&joined) {
                                ins[s] = Some(joined);
                                changed = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Every state only loses types but a slot's first, so this ends;
        // the cap is for a mistake.
        if !changed || round > 64 {
            break;
        }
    }
    Flow { access, index }
}

/// `addr` as `base + i * s + c`: a typed pointer plus a value the block
/// doesn't know scaled by a constant (through shifts left, multiplies
/// and adds of it to itself: `((i << 2) + i) << 3` is `i * 40`), and
/// maybe a constant after.
fn indexed_at(insts: &[Inst], vt: &[Option<At>], addr: ValueId) -> Option<Indexed> {
    let konst = |x: ValueId| match insts[x.index()].op {
        Op::Const(c) | Op::Pool(c) => Some(c),
        _ => None,
    };
    let no_carry = |c: ValueId| insts[c.index()].op == Op::Bit(false);
    // The two operands of an add, with no carry in.
    let add = |x: ValueId| match insts[x.index()].op {
        Op::Bin(BinOp::Add, a, b) => Some((a, b)),
        Op::Adc { a, b, c } if no_carry(c) => Some((a, b)),
        _ => None,
    };
    // A small constant on top (not an address the types place).
    let small = |x: ValueId| konst(x).filter(|_| vt[x.index()].is_none());
    let (sum, c) = match add(addr) {
        Some((x, y)) => match (small(x), small(y)) {
            (None, Some(c)) if vt[x.index()].is_some() => (x, c),
            (Some(c), None) if vt[y.index()].is_some() => (y, c),
            _ => (addr, 0),
        },
        None => (addr, 0),
    };
    let (x, y) = add(sum)?;
    let (base, other) = match (&vt[x.index()], &vt[y.index()]) {
        (Some(_), None) => (x, y),
        (None, Some(_)) => (y, x),
        _ => return None,
    };
    if konst(other).is_some() {
        return None;
    }
    let (k, i) = crate::print::linear(insts, &|_| None, other, 8);
    let (i, s) = i?;
    if k != 0 || s == 0 || konst(i).is_some() {
        return None;
    }
    Some(Indexed {
        base,
        at: vt[base.index()].clone()?,
        i,
        s,
        c,
    })
}

/// Whether a value of `op` could be a pointer: not a narrow load, a
/// shift, a small constant or a bit.
fn pointerish(op: &Op) -> bool {
    match *op {
        Op::Load { width, .. } => width == Width::Word,
        Op::GetReg(_)
        | Op::Bin(BinOp::Add | BinOp::Sub, ..)
        | Op::Adc { .. }
        | Op::Select { .. } => true,
        Op::Const(c) | Op::Pool(c) => c >= 0x0200_0000,
        _ => false,
    }
}

/// `p` moved `c` bytes (two's complement): still in its object, or
/// nothing.
fn moved(t: &Types, p: &At, c: u32) -> Option<At> {
    let off = p.off.wrapping_add(c);
    (off < t.size(&p.ty).max(1)).then(|| At {
        ty: p.ty.clone(),
        off,
    })
}

/// `p` plus an amount the block doesn't know: an element of the array at
/// `p`, or the same place in another `p.ty` (`p + i`).
fn indexed(t: &Types, p: &At) -> At {
    let (mut ty, mut off) = (&p.ty, p.off);
    loop {
        match ty {
            Ty::Array(e, _) => {
                let es = t.size(e).max(1);
                return At {
                    ty: (**e).clone(),
                    off: off % es,
                };
            }
            Ty::Named(_) => match t.field_at(ty, off) {
                Some((f, o)) => (ty, off) = (f, o),
                None => return p.clone(),
            },
            _ => return p.clone(),
        }
    }
}

/// `ty` as the lifted crate writes it: a marker of the `t` module, an
/// integer, an array, a `Ref`.
pub fn rust(t: &Types, ty: &Ty) -> String {
    match ty {
        Ty::U8 => "u8".into(),
        Ty::U16 => "u16".into(),
        Ty::U32 => "u32".into(),
        Ty::Ptr(to) => format!("Ref<{}>", rust(t, to)),
        Ty::Array(e, n) => format!("[{}; {n}]", rust(t, e)),
        Ty::Named(n) => match t.marker(n) {
            Some(id) => format!("t::{id}"),
            None => "u8".into(),
        },
        Ty::Bytes(n) => format!("[u8; {n}]"),
    }
}

/// The printed pointer to the `w` bytes at `at`, reached from `p`, a
/// printed value `k` bytes into the object (`k <= at.off`): its
/// expression, and whether the whole access is named (else its last step
/// is `.field::<uN>(off)`). `None` where it lands on no field.
pub fn pointer(t: &Types, at: &At, w: u32, p: &str, k: u32) -> Option<(String, bool)> {
    let (steps, rest) = t.path(&at.ty, at.off, w, None);
    if steps.is_empty() && rest.is_some() {
        return None;
    }
    let ty = rust(t, &at.ty);
    let mut e = match k {
        0 => format!("Ref::<{ty}>::new({p})"),
        k => format!("Ref::<{ty}>::to({p}, {k:#x})"),
    };
    for s in &steps {
        match s {
            Step::Field(f) => e.push_str(&format!(".{f}()")),
            Step::Idx(n) => e.push_str(&format!(".idx({n})")),
        }
    }
    if let Some(o) = rest {
        e.push_str(&format!(".field::<u{}>({o:#x})", 8 * w));
    }
    Some((e, rest.is_none()))
}

/// The printed pointer to the `w` bytes an indexed access ([`Indexed`])
/// reaches, from the printed base `p` and index `i`: an element of the
/// array of `x.s`-byte elements the base points into
/// (`Ref::<[t::Obj; 4]>::new(p).idx(i).color()`, the base `k` elements
/// in as `.idx(i.wrapping_add(k))`), or, where a whole object of the
/// base's type is `x.s` bytes, the `i`th object on from it
/// (`Ref::<t::Body>::new(p).add(i).color()`). The same address as the
/// code's: the base less its offset in its object, the steps to the
/// element, the element's own offset. As [`pointer`] returns; `None`
/// where neither fits or it lands on no field.
pub fn pointer_indexed(t: &Types, x: &Indexed, w: u32, p: &str, i: &str) -> Option<(String, bool)> {
    let b = &x.at;
    let ty = rust(t, &b.ty);
    let mut e = match b.off {
        0 => format!("Ref::<{ty}>::new({p})"),
        k => format!("Ref::<{ty}>::to({p}, {k:#x})"),
    };
    let push = |e: &mut String, steps: &[Step]| {
        for s in steps {
            match s {
                Step::Field(f) => e.push_str(&format!(".{f}()")),
                Step::Idx(n) => e.push_str(&format!(".idx({n})")),
            }
        }
    };
    let (elem, off) = match t.array_at(&b.ty, b.off, x.s) {
        // A byte offset into a struct is no element.
        Some((to, elem, k, o)) if x.s > 1 || to.is_empty() => {
            push(&mut e, &to);
            match k {
                0 => e.push_str(&format!(".idx({i})")),
                k => e.push_str(&format!(".idx({i}.wrapping_add({k:#x}))")),
            }
            (elem, o.wrapping_add(x.c))
        }
        _ if t.size(&b.ty) == x.s => {
            e.push_str(&format!(".add({i})"));
            (b.ty.clone(), b.off.wrapping_add(x.c))
        }
        _ => return None,
    };
    if off.checked_add(w).is_none_or(|end| end > t.size(&elem)) {
        return None;
    }
    let (steps, rest) = t.path(&elem, off, w, None);
    if steps.is_empty() && rest.is_some() {
        return None;
    }
    push(&mut e, &steps);
    if let Some(o) = rest {
        e.push_str(&format!(".field::<u{}>({o:#x})", 8 * w));
    }
    Some((e, rest.is_none()))
}

/// The names of the values the field the `w` bytes at `at` are takes
/// (`[values]`; native/lift-value-names), by value: only where the access
/// is the whole field.
pub fn values<'t>(t: &'t Types, at: &At, w: u32) -> Option<&'t BTreeMap<u32, String>> {
    let (steps, rest) = t.path(&at.ty, at.off, w, None);
    if rest.is_some() || steps.is_empty() {
        return None;
    }
    t.values_of(&at.ty, &steps)
}

/// The lifted crate's `t` module: every struct and union of the names
/// files as a marker type, its fields accessors on `Ref`.
pub fn module(t: &Types) -> String {
    let m = t.markers();
    let text = crate::emit::pretty(quote::quote! {
        /// The names files' structs and unions as marker types, each
        /// field an accessor on a `Ref` to one (native/lift-typed-access;
        /// `codegen::native::fields`).
        #[allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]
        pub mod t {
            use crate::{Bits, Guest, Ptr};
            #m
        }
    });
    format!("{FIELDS_PRELUDE}\n{text}")
}

/// What the `t` module's accessors and the typed accesses need of the
/// lifted crate, written into it only when a lift has types.
const FIELDS_PRELUDE: &str = include_str!("prelude_fields.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::SourceMap;
    use analysis::ir::Exit;
    use armv4t::sem::Reg;

    const SHA: &str = "0000000000000000000000000000000000000000";

    fn maps() -> Maps {
        let text = format!(
            r#"
[map]
rom_sha1 = "{SHA}"
source = "test"
format = 1

[functions]
0x08000001 = {{ name = "Update", size = 16, proto = "void Update(struct Kirby *kirby, u32 n)" }}
0x08000011 = {{ name = "Find", size = 16, proto = "struct Kirby *Find(void)" }}

[data]
0x03000010 = {{ name = "gKirbys", size = 32, proto = "struct Kirby [4]" }}

[locals]
0x08000011 = [
  {{ name = "k", reg = 5, from = 0x08000010, to = 0x08000020, type = "struct Kirby *" }},
]

[types."struct Vec"]
kind = "struct"
size = 4
fields = [
  {{ name = "x", off = 0, size = 2, type = "s16" }},
  {{ name = "y", off = 2, size = 2, type = "s16" }},
]

[types."struct Kirby"]
kind = "struct"
size = 8
fields = [
  {{ name = "pos", off = 0, size = 4, type = "struct Vec" }},
  {{ name = "next", off = 4, size = 4, type = "struct Kirby *" }},
]
"#
        );
        let m = SourceMap::from_texts(SHA, &[&text]).unwrap();
        Maps {
            types: m.typed.clone(),
            locals: m.locals.clone(),
        }
    }

    fn inst(op: Op) -> Inst {
        Inst { op, guard: None }
    }

    #[test]
    fn a_parameter_types_its_fields_and_the_pointer_it_loads() {
        let m = maps();
        // r0: kirby. y = kirby->pos.y; n = kirby->next; n->pos.x = y.
        let b = [
            inst(Op::Insn(0x0800_0000)),
            inst(Op::GetReg(Reg(0))),
            inst(Op::Const(2)),
            inst(Op::Bin(BinOp::Add, ValueId(1), ValueId(2))),
            inst(Op::Load {
                width: Width::Half,
                addr: ValueId(3),
            }),
            inst(Op::Const(4)),
            inst(Op::Bin(BinOp::Add, ValueId(1), ValueId(5))),
            inst(Op::Load {
                width: Width::Word,
                addr: ValueId(6),
            }),
            inst(Op::Store {
                width: Width::Half,
                addr: ValueId(7),
                value: ValueId(4),
            }),
            inst(Op::Exit(Exit::Jump(ValueId(1)))),
        ];
        let a = flow(&m, 0x0800_0001, &[&b], |_, _| Vec::new(), |_, _| None).access;
        let p = |i: usize, w: u32, v: &str, k: u32| {
            pointer(&m.types, &a[&(0, i)], w, v, k).map(|(e, named)| {
                assert!(named);
                e
            })
        };
        assert_eq!(
            p(4, 2, "r0", 0).as_deref(),
            Some("Ref::<t::Kirby>::new(r0).pos().y()")
        );
        assert_eq!(
            p(7, 4, "v3", 4).as_deref(),
            Some("Ref::<t::Kirby>::to(v3, 0x4).next()")
        );
        assert_eq!(
            p(8, 2, "v7", 0).as_deref(),
            Some("Ref::<t::Kirby>::new(v7).pos().x()")
        );
        // A byte of `next` is inside a field, not one.
        assert_eq!(
            pointer(
                &m.types,
                &At {
                    ty: a[&(0, 7)].ty.clone(),
                    off: 5
                },
                1,
                "p",
                5
            ),
            Some((
                "Ref::<t::Kirby>::to(p, 0x5).next().field::<u8>(0x1)".into(),
                false
            ))
        );
    }

    #[test]
    fn a_global_an_index_a_call_and_a_variable_type_a_register() {
        let m = maps();
        // Block 0: r4 = &gKirbys[i]; call Find. Block 1: r5 = r0 + r1 (a
        // variable's home: `k`); *(r4 + 4); *(r0 + 4); *(r5).
        let b0 = [
            inst(Op::Insn(0x0800_0010)),
            inst(Op::Const(0x0300_0010)),
            inst(Op::GetReg(Reg(1))),
            inst(Op::Bin(BinOp::Add, ValueId(1), ValueId(2))),
            inst(Op::SetReg(Reg(4), ValueId(3))),
            inst(Op::Exit(Exit::Jump(ValueId(1)))),
        ];
        let b1 = [
            inst(Op::Insn(0x0800_0014)),
            inst(Op::GetReg(Reg(4))),
            inst(Op::Const(4)),
            inst(Op::Bin(BinOp::Add, ValueId(1), ValueId(2))),
            inst(Op::Load {
                width: Width::Word,
                addr: ValueId(3),
            }),
            inst(Op::GetReg(Reg(0))),
            inst(Op::Bin(BinOp::Add, ValueId(5), ValueId(2))),
            inst(Op::Load {
                width: Width::Word,
                addr: ValueId(6),
            }),
            inst(Op::GetReg(Reg(1))),
            inst(Op::Bin(BinOp::And, ValueId(5), ValueId(8))),
            inst(Op::Bin(BinOp::Add, ValueId(9), ValueId(8))),
            inst(Op::SetReg(Reg(5), ValueId(10))),
            inst(Op::Load {
                width: Width::Half,
                addr: ValueId(10),
            }),
            inst(Op::Exit(Exit::Jump(ValueId(1)))),
        ];
        let a = flow(
            &m,
            0x0800_0011,
            &[&b0, &b1],
            |b, _| match b {
                0 => vec![(1, After::Call(Some(0x0800_0011)))],
                _ => Vec::new(),
            },
            |_, _| None,
        )
        .access;
        let kirby = Ty::Named("struct Kirby".into());
        let at = |off| At {
            ty: kirby.clone(),
            off,
        };
        // r4 survives the call; r0 is the callee's; r5's value takes the
        // variable's type.
        assert_eq!(a.get(&(1, 4)), Some(&at(4)));
        assert_eq!(a.get(&(1, 7)), Some(&at(4)));
        assert_eq!(a.get(&(1, 12)), Some(&at(0)));
    }

    #[test]
    fn a_scaled_index_prints_as_an_element() {
        let m = maps();
        // r0: kirby. gKirbys[r1].next, the index as i << 3;
        // kirby[r1].pos.y, as (i << 2) + (i << 2).
        let b = [
            inst(Op::Insn(0x0800_0000)),
            inst(Op::GetReg(Reg(1))),
            inst(Op::Const(3)),
            inst(Op::Shift {
                kind: armv4t::sem::ShiftType::Lsl,
                v: ValueId(1),
                amount: ValueId(2),
            }),
            inst(Op::Const(0x0300_0010)),
            inst(Op::Bin(BinOp::Add, ValueId(4), ValueId(3))),
            inst(Op::Const(4)),
            inst(Op::Bin(BinOp::Add, ValueId(5), ValueId(6))),
            inst(Op::Load {
                width: Width::Word,
                addr: ValueId(7),
            }),
            inst(Op::GetReg(Reg(0))),
            inst(Op::Const(2)),
            inst(Op::Shift {
                kind: armv4t::sem::ShiftType::Lsl,
                v: ValueId(1),
                amount: ValueId(10),
            }),
            inst(Op::Bin(BinOp::Add, ValueId(11), ValueId(11))),
            inst(Op::Bin(BinOp::Add, ValueId(9), ValueId(12))),
            inst(Op::Bin(BinOp::Add, ValueId(13), ValueId(10))),
            inst(Op::Load {
                width: Width::Half,
                addr: ValueId(14),
            }),
            inst(Op::Exit(Exit::Jump(ValueId(1)))),
        ];
        let f = flow(&m, 0x0800_0001, &[&b], |_, _| Vec::new(), |_, _| None);
        let p = |i: usize, w: u32| {
            let x = &f.index[&(0, i)];
            assert_eq!(x.i, ValueId(1));
            pointer_indexed(&m.types, x, w, "p", "i").map(|(e, named)| {
                assert!(named);
                e
            })
        };
        assert_eq!(
            p(8, 4).as_deref(),
            Some("Ref::<[t::Kirby; 4]>::new(p).idx(i).next()")
        );
        assert_eq!(
            p(15, 2).as_deref(),
            Some("Ref::<t::Kirby>::new(p).add(i).pos().y()")
        );
        // A step that is no element's size is no index.
        let mut x = f.index[&(0, 15)].clone();
        x.s = 4;
        assert_eq!(pointer_indexed(&m.types, &x, 2, "p", "i"), None);
    }

    #[test]
    fn a_slot_holds_what_was_stored_and_a_join_that_disagrees_is_untyped() {
        let m = maps();
        // Block 0: slot = r0 (kirby); r4 = r0; to 1 or 2. Block 1: r4 =
        // r1 (an integer); to 2. Block 2: *(slot + 4), *(r4 + 4).
        let b0 = [
            inst(Op::GetReg(Reg(0))),
            inst(Op::GetReg(Reg(13))),
            inst(Op::Store {
                width: Width::Word,
                addr: ValueId(1),
                value: ValueId(0),
            }),
            inst(Op::SetReg(Reg(4), ValueId(0))),
            inst(Op::Exit(Exit::Jump(ValueId(0)))),
        ];
        let b1 = [
            inst(Op::GetReg(Reg(1))),
            inst(Op::SetReg(Reg(4), ValueId(0))),
            inst(Op::Exit(Exit::Jump(ValueId(0)))),
        ];
        let b2 = [
            inst(Op::GetReg(Reg(13))),
            inst(Op::Load {
                width: Width::Word,
                addr: ValueId(0),
            }),
            inst(Op::Const(4)),
            inst(Op::Bin(BinOp::Add, ValueId(1), ValueId(2))),
            inst(Op::Load {
                width: Width::Word,
                addr: ValueId(3),
            }),
            inst(Op::GetReg(Reg(4))),
            inst(Op::Bin(BinOp::Add, ValueId(5), ValueId(2))),
            inst(Op::Load {
                width: Width::Word,
                addr: ValueId(6),
            }),
            inst(Op::Exit(Exit::Jump(ValueId(0)))),
        ];
        let a = flow(
            &m,
            0x0800_0001,
            &[&b0, &b1, &b2],
            |b, _| match b {
                0 => vec![(1, After::Jump), (2, After::Jump)],
                1 => vec![(2, After::Jump)],
                _ => Vec::new(),
            },
            |b, i| ((b == 0 && i == 2) || (b == 2 && i == 1)).then_some(-4),
        )
        .access;
        assert!(a.contains_key(&(2, 4)), "through the slot's pointer");
        assert!(!a.contains_key(&(2, 7)), "r4 is a pointer one way only");
    }
}
