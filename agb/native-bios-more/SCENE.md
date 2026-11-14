# native-bios-more: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-bios-more.gba`.
The native lifter's check runs it (`just lift-check`, the
`native-bios-more` table of `crates/codegen/lift-check.toml`, on
`units/native-bios-more-600.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/lift-bios-more`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
makes every call once and draws:

- rows 0-4, column `f % 240`: one pixel a call, its r0-r3 after it
  folded into 15 bits (`colour` in `src/main.rs`), in this order:
  `RegisterRamReset`, `HuffUnComp` 8-bit and 4-bit from ROM, `HuffUnComp`
  8-bit from RAM, `SoundDriverVSyncOff`;
- rows 40-79: the RAM Huffman call's 64 bytes (32 pixels) at row
  `40 + f % 40`, one of four 32-pixel slots across by `f / 40 % 4`; its
  first leaf is `f`'s low byte, so each row's colours differ.

`RegisterRamReset` forces the display blank each frame; the program sets
`DISPCNT` back at once, so no frame shows it. The palette and OAM it
writes and clears aren't drawn in mode 3; the native check compares them
as display memory, and `OUT`'s buffers and the registers as RAM and I/O.

At `f == 150` the program resets EWRAM, VRAM, the palette, OAM, sound and
the other registers and restarts by `SoftReset`, so it runs from `f = 0`
on a cleared screen, about every 151 frames. By frame 600 that has
happened three times: rows 0-4 hold the fourth run's frames so far, and
the right of the screen what agb's start-up leaves in VRAM.

Frame 600's hash is `fa22b5c1c506f81b` in our interpreter at `--timing
exact` and `--timing native` and in mGBA 0.10.5 (`mgba-dump ROM --frames
600 --hash`), 2026-11-12.
