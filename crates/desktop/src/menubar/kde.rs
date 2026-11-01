//! KDE's global menu for the SDL window (`docs/research/native-menu-bar.md`,
//! "KDE"): the menu bar on the session bus ([`super::dbusmenu`]), and the
//! window telling KWin where it is. On Wayland that's KWin's
//! `org_kde_kwin_appmenu`, asked on SDL's own `wl_display`; on X11 (and
//! Xwayland) two properties on the window, as Qt sets them.
//!
//! Where nothing would read the menu (GNOME, gamescope on the Deck, no
//! session bus), [`GlobalMenu::start`] gives None and nothing is exported.

use sdl3::sys::properties::{SDL_GetNumberProperty, SDL_GetPointerProperty};
use sdl3::sys::video::{
    SDL_GetWindowProperties, SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER,
    SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER, SDL_PROP_WINDOW_X11_WINDOW_NUMBER,
};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
use wayland_client::{Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols_plasma::appmenu::client::org_kde_kwin_appmenu::OrgKdeKwinAppmenu;
use wayland_protocols_plasma::appmenu::client::org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager;

use super::dbusmenu::{Export, PATH, Wake};
use super::{Action, State};

/// The menu bar in KDE's global menu, while this lives. Drop it before
/// the window it points from.
pub struct GlobalMenu {
    // Field order is drop order: the pointer goes before the menu. On
    // X11 there's none to keep: the properties go with the window.
    _wayland: Option<Appmenu>,
    export: Export,
}

impl GlobalMenu {
    /// Export the menu for `state` and point `window` at it, if SDL's
    /// `driver` is one KDE reads a global menu from and KDE is there to.
    pub fn start(
        window: &sdl3::video::Window,
        driver: &str,
        state: State,
        wake: Wake,
    ) -> Option<GlobalMenu> {
        match driver {
            "wayland" => {
                // KWin's global first: without it nobody would ask.
                let pending = PendingAppmenu::new(window)?;
                let export = exported(state, wake)?;
                let pointer = pending.point(&export.service())?;
                Some(GlobalMenu {
                    _wayland: Some(pointer),
                    export,
                })
            }
            "x11" => {
                let export = exported(state, wake)?;
                if !kde_reads_x11_menus(&export) {
                    return None;
                }
                point_x11(window, &export.service())?;
                Some(GlobalMenu {
                    _wayland: None,
                    export,
                })
            }
            _ => None,
        }
    }

    /// The rows clicked since the last call.
    pub fn clicks(&self) -> Vec<Action> {
        self.export.clicks()
    }

    /// Show the menu for `state`.
    pub fn set(&self, state: State) {
        self.export.set(state)
    }
}

fn exported(state: State, wake: Wake) -> Option<Export> {
    Export::session(state, wake)
        .inspect_err(|e| eprintln!("gbaa-desktop: no global menu: the session bus: {e}"))
        .ok()
}

/// KDE's appmenu module, which reads the X11 properties, is on the bus.
fn kde_reads_x11_menus(export: &Export) -> bool {
    let Ok(bus) = zbus::blocking::fdo::DBusProxy::new(export.connection()) else {
        return false;
    };
    let Ok(name) = "org.kde.kappmenu".try_into() else {
        return false;
    };
    bus.name_has_owner(name).unwrap_or(false)
}

/// Our own queue on SDL's Wayland connection.
struct Globals;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Globals {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Globals: OrgKdeKwinAppmenuManager);
delegate_noop!(Globals: OrgKdeKwinAppmenu);

/// KWin's appmenu manager, bound, and SDL's surface, before the menu is
/// on the bus.
struct PendingAppmenu {
    conn: wayland_client::Connection,
    queue: EventQueue<Globals>,
    manager: OrgKdeKwinAppmenuManager,
    surface: WlSurface,
}

/// The window's appmenu object, released on drop.
struct Appmenu {
    conn: wayland_client::Connection,
    _queue: EventQueue<Globals>,
    manager: OrgKdeKwinAppmenuManager,
    appmenu: OrgKdeKwinAppmenu,
}

impl PendingAppmenu {
    fn new(window: &sdl3::video::Window) -> Option<PendingAppmenu> {
        // SAFETY: SDL's own window and its properties, read while it's
        // open; null when it isn't on Wayland.
        let (display, surface) = unsafe {
            let props = SDL_GetWindowProperties(window.raw());
            (
                SDL_GetPointerProperty(
                    props,
                    SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER,
                    std::ptr::null_mut(),
                ),
                SDL_GetPointerProperty(
                    props,
                    SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER,
                    std::ptr::null_mut(),
                ),
            )
        };
        if display.is_null() || surface.is_null() {
            return None;
        }
        // SAFETY: the display outlives this (the window's [`GlobalMenu`]
        // is dropped before SDL closes it), and the surface is a live
        // `wl_surface` proxy.
        let backend =
            unsafe { wayland_backend::client::Backend::from_foreign_display(display.cast()) };
        let conn = wayland_client::Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<Globals>(&conn).ok()?;
        let qh = queue.handle();
        let manager = globals
            .bind::<OrgKdeKwinAppmenuManager, _, _>(&qh, 1..=2, ())
            .ok()?;
        let id = unsafe {
            wayland_backend::client::ObjectId::from_ptr(WlSurface::interface(), surface.cast())
        }
        .ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        queue.roundtrip(&mut Globals).ok()?;
        Some(PendingAppmenu {
            conn,
            queue,
            manager,
            surface,
        })
    }

    /// Tell KWin the window's menu is at `service`'s [`PATH`].
    fn point(self, service: &str) -> Option<Appmenu> {
        let PendingAppmenu {
            conn,
            mut queue,
            manager,
            surface,
        } = self;
        let appmenu = manager.create(&surface, &queue.handle(), ());
        appmenu.set_address(service.into(), PATH.into());
        queue
            .roundtrip(&mut Globals)
            .inspect_err(|e| eprintln!("gbaa-desktop: no global menu: Wayland: {e}"))
            .ok()?;
        Some(Appmenu {
            conn,
            _queue: queue,
            manager,
            appmenu,
        })
    }
}

impl Drop for Appmenu {
    fn drop(&mut self) {
        self.appmenu.release();
        if self.manager.version() >= 2 {
            self.manager.release();
        }
        let _ = self.conn.flush();
    }
}

/// Set the window's `_KDE_NET_WM_APPMENU_*` properties, on our own X11
/// connection to the display SDL opened.
fn point_x11(window: &sdl3::video::Window, service: &str) -> Option<()> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, PropMode};
    use x11rb::wrapper::ConnectionExt as _;

    // SAFETY: SDL's own window, read while it's open; 0 when it isn't on
    // X11.
    let xid = unsafe {
        SDL_GetNumberProperty(
            SDL_GetWindowProperties(window.raw()),
            SDL_PROP_WINDOW_X11_WINDOW_NUMBER,
            0,
        )
    };
    let xid = u32::try_from(xid).ok().filter(|&x| x != 0)?;
    let set = || -> Result<(), Box<dyn std::error::Error>> {
        let (conn, _) = x11rb::connect(None)?;
        let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> {
            Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        let utf8 = atom("UTF8_STRING")?;
        for (name, value) in [
            ("_KDE_NET_WM_APPMENU_SERVICE_NAME", service),
            ("_KDE_NET_WM_APPMENU_OBJECT_PATH", PATH),
        ] {
            conn.change_property8(PropMode::REPLACE, xid, atom(name)?, utf8, value.as_bytes())?
                .check()?;
        }
        conn.flush()?;
        Ok(())
    };
    set()
        .inspect_err(|e| eprintln!("gbaa-desktop: no global menu: X11: {e}"))
        .ok()
}
