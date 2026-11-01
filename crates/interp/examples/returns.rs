//! `cargo run --release -p interp --example returns -- ROM [--frames N]
//! [--press KEY@F | KEY@F-G/E]…`: a census of how a ROM's calls come back,
//! for `docs/research/code-without-rom-addresses.md`. The interpreter runs
//! the ROM from reset with a shadow stack of calls: every transfer that
//! leaves lr at the instruction after it (`bl`, `mov lr, pc; bx rN`, the
//! BIOS's `add lr, pc, #0; ldr pc, …`), every IRQ and every handler call. A
//! call returns when the guest reaches its return site with the caller's
//! sp and mode. It is abandoned when the stack in its mode rises above the
//! caller's sp first (a longjmp, a return to somewhere else, a `bl` used as
//! a jump), or unwound when a deeper frame's return site is reached.

use std::collections::HashMap;
use std::process::ExitCode;

use runtime::{Frame, Machine, Platform, io::key};

/// One `press` rule: keys held on frame `from`, or every `every` frames
/// from `from` to `to`.
struct Press {
    key: u16,
    from: u32,
    to: u32,
    every: u32,
}

/// Keys from the press rules, one frame at a time.
struct Script {
    rules: Vec<Press>,
    frame: u32,
}

impl Platform for Script {
    fn present(&mut self, _frame: &Frame) {}

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

/// A call on the shadow stack.
struct Call {
    ret: u32,
    sp: u32,
    mode: u32,
    site: u32,
    callee: u32,
}

#[derive(Default)]
struct Census {
    calls: u64,
    by_bl: u64,
    via_stub: u64,
    by_other: u64,
    irqs: u64,
    returned: u64,
    unwound: u64,
    abandoned: u64,
    /// (call site, callee) of the calls that didn't return, and how many.
    odd: HashMap<(u32, u32, &'static str), u64>,
}

fn mode(cpsr: u32) -> u32 {
    cpsr & 0x1f
}

fn region(a: u32) -> &'static str {
    match a >> 24 {
        0 => "bios",
        2 => "ewram",
        3 => "iwram",
        8..=0xd => "rom",
        _ => "other",
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut rom_path, mut frames, mut rules) = (None, 3600u32, Vec::new());
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
            _ => rom_path = Some(a.clone()),
        }
    }
    let Some(rom_path) = rom_path else {
        eprintln!("usage: returns ROM [--frames N] [--press KEY@F-G/E]...");
        return ExitCode::from(2);
    };
    let rom: &'static [u8] = match std::fs::read(&rom_path) {
        Ok(r) => Box::leak(r.into_boxed_slice()),
        Err(e) => {
            eprintln!("{rom_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut m = Box::new(Machine::new(rom, Script { rules, frame: 0 }));
    let mut stack: Vec<Call> = Vec::new();
    let mut c = Census::default();
    for _ in 0..frames {
        m.sched.frame_ended = false;
        while !m.sched.frame_ended && m.fault.is_none() {
            if m.sched.halted {
                m.idle();
                continue;
            }
            let (pc, sp, md) = (m.cpu.pc, m.cpu.r[13], mode(m.cpu.cpsr));
            if m.take_irq() {
                c.irqs += 1;
                stack.push(Call {
                    ret: pc,
                    sp,
                    mode: md,
                    site: pc,
                    callee: 0x18,
                });
                // The BIOS calls the game's handler with lr at its return path.
                let r14 = m.cpu.r[14];
                stack.push(Call {
                    ret: r14,
                    sp: m.cpu.r[13],
                    mode: mode(m.cpu.cpsr),
                    site: 0x18,
                    callee: m.cpu.pc,
                });
            }
            let (pc, sp, md) = (m.cpu.pc, m.cpu.r[13], mode(m.cpu.cpsr));
            let thumb = m.cpu.cpsr & 0x20 != 0;
            let next = pc.wrapping_add(if thumb { 2 } else { 4 });
            m.step();
            let to = m.cpu.pc;
            if to == next {
                continue;
            }
            let (sp2, md2) = (m.cpu.r[13], mode(m.cpu.cpsr));
            if m.cpu.r[14] & !1 == next && md2 == md && pc >= 0x4000 {
                c.calls += 1;
                let insn = if thumb {
                    m.bus.read16(pc) as u32
                } else {
                    m.bus.read32(pc)
                };
                let bl = if thumb {
                    insn & 0xf800 == 0xf800
                } else {
                    insn & 0x0f00_0000 == 0x0b00_0000
                };
                // The callee's first instruction a `bx rN` (not lr): a stub.
                let stub = if m.cpu.cpsr & 0x20 != 0 {
                    let h = m.bus.read16(to);
                    h & 0xff87 == 0x4700 && (h >> 3) & 15 != 14
                } else {
                    let w = m.bus.read32(to);
                    w & 0x0fff_fff0 == 0x012f_ff10 && w & 15 != 14
                };
                match (bl, stub) {
                    (true, true) => c.via_stub += 1,
                    (true, false) => c.by_bl += 1,
                    _ => c.by_other += 1,
                }
                stack.push(Call {
                    ret: next,
                    sp,
                    mode: md,
                    site: pc,
                    callee: to,
                });
                continue;
            }
            // A return: the nearest frame whose site, sp and mode this is.
            let hit = stack
                .iter()
                .rposition(|f| f.ret & !1 == to & !1 && f.sp == sp2 && f.mode == md2);
            if let Some(i) = hit {
                for f in stack.drain(i + 1..) {
                    c.unwound += 1;
                    *c.odd.entry((f.site, f.callee, "unwound past")).or_default() += 1;
                }
                stack.pop();
                c.returned += 1;
                continue;
            }
            // The stack in this mode now above a caller's sp: that call is
            // gone without coming back.
            while let Some(f) = stack.last()
                && f.mode == md2
                && sp2 > f.sp
                && sp2 >> 24 == f.sp >> 24
            {
                c.abandoned += 1;
                *c.odd.entry((f.site, f.callee, "abandoned")).or_default() += 1;
                stack.pop();
            }
        }
        if m.fault.is_some() {
            break;
        }
    }
    if let Some(f) = m.fault {
        println!("fault: {f}");
    }
    println!(
        "frames {frames}, calls {} (bl {}, bl to a bx stub {}, other lr writes {}), irqs {}",
        c.calls, c.by_bl, c.via_stub, c.by_other, c.irqs
    );
    println!(
        "returned to the site with the caller's sp {}, unwound past {}, abandoned {}, still open {}",
        c.returned,
        c.unwound,
        c.abandoned,
        stack.len()
    );
    let mut odd: Vec<_> = c.odd.into_iter().collect();
    odd.sort_by_key(|a| std::cmp::Reverse(a.1));
    println!("distinct (site, callee) that didn't return: {}", odd.len());
    for ((site, callee, how), n) in odd.iter().take(25) {
        println!(
            "  {n:>8}  {how:<12} call at {site:#010x} ({}) to {callee:#010x} ({})",
            region(*site),
            region(*callee)
        );
    }
    println!("still open, outermost first:");
    for f in &stack {
        println!(
            "  call at {:#010x} to {:#010x}, ret {:#010x}, mode {:#x}",
            f.site, f.callee, f.ret, f.mode
        );
    }
    ExitCode::SUCCESS
}
