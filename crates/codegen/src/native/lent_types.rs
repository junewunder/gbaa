//! Lent stack locals of their DWARF type (`native/lift-stack-typed-locals`;
//! `docs/native-port-design.md`, "Stack locals passed by address").
//!
//! A lent object ([`Lends::objects`]) is a local of bytes. Where the names
//! files give the function a stack variable (`[locals]`, `slot = s`, a C
//! type they lay out) whose bytes hold the whole object, and every access
//! to it fits that type, the local is that type instead: its bytes are the
//! variable's (the object widened to them, any slot of the frame inside
//! them taken in), and its accesses its fields. **Fits**: each access's
//! offset and width is one scalar field of the type, whole
//! ([`Types::scalar_at`]): the frame's own accesses to those bytes, and
//! every load and store through a borrowed parameter it is lent to, on
//! through the parameters those pass it on to. A borrowed parameter takes
//! a type where every address its callers pass it points at one object of
//! that one type (a typed local's start, a field or element of one, or a
//! typed parameter's such passed on: `&ctx`, `&ctx.pos`, `&colors[1]`; of
//! the objects starting there, [`Types::objects_at`], the one the
//! parameter's own C type points at, else the outermost) and its own
//! accesses fit it from its start;
//! an object is typed only where every parameter it reaches is, so each
//! access through a `Lent` of it names a field. Several lent objects in
//! one variable's bytes (`&colors[0]`, `&colors[1]`, each its own) are
//! that one local. The rest stay bytes, counted (`lent_locals_untyped`).
//!
//! The variable's slot counts from sp after the prologue
//! ([`crate::dwarf::prologue_sp`]), the object's bytes from the entry sp.
//! Two variables over the object's bytes (C scopes sharing a slot) of
//! different types type it not at all.

use super::fields::Maps;
use super::frame::lend::Lends;
use super::frame::{Frame, Kind, Place};
use crate::func::Func;
use crate::names::Home;
use crate::types::{Step, Ty, Types};
use std::collections::{BTreeMap, BTreeSet};

/// A lent object of its variable's type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typed {
    /// Its bytes from the entry sp, the variable's: `lo..lo + size`.
    pub lo: i32,
    pub ty: Ty,
    /// The variable's name and C type, for the declaration's comment.
    pub var: String,
    pub c: String,
}

/// One context's typed lent objects and borrowed parameters.
#[derive(Clone, Debug, Default)]
pub struct LentTypes {
    /// By function: its typed objects, by their `lo`.
    pub objects: BTreeMap<u32, BTreeMap<i32, Typed>>,
    /// The borrowed parameters of a type, by (function, register).
    pub params: BTreeMap<(u32, u8), Ty>,
    /// The lent objects left bytes.
    pub untyped: usize,
}

impl LentTypes {
    /// Function `f`'s lent objects as [`Frame::locals`] takes them: a
    /// typed one as its variable's bytes.
    pub fn spans(&self, lends: &Lends, f: u32, types: Option<&Types>) -> Vec<(i32, i32)> {
        let objs = lends.objects.get(&f).map_or(&[][..], Vec::as_slice);
        let typed = self.objects.get(&f);
        let mut v: Vec<(i32, i32)> = Vec::new();
        for &(lo, hi) in objs {
            let span = typed
                .and_then(|m| {
                    m.values()
                        .find(|t| t.lo <= lo && hi <= t.lo + size(types, t))
                })
                .map_or((lo, hi), |t| (t.lo, t.lo + size(types, t)));
            if !v.contains(&span) {
                v.push(span);
            }
        }
        v
    }

    /// The typed object of function `f` at `lo`, if it is one.
    pub fn object(&self, f: u32, lo: i32) -> Option<&Typed> {
        self.objects.get(&f)?.get(&lo)
    }
}

fn size(types: Option<&Types>, t: &Typed) -> i32 {
    types.map_or(0, |ts| ts.size(&t.ty) as i32)
}

/// The accessor path of steps `s`, after a closure's `s`: `.pos().x()`,
/// `.idx(3)`.
pub fn path(steps: &[Step]) -> String {
    let mut e = String::new();
    for s in steps {
        match s {
            Step::Field(f) => e.push_str(&format!(".{f}()")),
            Step::Idx(n) => e.push_str(&format!(".idx({n})")),
        }
    }
    e
}

/// The typed objects and parameters of one context's lends.
pub fn of_code(
    lends: &Lends,
    frames: &BTreeMap<u32, Frame>,
    funcs: &[Func],
    maps: Option<&Maps>,
) -> LentTypes {
    // Where each function's `[locals]` slots count from.
    let bases: BTreeMap<u32, i32> = funcs
        .iter()
        .filter(|f| lends.objects.contains_key(&f.entry))
        .filter_map(|f| {
            let b = f.blocks.get(&f.entry)?;
            Some((f.entry, crate::dwarf::prologue_sp(&b.block.insts)?))
        })
        .collect();
    of_lends(lends, frames, &bases, maps)
}

/// [`of_code`], with each function's sp after its prologue, from the
/// entry sp, given (`bases`).
pub fn of_lends(
    lends: &Lends,
    frames: &BTreeMap<u32, Frame>,
    bases: &BTreeMap<u32, i32>,
    maps: Option<&Maps>,
) -> LentTypes {
    let all: usize = lends.objects.values().map(Vec::len).sum();
    let Some(maps) = maps else {
        return LentTypes {
            untyped: all,
            ..LentTypes::default()
        };
    };
    let t = &maps.types;
    // What each borrowed parameter's own C type points at, by
    // (function, register).
    let want: BTreeMap<(u32, u8), Ty> = maps
        .locals
        .iter()
        .flat_map(|(&f, vs)| vs.iter().map(move |v| (f, v)))
        .filter_map(|(f, v)| match v.home {
            Home::Reg(r) if v.param && r < 4 => Some(((f, r), t.pointee(v.ty.as_deref()?)?)),
            _ => None,
        })
        .collect();
    // The object `off` bytes into a `ty` a `Lent` there passed to `to`
    // points at: of those starting there, the one `to`'s C type points at,
    // else the outermost.
    let sub = |ty: &Ty, off: i32, to: (u32, u8)| {
        let v = t.objects_at(ty, u32::try_from(off).ok()?);
        match want.get(&to) {
            Some(w) if v.contains(w) => Some(w.clone()),
            _ => v.into_iter().next(),
        }
    };

    // Each object's candidate: the one variable type holding it whose
    // frame accesses fit.
    let mut objs: BTreeMap<(u32, i32), Typed> = BTreeMap::new();
    for (&f, os) in &lends.objects {
        let (Some(fr), Some(&base)) = (frames.get(&f), bases.get(&f)) else {
            continue;
        };
        let vars = maps.locals.get(&f).map_or(&[][..], Vec::as_slice);
        let none = BTreeSet::new();
        let lent = lends.lent.get(&f).unwrap_or(&none);
        // The frame addresses it lends.
        let passed: BTreeSet<i32> = lends
            .lent_passes
            .get(&f)
            .into_iter()
            .flat_map(|m| m.values().flatten().map(|&(_, o)| o))
            .collect();
        for &(lo, hi) in os {
            let mut found: Vec<Typed> = Vec::new();
            for v in vars {
                let (Home::Slot(s), Some(c)) = (v.home, v.ty.as_deref()) else {
                    continue;
                };
                let Some(ty) = t.of_c(c) else { continue };
                let b = base + s;
                let n = t.size(&ty) as i32;
                if b <= lo && hi <= b + n && !found.iter().any(|x| x.lo == b && x.ty == ty) {
                    found.push(Typed {
                        lo: b,
                        ty,
                        var: v.name.clone(),
                        c: c.to_string(),
                    });
                }
            }
            let fits = matches!(&found[..], [one] if frame_fits(t, fr, lent, &passed, one, os));
            if debug() {
                eprintln!(
                    "lent types {f:#x}: object {lo}..{hi}, sp after the prologue {base}: variables {:?}, frame fits {fits}",
                    found.iter().map(|x| (x.lo, &x.c)).collect::<Vec<_>>()
                );
            }
            if fits {
                objs.insert((f, lo), found[0].clone());
            }
        }
    }

    // The borrowed parameters' types and the objects', each taken out
    // where something fails, until nothing does.
    let mut bad: BTreeSet<(u32, u8)> = BTreeSet::new();
    loop {
        // Each parameter's sources: a type, or `None` for an untyped one.
        let mut src: BTreeMap<(u32, u8), Vec<Option<Ty>>> = BTreeMap::new();
        for (&f, m) in &lends.lent_passes {
            for (at, ps) in m {
                let Some(&callee) = lends.callee.get(&(f, *at)) else {
                    continue;
                };
                for &(r, o) in ps {
                    let ty = holder(lends, f, callee, r, o)
                        .and_then(|lo| objs.get(&(f, lo)))
                        .and_then(|x| sub(&x.ty, o - x.lo, (callee, r)));
                    add(src.entry((callee, r)).or_default(), ty);
                }
            }
        }
        // Passes on: in rounds, as a parameter's type comes from its
        // callers'.
        let mut params: BTreeMap<(u32, u8), Ty> = BTreeMap::new();
        for _ in 0..64 {
            let mut next: BTreeMap<(u32, u8), Ty> = BTreeMap::new();
            let mut all_src = src.clone();
            for (&f, m) in &lends.passes {
                for (at, ps) in m {
                    let Some(&callee) = lends.callee.get(&(f, *at)) else {
                        continue;
                    };
                    for &(r, p, c) in ps {
                        let ty = params.get(&(f, p)).and_then(|ty| sub(ty, c, (callee, r)));
                        add(all_src.entry((callee, r)).or_default(), ty);
                    }
                }
            }
            for (k, s) in &all_src {
                if let [Some(ty)] = &s[..]
                    && !bad.contains(k)
                    && through_fits(t, lends, *k, ty)
                {
                    next.insert(*k, ty.clone());
                }
            }
            if next == params {
                break;
            }
            params = next;
        }
        // Every pass, checked both ways: a typed source (an object, a
        // parameter) goes only to a parameter of the type of what it
        // points at there; a typed parameter takes only such. What fails
        // is taken out, and an object with every object of its variable.
        let mut changed = false;
        for (&f, m) in &lends.passes {
            for (at, ps) in m {
                let Some(&callee) = lends.callee.get(&(f, *at)) else {
                    continue;
                };
                for &(r, p, c) in ps {
                    let (from, to) = (params.get(&(f, p)), params.get(&(callee, r)));
                    let same = from
                        .and_then(|ty| sub(ty, c, (callee, r)))
                        .is_some_and(|ty| Some(&ty) == to);
                    if !same && from.is_some() && bad.insert((f, p)) {
                        changed = true;
                    }
                    if !same && to.is_some() && bad.insert((callee, r)) {
                        changed = true;
                    }
                }
            }
        }
        for (&f, m) in &lends.lent_passes {
            for (at, ps) in m {
                let Some(&callee) = lends.callee.get(&(f, *at)) else {
                    continue;
                };
                for &(r, o) in ps {
                    let lo = holder(lends, f, callee, r, o);
                    let from = lo.and_then(|lo| objs.get(&(f, lo)));
                    let to = params.get(&(callee, r));
                    let same = from
                        .and_then(|x| sub(&x.ty, o - x.lo, (callee, r)))
                        .is_some_and(|ty| Some(&ty) == to);
                    if !same && let Some(x) = from {
                        if debug() {
                            eprintln!(
                                "lent types {f:#x}: variable at {} out: lent at {o} to {callee:#x} r{r}, typed {to:?}",
                                x.lo
                            );
                        }
                        let var = x.lo;
                        objs.retain(|&(g, _), y| g != f || y.lo != var);
                        changed = true;
                    }
                    if !same && to.is_some() && bad.insert((callee, r)) {
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            let mut out = LentTypes {
                params,
                untyped: all - objs.len(),
                ..LentTypes::default()
            };
            for ((f, _), x) in objs {
                out.objects.entry(f).or_default().insert(x.lo, x);
            }
            return out;
        }
    }
}

/// `GBAA_LENT_TYPES_DEBUG=1`: each lent object's variables and what
/// took it or a parameter out.
fn debug() -> bool {
    std::env::var_os("GBAA_LENT_TYPES_DEBUG").is_some()
}

/// `x` into the sources `v`, once.
fn add(v: &mut Vec<Option<Ty>>, x: Option<Ty>) {
    if !v.contains(&x) {
        v.push(x);
    }
}

/// The `lo` of function `f`'s lent object (as [`Lends::objects`] has it)
/// an address `o` of its frame passed in callee `k`'s register `r` lends:
/// the one holding the first byte the borrower reaches, as
/// `FnPrinter::lent_args` finds it.
fn holder(lends: &Lends, f: u32, k: u32, r: u8, o: i32) -> Option<i32> {
    let (lo, hi) = lends.reach.get(&(k, r)).copied()?;
    if lo >= hi {
        return None;
    }
    lends
        .objects
        .get(&f)?
        .iter()
        .find(|&&(a, b)| a <= o + lo && o + lo < b)
        .map(|&(a, _)| a)
}

/// Whether every frame access to `x`'s bytes fits its type, and nothing
/// else of the frame lies part in them: no other lent object, no slot
/// reaching past them, no access placed only by arithmetic.
fn frame_fits(
    t: &Types,
    fr: &Frame,
    lent: &BTreeSet<i32>,
    passed: &BTreeSet<i32>,
    x: &Typed,
    objs: &[(i32, i32)],
) -> bool {
    let (lo, hi) = (x.lo, x.lo + t.size(&x.ty) as i32);
    // Another lent object in the bytes is whole in them: one local.
    if objs
        .iter()
        .any(|&(a, b)| a < hi && lo < b && (a < lo || b > hi))
    {
        return false;
    }
    // A slot in the bytes: whole in them, and lent, a local, or no bytes
    // but an address lent (`lo..lo`, passed and never accessed).
    if fr.slots.iter().any(|s| {
        let ok = !s.escapes()
            || lent.contains(&s.lo)
            || (s.lo == s.hi && s.escapes == Kind::Call as u8 && passed.contains(&s.lo));
        s.lo < hi && lo < s.hi.max(s.lo + 1) && (s.lo < lo || s.hi > hi || !ok)
    }) {
        return false;
    }
    fr.places.values().all(|p| match p {
        Place::Exact(o, n) => {
            let end = o + i32::from(*n);
            if end <= lo || hi <= *o {
                return true;
            }
            *o >= lo && end <= hi && t.scalar_at(&x.ty, (o - lo) as u32, u32::from(*n)).is_some()
        }
        Place::Through(ks) => !ks.iter().any(|&k| lo <= k && k < hi),
        Place::Elsewhere => true,
    })
}

/// Whether every access through borrowed parameter `k` fits `ty` from its
/// start.
fn through_fits(t: &Types, lends: &Lends, (f, r): (u32, u8), ty: &Ty) -> bool {
    lends.through.get(&f).is_none_or(|m| {
        m.values()
            .filter(|th| th.0 == r)
            .all(|&(_, k, n)| k >= 0 && t.scalar_at(ty, k as u32, u32::from(n)).is_some())
    })
}
