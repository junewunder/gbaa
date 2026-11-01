---
title: mGBA's blending quirks (windows and sprites)
hook: with the OBJ window on, mGBA brightens non-target sprites; semi sprites ignore the window's effect bit
---

Found building `agb/windows-blend` (mGBA 0.10.5 source is in
`$GBAA_DATA/tools/mgba/src/`, `src/gba/renderers/software-obj.c` and
`video-software.c`):

- With `DISPCNT` bit 15 (OBJ window) on and BLDCNT brighten or darken,
  mGBA brightens every normal sprite outside the OBJ window even when
  BLDCNT bit 4 (sprites a first target) is clear. With the OBJ window off it
  doesn't. GBATEK says it shouldn't; we follow GBATEK. A frame that hits
  this needs `MGBA_HASH`/`MGBA_WHY` in its unit env.
- A semi-transparent sprite over a second target alpha-blends even where
  the window clears its effects bit (`PreprocessSprite` forces
  `FLAG_TARGET_1` regardless of the window). We match that.
- Quick probe of mGBA on a variant ROM: `target/release/mgba-dump ROM
  --frames 10 --dump F.png`, then `magick F.png -format "%[pixel:p{x,y}]" info:`.
