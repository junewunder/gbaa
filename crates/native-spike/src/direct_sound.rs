//! `native-direct-sound`: agb/direct-sound as a native Rust program, the
//! native check's direct-sound program. The same flags as `native-spike`
//! (src/host.rs).
//!
//! What it plays (agb/direct-sound/SOUND.md): FIFO A and B fed by DMA1 and
//! DMA2 ([`Hw::fifo_dma`]) from double buffers at fixed IWRAM addresses,
//! clocked by timer 0, with timer 1 counting up on timer 0's overflows to
//! raise an IRQ every 256 samples. Its handler ([`swap`]) re-latches both
//! channels on the other buffer and fills the one just played. The
//! buffers and the synth's state are this program's own globals, at the
//! ROM's addresses in the check layout, so the check compares them.

mod host;

use std::convert::Infallible;

use runtime::hw::{self, DispCnt, Exit, Hw, Native};
use runtime::io::{irq, reg};

pub use host::AnyHost;

/// The whole program's state.
pub struct Game {
    pub ram: Box<Ram>,
    pub hw: Hw<Game>,
}

impl Native for Game {
    type Host = AnyHost;
    fn hw(&mut self) -> &mut Hw<Game> {
        &mut self.hw
    }
    /// No CPSR: written over the library, never in a critical section.
    fn irq_masked(&self) -> bool {
        false
    }
}

/// Samples a buffer holds: timer 1's IRQ comes after each.
const LEN: usize = 256;

/// EWRAM then IWRAM, as the ROM has them: EWRAM is agb's (skipped by the
/// check by name), and in IWRAM the program's buffers at `0x0300_4000`
/// and its synth's state after them.
#[derive(zerocopy::IntoBytes, zerocopy::FromZeros, zerocopy::Immutable)]
#[repr(C)]
pub struct Ram {
    _ewram: [u8; 0x4_0000],
    _iwram: [u8; 0x4000],
    /// FIFO A's two halves, then FIFO B's.
    buf: [[[i8; LEN]; 2]; 2],
    /// Buffers filled since the start, A's phase, B's phase.
    state: [u32; 3],
    _rest: [u8; 0x8000 - 0x4000 - 4 * LEN - 12],
}

impl Ram {
    fn new() -> Box<Ram> {
        <Ram as zerocopy::FromZeros>::new_box_zeroed().expect("RAM's 288 KB")
    }
}

/// The buffers' address: FIFO `ch`'s half `half` is `LEN` bytes at
/// `BUF + (2 * ch + half) * LEN`.
const BUF: u32 = 0x0300_4000;

/// DMA to a FIFO: enabled, special start, 32-bit, repeat, destination
/// fixed.
const FIFO_DMA: u16 = 0x8000 | 3 << 12 | 1 << 10 | 1 << 9 | 2 << 5;

/// The RAM word at `a` (aligned, in IWRAM), for the FIFOs' DMA: a refill
/// past a buffer's end reads on into the next one, as on the hardware.
fn iwram32(g: &mut Game, a: u32) -> u32 {
    let bytes = zerocopy::IntoBytes::as_bytes(&*g.ram);
    let o = 0x4_0000 + a.wrapping_sub(0x0300_0000) as usize;
    let b = &bytes[o..o + 4];
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Fill buffer `half` of both FIFOs with the next `LEN` samples: on A a
/// sawtooth whose step changes with each buffer, on B a square whose
/// period does.
fn fill(g: &mut Game, half: usize) {
    let [n, mut a, mut b] = g.ram.state;
    let step = 2 + n % 5;
    let period = 32 + 8 * (n % 4);
    for i in 0..LEN {
        a = a.wrapping_add(step);
        b = (b + 1) % period;
        g.ram.buf[0][half][i] = ((a % 128) as i32 - 64) as i8;
        g.ram.buf[1][half][i] = if b < period / 2 { 40 } else { -40 };
    }
    g.ram.state = [n + 1, a, b];
}

/// Switch FIFO DMA channel `n` off and on again aimed at `src`, so it
/// latches there.
fn latch(g: &mut Game, n: usize, src: u32, fifo: usize) {
    g.hw.fifo_dma_stop(n);
    g.hw.fifo_dma(n, fifo, src, FIFO_DMA, iwram32);
}

/// FIFO `ch`'s buffer `half`.
const fn buf(ch: usize, half: usize) -> u32 {
    BUF + ((2 * ch + half) * LEN) as u32
}

/// Timer 1's IRQ: a buffer has played. Play the other, refill this one.
fn swap(g: &mut Game) {
    let played = (g.ram.state[0] % 2) as usize;
    let next = 1 - played;
    latch(g, 1, buf(0, next), 0);
    latch(g, 2, buf(1, next), 1);
    fill(g, played);
}

/// agb/direct-sound's `main`, from agb's entry on.
pub fn program(g: &mut Game) -> Result<Infallible, Exit> {
    host::agb_crt0(
        g,
        &host::Sections {
            ewram: (0x0802_5088, 0xB8),
            iwram: (0x0802_4E5C, 0x22C),
        },
    )?;
    g.hw.ppu.dispcnt = DispCnt::FORCED_BLANK;
    g.ram.state = [0; 3];
    fill(g, 0);
    fill(g, 1);
    // Swaps count buffers filled; the first swap plays buffer 1.
    g.ram.state[0] = 0;
    // Master on, no PSG; FIFO A left and B right at 100%, both on timer
    // 0, both reset.
    g.hw.write16(reg::SOUNDCNT_X, 0x80);
    g.hw.write16(reg::SOUNDCNT_H - 2, 0);
    g.hw.write16(
        reg::SOUNDCNT_H,
        1 << 2 | 1 << 3 | 1 << 9 | 1 << 11 | 1 << 12 | 1 << 15,
    );
    latch(g, 1, buf(0, 0), 0);
    latch(g, 2, buf(1, 0), 1);
    // agb's `add_interrupt_handler(Timer1, ..)`.
    g.hw.irq.on(irq::TIMER0 << 1, swap);
    // Timer 1 counts timer 0's overflows, IRQ every LEN, its first period
    // 8 pops longer (the ROM's SOUND.md); timer 0 every 1024 cycles.
    let tm1 = reg::TM0CNT_L + 4;
    g.hw.write16(tm1, (0x1_0000 - LEN - 8) as u16);
    g.hw.write16(tm1 + 2, 0x80 | 0x40 | 0x04);
    g.hw.write16(tm1, (0x1_0000 - LEN) as u16);
    g.hw.write16(reg::TM0CNT_L, 0xFC00);
    g.hw.write16(reg::TM0CNT_L + 2, 0x80);
    // agb's `VBlank::get`.
    g.hw.stat.vblank_irq = true;
    g.hw.irq.on(irq::VBLANK, |_| {});
    loop {
        hw::wait_vblank(g)?;
    }
}

fn record(g: &mut Game) {
    host::record(&g.hw, zerocopy::IntoBytes::as_bytes(&*g.ram));
}

fn main() -> std::process::ExitCode {
    host::main(
        "native-direct-sound",
        true,
        |h| {
            let mut g = Box::new(Game {
                ram: Ram::new(),
                hw: Hw::new(h),
            });
            g.hw.on_frame_end(record);
            g
        },
        program,
    )
}
