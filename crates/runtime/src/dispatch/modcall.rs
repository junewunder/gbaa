//! Calls a mod adds (`docs/mod-view-design.md`, "A call a mod adds"):
//! `call!(m, pc, f, { … })` in a mod's code, a call with no ROM return
//! site. Its lr is a reserved key outside the GBA map ([`call_key`], one
//! per site), which the macro takes as "came back", putting back every
//! register and flag as it found them ([`leave`]; its value is the
//! callee's r0). Below the callee's stack it leaves a [`RECORD`]-word
//! record in guest RAM: the registers as they were where the block the
//! call is in began, the key it began at, and the reserved key. A save state, rewind or unwind inside the
//! call keeps the record (it is guest RAM) but loses the mod's Rust frame;
//! when the callee then returns to the reserved key with no frame waiting
//! there, the dispatcher puts the registers back and goes on at that key
//! ([`resume`]), counting it as a mod resume.

use crate::machine::Machine;
use crate::platform::Platform;
use armv4t::interp::{Bus as _, T_BIT};
use armv4t::sem::Width;

/// Where the reserved keys start: above every address the GBA maps.
pub const KEYS: u32 = 0xF000_0000;

/// Words in a call's record: r0 to r14, the CPSR, the key to go on at and
/// the reserved key, lowest first.
pub const RECORD: usize = 18;

/// The record's size in bytes, which the call takes off sp.
pub const RECORD_BYTES: u32 = 4 * RECORD as u32;

/// The reserved key of the call a mod writes at `file:line:col` (the
/// macro's `file!()`, `line!()` and `column!()`): FNV-1a of the three
/// into the bits under [`KEYS`], word-aligned, with the Thumb bit set.
pub const fn call_key(file: &str, line: u32, col: u32) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    let b = file.as_bytes();
    let mut i = 0;
    while i < b.len() {
        h = (h ^ b[i] as u32).wrapping_mul(0x0100_0193);
        i += 1;
    }
    let mut j = 0;
    let tail = [line, col];
    while j < 2 {
        let mut k = 0;
        while k < 4 {
            h = (h ^ ((tail[j] >> (8 * k)) & 0xff)).wrapping_mul(0x0100_0193);
            k += 1;
        }
        j += 1;
    }
    KEYS | (h & 0x0fff_fffc) | 1
}

/// Whether `key` is one of [`call_key`]'s, in either state (an ARM callee
/// returning without `bx` drops the Thumb bit).
#[inline(always)]
pub fn is_call_key(key: u32) -> bool {
    key >= KEYS
}

/// The registers a call's record keeps: r0 to r14 and the CPSR, as the
/// block found them (before the call's own writes).
#[inline]
pub fn regs<P: Platform>(m: &Machine<P>) -> [u32; 16] {
    let mut r = [0; 16];
    r[..15].copy_from_slice(&m.cpu.r);
    r[15] = m.cpu.cpsr;
    r
}

/// Enter a call at reserved key `key`, the registers `regs` and the key
/// `at` its block began at: write the record below sp, point sp at it and
/// lr at `key`. Returns the new sp, the callee's.
#[inline]
pub fn enter<P: Platform>(m: &mut Machine<P>, regs: &[u32; 16], at: u32, key: u32) -> u32 {
    let base = m.cpu.r[13].wrapping_sub(RECORD_BYTES) & !3;
    let words = regs.iter().copied().chain([at, key]);
    for (i, w) in words.enumerate() {
        m.bus.store(Width::Word, base.wrapping_add(4 * i as u32), w);
    }
    m.cpu.r[13] = base;
    m.cpu.r[14] = key;
    base
}

/// The call came back: put every register, the flags and the state back as `regs`
/// (the block's, [`regs`]) has them. The call leaves no trace in them; its
/// value is the r0 it came back with, which the caller takes first.
#[inline]
pub fn leave<P: Platform>(m: &mut Machine<P>, regs: &[u32; 16]) {
    m.cpu.r.copy_from_slice(&regs[..15]);
    let keep = 0xf000_0000 | T_BIT;
    m.cpu.cpsr = m.cpu.cpsr & !keep | regs[15] & keep;
}

/// The callee came back to `key` with no call waiting for it (its frame
/// was lost to a state load, a rewind or an unwind): if the record at sp
/// is `key`'s, put back its registers, point the CPU at its key, and
/// return that key. None when sp holds no record of `key`, or one from
/// another mode.
pub fn resume<P: Platform>(m: &mut Machine<P>, key: u32) -> Option<u32> {
    let base = m.cpu.r[13];
    let word =
        |m: &mut Machine<P>, i: usize| m.bus.load(Width::Word, base.wrapping_add(4 * i as u32));
    if word(m, RECORD - 1) & !3 != key & !3 {
        return None;
    }
    let mut rec = [0u32; RECORD];
    for (i, w) in rec.iter_mut().enumerate() {
        *w = word(m, i);
    }
    let (cpsr, at) = (rec[15], rec[16]);
    if (cpsr ^ m.cpu.cpsr) & 0x1f != 0 || (at & 1 != 0) != (cpsr & T_BIT != 0) {
        return None;
    }
    m.cpu.r.copy_from_slice(&rec[..15]);
    // The flags and the state; the mode is the same.
    let keep = 0xf000_0000 | T_BIT;
    m.cpu.cpsr = m.cpu.cpsr & !keep | cpsr & keep;
    m.cpu.pc = at & !1;
    Some(at)
}
