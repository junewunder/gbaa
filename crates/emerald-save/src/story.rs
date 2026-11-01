//! Story-point saves (assets/fuzz-story-seeds): the crafted save moved
//! along the main story, for the fuzzer to start from where story-gated
//! screens are (menus, contests, the Battle Frontier, later maps).
//!
//! `crates/emerald-save/story.toml` lists the points in story order, each what
//! the game's scripts change since the one before (taken from pret's
//! `scripts.inc`): flags set and cleared (badges, `FLAG_HIDE_*` object
//! flags, story flags), vars, key items and HMs, the party's level and
//! where the player stands. Point k's save is the base with points 1..=k
//! applied, so the flags and vars agree with each other as a playthrough
//! leaves them. The file says whether its player is a boy (the rival May)
//! or a girl (Brendan), since the houses' and rival's flags differ; the
//! save's player is made that. Names are pret's, resolved against its headers
//! ([`Consts`]).

use std::collections::HashMap;
use std::path::Path;

use crate::{
    Blocks, Map, SB1_BAG_POCKETS, SB1_CONTINUE_GAME_WARP, SB1_LOCATION, SB1_MAP_LAYOUT_ID,
};
use crate::{SB1_FLAGS, SB1_PARTY, SB1_POS, SB1_VARS, SB2_ENCRYPTION_KEY, Trainer, put_warp};

/// The headers the points' names come from.
pub const HEADERS: [&str; 5] = [
    "include/constants/flags.h",
    "include/constants/opponents.h",
    "include/constants/vars.h",
    "include/constants/items.h",
    "include/constants/map_groups.h",
];

/// `VARS_START`: the first var, `SaveBlock1.vars[0]`.
const VARS_START: u32 = 0x4000;
const VARS: u32 = 256;
const FLAGS: u32 = 300 * 8;
/// The bag's key items pocket, and TMs and HMs', in [`SB1_BAG_POCKETS`].
const KEY_ITEMS: usize = 1;
const TMS_HMS: usize = 3;
/// `SaveBlock2.pokedex`: `order`, `mode`, `nationalMagic`; the national
/// mode and the magic `IsNationalPokedexEnabled` wants.
const SB2_DEX: usize = 0x18;
const DEX_MODE_NATIONAL: u8 = 1;
const NATIONAL_MAGIC: u8 = 0xDA;

/// One story point, as the TOML has it.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub name: String,
    pub title: String,
    /// Where the save stands: a `MAP_` name and a tile.
    pub map: String,
    pub x: i16,
    pub y: i16,
    /// The party's level.
    pub level: u8,
    #[serde(default)]
    pub set: Vec<String>,
    #[serde(default)]
    pub clear: Vec<String>,
    #[serde(default)]
    pub vars: toml::Table,
    #[serde(default)]
    pub items: Vec<String>,
    /// Trainers beaten (`TRAINER_` names): their flags, so gym leaders and
    /// the rival don't offer to battle again.
    #[serde(default)]
    pub beaten: Vec<String>,
    /// The national Pokédex on: `SaveBlock2.pokedex`'s mode and
    /// `nationalMagic`, which no flag or var holds.
    #[serde(default)]
    pub national_dex: bool,
}

/// A story TOML: its player and points, in order.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Story {
    pub male: bool,
    pub point: Vec<Point>,
}

/// Read a story TOML.
pub fn parse(text: &str) -> Result<Story, String> {
    let f: Story = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::new();
    for p in &f.point {
        if !seen.insert(&p.name) {
            return Err(format!("two points named {}", p.name));
        }
    }
    Ok(f)
}

/// pret's constants: each `#define NAME EXPR` and each enum member, by
/// name. Expressions are numbers, names, parentheses and `+ - * | <<`,
/// as the constants headers write them.
#[derive(Debug, Default)]
pub struct Consts {
    exprs: HashMap<String, String>,
}

impl Consts {
    /// The constants of `headers` (each a header's text).
    pub fn new<'a>(headers: impl IntoIterator<Item = &'a str>) -> Consts {
        let mut exprs = HashMap::new();
        for text in headers {
            let mut in_enum = false;
            // An enum member is the last one given a value, plus how many
            // came since.
            let (mut base, mut since) = (String::from("0"), 0);
            for line in text.lines() {
                let line = line.split("//").next().unwrap_or("").trim();
                if let Some(d) = line.strip_prefix("#define ") {
                    let mut it = d.trim().splitn(2, char::is_whitespace);
                    let (Some(name), Some(e)) = (it.next(), it.next()) else {
                        continue;
                    };
                    if !name.contains('(') {
                        exprs.insert(name.to_string(), e.trim().to_string());
                    }
                } else if line.starts_with("enum") {
                    in_enum = true;
                    (base, since) = ("0".into(), 0);
                } else if in_enum && line.starts_with('}') {
                    in_enum = false;
                } else if in_enum {
                    let item = line.trim_end_matches(',');
                    if item.is_empty() || item == "{" {
                        continue;
                    }
                    let name = match item.split_once('=') {
                        Some((n, e)) => {
                            (base, since) = (format!("({})", e.trim()), 0);
                            n.trim()
                        }
                        None => item.trim(),
                    };
                    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        continue;
                    }
                    exprs.insert(name.to_string(), format!("{base} + {since}"));
                    since += 1;
                }
            }
        }
        Consts { exprs }
    }

    /// The headers [`HEADERS`] of the pret tree at `src`.
    pub fn load(src: &Path) -> Result<Consts, String> {
        let texts = HEADERS
            .iter()
            .map(|h| {
                let p = src.join(h);
                std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Consts::new(texts.iter().map(String::as_str)))
    }

    /// The value of `name`.
    pub fn get(&self, name: &str) -> Result<i64, String> {
        self.eval(name, 0)
    }

    fn eval(&self, expr: &str, depth: u32) -> Result<i64, String> {
        if depth > 32 {
            return Err(format!("{expr}: defined in a loop"));
        }
        let tokens = tokens(expr);
        let mut at = 0;
        let v = self.expr(&tokens, &mut at, depth)?;
        if at != tokens.len() {
            return Err(format!("{expr}: can't read past {:?}", tokens[at]));
        }
        Ok(v)
    }

    /// `a op b op c …`, C's precedence for the four operators.
    fn expr(&self, t: &[String], at: &mut usize, depth: u32) -> Result<i64, String> {
        fn prec(op: &str) -> Option<u8> {
            Some(match op {
                "*" => 4,
                "+" | "-" => 3,
                "<<" => 2,
                "|" => 1,
                _ => return None,
            })
        }
        let mut vals = vec![self.atom(t, at, depth)?];
        let mut ops: Vec<&str> = Vec::new();
        let apply = |vals: &mut Vec<i64>, op: &str| {
            let b = vals.pop().unwrap();
            let a = vals.pop().unwrap();
            vals.push(match op {
                "*" => a * b,
                "+" => a + b,
                "-" => a - b,
                "<<" => a << b,
                _ => a | b,
            });
        };
        while let Some(op) = t.get(*at).filter(|o| prec(o).is_some()) {
            *at += 1;
            while let Some(&top) = ops.last()
                && prec(top) >= prec(op)
            {
                apply(&mut vals, ops.pop().unwrap());
            }
            ops.push(op);
            vals.push(self.atom(t, at, depth)?);
        }
        while let Some(op) = ops.pop() {
            apply(&mut vals, op);
        }
        Ok(vals[0])
    }

    fn atom(&self, t: &[String], at: &mut usize, depth: u32) -> Result<i64, String> {
        let tok = t.get(*at).ok_or("an expression ends early")?;
        *at += 1;
        if tok == "(" {
            let v = self.expr(t, at, depth)?;
            if t.get(*at).map(String::as_str) != Some(")") {
                return Err("a ( isn't closed".into());
            }
            *at += 1;
            return Ok(v);
        }
        if let Some(h) = tok.strip_prefix("0x").or(tok.strip_prefix("0X")) {
            return i64::from_str_radix(h, 16).map_err(|_| format!("bad number {tok}"));
        }
        if tok.starts_with(|c: char| c.is_ascii_digit()) {
            return tok.parse().map_err(|_| format!("bad number {tok}"));
        }
        let e = self
            .exprs
            .get(tok)
            .ok_or(format!("{tok}: not in pret's headers"))?;
        self.eval(e, depth + 1)
    }
}

fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cs = s.chars().peekable();
    while let Some(&c) = cs.peek() {
        if c.is_whitespace() {
            cs.next();
        } else if c.is_ascii_alphanumeric() || c == '_' {
            let mut w = String::new();
            while let Some(&c) = cs
                .peek()
                .filter(|c| c.is_ascii_alphanumeric() || **c == '_')
            {
                w.push(c);
                cs.next();
            }
            out.push(w);
        } else if c == '<' {
            cs.next();
            cs.next();
            out.push("<<".into());
        } else {
            out.push(c.to_string());
            cs.next();
        }
    }
    out
}

/// A `MAP_` name's group and number.
pub fn map(c: &Consts, name: &str) -> Result<Map, String> {
    let v = c.get(name)?;
    Ok(Map {
        group: (v >> 8) as u8,
        num: v as u8,
    })
}

/// Apply point `p` to `b`: its flags, vars and items, the party at its
/// level, and the player standing where it says (on the next Continue).
/// `layout` gives a map's layout ID, from the ROM's map headers.
pub fn apply(
    b: &mut Blocks,
    p: &Point,
    c: &Consts,
    t: &Trainer,
    layout: impl Fn(Map) -> Option<u16>,
) -> Result<(), String> {
    let at = |what: &str| format!("{}: {what}", p.name);
    let flag = |name: &str| -> Result<u32, String> {
        let f = c.get(name).map_err(|e| at(&e))? as u32;
        if f == 0 || f >= FLAGS {
            return Err(at(&format!("{name} = {f:#x} isn't a saved flag")));
        }
        Ok(f)
    };
    for name in &p.set {
        let f = flag(name)?;
        b.sb1[SB1_FLAGS + f as usize / 8] |= 1 << (f % 8);
    }
    for name in &p.clear {
        let f = flag(name)?;
        b.sb1[SB1_FLAGS + f as usize / 8] &= !(1 << (f % 8));
    }
    let start = c.get("TRAINER_FLAGS_START").map_err(|e| at(&e))?;
    for name in &p.beaten {
        if !name.starts_with("TRAINER_") {
            return Err(at(&format!("{name} isn't a TRAINER_ name")));
        }
        let f = (start + c.get(name).map_err(|e| at(&e))?) as usize;
        b.sb1[SB1_FLAGS + f / 8] |= 1 << (f % 8);
    }
    if p.national_dex {
        b.sb2[SB2_DEX + 1] = DEX_MODE_NATIONAL;
        b.sb2[SB2_DEX + 2] = NATIONAL_MAGIC;
    }
    for (name, v) in &p.vars {
        let id = c.get(name).map_err(|e| at(&e))? as u32;
        let i = id.wrapping_sub(VARS_START);
        if i >= VARS {
            return Err(at(&format!("{name} = {id:#x} isn't a saved var")));
        }
        let v = v
            .as_integer()
            .ok_or_else(|| at(&format!("{name}'s value isn't a number")))?;
        let v = if v < 0 || v > u16::MAX as i64 {
            return Err(at(&format!("{name} = {v} doesn't fit 16 bits")));
        } else {
            v as u16
        };
        let o = SB1_VARS + 2 * i as usize;
        b.sb1[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    let key = u16::from_le_bytes([b.sb2[SB2_ENCRYPTION_KEY], b.sb2[SB2_ENCRYPTION_KEY + 1]]);
    for name in &p.items {
        let item = c.get(name).map_err(|e| at(&e))? as u16;
        let pocket = if name.starts_with("ITEM_HM") || name.starts_with("ITEM_TM") {
            TMS_HMS
        } else {
            KEY_ITEMS
        };
        let (start, slots) = SB1_BAG_POCKETS[pocket];
        let slot = |s: usize| u16::from_le_bytes([b.sb1[start + 4 * s], b.sb1[start + 4 * s + 1]]);
        if (0..slots).any(|s| slot(s) == item) {
            continue;
        }
        let s = (0..slots)
            .find(|&s| slot(s) == 0)
            .ok_or_else(|| at("the bag pocket is full"))?;
        let o = start + 4 * s;
        b.sb1[o..o + 2].copy_from_slice(&item.to_le_bytes());
        b.sb1[o + 2..o + 4].copy_from_slice(&(1 ^ key).to_le_bytes());
    }
    for (i, m) in t.party.iter().enumerate() {
        let m = crate::Mon {
            level: p.level,
            ..*m
        };
        b.sb1[SB1_PARTY + 100 * i..SB1_PARTY + 100 * (i + 1)]
            .copy_from_slice(&crate::encode_mon(&m, t));
    }
    let m = map(c, &p.map).map_err(|e| at(&e))?;
    let l = layout(m).ok_or_else(|| at(&format!("{} has no map header", p.map)))?;
    b.sb1[SB1_POS..SB1_POS + 2].copy_from_slice(&p.x.to_le_bytes());
    b.sb1[SB1_POS + 2..SB1_POS + 4].copy_from_slice(&p.y.to_le_bytes());
    put_warp(&mut b.sb1, SB1_LOCATION, m, (p.x, p.y));
    put_warp(&mut b.sb1, SB1_CONTINUE_GAME_WARP, m, (p.x, p.y));
    b.sb1[SB1_MAP_LAYOUT_ID..SB1_MAP_LAYOUT_ID + 2].copy_from_slice(&l.to_le_bytes());
    Ok(())
}

/// `gMapGroups` in the US ROM: a pointer a group, each to its maps'
/// headers, whose `mapLayoutId` is at 0x12.
pub const MAP_GROUPS: u32 = 0x0848_6578;

/// Map `m`'s layout ID, from the ROM's map headers.
pub fn layout(rom: &[u8], m: Map) -> Option<u16> {
    let rd = |a: u32| -> Option<u32> {
        let o = a.checked_sub(0x0800_0000)? as usize;
        Some(u32::from_le_bytes(rom.get(o..o + 4)?.try_into().ok()?))
    };
    let group = rd(MAP_GROUPS + 4 * m.group as u32)?;
    let header = rd(group + 4 * m.num as u32)?;
    Some(rd(header + 0x12)? as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAGS_H: &str = "#define TRAINER_FLAGS_START 0x500\n\
        #define MAX_TRAINERS_COUNT 864\n\
        #define TRAINER_FLAGS_END (TRAINER_FLAGS_START + MAX_TRAINERS_COUNT - 1) // 0x85F\n\
        #define SYSTEM_FLAGS (TRAINER_FLAGS_END + 1)\n\
        #define FLAG_BADGE01_GET (SYSTEM_FLAGS + 0x7)\n\
        #define FLAG_HIDE_X 0x2A\n\
        #define TRAINER_X 3\n\
        #define VAR_X_STATE 0x4050\n\
        #define SHIFTED (1 << 4 | 2 * 3)\n";
    const ENUMS_H: &str = "enum\n{\n    // a group\n    MAP_A = (3 | (0 << 8)),\n    MAP_B = (1 | (25 << 8)),\n};\n\
        enum {\n    ITEM_NONE,\n    ITEM_X,\n    ITEM_HM01 = 10,\n    ITEM_HM02,\n};\n";

    #[test]
    fn reads_pret_constants() {
        let c = Consts::new([FLAGS_H, ENUMS_H]);
        assert_eq!(c.get("FLAG_BADGE01_GET"), Ok(0x867));
        assert_eq!(c.get("SHIFTED"), Ok(16 | 6));
        assert_eq!((c.get("ITEM_NONE"), c.get("ITEM_X")), (Ok(0), Ok(1)));
        assert_eq!(c.get("ITEM_HM02"), Ok(11));
        assert_eq!(map(&c, "MAP_B"), Ok(Map { group: 25, num: 1 }));
        assert!(c.get("FLAG_NOPE").is_err());
    }

    #[test]
    fn applies_a_point() {
        let c = Consts::new([FLAGS_H, ENUMS_H]);
        let points = parse(
            "male = true\n[[point]]\nname = \"p\"\ntitle = \"t\"\nmap = \"MAP_A\"\nx = 7\ny = 9\nlevel = 20\n\
             set = [\"FLAG_BADGE01_GET\"]\nclear = [\"FLAG_HIDE_X\"]\n\
             vars = { VAR_X_STATE = 3 }\nitems = [\"ITEM_X\", \"ITEM_HM01\"]\n\
             beaten = [\"TRAINER_X\"]\nnational_dex = true\n",
        )
        .unwrap()
        .point;
        let mut b = Blocks {
            sb2: vec![0; crate::SB2_SIZE],
            sb1: vec![0; crate::SB1_SIZE],
            storage: vec![0; crate::STORAGE_SIZE],
        };
        b.sb1[SB1_FLAGS + 0x2A / 8] = 0xFF;
        b.sb2[SB2_ENCRYPTION_KEY] = 0x55;
        let [alice, _] = crate::trainers();
        apply(&mut b, &points[0], &c, &alice, |_| Some(0x1234)).unwrap();
        let flag = |f: usize| b.sb1[SB1_FLAGS + f / 8] >> (f % 8) & 1;
        assert_eq!((flag(0x867), flag(0x2A), flag(0x2B)), (1, 0, 1));
        assert_eq!(flag(0x503), 1);
        assert_eq!(b.sb2[SB2_DEX..SB2_DEX + 3], [0, 1, 0xDA]);
        assert_eq!(b.sb1[SB1_VARS + 2 * 0x50], 3);
        let (k, t) = (SB1_BAG_POCKETS[KEY_ITEMS].0, SB1_BAG_POCKETS[TMS_HMS].0);
        assert_eq!(b.sb1[k..k + 4], [1, 0, 1 ^ 0x55, 0]);
        assert_eq!(b.sb1[t..t + 2], [10, 0]);
        assert_eq!(crate::location(&b), Map { group: 0, num: 3 });
        assert_eq!(b.sb1[SB1_POS], 7);
        assert_eq!(b.sb1[SB1_MAP_LAYOUT_ID], 0x34);
        // A second time adds no item twice.
        apply(&mut b, &points[0], &c, &alice, |_| Some(0x1234)).unwrap();
        assert_eq!(b.sb1[k + 4], 0);
        assert!(parse("male = true\n[[point]]\nname = \"p\"\n").is_err());
    }
}
