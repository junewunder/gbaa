//! Linked play (`--link FILE`): two GBAs running the same ROM on one link
//! cable ([`runtime::link`]), in one window, the two screens side by side
//! (`docs/research/multiplayer.md`, "Plan of units", 4).
//!
//! Player 1 is the cable's parent, its save kept as `--save` keeps any
//! game's; player 2 is the child, its save kept in `--link`'s file. The
//! hub runs both a frame at a time, in turns, in the interpreter. Player 1
//! plays on the mapping's keys and the first pad plugged in, player 2 on
//! its own keys ([`crate::gamepad::player2_keys`]) and the second pad;
//! headless, `--input` and `--input2` press their buttons. Player 1's sound
//! plays and paces both (or, muted, the frame clock). The menu pauses and
//! quits; fast-forward, rewind, save states and scripts aren't linked yet.

use crate::audio::Player;
use crate::dump::{pixels_hash, write_png_wide};
use crate::fast::{self, Clock};
use crate::input::Script;
use crate::present::Hold;
use crate::window::{Screen, side_by_side};
use crate::{Options, dump_at_path, sav_file};
use runtime::io::key;
use runtime::link::Link;
use runtime::platform::{HEIGHT, WIDTH};
use runtime::sched::FRAME_CYCLES;
use runtime::{AudioChunk, Frame, Machine, Platform, Timing};

/// The players: the parent and one child.
pub const PLAYERS: usize = 2;

/// A player's side of the front end: its last frame, its keys and (player
/// 1's) its sound.
pub struct Seat {
    /// Scripted presses, on top of what the window holds.
    script: Script,
    /// What the window's keyboard and pad hold for this player.
    pub(crate) held: u16,
    /// Frames presented.
    shown: u64,
    /// The last frame presented.
    pub frame: Box<Frame>,
    pub(crate) player: Option<Player>,
}

impl Seat {
    pub(crate) fn new(script: Script, player: Option<Player>) -> Seat {
        Seat {
            script,
            held: 0,
            shown: 0,
            frame: Box::new([0; WIDTH * HEIGHT]),
            player,
        }
    }
}

impl Platform for Seat {
    fn present(&mut self, frame: &Frame) {
        self.shown += 1;
        self.frame.copy_from_slice(frame);
        if let Some(p) = &mut self.player {
            p.end_frame();
        }
    }

    fn audio(&mut self, chunk: &AudioChunk<'_>) {
        if let Some(p) = &mut self.player {
            p.audio(chunk);
        }
    }

    /// Read after each frame's `present`, as [`crate::Desktop`]'s keys.
    fn keys(&mut self) -> u16 {
        self.script.keys(self.shown.saturating_sub(1)) | self.held
    }
}

/// Whether `o`'s other options go with `--link`.
pub fn check(o: &Options) -> Result<(), String> {
    let not = [
        (o.regs, "--regs"),
        (o.state.is_some(), "--state"),
        (o.state_out.is_some(), "--state-out"),
        (o.frame_hashes.is_some(), "--frame-hashes"),
        (o.frame_times.is_some(), "--frame-times"),
        (o.state_bench.is_some(), "--state-bench"),
        (o.rerun_bench.is_some(), "--rerun-bench"),
        (o.dump_states, "--dump-states"),
        (o.native_record.is_some(), "--native-record"),
        (o.rewind.is_some(), "--rewind"),
        (o.interp_blocks, "--interp-blocks"),
        (o.unwind, "--unwind"),
        (
            o.audio(),
            "--wav, --fifo-dump, --psg-dump, --audio-hash, --audio-log",
        ),
        (o.audio_pace.is_some(), "--audio-pace"),
        (o.fast_forward, "--fast-forward"),
        (o.fast_speed.is_some(), "--fast-speed"),
        (!o.scripts.is_empty(), "--script"),
        (!o.script_keys.is_empty(), "--script-keys"),
    ];
    match not.iter().find(|(set, _)| *set) {
        Some((_, name)) => Err(format!("{name} isn't for linked play (--link)")),
        None => Ok(()),
    }
}

/// Run two linked GBAs as `opts` says, their interpreters timed by
/// `timing`, in the window `screen` or headless.
pub fn run(opts: &Options, timing: Timing, screen: Option<Screen>) -> Result<(), String> {
    let link_sav = opts.link.as_ref().ok_or("linked play needs --link")?;
    let rom = opts.load_rom()?;
    let sound = (screen.is_some() && !opts.mute)
        .then(|| {
            Player::open()
                .map(|mut p| {
                    p.set_gain(opts.gain());
                    p
                })
                .inspect(|p| eprintln!("gbaa-desktop: audio: {}", p.describe()))
                .inspect_err(|e| eprintln!("gbaa-desktop: no audio ({e}); running muted"))
                .ok()
        })
        .flatten();
    // Muted, the window keeps time by a frame clock.
    let mut clock = (screen.is_some() && sound.is_none()).then(Clock::default);
    // Frames go to the window on the pacer's beat ([`crate::present`]).
    let mut hold = Hold::default();
    let mut a = Box::new(Machine::new(rom, Seat::new(opts.input.clone(), sound)));
    let mut b = Box::new(Machine::new(rom, Seat::new(opts.input2.clone(), None)));
    a.set_timing(timing);
    b.set_timing(timing);
    let mut savs = [
        match &opts.save {
            Some(path) => Some(sav_file::SavFile::open(path, &mut a.bus.save)?),
            None => None,
        },
        Some(sav_file::SavFile::open(link_sav, &mut b.bus.save)?),
    ];
    let mut link = Link::new(&mut [&mut *a, &mut *b]);
    if let Some(s) = &screen {
        s.set_title("gbaa: linked");
    }
    let mut wide = vec![0u16; PLAYERS * WIDTH * HEIGHT];
    let mut frames = 0;
    let mut open = true;
    while opts.frames.is_none_or(|n| frames < n) && open {
        // As the single window's: the wall clock for the cartridges' RTC.
        if screen.is_some()
            && let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        {
            a.bus.gpio.clock = t.as_secs() as i64;
            b.bus.gpio.clock = t.as_secs() as i64;
        }
        link.run(&mut [&mut *a, &mut *b], FRAME_CYCLES, |_, m| m.run_frame());
        frames += 1;
        for (p, m) in [&a, &b].into_iter().enumerate() {
            if let Some(f) = &m.fault {
                return Err(format!("player {}, frame {frames}: {f}", p + 1));
            }
        }
        for (f, m) in savs.iter_mut().zip([&mut a, &mut b]) {
            if let Some(f) = f {
                f.frame(&mut m.bus.save)?;
            }
        }
        if let Some(dir) = &opts.dump_dir
            && opts.dump_at.contains(&frames)
        {
            side_by_side(&[&a.platform.frame, &b.platform.frame], &mut wide);
            let path = dump_at_path(dir, frames);
            write_png_wide(&path, &wide).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let Some(s) = &screen else {
            continue;
        };
        hold.before_show();
        s.show_screens(&[&a.platform.frame, &b.platform.frame], false);
        if let Some(p) = &mut a.platform.player {
            p.wait();
        }
        if let Some(c) = &mut clock {
            c.wait(fast::frame());
        }
        hold.started();
        if s.paused() {
            hold.reset();
            // The sound stops (the queue emptied) until the menu lets go.
            if let Some(p) = &mut a.platform.player {
                p.silence();
            }
            s.wait_paused();
            if let Some(c) = &mut clock {
                c.reset();
            }
        }
        let i = s.input();
        open = i.open;
        a.platform.held = (i.keys | i.seats[0]) & key::ALL;
        b.platform.held = (i.keys2 | i.seats[1]) & key::ALL;
    }
    for (f, m) in savs.iter_mut().zip([&mut a, &mut b]) {
        if let Some(f) = f {
            f.flush(&mut m.bus.save)?;
            if screen.is_some() {
                crate::back_up_save(&f.path);
            }
        }
    }
    if let Some(p) = &a.platform.player {
        eprintln!("gbaa-desktop: {}", p.summary());
    }
    side_by_side(&[&a.platform.frame, &b.platform.frame], &mut wide);
    if let Some(path) = &opts.dump {
        write_png_wide(path, &wide).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if opts.hash {
        println!("frame {frames} hash {:016x}", pixels_hash(&wide));
    }
    println!("linked: {frames} frames, {} transfers", link.transfers);
    Ok(())
}
