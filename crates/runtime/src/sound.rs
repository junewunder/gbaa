//! Direct sound: FIFOs A and B, popped by timer overflows and refilled by
//! FIFO-timed DMA, and the sample stream handed to [`Platform::audio`].
//! Design in `docs/sound-design.md` (#52). The PSG's four channels are
//! in [`psg`], mixed into each record. With the master on and no timer
//! clocking a FIFO, the stream's samples come from the PSG's grid instead.
//!
//! A timer that clocks a FIFO has its overflows walked one at a time, in
//! clock order, by [`run_timers`], which every caller of [`timer::run`]
//! goes through: each overflow pops, refills, cascades and raises its IRQ
//! before the next is looked at. Stores to the FIFOs push at the moment of
//! the store ([`io_written`]).
//!
//! [`Platform::audio`]: crate::Platform::audio

use crate::bus::Bus;
use crate::dma;
use crate::io::{Io, event, reg};
use crate::timer::{self, ctl};

pub mod psg;
use psg::{PSG_CHUNK, Psg};

/// A FIFO's capacity in bytes.
pub const FIFO_SIZE: usize = 32;
/// A pop that leaves this many bytes or fewer asks the FIFO's DMA for more.
pub const REFILL_AT: usize = 16;
/// Records buffered between hand-offs to the platform.
pub const CHUNK: usize = 2048;
/// Restart and swap marks kept per chunk, each (agb makes two restarts and
/// one swap a frame). Marks past these are not recorded.
pub const MAX_MARKS: usize = 64;
/// `FIFO_A` and `FIFO_B` as bus addresses, what a feeding DMA points at.
pub const FIFO_ADDR: [u32; 2] = [0x0400_0000 + reg::FIFO_A, 0x0400_0000 + reg::FIFO_B];

/// One sound FIFO: a 32-byte ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fifo {
    buf: [u8; FIFO_SIZE],
    read: u8,
    len: u8,
}

impl Fifo {
    /// Bytes queued.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Queue a byte. A full FIFO drops it (and returns false).
    pub fn push(&mut self, b: u8) -> bool {
        if self.len() == FIFO_SIZE {
            return false;
        }
        self.buf[(self.read as usize + self.len()) % FIFO_SIZE] = b;
        self.len += 1;
        true
    }

    /// Take the oldest byte, if any.
    pub fn pop(&mut self) -> Option<i8> {
        if self.is_empty() {
            return None;
        }
        let b = self.buf[self.read as usize];
        self.read = ((self.read as usize + 1) % FIFO_SIZE) as u8;
        self.len -= 1;
        Some(b as i8)
    }

    /// Empty it (`SOUNDCNT_H` bit 11 or 15).
    pub fn reset(&mut self) {
        self.read = 0;
        self.len = 0;
    }
}

/// How promptly the guest re-latched its FIFO DMA after the timer IRQ
/// that asks it to (agb's buffer swap): the margin `docs/sound-design.md`
/// worries about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwapStats {
    /// Timer IRQs raised by a step that popped a FIFO, or by a cascade
    /// above one.
    pub swaps: u64,
    /// FIFO DMA channels (re)enabled within a swap's window.
    pub relatches: u64,
    /// The most cycles from a swap to a re-latch.
    pub max_cycles: u64,
    /// The most samples popped between a swap and a re-latch.
    pub max_pops: u32,
    /// The most refills between a swap and its FIFO's re-latch: each one
    /// read from the old buffer. Nonzero is the race.
    pub max_refills: u32,
    /// The pending swap: when it happened, and per FIFO whether it still
    /// waits for its re-latch, the pops and the refills since.
    swap_at: u64,
    waiting: [bool; 2],
    pops: [u32; 2],
    refills: [u32; 2],
}

/// The sound state the bus owns.
#[derive(Clone, Debug)]
pub struct Sound {
    pub fifo: [Fifo; 2],
    /// Each FIFO's current output sample: its last pop.
    pub out: [i8; 2],
    /// Records since the last hand-off: `(A, B)` per sample.
    pub records: [(i8, i8); CHUNK],
    /// Records since the last hand-off.
    pub len: usize,
    /// The stream mapped to `(left, right)`: each record, or each PSG grid
    /// sample while no timer clocks a FIFO.
    pub samples: [(i16, i16); PSG_CHUNK],
    pub samples_len: usize,
    /// `(index into samples, cycles per sample)` where the rate changes.
    pub spans: [(u16, u32); MAX_MARKS],
    pub spans_len: usize,
    /// Indices into `records` where a FIFO DMA was (re)enabled.
    pub restarts: [u16; MAX_MARKS],
    pub restarts_len: usize,
    /// Indices into `records` of the samples whose overflow raised a timer
    /// IRQ in the FIFO clock's cascade chain (agb's buffer swaps).
    pub swaps: [u16; MAX_MARKS],
    pub swaps_len: usize,
    /// Cycles per sample of the stream, from the last one made.
    pub period: u32,
    /// Records or samples dropped because a buffer was full, since the
    /// last hand-off.
    pub dropped: u32,
    pub swap: SwapStats,
    pub psg: Psg,
    /// `SOUNDBIAS` before the store [`io_written`] follows.
    bias_before: u16,
}

impl Default for Sound {
    fn default() -> Sound {
        Sound {
            fifo: [Fifo::default(); 2],
            out: [0; 2],
            records: [(0, 0); CHUNK],
            len: 0,
            samples: [(0, 0); PSG_CHUNK],
            samples_len: 0,
            spans: [(0, 0); MAX_MARKS],
            spans_len: 0,
            restarts: [0; MAX_MARKS],
            restarts_len: 0,
            swaps: [0; MAX_MARKS],
            swaps_len: 0,
            period: 0,
            dropped: 0,
            swap: SwapStats::default(),
            psg: Psg::default(),
            bias_before: 0,
        }
    }
}

/// What the platform gets: the records since the last hand-off.
#[derive(Clone, Copy, Debug)]
pub struct AudioChunk<'a> {
    /// Raw FIFO output, one `(A, B)` pair per record.
    pub fifo: &'a [(i8, i8)],
    /// The stream, mapped to stereo `(left, right)`: the records, or, while
    /// the master is on and no timer clocks a FIFO, the PSG's grid samples
    /// (with the FIFOs' last outputs).
    pub samples: &'a [(i16, i16)],
    /// Cycles per sample of the last sample made (the rate is `16777216 /
    /// period` Hz); 0 before the first.
    pub period: u32,
    /// Each run of `samples` at one rate: `(its first index, cycles per
    /// sample)`, from index 0 when there are any.
    pub spans: &'a [(u16, u32)],
    /// Indices into `fifo` where a FIFO DMA was (re)enabled: agb's
    /// re-latch after a buffer swap.
    pub restarts: &'a [u16],
    /// Indices into `fifo` of samples whose timer overflow raised an IRQ in
    /// the FIFO clock's cascade chain: agb's buffer swaps.
    pub swaps: &'a [u16],
    /// Samples lost since the last chunk because the buffer was full.
    pub dropped: u32,
    /// The PSG's share of the mix on mGBA's sample grid (`0x200 >>
    /// resolution` cycles), after the `SOUNDCNT_H` shift: what
    /// `--psg-dump` writes.
    pub psg: &'a [(i16, i16)],
    /// PSG samples lost since the last chunk.
    pub psg_dropped: u32,
}

impl Sound {
    /// The chunk to hand to the platform.
    pub fn chunk(&self) -> AudioChunk<'_> {
        AudioChunk {
            fifo: &self.records[..self.len],
            samples: &self.samples[..self.samples_len],
            period: self.period,
            spans: &self.spans[..self.spans_len],
            restarts: &self.restarts[..self.restarts_len],
            swaps: &self.swaps[..self.swaps_len],
            dropped: self.dropped,
            psg: &self.psg.out[..self.psg.len],
            psg_dropped: self.psg.dropped,
        }
    }

    /// Start a new chunk.
    pub fn clear(&mut self) {
        self.len = 0;
        self.samples_len = 0;
        self.spans_len = 0;
        self.restarts_len = 0;
        self.swaps_len = 0;
        self.dropped = 0;
        self.psg.clear();
    }

    /// Whether there's anything to hand off.
    pub fn pending(&self) -> bool {
        self.len != 0
            || self.samples_len != 0
            || self.restarts_len != 0
            || self.swaps_len != 0
            || self.dropped != 0
            || self.psg.len != 0
            || self.psg.dropped != 0
    }

    /// Record the FIFOs' outputs at `at`, mixed with the PSG.
    fn record(&mut self, io: &Io, at: u64) {
        if self.len == CHUNK {
            self.dropped += 1;
            return;
        }
        let (a, b) = (self.out[0], self.out[1]);
        self.records[self.len] = (a, b);
        self.len += 1;
        self.sample(mix(io, a, b, self.psg.level_at(io, at as i64)), self.period);
    }

    /// Add `s`, `period` cycles after the last sample, to the stream.
    fn sample(&mut self, s: (i16, i16), period: u32) {
        if self.samples_len == PSG_CHUNK {
            self.dropped += 1;
            return;
        }
        let rate_changed = self.spans_len == 0 || self.spans[self.spans_len - 1].1 != period;
        if rate_changed && self.spans_len < MAX_MARKS {
            self.spans[self.spans_len] = (self.samples_len as u16, period);
            self.spans_len += 1;
        }
        self.samples[self.samples_len] = s;
        self.samples_len += 1;
        self.period = period;
    }

    /// Run `f` on the PSG. If the master is on and no timer clocks a FIFO,
    /// each grid sample it computes becomes a sample of the stream, mixed
    /// with the FIFOs' last outputs.
    fn with_psg(&mut self, io: &mut Io, f: impl FnOnce(&mut Psg, &mut Io)) {
        let grid = io.raw16(reg::SOUNDCNT_X) & 0x80 != 0 && clocks(io) == 0;
        let from = self.psg.len;
        f(&mut self.psg, io);
        if !grid {
            return;
        }
        let period = psg::interval(io);
        for i in from..self.psg.len {
            let s = mix(io, self.out[0], self.out[1], self.psg.out[i]);
            self.sample(s, period);
        }
    }

    /// FIFO DMA channel feeding `fifo` was (re)enabled at `now`.
    pub fn restarted(&mut self, fifo: usize, now: u64) {
        if self.restarts_len < MAX_MARKS {
            self.restarts[self.restarts_len] = self.len as u16;
            self.restarts_len += 1;
        }
        let s = &mut self.swap;
        if s.waiting[fifo] {
            s.waiting[fifo] = false;
            s.relatches += 1;
            s.max_cycles = s.max_cycles.max(now.saturating_sub(s.swap_at));
            s.max_pops = s.max_pops.max(s.pops[fifo]);
            s.max_refills = s.max_refills.max(s.refills[fifo]);
        }
    }
}

/// Map one record to `(left, right)` with the registers as they are now,
/// adding `psg`, the PSG's share ([`Psg::level_at`]): GBATEK's 10-bit DAC,
/// centred and scaled to `i16`. The sum is mGBA's.
pub fn mix(io: &Io, a: i8, b: i8, psg: (i16, i16)) -> (i16, i16) {
    let h = io.raw16(reg::SOUNDCNT_H);
    let on = io.raw16(reg::SOUNDCNT_X) & 0x80 != 0;
    let bias = (io.raw16(reg::SOUNDBIAS) & 0x3FE) as i32;
    let level = |s: i8, full: u16| (s as i32) * if h & full != 0 { 4 } else { 2 };
    let (a, b) = (level(a, 1 << 2), level(b, 1 << 3));
    let side = |a_bit: u16, b_bit: u16, psg: i16| {
        let mut sum = bias + psg as i32;
        if on && h & a_bit != 0 {
            sum += a;
        }
        if on && h & b_bit != 0 {
            sum += b;
        }
        ((sum.clamp(0, 0x3FF) - 0x200) * 64) as i16
    };
    (side(1 << 9, 1 << 13, psg.0), side(1 << 8, 1 << 12, psg.1))
}

/// The timers whose overflows pop a FIFO, as a mask of `1 << n`: master
/// enable on, the FIFO routed to a speaker, and its timer select (mGBA's
/// condition).
pub fn clocks(io: &Io) -> u8 {
    if io.raw16(reg::SOUNDCNT_X) & 0x80 == 0 {
        return 0;
    }
    let h = io.raw16(reg::SOUNDCNT_H);
    let mut m = 0;
    if h & 0x0300 != 0 {
        m |= 1 << (h >> 10 & 1);
    }
    if h & 0x3000 != 0 {
        m |= 1 << (h >> 14 & 1);
    }
    m
}

/// The clocked timer at the bottom of timer `n`'s cascade chain, if the
/// chain is running.
fn chain_base(io: &Io, mut n: usize) -> Option<usize> {
    loop {
        let t = &io.timers[n];
        if t.ticking {
            return Some(n);
        }
        if n == 0 || t.control & ctl::ENABLE == 0 {
            return None;
        }
        n -= 1;
    }
}

/// Cycles between overflows of timer `n`, if it runs.
fn period(io: &Io, n: usize) -> Option<u32> {
    let base = chain_base(io, n)?;
    let mut p = 1u64 << io.timers[base].shift();
    for m in base..=n {
        p *= 0x1_0000 - io.raw16(reg::TM0CNT_L + 4 * m as u32) as u64;
    }
    Some(p.min(u32::MAX as u64) as u32)
}

/// Bring the timers up to `bus.io.now`, walking each overflow of a timer
/// that clocks a FIFO one at a time; [`timer::run`] when none does.
pub fn run_timers(bus: &mut Bus) {
    loop {
        let clocks = clocks(&bus.io);
        if clocks == 0 {
            break;
        }
        let mut next: Option<u64> = None;
        for n in 0..2 {
            if clocks & (1 << n) == 0 {
                continue;
            }
            if let Some(at) = chain_base(&bus.io, n).and_then(|b| bus.io.timers[b].next_overflow())
            {
                next = Some(next.map_or(at, |e| e.min(at)));
            }
        }
        let Some(at) = next.filter(|&at| at <= bus.io.now) else {
            break;
        };
        let over = timer::run_to(&mut bus.io, at);
        overflowed(bus, over, clocks, at);
    }
    timer::run(&mut bus.io);
    let now = bus.io.now as i64;
    advance_psg(bus, now);
}

/// Run the PSG's events due by `t`, making the stream's samples from its
/// grid if no timer clocks a FIFO.
pub fn advance_psg(bus: &mut Bus, t: i64) {
    bus.sound.with_psg(&mut bus.io, |p, io| p.advance(io, t));
}

/// The timers in `over` overflowed at cycle `at`: pop the FIFOs they
/// clock, refill them, and record the sample.
fn overflowed(bus: &mut Bus, over: u8, clocks: u8, at: u64) {
    let popping = over & clocks;
    if popping == 0 {
        return;
    }
    let h = bus.io.raw16(reg::SOUNDCNT_H);
    let mut popped: Option<usize> = None;
    for ch in 0..2 {
        let routed = h & (0x0300 << (4 * ch)) != 0;
        let timer = (h >> (10 + 4 * ch) & 1) as usize;
        if !routed || popping & (1 << timer) == 0 {
            continue;
        }
        popped = Some(popped.map_or(timer, |t| t.min(timer)));
        let s = &mut bus.sound;
        s.out[ch] = s.fifo[ch].pop().unwrap_or(0);
        if s.swap.waiting[ch] {
            s.swap.pops[ch] += 1;
        }
        if s.fifo[ch].len() <= REFILL_AT && dma::fifo_refill(bus, ch) && bus.sound.swap.waiting[ch]
        {
            bus.sound.swap.refills[ch] += 1;
        }
    }
    if let Some(timer) = popped
        && let Some(p) = period(&bus.io, timer)
    {
        bus.sound.period = p;
    }
    advance_psg(bus, at as i64);
    bus.sound.record(&bus.io, at);
    // A timer IRQ from this step, the popping timer's own or one cascaded
    // above it, asks for the next buffer: mark it, and time the re-latch.
    let Some(low) = popped else { return };
    let mut swap = false;
    for n in low..4 {
        let c = bus.io.timers[n].control;
        if n > low && c & (ctl::ENABLE | ctl::CASCADE) != ctl::ENABLE | ctl::CASCADE {
            break;
        }
        swap |= over & (1 << n) != 0 && c & ctl::IRQ != 0;
    }
    if swap {
        let s = &mut bus.sound;
        if s.swaps_len < MAX_MARKS && s.len > 0 {
            s.swaps[s.swaps_len] = (s.len - 1) as u16;
            s.swaps_len += 1;
        }
        let s = &mut s.swap;
        s.swaps += 1;
        s.swap_at = at;
        for ch in 0..2 {
            s.waiting[ch] = h & (0x0300 << (4 * ch)) != 0;
            s.pops[ch] = 0;
            s.refills[ch] = 0;
        }
    }
}

/// Before an I/O store of `bytes` at `o`: walk the overflows due before
/// it, if it can change what a FIFO does (a timer, the sound registers,
/// DMA1 or DMA2), and bring the PSG up to it if it's a sound register.
#[inline]
pub fn before_io_write(bus: &mut Bus, o: u32, bytes: u32) {
    let timers = (reg::TM0CNT_L..reg::TM0CNT_L + 16).contains(&o);
    let sound = (0x060..reg::FIFO_B + 4).contains(&o);
    let dma12 = (reg::DMA0SAD + 12..reg::DMA0SAD + 36).contains(&o);
    if timers || sound || dma12 {
        run_timers(bus);
    }
    if (0x060..reg::WAVE_RAM + 16).contains(&o) {
        let now = bus.io.now as i64;
        bus.sound.bias_before = bus.io.raw16(reg::SOUNDBIAS);
        bus.sound
            .with_psg(&mut bus.io, |p, io| p.before_write(io, o, bytes, now));
    }
}

/// After an I/O store of `v` (`w` wide) at `o`: push FIFO stores, low byte
/// first, and act on FIFO resets.
#[inline]
pub fn io_written(bus: &mut Bus, o: u32, bytes: u32, v: u32) {
    if (0x060..reg::WAVE_RAM + 16).contains(&o) {
        let (now, bias) = (bus.io.now as i64, bus.sound.bias_before);
        bus.sound.psg.written(&mut bus.io, o, bytes, v, now, bias);
    }
    if (reg::FIFO_A..reg::FIFO_B + 4).contains(&o) {
        let ch = ((o - reg::FIFO_A) / 4) as usize;
        for i in 0..bytes {
            bus.sound.fifo[ch].push((v >> (8 * i)) as u8);
        }
    }
    let resets = bus.io.events & (event::FIFO_A_RESET | event::FIFO_B_RESET);
    if resets != 0 {
        bus.io.events &= !resets;
        if resets & event::FIFO_A_RESET != 0 {
            bus.sound.fifo[0].reset();
        }
        if resets & event::FIFO_B_RESET != 0 {
            bus.sound.fifo[1].reset();
        }
    }
}

crate::snap::snap!(Fifo { buf, read, len });
crate::snap::snap!(SwapStats {
    swaps,
    relatches,
    max_cycles,
    max_pops,
    max_refills,
    swap_at,
    waiting,
    pops,
    refills
});
crate::snap::snap!(Sound {
    fifo,
    out,
    records,
    len,
    samples,
    samples_len,
    spans,
    spans_len,
    restarts,
    restarts_len,
    swaps,
    swaps_len,
    period,
    dropped,
    swap,
    psg,
    bias_before,
});

#[cfg(test)]
mod tests;
