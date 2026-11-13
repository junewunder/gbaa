//! What the native programs share (`native-spike`, `native-psg-square`):
//! gbaa-desktop's headless flags, a host that hashes each frame, the
//! native check's record (`--native-record`), and gbaa-desktop's window
//! to play in.
//!
//! ```text
//! PROGRAM --headless --frames N [--input PRESSES] [--frame-hashes F]
//!         [--dump FILE.png] [--native-record FILE] [--time-limit SECONDS]
//! PROGRAM [--scale N] [--mute] [--input PRESSES] [--native-record FILE]
//! ```
//!
//! Without `--headless` the program opens gbaa-desktop's window
//! (`desktop::window`: its keys, pads, sound, pacing and menu) and runs
//! until the window closes. It keeps the game's save as a play build of
//! the game does: `<name>.sav` in the game's folder
//! (`desktop::native_saves`; the Save folders page moves it), loaded
//! before the program starts, written once it settles and on closing,
//! when it is also backed up. The menus leave out save states, rewind,
//! scripts and netplay, which a native program hasn't got. Headless runs
//! keep no save, so the checks' runs stay reset-clean.
//!
//! `--input` is gbaa-desktop's (`button@frame[+frames],...`), read by the
//! same parser; `--frame-hashes` writes the same `frame N hash H` lines;
//! `--native-record` writes each frame's record as the play build's
//! `--native-record` does (crates/native-record), from the program's RAM
//! as the guest lays it out: a guest-layout build's own bytes, or the
//! image a play-layout build makes of its fields.
//!
//! A program that stops before its frames run out says where in one line
//! on stderr, the stop line (`native_record::stop`, unit
//! `native/native-first-stop`): at a panic (an I/O register the library
//! doesn't model, memory the program hasn't got), after the panic's own
//! message, with the frames presented and the lifted functions on the
//! stack; and when `--time-limit`'s seconds pass first (a loop that never
//! reaches a frame), exiting [`TIME_LIMIT_EXIT`]. `gbaa-first-stop` reads it.

use std::cell::RefCell;
use std::convert::Infallible;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use desktop::sav_file::SavFile;
use desktop::{Desktop, Script};
use native_record::stop::{Stop, Why, functions_in};
use native_record::{Frame as Record, Region, Writer};
use runtime::Platform;
use runtime::hw::{Exit, Host, Hw, Native};
use runtime::platform::{Frame, HEIGHT, WIDTH};
use runtime::save::Save;
use runtime::sound::AudioChunk;

/// The exit code when `--time-limit` stops the program (`timeout(1)`'s).
pub const TIME_LIMIT_EXIT: i32 = 124;

/// Frames presented so far and `--frames`, for the stop line: the panic
/// hook and the time limit's thread read them.
static SHOWN: AtomicU64 = AtomicU64::new(0);
static ASKED: AtomicU64 = AtomicU64::new(0);

pub struct Options {
    frames: u64,
    /// Seconds the run may take; 0 for no limit.
    time_limit: u64,
    input: Script,
    frame_hashes: Option<PathBuf>,
    dump: Option<PathBuf>,
    native_record: Option<PathBuf>,
    /// No `--headless`: the window to open.
    window: Option<Window>,
}

/// The window's flags, gbaa-desktop's.
struct Window {
    scale: usize,
    mute: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        frames: 0,
        time_limit: 0,
        input: Script::default(),
        frame_hashes: None,
        dump: None,
        native_record: None,
        window: None,
    };
    let mut headless = false;
    let mut window = Window {
        scale: 3,
        mute: false,
    };
    // The flags only one of the two front ends takes, as given.
    let (mut headless_only, mut window_only) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--headless" => headless = true,
            "--frames" => {
                o.frames = value()?.parse().map_err(|e| format!("--frames: {e}"))?;
                headless_only = Some(a);
            }
            "--scale" => {
                window.scale = value()?.parse().map_err(|e| format!("--scale: {e}"))?;
                window_only = Some(a);
            }
            "--mute" => {
                window.mute = true;
                window_only = Some(a);
            }
            "--time-limit" => {
                o.time_limit = value()?.parse().map_err(|e| format!("--time-limit: {e}"))?;
            }
            "--input" => o.input = Script::parse(value()?)?,
            "--frame-hashes" => {
                o.frame_hashes = Some(value()?.into());
                headless_only = Some(a);
            }
            "--dump" => {
                o.dump = Some(value()?.into());
                headless_only = Some(a);
            }
            "--native-record" => o.native_record = Some(value()?.into()),
            _ => return Err(format!("unknown argument {a:?}")),
        }
    }
    if headless {
        if let Some(a) = window_only {
            return Err(format!("{a} is the window's: not with --headless"));
        }
        if o.frames == 0 {
            return Err("a headless run needs its end: --headless --frames N".into());
        }
    } else {
        if let Some(a) = headless_only {
            return Err(format!("{a} needs --headless"));
        }
        if window.scale == 0 {
            return Err("--scale: at least 1".into());
        }
        o.window = Some(window);
    }
    Ok(o)
}

/// A headless front end: the script's keys, each frame's hash, the last
/// frame kept.
struct Headless {
    input: Script,
    frames: u64,
    shown: u64,
    hashes: String,
    last: Box<Frame>,
}

impl Host for Headless {
    fn present(&mut self, frame: &Frame) -> Result<(), Exit> {
        self.shown += 1;
        SHOWN.store(self.shown, Ordering::Relaxed);
        let h = desktop::frame_hash(frame);
        self.hashes
            .push_str(&format!("frame {} hash {h:016x}\n", self.shown));
        self.last.copy_from_slice(frame);
        if self.shown < self.frames {
            Ok(())
        } else {
            Err(Exit)
        }
    }

    /// As gbaa-desktop's: the presses of the frame just shown.
    fn keys(&mut self) -> u16 {
        self.input.keys(self.shown - 1)
    }
}

/// gbaa-desktop's window as the front end: its keys and pads, its sound
/// (which paces the program, or a frame clock when muted) and its menu,
/// until the window closes; and the game's `.sav`, shared with
/// `run_window`, which loads it and writes it out at the end.
struct Windowed(Desktop, Rc<RefCell<Option<SavFile>>>);

impl Host for Windowed {
    fn present(&mut self, frame: &Frame) -> Result<(), Exit> {
        // `keys`, read after each present, learns the window has closed.
        if !self.0.is_open() {
            return Err(Exit);
        }
        SHOWN.fetch_add(1, Ordering::Relaxed);
        Platform::present(&mut self.0, frame);
        Ok(())
    }

    fn audio(&mut self, chunk: &AudioChunk<'_>) {
        Platform::audio(&mut self.0, chunk);
    }

    fn keys(&mut self) -> u16 {
        Platform::keys(&mut self.0)
    }

    /// Written once it has settled, as gbaa-desktop's window writes it.
    fn save(&mut self, save: &mut Save) {
        let mut f = self.1.borrow_mut();
        if let Some(Err(e)) = f.as_mut().map(|f| f.frame(save)) {
            eprintln!("save not written: {e}");
        }
    }
}

/// [`Headless`] shared with `run`, which reads its hashes and last frame
/// once the program stops.
struct Shared(Rc<RefCell<Headless>>);

impl Host for Shared {
    fn present(&mut self, frame: &Frame) -> Result<(), Exit> {
        self.0.borrow_mut().present(frame)
    }

    fn keys(&mut self) -> u16 {
        self.0.borrow_mut().keys()
    }
}

/// Whatever host the program runs on: [`Shared`] here, another in tests.
pub struct AnyHost(pub Box<dyn Host>);

impl Host for AnyHost {
    fn present(&mut self, frame: &Frame) -> Result<(), Exit> {
        self.0.present(frame)
    }

    /// Kept for the frame's record, if there is one (as the play build's
    /// capture keeps it: each FIFO record's two bytes, each PSG sample's
    /// two little-endian halfwords), then passed on.
    fn audio(&mut self, chunk: &AudioChunk<'_>) {
        SOUND.with_borrow_mut(|s| {
            let Some((fifo, psg)) = s else {
                return;
            };
            for &(a, b) in chunk.fifo {
                fifo.extend([a as u8, b as u8]);
            }
            for &(l, r) in chunk.psg {
                psg.extend(l.to_le_bytes());
                psg.extend(r.to_le_bytes());
            }
        });
        self.0.audio(chunk);
    }

    fn keys(&mut self) -> u16 {
        self.0.keys()
    }

    fn save(&mut self, save: &mut Save) {
        self.0.save(save);
    }
}

/// Where agb's crt0 (agb 0.25's `entrypoint.s`) copies its sections from:
/// the ROM addresses of `.ewram` and `.iwram` (the ELF's load addresses),
/// and their sizes in bytes.
pub struct Sections {
    pub ewram: (u32, u32),
    pub iwram: (u32, u32),
}

/// agb's crt0 before `main`, as the native program keeps it: as time
/// only. It `CpuSet`s `.ewram` into EWRAM (agb's globals: replaced by the
/// library), sets `WAITCNT` and `CpuSet`s `.iwram` (agb's IRQ handler and
/// copy routines: run as Rust), then sets `IME`. The copies' bytes aren't
/// the program's; their time is the play build's (`hw::spend`), which a
/// frame drawn during boot shows.
pub fn agb_crt0<G: Native>(g: &mut G, s: &Sections) -> Result<(), Exit> {
    use runtime::hw::{self, bios::cpu_set_cycles};
    let hw = g.hw();
    let ctl = |bytes: u32| bytes / 2;
    let c = cpu_set_cycles(hw.waitcnt, s.ewram.0, 0x0200_0000, ctl(s.ewram.1));
    hw::spend(g, c)?;
    g.hw().write16(runtime::io::reg::WAITCNT, 0x4317);
    let hw = g.hw();
    let c = cpu_set_cycles(hw.waitcnt, s.iwram.0, 0x0300_0000, ctl(s.iwram.1));
    hw::spend(g, c)?;
    g.hw().irq.master_enable = true;
    Ok(())
}

thread_local! {
    /// `--native-record`'s writer and the record it fills, if asked for.
    static RECORD: RefCell<Option<(Writer<std::io::BufWriter<std::fs::File>>, Record)>> =
        const { RefCell::new(None) };
    /// The sound handed to the host since the last record (FIFO bytes,
    /// PSG bytes), kept only when there is a record to write.
    static SOUND: RefCell<Option<(Vec<u8>, Vec<u8>)>> = const { RefCell::new(None) };
}

/// The frame just finished, recorded (for [`Hw::on_frame_end`]): the
/// picture's hash, `ram` (the guest layout's EWRAM then IWRAM), the
/// library's I/O registers ([`Hw::raw16`]), the display's memories, and
/// the sound handed off since the last record (the frame's own, as the
/// play build's record has it).
pub fn record<G: Native>(hw: &Hw<G>, ram: &[u8]) {
    RECORD.with_borrow_mut(|r| {
        let Some((w, f)) = r else {
            return;
        };
        f.frame = hw.frames();
        f.hash = desktop::frame_hash(hw.frame());
        let (e, i) = ram.split_at(Region::Ewram.size());
        f.region_mut(Region::Ewram).copy_from_slice(e);
        f.region_mut(Region::Iwram).copy_from_slice(i);
        for o in (0..0x400).step_by(2) {
            f.set_io16(o, hw.raw16(o));
        }
        f.region_mut(Region::Palette)
            .copy_from_slice(&hw.ppu.palette);
        f.region_mut(Region::Vram).copy_from_slice(&hw.ppu.vram);
        f.region_mut(Region::Oam).copy_from_slice(&hw.ppu.oam);
        f.audio = SOUND.with_borrow_mut(|s| {
            let (fifo, psg) = s.as_mut()?;
            let a = native_record::Audio {
                samples: (fifo.len() / 2 + psg.len() / 4) as u32,
                hash: native_record::fnv(native_record::fnv(native_record::FNV, fifo), psg),
                silent: fifo.iter().chain(psg.iter()).all(|&b| b == 0),
            };
            fifo.clear();
            psg.clear();
            Some(a)
        });
        if let Err(e) = w.write(f) {
            panic!("--native-record: {e}");
        }
    });
}

/// Parse the command line, make the game on a headless host or in a
/// window, run `main` until the frames run out or the window closes, and
/// write what was asked for. `guest`: the build can record its RAM as the
/// guest lays it out.
pub fn main<G: Native<Host = AnyHost> + 'static>(
    name: &str,
    guest: bool,
    make: impl FnOnce(AnyHost) -> Box<G> + Send + 'static,
    main: fn(&mut G) -> Result<Infallible, Exit>,
) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args).and_then(|o| run(name, o, guest, make, main)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The stop line for a stop now, on stderr.
fn say_stop(functions: Vec<u32>, at: String, why: Why) {
    let s = Stop {
        frames: SHOWN.load(Ordering::Relaxed),
        asked: ASKED.load(Ordering::Relaxed),
        functions,
        at,
        why,
    };
    eprintln!("{}", s.line());
}

/// Say where a panic stopped the program, after the panic's own message:
/// the stop line, its functions the lifted ones the backtrace names.
fn stop_at_a_panic() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        before(info);
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let message = info.payload_as_str().unwrap_or("a panic with no message");
        say_stop(functions_in(&bt), at, Why::Panic(message.to_string()));
    }));
}

/// Stop the program [`TIME_LIMIT_EXIT`] with a stop line if it is still
/// running after `seconds`.
fn stop_after(seconds: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        say_stop(Vec::new(), String::new(), Why::TimeLimit(seconds));
        std::process::exit(TIME_LIMIT_EXIT);
    });
}

/// Start `--native-record`'s file, if asked for, on the thread the
/// program runs on.
fn record_to(p: Option<&PathBuf>) -> Result<(), String> {
    let Some(p) = p else {
        return Ok(());
    };
    let w = Writer::create(p).map_err(|e| format!("{}: {e}", p.display()))?;
    RECORD.set(Some((w, Record::new())));
    SOUND.set(Some((Vec::new(), Vec::new())));
    Ok(())
}

/// Write out what is left of `--native-record`'s file.
fn record_end() -> Result<(), String> {
    if let Some((mut w, _)) = RECORD.take() {
        w.flush().map_err(|e| format!("--native-record: {e}"))?;
    }
    Ok(())
}

/// The program in gbaa-desktop's window, on the window's game thread,
/// until the window closes, keeping the save of the game `name`.
fn run_window<G: Native<Host = AnyHost> + 'static>(
    name: &str,
    w: Window,
    input: Script,
    record: Option<PathBuf>,
    make: impl FnOnce(AnyHost) -> Box<G> + Send + 'static,
    main: fn(&mut G) -> Result<Infallible, Exit>,
) -> Result<(), String> {
    use desktop::{audio, fast, gamepad, room, window};
    let (sav, saves) = desktop::native_saves(name)?;
    let o = window::Options {
        scale: w.scale,
        fullscreen: false,
        controls: Default::default(),
        fast_speed: fast::DEFAULT_SPEED,
        volume: audio::DEFAULT_VOLUME,
        pad_script: None,
        screens: 1,
        server: room::DEFAULT_SERVER.into(),
        directions_apart: false,
        deadzones: [gamepad::DEFAULT_DEADZONE, gamepad::DEFAULT_TRIGGER_DEADZONE],
        deadzone_axes_apart: false,
        axis_deadzones: [None; 2],
        resume: false,
        saves,
        native: true,
    };
    window::run(o, move |screen| {
        record_to(record.as_ref())?;
        let gain = audio::volume_gain(audio::DEFAULT_VOLUME);
        let d = Desktop::window(screen, w.mute, gain).with_input(input);
        // Loaded once the program has made its chip, the kind it set.
        let file = Rc::new(RefCell::new(None));
        let mut g = make(AnyHost(Box::new(Windowed(d, file.clone()))));
        let f = SavFile::open(&sav, &mut g.hw().save)?;
        *file.borrow_mut() = Some(f);
        let Err(Exit) = main(&mut g);
        file.borrow_mut()
            .as_mut()
            .expect("opened")
            .flush(&mut g.hw().save)?;
        desktop::back_up_save(&sav);
        record_end()
    })?
}

fn run<G: Native<Host = AnyHost> + 'static>(
    name: &str,
    o: Options,
    guest: bool,
    make: impl FnOnce(AnyHost) -> Box<G> + Send + 'static,
    main: fn(&mut G) -> Result<Infallible, Exit>,
) -> Result<(), String> {
    ASKED.store(o.frames, Ordering::Relaxed);
    stop_at_a_panic();
    if o.time_limit > 0 {
        stop_after(o.time_limit);
    }
    if o.native_record.is_some() && !guest {
        return Err("--native-record: this build has no guest-layout RAM to record".into());
    }
    if let Some(w) = o.window {
        return run_window(name, w, o.input, o.native_record, make, main);
    }
    record_to(o.native_record.as_ref())?;
    let host = Rc::new(RefCell::new(Headless {
        input: o.input,
        frames: o.frames,
        shown: 0,
        hashes: String::new(),
        last: Box::new([0; WIDTH * HEIGHT]),
    }));
    let mut g = make(AnyHost(Box::new(Shared(host.clone()))));
    let Err(Exit) = main(&mut g);
    record_end()?;
    let h = host.borrow();
    if let Some(p) = &o.frame_hashes {
        std::fs::write(p, &h.hashes).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    if let Some(p) = &o.dump {
        desktop::write_png(p, &h.last).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}
