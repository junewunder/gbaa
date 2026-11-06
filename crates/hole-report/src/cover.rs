//! `--cover FILE` (native/hole-coverage; `docs/research/native-holes.md`,
//! "Coverage runs"): runs past the press script, and the list of what they
//! saw at each unresolved indirect jump.
//!
//! FILE (`holes/coverage/<game>.toml`) has `[[run]]` tables, each a set of
//! start points (reset, a save file or folder of them, an mGBA state or
//! folder of them) played for `frames` with a press script and, from
//! `random_from`, seeded random presses; and, after the [`MARKER`] line,
//! the `[[jump]]` list `--update` writes: for every jump Recovery leaves
//! unresolved, the runs that reached it and the targets it took. Without
//! `--update` the runs' sightings are checked against that list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The line after which `--update` rewrites the file.
pub const MARKER: &str =
    "# --- Observed: written by `gbaa-holes ROM --cover FILE --update`; checked without it ---";

/// Targets kept per jump; a jump that took more says so (`more`).
pub const MAX_TARGETS: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    #[serde(default)]
    pub run: Vec<RunSpec>,
    #[serde(default)]
    pub jump: Vec<Jump>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    pub name: String,
    /// A save file, or a folder: each `.sav` in it is a start point.
    /// Relative paths are under `$GBAA_DATA`.
    pub save: Option<String>,
    /// An mGBA state, or a folder: each `.ss` in it is a start point.
    pub state: Option<String>,
    /// Of a folder's files (sorted by name), every Nth, from the first.
    #[serde(default = "one")]
    pub every: usize,
    /// Frames played from each start point.
    pub frames: u32,
    /// The press script (`KEY@F` or `KEY@F-G/E`), frames counted from the
    /// start point.
    #[serde(default)]
    pub presses: Vec<String>,
    /// Random presses from this frame on, as well as the script's.
    pub random_from: Option<u32>,
    /// With random presses, how many seeds (1..=N) each start point plays.
    #[serde(default = "one_u32")]
    pub seeds: u32,
}

fn one() -> usize {
    1
}

fn one_u32() -> u32 {
    1
}

#[derive(Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(deny_unknown_fields)]
pub struct Jump {
    pub at: u32,
    /// The names of the runs that executed it; empty: no run reached it.
    pub runs: Vec<String>,
    /// Each target it took, in interworking form (bit 0: Thumb).
    pub targets: Vec<u32>,
    /// It took more than [`MAX_TARGETS`].
    #[serde(default)]
    pub more: bool,
}

/// What one start point is.
pub enum Start {
    Reset,
    Save(Vec<u8>),
    State(Vec<u8>, PathBuf),
}

pub fn data_dir() -> PathBuf {
    match std::env::var_os("GBAA_DATA") {
        Some(d) => PathBuf::from(d),
        None => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            Path::new(&home).join(".local/share/gbaa")
        }
    }
}

fn resolve(p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        data_dir().join(p)
    }
}

/// The files of `p` (itself, or a folder's ending in `.ext`, sorted).
fn files(p: &str, ext: &str, every: usize) -> Result<Vec<PathBuf>, String> {
    let p = resolve(p);
    if !p.is_dir() {
        return Ok(vec![p]);
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
        .map_err(|e| format!("{}: {e}", p.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|f| f.extension().is_some_and(|x| x == ext))
        .collect();
    // state-200000.ss before state-1000000.ss: by length, then name.
    v.sort_by_key(|f| {
        let n = f
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        (n.len(), n)
    });
    Ok(v.into_iter().step_by(every.max(1)).collect())
}

impl RunSpec {
    pub fn starts(&self) -> Result<Vec<Start>, String> {
        let read = |f: &Path| std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()));
        let mut out = Vec::new();
        if let Some(s) = &self.save {
            for f in files(s, "sav", self.every)? {
                out.push(Start::Save(read(&f)?));
            }
        }
        if let Some(s) = &self.state {
            for f in files(s, "ss", self.every)? {
                out.push(Start::State(read(&f)?, f));
            }
        }
        if self.save.is_none() && self.state.is_none() {
            out.push(Start::Reset);
        }
        Ok(out)
    }
}

pub fn load(path: &str) -> Result<(String, File), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let file: File = toml::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    if file.run.len() > 63 {
        return Err(format!("{path}: more than 63 runs"));
    }
    Ok((text, file))
}

/// `text` up to the marker, then `jumps` as TOML.
pub fn write(text: &str, jumps: &[Jump]) -> String {
    let mut out: String = match text.find(MARKER) {
        Some(i) => text[..i].to_string(),
        None => {
            let mut t = text.trim_end().to_string();
            t.push_str("\n\n");
            t
        }
    };
    out.push_str(MARKER);
    out.push('\n');
    let hex = |v: &[u32]| {
        v.iter()
            .map(|t| format!("0x{t:08x}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for j in jumps {
        let runs: Vec<String> = j.runs.iter().map(|r| format!("{r:?}")).collect();
        out.push_str(&format!(
            "\n[[jump]]\nat = 0x{:08x}\nruns = [{}]\ntargets = [{}]\n",
            j.at,
            runs.join(", "),
            hex(&j.targets)
        ));
        if j.more {
            out.push_str("more = true\n");
        }
    }
    out
}

/// The differences between the list and what the runs saw, one line each.
pub fn check(listed: &[Jump], seen: &[Jump]) -> Vec<String> {
    let a: BTreeMap<u32, &Jump> = listed.iter().map(|j| (j.at, j)).collect();
    let b: BTreeMap<u32, &Jump> = seen.iter().map(|j| (j.at, j)).collect();
    let mut out = Vec::new();
    for at in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
        match (a.get(at), b.get(at)) {
            (Some(_), None) => out.push(format!("0x{at:08x}: listed, no longer unresolved")),
            (None, Some(_)) => out.push(format!("0x{at:08x}: unresolved, not listed")),
            (Some(x), Some(y)) if x != y => {
                let ts = |j: &Jump| j.targets.iter().copied().collect::<BTreeSet<u32>>();
                let new: Vec<String> = ts(y)
                    .difference(&ts(x))
                    .map(|t| format!("0x{t:08x}"))
                    .collect();
                let gone: Vec<String> = ts(x)
                    .difference(&ts(y))
                    .map(|t| format!("0x{t:08x}"))
                    .collect();
                out.push(format!(
                    "0x{at:08x}: runs {:?} listed, {:?} seen; targets new [{}], not seen [{}]",
                    x.runs,
                    y.runs,
                    new.join(", "),
                    gone.join(", ")
                ));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_list_reads_back_and_checks() {
        let text = "[[run]]\nname = \"walk\"\nframes = 10\nrandom_from = 0\n";
        let seen = vec![
            Jump {
                at: 0x0800_0100,
                runs: vec!["press".into(), "walk".into()],
                targets: vec![0x0800_0201, 0x0300_0000],
                more: false,
            },
            Jump {
                at: 0x0800_0200,
                runs: vec![],
                targets: vec![],
                more: false,
            },
        ];
        let out = write(text, &seen);
        let file: File = toml::from_str(&out).unwrap();
        assert_eq!(file.run.len(), 1);
        assert_eq!(file.jump, seen);
        assert!(check(&file.jump, &seen).is_empty());
        // Writing again replaces the list rather than adding to it.
        assert_eq!(write(&out, &seen), out);
        let mut moved = seen.clone();
        moved[0].targets.push(0x0800_0301);
        moved.remove(1);
        let d = check(&file.jump, &moved);
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(d[0].contains("new [0x08000301]"), "{d:?}");
        assert!(d[1].contains("no longer unresolved"), "{d:?}");
    }
}
