//! gbaa's menu bar as `com.canonical.dbusmenu` (docs/research/native-menu-bar.md).
//!
//! Serves the menu at `/MenuBar` under the connection's unique name and
//! prints each item clicked. In the desktop the same address would go to the
//! window: `org_kde_kwin_appmenu.set_address` on Wayland, or the
//! `_KDE_NET_WM_APPMENU_*` properties on X11. Here a test plays the
//! global menu with busctl (the doc says how).

use std::collections::HashMap;
use std::sync::mpsc;

use serde::Serialize;
use zbus::blocking::connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, StructureBuilder, Type, Value};

/// What a click asks the frame loop to do.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    SaveState,
    LoadState,
    Fullscreen,
    Quit,
}

enum Kind {
    Submenu(Vec<Item>),
    Button(Action),
    Toggle(Action, bool),
    Separator,
}

struct Item {
    label: &'static str,
    /// dbusmenu's `shortcut`: modifier names, then the key.
    keys: &'static [&'static str],
    kind: Kind,
}

fn item(label: &'static str, keys: &'static [&'static str], kind: Kind) -> Item {
    Item { label, keys, kind }
}

fn separator() -> Item {
    item("", &[], Kind::Separator)
}

/// The tree under the root (id 0); ids are depth-first positions from 1.
fn menu(fullscreen: bool) -> Vec<Item> {
    vec![
        item(
            "_Game",
            &[],
            Kind::Submenu(vec![
                item(
                    "_Save State (slot 1)",
                    &["Shift", "F1"],
                    Kind::Button(Action::SaveState),
                ),
                item(
                    "_Load State (slot 1)",
                    &["F1"],
                    Kind::Button(Action::LoadState),
                ),
                separator(),
                item("_Quit", &["Control", "q"], Kind::Button(Action::Quit)),
            ]),
        ),
        item(
            "_View",
            &[],
            Kind::Submenu(vec![item(
                "_Fullscreen",
                &["F11"],
                Kind::Toggle(Action::Fullscreen, fullscreen),
            )]),
        ),
    ]
}

/// The `(ia{sv}av)` dbusmenu sends for each item.
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
                .unwrap(),
        )
    }
}

fn props(it: &Item) -> HashMap<String, OwnedValue> {
    let mut p = HashMap::new();
    let mut put = |k: &str, v: Value<'_>| {
        p.insert(k.to_string(), v.try_to_owned().unwrap());
    };
    match &it.kind {
        Kind::Separator => put("type", "separator".into()),
        Kind::Submenu(_) => put("children-display", "submenu".into()),
        Kind::Button(_) => {}
        Kind::Toggle(_, on) => {
            put("toggle-type", "checkmark".into());
            put("toggle-state", (*on as i32).into());
        }
    }
    if !it.label.is_empty() {
        put("label", it.label.into());
    }
    if !it.keys.is_empty() {
        let keys: Vec<Vec<&str>> = vec![it.keys.to_vec()];
        put("shortcut", keys.into());
    }
    p
}

/// Lays out `items`, numbering from `*next`; `depth` < 0 means all levels.
fn layout(items: &[Item], next: &mut i32, depth: i32) -> Vec<Layout> {
    items
        .iter()
        .map(|it| {
            let id = *next;
            *next += 1;
            let mut children = Vec::new();
            if let Kind::Submenu(sub) = &it.kind {
                let kids = layout(sub, next, depth - 1);
                if depth != 1 {
                    children = kids.into_iter().map(Value::from).collect();
                }
            }
            Layout {
                id,
                props: props(it),
                children,
            }
        })
        .collect()
}

/// Every item by id, flattened.
fn find(
    items: &[Item],
    next: &mut i32,
    id: i32,
) -> Option<(Option<Action>, HashMap<String, OwnedValue>, Vec<Layout>)> {
    for it in items {
        let me = *next;
        *next += 1;
        let sub = match &it.kind {
            Kind::Submenu(sub) => sub.as_slice(),
            _ => &[],
        };
        if me == id {
            let action = match it.kind {
                Kind::Button(a) | Kind::Toggle(a, _) => Some(a),
                _ => None,
            };
            let kids = layout(sub, next, -1);
            return Some((action, props(it), kids));
        }
        if let Some(found) = find(sub, next, id) {
            return Some(found);
        }
    }
    None
}

struct MenuBar {
    fullscreen: bool,
    revision: u32,
    clicks: mpsc::Sender<Action>,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl MenuBar {
    fn get_layout(
        &self,
        parent: i32,
        depth: i32,
        _names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let items = menu(self.fullscreen);
        let root = if parent == 0 {
            let kids = layout(&items, &mut 1, depth);
            let mut p = HashMap::new();
            p.insert(
                "children-display".into(),
                Value::from("submenu").try_to_owned().unwrap(),
            );
            Layout {
                id: 0,
                props: p,
                children: if depth == 0 {
                    vec![]
                } else {
                    kids.into_iter().map(Value::from).collect()
                },
            }
        } else {
            let (_, props, kids) = find(&items, &mut 1, parent)
                .ok_or_else(|| zbus::fdo::Error::InvalidArgs("no such id".into()))?;
            Layout {
                id: parent,
                props,
                children: kids.into_iter().map(Value::from).collect(),
            }
        };
        Ok((self.revision, root))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let items = menu(self.fullscreen);
        ids.into_iter()
            .filter_map(|id| find(&items, &mut 1, id).map(|(_, p, _)| (id, p)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let items = menu(self.fullscreen);
        find(&items, &mut 1, id)
            .and_then(|(_, mut p, _)| p.remove(&name))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("no such property".into()))
    }

    async fn event(
        &mut self,
        id: i32,
        event: String,
        _data: OwnedValue,
        _time: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if event != "clicked" {
            return Ok(());
        }
        let items = menu(self.fullscreen);
        let Some((Some(action), _, _)) = find(&items, &mut 1, id) else {
            return Err(zbus::fdo::Error::InvalidArgs("not a button".into()));
        };
        if action == Action::Fullscreen {
            // In the desktop the frame loop flips it and calls this back.
            self.fullscreen = !self.fullscreen;
            self.revision += 1;
            Self::layout_updated(&emitter, self.revision, 0).await?;
        }
        let _ = self.clicks.send(action);
        Ok(())
    }

    fn event_group(&self, _events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        // Qt's menus never send these; one Event at a time is enough here.
        vec![]
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
        emitter: &SignalEmitter<'_>,
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

fn main() -> zbus::Result<()> {
    let (tx, rx) = mpsc::channel();
    let bar = MenuBar {
        fullscreen: false,
        revision: 1,
        clicks: tx,
    };
    let conn = connection::Builder::session()?
        .serve_at("/MenuBar", bar)?
        .build()?;
    // The address a window would hand to KWin.
    println!("{} /MenuBar", conn.unique_name().unwrap());
    // The frame loop would drain this once per frame with try_recv.
    for action in rx {
        println!("clicked {action:?}");
        if action == Action::Quit {
            break;
        }
    }
    Ok(())
}
