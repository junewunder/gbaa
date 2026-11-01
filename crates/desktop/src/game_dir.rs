//! The folder a game's saves, states and resume file live in
//! (docs/save-locations-design.md): `<OS data dir>/gbaa/saves/<name>/` by
//! default, `<name>` being the game's name ([`runtime::name::GameName`]).
//! Inside it every file is named after the game, so a copied file still
//! says what it is: `<name>.sav`, `<name>.state1` to `.state4`,
//! `<name>.resume` (with `.old`, `.fault` and `.tmp`), and the `.sav`'s
//! backups in `backups/`.
//!
//! [`resolve`] picks the folder, highest first: a command-line flag (an
//! exact folder), the game's own setting (exact), the global setting (a
//! parent, `<name>/` inside it), the default (a parent). The flags are
//! `--save-dir` and `--state-dir`; the global setting is `save-dir` in
//! settings.conf ([`crate::settings`]); the game's own is `save-dir` in
//! its [`GameConf`], `games/<name>.conf` in the config folder. The menu's
//! Save folders page sets both, for the next launch.
//!
//! [`move_in`] copies a game's files into the folder the first time it
//! runs with none of its files there: from the folders it used before
//! ([`folder_pairs`]: the one the menu moved it from, the last global
//! setting's, the default), so switching launchers or settings doesn't
//! strand saves, else from beside its ROM ([`move_in_pairs`]).
//!
//! Headless runs use none of it: they keep a `.sav` only with `--save
//! FILE`, and a script's state slots beside the ROM, so the checks' runs
//! touch nothing of the player's.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where a game's files may be put, highest first, besides the default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layers {
    /// `--save-dir`: the exact folder for the game's files.
    pub save_flag: Option<PathBuf>,
    /// `--state-dir`: the exact folder for its states and resume file,
    /// which follow the saves' otherwise.
    pub state_flag: Option<PathBuf>,
    /// The game's own setting in its menu: an exact folder.
    pub per_game: Option<PathBuf>,
    /// The global setting in settings.conf: a parent, `<name>/` inside.
    pub global: Option<PathBuf>,
}

/// A game's own settings: `games/<name>.conf` in the config folder,
/// `key = value` lines as settings.conf has them:
///
/// - `save-dir = DIR`: the exact folder for the game's files ([`Layers`]'s
///   `per_game`); empty or left out for none.
/// - `moved-from = DIR`: the saves folder the game used when the menu last
///   changed where they go, the first place they're copied in from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameConf {
    pub save_dir: Option<PathBuf>,
    pub moved_from: Option<PathBuf>,
}

impl GameConf {
    /// The game `name`'s file in the config folder `config`.
    pub fn path(config: &Path, name: &str) -> PathBuf {
        config.join("games").join(format!("{name}.conf"))
    }

    /// Parse the file's text.
    pub fn parse(text: &str) -> Result<GameConf, String> {
        let mut c = GameConf::default();
        for (n, line) in text.lines().enumerate() {
            let err = |what: String| format!("line {}: {what}", n + 1);
            let Some(e) = crate::settings::entry(line) else {
                continue;
            };
            let (key, value) = e.map_err(err)?;
            let dir = (!value.is_empty()).then(|| PathBuf::from(value));
            match key {
                "save-dir" => c.save_dir = dir,
                "moved-from" => c.moved_from = dir,
                k => return Err(err(format!("unknown setting {k:?}"))),
            }
        }
        Ok(c)
    }

    /// Read the file at `path`: no path or no file is no settings.
    pub fn load(path: Option<&Path>) -> Result<GameConf, String> {
        let Some(path) = path else {
            return Ok(GameConf::default());
        };
        match std::fs::read_to_string(path) {
            Ok(text) => GameConf::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GameConf::default()),
            Err(e) => Err(e.to_string()),
        }
        .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The folders a game's files go in.
#[derive(Clone, Debug, PartialEq)]
pub struct Folders {
    /// The `.sav`, and its `backups/`.
    pub saves: PathBuf,
    /// The state slots and the resume file.
    pub states: PathBuf,
}

/// The folders for the game `name`: the highest of `layers` set, else
/// `saves/<name>/` in `data` (the OS data dir's `gbaa/`, [`data_dir`]).
/// `None` with neither.
pub fn resolve(layers: &Layers, name: &str, data: Option<&Path>) -> Option<Folders> {
    let saves = layers
        .save_flag
        .clone()
        .or_else(|| layers.per_game.clone())
        .or_else(|| layers.global.as_ref().map(|g| g.join(name)))
        .or_else(|| data.map(|d| d.join("saves").join(name)))?;
    let states = layers.state_flag.clone().unwrap_or_else(|| saves.clone());
    Some(Folders { saves, states })
}

impl Folders {
    /// `<name>.sav`, the cartridge save.
    pub fn sav(&self, name: &str) -> PathBuf {
        self.saves.join(format!("{name}.sav"))
    }

    /// `<name>` in the states folder, which [`crate::slots::path`] and
    /// [`crate::resume`] give their extensions (a game's name has no dot).
    pub fn states_stem(&self, name: &str) -> PathBuf {
        self.states.join(name)
    }
}

/// gbaa's folder in the OS data dir: [`data_dir_in`] the process's
/// environment.
pub fn data_dir() -> Option<PathBuf> {
    data_dir_in(|k| std::env::var_os(k))
}

/// gbaa's data folder, given how to read an environment variable:
/// `$XDG_DATA_HOME/gbaa`, else `~/.local/share/gbaa` (Linux, the Deck);
/// `~/Library/Application Support/gbaa` on macOS; `%APPDATA%\gbaa` on
/// Windows.
pub fn data_dir_in(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |k| var(k).filter(|d| !d.is_empty());
    if cfg!(windows) {
        return Some(Path::new(&set("APPDATA")?).join("gbaa"));
    }
    let home = || set("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Application Support/gbaa"));
    }
    let base = set("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".local/share")))?;
    Some(base.join("gbaa"))
}

/// The game's files kept beside its ROM before it had a folder, each with
/// where it goes in the folder: `<old>.sav` to `sav`, `<old>.state1` to
/// `.state4`, `.resume` and
/// `.resume.old` to the same in `states` (a [`Folders::states_stem`]).
/// `old` is the ROM's path, or for a build's own ROM where it is taken to
/// be ([`crate::embedded_path`]), so the old names are the ROM's.
pub fn move_in_pairs(old: &Path, sav: &Path, states: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = vec![(crate::sav_file::beside(old), sav.to_path_buf())];
    for slot in 1..=crate::slots::SLOTS {
        pairs.push((
            crate::slots::path(old, slot),
            crate::slots::path(states, slot),
        ));
    }
    pairs.push((crate::resume::path(old), crate::resume::path(states)));
    pairs.push((
        crate::resume::old_path(old),
        crate::resume::old_path(states),
    ));
    pairs
}

/// The places a game's files may be copied in from, tried in order, each
/// a list of files with where each goes ([`move_in`]).
pub type Sources = Vec<Vec<(PathBuf, PathBuf)>>;

/// The game's files in folders `from`, each with where it goes in `to`,
/// for [`move_in`]: the same names, as both are the game's folders. Files
/// already where they go (`from`'s states folder is `to`'s) are left out.
pub fn folder_pairs(from: &Folders, to: &Folders, name: &str) -> Vec<(PathBuf, PathBuf)> {
    let (old, new) = (from.states_stem(name), to.states_stem(name));
    let mut pairs = vec![(from.sav(name), to.sav(name))];
    for slot in 1..=crate::slots::SLOTS {
        pairs.push((
            crate::slots::path(&old, slot),
            crate::slots::path(&new, slot),
        ));
    }
    pairs.push((crate::resume::path(&old), crate::resume::path(&new)));
    pairs.push((crate::resume::old_path(&old), crate::resume::old_path(&new)));
    pairs.retain(|(from, to)| from != to);
    pairs
}

/// The first time a game runs with none of its files in its folder, copy
/// them in from the first of `sources` (lists of [`folder_pairs`] or
/// [`move_in_pairs`]) holding any, renamed to the game's name; the
/// originals stay. Files are never mixed from two places. With any of the
/// game's files already in the folder, nothing is copied, so a save there
/// is never copied over, and after the first run only the folder is used.
/// Gives the files copied in.
pub fn move_in(sources: &[Vec<(PathBuf, PathBuf)>]) -> Result<Vec<PathBuf>, String> {
    let all = || sources.iter().flatten();
    if all().any(|(from, to)| from == to || to.exists()) {
        return Ok(Vec::new());
    }
    let Some(pairs) = sources
        .iter()
        .find(|pairs| pairs.iter().any(|(from, _)| from.is_file()))
    else {
        return Ok(Vec::new());
    };
    let mut copied = Vec::new();
    for (from, to) in pairs.iter().filter(|(from, _)| from.is_file()) {
        std::fs::copy(from, to)
            .map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))?;
        copied.push(to.clone());
    }
    Ok(copied)
}
