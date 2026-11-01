//! `mgba-dump ROM --frames N [--state S] [--input SCRIPT] [--dump F.png]
//! [--hash] [--regs] [--frame-hashes F] [--dump-at N,N --dump-dir D] [--save F.sav]
//! [--save-state S] [--save-state-at N,N --state-dir D] [--bk2 LOG] [--bios F] [--clock-from T]
//! [--fifo-dump F] [--psg-dump F] [--audio-log F] [--wav F]`: runs ROM headless in
//! mGBA for N frames and writes what `gbaa-desktop --headless` would, in the
//! same format: the last frame as a PNG, its hash, the registers, each
//! frame's hash, chosen frames as PNGs (`D/frame-N.png`), the sound
//! FIFOs' samples (one `A B` byte pair per timer overflow), the PSG's share
//! of the mix (an `i16` LE pair per sample on its grid) and the records
//! each frame made (`frame records` lines, without ours' restarts and swaps),
//! and mGBA's mixed output as a WAV (`--wav`: 16-bit stereo at its sample
//! rate, 32768 Hz unless SOUNDBIAS asks for more, at its default 100%
//! volume, before its resampler). Milestone 3's
//! oracle (#55). A press in `--input` reaches the game at the same V-blank
//! as in ours. `--state` starts from a save state instead of reset (raw,
//! or the GUI's PNG with its save data; frames,
//! and the script's, count from it); `--save` loads a `.sav` into the
//! cartridge as mGBA's GUI loads the one beside the ROM; `--save-state` writes one after the
//! last frame, uncompressed (what `runtime::state` and mGBA both load);
//! `--save-state-at` one after each frame listed (`D/state-N.ss`).
//! `--bk2` plays a BizHawk movie's input log (`Input Log.txt` from the
//! `.bk2`) in place of `--input`: its line f is mGBA's frame f+1, as
//! BizHawk's mGBA core runs it, and its power button resets the core.
//! There mGBA is set up as BizHawk sets it for a movie: idle loops aren't
//! skipped, and the cartridge clock starts at `--clock-from T` (Unix
//! seconds, UTC; the movie's `RTCInitialTime`, by default BizHawk's
//! 2010-01-01) and gains `frame * 4389 >> 18` seconds. `--bios` runs a BIOS
//! dump in place of mGBA's HLE BIOS, its intro included, as BizHawk runs
//! one in a movie (its `SkipBios` applies only outside one): what an
//! Emerald TAS needs to sync (docs/research/fuzz-coverage.md, Story seeds).
//!
//! mGBA draws XBGR8, each 5-bit channel widened as `(c << 3) | (c >> 2)`;
//! taking the top five bits back gives BGR555, which hashes and writes
//! exactly as our PPU's frames do. Pixels mGBA computed in 8 bits (blends,
//! fades) lose their low bits, so those ROMs mismatch anyway.
//!
//! Without `--bk2` the cartridge clock is pinned to
//! [`runtime::gpio::FIXED_CLOCK`] in UTC, as our headless runs have it, so
//! hashes don't depend on the day.

mod bk2;

use desktop::Script;
use desktop::state_file::StateFile;
use runtime::Frame;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: mgba-dump ROM --frames N [--state S] [--input PRESSES] [--dump F.png] [--hash] [--regs] \
     [--frame-hashes F] [--dump-at N,N --dump-dir D] [--save F.sav] [--save-state S] \
     [--save-state-at N,N --state-dir D] [--bk2 LOG] [--bios F] [--clock-from T] [--fifo-dump F] [--psg-dump F] [--audio-log F] [--wav F]";

struct Options {
    rom: PathBuf,
    frames: u64,
    input: Script,
    dump: Option<PathBuf>,
    hash: bool,
    regs: bool,
    frame_hashes: Option<PathBuf>,
    dump_at: Vec<u64>,
    dump_dir: Option<PathBuf>,
    state: Option<PathBuf>,
    save: Option<PathBuf>,
    save_state: Option<PathBuf>,
    save_state_at: Vec<u64>,
    state_dir: Option<PathBuf>,
    bk2: Option<PathBuf>,
    bios: Option<PathBuf>,
    clock_from: Option<i64>,
    fifo_dump: Option<PathBuf>,
    psg_dump: Option<PathBuf>,
    audio_log: Option<PathBuf>,
    wav: Option<PathBuf>,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut args = args.into_iter();
    let (mut rom, mut frames) = (None, None);
    let mut o = Options {
        rom: PathBuf::new(),
        frames: 0,
        input: Script::default(),
        dump: None,
        hash: false,
        regs: false,
        frame_hashes: None,
        dump_at: Vec::new(),
        dump_dir: None,
        state: None,
        save: None,
        save_state: None,
        save_state_at: Vec::new(),
        state_dir: None,
        bk2: None,
        bios: None,
        clock_from: None,
        fifo_dump: None,
        psg_dump: None,
        audio_log: None,
        wav: None,
    };
    while let Some(a) = args.next() {
        let mut value = |flag: &str| args.next().ok_or(format!("{flag} needs a value"));
        match a.as_str() {
            "--frames" => {
                let v = value("--frames")?;
                frames = Some(v.parse().map_err(|_| format!("bad --frames {v:?}"))?);
            }
            "--input" => o.input = Script::parse(&value("--input")?)?,
            "--dump" => o.dump = Some(value("--dump")?.into()),
            "--frame-hashes" => o.frame_hashes = Some(value("--frame-hashes")?.into()),
            "--dump-at" => o.dump_at = desktop::frame_list(&value("--dump-at")?)?,
            "--dump-dir" => o.dump_dir = Some(value("--dump-dir")?.into()),
            "--state" => o.state = Some(value("--state")?.into()),
            "--save" => o.save = Some(value("--save")?.into()),
            "--save-state" => o.save_state = Some(value("--save-state")?.into()),
            "--save-state-at" => o.save_state_at = desktop::frame_list(&value("--save-state-at")?)?,
            "--state-dir" => o.state_dir = Some(value("--state-dir")?.into()),
            "--bk2" => o.bk2 = Some(value("--bk2")?.into()),
            "--bios" => o.bios = Some(value("--bios")?.into()),
            "--clock-from" => {
                let v = value("--clock-from")?;
                o.clock_from = Some(v.parse().map_err(|_| format!("bad --clock-from {v:?}"))?);
            }
            "--fifo-dump" => o.fifo_dump = Some(value("--fifo-dump")?.into()),
            "--psg-dump" => o.psg_dump = Some(value("--psg-dump")?.into()),
            "--audio-log" => o.audio_log = Some(value("--audio-log")?.into()),
            "--wav" => o.wav = Some(value("--wav")?.into()),
            "--hash" => o.hash = true,
            "--regs" => o.regs = true,
            _ if a.starts_with('-') => return Err(format!("unknown flag {a}")),
            _ if rom.is_none() => rom = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    o.rom = rom.ok_or("no ROM given")?;
    o.frames = frames.ok_or("--frames is required")?;
    if o.dump_at.is_empty() != o.dump_dir.is_none() {
        return Err("--dump-at and --dump-dir go together".into());
    }
    if let Some(&n) = o.dump_at.iter().max()
        && n > o.frames
    {
        return Err(format!("--dump-at {n} is past the last frame"));
    }
    if o.save_state_at.is_empty() != o.state_dir.is_none() {
        return Err("--save-state-at and --state-dir go together".into());
    }
    if let Some(&n) = o.save_state_at.iter().max()
        && n > o.frames
    {
        return Err(format!("--save-state-at {n} is past the last frame"));
    }
    if o.bk2.is_some() && o.input != Script::default() {
        return Err("--bk2 and --input don't go together".into());
    }
    if o.clock_from.is_some() && o.bk2.is_none() {
        return Err("--clock-from needs --bk2".into());
    }
    Ok(o)
}

/// What a run leaves: the last frame, r0–r15 and CPSR, the state, the FIFO
/// records (`A0 B0 A1 B1 …`, as `gbaa-desktop --fifo-dump`), how many
/// records each frame made, the PSG records (as `--psg-dump`), and the
/// mixed output if asked for (see [`Mix`]).
type Run = (
    Box<Frame>,
    [u32; 17],
    Vec<u8>,
    Vec<u8>,
    Vec<usize>,
    Vec<u8>,
    Option<Mix>,
);

/// mGBA's mixed output: its samples and their rate in Hz (the last rate the
/// game set; what came before is stretched to it).
type Mix = (Vec<(i16, i16)>, u32);

/// Called after each frame with its number (from 1) and picture, when asked.
type Each<'a> = dyn FnMut(u64, &Frame) -> Result<(), String> + 'a;

/// Called with a frame's number (from 1) and the state after it, for each
/// frame listed.
type Keep<'a> = dyn FnMut(u64, &[u8]) -> Result<(), String> + 'a;

/// What the core boots with: a BIOS dump (else mGBA's HLE BIOS), the
/// cartridge clock in Unix seconds, and whether mGBA is set up as BizHawk
/// sets it for a movie (the clock running from there, no idle loop skips).
#[cfg_attr(not(mgba), expect(dead_code))]
struct Boot<'a> {
    bios: Option<&'a [u8]>,
    clock: i64,
    bizhawk: bool,
}

/// The input mGBA's frame f (from 0) runs with: the keys, and whether the
/// core resets first.
type Input<'a> = dyn Fn(u64) -> bk2::Input + 'a;

/// An mGBA XBGR8 pixel as BGR555.
#[cfg_attr(not(any(mgba, test)), expect(dead_code))]
fn bgr555(p: u32) -> u16 {
    let p = p & 0x00FF_FFFF;
    let (r, g, b) = (p & 0xFF, (p >> 8) & 0xFF, (p >> 16) & 0xFF);
    ((r >> 3) | (g >> 3) << 5 | (b >> 3) << 10) as u16
}

fn main() -> ExitCode {
    match parse(std::env::args().skip(1)).and_then(|o| run(&o)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mgba-dump: {e}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run(o: &Options) -> Result<(), String> {
    let rom = std::fs::read(&o.rom).map_err(|e| format!("{}: {e}", o.rom.display()))?;
    let state = match &o.state {
        Some(p) => Some(desktop::state_file::read(p)?),
        None => None,
    };
    let bios = match &o.bios {
        Some(p) => Some(std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?),
        None => None,
    };
    let boot = Boot {
        bios: bios.as_deref(),
        clock: match (&o.bk2, o.clock_from) {
            (_, Some(t)) => t,
            (Some(_), None) => bk2::RTC_INITIAL_TIME,
            (None, None) => runtime::gpio::FIXED_CLOCK,
        },
        bizhawk: o.bk2.is_some(),
    };
    let save = match &o.save {
        Some(p) => Some(std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?),
        None => None,
    };
    let mut hashes = String::new();
    let mut each = |n: u64, frame: &Frame| -> Result<(), String> {
        if o.frame_hashes.is_some() {
            hashes.push_str(&format!(
                "frame {n} hash {:016x}\n",
                desktop::frame_hash(frame)
            ));
        }
        if let Some(dir) = &o.dump_dir
            && o.dump_at.contains(&n)
        {
            let path = desktop::dump_at_path(dir, n);
            desktop::write_png(&path, frame).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    };
    let every = o.frame_hashes.is_some() || o.dump_dir.is_some();
    let movie = match &o.bk2 {
        Some(p) => {
            let log = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            bk2::parse(&log)?
        }
        None => Vec::new(),
    };
    // mGBA's frame f runs from V-blank f-1 to V-blank f; ours reads
    // frame f's keys at V-blank f. So frame f-1's keys go into mGBA's
    // frame f, and both see a press from the same V-blank. A movie's
    // lines are mGBA's frames already.
    let input = |f: u64| match &o.bk2 {
        Some(_) => movie.get(f as usize).copied().unwrap_or_default(),
        None => bk2::Input {
            keys: f.checked_sub(1).map_or(0, |f| o.input.keys(f)),
            power: false,
        },
    };
    if let Some(dir) = &o.state_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut keep = |n: u64, state: &[u8]| -> Result<(), String> {
        let dir = o
            .state_dir
            .as_ref()
            .expect("--save-state-at needs --state-dir");
        let path = dir.join(format!("state-{n}.ss"));
        std::fs::write(&path, state).map_err(|e| format!("{}: {e}", path.display()))
    };
    mgba::record_audio(o.fifo_dump.is_some() || o.psg_dump.is_some() || o.audio_log.is_some());
    let (frame, regs, saved, fifo, per_frame, psg, mix) = mgba::run(
        &rom,
        &boot,
        state.as_ref(),
        save.as_deref(),
        o.frames,
        &input,
        o.wav.is_some(),
        every.then_some(&mut each as &mut Each),
        (&o.save_state_at, &mut keep),
    )?;
    if let Some(path) = &o.frame_hashes {
        std::fs::write(path, hashes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(path) = &o.fifo_dump {
        std::fs::write(path, fifo).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(path) = &o.psg_dump {
        std::fs::write(path, psg).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(path) = &o.audio_log {
        let log: String = per_frame
            .iter()
            .enumerate()
            .map(|(f, n)| format!("{f} {n}\n"))
            .collect();
        std::fs::write(path, log).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let (Some(path), Some((samples, rate))) = (&o.wav, mix) {
        std::fs::write(path, desktop::wav(&samples, rate))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(path) = &o.save_state {
        std::fs::write(path, saved).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(path) = &o.dump {
        desktop::write_png(path, &frame).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if o.hash {
        println!(
            "frame {} hash {:016x}",
            o.frames,
            desktop::frame_hash(&frame)
        );
    }
    if o.regs {
        // Between instructions mGBA's r15 is one instruction past the next
        // to execute (the fetch address); ours is the next.
        let half = if regs[16] & 0x20 != 0 { 2 } else { 4 };
        let pc = regs[15].wrapping_sub(half);
        println!("pc {pc:08x} cpsr {:08x}", regs[16]);
        for (i, r) in regs[..15].iter().enumerate() {
            print!("r{i}={r:08x}{}", if i % 5 == 4 { "\n" } else { " " });
        }
    }
    Ok(())
}

#[cfg(mgba)]
mod mgba {
    use super::*;
    use runtime::platform::{HEIGHT, WIDTH};
    use std::ffi::c_void;

    #[repr(C)]
    struct Core {
        _private: [u8; 0],
    }

    unsafe extern "C" {
        fn gbaa_mgba_open(
            rom: *const c_void,
            len: usize,
            frame: *mut u32,
            clock: i64,
            bizhawk: bool,
            bios: *const c_void,
            bios_len: usize,
        ) -> *mut Core;
        fn gbaa_mgba_run_frame(core: *mut Core, keys: u32);
        fn gbaa_mgba_reset(core: *mut Core);
        fn gbaa_mgba_regs(core: *mut Core, out: *mut u32);
        fn gbaa_mgba_close(core: *mut Core);
        fn gbaa_mgba_state_size(core: *mut Core) -> usize;
        fn gbaa_mgba_save_state(core: *mut Core, out: *mut c_void) -> bool;
        fn gbaa_mgba_load_state(core: *mut Core, state: *const c_void) -> bool;
        fn gbaa_mgba_restore_save(core: *mut Core, save: *const c_void, len: usize) -> bool;
        fn gbaa_mgba_record_audio(on: bool);
        fn gbaa_mgba_fifo(len: *mut usize) -> *const u8;
        fn gbaa_mgba_psg(len: *mut usize) -> *const u8;
        fn gbaa_mgba_mix_start(core: *mut Core);
        fn gbaa_mgba_mix(len: *mut usize, rate: *mut u32) -> *const u8;
    }

    /// Whether the FIFO and PSG records are kept (they are unless told).
    pub fn record_audio(on: bool) {
        // SAFETY: sets a flag the shim reads.
        unsafe { gbaa_mgba_record_audio(on) }
    }

    /// Run `rom` as `boot` has it (from `state`, if given) for `frames` frames, pressing
    /// `input`; the last frame, r0–r15 and CPSR, the state after, and the
    /// FIFO records and each frame's count of them, the PSG records, and
    /// the mixed output if `mix`. `keep` gets the state after each frame
    /// it lists.
    #[expect(clippy::too_many_arguments)]
    pub fn run(
        rom: &[u8],
        boot: &Boot,
        state: Option<&StateFile>,
        sav: Option<&[u8]>,
        frames: u64,
        input: &Input,
        mix: bool,
        mut each: Option<&mut Each>,
        (keep_at, keep): (&[u64], &mut Keep),
    ) -> Result<Run, String> {
        let mut xbgr = vec![0u32; WIDTH * HEIGHT];
        // SAFETY: the shim copies the ROM and the BIOS; `xbgr` outlives the core, which
        // is closed before it's read and dropped. A state is read only up
        // to the size checked. The FIFO records are `len` bytes the shim owns.
        let (mut per_frame, mut records) = (Vec::new(), 0);
        let (regs, saved, fifo, psg, mixed) = unsafe {
            let core = gbaa_mgba_open(
                rom.as_ptr().cast(),
                rom.len(),
                xbgr.as_mut_ptr(),
                boot.clock,
                boot.bizhawk,
                boot.bios.map_or(std::ptr::null(), |b| b.as_ptr().cast()),
                boot.bios.map_or(0, <[u8]>::len),
            );
            if core.is_null() {
                return Err("mGBA couldn't load the ROM (or the BIOS)".into());
            }
            if let Some(sav) = sav
                && !gbaa_mgba_restore_save(core, sav.as_ptr().cast(), sav.len())
            {
                gbaa_mgba_close(core);
                return Err("mGBA couldn't load the .sav".into());
            }
            let size = gbaa_mgba_state_size(core);
            if let Some(StateFile { state: s, save }) = state {
                // A state file may carry extra data after the state itself.
                if s.len() < size || !gbaa_mgba_load_state(core, s.as_ptr().cast()) {
                    gbaa_mgba_close(core);
                    return Err(format!("mGBA couldn't load the state ({} bytes)", s.len()));
                }
                if let Some(save) = save
                    && !gbaa_mgba_restore_save(core, save.as_ptr().cast(), save.len())
                {
                    gbaa_mgba_close(core);
                    return Err("mGBA couldn't load the state's save data".into());
                }
            }
            if mix {
                gbaa_mgba_mix_start(core);
            }
            let mut kept = vec![0u8; size];
            for f in 0..frames {
                let bk2::Input { keys, power } = input(f);
                if power {
                    gbaa_mgba_reset(core);
                }
                gbaa_mgba_run_frame(core, keys.into());
                let mut len = 0;
                gbaa_mgba_fifo(&mut len);
                per_frame.push(len / 2 - records);
                records = len / 2;
                if let Some(each) = &mut each {
                    let frame = to_frame(&xbgr);
                    if let Err(e) = each(f + 1, &frame) {
                        gbaa_mgba_close(core);
                        return Err(e);
                    }
                }
                if keep_at.contains(&(f + 1)) {
                    let r = if gbaa_mgba_save_state(core, kept.as_mut_ptr().cast()) {
                        keep(f + 1, &kept)
                    } else {
                        Err("mGBA couldn't save the state".into())
                    };
                    if let Err(e) = r {
                        gbaa_mgba_close(core);
                        return Err(e);
                    }
                }
            }
            let mut regs = [0; 17];
            gbaa_mgba_regs(core, regs.as_mut_ptr());
            let mut saved = vec![0u8; size];
            let ok = gbaa_mgba_save_state(core, saved.as_mut_ptr().cast());
            gbaa_mgba_close(core);
            if !ok {
                return Err("mGBA couldn't save the state".into());
            }
            let mut len = 0;
            let records = gbaa_mgba_fifo(&mut len);
            let fifo = if len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(records, len).to_vec()
            };
            let records = gbaa_mgba_psg(&mut len);
            let psg = if len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(records, len).to_vec()
            };
            let mut rate = 0;
            let samples = gbaa_mgba_mix(&mut len, &mut rate);
            let mixed = mix.then(|| {
                let bytes = if len == 0 {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(samples, len)
                };
                let pairs = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| {
                        (
                            i16::from_le_bytes([b[0], b[1]]),
                            i16::from_le_bytes([b[2], b[3]]),
                        )
                    })
                    .collect();
                (pairs, rate)
            });
            (regs, saved, fifo, psg, mixed)
        };
        Ok((to_frame(&xbgr), regs, saved, fifo, per_frame, psg, mixed))
    }

    /// mGBA's XBGR8 picture as one of ours.
    fn to_frame(xbgr: &[u32]) -> Box<Frame> {
        let mut frame = Box::new([0; WIDTH * HEIGHT]);
        for (d, &p) in frame.iter_mut().zip(xbgr) {
            *d = bgr555(p);
        }
        frame
    }
}

#[cfg(not(mgba))]
mod mgba {
    use super::*;

    pub fn run(
        _: &[u8],
        _: &Boot,
        _: Option<&StateFile>,
        _: Option<&[u8]>,
        _: u64,
        _: &Input,
        _: bool,
        _: Option<&mut Each>,
        _: (&[u64], &mut Keep),
    ) -> Result<Run, String> {
        Err("built without libmgba: run scripts/fetch-mgba.sh, then rebuild".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_round_trip() {
        // Every BGR555 colour, widened the way mGBA does, comes back.
        let widen = |c: u32| (c << 3) | (c >> 2);
        for c in 0..0x8000u32 {
            let xbgr =
                0xFF00_0000 | widen(c & 31) | widen((c >> 5) & 31) << 8 | widen(c >> 10) << 16;
            assert_eq!(bgr555(xbgr), c as u16);
        }
    }

    #[test]
    fn args() {
        let a = |s: &str| parse(s.split(' ').map(String::from));
        let o = a("x.gba --frames 60 --hash --dump f.png --input start@3").unwrap();
        assert_eq!((o.frames, o.hash, o.regs), (60, true, false));
        assert_eq!((o.state, o.save_state), (None, None));
        let st = a("x.gba --frames 1 --state a.ss --save-state b.ss --fifo-dump f").unwrap();
        assert_eq!(st.fifo_dump, Some("f".into()));
        assert_eq!(
            a("x.gba --frames 1 --audio-log l").unwrap().audio_log,
            Some("l".into())
        );
        assert_eq!(
            a("x.gba --frames 1 --wav w.wav").unwrap().wav,
            Some("w.wav".into())
        );
        assert_eq!(st.state, Some("a.ss".into()));
        assert_eq!(st.save_state, Some("b.ss".into()));
        assert_eq!(o.input.keys(3), runtime::io::key::START);
        let d = a("x.gba --frames 9 --frame-hashes h --dump-at 3,9 --dump-dir d").unwrap();
        assert_eq!((d.dump_at, d.dump_dir), (vec![3, 9], Some("d".into())));
        assert_eq!(d.frame_hashes, Some("h".into()));
        assert!(a("x.gba --frames 9 --dump-at 10 --dump-dir d").is_err());
        assert!(a("x.gba --frames 9 --dump-at 3").is_err());
        let k = a("x.gba --frames 9 --save-state-at 3,9 --state-dir s --bk2 l").unwrap();
        assert_eq!(
            (k.save_state_at, k.state_dir),
            (vec![3, 9], Some("s".into()))
        );
        assert_eq!(k.bk2, Some("l".into()));
        let b = a("x.gba --frames 9 --bk2 l --bios b.bin --clock-from 1262304000").unwrap();
        assert_eq!(
            (b.bios, b.clock_from),
            (Some("b.bin".into()), Some(1262304000))
        );
        assert!(a("x.gba --frames 9 --bk2 l --clock-from soon").is_err());
        assert!(a("x.gba --frames 9 --clock-from 0").is_err());
        assert!(a("x.gba --frames 9 --save-state-at 3").is_err());
        assert!(a("x.gba --frames 9 --save-state-at 10 --state-dir s").is_err());
        assert!(a("x.gba --frames 9 --bk2 l --input a@1").is_err());
        assert!(a("x.gba").is_err());
        assert!(a("x.gba --frames 1 --bogus").is_err());
    }
}
