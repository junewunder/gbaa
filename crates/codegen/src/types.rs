//! The readable form's types (`docs/mod-view-design.md`, the spike's gaps
//! 6 and 8): each typed datum of the source map as a `Ptr<T>` constant,
//! each struct and union of its `[types]` as a marker type with one
//! accessor per field, and the path an access at a constant offset into
//! one names (`gTasks.idx(15).next()`).
//!
//! Types are C's as the map writes them, read only as far as layout goes:
//! every scalar is `u8`, `u16` or `u32` by its size (values stay
//! register-wide; signedness shows in the code's own shifts), a pointer is
//! `Ptr<T>`, an array `[T; N]`, a struct or union its marker, and anything
//! else bytes, `[u8; N]`. The width an access prints is always its
//! instruction's: a path names a field only where the field's width is the
//! access's, and a bitfield only where the access is the narrowest one
//! covering it and, when that byte holds others, the code's mask or shifts
//! pick out its bits. Anything else prints as `.field::<uN>(off)` from the
//! deepest object that holds it.
//!
//! A field that takes a family of pret's named values (`[values]`,
//! mod/value-names) has its value-to-name table here, and each name is a
//! `u32` constant of the `globals` module, so a store or compare of the
//! field prints `OPTIONS_TEXT_SPEED_MID`, not `1`.

use crate::names::{Kind, SourceMap};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};

/// A C type, as far as its layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    U8,
    U16,
    U32,
    Ptr(Box<Ty>),
    Array(Box<Ty>, u32),
    /// A struct or union of `[types]`, by its C name.
    Named(String),
    /// Bytes of no type the map gives.
    Bytes(u32),
}

/// One step of a path into an object: a field's accessor or an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Field(String),
    Idx(u32),
}

/// A struct's or union's layout, its fields' types read.
#[derive(Clone, Debug, Default)]
struct Def {
    size: u32,
    /// The marker type's identifier.
    ident: String,
    /// The C name and kind, for its doc comment.
    c: String,
    /// A union (`[types]`' kind), whose fields share bytes.
    union: bool,
    fields: Vec<F>,
}

/// A field and its accessor.
#[derive(Clone, Debug)]
struct F {
    ident: String,
    /// The accessor's offset and type: the field's own, or for a bitfield
    /// the narrowest aligned access holding its bits.
    off: u32,
    ty: Ty,
    /// A bitfield's bits, counted from the accessor's offset.
    bits: Option<(u32, u32)>,
    /// The C declaration, for the accessor's doc comment.
    c: String,
    /// The family of named values it takes (`[values]`).
    values: Option<String>,
}

/// The typed view of a source map: its markers and typed globals.
#[derive(Clone, Debug, Default)]
pub struct Types {
    defs: BTreeMap<String, Def>,
    /// Each typed datum's type, by address.
    globals: BTreeMap<u32, (Ty, String)>,
    /// Each function's prototype as far as its registers, by key.
    protos: BTreeMap<u32, Proto>,
    /// Each family of named values a field takes: its values' names, by
    /// value.
    values: BTreeMap<String, BTreeMap<u32, String>>,
    /// Each value's name: its value, its family and the family's header.
    named: BTreeMap<String, (u32, String, String)>,
    /// Each enum's size, by its C name.
    enums: BTreeMap<String, u32>,
    /// The family each global that takes one takes (`[values]`'
    /// `globals`), by the datum's address.
    global_values: BTreeMap<u32, String>,
    /// The family each of a function's argument registers takes, where
    /// its parameter's C type is an enum family's (`PenState state`), by
    /// key and register.
    arg_values: BTreeMap<(u32, usize), String>,
}

/// A struct as `Ram`'s `ram_t` lays it out ([`Types::layout`]).
#[derive(Clone, Debug)]
pub struct Layout<'a> {
    /// Its marker's identifier.
    pub ident: &'a str,
    pub size: u32,
    /// Each field's identifier, offset and type, by offset: a bitfield's
    /// is its storage's, one field for the bitfields sharing bytes.
    pub fields: Vec<(String, u32, Ty)>,
    pub bits: Vec<Bitfield<'a>>,
}

/// A bitfield of a [`Layout`]: its identifier, its storage field's, its
/// bits in that field (lowest, width), and its C declaration.
#[derive(Clone, Debug)]
pub struct Bitfield<'a> {
    pub ident: &'a str,
    pub storage: String,
    pub lo: u32,
    pub n: u32,
    pub c: &'a str,
}

/// A field as [`Types::fields_of`] gives it: identifier, offset, type and
/// whether it is a bitfield.
pub type FieldOf<'a> = (&'a str, u32, &'a Ty, bool);

/// A function's prototype as the readable form prints it (the spike's gap
/// 7): what arrives in r0–r3 and whether r0 is the value it returns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proto {
    /// Each argument register's parameter, as its identifier and, for a
    /// pointer, what it points at (an integer is register-wide). Ends at
    /// the first parameter that isn't one register (a struct by value, a
    /// 64-bit integer, `...`) or past r3; `None` for one with no name.
    pub args: Vec<Option<(String, Option<Ty>)>>,
    /// It returns a value in r0: not `void`, a struct (returned through a
    /// hidden first argument, so it has no `args` either) or 64 bits.
    pub returns: bool,
    /// What the value returned points at, if it is a pointer (but
    /// `void *`).
    pub ret: Option<Ty>,
}

/// Names an accessor can't take: `Ptr`'s own methods and Rust's keywords.
const PTR_METHODS: &[&str] = &[
    "at", "field", "idx", "add", "clone", "self", "Self", "super", "crate",
];

impl Types {
    /// The types of `m`'s `[types]` and typed `[data]`, its identifiers
    /// already built.
    pub fn new(m: &SourceMap) -> Types {
        let mut t = Types::default();
        // Markers first (sizes and identifiers), then fields, which need
        // every struct's size.
        let reserved = crate::names::reserved();
        let mut taken: BTreeSet<String> = BTreeSet::new();
        for (name, d) in &m.types {
            if d.kind == "enum" {
                continue;
            }
            let bare = name
                .strip_prefix("struct ")
                .or_else(|| name.strip_prefix("union "))
                .unwrap_or(name);
            let Some(mut id) = crate::names::as_ident(bare) else {
                continue;
            };
            if reserved.contains(&id) || crate::names::printer_pattern(&id) {
                id.push('_');
            }
            while !taken.insert(id.clone()) {
                id.push('_');
            }
            t.defs.insert(
                name.clone(),
                Def {
                    size: d.size,
                    ident: id,
                    c: name.clone(),
                    union: d.kind == "union",
                    fields: Vec::new(),
                },
            );
        }
        let enums: BTreeMap<&str, u32> = m
            .types
            .iter()
            .filter(|(_, d)| d.kind == "enum")
            .map(|(n, d)| (n.as_str(), d.size))
            .collect();
        // The named values first: their names are the globals module's,
        // so none may be a datum's, a marker's or an I/O register's.
        let mut takes: BTreeMap<(&str, &str), &str> = BTreeMap::new();
        for (fam, v) in &m.values {
            let mut by_value = BTreeMap::new();
            for (n, x) in &v.names {
                let ok = crate::names::as_ident(n).as_deref() == Some(n.as_str())
                    && !reserved.contains(n)
                    && !crate::names::printer_pattern(n)
                    && !taken.contains(n)
                    && m.key_of(n).is_none()
                    && !n.starts_with("REG_");
                let free = t.named.get(n).is_none_or(|(y, _, _)| y == x);
                if ok && free {
                    by_value.insert(*x, n.clone());
                    t.named
                        .entry(n.clone())
                        .or_insert((*x, fam.clone(), v.file.clone()));
                }
            }
            if by_value.is_empty() {
                continue;
            }
            t.values.insert(fam.clone(), by_value);
            for (ty, f) in &v.fields {
                takes.insert((ty.as_str(), f.as_str()), fam.as_str());
            }
            for g in &v.globals {
                for (&k, item) in &m.data {
                    if &item.name == g {
                        t.global_values.insert(k, fam.clone());
                    }
                }
            }
        }
        let mut fields = BTreeMap::new();
        for (name, d) in &m.types {
            if !t.defs.contains_key(name) {
                continue;
            }
            let mut seen = BTreeSet::new();
            let mut fs = Vec::new();
            for f in &d.fields {
                let Some(mut id) = crate::names::as_ident(&f.name) else {
                    continue;
                };
                if crate::names::KEYWORDS.contains(&id.as_str())
                    || PTR_METHODS.contains(&id.as_str())
                {
                    id.push('_');
                }
                if !seen.insert(id.clone()) {
                    continue;
                }
                let (off, ty, bits, c) = match f.bits {
                    Some((lo, w)) if w > 0 => {
                        let (a, n) = narrowest(f.size, lo, w);
                        let c = format!("{} {} : {w}", f.ty, f.name);
                        (f.off + a, by_size(n), Some((lo - 8 * a, w)), c)
                    }
                    Some(_) => continue,
                    None => {
                        let ty = t.parse(&f.ty, Some(f.size), &enums);
                        (f.off, ty, None, format!("{} {}", f.ty, f.name))
                    }
                };
                let values = takes
                    .get(&(name.as_str(), f.name.as_str()))
                    .map(|s| s.to_string());
                fs.push(F {
                    ident: id,
                    off,
                    ty,
                    bits,
                    c,
                    values,
                });
            }
            fields.insert(name.clone(), fs);
        }
        for (name, fs) in fields {
            t.defs.get_mut(&name).unwrap().fields = fs;
        }
        for (&k, item) in &m.data {
            let Some(c) = &item.proto else { continue };
            if m.ident(Kind::Data, k).is_none() {
                continue;
            }
            // Size 0 is a label's (a global the linker script places,
            // typed by its declarations): its size isn't known.
            let size = item.size.filter(|&s| s > 0).map(|s| s as u32);
            let mut ty = t.parse(c, size, &enums);
            // An array of no stated length and no size runs to the next
            // datum (katam's `extern struct Kirby gKirbys[];`).
            if let (Ty::Array(e, 0), None) = (&ty, size)
                && let Some((&next, _)) = m.data.range(k + 1..).next()
                && let es = t.size(e)
                && es > 0
            {
                ty = Ty::Array(e.clone(), (next - k) / es);
            }
            t.globals.insert(k, (ty, c.clone()));
        }
        t.enums = enums.iter().map(|(n, s)| (n.to_string(), *s)).collect();
        for (&k, item) in &m.functions {
            if let Some(p) = item
                .proto
                .as_deref()
                .and_then(|c| t.proto(m, c, &item.name, &enums))
            {
                // An argument whose type names a family of named values
                // (an enum's: `[values."PenState"]`) takes it.
                if let Some((_, params)) = item
                    .proto
                    .as_deref()
                    .and_then(|c| crate::dwarf::proto_params(c, &item.name))
                {
                    for (r, (_, ty)) in params.iter().enumerate().take(p.args.len()) {
                        let ty = normal(ty);
                        let ty = ty.strip_prefix("enum ").unwrap_or(&ty);
                        let fam = ty.strip_suffix('_').unwrap_or(ty);
                        if t.values.contains_key(fam) {
                            t.arg_values.insert((k, r), fam.to_string());
                        }
                    }
                }
                t.protos.insert(k, p);
            }
        }
        t
    }

    /// The names of the values function `key`'s argument in register `r`
    /// takes, by value, where its parameter's type is an enum family's.
    pub fn values_of_arg(&self, key: u32, r: usize) -> Option<&BTreeMap<u32, String>> {
        self.values.get(self.arg_values.get(&(key, r))?)
    }

    /// Whether the map gives any function's prototype.
    pub fn has_protos(&self) -> bool {
        !self.protos.is_empty()
    }

    /// The function `key`'s prototype, if the map gives one.
    pub fn proto_of(&self, key: u32) -> Option<&Proto> {
        self.protos.get(&key)
    }

    /// The prototype `c` of the function `name`, as far as its registers.
    fn proto(
        &self,
        m: &SourceMap,
        c: &str,
        name: &str,
        enums: &BTreeMap<&str, u32>,
    ) -> Option<Proto> {
        let (ret, params) = crate::dwarf::proto_params(c, name)?;
        let ret = normal(&ret);
        let to = self.pointee(&ret);
        let returns = match ret.as_str() {
            "void" => false,
            r => match self.read(r, enums) {
                Some(Ty::Named(_)) => return None,
                Some(_) => true,
                None => false,
            },
        };
        let mut args = Vec::new();
        for (n, ty) in params.iter().take(4) {
            let ty = normal(ty);
            let ty = match ty.find('[') {
                // An array parameter is a pointer to its first element.
                Some(i) if !ty.contains('(') => self
                    .read(ty[..i].trim(), enums)
                    .map(|e| Ty::Ptr(Box::new(e))),
                _ if ty == "..." => None,
                _ => self.read(&ty, enums),
            };
            let ty = match ty {
                Some(Ty::Ptr(to)) => Some(*to),
                Some(Ty::U8 | Ty::U16 | Ty::U32) => None,
                _ => break,
            };
            args.push(n.as_deref().and_then(|n| m.local_ident(n)).map(|n| (n, ty)));
        }
        Some(Proto {
            args,
            returns,
            ret: to.filter(|_| returns),
        })
    }

    /// What `c`, a C type as the map writes it, points at, if it is a
    /// pointer to something the map types: not `void *`, nor a pointer to
    /// a function.
    pub fn pointee(&self, c: &str) -> Option<Ty> {
        let text = normal(c);
        if text.contains('(') || text.contains('[') {
            return None;
        }
        let to = text.strip_suffix('*')?.trim();
        if to == "void" {
            return None;
        }
        let enums: BTreeMap<&str, u32> = self.enums.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        self.read(to, &enums)
    }

    /// `c`, a C type as the map writes it, of `size` bytes if known: its
    /// layout, or bytes of that size where it can't be read or its size
    /// isn't `size`.
    fn parse(&self, c: &str, size: Option<u32>, enums: &BTreeMap<&str, u32>) -> Ty {
        let text = normal(c);
        let text = text.as_str();
        // An array of a type the map doesn't give (`IntrFunc [14]`): its
        // elements by their size.
        let ty = self.read(text, enums).or_else(|| {
            let (i, n) = (text.find('[')?, size?);
            let dims: Vec<u32> = text[i..]
                .split(['[', ']'])
                .filter(|s| !s.trim().is_empty())
                .map(|d| d.trim().parse().ok())
                .collect::<Option<_>>()?;
            let all = dims.iter().try_fold(1u32, |a, &d| a.checked_mul(d))?;
            if all == 0 || n % all != 0 {
                return None;
            }
            let base = by_size(n / all);
            Some(
                dims.iter()
                    .rev()
                    .fold(base, |t, &d| Ty::Array(Box::new(t), d)),
            )
        });
        // An array of no stated length takes its size's.
        let ty = match (ty, size) {
            (Some(Ty::Array(e, 0)), Some(n)) => {
                let es = self.size(&e);
                (es > 0 && n % es == 0).then(|| Ty::Array(e, n / es))
            }
            (ty, _) => ty,
        };
        match (ty, size) {
            (Some(ty), None) => ty,
            (Some(ty), Some(n)) if self.size(&ty) == n => ty,
            (_, Some(n)) => by_size(n),
            (None, None) => Ty::Bytes(1),
        }
    }

    fn read(&self, t: &str, enums: &BTreeMap<&str, u32>) -> Option<Ty> {
        if t.contains('(') {
            // A function pointer is a word; a function type or an array of
            // them is left to its size.
            let ptr = t.contains("(*)") && !t.ends_with(']');
            return ptr.then_some(Ty::U32);
        }
        if let Some(i) = t.find('[') {
            let mut ty = self.read(t[..i].trim(), enums)?;
            let dims: Vec<&str> = t[i..]
                .split(['[', ']'])
                .filter(|s| !s.trim().is_empty())
                .collect();
            for d in dims.iter().rev() {
                ty = Ty::Array(Box::new(ty), d.trim().parse().ok()?);
            }
            if t[i..].starts_with("[]") {
                ty = Ty::Array(Box::new(ty), 0);
            }
            return Some(ty);
        }
        if let Some(p) = t.strip_suffix('*') {
            let to = self.read(p.trim(), enums).unwrap_or(Ty::U8);
            return Some(Ty::Ptr(Box::new(to)));
        }
        let scalar = match t {
            "u8" | "s8" | "bool8" | "vu8" | "vs8" | "char" | "unsigned char" | "signed char"
            | "_Bool" | "void" => Some(Ty::U8),
            "u16" | "s16" | "bool16" | "vu16" | "vs16" | "short" | "short int"
            | "short unsigned int" | "unsigned short" | "short signed int" => Some(Ty::U16),
            "u32" | "s32" | "bool32" | "vu32" | "vs32" | "int" | "unsigned int" | "unsigned"
            | "signed int" | "long" | "long int" | "long unsigned int" | "unsigned long"
            | "long signed int" | "float" => Some(Ty::U32),
            _ => None,
        };
        if scalar.is_some() {
            return scalar;
        }
        if let Some(&n) = enums.get(t) {
            return Some(by_size(n));
        }
        // The typedef an enum family is keyed by (mzm's `MAKE_ENUM`:
        // `typedef u8 PenState`, its values `enum PenState_`): an integer,
        // register-wide (a field's size, when known, gives its width).
        if self.values.contains_key(t) {
            return Some(Ty::U32);
        }
        self.defs.contains_key(t).then(|| Ty::Named(t.to_string()))
    }

    /// `ty`'s size in bytes.
    pub fn size(&self, ty: &Ty) -> u32 {
        match ty {
            Ty::U8 => 1,
            Ty::U16 => 2,
            Ty::U32 | Ty::Ptr(_) => 4,
            Ty::Array(e, n) => self.size(e).saturating_mul(*n),
            Ty::Named(n) => self.defs.get(n).map_or(0, |d| d.size),
            Ty::Bytes(n) => *n,
        }
    }

    /// Whether the map typed no datum.
    pub fn is_empty(&self) -> bool {
        self.globals.is_empty()
    }

    /// The marker type's identifier of the struct or union `name`.
    pub fn marker(&self, name: &str) -> Option<&str> {
        self.defs.get(name).map(|d| d.ident.as_str())
    }

    /// The field of the struct or union `ty` that holds byte `off` (not a
    /// bitfield), and the offset in it.
    pub fn field_at(&self, ty: &Ty, off: u32) -> Option<(&Ty, u32)> {
        let Ty::Named(n) = ty else { return None };
        self.defs
            .get(n)?
            .fields
            .iter()
            .filter(|f| f.bits.is_none())
            .find(|f| (f.off..f.off + self.size(&f.ty)).contains(&off))
            .map(|f| (&f.ty, off - f.off))
    }

    /// The struct or union `name`'s size and each field's identifier,
    /// offset, type and whether it is a bitfield (its accessor's bytes),
    /// in the map's order (native/lift-mod-run-edit-report: the per-function
    /// check names a RAM byte by them).
    pub fn fields_of(&self, name: &str) -> Option<(u32, Vec<FieldOf<'_>>)> {
        let d = self.defs.get(name)?;
        let fs = d
            .fields
            .iter()
            .map(|f| (f.ident.as_str(), f.off, &f.ty, f.bits.is_some()))
            .collect();
        Some((d.size, fs))
    }

    /// The struct `name` as `Ram`'s `ram_t` lays it out
    /// (native/lift-struct-globals): its fields by offset, none
    /// overlapping. The bitfields whose accessors share bytes are one
    /// storage field (native/lift-global-bitfields), the unsigned integer
    /// of their bytes, or wider over padding up to the next field where
    /// that is one, named for them all (`left_up_speed_hue`; one alone:
    /// `<field>_bits`). `None` for a union, one whose fields overlap, or
    /// one with bitfields' bytes no aligned integer holds.
    pub fn layout(&self, name: &str) -> Option<Layout<'_>> {
        let d = self.defs.get(name)?;
        if d.union {
            return None;
        }
        // Each bitfield's accessor bytes; runs of them that overlap are
        // one storage.
        let mut bfs: Vec<&F> = d.fields.iter().filter(|f| f.bits.is_some()).collect();
        bfs.sort_by_key(|f| (f.off, f.bits.map_or(0, |b| b.0)));
        let mut groups: Vec<(u32, u32, Vec<&F>)> = Vec::new();
        for f in bfs {
            let (a, b) = (f.off, f.off + self.size(&f.ty));
            match groups.last_mut() {
                Some((_, hi, fs)) if a < *hi => {
                    *hi = (*hi).max(b);
                    fs.push(f);
                }
                _ => groups.push((a, b, vec![f])),
            }
        }
        let mut fs: Vec<(String, u32, Ty)> = d
            .fields
            .iter()
            .filter(|f| f.bits.is_none())
            .map(|f| (f.ident.clone(), f.off, f.ty.clone()))
            .collect();
        let starts: Vec<u32> = fs
            .iter()
            .map(|f| f.1)
            .chain(groups.iter().map(|g| g.0))
            .collect();
        let mut bits = Vec::new();
        for (lo, hi, mut gfs) in groups {
            // The bits counted from the storage's start, low first.
            let at = |f: &F| 8 * (f.off - lo) + f.bits.map_or(0, |b| b.0);
            gfs.sort_by_key(|f| at(f));
            let next = starts
                .iter()
                .copied()
                .filter(|&s| s >= hi)
                .min()
                .unwrap_or(d.size);
            let fits = |s: u32| s >= hi - lo && lo % s == 0 && lo + s <= d.size;
            let s = [4, 2, 1]
                .into_iter()
                .filter(|&s| fits(s) && lo + s <= next)
                .find(|&s| lo + s == next)
                .or_else(|| [1, 2, 4].into_iter().find(|&s| fits(s) && lo + s <= next))?;
            let ids: Vec<&str> = gfs.iter().map(|f| f.ident.as_str()).collect();
            let storage = match &ids[..] {
                [one] => format!("{one}_bits"),
                _ => ids.join("_"),
            };
            for f in &gfs {
                let (_, n) = f.bits.expect("a bitfield");
                bits.push(Bitfield {
                    ident: &f.ident,
                    storage: storage.clone(),
                    lo: at(f),
                    n,
                    c: &f.c,
                });
            }
            fs.push((storage, lo, by_size(s)));
        }
        fs.sort_by_key(|f| f.1);
        let mut end = 0;
        for (_, off, ty) in &fs {
            if *off < end {
                return None;
            }
            end = off + self.size(ty);
        }
        // Each name once: the fields' (a storage's may be a field's), and
        // the bitfields' methods' (`x` and `set_x`).
        let mut seen = BTreeSet::new();
        if !fs.iter().all(|f| seen.insert(f.0.clone())) {
            return None;
        }
        let mut seen = BTreeSet::new();
        if !bits
            .iter()
            .flat_map(|b| [b.ident.to_string(), format!("set_{}", b.ident)])
            .all(|n| seen.insert(n))
        {
            return None;
        }
        (end <= d.size).then_some(Layout {
            ident: d.ident.as_str(),
            size: d.size,
            fields: fs,
            bits,
        })
    }

    /// Each typed datum: its address and type.
    pub fn globals(&self) -> impl Iterator<Item = (u32, &Ty)> {
        self.globals.iter().map(|(&k, (ty, _))| (k, ty))
    }

    /// The fields named `name` that `ty` holds, at any depth (`gbaa-lift
    /// --find`): each one's offset in `ty`, size and path from `ty`
    /// (`.inner[].maxEnergy`, an array's elements as `[]`), at most
    /// `cap` of them. A field's name is its accessor's, a bitfield's
    /// bytes those of its accessor.
    pub fn spans_of(&self, ty: &Ty, name: &str, cap: usize) -> Vec<(u32, u32, String)> {
        let mut out = Vec::new();
        self.spans_into(ty, name, 0, "", cap, &mut out);
        out
    }

    fn spans_into(
        &self,
        ty: &Ty,
        name: &str,
        at: u32,
        path: &str,
        cap: usize,
        out: &mut Vec<(u32, u32, String)>,
    ) {
        match ty {
            Ty::Array(e, n) => {
                let es = self.size(e);
                if es == 0 {
                    return;
                }
                let one = self.spans_of(e, name, cap);
                if one.is_empty() {
                    return;
                }
                for i in 0..*n {
                    for (o, s, p) in &one {
                        if out.len() >= cap {
                            return;
                        }
                        out.push((at + i * es + o, *s, format!("{path}[]{p}")));
                    }
                }
            }
            Ty::Named(n) => {
                let Some(d) = self.defs.get(n) else { return };
                for f in &d.fields {
                    if out.len() >= cap {
                        return;
                    }
                    let p = format!("{path}.{}", f.ident);
                    if f.ident.trim_end_matches('_') == name {
                        out.push((at + f.off, self.size(&f.ty).max(1), p.clone()));
                    }
                    if f.bits.is_none() {
                        self.spans_into(&f.ty, name, at + f.off, &p, cap, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// The typed datum `x` lands on or in, as its address and type.
    pub fn global_at(&self, x: u32) -> Option<(u32, &Ty)> {
        let (&k, (ty, _)) = self.globals.range(..=x).next_back()?;
        (x - k < self.size(ty).max(1)).then_some((k, ty))
    }

    /// The fields of `ty` naming an access of `w` bytes at `off` exactly,
    /// and the type it lands on. `bits`: the bits of the loaded value the
    /// code keeps (its mask or shifts), which pick one of the bitfields
    /// sharing those bytes.
    pub fn walk(
        &self,
        ty: &Ty,
        off: u32,
        w: u32,
        bits: Option<(u32, u32)>,
    ) -> Option<(Vec<Step>, Ty)> {
        match ty {
            Ty::U8 | Ty::U16 | Ty::U32 | Ty::Ptr(_) => {
                (off == 0 && w == self.size(ty)).then(|| (Vec::new(), ty.clone()))
            }
            Ty::Bytes(_) => None,
            Ty::Array(e, n) => {
                let es = self.size(e);
                if es == 0 || off / es >= *n {
                    return None;
                }
                let (mut p, leaf) = self.walk(e, off % es, w, bits)?;
                p.insert(0, Step::Idx(off / es));
                Some((p, leaf))
            }
            Ty::Named(n) => {
                let d = self.defs.get(n)?;
                for f in d.fields.iter().filter(|f| f.bits.is_none()) {
                    if (f.off..f.off + self.size(&f.ty)).contains(&off)
                        && let Some((mut p, leaf)) = self.walk(&f.ty, off - f.off, w, bits)
                    {
                        p.insert(0, Step::Field(f.ident.clone()));
                        return Some((p, leaf));
                    }
                }
                let here: Vec<&F> = d
                    .fields
                    .iter()
                    .filter(|f| f.bits.is_some() && f.off == off && self.size(&f.ty) == w)
                    .collect();
                let f = match (&here[..], bits) {
                    ([f], _) => f,
                    (_, Some(b)) => {
                        let mut m = here.iter().filter(|f| f.bits == Some(b));
                        match (m.next(), m.next()) {
                            (Some(f), None) => f,
                            _ => return None,
                        }
                    }
                    _ => return None,
                };
                Some((vec![Step::Field(f.ident.clone())], f.ty.clone()))
            }
        }
    }

    /// The bitfields an access of `w` bytes at `off` into `ty` loads or
    /// stores whole with others (native/lift-bitfields): each one's bits,
    /// counted from the access's offset, where the access is the
    /// bitfields' accessor (as [`Types::walk`] finds them: through arrays
    /// and whole fields). Empty where it holds no bitfield that doesn't
    /// fill it.
    pub fn bits_at(&self, ty: &Ty, off: u32, w: u32) -> Vec<(u32, u32)> {
        match ty {
            Ty::Array(e, n) => {
                let es = self.size(e);
                if es == 0 || off / es >= *n {
                    return Vec::new();
                }
                self.bits_at(e, off % es, w)
            }
            Ty::Named(n) => {
                let Some(d) = self.defs.get(n) else {
                    return Vec::new();
                };
                if let Some(f) = d
                    .fields
                    .iter()
                    .find(|f| f.bits.is_none() && (f.off..f.off + self.size(&f.ty)).contains(&off))
                {
                    return self.bits_at(&f.ty, off - f.off, w);
                }
                d.fields
                    .iter()
                    .filter(|f| f.off == off && self.size(&f.ty) == w)
                    .filter_map(|f| f.bits.filter(|b| b.1 < 8 * w))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// An access of `w` bytes at `off` into `ty` that loads or stores
    /// bitfields with others, as one (native/lift-bitfields: a compiler
    /// joins `b->left = 0; b->hue = 1;` into one store): the steps to the
    /// struct holding them (through arrays and whole fields), that
    /// struct's C name, the access's offset in it, and every field of it
    /// with a byte in the access, by offset. `None` where the access holds
    /// no bitfield.
    pub fn joined(
        &self,
        ty: &Ty,
        off: u32,
        w: u32,
    ) -> Option<(Vec<Step>, String, u32, Vec<String>)> {
        match ty {
            Ty::Array(e, n) => {
                let es = self.size(e);
                if es == 0 || off / es >= *n || off % es + w > es {
                    return None;
                }
                let (mut p, s, o, fs) = self.joined(e, off % es, w)?;
                p.insert(0, Step::Idx(off / es));
                Some((p, s, o, fs))
            }
            Ty::Named(n) => {
                let d = self.defs.get(n)?;
                if let Some(f) = d.fields.iter().find(|f| {
                    f.bits.is_none() && f.off <= off && off + w <= f.off + self.size(&f.ty)
                }) {
                    let (mut p, s, o, fs) = self.joined(&f.ty, off - f.off, w)?;
                    p.insert(0, Step::Field(f.ident.clone()));
                    return Some((p, s, o, fs));
                }
                // Each field's bytes: a bitfield's, those its bits are in.
                let bytes = |f: &F| match f.bits {
                    Some((lo, k)) => (f.off + lo / 8, f.off + (lo + k - 1) / 8 + 1),
                    None => (f.off, f.off + self.size(&f.ty)),
                };
                let mut fs: Vec<&F> = d
                    .fields
                    .iter()
                    .filter(|f| {
                        let (a, b) = bytes(f);
                        a < off + w && off < b
                    })
                    .collect();
                if !fs.iter().any(|f| f.bits.is_some()) {
                    return None;
                }
                fs.sort_by_key(|f| (bytes(f).0, f.bits.map_or(0, |b| b.0)));
                let ids = fs.iter().map(|f| f.ident.clone()).collect();
                Some((Vec::new(), n.clone(), off, ids))
            }
            _ => None,
        }
    }

    /// Whether [`Types::walk`] of `bits` lands on a bitfield of exactly
    /// those bits that doesn't fill the access.
    pub fn is_bitfield(&self, ty: &Ty, off: u32, w: u32, bits: (u32, u32)) -> bool {
        let Some((steps, _)) = self.walk(ty, off, w, Some(bits)) else {
            return false;
        };
        let mut ty = ty;
        let mut last = None;
        for s in &steps {
            match (s, ty) {
                (Step::Idx(_), Ty::Array(e, _)) => ty = e,
                (Step::Field(id), Ty::Named(n)) => {
                    let Some(f) = self
                        .defs
                        .get(n)
                        .and_then(|d| d.fields.iter().find(|f| &f.ident == id))
                    else {
                        return false;
                    };
                    (ty, last) = (&f.ty, f.bits);
                }
                _ => return false,
            }
        }
        last == Some(bits) && bits.1 < 8 * w
    }

    /// The path an access of `w` bytes at `off` into `ty` prints: the
    /// steps of [`Types::walk`] if it names the access, else the steps
    /// into the deepest field or element holding all of it and the offset
    /// left (`.field::<uN>(off)`).
    pub fn path(
        &self,
        ty: &Ty,
        off: u32,
        w: u32,
        bits: Option<(u32, u32)>,
    ) -> (Vec<Step>, Option<u32>) {
        if let Some((p, _)) = self.walk(ty, off, w, bits) {
            return (p, None);
        }
        let inner = match ty {
            Ty::Array(e, n) => {
                let es = self.size(e);
                (es > 0 && off / es < *n && off % es + w <= es)
                    .then(|| (Step::Idx(off / es), &**e, off % es))
            }
            Ty::Named(n) => self.defs.get(n).and_then(|d| {
                d.fields
                    .iter()
                    .filter(|f| f.bits.is_none())
                    .find(|f| f.off <= off && off + w <= f.off + self.size(&f.ty))
                    .map(|f| (Step::Field(f.ident.clone()), &f.ty, off - f.off))
            }),
            _ => None,
        };
        match inner {
            Some((s, e, o)) => {
                let (mut p, rest) = self.path(e, o, w, bits);
                p.insert(0, s);
                (p, rest)
            }
            None => (Vec::new(), Some(off)),
        }
    }

    /// The array an index scaled by `s` steps through at `off` into `ty`
    /// (`docs/mod-view-design.md`, gap 5): the steps to it, its element
    /// type, the element `off` falls in and the offset into that element.
    /// On the way, arrays of other element sizes and fields are walked at
    /// `off`; the outermost array of `s`-byte elements is the one.
    pub fn array_at(&self, ty: &Ty, off: u32, s: u32) -> Option<(Vec<Step>, Ty, u32, u32)> {
        let (step, inner, o) = match ty {
            Ty::Array(e, n) => {
                let es = self.size(e);
                if es == 0 || off / es >= *n {
                    return None;
                }
                if es == s {
                    return Some((Vec::new(), (**e).clone(), off / s, off % s));
                }
                (Step::Idx(off / es), &**e, off % es)
            }
            Ty::Named(n) => {
                let f = self.defs.get(n)?.fields.iter().find(|f| {
                    f.bits.is_none()
                        && (f.off..f.off + self.size(&f.ty)).contains(&off)
                        && self.array_at(&f.ty, off - f.off, s).is_some()
                })?;
                (Step::Field(f.ident.clone()), &f.ty, off - f.off)
            }
            _ => return None,
        };
        let (mut p, e, k, o) = self.array_at(inner, o, s)?;
        p.insert(0, step);
        Some((p, e, k, o))
    }

    /// The names of the values the field a path of `ty` ends at takes
    /// ([`Types::walk`]'s steps), by value.
    pub fn values_of(&self, ty: &Ty, steps: &[Step]) -> Option<&BTreeMap<u32, String>> {
        let (mut ty, mut fam) = (ty, None);
        for s in steps {
            (ty, fam) = match (s, ty) {
                (Step::Idx(_), Ty::Array(e, _)) => (&**e, None),
                (Step::Field(id), Ty::Named(n)) => {
                    let f = self.defs.get(n)?.fields.iter().find(|f| &f.ident == id)?;
                    (&f.ty, f.values.as_ref())
                }
                _ => return None,
            };
        }
        self.values.get(fam?)
    }

    /// The type `steps` (whole fields and elements, as [`Types::walk`]
    /// gives them) lead to from `ty`.
    pub fn at_steps<'a>(&'a self, ty: &'a Ty, steps: &[Step]) -> Option<&'a Ty> {
        let mut ty = ty;
        for s in steps {
            ty = match (s, ty) {
                (Step::Idx(_), Ty::Array(e, _)) => e,
                (Step::Field(id), Ty::Named(n)) => {
                    &self.defs.get(n)?.fields.iter().find(|f| &f.ident == id)?.ty
                }
                _ => return None,
            };
        }
        Some(ty)
    }

    /// The names of the values the global at `addr` takes (`[values]`'
    /// `globals`), by value.
    pub fn values_of_global(&self, addr: u32) -> Option<&BTreeMap<u32, String>> {
        self.values.get(self.global_values.get(&addr)?)
    }

    /// Whether any field or global takes a family of named values.
    pub fn has_values(&self) -> bool {
        !self.global_values.is_empty()
            || !self.arg_values.is_empty()
            || self
                .defs
                .values()
                .any(|d| d.fields.iter().any(|f| f.values.is_some()))
    }

    /// A value's name: its value, its family and the family's header
    /// (empty for an enum of the DWARF's).
    pub fn value_of(&self, id: &str) -> Option<(u32, &str, &str)> {
        self.named
            .get(id)
            .map(|(x, fam, file)| (*x, fam.as_str(), file.as_str()))
    }

    /// The value a value's name stands for, if `id` is one.
    pub fn value_named(&self, id: &str) -> Option<u32> {
        self.named.get(id).map(|&(x, _, _)| x)
    }

    /// `ty` as the Rust type a `Ptr` points at.
    pub fn rust(&self, ty: &Ty) -> TokenStream {
        match ty {
            Ty::U8 => quote!(u8),
            Ty::U16 => quote!(u16),
            Ty::U32 => quote!(u32),
            Ty::Ptr(t) => {
                let t = self.rust(t);
                quote!(Ptr<#t>)
            }
            Ty::Array(e, n) => {
                let (e, n) = (self.rust(e), proc_macro2::Literal::u32_unsuffixed(*n));
                quote!([#e; #n])
            }
            Ty::Named(n) => match self.defs.get(n) {
                Some(d) => {
                    let id = format_ident!("{}", d.ident);
                    quote!(#id)
                }
                None => quote!(u8),
            },
            Ty::Bytes(n) => {
                let n = proc_macro2::Literal::u32_unsuffixed(*n);
                quote!([u8; #n])
            }
        }
    }

    /// The generated `globals` module: every typed datum of `names` as a
    /// `Ptr<T>` constant, every struct and union as a marker type with
    /// its size and its fields' accessors.
    pub fn module(&self, names: &SourceMap) -> TokenStream {
        let consts = self.globals.iter().filter_map(|(&k, (ty, c))| {
            let id = format_ident!("{}", names.ident(Kind::Data, k)?);
            let (t, a) = (self.rust(ty), crate::lower::hex(k));
            let doc = format!(" `{c}`");
            Some(quote!(#[doc = #doc] pub const #id: Ptr<#t> = Ptr::at(#a);))
        });
        let markers = self.markers();
        let values = self.named.iter().map(|(n, (x, fam, file))| {
            let (id, x) = (
                format_ident!("{n}"),
                proc_macro2::Literal::u32_unsuffixed(*x),
            );
            // A `#define` family is keyed by its prefix, an enum by its
            // type name (native/lift-value-names).
            let doc = match (
                fam.contains(|c: char| c.is_ascii_lowercase()),
                file.as_str(),
            ) {
                (false, _) => format!(" One of `{fam}_*`, pret's `{file}`."),
                (true, "") => format!(" One of enum `{fam}`'s values."),
                (true, f) => format!(" One of enum `{fam}`'s values, `{f}`."),
            };
            quote!(#[doc = #doc] pub const #id: u32 = #x;)
        });
        let io = crate::io_regs::consts();
        quote! {
            /// The source map's typed data as `Ptr<T>`s and its structs and
            /// unions as marker types, each field an accessor
            /// (`docs/mod-view-design.md`, the spike's gap 6), and the
            /// GBA's I/O registers ([`crate::io_regs`]).
            #[allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]
            pub mod globals {
                use super::prelude::*;
                #(#consts)*
                #(#values)*
                #io
                #markers
            }
        }
    }

    /// Every struct and union as a marker type with its size and its
    /// fields' accessors, as [`Types::module`] declares them: over `Ptr`,
    /// `Guest` and `Bits`, which the module they go in imports.
    pub fn markers(&self) -> TokenStream {
        let markers = self.defs.values().map(|d| {
            let id = format_ident!("{}", d.ident);
            let size = proc_macro2::Literal::u32_unsuffixed(d.size);
            let doc = format!(" `{}`, {} bytes", d.c, d.size);
            let fs = d.fields.iter().map(|f| {
                let (n, t) = (format_ident!("{}", f.ident), self.rust(&f.ty));
                let off = proc_macro2::Literal::u32_unsuffixed(f.off);
                // A bitfield that doesn't fill its access says its bits.
                let to = match f.bits {
                    Some((lo, w)) if w < 8 * self.size(&f.ty) => {
                        let (lo, w) = (
                            proc_macro2::Literal::u32_unsuffixed(lo),
                            proc_macro2::Literal::u32_unsuffixed(w),
                        );
                        quote!(Bits<#t, #lo, #w>)
                    }
                    _ => t.clone(),
                };
                let doc = match f.bits {
                    Some((lo, w)) => format!(
                        " `{}`: bits {lo}-{} of the {} at {}",
                        f.c,
                        lo + w - 1,
                        t.to_string().replace(' ', ""),
                        f.off
                    ),
                    None => format!(" `{}`", f.c),
                };
                quote! {
                    #[doc = #doc]
                    pub const fn #n(self) -> Ptr<#to> {
                        self.field(#off)
                    }
                }
            });
            quote! {
                #[doc = #doc]
                pub enum #id {}
                impl Guest for #id {
                    const SIZE: u32 = #size;
                }
                impl Ptr<#id> {
                    #(#fs)*
                }
            }
        });
        quote!(#(#markers)*)
    }
}

/// A C type's text without `const` and `volatile`, its words one space
/// apart.
fn normal(c: &str) -> String {
    let words: Vec<&str> = c
        .split_whitespace()
        .filter(|w| *w != "const" && *w != "volatile")
        .collect();
    let text = words
        .join(" ")
        .replace("*const", "*")
        .replace("*volatile", "*");
    text.trim().to_string()
}

/// The scalar of `n` bytes, or `n` bytes.
fn by_size(n: u32) -> Ty {
    match n {
        1 => Ty::U8,
        2 => Ty::U16,
        4 => Ty::U32,
        n => Ty::Bytes(n),
    }
}

/// The narrowest aligned access in a storage unit of `size` bytes holding
/// bits `lo..lo + w`: its byte offset in the unit and width.
fn narrowest(size: u32, lo: u32, w: u32) -> (u32, u32) {
    let (a, b) = (lo / 8, (lo + w - 1) / 8);
    for n in [1, 2, 4] {
        let s = a & !(n - 1);
        if b < s + n && s + n <= size {
            return (s, n);
        }
    }
    (0, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0000000000000000000000000000000000000000";

    fn map() -> SourceMap {
        let text = format!(
            r#"[map]
rom_sha1 = "{SHA}"
[data]
0x03005e00 = {{ name = "gTasks", size = 80, type = "struct Task [2]" }}
0x02037350 = {{ name = "gObjs", size = 8, type = "struct Obj [2]" }}
0x03005d90 = {{ name = "gSavePtr", size = 4, type = "struct Save *" }}
0x02000000 = {{ name = "sOdd", size = 6, type = "const const u16 []" }}
0x02000010 = {{ name = "sMystery", size = 3, type = "Mystery" }}
0x02000020 = {{ name = "gIntrTable", size = 56, type = "const IntrFunc [14]" }}
[types."struct Task"]
kind = "struct"
size = 40
fields = [
  {{ name = "func", off = 0, size = 4, type = "TaskFunc" }},
  {{ name = "next", off = 6, size = 1, type = "u8" }},
  {{ name = "data", off = 8, size = 32, type = "s16 [16]" }},
]
[types."struct Obj"]
kind = "struct"
size = 4
fields = [
  {{ name = "dir", off = 0, size = 2, type = "u16", bits = [0, 4] }},
  {{ name = "moveDir", off = 0, size = 2, type = "u16", bits = [4, 4] }},
  {{ name = "type", off = 2, size = 2, type = "u16", bits = [8, 8] }},
]
[types."struct Save"]
kind = "struct"
size = 176
fields = [
  {{ name = "key", off = 172, size = 4, type = "u32" }},
]
"#
        );
        SourceMap::from_texts(SHA, &[&text]).unwrap()
    }

    #[test]
    fn prototypes_give_registers() {
        let text = format!(
            r#"[map]
rom_sha1 = "{SHA}"
[functions]
0x08000001 = {{ name = "Get", proto = "u16 Get(u16 *quantity, u8 id)" }}
0x08000011 = {{ name = "Set", proto = "void Set(struct Save *save, const u8 buf[4])" }}
0x08000021 = {{ name = "Big", proto = "struct Save Big(u8 id)" }}
0x08000031 = {{ name = "Wide", proto = "u32 Wide(u8 lo, u64 b, u8 c)" }}
0x08000041 = {{ name = "Cb", proto = "u8 Cb(void (*m)(u8), u8)" }}
[types."struct Save"]
kind = "struct"
size = 8
fields = []
"#
        );
        let m = SourceMap::from_texts(SHA, &[&text]).unwrap();
        let p = |k: u32| m.typed.proto_of(k).cloned();
        let a = |n: &str, t: Option<Ty>| Some((n.to_string(), t));
        let save = Ty::Named("struct Save".into());
        assert_eq!(
            p(0x0800_0001),
            Some(Proto {
                args: vec![a("quantity", Some(Ty::U16)), a("id", None)],
                returns: true,
                ret: None,
            })
        );
        assert_eq!(
            p(0x0800_0011),
            Some(Proto {
                args: vec![a("save", Some(save)), a("buf", Some(Ty::U8))],
                returns: false,
                ret: None,
            })
        );
        // A struct returned by value takes a hidden first argument.
        assert_eq!(p(0x0800_0021), None);
        // A 64-bit parameter ends the registers' parameters.
        assert_eq!(p(0x0800_0031).unwrap().args, vec![a("lo", None)]);
        // A reserved name takes its `_`; one with no name, `None`.
        assert_eq!(p(0x0800_0041).unwrap().args, vec![a("m_", None), None]);
    }

    #[test]
    fn data_types_read_as_layouts() {
        let m = map();
        let t = &m.typed;
        let (k, ty) = t.global_at(0x0300_5e2e).unwrap();
        assert_eq!(k, 0x0300_5e00);
        assert_eq!(ty, &Ty::Array(Box::new(Ty::Named("struct Task".into())), 2));
        // An array of no stated length takes its size's; a type the map
        // doesn't know is its size in bytes.
        assert_eq!(
            t.global_at(0x0200_0004).unwrap().1,
            &Ty::Array(Box::new(Ty::U16), 3)
        );
        assert_eq!(t.global_at(0x0200_0010).unwrap().1, &Ty::Bytes(3));
        assert_eq!(
            t.global_at(0x0200_0020).unwrap().1,
            &Ty::Array(Box::new(Ty::U32), 14)
        );
        let save = t.global_at(0x0300_5d90).unwrap().1;
        assert_eq!(save, &Ty::Ptr(Box::new(Ty::Named("struct Save".into()))));
        assert_eq!(t.rust(save).to_string(), "Ptr < Save >");
    }

    #[test]
    fn a_label_takes_its_declarations_type() {
        // A global the linker script places has size 0 (a label) and its
        // declarations' type; one of no stated length runs to the next datum.
        let text = format!(
            r#"[map]
rom_sha1 = "{SHA}"
[data]
0x02020ee0 = {{ name = "gKirbys", size = 0, type = "struct Kirby []" }}
0x02020f00 = {{ name = "gLevel", size = 0, type = "struct Kirby" }}
0x02020f08 = {{ name = "gNext", size = 0, type = "u32" }}
[types."struct Kirby"]
kind = "struct"
size = 8
fields = [
  {{ name = "hp", off = 4, size = 2, type = "u16" }},
]
"#
        );
        let m = SourceMap::from_texts(SHA, &[&text]).unwrap();
        let t = &m.typed;
        let kirby = Ty::Named("struct Kirby".into());
        let (k, ty) = t.global_at(0x0202_0ef4).unwrap();
        assert_eq!(
            (k, ty),
            (0x0202_0ee0, &Ty::Array(Box::new(kirby.clone()), 4))
        );
        assert_eq!(t.global_at(0x0202_0f04).unwrap().1, &kirby);
        assert_eq!(t.global_at(0x0202_0f08).unwrap().1, &Ty::U32);
    }

    #[test]
    fn a_path_names_an_access_only_at_its_width() {
        let m = map();
        let t = &m.typed;
        let tasks = t.global_at(0x0300_5e00).unwrap().1.clone();
        let f = |s: &str| Step::Field(s.into());
        // gTasks[1].next, a byte.
        assert_eq!(
            t.path(&tasks, 46, 1, None),
            (vec![Step::Idx(1), f("next")], None)
        );
        // gTasks[1].data[3], a halfword.
        assert_eq!(
            t.path(&tasks, 54, 2, None),
            (vec![Step::Idx(1), f("data"), Step::Idx(3)], None)
        );
        // A byte of a halfword element: the element, then the raw offset.
        assert_eq!(
            t.path(&tasks, 55, 1, None),
            (vec![Step::Idx(1), f("data"), Step::Idx(3)], Some(1))
        );
        // A word over `next` and what follows: the task, then raw.
        assert_eq!(t.path(&tasks, 44, 4, None), (vec![Step::Idx(1)], Some(4)));
        // A function pointer is a word.
        assert_eq!(
            t.path(&tasks, 0, 4, None),
            (vec![Step::Idx(0), f("func")], None)
        );
    }

    #[test]
    fn an_index_steps_through_the_array_of_its_scale() {
        let m = map();
        let t = &m.typed;
        let tasks = t.global_at(0x0300_5e00).unwrap().1.clone();
        let task = Ty::Named("struct Task".into());
        // gTasks[i].next, gTasks[i + 1].func.
        assert_eq!(
            t.array_at(&tasks, 6, 40),
            Some((vec![], task.clone(), 0, 6))
        );
        assert_eq!(t.array_at(&tasks, 40, 40), Some((vec![], task, 1, 0)));
        // gTasks[1].data[j].
        assert_eq!(
            t.array_at(&tasks, 48, 2),
            Some((
                vec![Step::Idx(1), Step::Field("data".into())],
                Ty::U16,
                0,
                0
            ))
        );
        // No array of 12-byte elements; outside the array.
        assert_eq!(t.array_at(&tasks, 6, 12), None);
        assert_eq!(t.array_at(&tasks, 80, 40), None);
    }

    #[test]
    fn a_bitfield_is_named_where_its_bits_are_picked() {
        let m = map();
        let t = &m.typed;
        let objs = t.global_at(0x0203_7350).unwrap().1.clone();
        let f = |s: &str| Step::Field(s.into());
        // `ldrb` of byte 0 and `& 0xf`: dir; `>> 4`: moveDir.
        assert_eq!(
            t.path(&objs, 4, 1, Some((0, 4))),
            (vec![Step::Idx(1), f("dir")], None)
        );
        assert_eq!(
            t.path(&objs, 4, 1, Some((4, 4))),
            (vec![Step::Idx(1), f("moveDir")], None)
        );
        // Both or neither picked: the object and the raw byte.
        assert_eq!(t.path(&objs, 4, 1, None), (vec![Step::Idx(1)], Some(0)));
        assert_eq!(
            t.path(&objs, 4, 1, Some((0, 8))),
            (vec![Step::Idx(1)], Some(0))
        );
        // A halfword over both isn't either's narrowest access.
        assert_eq!(
            t.path(&objs, 4, 2, Some((0, 4))),
            (vec![Step::Idx(1)], Some(0))
        );
        // Alone in its byte (byte 3), a keyword's accessor, unmasked.
        assert_eq!(
            t.path(&objs, 7, 1, None),
            (vec![Step::Idx(1), f("type_")], None)
        );
        // A write's bits pick a bitfield only if they are all of it, and
        // it shares its byte.
        assert!(t.is_bitfield(&objs, 4, 1, (4, 4)));
        assert!(!t.is_bitfield(&objs, 4, 1, (4, 3)));
        assert!(!t.is_bitfield(&objs, 7, 1, (0, 8)));
    }

    #[test]
    fn the_module_declares_globals_and_accessors() {
        let m = map();
        let text = m.typed.module(&m).to_string();
        for want in [
            "pub const gTasks : Ptr < [Task ; 2] > = Ptr :: at (0x0300_5e00) ;",
            "pub const gSavePtr : Ptr < Ptr < Save > >",
            "pub enum Task { }",
            "const SIZE : u32 = 40 ;",
            "pub const fn data (self) -> Ptr < [u16 ; 16] > { self . field (8) }",
            "pub const fn dir (self) -> Ptr < Bits < u8 , 0 , 4 > > { self . field (0) }",
            "pub const fn type_ (self) -> Ptr < u8 > { self . field (3) }",
        ] {
            assert!(text.contains(want), "{want} in {text}");
        }
    }
}
