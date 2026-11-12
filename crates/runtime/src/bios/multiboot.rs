//! `MultiBoot` (SWI 0x25) and the boot-time receive of a GBA with no
//! cartridge: single-pak link play (`docs/research/multiplayer.md`, "The
//! cable over the internet"). The parent's game sends a program over the
//! cable in multiplayer mode; a child with no cartridge takes it into EWRAM
//! and boots it there.
//!
//! The exchange is GBATEK's ("BIOS Multi Boot (Single Game Pak)"), each
//! row one multiplayer transfer, the parent's halfword out and each
//! child's back (what a child sends in a row it set after the row before):
//!
//! | parent sends | child replies | what |
//! |---|---|---|
//! | `6200` | `0000`, then `720x` | the child is in multiplayer mode; `x` its bit, `2 << (id - 1)` |
//! | `610y` | `720x` | recognised: `y` the children the parent saw |
//! | header ×60h | `NN0x` | the 0xC0-byte header, a halfword a row, `NN` counting 60h down |
//! | `6200`, `620y` | `000x`, `720x` | header done, information again |
//! | `63pp` | `720x`, then `73cc` | palette byte `pp`; `cc` the child's random byte |
//! | `64hh` | `73cc` | `hh` = 11h + every child's `cc` (FFh for none) |
//! | `llll` | `73rr` | (`MultiBoot` from here) the length: bytes / 4 - 34h |
//! | data | `nnnn` | the program, a halfword a row, to `0x020000C0` on |
//! | `0065` | `0075` | done: until every child answers `0075` |
//! | `0066`, CRC | `0075`, CRC | the check value both sides computed |
//!
//! The game does the rows up to `64hh` itself; `MultiBoot` does the rest.
//! On each side the BIOS part runs as Rust at [`LOOP`], a BIOS address the
//! CPU waits at (as a call's stall waits at [`super::STALL_LOOP`]), doing
//! real transfers over the link hub ([`crate::link`]), so the program
//! takes the time 16-bit multiplayer transfers take. Both ends are this
//! BIOS, so the main transfer leaves out the real BIOS's encryption and
//! uses a check value of its own ([`crc`]); what the game itself sends
//! and sees, the rows before `MultiBoot`, is GBATEK's.
//!
//! Only multiplayer mode (r1 = 1) is linked: normal mode's `MultiBoot`
//! (r1 = 0 or 2) fails, as it does with no cable or no child.

use crate::io::{irq, reg};
use crate::link::{self, sio};
use crate::machine::boot_cpu;
use crate::{Machine, Platform};

/// Where the CPU waits while `MultiBoot` sends, or a machine with no
/// cartridge waits for a program: run by [`step`].
pub const LOOP: u32 = 0x3A0;
/// Where the program boots.
pub const ENTRY: u32 = 0x0200_00C0;
/// The header's halfwords.
const HEADER: u32 = 0x60;
/// The longest program, past the header: EWRAM's 256 KB.
pub const MAX_LEN: u32 = 0x4_0000 - 0xC0;
/// How long `MultiBoot` waits for a transfer, or for its children to be
/// ready, before it fails: 1/16 s.
const TIMEOUT: u64 = 1 << 20;
/// The BIOS's own wait before it sends the length (GBATEK's DELAY).
const DELAY: u64 = 1 << 20;
/// The parent's pause after each transfer, for the children's BIOS to take
/// the halfword and set its reply (theirs in an IRQ handler, ours at its
/// next step).
const GAP: u64 = 64;
/// How many `0065`s `MultiBoot` sends waiting for every child's `0075`.
const FINISH_TRIES: u32 = 256;
/// `CPSR`'s IRQ-disable bit: `MultiBoot` runs with IRQs off, as the real
/// one runs in the `swi` exception.
const CPSR_I: u32 = 1 << 7;

/// Who is in a `MultiBoot` exchange, and how far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MultiBoot {
    /// [`role`]'s values.
    pub role: u8,
    /// The role's [`phase`].
    pub phase: u8,
    /// The program's length in bytes, past the header.
    pub len: u32,
    /// Halfwords of the header or program sent or taken; the parent's
    /// `0065`s.
    pub pos: u32,
    /// The parent's program: where it starts.
    pub src: u32,
    /// The children the parent sends to (`MultiBoot`'s client bits); a
    /// child's own bit.
    pub client: u16,
    /// A child's random byte (`cc`).
    pub cc: u16,
    /// The check value so far ([`crc`]).
    pub crc: u16,
    /// The parent: whether a transfer is in flight.
    pub busy: bool,
    /// The parent: when it may go on (after [`DELAY`] or [`GAP`]), and when
    /// it gives up waiting for a transfer.
    pub wait: u64,
    pub timeout: u64,
    /// The parent: where the call returns to, and the `CPSR` I bit and
    /// `SIOCNT` IRQ bit it had.
    pub ret: u32,
    pub cpsr_i: u32,
    pub sio_irq: u16,
}

/// [`MultiBoot::role`].
pub mod role {
    pub const NONE: u8 = 0;
    /// A machine with no cartridge, waiting for a program.
    pub const CHILD: u8 = 1;
    /// `MultiBoot` sending.
    pub const PARENT: u8 = 2;
}

/// [`MultiBoot::phase`].
pub mod phase {
    // A child's.
    pub const WAIT: u8 = 0;
    pub const HEADER: u8 = 1;
    pub const INFO: u8 = 2;
    pub const PALETTE: u8 = 3;
    pub const LENGTH: u8 = 4;
    pub const DATA: u8 = 5;
    pub const FINISH: u8 = 6;
    pub const CHECK: u8 = 7;
    // The parent's.
    pub const DELAY: u8 = 10;
    pub const SEND_LENGTH: u8 = 11;
    pub const SEND_DATA: u8 = 12;
    pub const SEND_FINISH: u8 = 13;
    pub const SEND_CRC_NEXT: u8 = 14;
    pub const SEND_CRC: u8 = 15;
}

/// The check value both ends keep over the program's halfwords (this
/// BIOS's own: GBATEK's runs over the encrypted stream, which ours leaves
/// out).
pub fn crc(c: u16, h: u16) -> u16 {
    (c ^ h).rotate_left(5).wrapping_add(0x9E37)
}

/// Where [`crc`] starts.
const CRC_START: u16 = 0xC387;

/// Make `m`, a machine with no cartridge (`Machine::new` with an empty
/// ROM), as its BIOS leaves it at power-on:
/// waiting at [`LOOP`] for a program over the cable, its serial port in
/// multiplayer mode at 115,200 bps with the serial IRQ flagged (not taken:
/// `IE` and `IME` are clear), `SIOMLT_SEND` 0. The boot logo isn't shown,
/// as a cartridge's boot doesn't show it.
pub fn boot_cartless<P: Platform>(m: &mut Machine<P>) {
    m.cpu = boot_cpu();
    m.cpu.pc = LOOP;
    m.cpu.cpsr |= CPSR_I;
    m.bios.multiboot = MultiBoot {
        role: role::CHILD,
        ..MultiBoot::default()
    };
    let io = &mut m.bus.io;
    io.write16(reg::RCNT, 0);
    io.write16(reg::SIOCNT, sio::MULTI | sio::IRQ | 3);
    io.write16(reg::SIOMLT_SEND, 0);
}

/// `MultiBoot` (r0: the parameters, r1: the mode): start sending, or fail
/// at once (r0 = 1, nothing else changed) with no linked child to send to,
/// parameters out of range, or a mode other than multiplayer. Sending, the
/// CPU waits at [`LOOP`] and the call ends there ([`step`]), r0 0 when
/// every child took the program and 1 when not.
pub fn start<P: Platform>(m: &mut Machine<P>) {
    let p = m.cpu.r[0];
    let client = m.bus.read8(p + 0x1E) as u16 & 0xE;
    let src = m.bus.read32(p + 0x20);
    let len = m.bus.read32(p + 0x24).wrapping_sub(src);
    let io = &m.bus.io;
    let parent = io.sio_linked
        && link::multiplayer(io)
        && io.raw16(reg::SIOCNT) & (sio::SI | sio::SD) == sio::SD;
    if m.cpu.r[1] != 1
        || !parent
        || client == 0
        || !len.is_multiple_of(0x10)
        || !(0x100..=MAX_LEN).contains(&len)
    {
        m.cpu.r[0] = 1;
        return;
    }
    let siocnt = io.raw16(reg::SIOCNT);
    m.bios.multiboot = MultiBoot {
        role: role::PARENT,
        phase: phase::DELAY,
        len,
        src,
        client,
        crc: CRC_START,
        wait: m.now() + DELAY,
        ret: m.cpu.pc,
        cpsr_i: m.cpu.cpsr & CPSR_I,
        sio_irq: siocnt & sio::IRQ,
        ..MultiBoot::default()
    };
    m.cpu.cpsr |= CPSR_I;
    m.bus.io.write16(reg::SIOCNT, siocnt | sio::IRQ);
    m.cpu.pc = LOOP;
}

/// Run the BIOS at [`LOOP`] a little: answer a transfer that completed,
/// start the next, and wait to the next event. The cycles it took; `None`
/// when no exchange is running (the CPU got here some other way).
pub fn step<P: Platform>(m: &mut Machine<P>) -> Option<u32> {
    match m.bios.multiboot.role {
        role::CHILD => {
            if take_serial(m) {
                child(m);
            }
        }
        role::PARENT => {
            if let Some(n) = parent(m) {
                return Some(n);
            }
        }
        _ => return None,
    }
    if m.cpu.pc != LOOP {
        // Booted, or the call returned: what it took is ticked after.
        return Some(1);
    }
    let mut until = m.sched.next_event;
    let mb = &m.bios.multiboot;
    if mb.role == role::PARENT {
        until = until.min(if mb.busy { mb.timeout } else { mb.wait });
    }
    Some(until.saturating_sub(m.now()).clamp(1, u32::MAX as u64) as u32)
}

/// Whether a transfer completed since the last look (the serial IRQ's
/// flag, which `MultiBoot` and the child's BIOS keep enabled in `SIOCNT`),
/// clearing the flag.
fn take_serial<P: Platform>(m: &mut Machine<P>) -> bool {
    let io = &mut m.bus.io;
    let f = io.raw16(reg::IF);
    io.set_raw16(reg::IF, f & !irq::SERIAL);
    f & irq::SERIAL != 0
}

/// A child's BIOS takes the parent's halfword and sets its reply.
fn child<P: Platform>(m: &mut Machine<P>) {
    let io = &m.bus.io;
    let got = io.raw16(reg::SIOMULTI0);
    let id = (io.raw16(reg::SIOCNT) & sio::ID) >> sio::ID_SHIFT;
    let me = 1u16 << id;
    let mut mb = m.bios.multiboot;
    let out = match mb.phase {
        phase::WAIT | phase::INFO => match got & 0xFF00 {
            0x6200 => Some(0x7200 | me),
            0x6100 if mb.phase == phase::WAIT && got & me != 0 => {
                mb.phase = phase::HEADER;
                mb.pos = 0;
                Some((HEADER as u16) << 8 | me)
            }
            0x6300 if mb.phase == phase::INFO => {
                // Random to the real BIOS; here any byte the state fixes.
                mb.cc = (m.bus.io.now as u16 >> 4 & 0x7F) | 0x80;
                mb.phase = phase::PALETTE;
                Some(0x7300 | mb.cc)
            }
            _ => None,
        },
        phase::HEADER => {
            m.bus.write16(0x0200_0000 + 2 * mb.pos, got);
            mb.pos += 1;
            if mb.pos == HEADER {
                mb.phase = phase::INFO;
            }
            Some(((HEADER - mb.pos) as u16) << 8 | me)
        }
        phase::PALETTE => match got & 0xFF00 {
            0x6300 => Some(0x7300 | mb.cc),
            0x6400 => {
                mb.phase = phase::LENGTH;
                Some(0x7300 | (mb.cc ^ 0x5A))
            }
            _ => None,
        },
        phase::LENGTH => {
            let len = (got as u32 + 0x34) * 4;
            if (0x100..=MAX_LEN).contains(&len) {
                mb.phase = phase::DATA;
                mb.len = len;
                mb.pos = 0;
                mb.crc = CRC_START;
                Some(0)
            } else {
                mb = restart();
                Some(0)
            }
        }
        phase::DATA => {
            m.bus.write16(ENTRY + 2 * mb.pos, got);
            mb.crc = crc(mb.crc, got);
            mb.pos += 1;
            if 2 * mb.pos == mb.len {
                mb.phase = phase::FINISH;
            }
            Some(mb.pos as u16)
        }
        phase::FINISH => match got {
            0x0065 => Some(0x0075),
            0x0066 => {
                mb.phase = phase::CHECK;
                Some(mb.crc)
            }
            _ => None,
        },
        phase::CHECK => {
            if got == mb.crc {
                boot(m, id);
                return;
            }
            mb = restart();
            Some(0)
        }
        _ => None,
    };
    m.bios.multiboot = mb;
    if let Some(v) = out {
        m.bus.io.write16(reg::SIOMLT_SEND, v);
    }
}

/// A child starting over: waiting for the parent's `6200`s again.
fn restart() -> MultiBoot {
    MultiBoot {
        role: role::CHILD,
        ..MultiBoot::default()
    }
}

/// A child boots the program it took: GBATEK's boot mode (3, multiplayer)
/// and its player ID into the header's bytes 0xC4 and 0xC5, then to
/// [`ENTRY`] in ARM state, system mode, as a cartridge's boot leaves the
/// CPU.
fn boot<P: Platform>(m: &mut Machine<P>, id: u16) {
    m.bus.write8(0x0200_00C4, 3);
    m.bus.write8(0x0200_00C5, id as u8);
    m.bios.multiboot = MultiBoot::default();
    let mut cpu = boot_cpu();
    cpu.pc = ENTRY;
    cpu.r[14] = ENTRY;
    m.cpu = cpu;
    m.prefetch.at = u64::MAX;
}

/// The parent's `MultiBoot` a step on; `Some` with the cycles it took when
/// it waits for nothing in particular, `None` to wait to the next event.
fn parent<P: Platform>(m: &mut Machine<P>) -> Option<u32> {
    let now = m.now();
    let mut mb = m.bios.multiboot;
    if mb.busy {
        if !take_serial(m) {
            if now >= mb.timeout {
                finish(m, false);
                return Some(1);
            }
            return None;
        }
        mb.busy = false;
        mb.wait = now + GAP;
        let io = &m.bus.io;
        let replies: [u16; 3] =
            core::array::from_fn(|i| io.raw16(reg::SIOMULTI0 + 2 + 2 * i as u32));
        let all = |want: u16| (1..4).all(|c| mb.client & 1 << c == 0 || replies[c - 1] == want);
        match mb.phase {
            phase::SEND_LENGTH => {
                mb.phase = phase::SEND_DATA;
                mb.pos = 0;
            }
            phase::SEND_DATA => {
                mb.pos += 1;
                if 2 * mb.pos == mb.len {
                    mb.phase = phase::SEND_FINISH;
                    mb.pos = 0;
                }
            }
            phase::SEND_FINISH => {
                if all(0x0075) {
                    mb.phase = phase::SEND_CRC_NEXT;
                } else {
                    mb.pos += 1;
                    if mb.pos >= FINISH_TRIES {
                        m.bios.multiboot = mb;
                        finish(m, false);
                        return Some(1);
                    }
                }
            }
            phase::SEND_CRC_NEXT => mb.phase = phase::SEND_CRC,
            _ => {
                let ok = all(mb.crc);
                m.bios.multiboot = mb;
                finish(m, ok);
                return Some(1);
            }
        }
        m.bios.multiboot = mb;
        return None;
    }
    if now < mb.wait {
        return None;
    }
    let out = match mb.phase {
        phase::DELAY => {
            mb.phase = phase::SEND_LENGTH;
            (mb.len / 4 - 0x34) as u16
        }
        phase::SEND_LENGTH => (mb.len / 4 - 0x34) as u16,
        phase::SEND_DATA => {
            let h = m.bus.read16(mb.src + 2 * mb.pos);
            if mb.pos == 0 {
                mb.crc = CRC_START;
            }
            mb.crc = crc(mb.crc, h);
            h
        }
        phase::SEND_FINISH => 0x0065,
        phase::SEND_CRC_NEXT => 0x0066,
        _ => mb.crc,
    };
    mb.busy = true;
    mb.timeout = now + TIMEOUT;
    m.bios.multiboot = mb;
    take_serial(m);
    let io = &mut m.bus.io;
    io.write16(reg::SIOMLT_SEND, out);
    let v = io.raw16(reg::SIOCNT) | sio::START;
    io.write16(reg::SIOCNT, v);
    Some(8)
}

/// `MultiBoot` returns: r0 0 when every child took the program, 1 when
/// not, the IRQ bits as they were, a serial flag it left cleared.
fn finish<P: Platform>(m: &mut Machine<P>, ok: bool) {
    let mb = m.bios.multiboot;
    m.bios.multiboot = MultiBoot::default();
    m.cpu.r[0] = !ok as u32;
    m.cpu.pc = mb.ret;
    m.cpu.cpsr = m.cpu.cpsr & !CPSR_I | mb.cpsr_i;
    let io = &mut m.bus.io;
    let v = io.raw16(reg::SIOCNT) & !sio::IRQ | mb.sio_irq;
    io.write16(reg::SIOCNT, v);
    take_serial(m);
    m.bus.bios_latch = crate::bus::BIOS_LATCH_AFTER_SWI;
}

impl crate::snap::Snap for MultiBoot {
    fn snap(&mut self, v: &mut dyn crate::snap::Visitor) {
        // Version 4 on; older states lack it (and were saved with none
        // running).
        if v.version() < 4 {
            return;
        }
        let MultiBoot {
            role,
            phase,
            len,
            pos,
            src,
            client,
            cc,
            crc,
            busy,
            wait,
            timeout,
            ret,
            cpsr_i,
            sio_irq,
        } = self;
        role.snap(v);
        phase.snap(v);
        len.snap(v);
        pos.snap(v);
        src.snap(v);
        client.snap(v);
        cc.snap(v);
        crc.snap(v);
        busy.snap(v);
        wait.snap(v);
        timeout.snap(v);
        ret.snap(v);
        cpsr_i.snap(v);
        sio_irq.snap(v);
    }
}
