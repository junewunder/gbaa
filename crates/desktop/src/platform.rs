//! [`Desktop`], the desktop's [`Platform`]: the window's [`Screen`]
//! ([`crate::window`]: drawn and read on the main thread), or nothing at
//! all for headless runs, and the sound played through the host's device.
//! Fast-forward ([`crate::fast`]) is held and toggled as the mapping says
//! (Tab and ` on the keyboard by default; [`crate::gamepad`]). The save-state slots' keys and buttons
//! are read here too ([`crate::slots`]), and rewind's (`rewind` in the mapping, R by default;
//! [`crate::rewind`]). Fullscreen (the Steam Deck's
//! Game Mode) draws the frame at the largest whole scale that fits the
//! screen, centred on black ([`fit`]). The window's menu pauses the game
//! here, and sets fast-forward's speed. A netplay seat's machine
//! ([`netplay::gba::Seat`]) has a [`Pad`] instead, and nothing else of
//! this runs for it.

use crate::audio::Player;
use crate::capture::Capture;
use crate::fast::{self, Clock, DrawPacer, FastForward};
use crate::gamepad::{self, Pads};
use crate::input::Script;
use crate::present::Hold;
use crate::slots::Slots;
use crate::window::Screen;
use netplay::gba::{Pad, Seat};
use runtime::io::key;
use runtime::platform::{HEIGHT, WIDTH};
use runtime::{AudioChunk, Frame, Platform};
use std::time::{Duration, Instant};

/// The desktop front end.
pub struct Desktop {
    window: Option<Screen>,
    /// Pressed on top of the keyboard.
    script: Script,
    /// Gamepads, pressed on top of the keyboard too.
    pads: Option<Pads>,
    /// Frames presented so far.
    shown: u64,
    /// The sound, kept for headless dumps (`--wav` and the like).
    pub capture: Option<Capture>,
    /// Playback, which also paces the emulator (docs/sound-design.md).
    pub player: Option<Player>,
    /// The pacer when there's no sound to pace by: a frame clock at
    /// 59.73 Hz (the muted window).
    clock: Option<Clock>,
    /// Fast-forward, and whether it was on for the last frame.
    pub fast: FastForward,
    fast_was: bool,
    /// Fast-forward's own clock, when it has a speed.
    fast_clock: Clock,
    /// When fast-forward next redraws the window.
    draws: DrawPacer,
    /// How long a frame waits before going to the window, so frames reach
    /// the display evenly ([`crate::present`]).
    hold: Hold,
    /// The save-state slots' controls.
    pub slots: Slots,
    /// F6 asked for `--extract`'s export since the frame loop last looked
    /// ([`Desktop::take_export`]).
    export: bool,
    /// Rewind is held, and whether it was for the last frame shown.
    pub rewind: bool,
    rewinding: bool,
    /// The menu paused the game since the last frame: the pacers start
    /// afresh.
    resumed: bool,
    /// False once the window is closed or Ctrl+Q is pressed.
    open: bool,
    /// The keys pressed at the last read, and what a Lua script set to
    /// press instead at the next (`--script`'s `keysRead`).
    pub keys_held: u16,
    pub script_keys: Option<u16>,
    /// A netplay seat's pad: when there is one, the machine sees it alone.
    seat: Option<Pad>,
}

impl Desktop {
    /// No window: frames are dropped (the caller dumps the last one) and
    /// only scripted keys are pressed.
    pub fn headless() -> Desktop {
        Desktop {
            window: None,
            script: Script::default(),
            pads: None,
            shown: 0,
            capture: None,
            player: None,
            clock: None,
            fast: FastForward::default(),
            fast_was: false,
            fast_clock: Clock::default(),
            draws: DrawPacer::default(),
            hold: Hold::default(),
            slots: Slots::default(),
            export: false,
            rewind: false,
            rewinding: false,
            resumed: false,
            open: true,
            keys_held: 0,
            script_keys: None,
            seat: None,
        }
    }

    /// Pace by a frame clock at 59.73 Hz, as the muted window does.
    pub fn with_clock(self) -> Desktop {
        Desktop {
            clock: Some(Clock::default()),
            ..self
        }
    }

    /// Fast-forward as `fast` says.
    pub fn with_fast(self, fast: FastForward) -> Desktop {
        Desktop { fast, ..self }
    }

    /// Keep the sound in `capture`.
    pub fn with_capture(self, capture: Capture) -> Desktop {
        Desktop {
            capture: Some(capture),
            ..self
        }
    }

    /// Press the buttons `script` says, frame by frame.
    pub fn with_input(self, script: Script) -> Desktop {
        Desktop { script, ..self }
    }

    /// Press the buttons `pads` hold.
    pub fn with_pads(self, pads: Pads) -> Desktop {
        Desktop {
            pads: Some(pads),
            ..self
        }
    }

    /// The gamepads, for feeding them events.
    pub fn pads(&mut self) -> Option<&mut Pads> {
        self.pads.as_mut()
    }

    /// Play the sound through `player`, which paces the emulator.
    pub fn with_player(self, player: Player) -> Desktop {
        Desktop {
            player: Some(player),
            ..self
        }
    }

    /// Draw on `screen`, with the sound playing at `gain` unless `mute`. Exactly
    /// one thing paces the emulator: the audio device when there is one,
    /// else a frame clock at 59.73 Hz. With no device it warns once and
    /// runs muted.
    pub fn window(screen: Screen, mute: bool, gain: f32) -> Desktop {
        let player = if mute {
            None
        } else {
            Player::open()
                .map(|mut p| {
                    p.set_gain(gain);
                    p
                })
                .inspect(|p| eprintln!("gbaa-desktop: audio: {}", p.describe()))
                .inspect_err(|e| eprintln!("gbaa-desktop: no audio ({e}); running muted"))
                .ok()
        };
        Desktop {
            clock: player.is_none().then(Clock::default),
            player,
            window: Some(screen),
            ..Desktop::headless()
        }
    }

    /// Whether the menu has resume on launch on ([`crate::resume`]); off
    /// with no window.
    pub fn resume_on(&self) -> bool {
        self.window.as_ref().is_some_and(Screen::resume)
    }

    /// Show `title` as the window's title (nothing headless).
    pub fn set_title(&mut self, title: &str) {
        if let Some(s) = &self.window {
            s.set_title(title);
        }
    }

    /// What the window asked of the scripts ([`crate::console`]) since
    /// the last look; nothing headless.
    pub fn take_script_cmds(&self) -> Vec<crate::console::ScriptCmd> {
        self.window
            .as_ref()
            .map(Screen::take_script_cmds)
            .unwrap_or_default()
    }

    /// The keyboard and mouse events for the scripts' `input` since the
    /// last look; nothing headless.
    pub fn take_script_input(&self) -> Vec<script::input::Event> {
        self.window
            .as_ref()
            .map(Screen::take_script_input)
            .unwrap_or_default()
    }

    /// Show what the scripts show in the window's console.
    pub fn show_scripts(&self, h: &crate::scripting::Host, console: Vec<String>) {
        if let Some(s) = &self.window {
            s.show_scripts(console, h.buffers(), h.scripts());
        }
    }

    /// Draw the scripts' canvas layers over the game in the window.
    pub fn show_overlay(&self, h: &crate::scripting::Host) {
        if let Some(s) = &self.window {
            s.set_overlay(h.overlay());
        }
    }

    /// Whether F6 asked for `--extract`'s export since the last call.
    pub fn take_export(&mut self) -> bool {
        std::mem::take(&mut self.export)
    }

    /// Say whether there's a state load to undo, for the window's menu.
    pub fn set_undoable(&mut self, undoable: bool) {
        if let Some(s) = &self.window {
            s.set_undoable(undoable);
        }
    }

    /// Show `frame`, the state's after frame `shown` that rewind stepped
    /// back to, and read the controls for the next. The sound stops while
    /// rewinding (the queue emptied), and the window keeps the frame clock.
    pub fn rewound(&mut self, shown: u64, frame: &Frame) {
        self.shown = shown;
        if !self.rewinding {
            self.rewinding = true;
            if let Some(p) = &mut self.player {
                p.silence();
            }
            self.fast_clock.reset();
        }
        self.draw(frame, false);
        if self.window.is_some() {
            self.fast_clock.wait(fast::frame());
        }
        self.keys();
    }

    /// Redraw the window with `frame`, fast-forwarded or not (the exit
    /// report counts those apart).
    fn draw(&mut self, frame: &Frame, fast: bool) {
        if let Some(s) = &self.window {
            s.show(frame, fast);
        }
    }

    /// How long the pacers have held the emulator back, in all: the frame
    /// clocks' sleeps and the sound pacer's waits.
    pub fn paced(&self) -> Duration {
        self.clock.as_ref().map_or(Duration::ZERO, |c| c.slept)
            + self.fast_clock.slept
            + self.hold.slept
            + self.player.as_ref().map_or(Duration::ZERO, |p| p.waited)
    }

    /// False once the window is closed or Ctrl+Q is pressed. Always true
    /// headless.
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// Play at `volume` percent, or `muted`: no gain, the device still pacing
/// the emulator. Muting or unmuting empties the queue, as fast-forward
/// does, so what's queued doesn't play on at the old level.
pub(crate) fn set_sound(p: &mut Player, volume: u32, muted: bool) {
    let gain = if muted {
        0.0
    } else {
        crate::audio::volume_gain(volume)
    };
    if (gain == 0.0) != (p.gain() == 0.0) {
        p.silence();
    }
    p.set_gain(gain);
}

/// Where the frame goes on a `w`×`h` screen: the largest whole scale that
/// fits (at least 1), and the left and top margins that centre it. The
/// Deck's 1280×800 takes 5, with 40-pixel bars left and right.
pub(crate) fn fit(w: usize, h: usize) -> (usize, usize, usize) {
    fit_wide(w, h, WIDTH)
}

/// [`fit`] for a frame `wide` pixels across: screens side by side.
pub(crate) fn fit_wide(w: usize, h: usize, wide: usize) -> (usize, usize, usize) {
    let scale = (w / wide).min(h / HEIGHT).max(1);
    (
        scale,
        w.saturating_sub(wide * scale) / 2,
        h.saturating_sub(HEIGHT * scale) / 2,
    )
}

/// A recompiled game's netplay seat: its code is built for [`Desktop`].
impl Seat for Desktop {
    fn seat() -> Desktop {
        Desktop {
            seat: Some(Pad::default()),
            ..Desktop::headless()
        }
    }

    fn pad(&self) -> &Pad {
        self.seat.as_ref().expect("a netplay seat")
    }

    fn pad_mut(&mut self) -> &mut Pad {
        self.seat.as_mut().expect("a netplay seat")
    }
}

impl Platform for Desktop {
    fn present(&mut self, frame: &Frame) {
        if let Some(p) = &mut self.seat {
            return p.present(frame);
        }
        self.shown += 1;
        let fast = self.fast.active();
        if fast != self.fast_was || self.rewinding || self.resumed {
            self.rewinding = false;
            self.resumed = false;
            self.fast_was = fast;
            if let Some(p) = &mut self.player {
                p.silence();
            }
            if let Some(c) = &mut self.clock {
                c.reset();
            }
            self.fast_clock.reset();
            self.draws.reset();
            self.hold.reset();
        }
        if let Some(c) = &mut self.capture {
            c.end_frame();
        }
        if let Some(p) = &mut self.player {
            if fast {
                p.drop_frame();
            } else {
                p.end_frame();
            }
        }
        // At normal speed a frame goes to the window on the pacer's even
        // beat, not when done: a frame whose finish crossed a refresh the
        // one before didn't would be doubled and the next skipped
        // (deck/even-presents).
        if !fast && self.window.is_some() {
            self.hold.before_show();
        }
        if !fast || self.draws.due(Instant::now()) {
            self.draw(frame, fast);
        }
        if fast {
            if let Some(period) = self.fast.period() {
                self.fast_clock.wait(period);
            }
        } else {
            if let Some(p) = &mut self.player {
                p.wait();
            }
            if let Some(c) = &mut self.clock {
                c.wait(fast::frame());
            }
            self.hold.started();
        }
    }

    fn audio(&mut self, chunk: &AudioChunk<'_>) {
        if let Some(p) = &mut self.seat {
            return p.audio(chunk);
        }
        if let Some(c) = &mut self.capture {
            c.add(chunk);
        }
        if let Some(p) = &mut self.player {
            p.audio(chunk);
        }
    }

    /// Read after each frame's `present`, so frame `shown - 1`'s keys.
    /// Fast-forward's, the save-state slots' and rewind's controls are
    /// read here too, for the next frame.
    fn keys(&mut self) -> u16 {
        if let Some(p) = &mut self.seat {
            return p.keys();
        }
        if let Some(s) = &self.window
            && s.paused()
        {
            // The sound stops (the queue emptied) until the menu lets go.
            if let Some(p) = &mut self.player {
                p.silence();
            }
            s.wait_paused();
            self.resumed = true;
        }
        let window = self.window.as_ref().map(Screen::input).unwrap_or_default();
        if let Some(v) = window.speed {
            self.fast.speed = v;
        }
        if let Some((volume, muted)) = window.sound
            && let Some(p) = &mut self.player
        {
            set_sound(p, volume, muted);
        }
        // The pads' and the keyboard's, read through one mapping.
        let pads = self.pads.as_ref().map_or(0, Pads::keys) | window.pads | window.keys;
        let held = self.script.keys(self.shown.saturating_sub(1)) | (pads & key::ALL);
        self.fast
            .input(pads & gamepad::FAST != 0, pads & gamepad::FAST_TOGGLE != 0);
        self.rewind = pads & gamepad::REWIND != 0;
        self.slots.input(pads, window.slot, window.undo);
        self.export |= window.export;
        // Closed, or the menu asked for netplay, which takes over.
        if self.window.is_some() {
            self.open = window.open && !window.netplay;
        }
        let held = self.script_keys.take().unwrap_or(held);
        self.keys_held = held;
        held
    }
}
