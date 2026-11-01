//! Rust code generation from the analysed IR (decision 2): one Rust fn per
//! guest function, written as a crate to `out/<game>/`. Design:
//! `docs/codegen-design.md`.
//!
//! - [`cfg`]: each function's CFG, dominators, loop nest and irreducible
//!   regions, for structured codegen;
//! - [`func`]: each function's blocks, built from discovery's output, and
//!   what each block exit does;
//! - [`plan`]: each function's structuring plan and its structure check;
//! - [`locals`]: guest registers and flags held in Rust locals;
//! - [`saves`]: which functions keep r4-r11 and sp, proven per callee, and
//!   each function's summary: parameters, results and callee-saves;
//! - [`lower`]: IR to Rust tokens: functions, blocks and exits;
//! - [`print`]: a block's insts as Rust expressions and statements;
//! - [`emit`]: the generated crate's files;
//! - [`fn_diff`]: the crate of the per-function differential runs;
//! - [`io_regs`]: the GBA's I/O registers by name, for the readable form;
//! - [`names`]: the source map: names by address, and their identifiers;
//! - [`dwarf`]: what a readable block prints from DWARF: names, lines, calls;
//! - [`modcopy`]: a mod copy: functions cut out as Rust, built back in;
//! - [`find`]: which functions read or write a struct field;
//! - [`snapshot`]: the smallest structured function of each shape, printed.

pub mod cfg;
pub mod dwarf;
pub mod emit;
pub mod find;
pub mod fn_diff;
pub mod func;
pub mod io_regs;
pub mod locals;
pub mod lower;
pub mod modcopy;
pub mod names;
pub mod native;
pub mod plan;
pub mod print;
pub mod saves;
pub mod seen;
pub mod snapshot;
pub mod spins;
pub mod text;
pub mod types;
pub mod variants;

#[cfg(test)]
mod tests;
