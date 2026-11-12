use super::*;
use crate::discovery::{Code, Evidence};
use crate::vsa::func::Jump;

const B: u32 = ROM_BASE;

fn put16(rom: &mut [u8], a: u32, h: u16) {
    let o = (a - B) as usize;
    rom[o..o + 2].copy_from_slice(&h.to_le_bytes());
}

fn put32(rom: &mut [u8], a: u32, w: u32) {
    let o = (a - B) as usize;
    rom[o..o + 4].copy_from_slice(&w.to_le_bytes());
}

fn thumb(disc: &mut Discovery, a: u32, size: u32) {
    disc.code.insert(
        a,
        Code {
            size,
            thumb: true,
            evidence: Evidence::Confirmed,
        },
    );
}

/// libgcc's `__gnu_thumb1_case_uqi` at 0x40, called by a `bl` at 0x06.
fn case_helper() -> (Vec<u8>, Discovery) {
    let mut rom = vec![0u8; 0x100];
    let mut d = Discovery::default();
    put16(&mut rom, B, 0x2803); // cmp r0, #3
    thumb(&mut d, B, 2);
    put16(&mut rom, B + 6, 0xF000); // bl 0x40
    put16(&mut rom, B + 8, 0xF81B);
    thumb(&mut d, B + 6, 4);
    let helper = [
        0xb402, 0x4671, 0x0849, 0x0049, 0x5c09, 0x0049, 0x448e, 0xbc02, 0x4770,
    ];
    for (k, h) in helper.into_iter().enumerate() {
        put16(&mut rom, B + 0x40 + 2 * k as u32, h);
        thumb(&mut d, B + 0x40 + 2 * k as u32, 2);
    }
    d.funcs.insert(B | 1, Evidence::Confirmed);
    d.funcs.insert((B + 0x40) | 1, Evidence::Confirmed);
    (rom, d)
}

#[test]
fn case_helper_resolved_where_recovery_resolves_the_bl() {
    let (rom, mut d) = case_helper();
    d.rec.jumps.insert(
        B + 6,
        JumpRes::Resolved(Jump {
            targets: vec![B + 0x10, B + 0x14],
            table: None,
        }),
    );
    let r = classify(&rom, &d, &[], &Run::default(), None);
    assert_eq!(r.ret.len(), 1);
    assert_eq!(r.ret[0].func, (B + 0x40) | 1);
    assert_eq!(r.ret[0].at, B + 0x40 + 12); // add lr, r1
    assert_eq!(r.ret[0].sites, vec![B + 6]);
    assert!(r.ret[0].resolved);
    assert_eq!(r.hard(), 0);
}

#[test]
fn case_helper_unresolved_without_recovery() {
    let (rom, d) = case_helper();
    let r = classify(&rom, &d, &[], &Run::default(), None);
    assert_eq!(r.hard_ret(), 1);
}

#[test]
fn unresolved_jumps_are_recoverys() {
    let (rom, mut d) = case_helper();
    d.rec.jumps.insert(B + 0x20, JumpRes::Unresolved);
    d.rec.jumps.insert(
        B + 0x24,
        JumpRes::Rejected(Jump {
            targets: vec![],
            table: None,
        }),
    );
    d.rec.jumps.insert(
        B + 6,
        JumpRes::Resolved(Jump {
            targets: vec![B + 0x10],
            table: None,
        }),
    );
    let r = classify(&rom, &d, &[], &Run::default(), None);
    assert_eq!(r.unresolved.len(), 2);
    assert!(r.unresolved[1].rejected);
    assert_eq!(r.hard(), 2);
}

#[test]
fn return_arithmetic_seen_in_the_run() {
    // WarioWare's IWRAM helper: `bleq 0x3000468`, which returns to lr - 24.
    let rom = vec![0u8; 0x10];
    let mut run = Run::default();
    run.code.insert(
        0x0300_046c,
        Seen {
            raw: 0xe24e_e018,
            after_boot: true,
            ..Seen::default()
        },
    );
    run.ret_arith.insert(
        (0x0300_0444, 0x0300_0430),
        RetArith {
            site: 0x0300_0444,
            callee: 0x0300_0468,
            at: 0x0300_0490,
            to: 0x0300_0430,
            delta: -24,
            count: 5,
        },
    );
    run.call_targets.insert(0x0300_0468);
    run.call_targets.insert(0x0300_03a8);
    let r = classify(&rom, &Discovery::default(), &[], &run, None);
    assert_eq!(r.hard_ret(), 1);
    assert_eq!(r.ret[0].func, 0x0300_0468);
    assert_eq!(r.ret[0].delta, Some(-24));
    // Its code wasn't discovered, and didn't change.
    assert_eq!(r.ram_undiscovered_bytes, 4);
    assert!(r.ram_changed.is_empty());
}

#[test]
fn msr_in_the_wrapper_at_boot_and_elsewhere() {
    let mut rom = vec![0u8; 0x100];
    let mut d = Discovery::default();
    let msr = 0xe321_f01f; // msr cpsr_c, #0x1f
    for a in [B, B + 0x40, B + 0x80, B + 0x84] {
        put32(&mut rom, a, msr);
        d.code.insert(
            a,
            Code {
                size: 4,
                thumb: false,
                evidence: Evidence::Confirmed,
            },
        );
    }
    for f in [B, B + 0x40, B + 0x80] {
        d.funcs.insert(f, Evidence::Confirmed);
    }
    let mut run = Run::default();
    run.code.insert(
        B,
        Seen {
            raw: msr,
            mode_change: true,
            ..Seen::default()
        },
    );
    run.code.insert(
        B + 0x40,
        Seen {
            raw: msr,
            wrapper: true,
            after_boot: true,
            ..Seen::default()
        },
    );
    run.code.insert(
        B + 0x80,
        Seen {
            raw: msr,
            after_boot: true,
            ..Seen::default()
        },
    );
    let r = classify(&rom, &d, &[], &run, None);
    let places: Vec<(CpuWhere, CpuKind, bool)> =
        r.cpu.iter().map(|c| (c.place, c.kind, c.reached)).collect();
    assert_eq!(
        places,
        vec![
            (CpuWhere::Boot, CpuKind::Msr, true),
            (CpuWhere::Wrapper, CpuKind::MsrMask, true),
            (CpuWhere::Other, CpuKind::MsrMask, true),
            (CpuWhere::Other, CpuKind::Msr, false),
        ]
    );
    assert_eq!(r.hard(), 2);
}

#[test]
fn ram_code_that_changed_and_where_it_came_from() {
    let mut rom = vec![0u8; 0x40];
    let words = [0xe3a0_0001u32, 0xe280_0001, 0xe12f_ff1e];
    for (i, w) in words.iter().enumerate() {
        put32(&mut rom, B + 0x20 + 4 * i as u32, *w);
    }
    let mut run = Run::default();
    for (i, w) in words.iter().enumerate() {
        let changed = i == 1;
        run.code.insert(
            0x0300_0100 + 4 * i as u32,
            Seen {
                raw: *w,
                changed,
                ..Seen::default()
            },
        );
    }
    let r = classify(&rom, &Discovery::default(), &[], &run, None);
    assert_eq!(r.ram_changed.len(), 1);
    assert_eq!(
        r.ram_undiscovered,
        vec![RamRun {
            start: 0x0300_0100,
            end: 0x0300_010c,
            rom_src: Some(B + 0x20)
        }]
    );
    assert_eq!(r.ram_undiscovered_bytes, 12);
}

/// The flash library's `ReadFlash1` (`ldrb r0, [r0]; bx lr`) called at
/// 0x03007d48, where other code ran before: the call stands in, and its
/// RAM isn't a hole; an unrecognised routine called at 0x03007d60 still is.
#[test]
fn flash_stack_routines_are_library_calls() {
    let mut run = Run::default();
    for (a, raw, changed) in [
        (0x0300_7d48, 0x7800, true),
        (0x0300_7d4a, 0x4770, true),
        (0x0300_7d60, 0x7801, true),
        (0x0300_7d62, 0x4770, false),
    ] {
        run.code.insert(
            a,
            Seen {
                thumb: true,
                raw,
                changed,
                after_boot: true,
                ..Seen::default()
            },
        );
    }
    let mut stack = vec![0x00, 0x78, 0x70, 0x47];
    stack.resize(RAM_CALL_BYTES as usize, 0);
    run.ram_calls
        .entry(0x0300_7d49)
        .or_default()
        .insert(stack.clone());
    stack[0] = 0x01;
    run.ram_calls.entry(0x0300_7d61).or_default().insert(stack);
    run.indirect_calls.insert(0x0300_7d49, B + 0x10);
    run.indirect_calls.insert(0x0300_7d61, B + 0x10);
    let r = classify(&[0; 0x40], &Discovery::default(), &[], &run, None);
    assert_eq!(
        r.flash,
        vec![FlashCall {
            target: 0x0300_7d49,
            routine: crate::flash::Routine::ReadByte,
            len: 4,
        }]
    );
    assert_eq!(r.flash_routines(), 1);
    assert_eq!(
        r.ram_changed,
        vec![RamRun {
            start: 0x0300_7d60,
            end: 0x0300_7d62,
            rom_src: None
        }]
    );
    assert_eq!(r.indirect_non_entry, vec![(0x0300_7d61, B + 0x10)]);
    assert_eq!(r.ram_undiscovered_bytes, 4);
    assert_eq!(r.reached_bytes, 8);
}

#[test]
fn bios_and_summaries() {
    assert!(bios_implemented(0x0b) && bios_implemented(0x05) && bios_implemented(0x11));
    assert!(bios_implemented(0x0e) && bios_implemented(0x0f));
    // In the play build's BIOS but not the lifter's prelude: counted.
    assert!(!bios_implemented(0x01) && !bios_implemented(0x13) && !bios_implemented(0x10));
    assert!(!bios_implemented(0x26) && !bios_implemented(0x2b) && !bios_implemented(0x102));
    assert_eq!(
        summary_kind("callee-saves: 0x08001234: returns with sp changed: sp = unknown"),
        "returns with sp changed"
    );
    assert_eq!(
        summary_kind("0x08001234: reads r4-r11 at entry: r5"),
        "reads r4-r11 at entry"
    );
    assert!(summary_hard(&summary_kind(
        "0x08001234: returns with sp changed: r4 = unknown, sp = entry sp-4"
    )));
    assert!(summary_hard(&summary_kind(
        "0x08001234: reads r4-r11 a callee changed: r4"
    )));
    // Scratch r4-r11 alone is soft: the reads are the holes.
    assert!(!summary_hard(&summary_kind(
        "0x08001234: returns with r4-r11 changed: r4 = constant 0x3"
    )));
    assert!(!summary_hard("an indirect call"));
    // A read whose bits survive is hard, passed on or not; one masked
    // away, the caller's frame pointer and a function only jumped into
    // are soft (codegen::saves).
    assert!(summary_hard(&summary_kind(
        "0x08001234: reads r4-r11 at entry: r8, passed in r0"
    )));
    for soft in [
        "0x08001234: masks away r4-r11's entry bits: r4",
        "0x08001234: reads the caller's frame pointer: r7",
        "0x08001234: reads r4-r11 at entry, only jumped into: r6, passed in r1",
    ] {
        assert!(!summary_hard(&summary_kind(soft)), "{soft}");
    }
    let mut sums = BTreeMap::new();
    sums.insert(
        B,
        vec!["0x08000010: returns with sp changed: sp = unknown".to_string()],
    );
    sums.insert(B + 0x40, vec!["0x08000050: an indirect call".to_string()]);
    // Two hard kinds in one function count it once.
    sums.insert(
        B + 0x80,
        vec![
            "0x08000080: reads r4-r11 at entry: r4".to_string(),
            "0x08000081: changes r4-r11 and may be called through a pointer".to_string(),
        ],
    );
    let r = classify(
        &[0u8; 4],
        &Discovery::default(),
        &[],
        &Run::default(),
        Some(&sums),
    );
    assert_eq!(r.hard_summary(), 2);
    assert_eq!(r.summary.len(), 4);
}

#[test]
fn jumps_near_the_return_site_that_arent_returns() {
    // A veneer's jump to a function start, and a table jump inside the
    // callee's own function, both landing near the caller's return site.
    let mut run = Run::default();
    run.call_targets.extend([0x0300_0100, 0x0300_0200]);
    let near = |site, callee, to| RetArith {
        site,
        callee,
        at: callee + 4,
        to,
        delta: 8,
        count: 1,
    };
    run.ret_arith.insert(
        (0x0300_00f0, 0x0300_0200),
        near(0x0300_00f0, 0x0300_0100, 0x0300_0200),
    );
    run.ret_arith.insert(
        (0x0300_01f0, 0x0300_0240),
        near(0x0300_01f0, 0x0300_0200, 0x0300_0240),
    );
    let r = classify(&[0u8; 4], &Discovery::default(), &[], &run, None);
    assert!(r.ret.is_empty());
}

#[test]
fn a_rejected_bl_counts_once() {
    // The case helper's `bl` rejected by Recovery: counted as a jump, not
    // again as return arithmetic.
    let (rom, mut d) = case_helper();
    d.rec.jumps.insert(
        B + 6,
        JumpRes::Rejected(Jump {
            targets: vec![],
            table: None,
        }),
    );
    let r = classify(&rom, &d, &[], &Run::default(), None);
    assert_eq!(r.ret.len(), 1);
    assert!(r.ret[0].as_jump);
    assert_eq!(r.hard(), 1);
}

#[test]
fn a_rom_source_and_its_ram_copy_count_once() {
    // An msr in ROM code the game copies to IWRAM itself and runs there.
    let mut rom = vec![0u8; 0x40];
    let words = [0xe3a0_0001u32, 0xe321_f01f, 0xe12f_ff1e]; // mov; msr; bx lr
    let mut d = Discovery::default();
    for (i, w) in words.iter().enumerate() {
        let a = B + 0x20 + 4 * i as u32;
        put32(&mut rom, a, *w);
        d.code.insert(
            a,
            Code {
                size: 4,
                thumb: false,
                evidence: Evidence::Confirmed,
            },
        );
    }
    let mut run = Run::default();
    for (i, w) in words.iter().enumerate() {
        let s = Seen {
            raw: *w,
            after_boot: true,
            mode_change: i == 1,
            ..Seen::default()
        };
        run.code.insert(0x0300_0100 + 4 * i as u32, s);
    }
    let r = classify(&rom, &d, &[], &run, None);
    assert_eq!(r.cpu.len(), 1);
    assert_eq!(r.cpu[0].at, 0x0300_0104);
}

/// A Thumb leaf (`movs r1, #5; movs r0, #1; adds r0, #2; bx lr`) at
/// ROM 0x80, and a run that executed it at `dst`, entered by a call when
/// `called`.
fn copied_leaf(dst: u32, called: bool) -> (Vec<u8>, Run) {
    let mut rom = vec![0u8; 0x100];
    let leaf = [0x2105u16, 0x2001, 0x3002, 0x4770];
    let mut run = Run::default();
    for (k, h) in leaf.into_iter().enumerate() {
        put16(&mut rom, B + 0x80 + 2 * k as u32, h);
        run.code.insert(
            dst + 2 * k as u32,
            Seen {
                thumb: true,
                raw: h as u32,
                ..Seen::default()
            },
        );
    }
    if called {
        run.call_targets.insert(dst | 1);
    }
    (rom, run)
}

#[test]
fn ram_code_the_run_shows_copied_is_discovered_there() {
    let (rom, run) = copied_leaf(0x0300_0100, true);
    let d = Discovery::default();
    assert_eq!(
        classify(&rom, &d, &[], &run, None).ram_undiscovered_bytes,
        8
    );
    let copies = ram_copies(&rom, &d, &run);
    assert_eq!(copies.len(), 1);
    let c = &copies[0];
    assert!(c.seen);
    assert_eq!((c.overlay.src, c.overlay.dst), (B + 0x80, 0x0300_0100));
    // The image runs to the ROM's end: TAIL is longer than what's left.
    assert_eq!(c.overlay.len, 0x80);
    assert_eq!(c.entered, vec![0x0300_0101]);
    assert!(c.filled.is_empty());
    let overlays: Vec<Overlay> = copies.into_iter().map(|c| c.overlay).collect();
    let r = classify(&rom, &d, &overlays, &run, None);
    assert_eq!(r.ram_undiscovered_bytes, 0);
    assert!(r.ram_undiscovered.is_empty());
}

#[test]
fn a_bios_copy_entered_by_a_jump_gets_a_root_where_the_run_ran() {
    let (rom, run) = copied_leaf(0x0300_0200, false);
    let mut d = Discovery::default();
    d.copies.insert((B + 0x80, 0x0300_0200, 0x20));
    // No constant entry and no call: overlays() can't discover it.
    assert!(crate::discovery::overlays(&rom, &d).is_empty());
    let copies = ram_copies(&rom, &d, &run);
    assert_eq!(copies.len(), 1);
    let c = &copies[0];
    assert!(!c.seen);
    assert_eq!(c.overlay.len, 0x20);
    assert!(c.entered.is_empty());
    assert_eq!(c.filled, vec![0x0300_0201]);
    assert!(c.overlay.disc.code.contains_key(&0x0300_0206));
}

#[test]
fn ram_code_not_in_the_rom_or_changed_isnt_a_copy() {
    let (rom, mut run) = copied_leaf(0x0300_0100, true);
    // Built at run time: its bytes aren't the ROM's.
    for s in run.code.values_mut() {
        s.raw ^= 0x0100;
    }
    assert!(ram_copies(&rom, &Discovery::default(), &run).is_empty());
    let (rom, mut run) = copied_leaf(0x0300_0100, true);
    for s in run.code.values_mut() {
        s.changed = true;
    }
    assert!(ram_copies(&rom, &Discovery::default(), &run).is_empty());
}

#[test]
fn a_jump_in_a_rom_source_and_its_copy_counts_once() {
    let (rom, mut d) = case_helper();
    d.rec.jumps.insert(B + 0x84, JumpRes::Unresolved);
    d.rec.jumps.insert(B + 0x20, JumpRes::Unresolved);
    let mut od = Discovery::default();
    od.rec.jumps.insert(0x0300_0104, JumpRes::Unresolved);
    let o = Overlay {
        src: B + 0x80,
        dst: 0x0300_0100,
        len: 0x20,
        disc: od,
    };
    let r = classify(&rom, &d, &[o], &Run::default(), None);
    assert_eq!(
        r.unresolved.iter().map(|u| u.at).collect::<Vec<_>>(),
        vec![B + 0x20]
    );
    assert_eq!(r.overlay_unresolved.len(), 1);
    assert_eq!(r.hard_unresolved(), 2);
}

#[test]
fn a_copy_ends_where_another_source_agrees_further() {
    // ROM 0x80: a leaf, then the first three instructions of the leaf at
    // 0xc0. RAM 0x0300_0100 holds the first leaf, 0x0300_0110 the second:
    // at the first's offset, 0x0300_0110 is ROM 0x90, which agrees for
    // three instructions; ROM 0xc0 agrees for all six.
    let mut rom = vec![0u8; 0x100];
    let first = [0x2105u16, 0x2001, 0x3002, 0x4770];
    let second = [0x2201u16, 0x2302, 0x2403, 0x2504, 0x2605, 0x4770];
    let mut run = Run::default();
    for (k, &h) in first.iter().enumerate() {
        put16(&mut rom, B + 0x80 + 2 * k as u32, h);
    }
    for (k, &h) in second[..3].iter().enumerate() {
        put16(&mut rom, B + 0x90 + 2 * k as u32, h);
    }
    for (k, &h) in second.iter().enumerate() {
        put16(&mut rom, B + 0xc0 + 2 * k as u32, h);
    }
    for (dst, code) in [(0x0300_0100, &first[..]), (0x0300_0110, &second[..])] {
        for (k, &h) in code.iter().enumerate() {
            run.code.insert(
                dst + 2 * k as u32,
                Seen {
                    thumb: true,
                    raw: h as u32,
                    ..Seen::default()
                },
            );
        }
        run.call_targets.insert(dst | 1);
    }
    let copies = ram_copies(&rom, &Discovery::default(), &run);
    let got: Vec<(u32, u32, u32)> = copies
        .iter()
        .map(|c| (c.overlay.dst, c.overlay.len, c.overlay.src))
        .collect();
    assert_eq!(
        got,
        vec![(0x0300_0100, 0x10, B + 0x80), (0x0300_0110, 0x40, B + 0xc0)]
    );
    let overlays: Vec<Overlay> = copies.into_iter().map(|c| c.overlay).collect();
    let r = classify(&rom, &Discovery::default(), &overlays, &run, None);
    assert_eq!(r.ram_undiscovered_bytes, 0);
}

#[test]
fn ram_code_run_at_a_mirror_isnt_a_copy() {
    for dst in [0x0300_8100, 0x0204_0100] {
        let (rom, run) = copied_leaf(dst, true);
        assert!(ram_copies(&rom, &Discovery::default(), &run).is_empty());
    }
}
