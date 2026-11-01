# diff-unfilter: expected frame

`just agb-tests` builds this into `$GBAA_DATA/agb/diff-unfilter.gba`. The
scene is set up once before the first VBlank and never changes; check it
at frame 10 or later (`just unit diff-unfilter` does, from
`units/diff-unfilter.toml`):

    just mgba-diff "$GBAA_DATA/agb/diff-unfilter.gba" 10

Mode 0, BG0 only (4bpp, character base 0, screen block 31). Its palette,
tiles and the map's top-left 8x8 entries come out of the BIOS's unfilter
calls, from delta-filtered data in ROM (`src/main.rs`):

- `Diff16bitUnFilter` (swi 0x18) writes BG colour i (0-15) to palette
  RAM: red 2i, green 31 - 2i, blue 5i mod 32 (BGR555). Colour 0 is the
  backdrop, `0x03E0`, which is also what transparent pixels show.
- `Diff8bitUnFilterVram` (swi 0x17) writes tiles 0-3 to VRAM: byte i
  (tile i / 32, row i / 4 % 8) holds colours `c & 15` (left pixel, low
  nibble) and `(c + 1) & 15`, where c = i + i / 4.
- `Diff8bitUnFilterWram` (swi 0x16) writes 64 bytes to EWRAM: map entry
  (row r, column k), r, k < 8, is tile `(7(8r + k) + r) % 4`. Every other
  entry is tile 0.

So pixel (x, y) is: n = the tile at map entry (y / 8, x / 8), i =
32n + 4(y % 8) + (x % 8) / 2, c = i + i / 4, and colour `(c + x % 2) & 15`.
mGBA's frame matches that formula at every pixel (2026-10-26). Some of them:

| Pixel (x, y) | Tile | Colour | BGR555   |
|--------------|------|--------|----------|
| (0, 0)       | 0    | 0      | `0x03E0` |
| (1, 0)       | 0    | 1      | `0x17A2` |
| (7, 7)       | 0    | 7      | `0x0E2E` |
| (8, 0)       | 3    | 8      | `0x21F0` |
| (9, 3)       | 3    | 8      | `0x21F0` |
| (16, 0)      | 2    | 0      | `0x03E0` |
| (63, 63)     | 0    | 7      | `0x0E2E` |
| (100, 100)   | 0    | 6      | `0x7A6C` |
| (239, 159)   | 0    | 7      | `0x0E2E` |
