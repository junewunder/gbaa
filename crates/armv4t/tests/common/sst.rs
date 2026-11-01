//! The SingleStepTests ARM7TDMI harness (`just fetch-tests`), shared by
//! `armv4t/tests/sst.rs` (the interpreter, `just sst`) and
//! `analysis/tests/sst_ir.rs` (the IR builder and evaluator, `just sst-ir`),
//! which include it with `#[path]`.
//!
//! Each test sets up the CPU, runs one instruction, and compares registers,
//! banks, PSRs and the data bus transactions in order. Files are streamed
//! one at a time. Without the data a suite prints why and passes, so
//! `just check` works on a machine without `$GBAA_DATA`.
//!
//! Not compared: instruction fetches (kind 0; a jump's refill shows up in
//! r15 instead), each transaction's `cycle` and `access`, the state's
//! `pipeline` and `access`, and the address bits below the access width
//! (the interpreter's bus gets aligned addresses by contract). Cycle
//! counting isn't the interpreter's job (decision 11).
//!
//! Both suites print the same per-file table and hold every file to the
//! same pass counts.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use armv4t::decode::{Instr, decode_thumb};
use armv4t::interp::{Bank, Bus, Cpu, T_BIT};
use armv4t::sem::Width;

/// Files that must pass every case: data processing, branches and single
/// loads and stores, in both states (issue #4).
pub const MUST_PASS: &[&str] = &[
    "arm_b_bl",
    "arm_bx",
    "arm_data_proc_immediate",
    "arm_data_proc_immediate_shift",
    "arm_data_proc_register_shift",
    "arm_ldr_str_immediate_offset",
    "arm_ldr_str_register_offset",
    "arm_ldrh_strh",
    "arm_ldrsb_ldrsh",
    "thumb_add_cmp_mov_hi",
    "thumb_add_sp_or_pc",
    "thumb_add_sub",
    "thumb_add_sub_sp",
    "thumb_b",
    "thumb_bcc",
    "thumb_bl_blx_prefix",
    "thumb_bl_suffix",
    "thumb_bx",
    "thumb_data_proc",
    "thumb_ldr_pc_rel",
    "thumb_ldr_str_imm_offset",
    "thumb_ldr_str_reg_offset",
    "thumb_ldr_str_sp_rel",
    "thumb_ldrb_strb_imm_offset",
    "thumb_ldrh_strh_imm_offset",
    "thumb_ldrh_strh_reg_offset",
    "thumb_ldrsb_strb_reg_offset",
    "thumb_ldrsh_ldrsb_reg_offset",
    "thumb_lsl_lsr_asr",
    "thumb_mov_cmp_add_sub",
];

/// Passing cases for every file, as of the last `just sst`. Any change,
/// up or down, fails the test, so update this when a fix lands. Files below
/// 100% have their cause here and on issue #4.
pub const EXPECTED: &[(&str, usize)] = &[
    ("arm_b_bl", 50000),
    ("arm_bx", 50000),
    ("arm_cdp", 50000),
    ("arm_data_proc_immediate", 50000),
    ("arm_data_proc_immediate_shift", 50000),
    ("arm_data_proc_register_shift", 50000),
    ("arm_ldm_stm", 50000),
    ("arm_ldr_str_immediate_offset", 50000),
    ("arm_ldr_str_register_offset", 50000),
    ("arm_ldrh_strh", 50000),
    ("arm_ldrsb_ldrsh", 50000),
    ("arm_mcr_mrc", 50000),
    // `mrs pc, ..`: NBA writes r15 without refilling the pipeline.
    ("arm_mrs", 48420),
    ("arm_msr_imm", 50000),
    ("arm_msr_reg", 50000),
    ("arm_mul_mla", 50000),
    ("arm_mull_mlal", 50000),
    ("arm_stc_ldc", 50000),
    ("arm_swi", 50000),
    ("arm_swp", 50000),
    ("thumb_add_cmp_mov_hi", 50000),
    ("thumb_add_sp_or_pc", 50000),
    ("thumb_add_sub", 50000),
    ("thumb_add_sub_sp", 50000),
    ("thumb_b", 50000),
    ("thumb_bcc", 50000),
    ("thumb_bl_blx_prefix", 50000),
    ("thumb_bl_suffix", 50000),
    // Out of 24898: the 25102 v5 `blx` cases aren't counted.
    ("thumb_bx", 24898),
    ("thumb_data_proc", 50000),
    // Empty-list Thumb `ldmia`/`pop`: NBA keeps bit 0 of the loaded pc.
    ("thumb_ldm_stm", 49950),
    ("thumb_ldr_pc_rel", 50000),
    ("thumb_ldr_str_imm_offset", 50000),
    ("thumb_ldr_str_reg_offset", 50000),
    ("thumb_ldr_str_sp_rel", 50000),
    ("thumb_ldrb_strb_imm_offset", 50000),
    ("thumb_ldrh_strh_imm_offset", 50000),
    ("thumb_ldrh_strh_reg_offset", 50000),
    ("thumb_ldrsb_strb_reg_offset", 50000),
    ("thumb_ldrsh_ldrsb_reg_offset", 50000),
    ("thumb_lsl_lsr_asr", 50000),
    ("thumb_mov_cmp_add_sub", 50000),
    ("thumb_push_pop", 49974),
    ("thumb_swi", 50000),
    // Thumb `b<cond>` with cond 1110: NBA branches; ARMv4T says undefined.
    ("thumb_undefined_bcc", 0),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tx {
    kind: u32,
    size: u32,
    addr: u32,
    data: u32,
}

#[derive(Clone, PartialEq, Eq)]
pub struct State {
    r: [u32; 16],
    fiq: [u32; 7],
    /// svc, abt, irq, und: r13 and r14.
    banked: [[u32; 2]; 4],
    cpsr: u32,
    spsr: [u32; 5],
}

pub struct Case {
    pub initial: State,
    pub fin: State,
    pub txs: Vec<Tx>,
    pub opcode: u32,
    pub base_addr: u32,
}

/// Reads one `.json.bin` file (the format `transcode_json.py` documents).
pub fn read_file(path: &Path) -> Vec<Case> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let u = |p: usize| u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
    assert_eq!(u(0), 0xD33D_BAE0, "{}: bad magic", path.display());
    let state = |p: usize| {
        let v = |i: usize| u(p + 8 + 4 * i);
        State {
            r: std::array::from_fn(v),
            fiq: std::array::from_fn(|i| v(16 + i)),
            banked: std::array::from_fn(|k| [v(23 + 2 * k), v(24 + 2 * k)]),
            cpsr: v(31),
            spsr: std::array::from_fn(|i| v(32 + i)),
        }
    };
    let mut p = 8;
    (0..u(4))
        .map(|_| {
            let full = u(p) as usize;
            let mut q = p + 4;
            let initial = state(q);
            q += u(q) as usize;
            let fin = state(q);
            q += u(q) as usize;
            let n = u(q + 8) as usize;
            let txs = (0..n)
                .map(|i| {
                    let t = q + 12 + 24 * i;
                    Tx {
                        kind: u(t),
                        size: u(t + 4),
                        addr: u(t + 8),
                        data: u(t + 12),
                    }
                })
                .collect();
            q += u(q) as usize;
            let case = Case {
                initial,
                fin,
                txs,
                opcode: u(q + 8),
                base_addr: u(q + 12),
            };
            p += full;
            case
        })
        .collect()
}

/// A bus that answers loads from the recorded transactions and checks
/// every access against them, in order. Instruction fetches are skipped.
/// It also keeps the accesses it saw, as transactions.
pub struct TxBus<'a> {
    data: Vec<&'a Tx>,
    next: usize,
    error: Option<String>,
    pub seen: Vec<Tx>,
}

impl<'a> TxBus<'a> {
    pub fn new(c: &'a Case) -> Self {
        TxBus {
            data: c.txs.iter().filter(|t| t.kind != 0).collect(),
            next: 0,
            error: None,
            seen: Vec::new(),
        }
    }

    fn access(&mut self, kind: u32, w: Width, addr: u32, v: u32) -> u32 {
        self.seen.push(Tx {
            kind,
            size: w.bytes(),
            addr,
            data: v,
        });
        let what = if kind == 1 { "load" } else { "store" };
        let Some(&t) = self.data.get(self.next) else {
            self.fail(format!("extra {what}{} at {addr:#x}", w.bytes() * 8));
            return 0;
        };
        self.next += 1;
        let mask = ((1u64 << (8 * w.bytes())) - 1) as u32;
        let ok = t.kind == kind
            && t.size == w.bytes()
            && t.addr & !(t.size - 1) == addr
            && (kind == 1 || t.data & mask == v & mask);
        if !ok {
            self.fail(format!(
                "{what}{} {addr:#x} = {:#x}, expected kind {} size {} {:#x} = {:#x}",
                w.bytes() * 8,
                v & mask,
                t.kind,
                t.size,
                t.addr,
                t.data
            ));
        }
        t.data
    }

    fn fail(&mut self, e: String) {
        self.error.get_or_insert(e);
    }
}

impl Bus for TxBus<'_> {
    fn load(&mut self, w: Width, addr: u32) -> u32 {
        self.access(1, w, addr, 0)
    }
    fn store(&mut self, w: Width, addr: u32, v: u32) {
        self.access(2, w, addr, v);
    }
}

pub fn pc_offset(cpsr: u32) -> u32 {
    if cpsr & T_BIT != 0 { 4 } else { 8 }
}

/// SingleStepTests records every bank in full: `R` is the user-mode view
/// and `R_fiq`, `R_svc`, ... hold each mode's own registers, the current
/// mode's included.
pub fn to_cpu(s: &State) -> Cpu {
    let mut cpu = Cpu {
        pc: s.r[15].wrapping_sub(pc_offset(s.cpsr)),
        cpsr: s.cpsr,
        ..Cpu::default()
    };
    cpu.banks[Bank::User as usize] = s.r[8..15].try_into().unwrap();
    cpu.banks[Bank::Fiq as usize] = s.fiq;
    for (k, rr) in s.banked.iter().enumerate() {
        let b = &mut cpu.banks[Bank::Svc as usize + k];
        b[..5].copy_from_slice(&s.r[8..13]);
        b[5..].copy_from_slice(rr);
    }
    cpu.spsr[1..].copy_from_slice(&s.spsr);
    cpu.r[..8].copy_from_slice(&s.r[..8]);
    let live = cpu.banks[cpu.bank() as usize];
    cpu.r[8..].copy_from_slice(&live);
    cpu
}

/// The inverse of [`to_cpu`]: each bank as its own mode sees it.
pub fn to_state_view(cpu: &Cpu, r15: u32) -> State {
    let bank = |b: Bank| -> [u32; 7] {
        if cpu.bank() == b {
            cpu.r[8..].try_into().unwrap()
        } else if b == Bank::Fiq || cpu.bank() == Bank::Fiq {
            cpu.banks[b as usize]
        } else {
            // r8 to r12 are shared outside FIQ mode.
            let mut v = cpu.banks[b as usize];
            v[..5].copy_from_slice(&cpu.r[8..13]);
            v
        }
    };
    let mut r = [0; 16];
    r[..8].copy_from_slice(&cpu.r[..8]);
    r[8..15].copy_from_slice(&bank(Bank::User));
    r[15] = r15;
    State {
        r,
        fiq: bank(Bank::Fiq),
        banked: std::array::from_fn(|k| {
            let b = bank([Bank::Svc, Bank::Abt, Bank::Irq, Bank::Und][k]);
            [b[5], b[6]]
        }),
        cpsr: cpu.cpsr,
        spsr: cpu.spsr[1..].try_into().unwrap(),
    }
}

/// The first difference between the CPU and the expected state, as a short
/// field name (for grouping) and a description.
pub fn compare(cpu: &Cpu, r15: u32, want: &State) -> Option<(String, String)> {
    compare_states(&to_state_view(cpu, r15), want)
}

/// The first difference between two states, as in [`compare`].
pub fn compare_states(got: &State, want: &State) -> Option<(String, String)> {
    compare_fields(got, want)
        .into_iter()
        .find(|(_, g, w)| g != w)
        .map(|(f, g, w)| (f.clone(), format!("{f} = {g:#010x}, expected {w:#010x}")))
}

/// Every compared field as (name, got, want).
fn compare_fields(got: &State, want: &State) -> Vec<(String, u32, u32)> {
    let mut fields: Vec<(String, u32, u32)> = Vec::new();
    for i in 0..16 {
        fields.push((format!("r{i}"), got.r[i], want.r[i]));
    }
    fields.push(("cpsr".into(), got.cpsr, want.cpsr));
    for i in 0..7 {
        fields.push((format!("r{}_fiq", i + 8), got.fiq[i], want.fiq[i]));
    }
    for (k, name) in ["svc", "abt", "irq", "und"].iter().enumerate() {
        for j in 0..2 {
            fields.push((
                format!("r{}_{name}", 13 + j),
                got.banked[k][j],
                want.banked[k][j],
            ));
        }
    }
    for (k, name) in ["fiq", "svc", "abt", "irq", "und"].iter().enumerate() {
        fields.push((format!("spsr_{name}"), got.spsr[k], want.spsr[k]));
    }
    fields
}

/// A field's initial value and the CPSR, for failure messages.
fn describe(s: &State, field: &str) -> String {
    let cpu = to_cpu(s);
    let view = to_state_view(&cpu, s.r[15]);
    let v = compare_fields(&view, s)
        .into_iter()
        .find(|(f, ..)| f == field)
        .map_or(0, |(_, v, _)| v);
    format!("{field} = {v:#010x}, cpsr = {:#010x}", s.cpsr)
}

pub enum Outcome {
    Pass,
    /// Thumb `blx` (v5), which SingleStepTests files under `bx`; the decoder
    /// treats it as undefined, as mGBA does.
    V5Blx,
    Fail {
        field: String,
        detail: String,
    },
}

/// Prints a whole case (`SST_DUMP=<opcode in hex>`), for debugging.
fn dump(c: &Case) {
    let fields = compare_fields(&c.fin, &c.initial);
    eprintln!("opcode {:#010x} at {:#x}", c.opcode, c.base_addr);
    for (f, fin, init) in fields {
        let mark = if fin != init { " *" } else { "" };
        eprintln!("  {f:<8} {init:#010x} -> {fin:#010x}{mark}");
    }
    for t in &c.txs {
        eprintln!("  tx {t:x?}");
    }
}

/// One case's result: the outcome against the expected state, and the
/// state and bus accesses it ended with (none if it didn't run).
pub struct Run {
    pub outcome: Outcome,
    pub end: Option<(State, Vec<Tx>)>,
}

/// Run one case: set up the CPU, let `exec` run the instruction `raw` on it
/// (returning whether it ended in an exit, as `Cpu::execute` does), and
/// compare. A panic in `exec` is a failure with field `panic`.
pub fn run_case(c: &Case, exec: impl FnOnce(&mut Cpu, &mut TxBus, u32) -> bool) -> Run {
    let not_run = |outcome| Run { outcome, end: None };
    if let Ok(op) = std::env::var("SST_DUMP")
        && u32::from_str_radix(op.trim_start_matches("0x"), 16) == Ok(c.opcode)
    {
        dump(c);
    }
    let mut cpu = to_cpu(&c.initial);
    let thumb = cpu.thumb();
    let raw = if thumb { c.opcode & 0xFFFF } else { c.opcode };
    if thumb
        && matches!(decode_thumb(raw as u16, 0), Instr::Undefined { .. })
        && (0x4780..0x4800).contains(&raw)
    {
        return not_run(Outcome::V5Blx);
    }
    if cpu.pc != c.base_addr {
        return not_run(Outcome::Fail {
            field: "setup".into(),
            detail: format!("pc {:#x} != base_addr {:#x}", cpu.pc, c.base_addr),
        });
    }
    let mut bus = TxBus::new(c);
    let jumped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        exec(&mut cpu, &mut bus, raw)
    }));
    let jumped = match jumped {
        Ok(j) => j,
        Err(e) => {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            return not_run(Outcome::Fail {
                field: "panic".into(),
                detail: msg,
            });
        }
    };
    // r15 as the pipeline shows it: a jump refills it for the new state; an
    // instruction that falls through (even one that flips T with `msr`)
    // leaves the old state's prefetch in place.
    let r15 = if jumped {
        cpu.pc.wrapping_add(pc_offset(cpu.cpsr))
    } else {
        cpu.pc.wrapping_add(pc_offset(c.initial.cpsr))
    };
    let end = Some((to_state_view(&cpu, r15), bus.seen.clone()));
    let outcome = if let Some(e) = bus.error {
        Outcome::Fail {
            field: "bus".into(),
            detail: e,
        }
    } else if bus.next != bus.data.len() {
        let t = bus.data[bus.next];
        Outcome::Fail {
            field: "bus".into(),
            detail: format!("missing access: {t:x?}"),
        }
    } else {
        match compare(&cpu, r15, &c.fin) {
            None => Outcome::Pass,
            Some((field, detail)) => Outcome::Fail {
                detail: format!("{detail}; initial {}", describe(&c.initial, &field)),
                field,
            },
        }
    };
    Run { outcome, end }
}

/// What a suite's check made of one case: its outcome against the expected
/// state, and, for a differential suite, the first way it disagreed with
/// the reference run, as (field, description).
pub struct Checked {
    pub outcome: Outcome,
    pub diverged: Option<(String, String)>,
}

#[derive(Default)]
struct FileResult {
    name: String,
    pass: usize,
    total: usize,
    v5_blx: usize,
    /// Failures by first differing field, with one example each.
    causes: Vec<(String, usize, String)>,
    /// Disagreements with the reference run, the same way.
    diverged: Vec<(String, usize, String)>,
}

fn tally(list: &mut Vec<(String, usize, String)>, field: String, example: impl FnOnce() -> String) {
    match list.iter_mut().find(|(f, ..)| *f == field) {
        Some((_, n, _)) => *n += 1,
        None => list.push((field, 1, example())),
    }
}

fn run_file(path: &Path, check: &impl Fn(&Case) -> Checked) -> FileResult {
    let mut res = FileResult {
        name: file_name(path),
        ..FileResult::default()
    };
    for c in read_file(path) {
        let checked = check(&c);
        let at = |d: &str| format!("opcode {:#010x} at {:#x}: {d}", c.opcode, c.base_addr);
        if let Some((field, detail)) = checked.diverged {
            tally(&mut res.diverged, field, || at(&detail));
        }
        match checked.outcome {
            Outcome::Pass => {
                res.pass += 1;
                res.total += 1;
            }
            Outcome::V5Blx => res.v5_blx += 1,
            Outcome::Fail { field, detail } => {
                res.total += 1;
                tally(&mut res.causes, field, || at(&detail));
            }
        }
    }
    res.causes.sort_by_key(|c| std::cmp::Reverse(c.1));
    res.diverged.sort_by_key(|c| std::cmp::Reverse(c.1));
    res
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap()
        .to_string_lossy()
        .trim_end_matches(".json.bin")
        .to_string()
}

pub fn data_dir() -> PathBuf {
    let data = std::env::var("GBAA_DATA")
        .unwrap_or_else(|_| format!("{}/.local/share/gbaa", std::env::var("HOME").unwrap()));
    Path::new(&data).join("SingleStepTests-ARM7TDMI/v1")
}

/// Run every file through `check`, print the per-file table, and fail if a
/// file's pass count isn't the one in [`EXPECTED`], a [`MUST_PASS`] file
/// misses a case, or any case diverged. `diverged_label` names the
/// reference in the table's notes (`≠interp r0: 3`).
pub fn run_suite(diverged_label: &str, check: impl Fn(&Case) -> Checked) {
    let dir = data_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // Straight to stderr: the test harness captures `eprintln!` from a
        // passing test, and the skip should show in `just check`.
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "skipping SingleStepTests: {} not found (run `just fetch-tests`)",
            dir.display()
        );
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".json.bin"))
        .collect();
    files.sort();
    let only = std::env::var("SST_ONLY").ok();
    if let Some(only) = &only {
        files.retain(|p| p.to_string_lossy().contains(only));
    }
    // Panics are caught and counted per case; don't print each one.
    std::panic::set_hook(Box::new(|_| {}));

    let mut table = String::new();
    let mut problems = Vec::new();
    writeln!(
        table,
        "{:<34} {:>7} {:>7} {:>8}  notes",
        "file", "pass", "total", "%"
    )
    .unwrap();
    for path in &files {
        let r = run_file(path, &check);
        let pct = 100.0 * r.pass as f64 / r.total.max(1) as f64;
        let mut notes = Vec::new();
        if r.v5_blx > 0 {
            notes.push(format!("{} v5 blx not counted", r.v5_blx));
        }
        for (field, n, _) in &r.causes {
            notes.push(format!("{field}: {n}"));
        }
        for (field, n, _) in &r.diverged {
            notes.push(format!("{diverged_label} {field}: {n}"));
        }
        writeln!(
            table,
            "{:<34} {:>7} {:>7} {:>7.2}%  {}",
            r.name,
            r.pass,
            r.total,
            pct,
            notes.join(", ")
        )
        .unwrap();
        if std::env::var_os("SST_VERBOSE").is_some() {
            for (field, n, example) in &r.causes {
                writeln!(table, "    {field} ({n}): {example}").unwrap();
            }
        }
        for (field, n, example) in &r.diverged {
            writeln!(table, "    {diverged_label} {field} ({n}): {example}").unwrap();
            problems.push(format!("{}: {n} cases {diverged_label} ({field})", r.name));
        }
        if MUST_PASS.contains(&r.name.as_str()) && r.pass != r.total {
            problems.push(format!(
                "{} must pass every case: {}/{}",
                r.name, r.pass, r.total
            ));
        }
        match EXPECTED.iter().find(|(n, _)| *n == r.name) {
            Some(&(_, want)) if r.pass != want => problems.push(format!(
                "{} passes {}, expected {want} (update EXPECTED if this is a fix)",
                r.name, r.pass
            )),
            Some(_) => {}
            None => problems.push(format!("{}: not in EXPECTED", r.name)),
        }
    }
    let _ = std::panic::take_hook();
    if only.is_none() {
        let names: Vec<String> = files.iter().map(|p| file_name(p)).collect();
        for (name, _) in EXPECTED {
            if !names.iter().any(|n| n == name) {
                problems.push(format!("{name}.json.bin is missing from {}", dir.display()));
            }
        }
    }
    println!("{table}");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
