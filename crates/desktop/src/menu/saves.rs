//! The Save folders page (docs/save-locations-design.md): where this
//! game's saves, states and resume file go (an exact folder, kept in its
//! [`GameConf`]), and where every game's go (a parent, each game in
//! `<name>/` inside it, kept in settings.conf). Choosing either opens a
//! folder picker a pad can drive: up a folder, home, into a folder listed,
//! "Use this folder", or back to the default. A launcher's `--save-dir` is
//! over the game's setting: its row is greyed, "set by the launcher".
//!
//! A change counts from the next launch: this run's files stay where they
//! are, and on that launch they're copied into the new folder (from the
//! one the game's file says it `moved-from`, or settings.conf's
//! `previous-save-dir`), unless the game's files are there already.

use super::{Menu, Page, Rows, SAVES_ROW, footer, scrolled, title};
use crate::game_dir::{self, GameConf, Layers};
use std::path::{Path, PathBuf};

/// What the page shows and changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveFolders {
    /// The game's name, its folder's under a global setting.
    pub game: String,
    /// The game's own settings file ([`GameConf::path`]), `None` with no
    /// config folder: then the game's row is kept to this run.
    pub conf: Option<PathBuf>,
    /// The folder this run keeps the `.sav` in.
    pub now: Option<PathBuf>,
    /// The game's setting, an exact folder.
    pub per_game: Option<PathBuf>,
    /// The global setting, a parent.
    pub global: Option<PathBuf>,
    /// The global setting as settings.conf had it at launch: where this
    /// run's files are under it, so the one a change writes as
    /// `previous-save-dir`, however many picks the change took.
    pub global_at_launch: Option<PathBuf>,
    /// `--save-dir` is in charge of the game's folder.
    pub launcher: bool,
    /// gbaa's data folder, the default's parent ([`game_dir::data_dir`]).
    pub data: Option<PathBuf>,
}

impl SaveFolders {
    /// The folder the next launch keeps the `.sav` in, as the settings
    /// stand (the launcher's, if it's in charge, as this run's).
    pub fn next(&self) -> Option<PathBuf> {
        if self.launcher {
            return self.now.clone();
        }
        let layers = Layers {
            per_game: self.per_game.clone(),
            global: self.global.clone(),
            ..Default::default()
        };
        game_dir::resolve(&layers, &self.game, self.data.as_deref()).map(|f| f.saves)
    }
}

/// Which setting the picker sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Game,
    All,
}

impl Target {
    /// Its row on the Save folders page.
    fn row(self) -> usize {
        match self {
            Target::Game => 0,
            Target::All => 1,
        }
    }
}

/// The folder picker: the folder it's in and those in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Picker {
    pub target: Target,
    pub dir: PathBuf,
    /// The folders in `dir`, by name, hidden ones left out.
    pub dirs: Vec<PathBuf>,
    /// Why `dir` couldn't be read.
    pub error: Option<String>,
}

impl Picker {
    fn at(target: Target, dir: PathBuf) -> Picker {
        let mut p = Picker {
            target,
            dir: PathBuf::new(),
            dirs: Vec::new(),
            error: None,
        };
        p.go(dir);
        p
    }

    /// Go into `dir`, listing its folders.
    fn go(&mut self, dir: PathBuf) {
        self.error = None;
        self.dirs = match std::fs::read_dir(&dir) {
            Ok(entries) => {
                let mut dirs: Vec<PathBuf> = entries
                    .flatten()
                    .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                dirs.sort_by_key(|p| name(p).to_lowercase());
                dirs
            }
            Err(e) => {
                self.error = Some(format!("can't read it: {e}"));
                Vec::new()
            }
        };
        self.dir = dir;
    }
}

/// The user's home folder, where the picker starts with nothing set.
pub fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// A folder's last part, or all of it for a root.
fn name(p: &Path) -> String {
    p.file_name()
        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())
}

/// The most a row's value shows of a folder, in characters.
const SHORT: usize = 24;

/// `p` short enough for a row's value: the home folder as `~`, and only
/// its end if still long.
pub fn short(p: &Path) -> String {
    let full = match home().and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    };
    let n = full.chars().count();
    if n <= SHORT {
        return full;
    }
    let tail: String = full.chars().skip(n - (SHORT - 1)).collect();
    format!("…{tail}")
}

impl Menu {
    /// The Save folders page: the game's folder, every game's, where this
    /// run keeps them and where the next will, and Back.
    pub(super) fn saves_page(&mut self, ui: &mut egui::Ui, rows: &mut Rows) {
        title(ui, "Save folders");
        let s = self.saves.clone();
        let mut back = false;
        let lines = 1 + usize::from(self.saved.is_some());
        scrolled(ui, lines, |ui| {
            let game = match (&s.per_game, s.launcher) {
                (_, true) => "set by the launcher".to_owned(),
                (Some(p), false) => short(p),
                (None, false) => "with all saves".to_owned(),
            };
            let r = ui
                .add_enabled_ui(!s.launcher, |ui| rows.row(ui, "This game's saves", &game))
                .inner;
            if !s.launcher && r.response.clicked() {
                self.pick(Target::Game, s.per_game.clone());
            }
            let all = s.global.as_deref().map_or("default".to_owned(), short);
            if rows.row(ui, "All saves go in", &all).response.clicked() {
                self.pick(Target::All, s.global.clone());
            }
            back = rows.row(ui, "Back", "").step != 0;
            let said = |ui: &mut egui::Ui, text: String| {
                ui.add(egui::Label::new(egui::RichText::new(text).small()).wrap());
            };
            if let Some(now) = &s.now {
                said(ui, format!("This run's: {}", now.display()));
            }
            match s.next() {
                Some(next) if Some(&next) != s.now.as_ref() => said(
                    ui,
                    format!(
                        "From the next launch: {} (this game's files are copied there)",
                        next.display()
                    ),
                ),
                _ => {}
            }
            if s.global.is_some() {
                said(
                    ui,
                    "All saves: each game in a folder of its name inside".to_owned(),
                );
            }
        });
        let mut hints = vec!["A/Enter: choose a folder  ·  B/Esc: back"];
        hints.extend(self.saved.as_deref());
        footer(ui, &hints);
        if back {
            self.turn(Page::Main, SAVES_ROW);
        }
    }

    /// Open the picker for `target`, at the folder set (if it's there),
    /// else home.
    fn pick(&mut self, target: Target, set: Option<PathBuf>) {
        let start = set
            .filter(|d| d.is_dir())
            .or_else(home)
            .unwrap_or_else(|| PathBuf::from("/"));
        self.picker = Some(Picker::at(target, start));
        self.turn(Page::Folder, 0);
    }

    /// The folder picker: where it is, "Use this folder", back to the
    /// default, up a folder, home, the folders in it, and Back.
    pub(super) fn folder_page(&mut self, ui: &mut egui::Ui, rows: &mut Rows) {
        let Some(p) = self.picker.clone() else {
            return self.turn(Page::Saves, 0);
        };
        title(
            ui,
            match p.target {
                Target::Game => "This game's saves",
                Target::All => "All saves go in",
            },
        );
        ui.add(egui::Label::new(p.dir.display().to_string()).wrap());
        let mut chosen: Option<Option<PathBuf>> = None;
        let mut go: Option<PathBuf> = None;
        let mut back = false;
        let lines = 1 + usize::from(self.saved.is_some() || p.error.is_some());
        scrolled(ui, lines, |ui| {
            if rows.row(ui, "Use this folder", "").step != 0 {
                chosen = Some(Some(p.dir.clone()));
            }
            let default = match p.target {
                Target::Game => "Keep them with all saves",
                Target::All => "Use the default folder",
            };
            if rows.row(ui, default, "").step != 0 {
                chosen = Some(None);
            }
            if let Some(up) = p.dir.parent()
                && rows.link(ui, "Up a folder").step != 0
            {
                go = Some(up.to_path_buf());
            }
            if let Some(h) = home()
                && rows.link(ui, "Home").step != 0
            {
                go = Some(h);
            }
            for d in &p.dirs {
                if rows.link(ui, &name(d)).step != 0 {
                    go = Some(d.clone());
                }
            }
            back = rows.row(ui, "Back", "").step != 0;
        });
        let mut hints = vec!["A/Enter: choose  ·  B/Esc: back"];
        hints.extend(p.error.as_deref().or(self.saved.as_deref()));
        footer(ui, &hints);
        if let Some(d) = go {
            if let Some(p) = &mut self.picker {
                p.go(d);
            }
            // Its rows are new: like a page turned, the next pass is laid
            // out dry first, so the focus is on the first row before a key
            // comes.
            self.focus = Some(0);
            self.drawn = None;
        } else if let Some(dir) = chosen {
            self.set_folder(p.target, dir);
            self.turn(Page::Saves, p.target.row());
            // The turn drops what was said: say it on the page turned to.
            self.saved = self.folder_said.take();
        } else if back {
            self.turn(Page::Saves, p.target.row());
        }
    }

    /// Set `target`'s folder to `dir` (`None` for the default) and save
    /// it, with where the game's files are now for the next launch to
    /// copy them from.
    pub(super) fn set_folder(&mut self, target: Target, dir: Option<PathBuf>) {
        let s = &mut self.saves;
        let show = |d: &Option<PathBuf>| {
            d.as_ref()
                .map_or(String::new(), |d| d.display().to_string())
        };
        let said = match target {
            Target::Game => {
                if s.per_game == dir {
                    return;
                }
                s.per_game = dir.clone();
                s.conf.as_ref().map(|path| {
                    let mut keys = vec![("save-dir", show(&dir))];
                    keys.extend(
                        s.now
                            .as_ref()
                            .map(|n| ("moved-from", n.display().to_string())),
                    );
                    save_keys(path, &keys)
                })
            }
            Target::All => {
                if s.global == dir {
                    return;
                }
                s.global = dir.clone();
                let launch = s.global_at_launch.clone();
                self.settings.as_ref().map(|path| {
                    let keys = [
                        ("previous-save-dir", show(&launch)),
                        ("save-dir", show(&dir)),
                    ];
                    save_keys(path, &keys)
                })
            }
        };
        self.folder_said = said;
    }
}

/// Set each of `keys` in the file at `path` ([`crate::settings::save`]),
/// saying how it went.
fn save_keys(path: &Path, keys: &[(&str, String)]) -> String {
    for (k, v) in keys {
        if let Err(e) = crate::settings::save(path, k, v) {
            return format!("not saved: {e}");
        }
    }
    format!("saved to {}; counts from the next launch", path.display())
}

/// [`SaveFolders`] for the game `name`, its `.sav` this run at `sav`
/// (`None` with `--no-save`), its own settings and settings.conf's as
/// read, `--save-dir` given or not.
pub fn save_folders(
    name: &str,
    sav: Option<&Path>,
    conf: &GameConf,
    global: Option<PathBuf>,
    launcher: bool,
) -> SaveFolders {
    SaveFolders {
        game: name.to_owned(),
        conf: crate::gamepad::config_dir().map(|c| GameConf::path(&c, name)),
        now: sav.and_then(Path::parent).map(Path::to_path_buf),
        per_game: conf.save_dir.clone(),
        global_at_launch: global.clone(),
        global,
        launcher,
        data: game_dir::data_dir(),
    }
}
