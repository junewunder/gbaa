//! The script console (docs/research/lua-scripting.md, Recommended
//! design): the Lua scripts' console and text buffers, and a line to type
//! Lua into, as mGBA's scripting window has. A side panel to the right of
//! the frame in a window (the window widens to make room for it); over the
//! frame, at its right, when fullscreen, as on the Steam Deck. The
//! console button (F9; on a pad only if `console` is bound) or the
//! menu's Scripts page shows and hides it; that page also loads and
//! unloads scripts. Where the scripts' sidebars don't fit beside the
//! frame at its whole scale, they're a drawer the console's toggle opens
//! with it (`window::place`).
//!
//! The scripts run on the game thread ([`crate::scripting`]); the window
//! draws a copy of what they show ([`ScriptView`]), sent after each frame,
//! and sends back what's asked of them ([`ScriptCmd`]), done before the
//! next. Typed commands run in mGBA's way ([`script::Host::eval`]). On the
//! Deck, the line takes the keyboard from Steam's on-screen keyboard,
//! which SDL opens when the line is tapped.

use std::path::{Path, PathBuf};

/// What the window asks of the scripts.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptCmd {
    /// Load the script at the path, as `--script` does.
    Load(PathBuf),
    /// Unload the script with this id ([`script::Host::unload`]).
    Unload(u32),
    /// Run a line typed at the prompt.
    Eval(String),
}

/// The console lines the window keeps, newest last.
pub const CONSOLE_LINES: usize = 500;

/// What the scripts show, as the window draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptView {
    /// The console's newest [`CONSOLE_LINES`] lines.
    pub console: Vec<String>,
    /// The text buffers, by name, with what they hold.
    pub buffers: Vec<(String, String)>,
    /// The scripts loaded, by id.
    pub loaded: Vec<(u32, PathBuf)>,
}

impl ScriptView {
    /// Add console lines, dropping the oldest past [`CONSOLE_LINES`].
    pub fn add_console(&mut self, lines: impl IntoIterator<Item = String>) {
        self.console.extend(lines);
        let over = self.console.len().saturating_sub(CONSOLE_LINES);
        self.console.drain(..over);
    }
}

/// The panel's width in points.
pub const PANEL_WIDTH: f32 = 360.0;

/// Where the panel goes on a screen `screen` points in size: its right
/// [`PANEL_WIDTH`], no more than 60% of it.
pub fn panel_rect(screen: egui::Rect) -> egui::Rect {
    let w = PANEL_WIDTH.min(screen.width() * 0.6);
    egui::Rect::from_min_max(egui::pos2(screen.max.x - w, screen.min.y), screen.max)
}

/// The console panel's state.
#[derive(Default)]
pub struct Console {
    /// The panel is shown.
    pub shown: bool,
    /// Drawn over the frame (fullscreen), not beside it.
    pub overlay: bool,
    pub view: ScriptView,
    /// The prompt's line, and the lines entered before, oldest first.
    line: String,
    history: Vec<String>,
    /// How far back Up has gone through the history.
    back: Option<usize>,
    /// Asked of the scripts since the window last looked.
    pub cmds: Vec<ScriptCmd>,
    /// The line has the keyboard: keys go to it, not the game.
    pub typing: bool,
    /// Give the line the keyboard at the next pass.
    pub focus_line: bool,
    /// Where the line is, in points, for the on-screen keyboard.
    pub line_rect: Option<egui::Rect>,
}

impl Console {
    /// Show it or hide it.
    pub fn set_shown(&mut self, shown: bool) {
        self.shown = shown;
        if !shown {
            self.focus_line = false;
        }
    }

    /// Enter `line` at the prompt.
    pub fn enter(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if self.history.last().is_none_or(|l| l != line) {
            self.history.push(line.to_owned());
        }
        self.back = None;
        self.cmds.push(ScriptCmd::Eval(line.to_owned()));
    }

    /// Step through the history: `older` with Up, else Down (past the
    /// newest, an empty line).
    fn recall(&mut self, older: bool) {
        let n = self.history.len();
        self.back = match (self.back, older) {
            (None, true) if n > 0 => Some(n - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            _ => None,
        };
        self.line = self
            .back
            .map(|i| self.history[i].clone())
            .unwrap_or_default();
    }

    /// Draw the panel in `rect`: the text buffers above, the console
    /// below, the line at the bottom.
    pub fn ui(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let fill = if self.overlay {
            egui::Color32::from_black_alpha(215)
        } else {
            egui::Color32::from_gray(18)
        };
        ui.painter().rect_filled(rect, 0.0, fill);
        let inner = rect.shrink(6.0);
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.set_clip_rect(rect);
            let line_h = ui.spacing().interact_size.y + 2.0 * ui.spacing().item_spacing.y;
            let body = (inner.height() - line_h - 2.0 * ui.spacing().item_spacing.y).max(0.0);
            egui::ScrollArea::both()
                .id_salt("buffers")
                .max_height(body * 0.55)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if self.view.buffers.is_empty() {
                        let none = if self.view.loaded.is_empty() {
                            "No script loaded (menu, Scripts)"
                        } else {
                            "No text buffers"
                        };
                        ui.weak(none);
                    }
                    for (i, (name, text)) in self.view.buffers.iter().enumerate() {
                        let title = if name.is_empty() { "(unnamed)" } else { name };
                        egui::CollapsingHeader::new(title)
                            .id_salt(("buffer", i))
                            .default_open(true)
                            .show(ui, |ui| {
                                let t = egui::RichText::new(text.trim_end()).monospace();
                                ui.add(egui::Label::new(t).extend());
                            });
                    }
                });
            ui.separator();
            let left = (ui.available_height() - line_h).max(0.0);
            egui::ScrollArea::vertical()
                .id_salt("console")
                .stick_to_bottom(true)
                .max_height(left)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let v = ui.visuals().clone();
                    for l in &self.view.console {
                        let color = if l.starts_with("error: ") {
                            v.error_fg_color
                        } else if l.starts_with("warn: ") {
                            v.warn_fg_color
                        } else if l.starts_with("> ") {
                            v.strong_text_color()
                        } else {
                            v.text_color()
                        };
                        ui.label(egui::RichText::new(l).monospace().color(color));
                    }
                });
            let edit = egui::TextEdit::singleline(&mut self.line)
                .id_salt("prompt")
                .font(egui::TextStyle::Monospace)
                .hint_text("Lua: type a command, Enter runs it")
                .desired_width(f32::INFINITY);
            let r = ui.add(edit);
            // Enter takes the focus away from a one-line edit: run the
            // line and give it back.
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let line = std::mem::take(&mut self.line);
                self.enter(&line);
                r.request_focus();
            } else if r.has_focus() {
                let (up, down) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                    )
                });
                if up || down {
                    self.recall(up);
                }
            }
            if std::mem::take(&mut self.focus_line) {
                r.request_focus();
            }
            self.typing = r.has_focus() || ui.memory(|m| m.has_focus(r.id));
            self.line_rect = Some(r.rect);
        });
    }
}

/// The folders whose Lua scripts the menu offers: `scripts/` in the
/// config folder ([`crate::gamepad::config_dir`]), then each loaded
/// script's, each once.
pub fn script_dirs(config: Option<&Path>, loaded: &[(u32, PathBuf)]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = config.map(|c| c.join("scripts")).into_iter().collect();
    for (_, p) in loaded {
        let d = match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_owned(),
            _ => PathBuf::from("."),
        };
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// The `.lua` files directly in `dirs`, sorted within each folder.
pub fn scripts_in(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut all = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else {
            continue;
        };
        let mut v: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "lua") && p.is_file())
            .collect();
        v.sort();
        all.extend(v);
    }
    all
}
