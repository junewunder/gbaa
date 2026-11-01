---
title: Debugging Emerald against mGBA
hook: symbols from the pokeemerald ELF, a stepping harness, and timeline frames
---

- The ROM in `~/ROMs/gba` has the same sha1 (f3ae0881…) as
  `$GBAA_DATA/src/pokeemerald/pokeemerald.gba`, so its ELF and `.map` name
  every address. `nm` is in
  `$GBAA_DATA/toolchains/arm-gnu-toolchain-15.2.rel1-x86_64-arm-none-eabi/bin/`.
- A pc parked at `080008c6` is `WaitForVBlank` (the main loop idling), not a
  hang. `gMain` is at `030022c0` (callback2 at +4, intrCheck at +0x1c).
- To trace, a throwaway `crates/interp/examples/x.rs` can loop over
  `m.take_irq(); m.step()` itself (`Machine::sched`, `idle`, `take_irq` and
  `bus.read*` are public) and log symbol changes or watch a word. Delete it
  after.
- `gbaa-interp ROM --trace ADDR` prints frame, line, cycle, r0-r3 and lr
  each time ADDR runs. The same in mGBA: a throwaway C file in `/tmp` that
  links `$GBAA_DATA/tools/mgba/build/libmgba.a` (as `crates/mgba-dump/build.rs`
  does), loops `ARMRun(core->cpu)` and reads `gba->video.vcount`; the pc is
  `gprs[15]` minus 2 (Thumb) or 4. Tracing `SetGpuReg` (`080010b4`) in both
  found the copyright bug: our BIOS calls took no time.
- To look past a blocker, patch a scratch copy in `/tmp`: the flash check
  in AgbMain is the `beq` at file offset `0x40c` (`02 d0` to `02 e0`).
- mGBA timeline with no input (2026-10-21): copyright 16–150, GAME FREAK
  logo 300–500, bike 1400–2000, Groudon 2400, Rayquaza 3000, title from
  ~3650. Start@300 then A@900 reaches the main menu by 1000; A every 20
  frames from 1100 reaches the truck at ~5375.
- `scripts/debug/mgba-trace.c` is that mGBA tracer (`ROM FRAMES ADDR…`).
  Build: `M=$GBAA_DATA/tools/mgba; cc -O1 -o /tmp/t scripts/debug/mgba-trace.c
  -I$M/src/include -I$M/build/include $(sed 's/^/-D/' $M/build/gbaa-defines)
  -L$M/build -lmgba -lpthread -lm`.
- Lag frames (2026-10-21): with no wait states we run ~2x fast. mGBA enters
  LoadGameSave (`081534d0`) at frame 189 and Task_Scene1_Load (`0816cf18`)
  at 204; we did at 181 and 190, so the forest was 14 frames early (fixed
  by wait states: now 204 as in mGBA).
- When compiled code and the interpreter end a frame at different pcs,
  compare their running (not halted) cycles per frame: the recompiled
  binary prints `cycles: … compiled …, … halted` on stderr, and a scratch
  example looping `m.idle()`/`m.step()` gives the interpreter's. A gap of
  thousands of cycles is a cost counted differently (per-instruction state
  that only the interpreter's tick resets), not IRQ granularity.
- mGBA's alpha blending is off by one 5-bit step in some pixels (2026-10-21):
  our build of it uses 32-bit colour, and `mColorMix5Bit` mixes the widened
  8-bit channels, so the dropped low bits can round up. We mix in 5 bits
  (GBATEK, mGBA's 16-bit build). A frame a few pixels off in a blended layer
  by exactly one step: swap that mix into `ppu/blend.rs` for a run; if the
  hash then equals mGBA's, it's this, and the env file gets MGBA_HASH.
  Darken has its own form (2026-10-22, the main menu): mGBA's `_darken`
  (`software-private.h`) subtracts on the shifted 8-bit channel and masks,
  so green and blue (bits 8+, 16+) lose a step to the borrow; red doesn't.
  A darkened white reads (18,17,17) in mGBA, (18,18,18) in ours.
- mGBA's HLE LZ77 counts its own EWRAM/VRAM accesses through
  `GBAMemoryStall` when the `swi` is in ROM with prefetch on, so each is
  nearly free (2026-10-21). Without that, Emerald's bike scene loaded a
  frame late (from ~1244). Our `decomp::lz77` now does it; a scene that
  lags mGBA by a frame after a load: trace the LZ77 calls in both first.
- Per-line effects (2026-10-22): the PPU now draws each line at its
  H-blank start, before H-blank DMA (mGBA's order); it used to draw the
  whole frame at V-blank, which lost the Groudon scene's BG1HOFS wave. A
  frame whose diffs are swaps between a layer's own colours is a shifted
  layer: log the scroll register per line in mGBA by wrapping
  `gba->video.renderer->drawScanline` (ARMRun skips halted lines).
- A scene a frame late with every load step the same length (2026-10-22):
  diff cycles per instruction. `scripts/debug/mgba-cycles.c` prints `pc
  cycle` per instruction over a span in mGBA; a throwaway example looping
  `m.take_irq(); m.step()` and printing `m.cpu.pc`, `m.now()` gives ours.
  `paste` the two and sum (ours − mGBA's) by pc: the top pcs name the
  instruction mGBA prices differently. Found that way: failed conditions
  (1S), multiplies by operand, `ldm`/`stm` sequential (decision 11). The
  sound mixer (`SoundMainRAM`, IWRAM at `03001aa8`) runs every frame:
  a good first span (`03001aa8` to `ReadKeys` `080005e4`).
- `cargo build --example x` builds only the example: rebuild the bins
  (`-p interp -p desktop`) separately, or traces run the old code.
- A long BIOS call must not tick at once (2026-10-23, Imperium's intro):
  mGBA runs LZ77's cost (`biosStall`, when 18+) in its HLE BIOS's
  `StallCall` loop with the caller's I bit, so IRQs and frame ends come
  inside it. A two-frame LZ77 into VRAM ticked in one step merged two
  V-blank IRQs and made one `run_frame` cover two frames: ours ran a frame
  ahead from then on. Now `bios::swi` parks the CPU at `bios::STALL_LOOP`
  and `bios_step` burns it event by event. Sign of a missed one: mGBA's pc
  sits at `0000034c` (StallCall) for lines where ours doesn't.
- Struct offsets (2026-10-25): the pokeemerald ELF has DWARF, so
  `arm-none-eabi-gdb -batch -nx -ex 'ptype /o struct SaveBlock1' ELF`
  (the toolchain's gdb) prints every field's offset, and `print/d
  MAPSEC_ROUTE_101` an enum's value. Saves are read from where
  `gSaveBlock{1,2}Ptr` (`03005d8c`, `03005d90`) point: the blocks move.
  The cable club's attendants all want `FLAG_SYS_POKEDEX_GET`.
