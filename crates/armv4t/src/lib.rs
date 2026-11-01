//! ARMv4T (ARM7TDMI) decoding, instruction semantics and an interpreter.
//!
//! [`decode`] is the adapter over `unarm` (decision 4); [`sem`] is the `Sem`
//! trait and the instruction semantics (decision 3); [`interp`] runs them
//! over a CPU and a bus; [`cycles`] estimates what each instruction
//! costs. `no_std`, so the runtime can use them all.

#![cfg_attr(not(test), no_std)]

pub mod cycles;
pub mod decode;
pub mod idle;
pub mod interp;
pub mod sem;
pub mod text;
