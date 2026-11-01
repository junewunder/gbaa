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
#[derive(Debug, Default)]
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
}

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
}

/// Names an accessor can't take: `Ptr`'s own methods and Rust's keywords.
const PTR_METHODS: &[&str] = &[
    "at", "field", "idx", "clone", "self", "Self", "super", "crate",
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
            let size = item.size.map(|s| s as u32);
            let ty = t.parse(c, size, &enums);
            t.globals.insert(k, (ty, c.clone()));
        }
        for (&k, item) in &m.functions {
            if let Some(p) = item
                .proto
                .as_deref()
                .and_then(|c| t.proto(m, c, &item.name, &enums))
            {
                t.protos.insert(k, p);
            }
        }
        t
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
        Some(Proto { args, returns })
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
        let values = self.named.iter().map(|(n, (x, fam, file))| {
            let (id, x) = (
                format_ident!("{n}"),
                proc_macro2::Literal::u32_unsuffixed(*x),
            );
            let doc = format!(" One of `{fam}_*`, pret's `{file}`.");
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
                #(#markers)*
            }
        }
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
                returns: true
            })
        );
        assert_eq!(
            p(0x0800_0011),
            Some(Proto {
                args: vec![a("save", Some(save)), a("buf", Some(Ty::U8))],
                returns: false
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
