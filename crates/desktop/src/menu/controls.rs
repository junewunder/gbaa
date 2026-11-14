//! The Controls page (docs/design/remap-design.md): two tabs, the pads'
//! controls and the keyboard's, the page's top row; a switch for where
//! changes go (every game, or only this one); a picture of the GBA
//! labelled with what presses each button; and a row for each button, its
//! controls as chips. A chip's ×, or A (Enter, Delete, Backspace) on it,
//! takes that one control off; "+ Add" listens for the next control (a
//! dialog with a countdown) and adds it. A control another button had
//! moves, said in a toast that undoes it. The four directions are one row
//! of whole pads (the D-pad, a stick, four keys) unless the switch above
//! them binds them separately. The pads' tab ends with the sticks' and
//! triggers' deadzones; a switch gives the sticks' up and down a deadzone
//! of their own, apart from left and right.
//!
//! The page keeps its own focus, a row and a chip in it, moved by the
//! menu's [`Nav`]s: egui's arrow-key focus moves by geometry, erratic over
//! a grid of chips of different widths.

use super::{Menu, Nav, ROW_HEIGHT, footer};
use crate::gamepad::{self, Control, Label};
use crate::remap::{Controls, DIRECTIONS, Four};
use egui::{Color32, Rect, Sense, Vec2, vec2};
use runtime::io::key;
use sdl3::gamepad::{Axis, Button};
use sdl3::keyboard::Scancode;
use std::time::{Duration, Instant};

/// How long "+ Add" listens before it gives up.
pub const LISTEN_TIME: Duration = Duration::from_secs(5);

/// How long a toast stays.
const TOAST_TIME: Duration = Duration::from_secs(4);

/// A deadzone's step, in percent.
const DEADZONE_STEP: u32 = 5;

/// The page at least this wide (in points) has room for the GBA beside
/// the list.
pub const PICTURE_ROOM: f32 = 540.0;

/// The Controls page's state.
#[derive(Debug, Default)]
pub struct Page {
    /// Changes go to this game's section, not every game's.
    pub this_game: bool,
    /// The keyboard's tab, not the pads'.
    pub keyboard: bool,
    /// The directions are four rows, not one of whole pads.
    pub apart: bool,
    /// A native program's window ([`super::Menu::native`]): the shortcuts
    /// it has nothing for ([`NOT_NATIVE`]) are left out.
    pub native: bool,
    /// The focused row (of [`Page::rows`]) and the item in it.
    pub(super) focus: (usize, usize),
    /// The focus last scrolled into view.
    scrolled: Option<(usize, usize)>,
    /// Listening for a control to add, and until when.
    pub listen: Option<(Target, Instant)>,
    /// What the last change did, whether it can be undone, and when it
    /// was said.
    pub(super) toast: Option<(String, bool, Instant)>,
    /// The file before the last change.
    undo: Option<crate::remap::Config>,
    /// Reset was pressed once: again resets.
    confirm_reset: bool,
}

/// A deadzone the page steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dz {
    /// The sticks', both ways.
    Sticks,
    /// The sticks' left and right, while apart.
    Horizontal,
    /// The sticks' up and down, while apart.
    Vertical,
    Triggers,
}

impl Dz {
    /// Its key in the settings file.
    fn key(self) -> &'static str {
        match self {
            Dz::Sticks => "deadzone",
            Dz::Horizontal => "deadzone-horizontal",
            Dz::Vertical => "deadzone-vertical",
            Dz::Triggers => "trigger-deadzone",
        }
    }
}

/// What a row binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A button (a bit of [`gamepad::BUTTONS`]).
    Button(u16),
    /// The four directions together, by whole pads.
    Directions,
}

/// One control on a row: a pad's or a key, or a whole pad of directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chip {
    One(Control),
    Four(&'static Four),
}

/// A focus stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    /// The tabs: the controller's, or the keyboard's.
    Tab(bool),
    /// The scope switch's halves: every game, or this game.
    Scope(bool),
    Chip(Target, Chip),
    Add(Target),
    /// Drop this game's line, back to every game's.
    EveryGame(Target),
    /// The directions together or apart.
    Directions,
    /// A deadzone.
    Deadzone(Dz),
    /// The sticks' two ways' deadzones together or apart.
    DeadzoneAxes,
    Reset,
}

/// What a row shows.
#[derive(Clone, Debug)]
enum Kind {
    Tabs,
    Scope,
    Heading(&'static str),
    Binds {
        target: Target,
        label: String,
        /// This game changed it (shown in this-game mode).
        here: bool,
    },
    /// The directions bound separately, or not.
    Directions,
    /// A deadzone.
    Deadzone(Dz),
    /// The sticks' two ways' deadzones apart, or not.
    DeadzoneAxes,
    Foot,
}

#[derive(Clone, Debug)]
struct Row {
    kind: Kind,
    items: Vec<Item>,
}

/// The GBA's buttons in the page's order, then the shortcuts', with
/// their names on it.
const GBA_ROWS: [(u16, &str); 6] = [
    (key::A, "A"),
    (key::B, "B"),
    (key::L, "L"),
    (key::R, "R"),
    (key::START, "Start"),
    (key::SELECT, "Select"),
];
const DIRECTION_NAMES: [&str; 4] = ["Up", "Down", "Left", "Right"];
const SHORTCUTS: [(u16, &str); 6] = [
    (gamepad::FAST, "Fast-forward (hold)"),
    (gamepad::FAST_TOGGLE, "Fast-forward on/off"),
    (gamepad::SAVE_STATE, "Save state"),
    (gamepad::LOAD_STATE, "Load state"),
    (gamepad::REWIND, "Rewind (hold)"),
    (gamepad::CONSOLE, "Script console"),
];
/// The shortcuts a native program's window has nothing for: no save
/// states, rewind or scripts.
const NOT_NATIVE: [u16; 4] = [
    gamepad::SAVE_STATE,
    gamepad::LOAD_STATE,
    gamepad::REWIND,
    gamepad::CONSOLE,
];

/// A button's name on the page.
pub fn button_name(bit: u16) -> &'static str {
    let named = GBA_ROWS.iter().chain(&SHORTCUTS).find(|(b, _)| *b == bit);
    let dir = DIRECTIONS.iter().position(|&d| d == bit);
    named
        .map(|(_, n)| *n)
        .or(dir.map(|i| DIRECTION_NAMES[i]))
        .unwrap_or("?")
}

fn target_name(t: Target) -> &'static str {
    match t {
        Target::Button(bit) => button_name(bit),
        Target::Directions => "the directions",
    }
}

/// A control's name on the page: as printed on a pad or a key.
pub fn control_name(c: Control) -> String {
    let stick = |a: Axis| match a {
        Axis::LeftX | Axis::LeftY => "left stick",
        Axis::RightX | Axis::RightY => "right stick",
        _ => "",
    };
    match c {
        Control::Label(l) => format!(
            "{} button",
            match l {
                Label::A => "A",
                Label::B => "B",
                Label::X => "X",
                Label::Y => "Y",
            }
        ),
        Control::Button(b) => match b {
            Button::South => "bottom face button",
            Button::East => "right face button",
            Button::West => "left face button",
            Button::North => "top face button",
            Button::LeftShoulder => "LB",
            Button::RightShoulder => "RB",
            Button::Back => "Select",
            Button::Start => "Start",
            Button::Guide => "Guide",
            Button::LeftStick => "left stick click",
            Button::RightStick => "right stick click",
            Button::DPadUp => "D-pad up",
            Button::DPadDown => "D-pad down",
            Button::DPadLeft => "D-pad left",
            Button::DPadRight => "D-pad right",
            Button::LeftPaddle1 => "left grip",
            Button::RightPaddle1 => "right grip",
            Button::LeftPaddle2 => "left lower grip",
            Button::RightPaddle2 => "right lower grip",
            Button::Touchpad => "touchpad click",
            _ => return gamepad::control_name(c),
        }
        .into(),
        Control::Stick(Axis::TriggerLeft, _) => "LT".into(),
        Control::Stick(Axis::TriggerRight, _) => "RT".into(),
        Control::Stick(a, plus) => {
            let way = match (a, plus) {
                (Axis::LeftX | Axis::RightX, false) => "left",
                (Axis::LeftX | Axis::RightX, true) => "right",
                (_, false) => "up",
                (_, true) => "down",
            };
            format!("{} {way}", stick(a))
        }
        Control::Key(sc) => match sc {
            Scancode::Return => "Enter".into(),
            sc => sc.name().to_owned(),
        },
    }
}

fn chip_name(c: Chip) -> String {
    match c {
        Chip::One(c) => control_name(c),
        Chip::Four(f) => f.name.into(),
    }
}

/// The page's look.
const CHIP: Color32 = Color32::from_rgb(52, 60, 74);
const CHIP_HOVER: Color32 = Color32::from_rgb(66, 76, 94);
const FOCUS: Color32 = Color32::from_rgb(170, 210, 255);
const ROW_FOCUS: Color32 = Color32::from_rgb(34, 52, 82);
const RED: Color32 = Color32::from_rgb(235, 90, 90);
const PURPLE: Color32 = Color32::from_rgb(120, 80, 190);
const DIM: Color32 = Color32::from_rgb(120, 126, 136);
const BLUE: Color32 = Color32::from_rgb(38, 100, 180);
const BODY: Color32 = Color32::from_rgb(92, 70, 160);
/// A row's label column.
const LABEL_WIDTH: f32 = 150.0;
/// The GBA picture's column.
const PICTURE_WIDTH: f32 = 196.0;

impl Page {
    /// Leave the page, or come back to it: nothing pending.
    pub fn reset_state(&mut self) {
        self.listen = None;
        self.toast = None;
        self.undo = None;
        self.confirm_reset = false;
        self.scrolled = None;
        self.focus = (0, usize::from(self.keyboard));
    }

    /// When the toast goes, if there is one.
    pub fn toast_until(&self) -> Option<Instant> {
        self.toast.as_ref().map(|t| t.2 + TOAST_TIME)
    }

    pub(super) fn say(&mut self, text: String, undoable: bool) {
        self.toast = Some((text, undoable, Instant::now()));
    }

    /// The page's rows, as `controls` binds them now, the sticks'
    /// deadzones one or (`axes_apart`) two.
    fn rows(&self, controls: &Controls, axes_apart: bool) -> Vec<Row> {
        let keys = self.keyboard;
        let view = controls.view(self.this_game);
        let row = |kind, items| Row { kind, items };
        let mut rows = vec![
            row(Kind::Tabs, vec![Item::Tab(false), Item::Tab(true)]),
            row(Kind::Scope, vec![Item::Scope(false), Item::Scope(true)]),
        ];
        let binds = |rows: &mut Vec<Row>, target: Target, label: &str, chips: Vec<Chip>| {
            let here = self.this_game
                && match target {
                    Target::Button(bit) => controls.changed_here(bit, keys),
                    Target::Directions => {
                        DIRECTIONS.iter().any(|&d| controls.changed_here(d, keys))
                    }
                };
            let mut items: Vec<_> = chips.into_iter().map(|c| Item::Chip(target, c)).collect();
            items.push(Item::Add(target));
            if here {
                items.push(Item::EveryGame(target));
            }
            rows.push(Row {
                kind: Kind::Binds {
                    target,
                    label: label.into(),
                    here,
                },
                items,
            });
        };
        let ones = |bit: u16| view.of(bit, keys).into_iter().map(Chip::One).collect();
        rows.push(Row {
            kind: Kind::Heading("GAME BOY BUTTONS"),
            items: Vec::new(),
        });
        for (bit, name) in GBA_ROWS {
            binds(&mut rows, Target::Button(bit), name, ones(bit));
        }
        rows.push(row(Kind::Directions, vec![Item::Directions]));
        match Four::split(&view, keys).filter(|_| !self.apart) {
            Some(fours) => binds(
                &mut rows,
                Target::Directions,
                "Up / Down / Left / Right",
                fours.into_iter().map(Chip::Four).collect(),
            ),
            None => {
                for (&bit, name) in DIRECTIONS.iter().zip(DIRECTION_NAMES) {
                    binds(&mut rows, Target::Button(bit), name, ones(bit));
                }
            }
        }
        rows.push(Row {
            kind: Kind::Heading("SHORTCUTS"),
            items: Vec::new(),
        });
        for (bit, name) in SHORTCUTS {
            if self.native && NOT_NATIVE.contains(&bit) {
                continue;
            }
            binds(&mut rows, Target::Button(bit), name, ones(bit));
        }
        if !keys {
            rows.push(row(Kind::Heading("DEADZONES"), Vec::new()));
            rows.push(row(Kind::DeadzoneAxes, vec![Item::DeadzoneAxes]));
            let sticks: &[Dz] = if axes_apart {
                &[Dz::Horizontal, Dz::Vertical]
            } else {
                &[Dz::Sticks]
            };
            for &dz in sticks.iter().chain(&[Dz::Triggers]) {
                rows.push(row(Kind::Deadzone(dz), vec![Item::Deadzone(dz)]));
            }
        }
        rows.push(row(Kind::Foot, vec![Item::Reset]));
        rows
    }

    /// The focus kept to a stop that's there.
    fn clamp(&mut self, rows: &[Row]) {
        let (mut r, i) = self.focus;
        r = r.min(rows.len() - 1);
        if rows[r].items.is_empty() {
            r = (r..rows.len())
                .chain((0..r).rev())
                .find(|&r| !rows[r].items.is_empty())
                .unwrap_or(0);
        }
        self.focus = (r, i.min(rows[r].items.len().saturating_sub(1)));
    }

    fn focused(&self, rows: &[Row]) -> Option<Item> {
        rows.get(self.focus.0)?.items.get(self.focus.1).copied()
    }

    /// Whether the directions as asked (together) can't be shown so.
    fn forced_apart(&self, controls: &Controls) -> bool {
        !self.apart && Four::split(&controls.view(self.this_game), self.keyboard).is_none()
    }

    /// Whether the directions are shown each on its own row.
    fn shown_apart(&self, controls: &Controls) -> bool {
        self.apart || self.forced_apart(controls)
    }

    /// What A does on the focused item, for the prompt bar.
    fn prompt(&self, item: Option<Item>, controls: &Controls, axes_apart: bool) -> String {
        let a = if self.keyboard { "Enter" } else { "A" };
        let keys = self.keyboard;
        let view = controls.view(self.this_game);
        let a = match item {
            Some(Item::Chip(..)) if keys => "Enter or Delete",
            Some(Item::Deadzone(_)) => "Left/Right",
            _ => a,
        };
        let what = match item {
            Some(Item::Tab(k)) if k == keys => "is this tab; left or right for the other".into(),
            Some(Item::Tab(true)) => "show the keyboard's controls".into(),
            Some(Item::Tab(false)) => "show the controller's controls".into(),
            Some(Item::Deadzone(Dz::Sticks)) => {
                "change how far a stick goes before it counts".into()
            }
            Some(Item::Deadzone(Dz::Horizontal)) => {
                "change how far a stick goes left or right before it counts".into()
            }
            Some(Item::Deadzone(Dz::Vertical)) => {
                "change how far a stick goes up or down before it counts".into()
            }
            Some(Item::Deadzone(Dz::Triggers)) => {
                "change how far a trigger goes before it counts".into()
            }
            Some(Item::DeadzoneAxes) if axes_apart => {
                "give the sticks one deadzone both ways".into()
            }
            Some(Item::DeadzoneAxes) => {
                "give a stick's up and down a deadzone apart from left and right".into()
            }
            Some(Item::Scope(false)) => "make changes for every game".into(),
            Some(Item::Scope(true)) => "make changes for only this game".into(),
            Some(Item::Chip(t, c)) => format!("remove {} from {}", chip_name(c), target_name(t)),
            Some(Item::Add(Target::Directions)) => {
                if keys {
                    "add four keys to the directions".into()
                } else {
                    "add a stick or the D-pad to the directions".into()
                }
            }
            Some(Item::Add(Target::Button(bit))) => {
                let another = if view.of(bit, keys).is_empty() {
                    "a"
                } else {
                    "another"
                };
                format!("add {another} control to {}", button_name(bit))
            }
            Some(Item::EveryGame(t)) => {
                format!("use every game's controls for {}", target_name(t))
            }
            Some(Item::Directions) if self.forced_apart(controls) => {
                "say why the directions stay separate".into()
            }
            Some(Item::Directions) if self.apart => "bind the directions together".into(),
            Some(Item::Directions) => "bind each direction separately".into(),
            Some(Item::Reset) if self.confirm_reset => {
                "again to reset; anything else keeps them".into()
            }
            Some(Item::Reset) => self.reset_label().to_lowercase(),
            None => String::new(),
        };
        let (b, y) = if keys { ("Esc", "Ctrl+Z") } else { ("B", "Y") };
        let mut s = format!("{a} {what}   ·   {b} back");
        if self.undo.is_some() {
            s += &format!("   ·   {y} undo");
        }
        s
    }

    fn reset_label(&self) -> String {
        let what = if self.keyboard { "keys" } else { "controller" };
        if self.this_game {
            format!("Reset this game's {what} to every game's")
        } else {
            format!("Reset every game's {what} to defaults")
        }
    }
}

impl Menu {
    /// A move on the Controls page: its own focus, tabs, undo, and the
    /// control to add while it listens. Back is the caller's.
    pub(super) fn controls_nav(&mut self, n: Nav) {
        let p = &mut self.remap;
        if let Some((target, until)) = p.listen {
            // The other kind's (a pad pressed while it waits for a key)
            // is passed over: Steam Input says a pad's press as a key too,
            // and whichever came first would stop it. One too late stops it.
            if let Nav::Bind(c) = n
                && c.is_key() == p.keyboard
            {
                p.listen = None;
                if Instant::now() < until {
                    self.add(target, c);
                }
            }
            return;
        }
        let rows = p.rows(&self.controls, self.deadzone_axes_apart);
        p.clamp(&rows);
        let item = p.focused(&rows);
        if n != Nav::Ok {
            p.confirm_reset = false;
        }
        let (r, i) = p.focus;
        match n {
            Nav::Up | Nav::Down => {
                let found = if n == Nav::Up {
                    (0..r).rev().find(|&r| !rows[r].items.is_empty())
                } else {
                    (r + 1..rows.len()).find(|&r| !rows[r].items.is_empty())
                };
                if let Some(r) = found {
                    p.focus = (r, i);
                    p.clamp(&rows);
                }
            }
            // Left and right switch the tabs, and step a deadzone.
            Nav::Left | Nav::Right => {
                let right = n == Nav::Right;
                match item {
                    Some(Item::Tab(_)) => {
                        self.set_tab(right);
                        self.remap.focus = (0, usize::from(right));
                    }
                    Some(Item::Deadzone(dz)) => self.step_deadzone(dz, right),
                    _ if right => p.focus.1 = (i + 1).min(rows[r].items.len() - 1),
                    _ => p.focus.1 = i.saturating_sub(1),
                }
            }
            Nav::Erase => {
                if let Some(item @ Item::Chip(..)) = item {
                    self.act(item);
                }
            }
            Nav::Undo => self.undo_change(),
            Nav::Ok => {
                if let Some(item) = item {
                    self.act(item);
                }
            }
            _ => {}
        }
    }

    fn set_tab(&mut self, keyboard: bool) {
        let p = &mut self.remap;
        p.keyboard = keyboard;
        p.confirm_reset = false;
        p.listen = None;
    }

    /// The pads' deadzones, as the page has them now.
    pub fn pad_deadzones(&self) -> gamepad::Deadzones {
        let trigger = self.deadzones[1];
        if self.deadzone_axes_apart {
            let [h, v] = [Dz::Horizontal, Dz::Vertical].map(|dz| self.deadzone(dz));
            gamepad::Deadzones::percent_axes(h, v, trigger)
        } else {
            gamepad::Deadzones::percent(self.deadzones[0], trigger)
        }
    }

    /// `dz`, in percent: a stick's way never set is the sticks' one.
    fn deadzone(&self, dz: Dz) -> u32 {
        match dz {
            Dz::Sticks => self.deadzones[0],
            Dz::Triggers => self.deadzones[1],
            Dz::Horizontal => self.axis_deadzones[0].unwrap_or(self.deadzones[0]),
            Dz::Vertical => self.axis_deadzones[1].unwrap_or(self.deadzones[0]),
        }
    }

    /// Save `key = value` to the settings file, said under the page.
    fn save_setting(&mut self, key: &str, value: &str) {
        self.saved =
            self.settings
                .as_ref()
                .map(|path| match crate::settings::save(path, key, value) {
                    Ok(()) => format!("saved to {}", path.display()),
                    Err(e) => format!("not saved: {e}"),
                });
    }

    /// Step deadzone `dz` up or down, saved.
    fn step_deadzone(&mut self, dz: Dz, up: bool) {
        use crate::settings::{MAX_DEADZONE, MIN_DEADZONE};
        let d = self.deadzone(dz);
        let to = if up {
            (d + DEADZONE_STEP).min(MAX_DEADZONE)
        } else {
            d.saturating_sub(DEADZONE_STEP).max(MIN_DEADZONE)
        };
        if to == d {
            return;
        }
        match dz {
            Dz::Sticks => self.deadzones[0] = to,
            Dz::Triggers => self.deadzones[1] = to,
            Dz::Horizontal => self.axis_deadzones[0] = Some(to),
            Dz::Vertical => self.axis_deadzones[1] = Some(to),
        }
        self.deadzones_changed = true;
        self.save_setting(dz.key(), &to.to_string());
    }

    /// Do what `item` does: A on it, or a click.
    fn act(&mut self, item: Item) {
        let this_game = self.remap.this_game;
        let keys = self.remap.keyboard;
        if item != Item::Reset {
            self.remap.confirm_reset = false;
        }
        match item {
            Item::Tab(k) => self.set_tab(k),
            // A steps it up; Left and right, or the − and +, either way.
            Item::Deadzone(dz) => self.step_deadzone(dz, true),
            Item::DeadzoneAxes => {
                self.deadzone_axes_apart = !self.deadzone_axes_apart;
                self.deadzones_changed = true;
                let axes = if self.deadzone_axes_apart {
                    "apart"
                } else {
                    "together"
                };
                self.save_setting("deadzone-axes", axes);
            }
            Item::Scope(b) => self.remap.this_game = b,
            Item::Chip(t, c) => {
                let before = self.controls.config.clone();
                match (t, c) {
                    (Target::Button(bit), Chip::One(c)) => {
                        self.controls.remove(this_game, bit, c);
                    }
                    (_, Chip::Four(f)) => self.controls.remove_four(this_game, f),
                    (Target::Directions, Chip::One(_)) => {}
                }
                let text = format!("Removed {} from {}.", chip_name(c), target_name(t));
                self.edited(before, text);
                self.refocus(Some(t), false);
            }
            Item::Add(t) => {
                self.remap.listen = Some((t, Instant::now() + LISTEN_TIME));
            }
            Item::EveryGame(t) => {
                let before = self.controls.config.clone();
                let bits = match t {
                    Target::Button(bit) => vec![bit],
                    Target::Directions => DIRECTIONS.to_vec(),
                };
                for bit in bits {
                    self.controls.use_every_game(bit, keys);
                }
                let text = format!("{} uses every game's controls.", target_name(t));
                self.edited(before, text);
                self.refocus(Some(t), false);
            }
            Item::Directions if self.remap.forced_apart(&self.controls) => {
                self.remap.say(
                    "Up, Down, Left and Right have different controls, so they stay \
                     separate until they match."
                        .into(),
                    false,
                );
            }
            Item::Directions => {
                self.remap.apart = !self.remap.apart;
                let apart = if self.remap.apart {
                    "apart"
                } else {
                    "together"
                };
                self.save_setting("directions", apart);
            }
            Item::Reset if self.remap.confirm_reset => {
                self.remap.confirm_reset = false;
                let before = self.controls.config.clone();
                let text = format!("{}: done.", self.remap.reset_label());
                self.controls.reset(this_game, keys);
                self.edited(before, text);
                // The rows above may have joined; the focus stays on it.
                self.remap.focus = (
                    self.remap
                        .rows(&self.controls, self.deadzone_axes_apart)
                        .len()
                        - 1,
                    0,
                );
            }
            Item::Reset => self.remap.confirm_reset = true,
        }
    }

    /// `c` pressed for "+ Add" on `target`.
    fn add(&mut self, target: Target, c: Control) {
        let this_game = self.remap.this_game;
        let before = self.controls.config.clone();
        let name = control_name(c);
        if !gamepad::nameable(c) {
            let text = format!("{name} can't be bound: the mapping file has no name for it.");
            self.remap.say(text, false);
            return;
        }
        let taken = match target {
            Target::Button(bit) => self.controls.add(this_game, bit, c),
            Target::Directions => match Four::of(c) {
                Some(f) => self.controls.add_four(this_game, f),
                None => {
                    let what = if c.is_key() {
                        "four keys (arrows, WASD, IJKL, keypad)"
                    } else {
                        "a stick or the D-pad"
                    };
                    self.remap.say(
                        format!(
                            "{name} is one button: the directions take {what}. \
                             To give one direction a button, bind them separately."
                        ),
                        false,
                    );
                    return;
                }
            },
        };
        let name = match (target, Four::of(c)) {
            (Target::Directions, Some(f)) => f.name.to_owned(),
            _ => name,
        };
        if before == self.controls.config {
            let text = format!("{name} already presses {}.", target_name(target));
            self.remap.say(text, false);
            return;
        }
        // The focus stays on the row's "+ Add", past the new chip.
        self.refocus(Some(target), true);
        let text = if taken.is_empty() {
            format!("Added {name} to {}.", target_name(target))
        } else {
            let from: Vec<_> = taken.iter().map(|&b| button_name(b)).collect();
            let scope = if this_game { "this game" } else { "every game" };
            format!(
                "{name} moved from {} to {} ({scope}).",
                from.join(", "),
                target_name(target)
            )
        };
        self.edited(before, text);
    }

    /// The mapping went from `before` to now: saved, said, undoable.
    fn edited(&mut self, before: crate::remap::Config, text: String) {
        if before == self.controls.config {
            return;
        }
        self.remap.undo = Some(before);
        self.remap.say(text, true);
        self.changed();
    }

    fn undo_change(&mut self) {
        if let Some(before) = self.remap.undo.take() {
            let target = self.focused_target();
            self.controls.config = before;
            self.refocus(target, false);
            self.remap.say("Undone.".into(), false);
            self.changed();
        }
    }

    /// What the focused row binds, if it's a row of bindings.
    fn focused_target(&self) -> Option<Target> {
        let rows = self.remap.rows(&self.controls, self.deadzone_axes_apart);
        match rows.get(self.remap.focus.0)?.kind {
            Kind::Binds { target, .. } => Some(target),
            _ => None,
        }
    }

    /// Keep the focus on `target`'s row (or, if the directions were
    /// joined or split, theirs) as the rows change round it: on its
    /// "+ Add" if `add`, else the same item, clamped.
    fn refocus(&mut self, target: Option<Target>, add: bool) {
        let Some(target) = target else { return };
        let rows = self.remap.rows(&self.controls, self.deadzone_axes_apart);
        let is_dir = |t: Target| match t {
            Target::Directions => true,
            Target::Button(bit) => DIRECTIONS.contains(&bit),
        };
        let row = |same: &dyn Fn(Target) -> bool| {
            rows.iter().position(|r| match r.kind {
                Kind::Binds { target: t, .. } => same(t),
                _ => false,
            })
        };
        let Some(r) =
            row(&|t| t == target).or_else(|| is_dir(target).then(|| row(&is_dir)).flatten())
        else {
            return;
        };
        let items = &rows[r].items;
        let i = if add {
            items.iter().position(|i| matches!(i, Item::Add(_)))
        } else {
            None
        };
        self.remap.focus = (r, i.unwrap_or(self.remap.focus.1));
        self.remap.clamp(&rows);
    }

    /// The Controls page.
    pub(super) fn controls_page(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        if self.remap.listen.is_some_and(|(_, until)| until <= now) {
            self.remap.listen = None;
        }
        let rows = self.remap.rows(&self.controls, self.deadzone_axes_apart);
        self.remap.clamp(&rows);
        let mut clicked: Option<(usize, usize, bool)> = None;
        let keys = self.remap.keyboard;
        ui.vertical_centered(|ui| ui.heading("Controls"));
        // The tabs, row 0, and the scope switch under them, row 1.
        let focus_in = |r: usize| (self.remap.focus.0 == r).then_some(self.remap.focus.1);
        let (tab_focus, scope_focus) = (focus_in(0), focus_in(1));
        centered(ui, "controls tabs", |ui| {
            for (i, text) in ["Controller", "Keyboard"].into_iter().enumerate() {
                let on = keys == (i == 1);
                if segment(ui, text, on, tab_focus == Some(i)).clicked() {
                    clicked = Some((0, i, false));
                }
            }
        });
        ui.add_space(4.0);
        let title = match self.controls.rom.title.as_str() {
            "" => "this game".to_owned(),
            t => t.to_owned(),
        };
        let this_game = self.remap.this_game;
        centered(ui, "controls scope", |ui| {
            ui.label("Changes apply to");
            for (i, text) in ["Every game".to_owned(), format!("Only {title}")]
                .into_iter()
                .enumerate()
            {
                let on = this_game == (i == 1);
                if segment(ui, &text, on, scope_focus == Some(i)).clicked() {
                    clicked = Some((1, i, false));
                }
            }
        });
        let line = if this_game {
            "This game uses every game's controls, except the rows marked \"this game\"."
        } else {
            "A game can override these; its own changes are kept."
        };
        ui.vertical_centered(|ui| ui.label(egui::RichText::new(line).small().color(DIM)));
        ui.add(egui::Separator::default().spacing(6.0));
        let gap = ui.spacing().item_spacing.y;
        let small = ui.text_style_height(&egui::TextStyle::Small) + gap;
        let foot =
            ROW_HEIGHT + 2.0 * gap + 8.0 + small * (1 + usize::from(self.saved.is_some())) as f32;
        let height = (ui.available_height() - foot).max(ROW_HEIGHT);
        let width = ui.available_width();
        let picture = width >= PICTURE_ROOM - 2.0 * super::MARGIN;
        let view = self.controls.view(self.remap.this_game);
        let lit = match rows[self.remap.focus.0].kind {
            Kind::Binds { target, .. } => Some(target),
            _ => None,
        };
        ui.horizontal_top(|ui| {
            if picture {
                let (rect, _) = ui.allocate_exact_size(vec2(PICTURE_WIDTH, height), Sense::hover());
                gba(ui, rect, &view, keys, lit);
            }
            ui.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .auto_shrink(false)
                    .animated(false)
                    .show(ui, |ui| {
                        for (r, row) in rows.iter().enumerate() {
                            if let Some(c) = self.binds_row(ui, r, row) {
                                clicked = Some(c);
                            }
                        }
                    });
            });
        });
        // The foot: reset.
        let foot_row = rows.len() - 1;
        let foot_focus = self.remap.focus.0 == foot_row;
        ui.add(egui::Separator::default().spacing(6.0));
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if self.remap.confirm_reset {
                    format!("{}? Press again", self.remap.reset_label())
                } else {
                    self.remap.reset_label()
                };
                if button(ui, &text, foot_focus).clicked() {
                    clicked = Some((foot_row, 0, false));
                }
            });
        });
        let item = self.remap.focused(&rows);
        let prompt = self
            .remap
            .prompt(item, &self.controls, self.deadzone_axes_apart);
        let mut hints = vec![prompt.as_str()];
        hints.extend(self.saved.as_deref());
        footer(ui, &hints);
        self.toast_area(ui);
        if self.remap.listen.is_some() {
            if self.listen_dialog(ui, now) {
                self.remap.listen = None;
            }
            // The bar drains.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        } else if let Some((r, i, x)) = clicked {
            self.remap.focus = (r, i);
            let item = rows[r].items[i];
            // A chip's body only takes the focus; its × removes it.
            if x || !matches!(item, Item::Chip(..)) {
                self.act(item);
            } else {
                self.remap.confirm_reset = false;
            }
        }
    }

    /// One row of the list. Returns the item clicked, and whether on a
    /// chip's ×.
    fn binds_row(
        &mut self,
        ui: &mut egui::Ui,
        r: usize,
        row: &Row,
    ) -> Option<(usize, usize, bool)> {
        let (target, label, here) = match &row.kind {
            Kind::Heading(h) => {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(*h).small().color(DIM));
                return None;
            }
            Kind::Binds {
                target,
                label,
                here,
            } => (*target, label, *here),
            Kind::Directions => return self.directions_row(ui, r),
            Kind::Deadzone(dz) => return self.deadzone_row(ui, r, *dz),
            Kind::DeadzoneAxes => return self.deadzone_axes_row(ui, r),
            Kind::Tabs | Kind::Scope | Kind::Foot => return None,
        };
        let focus = (self.remap.focus.0 == r).then_some(self.remap.focus.1);
        let listening = self.remap.listen.map(|(t, _)| t) == Some(target);
        let mut clicked = None;
        self.row_frame(ui, r, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.set_min_height(ROW_HEIGHT - 4.0);
                let (rect, _) = ui.allocate_exact_size(vec2(LABEL_WIDTH, 20.0), Sense::hover());
                ui.painter().text(
                    rect.left_center() + vec2(6.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::TextStyle::Body.resolve(ui.style()),
                    ui.visuals().text_color(),
                );
                let mut chips = 0;
                for (i, item) in row.items.iter().enumerate() {
                    let focused = focus == Some(i);
                    match *item {
                        Item::Chip(_, c) => {
                            chips += 1;
                            let (body, x) = chip(ui, &chip_name(c), focused);
                            if x.clicked() {
                                clicked = Some((r, i, true));
                            } else if body.clicked() {
                                clicked = Some((r, i, false));
                            }
                        }
                        Item::Add(_) => {
                            if chips == 0 {
                                ui.label(egui::RichText::new("not bound").color(RED));
                            }
                            if add_button(ui, focused || listening).clicked() {
                                clicked = Some((r, i, false));
                            }
                            // Rows from every game's say nothing: `here` is
                            // false for them, if they're ever to be marked.
                            if here {
                                tag(ui);
                            }
                        }
                        Item::EveryGame(_) => {
                            let pressed = button(ui, "Use every-game", focused).clicked();
                            clicked = clicked.or(pressed.then_some((r, i, false)));
                        }
                        _ => {}
                    }
                }
            })
            .response
        });
        clicked
    }

    /// A row drawn by `add`, its background lit and scrolled to while
    /// it's focused.
    fn row_frame(
        &mut self,
        ui: &mut egui::Ui,
        r: usize,
        add: impl FnOnce(&mut egui::Ui) -> egui::Response,
    ) -> egui::Response {
        let bg = ui.painter().add(egui::Shape::Noop);
        let out = add(ui);
        let mut rect = out.rect.expand2(vec2(2.0, 1.0));
        rect.max.x = rect.max.x.max(ui.max_rect().right() - 8.0);
        if self.remap.focus.0 == r {
            ui.painter()
                .set(bg, egui::Shape::rect_filled(rect, 5.0, ROW_FOCUS));
            // Newly focused, scrolled to, once.
            if self.remap.scrolled != Some(self.remap.focus) {
                self.remap.scrolled = Some(self.remap.focus);
                ui.scroll_to_rect(rect, None);
            }
        }
        out
    }

    /// The switch binding the directions separately, over their rows.
    fn directions_row(&mut self, ui: &mut egui::Ui, r: usize) -> Option<(usize, usize, bool)> {
        let on = self.remap.shown_apart(&self.controls);
        let focused = self.remap.focus.0 == r;
        let resp = self.row_frame(ui, r, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_HEIGHT - 4.0);
                ui.add_space(6.0);
                let switch = switch(ui, on, focused);
                let label =
                    ui.add(egui::Label::new("Directions bound separately").sense(Sense::click()));
                switch | label
            })
            .inner
        });
        resp.clicked().then_some((r, 0, false))
    }

    /// The switch giving the sticks' up and down their own deadzone.
    fn deadzone_axes_row(&mut self, ui: &mut egui::Ui, r: usize) -> Option<(usize, usize, bool)> {
        let on = self.deadzone_axes_apart;
        let focused = self.remap.focus.0 == r;
        let resp = self.row_frame(ui, r, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_HEIGHT - 4.0);
                ui.add_space(6.0);
                let switch = switch(ui, on, focused);
                let text = "Up/down apart from left/right";
                let label = ui.add(egui::Label::new(text).sense(Sense::click()));
                switch | label
            })
            .inner
        });
        resp.clicked().then_some((r, 0, false))
    }

    /// A deadzone's row: its value, and − and + to step it.
    fn deadzone_row(
        &mut self,
        ui: &mut egui::Ui,
        r: usize,
        dz: Dz,
    ) -> Option<(usize, usize, bool)> {
        let focused = self.remap.focus.0 == r;
        let value = self.deadzone(dz);
        let label = match dz {
            Dz::Sticks => "Sticks",
            Dz::Horizontal => "Sticks left/right",
            Dz::Vertical => "Sticks up/down",
            Dz::Triggers => "Triggers",
        };
        let mut step = None;
        let resp = self.row_frame(ui, r, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_HEIGHT - 4.0);
                let (rect, body) = ui.allocate_exact_size(vec2(LABEL_WIDTH, 20.0), Sense::click());
                ui.painter().text(
                    rect.left_center() + vec2(6.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::TextStyle::Body.resolve(ui.style()),
                    ui.visuals().text_color(),
                );
                if button(ui, "−", false).clicked() {
                    step = Some(false);
                }
                let shown = ui.add_sized(vec2(48.0, 20.0), egui::Label::new(format!("{value}%")));
                if button(ui, "+", false).clicked() {
                    step = Some(true);
                }
                if focused {
                    let outline = shown.rect.expand(3.0);
                    ui.painter()
                        .rect_stroke(outline, 6.0, (2.0, FOCUS), egui::StrokeKind::Inside);
                }
                body
            })
            .inner
        });
        if let Some(up) = step {
            self.remap.focus = (r, 0);
            self.step_deadzone(dz, up);
            return None;
        }
        resp.clicked().then_some((r, 0, false))
    }

    /// The last change's toast, over the foot, its Undo a link.
    fn toast_area(&mut self, ui: &mut egui::Ui) {
        let Some((text, undoable, at)) = self.remap.toast.clone() else {
            return;
        };
        let left = TOAST_TIME.saturating_sub(at.elapsed());
        if left.is_zero() {
            self.remap.toast = None;
            return;
        }
        ui.ctx().request_repaint_after(left);
        let undo_key = if self.remap.keyboard { "Ctrl+Z" } else { "Y" };
        let mut undo = false;
        let rect = ui.min_rect();
        egui::Area::new(egui::Id::unique("controls toast"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.center_bottom() - vec2(0.0, 72.0))
            .pivot(egui::Align2::CENTER_BOTTOM)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgb(70, 58, 30))
                    .show(ui, |ui| {
                        ui.set_max_width(rect.width() - 40.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(&text);
                            if undoable && self.remap.undo.is_some() {
                                undo = ui.link(format!("Undo ({undo_key})")).clicked();
                            }
                        });
                    });
            });
        if undo {
            self.undo_change();
        }
    }

    /// The listening dialog over the page, the rest dimmed. Returns
    /// whether Cancel was clicked.
    fn listen_dialog(&mut self, ui: &mut egui::Ui, now: Instant) -> bool {
        let Some((target, until)) = self.remap.listen else {
            return false;
        };
        let screen = ui.ctx().content_rect();
        let left = until.saturating_duration_since(now);
        let keys = self.remap.keyboard;
        let mut cancel = false;
        egui::Area::new(egui::Id::unique("controls dim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ui.ctx(), |ui| {
                let (r, _) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter()
                    .rect_filled(r, 0.0, Color32::from_black_alpha(150));
            });
        egui::Area::new(egui::Id::unique("controls listen"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                egui::Frame::window(ui.style())
                    .stroke((1.5, FOCUS))
                    .inner_margin(16.0)
                    .show(ui, |ui| {
                        ui.set_width(300.0f32.min(screen.width() - 60.0));
                        ui.vertical_centered(|ui| {
                            let what = match target {
                                Target::Button(bit) => {
                                    format!("Adding a control to the GBA's {}", button_name(bit))
                                }
                                Target::Directions => "Adding to the directions".into(),
                            };
                            ui.label(what);
                            let ask = match (keys, target) {
                                (true, Target::Directions) => "Press an arrow key, W, I or 8…",
                                (true, _) => "Press a key…",
                                (false, Target::Directions) => "Move a stick or press the D-pad…",
                                (false, _) => "Press a button or move a stick…",
                            };
                            ui.label(egui::RichText::new(ask).size(17.0).strong());
                            ui.add_space(6.0);
                            let frac = left.as_secs_f32() / LISTEN_TIME.as_secs_f32();
                            ui.add(egui::ProgressBar::new(frac).desired_height(5.0).fill(BLUE));
                            ui.add_space(6.0);
                            cancel = ui.button("Cancel (Esc)").clicked();
                            let secs = left.as_secs_f32().ceil() as u32;
                            let note = if keys {
                                format!("Esc can't be bound. Waits {secs} s more, then cancels.")
                            } else {
                                format!(
                                    "Any button binds, B too. Waits {secs} s more, then cancels."
                                )
                            };
                            ui.label(egui::RichText::new(note).small().color(DIM));
                        });
                    });
            });
        cancel
    }
}

/// `add`'s widgets in a row, centred by its width (measured last pass).
fn centered(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    let id = egui::Id::unique(id);
    // Not measured yet (a page's dry first pass), it's left at the left.
    let w = ui.ctx().data(|d| d.get_temp::<f32>(id));
    let w = w.unwrap_or(ui.available_width());
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
        let start = ui.cursor().min.x;
        add(ui);
        let w = ui.min_rect().max.x - start;
        ui.ctx().data_mut(|d| d.insert_temp(id, w));
    });
}

/// The focus outline, drawn over `r` rather than given to the widget (a
/// button's stroke widens it, so it'd grow and shrink as focus moves).
fn outline(ui: &egui::Ui, r: &egui::Response, radius: f32, focused: bool) {
    if focused {
        ui.painter()
            .rect_stroke(r.rect, radius, (2.0, FOCUS), egui::StrokeKind::Inside);
    }
}

/// A half of a segmented switch: filled when it's the one chosen.
fn segment(ui: &mut egui::Ui, text: &str, on: bool, focused: bool) -> egui::Response {
    let fill = if on { BLUE } else { CHIP };
    let r = ui.add(egui::Button::new(egui::RichText::new(text).color(Color32::WHITE)).fill(fill));
    outline(
        ui,
        &r,
        ui.visuals().widgets.inactive.corner_radius.nw.into(),
        focused,
    );
    r
}

/// A button on the page, outlined when focused.
fn button(ui: &mut egui::Ui, text: &str, focused: bool) -> egui::Response {
    let r = ui.add(egui::Button::new(text).fill(CHIP));
    outline(
        ui,
        &r,
        ui.visuals().widgets.inactive.corner_radius.nw.into(),
        focused,
    );
    r
}

/// "+ Add", outlined: brighter while focused or listening.
fn add_button(ui: &mut egui::Ui, focused: bool) -> egui::Response {
    let r = ui.add(
        egui::Button::new("+ Add")
            .fill(Color32::TRANSPARENT)
            .stroke((1.0, DIM))
            .corner_radius(10.0),
    );
    outline(ui, &r, 10.0, focused);
    r
}

/// An on/off switch, a knob in a pill: blue and right when on.
fn switch(ui: &mut egui::Ui, on: bool, focused: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, if on { BLUE } else { CHIP });
    let x = if on {
        rect.right() - 10.0
    } else {
        rect.left() + 10.0
    };
    p.circle_filled(egui::pos2(x, rect.center().y), 7.0, Color32::WHITE);
    outline(ui, &r, 10.0, focused);
    r
}

/// The "this game" tag.
fn tag(ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(PURPLE)
        .corner_radius(6.0)
        .inner_margin(vec2(5.0, 0.0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("this game")
                    .small()
                    .color(Color32::WHITE),
            );
        });
}

/// A binding's chip, `text` and a ×: the body's response and the ×'s.
/// Focused, it's outlined and its × red.
fn chip(ui: &mut egui::Ui, text: &str, focused: bool) -> (egui::Response, egui::Response) {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let x_size = 14.0;
    let size = vec2(galley.size().x + 10.0 + x_size + 6.0, 20.0);
    let (rect, body) = ui.allocate_exact_size(size, Sense::click());
    let x_rect = Rect::from_center_size(
        egui::pos2(rect.right() - 5.0 - x_size / 2.0, rect.center().y),
        Vec2::splat(x_size),
    );
    let x = ui.interact(x_rect, body.id.with("x"), Sense::click());
    let p = ui.painter();
    let fill = if body.hovered() { CHIP_HOVER } else { CHIP };
    p.rect_filled(rect, 10.0, fill);
    if focused {
        p.rect_stroke(rect, 10.0, (2.0, FOCUS), egui::StrokeKind::Inside);
    }
    p.galley(
        egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0),
        galley,
        Color32::WHITE,
    );
    let red = focused || x.hovered();
    if red {
        p.circle_filled(x_rect.center(), x_size / 2.0, RED);
    }
    let c = x_rect.center();
    let d = 3.0;
    let stroke = (1.5, if red { Color32::WHITE } else { DIM });
    p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
    p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
    (body, x)
}

/// The GBA in `rect`, each button labelled with what presses it in `m`
/// (keys or pads'), `lit`'s button lit.
fn gba(ui: &egui::Ui, rect: Rect, m: &gamepad::Mapping, keys: bool, lit: Option<Target>) {
    let p = ui.painter_at(rect);
    let o = rect.min + vec2(0.0, 18.0);
    let at = |x: f32, y: f32| o + vec2(x, y);
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    let is_lit = |bit: u16| match lit {
        Some(Target::Button(b)) => b == bit,
        Some(Target::Directions) => DIRECTIONS.contains(&bit),
        None => false,
    };
    let part = |bit: u16| {
        if is_lit(bit) {
            BLUE
        } else {
            Color32::from_rgb(60, 48, 110)
        }
    };
    let names = |bit: u16| {
        let cs = m.of(bit, keys);
        if cs.is_empty() {
            return "nothing".to_owned();
        }
        let n: Vec<_> = cs.into_iter().map(control_name).collect();
        n.join(", ")
    };
    let dir_names = || {
        let view_fours = Four::split(m, keys);
        match view_fours {
            Some(f) if f.is_empty() => "nothing".to_owned(),
            Some(f) => f.iter().map(|f| f.name).collect::<Vec<_>>().join(", "),
            None => "each its own".to_owned(),
        }
    };
    // Shoulders, then the body over them.
    p.rect_filled(r(14.0, 0.0, 66.0, 12.0), 5.0, part(key::L));
    p.rect_filled(r(130.0, 0.0, 182.0, 12.0), 5.0, part(key::R));
    p.rect_filled(r(6.0, 6.0, 190.0, 100.0), 30.0, BODY);
    p.rect_filled(
        r(56.0, 18.0, 140.0, 78.0),
        4.0,
        Color32::from_rgb(28, 40, 30),
    );
    p.rect_stroke(
        r(56.0, 18.0, 140.0, 78.0),
        4.0,
        (2.0, Color32::from_rgb(20, 20, 24)),
        egui::StrokeKind::Outside,
    );
    // The D-pad's cross.
    let dpad = if DIRECTIONS.iter().any(|&d| is_lit(d)) {
        BLUE
    } else {
        Color32::from_rgb(40, 34, 70)
    };
    p.rect_filled(r(18.0, 46.0, 46.0, 56.0), 2.0, dpad);
    p.rect_filled(r(27.0, 37.0, 37.0, 65.0), 2.0, dpad);
    let font = egui::FontId::proportional(11.0);
    for (bit, c, name) in [
        (key::A, at(170.0, 42.0), "A"),
        (key::B, at(154.0, 60.0), "B"),
    ] {
        p.circle_filled(c, 9.0, part(bit));
        p.text(
            c,
            egui::Align2::CENTER_CENTER,
            name,
            font.clone(),
            Color32::WHITE,
        );
    }
    p.rect_filled(r(82.0, 86.0, 96.0, 91.0), 2.5, part(key::SELECT));
    p.rect_filled(r(102.0, 86.0, 116.0, 91.0), 2.5, part(key::START));
    // What presses each, round it.
    let label = |pos, align, text: String, bit: u16| {
        let colour = if is_lit(bit) { Color32::WHITE } else { DIM };
        p.text(pos, align, text, font.clone(), colour);
    };
    let top = rect.min;
    label(
        top,
        egui::Align2::LEFT_TOP,
        format!("L: {}", names(key::L)),
        key::L,
    );
    label(
        egui::pos2(rect.right(), top.y),
        egui::Align2::RIGHT_TOP,
        format!("R: {}", names(key::R)),
        key::R,
    );
    let mut y = o.y + 112.0;
    for (text, bit) in [
        (format!("A: {}", names(key::A)), key::A),
        (format!("B: {}", names(key::B)), key::B),
        (format!("D-pad: {}", dir_names()), key::UP),
        (format!("Start: {}", names(key::START)), key::START),
        (format!("Select: {}", names(key::SELECT)), key::SELECT),
    ] {
        label(
            egui::pos2(rect.left(), y),
            egui::Align2::LEFT_TOP,
            text,
            bit,
        );
        y += 15.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shortcuts' rows on a page, by name.
    fn shortcuts(p: &Page) -> Vec<String> {
        p.rows(&Controls::default(), false)
            .into_iter()
            .filter_map(|r| match r.kind {
                Kind::Binds {
                    target: Target::Button(b),
                    label,
                    ..
                } if SHORTCUTS.iter().any(|&(s, _)| s == b) => Some(label),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_native_window_leaves_out_states_rewind_and_scripts() {
        let mut p = Page::default();
        assert_eq!(shortcuts(&p).len(), SHORTCUTS.len());
        p.native = true;
        assert_eq!(
            shortcuts(&p),
            ["Fast-forward (hold)", "Fast-forward on/off"]
        );
    }
}
