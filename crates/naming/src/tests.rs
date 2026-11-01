use super::*;

fn toks(s: &str) -> Vec<String> {
    subtokens(s)
}

#[test]
fn subtokens_split_like_the_design() {
    assert_eq!(
        toks("Task_HandleChooseMonInput"),
        ["task", "handle", "choose", "mon", "input"]
    );
    assert_eq!(toks("DrawHPBar"), ["draw", "hp", "bar"]);
    assert_eq!(toks("GetMonData2"), ["get", "mon", "data", "2"]);
    assert_eq!(toks("sBgCnt.135"), ["s", "bg", "cnt", "135"]);
    assert_eq!(toks("the_A_thing"), ["thing"]);
    assert_eq!(data_subtokens("gPlayerParty"), ["player", "party"]);
    assert_eq!(data_subtokens("sTrainerId"), ["trainer", "id"]);
    assert_eq!(data_subtokens("gs"), ["gs"]);
}

#[test]
fn precision_and_recall_are_multisets() {
    let (p, r) = pr(&toks("GetMonData"), &toks("GetMonData2"));
    assert_eq!((p, r), (1.0, 0.75));
    assert_eq!(pr(&[], &toks("Foo")), (0.0, 0.0));
    let (p, _) = pr(&toks("MonMon"), &toks("Mon"));
    assert_eq!(p, 0.5);
    let mut s = Score::default();
    s.add(&toks("GetMonData"), &toks("GetMonData2"), false);
    s.add(&toks("Foo"), &toks("Bar"), false);
    assert_eq!((s.precision(), s.recall()), (0.5, 0.375));
    assert!(exact("getmondata", "GetMonData"));
}

#[test]
fn draws_whole_subtrees_then_fills_each_class() {
    // 1 calls 2 and 3; 3 calls 4. Sizes put them in all three classes.
    let mut insns = BTreeMap::new();
    let mut calls = BTreeMap::new();
    for k in 1..=60u32 {
        insns.insert(k, [5, 50, 500][(k % 3) as usize]);
    }
    calls.insert(1, BTreeSet::from([2, 3]));
    calls.insert(3, BTreeSet::from([4, 99]));
    let g = Graph { insns, calls };
    assert_eq!(g.subtree(1), BTreeSet::from([1, 2, 3, 4]));
    let plan = Plan {
        seed: 7,
        per_class: 5,
        roots: 1,
        max_subtree: 12,
    };
    let s = draw(&g, &plan);
    assert_eq!(s.roots.keys().copied().collect::<Vec<_>>(), [1]);
    assert!(s.roots[&1].is_subset(&s.keys));
    assert_eq!(s.keys.len(), 15);
    for c in Class::ALL {
        assert_eq!(
            s.keys.iter().filter(|k| Class::of(g.insns[k]) == c).count(),
            5
        );
    }
    assert_eq!(draw(&g, &plan), s, "the draw is seeded");
}

#[test]
fn levels_go_bottom_up_and_survive_cycles() {
    let keys = BTreeSet::from([1, 2, 3, 4, 5]);
    let calls = BTreeMap::from([
        (1, BTreeSet::from([2, 3])),
        (3, BTreeSet::from([4, 1])),
        (5, BTreeSet::from([5, 77])),
    ]);
    let lv = levels(&keys, &calls);
    assert_eq!(lv[0], [2, 4, 5]);
    assert!(lv.iter().position(|l| l.contains(&3)) < lv.iter().position(|l| l.contains(&1)));
    assert_eq!(lv.iter().map(Vec::len).sum::<usize>(), 5);
}

#[test]
fn answers_parse_with_or_without_a_fence() {
    let a = parse_answer(
        "```toml\nname = \"SeedRng\"\nsummary = \"seeds it\"\n[data]\nd_03005d80 = \"gRngValue\"\nx = \"y\"\n```",
    )
    .unwrap();
    assert_eq!(a.name, "SeedRng");
    assert_eq!(a.summary, "seeds it");
    assert_eq!(
        a.data,
        BTreeMap::from([(0x0300_5d80, "gRngValue".to_string())])
    );
    let fixed = "```toml\nname = \"A\"\n```\n\nWait, correcting:\n\n```toml\nname = \"B\"\n```\n";
    assert_eq!(parse_answer(fixed).unwrap().name, "B");
    assert_eq!(parse_answer("```\nname = \"C\"\n```").unwrap().name, "C");
    assert!(parse_answer("summary = \"x\"").is_err());
    assert!(parse_answer("I think it is Foo").is_err());
}

#[test]
fn the_vote_takes_the_commonest_name() {
    let a = |n: &str| Answer {
        data: BTreeMap::from([(8, n.to_string())]),
        ..Answer::default()
    };
    let v = vote(&[a("gB"), a("gA"), a("gB"), a("gA"), a("gC")]);
    assert_eq!(v[&8], "gA");
}

#[test]
fn json_reads_a_claude_result() {
    let j = Json::parse(
        r#"{"usage":{"input_tokens":2,"output_tokens":7},"result":"name = \"x\"\né","is_error":false,"a":[1,-2.5e1,null]}"#,
    )
    .unwrap();
    assert_eq!(j.str("result"), Some("name = \"x\"\né"));
    assert_eq!(j.get("usage").unwrap().num("output_tokens"), 7.0);
    assert_eq!(j.get("is_error"), Some(&Json::Bool(false)));
    assert_eq!(
        j.get("a"),
        Some(&Json::Arr(vec![
            Json::Num(1.0),
            Json::Num(-25.0),
            Json::Null
        ]))
    );
}

#[test]
fn siblings_match_masked_code_with_other_names() {
    // A Thumb BL pair and an ARM BL lose their offsets; a Thumb B keeps it.
    assert_eq!(mask_insn(0xf123, true), 0xf000);
    assert_eq!(mask_insn(0xf9ab, true), 0xf800);
    assert_eq!(mask_insn(0xe005, true), 0xe005);
    assert_eq!(mask_insn(0xeb00_1234, false), 0xeb00_0000);
    assert_eq!(mask_insn(0xe591_0004, false), 0xe591_0004);
    let m = |v: &[(u32, &[u8], &str)]| -> BTreeMap<u32, (Vec<u8>, String)> {
        v.iter()
            .map(|&(k, fp, n)| (k, (fp.to_vec(), n.to_string())))
            .collect()
    };
    let a = m(&[
        (1, b"x", "Foo"),
        (2, b"y", "Same"),
        (3, b"z", "Dup"),
        (4, b"z", "Dup2"),
        (5, b"w", "Lone"),
    ]);
    let b = m(&[
        (10, b"x", "Bar"),
        (20, b"y", "SAME"),
        (30, b"z", "Other"),
        (40, b"v", "Lone2"),
    ]);
    // x differs by name; y is the same name; z isn't unique in a.
    assert_eq!(sibling_pairs(&a, &b), [(1, 10)]);
}
