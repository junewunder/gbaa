# Recognising Emerald's functions across a compiler change

2026-10-28, quack, unit "Match Emerald functions across agbcc and modern
GCC" (mod/cross-compiler-match). A follow-up to `library-matching.md`,
where byte matching found 74% of an agbcc-built hack's sampled game
functions but 1 of 1,332 in Emerald Rogue and Emerald Imperium, which are
pokeemerald-expansion built with modern GCC. The question: can gbaa
recognise a function after a compiler change, and how often is it wrong?

The test: pokeemerald `731ad5bf` built twice from the same source, with
agbcc (A: byte-identical to Emerald) and with GCC 15.2 (`make modern`, B).
Every B function is matched to an A function or to none without looking
at a name, then scored against both builds' symbols.

## What it shows

- **Precision 97.1%, recall 70.5%.** Of the 12,288 B functions that have a
  counterpart in A, 8,658 are named right. 8,916 pairs are judged (5 more
  pair two unnamed pieces of data) and 258 aren't right by the symbols.
  180 of those are partial (below), leaving 78 plain wrong, so 99.1% of
  pairs name the function or the code inlined into it.
- **Spreading through the call graph does most of the work.** IR
  fingerprints alone find 19.0% at 98.1% precision, held to the strict
  bar that suits them (28.3% at 96.5% at the looser one spreading uses).
  Adding matched callees, callers and RAM variables as tokens, round after
  round, takes that to 65.6% at 97.2%. A BinDiff-style neighbourhood step adds
  4.3 points at 0.5 points of precision, and pruning pairs whose
  neighbours disagree buys most of that back.
- **Byte anchors add little here:** 65 unique byte-identical functions,
  all right: hand-written assembly, which no compiler touches (on Rogue
  they are m4a's and the GameCube multiboot's). A fingerprint finds most
  of them anyway.
- **Big functions match best:** 84% recall at 98.4% precision over 100
  instructions, 69.5% at 96.5% for 16 to 100, and 63% at 98.0% under 16.
  The small ones that match do so mostly through their callers.
- **Inlining shows, but doesn't break it.** 758 of the right pairs differ
  in size by half or more. 2,870 of A's 15,158 functions have no
  counterpart in B (inlined, merged or not discovered). "No match" is the
  right answer for those, and the partial pairs are where the matcher
  landed on one anyway.
- **On Emerald Rogue, Emerald's names are 94.5% right at 59.1% recall,**
  scored against a build of Rogue's source with symbols (below). That
  build, used as the reference itself, names 91% of june's Rogue ROM.

## The source map for the modern build

`just pret-names pokeemerald_modern` now works: 12,398 of its 12,435
entries named exactly (99.7%), and the map loads. It needed three fixes in
`crates/pret-names` and `crates/disc-check`:

- Modern GCC compiles pret's C from a pipe, so every object's `STT_FILE`
  is `<stdin>`, and static functions of one name (three `Cmd_end`) clashed.
  The objects are now named from the linker map beside the ELF
  (`pokeemerald_modern.map`): 308 renamed. Of the 11,951 names unique in
  both builds, the file agrees for 11,944. The other 7 are globals placed
  by the nearest local symbol, and libgcc's `libgcc2.c` against `_muldi3.o`.
- Two copies of data into one RAM buffer gave `[copies]` a duplicate key.
  One row per destination now, with later copies under `also`.
- The FireRed fix (`7c644af6`) filled gaps between ELF segments with 0xFF,
  so `just pret-names` refused Emerald, whose gaps are 0. The check now
  compares only the bytes the segments load (`disc_check::elf::builds`).
  Emerald's and FireRed's maps come out as before.

The modern build itself was already there: `scripts/fetch-pret.sh` runs
`make modern` with GCC 15.2 in the pinned checkout, beside the agbcc
build, which is untouched. So nothing was rebuilt for this test.

## The matcher

`crates/xmatch` (`just xmatch`, `docs/tools/commands.md`). Each side's
functions are codegen's entries in ROM (discovery, then `build_func`), so B
is matched exactly as a ROM with no symbols would be. Names come in only
at scoring.

**Fingerprint.** Built from codegen's IR after const-prop, not from the
printed Rust, and only from what should survive a new compiler:
- hardware addresses named, and loads and stores at a constant hardware
  address with their width;
- BIOS calls;
- ROM data pointed at, by the hash of its first bytes up to 16 or the
  first 0xFF (a Pokémon string's end), with words that look like
  pointers reduced to their region, because data moves with the code;
- constants of 16 or more that aren't addresses;
- loads and stores by width and the region their address is based in
  (EWRAM, IWRAM, hardware, ROM, stack);
- calls through a register, and the number of callees, bucketed.

Registers, stack offsets of spills, instruction order and branch shape are
left out. Two fingerprints are compared by the cosine of their
IDF-weighted token counts, times the size ratio to the power 0.25.

**Stages.**
1. *Bytes:* instruction bytes hashed with call and PC-relative offsets
   masked; equal and unique on both sides, 6 instructions or more.
2. *Fingerprints:* every unmatched pair sharing a rare token (on 60
   functions or fewer) is scored. A pair is taken when each is the other's
   best, the score is 0.7 or more, and it beats both sides' runners-up by
   0.2. This bar is strict because nothing else is known yet. Without
   spreading, the rounds repeat at this bar, each with fewer candidates
   left.
3. *Spreading:* rounds again, with new tokens: each matched callee,
   caller and function pointer by its pair, and each RAM address mapped
   to its counterpart (the address most of its matched users' partners
   load, both ways, two pairs or more agreeing). The bar is 0.5, margin 0.1.
4. *Neighbourhood:* within each matched pair's unmatched callees and
   callers on both sides, the mutual best at 0.5 or more, clear by 0.1.
5. *Prune:* drop pairs where more of their matched neighbours disagree
   than agree, then run the spreading rounds again (a dropped pair may be
   taken again if its neighbours now back it).

**Truth.** A B function's counterpart is the A function of the same name,
by file where a name is on more than one function. A pair is *partial*
when B's name is A's with a GCC suffix (`MPlayOpen.part.0` for
`MPlayOpen`; whole clones such as `.constprop.0` count here too, which
deflates precision slightly), or when A's function has no counterpart in B and is called
by the B function's counterpart: its body was inlined into it
(`HideBg <- HideBgInternal`). Partial pairs count as wrong in the
precision column. Pairs where neither side has a name (discovery took the
same data for code, 5 pairs) are left out.

## Results

`just xmatch score … 40`, A = `pokeemerald.gba`, B = `pokeemerald_modern.gba`
(15,158 and 12,435 functions; 12,288 B functions with a counterpart in A).
Precision / recall by size class of the B function:

| stages | precision | with partial | recall | pairs | small (<16) | mid (16-100) | large (>100) |
|---|---|---|---|---|---|---|---|
| fingerprints | 98.1% | 99.6% | 19.0% | 2,377 | 98.6% / 11.6% | 97.6% / 17.7% | 99.2% / 34.1% |
| bytes + fingerprints | 98.1% | 99.6% | 19.4% | 2,430 | 98.5% / 13.6% | 97.6% / 17.7% | 99.2% / 34.1% |
| fingerprints + spreading | 97.2% | 99.1% | 65.6% | 8,292 | 97.7% / 59.2% | 96.8% / 64.5% | 98.4% / 78.6% |
| bytes + fingerprints + spreading | 97.2% | 99.1% | 65.8% | 8,321 | 97.6% / 59.8% | 96.8% / 64.6% | 98.4% / 78.8% |
| + neighbourhood step | 96.7% | 98.8% | 70.1% | 8,915 | 97.6% / 62.6% | 96.0% / 69.2% | 98.3% / 84.1% |
| + prune (the default) | 97.1% | 99.1% | 70.5% | 8,921 | 98.0% / 63.0% | 96.5% / 69.5% | 98.4% / 84.3% |

The default run, by the stage that found each pair:

| stage | pairs | right | partial | unnamed | precision |
|---|---|---|---|---|---|
| bytes | 70 | 65 | 0 | 5 | 100% |
| fingerprints | 2,177 | 2,146 | 27 | 0 | 98.6% |
| spreading | 4,374 | 4,235 | 96 | 0 | 96.8% |
| neighbourhood | 2,300 | 2,212 | 57 | 0 | 96.2% |

By call-graph agreement (of a pair's matched neighbours, how many are
matched to the partner's neighbours):

| agreement | pairs | precision |
|---|---|---|
| no matched neighbour | 484 | 93.8% |
| agree only | 7,219 | 98.0% |
| some agree, some don't | 1,181 | 93.6% |
| disagree only | 37 | 81.1% |

So agreement is a usable confidence score: a reader could be shown only
the "agree only" names, 7,219 of them at 98.0% (58% recall).

**The bars** (`just xmatch sweep`, a selection; first bar is round 0's,
later bar the spreading rounds'):

| first bar | later bar | neighbourhood | prune | precision | with partial | recall |
|---|---|---|---|---|---|---|
| 0.7 / 0.2 | 0.6 / 0.15 | off | on | 98.2% | 99.6% | 56.9% |
| 0.7 / 0.2 | 0.5 / 0.1 | off | on | 97.5% | 99.4% | 66.0% |
| 0.7 / 0.2 | 0.5 / 0.1 | 0.5 | on | 97.1% | 99.1% | 70.5% |
| 0.7 / 0.2 | 0.5 / 0.1 | 0.3 | on | 96.5% | 98.8% | 73.9% |
| 0.5 / 0.1 | 0.5 / 0.1 | off | off | 97.0% | 99.0% | 65.4% |

Every setting tried lies between 95.9% and 98.2% precision, so the bar
trades recall, not much precision. The default is the third row. The bars
were picked on this same pair of builds, with no held-out pair, so the
numbers above are a little optimistic for other ROMs.

**What the wrong pairs are.** Mostly siblings: functions written side by
side from the same parts (`EraseMoveSelectArrow <- DrawMoveSelectArrow`,
`MoveSelectionDestroyCursorAt <- MoveSelectionCreateCursorAt`,
`GetBoxMonData3 <- GetBoxMonData2`, `StringCopy_PlayerName <-
CopyAndTranslatePlayerName`), where the right partner was inlined away or
matched elsewhere. A sibling's name is close to right for a reader; it is
still counted wrong.

## Emerald Rogue

`just xmatch name …/pokeemerald.gba …/pokeemerald.toml "Emerald Rogue.gba"
out/xmatch/rogue.toml --frames 992` names **6,412 of Rogue's 13,366 functions
(48%)**: 58 by bytes, 1,243 by fingerprints, 3,268 by spreading and 1,845
by the neighbourhood step. Emerald Imperium: 7,583 of 13,765 (55%). The
modern pokeemerald build, from the same source, gets 72%. Rogue's
expansion code is further from vanilla, as expected. The names file loads
as a source map for Rogue.

**The route's first checkpoints** (`units/rogue-newgame.toml`: splash at
frame 100, pret at 500, title at 992, all before any input). From reset to
frame 992, 401 functions are entered and 215 of them are named. Read in
address order, they are the boot path: `InitMainCallbacks`,
`InitIntrHandlers`, `ReadKeys`, `VBlankIntr`; m4a and the sound setup;
flash, RTC and the wireless library's init; save loading
(`CheckForFlashMemory`, `SetSaveBlocksPointers`, `LoadGameSave`); the
copyright screen and intro (`CB2_InitCopyrightScreenAfterBootup`,
`Task_Scene1_Load`, `CreateWaterDrop`, `CreateGameFreakLogoSprites`); and
the title screen (`CB2_InitTitleScreen`, `Task_TitleScreenPhase1` to `3`,
`SpriteCB_VersionBannerLeft`, `StartPokemonLogoShine`). Names that looked
out of place, checked against the code:

- `MapMusicMain`, `TryReceiveLinkBattleData`, `LinkMain2` and
  `Overworld_RecvKeysFromLinkIsRunning` run every frame. pokeemerald's
  main loop and VBlank handler call them (`main.c:155-166`, `:363`), so
  they are right.
- `AddMoney` (run 231 times before the title): Rogue's code XORs a value
  with a save-block key, adds, caps at 999,999 and stores it back
  encrypted. That is `AddMoney` with `GetMoney` and `SetMoney` inlined:
  right, whatever Rogue uses it for at boot.
- `SpriteCB_DigitalDisplay_Stop` (slot machine): Rogue's function steps a
  sprite left to x = 172, starts an animation and plays a sound. That is
  not the slot machine's code: **wrong**, probably a Rogue intro sprite.

**A random sample** (seed 7, six named functions of 10 to 30
instructions), four checked by reading Rogue's Thumb against pokeemerald's
C: `Cmd_playsewithpan`, `ShowBattlePointsWindow`,
`Mailbox_DoGiveMailPokeMenu` and `GetCoordEventScriptAtMapPosition` are
all right, down to the constants (`MAP_OFFSET` 7, the script pointer
stepped by 3). The other two (`AnimWaterBubbleProjectile_Step2`,
`STWI_send_MS_ChangeREQ`) weren't checked.

That's 1 wrong in about 10 names looked at closely, plus a boot path that
reads right. It's a look, not a measurement.

## Rogue with symbols

june asked for Rogue's real precision (unit "Score Rogue's names against a
symbol build of Rogue", mod/rogue-symbols). `just fetch-rogue`
(`scripts/fetch-rogue.sh`) builds Rogue's source, Pokabbie/pokeemerald-rogue
at `a6adfcf1` (branch expansion, EX v2.2.1), release, with GCC 15.2, and
`just rogue-names` makes its source map: 13,362 functions, every one named,
with DWARF. june's ROM says "EX v2.2.1a" (decoded from its text), and no
pushed commit has both: "v2.2.1a" is only on the vanilla branch
(`5ccb74cf`, a fix for berries not being plantable), so the release was
most likely built from an unpushed merge. The build is not byte-identical to
june's ROM, and clean builds are deterministic (`832651b2`, twice; an
earlier build over a half-made tree ordered one level-up move table
differently).

**Emerald's names on Rogue, scored** (`just xmatch score` with Emerald as
A, the Rogue build as B):

| stages | precision | with partial | recall | pairs |
|---|---|---|---|---|
| fingerprints | 97.9% | 99.3% | 12.5% | 1,307 |
| fingerprints + spreading | 94.7% | 96.8% | 52.7% | 5,692 |
| + neighbourhood step | 93.7% | 95.8% | 58.8% | 6,430 |
| + prune (the default) | **94.5%** | 96.6% | **59.1%** | 6,412 |

Recall is out of the 10,232 Rogue functions whose name Emerald also has;
3,130 are Rogue's own (`Rogue_*`, the expansion's battle code), which no
Emerald name can be right for. So the 97.1% of the same-source test drops
to 94.5% where the code has drifted from vanilla. Pairs whose matched
neighbours all agree are 95.7% right (5,260 pairs).

**Rogue's own names on june's ROM.** With the build as the reference
instead (`just xmatch name …/pokeemerald-rogue/pokeemerald.gba
…/pokeemerald-rogue.toml "Emerald Rogue.gba" OUT --frames 992`), **12,188 of the
ROM's 13,366 functions are named (91%)**, 10,753 of them byte-identical
and unique on both sides. Of the 401 functions run before the title, 395
are named. The slot-machine name the Emerald map gave
(`SpriteCB_DigitalDisplay_Stop`, above) is `SpriteCallback_DizzyWalking`
in `expansion_intro.o`: a Rogue intro sprite, as the code suggested.
Where both maps name a function (6,396), they agree on 94.5%, which is
what Emerald's 94.5% precision predicts if the build's names are close to
all right. There's no truth for june's ROM itself, so that's the
evidence.

## Other games

The same matcher with Emerald as the reference, on games that share no
source with it (`just xmatch name`, 2026-10-28). A name is counted
"library" when Emerald's map puts it in m4a, multiboot, flash, libgcc or a
BIOS wrapper's object, and "game" otherwise:

| ROM | functions | named | library | game |
|---|---|---|---|---|
| FireRed (same engine, with symbols) | 11,592 | 7,969 | | |
| Kirby & the Amazing Mirror | 5,606 | 106 | 77 | 29 |
| Kirby: Nightmare in Dream Land | 4,896 | 129 | 71 | 58 |
| The Minish Cap | 7,520 | 128 | 54 | 74 |
| Metroid: Zero Mission | 2,616 | 97 | 34 | 63 |
| WarioWare, Inc. | 5,192 | 90 | 17 | 73 |

- **Library names carry over; game names don't.** On the unrelated games
  the game names are almost all false (`MetatileBehavior_IsPacifidlogHorizontalLogRight`
  in Kirby), save a few pieces of Nintendo's sample code every game has
  (`IntrMain`, `ReadKeys`, `LZDecompressVram`). The bars tuned on
  same-source builds are too loose for a ROM that shares no source: 30 to
  75 confident wrong names a game.
- **FireRed, a different game on Emerald's engine:** 69% of its functions
  named, 81.3% right by pret's names, 84.5% recall of the 7,659 FireRed
  functions whose name Emerald also has. That 81.3% is a floor: most of
  the "wrong" pairs it lists are byte-identical code the two decomps name
  differently (`ChatEntryRoutine_Join <- Chat_Join`).
- So names come from a reference that shares the code: a decomp of the
  same game, an earlier game on its engine, or the libraries. The other
  games here need references of their own.

## Library names

What june wants from a reference in an unrelated game is its libraries,
not its game code (unit "Name only library functions in games unlike the
reference", mod/library-names). `--library` cuts the reference to the
functions in the library objects `crates/xmatch/libraries.toml` lists (m4a,
BIOS wrappers, libgcc, libc, flash, RTC, multiboot, GameCube multiboot,
the wireless adapter's libraries: 347 of Emerald's functions), so nothing
but a library name can come out. A game function that calls the same
helpers can still look like one, so library mode raises the spreading bar
to 0.6 with a 0.15 margin and leaves out the neighbourhood step
(`Config::library`, chosen by `xmatch sweep --library` on the two
modern-compiler pairs below).

Scored where there's truth, Emerald's libraries against:

| B | precision | with partial | recall | pairs |
|---|---|---|---|---|
| FireRed (agbcc, the same SDK) | 100% | 100% | 98.4% of 308 | 303 |
| Rogue build (GCC 15.2) | 97.8% | 100% | 65.2% of 270 | 180 |
| pokeemerald modern (GCC 15.2) | 97.3% | 100% | 66.3% of 273 | 186 |

The partial pairs are a library function the new compiler inlined into
its wrapper (`FadeOutBGM <- m4aMPlayFadeOut`). The games with no shared
source now get library names only:

| ROM | library names | by bytes |
|---|---|---|
| Kirby & the Amazing Mirror | 76 | 51 |
| Kirby: Nightmare in Dream Land | 69 | 48 |
| The Minish Cap | 49 | 21 |
| WarioWare, Inc. | 41 | 36 |
| Metroid: Zero Mission | 40 | 30 |

Their precision isn't measured yet: the decomps of Minish Cap, Metroid and
Kirby & the Amazing Mirror give truth for those three (unit "Bring in
other GBA decomps as library references", mod/decomp-references).

## Other decomps

june's ROMs of three more games are the versions their decomps rebuild, so
`just fetch-decomps` (`scripts/fetch-decomps.sh`) builds each
byte-identical, pinned by commit, with june's ROM copied in as baserom
(unit "Bring in other GBA decomps as library references",
mod/decomp-references). `just decomp-names` makes their source maps, and
they load as codegen's maps for june's ROMs themselves:

| decomp | game | functions named | real names (not `sub_…`) |
|---|---|---|---|
| zeldaret/tmc `6fb6dfb4` | The Minish Cap (USA) | 7,520 of 7,520 | 3,019 |
| metroidret/mzm `7708d105` | Metroid: Zero Mission (USA) | 2,611 of 2,616 | 2,510 |
| jiangzhengwenjz/katam `7d969fbc` | Kirby & the Amazing Mirror (USA) | 5,605 of 5,606 | 457 |

Getting them to build here: tmc's tools need `CMAKE_POLICY_VERSION_MINIMUM=3.5`
(CMake 4) and Homebrew's libpng on `CMAKE_PREFIX_PATH`; mzm and katam use
jiangzhengwenjz's agbcc fork, built beside pret's. mzm's ELF has no
cartridge header (`gbafix` writes it into the ROM after linking), so
`disc_check::elf::builds` now skips the header's 0xC0 bytes.

**Placeholders** (`sub_08001234`, `unk_…`, `nullsub_…`) are a decomp's
names for functions nobody has named. They still help as anchors, but
`xmatch name` never writes one, and truth ignores them, along with any pair
onto one.

**Their libraries.** `libraries.toml` gains each decomp's library objects:
`m4a_asm.o`, `libagbsyscall.o` and `syscalls.o`, `libgcc.o`, `eeprom.o`
and `agb_sram.o` (save libraries Emerald lacks), `multi_boot.o` and
`sio32_multi_load.o`. `xmatch matrix --library` scores every reference's
libraries on every other game with symbols. Strict precision / recall
(the number after "of" is how many functions B has whose name A also has):

| reference → | Emerald | FireRed | Minish Cap | Metroid | Kirby AM | Rogue build |
|---|---|---|---|---|---|---|
| Emerald | | 100% / 98.4% of 308 | 93.9% / 76.7% of 60 | 89.7% / 97.2% of 36 | 94.7% / 92.2% of 77 | 97.8% / 65.2% of 270 |
| Minish Cap | 97.9% / 76.7% | 97.8% / 76.3% | | 85.7% / 66.7% of 9 | 98.0% / 78.1% | 92.3% / 58.1% |
| Metroid | 97.2% / 100% | 97.1% / 97.1% | 85.7% / 75.0% of 8 | | 95.0% / 95.0% of 20 | 100% / 56.2% |
| Kirby AM | 94.7% / 92.2% | 94.4% / 91.9% | 98.1% / 79.7% | 70.4% / 90.5% of 21 | | 93.0% / 56.3% |

Between games the strict figure undercounts. Most "wrong" pairs are the
same code under the two decomps' names: mzm's `SramWrite`/`SramCheck`
for katam's `WriteSram`/`VerifySram`, `MultiBootStartParent` for
`MultiBootStartMaster`, `MPlayStop` for `m4aMPlayStop` (byte-identical).
Others are identical code in two libraries: flash's and SRAM's read and
verify loops are the same bytes, so Emerald's `ReadFlash_Core` names
Kirby's `ReadSram_Core`. The real errors are a few game functions
spreading takes for a library one (`Gibdo <- __negsf2`,
`MiscManager_Type4 <- MultiBootStartProbe`). Turning spreading off
(`--no-spread`) removes those and costs 10 to 35 points of recall, so
it stays on.

**The two games with no decomp**, all five references at once
(`xmatch name --library` with several references, which keeps each
function's strongest claim: byte-identical first, then score):

| ROM | library names | before (Emerald alone) | where from |
|---|---|---|---|
| Kirby: Nightmare in Dream Land | 80 | 69 | 61 from Kirby AM (HAL's same libraries) |
| WarioWare, Inc. | 45 | 41 | spread over all five |

Kirby AM's game names barely reach Nightmare in Dream Land (3 beyond the
libraries: `GetInput`, `LogoInitGraphics`, `LogoClearGraphics`), because
katam's real names are mostly its libraries; the rest are placeholders.

## Data-flow n-grams (tried, dropped)

Unit "Match library versions that differ a little (BSim-style)"
(mod/fuzzy-library-match) tried BSim's idea: tokens over the IR's data
flow, so a library function with a line changed still shares most of
them. Each computing op in a block (arithmetic and logic, shifts,
multiplies, sign extension, select, loads and stores by width) gave two
`Tok::Flow` tokens: its kind with its inputs' kinds (depth 1), and its
kind with its inputs' depth-1 hashes (depth 2). A kind was the op with
its operator, shift type or width; constants were only "under 16" or
not, register reads only "a register"; inputs sorted, flags, register
writes and timing left out. They entered the cosine weighted by a factor
W of their IDF weight.

Measured with `xmatch matrix --library` over the five references and the
Rogue build (the same six ROMs as above). Every setting was chosen on
this same truth; there is no held-out pair. Total recall is the sum over
the 30 cells of functions named right, over the 2,825 with a name on
both sides; the gate was that it rises and no cell loses more than a
point of strict precision.

| W | total recall | worst cell's strict precision change |
|---|---|---|
| 0 (before) | 78.35% | |
| 0.1 | 81.03% | −10.7 (Minish Cap → Metroid) |
| 0.15 | 80.03% | −5.3 (FireRed → Minish Cap) |
| 0.2 | 79.82% | −3.6 (FireRed → Minish Cap) |
| 0.25 | 79.18% | −3.6 (FireRed → Minish Cap) |
| 0.3 | 78.89% | −6.6 (FireRed → Metroid) |
| 0.5 | 75.67% | −6.6 (FireRed → Metroid) |
| 1 | 66.19% | −20.0 (Rogue → Metroid) |
| 0.25, scoring only | 77.32% | −2.3 (FireRed → Metroid) |

(W = 0 gave the before matrix exactly. "Scoring only" kept flow tokens
out of the candidate index, so they could only rescore pairs other tokens
proposed.) Strict precision / recall before, and at W = 0.2, the best
total recall that didn't lower the mean strict precision (94.2% before,
94.3% after):

Before:

| reference → | Emerald | FireRed | Minish Cap | Metroid | Kirby AM | Rogue build |
|---|---|---|---|---|---|---|
| Emerald | | 100% / 98.4% of 308 | 93.9% / 76.7% of 60 | 89.7% / 97.2% of 36 | 94.7% / 92.2% of 77 | 97.8% / 65.2% of 270 |
| FireRed | 100% / 98.4% of 308 | | 93.8% / 76.3% of 59 | 89.5% / 94.4% of 36 | 94.4% / 91.9% of 74 | 97.6% / 63.5% of 260 |
| Minish Cap | 97.9% / 76.7% of 60 | 97.8% / 76.3% of 59 | | 85.7% / 66.7% of 9 | 98.0% / 78.1% of 64 | 92.3% / 58.1% of 62 |
| Metroid | 97.2% / 100% of 35 | 97.1% / 97.1% of 35 | 85.7% / 75.0% of 8 | | 95.0% / 95.0% of 20 | 100% / 56.2% of 16 |
| Kirby AM | 94.7% / 92.2% of 77 | 94.4% / 91.9% of 74 | 98.1% / 79.7% of 64 | 70.4% / 90.5% of 21 | | 93.0% / 56.3% of 71 |
| Rogue build | 97.8% / 67.2% of 265 | 97.1% / 65.1% of 255 | 90.5% / 63.3% of 60 | 100% / 64.3% of 14 | 91.1% / 60.3% of 68 | |

W = 0.2:

| reference → | Emerald | FireRed | Minish Cap | Metroid | Kirby AM | Rogue build |
|---|---|---|---|---|---|---|
| Emerald | | 100% / 98.7% of 308 | 90.4% / 78.3% of 60 | 87.5% / 97.2% of 36 | 93.5% / 93.5% of 77 | 97.7% / 63.0% of 270 |
| FireRed | 100% / 98.7% of 308 | | 90.2% / 78.0% of 59 | 87.2% / 94.4% of 36 | 93.2% / 93.2% of 74 | 97.7% / 65.0% of 260 |
| Minish Cap | 97.9% / 76.7% of 60 | 97.8% / 76.3% of 59 | | 85.7% / 66.7% of 9 | 98.1% / 79.7% of 64 | 93.0% / 64.5% of 62 |
| Metroid | 97.2% / 100% of 35 | 97.1% / 97.1% of 35 | 100% / 75.0% of 8 | | 95.0% / 95.0% of 20 | 100% / 56.2% of 16 |
| Kirby AM | 94.7% / 93.5% of 77 | 94.5% / 93.2% of 74 | 98.1% / 81.2% of 64 | 70.4% / 90.5% of 21 | | 93.6% / 62.0% of 71 |
| Rogue build | 97.8% / 67.5% of 265 | 97.8% / 69.8% of 255 | 89.4% / 70.0% of 60 | 100% / 64.3% of 14 | 92.6% / 73.5% of 68 | |

The gain is real where a compiler changed: the Rogue build as reference
names 13 points more of Kirby AM, 7 more of Minish Cap and 5 more of FireRed, and Kirby
AM names 6 points more of Rogue. But the Pokémon references gain a few pairs
on Minish Cap, Metroid and Kirby AM and lose 1.2 to 3.6 points of strict
precision there (a wrong pair is 2 to 3 points of a 36- to 60-function
cell), and Emerald → Rogue loses 2 points of recall. Flow tokens are
many per function and common across functions, so even at a fifth of
their IDF weight they pull library-shaped game code towards library
functions. No weight passed the gate, so the change was dropped and
xmatch is as before. A next try might count flow tokens only at depth 2
or only in functions of 20 instructions or more, or use them as a
tie-break between candidates the other tokens already score within the
margin, rather than in the cosine.

## A model as judge

Unit "Trial Opus as judge for unmatched library functions"
(mod/opus-library-judge): for the library functions `xmatch name
--library` leaves unnamed, show a model the function's readable print and
the three nearest reference library functions, named, and ask which one it
is, or none. `xmatch judge OUT ROM MAP…` (`crates/xmatch/src/judge.rs`)
takes each of the six ROMs with symbols in turn as the game, names it from
the other five's libraries as `name --library` does, and asks `claude -p`
(through `naming::claude`, as `naming ask` runs it: no tools, safe mode,
an empty folder) at most `--limit` new calls, a rerun asking only what is
missing. The answers are scored strictly against the game's own map.

**Which unnamed functions look like library code.** The nearest
reference function's own-token similarity doesn't tell: at every bar from
0.3 to 0.9 about 1% of the unnamed functions over it carry a name a
reference library has (Emerald: 922 over 0.9, 5 of them). Where they sit
does. Libraries link as runs of objects, so a function within `--gap`
functions, by address, of a named library function on both sides is in
one. At gap 16 that leaves 174 functions across the six ROMs, 170 with a
real name, 123 of them a name some reference library has (of the 132
such names xmatch leaves unnamed); gap 8 gave 122 and 93. The similarity
bar (`--min`, default 0) is then left out.

**The trial**, 2026-10-29: all 170, Opus 5.5 at its default effort, 170
calls (the spend cap was 300), $16.99 list cost, 775 s of calls (4 at a
time), prompts 4 to 53 KB.

| game | asked | name in a reference | true name shown | named | right | same code | precision | recall | "none" right / wrong |
|---|---|---|---|---|---|---|---|---|---|
| Emerald | 19 | 12 | 7 | 9 | 7 | 0 | 77.8% | 58.3% | 7 / 3 |
| FireRed | 20 | 5 | 5 | 6 | 5 | 1 | 83.3% | 100% | 14 / 0 |
| Minish Cap | 19 | 11 | 8 | 10 | 8 | 1 | 80.0% | 72.7% | 7 / 2 |
| Metroid | 3 | 0 | 0 | 2 | 0 | 2 | 0% | - | 1 / 0 |
| Kirby AM | 12 | 7 | 4 | 7 | 4 | 1 | 57.1% | 57.1% | 3 / 2 |
| Rogue build | 97 | 88 | 61 | 72 | 61 | 2 | 84.7% | 69.3% | 6 / 19 |
| all | 170 | 123 | 85 | 106 | 85 | 7 | 80.2% | 69.1% | 38 / 26 |

("Right": the pick's name is the game's own, case aside. "Same code": a
wrong pick whose masked bytes are the function's, another decomp's name for
it. Recall is over the functions whose name a reference library has.)

- **Where the true name is shown, the judge picks it: 85 of 85.** Every
  wrong pick is a function whose own name wasn't among the three. The
  limit is the candidates, not the judge: 38 of the 123 answerable
  functions never had their name shown. Showing more candidates, or
  choosing them by call graph as well as tokens, is where recall is.
- **Of the 21 wrong picks, 7 are the same code** under another decomp's
  name (`SoundBias200` for `SoundBiasSet`, `__gedf2` for `__gtdf2`), and
  more are the same function by another name with a line changed
  (`MPlayFadeOut` for `m4aMPlayFadeOut`, `MPlayContinue`) or GCC's split
  body (`SiiRtcGetStatus.part.0`). The real errors are siblings: flash
  for SRAM or EEPROM (`ReadFlash` for `ReadSram`), one rfu callback for
  another, `ply_xwait` for `ply_xcmd_0C`. Counting the same-code picks
  right, precision is 86.8%.
- **It says "none" when it should** 38 times of 47; 26 times it says
  "none" to a function a reference has, always one whose name wasn't
  shown.
- **The gain**: 85 library names xmatch didn't give, 61 of them on the
  Rogue build (186 named before), for about 10 cents a call.

Four functions' readable prints panicked in codegen and were shown plain
in the trial: each has a block below its entry, where the line in effect
was looked up over a backwards range. Fixed since (unit "Readable print
panics on four library functions", mod/readable-print-btree-panic). Any larger run is june's call: the
naming loop at scale waits on june's naming gate (mod/naming-gate).

## What this means for naming

- **Names for the hacks come almost free.** Half of Rogue and Imperium
  can be named from Emerald's map, at roughly the precision measured here
  where the code is vanilla's. These are the anchors the naming loop
  lacked: callees with names (`naming-sample.md`, round 1 doubles F1 on
  functions whose callees are named).
- **RAM variables come with it.** Spreading maps RAM addresses between
  the builds (gMain, gTasks, the save block pointers) as a side effect.
  Naming the hack's data from those mappings is a small next step, not
  done here.
- **Agreement can gate what a reader sees.** Showing only pairs whose
  neighbours agree gives 98.0% at 58% recall. A "matched from Emerald"
  label on the rest would let a reader weigh them.

## Not done here

- Checkpoints past the title. Those need the route's button presses, and
  `gbaa-interp` takes none yet. route-check's runs could list the
  functions entered.
- Many-to-one matching. A function inlined into several callers is left
  unmatched ("no match"), not matched to each of them. The partial count
  shows how often a pair lands on inlined code instead.
- Strings decoded into tokens, and data names transferred.
