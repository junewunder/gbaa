// The stage-A prelude (docs/native-port-design.md, section 6, stage A),
// written into every lifted crate by `gbaa-lift` (crates/codegen/src/native.rs):
// the program's state as byte regions, the memory those regions and the
// hardware library make up, the BIOS calls, and the function table.
// Generated: don't edit; rerun `just lift`.

use runtime::hw::{self, Exit, Hw, Native};

/// EWRAM's and IWRAM's sizes: `Game::ram` is EWRAM then IWRAM, laid out as
/// the game has them (the check layout, `--layout guest`, is the only one
/// a byte-region program has).
pub const EWRAM: usize = 0x4_0000;
pub const IWRAM: usize = 0x8000;

/// What a lifted function's `lr` holds on entry: no code address, so a
/// return (`bx lr`, `pop {pc}`) is told from a jump by its value.
pub const RET: u32 = 0xFFFF_FFF0;

/// The IRQ-mode stack pointer a handler starts on: the BIOS's IRQ stack
/// (0x0300_7FA0) less the six registers its stub pushes.
pub const IRQ_SP: u32 = 0x0300_7F88;

/// The ROM, for its data (section 8: bytes where nothing is typed yet).
pub static ROM: &[u8] = include_bytes!("../rom.gba");

/// The whole program's state.
pub struct Game {
    /// EWRAM then IWRAM, as bytes (stage A: no types yet).
    pub ram: Box<[u8; EWRAM + IWRAM]>,
    pub hw: Hw<Game>,
    /// The stack pointer where the program last waited: where an IRQ
    /// handler that switches to system mode puts its frames, as the
    /// hardware's banked `sp` would.
    pub sp: u32,
    /// The CPSR's control byte as the program sets it (system mode, the
    /// I bit for a critical section), through the stand-ins for code that
    /// reads or writes the CPSR (holes files; [`msr_cpsr_c`]). While I is
    /// set the waits run no handler ([`Native::irq_masked`]).
    pub cpsr: u32,
}

impl Native for Game {
    type Host = native_host::AnyHost;
    fn hw(&mut self) -> &mut Hw<Game> {
        &mut self.hw
    }
    fn irq_masked(&self) -> bool {
        IRQ_HOLD && self.cpsr & 0x80 != 0
    }
}

/// IRQs held while the CPSR's I bit is set (`false`: `gbaa-lift --irq-at-waits`).
pub const IRQ_HOLD: bool = true;

/// A store that sets `IME` again runs what's pending at once, on the
/// stack the lifted code put in `Game::sp` before the store (`false`:
/// `gbaa-lift --ime-at-waits`, held to the next wait).
pub const IME_RELEASE: bool = true;

/// `msr cpsr_c, v` (a critical section's end, a stand-in's): the control
/// byte as written. Clearing the I bit runs the handlers of the IRQs it
/// held at once, as the hardware takes them, on the stack the program is
/// on (`sp`), as system mode's banked sp would be.
pub fn msr_cpsr_c(g: &mut Game, sp: u32, v: u32) {
    let held = g.cpsr & 0x80 != 0;
    g.cpsr = g.cpsr & !0xFF | v & 0xFF;
    if held && v & 0x80 == 0 {
        g.sp = sp;
        hw::irq_release(g);
    }
}

impl Game {
    pub fn new(host: native_host::AnyHost) -> Box<Game> {
        let mut g = Box::new(Game {
            ram: vec![0u8; EWRAM + IWRAM]
                .into_boxed_slice()
                .try_into()
                .expect("RAM's size"),
            hw: Hw::new(host),
            sp: 0x0300_7F00,
            cpsr: 0x1F,
        });
        g.hw.irq.set_handler(irq_entry);
        g.hw.on_frame_end(record);
        g
    }
}

fn record(g: &mut Game) {
    native_host::record(&g.hw, &g.ram[..]);
}

/// The handler the game stored at `0x0300_7FFC` (`IRQ_HANDLER_PTR`), run
/// as the BIOS's IRQ stub runs it: r0 = `0x0400_0000`, on the IRQ stack.
fn irq_entry(g: &mut Game) {
    let h = ld32(g, 0x0300_7FFC);
    if h == 0 {
        return;
    }
    let k = if h & 1 != 0 { h } else { h & !3 };
    // IRQ mode, I set, as the exception enters it; back as it was on the
    // return (the BIOS's `subs pc, lr, #4` restores the SPSR).
    let cpsr = g.cpsr;
    g.cpsr = 0x92;
    // A handler doesn't wait, so it can't be told to stop.
    let _ = call_fn(g, k, IRQ_SP, [0x0400_0000, 0, 0, 0]);
    g.cpsr = cpsr;
}

/// The RAM offset of a guest EWRAM or IWRAM address (mirrors included).
#[inline(always)]
fn ram_at(a: u32) -> Option<usize> {
    match a >> 24 {
        2 => Some((a & 0x3_FFFF) as usize),
        3 => Some(EWRAM + (a & 0x7FFF) as usize),
        _ => None,
    }
}

/// VRAM's offset for `a`, or `None` in the bitmap modes' dead window (the
/// play build's `Bus::vram_offset`).
#[inline(always)]
fn vram_at(g: &Game, a: u32) -> Option<usize> {
    let o = (a & 0x1_FFFF) as usize;
    if o < 0x1_8000 {
        Some(o)
    } else if o < 0x1_C000 && g.hw.ppu.dispcnt.0 & 7 >= 3 {
        None
    } else {
        Some(o - 0x8000)
    }
}

/// The I/O offset of `a` (`0x0400_0000` on), not wrapped: past 0x3FF
/// the play build maps nothing, so [`io_unused`] takes it.
#[inline(always)]
fn io_at(a: u32) -> u32 {
    a & 0x00FF_FFFF
}

/// I/O offsets the hardware has nothing at, or that the library doesn't
/// keep and the game only writes (a word store's high half): reads give 0,
/// writes go nowhere.
fn io_unused(o: u32) -> bool {
    matches!(
        o,
        0x0056..=0x005E
            | 0x00A8..=0x00AE
            | 0x00E0..=0x00FE
            | 0x0110..=0x011E
            | 0x0206
            | 0x020A
            | 0x020C..=0x02FF
            | 0x0302..
    )
}

fn io_read16(g: &mut Game, o: u32) -> u32 {
    if io_unused(o) {
        return 0;
    }
    g.hw.read16(o) as u32
}

fn io_write16(g: &mut Game, o: u32, v: u32) {
    if io_unused(o) {
        return;
    }
    if IME_RELEASE && o == runtime::io::reg::IME {
        return hw::write_ime(g, v as u16);
    }
    g.hw.write16(o, v as u16);
}

fn rom16(o: usize) -> u32 {
    match ROM.get(o..o + 2) {
        Some(b) => u16::from_le_bytes([b[0], b[1]]) as u32,
        None => (o as u32 >> 1) & 0xFFFF,
    }
}

#[inline(never)]
fn unmapped(what: &str, a: u32) -> ! {
    panic!("{what} at {a:#010x}: no memory the program has there")
}

pub fn ld8(g: &mut Game, a: u32) -> u32 {
    if let Some(o) = ram_at(a) {
        return g.ram[o] as u32;
    }
    match a >> 24 {
        4 => (io_read16(g, io_at(a) & !1) >> ((a & 1) * 8)) & 0xFF,
        5 => g.hw.ppu.palette[(a & 0x3FF) as usize] as u32,
        6 => vram_at(g, a).map_or(0, |o| g.hw.ppu.vram[o] as u32),
        7 => g.hw.ppu.oam[(a & 0x3FF) as usize] as u32,
        8..=0xD => {
            let o = (a & 0x1FF_FFFF) as usize;
            (rom16(o & !1) >> ((o & 1) * 8)) & 0xFF
        }
        _ => unmapped("a byte load", a),
    }
}

pub fn ld16(g: &mut Game, a: u32) -> u32 {
    if let Some(o) = ram_at(a) {
        return u16::from_le_bytes([g.ram[o], g.ram[o + 1]]) as u32;
    }
    let le = |m: &[u8], o: usize| u16::from_le_bytes([m[o], m[o + 1]]) as u32;
    match a >> 24 {
        4 => io_read16(g, io_at(a) & !1),
        5 => le(&g.hw.ppu.palette, (a & 0x3FE) as usize),
        6 => vram_at(g, a).map_or(0, |o| le(&g.hw.ppu.vram, o)),
        7 => le(&g.hw.ppu.oam, (a & 0x3FE) as usize),
        8..=0xD => rom16((a & 0x1FF_FFFE) as usize),
        _ => unmapped("a halfword load", a),
    }
}

pub fn ld32(g: &mut Game, a: u32) -> u32 {
    if let Some(o) = ram_at(a) {
        let r = &g.ram[o..o + 4];
        return u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
    }
    match a >> 24 {
        4 => {
            let o = io_at(a) & !3;
            io_read16(g, o) | io_read16(g, o + 2) << 16
        }
        5..=0xD => ld16(g, a) | ld16(g, a.wrapping_add(2)) << 16,
        _ => unmapped("a word load", a),
    }
}

pub fn st8(g: &mut Game, a: u32, v: u32) {
    let v = v as u8;
    if let Some(o) = ram_at(a) {
        g.ram[o] = v;
        return;
    }
    match a >> 24 {
        4 => {
            let o = io_at(a);
            let shift = (o & 1) * 8;
            // IF: a 1 acknowledges, so the other byte writes 0s.
            let keep = if o & !1 == runtime::io::reg::IF || io_unused(o & !1) {
                0
            } else {
                g.hw.raw16(o & !1) as u32 & !(0xFF << shift)
            };
            io_write16(g, o & !1, keep | (v as u32) << shift);
        }
        // Palette and VRAM take a byte store as a halfword of it.
        5 => {
            let o = (a & 0x3FE) as usize;
            g.hw.ppu.palette[o..o + 2].copy_from_slice(&[v, v]);
        }
        6 => {
            let bg_end = if g.hw.ppu.dispcnt.0 & 7 >= 3 {
                0x1_4000
            } else {
                0x1_0000
            };
            if let Some(o) = vram_at(g, a).filter(|&o| o < bg_end) {
                let o = o & !1;
                g.hw.ppu.vram[o..o + 2].copy_from_slice(&[v, v]);
            }
        }
        // OAM ignores byte stores; ROM takes none.
        7..=0xD => {}
        _ => unmapped("a byte store", a),
    }
}

pub fn st16(g: &mut Game, a: u32, v: u32) {
    let b = (v as u16).to_le_bytes();
    if let Some(o) = ram_at(a) {
        g.ram[o..o + 2].copy_from_slice(&b);
        return;
    }
    match a >> 24 {
        4 => io_write16(g, io_at(a) & !1, v & 0xFFFF),
        5 => {
            let o = (a & 0x3FE) as usize;
            g.hw.ppu.palette[o..o + 2].copy_from_slice(&b);
        }
        6 => {
            if let Some(o) = vram_at(g, a) {
                g.hw.ppu.vram[o..o + 2].copy_from_slice(&b);
            }
        }
        7 => {
            let o = (a & 0x3FE) as usize;
            g.hw.ppu.oam[o..o + 2].copy_from_slice(&b);
        }
        8..=0xD => {}
        _ => unmapped("a halfword store", a),
    }
}

pub fn st32(g: &mut Game, a: u32, v: u32) {
    if let Some(o) = ram_at(a) {
        g.ram[o..o + 4].copy_from_slice(&v.to_le_bytes());
        return;
    }
    match a >> 24 {
        4 => {
            let o = io_at(a) & !3;
            io_write16(g, o, v & 0xFFFF);
            io_write16(g, o + 2, v >> 16);
        }
        5..=7 => {
            st16(g, a, v & 0xFFFF);
            st16(g, a.wrapping_add(2), v >> 16);
        }
        8..=0xD => {}
        _ => unmapped("a word store", a),
    }
}

/// `IntrWait` (r0: discard old flags, r1: the interrupts to wait for), as
/// the play build's BIOS does it: the flags at `0x0300_7FF8` the game's
/// handler sets, `IME` forced on (straight to `Hw`: the BIOS runs with I
/// set, so it releases nothing), halted until one in `mask` is set. r0.
pub fn intr_wait(g: &mut Game, sp: u32, discard: bool, mask: u32) -> Result<u32, Exit> {
    const BIOS_IF: u32 = 0x0300_7FF8;
    const IME: u32 = runtime::io::reg::IME;
    let mut check = !discard;
    if discard {
        let f = ld16(g, BIOS_IF);
        st16(g, BIOS_IF, f & !mask);
        g.hw.write16(IME, 1);
    }
    loop {
        if check {
            g.hw.write16(IME, 0);
            let f = ld16(g, BIOS_IF);
            let hit = f & mask;
            if hit != 0 {
                st16(g, BIOS_IF, f ^ hit);
            }
            g.hw.write16(IME, 1);
            if hit != 0 {
                return Ok(hit);
            }
        }
        check = true;
        g.sp = sp;
        hw::halt(g)?;
    }
}

/// `CpuSet` (r0 source, r1 destination, r2 count and mode), as the play
/// build's: the copy or fill, its registers as it leaves them, and its
/// time ([`hw::spend`]).
pub fn cpu_set(g: &mut Game, sp: u32, r: &mut [u32; 4]) -> Result<(), Exit> {
    if r[0] < 0x0200_0000 {
        return Ok(());
    }
    let count = r[2] & 0x1F_FFFF;
    let fill = r[2] & 1 << 24 != 0;
    let word = r[2] & 1 << 26 != 0;
    let cycles = hw::bios::cpu_set_cycles(g.hw.waitcnt, r[0], r[1], r[2]);
    if word {
        let fill_word = if fill {
            let v = ld32(g, r[0] & !3);
            r[0] = r[0].wrapping_add(4);
            Some(v)
        } else {
            None
        };
        for _ in 0..count {
            let v = match fill_word {
                Some(v) => v,
                None => {
                    let v = ld32(g, r[0] & !3);
                    r[0] = r[0].wrapping_add(4);
                    v
                }
            };
            st32(g, r[1] & !3, v);
            r[1] = r[1].wrapping_add(4);
        }
    } else {
        let (src, dst) = (r[0] & !1, r[1] & !1);
        for i in 0..count {
            let v = ld16(g, if fill { src } else { src.wrapping_add(2 * i) });
            st16(g, dst.wrapping_add(2 * i), v);
        }
    }
    r[3] = 0x170;
    g.sp = sp;
    hw::spend(g, cycles)
}

/// `Div` (r0 / r1): r0 the quotient, r1 the remainder, r3 the quotient's
/// magnitude.
pub fn div(r: &mut [u32; 4]) {
    let (q, m, a) = hw::bios::div(r[0] as i32, r[1] as i32);
    r[0] = q as u32;
    r[1] = m as u32;
    r[3] = a;
}

/// A function pointer's call: `k` is the function's key (its ROM or IWRAM
/// address, bit 0 for Thumb), the ID of every function in stage A
/// (`--fn-ids rom`).
#[inline(never)]
pub fn call_fn(g: &mut Game, k: u32, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {
    match FNS.binary_search_by_key(&k, |&(e, _)| e) {
        Ok(i) => (FNS[i].1)(g, sp, a),
        Err(_) => panic!("a call or jump to {k:#010x}, which is no function's entry"),
    }
}

/// A lifted function: the game, the stack pointer, r0-r3 in; r0-r3 out.
pub type Fn = fn(&mut Game, u32, [u32; 4]) -> Result<[u32; 4], Exit>;

/// Every register and flag a function's extended entry (`x_<key>`) starts
/// with: r0-r14, and N, Z, C, V. A fall or jump from one function into
/// another's entry passes its own, as the hardware runs on with them; a
/// call (`f_<key>`) starts with [`Regs::call`].
pub struct Regs {
    pub r: [u32; 15],
    pub f: [bool; 4],
}

impl Regs {
    /// A call's: r0-r3 and sp given, lr [`RET`], the rest zero.
    pub fn call(sp: u32, a: [u32; 4]) -> Regs {
        let mut r = [0; 15];
        r[..4].copy_from_slice(&a);
        r[13] = sp;
        r[14] = RET;
        Regs { r, f: [false; 4] }
    }
}

fn main() -> std::process::ExitCode {
    native_host::main(NAME, true, Game::new, |g| {
        let r = ENTRY(g, 0x0300_7F00, [0; 4]);
        match r {
            Err(e) => Err(e),
            Ok(_) => panic!("the program's entry returned"),
        }
    })
}
