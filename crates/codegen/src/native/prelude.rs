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

/// The IRQ-mode stack pointer a handler starts on: the BIOS's IRQ stack
/// (0x0300_7FA0) less the six registers its stub pushes.
pub const IRQ_SP: u32 = 0x0300_7F88;

/// The ROM, for its data (section 8: bytes where nothing is typed yet).
pub static ROM: &[u8] = include_bytes!("../rom.gba");

/// The whole program's state.
pub struct Game {
    /// EWRAM then IWRAM: the typed globals of [`Ram`], and bytes.
    pub ram: Box<Ram>,
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
    /// The return address of the last call of a stand-in that reads it
    /// (`lr = true` in its holes file): the caller's `bl` sets it.
    pub lr: u32,
    /// The tasks a game's task switcher runs ([`task_resume`]).
    pub tasks: Tasks,
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
            ram: Ram::zeroed(),
            hw: Hw::new(host),
            sp: 0x0300_7F00,
            cpsr: 0x1F,
            lr: RET,
            tasks: Tasks::default(),
        });
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
        return g.ram[o] as u32 | (g.ram[o + 1] as u32) << 8;
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
        return (0..4).fold(0, |v, i| v | (g.ram[o + i] as u32) << (8 * i));
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
        8..=0xD => {}
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
        Err(_) => panic!("a call or jump to {k:#010x}, which is no function's entry"),
    }
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
        let r = ENTRY(g, 0x0300_7F00, [0; 4]);
        match r {
            Err(e) => Err(e),
            Ok(_) => panic!("the program's entry returned"),
        }
    })
}
