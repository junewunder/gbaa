//! `gbaa-holes ROM [--frames N] [--press KEY@F | KEY@F-G/E]… [--no-summaries]
//! [--no-run-roots] [--show N]`: what blocks a native build of ROM
//! (`docs/native-port-design.md`, section 6; `docs/research/native-holes.md`).
//!
//! Discovers the ROM and its IWRAM overlays, runs it in the interpreter
//! (instant timing, no save file) for N frames (default 5,400) with a press
//! script (default: the one of `docs/research/code-without-rom-addresses.md`,
//! [`DEFAULT_PRESSES`]), discovers the ROM again with the ROM targets of
//! the run's calls through a register it lacked as function starts
//! ([`analysis::discovery::discover_with`]; not with `--no-run-roots`),
//! computes the function summaries (`codegen::saves::summarize`), finds
//! the RAM copies again with what the run saw
//! ([`analysis::holes::ram_copies`]: the run's entries into BIOS copies,
//! and copies made by DMA or the game's own loops), and prints
//! [`analysis::holes::classify`]'s report over those: one ranking line,
//! then the holes by kind (`--show N` of each list, default 12).
//!
//! What the run records, per instruction executed: the address and
//! instruction set, its first raw value (and whether a later one differed:
//! RAM code that changed), whether it ran at the top level of an IRQ (the
//! IRQ wrapper) or after the first frame, whether it changed the CPU mode
//! or moved sp by [`analysis::holes::STACK_SWITCH`] or more. And a stack of
//! open calls (as the scrambled-return-address tool, `crates/interp/examples/lr_numbers.rs`,
//! keeps): a return through `bx`, `pop {pc}`, `mov pc` or `ldr pc` that lands
//! within 64 bytes before or 1 KiB after the innermost open call's return
//! site, but not on it, is return-address arithmetic. Calls through a
//! register record their target, and calls into RAM the bytes there.

use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;

use analysis::holes::{self, CpuWhere, RetArith, Run, STACK_SWITCH, Seen};
use armv4t::decode::{Instr, Reg, decode_arm, decode_thumb};
use runtime::{Machine, Platform, Timing, io::key};

/// The research doc's script: Start every 2 s from frame 300, A every half
/// second from 400, B every 0.75 s from 2500, and right, up and left held
/// in turn for 1000 frames each from 2000.
const DEFAULT_PRESSES: &[&str] = &[
    "start@300-5400/120",
    "a@400-5400/30",
    "b@2500-5400/45",
    "right@2000-2999",
    "up@3000-3999",
    "left@4000-5400",
];

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
        Some((r, e)) => (r, e.parse().ok().filter(|&e| e > 0)?),
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

const EXEC: u8 = 1;
const THUMB: u8 = 2;
const CHANGED: u8 = 4;
const WRAPPER: u8 = 8;
const AFTER_BOOT: u8 = 16;
const MODE: u8 = 32;
const STACK: u8 = 64;

/// Per halfword of a region: flags, and (RAM only) the first raw value.
struct Region {
    base: u32,
    mask: u32,
    flags: Vec<u8>,
    raw: Vec<u32>,
}

impl Region {
    fn new(base: u32, size: u32, ram: bool) -> Region {
        Region {
            base,
            mask: size - 1,
            flags: vec![0; size as usize / 2],
            raw: if ram {
                vec![0; size as usize / 2]
            } else {
                Vec::new()
            },
        }
    }
}

struct Frame {
    site: u32,
    callee: u32,
    ret: u32,
    irq: bool,
}

/// Per backward branch: the counters at its last take, and its
/// qualifying iterations this frame.
struct Loop {
    calls: u64,
    halts: u64,
    loads: u64,
    stores: u64,
    /// The last iteration's stores ([`Watcher::stores`]' difference).
    digest: u64,
    frame: u32,
    good: u32,
}

/// Which RAM bytes an IRQ handler (or what it calls) has written, and how
/// many loads outside IRQs read one.
struct Watcher {
    irq: bool,
    written: Vec<bool>,
    loads: u64,
    /// A sum over the stores outside IRQs of a hash of (address, value):
    /// two stretches that made the same stores, in any order, add the same.
    stores: u64,
}

fn mix(a: u32, v: u32) -> u64 {
    let x = ((a as u64) << 32 | v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^ x >> 29
}

fn ram_index(a: u32) -> Option<usize> {
    match a >> 24 {
        0x02 => Some((a & 0x3_FFFF) as usize),
        0x03 => Some(0x4_0000 + (a & 0x7FFF) as usize),
        _ => None,
    }
}

impl runtime::debug::Watch for Watcher {
    fn access(&mut self, a: runtime::debug::Access) {
        if a.write && !self.irq {
            self.stores = self.stores.wrapping_add(mix(a.addr, a.new));
        }
        let Some(i) = ram_index(a.addr) else { return };
        let bytes = i..(i + a.width as usize).min(self.written.len());
        if a.write {
            if self.irq {
                self.written[bytes].iter_mut().for_each(|w| *w = true);
            }
        } else if !self.irq && self.written[bytes].iter().any(|&w| w) {
            self.loads += 1;
        }
    }
}

struct Rec {
    rom: Region,
    ewram: Region,
    iwram: Region,
    stack: Vec<Frame>,
    far_jumps: BTreeSet<u32>,
    run: Run,
    after_boot: bool,
}

impl Rec {
    fn region(&mut self, a: u32) -> Option<&mut Region> {
        match a >> 24 {
            0x02 => Some(&mut self.ewram),
            0x03 => Some(&mut self.iwram),
            0x08..=0x0D => Some(&mut self.rom),
            _ => None,
        }
    }

    /// The address `a` is recorded under: RAM mirrors folded, ROM's
    /// wait-state mirrors folded to 0x0800_0000.
    fn canon(&self, a: u32) -> u32 {
        match a >> 24 {
            0x02 => 0x0200_0000 | (a & self.ewram.mask),
            0x03 => 0x0300_0000 | (a & self.iwram.mask),
            0x08..=0x0D => 0x0800_0000 | (a & self.rom.mask),
            _ => a,
        }
    }

    fn mark(&mut self, a: u32, thumb: bool, raw: u32, extra: u8) {
        let ab = if self.after_boot { AFTER_BOOT } else { 0 };
        let Some(r) = self.region(a) else { return };
        let i = ((a & r.mask) / 2) as usize;
        let Some(f) = r.flags.get_mut(i) else { return };
        if *f & EXEC == 0 {
            *f = EXEC | if thumb { THUMB } else { 0 };
            if !r.raw.is_empty() {
                r.raw[i] = raw;
            }
        } else if !r.raw.is_empty() && (r.raw[i] != raw || (*f & THUMB != 0) != thumb) {
            *f |= CHANGED;
        }
        *f |= extra | ab;
    }

    fn finish(mut self, rom: &[u8]) -> Run {
        let mut code = BTreeMap::new();
        for r in [&self.rom, &self.ewram, &self.iwram] {
            for (i, &f) in r.flags.iter().enumerate() {
                if f & EXEC == 0 {
                    continue;
                }
                let a = r.base + 2 * i as u32;
                let thumb = f & THUMB != 0;
                let raw = if r.raw.is_empty() {
                    let o = 2 * i;
                    let b = |k: usize| rom.get(o + k).copied().unwrap_or(0);
                    if thumb {
                        u16::from_le_bytes([b(0), b(1)]) as u32
                    } else {
                        u32::from_le_bytes([b(0), b(1), b(2), b(3)])
                    }
                } else {
                    r.raw[i]
                };
                code.insert(
                    a,
                    Seen {
                        thumb,
                        raw,
                        changed: f & CHANGED != 0,
                        wrapper: f & WRAPPER != 0,
                        after_boot: f & AFTER_BOOT != 0,
                        mode_change: f & MODE != 0,
                        stack_switch: f & STACK != 0,
                    },
                );
            }
        }
        self.run.code = code;
        self.run
    }
}

/// An instruction that can return: `bx`, `ldm`/`pop` with pc, a
/// data-processing write to pc from lr (`mov pc, lr`, `add pc, lr, …`),
/// `ldr pc` from the stack. Not `mov pc, r0` or `ldr pc, [rN]` from a
/// table: a switch's jump.
fn returns(i: &Instr) -> bool {
    match *i {
        Instr::Bx { .. } => true,
        Instr::Block {
            load: true, regs, ..
        } => regs & 1 << 15 != 0,
        Instr::Alu {
            rd: Reg::Pc,
            rn,
            op2,
            ..
        } => rn == Reg::Lr || matches!(op2, armv4t::decode::Operand2::Reg { rm: Reg::Lr, .. }),
        Instr::Load {
            rd: Reg::Pc, addr, ..
        } => addr.rn == Reg::Sp,
        _ => false,
    }
}

/// A call into RAM: the bytes at its target when called
/// ([`Run::ram_calls`]), for the flash library's stack routines
/// (`analysis::flash`).
fn ram_call(run: &mut Run, bus: &runtime::Bus, callee: u32) {
    let a = callee & !1;
    if !(0x0200_0000..0x0400_0000).contains(&a) {
        return;
    }
    let bytes: Vec<u8> = (0..holes::RAM_CALL_BYTES)
        .map(|i| bus.read8(a + i))
        .collect();
    let seen = run.ram_calls.entry(callee).or_default();
    // A loop that copies a different routine in each time is a handful; a
    // cap keeps code built at run time from growing it, and an empty entry
    // past it (recognised as nothing) says some weren't kept.
    if seen.len() < 16 {
        seen.insert(bytes);
    } else if !seen.contains(&bytes) {
        seen.insert(Vec::new());
    }
}

/// Whether the instruction before `a` is a call, so `a` is some call's
/// return site: a Thumb `bl` pair, an ARM `bl`, or ARM `mov lr, pc` and a
/// jump.
fn after_call(bus: &runtime::Bus, a: u32, thumb: bool) -> bool {
    if thumb {
        bus.read16(a.wrapping_sub(4)) & 0xF800 == 0xF000
            && bus.read16(a.wrapping_sub(2)) & 0xF800 == 0xF800
    } else {
        let w = bus.read32(a.wrapping_sub(4));
        let bl = w & 0x0F00_0000 == 0x0B00_0000 && w >> 28 != 0xF;
        bl || bus.read32(a.wrapping_sub(8)) == 0xE1A0_E00F
    }
}

fn run(
    rom: &'static [u8],
    frames: u32,
    rules: Vec<Press>,
    far_jumps: BTreeSet<u32>,
) -> Result<Run, String> {
    let mut m = Machine::new(rom, Script { rules, frame: 0 });
    m.timing = Timing::Instant;
    let rom_size = (rom.len() as u32).next_power_of_two().max(4);
    let mut rec = Rec {
        rom: Region::new(0x0800_0000, rom_size, false),
        ewram: Region::new(0x0200_0000, 0x4_0000, true),
        iwram: Region::new(0x0300_0000, 0x8000, true),
        stack: Vec::new(),
        far_jumps,
        run: Run::default(),
        after_boot: false,
    };
    let mut steps = 0u64;
    let mut watch = Watcher {
        irq: false,
        written: vec![false; 0x4_8000],
        loads: 0,
        stores: 0,
    };
    let (mut calls, mut halts) = (0u64, 0u64);
    // The instruction before set lr (and wasn't a call).
    let mut lr_just_set = false;
    let mut loops: std::collections::HashMap<u32, Loop> = std::collections::HashMap::new();
    for frame in 0..frames {
        m.platform.frame = frame;
        rec.after_boot = frame > 0;
        m.sched.frame_ended = false;
        while !m.sched.frame_ended {
            if let Some(f) = &m.fault {
                return Err(format!("frame {frame}: fault {f:?}"));
            }
            if m.sched.halted {
                halts += 1;
                m.idle();
                continue;
            }
            let before = m.cpu.pc;
            if m.take_irq() {
                rec.stack.push(Frame {
                    site: 0,
                    callee: m.cpu.pc,
                    ret: before,
                    irq: true,
                });
                rec.run.irq_handlers.insert(m.cpu.pc);
                watch.irq = true;
            }
            let thumb = m.cpu.cpsr & 0x20 != 0;
            let (mode, lr0, sp0) = (m.cpu.cpsr & 0x1f, m.cpu.r[14], m.cpu.r[13]);
            let pc = m.cpu.pc & if thumb { !1 } else { !3 };
            if pc < 0x4000 {
                // The BIOS: the IRQ return, a stall, the swi vector.
                m.step();
                let npc = m.cpu.pc & !1;
                if let Some(i) = rec.stack.iter().rposition(|f| f.ret & !1 == npc) {
                    rec.stack.truncate(i);
                    watch.irq = rec.stack.iter().any(|f| f.irq);
                }
                continue;
            }
            let raw = if thumb {
                m.bus.read16(pc) as u32
            } else {
                m.bus.read32(pc)
            };
            let size = if thumb { 2 } else { 4 };
            let next = pc.wrapping_add(size);
            let instr = if thumb {
                decode_thumb(raw as u16, pc)
            } else {
                decode_arm(raw, pc)
            };
            let bl = matches!(
                instr,
                Instr::ThumbBlLo { .. } | Instr::Branch { link: true, .. }
            );
            let top_irq = rec.stack.last().is_some_and(|f| f.irq);
            let canon = rec.canon(pc);
            m.step_watched(&mut watch);
            steps += 1;
            let mut extra = if top_irq { WRAPPER } else { 0 };
            let npc = m.cpu.pc & !1;
            let nmode = m.cpu.cpsr & 0x1f;
            if nmode != mode && !matches!(instr, Instr::Swi { .. }) && !m.sched.halted {
                extra |= MODE;
            }
            // sp-relative arithmetic (a large frame, a variable-length
            // array) moves sp but doesn't switch stacks.
            let sp_rel = matches!(
                instr,
                Instr::Alu {
                    rd: Reg::Sp,
                    rn: Reg::Sp,
                    ..
                }
            );
            let swi = matches!(instr, Instr::Swi { .. });
            if nmode == mode && !sp_rel && !swi && m.cpu.r[13].abs_diff(sp0) >= STACK_SWITCH {
                extra |= STACK;
            }
            rec.mark(canon, thumb, raw, extra);
            // A call: a taken `bl`, or a jump that leaves lr at the next
            // instruction having set it (`bx` with lr written), or a jump
            // through a register right after the instruction before set lr
            // to an address within 32 bytes of it (`mov lr, pc; bx r0`,
            // pret's `adr lr, 1f; bx r0`). Not a jump to lr itself: that
            // returns.
            let lr = m.cpu.r[14] & !1;
            let reg_jump = match instr {
                Instr::Bx { rm, .. } => rm != Reg::Lr && rm != Reg::Pc,
                Instr::Alu { rd: Reg::Pc, .. } | Instr::Load { rd: Reg::Pc, .. } => true,
                _ => false,
            };
            let near = lr >= pc.wrapping_sub(32) && lr <= pc.wrapping_add(32);
            let called = nmode == mode
                && npc != next
                && npc != lr
                && ((bl && lr == next)
                    || (lr == next && m.cpu.r[14] != lr0)
                    || (reg_jump && near && lr_just_set));
            lr_just_set = !called && nmode == mode && m.cpu.r[14] != lr0;
            let ntb = (m.cpu.cpsr >> 5) & 1;
            if let Instr::Branch {
                link: false,
                target,
                ..
            } = instr
                && npc == target & !1
                && target <= pc
                && !watch.irq
            {
                let l = loops.entry(canon).or_insert(Loop {
                    calls,
                    halts,
                    loads: watch.loads,
                    stores: watch.stores,
                    digest: 0,
                    frame,
                    good: 0,
                });
                if l.frame != frame {
                    l.frame = frame;
                    l.good = 0;
                }
                let digest = watch.stores.wrapping_sub(l.stores);
                let busy = calls > l.calls && halts == l.halts && watch.loads > l.loads;
                if busy && digest == l.digest {
                    l.good += 1;
                    if l.good >= holes::BUSY_ITERATIONS {
                        let e = rec.run.busy_loops.entry(canon).or_default();
                        *e = (*e).max(l.good);
                    }
                } else {
                    l.good = 0;
                }
                (l.calls, l.halts, l.loads, l.stores, l.digest) =
                    (calls, halts, watch.loads, watch.stores, digest);
            }
            if called {
                if !watch.irq {
                    calls += 1;
                }
                let site = if thumb && bl { pc.wrapping_sub(2) } else { pc };
                let site = rec.canon(site);
                let callee = rec.canon(npc) | ntb;
                rec.run.call_targets.insert(callee);
                if !bl {
                    rec.run.indirect_calls.entry(callee).or_insert(site);
                }
                ram_call(&mut rec.run, &m.bus, callee);
                if !rec.far_jumps.contains(&site) {
                    rec.stack.push(Frame {
                        site,
                        callee,
                        ret: lr,
                        irq: false,
                    });
                    if rec.stack.len() > 4096 {
                        rec.stack.remove(0);
                        watch.irq = rec.stack.iter().any(|f| f.irq);
                    }
                }
            } else if npc != next {
                if let Some(i) = rec.stack.iter().rposition(|f| f.ret & !1 == npc) {
                    rec.stack.truncate(i);
                    watch.irq = rec.stack.iter().any(|f| f.irq);
                } else if let Some(top) = rec.stack.last()
                    && !top.irq
                    && top.callee & !1 == canon
                    && matches!(instr, Instr::Bx { rm, .. } if rm != Reg::Lr && rm != Reg::Pc)
                {
                    // A `bl` to a lone `bx rN` (agbcc's `_call_via_rN`):
                    // a call through a register.
                    let (site, callee) = (top.site, rec.canon(npc) | ntb);
                    rec.run.call_targets.insert(callee);
                    rec.run.indirect_calls.entry(callee).or_insert(site);
                    ram_call(&mut rec.run, &m.bus, callee);
                    if let Some(top) = rec.stack.last_mut() {
                        top.callee = callee;
                    }
                } else if let Some(top) = rec.stack.last()
                    && !top.irq
                    && npc >= 0x4000
                    && returns(&instr)
                    && npc >= top.ret.wrapping_sub(64)
                    && npc < top.ret.wrapping_add(1024)
                    && !after_call(&m.bus, npc, ntb != 0)
                {
                    let delta = npc.wrapping_sub(top.ret) as i32;
                    let to = rec.canon(npc) | ntb;
                    let e = rec.run.ret_arith.entry((top.site, to)).or_insert(RetArith {
                        site: top.site,
                        callee: top.callee,
                        at: canon,
                        to,
                        delta,
                        count: 0,
                    });
                    e.count += 1;
                    rec.stack.pop();
                }
            }
        }
    }
    let mut out = rec.finish(rom);
    out.frames = frames;
    out.instructions = steps;
    Ok(out)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut rom_path, mut frames, mut presses) = (None, 5400u32, Vec::new());
    let (mut sums, mut show, mut run_roots) = (true, 12usize, true);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--frames" => match it.next().and_then(|n| n.parse().ok()) {
                Some(n) => frames = n,
                None => return usage(),
            },
            "--press" => match it.next() {
                Some(p) => presses.push(p.clone()),
                None => return usage(),
            },
            "--show" => match it.next().and_then(|n| n.parse().ok()) {
                Some(n) => show = n,
                None => return usage(),
            },
            "--no-summaries" => sums = false,
            "--no-run-roots" => run_roots = false,
            _ if rom_path.is_none() && !a.starts_with("--") => rom_path = Some(a.clone()),
            _ => return usage(),
        }
    }
    let Some(rom_path) = rom_path else {
        return usage();
    };
    if presses.is_empty() {
        presses = DEFAULT_PRESSES.iter().map(|s| s.to_string()).collect();
    }
    let mut rules = Vec::new();
    for p in &presses {
        match parse_press(p) {
            Some(r) => rules.push(r),
            None => {
                eprintln!("--press KEY@F or KEY@F-G/E: {p}");
                return ExitCode::from(2);
            }
        }
    }
    let rom: &'static [u8] = match std::fs::read(&rom_path) {
        Ok(r) => Box::leak(r.into_boxed_slice()),
        Err(e) => {
            eprintln!("{rom_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let game = std::path::Path::new(&rom_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(rom_path.clone());

    let t = std::time::Instant::now();
    let disc = analysis::discovery::discover(rom.to_vec());
    let overlays = analysis::discovery::overlays(rom, &disc);
    eprintln!("discovery: {:.1} s", t.elapsed().as_secs_f64());
    let t = std::time::Instant::now();
    let mut far_jumps = disc.rec.far_jumps.clone();
    for o in &overlays {
        far_jumps.extend(&o.disc.rec.far_jumps);
    }
    let run = match run(rom, frames, rules, far_jumps) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{game}: the run stopped: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "run: {:.1} s, {} instructions",
        t.elapsed().as_secs_f64(),
        run.instructions
    );
    // The run's calls through a register into ROM become starts: discover
    // again with their targets as roots. All of them, not only the ones
    // discovery lacked: a start the pointer scan found is lost when the new
    // roots reach it first by a `b` (m4a's `ply_goto`). Not every call
    // target: a far-jump `bl`'s lands inside a function.
    let roots: Vec<u32> = run
        .indirect_calls
        .keys()
        .copied()
        .filter(|&a| a >= 0x0800_0000)
        .collect();
    let lacked = roots.iter().filter(|a| !disc.funcs.contains_key(a)).count();
    let disc = if run_roots && lacked > 0 {
        let t = std::time::Instant::now();
        let d = analysis::discovery::discover_with(rom.to_vec(), &roots);
        eprintln!(
            "discovery with {} run roots: {:.1} s",
            roots.len(),
            t.elapsed().as_secs_f64()
        );
        d
    } else {
        disc
    };
    let summaries = sums.then(|| {
        let t = std::time::Instant::now();
        let ctx = codegen::func::Ctx::new(rom, &disc);
        let funcs = codegen::emit::build_all(&ctx).funcs;
        let s = codegen::saves::summarize(&ctx, &funcs);
        eprintln!("summaries: {:.1} s", t.elapsed().as_secs_f64());
        s.by_entry
            .iter()
            .filter(|(_, s)| !s.holes.is_empty())
            .map(|(&e, s)| (e, s.holes.iter().cloned().collect::<Vec<_>>()))
            .collect::<BTreeMap<u32, Vec<String>>>()
    });
    // The RAM copies again, now with the run's entries into them and the
    // copies only the run shows (`analysis::holes::ram_copies`).
    let t = std::time::Instant::now();
    let copies = holes::ram_copies(rom, &disc, &run);
    eprintln!("RAM copies: {:.1} s", t.elapsed().as_secs_f64());
    let (folded, filled) = (
        copies.iter().filter(|c| !c.seen).count(),
        copies.iter().filter(|c| !c.filled.is_empty()).count(),
    );
    let mut overlays = Vec::new();
    let mut copy_lines = Vec::new();
    for c in copies {
        let roots = |ks: &[u32]| {
            let s: Vec<String> = ks.iter().take(4).map(|k| format!("0x{k:08x}")).collect();
            let more = ks.len().saturating_sub(4);
            s.join(",")
                + &if more > 0 {
                    format!(" (+{more})")
                } else {
                    String::new()
                }
        };
        copy_lines.push(format!(
            "  0x{:08x}..0x{:08x} from ROM 0x{:08x}, {}: {} functions; entered at {}; filled {}",
            c.overlay.dst,
            c.overlay.dst + c.overlay.len,
            c.overlay.src,
            if c.seen {
                "seen in the run"
            } else {
                "a BIOS copy"
            },
            c.overlay.disc.funcs.len(),
            roots(&c.entered),
            if c.filled.is_empty() {
                "none".into()
            } else {
                roots(&c.filled)
            },
        ));
        overlays.push(c.overlay);
    }
    let r = holes::classify(rom, &disc, &overlays, &run, summaries.as_ref());

    println!("{}", r.line(&game));
    println!(
        "run roots: {} ROM targets of calls through a register, {lacked} not discovered{}",
        roots.len(),
        if lacked == 0 {
            ""
        } else if run_roots {
            ": all given to discovery as function starts"
        } else {
            " (--no-run-roots: not given to it)"
        }
    );
    println!(
        "run: {} frames, {} instructions, presses {}",
        run.frames,
        run.instructions,
        presses.join(" ")
    );
    let recovery = disc
        .rec
        .jumps
        .values()
        .filter(|j| !matches!(j, analysis::vsa::func::JumpRes::Resolved(_)))
        .count();
    println!(
        "Recovery: {} indirect jumps, {} not resolved ({} counted at their RAM copy instead; of the rest {} rejected, {} reached in the run); overlays {} not resolved",
        disc.rec.jumps.len(),
        recovery,
        recovery - r.unresolved.len(),
        r.unresolved.iter().filter(|u| u.rejected).count(),
        r.unresolved.iter().filter(|u| u.reached).count(),
        r.overlay_unresolved.len(),
    );
    let mut unres: Vec<_> = r.unresolved.iter().chain(&r.overlay_unresolved).collect();
    unres.sort_by_key(|u| (!u.reached, u.at));
    for u in unres.iter().take(show) {
        println!(
            "  jump 0x{:08x}{}{}",
            u.at,
            if u.rejected { " rejected" } else { "" },
            if u.reached { " (reached)" } else { "" }
        );
    }
    println!(
        "return-address arithmetic: {} ({} not resolved)",
        r.ret.len(),
        r.hard_ret()
    );
    for h in &r.ret {
        let sites: Vec<String> = h
            .sites
            .iter()
            .take(4)
            .map(|s| format!("0x{s:08x}"))
            .collect();
        println!(
            "  func 0x{:08x} at 0x{:08x}{} from {}{}: {}{}",
            h.func,
            h.at,
            h.delta
                .map(|d| format!(" returns to site{d:+}"))
                .unwrap_or_default(),
            sites.join(","),
            if h.sites.len() > 4 {
                format!(" (+{})", h.sites.len() - 4)
            } else {
                String::new()
            },
            if h.resolved {
                "resolved"
            } else {
                "NOT resolved"
            },
            if h.reached { ", reached" } else { "" },
        );
    }
    println!(
        "RAM copies: {} ({} BIOS copies discovery folded, {} seen in the run); roots filled where the run went further: {}",
        copy_lines.len(),
        folded,
        copy_lines.len() - folded,
        filled,
    );
    for l in copy_lines.iter().take(show) {
        println!("{l}");
    }
    println!("RAM code that changed: {} runs", r.ram_changed.len());
    for c in r.ram_changed.iter().take(show) {
        println!("  0x{:08x}..0x{:08x}", c.start, c.end);
    }
    let ram_bytes: u32 = r.ram_undiscovered.iter().map(|c| c.end - c.start).sum();
    println!(
        "RAM code run, not discovered: {} runs, {} bytes executed ({} span)",
        r.ram_undiscovered.len(),
        r.ram_undiscovered_bytes,
        ram_bytes
    );
    let mut ram = r.ram_undiscovered.clone();
    ram.sort_by_key(|c| std::cmp::Reverse(c.end - c.start));
    for c in ram.iter().take(show) {
        println!(
            "  0x{:08x}..0x{:08x} {}",
            c.start,
            c.end,
            c.rom_src
                .map(|s| format!("copy of ROM 0x{s:08x}"))
                .unwrap_or("not found in ROM".into())
        );
    }
    println!(
        "flash library stack routines (hardware-library calls, not holes): {} routines, {} places",
        r.flash_routines(),
        r.flash.len()
    );
    for f in r.flash.iter().take(show) {
        println!(
            "  0x{:08x} {} ({} bytes): {}",
            f.target,
            f.routine.name(),
            f.len,
            f.routine.call()
        );
    }
    let mut cpu: BTreeMap<(CpuWhere, holes::CpuKind), (usize, usize)> = BTreeMap::new();
    for c in &r.cpu {
        let e = cpu.entry((c.place, c.kind)).or_default();
        e.0 += 1;
        e.1 += c.reached as usize;
    }
    println!("CPU state: {} hard", r.hard_cpu());
    for ((p, k), (n, reached)) in &cpu {
        println!("  {:?} {}: {n} ({reached} reached)", p, k.text());
    }
    for c in r
        .cpu
        .iter()
        .filter(|c| c.place == CpuWhere::Other)
        .take(show)
    {
        println!(
            "  0x{:08x} {}{}",
            c.at,
            c.kind.text(),
            if c.reached { " (reached)" } else { "" }
        );
    }
    println!(
        "BIOS calls the library doesn't implement: {} (and {} CPU-state or BIOS items in scanned code nothing ran, not counted)",
        r.bios.len(),
        r.scanned_skipped
    );
    for (a, n) in r.bios.iter().take(show) {
        println!("  0x{a:08x} swi 0x{n:02x}");
    }
    println!(
        "calls through a register to a non-entry: {} (of {} targets)",
        r.indirect_non_entry.len(),
        run.indirect_calls.len()
    );
    for (t, s) in r.indirect_non_entry.iter().take(show) {
        println!("  0x{t:08x} from 0x{s:08x}");
    }
    match &summaries {
        Some(s) => {
            println!("summaries: {} functions with holes", s.len());
            let mut kinds: Vec<_> = r.summary.iter().collect();
            kinds.sort_by_key(|(_, k)| std::cmp::Reverse(k.funcs.len()));
            for (kind, k) in kinds.iter().take(show) {
                println!(
                    "  {:5} {}{}",
                    k.funcs.len(),
                    kind,
                    if k.hard { " (hard)" } else { "" }
                );
            }
        }
        None => println!("summaries: not measured (--no-summaries)"),
    }
    println!(
        "busy-wait loops that call functions (not counted as hard: an open question): {}",
        r.busy_loops.len()
    );
    // Each should be a spin loop's back edge (`--timing native` lets time
    // pass in it, docs/research/busy-wait-loops.md); one that isn't
    // belongs in crates/runtime/spins.toml.
    let listed = codegen::spins::for_rom(rom);
    let rom_base = analysis::program::ROM_BASE;
    for (a, n) in r.busy_loops.iter().take(show) {
        let spin = analysis::idle::is_spin_at(&disc, rom, rom_base, a & !1, &listed);
        let what = if spin {
            "a spin edge"
        } else {
            "NOT a spin edge: list it in crates/runtime/spins.toml"
        };
        println!("  0x{a:08x}: {n} iterations in a row at most, {what}");
    }
    println!("pointer arithmetic the types don't explain: not measured until stage B");
    println!(
        "ROM: {} bytes reached, {} not discovered",
        r.rom_reached_bytes, r.rom_undiscovered_bytes
    );
    ExitCode::SUCCESS
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: gbaa-holes ROM [--frames N] [--press KEY@F | KEY@F-G/E]... [--no-summaries] [--no-run-roots] [--show N]"
    );
    ExitCode::from(2)
}
