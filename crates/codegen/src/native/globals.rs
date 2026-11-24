//! RAM globals as typed fields (`docs/native-stage-b.md`, piece 5; unit
//! `native/ram-globals`): byte RAM carved into the globals the program's
//! constant-address accesses find.
//!
//! Each RAM access whose address is a constant in its block is a byte
//! range (its offset in `Game::ram`, EWRAM then IWRAM, and its width),
//! and so is each datum of known size in the names files that one of them
//! reaches. Overlapping ranges are one global; a global's element is the
//! widest access to it, and it is that unsigned integer, or an array of
//! them when it spans more than one (a datum used only as halfwords is
//! `[u16; N]`). A narrower access inside an element reads or writes its
//! bits of it. A global whose ends aren't on that width's alignment (a
//! datum of odd size) takes the widest width they are on, and its wider
//! accesses stay bytes. With no names, accesses that only touch are
//! globals of their own: nothing says they are one array.
//!
//! The globals are the fields of `Ram`. At `--layout guest` (the native
//! check's) it is a `#[repr(C)]` struct at the guest's offsets with byte
//! padding between, so `Ram` is still the guest's EWRAM and IWRAM byte
//! for byte: the prelude derefs it to `[u8; EWRAM + IWRAM]`, which every
//! other access (computed addresses, the stack, the hardware library, the
//! native check's comparison) uses as before. At `--layout packed` (the
//! play layout, unit `native/lift-packed-layout`) the fields are in the
//! lifter's order and the rest of RAM is `bytes`; `g.ram[offset]` finds a
//! byte in the field that holds it (`prelude_packed.rs`). A field's name
//! is the names file's for the datum at its start, else `d_<guest
//! address>`.
//!
//! A global the names files type as a struct (unit
//! `native/lift-struct-globals`) is that struct, `#[repr(C)]` in the
//! `ram_t` module beside `Ram`, so an access prints its field's path
//! (`g.ram.gPen.color`) over the same bytes, not an element (`gPen[3]`):
//! where the datum is exactly the global, the struct has a layout (no
//! union, every field at its type's alignment: [`Types::layout`]) and
//! every access to it lands in one scalar field or covers whole ones,
//! which it then reads or writes each in turn, and no computed access
//! indexes it as an array of its elements. Else it keeps its element
//! type.
//!
//! Its bitfields (unit `native/lift-global-bitfields`) are one storage
//! field for those sharing bytes (`left_up_speed_hue: u8`), so an access
//! the code doesn't say more of reads or writes that, named for them all,
//! as a joined accessor through a pointer is; the `ram_t` struct has a
//! getter and a setter for each (`tick()`, `set_tick(v)`), which an
//! access prints as where [`super::bitfields`] proves it the field's
//! ([`Bit`]).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use crate::types::{self, Step, Ty, Types};

/// EWRAM's size: IWRAM follows it in `Game::ram`.
const EWRAM: u32 = 0x4_0000;
const SIZE: u32 = EWRAM + 0x8000;

/// The offset in `Game::ram` of a guest EWRAM or IWRAM address (mirrors
/// included), as the prelude's `ram_at`.
pub fn ram_at(a: u32) -> Option<u32> {
    match a >> 24 {
        2 => Some(a & 0x3_FFFF),
        3 => Some(EWRAM + (a & 0x7FFF)),
        _ => None,
    }
}

/// The guest address of a `Game::ram` offset.
pub fn guest(off: u32) -> u32 {
    if off < EWRAM {
        0x0200_0000 + off
    } else {
        0x0300_0000 + off - EWRAM
    }
}

/// One global: `len` bytes from offset `lo`, of `elem`-byte elements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Global {
    pub lo: u32,
    pub len: u32,
    /// The element's width, or for a struct its alignment.
    pub elem: u32,
    pub name: String,
    /// The struct it is, if the names files type it as one it can be.
    pub typed: Option<Ty>,
}

impl Global {
    fn ty(&self, types: Option<&Types>) -> String {
        if let (Some(t), Some(ty)) = (types, &self.typed) {
            return rust_ty(t, ty);
        }
        let t = format!("u{}", self.elem * 8);
        if self.len == self.elem {
            t
        } else {
            format!("[{t}; {}]", self.len / self.elem)
        }
    }
}

/// The globals, by their start offset, and the names files' types that
/// the struct globals are of.
#[derive(Clone, Debug, Default)]
pub struct Globals(
    pub BTreeMap<u32, Global>,
    Option<Arc<Types>>,
    /// The names files' value names (`[values]`), where a field or
    /// global takes a family of them (native/lift-value-names).
    Option<Arc<Types>>,
);

/// How one access reaches its global: the place (`g.ram.d_…`,
/// `g.ram.d_…[i]` or a struct's field path), its element's width, and the
/// access's bit shift in it. Or, for an access over several whole fields
/// of a struct, `parts`: each field's place and width, from the lowest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub place: String,
    pub elem: u32,
    pub shift: u32,
    pub parts: Vec<(String, u32)>,
    /// The names of the values the field or global takes, by value
    /// (`[values]`; native/lift-value-names): only for an access of the
    /// whole of it.
    pub values: Option<BTreeMap<u32, String>>,
    /// The bitfield the access names, where the block's code says which
    /// ([`super::bitfields`]; native/lift-global-bitfields): the lift
    /// sets it, [`Globals::place`] never does.
    pub bit: Option<Bit>,
}

/// A struct global's bitfield an access names ([`Globals::bitfield`]):
/// the place of the struct holding it (`g.ram.gClock`), its identifier,
/// which is its getter's (`g.ram.gClock.tick()`, its value) and setter's
/// (`set_tick(v)`), and its lowest bit counted from the access's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bit {
    pub owner: String,
    pub ident: String,
    pub lo: u32,
    /// A load whose every use reads only the field's bits: printed as
    /// the getter, shifted to the access's bits.
    pub read: bool,
    /// A store printed as the setter, its load left out.
    pub fold: Option<super::bitfields::Fold>,
    /// A store printed as the setter of the value it stores shifted
    /// down, its load printed as it is: nothing it keeps of that load
    /// can have changed in memory since ([`super::bitfields::Found::sets`]).
    pub set: Option<u32>,
}

/// `ty` as the Rust type of a `Ram` field or a `ram_t` struct's field: a
/// pointer is a `u32`, a struct its `ram_t` struct.
fn rust_ty(t: &Types, ty: &Ty) -> String {
    match ty {
        Ty::U8 => "u8".into(),
        Ty::U16 => "u16".into(),
        Ty::U32 | Ty::Ptr(_) => "u32".into(),
        Ty::Array(e, n) => format!("[{}; {n}]", rust_ty(t, e)),
        Ty::Named(n) => format!("ram_t::{}", t.marker(n).unwrap_or("u8")),
        Ty::Bytes(n) => format!("[u8; {n}]"),
    }
}

/// `ty`'s alignment as a `ram_t` struct lays it out, if it has a layout
/// ([`Types::layout`]) all the way down and each field is at its
/// alignment.
fn align(t: &Types, ty: &Ty) -> Option<u32> {
    match ty {
        Ty::U8 | Ty::Bytes(_) => Some(1),
        Ty::U16 => Some(2),
        Ty::U32 | Ty::Ptr(_) => Some(4),
        Ty::Array(e, _) => align(t, e),
        Ty::Named(n) => {
            let l = t.layout(n)?;
            let mut a = 1;
            for (_, off, fty) in &l.fields {
                let fa = align(t, fty)?;
                if off % fa != 0 {
                    return None;
                }
                a = a.max(fa);
            }
            (l.size % a == 0).then_some(a)
        }
    }
}

/// A scalar's width: what an access can name whole.
fn scalar(ty: &Ty) -> Option<u32> {
    match ty {
        Ty::U8 => Some(1),
        Ty::U16 => Some(2),
        Ty::U32 | Ty::Ptr(_) => Some(4),
        _ => None,
    }
}

/// The steps as Rust: `.field` and `[i]`.
fn steps(p: &[Step]) -> String {
    let mut s = String::new();
    for st in p {
        match st {
            Step::Field(f) => write!(s, ".{f}").unwrap(),
            Step::Idx(i) => write!(s, "[{i}]").unwrap(),
        }
    }
    s
}

/// The steps from `ty` to the deepest field or element of its `ram_t`
/// layout ([`Types::layout`]) that holds the `w` bytes at `off`, its type
/// and the offset in it.
fn descend(t: &Types, ty: &Ty, off: u32, w: u32) -> Option<(Vec<Step>, Ty, u32)> {
    let inner = match ty {
        Ty::Array(e, n) => {
            let es = t.size(e);
            (es > 0 && off / es < *n && off % es + w <= es)
                .then(|| (Step::Idx(off / es), (**e).clone(), off % es))
        }
        Ty::Named(n) => t
            .layout(n)?
            .fields
            .into_iter()
            .find(|(_, fo, fty)| *fo <= off && off + w <= fo + t.size(fty))
            .map(|(id, fo, fty)| (Step::Field(id), fty, off - fo)),
        _ => None,
    };
    match inner {
        Some((s, e, o)) => {
            let (mut p, leaf, rest) = descend(t, &e, o, w)?;
            p.insert(0, s);
            Some((p, leaf, rest))
        }
        None => Some((Vec::new(), ty.clone(), off)),
    }
}

/// Where an access of `w` bytes at `off` into the struct global `base`
/// of type `ty` is: in one scalar field, or over whole ones.
fn struct_place(t: &Types, ty: &Ty, base: &str, off: u32, w: u32) -> Option<Place> {
    let (p, leaf, o) = descend(t, ty, off, w)?;
    if let Some(s) = scalar(&leaf) {
        return (o + w <= s).then(|| Place {
            place: format!("{base}{}", steps(&p)),
            elem: s,
            shift: o * 8,
            parts: Vec::new(),
            values: (o == 0 && w == s)
                .then(|| t.values_of(ty, &p).cloned())
                .flatten(),
            bit: None,
        });
    }
    // Over whole scalars, side by side.
    let mut parts = Vec::new();
    let mut at = off;
    while at < off + w {
        let (p, s) = [4, 2, 1].into_iter().find_map(|s| {
            let (p, leaf, o) = descend(t, ty, at, s)?;
            (o == 0 && scalar(&leaf) == Some(s)).then_some((p, s))
        })?;
        if at + s > off + w {
            return None;
        }
        parts.push((format!("{base}{}", steps(&p)), s));
        at += s;
    }
    Some(Place {
        place: String::new(),
        elem: w,
        shift: 0,
        parts,
        values: None,
        bit: None,
    })
}

/// The bitfields' getters and setters of the `ram_t` struct `l`
/// (native/lift-global-bitfields): `x()` the field's value, `set_x(v)`
/// its bits set to `v`'s low ones, the rest of its storage kept, as C's
/// `s.x = v` does. Empty for a struct with none.
fn bit_methods(t: &Types, l: &types::Layout) -> String {
    if l.bits.is_empty() {
        return String::new();
    }
    let mut s = format!("    impl {} {{\n", l.ident);
    for b in &l.bits {
        let Some(st) = l.fields.iter().find(|f| f.0 == b.storage) else {
            continue;
        };
        let bits = 8 * t.size(&st.2);
        let ty = format!("u{bits}");
        let low = if b.n >= 32 {
            u32::MAX
        } else {
            (1u32 << b.n) - 1
        };
        let mask = low << b.lo;
        let (f, lo, n, id) = (&b.storage, b.lo, b.n, b.ident);
        let get = match lo {
            0 => format!("self.{f} as u32 & {low:#x}"),
            _ => format!("(self.{f} as u32 >> {lo}) & {low:#x}"),
        };
        let put = match lo {
            0 => format!("v & {mask:#x}"),
            _ => format!("(v << {lo}) & {mask:#x}"),
        };
        let keep = !mask & (u32::MAX >> (32 - bits));
        write!(
            s,
            "        /// `{c}`: bits {lo}-{hi} of `{f}`.
        #[inline(always)]
        pub fn {id}(&self) -> u32 {{
            {get}
        }}
        /// `{id}` set to `v`'s low {n} bits, the rest of `{f}` kept.
        #[inline(always)]
        pub fn set_{id}(&mut self, v: u32) {{
            self.{f} = (self.{f} as u32 & {keep:#x} | {put}) as {ty};
        }}
",
            c = b.c,
            hi = lo + n - 1,
        )
        .unwrap();
    }
    s.push_str("    }\n");
    s
}

impl Globals {
    /// Carve the globals out of `accesses` (offset, width in bytes).
    /// `names` (guest address to identifier and size in bytes, 0 if not
    /// known) names a global that starts at a named datum, and a datum of
    /// known size that an access reaches is one global over all of it.
    ///
    /// `ranges` (offset, end, width) are the extents of computed accesses
    /// the value sets bound (`native/ref-in-object`): each is one object,
    /// so one global, if it reaches at most one of the globals the rest
    /// carve (a loop over an array whose first element is also read at
    /// its constant address); one reaching more is left out. Ranges
    /// aren't weighed against each other: two that overlap may join the
    /// globals they reach.
    pub fn carve(
        accesses: &[(u32, u32)],
        ranges: &[(u32, u32, u32)],
        names: &BTreeMap<u32, (String, u32)>,
    ) -> Globals {
        let first = Globals::carve_all(accesses, &[], names);
        let ranges: Vec<_> = ranges
            .iter()
            .copied()
            .filter(|&(lo, hi, _)| {
                first
                    .0
                    .values()
                    .filter(|g| g.lo < hi && lo < g.lo + g.len)
                    .count()
                    <= 1
            })
            .collect();
        if ranges.is_empty() {
            return first;
        }
        Globals::carve_all(accesses, &ranges, names)
    }

    fn carve_all(
        accesses: &[(u32, u32)],
        ranges: &[(u32, u32, u32)],
        names: &BTreeMap<u32, (String, u32)>,
    ) -> Globals {
        // (offset, end, width): an access, a range of them, or a named
        // datum's extent (width 0).
        let mut xs: Vec<(u32, u32, u32)> = accesses
            .iter()
            .map(|&(o, w)| (o, o + w, w))
            .chain(ranges.iter().copied())
            .filter(|&(_, e, _)| e <= SIZE)
            .collect();
        let extents: Vec<(u32, u32, u32)> = names
            .iter()
            .filter_map(|(&a, &(_, n))| {
                let o = ram_at(a)?;
                (n > 0 && a == guest(o) && o + n <= SIZE).then_some((o, o + n, 0))
            })
            .filter(|&(lo, hi, _)| xs.iter().any(|&(o, e, _)| o < hi && lo < e))
            .collect();
        xs.extend(extents);
        xs.sort_unstable();
        xs.dedup();
        let mut out = BTreeMap::new();
        let mut i = 0;
        while i < xs.len() {
            let (lo, mut hi, mut wmax) = xs[i];
            let mut j = i + 1;
            while j < xs.len() && xs[j].0 < hi {
                hi = hi.max(xs[j].1);
                wmax = wmax.max(xs[j].2);
                j += 1;
            }
            // The widest access, unless the global's ends aren't on its
            // alignment (a named datum's odd size): then the widest that
            // they are on, and the wider accesses stay bytes.
            let mut elem = wmax.max(1);
            while lo % elem != 0 || hi % elem != 0 {
                elem /= 2;
            }
            let g = guest(lo);
            let name = names
                .get(&g)
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| format!("d_{g:08x}"));
            out.insert(
                lo,
                Global {
                    lo,
                    len: hi - lo,
                    elem,
                    name,
                    typed: None,
                },
            );
            i = j;
        }
        Globals(out, None, None)
    }

    /// Type the globals the names files' `types` give a plain struct
    /// that is all of them (module doc), where every access of
    /// `accesses` (offset, width) to one lands in its fields and no
    /// computed range of `ranges` (offset, end, width) is an element of
    /// it ([`Globals::object`]'s `Obj::Index`), which typing would lose.
    /// Any other range (one over many globals, as a loose value set
    /// gives, or narrower than the element) reads and writes the bytes
    /// either way, and the struct is the same bytes
    /// (native/lift-equipment-words).
    pub fn type_structs(
        &mut self,
        types: &Arc<Types>,
        accesses: &[(u32, u32)],
        ranges: &[(u32, u32, u32)],
    ) {
        for g in self.0.values_mut() {
            let a = guest(g.lo);
            let Some((k, ty)) = types.global_at(a) else {
                continue;
            };
            if k != a || !matches!(ty, Ty::Named(_)) || types.size(ty) != g.len {
                continue;
            }
            let Some(al) = align(types, ty) else {
                continue;
            };
            let end = g.lo + g.len;
            let ok = g.lo % al == 0
                && !ranges
                    .iter()
                    .any(|&(lo, hi, w)| lo >= g.lo && hi <= end && w == g.elem && g.len > g.elem)
                && accesses
                    .iter()
                    .filter(|&&(o, w)| o < end && g.lo < o + w)
                    .all(|&(o, w)| {
                        o >= g.lo
                            && o + w <= end
                            && struct_place(types, ty, "", o - g.lo, w).is_some()
                    });
            if ok {
                g.typed = Some(ty.clone());
                g.elem = al;
            }
        }
        self.1 = Some(types.clone());
    }

    /// The `ram_t` module: each struct a struct global is, and those its
    /// fields are, `#[repr(C)]` with byte padding where the C struct has
    /// it. Empty when no global is a struct.
    pub fn structs(&self) -> String {
        let Some(t) = self.1.as_deref() else {
            return String::new();
        };
        let mut todo: Vec<String> = self
            .0
            .values()
            .filter_map(|g| match &g.typed {
                Some(Ty::Named(n)) => Some(n.clone()),
                _ => None,
            })
            .collect();
        let mut done = BTreeMap::new();
        while let Some(n) = todo.pop() {
            if done.contains_key(&n) {
                continue;
            }
            let Some(l) = t.layout(&n) else {
                continue;
            };
            let (id, size) = (l.ident, l.size);
            let mut s = format!(
                "    /// `{n}`, {size} bytes.
    #[repr(C)]
    pub struct {id} {{
"
            );
            let mut at = 0;
            for (f, off, ty) in &l.fields {
                let off = *off;
                if off > at {
                    writeln!(s, "        _p{at}_: [u8; {}],", off - at).unwrap();
                }
                writeln!(
                    s,
                    "        pub {f}: {},",
                    rust_ty(t, ty).replace("ram_t::", "")
                )
                .unwrap();
                at = off + t.size(ty);
                let mut inner = ty;
                while let Ty::Array(e, _) = inner {
                    inner = e;
                }
                if let Ty::Named(m) = inner {
                    todo.push(m.clone());
                }
            }
            if size > at {
                writeln!(s, "        _p{at}_: [u8; {}],", size - at).unwrap();
            }
            writeln!(
                s,
                "    }}
    const _: () = assert!(size_of::<{id}>() == {size});"
            )
            .unwrap();
            s.push_str(&bit_methods(t, &l));
            done.insert(n, s);
        }
        if done.is_empty() {
            return String::new();
        }
        let mut s = String::from(
            "
/// The names files' structs that `Ram`'s struct globals are (native/lift-struct-globals).
#[allow(non_camel_case_types, non_snake_case, dead_code)]
pub mod ram_t {
",
        );
        for d in done.values() {
            s.push_str(d);
        }
        s.push_str(
            "}
",
        );
        s
    }

    /// Where the access of `width` bytes at offset `off` is, if it is in
    /// a global that can hold it as typed (none for an access wider than
    /// its global's element).
    pub fn place(&self, off: u32, width: u32) -> Option<Place> {
        let (_, g) = self.0.range(..=off).next_back()?;
        if off + width > g.lo + g.len {
            return None;
        }
        if let (Some(t), Some(ty)) = (self.1.as_deref(), &g.typed) {
            return struct_place(t, ty, &format!("g.ram.{}", g.name), off - g.lo, width);
        }
        if width > g.elem {
            return None;
        }
        let i = (off - g.lo) / g.elem;
        let place = if g.len == g.elem {
            format!("g.ram.{}", g.name)
        } else {
            format!("g.ram.{}[{i}]", g.name)
        };
        let values = match self.2.as_deref() {
            Some(t) if g.len == g.elem && off == g.lo && width == g.elem => {
                t.values_of_global(guest(g.lo)).cloned()
            }
            _ => None,
        };
        Some(Place {
            place,
            elem: g.elem,
            shift: (off - g.lo - i * g.elem) * 8,
            parts: Vec::new(),
            values,
            bit: None,
        })
    }

    /// The struct global holding the access of `width` bytes at offset
    /// `off`: its type, its place and the access's offset in it.
    fn typed_at(&self, off: u32, width: u32) -> Option<(&Types, &Ty, String, u32)> {
        let (_, g) = self.0.range(..=off).next_back()?;
        if off + width > g.lo + g.len {
            return None;
        }
        Some((
            self.1.as_deref()?,
            g.typed.as_ref()?,
            format!("g.ram.{}", g.name),
            off - g.lo,
        ))
    }

    /// The bitfields the access of `width` bytes at offset `off` into a
    /// struct global holds with others, by their bits counted from the
    /// access's ([`Types::bits_at`]; native/lift-global-bitfields): empty
    /// unless it is their accessor, as [`super::bitfields::find`] takes
    /// them.
    pub fn bits_at(&self, off: u32, width: u32) -> Vec<(u32, u32)> {
        match self.typed_at(off, width) {
            Some((t, ty, _, o)) if t.joined(ty, o, width).is_some() => t.bits_at(ty, o, width),
            _ => Vec::new(),
        }
    }

    /// Whether the access of `width` bytes at offset `off` holds a
    /// bitfield of a struct global ([`Types::joined`]).
    pub fn holds_bits(&self, off: u32, width: u32) -> bool {
        self.typed_at(off, width)
            .is_some_and(|(t, ty, _, o)| t.joined(ty, o, width).is_some())
    }

    /// The bitfield `bits` (one of [`Globals::bits_at`]'s) of the access
    /// of `width` bytes at offset `off`: the place of the struct holding
    /// it and its identifier, if that struct's `ram_t` layout has it.
    pub fn bitfield(&self, off: u32, width: u32, bits: (u32, u32)) -> Option<(String, String)> {
        let (t, ty, base, o) = self.typed_at(off, width)?;
        if !t.is_bitfield(ty, o, width, bits) {
            return None;
        }
        let (mut p, _) = t.walk(ty, o, width, Some(bits))?;
        let Some(Step::Field(id)) = p.pop() else {
            return None;
        };
        // The struct holding it: its `ram_t` struct has its getter.
        let Some(Ty::Named(n)) = t.at_steps(ty, &p) else {
            return None;
        };
        let has = t.layout(n)?.bits.iter().any(|b| b.ident == id);
        has.then(|| (format!("{base}{}", steps(&p)), id))
    }

    /// Name the values a field or global takes (`[values]`;
    /// native/lift-value-names) from `types`.
    pub fn name_values(&mut self, types: Arc<Types>) {
        self.2 = Some(types);
    }

    /// The typed access through a computed address that the value sets
    /// bound to guest addresses `lo..=hi` (`native/ref-in-object`), of
    /// `width` bytes, if every address there is in one global that can
    /// hold it as typed: one address (aligned as the access aligns it) is
    /// that field, the pointer checked ([`Obj::Pin`]); more, an element
    /// of the global's array each, indexed by the pointer ([`Obj::Index`]).
    /// Anything else is pointer arithmetic the types don't explain.
    pub fn object(&self, lo: u32, hi: u32, width: u32) -> Option<Obj> {
        let (lo, hi) = (lo & !(width - 1), hi & !(width - 1));
        let (o_lo, o_hi) = (ram_at(lo)?, ram_at(hi)?);
        if o_hi < o_lo || o_hi - o_lo != hi - lo {
            return None;
        }
        if o_lo == o_hi {
            return self.place(o_lo, width).map(|p| Obj::Pin(p, o_lo));
        }
        let (_, g) = self.0.range(..=o_lo).next_back()?;
        (o_hi + width <= g.lo + g.len && width == g.elem && g.len > g.elem && g.typed.is_none())
            .then(|| Obj::Index {
                global: format!("g.ram.{}", g.name),
                off: g.lo,
                elem: g.elem,
                n: g.len / g.elem,
            })
    }

    /// The `Ram` struct at `layout`.
    pub fn ram_struct(&self, layout: Layout) -> String {
        let s = match layout {
            Layout::Guest => self.ram_guest(),
            Layout::Packed => self.ram_packed(),
        };
        s + &self.structs()
    }

    /// Each global at its offset, bytes between.
    fn ram_guest(&self) -> String {
        let mut s = String::new();
        s.push_str("/// EWRAM then IWRAM at the guest's offsets (`--layout guest`), each global the\n/// constant-address accesses find a typed field (native/ram-globals), bytes between.\n");
        s.push_str("#[repr(C, align(4))]\npub struct Ram {\n");
        let mut at = 0;
        let pad = |s: &mut String, from: u32, to: u32| {
            if to > from {
                writeln!(s, "    _p{from:05x}: [u8; {:#x}],", to - from).unwrap();
            }
        };
        for g in self.0.values() {
            pad(&mut s, at, g.lo);
            writeln!(s, "    pub {}: {},", g.name, g.ty(self.1.as_deref())).unwrap();
            at = g.lo + g.len;
        }
        pad(&mut s, at, SIZE);
        s.push_str("}\n");
        s
    }

    /// The globals in the lifter's order, then `bytes` (all of RAM, the
    /// globals' own bytes there unused), and `RAM_GLOBALS`, which the
    /// packed prelude (`prelude_packed.rs`) finds a byte's place by. The
    /// order is the widest element first (no padding between), then from
    /// the highest guest address down: globals the guest has side by side
    /// aren't, so an access that strays from its field into the next one
    /// reads other bytes here than in the guest layout.
    fn ram_packed(&self) -> String {
        let mut gs: Vec<&Global> = self.0.values().collect();
        gs.sort_by_key(|g| (std::cmp::Reverse(g.elem), std::cmp::Reverse(g.lo)));
        let mut s = String::new();
        s.push_str("/// EWRAM then IWRAM at the play layout (`--layout packed`): each global the\n/// constant-address accesses find a typed field (native/ram-globals) in the lifter's\n/// order, the rest of RAM `bytes`.\n");
        s.push_str("#[repr(C, align(4))]\npub struct Ram {\n");
        for g in &gs {
            writeln!(s, "    pub {}: {},", g.name, g.ty(self.1.as_deref())).unwrap();
        }
        writeln!(s, "    bytes: [u8; {SIZE:#x}],\n}}\n").unwrap();
        s.push_str(
            "/// Each global's RAM offset, size and field offset in `Ram`, by RAM offset.\n",
        );
        writeln!(
            s,
            "const RAM_GLOBALS: [(usize, usize, usize); {}] = [",
            self.0.len()
        )
        .unwrap();
        for g in self.0.values() {
            writeln!(
                s,
                "    ({:#x}, {}, std::mem::offset_of!(Ram, {})),",
                g.lo, g.len, g.name
            )
            .unwrap();
        }
        s.push_str("];\n");
        s
    }
}

/// Where `Ram`'s globals are (`gbaa-lift --layout`; docs/native-port-design.md,
/// section 7, "The check build").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// At the guest's offsets, bytes between: the native check's layout.
    #[default]
    Guest,
    /// In the lifter's order, the rest of RAM bytes: the play layout,
    /// which must draw what the guest layout does.
    Packed,
}

/// A typed access through a computed address ([`Globals::object`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Obj {
    /// Always this place, at this offset in `Game::ram`: the prelude's
    /// `pin` checks the pointer.
    Pin(Place, u32),
    /// An element of the array `global`, `n` elements of `elem` bytes
    /// from offset `off`: the prelude's `ix` finds it from the pointer,
    /// checked.
    Index {
        global: String,
        off: u32,
        elem: u32,
        n: u32,
    },
}

impl Obj {
    /// A load of `width` bytes through the pointer `a` (a `u32`
    /// expression, aligned as the access aligns it), as a `u32`
    /// expression.
    pub fn load(&self, a: &str, width: u32) -> String {
        match self {
            Obj::Pin(p, off) => format!("{{ pin({a}, {off:#x}); {} }}", load(p, width)),
            Obj::Index { elem: 4, .. } => self.elem(a),
            Obj::Index { .. } => format!("{} as u32", self.elem(a)),
        }
    }

    /// A store of `width` bytes of `v` through the pointer `a`, as a
    /// statement.
    pub fn store(&self, a: &str, width: u32, v: &str) -> String {
        match self {
            Obj::Pin(p, off) => format!("pin({a}, {off:#x}); {}", store(p, width, v)),
            Obj::Index { elem: 4, .. } => format!("{} = {v};", self.elem(a)),
            Obj::Index { elem, .. } => format!("{} = {v} as u{};", self.elem(a), elem * 8),
        }
    }

    /// A store of the constant `c` through `a` that fills its field
    /// whole, as [`store_const`]: a pinned field only.
    pub fn store_const(&self, a: &str, width: u32, c: u32) -> Option<String> {
        let Obj::Pin(p, off) = self else { return None };
        Some(format!("pin({a}, {off:#x}); {}", store_const(p, width, c)?))
    }

    fn elem(&self, a: &str) -> String {
        let Obj::Index {
            global,
            off,
            elem,
            n,
        } = self
        else {
            unreachable!()
        };
        format!("{global}[ix({a}, {off:#x}, {elem}, {n})]")
    }
}

/// A load of `width` bytes from `p`, as a `u32` expression.
pub fn load(p: &Place, width: u32) -> String {
    if !p.parts.is_empty() {
        let mut at = 0;
        let mut xs = Vec::new();
        for (f, w) in &p.parts {
            xs.push(match (at, w) {
                (0, 4) => f.clone(),
                (0, _) => format!("({f} as u32)"),
                (s, _) => format!("({f} as u32) << {}", s * 8),
            });
            at += w;
        }
        return format!("({})", xs.join(" | "));
    }
    match (p.shift, width == p.elem, p.elem) {
        (_, true, 4) => p.place.clone(),
        (_, true, _) => format!("{} as u32", p.place),
        (0, _, 4) => format!("({} & {:#x})", p.place, mask(width)),
        (s, _, 4) => format!("({} >> {s} & {:#x})", p.place, mask(width)),
        (0, _, _) => format!("({} as u32 & {:#x})", p.place, mask(width)),
        (s, _, _) => format!("(({} as u32) >> {s} & {:#x})", p.place, mask(width)),
    }
}

/// A store of `width` bytes of the `u32` expression `v` to `p`, as a
/// statement.
pub fn store(p: &Place, width: u32, v: &str) -> String {
    if !p.parts.is_empty() {
        let mut s = format!("{{ let pv: u32 = {v};");
        let mut at = 0;
        for (f, w) in &p.parts {
            match (at, w) {
                (0, 4) => write!(s, " {f} = pv;"),
                (0, _) => write!(s, " {f} = pv as u{};", w * 8),
                (a, _) => write!(s, " {f} = (pv >> {}) as u{};", a * 8, w * 8),
            }
            .unwrap();
            at += w;
        }
        s.push_str(" }");
        return s;
    }
    let t = format!("u{}", p.elem * 8);
    if width == p.elem {
        return if p.elem == 4 {
            format!("{} = {v};", p.place)
        } else {
            format!("{} = {v} as {t};", p.place)
        };
    }
    let (s, m) = (p.shift, mask(width));
    let keep = !(m << s) & mask(p.elem);
    match s {
        0 => format!("{0} = {0} & {keep:#x} | ({v} & {m:#x}) as {t};", p.place),
        _ => format!(
            "{0} = {0} & {keep:#x} | (({v} & {m:#x}) << {s}) as {t};",
            p.place
        ),
    }
}

/// A store of the constant `c` filling `p` whole, as a statement: a bare
/// literal its field's type takes, in decimal below 10000 (a count, an
/// amount: `g.ram.gEquipment.maxEnergy = 99;`), else hex. `None` where
/// `p` is parts, the store fills less than it, or `c` doesn't fit
/// (native/lift-equipment-words).
pub fn store_const(p: &Place, width: u32, c: u32) -> Option<String> {
    if !p.parts.is_empty() || width != p.elem || c & mask(width) != c {
        return None;
    }
    Some(if c < 10_000 {
        format!("{} = {c};", p.place)
    } else {
        format!("{} = {c:#x};", p.place)
    })
}

fn mask(width: u32) -> u32 {
    if width == 4 {
        u32::MAX
    } else {
        (1 << (width * 8)) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> BTreeMap<u32, (String, u32)> {
        BTreeMap::from([
            (0x0300_0010, ("gFrames".to_string(), 0)),
            (0x0200_0100, ("gTiles".to_string(), 6)),
            (0x0200_0300, ("gOdd".to_string(), 6)),
            (0x0200_0400, ("gUnused".to_string(), 8)),
        ])
    }

    #[test]
    fn globals_by_access_width() {
        // A named datum used only as halfwords: a [u16; 3], not bytes.
        let g = Globals::carve(
            &[(0x102, 2), (0x104, 2), (0x200, 4), (0x102, 2)],
            &[],
            &names(),
        );
        let v: Vec<_> = g.0.values().map(|g| (g.lo, g.len, g.elem)).collect();
        assert_eq!(v, vec![(0x100, 6, 2), (0x200, 4, 4)]);
        assert_eq!(g.0[&0x100].ty(None), "[u16; 3]");
        assert_eq!(g.0[&0x100].name, "gTiles");
        assert_eq!(g.0[&0x200].name, "d_02000200");
        assert_eq!(g.0[&0x200].ty(None), "u32");
        let p = g.place(0x104, 2).unwrap();
        assert_eq!(p.place, "g.ram.gTiles[2]");
        assert_eq!(load(&p, 2), "g.ram.gTiles[2] as u32");
        assert_eq!(store(&p, 2, "v3"), "g.ram.gTiles[2] = v3 as u16;");
        assert_eq!(load(&g.place(0x200, 4).unwrap(), 4), "g.ram.d_02000200");
        // Outside every global: bytes. With no names, adjacent halfwords
        // are globals of their own.
        assert_eq!(g.place(0x106, 2), None);
        assert_eq!(
            Globals::carve(&[(0x102, 2), (0x104, 2)], &[], &BTreeMap::new())
                .0
                .len(),
            2
        );
    }

    #[test]
    fn a_byte_inside_a_word_is_its_bits() {
        let g = Globals::carve(&[(EWRAM + 0x10, 4), (EWRAM + 0x12, 1)], &[], &names());
        assert_eq!(g.0.len(), 1);
        assert_eq!(g.0[&(EWRAM + 0x10)].name, "gFrames");
        let p = g.place(EWRAM + 0x12, 1).unwrap();
        assert_eq!((p.elem, p.shift), (4, 16));
        assert_eq!(load(&p, 1), "(g.ram.gFrames >> 16 & 0xff)");
        assert_eq!(
            store(&p, 1, "v1"),
            "g.ram.gFrames = g.ram.gFrames & 0xff00ffff | ((v1 & 0xff) << 16) as u32;"
        );
        let g = Globals::carve(&[(0x20, 2), (0x21, 1)], &[], &BTreeMap::new());
        let p = g.place(0x21, 1).unwrap();
        assert_eq!(load(&p, 1), "((g.ram.d_02000020 as u32) >> 8 & 0xff)");
        assert_eq!(
            store(&p, 1, "v1"),
            "g.ram.d_02000020 = g.ram.d_02000020 & 0xff | ((v1 & 0xff) << 8) as u16;"
        );
    }

    #[test]
    fn an_odd_sized_datum_keeps_its_wide_accesses_as_bytes() {
        // gOdd is 6 bytes: a word at its start doesn't fit its halfwords.
        let g = Globals::carve(&[(0x300, 4), (0x304, 2)], &[], &names());
        assert_eq!(g.0[&0x300].ty(None), "[u16; 3]");
        assert!(g.place(0x300, 4).is_none());
        assert!(g.place(0x304, 2).is_some());
        // A datum no access reaches is no global.
        assert!(!g.0.contains_key(&0x400));
    }

    #[test]
    fn a_pointer_into_one_global_is_its_field_or_element() {
        // Constant accesses carve a word at 0x10 and a byte at 0x40; a
        // loop's halfwords over 0x20..0x28 are an array of their own, and
        // its words over 0x3c..0x44 take in the byte.
        let g = Globals::carve(
            &[(0x10, 4), (0x40, 1)],
            &[(0x20, 0x28, 2), (0x3c, 0x44, 4)],
            &BTreeMap::new(),
        );
        let v: Vec<_> = g.0.values().map(|g| (g.lo, g.len, g.elem)).collect();
        assert_eq!(v, vec![(0x10, 4, 4), (0x20, 8, 2), (0x3c, 8, 4)]);
        // One address: the field, the pointer pinned to it (mirrors too).
        let o = g.object(0x0204_0012, 0x0204_0012, 2).unwrap();
        assert_eq!(
            o.load("v3 & !1", 2),
            "{ pin(v3 & !1, 0x12); (g.ram.d_02000010 >> 16 & 0xffff) }"
        );
        // A range: an index the pointer finds.
        let o = g.object(0x0200_0022, 0x0200_0026, 2).unwrap();
        assert_eq!(
            o.load("v3 & !1", 2),
            "g.ram.d_02000020[ix(v3 & !1, 0x20, 2, 4)] as u32"
        );
        assert_eq!(
            o.store("v3 & !1", 2, "v4"),
            "g.ram.d_02000020[ix(v3 & !1, 0x20, 2, 4)] = v4 as u16;"
        );
        let o = g.object(0x0200_003c, 0x0200_0040, 4).unwrap();
        assert_eq!(
            o.store("v1 & !3", 4, "v2"),
            "g.ram.d_0200003c[ix(v1 & !3, 0x3c, 4, 2)] = v2;"
        );
        // Not explained: bytes of a halfword array, a range past its
        // global, one across a mirror's end.
        assert_eq!(g.object(0x0200_0020, 0x0200_0027, 1), None);
        assert_eq!(g.object(0x0200_0020, 0x0200_002a, 2), None);
        assert_eq!(g.object(0x0203_fffe, 0x0204_0000, 2), None);
    }

    #[test]
    fn a_range_over_several_globals_is_none() {
        // A clear loop over two separate globals (crt0's .bss): no object.
        let g = Globals::carve(&[(0x8, 1), (0xc, 4)], &[(0x8, 0x10, 4)], &BTreeMap::new());
        assert_eq!(g.0.len(), 2);
        assert_eq!(g.object(0x0200_0008, 0x0200_000c, 4), None);
    }

    #[test]
    fn the_struct_is_the_whole_ram() {
        let g = Globals::carve(&[(0x8, 4), (SIZE - 4, 4)], &[], &BTreeMap::new());
        let s = g.ram_struct(Layout::Guest);
        assert!(s.contains("    _p00000: [u8; 0x8],\n    pub d_02000008: u32,\n"));
        assert!(s.contains("    _p0000c: [u8; 0x47ff0],\n    pub d_03007ffc: u32,\n}"));
        let s = Globals::default().ram_struct(Layout::Guest);
        assert!(s.contains("    _p00000: [u8; 0x48000],\n}"));
    }

    #[test]
    fn the_packed_struct_orders_its_fields_and_places_them() {
        let g = Globals::carve(
            &[(0x8, 4), (0xc, 4), (0x20, 2), (0x24, 1)],
            &[],
            &BTreeMap::new(),
        );
        let s = g.ram_struct(Layout::Packed);
        // Widest first, then from the highest address down; then bytes.
        assert!(s.contains(
            "    pub d_0200000c: u32,\n    pub d_02000008: u32,\n    pub d_02000020: u16,\n    pub d_02000024: u8,\n    bytes: [u8; 0x48000],\n}"
        ));
        // The table by RAM offset.
        assert!(s.contains("[(usize, usize, usize); 4] = [\n    (0x8, 4, std::mem::offset_of!(Ram, d_02000008)),\n    (0xc, 4,"));
        let s = Globals::default().ram_struct(Layout::Packed);
        assert!(s.contains("[(usize, usize, usize); 0] = [\n];"));
    }
    fn pen_types() -> Arc<Types> {
        let sha = "0000000000000000000000000000000000000000";
        let text = format!(
            r#"
[map]
rom_sha1 = "{sha}"
source = "test"
format = 1

[data]
0x03000000 = {{ name = "gPen", size = 8, proto = "struct Pen" }}
0x03000010 = {{ name = "gFlags", size = 4, proto = "union Flags" }}
0x03000020 = {{ name = "gMode", size = 2 }}

[types."struct Vec"]
kind = "struct"
size = 4
fields = [
  {{ name = "x", off = 0, size = 2, type = "s16" }},
  {{ name = "y", off = 2, size = 2, type = "s16" }},
]

[types."struct Pen"]
kind = "struct"
size = 8
fields = [
  {{ name = "pos", off = 0, size = 4, type = "struct Vec" }},
  {{ name = "state", off = 4, size = 1, type = "u8" }},
  {{ name = "color", off = 6, size = 2, type = "u16" }},
]

[types."union Flags"]
kind = "union"
size = 4
fields = [
  {{ name = "all", off = 0, size = 4, type = "u32" }},
  {{ name = "lo", off = 0, size = 2, type = "u16" }},
]

[values."PenState"]
fields = ["struct Pen.state"]
globals = ["gMode"]
names = [
  {{ name = "PEN_IDLE", value = 0 }},
  {{ name = "PEN_RIGHT", value = 1 }},
]
"#
        );
        let m = crate::names::SourceMap::from_texts(sha, &[&text]).unwrap();
        Arc::new(m.typed.clone())
    }

    #[test]
    fn a_field_or_global_that_takes_named_values_has_them() {
        let t = pen_types();
        let names = BTreeMap::from([
            (0x0300_0000, ("gPen".to_string(), 8)),
            (0x0300_0020, ("gMode".to_string(), 2)),
        ]);
        let acc = [
            (EWRAM + 4, 1),
            (EWRAM + 6, 2),
            (EWRAM + 0x20, 2),
            (EWRAM + 0x20, 1),
        ];
        let mut g = Globals::carve(&acc, &[], &names);
        g.type_structs(&t, &acc, &[]);
        g.name_values(t.clone());
        let vs = g.place(EWRAM + 4, 1).unwrap().values.unwrap();
        assert_eq!(vs[&1], "PEN_RIGHT");
        assert_eq!(g.place(EWRAM + 6, 2).unwrap().values, None);
        let vs = g.place(EWRAM + 0x20, 2).unwrap().values.unwrap();
        assert_eq!(vs[&0], "PEN_IDLE");
        // A byte of it isn't the global's value.
        assert_eq!(g.place(EWRAM + 0x20, 1).unwrap().values, None);
    }

    #[test]
    fn a_struct_global_is_its_struct() {
        let t = pen_types();
        let names = BTreeMap::from([
            (0x0300_0000, ("gPen".to_string(), 8)),
            (0x0300_0010, ("gFlags".to_string(), 4)),
        ]);
        let acc = [
            (EWRAM, 4),
            (EWRAM + 4, 1),
            (EWRAM + 6, 2),
            (EWRAM + 0x10, 4),
            (EWRAM + 0x10, 2),
        ];
        let mut g = Globals::carve(&acc, &[], &names);
        g.type_structs(&t, &acc, &[]);
        // The union keeps its element type.
        assert_eq!(g.0[&(EWRAM + 0x10)].typed, None);
        let pen = &g.0[&EWRAM];
        assert_eq!(pen.ty(Some(&t)), "ram_t::Pen");
        // A field, a nested field, a byte inside a halfword.
        let p = g.place(EWRAM + 6, 2).unwrap();
        assert_eq!(load(&p, 2), "g.ram.gPen.color as u32");
        assert_eq!(
            load(&g.place(EWRAM + 2, 2).unwrap(), 2),
            "g.ram.gPen.pos.y as u32"
        );
        let p = g.place(EWRAM + 7, 1).unwrap();
        assert_eq!(load(&p, 1), "((g.ram.gPen.color as u32) >> 8 & 0xff)");
        // A word over two whole fields reads and writes both.
        let p = g.place(EWRAM, 4).unwrap();
        assert_eq!(
            load(&p, 4),
            "((g.ram.gPen.pos.x as u32) | (g.ram.gPen.pos.y as u32) << 16)"
        );
        assert_eq!(
            store(&p, 4, "v1"),
            "{ let pv: u32 = v1; g.ram.gPen.pos.x = pv as u16; g.ram.gPen.pos.y = (pv >> 16) as u16; }"
        );
        // A constant filling a field whole is a literal of its type.
        let p = g.place(EWRAM + 6, 2).unwrap();
        assert_eq!(
            store_const(&p, 2, 99).as_deref(),
            Some("g.ram.gPen.color = 99;")
        );
        assert_eq!(
            store_const(&p, 2, 0xff00).as_deref(),
            Some("g.ram.gPen.color = 0xff00;")
        );
        assert_eq!(store_const(&p, 2, 0x1_0000), None);
        assert_eq!(store_const(&g.place(EWRAM + 7, 1).unwrap(), 1, 3), None);
        assert_eq!(store_const(&g.place(EWRAM, 4).unwrap(), 4, 3), None);
        // Padding is in no field: a word over it isn't the struct's.
        assert_eq!(g.place(EWRAM + 4, 4), None);
        let s = g.structs();
        assert!(s.contains("    pub struct Pen {\n        pub pos: Vec,\n        pub state: u8,\n        _p5_: [u8; 1],\n        pub color: u16,\n    }\n"));
        assert!(
            s.contains("    pub struct Vec {\n        pub x: u16,\n        pub y: u16,\n    }\n")
        );
        assert!(
            g.ram_struct(Layout::Guest)
                .contains("    pub gPen: ram_t::Pen,\n")
        );
        // An access the struct's fields can't hold keeps the old type.
        let acc2 = [(EWRAM + 4, 4)];
        let mut g = Globals::carve(&acc2, &[], &names);
        g.type_structs(&t, &acc2, &[]);
        assert_eq!(g.0[&EWRAM].typed, None);
        assert_eq!(g.0[&EWRAM].ty(Some(&t)), "[u32; 2]");
        // So does one a computed range indexes by its elements.
        let mut g = Globals::carve(&acc, &[], &names);
        g.type_structs(&t, &acc, &[(EWRAM, EWRAM + 8, 4)]);
        assert_eq!(g.0[&EWRAM].typed, None);
        // A range narrower than its elements, or over other globals too
        // (a loose value set), isn't an element of it: the struct holds
        // (native/lift-equipment-words).
        let mut g = Globals::carve(&acc, &[], &names);
        g.type_structs(
            &t,
            &acc,
            &[
                (EWRAM, EWRAM + 8, 2),
                (EWRAM, EWRAM + 0x14, 4),
                (EWRAM + 5, EWRAM + 7, 1),
            ],
        );
        assert_eq!(g.0[&EWRAM].typed, Some(Ty::Named("struct Pen".into())));
    }

    fn clock_types() -> Arc<Types> {
        let sha = "0000000000000000000000000000000000000000";
        let text = format!(
            r#"
[map]
rom_sha1 = "{sha}"
source = "test"
format = 1

[data]
0x03000000 = {{ name = "gClock", size = 8, proto = "struct Clock" }}

[types."struct Clock"]
kind = "struct"
size = 8
fields = [
  {{ name = "beats", off = 0, size = 2, type = "u16", bits = [0, 10] }},
  {{ name = "level", off = 0, size = 2, type = "u16", bits = [10, 6] }},
  {{ name = "tick", off = 2, size = 1, type = "u8", bits = [0, 4] }},
  {{ name = "next", off = 2, size = 1, type = "u8", bits = [4, 2] }},
  {{ name = "frames", off = 4, size = 4, type = "u32" }},
]
"#
        );
        let m = crate::names::SourceMap::from_texts(sha, &[&text]).unwrap();
        Arc::new(m.typed.clone())
    }

    #[test]
    fn a_struct_global_with_bitfields_is_its_struct() {
        let t = clock_types();
        let l = t.layout("struct Clock").unwrap();
        let fs: Vec<(&str, u32, &Ty)> =
            l.fields.iter().map(|f| (f.0.as_str(), f.1, &f.2)).collect();
        // The byte's fields take the padding after them: a halfword.
        assert_eq!(
            fs,
            [
                ("beats_level", 0, &Ty::U16),
                ("tick_next", 2, &Ty::U16),
                ("frames", 4, &Ty::U32)
            ]
        );
        // level's accessor is the byte at 1: its bits in the storage.
        let level = l.bits.iter().find(|b| b.ident == "level").unwrap();
        assert_eq!(
            (level.storage.as_str(), level.lo, level.n),
            ("beats_level", 10, 6)
        );
        let names = BTreeMap::from([(0x0300_0000, ("gClock".to_string(), 8))]);
        let acc = [
            (EWRAM, 2),
            (EWRAM + 1, 1),
            (EWRAM + 2, 1),
            (EWRAM, 4),
            (EWRAM + 4, 4),
        ];
        let mut g = Globals::carve(&acc, &[], &names);
        g.type_structs(&t, &acc, &[]);
        assert_eq!(g.0[&EWRAM].ty(Some(&t)), "ram_t::Clock");
        let s = g.structs();
        assert!(
            s.contains(
                "        pub beats_level: u16,
        pub tick_next: u16,
        pub frames: u32,
"
            ),
            "{s}"
        );
        assert!(
            s.contains(
                "        pub fn level(&self) -> u32 {
            (self.beats_level as u32 >> 10) & 0x3f
        }"
            ),
            "{s}"
        );
        assert!(
            s.contains(
                "        pub fn set_level(&mut self, v: u32) {
            self.beats_level = (self.beats_level as u32 & 0x3ff | (v << 10) & 0xfc00) as u16;
        }"
            ),
            "{s}"
        );
        // A byte of a storage, a word over two.
        let p = g.place(EWRAM + 2, 1).unwrap();
        assert_eq!(load(&p, 1), "(g.ram.gClock.tick_next as u32 & 0xff)");
        assert_eq!(
            store(&p, 1, "v1"),
            "g.ram.gClock.tick_next = g.ram.gClock.tick_next & 0xff00 | (v1 & 0xff) as u16;"
        );
        let p = g.place(EWRAM, 4).unwrap();
        assert_eq!(p.parts.len(), 2);
        // The bitfields an access is the accessor of, and their names.
        assert!(g.holds_bits(EWRAM, 4));
        assert!(!g.holds_bits(EWRAM + 4, 4));
        assert_eq!(g.bits_at(EWRAM, 4), []);
        assert_eq!(g.bits_at(EWRAM + 1, 1), [(2, 6)]);
        assert_eq!(g.bits_at(EWRAM + 2, 1), [(0, 4), (4, 2)]);
        assert_eq!(
            g.bitfield(EWRAM + 1, 1, (2, 6)),
            Some(("g.ram.gClock".to_string(), "level".to_string()))
        );
        assert_eq!(g.bitfield(EWRAM + 2, 1, (1, 2)), None);
    }

    #[test]
    fn bitfields_no_integer_holds_keep_the_old_type() {
        // Three bytes of bitfields: no u8, u16 or u32 is them.
        let sha = "0000000000000000000000000000000000000000";
        let text = format!(
            r#"
[map]
rom_sha1 = "{sha}"
source = "test"
format = 1

[types."struct Odd"]
kind = "struct"
size = 4
fields = [
  {{ name = "a", off = 0, size = 2, type = "u16", bits = [4, 10] }},
  {{ name = "b", off = 1, size = 2, type = "u16", bits = [6, 9] }},
  {{ name = "c", off = 3, size = 1, type = "u8" }},
]
"#
        );
        let m = crate::names::SourceMap::from_texts(sha, &[&text]).unwrap();
        assert!(m.typed.layout("struct Odd").is_none());
    }
}
