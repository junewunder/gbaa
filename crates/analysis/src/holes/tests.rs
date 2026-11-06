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
    assert!(bios_implemented(0x0b) && bios_implemented(0x2a));
    assert!(!bios_implemented(0x26) && !bios_implemented(0x2b));
    assert_eq!(
        summary_kind("callee-saves: 0x08001234: returns with r4-r11 or sp changed"),
        "returns with r4-r11 or sp changed"
    );
    assert!(summary_hard(&summary_kind(
        "0x08001234: returns with r4-r11 or sp changed"
    )));
    assert!(!summary_hard("an indirect call"));
    let mut sums = BTreeMap::new();
    sums.insert(
        B,
        vec!["0x08000010: returns with r4-r11 or sp changed".to_string()],
    );
    sums.insert(B + 0x40, vec!["0x08000050: an indirect call".to_string()]);
    let r = classify(
        &[0u8; 4],
        &Discovery::default(),
        &[],
        &Run::default(),
        Some(&sums),
    );
    assert_eq!(r.hard_summary(), 1);
    assert_eq!(r.summary.len(), 2);
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
