//! A route file, `units/<name>.toml`: a ROM, the buttons pressed on the
//! way, and the checkpoints (frames whose pictures must match) along it.
//!
//! ```toml
//! rom = "$HOME/ROMs/gba/Emerald Rogue.gba"
//! # state = "$GBAA_DATA/states/x.ss"   start from an mGBA save state
//!
//! [[press]]                 one press: Start from frame 1000
//! button = "start"
//! at = 1000
//!
//! [[press]]                 a run: A at 1200, 1220, … 1780
//! button = "a"
//! from = 1200
//! to = 1780
//! every = 20
//! # hold = 6                frames each press is held (default 6)
//!
//! [[checkpoint]]
//! name = "splash"
//! frame = 100
//! hash = "28f55138fee82a10"
//! # mgba_hash = "…"         what mGBA draws where it is known wrong,
//! # mgba_why = "…"          and why (both or neither)
//! # min_compiled = 0.97     the least share of running guest cycles, to
//! #                         this frame, the recompiled build may spend
//! #                         outside the interpreter fallback
//! # block_timed = [1]       frames after the checkpoint before and before
//! # block_timed_why = "…"   this one where the recompiled build's picture
//! #                         may differ from the interpreter's, since compiled
//! #                         code ticks once per block (docs/codegen-design.md,
//! #                         Timing), and why (both or neither)
//! ```
//!
//! `$HOME` and `$GBAA_DATA` in paths are expanded. Frames count from 1,
//! the first frame run, as `--frames`; a press at frame f is seen from
//! V-blank f, as `--input`'s `button@f`.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Frames a press holds its button unless it says (`--input`'s default).
pub const HOLD: u64 = 6;

const BUTTONS: [&str; 10] = [
    "a", "b", "select", "start", "right", "left", "up", "down", "r", "l",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    rom: String,
    state: Option<String>,
    #[serde(default)]
    press: Vec<Press>,
    #[serde(default)]
    checkpoint: Vec<Checkpoint>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Press {
    pub button: String,
    pub at: Option<u64>,
    pub from: Option<u64>,
    pub to: Option<u64>,
    pub every: Option<u64>,
    pub hold: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub name: String,
    pub frame: u64,
    pub hash: String,
    pub mgba_hash: Option<String>,
    pub mgba_why: Option<String>,
    pub min_compiled: Option<f64>,
    #[serde(default)]
    pub block_timed: Vec<u64>,
    pub block_timed_why: Option<String>,
}

impl Checkpoint {
    /// The hash mGBA must draw here.
    pub fn mgba_want(&self) -> &str {
        self.mgba_hash.as_deref().unwrap_or(&self.hash)
    }
}

/// A route, checked.
#[derive(Debug, PartialEq)]
pub struct Route {
    pub rom: PathBuf,
    pub state: Option<PathBuf>,
    /// The presses, as `--input` takes them (empty: none).
    pub input: String,
    /// In frame order.
    pub checkpoints: Vec<Checkpoint>,
}

impl Route {
    pub fn read(path: &Path, vars: &Vars) -> Result<Route, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Route::parse(&text, vars).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str, vars: &Vars) -> Result<Route, String> {
        let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
        let presses: Vec<String> = f.press.iter().map(press).collect::<Result<_, _>>()?;
        let mut input = Vec::new();
        for p in presses {
            input.extend(p.split(',').filter(|s| !s.is_empty()).map(String::from));
        }
        if f.checkpoint.is_empty() {
            return Err("no [[checkpoint]]".into());
        }
        for (i, c) in f.checkpoint.iter().enumerate() {
            let at = format!("checkpoint {:?}", c.name);
            if c.name.is_empty()
                || !c
                    .name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            {
                return Err(format!("{at}: a name is letters, digits and '-'"));
            }
            if f.checkpoint[..i].iter().any(|d| d.name == c.name) {
                return Err(format!("{at}: named twice"));
            }
            if c.frame == 0 {
                return Err(format!("{at}: frames count from 1"));
            }
            if i > 0 && c.frame <= f.checkpoint[i - 1].frame {
                return Err(format!(
                    "{at}: frame {} isn't after the one before's",
                    c.frame
                ));
            }
            for h in std::iter::once(&c.hash).chain(&c.mgba_hash) {
                if h.len() != 16 || !h.chars().all(|ch| ch.is_ascii_hexdigit()) {
                    return Err(format!("{at}: hash {h:?} isn't 16 hex digits"));
                }
            }
            if c.mgba_hash.is_some() != c.mgba_why.is_some() {
                return Err(format!("{at}: mgba_hash and mgba_why go together"));
            }
            if c.mgba_hash.as_ref() == Some(&c.hash) {
                return Err(format!("{at}: mgba_hash is hash"));
            }
            if c.min_compiled.is_some_and(|m| !(0.0..=1.0).contains(&m)) {
                return Err(format!("{at}: min_compiled is a share, from 0 to 1"));
            }
            let after = if i > 0 { f.checkpoint[i - 1].frame } else { 0 };
            if c.block_timed.is_empty() == c.block_timed_why.is_some() {
                return Err(format!("{at}: block_timed and block_timed_why go together"));
            }
            if let Some(b) = c.block_timed.iter().find(|&&b| b <= after || b >= c.frame) {
                return Err(format!(
                    "{at}: block_timed frame {b} isn't after the checkpoint before and before \
                     this one"
                ));
            }
        }
        Ok(Route {
            rom: vars.expand(&f.rom).into(),
            state: f.state.as_deref().map(|s| vars.expand(s).into()),
            input: input.join(","),
            checkpoints: f.checkpoint,
        })
    }

    /// The last frame to run.
    pub fn frames(&self) -> u64 {
        self.checkpoints.last().map_or(0, |c| c.frame)
    }

    /// The route cut at checkpoint `name`, its last.
    pub fn through(mut self, name: &str) -> Result<Route, String> {
        let i = self
            .checkpoints
            .iter()
            .position(|c| c.name == name)
            .ok_or(format!("no checkpoint {name:?}"))?;
        self.checkpoints.truncate(i + 1);
        Ok(self)
    }
}

/// One `[[press]]` as `--input` presses, comma-separated.
fn press(p: &Press) -> Result<String, String> {
    let at = format!("press of {:?}", p.button);
    if !BUTTONS.contains(&p.button.as_str()) {
        return Err(format!("{at}: buttons are {}", BUTTONS.join(" ")));
    }
    let hold = p.hold.unwrap_or(HOLD);
    if hold == 0 {
        return Err(format!("{at}: hold is at least 1"));
    }
    let frames: Vec<u64> = match (p.at, p.from, p.to, p.every) {
        (Some(at), None, None, None) => vec![at],
        (None, Some(from), Some(to), Some(every)) if every > 0 && from <= to => {
            (from..=to).step_by(every as usize).collect()
        }
        _ => {
            return Err(format!(
                "{at}: give `at`, or `from`, `to` (at or after from) and `every` (at least 1)"
            ));
        }
    };
    Ok(frames
        .iter()
        .map(|f| format!("{}@{f}+{hold}", p.button))
        .collect::<Vec<_>>()
        .join(","))
}

/// What `$HOME` and `$GBAA_DATA` stand for.
pub struct Vars {
    pub home: String,
    pub data: String,
}

impl Vars {
    pub fn from_env() -> Vars {
        let home = std::env::var("HOME").unwrap_or_default();
        let data = std::env::var("GBAA_DATA")
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or(format!("{home}/.local/share/gbaa"));
        Vars { home, data }
    }

    fn expand(&self, s: &str) -> String {
        s.replace("$GBAA_DATA", &self.data)
            .replace("$HOME", &self.home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        Vars {
            home: "/h".into(),
            data: "/d".into(),
        }
    }

    const ROUTE: &str = r#"
rom = "$HOME/r.gba"
[[press]]
button = "start"
at = 10
[[press]]
button = "a"
from = 20
to = 60
every = 20
hold = 2
[[checkpoint]]
name = "one"
frame = 5
hash = "0123456789abcdef"
[[checkpoint]]
name = "two"
frame = 70
hash = "0123456789abcdee"
mgba_hash = "0123456789abcded"
mgba_why = "known"
min_compiled = 0.5
block_timed = [6, 69]
block_timed_why = "a store mid-block"
"#;

    #[test]
    fn reads_a_route() {
        let r = Route::parse(ROUTE, &vars()).unwrap();
        assert_eq!(r.rom, PathBuf::from("/h/r.gba"));
        assert_eq!(r.input, "start@10+6,a@20+2,a@40+2,a@60+2");
        assert_eq!(r.frames(), 70);
        assert_eq!(r.checkpoints[1].mgba_want(), "0123456789abcded");
        assert_eq!(r.checkpoints[0].mgba_want(), "0123456789abcdef");
        let cut = Route::parse(ROUTE, &vars())
            .unwrap()
            .through("one")
            .unwrap();
        assert_eq!(cut.frames(), 5);
        assert!(
            Route::parse(ROUTE, &vars())
                .unwrap()
                .through("three")
                .is_err()
        );
    }

    #[test]
    fn refuses_bad_routes() {
        let bad = |from: &str, to: &str| {
            let text = ROUTE.replacen(from, to, 1);
            assert_ne!(text, ROUTE, "{from}");
            Route::parse(&text, &vars()).unwrap_err()
        };
        bad("\"start\"", "\"go\"");
        bad("every = 20", "every = 0");
        bad("to = 60", "to = 10");
        bad("at = 10", "at = 10\nfrom = 3");
        bad("hold = 2", "hold = 0");
        bad("frame = 70", "frame = 5");
        bad("frame = 5", "frame = 0");
        bad("name = \"two\"", "name = \"one\"");
        bad("name = \"two\"", "name = \"t o\"");
        bad("0123456789abcdee", "0123");
        bad("mgba_why = \"known\"\n", "");
        bad("0123456789abcded", "0123456789abcdee");
        bad("min_compiled = 0.5", "min_compiled = 1.5");
        bad("hold = 2", "held = 2");
        bad("block_timed_why = \"a store mid-block\"\n", "");
        bad("[6, 69]", "[5, 69]");
        bad("[6, 69]", "[6, 70]");
        let none = Route::parse("rom = \"x\"", &vars()).unwrap_err();
        assert!(none.contains("checkpoint"), "{none}");
    }

    #[test]
    fn expands_paths() {
        let text = ROUTE.replace("$HOME/r.gba", "$GBAA_DATA/agb/x.gba");
        let text = text.replacen("[[press]]", "state = \"$HOME/s.ss\"\n[[press]]", 1);
        let r = Route::parse(&text, &vars()).unwrap();
        assert_eq!(r.rom, PathBuf::from("/d/agb/x.gba"));
        assert_eq!(r.state, Some(PathBuf::from("/h/s.ss")));
    }
}
