# Naming a sample of Emerald, scored against pret

The mod view's naming loop (`docs/mod-view-design.md`, "The naming
loop"), run once on its capped sample: 300 Emerald functions, three
rounds and two of the three leak controls. `crates/naming` drew the
sample, built every prompt and scored the answers; an agent
(`claude -p`, Sonnet 5.5, no tools, safe mode, run from an empty folder)
named each function from its readable print with every pret name hidden.
Quack, 2026-10-27, branch `unit/mod/naming-sample` off main `f7ca3a6`.
The answers, prompts and `names.toml` are in `work/mod-naming/s1/` on
quack; the report below is `naming score`'s, as written.

## What it shows

- **The code is read, not remembered.** Round 0 scores subtoken F1
  0.155, inside REBENCH's leak-free 0.1-0.4. With only "Pokémon Emerald
  (U), function at ADDR, N instructions" (the no-code control) it scores
  0.076, half that, and 0.7% exact. So memory of pokeemerald gives little
  from an address, and Emerald's score is close to what an unnamed game
  would get, not an inflated upper bound.
- **Recognition is rare but real:** 2 of 10 trials of three prints named
  a gen-3 Pokémon game (one Emerald, from `gMain`'s address and its
  callback check; one Ruby/Sapphire, from the `gSprites` and `gTasks`
  strides); 8 said unknown. The leak goes through data addresses and
  struct strides, not the code.
- **Bottom-up passes names upward.** Round 1 lifts F1 from 0.155 to
  0.222 overall; on the 83 subtree functions with sampled callees, from
  0.147 to 0.292 (twice). Round 2 (callers too) adds little: 0.224
  overall, 0.287 on the subtree functions. Exact match stays low
  (2.7% to 3.7%), as pret's own decomps share only 28-53% of names.
- **Mid-sized functions name best** (F1 0.32 in rounds 1 and 2). Small
  ones (0.17) are mostly thunks and setters whose meaning is in a callee
  or datum we don't name; large ones (0.19) lose the main purpose in
  machine detail.
- **Data** by vote: F1 0.149 in round 0, 0.169 in round 2, over the 377
  data pret names at the exact address.
- **Cost:** about 11,500 tokens in and 220-270 out per call (the large
  class is most of the input: its prints run to ~100 KB), 5.5 s each at 6
  at a time; $14 a round, about $42 for the three rounds and $3.3 for
  the no-code control. All 1,210 calls answered; one answer needed the
  last of two TOML blocks (the agent corrected itself), which the parser
  now takes.
- **The FireRed control finds no remembering** (below, "FireRed
  control"): 0 of 30 FireRed functions whose code is Emerald's under
  another name were answered with Emerald's name, nor with FireRed's.
- **Not done here:** strings were not decoded into prompts. Locals are
  left out, as the design says.

## FireRed control

The design's third leak control, run 2026-10-28 on quack (unit FireRed
leak control for the naming sample). `just fetch-pret` builds pret's
pokefirered at `ce6406d` (SHA1 FireRed (U) 1.0's); `just pret-names
pokefirered` names 11,582 of our 11,592 entries. `naming sibling` then
drew 30 of the 591 FireRed functions whose code (8+ instructions, call
offsets masked, unique in each game) is an Emerald function's under
another pret name (4 small, 26 mid), and round 0 asked each with
Emerald's prompt:

```
just naming sibling $GBAA_DATA/src/pokefirered/pokefirered.gba \
    $GBAA_DATA/names/pokefirered.toml $GBAA_DATA/src/pokeemerald/pokeemerald.gba \
    $GBAA_DATA/names/pokeemerald.toml work/mod-naming/firered   # ~40 s
just naming ask $GBAA_DATA/src/pokefirered/pokefirered.gba work/mod-naming/firered 0
just naming score work/mod-naming/firered
```

| round | n | Emerald's name | FireRed's name | F1 FireRed | F1 Emerald | closer to Emerald's |
|---|---|---|---|---|---|---|
| 0 | 30 | 0 | 0 | 0.102 | 0.099 | 5 |

No answer gave either game's name; F1 against the two names is the
same, so nothing pulls answers toward the sibling's. Round-0 F1 0.102
is below Emerald's 0.155 (mid 0.079 against 0.225), from 30 functions
only; pokefirered's names are older and less descriptive in places
(`oac_poke_ally_`, `Q_8_8_mul`, `LavaridgeGym1FWarpEffect_3`), and the
answers read generic (`CallIndexedHandlerU8`, `UpdateStateMachineStep`). 30
calls, $0.53, 5.1 s each. Answers and report: `work/mod-naming/firered/`
on quack.

## Cheaper models and effort

Run 2026-10-28 on quack, on june's subscription, to see what naming costs
per function: the same 300 functions, rounds 0 and 1, with
`naming ask … --effort low`. The first row is this report's run
(Sonnet 5.5 at its default effort).

```
just naming ask ROM work/mod-naming/s1-haiku 0 --model claude-haiku-5-5 --effort low
just naming ask ROM work/mod-naming/s1-haiku 1 --model claude-haiku-5-5 --effort low
just naming ask ROM work/mod-naming/s1-sonnet-low 0 --effort low   # and 1
```

| model, effort | F1 r0 | F1 r1 | subtree r1 | data r1 | tokens in / out | ¢ a call (list) |
|---|---|---|---|---|---|---|
| Sonnet 5.5, default | 0.155 | 0.222 | 0.292 | 0.149 | 11,500 / 245 | 4.7 |
| Sonnet 5.5, low | 0.133 | 0.214 | 0.275 | 0.140 | 8,070 / 180 | 3.4 |
| Haiku 5.5, low | 0.074 | 0.084 | 0.085 | 0.060 | 8,090 / 480 | 0.26 |

- **Sonnet at low effort keeps the round-1 score** (0.214 against
  0.222; mid-sized functions 0.312) for 70% of the cost.
- **Haiku doesn't read the code.** Its round 0 (0.074) is the no-code
  control's score (0.076), and bottom-up adds almost nothing. Its names
  are fluent but generic (`AllocSlot` for `CreateTask`, where Sonnet low
  gives `CreateTask`). It is 13 times cheaper and not usable for naming.
- Dollars are `claude -p`'s list-price figure, which bills cache writes;
  the subscription's own meter is what says whether a run is sustainable.
  Answers and reports: `work/mod-naming/s1-haiku/` and `s1-sonnet-low/`
  on quack.

### Judged by hand

A blind judge (an Opus subagent) rated 30 of the sample's functions
(13 small, 9 mid, 8 large; seeded). Each function was shown as pret's C
source with its own name hidden, beside three shuffled, unlabelled names:
pret's, and round 1's from Sonnet low and Haiku low. The scale: 3, a
reader would predict what it does and why; 2, right idea; 1, true of the
mechanics only; 0, wrong or misleading.

| names | mean | 3 | 2 | 1 | 0 |
|---|---|---|---|---|---|
| pret | 2.80 | 26 | 3 | 0 | 1 |
| Sonnet 5.5, low | 0.77 | 0 | 1 | 21 | 8 |
| Haiku 5.5, low | 0.70 | 0 | 3 | 15 | 12 |

- **Neither model names a function's purpose.** Sonnet's names describe
  the machine code (`CallWithArg4ReturnU8` for `PlayerHasBerries`,
  `RunStateHandlerStep` for `LoadPyramidBagMenu`), and 8 of 30 are
  misleading (`ConvertKanaCharToSortKey` for `GetSSTidalLocation`).
  The one "right idea" is `RemoveItemFromPocket` for
  `RemovePyramidBagItem`.
- **The meaning isn't in what the prompt shows.** `PlayerHasBerries` is
  `return IsBagPocketNonEmpty(POCKET_BERRIES)`; with its callee unnamed,
  the print is a call with the argument 4. Bottom-up doesn't help when the
  leaves are named by their mechanics too.
- The judge saw pret's code, which uses pret's names for callees and
  globals, so pret's candidate had an edge; that doesn't change 0 of 30.
  Sheet, key and ratings: `work/mod-naming/judge/` on quack.

### An agent with tools remembers instead

An Opus agent with tools named 30 sampled functions that run on the
new-game route (`units/emerald-newgame.toml`; 70 of the 300 do): 15
game-level, 15 engine. It had the prints, the game's charset, the ROM,
and headless runs with Lua breakpoints, argument logs and screenshots;
no pret files and no web, and it was told to name only from evidence
and to record it. 9 minutes, about 190k tokens.

- **18 of 30 names are pret's exactly**, idiosyncratic ones included
  (`CreatePokeballSpriteToReleaseMon`, `ApplyNewEncryptionKeyToGameStats`,
  `MetatileBehavior_IsNorthArrowWarp`), where pret's own decomps share
  28-53%. The other 12 use pret's coinages (`ObjectEvent`, `CB2_`,
  `Task_`, `SaveBlock1`). Text and screens tell it the game, and then it
  recalls the decomp; its evidence is plausible but can't show it reasoned
  rather than recalled.
- So **Emerald can't measure a tool-using agent.** The one-shot rounds
  above stay honest only because the bare prints hide the game. Not
  judged by hand, since the judge would score memory.
- Its own notes on what decided names: hardware registers for engine
  code, the frame a function first runs set against screenshots for
  scene code, decoded text once (the Trainer Hill timer). It read the
  ROM through objdump rather than our prints, which it found too verbose.
  Answers and scripts: `work/mod-naming/opus-tools/` on quack.

## How it was run

```
just naming prepare '"$HOME/ROMs/gba/Pokemon - Emerald Version.gba"' \
    $GBAA_DATA/names/pokeemerald.toml work/mod-naming/s1      # ~20 s
just naming ask '"…Emerald….gba"' work/mod-naming/s1 0          # then 1, 2, nocode, recog
just naming score work/mod-naming/s1
```

The sample (seed 20261027): 300 of 14,882 candidates, 100 per size class
(under 16 instructions, 16-100, over 100); 30 whole subtrees of 3 to 12
functions hold 130 of them, the rest drawn at random. Left out: libgcc,
m4a and the SDK's libraries (`lib*`, `agb_flash*`, `siirtc`, `crt0`),
whose 262 names every print is given, as a library matcher would give
them in any game. Each prompt has the print (callees by the agent's name
or `f_<key>`, pool data as `addr!(d_<addr>)` or the voted name), the
callees with the agent's names and one-line summaries where the round
allows, the callers, and each datum with our size estimate (the gap to
the next pool target).

## The report

### Functions

Subtoken precision, recall and F1 (macro, per function; a missing or unparsed answer scores 0) and exact match, case-insensitive. REBENCH's leak-free token F1, 0.1-0.4, is the lower reference.

| round | class | n | precision | recall | F1 | exact |
|---|---|---|---|---|---|---|
| 0 | all | 300 | 0.148 | 0.162 | 0.155 | 2.7% |
| 0 | small (<16) | 100 | 0.109 | 0.116 | 0.113 | 3.0% |
| 0 | mid (16-100) | 100 | 0.213 | 0.239 | 0.225 | 5.0% |
| 0 | large (>100) | 100 | 0.123 | 0.132 | 0.127 | 0.0% |
| 1 | all | 300 | 0.217 | 0.228 | 0.222 | 3.7% |
| 1 | small (<16) | 100 | 0.150 | 0.162 | 0.156 | 2.0% |
| 1 | mid (16-100) | 100 | 0.316 | 0.333 | 0.324 | 9.0% |
| 1 | large (>100) | 100 | 0.187 | 0.188 | 0.187 | 0.0% |
| 2 | all | 300 | 0.218 | 0.230 | 0.224 | 3.7% |
| 2 | small (<16) | 100 | 0.161 | 0.180 | 0.170 | 2.0% |
| 2 | mid (16-100) | 100 | 0.299 | 0.333 | 0.315 | 9.0% |
| 2 | large (>100) | 100 | 0.193 | 0.178 | 0.185 | 0.0% |
| nocode | all | 300 | 0.076 | 0.076 | 0.076 | 0.7% |
| nocode | small (<16) | 100 | 0.088 | 0.086 | 0.087 | 1.0% |
| nocode | mid (16-100) | 100 | 0.079 | 0.081 | 0.080 | 0.0% |
| nocode | large (>100) | 100 | 0.059 | 0.063 | 0.061 | 1.0% |

#### Subtree functions with sampled callees (83)

Where bottom-up rounds can pass names upward.

| round | F1 | exact |
|---|---|---|
| 0 | 0.147 | 0.0% |
| 1 | 0.292 | 4.8% |
| 2 | 0.287 | 3.6% |

### Data

Each datum's name is the vote of the round's answers that touch it; scored where pret names that exact address, scope prefixes (g, s) dropped.

| round | n | precision | recall | F1 | exact |
|---|---|---|---|---|---|
| 0 | 377 | 0.166 | 0.136 | 0.149 | 2.9% |
| 1 | 377 | 0.177 | 0.151 | 0.163 | 4.0% |
| 2 | 377 | 0.182 | 0.158 | 0.169 | 4.8% |

### Cost

Per call, from each call's usage; tokens in count the cached prompt.

| round | calls | tokens in | tokens out | seconds | dollars (list) |
|---|---|---|---|---|---|
| 0 | 300 | 11391 | 266 | 5.6 | 14.47 |
| 1 | 300 | 11504 | 224 | 5.5 | 13.76 |
| 2 | 300 | 11605 | 218 | 5.3 | 13.48 |
| nocode | 300 | 674 | 833 | 10.7 | 3.31 |
| recog | 10 | 13056 | 501 | 9.6 | 0.57 |

Answers that didn't parse: 0: 0, 1: 0, 2: 0, nocode: 0.

### Recognition control

Three round-0 prints a trial; which game?

| trial | game | why |
|---|---|---|
| 0 | unknown | The code is generic (a word-copy memcpy, a tiny EWRAM-pointer setter, a thunk wrapper), with no strings or hardware constants. The ROM extends past 0x082e0000, so it is a large cartridge, but that does not pin down a title. |
| 1 | unknown | Nothing here names a game: the code is generic Thumb (a CpuSet fill wrapper with 0x05000000, a byte flag-set helper, and a 16-bit value lookup), and the data reference at 0x085eea42 only shows the ROM is at least 6 MB. |
| 2 | unknown | Nothing in these functions names a game: they are generic Thumb code (a byte-fill loop, a struct initializer, and a routine that writes BG scroll and window registers through a helper at 0x080010b4 from EWRAM shadow values), and the ROM is over 1 MB, so I can't pin down a title. |
| 3 | unknown | The code is generic Thumb with pop {r1}; bx r1 returns and an EWRAM global at 0x02038c08, but it has no strings, hardware registers or distinctive constants to identify a title. |
| 4 | unknown | Nothing here names a game: only generic Thumb code, EWRAM/IWRAM addresses and a jump table, with a ~1.4MB code layout that suggests a mid-size licensed title; any specific title would be a guess. |
| 5 | unknown | Nothing here names a title: the code uses the old Thumb interworking return (pop {r0}; bx r0) and keeps state in EWRAM at 0x0203cf2c, which fits many early-era GBA games, so I can't pick one. |
| 6 | unknown | Nothing identifies the game: the code is generic Thumb bitfield and table code, with an allocator-style call near 0x080009b8 and a fill helper at 0x082e7084. The data table at 0x0850e644 suggests a ROM of 8MB or more, but that is not enough to pick a title. |
| 7 | Pokémon Emerald | d_030022c0 matches gMain in Emerald, and the first function compares its callback1 field to a ROM function pointer (a standard Pokémon callback check). The pop-{r1}/bx-r1 returns and big-ROM layout also fit the agbcc-built gen-3 Pokémon engine. |
| 8 | Pokémon Ruby / Sapphire (Gen III) | d_02020630 is a 68-byte-stride sprite array (gSprites in Ruby/Sapphire) and d_03005e00 is a 16-entry, 40-byte-stride task table (gTasks). f_080a91e5 is a task-lookup loop that searches by function pointer. The exact Ruby vs Sapphire version can't be told from these functions. |
| 9 | unknown | Nothing here identifies a title: there are no strings or recognisable constants, only generic Thumb code (a byte-copy loop into EWRAM buffers around 0x02023064/0x02024064, and bx-return veneers) in a ROM larger than 2MB. |

### Side by side

20 functions, drawn with the sample's seed.

| key | class | pret | round 0 | round 1 | round 2 | no code |
|---|---|---|---|---|---|---|
| 0x0800a469 | mid | GetMultiplayerId | GetLinkPlayerId | GetLinkPlayerId | GetLinkPlayerIdOrCallbackState | FreeSpritePalette |
| 0x08034519 | small | LZDecompressWram | CallHelperThunk | LZ77UnCompWramThunk | LZ77UnCompWramThunk | SetControllerToSafari |
| 0x08035ae5 | large | DrawLinkBattleParticipantPokeballs | DrawPackedTileIndicators | BuildTaskTilemapEntriesAndDraw | BattleIntroDrawBattlerIconTilemap | BattleIntroGetMonsData |
| 0x08082ad5 | large | ProcessLinkPlayerCmds | UpdateCheatCodeEntry | ProcessLinkEntryResponse | ProcessLinkReceivedEntry | PlayerNotOnBikeMoving |
| 0x08093ab9 | mid | UpdateMovementNormal | TrySetPendingFlags | StepSequenceAndUpdateHistory | StepSequenceAndUpdateHistory | ObjectEventSetSingleMovement |
| 0x08093be5 | small | MovementAction_WalkSlowDiagonalUpLeft_Step1 | TrySetStateOnCheck | TrySetStateOnCheck | TryStepAndSetState | ObjectEventClearHeldMovement |
| 0x08096d01 | mid | UpdateObjectEventElevationAndPriority | UpdateSpriteGfxFlagsFromTable | ObjectEventUpdateSpriteElevationPriority | UpdateObjectEventSpriteSubpriorityAndPriority | UpdateObjectEventSpriteVisibility |
| 0x0809d649 | mid | GetVarPointer | ResolveHandle | ResolveHandlePtr | ResolveHandlePtr | UpdateCardFlipRegs |
| 0x080a8fb1 | mid | CreateTask | AllocateSlot | CreateTask | CreateTask | UpdateBlenderBerrySprite |
| 0x080dfd95 | large | CB2_InitBuyMenu | RunStateInitStep | InitTitleScreenOrFadeOut | SceneStateMachineStep | BattleAnimSpriteCallback |
| 0x080e08f1 | large | BuyMenuDrawObjectEvents | UpdateMapObjectSprites | UpdateEntitySpriteSlots | UpdateEntrySpritesLoop | SortContestants |
| 0x080f7a81 | large | UpdateContestResultBars | SetupFadeColorStepsForPalettes | StartPaletteFadeTasks | CreateInterpolationTasks | Task_HandleInput |
| 0x081029b5 | large | AnimTask_DoubleTeam | InitMenuCursorSprites | InitSpriteFadeTask | TaskInitSpriteFadeEffect | LilycoveLady_SetUpQuiz |
| 0x0811eaa5 | mid | IsEasyChatWordInvalid | IsInvalidCharCode | IsCharInCategory | IsCharInCategorySet | SlotMachine_DisableReelTimer |
| 0x0811f961 | large | BufferMenuFrameTiles | PushBufferedSnapshot | SnapshotSaveBlockCopy | SnapshotRestoreNextEntry | InitPartyMenu |
| 0x08121e69 | small | SetVBlankHBlankCallbacksToNull | CallHooksThenReturnViaPoppedPointer | CallStubsWithZeroArgAndReturnViaPoppedPointer | CallThunksAndReturnViaStack | SetUpFieldMove_SweetScent |
| 0x0812c3c9 | mid | DecideStop_Bias_Reel2_Bet1or2 | FindMatchingSlotIndex | FindMatchingWrappedLookupIndex | FindMatchingWrappedLookupSlot | Task_LinkContest_Wait |
| 0x08133d8d | small | TraderShowDecorationMenu | CallWithDefaultHandler | SpawnTaskAndReturn | StartTaskWithDefaultPriority | GetBattlePyramidTrainerFlag |
| 0x08137a99 | large | GetPokedexRatingText | GetTextForIdRange | GetTextForValueRange | GetTextRangeByIndex | DrawExperienceProgressBar |
| 0x08146ec1 | mid | VBlankCB_SetWinAndBlend | ResetWindowHdmaAndApplyRegs | SetupWindowScanlineEffect | SetupWindowScanlineEffect | SetFrontierBrainTrainerFlag |
