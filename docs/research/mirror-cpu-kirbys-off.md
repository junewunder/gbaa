# The CPU Kirbys turned off, on Amazing Mirror's lift

Unit native/mirror-cpu-kirbys-off, 2026-11-24. The first mod written into
Kirby & the Amazing Mirror's stage-A lift for a reason of the owner's own
(`docs/ideas.md`, the always-on world: the CPU Kirbys shouldn't wander about
on their own), after the trial walk-speed mod (`mirror-agent-mod.md`). This
page says how it was found, what the edit is, how it was checked, and what
the lift made hard.

## Where the CPU Kirbys come from

katam (the decomp) makes every Kirby in one place. The level setup,
`sub_080332BC(numHumans, localId, abilities, positions, flags)`
(`src/code_080332BC.c:26`, key `0x080332bd`), runs when a game starts:

- a loop `for (curKirbyId = 0; curKirbyId < 4; …)` calls `CreateKirby` for
  `gKirbys[0..4]` (`src/kirby.c:6093`: each gets its own task, `KirbyTask`,
  its sprites and its palette, and `++gNumKirbys`);
- a second loop, `for (otherKirbyId = numHumans; otherKirbyId < 4; …)`, calls
  `sub_0800ECAC` for each Kirby no human plays: it makes the AI controller
  (`code_0800ECAC*.c`) that `KirbyReadInput` reads in place of the pad for
  those Kirbys;
- a last loop over `gNumKirbys` gives each Kirby its `gCurLevelInfo[idx]`
  (each Kirby has its own room state: the world runs up to four rooms) and
  starts it arriving in its room.

Story mode calls it with `numHumans` 1 (`src/code_08123950.c:184` and
`:233`), the link game with the players' count, the demo with 4. Nearly all
the code that walks the Kirbys counts to `gNumKirbys` (109 uses in katam),
not to 4. So the smallest real cut is this one function: make only the
human players' Kirbys, and start no AI.

## The mod

`mods/kirby-mirror/cpu-kirbys-off/` (private, game-derived): one cut,
`sub_080332BC` (220 lines), three changed lines and a constant:

```
const CPU_KIRBYS: bool = false;
…
let v30: bool = if CPU_KIRBYS { v11 <= 3 } else { v11 < arg0 };   // the create loop's test
…
if CPU_KIRBYS && !v57 {                                            // the AI loop's guard
```

`v11 <= 3` is the print of the C's `curKirbyId < 4` (the loop is printed as
a do-while: its first pass is unconditional, which is right here, since
`numHumans` is at least 1); `arg0` is `numHumans`. With the constant `false`,
story mode makes one Kirby, `gNumKirbys` is 1, and no AI starts; with `true`
the function is the lift's own. The demo passes 4 humans, so it is the same
either way.

## How it was checked

`just lift-mod-run out/rom-links/kirby-mirror.gba mods/kirby-mirror/cpu-kirbys-off
units/kirby-mirror-native.toml` (about 4 min 30 s through `just long`), the
route of native/mirror-native-route (reset, title, a new file, the intro,
then right with jumps for 3,000 frames):

- **Constant `false`:** `lift-mod-run: pass (1667 of 3000 frames differ:
  1334-3000)`. Frames 1 to 1333 (the boot, title, file select and intro)
  are stock's. From 1334, the first room fading in, stock shows four Kirbys
  side by side and the mod one, pink; from 2000 to 3000 the only difference
  (263 pixels, 376 at 2111) is the yellow CPU Kirby who follows pink at the
  screen's left edge in stock, and isn't there in the mod. Pink Kirby walks,
  jumps and takes the Waddle Dee's hit where stock's does, the room scrolls
  the same, and the run reaches frame 3000 without a stop.
- **Constant `true`:** `lift-mod-run: pass (no frame differs, of 3000)`.

The per-function check (`fn-check --fn-check-cuts`) that `lift-mod-run` runs
last didn't build either time (hurt 2), so the recipe exits 1 after its
`pass` line.

## What the lift made hard

1. **Nothing much in finding it: the decomp did that.** `CreateKirby` is
   one of katam's few named functions, with one caller, and that caller's
   two loops say what they do (`aiKirbyState`, `otherKirbyId = arg0`). About
   10 minutes from the first grep to the cut. The cut kept the decomp's
   `// src/code_080332BC.c:55` line comments (the function has no rename in
   `names/kirby-amazing-mirror.toml`, so the walk-speed mod's hurt 3 didn't
   bite), and the loops were easy to match to the C by them.
2. **fn-check still can't build a cut that calls things.** This function
   makes seven calls through `x_` entries whose lift returns fewer registers
   than fn-check's adapter gives it (`expected u32, found (u32, u32, u32)`),
   the walk-speed mod's hurt 2 seven times over. Open as
   native/cut-callee-shapes (noted there).
3. **A constant at the top of the cut file isn't seen by fn-check.** The
   mod's own build (`--replace`) takes the whole file, so a file-level
   `const CPU_KIRBYS` built and ran; but fn-check's copy of a cut
   (`codegen::native::cut::Replacement::of`) takes only the function, from
   its `pub fn` line to its closing `}`, and the constant was "not found in
   this scope". The constant now lives at the top of the function body, which
   both builds see. A modder's natural place for a switch is the top of the
   file. Fix: `Replacement::of` carries the file's other items (`const`,
   `static`, helper `fn`s) along. Noted on native/cut-callee-shapes, whose
   code it is.
4. **Comparing a mod with its own switch flipped means copying the mod.**
   `lift-mod-run` compares stock with the mod as it is on disk, and writes
   over `out/lift-mod-run/<route>/` each run. To show that the constant
   `true` gives stock's frames I copied the mod to `/tmp` with the constant
   flipped and ran the recipe again (another stock lift and build, 4 min
   30 s), after copying the first run's PNGs aside. Fix: `lift-mod-run`
   takes a second mod (or `--set NAME=VALUE`) as the reference instead of
   stock, and writes under the mod's name. Filed: native/lift-mod-run-ab.
5. **The loop's bound prints as `v11 <= 3`.** The C's `< 4` became `<= 3`
   on a fresh `u8` value; changing it to a variable bound meant reading the
   loop as a do-while and checking that its first pass is safe. A `for` print
   (native/lift-for-bounds has landed for loops it recognises) would have
   made the edit `0..arg0`. Small.

## What the one cut doesn't do

It turns the CPU Kirbys off where they are made, nothing else. `gKirbys[1..3]`
stay as they were, and code that loops to 4 rather than `gNumKirbys` reads
them (24 such loops in katam's `cutscene_trigger.c`). The phone, which calls
the CPU Kirbys to the player (its ring is in `code_0806F780.c`), wasn't
read, and likely expects them to exist. The route reaches none of these, so
whether they misbehave with one Kirby is unknown: a longer route through a
door, a cutscene and a phone call is the next check before the always-on
world uses this mod.
