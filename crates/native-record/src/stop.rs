//! The stop line (unit `native/native-first-stop`): what a native program
//! prints to stderr when it stops before its frames run out, and what
//! `gbaa-first-stop` reads to say where a lifted game stopped.
//!
//! One line, [`PREFIX`] then tab-separated `key=value` fields, the free
//! text last (its tabs and line ends made spaces):
//!
//! ```text
//! native-stop:\tframes=0\tasked=600\tfunctions=0800a285,080002d1\tat=crates/runtime/src/hw.rs:665\tpanic=hw: I/O register 0x134 isn't modelled
//! native-stop:\tframes=12\tasked=600\tfunctions=\tat=\ttime-limit=120
//! ```
//!
//! `frames` is how many it presented, `asked` its `--frames`.
//! `functions` are the lifted functions on the stack at a panic, innermost
//! first, as their keys (the address, bit 0 set for Thumb): read from the
//! backtrace's symbol names ([`functions_in`]), so one the compiler
//! inlined into its caller doesn't show, and a hand-written program has
//! none. No guest pc: compiled code keeps none.

/// What every stop line starts with.
pub const PREFIX: &str = "native-stop:";

/// At most this many functions are kept.
pub const FUNCTIONS: usize = 8;

/// Why the program stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// A panic, with its message.
    Panic(String),
    /// `--time-limit`'s seconds passed first.
    TimeLimit(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    /// Frames presented before it.
    pub frames: u64,
    /// `--frames`.
    pub asked: u64,
    /// Lifted functions on the stack, innermost first.
    pub functions: Vec<u32>,
    /// The panic's place in the Rust source, `file:line`.
    pub at: String,
    pub why: Why,
}

fn one_line(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

impl Stop {
    /// The line, without its line end.
    pub fn line(&self) -> String {
        let fs: Vec<String> = self.functions.iter().map(|k| format!("{k:08x}")).collect();
        let why = match &self.why {
            Why::Panic(m) => format!("panic={}", one_line(m)),
            Why::TimeLimit(s) => format!("time-limit={s}"),
        };
        format!(
            "{PREFIX}\tframes={}\tasked={}\tfunctions={}\tat={}\t{why}",
            self.frames,
            self.asked,
            fs.join(","),
            one_line(&self.at),
        )
    }

    /// Read one line back; `None` for any other line.
    pub fn parse(line: &str) -> Option<Stop> {
        let mut it = line.trim_end_matches(['\r', '\n']).splitn(6, '\t');
        if it.next()? != PREFIX {
            return None;
        }
        let frames = it.next()?.strip_prefix("frames=")?.parse().ok()?;
        let asked = it.next()?.strip_prefix("asked=")?.parse().ok()?;
        let functions = it
            .next()?
            .strip_prefix("functions=")?
            .split(',')
            .filter(|k| !k.is_empty())
            .map(|k| u32::from_str_radix(k, 16).ok())
            .collect::<Option<_>>()?;
        let at = it.next()?.strip_prefix("at=")?.to_string();
        let why = it.next()?;
        let why = match why.strip_prefix("panic=") {
            Some(m) => Why::Panic(m.to_string()),
            None => Why::TimeLimit(why.strip_prefix("time-limit=")?.parse().ok()?),
        };
        Some(Stop {
            frames,
            asked,
            functions,
            at,
            why,
        })
    }

    /// The last stop line in a program's stderr.
    pub fn find(stderr: &str) -> Option<Stop> {
        stderr.lines().rev().find_map(Stop::parse)
    }
}

/// The lifted functions a backtrace's text names, innermost first: each
/// symbol `x_`, `f_` or `a_` and eight hex digits (the lifter's names for
/// a function's body, its entry and its four-register entry), a key that
/// repeats the one before it dropped, at most [`FUNCTIONS`].
pub fn functions_in(backtrace: &str) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let b = backtrace.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    for i in 0..b.len() {
        if out.len() == FUNCTIONS {
            break;
        }
        // "::x_0800a285", not part of a longer name on either side.
        let hit = i >= 2
            && &b[i - 2..i] == b"::"
            && matches!(b[i], b'x' | b'f' | b'a')
            && b.get(i + 1) == Some(&b'_')
            && b.get(i + 10).is_none_or(|&c| !ident(c));
        if !hit {
            continue;
        }
        let Some(k) = backtrace
            .get(i + 2..i + 10)
            .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
            .and_then(|h| u32::from_str_radix(h, 16).ok())
        else {
            continue;
        };
        if out.last() != Some(&k) {
            out.push(k);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_reads_back() {
        let s = Stop {
            frames: 3,
            asked: 600,
            functions: vec![0x0800_a285, 0x0300_0000],
            at: "crates/runtime/src/hw.rs:665".into(),
            why: Why::Panic("hw: I/O register 0x134 isn't modelled".into()),
        };
        assert_eq!(Stop::parse(&s.line()), Some(s.clone()));
        let t = Stop {
            functions: vec![],
            at: String::new(),
            why: Why::TimeLimit(120),
            ..s.clone()
        };
        assert_eq!(Stop::parse(&(t.line() + "\n")), Some(t.clone()));
        // The last one of a program's stderr, among other lines.
        let err = format!("thread 'main' panicked\n{}\nnote\n{}\n", s.line(), t.line());
        assert_eq!(Stop::find(&err), Some(t));
        assert_eq!(Stop::find("thread 'main' panicked at x\n"), None);
    }

    #[test]
    fn a_message_of_several_lines_stays_one() {
        let s = Stop {
            frames: 0,
            asked: 1,
            functions: vec![],
            at: "a\tb".into(),
            why: Why::Panic("left\n  right\tend".into()),
        };
        let l = s.line();
        assert_eq!(l.lines().count(), 1);
        let p = Stop::parse(&l).unwrap();
        assert_eq!(p.why, Why::Panic("left   right end".into()));
        assert_eq!(p.at, "a b");
    }

    #[test]
    fn functions_from_a_backtrace() {
        let bt = "   0: std::backtrace::Backtrace::force_capture\n\
                  \x20  3: runtime::hw::Hw<G>::write16\n\
                  \x20  4: gbaa_native_p::st16\n\
                  \x20  5: gbaa_native_p::m_0800a285::x_0800a285\n\
                  \x20  6: gbaa_native_p::m_0800a285::f_0800a285\n\
                  \x20  7: gbaa_native_p::m_08000000::x_080002d1\n\
                  \x20  8: gbaa_native_p::m_08000000::a_080002d1\n\
                  \x20  9: gbaa_native_p::m_08000000::x_0800zzzz\n\
                  \x20 10: gbaa_native_p::globals::d_03000010\n\
                  \x20 11: gbaa_native_p::m_08000000::f_080000c0::{{closure}}\n\
                  \x20 12: gbaa_native_p::m_08000000::f_080000c01\n\
                  \x20 13: gbaa_native_p::main\n";
        assert_eq!(
            functions_in(bt),
            vec![0x0800_a285, 0x0800_02d1, 0x0800_00c0]
        );
        assert!(functions_in("native_spike::game::main\n").is_empty());
        let many: String = (0..20)
            .map(|i| format!("  {i}: p::m::x_{:08x}\n", 0x0800_0000 + i))
            .collect();
        assert_eq!(functions_in(&many).len(), FUNCTIONS);
    }
}
