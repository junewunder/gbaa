//! `gbaa-dis ROM ADDR [LEN] [--arm] [--names FILE]` (`just dis`): the
//! instructions at ROM address ADDR, LEN bytes (default 0x40), one a line
//! as `addr: hex  text` in objdump's UAL form (`armv4t::text`). Thumb
//! unless `--arm`; an odd ADDR is Thumb with the bit stripped, like a `bx`
//! target. A Thumb `bl`'s two halves print as one `bl 0x…` line.
//! `--names FILE`, a names map for this ROM (`$GBAA_DATA/names/<game>.toml`,
//! `codegen::names`), prints a `<name>:` line at each named function's
//! start and ` <name>` after a branch to one. ROM addresses only
//! (0x08000000 and up; the mirrors at 0x0a/0x0c too).

use armv4t::decode::{Instr, decode_arm, decode_thumb, pair_thumb_bl};
use armv4t::text::Bl;
use codegen::names::SourceMap;
use std::process::ExitCode;

const USAGE: &str = "usage: gbaa-dis ROM ADDR [LEN] [--arm] [--names FILE]";

/// The listing of `bytes`, loaded at `base`, in Thumb or ARM. `name` names
/// a function key (bit 0 set: Thumb), as a names map's `[functions]` does.
fn disasm(
    bytes: &[u8],
    base: u32,
    thumb: bool,
    name: &dyn Fn(u32) -> Option<String>,
) -> Vec<String> {
    // A function's name at `addr` in either state, Thumb's key first.
    let named = |addr: u32, thumb: bool| {
        let (first, second) = if thumb {
            (addr | 1, addr)
        } else {
            (addr, addr | 1)
        };
        name(first).or_else(|| name(second))
    };
    let mut out = Vec::new();
    let step = if thumb { 2 } else { 4 };
    let mut i = 0;
    while i + step <= bytes.len() {
        let addr = base.wrapping_add(i as u32);
        if let Some(n) = name(if thumb { addr | 1 } else { addr }) {
            out.push(format!("{n}:"));
        }
        let half = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let (hex, text, len) = if thumb {
            let h = half(i);
            match (i + 4 <= bytes.len())
                .then(|| pair_thumb_bl(h, half(i + 2), addr))
                .flatten()
            {
                Some(t) => (format!("{h:04x} {:04x}", half(i + 2)), Bl(t).to_string(), 4),
                None => (format!("{h:04x}"), text(decode_thumb(h, addr), true), 2),
            }
        } else {
            let w = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
            (format!("{w:08x}"), text(decode_arm(w, addr), false), 4)
        };
        // A branch's target is its last word, bare hex.
        let target = text
            .rsplit(' ')
            .next()
            .and_then(|t| t.strip_prefix("0x"))
            .and_then(|t| u32::from_str_radix(t, 16).ok())
            .filter(|_| text.starts_with('b'));
        let label = target
            .and_then(|t| named(t, thumb))
            .map_or(String::new(), |n| format!(" <{n}>"));
        out.push(format!("{addr:08x}: {hex:<9}  {text}{label}"));
        i += len;
    }
    out
}

fn text(i: Instr, thumb: bool) -> String {
    match i {
        Instr::Undefined { .. } => "(undefined)".to_string(),
        i => i.text(thumb).to_string(),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut pos, mut arm, mut names) = (Vec::new(), false, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--arm" => arm = true,
            "--names" => match it.next() {
                Some(f) => names = Some(f.clone()),
                None => return fail(USAGE),
            },
            _ => pos.push(a.clone()),
        }
    }
    let num = |s: &str| {
        let h = s
            .strip_prefix("0x")
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(s);
        u32::from_str_radix(&h.replace('_', ""), 16).ok()
    };
    let (rom_path, addr, len) = match pos.as_slice() {
        [r, a] => (r, num(a), Some(0x40)),
        [r, a, l] => (r, num(a), num(l)),
        _ => return fail(USAGE),
    };
    let (Some(addr), Some(len)) = (addr, len) else {
        return fail("gbaa-dis: ADDR and LEN are hex (0x08000000)");
    };
    let thumb = !arm || addr & 1 == 1;
    let addr = addr & !1;
    if !(0x0800_0000..0x0e00_0000).contains(&addr) {
        return fail("gbaa-dis: only ROM addresses (0x08000000-0x0dffffff)");
    }
    let rom = match std::fs::read(rom_path) {
        Ok(r) => r,
        Err(e) => return fail(&format!("gbaa-dis: {rom_path}: {e}")),
    };
    let off = (addr & 0x01ff_ffff) as usize;
    if off >= rom.len() {
        return fail(&format!(
            "gbaa-dis: {addr:#010x} is past the ROM's end ({:#x} bytes)",
            rom.len()
        ));
    }
    let end = (off + len as usize).min(rom.len());
    let map = match names {
        None => None,
        Some(f) => match SourceMap::load(&codegen::names::sha1_hex(&rom), &[&f]) {
            Ok(m) => Some(m),
            Err(e) => return fail(&format!("gbaa-dis: {e}")),
        },
    };
    let name = |k: u32| map.as_ref()?.functions.get(&k).map(|i| i.name.clone());
    for l in disasm(&rom[off..end], addr, thumb, &name) {
        println!("{l}");
    }
    ExitCode::SUCCESS
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: u32) -> Option<String> {
        None
    }

    #[test]
    fn thumb_with_a_joined_bl() {
        // push {r4, lr}; bl to 0x08000008; pop {r4}; bx lr
        let b = [0x10, 0xb5, 0x00, 0xf0, 0x01, 0xf8, 0x10, 0xbc, 0x70, 0x47];
        assert_eq!(
            disasm(&b, 0x0800_0000, true, &none),
            [
                "08000000: b510       push {r4, lr}",
                "08000002: f000 f801  bl 0x08000008",
                "08000006: bc10       pop {r4}",
                "08000008: 4770       bx lr",
            ]
        );
    }

    #[test]
    fn arm() {
        // mov r0, #1; bx lr
        let b = [0x01, 0x00, 0xa0, 0xe3, 0x1e, 0xff, 0x2f, 0xe1];
        assert_eq!(
            disasm(&b, 0x0800_0100, false, &none),
            [
                "08000100: e3a00001   mov r0, #1",
                "08000104: e12fff1e   bx lr"
            ]
        );
    }

    #[test]
    fn names_label_starts_and_calls() {
        let b = [0x00, 0xf0, 0x00, 0xf8, 0x70, 0x47];
        let name = |k: u32| match k {
            0x0800_0001 => Some("Caller".to_string()),
            0x0800_0005 => Some("Leaf".to_string()),
            _ => None,
        };
        assert_eq!(
            disasm(&b, 0x0800_0000, true, &name),
            [
                "Caller:",
                "08000000: f000 f800  bl 0x08000004 <Leaf>",
                "Leaf:",
                "08000004: 4770       bx lr",
            ]
        );
    }

    #[test]
    fn a_lone_bl_half_at_the_end() {
        let b = [0x00, 0xf0];
        let l = disasm(&b, 0x0800_0000, true, &none);
        assert_eq!(l.len(), 1);
        assert!(l[0].starts_with("08000000: f000       bl.hi"), "{l:?}");
    }
}
