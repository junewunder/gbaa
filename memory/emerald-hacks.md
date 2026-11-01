---
title: Emerald Rogue and Imperium against mGBA
hook: timelines, menus that A doesn't leave, where ours differed, and Rogue's main loop (2026-10-23)
---

- Neither has an ELF or map: both are pokeemerald-expansion builds (modern
  GCC, 32 MB ROMs), so the pokeemerald symbols don't apply.
- mGBA timeline, no input: Rogue's @Pokabbie splash 50–150, PRET x RHH
  ~500, title from ~900. Imperium's copyright (Emerald's exact frame) to
  ~150, PRET x RHH 300–400, then Emerald's intro remade (GAME FREAK 700,
  bike ~2000, legends ~2800), title from ~3900.
- Rogue's new game has menus that A doesn't leave: on the difficulty
  menu and the Player Outfit screen A changes the setting; press Down to
  Save & Exit, then A (the route in `units/rogue-newgame.toml`). Imperium needs only
  A (`units/imperium-newgame.toml`); its truck is Emerald's exact frame.
- From reset our interpreter reached every checkpoint screen (Rogue's hub
  at 6000, Imperium's truck at 6800, both mGBA's hash) with no BIOS fault.
  Where it differed: menus' unselected boxes (darken rounding, as on
  Emerald), text a letter ahead and intro scenes shifted (timing).
- Rogue's title (2026-10-23): the stars are BG1 alpha-blended over BG0
  (eva 10, evb 16), so mGBA's 8-bit mix quirk (emerald-debugging.md) moves
  248 pixels: mgba_hash on the title checkpoint in `units/rogue-newgame.toml`. Also, from reset ours
  runs one frame ahead of mGBA from frame 707 (the PRET x RHH fade-out; 690–706
  match) through at least 1106; from mGBA's state at 795 we stay in sync, so
  the lead is a lag frame near 700.
- Imperium's route passed whole on 2026-10-23 once the LZ77 stall became
  interruptible (emerald-debugging.md): ours had run a frame ahead from 476,
  the same PRET x RHH fade-out. Rogue's lead was the same: at frame 692 a
  ~575k-cycle `swi 0x12` into VRAM (from `082f808c`) spans two V-blanks.
  With the stall, Rogue's route passes whole (2026-10-23); speech, outfit,
  naming and hub needed nothing else. Main menu and difficulty differ from
  mGBA only by its darken rounding (mgba_hash on both).
- Rogue's main loop is at `080004ce` (a pass a frame), `gMain` at
  `030014b4` (callback2 at +4): tracing it in both (scripts/debug/mgba-trace.c)
  shows which frames mGBA lags.
