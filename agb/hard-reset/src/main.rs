//! gbaa test ROM (bios/multiboot-hardreset): a program that calls the
//! BIOS's `HardReset` (`swi 0x26`) and must come back to its first screen.
//!
//! Mode 0 with no layers on, so the screen is the backdrop, palette 0.
//! From boot it is red. At frame 30 it turns green and [`MARKER`] is
//! written; at frame 40 the program calls `HardReset`, which clears every
//! RAM and register and starts the cartridge again: red, the marker 0 and
//! [`FRAMES`] counting from 0. So at frame 60 since power-on the screen
//! is red, `MARKER` reads 0 and `FRAMES` about 20 (until frame 70, when
//! the green comes back, and 80, the next reset):
//! `gbaa-interp hard-reset.gba --frames 60 --peek 0203FF00 --peek 0203FF04`.
//! A runtime without `HardReset` stops at the `swi`.
#![no_std]
#![no_main]

use core::ptr::write_volatile;

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const PALETTE: *mut u16 = 0x0500_0000 as *mut u16;
/// Written at frame 30; `HardReset` clears it.
const MARKER: *mut u32 = 0x0203_FF00 as *mut u32;
/// Frames since the cartridge last started.
const FRAMES: *mut u32 = 0x0203_FF04 as *mut u32;

const RED: u16 = 0x001F;
const GREEN: u16 = 0x03E0;

/// `HardReset`, from Thumb so the comment field is the call's number.
#[instruction_set(arm::t32)]
#[inline(never)]
fn hard_reset() -> ! {
    // SAFETY: the BIOS restarts the machine; nothing here runs again.
    unsafe { core::arch::asm!("swi #0x26", options(noreturn)) }
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT and the palette are the display's; the two words at
    // the top of EWRAM are this program's (it allocates nothing).
    unsafe {
        write_volatile(DISPCNT, 0); // mode 0, no layers: the backdrop
        write_volatile(PALETTE, RED);
    }
    let mut frame: u32 = 0;
    loop {
        unsafe { write_volatile(FRAMES, frame) };
        match frame {
            30 => unsafe {
                write_volatile(PALETTE, GREEN);
                write_volatile(MARKER, 0xC0FF_EE00);
            },
            40 => hard_reset(),
            _ => {}
        }
        vblank.wait_for_vblank();
        frame += 1;
    }
}
