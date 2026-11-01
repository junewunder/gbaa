//! Emerald's ten functions as native Rust, checked against the play
//! build (unit native/emerald-ten). The functions and their state are
//! Emerald translated, so they live in `mods/`, which is gitignored (its
//! own private repo); this file only points there.

#[path = "../../../../mods/emerald/native-ten/check.rs"]
mod check;

fn main() -> std::process::ExitCode {
    check::main()
}
