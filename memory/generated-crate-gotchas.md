---
title: Generated-crate builds that hang or outlive their job
hook: a name error can look like a 30-minute build; `just long stop` leaves rustc holding the lock; SLP crash on p37
---

- A resolve error (an undefined name) in a big generated crate (sst-gen's
  36 MB) doesn't fail fast: rustc's main thread spends tens of minutes in
  `rustc_resolve::diagnostics::UsePlacementFinder` looking for `use`
  suggestions. If a generated build runs far past its usual time with one
  busy thread, `eu-stack -p <rustc pid>` shows it; `cargo check` on the
  crate (about 20 s for sst-gen) prints the error once the cause is fixed,
  so check a new printer's output with `cargo check` before a full build.
- `just build-recompiled` (and so `run-recompiled`, `unit`, `sst-gen`)
  runs cargo in its own `systemd-run --scope`, outside a `just long`
  job's service: `just long stop` leaves that cargo and its rustc running,
  holding `/tmp/gbaa-build.lock`. Kill them by path:
  `pkill -A -f 'manifest-path out/<name>/Cargo.toml'`, then any rustc
  whose `/proc/<pid>/cwd` is under the checkout.
- With registers in locals (`gbaa-recompile --locals`), rustc 1.99.1's
  LLVM 23.1 crashes with SIGSEGV in `SLPVectorizerPass::
  vectorizeStoreChains` on Emerald's part p37 (rustc then suggests ever
  bigger `RUST_MIN_STACK`; 256 MB doesn't help). Building that part at
  `opt-level = "s"` (a `[profile.release.package.<part>]` table in the
  generated Cargo.toml; no SLP pass) gets past it. Also: that form's
  whole build at 6 jobs needs more than 8 GB.
