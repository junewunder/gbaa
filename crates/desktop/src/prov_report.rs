//! The provenance report (assets/provenance; `runtime::prov`): a build with
//! the `provenance` feature writes, beside each `--dump-at` PNG,
//! `frame-N.prov.txt` (where palette, VRAM and OAM's bytes came from, now
//! and in every store so far) and `frame-N.blobs.tsv` (the ROM ranges and
//! compressed blobs that reached them so far).

use runtime::Machine;
use runtime::platform::Platform;
use runtime::prov::{self, Region, via};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Copied ROM bytes this close together are one blob.
const GAP: u32 = 16;

/// A tag's class as words: where from, then the steps, lowest bit first.
fn class_name(class: usize) -> String {
    let from = if class & 128 != 0 { "rom" } else { "no-rom" };
    let steps: Vec<&str> = (0..7)
        .filter(|b| class >> b & 1 != 0)
        .map(|b| via::NAMES[b])
        .collect();
    format!("{from} {}", steps.join("+"))
}

fn class_of(tag: u32) -> usize {
    prov::steps(tag) as usize | if prov::origin(tag).is_some() { 128 } else { 0 }
}

/// Runs of set bits, joined across gaps under [`GAP`]: (start, bytes).
fn runs(bits: impl Iterator<Item = u32>) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for o in bits {
        match out.last_mut() {
            Some((s, n)) if o < *s + *n + GAP => *n = o + 1 - *s,
            _ => out.push((o, 1)),
        }
    }
    out
}

/// The ROM offsets set in a bitmap of [`prov::reached`]: `which` 0 for
/// copied, 1 for blobs' first bytes, 2 for guessed tables.
fn reached(which: usize) -> impl Iterator<Item = u32> {
    (0..prov::REACHED_WORDS).flat_map(move |i| {
        let w = prov::reached(i)[which];
        (0..32)
            .filter(move |b| w >> b & 1 != 0)
            .map(move |b| (i * 32 + b) as u32)
    })
}

/// One memory's bytes now, by class: (zero bytes, non-zero by class).
fn snapshot(mem: &[u8], base: u32) -> (usize, BTreeMap<usize, usize>) {
    let mut zero = 0;
    let mut by = BTreeMap::new();
    for (i, &b) in mem.iter().enumerate() {
        if b == 0 {
            zero += 1;
            continue;
        }
        let t = prov::tag_at(base + i as u32);
        *by.entry(if t == 0 { usize::MAX } else { class_of(t) })
            .or_default() += 1;
    }
    (zero, by)
}

/// Whether `tag` is a copy of ROM bytes (decompressed or not), not computed.
fn copied_from_rom(tag: u32) -> bool {
    prov::origin(tag).is_some() && prov::steps(tag) & via::DERIVED == 0
}

/// For a memory's computed non-zero bytes, the RAM buffer (EWRAM or
/// IWRAM, from a halfword-aligned base) whose ROM-copied bytes at the same
/// offsets equal the most of them: (its address, bytes matched). A palette
/// faded in is computed, but once the fade ends it equals the unfaded copy.
fn by_value<P: Platform>(m: &Machine<P>, mem: &[u8], base: u32) -> Option<(u32, usize)> {
    let computed: Vec<usize> = (0..mem.len())
        .filter(|&i| mem[i] != 0 && !copied_from_rom(prov::tag_at(base + i as u32)))
        .collect();
    let rams: [(&[u8], u32); 2] = [
        (&m.bus.ewram[..], 0x0200_0000),
        (&m.bus.iwram[..], 0x0300_0000),
    ];
    let mut best = None;
    for (ram, at) in rams {
        for b in (0..=ram.len() - mem.len()).step_by(2) {
            let n = computed
                .iter()
                .filter(|&&i| ram[b + i] == mem[i])
                .filter(|&&i| copied_from_rom(prov::tag_at(at + (b + i) as u32)))
                .count();
            if n > best.map_or(0, |(_, k)| k) {
                best = Some((at + b as u32, n));
            }
        }
    }
    best
}

fn pct(n: usize, of: usize) -> f64 {
    if of == 0 {
        0.0
    } else {
        100.0 * n as f64 / of as f64
    }
}

/// The report for frame `frame`, and the blobs TSV.
pub fn report<P: Platform>(m: &Machine<P>, frame: u64) -> (String, String) {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "provenance at frame {frame}\n\nnow (non-zero bytes by class):"
    );
    let mems: [(&str, &[u8], u32); 3] = [
        ("vram", &m.bus.video.vram, 0x0600_0000),
        ("palette", &m.bus.video.palette, 0x0500_0000),
        ("oam", &m.bus.video.oam, 0x0700_0000),
    ];
    for (name, mem, base) in mems {
        let (zero, by) = snapshot(mem, base);
        let nonzero = mem.len() - zero;
        let sum = |f: fn(usize) -> bool| -> usize {
            by.iter()
                .filter(|(c, _)| **c != usize::MAX && f(**c))
                .map(|(_, n)| n)
                .sum()
        };
        const DERIVED: usize = via::DERIVED as usize;
        let rom = sum(|c| c & 128 != 0 && c & DERIVED == 0);
        let tables = sum(|c| c & 128 != 0 && c & DERIVED != 0);
        let _ = writeln!(
            s,
            "  {name}: {} bytes, {zero} zero, {nonzero} non-zero; {rom} ({:.1}%) from rom, \
             {tables} ({:.1}%) computed with a rom table guessed",
            mem.len(),
            pct(rom, nonzero),
            pct(tables, nonzero)
        );
        for (c, n) in &by {
            let what = if *c == usize::MAX {
                "never stored".into()
            } else {
                class_name(*c)
            };
            let _ = writeln!(s, "    {n:>7} {:>5.1}%  {what}", pct(*n, nonzero));
        }
        if name == "palette"
            && let Some((at, n)) = by_value(m, mem, base)
        {
            let _ = writeln!(
                s,
                "    of the rest, {n} ({:.1}%) equal rom-copied bytes at the same offsets from \
                 {at:#010x}",
                pct(n, nonzero)
            );
        }
    }
    let _ = writeln!(s, "\nevery store so far, bytes by class:");
    for (name, r) in [
        ("vram", Region::Vram),
        ("palette", Region::Palette),
        ("oam", Region::Oam),
        ("fifo", Region::Fifo),
    ] {
        let all: u64 = (0..256).map(|c| prov::written(r, c) as u64).sum();
        let _ = writeln!(s, "  {name}: {all}");
        for c in 0..256 {
            let n = prov::written(r, c) as u64;
            if n != 0 {
                let _ = writeln!(
                    s,
                    "    {n:>10} {:>5.1}%  {}",
                    100.0 * n as f64 / all as f64,
                    class_name(c)
                );
            }
        }
    }
    let copied = runs(reached(0));
    let packed: Vec<u32> = reached(1).collect();
    let tables = runs(reached(2));
    let bytes: u32 = copied.iter().map(|r| r.1).sum();
    let _ = writeln!(
        s,
        "\nsource blobs so far: {} copied ranges ({bytes} bytes, ranges joined across gaps under \
         {GAP}), {} compressed blobs; {} guessed table ranges",
        copied.len(),
        packed.len(),
        tables.len()
    );
    let mut tsv = String::from("kind\tstart\tbytes\n");
    for (o, n) in copied {
        let _ = writeln!(tsv, "copied\t{:#09x}\t{n}", 0x0800_0000 + o);
    }
    for o in packed {
        let _ = writeln!(tsv, "packed\t{:#09x}\t", 0x0800_0000 + o);
    }
    for (o, n) in tables {
        let _ = writeln!(tsv, "table\t{:#09x}\t{n}", 0x0800_0000 + o);
    }
    (s, tsv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_join_close_bits() {
        assert_eq!(
            runs([1, 2, 3, 10, 40, 41].into_iter()),
            vec![(1, 10), (40, 2)]
        );
    }

    #[test]
    fn classes_read_as_words() {
        assert_eq!(class_name(128 | 1 | 16), "rom cpu+lz77");
        assert_eq!(class_name(1), "no-rom cpu");
    }
}
