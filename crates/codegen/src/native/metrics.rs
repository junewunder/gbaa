//! The lift's metrics and their bars (`native/lift-bars`;
//! `docs/native-stage-b.md`, "How every piece is checked").
//!
//! Every stage-B unit is checked by `just lift-check` staying green with a
//! metric moving: the lifter counts what it printed ([`Metrics`], `name
//! value` lines in `report.txt`), and `gbaa-lift --bars FILE` fails the
//! lift when any metric is over its bar ([`check`]). The bars are maxima,
//! one table per program in `crates/codegen/lift-check.toml`, and only
//! lowered. One exception: `lines_prelude` counts the prelude
//! (`native/prelude.rs`, hand-written runtime code in every lifted crate),
//! which may rise when a unit adds to it, its evidence saying how many
//! lines and why; `lines` counts everything else the lift writes but the
//! modules' wiring (`super::modules`), and only goes down.
//!
//! Memory accesses are split by how their address is known and by region,
//! so that a bar on one family means something:
//! - `const`: the address is a constant within its block (a literal, a
//!   pool word, arithmetic on them), by the region it is in, a byte
//!   access; one in RAM that is printed as a typed global's field
//!   (`super::globals`) is `ram_global` instead, which has no bar (it
//!   should rise), and `ram_globals` counts the globals;
//! - `sp`: sp-relative (the value-set analysis says it is the function's
//!   entry sp plus a constant);
//! - `computed`: anything else, by region where the value-set analysis
//!   bounds it to one, else `computed_unknown`; one in RAM the value sets
//!   bound to one typed global (`super::globals::Obj`) is `ram_object`
//!   instead (no bar: it should rise), and `pointer_unexplained` counts
//!   the computed RAM accesses that aren't (native/ref-in-object: they
//!   reach several globals, or one at a width not its element's). One
//!   the value sets don't bound but the heap's run types
//!   (`super::heap`, native/ref-heap) is a field through a `Ref<T>`,
//!   counted in `ref_accesses` (no bar: it should rise) with `ref_types`
//!   the types; `ref_unexplained` counts those through an object's
//!   pointer the types don't explain (left as bytes, in
//!   `computed_unknown` too).
//!
//! The sp-relative ones are split again by whether the stack slot they
//! reach escapes (`super::frame`): `sp_accesses_local_slot` and
//! `sp_accesses_escaping_slot` (one the frame's run didn't place counts as
//! escaping).
//!
//! Function words (`super::fnref`): the words of ROM outside code that hold
//! a function's key, `fn_words_not_proven` those the flow didn't prove
//! (barred), `fn_words_proven` the rest (no bar: it should rise).

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The regions accesses are counted by, in report order.
pub const REGIONS: [&str; 6] = ["ram", "io", "rom", "display", "sram", "other"];

/// The region of address `a`, an index into [`REGIONS`]: EWRAM and IWRAM;
/// I/O; the cartridge ROM (every wait-state mirror); palette, VRAM and
/// OAM; cartridge SRAM; the rest (BIOS, unmapped).
pub fn region(a: u32) -> usize {
    match a >> 24 {
        0x02 | 0x03 => 0,
        0x04 => 1,
        0x08..=0x0D => 2,
        0x05..=0x07 => 3,
        0x0E | 0x0F => 4,
        _ => 5,
    }
}

/// The one region every address in `lo..=hi` is in, if there is one.
pub fn region_of(lo: u32, hi: u32) -> Option<usize> {
    let r = region(lo);
    ((lo >> 24)..=(hi >> 24))
        .all(|t| region(t << 24) == r)
        .then_some(r)
}

/// Where an access's address is known to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Where {
    /// A constant in this region.
    Const(usize),
    /// A constant in RAM, printed as a typed global's field
    /// (`super::globals`): not a byte access.
    Global,
    /// Computed, into one typed global (`super::globals::Obj`): not a
    /// byte access.
    Object,
    /// Computed, a field of a heap object's type through its `Ref<T>`
    /// (`super::heap`): not a byte access.
    Ref,
    /// sp-relative.
    Sp,
    /// Computed, bounded to this region.
    Computed(usize),
    /// Computed, anywhere.
    Unknown,
}

/// The lift's metrics, in report order. Names are TOML bare keys, so a
/// bar is one `name = value` line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metrics(pub Vec<(String, u64)>);

impl Metrics {
    pub fn push(&mut self, name: impl Into<String>, v: u64) {
        self.0.push((name.into(), v));
    }

    pub fn get(&self, name: &str) -> Option<u64> {
        self.0.iter().find(|(n, _)| n == name).map(|&(_, v)| v)
    }

    /// The accesses, every family and region, zeros too, so the names
    /// don't depend on the program.
    pub fn push_accesses(&mut self, counts: &BTreeMap<(bool, Where), u64>) {
        for (store, dir) in [(false, "load"), (true, "store")] {
            let n = |w| counts.get(&(store, w)).copied().unwrap_or(0);
            for (r, name) in REGIONS.iter().enumerate() {
                self.push(format!("{dir}_const_{name}"), n(Where::Const(r)));
            }
            self.push(format!("{dir}_ram_global"), n(Where::Global));
            self.push(format!("{dir}_ram_object"), n(Where::Object));
            self.push(format!("{dir}_sp"), n(Where::Sp));
            for (r, name) in REGIONS.iter().enumerate() {
                self.push(format!("{dir}_computed_{name}"), n(Where::Computed(r)));
            }
            self.push(format!("{dir}_computed_unknown"), n(Where::Unknown));
        }
    }

    /// The `name value` lines.
    pub fn lines(&self) -> String {
        let mut s = String::new();
        for (n, v) in &self.0 {
            writeln!(s, "{n} {v}").unwrap();
        }
        s
    }

    /// The metrics as TOML, `name = value` in report order: what
    /// `gbaa-lift` writes beside `report.txt` as `metrics.toml`.
    pub fn to_toml(&self) -> String {
        let mut s = String::from("# Written by gbaa-lift: the lift's metrics, as in report.txt.\n");
        for (n, v) in &self.0 {
            writeln!(s, "{n} = {v}").unwrap();
        }
        s
    }

    /// Read [`Metrics::to_toml`]'s text back, in its order. A table isn't
    /// a metric: `[run]` is the built program's run (`super::first_stop`).
    pub fn from_toml(text: &str) -> Result<Metrics, String> {
        let d: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
        d.iter()
            .filter(|(_, v)| !v.is_table())
            .map(|(k, v)| {
                v.as_integer()
                    .and_then(|i| u64::try_from(i).ok())
                    .map(|i| (k.to_string(), i))
                    .ok_or(format!("{k}: not a whole number"))
            })
            .collect::<Result<_, _>>()
            .map(Metrics)
    }
}

/// Lower bars in a bars file's `text`, keeping its comments and order:
/// each (program, metric, value) sets `[program.<program>.bars]`'s
/// `metric` to `value`. Only lowers: a value at or over its bar, or a bar
/// that isn't there, is an error (raising a bar is a hand edit with a
/// reason). The new text and a line per bar changed.
pub fn lower_bars(
    text: &str,
    lower: &[(String, String, u64)],
) -> Result<(String, Vec<String>), String> {
    let mut d: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let mut changed = Vec::new();
    for (p, m, x) in lower {
        let at = format!("program.{p}.bars.{m}");
        let v = d
            .get_mut("program")
            .and_then(|t| t.get_mut(p.as_str()))
            .and_then(|t| t.get_mut("bars"))
            .and_then(|t| t.get_mut(m.as_str()))
            .and_then(|v| v.as_value_mut())
            .ok_or(format!("{at}: no such bar"))?;
        let bar = v
            .as_integer()
            .and_then(|i| u64::try_from(i).ok())
            .ok_or(format!("{at}: not a whole number"))?;
        if *x >= bar {
            return Err(format!(
                "{at}: {x} isn't under its bar of {bar}; bars are only lowered"
            ));
        }
        let decor = v.decor().clone();
        *v = toml_edit::Value::from(*x as i64);
        *v.decor_mut() = decor;
        changed.push(format!("{p}: {m} {bar} -> {x}"));
    }
    Ok((d.to_string(), changed))
}

/// The metrics with no bar: they are meant to rise.
pub const UNBARRED: &[&str] = &[
    "functions_recognised",
    "ram_globals",
    "load_ram_global",
    "store_ram_global",
    "fn_words_proven",
    "call_fn_complete",
    "load_ram_object",
    "store_ram_object",
    "ref_accesses",
    "ref_types",
];

/// The bars of `program` in a bars file: `[program.<name>.bars]`, each
/// `metric = maximum`.
pub fn bars(text: &str, program: &str) -> Result<BTreeMap<String, u64>, String> {
    let t: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
    let b = t
        .get("program")
        .and_then(|p| p.get(program))
        .ok_or(format!("no [program.{program}] table"))?
        .get("bars")
        .and_then(|b| b.as_table())
        .ok_or(format!("no [program.{program}.bars] table"))?;
    b.iter()
        .map(|(k, v)| {
            v.as_integer()
                .and_then(|i| u64::try_from(i).ok())
                .map(|i| (k.clone(), i))
                .ok_or(format!("bar {k}: not a whole number"))
        })
        .collect()
}

/// What [`check`] found: every failure, and the bars that could come down.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Metrics over their bars, bars naming no metric, metrics with no bar.
    pub failures: Vec<String>,
    /// Metrics under their bars: (name, value, bar).
    pub lowerable: Vec<(String, u64, u64)>,
}

/// Check `m` against `bars`: each metric at most its bar. A bar naming no
/// metric fails (a typo would hold nothing), and so does a metric with no
/// bar outside [`UNBARRED`] (a new metric starts with its bar).
pub fn check(m: &Metrics, bars: &BTreeMap<String, u64>) -> Verdict {
    let mut v = Verdict::default();
    for (n, x) in &m.0 {
        match bars.get(n) {
            Some(&b) if *x > b => v.failures.push(format!("{n} is {x}, over its bar of {b}")),
            Some(&b) if *x < b => v.lowerable.push((n.clone(), *x, b)),
            Some(_) => {}
            None if UNBARRED.contains(&n.as_str()) => {}
            None => v
                .failures
                .push(format!("{n} is {x} and has no bar (add `{n} = {x}`)")),
        }
    }
    for n in bars.keys() {
        if m.get(n).is_none() {
            v.failures
                .push(format!("the bar {n} names no metric the lift measures"));
        }
        if UNBARRED.contains(&n.as_str()) {
            v.failures
                .push(format!("{n} is meant to rise, so it has no bar"));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_by_address() {
        assert_eq!(REGIONS[region(0x0300_7f00)], "ram");
        assert_eq!(REGIONS[region(0x0400_0130)], "io");
        assert_eq!(REGIONS[region(0x0900_0000)], "rom");
        assert_eq!(REGIONS[region(0x0600_0000)], "display");
        assert_eq!(REGIONS[region(0x0E00_0000)], "sram");
        assert_eq!(REGIONS[region(0x0000_0100)], "other");
        assert_eq!(region_of(0x0200_0000, 0x0300_7fff), Some(0));
        assert_eq!(region_of(0x0300_0000, 0x0400_0000), None);
        // The rest isn't one span: 0 to 0x10 crosses everything.
        assert_eq!(region_of(0x0000_0000, 0x1000_0000), None);
    }

    #[test]
    fn a_bar_under_its_metric_fails_naming_both() {
        let mut m = Metrics::default();
        m.push("lines", 100);
        m.push("load_sp", 7);
        m.push("functions_recognised", 3);
        let text = "[program.p.bars]\nlines = 100\nload_sp = 6\n";
        let v = check(&m, &bars(text, "p").unwrap());
        assert_eq!(v.failures, vec!["load_sp is 7, over its bar of 6"]);
        let v = check(
            &m,
            &bars("[program.p.bars]\nlines = 120\nload_sp = 7\n", "p").unwrap(),
        );
        assert!(v.failures.is_empty());
        assert_eq!(v.lowerable, vec![("lines".to_string(), 100, 120)]);
    }

    #[test]
    fn metrics_toml_reads_back_in_order() {
        let mut m = Metrics::default();
        m.push("lines", 100);
        m.push("blocks", 7);
        m.push("functions", 3);
        assert_eq!(Metrics::from_toml(&m.to_toml()).unwrap(), m);
    }

    #[test]
    fn lowering_bars_keeps_the_file() {
        let text = "# The bars.\n\n[program.p]\nrom = \"p.gba\"\n\n[program.p.bars]\n\
                    lines = 120 # a note\nload_sp = 7\n\n# q's.\n[program.q.bars]\nlines = 50\n";
        let low = |p: &str, m: &str, x| (p.to_string(), m.to_string(), x);
        let (t, changed) =
            lower_bars(text, &[low("p", "lines", 100), low("q", "lines", 40)]).unwrap();
        assert_eq!(
            t,
            text.replace("lines = 120 #", "lines = 100 #")
                .replace("lines = 50", "lines = 40")
        );
        assert_eq!(changed, vec!["p: lines 120 -> 100", "q: lines 50 -> 40"]);
        assert_eq!(bars(&t, "p").unwrap()["lines"], 100);
        // Never raised, never equal, never a bar that isn't there.
        assert!(lower_bars(text, &[low("p", "load_sp", 8)]).is_err());
        assert!(lower_bars(text, &[low("p", "load_sp", 7)]).is_err());
        assert!(lower_bars(text, &[low("p", "blocks", 1)]).is_err());
        assert!(lower_bars(text, &[low("r", "lines", 1)]).is_err());
    }

    #[test]
    fn missing_and_unknown_bars_fail() {
        let mut m = Metrics::default();
        m.push("lines", 1);
        let v = check(&m, &bars("[program.p.bars]\nlnies = 1\n", "p").unwrap());
        assert_eq!(v.failures.len(), 2, "{:?}", v.failures);
        assert!(bars("[program.q.bars]\n", "p").is_err());
        assert!(bars("[program.p.bars]\nlines = -1\n", "p").is_err());
    }
}
