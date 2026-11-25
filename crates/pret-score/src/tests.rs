use super::*;

const LIFT: &str = "\
use crate::*;

/// Reads {r0}; returns {}.
pub fn UpdateTrack(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> { x_08001851(g, Regs::call(sp, a)) }
pub fn x_08001851(g: &mut Game, s: Regs) -> Result<[u32; 4], Exit> {
    let mut r0 = s.r[0];
    // dwarf r0: var_1
                // src/audio.c:661
                let v2: u32 = r13;

                let v4: u32 = v2;
                // src/audio.c:668
                if v4 == 0 {
                // src/audio.c:661
                r0 = 1;
                }
    Ok([r0, 0, 0, 0])
}
pub fn x_08001bf1(g: &mut Game, s: Regs) -> (u32, u32) { unk_1bf0(g, s.r[13], s.r[0]) }
pub fn unk_1bf0(g: &mut Game, mut r13: u32, p: u32) -> (u32, u32) {
                // src/audio.c:700
    (p, r13)
}
fn helper() -> u32 {
    3
}
";

#[test]
fn functions_are_named_and_their_lines_counted() {
    let f = fn_lines([LIFT]);
    assert_eq!(
        f,
        vec![
            FnLines {
                name: "UpdateTrack".into(),
                lines: 7,
                source_lines: 2
            },
            FnLines {
                name: "unk_1bf0".into(),
                lines: 1,
                source_lines: 1
            },
        ]
    );
}

#[test]
fn the_median_and_the_worst() {
    let f = |n: &str, l, s| FnLines {
        name: n.into(),
        lines: l,
        source_lines: s,
    };
    let (m, w) = worst(vec![f("a", 4, 1), f("b", 2, 1), f("c", 9, 3), f("d", 1, 1)]);
    assert_eq!(m, 2.5);
    assert_eq!(w[0].name, "a");
    assert_eq!(w[1].name, "c");
    let (m, _) = worst(vec![f("a", 4, 1), f("b", 2, 1), f("c", 9, 3)]);
    assert_eq!(m, 3.0);
}

#[test]
fn the_map_offers() {
    let map = r#"
[map]
rom_sha1 = "x"
[functions]
0x080003a5 = { name = "AgbMain", size = 288 }
0x080004c5 = { name = "Other", size = 20 }
[locals]
0x08000541 = [
  { name = "callback", reg = 0 },
]
0x08000685 = [
  { name = "i", reg = 1 },
  { name = "dmaRegs", reg = 0 },
]
[lines]
0x080003a4 = 90
0x080003aa = 94
[values."ABILITY"]
file = "a.h"
names = [
  { name = "ABILITY_NONE", value = 0 },
  { name = "ABILITY_STENCH", value = 1 },
]
[values."B"]
names = [{ name = "B_X", value = 0 }]
"#;
    assert_eq!(
        offer(map, 7).unwrap(),
        Offer {
            functions: 2,
            lines: 2,
            locals: 3,
            values: 3,
            c_lines: 7
        }
    );
}

#[test]
fn the_report_names_its_functions() {
    let r = "x\nnames (gbaa-lift --names): 2612 of 2624 functions named; 1934 data named\n";
    assert_eq!(functions_named(r), Some((2612, 2624)));
    assert_eq!(functions_named("nothing"), None);
}

fn score(lines: u64, median: f64) -> Score {
    Score {
        name: "g".into(),
        note: "a note".into(),
        lift: [("lines".to_string(), lines), ("line_notes".to_string(), 5)]
            .into_iter()
            .collect(),
        functions_named: 3,
        functions: 4,
        offer: Offer {
            functions: 4,
            lines: 10,
            locals: 0,
            values: 0,
            c_lines: 50,
        },
        run: String::new(),
        fns_noted: 2,
        median,
        worst: vec![FnLines {
            name: "Worst".into(),
            lines: 9,
            source_lines: 1,
        }],
        results: Results {
            found: 3,
            void: 2,
            void_agree: 1,
            one: 1,
            one_agree: 1,
            several: 1,
            four: 0,
        },
    }
}

#[test]
fn results_are_scored_against_the_prototypes() {
    let map = r#"
[functions]
0x08000101 = { name = "Draw", proto = "void Draw(u8 x)" }
0x08000201 = { name = "Get", proto = "u16 *Get(void)" }
0x08000301 = { name = "Mix", proto = "void Mix(void)" }
0x08000401 = { name = "Big", proto = "u64 Big(void)" }
0x08000501 = { name = "Lone" }
0x08000601 = { name = "Four", proto = "s32 Four(void)" }
"#;
    let lift = "\
pub fn Draw(g: &mut Game, sp: u32, a0: u32) -> Result<(), Exit> { x_08000101(g, Regs::call(sp, [a0, 0, 0, 0])) }
pub fn x_08000101(g: &mut Game, s: Regs) -> Result<(), Exit> {
}
pub fn Get(g: &mut Game, r13: u32) -> u32 {
}
pub fn Mix(g: &mut Game, r13: u32) -> (u32, u32) {
}
pub fn Big(g: &mut Game, r13: u32) -> (u32, u32) {
}
pub fn Lone(g: &mut Game, r13: u32) -> u32 {
}
pub fn Four(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> { x_08000601(g, Regs::call(sp, a)) }
";
    let r = results(map, [lift]).unwrap();
    assert_eq!(
        r,
        Results {
            found: 5,
            void: 2,
            void_agree: 1,
            one: 2,
            one_agree: 1,
            several: 2,
            four: 1,
        }
    );
}

#[test]
fn the_table_against_another_run() {
    let now = Summary {
        commit: "b".into(),
        game: vec![score(100, 2.0)],
    };
    let was = Summary {
        commit: "a".into(),
        game: vec![score(120, 2.5)],
    };
    let t = table(&now, Some(&was));
    assert!(
        t.contains("lines / decomp's C lines           100 (-20) / 50 (2.00x)"),
        "{t}"
    );
    assert!(
        t.contains("line notes / [lines]               5 (0) / 10 (50.0%)"),
        "{t}"
    );
    assert!(t.contains("2.00 of 2 fns (-0.50)"), "{t}");
    assert!(
        t.contains("built and run                      not run"),
        "{t}"
    );
    assert!(
        t.contains("results void, one / prototypes     1 (0) of 2, 1 (0) of 1 / 3 (66.7%)"),
        "{t}"
    );
    assert!(t.contains("g: a note"), "{t}");
    assert!(t.contains("9.0  Worst (9 lines, 1 source lines)"), "{t}");
}

#[test]
fn the_summary_reads_back() {
    let s = Summary {
        commit: "a".into(),
        game: vec![score(1, 1.5)],
    };
    let t = toml::to_string(&s).unwrap();
    assert_eq!(toml::from_str::<Summary>(&t).unwrap(), s);
}
