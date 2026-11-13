//! Where a built native game stops (unit `native/native-first-stop`).
//!
//! A lift that builds isn't a game that runs: the hardware library panics
//! on a register it doesn't model, the prelude on memory the program
//! hasn't got. `gbaa-first-stop OUT` (the last step of `just lift`) runs
//! the built program headless for [`FRAMES`] frames under a memory cap and
//! records how far it got, a [`Run`]: in `OUT/metrics.toml` as a `[run]`
//! table ([`Run::write_metrics`]; [`super::metrics::Metrics::from_toml`]
//! skips it), at the end of `OUT/report.txt` ([`Run::write_report`]), and,
//! read back from the lift ([`Run::read`]), in the hole report's corpus
//! table beside the hard holes (`gbaa-holes --corpus`). A stop is the
//! game's next hole.
//!
//! The program says where it stopped itself, in its stop line
//! (`native_record::stop`, printed by `native-host`): the frames presented,
//! the panic and the lifted functions on the stack. [`Run::of`] names the
//! stop's kind from the panic's text, so the texts here are those of
//! `runtime::hw`'s `write16`/`read16` and the prelude's `unmapped` and
//! `call_fn`; one that changes there falls to `panic`, with its message.

use native_record::stop::{Stop, Why};

/// The frames `just lift` runs a built program for.
pub const FRAMES: u64 = 600;

/// Seconds a run may take before it counts as a stop (`time-limit`).
pub const TIME_LIMIT: u64 = 120;

/// The stop kinds, as `stop = "…"` holds them.
pub mod kind {
    /// Every frame asked for was presented.
    pub const NONE: &str = "none";
    /// The crate didn't build: its hard holes are the stop.
    pub const NOT_BUILT: &str = "not-built";
    /// An I/O register the hardware library doesn't model.
    pub const REGISTER: &str = "unmodelled-register";
    /// A load or store where the program has no memory.
    pub const MEMORY: &str = "no-memory";
    /// A call or jump to an address that is no function's entry.
    pub const FUNCTION: &str = "no-function";
    /// Any other panic.
    pub const PANIC: &str = "panic";
    /// Still running when the time limit passed.
    pub const TIME: &str = "time-limit";
    /// Killed by a signal (the memory cap's is 9), with no stop line.
    pub const KILLED: &str = "killed";
    /// An error exit with no stop line.
    pub const FAILED: &str = "failed";
}

/// How the program's process ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ended {
    /// Its exit code, if it exited.
    pub code: Option<i32>,
    /// The signal that killed it, if one did.
    pub signal: Option<i32>,
    /// Its stderr (the end of it will do).
    pub stderr: String,
}

/// One headless run of a built lift.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Frames presented before the stop (0 when that isn't known: killed).
    pub frames_run: u64,
    pub frames_asked: u64,
    /// The stop's kind ([`kind`]).
    pub stop: String,
    /// What it stopped on, short: a register's address and name, an
    /// address, the hard holes' count. Empty when the kind says it all.
    pub what: String,
    /// The whole of it: the panic's message and place.
    pub detail: String,
    /// The lifted functions on the stack, innermost first (keys).
    pub functions: Vec<u32>,
}

/// The first `0x…` number in `s`.
fn hex_in(s: &str) -> Option<u32> {
    let h = &s[s.find("0x")? + 2..];
    let end = h.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(h.len());
    u32::from_str_radix(&h[..end], 16).ok()
}

/// A panic message's kind and what it names.
fn classify(message: &str) -> (&'static str, String) {
    if let Some(r) = message.strip_prefix("hw: I/O register ")
        && message.ends_with("isn't modelled")
        && let Some(o) = hex_in(r)
    {
        let a = 0x0400_0000 + o;
        let name = match crate::io_regs::at(a, 2) {
            Some((n, None)) => format!(" {n}"),
            Some((n, Some(off))) => format!(" {n}+{off}"),
            None => String::new(),
        };
        return (kind::REGISTER, format!("{a:#010x}{name}"));
    }
    if let Some(w) = message.strip_suffix(": no memory the program has there") {
        return (kind::MEMORY, w.to_string());
    }
    if message.starts_with("a call or jump to ")
        && let Some(a) = hex_in(message)
    {
        return (kind::FUNCTION, format!("{a:#010x}"));
    }
    (kind::PANIC, String::new())
}

/// A panic's place without the checkout it was built in: from `crates/`
/// on (a path dependency's file is absolute), else as it is.
fn place(at: &str) -> &str {
    at.rfind("/crates/").map_or(at, |i| &at[i + 1..])
}

impl Run {
    /// The crate didn't build, with `hard_holes` hard holes in its lift,
    /// `report` the lift's `report.txt`, which names each (the first is
    /// quoted: its function and address are where to start).
    pub fn not_built(frames_asked: u64, hard_holes: u64, report: &str) -> Run {
        let first = report
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with("hard hole in "));
        let (what, detail) = match (hard_holes, first) {
            (0, _) => (
                "the build failed".to_string(),
                "the lift has no hard hole, yet its crate didn't build".to_string(),
            ),
            (n, first) => (
                format!("{n} hard hole{}", if n == 1 { "" } else { "s" }),
                format!(
                    "{}each is a compile_error! in the crate, named in the lift's report",
                    first.map_or(String::new(), |f| format!("the first: {f}; "))
                ),
            ),
        };
        Run {
            frames_run: 0,
            frames_asked,
            stop: kind::NOT_BUILT.into(),
            what,
            detail,
            functions: Vec::new(),
        }
    }

    /// The run of a program asked for `frames_asked` frames that ended as `e`.
    pub fn of(frames_asked: u64, e: &Ended) -> Run {
        let run = |frames_run, stop: &str, what: String, detail: String, functions| Run {
            frames_run,
            frames_asked,
            stop: stop.into(),
            what,
            detail,
            functions,
        };
        if let Some(s) = Stop::find(&e.stderr) {
            return match s.why {
                Why::Panic(m) => {
                    let (k, what) = classify(&m);
                    let detail = match place(&s.at) {
                        "" => m,
                        at => format!("{m} ({at})"),
                    };
                    run(s.frames, k, what, detail, s.functions)
                }
                Why::TimeLimit(t) => run(
                    s.frames,
                    kind::TIME,
                    format!("{t} s"),
                    format!("still running after {t} s: a loop that reaches no frame"),
                    s.functions,
                ),
            };
        }
        let last = e
            .stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("nothing on stderr")
            .trim();
        match (e.code, e.signal) {
            (Some(0), _) => run(
                frames_asked,
                kind::NONE,
                String::new(),
                String::new(),
                Vec::new(),
            ),
            (_, Some(sig)) => run(
                0,
                kind::KILLED,
                format!("signal {sig}"),
                format!(
                    "killed by signal {sig}{} with no stop line: frames drawn unknown",
                    if sig == 9 {
                        " (the memory cap's, if nothing else sent it)"
                    } else {
                        ""
                    }
                ),
                Vec::new(),
            ),
            (code, None) => run(
                0,
                kind::FAILED,
                format!("exit {}", code.unwrap_or(-1)),
                format!("no stop line; its stderr ends: {last}"),
                Vec::new(),
            ),
        }
    }

    /// Whether every frame asked for ran.
    pub fn ran(&self) -> bool {
        self.stop == kind::NONE
    }

    /// The run in a few words: `600 of 600 frames, no stop`, `0 of 600
    /// frames, unmodelled-register 0x04000134 RCNT`, `didn't build: 3 hard
    /// holes`.
    pub fn summary(&self) -> String {
        if self.stop == kind::NOT_BUILT {
            return format!("didn't build: {}", self.what);
        }
        let frames = format!("{} of {} frames", self.frames_run, self.frames_asked);
        match (self.ran(), self.what.as_str()) {
            (true, _) => format!("{frames}, no stop"),
            (false, "") => format!("{frames}, {}", self.stop),
            (false, w) => format!("{frames}, {} {w}", self.stop),
        }
    }

    /// The functions as the lifter names them, innermost first.
    fn function_names(&self) -> Vec<String> {
        self.functions
            .iter()
            .map(|k| format!("f_{k:08x}"))
            .collect()
    }

    /// The lines that say it all: the summary, then the detail and the
    /// functions when there are any.
    pub fn lines(&self) -> Vec<String> {
        let mut l = vec![self.summary()];
        if !self.detail.is_empty() {
            l.push(self.detail.clone());
        }
        if !self.functions.is_empty() {
            l.push(format!(
                "in {} (the lifted functions on the stack, innermost first; one inlined into its caller doesn't show, and no guest pc is kept)",
                self.function_names().join(" < ")
            ));
        }
        l
    }

    /// `metrics`' text (a lift's `metrics.toml`) with this run as its
    /// `[run]` table, in place of an earlier one.
    pub fn write_metrics(&self, metrics: &str) -> Result<String, String> {
        let mut d: toml_edit::DocumentMut = metrics.parse().map_err(|e| format!("{e}"))?;
        let mut t = toml_edit::Table::new();
        t.decor_mut().set_prefix(
            "\n# Written by gbaa-first-stop: the built program run headless (the last step of `just lift`).\n",
        );
        t["frames_run"] = toml_edit::value(self.frames_run as i64);
        t["frames_asked"] = toml_edit::value(self.frames_asked as i64);
        t["stop"] = toml_edit::value(&self.stop);
        t["what"] = toml_edit::value(&self.what);
        t["detail"] = toml_edit::value(&self.detail);
        let fs: toml_edit::Array = self.function_names().into_iter().collect();
        t["functions"] = toml_edit::value(fs);
        d.remove("run");
        d.insert("run", toml_edit::Item::Table(t));
        Ok(d.to_string())
    }

    /// The `[run]` table of a lift's `metrics.toml`, if it has one.
    pub fn read(metrics: &str) -> Result<Option<Run>, String> {
        let t: toml::Table = metrics.parse().map_err(|e| format!("{e}"))?;
        let Some(r) = t.get("run") else {
            return Ok(None);
        };
        let n = |k: &str| {
            r.get(k)
                .and_then(|v| v.as_integer())
                .and_then(|i| u64::try_from(i).ok())
                .ok_or(format!("run.{k}: not a whole number"))
        };
        let s = |k: &str| {
            r.get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or(format!("run.{k}: not a string"))
        };
        let functions = r
            .get("functions")
            .and_then(|v| v.as_array())
            .ok_or("run.functions: not an array")?
            .iter()
            .map(|v| {
                v.as_str()
                    .and_then(|f| f.strip_prefix("f_"))
                    .and_then(|k| u32::from_str_radix(k, 16).ok())
                    .ok_or(format!("run.functions: {v} isn't f_ and a key"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Some(Run {
            frames_run: n("frames_run")?,
            frames_asked: n("frames_asked")?,
            stop: s("stop")?,
            what: s("what")?,
            detail: s("detail")?,
            functions,
        }))
    }

    /// `report`'s text (a lift's `report.txt`) ending with this run, in
    /// place of an earlier one: its lines, then `frames_run N` and `stop
    /// KIND` as the metrics above them are written.
    pub fn write_report(&self, report: &str) -> String {
        let mut s = match report.find(REPORT_HEAD) {
            Some(i) => report[..i].to_string(),
            None => report.to_string(),
        };
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(REPORT_HEAD);
        for l in self.lines() {
            s.push_str(&format!("  {l}\n"));
        }
        s.push_str(&format!(
            "frames_run {}\nframes_asked {}\nstop {}\n",
            self.frames_run, self.frames_asked, self.stop
        ));
        s
    }
}

/// The line the run's part of `report.txt` starts with.
const REPORT_HEAD: &str = "first stop (the built program run headless; gbaa-first-stop):\n";

#[cfg(test)]
mod tests {
    use super::*;

    fn panicked(frames: u64, at: &str, m: &str, functions: Vec<u32>) -> Ended {
        let s = Stop {
            frames,
            asked: 600,
            functions,
            at: at.into(),
            why: Why::Panic(m.into()),
        };
        Ended {
            code: Some(101),
            signal: None,
            stderr: format!("\nthread 'main' panicked at {at}:\n{m}\n{}\n", s.line()),
        }
    }

    #[test]
    fn an_unmodelled_register_by_address_and_name() {
        // hw.rs's text for `Hw::write16(0x134, …)`.
        let e = panicked(
            7,
            "/home/someone/gbaa/crates/runtime/src/hw.rs:665",
            "hw: I/O register 0x134 isn't modelled",
            vec![0x0800_a285, 0x0800_02d1],
        );
        let r = Run::of(600, &e);
        assert_eq!(
            (r.frames_run, r.stop.as_str(), r.what.as_str()),
            (7, kind::REGISTER, "0x04000134 RCNT")
        );
        assert_eq!(
            r.summary(),
            "7 of 600 frames, unmodelled-register 0x04000134 RCNT"
        );
        // The place loses the checkout's path.
        assert_eq!(
            r.detail,
            "hw: I/O register 0x134 isn't modelled (crates/runtime/src/hw.rs:665)"
        );
        assert!(r.lines()[2].starts_with("in f_0800a285 < f_080002d1 ("));
        // A register with no name is its address alone.
        let e = panicked(0, "", "hw: I/O register 0x15c isn't modelled", vec![]);
        let r = Run::of(600, &e);
        assert_eq!(
            (r.stop.as_str(), r.what.as_str()),
            (kind::REGISTER, "0x0400015c")
        );
        assert_eq!(r.detail, "hw: I/O register 0x15c isn't modelled");
    }

    #[test]
    fn the_other_stops() {
        let of = |m: &str| {
            let r = Run::of(600, &panicked(0, "src/main.rs:384", m, vec![]));
            (r.stop, r.what)
        };
        // The prelude's `unmapped` and `call_fn`.
        assert_eq!(
            of("a halfword store at 0x10000000: no memory the program has there"),
            (kind::MEMORY.into(), "a halfword store at 0x10000000".into())
        );
        assert_eq!(
            of("a call or jump to 0x08000254, which is no function's entry"),
            (kind::FUNCTION.into(), "0x08000254".into())
        );
        assert_eq!(
            of("the program's entry returned"),
            (kind::PANIC.into(), String::new())
        );
        // The time limit's line.
        let s = Stop {
            frames: 12,
            asked: 600,
            functions: vec![],
            at: String::new(),
            why: Why::TimeLimit(120),
        };
        let r = Run::of(
            600,
            &Ended {
                code: Some(124),
                signal: None,
                stderr: s.line(),
            },
        );
        assert_eq!(r.summary(), "12 of 600 frames, time-limit 120 s");
        // No stop line: every frame, a signal, an error.
        let end = |code, signal, stderr: &str| {
            Run::of(
                600,
                &Ended {
                    code,
                    signal,
                    stderr: stderr.into(),
                },
            )
        };
        let r = end(Some(0), None, "");
        assert!(r.ran());
        assert_eq!(r.summary(), "600 of 600 frames, no stop");
        assert_eq!(r.lines().len(), 1);
        let r = end(None, Some(9), "");
        assert_eq!(r.summary(), "0 of 600 frames, killed signal 9");
        let r = end(Some(1), None, "p: unknown argument \"--x\"\n\n");
        assert_eq!(r.summary(), "0 of 600 frames, failed exit 1");
        assert!(
            r.detail.ends_with("p: unknown argument \"--x\""),
            "{}",
            r.detail
        );
        let report = "p: 2 functions\n  hard hole in f_08000101 at 0x08000120: an mrs\n  \
                      hard hole in f_08000201 at 0x08000210: an msr\nlines 9\n";
        let r = Run::not_built(600, 3, report);
        assert_eq!(r.summary(), "didn't build: 3 hard holes");
        assert!(
            r.detail
                .starts_with("the first: hard hole in f_08000101 at 0x08000120: an mrs; each is"),
            "{}",
            r.detail
        );
        assert_eq!(
            Run::not_built(600, 1, "").summary(),
            "didn't build: 1 hard hole"
        );
        assert_eq!(
            Run::not_built(600, 0, report).summary(),
            "didn't build: the build failed"
        );
    }

    #[test]
    fn the_run_is_kept_in_the_lifts_metrics() {
        let metrics = "# Written by gbaa-lift.\nlines = 100\nhard_holes = 0\n";
        let e = panicked(
            2,
            "crates/runtime/src/hw.rs:665",
            "hw: I/O register 0x134 isn't modelled",
            vec![0x0300_0001],
        );
        let r = Run::of(600, &e);
        let t = r.write_metrics(metrics).unwrap();
        assert!(t.starts_with(metrics), "{t}");
        assert_eq!(Run::read(&t).unwrap(), Some(r.clone()));
        assert_eq!(Run::read(metrics).unwrap(), None);
        // The lift's own metrics read as before, the table skipped.
        let m = crate::native::metrics::Metrics::from_toml(&t).unwrap();
        assert_eq!(m.0, vec![("lines".into(), 100), ("hard_holes".into(), 0)]);
        // A second run takes the first's place.
        let again = Run::not_built(600, 2, "");
        let t2 = again.write_metrics(&t).unwrap();
        assert_eq!(Run::read(&t2).unwrap(), Some(again));
        assert_eq!(t2.matches("[run]").count(), 1);
        assert_eq!(t2.matches("Written by gbaa-first-stop").count(), 1);
    }

    #[test]
    fn the_run_ends_the_report() {
        let report = "soft holes: none\nlines 100\nhard_holes 0\n";
        let r = Run::of(
            600,
            &Ended {
                code: Some(0),
                ..Ended::default()
            },
        );
        let t = r.write_report(report);
        assert_eq!(
            t,
            "soft holes: none\nlines 100\nhard_holes 0\n\
             first stop (the built program run headless; gbaa-first-stop):\n\
             \x20 600 of 600 frames, no stop\n\
             frames_run 600\nframes_asked 600\nstop none\n"
        );
        // Again: one part, the new run's.
        let t2 = Run::not_built(600, 4, "").write_report(&t);
        assert!(t2.starts_with(report), "{t2}");
        assert_eq!(t2.matches("first stop").count(), 1);
        assert!(
            t2.ends_with("frames_run 0\nframes_asked 600\nstop not-built\n"),
            "{t2}"
        );
    }
}
