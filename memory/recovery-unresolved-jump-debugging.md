---
title: Finding why Recovery leaves a ROM's jump unresolved
hook: jt-check --rom ROM --trace ADDR; widening and frame escapes are the usual causes
---

How the 13 modern-GCC switches of Rogue and Imperium were found
(native/gcc-switch-recovery, 2026-11-27).

- `target/release/jt-check --rom ROM --trace ADDR [--back N] [--engine]`
  (run it with `just long`: discovery is 1.5 to 3 minutes on a
  pokeemerald-expansion ROM, seconds on agb) prints the jump's result,
  whether discovery's code has the address, the fixpoint states from the
  function and its entries, `frame escapes` (instructions that lower the
  escaped offset, and joins), the function's far jumps, and every root
  near it that reaches the jump (a part can fail where the function
  resolves). `--engine` reruns with the engine's own far answers: the
  default trace approximates them from the far-jump `bl` sites.
- A register Top at a merge point that every predecessor has bounded is
  widening (`WIDEN_AFTER` joins): the cause of two the card blamed on r6.
- A spill slot gone at the jump: look at `escaped`. `Some(i32::MIN)` is
  arithmetic on a frame address the analysis lost (Thumb `adds rd, rn, rm`
  goes through `add_with_carry`, not `add`).
- A main baseline for `just holes-all --against`: `git worktree add` was
  refused by the session's permissions; `git archive origin/main | tar -x
  -C work/<lane>/main`, with `mods` linked in, builds and runs it.
- Analysis changes move lift bars (crates/codegen/lift-check.toml) on agb
  and C programs; `codegen --test lift_bars` in `just check` fails first.
