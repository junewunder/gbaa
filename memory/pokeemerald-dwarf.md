---
title: pokeemerald's DWARF (locals, types, lines) through gdb
hook: binutils 15.2 rejects agbcc's .debug_abbrev; Fedora gdb reads it (info scope, ptype, info line) (2026-10-26)
---

- `$GBAA_DATA/src/pokeemerald/pokeemerald.elf` carries DWARF 2 (agbcc `-g`,
  `Makefile:119`): `.debug_info` 32 MB, `.debug_line`, `.debug_pubnames`.
- `arm-none-eabi-readelf`/`objdump --dwarf` (binutils 15.2) fail with
  ".debug_abbrev section not zero terminated" and print nothing useful.
- `/usr/bin/gdb` reads it: `gdb -batch -nx -ex 'info scope CreateBoxMon'
  -ex 'ptype struct Pokemon' -ex 'info line *0x080003a5' pokeemerald.elf`.
  A gdb Python script looping `info scope` over 400 functions ran in well
  under a minute.
- Each variable has one location for its whole scope (no location lists):
  `CreateBoxMon`'s stack-passed 6th parameter shows as `$r0`. Treat
  registers as hints. More in `docs/research/name-recovery.md`.
