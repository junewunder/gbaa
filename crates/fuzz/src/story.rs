//! Story points from a TAS (assets/fuzz-story-seeds): mGBA states saved
//! along a replayed movie (`mgba-dump --bk2 … --save-state-at`), read for
//! where the game is in the story. `gbaa-fuzz story DIR` lists each
//! `state-N.ss` in DIR: its frame, map, badges, a few story flags and the
//! party's size. Badges that only grow are the sign the movie stayed in
//! sync; the fuzzer's workers take the states as seeds.

use std::path::{Path, PathBuf};

use emerald_save as es;

/// Where mGBA's state (`GBASerializedState`) keeps IWRAM and EWRAM, as
/// `runtime::state` reads them.
const IWRAM: usize = 0x1_9000;
const EWRAM: usize = 0x2_1000;
const STATE_SIZE: usize = 0x6_1000;

/// pokeemerald's `gPlayerPartyCount`.
const PARTY_COUNT: u32 = 0x0202_44E9;
/// `FLAG_BADGE01_GET`; the other seven follow it.
const FLAG_BADGE01: u16 = 0x867;
/// Story flags worth a column, by name.
const FLAGS: [(&str, u16); 5] = [
    ("dex", 0x861),      // FLAG_SYS_POKEDEX_GET
    ("pokenav", 0x0BC),  // FLAG_RECEIVED_POKENAV
    ("clear", 0x864),    // FLAG_SYS_GAME_CLEAR
    ("frontier", 0x8D2), // FLAG_SYS_FRONTIER_PASS
    ("national", 0x896), // FLAG_SYS_NATIONAL_DEX
];

/// What a state says about the story.
#[derive(Debug, PartialEq, Eq)]
pub struct Point {
    pub map: (u8, u8),
    pub badges: u8,
    /// Which of [`FLAGS`] are set, a bit each.
    pub flags: u8,
    pub party: u8,
}

/// The byte at `addr` in the state's IWRAM or EWRAM.
fn read8(s: &[u8], addr: u32) -> Option<u8> {
    let at = match addr >> 24 {
        2 => EWRAM + (addr & 0x3_FFFF) as usize,
        3 => IWRAM + (addr & 0x7FFF) as usize,
        _ => return None,
    };
    s.get(at).copied()
}

fn read32(s: &[u8], addr: u32) -> Option<u32> {
    let b = [0, 1, 2, 3].map(|i| read8(s, addr + i));
    Some(u32::from_le_bytes([b[0]?, b[1]?, b[2]?, b[3]?]))
}

/// Read a state, `None` if it isn't one or the save block isn't set up.
pub fn point(s: &[u8]) -> Option<Point> {
    if s.len() < STATE_SIZE {
        return None;
    }
    let sb1 = read32(s, es::SB1_PTR)?;
    if !(0x0200_0000..0x0204_0000).contains(&sb1) {
        return None;
    }
    let flag = |f: u16| {
        read8(s, sb1 + es::SB1_FLAGS as u32 + f as u32 / 8).is_some_and(|b| b >> (f % 8) & 1 != 0)
    };
    let badges = (0..8).filter(|&i| flag(FLAG_BADGE01 + i)).count() as u8;
    let flags = FLAGS
        .iter()
        .enumerate()
        .filter(|(_, (_, f))| flag(*f))
        .fold(0, |m, (i, _)| m | 1 << i);
    Some(Point {
        map: (read8(s, sb1 + 4)?, read8(s, sb1 + 5)?),
        badges,
        flags,
        party: read8(s, PARTY_COUNT)?,
    })
}

/// The `state-N.ss` files in `dir`, by N.
pub fn states(dir: &Path) -> Result<Vec<(u64, PathBuf)>, String> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let n = name
                .strip_prefix("state-")?
                .strip_suffix(".ss")?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .collect();
    v.sort();
    Ok(v)
}

/// `gbaa-fuzz story DIR`: a line a state.
pub fn run(dir: &Path) -> Result<(), String> {
    println!(
        "frame\tmap\tbadges\tparty\t{}",
        FLAGS.map(|(n, _)| n).join("\t")
    );
    for (n, path) in states(dir)? {
        let s = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        match point(&s) {
            Some(p) => println!(
                "{n}\t{}.{}\t{}\t{}\t{}",
                p.map.0,
                p.map.1,
                p.badges,
                p.party,
                (0..FLAGS.len())
                    .map(|i| if p.flags >> i & 1 != 0 { "y" } else { "-" })
                    .collect::<Vec<_>>()
                    .join("\t")
            ),
            None => println!("{n}\t(no save block)"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_state() {
        let mut s = vec![0u8; STATE_SIZE];
        let sb1 = 0x0202_5A00u32;
        let at = |a: u32| EWRAM + (a & 0x3_FFFF) as usize;
        let p = IWRAM + (es::SB1_PTR & 0x7FFF) as usize;
        s[p..p + 4].copy_from_slice(&sb1.to_le_bytes());
        s[at(sb1) + 4] = 9;
        s[at(sb1) + 5] = 3;
        // Badges 1 and 2, and the game clear flag.
        s[at(sb1) + es::SB1_FLAGS + 0x867 / 8] |= 1 << (0x867 % 8);
        s[at(sb1) + es::SB1_FLAGS + 0x868 / 8] |= 1 << (0x868 % 8);
        s[at(sb1) + es::SB1_FLAGS + 0x864 / 8] |= 1 << (0x864 % 8);
        s[at(PARTY_COUNT)] = 4;
        let p = point(&s).unwrap();
        assert_eq!(
            p,
            Point {
                map: (9, 3),
                badges: 2,
                flags: 1 << 2,
                party: 4
            }
        );
        assert_eq!(point(&s[..100]), None);
        assert_eq!(point(&vec![0; STATE_SIZE]), None);
    }
}
