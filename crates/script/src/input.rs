//! mGBA 0.11-dev's `input` (`src/script/input.c` at master, 2026-10):
//! `input:isKeyActive(key)` (a one-character string, or a number: a
//! Unicode codepoint or a `C.KEY` value), `input:activeKeys()`, `input.seq`
//! (the next event's number) and `input.activeGamepad` (always nil: no
//! gamepad reaches scripts yet); and the `key`, `mouseButton`, `mouseMove`
//! and `mouseWheel` callbacks, each given its event, from what the front
//! end hands [`crate::Host::input`]. The constants `C.EV_TYPE`,
//! `C.INPUT_STATE`, `C.INPUT_DIR`, `C.KEY`, `C.KMOD` and `C.MOUSE_BUTTON`
//! are mGBA's.
//!
//! As in mGBA, a key is held while more presses than releases of it have
//! come (`_updateKeys`): a second press while one is held fires as `HELD`,
//! and a release while another press holds fires nothing. Which code a key
//! gives is the front end's (mGBA's Qt one: a letter's capital, a shifted
//! key's character, `C.KEY.SHIFT` for either shift).

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

/// `mSCRIPT_KEYBASE`: where `C.KEY`'s keys that aren't characters start.
const BASE: u32 = 0x80_0000;

/// `C.KEY` (`enum mScriptKey`).
pub const KEYS: &[(&str, u32)] = &[
    ("NONE", 0),
    ("BACKSPACE", 0x08),
    ("TAB", 0x09),
    ("ENTER", 0x0A),
    ("ESCAPE", 0x1B),
    ("DELETE", 0x7F),
    ("F1", BASE | 1),
    ("F2", BASE | 2),
    ("F3", BASE | 3),
    ("F4", BASE | 4),
    ("F5", BASE | 5),
    ("F6", BASE | 6),
    ("F7", BASE | 7),
    ("F8", BASE | 8),
    ("F9", BASE | 9),
    ("F10", BASE | 10),
    ("F11", BASE | 11),
    ("F12", BASE | 12),
    ("F13", BASE | 13),
    ("F14", BASE | 14),
    ("F15", BASE | 15),
    ("F16", BASE | 16),
    ("F17", BASE | 17),
    ("F18", BASE | 18),
    ("F19", BASE | 19),
    ("F20", BASE | 20),
    ("F21", BASE | 21),
    ("F22", BASE | 22),
    ("F23", BASE | 23),
    ("F24", BASE | 24),
    ("UP", BASE | 0x20),
    ("RIGHT", BASE | 0x21),
    ("DOWN", BASE | 0x22),
    ("LEFT", BASE | 0x23),
    ("PAGE_UP", BASE | 0x24),
    ("PAGE_DOWN", BASE | 0x25),
    ("HOME", BASE | 0x26),
    ("END", BASE | 0x27),
    ("INSERT", BASE | 0x28),
    ("BREAK", BASE | 0x29),
    ("CLEAR", BASE | 0x2A),
    ("PRINT_SCREEN", BASE | 0x2B),
    ("SYSRQ", BASE | 0x2C),
    ("MENU", BASE | 0x2D),
    ("HELP", BASE | 0x2E),
    ("LSHIFT", BASE | 0x30),
    ("RSHIFT", BASE | 0x31),
    ("SHIFT", BASE | 0x32),
    ("LCONTROL", BASE | 0x33),
    ("RCONTROL", BASE | 0x34),
    ("CONTROL", BASE | 0x35),
    ("LALT", BASE | 0x36),
    ("RALT", BASE | 0x37),
    ("ALT", BASE | 0x38),
    ("LSUPER", BASE | 0x39),
    ("RSUPER", BASE | 0x3A),
    ("SUPER", BASE | 0x3B),
    ("CAPS_LOCK", BASE | 0x3C),
    ("NUM_LOCK", BASE | 0x3D),
    ("SCROLL_LOCK", BASE | 0x3E),
    ("KP_0", BASE | 0x40),
    ("KP_1", BASE | 0x41),
    ("KP_2", BASE | 0x42),
    ("KP_3", BASE | 0x43),
    ("KP_4", BASE | 0x44),
    ("KP_5", BASE | 0x45),
    ("KP_6", BASE | 0x46),
    ("KP_7", BASE | 0x47),
    ("KP_8", BASE | 0x48),
    ("KP_9", BASE | 0x49),
    ("KP_PLUS", BASE | 0x4A),
    ("KP_MINUS", BASE | 0x4B),
    ("KP_MULTIPLY", BASE | 0x4C),
    ("KP_DIVIDE", BASE | 0x4D),
    ("KP_COMMA", BASE | 0x4E),
    ("KP_POINT", BASE | 0x4F),
    ("KP_ENTER", BASE | 0x50),
];

/// The `C.KEY` value named `name` (as in `C.KEY`, any case).
pub fn key(name: &str) -> Option<u32> {
    KEYS.iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, k)| k)
}

/// `C.KMOD` (`enum mScriptKeyModifier`).
pub mod kmod {
    pub const SHIFT: u16 = 0x3;
    pub const CONTROL: u16 = 0xC;
    pub const ALT: u16 = 0x30;
    pub const SUPER: u16 = 0xC0;
    pub const CAPS_LOCK: u16 = 0x100;
    pub const NUM_LOCK: u16 = 0x200;
    pub const SCROLL_LOCK: u16 = 0x400;
}

const KMODS: &[(&str, u16)] = &[
    ("NONE", 0),
    ("LSHIFT", 0x1),
    ("RSHIFT", 0x2),
    ("SHIFT", kmod::SHIFT),
    ("LCONTROL", 0x4),
    ("RCONTROL", 0x8),
    ("CONTROL", kmod::CONTROL),
    ("LALT", 0x10),
    ("RALT", 0x20),
    ("ALT", kmod::ALT),
    ("LSUPER", 0x40),
    ("RSUPER", 0x80),
    ("SUPER", kmod::SUPER),
    ("CAPS_LOCK", kmod::CAPS_LOCK),
    ("NUM_LOCK", kmod::NUM_LOCK),
    ("SCROLL_LOCK", kmod::SCROLL_LOCK),
];

/// `C.EV_TYPE`, as mGBA exports it (without `GAMEPAD_HAT`).
const EV_TYPES: &[(&str, u32)] = &[
    ("NONE", 0),
    ("KEY", 1),
    ("MOUSE_BUTTON", 2),
    ("MOUSE_MOVE", 3),
    ("MOUSE_WHEEL", 4),
    ("GAMEPAD_BUTTON", 5),
    ("TRIGGER", 7),
];

const DIRS: &[(&str, u32)] = &[
    ("NONE", 0),
    ("NORTH", 1),
    ("EAST", 2),
    ("SOUTH", 4),
    ("WEST", 8),
    ("UP", 1),
    ("RIGHT", 2),
    ("DOWN", 4),
    ("LEFT", 8),
    ("NORTHEAST", 3),
    ("NORTHWEST", 9),
    ("SOUTHEAST", 6),
    ("SOUTHWEST", 12),
];

/// A key's or button's state (`C.INPUT_STATE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Up = 0,
    Down = 1,
    /// Held: the keyboard's repeat, or a press while another holds it.
    Held = 2,
}

/// What the front end saw, for the scripts ([`crate::Host::input`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A key: a codepoint or a [`KEYS`] value, with [`kmod`]'s modifiers.
    Key { key: u32, state: State, mods: u16 },
    /// A mouse button: `C.MOUSE_BUTTON`'s 0 primary, 1 secondary, 2 middle, then 3 on.
    MouseButton { button: u8, down: bool },
    /// The pointer, in game-screen pixels (outside 0..240, 0..160 off it).
    MouseMove { x: i32, y: i32 },
    /// A scroll, as the window gives it.
    MouseWheel { x: i32, y: i32 },
    /// The window lost the keyboard: every held key lets go
    /// (`mScriptContextClearKeys`).
    ClearKeys,
}

/// The `input` global's state.
#[derive(Default)]
pub struct Input {
    seq: Cell<u64>,
    /// Each held key's presses less its releases (above 0).
    active: RefCell<BTreeMap<u32, u32>>,
    /// Where the pointer last was and the buttons held (bit per
    /// `C.MOUSE_BUTTON`), for the BizHawk shim's `input.getmouse`.
    pointer: Cell<(i32, i32)>,
    buttons: Cell<u32>,
}

/// The `input` global.
pub struct InputRef(pub Rc<Input>);

impl UserData for InputRef {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("seq", |_, i| Ok(i.0.seq.get()));
        f.add_field_method_get("activeGamepad", |_, _| Ok(Value::Nil));
        // BizHawk's, called with a dot (the shim, src/bizhawk.lua): the
        // pointer over the game picture and the buttons held.
        f.add_field_method_get("getmouse", |lua, i| {
            let i = i.0.clone();
            lua.create_function(move |lua, _: mlua::Variadic<Value>| {
                let ((x, y), b) = (i.pointer.get(), i.buttons.get());
                let t = lua.create_table()?;
                t.set("X", x)?;
                t.set("Y", y)?;
                let names = ["Left", "Right", "Middle", "XButton1", "XButton2"];
                for (bit, name) in names.into_iter().enumerate() {
                    t.set(name, b & (1 << bit) != 0)?;
                }
                t.set("Wheel", 0)?;
                Ok(t)
            })
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("isKeyActive", |_, i, key: Value| {
            let key = match key {
                Value::String(s) => {
                    let s = s.to_str()?;
                    let mut c = s.chars();
                    match (c.next(), c.next()) {
                        (Some(k), None) => u32::from(k),
                        _ => return Ok(false),
                    }
                }
                Value::Integer(n) => n as u32,
                Value::Number(n) => n as u32,
                _ => return Ok(false),
            };
            Ok(i.0.active.borrow().contains_key(&key))
        });
        m.add_method("activeKeys", |_, i, ()| {
            Ok(i.0.active.borrow().keys().copied().collect::<Vec<u32>>())
        });
    }
}

impl Input {
    /// The events `ev` fires: its callback's name and the event, none for
    /// a release while another press holds the key. Keys are counted as
    /// they go.
    pub(crate) fn events(&self, lua: &Lua, ev: Event) -> mlua::Result<Vec<(&'static str, Table)>> {
        match ev {
            Event::MouseMove { x, y } => self.pointer.set((x, y)),
            Event::MouseButton { button, down } if button < 32 => {
                let b = self.buttons.get() & !(1 << button);
                self.buttons.set(b | u32::from(down) << button);
            }
            _ => {}
        }
        let mut keys = vec![];
        match ev {
            Event::ClearKeys => {
                // mGBA lets each go once, whatever its count.
                let mut active = self.active.borrow_mut();
                for n in active.values_mut() {
                    *n = 1;
                }
                let held: Vec<u32> = active.keys().copied().collect();
                drop(active);
                for key in held {
                    keys.push((key, State::Up, 0));
                }
            }
            Event::Key { key, state, mods } => keys.push((key, state, mods)),
            _ => {}
        }
        let mut out = vec![];
        for (key, mut state, mods) in keys {
            let mut active = self.active.borrow_mut();
            let n = active.get(&key).copied().unwrap_or(0);
            match state {
                State::Up => {
                    if n > 1 {
                        active.insert(key, n - 1);
                        continue;
                    }
                    active.remove(&key);
                }
                State::Down => {
                    active.insert(key, n + 1);
                    if n > 0 {
                        state = State::Held;
                    }
                }
                State::Held => {}
            }
            drop(active);
            let t = self.event(lua, 1)?;
            t.set("state", state as u8)?;
            t.set("modifiers", mods)?;
            t.set("key", key)?;
            out.push(("key", t));
        }
        let mouse = |kind, name| -> mlua::Result<(&'static str, Table)> {
            let t = self.event(lua, kind)?;
            t.set("mouse", 0)?;
            Ok((name, t))
        };
        match ev {
            Event::MouseButton { button, down } => {
                let (name, t) = mouse(2, "mouseButton")?;
                t.set("state", if down { 1 } else { 0 })?;
                t.set("button", button)?;
                out.push((name, t));
            }
            Event::MouseMove { x, y } | Event::MouseWheel { x, y } => {
                let (name, t) = match ev {
                    Event::MouseMove { .. } => mouse(3, "mouseMove")?,
                    _ => mouse(4, "mouseWheel")?,
                };
                t.set("x", x)?;
                t.set("y", y)?;
                out.push((name, t));
            }
            _ => {}
        }
        Ok(out)
    }

    /// A new event of `C.EV_TYPE` `kind`, numbered.
    fn event(&self, lua: &Lua, kind: u32) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        t.set("type", kind)?;
        t.set("seq", self.seq.get())?;
        self.seq.set(self.seq.get() + 1);
        Ok(t)
    }
}

/// Set the `input` global, and `__INPUT_C`: the constants the prelude puts
/// in `C`.
pub fn install(lua: &Lua, input: Rc<Input>) -> mlua::Result<()> {
    let g = lua.globals();
    g.set("input", InputRef(input))?;
    let c = lua.create_table()?;
    let consts = |pairs: &mut dyn Iterator<Item = (&str, u32)>| -> mlua::Result<Table> {
        let t = lua.create_table()?;
        for (k, v) in pairs {
            t.set(k, v)?;
        }
        Ok(t)
    };
    c.set("EV_TYPE", consts(&mut EV_TYPES.iter().copied())?)?;
    let states = [("UP", 0), ("DOWN", 1), ("HELD", 2)];
    c.set("INPUT_STATE", consts(&mut states.into_iter())?)?;
    c.set("INPUT_DIR", consts(&mut DIRS.iter().copied())?)?;
    c.set("KEY", consts(&mut KEYS.iter().copied())?)?;
    let kmods = KMODS.iter().map(|&(k, v)| (k, u32::from(v)));
    c.set("KMOD", consts(&mut kmods.into_iter())?)?;
    let buttons = [("PRIMARY", 0), ("SECONDARY", 1), ("MIDDLE", 2)];
    c.set("MOUSE_BUTTON", consts(&mut buttons.into_iter())?)?;
    g.set("__INPUT_C", c)
}
