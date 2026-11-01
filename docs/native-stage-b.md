# Stage B of the native lifter: the plan

2026-11-01 on quack, after the native-ports sprint. `native-port-design.md`
said the typed lifter, function-pointer IDs (its section 4) and ROM
statics (section 8) would be filed only once the agb spike and Emerald's
ten had shown what's needed. They have. This is the plan, in order, with
the check for each piece. The units are in the tracker's `native` group;
each title below is a unit's.

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

1. **The native check stays green.** `just lift-check`: the three
   programs (and each program a unit adds) pass `route-check --native`
   against the `--timing native` play build for 600 frames; the planted
   wrong store and the planted hard hole are still caught.
2. **A number moves, and a bars file holds it.** `gbaa-lift` writes its
   metrics into `report.txt` (lines; functions lifted, recognised,
   replaced; each soft-hole family split by region: RAM, I/O, ROM, the
   display memories, sp-relative) and `--bars FILE` fails the lift when
   any metric is over its bar. The bars are maxima in one committed TOML,
   at today's values to start; a relation ("no four-register function
   without a summary hole") is a metric of its own with bar 0. A unit's
   check names the bar it lowers and to what; a fraction ("a quarter")
   is of the bar's value when the unit is claimed, since earlier units
   move the baseline. Bars are only lowered: raising one to make a check pass is
   changing the oracle, so it goes to june (`just ask`).
3. **The old behaviour is a planted control** wherever the stack is
   involved or the change could pass by not running: the native check
   skips the guest stack, so a wrong stack slot only shows where it reaches
   RAM or the picture. Each such unit keeps the old behaviour behind a flag
   and shows that `lift-check` fails with it.

## The pieces, in order

### 0. The bars (shared by everything after)

**Lift report metrics and a bars file the lift check holds**
(`native/lift-bars`). The metrics as `name value` lines, `--bars`, the file
at today's values, and `lift-check` using it. The program list moves from
the recipe into the same TOML, read by a Rust driver, so a unit adds a
program as a table rather than bash. Check: `lift-check` passes from the
TOML, and a test lowers one bar below today's value and sees the lift
fail, naming the metric.

As built (2026-11-01): the bars file is `crates/codegen/lift-check.toml`,
the driver `gbaa-lift-check`, the metric names
`crates/codegen/src/native/metrics.rs`; how to read and lower them is
`docs/tools/commands.md`, `just lift-check`. Each load and store is
counted by how its address is known: a constant in its block, by region;
sp-relative; or computed, by the one region the value-set analysis
bounds it to (it now records every memory instruction's addresses,
`analysis::vsa::func::Access`), else `computed_unknown`. Starting values
(native-spike, affine, mosaic-modes): lines 74,513, 74,262, 74,432;
functions 111, 110, 103, of them with a summary hole 78, 79, 77 and
four-register without one 29, 27, 22; sp-relative loads 2,177, 2,232,
1,202 and stores 1,351, 1,377, 1,015; computed loads no region bounds
812, 755, 1,766 and stores 415, 417, 815; constant ROM loads 479, 467,
556; constant RAM loads 34, 27, 27 and stores 9, 5, 5; `call_fn` calls
65, 61, 61; jumps by value 8 each; gap targets 22, 5, 143.
mosaic-modes' few sp-relative and many unknown accesses are its 143 gap
targets: code discovery didn't reach, so the value-set analysis never
ran on it.

### 1. The two shortcuts real games break

Both hold for the agb programs and not in general, so they come before any
real game. Each gets a small agb program of ours (source in `agb/`,
`global_asm!` for the machine-level case) added to `lift-check`.

- **Fallthrough into another function keeps r4-r11 and flags**
  (`native/lift-fallthrough`). Today a function whose last block falls (or
  jumps) into another function's entry prints a tail call, and the
  continuation starts with r4-r11 and NZCV zeroed. The fix: the
  continuation gets the caller's registers, by an extended entry or by
  copying its blocks into the caller (the unit's choice). Check: the new
  program passes the native check, and with the old tail call (a flag) it
  fails.

  As built (2026-11-01): **an extended entry.** Every lifted function is
  `x_<key>(g, Regs)`, taking r0-r14 and NZCV, and `f_<key>(g, sp, a)` is a
  one-line plain entry that calls it with `Regs::call(sp, a)` (lr `RET`,
  the rest zero). A fall or jump into another function's entry is
  `return x_<key>(g, Regs { … })` with every register and flag as they
  are; so is a call whose return site is another function's entry, which
  before was taken never to return. Copying the continuation's blocks
  into each caller was the other way: it duplicates code (agb's own
  programs have 13 or so falls each, into library functions), and a fall
  into a function that loops or falls on again would copy a chain. The
  extended entry costs one line per function, and the header it replaces
  was five lines, so the lifts got shorter. A stand-in (hand-written, with
  only the plain entry) is still entered by a tail call; so is a jump
  through a register to a target no table names (`call_fn`, plain entry).
  A call that returns into another function's entry passes the callee's
  r0-r3 and the caller's other registers and flags as they were before the
  call (the callee's flags and r12 are its own Rust locals); AAPCS keeps
  neither across a call, and before it panicked. Neither that path nor a
  `b` (rather than a fall) into an entry has a test program yet, and the
  summaries don't count r4-r11 or flags a fall passes on as parameters (a
  stage-B parameter unit must keep `x_` whole for fall targets).
  `gbaa-lift --fall-as-tail-call` prints the old tail call; lift-check's
  `planted_flags` runs it as a control. The report counts the falls.

  The test program, `agb/native-fallthrough`: `ft_a` pushes r4 and lr,
  sets r4 and the carry and runs into `ft_b`, whose result reads both and
  which pops the frame `ft_a` pushed (a shared epilogue: that's the only
  shape in which a function can hand r4 to the next and still keep r4 for
  its caller, so the summaries' callee-saves proof holds). `ft_c` calls
  `ft_b` through a function pointer (`bl` to a `bx r3` stub), which makes
  it a function to discovery: a plain `bl` to code that pops its caller's
  frame is a far jump by discovery's rule, and `ft_b` would be copied into
  both. With the old tail call `ft_a`'s pixels and `RESULTS` are wrong
  from frame 0.

  Found on the way: the lifter's decoding of discovery's gaps ran past a
  `bl` that never returns (agb's qrcodegen: a panic call, a `nop`, then a
  literal pool) and took the pool for code, which became an undefined
  instruction (a hard hole) in the new program's layout. `force_code` now
  stops after a `bl` whose return site, past `nop`s, is data discovery
  knows. That drops pool words decoded as code from all the lifts:
  mosaic-modes' gap targets fell from 143 to 24 and its lines from 74,432
  to 51,309, and the bars of the three programs came down with it
  (lines, blocks, ROM loads and unknown accesses).
- **IRQs held while CPSR's I bit is set** (`native/lift-irq-hold`). Today
  `Game::cpsr` is bookkeeping: an IRQ due at a wait inside a critical
  section runs at once, where the hardware runs it when I is cleared. The
  fix: waits wake but don't dispatch while I is set; writing CPSR with I
  clear dispatches what's pending. Check: the same pattern.

  As built (2026-11-01): `runtime::hw::Native` gains `irq_masked()`, the
  CPSR's I bit (`false` in the hand-written programs, which have no
  CPSR). While it is set the library's dispatch runs nothing and the
  requests wait in `IF`; `wait_irq` and `halt` return when a source they
  wait for is requested (the CPU wakes) rather than hang waiting for a
  handler that can't run; and `hw::irq_release` runs what's pending, at
  once and moving no time. The lifted prelude's `msr_cpsr_c(g, sp, v)` is
  a CPSR control-byte write: clearing I releases, on the stack the
  program is on. The critical-section restore stand-ins in every
  `holes/*.toml` call it (an `msr` is still a hard hole the lifter
  leaves to a stand-in), the prelude's IRQ entry sets IRQ mode with I
  for the handler's run, and agb's wrapper stand-in clears it around the
  Rust handler, as its `msr cpsr` does. `gbaa-lift --irq-at-waits`
  brings the old behaviour back (`IRQ_HOLD` false: `Game::cpsr` is
  bookkeeping again); lift-check's `planted_flags` runs it as the
  control.

  The prelude grew from 414 to 439 lines (the `irq_masked` impl,
  `IRQ_HOLD`, `msr_cpsr_c`, the IRQ entry's I bit), which put every
  program over its `lines` bar by 25. june's call (via the parent): the
  prelude is hand-written, not lifted, so it is a metric of its own,
  `lines_prelude` (bar 439), which may rise with a unit's evidence of why;
  `lines` is now everything else and only goes down. The programs' `lines`
  bars became their old values less the old 414-line prelude (the spike's
  73,686), so the lifted code didn't grow.

  **The play build had the same bug, the other way round.** At
  `--timing native` it takes IRQs only in the window after time passes;
  one the I bit held there waited past the `msr` to the next time point
  (the next wait), where the hardware takes it at the `msr`.
  `Machine::tick_native` now opens the window when I is clear and an IRQ
  the last sync found is still pending (only a held one can be: one found
  with I clear was taken in its window); a control `msr` ends a compiled
  block, so the compiled code and the interpreter take it at the same
  instruction. The exact timing already did this (`Sched::irq_line`).

  The test program, `agb/native-irq-hold`: a V-blank handler counts
  frames; `ih_probe` (Thumb, `global_asm!`) polls `VCOUNT` to line 100,
  sets I with its own `ih_disable` (ARM: `mrs`, `orr`, `msr cpsr_c`),
  polls to line 170, past V-blank's start, and clears I with
  `ih_restore` (`msr cpsr_c, r0`), reading the count before (`c0`), after
  the poll (`c1`) and after the restore (`c2`). The hardware gives `c1 -
  c0` 0 and `c2 - c0` 1 every pass, mGBA and both play-build timings
  agree (frame 601's hash), and the old lift gets `c1 - c0` 1 from frame
  2 (`RESULTS`, EWRAM 0x020000c8). The play build before the fix got
  `c2 - c0` 0.

  Not done here: `IME` has the same shape. agb's own `critical_section`
  clears `IME`, not I, and a write that sets `IME` with an IRQ pending is
  taken at once on the hardware; both builds take it at the next time
  point instead (the play build's `irqs_are_taken_only_where_time_passed`
  test pins that). They agree with each other, so the native check can't
  see it; only a test program against mGBA would.

  **Work in progress** (`native/ime-release`, paused 2026-11-01 after
  reading only; no code yet):
  - Play build: `Io` sets `event::IRQ` on an `IE` or `IME` write, but
    `tick_native` acts only on halt, stop and serial, and `Sched::irq_line`
    is false when `IME` was 0 at the last sync, so an `IME` write of 1 with
    `IE & IF` pending waits for the next time point. Fix: in `tick_native`,
    when `events & IRQ` is set, I is clear and `irq_pending()`, open the
    window and return true. Exact timing is unaffected, because its tick
    already syncs on any event. Caveat: the interpreter ticks after every
    instruction and compiled code at a block's exit, so the IRQ can land
    up to a block apart. The test program should write `IME` as a
    function's last act before `bx lr`, which also absorbs mGBA's IRQ delay
    of about 7 cycles. The pinned test `irqs_are_taken_only_where_time_passed`
    changes with it.
  - Hardware library: `Hw::write16(IME)` only sets `master_enable` and
    can't reach `G`. The prelude's `io_write16` must call `hw::irq_release`
    when `IME` goes from 0 to 1, on the program's current stack (agb's
    wrapper stand-in runs on `Game::sp`, which is stale between waits). The
    lifted code's local `r13` is current at each store, because the native
    lift runs no dead-register-write pass. So either stores that may reach
    I/O take `r13` (`st16(g, r13, a, v)`), or `g.sp = r13` is set before
    them, with no line added.
  - The old behaviour goes behind a `gbaa-lift` flag as a `planted_flags`
    control. The test program is `agb/native-ime-release`, modelled on
    `agb/native-irq-hold`: clear `IME`, poll past line 160, set `IME`, read
    the count. Its route is checked with `just mgba-diff` on both timings.

### 2. Recognising the agb library

**Parked (june, 2026-11-01).** gbaa's target is games, and agb programs
are only our test inputs, so making their lifts small doesn't move the
project. The four units in this piece are parked, and RAM globals no longer
wait on them. If a real game needs library recognition (Metroid's sound
engine, say), file a unit for that game.

Of the spike's 111 lifted functions, two are the program's. The rest are
agb 0.25, `core`, `alloc`, `compiler_builtins`, `portable_atomic` and
`qrcodegen`, the same bytes in every agb program but for addresses.

- **Library functions recognised by code bytes** (`native/lib-signatures`).
  Signatures from our own agb builds' ELF symbols (`$GBAA_DATA/agb/*.elf`):
  each function's bytes with call targets and literal-pool words masked,
  as `scripts/libmatch-poc.py` did for Emerald, in Rust; identical masked
  bytes under two names count as unrecognised. The lifter names a
  recognised function by its symbol. Check: leave-one-out over the agb
  0.25 builds (signatures from all but one, matched against the held-out
  ROM, scored against the held-out ELF's symbols) recognises at least 95%
  of the library functions that also occur in another build, with 0 wrong
  names. All our builds share agb 0.25 and one nightly, so that alone is
  easy; the false-positive control is the agbcc games (Emerald, Metroid),
  where Rust signatures should name nothing.
- **The four stand-ins written from recognised names**
  (`native/lib-standins`). Hand-written Rust per agb symbol, kept once
  (not per program), with its address filled in at lift. The three
  `holes/*.toml` lose their `[[standin]]`s, and a fourth program
  (`agb/template`) lifts with no holes file at all. The planted hard hole
  becomes "lift with the IRQ wrapper's replacement turned off".
- **Library leaves as native code** (`native/lib-leaves`). The panic path
  (agb's panic screen, `core::fmt`, `qrcodegen`) becomes a native `panic!`;
  `memcpy`/`memset`/`memmove`/`memcmp`, the division helpers and the
  critical sections become prelude functions. These leave RAM byte for
  byte as the lifted code did, so the native check still needs no skip
  list. The routes never panic, so a new `agb/native-panic` (panics at
  frame 30) checks the panic path: its native binary exits with the
  message. Bar: the spike under 20,000 lines (from about 74,500).
- **agb's runtime as hardware-library calls** (`native/lib-agb-runtime`).
  The interrupt list and `add_interrupt_handler` become `g.hw.irq`;
  `wait_for_vblank` and `VBlank::get` become `hw::wait_vblank`; the entry
  and crt0 copies become the program's start. Their RAM (the list's nodes,
  agb's globals) no longer exists natively, so the lifter writes the
  native check's skip list from what the replaced functions touch, as the
  hand spike's `--replaced` file does by hand; on the spike it may skip
  nothing the hand list doesn't, and a bar on skipped bytes holds it.
  agb's allocator stays lifted until pointers are typed (its heap holds the
  program's own objects). Bar: the spike under 8,000 lines.

Calls replace agb with hand-written Rust over `Hw`, never with the real
agb crate: agb targets the GBA (memory-mapped I/O, `asm!`) and doesn't
build for the host.

### 3. Function shape

- **Parameters and results from the summaries** (`native/lift-params`).
  `fn f(g, sp, a0, a1) -> u32` from each summary's `params` and `results`,
  where today every function is `fn(g, sp, [u32; 4]) -> [u32; 4]`. A
  function with a summary hole keeps the four-register form. Indirect calls
  pass all of r0-r3, through an adapter per entry in `FNS`. A call that
  passes a register its callee reads past its parameters (Emerald's
  `GetMonData2` callers) passes it explicitly. Bar: four-register
  functions no more than the functions with summary holes.
- **Function addresses followed to every use** (`native/fnref-flow`).
  Today pointers are ROM keys, because the literal lift loads them from
  ROM bytes and RAM. Dense IDs need every place a function address enters
  a value: pool words, words in ROM data (agb's vtables), constants stored
  to RAM. The lifter follows each such value (section 4's rule: only
  calls, stores, compares, the halves split and joined, and `| 1` folded;
  anything else a hard hole) and reports the words it proved and each
  indirect call's callee set. Check: unit tests per use; bar on function
  words not proven.
- **Function pointers as dense IDs** (`native/fnref-dense`). The proven
  words rewritten in the lifter's copy of ROM data, `FNS` dense from 1
  under a new `--fn-ids dense`; ROM keys stay the native check's (RAM holds
  the play build's numbers). Check: `lift-check` green with ROM keys, and
  each program's dense build draws the same 600 frames (`route-check
  --native … --packed`); bar: 0 ROM keys left in a dense build.
- **Indirect calls pass what their callees read**
  (`native/indirect-call-args`). The summaries unit left this open: assume
  all four registers, or compute what each pointer can reach. With the
  points-to from the dense IDs, an indirect call whose callee set is
  complete passes the union of its callees' parameters; an incomplete set
  keeps all four. The analysis goes where `codegen::saves::summarize` can
  use it too, so it shows on Emerald without lifting it (its 656
  disagreements from indirect calls, section 5's table, re-measured; no bar
  there, since Emerald's callbacks go through RAM globals whose callee sets
  may stay incomplete). Bar on the agb programs: no indirect call with a
  complete callee set still passing all four.

### 4. The stack

agb passes the addresses of stack values everywhere (`&mut self` on a
local), so locals need an escape analysis first.

- **Stack frames: which slots escape** (`native/frame-escape`). Per
  function, the frame's slots (from sp-relative accesses and their widths)
  and, for each, whether its address escapes: passed to a callee, stored,
  returned, or used in arithmetic past the slot. Reported, not printed.
  Check: unit tests on hand-built cases of each kind; the report's counts
  on the three programs.
- **Stack slots that don't escape as Rust locals**
  (`native/frame-locals`). Their loads and stores through sp become locals;
  escaping slots stay IWRAM bytes. Bar: 0 sp-relative accesses to slots
  that don't escape (a metric the escape analysis adds). Planted control: a
  slot the lift gets wrong on purpose, reaching a global, is caught.

Escaping slots passed as `&mut` (the design's section 1 form 1) come with
typed pointers, below; until then they are soft holes, counted.

### 5. Typed data

- **ROM data as typed statics** (`native/rom-statics`). ROM objects read at
  constant addresses or through pool pointers become `static`s typed by the
  widths the code reads them at (`[u16; N]`), loads from them indexing. A
  pointer loaded from ROM data stays a `RomRef` (a soft hole, counted).
  Needs the dense IDs, so a function-pointer word in a table is an
  `FnRef`. Bar: ROM byte loads at most a quarter of their bar.
- **RAM globals as typed fields** (`native/ram-globals`). Byte RAM carved
  into globals found by constant-address accesses (`d_<offset>`, sized and
  typed by access widths, names from a names file where one exists), laid
  out at the guest's offsets under `--layout guest`. Bar: constant-address
  RAM byte accesses at most a tenth of their bar.
- **Pointers into one object as typed access** (`native/ref-in-object`). A
  computed address whose value set stays in one global (or one local, or
  one ROM static) becomes an indexed, typed access. Bar: computed-address
  RAM byte accesses at most half of their bar.
- **Heap pointers as `Ref<T>`** (`native/ref-heap`). What agb's allocator
  returns, typed by the accesses through it; the allocator can be replaced
  after this. Same kind of bar.
- **Lifted programs at the packed layout draw the same**
  (`native/lift-packed-layout`). Once RAM is fields, a lift with fields in
  the lifter's order (no guest offsets) must draw the same frames as the
  guest-layout build (`route-check --native … --packed`).

Pointer arithmetic the types don't explain (section 6's last hard-hole
kind for stage B) is measured here for the first time.

## The first game

**Metroid: Zero Mission at stage A, then Emerald at stage B** (the
default of the question to june; see the tracker). The hole report
(`research/native-holes.md`) ranks Metroid closest: 18 hard holes, 13 of
them calls to non-entries and 2 the flash library's stack routines, all
of them covered by units already filed (`native/disc-run-call-targets`,
`native/disc-ram-copies`, `native/flash-stack-standin`,
`native/hole-coverage`). Emerald is the game june's romhacks are built on
and the one with types (pret's DWARF), but it has 34 hard holes and
15,158 functions, so a literal lift of it is the largest build there is.
Metroid first tests stage A on a real agbcc game at a size that builds,
and the two shortcuts above at game scale.

- **Lifted crates split into modules that build**
  (`native/lift-modules`). A literal lift of a real game is about a million
  lines (74,500 per 29 KB of agb code); one `main.rs` that size may not
  build. Split the generated crate into modules, proved on the agb programs.
- **Metroid lifts with no hard holes and builds**
  (`native/metroid-lifts`). Needs the discovery units, both shortcuts and
  the module split. The ROM's file name has spaces, so it lifts with
  `--name metroid` and `holes/metroid.toml`.
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

- **Shortcuts, then the first game.** The fallthrough and IRQ-hold fixes
  and the module split need nothing else. Metroid's lift waits on all
  three, on the four discovery and stand-in units the hole report filed,
  and on june's answer; its
  native check also waits on sound in the hardware library, because the
  route compares audio.
- **Recognition.** Signatures first; the stand-ins wait on them and on the
  IRQ-hold fix (which changes what the critical-section stand-ins do); the
  leaves wait on the stand-ins (same code path, and the leaves' bar
  assumes the stand-ins are gone); agb's runtime waits on the leaves.
- **Shape and data.** Parameters first, after the fallthrough fix (both
  rewrite how calls and continuations print); following function
  addresses waits on parameters (the `FNS` adapters); dense IDs and
  indirect-call arguments wait on that; ROM statics wait on the dense IDs (a function word in a table must
  already be an `FnRef`). The escape analysis needs only the bars; stack
  locals wait on it and on parameters (a function's signature decides
  what's in its frame). RAM globals wait on agb's runtime (agb's globals
  leave the lift first), and so does the packed layout; typed pointers
  wait on RAM globals and stack locals, since a pointer is typed by the
  object it points into; heap pointers wait on those.

Not filed, for stage C or later: the 8 jumps by value per program as
`match` (the structuring plan, unused so far), the 48 functions per
program whose callee-saves aren't proven (soft: each function keeps its
own r4-r11), escaping stack slots as `&mut`, and replacing agb's
allocator. Also not filed: discovery's own fix for the gaps the lifter decodes (a `bl`'s
second half taken for a pool, a scan stopped short): most of
mosaic-modes' 143 targets are in `qrcodegen`, which the leaves unit takes
out of the lift; file it if a game needs it.
