# Fuzzing spike: does fuzzing reveal graphics and code routes miss?

Unit `assets/fuzz-coverage-spike`. june wanted a CPU-heavy answer tried
before a token-heavy one for "How should asset extraction reach graphics a
route doesn't show?" (the design's [Graphics
coverage](../asset-extraction-design.md#graphics-coverage-junes-call)):
drive Emerald headless under the interpreter with random inputs from
mutated saves and game state, many cheap runs, no agent in the loop, and
keep what reaches something new. Run 2026-10-28 on quack.

Rerun it with `just fuzz-coverage` ([commands.md](../tools/commands.md));
the report lands in `out/fuzz/spike/report.txt`, the curve in
`curve.tsv`.

## How it fuzzes

`crates/fuzz` (`gbaa-fuzz`), built with the runtime's `provenance`
feature. `runtime::prov`'s tags are process-wide, so each worker is its
own process with one machine; workers share nothing (no corpus sync).

- **Phases.** Each worker first plays the new-game route
  (`units/emerald-newgame.toml`, reset to the truck, 5400 frames), keeping
  its states at the title, main menu, Birch's speech, the naming screen and
  the truck. What it finds there is the *route* column below. Then it boots
  the crafted `alice.sav` (`just emerald-saves`: Oldale Town's Pokémon
  Center 2F, Mudkip and Wurmple) and continues into the overworld. Then it
  fuzzes.
- **A fuzzing run** picks a kept state (a tournament of four, by what its
  runs found per pick), loads it, and plays 300 to 900 frames of random
  presses: a *walker* (a direction held 8 to 48 frames most of the time,
  A, B, Start now and then) or a *masher* (short presses, A most). One run
  in seven first *pokes* the game in RAM: random party Pokémon (any
  species, level, two random moves) written into `gPlayerParty`, random
  flags flipped in the live save block, or random bag items.
- **A mutated save** (one iteration in ten instead of a run): the crafted
  save with random party (1 to 6 Pokémon), a random map (any of the 34
  groups' maps, arriving at its first warp), random flags (and half the
  time the system flags and badges), random bag pockets, random Pokédex
  seen and owned bits, random vars; written as the game writes flash
  (`emerald_save::flash`), booted from reset and continued (Start, A, A),
  about 1400 frames.
- **Coverage feedback**, checked every 30 frames: a new ROM instruction run
  (a bit per ROM halfword, marked before each `Machine::step`), a new ROM
  load in the load log (`prov::take_loads`, by operation and source), or a
  new bit in the provenance spike's bitmaps of ROM bytes that reached
  palette, VRAM or OAM (compressed blobs' first bytes and copied bytes).
  Something new keeps the state there (at most 300 a worker, the worst for
  its picks dropped).
- **Counted the route's way.** Graphics are compressed blobs plus copied
  ROM ranges (runs joined across gaps under 16 bytes, as `gbaa-assets
  check-load-log` joins them) that reached palette, VRAM or OAM: the
  new-game route's 37 + 58 = 95, the design's "about 100". Each is also
  placed in the source map's ROM data symbols (`names/pokeemerald.toml`).
  Code is ROM instructions run, placed in pret's functions
  (`truth/pokeemerald.funcs.tsv`) and checked against code discovery's
  starts (`disc/Pokemon - Emerald Version.starts`). Screens are distinct
  `gMain.callback2` values seen.

## What it found

10 workers for 45 minutes, 2026-10-28: 7.5 CPU-hours, 4.0 million frames
(149 a CPU-second, with 10 running), 5,255 runs, 592 boots of a mutated
save, no faults. Peak memory about 1.5 GB for all ten.

| | route | fuzz |
|---|---:|---:|
| graphics: compressed blobs + copied ROM ranges reaching the screen | 95 (37 + 58) | 2,334 (720 + 1,614) |
| source map data symbols they fall in | 115 | 1,461 |
| of them named as images or palettes (`Gfx`, `Pal`, `Tiles`, `Pic`, `Icon`, …) | 94 | 1,203 |
| ROM loads (operation, source) in the load log | 139 | 4,729 |
| ROM instructions run | 40,170 | 210,748 |
| pret functions run | 1,153 | 4,903 |
| of them not among code discovery's starts | 22 | 84 |
| screens (`gMain.callback2`) | 14 | 61 |
| maps kept states stood in | | 168 |

- **Graphics: about 13 times the route's, about 16% of pret's.** 1,203
  image and palette symbols against pret's ~7,608 `INCGFX` files (the
  symbols and the files aren't one to one, so this is a rough share).
  Past the route: 228 item icon symbols, 151 Pokémon icons, 88 front and
  28 back pictures with 90 palettes, 139 tileset symbols (many maps' from
  the random warps), 84 overworld object pictures, 48 battle animation
  symbols, 26 trainer symbols, the bag, party, summary, Pokédex, trainer
  card and berry tag screens' own. The rest of the 1,461 are map layouts,
  metatiles and tables whose bytes are copied on the way to a tilemap.
- **Screens.** 47 the route never shows: continue and map loads, the
  options menu, Pokédex, party menu, summary, bag (and giving an item from
  it), battles, with the bag and party from inside battle,
  the trainer card, the berry tag screen. They were all found in the first
  CPU-hour or so; the count sat at 56 from 3 CPU-hours to 6 and reached
  61 near the end.
- **Code.** 210,748 ROM instructions in 4,903 of pret's 15,859 functions,
  about five times the route's. 84 of the functions run (62 past the
  route: battle controller emitters, `Cmd_getexp`, `MonGainEVs`,
  tileset animations and more) are among the 706 pret functions code
  discovery's 15,165 starts don't include: code that ran, at a function
  start discovery doesn't list (whether codegen covers it inside a
  neighbour wasn't checked).

### How it grew

Union of the workers, by CPU-hours (`curve.tsv`):

| CPU-hours | graphics | image symbols | instructions | functions | screens |
|---:|---:|---:|---:|---:|---:|
| 0.75 | 735 | 359 | 117,565 | 3,069 | 45 |
| 1.50 | 1,062 | 512 | 143,399 | 3,555 | 53 |
| 2.25 | 1,300 | 626 | 168,686 | 3,962 | 55 |
| 3.00 | 1,492 | 739 | 174,303 | 4,078 | 56 |
| 4.50 | 1,784 | 922 | 185,078 | 4,305 | 56 |
| 6.00 | 2,093 | 1,071 | 204,001 | 4,775 | 61 |
| 7.51 | 2,334 | 1,203 | 210,748 | 4,903 | 61 |

The first CPU-hour gives about 400 image symbols; after that the rate
settles to about 110 an hour (626 to 1,203 over the last 5.25), not yet
flat. The last quarter of the run added 15% of the graphics and 9.5% of
the instructions. Screens flatten first. At a steady 110 an hour the rest
of pret's graphics would take some 60 CPU-hours, but most of what's left
sits behind story progress (later towns' events, contests, the Battle
Frontier, link and Union Room screens, most Pokémon's battle pictures)
that random presses from a few seed states rarely reach, so the curve
will bend further.

## What it says about the question

Fuzzing finds what a route misses, cheaply: in one 45-minute run on one
machine, with no agent tokens, about 1,200 image and palette symbols with
their palettes as the game drew them (these are watched loads, so the
export's palettes, layouts and sprite timing come with them), against the
route's ~100 and the "more short scripts" option's few thousand. It
doesn't reach everything: story-gated screens need states a random walk
won't make, which a mutated save can only partly fake (flags and vars set
at random rarely make a consistent story point).

Cheap ways to push the curve further, none built here:

- **Share the corpus** between workers (they ran blind to each other, so
  much of each worker's time re-found what another had).
- **Seed states at story points**: saves with the right flags and vars for
  each town's events, a contest, the Frontier; or a published TAS's
  states, per june's rule on long routes.
- **Species and item sweeps** from a mutated save: a battle against each
  species, the bag with each pocket full, which a random draw reaches
  slowly (88 of 412 front pictures in 7.5 CPU-hours).
- **Pair it with the static scan** (option c): fuzzing gives the palettes
  and use of what it reaches, the scan names the compressed blobs it
  doesn't.

## Rescored against pret's used files

`just assets-coverage` (assets/coverage-report, the design's Graphics
coverage) scores this run against pret's 7,405 image, palette and
tilemap files used in play (7,586 `INCGFX` files less 181 never used):
1,609 found, 21.7% (the route phase: 132, 1.8%). Images 802 of 3,875
(576 with a palette pret keeps beside them, 132 greyscale, 94 with none
beside them), palettes 734 of 3,236, tilemaps 73 of 294. By kind:
tilesets' images 78%, item icons 48%, Pokémon icons 36%, overworld
objects 35%, menus and other screens 20%, battle animations 10%, Pokémon
pictures 9%, trainers 7%, footprints and contests and the Frontier about
none. `out/fuzz/spike/coverage.txt`, and `coverage.tsv` a line per file.

## v2: sweeps and a shared corpus

Unit `assets/fuzz-v2` (june, 2026-10-29: push toward ~90% of the coverage
report's used graphics). Run 2026-10-29 on quack as `just fuzz-coverage v2
12 35`: 12 workers for 35 minutes, 7.0 CPU-hours (the spike's 7.5), 3.5
million frames, no faults. Added to `crates/fuzz`:

- **Sweeps.** Each worker takes every twelfth item of each list, a sweep
  step in a quarter of its iterations. A *wild battle* against each
  species, normal then shiny (personality picked against the player's ID),
  with the party's lead the same species for its back picture: from the
  crafted save's overworld, `gEnemyParty` and `gBattleTypeFlags` written,
  `gMain.savedCallback` set to `CB2_ReturnToField`, `gMain.callback2` to
  `CB2_InitBattle`, then 900 frames of mashing. The game takes it: the
  battles reach `BattleMainCB2` and return to the field. A *trainer
  battle* the same way against each of the 854 trainers
  (`gTrainerBattleOpponent_A`, `BATTLE_TYPE_TRAINER`). A *sweep save*: a
  boot of each map in turn with every bag pocket full of consecutive items,
  the Pokédex all seen and owned, the system flags and badges on, and six
  consecutive species in the party. 1,148 sweep steps ran: about two thirds
  of the species and trainer lists, under half the maps.
- **A shared corpus** in `out/fuzz/NAME/shared/`: every two minutes each
  worker writes its coverage bitmaps and ORs in the others', so a state is
  kept only for what's new to all of them, and takes up to 24 of the states
  the others found most with (a state that found 100 things or more is
  shared, 150 a worker at most, about 640 KB each; deleted when the run
  ends).

Against pret's 7,405 used files (`just assets-coverage out/fuzz/v2`):
**2,987 found, 40.3%** (the spike: 1,609, 21.7%, for about the same CPU).

| kind (images) | used | spike | v2 |
|---|---:|---:|---:|
| Pokémon pictures | 1,247 | 9% | 62% |
| trainers | 101 | 7% | 68% |
| battle animations | 291 | 10% | 50% |
| tilesets | 194 | 76% | 86% |
| overworld objects | 244 | 35% | 42% |
| Pokémon icons | 415 | 36% | 38% |
| menus and other screens | 682 | 20% | 22% |
| battle interface and transitions | 67 | 30% | 16% |
| item icons | 218 | 48% | 8% |
| Pokémon footprints | 387 | 0.5% | 0.5% |
| contests and Battle Frontier | 14 | 0% | 0% |

Palettes 1,319 of 3,236 (41%; Pokémon pictures' 68%, tilesets' 36%),
tilemaps 71 of 294 (24%). Image and palette symbols by the old measure:
2,511 (the spike's 1,203).

**The curve** (`out/fuzz/v2/curve.tsv`, union of workers): image
symbols 574 at 0.7 CPU-hours, 1,157 at 1.75, 1,804 at 3.9, 2,339 at 6.0,
2,511 at 7.0. Still rising about 170 an hour at the end, not flat; the
last quarter added 10.5% of the graphics. Most of the rise is the sweeps
working through their lists, so it will slow once they're done.

**What's left, and why:**

- *Footprints* (385): only the Pokédex's entry page draws them. The sweep
  saves own every species, but random presses rarely open the Pokédex and
  page through entries. A Pokédex sweep (each entry opened) gets them.
- *Item icons* (200 missing; the spike had 105): the spike's random bags
  were browsed often; v2 spent those iterations on battles and its full
  bags hold consecutive items 37 apart per map, so most go unseen. A bag
  sweep that scrolls each pocket is the fix.
- *The rest of the Pokémon pictures and the trainers*: the sweeps didn't
  finish (about two thirds done); a longer run finishes them.
- *Menus and other screens* (533), *battle interface* (56), *contests and
  the Frontier*, tileset palettes (745, many maps' second palettes:
  weather, time of day, events): story-gated or screen-specific. These
  want seed states at story points (a TAS's states, per june's rule on
  long routes) and steering toward the code that loads them.

## Dex and bag sweeps

Unit `assets/fuzz-dex-bag-sweeps`, 2026-10-30 on quack: the two fixes
named above, in `crates/fuzz`. Each worker runs its share before fuzzing,
from the crafted save's overworld state with the game poked in RAM (the
Pokédex, party and national-dex flags, `VAR_NATIONAL_DEX`, the dex's
`nationalMagic` and mode, every entry seen and owned in all three copies
of the seen bits, and the start menu's cursor):

- **Pokédex**: chunks of 32 of the national dex's 386 entries.
  `sLastSelectedPokemon` set to the chunk's first, Start and A open the
  list, A until `sPokedexListItem` is set (the entry page loading), then
  Down every 20 frames until its `dexNum` is the chunk's last. The entry
  page takes a Down only once the last entry has loaded, so fast presses
  skip nothing; a fixed 80- or 120-frame cadence lost one press in two or
  one in five (an entry takes about 100 to 160 frames).
- **Bag**: four passes. Each fills the TMs and HMs pocket with the 58 TMs
  and HMs, the berries pocket with the 43 berries, and the items, Poké
  Balls and key items pockets with the pass's 76 of the other 275 items
  (the TM and berry lists print a move or berry number from the item, so
  only those go there), resets `gBagPosition` to the top of each pocket,
  opens the bag and presses Down every 8 frames through each pocket, then
  Right.
- **`table` rows.** A footprint is 1bpp, expanded to 4bpp in code
  (`DrawFootprint`), so its bytes reach VRAM as computed values, which
  `runtime::prov` tags with the ROM table it guesses they came from
  (`reached`'s third bitmap). The worker ignored that bitmap; it now logs
  it as `table` rows, and `gbaa-assets coverage` finds a `.1bpp` image
  (footprints, Spinda's spots) by them. Other images aren't, since a guess
  could name the wrong table.

The sweeps alone (`gbaa-fuzz run … --workers 13 --minutes 0`, about 90 s a
worker, no faults), scored by `just assets-coverage out/fuzz/dexbag`:
**footprints 386 of 387** (v2: 2; the one left is the question mark's,
which no national dex entry shows), **item icons 218 of 218** and their
palettes 253 of 253 (v2: 8%), and on the way 386 Pokémon front pictures
with their normal palettes. 1,805 of the 7,405 used files in 18 CPU-minutes.

## Story seeds

Unit `assets/fuzz-story-seeds`, 2026-10-30 on quack. What v2 left
(menus, battle interface, contests, the Frontier, tilesets' second
palettes) sits behind story progress, so the workers want seed states at
story points with flags and vars that agree. june's call: a published
TAS's states first, crafted story saves where it won't sync or doesn't
reach.

**The TAS doesn't sync.** TASVideos has two Emerald runs made on
BizHawk's mGBA core: GoddessMaria's glitchless any% (movie 3287, BizHawk
1.11.6, 475,027 frames) and drkspace's glitchless living dex (movie 6182,
BizHawk 2.9, 3,094,273 frames, one reset). `mgba-dump --bk2` replays a
movie's input log (3,000 frames a second) and `--save-state-at` keeps
states along it; `gbaa-fuzz story` reads each for map, badges and story
flags. Both desync early: 3287 gets through the intro, the starter
battle and Route 101, then from about frame 45,000 its player stands
in Oldale's mart, talking to a customer again and again, with no badge
by frame 126,000. 6182 goes wrong just after the starter: the player
wanders Route 101 for 160,000 frames and then stands in the house at
Littleroot to frame 1.2 million. The likely causes, untested: BizHawk ran both on the
official BIOS (SHA-1 300c20df…, `SkipBios` on) and `mgba-dump` runs
mGBA's HLE BIOS, whose decompression and other calls take different
cycles, so lag frames and the RNG drift; and BizHawk's clock runs from
2010-01-01 while ours stays at 2004-09-16. This machine has no BIOS
dump. With one, `mgba-dump` would need a `--bios` option to try again.

**Nor on the official BIOS** (unit `assets/fuzz-tas-bios`, 2026-10-30).
june's dump is at `$GBAA_DATA/bios/gba_bios.bin`. `mgba-dump --bk2` now
sets mGBA up as BizHawk 2.9's `bizinterface.c` and `MGBAHawk.cs` do for
a movie: idle loops not skipped, the clock at `RTCInitialTime`
(2010-01-01) plus `frame * 4389 >> 18` seconds, and `--bios` boots the
dump with its intro, since BizHawk's `SkipBios` holds only outside a
movie (`SkipBios && !DeterministicEmulationRequested`). Both runs still
desync, and earlier than under the HLE BIOS: 6182 gets the truck, the
clock and May's room, but by frame 8,800 it runs a few frames behind
the HLE run (text a character or two short) and then stands in May's
house for good, no starter; 3287 gets the starter and wanders Route 101
and Littleroot, no badge by frame 200,000. Skipping the intro or not,
the idle loop and the clock change nothing that `gbaa-fuzz story`
shows. What's left is the emulator: 6182 was made on BizHawk 2.9, whose
mGBA is TASEmulators' fork at 645282a (2023-04, 0.11-dev), and 3287 on
BizHawk 1.11.6's far older one; ours is 0.10.5, whose CPU and memory
timing differ from both. Building that fork for `mgba-dump` is the next
try (june's OK, 2026-10-30): `scripts/fetch-mgba.sh tas` builds it and
`just mgba-dump-tas` links `mgba-dump` to it, the shim unchanged (the
calls it makes have the same signatures in 0.11-dev). It can only help
6182: 3287's BizHawk 1.11.6 ran a far older mGBA.

**On the fork, 6182 syncs to the end** (2026-10-30, quack). With
`--bios`, BizHawk's clock and no idle skips, `just emerald-tas` replays
all 3,094,273 frames in about half an hour and keeps a state every
100,000 frames in `$GBAA_DATA/saves/emerald/story/`, beside the crafted
saves. `gbaa-fuzz story` on them: the Pokédex by frame 25,000, the
Stone Badge by 1.0 million (a living dex catches as it goes), the
PokéNav with it, then badges 2 to 8 by 2.8 million, and game clear, the
Frontier pass and the national dex by 2.9 million; badges never fall.
On 0.10.5 the same settings strand it in May's house; whether the
fork would sync without the BIOS wasn't tried. The shim had kept every FIFO and PSG
record of a run, which a whole movie grows past 8 GB; it keeps them now
only for the dumps that want them (`gbaa_mgba_record_audio`).

The workers take the folder's `state-N.ss` too: each loaded with
`load_mgba_state` (they're saved between frames, the game in its own
wait loop, never in the BIOS, so all 30 load), run 60 frames unpressed
and kept as a story seed. An mGBA state doesn't carry the cartridge's
save, so a seed's flash is whatever the worker booted last; a game
that saves or loads from one writes or reads that.

**Result.** `tas30`, 10 workers for 30 minutes seeded with the 12
crafted saves and the 30 TAS states (`just fuzz-coverage tas30 10 30 ROM
$GBAA_DATA/saves/emerald/story`), against `story30` (Crafted story saves,
below): 3,598 against
3,496 of 7,405 used files (47.2% to 48.6%), and 6,108 against 5,084 of
pret's functions reached. Menus gain most: images 142 to 169, tilemaps
43 to 73, palettes 93 to 121; the battle interface 12 to 20, overworld
objects 99 to 105, tileset palettes 381 to 388. Battle animations fall
again, 123 to 117, and Pokémon icons 147 to 145: 42 seeds take more of
the picks. Contests and the Frontier stay at 0 of 14 images, though the
last states stand past the Frontier pass: they want sweeps of their own.

**Crafted story saves instead.** `crates/emerald-save/story.toml` lists 12
points along the main story (after the Route 103 rival and the Pokédex,
each badge, Lilycove after the Aqua hideout, before the Elite Four, and
the post-game at the Battle Frontier), each what pret's scripts change
since the one before: flags set and cleared (badges, story flags, the
`FLAG_HIDE_*` object flags that decide which people stand where), vars
(each town's and route's `*_STATE`), key items and HMs, the party's
level and a tile to stand on. `just emerald-story` applies them to
`alice.sav` cumulatively and writes a save a point. Each worker boots
every one after its sweeps and keeps the overworld as a story seed:
never dropped, one pick in three. And picking now weighs a first
sighting of graphics 8 times an instruction's, so states whose runs load
unseen graphics get picked more.

All 12 saves load where they should: Continue in mGBA stands the player
in front of each point's Pokémon Center (Oldale through Sootopolis), in
the League's hall before the Elite Four and at the Frontier, with the
right badge count, and `gbaa-fuzz story` reads the same maps back.

**Result: a little, not the jump hoped for.** Two runs of 10 workers for
30 minutes on quack, `base30` without story seeds and `story30` with
them (`just assets-coverage`): 3,448 against 3,496 of 7,405 used files
(46.6% to 47.2%). The gain is where the card pointed: menu images 115 to
142, menu tilemaps 32 to 43, menu palettes 76 to 93, battle interface 10
to 12, overworld objects 94 to 99, Pokémon icons 135 to 147, and 4,656
to 5,084 of pret's functions reached. Tileset palettes went 395 to 381
and battle animations 128 to 123, time the story seeds took from the map
and battle sweeps. Contests and the Frontier stay at 0 of 14 images:
random presses from a Pokémon Center door don't reach a contest hall or
a Frontier facility's battle in 30 minutes. That wants sweeps of their
own (enter each hall, each facility), not more seeds.

## Contest and Frontier sweeps

Unit `assets/fuzz-contest-frontier-sweeps`, 2026-10-30 on quack. After
the story seeds each worker plays its share of 32 *scenes*, each booted
from the last story save (the Frontier, post-game) with a party of six
species from the first 149 at level 50, every master ribbon on each:

- **Contests**, five categories at four ranks. The player stands in
  Lilycove's contest lobby and a script written to free EWRAM (past
  `sRayScene`, at 0x0203F000) does the receptionist's part once the
  choices are made: `ClearLinkContestFlags`, rank and category vars,
  `TryEnterContestMon` (which picks the opponents), the trainer and
  player graphics, the contest type, `VAR_CONTEST_HALL_STATE` 1 and
  the warp to the hall. The game's own script context runs it, pointed at
  it as `ScriptContext_SetupScript` would (`sGlobalScriptContext`'s mode
  and pointer, its status, `sLockFieldControls`). The hall's own scripts
  then run the whole contest; with A every 8 frames one takes about
  29,000 frames to its results and back to the lobby.
- **Frontier challenges**, each of the 12 attendants (Tower singles,
  doubles and multis, Dome and Palace singles and doubles, Arena,
  Factory singles and doubles, Pike, Pyramid), the player across the
  counter and the attendant's script started the same way (a `goto`).
  Talking by pressing A from a crafted position never started them. A
  takes each menu's first choice; some screens need their own presses,
  read from `gMain.callback2`: the choose-half party menu gets slots 2
  to N entered in `gSelectedOrderFromParty` (N from `gSpecialVar_0x8005`)
  so the next A enters slot 1 and lands on Confirm (A alone toggles one
  mon in and out for good); the in-battle party menu Down, A, A (else A
  picks the fainted mon forever); the summary screen B, Left, A; the
  Dome's tourney tree A, B, Down, B; the Factory's rental and swap
  screens and the overworld past a lobby random presses, directions held
  16 frames, so the Pike's paths and the Pyramid's floors get walked.

`scenes.tsv` in each worker's folder has, every 600 frames, the screen,
the map and what was found. In a run of 16 workers with no fuzzing after
(`just fuzz-coverage scenes16 16 0 ROM STORY`, about 13 minutes), every
contest reached `CB2_ContestMain` and most their results screen; Tower
singles and doubles, Palace singles and doubles, Arena, Factory singles
and Pyramid reached battles (`BattleMainCB2`), the Factory's rental
screen and the Dome's tourney tree showed. Still short: Tower multis
(picking a partner in the multi room takes walking to one), Dome battles
(the tree shows, its pre-battle room's menu doesn't move on), Factory
doubles (rents, then sits on a summary screen) and the Pike (its run
ended back at the copyright screen, a reset, before any battle).

**Result:** contests and the Frontier 12 of 14 images (from 0 in
`story30`), 7 of 7 palettes and 6 of 6 tilemaps; the two images left
(`gJPContestGfx1`, `gContestFaces_Gfx`) only the scan has.
Battle interface and transitions 26 of 67 images (`story30`: 12): the
Frontier's battle environment palette, the Arena's judgment symbols, the
Dome's tree and info card, the Factory's menu and picture backgrounds,
the Pyramid floor palette, and the Frontier logo transition. The run as
a whole, without the 30 minutes of fuzzing `story30` had, scores 2,665
of 7,405 used files (36.0%).

## PC box sweep

Unit `assets/fuzz-pc-box-sweep`, 2026-10-31 on quack. f90c left 190
Pokémon icons and all 165 PC storage files: no run opened the PC. After
the bag passes each worker plays its share of four *PC passes* from the
crafted save's overworld (`crates/fuzz/src/worker.rs`, `pc_sweep`):

- The 14 boxes (`gPokemonStoragePtr`, 420 `BoxPokemon`) filled in RAM
  from a list of every species (1 to 411) and Unown's 27 other forms,
  each its own icon, the letter set by the personality's low two bits of
  each byte. Two passes cover the 438; a third repeats the first 420 in
  move items mode with each mon holding an item.
- The PC opened by a script written to free EWRAM, `special
  ShowPokemonStorageSystemPC` (index 63) then `waitstate`; Down to move
  Pokémon or move items, A. In a box the cursor goes Right six times a
  row (it wraps to the row's start) and Down, so every slot is hovered
  and its mon's picture and data drawn; Start goes to the box's title and
  Right scrolls to the next box. Each box gets its own wallpaper in RAM
  (`boxWallpapers`, the pass's 14 of the 17). Then the box menu's Jump
  (the box popup) and Wallpaper, the Party Pokémon button and, in move
  items, the first mon's item Info.
- The fourth pass puts every box on Walda's wallpaper and scrolls 30
  times, `waldaPhrase.patternId` and `iconId` poked before each scroll:
  all 16 patterns and 30 icons.

A box pass is about 10,400 frames, the Walda pass 3,000; four workers
with `--pc-only 1` (out/fuzz/pc2) take under two minutes.

**Result**, f90c's workers and pc2's scored together
(`just assets-coverage` on a folder of links to both): **4,672 of 6,755
(69.2%, from 64.0%)**, 6,070 (89.9%) with the scan's sheets. Pokémon
icons 414 of 415 (from 225); the one left is Deoxys's, which the box
reads from 0x400 into its sheet (Emerald's form frame), never its start.
PC storage files 132 of 165 found (from 0), 32 scan only (the wallpaper
tilemaps, unpacked to WRAM and drawn from there) (found since the contest-appeal unit's scorer change) and one missing,
`box_selection_popup.pal`, which `LoadChooseBoxMenuGfx` only loads when
asked to and no caller asks. Menus and other screens 227 of 682 images
(from 156), 160 of 560 palettes (from 101).

## Move sweep

Unit `assets/fuzz-move-sweep`, 2026-10-31 on quack. A move's animation,
and the sprites, backgrounds and palettes it loads, plays only when the
move is used in battle (or a contest appeal), so after the bag passes
each worker uses its share of the 354 moves, a wild battle each (1,800
frames; about 4 minutes a worker with 16). The lead, a random species at
level 100 and three quarters of its HP, knows only the move; the foe is
a level-100 Squirtle knowing only Splash (pure Water, no immunity), the
battle scene option on, A every 8 frames (FIGHT, then the first move,
about three uses a battle).

The first run found almost nothing new: the poked lead's OT is FUZZ, not
the player, so the game counts it as traded and at level 100 with no
badges it ignores orders (falls asleep, hits itself). The eighth badge's
flag set in RAM fixed that. `MoveSetup` then sets up moves that fail
(and so skip their animation) in that battle: heal moves (the HP),
Snore and Sleep Talk (the lead asleep), Dream Eater and Nightmare (the
foe asleep), Curse's Ghost form (a Gastly lead), Attract (female lead,
male foe), Counter and Mirror Coat (the foe Tackles or uses Water Gun),
one-hit KOs (the foe at level 30).

**Result** (`just fuzz-coverage moves16 16 1`, no story seeds, scored
by `just assets-coverage`, with `MoveSetup`): battle animations 197
of 291 images (`scenes16`, 16 workers with story seeds and no move
sweep: 129), 202 of 277 palettes (124) and 26 of 34 tilemaps (7); the
run 3,333 of 6,755 used files (49.3%). A run before `MoveSetup` had
188, 192 and 22. `MoveSetup`'s 11 targets all load in it: Soft-Boiled's
egg, Morning Sun's star and light beam, Moonlight, Curse's nail and
spirit, Mirror Coat's wall, Snore's Z, Fissure, Guillotine, Attract.

Of the 200 files missing before `MoveSetup`, 167 belong to an `ANIM_TAG_*` that
no script in `data/battle_anim_scripts.s` loads (fangs, fury swipes,
substitute, the eggs but one...): only the tag table points at them, so
no play reaches them, and the coverage report should count them as
unused. Most of the rest are background tilemaps for the other side or
for contests (`*_Player`, `*_Opponent`, `*_Contests`), drawn when the
foe uses the move or in a contest appeal, and Spit Up (Stockpile first).

**The foe's side** (`assets/fuzz-move-sweep-foe-side`, same day): after
the lead's pass each worker plays its share again with the roles swapped,
the wild mon (a random species, or `MoveSetup`'s) knowing the move and a
Squirtle lead knowing Splash (or the setup's Tackle or Water Gun), each
`foe-move-N` in `scenes.tsv`. `MoveSetup` carries over: the user sleeps
for Snore, the target for Dream Eater, the target's level drops for
one-hit KOs. One exception: the foe's Curse is a Squirtle's, since the
stat form's mask (`masks/curse`, drawn by
`AnimTask_DrawFallingWhiteLinesOnAttacker`) went missing once the lead's
Curse was always a Gastly's. Spit Up and Swallow fail with nothing
stockpiled, so in their battles `gDisableStructs[0..2].stockpileCounter`
(0x020242C5, 0x1C apart) is set to 3 every 60 frames once
`BattleMainCB2` runs (a switch-in clears it); Spit Up's red orb then
loads.

**Result** (`just fuzz-coverage foemoves16 16 1`, `just assets-coverage
out/fuzz/foemoves16`): battle animations 197 of 291 images, 202 of 277
palettes, 29 of 34 tilemaps (26 before); the run 3,346 of 6,755 used
files (49.5%; 3,333). Every `*_Player` and `*_Opponent` background
loads now; the 5 tilemaps left are the contest ones (`drill_contests`,
`guillotine_contests`, `impact_contests`, `scary_face_contest`,
`water_contest`), drawn only in a contest appeal. Images and palettes
barely move: the foe's pass draws the same sprites, and the files still
missing are mostly the `ANIM_TAG_*` no script loads. The two passes take
about 8 minutes a worker with 16, the whole run about 25.

**Contest appeals** (`assets/fuzz-contest-appeal-anims`, 2026-10-31):
in the scene sweep each contest's entered mon knows first one of the
five moves whose appeal draws a contest-only background (`CONTEST_MOVES`:
Mega Punch, Horn Drill, Guillotine, Scary Face, Surf, by category), and
A picks it every round (the move cursor stays put). All five blobs then
load, but the scorer still had them scan only:
`AnimLoadCompressedBgTilemapHandleContest` unpacks the tilemap to a
buffer and `RelocateBattleBgPal` rewrites each entry's palette bits
before the copy to VRAM, so the screen gets computed bytes, tagged with
the blob's start as their guessed table (`table` rows). `just
assets-coverage` now counts a compressed file found by a `table` row at
its blob's start. **Result** (`just fuzz-coverage contest16 16 0 ROM
STORY`, about 30 minutes; scored against 6,591 used files, the
unscripted tags left out since): battle animation tilemaps 34 of 34
(29), and the PC's 32 wallpaper tilemaps, drawn the same way, found too
(menus' tilemaps 58 of 218, from 26); the run 3,535 of 6,591 (53.6%;
3,498 before the scorer's change).

## Kirby & The Amazing Mirror

assets/kirby-fuzz, 2026-10-31 on quack: the fuzzer tried on a game with
no pret, whose assets we don't have all of. Rerun with `just kirby-fuzz`
([commands.md](../tools/commands.md)); the score is
`out/fuzz/kirby/scan-curve.tsv`, the blobs missed `out/fuzz/kirby/scan/`.

**The port.** What every worker shares moved out of Emerald's into
`crates/fuzz/src/core.rs`: the machine run frame by frame with each ROM
instruction marked, the provenance coverage, the kept states and the
tournament pick, the shared corpus, random presses, the logs. A game
gives two readers, its screen and its place. `core::call` is new: it
calls one of the game's functions between frames (System mode, the main
loop's stack, IRQs masked, the CPU put back after). Kirby's worker
(`crates/fuzz/src/kirby.rs`, about 200 lines) has no save to edit, so
its states come from play and RAM pokes:

- **Route**: a new file to the first room and the timing lab's walk
  (4,500 frames, the route phase).
- **Room sweep**: Kirby sent through a door to each of the 983 rooms
  in `gRoomProps` (`roomId` and `spawnLocation` written, then the door's
  entry, `sub_0805545C`, called), 600 frames of walking.
- **Ability sweep**: each of the 26 copy abilities set and its loaders
  called as the game does when it gives one back, 600 frames of
  mashing.
- **Pokes** (one run in seven): a warp to a random room, a random
  ability, health and lives refilled, or bits of the visited doors and
  treasures (the pause map's and the collection room's).

The addresses (`gKirbys` and its fields, `gRoomProps`'s length, three
functions) are katam's, the partial decomp whose RAM layout matches this
ROM (memory/kirby-amazing-mirror.md); it has no names for the graphics,
so it gives no denominator. With no decomp at all, the door and the
ability loaders are what `research/kirby-controller-probe`'s trace finds:
watch `stateFn` and `roomId` while Kirby goes through a door.

**The denominator** is the static scan's 1,202 LZ77 blobs (`gbaa-assets
scan`, its new `--curve` scoring a run every 5 minutes). 1,059 of them
start at a katam symbol, and 681 of the 744 missed have an aligned ROM
pointer to them, so nearly all are real data. Kirby keeps most of its
sprites uncompressed, so this counts backgrounds, tilesets and screens,
not sprites (the copied ranges are in `events.tsv` but have no
denominator).

**The run**: 12 workers for 45 minutes, 9.0 CPU-hours, 6.35 million
frames (about 196 a second a worker), 10,333 random runs, 1,009 sweep
items, 1,528 pokes, 1,017 faults (a run ended, about one in ten: most
seem to be warps into rooms whose door doesn't hold, not looked into).
Kept states stood in 495 rooms.

| minutes | blobs reached | of 1,202 |
|---:|---:|---:|
| route phase | 15 | 1.2% |
| 5 | 439 | 36.5% |
| 15 | 446 | 37.1% |
| 30 | 455 | 37.9% |
| 45 | 458 | **38.1%** |

The room sweep does nearly all of it: the rooms' graphics
(`0x0890_0000` to `0x08AF_FFFF`) are 352 of 355 reached. Random play and pokes then
add 19 blobs in 40 minutes: **flat**. Emerald's spike, for scale,
reached 743 of its 4,350 blobs (17%) in 7.5 CPU-hours with no sweeps.

**What's missed**, 744 blobs, 503 of them in `0x0820_0000` to
`0x082E_FFFF` (the tiled-background table `gUnk_082D7850`'s screens) and
146 in `0x0816` to `0x081F`: by a look at the sheets, the sub-games'
screens and instructions (Crackity Hack's `gCrackityHackBgMainTileset`,
Kirby Wave Ride's text), the same text in the other languages the US
ROM carries (`gCrackityHackGameEndedTileset_Japanese`, `_Italian`; only
one is ever shown), link and wireless screens ("please wait a moment"),
and cutscenes (the intro, bosses, the ending), each behind a menu, a
language setting or the story, which random presses from rooms don't
open. Each wants a sweep of its own, like Emerald's contests: the
sub-game menu entries, the language variable, the cutscene triggers.

**Screens and languages** (assets/kirby-fuzz-screens, 2026-10-31 on
quack). A fourth sweep, after the abilities, plays what the rooms don't
open, each in the six languages:

- **Screens.** The game leaves a room for game over or the ending
  through one fade, `sub_08020428(callback)`: it fades out, destroys
  every task, resets the heaps and calls the callback. The sweep calls it
  from the first room with a screen's creator: the logo, the title, the
  intro cutscene (`CreateCutscene`), the file select (`sub_08138B44`),
  the main menu as each sub-game leaves it (`sub_08138D64(n)`), the
  ending (`sub_0802B4A8`) and its multiplayer version (`sub_08025A80`),
  game over (`sub_08025AD0`) and the link screen (`sub_08032E98`). A
  creator that takes an argument, the sub-games' `sub_0801A744(n)` (Speed
  Eaters, Crackity Hack, Kirby Wave Ride and a fourth), is faded to a
  no-op and called 30 frames later. Then the first room with START
  pressed (the pause menu), and the collection room (`sub_081434F8`) with
  every bit of `gTreasures` set, browsed by a tap of RIGHT every 20
  frames: its 39 text strips are one per treasure and language. Menus and
  sub-games get a masher's presses (5,400 frames for a sub-game, enough
  to finish one), cutscenes none (a press skips them).
- **The pause help**, each of the 27 abilities given and START pressed:
  its text is `sHelpMenuUnkTiledBGsIndices[language][2 + ability]`, a
  full screen per ability and language.
- **Languages.** `gLanguage` isn't in RAM: it's a halfword constant in
  ROM (0x08D60A80, 1 for English), read wherever text is drawn. The
  worker plays a copy of the ROM with it patched (`Bus::rom` swapped,
  `keep_rom_crc` so the kept states load in either), then puts the
  original back. Items run language-fastest, so with a multiple of six
  workers each needs one copy (16 MB).

17 screens and 27 help items in six languages: 264 items, about 2.9
minutes of the run (the phase ran from 104 to 370 seconds across the
workers). The same `just kirby-fuzz` (12 workers, 45 minutes, 6.38
million frames, 9,752 random runs, 1,273 sweep items, 962 faults):

| minutes | blobs reached | of 1,202 | before (rooms and abilities) |
|---:|---:|---:|---:|
| route phase | 15 | 1.2% | 15 |
| 3 | 581 | 48.3% | |
| 5 | 961 | 80.0% | 439 |
| 7.5 | 1,035 | 86.1% | |
| 15 | 1,038 | 86.4% | 446 |
| 30 | 1,041 | 86.6% | 455 |
| 45 | 1,044 | **86.9%** | 458 (38.1%) |

**458 to 1,044**, nearly all in the sweep's three minutes; fuzzing then
adds 9 in 38 minutes, flat again. Of the 744 missed before, the
tiled-background table `gUnk_082D7850` held 643 (of its 783 entries);
it now misses 82. Two families made most of the gain: the collection
room's strips (entries 0x145 to 0x22E, 234, all reached) and the pause
help (Japanese and the four European languages' 30 screens each). A
first run without them (screens alone) reached 709 (59.0%).

**Still missed**, 158: 82 table entries (each language's last three help
screens, 29 to 31 for Japanese: help slots no ability reaches; scattered
entries from 183 to 324; runs of four from 560 to 687, and 732 to 782:
screens the sweep doesn't open), and 76 blobs outside the table: 44
in `0x0845_0000` to `0x084A_FFFF`, 6 in `0x082C` to `0x082E`, the rest
one to four at a time between `0x0893` and `0x08E9`. Not looked into
further.

**Generic and Emerald-only.**

| | generic (core.rs, the scan) | Emerald only | Kirby only |
|---|---|---|---|
| coverage | instructions, ROM loads, provenance bitmaps of ROM bytes reaching palette, VRAM, OAM | | |
| corpus | kept states, tournament pick, shared corpus and trades, random presses, logs | story seeds (pick weight) | |
| states | route presses; `core::call` to start a game's own function | crafted and mutated saves (`emerald-save`), story saves, a TAS's mGBA states | doors to rooms, abilities |
| sweeps | the worker's share of a list (`index`, `of`) | species, trainers, maps, Pokédex, bag, contests, Frontier: save blocks, `CB2_InitBattle`, scripts in RAM | rooms, abilities, screens through the game's fade, the pause help, languages by a patched ROM constant |
| screen / place | readers a game gives | `gMain.callback2` / map | `stateFn` / room |
| score | the static scan and its curve | pret's file list (`assets-coverage`), `report.rs`'s pret functions | |

Most of Emerald's reach came from its save editor and pret's names;
Kirby's came from three katam addresses. The generic part is the engine;
the reach comes from the game-specific sweeps, and a new game needs its
own few (a door, a level select, a menu index) found in RAM.

## Caveats

- `runtime::prov`'s tags aren't saved in a state, so after a state loads
  RAM bytes keep the tags of whatever ran before in that process. Loads
  straight from ROM (most of the blobs and ranges) are exact; a copy from
  a RAM buffer may credit the wrong ROM bytes now and then.
- Copied ranges split where a screen shows only part of a symbol (a few
  glyphs of a font, some metatiles), so "graphics" counts more ranges
  than images; the symbol counts are the fairer measure.
- "Named as images or palettes" is a name filter on the source map
  (`report.rs`'s `graphic`), not pret's file list.
