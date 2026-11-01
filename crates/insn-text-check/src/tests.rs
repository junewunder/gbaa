use super::*;

#[test]
fn objdump_lines_parse_with_their_state() {
    let l = parse_line(" 8000000:\tea00002e \tb\t0x80000c0").unwrap();
    assert_eq!((l.addr, l.enc), (0x0800_0000, Enc::Arm(0xEA00_002E)));
    assert_eq!(l.text, "b 0x80000c0");
    let l = parse_line(" 8000374:\tf000 fadc \tbl\t0x8000930").unwrap();
    assert_eq!(l.enc, Enc::Pair(0xF000, 0xFADC));
    assert_eq!(ours(&l), Ours::Text("bl 0x08000930".into()));
    assert_eq!(parse_line("08000000 <Start>:"), None);
}

#[test]
fn normalising_takes_out_objdumps_style() {
    for (ours, theirs) in [
        ("ldr r0, [pc, #36]", "ldr\tr0, [pc, #36]\t@ (0x8000374)"),
        (
            "orrsvs r12, r10, r3, lsr #21",
            "orrsvs\tip, sl, r3, lsr #21",
        ),
        ("mov r0, #0x40000000", "mov\tr0, #1073741824\t@ 0x40000000"),
        ("cmpvc sp, #0x9f000000", "cmpvc\tsp, #-1627389952"),
        ("mvncc r12, #0x1600000", "mvncc\tip, #88, 14"),
        ("adds r0, #1", "adds\tr0, r0, #1"),
        ("beq 0x08000414", "beq.n\t8000414 <AgbMain+0x70>"),
        ("ldm sp!, {r4}", "ldmfd\tsp!, {r4}"),
        ("ldr r0, [sp], #4", "pop\t{r0}\t\t@ (ldr r0, [sp], #4)"),
        ("mov r0, r0", "nop\t\t\t@ (mov r0, r0)"),
        ("ldr r0, [r1, #0]!", "ldr\tr0, [r1, #-0]!"),
        ("svc 0x5", "svc\t0x00000005"),
    ] {
        assert_eq!(normalise(ours), normalise(theirs), "{theirs}");
    }
    assert_ne!(normalise("cmp pc, r0"), normalise("cmpp pc, r0"));
    assert_ne!(
        normalise("ldr r0, [r1, #4]"),
        normalise("ldr r0, [r1, #-4]")
    );
}

#[test]
fn an_it_block_and_strict_bits_are_not_mismatches() {
    let mut t = Tally::default();
    for l in [
        " 8000036:\tbfcc      \titt\tgt",
        " 8000038:\t4bce      \tldrgt\tr3, [pc, #824]",
        " 800003a:\tc109      \tstmiagt\tr1!, {r0, r3}",
        " 8002230:\t63630000 \tcmnvs\tr3, #0",
    ] {
        t.add(&parse_line(l).unwrap());
    }
    assert_eq!(
        (t.not_v4t, t.should_be.len(), t.mismatched.len()),
        (3, 1, 0)
    );
}
