# native-dma: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-dma.gba`. The
stage-A lifter's check runs it (`just lift-check`, the `native-dma` table
of `crates/codegen/lift-check.toml`, on `units/native-dma-600.toml`); what
it exercises is in `src/main.rs`'s header (unit `native/hw-dma-registers`).

Mode 0 with no layer on: the screen is the backdrop colour, which H-blank
DMA (channel 0) sets each line from `LINES`. Line `y + 1` shows `LINES[y]`
for frame `f`: blue `(y + f) % 32`, red `31 - (y + f) % 32`, green `f %
32`, so horizontal bands that cycle from blue to red every 32 lines and
move up a line a frame, the whole picture greening over 32 frames. Line 0
shows what line 159's H-blank left.

Nothing else is shown, but the native check compares it: VRAM's first 256
bytes (`TILES`, an immediate DMA3 word copy from ROM at start), palette
entries 16-31 (`SHADOW`, V-blank DMA2 each frame), `FILL` (an immediate
DMA3 fill each frame), and `COUNTS` (the DMA1 IRQ, one per FIFO A refill,
about 17 a frame; the DMA3 IRQ, one per immediate copy), all in RAM or the
display memories.

| Build | Frame 600 |
|-------|-----------|
| mGBA 0.10.5, our interpreter at `--timing exact` | hash `d8d698342cc0e045` |
| the lift with `dma_set`'s `DMAxCNT` store one more (the planted control) | every transfer one unit longer: the check catches it on frame 1, the fill writing one unit past `FILL` in EWRAM (and H-blank DMA two lines a line, the bands moving twice as fast) |
