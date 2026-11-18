//! `gbaa-text-guess`: works out a game's text encoding from its screens
//! (the crate's doc; docs/research/text-from-screen.md). `trace` lists
//! the cartridge ranges a run of frames reads as data, `reads` the reads
//! after each code taken from a string range.

use std::collections::BTreeMap;
use std::process::ExitCode;

use text_guess::score;

const USAGE: &str = "usage: gbaa-text-guess find ROM [--unit N] [--relative] LINE...
       gbaa-text-guess trace ROM --frames A-B [--press KEY@F[+N]]...
       gbaa-text-guess reads ROM STRINGS --frames A-B [--press ...]...
       gbaa-text-guess font ROM STRINGS --frames A-B [--unit N] [--press ...]...
       gbaa-text-guess glyphs ROM BASE,STRIDE,WIDE,TALL,ROW CODES OUT.png
       gbaa-text-guess unknown ROM TABLE.tbl [--unit N] [--min N]
       gbaa-text-guess score ROM TABLE.tbl [--min N]
STRINGS and CODES are hex ranges LO-HI; BASE hex, the rest of a layout
decimal. docs/research/text-from-screen.md says how they fit together.";

fn read_rom(p: &str) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("{p}: {e}"))
}

/// A guessed Thingy table's codes.
fn guess_table(path: &str, unit: usize) -> Result<score::Codes, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let entries = codegen::text::parse_tbl(&text, unit).map_err(|e| format!("{path}{e}"))?;
    Ok(score::codes(&entries, unit))
}

/// The end a Thingy table names (`/FF00=`).
fn end_of(path: &str, unit: usize) -> Result<u32, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    text.lines()
        .find_map(|l| {
            l.strip_prefix('/')?
                .split_once('=')
                .map(|(h, _)| h.to_string())
        })
        .and_then(|h| u32::from_str_radix(&h, 16).ok())
        .filter(|_| unit > 0)
        .ok_or(format!("{path}: no end (`/FF00=`)"))
}

/// Codes as a table prints them: unknown ones as `{XXXX}`.
fn show(cs: &[u32], t: &score::Codes) -> String {
    cs.iter()
        .map(|c| match score::lookup(t, *c) {
            Some(Some(ch)) => ch.replace('\n', "\\n"),
            _ => format!("{{{c:04X}}}"),
        })
        .collect()
}

/// `LO-HI`, two hex addresses.
fn range(s: &str) -> Result<(u32, u32), String> {
    let hex = |h: &str| u32::from_str_radix(h.trim_start_matches("0x"), 16).ok();
    s.split_once('-')
        .and_then(|(a, b)| Some((hex(a)?, hex(b)?)))
        .ok_or(format!("bad range {s:?}: want LO-HI in hex"))
}

fn run(args: &[String]) -> Result<(), String> {
    let mut unit = 2;
    let mut pos = Vec::new();
    let mut frames = (0u32, 0u32);
    let mut min = 4;
    let mut relative = false;
    let mut presses = Vec::new();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--unit" => {
                unit = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|u| matches!(u, 1 | 2))
                    .ok_or("--unit wants 1 or 2")?
            }
            "--relative" => relative = true,
            "--min" => {
                min = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--min wants a count")?
            }
            "--frames" => {
                frames = it
                    .next()
                    .and_then(|v| v.split_once('-'))
                    .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                    .ok_or("--frames wants A-B")?
            }
            "--press" => presses.push(
                it.next()
                    .and_then(|v| text_guess::trace::parse_press(v))
                    .ok_or("--press wants KEY@F or KEY@F+N")?,
            ),
            _ => pos.push(a.clone()),
        }
    }
    match args.first().map(String::as_str) {
        Some("find") if pos.len() >= 2 => {
            let rom = read_rom(&pos[0])?;
            for line in &pos[1..] {
                let fits = if relative {
                    text_guess::relative(&rom, line, unit)
                } else {
                    text_guess::isomorphs(&rom, line, unit)
                };
                println!("{line:?}: {} fits", fits.len());
                for f in fits.iter().take(60) {
                    let m: Vec<String> = f
                        .map
                        .iter()
                        .map(|(c, v)| format!("{c:?}={v:0w$X}", w = 2 * unit))
                        .collect();
                    let caps: Vec<String> = f
                        .initials
                        .iter()
                        .map(|(c, v)| format!("{c}={v:0w$X}", w = 2 * unit))
                        .collect();
                    println!("  {:#09x}: {}", 0x0800_0000 + f.offset, m.join(" "));
                    if !caps.is_empty() {
                        println!("    initials: {}", caps.join(" "));
                    }
                }
            }
            Ok(())
        }
        Some("trace") if pos.len() == 1 => {
            let rom: &'static [u8] = Box::leak(read_rom(&pos[0])?.into_boxed_slice());
            let loads = text_guess::trace::record(rom, presses, frames.0, frames.1);
            // Merged ranges of the cartridge the frames read as data.
            let mut addrs: Vec<(u32, u32)> = loads.iter().map(|l| (l.addr, l.width)).collect();
            addrs.sort_unstable();
            let mut ranges: Vec<(u32, u32, usize)> = Vec::new();
            for (a, w) in addrs {
                match ranges.last_mut() {
                    Some(r) if a <= r.1 + 32 => {
                        r.1 = r.1.max(a + w);
                        r.2 += 1;
                    }
                    _ => ranges.push((a, a + w, 1)),
                }
            }
            for (a, b, n) in ranges {
                println!("{a:#010x}-{b:#010x} {:6} bytes {n:8} loads", b - a);
            }
            Ok(())
        }
        Some("reads") if pos.len() == 2 => {
            let rom: &'static [u8] = Box::leak(read_rom(&pos[0])?.into_boxed_slice());
            let strings = range(&pos[1])?;
            let loads = text_guess::trace::record(rom, presses, frames.0, frames.1);
            let mut last = None;
            for l in &loads {
                if (strings.0..strings.1).contains(&l.addr) {
                    println!(
                        "f{} string {:#010x} = {:0w$x}",
                        l.frame,
                        l.addr,
                        l.value,
                        w = 2 * l.width as usize
                    );
                    last = None;
                } else if l.addr >> 24 >= 8 {
                    let a = l.addr & !31;
                    if last != Some(a) {
                        println!("    {a:#010x} +{:x} w{}", l.addr - a, l.width);
                        last = Some(a);
                    }
                }
            }
            Ok(())
        }
        Some("font") if pos.len() == 2 => {
            let rom: &'static [u8] = Box::leak(read_rom(&pos[0])?.into_boxed_slice());
            let strings = range(&pos[1])?;
            let loads = text_guess::trace::record(rom, presses, frames.0, frames.1);
            let reads = text_guess::trace::code_reads(&loads, strings, unit as u32);
            println!(
                "{} codes read: {}",
                reads.len(),
                reads
                    .iter()
                    .map(|(c, _)| format!("{c:x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            for f in text_guess::fit_font(&reads, 3).iter().take(12) {
                println!(
                    "base {:#010x} stride {:3}: {} codes",
                    f.base, f.stride, f.codes
                );
            }
            Ok(())
        }
        Some("glyphs") if pos.len() == 4 => {
            // ROM LAYOUT CODES OUT: LAYOUT is base,stride,wide,tall,row
            // (hex base, the rest decimal), CODES a hex range LO-HI.
            let rom = read_rom(&pos[0])?;
            let v: Vec<&str> = pos[1].split(',').collect();
            let num = |i: usize| -> Result<u32, String> {
                v.get(i)
                    .and_then(|s| s.parse().ok())
                    .ok_or(format!("bad layout {:?}", pos[1]))
            };
            let base = v
                .first()
                .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
            let l = text_guess::glyph::Layout {
                base: base.ok_or(format!("bad layout {:?}", pos[1]))?,
                stride: num(1)?,
                wide: num(2)?,
                tall: num(3)?,
                row: num(4)?,
            };
            let (lo, hi) = range(&pos[2])?;
            let codes: Vec<u32> = (lo..hi).collect();
            let (w, h, px) = text_guess::glyph::sheet(&rom, l, &codes, 16, 3);
            text_guess::glyph::write_png(std::path::Path::new(&pos[3]), w, h, &px)
        }
        Some("unknown") if pos.len() == 2 => {
            let rom = read_rom(&pos[0])?;
            let guess = guess_table(&pos[1], unit)?;
            let end = end_of(&pos[1], unit)?;
            let found = score::strings(&rom, unit, end, &guess, &|_| true, min);
            println!("{} strings", found.len());
            let mut counts: BTreeMap<u32, (usize, String)> = BTreeMap::new();
            for (at, cs) in &found {
                for c in cs {
                    if score::lookup(&guess, *c).is_none() {
                        let e = counts.entry(*c).or_insert((0, String::new()));
                        e.0 += 1;
                        if e.1.is_empty() {
                            e.1 = format!("{:#010x}: {}", 0x0800_0000 + at, show(cs, &guess));
                        }
                    }
                }
            }
            for (c, (n, eg)) in counts {
                println!("{c:04X} x{n}  {eg}");
            }
            Ok(())
        }
        Some("score") if pos.len() == 2 => {
            let rom = read_rom(&pos[0])?;
            let sha1: String = runtime::sha1::sha1(&rom)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let (unit, end, truth) = score::real_table(&sha1)?;
            let guess = guess_table(&pos[1], unit)?;
            let s = score::score(&rom, unit, end, &truth, &guess, min);
            let pc = |a: usize, b: usize| 100.0 * a as f64 / b.max(1) as f64;
            println!(
                "strings: {} of {} right ({:.1}%)",
                s.strings_right,
                s.strings,
                pc(s.strings_right, s.strings)
            );
            println!(
                "characters: {} of {} right ({:.1}%)",
                s.chars_right,
                s.chars,
                pc(s.chars_right, s.chars)
            );
            println!(
                "codes used: {} of {} right ({:.1}%)",
                s.codes_right,
                s.codes,
                pc(s.codes_right, s.codes)
            );
            let mut wrong: Vec<_> = s.wrong.iter().collect();
            wrong.sort_by_key(|(_, w)| std::cmp::Reverse(w.2));
            for (c, (want, got, n)) in wrong {
                let got = match got {
                    None => "none".to_string(),
                    Some(g) => format!("{g:?}"),
                };
                println!("  {c:04X} x{n}: real {want:?}, guess {got}");
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
