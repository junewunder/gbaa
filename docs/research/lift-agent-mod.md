# A first Metroid mod on the lift, and what hurt

Unit native/lift-agent-mod, 2026-11-20. An agent new to the lifter did
what a modder would, on the stage-A lift rather than the mod view: picked a
small visible change to Metroid: Zero Mission, cut the function it lives in
(`gbaa-lift --cut`), edited the print and looked at it beside stock with
`just lift-mod-run`. This page says what was hard, ranked, each with the
fix it suggests, and the unit filed for it. It is the lift's version of `agent-first-mod.md` (the
Emerald mod on the mod view).

## The mod

`mods/metroid/energy/` (private, game-derived): one function, `SamusInit`
(`src/samus.c`, key `0x0800bcb9`), which a new game runs to clear Samus's
data and set her starting energy. One edit, two lines: `samus.c:8087` and
`:8088`, `gEquipment.maxEnergy = 99` and `gEquipment.currentEnergy = 99`,
now 45. In the cut the two lines print as halfword stores into the word
array, the value as hex:

```
g.ram.gEquipment[0] = g.ram.gEquipment[0] & 0xffff0000 | ((0x63u32 & 0xffff) << 0) as u32;
```

The edit is `0x63u32` to `45u32` in both.

Steps, about 10 minutes of wall time from the first grep to the PNGs
(07:23 to 07:31), plus reading:

```
# 1. find the value (about 3 min): grep the decomp, then the existing lift
grep -rn --include=*.c "maxEnergy = " $GBAA_DATA/src/mzm/src     # SamusInit, samus.c:8087
grep -n "SamusInit" out/lift-metroid/src/*.rs                     # an older lift, already there

# 2. cut (50 s, 230 MB; a whole lift into OUT)
target/release/gbaa-lift "$ROM" out/lift-metroid-cut --name metroid \
    --holes mods/metroid/holes.toml --cut mods/metroid/energy SamusInit

# 3. edit mods/metroid/energy/src/SamusInit.rs (0x63u32 -> 45u32, twice)

# 4. see it (4 min 58 s through `just long`; three generated crates)
ln -s "$ROM" out/rom-links/metroid.gba          # see "What hurt", 1
just lift-mod-run out/rom-links/metroid.gba mods/metroid/energy units/metroid-native.toml
```

`lift-mod-run` printed `895 of 2700 frames differ: 1806-2700` and 16
side-by-side PNGs in `out/lift-mod-run/metroid-native/`
(`side-1806.png` … `side-2700.png`). Frames 1 to 1805 (reset, title, file
select, the opening text) are the same; from 1806, the first frame in
Brinstar with the HUD up, every frame differs, and only in the HUD's energy
digits: stock `99`, the mod `45` (10 pixels at first, 11 to 13 from
`side-2282.png` on as the room scrolls under the HUD; the red in the third
panel is only over the digits). The energy tanks row is the same, since
45 like 99 is under 100.

## What hurt, ranked

1. **`lift-mod-run` can't be given the game's name.** It takes the name
   from the ROM's file name, and the Metroid ROM's is `Metroid - Zero
   Mission (USA).gba`, so the name is `metroid - zero mission (usa)`: it
   finds no `mods/<name>/holes.toml`, and the lift fails with `11 hard
   holes; the crate won't build` (checked by running its stock lift step
   alone, 54 s), under a crate name with spaces in it. `just lift` has
   `--name` for exactly this; `lift-mod-run` passes its extra flags to
   route-check, and `just fn-check`, which it calls, has the same lookup.
   I worked around it with a symlink `out/rom-links/metroid.gba`. **Fix:**
   `lift-mod-run` takes `--name NAME` (and passes it on to fn-check), or
   better, reads the name from the cut: `replace.toml`'s `[mod]` already
   holds the ROM's sha1, and the mod sits in `mods/<name>/`, so the recipe
   can take the name from DIR's parent and the holes file with it.
2. **A struct field prints as a word of an array, its value as hex.** The
   decomp line is `gEquipment.maxEnergy = 99;`; the print is the line
   quoted above: `gEquipment[0]`, a 0xffff0000 mask, `0x63`, `<< 0`, and
   `:8088`'s `currentEnergy` is `gEquipment[1]` with `<< 16`. The names map
   has `[types."struct Equipment"]` (`maxEnergy` at offset 0, u16) and
   types `gEquipment` with it, and the mod view's `mod-find` resolves the
   field by name, so the lift has what it needs but prints RAM as `u32`
   words. To be sure I'd edit the right value I had to check that word 0's
   low half is offset 0 and word 1's high half offset 6. **Fix:** where a
   global has a struct type, give the lift's `Ram` that layout (or an
   accessor per field) so the store prints
   `g.ram.gEquipment.maxEnergy = 99;`, constants in decimal when they are
   stored into a named field (the mod view's readable lowering already
   does both).
3. **Nothing on the lift's path finds the function.** The obvious data,
   `sStartingHealthAmmo.energy`, is the wrong place: a new game's 99 is a
   literal in `SamusInit`, and only grepping the decomp for `maxEnergy =`
   showed it (`sStartingHealthAmmo` is read by the status screen and the
   tank pickup). The mod view's `just mod-find ROM "" maxEnergy` answers
   it in 15 s (three writers, `SamusInit` first, with `samus.c:8087`), but
   neither the `--cut` entry in `docs/tools/commands.md` nor `lift-mod-run`'s
   points to it, and its names are the mod view's, not checked against the
   lift's. **Fix:** say in the `--cut` entry "find the function with `just
   mod-find`"; or a `gbaa-lift --find FIELD` that lists the lift's writers
   and readers of a field the same way.
4. **lift-mod-run says `fn-check: FAIL` for the edit it was asked to show.**
   Its last lines: `SamusInit … 8 DIFFER`, `IWRAM byte 0x03001530: 0x63
   reference, 0x2d replacement`, `fn-check: FAIL`, after `lift-mod-run:
   pass`. The difference is the mod, as the recipe's doc says, but the
   word FAIL and `the program stopped` read as a broken build. The
   stop is fn-check's own program, not the mod's: it ran 2,997 of 18,000
   frames with no input and panicked on `a call or jump to 0x03001945,
   which is no function's entry` (pc 0x0805d601), after the check of
   SamusInit had finished. **Fix:** under `lift-mod-run`, print the cut's
   result as "differs from its lift (the edit): IWRAM byte 0x03001530
   (gEquipment.maxEnergy) 99 -> 45", naming the byte by field and in
   decimal, and say the stop apart from the check; the stop itself is a
   lifter finding for a unit of its own.
5. **The side PNGs show a small change small.** Each is the frame at 1x,
   three across (728x160): ten pixels of HUD digits in one corner. To read
   the digits I cropped and scaled the HUD by hand (ImageMagick).
   **Fix:** add the differing pixels' bounding box to each line (`10
   pixels differ in x 8-23, y 0-7`) and a fourth panel or a second PNG
   with that box scaled 4x for both builds.
6. **The cut's header doesn't name everything in the body.** It explains
   `lN`, `vN`, `pc(…)`, `bank_sp`, `r13`, but not `pin(addr, off)` (on
   both lines I edited), `ix`/`rix` or `prim::sext`. Constants alone didn't
   need them, but I couldn't tell whether `pin` had to change with the
   value. **Fix:** one header line for each helper an edit meets
   (`pin`: which RAM the access is held to, leave it; `ix`/`rix`: a
   checked index into a global or a ROM table).
7. **Small ones.** `--cut` writes a whole lift into OUT (2,600 functions,
   for one cut file); a cut-only mode that writes nothing else would save
   the directory. The existing `out/lift-metroid` I grepped was from an
   older commit and returned a 4-tuple where the cut returns a 3-tuple,
   harmless here but confusing if I'd copied from it. `lift-mod-run`'s log
   repeats cargo's `unused field license` warning for each crate it
   builds, burying the result lines.

## Filed

1. native/lift-mod-run-name; 2. native/lift-equipment-words (the lift's
`Ram` holds `gEquipment: [u32; 5]` though the names map types it `struct
Equipment`, which has no bitfield); 3. native/lift-find-field (the mod view
is being retired, so a lift-side find); 4. native/lift-mod-run-edit-report,
and the stop as native/metroid-fn-check-stop; 5. native/lift-mod-run-zoom;
6. native/lift-cut-header-helpers. The small ones (7) are not filed.

## What went well

The `// src/samus.c:8087` line comments made the decomp line findable in
the print at once; the cut was right first time and its header said what
to keep. Once past the name, `lift-mod-run` was one command, under 5
minutes for three crates, and its summary (`895 of 2700 frames differ:
1806-2700`) told me where to look without opening a PNG; the third panel's
red sat exactly on the digits, so it was plain nothing else changed. The
modded crate built with no warnings from the edit, and the change showed on
the first run.
