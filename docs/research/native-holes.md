# Hole report: what blocks a native build of each game

research/native-holes (unit native/hole-report), 2026-11-01. For the
Stage-A lifter and for june: of the eight games of
`docs/research/code-without-rom-addresses.md`, which is closest to a
native build (`docs/native-port-design.md`, section 6), and what stands
in the way of each.

## Summary

- **Ranked by hard holes, then bytes reached but not discovered:**
  Metroid: Zero Mission (18), WarioWare (22), moonbeam (23), Kirby & the
  Amazing Mirror (29), Kirby: Nightmare in Dream Land (30), Pokémon Emerald
  (34), The Minish Cap (46), Emerald Rogue (86).
- **The commonest hard hole in five of the eight was a call through a
  register to an address discovery doesn't know as a function start** (13
  to 23 a game in Metroid, WarioWare, both Kirbys and Emerald; 15 and 26 in
  Minish Cap and Rogue; none in moonbeam): m4a's routines called through
  its tables (Emerald's `SoundMainBTM` family), and code the games copy
  into RAM with their own loops. Both are discovery gaps, not code a native
  program can't express: give discovery the roots and they go. Since
  2026-11-05 `just holes` gives discovery the run's ROM call targets
  ([With the run's call targets](#with-the-runs-call-targets)): none of the
  ROM ones is left, and the m4a code they reach brings 9 to 12 reached
  unresolved jumps a game (m4a's `mov r12, lr` … `bx r12` returns).
- **Indirect jumps Recovery leaves unresolved** (2 to 50) lead in Minish
  Cap (25) and Emerald Rogue (50), and almost none were reached in 90
  seconds of play (14 of 109 over all eight), so coverage runs that reach
  them are what tell real tables from dead code. moonbeam's largest class
  is 12 functions whose summary returns with sp changed.
- **Five games run code from RAM that changes:** the flash save library
  copies its routines onto the stack (`ReadFlash_Core` and its siblings, at
  0x03007dxx; Emerald, both Kirbys, Metroid, Rogue), and Emerald Rogue's
  mixer stamps templates into its loop. Both were in the design's list.
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

A place can be counted under two kinds: the flash library's stack
routines are RAM code that changed and, at each entry, a call to a
non-entry. A ROM source and its RAM copy are classified once, at the copy;
a `bl` to a switch helper that Recovery rejects counts as a jump only.

CPU-state and BIOS items in code only the pointer scan found and the run
never ran aren't counted (Emerald's 0x087f22xx: data that decodes as
`swi 0x64` and `ldm ^`); none of the eight has any once that is applied.
Other summary holes are counted elsewhere (an unknown jump, a mode change,
RAM code) or aren't holes for a native build (an indirect call is an
`FnRef`), and are printed but not counted.

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

Each line as `just holes` printed it (2026-11-01, before discovery took
the run's call targets; `--no-run-roots` prints these):

1. `Metroid - Zero Mission (USA).gba: hard 18 (jumps 3+0, return arithmetic 0, RAM code changed 2, CPU state 0, BIOS 0, calls to non-entries 13, summaries 0); not discovered 2098 bytes (12 ROM, 2086 RAM) of 83346 reached; busy loops calling functions 0`
2. `WarioWare, Inc. - Mega Microgame$!.gba: hard 22 (jumps 4+0, return arithmetic 2, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 14, summaries 2); not discovered 2266 bytes (184 ROM, 2082 RAM) of 54360 reached; busy loops calling functions 0`
3. `moonbeam.gba: hard 23 (jumps 11+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 0, summaries 12); not discovered 438 bytes (438 ROM, 0 RAM) of 49918 reached; busy loops calling functions 0`
4. `Kirby & The Amazing Mirror.gba: hard 29 (jumps 2+0, return arithmetic 0, RAM code changed 3, CPU state 0, BIOS 0, calls to non-entries 23, summaries 1); not discovered 1740 bytes (746 ROM, 994 RAM) of 105088 reached; busy loops calling functions 0`
5. `Kirby - Nightmare in Dream Land.gba: hard 30 (jumps 2+1, return arithmetic 0, RAM code changed 1, CPU state 3, BIOS 0, calls to non-entries 23, summaries 0); not discovered 3464 bytes (2514 ROM, 950 RAM) of 60236 reached; busy loops calling functions 0`
6. `Pokemon - Emerald Version.gba: hard 34 (jumps 11+0, return arithmetic 0, RAM code changed 1, CPU state 0, BIOS 0, calls to non-entries 20, summaries 2); not discovered 2444 bytes (772 ROM, 1672 RAM) of 58454 reached; busy loops calling functions 0`
7. `Legend of Zelda, The - The Minish Cap (USA).gba: hard 46 (jumps 25+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 15, summaries 6); not discovered 3294 bytes (386 ROM, 2908 RAM) of 40408 reached; busy loops calling functions 0`
8. `Emerald Rogue.gba: hard 86 (jumps 50+0, return arithmetic 0, RAM code changed 2, CPU state 0, BIOS 0, calls to non-entries 26, summaries 8); not discovered 962 bytes (662 ROM, 300 RAM) of 79416 reached; busy loops calling functions 3`

| Game | Hard | Jumps (reached) | Ret. arith. | RAM changed | CPU | Non-entries (RAM, ROM) | Summaries | Not discovered (ROM + RAM) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Metroid: Zero Mission | 18 | 3 (0) | 0 | 2 | 0 | 13 (13, 0) | 0 | 2,098 (12 + 2,086) |
| WarioWare, Inc. | 22 | 4 (1) | 2 | 0 | 0 | 14 (3, 11) | 2 | 2,266 (184 + 2,082) |
| moonbeam | 23 | 11 (3) | 0 | 0 | 0 | 0 | 12 | 438 (438 + 0) |
| Kirby & the Amazing Mirror | 29 | 2 (1) | 0 | 3 | 0 | 23 (5, 18) | 1 | 1,740 (746 + 994) |
| Kirby: Nightmare in Dream Land | 30 | 2+1 (2) | 0 | 1 | 3 | 23 (4, 19) | 0 | 3,464 (2,514 + 950) |
| Pokémon Emerald | 34 | 11 (0) | 0 | 1 | 0 | 20 (4, 16) | 2 | 2,444 (772 + 1,672) |
| The Minish Cap | 46 | 25 (5) | 0 | 0 | 0 | 15 (1, 14) | 6 | 3,294 (386 + 2,908) |
| Emerald Rogue | 86 | 50 (2) | 0 | 2 | 0 | 26 (5, 21) | 8 | 962 (662 + 300) |

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

What the run's targets were, per game: m4a's command table
(`gMPlayJumpTableTemplate` and its copies in RAM: Emerald, both Kirbys,
Minish Cap, Rogue), its leaf routines without a `push {lr}` that the
scan's score rejects, and WarioWare's pointers computed at run time (three
targets no ROM word points at).

Lines of 2026-11-05:

1. `WarioWare, Inc. - Mega Microgame$!.gba: hard 11 (jumps 4+0, return arithmetic 2, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 3, summaries 2); not discovered 2096 bytes (14 ROM, 2082 RAM) of 54360 reached; busy loops calling functions 0`
2. `Metroid - Zero Mission (USA).gba: hard 18 (jumps 3+0, return arithmetic 0, RAM code changed 2, CPU state 0, BIOS 0, calls to non-entries 13, summaries 0); not discovered 2098 bytes (12 ROM, 2086 RAM) of 83346 reached; busy loops calling functions 0`
3. `moonbeam.gba: hard 23 (jumps 11+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 0, summaries 12); not discovered 438 bytes (438 ROM, 0 RAM) of 49918 reached; busy loops calling functions 0`
4. `Kirby & The Amazing Mirror.gba: hard 23 (jumps 14+0, return arithmetic 0, RAM code changed 3, CPU state 0, BIOS 0, calls to non-entries 5, summaries 1); not discovered 1356 bytes (362 ROM, 994 RAM) of 105088 reached; busy loops calling functions 0`
5. `Kirby - Nightmare in Dream Land.gba: hard 23 (jumps 14+1, return arithmetic 0, RAM code changed 1, CPU state 3, BIOS 0, calls to non-entries 4, summaries 0); not discovered 2698 bytes (1748 ROM, 950 RAM) of 60236 reached; busy loops calling functions 0`
6. `Pokemon - Emerald Version.gba: hard 29 (jumps 22+0, return arithmetic 0, RAM code changed 1, CPU state 0, BIOS 0, calls to non-entries 4, summaries 2); not discovered 1722 bytes (50 ROM, 1672 RAM) of 58454 reached; busy loops calling functions 0`
7. `Legend of Zelda, The - The Minish Cap (USA).gba: hard 41 (jumps 34+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 1, summaries 6); not discovered 2960 bytes (52 ROM, 2908 RAM) of 40408 reached; busy loops calling functions 0`
8. `Emerald Rogue.gba: hard 75 (jumps 60+0, return arithmetic 0, RAM code changed 2, CPU state 0, BIOS 0, calls to non-entries 5, summaries 8); not discovered 342 bytes (42 ROM, 300 RAM) of 79416 reached; busy loops calling functions 3`

| Game | Hard | Jumps (reached) | Ret. arith. | RAM changed | CPU | Non-entries (RAM, ROM) | Summaries | Not discovered (ROM + RAM) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| WarioWare, Inc. | 11 | 4 (1) | 2 | 0 | 0 | 3 (3, 0) | 2 | 2,096 (14 + 2,082) |
| Metroid: Zero Mission | 18 | 3 (0) | 0 | 2 | 0 | 13 (13, 0) | 0 | 2,098 (12 + 2,086) |
| moonbeam | 23 | 11 (3) | 0 | 0 | 0 | 0 | 12 | 438 (438 + 0) |
| Kirby & the Amazing Mirror | 23 | 14 (13) | 0 | 3 | 0 | 5 (5, 0) | 1 | 1,356 (362 + 994) |
| Kirby: Nightmare in Dream Land | 23 | 14+1 (13) | 0 | 1 | 3 | 4 (4, 0) | 0 | 2,698 (1,748 + 950) |
| Pokémon Emerald | 29 | 22 (11) | 0 | 1 | 0 | 4 (4, 0) | 2 | 1,722 (50 + 1,672) |
| The Minish Cap | 41 | 34 (14) | 0 | 0 | 0 | 1 (1, 0) | 6 | 2,960 (52 + 2,908) |
| Emerald Rogue | 75 | 60 (12) | 0 | 2 | 0 | 5 (5, 0) | 8 | 342 (42 + 300) |

Every new unresolved jump is an m4a command routine returning through
the lr it saved in r12 (`mov r12, lr`, a `bl`, `bx r12`): reached, and a
return, which Recovery doesn't see yet (filed as its own unit). The
non-entries left are all in RAM: the flash library's stack routines and
the games' own IWRAM copies, the other two discovery units' work. The
lifter doesn't take run roots yet: `discover_with` is what it would call.

## Each game's worst holes

- **Metroid: Zero Mission.** All 13 calls to non-entries are into RAM:
  the flash library's stack routines (0x03007d09-0x03007d91, which also
  change) and the code Metroid copies to IWRAM itself (0x03003b90, copy of
  ROM 0x08004464; 0x03004100, of 0x080046d0). None of its three
  unresolved jumps was reached. Closest to native: discover its own
  IWRAM copies and stand in for the flash routines, and only three jumps
  remain to explain.
- **WarioWare.** The return arithmetic: 0x3000468 (returns to the site
  - 24) and 0x3000474 (site - 20), the IWRAM loop of
  `code-without-rom-addresses.md`, copied by the game from ROM 0x080007e4.
  One hand-written stand-in covers both. Its other RAM code (2,082 bytes,
  seven runs, each a copy of ROM bytes) needs only discovery.
- **Kirby & the Amazing Mirror.** The flash stack routines (three changing
  runs at 0x03007d58-0x03007da2) and 18 ROM call targets not discovered as
  starts (0x08149865-0x0814ffe1, most of them called from one site,
  0x0814fb86).
- **Kirby: Nightmare in Dream Land.** Its coroutine switch (code at
  0x08000234-0x080002a0: `str sp, [r3]; mov sp, r2` and back), which a native
  program can only express as a hand-written stand-in (threads or a state
  machine). Also 2,514 ROM bytes run but not discovered, the most of the
  eight.
- **Pokémon Emerald.** 16 m4a routines called through tables
  (`SoundMainBTM` 0x082df7e4 and its neighbours, from 0x082dfb5a), the
  flash stack routines, and `SoundMainRAM` (0x082df0e0) and `IntrMain`
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
  0x03005b10, `codegen::variants`), the flash stack routines, 8 functions
  with a run-time sp, and the only busy loops calling functions.

## What the Stage-A lifter should know

- Take holes from `analysis::holes::classify`; its `Report` lists each
  site. The IRQ wrapper and boot `msr`/`mrs` come back as `CpuWhere::Wrapper`
  and `Boot`: recognised, not holes.
- Most of each game's hard total is roots discovery lacks, not code that
  can't be expressed: the run's call targets (`Run::call_targets`,
  `indirect_calls`) and the RAM copies' ROM sources (`RamRun::rom_src`)
  are the roots to add.
- The flash library's stack routines are in five of the eight: one
  stand-in in the hardware library (flash commands) covers them all.
- Unreached unresolved jumps dominate Minish Cap's and Rogue's counts;
  until coverage reaches them, they are as likely dead as live.

## Proposed units

Filed from this report (see the tracker for their state):

- Discovery takes the run's call targets as function starts (the largest
  hard class in every game).
- Discovery finds code games copy to RAM themselves (loops, and `CpuSet`
  with arguments it can't fold): the RAM bytes not discovered.
- Coverage runs past the press script, to reach the unresolved jumps and
  settle each as a table or dead code.
- A hardware-library stand-in for the flash library's stack routines.
- Recovery takes a `bx` through a copy of lr as a return (filed after
  the run's call targets, 2026-11-05: m4a's `bx r12`).

## Limits

- One route of 90 seconds per game, from reset, no save file. Code the
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
