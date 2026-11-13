// The stage-A prelude (docs/native-port-design.md, section 6, stage A),
// written into every lifted crate by `gbaa-lift` (crates/codegen/src/native.rs):
// the program's state (RAM as bytes, which the typed globals of `Ram` lie
// over), the memory it and the hardware library make up, the BIOS calls,
// and the function table.
// Generated: don't edit; rerun `just lift`.

use runtime::hw::{self, Exit, Hw, Native};

/// EWRAM's and IWRAM's sizes: `Game::ram` is EWRAM then IWRAM, each byte
/// `g.ram[offset]` in either layout (the check layout, `--layout guest`, at
/// the game's offsets; the play layout, `--layout packed`, with the globals
/// as fields in the lifter's order). RAM is reached a byte at a time, so a
/// word may span two globals.
pub const EWRAM: usize = 0x4_0000;
pub const IWRAM: usize = 0x8000;

/// What a lifted function's `lr` holds on entry: no code address, so a
/// return (`bx lr`, `pop {pc}`) is told from a jump by its value.
pub const RET: u32 = 0xFFFF_FFF0;

/// The IRQ mode's stack pointer as the BIOS leaves it, till the reset
/// sets its own (`Game::banked`). A handler starts six registers below
/// it, the BIOS stub's push.
pub const IRQ_STACK: u32 = 0x0300_7FA0;

/// The ROM, for its data (section 8: bytes where nothing is typed yet).
pub static ROM: &[u8] = include_bytes!("../rom.gba");

/// The whole program's state.
pub struct Game {
    /// EWRAM then IWRAM: the typed globals of [`Ram`], and bytes.
    pub ram: Box<Ram>,
    pub hw: Hw<Game>,
    /// The stack pointer where the program last waited: where an IRQ
    /// handler that switches to system mode puts its frames, as the
    /// hardware's banked `sp` would (system mode's bank, [`msr_cpsr`]).
    pub sp: u32,
    /// The CPSR's control byte as the program sets it (the mode, the I
    /// bit for a critical section): the reset's and the IRQ wrapper's
    /// `msr` as the lifter prints them ([`msr_cpsr`]), and the stand-ins
    /// for other code that reads or writes the CPSR (holes files;
    /// [`msr_cpsr_c`]). While I is
    /// set the waits run no handler ([`Native::irq_masked`]).
    pub cpsr: u32,
    /// The stack pointers of the modes besides system and user (whose is
    /// [`Game::sp`]): IRQ, supervisor, and the rest (FIQ, abort,
    /// undefined) as one. A mode switch in the reset or the IRQ wrapper
    /// ([`msr_cpsr`]) banks sp here, as the hardware does. lr isn't
    /// banked: the wrappers keep it on the stack around their switch.
    pub banked: [u32; 3],
    /// The IRQ mode's SPSR: the CPSR an IRQ interrupted (its control
    /// byte; the flags of the code it stopped aren't known natively).
    pub spsr: u32,
    /// The return address of the last call of a stand-in that reads it
    /// (`lr = true` in its holes file): the caller's `bl` sets it.
    pub lr: u32,
    /// The tasks a game's task switcher runs ([`task_resume`]).
    pub tasks: Tasks,
    /// Where a `SoftReset` restarts the program ([`soft_reset`]), while
    /// its `Err(Exit)` unwinds the stack to `main`.
    pub reset: Option<u32>,
    /// What the BIOS region reads (`runtime::bus::BIOS_LATCH_*`), moved as
    /// the play build's BIOS moves it: after boot, after a call, in and
    /// after an IRQ.
    pub bios_latch: u32,
}

impl Native for Game {
    type Host = native_host::AnyHost;
    fn hw(&mut self) -> &mut Hw<Game> {
        &mut self.hw
    }
    fn irq_masked(&self) -> bool {
        IRQ_HOLD && self.cpsr & 0x80 != 0
    }
    /// DMA set up by its registers moves through the program's memory.
    fn dma_load(&mut self, a: u32, word: bool) -> u32 {
        if word { ld32(self, a) } else { ld16(self, a) }
    }
    fn dma_store(&mut self, a: u32, v: u32, word: bool) {
        if word { st32(self, a, v) } else { st16(self, a, v) }
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

/// The stack pointer of `mode` (a CPSR's low five bits), where a mode
/// switch banks it.
fn banked_sp(g: &mut Game, mode: u32) -> &mut u32 {
    match mode {
        0x10 | 0x1F => &mut g.sp,
        0x12 => &mut g.banked[0],
        0x13 => &mut g.banked[1],
        _ => &mut g.banked[2],
    }
}

/// `msr cpsr` (its control byte) in the reset or the IRQ wrapper, the
/// lifter's: on a mode switch sp (`sp`, the lifted code's r13) goes to
/// the old mode's bank and the new mode's comes back, as the result.
/// Clearing the I bit runs the IRQs it held at once, as [`msr_cpsr_c`].
pub fn msr_cpsr(g: &mut Game, sp: u32, v: u32) -> u32 {
    let (old, new) = (g.cpsr & 0x1F, v & 0x1F);
    let held = g.cpsr & 0x80 != 0;
    let sp = if old == new {
        sp
    } else {
        *banked_sp(g, old) = sp;
        *banked_sp(g, new)
    };
    g.cpsr = g.cpsr & !0xFF | v & 0xFF;
    if held && v & 0x80 == 0 {
        // The handler nests below this mode's sp as it is now.
        *banked_sp(g, new) = sp;
        hw::irq_release(g);
    }
    sp
}

/// sp (`sp`, the lifted code's r13) before a store that may set `IME`
/// again, in the reset or the IRQ wrapper: the current mode's bank, where
/// a handler it releases nests (`Game::sp` in system mode, as elsewhere).
pub fn bank_sp(g: &mut Game, sp: u32) {
    *banked_sp(g, g.cpsr & 0x1F) = sp;
}

/// `mrs rN, cpsr` in the reset or the IRQ wrapper: the flags from the
/// lifted code's locals, the control byte as the program set it.
pub fn mrs_cpsr(g: &Game, n: bool, z: bool, c: bool, v: bool) -> u32 {
    (n as u32) << 31 | (z as u32) << 30 | (c as u32) << 29 | (v as u32) << 28 | g.cpsr & 0xFF
}

impl Game {
    pub fn new(host: native_host::AnyHost) -> Box<Game> {
        let mut g = Box::new(Game {
            ram: Ram::zeroed(),
            hw: Hw::new(host),
            sp: 0x0300_7F00,
            cpsr: 0x1F,
            banked: [IRQ_STACK, 0x0300_7FE0, 0],
            spsr: 0,
            lr: RET,
            tasks: Tasks::default(),
            reset: None,
            bios_latch: runtime::bus::BIOS_LATCH_AFTER_BOOT,
        });
        // The save chip the ROM's library tag names, as the play build's bus.
        g.hw.save = runtime::save::Save::new(runtime::save::Kind::detect(ROM));
        g.hw.irq.set_handler(irq_entry);
        g.hw.on_frame_end(record);
        g
    }
}

impl Ram {
    fn zeroed() -> Box<Ram> {
        let l = std::alloc::Layout::new::<Ram>();
        // SAFETY: every bit pattern is a `Ram` (integers only), and the
        // allocation is `Ram`'s layout, owned by the box.
        let p = unsafe { std::alloc::alloc_zeroed(l) };
        if p.is_null() {
            std::alloc::handle_alloc_error(l);
        }
        unsafe { Box::from_raw(p.cast::<Ram>()) }
    }
}

// `Ram` (printed after the prelude) is EWRAM and IWRAM byte for byte:
// integer fields only, at the guest's offsets with no padding between
// (its size says so), little-endian as the GBA is.
const _: () = assert!(size_of::<Ram>() == EWRAM + IWRAM && cfg!(target_endian = "little"));

impl std::ops::Deref for Ram {
    type Target = [u8; EWRAM + IWRAM];
    fn deref(&self) -> &Self::Target {
        // SAFETY: `Ram` is that many initialised bytes (the assert above).
        unsafe { &*(self as *const Ram).cast() }
    }
}

impl std::ops::DerefMut for Ram {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: as `deref`; any bytes written are a valid `Ram`.
        unsafe { &mut *(self as *mut Ram).cast() }
    }
}

impl Ram {
    /// Where `n` bytes from RAM offset `o` are in `Ram`: in one piece here.
    #[inline(always)]
    fn span(o: usize, _n: usize) -> Option<usize> {
        Some(o)
    }
}

fn record(g: &mut Game) {
    native_host::record(&g.hw, &g.ram[..]);
}

/// The handler the game stored at `0x0300_7FFC` (`IRQ_HANDLER_PTR`), run
/// as the BIOS's IRQ stub runs it: r0 = `0x0400_0000`, on the IRQ stack
/// below the stub's six registers, the BIOS region reading the stub's
/// latch in it and after.
fn irq_entry(g: &mut Game) {
    let h = ld32(g, 0x0300_7FFC);
    if h == 0 {
        return;
    }
    g.bios_latch = runtime::bus::BIOS_LATCH_IN_IRQ;
    let k = if h & 1 != 0 { h } else { h & !3 };
    // IRQ mode, I set, as the exception enters it, the CPSR it stopped in
    // the SPSR; back as it was on the return (the BIOS's `subs pc, lr,
    // #4` restores the SPSR, which a wrapper that nests puts back). The
    // IRQ stack as it was: the stub pops what it pushed.
    let (cpsr, spsr, base) = (g.cpsr, g.spsr, g.banked[0]);
    g.spsr = cpsr;
    g.cpsr = 0x92;
    // A handler doesn't wait, so it can't be told to stop.
    let _ = call_fn(g, k, base.wrapping_sub(24), [0x0400_0000, 0, 0, 0]);
    // Nor restart: its `Err(Exit)` stops here, not at `main`.
    if let Some(to) = g.reset {
        panic!("a SoftReset (to {to:#010x}) in an IRQ handler, which the native program can't restart from");
    }
    (g.cpsr, g.spsr, g.banked[0]) = (cpsr, spsr, base);
    g.bios_latch = runtime::bus::BIOS_LATCH_AFTER_IRQ;
}

/// A BIOS call has returned: the BIOS region reads the opcode after its
/// `swi` return, as the play build's.
pub fn swi_done(g: &mut Game) {
    g.bios_latch = runtime::bus::BIOS_LATCH_AFTER_SWI;
}

/// The BIOS region's protection latch, `n` bytes at `a` (BIOS protection:
/// code outside the BIOS reads the last opcode fetched from it).
#[inline(never)]
fn bios_read(g: &Game, a: u32, n: u32) -> u32 {
    match n {
        1 => (g.bios_latch >> ((a & 3) * 8)) & 0xFF,
        2 => (g.bios_latch >> ((a & 2) * 8)) & 0xFFFF,
        _ => g.bios_latch,
    }
}

/// The BIOS's size: past it, the region reads open bus, which the
/// program doesn't keep.
const BIOS_SIZE: u32 = 0x4000;

/// The RAM offset of a guest EWRAM or IWRAM address (mirrors included).
#[inline(always)]
fn ram_at(a: u32) -> Option<usize> {
    match a >> 24 {
        2 => Some((a & 0x3_FFFF) as usize),
        3 => Some(EWRAM + (a & 0x7FFF) as usize),
        _ => None,
    }
}

/// A pointer the value sets say is always the global at RAM offset `off`
/// (native/ref-in-object): the lift reads the field, and checks the pointer.
#[inline(always)]
#[track_caller]
pub fn pin(a: u32, off: usize) {
    if ram_at(a) != Some(off) {
        unexplained(a, off, 1, 1)
    }
}

/// The element `a` points to of the global at RAM offset `off`, `n` of `e`
/// bytes, which the value sets bound it to (native/ref-in-object), checked.
#[inline(always)]
#[track_caller]
pub fn ix(a: u32, off: usize, e: usize, n: usize) -> usize {
    match ram_at(a) {
        Some(o) if o >= off && (o - off) % e == 0 && (o - off) / e < n => (o - off) / e,
        _ => unexplained(a, off, e, n),
    }
}

#[cold]
#[track_caller]
fn unexplained(a: u32, off: usize, e: usize, n: usize) -> ! {
    panic!("pointer {a:#010x} outside its global (RAM offset {off:#x}, {n} of {e} bytes): arithmetic the lift's types don't explain")
}

/// A pointer to a `T` in RAM (native/ref-heap): what the lift types a heap
/// pointer as, `T` a struct of the fields seen through it. Its value is
/// the guest's address; [`Game::at`] and [`Game::at_mut`] find the `T`.
#[repr(transparent)]
pub struct Ref<T>(pub u32, std::marker::PhantomData<T>);

impl<T> Ref<T> {
    #[inline(always)]
    pub fn new(a: u32) -> Ref<T> {
        Ref(a, std::marker::PhantomData)
    }

    /// The object whose field at offset `off` is at `a`.
    #[inline(always)]
    pub fn to(a: u32, off: u32) -> Ref<T> {
        Ref(a.wrapping_sub(off), std::marker::PhantomData)
    }
}

/// A struct the lift made of integer fields and byte padding only, packed
/// (so any bytes, anywhere, are one), and the alignment its objects have
/// in the guest.
pub unsafe trait Plain {
    const ALIGN: u32;
}

impl Game {
    /// The `T` at `r`: checked to be in EWRAM or IWRAM, not running past
    /// it, in one piece of `Ram`, and aligned in the guest as `T` is.
    #[inline(always)]
    #[track_caller]
    pub fn at<T: Plain>(&self, r: Ref<T>) -> &T {
        let o = obj_at::<T>(r.0);
        // SAFETY: `obj_at` checked `T`'s bytes are in `Ram`, and any bytes
        // are a `T` (`Plain`: packed, integers).
        unsafe { &*(&*self.ram as *const Ram).cast::<u8>().add(o).cast::<T>() }
    }

    #[inline(always)]
    #[track_caller]
    pub fn at_mut<T: Plain>(&mut self, r: Ref<T>) -> &mut T {
        let o = obj_at::<T>(r.0);
        // SAFETY: as `at`; any bytes written leave valid integers.
        unsafe { &mut *(&mut *self.ram as *mut Ram).cast::<u8>().add(o).cast::<T>() }
    }
}

/// Where in `Ram` the `T` at guest address `a` is.
#[inline(always)]
#[track_caller]
fn obj_at<T: Plain>(a: u32) -> usize {
    let n = size_of::<T>();
    let end = |o: usize| if o < EWRAM { EWRAM } else { EWRAM + IWRAM };
    match ram_at(a).filter(|&o| o + n <= end(o) && a % T::ALIGN == 0) {
        Some(o) => Ram::span(o, n).unwrap_or_else(|| bad_ref(a, n, T::ALIGN)),
        None => bad_ref(a, n, T::ALIGN),
    }
}

#[cold]
#[track_caller]
fn bad_ref(a: u32, n: usize, al: u32) -> ! {
    panic!("pointer {a:#010x} to {n} bytes isn't in RAM in one piece, aligned to {al}: an object the lift's heap types don't explain")
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
    // An immediate DMA runs in the store, through the program's memory.
    if (hw::dma_regs::DMA_START..hw::dma_regs::DMA_END).contains(&o) {
        return hw::write_dma(g, o, v as u16);
    }
    g.hw.write16(o, v as u16);
}

fn rom16(o: usize) -> u32 {
    match ROM.get(o..o + 2) {
        Some(b) => u16::from_le_bytes([b[0], b[1]]) as u32,
        None => (o as u32 >> 1) & 0xFFFF,
    }
}

/// Whether `a` reaches the EEPROM: all of `0D` behind a ROM of up to
/// 16 MB, its last 256 bytes behind a larger one (the play build's
/// `Bus::eeprom_at`).
fn eeprom_at(g: &Game, a: u32) -> bool {
    g.hw.save.kind == runtime::save::Kind::Eeprom
        && a >> 24 == 0xD
        && (ROM.len() <= 16 << 20 || a & 0x00FF_FF00 == 0x00FF_FF00)
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
        0 if a < BIOS_SIZE => bios_read(g, a, 1),
        0xE | 0xF => g.hw.save.read(a as u16) as u32,
        _ => unmapped("a byte load", a),
    }
}

pub fn ld16(g: &mut Game, a: u32) -> u32 {
    if let Some(o) = ram_at(a) {
        return g.ram[o] as u32 | (g.ram[o + 1] as u32) << 8;
    }
    let le = |m: &[u8], o: usize| u16::from_le_bytes([m[o], m[o + 1]]) as u32;
    match a >> 24 {
        4 => io_read16(g, io_at(a) & !1),
        5 => le(&g.hw.ppu.palette, (a & 0x3FE) as usize),
        6 => vram_at(g, a).map_or(0, |o| le(&g.hw.ppu.vram, o)),
        7 => le(&g.hw.ppu.oam, (a & 0x3FE) as usize),
        0xD if eeprom_at(g, a) => g.hw.save.eeprom_status() as u32,
        8..=0xD => rom16((a & 0x1FF_FFFE) as usize),
        0 if a < BIOS_SIZE => bios_read(g, a, 2),
        // The save has an 8-bit bus: wider loads see the byte repeated.
        0xE | 0xF => g.hw.save.read((a & !1) as u16) as u32 * 0x0101,
        _ => unmapped("a halfword load", a),
    }
}

pub fn ld32(g: &mut Game, a: u32) -> u32 {
    if let Some(o) = ram_at(a) {
        return (0..4).fold(0, |v, i| v | (g.ram[o + i] as u32) << (8 * i));
    }
    match a >> 24 {
        4 => {
            let o = io_at(a) & !3;
            io_read16(g, o) | io_read16(g, o + 2) << 16
        }
        5..=7 => ld16(g, a) | ld16(g, a.wrapping_add(2)) << 16,
        // ROM (the EEPROM answers halfword loads only, as the play build's).
        8..=0xD => rom16((a & 0x1FF_FFFE) as usize) | rom16((a.wrapping_add(2) & 0x1FF_FFFE) as usize) << 16,
        0xE | 0xF => g.hw.save.read((a & !3) as u16) as u32 * 0x0101_0101,
        0 if a < BIOS_SIZE => bios_read(g, a, 4),
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
        // OAM ignores byte stores; the BIOS and ROM take none.
        0 | 1 | 7..=0xD => {}
        0xE | 0xF => g.hw.save.write(a as u16, v),
        _ => unmapped("a byte store", a),
    }
}

pub fn st16(g: &mut Game, a: u32, v: u32) {
    let b = (v as u16).to_le_bytes();
    if let Some(o) = ram_at(a) {
        (g.ram[o], g.ram[o + 1]) = (b[0], b[1]);
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
        0xD if eeprom_at(g, a) => g.hw.save.eeprom_write(v as u16, 1),
        0 | 1 | 8..=0xD => {}
        // The addressed byte lane at the unaligned address, as runtime::bus;
        // mGBA 0.10.5 drops these stores (units/native-save-600.toml).
        0xE | 0xF => g.hw.save.write(a as u16, b[(a & 1) as usize]),
        _ => unmapped("a halfword store", a),
    }
}

pub fn st32(g: &mut Game, a: u32, v: u32) {
    if let Some(o) = ram_at(a) {
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            g.ram[o + i] = b;
        }
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
        0 | 1 | 8..=0xD => {}
        0xE | 0xF => g.hw.save.write(a as u16, (v >> ((a & 3) * 8)) as u8),
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

/// `BgAffineSet` (r0: 20-byte sources, r1: 16-byte destinations, r2:
/// count; `bg`) or `ObjAffineSet` (r0: 8-byte sources, r1: destination,
/// r2: count, r3: the stride between pa-pd), as the play build's: r0 and
/// r1 past the last entry, r2 -1 or one less than a negative count.
pub fn affine_set(g: &mut Game, r: &mut [u32; 4], bg: bool) {
    let h = |g: &mut Game, a: u32| ld16(g, a & !1) as i16 as i32;
    let (mut src, mut dst) = (r[0], r[1]);
    for _ in 0..(r[2] as i32).max(0) {
        if bg {
            let (ox, oy) = (ld32(g, src & !3) as i32, ld32(g, src.wrapping_add(4) & !3) as i32);
            let [cx, cy, sx, sy] = [8, 10, 12, 14].map(|o| h(g, src.wrapping_add(o)));
            let angle = h(g, src.wrapping_add(16)) as u16;
            let (m, x, y) = hw::bios::bg_affine(ox, oy, cx, cy, sx, sy, angle);
            for (k, v) in m.into_iter().enumerate() {
                st16(g, dst.wrapping_add(2 * k as u32) & !1, v as u16 as u32);
            }
            st32(g, dst.wrapping_add(8) & !3, x as u32);
            st32(g, dst.wrapping_add(12) & !3, y as u32);
            (src, dst) = (src.wrapping_add(20), dst.wrapping_add(16));
        } else {
            let [sx, sy, angle] = [0, 2, 4].map(|o| h(g, src.wrapping_add(o)));
            for v in hw::bios::obj_affine(sx, sy, angle as u16) {
                st16(g, dst & !1, v as u16 as u32);
                dst = dst.wrapping_add(r[3]);
            }
            src = src.wrapping_add(8);
        }
    }
    r[0] = src;
    r[1] = dst;
    r[2] = (r[2] as i32).min(0).wrapping_sub(1) as u32;
}

/// `CpuFastSet` (r0 source, r1 destination, r2 word count and fill), as
/// the play build's: eight words at a time, its registers as it leaves
/// them (r2 as given: the SWI dispatcher restores it), and its time.
pub fn cpu_fast_set(g: &mut Game, sp: u32, r: &mut [u32; 4]) -> Result<(), Exit> {
    if r[0] < 0x0200_0000 {
        return Ok(());
    }
    let fill = r[2] & 1 << 24 != 0;
    let cycles = hw::bios::cpu_fast_set_cycles(g.hw.waitcnt, r[0], r[1], r[2]);
    r[3] = r[2] << 12;
    let end = r[1].wrapping_add((r[2] & 0x1F_FFFF) * 4);
    if fill {
        r[3] = ld32(g, r[0] & !3);
    }
    while (r[1] as i32) < (end as i32) {
        for k in 0..8 {
            if !fill {
                let v = ld32(g, r[0] & !3);
                r[0] = r[0].wrapping_add(4);
                if k == 0 {
                    r[3] = v;
                }
                st32(g, r[1] & !3, v);
            } else {
                st32(g, r[1] & !3, r[3]);
            }
            r[1] = r[1].wrapping_add(4);
        }
    }
    g.sp = sp;
    hw::spend(g, cycles)
}

/// A decompressor's byte out at `*d`: stored (WRAM), or held until its
/// odd partner comes and the two stored as a halfword (VRAM).
fn unpack_put(g: &mut Game, d: &mut u32, half: &mut u32, b: u32, vram: bool) {
    if !vram {
        st8(g, *d, b);
    } else if *d & 1 != 0 {
        st16(g, *d ^ 1, *half | b << 8);
    } else {
        *half = b;
    }
    *d = d.wrapping_add(1);
}

/// `LZ77UnCompWram` (`vram` false) or `Vram` (r0 source, r1
/// destination), as the play build's: a back-reference reads the output
/// as it stands; r0 and r1 end past the data, r3 0; and its time, which
/// the prefetch buffer makes depend on where the `swi` is (`at`, Thumb if
/// `thumb`).
pub fn lz77(
    g: &mut Game,
    sp: u32,
    r: &mut [u32; 4],
    vram: bool,
    at: u32,
    thumb: bool,
) -> Result<(), Exit> {
    if r[0] & 0x0E00_0000 == 0 {
        return Ok(());
    }
    let w = g.hw.waitcnt;
    let cycles = hw::bios::lz77_cycles(w, at, thumb, r[0], r[1], vram, |a| ld8(g, a) as u8);
    let mut remaining = ld32(g, r[0] & !3) >> 8;
    let (mut s, mut d, mut half) = (r[0].wrapping_add(4), r[1], 0);
    'blocks: while remaining > 0 {
        let flags = ld8(g, s);
        s = s.wrapping_add(1);
        for bit in (0..8).rev() {
            if remaining == 0 {
                break 'blocks;
            }
            if flags >> bit & 1 != 0 {
                let block = ld8(g, s) << 8 | ld8(g, s.wrapping_add(1));
                s = s.wrapping_add(2);
                let mut from = d.wrapping_sub((block & 0xFFF) + 1);
                for _ in 0..(block >> 12) + 3 {
                    let b = ld8(g, from);
                    unpack_put(g, &mut d, &mut half, b, vram);
                    from = from.wrapping_add(1);
                    remaining = remaining.saturating_sub(1);
                }
            } else {
                let b = ld8(g, s);
                s = s.wrapping_add(1);
                unpack_put(g, &mut d, &mut half, b, vram);
                remaining -= 1;
            }
        }
    }
    (r[0], r[1], r[3]) = (s, d, 0);
    g.sp = sp;
    hw::spend(g, cycles)
}

/// `RLUnCompWram` (`vram` false) or `Vram` (r0 source, r1 destination),
/// as the play build's: padded with zeros to a multiple of 4 bytes; r0
/// and r1 end past the data. It takes no time (the play build's stalls
/// are the copies' and LZ77's).
pub fn rl(g: &mut Game, r: &mut [u32; 4], vram: bool) {
    if r[0] & 0x0E00_0000 == 0 {
        return;
    }
    let mut remaining = ld32(g, r[0] & !3) >> 8;
    let mut padding = (4u32.wrapping_sub(remaining) & 3) as i32;
    let (mut s, mut d, mut half) = (r[0].wrapping_add(4), r[1], 0);
    while remaining > 0 {
        let flag = ld8(g, s);
        s = s.wrapping_add(1);
        let (run, n) = match flag & 0x80 != 0 {
            true => (Some(ld8(g, s)), (flag & 0x7F) + 3),
            false => (None, flag + 1),
        };
        s = s.wrapping_add(run.is_some() as u32);
        for _ in 0..n.min(remaining) {
            let b = run.unwrap_or_else(|| ld8(g, s));
            s = s.wrapping_add(run.is_none() as u32);
            unpack_put(g, &mut d, &mut half, b, vram);
            remaining -= 1;
        }
    }
    if vram && d & 1 != 0 {
        padding -= 1;
        d = d.wrapping_add(1);
    }
    while padding > 0 {
        let n = if vram { 2 } else { 1 };
        if vram { st16(g, d, 0) } else { st8(g, d, 0) }
        padding -= n;
        d = d.wrapping_add(n as u32);
    }
    (r[0], r[1]) = (s, d);
}

/// `HuffUnComp` (r0 source, r1 destination), as the play build's
/// (`runtime::bios::decomp::huffman`): 4- or 8-bit symbols (other widths
/// do nothing), the output a word at a time; r0 and r1 end past the data.
/// It takes no time (the play build's is its entry's).
pub fn huff(g: &mut Game, r: &mut [u32; 4]) {
    let mut src = r[0] & !3;
    if src & 0x0E00_0000 == 0 {
        return;
    }
    let header = ld32(g, src);
    let mut remaining = (header >> 8) as i32;
    let width = match header & 0xF {
        0 => 8,
        w => w,
    };
    if 32 % width != 0 || width == 1 {
        return;
    }
    let root = src.wrapping_add(5);
    src = root.wrapping_add(ld8(g, src.wrapping_add(4)) << 1 | 1);
    let mut dst = r[1];
    let (mut at, mut node) = (root, ld8(g, root));
    let (mut word, mut filled) = (0u32, 0);
    while remaining > 0 {
        let bits = ld32(g, src);
        src = src.wrapping_add(4);
        for i in 0..32 {
            if remaining <= 0 {
                break;
            }
            let right = bits << i & 0x8000_0000 != 0;
            let child = (at & !1).wrapping_add((node & 0x3F) * 2 + 2) + right as u32;
            if node & if right { 0x40 } else { 0x80 } == 0 {
                at = child;
                node = ld8(g, at);
                continue;
            }
            word |= (ld8(g, child) & ((1 << width) - 1)) << filled;
            filled += width;
            (at, node) = (root, ld8(g, root));
            if filled == 32 {
                st32(g, dst, word);
                dst = dst.wrapping_add(4);
                remaining -= 4;
                (word, filled) = (0, 0);
            }
        }
    }
    (r[0], r[1]) = (src, dst);
}

/// `RegisterRamReset` (r0: what to reset), as the play build's
/// (`runtime::bios::register_ram_reset`): `DISPCNT` forced blank, then
/// each part flagged zeroed, the registers as stores so the library sees
/// them. IWRAM's top 0x200 bytes are kept. The serial registers (0x20)
/// aren't the library's (`runtime::hw` has no serial), so they stay. It
/// takes no time (the play build's is its entry's).
pub fn register_ram_reset(g: &mut Game, flags: u32) {
    st16(g, 0x0400_0000, 0x80);
    let ram = |g: &mut Game, lo: usize, hi: usize| (lo..hi).for_each(|o| g.ram[o] = 0);
    if flags & 0x01 != 0 {
        ram(g, 0, EWRAM);
    }
    if flags & 0x02 != 0 {
        ram(g, EWRAM, EWRAM + 0x7E00);
    }
    if flags & 0x04 != 0 {
        g.hw.ppu.palette.fill(0);
    }
    if flags & 0x08 != 0 {
        g.hw.ppu.vram.fill(0);
    }
    if flags & 0x10 != 0 {
        g.hw.ppu.oam.fill(0);
    }
    if flags & 0x40 != 0 {
        // The sound channels, SOUNDCNT_L/H/X and wave RAM; SOUNDBIAS.
        for o in [0x60, 0x62, 0x64, 0x68, 0x6C, 0x70, 0x72, 0x74, 0x78, 0x7C, 0x80, 0x82, 0x84] {
            st16(g, 0x0400_0000 + o, 0);
        }
        for o in (0x90..0xA0).step_by(2) {
            st16(g, 0x0400_0000 + o, 0);
        }
        st16(g, 0x0400_0088, 0x200);
    }
    if flags & 0x80 != 0 {
        // Display, DMA, timers (the affine backgrounds the identity),
        // then IE, IF (all acknowledged), WAITCNT and IME.
        let display = (0x004..=0x054).step_by(2);
        let dma = (0x0B0..0x0E0).step_by(2);
        let timers = (0x100..0x110).step_by(2);
        for o in display.chain(dma).chain(timers) {
            let identity = matches!(o, 0x20 | 0x26 | 0x30 | 0x36);
            st16(g, 0x0400_0000 + o, if identity { 0x100 } else { 0 });
        }
        for (o, v) in [(0x200, 0), (0x202, 0xFFFF), (0x204, 0), (0x208, 0)] {
            st16(g, 0x0400_0000 + o, v);
        }
    }
}

/// `SoftReset`, as the play build's (`runtime::bios::soft_reset`): IWRAM's
/// top 0x200 bytes (the stacks, the IRQ handler's pointer) cleared, then
/// the program restarts in system mode at the cartridge's entry, or at
/// EWRAM's start if the byte at `0x0300_7FFA` was non-zero, the BIOS
/// region reading as after boot. The restart is `main`'s: this returns
/// `Err(Exit)` with [`Game::reset`] set, which every frame passes up as it
/// would a stop. It takes no time.
pub fn soft_reset(g: &mut Game) -> Result<(), Exit> {
    let to_ram = ld8(g, 0x0300_7FFA) != 0;
    (EWRAM + 0x7E00..EWRAM + IWRAM).for_each(|o| g.ram[o] = 0);
    g.bios_latch = runtime::bus::BIOS_LATCH_AFTER_BOOT;
    g.reset = Some(if to_ram { 0x0200_0000 } else { 0x0800_0000 });
    Err(Exit)
}

/// `SoundBias` (r0: 0 for level 0, else 0x200), as the play build's: the
/// level (`SOUNDBIAS` bits 0-9) set at once, the upper bits kept.
pub fn sound_bias(g: &mut Game, level: u32) {
    const SOUNDBIAS: u32 = 0x0400_0000 + runtime::io::reg::SOUNDBIAS;
    let old = ld16(g, SOUNDBIAS);
    st16(g, SOUNDBIAS, old & !0x3FF | if level == 0 { 0 } else { 0x200 });
}

/// A function pointer's call: `k` is the function's key (its ROM or IWRAM
/// address, bit 0 for Thumb), the ID of every function in stage A
/// (`--fn-ids rom`).
#[inline(never)]
pub fn call_fn(g: &mut Game, k: u32, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {
    match FNS.binary_search_by_key(&k, |&(e, _)| e) {
        Ok(i) => (FNS[i].1)(g, sp, a),
        Err(_) => match flash_call(g, k, a) {
            Some(r) => Ok(r),
            None => panic!("a call or jump to {k:#010x}, which is no function's entry"),
        },
    }
}

/// A call into RAM holding one of the flash save library's stack routines
/// (known by its bytes) is its `runtime::hw::flash` call on the play
/// build's chip; the copy never runs. The routines take any memory
/// (Metroid's SRAM library verifies chip against RAM and back), so the
/// chip's side goes through `hw::flash`, the other through the bus.
/// r1-r3 come back as they went in (the callers don't read them).
fn flash_call(g: &mut Game, k: u32, a: [u32; 4]) -> Option<[u32; 4]> {
    use runtime::hw::flash::{self as fl, Routine};
    let bytes = |g: &mut Game, at: u32, n: usize| -> Vec<u8> {
        (0..n).map(|i| ld8(g, at.wrapping_add(i as u32)) as u8).collect()
    };
    matches!(k >> 24, 2 | 3).then_some(())?;
    let routine = fl::recognise(&bytes(g, k & !1, fl::MAX_LEN))?.routine;
    let chip = |x: u32| matches!(x >> 24, 0xE | 0xF);
    let (src, dst, n) = (a[0], a[1], a[2] as usize);
    let r0 = match routine {
        Routine::ReadByte if chip(src) => fl::read_byte(&g.hw.save, src) as u32,
        Routine::ReadByte => ld8(g, src),
        Routine::Read => {
            let mut b = vec![0; n];
            match chip(src) {
                true => fl::read(&g.hw.save, src, &mut b),
                false => b = bytes(g, src, n),
            }
            (0..n).for_each(|i| st8(g, dst.wrapping_add(i as u32), b[i] as u32));
            src
        }
        // 0, or `dst`'s address of the first byte that differs.
        Routine::Verify => {
            let (s, d) = (bytes(g, src, n), bytes(g, dst, n));
            let at = match (chip(src), chip(dst)) {
                (_, true) => fl::verify(&g.hw.save, &s, dst).checked_sub(dst),
                (true, false) => fl::verify(&g.hw.save, &d, src).checked_sub(src),
                _ => (0..n).position(|i| s[i] != d[i]).map(|i| i as u32),
            };
            at.map_or(0, |i| dst.wrapping_add(i))
        }
    };
    Some([r0, a[1], a[2], a[3]])
}

/// A lifted function's four-register form (`a_<key>` if shaped): sp, r0-r3 in; r0-r3 out.
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

// Tasks on host coroutines (native/task-coroutines): a scheduler that runs
// each task on a stack of its own (Kirby - Nightmare's object tasks)
// switches sp, so a task's frames stay on its stack while the scheduler
// goes on. Natively a task's lifted frames are on the host stack instead,
// so each task runs on a stackful coroutine (`corosensei`), kept while it
// waits under the guest sp it saved. The stand-ins for the switcher's
// resume, yield and exit (holes files) do the guest's stores and call
// these. The task's code keeps the `&mut Game` it started with across its
// suspensions, as crates/desktop/src/coro.rs's guest does: while a task is
// suspended only the scheduler's reference is used, and while it runs
// only the task's.

type TaskCo = corosensei::Coroutine<
    (*mut Game, [u32; 4]),
    (TaskStop, [u32; 4]),
    Result<[u32; 4], Exit>,
    corosensei::stack::DefaultStack,
>;
type TaskBack = corosensei::Yielder<(*mut Game, [u32; 4]), (TaskStop, [u32; 4])>;

/// A task coroutine's host stack: lifted frames are large.
pub const TASK_STACK: usize = 1 << 20;

/// Why a task's coroutine handed back to its resume.
enum TaskStop {
    /// [`task_yield`]: kept under this key, resumed through this yielder.
    Yield(u32, *const TaskBack),
    /// [`task_exit`]: over.
    Exit,
}

/// The tasks waiting to be resumed, by the guest sp each saved.
#[derive(Default)]
pub struct Tasks {
    waiting: std::collections::BTreeMap<u32, (TaskCo, *const TaskBack)>,
    /// The running task's way back to its resume; null outside a task.
    back: Option<*const TaskBack>,
    /// Stacks exited tasks left.
    spare: Vec<corosensei::stack::DefaultStack>,
}

impl Drop for Tasks {
    fn drop(&mut self) {
        for (co, _) in self.waiting.values_mut() {
            // SAFETY: a task's frames own nothing to drop (lifted code
            // holds integers, and `&mut Game`).
            unsafe { co.force_reset() };
        }
    }
}

/// Run the task that saved guest sp `key` until it yields or exits, or, if
/// none did, start one: function `start` (a key) on guest stack `sp` with
/// `a` in r0-r3. The task gets `a` back from its yield when one resumes it.
/// Gives what the yield or exit handed back: the registers the guest's
/// scheduler has after its call of resume.
pub fn task_resume(g: &mut Game, key: u32, start: u32, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> {
    assert!(g.tasks.back.is_none(), "a task resumed from inside a task");
    let (mut co, back) = match g.tasks.waiting.remove(&key) {
        Some((co, back)) => (co, back),
        None => {
            let stack = match g.tasks.spare.pop() {
                Some(s) => s,
                None => corosensei::stack::DefaultStack::new(TASK_STACK).expect("a task stack"),
            };
            let co = TaskCo::with_stack(stack, move |y: &TaskBack, (p, a): (*mut Game, [u32; 4])| {
                // SAFETY: the resume's game, which outlives the task (see above).
                let g = unsafe { &mut *p };
                g.tasks.back = Some(y as *const TaskBack);
                call_fn(g, start, sp, a)
            });
            (co, std::ptr::null())
        }
    };
    // A new task sets its own on its first run.
    g.tasks.back = (!back.is_null()).then_some(back);
    let r = co.resume((g as *mut Game, a));
    g.tasks.back = None;
    match r {
        corosensei::CoroutineResult::Yield((TaskStop::Yield(k, back), out)) => {
            if let Some((mut old, _)) = g.tasks.waiting.insert(k, (co, back)) {
                // A task left waiting under the same sp: its stack was reused.
                // SAFETY: as Tasks::drop's.
                unsafe { old.force_reset() };
            }
            Ok(out)
        }
        corosensei::CoroutineResult::Yield((TaskStop::Exit, out)) => {
            // SAFETY: as Tasks::drop's.
            unsafe { co.force_reset() };
            g.tasks.spare.push(co.into_stack());
            Ok(out)
        }
        corosensei::CoroutineResult::Return(Err(e)) => Err(e),
        corosensei::CoroutineResult::Return(Ok(_)) => {
            panic!("task {start:#010x} returned: its switcher has no frame to return to")
        }
    }
}

/// From inside a task: suspend it under `key` (the guest sp it saved),
/// handing `out` to its resume as the scheduler's registers. Gives the
/// registers the next [`task_resume`] of `key` passes.
pub fn task_yield(g: &mut Game, key: u32, out: [u32; 4]) -> [u32; 4] {
    let back = g.tasks.back.take().expect("a yield outside a task");
    // SAFETY: the running task's yielder, on its coroutine's stack.
    let (_, a) = unsafe { (*back).suspend((TaskStop::Yield(key, back), out)) };
    a
}

/// From inside a task: end it, handing `out` to its resume as the
/// scheduler's registers. Its frames are discarded.
pub fn task_exit(g: &mut Game, out: [u32; 4]) -> ! {
    let back = g.tasks.back.take().expect("an exit outside a task");
    // SAFETY: as task_yield's.
    unsafe { (*back).suspend((TaskStop::Exit, out)) };
    unreachable!("an exited task resumed")
}

fn main() -> std::process::ExitCode {
    native_host::main(NAME, true, Game::new, |g| {
        let mut to = None;
        loop {
            let r = match to {
                None => ENTRY(g, 0x0300_7F00, [0; 4]),
                // A `SoftReset` to EWRAM: the function there, if lifted.
                Some(k) => call_fn(g, k, 0x0300_7F00, [0; 4]),
            };
            match r {
                // A `SoftReset`: system mode, its stack and no tasks.
                Err(Exit) if g.reset.is_some() => {
                    to = g.reset.take().filter(|&k| k != 0x0800_0000);
                    (g.sp, g.cpsr, g.lr) = (0x0300_7F00, 0x1F, RET);
                    g.tasks = Tasks::default();
                }
                Err(e) => return Err(e),
                Ok(_) => panic!("the program's entry returned"),
            }
        }
    })
}
