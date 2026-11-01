//! The menu bar on macOS: [`super::tree`] as the app's NSMenu, through
//! muda (`docs/research/native-menu-bar.md`, "macOS: NSMenu through
//! muda").
//!
//! SDL builds its own main menu while video starts (the app menu and a
//! Window menu). [`MenuBar::start`], called once the window exists,
//! replaces it whole, so the app and Window menus are rebuilt here from
//! muda's predefined items. Everything here runs on the main thread, as
//! muda wants on macOS: the frame loop's.
//!
//! A click runs inside `SDL_PumpEvents` (AppKit dispatches it from
//! `[NSApp sendEvent:]`); muda hands it to our handler, which queues its
//! [`Action`] and wakes the frame loop, which drains [`MenuBar::clicks`]
//! once a frame.
//!
//! Only Quit has a key equivalent (Cmd+Q), so AppKit takes that key
//! before SDL sees it; Quit is in the app menu, where macOS keeps it, and
//! not in Game. Every other row names its hotkey in its text and leaves
//! the key to the window's own handling, so a press acts once.

use std::cell::Cell;
use std::sync::mpsc;

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use super::{Action, Item, State, Wake, tree};

/// The app's menu bar, while this lives.
pub struct MenuBar {
    _menu: muda::Menu,
    /// Every row with an action, to change its text and whether it's
    /// enabled when the [`State`] does.
    rows: Vec<(Action, MenuItem)>,
    clicks: mpsc::Receiver<Action>,
    state: Cell<State>,
}

impl MenuBar {
    /// Make the menu bar for `state` the app's. Call it after SDL has
    /// made the window, on the main thread.
    pub fn start(state: State, wake: Wake) -> Option<MenuBar> {
        let (tx, clicks) = mpsc::channel();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some(a) = e.id.0.parse().ok().and_then(Action::from_id) {
                let _ = tx.send(a);
                wake();
            }
        }));
        let menu = muda::Menu::new();
        let mut rows = Vec::new();
        let result = (|| -> muda::Result<Submenu> {
            menu.append(&app_menu(&mut rows)?)?;
            for m in tree(state) {
                let sub = Submenu::new(m.title, true);
                append(&sub, &for_macos(m.items), &mut rows)?;
                menu.append(&sub)?;
            }
            let window = Submenu::with_items(
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(None),
                    &PredefinedMenuItem::maximize(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::bring_all_to_front(None),
                ],
            )?;
            menu.append(&window)?;
            Ok(window)
        })();
        let window = result
            .inspect_err(|e| eprintln!("gbaa-desktop: no menu bar: {e}"))
            .ok()?;
        menu.init_for_nsapp();
        // Only once the menu is the app's: AppKit ignores it before.
        window.set_as_windows_menu_for_nsapp();
        Some(MenuBar {
            _menu: menu,
            rows,
            clicks,
            state: Cell::new(state),
        })
    }

    /// The rows clicked since the last call.
    pub fn clicks(&self) -> Vec<Action> {
        self.clicks.try_iter().collect()
    }

    /// Show the menu for `state`.
    pub fn set(&self, state: State) {
        if self.state.replace(state) == state {
            return;
        }
        let mut now = Vec::new();
        for m in tree(state) {
            texts(&for_macos(m.items), &mut now);
        }
        for (action, text, enabled) in now {
            if let Some((_, item)) = self.rows.iter().find(|(a, _)| *a == action) {
                if item.text() != text {
                    item.set_text(text);
                }
                if item.is_enabled() != enabled {
                    item.set_enabled(enabled);
                }
            }
        }
    }
}

/// The app menu (titled with the app's name by macOS): About, Services,
/// Hide and Quit, what SDL's had.
fn app_menu(rows: &mut Vec<(Action, MenuItem)>) -> muda::Result<Submenu> {
    let quit = MenuItem::with_id(
        Action::Quit.id().to_string(),
        "Quit",
        true,
        Some(Accelerator::new(Some(Modifiers::SUPER), Code::KeyQ)),
    );
    let app = Submenu::with_items(
        "gbaa",
        true,
        &[
            &PredefinedMenuItem::about(None, None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &quit,
        ],
    )?;
    rows.push((Action::Quit, quit));
    Ok(app)
}

/// `items` without Quit (the app menu has it) or a separator left last.
fn for_macos(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = items
        .into_iter()
        .filter(|i| {
            !matches!(
                i,
                Item::Row {
                    action: Action::Quit,
                    ..
                }
            )
        })
        .collect();
    while matches!(out.last(), Some(Item::Separator)) {
        out.pop();
    }
    out
}

/// A row's text: its label, then the hotkey it only names.
fn text(label: &str, shortcut: Option<&str>) -> String {
    match shortcut {
        Some(k) => format!("{label} ({k})"),
        None => label.to_owned(),
    }
}

fn append(to: &Submenu, items: &[Item], rows: &mut Vec<(Action, MenuItem)>) -> muda::Result<()> {
    for i in items {
        match i {
            Item::Row {
                label,
                shortcut,
                action,
                enabled,
            } => {
                let item = MenuItem::with_id(
                    action.id().to_string(),
                    text(label, *shortcut),
                    *enabled,
                    None,
                );
                to.append(&item)?;
                rows.push((*action, item));
            }
            Item::Submenu { label, items } => {
                let sub = Submenu::new(label, true);
                append(&sub, items, rows)?;
                to.append(&sub)?;
            }
            Item::Separator => to.append(&PredefinedMenuItem::separator())?,
        }
    }
    Ok(())
}

/// Every row's action, text and whether it's enabled, depth first.
fn texts(items: &[Item], out: &mut Vec<(Action, String, bool)>) {
    for i in items {
        match i {
            Item::Row {
                label,
                shortcut,
                action,
                enabled,
            } => out.push((*action, text(label, *shortcut), *enabled)),
            Item::Submenu { items, .. } => texts(items, out),
            Item::Separator => {}
        }
    }
}
