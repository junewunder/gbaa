# LLM pilot: 20 Metroid functions as plain Rust

Unit `native/llm-pilot-metroid`, 2026-11-17 (with june, 6 functions) and
2026-11-18 (headless, 14) on quack. june's decision: LLM passes may
alternate with the algorithmic lift. Claude rewrote 20 functions of
Metroid: Zero Mission from their stage-A lift (named by the decomp's
symbols, `--names`) as plain typed Rust, each checked by the per-function
check (`just fn-check`, `docs/native-port-design.md`, "The per-function
check"). There is no proof, only tests (june): this reports per function
what its budgeted check said, and what the route check said with all of
them in place. The rewrites are game-derived and live in the private
`mods/metroid/llm-pilot.toml` (stand-ins with their budgets and digests)
and `llm-pilot-notes.md` (raw notes); nothing here quotes them.

## How

- **Which functions.** The check only sees functions the program calls
  before it stops: the no-input run reaches frame 2,998 (boot, the intro,
  the title screen, a demo room's load), then jumps to a hole. A first run
  with `--runs 1` over 66 candidates picked from the names file (all with
  the lift itself as the replacement) showed 51 called; the 20 were picked
  from those for a mix of kinds, preferring many calls (each call gives 8
  runs: the call as it came, then 7 seeded variants of its parameters and
  of the RAM it read).
- **From what.** The lift (`out/fn-check-metroid`, its `ref_x_KEY`) for
  each function's summary (what it reads, what callers read back) and for
  where it banks sp and idles; the disassembly (`gbaa-dis --names`) as the
  quicker read of the logic. Not the decomp's C.
- **Shape.** A stand-in is `(g, sp, [r0..r3]) -> [r0..r3]` over the
  prelude's `ld*`/`st*`, `bank_sp`, `hw::idle`, `call_fn`. It can't use the
  lift's global names (the name picker sees the identifier as taken and
  renames its own), so globals are addresses and direct calls go by key
  through `call_fn`.

## Results

All 20 pass their budgets together in one run (`just fn-check … --replace
mods/metroid/llm-pilot.toml` with all 20 keys: "every budget spent in
2996 frames", 5.2 s of checking), and the lift with all 20 in place runs
Metroid's route (see "The route check" below).

| # | Function | Kind | Bytes | Budget | Result | Took |
|---|---|---|---|---|---|---|
| 1 | AudioCommand_Volume | leaf, struct | 20 | 1000 | pass, first try | 3 min |
| 2 | AudioCommand_PitchBend | leaf, struct | 22 | 1000 | pass, first try | 2 min |
| 3 | DMA2IntrCode | I/O, branch | 92 | 1000 | pass, second try (bank_sp) | 8 min |
| 4 | TitleScreenVBlank | I/O, loop | 208 | 1000 | pass, first try | 6 min |
| 5 | SoundPlay | call | 16 | 800 | pass, first try | 3 min |
| 6 | FixedInverse | call (division) | 24 | 1000 | pass, second try (scratch r2) | 7 min |
| 7 | CallbackSetVblank | stores a function pointer | 28 | 80 | pass, first try | 1 min |
| 8 | CallbackCallVblank | call through a pointer, I/O | 76 | 1000 | pass, first try | 3 min |
| 9 | UpdateInput | leaf, bits | 56 | 1000 | pass, first try | 2 min |
| 10 | ResetFreeOam | loop | 52 | 1000 | pass, first try | 2 min |
| 11 | unk_1bf0 | linked-list loop | 40 | 1000 | pass, second try (rotated load) | 4 min |
| 12 | AudioCommand_PanPot | leaf, struct | 20 | 850 | pass, first try | 1 min |
| 13 | AudioCommand_KeyShift | leaf, struct | 24 | 850 | pass, first try | 1 min |
| 14 | GetNoteDelay | table lookup, arithmetic | 108 | 500 | pass, first try | 4 min |
| 15 | AudioCommand_Voice | struct, switch on a type, call | 190 | 900 | pass, third try (borrows, rotated loads) | 9 min |
| 16 | DoSoundAction | bit fields, loop, call, I/O | 252 | 800 | pass, first try | 7 min |
| 17 | DmaTransfer | loop, DMA poll | 208 | 24 | pass at 24, second try (r3) | 6 min |
| 18 | BitFill | loop, DMA poll | 204 | 24 | pass at 24, second try (r3) | 5 min |
| 19 | TitleScreenProcessOam | calls by flags, struct | 134 | 1000 | pass, first try | 4 min |
| 20 | IntroHandler | switch on a stage, call through a table | 202 | 1000 | pass, second try (borrows) | 7 min |

"Took" is reading, writing and fixing, not waiting on builds (a check
build is about 2 minutes; the 14 of the second session were written in one
pass and checked together, so their times are estimates from that pass).
Tokens weren't metered per function in either session. The 14 rewrites'
code is about 4,000 output tokens in all (by size: 80 to 650 each, the
switches and DMA helpers the largest); reading each function's lift and
disassembly cost about 1,000 to 10,000 input tokens (a lift is 50 to 500
lines; the disassembly is a tenth of that and was the main read). The
second session as a whole, scouting, 14 rewrites, four check builds, a
stuck run's diagnosis and this write-up, took about 1 h 45 min.

## What the check caught

Every failure was a way the rewrite differed from the lift on inputs the
check makes up; none was a wrong reading of what a function does.

- **Scratch registers.** Most functions are compared on all of r0-r3 on
  return: callers through pointers, or a summary hole, may read any. So a
  rewrite must leave the original's scratch values: the saved lr a `pop
  {r0}` leaves in r0 (the stand-ins return `RET` there), a constant
  address left in r1, the loop counter's last value. FixedInverse's
  natural `0x10000 / x` failed 775 of 1000 on r2, where the division
  helper leaves the quotient; it now calls the helper. DmaTransfer and
  BitFill first failed on r3: their epilogue restores r8 through r3
  (`pop {r3, r4, r5}` then `mov r8, r3`), so r3 is the caller's r8, which
  stage A's interface passes as 0. This was the biggest share of the work
  and the least readable part of the result.
- **Where sp is banked.** The lift calls `bank_sp` with the current sp
  before every store through a pointer or to I/O, and the banked sp is
  compared. A rewrite must bank the same sp before the same kind of store
  (DMA2IntrCode banked the entry sp on every path and failed all 1000).
- **Misaligned word loads.** The lift loads a word as the ARM does:
  aligned, rotated by the address's low bits. The prelude's `ld32` reads
  the four bytes as they lie. The game's pointers are aligned, but the
  check's variants change RAM bytes the function read, pointers included:
  unk_1bf0 differed in 54 of 1000 runs (the next channel in its list) and
  AudioCommand_Voice in 94 (its voice table entry). A local `ldr` helper
  (aligned load, rotated) fixed both. These differences are on inputs the
  game never gives; the check can't tell.
- **Polls.** DmaTransfer and BitFill wait on DMA's busy bit; the lift
  turns that loop into `hw::idle` and, on the way out, `hw::idle_exit`
  (which syncs time). A rewrite has to make the same calls in the same
  order, including no call at all when the first read finds the bit clear
  (DmaTransfer) or an exit sync even then (BitFill's first wait). Both
  matched on the first try because the lift was read for it.
- **Budgets.** A budget over what the run's calls give (8 per call, less
  the variants whose reference panics) fails as "short of its budget":
  PanPot, KeyShift and Voice had their budgets set from the run's counts.
  CallbackSetVblank has 10 calls before the stop, so 80.

## What the check can't do

- **A reference that never returns.** With budgets of 100 or 1000,
  DmaTransfer and BitFill hang the check: a variant's random size sets the
  control word's high bits (repeat, start timing), the DMA repeats, and the
  lift itself idles on the busy bit forever (a backtrace showed
  `ref_f_080032b5` in `hw::sync`). There is no cap inside a run, and the
  binary prints nothing until it ends, so the first sign was a run at
  100% CPU for 43 minutes. Both pass at 24 runs (the first three or four
  calls). Filed: The per-function check caps a run that never returns
  (`native/fn-check-run-cap`).
- **Functions past the stop.** Only what the no-input run reaches before
  frame 2,998 can be checked; gameplay code (Samus, sprites, rooms) can't
  until the lift's stop is fixed or the check takes input.
- **Misalignment, the other way round.** The check moves RAM pointers
  passed as parameters only by multiples of 4, so a rewrite that mishandles
  an odd pointer parameter isn't seen; only pointers read from RAM are
  ever odd.

## The route check

`route-check --native` on `units/metroid-native.toml` (2,700 frames: reset,
the intro and title skipped by Start, a new game, the first room of
Brinstar, with presses) with the lift built from `mods/metroid/holes.toml`
plus all 20 stand-ins (`gbaa-lift --layout guest --replace
mods/metroid/llm-pilot.toml`) against the `--timing native` play build:
pass, 2,700 frames with 0 differences in pictures, RAM, I/O, palette,
VRAM and OAM, and audio the same every frame (2026-11-18; the lift reported
all 20 as stand-ins and ran its 600 frames with no stop). So the route
check caught nothing the per-function checks had missed. It covers what
those can't: the route plays on into gameplay with presses, past the
no-input run's stop, so every one of the 20 ran there in states the
check never made, and the two DMA helpers ran with sizes their 24-run
budget never saw.

## What it says about LLM passes

- Reading the logic was never the problem: every function's behaviour was
  right the first time. What failed was stage A's calling convention
  (scratch registers callers may read, the banked sp, r8 through r3) and
  the ARM's load semantics, all things the lift carries for free. A rewrite
  for stage A is half reading the program and half matching the lift's
  register traffic; the second half goes away once summaries narrow what
  callers read and stage B drops the guest stack.
- The per-function check is cheap (0.1-0.3 s per 1,000 runs) and caught
  every such difference at once, with the byte or register named. Its
  budget is bounded by how often the no-input run calls the function, and
  a reference that can run forever needs a cap.
