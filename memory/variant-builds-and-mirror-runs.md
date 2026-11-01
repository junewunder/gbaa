---
title: Timing a hand-patched generated build, and testing a value
hook: patch call sites, not call!; a copied crate needs its own --target-dir; use a positive control
---

- A play build (structured, instant) doesn't use the `call!` macro: each
  call is written out (`let sp0 = …; let Flow::Jump(a) = f(…); pc =
  settle(…)`). Patching the macros in `parts/*/src/lib.rs` changes
  nothing; patch the call sites in `parts/*/src/m*.rs` (a regex over
  those lines worked on all 17,452 of Kirby's).
- A copy of `out/<game>` built into the shared `target/recompiled` is
  taken as already built (same package names): the binary comes out
  byte-identical. Build the copy with its own `--target-dir` (the
  build-recompiled recipe's flock and systemd-run cap, by hand), and
  `cmp` the binaries before timing them.
- `perf record` fails on quack (`perf_event_paranoid`); time variant
  builds against each other instead, headless, interleaved, 4 runs each
  (Kirby 5,400 frames is about 10 s, noise about ±0.4 s).
- Asking "does the game use value X as a number" with two machines in
  lockstep (`crates/interp/examples/lr_numbers.rs`): swap X for a
  scrambled token outside the memory map, translate it back only where
  the game jumps to it, and keep both machines at the same pc. Moving X
  to the ROM's other mirror instead hid additions, masks and loads (a
  review caught it; the first result was wrong). Give the tool a positive
  control (`--self-test`) before trusting a clean run.
- agbcc writes a halfword field of a stack struct by word-wide read, mask,
  write, so uninitialised stack (old return addresses) passes through
  registers and flags in Emerald, Rogue and WarioWare without changing
  anything; compare registers and flags softly and stop only on pc,
  clock, mode, memory, I/O and the frame.
