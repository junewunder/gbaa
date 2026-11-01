//! Code discovery and analysis over decoded GBA ROMs: control flow, the IR,
//! and the passes that run on it.

pub mod discovery;
pub mod holes;
pub mod idle;
pub mod ir;
pub mod jumptable;
pub mod program;
pub mod vsa;
