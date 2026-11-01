//! The window, through SDL3: made, drawn and read on the main thread, as
//! SDL (and macOS) want, while the game runs on its own thread
//! ([`crate::main`]) and hands each frame over through a [`Screen`].
//!
//! SDL's renderer draws the frame (Vulkan, OpenGL, Metal, Direct3D or its
//! software renderer, as it picks) at the largest whole scale that fits,
//! centred on black (`SDL_SetRenderLogicalPresentation` with
//! `INTEGER_SCALE`), in the window's pixels, so fractional display scaling
//! doesn't blur it. The keyboard and the gamepads are read here too, and
//! what they hold is shared for the game thread to read after each frame.
//! The main thread draws as soon as a frame arrives and never waits for
//! vblank: the sound paces the game (docs/sound-design.md), one clock.
//!
//! Fullscreen (`--fullscreen`, then F11, Alt+Enter or the menu's row to
//! switch it) is SDL's borderless desktop fullscreen: the window takes the
//! monitor it's on at the monitor's own size, which SDL asks of the window
//! manager (X11) or compositor (Wayland), fractional scaling included, and
//! the frame is fitted to the pixels that gives as to any window's. Back
//! in a window, it returns to the size it had.
//!
//! The menu ([`crate::menu`], in egui) is drawn over the frame through the
//! same renderer ([`crate::egui_sdl`]). While it's open the game's
//! buttons are let go, and while it says so (never in a netplay session,
//! where the others play on) the game thread waits in
//! [`Screen::wait_paused`]. The script console ([`crate::console`]) is
//! drawn in the same egui pass: beside the frame in a window, which widens
//! for it, and over it fullscreen. While its line has the keyboard, keys
//! go to it and not the game, and SDL's text input is on (Steam's
//! on-screen keyboard on the Deck).

use crate::console::{ScriptCmd, ScriptView};
use crate::gamepad::{self, Control, PadEvent, Pads};
use crate::menu::{self, Echoes, Menu, MenuPad, Nav, Repeat, SoundKey, Source};
use crate::{egui_sdl, room, slots};
use runtime::Frame;
use runtime::platform::{HEIGHT, WIDTH};
use script::canvas::Overlay;
use sdl3::event::{Event, EventSender, WindowEvent};
use sdl3::gamepad::{Button, Gamepad};
use sdl3::keyboard::{Mod, Scancode};
use sdl3::mouse::MouseButton;
use sdl3::pixels::{Color, PixelFormat};
use sdl3::render::{FRect, ScaleMode};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The keys for save-state slots 1 to 4: load, or save with Shift.
pub const SLOT_KEYS: [Scancode; slots::SLOTS as usize] =
    [Scancode::F1, Scancode::F2, Scancode::F3, Scancode::F4];

/// The volume key `sc` is, if any: - (or the keypad's) down, = (or the
/// keypad's +) up, M mute. None is in the default mapping or player 2's;
/// one a mapping binds to a button is that button's instead.
pub fn sound_key(sc: Scancode) -> Option<SoundKey> {
    match sc {
        Scancode::Minus | Scancode::KpMinus => Some(SoundKey::Down),
        Scancode::Equals | Scancode::KpPlus => Some(SoundKey::Up),
        Scancode::M => Some(SoundKey::Mute),
        _ => None,
    }
}

/// Whether a key pressed during play opens the menu: F10, and Escape
/// while no pad is open. Steam's desktop controller layout sends Escape
/// for a pad's B (see [`quits`]), and SDL sees that pad too, so with a
/// pad open Escape would open the menu at every B.
pub fn opens_menu(sc: Scancode, pad_open: bool) -> bool {
    sc == Scancode::F10 || (sc == Scancode::Escape && !pad_open)
}

/// Whether a key pressed with `keymod` held quits: Ctrl+Q only, and on
/// macOS Cmd+Q as well. Never Escape, which Steam's desktop controller
/// layout sends for the Deck's B (and other layouts for a face button),
/// so a press of B closed the game; nor any key alone. Outside the menu
/// Escape opens it, as [`opens_menu`] says.
pub fn quits(sc: Scancode, keymod: Mod) -> bool {
    let held = if cfg!(target_os = "macos") {
        Mod::LCTRLMOD | Mod::RCTRLMOD | Mod::LGUIMOD | Mod::RGUIMOD
    } else {
        Mod::LCTRLMOD | Mod::RCTRLMOD
    };
    sc == Scancode::Q && keymod.intersects(held)
}

/// Either Alt key.
const ALT: Mod = Mod::LALTMOD.union(Mod::RALTMOD);

/// Whether a key pressed with `keymod` held switches fullscreen on or
/// off: F11, or Alt+Enter (the keypad's too).
pub fn switches_fullscreen(sc: Scancode, keymod: Mod) -> bool {
    sc == Scancode::F11
        || matches!(sc, Scancode::Return | Scancode::KpEnter) && keymod.intersects(ALT)
}

/// Whether `window` is fullscreen (or the window system has yet to say
/// it has left), or fills its display as if it were: gamescope (the
/// Deck's Game Mode) fits every window to its output and never says it's
/// fullscreen, so a window that widened for the sidebars or the console
/// there was only shrunk back. `ours` is whether we have widened it
/// ourselves: a desktop window manager clamps a widened window to the
/// screen, so one that fills it then is still a window to shrink back.
fn is_fullscreen(window: &sdl3::video::Window, ours: bool) -> bool {
    window.fullscreen_state() != sdl3::video::FullscreenType::Off
        || window
            .get_display()
            .and_then(|d| d.get_bounds())
            .is_ok_and(|r| fills(window.size(), (r.width(), r.height()), ours))
}

/// Whether a window of `size` fills a display of `display`, both in
/// points, and so counts as fullscreen: never one we widened (`ours`).
fn fills(size: (u32, u32), display: (u32, u32), ours: bool) -> bool {
    !ours && size.0 >= display.0 && size.1 >= display.1 && display.0 > 0
}

/// Ask for `window` fullscreen, at the monitor's own size, or back in its
/// window.
fn set_fullscreen(window: &mut sdl3::video::Window, on: bool) {
    // No fullscreen mode set is SDL's desktop fullscreen: the monitor as
    // the desktop has it, in its logical size.
    if let Err(e) = window.set_fullscreen(on) {
        eprintln!("gbaa-desktop: fullscreen: {e}");
    }
}

/// What the keyboard and pads hold, as the game thread reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    /// What the keyboard holds, through the mapping's keyboard lines
    /// ([`gamepad::Mapping::pressed`]): the GBA's buttons and the front
    /// end's, as [`Pads::keys`].
    pub keys: u16,
    /// What the pads hold ([`Pads::keys`]).
    pub pads: u16,
    /// Linked play ([`Options::screens`] 2): what player 2's keys on the
    /// keyboard hold ([`gamepad::player2_keys`]), and what the first and
    /// second pads plugged in hold ([`Pads::keys_of`]), each player's.
    pub keys2: u16,
    pub seats: [u16; 2],
    /// A slot key pressed since the game thread last looked, and whether
    /// with Shift (save).
    pub slot: Option<(u8, bool)>,
    /// F5 or the menu's row asked to undo the last state load, since the
    /// game thread last looked.
    pub undo: bool,
    /// F6 asked for `--extract`'s export, since the game thread last
    /// looked.
    pub export: bool,
    /// Fast-forward's speed, set in the menu since the game thread last
    /// looked.
    pub speed: Option<f64>,
    /// The volume (percent) and whether muted, set in the menu since the
    /// game thread last looked.
    pub sound: Option<(u32, bool)>,
    /// False once the window is closed or Ctrl+Q is pressed.
    pub open: bool,
    /// The menu asked for a netplay session ([`Screen::take_netplay_ask`]):
    /// the game alone stops for it.
    pub netplay: bool,
    /// The menu asked to leave the room: the session stops.
    pub leave: bool,
}

/// What the two threads share.
struct Shared {
    /// The newest frame (the screens side by side), whether the window
    /// hasn't drawn it yet, and whether it was fast-forwarded.
    frame: Vec<u16>,
    fresh: bool,
    fast: bool,
    title: Option<String>,
    input: Input,
    /// The game thread has a state load to undo.
    undoable: bool,
    /// The menu has resume on launch on ([`crate::resume`]).
    resume: bool,
    /// What the menu reports of a netplay session ([`Screen::set_report`]).
    report: Vec<String>,
    /// Where netplay is ([`Screen::set_netplay`]), whether it changed since
    /// the window last looked, the session the menu asked for and not yet
    /// taken, and whether it asked to leave the room.
    netplay: room::State,
    netplay_fresh: bool,
    netplay_ask: Option<room::Ask>,
    leave: bool,
    /// The menu has paused the game.
    paused: bool,
    /// The menu is open (paused or not: a netplay session plays on).
    menu_open: bool,
    /// The game thread has finished.
    done: bool,
    /// What the scripts show, and whether it changed since the window
    /// last looked; what the window asks of them, not yet taken.
    scripts: ScriptView,
    scripts_fresh: bool,
    script_cmds: Vec<ScriptCmd>,
    /// The keyboard and mouse, for the scripts' `input`, not yet taken.
    script_input: Vec<script::input::Event>,
    /// The scripts' canvas layers, over the first screen and beside the
    /// screens ([`script::Host::overlay`]), and whether they changed since
    /// the window last looked.
    overlay: Option<Overlay>,
    overlay_fresh: bool,
}

/// The shared state, and what wakes a paused game thread.
struct Sync {
    state: Mutex<Shared>,
    resumed: Condvar,
}

impl Sync {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        // A panic on the other thread ends the run anyway.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What wakes the main thread.
enum Note {
    Frame,
    Done,
}

/// The game thread's side of the window: one run's, then a netplay
/// session's, each with its own copy.
#[derive(Clone)]
pub struct Screen {
    shared: Arc<Sync>,
    wake: Arc<EventSender>,
}

impl Screen {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock()
    }

    /// Hand `frame` to the window to draw; `fast` if fast-forwarded.
    pub fn show(&self, frame: &Frame, fast: bool) {
        self.show_screens(&[frame], fast);
    }

    /// Hand the window a frame per screen ([`Options::screens`]), drawn
    /// side by side, the first on the left.
    pub fn show_screens(&self, frames: &[&Frame], fast: bool) {
        let mut s = self.lock();
        side_by_side(frames, &mut s.frame);
        s.fast = fast;
        let wake = !s.fresh;
        s.fresh = true;
        drop(s);
        // One wake-up per frame drawn: a frame that replaces one not yet
        // drawn rides on its wake-up.
        if wake {
            let _ = self.wake.push_custom_event(Note::Frame);
        }
    }

    /// Show `title` as the window's title.
    pub fn set_title(&self, title: &str) {
        self.lock().title = Some(title.to_owned());
        let _ = self.wake.push_custom_event(Note::Frame);
    }

    /// What's held now, taking the slot key pressed since last time.
    pub fn input(&self) -> Input {
        let mut s = self.lock();
        let i = s.input.clone();
        s.input.slot = None;
        s.input.speed = None;
        s.input.sound = None;
        s.input.undo = false;
        s.input.export = false;
        i
    }

    /// Whether the window is still open.
    pub fn is_open(&self) -> bool {
        self.lock().input.open
    }

    /// Take the netplay session the menu asked for, if any (and forget any
    /// leaving asked for before it).
    pub fn take_netplay_ask(&self) -> Option<room::Ask> {
        let mut s = self.lock();
        s.leave = false;
        s.input.leave = false;
        s.input.netplay = false;
        s.netplay_ask.take()
    }

    /// A session over: say where netplay is now, and forget leaving.
    pub fn end_session(&self, state: room::State) {
        let mut s = self.lock();
        s.leave = false;
        s.input.leave = false;
        drop(s);
        self.set_netplay(state);
    }

    /// Say where netplay is, for the menu's Netplay page.
    pub fn set_netplay(&self, state: room::State) {
        let mut s = self.lock();
        if s.netplay == state {
            return;
        }
        s.netplay = state;
        s.netplay_fresh = true;
        drop(s);
        let _ = self.wake.push_custom_event(Note::Frame);
    }

    /// Say whether there's a state load to undo, for the menu's row.
    pub fn set_undoable(&self, undoable: bool) {
        self.lock().undoable = undoable;
    }

    /// Whether the menu has resume on launch on.
    pub fn resume(&self) -> bool {
        self.lock().resume
    }

    /// Show `lines` (a netplay session's end lines so far) in the menu.
    pub fn set_report(&self, lines: Vec<String>) {
        let mut s = self.lock();
        if s.report == lines {
            return;
        }
        s.report = lines;
        drop(s);
        let _ = self.wake.push_custom_event(Note::Frame);
    }

    /// What the window asked of the scripts since the game thread last
    /// looked.
    pub fn take_script_cmds(&self) -> Vec<ScriptCmd> {
        std::mem::take(&mut self.lock().script_cmds)
    }

    /// The keyboard and mouse events for the scripts since the game thread
    /// last looked.
    pub fn take_script_input(&self) -> Vec<script::input::Event> {
        std::mem::take(&mut self.lock().script_input)
    }

    /// Show the scripts' new console lines, their buffers and the scripts
    /// loaded.
    pub fn show_scripts(
        &self,
        console: Vec<String>,
        buffers: Vec<(String, String)>,
        loaded: Vec<(u32, std::path::PathBuf)>,
    ) {
        let mut s = self.lock();
        if console.is_empty() && s.scripts.buffers == buffers && s.scripts.loaded == loaded {
            return;
        }
        s.scripts.add_console(console);
        s.scripts.buffers = buffers;
        s.scripts.loaded = loaded;
        s.scripts_fresh = true;
        drop(s);
        let _ = self.wake.push_custom_event(Note::Frame);
    }

    /// Draw `overlay` (the scripts' canvas layers) over the first screen,
    /// and its sidebars beside the screens, from now on; `None` for none.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        let mut s = self.lock();
        if s.overlay == overlay {
            return;
        }
        s.overlay = overlay;
        let wake = !s.fresh && !s.overlay_fresh;
        s.overlay_fresh = true;
        drop(s);
        if wake {
            let _ = self.wake.push_custom_event(Note::Frame);
        }
    }

    /// Whether the menu has paused the game.
    pub fn paused(&self) -> bool {
        self.lock().paused
    }

    /// Whether the menu is open, paused or not.
    pub fn menu_open(&self) -> bool {
        self.lock().menu_open
    }

    /// Wait while the menu keeps the game paused (and the window open).
    pub fn wait_paused(&self) {
        let s = self.lock();
        let _s = self
            .shared
            .resumed
            .wait_while(s, |s| {
                s.paused && s.input.open && s.netplay_ask.is_none() && !s.leave
            })
            .unwrap_or_else(|e| e.into_inner());
    }
}

/// Tells the main thread the game thread is done, however it ends (a
/// panic too).
struct Finished(Screen);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.lock().done = true;
        let _ = self.0.wake.push_custom_event(Note::Done);
    }
}

/// How to open the window.
pub struct Options {
    /// The frame's scale in a window, in the screen's pixels.
    pub scale: usize,
    /// Start fullscreen, borderless and pointerless.
    pub fullscreen: bool,
    /// The pads' mapping, for the ROM being played.
    pub controls: crate::remap::Controls,
    /// Fast-forward's speed to start the menu at.
    pub fast_speed: f64,
    /// The volume, in percent, to start the menu at.
    pub volume: u32,
    /// Controls for a virtual pad to press ([`gamepad::Script`]).
    pub pad_script: Option<gamepad::Script>,
    /// GBA screens side by side: 1, or 2 for two linked GBAs
    /// ([`crate::link`]).
    pub screens: usize,
    /// The netplay server the menu offers.
    pub server: String,
    /// The Controls page shows each direction on its own row.
    pub directions_apart: bool,
    /// The sticks' and the triggers' deadzones, in percent.
    pub deadzones: [u32; 2],
    /// The sticks' left and right take a deadzone apart from up and down.
    pub deadzone_axes_apart: bool,
    /// Those two, in percent, as the settings file has them.
    pub axis_deadzones: [Option<u32>; 2],
    /// Resume on launch, as the menu starts it.
    pub resume: bool,
    /// The menu's Save folders page, as the settings stand.
    pub saves: crate::menu::saves::SaveFolders,
}

/// `frames` side by side in `out`, a row of each in turn.
pub fn side_by_side(frames: &[&Frame], out: &mut [u16]) {
    let n = frames.len();
    for (y, row) in out.chunks_exact_mut(WIDTH * n).take(HEIGHT).enumerate() {
        for (f, part) in frames.iter().zip(row.as_chunks_mut::<WIDTH>().0) {
            part.copy_from_slice(&f[y * WIDTH..][..WIDTH]);
        }
    }
}

/// Open the window as `o` says and run `game` on its own thread (with a
/// big stack: [`crate::game_thread`]), drawing what it shows and reading
/// the controls for it, until it returns.
pub fn run<T: Send + 'static>(
    o: Options,
    game: impl FnOnce(Screen) -> T + Send + 'static,
) -> Result<T, String> {
    let sdl = sdl3::init().map_err(|e| format!("starting SDL: {e}"))?;
    let video = sdl
        .video()
        .map_err(|e| format!("starting SDL's video: {e}"))?;
    let events = sdl
        .event()
        .map_err(|e| format!("starting SDL's events: {e}"))?;
    events
        .register_custom_event::<Note>()
        .map_err(|e| format!("starting SDL's events: {e}"))?;
    let pad_system = sdl
        .gamepad()
        .inspect_err(|e| eprintln!("gbaa-desktop: no gamepads ({e}); keyboard only"))
        .ok();
    // `--fullscreen` opens it fullscreen at the monitor's size in points:
    // an X11 screen with no window manager (the Deck check's Xvfb) never
    // resizes a window asked to go fullscreen later.
    let wide = WIDTH * o.screens;
    let screen = o
        .fullscreen
        .then(|| {
            video
                .get_primary_display()
                .and_then(|d| d.get_bounds())
                .ok()
        })
        .flatten();
    let (w, h) = match screen {
        Some(r) => (r.width(), r.height()),
        None => ((wide * o.scale) as u32, (HEIGHT * o.scale) as u32),
    };
    let mut builder = video.window("gbaa", w, h);
    // Dragged to any size, the frame keeps to the largest whole scale that
    // fits, centred on black ([`crate::platform::fit`]).
    builder.high_pixel_density().resizable();
    if o.fullscreen {
        builder.fullscreen();
    }
    let mut window = builder
        .build()
        .map_err(|e| format!("opening a window: {e}"))?;
    // `scale` counts the screen's pixels; the window's size is in SDL's
    // points, which fractional scaling makes larger.
    let d = window.pixel_density();
    let d = if d > 0.0 { d } else { 1.0 };
    let points = |n: usize| (n as f32 / d).ceil() as u32;
    let windowed = (points(wide * o.scale), points(HEIGHT * o.scale));
    if d != 1.0 && !o.fullscreen {
        let _ = window.set_size(windowed.0, windowed.1);
    }
    // No smaller than the frame at 1x.
    let _ = window.set_minimum_size(points(wide), points(HEIGHT));
    // The pointer's hidden over a fullscreen game; shown in a window, or
    // while the menu or the console's panel is up, to click them.
    let mouse = sdl.mouse();
    mouse.show_cursor(!o.fullscreen);
    let mut pointer_shown = !o.fullscreen;
    // Opened fullscreen, its window size is the monitor's until it first
    // leaves fullscreen and is given `windowed`.
    let mut sized = !o.fullscreen;
    let mut canvas = window.into_canvas();
    let creator = canvas.texture_creator();
    let mut texture = creator
        .create_texture_streaming(PixelFormat::XRGB8888, wide as u32, HEIGHT as u32)
        .map_err(|e| format!("the window's texture: {e}"))?;
    texture.set_scale_mode(ScaleMode::Nearest);
    eprintln!(
        "gbaa-desktop: window: {}, {} renderer",
        video.current_video_driver(),
        canvas.renderer_name,
    );
    placed(canvas.window());
    let mut pump = sdl
        .event_pump()
        .map_err(|e| format!("starting SDL's events: {e}"))?;

    let shared = Arc::new(Sync {
        state: Mutex::new(Shared {
            frame: vec![0; wide * HEIGHT],
            fresh: false,
            fast: false,
            title: None,
            undoable: false,
            resume: o.resume,
            report: Vec::new(),
            netplay: room::State::Off,
            netplay_fresh: false,
            netplay_ask: None,
            leave: false,
            input: Input {
                open: true,
                ..Input::default()
            },
            paused: false,
            menu_open: false,
            done: false,
            scripts: ScriptView::default(),
            scripts_fresh: false,
            script_cmds: Vec::new(),
            script_input: Vec::new(),
            overlay: None,
            overlay_fresh: false,
        }),
        resumed: Condvar::new(),
    });
    let screen = || Screen {
        shared: shared.clone(),
        wake: Arc::new(events.event_sender()),
    };
    let (screen, finished) = (screen(), Finished(screen()));
    let game = crate::game_thread(move || {
        let _finished = finished;
        game(screen)
    });

    let mut pads = Pads::new(o.controls.mapping());
    let mut menu_pad = MenuPad::default();
    let mut menu = Menu::new(o.fast_speed, o.volume, o.controls);
    menu.settings = crate::settings::path();
    menu.server = o.server;
    menu.remap.apart = o.directions_apart;
    menu.deadzones = o.deadzones;
    menu.deadzone_axes_apart = o.deadzone_axes_apart;
    menu.axis_deadzones = o.axis_deadzones;
    menu.resume = o.resume;
    menu.saves = o.saves;
    pads.deadzones = menu.pad_deadzones();
    menu_pad.deadzones = pads.deadzones;
    // The menu bar in KDE's global menu, where there is one
    // (src/menubar/kde.rs). Declared after the window, so dropped before it.
    #[cfg(target_os = "linux")]
    let global_menu = {
        let wake = events.event_sender();
        crate::menubar::kde::GlobalMenu::start(
            canvas.window(),
            video.current_video_driver(),
            crate::menubar::State::default(),
            Box::new(move || {
                let _ = wake.push_custom_event(Note::Frame);
            }),
        )
    };
    // The app's menu bar on macOS (src/menubar/macos.rs), which replaces
    // the one SDL made, so only once the window exists.
    #[cfg(target_os = "macos")]
    let global_menu = {
        let wake = events.event_sender();
        crate::menubar::macos::MenuBar::start(
            crate::menubar::State::default(),
            Box::new(move || {
                let _ = wake.push_custom_event(Note::Frame);
            }),
        )
    };
    let mut echoes = Echoes::default();
    let mut held_move = Repeat::default();
    let mut painter = egui_sdl::Painter::new(&creator);
    let start = Instant::now();
    // When egui asks to be drawn again.
    let mut repaint: Option<Instant> = None;
    // After the menu closes, the game's buttons stay let go until all are.
    let mut latched = false;
    let mut menu_cost = Gaps::default();
    let mut script = match o.pad_script {
        Some(s) => Some(VirtualPad::attach(&sdl, s)?),
        None => None,
    };
    // Open pads by SDL's joystick ID: only opened ones send events.
    let mut open: HashMap<u32, Gamepad> = HashMap::new();
    let mut frame = vec![0u16; wide * HEIGHT];
    let mut pixels = vec![0u8; wide * HEIGHT * 4];
    let mut overlay: Option<Overlay> = None;
    // The frame's width with the scripts' sidebars, and how many of the
    // game's pixels they widened the window by.
    let mut shown_wide = wide;
    let mut sidebars = 0;
    let player2 = gamepad::player2_keys();
    let mut quit = false;
    let mut gaps = Gaps::default();
    // Frames shown with the menu open, and whether it was last time.
    let mut menu_gaps = Gaps::default();
    let mut shown_open = false;
    // Frames shown fast-forwarded, and whether the last one was.
    let mut fast_gaps = Gaps::default();
    let mut shown_fast = false;
    let keyboard = sdl.keyboard();
    let text_input = video.text_input();
    // SDL's text input is on (the console's line has the keyboard).
    let mut text_on = false;
    // The console button was held, and the window widened for the panel
    // (by these points).
    let mut console_held = false;
    let mut widened: Option<u32> = None;
    // For the scripts' `input`: the keys pressed while the game had the
    // keyboard, by scancode with the code each gave (their release gives
    // the same), whether the game has it, and where the frame was drawn.
    let mut script_pressed: HashMap<Scancode, u32> = HashMap::new();
    let mut script_events: Vec<script::input::Event> = Vec::new();
    let mut frame_at = (1, 0, 0);
    loop {
        let mut slot = None;
        let mut undo = false;
        let mut export = false;
        let mut redraw = false;
        let was_open = menu.open;
        let was_drawn = menu.drawn();
        let mut navs = Vec::new();
        let now = Instant::now();
        let mut wait = Duration::from_millis(100);
        for at in [
            repaint,
            script.as_ref().map(|s| s.next),
            menu.toast_until(now),
            held_move.next().filter(|_| menu.open),
        ]
        .into_iter()
        .flatten()
        {
            wait = wait.min(at.saturating_duration_since(now));
        }
        let first = if wait.is_zero() {
            pump.poll_event()
        } else {
            pump.wait_event_timeout(wait)
        };
        // When the batch came, not when the wait began: the echo filter
        // and the stick's and repeat's timings go by it.
        let now = Instant::now();
        if let Some(s) = &mut script {
            s.step(&open);
        }
        let density = canvas.window().pixel_density();
        menu_pad.listening = menu.listening();
        let keys_listening = menu.listening_for_keys();
        for ev in first.into_iter().chain(pump.poll_iter()) {
            if !menu.open {
                script_event(
                    &ev,
                    !menu.console.typing,
                    density,
                    frame_at,
                    &mut script_pressed,
                    &mut script_events,
                );
            }
            // A pad event goes to the game's buttons and to the menu.
            let mut menu_pad_feed =
                |id: u32, e: PadEvent, pads: &mut Pads, navs: &mut Vec<(Nav, Source)>| {
                    pads.feed(id, e);
                    let from = match e {
                        PadEvent::Axis(..) => Source::Stick(id),
                        _ => Source::Buttons(id),
                    };
                    let n = menu_pad.feed(id, e, was_open, now);
                    navs.extend(n.map(|n| (n, from)));
                };
            match ev {
                Event::Quit { .. } => quit = true,
                Event::KeyDown {
                    scancode: Some(sc),
                    keymod,
                    repeat,
                    ..
                } if menu.open => {
                    let ctrl = Mod::LCTRLMOD | Mod::RCTRLMOD;
                    if quits(sc, keymod) {
                        quit = true;
                    }
                    if switches_fullscreen(sc, keymod) {
                        if !repeat {
                            menu.fullscreen_asked = true;
                        }
                        continue;
                    }
                    // A field being typed in takes the keys, Enter and
                    // Escape ending it.
                    let n = match sc {
                        Scancode::Escape | Scancode::F10 if !repeat => Some(Nav::Back),
                        Scancode::Return | Scancode::KpEnter if menu.editing() && !repeat => {
                            Some(Nav::Ok)
                        }
                        Scancode::Backspace if menu.editing() => Some(Nav::Erase),
                        // Arrows held repeat at the menu's pace, not the
                        // keyboard's ([`Repeat`]).
                        Scancode::Up | Scancode::Down | Scancode::Left | Scancode::Right
                            if repeat =>
                        {
                            None
                        }
                        Scancode::Up if menu.editing() => Some(Nav::Up),
                        Scancode::Down if menu.editing() => Some(Nav::Down),
                        Scancode::Left if menu.editing() => Some(Nav::Left),
                        Scancode::Right if menu.editing() => Some(Nav::Right),
                        _ if menu.editing() => None,
                        _ if keys_listening => (!repeat).then_some(Nav::Bind(Control::Key(sc))),
                        Scancode::Backspace | Scancode::Delete if !repeat => Some(Nav::Erase),
                        Scancode::Z if keymod.intersects(ctrl) => (!repeat).then_some(Nav::Undo),
                        Scancode::Up => Some(Nav::Up),
                        Scancode::Down => Some(Nav::Down),
                        Scancode::Left => Some(Nav::Left),
                        Scancode::Right => Some(Nav::Right),
                        Scancode::Return | Scancode::Space | Scancode::X if !repeat => {
                            Some(Nav::Ok)
                        }
                        Scancode::Z if !repeat => Some(Nav::Back),
                        _ => None,
                    };
                    navs.extend(n.map(|n| (n, Source::Keys)));
                }
                // The console's line has the keyboard: keys edit it.
                Event::KeyDown {
                    scancode: Some(sc),
                    keymod,
                    repeat,
                    ..
                } if menu.console.typing => {
                    if quits(sc, keymod) {
                        quit = true;
                    }
                    if sc == Scancode::F10 && !repeat {
                        navs.push((Nav::Toggle, Source::Keys));
                    }
                    if switches_fullscreen(sc, keymod) {
                        if !repeat {
                            menu.fullscreen_asked = true;
                        }
                        continue;
                    }
                    if let Some(key) = egui_key(sc) {
                        for pressed in [true, false] {
                            menu.typed(egui::Event::Key {
                                key,
                                physical_key: None,
                                pressed,
                                repeat,
                                modifiers: egui::Modifiers::NONE,
                            });
                        }
                    }
                    redraw = true;
                }
                Event::TextInput { text, .. } if menu.editing() => {
                    menu.edit_text(&text);
                    redraw = true;
                }
                Event::TextInput { text, .. } => {
                    menu.typed(egui::Event::Text(text));
                    redraw = true;
                }
                Event::MouseWheel { x, y, .. } => {
                    menu.pointer(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(x, y),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: egui::Modifiers::NONE,
                    });
                    redraw = true;
                }
                Event::MouseMotion {
                    x, y, mousestate, ..
                } => {
                    menu.pointer(egui::Event::PointerMoved(pointer(
                        x,
                        y,
                        density,
                        menu_ppp(&canvas),
                    )));
                    // Drawn as it goes, or a drag (a scroll bar's, or a
                    // finger's on the list) shows only once let go. Over a
                    // running game, only a drag: the frames draw the rest.
                    redraw |= menu.open || (menu.console.shown && mousestate.left());
                }
                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                }
                | Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => {
                    menu.pointer(egui::Event::PointerButton {
                        pos: pointer(x, y, density, menu_ppp(&canvas)),
                        button: egui::PointerButton::Primary,
                        pressed: matches!(ev, Event::MouseButtonDown { .. }),
                        modifiers: egui::Modifiers::NONE,
                    });
                    redraw = true;
                }
                Event::KeyDown {
                    scancode: Some(sc),
                    keymod,
                    repeat: false,
                    ..
                } => {
                    if quits(sc, keymod) {
                        quit = true;
                    }
                    if opens_menu(sc, !open.is_empty()) {
                        navs.push((Nav::Toggle, Source::Keys));
                    }
                    if switches_fullscreen(sc, keymod) {
                        menu.fullscreen_asked = true;
                    }
                    if let Some(n) = SLOT_KEYS.iter().position(|&k| k == sc) {
                        let shift = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);
                        slot = Some((n as u8 + 1, shift));
                    }
                    if sc == Scancode::F5 {
                        undo = true;
                    }
                    if sc == Scancode::F6 {
                        export = true;
                    }
                    if let Some(k) = sound_key(sc)
                        && pads.mapping().pressed(|c| c == sc) == 0
                        && player2.pressed(|c| c == sc) == 0
                    {
                        menu.sound_key(k, Instant::now());
                        redraw = true;
                    }
                }
                Event::Window {
                    win_event:
                        WindowEvent::Exposed
                        | WindowEvent::Resized(..)
                        | WindowEvent::PixelSizeChanged(..)
                        | WindowEvent::DisplayChanged(..),
                    ..
                } => {
                    redraw = true;
                    if let Event::Window {
                        win_event: WindowEvent::PixelSizeChanged(..),
                        ..
                    } = ev
                    {
                        placed(canvas.window());
                    }
                }
                Event::GamepadAdded { which, .. } => {
                    if let Some(g) = pad_system.as_ref().and_then(|p| p.open(which).ok()) {
                        let name = g.name().unwrap_or_else(|| "unnamed".into());
                        eprintln!("gbaa-desktop: gamepad: {name}");
                        menu_pad_feed(
                            which.raw(),
                            PadEvent::Labels(labels(&g)),
                            &mut pads,
                            &mut navs,
                        );
                        open.insert(which.raw(), g);
                    }
                }
                Event::GamepadRemoved { which, .. } => {
                    menu_pad_feed(which.raw(), PadEvent::Gone, &mut pads, &mut navs);
                    open.remove(&which.raw());
                }
                Event::GamepadButtonDown { which, button, .. } => {
                    menu_pad_feed(which.raw(), PadEvent::Pressed(button), &mut pads, &mut navs)
                }
                Event::GamepadButtonUp { which, button, .. } => menu_pad_feed(
                    which.raw(),
                    PadEvent::Released(button),
                    &mut pads,
                    &mut navs,
                ),
                Event::GamepadAxisMotion {
                    which, axis, value, ..
                } => menu_pad_feed(
                    which.raw(),
                    PadEvent::Axis(axis, (f32::from(value) / 32767.0).max(-1.0)),
                    &mut pads,
                    &mut navs,
                ),
                ev if ev.is_user_event() => {
                    // Frees what the event carries; the shared state says
                    // the rest.
                    let _ = ev.as_user_event_type::<Note>();
                }
                _ => {}
            }
        }
        for (n, from) in navs.drain(..) {
            if echoes.keep(n, from, now) {
                held_move.pressed(n, now);
                menu.nav(n);
            }
            redraw = true;
        }
        // A direction still held moves again.
        if menu.open {
            let kb = pump.keyboard_state();
            let holds = |n: Nav| {
                let key = match n {
                    Nav::Up => Scancode::Up,
                    Nav::Down => Scancode::Down,
                    Nav::Left => Scancode::Left,
                    Nav::Right => Scancode::Right,
                    _ => return false,
                };
                kb.is_scancode_pressed(key) || menu_pad.holds(n)
            };
            if let Some(n) = held_move.due(Instant::now(), holds) {
                menu.nav(n);
                redraw = true;
            }
        } else {
            held_move = Repeat::default();
        }
        if menu.deadzones_changed {
            menu.deadzones_changed = false;
            pads.deadzones = menu.pad_deadzones();
            menu_pad.deadzones = pads.deadzones;
        }
        // The menu or the console's line took the keyboard: the scripts'
        // keys let go, as mGBA's do when its window loses focus.
        if (menu.open || menu.console.typing) && !script_pressed.is_empty() {
            script_pressed.clear();
            script_events.push(script::input::Event::ClearKeys);
        }
        // A menu bar row asks what its hotkey would.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        for action in global_menu.iter().flat_map(|g| g.clicks()) {
            action.ask(crate::menubar::Asks {
                slot: &mut slot,
                undo: &mut undo,
                fullscreen: &mut menu.fullscreen_asked,
                quit: &mut quit,
            });
        }
        if std::mem::take(&mut menu.fullscreen_asked) {
            // Asked of the window's own state: one that only fills the
            // display (gamescope) can still be asked to go fullscreen.
            let on = canvas.window().fullscreen_state() == sdl3::video::FullscreenType::Off;
            set_fullscreen(canvas.window_mut(), on);
            redraw = true;
        }
        let in_window = canvas.window().fullscreen_state() == sdl3::video::FullscreenType::Off;
        let pointer = in_window || menu.open || menu.console.shown;
        if pointer != pointer_shown {
            pointer_shown = pointer;
            mouse.show_cursor(pointer);
        }
        // On Wayland the compositor grants it a moment later: the menu's
        // row shows what the window is.
        let full = is_fullscreen(canvas.window(), widened.is_some() || sidebars != 0);
        if menu.fullscreen != full {
            menu.fullscreen = full;
            redraw = true;
            eprintln!(
                "gbaa-desktop: fullscreen {}",
                if full { "on" } else { "off" }
            );
            if !full && !sized {
                // A window manager may have maximized a window opened at
                // the monitor's size.
                let w = canvas.window_mut();
                let _ = w.restore();
                let _ = w.set_size(windowed.0, windowed.1);
                sized = true;
            }
        }
        if std::mem::take(&mut menu.mapping_changed) {
            pads.set_mapping(menu.controls.mapping());
        }
        if repaint.is_some_and(|t| t <= Instant::now()) {
            repaint = None;
            redraw = true;
        }
        let kb = pump.keyboard_state();
        // While the console's line has the keyboard, the game gets none.
        let typing = menu.console.typing;
        let down = |k| kb.is_scancode_pressed(k) && !typing;
        // Alt+Enter switches fullscreen: no Start with it.
        let alt = keyboard.mod_state().intersects(ALT);
        let mut input = Input {
            keys: pads
                .mapping()
                .pressed(|k| down(k) && !(alt && k == Scancode::Return)),
            pads: pads.keys(),
            keys2: player2.pressed(down),
            seats: [pads.keys_of(0), pads.keys_of(1)],
            slot,
            undo: undo || std::mem::take(&mut menu.undo),
            export,
            speed: std::mem::take(&mut menu.speed_changed).then_some(menu.speed),
            sound: std::mem::take(&mut menu.sound_changed).then_some((menu.volume, menu.muted)),
            open: !quit && !menu.quit,
            netplay: false,
            leave: false,
        };
        // The console button, at each press.
        let held = (input.keys | input.pads) & gamepad::CONSOLE != 0;
        if held && !console_held && !menu.open {
            menu.console.set_shown(!menu.console.shown);
            redraw = true;
        }
        console_held = held;
        // In a window, it widens for the panel; back when it's hidden.
        let full = is_fullscreen(canvas.window(), widened.is_some() || sidebars != 0);
        menu.console.overlay = full;
        let want = menu.console.shown && !full;
        if want != widened.is_some() && !full {
            let (ww, wh) = canvas.window().size();
            let ppp = menu_ppp(&canvas);
            let d = canvas.window().pixel_density().max(0.01);
            let add = (crate::console::PANEL_WIDTH * ppp / d).ceil() as u32;
            let w = canvas.window_mut();
            match widened.take() {
                Some(n) => {
                    let _ = w.set_size(ww.saturating_sub(n).max(1), wh);
                }
                None => {
                    let _ = w.set_size(ww + add, wh);
                    widened = Some(add);
                }
            }
            redraw = true;
        }
        // The line has the keyboard: text input on, where the line is
        // (Steam's on-screen keyboard opens over the rest).
        // Or a field of the menu's Netplay page is being typed in.
        if (menu.console.typing || menu.editing()) != text_on {
            text_on = menu.console.typing || menu.editing();
            if text_on {
                if let Some(r) = menu.console.line_rect.filter(|_| menu.console.typing) {
                    let ppp = menu_ppp(&canvas);
                    let r = sdl3::rect::Rect::new(
                        (r.min.x * ppp) as i32,
                        (r.min.y * ppp) as i32,
                        (r.width() * ppp) as u32,
                        (r.height() * ppp) as u32,
                    );
                    text_input.set_rect(canvas.window(), r, 0);
                }
                text_input.start(canvas.window());
            } else {
                text_input.stop(canvas.window());
            }
        }
        // The menu takes the controls while it's open, and until they're
        // all let go after.
        latched = menu.open || latched && (input.keys != 0 || input.pads != 0 || input.keys2 != 0);
        if latched {
            input = Input {
                speed: input.speed,
                sound: input.sound,
                undo: input.undo,
                export: input.export,
                open: input.open,
                ..Input::default()
            };
        }
        let (fresh, fast, title, done) = {
            let mut s = shared.lock();
            // A slot key or speed not yet taken stays.
            let slot = input.slot.or(s.input.slot);
            let speed = input.speed.or(s.input.speed);
            let sound = input.sound.or(s.input.sound);
            let undo = input.undo || s.input.undo;
            let export = input.export || s.input.export;
            s.input = Input {
                slot,
                speed,
                sound,
                undo,
                export,
                ..input
            };
            // Asked for and not yet done: nothing more to undo.
            menu.undoable = s.undoable && !undo;
            s.resume = menu.resume;
            if menu.report != s.report {
                menu.report = s.report.clone();
                redraw = true;
            }
            // Netplay asked for, or the room left: the game thread stops
            // what it's doing, paused or not.
            let asked = menu.netplay_ask.take();
            let leave = std::mem::take(&mut menu.leave);
            if asked.is_some() || leave {
                s.leave = leave;
                s.netplay_ask = asked.or(s.netplay_ask.take().filter(|_| !leave));
                shared.resumed.notify_all();
            }
            s.input.netplay = s.netplay_ask.is_some();
            s.input.leave = s.leave;
            if std::mem::take(&mut s.netplay_fresh) {
                menu.set_netplay(s.netplay.clone());
                redraw = true;
            }
            s.script_cmds.append(&mut menu.console.cmds);
            s.script_input.append(&mut script_events);
            if std::mem::take(&mut s.scripts_fresh) {
                menu.console.view = s.scripts.clone();
                redraw = true;
            }
            s.menu_open = menu.open;
            let paused = menu.open && menu.paused;
            if s.paused != paused || !s.input.open {
                s.paused = paused;
                shared.resumed.notify_all();
                // A pause isn't a hitch.
                gaps.last = None;
                menu_gaps.last = None;
                fast_gaps.last = None;
            }
            let mut fresh = std::mem::take(&mut s.fresh);
            if fresh {
                std::mem::swap(&mut frame, &mut s.frame);
            }
            if std::mem::take(&mut s.overlay_fresh) {
                overlay.clone_from(&s.overlay);
                fresh = true;
            }
            (fresh, s.fast, s.title.take(), s.done)
        };
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(g) = &global_menu {
            g.set(crate::menubar::State {
                undoable: menu.undoable,
                fullscreen: menu.fullscreen,
            });
        }
        if let Some(t) = title {
            let _ = canvas.window_mut().set_title(&t);
        }
        if fresh {
            let (sl, sr) = overlay.as_ref().map_or((0, 0), |o| (o.left, o.right()));
            let all = sl + wide + sr;
            if all != shown_wide {
                shown_wide = all;
                texture = creator
                    .create_texture_streaming(PixelFormat::XRGB8888, all as u32, HEIGHT as u32)
                    .map_err(|e| format!("the window's texture: {e}"))?;
                texture.set_scale_mode(ScaleMode::Nearest);
                pixels = vec![0; all * HEIGHT * 4];
            }
            // In a window, it widens for the sidebars at the game's scale.
            if sl + sr != sidebars
                && !is_fullscreen(canvas.window(), widened.is_some() || sidebars != 0)
            {
                let (ww, wh) = canvas.window().size();
                let d = canvas.window().pixel_density().max(0.01);
                let px = |n: usize| ((n * frame_at.0) as f32 / d).ceil() as u32;
                let ww = (ww + px(sl + sr)).saturating_sub(px(sidebars)).max(1);
                let _ = canvas.window_mut().set_size(ww, wh);
                sidebars = sl + sr;
            }
            to_xrgb_beside(&frame, wide, sl, all, &mut pixels);
            if let Some(o) = &overlay {
                draw_overlay(o, wide, &mut pixels);
            }
            texture
                .update(None, &pixels, all * 4)
                .map_err(|e| format!("the window's texture: {e}"))?;
        }
        // A frame shown while the menu was shut needs one more draw to
        // clear it away.
        if fresh || redraw || menu.drawn() != was_drawn || menu.open != was_open {
            canvas.set_draw_color(Color::BLACK);
            canvas.clear();
            let (w, h) = canvas
                .output_size()
                .map_err(|e| format!("the window's renderer: {e}"))?;
            // Beside the panel, in a window: the frame fits what's left.
            let side = if menu.console.shown && !menu.console.overlay {
                let ppp = menu_ppp(&canvas);
                let screen = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(w as f32 / ppp, h as f32 / ppp),
                );
                (crate::console::panel_rect(screen).width() * ppp) as u32
            } else {
                0
            };
            // Over the frame, fullscreen: the right sidebar's drawer opens
            // left of it.
            let cover = if menu.console.shown && menu.console.overlay {
                let ppp = menu_ppp(&canvas);
                let screen = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(w as f32 / ppp, h as f32 / ppp),
                );
                (crate::console::panel_rect(screen).width() * ppp) as usize
            } else {
                0
            };
            let sl = overlay.as_ref().map_or(0, |o| o.left);
            let placed = place(
                w.saturating_sub(side) as usize,
                h as usize,
                cover,
                (sl, wide, shown_wide - sl - wide),
                menu.console.shown,
            );
            frame_at = (placed.scale, placed.screens_x, placed.top);
            for (x, n, at_x) in placed.pieces {
                let src = FRect::new(x as f32, 0.0, n as f32, HEIGHT as f32);
                let at = FRect::new(
                    at_x as f32,
                    placed.top as f32,
                    (n * placed.scale) as f32,
                    (HEIGHT * placed.scale) as f32,
                );
                canvas
                    .copy(&texture, src, at)
                    .map_err(|e| format!("drawing the window: {e}"))?;
            }
            if menu.drawn() || was_drawn {
                let t = Instant::now();
                let out = menu.run((w, h), menu_ppp(&canvas), start.elapsed().as_secs_f64());
                let delay = out
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .map(|v| v.repaint_delay);
                let ctx = menu.ctx.clone();
                painter.paint(&mut canvas, &ctx, out)?;
                // Its layout settles a pass late, and a toast goes in a
                // few seconds: honour egui's asks (not its "never").
                if let Some(d) = delay.filter(|d| *d < Duration::from_secs(60)) {
                    repaint = Some(Instant::now() + d);
                }
                menu_cost.add(t.elapsed());
            }
            canvas.present();
            if fresh {
                // The gap that spans a switch between speeds is neither's.
                if fast != shown_fast {
                    shown_fast = fast;
                    gaps.last = None;
                    fast_gaps.last = None;
                }
                if menu.open {
                    menu_gaps.presented();
                } else if fast {
                    fast_gaps.presented();
                } else {
                    gaps.presented();
                }
            }
        }
        if menu.open != shown_open || menu.quit {
            if menu.open != shown_open {
                eprintln!(
                    "gbaa-desktop: menu {}",
                    if menu.open { "open" } else { "closed" }
                );
            }
            shown_open = menu.open;
            // Closed or quit in egui's pass: tell the game thread now.
            repaint = Some(Instant::now());
            gaps.last = None;
            menu_gaps.last = None;
            fast_gaps.last = None;
        }
        if done {
            break;
        }
    }
    if gaps.n > 0 {
        eprintln!("gbaa-desktop: {}", gaps.summary());
    }
    if fast_gaps.n > 0 {
        eprintln!("gbaa-desktop: fast-forward: {}", fast_gaps.summary());
    }
    if menu_gaps.n > 0 {
        eprintln!("gbaa-desktop: with the menu: {}", menu_gaps.summary());
    }
    if menu_cost.n > 0 {
        eprintln!(
            "gbaa-desktop: menu draws: {}, p50 {:.1} ms, p99 {:.1} ms, max {:.1} ms",
            menu_cost.n,
            menu_cost.quantile(0.5),
            menu_cost.quantile(0.99),
            menu_cost.max
        );
    }
    game.join().map_err(|_| "the game thread panicked".into())
}

/// The gaps between frames presented, to 0.1 ms, for a summary at the end
/// (pacing and hitching: docs/tools/commands.md, gbaa-desktop).
struct Gaps {
    last: Option<Instant>,
    /// Counts by gap in tenths of a millisecond; the last holds the rest.
    tenths: Vec<u32>,
    n: u64,
    /// The gaps' sum and the longest, in ms.
    sum: f64,
    max: f64,
}

impl Default for Gaps {
    fn default() -> Gaps {
        Gaps {
            last: None,
            tenths: vec![0; 1000],
            n: 0,
            sum: 0.0,
            max: 0.0,
        }
    }
}

impl Gaps {
    fn presented(&mut self) {
        let now = Instant::now();
        if let Some(t) = self.last.replace(now) {
            self.add(now - t);
        }
    }

    /// Count a gap (or a draw's time) of `d`.
    fn add(&mut self, d: Duration) {
        let ms = d.as_secs_f64() * 1000.0;
        let i = ((ms * 10.0) as usize).min(self.tenths.len() - 1);
        self.tenths[i] += 1;
        self.n += 1;
        self.sum += ms;
        self.max = self.max.max(ms);
    }

    /// The gap below which a share `q` of them fall, in ms.
    fn quantile(&self, q: f64) -> f64 {
        let want = (self.n as f64 * q).ceil() as u64;
        let mut seen = 0;
        for (i, &c) in self.tenths.iter().enumerate() {
            seen += u64::from(c);
            if seen >= want.max(1) {
                return (i + 1) as f64 / 10.0;
            }
        }
        self.max
    }

    fn summary(&self) -> String {
        let late: u32 = self.tenths[250..].iter().sum();
        format!(
            "presents: {} gaps, mean {:.1} ms, p50 {:.1} ms, p99 {:.1} ms, max {:.1} ms, over 25 ms: {late}",
            self.n,
            self.sum / self.n as f64,
            self.quantile(0.5),
            self.quantile(0.99),
            self.max
        )
    }
}

/// Say where SDL places the frame in `window` ([`crate::platform::fit`]
/// is the same rule).
fn placed(window: &sdl3::video::Window) {
    let (w, h) = window.size_in_pixels();
    let (scale, left, top) = crate::platform::fit(w as usize, h as usize);
    eprintln!(
        "gbaa-desktop: window {w}x{h} pixels ({:.2} per point): the frame at {scale}x, \
         {left} left and {top} top",
        window.pixel_density()
    );
}

/// Where `g`'s face buttons labelled A, B, X and Y are, as
/// [`gamepad::XBOX_LABELS`]; that layout itself unless all four positions
/// carry those letters.
fn labels(g: &Gamepad) -> [Button; 4] {
    use sdl3::gamepad::ButtonLabel as L;
    let mut at = [None; 4];
    for b in gamepad::XBOX_LABELS {
        let i = match g.button_label_for_gamepad_type(b) {
            L::A => 0,
            L::B => 1,
            L::X => 2,
            L::Y => 3,
            _ => continue,
        };
        at[i] = Some(b);
    }
    match at {
        [Some(a), Some(b), Some(x), Some(y)] => [a, b, x, y],
        _ => gamepad::XBOX_LABELS,
    }
}

/// `frame` (one screen or several side by side) as SDL's XRGB8888, four
/// bytes a pixel in memory order.
pub fn to_xrgb(frame: &[u16], out: &mut [u8]) {
    for (&c, px) in frame.iter().zip(out.as_chunks_mut::<4>().0) {
        let [r, g, b] = crate::dump::rgb(c);
        *px = u32::from_be_bytes([0, r, g, b]).to_ne_bytes();
    }
}

/// Where [`place`] puts the frame.
#[derive(Debug, PartialEq)]
struct Placed {
    scale: usize,
    top: usize,
    /// The screens' left edge in the window.
    screens_x: usize,
    /// The texture's pieces drawn, in order: its x, its width, and the
    /// window's x it's drawn at.
    pieces: Vec<(usize, usize, usize)>,
}

/// Where the frame goes in a `w`×`h` window (the console's panel beside
/// it left out): the texture `sl + wide + sr` across, the scripts'
/// sidebars either side of the screens. When the sidebars would bring
/// the screens below the whole scale they'd have alone (the Deck at
/// 1280x800, fullscreen), the sidebars are a drawer: the screens alone,
/// centred, and while `open` (the console's toggle) the sidebars over
/// them at the window's edges, the right one left of the `cover` pixels
/// the console's panel takes over the frame.
fn place(
    w: usize,
    h: usize,
    cover: usize,
    (sl, wide, sr): (usize, usize, usize),
    open: bool,
) -> Placed {
    let all = sl + wide + sr;
    let (scale, left, top) = crate::platform::fit_wide(w, h, all);
    let alone = crate::platform::fit_wide(w, h, wide);
    if all == wide || scale >= alone.0 {
        return Placed {
            scale,
            top,
            screens_x: left + sl * scale,
            pieces: vec![(0, all, left)],
        };
    }
    let (scale, left, top) = alone;
    let mut pieces = vec![(sl, wide, left)];
    if open {
        if sl > 0 {
            pieces.push((0, sl, 0));
        }
        if sr > 0 {
            let x = w.saturating_sub(cover).saturating_sub(sr * scale);
            pieces.push((sl + wide, sr, x));
        }
    }
    Placed {
        scale,
        top,
        screens_x: left,
        pieces,
    }
}

/// [`to_xrgb`] of `frame`, `wide` pixels across, `left` pixels into a
/// black frame `all` across: the screens between the scripts' sidebars.
fn to_xrgb_beside(frame: &[u16], wide: usize, left: usize, all: usize, out: &mut [u8]) {
    if all == wide {
        return to_xrgb(frame, out);
    }
    out.fill(0);
    for (row, line) in frame.chunks(wide).zip(out.chunks_mut(all * 4)) {
        to_xrgb(row, &mut line[left * 4..][..wide * 4]);
    }
}

/// `overlay` over `out`, as [`to_xrgb_beside`] gives it for `wide`
/// pixels of screens: its picture over the first screen, its sidebars
/// beside the screens.
fn draw_overlay(overlay: &Overlay, wide: usize, out: &mut [u8]) {
    let all = overlay.width + wide - WIDTH;
    for (y, row) in overlay.px.chunks(overlay.width).enumerate() {
        let line = &mut out[y * all * 4..][..all * 4];
        for (x, &o) in row.iter().enumerate() {
            if o >> 24 == 0 {
                continue;
            }
            // The right sidebar is past every screen.
            let x = if x < overlay.left + WIDTH {
                x
            } else {
                x + wide - WIDTH
            };
            let px: &mut [u8; 4] = (&mut line[x * 4..][..4]).try_into().unwrap();
            let [_, r, g, b] = u32::from_ne_bytes(*px).to_be_bytes();
            let [r, g, b] = crate::dump::over([r, g, b], o);
            *px = u32::from_be_bytes([0, r, g, b]).to_ne_bytes();
        }
    }
}

/// The menu's pixels a point in `canvas` now.
fn menu_ppp(canvas: &sdl3::render::Canvas<sdl3::video::Window>) -> f32 {
    let (w, h) = canvas
        .output_size()
        .unwrap_or((WIDTH as u32, HEIGHT as u32));
    menu::pixels_per_point(crate::platform::fit(w as usize, h as usize).0)
}

/// The egui key for a key pressed while the console's line has the
/// keyboard (text comes as SDL's text input).
fn egui_key(sc: Scancode) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match sc {
        Scancode::Return | Scancode::KpEnter => K::Enter,
        Scancode::Backspace => K::Backspace,
        Scancode::Delete => K::Delete,
        Scancode::Left => K::ArrowLeft,
        Scancode::Right => K::ArrowRight,
        Scancode::Up => K::ArrowUp,
        Scancode::Down => K::ArrowDown,
        Scancode::Home => K::Home,
        Scancode::End => K::End,
        Scancode::Escape => K::Escape,
        _ => return None,
    })
}

/// What `ev` tells the scripts' `input` (`ScriptingController::event`),
/// added to `out`: keys while `keys` (the game has the keyboard), the
/// mouse over the frame drawn at `at`; all keys let go when the window
/// loses focus.
fn script_event(
    ev: &Event,
    keys: bool,
    density: f32,
    at: (usize, usize, usize),
    pressed: &mut HashMap<Scancode, u32>,
    out: &mut Vec<script::input::Event>,
) {
    use script::input::{Event as E, State};
    match *ev {
        Event::KeyDown {
            scancode: Some(sc),
            keymod,
            repeat,
            ..
        } if keys => {
            let key = *pressed.entry(sc).or_insert_with(|| script_key(sc, keymod));
            if key != 0 {
                let state = if repeat { State::Held } else { State::Down };
                let mods = script_mods(keymod);
                out.push(E::Key { key, state, mods });
            }
        }
        Event::KeyUp {
            scancode: Some(sc),
            keymod,
            ..
        } => {
            if let Some(key) = pressed.remove(&sc).filter(|&k| k != 0) {
                let mods = script_mods(keymod);
                out.push(E::Key {
                    key,
                    state: State::Up,
                    mods,
                });
            }
        }
        Event::MouseMotion { x, y, .. } => {
            let (x, y) = game_point(x, y, density, at);
            out.push(E::MouseMove { x, y });
        }
        Event::MouseButtonDown { mouse_btn, .. } | Event::MouseButtonUp { mouse_btn, .. } => {
            let button = match mouse_btn {
                MouseButton::Left => 0,
                MouseButton::Right => 1,
                MouseButton::Middle => 2,
                MouseButton::X1 => 3,
                MouseButton::X2 => 4,
                MouseButton::Unknown => return,
            };
            let down = matches!(ev, Event::MouseButtonDown { .. });
            out.push(E::MouseButton { button, down });
        }
        // Qt's angle delta: 120 a notch.
        Event::MouseWheel { x, y, .. } => out.push(E::MouseWheel {
            x: (x * 120.0).round() as i32,
            y: (y * 120.0).round() as i32,
        }),
        Event::Window {
            win_event: WindowEvent::FocusLost,
            ..
        } if !pressed.is_empty() => {
            pressed.clear();
            out.push(E::ClearKeys);
        }
        _ => {}
    }
}

/// The code mGBA's Qt window gives a key (`qtToScriptingKey`), for the
/// scripts' `input`: `C.KEY`'s for keys that type nothing (either shift is
/// `SHIFT`, as Qt has it), keypad keys' `KP_` codes, else what the key
/// types with the shift and AltGr held now ([`crate::input::key_code`]: a
/// letter's capital). 0 for a key with none.
fn script_key(sc: Scancode, keymod: Mod) -> u32 {
    use Scancode as S;
    let name = match sc {
        S::Escape => "ESCAPE",
        S::Tab => "TAB",
        S::Backspace => "BACKSPACE",
        S::Return => "ENTER",
        S::Insert => "INSERT",
        S::Delete => "DELETE",
        S::Pause => "BREAK",
        S::PrintScreen => "PRINT_SCREEN",
        S::SysReq => "SYSRQ",
        S::Clear => "CLEAR",
        S::Home => "HOME",
        S::End => "END",
        S::Left => "LEFT",
        S::Up => "UP",
        S::Right => "RIGHT",
        S::Down => "DOWN",
        S::PageUp => "PAGE_UP",
        S::PageDown => "PAGE_DOWN",
        S::LShift | S::RShift => "SHIFT",
        S::LCtrl | S::RCtrl => "CONTROL",
        S::LAlt | S::RAlt => "ALT",
        S::LGui | S::RGui => "SUPER",
        S::CapsLock => "CAPS_LOCK",
        S::NumLockClear => "NUM_LOCK",
        S::ScrollLock => "SCROLL_LOCK",
        S::Application | S::Menu => "MENU",
        S::Help => "HELP",
        S::F1 => "F1",
        S::F2 => "F2",
        S::F3 => "F3",
        S::F4 => "F4",
        S::F5 => "F5",
        S::F6 => "F6",
        S::F7 => "F7",
        S::F8 => "F8",
        S::F9 => "F9",
        S::F10 => "F10",
        S::F11 => "F11",
        S::F12 => "F12",
        S::F13 => "F13",
        S::F14 => "F14",
        S::F15 => "F15",
        S::F16 => "F16",
        S::F17 => "F17",
        S::F18 => "F18",
        S::F19 => "F19",
        S::F20 => "F20",
        S::F21 => "F21",
        S::F22 => "F22",
        S::F23 => "F23",
        S::F24 => "F24",
        S::Kp0 => "KP_0",
        S::Kp1 => "KP_1",
        S::Kp2 => "KP_2",
        S::Kp3 => "KP_3",
        S::Kp4 => "KP_4",
        S::Kp5 => "KP_5",
        S::Kp6 => "KP_6",
        S::Kp7 => "KP_7",
        S::Kp8 => "KP_8",
        S::Kp9 => "KP_9",
        S::KpPlus => "KP_PLUS",
        S::KpMinus => "KP_MINUS",
        S::KpMultiply => "KP_MULTIPLY",
        S::KpDivide => "KP_DIVIDE",
        S::KpComma => "KP_COMMA",
        S::KpPeriod => "KP_POINT",
        S::KpEnter => "KP_ENTER",
        _ => "",
    };
    if let Some(k) = script::input::key(name).filter(|_| !name.is_empty()) {
        return k;
    }
    let typing = keymod & (Mod::LSHIFTMOD | Mod::RSHIFTMOD | Mod::CAPSMOD | Mod::MODEMOD);
    // SAFETY: a plain lookup in SDL's keymap, on the thread that runs SDL.
    let k = unsafe {
        sdl3::sys::keyboard::SDL_GetKeyFromScancode(
            sdl3::sys::scancode::SDL_Scancode(sc as i32),
            sdl3::sys::keycode::SDL_Keymod(typing.bits()),
            false,
        )
    };
    // SDL's codes for keys that type nothing have bit 30 set.
    match char::from_u32(k.0).filter(|_| k.0 & sdl3::sys::keycode::SDLK_SCANCODE_MASK.0 == 0) {
        Some(c) if k.0 != 0 => crate::input::key_code(c),
        _ => 0,
    }
}

/// `C.KMOD`'s modifiers in `keymod`, as Qt's (either shift is both bits).
fn script_mods(keymod: Mod) -> u16 {
    use script::input::kmod;
    [
        (Mod::LSHIFTMOD | Mod::RSHIFTMOD, kmod::SHIFT),
        (Mod::LCTRLMOD | Mod::RCTRLMOD, kmod::CONTROL),
        (Mod::LALTMOD | Mod::RALTMOD, kmod::ALT),
        (Mod::LGUIMOD | Mod::RGUIMOD, kmod::SUPER),
    ]
    .iter()
    .filter(|(m, _)| keymod.intersects(*m))
    .fold(0, |out, &(_, b)| out | b)
}

/// The game-screen pixel under window point (`x`, `y`) (SDL's, in points),
/// the frame drawn at `at` (`scale`, `left`, `top`, in pixels).
fn game_point(x: f32, y: f32, density: f32, at: (usize, usize, usize)) -> (i32, i32) {
    let (scale, left, top) = (at.0.max(1) as f32, at.1 as f32, at.2 as f32);
    (
        ((x * density - left) / scale).floor() as i32,
        ((y * density - top) / scale).floor() as i32,
    )
}

/// The mouse at `x`, `y` in SDL's window points, in egui's.
fn pointer(x: f32, y: f32, density: f32, ppp: f32) -> egui::Pos2 {
    egui::pos2(x * density / ppp, y * density / ppp)
}

/// A virtual gamepad pressing a [`gamepad::Script`]'s controls, through
/// SDL's own gamepad events as a real one does (`--pad-script`).
struct VirtualPad {
    /// Kept attached until dropped.
    _pad: sdl3::joystick::VirtualJoystickConnection,
    id: u32,
    script: gamepad::Script,
    /// The step it's on, when it's due, and the controls to let go then.
    at: usize,
    next: Instant,
    held: Vec<gamepad::Control>,
}

impl VirtualPad {
    fn attach(sdl: &sdl3::Sdl, script: gamepad::Script) -> Result<VirtualPad, String> {
        use sdl3::joystick::{JoystickType, VirtualJoystickDescription};
        let joy = sdl
            .joystick()
            .map_err(|e| format!("the virtual pad: {e}"))?;
        let desc = VirtualJoystickDescription::new()
            .name("gbaa virtual pad")
            .joystick_type(JoystickType::Gamepad)
            .with_buttons(gamepad::BUTTONS_ALL)
            .with_axes(gamepad::AXES_ALL);
        let pad = joy
            .attach_virtual_joystick(desc)
            .map_err(|e| format!("the virtual pad: {e}"))?;
        eprintln!("gbaa-desktop: virtual pad attached");
        Ok(VirtualPad {
            id: pad.id().into(),
            _pad: pad,
            script,
            at: 0,
            next: Instant::now(),
            held: Vec::new(),
        })
    }

    /// Press or let go what's due, on the pad once it's opened.
    fn step(&mut self, open: &HashMap<u32, Gamepad>) {
        let Some(g) = open.get(&self.id) else {
            // Not opened yet: the script waits for it.
            self.next = Instant::now() + Duration::from_millis(10);
            return;
        };
        let joy = unsafe { sdl3::sys::gamepad::SDL_GetGamepadJoystick(g.raw()) };
        let set = |c: gamepad::Control, down: bool| unsafe {
            use sdl3::sys::joystick::{SDL_SetJoystickVirtualAxis, SDL_SetJoystickVirtualButton};
            match c {
                gamepad::Control::Button(b) => {
                    SDL_SetJoystickVirtualButton(joy, b.to_ll().0, down);
                }
                gamepad::Control::Label(l) => {
                    let b = gamepad::XBOX_LABELS[l as usize];
                    SDL_SetJoystickVirtualButton(joy, b.to_ll().0, down);
                }
                gamepad::Control::Stick(a, up) => {
                    let v = match (down, up) {
                        (false, _) => 0,
                        (true, true) => i16::MAX,
                        (true, false) => -i16::MAX,
                    };
                    SDL_SetJoystickVirtualAxis(joy, a.to_ll().0, v);
                }
                // Scripts name a pad's controls only.
                gamepad::Control::Key(_) => {}
            }
        };
        while Instant::now() >= self.next {
            if !self.held.is_empty() {
                for c in std::mem::take(&mut self.held) {
                    set(c, false);
                }
                self.next = Instant::now() + gamepad::Script::GAP;
                continue;
            }
            let Some(step) = self.script.0.get(self.at) else {
                // Done: nothing more is due.
                self.next = Instant::now() + Duration::from_secs(3600);
                return;
            };
            self.at += 1;
            match step {
                gamepad::Step::Wait(d) => self.next = Instant::now() + *d,
                gamepad::Step::Press(cs) => {
                    for &c in cs {
                        set(c, true);
                    }
                    self.held = cs.clone();
                    self.next = Instant::now() + gamepad::Script::HOLD;
                }
            }
        }
    }
}

#[cfg(test)]
mod drawer_tests {
    use super::*;

    #[test]
    fn a_window_filling_its_display_is_fullscreen() {
        // gamescope's headless 1280x800, the window fitted to it.
        assert!(fills((1280, 800), (1280, 800), false));
        assert!(fills((1600, 800), (1280, 800), false));
        assert!(!fills((1200, 800), (1280, 800), false));
        assert!(!fills((720, 480), (1280, 800), false));
        assert!(!fills((0, 0), (0, 0), false));
        // The Deck's desktop: a 5x window (1200x800) widened for the
        // console, clamped to the screen. Still a window, so hiding the
        // console (or dropping the sidebars) shrinks it back.
        assert!(!fills((1280, 800), (1280, 800), true));
    }

    #[test]
    fn sidebars_are_a_drawer_when_they_would_shrink_the_game() {
        // A window that fits the 80-pixel panel at 3x: beside the game.
        let p = place(960, 480, 0, (0, WIDTH, 80), false);
        assert_eq!((p.scale, p.screens_x), (3, 0));
        assert_eq!(p.pieces, vec![(0, WIDTH + 80, 0)]);
        // The Deck: 320 across would be 4x, the game alone is 5x.
        let shut = place(1280, 800, 0, (0, WIDTH, 80), false);
        assert_eq!((shut.scale, shut.screens_x, shut.top), (5, 40, 0));
        assert_eq!(shut.pieces, vec![(0, WIDTH, 40)]);
        let open = place(1280, 800, 0, (0, WIDTH, 80), true);
        assert_eq!(open.pieces, vec![(0, WIDTH, 40), (WIDTH, 80, 1280 - 400)]);
        // Both sides, the console's panel over the frame's right.
        let open = place(1280, 800, 360, (40, WIDTH, 80), true);
        assert_eq!(
            open.pieces,
            vec![
                (40, WIDTH, 40),
                (0, 40, 0),
                (40 + WIDTH, 80, 1280 - 360 - 400)
            ]
        );
    }
}
