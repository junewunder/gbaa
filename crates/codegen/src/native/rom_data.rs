//! ROM data read at computed addresses as typed statics (unit
//! `native/rom-data-statics`; docs/native-stage-b.md, piece 5;
//! docs/native-port-design.md, section 8, "ROM data").
//!
//! A load whose address isn't a constant in its block but which the value
//! sets bound to ROM (`analysis::vsa::func::Access`) reads an object: a
//! table of halfwords, a string's bytes, a record's words. The lifter
//! carves those objects from the extents the loads reach, one per width
//! the code reads at (two loads of one width whose extents overlap are one
//! object), and prints each as a `static` view of the ROM's bytes typed by
//! that width (two loads of one width whose extents overlap are one
//! object): `pub static ROM_0808CAD0: &[u16; 12] = rom_at(0xcad0);`. A
//! load from it indexes the static, the pointer checked
//! (`ROM_0808CAD0[rix(a, 0x0808cad0, 2, 12)] as u32`), so a value set that
//! was wrong stops the program naming the pointer instead of reading
//! other bytes.
//!
//! The views are the same bytes as `ROM` (the prelude's `rom_at`, a
//! reference into the ROM included once, aligned to a word), so two
//! objects of different widths over the same bytes cost nothing. A word
//! loaded from a static that is a pointer stays a guest address (a
//! function's key stays a ROM key, `native/rom-statics`' to change; a data
//! pointer is a `RomRef` in the design, a number here).
//!
//! Left as bytes (`ld*`): extents in the EEPROM's window (`0D`), across a
//! wait-state window or past the ROM's end (the open bus).

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One static: `n` elements of `elem` bytes from guest address `base`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RomObj {
    pub name: String,
    pub base: u32,
    pub elem: u32,
    pub n: u32,
}

impl RomObj {
    /// A load through the pointer `a` (a `u32` expression, aligned as the
    /// access aligns it), as a `u32` expression.
    pub fn load(&self, a: &str) -> String {
        let e = format!(
            "{}[rix({a}, {:#x}, {}, {})]",
            self.name, self.base, self.elem, self.n
        );
        if self.elem == 4 {
            e
        } else {
            format!("{e} as u32")
        }
    }

    fn ty(&self) -> String {
        format!("[u{}; {}]", self.elem * 8, self.n)
    }
}

/// The ROM's objects, by (width, base).
#[derive(Clone, Debug, Default)]
pub struct RomData {
    objs: BTreeMap<(u32, u32), RomObj>,
    rom_len: u32,
}

/// The guest address `a`'s offset in the ROM, if it is in one wait-state
/// window's ROM (not the EEPROM's `0D`).
fn offset(a: u32) -> Option<u32> {
    match a >> 24 {
        0x08..=0x0C => Some(a & 0x01FF_FFFF),
        _ => None,
    }
}

/// The widest extent a static holds: a load whose value set spans more
/// (the "from here on" sets of a pointer the analysis lost) reads ROM
/// through a pointer, not an object, and stays bytes, as the function flow
/// reads nothing through one (`native/fnref-rom-read-bound`).
pub const MAX_EXTENT: u32 = 256 << 10;

/// `lo..end` (guest addresses) as ROM data a static can hold, if it can:
/// at most [`MAX_EXTENT`] bytes, in one window, not the EEPROM's, starting
/// inside the ROM's `rom_len` bytes. An extent that runs past the ROM's end (a value set wider than
/// the table: an index by a byte into a short one) ends there: what reads
/// past it stops the program at [`RomObj::load`]'s check, where the guest
/// would read the open bus.
fn holdable(lo: u32, end: u32, rom_len: u32) -> Option<(u32, u32)> {
    let o = offset(lo)?;
    if end <= lo
        || end - lo > MAX_EXTENT
        || lo >> 25 != (end - 1) >> 25
        || (end - 1) >> 24 == 0x0D
        || o >= rom_len
    {
        return None;
    }
    Some((lo, end.min(lo + (rom_len - o))))
}

impl RomData {
    /// Carve the objects from `ranges` (lowest address, end, width: a
    /// load's extent, aligned as it aligns its address) in a ROM of
    /// `rom_len` bytes.
    pub fn carve(ranges: &[(u32, u32, u32)], rom_len: u32) -> RomData {
        let mut xs: Vec<(u32, u32, u32)> = ranges
            .iter()
            .copied()
            .filter(|&(lo, _, w)| lo % w == 0)
            .filter_map(|(lo, end, w)| {
                let (lo, end) = holdable(lo, end, rom_len)?;
                // At least one whole element.
                (end - lo >= w).then_some((w, lo, end))
            })
            .collect();
        xs.sort_unstable();
        xs.dedup();
        let mut objs: Vec<(u32, u32, u32)> = Vec::new();
        for (w, lo, end) in xs {
            match objs.last_mut() {
                // An extent that overlaps the last one of its width joins
                // it (one window, as both are); one that only touches it
                // is an object of its own.
                Some((lw, _, lend)) if *lw == w && lo < *lend => {
                    *lend = (*lend).max(end);
                }
                _ => objs.push((w, lo, end)),
            }
        }
        let mut bases: BTreeMap<u32, usize> = BTreeMap::new();
        for &(_, lo, _) in &objs {
            *bases.entry(lo).or_default() += 1;
        }
        let mut out = BTreeMap::new();
        for (w, lo, end) in objs {
            let end = lo + (end - lo) / w * w;
            let name = if bases[&lo] > 1 {
                format!("ROM_{lo:08X}_{}", w * 8)
            } else {
                format!("ROM_{lo:08X}")
            };
            out.insert(
                (w, lo),
                RomObj {
                    name,
                    base: lo,
                    elem: w,
                    n: (end - lo) / w,
                },
            );
        }
        RomData { objs: out, rom_len }
    }

    /// The object a load of `w` bytes whose addresses are `lo..=hi`
    /// (aligned as it aligns them) reads, if one holds it.
    pub fn object(&self, lo: u32, hi: u32, w: u32) -> Option<&RomObj> {
        let (lo, end) = holdable(lo, hi + w, self.rom_len)?;
        let (_, o) = self.objs.range(..=(w, lo)).next_back()?;
        (o.elem == w && o.base <= lo && end <= o.base + o.n * o.elem).then_some(o)
    }

    /// An object that holds any of the bytes `lo..end` (guest addresses).
    pub fn overlapping(&self, lo: u32, end: u32) -> Option<&RomObj> {
        self.objs
            .values()
            .find(|o| o.base < end && lo < o.base + o.n * o.elem)
    }

    pub fn len(&self) -> usize {
        self.objs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objs.is_empty()
    }

    /// The statics' declarations, one line each (the datum's name, from
    /// the names files, in a comment).
    pub fn decls(&self, names: &BTreeMap<u32, (String, u32)>) -> String {
        let mut s = String::new();
        for o in self.objs.values() {
            write!(
                s,
                "pub static {}: &{} = rom_at({:#x});",
                o.name,
                o.ty(),
                offset(o.base).expect("carved in ROM")
            )
            .unwrap();
            match names.get(&o.base) {
                Some((n, _)) => writeln!(s, " // {n}").unwrap(),
                None => s.push('\n'),
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carves_by_width_and_joins_what_overlaps() {
        let names = BTreeMap::from([(0x0800_1000, ("sTable".to_string(), 8))]);
        let r = RomData::carve(
            &[
                (0x0800_1000, 0x0800_1008, 2),
                (0x0800_1004, 0x0800_1010, 2),
                (0x0800_1000, 0x0800_1004, 1),
                (0x0800_2001, 0x0800_2006, 2), // misaligned: left
                (0x0D00_0000, 0x0D00_0010, 2), // the EEPROM's window
                (0x0800_3000, 0x0800_3010, 4),
                (0x0900_0000, 0x0900_0010, 4), // past the ROM
                (0x080F_FFF8, 0x0810_0010, 4), // ends at the ROM's end
                (0x0800_3010, 0x0800_3014, 4), // touches: its own
                (0x0800_4000, 0x0805_0000, 2), // too wide: left
            ],
            0x10_0000,
        );
        assert_eq!(r.len(), 5);
        assert!(r.object(0x0800_4000, 0x0800_4000, 2).is_none());
        assert_eq!(r.object(0x0800_3010, 0x0800_3010, 4).unwrap().n, 1);
        assert_eq!(r.object(0x080F_FFF8, 0x080F_FFFC, 4).unwrap().n, 2);
        let o = r.object(0x0800_1002, 0x0800_100E, 2).unwrap();
        assert_eq!((o.name.as_str(), o.n), ("ROM_08001000_16", 8));
        assert_eq!(
            o.load("a"),
            "ROM_08001000_16[rix(a, 0x8001000, 2, 8)] as u32"
        );
        assert!(r.object(0x0800_1002, 0x0800_1010, 2).is_none());
        assert!(r.object(0x0800_1000, 0x0800_1000, 4).is_none());
        assert_eq!(
            r.object(0x0800_3000, 0x0800_300C, 4).unwrap().load("a"),
            "ROM_08003000[rix(a, 0x8003000, 4, 4)]"
        );
        let d = r.decls(&names);
        assert!(d.contains("pub static ROM_08001000_16: &[u16; 8] = rom_at(0x1000); // sTable\n"));
        assert!(d.contains("pub static ROM_08001000_8: &[u8; 4] = rom_at(0x1000); // sTable\n"));
    }
}
