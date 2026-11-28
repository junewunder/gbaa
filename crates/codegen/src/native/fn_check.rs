// The per-function check's driver (`gbaa-lift --fn-check KEY`, unit
// native/fn-check-any; written into the lifted crate as `src/fn_check.rs`
// by crates/codegen/src/native.rs): a replacement for one function (its
// stand-in, hand-written or an LLM's) against the reference, the
// function as the lifter prints it, both run natively in this build.
//
// The program runs headless from power-on with no input. Each checked
// function's `f_<key>` is a wrapper ([`call`]): at a call, while the
// function's budget lasts, it copies the whole state ([`Snap`]: RAM, the
// hardware but its host, the CPSR and banked stack pointers), and from it
// runs [`VARIANTS`] variants: the first as the program entered it, the
// rest with the parameters and RAM bytes the first run loaded changed by
// a generator seeded from the function's seed, the call's number and the
// variant's. Each variant runs the reference, then from the same state
// the replacement, and their ends are compared: EWRAM and IWRAM (but the
// stack below the entry sp, down to the lowest store either side made
// there), the I/O registers as stored, palette, VRAM and OAM, frames
// presented, the CPSR and sp, and the registers callers read. Then the
// state goes back and the reference runs the call for real, so the
// program stays on the lift's path.
//
// The interpreter is a second reference (unit native/fn-check-interp):
// every [`crate::FN_CHECK_INTERP`]th compared run (`gbaa-lift
// --interp-every N`; 0, never), the run's input state becomes a `Machine`
// ([`interp_load`]: RAM, I/O as stored, the display's memories, the save
// chip, the CPSR, sp and the banked sps, r0-r3) and the interpreter runs
// the ROM's code from the function's key to its return, untimed, as
// native-fn-diff's reference does; its end is compared with the lift's
// ([`compare_interp`]). It catches a wrong lift, which the replacement's
// compare can't: the lift is the replacement's reference. A run the
// interpreter can't follow (a fault, the BIOS, no return in 4,000,000
// instructions: a wait, which takes time the untimed run never gives) is
// counted, not compared. The return is pc at the sentinel's word in either
// state: `bx lr` lands in Thumb, an ARM `mov pc, lr` or `ldm …, {pc}` in
// ARM (no interworking on ARMv4T).
//
// A run whose reference panics, stops (`Exit`) or presents more than
// [`RUN_FRAMES_MAX`] frames (a variant that never returns: a random size
// that makes a DMA repeat, which the function then waits on for ever)
// isn't compared: it is counted as skipped. A replacement past the cap
// where its reference returned is a difference. What's kept is a seed, a count and a digest (of the
// reference's ends), not the states: the same seed and budget give the
// same digest while the lift and the program's path don't change.
// Generated: don't edit; rerun `just fn-check`.

use crate::*;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Runs per call of the program's: the call as it came, then variants.
pub const VARIANTS: u32 = 8;

/// The frames one run may present: past them it is stopped, as a panic
/// ([`CAPPED`]). A run that loops without letting time pass stops at
/// the prelude's `ACCESSES_MAX` instead.
pub const RUN_FRAMES_MAX: u32 = 600;

/// The start of a capped run's panic message.
const CAPPED: &str = "fn-check: past the cap";

/// The stack below the entry sp that may be the function's frames: an
/// IWRAM store this far below it or less moves the compare's floor.
const STACK_WINDOW: u32 = 0x2000;

/// One function under check, as `gbaa-lift --fn-check` prints it.
pub struct Spec {
    pub key: u32,
    pub name: &'static str,
    /// r0-r3 it reads on entry (its summary's; all four where it has none).
    pub params: u8,
    /// r0-r3 compared after it: its results and what callers read.
    pub results: u8,
    /// Of those, the ones holding only its entry lr on every return in
    /// the ROM's code (`Summary::lr_left`: `pop {r0}; bx r0`), which the
    /// lift gives back as passed (unit native/lift-frame-gone): the
    /// interpreter's end holds [`SENTINEL`] there (unit
    /// native/fn-check-interp-pop-r0).
    pub lr_left: u8,
    /// Where those came from, for the report.
    pub sig: &'static str,
    /// Where the replacement came from.
    pub replacement: &'static str,
    /// The replacement is a mod's cut (`--fn-check-cuts`, under
    /// `just lift-mod-run`): a difference from the lift is the edit,
    /// reported as such, not a failure.
    pub cut: bool,
    /// The typed globals the reference reads or writes by field (each its
    /// offset in `Game::ram` and its size, from `Ram`; not `offset_of!`,
    /// whose bare field name the names pass would take for a word in use,
    /// naming a global's constant `gTally_` where the lift has `gTally`):
    /// inputs a variant may change, with the RAM the first run loaded
    /// through `ld*`.
    pub globals: &'static [fn(&Ram) -> (usize, usize)],
    pub seed: u64,
    /// The budget: runs compared.
    pub runs: u32,
    /// The digest recorded with this seed and budget, if any.
    pub digest: Option<u64>,
}

type F = fn(&mut Game, u32, [u32; 4]) -> Result<[u32; 4], Exit>;

/// A RAM datum's type as the names files give it (the lift's
/// `FN_CHECK_RAM`, by crates/codegen/src/native/fn_check_names.rs): what a
/// differing byte is named by.
pub enum Ty {
    /// An integer or pointer of this many bytes.
    Scalar(u32),
    /// Elements, how many and each one's size.
    Array(&'static Ty, u32, u32),
    /// A struct's or union's fields, a plain one before a bitfield on
    /// the same bytes, and its size.
    Struct(&'static [Field], u32),
    /// Bytes of no type the names give.
    Bytes(u32),
}

pub struct Field {
    pub name: &'static str,
    pub off: u32,
    pub ty: &'static Ty,
}

impl Ty {
    fn size(&self) -> u32 {
        match self {
            Ty::Scalar(n) | Ty::Bytes(n) => *n,
            Ty::Array(_, n, es) => n * es,
            Ty::Struct(_, n) => *n,
        }
    }
}

/// The named place holding RAM offset `o`: the path to the deepest field
/// or element holding it (`gEquipment.maxEnergy`, `gSprites[3].pose`), the
/// RAM offset it starts at and its size when it is an integer (else 0).
fn ram_name(o: usize) -> Option<(String, usize, u32)> {
    let a = addr(o);
    let i = crate::FN_CHECK_RAM.partition_point(|e| e.0 <= a).checked_sub(1)?;
    let (lo, name, mut ty) = crate::FN_CHECK_RAM[i];
    let mut off = a - lo;
    if off >= ty.size().max(1) {
        return None;
    }
    let mut path = name.to_string();
    loop {
        match ty {
            Ty::Scalar(n) => return Some((path, o - off as usize, *n)),
            Ty::Bytes(_) => {
                if off > 0 {
                    path += &format!("+{off:#x}");
                }
                return Some((path, o, 0));
            }
            Ty::Array(e, n, es) => {
                let i = off / (*es).max(1);
                if i >= *n {
                    return Some((path, o, 0));
                }
                path += &format!("[{i}]");
                off -= i * es;
                ty = e;
            }
            Ty::Struct(fs, _) => {
                let Some(f) = fs
                    .iter()
                    .find(|f| f.off <= off && off < f.off + f.ty.size().max(1))
                else {
                    path += &format!("+{off:#x}");
                    return Some((path, o, 0));
                };
                path += &format!(".{}", f.name);
                off -= f.off;
                ty = f.ty;
            }
        }
    }
}

/// What the two sides of a compare are called: the check's own words, or
/// for a mod's cut the lift and the cut.
fn sides(cut: bool) -> (&'static str, &'static str) {
    if cut {
        ("lift", "cut")
    } else {
        ("reference", "replacement")
    }
}

#[derive(Default)]
struct Tally {
    calls: u64,
    entries: u32,
    runs: u32,
    differ: u32,
    ref_panics: u32,
    ref_exits: u32,
    ref_capped: u32,
    shown: Vec<String>,
    digest: u64,
    time: Duration,
    /// Runs the interpreter also ran, those whose end differed from the
    /// lift's, and those it couldn't follow (with the first reason).
    interp_runs: u32,
    interp_differ: u32,
    interp_skipped: u32,
    interp_why: Option<String>,
    interp_shown: Vec<String>,
}

const FNV: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        h = (h ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(0x100_0000_01b3);
    }
    for &b in chunks.remainder() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

static TALLIES: Mutex<Vec<(u32, Tally)>> = Mutex::new(Vec::new());
static START: Mutex<Option<Instant>> = Mutex::new(None);
/// The frame by which every budget was spent, once it was
/// (`GBAA_FN_CHECK_RUN_ON` runs on past it).
static SPENT_AT: Mutex<Option<u64>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Off,
    Reference,
    Replacement,
}

thread_local! {
    /// Which side a run is on: a call of a checked function inside it (its
    /// own recursion, another checked function) runs that side's code.
    static SIDE: std::cell::Cell<Side> = const { std::cell::Cell::new(Side::Off) };
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        ((self.next() as u64 * n as u64) >> 32) as u32
    }
}

/// The program's state, all a function can change.
struct Snap {
    ram: Box<[u8]>,
    hw: Box<runtime::hw::Checkpoint<Game>>,
    io: Vec<u16>,
    regs: [u32; 9],
    reset: Option<u32>,
}

impl Snap {
    fn take(g: &Game) -> Snap {
        Snap {
            ram: g.ram[..].into(),
            hw: Box::new(g.hw.checkpoint()),
            io: (0..0x400).step_by(2).map(|o| g.hw.raw16(o)).collect(),
            regs: [
                g.sp,
                g.cpsr,
                g.banked[0],
                g.banked[1],
                g.banked[2],
                g.spsr,
                g.lr,
                g.bios_latch,
                0,
            ],
            reset: g.reset,
        }
    }

    fn put(&self, g: &mut Game) {
        g.ram[..].copy_from_slice(&self.ram);
        g.hw.rewind(&self.hw);
        let r = self.regs;
        (g.sp, g.cpsr, g.banked, g.spsr, g.lr, g.bios_latch) =
            (r[0], r[1], [r[2], r[3], r[4]], r[5], r[6], r[7]);
        g.reset = self.reset;
    }
}

/// Where a RAM offset is in the guest.
fn addr(o: usize) -> u32 {
    if o < EWRAM {
        0x0200_0000 + o as u32
    } else {
        0x0300_0000 + (o - EWRAM) as u32
    }
}

/// The first difference between the reference's end (`r`, `out`) and the
/// replacement's (`g`, `new`), skipping RAM offsets `skip`.
fn compare(
    r: &Snap,
    out: [u32; 4],
    g: &Game,
    new: [u32; 4],
    results: u8,
    skip: (usize, usize),
    cut: bool,
) -> Option<String> {
    let (rw, nw) = sides(cut);
    let ram = &g.ram[..];
    if let Some(o) = (0..ram.len()).find(|&o| ram[o] != r.ram[o] && !(skip.0..skip.1).contains(&o))
    {
        let region = if o < EWRAM { "EWRAM" } else { "IWRAM" };
        // Named by field where the names files type it, an integer
        // field's value whole and in decimal.
        let int = |m: &[u8], lo: usize, n: u32| {
            m[lo..lo + n as usize]
                .iter()
                .rev()
                .fold(0u32, |v, &b| v << 8 | b as u32)
        };
        return Some(match ram_name(o) {
            Some((path, lo, n @ (1 | 2 | 4))) if lo + n as usize <= ram.len() => format!(
                "{region} {:#010x} {path} (u{}): {} {rw}, {} {nw}",
                addr(lo),
                n * 8,
                int(&r.ram, lo, n),
                int(ram, lo, n)
            ),
            Some((path, _, _)) => format!(
                "{region} byte {:#010x} ({path}): {} {rw}, {} {nw}",
                addr(o),
                r.ram[o],
                ram[o]
            ),
            None => format!(
                "{region} byte {:#010x}: {:#04x} {rw}, {:#04x} {nw}",
                addr(o),
                r.ram[o],
                ram[o]
            ),
        });
    }
    for (i, o) in (0..0x400u32).step_by(2).enumerate() {
        let v = g.hw.raw16(o);
        if v != r.io[i] {
            return Some(format!(
                "I/O register {o:#05x}: {:#06x} {rw}, {v:#06x} {nw}",
                r.io[i]
            ));
        }
    }
    for (what, a, b) in [
        ("palette", &r.hw.ppu().palette[..], &g.hw.ppu.palette[..]),
        ("VRAM", &r.hw.ppu().vram[..], &g.hw.ppu.vram[..]),
        ("OAM", &r.hw.ppu().oam[..], &g.hw.ppu.oam[..]),
    ] {
        if let Some(o) = (0..a.len()).find(|&o| a[o] != b[o]) {
            return Some(format!(
                "{what} byte {o:#x}: {:#04x} {rw}, {:#04x} {nw}",
                a[o], b[o]
            ));
        }
    }
    for (i, what, v) in [(0, "sp (system mode's)", g.sp), (1, "CPSR", g.cpsr)] {
        if v != r.regs[i] {
            return Some(format!(
                "{what}: {:#010x} {rw}, {v:#010x} {nw}",
                r.regs[i]
            ));
        }
    }
    if r.regs[8] != g.hw.frames() as u32 {
        return Some(format!(
            "frames presented: {} {rw}, {} {nw}",
            r.regs[8],
            g.hw.frames()
        ));
    }
    for n in (0..4).filter(|n| results & 1 << n != 0) {
        if out[n] != new[n] {
            return Some(format!(
                "r{n} on return: {:#010x} {rw}, {:#010x} {nw}",
                out[n], new[n]
            ));
        }
    }
    None
}

/// Change the inputs of a variant: some parameters, and some RAM bytes
/// the first run loaded (`loads`, offsets).
fn vary(g: &mut Game, rng: &mut Rng, args: &mut [u32; 4], params: u8, loads: &[u32]) {
    for (n, a) in args.iter_mut().enumerate() {
        if params & 1 << n == 0 || rng.below(2) == 0 {
            continue;
        }
        // A ROM, I/O or video address stays: a wrong one only panics.
        if matches!(*a >> 24, 4..=0xD) {
            continue;
        }
        let ram = matches!(*a >> 24, 2 | 3);
        *a = match (ram, rng.below(5)) {
            (true, 0..=2) => *a,
            (true, _) => a.wrapping_add(rng.below(9) * 4).wrapping_sub(16),
            (false, 0) => 0,
            (false, 1) => rng.below(256),
            (false, 2) => *a ^ 1 << rng.below(32),
            (false, 3) => a.wrapping_add(rng.below(17)).wrapping_sub(8),
            (false, _) => rng.next(),
        };
    }
    if loads.is_empty() {
        return;
    }
    for _ in 0..1 + rng.below(4) {
        let o = loads[rng.below(loads.len() as u32) as usize] as usize;
        let b = g.ram[o];
        g.ram[o] = match rng.below(3) {
            0 => rng.next() as u8,
            1 => b ^ 1 << rng.below(8),
            _ => b.wrapping_add(if rng.below(2) == 0 { 1 } else { 0xFF }),
        };
    }
}

/// The host while a run runs: the keys held as they were, and the frames
/// presented, which stop the run past [`RUN_FRAMES_MAX`].
struct Quiet(u16, u32);

impl runtime::hw::Host for Quiet {
    fn present(&mut self, _: &runtime::platform::Frame) -> Result<(), Exit> {
        self.1 += 1;
        if self.1 > RUN_FRAMES_MAX {
            panic!("{CAPPED} of {RUN_FRAMES_MAX} frames in one run");
        }
        Ok(())
    }
    fn keys(&mut self) -> u16 {
        self.0
    }
}

/// Run `f` on `side`, noting its RAM accesses; a panic is caught, quietly.
/// `window`: the RAM offsets whose lowest store is noted (the stack below
/// the entry sp).
fn run(
    g: &mut Game,
    f: F,
    side: Side,
    sp: u32,
    a: [u32; 4],
    window: (u32, u32),
) -> (Result<Result<[u32; 4], Exit>, String>, Accesses) {
    ACCESSES.with_borrow_mut(|x| {
        *x = Some(Accesses {
            loads: Vec::new(),
            lowest_store: window.1,
            window,
            count: 0,
        })
    });
    SIDE.set(side);
    // A host that shows, plays and saves nothing: a run that lets time
    // pass (a wait, a spin) presents frames, which the program's host
    // would count, record and hash; the state goes back, the host can't.
    let quiet = native_host::AnyHost(Box::new(Quiet(g.hw.keys.held(), 0)));
    let host = std::mem::replace(&mut g.hw.host, quiet);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(g, sp, a)));
    std::panic::set_hook(hook);
    g.hw.host = host;
    SIDE.set(Side::Off);
    let acc = ACCESSES.with_borrow_mut(|x| x.take()).unwrap_or_default();
    let r = r.map_err(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or(p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "?".into())
    });
    (r, acc)
}

/// The interpreter, the second reference.
type M = runtime::Machine<runtime::platform::Headless>;

/// Where the interpreter's run returns to: no function's address (a
/// Thumb key in the ROM header's entry branch).
const SENTINEL: u32 = 0x0800_0001;

/// The interpreter, made once ([`main`], on the program's own stack: a
/// `Machine` is too big for a task's).
static INTERP: Mutex<Option<Box<M>>> = Mutex::new(None);

/// The machine at `g`'s state, about to run function `key` from `sp` with
/// `a` in r0-r3 (the rest zero) and lr the [`SENTINEL`].
fn interp_load(m: &mut M, g: &Game, key: u32, sp: u32, a: [u32; 4]) {
    use armv4t::interp::{Bank, T_BIT};
    m.bus.ewram.copy_from_slice(&g.ram[..EWRAM]);
    m.bus.iwram.copy_from_slice(&g.ram[EWRAM..EWRAM + IWRAM]);
    m.bus.video.clone_from(&g.hw.ppu);
    for o in (0..runtime::io::IO_SIZE).step_by(2) {
        m.bus.io.set_raw16(o, g.hw.raw16(o));
    }
    m.bus.waitcnt_written();
    m.bus.save.clone_from(&g.hw.save);
    m.bus.bios_latch = g.bios_latch;
    m.bios = Default::default();
    m.fault = None;
    let c = &mut m.cpu;
    c.r = [0; 15];
    c.r[..4].copy_from_slice(&a);
    c.r[13] = sp;
    c.r[14] = SENTINEL;
    c.cpsr = g.cpsr & 0xDF | 0x10 | if key & 1 != 0 { T_BIT } else { 0 };
    c.banks = [[0; 7]; 6];
    c.banks[Bank::User as usize][5] = g.sp;
    c.banks[Bank::Irq as usize][5] = g.banked[0];
    c.banks[Bank::Svc as usize][5] = g.banked[1];
    for b in [Bank::Fiq, Bank::Abt, Bank::Und] {
        c.banks[b as usize][5] = g.banked[2];
    }
    c.spsr = [0; 6];
    c.spsr[Bank::Irq as usize] = g.spsr;
    runtime::dispatch::set_pc(c, key);
}

/// The interpreter from [`interp_load`]'s state to the return, untimed:
/// the lowest sp it saw, or why it didn't return.
fn interp_run(m: &mut M) -> Result<u32, String> {
    let mut low = m.cpu.r[13];
    for _ in 0..4_000_000 {
        if let Some(f) = m.fault {
            return Err(format!("a fault ({f:?})"));
        }
        // Either state: `bx lr` lands in Thumb at the sentinel, an ARM
        // `mov pc, lr` or `ldm …, {pc}` (no interworking on ARMv4T) in
        // ARM at its word.
        if m.cpu.pc & !3 == SENTINEL & !3 {
            return Ok(low);
        }
        if (m.cpu.pc as usize) < runtime::bus::BIOS_SIZE {
            return Err(format!("a jump into the BIOS at {:#x}", m.cpu.pc));
        }
        m.step_untimed();
        low = low.min(m.cpu.r[13]);
    }
    Err("no return in 4,000,000 instructions".into())
}

/// The first difference between the lift's end (`r`, `out`; `input` the
/// state both started from) and the interpreter's (`m`), skipping RAM
/// offsets `skip`. An I/O register is compared where either side changed
/// it (`DISPSTAT`'s and `VCOUNT`'s beam bits aside); a register the lift
/// returns its `lr` in holds [`SENTINEL`] in the interpreter, `RET` in
/// the lift; one of `lr_left` ([`Spec::lr_left`], among the `results`
/// compared: the ROM's code leaves its return address there, the lift
/// the caller's value) must hold [`SENTINEL`] in the interpreter,
/// whatever the lift's.
fn compare_interp(
    m: &M,
    input: &Snap,
    r: &Snap,
    out: [u32; 4],
    sp: u32,
    (results, lr_left): (u8, u8),
    skip: (usize, usize),
) -> Option<String> {
    let byte = |o: usize| {
        if o < EWRAM {
            m.bus.ewram[o]
        } else {
            m.bus.iwram[o - EWRAM]
        }
    };
    if let Some(o) =
        (0..EWRAM + IWRAM).find(|&o| byte(o) != r.ram[o] && !(skip.0..skip.1).contains(&o))
    {
        let region = if o < EWRAM { "EWRAM" } else { "IWRAM" };
        return Some(format!(
            "{region} byte {:#010x}: {:#04x} lift, {:#04x} interpreter",
            addr(o),
            r.ram[o],
            byte(o)
        ));
    }
    for (i, o) in (0..0x400u32).step_by(2).enumerate() {
        if o == 0x004 || o == 0x006 {
            continue;
        }
        let (before, lift, int) = (input.io[i], r.io[i], m.bus.io.raw16(o));
        if lift != int && (lift != before || int != before) {
            return Some(format!(
                "I/O register {o:#05x}: {before:#06x} before, {lift:#06x} lift, {int:#06x} interpreter"
            ));
        }
    }
    let v = &m.bus.video;
    for (what, a, b) in [
        ("palette", &r.hw.ppu().palette[..], &v.palette[..]),
        ("VRAM", &r.hw.ppu().vram[..], &v.vram[..]),
        ("OAM", &r.hw.ppu().oam[..], &v.oam[..]),
    ] {
        if let Some(o) = (0..a.len()).find(|&o| a[o] != b[o]) {
            return Some(format!(
                "{what} byte {o:#x}: {:#04x} lift, {:#04x} interpreter",
                a[o], b[o]
            ));
        }
    }
    if m.cpu.r[13] != sp {
        return Some(format!(
            "sp on return: {sp:#010x} entry (the lift's), {:#010x} interpreter",
            m.cpu.r[13]
        ));
    }
    let (lc, ic) = (r.regs[1] & 0xDF, m.cpu.cpsr & 0xDF);
    if lc != ic {
        return Some(format!(
            "CPSR's control bits: {lc:#04x} lift, {ic:#04x} interpreter"
        ));
    }
    for n in (0..4).filter(|n| results & 1 << n != 0) {
        if lr_left & 1 << n != 0 {
            if m.cpu.r[n] != SENTINEL {
                return Some(format!(
                    "r{n} on return: {:#010x} interpreter, not the return address the lift gives back as passed",
                    m.cpu.r[n]
                ));
            }
            continue;
        }
        let iv = match m.cpu.r[n] {
            SENTINEL => RET,
            v => v,
        };
        if out[n] != iv {
            return Some(format!(
                "r{n} on return: {:#010x} lift, {:#010x} interpreter",
                out[n], m.cpu.r[n]
            ));
        }
    }
    None
}

/// A checked function's `f_<key>`: the check at the program's calls while
/// its budget lasts, and the reference's run of the call.
pub fn call(
    g: &mut Game,
    sp: u32,
    a: [u32; 4],
    s: &'static Spec,
    reference: F,
    replacement: F,
) -> Result<[u32; 4], Exit> {
    match SIDE.get() {
        Side::Reference => return reference(g, sp, a),
        Side::Replacement => return replacement(g, sp, a),
        Side::Off => {}
    }
    // `GBAA_FN_CHECK_OFF=1`: the program on the reference alone, no
    // checks, to see that checking doesn't move its path.
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *OFF.get_or_init(|| std::env::var_os("GBAA_FN_CHECK_OFF").is_some()) {
        return reference(g, sp, a);
    }
    let (entry, spent) = {
        let mut ts = TALLIES.lock().unwrap();
        let t = tally(&mut ts, s.key);
        t.calls += 1;
        (t.entries, t.runs >= s.runs)
    };
    if spent {
        return reference(g, sp, a);
    }
    let t0 = Instant::now();
    START.lock().unwrap().get_or_insert(t0);
    let base = Snap::take(g);
    let mut loads: Vec<u32> = Vec::new();
    let mut local = Tally::default();
    let floor = (EWRAM as u32 + (sp.wrapping_sub(STACK_WINDOW) & 0x7FFF)).max(EWRAM as u32);
    let sp_off = EWRAM + (sp & 0x7FFF) as usize;
    let mut left = s.runs
        - TALLIES
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.0 == s.key)
            .map_or(0, |t| t.1.runs);
    for v in 0..VARIANTS {
        if left == 0 {
            break;
        }
        base.put(g);
        let mut args = a;
        if v > 0 {
            if s.params == 0 && loads.is_empty() {
                break;
            }
            let mut rng = Rng::new(s.seed ^ (s.key as u64) << 32 ^ (entry as u64) << 8 ^ v as u64);
            vary(g, &mut rng, &mut args, s.params, &loads);
        }
        let input = Snap::take(g);
        let (r, acc) = run(
            g,
            reference,
            Side::Reference,
            sp,
            args,
            (floor, sp_off as u32),
        );
        if v == 0 {
            loads = acc.loads;
            for at in s.globals {
                let (o, n) = at(&g.ram);
                loads.extend((o..o + n).map(|o| o as u32));
            }
            loads.retain(|&o| !(floor as usize..sp_off).contains(&(o as usize)));
            loads.sort_unstable();
            loads.dedup();
        }
        let out = match r {
            Err(p) if p.starts_with(CAPPED) => {
                local.ref_capped += 1;
                continue;
            }
            Err(_) => {
                local.ref_panics += 1;
                continue;
            }
            Ok(Err(Exit)) => {
                local.ref_exits += 1;
                continue;
            }
            Ok(Ok(out)) => out,
        };
        let mut after = Snap::take(g);
        after.regs[8] = g.hw.frames() as u32;
        input.put(g);
        // The interpreter, every FN_CHECK_INTERP'th compared run: from the
        // same input, its end against the lift's.
        let nth = s.runs - left;
        if crate::FN_CHECK_INTERP > 0 && nth % crate::FN_CHECK_INTERP == 0 {
            let mut im = INTERP.lock().unwrap();
            let m = im.as_mut().expect("the interpreter, made in main");
            interp_load(m, g, s.key, sp, args);
            match interp_run(m) {
                Err(why) => {
                    local.interp_skipped += 1;
                    local.interp_why.get_or_insert(why);
                }
                Ok(low) => {
                    local.interp_runs += 1;
                    let low = if low >> 24 == 3 {
                        EWRAM + (low & 0x7FFF) as usize
                    } else {
                        sp_off
                    };
                    let skip = (low.min(acc.lowest_store as usize), sp_off);
                    let regs = (s.results, s.lr_left);
                    if let Some(w) = compare_interp(m, &input, &after, out, sp, regs, skip) {
                        local.interp_differ += 1;
                        if local.interp_shown.len() < 3 {
                            let args: Vec<String> = (0..4)
                                .filter(|n| s.params & 1 << n != 0)
                                .map(|n| format!("r{n}={:#x}", args[n]))
                                .collect();
                            local.interp_shown.push(format!(
                                "call {entry} variant {v} ({}): {w}",
                                args.join(" ")
                            ));
                        }
                    }
                }
            }
        }
        let (n, nacc) = run(
            g,
            replacement,
            Side::Replacement,
            sp,
            args,
            (floor, sp_off as u32),
        );
        // The stack the reference's frames reached is its own, and the
        // replacement's: neither is compared.
        let skip = (acc.lowest_store.min(nacc.lowest_store) as usize, sp_off);
        let ref_low = acc.lowest_store as usize;
        local.runs += 1;
        left -= 1;
        let mut h = fnv(local.digest ^ FNV, &after.ram[..ref_low]);
        h = fnv(h, &after.ram[sp_off..]);
        let io: Vec<u8> = after.io.iter().flat_map(|v| v.to_le_bytes()).collect();
        h = fnv(h, &io);
        for (i, w) in out.iter().enumerate() {
            if s.results & 1 << i != 0 {
                h = fnv(h, &w.to_le_bytes());
            }
        }
        local.digest = h;
        let what = match n {
            Err(p) => Some(format!("the {} panicked: {p}", sides(s.cut).1)),
            Ok(Err(Exit)) => Some(format!(
                "the {} stopped the program (Exit) where the {} returned",
                sides(s.cut).1,
                sides(s.cut).0
            )),
            Ok(Ok(new)) => compare(&after, out, g, new, s.results, skip, s.cut),
        };
        if let Some(w) = what {
            local.differ += 1;
            if local.shown.len() < 3 {
                let args: Vec<String> = (0..4)
                    .filter(|n| s.params & 1 << n != 0)
                    .map(|n| format!("r{n}={:#x}", args[n]))
                    .collect();
                local.shown.push(format!(
                    "call {entry} variant {v} ({}): {w}",
                    args.join(" ")
                ));
            }
        }
    }
    base.put(g);
    let all_spent = {
        let mut ts = TALLIES.lock().unwrap();
        let t = tally(&mut ts, s.key);
        t.entries += 1;
        t.runs += local.runs;
        t.differ += local.differ;
        t.ref_panics += local.ref_panics;
        t.ref_exits += local.ref_exits;
        t.ref_capped += local.ref_capped;
        let room = 3usize.saturating_sub(t.shown.len());
        t.shown.extend(local.shown.into_iter().take(room));
        t.interp_runs += local.interp_runs;
        t.interp_differ += local.interp_differ;
        t.interp_skipped += local.interp_skipped;
        if t.interp_why.is_none() {
            t.interp_why = local.interp_why;
        }
        let room = 3usize.saturating_sub(t.interp_shown.len());
        t.interp_shown
            .extend(local.interp_shown.into_iter().take(room));
        t.digest = fnv(t.digest ^ FNV, &local.digest.to_le_bytes());
        t.time += t0.elapsed();
        FN_CHECKS
            .iter()
            .all(|c| ts.iter().any(|t| t.0 == c.key && t.1.runs >= c.runs))
    };
    // `GBAA_FN_CHECK_RUN_ON=1`: on to the program's end (its frames or
    // its stop), to compare where it ends with `GBAA_FN_CHECK_OFF`'s.
    if all_spent {
        let frames = *SPENT_AT.lock().unwrap().get_or_insert(g.hw.frames());
        if std::env::var_os("GBAA_FN_CHECK_RUN_ON").is_none() {
            std::process::exit(report(Ok(frames)));
        }
    }
    reference(g, sp, a)
}

fn tally(ts: &mut Vec<(u32, Tally)>, key: u32) -> &mut Tally {
    let i = match ts.iter().position(|t| t.0 == key) {
        Some(i) => i,
        None => {
            ts.push((key, Tally::default()));
            ts.len() - 1
        }
    };
    &mut ts[i].1
}

/// Print each function's line and the verdict; the exit code. `end`:
/// the frame by which every budget was spent, or how the program ended
/// short of them (its stop's message, if it stopped on a panic).
fn report(end: Result<u64, Option<String>>) -> i32 {
    let ts = TALLIES.lock().unwrap();
    let mut ok = true;
    match &end {
        Ok(n) => println!(
            "fn-check: {} functions, every budget spent in {n} frames",
            FN_CHECKS.len()
        ),
        Err(_) => println!(
            "fn-check: {} functions; the program stopped",
            FN_CHECKS.len()
        ),
    }
    // A cut's difference from its lift is the edit (under lift-mod-run).
    let mut edits = 0;
    for s in FN_CHECKS {
        let none = Tally::default();
        let t = ts.iter().find(|t| t.0 == s.key).map_or(&none, |t| &t.1);
        let verdict = if t.differ > 0 && s.cut && t.interp_differ == 0 {
            edits += 1;
            format!("differs from its lift (the edit) in {} runs", t.differ)
        } else if t.differ > 0 {
            ok = false;
            format!("{} DIFFER", t.differ)
        } else if t.interp_differ > 0 {
            ok = false;
            format!("the lift and the interpreter DIFFER in {}", t.interp_differ)
        } else if t.runs == 0 {
            ok = false;
            "NOT RUN (never called, or every run's reference stopped)".to_string()
        } else if t.runs < s.runs && s.cut {
            // A cut called less often than its budget: what ran is the
            // report, the same as its lift.
            format!("the same as its lift, in {} runs", t.runs)
        } else if t.runs < s.runs {
            ok = false;
            format!("short of its budget ({} runs)", t.runs)
        } else {
            "same".to_string()
        };
        let digest = match s.digest {
            Some(d) if t.runs >= s.runs && d != t.digest => {
                format!(" (recorded {d:016x}: the reference's ends changed)")
            }
            Some(_) if t.runs >= s.runs => " (as recorded)".to_string(),
            _ => String::new(),
        };
        println!(
            "  {} ({:#010x}): {} runs of budget {} from {} of {} calls in {:.2?}: {verdict}; seed {} digest {:016x}{digest}",
            s.name, s.key, t.runs, s.runs, t.entries, t.calls, t.time, s.seed, t.digest
        );
        println!("      replacement: {}; {}", s.replacement, s.sig);
        if t.ref_panics + t.ref_exits + t.ref_capped > 0 {
            println!(
                "      skipped: {} runs whose reference panicked, {} that stopped the program, {} capped (past {RUN_FRAMES_MAX} frames)",
                t.ref_panics, t.ref_exits, t.ref_capped
            );
        }
        for w in &t.shown {
            println!("      {w}");
        }
        if crate::FN_CHECK_INTERP > 0 {
            // No run compared is no agreement: say so.
            let agreed = if t.interp_runs == 0 {
                "none compared".to_string()
            } else if t.interp_differ == 0 {
                "agreed with the lift".to_string()
            } else {
                format!("{} DIFFER with the lift", t.interp_differ)
            };
            let skipped = match &t.interp_why {
                Some(why) if t.interp_skipped > 0 => format!(
                    "; {} it couldn't follow (first: {why})",
                    t.interp_skipped
                ),
                _ => String::new(),
            };
            println!(
                "      interpreter (every {}th run, untimed): {} runs also checked: {agreed}{skipped}",
                crate::FN_CHECK_INTERP,
                t.interp_runs
            );
            for w in &t.interp_shown {
                println!("      {w}");
            }
        }
    }
    let wall = START.lock().unwrap().map(|t| t.elapsed());
    let wall = wall.map_or(String::new(), |w| format!(" ({w:.2?} since the first check)"));
    // The program's own stop, apart from the checks: what the lift can't
    // run yet, after the runs above.
    if let Err(stop) = &end {
        println!(
            "fn-check: the program stopped before every budget was spent, after the runs above ({})",
            stop.as_deref().unwrap_or("its frames ran out")
        );
    }
    let cuts = FN_CHECKS.iter().filter(|s| s.cut).count();
    if ok && cuts == FN_CHECKS.len() {
        // Every function a mod's cut (lift-mod-run): the edit is the news.
        println!(
            "fn-check: {edits} of {cuts} cuts differ from their lifts (the edit){wall}"
        );
        return if end.is_ok() { 0 } else { 1 };
    }
    println!("fn-check: {}{wall}", if ok { "pass" } else { "FAIL" });
    if ok { 0 } else { 1 }
}

/// The program run headless (`play`, the lift's own main) under the
/// checks; exits with the report when every budget is spent, or here when
/// the program ends first (or after, under `GBAA_FN_CHECK_RUN_ON`).
pub fn main(play: fn() -> std::process::ExitCode) -> std::process::ExitCode {
    if crate::FN_CHECK_INTERP > 0 {
        *INTERP.lock().unwrap() = Some(Box::new(M::new(ROM, runtime::platform::Headless)));
    }
    // A stop (a panic: what the lift can't run yet) ends the program as
    // the frames running out does; the report says what was checked.
    let stop = std::panic::catch_unwind(play).err().map(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "a panic".into())
    });
    // Every budget spent (`GBAA_FN_CHECK_RUN_ON` ran on past it): the
    // report's verdict is the exit code. Otherwise the program stopped
    // short of the budgets, a failure whatever the runs so far showed.
    let spent = *SPENT_AT.lock().unwrap();
    match spent {
        Some(frames) => std::process::ExitCode::from(report(Ok(frames)) as u8),
        None => std::process::ExitCode::from(report(Err(stop)).max(1) as u8),
    }
}
