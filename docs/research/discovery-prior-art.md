# Discovery prior art

Research for issue #6 (decision 5, code discovery, and "Prior art we will
use" in [decisions.md](../decisions.md)). What gbadisasm, Ghidra and IDA do
to find ARM/Thumb code, what we borrow, whether our Emerald ROM matches
pret's build, how to get pokeemerald's symbols without committing them, and
a proposed accuracy check against those symbols.

Researched 2026-10-19 on quack. Every `file:line` pointer names a repo and
commit:

| Repo | Commit |
|---|---|
| camthesaxman/gbadisasm | `e35982bd105fd8b9bb497d955900f8375cdc9e60` |
| pret/pokeemerald (`master`) | `731ad5bfd6e6f265508d0efcca0ba42f9dcf5881` |
| pret/pokeemerald (`symbols` branch) | `dba968c67d85caf9595abe12a51ff739d4dc5937` |
| pret/agbcc | `da598c1d918402c42c0c0d7128ba14567f3175e9` |
| HexRaysSA/ida-sdk | `d51b0e7c920cf67e7724b56a3dd9a74f6dd84606` |
| ldebug/ida-gba-ldr | `a1078a17cf85538d0f6098f7c27a0223b770f1c8` |
| laqieer/ida_gba_stuff | `17cc5267422adfe6c7eb1789284854d303160d4b` |
| NationalSecurityAgency/ghidra | `c91ec18e6780a8a531a806a0660a9c28978a99a0` |

## Summary

- **The ROM matches.** `~/ROMs/gba/Pokemon - Emerald Version.gba` has the
  SHA1 pret expects, and a local pokeemerald build is byte-identical to it.
  The decomp's symbols are ground truth for this ROM.
- **Ground truth needs the ELF, not the published `.sym`.** pret's `symbols`
  branch strips the function flag and the Thumb bit. Building pokeemerald
  takes under a minute and gives `pokeemerald.elf`: 15,859 function starts in
  ROM (15,844 Thumb, 15 ARM).
- **gbadisasm alone finds almost nothing without hand seeds.** It follows
  `bl`/`b` and nothing else. Seeded with `AgbMain` and `IntrMain` it finds 298
  functions (recall 1.9%), because Emerald reaches most of its code through
  function pointers (callbacks, tasks, script commands).
- **A validated pointer scan fixes that, as decision 5d predicts.** Seeding
  gbadisasm's descent with every word-aligned odd ROM pointer whose target
  starts with Thumb `push {…, lr}` gives **precision 99.87%, recall 93.49%**
  (14,827 of 15,859). That is the baseline our discovery has to beat.
- What is left over is specific: dead code no pointer or call reaches (~600),
  address-taken functions without a `push {lr}` prologue (414; leaf
  functions such as `IntrDummy`), ARM code nothing points at (12: `Init`,
  reached only by `b`, and routines that only run after being copied to
  RAM), and agbcc's `bl`-as-far-jump inside big functions (14
  false starts).

## gbadisasm

pret's decomps were bootstrapped with gbadisasm. It is not under the pret
org: `github.com/pret/gbadisasm` does not exist (404 on 2026-10-19); the tool
lives at `github.com/camthesaxman/gbadisasm`. Its last commit is from
2020-01-06. The whole analyser is one 940-line file on Capstone 3.0.5-rc2.
`references.md` still has the pret URL (see "Corrections" below).

### How it works

Every pointer in this section and in "What we borrow" below is into
`camthesaxman/gbadisasm@e35982bd` and is written `FILE:LINE` (`disasm.c` or
`main.c`). A bare line number in
backticks, such as `504-510`, is in the same file as the pointer before it.

- **Seeds.** The only automatic seed is the ROM entry, `0x08000000` as ARM,
  plus the header as data (`disasm.c:931-935`). Everything else comes from a
  config file of `arm_func ADDR [name]` / `thumb_func ADDR [name]` lines
  (`main.c:94-167`). The repo's example config seeds four Ruby functions by
  hand, `IntrMain` and `AgbMain` among them (`pokeruby.cfg`), and PikalaxALT/gbadismgenconf exists to
  generate a config from a known-good ELF. In practice it was driven by
  hand-curated seed lists.
- **Worklist.** A flat label table (linear search, `disasm.c:42-93`); the
  loop takes any unprocessed code label and decodes linearly from it
  (`disasm.c:329-541`) until a return or an unconditional non-`bl` branch.
  Each label records its mode (ARM/Thumb); a block inherits the mode of the
  label it starts from (`disasm.c:346-348`). There is no CFG and no
  function-extent model.
- **Returns** are `bx` with condition AL, `mov pc, rX`, or `pop {…, pc}`
  (`disasm.c:138-164`). Any unconditional `bx` ends the block, including an
  indirect call.
- **Branches.** `b`/`bl` targets become labels in the current mode
  (`disasm.c:410-447`). A `bl` target is assumed to be a function, unless the
  halfword after the `bl` is a literal pool or zero padding, in which case the
  `bl` is taken as a far jump (`disasm.c:421-437`). A plain `b` target is
  demoted to a non-function label unless something already proved it a
  function (`disasm.c:438-445`), so a tail call by `b` hides a function.
- **Literal pools.** `ldr rX, [pc, #imm]` marks the pool word as data
  (`disasm.c:166-186`, `504-510`). Pools found this way are what separates
  code from data; gaps nobody labelled print as `.byte`.
- **Mode switches and indirect jumps.** A pool load or `adr` whose register is
  used by the **very next** instruction as `bx rX` (target mode from bit 0)
  or `mov pc, rX` (same mode) adds a function there (`disasm.c:483-531`,
  `297-316`). One instruction of lookahead, no data flow.
- **Mode after a return.** If code in the other mode is labelled right after
  a return, it is promoted to a function (`disasm.c:392-404`, `740-743`):
  hand-written ARM following Thumb is a separate function.
- **agbcc jump tables.** A five-state matcher over consecutive instructions
  (`lsl`; `ldr rX, [pc]`; `add`; `ldr rX, [rX]`; `mov pc, rX`, with one
  instruction of grace) reads the table address from the pool, then walks
  table entries as Thumb code labels until it hits the lowest target or a
  non-ROM word (`disasm.c:198-295`). Issue #5 owns jump tables; this is the
  agbcc pattern it should start from.
- **Bad instructions.** Instructions outside ARMv4T are skipped (2 bytes in
  Thumb, 4 in ARM) rather than ending the block (`disasm.c:318-327`,
  `357-376`). For a recompiler that is wrong: an undecodable word on a
  supposed code path means the path is data.
- **Alignment.** Function starts must be 2-aligned (Thumb) or 4-aligned (ARM);
  Thumb starts at `addr % 4 == 2` print as
  `non_word_aligned_thumb_func_start` (`disasm.c:769-791`). Three Emerald
  functions are like that.

### What happened on Emerald

Built at `~/.local/share/gbaa/src/gbadisasm` (`make`, a few seconds) and run
against the ROM; results scored with the check proposed below.

| Run | Starts found | Precision | Recall | Time |
|---|---|---|---|---|
| No config | 1 | – | 0.0% | <1 s |
| Seeds: `IntrMain` (ARM), `AgbMain` (Thumb) | 298 | 99.66% | 1.87% | 1 s |
| Raw pointer scan alone (every aligned odd ROM pointer) | 20,091 | 37.01% | 46.89% | – |
| Pointer scan, target starts with Thumb `push {…, lr}` | 6,997 | 99.97% | 44.11% | – |
| gbadisasm seeded with the two above + the `push` scan | 14,846 | **99.87%** | **93.49%** | 31 s |

With no config it stops after one function: `Init` loads `AgbMain` from the
pool, does `mov lr, pc`, then `bx r1`. The `mov` sits between the load and the
`bx`, so the one-instruction lookahead misses it, and the unconditional `bx`
is read as a return.

Of the 1,032 functions the best run misses:

| Kind | Count | Example |
|---|---|---|
| Thumb, not referenced by any ROM word or `bl` | 550 | `Unused_LoadBgPalette`, `CheckHeap` |
| Thumb, address taken, no `push {…, lr}` prologue | 414 | `IntrDummy`, `AnimCmd_end` |
| Thumb, reached only by `bl` from other missed (dead) code | 55 | `CheckMemBlockInternal` (called only from the unused `CheckHeap`) |
| ARM, no ROM word or `bl` points at it | 12 | `Init` (reached by `b`, so never a function to gbadisasm), `SoundMainRAM_Unk1`, `sio32intr_clock_master` |
| ARM, address taken | 1 | `IntrSIO32` |

Of the 19 false starts, 14 are `bl` targets inside large functions
(`HandleTurnActionSelectionState`, `Cmd_moveend`, `PokemonUseItemEffects`,
`BattleStringExpandPlaceholders`): agbcc emits `bl` as a long unconditional
branch when a function is too big for `b`, for example `bhs _0803CD1E; bl
sub_0803BE9C; _0803CD1E:` in `HandleTurnActionSelectionState`. Every `bl`
that reaches these 14 targets sits inside the same function as the target
(checked against the ELF's function extents). gbadisasm's "followed by a
pool or padding" test does not catch them. Three more are
labels inside hand-written asm (`umul3232H32`, `SoundMainRAM`, `MPlayMain`)
and two are rodata bytes that look like `push`.

### What we borrow

- **The structure, not the code.** Worklist of (address, mode) labels;
  linear decode per block; pools marked as data when loaded. Recursive
  descent is decision 5a already; gbadisasm confirms it is enough to separate
  code from pools on agbcc output.
- **The return set** (`bx` AL, `mov pc`, `pop {pc}`), except that `bx rX`
  after `mov lr, pc` is a call. Our const-prop (5b) replaces the
  one-instruction lookahead; Emerald's own `Init` shows the lookahead fails on
  the first function.
- **The far-jump `bl` heuristic, extended.** Keep "`bl` followed by a pool or
  zero padding is a jump". Add: a `bl` whose target lies inside the address
  range of the function that contains the `bl` (from its start to the
  furthest address descent reached in it) is a far jump, not a call.
  Measured with the ELF's function extents, that removes all 14 agbcc false
  starts; with extents from our own descent it is untested. Emitting a far-jump `bl` as a Rust call would be wrong (lr is
  clobbered and nothing returns), so this matters for codegen, not just for
  the accuracy number.
- **`b` targets can still be functions** (tail calls). Do not demote a label
  that the pointer scan or a `bl` also reached, the way gbadisasm's `isFunc`
  does (`disasm.c:297-316`, `419`).
- **Mode change after a return starts a new function**
  (`disasm.c:392-404`).
- **The agbcc jump-table pattern** (`disasm.c:198-295`) as the starting point
  for issue #5.
- **Not borrowed:** skipping undecodable instructions; treating every
  unconditional `bx` as a return; config-file seeding as the main seed source
  (the pointer scan and runtime seeds of decision 6 replace it).

## Ghidra

Read from a sparse clone at `c91ec18e`. Pointers are
`ghidra@c91ec18e:PATH:LINE`. Paths are long, so they are abbreviated as
follows (expand them before following a pointer). A bare line number in
backticks, such as `605-669`, is in the same file as the pointer before it.

- `ARM/` = `Ghidra/Processors/ARM/`
- `Dis/` = `Ghidra/Framework/SoftwareModeling/src/main/java/ghidra/program/disassemble/`
- `Pseudo` = `Ghidra/Framework/SoftwareModeling/src/main/java/ghidra/app/util/PseudoDisassembler.java`
- `Ana/` = `Ghidra/Features/Base/src/main/java/ghidra/app/plugin/core/analysis/`
- `Util/` = `Ghidra/Features/Base/src/main/java/ghidra/program/util/`
- `FSA` = `Ghidra/Features/BytePatterns/src/main/java/ghidra/app/analyzers/FunctionStartAnalyzer.java`
- `AIF` = `Ghidra/Features/Base/src/main/java/ghidra/app/plugin/prototype/analysis/ArmAggressiveInstructionFinderAnalyzer.java`
- `ArmA` = `Ghidra/Processors/ARM/src/main/java/ghidra/app/plugin/core/analysis/ArmAnalyzer.java`

### How it works

- **Mode is a context bit.** `TMode` is bit 0 of the ARM context register
  (`ghidra@c91ec18e:ARM/data/languages/ARM.sinc:90`). Context that isn't
  `noflow` is copied to every static flow target
  (`ghidra@c91ec18e:Dis/Disassembler.java:1301`), so a Thumb `b`/`bl` target
  is Thumb with no extra work. ARMv4T defaults to `TMode=0`
  (`ghidra@c91ec18e:ARM/data/languages/ARMt_v45.pspec:16`).
- **`bx` is resolved by constant propagation, not by the disassembler.** To
  the disassembler `bx Rm` is a computed jump with no flow
  (`ghidra@c91ec18e:ARM/data/languages/ARMinstructions.sinc:2480`), except
  `bx lr`, which is a return (`ghidra@c91ec18e:ARM/data/languages/ARMinstructions.sinc:2472`). `ArmAnalyzer` extends
  `ConstantPropagationAnalyzer`
  (`ghidra@c91ec18e:ArmA:43`);
  when a computed jump or call resolves to a constant (`ghidra@c91ec18e:ArmA:182-188`,
  `210-213`), `flowArmThumb` (`ghidra@c91ec18e:ArmA:760`) clears bit 0, sets `TMode` at the
  target from the low bit (`ghidra@c91ec18e:ArmA:805-825`) and disassembles there (`ghidra@c91ec18e:ArmA:847-866`).
  An unknown value creates nothing (`ghidra@c91ec18e:ArmA:220-233`). This is decision 5b exactly.
- **`mov lr, pc; bx rX` is a call.** A context bit (`LRset`) set by `mov lr,
  pc` turns the next `bx` into a call
  (`ghidra@c91ec18e:ARM/data/languages/ARMTHUMBinstructions.sinc:2675`, then
  `1601`; ARM `ghidra@c91ec18e:ARM/data/languages/ARMinstructions.sinc:3614`, `2489`). `ArmAnalyzer` also
  rewrites a computed jump to CALL when `lr` holds the next address
  (`ghidra@c91ec18e:ArmA:106-122`), a jump through `lr` to RETURN (`95-104`),
  and a "return" to a constant to BRANCH (`237-251`). This is the case that
  stops gbadisasm at `Init`.
- **Recursive descent.** `Disassembler.disassemble`
  (`ghidra@c91ec18e:Dis/Disassembler.java:559`) runs a queue (`605-669`);
  `disassembleInstructionBlock` (`907`) decodes straight-line code until no
  fallthrough or an overlap (`968-1066`). A flow into defined data or the
  middle of an instruction is a conflict and is skipped (`614-627`).
  Undecodable bytes (`1102-1106`) or more than 16 identical repeated
  instructions (`83`, `1068-1071`) end the block with a "Bad Instruction"
  bookmark (`1447-1495`); call targets are queued only after their block is
  committed, and flows from past a conflict are dropped
  (`ghidra@c91ec18e:Dis/DisassemblerQueue.java:209-247`).
- **Validating a candidate function: `checkValidSubroutine`**
  (`ghidra@c91ec18e:Pseudo:609-842`, overlap checks `948-1007`). It pseudo-
  disassembles from the candidate without committing: rejects a zero first
  word, follows fallthrough and branches, fails on a bad instruction or a
  jump into the middle of an instruction, requires reaching a return or a
  call to a known function, and rejects bodies that overlap data or existing
  code, or are a single instruction.
- **Function-start byte patterns**
  (`ghidra@c91ec18e:ARM/data/patterns/ARM_LE_patterns.xml`). Thumb: a
  function end (`pop {…, pc}`, `bx lr`, `mov r8, r8` filler) followed by
  `push {…, lr}` and a `sub sp` / `mov` / `ldr` / `add r7, sp`
  (`2-43`); a standalone `push` + `sub sp` with `validcode="4"` and something
  defined right before (`165-219`); looser `push` patterns only right after a
  function (`222-255`). ARM: `stmdb sp!, {…, lr}` / `str lr, [sp, #-x]`
  (`45-163`). `FunctionStartAnalyzer` applies the `after=` condition
  (`ghidra@c91ec18e:FSA:379-445`) and `validcode=N` (at least N clean
  instructions; `257-283`), then sets `TMode` and queues the match
  (`300-308`).
- **Pointers.** A `ldr rX, [pc, #imm]` value is read when the pool is in
  read-only memory or within 4 KB ahead
  (`ghidra@c91ec18e:Util/VarnodeContext.java:546-560`, `815-835`). It
  becomes a reference only if it isn't below 1024, near the top of memory or
  -1 (`ghidra@c91ec18e:Ana/ConstantPropagationContextEvaluator.java:158-178`),
  and isn't the instruction's own fallthrough or inside an instruction or
  function (`ghidra@c91ec18e:Util/SymbolicPropogator.java:2826-2870`). Code is
  made there only if the value is used as a flow target into executable
  memory (`ghidra@c91ec18e:Ana/ConstantPropagationContextEvaluator.java:228-234`).
  `OperandReferenceAnalyzer` turns an address into code only if the value
  isn't used arithmetically (`ghidra@c91ec18e:Ana/OperandReferenceAnalyzer.java:645-678`)
  and `isValidSubroutine` passes (`412-445`); its pointer check wants ≥ 4096,
  aligned, mapped, not offcut, not mid-function (`952-1035`).
- **Thumb `bl` is always a call** (`ghidra@c91ec18e:ARM/data/languages/ARMTHUMBinstructions.sinc:1484`),
  unless a user sets the `CALLoverride` context bit
  (`ghidra@c91ec18e:ARM/data/languages/ARM.sinc:97`). No far-jump heuristic.
- **No-return.** The disassembler drops fallthrough after calls to known
  no-return functions (`ghidra@c91ec18e:Dis/Disassembler.java:1289-1292`,
  `1420-1444`). `FindNoReturnFunctionsAnalyzer` collects evidence at each call
  site (fallthrough runs into data, a function starts right after, conflicting
  references; `ghidra@c91ec18e:Ana/FindNoReturnFunctionsAnalyzer.java:545-622`)
  and marks the callee no-return at 3 such sites (`56`, `383-411`), then
  repairs the damaged fallthrough code.
- **Gap filling.** The ARM Aggressive Instruction Finder (a prototype, off by
  default) walks undefined gaps, tries the neighbour's mode and then the
  other (`ghidra@c91ec18e:AIF:228-271`), skips `nop`/`mov rX, rX` filler
  (`542-561`), wants more than 2 instructions that reach known code and a
  body not scattered over 4 KB (`424-479`), and undoes its work if the
  error-bookmark count rose (`496-508`).
- **Switch tables** (for issue #5): `ArmAnalyzer.recoverSwitches`
  (`ghidra@c91ec18e:ArmA:272-607`, off by default per `44-48`) and the
  `__gnu_thumb1_case_*` helper patterns
  (`ghidra@c91ec18e:ARM/data/patterns/ARM_switch_patterns.xml:21-134`),
  modelled as call fixups
  (`ghidra@c91ec18e:ARM/data/languages/ARM_v45.cspec:93-194`).

### What we borrow from Ghidra

- **`checkValidSubroutine` as the validator for pointer-scan and gap
  candidates** (decision 5d), in place of the `push {lr}` test: clean decode,
  reaches a return or a known call, no overlap with code or pools, more than
  one instruction. This is the fix for the 414 leaf functions the `push`
  scan misses.
- **Prologue patterns as a confidence score**, not a gate: the Thumb table
  above, especially "function end, then `push {…, lr}`", feeds the 0–100
  score borrowed from IDA.
- **Pointer filters from `OperandReferenceAnalyzer`/`SymbolicPropogator`**:
  aligned, in ROM, not offcut, not inside an existing function, and, for
  values found by const-prop, used as an address rather than in arithmetic.
- **Conflicts as first-class results.** A decode failure or a flow into the
  middle of an instruction marks the path that led there; a speculative seed
  whose closure hits one is rolled back (Aggressive Instruction Finder's
  "undo if errors rose"). Confirmed seeds (entry, `bl` from confirmed code)
  never are.
- **The `lr` rules from `ArmAnalyzer`**: computed jump with `lr` = next
  address is a call; jump through `lr` is a return; "return" to a constant is
  a branch. Our const-prop over the `Sem` IR gets all three.
- **Evidence-threshold no-return** (3 sites), together with IDA's lazy
  re-check of callers.
- **Not borrowed:** Ghidra's lack of a far-jump `bl` heuristic. On agbcc
  output it would produce the same 14 false starts gbadisasm does.

## IDA Pro

IDA is closed source. Hex-Rays now publishes the SDK on GitHub, including
the ARM module's private header, so names and flags below are from
`HexRaysSA/ida-sdk@d51b0e7c920cf67e7724b56a3dd9a74f6dd84606`
(`src/module/arm/arm.hpp`, `src/include/auto.hpp`, `funcs.hpp`, `idp.hpp`).
Behaviour comes from the docs
(<https://docs.hex-rays.com/ida-actions/options>) and Igor's tips on the
Hex-Rays blog (#134 "ARM BL jumps", #137 "processor modes and segment
registers"). The ARM module's algorithms themselves aren't public; where only
a function name is known, that's said.

- **Mode is a virtual segment register `T`** (0 ARM, nonzero Thumb, ARM by
  default), stored as ranges with changepoints, set by following `bx`/`blx`
  mode switches (tip 137). The module has `guess_arm_or_thumb(ea,
  can_be_thumb, can_be_arm)`, `propagate_t_bit_to(…, use_low_bit)` and
  `try_code_start(…, respect_low_bit)`; only the names are public. ARM
  options: "No automatic ARM-Thumb switch", "Disable BL jumps detection",
  "Disable pointer dereferencing" (display only), "Simplify instructions".
- **Queues.** Auto-analysis keeps one address-sorted queue per kind of work
  and drains each before the next: unexplored, code, *weak code* (IDA's own
  guesses), procedures, tails, …, FLIRT passes, final (`auto.hpp`). Kernel
  options include "Trace execution flow", "Create function if data xref
  data->code32 exists", "Create functions if call is present", "Perform
  'no-return' analysis", "Make final analysis pass".
- **Pointers in data.** "Automatically convert data to offsets" only fires
  when the target is a valid address above 0x20, not in the middle of an
  item, not reached by fallthrough from the previous instruction, and the
  item is a dword (docs). Processors can score a candidate function start
  through `ev_may_be_func` (0–50 no, 51–99 needs more evidence, 100 certain;
  `idp.hpp`).
- **`bl` as a far jump.** The docs say IDA decides per `bl` whether it is a
  call or a jump, with per-site overrides "Force BL call"/"Force BL jump"
  (tip 134). The header's default flags include `IDP_NO_BL_JUMPS` ("disable
  detection"), and the shipped default comes from `ARM_DISABLE_BL_JUMPS` in
  `ida.cfg`, value unconfirmed. The module has `is_bl_call` and
  `range_uses_lr`, which suggests the test is whether the code after the `bl`
  uses `lr` (unconfirmed).
- **No-return** is a function flag with a "pending" state; marking a callee
  noreturn re-analyses its callers (`FUNC_NORET`, `FUNC_NORET_PENDING`,
  `set_noret_insn` in `funcs.hpp`).
- **GBA loaders.** `ldebug/ida-gba-ldr@a1078a17` (`gba.cpp`) recognises the
  header (the `b` at offset 0, logo, checksum), maps ROM/EWRAM/IWRAM, makes
  code at `0x08000000` and leaves `T` at its ARM default.
  `laqieer/ida_gba_stuff@17cc5267` ships FLIRT signatures for libagb, libgcc,
  libc and the m4a sound driver, and `agb_main_finder.idc`, which reads
  `AgbMain` and the IRQ handler out of crt0's literal pool.

### What we borrow from IDA

- **Strong before weak.** Drain recursive descent from confirmed seeds
  completely, then try pointer-scan candidates in order of score, dropping any
  that land inside code already claimed. gbadisasm has no such order: every
  seed is trusted equally, which is how two rodata bytes that look like
  `push` became false starts in the 93.5% run.
- **Score candidates 0–100** instead of a yes/no prologue test: a `push {…,
  lr}` start, a clean decode to a return, no overlap, and being referenced
  from a word that is itself in a pool or a known pointer table all add
  points.
- **IDA's offset rules** for what counts as a pointer during the scan.
- **Lazy no-return**, re-checking callers when a callee becomes no-return, so
  descent doesn't decode the padding after a call to a function that never
  returns.
- **Per-site overrides** (force call / force jump, and mode) in a per-game
  hints file, for the cases heuristics get wrong. Same role as gbadisasm's
  config, but as exceptions rather than the main seed source.
- **The crt0 trick** as an extra seed source: `AgbMain` and the IRQ handler
  come straight out of crt0's pool. Our const-prop gets this for free on
  `Init`, but it is a cheap check.
- **Later, maybe:** signatures for the AGB SDK libraries and m4a, to name
  and pin library functions; mostly useful for hand-written asm (m4a) that
  breaks heuristics.

## Emerald ROM hash

Command, run on quack:

```
$ sha1sum ~/ROMs/gba/"Pokemon - Emerald Version.gba"
f3ae088181bf583e55daf962a92bb46f4f1d07b7  ~/ROMs/gba/Pokemon - Emerald Version.gba
```

pret's expected hash, from
<https://github.com/pret/pokeemerald/blob/master/rom.sha1> (at
`731ad5bf`):

```
f3ae088181bf583e55daf962a92bb46f4f1d07b7  pokeemerald.gba
```

**They match.** The local build below is also byte-identical to the ROM
(`cmp` reports no difference), so the build's ELF describes exactly the bytes
our recompiler will read.

## Getting the symbol map without committing it

Everything lives under `$GBAA_DATA` (default `~/.local/share/gbaa/`), never in
the repo.

**The published `.sym` is not enough.** pret's `symbols` branch
(`dba968c6`) has `pokeemerald.sym`, 73,204 lines of `ADDR SCOPE SIZE NAME`.
It comes from `objdump -t` with the flags column cut out (the `$(SYM)` rule
at `pokeemerald@731ad5bf:Makefile:397`), so it has no function/object type and no
Thumb bit: function starts are indistinguishable from data and local branch
labels in it. It is fine for naming addresses, not for scoring discovery.
Fetch with `git -C pokeemerald fetch origin symbols && git show
origin/symbols:pokeemerald.sym`.

**Build it instead.** On quack, 2026-10-19, no sudo needed:

1. Toolchain: ARM's prebuilt GNU toolchain 15.2.rel1
   (`https://developer.arm.com/-/media/Files/downloads/gnu/15.2.rel1/binrel/arm-gnu-toolchain-15.2.rel1-x86_64-arm-none-eabi.tar.xz`,
   155 MB) unpacked to `$GBAA_DATA/toolchains/`. Only binutils are used; agbcc
   is the compiler.
2. `git clone https://github.com/pret/agbcc` and
   `https://github.com/pret/pokeemerald` into `$GBAA_DATA/src/`.
3. In `agbcc/`: `./build.sh && ./install.sh ../pokeemerald` with the
   toolchain's `bin/` on `PATH`. **30 s.**
4. In `pokeemerald/`: `make -j16` with the toolchain on `PATH` and libpng
   from Homebrew (`PKG_CONFIG_PATH=/home/linuxbrew/.linuxbrew/lib/pkgconfig
   CPATH=/home/linuxbrew/.linuxbrew/include
   LIBRARY_PATH=/home/linuxbrew/.linuxbrew/lib`; quack has no
   `libpng-devel`). **23 s** (memory not measured; 14 GB was available).

Outputs, all in `$GBAA_DATA/src/pokeemerald/`: `pokeemerald.gba` (SHA1
`f3ae0881…`, identical to the ROM), `pokeemerald.elf` (56 MB, full symbol
table), `pokeemerald.map` (2.9 MB linker map). `just fetch-pret`
(`scripts/fetch-pret.sh`) is steps 1–4, pinned to these commits.

**What the ELF says about Emerald's code** (`arm-none-eabi-readelf -sW`):

- 15,858 `FUNC` symbols at 15,856 distinct addresses, all in ROM: `.text`
  (15,471 Thumb) and `lib_text` (375 Thumb, 12 ARM). Thumb symbols carry
  bit 0 set, as the ARM ELF ABI requires.
- crt0's ARM entry points `Start`, `Init` and `IntrMain` are `NOTYPE`
  (`src/crt0.s` doesn't use the `arm_func_start` macro), but each sits on a
  `$a` mapping symbol, so they can be recovered.
- 29,319 `$t`, 16 `$a` and 26,666 `$d` mapping symbols in the code
  sections `.text` and `lib_text`: a byte-level code/data map, usable later for a "bytes classified
  correctly" metric.
- **No function is linked at a RAM address.** Code that runs from IWRAM
  (`SoundMainRAM`, `IntrMain`, the `*_in_RAM` link routines) is linked at its
  ROM address and copied at run time (`CpuCopy32(SoundMainRAM & ~1,
  SoundMainRAM_Buffer, …)` at `pokeemerald@731ad5bf:src/m4a.c:74`). Relevant
  to decision 7 (IWRAM overlays): the copy source is a ROM function, so the
  overlay can be found from the copy call.
- `SoundMainRAM` is a `thumb_func` whose fifth instruction, after a `beq`,
  switches to ARM with `adr r1, …; bx r1`
  (`pokeemerald@731ad5bf:src/m4a_1.s:88-96`), and later switches back to
  Thumb the same way (`117-118`): one symbol, both modes.

## Proposed discovery-accuracy check

A later issue implements this as a binary (or `just` recipe) in the repo; the
truth file stays in `$GBAA_DATA`.

**Truth file** (`$GBAA_DATA/truth/pokeemerald.funcs.tsv`), generated from
the ELF: tab-separated, one line per function start, sorted numerically by
the first column, for example `0x080003a5<TAB>288<TAB>AgbMain`.

Columns: interworking address as `0x` + 8 lowercase hex digits (bit 0 set
means Thumb, clear means ARM), size in bytes from the symbol (0 for the
untyped crt0 labels and a few asm symbols), name. Rows:

1. every `STT_FUNC` symbol whose address, with bit 0 cleared, is in ROM
   (`0x08000000 ≤ a < 0x0A000000`);
2. every `GLOBAL NOTYPE` symbol in an executable section that sits on a `$a`
   or `$t` mapping symbol, with the mode taken from the mapping symbol (picks
   up `Start`, `Init`, `IntrMain`);
3. aliases (two names, one address) collapse to one row, keeping the first
   name in `readelf -sW` order.

Emerald gives 15,859 rows (15,844 Thumb, 15 ARM). Rule 2 adds exactly three
rows (`Start`, `Init`, `IntrMain`); the generator should print the names it
adds under rule 2, since on another decomp it could pick up a global asm
label that isn't a function.

**Discovered file**: our discovery's output, one start per line, same
interworking convention, hex with or without `0x`, case-insensitive; `#`
starts a comment; blank lines ignored; duplicates collapse. Starts outside ROM (IWRAM overlay
entries) are counted and printed but not scored, since the ELF has no truth
for them.

**Matching rule.** A discovered start matches a truth row iff the 32-bit
values are equal: same address **and** same mode. Before matching, check
alignment: an ARM start (bit 0 clear) must be 4-aligned; a Thumb start is
2-aligned once bit 0 is cleared (always true). A misaligned ARM start means
the discovery is broken, so the check exits non-zero and scores nothing. A start at
the right address in the wrong mode is a false start and a miss, not a match.
Nothing fuzzy: a start a few bytes off is wrong.

**The two numbers:**

- **precision** = matched / discovered starts in ROM
- **recall** = matched / truth rows

printed as `precision 0.9987 (14827/14846)` and `recall 0.9349
(14827/15859)`. Under them, as diagnostics only: false starts split into
wrong mode / inside a true function (`start < a < start + max(size, 1)`,
so size-0 rows never contain anything) / elsewhere,
the count ignored as outside ROM, and the first N misses and false starts
with names.

**Baselines to beat** (from the runs above): precision 99.87%, recall 93.49%
(gbadisasm descent seeded by a `push`-validated pointer scan). Recall can
never quite reach 100% from static analysis plus pointer scan: ~600
functions are dead (no pointer, no call from live code). Missing those costs
nothing at run time; a gap-sweep for prologues in undiscovered regions is
what would find them, if we ever want to.

The throwaway prototypes that produced the numbers above (`truth.py`,
`score.py`, `scan.py`, ~100 lines of Python) were in `/tmp/gbaa-disc/` and
are not committed; the spec above is the part to keep.

## Corrections and effects on decisions

Not edited in `decisions.md`; for whoever owns it:

- **gbadisasm is not pret's repo.** `decisions.md` ("Prior art we will use")
  and `references.md` say "pret's gbadisasm" with URL
  `github.com/pret/gbadisasm`, which 404s. It is
  `github.com/camthesaxman/gbadisasm`, used by pret.
- **Decision 5d's validation ("prologue-like") needs to be looser than
  `push {lr}`.** 414 address-taken Emerald functions (2.6%) have no
  `push {…, lr}` prologue: leaf functions that start with `bx lr`, a load, or
  a branch. Validate a scanned candidate by decoding it cleanly to a return
  without hitting a known pool or overlapping confirmed code, and use the
  prologue only to rank candidates.
- **Decision 5a needs a far-jump `bl` rule.** Some rule is not optional for
  codegen; the one proposed above is tested only with the ELF's extents.
- **Open decision 7 (IWRAM overlays, in `open-decisions.md`) gets a lead:**
  in pret builds, RAM-run code is a ROM function
  plus a copy call, so overlays can be found statically from `CpuCopy`/DMA
  sources rather than only by hashing at run time.
- **This contradicts the text of 5d**, which lists "prologue-like" as a
  validation criterion; the proposal above demotes it to a ranking signal.
  Otherwise the measurements support decisions 5 and 6 (recursive descent
  plus a validated pointer scan reaches 93.5% recall before const-prop or
  runtime seeds).
