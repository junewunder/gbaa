---
title: Rogue's r11 takers: how the [[takes]] chain was traced
hook: naming the dispatchers makes every callback above them a taker, up to AgbMain; 29 + 89 + 13 so far, 14 left (2026-11-27)
open: true
---

GenerateFloorLayout's by-value struct carries its caller's r11
(native/rogue-build). Exact means a `[[takes]]` for every function a ROM
word points at on the r11 chain, with the pointer call sites that reach it.
What 2026-11-27 (quack lane 1) found:

- The chain is not 29 functions. Naming the dispatchers' sites (RunTasks,
  RunScriptCommand's command call, ScrCmd_special/specialvar, CallCallbacks
  inlined twice in AgbMainLoop, the field-effect table loops, the party
  menu's field-move call through `bx r11`) makes the dispatchers take r11,
  so every main callback, task and script command that calls one directly
  takes it too: round two listed 114 more, up to AgbMain (crt0's
  `bx r1`). Round three left 25, then 14 needing a hand read. r11 is
  mostly the reset value passed down untouched; the field-move path sets
  r11 to the function pointer itself before calling it.
- `gbaa-holes`' summary hole prints only the first function ("zeroed
  through 0x… and N more"). To get the whole list, patch
  `crates/codegen/src/saves/summary.rs`'s `more` text to print `through`
  (an env-gated `format!("{:x?}")`; don't commit) and run `just holes ROM
  --holes mods/emerald-rogue/holes.toml` (about 4 min, 4 GB under `just
  long`). `gbaa-lift --holes-only` alone took over 30 min (names loading):
  use `just holes`.
- A wrong site is a hard hole of its own ("the holes file's takes matches
  no indirect call"), so the holes run checks the addresses. A site is the
  second halfword of the Thumb `bl` to a `bx rN` stub. Dead copies
  (the outlined CallCallbacks, a dive loop printed as a resolved set) match
  nothing: leave them out.
- Many ROM words of a callback are compares (`ldr r3, =X; ldr r2, [gMain,
  #4]; cmp r2, r3`), not calls; a word in compressed data (TryDoDiveWarp's,
  inside an item icon) is no pointer (`sites = []`).
- Scratch tools (word finder, literal tracer that sorts words into
  SetMainCallback2 / CreateTask / gMain field / gTasks store, a pret-build
  byte matcher for names) are in `mods/emerald-rogue/takes-work/` (private).
  The pret build in `$GBAA_DATA/src/pokeemerald-rogue` is shifted from the
  ROM; its `pokeemerald-rogue.syms` is from yet another build, so match by
  bytes (`rev.py`), not by address.
