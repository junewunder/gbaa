# A first Amazing Mirror mod on the lift, and what hurt

Unit native/mirror-agent-mod, 2026-11-24. The Metroid trial
(`lift-agent-mod.md`) again, on the game the owner wants to mod: an agent
new to the lifter made one small visible change to Kirby & the Amazing
Mirror on the stage-A lift, through `gbaa-lift --cut`, an edit of the print
and `just lift-mod-run` on Amazing Mirror's native route
(`units/kirby-mirror-native.toml`). This page says what was hard, ranked,
each with the fix it suggests and the unit filed for it, and what differed
from Metroid.

## The mod

`mods/kirby-mirror/walk-speed/` (private, game-derived): one function,
katam's `sub_0805B2FC` (`src/kirby.c:14460`, key `0x0805b2fd`), which the
names file `names/kirby-amazing-mirror.toml` calls `KirbyGroundAccel`. Each
frame Kirby walks it adds 0x20 to `kirby->base.xspeed` up to a top speed
(1/256 pixel a frame): 0x14C walking, 0x1AC in water, 0x1DC and 0x24C
dashing. One edit, one line: the walking top speed, printed in the cut as

```
r3 = 0x14cu32;
```

is now `0x298u32`, double. Steps, about 13 minutes of wall time from the
first grep to the PNGs (01:13 to 01:26), plus reading:

```
# 1. find the value (about 7 min)
just lift-find ROM xspeed --name kirby-mirror            # 2 min 20 s: 1139 writers (hurt 1)
gbaa-desktop --headless --frames 1702 --input '…route…' --script watch.lua ROM
                                                          # 8 s: a write watch on gKirbys[0].base.xspeed
#    -> pc 0x0805B382, xspeed 160, 192, … to 332 and held: the cap is 332 = 0x14C
#    -> the function holding 0x0805B382: katam.toml's sizes, sub_0805B2FC; kirby.c:14460

# 2. cut (64 s, a whole lift into OUT)
gbaa-lift ROM out/lift-kirby-mirror-cut --name kirby-mirror \
    --holes mods/kirby-mirror/holes.toml --cut mods/kirby-mirror/walk-speed 0x0805b2fd
#    (`--cut … sub_0805B2FC` failed: "names no function"; hurt 3)

# 3. edit src/KirbyGroundAccel.rs (0x14cu32 -> 0x298u32)

# 4. see it (4 min 30 s through `just long`)
ln -s ROM out/rom-links/kirby-mirror.gba                  # as Metroid's: hurt 4
just lift-mod-run out/rom-links/kirby-mirror.gba mods/kirby-mirror/walk-speed units/kirby-mirror-native.toml
```

The watch script is kept beside the mod (`find-writer.lua`).
`lift-mod-run` printed `1337 of 3000 frames differ: 1664-3000` and 16
side PNGs in `out/lift-mod-run/kirby-mirror-native/`. Frames 1 to 1663
(reset, title, file, the intro, Kirby standing and his first steps right
from 1650, below the old cap) are the same. From 1664 (180 pixels: only
Kirby, a few pixels further right) he is ahead; by 1753 he has run into the
Waddle Dee the stock run hasn't reached ("OUCH!", the enemy's name bar), so
the camera and most of the screen differ (22,852 pixels); from 2554 the
screens agree again but for 36 pixels, his health bar, one segment down
from the hit.

## What hurt, ranked

1. **`lift-find` can't say which writer matters.** Kirby's speed is
   `gKirbys[].base.xspeed`, which every state, enemy, boss and door writes:
   `just lift-find ROM xspeed` listed 1139 writers and 547 readers (2,484
   lines), the walk's among them by name but nothing saying it is the walk.
   Metroid's `maxEnergy` had three writers. Grepping katam for `xspeed`
   wasn't better (hundreds in `kirby.c`, mostly `sub_` functions). What
   found it in 8 s was dynamic: a Lua write watchpoint on the field while
   Kirby walks on the route, which gave the pc and the values (the cap 332
   is plain in them). **Fix:** `lift-find` with a route and frames, listing
   only the functions that wrote the field on those frames, with the values
   written. Filed: native/lift-find-on-route.
2. **`lift-mod-run` fails at its last step on an unedited line.** After
   `lift-mod-run: pass` its fn-check of the cut didn't compile: the cut
   prints `r3 = KirbyGroundFriction(g, r13, v13);` (in the lift that
   callee returns only r3), but fn-check's lift reads all of the cut's r0-r3
   through its adapter, so the callee there returns `(u32, u32, u32)`, and
   `error[E0308]: mismatched types` ends the recipe with exit 1. The frames
   and PNGs were already written, so the mod was seen, but the per-function
   check never ran. **Fix:** a cut's call sites take whatever shape the
   build gives its callee, or fn-check keeps the lift's shapes. Filed:
   native/cut-callee-shapes.
3. **The names disagree, and the source lines vanish.** The decomp and
   `lift-find`'s neighbours call the function `sub_0805B2FC`; the lift calls
   it `KirbyGroundAccel` (from `names/kirby-amazing-mirror.toml`, a
   research rename) and `--cut DIR sub_0805B2FC` answers "names no
   function", so I cut by key. Worse, the rename dropped the decomp's line
   comments: the cut has no `// src/kirby.c:N` at all, and `lift-find`
   prints addresses (`0x0805b356`) for it where `sub_0805B3A0`, next door,
   prints `src/kirby.c:14508`. Metroid's write-up named those comments as
   what made its edit easy. Here I matched `r3 = 0x14cu32` to the C's
   `0x14C` by eye. **Fix:** a rename keeps the decomp entry's file and lines,
   and `--cut`/`lift-find` take either name (the names file has `katam =`).
   Filed: native/names-keep-source-lines.
4. **`lift-mod-run` can't be given the game's name** (Metroid's hurt 1,
   still open as native/lift-mod-run-name): the ROM is
   `Kirby & The Amazing Mirror.gba`, so the recipe looks for
   `mods/kirby & the amazing mirror/holes.toml`. Same symlink workaround;
   noted on that unit.
5. **Every flag test reads through a rotate.** Each field load prints as
   `prim::shift(ShiftType::Ror, v19, (v14.wrapping_add(0x8u32) & 0x3u32) << 3, false).0 & 0x10u32`
   for the C's `kirby->base.flags & 0x10`: the rotate of an unaligned load,
   always 0 for this struct. Reading which branch was "walking, not in
   water" took a pass with the decomp open. **Fix:** drop the rotate where
   the lift proves the address aligned; already filed as
   native/lift-aligned-loads (noted with this case).
6. **`lift-mod-run` says what changed on screen, not in the game.** 1337
   frames differ, and from 1753 the red covers the whole picture, because
   a faster Kirby reaches an enemy sooner; the PNGs show consequences. That
   the cause was his speed (xspeed 352 against 332 from frame 1661) I knew
   only from my watch. **Fix:** with the frames, name the first RAM field
   to differ (`gKirbys[0].base.xspeed at frame 1661: 332 stock, 352 mod`).
   Not filed separately: noted here for native/lift-mod-run-edit-report,
   which names the cut's edit by field.

Small ones, not filed: `--cut` writes a whole lift into OUT for one cut
file (Metroid's 7, still so); the value is in 1/256 pixel a frame and
nothing says so (the decomp doesn't either).

## What differs from Metroid

- **Finding the place was the work.** Metroid's value had a named field
  and a named function (`SamusInit`, three writers of `maxEnergy`); a grep
  of the decomp found it. Amazing Mirror's decomp is mostly `sub_` names,
  the field has a thousand writers, and the value is a cap in code, not a
  start value. Static search didn't narrow it; a watch on the running game
  did. A modder of this game will need the dynamic route most of the time.
- **The struct printed by field.** Metroid's `gEquipment` printed as `u32`
  words with masks (its hurt 2); Kirby's prints as
  `g.get(Ref::<t::Kirby>::new(v14).base().flags())`, so which field each
  load is was clear at once. The cut's parameter is even named `kirby`.
- **The cut header explained everything I met** (`prim::sext`, `pc`,
  `bank_sp`; Metroid's hurt 6 is done).
- **The build broke where Metroid's only said FAIL.** Metroid's fn-check
  ran and reported the edit as a difference; here it didn't build (hurt 2),
  because this cut calls a function and Metroid's didn't.
- **The change is big on screen.** No zoom was needed (Metroid's hurt 5);
  the opposite problem, a change whose effects spread over the screen
  (hurt 6).

## What went well

The cut was right first time and built in the mod's lift with no warnings;
the edit was one constant beside three others with the same shape, and the
lift's choice between them mirrored the C's two ternaries closely. The
names file's research comment on `KirbyGroundAccel` ("his top speed (0x14c,
0x1ac in water, 0x1dc and 0x24c running)") confirmed the constant once I
had the function. `lift-mod-run` was one command, under 5 minutes, and
its first differing frame, 1664, sat a few frames after Kirby's speed first
passed the old cap: the stock frames before it are unchanged.
