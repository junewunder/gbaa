use super::*;

fn sym(name: &str, value: u32, size: u64, kind: SymKind, global: bool, section: usize) -> Sym {
    Sym {
        name: name.to_string(),
        value,
        size,
        kind,
        global,
        section: Some(section),
        exec: section == 1,
    }
}

/// Section 1 is code, 2 ROM data, 3 RAM. Files `main.o` and `ram.o`
/// (section 0 stands for none).
fn syms() -> Vec<Sym> {
    use SymKind::*;
    vec![
        sym("main.o", 0, 0, File, false, 0),
        sym("$t", 0x0800_0000, 0, NoType, false, 1),
        sym("Main", 0x0800_0001, 0x20, Func, true, 1),
        sym("Helper", 0x0800_0021, 0x10, Func, true, 1),
        sym("Copied", 0x0800_0031, 0x10, Func, true, 1),
        sym("Dead", 0x0800_0041, 0x8, Func, true, 1),
        sym("Pointed", 0x0800_0049, 0x8, Func, true, 1),
        sym("asm_loop", 0x0800_0050, 0, NoType, false, 1),
        sym("Tail", 0x0800_0061, 0x8, Func, true, 1),
        sym("$d", 0x0800_0100, 0, NoType, false, 2),
        sym("gTable", 0x0800_0100, 8, Object, true, 2),
        sym("ram.o", 0, 0, File, false, 0),
        sym("EWRAM_START", 0x0200_0000, 0, NoType, false, 3),
        sym("gHeap", 0x0200_0000, 0x100, Object, true, 3),
        sym("sCounter", 0x0200_0100, 4, Object, false, 3),
    ]
}

#[test]
fn names_entries_and_explains_misses() {
    let mut rom = vec![0u8; 0x120];
    // A pool at 0x110: gHeap, gHeap+8, an unnamed RAM word, gTable+4,
    // and a pointer to Pointed (at 0x118, outside the pool's words read).
    let pool = [0x0200_0000u32, 0x0200_0008, 0x0200_0200, 0x0800_0104];
    for (i, w) in pool.iter().enumerate() {
        rom[0x100 + i * 4..0x104 + i * 4].copy_from_slice(&w.to_le_bytes());
    }
    rom[0x118..0x11c].copy_from_slice(&0x0800_0049u32.to_le_bytes());
    let ours = Ours {
        // Main, a continuation inside Main, Helper in ARM, an asm label,
        // one past every function, and one in a table.
        entries: [
            0x0800_0001,
            0x0800_0011,
            0x0800_0020,
            0x0800_0051,
            0x0800_0091,
            0x0800_0105,
        ]
        .into(),
        discovered: BTreeSet::new(),
        code: [(0x0800_0060, true)].into(),
        data: vec![(0x0800_0100, 0x0800_0110)],
        copies: vec![
            Copy {
                src: 0x0800_0030,
                dst: 0x0300_0000,
                len: 0x10,
                entries: [0x0300_0001].into(),
            },
            // Data copied over the same place later: one TOML key, not two.
            Copy {
                src: 0x0800_0100,
                dst: 0x0300_0000,
                len: 4,
                entries: BTreeSet::new(),
            },
        ],
    };
    let n = match_names(&syms(), &rom, &ours);
    let by: BTreeMap<u32, &Name> = n.entries.iter().map(|e| (e.key, &e.name)).collect();
    assert_eq!(
        by[&0x0800_0001],
        &Name::Func {
            name: "Main".into(),
            size: 0x20
        }
    );
    assert_eq!(
        by[&0x0800_0011],
        &Name::Inside {
            func: "Main".into(),
            off: 0x10
        }
    );
    assert_eq!(by[&0x0800_0020], &Name::OtherMode("Helper".into()));
    assert_eq!(by[&0x0800_0051], &Name::Label("asm_loop".into()));
    assert_eq!(by[&0x0800_0091], &Name::Unknown);
    assert_eq!(
        by[&0x0800_0105],
        &Name::Data {
            datum: "gTable".into(),
            off: 5
        }
    );
    let copied = n.entries.iter().find(|e| e.key == 0x0300_0001).unwrap();
    assert_eq!(copied.rom, Some(0x0800_0031));
    assert_eq!(
        copied.name,
        Name::Func {
            name: "Copied".into(),
            size: 0x10
        }
    );
    let missed: Vec<(&str, Miss)> = n
        .missed
        .iter()
        .map(|(r, m)| (r.name.as_str(), *m))
        .collect();
    assert_eq!(
        missed,
        [
            ("Helper", Miss::Dead),
            ("Dead", Miss::Dead),
            ("Pointed", Miss::Pointer),
            ("Tail", Miss::Reached)
        ]
    );
    assert_eq!(n.data[&0x0200_0000].name, "gHeap");
    assert_eq!(n.data[&0x0200_0000].aliases, ["EWRAM_START"]);
    assert!(
        !n.data.contains_key(&0x0800_0050),
        "asm labels in code aren't data"
    );
    assert_eq!(
        n.ram,
        PoolHits {
            exact: 1,
            inside: 1,
            unnamed: 1
        }
    );
    assert_eq!(
        n.rom,
        PoolHits {
            exact: 0,
            inside: 1,
            unnamed: 0
        }
    );
    assert_eq!(n.named(), 3);
    assert_eq!(
        n.copies,
        [
            CopyNames {
                src: 0x0800_0030,
                dst: 0x0300_0000,
                len: 0x10,
                entries: 1,
                funcs: vec![(0x0300_0001, "Copied".into())],
            },
            CopyNames {
                src: 0x0800_0100,
                dst: 0x0300_0000,
                len: 4,
                entries: 0,
                funcs: vec![],
            }
        ]
    );
    // Files: a global function by the local code before it, a static
    // exactly; a global datum has none.
    let file = |k: u32| n.entries.iter().find(|e| e.key == k).unwrap().file.clone();
    assert_eq!(file(0x0800_0001).as_deref(), Some("main.o"));
    assert_eq!(file(0x0800_0051).as_deref(), Some("main.o"));
    assert_eq!(file(0x0800_0091), None);
    assert_eq!(n.data[&0x0200_0100].file.as_deref(), Some("ram.o"));
    assert_eq!(n.data[&0x0200_0000].file, None);
    let sha = "f3ae088181bf583e55daf962a92bb46f4f1d07b7";
    let toml = n.toml(&["test".into()], sha, "pret/test@0", None);
    let parsed: toml::Table = toml.parse().expect("the names file is TOML");
    assert_eq!(parsed["map"]["rom_sha1"].as_str(), Some(sha));
    assert_eq!(parsed["functions"].as_table().unwrap().len(), 3);
    assert_eq!(parsed["data"].as_table().unwrap().len(), 3);
    assert!(toml.contains(
        "0x03000001 = { name = \"Copied\", size = 16, file = \"main.o\", rom = 0x08000031 }"
    ));
    assert!(toml.contains("0x02000100 = { name = \"sCounter\", size = 4, file = \"ram.o\" }"));
    assert!(toml.contains(
        "0x03000000 = { rom = 0x08000030, len = 16, also = [{ rom = 0x08000100, len = 4 }], functions = [{ key = 0x03000001, name = \"Copied\" }] }"
    ));
    // It loads as a source map, every function named.
    let m = codegen::names::SourceMap::from_texts(sha, &[&toml]).unwrap();
    for &k in m.functions.keys() {
        assert!(
            m.ident(codegen::names::Kind::Func, k).is_some(),
            "0x{k:08x}"
        );
    }
    assert_eq!(
        m.ident(codegen::names::Kind::Func, 0x0800_0001),
        Some("Main")
    );
    assert!(toml.contains("0x08000011 = { func = \"Main\", offset = 0x10 }"));
}

/// Two units whose abbrev tables run together unterminated (agbcc's):
/// each gets its own, cut at the repeated code and terminated.
#[test]
fn abbrev_tables_are_cut_and_terminated() {
    // Table at 0: code 1 (compile_unit, no children, name/string), code 2
    // (base_type, no children, no attrs). Table at 9: code 1 again.
    let abbrev = [
        1, 0x11, 0, 0x03, 0x08, 0, 0, // 0..7
        2, 0x24, 0, 0, 0, // 7..12
        1, 0x24, 0, 0, 0, // 12..17: the next table
    ];
    let unit = |off: u32| {
        let mut u = vec![9, 0, 0, 0, 2, 0];
        u.extend(off.to_le_bytes());
        u.push(4);
        u.extend([0, 0]);
        u
    };
    let mut info = unit(0);
    info.extend(unit(12));
    let (info2, ab2) = dwarf::fix_abbrevs(&info, &abbrev);
    assert_eq!(
        ab2,
        [
            1, 0x11, 0, 0x03, 0x08, 0, 0, 2, 0x24, 0, 0, 0, 0, 1, 0x24, 0, 0, 0, 0
        ]
    );
    assert_eq!(&info2[6..10], &0u32.to_le_bytes());
    assert_eq!(&info2[13 + 6..13 + 10], &13u32.to_le_bytes());
}

/// A line program a byte or few short of the next one is grown to it; a
/// wider gap (a program no unit names) is left alone.
#[test]
fn short_line_programs_grow_to_the_next() {
    let mut line = vec![0u8; 100];
    line[0..4].copy_from_slice(&15u32.to_le_bytes()); // gap to 20 is 16
    line[20..24].copy_from_slice(&10u32.to_le_bytes()); // gap to 60 is 36
    line[60..64].copy_from_slice(&35u32.to_le_bytes()); // gap to the end is 36
    let out = dwarf::fix_lines(&line, vec![60, 0, 20]);
    assert_eq!(&out[0..4], &16u32.to_le_bytes());
    assert_eq!(&out[20..24], &10u32.to_le_bytes());
    assert_eq!(&out[60..64], &36u32.to_le_bytes());
}

/// A modern build's `<stdin>` objects take their names from the map's
/// input sections, a long section name's address on the next line.
#[test]
fn stdin_objects_are_named_from_the_map() {
    use SymKind::*;
    let map = [
        " .text          0x00000000        0x0 data/unused.o",
        " .text          0x080003ac      0x628 src/main.o",
        "                0x080003ac                AgbMain",
        " .text.unlikely",
        "                0x080009d4       0x20 src/battle.o",
        " ewram_data     0x02000004        0x5 src/battle.o",
        " .bss           0x0300004e        0x3 src/siirtc.o",
        " .bss           0x03000051        0x1 src/factory.o",
    ]
    .join("\n");
    let sections = map_sections(&map);
    assert_eq!(sections[&0x0800_03ac], (0x0800_09d4, "main.o".to_string()));
    assert_eq!(
        sections[&0x0800_09d4],
        (0x0800_09f4, "battle.o".to_string())
    );
    assert_eq!(
        sections[&0x0200_0004],
        (0x0200_0009, "battle.o".to_string())
    );
    assert_eq!(sections.len(), 5);
    let mut syms = vec![
        sym("<stdin>", 0, 0, File, false, 0),
        sym("Cmd_end", 0x0800_0401, 4, Func, false, 1),
        sym("<stdin>", 0, 0, File, false, 0),
        sym("sState", 0x0200_0004, 1, Object, false, 3),
        sym("Cmd_end", 0x0800_09d5, 4, Func, false, 1),
        sym("<stdin>", 0, 0, File, false, 0),
        // An odd data address: bit 0 isn't a Thumb bit here.
        sym("sSwapped", 0x0300_0051, 1, NoType, false, 3),
        sym("<stdin>", 0, 0, File, false, 0),
        sym("AgbMain", 0x0800_03ad, 4, Func, true, 1),
    ];
    assert_eq!(name_stdin_files(&mut syms, &sections), 3);
    let files: Vec<&str> = syms
        .iter()
        .filter(|s| s.kind == File)
        .map(|s| s.name.as_str())
        .collect();
    // The last places only a global, which names no object.
    assert_eq!(files, ["main.o", "battle.o", "factory.o", "<stdin>"]);
}

#[test]
fn a_field_takes_the_define_family_pret_sets_it_with() {
    use dwarf::{Field, Layout};
    use values::*;
    let h = "#define OPTIONS_TEXT_SPEED_SLOW 0\n\
             #define OPTIONS_TEXT_SPEED_MID 1 // the default\n\
             #define  OPTIONS_TEXT_SPEED_FAST\t0x2\n\
             #define OPTIONS_TEXT_SPEED_COUNT 3\n\
             #define OPTIONS_TEXT_SPEED_LATER (OPTIONS_TEXT_SPEED_FAST + 1)\n\
             #define COOL_RIBBON_NORMAL 1\n\
             #define COOL_RIBBON_SUPER 2\n";
    let defs = defines_in(h, "include/constants/global.h");
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "OPTIONS_TEXT_SPEED_SLOW",
            "OPTIONS_TEXT_SPEED_MID",
            "OPTIONS_TEXT_SPEED_FAST",
            "OPTIONS_TEXT_SPEED_COUNT",
            "COOL_RIBBON_NORMAL",
            "COOL_RIBBON_SUPER"
        ]
    );
    assert_eq!(upper_snake("optionsTextSpeed"), "OPTIONS_TEXT_SPEED");
    assert_eq!(upper_snake("otId"), "OT_ID");
    let c = "gSaveBlock2Ptr->optionsTextSpeed = OPTIONS_TEXT_SPEED_MID;\n\
             if (p.cool == 2) x.coolRibbon = COOL_RIBBON_SUPER;\n\
             .effect = EFFECT_HIT, gContestMoves[m].effect != CONTEST_EFFECT_X;";
    let uses: std::collections::BTreeSet<_> = uses_in(c).into_iter().collect();
    assert!(uses.contains(&("optionsTextSpeed".into(), "OPTIONS_TEXT_SPEED_MID".into())));
    assert!(uses.contains(&("effect".into(), "CONTEST_EFFECT_X".into())));
    assert!(!uses.iter().any(|(f, _)| f == "cool"));
    let field = |name: &str, bits| Field {
        name: name.into(),
        off: 20,
        size: 2,
        ty: "u16".into(),
        bits,
    };
    let types = BTreeMap::from([(
        "struct SaveBlock2".to_string(),
        Layout::Struct {
            size: 24,
            fields: vec![
                field("optionsTextSpeed", Some((0, 3))),
                // Too narrow for FAST.
                field("optionsTextSpeed2", Some((3, 1))),
                field("cool", None),
                field("coolRibbon", None),
            ],
        },
    )]);
    let fams = families(&defs, &uses, &types);
    let ts = &fams["OPTIONS_TEXT_SPEED"];
    assert_eq!(ts.fields, ["struct SaveBlock2.optionsTextSpeed"]);
    // COUNT is a bound, not a value.
    assert_eq!(
        ts.names,
        [
            ("OPTIONS_TEXT_SPEED_SLOW".to_string(), 0),
            ("OPTIONS_TEXT_SPEED_MID".to_string(), 1),
            ("OPTIONS_TEXT_SPEED_FAST".to_string(), 2)
        ]
    );
    // `cool` spells COOL_RIBBON_* by name, but pret never sets it so.
    assert_eq!(fams["COOL_RIBBON"].fields, ["struct SaveBlock2.coolRibbon"]);
    assert!(!fams.contains_key("COOL"));
    // What it writes loads as codegen reads it.
    let text = format!("[map]\nrom_sha1 = \"00\"\n{}", toml(&fams));
    let m = codegen::names::SourceMap::from_texts("00", &[&text]).unwrap();
    let v = &m.values["OPTIONS_TEXT_SPEED"];
    assert_eq!(v.file, "include/constants/global.h");
    assert_eq!(
        v.fields,
        [(
            "struct SaveBlock2".to_string(),
            "optionsTextSpeed".to_string()
        )]
    );
    assert_eq!(v.names[1], ("OPTIONS_TEXT_SPEED_MID".to_string(), 1));
}
