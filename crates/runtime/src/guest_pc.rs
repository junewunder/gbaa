//! The guest pc a lifted program last reached (unit
//! `native/first-stop-guest-pc`): compiled code keeps no pc, so each block
//! a lift prints (`codegen::native`) starts with [`set`] of its guest
//! address, the key of the block (bit 0 set for Thumb). A program that
//! stops (a panic, its time limit) says where with [`get`] in its stop
//! line (`native_record::stop`). It names the block, not the instruction
//! in it: one relaxed store per block is the whole cost. A hand-written
//! program sets none, and [`get`] stays `None`.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// The last block's address; 0 before the first (no lifted block is at 0).
static PC: AtomicU32 = AtomicU32::new(0);

/// The program has entered the block at `pc`.
#[inline(always)]
pub fn set(pc: u32) {
    PC.store(pc, Relaxed);
}

/// The last block entered, from any thread; `None` before the first.
pub fn get() -> Option<u32> {
    match PC.load(Relaxed) {
        0 => None,
        pc => Some(pc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_block_set_is_read() {
        set(0x0800_0a0d);
        set(0x0300_0100);
        assert_eq!(get(), Some(0x0300_0100));
        set(0);
        assert_eq!(get(), None);
    }
}
