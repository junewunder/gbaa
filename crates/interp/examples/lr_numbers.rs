//! `cargo run --release -p interp --example lr_numbers -- ROM [--frames N]
//! [--press KEY@F | KEY@F-G/E]… [--skip ADDR]…`: which code uses a return
//! address as a number? For `docs/research/code-without-rom-addresses.md`.
//!
//! Two machines run the ROM in lockstep, one instruction at a time, with
//! instant timing. In the second, every call (an instruction that leaves lr
//! at the instruction after it, by `bl` or by jumping with lr set) gets a
//! token in lr instead of its return address: a value outside the GBA's
//! memory map (`0x1…_…`) with the address's bits scrambled. When the second
//! machine jumps to a token, it is translated back to the address before
//! the next instruction runs, so both machines always run the same code at
//! the same addresses: this is the "something other than ROM addresses in
//! lr" scheme, simulated. After every instruction the machines' clocks,
//! pc and CPU mode must be equal, and at each frame end RAM, VRAM, OAM,
//! palette, the I/O registers and the drawn frame, where a RAM word may
//! differ only where its bytes hold part of a token (stale return
//! addresses left in dead stack, partly overwritten). Code that adds to a
//! return address and jumps there, loads through it, compares it and
//! branches, or stores something computed from it changes one of these,
//! and the run stops there, naming the innermost call with a token. A
//! register or flag holding a value computed from a token (other than the
//! token itself) is counted and the first few reported, but doesn't stop
//! the run unless it reaches one of those: agbcc's word-wide writes to a
//! halfword field carry uninitialised stack (an old return address) along
//! in a register before overwriting it. `--skip ADDR` leaves calls to the
//! function at ADDR with their real return address, so a run can go on
//! past one known use to find the next. `--self-test` (on a ROM with
//! libgcc's `__gnu_thumb1_case_uqi`, such as moonbeam) is the positive
//! control: it must catch that helper reading its return address.
//!
//! Not covered: the return addresses the IRQ entry makes (`lr_irq`) and
//! the BIOS's own calls into the game.

use std::process::ExitCode;

use runtime::{Machine, Platform, Timing, io::key};

struct Press {
    key: u16,
    from: u32,
    to: u32,
    every: u32,
}

struct Script {
    rules: Vec<Press>,
    frame: u32,
}

impl Platform for Script {
    fn present(&mut self, _frame: &runtime::Frame) {}

    fn keys(&mut self) -> u16 {
        let f = self.frame;
        self.frame += 1;
        self.rules
            .iter()
            .filter(|p| f >= p.from && f <= p.to && (f - p.from).is_multiple_of(p.every))
            .fold(0, |k, p| k | p.key)
    }
}

fn parse_press(s: &str) -> Option<Press> {
    let (name, when) = s.split_once('@')?;
    let key = match name {
        "a" => key::A,
        "b" => key::B,
        "start" => key::START,
        "select" => key::SELECT,
        "up" => key::UP,
        "down" => key::DOWN,
        "left" => key::LEFT,
        "right" => key::RIGHT,
        "l" => key::L,
        "r" => key::R,
        _ => return None,
    };
    let (range, every) = match when.split_once('/') {
        Some((r, e)) => (r, e.parse().ok()?),
        None => (when, 1),
    };
    let (from, to) = match range.split_once('-') {
        Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
        None => (range.parse().ok()?, range.parse().ok()?),
    };
    Some(Press {
        key,
        from,
        to,
        every,
    })
}

/// XORed into an address's low 28 bits to make its token. Its low two bits
/// are clear, so a token keeps the address's alignment and Thumb bit.
const SCRAMBLE: u32 = 0x0555_5554;

/// The token for return address `x` (below `0x1000_0000`, as every GBA
/// code address is): in `0x1000_0000..0x2000_0000`, which nothing maps.
fn token(x: u32) -> u32 {
    0x1000_0000 | ((x ^ SCRAMBLE) & 0x0FFF_FFFF)
}

fn untoken(t: u32) -> u32 {
    (t & 0x0FFF_FFFF) ^ SCRAMBLE
}

fn is_token(v: u32) -> bool {
    v >> 28 == 1
}

/// The second machine's `q` agrees with the first's `p`: equal, or `p`'s
/// token.
fn agree(p: u32, q: u32) -> bool {
    p == q || (p >> 28 == 0 && q == token(p))
}

/// A word of RAM whose bytes differ only where it holds part of a token,
/// at any alignment: a stale or partly overwritten copy of a return
/// address. Every token differs from its address by the same bits,
/// `0x1000_0000 ^ SCRAMBLE`. A byte of it that is loaded later shows in
/// the registers, which must agree exactly.
fn part_token(p: u32, q: u32) -> bool {
    let d = p ^ q;
    (0..4).any(|r| {
        let t = (0x1000_0000 ^ SCRAMBLE).rotate_left(8 * r);
        (0..4).all(|i| {
            let b = (d >> (8 * i)) & 0xFF;
            b == 0 || b == (t >> (8 * i)) & 0xFF
        })
    })
}

type M = Machine<Script>;

/// The first difference in what the CPU does: the clock, pc, and the
/// CPSR's mode and T bit. (Its flags are compared with the registers: a
/// flag that differs changes nothing until a conditional instruction
/// reads it, and then the pc, a register or memory shows it.)
fn flow_differs(a: &M, b: &M) -> Option<String> {
    if a.now() != b.now() {
        return Some(format!("clock {} vs {}", a.now(), b.now()));
    }
    if a.cpu.pc != b.cpu.pc {
        return Some(format!("pc {:#010x} vs {:#010x}", a.cpu.pc, b.cpu.pc));
    }
    if (a.cpu.cpsr ^ b.cpu.cpsr) & 0xFF != 0 {
        return Some(format!("cpsr {:#010x} vs {:#010x}", a.cpu.cpsr, b.cpu.cpsr));
    }
    None
}

/// The first difference in the flags, the SPSRs or a register (current or
/// banked) other than holding the other machine's value's token: a value
/// computed from a token, which changes nothing by itself.
fn regs_differ(a: &M, b: &M) -> Option<String> {
    if a.cpu.cpsr != b.cpu.cpsr {
        return Some(format!(
            "flags {:#010x} vs {:#010x}",
            a.cpu.cpsr, b.cpu.cpsr
        ));
    }
    if a.cpu.spsr != b.cpu.spsr {
        return Some(format!("spsr {:x?} vs {:x?}", a.cpu.spsr, b.cpu.spsr));
    }
    for i in 0..15 {
        if !agree(a.cpu.r[i], b.cpu.r[i]) {
            return Some(format!("r{i} {:#010x} vs {:#010x}", a.cpu.r[i], b.cpu.r[i]));
        }
    }
    for (k, (x, y)) in a.cpu.banks.iter().zip(&b.cpu.banks).enumerate() {
        for i in 0..7 {
            if !agree(x[i], y[i]) {
                return Some(format!(
                    "bank {k} r{} {:#010x} vs {:#010x}",
                    8 + i,
                    x[i],
                    y[i]
                ));
            }
        }
    }
    None
}

/// The first difference in memory or the frame drawn, and how many RAM
/// words of the second machine hold a token.
fn memory_differs(a: &M, b: &M) -> (Option<String>, usize) {
    let mut tokens = 0;
    for (base, x, y) in [
        (0x0200_0000u32, &a.bus.ewram[..], &b.bus.ewram[..]),
        (0x0300_0000, &a.bus.iwram[..], &b.bus.iwram[..]),
    ] {
        for (i, (p, q)) in x
            .as_chunks::<4>()
            .0
            .iter()
            .zip(y.as_chunks::<4>().0)
            .enumerate()
        {
            let (p, q) = (u32::from_le_bytes(*p), u32::from_le_bytes(*q));
            if p != q {
                if !agree(p, q) && !part_token(p, q) {
                    let at = base + 4 * i as u32;
                    return (Some(format!("{at:#010x}: {p:#010x} vs {q:#010x}")), tokens);
                }
                tokens += 1;
            }
        }
    }
    for (name, x, y) in [
        ("vram", &a.bus.video.vram[..], &b.bus.video.vram[..]),
        ("oam", &a.bus.video.oam[..], &b.bus.video.oam[..]),
        (
            "palette",
            &a.bus.video.palette[..],
            &b.bus.video.palette[..],
        ),
    ] {
        if let Some(i) = x.iter().zip(y).position(|(p, q)| p != q) {
            return (Some(format!("{name} byte {i:#x}")), tokens);
        }
    }
    for o in (0..runtime::io::IO_SIZE).step_by(2) {
        let (p, q) = (a.bus.io.read16(o), b.bus.io.read16(o));
        if p != q {
            return (
                Some(format!("I/O register {:#05x}: {p:#06x} vs {q:#06x}", o)),
                tokens,
            );
        }
    }
    if a.ppu.frame != b.ppu.frame {
        return (Some("the frame drawn".into()), tokens);
    }
    (None, tokens)
}

fn parse_addr(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim_start_matches("0x").replace('_', "").as_str(), 16).ok()
}

/// A call given a token, still open.
struct Open {
    site: u32,
    callee: u32,
    ret: u32,
}

/// The two machines and what the run has counted.
struct Pair {
    a: Box<M>,
    b: Box<M>,
    skip: Vec<u32>,
    open: Vec<Open>,
    steps: u64,
    tokened: u64,
    skipped: u64,
    /// Instructions after which a register held a value computed from a
    /// token, and the first few of them.
    soft: u64,
    soft_seen: Vec<String>,
}

impl Pair {
    /// One instruction (or a halted wait, or an IRQ's entry) in both
    /// machines. Err: where they first differ.
    fn step(&mut self) -> Result<(), String> {
        let (a, b) = (&mut self.a, &mut self.b);
        if a.sched.halted != b.sched.halted {
            return Err("one machine halted, the other not".into());
        }
        if a.sched.halted {
            a.idle();
            b.idle();
            return Ok(());
        }
        if a.take_irq() != b.take_irq() {
            return Err("an IRQ taken by one machine only".into());
        }
        let (pc, md, lr0) = (a.cpu.pc, a.cpu.cpsr & 0x1f, a.cpu.r[14]);
        let thumb = a.cpu.cpsr & 0x20 != 0;
        let next = pc.wrapping_add(if thumb { 2 } else { 4 });
        let raw = if thumb {
            a.bus.read16(pc) as u32
        } else {
            a.bus.read32(pc)
        };
        let bl = if thumb {
            raw & 0xF800 == 0xF800
        } else {
            raw & 0x0F00_0000 == 0x0B00_0000 && raw >> 28 != 0xF
        };
        a.step();
        b.step();
        self.steps += 1;
        // The second machine returned through a token: back to the
        // address, and that call is closed.
        if is_token(b.cpu.pc) {
            b.cpu.pc = untoken(b.cpu.pc);
            let at = b.cpu.pc;
            if let Some(i) = self.open.iter().rposition(|o| o.ret & !1 == at) {
                self.open.truncate(i);
            }
        }
        // A call in the first machine: the second's lr gets the token.
        let called = a.cpu.r[14] & !1 == next
            && (a.cpu.r[14] != lr0 || bl)
            && (a.cpu.pc != next || bl)
            && a.cpu.cpsr & 0x1f == md
            && pc >= 0x4000
            && b.cpu.r[14] == a.cpu.r[14];
        if called {
            if self.skip.contains(&(a.cpu.pc & !1)) {
                self.skipped += 1;
            } else {
                b.cpu.r[14] = token(a.cpu.r[14]);
                self.tokened += 1;
                self.open.push(Open {
                    site: pc,
                    callee: a.cpu.pc,
                    ret: next,
                });
                if self.open.len() > 4096 {
                    self.open.remove(0);
                }
            }
        }
        let call = || match self.open.last() {
            Some(o) => format!(
                "innermost call with a token: at {:#010x} to {:#010x}, returning to {:#010x}",
                o.site, o.callee, o.ret
            ),
            None => "no call with a token open".into(),
        };
        let at = format!(
            "step {}: after {pc:#010x} ({raw:#x}, {})",
            self.steps,
            if thumb { "thumb" } else { "arm" }
        );
        if let Some(d) = flow_differs(a, b) {
            return Err(format!("{at}: {d}\n{}", call()));
        }
        if let Some(d) = regs_differ(a, b) {
            self.soft += 1;
            if self.soft_seen.len() < 5
                && !self
                    .soft_seen
                    .iter()
                    .any(|s| s.contains(&format!("{pc:#010x}")))
            {
                self.soft_seen.push(format!("{at}: {d}; {}", call()));
            }
        }
        Ok(())
    }
}

/// The positive control: in IWRAM, a Thumb `bl` to a stub that jumps to
/// the ROM's `__gnu_thumb1_case_uqi` (libgcc's switch helper, which reads
/// a byte table at its return address and returns past it), the table
/// after the `bl`, and a `b .` where entry 1 sends it. The run must stop
/// at the helper's first use of lr.
fn self_test(p: &mut Pair, rom: &[u8]) -> ExitCode {
    // push {r1}; mov r1, lr; lsrs r1, r1, #1; lsls r1, r1, #1; ldrb r1, [r1, r0]
    let uqi = [0x02, 0xb4, 0x71, 0x46, 0x49, 0x08, 0x49, 0x00, 0x09, 0x5c];
    let Some(at) = rom.windows(uqi.len()).position(|w| w == uqi) else {
        println!("self-test: this ROM has no __gnu_thumb1_case_uqi (moonbeam does)");
        return ExitCode::from(2);
    };
    let helper = 0x0800_0000 + at as u32;
    let mut code: Vec<u8> = Vec::new();
    let half = |c: &mut Vec<u8>, h: u16| c.extend_from_slice(&h.to_le_bytes());
    // 0x00: movs r0, #1
    half(&mut code, 0x2001);
    // 0x02: bl 0x10 (offset 0x10 - 0x06 = 0x0a, in halfwords 5)
    half(&mut code, 0xF000);
    half(&mut code, 0xF800 | 5);
    // 0x06: the table: entry 0 -> +0, entry 1 -> +4 (to 0x0a)
    code.extend_from_slice(&[0x00, 0x02]);
    // 0x08: b . (entry 0, never taken); 0x0a: b . (entry 1)
    half(&mut code, 0xE7FE);
    half(&mut code, 0xE7FE);
    // 0x0c: padding; 0x10: the stub: ldr r3, [pc, #0]; bx r3; the helper
    half(&mut code, 0);
    half(&mut code, 0);
    half(&mut code, 0x4B00);
    half(&mut code, 0x4718);
    code.extend_from_slice(&(helper | 1).to_le_bytes());
    for m in [&mut p.a, &mut p.b] {
        m.bus.iwram[..code.len()].copy_from_slice(&code);
        m.cpu.pc = 0x0300_0000;
        m.cpu.cpsr = 0x1F | 0x20;
    }
    for _ in 0..40 {
        if let Err(e) = p.step() {
            println!("self-test: caught, as it must be:\n{e}");
            return ExitCode::SUCCESS;
        }
    }
    println!("self-test FAILED: the helper's use of its return address went unseen");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut rom_path, mut frames, mut rules) = (None, 3600u32, Vec::new());
    let (mut skip, mut test) = (Vec::new(), false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--frames" => frames = it.next().and_then(|n| n.parse().ok()).unwrap_or(frames),
            "--press" => match it.next().and_then(|p| parse_press(p)) {
                Some(p) => rules.push(p),
                None => {
                    eprintln!("--press KEY@F or KEY@F-G/E");
                    return ExitCode::from(2);
                }
            },
            "--skip" => match it.next().and_then(|s| parse_addr(s)) {
                Some(x) => skip.push(x & !1),
                None => {
                    eprintln!("--skip ADDR (hex)");
                    return ExitCode::from(2);
                }
            },
            "--self-test" => test = true,
            _ => rom_path = Some(a.clone()),
        }
    }
    let Some(rom_path) = rom_path else {
        eprintln!(
            "usage: lr_numbers ROM [--frames N] [--press KEY@F-G/E]... [--skip ADDR]... [--self-test]"
        );
        return ExitCode::from(2);
    };
    let rom: &'static [u8] = match std::fs::read(&rom_path) {
        Ok(r) => Box::leak(r.into_boxed_slice()),
        Err(e) => {
            eprintln!("{rom_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let script = |rules: &[Press]| Script {
        rules: rules
            .iter()
            .map(|p| Press {
                key: p.key,
                from: p.from,
                to: p.to,
                every: p.every,
            })
            .collect(),
        frame: 0,
    };
    let mut p = Pair {
        a: Box::new(Machine::new(rom, script(&rules))),
        b: Box::new(Machine::new(rom, script(&rules))),
        skip,
        open: Vec::new(),
        steps: 0,
        tokened: 0,
        skipped: 0,
        soft: 0,
        soft_seen: Vec::new(),
    };
    p.a.set_timing(Timing::Instant);
    p.b.set_timing(Timing::Instant);
    if test {
        return self_test(&mut p, rom);
    }
    let mut most_tokens = 0;
    for frame in 0..frames {
        p.a.sched.frame_ended = false;
        p.b.sched.frame_ended = false;
        while !p.a.sched.frame_ended && p.a.fault.is_none() {
            if let Err(e) = p.step() {
                println!("frame {frame}, {e}");
                return ExitCode::FAILURE;
            }
        }
        if p.b.sched.frame_ended != p.a.sched.frame_ended {
            println!("frame {frame}: the frame ended in one machine only");
            return ExitCode::FAILURE;
        }
        if let Some(f) = p.a.fault {
            println!("frame {frame}: fault: {f}");
            return ExitCode::FAILURE;
        }
        let (d, tokens) = memory_differs(&p.a, &p.b);
        most_tokens = most_tokens.max(tokens);
        if let Some(d) = d {
            println!("frame {frame}: memory differs at {d}");
            return ExitCode::FAILURE;
        }
    }
    println!(
        "frames {frames}, steps {}, calls given a token {} (skipped {}), \
         most RAM words holding a token at a frame end {most_tokens}: no difference",
        p.steps, p.tokened, p.skipped
    );
    println!(
        "register-only differences (a value computed from a token that changed nothing): {}",
        p.soft
    );
    for s in &p.soft_seen {
        println!("  {s}");
    }
    ExitCode::SUCCESS
}
