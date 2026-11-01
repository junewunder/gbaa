//! Spike: winit for the window, OpenGL through glutin, and egui (drawn by
//! egui_glow) for both the picture (a 240×160 texture, nearest-sampled at
//! a whole scale) and the menu over it. The pad's controls become egui's
//! arrow, Enter and Escape keys, so egui's own focus moves through the
//! menu. See docs/research/ui-library.md.

#[path = "../../common.rs"]
mod common;

use common::{Cost, H, Nav, SPEEDS, Settings, Shared, Stats, W, fit};
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use raw_window_handle::HasWindowHandle;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

enum User {
    Frame,
    /// egui asks to be drawn again (layout settling, animations).
    Repaint,
    Nav(Nav),
}

struct Gl {
    window: Window,
    context: PossiblyCurrentContext,
    surface: Surface<WindowSurface>,
    egui: egui_glow::EguiGlow,
    texture: Option<egui::TextureHandle>,
}

struct App {
    shared: Arc<Shared>,
    proxy: winit::event_loop::EventLoopProxy<User>,
    gl: Option<Gl>,
    open: bool,
    /// The menu just opened: focus its first item.
    opened: bool,
    settings: Settings,
    stats: Stats,
    cost: Cost,
    quit: bool,
}

impl App {
    fn nav(&mut self, el: &ActiveEventLoop, n: Nav) {
        let Some(gl) = &mut self.gl else { return };
        let key = match n {
            Nav::Quit => {
                self.quit = true;
                self.shared.quit.store(true, Ordering::Relaxed);
                el.exit();
                return;
            }
            Nav::Fullscreen => {
                self.settings.fullscreen = !self.settings.fullscreen;
                apply_fullscreen(&gl.window, self.settings.fullscreen);
                return;
            }
            Nav::Menu => {
                self.set_open(!self.open);
                self.draw();
                return;
            }
            Nav::Back => {
                self.set_open(!self.open);
                self.draw();
                return;
            }
            _ if !self.open => return,
            Nav::Up => egui::Key::ArrowUp,
            Nav::Down => egui::Key::ArrowDown,
            Nav::Left => egui::Key::ArrowLeft,
            Nav::Right => egui::Key::ArrowRight,
            Nav::Ok => egui::Key::Enter,
        };
        // As if typed, for egui's focus to handle.
        let events = &mut gl.egui.egui_winit.egui_input_mut().events;
        for pressed in [true, false] {
            events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        self.draw();
    }

    fn set_open(&mut self, open: bool) {
        self.open = open;
        self.opened = open;
        self.shared.paused.store(open, Ordering::Relaxed);
        self.stats.skip();
    }

    fn draw(&mut self) {
        let Some(gl) = &mut self.gl else { return };
        let t = Instant::now();
        let image = {
            let f = self.shared.frame.lock().unwrap();
            let px: Vec<egui::Color32> =
                f.1.iter()
                    .map(|&c| egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8))
                    .collect();
            egui::ColorImage::new([W, H], px)
        };
        let texture = gl
            .egui
            .egui_ctx
            .load_texture("gba", image, egui::TextureOptions::NEAREST);
        let tex = texture.id();
        gl.texture = Some(texture);
        let size = gl.window.inner_size();
        let (open, opened) = (self.open, std::mem::take(&mut self.opened));
        let s = &mut self.settings;
        let mut close = false;
        let mut toggle = false;
        let mut quit = false;
        gl.egui.run(&gl.window, |ui| {
            let ppp = ui.ctx().pixels_per_point();
            // A whole scale in physical pixels, placed in points.
            let (k, l, t) = fit(size.width as usize, size.height as usize);
            let rect = egui::Rect::from_min_size(
                egui::pos2(l as f32 / ppp, t as f32 / ppp),
                egui::vec2((W * k) as f32 / ppp, (H * k) as f32 / ppp),
            );
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex, rect, uv, egui::Color32::WHITE);
            if !open {
                return;
            }
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(160));
            egui::Window::new("gbaa")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    let resume = ui.button("Resume");
                    if opened {
                        resume.request_focus();
                    }
                    close |= resume.clicked();
                    ui.add(
                        egui::Slider::new(&mut s.speed, 0..=SPEEDS.len() - 1)
                            .text("Fast-forward")
                            .show_value(false),
                    );
                    ui.label(s.speed_label());
                    let mut slot = s.slot + 1;
                    ui.add(egui::Slider::new(&mut slot, 1..=4).text("Save slot"));
                    s.slot = slot - 1;
                    if ui.button("Save state").clicked() {
                        s.status = format!("saved to slot {}", s.slot + 1);
                    }
                    if ui.button("Load state").clicked() {
                        s.status = format!("loaded slot {}", s.slot + 1);
                    }
                    if ui.button("Undo load state").clicked() {
                        s.status = "load undone".into();
                    }
                    let mut full = s.fullscreen;
                    toggle |= ui.checkbox(&mut full, "Fullscreen").changed();
                    quit |= ui.button("Quit").clicked();
                    ui.label(&s.status);
                });
        });
        if close {
            self.open = false;
            self.shared.paused.store(false, Ordering::Relaxed);
            self.stats.skip();
        }
        if toggle {
            self.settings.fullscreen = !self.settings.fullscreen;
            apply_fullscreen(&gl.window, self.settings.fullscreen);
        }
        if quit {
            self.quit = true;
            self.shared.quit.store(true, Ordering::Relaxed);
        }
        unsafe {
            use egui_glow::glow::HasContext;
            let g = gl.egui.painter.gl();
            g.clear_color(0.0, 0.0, 0.0, 1.0);
            g.clear(egui_glow::glow::COLOR_BUFFER_BIT);
        }
        gl.egui.paint(&gl.window);
        gl.surface.swap_buffers(&gl.context).unwrap();
        self.cost.add(t);
        if !self.open {
            self.stats.present();
        }
    }
}

fn apply_fullscreen(w: &Window, on: bool) {
    w.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
    w.set_cursor_visible(!on);
}

impl ApplicationHandler<User> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gl.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("gbaa ui spike: egui")
            .with_inner_size(winit::dpi::PhysicalSize::new(W as u32 * 3, H as u32 * 3));
        let template = glutin::config::ConfigTemplateBuilder::new();
        let (window, config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(attrs))
            .build(el, template, |mut c| c.next().unwrap())
            .unwrap();
        let window = window.unwrap();
        let display = config.display();
        let raw = window.window_handle().unwrap().as_raw();
        let context = unsafe {
            display
                .create_context(&config, &ContextAttributesBuilder::new().build(Some(raw)))
                .or_else(|_| {
                    display.create_context(
                        &config,
                        &ContextAttributesBuilder::new()
                            .with_context_api(glutin::context::ContextApi::Gles(None))
                            .build(Some(raw)),
                    )
                })
                .unwrap()
        };
        let size = window.inner_size();
        let attrs = glutin::surface::SurfaceAttributesBuilder::<WindowSurface>::new().build(
            raw,
            NonZeroU32::new(size.width).unwrap(),
            NonZeroU32::new(size.height).unwrap(),
        );
        let surface = unsafe { display.create_window_surface(&config, &attrs).unwrap() };
        let context = context.make_current(&surface).unwrap();
        // The game paces itself (by the sound in gbaa); waiting for vblank
        // too would be a second clock.
        let vsync = std::env::var_os("SPIKE_VSYNC").is_some();
        let interval = if vsync {
            SwapInterval::Wait(NonZeroU32::MIN)
        } else {
            SwapInterval::DontWait
        };
        if let Err(e) = surface.set_swap_interval(&context, interval) {
            eprintln!("spike egui: swap interval: {e}");
        }
        let glow = unsafe {
            egui_glow::glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s))
        };
        let egui = egui_glow::EguiGlow::new(el, Arc::new(glow), None, None, false);
        egui.egui_ctx.set_zoom_factor(1.5);
        let proxy = std::sync::Mutex::new(self.proxy.clone());
        egui.egui_ctx.set_request_repaint_callback(move |_| {
            let _ = proxy.lock().unwrap().send_event(User::Repaint);
        });
        if std::env::args().any(|a| a == "--fullscreen") {
            self.settings.fullscreen = true;
            apply_fullscreen(&window, true);
        }
        self.gl = Some(Gl {
            window,
            context,
            surface,
            egui,
            texture: None,
        });
    }

    fn user_event(&mut self, el: &ActiveEventLoop, e: User) {
        match e {
            User::Frame | User::Repaint => self.draw(),
            User::Nav(n) => self.nav(el, n),
        }
        if self.quit {
            el.exit();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, e: WindowEvent) {
        let Some(gl) = &mut self.gl else { return };
        if let WindowEvent::Resized(s) = &e
            && let (Some(w), Some(h)) = (NonZeroU32::new(s.width), NonZeroU32::new(s.height))
        {
            gl.surface.resize(&gl.context, w, h);
        }
        match &e {
            WindowEvent::CloseRequested => {
                self.shared.quit.store(true, Ordering::Relaxed);
                el.exit();
                return;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // The menu's own keys; the rest go to egui.
                match &event.logical_key {
                    Key::Named(NamedKey::F11) => return self.nav(el, Nav::Fullscreen),
                    Key::Named(NamedKey::Escape) => return self.nav(el, Nav::Back),
                    _ => {}
                }
            }
            _ => {}
        }
        let response = gl.egui.on_window_event(&gl.window, &e);
        if response.repaint || matches!(e, WindowEvent::RedrawRequested) {
            self.draw();
        }
        if self.quit {
            el.exit();
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let Some(gl) = &mut self.gl {
            gl.egui.destroy();
        }
    }
}

fn main() {
    let el = EventLoop::<User>::with_user_event().build().unwrap();
    let shared = Shared::new();
    let proxy = el.create_proxy();
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
        proxy: el.create_proxy(),
        gl: None,
        open: false,
        opened: false,
        settings: Settings {
            speed: 2,
            ..Settings::default()
        },
        stats: Stats::default(),
        cost: Cost::default(),
        quit: false,
    };
    el.run_app(&mut app).unwrap();
    app.stats.report("egui");
    app.cost.report("egui");
    eprintln!(
        "spike egui: game made {} frames",
        app.shared.made.load(Ordering::Relaxed)
    );
}
