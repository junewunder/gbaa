//! A BizHawk movie's input log (`Input Log.txt` inside a `.bk2`, a zip):
//! one line a frame, `|tilt x, tilt y, tilt z, light,UDLRSsBAlrP|`, a
//! letter for each button held that frame and `.` for one up. `P` is the
//! power button: BizHawk resets the core before that frame. Movies of
//! BizHawk's mGBA core replay in `mgba-dump --bk2`.

use runtime::io::key;

/// BizHawk's default `RTCInitialTime` for its mGBA core, 2010-01-01 00:00
/// UTC, in Unix seconds.
pub const RTC_INITIAL_TIME: i64 = 1_262_304_000;

/// The log's buttons, in the order its lines hold them (power last).
const BUTTONS: [u16; 10] = [
    key::UP,
    key::DOWN,
    key::LEFT,
    key::RIGHT,
    key::START,
    key::SELECT,
    key::B,
    key::A,
    key::L,
    key::R,
];

/// A frame's input: the keys held, and whether the core resets first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub keys: u16,
    pub power: bool,
}

/// Each frame's input, from the first.
pub fn parse(log: &str) -> Result<Vec<Input>, String> {
    let mut frames = Vec::new();
    for (n, line) in log.lines().enumerate() {
        let line = line.trim_end();
        let Some(body) = line.strip_prefix('|').and_then(|l| l.strip_suffix('|')) else {
            continue;
        };
        let bad = || format!("input log line {}: {line:?}", n + 1);
        let buttons = body.rsplit(',').next().ok_or_else(bad)?.as_bytes();
        if buttons.len() != BUTTONS.len() + 1 {
            return Err(bad());
        }
        let keys = BUTTONS
            .iter()
            .zip(buttons)
            .filter(|&(_, &c)| c != b'.')
            .fold(0, |k, (&b, _)| k | b);
        frames.push(Input {
            keys,
            power: buttons[BUTTONS.len()] != b'.',
        });
    }
    if frames.is_empty() {
        return Err("no input lines (expected an Input Log.txt)".into());
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let log = "[Input]\r\nLogKey:#Tilt X|Tilt Y|Tilt Z|Light Sensor|Up|Down|Left|Right|Start|Select|B|A|L|R|Power|\r\n\
                   |    0,    0,    0,  100,...........|\r\n\
                   |    0,    0,    0,  100,U......A.r.|\r\n\
                   |    0,    0,    0,    0,..L.Ss..l.P|\r\n[/Input]\r\n";
        let f = parse(log).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], Input::default());
        assert_eq!(f[1].keys, key::UP | key::A | key::R);
        assert_eq!(f[2].keys, key::LEFT | key::START | key::SELECT | key::L);
        assert!(f[2].power && !f[1].power);
        assert!(parse("|0,0,0,0,....|").is_err());
        assert!(parse("nothing").is_err());
    }
}
