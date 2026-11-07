# Stage B of the native lifter: the plan

2026-11-01 on quack, after the native-ports sprint. `native-port-design.md`
said the typed lifter, function-pointer IDs (its section 4) and ROM
statics (section 8) would be filed only once the agb spike and Emerald's
ten had shown what's needed. They have. This is the plan, in order, with
the check for each piece. The units are in the tracker's `native` group;
each title below is a unit's.

This file is the plan, not a results log. A done piece gets one line:
what it changed and the unit whose evidence has the detail (`just show
<id> --full`); how it works is the module doc of the code named; current
values are in [native-stage-b.numbers.md](native-stage-b.numbers.md),
written by `just lift-metrics --write-doc docs/native-stage-b.numbers.md`.
What is still open and steers later units is under "Known gaps", at the
end. `docs/budgets.toml` holds this file to a line budget.

## Where stage A left us

`gbaa-lift` (`native-port-design.md`, "The stage-A lifter as built") lifts a
whole agb program to Rust over byte RAM, and it is exact: the native check
passes for 600 frames on three programs. But it is literal: about 74,500
lines for a 330-line program. Of the spike's 29 KB of code, the panic screen
(agb's renderer, `core::fmt`, and the `qrcodegen` crate that draws its QR
code) is about 23.6 KB, and the program's own code is 1.1 KB (`nm -S
native-spike.elf`). Stack frames are IWRAM bytes, pointers are guest
addresses, ROM data is read at run time, function pointers are ROM keys,
every function takes and returns all of r0-r3, and four hand-written
stand-ins in `holes/<program>.toml` cover agb 0.25 code that touches CPU
mode.

## How every piece is checked

1. **The native check stays green.** `just lift-check`: the programs of
   `crates/codegen/lift-check.toml` (and each program a unit adds) pass
   `route-check --native` against the `--timing native` play build for
   600 frames; the planted wrong store and the planted hard hole are
   still caught; each program lifted at the packed layout draws the same.
2. **A number moves, and a bars file holds it.** `gbaa-lift` writes its
   metrics (`metrics.toml` beside `report.txt`; the names and what each
   counts are `crates/codegen/src/native/metrics.rs`) and `--bars FILE`
   fails the lift when any metric is over its bar. The bars are maxima in
   `lift-check.toml`; a relation ("no four-register function without a
   summary hole") is a metric of its own with bar 0. A unit's check names
   the bar it lowers and to what; a fraction ("a quarter") is of the bar's
   value when the unit is claimed, since earlier units move the baseline.
   Bars are only lowered: raising one to make a check pass is changing the
   oracle, so it goes to june (`just ask`). The one exception is
   `lines_prelude`, the hand-written prelude, which may rise with the
   unit's evidence saying how many lines and why; `lines` is the rest and
   only goes down. How to read and lower bars: `docs/tools/commands.md`,
   `just lift-check` and `just lift-metrics`.
3. **The old behaviour is a planted control** wherever the stack is
   involved or the change could pass by not running: the native check
   skips the guest stack, so a wrong stack slot only shows where it reaches
   RAM or the picture. Each such unit keeps the old behaviour behind a flag
   (`planted_flags` in `lift-check.toml`) and shows that `lift-check`
   fails with it.

Each new behaviour gets a small agb program of ours (source in `agb/`,
`global_asm!` for the machine-level case; its module doc says what it
tests and what the old lift gets wrong) added to `lift-check`.

## The pieces, in order

### 0. The bars (shared by everything after)

**Lift report metrics and a bars file the lift check holds**
(`native/lift-bars`). Done: the metrics, `--bars`, `lift-check.toml` (one
table per program: ROM, route, flags, bars) and its Rust driver
`gbaa-lift-check`; a unit adds a program as a table. Each load and store
is counted by how its address is known: a constant in its block, by
region; sp-relative; or computed, by the one region the value-set
analysis bounds it to, else `computed_unknown`.

### 1. The shortcuts real games break

They hold for the agb programs and not in general, so they come before
any real game.

- **Fallthrough into another function keeps r4-r11 and flags**
  (`native/lift-fallthrough`). Done: every lifted function is
  `x_<key>(g, Regs)` (r0-r14 and NZCV) behind a one-line plain entry
  `f_<key>`; a fall, jump or call-return into another function's entry
  passes every register and flag (decision: an extended entry, not a copy
  of the blocks). Control `--fall-as-tail-call`; program
  `agb/native-fallthrough`. On the way, gap decoding stopped after a `bl`
  whose return site is a known literal pool, which dropped pool words
  decoded as code from every lift.
- **IRQs held while CPSR's I bit is set** (`native/lift-irq-hold`). Done:
  `runtime::hw::Native::irq_masked`, `hw::irq_release` and the prelude's
  `msr_cpsr_c`; waits wake but don't dispatch while I is set. The play
  build had the same bug the other way round at `--timing native` and was
  fixed with it (`Machine::tick_native`). Control `--irq-at-waits`;
  program `agb/native-irq-hold`. This is where `lines_prelude` split from
  `lines` (june's call).
- **A pending IRQ is taken the moment IME is set again**
  (`native/ime-release`). `IME` has the I bit's shape: agbcc games'
  critical sections clear `IME`, not I. Both builds took it at the next
  time point and agreed, so only mGBA could see it. Done: the play build
  acts on the write taking `IME` from 0 to 1 (`event::IME_ON`), and the
  recompiler at `--timing native` ends a block after a store that folds
  to `IME`; the lift's `hw::write_ime` runs what's pending, and the lift
  sets `g.sp = r13;` before every store that may reach I/O so the
  handler runs on the right stack. Control `--ime-at-waits`; program
  `agb/native-ime-release` (frame 601 matches mGBA).

### 2. Recognising the agb library

**Parked (june, 2026-11-01).** gbaa's target is games, and agb programs
are only our test inputs, so making their lifts small doesn't move the
project. The four units in this piece are parked, and RAM globals no longer
wait on them. If a real game needs library recognition (Metroid's sound
engine, say), file a unit for that game.

Of the spike's 111 lifted functions, two are the program's. The rest are
agb 0.25, `core`, `alloc`, `compiler_builtins`, `portable_atomic` and
`qrcodegen`, the same bytes in every agb program but for addresses.

The four, each card holding its check and bar: **library functions
recognised by code bytes** (`native/lib-signatures`; leave-one-out over
our agb builds, the agbcc games as the false-positive control), **the
four stand-ins written from recognised names** (`native/lib-standins`),
**library leaves as native code** (`native/lib-leaves`; the panic path,
`mem*` and division helpers; bar: the spike under 20,000 lines) and
**agb's runtime as hardware-library calls** (`native/lib-agb-runtime`;
bar: the spike under 8,000 lines).

Calls replace agb with hand-written Rust over `Hw`, never with the real
agb crate: agb targets the GBA (memory-mapped I/O, `asm!`) and doesn't
build for the host.

### 3. Function shape

- **Parameters and results from the summaries** (`native/lift-params`).
  Done (`crates/codegen/src/native/shape.rs`): a function whose summary
  and callees' runs have no hole is `f_<key>(g, sp, a0, a2) ->
  Result<u32, Exit>` (`()`, `u32` or a tuple), with a four-register
  adapter `a_<key>` for `FNS` and `ENTRY`; four-register functions
  without a summary hole are 0 (bar 0).
- **Function addresses followed to every use** (`native/fnref-flow`).
  Done (`crates/codegen/src/native/fnref.rs`, whose module doc is the
  rule and the closed world it rests on): a whole-program abstract run
  that reports, per function word, proven or why not, and each indirect
  call's callee set, complete or not. Nothing printed changes; metrics
  `fn_words_not_proven` (barred) and `fn_words_proven`.
  `GBAA_FNREF_TRACE=1` traces it. Proofs are weak on agb: see "Known
  gaps", the pile.
- **Function pointers as dense IDs** (`native/fnref-dense`). The proven
  words rewritten in the lifter's copy of ROM data, `FNS` dense from 1
  under a new `--fn-ids dense`; ROM keys stay the native check's (RAM holds
  the play build's numbers). Check: `lift-check` green with ROM keys, and
  each program's dense build draws the same 600 frames (`route-check
  --native … --packed`); bar: 0 ROM keys left in a dense build. Waits on
  the heap split by allocation site (`native/fnref-heap-sites`).
- **Indirect calls pass what their callees read**
  (`native/indirect-call-args`). Done: a call (not a jump by value)
  whose `native/fnref-flow` callee set is complete reads the union of
  its callees' parameters and takes back what any may change
  (`codegen::saves::summarize_sets`; the lift summarises twice, plainly
  for the frames, then with the sets). `call_fn` passes only those:
  `{ let o = call_fn(g, k, r13, [r0, r1, 0, 0])?; r0 = o[0]; }`; an
  incomplete set keeps all four. `native::callee_sets` gives
  `gbaa-recompile --summaries --callee-sets` the sets without a lift.
  Metrics `call_fn_complete` and `call_fn_four_complete`. The bar is
  the values measured (56-60 per program, the owner's call), not 0:
  every complete set is the pile's ~21 address-taken functions, whose
  summaries read all four between them; it falls when
  `native/fnref-heap-sites` narrows the sets. Emerald (656
  disagreements from indirect calls without sets) is re-measured in
  `native/indirect-call-args-emerald`, once the flow scales
  (`native/fnref-flow-scale`).

### 4. The stack

agb passes the addresses of stack values everywhere (`&mut self` on a
local), so locals need an escape analysis first.

- **Stack frames: which slots escape** (`native/frame-escape`). Done
  (`crates/codegen/src/native/frame.rs`): per function, its slots and
  whether and how each one's address escapes; the report's `stack
  frames` line and metrics `sp_accesses_local_slot` and
  `sp_accesses_escaping_slot` (summing to `load_sp` + `store_sp`).
- **Stack slots that don't escape as Rust locals**
  (`native/frame-locals`). Done: a slot that doesn't escape, lies below
  the entry sp and is only accessed aligned is a Rust local `l<offset>`
  (`Frame::locals`); escaping slots stay IWRAM bytes; slots at or above
  the entry sp are the caller's (`sp_accesses_callers_frame`).
  `sp_accesses_local_slot` is 0 (bar 0). Control `--plant-slot=ADDR`;
  program `agb/native-frame-locals`. `gbaa-lift --shadow-locals` keeps
  every local in the frame's bytes too and prints `shadow local:` where a
  load differs (how to run it: `docs/tools/commands.md`).

Escaping slots passed as `&mut` (the design's section 1 form 1) come with
typed pointers, below; until then they are soft holes, counted.

### 5. Typed data

- **ROM data as typed statics** (`native/rom-statics`). ROM objects read at
  constant addresses or through pool pointers become `static`s typed by the
  widths the code reads them at (`[u16; N]`), loads from them indexing. A
  pointer loaded from ROM data stays a `RomRef` (a soft hole, counted).
  Needs the dense IDs, so a function-pointer word in a table is an
  `FnRef`. Bar: the computed ROM loads at most a quarter of their bar
  (constant ROM loads are already 0: each was a literal pool's word, which
  the IR has as a constant, so `native/ram-globals` dropped them).
- **RAM globals as typed fields** (`native/ram-globals`). Done
  (`crates/codegen/src/native/globals.rs`): constant-address RAM accesses
  carved into globals, the fields of `Ram` at the guest's offsets under
  `--layout guest`, names from `--names DIR` where a names file has them;
  `load_const_ram` and `store_const_ram` (byte accesses) are 0.
- **Pointers into one object as typed access** (`native/ref-in-object`).
  Done (`Globals::object`): a computed RAM access whose value set stays in
  one global is that global's field (`pin`) or element (`ix`), checked at
  run time, so a wrong value set stops the program naming it. Computed
  RAM byte accesses fell to 0-1; the new soft hole `pointer_unexplained`
  counts the rest (crt0's `.bss` clear).
- **Heap pointers as `Ref<T>`** (`native/ref-heap`). What agb's allocator
  returns, typed by the accesses through it; the allocator can be replaced
  after this. Same kind of bar.
- **Lifted programs at the packed layout draw the same**
  (`native/lift-packed-layout`). Done: `gbaa-lift --layout packed`
  (`Globals::ram_struct`, `native/prelude_packed.rs`) orders `Ram`'s
  fields the lifter's way; a packed program records RAM as the guest lays
  it out (`Ram::image`) and `route-check --packed` compares it whole.
  Every program passes in `lift-check`.

Pointer arithmetic the types don't explain (section 6's last hard-hole
kind for stage B) is measured here for the first time.

## The first game

**Metroid: Zero Mission at stage A, then Emerald at stage B** (the
default of the question to june; see the tracker). The hole report
(`research/native-holes.md`; its current numbers are
[native-holes.numbers.md](research/native-holes.numbers.md)) ranked
Metroid closest when this was chosen. Emerald is the game june's
romhacks are built on and the one with types (pret's DWARF), but it has
15,158 functions, so a literal lift of it is the largest build there is.
Metroid first tests stage A on a real agbcc game at a size that builds,
and the shortcuts above at game scale.

- **Lifted crates split into modules that build**
  (`native/lift-modules`). Done (`codegen::native::modules`): `main.rs`
  (prelude, `Ram`, `ENTRY`), `fns.rs` and `m_<first key>.rs` modules of
  about 4,000 lines, so rustc's codegen runs on several cores. The
  synthetic million-line crate (`gbaa-lift --copies 14`) builds in under
  a minute and 4 GB.
- **Metroid lifts with no hard holes and builds**
  (`native/metroid-lifts`). Needs the discovery units, the shortcuts and
  the module split. The ROM's file name has spaces, so it lifts with
  `--name metroid`; its holes file is `mods/metroid/holes.toml` (private:
  its stand-ins port the game's own code). Where it stands: the stand-ins
  (the reset, the IRQ dispatcher, the classic NES Metroid the ROM carries)
  and five IWRAM copies (`[[copy]]`, `native/lift-ram-copies`) leave 9
  hard holes, all BIOS calls the prelude lacks (`native/lift-bios-calls`).
  The function-address flow is skipped (`skip_fn_flow`) until it settles
  on a whole game (`native/fnref-flow-scale`). A measurement build with
  the BIOS calls stood in by panics (2026-11-06, quack): about 957,000
  lines in 266 modules, the crate alone rebuilt in release in 31.5 s
  wall, peak 3.99 GB, as the synthetic crate predicted.
- **Metroid passes the native check on a route**
  (`native/metroid-native-check`). A route `units/metroid-native.toml`
  from boot into gameplay, compared every frame with sound
  (`native/hw-sound`). If the first run shows more than a session's worth
  of differences, the unit files one unit per cause and stops.

Emerald comes after typed RAM and pointers, typed from pret's DWARF (the
gaps Emerald's ten listed: fills, bitfields, retyping from access widths,
switches as `match`, explicit C arithmetic). It is filed when Metroid
passes, so that it is designed once.

## Order and what waits on what

Everything waits on the bars, since each check is "lift-check green with
a bar lowered". After that, three lines of work can go side by side:

- **Shortcuts, then the first game.** The fallthrough, IRQ-hold and IME
  fixes and the module split need nothing else. Metroid's lift waits on
  them, on the discovery and stand-in units the hole report filed, on the
  BIOS calls, and on june's answer; its native check also waits on sound
  in the hardware library, because the route compares audio.
- **Recognition** (parked). Signatures first; the stand-ins wait on them
  and on the IRQ-hold fix; the leaves wait on the stand-ins; agb's runtime
  waits on the leaves.
- **Shape and data.** Parameters first, after the fallthrough fix (both
  rewrite how calls and continuations print); following function
  addresses waits on parameters (the `FNS` adapters); dense IDs wait on
  that and on the heap by allocation site, and indirect-call arguments
  on dense IDs; ROM statics wait on the dense IDs (a function word in a
  table must already be an `FnRef`). The escape analysis needs only the
  bars; stack locals wait on it and on parameters (a function's signature
  decides what's in its frame). The packed layout waits on RAM globals;
  typed pointers wait on RAM globals and stack locals, since a pointer is
  typed by the object it points into; heap pointers wait on those.

Not filed, for stage C or later: the 8 jumps by value per program as
`match` (the structuring plan, unused so far), the functions whose
callee-saves aren't proven (soft: each function keeps its own r4-r11),
escaping stack slots as `&mut`, and replacing agb's allocator. Also not
filed: discovery's own fix for the gaps the lifter decodes (a `bl`'s
second half taken for a pool, a scan stopped short; `gap_targets`); file
it if a game needs it.

## Known gaps

What the done pieces left open, for the units that come next.

- **Falls and calls into an entry** (piece 1). A call that returns into
  another function's entry passes the callee's r0-r3 and the caller's
  other registers and flags as before the call; neither that path nor a
  `b` (rather than a fall) into an entry has a test program. The
  summaries don't count r4-r11 or flags a fall passes on as parameters,
  so `x_` stays whole for fall targets. A stand-in, and a jump through a
  register to a target no table names (`call_fn`), are still entered by
  the plain entry.
- **IME with I set** (piece 1). The play build drops `IME_ON` when I is
  set at the tick after the `IME` write, so "I set, `IME` on, `msr`
  clears I" waits for the next time point where the hardware library
  takes it at the `msr`; fixing only the play build would split it from
  the lift's IRQ wrapper. A store to `IME` through an address the
  recompiler can't fold still ticks at its block's end.
- **Shapes return too much** (piece 3). Many shaped functions return all
  four registers, because a caller whose run stopped (a fall into library
  code) calls them, and a jump by value the run didn't prove a return
  adds r0 to every function. `native/indirect-call-args` and the stack
  units narrow those as the summaries' holes close.
- **The pile** (piece 3, function addresses). agb keeps closures and
  trait objects on the heap, and every heap store goes to one pile, so a
  load through any heap pointer may give every function address stored
  there, and one such load used as a number marks them all: most words
  aren't proven, and each `call_fn` set, though complete, is every
  address-taken function (17 on the spike). Splitting the pile by the
  value-set bounds on each store proved nothing more (agb's heap stores
  aren't bounded) and was nine times slower; the heap by allocation site
  and field (`native/fnref-heap-sites`) is what dense IDs need.
- **Function-address flow, smaller gaps** (for `native/fnref-dense`).
  Proofs are per origin, and a pool load folded from a non-pc-relative
  address gets a code-constant origin, so dense needs per-site constant
  rewriting. Words no tracked load reaches ("never read") stay ROM keys.
  The widening (more than 8 numbers are unknown, more than 16 exact ROM
  pointers become one "from the lowest on") makes the first reason given
  per word move with the code. A load whose bounds start in ROM and end
  outside it, or have none, reads no ROM and flags nothing (the closed
  world); a ROM-bounded read wider than 256 KB reads nothing (flagged); a
  RAM-to-RAM `CpuSet` isn't followed; `ror` by an unknown amount keeps a
  function's form. On a whole game the run doesn't settle in minutes yet
  (`native/fnref-flow-scale`).
- **Escape analysis** (piece 4). An escaping address marks only the
  slots at and above it; walking down too would leave about three times
  the accesses in memory, so it stays open and `--shadow-locals` checks
  it at run time. A callee's stack-argument bytes count only its exact
  sp-relative reads, and an indirect call is assumed to call the function
  that reads most above its entry sp (54 bytes on the spike), which
  callee sets would narrow.
- **Typed RAM** (piece 5). Without a names file, accesses that only touch
  are separate globals (the agb programs' globals are all `d_<address>`
  scalars). The `ix` (range) form runs in no lift yet: every typed access
  on the agb programs is a single pinned address (agb's allocator state);
  ranges aren't weighed against each other when carving. Most computed
  accesses (`load_computed_unknown`) have no value set at all:
  parameters from callers the analysis can't see and the heap's
  pointers, `native/ref-heap`'s. Escaping stack slots and ROM statics
  aren't pointed into yet. Pointers are guest addresses in both layouts
  until `native/ref-heap`, and the packed prelude's `ld16`/`st32` go a
  byte at a time (a word may span two globals).
