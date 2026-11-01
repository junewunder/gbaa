---
title: Emerald battles by RAM pokes, and why a move doesn't animate
hook: a foreign OT above level 10 ignores orders (set 0x86E); moves that fail or miss skip their animation
---

- A battle from the overworld: `gEnemyParty`, `gBattleTypeFlags`,
  `gMain.savedCallback` = `CB2_ReturnToField`, `gMain.state` 0, callback2
  = `CB2_InitBattle` (crates/fuzz/src/worker.rs `battle`, `move_battle`).
- The party mons `emerald-save` encodes for the fuzzer have OT name
  FUZZ, not the player's, so `IsMonDisobedient` treats them as traded:
  above the badge level (10 with none) they ignore orders, "falls
  asleep" or hit themselves (`BattleScript_IgnoresAndFallsAsleep`), and
  `gMoveResultFlags` reads 1 as if missed. Setting `FLAG_BADGE08_GET`
  (0x86E, in SaveBlock1's flags) makes any level obey.
- A move's animation (`attackanimation`) is skipped when it misses, fails
  or doesn't affect: heal moves at full HP, Snore and Sleep Talk awake,
  Dream Eater and Nightmare on a waking foe, Attract between the same
  gender, Counter and Mirror Coat unhit, one-hit KOs (30 plus the level
  gap in 100), Curse's Ghost form only from a Ghost. `MoveSetup` in
  worker.rs sets each up.
- 167 of pret's battle-animation files the coverage report counts as
  used belong to an `ANIM_TAG_*` that no script in
  `data/battle_anim_scripts.s` loads (fangs, fury_swipes, substitute...):
  only the ROM's tag table points at them, so no play reaches them.
  The coverage report leaves them out as "unscripted" since
  assets/coverage-unscripted-anim-tags (164 files by its rule).
- RAM: `gBattleMons` 0x02024084 (0x58 each; hp at 0x28, status1 0x4C),
  `gCurrentMove` 0x020241EA, `gMoveResultFlags` 0x0202427C,
  `gBattlescriptCurrInstr` 0x02024214 (names in
  $GBAA_DATA/names/pokeemerald.toml map it to `BattleScript_*`).
