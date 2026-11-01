//! A hook on every prefetch stall ([`crate::bus::Bus::stall`]), for
//! docs/research/why-timing-is-runtime.md: log what each memory
//! instruction's data accesses cost and from what, or replace that cost
//! (the buffer off, no cost, one baked number a site) to see what the game
//! does with its timing wrong. Only in builds with the `timing-lab`
//! feature (gbaa-desktop's, `just timing-lab`).

use core::sync::atomic::{AtomicPtr, Ordering};

/// One memory instruction's data accesses, as the bus settles them.
#[derive(Clone, Copy, Debug)]
pub struct Stall {
    /// The ROM region it runs from (`0x08`..`0x0D`).
    pub region: usize,
    /// Its address plus two fetches.
    pub pc: u32,
    /// It loads (has an I cycle).
    pub load: bool,
    /// Its data accesses' cycles, before the buffer's overlap: what the
    /// address it touched costs (`Waits::data`).
    pub data: i32,
    /// It touched the cartridge, which the buffer can't overlap.
    pub cart: bool,
    /// `WAITCNT` as the game last wrote it (bit 14: the buffer is on).
    pub waitcnt: u16,
    /// How far ahead of `pc` the buffer had fetched, in bytes, before it
    /// ran (`u32::MAX`-ish when it was behind or emptied).
    pub ahead: u32,
    /// What the bus adds for it.
    pub cost: i32,
}

/// What the bus adds instead of [`Stall::cost`].
pub type Hook = fn(&Stall) -> i32;

static HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Send every stall through `h` from now on.
pub fn set(h: Hook) {
    HOOK.store(h as *mut (), Ordering::Relaxed);
}

/// `s`'s cost, through the hook if one is set.
pub(crate) fn cost(s: &Stall) -> i32 {
    let h = HOOK.load(Ordering::Relaxed);
    if h.is_null() {
        return s.cost;
    }
    // SAFETY: only `set` stores here, and it stores a `Hook`.
    let h: Hook = unsafe { core::mem::transmute::<*mut (), Hook>(h) };
    h(s)
}
