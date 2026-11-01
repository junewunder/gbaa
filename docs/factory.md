# gbaa: the factory

The standing spec. `tracker/` holds state, questions and decisions (through
the tracker CLI), and this file holds what doesn't change from unit to unit. Replace
every fill-in placeholder before the first unattended run (`just loop` refuses
until then), and keep it short: rules here, each with a pointer; how a tool
works goes in `docs/tools/<tool>.md` (`just lint` caps this file at 4,000
words). A loop session gets "The loop" below in its prompt (keep that
heading), with any section whose heading ends "(every session)", and looks
up the rest when its unit needs a rule.

## Goal

gbaa recompiles Game Boy Advance ROMs to Rust and runs them on its own
runtime. The goal is **every GBA game**: each ROM recompiles, and the
recompiled build draws and plays what mGBA does at the game's checkpoints,
with almost all guest time in compiled code. Pokémon Emerald is the
turning point where the factory ramps up, not the end. june supplies the
ROMs (`~/ROMs/gba/`, read only); agents never download ROMs.
Settled design: `docs/decisions.md`; what's still open takes its listed
default (`docs/open-decisions.md`).

**The far line:** every GBA game recompiles and plays as mGBA does, from
ROMs june supplies. The direction, not the finish.
**The satisfied line** (june, 2026-10-21): june plays Pokémon Emerald
romhacks (Emerald Rogue, Emerald Imperium) natively on a Steam Deck: a
controller works, the game saves in-game and to save states, fast-forward
and, if it can be done, rewind work, and it never feels like a janky
emulation layer (steady frame pacing, clean audio, almost all time in
compiled code). This is "done" for now; the far line is where it goes
after.
**Stretch, past the satisfied line** (june, 2026-10-21): builds for macOS
and Windows too, with controller support on all three and remapping, global
or per ROM. Choices made now (input library, config format) shouldn't rule
these out. Both lines are provisional: each change goes here with what showed it.
**Next, past the satisfied line** (june, 2026-10-21; met 2026-10-24 on quack):
the three phases in `docs/open-decisions.md` ("Plan (2026-10-21, with
june)"): lean codegen (done), then structured codegen that still matches
exactly (the `structured` group, ending at june's gate), and only then more
ROMs than Emerald. Don't start other games before that gate passes.

**Worth the tokens because:** the work is many similar, separately checkable steps (one screen feature,
one test ROM, one game checkpoint), and mGBA judges each without june.

## The unit

20–35 minutes of wall-clock, fixed cost at most half
(`docs/factory-units.md` has the measurements). Four kinds:
- **two related screen features** with one test program covering both
  (windows + blending, affine backgrounds + affine sprites, mosaic + modes);
- **one test ROM**, its result channel named in the unit;
- **one target checkpoint**: ROM X at frame N (from boot with an input
  script, or from a save state) matches mGBA, recompiled equal to
  interpreted;
- **the BIOS calls a target needs**, batched by target.
Shared code several units need is a unit of its own that they need. A unit's
detail names its display mode and result channel, the units it needs, the
hook point it plugs into, and bus rules its tests will meet (byte writes to
OBJ VRAM and OAM are dropped).

**How "done" is decided** (each unit's `check`): *computed* for every unit with a `units/<name>.toml` route
(probe `just unit <name>`) or a test-ROM recipe; *independent* where mGBA
is known wrong (`mgba_hash` and `mgba_why` on its checkpoint in `units/*.toml`); *recorded*, with
evidence, for shared code, research and docs.

**Inputs it may read:** the repo; `$GBAA_DATA` (default
`~/.local/share/gbaa/`: test suites, mGBA's build, source copies, save
states in `states/`); `~/ROMs/gba/` read only. Never june's own checkouts.
**What it produces, and where:** code and tests in `crates/`, a
route `units/<name>.toml` per ROM path (its checkpoints, one check;
`docs/tools/commands.md`), agb test programs in `agb/<name>/`
(source only; builds go to `$GBAA_DATA/agb/`), docs in `docs/`. Outputs go
in `out/units/<name>/`, generated crates in `out/<game>/`.

## The oracle

What decides a unit is right. `just check` is the mechanical part and must pass
before anything is marked `done`.

- **`just check` runs:** fmt, clippy `-D warnings`, the workspace tests,
  `check-nostd` and `rebuildable` (AGENTS.md). The merge gate.
- **Beyond `just check`:** **mGBA is the oracle.** A checkpoint's
  expected hash is what mGBA draws (`just mgba-diff`), never one set by eye;
  `just unit <name>` clears the unit's outputs, then needs mGBA = our
  interpreter = the recompiled build. The scored checks keep their bars and
  run when a unit touches their crate: `sst`, `sst-ir`, `jt-score`,
  `disc-pret` (both builds), `disc-agb` for `armv4t`/`analysis`;
  `first-frame`, `gba-tests`, `armwrestler`, `mgba-oracle` for `runtime`/
  `codegen`. Evidence cites the hashes, counts or pass lines they printed.
- **Known noise:** none: runs are deterministic. A difference is a bug or a
  known mismatch, never noise; never rerun to make it go away.
- **Lanes share `work/`:** `just check` never looks at `work/`: outputs
  are per clone, in `out/`, so another lane's half-done unit can't break it.
- **The oracle checks the real target,** not a convenient stand-in. If the
  result ships somewhere (a device, stock software, a printed page), the check
  runs there or against something proven equivalent.

**Independent check:** mechanical where mGBA agrees; independent (a fresh
context compares our frame with the test's `.asm`, a `SCENE.md` or GBATEK)
where mGBA is known wrong, or when a hash is recorded without mGBA.
- *mechanical*: `just check` passing is enough; the worker records the unit done.
- *independent*: a context that didn't do the work checks it. In a session,
  spawn a verifier subagent with a fresh context (give it the unit, its
  acceptance criteria and where the output is; ask for a verdict with
  evidence; a cheaper model is fine for mechanical checks). In the headless
  loop the next iteration picks up `review` units. The tracker enforces this
  for units whose check is `independent`.

**Never make the oracle pass by changing the oracle.** Editing tests, goldens,
baselines, scenarios or pinned dependencies to turn a red check green is a
decision for the owner (`just ask`), never a way to finish a unit.

## What must never be lost

Everything this factory makes is either in git, or rebuilt by a command in
git from inputs that can be fetched again. `work/`, the store and any
editor project are caches of CPU time; losing them costs a rebuild, never
work. If a file vanished and running the scripts wouldn't bring it back
without judgement calls, it belongs in git.

- **Written, so in git:** code, recipes, `units/*.toml`, `agb/<name>/` sources
  and their `SCENE.md`, docs, decisions, known-mismatch lines, committed
  seeds (decision 6).
- **Inputs outside git, never rebuilt:** `~/ROMs/gba/` (june's, read only)
  and save states in `$GBAA_DATA/states/` (made by playing or by cheats in
  mGBA; each is named in the `units/*.toml` that uses it and worth backing
  up).
- **Derived, so rebuilt:** `out/<game>/` (`just recompile`), `out/units/<name>/`
  (`just unit`), `$GBAA_DATA/agb/` (`just agb-build`, `just agb-tests`),
  test suites (`just fetch-tests`), truth files (`just disc-truth`),
  mGBA's build (`scripts/fetch-mgba.sh`).
- **Hand fixes to derived files** become a pipeline step, and a unit with
  one isn't done until a rebuild reproduces it: fix the generator or its
  input first; else a script step that makes the change; a binary patch
  (xdelta, bsdiff) from the pipeline's output, applied by that step, only
  as a last resort.
- **`just rebuildable`** lists files on disk with no source or copy in git;
  `just check` runs it. It checks that every input a
  `units/*.toml` names exists, and lists untracked files in the repo.
- **The drill:** not used: every unit's check already rebuilds its
  outputs from empty (`just unit` clears `out/units/<name>/` first).

## Done and stopping

- **The factory is done when:** june passes the Steam Deck gate
  (`deck/play-gate`): the satisfied line, above. Every ROM june supplies is
  the far line, for after.
- **Stop and report to the owner when:** a unit still going at ~3 hours stops,
  writes what it learned (`just note`) and is split into smaller units or
  asked about. Up to an hour or so is normal, not a sign of trouble (june,
  2026-10-21); 20–35 minutes is only the size units are planned at; a change to any recorded hash or bar without a stated reason fails
  the unit; the compiled share of guest time on a target falling.
  Ask the owner about what you found (`just ask`) and stop; a factory that
  keeps running past its point is spending the owner's usage on nothing. A
  unit that needs the owner is not a stop rule: it waits on its
  question and the loop takes the next workable one, until `just next` has
  nothing left.
- **Gates:** a unit of kind gate (`just tracker add-unit <id> --gate --title
  "Playtest Marth"`) holds back the units that need it, and what needs those,
  until the owner passes it (`just pass <id>`). Work that doesn't need it goes on.
  Here: june looks (a) when Emerald's title screen runs recompiled in the
  window, before gameplay checkpoints; (b) before any change to a settled
  decision; (c) before a discovery or jump-table bar is lowered. (b) and (c)
  go through `just ask`.

## The loop

"Continue" means: run this loop for **one unit**, then report. No other
context is needed. Assume the previous session was cut off mid-unit. "Oversee"
means something else: run the lanes detached (`just fleet`) and supervise
them, following the overseer's guide (`just oversee-guide`).

1. **Orient.** Know which machine this is and what it can do: its section of
   `docs/machines.md` (a loop session has it in its prompt). Run
   `just status`. Run `git status` and leave changes you didn't make alone:
   another agent may own them.
2. **Pick and claim.** `just next --claim` names the unit, claims it and prints
   its card: what done means, its needs, the decisions that apply (titles;
   `just show <id> --full` for why), notes from earlier attempts. (The
   headless loop has already done this for you, and checked out the unit's
   branch.) An interrupted unit means a session
   stopped mid-unit: the card says where its work is (its branch, which the
   loop checks out, or what it left uncommitted). Keep what's good, finish it.
3. **Do** the unit, within the rules in `AGENTS.md`.
4. **Check.** Run `just check`. Then, for any unit that changed code, a
   fresh subagent reviews the branch's diff (`git diff origin/main...HEAD`)
   for bugs; fix what it finds that's real, and name the review in the
   evidence. If the unit's check is independent, get it now
   (verifier subagent), or `just review <id> --evidence "…"` and leave it for
   a later session.
5. **Record**, the moment the unit is settled:
   - computed: `just recheck <id>` (the tracker runs its probe; you don't mark
     it). recorded or independent: `just done <id> --evidence "…"
     [--checker <tag>]`. Or `just drop <id> --why "…"`.
   - What the next attempt must know: `just note <id> "…"` (280 characters).
   - A judgement call: `just decide --title "…" --why "…" --scope <selector>`
     (the units it applies to; `--scope '*'` for every unit).
   - A discovered fact that would cost real time to rediscover goes in
     `memory/` (see the memory rules in `AGENTS.md`).
   - A new unit you discovered: first `just find <words> --open`; if it's
     filed already, `just note` that unit instead. If not,
     `just tracker add-unit <group>/<slug> --title "…"`, with `--uses <resource>` for every resource it needs, checking included.
     If several units will need the same new code (a family of similar
     items), that code is a unit of its own, and they need it (`--needs`).
     Two sessions on two machines must never design the same module twice.
6. **Commit:** one branch per unit (the loop's), message `<id>: <what>` with the
   unit's time/obstacle log in the body. Sessions never push or force-push;
   the loop merges into main after `just check` and the unit's check pass,
   and pushes.
   Stage only this unit's files (`git add <paths>`, never `git add -A`), and
   never `tracker/`: the tracker CLI pushes its own writes to main. On a
   unit branch (the loop's), the last step is to merge the latest main into
   it, settle any conflict, rerun the checks and commit; the loop then merges
   the branch into main (the worker prompt: `just prompt`), running `just check` once more
   first. A unit whose own check takes more than about 5 minutes is tagged
   `slow-check` (`just tracker edit <id> --tags slow-check`), and that step
   skips it.
7. **Stop** after this one unit, interactive or headless. Interactively,
   report in a few lines, in titles: the unit and its new status, questions
   waiting, and what `just next` offers now.

When something needs the owner: check `just decisions <words>` first. If it isn't
answered, `just ask` with options (each saying what it changes), a default and
`--blocks` for what waits on it. That ends this unit, not the run: the loop
moves on to the next workable unit. Don't guess.

## Resources

- **Memory:** run anything heavy (builds, checkers, emulators, the pipeline)
  through `just capped <cmd>`, or a recipe that already uses it. A runaway
  child must die in its own scope, not take the desktop and every other agent
  down.
  A command killed for going over the cap says "killed: out of memory at
  <cap>": that's the cap, not the pipeline, so record it as such.
- **CPU:** a capped or long job is a heavy one, and a machine runs only as
  many at once as its "Heavy" in `docs/machines.md` says (usually 1), across
  its lanes; the rest say "heavy: waiting for the CPU" and start when one
  ends. `just capped --light` / `just long start <name> --light` skip the
  queue for something that barely uses the CPU.
- **Machines:** which machines run the factory, their lanes, accounts and
  what each can do are in `docs/machines.md`, and only there.
- **Shared resources are locks.** Something only one unit may use at a time
  on a machine (an editor, a device, the screen) is a resource in
  `tracker/project.json`, and units that need it say so (`--uses <name>`).
  A resource's `named_by` lists the words (regexes) that mean a unit needs
  it, such as an editor's `["\\bPIE\\b", "just look"]`: a unit naming one in its
  title, detail or probe without `--uses` gets a warning when it's filed and
  from `just lint` (a warning only: it never fails a check).
  A lane holds the resource for its session's life; the machine's other
  lanes take other units meanwhile. This project has none yet.
- **Display, devices, shared state:** headless runs need no display; the build lock
  is shared by every lane: the recipes take it themselves
  (`CARGO_BUILD_JOBS=6 flock /tmp/gbaa-build.lock`, the justfile's `lock`),
  around the compile only. Never wrap `just` in it (a recipe taking it under
  a caller holding it deadlocks); a bare `cargo build/test/clippy` takes it
  the same way; generated or new harness binaries under `systemd-run --user
  --scope -q -p MemoryMax=3G -p MemorySwapMax=0` (6G known-heavy), with
  `$$VAR` for systemd-run's own expansion; before a generated-crate build,
  `free -g` available ≥ 4 + 3 GB (`build-recompiled` checks it). Local
  experiments go in a scratch copy and scratch output folder
  (`docs/factory-units.md`, "Resource rules").
- **GUI:** `--headless` wherever it suffices. A check that needs a real window
  (resize, fullscreen, menus, a pad) runs it in a private headless KWin through
  the `cu` CLI (`cu session`, the linux-computer-use skill), or in a container
  with its own X server; never on june's screen. Anything june must see or
  hear is a gate.
- **Lists every unit adds to clash.** When each unit adds an entry somewhere
  (a registry, an import list, a table), prefer one file per entry, found by
  name (a folder the code scans), over one shared list: two branches adding
  to the same list conflict on merge, two new files never do.
- **Tidy:** something a lane starts and leaves running between units must
  be stopped there, or it runs on after the factory has finished.
- **Long jobs:** anything that may outlast one Bash call runs as
  `just long start <name> -- <cmd>` and is waited on with `just long wait <name>`.
  A job over about an hour means the unit is too big: split it per checkpoint or per feature.
