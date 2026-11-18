//! Scripted input for headless runs, as `gbaa-desktop --input` and the
//! netplay feel sim ([`crate::feel`]) read it: `start@60,down@90+2` holds
//! Start for [`HOLD`] frames from frame 60 and Down for two frames from
//! frame 90. Frames count from 0, the first frame run; a press shows in
//! `KEYINPUT` from that frame's V-blank, when the runner reads the keys.
//! `a@1400-3400/60` presses A every 60 frames from 1400 to 3400.

use runtime::io::key;

/// How many frames a press holds its button when the script doesn't say.
/// Long enough for a game that polls the keys only every few frames.
pub const HOLD: u64 = 6;

/// Button names in scripts.
const NAMES: [(&str, u16); 10] = [
    ("a", key::A),
    ("b", key::B),
    ("select", key::SELECT),
    ("start", key::START),
    ("right", key::RIGHT),
    ("left", key::LEFT),
    ("up", key::UP),
    ("down", key::DOWN),
    ("r", key::R),
    ("l", key::L),
];

/// A press's `frame[+frames]`, `frame` maybe `from-to/every`: when each
/// press starts and how many frames it holds.
pub fn when(at: &str) -> Option<impl Iterator<Item = (u64, u64)>> {
    let (at, frames) = match at.split_once('+') {
        Some((f, n)) => (f, n.parse().ok().filter(|&n| n > 0)?),
        None => (at, HOLD),
    };
    let (from, to, every) = match at.split_once('-') {
        Some((from, rest)) => {
            let (to, every) = rest.split_once('/')?;
            (from.parse().ok()?, to.parse().ok()?, every.parse().ok()?)
        }
        None => {
            let at = at.parse().ok()?;
            (at, at, 1)
        }
    };
    if every == 0 || to < from {
        return None;
    }
    let at = (from..=to).step_by(usize::try_from(every).ok()?);
    Some(at.map(move |from| (from, frames)))
}

/// A list of button presses, each held over a range of frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script(Vec<Press>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Press {
    keys: u16,
    from: u64,
    frames: u64,
}

impl Script {
    /// Parse `button@frame[+frames],...`, where `frame` may be
    /// `from-to/every`: a press every `every` frames from `from` to `to`.
    pub fn parse(s: &str) -> Result<Script, String> {
        let presses = |p: &str| -> Option<Vec<Press>> {
            let (name, at) = p.split_once('@')?;
            let keys = NAMES.iter().find(|(n, _)| *n == name)?.1;
            let at = when(at)?;
            Some(
                at.map(|(from, frames)| Press { keys, from, frames })
                    .collect(),
            )
        };
        s.split(',')
            .map(|p| {
                presses(p.trim()).ok_or(format!(
                    "bad press {p:?} (button@frame[+frames], frame N or from-to/every)"
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|v| Script(v.concat()))
    }

    /// The buttons held during `frame`.
    pub fn keys(&self, frame: u64) -> u16 {
        self.0
            .iter()
            .filter(|p| (p.from..p.from.saturating_add(p.frames)).contains(&frame))
            .fold(0, |keys, p| keys | p.keys)
    }
}

/// Spans of frames, `frame[+frames],...` as a press's, without the
/// button: when headless netplay's `--menu` is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spans(Vec<(u64, u64)>);

impl Spans {
    pub fn parse(s: &str) -> Result<Spans, String> {
        s.split(',')
            .map(|p| {
                when(p.trim())
                    .map(Iterator::collect::<Vec<_>>)
                    .ok_or(format!(
                        "bad span {p:?} (frame[+frames], frame N or from-to/every)"
                    ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|v| Spans(v.concat()))
    }

    /// Whether `frame` is in a span.
    pub fn contains(&self, frame: u64) -> bool {
        self.0
            .iter()
            .any(|&(from, n)| (from..from.saturating_add(n)).contains(&frame))
    }
}
