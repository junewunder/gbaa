# Where the design walkthrough stopped

Pick up here. Settled decisions are in [decisions.md](decisions.md); risks in
[risks.md](risks.md); papers and projects in [references.md](references.md).

**june's depth (2026-10-19):** decisions 1–5 were in june's depth. From
decision 6 on, most are more in the weeds than she can judge alone. She
trusts Claude's defaults; anything genuinely uncertain becomes a research
unit (one or two research agents), and a Claude walks her through the
result before it's locked in. Discuss with her directly only what's in her
depth.

**How the walkthrough works:** one decision at a time. For each: what it is,
the real alternatives, why this one and what it costs, what would change our
minds. june pushes back; where she picks differently, the spec follows her.
Nothing below is settled. Once all are done, settle the factory's own design
(end of this file), then scaffold with the wfactory skill.

**Settled so far:** 1 static recompilation (it's fun; ESP32 is a stretch
goal), 2 emit Rust (LLVM IR as fallback, no C), 3 `Sem` trait plus a small
IR, 4 `unarm` behind our own enum, 5 code discovery, 6 profile-guided discovery.
Milestone 2 (first frame, #32) settled 8 one Rust fn per guest function,
11 timing, 12 BIOS and 13 runtime; what's left of them is below.

## 7. Code copied into IWRAM: what's left

Settled for agb ROMs in decisions.md (7, #54): overlays found from the
BIOS copies, one per function, guarded by their own instruction bytes.
Still open:

- Copies discovery doesn't see are found by running the ROM at
  recompile time (decision 7, copies seen running), so a copy made only
  after 1800 frames with no input still runs in the fallback. Copies into
  EWRAM (no ROM we have runs code there), code run on the stack
  (agb_flash's `ReadFlash_Core`: its address moves with the call depth;
  position-independent overlays would cover it) and code built at run
  time (no ROM bytes to compile) stay interpreted. Code that patches
  itself is compiled for each version `gbaa-recompile` sees in its
  1800-frame run (decision 7, patched versions); later versions fall back.
- RAM code reached only through a computed address (a function pointer in
  data) isn't an overlay entry, so it stays interpreted. jsmolka's `nes`
  copies its code into VRAM, where no overlay goes, so it all runs in the
  fallback.

## 8. One Rust fn per guest function: what's left

Settled in decisions.md (8), structuring included (2026-10-24, design in
`docs/structured-codegen-design.md`). Still open:

- Resumes: interpreted with one structured body (the default), or a second
  v1 body per function; asked of june.
- The frame end still unwinds every compiled frame; handing it off without
  unwinding waits for the resume counts after the call and IRQ steps.
- The deepest host stack hasn't been measured (risks.md).

## 9. Flag liveness and constant propagation

- Flag liveness: ARM sets NZCV constantly, most of it dead; RISC-V has no
  flags register. Backward analysis over the decision-3 IR.
- Const-prop: literal-pool loads become constants; stores to known I/O
  addresses skip the memory map.
- Built (2026-10-21): both passes, block-local const-prop and
  function-wide flag liveness (`docs/sem-design.md`, "As built"). Open:
  cross-block entry values for const-prop, direct I/O store calls.

## 10. ABI trust

Re-scoped in decisions.md (10, 2026-10-24): registers in locals need no ABI
trust. Still open: whether call summaries are worth building at all, for
the locals spike (`structured/regs-locals`) to say.

## 11–13. Timing, BIOS, runtime: what's left

Settled in decisions.md (11, 12, 13). Still open: mid-block sync after I/O stores, if a ROM needs
IRQs sooner than its block's end; the unimplemented BIOS calls (Huffman,
Diff, the sound driver, HardReset; MultiBoot in normal mode); the PSG sound channels and
desktop audio playback (DMA sound is done, #52); affine modes, windows, blending
and mosaic; the ESP32 backend.

## 14. Workspace layout

Crates `armv4t` (decoder adapter + semantics), `analysis`, `codegen`,
`runtime` (no_std), `desktop`, `esp`; generated code in `out/<game>/`.

## 15. Testing / oracles

- jsmolka's gba-tests and armwrestler (test ROMs reporting pass/fail) for the
  interpreter.
- **New from the decoder research:** SingleStepTests ARM7TDMI JSON tests
  (per-instruction before/after CPU state, MIT): an exact oracle for the
  `Sem` interpreter and natural factory units.
- Decoder sweep: unarm vs our adapter (and vs `arm-none-eabi-objdump` or
  `llvm-objdump`, neither installed on quack yet; both from dnf).
- Differential testing against mGBA. Built (#55): `scripts/fetch-mgba.sh`
  builds a static libmgba 0.10.5 (the flatpak's version) under
  `$GBAA_DATA/tools/mgba/`, `mgba-dump` runs a ROM headless on it (built-in
  HLE BIOS, not skipped), and `just mgba-diff` compares its last frame's hash
  with our interpreter's, with a known-mismatch list. Default: the last
  frame's hash, the first comparison the factory's units are judged by.
  Still open: registers at each function entry (needs hooks into mGBA's
  CPU), and moving past 0.10.5, which fails two of jsmolka's newer CPU
  tests (#60).
- Interpreter vs recompiled build.
- ROMs written with `agb`.

## Milestones as proposed

1. Decoder + interpreter passing gba-tests and armwrestler (now also
   SingleStepTests).
2. Recompile a small agb ROM (mode 3) and run it on the desktop runtime.
   Done 2026-10-20 (#32): june's ROM (a tiled background, not mode 3),
   jsmolka's `arm`/`thumb` and an agb sprite example draw the same frame
   recompiled as interpreted (`just first-frame`).
3. Differential testing against mGBA.
4. Flag liveness + const-prop, measured by generated-code shrinkage.
5. A real game with a decomp (pokeemerald, pokeruby) to check analysis
   against symbols.

**Plan (2026-10-19, with june):** milestone 2 is one more sprint run by
hand (brief: issue #36), ending in three trial factory units (tile
backgrounds, sprites, armwrestler) that size the factory's units. june is
writing the first-frame ROM in
`~/Documents/homelab/gba-cart1` (done 2026-10-20: a 4bpp tiled
background, so tile backgrounds come before first frame); agents only
build it.
armwrestler moved out of milestone 1: it needs tile modes and input.
After first frame, the target is an agb ROM, then Emerald. june
prefers big sprints so they steer less often.

**Plan (2026-10-21, with june):** the next three phases, in order.

1. **Lean codegen, now.** Generated Emerald is ~8 Rust statements per
   guest instruction, and rustc's front end needs ~55 KB of memory per
   instruction (part p07, 20k instructions: 1.13 GB, 56 s). That's why it
   takes 42 part crates, and the full build peaks at 7.2 GB with 6 jobs.
   Milestone 4 (flag liveness, const-prop) plus a leaner printer
   (constants inline, single-use values as expressions, one call per
   multi-result prim). The part crates stay until the code is lean; Emerald
   will still need some split after that.
2. **Structured codegen that still matches exactly** (open decision 8):
   `if`/loops instead of `loop { match pc }`, then registers as Rust
   locals (decision 10). These are a decompiler's next steps, but every
   one must keep the frame hash matching the interpreter.
3. **Then more ROMs than Emerald.**

## Planned research units (become tracker units at scaffold time)

- **Jump-table recovery: value-set analysis vs per-compiler patterns**
  (decision 5c). Try both on real inputs (agb/LLVM, agbcc, newer GCC) and
  read the prior art (pret's gbadisasm, Ghidra's ARM switch analysis, IDA).
  If still unclear at the end: block on a question to june with a written
  report.
- **Discovery prior art** (decision 5): Ghidra, IDA, pret's gbadisasm, and
  the pret decomps (pokeemerald etc.: GBA→C by hand, with symbol maps); what
  we can borrow for recursive descent, mode tracking and the pointer scan.

## After the decisions: the factory's own design

Settled 2026-10-20: decision 16 in decisions.md, spec in `docs/factory.md`.
