//! Spike: winit for the window, softbuffer to put pixels in it, and the
//! menu drawn by hand into the 240×160 frame (an 8×8 font), so it scales
//! with the picture. Driven by a pad (gilrs), the keyboard or the mouse.
//! See docs/research/ui-library.md.

#[path = "../../common.rs"]
mod common;

use common::{Cost, H, Nav, SPEEDS, Settings, Shared, Stats, W, fit};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

enum User {
    Frame,
    Nav(Nav),
}

const ITEMS: usize = 8;

struct App {
    shared: Arc<Shared>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    open: bool,
    focus: usize,
    settings: Settings,
    /// The frame with the menu drawn over it.
    small: Vec<u32>,
    cursor: (f64, f64),
    stats: Stats,
    cost: Cost,
}

impl App {
    fn label(&self, i: usize) -> String {
        let s = &self.settings;
        match i {
            0 => "Resume".into(),
            1 => format!("Fast-forward  < {} >", s.speed_label()),
            2 => format!("Save slot     < {} >", s.slot + 1),
            3 => "Save state".into(),
            4 => "Load state".into(),
            5 => "Undo load state".into(),
            6 => format!("Fullscreen    {}", if s.fullscreen { "on" } else { "off" }),
            _ => "Quit".into(),
        }
    }

    fn nav(&mut self, el: &ActiveEventLoop, n: Nav) {
        match n {
            Nav::Quit => {
                self.shared.quit.store(true, Ordering::Relaxed);
                el.exit();
                return;
            }
            Nav::Fullscreen => {
                self.toggle_fullscreen();
                return;
            }
            _ => {}
        }
        if n == Nav::Menu || (!self.open && n == Nav::Back) {
            self.open = !self.open;
            self.focus = 0;
            self.shared.paused.store(self.open, Ordering::Relaxed);
            self.stats.skip();
            self.draw();
            return;
        }
        if !self.open {
            return;
        }
        let s = &mut self.settings;
        match (n, self.focus) {
            (Nav::Up, _) => self.focus = (self.focus + ITEMS - 1) % ITEMS,
            (Nav::Down, _) => self.focus = (self.focus + 1) % ITEMS,
            (Nav::Left, 1) => s.speed = s.speed.saturating_sub(1),
            (Nav::Right, 1) => s.speed = (s.speed + 1).min(SPEEDS.len() - 1),
            (Nav::Left, 2) => s.slot = (s.slot + 3) % 4,
            (Nav::Right, 2) => s.slot = (s.slot + 1) % 4,
            (Nav::Back, _) | (Nav::Ok, 0) => {
                self.open = false;
                self.shared.paused.store(false, Ordering::Relaxed);
                self.stats.skip();
            }
            (Nav::Ok, 3) => s.status = format!("saved to slot {}", s.slot + 1),
            (Nav::Ok, 4) => s.status = format!("loaded slot {}", s.slot + 1),
            (Nav::Ok, 5) => s.status = "load undone".into(),
            (Nav::Ok | Nav::Left | Nav::Right, 6) => self.toggle_fullscreen(),
            (Nav::Ok, 7) => {
                self.shared.quit.store(true, Ordering::Relaxed);
                el.exit();
            }
            _ => {}
        }
        self.draw();
    }

    fn toggle_fullscreen(&mut self) {
        let w = self.window.as_ref().unwrap();
        self.settings.fullscreen = w.fullscreen().is_none();
        w.set_fullscreen(
            self.settings
                .fullscreen
                .then_some(Fullscreen::Borderless(None)),
        );
        w.set_cursor_visible(!self.settings.fullscreen);
    }

    /// The menu row at the cursor, if any.
    fn row_at(&self) -> Option<usize> {
        let size = self.window.as_ref()?.inner_size();
        let (s, l, t) = fit(size.width as usize, size.height as usize);
        let x = (self.cursor.0 as isize - l as isize) / s as isize;
        let y = (self.cursor.1 as isize - t as isize) / s as isize;
        let row = (y - MENU_TOP as isize - 14) / ROW as isize;
        ((20..220).contains(&x) && y >= MENU_TOP as isize + 14 && (row as usize) < ITEMS)
            .then_some(row as usize)
    }

    fn draw(&mut self) {
        let labels: Vec<String> = (0..ITEMS).map(|i| self.label(i)).collect();
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let t = Instant::now();
        let size = window.inner_size();
        let (w, h) = (size.width as usize, size.height as usize);
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w as u32), NonZeroU32::new(h as u32)) else {
            return;
        };
        surface.resize(nw, nh).unwrap();
        self.small
            .copy_from_slice(&self.shared.frame.lock().unwrap().1);
        if self.open {
            menu(&mut self.small, &labels, self.focus, &self.settings.status);
        }
        let mut buf = surface.buffer_mut().unwrap();
        let (s, l, top) = fit(w, h);
        buf.fill(0);
        place(&self.small, s, &mut buf, w, l, top);
        buf.present().unwrap();
        self.cost.add(t);
        if !self.open {
            self.stats.present();
        }
    }
}

const MENU_TOP: usize = 20;
const ROW: usize = 12;

/// Dim `f` and draw the menu over it, row `focus` lit.
fn menu(f: &mut [u32], labels: &[String], focus: usize, status: &str) {
    for c in f.iter_mut() {
        *c = (*c >> 2) & 0x3f3f3f;
    }
    let bottom = MENU_TOP + 14 + ROW * labels.len() + 14;
    rect(f, 16, MENU_TOP, 224, bottom, 0x101828);
    text(f, 24, MENU_TOP + 4, "gbaa", 0xf0c020);
    for (i, l) in labels.iter().enumerate() {
        let y = MENU_TOP + 14 + ROW * i;
        if i == focus {
            rect(f, 20, y, 220, y + ROW, 0x3060c0);
        }
        text(f, 28, y + 2, l, 0xffffff);
    }
    text(f, 24, bottom - 11, status, 0x80c080);
}

fn rect(f: &mut [u32], x0: usize, y0: usize, x1: usize, y1: usize, c: u32) {
    for y in y0..y1.min(H) {
        f[y * W + x0..y * W + x1.min(W)].fill(c);
    }
}

fn text(f: &mut [u32], x: usize, y: usize, s: &str, c: u32) {
    for (i, ch) in s.chars().enumerate() {
        let Some(g) = font8x8::legacy::BASIC_LEGACY.get(ch as usize) else {
            continue;
        };
        for (r, bits) in g.iter().enumerate() {
            for b in 0..8 {
                let (px, py) = (x + i * 8 + b, y + r);
                if bits >> b & 1 != 0 && px < W && py < H {
                    f[py * W + px] = c;
                }
            }
        }
    }
}

/// `f` scaled up by `s` into `buf` (rows `stride` apart) at (`l`, `t`),
/// as gbaa-desktop's `place_into`.
fn place(f: &[u32], s: usize, buf: &mut [u32], stride: usize, l: usize, t: usize) {
    let w = W * s;
    for (y, row) in f.chunks(W).enumerate() {
        let first = (t + y * s) * stride + l;
        let out = &mut buf[first..first + w];
        for (x, &c) in row.iter().enumerate() {
            out[x * s..][..s].fill(c);
        }
        for i in 1..s {
            buf.copy_within(first..first + w, first + i * stride);
        }
    }
}

impl ApplicationHandler<User> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("gbaa ui spike: overlay")
            .with_inner_size(winit::dpi::PhysicalSize::new(W as u32 * 3, H as u32 * 3));
        let window = Rc::new(el.create_window(attrs).unwrap());
        let context = softbuffer::Context::new(window.clone()).unwrap();
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).unwrap());
        self.window = Some(window);
        if std::env::args().any(|a| a == "--fullscreen") {
            self.toggle_fullscreen();
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, e: User) {
        match e {
            User::Frame => self.draw(),
            User::Nav(n) => self.nav(el, n),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, e: WindowEvent) {
        match e {
            WindowEvent::CloseRequested => {
                self.shared.quit.store(true, Ordering::Relaxed);
                el.exit();
            }
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => self.draw(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let n = match event.logical_key {
                    Key::Named(NamedKey::ArrowUp) => Nav::Up,
                    Key::Named(NamedKey::ArrowDown) => Nav::Down,
                    Key::Named(NamedKey::ArrowLeft) => Nav::Left,
                    Key::Named(NamedKey::ArrowRight) => Nav::Right,
                    Key::Named(NamedKey::Enter) => Nav::Ok,
                    Key::Named(NamedKey::Escape) => Nav::Back,
                    Key::Named(NamedKey::F11) => Nav::Fullscreen,
                    _ => return,
                };
                self.nav(el, n);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.open
                    && let Some(r) = self.row_at()
                    && r != self.focus
                {
                    self.focus = r;
                    self.draw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if self.open && self.row_at().is_some() {
                    self.nav(el, Nav::Ok);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => self.nav(el, Nav::Menu),
            _ => {}
        }
    }
}

fn main() {
    let el = EventLoop::<User>::with_user_event().build().unwrap();
    let shared = Shared::new();
    let proxy: EventLoopProxy<User> = el.create_proxy();
    common::spawn_game(shared.clone(), move || {
        let _ = proxy.send_event(User::Frame);
    });
    let proxy = el.create_proxy();
    common::spawn_pad(move |n| {
        let _ = proxy.send_event(User::Nav(n));
    });
    let proxy = el.create_proxy();
    common::spawn_script(move |n| {
        let _ = proxy.send_event(User::Nav(n));
    });
    let mut app = App {
        shared,
        window: None,
        surface: None,
        open: false,
        focus: 0,
        settings: Settings {
            speed: 2,
            ..Settings::default()
        },
        small: vec![0; W * H],
        cursor: (0.0, 0.0),
        stats: Stats::default(),
        cost: Cost::default(),
    };
    el.run_app(&mut app).unwrap();
    app.stats.report("overlay");
    app.cost.report("overlay");
    eprintln!(
        "spike overlay: game made {} frames",
        app.shared.made.load(Ordering::Relaxed)
    );
}
