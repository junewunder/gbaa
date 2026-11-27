# LLM pilot: 20 Metroid functions as plain Rust

Round two, 20 Amazing Mirror functions on today's lift, is at the end
("Round two").

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
  calls). Fixed by The per-function check caps a run that never returns
  (`native/fn-check-run-cap`, 2026-11-18): a run stops past 600 frames and
  a capped reference is skipped and counted; at 1000 runs each, DmaTransfer
  passes with 7 capped (59 s) and BitFill with 24 (10 s).
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

## The rewrites' repeated transformations (`native/llm-pilot-patterns`)

What Claude did to more than one of the 20, compared with the lift each
was read from, and whether the lifter could do it with a proof (exact on
every input, not just the check's). Functions are named with their keys
(Thumb, so odd). "Lifter unit" is the unit filed for the pattern, or the
open unit that already covers it.

| Pattern | In | Provable? | Lifter unit |
|---|---|---|---|
| Flag and register writes nothing reads dropped | all 20 | yes | done: `native/lift-dead-writes` |
| `pop {pc}` / `pop {r0}; bx r0` of the entry lr as a plain return | 15 | yes | done: `native/lift-pop-return` |
| Branch conditions as comparisons and bit tests | 12 | yes | new: `native/lift-branch-conditions` |
| Compiler narrowing idioms as casts and masks | 6 | yes | new: `native/lift-narrowing-casts` |
| Single-use temporaries inline, constants at their use | all 20 | yes | done: `native/lift-inline-temps` |
| One `bank_sp` per run of stores at the same sp | 10 | yes, with care | done: `native/lift-bank-sp-once` |
| A counted loop as `for` over a range | 2 | yes | done: `native/lift-for-loops` (a constant start only: DoSoundAction's, not ResetFreeOam's) |
| A compare tree on one value as `match` | 3 | yes | done: `native/lift-switch-match` |
| `if`/`while`/early return in place of the block match | 12 | yes | `native/lift-structured` (in progress) |
| Table and array indexing (`base + i * size`) | 5 | yes | `native/rom-statics` (ROM), done `native/ref-in-object` (RAM) |
| Names for locals, parameters and constants | all 20 | no | none (names files; the mod view's local naming) |
| Scratch registers kept in the results | all 20 | no: forced | none (stage A; `native/fnref-live-registers` narrows) |
| A misaligned word load's rotate dropped | 3 | no | none |
| An unrolled sequence rolled into a loop | 1 | n/a | none (one function) |

Not transformations: the DMA polls (`hw::idle`/`idle_exit`) and the key
normalising of a pointer call were copied from the lift as they were.

**Flag and register writes nothing reads.** Stage A writes N, Z, C and V
at every flag-setting instruction and prints each `add_with_carry` up to
three times for its three results; the rewrites keep no flag at all.
UpdateInput (0x08000969) writes all four and reads none; ResetFreeOam
(0x08000a19) and FixedInverse (0x08000a81) the same. A callee-saved
register's restore before the return (`r4 = l8`) is dead too: the lift
returns r0-r3 only. Liveness inside a function proves it: flags are read
only by a later condition in the function or by a fall into another
function's extended entry (`native/lift-fallthrough`), and the summaries
say which registers a call or return passes on.

**The entry lr's return.** Every function that pushes lr ends by
`if t & !3 == RET { return } else { call_fn(t) }` on the popped word.
`native/frame-locals` already holds that slot in a Rust local written
only from r14 at entry, and stage A's call passes `RET` there, so the
`call_fn` arm can't run and the rewrites return plainly (with `RET` in
the register the pop went through: r0 in DMA2IntrCode 0x08003381,
SoundPlay 0x08002a19, CallbackSetVblank 0x08000ae5; r1 in FixedInverse
0x08000a81, GetNoteDelay 0x080020d5 and IntroHandler 0x0808117d).
The lifter now prints it so (`native/lift-pop-return`): the summaries'
run proves the jump's target the entry lr (a pop of the word the
prologue pushed, or lr itself), and `lr_entries` that every way into the
function passes `RET` as its lr (a call does; a fall or tail jump passes
the jumper's, proven its own entry lr). Metroid: 2,610 such returns plain,
1 still through `call_fn` (`entry_lr_returns_call_fn`, barred); the agb
programs' few are falls after a call to a panic whose summary may return.

**Branch conditions.** `cmp r1, #128` then `bgt` is the lift's
`add_with_carry(r1, -128, true)` and `!(z | (n ^ v))`; the rewrite says
`first > 127` (ResetFreeOam 0x08000a19). Each ARM condition after a
`cmp`/`cmn`/`subs` has an exact closed form over the operands (EQ `a ==
b`, HI `a > b`, GT `a as i32 > b as i32`, and so on), and after
`ands`/`tst` or a shift that sets flags it is a bit test (`ands r0, r4;
cmp r0, #0; beq` is `x & 0x80 == 0`, DoSoundAction 0x080027f9; N after
`lsls #k` is bit 31-k). Seen in unk_1bf0 0x08001bf1, CallbackSetVblank
0x08000ae5, CallbackCallVblank 0x08000a99, DMA2IntrCode 0x08003381,
DoSoundAction 0x080027f9, AudioCommand_Voice 0x080022cd, DmaTransfer
0x080031e5, BitFill 0x080032b5, TitleScreenProcessOam 0x08076d91,
GetNoteDelay 0x080020d5,
IntroHandler 0x0808117d. Done (`crates/codegen/src/native/cond.rs`):
every condition with such a form prints as it; the rest are counted in
`branches_on_raw_flags` (barred; 0 on the C programs).

**Narrowing idioms.** agbcc narrows with shift pairs and masks:
`lsls #16; asrs #16` is `as i16 as i32` (FixedInverse 0x08000a81, both
ways round its division), `lsls #24; asrs #24` the s8 loop counter of
DoSoundAction 0x080027f9, `lsls #24; lsrs #24` or `ands` with a register
holding 0xff a `u8` (unk_1bf0 0x08001bf1's countdown, DMA2IntrCode
0x08003381's frame counter, GetNoteDelay 0x080020d5's parameters,
IntroHandler 0x0808117d's test of a callee's `u8` result). The shift
pairs are bit identities; the register mask needs the register's
constant carried into the block (the mod view's `lower::known` does this
for the play build).
The lifter now prints them so (`native/lift-narrowing-casts`,
`native::narrow`): a pair as `x as u8 as u32`, `as i8`, `as u16`, `as
i16` (other widths a mask or `prim::sext`), an `and` with 0xff or 0xffff
as a cast, the register's constant carried in by `lower::known` over the
native printer's edges. Metroid: 3,183 guest pairs, none left as two
shifts (`shift_pairs_left`, barred; a pair on a ROM literal folds to its
constant), 3,319 casts.

**Single-use temporaries.** Every lifted value is its own `let vN`, and
constants are their own lines (`let v5 = 0xfffffff8u32; let v6 =
v2.wrapping_add(v5);`). A pure value used once can print inside its use;
the rewrites are 5 to 20 times shorter mostly from this and the flags.

**`bank_sp` once.** The lift banks sp before every store that may reach
I/O or go through a pointer, so ResetFreeOam's loop banks three times an
iteration and DMA2IntrCode before each of its six I/O stores. The rewrites bank once per
run of stores at one sp (AudioCommand_Volume 0x0800238d, PitchBend
0x080023b5, PanPot 0x080023a1, KeyShift 0x080022b5, ResetFreeOam
0x08000a19, unk_1bf0 0x08001bf1, DMA2IntrCode 0x08003381,
TitleScreenVBlank 0x080776fd, DmaTransfer 0x080031e5, BitFill
0x080032b5). A bank is redundant when the last bank on every path to it
had the same value and nothing between can bank another (a call, an
idle, an IME release whose handler runs); the check compares the banked
sp, so this needs that proof, not just the check passing. Where the
rewrites moved the bank earlier than the first store (the four
AudioCommands bank at entry), the lifter shouldn't follow: that only
passes because nothing before the store banks differently.
Done (native/lift-bank-sp-once): the lift leaves a store's bank out where
the bank holds its `r13` on every way in (a must-analysis over the
function's blocks: a write of `r13`, a `msr`, a call, a `swi` or a wait
ends the run), and `irq_entry` puts the stopped mode's sp back after a
handler, so a store that runs one doesn't. Metroid 6588 calls to 2484.

**Counted loops.** ResetFreeOam 0x08000a19 (`for` from the free slot to
127) and DoSoundAction 0x080027f9 (an s8 count from 11 down to 0). The
mod view's `lower::For` (`mod/readable-for-loops`) recognises exactly
this on the play build's structured form; the native lift should reuse
it once `native/lift-structured` lands, not design it again. Done
(`native/lift-for-loops`, `codegen::native::counted`): the same proof,
a one-block loop run over its range from a constant start, shares
`lower::counted`; DoSoundAction's countdown prints as `for r0 in
(0..=11).rev()`, ResetFreeOam's loop, whose start is a value read from
RAM, stays a `loop` (no proof of its range without one). Metroid: 95 of
696 loops; the C programs: their startup's clear of IWRAM. Then
`native/lift-for-bounds` (`codegen::native::bounds`): loops of several
blocks, to a parameter's bound, counting down, proven symbolically from
the guard before the loop; Metroid 195 of 694 (2026-11-23). A bound read
from RAM each round (a global's, a field's) stays a `loop`: nothing yet
shows the body's calls leave it.

**Compare trees as `match`.** agbcc compiles a small switch as a
binary search of `cmp`/`beq`/`bgt` on one register (IntroHandler
0x0808117d's stage 0-3); AudioCommand_Voice 0x080022cd's PSG type 0-4
and DoSoundAction 0x080027f9's mix setting are the same shape. Where
every compare in the tree is of one unchanged value against constants,
the tree is a `match` with the leaves as arms.
The lifter now prints it so (`native/lift-switch-match`,
`codegen::native::switch`): a block's run of guarded exits, each on the
flags of one compare of the same value against a constant (its truth
table over N, Z, C and V), with nothing between them but register and
flag writes, is one `match` on the value; the blocks its edges bring
inline that go on comparing the register holding it join the tree, and a
run that ends at its last test leaves the rest of its block as the last
arm. Each leaf's values come from evaluating the tree on every stretch of
values between the compares' break points. IntroHandler is
`match { 1, 2, 3, 4..=0x7fffffff, 0, _ }`; AudioCommand_Voice's PSG type
switch is a `match` from its second test on (its first block's edge
brings in more than the one block); DoSoundAction's mix setting is one.
Metroid: 761 trees printed as a `match` (671 blocks taken in), 39 blocks
with two or more such compares left as `if`s (`compare_trees_left`,
barred).

**What isn't provable.** Names are guesses checked by nothing but
reading. The kept scratch registers are the price of stage A's interface
(every caller may read r0-r3), and go away with narrower results, not a
rewrite. Dropping a word load's rotate (AudioCommand_Volume, unk_1bf0,
AudioCommand_Voice) changed the result on odd pointers read from RAM: the
check caught two of the three, and the lifter can drop the rotate only
where a value set proves the address aligned, which a pointer read from
RAM never is. Rolling TitleScreenVBlank's eight unrolled scroll copies
into a loop is one function, so not filed.

## Round two: 20 Amazing Mirror functions (`native/llm-pilot-mirror`)

2026-11-24, headless on quack, main at 4ff1b2c1. The same pilot on Kirby &
the Amazing Mirror, the game the owner wants to mod, from today's lift
(katam's names and types: typed field access, `// file:line` comments,
narrowing casts, plain returns). The rewrites are in the private
`mods/kirby-mirror/llm-pilot.toml`, the route's presses in
`llm-pilot.input` beside it; nothing here quotes them.

### How

- **Reaching gameplay.** The per-function check runs with no input, and
  Mirror's no-input run never reaches Kirby's controller. `just fn-check`
  now takes `--input PRESSES` (gbaa-desktop's presses, passed to the
  program), and the check ran along the native route's presses
  (`units/kirby-mirror-native.toml`: Start and A into the first room, then
  right with jumps) for 3,200 frames. A scouting run with `--runs 1` over 46
  candidates from katam's map and `names/kirby-amazing-mirror.toml` found
  36 called; the link code past `MultiSioInit` (main, send, receive, the
  link screen) and the pause menu are not reached on the route.
- **From what.** The lift (`out/fn-check-kirby-mirror`, each `ref_f_KEY`)
  only. Unlike round one, the disassembly wasn't needed: the lift now reads
  as C with registers (typed fields, line numbers, conditions as
  comparisons), 15 to 220 lines a function.
- **Shape.** As round one: `(g, sp, [r0..r3]) -> [r0..r3]`. Typed access
  works in a stand-in (`Ref::<t::Kirby>::new(a[0])`, `k.base().xspeed()`),
  so the rewrites are typed where the lift is. Globals and calls are still
  by address and key (`call_fn`): a lift name in a stand-in's text, even
  in its `why`, makes the name picker rename the lift's (the report says
  `TaskGetNextSlot_`). A pointer read from RAM may be odd in the check's
  variants, so loads through one rotate as the ARM's (a nested `rd`
  helper); a pointer parameter is moved only by multiples of 4, so loads
  through it are plain.
- **The interpreter.** Run with `--interp-every 0`: on this lift the
  interpreter disagrees with the lift itself in 12 of the 46 scouted
  functions, every time on r0 after a `pop {r0}; bx r0` return in a
  function whose summary has a hole (lift: r0 as it came; interpreter: the
  return address). That is the check's or the lift's, not a rewrite's:
  filed as `native/fn-check-interp-pop-r0`.

### Results

All 20 pass together (`just fn-check … --replace
mods/kirby-mirror/llm-pilot.toml --fn-check` all 20 `--frames 3200
--interp-every 0 --input …`: "every budget spent in 2954 frames", 9 s of
checking), each digest recorded. Every one passed on the first build that
compiled; the only failures were three borrow errors (a `g` load inside
another call's arguments), fixed in a minute.

| # | Function | Kind | Bytes | Budget | Result |
|---|---|---|---|---|---|
| 1 | ObjectSetBounds | leaf, stack argument | 28 | 500 | pass |
| 2 | ObjectSetHitbox | leaf, abs and halves | 86 | 500 | pass |
| 3 | AnimCmd_JumpBack | leaf, sprite command | 12 | 250 | pass |
| 4 | TaskGetNextSlot | task pool | 38 | 13 (2 calls) | pass |
| 5 | IwramMalloc | loop, first-fit heap | 104 | 500 | pass |
| 6 | TasksExec | task loop, pointer calls, spin | 228 | 1000 | pass |
| 7 | TaskCreate | task, allocators, list insert | 314 | 500 | pass |
| 8 | TaskDestroy | task, destructor call, unlink, free | 184 | 230 | pass |
| 9 | GetInput | input, key-repeat loop | 252 | 1000 | pass |
| 10 | VramMalloc | nested loops | 184 | 500 | pass |
| 11 | sub_08141E74 | main menu's task (state call) | 76 | 1000 | pass |
| 12 | TitleScreenMain | menu state, switch on a state | 168 | 1000 | pass |
| 13 | MultiSioInit | link code, I/O | 248 | 3 (1 call) | pass |
| 14 | KirbyReadInput | controller: input by player | 328 | 1000 | pass |
| 15 | KirbyEnterStand | controller: state entry | 240 | 170 | pass |
| 16 | KirbyEnterWalk | controller: state entry | 196 | 110 | pass |
| 17 | KirbyEnterFall | controller: switch on animation | 300 | 60 | pass |
| 18 | KirbyGroundFriction | controller: compare tree | 204 | 1000 | pass |
| 19 | KirbyGroundAccel | controller: speed | 164 | 1000 | pass |
| 20 | sub_08154FE8 | arithmetic: angle of a vector | 190 | 1000 | pass |

Budgets are what the route's calls give (8 runs a call less those whose
reference panicked: up to 81% for `TasksExec`, whose variants scramble
task pointers). Three are small: `TaskGetNextSlot` (2 calls, 13 runs),
`MultiSioInit` (1 call, 3 runs) and `TaskDestroy` (230 of 500).

Time: scouting 10 min (one check build), reading the 20 lifts 15 min,
writing all 20 in one pass 15 min, fixes and the second build 5 min; the
route check and this write-up the rest, about 1 h 30 min in all. Tokens
weren't metered: the lifts read are about 2,000 lines (some 30,000 input
tokens), the rewrites about 900 lines of TOML (some 10,000 output
tokens), 2 to 8 times shorter than their lifts.

**The route check.** The lift with all 20 in place (`just lift … --name
kirby-mirror --layout guest --replace mods/kirby-mirror/llm-pilot.toml`:
20 stand-ins, 600 of 600 frames, no stop) against the `--timing native`
play build, `route-check --native` on `units/kirby-mirror-native.toml`:
pass, 3,000 frames, 0 differences in pictures, RAM, I/O, palette, VRAM
and OAM, audio the same.

**A planted check.** Two deliberate faults in a copy: "fixing"
KirbyGroundFriction so a slowing from the left also stops at 0 was caught
(1 run in 1,000, the EWRAM byte of his speed named); an off-by-four in
IwramMalloc's split test (`free > size` for `free >= size + 4`) was not, in
500 runs: block sizes are multiples of 4, so only a free size the check's
random bytes never made (one to three over) tells them apart.

### What round two says

- **The lift is now the thing to read.** Round one read the disassembly;
  this round the lift alone was enough, and the rewrites came out right
  the first time, register traffic included. What made that possible is
  round one's filed units (plain returns, comparisons, casts, temporaries
  inline, one bank a run, `for`, `match`) and the typed names.
- **A quirk kept is a quirk found.** KirbyGroundFriction stops a slowing
  speed at 0 from the right only: from the left the sign test compares
  0x8000 with 0xffff8000 and never matches, so friction can push him
  through 0 to the right. The rewrite keeps it (the check would catch a
  fix, as above) and says so in its `why`.
- **Still the cost of stage A.** Every rewrite carries the scratch
  registers its callers may read, the saved r8 that comes back in r3
  (TaskCreate, GetInput, VramMalloc, TasksExec), the exact sp it banks and
  passes, and the stack word a callee takes as its fifth argument. As in
  round one, that is half the work and none of the meaning.
- **The check's reach.** Input widens it to gameplay, but only what one
  route calls; the link code past its init and the pause menu stay out of
  reach, and functions called once or twice get a budget of a few runs.
  Random variants miss faults that need structured inputs (the
  off-by-four).

### The rewrites' repeated transformations

Over the 20, against the lift each was read from: what was done to more
than one, whether an algorithm could prove it (exact on every input), and
the unit. Left out: what `native/mirror-machine-count` counted and filed
(`vN` names, `pc(…)`, rotates, adds of zero, casts, `Result`), and round
one's, which the lift now does. "In" counts rewrites; "lift" counts
occurrences in Mirror's whole lift (`src/m_*.rs`, at 4ff1b2c1 with the 20
stand-ins).

| Pattern | In | Lift | Provable? | Unit |
|---|---|---|---|---|
| A field's address held in a register (`Ref::<T>::to(r, off)`) as its base's field | 8 | 12,035 | yes | `native/lift-field-base` (done: 14,153 to 2,157 at 341e0997, 11,996 field bases; the rest mostly a pointer loaded from memory into an object, a base register reused (as a loop counter), sums with a register index, and blocks after `call_fn`/`CallVia`, which kill every fact) |
| A value chosen in each arm (`r3 = 0x14c; if … { r3 = 0x1ac; break 'b2 }`) as an `if`/`match` value; a break to the function's end as `return` | 13 (10 values, 4 returns) | 7,177 labelled blocks | yes | `native/lift-arm-values` (done: 7,195 to 5,433 on fd0c09f6's lift with the holes file, 34 breaks to returns and 1,728 blocks' code an `if`'s other arm; left by where the first break is: deeper in an `if`'s arm 3,320, in another labelled block 1,734, in a `match` arm 364, beside a match's mark 15. Metroid 3,306 to 2,274. A value chosen in each arm stays a register written in each; the arms now join at the `if`) |
| A chain of `&` with constant masks as one mask (`x & !0x0c001f72`) | 3 | 386 | yes | new: `native/lift-mask-fold` |
| The task struct lookup (katam's `TaskGetStructPtr`: `flags & 0x10`, IWRAM bytes or EWRAM words) as one helper | 4 | 1,070 | yes (the same expression); its name from the names files | new: `native/lift-task-struct` |
| A call to libgcc's division (`__divsi3`) as `/` | 1 (and Metroid's FixedInverse) | 218 | yes, where only the quotient is read | `native/lift-libgcc-div` (done: 207 calls to 34 on 47ad536a's lift with the holes file, 173 printed as `/` or `%`; the rest have r2 or r3 live after them, read by a four-register function's return or a `call_fn`; modern GCC's routines wait on `native/summary-paired-guards`) |
| A pointer parameter bound once as `Ref<T>` | 10 | | yes | `native/lift-pointer-types` (ready) |
| Rotates dropped for a pointer parameter (always aligned in the game) | 7 | | only from callers' facts or the type's alignment | noted on `native/lift-aligned-loads` |
| Rotates dropped for a stack argument (`sp` is always aligned) | 2 | | yes | noted on `native/lift-aligned-loads` |
| The same bounds-and-hitbox sequence as one nested helper | 3 | | n/a (copied code in the source) | none |
| Scratch registers, saved r8 in r3, exact sp | 20 | | no: forced | none (stage A) |

**Field bases.** agbcc keeps `&kirby->unk56` or `&kirby->xspeed` in a
register and uses it for a load, a store and a reload; the lift types the
access by subtracting the offset back (`Ref::<t::Kirby>::to(r3,
0x56).base().unk56()`) from a register whose value it built as `kirby +
0x56` a few lines up. The rewrites name the field on the base (8 of 20:
KirbyReadInput, Enter{Stand,Walk,Fall}, GroundFriction, ObjectSetBounds,
ObjectSetHitbox, MultiSioInit). Where the base is still live under a name,
printing it is exact.

**Arm values and returns.** Of the 12 rewrites whose lifts have labelled
blocks, none kept one. A register set differently in each arm and read
after the join became `let x = if … { … } else { … }` or a `match` in 10
rewrites (7 of those 12: KirbyGroundAccel's top speed from two flags,
GroundFriction's friction by animation, KirbyReadInput's source of keys,
TasksExec's which-tasks-run, TaskCreate's struct address, TitleScreenMain's
next state, the angle's numerator and denominator; and 3 whose lifts used
plain `if`s: the task struct pointer in sub_08141E74, TaskGetNextSlot's
task, ObjectSetHitbox's half sizes). A break to code that only returns
became `return` in 4 (IwramMalloc, VramMalloc, TaskDestroy,
KirbyGroundAccel); the other 2 became plain `if`/`else` or a `match`
(KirbyEnterStand, GetInput). Both are exact where each arm only computes
the value, or the label's tail is the epilogue.

**Mask chains.** Clearing flags bit by bit compiles to one `and` each, and
the lift folds none: KirbyEnterStand's four masks are one,
`!0x0c001f72`. Constant folding.

**The task struct lookup.** Every task function starts with the same two
arms (katam's `TaskGetStructPtr`): 1,070 times in the lift. Recognising the
expression is exact; a name needs the names files (a decomp macro has no
symbol).

**Division.** `__divsi3` returns the quotient in r0 and its scratch in
r1-r3, so a rewrite as `/` is exact only where nothing reads r1-r3 after
the call: round one's FixedInverse failed on r2 for that reason, round
two's angle function passed because its callers read only r0 and r3.
Needs `native/lift-returns-read`. Done by `native/lift-libgcc-div`: the
callee known by its names-file symbol, its call reads r0 and r1 alone
where the result printed is the only one live after it (the routine
passes r2 and r3 back on its divide-by-0 path, so each call seemed to
read them), and a divisor that may be 0 keeps the call on that path.

Tooling found on the way, filed: the interpreter cross-check's r0
(`native/fn-check-interp-pop-r0`) and stand-ins naming the lift's globals
and functions (`native/standin-lift-names`).
