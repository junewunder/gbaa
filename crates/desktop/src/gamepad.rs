//! Gamepads, read through SDL3's gamepad API (evdev and its own drivers on
//! Linux, so the virtual pad Steam Input makes in the Deck's Game Mode;
//! XInput and the like elsewhere). Every connected pad presses the GBA's
//! buttons through one [`Mapping`]: the default, with any buttons a config
//! file names bound as it says, for every game and then for this one
//! ([`crate::remap`]). The keyboard's keys are in the same mapping, on
//! lines of their own (`key a = z`), so a pad's line and a key's for the
//! same button replace each other's defaults separately
//! ([`Mapping::pressed`] reads them).
//!
//! The held state is our own ([`Pads::feed`]), not SDL's, so a test can
//! feed events, and a pad that goes away lets go of everything it held.
//! The window's thread ([`crate::window`]) feeds it SDL's events.

use runtime::io::key;
use sdl3::gamepad::{Axis, Button};
use sdl3::keyboard::Scancode;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// How far a stick goes before it counts as a press, unless the settings
/// file says otherwise ([`crate::settings`]), in percent. Wide, as the
/// GBA's pad is all or nothing: a small stray move presses nothing.
pub const DEFAULT_DEADZONE: u32 = 40;

/// The triggers', as [`DEFAULT_DEADZONE`] is the sticks'.
pub const DEFAULT_TRIGGER_DEADZONE: u32 = 20;

/// How far the sticks and the triggers go before they count, each 0 to 1:
/// a stick's left and right apart from its up and down, so stray ups can
/// take a wider one than quick sideways moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deadzones {
    pub horizontal: f32,
    pub vertical: f32,
    pub trigger: f32,
}

impl Default for Deadzones {
    fn default() -> Deadzones {
        Deadzones::percent(DEFAULT_DEADZONE, DEFAULT_TRIGGER_DEADZONE)
    }
}

impl Deadzones {
    /// The sticks' (both ways) and the triggers', in percent.
    pub fn percent(stick: u32, trigger: u32) -> Deadzones {
        Deadzones::percent_axes(stick, stick, trigger)
    }

    /// The sticks' left and right, their up and down, and the triggers',
    /// in percent.
    pub fn percent_axes(horizontal: u32, vertical: u32, trigger: u32) -> Deadzones {
        Deadzones {
            horizontal: horizontal as f32 / 100.0,
            vertical: vertical as f32 / 100.0,
            trigger: trigger as f32 / 100.0,
        }
    }

    /// Axis `a`'s.
    pub fn of(self, a: Axis) -> f32 {
        match a {
            Axis::TriggerLeft | Axis::TriggerRight => self.trigger,
            Axis::LeftY | Axis::RightY => self.vertical,
            _ => self.horizontal,
        }
    }
}

/// A face button by the letter printed on it, whichever position it's in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    A,
    B,
    X,
    Y,
}

/// Where the face buttons labelled A, B, X and Y are on an Xbox pad (and
/// the Steam Deck): south, east, west and north. Pads whose buttons carry
/// no letters (PlayStation's shapes) are read as if laid out so.
pub const XBOX_LABELS: [Button; 4] = [Button::South, Button::East, Button::West, Button::North];

/// A control on a pad: a button, a face button by its label, or a stick or
/// trigger pushed one way; or a keyboard key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Button(Button),
    Label(Label),
    /// The axis pushed past the deadzone, toward its positive end or not
    /// (SDL's sticks are positive right and down, its triggers positive as
    /// they're pulled).
    Stick(Axis, bool),
    /// A keyboard key, by position.
    Key(Scancode),
}

impl Control {
    /// Whether it's a keyboard key, not a pad's.
    pub fn is_key(self) -> bool {
        matches!(self, Control::Key(_))
    }
}

/// Control names in config files.
const CONTROLS: &[(&str, Control)] = &[
    ("south", Control::Button(Button::South)),
    ("east", Control::Button(Button::East)),
    ("north", Control::Button(Button::North)),
    ("west", Control::Button(Button::West)),
    ("label-a", Control::Label(Label::A)),
    ("label-b", Control::Label(Label::B)),
    ("label-x", Control::Label(Label::X)),
    ("label-y", Control::Label(Label::Y)),
    ("lb", Control::Button(Button::LeftShoulder)),
    ("rb", Control::Button(Button::RightShoulder)),
    ("lt", Control::Stick(Axis::TriggerLeft, true)),
    ("rt", Control::Stick(Axis::TriggerRight, true)),
    ("select", Control::Button(Button::Back)),
    ("start", Control::Button(Button::Start)),
    ("mode", Control::Button(Button::Guide)),
    ("lthumb", Control::Button(Button::LeftStick)),
    ("rthumb", Control::Button(Button::RightStick)),
    ("dpad-up", Control::Button(Button::DPadUp)),
    ("dpad-down", Control::Button(Button::DPadDown)),
    ("dpad-left", Control::Button(Button::DPadLeft)),
    ("dpad-right", Control::Button(Button::DPadRight)),
    ("lstick-up", Control::Stick(Axis::LeftY, false)),
    ("lstick-down", Control::Stick(Axis::LeftY, true)),
    ("lstick-left", Control::Stick(Axis::LeftX, false)),
    ("lstick-right", Control::Stick(Axis::LeftX, true)),
    ("rstick-up", Control::Stick(Axis::RightY, false)),
    ("rstick-down", Control::Stick(Axis::RightY, true)),
    ("rstick-left", Control::Stick(Axis::RightX, false)),
    ("rstick-right", Control::Stick(Axis::RightX, true)),
    // Past an Xbox pad's: the Steam Controller's and Deck's grips, the
    // Elite's paddles, a share or capture button, a touchpad's click.
    ("right-paddle1", Control::Button(Button::RightPaddle1)),
    ("left-paddle1", Control::Button(Button::LeftPaddle1)),
    ("right-paddle2", Control::Button(Button::RightPaddle2)),
    ("left-paddle2", Control::Button(Button::LeftPaddle2)),
    ("misc1", Control::Button(Button::Misc1)),
    ("misc2", Control::Button(Button::Misc2)),
    ("misc3", Control::Button(Button::Misc3)),
    ("misc4", Control::Button(Button::Misc4)),
    ("misc5", Control::Button(Button::Misc5)),
    ("misc6", Control::Button(Button::Misc6)),
    ("touchpad", Control::Button(Button::Touchpad)),
];

/// Fast-forward while held ([`crate::fast`]): a bit past the GBA's ten,
/// which [`Pads::keys`] reports alongside them.
pub const FAST: u16 = 1 << 14;
/// Fast-forward on or off at each press.
pub const FAST_TOGGLE: u16 = 1 << 15;
/// Save a state to the current slot, and load it ([`crate::slots`]), at
/// each press.
pub const SAVE_STATE: u16 = 1 << 12;
pub const LOAD_STATE: u16 = 1 << 13;
/// Rewind while held ([`crate::rewind`]).
pub const REWIND: u16 = 1 << 10;
/// Show or hide the script console ([`crate::console`]) at each press;
/// the window reads it, not the game.
pub const CONSOLE: u16 = 1 << 11;

/// Button names in config files: the GBA's, as in `--input` scripts, and
/// the front end's own.
pub const BUTTONS: [(&str, u16); 16] = [
    ("a", key::A),
    ("b", key::B),
    ("select", key::SELECT),
    ("start", key::START),
    ("right", key::RIGHT),
    ("left", key::LEFT),
    ("up", key::UP),
    ("down", key::DOWN),
    ("r", key::R),
    ("l", key::L),
    ("fast", FAST),
    ("fast-toggle", FAST_TOGGLE),
    ("save-state", SAVE_STATE),
    ("load-state", LOAD_STATE),
    ("rewind", REWIND),
    ("console", CONSOLE),
];

/// The default bindings: on the keyboard, arrows, X = A, Z = B (mGBA's
/// and RetroArch's: B left of A, as on a GBA), A = L, S = R, Enter = Start, Backspace = Select, Tab held fast-forwards and `
/// toggles it, R held rewinds. On pads, the GBA's A and B are the face buttons labelled A
/// and B (south and east on Xbox pads and the Steam Deck, the other way
/// round on Nintendo's), not where they sit on a GBA, and the buttons
/// across from them press the same: Y = A, X = B (june, 2026-10-29). Shoulders
/// and triggers both press L and R; the left stick is a second D-pad.
/// Holding the right stick in fast-forwards; clicking the left one turns
/// fast-forward on or off. Pushing the right stick left, which the GBA
/// has no use for, rewinds. Saving and loading a state and the script
/// console have no pad control by default (june, 2026-10-28 and
/// 2026-10-29: too easy to hit); F1-F4 and F9 on the keyboard, the menu,
/// or a `save-state`/`load-state`/`console` line bind them.
pub const DEFAULT: &str = "\
a = label-a label-y
b = label-b label-x
l = lb lt
r = rb rt
start = start
select = select
up = dpad-up lstick-up
down = dpad-down lstick-down
left = dpad-left lstick-left
right = dpad-right lstick-right
fast = rthumb
fast-toggle = lthumb
rewind = rstick-left
key up = up
key down = down
key left = left
key right = right
key a = x
key b = z
key l = a
key r = s
key start = return
key select = backspace
key fast = tab
key fast-toggle = `
key rewind = r
key console = f9
";

/// Player 2's keys in linked play ([`crate::link`]), the right hand's
/// side of the keyboard; player 1 keeps the mapping's.
const PLAYER2: &str = "\
key up = i
key down = k
key left = j
key right = l
key a = n
key b = b
key l = u
key r = o
key start = .
key select = ,
";

/// Player 2's keys in linked play: I J K L the D-pad, N = A, B = B,
/// U = L, O = R, . = Start, ',' = Select.
pub fn player2_keys() -> Mapping {
    Mapping::parse(PLAYER2).expect("player 2's keys parse")
}

/// A control's name in config files: a key's is SDL's, lower case, with
/// `_` for its spaces (`left_shift`, `keypad_enter`), and `hash` for `#`.
pub fn control_name(c: Control) -> String {
    match c {
        Control::Key(sc) => match sc.name() {
            "#" => "hash".into(),
            n => n.to_lowercase().replace(' ', "_"),
        },
        c => CONTROLS
            .iter()
            .find(|(_, k)| *k == c)
            .map_or("?", |(n, _)| n)
            .into(),
    }
}

/// Whether a config file can name `c`: its name reads back as it.
/// (A key SDL has no name for can't be, nor a button newer than
/// [`CONTROLS`].)
pub fn nameable(c: Control) -> bool {
    let name = control_name(c);
    match c {
        Control::Key(sc) => key_named(&name) == Some(sc),
        c => CONTROLS.iter().any(|&(n, k)| n == name && k == c),
    }
}

/// The key a config file's name names ([`control_name`]).
fn key_named(name: &str) -> Option<Scancode> {
    match name {
        "hash" => Scancode::from_name("#"),
        "" => None,
        n => Scancode::from_name(&n.replace('_', " ")),
    }
}

/// One line of a mapping: a button (a bit of [`BUTTONS`]), whether it's
/// the keyboard's line for it or the pads', and the controls that press
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub bit: u16,
    pub keys: bool,
    pub controls: Vec<Control>,
}

/// Which controls press which GBA buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping(Vec<(Control, u16)>);

impl Default for Mapping {
    fn default() -> Mapping {
        Mapping::parse(DEFAULT).expect("the default mapping parses")
    }
}

impl Mapping {
    /// Parse lines of `button = control control …` (an empty list unbinds
    /// it). `#` starts a comment. Buttons the text doesn't name are
    /// unbound: [`Mapping::with`] lays a file over the default.
    pub fn parse(text: &str) -> Result<Mapping, String> {
        Ok(Mapping(
            Mapping::lines(text.lines().enumerate())?
                .into_iter()
                .flat_map(|b| b.controls.into_iter().map(move |c| (c, b.bit)))
                .collect(),
        ))
    }

    /// This mapping with each button `text` names bound only as it says.
    pub fn with(&self, text: &str) -> Result<Mapping, String> {
        Ok(self.with_lines(Mapping::lines(text.lines().enumerate())?))
    }

    /// This mapping with each button `lines` names bound only as it says:
    /// a pads' line replaces the pads' controls, a keyboard's line the
    /// keys.
    pub fn with_lines(&self, lines: Vec<Binding>) -> Mapping {
        let mut binds: Vec<_> = self
            .0
            .iter()
            .filter(|(c, bit)| lines.iter().all(|l| l.bit != *bit || l.keys != c.is_key()))
            .copied()
            .collect();
        for l in lines {
            binds.extend(l.controls.into_iter().map(|c| (c, l.bit)));
        }
        Mapping(binds)
    }

    /// The pads' controls that press `bit`.
    pub fn controls(&self, bit: u16) -> Vec<Control> {
        self.of(bit, false)
    }

    /// The keys that press `bit`.
    pub fn keys(&self, bit: u16) -> Vec<Control> {
        self.of(bit, true)
    }

    /// The controls that press `bit`: the keys, or the pads'.
    pub fn of(&self, bit: u16, keys: bool) -> Vec<Control> {
        self.0
            .iter()
            .filter(|(c, b)| *b == bit && c.is_key() == keys)
            .map(|(c, _)| *c)
            .collect()
    }

    /// The buttons (bits of [`BUTTONS`]) the keys `down` says are held
    /// press.
    pub fn pressed(&self, down: impl Fn(Scancode) -> bool) -> u16 {
        self.0
            .iter()
            .filter(|(c, _)| matches!(c, Control::Key(k) if down(*k)))
            .fold(0, |keys, (_, bit)| keys | bit)
    }

    /// Parse `button = controls` lines, numbered from 0 (sections are
    /// [`crate::remap::Config`]'s).
    pub fn lines<'a>(text: impl Iterator<Item = (usize, &'a str)>) -> Result<Vec<Binding>, String> {
        let mut out = Vec::new();
        for (n, line) in text {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let err = |what: String| format!("line {}: {what}", n + 1);
            if line.starts_with('[') {
                return Err(err("a section here".into()));
            }
            let (button, controls) = line
                .split_once('=')
                .ok_or_else(|| err("expected `button = controls`".into()))?;
            let button = button.trim();
            let (keys, button) = match button.strip_prefix("key ") {
                Some(b) => (true, b.trim()),
                None => (false, button),
            };
            let bit = BUTTONS
                .iter()
                .find(|(n, _)| *n == button)
                .ok_or_else(|| err(format!("unknown GBA button {button:?}")))?
                .1;
            if out.iter().any(|l: &Binding| l.bit == bit && l.keys == keys) {
                return Err(err(format!("{button} named twice")));
            }
            let controls = controls
                .split_whitespace()
                .map(|c| {
                    let found = if keys {
                        key_named(c).map(Control::Key)
                    } else {
                        CONTROLS.iter().find(|(n, _)| *n == c).map(|(_, c)| *c)
                    };
                    found.ok_or_else(|| {
                        let what = if keys { "key" } else { "control" };
                        err(format!("unknown {what} {c:?}"))
                    })
                })
                .collect::<Result<_, _>>()?;
            out.push(Binding {
                bit,
                keys,
                controls,
            });
        }
        Ok(out)
    }
}

/// What a pad did, as [`Pads::feed`] takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadEvent {
    Pressed(Button),
    Released(Button),
    /// A stick or trigger moved: from -1 to 1 (triggers 0 to 1).
    Axis(Axis, f32),
    /// Where its face buttons labelled A, B, X and Y are, as
    /// [`XBOX_LABELS`] (what a pad is taken to have until told).
    Labels([Button; 4]),
    /// Unplugged: everything it held is let go.
    Gone,
}

/// What one pad holds.
struct Held {
    buttons: Vec<Button>,
    axes: HashMap<Axis, f32>,
    labels: [Button; 4],
}

impl Default for Held {
    fn default() -> Held {
        Held {
            buttons: Vec::new(),
            axes: HashMap::new(),
            labels: XBOX_LABELS,
        }
    }
}

impl Held {
    /// Whether `c` is held, a stick or trigger past its deadzone.
    fn has(&self, c: Control, dz: Deadzones) -> bool {
        match c {
            Control::Button(b) => self.buttons.contains(&b),
            Control::Label(l) => self.buttons.contains(&self.labels[l as usize]),
            Control::Stick(a, up) => self
                .axes
                .get(&a)
                .is_some_and(|&v| if up { v > dz.of(a) } else { v < -dz.of(a) }),
            Control::Key(_) => false,
        }
    }
}

/// Every pad's held controls, read through one mapping.
pub struct Pads {
    mapping: Mapping,
    /// By SDL's joystick instance ID.
    held: HashMap<u32, Held>,
    /// Their IDs in the order they came: linked play's seats.
    order: Vec<u32>,
    /// How far a stick or trigger goes before it's pressed.
    pub deadzones: Deadzones,
}

impl Pads {
    /// No pads yet: [`Pads::feed`] tells it what they do.
    pub fn new(mapping: Mapping) -> Pads {
        Pads {
            mapping,
            held: HashMap::new(),
            order: Vec::new(),
            deadzones: Deadzones::default(),
        }
    }

    /// Read the pads through `mapping` from now on (the menu's remap
    /// screen changed it).
    pub fn set_mapping(&mut self, mapping: Mapping) {
        self.mapping = mapping;
    }

    /// The mapping they're read through, the keyboard's lines too.
    pub fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    /// Pad `id` did `e`.
    pub fn feed(&mut self, id: u32, e: PadEvent) {
        if !self.order.contains(&id) {
            self.order.push(id);
        }
        let held = self.held.entry(id).or_default();
        match e {
            PadEvent::Pressed(b) => {
                if !held.buttons.contains(&b) {
                    held.buttons.push(b);
                }
            }
            PadEvent::Released(b) => held.buttons.retain(|&h| h != b),
            PadEvent::Axis(a, v) => {
                held.axes.insert(a, v);
            }
            PadEvent::Labels(l) => held.labels = l,
            PadEvent::Gone => {
                self.held.remove(&id);
                self.order.retain(|&o| o != id);
            }
        }
    }

    /// The buttons held now, on any pad: the GBA's, [`FAST`],
    /// [`FAST_TOGGLE`], [`SAVE_STATE`], [`LOAD_STATE`], [`REWIND`] and
    /// [`CONSOLE`].
    pub fn keys(&self) -> u16 {
        self.mapping
            .0
            .iter()
            .filter(|(c, _)| self.held.values().any(|h| h.has(*c, self.deadzones)))
            .fold(0, |keys, (_, bit)| keys | bit)
    }

    /// The buttons held now on the `n`th pad still plugged in (from 0), in
    /// the order they came: player `n + 1`'s in linked play.
    pub fn keys_of(&self, n: usize) -> u16 {
        let Some(h) = self.order.get(n).and_then(|id| self.held.get(id)) else {
            return 0;
        };
        self.mapping
            .0
            .iter()
            .filter(|(c, _)| h.has(*c, self.deadzones))
            .fold(0, |keys, (_, bit)| keys | bit)
    }
}

/// Every button and axis of SDL's standard gamepad, for a virtual one.
pub const BUTTONS_ALL: [Button; 15] = [
    Button::South,
    Button::East,
    Button::West,
    Button::North,
    Button::Back,
    Button::Guide,
    Button::Start,
    Button::LeftStick,
    Button::RightStick,
    Button::LeftShoulder,
    Button::RightShoulder,
    Button::DPadUp,
    Button::DPadDown,
    Button::DPadLeft,
    Button::DPadRight,
];
pub const AXES_ALL: [Axis; 6] = [
    Axis::LeftX,
    Axis::LeftY,
    Axis::RightX,
    Axis::RightY,
    Axis::TriggerLeft,
    Axis::TriggerRight,
];

/// A step of a [`Script`].
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Wait(std::time::Duration),
    /// Press these together, hold them [`Script::HOLD`], let go.
    Press(Vec<Control>),
}

/// Controls for a virtual pad to press (`--pad-script`), for driving the
/// window unattended: words split by spaces, each a number of
/// milliseconds to wait, or controls (as in a mapping file) joined by `+`
/// to press together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script(pub Vec<Step>);

impl Script {
    /// How long each press is held, and the gap after it's let go.
    pub const HOLD: std::time::Duration = std::time::Duration::from_millis(80);
    pub const GAP: std::time::Duration = std::time::Duration::from_millis(40);

    pub fn parse(text: &str) -> Result<Script, String> {
        text.split_whitespace()
            .map(|w| match w.parse::<u64>() {
                Ok(ms) => Ok(Step::Wait(std::time::Duration::from_millis(ms))),
                Err(_) => w
                    .split('+')
                    .map(|c| {
                        CONTROLS
                            .iter()
                            .find(|(n, _)| *n == c)
                            .map(|(_, c)| *c)
                            .ok_or_else(|| format!("unknown control {c:?} in the pad script"))
                    })
                    .collect::<Result<_, _>>()
                    .map(Step::Press),
            })
            .collect::<Result<_, _>>()
            .map(Script)
    }
}

/// Where the window looks for a mapping when `--pad-config` doesn't say:
/// `gamepad.conf` in [`config_dir`].
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("gamepad.conf"))
}

/// `--config-dir`, set once at start-up by [`crate::main`].
static CONFIG_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Makes `dir` the config folder for the rest of the run (`--config-dir`),
/// over the environment [`config_dir_in`] reads. Only the first call counts.
pub fn set_config_dir(dir: PathBuf) {
    let _ = CONFIG_DIR.set(dir);
}

/// The folder of the window's config files (`gamepad.conf`,
/// `settings.conf`): `--config-dir` if given ([`set_config_dir`]), else
/// [`config_dir_in`] the process's environment.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = CONFIG_DIR.get() {
        return Some(dir.clone());
    }
    config_dir_in(|k| std::env::var_os(k))
}

/// The config folder, given how to read an environment variable:
/// `$GBAA_CONFIG_DIR` itself if set (a lane's own, say, keeping it off the
/// owner's), else `$XDG_CONFIG_HOME/gbaa`, else `~/.config/gbaa`; on
/// Windows `%APPDATA%\gbaa` after `GBAA_CONFIG_DIR`.
pub fn config_dir_in(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |k| var(k).filter(|d| !d.is_empty());
    if let Some(dir) = set("GBAA_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        return Some(Path::new(&set("APPDATA")?).join("gbaa"));
    }
    let base = set("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| Path::new(&h).join(".config")))?;
    Some(base.join("gbaa"))
}
