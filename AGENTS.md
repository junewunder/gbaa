# gbaa (GBA Advanced)

Static recompiler: GBA ROMs (ARM7TDMI) to Rust, plus a runtime crate.

Visibility: public (github.com/junewunder/gbaa). History before 2026-11-01
is in the private github.com/junewunder/gbaa-private (remote `private`,
branch `private-main` in june's checkout); the public root commit is
those files with private details taken out, not an exact copy. A branch begun before the switch moves over
with `git rebase --onto main private-main <branch>`; a pre-push hook refuses
to push the old history to the public repo.

## Developed in public

Everything tracked is public, and june has OK'd pushing without asking
(the usual ask-first rule for public repos doesn't apply here). So nothing
tracked may hold:

- Game-derived code: translated functions, mod copies, hand ports,
  snapshots of output on a real game. It goes in `mods/<game>/`, which is
  gitignored and its own private repo (lane clones link it in through
  `factory/lane-setup`). Short quotes of a few instructions in a doc are
  fine; whole functions aren't.
- Machine facts: hostnames' tailnet domain, IPs, accounts, the NAS, where
  ROMs come from. They go in `docs/machines.private.md` (gitignored).
- ROMs, saves, or anything taken out of a ROM.

`crates/leak-check` fails `just check` on the private strings it knows
(`$GBAA_DATA/leak-words.txt`, not in git). It only knows what it's told:
when a new private name appears, add it there.

Design lives in `docs/`: `decisions.md` (settled), `open-decisions.md`
(still open), `risks.md`, `references.md`, `ideas.md` (wishes not yet planned). Never commit ROMs or test ROM
binaries; link them from outside the repo.

## Layout and commands

Crates live in `crates/`: `armv4t`, `analysis`, `codegen`, `runtime`
(`#![no_std]`), `desktop`, `native-spike` (the first native port, by
hand), `native-host` (the native programs' headless front end), `interp` (the headless `gbaa-interp ROM
[--frames N] [--test]` runner), `hole-report` (`gbaa-holes`, what blocks a native
build), and the `disc-check`, `jt-check`, `aw-check` and `mgba-dump` tools; `gbaa-dis` (`just dis`) disassembles ROM code.
`codegen`'s `gbaa-lift` is the stage-A lifter (`just lift`, `just
lift-check`; `just lift` ends by running the build and recording where it stops, `just first-stop`); `holes/<game>.toml` are its stand-ins (a commercial game's
are game-derived: `mods/<game>/holes.toml`). Generated code goes in
`out/<game>/` (gitignored).

- `just check`: fmt, clippy `-D warnings`, tests, `check-nostd` and `rebuildable`. The
  merge gate.
- `just check-nostd`: builds `runtime` for `riscv32imac-unknown-none-elf` to
  prove it stays no_std.
- The build lock (`CARGO_BUILD_JOBS=6 scripts/build-lock`, the justfile's
  `lock`: three builds at once) is taken by each recipe around its cargo step. Never
  wrap `just` in it (that deadlocks); a bare `cargo build/test/clippy` takes
  it: `env CARGO_BUILD_JOBS=6 scripts/build-lock cargo …`.
- `just fetch-tests`: shallow-clones the test suites, pinned by commit in
  `scripts/fetch-tests.sh`, into `$GBAA_DATA` (default
  `~/.local/share/gbaa/`): `SingleStepTests-ARM7TDMI/`, `gba-tests/`
  (jsmolka), `armwrestler/` (destoer's GBA build, original and fixed ROMs).
- Everything else (the scored checks `sst`, `sst-ir`, `gba-tests`,
  `armwrestler`, `disc-*`, `jt-score`; agb builds; `gbaa-desktop`; codegen,
  `recompile*` and `first-frame`; sound and `audio-*`; `mgba-diff` and
  `mgba-oracle`) is in `docs/tools/commands.md`. Read the entry before
  running or changing one.
- Comparing across games or programs: `just holes-all`, `just lift-metrics`.

## Factory

- `docs/factory.md` is the factory spec: the unit, the oracle, when to stop,
  and the loop. "Continue" means work one unit by that loop; "oversee" means run
  the lanes detached on the machines the owner includes (`just fleet`) and
  supervise them (`just oversee-guide`). Machine facts: `docs/machines.md`.
- `tracker/` is the state, changed only through the tracker CLI: `just status`,
  `just next --claim`, `just done`, `just ask`, `just lint` (`just tracker --help`).
- Talk to the owner in titles, never bare IDs: what a unit is first, the ID after in
  parentheses.
