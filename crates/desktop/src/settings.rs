//! The window's settings file: `settings.conf` in
//! [`crate::gamepad::config_dir`], beside `gamepad.conf`. Lines are
//! `key = value`, with `#` comments:
//!
//! - `fast-speed = N`: fast-forward's speed ([`fast::parse_speed`]).
//! - `volume = PCT`: the master volume ([`audio::parse_volume`]).
//! - `server = ADDR`: the netplay server the menu's Netplay page uses
//!   (host or IP, and maybe `:PORT`; [`crate::room::DEFAULT_SERVER`] if
//!   left out).
//! - `directions = together|apart`: the Controls page shows the four
//!   directions as one row of whole pads, or each on its own row.
//! - `deadzone = PCT`, `trigger-deadzone = PCT`: how far a stick, or a
//!   trigger, goes before it counts, [`MIN_DEADZONE`] to [`MAX_DEADZONE`]
//!   percent ([`crate::gamepad::DEFAULT_DEADZONE`], 40, and
//!   [`crate::gamepad::DEFAULT_TRIGGER_DEADZONE`], 20, if left out).
//! - `deadzone-axes = together|apart`: the sticks' left and right take
//!   the same deadzone as their up and down, or each their own:
//!   `deadzone-horizontal = PCT` and `deadzone-vertical = PCT` (each
//!   `deadzone`'s if left out). Together if left out, and then those two
//!   are kept but not used.
//! - `resume = on|off`: start where the last run left off
//!   ([`crate::resume`]); off if left out, so a fresh
//!   config boots at the title screen.
//! - `save-dir = DIR`: keep all saves in DIR, each game's in a `<name>/`
//!   folder inside it ([`crate::game_dir`]); empty or left out for the
//!   default. `previous-save-dir = DIR` is the one the run that changed it
//!   started with (empty for the default), which the menu writes so games played under it bring
//!   their saves along on their next launch.
//!
//! A flag (`--fast-speed`, `--volume`) wins over the file, and the file
//! over the default. [`set`] rewrites one key in place, comments kept, for
//! what the window saves.

use crate::{audio, fast};
use std::path::{Path, PathBuf};

/// Where the window looks for its settings.
pub fn path() -> Option<PathBuf> {
    crate::gamepad::config_dir().map(|d| d.join("settings.conf"))
}

/// What the settings file says; `None` for a key it leaves out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// `fast-speed`: a multiple of normal speed; 0 is unthrottled.
    pub fast_speed: Option<f64>,
    /// `volume`: percent, [`audio::DEFAULT_VOLUME`] the default level.
    pub volume: Option<u32>,
    /// `server`: netplay's server, as the menu last set it.
    pub server: Option<String>,
    /// `directions`: whether the Controls page shows them apart.
    pub directions_apart: Option<bool>,
    /// `deadzone`: the sticks', in percent.
    pub deadzone: Option<u32>,
    /// `trigger-deadzone`: the triggers', in percent.
    pub trigger_deadzone: Option<u32>,
    /// `deadzone-axes`: whether the sticks' two ways have their own.
    pub deadzone_axes_apart: Option<bool>,
    /// `deadzone-horizontal`, `deadzone-vertical`: the sticks' left and
    /// right, and up and down, in percent, while apart.
    pub deadzone_horizontal: Option<u32>,
    pub deadzone_vertical: Option<u32>,
    /// `resume`: resume on launch.
    pub resume: Option<bool>,
    /// `save-dir`: the parent folder for every game's saves.
    pub save_dir: Option<PathBuf>,
    /// `previous-save-dir`: the `save-dir` the run that last changed it
    /// started with.
    pub previous_save_dir: Option<PathBuf>,
}

impl Settings {
    /// Whether to resume on launch: only when the file says `resume = on`
    /// (june, 2026-10-30), so a fresh config boots at the title screen.
    pub fn resume_on(&self) -> bool {
        self.resume.unwrap_or(false)
    }
}

/// The deadzone's bounds, in percent.
pub const MIN_DEADZONE: u32 = 5;
pub const MAX_DEADZONE: u32 = 90;

/// A deadzone in percent, `20` or `20%`.
pub fn parse_deadzone(value: &str) -> Result<u32, String> {
    let n: u32 = value
        .trim_end_matches('%')
        .trim()
        .parse()
        .map_err(|_| format!("deadzone {value:?} isn't a percent"))?;
    if !(MIN_DEADZONE..=MAX_DEADZONE).contains(&n) {
        return Err(format!("deadzone {n}%: {MIN_DEADZONE} to {MAX_DEADZONE}"));
    }
    Ok(n)
}

/// A line's `key = value`, or `None` for a blank or comment line.
pub(crate) fn entry(line: &str) -> Option<Result<(&str, &str), String>> {
    let line = line.split('#').next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    Some(
        line.split_once('=')
            .map(|(k, v)| (k.trim(), v.trim()))
            .ok_or_else(|| format!("{line:?} isn't `key = value`")),
    )
}

/// Parse the file's text.
pub fn parse(text: &str) -> Result<Settings, String> {
    let mut s = Settings::default();
    for (n, line) in text.lines().enumerate() {
        let err = |what: String| format!("line {}: {what}", n + 1);
        let Some(e) = entry(line) else { continue };
        let (key, value) = e.map_err(err)?;
        match key {
            "fast-speed" => s.fast_speed = Some(fast::parse_speed(value).map_err(err)?),
            "volume" => s.volume = Some(audio::parse_volume(value).map_err(err)?),
            "server" if !value.is_empty() => s.server = Some(value.to_owned()),
            "server" => {}
            "directions" => {
                s.directions_apart = Some(match value {
                    "apart" => true,
                    "together" => false,
                    v => return Err(err(format!("directions {v:?}: together or apart"))),
                })
            }
            "deadzone" => s.deadzone = Some(parse_deadzone(value).map_err(err)?),
            "trigger-deadzone" => s.trigger_deadzone = Some(parse_deadzone(value).map_err(err)?),
            "deadzone-axes" => {
                s.deadzone_axes_apart = Some(match value {
                    "apart" => true,
                    "together" => false,
                    v => return Err(err(format!("deadzone-axes {v:?}: together or apart"))),
                })
            }
            "deadzone-horizontal" => {
                s.deadzone_horizontal = Some(parse_deadzone(value).map_err(err)?)
            }
            "deadzone-vertical" => s.deadzone_vertical = Some(parse_deadzone(value).map_err(err)?),
            "resume" => {
                s.resume = Some(match value {
                    "on" => true,
                    "off" => false,
                    v => return Err(err(format!("resume {v:?}: on or off"))),
                })
            }
            "save-dir" => s.save_dir = (!value.is_empty()).then(|| value.into()),
            "previous-save-dir" => s.previous_save_dir = (!value.is_empty()).then(|| value.into()),
            k => return Err(err(format!("unknown setting {k:?}"))),
        }
    }
    Ok(s)
}

/// Read the file at `path`: no path or no file is no settings.
pub fn load(path: Option<&Path>) -> Result<Settings, String> {
    let Some(path) = path else {
        return Ok(Settings::default());
    };
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e.to_string()),
    }
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// `text` with `key` set to `value`: its first line for `key` rewritten
/// (a comment after it kept), any later ones dropped, or a line added at
/// the end.
pub fn set(text: &str, key: &str, value: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for line in text.lines() {
        let is_key = matches!(entry(line), Some(Ok((k, _))) if k == key);
        if !is_key {
            out.push_str(line);
        } else if done {
            continue;
        } else {
            out.push_str(&format!("{key} = {value}"));
            if let Some((_, comment)) = line.split_once('#') {
                out.push_str(&format!(" #{comment}"));
            }
            done = true;
        }
        out.push('\n');
    }
    if !done {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

/// Set `key` to `value` in the file at `path`, making it (and its folder)
/// if need be.
pub fn save(path: &Path, key: &str, value: &str) -> Result<(), String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(err(e)),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    std::fs::write(path, set(&text, key, value)).map_err(err)
}
