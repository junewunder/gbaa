# mosaic-modes: expected frame

`just agb-tests` builds this into `$GBAA_DATA/agb/mosaic-modes.gba`. The
scene is set up once before the first VBlank and never changes; check it
at frame 10 or later (`just unit mosaic-modes` does, from
`units/mosaic-modes.env`):

    just first-frame "$GBAA_DATA/agb/mosaic-modes.gba" 10
    just mgba-diff "$GBAA_DATA/agb/mosaic-modes.gba" 10

Mode 1: text BG0 and BG1, affine BG2, sprites on, 1D OBJ mapping.
`MOSAIC` = `0x5324`: background blocks 5 wide and 3 tall, sprite blocks
4 wide and 6 tall, all lined up with the screen's (0, 0). Colours are
BGR555.

- Backdrop grey (`0x4210`).
- BG tiles are 8bpp, every texel its own colour: background k's (0, 1,
  2) texel (c, r) is red 4c, green 4r, blue 6 + 6k.
- BG0 (priority 0, **mosaic**): 256×256, scrolled by (3, 5). Tile (tx,
  ty) is drawn where tx + ty is a multiple of 4, flipped horizontally in
  odd columns. Screen (x, y) shows the map at (xs + 3, ys + 5), where
  xs = x - x % 5 and ys = y - y % 3.
- BG1 (priority 1, no mosaic): scrolled by (7, 2). Tile in rows
  ty % 3 = 1, columns tx % 4 = 0, and tx % 4 = 2 flipped vertically.
- BG2 (priority 2, **mosaic**): affine 128×128, wrapping, tile on a
  checker (tx + ty even). Matrix (222, 128, -128, 222), X = -1000,
  Y = 3000 (/256). Screen (x, y) shows map
  `((222·xs + 128·ys - 1000) >> 8, (-128·xs + 222·ys + 3000) >> 8)`,
  wrapped to 0-127.
- Sprites, all 16×16 8bpp: texel (c, r) is OBJ index (1 + c + 16r) & 255
  (the last texel 0, transparent); index i is red 2 (i % 16), green
  2 (i / 16), blue full.
  - 0: **mosaic** at (21, 13), off the block grid, priority 0. On line y
    it shows row max(0, y - 13 - y % 6); at x, column max(0, x - x % 4 -
    21). Its last block (x 36-39) runs past its right edge to x 39.
  - 1: **mosaic**, flipped both ways, at (60, 36), priority 1: behind
    BG0, in front of BG1 and BG2.
  - 2: no mosaic at (100, 50).
  - 3: affine, double size (32×32 box at (140, 60)), 45 degrees
    (181, 181, -181, 181).
  - 4: affine, **mosaic**, 90 degrees (0, 256, -256, 0), at (180, 96),
    a box on the block grid: box pixel (bx, by) shows box pixel
    (bx - bx % 4, by - by % 6)'s texel, `(dy + 8, -dx + 8)` from the
    box's centre.

| Pixel (x, y)   | Colour   | Why                                              |
|----------------|----------|--------------------------------------------------|
| (0, 0)         | `0x1A8C` | BG0 map (3, 5): tile (0, 0), texel (3, 5)        |
| (4, 2)         | `0x1A8C` | same BG0 block as (0, 0)                         |
| (5, 0)         | `0x4210` | BG0 tile (1, 0) empty; BG2 map (0, 9) empty      |
| (10, 0)        | `0x4B10` | BG2 map (4, 6): tile (0, 0), texel (4, 6)        |
| (14, 2)        | `0x4B10` | same BG2 block as (10, 0)                        |
| (14, 3)        | `0x4210` | next block down: BG2 map (6, 9), empty tile      |
| (30, 10)       | `0x3214` | BG1 map (37, 12): texel (5, 4)                   |
| (12, 10)       | `0x318C` | BG1 map (19, 12), flipped: texel (3, 3)          |
| (21, 13)       | `0x7C02` | sprite 0 texel (0, 0): block starts left of it   |
| (30, 20)       | `0x7D50` | sprite 0: block (28, 18), texel (7, 5)           |
| (37, 13)       | `0x7C40` | sprite 0's last block past its edge: texel (15, 0) |
| (60, 36)       | `0x1880` | BG0 (tile (7, 5), flipped) over sprite 1         |
| (75, 51)       | `0x1804` | BG0 (tile (9, 7), flipped) over sprite 1         |
| (66, 40)       | `0x7FD8` | sprite 1: block (64, 36), texel (11, 15) flipped |
| (101, 52)      | `0x7C84` | sprite 2 texel (1, 2)                            |
| (156, 76)      | `0x7E12` | sprite 3's centre: texel (8, 8)                  |
| (180, 96)      | `0x4210` | sprite 4 texel (0, 16): outside; nothing behind  |
| (188, 100)     | `0x7E02` | sprite 4: block (188, 96), texel (0, 8)          |
| (191, 101)     | `0x7E02` | same sprite 4 block                              |
| (184, 102)     | `0x7F0E` | sprite 4: block (184, 102), texel (6, 12)        |

Each row was derived by hand from the rules above and checked against
our frame (2026-10-21). mGBA 0.10.5 draws the same frame.
