//! The native menu bar's items and what choosing one does, shared by the
//! macOS menu (muda) and KDE's global menu (dbusmenu), which only draw
//! [`tree`] and hand back the [`Action`] of the row clicked
//! (`docs/research/native-menu-bar.md`, "The plan"). Nothing here needs a
//! window.
//!
//! Each row does what a hotkey already does, so the frame loop
//! ([`crate::window`]) maps an action onto the same flags the keys set
//! ([`Action::ask`]): `slot` (F1–F4, Shift for save), `undo` (F5),
//! `menu.fullscreen_asked` (F11) and `quit` (Ctrl+Q).

use crate::slots::SLOTS;

#[cfg(target_os = "linux")]
pub mod dbusmenu;
#[cfg(target_os = "linux")]
pub mod kde;
#[cfg(target_os = "macos")]
pub mod macos;

/// What wakes the frame loop after a click.
pub type Wake = Box<dyn Fn() + Send + Sync>;

/// What a menu bar row asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Save to slot 1 to [`SLOTS`] (Shift+F1–F4).
    Save(u8),
    /// Load slot 1 to [`SLOTS`] (F1–F4).
    Load(u8),
    /// Put back the game as it was before the last load (F5).
    UndoLoad,
    /// Switch fullscreen on or off (F11).
    Fullscreen,
    /// Close the game (Ctrl+Q).
    Quit,
}

/// The frame loop's flags an [`Action`] sets, the ones its keys set.
#[derive(Debug)]
pub struct Asks<'a> {
    /// A slot asked for, and whether to save to it (else load).
    pub slot: &'a mut Option<(u8, bool)>,
    pub undo: &'a mut bool,
    /// The menu's [`crate::menu::Menu::fullscreen_asked`].
    pub fullscreen: &'a mut bool,
    pub quit: &'a mut bool,
}

impl Action {
    /// Set the flag the action's hotkey sets. A slot outside 1 to
    /// [`SLOTS`] asks nothing.
    pub fn ask(self, to: Asks) {
        match self {
            Action::Save(s) | Action::Load(s) if !(1..=SLOTS).contains(&s) => {}
            Action::Save(s) => *to.slot = Some((s, true)),
            Action::Load(s) => *to.slot = Some((s, false)),
            Action::UndoLoad => *to.undo = true,
            Action::Fullscreen => *to.fullscreen = true,
            Action::Quit => *to.quit = true,
        }
    }

    /// The action's number, for menus that name rows by an integer
    /// (dbusmenu's ids, muda's string ids): never 0, which dbusmenu
    /// keeps for the root.
    pub fn id(self) -> i32 {
        match self {
            Action::Save(s) => 100 + i32::from(s),
            Action::Load(s) => 200 + i32::from(s),
            Action::UndoLoad => 1,
            Action::Fullscreen => 2,
            Action::Quit => 3,
        }
    }

    /// The action [`Action::id`] numbered `id`, if any.
    pub fn from_id(id: i32) -> Option<Action> {
        let slot = |base: i32| {
            u8::try_from(id - base)
                .ok()
                .filter(|s| (1..=SLOTS).contains(s))
        };
        match id {
            1 => Some(Action::UndoLoad),
            2 => Some(Action::Fullscreen),
            3 => Some(Action::Quit),
            101..=199 => slot(100).map(Action::Save),
            201..=299 => slot(200).map(Action::Load),
            _ => None,
        }
    }
}

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A row to click: its label, the hotkey shown beside it (only
    /// shown: the window's own key handling does the key), and whether it
    /// can be chosen now.
    Row {
        label: String,
        shortcut: Option<&'static str>,
        action: Action,
        enabled: bool,
    },
    /// A row that opens more rows.
    Submenu {
        label: String,
        items: Vec<Item>,
    },
    Separator,
}

/// A top-level menu: its title and rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub title: &'static str,
    pub items: Vec<Item>,
}

/// What the rows depend on, from the frame loop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// There's a state load to undo ([`crate::menu::Menu::undoable`]).
    pub undoable: bool,
    /// The window is fullscreen: Fullscreen's row says how to leave it.
    pub fullscreen: bool,
}

/// The slot keys' names, slot 1 first: what [`crate::window`]'s
/// `SLOT_KEYS` are.
const SLOT_KEY_NAMES: [&str; SLOTS as usize] = ["F1", "F2", "F3", "F4"];
const SAVE_KEY_NAMES: [&str; SLOTS as usize] = ["Shift+F1", "Shift+F2", "Shift+F3", "Shift+F4"];

fn row(label: impl Into<String>, shortcut: &'static str, action: Action) -> Item {
    Item::Row {
        label: label.into(),
        shortcut: Some(shortcut),
        action,
        enabled: true,
    }
}

/// The quit key as the platform writes it ([`crate::window::quits`]).
fn quit_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+Q"
    } else {
        "Ctrl+Q"
    }
}

/// The menu bar: Game (Save State and Load State, a row per slot; Undo
/// Load; Quit) and View (Fullscreen).
pub fn tree(state: State) -> Vec<Menu> {
    let slots = |names: &[&'static str], to: fn(u8) -> Action| {
        (1..=SLOTS)
            .zip(names)
            .map(|(s, &key)| row(format!("Slot {s}"), key, to(s)))
            .collect()
    };
    let undo = Item::Row {
        label: "Undo Load State".into(),
        shortcut: Some("F5"),
        action: Action::UndoLoad,
        enabled: state.undoable,
    };
    let fullscreen = if state.fullscreen {
        "Leave Fullscreen"
    } else {
        "Fullscreen"
    };
    vec![
        Menu {
            title: "Game",
            items: vec![
                Item::Submenu {
                    label: "Save State".into(),
                    items: slots(&SAVE_KEY_NAMES, Action::Save),
                },
                Item::Submenu {
                    label: "Load State".into(),
                    items: slots(&SLOT_KEY_NAMES, Action::Load),
                },
                undo,
                Item::Separator,
                row("Quit", quit_key(), Action::Quit),
            ],
        },
        Menu {
            title: "View",
            items: vec![row(fullscreen, "F11", Action::Fullscreen)],
        },
    ]
}

/// Every row's action in `menus`, depth first, with whether it's enabled.
pub fn actions(menus: &[Menu]) -> Vec<(Action, bool)> {
    fn walk(items: &[Item], out: &mut Vec<(Action, bool)>) {
        for i in items {
            match i {
                Item::Row {
                    action, enabled, ..
                } => out.push((*action, *enabled)),
                Item::Submenu { items, .. } => walk(items, out),
                Item::Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    for m in menus {
        walk(&m.items, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq)]
    struct Flags {
        slot: Option<(u8, bool)>,
        undo: bool,
        fullscreen: bool,
        quit: bool,
    }

    fn asked(a: Action) -> Flags {
        let mut f = Flags::default();
        a.ask(Asks {
            slot: &mut f.slot,
            undo: &mut f.undo,
            fullscreen: &mut f.fullscreen,
            quit: &mut f.quit,
        });
        f
    }

    #[test]
    fn actions_set_what_their_hotkeys_set() {
        assert_eq!(
            asked(Action::Save(2)),
            Flags {
                slot: Some((2, true)),
                ..Flags::default()
            }
        );
        assert_eq!(
            asked(Action::Load(4)),
            Flags {
                slot: Some((4, false)),
                ..Flags::default()
            }
        );
        assert_eq!(
            asked(Action::UndoLoad),
            Flags {
                undo: true,
                ..Flags::default()
            }
        );
        assert_eq!(
            asked(Action::Fullscreen),
            Flags {
                fullscreen: true,
                ..Flags::default()
            }
        );
        assert_eq!(
            asked(Action::Quit),
            Flags {
                quit: true,
                ..Flags::default()
            }
        );
        assert_eq!(asked(Action::Load(0)), Flags::default());
        assert_eq!(asked(Action::Save(SLOTS + 1)), Flags::default());
    }

    #[test]
    fn the_tree_has_every_action_once() {
        let rows = actions(&tree(State::default()));
        let mut want = vec![];
        want.extend((1..=SLOTS).map(Action::Save));
        want.extend((1..=SLOTS).map(Action::Load));
        want.extend([Action::UndoLoad, Action::Quit, Action::Fullscreen]);
        let got: Vec<Action> = rows.iter().map(|&(a, _)| a).collect();
        assert_eq!(got, want);
        let titles: Vec<_> = tree(State::default()).iter().map(|m| m.title).collect();
        assert_eq!(titles, ["Game", "View"]);
    }

    #[test]
    fn undo_is_enabled_only_with_a_load_to_undo() {
        let enabled = |s| {
            actions(&tree(s))
                .into_iter()
                .find(|&(a, _)| a == Action::UndoLoad)
                .unwrap()
                .1
        };
        assert!(!enabled(State::default()));
        assert!(enabled(State {
            undoable: true,
            ..State::default()
        }));
        // Every other row is always enabled.
        assert!(
            actions(&tree(State::default()))
                .iter()
                .all(|&(a, on)| on || a == Action::UndoLoad)
        );
    }

    #[test]
    fn fullscreen_row_says_which_way() {
        let label = |s| match &tree(s)[1].items[0] {
            Item::Row { label, .. } => label.clone(),
            i => panic!("{i:?}"),
        };
        assert_eq!(label(State::default()), "Fullscreen");
        assert_eq!(
            label(State {
                fullscreen: true,
                ..State::default()
            }),
            "Leave Fullscreen"
        );
    }

    #[test]
    fn shortcuts_name_the_hotkeys() {
        let menus = tree(State::default());
        let Item::Submenu { items, .. } = &menus[0].items[0] else {
            panic!()
        };
        assert!(matches!(
            &items[0],
            Item::Row { label, shortcut: Some("Shift+F1"), action: Action::Save(1), .. }
                if label == "Slot 1"
        ));
        let Item::Submenu { items, .. } = &menus[0].items[1] else {
            panic!()
        };
        assert!(matches!(
            &items[3],
            Item::Row {
                shortcut: Some("F4"),
                action: Action::Load(4),
                ..
            }
        ));
    }

    #[test]
    fn slot_key_names_are_the_slot_keys() {
        for (k, name) in crate::window::SLOT_KEYS.iter().zip(SLOT_KEY_NAMES) {
            assert_eq!(k.name(), name);
        }
        for (load, save) in SLOT_KEY_NAMES.iter().zip(SAVE_KEY_NAMES) {
            assert_eq!(format!("Shift+{load}"), save);
        }
    }

    #[test]
    fn ids_round_trip_and_never_zero() {
        for (a, _) in actions(&tree(State::default())) {
            assert_ne!(a.id(), 0);
            assert_eq!(Action::from_id(a.id()), Some(a), "{a:?}");
        }
        for id in [0, 4, 100, 105, 200, 299, -1] {
            assert_eq!(Action::from_id(id), None, "{id}");
        }
        let mut ids: Vec<_> = actions(&tree(State::default()))
            .iter()
            .map(|(a, _)| a.id())
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 2 * SLOTS as usize + 3);
    }
}
