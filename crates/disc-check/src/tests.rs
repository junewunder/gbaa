use super::*;

/// Truth: Thumb `ThumbFn` at 0x08000101 (0x40 bytes), ARM `ArmFn` at
/// 0x08000200 (0x10 bytes), and size-0 Thumb `Label` at 0x08000301.
const TRUTH: &str = "\
0x08000101\t64\tThumbFn
0x08000200\t16\tArmFn
0x08000301\t0\tLabel
";

fn truth() -> Vec<TruthRow> {
    parse_truth(TRUTH).unwrap()
}

fn run(disc: &str) -> Score {
    score(&truth(), &parse_discovered(disc).unwrap()).unwrap()
}

fn kinds(s: &Score) -> Vec<(u32, FalseKind)> {
    s.false_starts.iter().map(|f| (f.addr, f.kind)).collect()
}

fn miss_addrs(s: &Score) -> Vec<u32> {
    s.misses.iter().map(|r| r.addr).collect()
}

#[test]
fn exact_match() {
    let s = run("0x08000101\n0x08000200\n0x08000301\n");
    assert_eq!((s.matched, s.discovered, s.truth), (3, 3, 3));
    assert!(s.false_starts.is_empty() && s.misses.is_empty());
    assert!(
        s.report(5)
            .starts_with("precision 1.0000 (3/3)\nrecall 1.0000 (3/3)\n")
    );
}

#[test]
fn wrong_mode_is_a_false_start_and_a_miss() {
    // ThumbFn's address as ARM, ArmFn's address as Thumb.
    let s = run("0x08000100\n0x08000201\n");
    assert_eq!(s.matched, 0);
    assert_eq!(
        kinds(&s),
        [
            (0x0800_0100, FalseKind::WrongMode),
            (0x0800_0201, FalseKind::WrongMode)
        ]
    );
    assert_eq!(miss_addrs(&s), [0x0800_0101, 0x0800_0200, 0x0800_0301]);
}

#[test]
fn start_inside_a_true_function() {
    // ThumbFn covers 0x08000100..0x08000140; ArmFn 0x08000200..0x08000210.
    let s = run("0x08000121\n0x0800013f\n0x08000208\n0x08000141\n");
    assert_eq!(
        kinds(&s),
        [
            (0x0800_0121, FalseKind::Inside),
            (0x0800_013f, FalseKind::Inside),
            (0x0800_0141, FalseKind::Elsewhere),
            (0x0800_0208, FalseKind::Inside),
        ]
    );
    assert_eq!(
        s.false_starts[0].near,
        Some((0x0800_0101, "ThumbFn".into()))
    );
    assert!(s.report(5).contains("0x08000121 inside (ThumbFn+0x20)"));
}

#[test]
fn nested_function_does_not_hide_its_container() {
    let t = parse_truth("0x08000001\t256\tBig\n0x08000011\t4\tSmall\n").unwrap();
    let d = parse_discovered("0x08000081\n").unwrap();
    let s = score(&t, &d).unwrap();
    assert_eq!(kinds(&s), [(0x0800_0081, FalseKind::Inside)]);
    assert_eq!(s.false_starts[0].near, Some((0x0800_0001, "Big".into())));
}

#[test]
fn size_zero_row_contains_nothing() {
    // Label is a size-0 Thumb row at 0x08000300 with nothing after it, so
    // only a too-wide extent for size 0 would call these inside.
    let s = run("0x08000303\n0x08000304\n");
    assert_eq!(
        kinds(&s),
        [
            (0x0800_0303, FalseKind::Elsewhere),
            (0x0800_0304, FalseKind::Elsewhere)
        ]
    );
    assert_eq!(s.false_starts[0].near, Some((0x0800_0301, "Label".into())));
}

#[test]
fn duplicates_collapse() {
    let s = run("0x08000101\n08000101\n0X08000101\n0x08000101 # again\n");
    assert_eq!((s.matched, s.discovered), (1, 1));
    assert!(
        s.report(5)
            .starts_with("precision 1.0000 (1/1)\nrecall 0.3333 (1/3)\n")
    );
}

#[test]
fn comments_blank_lines_and_hex_forms() {
    let d = parse_discovered(
        "# header\n\n   \n0x08000101  # ThumbFn\n  080002A0\n\t# indented\n0X08000301\n",
    )
    .unwrap();
    assert_eq!(
        d.in_rom.iter().copied().collect::<Vec<_>>(),
        [0x0800_0101, 0x0800_02a0, 0x0800_0301]
    );
    let t = parse_truth("# comment\n\n0x08000101\t64\tThumbFn  # note\n").unwrap();
    assert_eq!(t[0].name, "ThumbFn");
    assert!(parse_truth("0x08000101\t64\n").is_err());
    assert!(parse_discovered("+8000101\n").is_err());
    assert!(parse_discovered("0x08000101\nnot-hex\n").is_err_and(|e| e.line == 2));
}

#[test]
fn out_of_rom_starts_are_counted_not_scored() {
    // IWRAM (Thumb and ARM), BIOS, and the first address past ROM.
    let s = run("0x03000001\n0x03000100\n0x00000000\n0x0a000001\n0x08000101\n");
    assert_eq!(
        s.outside,
        [0x0000_0000, 0x0300_0001, 0x0300_0100, 0x0a00_0001]
    );
    assert_eq!((s.matched, s.discovered), (1, 1));
    assert!(s.false_starts.is_empty());
    assert!(s.report(5).contains("ignored outside ROM: 4 "));
    // The last ROM address counts as in ROM.
    assert!(in_rom(0x09ff_fffd) && !in_rom(0x0a00_0000) && !in_rom(0x07ff_ffff));
}

#[test]
fn misaligned_arm_start_fails() {
    let d = parse_discovered("0x08000101\n0x08000202\n").unwrap();
    assert_eq!(score(&truth(), &d), Err(Misaligned(vec![0x0800_0202])));
    // Thumb starts only need 2-alignment, which bit 0 always gives.
    let d = parse_discovered("0x08000103\n").unwrap();
    assert!(score(&truth(), &d).is_ok());
}

#[test]
fn truth_scores_perfectly_against_itself() {
    let disc: String = TRUTH
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let s = run(&disc);
    assert!(
        s.report(5)
            .starts_with("precision 1.0000 (3/3)\nrecall 1.0000 (3/3)\n")
    );
}

fn sym(name: &str, value: u32, size: u64, kind: SymKind, global: bool, section: usize) -> Sym {
    Sym {
        name: name.into(),
        value,
        size,
        kind,
        global,
        section: Some(section),
        exec: section == 1,
    }
}

#[test]
fn truth_rules() {
    use SymKind::*;
    // Section 1 is executable, section 2 isn't.
    let syms = [
        sym("$a", 0x0800_0000, 0, NoType, false, 1),
        sym("Start", 0x0800_0000, 0, NoType, true, 1),
        sym("$t", 0x0800_0100, 0, NoType, false, 1),
        sym("ThumbFn", 0x0800_0101, 64, Func, true, 1),
        sym("ThumbAlias", 0x0800_0101, 64, Func, true, 1),
        sym("$t.1", 0x0800_0140, 0, NoType, false, 1),
        sym("AsmThumb", 0x0800_0140, 0, NoType, true, 1),
        // Rule 2 needs GLOBAL, an executable section and a mapping symbol.
        sym("LocalLabel", 0x0800_0140, 0, NoType, false, 1),
        sym("NoMapping", 0x0800_0150, 0, NoType, true, 1),
        sym("$d", 0x0800_0160, 0, NoType, false, 2),
        sym("$a", 0x0800_0160, 0, NoType, false, 2),
        sym("DataLabel", 0x0800_0160, 0, NoType, true, 2),
        sym("gData", 0x0800_0170, 4, Other, true, 1),
        // Rule 1 is ROM only; IWRAM functions are dropped.
        sym("IwramFn", 0x0300_0001, 8, Func, true, 3),
        sym("ArmFn", 0x0800_0200, 16, Func, true, 1),
    ];
    let t = build_truth(&syms);
    assert_eq!(
        format_truth(&t.rows),
        "0x08000000\t0\tStart\n0x08000101\t64\tThumbFn\n0x08000141\t0\tAsmThumb\n0x08000200\t16\tArmFn\n"
    );
    assert_eq!(t.rule2_added, ["Start", "AsmThumb"]);
    // The generator's output reads back unchanged.
    assert_eq!(parse_truth(&format_truth(&t.rows)).unwrap(), t.rows);
}

#[test]
fn rule2_name_on_an_existing_function_adds_nothing() {
    use SymKind::*;
    let syms = [
        sym("$t", 0x0800_0100, 0, NoType, false, 1),
        sym("ThumbFn", 0x0800_0101, 64, Func, true, 1),
        sym("ThumbFnLabel", 0x0800_0100, 0, NoType, true, 1),
    ];
    let t = build_truth(&syms);
    assert_eq!(t.rows.len(), 1);
    assert_eq!(t.rows[0].name, "ThumbFn");
    assert!(t.rule2_added.is_empty());
}

#[test]
fn label_before_its_function_keeps_the_function_size() {
    use SymKind::*;
    let syms = [
        sym("$t", 0x0800_0100, 0, NoType, false, 1),
        sym("AsmEntry", 0x0800_0100, 0, NoType, true, 1),
        sym("ThumbFn", 0x0800_0101, 64, Func, true, 1),
    ];
    let t = build_truth(&syms);
    assert_eq!(
        t.rows,
        [TruthRow {
            addr: 0x0800_0101,
            size: 64,
            name: "AsmEntry".into()
        }]
    );
    assert!(t.rule2_added.is_empty());
}
