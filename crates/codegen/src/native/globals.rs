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
//! The globals are the fields of `Ram`, a `#[repr(C)]` struct at the
//! guest's offsets (`--layout guest`, the only layout yet) with byte
//! padding between, so `Ram` is still the guest's EWRAM and IWRAM byte
//! for byte: the prelude derefs it to `[u8; EWRAM + IWRAM]`, which every
//! other access (computed addresses, the stack, the hardware library, the
//! native check's comparison) uses as before. A field's name is the names
//! file's for the datum at its start, else `d_<guest address>`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

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
fn guest(off: u32) -> u32 {
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
    pub elem: u32,
    pub name: String,
}

impl Global {
    fn ty(&self) -> String {
        let t = format!("u{}", self.elem * 8);
        if self.len == self.elem {
            t
        } else {
            format!("[{t}; {}]", self.len / self.elem)
        }
    }
}

/// The globals, by their start offset.
#[derive(Clone, Debug, Default)]
pub struct Globals(pub BTreeMap<u32, Global>);

/// How one access reaches its global: the place (`g.ram.d_…` or
/// `g.ram.d_…[i]`), its element's width, and the access's bit shift in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub place: String,
    pub elem: u32,
    pub shift: u32,
}

impl Globals {
    /// Carve the globals out of `accesses` (offset, width in bytes).
    /// `names` (guest address to identifier and size in bytes, 0 if not
    /// known) names a global that starts at a named datum, and a datum of
    /// known size that an access reaches is one global over all of it.
    pub fn carve(accesses: &[(u32, u32)], names: &BTreeMap<u32, (String, u32)>) -> Globals {
        // (offset, end, width): an access, or a named datum's extent (width 0).
        let mut xs: Vec<(u32, u32, u32)> = accesses
            .iter()
            .filter(|&&(o, w)| o + w <= SIZE)
            .map(|&(o, w)| (o, o + w, w))
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
                },
            );
            i = j;
        }
        Globals(out)
    }

    /// Where the access of `width` bytes at offset `off` is, if it is in
    /// a global that can hold it as typed (none for an access wider than
    /// its global's element).
    pub fn place(&self, off: u32, width: u32) -> Option<Place> {
        let (_, g) = self.0.range(..=off).next_back()?;
        if off + width > g.lo + g.len || width > g.elem {
            return None;
        }
        let i = (off - g.lo) / g.elem;
        let place = if g.len == g.elem {
            format!("g.ram.{}", g.name)
        } else {
            format!("g.ram.{}[{i}]", g.name)
        };
        Some(Place {
            place,
            elem: g.elem,
            shift: (off - g.lo - i * g.elem) * 8,
        })
    }

    /// The `Ram` struct: each global at its offset, bytes between.
    pub fn ram_struct(&self) -> String {
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
            writeln!(s, "    pub {}: {},", g.name, g.ty()).unwrap();
            at = g.lo + g.len;
        }
        pad(&mut s, at, SIZE);
        s.push_str("}\n");
        s
    }
}

/// A load of `width` bytes from `p`, as a `u32` expression.
pub fn load(p: &Place, width: u32) -> String {
    match (p.shift, width == p.elem, p.elem) {
        (_, true, 4) => p.place.clone(),
        (_, true, _) => format!("{} as u32", p.place),
        (0, _, 4) => format!("({} & {:#x})", p.place, mask(width)),
        (s, _, 4) => format!("({} >> {s} & {:#x})", p.place, mask(width)),
        (s, _, _) => format!("(({} as u32) >> {s} & {:#x})", p.place, mask(width)),
    }
}

/// A store of `width` bytes of the `u32` expression `v` to `p`, as a
/// statement.
pub fn store(p: &Place, width: u32, v: &str) -> String {
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
    format!(
        "{0} = {0} & {keep:#x} | (({v} & {m:#x}) << {s}) as {t};",
        p.place
    )
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
        let g = Globals::carve(&[(0x102, 2), (0x104, 2), (0x200, 4), (0x102, 2)], &names());
        let v: Vec<_> = g.0.values().map(|g| (g.lo, g.len, g.elem)).collect();
        assert_eq!(v, vec![(0x100, 6, 2), (0x200, 4, 4)]);
        assert_eq!(g.0[&0x100].ty(), "[u16; 3]");
        assert_eq!(g.0[&0x100].name, "gTiles");
        assert_eq!(g.0[&0x200].name, "d_02000200");
        assert_eq!(g.0[&0x200].ty(), "u32");
        let p = g.place(0x104, 2).unwrap();
        assert_eq!(p.place, "g.ram.gTiles[2]");
        assert_eq!(load(&p, 2), "g.ram.gTiles[2] as u32");
        assert_eq!(store(&p, 2, "v3"), "g.ram.gTiles[2] = v3 as u16;");
        assert_eq!(load(&g.place(0x200, 4).unwrap(), 4), "g.ram.d_02000200");
        // Outside every global: bytes. With no names, adjacent halfwords
        // are globals of their own.
        assert_eq!(g.place(0x106, 2), None);
        assert_eq!(
            Globals::carve(&[(0x102, 2), (0x104, 2)], &BTreeMap::new())
                .0
                .len(),
            2
        );
    }

    #[test]
    fn a_byte_inside_a_word_is_its_bits() {
        let g = Globals::carve(&[(EWRAM + 0x10, 4), (EWRAM + 0x12, 1)], &names());
        assert_eq!(g.0.len(), 1);
        assert_eq!(g.0[&(EWRAM + 0x10)].name, "gFrames");
        let p = g.place(EWRAM + 0x12, 1).unwrap();
        assert_eq!((p.elem, p.shift), (4, 16));
        assert_eq!(load(&p, 1), "(g.ram.gFrames >> 16 & 0xff)");
        assert_eq!(
            store(&p, 1, "v1"),
            "g.ram.gFrames = g.ram.gFrames & 0xff00ffff | ((v1 & 0xff) << 16) as u32;"
        );
        let g = Globals::carve(&[(0x20, 2), (0x21, 1)], &BTreeMap::new());
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
        let g = Globals::carve(&[(0x300, 4), (0x304, 2)], &names());
        assert_eq!(g.0[&0x300].ty(), "[u16; 3]");
        assert!(g.place(0x300, 4).is_none());
        assert!(g.place(0x304, 2).is_some());
        // A datum no access reaches is no global.
        assert!(!g.0.contains_key(&0x400));
    }

    #[test]
    fn the_struct_is_the_whole_ram() {
        let g = Globals::carve(&[(0x8, 4), (SIZE - 4, 4)], &BTreeMap::new());
        let s = g.ram_struct();
        assert!(s.contains("    _p00000: [u8; 0x8],\n    pub d_02000008: u32,\n"));
        assert!(s.contains("    _p0000c: [u8; 0x47ff0],\n    pub d_03007ffc: u32,\n}"));
        let s = Globals::default().ram_struct();
        assert!(s.contains("    _p00000: [u8; 0x48000],\n}"));
    }
}
