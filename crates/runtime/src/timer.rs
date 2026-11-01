//! Timers 0–3: prescaler, reload, cascade (count-up) and the overflow IRQ.
//! A timer that clocks a sound FIFO has its overflows walked one at a time
//! by [`crate::sound::run_timers`], which steps this module's [`run_to`].
//!
//! A timer isn't stepped every cycle. Each keeps the counter it had at a
//! time `since`; reads work the current value out from [`Io::now`], and the
//! machine brings every timer up to date at each sync, raising overflow
//! IRQs and feeding cascades. A timer whose overflow someone listens to (its
//! IRQ, or a cascading timer above it) gets a scheduled event at that
//! overflow, so the IRQ is on time to within the caller's tick.
//!
//! [`Io::now`]: crate::io::Io::now

use crate::io::{Io, irq, reg};

/// `TMxCNT_H` bits.
pub mod ctl {
    /// Prescaler select: 1, 64, 256 or 1024 cycles a tick.
    pub const PRESCALER: u16 = 3;
    /// Count-up: tick on the timer below's overflow instead of the clock.
    pub const CASCADE: u16 = 1 << 2;
    pub const IRQ: u16 = 1 << 6;
    pub const ENABLE: u16 = 1 << 7;
}

/// One timer's running state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timer {
    /// The counter at `since`.
    pub counter: u16,
    /// When the counter was `counter`: the start of its current tick.
    pub since: u64,
    /// `TMxCNT_H` as of the last sync, which is what the timer runs on
    /// (the register itself may have been rewritten since).
    pub control: u16,
    /// Counting the clock: enabled and not cascading.
    pub ticking: bool,
}

impl Timer {
    /// log2 of the prescaler.
    #[inline]
    pub fn shift(&self) -> u32 {
        [0, 6, 8, 10][(self.control & ctl::PRESCALER) as usize]
    }

    /// The counter at time `now`, if nothing happens to the timer before
    /// then. What `TMxCNT_L` reads.
    #[inline]
    pub fn read(&self, now: u64, reload: u16) -> u16 {
        if !self.ticking {
            return self.counter;
        }
        let ticks = now.saturating_sub(self.since) >> self.shift();
        advance(self.counter, reload, ticks).0
    }

    /// When the counter next overflows, if it's ticking.
    pub fn next_overflow(&self) -> Option<u64> {
        self.ticking
            .then(|| self.since + ((0x1_0000 - self.counter as u64) << self.shift()))
    }
}

/// `counter` after `ticks` more ticks, reloading with `reload` at each
/// overflow, and how many overflows there were.
fn advance(counter: u16, reload: u16, ticks: u64) -> (u16, u64) {
    let total = counter as u64 + ticks;
    if total < 0x1_0000 {
        return (total as u16, 0);
    }
    let period = 0x1_0000 - reload as u64;
    let over = total - 0x1_0000;
    ((reload as u64 + over % period) as u16, 1 + over / period)
}

/// `TMxCNT_L`'s offset for timer `n`: its reload value as stored.
fn reload_reg(n: usize) -> u32 {
    reg::TM0CNT_L + 4 * n as u32
}

/// Bring every timer up to `io.now`, on the controls they last synced with:
/// count, reload, cascade, and raise overflow IRQs.
pub fn run(io: &mut Io) {
    run_to(io, io.now);
}

/// [`run`], but only up to `now` (at most `io.now`). Returns the timers
/// that overflowed, as a mask of `1 << n`.
pub fn run_to(io: &mut Io, now: u64) -> u8 {
    let mut carry = 0;
    let mut overflowed = 0;
    for n in 0..4 {
        let reload = io.raw16(reload_reg(n));
        let t = &mut io.timers[n];
        let ticks = if t.ticking {
            let shift = t.shift();
            let ticks = (now - t.since) >> shift;
            t.since += ticks << shift;
            ticks
        } else if t.control & ctl::ENABLE != 0 {
            // Cascading (timer 0 never is: see `control_written`).
            carry
        } else {
            0
        };
        let (counter, overflows) = advance(t.counter, reload, ticks);
        t.counter = counter;
        if overflows > 0 {
            overflowed |= 1 << n;
            if t.control & ctl::IRQ != 0 {
                io.request_irq(irq::TIMER0 << n);
            }
        }
        carry = overflows;
    }
    overflowed
}

/// Timer `n`'s control or reload was written (the bus calls this in the
/// store, after running the timers up to now on the old settings): switch
/// timer `n` to the new ones. A timer
/// that turns on starts from its reload value.
pub fn control_written(io: &mut Io, n: usize) {
    run(io);
    let control = io.raw16(reload_reg(n) + 2);
    let reload = io.raw16(reload_reg(n));
    let now = io.now;
    let t = &mut io.timers[n];
    let was_on = t.control & ctl::ENABLE != 0;
    let on = control & ctl::ENABLE != 0;
    // Timer 0 has nothing below it to count, so its count-up bit is
    // ignored (as in mGBA).
    let ticking = on && (n == 0 || control & ctl::CASCADE == 0);
    if on && !was_on {
        t.counter = reload;
    }
    // Clocked counting (re)starts now if it just began or the prescaler
    // changed.
    if (on && !was_on) || (ticking && !t.ticking) || (t.control ^ control) & ctl::PRESCALER != 0 {
        t.since = now;
    }
    t.control = control;
    t.ticking = ticking;
}

/// The next time a timer overflow matters (an IRQ, a cascade above it, or
/// a timer in `also`, a mask of `1 << n`: the sound FIFOs' clocks), if any.
pub fn next_event(io: &Io, also: u8) -> Option<u64> {
    let mut next: Option<u64> = None;
    for n in 0..4 {
        let t = &io.timers[n];
        let listened = t.control & ctl::IRQ != 0
            || also & (1 << n) != 0
            || io.timers.get(n + 1).is_some_and(|up| {
                up.control & (ctl::ENABLE | ctl::CASCADE) == ctl::ENABLE | ctl::CASCADE
            });
        if !listened {
            continue;
        }
        // A cascading timer overflows on a tick of the one below, which
        // has its own event if it's listened to.
        if let Some(at) = t.next_overflow() {
            next = Some(next.map_or(at, |e| e.min(at)));
        }
    }
    next
}

crate::snap::snap!(Timer {
    counter,
    since,
    control,
    ticking
});

#[cfg(test)]
mod tests;
