//! Value-set analysis: `Sem<V = AbsVal>` (decision 3) and the per-function
//! fixpoint that runs it. Jump-table recovery (decision 5c,
//! [`crate::jumptable`]) and the far-jump `bl` rule (5a) are built on it.
//!
//! - [`domain`]: the abstract values and flags;
//! - [`exec`]: the `Sem` implementation, one instruction at a time;
//! - [`func`]: the fixpoint over one function.
//!
//! Nothing here knows any compiler's idioms. Design and evidence:
//! `docs/research/jump-tables.md`.
//!
//! Known gaps, as in the research prototype: the analysis is
//! non-relational beyond `x = y + k`, ignores stores outside the stack, and
//! caps sets at [`domain::MAX_SET`] values.

pub mod domain;
pub mod exec;
pub mod func;

#[cfg(test)]
mod tests;
