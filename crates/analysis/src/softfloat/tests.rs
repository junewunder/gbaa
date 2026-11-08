use super::*;

fn sig(routine: Routine, entry: u32, code: &[u8]) -> Signature {
    Signature {
        routine,
        source: "test",
        entry,
        len: code.len() as u32,
        hash: fnv1a(code),
        first: u32::from_le_bytes(code[..4].try_into().unwrap()),
    }
}

#[test]
fn found_wherever_it_lies_and_only_at_arm_entries() {
    let code: Vec<u8> = (0..32u8).collect();
    let sigs = [
        sig(Routine::Mul, 0, &code[..16]),
        sig(Routine::Div, 16, &code),
    ];
    let mut image = vec![0xffu8; 8];
    image.extend_from_slice(&code);
    let base = 0x0800_0000;
    let mul = recognise_in(&image, base, base + 8, &sigs).expect("mul");
    assert_eq!(mul.routine, Routine::Mul);
    assert_eq!((mul.start, mul.end), (base + 8, base + 24));
    let div = recognise_in(&image, base, base + 24, &sigs).expect("div");
    assert_eq!(div.routine, Routine::Div);
    assert_eq!((div.start, div.end), (base + 8, base + 40));
    assert!(
        recognise_in(&image, base, base + 9, &sigs).is_none(),
        "Thumb"
    );
    assert!(recognise_in(&image, base, base + 4, &sigs).is_none());
    // Past the image's end, or before its start.
    assert!(recognise_in(&image[..30], base, base + 24, &sigs).is_none());
    assert!(recognise_in(&image, base + 0x10, base + 0x10, &sigs).is_none());
}

#[test]
fn the_real_signatures_match_nothing_blank() {
    let image = vec![0u8; 0x1000];
    assert!(scan(&image, 0x0800_0000).is_empty());
}

#[test]
fn scan_finds_both_where_they_lie() {
    let code: Vec<u8> = (0..32u8).collect();
    let sigs = [
        sig(Routine::Mul, 0, &code[..16]),
        sig(Routine::Div, 16, &code),
    ];
    let mut image = vec![0xffu8; 12];
    image.extend_from_slice(&code);
    image.extend_from_slice(&code[..16]);
    let base = 0x0800_0000;
    let found = scan_in(&image, base, &sigs);
    let got: Vec<(Routine, u32)> = found.iter().map(|f| (f.routine, f.entry - base)).collect();
    assert_eq!(
        got,
        [(Routine::Mul, 12), (Routine::Div, 28), (Routine::Mul, 44)]
    );
}

#[test]
fn signatures_are_well_formed() {
    for s in SIGNATURES {
        assert!(s.len >= 4 && s.entry < s.len && s.entry % 4 == 0, "{s:?}");
    }
}
