//! `gbaa-softfloat-check ROM [--cases N] [--pins]`: the runtime's stand-ins
//! for libgcc's `__muldf3` and `__divdf3` (`runtime::softfloat`) against
//! the game's own copies run in the interpreter (`analysis::softfloat::scan`
//! finds them): every pair of a list of edge values (zeros, denormals, the
//! normal range's ends, infinities, quiet and signalling NaNs with
//! payloads, values a rounding tie or an underflow turns on), then N
//! random pairs of each kind (any bits; results near the denormal range;
//! near overflow; short mantissas, whose products and quotients are exact
//! or ties). Prints each copy's count of differences and the first few;
//! fails unless every copy has none. `--pins` prints the edge grid's
//! results as `runtime::softfloat`'s test table pins them.

use analysis::softfloat::{Found, Routine, scan};
use armv4t::interp::T_BIT;
use runtime::Machine;
use runtime::dispatch;
use runtime::platform::Headless;
use std::process::ExitCode;

/// Where a run returns to: a Thumb key nothing here reaches.
const SENTINEL: u32 = 0x0800_0001;

/// The edge values, as bits.
const EDGES: &[u64] = &[
    0x0000_0000_0000_0000, // 0
    0x8000_0000_0000_0000, // -0
    0x0000_0000_0000_0001, // the least denormal
    0x8000_0000_0000_0003,
    0x0008_0000_0000_0000, // 2^-1023
    0x000f_ffff_ffff_ffff, // the greatest denormal
    0x0010_0000_0000_0000, // the least normal
    0x0018_0000_0000_0001,
    0x3cb0_0000_0000_0000, // 2^-52
    0x3fd5_5555_5555_5555, // 1/3
    0x3fb9_9999_9999_999a, // 0.1
    0x3ff0_0000_0000_0000, // 1
    0xbff0_0000_0000_0000, // -1
    0x3ff0_0000_0000_0001, // 1 + ulp
    0x3fef_ffff_ffff_ffff, // 1 - ulp/2
    0x3ff8_0000_0000_0000, // 1.5
    0x4000_0000_0000_0000, // 2
    0x4008_0000_0000_0000, // 3
    0xc014_0000_0000_0000, // -5
    0x1ff0_0000_0000_0000, // 2^-512
    0x2000_0000_0000_0001,
    0x5ff0_0000_0000_0000, // 2^512
    0x7fe0_0000_0000_0000, // 2^1023
    0x7fef_ffff_ffff_ffff, // the greatest finite
    0xffef_ffff_ffff_ffff,
    0x7ff0_0000_0000_0000, // inf
    0xfff0_0000_0000_0000, // -inf
    0x7ff8_0000_0000_0000, // the default quiet NaN
    0xfff8_0000_dead_beef, // a quiet NaN with a payload, negative
    0x7ff0_0000_0000_0001, // a signalling NaN
    0x7ff4_0000_1234_0000,
    0xfff7_ffff_ffff_ffff,
];

/// The values `--pins` prints the grid of: a few of each kind.
const PINS: &[u64] = &[
    0x0000_0000_0000_0000,
    0x8000_0000_0000_0003,
    0x000f_ffff_ffff_ffff,
    0x0010_0000_0000_0000,
    0x3fd5_5555_5555_5555,
    0x3fef_ffff_ffff_ffff,
    0xc014_0000_0000_0000,
    0x1ff0_0000_0000_0000,
    0x7fef_ffff_ffff_ffff,
    0xfff0_0000_0000_0000,
    0xfff8_0000_dead_beef,
    0x7ff4_0000_1234_0000,
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A double with this biased exponent and sign, the mantissa random.
    fn with_exp(&mut self, e: u64) -> u64 {
        self.next() & ((1 << 63) | ((1 << 52) - 1)) | e << 52
    }
}

/// The game's routine at `f.entry` on `a` and `b`, run in the
/// interpreter from a booted machine.
fn run(
    m: &mut Machine<Headless>,
    boot: &armv4t::interp::Cpu,
    f: &Found,
    a: u64,
    b: u64,
) -> Result<u64, String> {
    m.cpu = boot.clone();
    m.fault = None;
    m.cpu.cpsr &= !T_BIT;
    m.cpu.r[0] = a as u32;
    m.cpu.r[1] = (a >> 32) as u32;
    m.cpu.r[2] = b as u32;
    m.cpu.r[3] = (b >> 32) as u32;
    m.cpu.r[14] = SENTINEL;
    dispatch::set_pc(&mut m.cpu, f.entry);
    for _ in 0..100_000 {
        if let Some(e) = m.fault {
            return Err(format!("fault {e:?}"));
        }
        if dispatch::key(&m.cpu) == SENTINEL {
            return Ok(m.cpu.r[0] as u64 | (m.cpu.r[1] as u64) << 32);
        }
        m.step_untimed();
    }
    Err("didn't return".into())
}

fn ours(r: Routine, a: u64, b: u64) -> u64 {
    match r {
        Routine::Mul => runtime::softfloat::muldf3(a, b),
        Routine::Div => runtime::softfloat::divdf3(a, b),
    }
}

/// The pairs: the edge grid, then `n` of each random kind.
fn cases(r: Routine, n: usize, seed: u64) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = EDGES
        .iter()
        .flat_map(|&a| EDGES.iter().map(move |&b| (a, b)))
        .collect();
    let mut rng = Rng(seed | 1);
    for _ in 0..n {
        // Any bits.
        v.push((rng.next(), rng.next()));
        // A result near or in the denormals (biased exponent sum or
        // difference about 1023 - 1077 .. 1023 + 2), operands denormal
        // now and then.
        let ea = rng.below(0x7fe) + 1;
        let target = rng.below(60) as i64 - 56;
        let eb = match r {
            Routine::Mul => target - ea as i64 + 1023,
            Routine::Div => ea as i64 - target + 1023,
        };
        let a = rng.with_exp(ea);
        let b = if (1..0x7ff).contains(&eb) {
            rng.with_exp(eb as u64)
        } else {
            rng.next() & ((1 << 63) | ((1 << 52) - 1))
        };
        v.push(if rng.below(2) == 0 { (a, b) } else { (b, a) });
        // A result near overflow.
        let ea = rng.below(0x7fe) + 1;
        let target = 0x7fe - rng.below(4) as i64;
        let eb = match r {
            Routine::Mul => target - ea as i64 + 1023,
            Routine::Div => ea as i64 - target + 1023,
        };
        if (1..0x7ff).contains(&eb) {
            v.push((rng.with_exp(ea), rng.with_exp(eb as u64)));
        }
        // Short mantissas: exact results and rounding ties, here and
        // where the result is denormal.
        let short = |rng: &mut Rng, e: u64| {
            let bits = rng.below(53);
            let m = (rng.next() & ((1u64 << bits) - 1)) << (52 - bits);
            rng.next() & 1 << 63 | e << 52 | m & ((1 << 52) - 1)
        };
        let ea = rng.below(0x7fe) + 1;
        let eb = if rng.below(2) == 0 {
            rng.below(0x7fe) + 1
        } else {
            match r {
                Routine::Mul => (0x3ff + 0x3ff - 1022 - rng.below(54))
                    .saturating_sub(ea)
                    .max(1),
                Routine::Div => (ea + 1022 + rng.below(54)).clamp(1, 0x7fe),
            }
        };
        v.push((short(&mut rng, ea), short(&mut rng, eb)));
    }
    v
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut rom_path = None;
    let mut n = 50_000;
    let mut pins = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cases" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => n = v,
                None => return usage(),
            },
            "--pins" => pins = true,
            _ if rom_path.is_none() && !a.starts_with("--") => rom_path = Some(a.clone()),
            _ => return usage(),
        }
    }
    let Some(rom_path) = rom_path else {
        return usage();
    };
    let rom: &'static [u8] = match std::fs::read(&rom_path) {
        Ok(b) => b.leak(),
        Err(e) => {
            eprintln!("{rom_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let found = scan(rom, 0x0800_0000);
    if found.is_empty() {
        println!("{rom_path}: no __muldf3 or __divdf3 found");
        return ExitCode::FAILURE;
    }
    let mut m = Box::new(Machine::new(rom, Headless));
    let boot = m.cpu.clone();
    let mut bad = false;
    for f in &found {
        if pins {
            for &a in PINS {
                for &b in PINS {
                    match run(&mut m, &boot, f, a, b) {
                        Ok(x) => {
                            println!("    ({:?}, {a:#018x}, {b:#018x}, {x:#018x}),", f.routine)
                        }
                        Err(e) => {
                            eprintln!("{} {a:#x} {b:#x}: {e}", f.routine.name());
                            bad = true;
                        }
                    }
                }
            }
            continue;
        }
        let cs = cases(f.routine, n, f.entry as u64);
        let mut differ = 0;
        let mut shown = 0;
        for &(a, b) in &cs {
            let want = run(&mut m, &boot, f, a, b);
            let got = ours(f.routine, a, b);
            if want.as_ref().ok() != Some(&got) {
                differ += 1;
                if shown < 8 {
                    shown += 1;
                    println!(
                        "  {}({a:#018x}, {b:#018x}): the game's {}, ours {got:#018x}",
                        f.routine.name(),
                        match want {
                            Ok(w) => format!("{w:#018x}"),
                            Err(e) => e,
                        }
                    );
                }
            }
        }
        println!(
            "{} at 0x{:08x}: {} cases, {differ} differ",
            f.routine.name(),
            f.entry,
            cs.len()
        );
        bad |= differ > 0;
    }
    if bad {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: gbaa-softfloat-check ROM [--cases N] [--pins]");
    ExitCode::FAILURE
}
