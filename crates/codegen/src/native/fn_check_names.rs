//! The RAM names the per-function check's report gives a differing byte
//! (unit `native/lift-mod-run-edit-report`): each named datum in RAM, with
//! its type as the names files give it, written into a checked lift as
//! `FN_CHECK_RAM`, a table of `fn_check::Ty`s the driver
//! (`native/fn_check.rs`, `ram_name`) walks down to the field holding a
//! byte: `IWRAM 0x03001530 gEquipment.maxEnergy: 99 lift, 45 cut`.
//!
//! Only the types the RAM data reach are written, each struct once (a
//! static of its fields), so the table is the size of the names files,
//! not of RAM.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use super::globals;
use crate::types::{Ty, Types};

/// The table's Rust text: `FN_CHECK_RAM` and the statics it refers to.
/// `data`: each datum's identifier and size (0: not known) by guest
/// address; `types`: the names files' types, if any.
pub fn table(data: &BTreeMap<u32, (String, u32)>, types: Option<&Types>) -> String {
    // Every RAM datum: by name, typed where the types say.
    let mut ram: BTreeMap<u32, (String, Option<&Ty>, u32)> = BTreeMap::new();
    for (&a, (n, size)) in data {
        if globals::ram_at(a).is_some() {
            ram.insert(a, (n.clone(), None, (*size).max(1)));
        }
    }
    if let Some(t) = types {
        for (a, ty) in t.globals() {
            if globals::ram_at(a).is_none() {
                continue;
            }
            let size = t.size(ty).max(1);
            ram.entry(a)
                .and_modify(|e| {
                    e.1 = Some(ty);
                    e.2 = size;
                })
                .or_insert_with(|| (format!("d_{a:08x}"), Some(ty), size));
        }
    }
    let mut w = Writer {
        types,
        structs: BTreeMap::new(),
        arrays: BTreeMap::new(),
        out: String::new(),
        busy: BTreeSet::new(),
    };
    let mut rows = String::new();
    for (a, (n, ty, size)) in &ram {
        let t = match ty {
            Some(ty) => w.ty(ty),
            None => format!("fn_check::Ty::Bytes({size})"),
        };
        writeln!(rows, "    ({a:#010x}, {n:?}, &{t}),").unwrap();
    }
    let mut s = String::from(
        "\n/// Each named datum in RAM, by guest address, with its type as the names files\n/// give it: what the per-function check's report names a byte by.\npub static FN_CHECK_RAM: &[(u32, &str, &fn_check::Ty)] = &[\n",
    );
    s.push_str(&rows);
    s.push_str("];\n");
    s.push_str(&w.out);
    s
}

struct Writer<'a> {
    types: Option<&'a Types>,
    /// Each struct written, its static's number.
    structs: BTreeMap<String, usize>,
    /// Each array type written, its static's number, by its text.
    arrays: BTreeMap<String, usize>,
    out: String,
    /// Structs being written (a struct in itself, which C can't, is bytes).
    busy: BTreeSet<String>,
}

impl Writer<'_> {
    /// `ty` as a `fn_check::Ty` expression.
    fn ty(&mut self, ty: &Ty) -> String {
        let Some(t) = self.types else {
            return "fn_check::Ty::Bytes(1)".into();
        };
        match ty {
            Ty::U8 | Ty::U16 | Ty::U32 | Ty::Ptr(_) => {
                format!("fn_check::Ty::Scalar({})", t.size(ty))
            }
            Ty::Bytes(n) => format!("fn_check::Ty::Bytes({n})"),
            Ty::Array(e, n) => {
                // A static of its own: a reference to a static isn't
                // promoted inside an enum's constructor.
                let es = t.size(e);
                let e = self.ty(e);
                let text = format!("fn_check::Ty::Array(&{e}, {n}, {es})");
                let i = match self.arrays.get(&text) {
                    Some(&i) => i,
                    None => {
                        let i = self.arrays.len();
                        writeln!(self.out, "static FCA_{i}: fn_check::Ty = {text};").unwrap();
                        self.arrays.insert(text, i);
                        i
                    }
                };
                format!("FCA_{i}")
            }
            Ty::Named(name) => {
                if let Some(i) = self.structs.get(name) {
                    return format!("FCT_{i}");
                }
                let Some((size, fields)) = t.fields_of(name) else {
                    return format!("fn_check::Ty::Bytes({})", t.size(ty).max(1));
                };
                if !self.busy.insert(name.clone()) {
                    return format!("fn_check::Ty::Bytes({})", size.max(1));
                }
                // A plain field first, so a byte a bitfield shares with
                // one is named by the plain one.
                let mut fs: Vec<_> = fields.into_iter().collect();
                fs.sort_by_key(|f| f.3);
                let mut items = Vec::new();
                for (ident, off, fty, _) in fs {
                    let e = self.ty(fty);
                    items.push(format!(
                        "fn_check::Field {{ name: {ident:?}, off: {off}, ty: &{e} }}"
                    ));
                }
                self.busy.remove(name);
                let i = self.structs.len();
                self.structs.insert(name.clone(), i);
                writeln!(
                    self.out,
                    "static FCF_{i}: [fn_check::Field; {}] = [{}];\nstatic FCT_{i}: fn_check::Ty = fn_check::Ty::Struct(&FCF_{i}, {size});",
                    items.len(),
                    items.join(", ")
                )
                .unwrap();
                format!("FCT_{i}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::SourceMap;

    const SHA: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn ram_data_with_their_types_each_struct_once() {
        let text = format!(
            r#"[map]
rom_sha1 = "{SHA}"
[data]
0x03001530 = {{ name = "gEquipment", size = 4, type = "struct Equipment" }}
0x03001600 = {{ name = "gPair", size = 8, type = "struct Equipment [2]" }}
0x08001000 = {{ name = "sStart", size = 4, type = "struct Equipment" }}
[types."struct Equipment"]
kind = "struct"
size = 4
fields = [
  {{ name = "flags", off = 0, size = 2, type = "u16", bits = [0, 4] }},
  {{ name = "maxEnergy", off = 0, size = 2, type = "u16" }},
  {{ name = "energy", off = 2, size = 2, type = "u16" }},
]
"#
        );
        let m = SourceMap::from_texts(SHA, &[&text]).unwrap();
        let data = BTreeMap::from([
            (0x0300_1530, ("gEquipment".to_string(), 4)),
            (0x0300_1600, ("gPair".to_string(), 8)),
            (0x0300_1700, ("gUntyped".to_string(), 0)),
            (0x0800_1000, ("sStart".to_string(), 4)),
        ]);
        let t = table(&data, Some(&m.typed));
        // RAM data only, the ROM's left out; an untyped one is bytes.
        assert!(t.contains("(0x03001530, \"gEquipment\", &FCT_0),"), "{t}");
        assert!(t.contains("(0x03001600, \"gPair\", &FCA_0),"), "{t}");
        assert!(
            t.contains("(0x03001700, \"gUntyped\", &fn_check::Ty::Bytes(1)),"),
            "{t}"
        );
        assert!(!t.contains("sStart"), "{t}");
        // The struct once, its plain field before the bitfield on its bytes.
        assert_eq!(t.matches("static FCT_").count(), 1, "{t}");
        let fields = t.lines().find(|l| l.starts_with("static FCF_0")).unwrap();
        let (plain, bits) = (
            fields.find("maxEnergy").unwrap(),
            fields.find("flags").unwrap(),
        );
        assert!(plain < bits, "{fields}");
        assert!(
            t.contains("static FCA_0: fn_check::Ty = fn_check::Ty::Array(&FCT_0, 2, 4);"),
            "{t}"
        );
    }
}
