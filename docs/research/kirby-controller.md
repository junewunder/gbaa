# Kirby's controller and assets in Amazing Mirror

research/kirby-controller-probe, 2026-10-28. june's pie-in-the-sky question: could Kirby & the
Amazing Mirror's Kirby become a Rivals of Aether 2 Workshop character (rivals-forge, through the
official RoA1 2D-character import)? This probe asks only whether an agent can find Kirby's
controller code (the per-frame update, the state machine, and the slide, air puff, jump and float
with their physics constants) and extract Kirby's assets (sprite frames per move, palettes,
hitboxes). Building the character is rivals-forge's job.

**Answer: yes for the code and the constants, mostly for the assets.** About 40 minutes of agent
time found the per-frame task, 16 states with their entry functions, every walking, running,
jumping, floating, sliding and puffing constant, and both kinds of hitbox. The asset pipeline ran
unchanged on Kirby and exports the palettes and the frames Kirby draws as one sprite (float,
inhale) cleanly. Frames drawn from several sprites of mixed sizes (walk, jump, fall) come out of
that recording as loose tiles, so a follow-up (research/kirby-sprite-tables) reads the game's own
sprite tables instead: every one of plain Kirby's animations now comes out whole ("Sprite
tables", below).

ROM: `Kirby & The Amazing Mirror.gba` (USA), sha1 `274b102b6d940f46861a92b4e65f89a51815c12c`, the
same sha1 the katam decomp builds.

## Rebuilding

`just kirby-assets` (about 25 s once built) writes everything to `out/assets/kirby/` (gitignored):

- `moves/trace.txt`: Kirby's state every frame while `scripts/kirby/moves.lua` presses the moves
  (the line format is in the script's header), plus the provenance build's `loads.tsv`,
  `uses.tsv`, `oam.tsv` and ten frame PNGs.
- `tiles/`, `palettes/`, `sprites/` (Aseprite), `sprites.txt` and `manifest.toml`: `gbaa-assets
  graphics` on that recording, the same exporter Emerald uses.
- `sprite-tables/`: `gbaa-assets sprite-tables`, read from the ROM alone ("Sprite tables",
  below). `anim_<id>[_<name>].aseprite` and `.png` for the 46 animations plain Kirby's
  `AnimInfo` table (`0x0834C5D4`, 185 entries) uses, whole frames in the pink palette; names from
  `scripts/kirby/anims.toml`. `anims.txt` lists the scripts of all 939 animations in the table,
  `hitboxes.tsv` their `AnimCmd_6` boxes.
- `controller/src/*.rs`: the mod view's cut (`gbaa-recompile --cut`) of 11 controller functions,
  named from `names/kirby-amazing-mirror.toml`.

The recipe fails unless all 12 move states show up in the trace. Two runs give identical traces
and tiles.

## How it was found

1. **The decomp gives the layout.** katam (github.com/jiangzhengwenjz/katam @7d969fb, matching
   this ROM) is mostly unnamed (`sub_XXXXXXXX`), but it has `struct Kirby` (0x1A8 bytes),
   `gKirbys` at `0x02020EE0` (four of them, player 1 first) and the call `kirby->stateFn(kirby)`.
   That was enough to know what to watch.
2. **A RAM trace names the states.** `scripts/kirby/moves.lua` waits until Kirby first stands,
   then holds a fixed list of inputs (walk, dash, tap jump, held jump, float and flap, B to puff,
   crouch, down+A slide, inhale, running jump) and logs `stateFn`, position, speed, animation and
   boxes every frame. Each input lands in its own state, so the trace names them. The per-frame
   speeds give the constants. The decomp's C confirmed each one, e.g. the slide's `0x340` for 10
   frames, then `0x200`.
3. **The mod view.** Its cut works on Kirby with a 42-entry source map (`names/kirby-amazing-mirror.toml`). It prints the
   exact form, so `KirbyGroundAccel`, 30 lines of C, is 345 lines of Rust with `0x14c`, `0x1ac`
   and `0xee << 1` in it. That form is readable enough to check a constant, but the decomp's C was
   far quicker to read. For a game without a decomp, the trace plus this cut is the way in.
4. **Discovery** (`just discover`) found all 41 functions named here as function starts (5677 in
   the ROM). Where the decomp gives no address (`kirbyFlyUp`, `ObjectSetBounds`), the start next to
   its neighbour in the decomp's source order gives it.

## Functions

Addresses are Thumb entry points (as in the source map, minus bit 0). Confidence: **high** means
both the trace and the decomp's C agree. **Medium** means it comes from one of them only.

| Address | Name given | katam | What it does | Confidence |
|---|---|---|---|---|
| 0x0803EE18 | KirbyTask | sub_0803EE18 | Per-frame player task: input, pre-state checks, `stateFn(kirby)`; reruns the new state at once when the state changed | high |
| 0x0803F324 | KirbyReadInput | sub_0803F324 | `movementState` = held keys (`gHeldKeys` for player 1, the link or AI otherwise), `unk11A` = newly pressed | high |
| 0x0803FBB4 | KirbyPreStateChecks | sub_0803FBB4 | Death at hp 0, the low-health sound, and more not read | medium |
| 0x0803FE74 / 0x0803FF64 | KirbyEnterStand / KirbyStand | sub_0803FE74 / sub_0803FF64 | Idle on the ground; the hub that branches to every ground move | high |
| 0x08040868 / 0x08040930 | KirbyEnterCrouch / KirbyCrouch | | Down held on the ground | high |
| 0x080411E8 / 0x080412AC | KirbyEnterWalk / KirbyWalk | | Walking; a second tap within 16 frames goes to run | high |
| 0x08041B08 / 0x08041C50 | KirbyEnterRun / KirbyRun | | Running (dash) | high |
| 0x08042D70 / 0x08042E28 | KirbyEnterSlide / KirbySlide | | Slide (down+A); its entry spawns the slide's attack object and dust | high |
| 0x08043360 / 0x080435F8 | KirbyEnterJump / KirbyJumpRise | | A on the ground: the rise while A is held | high |
| 0x0804464C | KirbyJumpApex | sub_0804464C | After the rise (A let go or 21 frames): vy cut to 138, then gravity | high |
| 0x08044EA8 / 0x08044FD4 | KirbyEnterFall / KirbyFall | | Falling; A here starts the float | high |
| 0x080458FC / 0x08045A34 | KirbyEnterLandingBounce / KirbyLandingBounce | | Landing after a long fall: one bounce (vy 710, gravity 58) | high |
| 0x080476C4 | KirbyEnterFloat | kirbyFlyUp | A in the air | medium (address from discovery's neighbour) |
| 0x08047814 | KirbyFloatInflate | sub_08047814 | Puffing up: 11 frames, gravity 22 | high |
| 0x080487AC / 0x080488E0 | KirbyEnterFloatFlap / KirbyFloatFlap | | Floating: each A flap adds lift | high |
| 0x0804990C / 0x08049A60 | KirbyEnterExhale / KirbyExhale | | B while floating: the air puff; spawns the air bullet | high |
| 0x0804A728 | KirbyTakeHit | sub_0804A728 | Contact damage: `hp -= attacker.unk63`, knockback `0x180` or the attacker's `unk64` | high |
| 0x0804ACFC | KirbyHurt | sub_0804ACFC | Hurt (and dying) state | high |
| 0x0805474C / 0x08054950 | KirbyEnterInhale / KirbyInhale | | B on the ground: inhaling | high |
| 0x08055920, 0x08055D9C, 0x08056128 | KirbyEnterRoomArrive, KirbyRoomArrive, KirbyRoomArriveEnd | | The states while arriving in a room | medium |
| 0x0805B2FC | KirbyGroundAccel | sub_0805B2FC | Walk/run acceleration and speed cap | high |
| 0x0805B1B8 | KirbyGroundFriction | sub_0805B1B8 | Deceleration with no direction held (by animation and surface) | high |
| 0x0805C070 | KirbyWallBump | sub_0805C070 | Hitting a wall above speed 178: the bump animation | high |
| 0x0803E2B0 | ObjectSetHitbox | sub_0803E2B0 | Sets an object's hitbox from corners, stored at +0x38 as left, top, half width, half height | high |
| 0x0803E308 | ObjectSetBounds | ObjectSetBounds | Sets an object's terrain bounds (+0x3C: left, top, right, bottom) | high |
| 0x080711A8 | SpawnSlideAttack | sub_080711A8 | The slide's attack object | high |
| 0x08072C5C | SpawnAirPuff | sub_08072C5C | The air bullet | high |
| 0x08089D98 | SpawnSlideDust | sub_08089D98 | The slide's dust effect | high |

The game has many more states: `stateFn` gets 246 assignments in katam's `kirby.c`, most of them
for the copy abilities, water, ladders, doors and cutscenes. The ones above are what plain Kirby
does with the D-pad, A and B.

## The state machine

`KirbyTask` runs once a frame for each Kirby. It reads input into `movementState` (held) and
`unk11A` (new this frame), then calls `stateFn`. Each state's function is the whole per-frame
behaviour of that state. It moves to another state by calling that state's `Enter…` function, which
sets the animation, clears timers and flags and stores the new `stateFn`. When the state changed,
`KirbyTask` calls the new one once more in the same frame. Seen in the trace:

```
Stand ─right/left→ Walk ─tap again→ Run ─(let go)→ Walk ─(speed 0)→ Stand
Stand ─down→ Crouch ─A→ Slide ─(speed < 0x80)→ Stand
Stand/Walk/Run ─A→ JumpRise ─(A let go or 21 frames)→ JumpApex → Fall ─(ground)→ Stand
Fall/JumpApex ─A→ FloatInflate → FloatFlap ─B→ Exhale → Fall
Fall (from high) ─(ground)→ LandingBounce → Fall → Stand
Stand ─B→ Inhale ─(let go)→ Stand
any ─(enemy contact)→ TakeHit → Hurt → Walk/Stand
```

## Constants

Speeds are in 1/256 pixel a frame (`xspeed`, `yspeed`; `yspeed` positive is up). Position is
pixels << 8. The trace and the code agree on every one.

| Move | Constant | Value |
|---|---|---|
| Walk | acceleration a frame | 0x20 (first frame 64) |
| Walk | top speed | 0x14C (332, 1.30 px/frame); 0x1AC with object flag `unkC & 0x40` |
| Run | top speed | 0x1DC (476, 1.86 px/frame); 0x24C with `unkC & 0x40` |
| Ground | deceleration, no direction held | 14 a frame (8 or 9 by flag; 4 on slippery ground) |
| Wall | bump above | speed 178 |
| Jump | launch speed | 0x400 (1024, 4 px/frame) |
| Jump | gravity while rising | 38 a frame, for at most 21 frames held |
| Jump | release cut | vy becomes 138, then gravity 38 |
| Fall | gravity / terminal speed | 38 / -640 (2.5 px/frame) |
| Air | horizontal top speed (running jump) | 504, gaining 48 a frame |
| Float | flap lift | +120 a frame up to 336 |
| Float | gravity / terminal speed | 22 / -288 |
| Landing bounce | launch speed / gravity | 710 / 58 |
| Slide | speed | 0x340 (832) for 10 frames, then 0x200, slowing by 27 a frame (13 on slippery ground) |
| Air puff | bullet speed | 0x3C0 (960) |
| Hurt | knockback | 0x180, or the attacker's `unk64` |

## Hitboxes

There is no hitbox table for Kirby's own moves. Boxes are set in code, with immediates, in two
places:

- **Body box.** `ObjectSetHitbox(obj, left, top, right, bottom)` stores the left and top corner and the half
  width and height at +0x38 (`unk38` to `unk3B`). `ObjectSetBounds` stores the terrain box at +0x3C. Kirby's changes
  with posture. The trace shows -4,-2 with half size 4,4 standing, and -6,-8 with half size 5,7
  when puffed up. Crouching is set from (-7,3)-(5,7).
- **Attack boxes.** Each attack is a separate object, spawned by the move's entry, with its box
  and damage set right there. The slide's (`SpawnSlideAttack`) is (1,-6)-(16,14), damage
  `unk63 = 2`, knockback `unk64 = 0x1C0`. It is bigger with Sword or Master: (1,-8)-(32,14) and
  (1,-10)-(46,14). The air puff's (`SpawnAirPuff`) is (-5,-5)-(5,5), damage 2, knockback 0x1C0.
  Damage taken is `hp -= attacker.unk63` (`KirbyTakeHit`). `unk68` holds the attack's property
  bits.

The sprite engine (the one Sonic Advance uses) also has a per-frame hitbox command in its
animation scripts (`AnimCmd_6`, a box copied into the sprite's slot `unk20[i]`). Kirby's movement
animations set none: slot 0 stays -1 in every move traced, and the table agrees (no box in
animations 0 to 63). Some ability attack objects read it into their bounds every frame (katam
`sub_080753CC`, spawned by `sub_0807529C` with the `AnimInfo` table `gUnk_0834FF14`).

`hitboxes.tsv` lists every box: 73 of the 939 animations set one, 1,192 steps in all (anim,
variant, step, frame, slot, then left, top, right, bottom from the pivot, as `s8`; a box stays
until the script clears it). Matched against katam's `AnimInfo` tables, they belong to the copy
abilities: `gUnk_0834FF14` (the one above) uses animations 94, 111, 148 and 152, and
fifteen more tables use 94, 163 to 192 and 455 to 639. Animation 94, in ten of those tables, is a shared one.
Animation 148, variant 2, for instance, holds (0,-37)-(41,10) for 8 steps. Plain Kirby's table
uses only 64 and 68 of them, two animations (cutscenes, by their length) over 300 frames long whose box is a
zero-size point, (40,-42)-(40,-42). The engine mirrors a box when the sprite is flipped
(`sub_08155494`); the list is unflipped, facing right.

## Assets

- **Frames.** Player 1's frames are streamed one at a time into VRAM `0x06010000`
  (`(index << 11) + 0x06010000` for Kirby `index`) from ROM `0x084BEECC`–`0x084C9B6C`. 56
  distinct frames load during the moves. `gbaa-assets graphics` exports 131 sheets and 59
  animations from the run, 71 of them in Kirby's range. Frames Kirby draws as one 32×32 sprite
  come out whole: the float (`gfx_084c692c`, 6 frames), inflate and inhale. Most of the rest are
  built from 2 to 4 sprites of different sizes per frame (the engine's `SpriteAttributes`,
  `numSubframes`). The exporter sees one blob "shown at 3 sizes" and writes loose tiles, so walk,
  jump and fall aren't usable as sheets from the recording; `sprite-tables/` has them whole. The composing step (assets/sprites-composed)
  combined only 2.
- **Animation ids.** Kirby's sprite `animId`/`variant` by move: stand 0/0, crouch 0/1, walk 1/0,
  run 2/0, slide 3/0–1, jump/fall 4/0–5, float inflate 6/5, flap 7/0–1, puff 6/7, hurt 10/1.
  `animationIndex` (+0xD4) maps to these through per-ability `AnimInfo` tables (`gUnk_0834D484`
  and its neighbours).
- **Palettes.** Kirby's 16 colours: pink (player 1) `0x084BB12C`, yellow `0x084BB1AC`, red
  `0x084BB1CC`, green `0x084BB1EC`, exported as `palettes/pal_<addr>.pal`. The exporter gives a
  sheet the palette it was drawn with most. The CPU Kirbys share the frames, so some sheets come
  out yellow.

## Sprite tables

research/kirby-sprite-tables. `gbaa-assets sprite-tables` (`crates/assets/src/spritetables.rs`)
reads the engine's `struct SpriteTables` at `0x083B909C` (katam `include/sprite.h`) from the ROM,
with no recording:

- **`anims[animId]`** is an array of variant pointers. Each variant's script sits just before
  its array, so the array ends at the first word that isn't a ROM pointer below it (the last
  animation's ends where `anims` itself starts). A script is commands with negative ids
  (`gUnk_08D6081C`: -1 `GetTiles`, -2 `GetPalette`, -3 `JumpBack`, -4 stop, -5 sound, -6 the
  hitbox, -7 move, -9 `SetIdAndVariant`, -11 priority, and three unknowns of 3, 4 and 2 words)
  and `ShowFrame`s (delay in video frames, frame index; -1 shows nothing).
- **`attrs[animId]`** holds the frames, 12 bytes each: the first piece in `oamData` (bits 0-13)
  and the frame's flips (14, 15), the piece count, width, height, and the pivot's offset inside
  the frame. **`oamData[animId]`** holds the pieces, OAM attributes 0 to 2 (shape, size, flips,
  the piece's place in the frame, its first tile from the frame's).
- **Tiles**: each frame's `GetTiles` copies `count` tiles from `tiles4bpp + 32 × index`
  (`0x084BED4C`) to the sprite's VRAM; the pieces' tile numbers count from there. Every one of the
  939 animations uses this form; none uses the engine's other one (`attrs` 16 bytes a frame, tile
  lists through `unk18`).
- **Palette**: `GetPalette` names a 16-colour palette from `0x084BB0EC`. Kirby's scripts load
  palette 0 (`0x084BB0EC`). Player 1's pink (`0x084BB12C`) is palette 2, set by the player's
  code, so the recipe passes it.

Each animation becomes one Aseprite file: one canvas for every frame, with the pivot at the same
point (the sprite user data says where), the frames first (tag `frames`), then each variant's
steps as linked cels with their delays (a tag a variant: `walk`, `crouch`, `fall`; its user data
says how the script ends). Each step's cel user data carries its frame, video frames and any
hitbox. The `.png` is the frames side by side. All 46 of plain Kirby's animations draw whole:
walk (anim 1, 11 frames), run (2), slide (3), jump and fall (4, 18 frames with the somersault),
inhale, inflate and puff (6), float (7), hurt (10). `scripts/kirby/anims.toml` maps the moves to
animations from the trace (`spr=`): stand 0/0, crouch 0/1, walk 1/0, run 2/0, slide 3/0-1, jump
rise 4/0, fall 4/1-2, landing bounce 4/3 and 4/5, inhale 6/0-2 and 6/8-9, inflate 6/5, puff 6/7,
float 7/0-1, hurt 10/1. The command fails if a named animation can't be drawn.

Kirby's movement scripts end with -4 (stop on the last frame), the walk too, so the player's code
restarts them; how it picks the variant is in `KirbyWalk` and its neighbours, not in the table.

## What's left (for a rivals-forge port)

- **Copy abilities.** Their animations are in the same table and draw the same way: pass an
  ability's `AnimInfo` table (katam `kirby.c`, e.g. `gUnk_0834FF14`) as `--anim-info`. Which
  table is which ability isn't named here.
- RoA's movement is in pixels a frame at 60 fps, and the GBA runs at 59.73. The constants above
  divide by 256 directly. Kirby is about 24 px tall, and RoA characters are roughly 3 to 4 times
  that.

Nothing here is game data: addresses and numbers only. The frames and palettes stay in `out/`.
