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
//! objects of different widths over the same bytes cost nothing.
//!
//! A word loaded from a static that is a function's address is what the
//! crate's copy of the ROM holds: at `--fn-ids dense` a dense function's
//! ID (an `FnRef`, `native/fnref-dense`), else its key; the lift counts
//! the function words the flow read in a static that a dense build leaves
//! as keys (`rom_static_fn_keys`, `native/rom-statics`). A word that is a
//! pointer into ROM data stays a guest address (a `RomRef` in the design,
//! docs/native-port-design.md, section 8): the soft hole `rom_refs`
//! counts the loads from a word static that holds one ([`RomObj::ptrs`]).
//!
//! Left as bytes (`ld*`): extents in the EEPROM's window (`0D`), across a
//! wait-state window, past the ROM's end (the open bus), or wider than
//! [`MAX_EXTENT`] ([`WIDE_EXTENT`] where the flow read no function word
//! in it): what is left is a load through a pointer read from ROM, whose
//! value set spans the objects it may point at.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// One static: `n` elements of `elem` bytes from guest address `base`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RomObj {
    pub name: String,
    pub base: u32,
    pub elem: u32,
    pub n: u32,
    /// A static of words some of which hold an address in ROM data (not
    /// a function's key): a word loaded from it may be a pointer into ROM,
    /// a `RomRef` in the design (docs/native-port-design.md, section 8)
    /// and a guest address here (`native/rom-statics`).
    pub ptrs: bool,
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
    /// The extents wider than [`MAX_EXTENT`] that [`RomData::carve`]
    /// held, as it clipped them.
    wide: BTreeSet<(u32, u32)>,
}

/// The guest address `a`'s offset in the ROM, if it is in one wait-state
/// window's ROM (not the EEPROM's `0D`).
fn offset(a: u32) -> Option<u32> {
    match a >> 24 {
        0x08..=0x0C => Some(a & 0x01FF_FFFF),
        _ => None,
    }
}

/// The widest extent a static holds of any ROM data: as wide as the
/// function flow's one read (`fnref`'s `ROM_READ`), so a static's every
/// function word is one the flow could read and prove. A load whose value
/// set spans more (the "from here on" sets of a pointer the analysis
/// lost) reads ROM through a pointer, not an object, and stays bytes, as
/// the function flow reads nothing past it (`native/fnref-rom-read-bound`).
pub const MAX_EXTENT: u32 = 256 << 10;

/// The widest extent a static holds of data in which the function flow
/// read no function word (`native/rom-statics`): a halfword index into
/// records of 8 bytes (Metroid's song table, read by song number). A
/// dense build changes only words the flow read, so such a static holds
/// the same keys, if any, as the ROM it views, read past the flow's
/// widest read or not.
pub const WIDE_EXTENT: u32 = 512 << 10;

/// `lo..end` (guest addresses) as ROM data a static can hold, if it can:
/// at most `max` bytes, in one window, not the EEPROM's, starting
/// inside the ROM's `rom_len` bytes. An extent that runs past the ROM's end (a value set wider than
/// the table: an index by a byte into a short one) ends there: what reads
/// past it stops the program at [`RomObj::load`]'s check, where the guest
/// would read the open bus.
fn holdable(lo: u32, end: u32, rom_len: u32, max: u32) -> Option<(u32, u32)> {
    let o = offset(lo)?;
    if end <= lo
        || end - lo > max
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
    /// load's extent, aligned as it aligns its address) in `rom`. `is_fn`
    /// says whether a word is a function's key, `read` whether the
    /// function flow read the one at an address: an extent wider than
    /// [`MAX_EXTENT`] is held only if no function word in it was read.
    pub fn carve(
        ranges: &[(u32, u32, u32)],
        rom: &[u8],
        is_fn: impl Fn(u32) -> bool,
        read: impl Fn(u32) -> bool,
    ) -> RomData {
        let rom_len = rom.len() as u32;
        let no_fn_words = |lo: u32, end: u32| {
            let o = offset(lo).expect("holdable") as usize & !3;
            let e = (offset(end - 1).expect("holdable") as usize + 1).min(rom.len());
            rom.get(o..e).is_some_and(|b| {
                !b.as_chunks::<4>().0.iter().enumerate().any(|(i, c)| {
                    is_fn(u32::from_le_bytes(*c)) && read(0x0800_0000 + (o + 4 * i) as u32)
                })
            })
        };
        let mut wide = BTreeSet::new();
        let mut xs: Vec<(u32, u32, u32)> = ranges
            .iter()
            .copied()
            .filter(|&(lo, _, w)| lo % w == 0)
            .filter_map(|(lo, end, w)| {
                let (lo, end) = match holdable(lo, end, rom_len, MAX_EXTENT) {
                    Some(x) => x,
                    None => {
                        let x = holdable(lo, end, rom_len, WIDE_EXTENT)
                            .filter(|&(lo, end)| no_fn_words(lo, end))?;
                        wide.insert(x);
                        x
                    }
                };
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
        let words = |lo: u32, end: u32| {
            let o = offset(lo).expect("carved in ROM") as usize;
            let e = offset(end - 1).expect("carved in ROM") as usize + 1;
            rom[o..e.min(rom.len())]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect::<Vec<u32>>()
        };
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
                    ptrs: w == 4
                        && words(lo, end)
                            .into_iter()
                            .any(|x| offset(x).is_some_and(|o| o < rom_len) && !is_fn(x)),
                },
            );
        }
        RomData {
            objs: out,
            rom_len,
            wide,
        }
    }

    /// The object a load of `w` bytes whose addresses are `lo..=hi`
    /// (aligned as it aligns them) reads, if one holds it.
    pub fn object(&self, lo: u32, hi: u32, w: u32) -> Option<&RomObj> {
        // One wider than [`MAX_EXTENT`] only as [`RomData::carve`] held it.
        let (lo, end) = match holdable(lo, hi + w, self.rom_len, MAX_EXTENT) {
            Some(x) => x,
            None => {
                holdable(lo, hi + w, self.rom_len, WIDE_EXTENT).filter(|x| self.wide.contains(x))?
            }
        };
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
        let mut rom = vec![0u8; 0x10_0000];
        rom[0x4_8000..0x4_8004].copy_from_slice(&0x0800_0101u32.to_le_bytes());
        // A pointer into ROM data in the word static at 0x0800_3000.
        rom[0x3004..0x3008].copy_from_slice(&0x0800_2000u32.to_le_bytes());
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
                (0x0800_4000, 0x0805_0000, 2), // too wide, a function word: left
                (0x0806_0000, 0x080D_0000, 4), // wide, no function word
                (0x0806_0000, 0x080F_0000, 4), // too wide: left
            ],
            &rom,
            |x| x == 0x0800_0101,
            |a| a == 0x0804_8000,
        );
        assert_eq!(r.len(), 6);
        assert!(r.object(0x0800_4000, 0x0800_4000, 2).is_none());
        assert_eq!(r.object(0x0806_0000, 0x080C_FFFC, 4).unwrap().n, 0x1_c000);
        assert!(r.object(0x0806_0000, 0x080E_FFFC, 4).is_none());
        assert!(r.object(0x0800_3000, 0x0800_3000, 4).unwrap().ptrs);
        assert!(!r.object(0x0806_0000, 0x0806_0000, 4).unwrap().ptrs);
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
