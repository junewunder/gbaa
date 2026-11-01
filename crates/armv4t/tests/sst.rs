//! SingleStepTests ARM7TDMI against the interpreter. `just sst` runs this in
//! release mode and prints the per-file table; the harness is in
//! `common/sst.rs`.

#[path = "common/sst.rs"]
#[allow(dead_code)]
mod sst;

#[test]
fn single_step_tests() {
    sst::run_suite("", |c| sst::Checked {
        outcome: sst::run_case(c, |cpu, bus, raw| cpu.execute(bus, raw)).outcome,
        diverged: None,
    });
}
