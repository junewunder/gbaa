//! The timing-lab build's stall hook (`runtime::timing_lab`;
//! docs/research/why-timing-is-runtime.md, `just timing-lab`).
//! `GBAA_TIMING_LAB` picks what each memory instruction's data accesses
//! cost:
//!
//! - `log`: what they really cost, logging each site's costs and their
//!   inputs to `GBAA_TIMING_LAB_LOG` (a TSV, one row per distinct case).
//! - `no-prefetch`: as if the game had left the prefetch buffer off.
//! - `zero`: nothing (no `rt::stall`, and the accesses free).
//! - `baked`: each site's most common cost in the log, whatever happens.
//! - `table`: the log's most common cost for the site, the buffer's lead
//!   and `WAITCNT`: a static table with everything but the address.
//!
//! Sites and cases the log never saw keep their real cost; with
//! `GBAA_TIMING_LAB_STATS` set, those two modes write there how many
//! stalls ran and how many the log had no entry for.

use runtime::timing_lab::{self, Stall};
use std::collections::HashMap;
use std::sync::Mutex;

/// What a logged case is: the site and every input of its cost.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Case {
    pc: u32,
    load: bool,
    data: i32,
    cart: bool,
    waitcnt: u16,
    /// The buffer's lead in bytes, 16 for none (behind, or emptied).
    ahead: u32,
    /// The memory instruction before it (its `pc`), the path in.
    prev: u32,
    cost: i32,
}

#[derive(Default)]
struct Lab {
    mode: Mode,
    log: HashMap<Case, u64>,
    prev: u32,
    baked: HashMap<u32, i32>,
    table: HashMap<(u32, u32, u16), i32>,
    /// Stalls run, and those `baked` or `table` had no entry for.
    runs: u64,
    unseen: u64,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Mode {
    #[default]
    Log,
    NoPrefetch,
    Zero,
    Baked,
    Table,
}

static LAB: Mutex<Option<Lab>> = Mutex::new(None);

fn lead(ahead: u32) -> u32 {
    if ahead < 16 { ahead } else { 16 }
}

fn hook(s: &Stall) -> i32 {
    let mut g = LAB.lock().unwrap();
    let Some(lab) = g.as_mut() else {
        return s.cost;
    };
    match lab.mode {
        Mode::Log => {
            let c = Case {
                pc: s.pc,
                load: s.load,
                data: s.data,
                cart: s.cart,
                waitcnt: s.waitcnt,
                ahead: lead(s.ahead),
                prev: lab.prev,
                cost: s.cost,
            };
            lab.prev = s.pc;
            *lab.log.entry(c).or_default() += 1;
            s.cost
        }
        Mode::NoPrefetch => s.data,
        Mode::Zero => 0,
        Mode::Baked | Mode::Table => {
            let c = if lab.mode == Mode::Baked {
                lab.baked.get(&s.pc)
            } else {
                lab.table.get(&(s.pc, lead(s.ahead), s.waitcnt))
            };
            lab.runs += 1;
            lab.unseen += c.is_none() as u64;
            *c.unwrap_or(&s.cost)
        }
    }
}

/// The log's rows: each case and how often it ran.
fn read_log(path: &str) -> Result<Vec<(Case, u64)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut rows = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16);
        let bad = || format!("{path}: bad row {line:?}");
        if f.len() != 9 {
            return Err(bad());
        }
        let c = Case {
            pc: hex(f[0]).map_err(|_| bad())?,
            load: f[1] == "1",
            data: f[2].parse().map_err(|_| bad())?,
            cart: f[3] == "1",
            waitcnt: hex(f[4]).map_err(|_| bad())? as u16,
            ahead: f[5].parse().map_err(|_| bad())?,
            prev: hex(f[6]).map_err(|_| bad())?,
            cost: f[7].parse().map_err(|_| bad())?,
        };
        rows.push((c, f[8].parse().map_err(|_| bad())?));
    }
    Ok(rows)
}

/// The most common cost of each key in `rows`.
fn most_common<K: std::hash::Hash + Eq + Copy>(
    rows: &[(Case, u64)],
    key: impl Fn(&Case) -> K,
) -> HashMap<K, i32> {
    let mut n: HashMap<(K, i32), u64> = HashMap::new();
    for (c, k) in rows {
        *n.entry((key(c), c.cost)).or_default() += k;
    }
    let mut best: HashMap<K, (u64, i32)> = HashMap::new();
    for ((k, cost), count) in n {
        let b = best.entry(k).or_insert((0, cost));
        // Ties go to the lower cost, so the table doesn't hang on order.
        if (count, -cost) > (b.0, -b.1) {
            *b = (count, cost);
        }
    }
    best.into_iter().map(|(k, (_, c))| (k, c)).collect()
}

/// Set the hook up from `GBAA_TIMING_LAB`, if it's set.
pub fn install() -> Result<(), String> {
    let Ok(mode) = std::env::var("GBAA_TIMING_LAB") else {
        return Ok(());
    };
    let log = std::env::var("GBAA_TIMING_LAB_LOG").ok();
    let mode = match mode.as_str() {
        "log" => Mode::Log,
        "no-prefetch" => Mode::NoPrefetch,
        "zero" => Mode::Zero,
        "baked" => Mode::Baked,
        "table" => Mode::Table,
        _ => {
            return Err(format!(
                "GBAA_TIMING_LAB={mode}: not log, no-prefetch, zero, baked or table"
            ));
        }
    };
    let mut lab = Lab {
        mode,
        ..Lab::default()
    };
    if matches!(mode, Mode::Log | Mode::Baked | Mode::Table) {
        let Some(path) = &log else {
            return Err("GBAA_TIMING_LAB_LOG: not set".into());
        };
        if mode != Mode::Log {
            let rows = read_log(path)?;
            lab.baked = most_common(&rows, |c| c.pc);
            lab.table = most_common(&rows, |c| (c.pc, c.ahead, c.waitcnt));
        }
    }
    *LAB.lock().unwrap() = Some(lab);
    timing_lab::set(hook);
    Ok(())
}

/// After the run: write the log, in `log` mode.
pub fn finish() -> Result<(), String> {
    let Some(lab) = LAB.lock().unwrap().take() else {
        return Ok(());
    };
    if lab.mode != Mode::Log {
        if let Ok(path) = std::env::var("GBAA_TIMING_LAB_STATS") {
            let s = format!("runs {}\nunseen {}\n", lab.runs, lab.unseen);
            std::fs::write(&path, s).map_err(|e| format!("{path}: {e}"))?;
        }
        return Ok(());
    }
    let path = std::env::var("GBAA_TIMING_LAB_LOG").map_err(|_| "GBAA_TIMING_LAB_LOG: not set")?;
    let mut rows: Vec<_> = lab.log.into_iter().collect();
    rows.sort();
    let mut s = String::from("pc\tload\tdata\tcart\twaitcnt\tahead\tprev\tcost\tcount\n");
    for (c, n) in rows {
        s += &format!(
            "{:08x}\t{}\t{}\t{}\t{:04x}\t{}\t{:08x}\t{}\t{n}\n",
            c.pc, c.load as u8, c.data, c.cart as u8, c.waitcnt, c.ahead, c.prev, c.cost
        );
    }
    std::fs::write(&path, s).map_err(|e| format!("{path}: {e}"))
}
