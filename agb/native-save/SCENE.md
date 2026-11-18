# native-save: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-save.gba`. The
native lifter's check runs it (`just lift-check`, the `native-save` table
of `crates/codegen/lift-check.toml`, on `units/native-save-600.toml`);
what it exercises is in `src/main.rs`'s header (unit
`native/lift-save-memory`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
stores to SRAM, loads it back and draws one pixel, at row `f / 240 % 4`,
column `f % 240`: everything it loaded (`OUT` in `src/main.rs`) folded
into 15 bits (`colour`). The four rows hold 960 frames, so the picture
first wraps at frame 960: by frame 600, row 0 holds frames 0-239, row 1
frames 240-479 and row 2 up to column 119 frames 480-599; the rest of
row 2, and row 3, are not drawn yet. The
rest of the frame buffer is what agb's start-up left in VRAM, the same
every run. `OUT`'s bytes are also compared as RAM by the native check.

Frame 600's hash is `545740032aa48b22` in our interpreter at `--timing
exact` (2026-11-12). mGBA 0.10.5 (`mgba-dump ROM --frames 600 --hash`)
draws `8e12e78d9070cb55`: it drops a halfword or word store to SRAM at
an address that isn't aligned (`GBAStore16`, `STORE_SRAM`), where the play
build's bus stores the addressed byte lane, so its loads of the lane
stores' bytes differ. With the program's `strh` and `str` taken out our
interpreter draws mGBA's hash exactly.
