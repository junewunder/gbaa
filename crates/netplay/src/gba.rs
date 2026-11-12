//! The GBA's adapter: a session's GBAs on one link cable
//! ([`runtime::link`]), one a seat, the parent in seat 0, as a
//! [`Console`]. They run in the interpreter, or on a recompiled build's
//! code ([`Gba::with_code`]): a peer running a console recompiled would
//! desync from one running it interpreted, or on another build's code, so
//! the build ID says which. Recompiled, a turn of the cable unwinds
//! compiled code to the dispatcher at the hub's deadline, unless the front
//! end runs each console on a guest stack of its own ([`Turns`]).
//!
//! A machine's platform is a [`Seat`]: [`Pad`], or a front end's own
//! platform (recompiled code is built for one platform type), which hands
//! the consoles a [`Pad`] of its own.
//!
//! A seat's state is its machine's whole save state, cartridge save
//! included. The hub's own state (hub time, a transfer in flight) isn't in
//! the machines' states, so loading them plugs a new cable in: at the
//! start that's exactly where a straight run starts.
//!
//! When the other players leave, the front end unplugs the cable
//! ([`Gba::unplug`]): its own seat's console plays on alone, as pulling a
//! real cable leaves it, and the game says what it says of a lost link.
//!
//! The front end may give the seat it shows a [`Speaker`]: the sound is
//! the machine's output alone, so a seat with one computes what a seat
//! without one does, and the peers' hashes still match. The picture is
//! the PPU's output alone too: snapshots and hashes leave it out
//! ([`Machine::save_unseen`]), so frames rollback runs again aren't drawn.

use std::any::Any;

use runtime::dispatch::{Code, Stats};
use runtime::io::key;
use runtime::link::Link;
use runtime::platform::{Frame, Platform};
use runtime::sched::FRAME_CYCLES;
use runtime::{AudioChunk, Machine, Timing, snap, state};

use crate::{Console, Fnv, Snapshot};

/// The GBA's clock, cycles a second.
pub const CYCLES_PER_SECOND: u64 = 1 << 24;

/// Where a seat's sound goes: the front end's player. `Any`, so the front
/// end can reach its own type again (to pace by it, say).
pub trait Speaker: Any {
    /// The sound since the last call ([`Platform::audio`]).
    fn audio(&mut self, chunk: &AudioChunk<'_>);

    /// The frame is done (its [`Platform::present`]).
    fn end_frame(&mut self);
}

/// A seat's pad: the keys it holds this frame, and where its sound goes,
/// if anywhere. The front end shows frames from the machines' PPUs.
#[derive(Default)]
pub struct Pad {
    pub keys: u16,
    pub speaker: Option<Box<dyn Speaker>>,
    /// Rollback running frames again: the speaker hears none of them.
    pub quiet: bool,
}

impl Platform for Pad {
    fn present(&mut self, _frame: &Frame) {
        if let Some(s) = self.speaker.as_mut().filter(|_| !self.quiet) {
            s.end_frame();
        }
    }

    fn keys(&mut self) -> u16 {
        self.keys
    }

    fn audio(&mut self, chunk: &AudioChunk<'_>) {
        if let Some(s) = self.speaker.as_mut().filter(|_| !self.quiet) {
            s.audio(chunk);
        }
    }
}

/// A machine's platform in a session: whatever it is, the consoles see
/// only its [`Pad`], so every seat computes the same on any platform.
pub trait Seat: Platform + Sized + 'static {
    /// A seat's platform, its pad up.
    fn seat() -> Self;
    fn pad(&self) -> &Pad;
    fn pad_mut(&mut self) -> &mut Pad;
}

impl Seat for Pad {
    fn seat() -> Pad {
        Pad::default()
    }

    fn pad(&self) -> &Pad {
        self
    }

    fn pad_mut(&mut self) -> &mut Pad {
        self
    }
}

/// How recompiled consoles take their turns of the cable, given to
/// [`Gba::set_turns`]; without, each turn runs
/// [`runtime::dispatch::run_frame`], which unwinds compiled code to the
/// dispatcher at the hub's deadline, a scanline at most, and a structured
/// function resumed past its entry runs in the interpreter to the next
/// key. A front end with guest stacks (the desktop's `coro`) runs each
/// console on its own, switching out at the deadline as at the frame end,
/// and goes on in compiled code at the next turn, and the next frame.
///
/// A snapshot holds the machines alone, so a restore ends the runs
/// ([`Turns::end`]) and the next turn starts each afresh from `cpu.pc`.
/// That computes what the run that went on did only if a function resumed
/// past its entry runs as compiled code would tick it:
/// [`runtime::dispatch::Nest::timed_resumes`], which the turns must set.
pub trait Turns<P: Platform> {
    /// Run seat `seat`'s machine `m` on `code` until its run stops at the
    /// frame end, the hub's deadline or a fault, as `run_frame` does,
    /// counting into `stats`. A frame ended (V-blank) is presented, as
    /// `run_frame` presents it.
    fn turn(&mut self, seat: usize, m: &mut Machine<P>, code: &'static Code<P>, stats: &mut Stats);

    /// The machines were replaced (a restore or a load): end every run
    /// in progress without running it on.
    fn end(&mut self);
}

/// Linked GBAs running one ROM, or (single-pak, [`Gba::single_pak`]) seat
/// 0 running it and the others with no cartridge, booting what it sends.
pub struct Gba<P: Seat = Pad> {
    /// How recompiled consoles take their turns, if not by unwinding.
    /// Before `machines`, which the runs it holds point into: dropped
    /// first.
    turns: Option<Box<dyn Turns<P>>>,
    machines: Vec<Box<Machine<P>>>,
    link: Link,
    /// The recompiled code the machines run, if any; the interpreter
    /// without.
    code: Option<&'static Code<P>>,
    /// The dispatcher's counts, which nothing reads: not console state.
    stats: Box<Stats>,
    /// Cycles every seat ran, every run's (rollback's again too): with
    /// [`Turns`], the compiled share is what the rest leaves
    /// ([`Gba::stats`]).
    ran: u64,
    /// The session's clock at its start; `None` leaves each cartridge's.
    clock: Option<i64>,
    /// Frames run.
    frames: u64,
    build: [u8; 32],
    /// Which seats' cartridge saves changed since the front end last
    /// asked ([`Gba::take_changed`]). Not console state: each frame moves
    /// the saves' change flags here and clears them, so a frame run again
    /// by rollback computes what it did the first time.
    changed: [bool; runtime::link::MAX_PLAYERS],
    /// The cable unplugged ([`Gba::unplug`]): the one seat still running,
    /// on a cable of its own.
    alone: Option<usize>,
    /// The one seat whose picture is drawn ([`Gba::show_only`]), or all.
    shown: Option<usize>,
}

/// Every seat's consoles and the cable, in memory ([`Snapshot`]); not
/// the pictures, so restoring one leaves the last frame shown.
#[derive(Default)]
pub struct Snap {
    payloads: Vec<Vec<u8>>,
    /// Each machine's `SIOCNT` bits the cable owns: the hub's, outside the
    /// payload.
    owned: Vec<u16>,
    link: Option<Link>,
    frames: u64,
}

impl Gba {
    /// One machine a seat (1 to 4) running `rom` from reset, seat `i` with
    /// the cartridge save `saves[i]` if given. A peer gives its own seat's
    /// save; the others' states arrive at the start. The interpreter
    /// runs exact.
    pub fn new(rom: &'static [u8], saves: &[Option<&[u8]>]) -> Gba {
        Gba::with_code(rom, saves, None, Timing::Exact)
    }

    /// Single-pak link play: seat 0 running `rom` from reset with the
    /// cartridge save `save` if given, and `seats - 1` (1 to 3) GBAs with
    /// no cartridge ([`runtime::bios::multiboot::boot_cartless`]), whose BIOS waits for the
    /// program seat 0's game sends with `MultiBoot` and boots it. In the
    /// interpreter, counting cycles by `timing`.
    pub fn single_pak(
        rom: &'static [u8],
        seats: usize,
        save: Option<&[u8]>,
        timing: Timing,
    ) -> Gba {
        assert!(
            (2..=runtime::link::MAX_PLAYERS).contains(&seats),
            "2 to 4 seats"
        );
        let mut saves = vec![None; seats];
        saves[0] = save;
        let cartless = (1u8 << seats) - 2;
        Gba::build(rom, &saves, None, timing, cartless)
    }
}

impl<P: Seat> Gba<P> {
    /// [`Gba::new`], the machines running `code` (recompiled from `rom`)
    /// where it has a block and the interpreter elsewhere, as a recompiled
    /// game's window does; only the interpreter with `None`. The
    /// interpreter counts cycles by `timing` ([`Machine::timing`]), which
    /// the build ID holds, as it holds the code's own.
    pub fn with_code(
        rom: &'static [u8],
        saves: &[Option<&[u8]>],
        code: Option<&'static Code<P>>,
        timing: Timing,
    ) -> Gba<P> {
        Gba::build(rom, saves, code, timing, 0)
    }

    /// [`Gba::with_code`], the seats whose bits are set in `cartless` with
    /// no cartridge ([`Gba::single_pak`]).
    fn build(
        rom: &'static [u8],
        saves: &[Option<&[u8]>],
        code: Option<&'static Code<P>>,
        timing: Timing,
        cartless: u8,
    ) -> Gba<P> {
        let mut machines: Vec<Box<Machine<P>>> = saves
            .iter()
            .enumerate()
            .map(|(i, save)| {
                let none = cartless & 1 << i != 0;
                let mut m = Box::new(Machine::new(if none { &[] } else { rom }, P::seat()));
                if none {
                    runtime::bios::multiboot::boot_cartless(&mut m);
                }
                m.set_timing(timing);
                m.spin_list = code.map_or(&[][..], |c| c.spins);
                if let Some(s) = save {
                    m.bus.save.data[..s.len()].copy_from_slice(s);
                }
                m
            })
            .collect();
        let link = Link::new(&mut refs(&mut machines));
        let mut build = [0; 32];
        match code {
            None => build[..16].copy_from_slice(b"gbaa gba interp\0"),
            Some(c) => {
                build[..8].copy_from_slice(b"gbaa rc\0");
                build[8..16].copy_from_slice(&code_id(c).to_le_bytes());
            }
        }
        build[16..20].copy_from_slice(&snap::VERSION.to_le_bytes());
        build[20..24].copy_from_slice(&state::crc32(rom).to_le_bytes());
        build[24..28].copy_from_slice(&(rom.len() as u32).to_le_bytes());
        build[28] = saves.len() as u8;
        build[29] = timing as u8;
        build[30] = cartless;
        Gba {
            turns: None,
            machines,
            link,
            code,
            stats: Box::default(),
            ran: 0,
            clock: None,
            frames: 0,
            build,
            changed: [false; runtime::link::MAX_PLAYERS],
            alone: None,
            shown: None,
        }
    }

    /// Draw only seat `seat`'s picture (`None`: every seat's), the one
    /// this peer shows: the others' PPUs skip their lines, as in frames
    /// rollback runs again. The picture is outside snapshots and hashes,
    /// so this changes nothing any console computes, only the time a
    /// frame takes (multiplayer/rollback-feel-sim).
    pub fn show_only(&mut self, seat: Option<usize>) {
        self.shown = seat;
        self.quiet(false);
    }

    /// Pull the cable out of every console but seat `keep`'s, which plays
    /// on alone from here, each [`Console::step`] running it (and only it)
    /// a frame: the others' players have left. It sees what a GBA with no
    /// cable in sees (SI high, SD low, a transfer never starting); one in
    /// flight never completes, and the game's own link error follows. Not
    /// for a session's consoles, which every peer must run alike.
    pub fn unplug(&mut self, keep: usize) {
        assert!(keep < self.machines.len(), "seat {keep}");
        self.alone = Some(keep);
        self.link = Link::new(&mut [&mut *self.machines[keep]]);
    }

    /// The seat still running since [`Gba::unplug`], if it was.
    pub fn alone(&self) -> Option<usize> {
        self.alone
    }

    /// Whether seat `seat`'s cartridge save changed since the last call:
    /// what a front end's save writer looks at.
    pub fn take_changed(&mut self, seat: usize) -> bool {
        std::mem::take(&mut self.changed[seat])
    }

    pub fn machines(&self) -> &[Box<Machine<P>>] {
        &self.machines
    }

    pub fn machines_mut(&mut self) -> &mut [Box<Machine<P>>] {
        &mut self.machines
    }

    /// Whether the machines run recompiled code.
    pub fn recompiled(&self) -> bool {
        self.code.is_some()
    }

    /// Take the turns of recompiled consoles by `turns` ([`Turns`]).
    pub fn set_turns(&mut self, turns: Box<dyn Turns<P>>) {
        self.turns = Some(turns);
    }

    /// The dispatcher's counts, every seat's and every run's (rollback's
    /// again too) together. With [`Turns`], compiled code suspended
    /// mid-run, or ended by a restore, never returned to the dispatcher to
    /// count its cycles, so the compiled ones are every cycle run but the
    /// interpreter's and the halts'.
    pub fn stats(&self) -> Stats {
        let mut s = (*self.stats).clone();
        if self.turns.is_some() {
            s.compiled_cycles = self
                .ran
                .saturating_sub(s.interpreted_cycles + s.halted_cycles);
        }
        s
    }

    pub fn link(&self) -> &Link {
        &self.link
    }
}

/// What names a recompiled build among others of one ROM: a hash of its
/// shape (every key, overlay guard, interpreted block start and folded ROM
/// range). Two peers whose code differs in any of these would run apart.
fn code_id<P: Platform>(c: &Code<P>) -> u64 {
    let mut h = Fnv::default();
    h.bytes(&c.rom_hash.to_le_bytes());
    h.bytes(&[c.timing as u8]);
    let mut n = |v: u64| h.bytes(&v.to_le_bytes());
    n(c.blocks.len() as u64);
    for (k, _) in c.blocks {
        n(*k as u64);
    }
    n(c.ram_blocks.len() as u64);
    for &(k, _, first, versions) in c.ram_blocks {
        n((k as u64) << 32 | (first as u64) << 16 | versions as u64);
    }
    n(c.overlays.len() as u64);
    for o in c.overlays {
        for &(a, l, at) in o.ranges {
            n((a as u64) << 32 | l as u64);
            n(at as u64);
        }
        n(o.image.map_or(u64::MAX, |i| i.len() as u64));
    }
    n(c.inner.len() as u64);
    for &k in c.inner {
        n(k as u64);
    }
    n(c.folded.len() as u64);
    for &(s, e) in c.folded {
        n((s as u64) << 32 | e as u64);
    }
    h.0
}

fn refs<P: Platform>(ms: &mut [Box<Machine<P>>]) -> Vec<&mut Machine<P>> {
    ms.iter_mut().map(|m| &mut **m).collect()
}

impl<P: Seat> Console for Gba<P> {
    type Input = u16;

    fn seats(&self) -> usize {
        self.machines.len()
    }

    fn step(&mut self, inputs: &[u16]) -> Result<(), String> {
        let clock = self
            .clock
            .map(|c| c + (self.frames * FRAME_CYCLES / CYCLES_PER_SECOND) as i64);
        for (m, &k) in self.machines.iter_mut().zip(inputs) {
            m.platform.pad_mut().keys = k & key::ALL;
            if let Some(c) = clock {
                m.bus.gpio.clock = c;
            }
        }
        // Recompiled, each turn of the cable's runs to the hub's deadline
        // and unwinds to the dispatcher, or switches out on its guest
        // stack ([`Turns`]). Either way the machine holds the whole state
        // a snapshot needs.
        let (code, stats, ran) = (self.code, &mut *self.stats, &mut self.ran);
        let mut turns = self.turns.as_deref_mut();
        let mut run = |i: usize, m: &mut Machine<P>| {
            let t = m.now();
            match (code, &mut turns) {
                (Some(c), Some(turns)) => turns.turn(i, m, c, stats),
                (Some(c), None) => runtime::dispatch::run_frame(m, c, stats),
                (None, _) => m.run_frame(),
            }
            *ran += m.now() - t;
        };
        match self.alone {
            Some(k) => self
                .link
                .run(&mut [&mut *self.machines[k]], FRAME_CYCLES, |_, m| {
                    run(k, m)
                }),
            None => self
                .link
                .run(&mut refs(&mut self.machines), FRAME_CYCLES, run),
        }
        self.frames += 1;
        for (m, c) in self.machines.iter_mut().zip(&mut self.changed) {
            *c |= std::mem::take(&mut m.bus.save.dirty);
        }
        for (i, m) in self.machines.iter().enumerate() {
            if let Some(f) = &m.fault {
                return Err(format!("seat {i}, frame {}: {f}", self.frames));
            }
        }
        Ok(())
    }

    fn save(&mut self, seat: usize, out: &mut Vec<u8>) {
        self.machines[seat].save_state(&mut |b| out.extend_from_slice(b));
    }

    fn load(&mut self, states: &[Vec<u8>]) -> Result<(), String> {
        if let Some(t) = &mut self.turns {
            t.end();
        }
        for (i, (m, s)) in self.machines.iter_mut().zip(states).enumerate() {
            m.load_state(s)
                .map_err(|e| format!("seat {i}'s state: {e}"))?;
        }
        self.link = Link::new(&mut refs(&mut self.machines));
        Ok(())
    }

    fn set_clock(&mut self, unix: i64) {
        self.clock = Some(unix);
    }

    fn hash(&mut self) -> u64 {
        let mut h = Fnv::default();
        for m in &mut self.machines {
            m.save_unseen(&mut |b| h.bytes(b));
        }
        h.bytes(&self.link.now().to_le_bytes());
        h.bytes(&self.link.transfers.to_le_bytes());
        h.0
    }

    fn build_id(&self) -> [u8; 32] {
        self.build
    }
}

impl<P: Seat> Snapshot for Gba<P> {
    type Snap = Snap;

    fn snapshot(&mut self, into: &mut Snap) {
        into.payloads.resize_with(self.machines.len(), Vec::new);
        for (m, p) in self.machines.iter_mut().zip(&mut into.payloads) {
            p.clear();
            m.save_unseen(&mut |b| p.extend_from_slice(b));
        }
        into.owned.clear();
        into.owned
            .extend(self.machines.iter().map(|m| m.bus.io.sio_owned));
        into.link = Some(self.link.clone());
        into.frames = self.frames;
    }

    fn restore(&mut self, from: &Snap) -> Result<(), String> {
        let link = from.link.as_ref().ok_or("an empty snapshot")?;
        if let Some(t) = &mut self.turns {
            t.end();
        }
        for (i, (m, p)) in self.machines.iter_mut().zip(&from.payloads).enumerate() {
            m.load_unseen(p)
                .map_err(|e| format!("seat {i}'s snapshot: {e}"))?;
        }
        for (m, &o) in self.machines.iter_mut().zip(&from.owned) {
            m.bus.io.sio_owned = o;
        }
        self.link = link.clone();
        self.frames = from.frames;
        Ok(())
    }

    fn snap_hash(&self, s: &Snap) -> u64 {
        let mut h = Fnv::default();
        for p in &s.payloads {
            h.bytes(p);
        }
        let link = s.link.as_ref().unwrap_or(&self.link);
        h.bytes(&link.now().to_le_bytes());
        h.bytes(&link.transfers.to_le_bytes());
        h.0
    }

    fn quiet(&mut self, on: bool) {
        let shown = self.shown;
        for (i, m) in self.machines.iter_mut().enumerate() {
            m.platform.pad_mut().quiet = on;
            m.ppu.draw = !on && shown.is_none_or(|s| s == i);
        }
    }
}
