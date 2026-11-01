//! `insn-text-check rom OBJDUMP ROM`: every instruction discovery finds in a
//! GBA ROM, our text against objdump's (`-m armv4t`, ARM and
//! `force-thumb` dumps of the whole image, looked up by address).
//!
//! `insn-text-check image OBJDUMP ROM arm|thumb`: every word (halfword) of
//! the whole image, code or not, in one state.
//!
//! `insn-text-check elf OBJDUMP ELF [--every N]`: every Nth instruction
//! line of `objdump -d ELF` (default every one), which the ELF's mapping
//! symbols split into ARM and Thumb.
//!
//! Prints the tally and the first mismatches; exits 1 on any mismatch.

use std::collections::HashMap;
use std::process::{Command, exit};

use insn_text_check::{Line, Tally, parse_line};

fn dump(objdump: &str, args: &[&str]) -> Vec<Line> {
    let out = Command::new(objdump)
        .args(args)
        .output()
        .unwrap_or_else(|e| fail(&format!("running {objdump}: {e}")));
    if !out.status.success() {
        fail(&format!(
            "{objdump} {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_line)
        .collect()
}

fn fail(msg: &str) -> ! {
    eprintln!("insn-text-check: {msg}");
    exit(2)
}

fn rom(objdump: &str, path: &str) -> Tally {
    let bytes = std::fs::read(path).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
    let disc = analysis::discovery::discover(bytes);
    let base = [
        "-D",
        "-b",
        "binary",
        "-m",
        "armv4t",
        "--adjust-vma=0x08000000",
    ];
    let index = |lines: Vec<Line>| -> HashMap<u32, Line> {
        lines.into_iter().map(|l| (l.addr, l)).collect()
    };
    let arm = index(dump(objdump, &[&base[..], &[path]].concat()));
    let thumb = index(dump(
        objdump,
        &[&base[..], &["-M", "force-thumb", path]].concat(),
    ));
    let mut t = Tally::default();
    for (&start, code) in &disc.code {
        let (lines, step) = if code.thumb { (&thumb, 2) } else { (&arm, 4) };
        let mut a = start;
        while a < start + code.size {
            match lines.get(&a) {
                Some(l) => {
                    t.add(l);
                    // A `bl` line covers both halves.
                    a += if matches!(l.enc, insn_text_check::Enc::Pair(..)) {
                        4
                    } else {
                        step
                    };
                }
                // objdump read the halfword before as the first of a
                // 32-bit pair (data before code): dump this one alone.
                None => {
                    let (from, to) = (format!("--start-address={a:#x}"), a + step);
                    let to = format!("--stop-address={to:#x}");
                    let state: &[&str] = if code.thumb {
                        &["-M", "force-thumb"]
                    } else {
                        &[]
                    };
                    match dump(objdump, &[&base[..], state, &[&from, &to, path]].concat()).first() {
                        Some(l) if l.addr == a => t.add(l),
                        _ => t.missing.push(a),
                    }
                    a += step;
                }
            }
        }
    }
    t
}

fn image(objdump: &str, path: &str, thumb: bool) -> Tally {
    let mut args = vec![
        "-D",
        "-b",
        "binary",
        "-m",
        "armv4t",
        "--adjust-vma=0x08000000",
    ];
    if thumb {
        args.extend(["-M", "force-thumb"]);
    }
    args.push(path);
    let mut t = Tally::default();
    for l in dump(objdump, &args) {
        t.add(&l);
    }
    t
}

fn elf(objdump: &str, path: &str, every: usize) -> Tally {
    let mut t = Tally::default();
    for l in dump(objdump, &["-d", "-m", "armv4t", path])
        .iter()
        .step_by(every)
    {
        t.add(l);
    }
    t
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: insn-text-check rom OBJDUMP ROM | image OBJDUMP ROM arm|thumb | elf OBJDUMP ELF [--every N]";
    let t = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["rom", od, path] => rom(od, path),
        ["elf", od, path] => elf(od, path, 1),
        ["image", od, path, state @ ("arm" | "thumb")] => image(od, path, state == "thumb"),
        ["elf", od, path, "--every", n] => elf(od, path, n.parse().unwrap_or_else(|_| fail(usage))),
        _ => fail(usage),
    };
    println!(
        "matched {}, mismatched {}, should-be bits differ {}; not compared: objdump data/undefined {}, our undefined {}, Thumb-2 pairs {}, no objdump line {}",
        t.matched,
        t.mismatched.len(),
        t.should_be.len(),
        t.data,
        t.undefined,
        t.not_v4t,
        t.missing.len()
    );
    if !t.missing.is_empty() {
        println!(
            "no objdump line at {:#010x?}",
            &t.missing[..t.missing.len().min(10)]
        );
    }
    for (l, ours, theirs) in t.should_be.iter().take(10) {
        println!(
            "should-be bits {:#010x} {:?}: ours {ours:?}, objdump {theirs:?}",
            l.addr, l.enc
        );
    }
    for (l, ours, theirs) in t.mismatched.iter().take(40) {
        println!(
            "{:#010x} {:?}: ours {ours:?}, objdump {theirs:?} (raw {:?})",
            l.addr, l.enc, l.text
        );
    }
    if !t.mismatched.is_empty() {
        exit(1);
    }
}
