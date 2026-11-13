//! gbaa test ROM for the serial port's registers in lifted code
//! (native/hw-serial-registers): a program that writes and reads every
//! register of `0x0400_0120`-`0x0400_015F` (`SIODATA32`/`SIOMULTI0`-`3`,
//! `SIOCNT`, `SIODATA8`, `KEYCNT`, `RCNT`, `JOYCNT`, `JOY_RECV`,
//! `JOY_TRANS`, `JOYSTAT` and the unused halfwords between them) with no
//! link partner, as a game's start-up clears them and its link library
//! polls them.
//!
//! Each frame `f` (from 0) the main loop first loads what the last frame
//! left, a wait later (`after`: a transfer it started is still started,
//! or not), then stores to every register, values from the frame's
//! number, as halfwords, as words (`SIODATA32`, `SIOCNT` with `SIODATA8`,
//! `KEYINPUT` with `KEYCNT`, `RCNT`, `JOYCNT`, `JOY_RECV`, `JOY_TRANS`,
//! `JOYSTAT`) and as bytes (`SIOCNT`'s high byte, `RCNT`'s low, `JOYCNT`'s
//! low, an unused one), and loads them back as halfwords, words and
//! bytes. `SIOCNT` goes through `sio_set(reg, v)`, Thumb in `global_asm!`,
//! one `strh`, which the lift check plants a wrong value in; its values
//! (`CNT`) start a transfer in each mode, with and without its IRQ. Every
//! eighth frame it then calls `RegisterRamReset(0x20)`, the serial
//! registers' flag, and loads them all again (`reset`).
//!
//! The serial IRQ is enabled, its handler counting in `IRQS`. What it
//! read is kept in `OUT`, a global in RAM, and folded into a pixel of
//! mode 3: row `f / 240 % 4`, column `f % 240`. A lift that drops or
//! mangles a store to these registers, reads one back wrongly, ends a
//! transfer or raises its IRQ where the play build doesn't, leaves
//! different RAM and draws a different frame. What it draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::ptr::{read_volatile, write_volatile};

core::arch::global_asm!(
    ".section .text.native_serial, \"ax\", %progbits",
    ".syntax unified",
    ".balign 4",
    ".thumb",
    // void sio_set(u16 *reg, u32 v)
    ".global sio_set",
    ".type sio_set, %function",
    ".thumb_func",
    "sio_set:",
    "    strh r1, [r0]",
    "    bx lr",
);

unsafe extern "C" {
    fn sio_set(reg: *mut u16, v: u32);
}

const IO: u32 = 0x0400_0000;
const DISPCNT: u32 = 0x000;
const SIODATA32: u32 = 0x120;
const SIOCNT: u32 = 0x128;
const SIODATA8: u32 = 0x12A;
const KEYINPUT: u32 = 0x130;
const KEYCNT: u32 = 0x132;
const RCNT: u32 = 0x134;
const JOYCNT: u32 = 0x140;
const JOY_RECV: u32 = 0x150;
const JOY_TRANS: u32 = 0x154;
const JOYSTAT: u32 = 0x158;
const VRAM: u32 = 0x0600_0000;

/// Mode 3, BG2 on.
const MODE3: u16 = 0x0403;

/// The halfwords of `0x120`-`0x15F`.
const HALVES: usize = 32;

/// `SIOCNT`, a frame in turn: a transfer started (bit 7) in each mode,
/// with its IRQ asked for (bit 14) and without.
const CNT: [u16; 8] = [
    0x4081, // normal, 8 bits, the internal clock, started, its IRQ
    0x1080, // normal, 32 bits, an external clock, started
    0x6083, // multiplayer at 115200 baud, started, its IRQ
    0x0000, // nothing
    0x7F8F, // UART, everything on
    0x0081, // normal, 8 bits, the internal clock, started, no IRQ
    0xFFFF, // every bit
    0x4003, // normal, the IRQ asked for and nothing started
];

/// What the program read, for the native check to compare.
#[repr(C, align(4))]
struct Out {
    /// Every halfword of `0x120`-`0x15F` as the last frame left it, loaded
    /// after the wait (`ldrh`).
    after: [u16; HALVES],
    /// Every halfword after this frame's stores (`ldrh`).
    regs: [u16; HALVES],
    /// `ldr` of `SIODATA32`, `SIOCNT`, `KEYINPUT`, `RCNT`, `JOYCNT`,
    /// `JOY_RECV`, `JOY_TRANS` and `JOYSTAT`.
    words: [u32; 8],
    /// `ldrb` of `SIOCNT`'s two bytes, `RCNT`'s two, `JOYCNT`'s low,
    /// `SIODATA8`'s low, `KEYCNT`'s high and an unused one.
    bytes: [u8; 8],
    /// Every halfword after `RegisterRamReset(0x20)`, on the frames that
    /// call it (else the last such frame's).
    reset: [u16; HALVES],
    /// Serial IRQs taken so far, as this frame's start saw them.
    irqs: u32,
}

/// RAM the main loop and the serial handler share. (ARMv4T has no atomics.)
struct Shared<T>(UnsafeCell<T>);

// SAFETY: only the main loop touches OUT; only the handler writes IRQS,
// which the main loop only reads.
unsafe impl<T> Sync for Shared<T> {}

static OUT: Shared<Out> = Shared(UnsafeCell::new(Out {
    after: [0; HALVES],
    regs: [0; HALVES],
    words: [0; 8],
    bytes: [0; 8],
    reset: [0; HALVES],
    irqs: 0,
}));
static IRQS: Shared<u32> = Shared(UnsafeCell::new(0));

fn serial_irq() {
    let c = IRQS.0.get();
    // SAFETY: the handler's own count (see Shared).
    unsafe { write_volatile(c, read_volatile(c) + 1) };
}

fn ldrb(o: u32) -> u8 {
    // SAFETY: a byte of an I/O register.
    unsafe { read_volatile((IO + o) as *const u8) }
}

fn ldrh(o: u32) -> u16 {
    // SAFETY: an I/O register's halfword.
    unsafe { read_volatile((IO + o) as *const u16) }
}

fn ldr(o: u32) -> u32 {
    // SAFETY: two I/O registers' word.
    unsafe { read_volatile((IO + o) as *const u32) }
}

fn strb(o: u32, v: u32) {
    // SAFETY: a byte of an I/O register.
    unsafe { write_volatile((IO + o) as *mut u8, v as u8) };
}

fn strh(o: u32, v: u32) {
    // SAFETY: an I/O register's halfword.
    unsafe { write_volatile((IO + o) as *mut u16, v as u16) };
}

fn str(o: u32, v: u32) {
    // SAFETY: two I/O registers' word.
    unsafe { write_volatile((IO + o) as *mut u32, v) };
}

/// Every halfword of the serial registers' range, loaded into `to`.
fn load_all(to: &mut [u16; HALVES]) {
    for (i, r) in to.iter_mut().enumerate() {
        *r = ldrh(SIODATA32 + 2 * i as u32);
    }
}

/// One frame's loads and stores, into `o`.
fn frame(f: u32, o: &mut Out) {
    // What the last frame left, a wait later.
    load_all(&mut o.after);
    // SAFETY: the handler's count, read only (see Shared).
    o.irqs = unsafe { read_volatile(IRQS.0.get()) };
    let x = f.wrapping_mul(0x9E37_79B9);
    // Halfwords, the unused ones between the registers too.
    for i in 0..HALVES as u32 {
        let at = SIODATA32 + 2 * i;
        if at != SIOCNT && at != KEYINPUT {
            strh(at, x.rotate_left(i) ^ i * 0x1111);
        }
    }
    // SAFETY: sio_set stores SIOCNT's halfword, as AAPCS keeps everything
    // else.
    unsafe { sio_set((IO + SIOCNT) as *mut u16, CNT[(f % 8) as usize] as u32) };
    // Words and bytes, a different set each frame.
    match f % 4 {
        0 => {
            str(SIODATA32, x ^ 0x1234_5678);
            str(JOY_RECV, !x);
            strb(RCNT, f);
        }
        1 => {
            str(SIOCNT, CNT[(f / 8 % 8) as usize] as u32 | x << 16);
            str(JOY_TRANS, x.rotate_left(9));
            strb(SIOCNT + 1, f >> 1);
        }
        2 => {
            str(KEYINPUT, x);
            str(RCNT, x.rotate_left(3));
            str(JOYSTAT, x.rotate_left(17));
            strb(JOYCNT, f);
        }
        _ => {
            str(JOYCNT, !x);
            str(SIODATA32 + 4, x.rotate_left(21));
            strb(SIOCNT + 5, f);
            strb(SIODATA8 + 1, f);
        }
    }
    load_all(&mut o.regs);
    for (w, at) in o.words.iter_mut().zip([
        SIODATA32, SIOCNT, KEYINPUT, RCNT, JOYCNT, JOY_RECV, JOY_TRANS, JOYSTAT,
    ]) {
        *w = ldr(at);
    }
    for (b, at) in o.bytes.iter_mut().zip([
        SIOCNT,
        SIOCNT + 1,
        RCNT,
        RCNT + 1,
        JOYCNT,
        SIODATA8,
        KEYCNT + 1,
        SIOCNT + 5,
    ]) {
        *b = ldrb(at);
    }
    if f % 8 == 5 {
        // SAFETY: RegisterRamReset of the serial registers alone; it
        // also blanks the display, which is set again after it.
        unsafe {
            asm!(
                "swi 0x01",
                inout("r0") 0x20 => _,
                out("r1") _,
                out("r2") _,
                out("r3") _,
                out("r12") _,
            );
        }
        strh(DISPCNT, MODE3 as u32);
        load_all(&mut o.reset);
    }
}

/// Everything the frame read, folded into a BGR555 colour.
fn colour(o: &Out) -> u16 {
    let mut v = o.irqs;
    for &h in o.after.iter().chain(&o.regs).chain(&o.reset) {
        v = v.rotate_left(5) ^ h as u32;
    }
    for &w in &o.words {
        v = v.rotate_left(7) ^ w;
    }
    for &b in &o.bytes {
        v = v.rotate_left(3) ^ b as u32;
    }
    ((v ^ v >> 15 ^ v >> 30) & 0x7FFF) as u16
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    use agb::interrupt::{Interrupt, add_interrupt_handler};
    // SAFETY: the handler is kept for the whole program and only steps
    // its count (see Shared).
    let _serial = unsafe { add_interrupt_handler(Interrupt::Serial, |_| serial_irq()) };
    let vblank = agb::interrupt::VBlank::get();
    strh(DISPCNT, MODE3 as u32);
    let mut f: u32 = 0;
    loop {
        // SAFETY: OUT is the main loop's (see Shared).
        let o = unsafe { &mut *OUT.0.get() };
        frame(f, o);
        let px = VRAM + ((f / 240 % 4) * 240 + f % 240) * 2;
        // SAFETY: a pixel of mode 3's frame buffer, rows 0-3.
        unsafe { write_volatile(px as *mut u16, colour(o)) };
        vblank.wait_for_vblank();
        f += 1;
    }
}
