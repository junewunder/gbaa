//! Code discovery and analysis over decoded GBA ROMs: control flow, the IR,
//! and the passes that run on it.

pub mod cap;
pub mod discovery;
pub mod flash;
pub mod holes;
pub mod idle;
pub mod ir;
pub mod jumptable;
pub mod numbers_doc;
pub mod program;
pub mod vsa;
