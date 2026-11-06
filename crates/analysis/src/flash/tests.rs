use super::*;

#[test]
fn fnv1a_known_values() {
    assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
}

#[test]
fn read_byte_is_ldrb_then_bx_lr() {
    // `ldrb r0, [r0]; bx lr`, then whatever the stack held.
    let code = [0x00, 0x78, 0x70, 0x47, 0xc5, 0x00, 0x00, 0x00];
    let s = recognise(&code).expect("recognised");
    assert_eq!(s.routine, Routine::ReadByte);
    assert_eq!(s.len, 4);
    assert!(recognise(&code[..3]).is_none(), "shorter than the routine");
    assert!(recognise(&[0x01, 0x78, 0x70, 0x47]).is_none());
}

#[test]
fn longest_match_wins() {
    let long = [1u8, 2, 3, 4, 5, 6];
    let sigs = [
        Signature {
            routine: Routine::ReadByte,
            compiler: "test",
            len: 2,
            hash: fnv1a(&long[..2]),
        },
        Signature {
            routine: Routine::Read,
            compiler: "test",
            len: 6,
            hash: fnv1a(&long),
        },
    ];
    assert_eq!(
        recognise_in(&long, &sigs).map(|s| s.routine),
        Some(Routine::Read)
    );
    assert_eq!(
        recognise_in(&long[..4], &sigs).map(|s| s.routine),
        Some(Routine::ReadByte)
    );
    assert!(recognise_in(&[9, 9, 9], &sigs).is_none());
}

#[test]
fn signatures_fit_a_ram_call_and_differ() {
    for (i, a) in SIGNATURES.iter().enumerate() {
        assert!(a.len <= crate::holes::RAM_CALL_BYTES, "{a:?}");
        for b in &SIGNATURES[i + 1..] {
            assert!((a.len, a.hash) != (b.len, b.hash), "{a:?} {b:?}");
        }
    }
}
