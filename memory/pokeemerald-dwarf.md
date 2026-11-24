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

## Adding -g to another agbcc decomp (mzm, 2026-11-20)

- `-g` alone moved mzm's ROM (sha1 off): agbcc ends each `.s` with
  `.section .text`, `.Letext0:` and the debug sections, so the Makefile's
  appended `.align 2, 0 @ dont insert nops` pads a debug section, and the
  section the C file really ended in (`.rodata` in 469 of 654 files) loses
  it; `.text` then gets nop padding. Appending `.text` (pokeemerald's fix)
  isn't enough for mzm: re-enter the section open before agbcc's
  `.section .text` (`scripts/agbcc-dwarf`).
- mzm pipes preproc into cpp, so units are named `<stdin>`: sed the line
  markers to the C file's name before agbcc.

## katam needs nothing (2026-11-20)

- katam's Makefile passes `-g` already (`DINFO ?= 1`) and appends `.text`
  before its align, so `katam.elf` has DWARF and the ROM matches: katam.toml
  has 14,474 located locals in 4,901 functions and 99,340 lines covering
  all 5,024 C functions. Check the Makefile before writing a wrapper.
