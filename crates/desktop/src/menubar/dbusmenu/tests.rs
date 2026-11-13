//! The global menu walked and clicked as Plasma's reader does it, over a
//! private `dbus-daemon`: the card's check for KDE's half of the menu bar.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use zbus::blocking::{Connection, MessageIterator, Proxy, connection};
use zbus::zvariant::{OwnedValue, Structure, Value};

use super::*;

/// A session bus of the test's own, gone when this drops.
struct Bus {
    daemon: Child,
    address: String,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// None (and a line saying why) where no `dbus-daemon` is installed.
fn bus() -> Option<Bus> {
    let daemon = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
        .stdout(Stdio::piped())
        .stdin(Stdio::null())
        .spawn();
    let mut daemon = match daemon {
        Ok(d) => d,
        Err(e) => {
            eprintln!("skipped: no dbus-daemon to run a private bus ({e})");
            return None;
        }
    };
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim().to_string();
    assert!(!address.is_empty(), "dbus-daemon printed no address");
    Some(Bus { daemon, address })
}

fn connect(bus: &Bus) -> Connection {
    connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap()
}

/// An item as the reader sees it.
#[derive(Debug)]
struct Seen {
    id: i32,
    props: HashMap<String, OwnedValue>,
    children: Vec<Seen>,
}

impl Seen {
    fn label(&self) -> String {
        self.str("label")
    }

    fn str(&self, name: &str) -> String {
        self.props
            .get(name)
            .map(|v| String::try_from(v.try_clone().unwrap()).unwrap())
            .unwrap_or_default()
    }

    fn labels(&self) -> Vec<String> {
        self.children.iter().map(Seen::label).collect()
    }

    fn child(&self, label: &str) -> &Seen {
        self.children
            .iter()
            .find(|c| c.label() == label)
            .unwrap_or_else(|| panic!("no {label:?} in {:?}", self.labels()))
    }
}

fn seen(v: &Value<'_>) -> Seen {
    let v = match v {
        Value::Value(inner) => inner.as_ref(),
        v => v,
    };
    let s: &Structure = match v {
        Value::Structure(s) => s,
        v => panic!("not an item: {v:?}"),
    };
    let [id, props, children] = s.fields() else {
        panic!("not (ia{{sv}}av): {s:?}")
    };
    let Value::I32(id) = id else { panic!("{id:?}") };
    let props = HashMap::<String, OwnedValue>::try_from(props.try_clone().unwrap()).unwrap();
    let Value::Array(children) = children else {
        panic!("{children:?}")
    };
    Seen {
        id: *id,
        props,
        children: children.iter().map(seen).collect(),
    }
}

/// An item as GetLayout sends it, its children still variants.
type Wire = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn layout(menu: &Proxy, parent: i32, depth: i32) -> (u32, Seen) {
    let reply = menu
        .call_method("GetLayout", &(parent, depth, Vec::<String>::new()))
        .unwrap();
    let (revision, (id, props, children)): (u32, Wire) = reply.body().deserialize().unwrap();
    let children = children.iter().map(|c| seen(c)).collect();
    (
        revision,
        Seen {
            id,
            props,
            children,
        },
    )
}

fn click(menu: &Proxy, id: i32) -> zbus::Result<()> {
    menu.call_method("Event", &(id, "clicked", Value::from(0i32), 0u32))
        .map(|_| ())
}

#[test]
fn shortcuts_in_dbusmenu_form() {
    assert_eq!(shortcut("Shift+F1"), [["Shift", "F1"]]);
    assert_eq!(shortcut("Ctrl+Q"), [["Control", "Q"]]);
    assert_eq!(shortcut("F11"), [["F11"]]);
}

#[test]
fn a_private_bus_walks_and_clicks_the_menu() {
    let Some(bus) = bus() else { return };
    let woken = Arc::new(AtomicUsize::new(0));
    let w = woken.clone();
    let export = Export::serve(
        connect(&bus),
        State::default(),
        Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    let reader = connect(&bus);
    let menu = Proxy::new(&reader, export.service(), PATH, INTERFACE).unwrap();
    let version: u32 = menu.get_property("Version").unwrap();
    assert_eq!(version, 3);

    // The whole tree, as the panel first asks for it.
    let (revision, root) = layout(&menu, 0, -1);
    assert_eq!(revision, 1);
    assert_eq!(root.id, 0);
    assert_eq!(root.str("children-display"), "submenu");
    assert_eq!(root.labels(), ["Game", "View"]);
    let game = root.child("Game");
    assert_eq!(game.str("children-display"), "submenu");
    assert_eq!(
        game.labels(),
        ["Save State", "Load State", "Undo Load State", "", "Quit"]
    );
    assert_eq!(game.children[3].str("type"), "separator");
    let save = game.child("Save State");
    assert_eq!(save.labels(), ["Slot 1", "Slot 2", "Slot 3", "Slot 4"]);
    let shortcut = save.child("Slot 2").props["shortcut"].try_clone().unwrap();
    assert_eq!(
        Vec::<Vec<String>>::try_from(shortcut).unwrap(),
        [["Shift", "F2"]]
    );
    let undo = game.child("Undo Load State");
    assert!(!bool::try_from(undo.props["enabled"].try_clone().unwrap()).unwrap());
    let view = root.child("View");
    assert_eq!(view.labels(), ["Fullscreen"]);

    // Every id names one item.
    fn ids(s: &Seen, out: &mut Vec<i32>) {
        out.push(s.id);
        s.children.iter().for_each(|c| ids(c, out));
    }
    let mut all = vec![];
    ids(&root, &mut all);
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n, "{all:?}");

    // One level at a time, as a submenu is opened.
    let (_, top) = layout(&menu, 0, 1);
    assert!(top.children.iter().all(|c| c.children.is_empty()));
    let (_, opened) = layout(&menu, save.id, -1);
    assert_eq!(opened.labels(), save.labels());
    let named = menu
        .call_method("GetProperty", &(view.children[0].id, "label"))
        .unwrap();
    let named: OwnedValue = named.body().deserialize().unwrap();
    assert_eq!(String::try_from(named).unwrap(), "Fullscreen");

    // Clicks reach the frame loop and wake it.
    assert!(export.clicks().is_empty());
    click(&menu, save.child("Slot 2").id).unwrap();
    click(&menu, game.child("Load State").child("Slot 4").id).unwrap();
    click(&menu, game.child("Quit").id).unwrap();
    assert_eq!(
        export.clicks(),
        [Action::Save(2), Action::Load(4), Action::Quit]
    );
    assert_eq!(woken.load(Ordering::SeqCst), 3);
    // A disabled row and a submenu's title ask nothing; no item is an
    // error.
    click(&menu, undo.id).unwrap();
    click(&menu, save.id).unwrap_err();
    click(&menu, 9999).unwrap_err();
    // Hovering isn't clicking.
    menu.call_method(
        "Event",
        &(view.children[0].id, "hovered", Value::from(0i32), 0u32),
    )
    .unwrap();
    let unknown: Vec<i32> = menu
        .call_method(
            "EventGroup",
            &(vec![
                (view.children[0].id, "clicked", Value::from(0i32), 0u32),
                (9999, "clicked", Value::from(0i32), 0u32),
            ],),
        )
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    assert_eq!(unknown, [9999]);
    assert_eq!(export.clicks(), [Action::Fullscreen]);
    assert_eq!(woken.load(Ordering::SeqCst), 4);

    // A change the frame loop sets is announced, and read back.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INTERFACE)
        .unwrap()
        .member("LayoutUpdated")
        .unwrap()
        .build();
    let mut updates = MessageIterator::for_match_rule(rule, &reader, None).unwrap();
    export.set(State::default());
    export.set(State {
        undoable: true,
        fullscreen: true,
        native: false,
    });
    let update = updates.next().unwrap().unwrap();
    let (revision, parent): (u32, i32) = update.body().deserialize().unwrap();
    assert_eq!((revision, parent), (2, 0));
    let (revision, root) = layout(&menu, 0, -1);
    assert_eq!(revision, 2);
    assert_eq!(root.child("View").labels(), ["Leave Fullscreen"]);
    let undo = root.child("Game").child("Undo Load State");
    assert!(!undo.props.contains_key("enabled"));
    click(&menu, undo.id).unwrap();
    assert_eq!(export.clicks(), [Action::UndoLoad]);
}
