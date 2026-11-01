//! The guest on its own stack (`docs/structured-codegen-design.md`, step
//! 7): the dispatcher and compiled code run in a `corosensei` coroutine,
//! and the tick that finds the frame over switches to the frame loop
//! ([`runtime::dispatch::Nest::frame_end`]) instead of unwinding. The frame
//! loop switches back for the next frame, and compiled code goes on.
//!
//! The machine is owned through a raw pointer (`Box::into_raw`). While a
//! guest is alive the frame loop touches it only through the pointer
//! [`Guest::frame`] hands out at the switch (derived from the guest's
//! innermost `&mut`), and only until it calls [`Guest::frame`] again. The
//! same for the [`Stats`], which the guest reaches through a raw pointer
//! too. Dropping a suspended guest (a state load, a rewind) discards its
//! frames without running them (they have no destructors); the next
//! [`Guest::frame`] starts a new one from `cpu.pc`.
//!
//! Netplay runs a guest per console on one thread ([`Guests`]), each
//! switching out at the link hub's deadline as at the frame end; a
//! rollback's restore ends them, and new ones start on their stacks
//! ([`Guest::into_stack`]).

use crate::{Code, Desktop, DesktopMachine};
use corosensei::stack::{DefaultStack, Stack};
use corosensei::{Coroutine, CoroutineResult, Yielder};
use runtime::dispatch::{self, Stats};
use std::cell::Cell;

/// The guest stack's size: about 20 times the deepest seen on every route
/// (the design doc's step 7), with a guard page below it.
pub const STACK: usize = 1 << 20;

type Co = Coroutine<(), *mut DesktopMachine, (), DefaultStack>;

type Back = *const Yielder<(), *mut DesktopMachine>;

thread_local! {
    /// The running guest's way back to the frame loop: set for each
    /// resume ([`Guest::frame`]), as guests on one thread take turns.
    static YIELDER: Cell<Back> = const { Cell::new(std::ptr::null()) };
}

/// [`runtime::dispatch::Nest::frame_end`]: switch to the frame loop,
/// handing it the machine.
fn switch(m: *mut ()) {
    let y = YIELDER.get();
    assert!(!y.is_null(), "a frame end switch outside a guest");
    // SAFETY: set by the running guest's coroutine, whose stack this is.
    unsafe { (*y).suspend(m as *mut DesktopMachine) };
}

pub struct Guest {
    /// None only in [`Guest::into_stack`].
    co: Option<Co>,
    owner: *mut DesktopMachine,
    /// The stack's top.
    top: usize,
    /// The guest's [`YIELDER`], once it has started.
    back: Box<Cell<Back>>,
}

impl Guest {
    /// A guest for `owner`, started at its `cpu.pc` by the first
    /// [`Guest::frame`]. `owner` and `stats` must outlive it.
    pub fn new(owner: *mut DesktopMachine, code: &'static Code, stats: *mut Stats) -> Guest {
        let stack = DefaultStack::new(STACK).expect("a guest stack");
        Guest::on(stack, owner, code, stats)
    }

    /// [`Guest::new`] on `stack`, one an earlier guest left
    /// ([`Guest::into_stack`]).
    pub fn on(
        stack: DefaultStack,
        owner: *mut DesktopMachine,
        code: &'static Code,
        stats: *mut Stats,
    ) -> Guest {
        let top = stack.base().get();
        let back = Box::new(Cell::new(std::ptr::null()));
        let mine: *const Cell<Back> = &*back;
        let co = Coroutine::with_stack(stack, move |y: &Yielder<_, _>, ()| {
            YIELDER.set(y as *const _);
            // SAFETY: boxed in the guest, which outlives its coroutine.
            unsafe { (*mine).set(y as *const _) };
            // SAFETY: the frame loop doesn't touch the machine while the
            // guest runs (see the module's doc).
            let m = unsafe { &mut *owner };
            m.nest.frame_end = Some(switch);
            // Returns only on a fault, which the frame loop sees.
            let _ = dispatch::run_guest(m, code, stats);
            m.nest.frame_end = None;
        });
        Guest {
            co: Some(co),
            owner,
            top,
            back,
        }
    }

    /// Run the guest to the next frame end: the machine to use until the
    /// next call. None: the guest stopped on a fault and is over; use the
    /// owner's pointer.
    pub fn frame(&mut self) -> Option<*mut DesktopMachine> {
        let co = self.co.as_mut().expect("a guest");
        // A started guest suspends through its own yielder, not the one
        // that last ran; the first resume sets it.
        let outer = YIELDER.replace(self.back.get());
        let r = co.resume(());
        YIELDER.set(outer);
        match r {
            CoroutineResult::Yield(m) => Some(m),
            CoroutineResult::Return(()) => None,
        }
    }

    /// End the guest as dropping it does, and keep its stack for the next.
    pub fn into_stack(mut self) -> DefaultStack {
        self.stop();
        self.co.take().expect("a guest").into_stack()
    }

    /// Discard a suspended guest's frames ([`Drop`]); it can't resume.
    fn stop(&mut self) {
        let Some(co) = &mut self.co else {
            return;
        };
        let suspended = co.started() && !co.done();
        // SAFETY: the guest's frames own nothing to drop; one not started
        // or done has none.
        unsafe { co.force_reset() };
        if suspended {
            // SAFETY: the guest is gone, so the owner's is the only pointer.
            unsafe { (*self.owner).nest.forget_run() };
        }
    }

    /// How deep the guest's stack has been: bytes from its top to the
    /// lowest byte written. Unix only, where the stack is all mapped,
    /// zeroed, [`STACK`] bytes above its guard page.
    pub fn high_water(&self) -> Option<usize> {
        if !cfg!(unix) {
            return None;
        }
        let mut p = self.top - STACK;
        // SAFETY: the stack's mapped, readable range.
        while p < self.top && unsafe { *(p as *const u8) } == 0 {
            p += 1;
        }
        Some(self.top - p)
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Netplay's consoles on guest stacks ([`netplay::gba::Turns`]): each
/// seat's guest runs a turn of the cable to the hub's deadline and
/// switches out, and goes on in compiled code at the next turn, where
/// unwinding would resume a structured function in the interpreter. A
/// rollback's restore ends them; new ones start from each `cpu.pc`, on
/// the same stacks, running the functions they resume block-timed
/// ([`runtime::dispatch::Nest::timed_resumes`]), so they compute what the
/// guests that ran on did.
#[derive(Default)]
pub struct Guests {
    /// Each seat's guest.
    seats: Vec<Option<Guest>>,
    /// Stacks ended guests left.
    spare: Vec<DefaultStack>,
}

impl netplay::gba::Turns<Desktop> for Guests {
    fn turn(
        &mut self,
        seat: usize,
        m: &mut DesktopMachine,
        code: &'static Code,
        stats: &mut Stats,
    ) {
        if self.seats.len() <= seat {
            self.seats.resize_with(seat + 1, || None);
        }
        let spare = &mut self.spare;
        let g = self.seats[seat].get_or_insert_with(|| match spare.pop() {
            Some(s) => Guest::on(s, m, code, stats),
            None => Guest::new(m, code, stats),
        });
        m.nest.timed_resumes = true;
        // The hub uses the machine between turns, as the window's frame
        // loop does between frames, while the guest is switched out.
        match g.frame() {
            Some(p) => {
                // SAFETY: the guest's, until it resumes.
                let m = unsafe { &mut *p };
                if std::mem::take(&mut m.nest.vblank) {
                    m.show_frame();
                }
            }
            // A fault ended the guest; the hub sees it.
            None => {
                if let Some(g) = self.seats[seat].take() {
                    self.spare.push(g.into_stack());
                }
            }
        }
    }

    fn end(&mut self) {
        for g in &mut self.seats {
            if let Some(g) = g.take() {
                self.spare.push(g.into_stack());
            }
        }
    }
}
