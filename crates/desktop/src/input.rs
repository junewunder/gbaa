//! Scripted input for headless runs: `--input 'start@60,down@90+2'`
//! holds Start for [`netplay::script::HOLD`] frames from frame 60 and Down for two frames
//! from frame 90. Frames count from 0, the first frame run; a press shows
//! in `KEYINPUT` from that frame's V-blank, when the runner reads the keys.
//! `a@1400-3400/60` presses A every 60 frames from 1400 to 3400. The
//! parsing is [`netplay::script`]'s, which the netplay feel sim reads too.

use netplay::script::when;
pub use netplay::script::{Script, Spans};

/// The code mGBA's Qt window gives the key that types `c`: its capital
/// below U+0100 (Qt's key code), else `c` (the typed text).
pub fn key_code(c: char) -> u32 {
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) if u32::from(u) < 0x100 => u32::from(u),
        _ => u32::from(c),
    }
}

/// `--script-keys`: keyboard keys held for the scripts' `input`
/// (`script::input`), as `--input`'s presses: `key@frame[+frames],...`.
/// A key is a character, as mGBA's window gives it (a letter's capital, a
/// shifted key's character: `!`, not shift and 1), `u+HEX` (a codepoint:
/// `u+2c` for a comma), `space`, or a `C.KEY` name in any case (`shift`,
/// `f1`, `kp_0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyScript(Vec<(u32, u64, u64)>);

impl KeyScript {
    pub fn parse(s: &str) -> Result<KeyScript, String> {
        let presses = |p: &str| -> Option<Vec<(u32, u64, u64)>> {
            let (name, at) = p.rsplit_once('@')?;
            let mut c = name.chars();
            let key = match (c.next(), c.next()) {
                (Some(k), None) => key_code(k),
                _ if name.eq_ignore_ascii_case("space") => 0x20,
                _ => match name.strip_prefix("u+").or(name.strip_prefix("U+")) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok().filter(|&k| k > 0)?,
                    None => script::input::key(name).filter(|&k| k > 0)?,
                },
            };
            Some(when(at)?.map(|(from, n)| (key, from, n)).collect())
        };
        s.split(',')
            .map(|p| {
                presses(p.trim()).ok_or(format!(
                    "bad key press {p:?} (key@frame[+frames]; a key is a character, \
                     u+HEX, space or a C.KEY name)"
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|v| KeyScript(v.concat()))
    }

    /// The keys held during `frame`, and the modifiers (`C.KMOD`) they make.
    pub fn held(&self, frame: u64) -> (std::collections::BTreeSet<u32>, u16) {
        use script::input::{key, kmod};
        let held: std::collections::BTreeSet<u32> = self
            .0
            .iter()
            .filter(|&&(_, from, n)| (from..from.saturating_add(n)).contains(&frame))
            .map(|&(k, _, _)| k)
            .collect();
        let mods = [
            ("SHIFT", kmod::SHIFT),
            ("CONTROL", kmod::CONTROL),
            ("ALT", kmod::ALT),
            ("SUPER", kmod::SUPER),
        ]
        .iter()
        .filter(|(n, _)| key(n).is_some_and(|k| held.contains(&k)))
        .fold(0, |m, &(_, b)| m | b);
        (held, mods)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_parse_as_presses_do() {
        let s = Spans::parse("10+3, 100-200/50+20").unwrap();
        assert!(!s.contains(9) && s.contains(10) && s.contains(12) && !s.contains(13));
        assert!(s.contains(150) && s.contains(169) && !s.contains(170));
        assert!(s.contains(219) && !s.contains(220));
        assert!(Spans::parse("x").is_err() && Spans::parse("5+0").is_err());
    }
}
