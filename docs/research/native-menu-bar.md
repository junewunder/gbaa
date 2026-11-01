# A native menu bar: macOS and KDE's global menu

research/native-menu-bar, 2026-10-30. june (2026-10-29) wants a real menu
bar, with Save State, Load State, Fullscreen, Quit and the like, on macOS
and on Linux, where she uses KDE's global menu (and almost nothing
supports it). The question: how to get one from our SDL3 window.

**Answer:** SDL can't do it for us, so we build the menu twice, behind one
small `menubar` module in `desktop` that turns clicks into the same actions
the hotkeys already trigger:

- **macOS:** `muda`, a macOS-only dependency. `Menu::init_for_nsapp()`
  after SDL makes the window replaces the NSApp main menu SDL built. About
  100 lines. It can't be seen working without june's Mac.
- **Linux (KDE):** our own `com.canonical.dbusmenu` export over `zbus`,
  plus a pointer from the window to it. On Wayland the pointer is the
  `org_kde_kwin_appmenu` protocol, sent on SDL's own `wl_display`. On X11
  it's two window properties. About 350 lines. The spike here
  (`docs/research/native-menu-bar/`) is the dbusmenu half, and it works.
- The in-game menu (F10/Escape, egui) stays as it is. The Deck, GNOME
  and Windows have no menu bar for us to fill, and the menu bar only
  duplicates what that menu and the hotkeys already do.

## What SDL gives

Nothing usable yet. SDL 3.4 (we build 3.4.18) has a **tray** API
(`SDL_CreateTray`, `SDL_CreateTrayMenu`), which on Linux dlopens
libappindicator/GTK. That's a status-area icon, not a window's menu bar.
A native menu bar API is SDL PR #13752 (playmer, opened 2025-08,
issue #9590). It covers Win32 and Cocoa only, and was still open for
feedback in January 2026 ([forum thread][sdl-forum]). Its author hasn't
settled on a Linux approach (GTK, or GIO menus through portals were
mentioned), and nobody has mentioned dbusmenu. If it lands, the macOS half
below could move onto it. The KDE half would still be ours.

What SDL does give is each window's native handles, as window properties:

| Backend | Properties |
|---|---|
| Cocoa | `SDL.window.cocoa.window` (the NSWindow) |
| Wayland | `SDL.window.wayland.display`, `SDL.window.wayland.surface` |
| X11 | `SDL.window.x11.display`, `SDL.window.x11.window` |

`sdl3::video::Window` exposes these through `SDL_GetWindowProperties`
(`sdl3::sys`).

## macOS: NSMenu through muda

SDL's Cocoa backend already builds a main menu in `Cocoa_RegisterApp`
(`src/video/cocoa/SDL_cocoaevents.m`), but only if `[NSApp mainMenu]` is
nil and no `MainMenu.nib` is bundled. It has the application menu (About,
Preferences… with no action, Services, Hide, Quit ⌘Q) and a Window menu
(Close ⌘W, Minimize ⌘M, Zoom, Toggle Full Screen ⌃⌘F). So a Mac build
already shows a menu bar, just none of ours.

The NSMenu is global to the app, not per window, so any toolkit can
replace it. **muda** (tauri's menu crate, 0.19) does this with
`Menu::init_for_nsapp()`, and it needs no tao or winit. Its winit example
calls it after the event loop and window exist. With SDL that's after
`video.window(...).build()`, because SDL's `Cocoa_RegisterApp` runs during
video init and would otherwise have set its own menu. Our menu replaces
SDL's whole, so ours has to rebuild the application menu with muda's
`PredefinedMenuItem`s (about, services, hide, hide_others, show_all,
quit) and the Window menu with `set_as_windows_menu_for_nsapp()`, after
`init_for_nsapp` (registering it before is silently ignored).

Things to watch for:

- **Main thread only.** muda panics off it on macOS. Our window and frame
  loop are on the main thread already. The guest runs on a coroutine
  (`src/coro.rs`), not a thread.
- **Events.** A click calls NSMenuItem's action, which AppKit dispatches
  inside `[NSApp sendEvent:]`, and SDL calls that from `SDL_PumpEvents`. So
  `MenuEvent::receiver().try_recv()` once a frame, after the SDL event
  poll, sees it.
- **Key equivalents.** AppKit matches a menu's key equivalents before the
  key reaches SDL. Give an item an accelerator only where we want AppKit to
  own the key: Cmd+Q, Cmd+H, and Ctrl+Cmd+F if Fullscreen goes in the
  Window menu. Leave F1–F4, F5, F11 as labels in the item's text, or our
  own key handling and the menu would both act on one press.
- **Fullscreen.** Our own item should set `menu.fullscreen_asked`, the
  path F11 takes (`window.rs`). It shouldn't be muda's predefined
  `fullscreen` item, which calls `toggleFullScreen:` behind our settings'
  back.
- **Dependency hygiene.** muda's default `gtk` feature pulls GTK3 and
  libxdo on Linux. Depend on it only under
  `[target.'cfg(target_os = "macos")'.dependencies]`, so Linux and the
  Deck build nothing new.

Building our own items with `objc2-app-kit` instead of muda is about the
same size (an `NSObject` subclass as the target, `define_class!`). muda
saves the selector plumbing and its accelerator parsing, so it's the
better first step. thomashope's `native-menu-bar` C library, suggested
on SDL's forum as a stop-gap, adds a C build for what muda already does.

## KDE: dbusmenu plus the appmenu pointer

KDE's global menu (the Global Menu applet, or the title bar's
application menu button) doesn't take a menu from the window system. The
app **exports its menu on the session bus** as `com.canonical.dbusmenu`,
the protocol Canonical made for Unity. It then **tells the window manager
where that is**. Plasma's `appmenu` kded module and the applet read it
from KWin and draw it. On quack the pieces are running: kded6 owns
`org.kde.kappmenu` and `com.canonical.AppMenu.Registrar`, and plasmashell
owns `org.kde.kappmenuview`.

How the window says where its menu is depends on SDL's backend:

- **Wayland:** KWin's `org_kde_kwin_appmenu_manager` global
  ([protocol][appmenu-xml], KWin only, not Mutter, Sway or Hyprland).
  `create(wl_surface)` gives an `org_kde_kwin_appmenu`, then
  `set_address(service_name, object_path)`. The menu must be on the bus
  before `set_address`. To get to SDL's surface from Rust:
  `wayland_backend::client::Backend::from_foreign_display(display_ptr)`
  (wayland-backend's `client_system` feature, with wayland-sys's `dlopen`
  so we still link no system library). Then `ObjectId::from_ptr(
  WlSurface::interface(), surface_ptr)` to make SDL's surface a proxy, and
  bind the manager on our own `EventQueue`. One roundtrip at startup, and
  nothing to dispatch after. `wayland-protocols-plasma` (feature `client`)
  has the generated bindings (`appmenu::client`).
- **X11 (and Xwayland):** two `UTF8_STRING` properties on the window,
  `_KDE_NET_WM_APPMENU_SERVICE_NAME` and `_KDE_NET_WM_APPMENU_OBJECT_PATH`.
  KWin and plasma-workspace's `appmenu.cpp` read these. Setting a
  property on another client's window is ordinary X11, so `x11rb`'s pure
  Rust connection to `$DISPLAY` can do it with SDL's window number, and
  no Xlib is needed. (The older route, `com.canonical.AppMenu.Registrar
  .RegisterWindow(xid, path)`, also works on KDE through kded's importer,
  but the properties are what Qt sets.)

Which backend SDL picks on KDE depends on the compositor's protocols
(`SDL_GetCurrentVideoDriver` says). So both pointers are needed: they're
small, and the dbusmenu export behind them is the same.

**The dbusmenu export** is the part with no ready crate. `ksni` (the KDE
tray crate) implements it over zbus, but only behind its tray service.
`libdbusmenu-glib` is a GLib C library, and Qt does it inside
`QDBusMenuBar`. So we write it: one `#[zbus::interface(name =
"com.canonical.dbusmenu")]` object with `GetLayout`, `GetGroupProperties`,
`GetProperty`, `Event`, `EventGroup`, `AboutToShow`, `AboutToShowGroup`,
the `LayoutUpdated` signal (sent when Fullscreen's check or a slot's
label changes) and the `Version` (3), `TextDirection`, `Status` and
`IconThemePath` properties. An item is `(ia{sv}av)`: id, properties
(`label` with `_` mnemonics, `type` "separator", `toggle-type`
"checkmark" with `toggle-state`, `children-display` "submenu",
`enabled`, `shortcut` as `aas` such as `[["Shift","F1"]]`), and children.
The `shortcut` is display only. Our own key handling still does the
work, so there's no double press on Linux.

zbus serves on its own executor thread. A click arrives there and goes to
the frame loop through an `mpsc` channel, drained once a frame like
muda's receiver. The frame loop can't block on it.

### The spike

`docs/research/native-menu-bar/` (outside the workspace, like
`lua-probe`) is that export, about 320 lines with zbus 5. It has a Game
menu (Save State slot 1 ⇧F1, Load State slot 1 F1, a separator, Quit
Ctrl+Q) and a View menu (Fullscreen F11, a checkmark). It prints the
address it would hand to KWin and each click. Run it, then play the
global menu with busctl:

```
cargo build --manifest-path docs/research/native-menu-bar/Cargo.toml --target-dir target/menubar-spike
target/menubar-spike/debug/menubar-spike          # prints e.g. ":1.2889 /MenuBar"
busctl --user call :1.2889 /MenuBar com.canonical.dbusmenu GetLayout iias -- 0 -1 0
busctl --user call :1.2889 /MenuBar com.canonical.dbusmenu Event isvu 7 clicked i 0 0
```

On quack (2026-10-30) `Version` read 3. `GetLayout(0, -1)` returned the
whole tree with labels, the separator, shortcuts and the checkmark.
`Event(7, "clicked")` printed `clicked Fullscreen`, bumped the revision to
2 and flipped `toggle-state` to 1 in the next `GetLayout(6, -1)`. Clicks
on Load State and Quit printed theirs, and Quit ended the program. Quit
exits before zbus replies to that `Event`, so busctl reports "Remote peer
disconnected". The real module must answer before the window goes.

What the spike doesn't show: the menu in Plasma's panel. That needs the
appmenu pointer from a real SDL window, and a look at a KDE desktop.
Lane sessions here have no display (`WAYLAND_DISPLAY` is unset), and
the screen is june's. That look is the implementation's check.

## The plan, when it's built

1. `desktop::menubar`: the item tree (Game: Save State ▸ slots 1–4, Load
   State ▸ slots 1–4, Undo Load (F5), Quit; View: Fullscreen; maybe
   Options… to open the egui menu) and an `Action` enum. The frame loop
   maps actions onto the flags the keys already set (`slot`, `undo`,
   `menu.fullscreen_asked`, quit). Testable without a window. Built
   (2026-10-31): `crates/desktop/src/menubar.rs`, `tree`, `Action::ask`
   and `Action::id` (the integer ids dbusmenu and muda name rows by).
2. Linux: the dbusmenu object (from the spike), then the Wayland or X11
   pointer chosen by `SDL_GetCurrentVideoDriver`. A no-op where KWin's
   global isn't advertised and no appmenu service is on the bus (GNOME,
   gamescope on the Deck). New dependencies: zbus, wayland-client and
   wayland-backend (`client_system`, dlopen), wayland-protocols-plasma,
   x11rb. All pure Rust except libwayland, which is dlopened as SDL already
   does.
   Built (2026-10-31): `crates/desktop/src/menubar/dbusmenu.rs` (the
   export; its test walks and clicks the menu on a private
   `dbus-daemon`) and `menubar/kde.rs` (the pointers). On Wayland it
   exports only when KWin's appmenu global is advertised; on X11 only
   when `org.kde.kappmenu` is on the bus. Not yet seen in a panel.
3. macOS: muda behind `cfg(target_os = "macos")`, the same tree.
   Checked on june's Mac.
   Built (2026-10-31): `crates/desktop/src/menubar/macos.rs`, started
   after the window. The app menu (About, Services, Hide, Quit ⌘Q, our
   own Quit row, so Game leaves Quit out) and Window menu (Minimize,
   Zoom, Bring All to Front; no Close ⌘W) are rebuilt from muda's
   predefined items. Other rows name their hotkey in their text, "Slot 1
   (F1)". Compiled for `aarch64-apple-darwin` with clippy in a scratch
   crate of `menubar.rs` and `macos.rs` (the whole crate can't cross-build:
   SDL and Lua are C); not yet seen on a Mac.

The Linux check can be automated up to the bus: a test that starts a
private `dbus-daemon --session`, exports the menu and walks it. Seeing it
in the panel needs a KDE desktop once.

[sdl-forum]: https://discourse.libsdl.org/t/sdl3-menu-on-windows-mac-and-linux/65503
[appmenu-xml]: https://wayland.app/protocols/kde-appmenu

Other sources: muda's docs (docs.rs/muda 0.19.3, `init_for_nsapp`, its
winit example); plasma-workspace `appmenu/appmenu.cpp` (the X11 atoms);
KWin's appmenu review D3089 (why the address goes on the window, not by
window id over D-Bus); ksni 0.3.7 `src/dbus_interface.rs` (a zbus
dbusmenu to compare against).
