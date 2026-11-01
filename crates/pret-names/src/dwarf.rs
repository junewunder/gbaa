//! pret's DWARF 2 (agbcc's): each function's prototype and locals, the
//! line table, the globals' types, and struct, union and enum layouts, for
//! the source map's `proto`, `[locals]`, `[lines]` and `[types]`
//! (`docs/mod-view-design.md`, "What it holds").
//!
//! agbcc doesn't terminate its `.debug_abbrev` tables, so each runs on into
//! the next object's, whose codes start again at 1: binutils and gimli
//! both refuse them. [`load`] reads each table up to its first repeated
//! code (where the next one starts), writes them out terminated, and points
//! each unit's header at its copy, before gimli reads anything.

use gimli::{AttributeValue, DwTag, EndianSlice, LittleEndian, Reader as _, constants as dw};
use object::{Object, ObjectSection};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

type R<'a> = EndianSlice<'a, LittleEndian>;
type Unit<'a> = gimli::Unit<R<'a>>;

/// Where a local lives over `[from, to)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    /// A register, r0–r15.
    Reg(u8),
    /// A stack slot at this offset from `sp` (the frame base after the
    /// prologue: agbcc's Thumb frame pointer r7 is `sp` there).
    Slot(i32),
}

/// A parameter or variable of one function, over one address range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub name: String,
    pub home: Home,
    pub from: u32,
    pub to: u32,
    /// Its C type, as text.
    pub ty: String,
    pub param: bool,
}

/// What DWARF says of one function, at its entry address (bit 0 clear).
#[derive(Clone, Debug, Default)]
pub struct Func {
    pub name: String,
    /// `u32 GetMonData2(struct Pokemon *mon, s32 field)`.
    pub proto: String,
    /// The source file of its unit (`src/pokemon.c`).
    pub src: String,
    /// Its end address (exclusive).
    pub end: u32,
    pub locals: Vec<Local>,
    /// Variables with no home DWARF gives (agbcc drops the location of
    /// one it keeps in more than one place), by name.
    pub homeless: Vec<String>,
    /// Its `static` variables: address, name and type.
    pub statics: Vec<(u32, String, String)>,
}

/// One field of a struct or union.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub off: u32,
    pub size: u32,
    pub ty: String,
    /// A bitfield's lowest bit (little-endian, from the start of its
    /// `size`-byte unit) and width.
    pub bits: Option<(u32, u32)>,
}

/// A struct, union or enum layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    Struct {
        size: u32,
        fields: Vec<Field>,
    },
    Union {
        size: u32,
        fields: Vec<Field>,
    },
    Enum {
        size: u32,
        values: Vec<(String, i64)>,
    },
}

/// Everything read from one ELF's DWARF.
#[derive(Debug, Default)]
pub struct Debug {
    /// By entry address (bit 0 clear).
    pub funcs: BTreeMap<u32, Func>,
    /// Each line-table row's address: its file and line. A later row at
    /// one address wins (agbcc writes the statement that starts there last).
    pub lines: BTreeMap<u32, (String, u32)>,
    /// Each global with a fixed address: its name and type.
    pub globals: BTreeMap<u32, (String, String)>,
    /// Each named struct, union or enum, by its type text
    /// (`struct Pokemon`, or a typedef's name for an anonymous one).
    pub types: BTreeMap<String, Layout>,
    /// Type names given two different layouts (statics of two files); the
    /// first is kept.
    pub clashes: Vec<String>,
    /// Units gimli couldn't read even with the abbrevs rewritten.
    pub bad_units: Vec<String>,
    /// Units whose line program stops early: their rows up to there are kept.
    pub bad_lines: Vec<String>,
}

fn uleb(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let byte = *b.get(*at)?;
        *at += 1;
        if shift < 64 {
            v |= u64::from(byte & 0x7f) << shift;
        }
        shift += 7;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
}

/// The abbrev table at `start`, up to its terminator, its first repeated
/// code, or the section's end; as bytes ending in the terminator.
fn abbrev_table(abbrev: &[u8], start: usize) -> Vec<u8> {
    let mut at = start;
    let mut seen = std::collections::HashSet::new();
    let mut end = start;
    while let Some(code) = uleb(abbrev, &mut at) {
        if code == 0 || !seen.insert(code) {
            break;
        }
        let Some(_tag) = uleb(abbrev, &mut at) else {
            break;
        };
        at += 1; // children
        let mut ok = false;
        while let (Some(a), Some(f)) = (uleb(abbrev, &mut at), uleb(abbrev, &mut at)) {
            if a == 0 && f == 0 {
                ok = true;
                break;
            }
        }
        if !ok {
            break;
        }
        end = at;
    }
    let mut t = abbrev[start..end].to_vec();
    t.push(0);
    t
}

/// `.debug_info` and `.debug_abbrev` made readable: every unit's abbrev
/// table cut at its end and terminated, each in a copy of its own.
pub fn fix_abbrevs(info: &[u8], abbrev: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut info = info.to_vec();
    let mut out = Vec::new();
    let mut moved: HashMap<u32, u32> = HashMap::new();
    let mut at = 0usize;
    while at + 11 <= info.len() {
        let len = u32::from_le_bytes(info[at..at + 4].try_into().unwrap()) as usize;
        if len == 0 || len == 0xffff_ffff {
            break;
        }
        let off = u32::from_le_bytes(info[at + 6..at + 10].try_into().unwrap());
        let new = *moved.entry(off).or_insert_with(|| {
            let n = out.len() as u32;
            if (off as usize) < abbrev.len() {
                out.extend(abbrev_table(abbrev, off as usize));
            } else {
                out.push(0);
            }
            n
        });
        info[at + 6..at + 10].copy_from_slice(&new.to_le_bytes());
        at += 4 + len;
    }
    (info, out)
}

/// `.debug_line` with a line program's length made the gap to the next
/// one (or to the section's end) where it falls a few bytes short of it.
/// agbcc's is 1 to 5 bytes short in 27 of Emerald's large files, which
/// cuts off each one's last end-of-sequence.
pub fn fix_lines(line: &[u8], mut starts: Vec<usize>) -> Vec<u8> {
    let mut out = line.to_vec();
    starts.sort_unstable();
    starts.dedup();
    for (i, &at) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(line.len());
        let Some(len) = line.get(at..at + 4) else {
            continue;
        };
        let len = u32::from_le_bytes(len.try_into().unwrap()) as usize;
        // Only a few bytes short: a gap any wider holds a program no unit
        // names (an asm file's).
        let gap = end.saturating_sub(at + 4);
        if (len + 1..=len + 8).contains(&gap) {
            out[at..at + 4].copy_from_slice(&(gap as u32).to_le_bytes());
        }
    }
    out
}

/// gimli's view of the sections, with `.debug_info`, `.debug_abbrev` and
/// `.debug_line` replaced.
fn sections<'a>(
    info: &'a [u8],
    abbrev: &'a [u8],
    line: &'a [u8],
    sect: &dyn Fn(&str) -> &'a [u8],
) -> Result<gimli::Dwarf<R<'a>>, String> {
    gimli::Dwarf::load(|id: gimli::SectionId| -> Result<R<'a>, gimli::Error> {
        let data: &[u8] = match id {
            gimli::SectionId::DebugInfo => info,
            gimli::SectionId::DebugAbbrev => abbrev,
            gimli::SectionId::DebugLine => line,
            _ => sect(id.name()),
        };
        Ok(EndianSlice::new(data, LittleEndian))
    })
    .map_err(|e| e.to_string())
}

/// Reads `elf`'s DWARF.
pub fn load(elf: &[u8]) -> Result<Debug, String> {
    let file = object::File::parse(elf).map_err(|e| e.to_string())?;
    let sect = |name: &str| {
        file.section_by_name(name)
            .and_then(|s| s.data().ok())
            .unwrap_or(&[])
    };
    let (info, abbrev) = fix_abbrevs(sect(".debug_info"), sect(".debug_abbrev"));
    // Each unit's line program, to measure them by.
    let mut starts = Vec::new();
    {
        let dwarf = sections(&info, &abbrev, sect(".debug_line"), &sect)?;
        let mut units = dwarf.units();
        while let Ok(Some(h)) = units.next() {
            if let Ok(u) = dwarf.unit(h)
                && let Some(lp) = &u.line_program
            {
                starts.push(lp.header().offset().0);
            }
        }
    }
    let line = fix_lines(sect(".debug_line"), starts);
    let dwarf = sections(&info, &abbrev, &line, &sect)?;
    let mut d = Debug::default();
    let mut units = dwarf.units();
    loop {
        let header = match units.next() {
            Ok(Some(h)) => h,
            Ok(None) => break,
            Err(e) => {
                d.bad_units.push(format!("unit list: {e}"));
                break;
            }
        };
        let at = header.offset().as_debug_info_offset().map_or(0, |o| o.0);
        let unit = match dwarf.unit(header) {
            Ok(u) => u,
            Err(e) => {
                d.bad_units.push(format!("unit at 0x{at:x}: {e}"));
                continue;
            }
        };
        if let Err(e) = read_unit(&dwarf, &unit, &mut d) {
            d.bad_units.push(format!("unit at 0x{at:x}, reading: {e}"));
        }
    }
    Ok(d)
}

struct Cx<'a, 'u> {
    dwarf: &'a gimli::Dwarf<R<'a>>,
    unit: &'u Unit<'a>,
    names: HashMap<gimli::UnitOffset, String>,
}

impl<'a> Cx<'a, '_> {
    fn entry(
        &self,
        off: gimli::UnitOffset,
    ) -> Option<gimli::DebuggingInformationEntry<'_, '_, R<'a>>> {
        self.unit.entry(off).ok()
    }

    fn name_of(&self, e: &gimli::DebuggingInformationEntry<'_, '_, R<'a>>) -> Option<String> {
        let v = e.attr_value(dw::DW_AT_name).ok()??;
        let s = self.dwarf.attr_string(self.unit, v).ok()?;
        Some(s.to_string_lossy().into_owned())
    }

    fn type_ref(
        &self,
        e: &gimli::DebuggingInformationEntry<'_, '_, R<'a>>,
    ) -> Option<gimli::UnitOffset> {
        match e.attr_value(dw::DW_AT_type).ok()?? {
            AttributeValue::UnitRef(o) => Some(o),
            _ => None,
        }
    }

    /// A C declarator for type `t` around `inner` (a name, or empty).
    fn decl(&self, t: Option<gimli::UnitOffset>, inner: &str, depth: u32) -> String {
        let join = |base: String| {
            if inner.is_empty() {
                base
            } else {
                format!("{base} {inner}")
            }
        };
        let Some(t) = t else {
            return join("void".into());
        };
        let Some(e) = self.entry(t) else {
            return join("?".into());
        };
        if depth > 16 {
            return join("?".into());
        }
        let next = self.type_ref(&e);
        match e.tag() {
            dw::DW_TAG_pointer_type => self.decl(next, &format!("*{inner}"), depth + 1),
            dw::DW_TAG_const_type => {
                let i = if inner.is_empty() {
                    "const".to_string()
                } else {
                    format!("const {inner}")
                };
                // `const T` reads better before a base type.
                match next.and_then(|n| self.entry(n)).map(|n| n.tag()) {
                    Some(dw::DW_TAG_pointer_type) => self.decl(next, &i, depth + 1),
                    _ => format!("const {}", self.decl(next, inner, depth + 1)),
                }
            }
            dw::DW_TAG_volatile_type => match next.and_then(|n| self.entry(n)).map(|n| n.tag()) {
                Some(dw::DW_TAG_pointer_type) => {
                    let i = if inner.is_empty() {
                        "volatile".to_string()
                    } else {
                        format!("volatile {inner}")
                    };
                    self.decl(next, &i, depth + 1)
                }
                _ => format!("volatile {}", self.decl(next, inner, depth + 1)),
            },
            dw::DW_TAG_array_type => {
                let mut dims = String::new();
                if let Ok(mut kids) = self.unit.entries_tree(Some(t))
                    && let Ok(root) = kids.root()
                {
                    let mut ch = root.children();
                    while let Ok(Some(c)) = ch.next() {
                        let c = c.entry();
                        if c.tag() == dw::DW_TAG_subrange_type {
                            match c
                                .attr_value(dw::DW_AT_upper_bound)
                                .ok()
                                .flatten()
                                .and_then(|v| v.udata_value())
                            {
                                Some(ub) if ub != u64::from(u32::MAX) => {
                                    dims += &format!("[{}]", ub + 1)
                                }
                                _ => dims += "[]",
                            }
                        }
                    }
                }
                let i = if inner.starts_with('*') {
                    format!("({inner})")
                } else {
                    inner.to_string()
                };
                self.decl(next, &format!("{i}{dims}"), depth + 1)
            }
            dw::DW_TAG_subroutine_type => {
                let params = self.params(t, depth);
                let i = if inner.starts_with('*') {
                    format!("({inner})")
                } else {
                    inner.to_string()
                };
                self.decl(next, &format!("{i}({params})"), depth + 1)
            }
            _ => join(self.base_name(t, &e)),
        }
    }

    /// The name a struct, union, enum, typedef or base type goes by.
    fn base_name(
        &self,
        t: gimli::UnitOffset,
        e: &gimli::DebuggingInformationEntry<'_, '_, R<'a>>,
    ) -> String {
        if let Some(n) = self.names.get(&t) {
            return n.clone();
        }
        let kw = match e.tag() {
            dw::DW_TAG_structure_type => "struct ",
            dw::DW_TAG_union_type => "union ",
            dw::DW_TAG_enumeration_type => "enum ",
            _ => "",
        };
        match self.name_of(e) {
            Some(n) => format!("{kw}{n}"),
            None => format!("{kw}<anon>"),
        }
    }

    /// A subprogram's or subroutine type's parameter list.
    fn params(&self, at: gimli::UnitOffset, depth: u32) -> String {
        let mut ps = Vec::new();
        let mut dots = false;
        if let Ok(mut tree) = self.unit.entries_tree(Some(at))
            && let Ok(root) = tree.root()
        {
            let mut ch = root.children();
            while let Ok(Some(c)) = ch.next() {
                let c = c.entry();
                match c.tag() {
                    dw::DW_TAG_formal_parameter => {
                        let n = self.name_of(c).unwrap_or_default();
                        ps.push(self.decl(self.type_ref(c), &n, depth + 1));
                    }
                    dw::DW_TAG_unspecified_parameters => dots = true,
                    _ => {}
                }
            }
        }
        if dots {
            ps.push("...".into());
        }
        if ps.is_empty() {
            "void".into()
        } else {
            ps.join(", ")
        }
    }
}

fn udata(e: &gimli::DebuggingInformationEntry<'_, '_, R<'_>>, at: gimli::DwAt) -> Option<u64> {
    e.attr_value(at).ok()??.udata_value()
}

fn addr(e: &gimli::DebuggingInformationEntry<'_, '_, R<'_>>, at: gimli::DwAt) -> Option<u32> {
    match e.attr_value(at).ok()?? {
        AttributeValue::Addr(a) => Some(a as u32),
        v => v.udata_value().map(|v| v as u32),
    }
}

/// A location expression's home: `DW_OP_reg<n>`, or `DW_OP_breg13`/
/// `DW_OP_breg7`/`DW_OP_fbreg` plus an offset (a stack slot).
fn home(expr: &[u8]) -> Option<Home> {
    let (&op, rest) = expr.split_first()?;
    let sleb = |b: &[u8]| {
        let mut r = EndianSlice::new(b, LittleEndian);
        r.read_sleb128().ok()
    };
    match op {
        0x50..=0x5f if rest.is_empty() => Some(Home::Reg(op - 0x50)),
        // breg13 (sp), breg7 (agbcc's Thumb frame pointer, sp after the
        // prologue), fbreg.
        0x7d | 0x77 | 0x91 => Some(Home::Slot(sleb(rest)? as i32)),
        _ => None,
    }
}

/// A variable's fixed address (`DW_OP_addr`).
fn static_addr(e: &gimli::DebuggingInformationEntry<'_, '_, R<'_>>) -> Option<u32> {
    let b = match e.attr_value(dw::DW_AT_location).ok()?? {
        AttributeValue::Exprloc(x) => x.0.slice(),
        AttributeValue::Block(b) => b.slice(),
        _ => return None,
    };
    (b.len() == 5 && b[0] == 0x03).then(|| u32::from_le_bytes(b[1..5].try_into().unwrap()))
}

/// An address as an `[from, to)` range of a DIE (low_pc/high_pc).
fn range(e: &gimli::DebuggingInformationEntry<'_, '_, R<'_>>) -> Option<(u32, u32)> {
    let lo = addr(e, dw::DW_AT_low_pc)?;
    let hi = match e.attr_value(dw::DW_AT_high_pc).ok()?? {
        AttributeValue::Addr(a) => a as u32,
        v => lo + v.udata_value()? as u32,
    };
    Some((lo, hi))
}

fn read_unit<'a>(
    dwarf: &'a gimli::Dwarf<R<'a>>,
    unit: &Unit<'a>,
    d: &mut Debug,
) -> Result<(), gimli::Error> {
    let mut cx = Cx {
        dwarf,
        unit,
        names: HashMap::new(),
    };
    // Typedefs of anonymous structs, unions and enums name them.
    let mut es = unit.entries();
    while let Some((_, e)) = es.next_dfs()? {
        if e.tag() == dw::DW_TAG_typedef
            && let (Some(n), Some(t)) = (cx.name_of(e), cx.type_ref(e))
            && let Some(te) = cx.entry(t)
        {
            let agg = matches!(
                te.tag(),
                dw::DW_TAG_structure_type | dw::DW_TAG_union_type | dw::DW_TAG_enumeration_type
            );
            if agg && cx.name_of(&te).is_none() {
                cx.names.entry(t).or_insert(n);
            }
        }
    }
    let src = {
        let mut es = unit.entries();
        es.next_dfs()?
            .and_then(|(_, e)| cx.name_of(e))
            .unwrap_or_default()
    };
    // Lines.
    if let Some(lp) = unit.line_program.clone() {
        let mut rows = lp.rows();
        let mut last = 0u64;
        loop {
            let (h, row) = match rows.next_row() {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => {
                    d.bad_lines.push(format!("{src}: {e} after 0x{last:x}"));
                    break;
                }
            };
            let Some(line) = row.line() else { continue };
            let file = row
                .file(h)
                .and_then(|f| dwarf.attr_string(unit, f.path_name()).ok())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            last = row.address();
            if !row.end_sequence() {
                d.lines
                    .insert(row.address() as u32, (file, line.get() as u32));
            }
        }
    }
    // Functions, globals and types.
    let mut tree = unit.entries_tree(None)?;
    let root = tree.root()?;
    let mut ch = root.children();
    while let Some(node) = ch.next()? {
        let e = node.entry().clone();
        match e.tag() {
            dw::DW_TAG_subprogram => {
                let Some((lo, hi)) = range(&e) else { continue };
                let name = cx.name_of(&e).unwrap_or_default();
                let params = cx.params(e.offset(), 0);
                let proto = cx.decl(cx.type_ref(&e), &format!("{name}({params})"), 0);
                let mut f = Func {
                    name,
                    proto,
                    src: src.clone(),
                    end: hi,
                    ..Func::default()
                };
                locals(&cx, node, (lo, hi), &mut f)?;
                for (a, n, ty) in &f.statics {
                    d.globals.entry(*a).or_insert((n.clone(), ty.clone()));
                }
                d.funcs.entry(lo & !1).or_insert(f);
            }
            dw::DW_TAG_variable => {
                if let Some(a) = static_addr(&e) {
                    let n = cx.name_of(&e).unwrap_or_default();
                    let ty = cx.decl(cx.type_ref(&e), "", 0);
                    d.globals.entry(a).or_insert((n, ty));
                }
            }
            _ => {}
        }
    }
    // Every struct, union and enum anywhere in the unit.
    let mut es = unit.entries();
    while let Some((_, e)) = es.next_dfs()? {
        let tag: DwTag = e.tag();
        if !matches!(
            tag,
            dw::DW_TAG_structure_type | dw::DW_TAG_union_type | dw::DW_TAG_enumeration_type
        ) || e.attr_value(dw::DW_AT_declaration)?.is_some()
        {
            continue;
        }
        let name = cx.base_name(e.offset(), e);
        if name.ends_with("<anon>") {
            continue;
        }
        let Some(layout) = layout(&cx, e.offset())? else {
            continue;
        };
        match d.types.get(&name) {
            Some(l) if *l != layout => d.clashes.push(name),
            Some(_) => {}
            None => {
                d.types.insert(name, layout);
            }
        }
    }
    Ok(())
}

fn layout(cx: &Cx<'_, '_>, at: gimli::UnitOffset) -> Result<Option<Layout>, gimli::Error> {
    let mut tree = cx.unit.entries_tree(Some(at))?;
    let root = tree.root()?;
    let e = root.entry().clone();
    let size = udata(&e, dw::DW_AT_byte_size).unwrap_or(0) as u32;
    let mut fields = Vec::new();
    let mut values = Vec::new();
    let mut ch = root.children();
    while let Some(c) = ch.next()? {
        let c = c.entry();
        match c.tag() {
            dw::DW_TAG_member => {
                let name = cx.name_of(c).unwrap_or_default();
                let ty = cx.decl(cx.type_ref(c), "", 0);
                let off = match c.attr_value(dw::DW_AT_data_member_location)? {
                    Some(AttributeValue::Exprloc(x)) => {
                        // DW_OP_plus_uconst n.
                        let b = x.0.slice();
                        if b.first() == Some(&0x23) {
                            let mut r = EndianSlice::new(&b[1..], LittleEndian);
                            r.read_uleb128()? as u32
                        } else {
                            0
                        }
                    }
                    Some(AttributeValue::Block(b)) if b.slice().first() == Some(&0x23) => {
                        let mut r = EndianSlice::new(&b.slice()[1..], LittleEndian);
                        r.read_uleb128()? as u32
                    }
                    Some(v) => v.udata_value().unwrap_or(0) as u32,
                    None => 0,
                };
                let tsize = cx
                    .type_ref(c)
                    .and_then(|t| type_size(cx, t, 0))
                    .unwrap_or(0);
                let fsize = udata(c, dw::DW_AT_byte_size).map_or(tsize, |s| s as u32);
                // DWARF 2 counts a bitfield's bit_offset from the unit's
                // most significant bit.
                let bits = match (udata(c, dw::DW_AT_bit_size), udata(c, dw::DW_AT_bit_offset)) {
                    (Some(w), Some(bo)) => {
                        Some(((fsize * 8).saturating_sub(bo as u32 + w as u32), w as u32))
                    }
                    _ => None,
                };
                fields.push(Field {
                    name,
                    off,
                    size: fsize,
                    ty,
                    bits,
                });
            }
            dw::DW_TAG_enumerator => {
                let name = cx.name_of(c).unwrap_or_default();
                let v = match c.attr_value(dw::DW_AT_const_value)? {
                    Some(AttributeValue::Sdata(v)) => v,
                    Some(AttributeValue::Data1(v)) => i64::from(v as i8),
                    Some(AttributeValue::Data2(v)) => i64::from(v as i16),
                    Some(AttributeValue::Data4(v)) => i64::from(v as i32),
                    Some(v) => v.udata_value().unwrap_or(0) as i64,
                    None => 0,
                };
                values.push((name, v));
            }
            _ => {}
        }
    }
    Ok(Some(match e.tag() {
        dw::DW_TAG_structure_type => Layout::Struct { size, fields },
        dw::DW_TAG_union_type => Layout::Union { size, fields },
        _ => Layout::Enum { size, values },
    }))
}

/// A type's size in bytes, following typedefs, qualifiers and arrays.
fn type_size(cx: &Cx<'_, '_>, t: gimli::UnitOffset, depth: u32) -> Option<u32> {
    let e = cx.entry(t)?;
    if let Some(s) = udata(&e, dw::DW_AT_byte_size) {
        return Some(s as u32);
    }
    if depth > 16 {
        return None;
    }
    match e.tag() {
        dw::DW_TAG_pointer_type => Some(4),
        dw::DW_TAG_array_type => {
            let mut n = 1u32;
            let mut tree = cx.unit.entries_tree(Some(t)).ok()?;
            let root = tree.root().ok()?;
            let mut ch = root.children();
            while let Ok(Some(c)) = ch.next() {
                if c.entry().tag() == dw::DW_TAG_subrange_type {
                    let ub = udata(c.entry(), dw::DW_AT_upper_bound)?;
                    n = n.checked_mul(ub as u32 + 1)?;
                }
            }
            Some(n * type_size(cx, cx.type_ref(&e)?, depth + 1)?)
        }
        _ => type_size(cx, cx.type_ref(&e)?, depth + 1),
    }
}

/// A function's parameters and variables, with lexical blocks' ranges.
fn locals<'a>(
    cx: &Cx<'a, '_>,
    node: gimli::EntriesTreeNode<'_, '_, '_, R<'a>>,
    span: (u32, u32),
    f: &mut Func,
) -> Result<(), gimli::Error> {
    let mut ch = node.children();
    while let Some(c) = ch.next()? {
        let e = c.entry().clone();
        match e.tag() {
            dw::DW_TAG_formal_parameter | dw::DW_TAG_variable => {
                let name = cx.name_of(&e).unwrap_or_default();
                let ty = cx.decl(cx.type_ref(&e), "", 0);
                let at = match e.attr_value(dw::DW_AT_location)? {
                    Some(AttributeValue::Exprloc(x)) => home(x.0.slice()),
                    Some(AttributeValue::Block(b)) => home(b.slice()),
                    _ => None,
                };
                match at {
                    Some(home) => f.locals.push(Local {
                        name,
                        home,
                        from: span.0,
                        to: span.1,
                        ty,
                        param: e.tag() == dw::DW_TAG_formal_parameter,
                    }),
                    None => match static_addr(&e) {
                        // A function's static: a global.
                        Some(a) => f.statics.push((a, name, ty)),
                        None => f.homeless.push(name),
                    },
                }
            }
            dw::DW_TAG_lexical_block => {
                let inner = range(&e).unwrap_or(span);
                locals(cx, c, inner, f)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn quoted(s: &str) -> String {
    crate::quoted(s)
}

impl Debug {
    /// The function whose code holds `a` (bit 0 ignored).
    pub fn func_at(&self, a: u32) -> Option<(u32, &Func)> {
        let a = a & !1;
        let (&at, f) = self.funcs.range(..=a).next_back()?;
        (a < f.end.max(at + 1)).then_some((at, f))
    }

    /// `, src = "…", proto = "…"` for the function at `rom_addr`, if
    /// DWARF has one there.
    pub fn func_fields(&self, rom_addr: u32) -> String {
        match self.funcs.get(&(rom_addr & !1)) {
            Some(f) => format!(", src = {}, proto = {}", quoted(&f.src), quoted(&f.proto)),
            None => String::new(),
        }
    }

    /// `, type = "…"` for the global at `a`.
    pub fn data_fields(&self, a: u32) -> String {
        match self.globals.get(&a) {
            Some((_, ty)) => format!(", type = {}", quoted(ty)),
            None => String::new(),
        }
    }

    /// `[locals]` for the functions at `keys` (codegen's keys of ROM code),
    /// `[lines]` for all of the ROM, and `[types]`.
    pub fn tables(&self, keys: &[u32]) -> String {
        let mut o = String::new();
        o.push_str(
            "\n# Each function's parameters and variables from DWARF: a register (`reg`)\n\
             # or a stack slot (`slot`, an offset from sp after the prologue), over\n\
             # [from, to). Variables agbcc gave no location are left out.\n[locals]\n",
        );
        for &k in keys {
            let Some(f) = self.funcs.get(&(k & !1)) else {
                continue;
            };
            if f.locals.is_empty() {
                continue;
            }
            writeln!(o, "0x{k:08x} = [").unwrap();
            for l in &f.locals {
                let home = match l.home {
                    Home::Reg(r) => format!("reg = {r}"),
                    Home::Slot(s) => format!("slot = {s}"),
                };
                let param = if l.param { ", param = true" } else { "" };
                writeln!(
                    o,
                    "  {{ name = {}, {home}, from = 0x{:08x}, to = 0x{:08x}, type = {}{param} }},",
                    quoted(&l.name),
                    l.from,
                    l.to,
                    quoted(&l.ty)
                )
                .unwrap();
            }
            o.push_str("]\n");
        }
        o.push_str(
            "\n# The line table: each row's address and its line, in the `src` of the\n\
             # function holding it, else \"file:line\".\n[lines]\n",
        );
        for (&a, (file, line)) in &self.lines {
            match self.func_at(a) {
                Some((_, f)) if f.src == *file => writeln!(o, "0x{a:08x} = {line}"),
                _ => writeln!(o, "0x{a:08x} = {}", quoted(&format!("{file}:{line}"))),
            }
            .unwrap();
        }
        o.push_str(
            "\n# Structs, unions and enums by C name (a typedef's for an anonymous one):\n\
             # fields with byte offset, size, type and bitfield bits [lowest, width].\n",
        );
        for (name, l) in &self.types {
            writeln!(o, "\n[types.{}]", quoted(name)).unwrap();
            let (kind, size) = match l {
                Layout::Struct { size, .. } => ("struct", size),
                Layout::Union { size, .. } => ("union", size),
                Layout::Enum { size, .. } => ("enum", size),
            };
            writeln!(o, "kind = {}\nsize = {size}", quoted(kind)).unwrap();
            match l {
                Layout::Struct { fields, .. } | Layout::Union { fields, .. } => {
                    o.push_str("fields = [\n");
                    for f in fields {
                        let bits = f
                            .bits
                            .map_or(String::new(), |(lo, w)| format!(", bits = [{lo}, {w}]"));
                        writeln!(
                            o,
                            "  {{ name = {}, off = {}, size = {}, type = {}{bits} }},",
                            quoted(&f.name),
                            f.off,
                            f.size,
                            quoted(&f.ty)
                        )
                        .unwrap();
                    }
                    o.push_str("]\n");
                }
                Layout::Enum { values, .. } => {
                    o.push_str("values = [\n");
                    for (n, v) in values {
                        writeln!(o, "  {{ name = {}, value = {v} }},", quoted(n)).unwrap();
                    }
                    o.push_str("]\n");
                }
            }
        }
        o
    }
}
