use std::collections::HashMap;

use crate::m4a::{self, Cmd, Mem};
use crate::midi::{self, Smf};
use crate::{mplay, pret, songs};

#[test]
fn midi_round_trips() {
    let smf = Smf {
        ticks_per_beat: 24,
        tracks: vec![vec![
            (0, vec![0x90, 60, 100]),
            (200, vec![0x80, 60, 0]),
            (200, midi::meta(midi::END_OF_TRACK, &[])),
        ]],
    };
    assert_eq!(Smf::parse(&smf.to_bytes()).unwrap(), smf);
}

#[test]
fn decodes_running_status_patterns_and_loops() {
    // VOICE 1; N12 Cn3 v100; W12; (N12) Dn3; W12; PATT p; GOTO 2; FINE
    // p: N24 En3; W24; PEND
    let base = 0x0800_0000;
    let mut b = vec![0xBD, 1, 0xDB, 60, 100, 0x8C, 62, 0x8C, 0xB3];
    b.extend((base + 20u32).to_le_bytes());
    b.push(0xB2);
    b.extend((base + 2u32).to_le_bytes());
    b.push(0xB1);
    b.extend([0, 0, 0, 0]);
    b.truncate(20);
    b.extend([0xE7, 64, 0x98, 0xB4]);
    let t = m4a::decode_track(Mem { base, bytes: &b }, base).unwrap();
    let want = vec![
        (0, Cmd::Voice(1)),
        (
            0,
            Cmd::Note {
                key: 60,
                vel: 100,
                len: 12,
            },
        ),
        (
            12,
            Cmd::Note {
                key: 62,
                vel: 100,
                len: 12,
            },
        ),
        (
            24,
            Cmd::Note {
                key: 64,
                vel: 100,
                len: 24,
            },
        ),
    ];
    assert_eq!(t.events, want);
    assert_eq!(t.loop_at, Some((0, 48)));
    assert_eq!(t.patts, vec![24]);
    assert_eq!(t.spans, vec![(24, 48)]);
    assert_eq!(t.end, 48);
}

#[test]
fn assembles_mid2agb_text() {
    let mut defs = HashMap::new();
    mplay::equs(
        "\t.equ\tVOL,\t0xbe\n\t.equ\tmxv,\t0x7F\n\t.equ\tc_v,\t0x40\n\t.equ\tPAN,0xbf\n\t.equ\tGOTO,0xb2\n",
        &mut defs,
    );
    let src = "\t.equ\ts_mvl, 80\n\t.equ\ts_grp, voicegroup_x\ns_1:\n\t.byte\t\tVOL   , 127*s_mvl/mxv\n\t.byte\t\tPAN   , c_v-5 @ left\n\t.byte\tGOTO\n\t .word\ts_1\n\t.align\t2\ns:\n\t.word\ts_grp\n";
    let a = mplay::assemble(src, &defs).unwrap();
    assert_eq!(
        a.bytes,
        [0xBE, 80, 0xBF, 59, 0xB2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(a.labels["s"], 12);
    assert_eq!(a.relocs, [5]);
    assert_eq!(a.externs, [(12, "voicegroup_x".to_string())]);
}

#[test]
fn vol_scales_back_through_master_volume() {
    let smf = Smf {
        ticks_per_beat: 24,
        tracks: vec![vec![
            (0, midi::meta(midi::TEXT, songs::AUTO_VOL.as_bytes())),
            (0, vec![0xB0, 7, 80]),
            (0, vec![0xB0, 7, 50]),
        ]],
    };
    let out = pret::for_mid2agb(&smf, 80);
    // The marked VOL is mid2agb's own; 80*80/127 is the least that gives 50.
    assert_eq!(out.tracks[0], vec![(0, vec![0xB0, 7, 80])]);
}

#[test]
fn reads_pret_tables() {
    let t = pret::song_table(
        "gSongTable::\n\tsong mus_dummy, MUSIC_PLAYER_BGM, 0\n\tsong se_a, 1, 1\n",
    );
    assert_eq!(t, ["mus_dummy", "se_a"]);
    let c = pret::midi_cfg("se_a.mid:   -E -R50 -V080\n");
    assert_eq!(c["se_a"], ["-E", "-R50", "-V080"]);
}

#[test]
fn finds_bar_lines_from_wait_splits() {
    // W24 W24 where mid2agb would write W48 is a bar line at 24; W03 W03
    // W03 W03 where it would write W12 is a bar every 3 clocks.
    let song = |waits: &[u8]| {
        let mut b = vec![0xD8, 60, 100];
        b.extend(waits.iter().map(|&w| 0x80 + w));
        b.push(0xB1);
        let header = m4a::Header {
            addr: 0,
            block_count: 0,
            priority: 0,
            reverb: 0,
            tone: 0,
            tracks: vec![0],
        };
        m4a::Song {
            header,
            tracks: vec![m4a::decode_track(Mem { base: 0, bytes: &b }, 0).unwrap()],
        }
    };
    assert!(songs::bars(&song(&[24, 24])).has(24));
    assert_eq!(songs::bars(&song(&[3, 3, 3, 3])).segs, [(0, 3)]);
    // W12 W24 W96 W96 where 4/4 from 0 would write W96 W96 W36: bar lines
    // restarted at 12 and again at 36 (pret's mus_littleroot). (0x80 + 48
    // is W96.)
    let b = songs::bars(&song(&[12, 24, 48, 48]));
    let lines: Vec<u32> = (1..228).filter(|&c| b.has(c)).collect();
    assert_eq!(lines, [12, 36, 132], "{b:?}");
}

#[test]
fn marks_a_split_no_bars_explain() {
    // N06 W04 W12, where mid2agb would write W16: a split marker at 4,
    // as a second track's W96 W96 leaves no place to restart bars at 4.
    let b = [
        0xD6, 60, 100, 0x84, 0x8C, 0xB1, 0xD6, 60, 100, 0xB0, 0xB0, 0xB1,
    ];
    let song = m4a::Song {
        header: m4a::Header {
            addr: 0,
            block_count: 0,
            priority: 0,
            reverb: 0,
            tone: 0,
            tracks: vec![0, 6],
        },
        tracks: [0, 6]
            .map(|a| m4a::decode_track(Mem { base: 0, bytes: &b }, a).unwrap())
            .to_vec(),
    };
    let bars = songs::bars(&song);
    assert_eq!(songs::marks(&song, &bars), [vec![4], vec![]], "{bars:?}");
}

#[test]
fn finds_the_patterns_mid2agb_makes() {
    // Two bars of four quarter notes: the second is a PATT of the first,
    // unless a marker with a value of its own keeps it apart.
    let smf = |mark: Option<u8>| {
        let mut t = Vec::new();
        for bar in 0..2u32 {
            if bar == 1
                && let Some(v) = mark
            {
                t.push((96, vec![0xB0, songs::MARK_CC, v]));
            }
            for (k, key) in [60u8, 62, 64, 65].into_iter().enumerate() {
                let at = bar * 96 + 24 * k as u32;
                t.push((at, vec![0x90, key, 100]));
                t.push((at + 24, vec![0x80, key, 0]));
            }
        }
        t.sort_by_key(|e| e.0);
        t.push((192, midi::meta(midi::END_OF_TRACK, &[])));
        Smf {
            ticks_per_beat: 24,
            tracks: vec![vec![(0, midi::meta(midi::END_OF_TRACK, &[]))], t],
        }
    };
    assert_eq!(crate::mid2agb::patterns(&smf(None), 1, true), [(96, 0)]);
    assert_eq!(crate::mid2agb::patterns(&smf(Some(1)), 1, true), []);
}

#[test]
fn load_log_check_finds_blobs_ranges_and_uses() {
    use crate::loadlog::{Symbols, check};
    let loads = "# runs\nframe\top\tsrc\tsrc_len\tdst\tdst_len\truns\n\
        5\tlz77\t0x08001000\t40\t0x02000000\t64\t-\n\
        6\tdma\t0x08002000\t32\t0x06000000\t32\t-\n\
        7\tdma\t0x02000100\t64\t0x06004000\t64\t0x08003000/0/8/cpu,0x08003010/0/4/cpu\n";
    let reached = "# r\nkind\tstart\tbytes\npacked\t0x08001000\t\n\
        copied\t0x08002000\t16\ncopied\t0x08003000\t8\ncopied\t0x08003010\t4\n";
    let head = "layer\tbpp\tsize\tpart\ttile_src\ttile_via\ttile_off\tblob\tblob_off\tbank\tpal_src\tpal_via\tpal_off";
    let row = "bg0\t4\t1x1\t0\t0x08001000\tlz77+dma\t32\t0x08001000\t32\t1\t-\t-\t-";
    let uses = format!("# u\n{head}\tfirst\tlast\tframes\n{row}\t3\t9\t7\n");
    let shown = format!("# every tile shown on frame 9\nvram\t{head}\n0x06000020\t{row}\n");
    let mut syms = Symbols::new();
    syms.insert(0x0800_1000, ("gBlob".into(), Some(40)));
    let r = check(loads, reached, &uses, &shown, &syms).unwrap();
    assert!(r.pass, "{}", r.text);
    // A pret size the log's length doesn't reach, a range byte not loaded,
    // and a tile whose use ended before the last frame: each fails.
    syms.insert(0x0800_1000, ("gBlob".into(), Some(44)));
    assert!(!check(loads, reached, &uses, &shown, &syms).unwrap().pass);
    syms.insert(0x0800_1000, ("gBlob".into(), Some(40)));
    let more = format!("{reached}copied\t0x08004000\t2\n");
    assert!(!check(loads, &more, &uses, &shown, &syms).unwrap().pass);
    let old = format!("# u\n{head}\tfirst\tlast\tframes\n{row}\t3\t8\t6\n");
    assert!(!check(loads, reached, &old, &shown, &syms).unwrap().pass);
}

#[test]
fn lz77_and_rl_decode_the_bios_formats() {
    use crate::graphics::{lz77, rl};
    // 8 bytes: "ab" literal, then a back-reference of 6 at distance 2.
    let lz = [0x10, 8, 0, 0, 0b0010_0000, b'a', b'b', 0x30, 0x01];
    assert_eq!(lz77(&lz), Some((b"abababab".to_vec(), 9)));
    // 7 bytes: a run of 4 'x', then 3 literals.
    let r = [0x30, 7, 0, 0, 0x81, b'x', 0x02, b'a', b'b', b'c'];
    assert_eq!(rl(&r), Some((b"xxxxabc".to_vec(), 10)));
    assert_eq!(lz77(&r), None);
}

#[test]
fn palettes_and_sheets_round_trip() {
    use crate::graphics::{indexed_png, jasc, read_indexed_png, read_jasc, tile_pixels};
    let cs: Vec<u16> = (0..16).map(|i| i * 0x0421 + (i & 1) * 0x1000).collect();
    assert_eq!(read_jasc(&jasc(&cs)), Some(cs.clone()));
    // Two 4bpp tiles side by side: the low nibble is the left pixel.
    let mut data = vec![0u8; 64];
    data[0] = 0x21;
    data[32] = 0xF3;
    let (w, h, px) = tile_pixels(&data, 4, 2);
    assert_eq!(
        (w, h, &px[..2], &px[8..10]),
        (16, 8, &[1, 2][..], &[3, 15][..])
    );
    let png = indexed_png(w, h, 4, &px, &cs).unwrap();
    let (w2, h2, px2, pal) = read_indexed_png(&png).unwrap();
    assert_eq!((w2, h2, px2, pal), (w, h, px, cs));
}

#[test]
fn incgfx_calls_are_joined_by_symbol() {
    use crate::pretgfx::parse_incgfx;
    let c = "static const u32 sGfx[] = INCGFX_U32(\"graphics/a.png\", \".4bpp.lz\");\n\
        const u16 gPal[] = INCGFX_U16( \"graphics/a.pal\" , \".gbapal\" );\n\
        const u32 gPics[] = INCGFX_U32(\"graphics/b.png\", \".4bpp\", \"-mwidth 2 -mheight 4\");";
    let g = parse_incgfx(c, "src/x.c");
    let got: Vec<(&str, &str, &str, &str)> = g
        .iter()
        .map(|g| {
            (
                g.symbol.as_str(),
                g.source.as_str(),
                g.ext.as_str(),
                g.args.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("sGfx", "graphics/a.png", ".4bpp.lz", ""),
            ("gPal", "graphics/a.pal", ".gbapal", ""),
            ("gPics", "graphics/b.png", ".4bpp", "-mwidth 2 -mheight 4"),
        ]
    );
}

#[test]
fn blobs_are_classed_by_use() {
    use crate::graphics::{Kind, find, parse_loads, parse_uses};
    // ROM: 0x100 a palette, 0x200 two tiles, 0x300 a packed tilemap.
    let mut rom = vec![0u8; 0x400];
    rom[0x300..0x309].copy_from_slice(&[0x10, 8, 0, 0, 0b0010_0000, 1, 0, 0x30, 0x01]);
    let loads = parse_loads(
        "# runs\nframe\top\tsrc\tsrc_len\tdst\tdst_len\truns\n\
         1\tcpuset\t0x08000100\t32\t0x05000000\t32\t-\n\
         1\tdma\t0x08000200\t64\t0x06000000\t64\t-\n\
         1\tlz77\t0x08000300\t12\t0x02000000\t8\t-\n\
         2\tcpuset\t0x02000000\t8\t0x06004000\t8\t0x08000300/0/8/lz77\n\
         3\tdma\t0x08000000\t16\t0x03000000\t16\t-\n",
    )
    .unwrap();
    let uses = parse_uses(
        "# u\nlayer\tbpp\tsize\tpart\ttile_src\ttile_via\ttile_off\tblob\tblob_off\tbank\tpal_src\tpal_via\tpal_off\tfirst\tlast\tframes\n\
         bg0\t4\t1x1\t0\t0x08000220\tdma\t0\t0x08000200\t32\t2\t0x08000100\tcpuset\t0\t2\t9\t8\n",
    )
    .unwrap();
    let f = find(&rom, &[(loads, uses)]);
    let kinds: Vec<(u32, Kind)> = f.blobs.values().map(|b| (b.addr, b.kind)).collect();
    assert_eq!(
        kinds,
        [
            (0x0800_0100, Kind::Palette),
            (0x0800_0200, Kind::Tiles),
            (0x0800_0300, Kind::Tilemap)
        ]
    );
    assert_eq!(f.blobs[&0x0800_0300].data, [1, 0, 1, 0, 1, 0, 1, 0]);
    assert_eq!(f.tiles[&0x0800_0200].palette(), Some((0x0800_0100, 0)));
}

#[test]
fn tilemaps_over_tiles_compare_frames_within_one_route() {
    use crate::graphics::{Kind, find, parse_loads, parse_uses};
    // Route A fills BG VRAM with tiles at frame 100 and writes a blob over
    // them at 130 (a tileset animation: tiles). Route B writes another blob
    // there at its own frame 120: a tilemap, though 120 is near A's 100.
    let rom = vec![0u8; 0x400];
    let head = "# runs\nframe\top\tsrc\tsrc_len\tdst\tdst_len\truns\n";
    let a = parse_loads(&format!(
        "{head}100\tdma\t0x08000200\t64\t0x06000000\t64\t-\n\
         130\tdma\t0x08000340\t32\t0x06000000\t32\t-\n"
    ))
    .unwrap();
    let b = parse_loads(&format!(
        "{head}120\tdma\t0x08000300\t32\t0x06000000\t32\t-\n"
    ))
    .unwrap();
    let uses = parse_uses(
        "# u\nlayer\tbpp\tsize\tpart\ttile_src\ttile_via\ttile_off\tblob\tblob_off\tbank\tpal_src\tpal_via\tpal_off\tfirst\tlast\tframes\n\
         bg0\t4\t1x1\t0\t0x08000200\tdma\t0\t0x08000200\t0\t0\t-\t-\t0\t100\t110\t10\n",
    )
    .unwrap();
    let f = find(&rom, &[(a, uses), (b, Vec::new())]);
    let kinds: Vec<(u32, Kind)> = f.blobs.values().map(|b| (b.addr, b.kind)).collect();
    assert_eq!(
        kinds,
        [
            (0x0800_0200, Kind::Tiles),
            (0x0800_0300, Kind::Tilemap),
            (0x0800_0340, Kind::Tiles)
        ]
    );
}

#[test]
fn manifest_merge_keeps_other_sections() {
    use crate::graphics::merge_manifest;
    let old = "# Written by `gbaa-assets songs` (crates/assets). Don't edit.\n\n[manifest]\nformat = 1\n\n[tiles]\n0x1 = {}\n\n[songs]\n0x2 = {}\n";
    let m = merge_manifest(old, &["tiles"], "[tiles]\n0x3 = {}\n", "[manifest]\n");
    assert_eq!(
        m,
        "[manifest]\nformat = 1\n\n[songs]\n0x2 = {}\n\n[tiles]\n0x3 = {}\n"
    );
}

/// RIFF sub-chunks from `at` to `end`: id, body offset, length.
fn riff_chunks(b: &[u8], mut at: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    while at + 8 <= end {
        let id = b[at..at + 4].try_into().unwrap();
        let n = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
        out.push((id, at + 8, n));
        at += 8 + n + (n & 1);
    }
    out
}

#[test]
fn dpcm_decodes_high_nibble_first() {
    use crate::samples::{decode_dpcm, dpcm_len};
    // Start 5, steps 0, +1, +4, +9.
    assert_eq!(decode_dpcm(&[5, 0x01, 0x23], 4), vec![5, 6, 10, 19]);
    // The second block starts afresh.
    let mut b = vec![0u8; 33];
    b.extend([0xF8, 0x00]);
    assert_eq!(decode_dpcm(&b, 66)[64..], [-8, -8]);
    // wav2agb --no-pad: a starting byte, then a byte per two steps.
    assert_eq!(dpcm_len(64), 33);
    assert_eq!(dpcm_len(65), 34);
    assert_eq!(dpcm_len(66), 35);
    assert_eq!(dpcm_len(67), 35);
}

#[test]
fn wave_writes_wav2agb_chunks() {
    use crate::samples::Wave;
    let w = Wave {
        addr: 0x0800_0000,
        dpcm: false,
        looped: true,
        pitch: 13379 * 1024 + 7,
        loop_start: 2,
        size: 4,
        samples: vec![0, 1, -1, 127, -128],
    };
    let b = w.to_wav();
    assert_eq!(&b[..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize,
        b.len() - 8
    );
    let c = riff_chunks(&b, 12, b.len());
    let get = |id: &[u8; 4]| {
        let &(_, at, n) = c.iter().find(|x| &x.0 == id).unwrap();
        &b[at..at + n]
    };
    assert_eq!(get(b"data"), [0x80, 0x81, 0x7F, 0xFF, 0x00]);
    assert_eq!(get(b"agbp"), (13379 * 1024 + 7u32).to_le_bytes());
    assert_eq!(get(b"agbl"), 4u32.to_le_bytes());
    let smpl = get(b"smpl");
    // One loop, from 2 to the last sample (inclusive).
    assert_eq!(smpl[28..32], 1u32.to_le_bytes());
    assert_eq!(smpl[44..48], 2u32.to_le_bytes());
    assert_eq!(smpl[48..52], 4u32.to_le_bytes());
}

#[test]
fn sf2_records_line_up() {
    use crate::sf2::{self, Preset, Sample, Zone};
    let s = Sample {
        name: "s".into(),
        data: vec![0, 100, -100],
        rate: 8000,
        root: 60,
        correction: 0,
        looped: Some((1, 3)),
    };
    let zone = |keys| Zone {
        keys,
        sample: 0,
        gens: vec![(sf2::op::SAMPLE_MODES, 1)],
    };
    let p = Preset {
        name: "p".into(),
        bank: 0,
        program: 3,
        zones: vec![zone((0, 59)), zone((60, 127))],
    };
    let b = sf2::write("t", &[s], &[p]);
    assert_eq!(
        u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize,
        b.len() - 8
    );
    let mut sub = HashMap::new();
    for (_, at, n) in riff_chunks(&b, 12, b.len()) {
        for (id, at2, n2) in riff_chunks(&b, at + 4, at + n) {
            sub.insert(id, (at2, n2));
        }
    }
    let n = |id: &[u8; 4], size| {
        assert_eq!(sub[id].1 % size, 0, "{}", String::from_utf8_lossy(id));
        sub[id].1 / size
    };
    // One preset and its terminal, one bag each, two instrument zones.
    assert_eq!(n(b"phdr", 38), 2);
    assert_eq!(n(b"pbag", 4), 2);
    assert_eq!(n(b"inst", 22), 2);
    assert_eq!(n(b"ibag", 4), 3);
    // Each zone: key range, sample modes, sample: 6 and a terminal.
    assert_eq!(n(b"igen", 4), 7);
    assert_eq!(n(b"shdr", 46), 2);
    // The sample's loop, in the data's points (3 and 46 zeros).
    assert_eq!(sub[b"smpl"].1, (3 + 46) * 2);
    let (at, _) = sub[b"shdr"];
    let w = |i: usize| u32::from_le_bytes(b[at + 20 + 4 * i..at + 24 + 4 * i].try_into().unwrap());
    assert_eq!([w(0), w(1), w(2), w(3), w(4)], [0, 3, 1, 3, 8000]);
}

/// A walking strip DMA'd a frame at a time into one slot (Emerald's object
/// events): its frames joined into the source map's symbol, laid across,
/// its loop found through a lag frame, and an Aseprite file that reads
/// back.
#[test]
fn sprite_strip_from_refilled_slot() {
    use crate::graphics::{Blob, Found, Kind, Store, TileInfo};
    use crate::sprites;
    let base = 0x0800_0100u32;
    let fb = 2 * 4 * 32;
    let mut rom = vec![0u8; 0x100 + 9 * fb as usize];
    for (i, b) in rom[0x100..].iter_mut().enumerate() {
        *b = (i / fb as usize + 1) as u8 * 0x11;
    }
    // Export saw two frames' copies as blobs of their own.
    let mut found = Found::default();
    for f in [0u32, 3] {
        let a = base + f * fb;
        let at = (a - 0x0800_0000) as usize;
        found.blobs.insert(
            a,
            Blob {
                addr: a,
                store: Store::Raw,
                size: fb,
                data: rom[at..at + fb as usize].to_vec(),
                kind: Kind::Tiles,
                first_frame: 10,
                vram: None,
            },
        );
        found.tiles.insert(
            a,
            TileInfo {
                bpp: 4,
                obj: true,
                ..Default::default()
            },
        );
    }
    // Walking in place: 3, 0, 4, 0 eight frames each, one held nine.
    let mut text = String::from(
        "# oam\nframe\tindex\tx\ty\tsize\tbpp\tmapping\ttile\tbank\tflip\ttile_src\ttile_via\ttile_off\tblob\tblob_off\tpal_src\tpal_via\tpal_off\n",
    );
    let mut frame = 10;
    for (k, f) in [3u32, 0, 4, 0].repeat(4).into_iter().enumerate() {
        let a = base + f * fb;
        text.push_str(&format!(
            "{frame}\t5\t100\t60\t2x4\t4\t1d\t32\t0\t-\t{a:#010x}\tdma\t0\t{a:#010x}\t0\t-\t-\t-\n"
        ));
        frame += if k == 6 { 9 } else { 8 };
    }
    let lines = sprites::parse_oam(&text).unwrap();
    let sizes = [(base, 9 * fb as u64)].into_iter().collect();
    let sp = sprites::find(&rom, &mut found, &[(lines, frame + 20)], &sizes);
    let sh = &sp.sheets[&base];
    assert_eq!((sh.w, sh.h, sh.frames, sh.across), (2, 4, 9, true));
    assert_eq!(found.blobs[&base].size, 9 * fb);
    assert!(!found.blobs.contains_key(&(base + 3 * fb)));
    assert_eq!(sh.anims.len(), 1);
    let steps: Vec<(u32, u64)> = sh.anims[0].steps.iter().map(|s| (s.0.0, s.1)).collect();
    assert_eq!(steps, [(0, 8), (3, 8), (0, 8), (4, 8)]);
    assert!(sh.anims[0].looped && !sh.anims[0].partial);
    // Frames side by side: frame k's pixels are k + 1.
    let l = found.layouts[&base];
    let (w, h, px) = sprites::sheet_pixels(&found.blobs[&base].data, 4, l);
    assert_eq!((w, h), (144, 32));
    assert_eq!((px[0], px[16 * 3], px[143]), (1, 4, 9));
    let dir = std::env::temp_dir().join(format!("gbaa-sprites-test-{}", std::process::id()));
    let names = [(base, "walking".to_string())].into_iter().collect();
    let wr = sprites::write(&found, &sp, &names, &dir).unwrap();
    assert_eq!((wr.sheets, wr.anims), (1, 1));
    let bytes = std::fs::read(dir.join("sprites/walking.aseprite")).unwrap();
    let f = asefile::AsepriteFile::read(&bytes[..]).unwrap();
    assert_eq!((f.num_frames(), f.num_tags()), (13, 2));
    assert_eq!(f.frame(9).duration(), 134);
    let _ = std::fs::remove_dir_all(&dir);
}

mod overrides {
    use crate::graphics::{self, Store};
    use crate::overrides::{self, Entry, Kind};
    use crate::sprites::{self, Layout, Sheet};

    /// Tile bytes with every nibble different from its neighbours'.
    fn tiles(n: usize, bpp: u8) -> Vec<u8> {
        (0..n * 8 * bpp as usize)
            .map(|i| (i * 7 + i / 32) as u8)
            .collect()
    }

    fn entry(len: usize, bpp: u8, width: usize, layout: Option<Layout>) -> Entry {
        Entry {
            addr: 0x0800_0100,
            kind: Kind::Tiles,
            store: Store::Raw,
            size: len as u32,
            bpp,
            width,
            layout,
            frames: None,
            frame: None,
            composed: false,
        }
    }

    /// A sheet's exported PNG encodes back to its bytes, laid out plain,
    /// a frame down or a frame across, 4 or 8 bpp; an edited pixel
    /// changes only its own nibble.
    #[test]
    fn png_sheets_encode_back() {
        for (bpp, n, layout) in [
            (4, 20, None),
            (8, 6, None),
            (
                4,
                24,
                Some(Layout {
                    w: 2,
                    h: 4,
                    across: true,
                }),
            ),
            (
                4,
                8,
                Some(Layout {
                    w: 2,
                    h: 2,
                    across: false,
                }),
            ),
        ] {
            let data = tiles(n, bpp);
            let w = graphics::sheet_width(n);
            let (pw, ph, mut px) = match layout {
                Some(l) => sprites::sheet_pixels(&data, bpp, l),
                None => graphics::tile_pixels(&data, bpp, w),
            };
            let e = entry(data.len(), bpp, pw / 8, layout);
            let pal = graphics::grey(bpp);
            let png = graphics::indexed_png(pw, ph, bpp, &px, &pal).unwrap();
            assert_eq!(overrides::from_png(&png, &e, &data).unwrap(), data);
            px[0] ^= 1;
            let png = graphics::indexed_png(pw, ph, bpp, &px, &pal).unwrap();
            let got = overrides::from_png(&png, &e, &data).unwrap();
            let diff: Vec<usize> = (0..data.len()).filter(|&i| got[i] != data[i]).collect();
            assert_eq!(diff, [0], "{bpp} bpp, {layout:?}");
            assert_eq!(got[0] ^ data[0], 1);
            // A PNG of another size is refused.
            let png =
                graphics::indexed_png(pw + 8, ph, bpp, &vec![0; (pw + 8) * ph], &pal).unwrap();
            assert!(overrides::from_png(&png, &e, &data).is_err());
        }
    }

    /// A sprite's exported Aseprite file encodes back to its sheet's bytes.
    #[test]
    fn aseprite_sprites_encode_back() {
        let data = tiles(3 * 8, 4);
        let sh = Sheet {
            addr: 0x0800_0100,
            w: 2,
            h: 4,
            bpp: 4,
            one_d: true,
            across: true,
            frames: 3,
            shown: Default::default(),
            bank: 0,
            palette: None,
            anims: vec![],
            composed: None,
        };
        let file = sprites::aseprite(&data, &sh, &graphics::grey(4)).unwrap();
        let mut e = entry(
            data.len(),
            4,
            6,
            Some(Layout {
                w: 2,
                h: 4,
                across: true,
            }),
        );
        e.frames = Some(3);
        e.frame = Some((2, 4));
        assert_eq!(overrides::from_aseprite(&file, &e, &data).unwrap(), data);
        e.frame = Some((4, 2));
        assert!(overrides::from_aseprite(&file, &e, &data).is_err());
    }

    /// A composed sprite's Aseprite file: a canvas holding its
    /// arrangement, round-tripped, encoded back to its sheet's bytes, and
    /// an edit to a flipped part of the arrangement lands in the frame it
    /// shows, unflipped.
    #[test]
    fn composed_sprites_encode_back() {
        use crate::sprites::{Composed, Part};
        let data = tiles(3 * 4, 4);
        let part = |x, frame, hflip| Part {
            x,
            y: 0,
            frame,
            hflip,
            vflip: false,
        };
        let sh = Sheet {
            addr: 0x0800_0100,
            w: 2,
            h: 2,
            bpp: 4,
            one_d: true,
            across: false,
            frames: 3,
            shown: Default::default(),
            bank: 0,
            palette: None,
            anims: vec![],
            composed: Some(Composed {
                w: 48,
                h: 16,
                arrangements: vec![(vec![part(0, 2, false), part(32, 1, true)], 30, (0, 5))],
                grid: None,
            }),
        };
        let pal = graphics::grey(4);
        let file = sprites::aseprite(&data, &sh, &pal).unwrap();
        sprites::round_trip(&file, &data, &sh, &pal).unwrap();
        let mut e = entry(data.len(), 4, 16, None);
        e.frames = Some(3);
        e.frame = Some((2, 2));
        assert!(overrides::from_aseprite(&file, &e, &data).is_err());
        e.composed = true;
        assert_eq!(overrides::from_aseprite(&file, &e, &data).unwrap(), data);
        // Recolour the arrangement's second part's top-left pixel: frame 1
        // shown flipped, so its top-right pixel.
        let mut a = aseprite::AsepriteFile::from_reader(&file[..]).unwrap();
        let layer = a.layer_ref(2).unwrap();
        let cel = a.cel(layer, 3).unwrap().clone();
        let aseprite::CelKind::Compressed {
            mut pixels, x, y, ..
        } = cel.kind
        else {
            panic!("a part's cel is pixels");
        };
        pixels.data[0] ^= 1;
        let ud = cel.user_data.clone().unwrap();
        a.set_cel(layer, 3, pixels, x, y).unwrap();
        a.set_cel_user_data(layer, 3, ud);
        let mut edited = Vec::new();
        a.write_to(&mut edited).unwrap();
        let got = overrides::from_aseprite(&edited, &e, &data).unwrap();
        let diff: Vec<usize> = (0..data.len()).filter(|&i| got[i] != data[i]).collect();
        // Frame 1 (bytes 128..256), tile 1 (its top right), row 0's last
        // pixel: the high nibble of byte 3.
        assert_eq!(diff, [128 + 32 + 3]);
    }

    /// Palettes, and the ROM's bytes an override replaces: an LZ77 blob
    /// decoded to its header's size.
    #[test]
    fn palettes_and_originals() {
        let bytes: Vec<u8> = [0x7FFFu16, 0x001F, 0x03E0, 0x1234]
            .iter()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        let text = graphics::jasc(&graphics::colours(&bytes));
        assert_eq!(overrides::from_pal(&text, &bytes).unwrap(), bytes);
        assert!(overrides::from_pal(&text, &bytes[..6]).is_err());
        let mut rom = vec![0u8; 0x100];
        rom.extend([0x10, 10, 0, 0, 0x10, b'A', b'B', b'C', 0x30, 0x02, b'X']);
        let mut e = entry(12, 4, 16, None);
        e.store = Store::Lz77;
        assert_eq!(overrides::original(&rom, &e).unwrap(), b"ABCABCABCX");
    }

    #[test]
    fn frames_check_compares_runs() {
        let run = |hs: &[&str]| -> String {
            hs.iter()
                .enumerate()
                .map(|(i, h)| format!("frame {} hash {h}\n", i + 1))
                .collect()
        };
        let plain = run(&["a", "b", "c"]);
        assert!(overrides::check_frames(&plain, &plain, &run(&["a", "x", "c"]), &[2]).is_ok());
        assert!(overrides::check_frames(&plain, &plain, &run(&["a", "x", "c"]), &[3]).is_err());
        assert!(overrides::check_frames(&plain, &run(&["a", "b", "y"]), &plain, &[]).is_err());
    }
}

/// A one-animation sprite table: a frame of two 8×8 pieces (the second
/// flipped), a script loading its tiles, setting a hitbox and looping.
#[test]
fn sprite_tables_compose_frames_and_hitboxes() {
    use crate::spritetables as st;
    let mut rom = vec![0u8; 0x800];
    let mut put =
        |a: u32, v: u32| rom[(a - 0x0800_0000) as usize..][..4].copy_from_slice(&v.to_le_bytes());
    for (i, v) in [0x100, 0x200, 0x300, 0x400, 0x500, 0x500]
        .iter()
        .enumerate()
    {
        put(0x0800_0000 + 4 * i as u32, 0x0800_0000 + v);
    }
    // GetTiles 0 ×2; AnimCmd_6 slot 0 (-2,-3)-(4,5); show 0 for 4; show 0 for 2; JumpBack 4.
    let script = [
        -1i32,
        0,
        2,
        -6,
        0,
        i32::from_le_bytes([-2i8 as u8, -3i8 as u8, 4, 5]),
        4,
        0,
        2,
        0,
        -3,
        4,
    ];
    for (i, w) in script.iter().enumerate() {
        put(0x0800_0020 + 4 * i as u32, *w as u32);
    }
    put(0x0800_0080, 0x0800_0020);
    put(0x0800_0100, 0x0800_0080);
    put(0x0800_0200, 0x0800_0600);
    put(0x0800_0300, 0x0800_0700);
    // Frame 0: piece 0, 2 pieces, 16×8, pivot (8, 4).
    put(0x0800_0600, 2 << 16);
    put(0x0800_0604, 16 | 8 << 16);
    put(0x0800_0608, 8 | 4 << 16);
    // Pieces: 8×8 at (0, 0) tile 0; 8×8 at (8, 0) hflipped, tile 1.
    put(0x0800_0700, 0);
    put(0x0800_0704, 0);
    put(0x0800_0708, (8 | 0x1000) | 1 << 16);
    rom[0x500..0x520].fill(0x11);
    rom[0x520] = 0x02;
    let t = st::tables(&rom, 0x0800_0000).unwrap();
    assert_eq!(st::count(&rom, &t), 1);
    let a = st::anim(&rom, &t, 0).unwrap();
    assert_eq!(a.variants.len(), 1);
    let v = &a.variants[0];
    assert_eq!(v.end, st::End::Loop(0));
    assert_eq!(
        v.steps
            .iter()
            .map(|s| (s.frame, s.delay))
            .collect::<Vec<_>>(),
        [(0, 4), (0, 2)]
    );
    assert_eq!(v.steps[0].tiles, Some((0, 2)));
    assert_eq!(v.steps[0].boxes[0].rect, [-2, -3, 4, 5]);
    let b = st::bounds(&a);
    assert_eq!(b, (-8, -4, 8, 4));
    let px = st::draw(&rom, &t, a.frames[0].as_ref().unwrap(), (0, 2), b).unwrap();
    assert!(px[..8].iter().all(|&p| p == 1));
    assert_eq!(px[15], 2);
    assert!(px[8..15].iter().all(|&p| p == 0));
    assert!(st::hitbox_lines(&a).starts_with("0\t0\t0\t0\t0\t-2\t-3\t4\t5\t"));
}

#[test]
fn coverage_parses_aggregates_and_scores() {
    use crate::coverage::{self, Class, Found, Status};
    let c = "const u32 gA_Gfx[] = INCGFX_U32(\"graphics/x/a.png\", \".4bpp.lz\");\n\
             const u16 gPals[][16] = {\n  INCGFX_U16(\"graphics/x/p0.pal\", \".gbapal\"),\n  \
             INCGFX_U16(\"graphics/x/p1.pal\", \".gbapal\"),\n};\n\
             const u32 sUnusedGfx[] = INCGFX_U32(\"graphics/y/b.png\", \".4bpp\");\n\
             const u32 gLone_Gfx[] = INCGFX_U32(\"graphics/z/c.png\", \".4bpp\");\n";
    let incs = coverage::parse(c, "src/x.c");
    let syms: Vec<_> = incs
        .iter()
        .map(|i| (i.symbol.as_str(), i.index, i.count))
        .collect();
    assert_eq!(
        syms,
        [
            ("gA_Gfx", 0, 1),
            ("gPals", 0, 2),
            ("gPals", 1, 2),
            ("sUnusedGfx", 0, 1),
            ("gLone_Gfx", 0, 1)
        ]
    );
    assert_eq!(
        coverage::class("graphics/x/p0.pal", ".gbapal"),
        Class::Palette
    );
    assert_eq!(
        coverage::kind("graphics/pokemon/abra/icon.png"),
        "Pokémon icons"
    );
    let data = vec![
        (
            0x0800_0100,
            "gA_Gfx".to_string(),
            64,
            Some("x.o".to_string()),
        ),
        (0x0800_0200, "gPals".to_string(), 64, None),
        (0x0800_0300, "sUnusedGfx".to_string(), 32, None),
        (0x0800_0400, "gLone_Gfx".to_string(), 32, None),
    ];
    // A ROM pointing at all but gLone_Gfx.
    let mut rom = Vec::new();
    for a in [0x0800_0100u32, 0x0800_0200, 0x0800_0300] {
        rom.extend(a.to_le_bytes());
    }
    let files = coverage::files(&incs, &data, &rom, &Default::default());
    let by = |s: &str| files.iter().position(|f| f.source.ends_with(s)).unwrap();
    assert_eq!(
        files[by("p1.pal")].ranges,
        [(0x0800_0220, 0x0800_0240, false)]
    );
    assert_eq!(
        files[by("a.png")].ranges,
        [(0x0800_0100, 0x0800_0140, true)]
    );
    assert!(
        files[by("b.png")]
            .unused
            .as_ref()
            .unwrap()
            .contains("unused")
    );
    assert!(
        files[by("c.png")]
            .unused
            .as_ref()
            .unwrap()
            .starts_with("unreferenced")
    );
    // Bytes computed from a compressed blob's unpacked data (tagged with
    // its start) find it; a guess inside it doesn't.
    let mut found = Found::default();
    found.computed.insert(0x0800_0120);
    assert_eq!(
        coverage::score(&files, &found)[by("a.png")],
        Some(Status::Missing)
    );
    found.computed.insert(0x0800_0100);
    assert_eq!(
        coverage::score(&files, &found)[by("a.png")],
        Some(Status::Greyscale)
    );
    let mut found = Found::default();
    found.packed.insert(0x0800_0100);
    let st = coverage::score(&files, &found);
    assert_eq!(st[by("a.png")], Some(Status::Greyscale));
    assert_eq!(st[by("p0.pal")], Some(Status::Missing));
    assert_eq!(st[by("b.png")], None);
    // The second palette's bytes copied: the image beside it has one.
    found.bytes.insert(0x0800_0230);
    let st = coverage::score(&files, &found);
    assert_eq!(st[by("a.png")], Some(Status::WithPalette));
    assert_eq!(st[by("p1.pal")], Some(Status::Found));
    let (text, tsv) = coverage::report(&files, &[("run", st)]);
    assert!(text.contains("used in play: 3; left out: 2"), "{text}");
    assert_eq!(tsv.lines().count(), 6);
}

#[test]
fn coverage_curve_scores_a_fuzz_run_over_time() {
    use crate::coverage;
    let c = "const u32 gA_Gfx[] = INCGFX_U32(\"graphics/x/a.png\", \".4bpp.lz\");\n\
             const u16 gPals[][16] = {\n  INCGFX_U16(\"graphics/x/p0.pal\", \".gbapal\"),\n  \
             INCGFX_U16(\"graphics/x/p1.pal\", \".gbapal\"),\n};\n";
    let incs = coverage::parse(c, "src/x.c");
    let data = vec![
        (0x0800_0100, "gA_Gfx".to_string(), 64, None),
        (0x0800_0200, "gPals".to_string(), 64, None),
    ];
    let rom: Vec<u8> = [0x0800_0100u32, 0x0800_0200]
        .iter()
        .flat_map(|a| a.to_le_bytes())
        .collect();
    let files = coverage::files(&incs, &data, &rom, &Default::default());
    let dir = std::env::temp_dir().join(format!("gbaa-curve-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("w00")).unwrap();
    std::fs::write(
        dir.join("w00/events.tsv"),
        "secs\tphase\tkind\taddr\tlen\tcallback2\n\
         10.0\troute\tpacked\t0x08000100\t64\t0x0\n\
         70.5\tfuzz\tcopied\t0x08000230\t2\t0x0\n",
    )
    .unwrap();
    let curve = coverage::curve(&files, &dir, 60.0).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    // The image alone by a minute (greyscale), then its palette.
    assert_eq!(
        curve,
        "secs\tfound\tused\tshare\timages\tpalettes\ttilemaps\n\
         60\t1\t3\t33.3\t1\t0\t0\n\
         70\t2\t3\t66.7\t1\t1\t0\n"
    );
}

#[test]
fn coverage_leaves_out_tileset_palette_slots_never_loaded() {
    use crate::coverage;
    let mut c = String::new();
    for (sym, dir) in [
        ("gTilesetPalettes_General", "primary/general"),
        ("gTilesetPalettes_Petalburg", "secondary/petalburg"),
        ("gTilesetPalettes_SecretBase", "primary/secret_base"),
    ] {
        c += &format!("const u16 {sym}[][16] = {{\n");
        for i in 0..16 {
            c += &format!(
                "  INCGFX_U16(\"data/tilesets/{dir}/palettes/{i:02}.pal\", \".gbapal\"),\n"
            );
        }
        c += "};\n";
    }
    let incs = coverage::parse(&c, "src/graphics.c");
    let data: Vec<_> = (0..3u32)
        .map(|k| {
            let s = ["gTilesetPalettes_General", "gTilesetPalettes_Petalburg"];
            let name = s
                .get(k as usize)
                .copied()
                .unwrap_or("gTilesetPalettes_SecretBase");
            (0x0800_0000 + k * 0x200, name.to_string(), 0x200, None)
        })
        .collect();
    let rom: Vec<u8> = (0..3u32)
        .flat_map(|k| (0x0800_0000 + k * 0x200).to_le_bytes())
        .collect();
    let files = coverage::files(&incs, &data, &rom, &Default::default());
    let unused = |dir: &str| -> Vec<usize> {
        (0..16)
            .filter(|i| {
                let src = format!("data/tilesets/{dir}/palettes/{i:02}.pal");
                files
                    .iter()
                    .find(|f| f.source == src)
                    .unwrap()
                    .unused
                    .is_some()
            })
            .collect()
    };
    assert_eq!(unused("primary/general"), (6..16).collect::<Vec<_>>());
    assert_eq!(
        unused("secondary/petalburg"),
        [0, 1, 2, 3, 4, 5, 13, 14, 15]
    );
    assert!(unused("primary/secret_base").is_empty());
}

#[test]
fn scan_finds_clean_lz77_and_marks_the_rest() {
    use crate::coverage::{self, Found, Status};
    use crate::scan::{self, Blob};
    // A literal, then copies of 18 and 13 bytes: 32 bytes from 10.
    let blob = [0x10, 0x20, 0, 0, 0x60, 0xAB, 0xF0, 0x00, 0xA0, 0x00];
    let mut rom = vec![0u8; 0x40];
    rom[0x10..0x1a].copy_from_slice(&blob);
    // A back-reference before the start: not a blob.
    rom[0x20..0x27].copy_from_slice(&[0x10, 0x20, 0, 0, 0x80, 0x00, 0x05]);
    // A size that isn't a multiple of 4, and one unaligned header.
    rom[0x28..0x32].copy_from_slice(&[0x10, 0x21, 0, 0, 0x60, 0xAB, 0xF0, 0x00, 0xA0, 0x00]);
    rom.extend([0, 0x10, 0x20, 0, 0]);
    let found = scan::lz77_blobs(&rom);
    let b = Blob {
        addr: 0x0800_0010,
        packed: 10,
        size: 32,
    };
    assert_eq!(found, [b]);
    let mut run = Found::default();
    assert!(!scan::reached(&run, &b));
    run.bytes.insert(0x0800_0019);
    assert!(scan::reached(&run, &b));
    // Missing in a run, its blob found by the scan: scan only.
    let incs = coverage::parse(
        "const u32 gA_Gfx[] = INCGFX_U32(\"graphics/x/a.png\", \".4bpp.lz\");",
        "src/x.c",
    );
    let data = vec![(0x0800_0010, "gA_Gfx".to_string(), 10, None)];
    let words = 0x0800_0010u32.to_le_bytes();
    let files = coverage::files(&incs, &data, &words, &Default::default());
    let st = coverage::score(&files, &Found::default());
    let starts = found.iter().map(|b| b.addr).collect();
    assert_eq!(
        coverage::with_scan(&files, &st, &starts),
        [Some(Status::ScanOnly)]
    );
    let dir = std::env::temp_dir().join(format!("gbaa-scan-test-{}", std::process::id()));
    let toml = scan::write(&rom, &found, &dir).unwrap();
    assert!(toml.contains("palette = \"none\"") && toml.contains(scan::MARK));
    let png = std::fs::read(dir.join("scan/lz_08000010.png")).unwrap();
    let (w, h, px, _) = crate::graphics::read_indexed_png(&png).unwrap();
    assert_eq!((w, h, px[0], px[1]), (8, 8, 0xB, 0xA));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn computed_bytes_find_only_1bpp_images() {
    use crate::coverage::{self, Found, Status};
    let incs = coverage::parse(
        "const u8 gMonFootprint_Abra[] = INCGFX_U8(\"graphics/pokemon/abra/footprint.png\", \".1bpp\");\n\
         const u32 gA_Gfx[] = INCGFX_U32(\"graphics/x/a.png\", \".4bpp\");",
        "src/x.c",
    );
    let data = vec![
        (0x0800_0100, "gMonFootprint_Abra".to_string(), 32, None),
        (0x0800_0200, "gA_Gfx".to_string(), 32, None),
    ];
    let mut rom = Vec::new();
    for a in [0x0800_0100u32, 0x0800_0200] {
        rom.extend(a.to_le_bytes());
    }
    let files = coverage::files(&incs, &data, &rom, &Default::default());
    let by = |s: &str| files.iter().position(|f| f.source.ends_with(s)).unwrap();
    let mut found = Found::default();
    found.computed.extend([0x0800_0104, 0x0800_0204]);
    let st = coverage::score(&files, &found);
    assert_ne!(st[by("footprint.png")], Some(Status::Missing));
    assert_eq!(st[by("a.png")], Some(Status::Missing));
}

#[test]
fn coverage_leaves_out_anim_tags_no_script_loads() {
    use crate::coverage;
    let table = "const struct CompressedSpriteSheet gBattleAnimPicTable[] =\n{\n    \
                 {gBattleAnimSpriteGfx_Bone, 0x0200, ANIM_TAG_BONE},\n    \
                 {gBattleAnimSpriteGfx_Fangs, 0x0400, ANIM_TAG_FANGS},\n    \
                 {gBattleAnimSpriteGfx_Rocks, 0x0C00, ANIM_TAG_ROCKS},\n    \
                 {gBattleAnimSpriteGfx_Shared, 0x0200, ANIM_TAG_A},\n    \
                 {gBattleAnimSpriteGfx_Shared, 0x0200, ANIM_TAG_B},\n    \
                 {gBattleAnimSpriteGfx_Named, 0x0200, ANIM_TAG_C},\n};\n\
                 const struct CompressedSpritePalette gBattleAnimPaletteTable[] =\n{\n    \
                 {gBattleAnimSpritePal_Fangs, ANIM_TAG_FANGS},\n};\n";
    let scripts =
        "Move_BONE_CLUB:\n\tloadspritegfx ANIM_TAG_BONE\n\tloadspritegfx ANIM_TAG_B @ x\n";
    let code = [
        "const u32 gBattleAnimSpriteGfx_Fangs[] = INCGFX_U32(\"a.png\", \".4bpp.lz\");\n\
         extern const u32 gBattleAnimSpriteGfx_Named[];\n",
        "LoadCompressedSpriteSheetUsingHeap(&gBattleAnimPicTable[GET_TRUE_SPRITE_INDEX(ANIM_TAG_ROCKS)]);\n\
         LoadCompressedSpriteSheet(gBattleAnimSpriteGfx_Named);\n",
    ];
    let u = coverage::unscripted(table, scripts, &code);
    let keys: Vec<_> = u.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["gBattleAnimSpriteGfx_Fangs", "gBattleAnimSpritePal_Fangs"]
    );
    // In the denominator: left out with that reason.
    let incs = coverage::parse(code[0], "src/graphics.c");
    let data = vec![(
        0x0800_0100,
        "gBattleAnimSpriteGfx_Fangs".to_string(),
        64,
        None,
    )];
    let files = coverage::files(&incs, &data, &0x0800_0100u32.to_le_bytes(), &u);
    assert!(files[0].unused.as_ref().unwrap().starts_with("unscripted"));
}
