# Hole report: what blocks a native build of each game

research/native-holes (unit native/hole-report), 2026-11-01. For the
Stage-A lifter and for june: of the eight games of
`docs/research/code-without-rom-addresses.md`, which is closest to a
native build (`docs/native-port-design.md`, section 6), and what stands
in the way of each.

## Summary

- **Ranked by hard holes, then bytes reached but not discovered:**
  Metroid: Zero Mission (4), WarioWare (19), Kirby & the Amazing Mirror
  (21), moonbeam (23), Kirby: Nightmare in Dream Land (25), Pokémon
  Emerald (29), The Minish Cap (46), Emerald Rogue (80). (Before the RAM
  copies were found, 2026-11-06: 7, 21, 21, 23, 25, 29, 46, 80; before
  the flash stand-in, 2026-11-05: 18, 22, 29, 23, 30, 34, 46, 86.)
- **No game runs RAM code the report doesn't discover** (it was 228 to
  2,908 bytes in seven of the eight): the code the games copy into RAM
  themselves, by DMA (pret's `IntrMain`), by their own loops, or by a BIOS
  copy entered only through a pointer (m4a's `SoundMainRAM`), is found
  with the run's evidence ("RAM copies", below). It brought seven jumps
  through registers in the copies into view (two in Metroid; none in
  moonbeam, and Kirby: Nightmare's one copy jump was counted before):
  five were counted at their ROM source before and now count at the copy
  instead, and two are new (Metroid's 0x030016d8, reached, and Minish
  Cap's 0x0300670c, not).
- **The commonest hard hole in five of the eight is a call through a
  register to an address discovery doesn't know as a function start** (13
  to 19 a game in WarioWare, both Kirbys and Emerald; 14 and 21 in
  Minish Cap and Rogue; none in Metroid and moonbeam), all into ROM now:
  m4a's routines called through its tables (Emerald's `SoundMainBTM`
  family). They are discovery gaps, not code a native program can't
  express: give discovery the roots and they go. (The calls into code the
  games copy into RAM, Metroid's four, WarioWare's two and Minish Cap's
  one, are gone with the RAM copies.)
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
4. the RAM copies again, with what the run saw
   (`analysis::holes::ram_copies`, "RAM copies" below);
5. the classification, `analysis::holes::classify` (in the analysis crate
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
a call to a non-entry. A ROM source and its RAM copy are classified once, at the copy (an
indirect jump too: the ROM source's is dropped where the copy's analysis
has it);
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

### RAM copies

Discovery folds only a BIOS `CpuSet`/`CpuFastSet` out of ROM with
constant arguments into a copy, and discovers a copy at its destination
only from the constant RAM addresses the ROM's code reaches. The games
copy code other ways (by DMA, by their own loops) and enter copies
through pointers. `analysis::holes::ram_copies` takes the run as the
evidence:

1. each BIOS copy into RAM gets the run's call targets and IRQ handlers
   inside it as roots, besides the ROM's constant entries;
2. RAM code the run executed outside those is a copy of the ROM where
   its first bytes (up to 64, at least 8) are at the same alignment: of
   several such places, the one whose bytes agree with the most of the
   executed code from there on (gaps up to 1 KiB). A copy ends where its
   source stops agreeing, or at code after a gap that another place
   agrees with further (another copy whose first bytes happen to match).
   Metroid's 0x03004100, which the first-64-bytes match took for a copy
   of 0x080046d0 (they agree for 0xc0 bytes), is more of the copy at
   0x03003b90 from 0x08004464: all its executed bytes agree with
   0x080049d4. Its image runs 256 bytes past its
   last executed instruction (its literal pools, the end of its last
   function), short of the next copy;
3. in both, where the run executed code that agrees with the ROM source
   but discovery from those roots doesn't reach (a jump through a register
   into it, not a call), the lowest such address becomes a root too, until
   none is left.

Code that changed while it ran (a hole of its own) and the flash
routines (the hardware library's) are left out, and code whose bytes
aren't in the ROM (built at run time) stays undiscovered. The report
lists each copy: where, from where, a BIOS copy or seen in the run, its
functions, the roots it was entered at and the ones filled in. Over the
eight: 24 copies, 8 of them BIOS copies, 13 with roots filled in.
`codegen::seen` finds IWRAM copies by running the ROM too, for the play
build's overlays; it widens each by comparing RAM with the ROM, which
the hole report's run doesn't keep.

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

Each line as `just holes` prints it (2026-11-06, with the RAM copies):

1. `Metroid - Zero Mission (USA).gba: hard 4 (jumps 2+2, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 0, summaries 0); not discovered 12 bytes (12 ROM, 0 RAM) of 83346 reached; busy loops calling functions 0; flash library stand-ins 2`
2. `WarioWare, Inc. - Mega Microgame$!.gba: hard 19 (jumps 3+1, return arithmetic 2, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 11, summaries 2); not discovered 184 bytes (184 ROM, 0 RAM) of 54360 reached; busy loops calling functions 0; flash library stand-ins 1`
3. `Kirby & The Amazing Mirror.gba: hard 21 (jumps 1+1, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 18, summaries 1); not discovered 746 bytes (746 ROM, 0 RAM) of 105088 reached; busy loops calling functions 0; flash library stand-ins 2`
4. `moonbeam.gba: hard 23 (jumps 11+0, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 0, summaries 12); not discovered 438 bytes (438 ROM, 0 RAM) of 49918 reached; busy loops calling functions 0; flash library stand-ins 0`
5. `Kirby - Nightmare in Dream Land.gba: hard 25 (jumps 2+1, return arithmetic 0, RAM code changed 0, CPU state 3, BIOS 0, calls to non-entries 19, summaries 0); not discovered 2514 bytes (2514 ROM, 0 RAM) of 60236 reached; busy loops calling functions 0; flash library stand-ins 2`
6. `Pokemon - Emerald Version.gba: hard 29 (jumps 10+1, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 16, summaries 2); not discovered 772 bytes (772 ROM, 0 RAM) of 58454 reached; busy loops calling functions 0; flash library stand-ins 2`
7. `Legend of Zelda, The - The Minish Cap (USA).gba: hard 46 (jumps 25+1, return arithmetic 0, RAM code changed 0, CPU state 0, BIOS 0, calls to non-entries 14, summaries 6); not discovered 386 bytes (386 ROM, 0 RAM) of 40408 reached; busy loops calling functions 0; flash library stand-ins 0`
8. `Emerald Rogue.gba: hard 80 (jumps 49+1, return arithmetic 0, RAM code changed 1, CPU state 0, BIOS 0, calls to non-entries 21, summaries 8); not discovered 662 bytes (662 ROM, 0 RAM) of 79416 reached; busy loops calling functions 3; flash library stand-ins 2`

| Game | Hard | Jumps (reached) | Ret. arith. | RAM changed | CPU | Non-entries (RAM, ROM) | Summaries | Not discovered (ROM + RAM) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Metroid: Zero Mission | 4 | 2+2 (2) | 0 | 0 | 0 | 0 | 0 | 12 (12 + 0) |
| WarioWare, Inc. | 19 | 3+1 (2) | 2 | 0 | 0 | 11 (0, 11) | 2 | 184 (184 + 0) |
| Kirby & the Amazing Mirror | 21 | 1+1 (2) | 0 | 0 | 0 | 18 (0, 18) | 1 | 746 (746 + 0) |
| moonbeam | 23 | 11 (3) | 0 | 0 | 0 | 0 | 12 | 438 (438 + 0) |
| Kirby: Nightmare in Dream Land | 25 | 2+1 (2) | 0 | 0 | 3 | 19 (0, 19) | 0 | 2,514 (2,514 + 0) |
| Pokémon Emerald | 29 | 10+1 (1) | 0 | 0 | 0 | 16 (0, 16) | 2 | 772 (772 + 0) |
| The Minish Cap | 46 | 25+1 (5) | 0 | 0 | 0 | 14 (0, 14) | 6 | 386 (386 + 0) |
| Emerald Rogue | 80 | 49+1 (3) | 0 | 1 | 0 | 21 (0, 21) | 8 | 662 (662 + 0) |

## Each game's worst holes

- **Metroid: Zero Mission.** Four hard holes, all jumps through a
  register: two in ROM, unreached (0x087608fc, 0x087d8004), and two in its
  IWRAM copies, both reached: 0x03000d80, in its IRQ dispatcher (copied
  from 0x08000104), and 0x030016d8 (from 0x08057f7c). Its 4 calls to
  non-entries were into code it copies to IWRAM itself (0x03003b90-0x03004440:
  0x03003b90 from 0x08004464, then 0x08004310's and 0x080043b4's
  functions after it), now discovered; the other 9 were the flash
  library's stack routines (0x03007d09-0x03007d91), stood in for.
  Closest to native: four jumps to explain.
- **WarioWare.** The return arithmetic: 0x3000468 (returns to the site
  - 24) and 0x3000474 (site - 20), the IWRAM loop of
  `code-without-rom-addresses.md`, copied by the game from ROM 0x080007e4.
  One hand-written stand-in covers both. Its other RAM code (2,048 bytes
  in seven copies, none of them a BIOS copy) is discovered from the run.
- **Kirby & the Amazing Mirror.** 18 ROM call targets not discovered as
  starts (0x08149865-0x0814ffe1, most of them called from one site,
  0x0814fb86).
- **Kirby: Nightmare in Dream Land.** Its coroutine switch (code at
  0x08000234-0x080002a0: `str sp, [r3]; mov sp, r2` and back), which a native
  program can only express as a hand-written stand-in (threads or a state
  machine). Also 2,514 ROM bytes run but not discovered, the most of the
  eight.
- **Pokémon Emerald.** 16 m4a routines called through tables
  (`SoundMainBTM` 0x082df7e4 and its neighbours, from 0x082dfb5a).
  `SoundMainRAM` (a BIOS copy of 0x082df0e0 entered through `SoundInfo`)
  and `IntrMain` (copied by DMA from 0x08000248) are discovered from the
  run. Eleven unresolved jumps: ten in ROM, none reached, and `IntrMain`'s
  jump through its table at 0x03002878, reached.
- **moonbeam.** No RAM code at all, no call to a non-entry, and the fewest
  bytes undiscovered; its holes are 11 unresolved jumps (3 reached) and 12
  functions whose summary returns with sp changed (a run-time sp
  adjustment). Its interworking veneers (`bx pc` then ARM) are calls the
  tool follows, not holes.
- **The Minish Cap.** 25 unresolved jumps in ROM, 5 of them reached
  (0x080002a2, 0x080002a6, 0x080002b2, 0x08004486, 0x080af758), and one
  in its IWRAM code, unreached (0x0300670c). That code (2,908 bytes run)
  is a BIOS copy (0x0300404c from 0x080af3a4, entered only by a jump) and
  a copy only the run shows (0x030056f0-0x030068a4 from 0x080b197c, 21
  functions), both discovered now.
- **Emerald Rogue.** 50 unresolved jumps (2 reached: in `SoundMain` and
  `MPlayJumpTableCopy`; one more, reached, in its IRQ dispatcher's IWRAM
  copy at 0x03001a58, which counts instead of its ROM source), its mixer's template stamping (0x030059f0-
  0x03005b10, `codegen::variants`), 8 functions
  with a run-time sp, and the only busy loops calling functions.

## What the Stage-A lifter should know

- Take holes from `analysis::holes::classify`; its `Report` lists each
  site. The IRQ wrapper and boot `msr`/`mrs` come back as `CpuWhere::Wrapper`
  and `Boot`: recognised, not holes.
- Most of each game's hard total is roots discovery lacks, not code that
  can't be expressed: the run's call targets (`Run::call_targets`,
  `indirect_calls`) are the roots to add. The RAM copies are
  `analysis::holes::ram_copies` (each an `Overlay`, with the roots it was
  given), which needs a run; a lift of a game that copies code without a
  BIOS copy discovery folds needs that run too, or the same copies found
  statically.
- The flash library's stack routines (six of the eight call them, five
  at changing addresses) are `Report::flash`: at each such call into RAM,
  call the `runtime::hw::flash` function its `Routine::call` names on
  `Hw::save`, with the routine's arguments (r0-r2) as its slices.
- Unreached unresolved jumps dominate Minish Cap's and Rogue's counts;
  until coverage reaches them, they are as likely dead as live.

## Proposed units

Filed from this report (see the tracker for their state):

- Discovery takes the run's call targets as function starts (the largest
  hard class in every game).
- Discovery finds code games copy to RAM themselves (loops, DMA, and
  `CpuSet` with arguments it can't fold): the RAM bytes not discovered.
  Done from the run ("RAM copies", above); found statically, it would
  serve a lift with no run.
- Coverage runs past the press script, to reach the unresolved jumps and
  settle each as a table or dead code.
- A hardware-library stand-in for the flash library's stack routines
  (done: "The flash library's stack routines", above).

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
