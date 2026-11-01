# affine: expected frame

`just agb-tests` builds this into `$GBAA_DATA/agb/affine.gba`. The scene
is set up once before the first VBlank and never changes; check it at
frame 10 or later (`just unit affine` does, from `units/affine.env`):

    just first-frame "$GBAA_DATA/agb/affine.gba" 10
    just mgba-diff "$GBAA_DATA/agb/affine.gba" 10

Mode 2, BG2, BG3 and OBJ on, 1D OBJ mapping. Colours are BGR555. Map
coordinates below are `(PA·x + PB·y + X) >> 8` and `(PC·x + PD·y + Y) >> 8`
for screen `(x, y)`, arithmetic shift (floor).

- Backdrop grey (`0x4210`). BG palette: 1 red `0x001F`, 2 blue `0x7C00`,
  3 yellow `0x03FF`, 4 white `0x7FFF`, 5 dark grey `0x0C63`. OBJ palette:
  1 green `0x03E0`, 2 magenta `0x7C1F`, 3 cyan `0x7FE0`.
- BG tiles (8bpp): 1 solid red, 2 solid blue, 3 a dark top row over
  yellow columns 0-3 and white 4-7.
- BG2 (priority 1): 128×128, no wrap. Tile (tx, ty) is empty where
  tx % 4 = 3, else tile 1 where tx + ty is even, else tile 3. Matrix
  (222, 128, -128, 222), 30 degrees; X = -80.0, Y = 14003/256 ≈ 54.7.
- BG3 (priority 2): 256×256, wrapping. Blue where
  (mx / 16 + my / 16) is even. Matrix (0x180, 0, 0, 0x180); X = -37.5,
  Y = 12.25.
- Sprites: 32×32 4bpp (tile 0 magenta, the rest of the top tile row and
  left tile column green, the others cyan) and 16×16 8bpp (texel rows
  striped two green, two cyan). For a box pixel at (dx, dy) from the
  box's centre, the texel is `((PA·dx + PB·dy) >> 8) + w/2`,
  `((PC·dx + PD·dy) >> 8) + h/2`; outside the sprite, nothing.
  - 0: 32×32, double size (64×64 box at (8, 8)), (181, 181, -181, 181):
    45 degrees. Priority 0.
  - 1: 32×32 at (100, 100), (0x80, 0, 0, 0x80): twice the size, so the
    box shows texels 8-23, all cyan. Priority 2: behind BG2, in front of
    BG3.
  - 2: 16×16 8bpp, double size (32×32 box at (180, 20)),
    (0x100, 0x80, 0, 0x100): sheared, texel column dx + dy/2 + 8.
  - 3: 32×32, double size at (200, 230), (0, 0x100, -0x100, 0): 90
    degrees. Its box rows 26-63 wrap to screen rows 0-37; texels land on
    x 217-239 (clipped at the right edge), y 0-21, all cyan.

| Pixel (x, y)   | Colour   | Why                                              |
|----------------|----------|--------------------------------------------------|
| (0, 0)         | `0x4210` | BG2 map x -80, outside; BG3 (218, 12) empty      |
| (120, 80)      | `0x001F` | BG2 map (64, 64): tile (8, 8), tile 1            |
| (70, 50)       | `0x7FFF` | BG2 map (5, 63): tile 3, column 5                |
| (102, 0)       | `0x03FF` | BG2 map (8, 3): tile 3, column 0                 |
| (93, 0)        | `0x0C63` | BG2 map (0, 8): tile 3's top row                 |
| (80, 50)       | `0x001F` | BG2 map (14, 58): tile (1, 7), tile 1            |
| (60, 50)       | `0x7C00` | BG2 map x -3, outside; BG3 (52, 87) blue         |
| (121, 7)       | `0x7C00` | BG2 map (28, 0): empty column 3; BG3 blue        |
| (230, 150)     | `0x4210` | BG2 map x 194, outside; BG3 (51, 237) empty      |
| (0, 159)       | `0x7C00` | BG3 (218, 250): x wrapped from -38               |
| (40, 40)       | `0x7FE0` | sprite 0's centre: texel (16, 16), cyan          |
| (40, 18)       | `0x7C1F` | sprite 0, dy -22: texel (0, 0), magenta          |
| (40, 17)       | `0x4210` | sprite 0, dy -23: texel row -1, nothing          |
| (109, 100)     | `0x001F` | sprite 1's box, under BG2 (map (64, 86))         |
| (120, 131)     | `0x7FE0` | sprite 1 where BG2's column is empty             |
| (188, 36)      | `0x03E0` | sprite 2, dy 0: texel (0, 8), green              |
| (187, 36)      | `0x4210` | just left of it: texel column -1                 |
| (192, 28)      | `0x03E0` | sprite 2, dy -8: texel (0, 0), sheared 4 right   |
| (191, 28)      | `0x7C00` | just left of it: BG3                             |
| (191, 30)      | `0x7FE0` | sprite 2, dy -6: texel (0, 2), cyan              |
| (190, 30)      | `0x7C00` | just left of it: BG3                             |
| (217, 10)      | `0x7FE0` | sprite 3, wrapped: texel (20, 31)                |
| (216, 10)      | `0x7C00` | just left of it: texel row 32, nothing; BG3      |
| (230, 21)      | `0x7FE0` | sprite 3: texel column 31                        |
| (230, 22)      | `0x4210` | texel column 32, nothing                         |
| (239, 0)       | `0x7FE0` | sprite 3 at the right edge: texel (10, 9)        |

Every other pixel follows the same rules. mGBA 0.10.5 draws the same
frame.
