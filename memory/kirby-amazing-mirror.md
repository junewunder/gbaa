---
title: Kirby & the Amazing Mirror: decomp, player struct, scripts
hook: katam matches our ROM; gKirbys at 0x02020EE0; Lua keysRead drives Kirby once stateFn = 0x0803FF65
---

- The katam decomp (github.com/jiangzhengwenjz/katam, shallow clone about 23 MB) builds sha1
  `274b102b…`, our `~/ROMs/gba/Kirby & The Amazing Mirror.gba`. Mostly `sub_XXXXXXXX` names, but
  structs are there: `include/kirby.h` (`struct Kirby`, 0x1A8), `include/data.h` (`ObjectBase`:
  x +0x40, y +0x44, xspeed +0x50, yspeed +0x52), `include/sprite.h` (the Sonic Advance sprite
  engine). RAM symbols are in `linker.ld` (`gKirbys = 0x02020EE0`). It has no symbol file for ROM
  functions: get an address from the `sub_` name, or from `just discover`'s starts next to it.
- Headless with no `--save`, the cartridge is blank. `start@400,start@700,a@800,a@950,a@1100,
  a@1300,a@1500` reaches the first room, and Kirby first stands at frame 1340. Without the later
  A presses, the intro runs on and he appears near frame 2188.
- Kirby ignored the D-pad for over 100 frames after first standing (still the intro); 300 is safe:
  wait that long before scripted moves.
- `emu:setKeys` in a `keysRead` callback drives him reliably and deterministically:
  `scripts/kirby/moves.lua`, `just kirby-assets`. The research is in
  `docs/research/kirby-controller.md`.
- Timing probes (research/why-timing-is-runtime, `scripts/kirby/timing.lua`): `gRngVal` 0x030068D8
  (an LCG, drawn every other frame on the title), `gFrameCount` 0x03002E64 (counted in the V-blank
  handler, so lags don't show in it). A lag frame: `gMainFlags` (0x03002440) bit 24 clear when our
  frame ends (V-blank start); the main loop sets it just before `VBlankIntrWait`. 19 lags from
  reset to frame 4500 on the timing-lab input, all boot and loading.
- Screens and language (assets/kirby-fuzz-screens): `gLanguage` is a ROM halfword at 0x08D60A80
  (1 = English; 0 Japanese, 2 German, 3 French, 4 Spanish, 5 Italian), not RAM: patch a ROM copy.
  `sub_08020428(callback)` fades out, destroys every task, resets the heaps, then calls callback:
  call it between frames to reach any screen (`crates/fuzz/src/kirby.rs`'s SCREENS lists them).
- A screen's state by hand (assets/kirby-fuzz-last-misses): tasks are `gTasks` (0x030019F0, 0x80 of
  0x14 bytes: `structOffset` +6, `main` +8, `flags` +0x12, 0x10 = EWRAM); the struct is at
  0x03000000 + offset (EWRAM: 0x02000000 + offset*4). Most screens keep a state function pointer
  in it (main menu +0x9CC, sub-game menu +0x154, link +0x40): set it to reach a drawing state.
  The link code rewrites `gUnk_02038580` on its own; write it again just before the state.
- Every Kirby is made in `sub_080332BC` (`code_080332BC.c:26`, key 0x080332bd): `CreateKirby` for
  all four, then `sub_0800ECAC` (the AI controller) for each from `numHumans` (1 in story) up.
  Bounding the first loop to `numHumans` and skipping the second leaves one Kirby, and the route
  plays on (native/mirror-cpu-kirbys-off, `docs/research/mirror-cpu-kirbys-off.md`).
