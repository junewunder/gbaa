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

  As built (2026-11-06; `crates/codegen/src/native/shape.rs`): a
  function is *shaped* when its summary has no hole and none of the
  callee summaries its run used (`Summary::callees`, its calls and tail
  calls) stopped at a hole (`Summary::stopped`: a jump into code the run
  doesn't follow, to an unknown place, a mode change; reads past it are
  unseen, so its callers' parameters may be short). A shaped function is
  `f_<key>(g, sp, a0, a2) -> Result<u32, Exit>` (`()`, `u32` or a tuple),
  its extended entry `x_<key>` returns the same, and `a_<key>` is its
  four-register adapter, which `FNS` and `ENTRY` hold. Parameters are the
  summary's `params`, which already include a register passed on to a
  callee that reads it (the `GetMonData2` case is a parameter of the
  caller, so a direct call passes the callee's parameters and nothing
  else). Results are what may be read after it returns, among the
  registers it may change: the summary's `results`, r0 (an indirect
  call's caller reads it, as the summaries assume), all four where a
  direct caller's reads aren't known (its run stopped, or it is in the
  other code context), what is read after a function that returns its
  value unread (`forwards`), all four after a call by value whose target
  the lifter knows (the summaries take it as indirect), and what the
  callers of a function that falls or jumps into it read (a fixed point; a jump by value the run didn't
  prove a return adds its function's to every function: r0 on all five
  programs). A result it may leave as it came is a parameter too. A
  register it never changes stays the caller's. The indirect calls'
  r1-r3 come back from the adapter as they went in, which the summaries
  take as undefined anyway. Measured: 27, 22, 29, 28, 29 shaped
  functions (affine, mosaic-modes, native-spike, native-fallthrough,
  native-irq-hold); four-register functions without a summary hole 0 on
  each (bar 0); `lines` unchanged (each adapter's line replaces the
  summary comment the signature now says). Many shaped functions return
  all four, because a caller whose run stopped (a fall into library
  code) calls them: `native/indirect-call-args` and the stack units
  narrow those as the summaries' holes close.
- **Function addresses followed to every use** (`native/fnref-flow`).
  Today pointers are ROM keys, because the literal lift loads them from
  ROM bytes and RAM. Dense IDs need every place a function address enters
  a value: pool words, words in ROM data (agb's vtables), constants stored
  to RAM. The lifter follows each such value (section 4's rule: only
  calls, stores, compares, the halves split and joined, and `| 1` folded;
  anything else a hard hole) and reports the words it proved and each
  indirect call's callee set. Check: unit tests per use; bar on function
  words not proven.

  As built (2026-11-06): `crates/codegen/src/native/fnref.rs`, a
  whole-program abstract run of every function's IR (stand-ins' original
  code too). A value is a set of tokens: a function's key in a form
  (whole; `>> 16`, `& 0xffff` or a halfword store; `<< 16` of either;
  bytes; `!` of it or a difference with it, the two sides of a compare),
  each with its origin (a ROM word, or a constant the code makes); a
  number; a ROM pointer, exact or "from here on" after an index the run
  doesn't follow (reading every word to the next code); and two flags,
  read from RAM and unknown. Sources: constants and pool words equal to a
  function's key (the IR's `Pool` op is placed back at its word by
  decoding the `ldr [pc]`), loads through a ROM pointer of a word holding
  one, the start's copies of ROM data to RAM, `CpuSet` from ROM. Memory:
  each function's frame words where the frame's run placed the access
  and the slot doesn't escape (a saved register's slot reads as the
  register as it came, unknown; one at or above the entry sp, a stack
  argument, also reads the pile its caller's store went to), constant
  RAM addresses one cell each (mirrors folded), and one pile for every
  other store; memory marks what else was stored, so a cell nothing
  else reached is exactly what was; a load the value-set analysis bounds
  to ROM reads the words there. A call passes r0-r3 and takes back its callee's
  returns; a tail call or fall passes every register; an indirect call
  passes them to each function its target may hold. The rule: a function
  value may be called, stored and loaded back (a copy by bytes or halves
  reassembles it), compared for equality as a whole (`== 0` of a
  difference with another, 0 or an unknown: a small constant could be a
  dense ID, and a half's top is 0 when dense, so halves aren't
  compared), split and joined (a half joined only with the other half),
  `| 1` on a Thumb key; anything else (an address, other arithmetic, an
  ordered compare, a bit test, a piece used as a number, a piece called)
  marks its origin not proven, and so does a value given to the BIOS or
  stored outside RAM (I/O, display memory, SRAM). A word is proven when the run read it and no
  value it entered met any of those. It rests on a closed world: function
  and ROM addresses enter values only from those sources, so an unknown
  value is never one, and a callee set of whole addresses is complete,
  unless the run dropped a ROM word or pointer on its way (a flag on the
  value, carried through memory: a "from here on" read cut short, a
  ROM-bounded read too wide to make, a ROM pointer less an unknown);
  a call through such a value is incomplete. A
  forbidden use is reported, not a hard hole: nothing printed changes yet
  (`main.rs` byte-identical), and `native/fnref-dense` is where one stops
  a dense build. The report's `function addresses` line and a line per
  word, constant and site; metrics `fn_words_not_proven` (barred at the
  values below) and `fn_words_proven` (unbarred). The run takes 3-6 s of
  each lift's 6-12 s (about a second of it reading the 110 KB of ROM data).

  | program | function words (pools, data) | proven | never read | not allowed | through the pile | code constants proven | `call_fn` sets complete |
  |---|---|---|---|---|---|---|---|
  | native-spike | 58 (34, 24) | 5 | 9 | 44 | 43 | 11 of 13 | 59 of 59 |
  | affine | 56 (34, 22) | 4 | 15 | 37 | 35 | 11 of 13 | 57 of 57 |
  | mosaic-modes | 55 (34, 21) | 4 | 14 | 37 | 36 | 11 of 14 | 57 of 57 |
  | native-fallthrough | 57 (35, 22) | 4 | 15 | 38 | 36 | 11 of 13 | 58 of 58 |
  | native-irq-hold | 61 (36, 25) | 6 | 15 | 40 | 38 | 11 of 13 | 61 of 61 |
  | native-frame-locals | 56 (34, 22) | 4 | 15 | 37 | 35 | 11 of 13 | 57 of 57 |

  ("Not allowed" counts the BIOS too; "through the pile" are the
  not-proven words whose values were stored where the run can't place
  them.) **The pile is the limit.** agb keeps its closures and trait
  objects on the heap, and every heap store goes to the one pile, so a
  load through any heap pointer may give every function address ever
  stored there; when one such load is used as a number (a length, a
  flag), every one of them is marked. That is most of the not-proven
  words, and why each `call_fn` set, though complete, is every
  address-taken function (17 on the spike). Splitting the pile by the
  value-set analysis's bounds on each store's address proved nothing
  more (agb's heap stores aren't bounded) and was nine times slower.
  What would: the heap by allocation site and field (agb's allocator's
  callers), which `native/fnref-dense`'s bar (0 ROM keys left) needs.
  The "never read" words are words no tracked load reaches (vtables of
  types never made, or reached by a path the run loses); they stay ROM
  keys. Order matters a little to the widening (more than 8 numbers are
  unknown, more than 16 exact ROM pointers become one "from the lowest
  on"), so the first reason given per word can move with the code. Gaps
  the review named, kept: a "from here on" read stops after 256 KB (the
  agb programs' data runs to about 110 KB, which the old 16 KB cut lost)
  and a ROM-bounded read wider than that reads nothing; a ROM pointer
  less an unknown is unknown (each flags the value, so a call through it
  is incomplete rather than wrongly complete); a load through a pointer
  with no bounds at all reads no ROM; a RAM-to-RAM `CpuSet` isn't
  followed; `ror` by an unknown amount (a word load's alignment rotation)
  keeps a function's form, and a ROM pointer's. `native/fnref-dense` should know that proofs are per origin,
  and a pool load folded from a non-pc-relative address gets a
  code-constant origin, so dense needs per-site constant rewriting.
  `GBAA_FNREF_TRACE=1` prints the run's rounds and each use not allowed.
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

  As built (2026-11-06): the sets are `native/fnref-flow`'s, a call's
  (not a jump by value's) when complete. `codegen::saves::summarize_sets`
  takes them by call address: such a call reads the union of its
  callees' parameters (the cause, the first callee that reads one), leaves
  what any of them may leave, and what is read after it is read after
  each callee, so their results include it; the callees join
  `Summary::callees`, so one that grows reruns the caller. An incomplete
  set is as before (r0-r3 in, r0 out). The lift summarises twice: plainly
  first, since the frames need summaries and the flow needs the frames,
  then with the sets (the frames stay the first's).
  `native::callee_sets` is the flow over one context without a lift, for
  `gbaa-recompile --summaries`. A `call_fn` call with a complete set
  passes what its callees read (a shaped callee's parameters, all four
  for a four-register one) and a register one may change while another
  leaves it as it came (that one's adapter hands back what it was given),
  and takes back what any may change; the rest stay the caller's:
  `{ let o = call_fn(g, k, r13, [r0, r1, 0, 0])?; r0 = o[0]; }`.
  Metrics `call_fn_complete` and `call_fn_four_complete`, and a report
  line, `indirect call arguments`.

  | program | `call_fn` calls | complete set | still all four | complete sets: callees' summaries read all four / a four-register callee |
  |---|---|---|---|---|
  | native-spike | 63 | 58 | 58 | 59 / 59 of 59 |
  | affine | 61 | 56 | 56 | 57 / 57 of 57 |
  | mosaic-modes | 61 | 56 | 56 | 57 / 57 of 57 |
  | native-fallthrough | 62 | 57 | 57 | 57 / 58 of 58 |
  | native-irq-hold | 65 | 60 | 60 | 59 / 61 of 61 |
  | native-frame-locals | 61 | 56 | 56 | 57 / 57 of 57 |

  **So the bar is not 0: it is the values measured.** Every complete set
  on these programs that `call_fn` prints is the same one, every
  address-taken function (21 on the spike, `native/fnref-flow`'s pile),
  and between them those functions' summaries read all four registers
  even taken at their word; 15 of the 21 are four-register besides (a
  summary hole, or the IRQ wrapper's stand-in). (The few sets whose
  summaries read fewer have a four-register callee too; a set counted
  here may be a call the lifter prints directly, its target known.) The sets narrow only
  with the heap split by allocation site and field (`native/fnref-dense`
  needs that too). What did change is the results: a callee's leftovers
  in r1-r3 after a call through a complete set are now defined (they were
  the "undefined" that let `a_<key>` hand r1-r3 back as given), and a
  caller that reads or passes them on makes them results. So more shaped
  functions return two to four registers (the spike's `f_0800017d`
  returns `(r0, r1)`; what is read after a tail call by value went from
  {r0} to all four), where an adapter used to hand back a register the
  function really changed. `lines` unchanged on all six; `lift-check`
  passes.

  Emerald isn't re-measured: `gbaa-recompile --summaries --callee-sets`
  runs the flow over its ROM code, which didn't finish in 35 minutes (553
  indirect calls waiting on it; Metroid's round 1 alone took 8 minutes,
  `native/fnref-flow-scale`). Without the flag `--summaries` is as it was
  (25 s; 13,359 agree, 656 from indirect calls). The re-measurement is
  `native/indirect-call-args-emerald`, after the flow scales.

### 4. The stack

agb passes the addresses of stack values everywhere (`&mut self` on a
local), so locals need an escape analysis first.

- **Stack frames: which slots escape** (`native/frame-escape`). Per
  function, the frame's slots (from sp-relative accesses and their widths)
  and, for each, whether its address escapes: passed to a callee, stored,
  returned, or used in arithmetic past the slot. Reported, not printed.
  Check: unit tests on hand-built cases of each kind; the report's counts
  on the three programs.

  As built (2026-11-06): `crates/codegen/src/native/frame.rs`, an
  abstract run of each lifted function's IR from its entry (values are
  the entry sp plus a constant, a constant, a register as it came, or
  "made from these sp offsets" by arithmetic it doesn't follow; frame
  words keep what was stored in them). Slots are the byte ranges
  sp-relative accesses reach, overlaps merged; an escaping address no
  access reaches is an empty slot of its own. The kinds: passed to a
  callee in a register it reads (its summary's parameters, all four
  without one; a tail call or fall, every register); a callee's stack
  argument (at or above sp at the call, within the bytes the callee reads
  above its entry sp; an indirect call, the most any function reads);
  stored outside the frame or in a slot that escapes; returned in r0-r3;
  arithmetic the run can't follow, used as an address; sp lost. An
  escaping address is an object of unknown size: its slot escapes, and so
  does every slot above it up to the first saved-register slot (every
  store a word holding r4-r11 or lr as they came) or the entry sp.
  Nothing printed changes (`main.rs` is byte-identical). The report's
  `stack frames` line and two metrics, barred in `lift-check.toml`:
  `sp_accesses_local_slot` and `sp_accesses_escaping_slot`, which sum to
  `load_sp` + `store_sp`.

  | program | slots | escaping | to slots kept | to slots escaping |
  |---|---|---|---|---|
  | native-spike | 856 | 391 | 2,239 | 1,289 |
  | affine | 852 | 248 | 2,905 | 704 |
  | mosaic-modes | 718 | 353 | 1,093 | 1,124 |
  | native-fallthrough | 858 | 252 | 2,892 | 708 |
  | native-irq-hold | 866 | 253 | 2,912 | 708 |

  On native-spike, by kind: passed to a callee 148, stack argument 232,
  stored 122, returned 2, arithmetic 69; no function loses sp. The spike's
  stack-argument count is high because one function there reads 54 bytes
  above its entry sp, and every indirect call is assumed to call it; the
  callee sets of `native/fnref-flow` would narrow that. Two gaps the
  locals unit must weigh: an escaping address marks only the slots at and
  above it (a pointer into an object's middle reaching below isn't
  followed), and a callee's stack-argument bytes count only its exact
  sp-relative reads.
- **Stack slots that don't escape as Rust locals**
  (`native/frame-locals`). Their loads and stores through sp become locals;
  escaping slots stay IWRAM bytes. Bar: 0 sp-relative accesses to slots
  that don't escape (a metric the escape analysis adds). Planted control: a
  slot the lift gets wrong on purpose, reaching a global, is caught.

  As built (2026-11-06): a slot that doesn't escape, lies below the entry
  sp and is only accessed aligned (`Frame::locals`) is a Rust local,
  `l<bytes below the entry sp>`, a `u32` for an aligned word only ever
  accessed whole, else `[u8; N]`, declared zero at the top of `x_<key>`;
  its loads and stores are the local's, and the address arithmetic only
  they used isn't printed (a per-block pass drops every pure value
  nothing printed uses). Slots at or above the entry sp are the caller's
  (a fall's shared epilogue, stack arguments), never locals: a metric of
  their own, `sp_accesses_callers_frame`. The escape analysis gained one
  rule: a callee from whose entry sp + k an address escapes (agb's
  `assert_failed` passes on a pointer to its stack argument) reaches up
  into its caller's frame from the caller's sp + k, an escaping address
  there, unless the caller is done with its frame after the call (the
  callee never returns, or a tail call): then the locals are written to
  the frame's bytes before the call (`Frame::flush`; 4 calls on the
  spike). For a call through a register, the reach is the lowest of the
  functions whose address is a ROM word. The gap the escape analysis left
  open, a pointer that walks down from where it escaped, stays open:
  walking down too would leave about three times the accesses in memory
  (native-spike: 2,711 sp accesses to escaping slots, not 1,289).
  `gbaa-lift --shadow-locals` checks it at run time instead: every local
  is kept in the frame's bytes too, and a load that differs from them
  prints `shadow local:`. All six programs run 601 frames with none (a
  lift with one mirrored store taken out prints them from frame 0).

  The control is a sixth program, `agb/native-frame-locals`: `fl_keep`
  keeps a word, a halfword and a byte in its frame across a call that
  clobbers r0-r3 and returns them combined, which the main loop stores to
  a global; `fl_esc`'s slot escapes to `fl_double`, which writes through
  it. `--plant-slot=0x08000182` makes `fl_keep`'s local store of `x + 7`
  store one more (the lift fails if the address isn't a store to a local).

  Lines are with `native/ram-globals` merged, which landed alongside.

  | program | locals | accesses to them | `load_sp` | `store_sp` | lines |
  |---|---|---|---|---|---|
  | native-spike | 458 | 2,291 | 2,177 → 669 | 1,351 → 628 | 72,733 → 71,823 |
  | affine | 597 | 2,907 | 2,232 → 268 | 1,377 → 443 | 72,638 → 71,213 |
  | mosaic-modes | 362 | 1,151 | 1,202 → 574 | 1,015 → 554 | 49,882 → 49,238 |
  | native-fallthrough | 597 | 2,892 | 2,221 → 268 | 1,379 → 449 | 71,520 → 70,103 |
  | native-irq-hold | 606 | 2,923 | 2,227 → 267 | 1,393 → 448 | 72,204 → 70,762 |
  | native-frame-locals | 603 | 2,904 | 265 | 446 | 70,185 |

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

  As built (2026-11-06): `crates/codegen/src/native/globals.rs`. Every
  constant-address RAM access of the functions printed (not stand-ins')
  is a byte range in `Game::ram`; a names file's datum of known size that
  one reaches is one too; overlapping ranges are one global, its element
  the widest access (`u32`, `[u16; N]`), a narrower access inside an
  element its bits (shift and mask; a store a read-modify-write). Aligned
  accesses nest, so only a datum of odd size can leave a wide access as
  bytes. Without names, accesses that only touch are separate globals:
  nothing says they are one array (the agb programs have no names file,
  so their globals are all `d_<guest address>` scalars). The globals are
  the fields of `Ram`, `#[repr(C, align(4))]`, at the guest's offsets
  with `[u8; N]` padding between; the prelude asserts its size is EWRAM
  plus IWRAM (so it has no padding of its own) and the target
  little-endian, allocates it zeroed and derefs it to `[u8; EWRAM +
  IWRAM]`, which every other access, the stand-ins and the native check
  use as before. `gbaa-lift --names DIR` (default `names/`) and
  `--layout guest` (the only value yet). Metrics: `load_const_ram` and
  `store_const_ram` now count byte accesses only (0 on all five
  programs, from 27-36 and 5-11); the field accesses are
  `load_ram_global` and `store_ram_global` and the globals `ram_globals`
  (23-28), unbarred. The spike's planted wrong store is a field store
  now, still caught.

  The field accesses don't print their address, so the constant
  arithmetic only they used isn't printed; nor is a load from a
  constant ROM address nothing uses. Every constant ROM load was one: a
  literal pool's, whose word the IR already has as a `Pool` constant.
  So `load_const_rom` fell to 0 (from 425-514) and `lines` by 830-1,010
  a program, which pays for `Ram` (one line per global and gap). For
  `native/rom-statics` that leaves the computed ROM loads as its bar.
  The prelude grew by 31 lines (439 to 470): `Ram`'s zeroed allocation,
  its two `Deref` impls and the layout assertion.
- **Pointers into one object as typed access** (`native/ref-in-object`). A
  computed address whose value set stays in one global (or one local, or
  one ROM static) becomes an indexed, typed access. Bar: computed-address
  RAM byte accesses at most half of their bar.

  As built (2026-11-06): `Globals::object` in
  `crates/codegen/src/native/globals.rs`. A load or store whose address
  isn't a constant in its block but which the value sets bound to RAM
  (`analysis::vsa::func::Access`, the instruction's hull, in one
  mirror) is carved like a constant one: a single address is an access
  at it; a range (lowest to highest address plus the width) is an
  object's extent, one global, if it reaches at most one of the globals
  the rest carve, else it is left out (ranges aren't weighed against
  each other: two that overlap may join the globals they reach). Printing: a single address is
  that global's field, with the prelude's `pin(a, off)` checking the
  pointer (`{ pin(v3 & !3, 0x8); g.ram.d_02000008 }`); a range inside
  one array global at its element's width is
  `g.ram.d_…[ix(a, off, elem, n)]`, the element the pointer finds, checked.
  A pointer the value sets got wrong (a caller they didn't see) stops the
  program naming it, rather than reading the wrong bytes. Both print
  their address, so nothing else is dropped and `lines` is unchanged.

  Metrics: `load_ram_object` and `store_ram_object` (unbarred, meant to
  rise) count the typed ones, and `pointer_unexplained`, the new soft
  hole, the computed RAM accesses no one global explains (several
  globals, or one at a width not its element's): measured for the
  first time here, 1 on every program but native-irq-hold (0): crt0's
  `.bss` clear, a word loop over four separate globals. The report says
  it on its `pointers into one global` line.

  | program | `load_computed_ram` | `store_computed_ram` | typed loads, stores | `pointer_unexplained` |
  |---|---|---|---|---|
  | native-spike | 27 → 0 | 9 → 1 | 27, 8 | 1 |
  | affine | 26 → 0 | 9 → 1 | 26, 8 | 1 |
  | mosaic-modes | 26 → 0 | 9 → 1 | 26, 8 | 1 |
  | native-fallthrough | 23 → 0 | 8 → 1 | 23, 7 | 1 |
  | native-irq-hold | 25 → 0 | 9 → 0 | 25, 9 | 0 |
  | native-frame-locals | 23 → 0 | 8 → 1 | 23, 7 | 1 |

  Every typed access on the agb programs is a single address (agb's
  allocator state, `0x02000000`-`0x02000014`, reached through a pointer
  its callers pass): the value sets knew it where the block didn't. No
  program has a range inside one global yet, so the `ix` form is checked
  only by unit tests of what it prints; no lift runs it. The prelude grew by 27 lines (470 to
  497): `pin`, `ix` and their shared panic.

  Not done here, because nothing is there to point into: escaping stack
  slots are still frame bytes (a local's address never escapes, so no
  pointer reaches one; `&mut` locals are stage C), and ROM statics wait
  on `native/rom-statics`, whose loads index the static from the value
  sets' extents. Most computed accesses (`load_computed_unknown`, 589-762
  a program) have no value set at all: parameters from callers the
  analysis can't see and the heap's pointers, `native/ref-heap`'s.
- **Heap pointers as `Ref<T>`** (`native/ref-heap`). What agb's allocator
  returns, typed by the accesses through it; the allocator can be replaced
  after this. Same kind of bar.
- **Lifted programs at the packed layout draw the same**
  (`native/lift-packed-layout`). Once RAM is fields, a lift with fields in
  the lifter's order (no guest offsets) must draw the same frames as the
  guest-layout build (`route-check --native … --packed`).

  As built (2026-11-06): `gbaa-lift --layout packed`
  (`Globals::ram_struct`, `crates/codegen/src/native/globals.rs`, and
  `native/prelude_packed.rs`, which takes the place of the prelude's
  guest-layout `Ram` code). `Ram`'s fields are the globals, widest
  element first and then from the highest guest address down (so globals
  the guest has side by side aren't neighbours here), then `bytes`, all
  of RAM; `RAM_GLOBALS` gives each global's RAM offset, size and
  `offset_of!` field. Typed accesses are the same code in both layouts;
  everything else reaches RAM as `g.ram[offset]` (computed addresses,
  the stack, the BIOS calls, the stand-ins through `ld`/`st`), which the
  packed `Index` finds in the field holding that byte, else in `bytes`.
  So the prelude's `ld16`/`ld32`/`st16`/`st32` now go a byte at a time
  (a word may span two globals, as crt0's `.bss` clear's do): 3 more
  prelude lines (497 to 500). A packed program records RAM as the guest
  lays it out (`Ram::image`), and `route-check --packed` now compares its
  records with the guest build's whole, nothing skipped, after the frame
  hashes. `gbaa-lift-check` lifts and builds every program once more at
  `packed` and passes it as `--packed`: all six draw and record the same
  600 (601) frames. Pointers are still guest addresses in both layouts
  (no `Ref<T>` until `native/ref-heap`); a packed build with two of the
  spike's allocator globals swapped in `RAM_GLOBALS` crashed the check,
  by hand.

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

  Built 2026-11-06 on quack (`codegen::native::modules`). `src/` is
  `main.rs` (the prelude, RAM's struct, `NAME`, `ENTRY`, and a `mod` and
  glob `use` per file), `fns.rs` (`FNS`) and `m_<first key>.rs`: the
  functions in print order (ROM address, then each IWRAM copy), a module
  closing at the first function that takes it past 4,000 lines (one
  longer function is a module of its own). Each module starts `use
  crate::*;`. The point is codegen: rustc gives a module its own codegen
  unit, so one `main.rs` ran LLVM on one core. The `lines` metric leaves
  out the wiring (`mod`, `use`, each module's head); the four `use`
  lines the root lost lowered every program's bar by 5.

  Measured on quack (24 cores, the build lock's `CARGO_BUILD_JOBS=6`, the
  crate alone rebuilt, deps built; `/usr/bin/time`, maxrss the largest
  rustc):

  | crate | lines, all files | modules | wall | CPU | peak rustc RSS |
  |---|---|---|---|---|---|
  | native-spike, one `main.rs` | 72,320 | 1 | 6.4, 9.7, 8.1 s | 6.5-9.9 s | 0.50 GB |
  | native-spike, split | 72,439 | 17 (+2) | 4.3, 6.7, 5.5 s | 8.3-9.3 s | 0.55 GB |
  | spike `--copies 4` | 288,084 | 68 | 18.1 s | 30.2 s | 1.27 GB |
  | spike `--copies 14` | 1,006,884 | 238 | 54.8 s | 94.0 s | 3.67 GB |

  (Runs of the two spike crates alternated, on a machine with the other
  lane busy, hence the spread.) Peak RAM grows by about 14 MB per
  4,000-line module, both from 1 to 4 copies and from 4 to 14: rustc's
  front end holds the whole crate, so this is the crate's cost spread
  over its modules, not a module's alone; a million-line game builds in
  under 4 GB and a minute. `gbaa-lift --copies N` makes the synthetic
  crate (each copy's functions renamed `f<i>_<key>`, kept by a `#[used]`
  table of their entries).
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
