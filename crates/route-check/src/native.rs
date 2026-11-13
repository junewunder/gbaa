//! `route-check --native NATIVE PLAY ROUTE [--replaced FILE] [--holes
//! FILE] [--name NAME] [--packed BIN] [--planted BIN]`: a native program against its play build
//! (docs/native-port-design.md, section 7; `just native-check`).
//!
//! Runs PLAY (the ROM's recompiled build, at `--timing native`; then again
//! with `--interp` for the lowest sp, which only the interpreter keeps,
//! and whose records must be the compiled run's) and NATIVE
//! (the native program's check build, `--layout guest`) on the route's
//! presses for its frames, each writing a record of every frame as it is
//! presented (`--native-record`, crates/native-record), and compares them
//! frame by frame:
//!
//! - the picture (its hash);
//! - EWRAM and IWRAM, but the guest stack (from the lowest `sp` the play
//!   build reached, which its interpreter reports, to IWRAM's end: the
//!   stacks and the BIOS's words above them), the destinations of code copies
//!   (`Discovery::copies` that a RAM entry lies in: the native program runs
//!   that code as Rust and copies none of it), the destinations of the
//!   lifter's holes file's `[[copy]]` entries (the code a game copies to
//!   RAM itself, which the native program likewise runs as Rust and never
//!   writes; the file is `--holes FILE`, else found as `just lift` finds
//!   it, by the game's name: `holes/<name>.toml`, else
//!   `mods/<name>/holes.toml`, the name `--name NAME`'s or the ROM's file
//!   name lowercased) and the ranges `--replaced` FILE names
//!   (`[[replaced]]`: the globals of a library the native program replaces,
//!   each with its why);
//! - the I/O registers the library models ([`IO`]): the display's,
//!   `DISPSTAT`'s writable bits, `VCOUNT`, the sound registers (with
//!   `SOUNDCNT_X`'s channel status, which the shared mixer sets), wave
//!   RAM, the FIFOs' last stores, the timers' reloads and controls, the
//!   serial port's registers and `KEYCNT` (kept as stored, with no
//!   partner), `IE`, `IF`, `WAITCNT`, `IME`. Not DMA or `KEYINPUT` (latched
//!   after the present, when the play build records);
//! - palette, VRAM and OAM, but FILE's ranges there too (a replaced
//!   library's state kept in VRAM: agb's tile allocator's free list);
//! - each frame's audio (its samples' count and hash), when both make
//!   some: the library mixes with the play build's own sound path, so a
//!   native program records its samples as the play build does. Play's
//!   silence equals a program's none (a host that records none); play's
//!   sound against none fails unless `--no-audio WHY` says why it may
//!   pass, and the pass line says "audio NOT compared".
//!
//! and fails at the first difference, naming the frame, the region and the
//! byte (address, play's value, native's). `--packed BIN`: the same
//! program's play layout must draw NATIVE's frames (frame hashes), and
//! its records must be NATIVE's, nothing skipped (RAM as the guest lays it
//! out, which a packed program records).
//! `--planted BIN` (any number): a build with a planted fault (one wrong
//! store, or an old behaviour a lifter flag brings back), which must
//! differ; its first difference is printed. A lowest sp outside IWRAM
//! fails: its skip would cover other RAM.
//!
//! Each binary may be followed by its own flags, space-separated, as
//! `--same-states` takes them.

use crate::route::{Route, Vars};
use crate::{Stop, Stop::*, file, tail};
use native_record::{Frame, Reader, Region};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The I/O halfwords compared, `start..end` (even), the bits of each
/// compared, and what they are.
pub const IO: &[(u32, u32, u16, &str)] = &[
    (0x000, 0x004, 0xFFFF, "DISPCNT"),
    (0x004, 0x006, 0xFFFF, "DISPSTAT (writable bits)"),
    (0x006, 0x008, 0xFFFF, "VCOUNT"),
    (0x008, 0x056, 0xFFFF, "the display's registers"),
    (0x060, 0x084, 0xFFFF, "the sound registers"),
    (0x084, 0x086, 0xFFFF, "SOUNDCNT_X"),
    (0x086, 0x0A0, 0xFFFF, "SOUNDBIAS and wave RAM"),
    (0x0A0, 0x0A8, 0xFFFF, "the FIFOs' last stores"),
    (0x100, 0x110, 0xFFFF, "the timers' reloads and controls"),
    (0x120, 0x130, 0xFFFF, "the serial registers"),
    (0x132, 0x160, 0xFFFF, "KEYCNT, RCNT and the JOY bus"),
    (0x200, 0x204, 0xFFFF, "IE and IF"),
    (0x204, 0x206, 0xFFFF, "WAITCNT"),
    (0x208, 0x20A, 0xFFFF, "IME"),
];

/// IWRAM's start and end: the stacks lie below its end.
const IWRAM_BASE: u32 = 0x0300_0000;
const IWRAM_END: u32 = 0x0300_8000;

/// A range of RAM the check skips, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Skip {
    pub start: u32,
    pub end: u32,
    pub why: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplacedFile {
    replaced: Vec<Replaced>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Replaced {
    start: u32,
    end: u32,
    why: String,
}

/// The options after `--native NATIVE PLAY ROUTE`.
#[derive(Default)]
pub struct Extra {
    pub replaced: Option<String>,
    /// `--holes FILE`: the lifter's holes file, whose `[[copy]]`
    /// destinations are skipped. Without it, found by the game's name.
    pub holes: Option<String>,
    /// `--name NAME`: the game's name, as `gbaa-lift --name` takes it, to
    /// find its holes file (default: the ROM's file name lowercased).
    pub name: Option<String>,
    pub packed: Option<String>,
    pub planted: Vec<String>,
    /// `--no-audio WHY`: the native program makes no audio and the run may
    /// pass without its audio compared, for this reason.
    pub no_audio: Option<String>,
}

pub fn parse_extra(rest: &[String]) -> Result<Extra, String> {
    let mut e = Extra::default();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let v = it.next().ok_or(format!("{a} needs a value"))?.clone();
        match a.as_str() {
            "--replaced" => e.replaced = Some(v),
            "--holes" => e.holes = Some(v),
            "--name" => e.name = Some(v),
            "--packed" => e.packed = Some(v),
            "--planted" => e.planted.push(v),
            "--no-audio" => e.no_audio = Some(v),
            _ => return Err(format!("unknown argument {a:?}")),
        }
    }
    Ok(e)
}

/// The whole check. Ok: what passed.
pub fn run(
    native: &str,
    play: &str,
    route: &str,
    extra: &Extra,
    vars: &Vars,
) -> Result<String, Stop> {
    let r = Route::read(Path::new(route), vars).map_err(Bad)?;
    if r.state.is_some() {
        return Err(Bad(format!(
            "{route} starts from a state: a native program starts at power-on"
        )));
    }
    if !r.rom.exists() {
        return Err(Bad(format!("ROM {} is missing", r.rom.display())));
    }
    let name = Path::new(route)
        .file_stem()
        .map_or(String::new(), |s| s.to_string_lossy().into_owned());
    let out = PathBuf::from("out/native-check").join(&name);
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    }
    std::fs::create_dir_all(&out).map_err(|e| Unrun(format!("{}: {e}", out.display())))?;
    let frames = r.frames();

    // What the native program never writes, known before any run: code
    // copies and replaced libraries. Printed now, so they show even when
    // a run fails.
    let rom = std::fs::read(&r.rom).map_err(|e| Unrun(format!("{}: {e}", r.rom.display())))?;
    let mut fixed = code_copies(rom);
    if let Some(f) = holes_file(Path::new(""), &r.rom, extra) {
        let text = std::fs::read_to_string(&f).map_err(|e| Bad(format!("{}: {e}", f.display())))?;
        let h = codegen::native::Holes::parse(&text)
            .map_err(|e| Bad(format!("{}: {e}", f.display())))?;
        fixed.extend(holes_copies(&h, &f));
    }
    if let Some(f) = &extra.replaced {
        fixed.extend(replaced(f)?);
    }
    for s in &fixed {
        println!(
            "native {name}: not compared {:#010x}..{:#010x}: {}",
            s.start, s.end, s.why
        );
    }

    runner(&out, "play", play, &r, &["--timing", "native"], true)?;
    // Only the interpreter keeps the lowest sp (runtime::Machine::low_sp):
    // the same binary again, interpreted, which must hold the same state.
    let interp_log = runner(
        &out,
        "interp",
        play,
        &r,
        &["--timing", "native", "--interp"],
        true,
    )?;
    let low_sp = lowest_sp(&interp_log).ok_or(Unrun(format!(
        "{play} --interp didn't report its lowest sp ({})",
        out.join("interp.log").display()
    )))?;
    runner(&out, "native", native, &r, &[], true)?;

    // The skip runs from the lowest sp to IWRAM's end: below IWRAM it
    // would take EWRAM with it (r13 used as a scratch register, or a stack
    // in EWRAM), which this check doesn't handle.
    if !(IWRAM_BASE..=IWRAM_END).contains(&low_sp) {
        return Err(Fail(format!(
            "the play build's lowest sp {low_sp:#010x} is outside IWRAM: the stack skip would \
             swallow other RAM"
        )));
    }
    let stack = Skip {
        start: low_sp,
        end: IWRAM_END,
        why: format!("the guest stack, from the lowest sp {low_sp:#010x}"),
    };
    println!(
        "native {name}: not compared {:#010x}..{:#010x}: {}",
        stack.start, stack.end, stack.why
    );
    let mut skips = vec![stack];
    skips.extend(fixed);

    compare(&out.join("play.rec"), &out.join("interp.rec"), &[], frames).map_err(|e| match e {
        Differ::Fail(f) => Fail(format!(
            "the play build's compiled code differs from its interpreter (so from itself): {f}"
        )),
        Differ::Unrun(f) => Unrun(f),
    })?;
    let (said, loud) = compare(
        &out.join("play.rec"),
        &out.join("native.rec"),
        &skips,
        frames,
    )
    .map_err(|e| match e {
        Differ::Fail(f) => Fail(format!("native differs from play: {f}")),
        Differ::Unrun(f) => Unrun(f),
    })?;
    // Sound the native program can't make: a fail, unless the run says
    // why it may pass without it.
    let said = match (loud, &extra.no_audio) {
        (0, _) => said,
        (_, Some(why)) => format!("{said} (--no-audio: {why})"),
        (n, None) => {
            return Err(Fail(format!(
                "the play build made sound in {n} frames and the native program none: {said}; \
                 give --no-audio WHY to pass without it"
            )));
        }
    };
    let mut parts = vec![format!("{frames} frames, {said}")];

    if let Some(p) = &extra.packed {
        runner(&out, "packed", p, &r, &[], true)?;
        let (a, b) = (
            read_hashes(&out.join("native-hashes.txt"))?,
            read_hashes(&out.join("packed-hashes.txt"))?,
        );
        if let Some(i) = (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i)) {
            return Err(Fail(format!(
                "the play layout draws differently from the guest layout at frame {}: {} and {}",
                i + 1,
                a.get(i).map_or("(none)", String::as_str),
                b.get(i).map_or("(none)", String::as_str)
            )));
        }
        // Its records too, whole: both are native programs, so nothing is
        // skipped (a byte the layout moved shows here first).
        compare(
            &out.join("native.rec"),
            &out.join("packed.rec"),
            &[],
            frames,
        )
        .map_err(|e| match e {
            Differ::Fail(f) => Fail(format!(
                "the play layout's record differs from the guest layout's: {f}"
            )),
            Differ::Unrun(f) => Unrun(f),
        })?;
        parts.push(format!(
            "the play layout draws the same {} frames, its records the same",
            a.len()
        ));
    }
    for (i, p) in extra.planted.iter().enumerate() {
        let run = match i {
            0 => "planted".to_string(),
            _ => format!("planted-{}", i + 1),
        };
        runner(&out, &run, p, &r, &[], true)?;
        let bin = p.split_whitespace().next().unwrap_or_default();
        match compare(
            &out.join("play.rec"),
            &out.join(format!("{run}.rec")),
            &skips,
            frames,
        ) {
            Ok(_) => {
                return Err(Fail(format!(
                    "the planted fault ({bin}) matched the play build: the check can't see it"
                )));
            }
            Err(Differ::Unrun(e)) => return Err(Unrun(e)),
            Err(Differ::Fail(f)) => {
                println!("native {name}: planted fault caught ({bin}): {f}");
                parts.push(format!("planted fault caught ({bin}: {f})"));
            }
        }
    }
    Ok(parts.join("; "))
}

/// Run `cmd` (a binary and its flags) along `r` into `out`: NAME.rec (if
/// `record`), NAME-hashes.txt, NAME.txt and NAME.log. A play build (given
/// `flags`) gets the ROM first. Returns its stderr.
fn runner(
    out: &Path,
    name: &str,
    cmd: &str,
    r: &Route,
    flags: &[&str],
    record: bool,
) -> Result<String, Stop> {
    let play = !flags.is_empty();
    let mut words = cmd.split_whitespace();
    let bin = words.next().ok_or(Bad(format!("{name}: no binary")))?;
    let mut c = Command::new("systemd-run");
    c.args([
        "--user",
        "--scope",
        "-q",
        "-p",
        "MemoryMax=3G",
        "-p",
        "MemorySwapMax=0",
    ])
    .arg(bin);
    if play {
        c.arg(&r.rom);
    }
    c.args(words)
        .args(["--headless", "--frames", &r.frames().to_string()]);
    if !r.input.is_empty() {
        c.args(["--input", &r.input]);
    }
    c.args(flags);
    if record {
        c.arg("--native-record")
            .arg(out.join(format!("{name}.rec")));
    }
    let (stdout, stderr) = (
        out.join(format!("{name}.txt")),
        out.join(format!("{name}.log")),
    );
    let status = c
        .arg("--frame-hashes")
        .arg(out.join(format!("{name}-hashes.txt")))
        .stdin(Stdio::null())
        .stdout(file(&stdout)?)
        .stderr(file(&stderr)?)
        .status()
        .map_err(|e| Unrun(format!("{bin}: {e}")))?;
    if !status.success() {
        return Err(Fail(format!(
            "{bin} exited with an error ({}: {})",
            stderr.display(),
            tail(&stderr)
        )));
    }
    std::fs::read_to_string(&stderr).map_err(|e| Unrun(format!("{}: {e}", stderr.display())))
}

/// gbaa-desktop's `lowest sp 0x…` line.
fn lowest_sp(log: &str) -> Option<u32> {
    let l = log
        .lines()
        .find_map(|l| l.strip_prefix("gbaa-desktop: lowest sp 0x"))?;
    u32::from_str_radix(l.trim(), 16).ok()
}

/// The copies of code out of ROM into IWRAM (agb's `.iwram`): the native
/// program runs that code as Rust functions and copies none of it. The
/// whole copy is skipped, so a variable copied with the code (an agb
/// `#[link_section = ".iwram"]` static) isn't compared: the skip says so.
/// Copies into EWRAM are compared whole (agb's `.ewram` carries `.data`).
fn code_copies(rom: Vec<u8>) -> Vec<Skip> {
    let d = analysis::discovery::discover(rom);
    d.copies
        .iter()
        .filter(|&&(_, dst, n)| {
            (IWRAM_BASE..IWRAM_END).contains(&dst)
                && d.ram_entries
                    .iter()
                    .any(|&e| (dst..dst + n).contains(&(e & !1)))
        })
        .map(|&(src, dst, n)| Skip {
            start: dst,
            end: (dst + n).min(IWRAM_END),
            why: format!("code copied from ROM {src:#010x} (and any variables copied with it)"),
        })
        .collect()
}

/// The holes file the lifter takes for this ROM: `--holes FILE`, else
/// `holes/<name>.toml`, else `mods/<name>/holes.toml` (a commercial game's,
/// private), under ROOT (the checkout: `just lift` looks there); `None`
/// if there is none.
fn holes_file(root: &Path, rom: &Path, extra: &Extra) -> Option<PathBuf> {
    if let Some(f) = &extra.holes {
        return Some(PathBuf::from(f));
    }
    let name = match &extra.name {
        Some(n) => n.clone(),
        None => rom.file_stem()?.to_string_lossy().to_lowercase(),
    };
    [
        root.join(format!("holes/{name}.toml")),
        root.join(format!("mods/{name}/holes.toml")),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// The holes file's `[[copy]]` destinations: code the game copies to RAM
/// itself (by DMA, its own loop), which the lifter lifts at its RAM keys
/// and the native program runs as Rust, never writing the bytes. The
/// lifter refuses one that overlaps a static copy, so none is skipped
/// twice. As with [`code_copies`], the whole copy is skipped.
fn holes_copies(h: &codegen::native::Holes, file: &Path) -> Vec<Skip> {
    h.copies_seen
        .iter()
        .map(|c| Skip {
            start: c.dst,
            end: c.dst + c.len,
            why: format!(
                "code the game copies from ROM {:#010x} ({}'s [[copy]])",
                c.src,
                file.display()
            ),
        })
        .collect()
}

fn replaced(path: &str) -> Result<Vec<Skip>, Stop> {
    let text = std::fs::read_to_string(path).map_err(|e| Bad(format!("{path}: {e}")))?;
    let f: ReplacedFile = toml::from_str(&text).map_err(|e| Bad(format!("{path}: {e}")))?;
    Ok(f.replaced
        .into_iter()
        .map(|r| Skip {
            start: r.start,
            end: r.end,
            why: format!("replaced: {}", r.why),
        })
        .collect())
}

fn read_hashes(p: &Path) -> Result<Vec<String>, Stop> {
    Ok(std::fs::read_to_string(p)
        .map_err(|e| Unrun(format!("{}: {e}", p.display())))?
        .lines()
        .map(String::from)
        .collect())
}

pub enum Differ {
    Fail(String),
    Unrun(String),
}

/// Compare two records frame by frame. Ok: what was compared, and how
/// many frames had sound in `a` that `b` didn't make (no audio at all).
pub fn compare(a: &Path, b: &Path, skips: &[Skip], frames: u64) -> Result<(String, u64), Differ> {
    let open =
        |p: &Path| Reader::open(p).map_err(|e| Differ::Unrun(format!("{}: {e}", p.display())));
    let (mut ra, mut rb) = (open(a)?, open(b)?);
    let mut n = 0;
    // Frames whose audio both made, or play's was silent (as the native
    // program's none is), or play's wasn't.
    let (mut compared, mut silent, mut loud) = (0, 0, 0);
    loop {
        let fa = ra
            .read_frame()
            .map_err(|e| Differ::Unrun(format!("{}: {e}", a.display())))?;
        let fb = rb
            .read_frame()
            .map_err(|e| Differ::Unrun(format!("{}: {e}", b.display())))?;
        let (fa, fb) = match (fa, fb) {
            (None, None) => break,
            (Some(x), Some(y)) => (x, y),
            (x, _) => {
                return Err(Differ::Fail(format!(
                    "{} ends after {n} frames",
                    if x.is_none() { "play" } else { "native" }
                )));
            }
        };
        n += 1;
        if let Some(d) = frame_diff(fa, fb, skips) {
            return Err(Differ::Fail(d));
        }
        match (fa.audio, fb.audio) {
            (Some(_), Some(_)) => compared += 1,
            // Silence the native program can't make is silence still.
            (Some(x), None) if x.silent => silent += 1,
            (Some(_), None) => loud += 1,
            _ => {}
        }
    }
    if n != frames {
        return Err(Differ::Fail(format!("{n} frames recorded, not {frames}")));
    }
    let audio = if loud > 0 {
        format!(
            "audio NOT compared: the native program makes none, and play's had sound in {loud} \
             of {n} frames"
        )
    } else if compared > 0 {
        format!("audio the same in {compared} frames")
    } else if silent > 0 {
        format!("audio silent in both ({silent} frames: the native program makes none)")
    } else {
        "audio: neither makes any".into()
    };
    Ok((
        format!("0 differences in pictures, RAM, I/O, palette, VRAM and OAM; {audio}"),
        loud,
    ))
}

/// The first difference between two frames' records, named.
fn frame_diff(a: &Frame, b: &Frame, skips: &[Skip]) -> Option<String> {
    let f = a.frame;
    if a.frame != b.frame {
        return Some(format!("frame numbers {} and {}", a.frame, b.frame));
    }
    for r in Region::ALL {
        let (x, y) = (a.region(r), b.region(r));
        let mut first = None;
        let mut count = 0;
        for i in 0..x.len() {
            let addr = r.base() + i as u32;
            let mask = mask(r, addr, skips);
            if (x[i] ^ y[i]) & mask == 0 {
                continue;
            }
            count += 1;
            first.get_or_insert(i);
        }
        if let Some(i) = first {
            let addr = r.base() + i as u32;
            let mut s = format!(
                "frame {f}: {} byte {addr:#010x}: play {:#04x}, native {:#04x}",
                r.name(),
                x[i],
                y[i]
            );
            if r == Region::Io {
                let o = i as u32 & !1;
                write!(s, " ({})", io_name(o)).unwrap();
            }
            write!(s, "; {count} bytes differ in {}", r.name()).unwrap();
            if a.hash != b.hash {
                write!(s, ", and the picture").unwrap();
            }
            return Some(s);
        }
    }
    if a.hash != b.hash {
        return Some(format!(
            "frame {f}: the picture: play {:016x}, native {:016x}",
            a.hash, b.hash
        ));
    }
    if let (Some(x), Some(y)) = (a.audio, b.audio)
        && x != y
    {
        return Some(format!(
            "frame {f}: audio: play {} samples hash {:016x}, native {} samples hash {:016x}",
            x.samples, x.hash, y.samples, y.hash
        ));
    }
    None
}

/// The bits of byte `addr` of region `r` compared.
fn mask(r: Region, addr: u32, skips: &[Skip]) -> u8 {
    if r == Region::Io {
        let o = addr - r.base();
        return IO
            .iter()
            .find(|&&(s, e, _, _)| (s..e).contains(&(o & !1)))
            .map_or(0, |&(_, _, m, _)| (m >> (8 * (o & 1))) as u8);
    }
    if skips.iter().any(|s| (s.start..s.end).contains(&addr)) {
        0
    } else {
        0xFF
    }
}

fn io_name(o: u32) -> &'static str {
    IO.iter()
        .find(|&&(s, e, _, _)| (s..e).contains(&o))
        .map_or("?", |&(_, _, _, n)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_first_differing_byte_outside_the_skips() {
        let (mut a, mut b) = (Frame::new(), Frame::new());
        a.frame = 3;
        b.frame = 3;
        let skips = [Skip {
            start: 0x0300_7F00,
            end: IWRAM_END,
            why: "stack".into(),
        }];
        // In the stack: skipped.
        b.region_mut(Region::Iwram)[0x7F10] = 1;
        assert_eq!(frame_diff(&a, &b, &skips), None);
        // DMA's registers and KEYINPUT: not compared.
        b.set_io16(0x0B0, 5);
        b.set_io16(0x130, 0x03FE);
        assert_eq!(frame_diff(&a, &b, &skips), None);
        // The serial port's: compared, now the library keeps them.
        b.set_io16(0x128, 0x0080);
        assert!(frame_diff(&a, &b, &skips).unwrap().contains("serial"));
        b.set_io16(0x128, 0);
        b.set_io16(0x134, 0x8000);
        assert!(frame_diff(&a, &b, &skips).unwrap().contains("RCNT"));
        b.set_io16(0x134, 0);
        // A timer: compared, now the library has the play build's.
        b.set_io16(0x102, 0x0080);
        assert!(frame_diff(&a, &b, &skips).unwrap().contains("timers"));
        b.set_io16(0x102, 0);
        b.region_mut(Region::Ewram)[0xC0] = 2;
        assert_eq!(
            frame_diff(&a, &b, &skips).unwrap(),
            "frame 3: EWRAM byte 0x020000c0: play 0x00, native 0x02; 1 bytes differ in EWRAM"
        );
        let mut c = Frame::new();
        c.frame = 3;
        c.set_io16(0x82, 0x100);
        assert!(frame_diff(&a, &c, &skips).unwrap().contains("0x04000083"));
        assert!(
            frame_diff(&a, &c, &skips)
                .unwrap()
                .contains("sound registers")
        );
    }

    #[test]
    fn reads_the_lowest_sp() {
        let log = "x\ngbaa-desktop: lowest sp 0x03007e40\n";
        assert_eq!(lowest_sp(log), Some(0x0300_7E40));
        assert_eq!(lowest_sp("nothing"), None);
    }

    #[test]
    fn skips_the_holes_files_copies() {
        let text = r#"
[[copy]]
src = 0x08000104
dst = 0x03000c7c
len = 0x230
roots = [0x03000c7c]
why = "a dispatcher"

[[copy]]
src = 0x08004464
dst = 0x02010000
len = 0x10
roots = [0x02010001]
why = "a Thumb function"
"#;
        let h = codegen::native::Holes::parse(text).unwrap();
        let f = Path::new("mods/x/holes.toml");
        let skips = holes_copies(&h, f);
        assert_eq!(
            skips.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(),
            [(0x0300_0C7C, 0x0300_0EAC), (0x0201_0000, 0x0201_0010)]
        );
        assert!(skips[0].why.contains("0x08000104"));
        assert!(skips[0].why.contains("mods/x/holes.toml"));
        // Inside a copy: skipped; just past its end: compared.
        let (a, mut b) = (Frame::new(), Frame::new());
        b.region_mut(Region::Iwram)[0xC7C] = 1;
        b.region_mut(Region::Iwram)[0xEAB] = 1;
        assert_eq!(frame_diff(&a, &b, &skips), None);
        b.region_mut(Region::Iwram)[0xEAC] = 1;
        assert!(frame_diff(&a, &b, &skips).unwrap().contains("0x03000eac"));
    }

    #[test]
    fn finds_the_holes_file_by_name() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let rom = Path::new("/roms/No Such Game.gba");
        let given = Extra {
            holes: Some("h.toml".into()),
            ..Extra::default()
        };
        assert_eq!(holes_file(root, rom, &given), Some(PathBuf::from("h.toml")));
        // A name with no file: none.
        assert_eq!(holes_file(root, rom, &Extra::default()), None);
        // By the ROM's file name lowercased, or --name: native-spike's.
        let spike = Some(root.join("holes/native-spike.toml"));
        let found = holes_file(root, Path::new("/x/Native-Spike.gba"), &Extra::default());
        assert_eq!(found, spike);
        let named = Extra {
            name: Some("native-spike".into()),
            ..Extra::default()
        };
        assert_eq!(holes_file(root, rom, &named), spike);
    }
}
