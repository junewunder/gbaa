//! The lifter on a compiler's trap (native/lift-undefined-insns): a tiny
//! hand-assembled program whose one Thumb function ends a path the way
//! modern GCC does a null dereference, `movs r3, #0; ldr r3, [r3]; udf`.
//! No lift-check program has a trap in lifted code, so this is where the
//! printed stop is seen.

use codegen::native::{Holes, Lifted, lift};

/// The program, its `udf` halfword `udf`:
///
/// ```text
/// 08000000  b 0x080000c0        (the header's branch)
/// 080000c0  add r0, pc, #1
/// 080000c4  bx r0
/// 080000c8  movs r3, #0         (Thumb from here)
/// 080000ca  cmp r0, #0
/// 080000cc  bne 0x080000d2
/// 080000ce  ldr r3, [r3, #0]
/// 080000d0  udf
/// 080000d2  b 0x080000d2
/// ```
fn lifted(udf: u16) -> Lifted {
    let mut rom = vec![0u8; 0xd4];
    rom[..4].copy_from_slice(&0xEA00_002Eu32.to_le_bytes());
    rom[0xc0..0xc4].copy_from_slice(&0xE28F_0001u32.to_le_bytes());
    rom[0xc4..0xc8].copy_from_slice(&0xE12F_FF10u32.to_le_bytes());
    for (i, h) in [0x2300, 0x2800, 0xd101, 0x681b, udf, 0xe7fe]
        .into_iter()
        .enumerate()
    {
        rom[0xc8 + 2 * i..0xca + 2 * i].copy_from_slice(&h.to_le_bytes());
    }
    lift(&rom, "trap", &Holes::default())
}

fn undefined(l: &Lifted) -> Vec<String> {
    let holes = l.holes.iter().map(|h| h.to_string());
    holes.filter(|h| h.contains("undefined")).collect()
}

/// GCC's `udf #255` and LLVM's `udf #254` are no hole: the lift prints a
/// stop naming the address, in a crate that parses as Rust.
#[test]
fn a_compilers_trap_lifts_as_a_stop_not_a_hole() {
    for udf in [0xdeff, 0xdefe] {
        let l = lifted(udf);
        assert_eq!(undefined(&l), Vec::<String>::new(), "{udf:#06x}");
        let stops: Vec<&str> = l
            .files
            .iter()
            .flat_map(|(_, text)| text.lines())
            .filter(|line| line.contains("a compiler's trap"))
            .collect();
        assert_eq!(stops.len(), 1, "{udf:#06x}: {stops:?}");
        assert!(stops[0].trim_start().starts_with("panic!(\"0x080000d0:"));
        for (path, text) in &l.files {
            syn::parse_file(text).unwrap_or_else(|e| panic!("{path}: {e}"));
        }
    }
}

/// Any other undefined instruction (`udf #0`) is still a hard hole.
#[test]
fn another_undefined_instruction_is_still_a_hard_hole() {
    let l = lifted(0xde00);
    let holes = undefined(&l);
    assert_eq!(
        holes.len(),
        1,
        "{:?}",
        l.holes.iter().map(|h| h.to_string()).collect::<Vec<_>>()
    );
    assert!(holes[0].contains("at 0x080000d0: an undefined instruction"));
    assert!(!l.files.iter().any(|(_, t)| t.contains("a compiler's trap")));
}
