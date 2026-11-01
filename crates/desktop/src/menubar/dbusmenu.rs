//! The menu bar ([`super::tree`]) exported as `com.canonical.dbusmenu`,
//! the protocol KDE's global menu reads a non-Qt window's menu with
//! (`docs/research/native-menu-bar.md`, "KDE"). Grown from the spike in
//! `docs/research/native-menu-bar/`.
//!
//! zbus answers the menu on its own thread. A click goes to the frame loop
//! through a channel, with a `wake` call so a loop waiting on SDL's events
//! looks at once; the loop drains it with [`Export::clicks`] and tells the
//! menu what changed with [`Export::set`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use serde::Serialize;
use zbus::blocking::{Connection, connection};
use zbus::zvariant::{OwnedValue, StructureBuilder, Type, Value};

use super::{Action, Item, Menu, State, tree};

/// Where the menu is served, under the connection's unique name.
pub const PATH: &str = "/MenuBar";
const INTERFACE: &str = "com.canonical.dbusmenu";

pub use super::Wake;

/// The ids of rows without an action: top menus from here, then the
/// submenus and separators inside them, numbered depth first. The
/// tree's shape never changes, so neither do they.
const MENU_IDS: i32 = 1000;
const OTHER_IDS: i32 = 2000;

/// One dbusmenu item: its id, properties and children.
#[derive(Debug)]
struct Node {
    id: i32,
    props: HashMap<String, OwnedValue>,
    children: Vec<Node>,
    /// A row's action, and whether it can be chosen now.
    action: Option<(Action, bool)>,
}

fn owned(v: Value<'_>) -> OwnedValue {
    v.try_to_owned()
        .expect("plain values have no file descriptors")
}

/// dbusmenu marks a mnemonic with `_`, so a literal one is doubled.
fn label(text: &str) -> OwnedValue {
    owned(text.replace('_', "__").into())
}

/// A shortcut as [`super::tree`] writes it ("Shift+F1", "Ctrl+Q") in
/// dbusmenu's form: one chord of modifier names, then the key.
pub fn shortcut(keys: &str) -> Vec<Vec<String>> {
    let chord = keys
        .split('+')
        .map(|k| match k {
            "Ctrl" => "Control",
            "Cmd" => "Super",
            k => k,
        })
        .map(String::from)
        .collect();
    vec![chord]
}

fn submenu(id: i32, title: &str, children: Vec<Node>) -> Node {
    let mut props = HashMap::new();
    props.insert("label".into(), label(title));
    props.insert("children-display".into(), owned("submenu".into()));
    Node {
        id,
        props,
        children,
        action: None,
    }
}

fn nodes(items: &[Item], next: &mut i32) -> Vec<Node> {
    items
        .iter()
        .map(|item| match item {
            Item::Row {
                label: text,
                shortcut: keys,
                action,
                enabled,
            } => {
                let mut props = HashMap::new();
                props.insert("label".into(), label(text));
                if !enabled {
                    props.insert("enabled".into(), owned(false.into()));
                }
                if let Some(keys) = keys {
                    props.insert("shortcut".into(), owned(shortcut(keys).into()));
                }
                Node {
                    id: action.id(),
                    props,
                    children: vec![],
                    action: Some((*action, *enabled)),
                }
            }
            Item::Submenu { label, items } => {
                let id = *next;
                *next += 1;
                submenu(id, label, nodes(items, next))
            }
            Item::Separator => {
                let id = *next;
                *next += 1;
                let mut props = HashMap::new();
                props.insert("type".into(), owned("separator".into()));
                Node {
                    id,
                    props,
                    children: vec![],
                    action: None,
                }
            }
        })
        .collect()
}

/// The root (id 0) over `menus`.
fn root(menus: &[Menu]) -> Node {
    let mut next = OTHER_IDS;
    let children = menus
        .iter()
        .zip(MENU_IDS..)
        .map(|(m, id)| submenu(id, m.title, nodes(&m.items, &mut next)))
        .collect();
    let mut root = submenu(0, "", children);
    root.props.remove("label");
    root
}

impl Node {
    fn find(&self, id: i32) -> Option<&Node> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    /// The `(ia{sv}av)` for this node and `depth` levels below it (all
    /// of them for -1), with only the properties `names` asks for (all
    /// when it's empty).
    fn layout(&self, depth: i32, names: &[String]) -> Layout {
        Layout {
            id: self.id,
            props: self.props_named(names),
            children: if depth == 0 {
                vec![]
            } else {
                self.children
                    .iter()
                    .map(|c| c.layout(depth - 1, names).into())
                    .collect()
            },
        }
    }

    fn props_named(&self, names: &[String]) -> HashMap<String, OwnedValue> {
        self.props
            .iter()
            .filter(|(k, _)| names.is_empty() || names.contains(k))
            .map(|(k, v)| (k.clone(), v.try_clone().expect("no file descriptors")))
            .collect()
    }
}

/// One item as dbusmenu sends it.
#[derive(Serialize, Type)]
struct Layout {
    id: i32,
    props: HashMap<String, OwnedValue>,
    children: Vec<Value<'static>>,
}

impl From<Layout> for Value<'static> {
    fn from(l: Layout) -> Self {
        Value::from(
            StructureBuilder::new()
                .add_field(l.id)
                .add_field(l.props)
                .add_field(l.children)
                .build()
                .expect("three fields"),
        )
    }
}

/// The menu as last set, shared by the bus's thread and the frame loop's.
struct Shown {
    state: State,
    root: Node,
    revision: u32,
}

struct MenuBar {
    shown: Arc<Mutex<Shown>>,
    clicks: mpsc::Sender<Action>,
    wake: Wake,
}

impl MenuBar {
    fn shown(&self) -> std::sync::MutexGuard<'_, Shown> {
        self.shown.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hand a click on `id` to the frame loop; false if `id` isn't a row.
    fn click(&self, id: i32) -> bool {
        let action = self.shown().root.find(id).and_then(|n| n.action);
        match action {
            Some((action, enabled)) => {
                if enabled {
                    let _ = self.clicks.send(action);
                    (self.wake)();
                }
                true
            }
            None => false,
        }
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl MenuBar {
    fn get_layout(
        &self,
        parent: i32,
        depth: i32,
        names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let shown = self.shown();
        let node = shown
            .root
            .find(parent)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no item {parent}")))?;
        Ok((shown.revision, node.layout(depth, &names)))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let shown = self.shown();
        ids.into_iter()
            .filter_map(|id| shown.root.find(id).map(|n| (id, n.props_named(&names))))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        self.shown()
            .root
            .find(id)
            .and_then(|n| n.props.get(&name))
            .map(|v| v.try_clone().expect("no file descriptors"))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("item {id} has no {name}")))
    }

    /// Only "clicked" does anything; opening and hovering ask nothing.
    fn event(
        &self,
        id: i32,
        event: String,
        _data: OwnedValue,
        _time: u32,
    ) -> zbus::fdo::Result<()> {
        if event != "clicked" {
            return Ok(());
        }
        if self.click(id) {
            Ok(())
        } else {
            Err(zbus::fdo::Error::InvalidArgs(format!("no row {id}")))
        }
    }

    /// The ids of events whose item isn't there.
    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        events
            .into_iter()
            .filter(|(id, event, _, _)| event == "clicked" && !self.click(*id))
            .map(|(id, ..)| id)
            .collect()
    }

    /// Nothing is built lazily, so nothing changes on opening.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (vec![], vec![])
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> String {
        "ltr".into()
    }
    #[zbus(property)]
    fn status(&self) -> String {
        "normal".into()
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        vec![]
    }
}

/// The menu bar, on the bus while this lives.
pub struct Export {
    conn: Connection,
    shown: Arc<Mutex<Shown>>,
    clicks: mpsc::Receiver<Action>,
}

impl Export {
    /// Serve the menu for `state` on `conn`'s bus at [`PATH`].
    pub fn serve(conn: Connection, state: State, wake: Wake) -> zbus::Result<Export> {
        let shown = Arc::new(Mutex::new(Shown {
            state,
            root: root(&tree(state)),
            revision: 1,
        }));
        let (tx, clicks) = mpsc::channel();
        let bar = MenuBar {
            shown: shown.clone(),
            clicks: tx,
            wake,
        };
        conn.object_server().at(PATH, bar)?;
        Ok(Export {
            conn,
            shown,
            clicks,
        })
    }

    /// Connect to the session bus and serve the menu there.
    pub fn session(state: State, wake: Wake) -> zbus::Result<Export> {
        Export::serve(connection::Builder::session()?.build()?, state, wake)
    }

    /// The bus connection's unique name: the service name a window hands
    /// to the window manager, with [`PATH`].
    pub fn service(&self) -> String {
        self.conn
            .unique_name()
            .map(|n| n.to_string())
            .unwrap_or_default()
    }

    /// The connection the menu is on.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The rows clicked since the last call, oldest first.
    pub fn clicks(&self) -> Vec<Action> {
        self.clicks.try_iter().collect()
    }

    /// Show the menu for `state`; when that changes a row, the menu's
    /// readers hear `LayoutUpdated` and fetch it again.
    pub fn set(&self, state: State) {
        let revision = {
            let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
            if shown.state == state {
                return;
            }
            shown.state = state;
            shown.root = root(&tree(state));
            shown.revision += 1;
            shown.revision
        };
        if let Err(e) = self.conn.emit_signal(
            None::<()>,
            PATH,
            INTERFACE,
            "LayoutUpdated",
            &(revision, 0i32),
        ) {
            eprintln!("gbaa-desktop: the global menu's update: {e}");
        }
    }
}

#[cfg(test)]
mod tests;
