use super::*;

const SHA: &str = "f3ae088181bf583e55daf962a92bb46f4f1d07b7";

fn head(sha: &str) -> String {
    format!("[map]\nrom_sha1 = \"{sha}\"\nsource = \"test\"\nformat = 1\n")
}

#[test]
fn sha1_matches_the_standard_vectors() {
    assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    assert_eq!(sha1_hex(long), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
    let million = vec![b'a'; 1_000_000];
    assert_eq!(
        sha1_hex(&million),
        "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
    );
}

#[test]
fn a_missing_names_dir_gives_no_files() {
    let dir = std::env::temp_dir().join("gbaa-names-test-no-such-dir");
    assert!(!dir.exists());
    assert_eq!(
        files_for(&dir, SHA).unwrap(),
        Vec::<std::path::PathBuf>::new()
    );
}

#[test]
fn refuses_another_roms_map_and_a_missing_header() {
    let other = format!(
        "{}[functions]\n0x08000001 = {{ name = \"Main\" }}\n",
        head("0000000000000000000000000000000000000000")
    );
    let e = SourceMap::from_texts(SHA, &[&other]).unwrap_err();
    assert!(e.contains("isn't this ROM's"), "{e}");
    let bare = "[functions]\n0x08000001 = { name = \"Main\" }\n";
    let e = SourceMap::from_texts(SHA, &[bare]).unwrap_err();
    assert!(e.contains("no [map]"), "{e}");
    let future = format!("{}format = 2\n", head(SHA).replace("format = 1\n", ""));
    assert!(SourceMap::from_texts(SHA, &[&future]).is_err());
    // Upper-case hex is the same hash.
    assert!(SourceMap::from_texts(SHA, &[&head(&SHA.to_uppercase())]).is_ok());
}

#[test]
fn a_later_file_wins_per_key_and_report_tables_are_ignored() {
    let pret = format!(
        "{}[functions]\n\
         0x08000001 = {{ name = \"Main\", size = 32, file = \"main.o\" }}\n\
         0x08000021 = {{ name = \"Helper\", size = 16, file = \"main.o\" }}\n\
         [inside]\n0x08000011 = {{ func = \"Main\", offset = 0x10 }}\n\
         [unnamed]\n0x08000091 = \"no symbol\"\n\
         [missed]\n0x08000041 = {{ name = \"Dead\", size = 8, why = \"unreached\" }}\n\
         [copies]\n0x03000000 = {{ rom = 0x08000030, len = 16, functions = [{{ key = 0x03000001, name = \"Copied\" }}] }}\n\
         [data]\n0x02000000 = {{ name = \"gHeap\", size = 256 }}\n",
        head(SHA)
    );
    let given = format!(
        "{}[functions]\n0x08000021 = {{ name = \"DrawThing\" }}\n0x08000061 = {{ name = \"NewHelper\" }}\n",
        head(SHA)
    );
    let m = SourceMap::from_texts(SHA, &[&pret, &given]).unwrap();
    assert_eq!(m.ident(Kind::Func, 0x0800_0001), Some("Main"));
    assert_eq!(m.ident(Kind::Func, 0x0800_0021), Some("DrawThing"));
    // The later entry replaces the whole entry, not just its name.
    assert_eq!(m.functions[&0x0800_0021].file, None);
    assert_eq!(m.ident(Kind::Func, 0x0800_0061), Some("NewHelper"));
    assert_eq!(m.ident(Kind::Func, 0x0300_0001), Some("Copied"));
    assert_eq!(m.ident(Kind::Data, 0x0200_0000), Some("gHeap"));
    // Nothing from the report tables.
    assert_eq!(m.ident(Kind::Func, 0x0800_0011), None);
    assert_eq!(m.ident(Kind::Func, 0x0800_0041), None);
    assert_eq!(m.idents().count(), 5);
    assert_eq!(m.key_of("Helper"), None);
    assert_eq!(m.key_of("DrawThing"), Some((Kind::Func, 0x0800_0021)));
}

#[test]
fn refuses_one_name_on_two_addresses_of_one_file() {
    let twice = format!(
        "{}[functions]\n0x08000001 = {{ name = \"Main\" }}\n0x08000021 = {{ name = \"Main\" }}\n",
        head(SHA)
    );
    let e = SourceMap::from_texts(SHA, &[&twice]).unwrap_err();
    assert!(e.contains("Main is on both"), "{e}");
    let func_and_data = format!(
        "{}[functions]\n0x08000001 = {{ name = \"gThing\" }}\n[data]\n0x02000000 = {{ name = \"gThing\" }}\n",
        head(SHA)
    );
    assert!(SourceMap::from_texts(SHA, &[&func_and_data]).is_err());
    // Across files it's allowed: each gets its address.
    let a = format!(
        "{}[functions]\n0x08000001 = {{ name = \"Main\" }}\n",
        head(SHA)
    );
    let b = format!(
        "{}[functions]\n0x08000021 = {{ name = \"Main\" }}\n",
        head(SHA)
    );
    let m = SourceMap::from_texts(SHA, &[&a, &b]).unwrap();
    assert_eq!(m.ident(Kind::Func, 0x0800_0001), Some("Main__08000001"));
    assert_eq!(m.ident(Kind::Func, 0x0800_0021), Some("Main__08000021"));
}

#[test]
fn keywords_and_printer_names_get_an_underscore() {
    let map = format!(
        "{}[functions]\n\
         0x08000001 = {{ name = \"type\" }}\n\
         0x08000011 = {{ name = \"match\" }}\n\
         0x08000021 = {{ name = \"settle\" }}\n\
         0x08000031 = {{ name = \"r3\" }}\n\
         0x08000041 = {{ name = \"f_0800abcd\" }}\n\
         0x08000051 = {{ name = \"Flow\" }}\n\
         0x08000061 = {{ name = \"rt\" }}\n\
         0x08000071 = {{ name = \"cycles\" }}\n\
         0x08000081 = {{ name = \"r3x\" }}\n\
         0x08000091 = {{ name = \"sBgCnt.135\" }}\n\
         0x080000a1 = {{ name = \"9lives\" }}\n\
         [data]\n0x02000000 = {{ name = \"box\" }}\n0x02000010 = {{ name = \"M\" }}\n",
        head(SHA)
    );
    let m = SourceMap::from_texts(SHA, &[&map]).unwrap();
    let id = |k| m.ident(Kind::Func, k);
    assert_eq!(id(0x0800_0001), Some("type_"));
    assert_eq!(id(0x0800_0011), Some("match_"));
    assert_eq!(id(0x0800_0021), Some("settle_"));
    assert_eq!(id(0x0800_0031), Some("r3_"));
    assert_eq!(id(0x0800_0041), Some("f_0800abcd_"));
    assert_eq!(id(0x0800_0051), Some("Flow_"));
    assert_eq!(id(0x0800_0061), Some("rt_"));
    assert_eq!(id(0x0800_0071), Some("cycles_"));
    assert_eq!(id(0x0800_0081), Some("r3x"));
    assert_eq!(id(0x0800_0091), Some("sBgCnt_135"));
    assert_eq!(id(0x0800_00a1), None);
    assert_eq!(m.ident(Kind::Data, 0x0200_0000), Some("box_"));
    assert_eq!(m.ident(Kind::Data, 0x0200_0010), Some("M_"));
    // Every printed identifier parses as one.
    for (i, _, _) in m.idents() {
        syn::parse_str::<syn::Ident>(i).unwrap();
    }
}

#[test]
fn the_prelude_list_is_read_off_the_prelude() {
    let p = prelude_exports();
    for n in [
        "rt",
        "Flow",
        "Exit",
        "prim",
        "MulKind",
        "Flag",
        "PsrMask",
        "Reg",
        "ShiftType",
        "Width",
        "M",
        "call",
        "settle",
        "yield_at",
    ] {
        assert!(p.contains(n), "{n} missing from {p:?}");
    }
    assert!(!p.contains("self") && !p.contains("_"), "{p:?}");
}

#[test]
fn a_repeated_name_is_qualified_by_its_file_stem_then_address() {
    let map = format!(
        "{}[functions]\n\
         0x08000001 = {{ name = \"Cmd_call\", file = \"src/scrcmd.c\" }}\n\
         0x08000101 = {{ name = \"Cmd_call\", file = \"src/battle_script_commands.c\" }}\n\
         0x08000201 = {{ name = \"Task\", file = \"src/mystery_gift.o\" }}\n\
         0x08000301 = {{ name = \"Task\", file = \"data/mystery_gift.o\" }}\n\
         0x08000401 = {{ name = \"Unique\", file = \"src/scrcmd.c\" }}\n\
         [data]\n\
         0x02000000 = {{ name = \"sBgTemplates\", file = \"party_menu.o\" }}\n\
         0x02000100 = {{ name = \"sBgTemplates\", file = \"start_menu.o\" }}\n",
        head(SHA)
    );
    let m = SourceMap::from_texts(SHA, &[&map]).unwrap();
    let id = |k| m.ident(Kind::Func, k);
    assert_eq!(id(0x0800_0001), Some("Cmd_call__scrcmd"));
    assert_eq!(id(0x0800_0101), Some("Cmd_call__battle_script_commands"));
    assert_eq!(id(0x0800_0201), Some("Task__mystery_gift_08000201"));
    assert_eq!(id(0x0800_0301), Some("Task__mystery_gift_08000301"));
    assert_eq!(id(0x0800_0401), Some("Unique"));
    assert_eq!(
        m.ident(Kind::Data, 0x0200_0000),
        Some("sBgTemplates__party_menu")
    );
    assert_eq!(
        m.ident(Kind::Data, 0x0200_0100),
        Some("sBgTemplates__start_menu")
    );
    // The table is one-to-one.
    for (i, kind, k) in m.idents() {
        assert_eq!(m.ident(kind, k), Some(i));
        assert_eq!(m.key_of(i), Some((kind, k)));
    }
}

#[test]
fn a_qualified_form_meeting_a_given_name_takes_its_address() {
    let map = format!(
        "{}[functions]\n0x08000001 = {{ name = \"loop\" }}\n0x08000011 = {{ name = \"loop_\" }}\n",
        head(SHA)
    );
    let m = SourceMap::from_texts(SHA, &[&map]).unwrap();
    assert_eq!(m.ident(Kind::Func, 0x0800_0001), Some("loop_"));
    assert_eq!(m.ident(Kind::Func, 0x0800_0011), Some("loop___08000011"));
}

#[test]
fn the_readable_list_takes_keys_and_names() {
    let text = format!(
        "{}[functions]\n0x08000101 = {{ name = \"AgbMain\" }}\n[data]\n0x02000000 = {{ name = \"gBuf\" }}\n",
        head(SHA)
    );
    let m = SourceMap::from_texts(SHA, &[&text]).unwrap();
    let list = "# picks\n0x08000201\nf_08000301  # by link name\n\nAgbMain\n";
    let keys = readable_list(list, Some(&m), &[]).unwrap();
    assert_eq!(
        keys,
        BTreeSet::from([0x0800_0101, 0x0800_0201, 0x0800_0301])
    );
    // Data isn't a function, and a name needs a map.
    let e = readable_list("gBuf\n", Some(&m), &[]).unwrap_err();
    assert!(e.contains("line 1"), "{e}");
    assert!(readable_list("AgbMain\n", None, &[]).is_err());
    // `*` is every function.
    let keys = readable_list("*\n0x08000201\n", None, &[0x0800_0101]).unwrap();
    assert_eq!(keys, BTreeSet::from([0x0800_0101, 0x0800_0201]));
}

#[test]
fn locals_and_lines_load_and_a_later_list_replaces_the_whole() {
    let a = format!(
        "{}[functions]\n0x08000001 = {{ name = \"F\", src = \"src/f.c\", proto = \"u32 F(u8 x)\" }}\n\
         [locals]\n0x08000001 = [\n  {{ name = \"x\", reg = 0, from = 0x08000000, to = 0x08000010, type = \"u8\", param = true }},\n  \
         {{ name = \"buf\", slot = 8, from = 0x08000004, to = 0x08000010 }},\n]\n\
         [lines]\n0x08000000 = 12\n0x08000004 = \"include/f.h:3\"\n",
        head(SHA)
    );
    let b = format!(
        "{}[locals]\n0x08000001 = [{{ name = \"y\", reg = 1, from = 0x08000000, to = 0x08000002 }}]\n",
        head(SHA)
    );
    let m = SourceMap::from_texts(SHA, &[&a]).unwrap();
    let f = &m.functions[&0x0800_0001];
    assert_eq!(f.src.as_deref(), Some("src/f.c"));
    assert_eq!(f.proto.as_deref(), Some("u32 F(u8 x)"));
    let ls = &m.locals[&0x0800_0001];
    assert_eq!(ls.len(), 2);
    assert_eq!(ls[0].home, Home::Reg(0));
    assert!(ls[0].param);
    assert_eq!(ls[0].ty.as_deref(), Some("u8"));
    assert_eq!(
        (ls[1].home, ls[1].from, ls[1].to),
        (Home::Slot(8), 0x0800_0004, 0x0800_0010)
    );
    assert_eq!(
        m.lines[&0x0800_0000],
        Line {
            file: None,
            line: 12
        }
    );
    assert_eq!(
        m.lines[&0x0800_0004],
        Line {
            file: Some("include/f.h".into()),
            line: 3
        }
    );
    let m = SourceMap::from_texts(SHA, &[&a, &b]).unwrap();
    assert_eq!(m.locals[&0x0800_0001].len(), 1);
    assert_eq!(m.locals[&0x0800_0001][0].name, "y");
    let bad = format!(
        "{}[locals]\n0x08000001 = [{{ name = \"z\", from = 1, to = 2 }}]\n",
        head(SHA)
    );
    let e = SourceMap::from_texts(SHA, &[&bad]).unwrap_err();
    assert!(e.contains("reg (0-15) or slot"), "{e}");
}
