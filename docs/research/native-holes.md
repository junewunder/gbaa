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
  them are what tell real tables from dead code. moonbeam's largest class
  is 12 functions whose summary returns with sp changed.
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
  settle each as a table or dead code.
- A hardware-library stand-in for the flash library's stack routines
  (done: "The flash library's stack routines", above).
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
