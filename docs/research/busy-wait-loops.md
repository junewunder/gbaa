# Busy-wait loops that call functions, under native timing

The question is the busy-loop question (`q/busy-loop-calls-functions-let`).
Emerald's `DoMapLoadLoop` (`while (!LoadMapInStepsLocal(state, FALSE));`)
waits for the V-blank handler's `ProcessDma3Requests` to drain the DMA3
queue. It polls through calls (`FreeTempTileDataBuffersIfPossible` →
`IsDma3ManagerBusyWithBgCopy` → `CheckForSpaceForDma3Request`), so it is
not an idle edge (`armv4t::idle`, branch `t3/c58bf1ab`, head f773f153).
Under `--timing native` no time passes in it, and it hangs. The hole report
(`docs/research/native-holes.md`) finds three such loops in Rogue. It
finds none in Emerald's route, which never reaches the new game.

## What others do

**Emulators detect idle loops and halt or skip time. All of them use
heuristics, and some add per-game lists.**

- **mGBA** ([memory.c](https://github.com/mgba-emu/mgba/blob/master/src/gba/memory.c)):
  when the same jump target comes round again, it saves the registers. On
  the next pass it compares them. If they are equal, it simulates the
  Thumb loop body: any store, or a load from a base register it can't
  know, rejects the loop; non-constant I/O reads reject it too. If the loop
  passes, it is marked `IDLE_LOOP_REMOVE` and the CPU halts until the next
  IRQ. After 10,000 failed detections (`IDLE_LOOP_THRESHOLD`) it gives up.
  It also keeps a hand list: [overrides.c](https://github.com/mgba-emu/mgba/blob/master/src/gba/overrides.c)
  has 25 games with a fixed `idleLoop`, including every Emerald region at
  `0x080008C6`. So mGBA uses both detection and a per-game list.
- **Dolphin** ([PPCAnalyst.cpp](https://github.com/dolphin-emu/dolphin/blob/master/Source/Core/Core/PowerPC/PPCAnalyst.cpp)
  `IsBusyWaitLoop`): a loop counts as a wait if it branches to itself, has
  no stores, and no pass depends on the one before. The JIT then exits
  through `CoreTiming::GlobalIdle` ([Jit.cpp](https://github.com/dolphin-emu/dolphin/blob/master/Source/Core/Core/PowerPC/Jit64/Jit.cpp)).
  Games with more complicated idle loops are missed
  ([forum](https://forums.dolphin-emu.org/Thread-idle-loop-skipping-in-sonic-colors),
  [issue 2655](https://bugs.dolphin-emu.org/issues/2655)). The workaround
  there was a per-game speedhack that makes the loop take more virtual
  time. That is a time budget.
- **melonDS** ([ARMJIT.cpp](https://github.com/melonDS-emu/melonDS/blob/master/src/ARMJIT.cpp)
  `IsIdleLoop`) copies Dolphin's rule; its comment links to Dolphin's code.
  This is the same rule as gbaa's `is_idle_edge`.
- **PCSX2**: two speedhacks. "INTC spin detection" catches games that spin
  on the interrupt status register waiting for vsync. "Wait loop detection"
  catches the BIOS idle loop. Both are optional and can break games
  ([forum guide](https://forums.pcsx2.net/Thread-game-in-slow-motion-what-could-be-the-problem?page=2)).
- **PPSSPP** has no general detector that I could find. PSP waits are
  kernel calls. Where a game's timing goes wrong, PPSSPP charges virtual
  time with `hleEatCycles` ([PR 18513](https://github.com/hrydgard/ppsspp/pull/18513)).
- I found no documented idle detection for NO$GBA, higan/ares or Citra. The
  first two are accuracy-first emulators that just run the loop. Citra runs
  an OS where waits are kernel calls.

None of these has to stay deterministic across two code generators. mGBA's
register comparison comes closest to proving that a loop makes no
progress.

**Recompilers and native ports.** All of them restructure the wait, or
run the game's threads on host threads.

- **pokeemerald PC port** (Kurausukun, `pc_port` branch of
  [Kurausukun/pokeemerald](https://github.com/Kurausukun/pokeemerald)):
  this port hits exactly our loop and patches it by hand. In `src/bg.c`,
  `IsDma3ManagerBusyWithBgCopy` under `#ifdef PORTABLE` does
  `RunDMAsAndVBlank(); return FALSE;`. The V-blank handler is run inside
  the poll. The rest of the game is turned inside out: `src/platform/sdl2.c`
  calls `MainLoop()` once per fixed timestep, then `RunDMAsAndVBlank()`,
  and `VBlankIntrWait` is an empty function.
- **SM64 PC ports** ([sm64ex pc_main.c](https://github.com/Render96/Render96ex/blob/d0d88b9bbda70283f40c1bbe0c56d03ec4206588/src/pc/pc_main.c),
  [sm64plus](https://cocalc.com/github/MorsGames/sm64plus/blob/master/src/pc/pc_main.c)):
  libultra's threads are removed. The host loop calls
  `game_loop_one_iteration()`, `set_vblank_handler` is a stub, and
  `osRecvMesg` is reimplemented.
- **zelda3** ([main.c](https://github.com/snesrev/zelda3/blob/master/src/main.c),
  [zelda_rtl.c](https://github.com/snesrev/zelda3/blob/master/src/zelda_rtl.c)):
  `ZeldaRunFrame` runs the game loop, then `Interrupt_NMI(input)`. The wait
  for NMI is gone.
- **N64Recomp / Zelda64Recomp** ([N64ModernRuntime](https://github.com/N64Recomp/N64ModernRuntime),
  [events.cpp](https://github.com/N64Recomp/N64ModernRuntime/blob/main/ultramodern/src/events.cpp)):
  game threads are real host threads. A "VI Thread" `sleep_until`s each
  retrace and posts the VI message to the game's queue. This works because
  N64 games wait in `osRecvMesg`, an OS call, not in a spin. It is not
  deterministic, and it does not need to be.
- **jamulator**, Andrew Kelley's NES static recompiler
  ([post](https://andrewkelley.me/post/jamulator.html)): the generated code
  calls the runtime after every instruction, and NMI is taken there. The
  post names interrupts from parallel hardware as one of the reasons static
  recompilation is hard.
- **Chocolate Doom** ([d_loop.c](https://github.com/chocolate-doom/chocolate-doom/blob/master/src/d_loop.c)):
  DOS Doom spun on a tick counter that the timer interrupt advanced. The
  port's `TryRunTics` reads host time and adds `I_Sleep(1)` to the wait
  loop.

For Xbox 360 recompilers (XenonRecomp/UnleashedRecomp) and ps2recomp I found
no write-up on busy waits. On those consoles, guest threads become host
threads.

## Is the GBA the only console with this problem?

No. The problem shows up wherever interrupts are the only concurrency: a
console with no OS, where a game waits by spinning on memory that an
interrupt handler writes.

- **NES and SNES**: the "wait for NMI" flag loop is the standard pattern
  ([nesdev](https://nesdev.nes.science/f10/t2991.xhtml)). zelda3 and
  jamulator each had to deal with it.
- **Game Boy, Genesis and DS**: the same pattern, plus `halt`/`IntrWait`.
- **PS2**: games spin on the interrupt status register, which is what
  PCSX2's INTC hack exists for.
- **DOS PCs**: games spin on a tick counter the timer interrupt advances.
- **N64, PSP, 3DS and Xbox 360** mostly avoid it. Their games block in OS
  calls (`osRecvMesg`, kernel waits), so recompilers implement those calls
  with host synchronisation.

The GBA is a bad case for two reasons: it has no OS, and pret-style games
run their own DMA queues serviced by the V-blank handler. gbaa's situation
is also unusual. Most native ports give up determinism (host threads,
wall-clock timers) or patch each wait by hand. gbaa needs native, compiled
and interpreted builds to match byte for byte, which rules out both.

## The options

The constraints: (D) deterministic and identical in compiled,
interpreted and native builds; (P) native code reads as plain Rust;
(G) works across games without hand work; (T) the play builds' exact
timing is untouched.

| Option | D | P | G | T | Solves Emerald? | Notes |
|---|---|---|---|---|---|---|
| (a) back-edge budget (all back edges, global count) | yes | poor: a counter on every loop | yes | yes | yes | fires inside long non-wait loops; a large N makes waits slow |
| (b) idle through store-free callees | yes | good | partly | yes | **no** | `codegen::saves` summaries are register-only, so a store bit is new work. `LoadMapInStepsLocal` stores in other states, and `IsDma3ManagerBusyWithBgCopy` clears busy bits when it makes progress |
| (c) hand list per game | yes | best | no | yes | yes, by hand | what mGBA's overrides and the PC port do; an unlisted loop hangs |
| (d) mGBA-style no-progress check (registers equal and no store that changes memory) | yes | **no** | yes | yes | yes | sound: fires only on a loop that would never end. Native Rust can't see its own stores or locals |
| (e) exit depends only on IRQ-written memory (dataflow) | yes | good | partly | yes | no | `DoMapLoadLoop`'s exit also depends on `*state`; needs path-sensitive analysis across calls |
| (f) time passes at re-reads of IRQ-written addresses | yes | poor | yes | yes | yes | in native, every such field read becomes a call; it fires constantly in non-wait code (`gMain.vblankCounter1`) |
| (g) V-blank from a host timer or thread | **no** | poor (shared `g`) | yes | yes | yes | N64Recomp's way; breaks byte identity |
| **(a′) scoped, sticky spin budget** (below) | yes | good: one call per call-loop | yes | yes | yes | needs calibration of N |

## Recommendation: (a′), a spin budget limited to loops that call

Build on june's option (a), with three changes. Count only the loops that
can be this kind of wait. Count per loop. Once a loop is over budget, it
stays idle until time passes some other way.

**Spin edge** (bytes only, like `is_idle_edge`, so the interpreter, codegen
and lifter agree with no list passed between them; add it as
`armv4t::idle::is_spin_edge`, listed by `analysis::idle::spin_edges`): a
backward branch without link, not an idle edge, whose straight range
[target, branch] is at most 64 instructions and contains a call (`bl`, or
a write of `lr` followed by `bx`/`mov pc`). ROM code only. Optional
refinement, to reduce noise: leave out loops whose exit compares a register
that the range only increments by a constant against an immediate (a
counted `for`).

**Behaviour under `--timing native`**, as `Sched` state:

1. Keep a small map from spin-edge address to `passes`. Clear it at every
   point where time passes other than through rule 3 (BIOS waits, idle
   edges, beam/timer reads synced, DMA and BIOS costs), and whenever the
   beam crosses line 160 (frame end), wherever that happens.
2. On each taken spin edge: `passes += 1`.
3. If `passes > N`, the edge is stuck. Do what `idle_edge` does: run the
   clock to the next event (a half line), then the loop tests again. A
   stuck edge stays stuck until rule 1 clears the map.

States are saved at a frame end, where the map is empty, so it never needs
saving (the same argument as `Sched::window`). Compiled and interpreted
builds see the same branch instructions, so they take every IRQ at the same
instruction. A wait for V-blank costs N passes plus about 456 half-line
idles a frame. Ordinary loops with calls under N passes still take no
time.

**Native output.** The lifter prints `hw::spin(g)` as the last statement of
each such loop:
`while !load_map_in_steps_local(g, state, false) { hw::spin(g); }`.
`spin` is `#[track_caller]`, so the call site is the key and no ID is
printed. The unmodded lift keeps loops one to one with spin edges, so its
counts match the play build's. A modded build stays deterministic within
itself.

**N.** Start at 2,048, then calibrate it. Add a counter to the hole report
for the longest streak of each spin edge that ended on its own (no stuck
pass) on the eight games' routes, and set N to 4× the largest. The longest
known wait streak is Rogue's, at 186 a frame. Expansion-based species loops
(about 1,500 calls) are the non-wait loops most likely to come near N.

**Safety nets.**

- The hole report checks that each of its `busy_loops` addresses is a spin
  edge. A wait outside the straight-range shape is reported, and goes into
  a per-game TOML list of extra spin edges: option (c) as a fallback, not
  as the rule.
- `--interp` debug runs add mGBA's check (d) when an edge sticks: compare
  the registers with the previous pass. Report stuck loops that were still
  making progress, which are N misfires.

## What it costs

- Code: one new `Idle::Spin` kind beside `Edge`/`Exit` in
  `codegen::func`/`lower.rs`, `rt::spin_edge` in `runtime::dispatch`, the
  map in `Sched`, the byte test and its list, `hw::spin` in the native
  library, and the hole-report calibration counter.
- Readability: one `hw::spin(g)` in each unbounded loop that calls. The
  count is not measured yet; `spin_edges` per game should print it.
- Timing: a legitimate loop with more than N calls between time points gets
  half-lines added (about 1 frame per 450 extra passes). Real hardware
  takes time there too. Native timing is its own model and only has to
  match itself.
- Exact timing (the default play build) is unchanged; all of this runs only
  under `--timing native`.
