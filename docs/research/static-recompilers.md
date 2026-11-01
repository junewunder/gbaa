# How static recompilers and lifters emit code and stay correct

This is one of the four literature reviews behind the structured codegen design. It is
unit `structured/lit-recompilers`, for `structured/design`, and part of phase 2 of the
plan of 2026-10-21 in [open-decisions.md](../open-decisions.md). The
question: what do other static recompilers, binary lifters and dynamic
translators do about indirect jumps, self-modifying code, interrupts, cycle
timing and condition flags? And which of their techniques can we take while
the frame hash still matches the interpreter?

Researched 2026-10-24 on quack. Sources were read in their primary form: the
tools' source trees at the commits under "Sources" (shallow clones, never
built or run), and the papers and the authors' own posts. Claims about
Rosetta 2 are marked Apple-stated or reverse-engineered. Every source is
also in [references.md](../references.md). Control-flow structuring itself
(Relooper, Ramsey, decompilers' structurers) is the companion review's
topic, [structured-control-flow.md](structured-control-flow.md). This one
cites it where the two meet.

## Summary

- **Nobody else is exact in our sense.** No tool reviewed checks its output
  against a reference at the level of a frame hash, except one of the GBA
  recompilers below: mstan's gbarecomp co-simulates against mGBA with a
  full-state hash.
  - N64Recomp, XenonRecomp, ReXGlue, PS2Recomp and DolRecomp are checked
    by play-testing and unit tests.
  - The lifters (McSema, rev.ng, RetDec, mctoll) are checked per
    instruction, or end-to-end on program output.
  - The console recompilers get away with it because they replace the
    guest OS. N64Recomp has no interrupts and no cycle counting in its
    generated code at all: guest threads are host threads, and they switch
    only inside OS calls.

  We have no OS to stand on. So the GBA tools and the dynamic translators
  are the real prior art for interrupts and timing, and the lifters are the
  prior art for flags and indirect jumps.
- **Every tool keeps flags exact the same way we do.** It computes them,
  drops a write only when the write is provably dead before any boundary,
  and treats every boundary it can't see past as reading all four flags.
  - QEMU ends its liveness at the end of each translation block.
  - gpsp does Thumb blocks only (ARM mode always computes every flag).
  - FEX and Box64 run over a multi-block unit.
  - McSema's State-aware dead-store pass marks everything live at returns,
    indirect jumps and indirect calls.
  - Asphaltian's GBA recompiler has a flag-liveness pass that is ours,
    almost line for line.

  Nobody keeps flags lazy across an unknown boundary. Our decision 9 is
  the standard technique, already built. The two tools that kept flags in
  per-function locals got flags across calls wrong, silently: mctoll
  resets them to false at each function entry, and RetDec's register
  localization cuts them. That is a warning for the registers review.
- **A gap in our liveness, found here: stale flags reach memory through
  the IRQ handler.** Liveness follows a function's static edges, and an IRQ
  can be taken at any block exit, including a static edge whose flag
  writes were dropped. Emerald's `IntrMain` (pokeemerald `src/crt0.s:40-41`)
  does `mrs r0, spsr; stmfd sp!, {r0-r3,lr}`. That stores the interrupted
  CPSR, stale flags and all, to the IRQ stack in IWRAM. The frame hash
  can't see it: the value only goes back into the SPSR, and the flags it
  restores are dead. But a memory compare can.
  - QEMU, gpsp, FEX and Box64 avoid this because their yield point is a
    block end, where all flags are live.
  - The exact fix for us is to compute the dropped flag writes only on the
    yield path, `if rt::tick(..) { <dead flag writes>; return … }`. That
    branch is cold, so it costs code size, not time. The design should
    decide this.
- **Resume entries are the shared hard problem, and the tools split three
  ways on it.**
  - **No resumes.** N64Recomp and XenonRecomp map functions to functions
    and never enter mid-function, because nothing preempts them.
  - **Resume anywhere.** PS2Recomp, DolRecomp and gbarecomp put a
    `switch (ctx->pc)` at function entry. DolRecomp and gbarecomp have a
    case for *every instruction*, and gbarecomp's `static_resume_all`
    defaults to true "because IRQ/SWI returns and host-stack unwinds may
    resume at any instruction boundary". That is our v1, finer grained.
  - **Fewer yield points, so fewer resumes.** PS2Recomp yields only at back
    edges and calls, so its resume points are return sites, syscall+4 and
    loop heads. Toal's HOWTO, DolRecomp and Asphaltian check timing only
    at back edges (Asphaltian at function entry and loop heads).
- **The most useful technique for us: take the IRQ inside the tick, as a
  nested host call, and don't unwind.** gbarecomp does this
  (`g_irq_nest_depth`). Andrew Kelley's NES recompiler (jamulator) did it
  for NMIs and called it "elegant but imperfect": it broke on games that
  return from the NMI somewhere other than where it came from.
  - The flaw is fixed by a guard we already use for calls: after the nested
    handler returns, continue in compiled code only if `cpu.pc` is the
    expected key; otherwise unwind as now. That is the same guard DolRecomp
    and PS2Recomp put after their calls.
  - Guest-visible behaviour doesn't change: the IRQ is taken at the same
    tick, with the same pc. Only the host mechanism does.
  - With halts handled the same way and the frame end handed off without
    unwinding, almost nothing unwinds any more.
  - That attacks the cost the companion review found, two bodies per
    function, from the other side. Resumes become rare enough for the v1
    body to be the cold path, or the interpreter's.
- **Merging ticks or moving yield points is not exact by construction.**
  Our `rt::tick` both adds cycles and runs the hardware: scanlines, timers,
  DMA, IRQ checks. A tick moved later makes `VCOUNT` and timer reads see an
  older clock, and makes IRQs land later. QEMU's icount mode exists for
  exactly this reason: it ends a block after each I/O instruction and
  retranslates a shorter block to hit a timer deadline. gbarecomp lists
  "IRQ take-point precision" as an open gap. So: keep one tick per CFG edge,
  as the companion review says. Treat fewer yield points as an experiment
  measured by the frame hash, not as a free optimisation.
- **Indirect jumps: everyone uses a lookup table plus a guard, and we
  already do.** Unresolved targets are handled in three ways:
  - fatal (N64Recomp, XenonRecomp's null call, ReXGlue, rev.ng's
    `unknown_pc()` → `abort()`);
  - skip and report (PS2Recomp);
  - an interpreter or run-time compiler (DolRecomp, gbarecomp, JRickey's
    gba-recomp, jamulator, Rosetta 2's JIT).

  Every GBA tool keeps an interpreter, mostly for RAM code. Our dispatcher
  and fallback are the safe end of this range. What we could take is
  speed:
  - nested `dispatch::call` for indirect calls instead of unwinding
    (ReXGlue's slow path, Rosetta's lookup table plus return stack);
  - a direct-indexed key table instead of the binary search (XenonRecomp's
    "perfect hash", Rosetta's table of function starts).
- **Self-modifying code is out of scope for every tool except the GBA ones
  and the dynamic translators.** McSema's docs say so outright.
  gbarecomp and JRickey check the bytes or a hash of RAM code before
  running it, as our overlay guards do (decision 7). gpsp keeps a tag per
  RAM halfword, compares it on every store and flushes its RAM cache on a
  hit. Our guards are the static form of the same thing, and none of the
  techniques reviewed is better for us.
- **Build cost is the constraint the recompilers talk about most, and Rust
  is worse at it than C.**
  - gbarecomp: 201.7 MiB of C++ for Minish Cap. 10–22 min as one GCC
    translation unit, 56 s as 16 Clang shards (i9-9900K).
  - Our Emerald: 286 MiB of Rust, 815k instructions. 7 min 19 s under
    8 GB, as 42 part crates, before the lean printer.
  - JRickey's gba-recomp chose to emit C rather than Rust: its architecture
    doc says rustc takes more than 30 min and 32 GB on single huge
    generated files.
  - XenonRecomp's registers-as-locals options cut 20 MB from Sonic
    Unleashed's executable.

  Structuring must not grow the code, and two bodies per function would.

## What we start from

The code is described in [codegen-design.md](../codegen-design.md) and, for
structuring, in the companion review's "What we start from". In short:

- one Rust fn per guest function, `loop { match pc }` over its blocks
  (decision 8);
- all guest state in `Machine` at every block boundary;
- a tick at every block exit with that edge's static cycle count
  (decision 11);
- flags computed only where flag liveness over the function's static edges
  says they are live (decision 9);
- every block start enterable;
- IRQs, halts and frame ends unwind the whole host stack to the
  dispatcher, which takes them;
- IWRAM overlays checked against their bytes in each block's arm
  (decision 7).

### What "exact" asks, for these techniques

The unit's six concerns, with the rule each technique below is judged by:

| Concern | Exact if |
|---|---|
| Every flag a later instruction may read | A flag write is dropped only when no path to a reader exists before a point where all flags are live. The yield return of a tick is such a point, since an IRQ handler may save the SPSR (below). |
| Cycle counts and tick points | Each CFG edge's cost is added by the same `rt::tick` at the same place. A tick that moves changes what `VCOUNT`, timer and DMA state the next I/O read sees, and when IRQs land. |
| I/O order | Each bus access runs once, in program order. Nothing reads an I/O register twice or moves an access past a tick. |
| IRQs | An IRQ is taken at the tick where it is now, with `cpu.pc` the next block's key. How the host gets there (unwinding or a nested call) doesn't matter if the guest state is the same. |
| Self-patching RAM code | Every run of compiled RAM code is guarded by a check of the bytes it was compiled from, at least once per block, since a block can patch the next one. |
| The interpreter fallback | Any key compiled code doesn't own goes to the dispatcher, which runs the interpreter. Nothing is fatal. |

### Our numbers, for comparison

From `out/pokemon-emerald-version` (generated 2026-10-22, after the lean
printer and the timing ops; counts by `grep -o` over `src/` and `parts/`):

| | Emerald |
|---|---|
| Guest instructions in blocks | 814,601 (in 112,843 blocks, 15,158 functions) |
| Generated Rust | 286 MiB (299,865,140 bytes), 6,675,381 lines in 42 part crates |
| Per guest instruction | 368 bytes, 8.2 lines |
| `rt::tick` calls (one per CFG edge) | 153,229, plus 194 `rt::tick_ram` |
| `rt::yielding` checks after calls | 54,075 |
| `return Flow::Jump` (yields and leaving the function) | 222,740 |
| Dynamic exits (`rt::exit`) | 16,410 |
| Packed CPSR writes, `m.cpu.cpsr = …` | 75,892, plus 30,008 single `set_flag`s |
| Overlay guards | 97 |
| Build | 7 min 19 s under an 8 GB cap, largest process 2.3 GB (`shared/emerald-crate-build`, before the lean printer); peak 7.2 GB with 6 jobs (open-decisions.md) |

## The tools

### Console recompilers

**N64Recomp** (MIPS → C; with N64ModernRuntime):

- **Shape.** One C function per guest function, `void func_X(uint8_t*
  rdram, recomp_context* ctx)`. Control flow inside is flat
  `goto L_xxxxxxxx` over labels collected in a first pass
  (`src/recompilation.cpp:763-873`).
- **Function boundaries** come from an ELF or a symbols TOML. Only
  "static" functions reached by `jal` are found by the tool.
- **Jump tables** are matched by a linear register scan
  (`src/analysis.cpp`). They become `switch` with
  `default: switch_error(...)`, which asserts and exits.
- **Indirect calls** go through `LOOKUP_FUNC`, a hash-map lookup. A miss
  is fatal (`librecomp/src/overlays.cpp:364-372`).
- **Overlays** are recompiled ahead of time, with relocation macros. Self-
  modifying code is not handled.
- **Interrupts and timing.**
  - Generated code checks nothing and counts no cycles.
  - `osGetCount()` returns wall-clock time.
  - Each libultra thread is a host `std::thread`, but only one runs at a
    time, and they switch only inside OS calls.
  - "Interrupts" are host threads posting libultra messages on a wall-clock
    schedule.
- **Registers.** GPRs live in a context struct. HI and LO are function
  locals, which is an ABI bet.
- **Size and compile time:** none reported.

*Exact for us?* The shape is; the run model isn't. Without an OS layer, a
recompiler that never yields can't take an IRQ at all. What transfers:

- the function-per-function shape (the companion review's Ramsey
  translation is its structured form);
- the fatal-miss policy, as a warning of what we avoid.

**XenonRecomp** (Xbox 360 PowerPC → C++) and **ReXGlue** (its descendant,
with Xenia):

- **Shape.** The same: one function per guest function, `goto loc_X`, `b`
  out of the function as a call plus `return`.
- **Jump tables** come from patterns (`mtctr; bctr`). XenonRecomp's are
  tuned to one game; ReXGlue detects four kinds.
- **Indirect calls.** XenonRecomp's go through a table load with no check:
  `(guest − code base) × 2` gives a slot of 8-byte host pointers past the
  image (`XenonUtils/ppc_context.h:110-113`), and an empty slot is a null
  call. ReXGlue bounds-checks, and on a miss falls back to a dispatcher
  across every loaded module.
- **Flags.** The condition register is computed eagerly: each CR field is
  four bytes set by one `compare` (`ppc_context.h:147-165`), and XER's
  carry is computed per op. There is no liveness pass; Clang deletes the
  dead stores.
- **Registers as locals.** The `ctr_as_local`, `cr_as_local`,
  `non_volatile_as_local` and similar options move registers into C locals.
  - The README says this is valid only if the game follows the PowerPC ABI,
    and to enable them only "after having a successfully functioning
    recompilation".
  - Its one number: "the executable size decreases by around 20 MB with
    these optimizations, and frame times are reduced by several
    milliseconds".
- **Other.** No exceptions, no interrupts, no cycles (`mftb` is the host
  TSC), no interpreter.

*Exact for us?*

- Eager flags with compiler DCE: yes, but weaker than ours. Our liveness
  sees across blocks; Clang's dead-store pass can't see past a store to the
  context struct.
- The unchecked table: no. A miss must reach the dispatcher.
- Registers as locals is the registers review's topic. The lesson here is
  that the only reported win (20 MB, a few ms) came with an ABI assumption
  that the authors themselves gate behind "the game already works".

**PS2Recomp** (R5900 → C++):

- **Shape.** One function per guest function, entered through
  `switch (ctx->pc) { case X: goto label_X; ... }` over its resume points
  (`function_emitter.cpp:91-103`). The resume points are:
  - every call's return address;
  - every `syscall`+4;
  - every loop head (`control_flow_analyzer.cpp:104-197`).

  `ctx->pc` is stored before every instruction.
- **Timing.** Back edges emit `if (runtime->eeCheckpointDue()) return;`
  and charge a nominal 32 cycles. Each dispatch charges 8.
- **Interrupts and threads.** On a due checkpoint, generated code returns
  to one scheduler. The scheduler runs events, interrupts (as a separate
  invocation) and thread switches, then resumes by pc. Blocking kernel calls
  unwind by C++ exception.
- **Calls.** `dispatchGuestBranch` returns true only if the callee left
  `ctx->pc` at the return site. Otherwise the caller returns too.
- **Misses.** A missing function is reported and skipped or stopped,
  per policy.

*Exact for us?* Its structure is ours: resume switch, central dispatcher,
guarded returns. Its timing isn't: nominal costs at back edges only. What
transfers is the restricted resume set. Yields only at back edges and calls
mean the only interior entries are loop heads and return sites. That would
make a structured body with a few entry points practical. But for us it
moves IRQs and tick points, so it changes timing (the tick rule above).

**DolRecomp** (GameCube/Wii → C or LLVM):

- **Shape.** Not functions: fixed chunks of up to 4,096 instructions
  (128 for the LLVM back end). Each chunk has a `switch (ctx->pc)` with a
  case for every instruction (`src/backend/emitter.c:1918-1928`).
- **Timing.** `ctx->downcount -= N` at each block leader. Back edges return
  when the budget is spent. Per-opcode costs are rough (`divw` 40, default
  1).
- **Calls.** A cross-chunk `bl` resumes inline only "if the callee came
  back to the instruction after the call" (`emitter.c:344-375`).
- **Fallback.** Unsupported instructions call a host-supplied
  `instruction_fallback`. That design assumes an emulator that can run any
  pc.
- **Self-modifying code** is "reported … so [it] can be reviewed and
  patched manually".
- **Flags.** The packed CR word is updated eagerly in C. The LLVM back end
  promotes state to SSA under an ABI policy flag.

*Exact for us?* Mostly ours already, at a finer grain. The every-instruction
entry switch is v1 with smaller blocks. Nothing new is exact and cheaper.

### GBA recompilers

These are the closest prior art and were unknown to this project before
this review. All four are active in 2026.

**mstan/gbarecomp and arm-recomp-core** (ARMv4T → C++). It reports
ports of Minish Cap, Emerald, Mother 3 and others.

- **Shape.** A function per discovered function, `L_x:` labels and `goto`.
  `bl` is a host call, after a push to a shadow return stack. An exit from
  the function is a `musttail` call or a dispatch.
- **Resume entries.** With `static_resume_all` (the default), every
  instruction gets a resume entry (`docs/TOML_SCHEMA.md:122`).
- **Every instruction** writes `R[15]` and checks
  `if (runtime_should_yield()) return;`
  (`profiles/armv4t_gba/arm_codegen.cpp:1540-1556`).
- **Timing.** Cycles accumulate per instruction, with mGBA's model (wait
  states, prefetch, multiply cycles), into one `runtime_tick` per block.
  - The device state is materialised only at the next event horizon.
  - IRQ eligibility is checked every instruction, and **IRQs are taken
    inside `runtime_tick` as nested host calls** (`g_irq_nest_depth`).
  - Its accuracy list still says the take-point is "edge-quantized, not
    cycle-exact" (`GBA_ACCURACY_BURNDOWN.md:181-184`).
- **Flags** are eager (`arm_set_nzcv_add` writes `cpsr` on every S op).
  No liveness pass was found.
- **Fallback, in three tiers:**
  1. static code;
  2. "heal shards": C for a missed pc, compiled at run time by gcc or tcc
     and cached;
  3. an interpreter, "loudly logged".

  RAM code is byte-checked before it runs.
- **Validation.** A co-simulation oracle against pinned mGBA, with a
  full-state hash on strides of the cycle counter (`COSIM_ORACLE.md`), and
  a forced-interpreter build for differential runs.
- **Size and time** (`docs/BUILD_PERFORMANCE.md`): "the 201.7 MiB Minish Cap
  corpus" (44,584 functions):

  | Build | Time |
  |---|---|
  | One GCC translation unit | 10 min 34 s – 21 min 37 s |
  | 16 GCC shards, 8 jobs | 1 min 59.6 s |
  | 16 Clang shards | 55.9 s |
  | Clang, full build and link | 1 min 3.8 s |

  Monolithic builds are now refused (2 to 64 adaptive shards).

*Exact for us?*

- Its timing model matches mGBA as ours does.
- **The nested IRQ is exact if guarded** (below), and it is the one big
  technique here we don't have.
- Per-instruction yield checks and resume entries cost a compare and a
  store per instruction. We don't need them: compiled code takes IRQs only
  at block boundaries (decision 11), so only block starts are resumed.
- Its build numbers are the best comparison for ours: 201.7 MiB of C++
  builds in about a minute sharded, against our 286 MiB of Rust in more
  than 7 min.

**JRickey/gba-recomp** (a Rust tool that emits C11):

- **Shape.** Per basic block. Blocks return the next key to a trampoline,
  so guest calls are not host calls. Union-find "chain groups" of up to
  8,192 instructions turn static edges into `goto`. Back edges are gated:
  `if (a->chain_gate(m)) return key;` (`crates/recomp-core/src/emit.rs`).
- **Fallback.** Hard instructions (`swi`, `mrs`/`msr`, swaps, writes to pc)
  run through `a->interp_one(m)` with a pc check, "so it can never be wrong".
  RAM blocks are hash-guarded.
- **Timing.** A tick per block exit. IRQs are checked at block edges,
  "delivery delayed a few instructions — empirically fine".
- **Flags.** Per-flag liveness is promised in the docs, but not found in
  the code.
- **Why it emits C, not Rust** (`docs/architecture.md:63-64`): "rustc has
  documented pathologies on huge machine-generated files (>30 min/>32 GB
  RAM on single generated files, both codegen backends …)". Its `emit.rs`
  says a full translation "can exceed the source image by two orders of
  magnitude".

*Exact for us?* Its design is close to our v1 in Rust-free form. The rustc
warning is what our phase 1 ran into: rustc's front end needs ~55 KB per
instruction (open-decisions.md). It argues against any structuring that
duplicates code.

**Asphaltian/GBARecomp** (→ C#):

- **Shape.** One method per function, from a decomp's symbols. `goto`
  inside.
- **Jump tables** (agbcc's Thumb pattern) become `switch` with a throwing
  default.
- **Timing.** `Scheduler.Cycles += cost` per block. The event check runs
  only at function entry, at loop heads, and after `msr` to the control
  field.
- **Flags.** `FlagLiveness.cs` is a backward dataflow over NZCV with all
  flags live at calls, tail calls, indirect calls and indirect jumps. That
  is ours (decision 9).

**Kirby & the Amazing Mirror recomp** (Python tool → C):

- **Shape.** One C function per guest function, `goto`. Indirect targets go
  through `rt_lookup`.
- **RAM code** is emitted as overlays "keyed by address and body hash", with
  an interpreter fallback.
- **Validation.** About 420 instructions per mode are compared against
  Unicorn, with ARM7TDMI-specific cases left out (Unicorn is ARMv5). Calls,
  IRQs and `swi` are not covered.
- **Hard cases noted:** direct calls find "fewer than 10% of the functions"
  (function pointers); `bl` is used as a long jump; `pop {r0}; bx r0`
  returns. We handle all three already (decisions 5a and 8).

### Older work

- **UltraHLE** (1999). No primary source on its CPU core was found, so it
  isn't known whether it recompiled statically. Its lasting idea is HLE at
  the library level, which N64ModernRuntime still follows.
- **Graham Toal, "An Emulator Writer's HOWTO for Static Binary
  Translation"** (c. 2005). Start from the interpreter as a
  `switch (pc)` with one case per instruction. Then fall through between
  cases, keep labels only at jump targets, and optimise the blocks.
  - On flags: "most of [the code] is coming from the setting of flags which
    are never used". Remove them by def/use.
  - On timing: interrupts and the cycle counter "only need to be done when
    you execute a branch backwards".
  - On indirect jumps: profile to find their targets, and send misses to
    the interpreter.
  - On validation: a dual-CPU harness comparing registers after every
    instruction.

  This is our v1 and decisions 6 and 9, two decades early.
- **Andrew Kelley, jamulator** (2013, NES → LLVM). The post the unit
  attributes to Michael Steil is Kelley's.
  - It ticks after each instruction.
  - `JMP (ind)` is a switch over known labels, falling back to an
    interpreter.
  - The NMI handler is called from inside the tick on the native stack,
    with `RTI` as `return`: "elegant but imperfect", since it breaks on
    games that play tricks with RTI.
  - His conclusion: static recompilation "is thwarted by the inability to
    completely disassemble a game without executing it". That is decision 6's
    reason for profile-guided discovery, and the reason the fallback must
    never be fatal.

### Binary lifters

**McSema and Remill** (Trail of Bits; Anvill is the successor):

- **Remill's job.** It lifts one instruction at a time into calls to C++
  semantics functions over a `State` struct, then inlines them.
  - Memory goes through intrinsics that thread a `Memory *` token, "to
    enforce a total order across all memory writes".
  - Control flow it can't resolve becomes intrinsics:
    `__remill_jump`, `__remill_function_call`, `__remill_missing_block`
    and `__remill_async_hyper_call`. The client defines them.
- **Flags.** Each is a separate byte in `State`, with `volatile` padding
  bytes between them so that LLVM can't merge the stores
  (`include/remill/Arch/AArch32/Runtime/State.h:100`).
- **Predication.** A predicated instruction is an `if` around its semantics
  (`DEF_COND_SEM`).
- **AArch32 semantics.** The hand-written ones were deleted in 2023 in
  favour of Ghidra's SLEIGH.
- **McSema** takes its CFG from IDA. It keeps `State` in one thread-local
  global, then runs SROA, mem2reg and a State-aware dead-store pass.
  - That pass is liveness over State slots. It marks everything live at
    `ret`, `indirectbr` and indirect calls ("Code that we return to or
    branch to could read out registers so mark as all live"), and keeps
    only the argument slots live at a direct call.
  - **Indirect jumps** become a `switch` over IDA's successors, falling
    back to `__remill_jump`.
  - Its docs: "The translator is not designed to accommodate
    self-modifying code", and "We do not support raw binary blobs (e.g.
    firmware)".
- **Anvill** gives each function its own copy of `State`. That is sound
  only with a declared ABI.
- **Validation.** Remill's x86 and AArch64 tests run each case natively and
  lifted from the same state and `memcmp` the two States, with undefined
  flags masked. ARM and Thumb get only a few hand-written cases.
  Dasgupta et al. (PLDI 2020) found 29 bugs in McSema's lifting by
  translation validation.
- **Size and time:** none reported.

*Exact for us?*

- Global state plus liveness that ends at unknown boundaries: yes. It is
  our state model exactly.
- The validation harness (same start state, run both, compare all state,
  mask what's undefined) is `just sst-gen`'s form.
- Per-function State (Anvill) is the ABI bet again.

**rev.ng** (QEMU's TCG as the lifter, → LLVM):

- **Shape.** Everything is lifted into one `root` function. Unknown targets
  go to a dispatcher `switch` on the pc whose default calls `unknown_pc()`,
  which aborts (`lib/Lift/JumpTargetManager.cpp:1090-1122`,
  `share/revng/support.c:305-318`).
- **Jump targets** are found by iterated value analysis (OSRA in the CC '17
  paper). It needs "condition numbering" to reason about predicated ARM
  code.
- **Flags.** ARM flags come in QEMU's layout (below).
- **Numbers (CC '17):** CFG recovery for ARM `ls` took 119.53 s (50.53 s on
  a later branch), against IDA's 5.39 s. Recompiled SPEC2006 int ran
  3.439× slower than native, against QEMU's 5.680× (ICCST '18). No
  output-size figure was found.

*Exact for us?* Its dispatcher is ours with a fatal default. Its abort is
the policy our fallback replaces.

**RetDec** (Avast; in limited maintenance):

- **State.** Every register is an LLVM global, and N, Z, C and V are four
  `i1` globals.
- **Known bug:** `ANDS` and `TST` never write the shifter carry into C. The
  code says so in a comment (`src/capstone2llvmir/arm/arm.cpp:866-875`).
- **Register localization** turns every register into a per-function local,
  with no liveness and no marshalling at calls. So a flag set by a caller
  and read by a callee is silently lost. That is fine for decompiling C,
  and wrong for an executable.
- **Jump tables** are matched as `load(add(shl(idx), table))` with a
  dominating bound.

*Exact for us?* No: it is a decompiler's model and drops flags across
calls. Its ARM tests (212 hand-written expectations) found neither bug.

**Ghidra's SLEIGH and P-code**:

- **Flags.** Each flag is a one-byte register varnode (`NG ZR CY OV`).
  Every flag-setting instruction computes `tmp*` values, and the S-bit
  subconstructor commits them (`ARMinstructions.sinc:703-763`).
- **Predication** is `if (!cc) goto inst_next`.
- **Gaps for the GBA:**
  - `mrs Rd, cpsr` builds only the flag bits; mode, I, F and T read as zero
    (`ARMinstructions.sinc:4251-4256`).
  - `msr cpsr, #imm` doesn't update the flags.
  - Only supervisor-mode banks exist.

  It is a user-mode decompilation model, not an emulator's.

*Exact for us?* Not as a semantics source. Our semantics are shared with the
interpreter and checked by SingleStepTests (decision 3).

**llvm-mctoll** (Microsoft; raises ELF to LLVM IR):

- **Flags.** N, Z, C and V are four `i1` allocas per function, set to
  false at entry (`ARM/ARMSelectionDAGISel.cpp:92-101`). So flags never
  cross a call or return.
- **Bugs, from reading the code** (not running it):
  - SUB's carry is computed as `uadd(~S1, S0 + 1)` with a plain add first
    (`ARM/DAG/IREmitter.cpp:263-268`), so `S0 = 0xFFFFFFFF` gives C = 0
    where ARM gives 1.
  - Logical ops never set C.
  - Shift carries ignore shift-by-0 and shifts of 32 or more.
- **Indirect jumps.** `bx reg` becomes an `indirectbr` with no destination
  list and no fallback.
- **Validation** is end-to-end on program output (lit tests).
- **Paper:** Yadavalli and Smith's LCTES 2019 paper couldn't be fetched
  (ACM returned 403), so its numbers aren't checked here.

*Exact for us?* No. It is the clearest example of what per-function flag
locals with per-program tests miss.

**Lifter correctness studies.** Kim et al. (ASE 2017) compared BAP, PyVEX
and BINSEC on x86 with an SMT solver and found 24 semantic bugs, five of
them in flag computation. No study of ARM lifters' flags was found.
SingleStepTests, run against both our interpreter and generated code, is
stronger than anything these tools have for ARM.

### Dynamic translators

**QEMU TCG** (Bellard, USENIX ATC 2005; source at `d7314928`):

- **Flags, x86 guest.** Lazy: `CC_OP`/`CC_SRC`/`CC_DST`, with a backward
  pass inside the block and, as the 2005 paper says, "At the end of TB we
  consider that these variables are used".
- **Flags, ARM guest** (the one that matters for us). No lazy `CC_OP`.
  NZCV are four separate 32-bit values (`target/arm/cpu.h:288-292`):
  - `CF` is 0 or 1;
  - `NF` and `VF` hold the flag in bit 31;
  - `ZF` holds the result, so Z is set iff `ZF == 0`.

  So N and Z cost one move each, and C falls out of a double-width add.
  `pstate_read` composes the CPSR only when something reads the whole
  word. Dead flag stores are removed by TCG's generic liveness pass, inside
  a block only. Every global is synced to memory at block end and before
  any helper not marked as leaving globals alone (`tcg/tcg.c:3691-3732,
  3953-3964`).
- **Interrupts** are checked at every block entry (a negative test of
  `icount_decr`), so latency is at most one block (≤ 512 instructions).
  Writes to the CPSR end the block so that an unmasked IRQ is seen.
- **icount** counts instructions, not cycles. To hit a timer deadline
  exactly, it retranslates a block with exactly the right number of
  instructions. After an I/O access in mid-block, it re-runs from a
  one-instruction block. It is "incompatible with multi-threaded TCG".
- **Self-modifying code.** Stores to pages holding code take a slow path
  that invalidates the overlapping blocks. If the running block is hit,
  execution single-steps.
- **Indirect jumps** go through a 4,096-entry per-CPU jump cache, then a
  global hash table.

*Exact for us?*

- Liveness ending at block end: yes, and it is why QEMU has no stale-SPSR
  problem.
- The ARM flag layout: yes, if every reader of the whole CPSR (`mrs`, IRQ
  entry's SPSR copy, the frame-end state, `--regs`) composes through one
  accessor, since the interpreter and generated code share `Cpu`. It turns
  our 75,892 read-modify-write CPSR updates into plain stores. Its cost is
  a compose at each whole-CPSR read and a changed `Machine` layout.
- icount's exact deadlines show what exact interrupt timing costs:
  retranslation and single-instruction blocks after I/O. We chose block
  granularity instead (decision 11).

**Rosetta 2** (Apple; x86-64 → arm64):

- **Apple-stated.** Translation is ahead of time, at install or first
  launch, and cached. Code not seen then, including JIT output, is
  translated on the fly ("we'll compile it on the fly"). Nothing is
  published on flags, branches or self-modifying code.
- **Reverse-engineered by Dougall Johnson** (2022):
  - x86 flags live in the host's NZCV, with an undocumented M1 mode for
    PF and AF and a carry kept in inverted form (`CFINV`).
  - It "avoids computing flags when they are unused and don't escape".
  - Indirect branches use a lookup table holding all function starts plus
    other entries, falling back to the JIT.
  - `CALL` and `RET` become `BL` and `RET`, checked against a separate
    return stack.
  - Code grows about 1.64× (sqlite3: 1.05 MB of x86 to 1.72 MB of arm64).
- **Reverse-engineered by Koh M. Nakagawa** (Project Champollion, 2021):
  the debug IR dump shows per-block `flags liveout #nzcvpa` lines. Reading
  them as flag liveness is an inference.

*Exact for us?* Its split is ours: ahead-of-time code, with the interpreter
standing in for its JIT. The guarded return stack is our guarded return. Its
1.64× growth is for one host instruction per guest instruction. Ours is 368
bytes of source per instruction, which says how much the printer still
costs before LLVM.

**FEX-Emu and Box64** (x86 → arm64):

- **FEX.** `RedundantFlagCalculationElimination.cpp` is a worklist liveness
  over a whole multi-block unit. Only a conditional or direct jump to a
  block in the unit narrows the set; every other exit is all-live (lines
  523-604).
- **Box64.** Deferred flags (an op-and-operands record, QEMU's `CC_OP`),
  plus a backward "need" analysis over the block graph (`updateNeed`,
  `src/dynarec/dynarec_native.c:303-343`). Self-modifying code is caught
  by page protection plus a hash of each block, checked again on its next
  entry.

*Exact for us?* Both are decision 9, over a region instead of a function.
Lazy flags can't help us: there is nowhere to keep them lazy across a yield.

**gpsp** (libretro; a GBA dynarec):

- **Flags.** Thumb gets a backward three-mask liveness per block, with
  everything live at block end (`cpu_threaded.c:2984-2996`). ARM mode has
  none: `arm_dead_flag_eliminate()` is `flag_status = 0xF`, "For now this
  just sets a variable that says flags should always be computed".
- **Timing.**
  - Each instruction adds a fixed per-region, all-sequential cost, with
    guesses like `cycle_count += 64; /* Big under-estimation here */` for
    `swi`.
  - The sum is subtracted from a down-counter at block exits.
  - When the counter goes negative, `update_gba()` runs the hardware and
    raises IRQs.
  - IRQs caused by an I/O write are raised right after the store.
  - Idle loops are skipped by per-game overrides.
- **Self-modifying code.** Every RAM halfword has a shadow tag, and every
  RAM store (DMA too) compares it. A hit flushes the whole RAM translation
  cache.

*Exact for us?*

- Its flag liveness: yes, and ours is wider (function-wide, ARM too).
- Its timing: no. It is an approximation of the kind decision 11 replaced
  with mGBA's model.
- Its RAM tags are a run-time alternative to our overlay guards. They cost
  a compare on every RAM store, against our compare per guarded block. For
  ahead-of-time code ours is the better trade.

## Techniques, by concern

Each row is a technique, who uses it, whether it stays exact for us, and its
cost.

### Condition flags

| Technique | Used by | Exact for us? | Cost |
|---|---|---|---|
| Compute eagerly, compiler DCE | XenonRecomp, ReXGlue, DolRecomp (C), gbarecomp, Remill/McSema before their pass | Yes | Slow; DCE can't see past stores to `Machine` |
| Liveness, all live at unknown boundaries | QEMU (per block), gpsp (Thumb block), FEX, Box64, McSema's pass, Asphaltian, **ours** (decision 9, per function) | Yes, with the yield fix below | Built |
| All live at a yield: dropped writes computed on the tick's return path | QEMU, gpsp (implicitly: their yield is a block end) | Yes; makes the IRQ handler's saved SPSR exact | Code size on cold paths only; the printer must keep the dropped values computable there |
| Split NZCV fields, Z as "result == 0" | QEMU (ARM guest), rev.ng | Yes, if one accessor composes the CPSR for every whole-word read | Plain stores instead of 75,892 read-modify-writes; a `Cpu` layout change shared with the interpreter |
| Lazy op-and-operands (`CC_OP`) | QEMU (x86), Box64, Valgrind | Exact only if forced at every yield, call and dynamic exit, so little survives liveness | A run-time tag check at each reader; not worth it for ARM, whose flags are cheap |
| Interprocedural flag summaries ("callee reads none at entry") | none found | Yes, if checked (decision 10's modular rule) | Cross-call liveness; flags rarely cross calls in compiled code |
| Per-function flag locals, no marshalling | mctoll, RetDec, Anvill | **No:** flags across calls silently lost | — |

### Cycles, tick points and interrupts

| Technique | Used by | Exact for us? | Cost |
|---|---|---|---|
| Tick at every CFG edge, static cost, IRQs at block boundaries | **ours**, JRickey, gpsp (approximate costs) | Yes (baseline) | One add-and-compare per edge |
| Yield check only at back edges and calls | PS2Recomp, DolRecomp, Toal, Asphaltian (function entry and loop heads) | **No** as an optimisation: IRQs land later and `VCOUNT`/timer reads see an older clock. Only as an experiment checked by the frame hash | Fewer resume entries |
| Per-instruction yield check, every instruction resumable | gbarecomp | Yes, finer than we need | A store and a compare per instruction |
| Exact deadlines by retranslation, a block ending after I/O | QEMU icount | Exact to the instruction | Retranslation; not available ahead of time |
| **IRQ taken inside the tick as a nested call, guarded on return** | gbarecomp (nested), jamulator (unguarded) | **Yes:** same tick, same pc, same handler; unwind if the handler returns elsewhere | Host stack depth per nested IRQ; the frame end and halts need a non-unwinding path too |
| IRQs as host messages, no preemption | N64ModernRuntime, ReXGlue | No: needs an OS layer | — |

### Indirect jumps and calls

| Technique | Used by | Exact for us? | Cost |
|---|---|---|---|
| Switch over known targets, fallback arm to the dispatcher | **ours**, McSema, rev.ng, PS2Recomp | Yes, when the default isn't fatal | Built |
| Fatal default | N64Recomp, XenonRecomp, ReXGlue, rev.ng | No | — |
| Guarded return (continue only if the callee came back to the return site) | **ours** (the `match`), DolRecomp, PS2Recomp, Rosetta 2 | Yes | Built |
| Nested lookup for indirect calls instead of unwinding | ReXGlue's slow path, Rosetta 2, our `rt::call` for stub calls | Yes, with the same guard | Host stack per call; fewer resumes |
| Direct-indexed key table | XenonRecomp (×2 slots), Rosetta 2 (function starts) | Yes (same lookup result) | Memory: a flat ROM table is 128 MB of slots, so it would need two levels; not for the ESP32 as is |
| Interpreter or run-time compiler for misses | DolRecomp, gbarecomp, JRickey, jamulator, Rosetta 2 | Yes | Built (the fallback) |

### Self-modifying and RAM code

| Technique | Used by | Exact for us? | Cost |
|---|---|---|---|
| Byte or hash guard before running RAM code | **ours** (per block), gbarecomp, JRickey, Kirby | Yes | One compare per guarded block |
| Tag per RAM halfword, checked on every store | gpsp | Yes | A compare on every RAM store, everywhere |
| Page protection and invalidation | QEMU, Box64 | Yes | Needs an MMU; not on the ESP32 |
| Out of scope or manual patches | McSema, N64Recomp, XenonRecomp, DolRecomp | No | — |

## Output size and compile time reported

| Tool | Input | Output | Build |
|---|---|---|---|
| **gbaa** (2026-10-22) | Emerald, 815k instructions | 286 MiB Rust, 42 part crates | 7 min 19 s under 8 GB (before the lean printer) |
| gbarecomp | Minish Cap, 44,584 functions | 201.7 MiB C++ | 56 s, 16 Clang shards; 10–22 min as one GCC unit (i9-9900K) |
| JRickey gba-recomp | — | "can exceed the source image by two orders of magnitude" | rustc: ">30 min/>32 GB RAM on single generated files", hence C |
| XenonRecomp | Sonic Unleashed | 20 MB smaller with registers as locals | none |
| Rosetta 2 (RE) | sqlite3 | 1.64× the x86 code | ahead of time, at install |
| rev.ng | ARM `ls` | none | 119.53 s CFG recovery (50.53 s later); run time 3.4× native |
| Kirby recomp | Amazing Mirror | a 122 MB release zip | none |
| N64Recomp, PS2Recomp, DolRecomp, ReXGlue, McSema, RetDec, Ghidra, mctoll | — | none found | none found |

## What our design should take

For `structured/design`:

1. **Keep decision 9's flag liveness and its rule that unknown boundaries
   read every flag.** Every tool that does better on paper does it with an
   ABI bet that breaks on hand-written code. Don't move flags into
   per-function locals without a checked summary (decision 10's rule).
2. **Make the IRQ yield point all-live.** On the return path of each tick,
   compute the flag writes liveness dropped on that edge. Then the CPSR
   that Emerald's `IntrMain` saves to the stack is the interpreter's. It is
   cheap because the path is cold, and it is the one place the current
   flags aren't exact in memory. Check it with a memory compare at a
   checkpoint, for `structured/lit-validation`.
3. **Take IRQs, halts and the frame end without unwinding.**
   - Run the IRQ handler as a nested dispatcher call inside the tick
     (gbarecomp). Continue in compiled code only if the handler returns to
     the expected key, and unwind otherwise.
   - Hand off the frame end without unwinding (the run thread already has
     its own 256 MB stack).
   - Run halts forward in place.

   This is exact by the table above. It is what makes the companion
   review's "two bodies" affordable: resumes become rare, so the v1 body
   can be emitted lazily, or left to the interpreter. Measure it first:
   the share of dispatcher entries at non-entry keys before and after.
4. **Keep one tick per CFG edge.** Don't merge ticks or move yield points
   to back edges as an optimisation (PS2Recomp, DolRecomp, Toal). For us
   that changes timing, so it is only an experiment the frame hash judges.
5. **Use nested `dispatch::call` for indirect calls, with the guarded
   return,** as stub calls already do (open decision 8's follow-up). It is
   the same pattern as Rosetta's return stack and ReXGlue's slow path, and
   it removes the other big source of resumes.
6. **Consider QEMU's split NZCV layout** as a separate, measurable change to
   `Cpu`, shared with the interpreter so the two can't drift. Do it only if
   the lean printer's remaining CPSR updates show in compile time or a
   profile.
7. **Never grow the code.** The Rust compile cost is what the field's only
   Rust-adjacent GBA tool fled from, and our biggest cost. So:
   - no node splitting;
   - no every-instruction resume switch;
   - nothing that duplicates blocks;
   - two bodies only if item 3 can't make resumes rare.
8. **Keep the fallback non-fatal and the RAM guards as they are.** Every
   GBA tool converged on an interpreter plus byte guards. The fatal
   policies of the console tools work only because they replace the OS.

## Sources

Read from the primary text or source, on 2026-10-24. Source trees at the
commit given.

Console and GBA recompilers:

- N64Recomp (incl. RSPRecomp, LiveRecomp), github.com/N64Recomp/N64Recomp
  at `ffb39cd`; N64ModernRuntime, github.com/N64Recomp/N64ModernRuntime at
  `cdf5abb`.
- XenonRecomp, github.com/hedge-dev/XenonRecomp at `ddd128b`.
- ReXGlue SDK, github.com/rexglue/rexglue-sdk at `c94f5eb`.
- PS2Recomp, github.com/ran-j/PS2Recomp at `e563698`.
- DolRecomp, github.com/ExpansionPak/DolRecomp at `02e27ac`.
- gbarecomp, github.com/mstan/gbarecomp at `7bb70ec`, and arm-recomp-core,
  github.com/mstan/arm-recomp-core at `61cdc51`.
- gba-recomp, github.com/JRickey/gba-recomp at `de4edf5`.
- GBARecomp, github.com/Asphaltian/GBARecomp at `d18ce17`.
- kirby-amazing-mirror-recomp,
  github.com/Felipe-Gomes-Amorim/kirby-amazing-mirror-recomp at `2bb49d0`.
- Graham Toal, "An Emulator Writer's HOWTO for Static Binary Translation",
  gtoal.com/sbt (c. 2005–2006).
- Andrew Kelley, "Statically Recompiling NES Games into Native Executables
  with LLVM and Go", andrewkelley.me/post/jamulator.html, 2013-06-07.
- Wes Fenlon, interview with Wiseguy (N64Recomp), Read Only Memo,
  2024-05-19.

Lifters:

- Remill, github.com/lifting-bits/remill at `56918a8c` (and `38ec19ad`, the
  last hand-written AArch32 semantics, before `ca80510`); McSema,
  github.com/lifting-bits/mcsema at `26368053`; Anvill,
  github.com/lifting-bits/anvill at `9948d26c`.
- Artem Dinaburg and Andrew Ruef, "McSema: Static Translation of X86
  Instructions to LLVM", REcon 2014 (slides).
- rev.ng, github.com/revng/revng at `352c75e3`. Alessandro Di Federico,
  Mathias Payer, Giovanni Agosta, "rev.ng: A Unified Binary Analysis
  Framework to Recover CFGs and Function Boundaries", CC 2017. Alessandro
  Di Federico, Pietro Fezzardi, Giovanni Agosta, "rev.ng: A
  Multi-Architecture Framework for Reverse Engineering and Vulnerability
  Discovery", ICCST 2018. Di Federico, LLVM Developers' Meeting 2016
  (slides).
- RetDec, github.com/avast/retdec at `94505857`.
- Ghidra, `Ghidra/Processors/ARM/data/languages/` and
  `GhidraDocs/languages/html/pcoderef.html`,
  github.com/NationalSecurityAgency/ghidra at `16400303`.
- llvm-mctoll, github.com/microsoft/llvm-mctoll at `ffd1e8ba`. S. Bharadwaj
  Yadavalli and Aaron Smith, "Raising Binaries to LLVM IR with MCTOLL (WIP
  Paper)", LCTES 2019, doi 10.1145/3316482.3326354 (not obtained; ACM
  returned 403).
- Soomin Kim et al., "Testing Intermediate Representations for Binary
  Analysis", ASE 2017. Sandeep Dasgupta et al., "Scalable Validation of
  Binary Lifters", PLDI 2020 (abstract only).

Dynamic translators:

- Fabrice Bellard, "QEMU, a Fast and Portable Dynamic Translator", USENIX
  ATC 2005, FREENIX track. QEMU, github.com/qemu/qemu at `d7314928`
  (`target/arm/`, `target/i386/tcg/`, `tcg/tcg.c`, `accel/tcg/`,
  `docs/devel/tcg-icount.rst`).
- Apple, "About the Rosetta translation environment" (developer.apple.com),
  and WWDC 2020 session 10686. Dougall Johnson, "Why is Rosetta 2 fast?",
  dougallj.wordpress.com, 2022-11-09. Koh M. Nakagawa, "Project Champollion",
  github.com/FFRI/ProjectChampollion at `b2c08320`.
- FEX-Emu, github.com/FEX-Emu/FEX at `9eb5d92c`; Box64,
  github.com/ptitSeb/box64 at `420eda69`.
- gpsp, github.com/libretro/gpsp at `5819380c`.

Not found: any primary source on UltraHLE's CPU core, on Neil Bradley's
"Orion" translator, or on Rosetta 2's handling of self-modifying code.
Dolphin, NanoBoyAdvance and mGBA's old dynarec were not read. The
pokeemerald IRQ handler is `src/crt0.s` in `$GBAA_DATA/src/pokeemerald`.

## Reproducing

The counts in "Our numbers" are `grep -rhoF '<pattern>' src parts
--include=*.rs | wc -l` in a generated `out/pokemon-emerald-version`, for
the patterns `insn!(`, `rt::tick(`, `rt::tick_ram(`, `rt::yielding(`,
`return Flow::Jump`, `rt::exit(`, `m.cpu.cpsr = `, `set_flag` and
`rt::overlay_valid(`. The size is `find . -name '*.rs' | xargs cat | wc
-lc` there. Each takes seconds. The tools' claims can be checked in their
trees at the commits above, by the file and line given.
