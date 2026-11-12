# Hole report: what blocks a native build of each game

research/native-holes (unit native/hole-report), 2026-11-01. For the
Stage-A lifter and for june: of the eight games of
`docs/research/code-without-rom-addresses.md`, and Emerald Imperium
(modern GCC, added by `native/modern-gcc-jumps`), which is closest to a
native build (`docs/native-port-design.md`, section 6), and what stands
in the way of each.

This file is the method and what the findings mean. The current counts,
a row per game, are [native-holes.numbers.md](native-holes.numbers.md),
written by `just holes-all --write-doc docs/research/native-holes.numbers.md`
(the corpus is `holes/corpus.toml`); each game's full report is
`just holes ROM`. Each unit that moved the counts is named below; its
tracker evidence has the before and after (`just show <id> --full`).
`docs/budgets.toml` holds this file to a line budget.

## Summary

- **Most of the first count was roots discovery lacked, not code a
  native program can't express.** Calls through a register to addresses
  discovery didn't know as starts (m4a's routines called through its
  tables, code the games copy to RAM with their own loops) were the
  commonest hard hole in five of the eight. Three units took them out:
  discovery takes the run's call targets as starts
  (`native/disc-run-call-targets`), the RAM copies are found with the
  run's evidence (`native/disc-ram-copies`), and Recovery takes m4a's
  `mov r12, lr` … `bx r12` as a return (`native/recovery-saved-lr-return`).
  No game has a call to a non-entry or RAM code the report doesn't
  discover now.
- **What's left is mostly indirect jumps Recovery leaves unresolved**,
  most in Minish Cap and Emerald Rogue, and most never reached by any run:
  coverage runs (`native/hole-coverage`, "Coverage runs" below) tell real
  tables from dead code. The reached ones are jumps into RAM code and
  a few real switch tables Recovery misses (Rogue's, moonbeam's). The summaries' holes were mostly the
  analysis being too cautious ("Functions that change r4-r11 or sp",
  below; `native/callee-save-breaks`): 31 then, 14 after, all real
  convention breaks in hand-written code or data taken for code.
- **The flash library's stack routines are hardware-library calls, not
  holes** (`native/flash-stack-standin`; below). Five games ran code from
  RAM that changes; only Emerald Rogue's mixer, stamping templates into
  its loop, still does. Both were in the design's list.
- **The controls hold:** WarioWare's IWRAM helper (0x3000468, `sub lr, lr,
  #24`) and its twin are the only return-address arithmetic not resolved;
  moonbeam's `__gnu_thumb1_case_uqi` and `_uhi` are found statically and
  resolved (Recovery resolves each `bl` to them as a jump), and its
  interworking veneers (`bx pc` then ARM) are calls the tool follows;
  Emerald's unresolved-jump count is Recovery's.
- **Kirby: Nightmare in Dream Land switches stacks** (three sites,
  0x08000244, 0x08000278 and 0x08000298, in code at
  0x08000234-0x080002a4: `str sp, [r3]; mov sp, r2` and back, a
  coroutine: its object tasks, each on its own stack); a native program
  can only express it as a stand-in that runs each task on a stackful
  coroutine ("Kirby: Nightmare's task switcher", below).
- **Busy-wait loops that call functions** (Emerald's `DoMapLoadLoop`
  case, an open question to june): with the press script, found only in
  Emerald Rogue, three, in `LoadMapInStepsLocal`,
  `FreeTempTileDataBuffersIfPossible` and `CB2_NewGame` (186
  non-progressing iterations in a row). The coverage runs find more (Rogue
  15, Emerald 5): map loads the press script never reaches.

## The tool

`just holes ROM` (`crates/hole-report`, `gbaa-holes`; its steps are in
the command entry, `docs/tools/commands.md`): discovery and its IWRAM
overlays, a 5,400-frame instant-timing run with the research press
script (Start, A and B tapped, then right, up and left held 1,000 frames
each from frame 2000), discovery again with the run's call targets
("With the run's call targets"), the summaries, the RAM copies with the
run ("RAM copies"), then `analysis::holes::classify` (in the analysis
crate so the lifter reuses it). About a minute a game; `just holes-all`
runs the corpus into one table (about 10 minutes).

### What counts as a hard hole

| Hole | How it's found | Counted |
|---|---|---|
| Indirect jump not resolved | Recovery's `Unresolved` and `Rejected`, ROM and overlays; its `Ram` (one constant RAM target) where no overlay has a function there | every one, reached or not ("jumps A+B": ROM + overlays) |
| Return-address arithmetic | the run: a return (`bx`, `pop {pc}`, `mov pc, lr`, `ldr pc, [sp]`) landing 64 bytes before to 1 KiB after the innermost open call's return site, not on it, not on another call's return site, not on a function start, not inside the callee's own function; statically: a function doing arithmetic on lr, or loading through it, before writing lr | where Recovery doesn't resolve the call site as a jump |
| RAM code that changed | the run: an address run again with different bytes or instruction set | each run of addresses (gaps up to 16 bytes) |
| CPU state | `msr`, `mrs`, `ldm`/`stm` with the S bit, `s` writes to pc, stack switches (sp moved 4 KiB or more by one instruction other than sp-relative arithmetic) | outside the IRQ wrapper (code run at the top level of an IRQ, and the rest of its function; for a copy made by the game's loop, at the RAM address) and boot (code run only in the first frame, and its function) |
| BIOS call outside the library | `swi` numbers the native lifter doesn't print (the ones not in `analysis::holes::NATIVE_BIOS`) | each site |
| Call to a non-entry | the run: a call through a register (`mov lr, pc` or `adr lr, …` then `bx rN`, or a `bl` to a `bx rN` stub other than `bx pc`) to an address not a discovered function start or RAM entry | each target |
| Summary hole | `analysis::holes::SUMMARY_HARD`: a return with sp changed (moved by a run-time amount, as a variable-length array does), a read of r4-r11 a callee changed or that came in as a parameter, a function that changes r4-r11 whose key is a word in the ROM (below) | each function, once |

A place can be counted under two kinds: a routine copied onto the stack
that isn't recognised would be RAM code that changed and, at each entry,
a call to a non-entry. A ROM source and its RAM copy are classified once,
at the copy (an indirect jump too: the ROM source's is dropped where the
copy's analysis has it); a `bl` to a switch helper that Recovery rejects
counts as a jump only.

CPU-state and BIOS items in code only the pointer scan found and the run
never ran aren't counted (Emerald's 0x087f22xx: data that decodes as
`swi 0x64` and `ldm ^`). Other summary holes are counted elsewhere (an
unknown jump, a mode change, RAM code) or aren't holes for a native build
(an indirect call is an `FnRef`), and are printed but not counted.

**The BIOS count is the interpreter's, not the lifter's** (against
`runtime::bios`; Metroid's lift found 9 the report counts as 0;
`native/lift-bios-calls`). **Not discovered** is the bytes of code the
run executed that discovery (or an overlay's) didn't find, ROM and RAM.

### The flash library's stack routines

Nintendo's `AgbFlash` (pret's `agb_flash.c`) copies three tiny routines
(`ReadFlash1`, `ReadFlash_Core`, `VerifyFlashSector_Core`) into its
caller's frame and calls them there. The run keeps the 64 bytes at each
call into RAM (`Run::ram_calls`); a target whose every version is one
`analysis::flash` recognises (length and hash: agbcc's three, modern
GCC's two) is a call of `runtime::hw::flash::{read_byte, read, verify}`
on `Hw::save`, listed as a "flash library stand-in", and its RAM isn't
changed code, undiscovered code nor a non-entry. Six of the eight call
them (not moonbeam, Minish Cap); no stage-A build lifts one yet.

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
   Its image runs 256 bytes past its last executed instruction (its
   literal pools, the end of its last function), short of the next copy;
3. in both, where the run executed code that agrees with the ROM source
   but discovery from those roots doesn't reach (a jump through a register
   into it, not a call), the lowest such address becomes a root too, until
   none is left.

Code that changed while it ran (a hole of its own) and the flash
routines (the hardware library's) are left out, and code whose bytes
aren't in the ROM (built at run time) stays undiscovered. The report
lists each copy (where, from where, how found, its functions, the roots
it was entered at and filled in); the ones seen in the run again as
`[[copy]]` tables for the lift's holes file (`native/lift-ram-copies`).
`codegen::seen` finds IWRAM copies by running the ROM too, for the play
build's overlays, widening each by comparing RAM with the ROM.

### Busy loops that call functions

Counted but not added to the hard total until june answers how a native
program lets time pass in them: a backward branch outside IRQs taken 64
or more times in a row in one frame by iterations that each made a call,
didn't halt, loaded RAM an IRQ handler had written, and made the same
stores as the iteration before (no progress of their own). At 8 in a row
the same rule also caught pure loops over short lists (Rogue's `FlagGet`
per item, 27); at 64 only the map-load loops remain.

## With the run's call targets

`gbaa-holes` discovers the ROM a second time after the run, with every
ROM target of the run's calls through a register as a function start
(`analysis::discovery::discover_with`), and classifies against that
(`--no-run-roots` leaves it out; `native/disc-run-call-targets`). Two
things made that work:

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
targets no ROM word points at). The lifter doesn't take run roots yet:
`discover_with` is what it would call.

## Coverage runs

`gbaa-holes ROM --cover holes/coverage/<game>.toml` (`native/hole-coverage`;
`crates/hole-report/src/cover.rs`; command entry in
`docs/tools/commands.md`) plays coverage runs after the press script,
into the same record. Every run is seeded and repeatable: a random walk
(a direction or none held 1 to 48 frames, A, B, Start, Select, L, R now
and then, A tapped every eighth frame) after a short script. The runs,
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

The run records, at every indirect jump executed (`bx`, a load or
data-processing write to pc, `ldm` with pc), the runs that executed it and
each target (up to 256). Each coverage file's `[[jump]]` list, below its
marker, is every jump Recovery leaves unresolved, with the runs that
reached it (none: unreached by every run) and its targets, written with
`--update`; without it the tool checks the list against the runs and
fails on any difference. **Those lists are the per-game record** of which
jumps are reached and where they go. A game takes 4 to 25 minutes
(Emerald the longest: 107,400 frames, 19 G instructions).

What the reached ones were, when the coverage runs were first made: m4a's
returns through r12 (11 to 14 a game, taken by Recovery since
`native/recovery-saved-lr-return`); Minish Cap's two-instruction veneers
into the code it copies to IWRAM itself (0x0800029e-0x080002f2,
`CheckBits`, `CalcCollisionStaticEntity`; the walk reached 12 the press
script didn't); real switch tables Recovery misses (four in Rogue,
reached only from the owner's save, five in moonbeam); and jumps into
RAM code (WarioWare's 0x080efe76, Rogue's `SoundMain`, Kirby:
Nightmare's IWRAM 0x030010f8 back to ROM).

Unreached by everything include, where named, the GameCube multiboot
handlers (Emerald, Rogue), `__make_dp` (libc's soft-float, Emerald and
WarioWare), `IntrMain`'s and `__start`'s tails, and most of Rogue's; a
coverage-guided run, or saves further into Rogue, is what would settle
them.

## What discovery misses

Bytes run but not discovered (the report lists each run with what
discovery has around it). The coverage runs miss no more than the press
script (2026-11-07), none a jump into a function's middle or the wrong
instruction set. Bytes, Nightmare / Mirror:

| Cause | Before | After | Now |
|---|---|---|---|
| a task run as a coroutine (`push {…, lr}`, then an endless loop that yields each frame), reached only from a table of (kind, task) words: the scan wanted a return; five tasks and the function only one calls | 1,622 / 0 | 0 / 0 | 0 / 0 |
| a BIOS copy whose length reads on into ROM code a `bl` calls (Mirror: 0x1c0 bytes from 0x08158f14 over its heap allocator), and the code after the calls | 0 / 312 | 0 / 0 | 0 / 0 |
| m4a's `SoundMainRAM` copy ends in the `bx r3` that `SoundMain` calls in ROM (a stub), so `SoundMain`'s tail after the call (every m4a game: Emerald and Minish Cap too) | 38 / 38 | 38 / 38 | 0 / 0 |
| the return from the scheduler's call to resume ("Kirby: Nightmare's task switcher", below), through a `bx pc` veneer taken for a computed return | 44 / 0 | 44 / 0 | 0 / 0 |
| crt0's copy (0x08000108, 0x140 bytes) reads on into the switcher, which a veneer's ARM `b` enters | 36 / 0 | 36 / 0 | 0 / 0 |
| `bx rN` stubs (agbcc's `_call_via_rN`, m4a's): a `bl` to one calls rN, and the stub isn't kept as code | 8 / 12 | 8 / 12 | 10 / 14 |

After: `native/kirby-undiscovered` (endless tasks; `discovery::copy_cuts`
at a `bl` target). Now: `native/copy-tail-discovery`: cuts at `b` and
stub targets too (a `bl` to a stub returns); a Thumb `bx pc` is never a
return (`classify` took the veneer's linear predecessor, a `pop {…, pc}`,
for its own); a constant jump into RAM, Recovery's `JumpRes::Ram`, is
resolved where an overlay has a function there: `SoundMain`'s, Minish
Cap's two reached ones, one each in Rogue and Imperium (hard 106 to 103).
Resume's `bx r1` (0x08000254), found now, is real: Nightmare 9 to 10, its
lift stopped there until the switcher's stand-in covers it.

## Each game's worst holes

The counts are the numbers file's; what they are, beyond the summary's:

- **Metroid: Zero Mission.** Jumps through a register only: two in ROM,
  unreached (0x087608fc, 0x087d8004), and two in its IWRAM copies, both
  reached: 0x03000d80, in its IRQ dispatcher (copied from 0x08000104),
  and 0x030016d8 (from 0x08057f7c).
- **WarioWare.** The return arithmetic: 0x3000468 (returns to the site
  - 24) and 0x3000474 (site - 20), the IWRAM loop of
  `code-without-rom-addresses.md`, copied by the game from ROM 0x080007e4.
  One hand-written stand-in covers both.
- **Kirby & the Amazing Mirror.** The fewest hard holes: most of what it
  had were m4a's routines called through tables and its returns through
  r12. **Kirby: Nightmare** has the coroutine switch, and with it the
  ROM bytes run but not discovered (above).
- **Pokémon Emerald.** Unresolved jumps, none of m4a's now; `IntrMain`'s
  jump through its table counts at its copy (reached).
- **The Minish Cap.** Unresolved jumps, the veneers into its IWRAM code
  most of the reached ones. That code is a BIOS copy (0x0300404c from
  0x080af3a4, entered only by a jump) and a copy only the run shows
  (0x030056f0-0x030068a4 from 0x080b197c), both discovered now.
- **Emerald Rogue and Imperium.** Unresolved jumps, mostly unreached and
  most not tables now (below); Rogue's
  mixer's template stamping (0x030059f0-0x03005b10,
  `codegen::variants`); libgcc's soft-float division helper (below); and
  the busy loops calling functions.

### Kirby: Nightmare's task switcher

Nightmare's three CPU-state holes (2026-11-07) are its object-task
switcher, ARM routines at 0x08000234-0x080002a4 reached from Thumb by
`bx pc` veneers (0x080cfdc4, 0x080cfdcc, 0x080cfdd4). Each object's task
has its own IWRAM stack and is run once a frame:

- **Resume** (0x08000244, `mov sp, r2`), called only by the scheduler
  (0x08005364), r1 the task's resume address, r2 its saved sp: saves lr,
  r4-r11, stores the main sp at 0x03004c94, takes the task's sp, pops its
  r4-r11, `bx r1`.
- **Yield** (0x08000278, `ldr sp, [r3]`), r0 the frames to wait: pushes
  r4-r11, stores the task's sp, lr (its resume address) and r0, takes the
  main sp back and returns to the scheduler after its resume call.
- **Exit** (0x08000298, `ldr sp, [r3]`): frees the task (0x08005655),
  then returns to the scheduler the same way.

A byte scan finds 11,063 Thumb `bl`s to yield and 232 to exit (not
checked against discovery); a typical task is `loop { update();
yield(1) }` (0x080093fc). sp is already a parameter and the stacks IWRAM
bytes, so the guest side is a store and a load; the host stack is the
hole, since a yield must keep the task's frames while the scheduler goes
on. The fix is a stand-in running each task's lifted code on a stackful
coroutine (`corosensei`, as `crates/desktop/src/coro.rs`), keyed by its
saved sp; a new task starts with the r4-r11 the game put on its stack.
A state machine would split functions at 11,063 sites. Filed as
`native/nightmare-task-switch`. No other corpus game switches stacks
(Amazing Mirror doesn't); a game with a per-task-stack scheduler would
show the same holes, and the rule finds it anywhere.

## Functions that change r4-r11 or sp

`codegen::saves` runs each function abstractly; the lift keeps every
caller's r4-r11 and sp in its own locals and passes only r0-r3
(`Regs::call` zeroes the rest). On 2026-11-06 the report counted 31
functions returning with r4-r11 or sp changed. By cause, each read from
the disassembly (Emerald's by pret's map and source):

| Cause | Kind | Emer. | Kirby M. | Wario | moon | Minish | Rogue |
|---|---|---|---|---|---|---|---|
| a pop and return under one condition (`ldmhi sp!, {r4, r5, lr}; bxhi lr`, libgcc's `RETLDM`; `ldmdbeq r11, {r11, sp, lr}` in `IntrSIO32`, librfu_intr.c) | too cautious | 1 | | | 10 | | 4 |
| an ARM `ldr r4, [sp], #4` (newlib's `strcmp`): the load's rotation by the address's low bits wasn't known to be 0 | too cautious | | | | | | 2 |
| a loop counter's first value reaching past the loop (modern GCC's `strh` at sp + counter + 18) and killing two save slots | a value it couldn't follow | | | | | | 1 |
| hand-written scratch use, every caller saving r4-r11 round the call (TMC's `player.s`; its r8/r11 parameters to `sub_080086D8` are the soft kind below) | real, unobserved | | | | | 5 | |
| register parameters and results: TMC's `ClampPlayerVelocity` (r4, r5 from `AddPlayerVelocity`), WarioWare's copy loop 0x080efe51 and ARM mixer 0x080f0ab0 (r4-r7 in), libgcc ieee754-df.S's denormal paths entered by `bleq` (r4, r5 out to `__muldf3`, `__divdf3`) | real | | | 2 | 2 | 1 | 1 |
| data the pointer scan decoded as Thumb (`sDodrioHeadToColumnMap`, `sAreaMapScreenSizes`) | not code | 1 | 1 | | | | |

The fixes: the run keeps, per guard, a state where it holds and one
where it doesn't, and a guarded exit leaves with the first; the entry sp
is word-aligned and a pure op of constants folds; blocks run lowest
address first. A return with r4-r11 changed is no longer hard: the
summary records what it changes (`Summary::changes`), its callers take
those registers as changed, and the hard holes are the reads that see
it: a caller reading one before writing it, a function that changes
them reading one as it came (other than to save it), and one whose key
is a word in the ROM (called through a pointer, callers unknown). A
function that keeps them and reads one as it came is mostly agbcc
building a word byte by byte in an unset register (`ands r4, r0; orrs
r4, r1`, `ply_xwave`), but not always: TMC's `sub_080086D8` keeps them
and takes r8 and r11 from its `player.s` callers as parameters. That
kind was soft, as main never looked; the next section follows the bits. After: 14, the six real ones, the six callers of
moonbeam's and Rogue's that read the r4 the helper returns, and the two
data. Each real one is the lift's stand-in to write (`[[standin]]` in
the game's holes file), the others as written. libgcc's `__muldf3` and
`__divdf3` are every game's: `runtime::softfloat`, found by its code.

## Entry reads of r4-r11, by the bits that survive

`native/entry-read-masking`, 2026-11-08. The summary's run
(`codegen::saves`) now follows which bits of each value may hold r4-r11's
entry values (`V::Part`): an `and` or `orr` with a constant clears bits, a
shift moves them, an add carries them up, a copy to a register, a flag or
a stack word keeps them. A use is an address, a value stored elsewhere, a
condition, a callee's or BIOS call's argument (`mov r0, r8; bl`), or a
value returned in r0-r3 that a caller's own instruction reads (agbcc's
`pop {r3}; mov r8, r3` leaves the entry r8 in r3, unread). The kinds:

- `reads r4-r11 at entry` (hard): a use, in a function entered by a call
  (`bl`, a complete indirect set, or a pointer: the key as a ROM word),
  whose `Regs::call` zeroes r4-r12.
- `masks away r4-r11's entry bits` (soft): every bit cleared before any
  use, agbcc's word built byte by byte in an unset register.
- `reads r4-r11 at entry, only jumped into` (soft): no `bl` or pointer
  enters it, nor a tail call or fall from a function one enters.
- `reads the caller's frame pointer` (soft): agb's backtrace walking r7
  from a frame record (`add r7, sp, #12` at the saved r7); natively the
  walk finds no frames. Only the agb programs have it.

A value that holds entry bits on one path and not another is unknown at
the merge, as before (a path that can't happen, or a C local never set):
counting those made LLVM's paths in agb hard.

Summary holes that are hard, before and after (`just holes-all --against`):
3 entry readers (of 5 summary holes) and 36; corpus hard 130 and 139. The 36, from the report:

| Cause | Functions |
|---|---|
| libgcc ieee754-df.S's denormal paths, r4 and r5 in (ARM) | Emerald 0x082dfd09, Rogue 0x0800cb45, Imperium 0x08003879, Kirby M. 0x0814fd35, Nightmare 0x080ce1f9, Minish 0x080afbd1 |
| hand-written register parameters | Minish 0x080086d9 (`sub_080086D8`, r8 and r11), 0x08060ba1 (derefs r5), 0x08008a89 (r5, `ClampPlayerVelocity`'s kind); WarioWare 0x080efe51, 0x080f0ab0 |
| modern GCC's far jump by `bl` into a fragment of the caller's, taken for a call | moonbeam 0x0801a081 and six more, Rogue 0x0820c629, Imperium 0x0821e331 (not looked into one by one) |
| agbcc's partly built word stored with the unset register's other bits (a struct's padding) | Emerald 0x080852d5, Kirby M. 0x08132889, WarioWare 0x080abfa5, Metroid 0x08073ce5 |
| an unset register used as an index (`lsls r1, r4, #2; ldr`) | Minish 0x08037c3d |
| data decoded as code | Emerald 0x0850dbaf, 0x085ef6a1; Metroid 0x087d8000; WarioWare 0x083db2d7 |
| not looked into | Nightmare 0x08064759, 0x080647fd, 0x0806b411, 0x0806b671; Rogue 0x081b42e1, 0x0826c5e7; moonbeam 0x0800019b |

The fully masked reads (soft), by function and the op that clears the
last bit: Emerald 0x082e14f5 (0x082e151a), 0x082e1629 (0x082e164e),
0x082e8a3d (0x082e8adc); Kirby M. 0x0815149d (0x081514c2); Nightmare
0x080cf961 (0x080cf986); Metroid 0x08073ce5 (0x08073cfe, r8); Minish
0x08037c3d, 0x080622f5, 0x0806252d, 0x08068319, 0x08068579, 0x080b13d5;
moonbeam 0x0801a081 (r7, four); WarioWare 0x0807f6e5, 0x0808e325,
0x080afab9, 0x080d19b9, 0x080d355d, 0x080dffe5, 0x080efe51, 0x080f56ed.
Only jumped into: Rogue's 0x080ba12b-0x080bbfff fragments (15, past
unresolved jumps), Nightmare 0x0804c489, moonbeam 0x08038310. A tail call
or fall from a called function counting as a call (2026-11-12) left the 36.

## Modern GCC's unresolved jumps, by kind

`native/modern-gcc-jumps`, 2026-11-07: every unresolved ROM jump of
Imperium and Rogue (`just holes ROM --show 200`), sorted by the code
before it (`just dis`), then the largest kinds fixed in
`analysis::vsa::exec`. Imperium / Rogue, before and after:

| Kind | Before | After |
|---|---|---|
| Switch on a narrowed copy: `lsls r3, r0, #16; lsrs r3, r3, #16; cmp r3, #7; bhi`, indexed with r0 (an `ldrsh`'s `s16`) | 34 / 23 | 2 / 2 |
| The frame rule: a table's base or index in a stack slot a call forgot (the frame had escaped), or the saved lr a `pop {r3}; bx r3` returns through | 13 / 8 | 1 / 3 |
| Other tables: a far-jump default (`bls; bl`) the probe missed, a base kept in a high register round a loop, one not looked into | 2 / 2 | 2 / 2 |
| Data decoded as code (pointer tables at 0x0828xxxx and 0x0826xxxx) | 14 / 6 | 14 / 6 |
| libgcc's `_call_via_r0`-`r2` (`bx rN` stubs, calls by `bl`) | 3 / 3 | 3 / 3 |
| Veneers into RAM code (`ldr r3, =ram; bx r3`, ARM `ldr pc, [pc, #-4]`) | 3 / 2 | 3 / 2 |
| Through a function pointer (an argument, a pointer loaded from RAM) | 2 / 2 | 2 / 2 |
| Total | 71 / 46 | 27 / 20 |

The fixes (`analysis::vsa`): a sign extension is a derivation
(`DOp::Sext`) and a `lsl`/`lsr` pair by `32 - w` over a value that fits
gives it back, so the copy's bound narrows the original; a call or a
store through an unknown pointer forgets the frame only from the lowest
escaped local up, keeping saved registers, and a reloaded local keeps
its bound. No game's hard count rose; `jt-score`'s modern build went
from 584 to 610 of 617 (agbcc 676/676, 0 wrong). What's left is mostly
not tables: data decoded as code, and stubs and veneers a native build
could call through `FnRef`.

## What the Stage-A lifter should know

- Take holes from `analysis::holes::classify`; its `Report` lists each
  site. The IRQ wrapper and boot `msr`/`mrs` come back as `CpuWhere::Wrapper`
  and `Boot`: recognised, not holes.
- The run's call targets (`Run::call_targets`, `indirect_calls`) are the
  roots discovery lacks. The RAM copies are `analysis::holes::ram_copies`
  (each an `Overlay`, with the roots it was given), which needs a run; a
  lift of a game that copies code without a BIOS copy discovery folds
  needs that run too (the `[[copy]]` tables), or the same copies found
  statically.
- The flash library's stack routines are `Report::flash`: at each such
  call into RAM, call the `runtime::hw::flash` function its
  `Routine::call` names on `Hw::save`, with the routine's arguments
  (r0-r2) as its slices.
- Unreached unresolved jumps (most of Minish Cap's and Rogue's) are as
  likely dead as live until coverage reaches them.

## Limits

- One route of 90 seconds per game, from reset, no save file (the
  coverage runs add random walks, saves and Emerald's TAS states). Code
  the route doesn't reach shows only through static finds (Recovery's
  jumps, static return arithmetic, CPU-state instructions).
- The return-arithmetic rule is a heuristic over the run; its filters
  removed every false positive seen (a long-call veneer, a switch in the
  callee, a RAM switch jumping to its ROM original).
- The run's calls are lr_numbers' plus jumps through a register just
  after lr is set (`mov lr, pc; bx r0`; `adr lr, …; bx r0` in `IntrMain`).
- The busy-loop rule is a heuristic too: it misses a wait that stores
  each time round (a counter), or one the route doesn't reach.
- **Not measured here:** pointer arithmetic or casts the types don't
  explain (stage B measures it: it needs the typed lift), and behaviour
  that depends on cycle timing (the native-timing play build's to find).
