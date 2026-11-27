---
title: Tracing why a dense build leaves a function's key
hook: the report's "not proven … at ADDR", then GBAA_FNREF_WATCH=WORD; a constant's site may be arithmetic on junk, not a Const (2026-11-27)
---

What found c-fnptr-gcc's and Metroid's keys (native/fnref-dense-keys-left, quack):

- `gbaa-lift ROM OUT --name N --fn-ids dense` on a C program takes 0.1 s; Metroid's 1.5 min
  (run it as a `just long` job). OUT/report.txt lists every function word and code constant
  (`  word 0x… = f_…: not proven: WHY at ADDR`, `  constant in block B inst I = f_…: …`). Group
  them: `grep "^  constant" report.txt | grep "not proven" | sed 's/.*= \(f_[0-9a-f]*\): not
  proven: \([a-z ]*\).*/\1 \2/' | sort | uniq -c | sort -rn`.
- `GBAA_FNREF_WATCH=0xWORD` prints each step of that word's token (read, stored to which cell,
  passed, HOLE) and a `road:` line naming the read that brought it to the hole. Cells: `0x102…`
  is an object's "somewhere in it" cell (`obj_cell`), 0x0ffffff0 the stacks'.
- A code constant's site (`block B inst I`) is often not an `Op::Const`: `classify` also runs on
  a sum's result, so arithmetic on a junk value that lands on a key makes a "constant". To see
  the IR and operands, a temporary `eprintln!` under an env var in `classify` or the `Op::Bin`
  arm, built into a separate `CARGO_TARGET_DIR` while lift-check runs (it uses
  target/release/gbaa-lift throughout).
- Metroid's private bars (mods/metroid/lift-bars.toml) and Emerald's were already over on main
  for casts, locals_noted, unread_lets (and Emerald's for_counter_arith) on 2026-11-27: compare
  with main's lifter before blaming a change.
- Making a hole's number unknown (`unk`) instead of a ROM number floods: a store through an
  unknown address sends its function tokens to the pile. `Tok::RomDull` keeps it in ROM.
