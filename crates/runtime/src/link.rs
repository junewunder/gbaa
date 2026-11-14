//! The link hub: up to four GBAs on one serial cable, in one process
//! (`docs/research/multiplayer.md`, "GBA adapter: the link hub").
//!
//! A [`Link`] doesn't own the machines: each call borrows them, player 0
//! (the parent, whose SI terminal is grounded) first. It runs them in
//! turns, each up to a shared deadline: a scanline ([`QUANTUM`]) ahead, or
//! the end of the transfer in flight if that is sooner. Player 0 goes
//! first; its turn also ends when it asks for a transfer (a `SIOCNT` write
//! with the start bit, which is a sync point, [`crate::io::event::SERIAL`]).
//! The others then run up to that cycle, every player's `SIOMLT_SEND` is
//! latched and its busy bit set, and the transfer completes on every
//! machine at the same cycle of hub time, [`TRANSFER_CYCLES`] later:
//! `SIOMULTI0`-`3` get the four halfwords (`0xFFFF` for a missing player),
//! the busy bit clears, the player ID shows in bits 4-5 and the serial IRQ
//! is raised where `SIOCNT` enables it. A machine overshoots a deadline by
//! up to a block (or an instruction, in the interpreter), so it sees the
//! end a little late, as decision 11's block-granular ticks see any event.
//! The order of turns is fixed, so a run is deterministic: what netplay
//! needs.
//!
//! The hub owns `SIOCNT`'s terminal, ID and error bits on a machine in
//! multiplayer mode ([`crate::io::Io::sio_owned`]), and the busy bit while
//! a transfer is in flight (always, on a child): SI is 0 on the parent of
//! a linked group and 1 elsewhere; SD is 1 when every player is in
//! multiplayer mode. Normal, UART and JOY bus modes aren't linked yet: a
//! machine in one of them sees no partner, as with no cable. The wireless
//! adapter, which speaks normal 32-bit mode, is [`rfu`]'s own hub.
//!
//! Hub time is cycles since [`Link::new`], the same on every machine at a
//! turn's end; each machine's own clock is offset from it by where it was
//! then. The hub's own state (hub time, a transfer in flight) isn't in the
//! machines' save states.

pub mod rfu;

use crate::io::{Io, irq, reg};
use crate::machine::Machine;
use crate::platform::Platform;
use crate::sched::LINE_CYCLES;

/// The most consoles a cable links.
pub const MAX_PLAYERS: usize = 4;

/// How far a turn runs with no transfer in flight.
pub const QUANTUM: u64 = LINE_CYCLES;

/// Cycles a multiplayer transfer takes, by `SIOCNT`'s baud bits (9600,
/// 38400, 57600, 115200 bps) and the number of players less one: mGBA's
/// table (`GBASIOCyclesPerTransfer`, `src/gba/sio.c`).
pub const TRANSFER_CYCLES: [[u64; MAX_PLAYERS]; 4] = [
    [38326, 73003, 107680, 142356],
    [9582, 18251, 26920, 35589],
    [6388, 12167, 17947, 23726],
    [3194, 6075, 8973, 11863],
];

/// `SIOCNT`'s bits in multiplayer mode (GBATEK).
pub mod sio {
    pub const BAUD: u16 = 3;
    /// SI terminal: 0 on the parent, 1 on a child.
    pub const SI: u16 = 1 << 2;
    /// SD terminal: 1 when every console is ready.
    pub const SD: u16 = 1 << 3;
    pub const ID_SHIFT: u16 = 4;
    pub const ID: u16 = 3 << ID_SHIFT;
    pub const ERROR: u16 = 1 << 6;
    /// Written 1 by the parent to start a transfer; reads 1 while busy.
    pub const START: u16 = 1 << 7;
    pub const MODE: u16 = 3 << 12;
    pub const MULTI: u16 = 2 << 12;
    pub const IRQ: u16 = 1 << 14;
}

/// Whether a machine's serial port is in multiplayer mode: `RCNT` bit 15
/// clear and `SIOCNT` bits 12-13 `10`.
pub fn multiplayer(io: &Io) -> bool {
    io.raw16(reg::RCNT) & 0x8000 == 0 && io.raw16(reg::SIOCNT) & sio::MODE == sio::MULTI
}

/// Whether the CPU wrote the start bit on a linked machine in multiplayer
/// mode and the hub hasn't answered yet: it ends the machine's turn.
pub(crate) fn start_requested(io: &Io) -> bool {
    io.sio_linked
        && io.sio_owned & sio::START == 0
        && io.raw16(reg::SIOCNT) & sio::START != 0
        && multiplayer(io)
}

/// A transfer in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Transfer {
    /// When it completes, in hub time.
    end: u64,
    /// Each player's `SIOMLT_SEND` at the start.
    data: [u16; MAX_PLAYERS],
}

/// The hub: the cable's state between turns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    players: usize,
    /// Hub time: every machine has reached it.
    now: u64,
    /// Each machine's clock at hub time 0.
    base: [u64; MAX_PLAYERS],
    transfer: Option<Transfer>,
    /// Transfers completed.
    pub transfers: u64,
}

impl Link {
    /// Plug the cable into `ms` (1 to [`MAX_PLAYERS`] machines, the parent
    /// first), at hub time 0.
    pub fn new<P: Platform>(ms: &mut [&mut Machine<P>]) -> Link {
        assert!(
            (1..=MAX_PLAYERS).contains(&ms.len()),
            "a link has 1 to {MAX_PLAYERS} players"
        );
        let mut base = [0; MAX_PLAYERS];
        for (b, m) in base.iter_mut().zip(ms.iter_mut()) {
            *b = m.now();
            m.bus.io.sio_linked = true;
        }
        let l = Link {
            players: ms.len(),
            now: 0,
            base,
            transfer: None,
            transfers: 0,
        };
        l.refresh(ms);
        l
    }

    /// Unplug the cable: the machines run on their own again, their serial
    /// registers stored as written. A transfer in flight never completes.
    pub fn unplug<P: Platform>(self, ms: &mut [&mut Machine<P>]) {
        for m in ms.iter_mut() {
            m.bus.io.sio_linked = false;
            m.bus.io.sio_owned = 0;
            m.set_deadline(u64::MAX);
        }
    }

    /// Hub time: cycles since [`Link::new`].
    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn players(&self) -> usize {
        self.players
    }

    /// Run every machine `cycles` further, in turns. `run` runs machine
    /// `i` until its run loop stops at the frame end or the deadline the
    /// hub set ([`Machine::run_frame`], or
    /// [`crate::dispatch::run_frame`] with recompiled code; not
    /// [`crate::dispatch::run_guest`]). Each machine presents its frames
    /// to its own platform as it reaches them, and reads its keys there.
    /// Stops early if a machine faults.
    pub fn run<P: Platform>(
        &mut self,
        ms: &mut [&mut Machine<P>],
        cycles: u64,
        mut run: impl FnMut(usize, &mut Machine<P>),
    ) {
        assert_eq!(
            ms.len(),
            self.players,
            "the machines the link was made with"
        );
        let end = self.now + cycles;
        while self.now < end {
            if ms.iter().any(|m| m.fault.is_some()) {
                return;
            }
            let mut d = (self.now + QUANTUM).min(end);
            if let Some(t) = self.transfer {
                d = d.min(t.end);
            }
            if self.turn(ms, 0, d, &mut run) {
                self.start(ms, &mut run);
                continue;
            }
            for i in 1..self.players {
                self.turn(ms, i, d, &mut run);
            }
            self.now = d;
            if self.transfer.is_some_and(|t| t.end <= d) {
                self.complete(ms);
            }
            self.refresh(ms);
        }
    }

    /// Run machine `i` until it reaches hub time `d`. Whether it stopped
    /// short instead, the parent asking for a transfer the hub can start.
    fn turn<P: Platform>(
        &mut self,
        ms: &mut [&mut Machine<P>],
        i: usize,
        d: u64,
        run: &mut impl FnMut(usize, &mut Machine<P>),
    ) -> bool {
        let ready = self.ready(ms);
        let idle = self.transfer.is_none();
        let m = &mut *ms[i];
        let target = self.base[i] + d;
        let mut asked = false;
        loop {
            if start_requested(&m.bus.io) {
                if i == 0 && ready && idle {
                    asked = true;
                    break;
                }
                // Only a ready parent starts one; the bit doesn't stick.
                let v = m.bus.io.raw16(reg::SIOCNT) & !sio::START;
                m.bus.io.set_raw16(reg::SIOCNT, v);
            }
            if m.now() >= target || m.fault.is_some() {
                break;
            }
            m.set_deadline(target);
            run(i, m);
        }
        m.set_deadline(u64::MAX);
        asked
    }

    /// Whether every player is in multiplayer mode, and there are two or
    /// more: SD.
    fn ready<P: Platform>(&self, ms: &[&mut Machine<P>]) -> bool {
        self.players > 1 && ms.iter().all(|m| multiplayer(&m.bus.io))
    }

    /// The parent asked for a transfer: bring the others to its hub time,
    /// latch every player's halfword and start it there.
    fn start<P: Platform>(
        &mut self,
        ms: &mut [&mut Machine<P>],
        run: &mut impl FnMut(usize, &mut Machine<P>),
    ) {
        let s = ms[0].now() - self.base[0];
        for i in 1..self.players {
            self.turn(ms, i, s, run);
        }
        let mut data = [0xFFFF; MAX_PLAYERS];
        for (d, m) in data.iter_mut().zip(ms.iter_mut()) {
            let io = &mut m.bus.io;
            if !multiplayer(io) {
                continue;
            }
            *d = io.raw16(reg::SIOMLT_SEND);
            for p in 0..MAX_PLAYERS as u32 {
                io.set_raw16(reg::SIOMULTI0 + 2 * p, 0xFFFF);
            }
            let v = io.raw16(reg::SIOCNT) | sio::START;
            io.set_raw16(reg::SIOCNT, v);
        }
        let baud = (ms[0].bus.io.raw16(reg::SIOCNT) & sio::BAUD) as usize;
        self.transfer = Some(Transfer {
            end: s + TRANSFER_CYCLES[baud][self.players - 1],
            data,
        });
        self.refresh(ms);
    }

    /// The transfer in flight ends on every machine.
    fn complete<P: Platform>(&mut self, ms: &mut [&mut Machine<P>]) {
        let Some(t) = self.transfer.take() else {
            return;
        };
        for (i, m) in ms.iter_mut().enumerate() {
            let io = &mut m.bus.io;
            if !multiplayer(io) {
                continue;
            }
            for (p, &v) in t.data.iter().enumerate() {
                io.set_raw16(reg::SIOMULTI0 + 2 * p as u32, v);
            }
            let v = io.raw16(reg::SIOCNT) & !(sio::START | sio::ID | sio::ERROR)
                | (i as u16) << sio::ID_SHIFT;
            io.set_raw16(reg::SIOCNT, v);
            if v & sio::IRQ != 0 {
                io.request_irq(irq::SERIAL);
            }
        }
        self.transfers += 1;
    }

    /// Set the terminals the cable gives each machine, and which bits of
    /// `SIOCNT` the hub owns there.
    fn refresh<P: Platform>(&self, ms: &mut [&mut Machine<P>]) {
        let ready = self.ready(ms);
        let busy = self.transfer.is_some();
        for (i, m) in ms.iter_mut().enumerate() {
            let io = &mut m.bus.io;
            if !multiplayer(io) {
                io.sio_owned = 0;
                continue;
            }
            let si = i > 0 || self.players < 2;
            let v = io.raw16(reg::SIOCNT) & !(sio::SI | sio::SD)
                | if si { sio::SI } else { 0 }
                | if ready { sio::SD } else { 0 };
            io.set_raw16(reg::SIOCNT, v);
            io.sio_owned = sio::SI
                | sio::SD
                | sio::ID
                | sio::ERROR
                | if i > 0 || busy { sio::START } else { 0 };
        }
    }
}

#[cfg(test)]
mod tests;
