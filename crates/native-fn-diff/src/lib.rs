//! fn-diff for native functions (`docs/native-port-design.md`, "The check
//! build", last paragraph): a function lifted to native Rust, run from
//! the play build's recorded entry states, must leave the RAM, the I/O
//! registers and the result the play build's run of the same function
//! leaves.
//!
//! - **Entry states** are `fn-diff`'s ([`fn_diff::record`]): the
//!   interpreter runs the ROM with no input and saves the machine at the
//!   first entries of each function. A function the run never enters
//!   borrows other functions' states with its pc set, as `fn-diff --hand`
//!   does. Each state then runs in [`Spec::variants`] variants: the
//!   game's [`Func::inputs`] sets the parameters and the globals the
//!   function reads from a seeded generator (variant 0 of a state the
//!   function was entered in is that state unchanged).
//! - **The reference** is the interpreter from that state to the return,
//!   untimed ([`Machine::step_untimed`]): the native rule, code takes no
//!   time, so no interrupt is taken, the beam and the timers stand still,
//!   and a read of `VCOUNT` or a timer answers what it would at the entry.
//!   The play build's generated code is checked against the interpreter
//!   instruction by instruction elsewhere (`fn-diff`, `first-frame`).
//! - **The native side** is built from the same state by the game's
//!   [`NativeGame::from_machine`]: its RAM the guest's bytes (the check
//!   build's `--layout guest`), its `Hw` from the registers. It gets the
//!   parameters the function's DWARF prototype names (its summary where
//!   there is none, [`codegen::saves::summarize`]), r0 first, and nothing
//!   else.
//! - **Compared**: EWRAM and IWRAM byte for byte except the guest stack
//!   below the entry `sp` (the native program has no stack); the display
//!   registers, palette, VRAM and OAM; each I/O register the game says it
//!   models ([`NativeGame::io`]); every other I/O register must be one the
//!   reference didn't change; and the result, where the prototype has one.
//!
//! Then a fault: one byte of the ROM changed under one function's
//! reference ([`Spec::fault`]) must make it differ.

use armv4t::interp::T_BIT;
use codegen::func::Ctx;
use codegen::names::SourceMap;
use runtime::Machine;
use runtime::dispatch;
use runtime::platform::Headless;
use runtime::video::{REGS_END, Video};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

pub type M = Machine<Headless>;

/// A native program's state, as the check builds it.
pub trait NativeGame: Sized {
    /// The program at the entry state `m`: RAM from the guest's, hardware
    /// from its registers.
    fn from_machine(m: &mut M) -> Self;
    /// EWRAM's 256 KB and IWRAM's 32 KB as the program holds them.
    fn ewram(&self) -> &[u8];
    fn iwram(&self) -> &[u8];
    /// The display as the program's `Hw` has it.
    fn video(&self) -> &Video;
    /// The other I/O registers the program models: offset, the bits that
    /// mean something, and its value, to compare with the play build's
    /// stored register.
    fn io(&self) -> Vec<(u32, u16, u16)>;
    /// Registers whose value as the CPU reads them (not as stored) the
    /// program models: offset and value (the beam's line, timer counters).
    /// Time doesn't move in a function, so these must be what the
    /// reference reads after it too.
    fn io_reads(&self) -> Vec<(u32, u16)> {
        Vec::new()
    }
}

/// One native function under check.
pub struct Func<G> {
    /// Its ROM entry, Thumb bit set.
    pub key: u32,
    pub name: &'static str,
    /// Run it with its parameters (r0 first, as many as its prototype
    /// names); its result as r0 would hold it, or 0.
    pub run: fn(&mut G, &[u32]) -> u32,
    /// Variant `i` of an entry state: the parameters (r0-r3) and the
    /// globals the function reads, drawn from the generator.
    pub inputs: fn(&mut M, &mut Rng, usize),
}

/// One byte of the ROM changed under one function's reference.
pub struct Fault {
    pub key: u32,
    /// The ROM address of the byte, and what it's XORed with.
    pub at: u32,
    pub xor: u8,
    /// What the byte is, for the report.
    pub what: &'static str,
}

/// What a game's binary hands [`main`].
pub struct Spec<G: 'static> {
    pub funcs: &'static [Func<G>],
    /// Frames of the recording run.
    pub frames: u32,
    /// Runs per entry state.
    pub variants: usize,
    pub fault: Fault,
}

/// xorshift64*, seeded per run.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    /// Uniform in `0..n`.
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }
    pub fn chance(&mut self, one_in: u32) -> bool {
        self.below(one_in) == 0
    }
}

/// Where a guest RAM address is in the machine's arrays.
fn ram_mut(m: &mut M, addr: u32) -> &mut [u8] {
    match addr >> 24 {
        2 => &mut m.bus.ewram[(addr & 0x3_FFFF) as usize..],
        3 => &mut m.bus.iwram[(addr & 0x7FFF) as usize..],
        _ => panic!("{addr:#010x} isn't RAM"),
    }
}

/// Write guest RAM directly (no bus side effects), little-endian.
pub fn poke(m: &mut M, addr: u32, bytes: &[u8]) {
    ram_mut(m, addr)[..bytes.len()].copy_from_slice(bytes);
}
pub fn poke8(m: &mut M, addr: u32, v: u8) {
    poke(m, addr, &[v]);
}
pub fn poke16(m: &mut M, addr: u32, v: u16) {
    poke(m, addr, &v.to_le_bytes());
}
pub fn poke32(m: &mut M, addr: u32, v: u32) {
    poke(m, addr, &v.to_le_bytes());
}
pub fn peek32(m: &mut M, addr: u32) -> u32 {
    u32::from_le_bytes(ram_mut(m, addr)[..4].try_into().unwrap())
}
/// Fill `n` bytes at `addr` from the generator.
pub fn scramble(m: &mut M, rng: &mut Rng, addr: u32, n: u32) {
    for i in 0..n {
        poke8(m, addr + i, rng.next_u32() as u8);
    }
}

/// [`scramble`] `n` bytes at `addr` and up to 16 on each side (inside
/// the same RAM region), so a native store that strays past its target
/// shows even where it stores zeroes.
pub fn scramble_around(m: &mut M, rng: &mut Rng, addr: u32, n: u32) {
    let (lo, hi) = match addr >> 24 {
        2 => (0x0200_0000, 0x0204_0000),
        _ => (0x0300_0000, 0x0300_8000),
    };
    let a = addr.saturating_sub(16).max(lo);
    let b = (addr + n + 16).min(hi);
    scramble(m, rng, a, b - a);
}

/// The parameters each function gets and whether a result comes back:
/// (r0-r3 mask, has a result in r0, where that came from).
fn signatures<G>(rom: &[u8], funcs: &[Func<G>]) -> BTreeMap<u32, (u8, bool, String)> {
    let sha = codegen::names::sha1_hex(rom);
    let dir = std::env::var_os("GBAA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/gbaa")
        })
        .join("names");
    let files = codegen::names::files_for(&dir, &sha).unwrap_or_default();
    let names = (!files.is_empty())
        .then(|| SourceMap::load(&sha, &files).ok())
        .flatten();
    let disc = analysis::discovery::discover(rom.to_vec());
    let ctx = Ctx::new(rom, &disc);
    let all = codegen::emit::build_all(&ctx).funcs;
    let sums = codegen::saves::summarize(&ctx, &all);
    let regs = |m: u16| -> String {
        let v: Vec<String> = (0..4)
            .filter(|r| m & 1 << r != 0)
            .map(|r| format!("r{r}"))
            .collect();
        if v.is_empty() {
            "-".into()
        } else {
            v.join(",")
        }
    };
    let mut out = BTreeMap::new();
    for f in funcs {
        let sum = sums.get(f.key).map(|s| (s.params, s.results));
        let dwarf = names.as_ref().and_then(|m| {
            let item = m.functions.get(&f.key)?;
            codegen::saves::proto_regs(m, item.proto.as_ref()?, &item.name)
        });
        let (p, r, from) = match (dwarf, sum) {
            (Some((p, r)), _) => (p, r, "DWARF prototype"),
            (None, Some((p, r))) => (p, r, "summary"),
            (None, None) => panic!("{}: no prototype and no summary", f.name),
        };
        let note = match (dwarf, sum) {
            (Some(d), Some(s)) if d != s => format!(
                "; summary differs: params {} results {}",
                regs(s.0 as u16),
                regs(s.1)
            ),
            _ => String::new(),
        };
        assert!(p & (p + 1) == 0, "{}: parameters not r0 up", f.name);
        out.insert(
            f.key,
            (
                p,
                r & 1 != 0,
                format!(
                    "params {} result {} ({from}{note})",
                    regs(p as u16),
                    regs(r & 1)
                ),
            ),
        );
    }
    out
}

/// How the reference run ended, if not by returning.
enum Stop {
    Fault(runtime::machine::Fault),
    Bios,
    RanAway,
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Stop::Fault(x) => write!(f, "a fault, {x:?}"),
            Stop::Bios => write!(f, "it ran into the BIOS"),
            Stop::RanAway => write!(f, "no return in 4,000,000 instructions"),
        }
    }
}

/// Where a run returns to: an address no function here reaches.
const SENTINEL: u32 = 0x0800_0001;

/// The interpreter from `m`'s state to the return, untimed. Returns the
/// lowest `sp` seen.
fn reference(m: &mut M) -> Result<u32, Stop> {
    let mut low = m.cpu.r[13];
    for _ in 0..4_000_000 {
        if let Some(f) = m.fault {
            return Err(Stop::Fault(f));
        }
        if dispatch::key(&m.cpu) == SENTINEL {
            return Ok(low);
        }
        if (m.cpu.pc as usize) < runtime::bus::BIOS_SIZE {
            return Err(Stop::Bios);
        }
        m.step_untimed();
        low = low.min(m.cpu.r[13]);
    }
    Err(Stop::RanAway)
}

/// The first difference between the reference's end and the native
/// program's, or `None`.
fn compare<G: NativeGame>(
    m: &mut M,
    g: &G,
    io_before: &[u16],
    stack: (u32, u32),
    result: Option<(u32, u32)>,
) -> Option<String> {
    let first = |a: &[u8], b: &[u8], base: u32, skip: (u32, u32)| {
        a.iter()
            .zip(b)
            .enumerate()
            .map(|(i, (x, y))| (base + i as u32, x, y))
            .find(|&(at, x, y)| x != y && !(skip.0..skip.1).contains(&at))
            .map(|(at, x, y)| format!("{at:#010x}: {x:#04x} vs {y:#04x} native"))
    };
    if let Some(w) = first(&m.bus.ewram, g.ewram(), 0x0200_0000, (0, 0)) {
        return Some(format!("EWRAM {w}"));
    }
    if let Some(w) = first(&m.bus.iwram, g.iwram(), 0x0300_0000, stack) {
        return Some(format!("IWRAM {w}"));
    }
    let (rv, nv) = (m.bus.video_for_draw(), g.video());
    for o in (0..REGS_END).step_by(2) {
        if rv.read16(o) != nv.read16(o) {
            return Some(format!(
                "display register {o:#05x}: {:#06x} vs {:#06x} native",
                rv.read16(o),
                nv.read16(o)
            ));
        }
    }
    for (what, a, b) in [
        ("palette", &rv.palette[..], &nv.palette[..]),
        ("VRAM", &rv.vram[..], &nv.vram[..]),
        ("OAM", &rv.oam[..], &nv.oam[..]),
    ] {
        if let Some(w) = first(a, b, 0, (0, 0)) {
            return Some(format!("{what} offset {w}"));
        }
    }
    let modelled = g.io();
    for &(o, mask, v) in &modelled {
        let r = m.bus.io.raw16(o) & mask;
        if r != v & mask {
            return Some(format!(
                "I/O {o:#05x}: {r:#06x} vs {:#06x} native",
                v & mask
            ));
        }
    }
    for (o, v) in g.io_reads() {
        let r = m.bus.io.read16(o);
        if r != v {
            return Some(format!("I/O {o:#05x} as read: {r:#06x} vs {v:#06x} native"));
        }
    }
    for o in (REGS_END..runtime::io::IO_SIZE).step_by(2) {
        let (before, after) = (io_before[o as usize / 2], m.bus.io.raw16(o));
        if before != after && !modelled.iter().any(|&(mo, _, _)| mo == o) {
            return Some(format!(
                "I/O {o:#05x} the native program doesn't model: {before:#06x} -> {after:#06x}"
            ));
        }
    }
    if let Some((r, n)) = result
        && r != n
    {
        return Some(format!("result {r:#010x} vs {n:#010x} native"));
    }
    None
}

/// One function's runs: (runs, differ, the first few differences).
struct Tally {
    runs: usize,
    differ: usize,
    shown: Vec<String>,
}

fn run_func<G: NativeGame>(
    m: &mut M,
    f: &Func<G>,
    sig: (u8, bool),
    states: &[(&fn_diff::State, bool)],
    variants: usize,
) -> Tally {
    let mut t = Tally {
        runs: 0,
        differ: 0,
        shown: Vec::new(),
    };
    for &(s, own) in states {
        for v in 0..variants {
            m.load_payload(&s.payload).expect("a payload of this build");
            m.cpu.cpsr |= T_BIT;
            dispatch::set_pc(&mut m.cpu, f.key);
            let mut rng = Rng::new((f.key as u64) << 32 ^ s.hit << 8 ^ v as u64 ^ s.entry as u64);
            if !(own && v == 0) {
                (f.inputs)(m, &mut rng, v);
            }
            m.cpu.r[14] = SENTINEL;
            let args: Vec<u32> = (0..4)
                .filter(|r| sig.0 & 1 << r != 0)
                .map(|r| m.cpu.r[r])
                .collect();
            let entry_sp = m.cpu.r[13];
            let io_before: Vec<u16> = (0..runtime::io::IO_SIZE)
                .step_by(2)
                .map(|o| m.bus.io.raw16(o))
                .collect();
            let mut g = G::from_machine(m);
            t.runs += 1;
            let what = match reference(m) {
                Err(e) => Some(format!("the reference didn't return: {e}")),
                Ok(low) => {
                    // Quiet while the native function runs: a panic there
                    // is a difference, reported below.
                    let hook = std::panic::take_hook();
                    let native = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (f.run)(&mut g, &args)
                    }));
                    std::panic::set_hook(hook);
                    match native {
                        Err(p) => Some(format!(
                            "the native function panicked: {}",
                            p.downcast_ref::<String>()
                                .map(String::as_str)
                                .or(p.downcast_ref::<&str>().copied())
                                .unwrap_or("?")
                        )),
                        Ok(n) => compare(
                            m,
                            &g,
                            &io_before,
                            (low, entry_sp),
                            sig.1.then(|| (m.cpu.r[0], n)),
                        ),
                    }
                }
            };
            if let Some(w) = what {
                t.differ += 1;
                if t.shown.len() < 3 {
                    let args: Vec<String> = args.iter().map(|a| format!("{a:#x}")).collect();
                    t.shown.push(format!(
                        "state f_{:08x} entry {}{} variant {v} ({}): {w}",
                        s.entry,
                        s.hit,
                        if own { "" } else { " (borrowed)" },
                        args.join(", ")
                    ));
                }
            }
        }
    }
    t
}

/// The binary's `main`: `native-NAME ROM`.
pub fn main<G: NativeGame>(spec: &Spec<G>) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(rom_path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: native-NAME ROM");
        return ExitCode::from(2);
    };
    let rom: &'static [u8] = std::fs::read(rom_path).expect("ROM").leak();
    let t0 = std::time::Instant::now();
    let sigs = signatures(rom, spec.funcs);
    println!(
        "native-fn-diff: {} functions; signatures in {:.0?}",
        spec.funcs.len(),
        t0.elapsed()
    );

    let picks: BTreeMap<u32, Vec<u64>> = spec
        .funcs
        .iter()
        .map(|f| (f.key, vec![0, 1, 2, 5, 17, 61, 233, 997]))
        .collect();
    let states = fn_diff::record(rom, &picks, spec.frames);
    let mut by: BTreeMap<u32, Vec<&fn_diff::State>> = BTreeMap::new();
    for s in &states {
        by.entry(s.entry).or_default().push(s);
    }
    // Lenders for the functions the run never enters: one state of each
    // function it does, up to eight.
    let lenders: Vec<&fn_diff::State> = by.values().map(|v| v[0]).take(8).collect();
    assert!(!lenders.is_empty(), "the run entered none of the functions");

    let mut m = Box::new(Machine::new(rom, Headless));
    let mut differ = 0;
    let mut failed = Vec::new();
    for f in spec.funcs {
        let (p, r, sig_text) = &sigs[&f.key];
        let own: Vec<(&fn_diff::State, bool)> = match by.get(&f.key) {
            Some(v) => v.iter().map(|s| (*s, true)).collect(),
            None => lenders.iter().map(|s| (*s, false)).collect(),
        };
        let t = run_func(&mut m, f, (*p, *r), &own, spec.variants);
        println!(
            "  {:28} {} states{}, {} runs: {}  [{sig_text}]",
            f.name,
            own.len(),
            if own.first().is_some_and(|s| !s.1) {
                " borrowed"
            } else {
                ""
            },
            t.runs,
            if t.differ == 0 {
                "same".to_string()
            } else {
                format!("{} DIFFER", t.differ)
            }
        );
        for w in &t.shown {
            println!("      {w}");
        }
        if t.differ > 0 {
            differ += 1;
            failed.push(f.name);
        }
    }
    println!("native {differ} of {} differ", spec.funcs.len());

    // The fault: one byte of the ROM changed under one function.
    let fl = &spec.fault;
    let f = spec
        .funcs
        .iter()
        .find(|f| f.key == fl.key)
        .expect("fault's function");
    let mut bad = rom.to_vec();
    bad[(fl.at - 0x0800_0000) as usize] ^= fl.xor;
    let crc = m.bus.rom_crc();
    let mut fm = Box::new(Machine::new(bad.leak(), Headless));
    fm.bus.keep_rom_crc(crc);
    let (p, r, _) = &sigs[&f.key];
    let own: Vec<(&fn_diff::State, bool)> = match by.get(&f.key) {
        Some(v) => v.iter().map(|s| (*s, true)).collect(),
        None => lenders.iter().map(|s| (*s, false)).collect(),
    };
    let t = run_func(&mut fm, f, (*p, *r), &own, spec.variants);
    let caught = t.differ > 0;
    println!(
        "fault ({} at {:#010x} ^ {:#04x}, under {}): {}, {} of {} runs differ",
        fl.what,
        fl.at,
        fl.xor,
        f.name,
        if caught { "caught" } else { "NOT caught" },
        t.differ,
        t.runs
    );
    if let Some(w) = t.shown.first() {
        println!("      {w}");
    }
    let ok = differ == 0 && caught;
    println!(
        "native-fn-diff: {} ({:.0?})",
        if ok { "pass" } else { "FAIL" },
        t0.elapsed()
    );
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
