//! The window's menu, in egui (docs/decisions.md; drawn through SDL's
//! renderer by [`crate::egui_sdl`]): pause the game, set fast-forward's
//! speed, set the volume (saved to [`crate::settings`]) or mute it, undo the last state load, remap the pad (for this game or every
//! game, [`crate::remap`]), switch fullscreen, load and unload Lua scripts
//! and show their console ([`crate::console`], drawn in the same egui
//! pass), host or join a netplay room (its Netplay page), close it, or
//! quit. In netplay it reports how the session has gone ([`Menu::report`])
//! at the screen's foot. It opens on a pad with Start and Select held
//! together, or F10 on the keyboard.
//!
//! With it shut, the volume keys ([`SoundKey`]) step the volume and mute
//! it as its rows do, and a toast shows the level for [`TOAST_TIME`].
//! In a netplay room a small mark in the top right corner ([`Mark`]) says
//! how the room is: connected, waiting for players, or lost, and how many
//! players are in it.
//!
//! A pad drives it through [`MenuPad`], which turns its controls into
//! [`Nav`]: the D-pad and left stick become egui's arrow keys and the
//! button labelled A its Enter, so egui's own focus moves through the menu.
//! B (or Start) closes the menu itself, before egui sees anything: egui's
//! Escape only clears the focus (docs/research/ui-library.md, the egui
//! spike, has these lessons). Each row is one focus stop, its value
//! changed with left and right in place.
//!
//! The Controls page ([`controls`]) shows what presses each GBA button and
//! shortcut, on the pads or the keyboard, for every game or this one, and
//! adds and removes controls one at a time; it keeps its own focus.
//!
//! The Save folders page ([`saves`]) sets where this game's saves go, and
//! every game's, with a folder picker a pad drives.
//!
//! The Netplay page ([`crate::room`]) hosts a room (its code shown big, to
//! read out) or joins one by its code, through the server it names (fly's
//! unless another is set, and then remembered in [`crate::settings`]), and
//! leaves it. A field is typed on the keyboard, or on a pad: up and down
//! step the letter at the cursor, left and right move it ([`Edit`]); A or
//! Enter keeps it, B or Escape drops it. In a session it shows the room,
//! the seats, the round trip, how far ahead or behind, the rollbacks and
//! the frames run again.

use crate::audio::MAX_VOLUME;
use crate::console::{self, Console, ScriptCmd};
use crate::gamepad::{self, Control, Label, PadEvent, XBOX_LABELS};
use crate::remap::Controls;

pub mod controls;
pub mod saves;
use crate::room;
use sdl3::gamepad::{Axis, Button};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Fast-forward's speeds, in the order the menu steps through them: a
/// multiple of normal speed, 0 for unthrottled.
pub const SPEEDS: [f64; 8] = [1.5, 2.0, 2.5, 3.0, 4.0, 8.0, 16.0, 0.0];

/// The Volume row's step, in percent.
pub const VOLUME_STEP: u32 = 10;

/// How long the volume keys' toast stays up.
pub const TOAST_TIME: Duration = Duration::from_secs(1);

/// How long the toast saying the other netplay player left, or that the
/// players' games went out of sync, stays up.
pub const LEFT_TOAST_TIME: Duration = Duration::from_secs(8);

/// How a netplay room is, as the mark over the game shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Asking the server for a room, or to join one.
    Connecting,
    /// In the room, waiting for the other player.
    Waiting,
    /// Playing, every player answering.
    Connected,
    /// Playing, but the other player has sent nothing for a while.
    Quiet,
    /// Playing, but the peers' consoles have gone apart (a desync): the
    /// players' games may differ.
    Desynced,
    /// The other player has left: this one plays on alone.
    Lost,
}

impl Mark {
    /// The mark for netplay at `state`, if it's in a room.
    pub fn of(state: &room::State) -> Option<Mark> {
        Some(match state {
            room::State::Off | room::State::Ended(_) => return None,
            room::State::Making | room::State::Joining(_) => Mark::Connecting,
            room::State::Waiting { .. } => Mark::Waiting,
            room::State::Playing { left: Some(_), .. } => Mark::Lost,
            room::State::Playing {
                desync: Some(_), ..
            } => Mark::Desynced,
            room::State::Playing { quiet: true, .. } => Mark::Quiet,
            room::State::Playing { .. } => Mark::Connected,
        })
    }

    /// What it says: how it is, and the players in the room at `state`
    /// (the one it's the mark of) of its seats.
    pub fn text(self, state: &room::State) -> String {
        let (filled, seats) = match state {
            room::State::Waiting { filled, seats, .. } => (*filled, *seats),
            room::State::Playing { seats, .. } => (*seats, *seats),
            _ => (1, room::SEATS),
        };
        let (said, n) = match self {
            Mark::Connecting => return "Connecting…".into(),
            Mark::Waiting => ("Waiting for players", filled),
            Mark::Connected => ("Connected", seats),
            Mark::Quiet => ("Not hearing from them", seats),
            Mark::Desynced => ("Out of sync", seats),
            Mark::Lost => ("Lost: playing alone", 1),
        };
        format!("{said} · {n}/{seats}")
    }

    /// Its dot's colour: green connected, amber waiting, red lost or out
    /// of sync.
    pub fn colour(self) -> egui::Color32 {
        match self {
            Mark::Connected => egui::Color32::from_rgb(80, 200, 100),
            Mark::Connecting | Mark::Waiting | Mark::Quiet => egui::Color32::from_rgb(230, 170, 50),
            Mark::Lost | Mark::Desynced => egui::Color32::from_rgb(220, 70, 60),
        }
    }
}

/// A volume key, pressed with the menu shut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKey {
    Down,
    Up,
    Mute,
}

/// A move through the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    /// Open the menu, or close it.
    Toggle,
    Up,
    Down,
    Left,
    Right,
    /// Act on the focused row.
    Ok,
    /// Close the menu (or go back a page, or stop listening).
    Back,
    /// Backspace, in a field being typed in; Backspace or Delete on the
    /// Controls page, removing the focused control.
    Erase,
    /// A control pressed while the remap screen listens.
    Bind(Control),
    /// Undo the Controls page's last change: Y, Ctrl+Z.
    Undo,
}

/// How far a stick goes to move the menu (or further, past a wider
/// deadzone): a deliberate push, not a brush.
const STICK_PUSH: f32 = 0.5;

/// How long after a stick comes back to the middle a push the other way
/// is its spring overshooting, not a move.
const FLICK_TIME: Duration = Duration::from_millis(150);

/// What one pad holds, as the menu reads it.
#[derive(Default)]
struct Held {
    buttons: Vec<Button>,
    /// Each stick's and trigger's direction, -1, 0 or 1: pushed past
    /// [`STICK_PUSH`], it's that way until it's back in the deadzone.
    axes: HashMap<Axis, i8>,
    /// The left stick, as it last read.
    left: (f32, f32),
    /// The way the left stick moved the menu, until it's back in the
    /// deadzone.
    stick: Option<Nav>,
    /// The way it last moved it, and when it came back.
    let_go: Option<(Nav, Instant)>,
    labels: Option<[Button; 4]>,
}

/// Every pad's controls, as menu moves.
#[derive(Default)]
pub struct MenuPad {
    held: HashMap<u32, Held>,
    /// The remap screen is listening ([`Menu::listening`]): the next
    /// control pressed is a [`Nav::Bind`].
    pub listening: bool,
    /// Inside its deadzone, a stick is in the middle, a trigger let go.
    pub deadzones: gamepad::Deadzones,
}

/// The opposite way.
fn opposite(n: Nav) -> Nav {
    match n {
        Nav::Up => Nav::Down,
        Nav::Down => Nav::Up,
        Nav::Left => Nav::Right,
        Nav::Right => Nav::Left,
        n => n,
    }
}

impl MenuPad {
    /// Whether any pad holds direction `n`: on the D-pad, or the left
    /// stick that moved it there.
    pub fn holds(&self, n: Nav) -> bool {
        let button = match n {
            Nav::Up => Button::DPadUp,
            Nav::Down => Button::DPadDown,
            Nav::Left => Button::DPadLeft,
            Nav::Right => Button::DPadRight,
            _ => return false,
        };
        self.held
            .values()
            .any(|h| h.buttons.contains(&button) || h.stick == Some(n))
    }

    /// Pad `id` did `e` at `now`; `open` is whether the menu is. Returns
    /// the move it makes, if any: with the menu shut only Start and Select
    /// together (opening it), as the second of the two goes down.
    pub fn feed(&mut self, id: u32, e: PadEvent, open: bool, now: Instant) -> Option<Nav> {
        let held = self.held.entry(id).or_default();
        match e {
            PadEvent::Pressed(b) => {
                if !held.buttons.contains(&b) {
                    held.buttons.push(b);
                }
                let chord = [Button::Start, Button::Back];
                if chord.contains(&b) && chord.iter().all(|c| held.buttons.contains(c)) {
                    return Some(Nav::Toggle);
                }
                if !open {
                    return None;
                }
                let labels = held.labels.unwrap_or(XBOX_LABELS);
                if self.listening {
                    let label = [Label::A, Label::B, Label::X, Label::Y];
                    return Some(Nav::Bind(match labels.iter().position(|&l| l == b) {
                        Some(i) => Control::Label(label[i]),
                        None => Control::Button(b),
                    }));
                }
                match b {
                    Button::DPadUp => Some(Nav::Up),
                    Button::DPadDown => Some(Nav::Down),
                    Button::DPadLeft => Some(Nav::Left),
                    Button::DPadRight => Some(Nav::Right),
                    // Start closes it too, unless it's half of the chord
                    // (Select is already down).
                    Button::Start if !held.buttons.contains(&Button::Back) => Some(Nav::Back),
                    b if b == labels[0] => Some(Nav::Ok),
                    b if b == labels[1] => Some(Nav::Back),
                    b if b == labels[3] => Some(Nav::Undo),
                    _ => None,
                }
            }
            PadEvent::Released(b) => {
                held.buttons.retain(|&h| h != b);
                None
            }
            PadEvent::Axis(a, v) => {
                let dz = self.deadzones.of(a);
                let push = STICK_PUSH.max(dz);
                let was = held.axes.get(&a).copied().unwrap_or(0);
                let dir = if v.abs() > push {
                    v.signum() as i8
                } else if v.abs() <= dz {
                    0
                } else {
                    was
                };
                held.axes.insert(a, dir);
                let moved = match a {
                    Axis::LeftX => {
                        held.left.0 = v;
                        held.stick_moved(dz, push, now)
                    }
                    Axis::LeftY => {
                        held.left.1 = v;
                        held.stick_moved(dz, push, now)
                    }
                    _ => None,
                };
                if !open {
                    return None;
                }
                if self.listening {
                    return (dir != 0 && dir != was)
                        .then_some(Nav::Bind(Control::Stick(a, dir > 0)));
                }
                moved
            }
            PadEvent::Labels(l) => {
                held.labels = Some(l);
                None
            }
            PadEvent::Gone => {
                self.held.remove(&id);
                None
            }
        }
    }
}

impl Held {
    /// The left stick read `self.left`: the way it moves the menu, if
    /// it's newly pushed. The way it's pushed furthest counts, past
    /// `push`; it moves again once it's back inside `dz`, or turned a
    /// corner, but not straight back the other way (its spring
    /// overshooting, as it's let go).
    fn stick_moved(&mut self, dz: f32, push: f32, now: Instant) -> Option<Nav> {
        let (x, y) = self.left;
        if x.abs() <= dz && y.abs() <= dz {
            if let Some(n) = self.stick.take() {
                self.let_go = Some((n, now));
            }
            return None;
        }
        let (v, n) = if x.abs() >= y.abs() {
            (x, if x < 0.0 { Nav::Left } else { Nav::Right })
        } else {
            (y, if y < 0.0 { Nav::Up } else { Nav::Down })
        };
        if v.abs() <= push || self.stick == Some(n) {
            return None;
        }
        let before = self.stick.replace(n);
        let flick = before == Some(opposite(n))
            || self
                .let_go
                .is_some_and(|(m, t)| m == opposite(n) && now.duration_since(t) < FLICK_TIME);
        (!flick).then_some(n)
    }
}

/// Where a move came from: the keyboard, or a pad's buttons or sticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Keys,
    Buttons(u32),
    Stick(u32),
}

/// How close together the same move from two places is one press said
/// twice: Steam Input sends a Steam Controller's D-pad as the pad's and
/// as arrow keys, its A as Enter.
const ECHO_TIME: Duration = Duration::from_millis(80);

/// How long a direction is held before it moves again, and then how
/// often it does.
const REPEAT_DELAY: Duration = Duration::from_millis(500);
const REPEAT_EVERY: Duration = Duration::from_millis(100);

/// A direction held moves again and again: the D-pad's, a stick's, an
/// arrow key's alike (the keyboard's own repeat is left out, so it's the
/// same pace).
#[derive(Default)]
pub struct Repeat {
    /// The direction last pressed, and when it next moves.
    held: Option<(Nav, Instant)>,
}

impl Repeat {
    /// `n` was taken at `now`: a direction starts repeating; anything
    /// else stops it.
    pub fn pressed(&mut self, n: Nav, now: Instant) {
        let way = matches!(n, Nav::Up | Nav::Down | Nav::Left | Nav::Right);
        self.held = way.then_some((n, now + REPEAT_DELAY));
    }

    /// The move due at `now`, if its direction's still held (`holds`
    /// says what is).
    pub fn due(&mut self, now: Instant, holds: impl Fn(Nav) -> bool) -> Option<Nav> {
        let (n, at) = self.held?;
        if !holds(n) {
            self.held = None;
            return None;
        }
        if now < at {
            return None;
        }
        self.held = Some((n, now + REPEAT_EVERY));
        Some(n)
    }

    /// When it next moves, to wake for.
    pub fn next(&self) -> Option<Instant> {
        self.held.map(|(_, at)| at)
    }
}

/// Drops a move that's an echo of the last one, from another place: the
/// same move, or any move just after a control was bound (the press that
/// bound it, said again once it's stopped listening, would act on the
/// page: A pressing "+ Add" again, B leaving).
#[derive(Default)]
pub struct Echoes {
    last: Option<(Nav, Source, Instant)>,
}

impl Echoes {
    /// Whether to take `n`, from `from` at `now`.
    pub fn keep(&mut self, n: Nav, from: Source, now: Instant) -> bool {
        let echo = !matches!(n, Nav::Bind(_))
            && self.last.is_some_and(|(m, f, t)| {
                (m == n || matches!(m, Nav::Bind(_)))
                    && f != from
                    && now.duration_since(t) < ECHO_TIME
            });
        if !echo {
            self.last = Some((n, from, now));
        }
        !echo
    }
}

/// A screen of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Main,
    Remap,
    Scripts,
    Netplay,
    Saves,
    Folder,
}

/// The main page's Controls row, from 0 (Undo load state counted, greyed
/// or not).
const CONTROLS_ROW: usize = 5;
/// Its Scripts row, Fullscreen's next.
const SCRIPTS_ROW: usize = 7;
/// Its Netplay row.
const NETPLAY_ROW: usize = 8;
/// Its Save folders row.
const SAVES_ROW: usize = 10;

/// A field of the Netplay page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Server,
    Code,
}

impl Field {
    /// What a pad steps through, and what typing takes: a space is
    /// nothing, dropped when the field is kept.
    fn alphabet(self) -> &'static [u8] {
        match self {
            Field::Server => b" 0123456789.:abcdefghijklmnopqrstuvwxyz-",
            Field::Code => relay::CODE_ALPHABET,
        }
    }

    fn max(self) -> usize {
        match self {
            Field::Server => 64,
            Field::Code => relay::CODE_LEN,
        }
    }
}

/// A field being typed in: its text and where the cursor is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    field: Field,
    text: Vec<u8>,
    cursor: usize,
}

impl Edit {
    fn new(field: Field, text: &str) -> Edit {
        let text: Vec<u8> = text.bytes().take(field.max()).collect();
        Edit {
            field,
            cursor: text.len(),
            text,
        }
    }

    /// Step the letter at the cursor through the alphabet (a new one, at
    /// the end).
    fn step(&mut self, step: i32) {
        let a = self.field.alphabet();
        if self.cursor == self.text.len() {
            if self.text.len() < self.field.max() {
                self.text.push(a[if step > 0 { 0 } else { a.len() - 1 }]);
            }
            return;
        }
        let c = &mut self.text[self.cursor];
        let i = a.iter().position(|x| x == c).unwrap_or(0) as i32;
        *c = a[(i + step).rem_euclid(a.len() as i32) as usize];
    }

    /// Move the cursor, no further than past the last letter.
    fn go(&mut self, step: i32) {
        let end = self.text.len() as i32;
        self.cursor = (self.cursor as i32 + step).clamp(0, end) as usize;
    }

    /// Type `t` at the cursor: what the alphabet hasn't is left out, a
    /// code's letters in capitals. A full field's letters are typed over.
    fn typed(&mut self, t: &str) {
        for c in t.bytes() {
            let c = match self.field {
                Field::Code => c.to_ascii_uppercase(),
                Field::Server => c.to_ascii_lowercase(),
            };
            if !self.field.alphabet().contains(&c) {
                continue;
            }
            if self.cursor < self.text.len() && self.text.len() == self.field.max() {
                self.text[self.cursor] = c;
            } else if self.text.len() < self.field.max() {
                self.text.insert(self.cursor, c);
            } else {
                continue;
            }
            self.cursor += 1;
        }
    }

    /// Backspace.
    fn erase(&mut self) {
        if self.cursor > 0 {
            self.text.remove(self.cursor - 1);
            self.cursor -= 1;
        }
    }

    /// The text kept: spaces dropped.
    fn kept(&self) -> String {
        self.text
            .iter()
            .filter(|&&c| c != b' ')
            .map(|&c| c as char)
            .collect()
    }

    /// As the row shows it: the letter at the cursor in brackets (none
    /// past a full field's last), a code's letters still to come as
    /// underscores.
    fn shown(&self) -> String {
        let n = match self.field {
            Field::Code => self.field.max(),
            Field::Server => (self.text.len() + 1).min(self.field.max()),
        };
        (0..n)
            .map(|i| {
                let c = self.text.get(i).map_or('_', |&c| c as char);
                let c = if c == ' ' { '·' } else { c };
                if i == self.cursor {
                    format!("[{c}]")
                } else {
                    c.to_string()
                }
            })
            .collect()
    }
}

/// The menu's state, and its egui context.
pub struct Menu {
    pub ctx: egui::Context,
    pub open: bool,
    page: Page,
    /// The page the last pass drew, if it's still open.
    drawn: Option<Page>,
    /// The row to focus at the next pass (just opened, or a page turned).
    focus: Option<usize>,
    /// The focused row as the last pass left it, scrolled into view.
    shown: Option<egui::Id>,
    /// The game is paused (while the menu is open, but never in a netplay
    /// session: the others play on, and this seat's pad reads as let go).
    pub paused: bool,
    /// Fast-forward's speed, as [`SPEEDS`] counts it.
    pub speed: f64,
    /// The speed changed since the game thread last looked.
    pub speed_changed: bool,
    /// The master volume, in percent ([`crate::audio::volume_gain`]).
    pub volume: u32,
    /// The sound is muted: played at no gain, the device still pacing.
    pub muted: bool,
    /// The volume or mute changed since the game thread last looked.
    pub sound_changed: bool,
    /// Where a volume set here is saved: the settings file, `None` to keep
    /// it to this run.
    pub settings: Option<PathBuf>,
    /// There's a state load to undo: its row can be chosen, else it's
    /// greyed and skipped.
    pub undoable: bool,
    /// Undo the last state load (chosen since the window last looked).
    pub undo: bool,
    /// A netplay session's end lines so far, shown under the main page.
    pub report: Vec<String>,
    /// Where netplay is, as the game thread last said.
    pub netplay: room::State,
    /// A session asked for on the Netplay page, for the window to pass on.
    pub netplay_ask: Option<room::Ask>,
    /// Leave the room (asked for since the window last looked).
    pub leave: bool,
    /// The server the Netplay page uses.
    pub server: String,
    /// The room code typed to join.
    code: String,
    /// The seats in a room the Netplay page hosts (2-4).
    players: usize,
    /// The Netplay page's field being typed in.
    edit: Option<Edit>,
    pub quit: bool,
    /// The pads' mapping, as the remap screen edits it.
    pub controls: Controls,
    /// The Controls page's state.
    pub remap: controls::Page,
    /// The mapping changed since the window last looked.
    pub mapping_changed: bool,
    /// The sticks' and the triggers' deadzones, in percent.
    pub deadzones: [u32; 2],
    /// The sticks' left and right take a deadzone apart from their up
    /// and down: [`Menu::axis_deadzones`], not the sticks' one.
    pub deadzone_axes_apart: bool,
    /// The sticks' left and right deadzone, and up and down, in percent,
    /// as saved; one never set is the sticks' one.
    pub axis_deadzones: [Option<u32>; 2],
    /// They changed since the window last looked.
    pub deadzones_changed: bool,
    /// The window is fullscreen, as the window last said; the menu's row
    /// asks for the other, once ([`Menu::fullscreen_asked`]).
    pub fullscreen: bool,
    pub fullscreen_asked: bool,
    /// Resume on launch ([`crate::resume`]): the game thread reads it, and
    /// it's saved to the settings file as it changes.
    pub resume: bool,
    /// What the last save did.
    saved: Option<String>,
    /// The Save folders page's settings ([`saves`]).
    pub saves: saves::SaveFolders,
    /// Its folder picker, while open.
    picker: Option<saves::Picker>,
    /// What setting a folder did, for the page turned back to.
    folder_said: Option<String>,
    /// The script console, drawn under the menu.
    pub console: Console,
    /// The scripts the Scripts page offers, found as it opened.
    files: Vec<PathBuf>,
    /// Keys for egui's next pass.
    events: Vec<egui::Event>,
    /// The volume keys' toast, and when it goes.
    toast: Option<(String, Instant)>,
}

impl Menu {
    /// Shut, fast-forward at `speed`, the sound at `volume` percent, the
    /// pads mapped as `controls` says.
    pub fn new(speed: f64, volume: u32, controls: Controls) -> Menu {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Dark);
        style(&ctx);
        Menu {
            ctx,
            open: false,
            page: Page::Main,
            drawn: None,
            focus: None,
            shown: None,
            paused: false,
            speed,
            speed_changed: false,
            volume,
            muted: false,
            sound_changed: false,
            settings: None,
            undoable: false,
            undo: false,
            report: Vec::new(),
            netplay: room::State::Off,
            netplay_ask: None,
            leave: false,
            server: room::DEFAULT_SERVER.into(),
            code: String::new(),
            players: room::SEATS,
            edit: None,
            quit: false,
            controls,
            remap: controls::Page::default(),
            mapping_changed: false,
            deadzones: [gamepad::DEFAULT_DEADZONE, gamepad::DEFAULT_TRIGGER_DEADZONE],
            deadzone_axes_apart: false,
            axis_deadzones: [None, None],
            deadzones_changed: false,
            fullscreen: false,
            fullscreen_asked: false,
            resume: false,
            saved: None,
            saves: saves::SaveFolders::default(),
            picker: None,
            folder_said: None,
            console: Console::default(),
            files: Vec::new(),
            events: Vec::new(),
            toast: None,
        }
    }

    /// Open it (pausing the game, unless in a netplay session) or shut it
    /// (going on).
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        self.page = Page::Main;
        self.drawn = None;
        self.focus = open.then_some(0);
        self.remap.reset_state();
        self.edit = None;
        self.saved = None;
        self.paused = open && !self.in_session();
        self.events.clear();
    }

    /// Whether the remap screen is waiting for a control to bind.
    pub fn listening(&self) -> bool {
        self.open && self.page == Page::Remap && self.remap.listen.is_some()
    }

    /// Whether it's waiting for a key: the next one pressed is a
    /// [`Nav::Bind`].
    pub fn listening_for_keys(&self) -> bool {
        self.listening() && self.remap.keyboard
    }

    /// Whether a field of the Netplay page is being typed in: keys and
    /// text go to it.
    pub fn editing(&self) -> bool {
        self.open && self.edit.is_some()
    }

    /// Type `text` into the field being typed in.
    pub fn edit_text(&mut self, text: &str) {
        if let Some(e) = &mut self.edit {
            e.typed(text);
        }
    }

    /// Whether a netplay session is being played: the menu doesn't pause
    /// it, as the other players would wait.
    pub fn in_session(&self) -> bool {
        matches!(self.netplay, room::State::Playing { .. })
    }

    /// Where netplay is now. A session begun closes the menu, so the game
    /// goes on; the other player leaving, or the games going out of sync,
    /// says so over the game.
    pub fn set_netplay(&mut self, state: room::State) {
        let playing = |s: &room::State| matches!(s, room::State::Playing { .. });
        let began = playing(&state) && !playing(&self.netplay);
        let left = |s: &room::State| match s {
            room::State::Playing { left, .. } => left.clone(),
            _ => None,
        };
        if let Some(l) = left(&state)
            && left(&self.netplay).as_ref() != Some(&l)
        {
            self.toast = Some((l, Instant::now() + LEFT_TOAST_TIME));
        }
        let desync = |s: &room::State| match s {
            room::State::Playing { desync, .. } => *desync,
            _ => None,
        };
        if let Some(f) = desync(&state)
            && desync(&self.netplay).is_none()
        {
            self.toast = Some((room::desync_note(f), Instant::now() + LEFT_TOAST_TIME));
        }
        // The page's rows change with it: the focus goes to the first.
        if std::mem::discriminant(&state) != std::mem::discriminant(&self.netplay)
            && self.page == Page::Netplay
        {
            self.focus = Some(0);
        }
        self.netplay = state;
        if began && self.open {
            self.set_open(false);
        }
    }

    /// Keep the field being typed in: the server saved, the code kept to
    /// join with.
    fn keep_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let text = e.kept();
        match e.field {
            Field::Code => self.code = text,
            Field::Server => {
                self.server = if text.is_empty() {
                    room::DEFAULT_SERVER.into()
                } else {
                    text
                };
                self.saved = self.settings.as_ref().map(|path| {
                    match crate::settings::save(path, "server", &self.server) {
                        Ok(()) => format!("saved to {}", path.display()),
                        Err(e) => format!("not saved: {e}"),
                    }
                });
            }
        }
    }

    /// Turn to `page`, focusing its row `focus`.
    fn turn(&mut self, page: Page, focus: usize) {
        if page == Page::Scripts {
            let config = gamepad::config_dir();
            let dirs = console::script_dirs(config.as_deref(), &self.console.view.loaded);
            self.files = console::scripts_in(&dirs);
        }
        self.page = page;
        self.focus = Some(focus);
        self.remap.reset_state();
        self.edit = None;
        self.saved = None;
    }

    /// Take a move. Arrows and Ok go to egui as keys pressed and let go.
    pub fn nav(&mut self, n: Nav) {
        if let Some(e) = &mut self.edit {
            match n {
                Nav::Up => e.step(1),
                Nav::Down => e.step(-1),
                Nav::Left => e.go(-1),
                Nav::Right => e.go(1),
                Nav::Erase => e.erase(),
                Nav::Ok => self.keep_edit(),
                Nav::Back => self.edit = None,
                Nav::Toggle => self.set_open(!self.open),
                Nav::Bind(_) | Nav::Undo => {}
            }
            return;
        }
        let key = match n {
            Nav::Toggle => return self.set_open(!self.open),
            Nav::Back if self.listening() => {
                self.remap.listen = None;
                return;
            }
            Nav::Back if self.page == Page::Remap => return self.turn(Page::Main, CONTROLS_ROW),
            Nav::Back if self.page == Page::Scripts => return self.turn(Page::Main, SCRIPTS_ROW),
            Nav::Back if self.page == Page::Netplay => return self.turn(Page::Main, NETPLAY_ROW),
            Nav::Back if self.page == Page::Saves => return self.turn(Page::Main, SAVES_ROW),
            Nav::Back if self.page == Page::Folder => {
                let row = match self.picker.as_ref().map(|p| p.target) {
                    Some(saves::Target::All) => 1,
                    _ => 0,
                };
                return self.turn(Page::Saves, row);
            }
            Nav::Back => return self.set_open(false),
            _ if !self.open => return,
            _ if self.page == Page::Remap => return self.controls_nav(n),
            Nav::Bind(_) | Nav::Undo | Nav::Erase => return,
            Nav::Up => egui::Key::ArrowUp,
            Nav::Down => egui::Key::ArrowDown,
            Nav::Left => egui::Key::ArrowLeft,
            Nav::Right => egui::Key::ArrowRight,
            Nav::Ok => egui::Key::Enter,
        };
        for pressed in [true, false] {
            self.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }

    /// The mapping was edited: save it, and have the pads read it.
    fn changed(&mut self) {
        self.mapping_changed = true;
        self.saved = match self.controls.save() {
            Ok(Some(path)) => Some(format!("saved to {}", path.display())),
            Ok(None) => None,
            Err(e) => Some(format!("not saved: {e}")),
        };
    }

    /// Set the volume and save it, unless it's already that.
    fn set_volume(&mut self, volume: u32) {
        if volume == self.volume {
            return;
        }
        self.volume = volume;
        self.sound_changed = true;
        self.saved = self.settings.as_ref().map(|path| {
            match crate::settings::save(path, "volume", &volume.to_string()) {
                Ok(()) => format!("saved to {}", path.display()),
                Err(e) => format!("not saved: {e}"),
            }
        });
    }

    /// Turn resume on launch on or off, and save it.
    fn set_resume(&mut self, on: bool) {
        self.resume = on;
        let value = if on { "on" } else { "off" };
        self.saved =
            self.settings
                .as_ref()
                .map(|path| match crate::settings::save(path, "resume", value) {
                    Ok(()) => format!("saved to {}", path.display()),
                    Err(e) => format!("not saved: {e}"),
                });
    }

    /// A volume key pressed at `now`: step the volume (saved, and
    /// unmuting) or mute it, as the rows do, and show the level.
    pub fn sound_key(&mut self, k: SoundKey, now: Instant) {
        match k {
            SoundKey::Mute => {
                self.muted = !self.muted;
                self.sound_changed = true;
            }
            SoundKey::Down | SoundKey::Up => {
                let step = if k == SoundKey::Up { 1 } else { -1 };
                self.sound_changed |= std::mem::take(&mut self.muted);
                self.set_volume(step_volume(self.volume, step));
            }
        }
        let text = if self.muted {
            "Muted".to_string()
        } else {
            format!("Volume {}%", self.volume)
        };
        self.toast = Some((text, now + TOAST_TIME));
    }

    /// When the toast goes, if one is up at `now`.
    pub fn toast_until(&self, now: Instant) -> Option<Instant> {
        let page = self.open.then(|| self.remap.toast_until()).flatten();
        let main = self.toast.as_ref().map(|t| t.1);
        main.into_iter().chain(page).filter(|&t| t > now).min()
    }

    /// Pass a pointer event (moved, clicked or scrolled) to egui.
    pub fn pointer(&mut self, e: egui::Event) {
        if self.open || self.console.shown {
            self.events.push(e);
        }
    }

    /// Pass a key or text typed to egui: the console's line has the
    /// keyboard.
    pub fn typed(&mut self, e: egui::Event) {
        if self.console.typing && !self.open {
            self.events.push(e);
        }
    }

    /// Whether egui has anything to draw: the menu, the console, the
    /// toast or the netplay mark.
    pub fn drawn(&self) -> bool {
        self.open
            || self.console.shown
            || self.toast_until(Instant::now()).is_some()
            || Mark::of(&self.netplay).is_some()
    }

    /// Run one egui pass over a screen of `size` pixels at `ppp` pixels a
    /// point, at `time` seconds.
    pub fn run(&mut self, size: (u32, u32), ppp: f32, time: f64) -> egui::FullOutput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size.0 as f32 / ppp, size.1 as f32 / ppp),
            )),
            time: Some(time),
            focused: true,
            events: std::mem::take(&mut self.events),
            ..egui::RawInput::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        let ctx = self.ctx.clone();
        // egui places the window by the size it had last pass: on a page's
        // first pass that's the old page's (or none), and it'd be drawn
        // off-centre until something redraws it. So a page's first pass is
        // dry, and drawn over by a real one. (egui's own `request_discard`
        // would rerun it without its keys.)
        if self.open && self.drawn != Some(self.page) {
            self.drawn = Some(self.page);
            let dry = egui::RawInput {
                events: Vec::new(),
                ..input.clone()
            };
            let mut out = ctx.run_ui(dry, |ui| self.ui(ui, true));
            out.append(ctx.run_ui(input, |ui| self.ui(ui, false)));
            return out;
        }
        ctx.run_ui(input, |ui| self.ui(ui, false))
    }

    /// A `dry` pass only lays it out: it has no keys and leaves the focus
    /// to ask for to the next.
    fn ui(&mut self, ui: &mut egui::Ui, dry: bool) {
        let screen = ui.ctx().content_rect();
        if self.console.shown {
            self.console.ui(ui, console::panel_rect(screen));
        } else {
            self.console.typing = false;
        }
        if !self.open {
            if self.toast_until(Instant::now()).is_none() {
                self.toast = None;
            }
            if let Some((text, _)) = &self.toast {
                egui::Area::new(egui::Id::unique("toast"))
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 16.0])
                    .interactable(false)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(text.as_str()));
                    });
            }
            if let Some(mark) = Mark::of(&self.netplay) {
                draw_mark(ui.ctx(), mark, &self.netplay);
            }
            return;
        }
        self.toast = None;
        let focus = if dry { self.focus } else { self.focus.take() };
        ui.painter()
            .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
        let page = self.page;
        // Every page is the same box, so turning one doesn't move or
        // resize it: the title stays put, the hints stay at the foot.
        let frame = egui::Frame::window(ui.style()).inner_margin(MARGIN);
        let edge = 2.0 * (EDGE + MARGIN + 1.0);
        // But the Controls page, wider for the GBA beside its list.
        let width = if page == Page::Remap {
            CONTROLS_WIDTH
        } else {
            PAGE_WIDTH
        };
        let size = egui::vec2(
            width.min(screen.width() - edge),
            PAGE_HEIGHT.min(screen.height() - edge),
        )
        .max(egui::vec2(ROW_HEIGHT, ROW_HEIGHT));
        egui::Window::new("menu")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .frame(frame)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(size.x);
                ui.set_height(size.y);
                // Focus lost (to a click outside, or Escape on a
                // keyboard) comes back to the first row.
                let lost = ui.memory(|m| m.focused().is_none());
                let mut rows = Rows {
                    focus: focus.or(lost.then_some(0)),
                    n: 0,
                    shown: self.shown,
                };
                match page {
                    Page::Main => self.main_page(ui, &mut rows),
                    Page::Remap => self.controls_page(ui),
                    Page::Scripts => self.scripts_page(ui, &mut rows),
                    Page::Netplay => self.netplay_page(ui, &mut rows),
                    Page::Saves => self.saves_page(ui, &mut rows),
                    Page::Folder => self.folder_page(ui, &mut rows),
                }
                self.shown = rows.shown;
            });
        if page == Page::Main && !self.report.is_empty() {
            egui::Area::new(egui::Id::unique("report"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -8.0])
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(screen.width() - 40.0);
                        for l in &self.report {
                            ui.label(egui::RichText::new(l).small());
                        }
                    });
                });
        }
    }

    fn main_page(&mut self, ui: &mut egui::Ui, rows: &mut Rows) {
        title(ui, "gbaa");
        let lines = 1 + usize::from(self.saved.is_some());
        scrolled(ui, lines, |ui| {
            // In netplay the game plays on: the others don't wait.
            let said = match (self.in_session(), self.paused) {
                (true, _) => "off (netplay)",
                (false, true) => "on",
                (false, false) => "off",
            };
            let paused = rows.row(ui, "Paused", said);
            if paused.step != 0 && !self.in_session() {
                self.paused = !self.paused;
            }
            let speed = rows.row(ui, "Fast-forward", &speed_label(self.speed));
            if speed.step != 0 {
                self.speed = step_speed(self.speed, speed.step);
                self.speed_changed = true;
            }
            let volume = rows.row(ui, "Volume", &format!("{}%", self.volume));
            if volume.step != 0 {
                self.set_volume(step_volume(self.volume, volume.step));
            }
            let mute = rows.row(ui, "Mute", if self.muted { "on" } else { "off" });
            if mute.step != 0 {
                self.muted = !self.muted;
                self.sound_changed = true;
            }
            // Greyed, it's no focus stop: the arrows pass over it.
            let undo = ui
                .add_enabled_ui(self.undoable, |ui| rows.row(ui, "Undo load state", ""))
                .inner;
            // Undone, the game goes on to show it.
            if self.undoable && undo.step != 0 {
                self.undo = true;
                self.undoable = false;
                self.set_open(false);
            }
            if rows.link(ui, "Controls").step != 0 {
                self.turn(Page::Remap, 0);
            }
            let full = if self.fullscreen { "on" } else { "off" };
            if rows.row(ui, "Fullscreen", full).step != 0 {
                self.fullscreen_asked = true;
            }
            if rows.link(ui, "Scripts").step != 0 {
                self.turn(Page::Scripts, 0);
            }
            if rows.link(ui, "Netplay").step != 0 {
                self.turn(Page::Netplay, 0);
            }
            let resume = rows.row(
                ui,
                "Resume on launch",
                if self.resume { "on" } else { "off" },
            );
            if resume.step != 0 {
                self.set_resume(!self.resume);
            }
            if rows.link(ui, "Save folders").step != 0 {
                // The game's row is greyed when the launcher has it.
                self.turn(Page::Saves, usize::from(self.saves.launcher));
            }
            if rows.row(ui, "Close", "").step != 0 {
                self.set_open(false);
            }
            self.quit |= rows.row(ui, "Quit", "").step != 0;
        });
        let mut hints = vec!["A/Enter: choose  ·  ‹ ›: change  ·  B/Esc: close"];
        hints.extend(self.saved.as_deref());
        footer(ui, &hints);
    }
}

impl Menu {
    /// The Scripts page: the console shown or not, a row to type a
    /// command, each script loaded (A unloads it), each one found to load
    /// (A loads it, and shows the console), and Back. Loading and
    /// unloading close the menu: the scripts run with the game, which the
    /// menu pauses.
    fn scripts_page(&mut self, ui: &mut egui::Ui, rows: &mut Rows) {
        title(ui, "Scripts");
        let shown = if self.console.shown { "on" } else { "off" };
        if rows.row(ui, "Console", shown).step != 0 {
            self.console.set_shown(!self.console.shown);
        }
        if rows.row(ui, "Type a command", "").step != 0 {
            self.console.set_shown(true);
            self.console.focus_line = true;
            self.set_open(false);
            return;
        }
        let name = |p: &std::path::Path| {
            p.file_name()
                .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())
        };
        let same = |a: &std::path::Path, b: &std::path::Path| {
            a == b || std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
        };
        let loaded = self.console.view.loaded.clone();
        let mut back = false;
        scrolled(ui, 1, |ui| {
            for (id, p) in &loaded {
                let r = rows.row(ui, &format!("Unload {}", name(p)), "");
                // The game is paused while the menu's open (scripts don't
                // run in netplay), and the scripts run with it: close for
                // it to be done.
                if r.response.clicked() {
                    self.console.cmds.push(ScriptCmd::Unload(*id));
                    self.set_open(false);
                }
            }
            let files: Vec<_> = self
                .files
                .iter()
                .filter(|f| !loaded.iter().any(|(_, p)| same(p, f)))
                .cloned()
                .collect();
            for f in files {
                let r = rows.row(ui, &format!("Load {}", name(&f)), "");
                if r.response.clicked() {
                    self.console.cmds.push(ScriptCmd::Load(f));
                    self.console.set_shown(true);
                    self.set_open(false);
                }
            }
            back = rows.row(ui, "Back", "").step != 0;
        });
        let dir = gamepad::config_dir().map(|d| d.join("scripts"));
        let hint = match dir {
            Some(d) => format!("Scripts from {} and beside those loaded", d.display()),
            None => "Scripts beside those loaded".to_owned(),
        };
        footer(ui, &[&hint]);
        if back {
            self.turn(Page::Main, SCRIPTS_ROW);
        }
    }
}

impl Menu {
    /// The Netplay page: out of a room, the server, Host, the code and
    /// Join; in one, where it is (the code big while the host waits) and
    /// Leave.
    fn netplay_page(&mut self, ui: &mut egui::Ui, rows: &mut Rows) {
        title(ui, "Netplay");
        let mut back = false;
        let lines = 1 + usize::from(self.saved.is_some());
        let state = self.netplay.clone();
        scrolled(ui, lines, |ui| {
            let said = |ui: &mut egui::Ui, text: &str| {
                ui.add(egui::Label::new(text).wrap());
            };
            match &state {
                room::State::Off | room::State::Ended(_) => {
                    if let room::State::Ended(why) = &state {
                        said(ui, why);
                    }
                    let server = match self.edit.as_ref().filter(|e| e.field == Field::Server) {
                        Some(e) => e.shown(),
                        None => self.server.clone(),
                    };
                    if rows.row(ui, "Server", &server).response.clicked() {
                        self.edit = Some(Edit::new(Field::Server, &self.server));
                    }
                    let players = rows.row(ui, "Players", &self.players.to_string()).step;
                    if players != 0 {
                        // 2, 3, 4 and round: A steps on, as right does.
                        let n = room::MAX_SEATS - 1;
                        self.players =
                            2 + (self.players as i32 - 2 + players).rem_euclid(n as i32) as usize;
                    }
                    if rows.row(ui, "Host a room", "").step != 0 {
                        self.netplay_ask = Some(room::Ask {
                            server: self.server.clone(),
                            role: room::Role::Host,
                            players: self.players,
                        });
                    }
                    let code = match self.edit.as_ref().filter(|e| e.field == Field::Code) {
                        Some(e) => e.shown(),
                        None if self.code.is_empty() => "type it".into(),
                        None => self.code.clone(),
                    };
                    if rows.row(ui, "Room code", &code).response.clicked() {
                        self.edit = Some(Edit::new(Field::Code, &self.code));
                    }
                    if rows.row(ui, "Join", "").step != 0 {
                        match room::parse_code(&self.code) {
                            Ok(c) => {
                                self.netplay_ask = Some(room::Ask {
                                    server: self.server.clone(),
                                    role: room::Role::Join(c),
                                    players: 0,
                                })
                            }
                            Err(_) => {
                                self.saved = Some(format!(
                                    "type the room's code first ({} letters and digits)",
                                    relay::CODE_LEN
                                ))
                            }
                        }
                    }
                }
                room::State::Making => said(ui, "Asking the server for a room…"),
                room::State::Waiting {
                    code,
                    filled,
                    seats,
                } => {
                    said(ui, "Your room's code, for the others to join with:");
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(code).size(44.0).monospace().strong());
                    });
                    said(
                        ui,
                        &format!("{filled} of {seats} players in; waiting for the rest…"),
                    );
                }
                room::State::Joining(code) => said(ui, &format!("Joining room {code}…")),
                room::State::Playing { code, lines, .. } => {
                    ui.label(egui::RichText::new(format!("Room {code}")).heading());
                    for l in lines {
                        said(ui, l);
                    }
                }
            }
            if !matches!(state, room::State::Off | room::State::Ended(_))
                && rows.row(ui, "Leave the room", "").step != 0
            {
                self.leave = true;
            }
            back = rows.row(ui, "Back", "").step != 0;
        });
        let hint = if self.edit.is_some() {
            "type, or up/down: letter  ·  ‹ ›: move  ·  A: keep  ·  B: drop"
        } else {
            "A/Enter: choose  ·  B/Esc: back"
        };
        let mut hints = vec![hint];
        hints.extend(self.saved.as_deref());
        footer(ui, &hints);
        if back {
            self.turn(Page::Main, NETPLAY_ROW);
        }
    }
}

/// The rows of a page, counted so one can be focused.
struct Rows {
    /// The row to focus.
    focus: Option<usize>,
    n: usize,
    /// The focused row, once it's been scrolled into view.
    shown: Option<egui::Id>,
}

impl Rows {
    /// A row with a `value`, which left and right step.
    fn row(&mut self, ui: &mut egui::Ui, label: &str, value: &str) -> Row {
        self.add(ui, label, Value::Text(value))
    }

    /// A row that turns to another page, marked so.
    fn link(&mut self, ui: &mut egui::Ui, label: &str) -> Row {
        self.add(ui, label, Value::Link)
    }

    fn add(&mut self, ui: &mut egui::Ui, label: &str, value: Value) -> Row {
        let r = row(ui, label, value);
        if self.focus == Some(self.n) {
            r.response.request_focus();
        }
        // Newly focused, it's scrolled to, once: not every pass, or a wheel
        // or a drag couldn't scroll it away. (egui's `gained_focus` is
        // never true for a focus the arrow keys moved: it moves as the
        // pass begins, and "last pass's focus" with it.)
        if r.response.has_focus() && self.shown != Some(r.response.id) {
            self.shown = Some(r.response.id);
            r.response.scroll_to_me(None);
        }
        self.n += 1;
        r
    }
}

/// Every page's box, in points (less where the screen is smaller); the
/// main page's rows fit it without scrolling.
const PAGE_WIDTH: f32 = 320.0;
const PAGE_HEIGHT: f32 = 380.0;
/// The Controls page's width: the Deck's whole screen, less its edges.
const CONTROLS_WIDTH: f32 = 620.0;
/// The box's padding, and the least gap left round it on the screen.
const MARGIN: f32 = 12.0;
const EDGE: f32 = 8.0;
/// A row's height, in points.
const ROW_HEIGHT: f32 = 26.0;
/// A value's colour, set off from its label.
const VALUE: egui::Color32 = egui::Color32::from_rgb(150, 190, 235);

/// The menu's look: text that reads at the Deck's 1280x800 and in a small
/// window, a focus that stands out from across the room, rounded boxes.
fn style(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, TextStyle};
    ctx.all_styles_mut(|s| {
        // No fades: a pad press shows at once, and nothing asks to be
        // redrawn after it.
        s.animation_time = 0.0;
        for (t, size) in [
            (TextStyle::Heading, 18.0),
            (TextStyle::Body, 13.0),
            (TextStyle::Button, 13.0),
            (TextStyle::Small, 11.0),
        ] {
            s.text_styles
                .insert(t, FontId::new(size, FontFamily::Proportional));
        }
        s.spacing.item_spacing.y = 2.0;
        // A list longer than its box shows its handle, not only when
        // hovered: a pad has no pointer.
        s.spacing.scroll.dormant_handle_opacity = 0.7;
        s.visuals.window_corner_radius = 10.into();
        s.visuals.window_fill = egui::Color32::from_rgb(28, 30, 34);
        s.visuals.selection.bg_fill = egui::Color32::from_rgb(38, 100, 180);
        s.visuals.selection.stroke.color = egui::Color32::from_rgb(170, 210, 255);
    });
}

/// A page's title, over a rule.
fn title(ui: &mut egui::Ui, text: &str) {
    ui.vertical_centered(|ui| ui.heading(text));
    ui.add(egui::Separator::default().spacing(RULE));
}

/// The space a rule takes.
const RULE: f32 = 8.0;

/// A page's scrolling rows, `add`ed into all the height left above a
/// [`footer`] of `lines` hints.
fn scrolled<R>(ui: &mut egui::Ui, lines: usize, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let gap = ui.spacing().item_spacing.y;
    let line = ui.text_style_height(&egui::TextStyle::Small) + gap;
    let height = ui.available_height() - (RULE + gap) - lines as f32 * line - gap;
    // Not eased either: a row moved to is in view at the next pass.
    egui::ScrollArea::vertical()
        .max_height(height.max(ROW_HEIGHT))
        .auto_shrink(false)
        .animated(false)
        .show(ui, add)
        .inner
}

/// A page's hints, each kept to a line, under a rule at the box's foot.
fn footer(ui: &mut egui::Ui, lines: &[&str]) {
    ui.add(egui::Separator::default().spacing(RULE));
    for l in lines {
        ui.add(egui::Label::new(egui::RichText::new(*l).small()).truncate());
    }
}

/// `speed` as the menu shows it.
pub fn speed_label(speed: f64) -> String {
    if speed == 0.0 {
        "unlimited".into()
    } else {
        format!("{speed}x")
    }
}

/// The speed after `speed` in [`SPEEDS`] (`step` 1) or before it (-1),
/// wrapping round; a speed not in the list steps to its neighbour.
pub fn step_speed(speed: f64, step: i32) -> f64 {
    // Unthrottled is the fastest.
    let rank = |s: f64| if s == 0.0 { f64::INFINITY } else { s };
    let n = SPEEDS.len();
    let i = if step > 0 {
        SPEEDS
            .iter()
            .position(|&s| rank(s) > rank(speed))
            .unwrap_or(0)
    } else {
        SPEEDS
            .iter()
            .rposition(|&s| rank(s) < rank(speed))
            .unwrap_or(n - 1)
    };
    SPEEDS[i]
}

/// The volume a [`VOLUME_STEP`] above `volume` (`step` 1) or below it
/// (-1), held within 0..=[`MAX_VOLUME`]; a volume off the steps goes to
/// the next one that way.
pub fn step_volume(volume: u32, step: i32) -> u32 {
    if step > 0 {
        ((volume / VOLUME_STEP + 1) * VOLUME_STEP).min(MAX_VOLUME)
    } else {
        volume.div_ceil(VOLUME_STEP).saturating_sub(1) * VOLUME_STEP
    }
}

/// A row and what was done to it: `step` 1 for Enter, a click or right,
/// -1 for left, else 0.
struct Row {
    response: egui::Response,
    step: i32,
}

/// What a row shows on its right.
#[derive(Clone, Copy)]
enum Value<'a> {
    /// A value (none if empty), which left and right step.
    Text(&'a str),
    /// The row turns to another page.
    Link,
}

/// One menu row, one focus stop: `label` on the left, `value` (`‹ value ›`
/// while focused) or a link's `›` on the right. Focused, it's filled and
/// outlined, with a bar at its left; greyed, its text is faint.
fn row(ui: &mut egui::Ui, label: &str, value: Value) -> Row {
    let size = egui::vec2(ui.available_width(), ROW_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let focused = response.has_focus();
    let mut step = i32::from(response.clicked());
    let stepped = matches!(value, Value::Text(v) if !v.is_empty());
    if focused && stepped {
        // Left and right change the value; they mustn't move the focus.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    ..Default::default()
                },
            )
        });
        ui.input(|i| {
            step += i32::from(i.key_pressed(egui::Key::ArrowRight));
            step -= i32::from(i.key_pressed(egui::Key::ArrowLeft));
        });
    }
    let v = ui.visuals();
    let painter = ui.painter();
    let (text, value_colour) = if focused {
        let sel = v.selection;
        painter.rect(
            rect,
            5.0,
            sel.bg_fill,
            (1.5, sel.stroke.color),
            egui::StrokeKind::Inside,
        );
        let bar = egui::Rect::from_min_size(rect.min, egui::vec2(4.0, rect.height()));
        let round = egui::CornerRadius {
            nw: 5,
            sw: 5,
            ne: 0,
            se: 0,
        };
        painter.rect_filled(bar, round, sel.stroke.color);
        (egui::Color32::WHITE, egui::Color32::WHITE)
    } else if !ui.is_enabled() {
        // The greyed ui fades it too.
        (v.text_color(), v.text_color())
    } else {
        if response.hovered() {
            painter.rect_filled(rect, 5.0, v.widgets.hovered.weak_bg_fill);
        }
        (v.text_color(), VALUE)
    };
    let font = egui::TextStyle::Button.resolve(ui.style());
    let inset = rect.shrink2(egui::vec2(12.0, 0.0));
    painter.text(
        inset.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        font.clone(),
        text,
    );
    let value = match value {
        Value::Text("") => return Row { response, step },
        Value::Text(value) if focused => format!("‹  {value}  ›"),
        Value::Text(value) => value.to_owned(),
        Value::Link => "›".to_owned(),
    };
    painter.text(
        inset.right_center(),
        egui::Align2::RIGHT_CENTER,
        value,
        font,
        value_colour,
    );
    Row { response, step }
}

/// The menu's scale, in pixels a point, for a frame drawn at `scale`: 2 on
/// the Deck (5x), never under 1, so its text reads on a TV or a handheld.
pub fn pixels_per_point(scale: usize) -> f32 {
    (scale as f32 * 0.4).max(1.0)
}

/// Draw `mark` small in the top right corner, over the game: a dot in its
/// colour and its text, on a dim backing, out of the way of play.
fn draw_mark(ctx: &egui::Context, mark: Mark, state: &room::State) {
    egui::Area::new(egui::Id::unique("netplay mark"))
        .anchor(egui::Align2::RIGHT_TOP, [-6.0, 6.0])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(150))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let (r, p) =
                            ui.allocate_painter(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        p.circle_filled(r.rect.center(), 4.0, mark.colour());
                        ui.label(
                            egui::RichText::new(mark.text(state))
                                .size(11.0)
                                .color(egui::Color32::from_gray(230)),
                        );
                    });
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pass at the Deck's size, as the window runs it: how many
    /// shapes it draws.
    fn pass(m: &mut Menu) -> usize {
        let mut out = m.run((1280, 800), 2.0, 0.0);
        out.textures_delta.clear();
        out.shapes.len()
    }

    fn press(p: &mut MenuPad, b: Button, open: bool) -> Option<Nav> {
        let n = p.feed(0, PadEvent::Pressed(b), open, Instant::now());
        p.feed(0, PadEvent::Released(b), open, Instant::now());
        n
    }

    #[test]
    fn start_and_select_open_it_and_b_closes_it() {
        let mut p = MenuPad::default();
        assert_eq!(press(&mut p, Button::South, false), None);
        assert_eq!(press(&mut p, Button::DPadDown, false), None);
        assert_eq!(
            p.feed(0, PadEvent::Pressed(Button::Back), false, Instant::now()),
            None
        );
        assert_eq!(
            p.feed(0, PadEvent::Pressed(Button::Start), false, Instant::now()),
            Some(Nav::Toggle)
        );
        p.feed(0, PadEvent::Released(Button::Start), true, Instant::now());
        p.feed(0, PadEvent::Released(Button::Back), true, Instant::now());
        // An Xbox pad: A south, B east.
        assert_eq!(press(&mut p, Button::South, true), Some(Nav::Ok));
        assert_eq!(press(&mut p, Button::East, true), Some(Nav::Back));
        assert_eq!(press(&mut p, Button::Start, true), Some(Nav::Back));
        assert_eq!(press(&mut p, Button::DPadUp, true), Some(Nav::Up));
    }

    #[test]
    fn a_and_b_follow_the_labels() {
        let mut p = MenuPad::default();
        // A Nintendo layout: A east, B south.
        let nintendo = [Button::East, Button::South, Button::North, Button::West];
        p.feed(0, PadEvent::Labels(nintendo), true, Instant::now());
        assert_eq!(press(&mut p, Button::East, true), Some(Nav::Ok));
        assert_eq!(press(&mut p, Button::South, true), Some(Nav::Back));
    }

    #[test]
    fn the_stick_moves_once_per_push() {
        let mut p = MenuPad::default();
        let t = Instant::now();
        let mut push =
            |a, v, ms| p.feed(0, PadEvent::Axis(a, v), true, t + Duration::from_millis(ms));
        // Past the deadzone but short of half: a brush, nothing.
        assert_eq!(push(Axis::LeftY, 0.45, 0), None);
        assert_eq!(push(Axis::LeftY, 0.8, 10), Some(Nav::Down));
        assert_eq!(push(Axis::LeftY, 1.0, 20), None);
        // Easing off, but not into the deadzone, doesn't let it go.
        assert_eq!(push(Axis::LeftY, 0.45, 30), None);
        assert_eq!(push(Axis::LeftY, 0.9, 40), None);
        // Let go, its spring overshoots: no move up.
        assert_eq!(push(Axis::LeftY, 0.0, 50), None);
        assert_eq!(push(Axis::LeftY, -0.6, 60), None);
        assert_eq!(push(Axis::LeftY, 0.0, 70), None);
        // Up on purpose, later, is a move.
        assert_eq!(push(Axis::LeftY, -0.9, 400), Some(Nav::Up));
        // Rolled round the corner to the right, a move; the furthest way
        // counts, so a little up with it isn't one.
        assert_eq!(push(Axis::LeftX, 0.95, 410), Some(Nav::Right));
        assert_eq!(push(Axis::LeftY, -0.2, 420), None);
        // Swung straight across to the left, without the middle: a flick.
        assert_eq!(push(Axis::LeftX, -0.9, 430), None);
    }

    #[test]
    fn a_wider_deadzone_takes_a_wider_push() {
        let mut p = MenuPad {
            deadzones: crate::gamepad::Deadzones::percent(70, 20),
            ..MenuPad::default()
        };
        let t = Instant::now();
        let mut push = |a, v| p.feed(0, PadEvent::Axis(a, v), true, t);
        assert_eq!(push(Axis::LeftY, 0.6), None);
        assert_eq!(push(Axis::LeftY, 0.8), Some(Nav::Down));
        // Listening, a trigger binds past half (or its deadzone, if wider).
        p.listening = true;
        let n = p.feed(0, PadEvent::Axis(Axis::TriggerLeft, 0.6), true, t);
        assert_eq!(n, Some(Nav::Bind(Control::Stick(Axis::TriggerLeft, true))));
    }

    #[test]
    fn a_held_direction_moves_again_and_again() {
        let mut r = Repeat::default();
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        r.pressed(Nav::Down, at(0));
        let held = |n| n == Nav::Down;
        // Nothing for 0.5 s, then a move every 0.1 s.
        assert_eq!(r.due(at(499), held), None);
        assert_eq!(r.due(at(500), held), Some(Nav::Down));
        assert_eq!(r.due(at(550), held), None);
        assert_eq!(r.due(at(600), held), Some(Nav::Down));
        assert_eq!(r.next(), Some(at(700)));
        // Let go, it stops; another press, or Ok, starts or stops it.
        assert_eq!(r.due(at(1000), |_| false), None);
        assert_eq!(r.next(), None);
        r.pressed(Nav::Up, at(1100));
        r.pressed(Nav::Ok, at(1150));
        assert_eq!(r.next(), None);
        // A pad holds the D-pad's way, and the stick's.
        let mut p = MenuPad::default();
        p.feed(0, PadEvent::Pressed(Button::DPadUp), true, t);
        assert!(p.holds(Nav::Up) && !p.holds(Nav::Down));
        p.feed(0, PadEvent::Released(Button::DPadUp), true, t);
        p.feed(0, PadEvent::Axis(Axis::LeftX, 0.9), true, t);
        assert!(p.holds(Nav::Right) && !p.holds(Nav::Up));
        p.feed(0, PadEvent::Axis(Axis::LeftX, 0.0), true, t);
        assert!(!p.holds(Nav::Right));
    }

    #[test]
    fn a_press_said_twice_is_one_move() {
        let mut e = Echoes::default();
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        // Steam's D-pad: the pad's press, and an arrow key with it.
        assert!(e.keep(Nav::Down, Source::Buttons(3), at(0)));
        assert!(!e.keep(Nav::Down, Source::Keys, at(5)));
        // Pressed again, the same way twice is two moves.
        assert!(e.keep(Nav::Down, Source::Buttons(3), at(30)));
        assert!(e.keep(Nav::Down, Source::Buttons(3), at(60)));
        // Long after, the keyboard's is its own.
        assert!(e.keep(Nav::Down, Source::Keys, at(500)));
        assert!(e.keep(Nav::Up, Source::Stick(3), at(505)));
        // A bind is never dropped; the same press from the other pad (or
        // as Enter), once it's stopped listening, is.
        let a = Nav::Bind(Control::Label(Label::A));
        assert!(e.keep(a, Source::Buttons(3), at(1000)));
        assert!(!e.keep(Nav::Ok, Source::Buttons(4), at(1010)));
        assert!(!e.keep(Nav::Ok, Source::Keys, at(1020)));
        assert!(e.keep(Nav::Ok, Source::Buttons(3), at(1030)));
    }

    #[test]
    fn speeds_step_and_wrap() {
        assert_eq!(step_speed(2.0, 1), 2.5);
        assert_eq!(step_speed(16.0, 1), 0.0);
        assert_eq!(step_speed(0.0, 1), 1.5);
        assert_eq!(step_speed(1.5, -1), 0.0);
        assert_eq!(step_speed(0.0, -1), 16.0);
        // --fast-speed 5 and 1.2 aren't in the list.
        assert_eq!(step_speed(5.0, 1), 8.0);
        assert_eq!(step_speed(5.0, -1), 4.0);
        assert_eq!(step_speed(1.2, 1), 1.5);
        assert_eq!(speed_label(2.5), "2.5x");
        assert_eq!(speed_label(2.0), "2x");
    }

    #[test]
    fn a_room_shows_its_mark_over_the_game() {
        let mut m = Menu::new(4.0, 100, Controls::default());
        // In no room, nothing is drawn over the game.
        assert!(!m.drawn());
        let playing = |left: Option<&str>, quiet| room::State::Playing {
            code: "QWERTY".into(),
            seat: 0,
            seats: 2,
            lines: Vec::new(),
            left: left.map(Into::into),
            quiet,
            desync: None,
        };
        let marks = [
            (room::State::Making, Mark::Connecting, "Connecting…"),
            (
                room::State::Joining("QWERTY".into()),
                Mark::Connecting,
                "Connecting…",
            ),
            (
                room::State::Waiting {
                    code: "QWERTY".into(),
                    filled: 1,
                    seats: 2,
                },
                Mark::Waiting,
                "Waiting for players · 1/2",
            ),
            (
                room::State::Waiting {
                    code: "QWERTY".into(),
                    filled: 3,
                    seats: 4,
                },
                Mark::Waiting,
                "Waiting for players · 3/4",
            ),
            (playing(None, false), Mark::Connected, "Connected · 2/2"),
            (
                playing(None, true),
                Mark::Quiet,
                "Not hearing from them · 2/2",
            ),
            (
                playing(Some("Player 2 left the room"), false),
                Mark::Lost,
                "Lost: playing alone · 1/2",
            ),
            (
                room::State::Playing {
                    code: "QWERTY".into(),
                    seat: 0,
                    seats: 2,
                    lines: Vec::new(),
                    left: None,
                    quiet: true,
                    desync: Some(240),
                },
                Mark::Desynced,
                "Out of sync · 2/2",
            ),
        ];
        let bare = pass(&mut m);
        for (state, mark, text) in marks {
            assert_eq!(Mark::of(&state), Some(mark));
            assert_eq!(mark.text(&state), text);
            m.set_netplay(state);
            m.toast = None;
            assert!(m.drawn());
            // Drawn with the menu shut: more than a bare pass.
            assert!(pass(&mut m) > bare, "{mark:?} drew nothing");
        }
        m.set_netplay(room::State::Ended("done".into()));
        assert!(!m.drawn());
    }

    #[test]
    fn a_netplay_session_is_never_paused() {
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.set_netplay(room::State::Playing {
            code: "QWERTY".into(),
            seat: 1,
            seats: 2,
            lines: Vec::new(),
            left: None,
            quiet: false,
            desync: None,
        });
        pass(&mut m);
        m.nav(Nav::Toggle);
        pass(&mut m);
        assert!(m.open && !m.paused);
        // The Paused row (focused first) doesn't pause it either.
        m.nav(Nav::Ok);
        pass(&mut m);
        assert!(m.open && !m.paused);
        // Out of the session, the menu pauses again.
        m.set_netplay(room::State::Off);
        m.set_open(false);
        m.nav(Nav::Toggle);
        assert!(m.open && m.paused);
    }

    #[test]
    fn the_pad_drives_the_menu() {
        let mut m = Menu::new(4.0, 100, Controls::default());
        pass(&mut m);
        m.nav(Nav::Toggle);
        assert!(m.open && m.paused);
        // Drawn, with the first row (Paused) focused.
        assert!(pass(&mut m) > 0);
        // Down to fast-forward, right once: 8x.
        m.nav(Nav::Down);
        pass(&mut m);
        m.nav(Nav::Right);
        pass(&mut m);
        assert_eq!((m.speed, m.speed_changed), (8.0, true));
        // Left and right didn't move the focus: left goes back to 4x.
        m.nav(Nav::Left);
        pass(&mut m);
        assert_eq!(m.speed, 4.0);
        // Up to Paused, A: the game goes on with the menu open.
        m.nav(Nav::Up);
        pass(&mut m);
        m.nav(Nav::Ok);
        pass(&mut m);
        assert!(m.open && !m.paused);
        // Down ten times to Close (past Undo load state, greyed), A.
        for _ in 0..10 {
            m.nav(Nav::Down);
            pass(&mut m);
        }
        m.nav(Nav::Ok);
        pass(&mut m);
        assert!(!m.open && !m.quit);
        // Opened again, the focus is back on the first row.
        m.nav(Nav::Toggle);
        pass(&mut m);
        m.nav(Nav::Ok);
        pass(&mut m);
        assert!(!m.paused);
        m.nav(Nav::Back);
        assert!(!m.open);
    }

    #[test]
    fn quit_is_the_last_row() {
        let mut m = Menu::new(0.0, 100, Controls::default());
        m.nav(Nav::Toggle);
        pass(&mut m);
        for _ in 0..11 {
            m.nav(Nav::Down);
            pass(&mut m);
        }
        m.nav(Nav::Ok);
        pass(&mut m);
        assert!(m.quit);
    }

    #[test]
    fn the_fullscreen_row_asks_the_window() {
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.nav(Nav::Toggle);
        pass(&mut m);
        // Paused, Fast-forward, Volume, Mute, Controls (Undo greyed),
        // Fullscreen.
        go(
            &mut m,
            &[
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Ok,
            ],
        );
        assert!(m.fullscreen_asked && m.open);
        // Left or right asks too, with the menu staying on the row.
        m.fullscreen_asked = false;
        m.fullscreen = true;
        go(&mut m, &[Nav::Left]);
        assert!(m.fullscreen_asked && !m.quit);
    }

    /// Make each move, with a pass after it.
    fn go(m: &mut Menu, navs: &[Nav]) {
        for &n in navs {
            m.nav(n);
            pass(m);
        }
    }

    /// A menu for ROM `rom`, open on the Controls page.
    fn on_controls(rom: crate::remap::RomId) -> Menu {
        let mut m = Menu::new(
            4.0,
            100,
            Controls {
                rom,
                ..Controls::default()
            },
        );
        pass(&mut m);
        go(
            &mut m,
            &[
                Nav::Toggle,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Ok,
            ],
        );
        assert_eq!(m.page, Page::Remap);
        // It opens on the tabs; down to the scope switch.
        assert!(shows(&mut m, "A is this tab; left or right for the other"));
        go(&mut m, &[Nav::Down]);
        m
    }

    /// The text this pass draws.
    fn texts(m: &mut Menu) -> Vec<String> {
        let mut out = m.run((1280, 800), 2.0, 0.0);
        out.textures_delta.clear();
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    fn shows(m: &mut Menu, what: &str) -> bool {
        texts(m).iter().any(|t| t.contains(what))
    }

    #[test]
    fn the_controls_page_adds_removes_and_undoes() {
        use runtime::io::key;
        let mut m = on_controls(Default::default());
        // It opens for every game, on the scope switch: A there changes
        // nothing.
        go(&mut m, &[Nav::Ok]);
        assert!(!m.remap.this_game && !m.mapping_changed);
        assert!(shows(&mut m, "A make changes for every game"));
        // Down to A's row: its chip, then "+ Add", which listens.
        go(&mut m, &[Nav::Down]);
        assert!(shows(&mut m, "A remove A button from A"));
        go(&mut m, &[Nav::Right, Nav::Right]);
        assert!(shows(&mut m, "A add another control to A"));
        go(&mut m, &[Nav::Ok]);
        assert!(m.listening() && !m.listening_for_keys());
        assert!(shows(&mut m, "Adding a control to the GBA's A"));
        // Moves are the dialog's now: arrows do nothing.
        go(&mut m, &[Nav::Down, Nav::Ok]);
        assert!(m.listening());
        let mut p = MenuPad {
            listening: true,
            ..MenuPad::default()
        };
        // The right shoulder is added, and moved from R: said, undoable.
        let n = press(&mut p, Button::RightShoulder, true).unwrap();
        let rb = Control::Button(Button::RightShoulder);
        assert_eq!(n, Nav::Bind(rb));
        go(&mut m, &[n]);
        assert!(!m.listening() && m.mapping_changed);
        let a = [Control::Label(Label::A), Control::Label(Label::Y), rb];
        assert_eq!(m.controls.mapping().controls(key::A), a);
        assert!(!m.controls.mapping().controls(key::R).contains(&rb));
        assert!(shows(&mut m, "RB moved from R to A (every game)."));
        assert!(
            m.controls
                .config
                .names(&crate::remap::Section::All, key::A, false)
        );
        // Y undoes it.
        p.listening = false;
        assert_eq!(press(&mut p, Button::North, true), Some(Nav::Undo));
        go(&mut m, &[Nav::Undo]);
        assert_eq!(m.controls.mapping(), crate::gamepad::Mapping::default());
        // Adding what's there already says so.
        m.mapping_changed = false;
        go(&mut m, &[Nav::Ok, Nav::Bind(Control::Label(Label::A))]);
        assert!(shows(&mut m, "A button already presses A."));
        assert!(!m.mapping_changed);
        // Left to the Y button's chip and the A button's: A removes each,
        // and the row says nothing presses it.
        go(&mut m, &[Nav::Left, Nav::Ok]);
        assert_eq!(
            m.controls.mapping().controls(key::A),
            [Control::Label(Label::A)]
        );
        go(&mut m, &[Nav::Left, Nav::Ok]);
        assert!(m.controls.mapping().controls(key::A).is_empty());
        assert!(shows(&mut m, "not bound"));
        // The focus is on "+ Add" now, the row's only stop.
        assert!(shows(&mut m, "A add a control to A"));
        // Undo puts back the last one taken.
        go(&mut m, &[Nav::Undo]);
        assert_eq!(
            m.controls.mapping().controls(key::A),
            [Control::Label(Label::A)]
        );
        // B: back to the main page, focused on Controls.
        go(&mut m, &[Nav::Back]);
        assert_eq!(m.page, Page::Main);
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.page, Page::Remap);
    }

    #[test]
    fn the_controls_page_changes_one_game() {
        use crate::remap::{RomId, Section};
        use runtime::io::key;
        let mut m = on_controls(RomId {
            hash: 7,
            title: "POKEMON EMER".into(),
        });
        assert!(shows(&mut m, "Only POKEMON EMER"));
        go(&mut m, &[Nav::Right, Nav::Ok]);
        assert!(m.remap.this_game);
        // Rows from every game's say nothing of it.
        assert!(!shows(&mut m, "from every game"));
        // B's "+ Add", and LB: moved from L, for this game only.
        go(
            &mut m,
            &[Nav::Down, Nav::Down, Nav::Right, Nav::Right, Nav::Ok],
        );
        let lb = Control::Button(Button::LeftShoulder);
        go(&mut m, &[Nav::Bind(lb)]);
        assert!(shows(&mut m, "LB moved from L to B (this game)."));
        assert!(shows(&mut m, "this game"));
        let here = |m: &Menu, bit| m.controls.config.names(&Section::Hash(7), bit, false);
        assert!(here(&m, key::B) && here(&m, key::L));
        assert!(!m.controls.config.names(&Section::All, key::B, false));
        // Right of "+ Add": "use every-game", which drops this game's B.
        go(&mut m, &[Nav::Right]);
        assert!(shows(&mut m, "A use every game's controls for B"));
        go(&mut m, &[Nav::Ok]);
        assert!(!here(&m, key::B) && here(&m, key::L));
        assert_eq!(
            m.controls.mapping().controls(key::B),
            [Control::Label(Label::B), Control::Label(Label::X)]
        );
    }

    #[test]
    fn the_keyboard_tab_listens_for_keys() {
        use runtime::io::key;
        use sdl3::keyboard::Scancode;
        let mut m = on_controls(Default::default());
        // Up to the tabs: right turns to the keyboard's.
        go(&mut m, &[Nav::Up, Nav::Right]);
        assert!(m.remap.keyboard);
        assert!(shows(
            &mut m,
            "Enter is this tab; left or right for the other"
        ));
        go(&mut m, &[Nav::Down, Nav::Left]);
        assert!(shows(&mut m, "Enter make changes for every game"));
        // B's "+ Add".
        go(&mut m, &[Nav::Down, Nav::Down, Nav::Right, Nav::Ok]);
        assert!(m.listening_for_keys());
        assert!(shows(&mut m, "Press a key…"));
        // A pad's press is passed over (Steam says it as a key too);
        // Escape stops it, leaving the page open.
        go(&mut m, &[Nav::Bind(Control::Button(Button::South))]);
        assert!(m.listening() && !m.mapping_changed);
        go(&mut m, &[Nav::Back]);
        assert!(!m.listening() && m.page == Page::Remap);
        // J is added to B, the pads' B as it was.
        go(&mut m, &[Nav::Ok]);
        let j = Control::Key(Scancode::J);
        go(&mut m, &[Nav::Bind(j)]);
        let mapping = m.controls.mapping();
        assert_eq!(mapping.keys(key::B), [Control::Key(Scancode::Z), j]);
        assert_eq!(
            mapping.controls(key::B),
            [Control::Label(Label::B), Control::Label(Label::X)]
        );
        // Delete (or Backspace) on J's chip takes it off.
        go(&mut m, &[Nav::Left]);
        assert!(shows(&mut m, "Enter or Delete remove J from B"));
        go(&mut m, &[Nav::Erase]);
        assert_eq!(
            m.controls.mapping().keys(key::B),
            [Control::Key(Scancode::Z)]
        );
        // Up to the tabs and left, back to the pads.
        go(&mut m, &[Nav::Up; 4]);
        go(&mut m, &[Nav::Left]);
        assert!(!m.remap.keyboard);
    }

    #[test]
    fn the_directions_are_one_row_of_whole_pads_or_four() {
        use crate::remap::DIRECTIONS;
        use runtime::io::key;
        let mut m = on_controls(Default::default());
        assert!(shows(&mut m, "Up / Down / Left / Right"));
        // Its "+ Add", past six rows, the switch, and two chips.
        go(&mut m, &[Nav::Down; 8]);
        go(&mut m, &[Nav::Right, Nav::Right]);
        assert!(shows(
            &mut m,
            "A add a stick or the D-pad to the directions"
        ));
        // A face button can't be four directions: said, nothing changed.
        go(
            &mut m,
            &[Nav::Ok, Nav::Bind(Control::Button(Button::North))],
        );
        assert!(shows(&mut m, "is one button"));
        assert!(!m.mapping_changed);
        // The right stick is, all four ways (taken from rewind).
        go(&mut m, &[Nav::Ok]);
        go(&mut m, &[Nav::Bind(Control::Stick(Axis::RightX, false))]);
        for bit in DIRECTIONS {
            assert_eq!(m.controls.mapping().controls(bit).len(), 3);
        }
        assert!(shows(
            &mut m,
            "right stick moved from Rewind (hold) to the directions"
        ));
        // A on the left stick's chip takes it off all four.
        go(&mut m, &[Nav::Left, Nav::Left, Nav::Ok]);
        assert!(
            !m.controls
                .mapping()
                .controls(key::UP)
                .contains(&Control::Stick(Axis::LeftY, false))
        );
        assert_eq!(m.controls.mapping().controls(key::UP).len(), 2);
        // The switch above them binds them separately; saved.
        let dir = std::env::temp_dir().join(format!("gbaa-menu-dirs-{}", std::process::id()));
        let path = dir.join("settings.conf");
        m.settings = Some(path.clone());
        go(&mut m, &[Nav::Up]);
        assert!(shows(&mut m, "Directions bound separately"));
        assert!(shows(&mut m, "A bind each direction separately"));
        go(&mut m, &[Nav::Ok]);
        assert!(m.remap.apart);
        assert!(!shows(&mut m, "Up / Down / Left / Right"));
        assert!(shows(&mut m, "Right") && shows(&mut m, "Down"));
        assert_eq!(
            crate::settings::load(Some(&path)).unwrap().directions_apart,
            Some(true)
        );
        std::fs::remove_dir_all(&dir).unwrap();
        // Apart, one direction takes a lone button, and then they can't
        // be together: the switch stays on, and says why.
        // Down to Right's row (its D-pad and right stick), and its
        // "+ Add".
        go(&mut m, &[Nav::Down; 4]);
        assert!(shows(&mut m, "A remove D-pad right from Right"));
        go(&mut m, &[Nav::Right, Nav::Right, Nav::Ok]);
        go(&mut m, &[Nav::Bind(Control::Button(Button::North))]);
        assert!(
            m.controls
                .mapping()
                .controls(key::RIGHT)
                .contains(&Control::Button(Button::North))
        );
        go(&mut m, &[Nav::Up; 4]);
        go(&mut m, &[Nav::Ok]);
        assert!(!m.remap.apart);
        assert!(shows(&mut m, "Right") && !shows(&mut m, "Up / Down / Left / Right"));
        assert!(shows(&mut m, "A say why the directions stay separate"));
        go(&mut m, &[Nav::Ok]);
        assert!(shows(&mut m, "so they stay separate until they match"));
        assert!(!m.remap.apart);
        // On Right's row, undo joins them again: the focus follows to
        // their row, not to whatever row took Right's place.
        go(&mut m, &[Nav::Down; 4]);
        assert!(shows(&mut m, "A remove D-pad right from Right"));
        go(&mut m, &[Nav::Undo]);
        assert!(shows(&mut m, "Up / Down / Left / Right"));
        assert!(shows(&mut m, "A remove D-pad from the directions"));
    }

    #[test]
    fn every_pad_button_can_be_bound() {
        use runtime::io::key;
        // Every button SDL has a name in the file, so a binding reads back:
        // a Steam Controller's grip was written as "?" and crashed it.
        for i in 0..64 {
            let Some(b) = Button::from_ll(sdl3::sys::gamepad::SDL_GamepadButton(i)) else {
                continue;
            };
            assert!(crate::gamepad::nameable(Control::Button(b)), "{b:?}");
        }
        let mut m = on_controls(Default::default());
        let grip = Control::Button(Button::LeftPaddle1);
        go(
            &mut m,
            &[Nav::Down, Nav::Right, Nav::Right, Nav::Ok, Nav::Bind(grip)],
        );
        assert!(m.controls.mapping().controls(key::A).contains(&grip));
        assert!(shows(&mut m, "Added left grip to A."));
    }

    #[test]
    fn the_deadzones_step_and_save_and_toasts_go() {
        let mut m = on_controls(Default::default());
        let dir = std::env::temp_dir().join(format!("gbaa-menu-dz-{}", std::process::id()));
        let path = dir.join("settings.conf");
        m.settings = Some(path.clone());
        // Past the shortcuts and the switch, the sticks' deadzone: right
        // steps it up.
        go(&mut m, &[Nav::Down; 16]);
        assert!(shows(&mut m, "Sticks") && shows(&mut m, "40%"));
        assert!(shows(
            &mut m,
            "change how far a stick goes before it counts"
        ));
        go(&mut m, &[Nav::Right]);
        assert_eq!(m.deadzones, [45, 20]);
        assert!(m.deadzones_changed);
        // The triggers' own, down, no lower than 5%.
        go(&mut m, &[Nav::Down]);
        go(&mut m, &[Nav::Left; 5]);
        assert_eq!(m.deadzones, [45, 5]);
        let saved = crate::settings::load(Some(&path)).unwrap();
        assert_eq!(
            (saved.deadzone, saved.trigger_deadzone),
            (Some(45), Some(5))
        );
        std::fs::remove_dir_all(&dir).unwrap();
        // The keyboard has none.
        go(&mut m, &[Nav::Up; 30]);
        go(&mut m, &[Nav::Right]);
        assert!(!shows(&mut m, "Triggers"));
        // A toast goes after a while.
        assert_eq!(m.page, Page::Remap);
        m.remap.say("Hello.".into(), false);
        pass(&mut m);
        assert!(shows(&mut m, "Hello."));
        let (text, undo, at) = m.remap.toast.take().unwrap();
        m.remap.toast = Some((text, undo, at - Duration::from_secs(60)));
        assert!(!shows(&mut m, "Hello."));
    }

    #[test]
    fn up_and_down_take_their_own_deadzone_apart() {
        use sdl3::gamepad::Axis;
        let mut m = on_controls(Default::default());
        let dir = std::env::temp_dir().join(format!("gbaa-menu-dza-{}", std::process::id()));
        let path = dir.join("settings.conf");
        m.settings = Some(path.clone());
        // Together by default: one sticks' row, both ways the same.
        go(&mut m, &[Nav::Down; 15]);
        assert!(shows(&mut m, "Up/down apart from left/right"));
        assert!(!shows(&mut m, "Sticks up/down"));
        assert_eq!(m.pad_deadzones(), crate::gamepad::Deadzones::default());
        // A splits them: two rows, each starting at the sticks' one.
        go(&mut m, &[Nav::Ok]);
        assert!(m.deadzone_axes_apart && m.deadzones_changed);
        assert!(shows(&mut m, "Sticks left/right") && shows(&mut m, "Sticks up/down"));
        // Up and down wider, left and right narrower, each saved apart.
        go(&mut m, &[Nav::Down, Nav::Left, Nav::Left]);
        assert!(shows(
            &mut m,
            "change how far a stick goes left or right before it counts"
        ));
        go(&mut m, &[Nav::Down, Nav::Right, Nav::Right, Nav::Right]);
        assert_eq!(m.axis_deadzones, [Some(30), Some(55)]);
        assert_eq!(m.deadzones, [40, 20]);
        let dz = m.pad_deadzones();
        assert_eq!((dz.of(Axis::LeftX), dz.of(Axis::LeftY)), (0.3, 0.55));
        assert_eq!(dz.of(Axis::TriggerLeft), 0.2);
        let saved = crate::settings::load(Some(&path)).unwrap();
        assert_eq!(
            (
                saved.deadzone_axes_apart,
                saved.deadzone_horizontal,
                saved.deadzone_vertical,
                saved.deadzone
            ),
            (Some(true), Some(30), Some(55), None)
        );
        // Together again: the one row, the pads back to it; the two kept.
        go(&mut m, &[Nav::Up, Nav::Up, Nav::Ok]);
        assert!(!m.deadzone_axes_apart);
        assert!(!shows(&mut m, "Sticks up/down"));
        assert_eq!(m.pad_deadzones(), crate::gamepad::Deadzones::default());
        assert_eq!(m.axis_deadzones, [Some(30), Some(55)]);
        let saved = crate::settings::load(Some(&path)).unwrap();
        assert_eq!(saved.deadzone_axes_apart, Some(false));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reset_asks_once_and_listening_gives_up() {
        use runtime::io::key;
        let mut m = on_controls(Default::default());
        go(&mut m, &[Nav::Down, Nav::Right, Nav::Right, Nav::Ok]);
        // Five seconds on, it's stopped listening.
        let (t, _) = m.remap.listen.unwrap();
        m.remap.listen = Some((t, Instant::now()));
        pass(&mut m);
        assert!(!m.listening());
        go(
            &mut m,
            &[Nav::Ok, Nav::Bind(Control::Button(Button::North))],
        );
        assert_eq!(m.controls.mapping().controls(key::A).len(), 3);
        // Down to the foot (every row reached), right to Reset.
        go(&mut m, &[Nav::Down; 20]);
        go(&mut m, &[Nav::Right]);
        assert!(shows(&mut m, "reset every game's controller to defaults"));
        go(&mut m, &[Nav::Ok]);
        assert!(shows(&mut m, "Press again"));
        assert_eq!(m.controls.mapping().controls(key::A).len(), 3);
        // A move away forgets it.
        go(&mut m, &[Nav::Left, Nav::Right, Nav::Ok]);
        assert_eq!(m.controls.mapping().controls(key::A).len(), 3);
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.controls.mapping(), crate::gamepad::Mapping::default());
    }

    /// Where this pass draws the menu, on a screen of `size` pixels at
    /// `ppp`: all it paints but the dimming.
    fn drawn_at(m: &mut Menu, size: (u32, u32), ppp: f32) -> egui::Rect {
        let mut out = m.run(size, ppp, 0.0);
        out.textures_delta.clear();
        let screen = egui::vec2(size.0 as f32, size.1 as f32) / ppp;
        out.shapes
            .iter()
            .map(|c| c.shape.visual_bounding_rect().intersect(c.clip_rect))
            .filter(|r| r.is_positive() && r.width() < screen.x - 1.0)
            .fold(egui::Rect::NOTHING, |a, r| a.union(r))
    }

    #[test]
    fn every_page_draws_the_same_box_at_once() {
        let drawn = |m: &mut Menu| drawn_at(m, (1280, 800), 2.0);
        let mut m = Menu::new(4.0, 100, Controls::default());
        pass(&mut m);
        go(
            &mut m,
            &[Nav::Toggle, Nav::Down, Nav::Down, Nav::Down, Nav::Down],
        );
        let top = drawn(&mut m);
        let same =
            |r: egui::Rect| (r.min - top.min).length() < 0.5 && (r.max - top.max).length() < 0.5;
        // The first pass of a page draws it where it settles, not a pass
        // late, and the same size as the last.
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.page, Page::Remap);
        // The Controls page is wider, the Deck's whole screen less its
        // edges, and as tall.
        let remap = drawn(&mut m);
        assert!(
            (remap.min.y - top.min.y).abs() <= 1.0
                && (remap.max.y - top.max.y).abs() < 0.5
                && (remap.min.x - EDGE).abs() <= 1.0,
            "{remap:?} vs {top:?}"
        );
        m.nav(Nav::Back);
        let back = drawn(&mut m);
        assert_eq!(m.page, Page::Main);
        assert!(same(back), "{back:?} vs {top:?}");
        go(&mut m, &[Nav::Down, Nav::Down, Nav::Ok]);
        assert_eq!(m.page, Page::Scripts);
        let scripts = drawn(&mut m);
        assert!(same(scripts), "{scripts:?} vs {top:?}");
    }

    #[test]
    fn a_small_window_fits_the_menu_and_scrolls_to_every_row() {
        // A 2x window, 480x320, at the least zoom.
        let size = (480, 320);
        let mut m = Menu::new(4.0, 100, Controls::default());
        let run = |m: &mut Menu| m.run(size, 1.0, 0.0).textures_delta.clear();
        run(&mut m);
        m.nav(Nav::Toggle);
        // Its twelve focus stops, Undo load state greyed.
        for i in 0..12 {
            if i > 0 {
                m.nav(Nav::Down);
            }
            for _ in 0..3 {
                run(&mut m);
            }
            let r = drawn_at(&mut m, size, 1.0);
            assert!(r.min.y >= 0.0 && r.max.y <= 320.0, "{r:?}");
            let id = m.ctx.memory(|mem| mem.focused()).unwrap();
            let row = m.ctx.read_response(id).unwrap();
            // Clipped by the scrolled list's edge, it'd sense less.
            assert_eq!(row.interact_rect.height(), row.rect.height());
        }
        // Quit, the last row, in view: chosen, it quits.
        m.nav(Nav::Ok);
        run(&mut m);
        assert!(m.quit);
    }

    #[test]
    fn the_focused_row_is_scrolled_into_view() {
        let mut m = on_controls(Default::default());
        // The last shortcut's row, at the list's foot: scrolled to, it's
        // drawn inside the list.
        go(&mut m, &[Nav::Down; 14]);
        assert!(m.remap.focus.0 > 10);
        for _ in 0..3 {
            pass(&mut m);
        }
        let mut out = m.run((1280, 800), 2.0, 0.0);
        out.textures_delta.clear();
        let shown = out.shapes.iter().any(|c| match &c.shape {
            egui::Shape::Text(t) => {
                t.galley.text() == "Script console"
                    && c.clip_rect.contains_rect(t.visual_bounding_rect())
            }
            _ => false,
        });
        assert!(shown);
    }

    #[test]
    fn netplay_s_report_shows_under_the_main_page() {
        // The text this pass draws.
        fn texts(m: &mut Menu) -> Vec<String> {
            let mut out = m.run((1280, 800), 2.0, 0.0);
            out.textures_delta.clear();
            out.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect()
        }
        let line = "netplay link: round trip 41 ms (least 30, most 90)";
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.report = vec![line.into()];
        // Shut, it shows nothing.
        assert!(!texts(&mut m).iter().any(|t| t == line));
        m.nav(Nav::Toggle);
        assert!(texts(&mut m).iter().any(|t| t == line));
        // Not over the remap screen.
        go(
            &mut m,
            &[Nav::Down, Nav::Down, Nav::Down, Nav::Down, Nav::Ok],
        );
        assert_eq!(m.page, Page::Remap);
        assert!(!texts(&mut m).iter().any(|t| t == line));
    }

    #[test]
    fn undo_load_state_is_greyed_until_theres_a_load() {
        // Nothing to undo: down four times from Paused skips it, to
        // Controls.
        let mut m = Menu::new(0.0, 100, Controls::default());
        m.nav(Nav::Toggle);
        pass(&mut m);
        for _ in 0..4 {
            m.nav(Nav::Down);
            pass(&mut m);
        }
        m.nav(Nav::Ok);
        pass(&mut m);
        assert!(m.open && m.page == Page::Remap && !m.undo);
        m.nav(Nav::Back);
        m.nav(Nav::Back);
        assert!(!m.open);
        // A load to undo: the fifth row asks for it, and closes the menu.
        m.undoable = true;
        m.nav(Nav::Toggle);
        pass(&mut m);
        for _ in 0..4 {
            m.nav(Nav::Down);
            pass(&mut m);
        }
        m.nav(Nav::Ok);
        pass(&mut m);
        assert!(!m.open && m.undo && !m.undoable && !m.quit);
    }

    #[test]
    fn the_scripts_page_loads_and_unloads() {
        let dir = std::env::temp_dir().join(format!("gbaa-menu-scripts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.lua"), dir.join("b.lua"));
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.console.view.loaded = vec![(1, a.clone())];
        pass(&mut m);
        // Paused, Fast-forward, Volume, Mute, Controls (Undo greyed),
        // Fullscreen, Scripts.
        go(
            &mut m,
            &[
                Nav::Toggle,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
            ],
        );
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.page, Page::Scripts);
        // Console, Type a command, Unload a.lua, Load b.lua (a.lua is
        // loaded already), Back.
        assert_eq!(m.files, [a.clone(), b.clone()]);
        // The page's first row takes the focus at the pass after it turns.
        pass(&mut m);
        go(&mut m, &[Nav::Ok]);
        assert!(m.console.shown && m.open);
        go(&mut m, &[Nav::Down, Nav::Down, Nav::Down, Nav::Ok]);
        assert_eq!(m.console.cmds, [ScriptCmd::Load(b.clone())]);
        assert!(!m.open);
        // Again: Unload a.lua.
        m.console.cmds.clear();
        go(
            &mut m,
            &[
                Nav::Toggle,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Ok,
            ],
        );
        assert_eq!(m.page, Page::Scripts);
        pass(&mut m);
        go(&mut m, &[Nav::Down, Nav::Down, Nav::Ok]);
        assert_eq!(m.console.cmds, [ScriptCmd::Unload(1)]);
        assert!(!m.open);
        // Type a command: the menu shuts and the line takes the keyboard.
        go(
            &mut m,
            &[
                Nav::Toggle,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Down,
                Nav::Ok,
            ],
        );
        pass(&mut m);
        go(&mut m, &[Nav::Down, Nav::Ok]);
        pass(&mut m);
        assert!(!m.open && m.console.typing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typing_at_the_prompt_runs_it() {
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.console.set_shown(true);
        m.console.focus_line = true;
        pass(&mut m);
        assert!(m.console.typing);
        m.typed(egui::Event::Text("HELP()".into()));
        pass(&mut m);
        for pressed in [true, false] {
            m.typed(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        pass(&mut m);
        pass(&mut m);
        assert_eq!(m.console.cmds, [ScriptCmd::Eval("HELP()".into())]);
        assert!(m.console.typing);
    }

    #[test]
    fn volumes_step_and_stop_at_the_ends() {
        assert_eq!(step_volume(100, 1), 110);
        assert_eq!(step_volume(100, -1), 90);
        assert_eq!(step_volume(200, 1), 200);
        assert_eq!(step_volume(0, -1), 0);
        assert_eq!(step_volume(195, 1), 200);
        // --volume 55 isn't a step: to the next one either way.
        assert_eq!(step_volume(55, 1), 60);
        assert_eq!(step_volume(55, -1), 50);
        assert_eq!(step_volume(5, -1), 0);
    }

    #[test]
    fn the_volume_keys_step_mute_save_and_toast() {
        let dir = std::env::temp_dir().join(format!("gbaa-menu-keys-{}", std::process::id()));
        let path = dir.join("settings.conf");
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.settings = Some(path.clone());
        let now = Instant::now();
        assert!(!m.drawn());
        m.sound_key(SoundKey::Up, now);
        assert_eq!(m.volume, 110);
        assert!(std::mem::take(&mut m.sound_changed));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "volume = 110\n");
        assert_eq!(m.toast.as_ref().unwrap().0, "Volume 110%");
        assert!(m.drawn() && !m.open);
        assert_eq!(m.toast_until(now + TOAST_TIME), None);
        // Muted (not saved); a step unmutes it.
        m.sound_key(SoundKey::Mute, now);
        assert!(m.muted && std::mem::take(&mut m.sound_changed));
        assert_eq!(m.toast.as_ref().unwrap().0, "Muted");
        m.sound_key(SoundKey::Down, now);
        assert!(!m.muted && m.sound_changed);
        assert_eq!(m.volume, 100);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "volume = 100\n");
        // The toast is drawn over the game, and goes as the menu opens.
        pass(&mut m);
        go(&mut m, &[Nav::Toggle]);
        assert!(m.toast.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_volume_and_mute_rows_set_and_save_it() {
        let dir = std::env::temp_dir().join(format!("gbaa-menu-volume-{}", std::process::id()));
        let path = dir.join("settings.conf");
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.settings = Some(path.clone());
        pass(&mut m);
        // Down to Volume, right twice: 120%, saved.
        go(&mut m, &[Nav::Toggle, Nav::Down, Nav::Down, Nav::Right]);
        assert_eq!(
            (m.volume, std::mem::take(&mut m.sound_changed)),
            (110, true)
        );
        go(&mut m, &[Nav::Right]);
        assert_eq!(m.volume, 120);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(crate::settings::parse(&text).unwrap().volume, Some(120));
        // Left, held, stops at 0.
        for _ in 0..14 {
            go(&mut m, &[Nav::Left]);
        }
        assert_eq!(m.volume, 0);
        m.sound_changed = false;
        // At the end already: no change to send.
        go(&mut m, &[Nav::Left]);
        assert!(!m.sound_changed);
        // Down to Mute, A: muted (not saved); left: not.
        go(&mut m, &[Nav::Down, Nav::Ok]);
        assert!(m.muted && m.sound_changed && m.open);
        go(&mut m, &[Nav::Left]);
        assert!(!m.muted);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "volume = 0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_resume_row_turns_it_on_and_saves_it() {
        let dir = std::env::temp_dir().join(format!("gbaa-menu-resume-{}", std::process::id()));
        let path = dir.join("settings.conf");
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.settings = Some(path.clone());
        // Off by default.
        assert!(!m.resume);
        pass(&mut m);
        // Down eight times to it (past Undo load state, greyed), A.
        go(&mut m, &[Nav::Toggle]);
        go(&mut m, &[Nav::Down; 8]);
        go(&mut m, &[Nav::Ok]);
        assert!(m.resume && m.open);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "resume = on\n");
        go(&mut m, &[Nav::Right]);
        assert!(!m.resume);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(crate::settings::parse(&text).unwrap().resume, Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_save_folders_page_picks_folders_on_a_pad() {
        let dir = std::env::temp_dir().join(format!("gbaa-menu-saves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let start = dir.join("start");
        std::fs::create_dir_all(start.join("sub")).unwrap();
        std::fs::create_dir_all(start.join(".hidden")).unwrap();
        let conf = dir.join("config/games/game.conf");
        let settings = dir.join("config/settings.conf");
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.settings = Some(settings.clone());
        m.saves = saves::SaveFolders {
            game: "game".into(),
            conf: Some(conf.clone()),
            now: Some(dir.join("now")),
            per_game: Some(start.clone()),
            data: Some(dir.join("data")),
            ..Default::default()
        };
        pass(&mut m);
        // Down nine times to Save folders (past Undo load state, greyed).
        go(&mut m, &[Nav::Toggle]);
        go(&mut m, &[Nav::Down; 9]);
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.page, Page::Saves);
        assert!(shows(&mut m, "This run's:"));
        // The game's row opens the picker where it's set: Use this
        // folder, back to the default, up, home, then its folders.
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.page, Page::Folder);
        assert!(shows(&mut m, "sub") && !shows(&mut m, ".hidden"));
        let to_sub = 3 + usize::from(saves::home().is_some());
        go(&mut m, &vec![Nav::Down; to_sub]);
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.picker.as_ref().unwrap().dir, start.join("sub"));
        // Into it, the focus is on Use this folder.
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.page, Page::Saves);
        assert_eq!(m.saves.per_game, Some(start.join("sub")));
        let text = std::fs::read_to_string(&conf).unwrap();
        let c = crate::game_dir::GameConf::parse(&text).unwrap();
        assert_eq!(c.save_dir, Some(start.join("sub")));
        assert_eq!(c.moved_from, Some(dir.join("now")));
        assert!(shows(&mut m, "From the next launch"));
        assert!(shows(&mut m, "counts from the next launch"));
        // Back to the default.
        go(&mut m, &[Nav::Ok, Nav::Down, Nav::Ok]);
        assert_eq!(m.saves.per_game, None);
        assert!(shows(&mut m, "with all saves"));
        // B goes back a page at a time.
        go(&mut m, &[Nav::Ok, Nav::Back]);
        assert_eq!(m.page, Page::Saves);
        go(&mut m, &[Nav::Back]);
        assert_eq!(m.page, Page::Main);
        // With the launcher in charge the game's row is greyed and passed
        // over: A on the page picks every game's folder.
        m.saves.launcher = true;
        go(&mut m, &[Nav::Back, Nav::Toggle]);
        go(&mut m, &[Nav::Down; 9]);
        go(&mut m, &[Nav::Ok]);
        assert!(shows(&mut m, "set by the launcher"));
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.picker.as_ref().unwrap().target, saves::Target::All);
        go(&mut m, &[Nav::Ok]);
        let dir_now = m.saves.global.clone();
        assert!(dir_now.is_some());
        let s = crate::settings::parse(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!((s.save_dir, s.previous_save_dir), (dir_now, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_global_picked_twice_moves_from_the_launch_folder() {
        let dir = std::env::temp_dir().join(format!("gbaa-menu-global-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = dir.join("config/settings.conf");
        let mut m = Menu::new(4.0, 100, Controls::default());
        m.settings = Some(settings.clone());
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
        m.saves = saves::SaveFolders {
            game: "game".into(),
            global: Some(a.clone()),
            global_at_launch: Some(a.clone()),
            ..Default::default()
        };
        // A mis-pick, then the folder meant: the next launch still moves
        // the files from where this run has them.
        m.set_folder(saves::Target::All, Some(b));
        m.set_folder(saves::Target::All, Some(c.clone()));
        let s = crate::settings::parse(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!((s.save_dir, s.previous_save_dir), (Some(c), Some(a)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_folders_keep_their_end() {
        let p = std::path::Path::new("/run/media/deck/sdcard/Emulation/saves/gbaa");
        let s = saves::short(p);
        assert!(s.starts_with('…') && s.ends_with("saves/gbaa"), "{s}");
        assert_eq!(saves::short(std::path::Path::new("/a/b")), "/a/b");
    }

    #[test]
    fn muting_keeps_the_device_pacing() {
        use crate::audio::{DEFAULT_GAIN, Player};
        use crate::platform::set_sound;
        let mut p = Player::simulated(48000);
        set_sound(&mut p, 150, false);
        assert_eq!(p.gain(), DEFAULT_GAIN * 1.5);
        // Queued sound at the old level is dropped as the mute starts.
        p.queue().push(&[[0.5; 2]; 100]);
        set_sound(&mut p, 150, true);
        assert_eq!(p.gain(), 0.0);
        assert!(p.queue().is_empty());
        // Muted frames still fill the queue (with silence), so the device
        // goes on pacing the game.
        p.end_frame();
        p.flush();
        assert!(!p.queue().is_empty());
        set_sound(&mut p, 150, false);
        assert_eq!(p.gain(), DEFAULT_GAIN * 1.5);
    }

    #[test]
    fn deck_text_is_zoomed() {
        assert_eq!(pixels_per_point(5), 2.0);
        assert_eq!(pixels_per_point(1), 1.0);
    }

    #[test]
    fn a_field_is_typed_on_a_pad_or_a_keyboard() {
        let mut e = Edit::new(Field::Code, "");
        // Up on the slot after the last adds a letter; up and down step it.
        e.step(1);
        e.step(1);
        e.step(-1);
        assert_eq!(e.kept(), "A");
        e.go(1);
        e.step(-1);
        assert_eq!(e.kept(), "A9");
        assert_eq!(e.shown(), "A[9]____");
        // Typing goes in at the cursor, in capitals, past what isn't in
        // the code's alphabet (no O, 0, I or 1).
        e.typed("bc0d-ef");
        assert_eq!(e.kept(), "ABCDEF");
        assert_eq!(e.shown(), "ABCDEF");
        // Full: never longer; a letter moved back to is typed over.
        e.typed("g");
        assert_eq!(e.kept(), "ABCDEF");
        e.go(-1);
        e.typed("g");
        assert_eq!(e.kept(), "ABCDEG");
        e.erase();
        e.erase();
        assert_eq!(e.kept(), "ABCD");
        let mut s = Edit::new(Field::Server, "gbaa.example:7374");
        assert_eq!(s.cursor, 17);
        s.erase();
        s.typed("5 ");
        assert_eq!(s.kept(), "gbaa.example:7375");
    }

    #[test]
    fn the_netplay_page_hosts_joins_and_leaves() {
        let mut m = Menu::new(4.0, 100, Controls::default());
        assert_eq!(m.server, room::DEFAULT_SERVER);
        pass(&mut m);
        go(&mut m, &[Nav::Toggle]);
        go(&mut m, &[Nav::Down; NETPLAY_ROW - 1]);
        go(&mut m, &[Nav::Ok]);
        assert_eq!(m.page, Page::Netplay);
        pass(&mut m);
        // Server, Players, Host a room, Room code, Join, Back. Join with no
        // code asks for one.
        go(
            &mut m,
            &[Nav::Down, Nav::Down, Nav::Down, Nav::Down, Nav::Ok],
        );
        assert_eq!(m.netplay_ask, None);
        assert!(m.saved.as_deref().unwrap().contains("code"));
        // The code, typed on a pad: A on its row, up for each new letter.
        go(&mut m, &[Nav::Up, Nav::Ok]);
        assert!(m.editing());
        go(&mut m, &[Nav::Up, Nav::Up, Nav::Right, Nav::Down]);
        assert_eq!(m.edit.as_ref().unwrap().shown(), "B[9]____");
        // The rest on the keyboard; Enter keeps it.
        go(&mut m, &[Nav::Right]);
        m.edit_text("cdef");
        assert_eq!(m.edit.as_ref().unwrap().shown(), "B9CDEF");
        go(&mut m, &[Nav::Ok]);
        assert!(!m.editing());
        assert_eq!(m.code, "B9CDEF");
        go(&mut m, &[Nav::Down, Nav::Ok]);
        assert_eq!(
            m.netplay_ask.take(),
            Some(room::Ask {
                server: room::DEFAULT_SERVER.into(),
                role: room::Role::Join(*b"B9CDEF"),
                players: 0,
            })
        );
        // Hosting: the code shows big while it waits; the session begun
        // closes the menu.
        go(&mut m, &[Nav::Up, Nav::Up, Nav::Ok]);
        let ask = m.netplay_ask.take().unwrap();
        assert_eq!((ask.role, ask.players), (room::Role::Host, 2));
        // Players steps 2, 3, 4 and round, A as right; left goes back.
        go(
            &mut m,
            &[
                Nav::Up,
                Nav::Ok,
                Nav::Ok,
                Nav::Ok,
                Nav::Left,
                Nav::Down,
                Nav::Ok,
            ],
        );
        let ask = m.netplay_ask.take().unwrap();
        assert_eq!((ask.role, ask.players), (room::Role::Host, 4));
        m.set_netplay(room::State::Waiting {
            code: "QWERTY".into(),
            filled: 3,
            seats: 4,
        });
        pass(&mut m);
        assert!(m.open);
        m.set_netplay(room::State::Playing {
            code: "QWERTY".into(),
            seat: 0,
            seats: 2,
            lines: vec!["Round trip 40 ms".into()],
            left: None,
            quiet: false,
            desync: None,
        });
        assert!(!m.open);
        assert!(m.toast.is_none());
        // The games go out of sync: a toast says so over the game, once,
        // and the Netplay page's lines with it.
        let apart = room::State::Playing {
            code: "QWERTY".into(),
            seat: 0,
            seats: 2,
            lines: vec![format!("{}: your games may differ", room::desync_note(240))],
            left: None,
            quiet: false,
            desync: Some(240),
        };
        m.set_netplay(apart.clone());
        assert_eq!(
            m.toast.as_ref().unwrap().0,
            "Out of sync with the other player since frame 240"
        );
        assert_eq!(Mark::of(&apart), Some(Mark::Desynced));
        m.toast = None;
        m.set_netplay(apart);
        assert!(m.toast.is_none());
        // The other player leaves: a toast says so over the game, once.
        let alone = room::State::Playing {
            code: "QWERTY".into(),
            seat: 0,
            seats: 2,
            lines: vec!["Player 2 left the room.".into()],
            left: Some("Player 2 left the room".into()),
            quiet: false,
            desync: Some(240),
        };
        m.set_netplay(alone.clone());
        assert_eq!(m.toast.as_ref().unwrap().0, "Player 2 left the room");
        assert!(m.drawn());
        m.toast = None;
        m.set_netplay(alone);
        assert!(m.toast.is_none());
        // Leave the room, then Back.
        go(&mut m, &[Nav::Toggle]);
        go(&mut m, &[Nav::Down; NETPLAY_ROW - 1]);
        go(&mut m, &[Nav::Ok]);
        pass(&mut m);
        go(&mut m, &[Nav::Ok]);
        assert!(m.leave);
    }
}
