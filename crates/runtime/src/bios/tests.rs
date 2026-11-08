extern crate std;

use super::*;
use crate::bus::BIOS_LATCH_AFTER_IRQ;
use crate::io::event;
use crate::platform::Headless;
use armv4t::interp::{I_BIT, T_BIT};
use std::boxed::Box;
use std::format;

const EWRAM: u32 = 0x0200_0000;
const VRAM: u32 = 0x0600_0000;
const IO: u32 = 0x0400_0000;
/// Where the test `swi`s sit, and the address after them.
const AT: u32 = 0x0800_0100;
const NEXT: u32 = AT + 4;

fn machine() -> Box<Machine<Headless>> {
    let rom: &'static [u8] = Box::leak(std::vec![0u8; 0x200].into_boxed_slice());
    let mut m = Box::new(Machine::new(rom, Headless));
    m.cpu.pc = NEXT;
    m
}

/// Run call `n` from an ARM `swi` at [`AT`], and any stall it leaves.
fn call(m: &mut Machine<Headless>, n: u8) {
    swi(m, (n as u32) << 16).unwrap();
    while m.cpu.pc == STALL_LOOP {
        m.step();
    }
}

fn regs(m: &mut Machine<Headless>, r: &[u32]) {
    m.cpu.r[..r.len()].copy_from_slice(r);
}

fn put(m: &mut Machine<Headless>, addr: u32, bytes: &[u8]) {
    for (i, &b) in bytes.iter().enumerate() {
        m.bus.write8(addr + i as u32, b);
    }
}

fn bytes(m: &Machine<Headless>, addr: u32, n: u32) -> std::vec::Vec<u8> {
    (addr..addr + n).map(|a| m.bus.read8(a)).collect()
}

#[test]
fn number_comes_from_the_comment_field() {
    assert_eq!(number(0x05_0000, false), 5);
    assert_eq!(number(0xAB_1234, false), 0xAB);
    assert_eq!(number(0x05, true), 5);
}

#[test]
fn div() {
    let mut m = machine();
    // r0 = quotient, r1 = remainder, r3 = |quotient|, rounding toward 0.
    for (num, den, q, r) in [
        (7, 2, 3, 1),
        (-7, 2, -3, -1),
        (7, -2, -3, 1),
        (-7, -2, 3, -1),
    ] {
        regs(&mut m, &[num as u32, den as u32]);
        call(&mut m, call::DIV);
        assert_eq!(m.cpu.r[0] as i32, q, "{num}/{den}");
        assert_eq!(m.cpu.r[1] as i32, r, "{num}%{den}");
        assert_eq!(m.cpu.r[3], q.unsigned_abs());
    }
    // DivArm takes them the other way round.
    regs(&mut m, &[2, 7]);
    call(&mut m, call::DIV_ARM);
    assert_eq!(m.cpu.r[..2], [3, 1]);
    // The edge cases, as mGBA returns them.
    regs(&mut m, &[(-5i32) as u32, 0]);
    call(&mut m, call::DIV);
    assert_eq!(
        [m.cpu.r[0], m.cpu.r[1], m.cpu.r[3]],
        [u32::MAX, (-5i32) as u32, 1]
    );
    regs(&mut m, &[i32::MIN as u32, u32::MAX]);
    call(&mut m, call::DIV);
    let min = i32::MIN as u32;
    assert_eq!([m.cpu.r[0], m.cpu.r[1], m.cpu.r[3]], [min, 0, min]);
}

#[test]
fn sqrt() {
    let mut m = machine();
    let cases = [
        (0, 0),
        (1, 1),
        (3, 1),
        (4, 2),
        (15, 3),
        (16, 4),
        (1 << 30, 1 << 15),
        (u32::MAX, 0xFFFF),
    ];
    for (x, root) in cases {
        regs(&mut m, &[x, 0x11, 0x22, 0x33]);
        call(&mut m, call::SQRT);
        assert_eq!(m.cpu.r[..4], [root, 0x11, 0x22, 0x33], "sqrt {x}");
    }
    for i in (0u32..=0xFFFF).step_by(97) {
        assert_eq!(math::sqrt(i * i), i);
        if i > 0 {
            assert_eq!(math::sqrt(i * i - 1), i - 1);
        }
    }
}

/// Expected values from an independent port of mGBA's C `_ArcTan` and
/// `_ArcTan2` (the same polynomial as the real BIOS), as (r0, r1, r3).
#[test]
fn arc_tan() {
    let mut m = machine();
    let cases: [(i32, u32, u32, u32); 5] = [
        (0x4000, 0x2000, 0xFFFF_C000, 0x8000),
        (-0x4000, 0xFFFF_E000, 0xFFFF_C000, 0x8000),
        (0x097E, 0x05FF, 0xFFFF_FE98, 0xA1CB),
        (0x1234, 0x0B4A, 0xFFFF_FAD3, 0x9EC7),
        (-0x2000, 0xFFFF_ED1C, 0xFFFF_F000, 0x9720),
    ];
    for (t, r0, r1, r3) in cases {
        regs(&mut m, &[t as u32, 0, 0, 0]);
        call(&mut m, call::ARC_TAN);
        assert_eq!(
            [m.cpu.r[0], m.cpu.r[1], m.cpu.r[3]],
            [r0, r1, r3],
            "atan({t:#x})"
        );
    }
}

#[test]
fn arc_tan2() {
    let mut m = machine();
    // (x, y) to 0..0xFFFF for 0..2π. The axes are exact and leave r1 alone.
    for (x, y, want) in [(5, 0, 0), (-5, 0, 0x8000), (0, 5, 0x4000), (0, -5, 0xC000)] {
        regs(&mut m, &[x as u32, y as u32, 0, 0]);
        call(&mut m, call::ARC_TAN2);
        assert_eq!(m.cpu.r[..4], [want, y as u32, 0, 0x170], "({x}, {y})");
    }
    // One point in each octant, so each branch runs.
    let cases = [
        (1000, 300, 0x0BDF),
        (300, 1000, 0x3421),
        (-300, 1000, 0x4BE0),
        (-1000, 300, 0x7420),
        (-1000, -300, 0x8BDF),
        (-300, -1000, 0xB421),
        (300, -1000, 0xCBE0),
        (1000, -300, 0xF420),
    ];
    for (x, y, want) in cases {
        regs(&mut m, &[x as u32, y as u32, 0, 0]);
        call(&mut m, call::ARC_TAN2);
        assert_eq!(m.cpu.r[..4], [want, 0xFFFF_FA3E, 0, 0x170], "({x}, {y})");
    }
    regs(&mut m, &[7, 7]);
    call(&mut m, call::ARC_TAN2);
    assert_eq!(m.cpu.r[..2], [0x2000, 0xFFFF_C000]);
}

#[test]
fn cpu_set() {
    let mut m = machine();
    let src = EWRAM;
    for i in 0..16u32 {
        m.bus.write32(src + 4 * i, 0x1111_1111 * i);
    }
    // Halfword copy of 3: r0 and r1 unchanged, r3 = 0x170.
    regs(&mut m, &[src, EWRAM + 0x100, 3]);
    call(&mut m, call::CPU_SET);
    assert_eq!(bytes(&m, EWRAM + 0x100, 8), [0, 0, 0, 0, 0x11, 0x11, 0, 0]);
    assert_eq!(
        [m.cpu.r[0], m.cpu.r[1], m.cpu.r[3]],
        [src, EWRAM + 0x100, 0x170]
    );
    // Word copy of 4: both pointers move past the data.
    regs(&mut m, &[src + 4, EWRAM + 0x200, 1 << 26 | 4]);
    call(&mut m, call::CPU_SET);
    for i in 0..4 {
        assert_eq!(m.bus.read32(EWRAM + 0x200 + 4 * i), 0x1111_1111 * (i + 1));
    }
    assert_eq!(m.bus.read32(EWRAM + 0x210), 0);
    assert_eq!(m.cpu.r[..2], [src + 20, EWRAM + 0x210]);
    // Word fill and halfword fill.
    regs(&mut m, &[src + 8, EWRAM + 0x300, 1 << 26 | 1 << 24 | 3]);
    call(&mut m, call::CPU_SET);
    for i in 0..3 {
        assert_eq!(m.bus.read32(EWRAM + 0x300 + 4 * i), 0x2222_2222);
    }
    assert_eq!(m.cpu.r[..2], [src + 12, EWRAM + 0x30C]);
    regs(&mut m, &[src + 12, EWRAM + 0x400, 1 << 24 | 3]);
    call(&mut m, call::CPU_SET);
    assert_eq!(
        bytes(&m, EWRAM + 0x400, 8),
        [0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0, 0]
    );
    // A source in the BIOS does nothing.
    regs(&mut m, &[0x100, EWRAM + 0x500, 1 << 26 | 4, 7]);
    call(&mut m, call::CPU_SET);
    assert_eq!(m.bus.read32(EWRAM + 0x500), 0);
    assert_eq!(m.cpu.r[3], 7);
}

#[test]
fn calls_take_time_as_in_mgba() {
    let mut m = machine();
    let took = |m: &mut Machine<Headless>, n: u8, r: &[u32]| {
        regs(m, r);
        let t = m.now();
        call(m, n);
        m.now() - t
    };
    // Div: 45 in and out of the BIOS, then 4 + 13 a quotient bit + 7.
    assert_eq!(took(&mut m, call::DIV, &[1000, 3]), 45 + 4 + 13 * 8 + 7);
    assert_eq!(took(&mut m, call::DIV, &[3, 1000]), 45 + 4 + 13 + 7);
    // A word fill of VRAM: 25 around the loop, 4 + 2 (a 32-bit VRAM store)
    // a word.
    let fill = 1 << 24 | 1 << 26;
    assert_eq!(
        took(&mut m, call::CPU_SET, &[EWRAM, VRAM, fill | 0x100]),
        45 + 25 + 6 * 0x100
    );
    // A word copy from EWRAM: 5 (with the load's I cycle) + 2 + 6 (a 32-bit
    // EWRAM load).
    assert_eq!(
        took(&mut m, call::CPU_SET, &[EWRAM, VRAM, 1 << 26 | 0x10]),
        45 + 25 + 0x10 * (5 + 2 + 6)
    );
    // CpuFastSet moves 8 words a burst: 1 + 8 for an IWRAM store burst.
    assert_eq!(
        took(
            &mut m,
            call::CPU_FAST_SET,
            &[EWRAM, 0x0300_0000, 1 << 24 | 16]
        ),
        45 + 30 + 2 * (4 + 8)
    );
    // The waits take nothing here: halting is their time.
    assert_eq!(took(&mut m, call::HALT, &[]), 0);
}

#[test]
fn cpu_fast_set() {
    let mut m = machine();
    for i in 0..16u32 {
        m.bus.write32(EWRAM + 4 * i, i + 1);
    }
    // A copy of 10 words moves 16: the count rounds up to 8.
    regs(&mut m, &[EWRAM, EWRAM + 0x100, 10]);
    call(&mut m, call::CPU_FAST_SET);
    for i in 0..16 {
        assert_eq!(m.bus.read32(EWRAM + 0x100 + 4 * i), i + 1);
    }
    assert_eq!(m.bus.read32(EWRAM + 0x140), 0);
    // r2 as given: the SWI dispatcher restores it.
    assert_eq!(m.cpu.r[..4], [EWRAM + 0x40, EWRAM + 0x140, 10, 9]);
    // A fill of 3 writes 8.
    regs(&mut m, &[EWRAM + 8, EWRAM + 0x200, 1 << 24 | 3]);
    call(&mut m, call::CPU_FAST_SET);
    for i in 0..8 {
        assert_eq!(m.bus.read32(EWRAM + 0x200 + 4 * i), 3);
    }
    assert_eq!(m.bus.read32(EWRAM + 0x220), 0);
}

#[test]
fn bg_affine_set() {
    let mut m = machine();
    let (src, dst) = (EWRAM, EWRAM + 0x100);
    // The identity (as agb's own test checks), then a quarter turn about
    // texture (16, 8) shown at screen (8, 4).
    let entries: [(i32, i32, i16, i16, i16, i16, u16); 2] = [
        (0, 0, 0, 0, 0x100, 0x100, 0),
        (16 << 8, 8 << 8, 8, 4, 0x100, 0x100, 0x4000),
    ];
    for (k, &(ox, oy, cx, cy, sx, sy, angle)) in entries.iter().enumerate() {
        let a = src + 20 * k as u32;
        m.bus.write32(a, ox as u32);
        m.bus.write32(a + 4, oy as u32);
        for (j, v) in [cx as u16, cy as u16, sx as u16, sy as u16, angle]
            .into_iter()
            .enumerate()
        {
            m.bus.write16(a + 8 + 2 * j as u32, v);
        }
    }
    regs(&mut m, &[src, dst, 2]);
    call(&mut m, call::BG_AFFINE_SET);
    let half = |a: u32| m.bus.read16(a) as i16;
    assert_eq!(
        [half(dst), half(dst + 2), half(dst + 4), half(dst + 6)],
        [0x100, 0, 0, 0x100]
    );
    assert_eq!([m.bus.read32(dst + 8), m.bus.read32(dst + 12)], [0, 0]);
    // 90°: pa = cos = 0, pb = -sin, pc = sin, pd = 0. GBATEK's start point:
    // x = ox - (cx·pa + cy·pb), y = oy - (cx·pc + cy·pd).
    let d = dst + 16;
    assert_eq!(
        [half(d), half(d + 2), half(d + 4), half(d + 6)],
        [0, -0x100, 0x100, 0]
    );
    assert_eq!(m.bus.read32(d + 8) as i32, (16 << 8) + 4 * 0x100);
    assert_eq!(m.bus.read32(d + 12) as i32, (8 << 8) - 8 * 0x100);
    assert_eq!(m.cpu.r[..3], [src + 40, dst + 32, u32::MAX]);
}

#[test]
fn obj_affine_set() {
    let mut m = machine();
    // Scale x2 horizontally, 45°: cos = sin = 0x2D41 / 0x4000.
    for (j, v) in [0x200u16, 0x100, 0x2000].into_iter().enumerate() {
        m.bus.write16(EWRAM + 2 * j as u32, v);
    }
    // Into OAM's parameter slots: a stride of 8.
    let oam = 0x0700_0006;
    regs(&mut m, &[EWRAM, oam, 1, 8]);
    call(&mut m, call::OBJ_AFFINE_SET);
    let p: std::vec::Vec<i16> = (0..4).map(|k| m.bus.read16(oam + 8 * k) as i16).collect();
    // 0x200 * 0x2D41 >> 14 and 0x100 * 0x2D41 >> 14.
    let (c2, c1) = (0x16A, 0xB5);
    assert_eq!(p, [c2, -c2, c1, c1]);
    assert_eq!(
        m.bus.read16(0x0700_0008),
        0,
        "the stride skips the attributes"
    );
    assert_eq!(m.cpu.r[..2], [EWRAM + 8, oam + 32]);
    // Stride 2: a packed matrix.
    regs(&mut m, &[EWRAM, EWRAM + 0x100, 1, 2]);
    call(&mut m, call::OBJ_AFFINE_SET);
    let p: std::vec::Vec<i16> = (0..4)
        .map(|k| m.bus.read16(EWRAM + 0x100 + 2 * k) as i16)
        .collect();
    assert_eq!(p, [c2, -c2, c1, c1]);
}

/// "ABC", then 6 bytes from 3 back, then "X"; then "AAAAA" from one
/// literal and an overlapping back-reference.
const LZ77: [u8; 11] = [0x10, 10, 0, 0, 0x10, b'A', b'B', b'C', 0x30, 0x02, b'X'];
const LZ77_RUN: [u8; 8] = [0x10, 5, 0, 0, 0x40, b'A', 0x10, 0x00];

#[test]
fn lz77() {
    for (n, dst) in [
        (call::LZ77_UNCOMP_WRAM, EWRAM + 0x100),
        (call::LZ77_UNCOMP_VRAM, VRAM),
    ] {
        let mut m = machine();
        put(&mut m, EWRAM, &LZ77);
        regs(&mut m, &[EWRAM, dst, 0, 9]);
        call(&mut m, n);
        assert_eq!(bytes(&m, dst, 11), b"ABCABCABCX\0");
        assert_eq!(
            [m.cpu.r[0], m.cpu.r[1], m.cpu.r[3]],
            [EWRAM + 11, dst + 10, 0]
        );
    }
    let mut m = machine();
    put(&mut m, EWRAM, &LZ77_RUN);
    regs(&mut m, &[EWRAM, EWRAM + 0x100]);
    call(&mut m, call::LZ77_UNCOMP_WRAM);
    assert_eq!(bytes(&m, EWRAM + 0x100, 6), b"AAAAA\0");
}

/// mGBA's LZ77 sends its accesses below the cartridge through the prefetch
/// stall when called from ROM with the buffer on (Emerald's bike scene
/// loads a frame sooner for it), and not when called from RAM.
#[test]
fn lz77_from_rom_overlaps_prefetch() {
    let took = |pc: u32, waitcnt: u16| {
        let mut m = machine();
        m.bus.write16(IO + 0x204, waitcnt);
        m.cpu.pc = pc;
        put(&mut m, EWRAM, &LZ77);
        regs(&mut m, &[EWRAM, VRAM]);
        let t = m.now();
        call(&mut m, call::LZ77_UNCOMP_VRAM);
        assert_eq!(bytes(&m, VRAM, 10), b"ABCABCABCX");
        m.now() - t
    };
    let plain = took(NEXT, 0x0317);
    assert!(took(NEXT, 0x4317) < plain);
    assert_eq!(took(0x0300_0104, 0x4317), plain);
}

/// A long LZ77 runs its time as a stall at [`STALL_LOOP`], a piece at a
/// time up to each event, and an IRQ returns into it: Emerald Imperium's
/// intro decompresses into VRAM for two frames, and run at once it merged
/// their V-blank IRQs into one.
#[test]
fn a_long_lz77_stalls_where_irqs_return() {
    // "A", then 18-byte back-references to it: 1 + 7 * 18 + 8 * 8 * 18 bytes.
    let mut data = std::vec![0x10, 0, 0, 0, 0x7F, b'A'];
    let size: u32 = 1 + 7 * 18 + 8 * 8 * 18;
    data[1..4].copy_from_slice(&size.to_le_bytes()[..3]);
    data.extend([0xF0, 0x00].repeat(7));
    for _ in 0..8 {
        data.push(0xFF);
        data.extend([0xF0, 0x00].repeat(8));
    }
    let run = |interrupt: bool| {
        let mut m = machine();
        put(&mut m, EWRAM, &data);
        regs(&mut m, &[EWRAM, EWRAM + 0x1000]);
        let t = m.now();
        swi(&mut m, (call::LZ77_UNCOMP_WRAM as u32) << 16).unwrap();
        assert_eq!(m.cpu.pc, STALL_LOOP);
        m.step();
        assert_eq!(m.cpu.pc, STALL_LOOP, "the stall outlasts a line event");
        assert!(m.now() <= m.sched.next_event);
        if interrupt {
            irq(&mut m, 0);
            assert_eq!(m.cpu.pc, STALL_LOOP, "the IRQ returns to the stall");
        }
        while m.cpu.pc == STALL_LOOP {
            m.step();
        }
        assert_eq!(m.cpu.pc, NEXT);
        assert_eq!(
            bytes(&m, EWRAM + 0x1000, size),
            std::vec![b'A'; size as usize]
        );
        m.now() - t
    };
    assert!(run(false) > 2 * crate::sched::LINE_CYCLES);
    assert_eq!(run(true), run(false));
}

/// A long `CpuSet` runs its time as a stall too: Emerald clears VRAM with
/// one (0xC000 halfwords, most of a frame) opening the trade screen's
/// Summary, and run at once it starved the link's serial IRQs.
#[test]
fn a_long_cpu_set_stalls_where_irqs_return() {
    let run = |interrupt: bool| {
        let mut m = machine();
        put(&mut m, EWRAM, &[0x5A, 0xA5]);
        regs(&mut m, &[EWRAM, VRAM, 1 << 24 | 0x2000]);
        let t = m.now();
        swi(&mut m, (call::CPU_SET as u32) << 16).unwrap();
        assert_eq!(m.cpu.pc, STALL_LOOP);
        if interrupt {
            irq(&mut m, 0);
            assert_eq!(m.cpu.pc, STALL_LOOP, "the IRQ returns to the stall");
        }
        while m.cpu.pc == STALL_LOOP {
            m.step();
        }
        assert_eq!(m.cpu.pc, NEXT);
        assert_eq!(m.bus.read16(VRAM + 2 * 0x1FFF), 0xA55A);
        m.now() - t
    };
    assert!(run(false) > 2 * crate::sched::LINE_CYCLES);
    assert_eq!(run(true), run(false));
}

#[test]
fn rl() {
    // "ZZZZZ" (a run of 5), then "ab" copied: 7 bytes, padded to 8.
    let data = [0x30, 7, 0, 0, 0x82, b'Z', 0x01, b'a', b'b'];
    for (n, dst) in [
        (call::RL_UNCOMP_WRAM, EWRAM + 0x100),
        (call::RL_UNCOMP_VRAM, VRAM),
    ] {
        let mut m = machine();
        put(&mut m, EWRAM, &data);
        // Something to see the padding overwrite.
        m.bus.write32(dst + 4, 0xFFFF_FFFF);
        regs(&mut m, &[EWRAM, dst]);
        call(&mut m, n);
        // VRAM takes halfwords, so an odd size's last byte has no partner
        // and is never written (as in mGBA); nor is the padding then.
        let want = if n == call::RL_UNCOMP_VRAM {
            b"ZZZZZa\xFF\xFF"
        } else {
            b"ZZZZZab\0"
        };
        assert_eq!(bytes(&m, dst, 8), want, "swi {n:#x}");
        assert_eq!(m.cpu.r[..2], [EWRAM + 9, dst + 8]);
    }
}

/// A mod's override (docs/asset-extraction-design.md, "Overrides"): LZ77
/// and RL write its bytes instead of what they decode, keyed by the
/// blob's address, and leave registers and time as without it; another
/// blob decodes as before.
#[test]
fn decompressors_serve_asset_overrides() {
    const RL: [u8; 9] = [0x30, 7, 0, 0, 0x82, b'Z', 0x01, b'a', b'b'];
    let run = |n: u8, data: &[u8], dst: u32, over: &'static [(u32, &'static [u8])]| {
        let mut m = machine();
        m.bus.asset_overrides = over;
        put(&mut m, EWRAM, data);
        regs(&mut m, &[EWRAM, dst, 0, 9]);
        let t = m.now();
        call(&mut m, n);
        let len = data[1] as u32;
        (bytes(&m, dst, len), m.cpu.r, m.now() - t)
    };
    for (n, data, new) in [
        (call::LZ77_UNCOMP_WRAM, &LZ77[..], &b"0123456789"[..]),
        (call::LZ77_UNCOMP_VRAM, &LZ77[..], b"0123456789"),
        (call::RL_UNCOMP_WRAM, &RL[..], b"abcdefg"),
        (call::RL_UNCOMP_VRAM, &RL[..], b"abcdefg"),
    ] {
        let dst = if n == call::LZ77_UNCOMP_VRAM || n == call::RL_UNCOMP_VRAM {
            VRAM
        } else {
            EWRAM + 0x100
        };
        let over: &'static [(u32, &'static [u8])] = Box::leak(Box::new([(EWRAM, new)]));
        let elsewhere: &'static [(u32, &'static [u8])] = Box::leak(Box::new([(EWRAM + 4, new)]));
        let (plain, r, t) = run(n, data, dst, &[]);
        let (got, r2, t2) = run(n, data, dst, over);
        // VRAM's halfwords leave an odd size's last byte unwritten, with
        // the mod as without it.
        let mut want = new.to_vec();
        if dst == VRAM && new.len() % 2 == 1 {
            *want.last_mut().unwrap() = plain[new.len() - 1];
        }
        assert_eq!(got, want, "swi {n:#x}");
        assert_eq!((r2, t2), (r, t), "swi {n:#x}");
        assert_eq!(run(n, data, dst, elsewhere).0, plain, "swi {n:#x}");
    }
}

#[test]
fn diff8_unfilter() {
    // Deltas 1, 2, -1, 3, 0x10: bytes 1, 3, 2, 5, 0x15. The source is
    // taken word-aligned.
    let data = [0x81, 5, 0, 0, 1, 2, 0xFF, 3, 0x10, 7];
    let mut m = machine();
    put(&mut m, EWRAM, &data);
    m.bus.write32(EWRAM + 0x104, 0xFFFF_FFFF);
    regs(&mut m, &[EWRAM + 2, EWRAM + 0x100]);
    call(&mut m, call::DIFF8_UNFILTER_WRAM);
    assert_eq!(bytes(&m, EWRAM + 0x100, 6), [1, 3, 2, 5, 0x15, 0xFF]);
    assert_eq!(m.cpu.r[..2], [EWRAM + 9, EWRAM + 0x105]);
    // VRAM takes the bytes in pairs, so an odd size reads one delta past
    // the data and writes its sum too (mGBA): 0x15 + 7.
    let mut m = machine();
    put(&mut m, EWRAM, &data);
    regs(&mut m, &[EWRAM, VRAM]);
    call(&mut m, call::DIFF8_UNFILTER_VRAM);
    assert_eq!(bytes(&m, VRAM, 6), [1, 3, 2, 5, 0x15, 0x1C]);
    assert_eq!(m.cpu.r[..2], [EWRAM + 10, VRAM + 6]);
}

#[test]
fn diff16_unfilter() {
    // Deltas 0x100, -1, 0x8000: halfwords 0x100, 0xFF, 0x80FF.
    for dst in [EWRAM + 0x100, VRAM] {
        let mut m = machine();
        m.bus.write32(EWRAM, 0x82 | 6 << 8);
        for (i, d) in [0x100, 0xFFFF, 0x8000].into_iter().enumerate() {
            m.bus.write16(EWRAM + 4 + 2 * i as u32, d);
        }
        regs(&mut m, &[EWRAM, dst]);
        call(&mut m, call::DIFF16_UNFILTER);
        let out: std::vec::Vec<u16> = (0..3).map(|i| m.bus.read16(dst + 2 * i)).collect();
        assert_eq!(out, [0x100, 0xFF, 0x80FF], "to {dst:#x}");
        assert_eq!(m.cpu.r[..2], [EWRAM + 10, dst + 6]);
    }
}

#[test]
fn unfilter_ignores_a_bios_source() {
    for n in [
        call::DIFF8_UNFILTER_WRAM,
        call::DIFF8_UNFILTER_VRAM,
        call::DIFF16_UNFILTER,
    ] {
        let mut m = machine();
        regs(&mut m, &[0x100, EWRAM]);
        call(&mut m, n);
        assert_eq!(m.bus.read32(EWRAM), 0, "swi {n:#x}");
        assert_eq!(m.cpu.r[..2], [0x100, EWRAM]);
    }
}

#[test]
fn midi_key2freq() {
    // (freq, key, fine) and r0 after, worked through mGBA's formula in C
    // (gcc, glibc's exp2f, mGBA's int32_t registers). Key 180 is the
    // frequency itself, each 12 keys below halve it; 2^31 and up, x/0 and
    // 0/0 give 0x8000_0000, as mGBA on x86-64.
    for (freq, key, fine, out) in [
        (0x2B11_0000, 180, 0, 0x2B11_0000),
        (0x2B11_0000, 168, 0, 0x1588_8000),
        (0x2B11_0000, 192, 0, 0x5622_0000),
        (0x2B11_0000, 69, 0, 0x12_1B71),
        (0x2B11_0000, 60, 128, 0xB_1503),
        (3_546_000, 57, 77, 0xB92),
        (12_345_678, 100, 255, 0x1_F6CD),
        (0xE000_0000, 180, 255, 0x8000_0000),
        (0x7000_0000, 180, 255, 0x76A2_1280),
        (0x7FFF_FFFF, 240, 0, 0x8000_0000),
        (1000, 3000, 0, 0x8000_0000),
        (0, 3000, 0, 0x8000_0000),
        (0x2B11_0000, u32::MAX, 0, 0x514C),
    ] {
        let mut m = machine();
        m.bus.write32(EWRAM + 4, freq);
        regs(&mut m, &[EWRAM, key, fine, 7]);
        call(&mut m, call::MIDI_KEY2FREQ);
        assert_eq!(m.cpu.r[..4], [out, key, fine, 7], "{freq:#x} {key} {fine}");
    }
}

#[test]
fn exp2f_rounds_as_glibc() {
    // std's f32::exp2 is the host's exp2f: glibc's, on the machines that
    // run mGBA here. Every exponent a key of 0-255 and a fine adjust give.
    for key in 0..256u32 {
        for fine in 0..256u32 {
            let x = (180.0 - key as f32 - fine as f32 / 256.0) / 12.0;
            assert_eq!(math::exp2f(x).to_bits(), x.exp2().to_bits(), "2^{x}");
        }
    }
    for x in [
        0.0,
        -0.0,
        15.0,
        -149.5,
        -150.0,
        -1e9,
        200.0,
        f32::NEG_INFINITY,
    ] {
        assert_eq!(math::exp2f(x).to_bits(), x.exp2().to_bits(), "2^{x}");
    }
    assert!(math::exp2f(f32::NAN).is_nan());
}

#[test]
fn huffman() {
    // Codes 0, 10 and 11: the root's right child is a node whose children
    // are both leaves. Tree size byte 3: 8 bytes of tree, then the bits.
    let tree = [3, 0x80, 1, 0xC0, 2, 3, 0, 0];
    // 4-bit units 1,2,3,1,1,1,1,1 (0 10 11 0 0 0 0 0), then 8-bit ones
    // 1,2,3,1 (0 10 11 0): one output word each.
    for (width, want) in [(4, 0x1111_1321), (8, 0x0103_0201)] {
        let mut m = machine();
        m.bus.write32(EWRAM, 0x20 | width | 4 << 8);
        put(&mut m, EWRAM + 4, &tree);
        m.bus.write32(EWRAM + 12, 0x5800_0000);
        regs(&mut m, &[EWRAM, EWRAM + 0x100]);
        call(&mut m, call::HUFF_UNCOMP);
        assert_eq!(m.bus.read32(EWRAM + 0x100), want, "{width}-bit");
        assert_eq!(m.cpu.r[..2], [EWRAM + 16, EWRAM + 0x104]);
    }
}

#[test]
fn bit_unpack() {
    // 1 bit to 4, low bits first: 0b1011_0001 -> 1,0,0,0,1,1,0,1.
    for (bias, want) in [
        (0, 0x1011_0001),
        (2, 0x3033_0003),
        (0x8000_0002, 0x3233_2223),
    ] {
        let mut m = machine();
        m.bus.write8(EWRAM, 0b1011_0001);
        let info = EWRAM + 0x10;
        m.bus.write16(info, 1);
        m.bus.write8(info + 2, 1);
        m.bus.write8(info + 3, 4);
        m.bus.write32(info + 4, bias);
        regs(&mut m, &[EWRAM, EWRAM + 0x100, info]);
        call(&mut m, call::BIT_UNPACK);
        assert_eq!(m.bus.read32(EWRAM + 0x100), want, "bias {bias:#x}");
        assert_eq!(m.cpu.r[..2], [EWRAM + 1, EWRAM + 0x104]);
    }
}

/// Enter an IRQ the way the runner (#27) runs the BIOS's IRQ stub: IRQ
/// mode, then [`irq_enter`]. Returns what the stub pushes (r0-r3 and
/// `lr_irq`) and the `spsr_irq` a handler that nests saves.
fn irq_in(m: &mut Machine<Headless>) -> [u32; 6] {
    let ret = m.cpu.pc + 4;
    m.cpu.enter_exception(mode::IRQ, 0x18, ret);
    irq_enter(m);
    let r = &m.cpu.r;
    let saved = [r[0], r[1], r[2], r[3], r[14], m.cpu.spsr()];
    m.cpu.r[0] = IO;
    saved
}

/// The stub's return, in IRQ mode: pop, `subs pc, lr, #4`, then
/// [`irq_return`].
fn irq_out(m: &mut Machine<Headless>, saved: [u32; 6]) {
    assert_eq!(m.cpu.cpsr & 0x1F, mode::IRQ);
    m.cpu.r[..4].copy_from_slice(&saved[..4]);
    m.cpu.write_cpsr(saved[5], !0);
    m.cpu.pc = saved[4] - 4;
    irq_return(m);
}

/// A whole IRQ whose handler records `flags` in [`BIOS_IF`], as agb's does.
fn irq(m: &mut Machine<Headless>, flags: u16) {
    let saved = irq_in(m);
    let f = m.bus.read16(BIOS_IF);
    m.bus.write16(BIOS_IF, f | flags);
    irq_out(m, saved);
}

fn halted(m: &mut Machine<Headless>) -> bool {
    m.bus.io.take_events() & event::HALT != 0
}

#[test]
fn halt_and_stop_write_haltcnt() {
    let mut m = machine();
    call(&mut m, call::HALT);
    assert_eq!(m.bus.io.take_events(), event::HALT);
    assert_eq!(m.cpu.pc, NEXT);
    call(&mut m, call::STOP);
    assert_eq!(m.bus.io.take_events(), event::STOP);
    regs(&mut m, &[0, 0, 0x80]);
    call(&mut m, call::CUSTOM_HALT);
    assert_eq!(m.bus.io.take_events(), event::STOP);
}

#[test]
fn halt_finishes_after_the_irq_that_wakes_it() {
    let mut m = machine();
    call(&mut m, call::HALT);
    irq(&mut m, 0);
    assert_eq!(
        m.bus.read32(0),
        BIOS_LATCH_AFTER_SWI,
        "the BIOS returns last"
    );
    assert!(m.bios.pending().is_empty());
    // Woken with IME off (no IRQ taken), then an IRQ elsewhere.
    call(&mut m, call::HALT);
    m.cpu.pc = NEXT + 0x40;
    irq(&mut m, 0);
    assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_IRQ);
    assert!(m.bios.pending().is_empty());
}

#[test]
fn vblank_intr_wait_halts_until_the_handler_flags_vblank() {
    let mut m = machine();
    // An old V-blank flag is discarded.
    m.bus.write16(BIOS_IF, 1);
    call(&mut m, call::VBLANK_INTR_WAIT);
    assert_eq!(m.cpu.r[..2], [1, 1]);
    assert_eq!(m.bus.read16(BIOS_IF), 0);
    assert_eq!(m.bus.read16(IO + reg::IME), 1, "IME forced on");
    assert!(halted(&mut m));
    assert_eq!(m.cpu.pc, NEXT);

    // An H-blank IRQ: the handler flags H-blank, so the wait halts again.
    let saved = irq_in(&mut m);
    assert_eq!(m.bus.read32(0), BIOS_LATCH_IN_IRQ);
    m.bus.write16(BIOS_IF, 2);
    irq_out(&mut m, saved);
    assert!(halted(&mut m));
    assert_eq!(m.cpu.r[0], 1);
    assert!(!m.bios.pending().is_empty());

    // A nested IRQ inside the handler (run in system mode, as agb's is):
    // its return doesn't check.
    let outer = irq_in(&mut m);
    m.cpu.write_cpsr(mode::SYS, 0xFF);
    m.cpu.pc = 0x0300_1000;
    let inner = irq_in(&mut m);
    m.bus.write16(BIOS_IF, 3);
    irq_out(&mut m, inner);
    assert_eq!(m.cpu.pc, 0x0300_1000);
    assert!(!halted(&mut m));
    assert!(!m.bios.pending().is_empty());
    assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_IRQ);
    m.cpu.write_cpsr(mode::IRQ | I_BIT, 0xFF);
    irq_out(&mut m, outer);

    // Back at the wait with V-blank flagged: it ends, takes only its own
    // flag, returns it in r0, and doesn't halt.
    assert_eq!(m.cpu.pc, NEXT);
    assert_eq!(m.cpu.cpsr & 0x1F, mode::SYS);
    assert!(m.bios.pending().is_empty());
    assert_eq!(m.bus.read16(BIOS_IF), 2);
    assert!(!halted(&mut m));
    assert_eq!(m.cpu.r[..2], [1, 1]);
    assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_SWI);
}

#[test]
fn intr_wait_without_discard_returns_on_an_old_flag() {
    let mut m = machine();
    m.bus.write16(BIOS_IF, 0x0009);
    regs(&mut m, &[0, 0x000C]);
    call(&mut m, call::INTR_WAIT);
    assert!(m.bios.pending().is_empty());
    assert!(!halted(&mut m));
    assert_eq!(m.cpu.r[..2], [8, 0x000C]);
    assert_eq!(m.bus.read16(BIOS_IF), 1);
    assert_eq!(m.bus.read16(IO + reg::IME), 1);
    // No flag yet: it halts, until a timer 0 IRQ.
    regs(&mut m, &[0, 0x0008]);
    call(&mut m, call::INTR_WAIT);
    assert!(halted(&mut m));
    irq(&mut m, 0x0008);
    assert_eq!(m.cpu.r[0], 8);
    assert!(m.bios.pending().is_empty());
}

#[test]
fn a_wait_the_cpu_left_is_dropped() {
    let mut m = machine();
    call(&mut m, call::VBLANK_INTR_WAIT);
    assert!(halted(&mut m));
    // The halt ended without an IRQ (IRQs off in the CPSR) and the code
    // moved on. The next IRQ strikes elsewhere and must not finish the
    // wait there.
    m.cpu.pc = NEXT + 0x40;
    m.cpu.r[0] = 0x55;
    irq(&mut m, 1);
    assert!(m.bios.pending().is_empty());
    assert_eq!(m.cpu.r[0], 0x55);
    assert_eq!(
        m.bus.read16(BIOS_IF),
        1,
        "the flag is left for a later wait"
    );
    assert!(!halted(&mut m));
}

#[test]
fn a_wait_inside_a_handler_finishes_from_a_nested_irq() {
    let mut m = machine();
    let saved = irq_in(&mut m);
    m.cpu.write_cpsr(mode::SYS, 0xFF);
    m.cpu.pc = 0x0300_1004;
    call(&mut m, call::VBLANK_INTR_WAIT);
    assert!(halted(&mut m));
    irq(&mut m, 1);
    assert!(m.bios.pending().is_empty());
    assert_eq!((m.cpu.pc, m.cpu.cpsr & 0x1F), (0x0300_1004, mode::SYS));
    m.cpu.write_cpsr(mode::IRQ | I_BIT, 0xFF);
    irq_out(&mut m, saved);
    assert_eq!(m.cpu.pc, NEXT);
}

#[test]
fn calls_inside_the_handler_leave_the_wait_alone() {
    let mut m = machine();
    call(&mut m, call::VBLANK_INTR_WAIT);
    m.bus.io.take_events();
    let saved = irq_in(&mut m);
    m.cpu.pc = 0x0300_1004;
    regs(&mut m, &[9, 3]);
    call(&mut m, call::DIV);
    assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_SWI);
    call(&mut m, call::HALT);
    m.bus.write16(BIOS_IF, 1);
    irq_out(&mut m, saved);
    assert!(m.bios.pending().is_empty());
    assert_eq!(m.cpu.r[0], 1);
}

#[test]
fn bios_reads_after_a_swi() {
    let mut m = machine();
    assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_BOOT);
    regs(&mut m, &[16]);
    call(&mut m, call::SQRT);
    assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_SWI);
}

#[test]
fn unimplemented_calls_stop_with_their_number_and_address() {
    let mut m = machine();
    let before = (m.cpu.clone(), m.bus.bios_latch);
    // 0x2B is past the BIOS's last call.
    let e = swi(&mut m, 0x2B_0000).unwrap_err();
    assert_eq!(
        e,
        Unimplemented {
            number: 0x2B,
            pc: AT
        }
    );
    assert_eq!(format!("{e}"), "unimplemented swi 0x2b at 0x08000100");
    assert_eq!((m.cpu.clone(), m.bus.bios_latch), before, "nothing changed");
    m.cpu.cpsr |= T_BIT;
    m.cpu.pc = 0x0800_0102;
    assert_eq!(
        swi(&mut m, 0x2C).unwrap_err(),
        Unimplemented {
            number: 0x2C,
            pc: 0x0800_0100
        }
    );
}

#[test]
fn implemented_lists_what_swi_runs() {
    for n in 0..=255u8 {
        let mut m = machine();
        // Sources of zeros, lengths of zero: every call ends at once.
        regs(&mut m, &[EWRAM, 1, 0, 0]);
        let ran = swi(&mut m, (n as u32) << 16).is_ok();
        assert_eq!(ran, call::IMPLEMENTED.contains(&n), "swi {n:#04x}");
    }
}

#[test]
fn thumb_reads_the_low_byte() {
    let mut m = machine();
    m.cpu.cpsr |= T_BIT;
    m.cpu.pc = 0x0800_0102;
    regs(&mut m, &[7, 2]);
    swi(&mut m, call::DIV as u32).unwrap();
    assert_eq!(m.cpu.r[..2], [3, 1]);
    assert_eq!(m.cpu.pc, 0x0800_0102);
}

#[test]
fn soft_reset() {
    for (flag, entry) in [(0, ROM_ENTRY), (1, EWRAM)] {
        let mut m = machine();
        m.bus.write32(0x0300_7E00, 0xFFFF_FFFF);
        m.bus.write32(0x0300_7DFC, 0xFFFF_FFFF);
        m.bus.write8(RESET_FLAG, flag);
        m.cpu.switch_mode(mode::IRQ);
        m.cpu.r = [0x55; 15];
        m.cpu.spsr[Bank::Irq as usize] = 0x1F;
        call(&mut m, call::VBLANK_INTR_WAIT);
        call(&mut m, call::SOFT_RESET);
        assert_eq!(m.cpu.pc, entry);
        assert_eq!(m.cpu.cpsr, mode::SYS);
        let mut r = [0; 15];
        r[13] = SP_SYS;
        assert_eq!(m.cpu.r, r);
        assert_eq!(m.cpu.banks[Bank::Irq as usize][5..], [SP_IRQ, 0]);
        assert_eq!(m.cpu.banks[Bank::Svc as usize][5..], [SP_SVC, 0]);
        assert_eq!(m.cpu.spsr[Bank::Irq as usize], 0);
        assert_eq!(m.bus.read32(0x0300_7E00), 0);
        assert_eq!(m.bus.read32(0x0300_7DFC), 0xFFFF_FFFF);
        assert_eq!(m.bios, Bios::default());
        assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_BOOT);
    }
}

/// `HardReset` leaves the machine as `Machine::new` makes it, but for the
/// clock and the save, from anywhere: IRQ mode, Thumb, mid-wait.
#[test]
fn hard_reset_is_power_on() {
    let mut m = machine();
    m.bus.save.dirty = true;
    m.tick(5000);
    for a in [
        EWRAM,
        0x0300_0000,
        0x0300_7FFC,
        0x0500_0000,
        VRAM,
        0x0700_0000,
    ] {
        m.bus.write32(a, 0x1234_5678);
    }
    for (o, v) in [
        (reg::DISPCNT, 0x0403),
        (reg::BG0CNT, 0x1234),
        (reg::BG2PA, 0x55),
        (reg::IE, 1),
        (reg::IME, 1),
        (reg::WAITCNT, 0x4317),
        (reg::KEYCNT, 0x4001),
        (reg::SOUNDCNT_X, 0x80),
        (0x080, 0x7777),
        (reg::TM0CNT_L + 2, 0x80),
        (reg::DMA0SAD, 0x100),
    ] {
        m.bus.write16(IO + o, v);
    }
    m.cpu.switch_mode(mode::IRQ);
    m.cpu.r = [0x55; 15];
    m.cpu.spsr[Bank::Irq as usize] = 0x1F;
    call(&mut m, call::VBLANK_INTR_WAIT);
    m.cpu.cpsr |= T_BIT;
    m.cpu.pc = 0x0800_0102;
    let now = m.now();
    swi(&mut m, call::HARD_RESET as u32).unwrap();

    let fresh = machine();
    assert_eq!(m.cpu, crate::machine::boot_cpu());
    assert_eq!(m.bios, Bios::default());
    assert!(!m.sched.halted);
    assert_eq!(m.now(), now, "the clock runs on");
    assert!(m.bus.save.dirty, "the save is kept");
    assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_BOOT);
    assert!(m.bus.ewram == fresh.bus.ewram);
    assert!(m.bus.iwram == fresh.bus.iwram);
    assert!(m.bus.video.palette == fresh.bus.video.palette);
    assert!(m.bus.video.vram == fresh.bus.video.vram);
    assert!(m.bus.video.oam == fresh.bus.video.oam);
    // VCOUNT and DISPSTAT's flags are the LCD's, which runs on.
    for o in (0..crate::io::IO_SIZE)
        .step_by(2)
        .filter(|&o| o != 4 && o != 6)
    {
        assert_eq!(m.bus.io.raw16(o), fresh.bus.io.raw16(o), "I/O {o:#05x}");
    }
    assert_eq!(m.bus.waits, fresh.bus.waits);
}

/// No link partner answers `MultiBoot`: it fails.
#[test]
fn multi_boot_fails() {
    let mut m = machine();
    regs(&mut m, &[0x0200_0000, 1]);
    call(&mut m, call::MULTI_BOOT);
    assert_eq!(m.cpu.r[..2], [1, 1]);
    assert_eq!(m.cpu.pc, NEXT);
}

#[test]
fn register_ram_reset() {
    let mut m = machine();
    m.bus.write32(EWRAM, 1);
    m.bus.write32(0x0300_0000, 1);
    m.bus.write32(0x0300_7E00, 1);
    m.bus.write32(0x0500_0000, 1);
    m.bus.write32(VRAM, 1);
    m.bus.write32(0x0700_0000, 1);
    m.bus.write16(IO + reg::BG0CNT, 0x1234);
    m.bus.write16(IO + reg::BG2PA, 0x55);
    m.bus.write16(IO + reg::IE, 1);
    m.bus.write16(IO + reg::SOUNDBIAS, 0);
    // Only RAM, not registers.
    regs(&mut m, &[0x1F]);
    call(&mut m, call::REGISTER_RAM_RESET);
    assert_eq!(m.bus.io.dispcnt(), 0x80, "forced blank");
    for a in [EWRAM, 0x0300_0000, 0x0500_0000, VRAM, 0x0700_0000] {
        assert_eq!(m.bus.read32(a), 0, "{a:#x}");
    }
    assert_eq!(m.bus.read32(0x0300_7E00), 1, "the top of IWRAM is kept");
    assert_eq!(m.bus.read16(IO + reg::BG0CNT), 0x1234);
    // Then the registers.
    regs(&mut m, &[0xE0]);
    call(&mut m, call::REGISTER_RAM_RESET);
    assert_eq!(m.bus.read16(IO + reg::BG0CNT), 0);
    assert_eq!(m.bus.io.raw16(reg::BG2PA), 0x100);
    assert_eq!(m.bus.read16(IO + reg::IE), 0);
    assert_eq!(m.bus.read16(IO + reg::SOUNDBIAS), 0x200);
}

/// GBATEK: r0 = 0 moves the level to 0, anything else to 0x200; the
/// upper bits stay. (Bit 0 isn't stored.)
#[test]
fn sound_bias() {
    for (old, r0, new) in [
        (0x4200, 0, 0x4000),
        (0xC3FE, 0, 0xC000),
        (0x4000, 1, 0x4200),
        (0x83FE, 0xFFFF_FFFF, 0x8200),
        (0x0100, 0x200, 0x0200),
        (0x0200, 1, 0x0200),
    ] {
        let mut m = machine();
        m.bus.write16(IO + reg::SOUNDBIAS, old);
        regs(&mut m, &[r0, 1, 2, 3]);
        let (r, now) = (m.cpu.r, m.now());
        call(&mut m, call::SOUND_BIAS);
        assert_eq!(
            m.bus.read16(IO + reg::SOUNDBIAS),
            new,
            "{old:#x}, r0 = {r0:#x}"
        );
        assert_eq!(m.cpu.r, r, "no return value");
        assert_eq!(m.now() - now, 45);
        assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_SWI);
    }
}

#[test]
fn get_bios_checksum() {
    let mut m = machine();
    call(&mut m, call::GET_BIOS_CHECKSUM);
    assert_eq!(
        [m.cpu.r[0], m.cpu.r[1], m.cpu.r[3]],
        [0xBAAE_187F, 1, 0x4000]
    );
}

#[test]
fn bit_unpack_wide_units_and_a_partial_word() {
    let mut m = machine();
    m.bus.write16(EWRAM, 0x3412);
    let info = EWRAM + 0x10;
    for (dst_width, len, want) in [
        (16, 2, &[0x0034_0012u32][..]),
        (32, 2, &[0x12, 0x34][..]),
        // 8 to 16 from one byte: half a word, never written.
        (16, 1, &[][..]),
    ] {
        m.bus.write32(EWRAM + 0x100, 0xAAAA_AAAA);
        m.bus.write16(info, len);
        m.bus.write8(info + 2, 8);
        m.bus.write8(info + 3, dst_width);
        m.bus.write32(info + 4, 0);
        regs(&mut m, &[EWRAM, EWRAM + 0x100, info]);
        call(&mut m, call::BIT_UNPACK);
        for (k, &w) in want.iter().enumerate() {
            assert_eq!(m.bus.read32(EWRAM + 0x100 + 4 * k as u32), w);
        }
        if want.is_empty() {
            assert_eq!(m.bus.read32(EWRAM + 0x100), 0xAAAA_AAAA);
        }
        assert_eq!(m.cpu.r[1], EWRAM + 0x100 + 4 * want.len() as u32);
    }
}

#[test]
fn cpu_fast_set_of_nothing() {
    let mut m = machine();
    m.bus.write32(EWRAM, 5);
    regs(&mut m, &[EWRAM, EWRAM + 0x100, 1 << 24]);
    call(&mut m, call::CPU_FAST_SET);
    assert_eq!(m.bus.read32(EWRAM + 0x100), 0);
    assert_eq!(m.cpu.r[..3], [EWRAM, EWRAM + 0x100, 1 << 24]);
}

#[test]
fn affine_sets_wrap_at_the_top_of_memory() {
    let mut m = machine();
    regs(&mut m, &[0xFFFF_FFF8, 0xFFFF_FFF8, 1, 2]);
    call(&mut m, call::BG_AFFINE_SET);
    assert_eq!(m.cpu.r[..3], [12, 8, u32::MAX]);
    regs(&mut m, &[0xFFFF_FFFC, 0xFFFF_FFFC, (-3i32) as u32, 2]);
    call(&mut m, call::OBJ_AFFINE_SET);
    assert_eq!(
        m.cpu.r[2],
        (-4i32) as u32,
        "a negative count ends at count - 1"
    );
}

#[test]
fn register_ram_reset_acknowledges_and_disables_interrupts() {
    let mut m = machine();
    m.bus.io.write16(reg::IE, 1);
    m.bus.io.write16(reg::IME, 1);
    m.bus.io.request_irq(1);
    m.bus.io.write16(reg::DISPCNT, 3);
    regs(&mut m, &[0]);
    call(&mut m, call::REGISTER_RAM_RESET);
    assert_eq!(m.bus.io.dispcnt(), 0x80, "even with no flags");
    assert_eq!(m.bus.read16(IO + reg::IE), 1);
    regs(&mut m, &[0x80]);
    call(&mut m, call::REGISTER_RAM_RESET);
    for o in [reg::IE, reg::IF, reg::IME] {
        assert_eq!(m.bus.read16(IO + o), 0, "{o:#x}");
    }
}

/// A handler that waits inside the wait it interrupted (re-enabling IRQs):
/// the inner wait finishes first, then the outer one, each from its own
/// IRQ.
#[test]
fn a_wait_inside_a_wait() {
    let mut m = machine();
    call(&mut m, call::VBLANK_INTR_WAIT);
    let hblank = irq_in(&mut m);
    m.cpu.write_cpsr(mode::SYS, 0xFF);
    m.cpu.pc = 0x0300_1004;
    regs(&mut m, &[1, 8]);
    call(&mut m, call::INTR_WAIT);
    assert_eq!(m.bios.pending().len(), 2);
    irq(&mut m, 8);
    assert_eq!(m.cpu.r[0], 8, "the inner wait ended");
    assert_eq!(m.bios.pending().len(), 1);
    m.bus.io.take_events();
    m.cpu.write_cpsr(mode::IRQ | I_BIT, 0xFF);
    irq_out(&mut m, hblank);
    // Back at the outer wait with no V-blank yet: it halts again.
    assert!(halted(&mut m));
    assert_eq!(m.bios.pending().len(), 1);
    irq(&mut m, 1);
    assert!(m.bios.pending().is_empty());
    assert_eq!(m.cpu.r[0], 1);
}

/// A `Halt` left without an IRQ (IME off) is replaced by the next call at
/// its level, so a later `Halt` still ends with the after-swi BIOS read.
#[test]
fn a_new_call_replaces_a_stale_one() {
    let mut m = machine();
    call(&mut m, call::HALT);
    m.cpu.pc = NEXT + 0x40;
    call(&mut m, call::HALT);
    assert_eq!(m.bios.pending().len(), 1);
    irq(&mut m, 0);
    assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_SWI);
    assert!(m.bios.pending().is_empty());
}

#[test]
fn sound_driver_calls_return_at_once() {
    for &n in call::SOUND_DRIVER {
        let mut m = machine();
        regs(&mut m, &[EWRAM, 0x1234, 3, 4]);
        m.bus.write32(EWRAM, 0x5555_5555);
        let (cpu, now) = (m.cpu.clone(), m.now());
        call(&mut m, n);
        assert_eq!(m.cpu, cpu, "swi {n:#04x} leaves the registers");
        assert_eq!(m.bus.read32(EWRAM), 0x5555_5555, "swi {n:#04x}");
        assert_eq!(m.now() - now, 45, "swi {n:#04x} takes the entry time");
        assert_eq!(m.bus.read32(0), BIOS_LATCH_AFTER_SWI);
    }
}

#[test]
fn sound_driver_get_jump_list() {
    let mut m = machine();
    let table = EWRAM + 0x10;
    regs(&mut m, &[table, 7, 8, 9, 10]);
    m.bus.write32(table - 4, 0x1111_1111);
    m.bus.write32(table + 0x90, 0x2222_2222);
    call(&mut m, call::SOUND_DRIVER_GET_JUMP_LIST);
    for i in 0..36 {
        assert_eq!(m.bus.read32(table + 4 * i), NOP_CALL, "word {i}");
    }
    assert_eq!(m.bus.read32(table - 4), 0x1111_1111);
    assert_eq!(m.bus.read32(table + 0x90), 0x2222_2222);
    // mGBA's stmia r0! leaves r0 past the table; r2 is restored, r4 kept.
    assert_eq!(m.cpu.r[..5], [table + 0x90, 0, 8, NOP_CALL, 10]);
}

/// A call through the jump list returns to the caller, in its state.
#[test]
fn the_jump_list_returns() {
    for (lr, thumb) in [(0x0800_0201, true), (0x0800_0204, false)] {
        let mut m = machine();
        m.cpu.pc = NOP_CALL;
        m.cpu.r[14] = lr;
        m.step();
        assert_eq!(m.fault, None);
        assert_eq!(m.cpu.pc, lr & !1);
        assert_eq!(m.cpu.thumb(), thumb);
        assert_eq!(m.bus.read32(0), 0xE3A0_C301, "mGBA's prefetch after the bx");
    }
}
