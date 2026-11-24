# Ideas, not planned

Things june wants someday that aren't tracker units yet, mostly because they
need a game's decomp worked far enough to change the game, or are mods
rather than gbaa features. When one becomes doable, file a unit and take it
off this list.

## Kirby & the Amazing Mirror, on its decomp

From june's Kirby netplay playtest with Athena (2026-10-29). The decomp is
katam (jiangzhengwenjz/katam, pinned in `scripts/fetch-decomps.sh`): it
rebuilds june's ROM byte-identical here, it's mostly C (about 177k lines of
C to 61k of assembly), and its names are mostly placeholders
(`sub_XXXXXXXX`). `docs/research/kirby-controller.md` has what's known.

- **A real multiplayer mod.** Netcode in the game itself, so each player's
  machine needn't run and roll back every player's whole console. june:
  rollback works, but this game ultimately wants a real decomp with a real
  multiplayer mod.
- **An always-on world you drop into.** A server that keeps a game going
  with nobody in it, where players join and leave as they like, and the CPU
  Kirbys are turned off so they aren't wandering about on their own.
- **Typing the room code in the game**, instead of in gbaa's menu.
- **Pausing for one player only.** The game's own pause stops every
  player, and only the one who paused can unpause. A per-game mod could
  make it pause just yours.
- **Kirby in Rivals of Aether 2.** A Workshop character built from Kirby's
  controller code and sprites (Fighter Kirby, say), through rivals-forge.
  The probe in `docs/research/kirby-controller.md` found the code and
  constants and exports the sprites.
- **A Master Hand boss fight in Rivals 2.**

## Other games

- **Squeak Squad with multiplayer** (Athena's wish). That's a DS game with
  no multiplayer built in, so it's far outside what gbaa does today.

## Native lifts

- **Prove small rewritten functions equal to the lift.** The per-function
  check (`native/fn-check-any`) only tests a rewritten function on inputs
  against the machine-shaped lift. For small leaf functions with no loops,
  or loops with a small bound, a solver could prove the two equal on every
  input: both sides as formulas over registers and memory, an SMT query for
  any input where they differ. Bounded, and only for leaves to start, but
  it would turn the LLM pilot's "passed its tests" into "proved" for the
  functions it covers. june (2026-11-13): park it until the pilot shows how
  often tests miss.
- **A rewrite loop that grows the printer.** By hand, apart from the
  factory: pick about 20 lifted functions of different shapes, have an
  agent rewrite each toward the decomp's C (still Rust against the lift's
  own types), check each rewrite against its lift (`just fn-check`), then
  sort the edits by kind. A kind that recurs becomes a unit for a printing
  transform; one-offs are dropped. It is for the gaps with no known
  printed form yet (stack arrays, locals that are only a comment, calls
  returning `(r0, r2, r3)`). The score is the pret table
  (`native/lift-pret-score`). It needs a lift that builds: Metroid now,
  Emerald after `native/emerald-builds`. june (2026-11-20).
