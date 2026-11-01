//! Rollback, Slippi's kind (`docs/research/multiplayer.md`, "Rollback"):
//! every peer runs every console and only inputs cross the network, and a
//! peer never waits for the others' inputs. It runs each
//! frame at once on the inputs it has, guessing a seat's missing input as
//! that seat's last one, and keeps a snapshot of the consoles before every
//! frame it guessed. When a seat's real input arrives and differs from the
//! guess, it loads the snapshot before that frame and runs again to where
//! it was, quietly (no sound, no frames shown), so the other player is
//! seen to jump rather than the game to stop.
//!
//! - **Input delay D** (0 by default, and june's rule: never the fix for
//!   a bad connection): this seat's input is for frame N + D, so a round
//!   trip under 2 D frames needs no rollback. Frames 0 to D - 1 run on no
//!   input.
//! - **The window W**: a peer runs at most W frames past the last frame
//!   it has every seat's input for; beyond that it waits (the other peer
//!   gone, near enough). It bounds the snapshots kept (one a frame, all
//!   consoles: ~1.3 MB for two GBAs) and the frames one rollback may run
//!   again, nothing else.
//! - **Time sync**: if this peer runs ahead of another (it sees fewer of
//!   their frames than they see of its), it holds back one frame now and
//!   then ([`Status::Skipped`]), so both guess about as far and neither
//!   rolls back much more than the other.
//! - **Inputs** travel unreliably: each message repeats every input from
//!   the first one some other seat still lacks, as each seat's
//!   [`Msg::Input::have`] tells, so a lost or late message costs nothing
//!   while the next gets through.
//! - **Hashes**: every [`HASH_EVERY`]th frame's consoles are hashed once
//!   every seat's inputs before it are in (from that frame's snapshot) and
//!   sent; differing hashes are recorded as desyncs. No resync yet.
//!
//! The start (every seat's state, seat 0's settings) is
//! [`crate::session`]'s; the messages are [`Msg`].

use std::collections::{BTreeMap, VecDeque};

use crate::msg::Msg;
use crate::session::HASH_EVERY;
use crate::{Config, Delivery, Error, Input, Netplay, Snapshot, Status, To, Transport};

/// The default window: 5 seconds ahead (june, 2026-10-27: don't stop the
/// game over a late input). Two GBAs' snapshots for it are ~390 MB.
pub const WINDOW: u32 = 300;

/// Counts over a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Times a guess turned out wrong and the consoles went back.
    pub rollbacks: u64,
    /// Frames run again for them.
    pub rerun: u64,
    /// The most frames one rollback went back.
    pub deepest: u32,
    /// Turns spent waiting at the window's edge.
    pub stalls: u64,
    /// Frames held back for time sync.
    pub skips: u64,
}

pub struct Rollback<C: Snapshot, T: Transport> {
    console: C,
    net: T,
    seat: usize,
    seats: usize,
    build: [u8; 32],
    config: Option<Config>,
    window: u32,
    started: bool,
    /// Frames run.
    frame: u32,
    /// Every seat's inputs from frame 0, as far as they've arrived in order.
    inputs: Vec<Vec<C::Input>>,
    /// How many of this seat's inputs each seat holds.
    acked: Vec<u32>,
    /// The first frame that may yet be run again: `snaps[i]` is the
    /// consoles before frame `base + i` and `used[i]` the inputs it ran
    /// on, up to the frame about to run.
    base: u32,
    snaps: VecDeque<C::Snap>,
    used: VecDeque<Vec<C::Input>>,
    spare: Vec<C::Snap>,
    /// Each seat's inputs, from 0, already compared with what ran.
    checked: Vec<usize>,
    states: BTreeMap<u32, Vec<Option<Vec<u8>>>>,
    hashes: BTreeMap<u32, Vec<Option<u64>>>,
    /// The next frame to hash.
    next_hash: u32,
    log: Vec<(u32, u64)>,
    desyncs: Vec<u32>,
    /// What each seat last said: how many of its own inputs and of this
    /// seat's it held.
    said: Vec<(u32, u32)>,
    /// How far ahead of each seat this peer runs, smoothed, in frames.
    lead: Vec<f32>,
    /// Frames until time sync may hold one back again.
    cooldown: u32,
    pub stats: Stats,
}

/// Frames between two held back.
const SKIP_EVERY: u32 = 12;

impl<C: Snapshot, T: Transport> Rollback<C, T> {
    /// Join as `seat`, `console` holding this seat's own state (the others'
    /// arrive). Seat 0 gives the session's settings in `host`; `window` is W.
    pub fn new(
        console: C,
        net: T,
        seat: usize,
        host: Option<Config>,
        window: u32,
    ) -> Rollback<C, T> {
        let seats = console.seats();
        assert!(seat < seats, "seat {seat} of {seats}");
        assert_eq!(seat == 0, host.is_some(), "seat 0, and only it, hosts");
        assert!(window > 0, "a window of at least one frame");
        let build = console.build_id();
        let mut s = Rollback {
            console,
            net,
            seat,
            seats,
            build,
            config: host,
            window,
            started: false,
            frame: 0,
            inputs: vec![Vec::new(); seats],
            acked: vec![0; seats],
            base: 0,
            snaps: VecDeque::new(),
            used: VecDeque::new(),
            spare: Vec::new(),
            checked: vec![0; seats],
            states: BTreeMap::new(),
            hashes: BTreeMap::new(),
            next_hash: 0,
            log: Vec::new(),
            desyncs: Vec::new(),
            said: vec![(0, 0); seats],
            lead: vec![0.0; seats],
            cooldown: 0,
            stats: Stats::default(),
        };
        if let Some(c) = host {
            s.send(
                To::Everyone,
                Delivery::Reliable,
                Msg::Start {
                    delay: c.delay,
                    clock: c.clock,
                },
            );
        }
        let mut data = Vec::new();
        s.console.save(seat, &mut data);
        s.send(
            To::Everyone,
            Delivery::Reliable,
            Msg::State {
                frame: 0,
                build,
                data: data.clone(),
            },
        );
        s.states.entry(0).or_insert_with(|| vec![None; seats])[seat] = Some(data);
        s
    }

    /// The transport, to say goodbye on, say ([`crate::udp::Udp::leave`]).
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.net
    }

    pub fn config(&self) -> Option<Config> {
        self.config
    }

    pub fn started(&self) -> bool {
        self.started
    }

    /// How many frames this peer runs ahead of the furthest-behind other
    /// seat, as time sync smooths it (behind, if negative).
    pub fn lead(&self) -> f32 {
        (0..self.seats)
            .filter(|&s| s != self.seat)
            .map(|s| self.lead[s])
            .reduce(f32::max)
            .unwrap_or(0.0)
    }

    /// The frames whose hashes differed between peers.
    pub fn desyncs(&self) -> &[u32] {
        &self.desyncs
    }

    /// The last frame every seat's input is in for, plus one.
    pub fn confirmed(&self) -> u32 {
        self.inputs
            .iter()
            .map(|i| i.len() as u32)
            .min()
            .unwrap_or(0)
            .min(self.frame)
    }

    /// Whether nothing more can change here: every seat's input is in
    /// for every frame run, and every other seat holds all of this one's.
    pub fn settled(&self) -> bool {
        let mine = self.inputs[self.seat].len() as u32;
        self.confirmed() == self.frame
            && (0..self.seats)
                .filter(|&s| s != self.seat)
                .all(|s| self.acked[s] >= mine)
    }

    fn delay(&self) -> usize {
        self.config.map_or(0, |c| c.delay as usize)
    }

    fn send(&mut self, to: To, how: Delivery, m: Msg) {
        self.net.send(to, how, m.encode());
    }

    fn take(&mut self, from: usize, msg: Msg) -> Result<(), Error> {
        let seats = self.seats;
        match msg {
            Msg::State { frame, build, data } => {
                if build != self.build {
                    return Err(Error::Build { seat: from });
                }
                self.states
                    .entry(frame)
                    .or_insert_with(|| vec![None; seats])[from] = Some(data);
            }
            Msg::Start { delay, clock } => {
                if from != 0 {
                    return Err(Error::Bad { seat: from });
                }
                self.config = Some(Config { delay, clock });
            }
            Msg::Input {
                first,
                inputs,
                have,
            } => {
                let size = <C::Input as Input>::SIZE;
                if inputs.len() % size != 0 || have.len() != seats {
                    return Err(Error::Bad { seat: from });
                }
                let theirs = &mut self.inputs[from];
                for (i, b) in inputs.chunks_exact(size).enumerate() {
                    if first as usize + i == theirs.len() {
                        theirs.push(C::Input::read(b));
                    }
                }
                self.acked[from] = self.acked[from].max(have[self.seat]);
                // The newest word from them: datagrams may come out of order.
                if have[from] >= self.said[from].0 {
                    self.said[from] = (have[from], have[self.seat]);
                }
            }
            Msg::Hash { frame, hash } => {
                self.hashes
                    .entry(frame)
                    .or_insert_with(|| vec![None; seats])[from] = Some(hash);
            }
        }
        Ok(())
    }

    fn start(&mut self) -> Result<bool, Error> {
        let Some(config) = self.config else {
            return Ok(false);
        };
        let ready = self
            .states
            .get(&0)
            .is_some_and(|s| s.iter().all(Option::is_some));
        if !ready {
            return Ok(false);
        }
        let states: Vec<Vec<u8>> = self
            .states
            .remove(&0)
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect();
        self.console.load(&states).map_err(Error::Console)?;
        self.console.set_clock(config.clock);
        for i in &mut self.inputs {
            let early = std::mem::take(i);
            i.resize(config.delay as usize, C::Input::default());
            i.extend(early);
        }
        self.acked.fill(config.delay as u32);
        self.started = true;
        Ok(true)
    }

    /// Frame `f`'s inputs as best known: a seat's own, or its last.
    fn guess(&self, f: u32) -> Vec<C::Input> {
        self.inputs
            .iter()
            .map(|i| i.get(f as usize).or(i.last()).copied().unwrap_or_default())
            .collect()
    }

    /// If an input that has arrived differs from what its frame ran on,
    /// go back to that frame and run again to here.
    fn correct(&mut self) -> Result<(), Error> {
        let mut wrong: Option<u32> = None;
        for s in 0..self.seats {
            let have = self.inputs[s].len().min(self.frame as usize);
            let from = self.checked[s].max(self.base as usize);
            for f in from..have {
                if self.inputs[s][f] != self.used[f - self.base as usize][s] {
                    wrong = Some(wrong.map_or(f as u32, |w| w.min(f as u32)));
                    break;
                }
            }
            self.checked[s] = have;
        }
        let Some(f) = wrong else {
            return Ok(());
        };
        let depth = self.frame - f;
        self.stats.rollbacks += 1;
        self.stats.rerun += depth as u64;
        self.stats.deepest = self.stats.deepest.max(depth);
        let i = (f - self.base) as usize;
        self.console
            .restore(&self.snaps[i])
            .map_err(Error::Console)?;
        self.console.quiet(true);
        let mut r = Ok(());
        for g in f..self.frame {
            let i = (g - self.base) as usize;
            if g > f {
                self.console.snapshot(&mut self.snaps[i]);
            }
            self.used[i] = self.guess(g);
            r = self.console.step(&self.used[i]);
            if r.is_err() {
                break;
            }
        }
        self.console.quiet(false);
        r.map_err(Error::Console)
    }

    /// Hash and compare what's now certain, and let go of the snapshots no
    /// rollback can need.
    fn settle(&mut self) -> Result<(), Error> {
        let c = self.confirmed();
        while self.next_hash <= c {
            let f = self.next_hash;
            let h = if f == self.frame {
                self.console.hash()
            } else {
                self.console
                    .snap_hash(&self.snaps[(f - self.base) as usize])
            };
            self.log.push((f, h));
            let seats = self.seats;
            self.hashes.entry(f).or_insert_with(|| vec![None; seats])[self.seat] = Some(h);
            self.send(
                To::Everyone,
                Delivery::Reliable,
                Msg::Hash { frame: f, hash: h },
            );
            self.next_hash += HASH_EVERY;
        }
        let done: Vec<u32> = self
            .hashes
            .iter()
            .filter(|(_, hs)| hs.iter().all(Option::is_some))
            .map(|(&f, _)| f)
            .collect();
        for f in done {
            let hs = self.hashes.remove(&f).unwrap();
            if hs.iter().any(|h| *h != hs[0]) {
                self.desyncs.push(f);
            }
        }
        while self.base < c {
            self.spare.extend(self.snaps.pop_front());
            self.used.pop_front();
            self.base += 1;
        }
        Ok(())
    }

    /// Whether to hold this frame back, this peer running ahead.
    fn hold_back(&mut self) -> bool {
        let me = self.inputs[self.seat].len() as f32;
        let mut ahead = 0f32;
        for s in (0..self.seats).filter(|&s| s != self.seat) {
            // Each side's lead over the other, as each sees it; if both
            // run alike, both are the one-way trip.
            let mine = me - self.inputs[s].len() as f32;
            let theirs = self.said[s].0 as f32 - self.said[s].1 as f32;
            let l = &mut self.lead[s];
            *l += ((mine - theirs) / 2.0 - *l) * 0.1;
            ahead = ahead.max(*l);
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }
        if ahead >= 1.0 {
            self.cooldown = SKIP_EVERY;
            for l in &mut self.lead {
                *l -= 1.0;
            }
            return true;
        }
        false
    }

    fn send_inputs(&mut self) {
        let from = (0..self.seats)
            .filter(|&s| s != self.seat)
            .map(|s| self.acked[s])
            .min()
            .unwrap_or(0);
        let mut inputs = Vec::new();
        for i in self.inputs[self.seat].iter().skip(from as usize) {
            i.write(&mut inputs);
        }
        let have = self.inputs.iter().map(|i| i.len() as u32).collect();
        let m = Msg::Input {
            first: from,
            inputs,
            have,
        };
        self.send(To::Everyone, Delivery::Unreliable, m);
    }
}

impl<C: Snapshot, T: Transport> Netplay for Rollback<C, T> {
    type Console = C;
    type Net = T;

    fn pump(&mut self) -> Result<(), Error> {
        while let Some((from, payload)) = self.net.recv() {
            if from >= self.seats || from == self.seat {
                return Err(Error::Bad { seat: from });
            }
            let msg = Msg::decode(&payload).ok_or(Error::Bad { seat: from })?;
            self.take(from, msg)?;
        }
        if self.started {
            self.send_inputs();
            // A peer that has stopped running still settles what arrives.
            self.correct()?;
            self.settle()?;
        }
        Ok(())
    }

    fn advance(&mut self, local: C::Input) -> Result<Status, Error> {
        self.pump()?;
        if !self.started && !self.start()? {
            return Ok(Status::Starting);
        }
        let ahead = self.frame as usize + self.delay();
        if self.inputs[self.seat].len() == ahead {
            self.inputs[self.seat].push(local);
            self.send_inputs();
        }
        self.correct()?;
        self.settle()?;
        if self.frame - self.confirmed() >= self.window {
            self.stats.stalls += 1;
            return Ok(Status::Waiting);
        }
        if self.hold_back() {
            self.stats.skips += 1;
            return Ok(Status::Skipped);
        }
        let f = self.frame;
        let now = self.guess(f);
        let sure = self.inputs.iter().all(|i| i.len() > f as usize);
        if sure && self.snaps.is_empty() {
            // Never run again: no snapshot.
            self.base = f + 1;
        } else {
            let mut snap = self.spare.pop().unwrap_or_default();
            self.console.snapshot(&mut snap);
            self.snaps.push_back(snap);
            self.used.push_back(now.clone());
        }
        self.console.step(&now).map_err(Error::Console)?;
        self.frame += 1;
        Ok(Status::Stepped)
    }

    fn frame(&self) -> u32 {
        self.frame
    }

    fn seat(&self) -> usize {
        self.seat
    }

    fn console(&self) -> &C {
        &self.console
    }

    fn console_mut(&mut self) -> &mut C {
        &mut self.console
    }

    fn transport(&self) -> &T {
        &self.net
    }

    fn hashes(&self) -> &[(u32, u64)] {
        &self.log
    }

    fn desyncs(&self) -> &[u32] {
        &self.desyncs
    }

    fn lead(&self) -> Option<f32> {
        Some(Rollback::lead(self))
    }

    fn summary(&self) -> String {
        let st = self.stats;
        format!(
            "{} rollbacks, {} frames run again, deepest {}, {} stalls, {} held back",
            st.rollbacks, st.rerun, st.deepest, st.stalls, st.skips
        )
    }
}
