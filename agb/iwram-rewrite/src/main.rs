//! gbaa test ROM (#54): a function in IWRAM that the program rewrites
//! while it runs, for the overlay guard (decision 7).
//!
//! agb's crt0 copies `step` to IWRAM with the rest of `.iwram`. Each frame
//! the program runs `x = step(x)` and draws `x` as one pixel in mode 3, so
//! the frame records every value. At frame 20 it overwrites `step` with
//! `add r0, r0, #7; bx lr`; at frame 40 it writes the original words back.
//! A recompiled build must run the new code from frame 20 (the overlay is
//! stale) and its own again from frame 40, and draw what the interpreter
//! draws.
#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};

/// `x -> 3x + 1`, in IWRAM and ARM state.
#[unsafe(link_section = ".iwram")]
#[instruction_set(arm::a32)]
#[inline(never)]
extern "C" fn step(x: u32) -> u32 {
    x.wrapping_mul(3).wrapping_add(1)
}

/// `add r0, r0, #7; bx lr`.
const ADD7: [u32; 2] = [0xE280_0007, 0xE12F_FF1E];

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    let code = step as *const () as *mut u32;
    // SAFETY: `step` is at least two ARM instructions in IWRAM, which is
    // writable; DISPCNT and VRAM are the display's registers and memory.
    let saved = unsafe { [read_volatile(code), read_volatile(code.add(1))] };
    unsafe { write_volatile(DISPCNT, 0x0403) }; // mode 3, BG2 on
    let mut x: u32 = 1;
    let mut frame: usize = 0;
    loop {
        let words = match frame {
            20 => Some(ADD7),
            40 => Some(saved),
            _ => None,
        };
        if let Some(w) = words {
            unsafe {
                write_volatile(code, w[0]);
                write_volatile(code.add(1), w[1]);
            }
        }
        x = step(x);
        unsafe { write_volatile(VRAM.add(frame % (240 * 160)), x as u16 & 0x7FFF) };
        vblank.wait_for_vblank();
        frame += 1;
    }
}
