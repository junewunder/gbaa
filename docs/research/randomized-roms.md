# Randomized ROMs: what a randomizer changes in Emerald's code

The question (from `docs/research/lua-scripting.md`): Ironmon starts every
new run by loading a freshly randomized copy of the game, and our
recompiled Emerald refuses any ROM but its own. How much of a randomized
Emerald would the compiled code still be right for, and what would each
way of supporting it cost?

Short answer: the randomizer Ironmon uses changes **20 of our 15,158
compiled Emerald functions** with the Kaizo settings, an upper bound,
measured from the randomizer's source and our generated code. Everything
else it changes is data the game reads at run time. A per-function check
at load would keep the rest compiled.

## Which randomizer, which settings

- **The randomizer.** The Ironmon tracker (v9.4.0) makes each new run by
  running the Universal Pokemon Randomizer ZX (UPR ZX, Java, latest
  v4.6.1, 2024-11) from the command line:
  `java -Xmx4608M -jar PokeRandoZX.jar cli -s SETTINGS.rnqs -i ROM -o NEW.gba -l`
  (`ironmon_tracker/Main.lua:679`), then `emu:loadFile`s the result. That
  is its "Generate" mode; its "Premade" mode loads ROMs the player
  randomized beforehand, with whatever tool, in practice the same one. A
  fork, `something-smart/ironmon-randomizer`, is UPR ZX with Ironmon
  presets; its Gen 3 code is the same handler.
- **The settings.** A `.rnqs` file is UPR's settings string with a
  header: a big-endian version (321 = 4.6.0), a length, then the base64
  string (`Settings.java:321-350`). The Emerald Kaizo Ironmon string
  (posted by the community, 2023) decodes, at the bytes that matter here,
  to: random starters with random held items, completely random static
  encounters, random movesets, trainers, wild Pokemon, abilities, TMs,
  evolutions, and the misc tweaks *fastest text*, *running shoes
  indoors*, *run without running shoes*, *random PC potion*, *national
  dex at start* and *ban Lucky Egg* (`0x410b8`). Kaizo Ironmon Plus
  (`Corvimae/kaizo-ironmon-plus`) is another Emerald (U) settings file
  of the same version.

## Code or data

Read from UPR ZX v4.6.1's source (`Gen3RomHandler.java`,
`config/gen3_offsets.ini` `[Emerald (U)]`, the IPS files under
`pkrandom/patches/`): almost everything is **data** (base stats,
learnsets, evolutions, trainers and their parties, wild tables, TM lists,
event scripts holding static encounters, the starter table). The game
reads that through pointers at run time, so compiled code reads the new
values with no change.

Some is **code**, small and in fixed places:

| Change | Where | When (Kaizo turns all on) |
|---|---|---|
| Starter's held item | immediates in `CB2_GiveStarter` | random starter items |
| Intro Pokemon | species constants in two Birch-speech functions | always |
| Latios/Latias roamer | species, level and three rewritten instructions in `CreateInitialRoamerMon` | every Emerald run |
| Deoxys and Mew obey | compares in `IsBattlerModernFatefulEncounter` | random starters, wild or statics |
| First battle (Zigzagoon) | a hook in `SetUpBattleVarsAndBirchZigzagoon`, and a new routine at `0x08FE0000` | every Emerald run |
| Instant text | a hook in `RunTextPrinters`, and new Thumb and ARM code at `0x08A00000` | fastest text |
| Running shoes | a byte in `IsRunningDisallowed`, a branch in `PlayerNotOnBikeMoving` | the two running tweaks |

The new routines sit in empty ROM, where nothing was compiled; the hooks
jump there, so they would run in the interpreter.

## Measured: functions a Kaizo run makes stale

`docs/research/randomized-roms/probe.rs` reads a generated crate (each
compiled function's instruction addresses, every ROM load whose value
const-prop baked into the code, and every direct call) and counts the
functions whose bytes a set of changes touches. The changes are
`randomized-roms/kaizo-emerald.txt`: every code write above, and every
data table the Kaizo settings rewrite that compiled code could read at a
fixed address, **whole** (a real run changes only parts of each), so the
count is an upper bound. Left out: TM and tutor text and the Mossdeep
Steven team, which no compiled function bakes in (`--baked`). Run on
`out/pokemon-emerald-version` (generated 2026-10-22):

- 15,158 compiled functions, 1.63 MB of instructions, 58,758 baked ROM
  loads.
- **20 functions stale**: 10 because their instructions changed, 12
  because a value they baked in did (2 both). Of the 20:
  - 11 are the code changes above (`RunTextPrinters`, `CB2_GiveStarter`,
    `CreateInitialRoamerMon`, …), plus `GetEggMoves`, which bakes in the
    first word of the egg-move table, and `NewGameInitPCItems`, which
    bakes in the PC potion.
  - 5 bake in a constant from `gWildMonHeaders` or `gTrainers` (the
    Dome Brain's class and picture). UPR rewrites the encounter slots and
    trainer names, not those fields, so these are probably false alarms.
  - 4 aren't real functions: discovery found "code" inside
    `gSpeciesInfo` and the party-menu palette tables. They never run.
- Baked constants come from 348 data symbols in all; apart from the ones
  above, none is a table UPR changes (the list: `--baked`).
- **65 unchanged functions call a stale one directly.** Calls between
  compiled functions are direct Rust calls (decision 8), so those
  callers would still run the stale compiled body. Counting every caller
  as stale too, transitively, makes it 155, among them `CB2_Overworld`,
  `BattleMainCB2` and `MainCB`: the main loop's callbacks.

Not measured: a real randomized ROM. Running the randomizer's jar here
was refused (running downloaded code needs the owner's OK). The probe
takes one directly once it can be made: `--base base.gba --other
randomized.gba` counts the bytes that differ and the functions they hit.

## What each option costs

**a) Per-function check at load, interpreter fallback** (keeps compiled
code where the randomizer left it alone).

- Codegen emits, per function, a hash of its instruction bytes and its
  baked loads' bytes (15,158 entries, ~120 KB with the ranges). At load
  the runtime hashes those 1.7 MB of the new ROM: a few milliseconds.
- A stale function must stop running compiled. The dispatcher drops its
  keys (`Code::blocks`), and, because of direct calls, either:
  - each compiled function starts with a check of a stale bit and hands
    over to the interpreter if set: one load and branch per call, paid
    by every build, also for the exact ROM (unmeasured; likely ~1%), or
  - every direct caller is treated as stale too: no cost on the exact
    ROM, but 155 functions interpreted, the main callbacks among them
    (they mostly call others, which stay compiled).
- New code in empty ROM runs in the interpreter, as any unknown address
  does now. `RunTextPrinters` runs every frame, so the instant-text hook
  and its routine are interpreted every frame: some hundreds of
  instructions, a small part of a frame.
- Whole-ROM identity moves to "same game, per-function check": save
  states, rewind and netplay compare the ROM hash today and must compare
  the randomized ROM's own hash instead.
- Work: two to three units (codegen's table and the entry check, the
  runtime's stale set, the desktop's loader and its tests), plus a check
  that runs a randomized ROM against mGBA. It needs a randomized ROM to
  test with, so it needs the randomizer run.
- Speed: the compiled ~1270 frames/s on quack, less the interpreted
  share; on the Deck the same.

**b) Fully interpreted** for any ROM but the exact one.

- The desktop already runs other ROMs in the interpreter
  (`gbaa-desktop`); a recompiled build would load the new ROM into the
  same path instead of refusing it. Work: about one unit.
- Speed: ~220 frames/s on quack, 3.7× real time, ~5.8× slower than
  compiled (`docs/research/lua-scripting.md`). The Deck's cores are about
  2–2.5× slower (`docs/research/multiplayer.md`): roughly 90–110
  frames/s, 1.5–1.8× real time, unmeasured. Fine on a PC, thin on the
  Deck, and the whole of a run is played that way.
- Exact by construction: the interpreter is what the compiled code is
  checked against.

**c) No support.** Nothing to build. Ironmon's new-run button doesn't
work in a recompiled build, and neither does tracking a run randomized
elsewhere, since every run's ROM differs from the one we compiled: only
`gbaa-desktop`'s interpreter, or mGBA, would run it.

ROM hacks built on Emerald (Rogue, Imperium, Kaizo hacks rebuilt from
source) change code everywhere; they need their own recompile or the
interpreter under every option.

## Sources

- UPR ZX: <https://github.com/Ajarmar/universal-pokemon-randomizer-zx>,
  tag v4.6.1: `src/com/dabomstew/pkrandom/romhandlers/Gen3RomHandler.java`,
  `config/gen3_offsets.ini`, `patches/instant_text/em_instant_text.ips`,
  `patches/hardcoded_statics/em_firstbattle.ips`,
  `asm/hardcoded_statics/em_firstbattle.asm`, `Settings.java`.
- Ironmon tracker v9.4.0: <https://github.com/besteon/Ironmon-Tracker>,
  `ironmon_tracker/Main.lua` (the jar's command line).
- Emerald Kaizo Ironmon settings string: <https://pastebin.com/z3Q8hVa9>
  (2023; decoded with `Settings.fromString`'s layout).
- Kaizo Ironmon Plus settings: <https://github.com/Corvimae/kaizo-ironmon-plus>.
- Symbols: `$GBAA_DATA/symbols/pokeemerald.sym`.
