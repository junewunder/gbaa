//! Hand-assembled ROMs, one per discovery rule.

use super::*;

const B: u32 = ROM_BASE;

/// A ROM being assembled. Addresses are offsets from the start of ROM.
struct Asm(Vec<u8>);

impl Asm {
    fn new() -> Asm {
        Asm(vec![0xff; 0x800])
    }

    fn h(&mut self, at: u32, v: u16) -> &mut Self {
        self.0[at as usize..at as usize + 2].copy_from_slice(&v.to_le_bytes());
        self
    }

    fn w(&mut self, at: u32, v: u32) -> &mut Self {
        self.0[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
        self
    }

    /// Thumb `bl`.
    fn bl(&mut self, at: u32, to: u32) -> &mut Self {
        let off = to as i32 - (at as i32 + 4);
        self.h(at, 0xf000 | ((off >> 12) as u16 & 0x7ff));
        self.h(at + 2, 0xf800 | ((off >> 1) as u16 & 0x7ff))
    }

    /// Thumb `b<cond>`: 1 is `ne`.
    fn bcond(&mut self, at: u32, cond: u16, to: u32) -> &mut Self {
        let off = (to as i32 - (at as i32 + 4)) / 2;
        self.h(at, 0xd000 | cond << 8 | (off as u16 & 0xff))
    }

    /// The ROM entry: ARM code that calls the Thumb function at `main`
    /// through `mov lr, pc; bx r0`, then loops.
    fn entry(&mut self, main: u32) -> &mut Self {
        self.w(0x0, 0xe59f_0008) // ldr r0, [pc, #8]
            .w(0x4, 0xe1a0_e00f) // mov lr, pc
            .w(0x8, 0xe12f_ff10) // bx r0
            .w(0xc, 0xeaff_fffe) // b .
            .w(0x10, B + main + 1)
    }

    fn run(&self) -> Discovery {
        discover(self.0.clone())
    }
}

fn thumb(off: u32) -> u32 {
    B + off + 1
}

#[test]
fn descent_follows_calls_from_the_entry() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: push {lr}; bl leaf; pop {pc}
        .h(0x100, 0xb500)
        .bl(0x102, 0x200)
        .h(0x106, 0xbd00)
        // leaf: bx lr
        .h(0x200, 0x4770);
    let d = a.run();
    let want = [
        (B, Evidence::Confirmed),
        (thumb(0x100), Evidence::Confirmed),
        (thumb(0x200), Evidence::Confirmed),
    ];
    assert_eq!(d.funcs.into_iter().collect::<Vec<_>>(), want);
    // The entry's literal pool is data, and isn't decoded as code.
    assert!(d.data.contains(&(B + 0x10, B + 0x14)));
    assert!(!d.code.contains_key(&(B + 0x10)));
    assert_eq!(d.code[&(B + 0x102)].size, 4);
}

#[test]
fn pointer_scan_takes_validated_candidates() {
    let mut a = Asm::new();
    a.entry(0x100)
        .h(0x100, 0x4770) // main: bx lr
        // A leaf nothing calls: movs r0, #1; bx lr.
        .h(0x300, 0x2001)
        .h(0x302, 0x4770)
        // Text that would pop the caller's frame: pop {r0-r7, pc}.
        .h(0x340, 0xbdff)
        // A table of function pointers next to one already found.
        .w(0x400, thumb(0x100))
        .w(0x404, thumb(0x300))
        .w(0x408, thumb(0x340));
    let d = a.run();
    assert_eq!(d.funcs.get(&thumb(0x300)), Some(&Evidence::Scanned));
    assert!(!d.funcs.contains_key(&thumb(0x340)));
    assert_eq!(d.code[&(B + 0x302)].evidence, Evidence::Scanned);
    assert_eq!(d.scan.accepted, 1);
}

#[test]
fn pointer_scan_takes_a_lone_pool_getter() {
    let mut a = Asm::new();
    a.entry(0x100)
        .h(0x100, 0x4770)
        // ldr r0, [pc, #0]; bx lr; .word 0x06010000: one word in data
        // points at it, with nothing found beside it.
        .h(0x300, 0x4800)
        .h(0x302, 0x4770)
        .w(0x304, 0x0601_0000)
        // movs r0, #0; bx lr: a function's tail as often as a getter.
        .h(0x340, 0x2000)
        .h(0x342, 0x4770)
        .w(0x400, 0)
        .w(0x404, thumb(0x300))
        .w(0x408, 0)
        .w(0x40c, thumb(0x340))
        .w(0x410, 0);
    let d = a.run();
    assert_eq!(d.funcs.get(&thumb(0x300)), Some(&Evidence::Scanned));
    assert!(d.data.contains(&(B + 0x304, B + 0x308)));
    assert!(!d.funcs.contains_key(&thumb(0x340)));
}

#[test]
fn pointer_scan_rejects_an_unbalanced_body() {
    let mut a = Asm::new();
    a.entry(0x100)
        .h(0x100, 0x4770)
        // push {lr}; add sp, #8; pop {pc}: returns with sp 8 above entry.
        .h(0x300, 0xb500)
        .h(0x302, 0xb002)
        .h(0x304, 0xbd00)
        .w(0x400, thumb(0x100))
        .w(0x404, thumb(0x300));
    assert!(!a.run().funcs.contains_key(&thumb(0x300)));
}

#[test]
fn far_jump_bl_is_not_a_call() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: push {r4, lr}; cmp r0, #0; bne 1f; bl tail; 1: movs r0, #1;
        // tail: pop {r4, pc}
        .h(0x100, 0xb510)
        .h(0x102, 0x2800)
        .bcond(0x104, 1, 0x10a)
        .bl(0x106, 0x10c)
        .h(0x10a, 0x2001)
        .h(0x10c, 0xbd10);
    let d = a.run();
    assert!(d.rec.far_jumps.contains(&(B + 0x106)));
    assert!(!d.funcs.contains_key(&thumb(0x10c)));
    assert!(d.code.contains_key(&(B + 0x10c)));
}

#[test]
fn bl_to_a_bx_stub_calls_the_register() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: push {lr}; ldr r3, =callee; bl stub; pop {pc}
        .h(0x100, 0xb500)
        .h(0x102, 0x4b02) // ldr r3, [pc, #8] -> 0x10c
        .bl(0x104, 0x180)
        .h(0x108, 0xbd00)
        .h(0x10a, 0x46c0)
        .w(0x10c, thumb(0x200))
        .h(0x180, 0x4718) // stub: bx r3
        .h(0x200, 0x4770); // callee: bx lr
    let d = a.run();
    assert!(d.funcs.contains_key(&thumb(0x200)), "{:x?}", d.funcs);
    assert!(!d.funcs.contains_key(&thumb(0x180)));
}

#[test]
fn starts_file_is_the_accuracy_check_format() {
    let mut a = Asm::new();
    a.entry(0x100).h(0x100, 0x4770);
    let f = a.run().starts_file();
    let lines: Vec<&str> = f.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(
        lines,
        ["0x08000000  # confirmed", "0x08000101  # confirmed"]
    );
}

#[test]
fn one_extent_per_address() {
    let mut d = Driver::new(
        Program::new(vec![0; 0x100], Vec::new(), Vec::new()),
        vec![ROM_BASE],
    );
    for k in [(B + 0x40, false), (B + 0x40, true), (B + 0x80, true)] {
        d.roots.insert(k, Kind::Func);
    }
    // A capped root bounds nothing.
    d.roots.insert((B + 0x60, true), Kind::Func);
    d.capped.insert((B + 0x60, true));
    let f = d.extents();
    assert_eq!(
        f.iter().map(|f| (f.start, f.end)).collect::<Vec<_>>(),
        [(B + 0x40, B + 0x80), (B + 0x80, B + 0x100)]
    );
}

#[test]
fn mode_switch_inside_a_function_is_an_entry() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: adr r1, 1f; bx r1; .align; 1: (ARM) bx lr
        .h(0x100, 0xa100) // add r1, pc, #0 -> 0x104
        .h(0x102, 0x4708) // bx r1 (ARM, bit 0 clear)
        .w(0x104, 0xe12f_ff1e); // bx lr
    let d = a.run();
    assert_eq!(d.entries.get(&(B + 0x104)), Some(&Evidence::Confirmed));
    assert!(!d.funcs.contains_key(&(B + 0x104)));
    assert!(d.code.contains_key(&(B + 0x104)));
}

#[test]
fn scanned_evidence_leaves_confirmed_code_alone() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: push {lr}; cmp r0, #1; bhi 1f; lsls r0, #2; ldr r1, =table;
        // adds r0, r1; ldr r0, [r0]; mov pc, r0; 1: pop {pc}
        .h(0x100, 0xb500)
        .h(0x102, 0x2801)
        .h(0x104, 0xd806) // bhi 0x114
        .h(0x106, 0x0080)
        .h(0x108, 0x4901) // ldr r1, [pc, #4] -> 0x110
        .h(0x10a, 0x1840)
        .h(0x10c, 0x6800)
        .h(0x10e, 0x4687)
        .w(0x110, B + 0x118)
        .h(0x114, 0xbd00)
        .w(0x118, thumb(0x140))
        .w(0x11c, thumb(0x150))
        // Dead code inside main that passes validation: movs r0, #1; bx lr.
        .h(0x130, 0x2001)
        .h(0x132, 0x4770)
        // The two cases: pop {pc}.
        .h(0x140, 0xbd00)
        .h(0x150, 0xbd00)
        // Before main, a leaf whose load reads main's first case as a
        // pool: ldr r0, [pc, #0xbc] -> 0x140; bx lr.
        .h(0x80, 0x482f)
        .h(0x82, 0x4770)
        // A table pointing at both, next to main.
        .w(0x400, thumb(0x130))
        .w(0x404, thumb(0x100))
        .w(0x408, thumb(0x80));
    let d = a.run();
    // The scan takes both: a false start inside main, and a bogus pool
    // over main's first case.
    assert_eq!(d.funcs.get(&thumb(0x130)), Some(&Evidence::Scanned));
    assert_eq!(d.funcs.get(&thumb(0x80)), Some(&Evidence::Scanned));
    // main's extent still holds its cases, and descent still reaches them.
    assert_eq!(d.tables(), 1, "{:x?}", d.rec.jumps);
    for case in [0x140, 0x150] {
        assert_eq!(d.code[&(B + case)].evidence, Evidence::Confirmed);
    }
    assert!(!d.funcs.contains_key(&thumb(0x140)));
}

#[test]
fn a_start_its_function_runs_into_is_not_a_start() {
    let mut a = Asm::new();
    a.entry(0x100)
        .h(0x100, 0x4770) // main: bx lr
        // A function whose push comes after two instructions (modern
        // GCC): movs r1, #0; movs r2, #0; push {lr}; movs r0, #1; pop {pc}.
        .h(0x300, 0x2100)
        .h(0x302, 0x2200)
        .h(0x304, 0xb500)
        .h(0x306, 0x2001)
        .h(0x308, 0xbd00)
        // A leaf that loads the function's address: ldr r0, [pc, #4]; bx lr.
        .h(0x500, 0x4801)
        .h(0x502, 0x4770)
        .w(0x508, thumb(0x300))
        // A table holding main and the leaf, and a stray word pointing at
        // the push.
        .w(0x400, thumb(0x100))
        .w(0x404, thumb(0x500))
        .w(0x600, thumb(0x304));
    let d = a.run();
    // The stray start is taken first; the true one, loaded by the leaf
    // the same batch found, runs into it, and it stops being a start.
    assert_eq!(d.funcs.get(&thumb(0x300)), Some(&Evidence::Scanned));
    assert!(!d.funcs.contains_key(&thumb(0x304)), "{:x?}", d.funcs);
    assert!(d.entries.contains_key(&thumb(0x304)));
    assert_eq!(d.scan.batches, 3);
}

#[test]
fn scanned_start_leaves_a_confirmed_entry_an_entry() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: adr r1, 1f; bx r1; ...; 1: (ARM) bx lr
        .h(0x100, 0xa107) // add r1, pc, #28 -> 0x120
        .h(0x102, 0x4708)
        // Dead code between the bx and its target: movs r0, #1; bx lr.
        .h(0x110, 0x2001)
        .h(0x112, 0x4770)
        .w(0x120, 0xe12f_ff1e)
        .w(0x400, thumb(0x110))
        .w(0x404, thumb(0x100));
    let d = a.run();
    assert_eq!(d.funcs.get(&thumb(0x110)), Some(&Evidence::Scanned));
    assert_eq!(d.entries.get(&(B + 0x120)), Some(&Evidence::Confirmed));
    assert!(!d.funcs.contains_key(&(B + 0x120)), "{:x?}", d.funcs);
}

#[test]
fn new_data_over_a_jump_target_redoes_the_jump() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: push {lr}; bl leaf; ldr r1, =arm; bx r1
        .h(0x100, 0xb500)
        .bl(0x102, 0x200)
        .h(0x106, 0x4901) // ldr r1, [pc, #4] -> 0x10c
        .h(0x108, 0x4708)
        .h(0x10a, 0x46c0)
        .w(0x10c, B + 0x300)
        // leaf: ldr r0, [pc, #0xfc] -> 0x300; bx lr. Its pool is the jump's
        // target, found only after main's first analysis.
        .h(0x200, 0x483f)
        .h(0x202, 0x4770)
        .w(0x300, 0xe12f_ff1e);
    let d = a.run();
    assert!(
        d.data
            .iter()
            .any(|&(lo, hi)| (lo..hi).contains(&(B + 0x300)))
    );
    // main read the target only to check it decodes: the check is redone.
    assert!(
        matches!(d.rec.jumps[&(B + 0x108)], JumpRes::Rejected(_)),
        "{:x?}",
        d.rec.jumps
    );
}

#[test]
fn nothing_follows_a_call_that_never_returns() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: push {lr}; bl die; then what would pop the caller's frame.
        .h(0x100, 0xb500)
        .bl(0x102, 0x200)
        .h(0x106, 0xbdff)
        .h(0x200, 0xe7fe); // die: b .
    let d = a.run();
    assert_eq!(d.funcs.get(&thumb(0x200)), Some(&Evidence::Confirmed));
    assert!(!d.code.contains_key(&(B + 0x106)));
}

#[test]
fn bl_into_the_callers_own_span_is_a_jump() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: push {r4, lr}; cmp r0, #0; bne 1f; bl cold; nop;
        // cold: bl die; udf; nop; 1: pop {r4, pc}
        .h(0x100, 0xb510)
        .h(0x102, 0x2800)
        .h(0x104, 0xd106) // bne 0x114
        .bl(0x106, 0x10c)
        .h(0x10a, 0x46c0)
        .bl(0x10c, 0x200)
        .h(0x110, 0xde00)
        .h(0x112, 0x46c0)
        .h(0x114, 0xbd10)
        .h(0x200, 0xe7fe); // die: b .
    let d = a.run();
    // LLVM's long branch to a cold block: the block never returns, so only
    // its place inside main says it is main's.
    assert!(d.rec.far_jumps.contains(&(B + 0x106)));
    assert!(!d.funcs.contains_key(&thumb(0x10c)), "{:x?}", d.funcs);
    assert_eq!(d.code[&(B + 0x10c)].evidence, Evidence::Confirmed);
    assert!(d.funcs.contains_key(&thumb(0x200)));
}

#[test]
fn crt0_copies_and_jumps_over_its_pool() {
    let mut a = Asm::new();
    // agb's crt0: CpuSet a section out of ROM, then ldr r3, =main; bx r3.
    a.w(0x00, 0xe59f_0010) // ldr r0, [pc, #16] -> 0x18
        .w(0x04, 0xe59f_1010) // ldr r1, [pc, #16] -> 0x1c
        .w(0x08, 0xe59f_2010) // ldr r2, [pc, #16] -> 0x20
        .w(0x0c, 0xef0b_0000) // swi 0x0b (CpuSet)
        .w(0x10, 0xe59f_300c) // ldr r3, [pc, #12] -> 0x24
        .w(0x14, 0xe12f_ff13) // bx r3
        .w(0x18, B + 0x300)
        .w(0x1c, 0x0300_0000)
        .w(0x20, 0x0400_0004) // 4 words
        .w(0x24, thumb(0x100))
        .h(0x100, 0x4770);
    let d = a.run();
    assert_eq!(d.funcs.get(&thumb(0x100)), Some(&Evidence::Confirmed));
    assert!(d.entries.is_empty(), "{:x?}", d.entries);
    assert_eq!(
        d.copies.iter().copied().collect::<Vec<_>>(),
        [(B + 0x300, 0x0300_0000, 16)]
    );
    assert!(
        d.data
            .iter()
            .any(|&(lo, hi)| lo <= B + 0x300 && B + 0x310 <= hi)
    );
}

#[test]
fn extra_starts_are_confirmed_and_cut_a_copy_source() {
    let mut a = Asm::new();
    // crt0_copies_and_jumps_over_its_pool's crt0, copying 4 words from
    // 0x300; a run saw calls to 0x308 (inside the copy's source: m4a's
    // `SoundMainRAM` copy reads past its end) and to 0x400, which nothing
    // in ROM points at.
    a.w(0x00, 0xe59f_0010)
        .w(0x04, 0xe59f_1010)
        .w(0x08, 0xe59f_2010)
        .w(0x0c, 0xef0b_0000)
        .w(0x10, 0xe59f_300c)
        .w(0x14, 0xe12f_ff13)
        .w(0x18, B + 0x300)
        .w(0x1c, 0x0300_0000)
        .w(0x20, 0x0400_0004)
        .w(0x24, thumb(0x100))
        .h(0x100, 0x4770)
        .h(0x308, 0x2001) // movs r0, #1
        .h(0x30a, 0x4770) // bx lr
        .h(0x400, 0x2002) // movs r0, #2
        .h(0x402, 0x4770); // bx lr
    let plain = a.run();
    assert!(!plain.funcs.contains_key(&thumb(0x308)));
    assert!(!plain.funcs.contains_key(&thumb(0x400)));
    let d = discover_with(a.0.clone(), &[thumb(0x308), thumb(0x400)]);
    for f in [thumb(0x100), thumb(0x308), thumb(0x400)] {
        assert_eq!(d.funcs.get(&f), Some(&Evidence::Confirmed), "{f:x}");
    }
    assert!(d.code.contains_key(&(B + 0x30a)));
    assert!(d.code.contains_key(&(B + 0x402)));
    assert!(d.data.contains(&(B + 0x300, B + 0x308)), "{:x?}", d.data);
    // The copy itself is unchanged.
    assert_eq!(
        d.copies.iter().copied().collect::<Vec<_>>(),
        [(B + 0x300, 0x0300_0000, 16)]
    );
}

#[test]
fn a_leaf_inside_the_callers_span_is_still_called() {
    let mut a = Asm::new();
    a.entry(0x100)
        // main: push {r4, lr}; cmp r0, #0; bne 1f; bl leaf; pop {r4, pc};
        // leaf: movs r0, #1; bx lr; 1: movs r0, #2; pop {r4, pc}
        .h(0x100, 0xb510)
        .h(0x102, 0x2800)
        .h(0x104, 0xd104) // bne 0x110
        .bl(0x106, 0x10c)
        .h(0x10a, 0xbd10)
        .h(0x10c, 0x2001)
        .h(0x10e, 0x4770)
        .h(0x110, 0x2002)
        .h(0x112, 0xbd10);
    let d = a.run();
    assert!(!d.rec.far_jumps.contains(&(B + 0x106)));
    assert_eq!(d.funcs.get(&thumb(0x10c)), Some(&Evidence::Confirmed));
    assert!(d.code.contains_key(&(B + 0x10a)));
}

#[test]
fn code_copied_to_iwram_is_discovered_where_it_runs() {
    let mut a = Asm::new();
    // crt0: CpuSet 8 words from 0x300 to IWRAM, install the IRQ handler
    // at its start, then ldr r3, =main; bx r3.
    a.w(0x00, 0xe59f_0020) // ldr r0, [pc, #32] -> 0x28
        .w(0x04, 0xe59f_1020) // ldr r1, [pc, #32] -> 0x2c
        .w(0x08, 0xe59f_2020) // ldr r2, [pc, #32] -> 0x30
        .w(0x0c, 0xef0b_0000) // swi 0x0b (CpuSet)
        .w(0x10, 0xe59f_101c) // ldr r1, [pc, #28] -> 0x34
        .w(0x14, 0xe59f_001c) // ldr r0, [pc, #28] -> 0x38
        .w(0x18, 0xe581_0000) // str r0, [r1]
        .w(0x1c, 0xe59f_3018) // ldr r3, [pc, #24] -> 0x3c
        .w(0x20, 0xe12f_ff13) // bx r3
        .w(0x28, B + 0x300)
        .w(0x2c, 0x0300_0000)
        .w(0x30, 0x0400_0008)
        .w(0x34, 0x0300_7ffc)
        .w(0x38, 0x0300_0000)
        .w(0x3c, thumb(0x100))
        // main: push {lr}; ldr r3, =0x03000010; bl stub; pop {pc}
        .h(0x100, 0xb500)
        .h(0x102, 0x4b02) // ldr r3, [pc, #8] -> 0x10c
        .bl(0x104, 0x180)
        .h(0x108, 0xbd00)
        .h(0x10a, 0x46c0)
        .w(0x10c, 0x0300_0010)
        .h(0x180, 0x4718) // stub: bx r3
        // The copy: the IRQ handler (bx lr), a data word, and an ARM
        // function at +0x10: b 1f; 1: bx lr.
        .w(0x300, 0xe12f_ff1e)
        .w(0x304, 0xdead_beef)
        .w(0x310, 0xea00_0000)
        .w(0x318, 0xe12f_ff1e);
    let d = a.run();
    assert_eq!(
        d.ram_entries.iter().copied().collect::<Vec<_>>(),
        [0x0300_0000, 0x0300_0010]
    );
    // ROM discovery is as it was: the stub isn't a function.
    assert!(!d.funcs.contains_key(&thumb(0x180)));
    let ov = overlays(&a.0, &d);
    assert_eq!(ov.len(), 1);
    let o = &ov[0];
    assert_eq!((o.src, o.dst, o.len), (B + 0x300, 0x0300_0000, 32));
    assert_eq!(o.rom_addr(0x0300_0018), B + 0x318);
    let funcs: Vec<u32> = o.disc.funcs.keys().copied().collect();
    assert_eq!(funcs, [0x0300_0000, 0x0300_0010]);
    let code: Vec<u32> = o.disc.code.keys().copied().collect();
    assert_eq!(code, [0x0300_0000, 0x0300_0010, 0x0300_0018]);
}
