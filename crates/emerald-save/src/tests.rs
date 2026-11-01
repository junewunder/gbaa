use super::*;

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// A substructure, decrypted, as `GetSubstruct` finds it.
fn substruct(mon: &[u8], kind: usize) -> Vec<u8> {
    let personality = le32(mon, 0);
    let key = personality ^ le32(mon, 4);
    let at = 32 + 12 * SUBSTRUCT_ORDER[(personality % 24) as usize][kind];
    (0..3)
        .flat_map(|i| (le32(mon, at + 4 * i) ^ key).to_le_bytes())
        .collect()
}

fn blank() -> Blocks {
    Blocks {
        sb2: vec![0; SB2_SIZE],
        sb1: vec![0; SB1_SIZE],
        storage: vec![0; STORAGE_SIZE],
    }
}

#[test]
fn text_uses_the_charmap() {
    assert_eq!(text("Ab 9", 6), [0xBB, 0xD6, 0x00, 0xAA, 0xFF, 0xFF]);
}

#[test]
fn experience_matches_the_tables() {
    // gExperienceTables: medium slow at 5, medium fast at 3 and 5.
    assert_eq!(Growth::MediumSlow.exp(5), 135);
    assert_eq!(Growth::MediumFast.exp(3), 27);
    assert_eq!(Growth::MediumFast.exp(5), 125);
}

#[test]
fn stats_follow_the_nature() {
    // Hardy (0): none changes. Mudkip at 5: (2*base + 15) * 5 / 100 + 5.
    let m = Mon {
        species: MUDKIP,
        level: 5,
        personality: 0,
    };
    assert_eq!(stats(&m), [20, 12, 10, 9, 10, 10]);
    // Lonely (1): attack up, defense down.
    let m = Mon {
        personality: 1,
        ..m
    };
    assert_eq!(stats(&m), [20, 13, 9, 9, 10, 10]);
}

#[test]
fn a_mon_decodes_as_the_game_reads_it() {
    for t in trainers() {
        for m in &t.party {
            let b = encode_mon(m, &t);
            let subs: Vec<Vec<u8>> = (0..4).map(|k| substruct(&b, k)).collect();
            let sum = subs
                .iter()
                .flat_map(|s| {
                    s.chunks(2)
                        .map(|w| u16::from_le_bytes([w[0], w[1]]))
                        .collect::<Vec<_>>()
                })
                .fold(0u16, u16::wrapping_add);
            assert_eq!(le16(&b, 28), sum, "checksum");
            assert_eq!(le16(&subs[0], 0), m.species.id);
            assert_eq!(le32(&subs[0], 4), m.species.growth.exp(m.level as u32));
            assert_eq!(le16(&subs[1], 0), m.species.moves[0].0);
            assert_eq!(subs[1][9], m.species.moves[1].1);
            assert_eq!(le16(&subs[3], 2) & 0x7F, m.level as u16);
            assert_eq!(b[84], m.level);
            assert_eq!(le16(&b, 86), le16(&b, 88), "full HP");
            assert_eq!(&b[20..27], &text(t.name, 7)[..]);
            // Not shiny (GET_SHINY_VALUE < 8 would be).
            let v =
                (t.id >> 16) ^ (t.id & 0xFFFF) ^ (m.personality >> 16) ^ (m.personality & 0xFFFF);
            assert!(v >= 8, "{} is shiny", m.species.name);
        }
    }
}

#[test]
fn ribbons_land_in_misc_with_the_checksum_kept() {
    let t = &trainers()[0];
    for m in &t.party {
        let mut b = encode_mon(m, t);
        add_ribbons(&mut b, 0x4924);
        let subs: Vec<Vec<u8>> = (0..4).map(|k| substruct(&b, k)).collect();
        let sum = subs
            .iter()
            .flat_map(|s| s.chunks(2).map(|w| u16::from_le_bytes([w[0], w[1]])))
            .fold(0u16, u16::wrapping_add);
        assert_eq!(le16(&b, 28), sum, "checksum");
        assert_eq!(le32(&subs[3], 8), 0x4924);
        assert_eq!(le16(&subs[0], 0), m.species.id);
    }
}

#[test]
fn a_held_item_lands_in_growth_with_the_checksum_kept() {
    let t = &trainers()[0];
    for m in &t.party {
        let mut b = encode_mon(m, t);
        set_held_item(&mut b, 0x0123);
        let subs: Vec<Vec<u8>> = (0..4).map(|k| substruct(&b, k)).collect();
        let sum = subs
            .iter()
            .flat_map(|s| s.chunks(2).map(|w| u16::from_le_bytes([w[0], w[1]])))
            .fold(0u16, u16::wrapping_add);
        assert_eq!(le16(&b, 28), sum, "checksum");
        assert_eq!(le16(&subs[0], 2), 0x0123);
        assert_eq!(le16(&subs[0], 0), m.species.id);
    }
}

#[test]
fn the_trainers_differ() {
    let [a, b] = trainers();
    assert_ne!(a.name, b.name);
    assert_ne!(a.id, b.id);
    assert!(a.party.len() >= 2 && b.party.len() >= 2);
}

#[test]
fn patch_sets_the_warp_party_and_flags() {
    let t = &trainers()[0];
    let b = patch(&blank(), t);
    assert_eq!(&b.sb2[..8], &text("ALICE", 8)[..]);
    assert_eq!(b.sb2[SB2_SPECIAL_WARP_FLAGS], CONTINUE_GAME_WARP);
    assert_eq!(location(&b), OLDALE_PC_2F);
    assert_eq!(&b.sb1[0x0C..0x14], [2, 3, 0xFF, 0, 10, 0, 3, 0]);
    assert_eq!(le16(&b.sb1, SB1_MAP_LAYOUT_ID), 62);
    assert_eq!(b.sb1[SB1_PARTY_COUNT], 2);
    assert_eq!(
        &b.sb1[SB1_PARTY..SB1_PARTY + 100],
        &encode_mon(&t.party[0], t)[..]
    );
    // FLAG_SYS_POKEMON_GET, 0x860: byte 0x10C, bit 0.
    assert_eq!(b.sb1[SB1_FLAGS + 0x10C] & 1, 1);
    // FLAG_SYS_POKEDEX_GET, 0x861: the cable club's attendants check it.
    assert_eq!(b.sb1[SB1_FLAGS + 0x10C] & 2, 2);
    // Mudkip, national 258: byte 32, bit 1, in all four dex arrays.
    for (blk, off) in [
        (&b.sb2, SB2_DEX_OWNED),
        (&b.sb2, SB2_DEX_SEEN),
        (&b.sb1, SB1_DEX_SEEN1),
        (&b.sb1, SB1_DEX_SEEN2),
    ] {
        assert_eq!(blk[off + 32], 2);
    }
}

#[test]
fn flash_sectors_are_valid() {
    let mut b = blank();
    b.sb1[SB1_SIZE - 1] = 7;
    b.storage[5] = 9;
    let f = flash(&b);
    assert_eq!(f.len(), SAVE_SIZE);
    let sizes = [SB2_SIZE, 3968, 3968, 3968, SB1_SIZE - 3 * 3968];
    for id in 0..SECTORS_PER_SLOT {
        let s = &f[id * SECTOR_SIZE..(id + 1) * SECTOR_SIZE];
        let size = sizes.get(id).copied().unwrap_or(if id < 13 {
            3968
        } else {
            STORAGE_SIZE - 8 * 3968
        });
        assert_eq!(le16(s, FOOTER_ID) as usize, id);
        assert_eq!(le32(s, FOOTER_SIGNATURE), SECTOR_SIGNATURE);
        assert_eq!(le32(s, FOOTER_COUNTER), 0);
        assert_eq!(
            le16(s, FOOTER_CHECKSUM),
            checksum(&s[..size]),
            "sector {id}"
        );
    }
    assert_eq!(f[4 * SECTOR_SIZE + (SB1_SIZE - 1 - 3 * 3968)], 7);
    assert_eq!(f[5 * SECTOR_SIZE + 5], 9);
    assert!(
        f[SECTORS_PER_SLOT * SECTOR_SIZE..]
            .iter()
            .all(|&x| x == 0xFF)
    );
}

#[test]
fn checksum_folds_the_halves() {
    assert_eq!(checksum(&[0x01, 0x00, 0x02, 0x00]), 3);
    assert_eq!(checksum(&0x0001_FFFFu32.to_le_bytes()), 0x0000);
}

#[test]
fn read_finds_what_flash_wrote() {
    let mut base = blank();
    base.sb2[100] = 7;
    base.storage[STORAGE_SIZE - 1] = 9;
    let [alice, _] = trainers();
    let b = patch(&base, &alice);
    let image = flash(&b);
    assert_eq!(read(&image), Some(b.clone()));
    assert_eq!(
        party(&b),
        vec![(0x1A2B_3C4D, alice.id), (0x5E6F_7081, alice.id)]
    );
    // A newer slot 2 wins; a bad checksum there falls back to slot 1.
    let mut newer = b.clone();
    newer.sb1[SB1_PARTY_COUNT] = 1;
    let mut two = image.clone();
    let slot2 = flash(&newer);
    two[SECTORS_PER_SLOT * SECTOR_SIZE..2 * SECTORS_PER_SLOT * SECTOR_SIZE]
        .copy_from_slice(&slot2[..SECTORS_PER_SLOT * SECTOR_SIZE]);
    for i in 0..SECTORS_PER_SLOT {
        let at = (SECTORS_PER_SLOT + i) * SECTOR_SIZE + FOOTER_COUNTER;
        two[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
    }
    assert_eq!(read(&two), Some(newer));
    two[SECTORS_PER_SLOT * SECTOR_SIZE + 5] ^= 1;
    assert_eq!(read(&two), Some(b));
    assert_eq!(read(&[0xFF; SAVE_SIZE]), None);
}
