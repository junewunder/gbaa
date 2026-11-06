# gbaa design decisions

Settled one at a time with june. Each entry: the decision, the alternatives,
why, the cost, and what would change our minds.

## 1. Static recompilation

**Decided:** translate the ROM's ARM/Thumb code ahead of time into Rust, with
an interpreter fallback for code discovery misses. Not an interpreter or JIT
emulator alone, not a decomp port.

**Why:** the project exists because it's fun, and the fun part is the
recompiler. It's also practice for a later Wii recompiler (where interpreting
is too slow). Anything less than static recompilation and june would sooner drop
the project. For GBA on a desktop alone an interpreter would be fast enough.

**Alternatives:** interpreter (mGBA); JIT (specialises at run time and never
faces the code/data separation problem); decomp port (per-game manual work;
decomps are ground truth for us, not a method).

**Costs:** code discovery is undecidable in general, so the interpreter
fallback is required; code copied into RAM breaks the "fixed program"
assumption; timing gets coarser.

**Would change our minds:** a real game spending most of its cycles in the
fallback interpreter.

**ESP32 is a stretch goal, not an initial goal.** The XIAO ESP32-C6 was never
june's pick (4 MB flash can't hold a 32 MB ROM). If it happens, likely an
ESP32-P4 or S3 with external storage.

## 2. Emit Rust source

**Decided:** emit Rust source (`quote!` + `prettyplease`), compiled with the
runtime by cargo. If Rust hits a wall, the next backend to try is LLVM IR
(`inkwell`). C is out: june wants the portability rustc/LLVM gives (many
hosts and targets from one toolchain), and wasn't sure how far C would go there.
Cranelift is out: no 32-bit RISC-V or Xtensa backend, weaker optimiser.

**Why:** the runtime is Rust, so the memory accessors inline into generated
code with no FFI or cross-language LTO; one toolchain for desktop and no_std;
rustc type-checks the output.

**Costs:** no `goto` and no guaranteed tail calls; compile time on huge
generated functions. Tracked in [risks.md](risks.md). june's view: these are
problems to solve, not reasons to switch.

**Goto elimination has prior art** (relooper, stackifier, Ramsey's "Beyond
Relooper", decompiler structuring); see decision 8 and risks.md.

**Readability of the output is not a goal now.** The baseline goal is games
running exactly, with no requirement that the code be readable. Correct but
mechanical (`cpu.r[3] = …`) is fine; june will read it only when debugging.
Don't trade exactness, compile cost or speed for readability. Long-term
framing: output close to a decomp's (C-like, named functions and variables,
`if (gMain.intrCheck & 1)`) is what would make games easy to mod. The output
is a starting point that people or LLMs rewrite into readable code, maybe with
recovered abstractions or a small game-engine layer to build on. Not in scope
for the early milestones.

**Idea, not designed (june, 2026-10-24): a source map.** At some point,
generated code carries a map from guest addresses to names for functions and
variables. The decomps' symbols fill it where they exist, and LLMs gradually
propose proper names for the rest. Naming changes no behaviour, so it never
costs exactness, which makes it the cheapest route to readable output.

**Two views of a ROM (june, 2026-10-26; groups `mod` and `assets`):**
- **The play view** is what the factory builds: printed only for exactness,
  build cost and speed, and never read. Everything above holds for it.
  Registers as Rust locals stay off by default here, behind `--locals`
  (d/registers-rust-locals-cost-58).
- **The mod view** is for modding, read mostly by agents and sometimes by
  people. It shares the analysis, structuring and runtime, and differs only
  in printing: names from the source map, and a readable form (locals on,
  short macros for ticks, calls and returns) for the functions a mod
  touches, not the whole game.
- **Names live outside the code** until a mod starts: the source map is a
  sidecar file the printer applies, so codegen can keep improving and every
  regeneration keeps every name. Only when june starts a mod is the named
  code copied and edited by hand; from then on it's ordinary Rust in git.
- **The decomps are the oracle.** pret's pokeemerald (and pokeruby,
  pokefirered) give exact names for functions and variables, and their
  graphics and music as files, to score naming and asset extraction against.
- **No way back to the GBA.** Generated Rust runs on the emulated machine,
  so it can't become a GBA ROM; mods run in gbaa only. Patching the ROM
  itself (the ROM-hack way) would be a separate tool, and isn't planned.
- **Legality:** generated code is the game's code translated, so it stays
  private, like the ROM. A source map holds only addresses and names.
- **Design:** [mod-view-design.md](mod-view-design.md) (the map's format,
  the printer's check, the readable form, mod copies, the naming loop).
- **Asset design:** [asset-extraction-design.md](asset-extraction-design.md)
  (load and use logs, the export formats, the manifest, the checks against
  pret, overrides served at the load).

**Modded builds are plain Rust programs (june, 2026-10-31).** Save states,
rewind and readable mods can't all hold: a mod is ordinary Rust, ordinary
Rust can reach the world (a network request, a file, the clock), and
nothing that touches the world rewinds for free. No middle ground for now:
- **Unmodded builds keep everything:** save states, rewind, netplay
  rollback, mGBA states and the byte-for-byte checks, mostly native code
  and the nice UI. Real ROM addresses stay in their state (return
  addresses and function pointers both;
  docs/research/code-without-rom-addresses.md).
- **Modded builds get none of that.** No save states, no rewind, no
  rollback. They are fully native code using ordinary memory, not a
  GBA-sized memory block a developer has to work around: literally a Rust
  program. Anything they need of that kind they work out themselves.
- **How a game gets there** is designed in
  [native-port-design.md](native-port-design.md) (unit `native/design`,
  2026-10-31; nothing built yet): state in one typed `Game` struct, the
  hardware as a library, time only at waits, function pointers as IDs,
  holes as build errors, the play build as the oracle. Today's mod copies
  (cut functions running on `Machine`) are the older form.
- **Settles the stack questions for mods:** no resumable mod functions, no
  replay-from-checkpoint, no held frame hand-off. The guest-RAM record that
  re-runs a lost mod call's block (`rt::modcall::resume`) is unsound under
  rollback (a restored run calls the callee one and a half times, a
  straight run once) and is no longer needed.

## 3. Semantics written once, over a `Sem` trait, plus a small IR

**Decided:** each instruction's meaning is one function generic over a
tagless-final `Sem` trait. Decoded fields (registers, immediates, S bit) are
plain Rust values, so they are static by construction; only `S::V` is
dynamic, and no binding-time analysis is needed. Interpretations:

- `Sem<V = u32>`: the interpreter (monomorphised, zero-cost).
- `Sem<V = ValueId>`: builds a small per-block SSA IR (explicit flag values,
  loads/stores, control exits). Flag liveness (backward) and const-prop run
  on it; the IR is then printed to Rust. This replaces the original plan of
  `V = TokenStream`, which can't host backward analyses.
- `Sem<V = AbsVal>`: an abstract interpreter (value-set analysis) for
  discovery. **Required, not optional** (2026-10-19, from the jump-table
  research): jump-table recovery (5c) and the far-jump `bl` rule (5a) both
  run on it.

**Alternatives:** two hand-written copies (drift); a spec DSL generating both
(no ARMv4T Sail model, a language to maintain); IR-only (P-code/VEX style).

**Costs:** designing the IR; the trait needs effect operations (branches,
CPSR writes, `swi`, loads into `pc`), which is most of the design work.

**Would change our minds:** the IR becoming the real semantics with the trait
a mere builder (drop the trait, interpret the IR); the effect interface
tangling around `ldm`/`stm` and mode switches.

## 4. Decoder: `unarm` behind our own enum

**Decided:** use `unarm` (MIT, no_std + alloc, used by objdiff and the DS
decomp tooling) with `Options.version = V4T`, wrapped by an adapter (~300–450
lines) that maps its `Ins` into our own instruction enum, which `Sem`
consumes. Pin the version. june preferred off-the-shelf over hand-written.

**Evidence (research agent, 2026-10-18, probe in /tmp/gbaa-decoder-probe):**
- Sweeping all 2^28 ARM words (cond = AL) and all 65,536 Thumb halfwords with
  V4T: no v5+ instruction decodes; one v6K/v8 hint (`csdb`, E320F014) slips
  through and must be filtered.
- Quirks correct: ldm/stm base in list, mul rd==rm, ldm user-bank bit, msr
  field masks, shift-by-register, `ldr pc`, `movs pc, lr`.
- `yaxpeax-arm` ignores its ARMv4 setting (decodes clz, blx, ldrd, qadd, bkpt
  as valid) and has an untyped operand list: ruled out.

**The adapter must handle:**
- ~60 of unarm's 233 variants are reachable on v4T; fold aliases back
  (shifted `mov` → `Lsl`/`Lsr`/…, sp `ldr`/`str` → `Pop`/`Push`, Thumb
  variants reuse ARM ones with `thumb: true`); catch-all arm for v5/v6.
- A small raw-bits fallback for encodings unarm calls `Illegal` but the
  ARM7TDMI executes: nonzero "should be zero" bits (mul [15:12], bx [19:8],
  tst/teq/cmp/cmn with rd ≠ 0, 112 Thumb `bx` encodings) and empty register
  lists.
- unarm bug: msr immediate rotates by `rot` instead of `rot*2` (E328F20F gives
  0xC0000003, should be 0xF0000000) in `MsrOp2::parse`. Work around it and
  send the fix upstream.
- Thumb `bl` halves are decoded separately (a lone half is `Illegal` in unarm).

**Correction:** real typed ARM+Thumb decoders are 1,000–1,500 lines (mGBA
~1,490, rustboyadvance-ng ~1,089), not the 600–1,000 first claimed.

**Fallback:** hand-write ~1,100 lines, keeping unarm as a dev-dependency to
sweep-compare against. Choose it if unarm's 23k-line generated parser hurts
ESP32 build size or compile time.

**Would change our minds:** a steady trickle of unarm bugs in rare corners
(its README makes no correctness promises).

**Watch (2026-10-19, from building the adapter, #2):** two unarm bugs so far
(the msr rotate above, and `ldrh`/`strh`/`ldrsb`/`ldrsh [r0, rm]!` decoding as
`Illegal`), and unarm is stricter than the research found: it rejects many
encodings the ARM7TDMI executes (`mov` with rn ≠ 0, junk in `swp`/`mrs`/`msr`
should-be-zero bits, empty `msr` masks). The adapter rewrites each word into a
form unarm accepts before parsing (following mGBA) and checks against an
independent reference decoder over all 2^32 ARM words. Not enough yet to
switch, but it counts toward the trickle.

## 5. Code discovery

**Decided:**
- **5a.** Recursive descent from the ROM entry and branch/`bl` targets, not
  a linear sweep (literal pools sit between functions). Take inspiration from
  Ghidra, IDA and existing GBA disassemblers/decompilers (prior art exists;
  borrow heavily; see `research/discovery-prior-art.md`).
  **Far-jump `bl`:** agbcc and GCC use `bl` as a long jump (usually a switch's
  default case) inside big functions; treating it as a call gives wrong code.
  Rule: a `bl` is a jump if every return its target reaches leaves sp above
  its entry value (it pops the caller's frame). Computed as a fixpoint on
  `Sem<V = AbsVal>`. Measured 547/548 on agbcc Emerald and 721/732 on modern
  GCC, no false ones, and it needs no function extents. Also: GCC's `bl` to
  a local `bx rN` stub is an indirect call to rN, not a call to the stub.
  **LLVM (agb) additions** (2026-10-20, #25 and #30): rustc/LLVM uses `bl`
  as a long branch to cold panic blocks and inside a `main` that never
  returns, where no return exists for the rule to see. So a `bl` from a
  function with a `push {…, lr}` prologue into the span of its own reached
  code is a jump too, unless the target is itself a prologue or a `bx rN`
  stub (functions don't nest). The probe also answers "never returns"
  (no return at all, every jump resolved, none leaving), and nothing
  follows such a call. And a probe that reaches no return keeps an earlier
  "jump" unless what stopped it is such a call or data, which ends a
  flip-flop between cases that far-jump to each other. Measured: 548/548 agbcc, 726/732 modern GCC, no
  false ones; 138/138 on june's first-frame ROM (`just disc-agb`).
- **5b.** ARM/Thumb mode tracked through `bx` targets resolved by const-prop;
  unresolved targets become dynamic exits through the dispatcher.
- **5c. Jump tables: value-set analysis** (`Sem<V = AbsVal>`) over the
  jump's backward slice, **no per-compiler matchers** (2026-10-19, june
  adopted `research/jump-tables.md`). Both approaches resolve 675/676 tables
  on agbcc Emerald; on a modern-GCC Emerald build, V gets 583/617, matchers
  580, the union 600; neither gave a wrong target. Matchers cost ~280 lines
  per compiler family and a second way to be wrong, and can be added behind V
  later without changing the design. Discovery rejects tables whose targets
  don't decode or leave the function. If modern-GCC games later spend time in
  unresolved switches, add backward refinement through the IR (Ghidra-style),
  which recovers most of V's misses.
- **5d.** ROM pointer scan for candidate entries, kept apart from code
  recursive descent confirmed. **Validation** (2026-10-19, from the discovery
  research): a candidate decodes cleanly to a return without running into a
  known literal pool or overlapping confirmed code. A `push {…, lr}` prologue
  only **ranks** candidates; requiring it would drop 414 address-taken
  Emerald functions, and only 61% of modern-GCC Thumb functions start with
  one (agbcc: 93%).
  **Plus one piece of evidence** (2026-10-20, from building discovery, #18):
  a validated candidate also needs a `push {…, lr}` prologue, code loading
  its address, or a known function pointer beside it in a table. Without
  that, text and battle scripts that decode as plausible code drop Emerald
  precision to ~0.89. Cost: a leaf function with none of the three is missed
  (the interpreter fallback and seed logging, decision 6, catch it).
  Measured: Emerald precision 0.9992, recall 0.9555; modern GCC 0.9985 /
  0.7750, where most misses are dead functions.
- **Checking discovery:** precision and recall of function starts against
  the pokeemerald ELF, specified in `research/discovery-prior-art.md`
  ("Proposed discovery-accuracy check"). Baseline to beat: 99.87% precision,
  93.49% recall (gbadisasm seeded by a pointer scan).

**Would change our minds:** runtime seed logging (6) finding nearly every
missing entry quickly makes the scan optional.

## 6. Profile-guided discovery

**Decided (june trusts Claude's call here):** when execution reaches an
address with no compiled function, the runtime runs the interpreter and logs
the address as a seed for the next recompile. The interpreter hands control
back to compiled code as soon as it reaches an address that has some. The
seed file comes from play, can't be regenerated from the repo, and so is
committed per game.

**Alternatives:** fail hard on unknown addresses (N64Recomp-style; needs
perfect discovery, usually from symbols); a JIT fallback (a second backend).

**Why:** discovery misses cost speed, not correctness, and shrink with each
play-and-recompile round.

**Costs:** the interpreter/compiled handoff must be exact (registers held in
Rust locals under decision 10's ABI-trust mode must be written back at every
handoff); coverage depends on how far someone plays.

**Would change our minds:** a game stuck in the interpreter in a hot loop it
never leaves; then add a bounded recompile-while-running mode.

## Prior art we will use

`gbadisasm` (github.com/camthesaxman/gbadisasm, used by pret; recursive descent, agbcc jump tables) is the first thing
the discovery research reads and borrows from, and the pret decomps' symbol
maps are ground truth for checking discovery (milestone 5).

## 7. Code copied into IWRAM

**Decided for agb ROMs (#54, sprint #56):** code a ROM
copies into IWRAM is recompiled where it runs, as overlays, and runs
compiled while IWRAM still holds the bytes it was compiled from.

- **What gets copied** comes from discovery's BIOS copies
  (`Discovery::copies`: a `CpuSet`/`CpuFastSet` out of ROM with constant
  arguments, which is how agb's crt0, 0.13 and 0.25 alike, copies
  `.iwram`; also through a `swi; bx lr` wrapper called with constant
  arguments, which is how pokeemerald-expansion's `m4aSoundInit` copies
  the sound mixer). Only copies into IWRAM; EWRAM would work the same way but no
  ROM we have runs code there, so it isn't done.
- **Where it's entered:** constant addresses inside a copy's destination
  that the ROM's code reaches (`Discovery::ram_entries`): call and jump
  targets, rN at a `bl` to a `bx rN` stub (how ROM code reaches agb's
  `memcpy` and the mixer), and the word stored to the IRQ vector
  0x03007FFC (`InterruptHandler`). From those, discovery runs again over
  the copy's bytes mapped at their IWRAM address (`discover_at`), with no
  pointer scan, so `bl` targets inside the copy are found too.
- **One overlay per block.** Codegen builds each of the copy's
  functions like a ROM one, at its IWRAM addresses, reading the ROM bytes
  it came from. Each block is guarded by its own instruction bytes (and
  any IWRAM `bx rN` stub its function calls through), not the whole copy
  or function: the copy also holds data written all the time
  (`agb_rs__program_counter` on every IRQ, `agb_rs__buffer_size`), and a
  function may patch one of its own loops (Emerald Rogue's mixer stamps
  one of three templates into its unrolled loop for each channel), which
  should send only that loop to the fallback. (Per function until
  `discovery/modern-iwram-code`.)
- **Patched versions** (`codegen/src/variants.rs`): `gbaa-recompile`
  runs the ROM in the interpreter for 1800 frames with no input and notes
  each overlay block's bytes whenever it starts running with them changed
  from the ROM's; each set seen is built as another version of the block,
  an overlay with its own bytes as its guard, kept only if its extent and
  exits are the block's. The block's match arm runs whichever version
  IWRAM matches. Emerald Rogue's mixer loop has three, all seen by frame
  460. Bytes never seen still go to the fallback.
- **Copies seen running** (`codegen/src/seen.rs`, `discovery/ram-code-misses`):
  in the same kind of run, the first time code runs at an IWRAM address
  outside every known copy, codegen looks for the bytes there in the ROM
  (the place whose first 8 bytes match and which agrees longest, at least
  12 bytes, and no tie), widens the match while IWRAM and the ROM agree,
  and treats it as a copy: discovered from the addresses seen entering
  it, compiled and guarded like a static one. This finds pret's
  `IntrMain`, which `DmaCopy32` copies (0x800 bytes to 0x03001930). Code
  run in the live stack (at or above `sp`) is left to the fallback:
  agb_flash's `ReadFlash_Core`, copied onto the stack by a CPU loop, lands
  wherever its caller's frame is (0x03007d28 and 0x03007d58 in the
  recompile run, 0x03007dd4 in a later one), so an overlay there would
  rarely match, and it only reads the save at boot.
- **The guard** (`runtime/src/overlay.rs`): every IWRAM store
  (`Bus::write8/16/32`, so CPU, DMA and the `CpuSet` HLE alike) tests one
  bit per IWRAM word, and a hit marks the overlays covering that word
  dirty; `RegisterRamReset` marks all of them. Entering an overlay (the
  dispatcher's lookup, `rt::call`, and the top of the overlay's own loop,
  since its own stores may rewrite it) tests a flag; only a dirty overlay
  compares its bytes with the ROM's, once. A mismatch sends the key to the
  interpreter fallback, which runs whatever IWRAM holds; writing the old
  bytes back makes it valid again.
- **A `bl` to a `bx rN` stub is an indirect call** (`rt::call`): the
  stub's `bx` through `apply_exit`, then the block rN names through the
  table, keeping the return site, instead of unwinding to the fallback to
  run the stub. Stubs stay out of the function starts (decision 5a).

**Why:** this is where the fallback's time went. Over 600 frames (#54):

| ROM | fallback before | after | compiled in RAM |
|---|---|---|---|
| agb ROM at 10512 Hz | 19.2% | 0.1% | 19.1% |
| agb ROM at 18157 Hz | 26.4% | 0.1% | 26.2% |
| june's first-frame ROM | 7.9% | 0.3% | 7.5% |

with no overlay invalidated in any of them, and the frame and registers
equal to the interpreter's (`just first-frame`). The lead from the
discovery research (copies are found statically from the copy call)
holds, so nothing hashes at run time.

**Alternatives:** keying overlays by a hash of the bytes, checked on every
entry (the proposal before #54; O(size) per entry); guarding the whole
copy (invalidated by every IRQ in agb); interpreting RAM code (decision 1
would lose a fifth of the agb sound ROM's time to it).

**Costs:** a bit test on every IWRAM store, stack pushes included; a flag
test before every overlay block. A store that rewrites a later instruction
of the block running is seen only at the next block. At most 4096
overlay blocks (a copy past that stays interpreted); a store to a watched
word scans the overlay table. The share compiled in RAM counts only the
cycles overlays tick, not a `swi` or DMA they start. Code that only
reaches RAM through a computed address (a function pointer loaded from
data) isn't entered, so it stays interpreted.

**Evidence:** the runtime's guard tests (`dispatch/tests.rs`: an overlay
runs; a store into its code sends it to the fallback; a store to a data
word between overlays doesn't; the old bytes make it valid again), and
`agb/iwram-rewrite`, an agb program that overwrites its own IWRAM function
at frame 20 and restores it at frame 40: interpreted and recompiled
frames and registers match, with one invalidation.

**Still open** (open-decisions.md, 7): copies the recompile run doesn't
reach, code on the stack, EWRAM code, code built at run time.

## 8. One Rust fn per guest function

**Decided (milestone 2, codegen #31, checked by first frame #32):** each
discovered function becomes one Rust fn, `fn(&mut Machine, key) -> Flow`,
whose body is `loop { match pc }` over its blocks; every block start of
every function goes into one sorted address → fn table, which the
dispatcher binary-searches.

- A call to a known function (`bl`, `mov lr, pc; bx rN`) is a Rust call.
  A `bl` to a `bx rN` stub (GCC's and LLVM's call-via-register) is an
  indirect call through the table, `rt::call` (decision 7). Its return is guarded by the `match`
  itself: if the callee comes back to the expected return site the caller
  continues there; any other address goes up through `_` to the
  dispatcher.
- Indirect calls and tail calls (`b other_fn`) unwind to the dispatcher,
  which bounds host stack growth from guest tail-call chains. Guest
  recursion is host recursion; the run thread gets a 256 MB stack.
- Every resolved jump table keeps a fallback arm: an entry that isn't a
  block of the function leaves through `_` to the dispatcher.
- Every block start is enterable (return sites, table targets, IRQ return
  points), which is why the fn takes the key to start at.

**Why:** the naive form is the baseline later milestones measure against
(sprint #36). `loop { match }` handles irreducible flow for free, and the
guard costs no comparison.

**Alternatives:** structured control flow (Ramsey) now; one fn per block
(every transfer through the dispatcher).

**Costs:** every transfer goes through the loop head, so rustc sees no
loops; generated code is large (june's ROM: 138 functions, 100 s and
1.6 GB to build).

**Evidence:** 22,500 SingleStepTests cases through generated Rust match
the interpreter (`just sst-gen`); jsmolka's `arm`, `thumb`, `memory`, `nes`
(which runs in RAM, so in the fallback) and `bios` match it at their idle
loops (`just recompile-check`, #31); and june's first-frame ROM, `arm`,
`thumb` and the agb sprite example draw the identical frame
(`just first-frame`, #32).

**Structuring (decided 2026-10-24, `structured/design`; design in
`docs/structured-codegen-design.md`):** reducible functions get Ramsey's
dominator-tree translation into `loop`, labeled blocks, `if` and `match`;
each irreducible region stays v1's `loop { match pc }` as one collapsed
node; overlays and functions that fail the recompile-time structure check
stay v1. Every edge keeps v1's flag writes, cycles, tick and yield return;
nothing merges ticks, re-tests a condition or copies a block. The
structured body runs only from the function's entry. By default (asked of
june, 2026-10-24) the dispatch table then holds only function entries and
resumes run in the interpreter until the next entry (on the Emerald
new-game route: frames exact, compiled share 99.2% to 93.1%), and tail and
indirect calls settle at the call site and IRQs run inside the tick
without unwinding, to make resumes rare. The alternative is a second, v1
body per function for resumes.

**Why:** the reviews (`docs/research/structured-control-flow.md`,
`static-recompilers.md`) find Ramsey's translation the only technique
that adds no code and no computation on reducible input, and 99.5–99.9%
of Emerald's and modern GCC's functions are reducible. No paper handles
our many entries; one body keeps the generated code from growing.

**Would change our minds:** the structured form making LLVM slower to
build than it makes frames faster; resumes staying common after the
non-unwinding steps.

**Still open** (open-decisions.md, 8): the frame end without unwinding,
and the deepest host stack.

## 10. ABI trust: modular checking at open edges, inference inside

Keeping registers in Rust locals across calls needs per-function summaries
(reads, clobbers, preserves). june's framing, which we adopt: Hoare-style.
Give specs to the connectives and builtins (BIOS calls), check each function
against its spec, and whole-program soundness follows by composition.
Inference is "where the constraints get solved": bottom-up over the call
graph, which needs every caller in view.

So: infer summaries for functions whose callers are all known (direct `bl`
only). At open edges (address-taken functions, anything the interpreter can
call into, indirect-call targets) the spec is fixed to AAPCS and checked, not
inferred. Same shape as inferring private functions but requiring
annotations on exports. A function that fails its check stays in safe mode
(all registers in `cpu.r[]`).

Interrupts are only taken at sync points (decision 11), where everything is
written back, so they don't break this. Oracle: run safe and trusting builds
and diff register state.

**When:** not before milestone 4. Desktop doesn't need the speed.

**Would change our minds:** the diff oracle finding mismatches we can't
explain, or open edges being so common that inference buys nothing.

**Re-scoped (2026-10-24, `structured/design`, from
`docs/research/registers-and-types.md`):** registers in locals need no ABI
trust. Inside a function, guest registers and NZCV live in Rust locals,
written back at every point something outside the function can read the
CPU (calls, exits, `swi`, PSR and user-bank ops, every tick's yield path)
and reloaded after the ones that write; LLVM's SROA does the promotion.
That is the new "safe mode", and it is exact by construction. Summaries
then only shrink a direct call's write-back and reload sets (no push or
pop can be deleted: the stack is guest memory), so they wait until the
locals spike shows calls matter. If built: Van Emmerik's bottom-up
preservation analysis with `<all>` for unknown callees, checked as
`r_exit = r_entry` over all paths (rev.ng), a stack-slot restore trusted
only where value sets prove no store between can hit the slot, agbcc's
convention (r4–r10 and sp preserved) as the fixed spec at open edges, and
every failure staying in safe mode. Stack slots stay in memory and no type
recovery goes into codegen.

## 11. Timing

**Decided (milestone 2, interpreter #27, codegen #31):** an estimated cycle
count, not cycle accuracy. `Machine::tick(cycles) -> bool` advances a
`u64` clock and runs scanlines (1232 cycles a line, H-blank from 1006, 228
lines), timers (lazily), DMA (in the bus store, so compiled code loses no
copies) and IRQs. The interpreter ticks per instruction; generated code
ticks at every block exit with the block's summed cost
(`armv4t::cycles::cost`, shared by both).

**Wait states (emerald/wait-states, 2026-10-21):** counted as mGBA counts
them (`runtime::waits`). An instruction's `Cost` is its base cycles plus a
count of sequential and nonsequential code fetches, priced at run time from
the region's `WAITCNT` wait states (16- or 32-bit), so generated code emits
`base + rt::fetch(m, region, thumb, s, n)`. Data accesses through the CPU's
bus trait add their region's nonsequential waits to `Bus::data_cycles`,
which the next tick takes; DMA units cost their regions' waits. With the
prefetch buffer on, each memory instruction run from ROM ends with mGBA's
`GBAMemoryStall` (`rt::stall` in generated code, the same call in the
interpreter). Not modelled: VRAM stalls in mode 2 and EWRAM's wait
control register. Without the waits Emerald ran about 2x fast and skipped
mGBA's lag frames; with them its intro reaches `Task_Scene1_Load` at frame
204, as mGBA does.

**Time that hangs on values (emerald/intro-rayquaza, 2026-10-22):** also
as mGBA counts it. An instruction whose condition fails costs 1S
(`cycles::SKIPPED`), not its own cost; a multiply's internal cycles come
from its operand (`cycles::mul_wait`, 1 to 4 by its top bytes), its next
fetch is N, and the prefetch stall overlaps them as data; an `ldm`/`stm`'s
accesses are all sequential, the first taking its region's N less S off
once (`Bus::block`). The interpreter applies them in `Machine::step`;
generated code gets them from IR `Timing` ops the builder adds under the
instruction's condition (`rt::skipped`, `rt::multiply`, `rt::block`).
Before, Emerald's sound mixer ran 6 lines a frame long and the Kyogre
scene's load missed a V-blank that mGBA makes.

IRQs, halts and frame ends are handled only at the dispatcher's top, for
compiled and interpreted code alike: one IRQ path, the BIOS stub done in
Rust (`take_irq`/`irq_return`, handler at `0x03007FFC`). An IRQ taken out
of compiled code returns to a block start; the fallback takes them between
any two instructions. Blocks are capped at 256
instructions to bound IRQ latency; there is no mid-block sync after I/O
stores.

**Why:** the GBA games we target sync on V-blank, not on exact cycles;
block-granular ticks keep the fast path an add and three compares.

**Costs:** IRQs land at slightly different instructions in the
interpreter and compiled code, so the checks compare final state (frames,
registers at an idle loop), not traces. An IRQ enabled by an I/O store is
taken at the end of its block.

**Would change our minds:** a ROM that needs IRQs within a few
instructions of the store that enables them (then split blocks after
constant I/O stores), or one whose behaviour depends on wait states.

**Play builds don't keep mGBA's timing (mod/no-timing-flag, june,
2026-10-30):** `gbaa-recompile --timing instant|static|exact`. A game's
build and a cut default to instant: every fetch as if the prefetch buffer
always had it (no wait states), data accesses free, one cycle constant per
block exit, and no instruction address on any access. exact (everything
above) stays for the checks, which all pass `--timing exact`: mGBA is still
the oracle. On Kirby instant loses a few lag frames and shifts the RNG,
nothing a real GBA couldn't do; june can't tell it apart and finds it more
responsive. docs/research/mod-view-without-timing.md.

**The interpreter plays instant too (june, 2026-10-31):** `gbaa-desktop
--timing instant|exact|static` (`runtime::Machine::timing`). Playing a ROM
in the interpreter is instant by default; a recompiled build's fallback and
`--interp` follow the build's own timing (`Code::timing`). Every check
that runs `gbaa-desktop` or a build passes `--timing exact`; `gbaa-interp`,
a check runner, stays exact unless told. Netplay's build ID holds both the
code's timing and the interpreter's, so peers on different timing are
refused.

**Native timing, for the native port's check (native/timing-native,
2026-11-01; june's call, d/may-native-port-s-check):** `--timing native`
(`gbaa-recompile`, `gbaa-desktop`, `gbaa-interp`, `just first-frame ROM
FRAMES native`, `route-check --timing native`) times the play build the
way a native program does (docs/native-port-design.md, sections 3 and 7):
code takes no time. The clock moves only at the BIOS waits; at the copies'
and LZ77's stall (`bios::call`'s pricing, the 45 cycles in and out
dropped), so Emerald's 96 KB VRAM fill still spans a V-blank; at DMA's
cost; at an idle loop's back edge (a backward branch over at most 32
instructions with no store, no call and no register carried from one
pass to the next, so a poll and not a `strlen` or a delay count;
`armv4t::idle`, listed by `analysis::idle`; ROM code only), which runs
the clock to the next event, and at its exit, which syncs a clock a DMA
moved; and at a stuck spin loop (below). A read of `VCOUNT`, `DISPSTAT`
or a timer moves no time (d/a-vcount-dispstat-or-timer-read-outside-a-poll):
a poll on one is an idle loop, which sees each line twice, at its start
and its H-blank. IRQs are taken only right after a sync
(`Sched::window`), so the generated code and the interpreter take every
IRQ at the same instruction: compiled and `--interp` states match byte
for byte. A native build only runs at native timing (or `--interp`), and
the other timings are unchanged.

**Spin loops (june, d/busy-loop-calls-functions-let, option (a) as
refined in docs/research/busy-wait-loops.md):** a loop that waits for an
interrupt through the functions it calls (Emerald's `DoMapLoadLoop`)
would hang, no time passing in it. A spin edge is a backward branch over
at most 64 instructions holding a call, not an idle edge
(`armv4t::idle::is_spin_edge`, listed by `analysis::idle::spin_edges`),
or one in the game's list (`crates/runtime/spins.toml`, by ROM SHA-1, for
loops the rule misses; the hole report says which busy loops aren't spin
edges). Each counts its passes on its own (`Sched::spins`); every sync
clears the counts, but a stuck loop's own, and the frame's end clears them
too. Past `SPIN_BUDGET` (2,048) passes, each further pass runs the clock
to the next event, as an idle edge. The native library's `hw::spin(g)`,
on the same loops, does the same by call site.

**Costs:** a loop that polls anything and stores, with no call and no
BIOS wait, hangs, no time passing in it, until it is listed in
`spins.toml`. Emerald's boot has one: librfu's `AgbRFU_SoftReset` polls
timer 3 while storing to `RCNT` (listed, `0x082e2b6a`); the hole report
only finds loops that call. A long loop of real work with a
call passes its budget and takes half a line a pass. An H-blank handler
runs at H-blank's start, and code after a wait it ends resumes there.
(The library's waits once stepped whole lines, returning half a line
later; since native/hw-sound they run event by event on the play build's
clock, as its scheduler does, timer overflows included, and resume where
it does.) `route-check --timing native` serves
`--same-states`; a unit's own run still compares with mGBA's hashes, made
at exact timing, so it fails there. Netplay's timed resumes
(`interp_block`) don't see idle or spin edges.

## 12. BIOS

**Decided (milestone 2, #28):** high-level emulation in `runtime::bios`,
one entry (`bios::swi`) for the interpreter and generated code. Implemented:
00–12, 14, 15 and 27 (SoftReset through LZ77, RL, CustomHalt), integer-only
like the original. The waits (`Halt`, `Stop`, `IntrWait`,
`VBlankIntrWait`, `CustomHalt`) halt by writing `HALTCNT`, as the real BIOS
does, and are finished by the IRQ-return hook, so codegen doesn't model
them. BIOS open bus follows the latch values GBATEK and jsmolka's `bios.gba`
give. An unimplemented call faults with its number and address. Calls take
time as mGBA's HLE BIOS counts it, ticked after the call (`bios::call`): 45
cycles in and out, plus `Div`'s loop, and the copies' and LZ77's memory
accesses at each region's wait states. Emerald's copyright screen depends
on it: its 96 KB VRAM fill has to span a V-blank.

**Why:** no BIOS image to ship, and the calls are small and documented.

**Costs:** each call is ours to get right; GBATEK, mGBA's HLE and the
Cult-of-GBA BIOS are the references, and jsmolka's `bios.gba` the check.

**Still missing:** HuffUnComp (13), the Diff unfilters (16–18), the sound
driver calls (19–24, 28–2A), MultiBoot (25) and HardReset (26). Emerald's m4a engine will
want some; add them when a target ROM faults on one.

## 13. Runtime

**Decided (milestone 2, #26, #29, #33, #34; sound #52):** `runtime` is `#![no_std]`
(`just check-nostd` builds it for `riscv32imac`), with the machine generic
over a `Platform` trait (`present` a frame, `keys`, `audio`: the DMA sound
FIFOs' samples, raw and as `i16` stereo, once a frame;
`docs/sound-design.md`).
A generated crate picks its platform with one type alias. The desktop
backend was minifb; it is now SDL3 (the owner's SDL3 + egui decision:
window, renderer and gamepads; built from source with cmake and a C
compiler and linked statically, dlopening X11/Wayland, GL and udev), with
integer scaling, a keymap, and a headless mode that dumps PNGs and frame
hashes. The PPU is per scanline: bitmap modes 3–5, text backgrounds in modes
0 and 1, and regular sprites.

**june's position on scope:** how far the PPU goes is limited by how long
interest lasts. Every PPU feature would be lovely; landing on just june's
agb sound ROM and pokeemerald working would also be a fine outcome. So order
the PPU work by what the next target ROM needs, not by feature
completeness.

**Still open:** audio playback on the desktop (cpal, #53), the PSG
channels 1–4, affine
backgrounds and sprites, windows, blending, mosaic, and the ESP32 backend
(stretch goal; `Machine::new` builds the bus by value, which an ESP32 will
need in place).

## 15. Test inputs (june's position; expand as we go)

Start with the existing GBA test suites (SingleStepTests ARM7TDMI, jsmolka
gba-tests, armwrestler), then an agb ROM, then the ROMs already on quack,
then go from there. None of these are committed; link them from outside.

**ROMs on quack** (`~/ROMs/gba/`): Pokemon Emerald (matches the pokeemerald
decomp: symbols as ground truth once its hash is checked), Emerald Imperium
and Emerald Rogue (romhacks built on the decomp), WarioWare Mega
Microgame$!, Super Mariomon, moonbeam. All of these are targets. An older Emerald Rogue is in
`~/ROMs/gba-originals/`.

## Bootstrap before the factory

The early work (crate skeleton, `Sem` trait and IR, unarm adapter, test
harness) isn't factory-shaped. Bootstrap it by hand, tracked as GitHub issues
in the private `junewunder/gbaa` repo. Then try one or two unit sizes, find
the sweet spot, and only then scaffold the factory.

## 16. The factory (settled with june, 2026-10-20)

The proposals in `docs/factory-units.md` were taken as written, with three
changes. **mGBA is the oracle:** a checkpoint's expected hash is what mGBA
draws, not one set by eye, so checks are mechanical where mGBA agrees.
**The goal is every GBA game**, from ROMs june supplies; Emerald is where
the factory ramps up, not the end. **Save states:** load mGBA's save states
into the runtime at a frame boundary (all guest state is in `Machine` there,
and the dispatcher resumes any `cpu.pc`), so a checkpoint deep in a game is
a state plus N frames, compared with mGBA from the same state. Two lanes on
quack. The spec is `docs/factory.md`; machines are in `docs/machines.md`.

**The satisfied line (june, 2026-10-21).** "Every GBA game" is the far
line. Done for now is june playing the Emerald romhacks natively on a Steam
Deck, with a controller, in-game saves and save states, fast-forward and
rewind if possible, and no janky emulation-layer feel. `docs/factory.md`
(Goal) has it; the units are the "Playing on the Steam Deck" group, ending
at its gate.
