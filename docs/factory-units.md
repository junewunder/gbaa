# Factory units: what milestone 2's trial units measured

Close-out of the milestone 2 sprint (brief #36, 2026-10-20). Input for
scaffolding the factory with june (wfactory skill, section 1).
**Everything marked _Proposal_ is a proposal for june, not a decision.**
Settled design stays in [decisions.md](decisions.md); the questions these
answer are "After the decisions: the factory's own design" in
[open-decisions.md](open-decisions.md).

Sources: the time/obstacle logs on the tile backgrounds (#33), sprites (#34)
and armwrestler (#35) issues; PR timestamps (`gh pr view N --json
createdAt,mergedAt`); the sprint-state comment on the runtime core (#26).

## Measured

Trial units. Wall-clock is the unit's own log (orient to merge); code is
writing the feature plus its unit tests.

| Unit | Size tried | Wall-clock | Code | Review | Diff |
|---|---|---|---|---|---|
| Tile backgrounds (#33, PR #45) | one PPU feature | 10 min | ~2 min | ~4 min | +313, 2 files |
| Sprites (#34, PR #46) | one PPU feature | 13 min | ~4 min (2 code, 2 tests) | ~6 min | +772, 3 files |
| armwrestler, interpreter (#35, PR #47) | one test-ROM case | 24 min (~6 waiting on the build lock) | ~10 min (input, OCR, recipe) | ~9 min with fixes | +474, 11 files |
| armwrestler, recompiled (#35, PR #49) | same case, second runner | 8 min (~1:40 per walk building two generated crates) | ~3 min | ~3 min | +72, 2 files |

The bootstrap issues, for contrast (draft PR opened to merge, which misses
1–4 min of orienting before the PR):

| Issue | PR | Wall-clock | Diff | What made it non-unit-shaped |
|---|---|---|---|---|
| Runtime core (#26) | #37 | 16 min | +1239 | defined the hooks four other issues used |
| Scanned evidence (#25) | #38 | 24 min | +214 | five scored checks (`sst`, `sst-ir`, `jt-score`, `disc-pret` ×2) |
| Desktop + bitmap modes (#29) | #42 | 12 min, PNG check 18 min later | +1618 | stand-in runner until the interpreter (#27) landed |
| Interpreter on runtime (#27) | #41 | 31 min | +1754 | took over DMA and timers by overseer call; two reviews |
| BIOS HLE (#28) | #40 | 39 min | +1826 | shared the halt/IRQ path with #27; two reviews |
| Codegen v1 (#31) | #39 | 73 min (design note first) | +2670 | design note reviewed before code; waited on #27/#28 |
| Discovery on agb (#30) | #44 | 63 min | +567 | open-ended accuracy work; two reviews |
| First frame (#32) | #50 | 21 min | +254 | integration of all of the above |

The sprint ran 12 PRs in ~1 h 55 min (16:08–18:01 UTC) with up to five
threads at once. Wall-clock alone doesn't separate the two groups. What
does: every bootstrap issue needed an overseer call or another issue's hook;
the trial units needed none, except the two splits forced by unmerged
dependencies (#34 rendered through a throwaway BIOS stub until BIOS HLE merged; #35's recompiled half waited on
codegen and first frame).

## Unit size that worked

- **One PPU feature is too small.** Fixed cost (orient, PR, check fixes,
  review, log) took 7–9 of 10–13 minutes; code took 2–4. #34's log: "a
  factory unit could reasonably be two features like this one."
- **One test-ROM case is about right**: 24 + 8 = 32 min, fixed cost about a
  third. The unknown was the ROM's result channel (screen text only, no
  memory), which cost the OCR tool.
- **One SingleStepTests file is gone as a size**: `just sst` already passes
  every file but documented hardware quirks (`EXPECTED` in
  `armv4t/tests/common/sst.rs`).
- **One BIOS call is too small**: all 22 calls BIOS HLE (#28) emulates took 39 min as
  one issue.

_Proposal:_ size units at **20–35 min of wall-clock with fixed cost at most
half**, which in practice is:

- two related PPU features with one test program covering both (windows +
  blending, affine backgrounds + affine sprites, mosaic + modes 1/2);
- one test-ROM case, with its result channel named in the brief;
- one target step: "ROM X at frame N with input script S draws hash H,
  recompiled equal to interpreted", for the agb sound ROM and later Emerald;
- the remaining BIOS calls a target needs, batched by target.

## Fixed cost vs code cost

| Cost | Per unit | Notes |
|---|---|---|
| Orient: issue, brief, prior unit's log, hook point, open draft PR | 1–4 min | 4 min when the ROM's source had to be read for its result channel |
| `just check` failures (fmt rewrap, clippy) | ~1 min | run `cargo fmt` and clippy before the first full check |
| Fresh-subagent review, then its fixes | 3–9 min | found a real bug or a false pass in 3 of 4 trial PRs: 8bpp 2D mapping ignoring tile bit 0 (#34), stale dumps read as results twice (#35) |
| Log and merge | ~1 min | |
| Waiting on the build lock | 0–6 min | 6 of #35's 24 min; its ROM runs compiled nothing |
| **Code + unit tests** | **2–10 min** | |

Keep the review: it is the largest fixed cost and the one that earned it.
Cut the lock wait instead (see resource rules).

## A unit's check command

A `just` recipe that **starts from empty outputs** and **decides by frame
hash, registers or counts**. Stale outputs produced a false pass twice in
#35 (a sabotage run's dumps, then a failed run's dumps read as passing).
Eye checks set the expected value once; after that the hash decides.

_Proposal_ (not built yet; a sketch of the shape):

```just
# One unit's check: the interpreter's frame and registers match the recorded
# ones, then the recompiled build matches the interpreter.
unit name:
    #!/usr/bin/env bash
    set -eu
    out=out/units/{{name}}
    rm -rf "$out" && mkdir -p "$out"                  # never read a stale dump
    source units/{{name}}.env                          # ROM FRAMES INPUT HASH
    {{lock}} cargo build -q --release -p desktop
    systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0 \
        target/release/gbaa-desktop "$ROM" --headless --frames "$FRAMES" \
        --input "$INPUT" --hash --regs --dump "$out/interp.png" > "$out/interp.txt"
    grep -qx "frame $FRAMES hash $HASH" "$out/interp.txt" \
        || { echo "FAIL {{name}}: $(head -1 "$out/interp.txt"), want $HASH"; exit 1; }
    just first-frame "$ROM" "$FRAMES"                  # recompiled = interpreted
```

- One file per unit, `units/<name>.env` (ROM path under `$GBAA_DATA`, frames,
  input script, expected hash, and a line saying how the hash was verified),
  so parallel units never edit a shared list.
- Unit outputs go in `out/units/<name>/`, apart from generated crates in
  `out/<game>/` (#35's name clash: `just recompile armwrestler.gba` refused
  to overwrite the unit's dumps).
- Test-ROM cases print pass/fail from their named result channel (r12 for
  jsmolka's, a memory word, or screen text + font like `aw-check`) and fail
  on a short count, as `just armwrestler` does with its 79 results.
- New test programs: agb source committed under `tests/agb/<name>/` with a
  `SCENE.md` listing expected pixels (as `$GBAA_DATA/tools/obj-sprites/`
  did), built by `just agb-build` into `$GBAA_DATA/agb/`. june allowed this
  on #36 (source only; ROMs and ELFs never committed). The agb sound ROM's
  `$GBAA_DATA` copy may be rebuilt with other tracks enabled.
- `first-frame` takes ~1:40 per ROM because it builds a generated crate.

## Resource rules for a factory lane on quack

quack: 24 cores, 30 GB RAM, of which the desktop and browser usually leave
~10 GB available; 7 GB swap, often full. In this sprint five threads each
compiling on all cores pushed load to 77, and codegen's SingleStepTests
harness was OOM-killed twice at 8.8 and 10.4 GB, taking june's desktop with
it (a self-jump case looped inside generated code while the harness logged
every access; fixed in #39 by making every case end after one exit).

1. **Every compile** runs under `CARGO_BUILD_JOBS=6
   scripts/build-lock` (three at once). The recipes take it themselves (the justfile's
   `lock`), around the cargo step only, so a built binary runs without
   waiting; a bare `cargo build/test/clippy` takes it the same way. Never
   wrap `just` in it: a recipe taking it under a caller holding it deadlocks.
2. **Every run of a generated or new harness binary** runs under
   `systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0`
   (6G for a known-heavy harness). Prove memory is small and flat on a tiny
   input first (`/usr/bin/time -v`, maximum resident set size).
3. **Before a heavy job**, `free -g` available must be at least the job's need
   + 3 GB. A generated crate's release build needs 4 GB
   (`GBAA_GEN_NEED_GB`); `build-recompiled` checks it.
4. Generated loops never terminate on their own in a harness: each case ends
   after one exit.
5. `systemd-run` expands `$VAR` in its own arguments: write `$$VAR`, or put
   the shell in a script file (#35 lost its exit code to `${PIPESTATUS[0]}`).
6. Local experiments (a stub, a sabotage run) go in a scratch copy and a
   scratch output directory, never the worktree or `out/` a reviewer reads.

_Proposal:_ move the lock inside the recipes, around the cargo step only, so
running a prebuilt binary never waits (that would have saved ~5 of #35's 24
min). Lanes then call `just` without wrapping it, because a recipe taking
the lock while its caller holds it deadlocks. Until that change merges,
rule 1 stands as written.

## Proposed answers: the factory's own design

Numbered as in open-decisions.md. All proposals for june.

1. **The unit.** The four kinds under "Unit size that worked", 20–35 min
   each. Drop "one instruction format" and "one SST file" (the CPU suites
   already pass) and "one BIOS call" alone (too small).
2. **The oracle.** `just check` stays the merge gate (fmt, clippy, tests,
   no_std; 38 s from a fresh worktree on quack, with 6 jobs). Each unit adds a computed check as above.
   `just gba-tests`, `just armwrestler`, `just sst`, `just sst-ir`,
   `just jt-score`, `just disc-pret`, `just disc-agb` keep their bars and
   run when a unit touches their crate, as the sprint did. mGBA
   differential is milestone 3; until then a new expected hash is set by
   hand-derivable expectations (`.asm` sources, `SCENE.md`, GBATEK).
3. **Mechanical or independent.** Mechanical once a unit's hash is recorded.
   Independent (a fresh subagent compares the PNG with `SCENE.md` or the
   `.asm`) whenever a hash is first recorded or changed, because that hash
   was set by eye. Keep the fresh-subagent code review on every unit.
4. **Done and kill.** A target is done when its steps pass `just unit`
   recompiled, with the compiled share of guest cycles reported (june's ROM:
   97.9% at 60 frames, 92.1% at 600; the rest is agb's IWRAM code, decision
   7). Kill: a unit passing 3× its size (~90 min) stops and writes what it
   learned; a change to any recorded hash or bar without a stated reason
   fails the unit.
5. **Gates.** june looks (a) when the agb sound ROM plays recompiled in a
   window, before Emerald starts, because sound can't be judged by a frame
   hash yet; (b) before any change to a settled decision; (c) before a
   discovery or jump-table bar is lowered.
6. **Computed.** Every unit with a `units/<name>.env` file or a test-ROM
   recipe. Recorded: research units and docs.
7. **Commit policy.** The repo now has a private origin
   (github.com/junewunder/gbaa), unlike this item's "no origin". Default
   from the skill: one branch per unit, the loop merges finished branches
   into main after `just check` and the unit's check pass and pushes;
   sessions never push, never force-push. The unit's time/obstacle log goes
   in the commit message, since there's no issue to comment on.
8. **Resources.** The rules above. Heavy count 1 (one generated-crate build
   or capped heavy run at a time), plus the build lock for compiling.
9. **Machines.** quack only, **2 lanes**. Builds serialize on the lock
   anyway: with five threads, a 3-second ROM run waited 1–2 min about three
   times. Raise to 3 if lock waits stay under ~10% of unit time.
10. **Shared code, as units before the lanes start:** the `just unit` recipe
    and `units/` format; a `tests/agb/` template with its `agb-build` entry;
    a halfword-fill helper for PPU tests (byte stores to OBJ VRAM and OAM are
    dropped, as on hardware: 12 of 13 of #34's tests failed on that); a
    total-instruction count in `dispatch::Stats`, so "mostly recompiled"
    becomes a number a check can bar; the lock-inside-recipes change.
11. **Never lost.** Written, committed: `tests/agb/` sources and `SCENE.md`,
    `units/*.env`, recipes, decisions, committed seeds (decision 6).
    Derived: `out/<game>/` (`just recompile`), `out/units/` (`just unit`),
    `$GBAA_DATA/agb/` (`just agb-build`), test suites (`just fetch-tests`),
    truth files (`just disc-truth`). Outside git and not rebuildable:
    `~/ROMs/gba/` (read only).

The AGENTS.md visibility line this section asks for exists already
(`Visibility: private (github.com/junewunder/gbaa)`).

## What a unit's brief must name

Each item cost a trial unit 1–4 min:

- The ROM's display mode and result channel. #35's brief said tile-mode
  text; armwrestler draws mode 4 bitmap text, screen only.
- Its "after" line: every unit whose merge it needs (#34 waited on BIOS HLE
  because agb's start-up calls `swi 0xB` first).
- The hook point it plugs into (`bg_line`, `ObjLine` in compose): both PPU
  units found theirs already named in `ppu.rs`, and that is why they were
  quick.
- Known bus rules its tests will meet (byte writes to OBJ VRAM/OAM dropped).

Tooling notes: `gh issue view N --comments` prints nothing on an issue with
no comments, so read issues with `--json`. PRs can't take PNGs from the CLI,
so a unit's PNGs stay in `out/units/<name>/` and the hash goes in the PR.

## Defaults taken writing this

- The overseer, not june, answered this close-out's questions; every
  _Proposal_ above is open for june at scaffold time.
- Lane count (2), unit time band (20–35 min) and kill bound (3×) come from
  four trial PRs; treat them as first guesses to re-measure after the first
  ten factory units.
