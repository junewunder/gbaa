---
title: Where a generated crate's build time goes
hook: LLVM, not rustc's front end: p07 checks in 4.7 s of a 60 s build; how to measure one part (2026-10-24)
---

- Emerald part p07 (19,880 guest instructions, 153k lines), after the lean
  printer, rustc 1.99.1, the generated crate's release profile (opt 3, 16
  codegen units): `cargo build` 60 s, 740 MB peak; `cargo check` 4.7 s,
  466 MB. So statements the printer drops save front-end memory, but build
  time only falls if LLVM gets less or simpler IR.
- Measure one part without rebuilding the rest: touch
  `out/<game>/parts/pNN/src/lib.rs`, then under the build lock
  `/usr/bin/time -f "%e s %M KB" cargo build -q --release --manifest-path
  out/<game>/Cargo.toml --target-dir target/recompiled -p <part's name>`
  (the name is in the part's `Cargo.toml`). For the front end alone, the
  same with `cargo check` and a scratch `--target-dir`.
- The desktop's "recompiled:" line now counts resumes (dispatcher calls
  past a function's entry) and yields; Emerald's new-game route: 33 calls,
  9.5 resumes, 2 yields a frame.
