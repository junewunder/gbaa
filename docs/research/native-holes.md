# Hole report: what blocks a native build of each game

research/native-holes (unit native/hole-report), 2026-11-01. For the
Stage-A lifter and for june: of the eight games of
`docs/research/code-without-rom-addresses.md`, which is closest to a
native build (`docs/native-port-design.md`, section 6), and what stands
in the way of each.

## Summary

- **Ranked by hard holes, then bytes reached but not discovered:**
  Metroid: Zero Mission (7), WarioWare (10), Kirby & the Amazing Mirror
  (15), Kirby: Nightmare in Dream Land (18), moonbeam (23), Pokémon Emerald
  (24), The Minish Cap (41), Emerald Rogue (69), with the run's call
  targets as starts ([below](#with-the-runs-call-targets)). (Before them,
  with the flash stand-in: 7, 21, 21, 23, 25, 29, 46, 80; before both,
  2026-11-01: 18, 22, 29, 30, 23, 34, 46, 86.)
- **The commonest hard hole in five of the eight was a call through a
  register to an address discovery doesn't know as a function start** (13
  to 19 a game in WarioWare, both Kirbys and Emerald; 15 and 21 in
  Minish Cap and Rogue; 4 in Metroid; none in moonbeam): m4a's routines called through
  its tables (Emerald's `SoundMainBTM` family), and code the games copy
  into RAM with their own loops. Both are discovery gaps, not code a native
  program can't express: give discovery the roots and they go. Since
  2026-11-05 `just holes` gives discovery the run's ROM call targets: none
  of the ROM ones is left, and the m4a code they reach brings 9 to 12
  reached unresolved jumps a game (m4a's `mov r12, lr` … `bx r12`
  returns).
- **Indirect jumps Recovery leaves unresolved** (2 to 50) lead in Minish
  Cap (25) and Emerald Rogue (50), and almost none were reached in 90
  seconds of play (14 of 109 over all eight), so coverage runs that reach
  them are what tell real tables from dead code. With the coverage runs
  (2026-11-06, [below](#coverage-runs)) 88 of the 165 unresolved since
  the run's call targets are reached (68 by the press script), each with
  its targets: m4a's `bx r12` returns, Minish Cap's IWRAM veneers, and
  real switch tables in Rogue and moonbeam; the other 77 no run reached.
  moonbeam's largest class is 12 functions whose summary returns with sp
  changed.
- **Five games ran code from RAM that changes; one still does.** The
  flash save library copies its routines onto the stack (`ReadFlash_Core`
  and its siblings, at 0x03007dxx; Emerald, both Kirbys, Metroid, Rogue,
  and WarioWare once, unchanged): these are now hardware-library calls
  (`runtime::hw::flash`, recognised by `analysis::flash`; "The flash
  library's stack routines" below), not holes. Emerald Rogue's mixer
  stamping templates into its loop remains. Both were in the design's
  list.
- **The controls hold:** WarioWare's IWRAM helper (0x3000468, `sub lr, lr,
  #24`) and its twin are the only return-address arithmetic not resolved;
  moonbeam's `__gnu_thumb1_case_uqi` and `_uhi` are found statically and
  resolved (Recovery resolves each `bl` to them as a jump); Emerald's
  unresolved-jump count (11) is Recovery's.
- **Kirby: Nightmare in Dream Land switches stacks** (three sites,
  0x08000244, 0x08000278 and 0x08000298, in code at 0x08000234-0x080002a0: it saves sp and resumes on another stack, a
  coroutine); a hand-written stand-in is needed there.
- **Busy-wait loops that call functions** (Emerald's `DoMapLoadLoop`
  case, an open question to june): found only in Emerald Rogue, three, in
  `LoadMapInStepsLocal`, `FreeTempTileDataBuffersIfPossible` and
  `CB2_NewGame` (186 non-progressing iterations in a row). The 90-second
  route doesn't reach Emerald's new-game map load.

## The tool

`just holes ROM` (`crates/hole-report`, binary `gbaa-holes`; command
entry in `docs/tools/commands.md`) runs, for one ROM:

1. discovery and its IWRAM overlays (`analysis::discovery::{discover,
   overlays}`);
2. a 5,400-frame interpreter run, instant timing, no save file, with the
   research press script: `start@300-5400/120 a@400-5400/30
   b@2500-5400/45 right@2000-2999 up@3000-3999 left@4000-5400` (Start every
   2 s from frame 300, A every half second from 400, B every 0.75 s from
   2500, right, up and left held for 1,000 frames each from 2000; the
   research doc gives the script in words, this is its reading);
3. the function summaries (`codegen::saves::summarize`);
4. the classification, `analysis::holes::classify` (in the analysis crate
   so the lifter reuses it), and prints one ranking line and each list.

About a minute a game (Emerald: 17 s discovery, 57 s for the run's 947 M
instructions, 5 s summaries).

### What counts as a hard hole

| Hole | How it's found | Counted |
|---|---|---|
| Indirect jump not resolved | Recovery's `Unresolved` and `Rejected`, ROM and overlays | every one, reached or not ("jumps A+B": ROM + overlays) |
| Return-address arithmetic | the run: a return (`bx`, `pop {pc}`, `mov pc, lr`, `ldr pc, [sp]`) landing 64 bytes before to 1 KiB after the innermost open call's return site, not on it, not on another call's return site, not on a function start, not inside the callee's own function; statically: a function doing arithmetic on lr, or loading through it, before writing lr | where Recovery doesn't resolve the call site as a jump |
| RAM code that changed | the run: an address run again with different bytes or instruction set | each run of addresses (gaps up to 16 bytes) |
| CPU state | `msr`, `mrs`, `ldm`/`stm` with the S bit, `s` writes to pc, stack switches (sp moved 4 KiB or more by one instruction other than sp-relative arithmetic) | outside the IRQ wrapper (code run at the top level of an IRQ, and the rest of its function; for a copy made by the game's loop, at the RAM address) and boot (code run only in the first frame, and its function) |
| BIOS call outside the library | `swi` numbers `runtime::bios` doesn't implement (HardReset, above 0x2A) | each site |
| Call to a non-entry | the run: a call through a register (`mov lr, pc` or `adr lr, …` then `bx rN`, or a `bl` to a `bx rN` stub other than `bx pc`) to an address not a discovered function start or RAM entry | each target |
| Summary hole | `returns with r4-r11 or sp changed` (sp moved by a run-time amount, as a variable-length array does) | each function |

A place can be counted under two kinds: a routine copied onto the stack
that isn't recognised would be RAM code that changed and, at each entry,
a call to a non-entry. A ROM source and its RAM copy are classified once, at the copy;
a `bl` to a switch helper that Recovery rejects counts as a jump only.

CPU-state and BIOS items in code only the pointer scan found and the run
never ran aren't counted (Emerald's 0x087f22xx: data that decodes as
`swi 0x64` and `ldm ^`); none of the eight has any once that is applied.
Other summary holes are counted elsewhere (an unknown jump, a mode change,
RAM code) or aren't holes for a native build (an indirect call is an
`FnRef`), and are printed but not counted.

### The flash library's stack routines

Nintendo's `AgbFlash` (pret's `agb_flash.c`) copies three tiny routines
into a buffer in its caller's frame and calls them there: `ReadFlash1`
(a byte: the chip's ID and status), `ReadFlash_Core` (a sector into RAM)
and `VerifyFlashSector_Core` (RAM against a sector). The run keeps the
64 bytes at each call into RAM (`Run::ram_calls`); a target whose every
version is a routine `analysis::flash` recognises (its code's length and
hash: agbcc's three, modern GCC's `ReadFlash_Core` and
`VerifyFlashSector_Core`) is a call of `runtime::hw::flash::{read_byte,
read, verify}` on the hardware library's save chip (`Hw::save`), listed in
the report and in the ranking line ("flash library stand-ins", routines),
and the RAM it ran is neither changed code, undiscovered code nor a
non-entry. In the eight: Emerald (`ReadFlash1`, `ReadFlash_Core`), Rogue
(the same, modern GCC's), Metroid and both Kirbys (`ReadFlash_Core`,
`VerifyFlashSector_Core`), WarioWare (`ReadFlash_Core`, once). The
lifter doesn't call them yet: no stage-A build lifts a game that saves to
flash.

**Not discovered** is the bytes of code the run executed that discovery
(or an overlay's) didn't find, ROM and RAM.

**Not measured here:** pointer arithmetic or casts the types don't explain
(not measured until stage B: it needs the typed lift), and behaviour that
depends on cycle timing (the native-timing play build's to find).

**Busy loops that call functions** are counted but not added to the hard
total until june answers how a native program lets time pass in them: a
backward branch outside IRQs taken 64 or more times in a row in one frame
by iterations that each made a call, didn't halt, loaded RAM an IRQ
handler had written, and made the same stores as the iteration before (no
progress of their own). At 8 in a row the same rule also caught pure
loops over short lists (Rogue's `FlagGet` per item, 27); at 64 only the
three map-load loops remain.

## The ranking

Each line as `just holes` printed it (2026-11-05, with the flash stand-in,
before discovery took the run's call targets; `--no-run-roots` prints
these):

1. `Metroid - Zero Mission (USA).gba: hard 7 (jumps 3+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 4, summaries 0); not discovered 1946 bytes (12 ROM, 1934 RAM) of 83346 reached; busy loops calling functions 0; flash library stand-ins 2`
2. `Kirby & The Amazing Mirror.gba: hard 21 (jumps 2+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 18, summaries 1); not discovered 1584 bytes (746 ROM, 838 RAM) of 105088 reached; busy loops calling functions 0; flash library stand-ins 2`
3. `WarioWare, Inc. - Mega Microgame$!.gba: hard 21 (jumps 4+0, return arithmetic 2, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 13, summaries 2); not discovered 2232 bytes (184 ROM, 2048 RAM) of 54360 reached; busy loops calling functions 0; flash library stand-ins 1`
4. `moonbeam.gba: hard 23 (jumps 11+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 0, summaries 12); not discovered 438 bytes (438 ROM, 0 RAM) of 49918 reached; busy loops calling functions 0; flash library stand-ins 0`
5. `Kirby - Nightmare in Dream Land.gba: hard 25 (jumps 2+1, return arithmetic 0, RAM code changed 0, CPU state 3, BIOS 0, calls to non-entries 19, summaries 0); not discovered 3340 bytes (2514 ROM, 826 RAM) of 60236 reached; busy loops calling functions 0; flash library stand-ins 2`
6. `Pokemon - Emerald Version.gba: hard 29 (jumps 11+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 16, summaries 2); not discovered 2360 bytes (772 ROM, 1588 RAM) of 58454 reached; busy loops calling functions 0; flash library stand-ins 2`
7. `Legend of Zelda, The - The Minish Cap (USA).gba: hard 46 (jumps 25+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 15, summaries 6); not discovered 3294 bytes (386 ROM, 2908 RAM) of 40408 reached; busy loops calling functions 0; flash library stand-ins 0`
8. `Emerald Rogue.gba: hard 80 (jumps 50+0, return arithmetic 0, RAM code changed 1, CPU state 0, BIOS 0, calls to non-entries 21, summaries 8); not discovered 890 bytes (662 ROM, 228 RAM) of 79416 reached; busy loops calling functions 3; flash library stand-ins 2`

| Game | Hard | Jumps (reached) | Ret. arith. | RAM changed | CPU | Non-entries (RAM, ROM) | Summaries | Not discovered (ROM + RAM) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Metroid: Zero Mission | 7 | 3 (0) | 0 | 0 | 0 | 4 (4, 0) | 0 | 1,946 (12 + 1,934) |
| Kirby & the Amazing Mirror | 21 | 2 (1) | 0 | 0 | 0 | 18 (0, 18) | 1 | 1,584 (746 + 838) |
| WarioWare, Inc. | 21 | 4 (1) | 2 | 0 | 0 | 13 (2, 11) | 2 | 2,232 (184 + 2,048) |
| moonbeam | 23 | 11 (3) | 0 | 0 | 0 | 0 | 12 | 438 (438 + 0) |
| Kirby: Nightmare in Dream Land | 25 | 2+1 (2) | 0 | 0 | 3 | 19 (0, 19) | 0 | 3,340 (2,514 + 826) |
| Pokémon Emerald | 29 | 11 (0) | 0 | 0 | 0 | 16 (0, 16) | 2 | 2,360 (772 + 1,588) |
| The Minish Cap | 46 | 25 (5) | 0 | 0 | 0 | 15 (1, 14) | 6 | 3,294 (386 + 2,908) |
| Emerald Rogue | 80 | 50 (2) | 0 | 1 | 0 | 21 (0, 21) | 8 | 890 (662 + 228) |

## With the run's call targets

Since 2026-11-05 (unit native/disc-run-call-targets) `gbaa-holes` discovers
the ROM a second time after the run, with every ROM target of the run's
calls through a register as a function start
(`analysis::discovery::discover_with`), and classifies against that
(`--no-run-roots` gives the table above). Two things made that work:

- **All the targets, not only the missing ones.** A start the pointer scan
  found the first time (Kirby's `ply_goto`) is lost when a new root reaches
  it first by a tail `b`: code at an instruction start the scan only takes
  when a pool loads it. Every target is code the run called, so all are
  confirmed starts. Only calls through a register: a far-jump `bl`'s
  target is inside a function, and taking every call target cut those
  (moonbeam's undiscovered bytes went from 438 to 4,932).
- **A copy source ends at a run root.** m4a copies `SoundMainRAM` to
  IWRAM 2 KiB at a time (`CpuCopy32` of the buffer's size), so the copy
  reads on into `SoundMainBTM`, `ply_fine` and their neighbours, which run
  in ROM. Copy sources are data to discovery; with run roots, a source
  ends at the first one inside it. Plain `discover` (no extra roots) is
  unchanged, and `disc-pret`, `disc-agb` and `jt-score` keep their bars.

What the run's targets were: m4a's command table
(`gMPlayJumpTableTemplate` and its copies in RAM: Emerald, both Kirbys,
Minish Cap, Rogue), leaf routines without a `push {lr}` that the scan's
score passes over, and WarioWare's pointers computed at run time (three
targets no ROM word points at).

Lines of 2026-11-05, with the flash stand-in and the run's call targets:

1. `Metroid - Zero Mission (USA).gba: hard 7 (jumps 3+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 4, summaries 0); not discovered 1946 bytes (12 ROM, 1934 RAM) of 83346 reached; busy loops calling functions 0; flash library stand-ins 2`
2. `WarioWare, Inc. - Mega Microgame$!.gba: hard 10 (jumps 4+0, return arithmetic 2, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 2, summaries 2); not discovered 2062 bytes (14 ROM, 2048 RAM) of 54360 reached; busy loops calling functions 0; flash library stand-ins 1`
3. `Kirby & The Amazing Mirror.gba: hard 15 (jumps 14+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 0, summaries 1); not discovered 1200 bytes (362 ROM, 838 RAM) of 105088 reached; busy loops calling functions 0; flash library stand-ins 2`
4. `Kirby - Nightmare in Dream Land.gba: hard 18 (jumps 14+1, return arithmetic 0, RAM code changed 0, CPU state 3, BIOS 0, calls to non-entries 0, summaries 0); not discovered 2574 bytes (1748 ROM, 826 RAM) of 60236 reached; busy loops calling functions 0; flash library stand-ins 2`
5. `moonbeam.gba: hard 23 (jumps 11+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 0, summaries 12); not discovered 438 bytes (438 ROM, 0 RAM) of 49918 reached; busy loops calling functions 0; flash library stand-ins 0`
6. `Pokemon - Emerald Version.gba: hard 24 (jumps 22+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 0, summaries 2); not discovered 1638 bytes (50 ROM, 1588 RAM) of 58454 reached; busy loops calling functions 0; flash library stand-ins 2`
7. `Legend of Zelda, The - The Minish Cap (USA).gba: hard 41 (jumps 34+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 1, summaries 6); not discovered 2960 bytes (52 ROM, 2908 RAM) of 40408 reached; busy loops calling functions 0; flash library stand-ins 0`
8. `Emerald Rogue.gba: hard 69 (jumps 60+0, return arithmetic 0, RAM code changed 1, CPU state 0, BIOS 0, calls to non-entries 0, summaries 8); not discovered 270 bytes (42 ROM, 228 RAM) of 79416 reached; busy loops calling functions 3; flash library stand-ins 2`

| Game | Hard | Jumps (reached) | Ret. arith. | RAM changed | CPU | Non-entries (RAM, ROM) | Summaries | Not discovered (ROM + RAM) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Metroid: Zero Mission | 7 | 3 (0) | 0 | 0 | 0 | 4 (4, 0) | 0 | 1,946 (12 + 1,934) |
| WarioWare, Inc. | 10 | 4 (1) | 2 | 0 | 0 | 2 (2, 0) | 2 | 2,062 (14 + 2,048) |
| Kirby & the Amazing Mirror | 15 | 14 (13) | 0 | 0 | 0 | 0 | 1 | 1,200 (362 + 838) |
| Kirby: Nightmare in Dream Land | 18 | 14+1 (13) | 0 | 0 | 3 | 0 | 0 | 2,574 (1,748 + 826) |
| moonbeam | 23 | 11 (3) | 0 | 0 | 0 | 0 | 12 | 438 (438 + 0) |
| Pokémon Emerald | 24 | 22 (11) | 0 | 0 | 0 | 0 | 2 | 1,638 (50 + 1,588) |
| The Minish Cap | 41 | 34 (14) | 0 | 0 | 0 | 1 (1, 0) | 6 | 2,960 (52 + 2,908) |
| Emerald Rogue | 69 | 60 (12) | 0 | 1 | 0 | 0 | 8 | 270 (42 + 228) |

Every new unresolved jump is an m4a command routine returning through
the lr it saved in r12 (`mov r12, lr`, a `bl`, `bx r12`): reached, and a
return, which Recovery doesn't see yet (filed as its own unit). The
non-entries left are all in RAM, the games' own IWRAM copies: the RAM
copies unit's work. The lifter doesn't take run roots yet:
`discover_with` is what it would call.

## Coverage runs

Since 2026-11-06 (unit native/hole-coverage) `gbaa-holes ROM --cover
holes/coverage/<game>.toml` plays coverage runs after the press script,
into the same record (`crates/hole-report/src/cover.rs`; command entry in
`docs/tools/commands.md`). Every run is seeded and repeatable: a random
walk (a direction or none held 1 to 48 frames, A, B, Start, Select, L, R
now and then, A tapped every eighth frame) after a short script. The runs,
per game:

- **walk** (all eight): from reset, Start and A through the title for
  1,500 frames, then the random walk; four seeds, 9,000 frames each.
- **saves** and **save-backups** (moonbeam, Kirby & the Amazing Mirror,
  Rogue; Emerald's one is **save**, with no backups run): the owner's saves
  in `$GBAA_DATA/saves/<game>/` and their backups, continued, then the
  walk; 6,000 or 9,000 frames each.
- **story** and **tas** (Emerald): every other story-point save (`just
  emerald-story`), continued as the fuzzer does, 3,000 frames; every
  other state along the living-dex TAS (`just emerald-tas`), 2,000 frames.

`gbaa-fuzz` itself isn't fed in: it is Emerald's and Kirby & the Amazing
Mirror's only, and its runs aren't kept as states the hole tool could
replay; its story seeds are (the **story** and **tas** runs).

The run now records, at every indirect jump executed (`bx`, a load or
data-processing write to pc, `ldm` with pc), the runs that executed it and
each target (up to 256). Each file's `[[jump]]` list, below its marker, is
every jump Recovery leaves unresolved, with the runs that reached it
(none: unreached by every run) and its targets, written with `--update`;
without it the tool checks the list against the runs and fails on any
difference (Metroid's and Minish Cap's reruns matched). A game takes 4 to
25 minutes (Emerald the longest: 107,400 frames, 19 G instructions).

Unresolved jumps reached by any run (by the press script alone), with the
ranking lines' totals as classified against all runs:

| Game | Unresolved jumps | Reached by any run | By the press script | Frames played |
|---|---:|---:|---:|---:|
| Metroid: Zero Mission | 3 | 0 | 0 | 41,400 |
| WarioWare, Inc. | 4 | 1 | 1 | 41,400 |
| Kirby & the Amazing Mirror | 14 | 13 | 13 | 77,400 |
| Kirby: Nightmare in Dream Land | 15 | 14 | 14 | 41,400 |
| moonbeam | 11 | 5 | 3 | 65,400 |
| Pokémon Emerald | 22 | 11 | 11 | 107,400 |
| The Minish Cap | 34 | 26 | 14 | 41,400 |
| Emerald Rogue | 62 | 18 | 12 | 77,400 |
| All eight | 165 | 88 | 68 | |

(Rogue has 62, not 60: the runs' extra call targets gave discovery more
code. Its busy loops that call functions rose from 3 to 15, Emerald's from
0 to 5: map loads the press script never reached.)

What the reached ones are:

- **m4a's returns through r12** (`mov r12, lr` … `bx r12`: `ply_rept`,
  `clear_modM`, `MPlayJumpTableCopy`): 11 to 14 a game in Emerald, both
  Kirbys, Minish Cap and Rogue, each with one target, the return site in
  `MPlayMain` or `SoundInit`. The Recovery unit for a `bx` through a copy of
  lr (filed) takes these.
- **Minish Cap's veneers into its IWRAM code**: 15 two-instruction stubs
  at 0x0800029e-0x080002f2 (`GetTileType…`, `GetActTile…`,
  `GetCollisionData…`) and `CheckBits`, `CalcCollisionStaticEntity`, each
  jumping to one IWRAM address (0x03005780-0x03005ff0): the code the
  game copies to IWRAM itself. The walk reached 12 of them the press script
  didn't.
- **Switch tables the run resolved**: Rogue's 0x0819aa60 (13 targets),
  0x0819d4be (8), 0x0819de7e (9) and 0x081c3064 (2), reached only from the
  owner's save; moonbeam's 0x08002bc4 (6), 0x08002be2 (2), 0x08006aee (8),
  0x08027558 (7) and 0x08020616 (1). These are real tables Recovery
  misses.
- **Jumps into RAM code**: WarioWare's 0x080efe76 (four IWRAM targets),
  Rogue's `SoundMain` into IWRAM (0x03005599), Kirby:
  Nightmare's IWRAM 0x030010f8 back to ROM.

Unreached by everything include, where named, the GameCube multiboot
handlers (Emerald, Rogue), `__make_dp` (libc's soft-float, Emerald and
WarioWare), `IntrMain`'s and `__start`'s tails, and Rogue's 44 others,
which no save or walk reached; a coverage-guided run, or saves further
into Rogue, is what would settle them.

### Each game's unresolved jumps

Function names from the public decomps' symbol maps
(`$GBAA_DATA/names/`: pret's for Emerald, Minish Cap, Metroid and Kirby &
the Amazing Mirror; xmatch's library names for the others).

#### Metroid: Zero Mission

0 of 3 reached (0 by the press script).

Unreached by every run: 0x08000208 (IntrMain+0x104), 0x087608fc, 0x087d8004.

#### WarioWare, Inc.

1 of 4 reached (1 by the press script).

| Jump | In | Reached by | Targets |
|---|---|---|---|
| 0x080efe76 |  | press, walk | 4: 0x03006900, 0x03006a74, 0x03006b04, 0x03006c28 |

Unreached by every run: 0x080001c0, 0x080f56a4 (__make_dp+0x24), 0x083db2e6.

#### Kirby & the Amazing Mirror

13 of 14 reached (13 by the press script).

| Jump | In | Reached by | Targets |
|---|---|---|---|
| 0x0814f88c | MPlayJumpTableCopy+0x14 | press, walk, saves, save-backups | 1: 0x0815053b (SoundInit+0xa2) |
| 0x0814f940 | ply_rept+0x38 | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814f956 | ply_rept+0x4e | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814f968 | ply_rept+0x60 | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814f998 | ply_rept+0x90 | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814f9ac | ply_rept+0xa4 | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814f9c2 | ply_rept+0xba | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814f9d6 | ply_rept+0xce | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814f9e8 | ply_rept+0xe0 | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814f9f4 | ply_rept+0xec | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814fa0e | ply_rept+0x106 | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814ffdc | clear_modM+0x38 | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |
| 0x0814fff0 | clear_modM+0x4c | press, walk, saves, save-backups | 1: 0x0814fb8b (MPlayMain+0x102) |

Unreached by every run: 0x080001bc (__start+0x1bc).

#### Kirby: Nightmare in Dream Land

14 of 15 reached (14 by the press script).

| Jump | In | Reached by | Targets |
|---|---|---|---|
| 0x030010f8 |  | press, walk | 1: 0x080010cd |
| 0x080cdd50 | MPlayJumpTableCopy+0x14 | press, walk | 1: 0x080ce9ff (SoundInit+0xa2) |
| 0x080cde04 | ply_rept+0x38 | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cde1a | ply_rept+0x4e | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cde2c | ply_rept+0x60 | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cde5c | ply_rept+0x90 | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cde70 | ply_rept+0xa4 | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cde86 | ply_rept+0xba | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cde9a | ply_rept+0xce | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cdeac | ply_rept+0xe0 | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cdeb8 | ply_rept+0xec | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080cded2 | ply_rept+0x106 | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080ce4a0 | clear_modM+0x38 | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |
| 0x080ce4b4 | clear_modM+0x4c | press, walk | 1: 0x080ce04f (MPlayMain+0x102) |

Unreached by every run: 0x0800474e.

#### moonbeam

5 of 11 reached (3 by the press script).

| Jump | In | Reached by | Targets |
|---|---|---|---|
| 0x08002bc4 |  | press, walk, saves, save-backups | 6: 0x0800265b, 0x080026c3, 0x080038d5, 0x08003a13, 0x08005019, 0x08005ab9 |
| 0x08002be2 |  | saves, save-backups | 2: 0x08004fdd, 0x080059f7 |
| 0x08006aee |  | saves, save-backups | 8: 0x0800122d, 0x0800125f, 0x08001325, 0x08004b4f, 0x08004d91, 0x080056a1, 0x08005d75, 0x08006155 |
| 0x08020616 |  | press, walk, saves, save-backups | 1: 0x080206cb |
| 0x08027558 |  | press, walk, saves, save-backups | 7: 0x0802755b, 0x080277a9, 0x080277b1, 0x080277f1, 0x08027819, 0x0802782d, 0x08027865 |

Unreached by every run: 0x08002d9c, 0x0801ba92, 0x0802dbc0, 0x0803842c, 0x0803846e, 0x08040080.

#### Pokémon Emerald

11 of 22 reached (11 by the press script).

| Jump | In | Reached by | Targets |
|---|---|---|---|
| 0x082df860 |  | press, walk, save, story | 1: 0x082e057f (SoundInit+0xa2) |
| 0x082df92a |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082df93c |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082df96c |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082df980 |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082df996 |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082df9aa |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082df9bc |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082df9f6 |  | press, walk, save, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082dffb4 |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |
| 0x082dffc8 |  | press, walk, save, story, tas | 1: 0x082dfb5f (MPlayMain+0x102) |

Unreached by every run: 0x08000370 (IntrMain+0x128), 0x082db334, 0x082db596, 0x082db848, 0x082dee74 (GameCubeMultiBoot_Main+0xf0), 0x082dee94 (GameCubeMultiBoot_ExecuteProgram+0x10), 0x082deef6 (GameCubeMultiBoot_HandleSerialInterrupt+0x14), 0x082e89f4 (__make_dp+0x24), 0x0850dbce, 0x08613d08, 0x087f22e8.

#### The Minish Cap

26 of 34 reached (14 by the press script).

| Jump | In | Reached by | Targets |
|---|---|---|---|
| 0x0800029e | GetTileTypeRelativeToEntity+0x2 | walk | 1: 0x03005780 |
| 0x080002a2 | GetTileTypeAtEntity+0x2 | press, walk | 1: 0x0300579c |
| 0x080002a6 | GetTileTypeAtWorldCoords+0x2 | press, walk | 1: 0x030057a8 |
| 0x080002aa | GetTileTypeAtRoomCoords+0x2 | walk | 1: 0x030057bc |
| 0x080002b2 | GetTileTypeAtTilePos+0x2 | press, walk | 1: 0x030057d4 |
| 0x080002b6 | GetActTileRelativeToEntity+0x2 | walk | 1: 0x03005800 |
| 0x080002ba | GetActTileAtEntity+0x2 | walk | 1: 0x0300581c |
| 0x080002ca | GetActTileAtTilePos+0x2 | walk | 1: 0x03005854 |
| 0x080002ce | GetCollisionDataRelativeTo+0x2 | walk | 1: 0x03005864 |
| 0x080002d2 | GetCollisionDataAtEntity+0x2 | walk | 1: 0x03005880 |
| 0x080002e2 | GetCollisionDataAtTilePos+0x2 | walk | 1: 0x030058b8 |
| 0x080002e6 | GetActTileForTileType+0x2 | walk | 1: 0x030058c8 |
| 0x080002ee | sub_080B1B84+0x2 | walk | 1: 0x030058f8 |
| 0x080002f2 | sub_080B1BA4+0x2 | walk | 1: 0x03005918 |
| 0x08000f12 | CheckBits+0x2 | walk | 1: 0x03005e60 |
| 0x08004486 | CalcCollisionStaticEntity+0x2 | press, walk | 1: 0x03005ff0 |
| 0x080af758 | MPlayJumpTableCopy+0x14 | press, walk | 1: 0x080b03f1 (SoundInit+0x80) |
| 0x080af822 | ply_rept+0x4e | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |
| 0x080af834 | ply_rept+0x60 | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |
| 0x080af864 | ply_rept+0x90 | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |
| 0x080af878 | ply_rept+0xa4 | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |
| 0x080af88e | ply_rept+0xba | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |
| 0x080af8c0 | ply_rept+0xec | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |
| 0x080af8ee | ply_rept+0x11a | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |
| 0x080afe84 | clear_modM+0x38 | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |
| 0x080afe98 | clear_modM+0x4c | press, walk | 1: 0x080afa15 (MPlayMain+0x10c) |

Unreached by every run: 0x080002be (GetActTileAtWorldCoords+0x2), 0x080002c2 (GetActTileAtRoomCoords+0x2), 0x080002d6 (GetCollisionDataAtWorldCoords+0x2), 0x0800129a (sub_08001290+0xa), 0x080012c4 (EnemyFunctionHandler+0x26), 0x080042aa (UpdateAnimationVariableFrames+0x34), 0x080045c2 (sub_080045B4+0xe), 0x080045d8 (CalculateDirectionTo+0x4).

#### Emerald Rogue

18 of 62 reached (12 by the press script).

| Jump | In | Reached by | Targets |
|---|---|---|---|
| 0x0800b7f0 | SoundMain+0x68 | press, walk, saves, save-backups | 1: 0x03005599 |
| 0x0800c69c | MPlayJumpTableCopy+0x14 | press, walk, saves, save-backups | 1: 0x0800a099 (SoundInit+0x94) |
| 0x0800c750 | ply_rept+0x38 | save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c766 | ply_rept+0x4e | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c778 | ply_rept+0x60 | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c7a8 | ply_rept+0x90 | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c7bc | ply_rept+0xa4 | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c7d2 | ply_rept+0xba | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c7e6 | ply_rept+0xce | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c7f8 | ply_rept+0xe0 | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c804 | ply_rept+0xec | save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800c832 | ply_rept+0x11a | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800cdf0 | clear_modM+0x38 | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0800ce04 | clear_modM+0x4c | press, walk, saves, save-backups | 1: 0x0800c99b (MPlayMain+0x102) |
| 0x0819aa60 |  | saves | 13: 0x0819aa63, 0x0819aab7, 0x0819aad9, 0x0819aaeb, 0x0819ab07, 0x0819ab11, 0x0819ab29, 0x0819ab3d, … |
| 0x0819d4be |  | saves | 8: 0x0819d4c1, 0x0819d4f1, 0x0819d4fd, 0x0819d503, 0x0819d51d, 0x0819d537, 0x0819d541, 0x0819d54b |
| 0x0819de7e |  | saves | 9: 0x0819de81, 0x0819deb1, 0x0819debd, 0x0819dec3, 0x0819decd, 0x0819ded7, 0x0819dee1, 0x0819deeb, … |
| 0x081c3064 |  | saves | 2: 0x081c306f, 0x081c30b9 |

Unreached by every run: 0x08000384, 0x0800b5a0 (GameCubeMultiBoot_Main+0xf0), 0x0800b622 (GameCubeMultiBoot_HandleSerialInterrupt+0x14), 0x0800f130, 0x0800f134, 0x0800f138, 0x08022210, 0x080222f0, 0x080287a8, 0x0802d2c2, 0x0802f970, 0x080375c6, 0x0803d1ec, 0x08041c68, 0x0806ae5e, 0x0806c416, 0x0807e002, 0x0808306e, 0x080a4132, 0x080ba13c, 0x080bb454, 0x080bceca, 0x0811ab26, 0x0811dc06, 0x0811dc3a, 0x08120270, 0x0813a1d4, 0x0814c1d0, 0x0814c474, 0x081786cc, 0x0818079c, 0x08184db0, 0x0819e5e4, 0x0819e910, 0x081b2f68, 0x081b305c, 0x081e2676, 0x0822bcf4, 0x08264aaa, 0x0826a946, 0x0826a9c2, 0x0826b184, 0x0826bb32, 0x0826c5fa.

## Each game's worst holes

As of "The ranking", before discovery took the run's call targets: the
ROM call targets named here are starts now.

- **Metroid: Zero Mission.** Its 4 calls to non-entries are into the code
  Metroid copies to IWRAM itself (0x03003b90, copy of ROM 0x08004464;
  0x03004100, of 0x080046d0); the other 9 were the flash library's stack
  routines (0x03007d09-0x03007d91), stood in for now. None of its three
  unresolved jumps was reached. Closest to native: discover its own
  IWRAM copies, and only three jumps remain to explain.
- **WarioWare.** The return arithmetic: 0x3000468 (returns to the site
  - 24) and 0x3000474 (site - 20), the IWRAM loop of
  `code-without-rom-addresses.md`, copied by the game from ROM 0x080007e4.
  One hand-written stand-in covers both. Its other RAM code (2,048 bytes,
  each a copy of ROM bytes) needs only discovery.
- **Kirby & the Amazing Mirror.** 18 ROM call targets not discovered as
  starts (0x08149865-0x0814ffe1, most of them called from one site,
  0x0814fb86).
- **Kirby: Nightmare in Dream Land.** Its coroutine switch (code at
  0x08000234-0x080002a0: `str sp, [r3]; mov sp, r2` and back), which a native
  program can only express as a hand-written stand-in (threads or a state
  machine). Also 2,514 ROM bytes run but not discovered, the most of the
  eight.
- **Pokémon Emerald.** 16 m4a routines called through tables
  (`SoundMainBTM` 0x082df7e4 and its neighbours, from 0x082dfb5a), and `SoundMainRAM` (0x082df0e0) and `IntrMain`
  copied to IWRAM without a BIOS copy discovery recognises. Eleven
  unresolved jumps, none reached.
- **moonbeam.** No RAM code at all, no call to a non-entry, and the fewest
  bytes undiscovered; its holes are 11 unresolved jumps (3 reached) and 12
  functions whose summary returns with sp changed (a run-time sp
  adjustment). Its interworking veneers (`bx pc` then ARM) are calls the
  tool follows, not holes.
- **The Minish Cap.** 25 unresolved jumps, 5 of them reached (0x080002a2,
  0x080002a6, 0x080002b2, 0x08004486, 0x080af758), and
  2,908 bytes of IWRAM code it copies itself (0x030040e0, copy of ROM
  0x080af438, the largest).
- **Emerald Rogue.** 50 unresolved jumps (2 reached: in `SoundMain` and
  `MPlayJumpTableCopy`), its mixer's template stamping (0x030059f0-
  0x03005b10, `codegen::variants`), 8 functions
  with a run-time sp, and the only busy loops calling functions.

## What the Stage-A lifter should know

- Take holes from `analysis::holes::classify`; its `Report` lists each
  site. The IRQ wrapper and boot `msr`/`mrs` come back as `CpuWhere::Wrapper`
  and `Boot`: recognised, not holes.
- Most of each game's hard total is roots discovery lacks, not code that
  can't be expressed: the run's call targets (`Run::call_targets`,
  `indirect_calls`) and the RAM copies' ROM sources (`RamRun::rom_src`)
  are the roots to add.
- The flash library's stack routines (six of the eight call them, five
  at changing addresses) are `Report::flash`: at each such call into RAM,
  call the `runtime::hw::flash` function its `Routine::call` names on
  `Hw::save`, with the routine's arguments (r0-r2) as its slices.
- Unreached unresolved jumps dominate Minish Cap's and Rogue's counts;
  until coverage reaches them, they are as likely dead as live.

## Proposed units

Filed from this report (see the tracker for their state):

- Discovery takes the run's call targets as function starts (the largest
  hard class in every game; done: "With the run's call targets", above).
- Discovery finds code games copy to RAM themselves (loops, and `CpuSet`
  with arguments it can't fold): the RAM bytes not discovered.
- Coverage runs past the press script, to reach the unresolved jumps and
  settle each as a table or dead code (done: "Coverage runs", above; 77
  still unreached).
- A hardware-library stand-in for the flash library's stack routines
  (done: "The flash library's stack routines", above).
- Recovery takes a `bx` through a copy of lr as a return (filed after
  the run's call targets, 2026-11-05: m4a's `bx r12`).

## Limits

- One route of 90 seconds per game, from reset, no save file (the
  coverage runs, above, add random walks, saves and Emerald's TAS states). Code the
  route doesn't reach shows only through static finds (Recovery's jumps,
  static return arithmetic, CPU-state instructions).
- The return-arithmetic rule is a heuristic over the run; its filters
  removed every false positive seen (a long-call veneer, a switch inside
  the callee, a RAM copy of a switch jumping to its ROM original), and
  kept both WarioWare helpers.
- The run's calls are what lr_numbers counts plus jumps through a
  register right after lr is set nearby (`mov lr, pc; bx r0`, pret's
  `adr lr, …; bx r0` in `IntrMain`). lr_numbers itself misses those.
- The busy-loop rule is a heuristic too (above); it doesn't see a wait
  that makes stores each time round (a counter), or one the route doesn't
  reach.
