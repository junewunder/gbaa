//! Which lifted functions write and read a named field or global
//! (`gbaa-lift ROM --find FIELD`, `just lift-find`; unit
//! `native/lift-find-field`): the lift's answer to "where is this value
//! set", for a mod's cut.
//!
//! With [`super::Holes::find_touches`] the printer records each load and
//! store whose RAM place it knows ([`Touch`]): a constant address, an
//! address the value sets bound to RAM (`pin`'s and `ix`'s, or none
//! printed), or a field through a pointer the names files type. The field
//! is every place the names files put a field of that name (in each typed
//! global, through arrays and nested structs), or the datum of that name;
//! an access matches where its bytes overlap one. An access whose address
//! the lift can't bound (a pointer from the heap no type explains, one
//! passed in) isn't seen.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use super::globals;
use crate::types::{Ty, Types};

/// One load or store whose RAM place the lift knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Touch {
    /// The function printing it, by key.
    pub func: u32,
    /// Its instruction's address.
    pub at: u32,
    pub write: bool,
    pub place: Place,
}

/// Where a [`Touch`] lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// `w` bytes at one of the offsets in `Game::ram` from `lo` to `hi`
    /// by `stride` (`lo == hi`: exactly there).
    Ram {
        lo: u32,
        hi: u32,
        w: u32,
        stride: u32,
    },
    /// `w` bytes at `off` in an object of type `ty`, through a pointer
    /// the names files type.
    Typed { ty: Ty, off: u32, w: u32 },
}

/// What the finder needs of the names files.
pub struct Names<'a> {
    /// Each datum's identifier and size, by guest address.
    pub data: &'a BTreeMap<u32, (String, u32)>,
    pub types: Option<&'a Types>,
    /// Each source line's first instruction (`file:line`), by address.
    pub lines: &'a BTreeMap<u32, String>,
    /// Each function's name, by key.
    pub fns: &'a BTreeMap<u32, String>,
}

/// The most places of one field a type's arrays are expanded to.
const CAP: usize = 1 << 16;

/// One function's accesses that name the field.
#[derive(Debug, Default)]
pub struct Hit {
    pub name: String,
    /// `(write, maybe, line, path)`, each once: `maybe` where its
    /// address is one of several, the field's one of them.
    pub accesses: BTreeSet<(bool, bool, String, String)>,
}

/// The places named `field`, as `Game::ram` byte ranges with their
/// paths (`gEquipment.maxEnergy`), by start.
fn ram_places(field: &str, names: &Names) -> BTreeMap<u32, Vec<(u32, String)>> {
    let mut out: BTreeMap<u32, Vec<(u32, String)>> = BTreeMap::new();
    let name_of = |a: u32| {
        names
            .data
            .get(&a)
            .map_or_else(|| format!("d_{a:08x}"), |(n, _)| n.clone())
    };
    if let Some(t) = names.types {
        for (a, ty) in t.globals() {
            let Some(r) = globals::ram_at(a) else {
                continue;
            };
            for (o, s, p) in t.spans_of(ty, field, CAP) {
                out.entry(r + o)
                    .or_default()
                    .push((r + o + s, format!("{}{p}", name_of(a))));
            }
        }
    }
    for (&a, (n, size)) in names.data {
        if n == field
            && let Some(r) = globals::ram_at(a)
        {
            out.entry(r)
                .or_default()
                .push((r + (*size).max(1), n.clone()));
        }
    }
    out
}

/// Whether the field's name is one the names files know: a field of a
/// struct some typed global or pointer may hold, or a datum's.
pub fn known(field: &str, names: &Names) -> bool {
    !ram_places(field, names).is_empty()
        || names.types.is_some_and(|t| {
            t.globals()
                .any(|(_, ty)| !t.spans_of(ty, field, 1).is_empty())
        })
}

/// The functions of `touches` that write or read `field`, by key.
pub fn find(touches: &[Touch], field: &str, names: &Names) -> BTreeMap<u32, Hit> {
    let places = ram_places(field, names);
    let longest = places
        .iter()
        .flat_map(|(&lo, v)| v.iter().map(move |(hi, _)| hi - lo))
        .max()
        .unwrap_or(0);
    // A typed pointer's struct: the field's places in it, once a type.
    let mut in_ty: BTreeMap<String, Vec<(u32, u32, String)>> = BTreeMap::new();
    let mut out: BTreeMap<u32, Hit> = BTreeMap::new();
    for t in touches {
        let mut paths: Vec<(bool, String)> = Vec::new();
        match &t.place {
            &Place::Ram { lo, hi, w, stride } => {
                let exact = lo == hi;
                for (&a, v) in places.range(lo.saturating_sub(longest)..hi + w) {
                    for (b, path) in v {
                        // The first slot reaching past `a`, if before `b`.
                        let k = (a + 1)
                            .saturating_sub(w)
                            .saturating_sub(lo)
                            .div_ceil(stride);
                        let x = lo + k * stride;
                        if x <= hi && x < *b && x + w > a {
                            paths.push((!exact, path.clone()));
                        }
                    }
                }
            }
            Place::Typed { ty, off, w } => {
                let Some(types) = names.types else { continue };
                let Ty::Named(n) = ty else { continue };
                let spans = in_ty
                    .entry(n.clone())
                    .or_insert_with(|| types.spans_of(ty, field, CAP));
                let short = n
                    .strip_prefix("struct ")
                    .or_else(|| n.strip_prefix("union "))
                    .unwrap_or(n);
                for (o, s, p) in spans.iter() {
                    if *o < off + w && o + s > *off {
                        paths.push((false, format!("{short}{p}")));
                    }
                }
            }
        }
        if paths.is_empty() {
            continue;
        }
        let entry = t.func & !1;
        let line = names
            .lines
            .range(..=t.at)
            .next_back()
            .filter(|&(&a, _)| a >= entry)
            .map_or_else(|| format!("{:#010x}", t.at), |(_, l)| l.clone());
        let hit = out.entry(t.func).or_insert_with(|| Hit {
            name: names
                .fns
                .get(&t.func)
                .cloned()
                .unwrap_or_else(|| format!("f_{:08x}", t.func)),
            ..Hit::default()
        });
        for (maybe, p) in paths {
            hit.accesses.insert((t.write, maybe, line.clone(), p));
        }
    }
    out
}

/// `hits` as `gbaa-lift --find` prints them: writers, then readers, a
/// line a function, each access by its source line (else its
/// instruction's address) and path; then those that may write or read
/// it, whose address is one of several.
pub fn report(field: &str, hits: &BTreeMap<u32, Hit>) -> String {
    let mut s = String::new();
    let side = |write: bool, maybe: bool| -> Vec<(u32, &Hit)> {
        hits.iter()
            .filter(|(_, h)| h.accesses.iter().any(|a| (a.0, a.1) == (write, maybe)))
            .map(|(&k, h)| (k, h))
            .collect()
    };
    let lists = [
        ("writers", side(true, false), true, false),
        ("readers", side(false, false), false, false),
        (
            "may write (its address one of several, the field's one of them)",
            side(true, true),
            true,
            true,
        ),
        ("may read", side(false, true), false, true),
    ];
    writeln!(
        s,
        "{field}: {} writers, {} readers in the lift; {} may write it, {} may read it",
        lists[0].1.len(),
        lists[1].1.len(),
        lists[2].1.len(),
        lists[3].1.len()
    )
    .unwrap();
    for (title, list, write, maybe) in lists {
        if list.is_empty() {
            continue;
        }
        writeln!(s, "{title}:").unwrap();
        for (k, h) in list {
            // Each source line once, with the paths it names.
            let mut lines: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for (_, _, l, p) in h.accesses.iter().filter(|a| (a.0, a.1) == (write, maybe)) {
                let ps = lines.entry(l).or_default();
                if !ps.contains(&p.as_str()) {
                    ps.push(p);
                }
            }
            let lines: Vec<String> = lines
                .into_iter()
                .map(|(l, ps)| format!("{l} {}", ps.join(" ")))
                .collect();
            writeln!(s, "  {} ({k:#010x}): {}", h.name, lines.join(", ")).unwrap();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_datum_by_name_exact_and_maybe() {
        // gHp: 2 bytes at IWRAM 0x03000100 (Game::ram offset 0x40100).
        let data = BTreeMap::from([(0x0300_0100, ("gHp".to_string(), 2))]);
        let lines = BTreeMap::from([(0x0800_0010, "src/hp.c:7".to_string())]);
        let fns = BTreeMap::from([(0x0800_0001, "HpInit".to_string())]);
        let names = Names {
            data: &data,
            types: None,
            lines: &lines,
            fns: &fns,
        };
        assert!(known("gHp", &names));
        assert!(!known("gMp", &names));
        let ram = |lo, hi, w, stride| Place::Ram { lo, hi, w, stride };
        let touch = |func, at, write, place| Touch {
            func,
            at,
            write,
            place,
        };
        let touches = [
            // A halfword store on it, under its line.
            touch(0x0800_0001, 0x0800_0012, true, ram(0x40100, 0x40100, 2, 2)),
            // A word load over it and the next two bytes.
            touch(0x0800_0001, 0x0800_0014, false, ram(0x400fe, 0x400fe, 4, 4)),
            // Words of a range it lies in: may read.
            touch(0x0800_0101, 0x0800_0110, false, ram(0x400f0, 0x40110, 4, 4)),
            // Elements of 8 bytes that step over it: not it.
            touch(0x0800_0201, 0x0800_0210, true, ram(0x400fc, 0x4010c, 2, 8)),
            // Next door.
            touch(0x0800_0301, 0x0800_0310, true, ram(0x40102, 0x40102, 2, 2)),
        ];
        let hits = find(&touches, "gHp", &names);
        assert_eq!(
            hits.keys().copied().collect::<Vec<_>>(),
            [0x0800_0001, 0x0800_0101]
        );
        let r = report("gHp", &hits);
        assert_eq!(
            r,
            "gHp: 1 writers, 1 readers in the lift; 0 may write it, 1 may read it\n\
             writers:\n  HpInit (0x08000001): src/hp.c:7 gHp\n\
             readers:\n  HpInit (0x08000001): src/hp.c:7 gHp\n\
             may read:\n  f_08000101 (0x08000101): 0x08000110 gHp\n"
        );
    }
}
