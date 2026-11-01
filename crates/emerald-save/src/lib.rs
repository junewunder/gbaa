//! Crafted Pokémon Emerald saves for link tests (multiplayer/cable-club-saves).
//!
//! A save is the game's own new-game data (its three save blocks as
//! `NewGameInitData` leaves them, taken from a run to the truck: see
//! `main.rs`), patched: a trainer's name, gender and ID, a party, a few
//! flags, and a continue-game warp to Oldale Town's Pokémon Center 2F, in
//! front of the cable club attendant. Then it is written as the game writes
//! flash: 14 checksummed sectors in slot 1, the rest erased.
//!
//! Offsets are pokeemerald's (`include/global.h`, `include/pokemon.h`,
//! `include/save.h`), as `arm-none-eabi-gdb -ex 'ptype /o struct
//! SaveBlock1'` prints them from the ELF in `$GBAA_DATA/src/pokeemerald`.

/// The flash chip: 128 KB.
pub const SAVE_SIZE: usize = 0x20000;
/// `sizeof(struct SaveBlock2)`, `SaveBlock1`, `PokemonStorage`.
pub const SB2_SIZE: usize = 3884;
pub const SB1_SIZE: usize = 15752;
pub const STORAGE_SIZE: usize = 33744;

const SECTOR_SIZE: usize = 0x1000;
const SECTOR_DATA_SIZE: usize = 3968;
const SECTOR_SIGNATURE: u32 = 0x0801_2025;
const SECTORS_PER_SLOT: usize = 14;
// The footer: id, checksum, signature, counter.
const FOOTER_ID: usize = 0xFF4;
const FOOTER_CHECKSUM: usize = 0xFF6;
const FOOTER_SIGNATURE: usize = 0xFF8;
const FOOTER_COUNTER: usize = 0xFFC;

// struct SaveBlock2.
const SB2_NAME: usize = 0x00;
pub const SB2_GENDER: usize = 0x08;
pub const SB2_SPECIAL_WARP_FLAGS: usize = 0x09;
const SB2_TRAINER_ID: usize = 0x0A;
pub const SB2_DEX_OWNED: usize = 0x28;
pub const SB2_DEX_SEEN: usize = 0x5C;
/// `specialSaveWarpFlags`' `CONTINUE_GAME_WARP` (`include/save_location.h`).
pub const CONTINUE_GAME_WARP: u8 = 1;

// struct SaveBlock1.
pub const SB1_POS: usize = 0x00;
pub const SB1_LOCATION: usize = 0x04;
pub const SB1_CONTINUE_GAME_WARP: usize = 0x0C;
const SB1_LAST_HEAL_LOCATION: usize = 0x1C;
pub const SB1_MAP_LAYOUT_ID: usize = 0x32;
pub const SB1_PARTY_COUNT: usize = 0x234;
pub const SB1_PARTY: usize = 0x238;
pub const SB1_DEX_SEEN1: usize = 0x988;
pub const SB1_FLAGS: usize = 0x1270;
/// The bag's pockets: items, key items, Poké Balls, TMs and HMs, berries
/// (`struct ItemSlot` each: item, then quantity XORed with the low half of
/// `SaveBlock2.encryptionKey`), with their slots.
pub const SB1_BAG_POCKETS: [(usize, usize); 5] = [
    (0x560, 30),
    (0x5D8, 30),
    (0x650, 16),
    (0x690, 64),
    (0x790, 46),
];
/// `SaveBlock1.vars`, 256 of them.
pub const SB1_VARS: usize = 0x139C;
/// `SaveBlock2.encryptionKey`.
pub const SB2_ENCRYPTION_KEY: usize = 0xAC;
pub const SB1_DEX_SEEN2: usize = 0x3B24;

/// A map, as `MAP_GROUP` and `MAP_NUM` of `include/constants/map_groups.h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Map {
    pub group: u8,
    pub num: u8,
}

/// Where `NewGameInitData` leaves the player: `MAP_INSIDE_OF_TRUCK`.
pub const INSIDE_OF_TRUCK: Map = Map { group: 25, num: 40 };
/// `MAP_OLDALE_TOWN_POKEMON_CENTER_2F`: the cable club.
pub const OLDALE_PC_2F: Map = Map { group: 2, num: 3 };
/// `LAYOUT_POKEMON_CENTER_2F`.
const LAYOUT_POKEMON_CENTER_2F: u16 = 62;
/// `MAP_OLDALE_TOWN`, and its heal location (`src/data/heal_locations.json`).
const OLDALE_TOWN: Map = Map { group: 0, num: 10 };
const OLDALE_HEAL: (i16, i16) = (6, 17);
/// The tile below the cable club attendant (`Common_EventScript_DirectCornerAttendant`
/// at 10,2 in `data/maps/OldaleTown_PokemonCenter_2F/map.json`).
pub const AT_ATTENDANT: (i16, i16) = (10, 3);

// include/constants/flags.h.
const FLAG_SYS_POKEMON_GET: u16 = 0x860;
/// Every cable club attendant wants it (`CableClub_EventScript_DirectCornerAttendant`).
const FLAG_SYS_POKEDEX_GET: u16 = 0x861;
const FLAG_RECEIVED_POKEDEX_FROM_BIRCH: u16 = 0x8E4;
const FLAG_VISITED_LITTLEROOT_TOWN: u16 = 0x86F;
const FLAG_VISITED_OLDALE_TOWN: u16 = 0x870;

/// `WARP_ID_NONE`: the warp places the player at its x, y.
const WARP_ID_NONE: u8 = 0xFF;

/// A species, with what its level-1 moves and stats need
/// (`src/data/pokemon/species_info.h`, `level_up_learnsets.h`,
/// `src/data/battle_moves.h`).
#[derive(Clone, Copy, Debug)]
pub struct Species {
    pub id: u16,
    /// The national dex number (`include/constants/pokedex.h`).
    pub dex: u16,
    pub name: &'static str,
    /// HP, attack, defense, speed, special attack, special defense.
    pub base: [u16; 6],
    pub growth: Growth,
    /// Move IDs and their PP.
    pub moves: [(u16, u8); 2],
}

#[derive(Clone, Copy, Debug)]
pub enum Growth {
    MediumFast,
    MediumSlow,
}

impl Growth {
    /// Experience at `level` (`src/data/pokemon/experience_tables.h`).
    pub fn exp(self, level: u32) -> u32 {
        let n = level;
        match self {
            Growth::MediumFast => n * n * n,
            Growth::MediumSlow => (6 * n * n * n / 5 + 100 * n).saturating_sub(15 * n * n + 140),
        }
    }
}

const TACKLE: (u16, u8) = (33, 35);
const GROWL: (u16, u8) = (45, 40);
const SCRATCH: (u16, u8) = (10, 35);
const STRING_SHOT: (u16, u8) = (81, 40);

pub const MUDKIP: Species = Species {
    id: 283,
    dex: 258,
    name: "MUDKIP",
    base: [50, 70, 50, 40, 50, 50],
    growth: Growth::MediumSlow,
    moves: [TACKLE, GROWL],
};
pub const TORCHIC: Species = Species {
    id: 280,
    dex: 255,
    name: "TORCHIC",
    base: [45, 60, 40, 45, 70, 50],
    growth: Growth::MediumSlow,
    moves: [SCRATCH, GROWL],
};
pub const ZIGZAGOON: Species = Species {
    id: 288,
    dex: 263,
    name: "ZIGZAGOON",
    base: [38, 30, 41, 60, 30, 41],
    growth: Growth::MediumFast,
    moves: [TACKLE, GROWL],
};
pub const WURMPLE: Species = Species {
    id: 290,
    dex: 265,
    name: "WURMPLE",
    base: [45, 45, 35, 20, 20, 30],
    growth: Growth::MediumFast,
    moves: [TACKLE, STRING_SHOT],
};

/// A party Pokémon: caught on Route 101 in a Poké Ball, every IV 15, no EVs.
#[derive(Clone, Copy, Debug)]
pub struct Mon {
    pub species: Species,
    pub level: u8,
    pub personality: u32,
}

/// A trainer and their party.
#[derive(Clone, Debug)]
pub struct Trainer {
    /// The file name, without `.sav`.
    pub file: &'static str,
    /// Up to 7 characters (A-Z, a-z, 0-9, space).
    pub name: &'static str,
    pub female: bool,
    /// The trainer ID in the low half, the secret ID in the high half.
    pub id: u32,
    pub party: Vec<Mon>,
}

/// The two link-test trainers: different names, genders, IDs and parties,
/// two Pokémon each (the trade centre wants two, `CheckPartyTradeRequirements`).
pub fn trainers() -> [Trainer; 2] {
    [
        Trainer {
            file: "alice",
            name: "ALICE",
            female: true,
            id: 54321 << 16 | 12345,
            party: vec![
                Mon {
                    species: MUDKIP,
                    level: 5,
                    personality: 0x1A2B_3C4D,
                },
                Mon {
                    species: WURMPLE,
                    level: 3,
                    personality: 0x5E6F_7081,
                },
            ],
        },
        Trainer {
            file: "bob",
            name: "BOB",
            female: false,
            id: 13579 << 16 | 24680,
            party: vec![
                Mon {
                    species: TORCHIC,
                    level: 5,
                    personality: 0x9A8B_7C6D,
                },
                Mon {
                    species: ZIGZAGOON,
                    level: 3,
                    personality: 0x2468_ACE0,
                },
            ],
        },
    ]
}

/// The game's three save blocks, as they sit in EWRAM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocks {
    pub sb2: Vec<u8>,
    pub sb1: Vec<u8>,
    pub storage: Vec<u8>,
}

/// `s` in the game's character set (`charmap.txt`), padded with `EOS`
/// (0xFF) to `len`. Panics on a character it doesn't have or a string
/// longer than `len`.
pub fn text(s: &str, len: usize) -> Vec<u8> {
    assert!(s.len() <= len, "{s:?} is longer than {len}");
    let mut out: Vec<u8> = s
        .chars()
        .map(|c| match c {
            ' ' => 0x00,
            '0'..='9' => 0xA1 + (c as u8 - b'0'),
            'A'..='Z' => 0xBB + (c as u8 - b'A'),
            'a'..='z' => 0xD5 + (c as u8 - b'a'),
            _ => panic!("{c:?} isn't in the charmap here"),
        })
        .collect();
    out.resize(len, 0xFF);
    out
}

/// Where each of the four substructures (growth, attacks, EVs, misc) sits,
/// by `personality % 24` (`GetSubstruct` in `src/pokemon.c`).
const SUBSTRUCT_ORDER: [[usize; 4]; 24] = [
    [0, 1, 2, 3],
    [0, 1, 3, 2],
    [0, 2, 1, 3],
    [0, 3, 1, 2],
    [0, 2, 3, 1],
    [0, 3, 2, 1],
    [1, 0, 2, 3],
    [1, 0, 3, 2],
    [2, 0, 1, 3],
    [3, 0, 1, 2],
    [2, 0, 3, 1],
    [3, 0, 2, 1],
    [1, 2, 0, 3],
    [1, 3, 0, 2],
    [2, 1, 0, 3],
    [3, 1, 0, 2],
    [2, 3, 0, 1],
    [3, 2, 0, 1],
    [1, 2, 3, 0],
    [1, 3, 2, 0],
    [2, 1, 3, 0],
    [3, 1, 2, 0],
    [2, 3, 1, 0],
    [3, 2, 1, 0],
];

pub const IV: u16 = 15;
const STANDARD_FRIENDSHIP: u8 = 70;
const LANGUAGE_ENGLISH: u8 = 2;
const VERSION_EMERALD: u16 = 3;
const ITEM_POKE_BALL: u16 = 4;
const MAPSEC_ROUTE_101: u8 = 16;
const MAIL_NONE: u8 = 0xFF;

/// A mon's stats: max HP, attack, defense, speed, special attack, special
/// defense, as `CalculateMonStats` computes them with no EVs.
pub fn stats(m: &Mon) -> [u16; 6] {
    let l = m.level as u16;
    let nature = (m.personality % 25) as usize;
    // gNatureStatTable: nature / 5 raises a stat, nature % 5 lowers one,
    // in the order attack, defense, speed, special attack, special defense.
    let (up, down) = (nature / 5 + 1, nature % 5 + 1);
    std::array::from_fn(|i| {
        let n = (2 * m.species.base[i] + IV) * l / 100;
        match () {
            _ if i == 0 => n + l + 10,
            _ if up != down && i == up => (n + 5) * 110 / 100,
            _ if up != down && i == down => (n + 5) * 90 / 100,
            _ => n + 5,
        }
    })
}

/// `struct Pokemon` (100 bytes): the box part encrypted and checksummed as
/// `CreateMon` leaves it, then the party stats.
pub fn encode_mon(m: &Mon, t: &Trainer) -> [u8; 100] {
    let mut out = [0u8; 100];
    let s = &m.species;
    out[0..4].copy_from_slice(&m.personality.to_le_bytes());
    out[4..8].copy_from_slice(&t.id.to_le_bytes());
    out[8..18].copy_from_slice(&text(s.name, 10));
    out[18] = LANGUAGE_ENGLISH;
    out[19] = 1 << 1; // hasSpecies
    out[20..27].copy_from_slice(&text(t.name, 7));

    let mut sub = [[0u8; 12]; 4];
    // Growth: species, held item, experience, PP bonuses, friendship.
    sub[0][0..2].copy_from_slice(&s.id.to_le_bytes());
    sub[0][4..8].copy_from_slice(&s.growth.exp(m.level as u32).to_le_bytes());
    sub[0][9] = STANDARD_FRIENDSHIP;
    // Attacks: moves, then PP.
    for (i, &(id, pp)) in s.moves.iter().enumerate() {
        sub[1][2 * i..2 * i + 2].copy_from_slice(&id.to_le_bytes());
        sub[1][8 + i] = pp;
    }
    // EVs and contest stats stay 0. Misc: met location, then met level, game,
    // ball and OT gender, then the IVs (ability 0, not an egg).
    sub[3][1] = MAPSEC_ROUTE_101;
    let origins =
        m.level as u16 | VERSION_EMERALD << 7 | ITEM_POKE_BALL << 11 | (t.female as u16) << 15;
    sub[3][2..4].copy_from_slice(&origins.to_le_bytes());
    let ivs = (0..6).fold(0u32, |a, i| a | (IV as u32) << (5 * i));
    sub[3][4..8].copy_from_slice(&ivs.to_le_bytes());

    let checksum = sub.iter().flat_map(|b| b.chunks(2)).fold(0u16, |a, w| {
        a.wrapping_add(u16::from_le_bytes([w[0], w[1]]))
    });
    out[28..30].copy_from_slice(&checksum.to_le_bytes());

    let order = SUBSTRUCT_ORDER[(m.personality % 24) as usize];
    let key = m.personality ^ t.id;
    for (kind, b) in sub.iter().enumerate() {
        let at = 32 + 12 * order[kind];
        for (i, w) in b.chunks(4).enumerate() {
            let w = u32::from_le_bytes(w.try_into().unwrap()) ^ key;
            out[at + 4 * i..at + 4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
    }

    // Party data: status, level, mail, HP, then the stats.
    out[84] = m.level;
    out[85] = MAIL_NONE;
    let st = stats(m);
    out[86..88].copy_from_slice(&st[0].to_le_bytes());
    for (i, v) in st.iter().enumerate() {
        out[88 + 2 * i..90 + 2 * i].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// Give an encoded `struct Pokemon` the ribbons in `ribbons` (the misc
/// substructure's ribbon word: cool, beauty, cute, smart and tough ranks,
/// three bits each from bit 0, then the others), its checksum kept.
pub fn add_ribbons(mon: &mut [u8; 100], ribbons: u32) {
    edit_word(mon, 3, 2, |w| w | ribbons);
}

/// Give an encoded `struct Pokemon` held item `item` (the growth
/// substructure's first word: species, then held item), its checksum kept.
pub fn set_held_item(mon: &mut [u8; 100], item: u16) {
    edit_word(mon, 0, 0, |w| (w & 0xFFFF) | (item as u32) << 16);
}

/// Word `i` of substructure `kind` (growth, attacks, EVs, misc) of an
/// encoded `struct Pokemon`, changed by `f`: decrypted, changed, encrypted
/// again, and the checksum adjusted.
fn edit_word(mon: &mut [u8; 100], kind: usize, i: usize, f: impl FnOnce(u32) -> u32) {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let personality = word(mon, 0);
    let key = personality ^ word(mon, 4);
    let at = 32 + 12 * SUBSTRUCT_ORDER[(personality % 24) as usize][kind] + 4 * i;
    let old = word(mon, at) ^ key;
    let new = f(old);
    mon[at..at + 4].copy_from_slice(&(new ^ key).to_le_bytes());
    let halves = |w: u32| (w as u16).wrapping_add((w >> 16) as u16);
    let sum = u16::from_le_bytes([mon[28], mon[29]])
        .wrapping_sub(halves(old))
        .wrapping_add(halves(new));
    mon[28..30].copy_from_slice(&sum.to_le_bytes());
}

/// A `WarpData`: group, num, warp ID, pad, x, y.
pub fn put_warp(b: &mut [u8], at: usize, map: Map, (x, y): (i16, i16)) {
    b[at] = map.group;
    b[at + 1] = map.num;
    b[at + 2] = WARP_ID_NONE;
    b[at + 4..at + 6].copy_from_slice(&x.to_le_bytes());
    b[at + 6..at + 8].copy_from_slice(&y.to_le_bytes());
}

pub fn set_flag(sb1: &mut [u8], flag: u16) {
    sb1[SB1_FLAGS + flag as usize / 8] |= 1 << (flag % 8);
}

/// The truck's blocks, made `t`'s: standing below the cable club attendant
/// on the next Continue, with `t`'s party. Everything else stays as the new
/// game left it (money, bag, options, the encryption key).
pub fn patch(base: &Blocks, t: &Trainer) -> Blocks {
    assert!(!t.party.is_empty() && t.party.len() <= 6);
    let mut b = base.clone();
    let sb2 = &mut b.sb2;
    sb2[SB2_NAME..SB2_NAME + 8].copy_from_slice(&text(t.name, 8));
    sb2[SB2_GENDER] = t.female as u8;
    sb2[SB2_SPECIAL_WARP_FLAGS] |= CONTINUE_GAME_WARP;
    sb2[SB2_TRAINER_ID..SB2_TRAINER_ID + 4].copy_from_slice(&t.id.to_le_bytes());

    let sb1 = &mut b.sb1;
    let (x, y) = AT_ATTENDANT;
    sb1[SB1_POS..SB1_POS + 2].copy_from_slice(&x.to_le_bytes());
    sb1[SB1_POS + 2..SB1_POS + 4].copy_from_slice(&y.to_le_bytes());
    // The map the save loads first (`LoadSaveblockMapHeader`) and the one
    // `CB2_ContinueSavedGame` then warps into, objects made afresh.
    put_warp(sb1, SB1_LOCATION, OLDALE_PC_2F, AT_ATTENDANT);
    put_warp(sb1, SB1_CONTINUE_GAME_WARP, OLDALE_PC_2F, AT_ATTENDANT);
    put_warp(sb1, SB1_LAST_HEAL_LOCATION, OLDALE_TOWN, OLDALE_HEAL);
    sb1[SB1_MAP_LAYOUT_ID..SB1_MAP_LAYOUT_ID + 2]
        .copy_from_slice(&LAYOUT_POKEMON_CENTER_2F.to_le_bytes());
    sb1[SB1_PARTY_COUNT] = t.party.len() as u8;
    sb1[SB1_PARTY..SB1_PARTY + 600].fill(0);
    for (i, m) in t.party.iter().enumerate() {
        sb1[SB1_PARTY + 100 * i..SB1_PARTY + 100 * (i + 1)].copy_from_slice(&encode_mon(m, t));
    }
    for f in [
        FLAG_SYS_POKEMON_GET,
        FLAG_SYS_POKEDEX_GET,
        FLAG_RECEIVED_POKEDEX_FROM_BIRCH,
        FLAG_VISITED_LITTLEROOT_TOWN,
        FLAG_VISITED_OLDALE_TOWN,
    ] {
        set_flag(sb1, f);
    }
    // Caught, as `GetSetPokedexFlag` marks it: owned and seen in block 2,
    // and seen in block 1's two copies.
    for m in &t.party {
        let n = m.species.dex as usize - 1;
        let (at, bit) = (n / 8, 1 << (n % 8));
        for off in [SB2_DEX_OWNED, SB2_DEX_SEEN] {
            b.sb2[off + at] |= bit;
        }
        for off in [SB1_DEX_SEEN1, SB1_DEX_SEEN2] {
            b.sb1[off + at] |= bit;
        }
    }
    b
}

/// `CalculateChecksum` in `src/save.c`: the 32-bit sum of the words, folded.
pub fn checksum(data: &[u8]) -> u16 {
    let sum = data
        .as_chunks::<4>()
        .0
        .iter()
        .fold(0u32, |a, w| a.wrapping_add(u32::from_le_bytes(*w)));
    ((sum >> 16) as u16).wrapping_add(sum as u16)
}

/// The flash image: slot 1's 14 sectors with save counter 0 (so slot 1 is
/// the one read), sector N holding section N (`sSaveSlotLayout`), and the
/// rest of the chip erased (0xFF), as on a cartridge saved once.
pub fn flash(b: &Blocks) -> Vec<u8> {
    assert_eq!(
        (b.sb2.len(), b.sb1.len(), b.storage.len()),
        (SB2_SIZE, SB1_SIZE, STORAGE_SIZE)
    );
    let mut out = vec![0xFF; SAVE_SIZE];
    let chunks = |data: &[u8], n: usize| -> Vec<Vec<u8>> {
        let v: Vec<Vec<u8>> = data.chunks(SECTOR_DATA_SIZE).map(<[u8]>::to_vec).collect();
        assert_eq!(v.len(), n);
        v
    };
    let mut sections = chunks(&b.sb2, 1);
    sections.extend(chunks(&b.sb1, 4));
    sections.extend(chunks(&b.storage, 9));
    assert_eq!(sections.len(), SECTORS_PER_SLOT);
    for (id, data) in sections.iter().enumerate() {
        let s = &mut out[id * SECTOR_SIZE..(id + 1) * SECTOR_SIZE];
        s.fill(0);
        s[..data.len()].copy_from_slice(data);
        s[FOOTER_ID..FOOTER_ID + 2].copy_from_slice(&(id as u16).to_le_bytes());
        s[FOOTER_CHECKSUM..FOOTER_CHECKSUM + 2].copy_from_slice(&checksum(data).to_le_bytes());
        s[FOOTER_SIGNATURE..FOOTER_SIGNATURE + 4].copy_from_slice(&SECTOR_SIGNATURE.to_le_bytes());
        s[FOOTER_COUNTER..FOOTER_COUNTER + 4].copy_from_slice(&0u32.to_le_bytes());
    }
    out
}

/// The save blocks in a flash image, as the game loads them: the slot
/// whose 14 sectors are all there with good checksums, the newer by save
/// counter if both are. `None` if neither is whole.
pub fn read(flash: &[u8]) -> Option<Blocks> {
    let slot = |n: usize| -> Option<(u32, Blocks)> {
        let mut sections = vec![None; SECTORS_PER_SLOT];
        let mut counter = 0;
        for i in 0..SECTORS_PER_SLOT {
            let s = flash
                .get((n * SECTORS_PER_SLOT + i) * SECTOR_SIZE..)?
                .get(..SECTOR_SIZE)?;
            let word = |at: usize| u32::from_le_bytes(s[at..at + 4].try_into().unwrap());
            let id = u16::from_le_bytes([s[FOOTER_ID], s[FOOTER_ID + 1]]) as usize;
            if word(FOOTER_SIGNATURE) != SECTOR_SIGNATURE || id >= SECTORS_PER_SLOT {
                return None;
            }
            counter = word(FOOTER_COUNTER);
            sections[id] = Some(s);
        }
        let sections: Vec<&[u8]> = sections.into_iter().collect::<Option<_>>()?;
        // Each section's size, as `flash` cuts the blocks.
        let join = |ids: std::ops::Range<usize>, size: usize| -> Option<Vec<u8>> {
            let mut out = Vec::with_capacity(size);
            for id in ids {
                let n = (size - out.len()).min(SECTOR_DATA_SIZE);
                let s = sections[id];
                let sum = u16::from_le_bytes([s[FOOTER_CHECKSUM], s[FOOTER_CHECKSUM + 1]]);
                if checksum(&s[..n]) != sum {
                    return None;
                }
                out.extend_from_slice(&s[..n]);
            }
            Some(out)
        };
        let b = Blocks {
            sb2: join(0..1, SB2_SIZE)?,
            sb1: join(1..5, SB1_SIZE)?,
            storage: join(5..SECTORS_PER_SLOT, STORAGE_SIZE)?,
        };
        Some((counter, b))
    };
    match (slot(0), slot(1)) {
        (Some(a), Some(b)) => Some(if b.0 > a.0 { b.1 } else { a.1 }),
        (a, b) => a.or(b).map(|s| s.1),
    }
}

/// Each party Pokémon's personality and original trainer's ID (the
/// trainer ID low, the secret ID high): who it is and where it came from.
pub fn party(b: &Blocks) -> Vec<(u32, u32)> {
    let n = (b.sb1[SB1_PARTY_COUNT] as usize).min(6);
    let word = |at: usize| u32::from_le_bytes(b.sb1[at..at + 4].try_into().unwrap());
    (0..n)
        .map(|i| (word(SB1_PARTY + 100 * i), word(SB1_PARTY + 100 * i + 4)))
        .collect()
}

/// The location the blocks' player is in (`SaveBlock1.location`).
pub fn location(b: &Blocks) -> Map {
    Map {
        group: b.sb1[SB1_LOCATION],
        num: b.sb1[SB1_LOCATION + 1],
    }
}

/// `gLink` (its `state` is the second byte) and `gLinkStatus`, from the
/// pokeemerald ELF's symbols.
pub const G_LINK: u32 = 0x0300_3170;
pub const G_LINK_STATUS: u32 = 0x0300_30E0;
/// `LINK_STATE_CONN_ESTABLISHED` (`include/link.h`).
pub const CONN_ESTABLISHED: u8 = 4;
/// `gSaveBlock1Ptr`; the map is the first two bytes of its `location`, at
/// 4. `MAP_TRADE_CENTER` is group 25, number 25.
pub const SB1_PTR: u32 = 0x0300_5D8C;
pub const TRADE_CENTER: [u8; 2] = [25, 25];

/// The presses both players make by default: continue (Start, A, A), then
/// Up at the attendant and A every 60 frames from 1400 to 3400: her lines,
/// Trade Center, the save's two Yeses, and (the parent's) the link's
/// confirmation. Both stand in the Trade Center by about frame 3900.
pub const TRADE_CENTER_PRESSES: &str = "start@300,a@900,a@1100,up@1350,a@1400-3400/60";
/// A player's scripted presses, `button@frame[+held]` separated by commas
/// (frames from 0), or `button@from-to/every[+held]` for a press every
/// `every` frames; a press is held 6 frames unless `+held` says.
#[derive(Clone, Default)]
pub struct Presses(Vec<(u16, u64, u64)>);

impl Presses {
    pub fn parse(s: &str) -> Result<Presses, String> {
        use runtime::io::key;
        let mut v = Vec::new();
        for p in s.split(',').filter(|p| !p.is_empty()) {
            let bad = || format!("bad press {p:?}: want button@frame[+held]");
            let (b, at) = p.split_once('@').ok_or_else(bad)?;
            let (at, held) = at.split_once('+').unwrap_or((at, "6"));
            let (at, every) = at.split_once('/').unwrap_or((at, "1"));
            let (from, to) = at.split_once('-').unwrap_or((at, at));
            let k = match b {
                "a" => key::A,
                "b" => key::B,
                "select" => key::SELECT,
                "start" => key::START,
                "right" => key::RIGHT,
                "left" => key::LEFT,
                "up" => key::UP,
                "down" => key::DOWN,
                "r" => key::R,
                "l" => key::L,
                _ => return Err(bad()),
            };
            let num = |n: &str| n.parse::<u64>().map_err(|_| bad());
            let (from, to, every, held) = (num(from)?, num(to)?, num(every)?, num(held)?);
            if every == 0 {
                return Err(bad());
            }
            v.extend((from..=to).step_by(every as usize).map(|at| (k, at, held)));
        }
        Ok(Presses(v))
    }

    pub fn keys(&self, f: u64) -> u16 {
        self.0
            .iter()
            .filter(|&&(_, at, held)| (at..at + held).contains(&f))
            .fold(0, |k, &(b, _, _)| k | b)
    }
}

pub mod story;

#[cfg(test)]
mod tests;
