# windows-blend: expected frame

`just agb-tests` builds this into `$GBAA_DATA/agb/windows-blend.gba`. The
scene is set up once before the first VBlank and never changes; check it
at frame 10 or later (`just unit windows-blend` does, from
`units/windows-blend.env`):

    just first-frame "$GBAA_DATA/agb/windows-blend.gba" 10
    just mgba-diff "$GBAA_DATA/agb/windows-blend.gba" 10

Mode 0, BG0, BG1 and OBJ on, 1D OBJ mapping, WIN0, WIN1 and the OBJ window
on. Colours are BGR555.

- BG0 (priority 0): red (`0x001F`) where x < 120, every row.
- BG1 (priority 1): blue (`0x7C00`) where y < 128, every column.
- Backdrop grey (`0x4210`).
- Sprites, green (`0x03E0`), priority 0: 0, an OBJ-window 32x32 at
  (104, 20); 1, a semi-transparent 16x16 at (180, 100); 2, a normal 8x8 at
  (40, 136); 3, a semi-transparent 8x8 at (140, 64).
- WIN0 (x 16..48, y 16..48): BG1 only, no effects. WIN1 (x 32..160,
  y 32..96): BG0, BG1 and sprites, no effects. OBJ window: BG1 and
  effects. Outside: everything.
- BLDCNT: brighten, first targets BG0 and sprites, second target BG1;
  EVY 8, EVA 8, EVB 8. Brightened red is `0x3DFF`, brightened green
  `0x3FEF`, green blended half and half with blue `0x3DE0`.

| Pixel (x, y)   | Colour   | Why                                              |
|----------------|----------|--------------------------------------------------|
| (0, 0)         | `0x3DFF` | outside: BG0, brightened                         |
| (15, 16)       | `0x3DFF` | just left of WIN0                                |
| (200, 0)       | `0x7C00` | outside: BG1, not a first target                 |
| (0, 140)       | `0x3DFF` | BG0 below BG1's end, brightened                  |
| (200, 140)     | `0x4210` | backdrop, not a first target                     |
| (16, 16)       | `0x7C00` | WIN0's first pixel: BG1 only                     |
| (47, 47)       | `0x7C00` | WIN0's last pixel                                |
| (32, 32)       | `0x7C00` | WIN0 and WIN1 overlap: WIN0 wins                 |
| (48, 48)       | `0x001F` | WIN1: BG0, no effects                            |
| (119, 95)      | `0x001F` | WIN1's last row, BG0's last column               |
| (120, 95)      | `0x7C00` | WIN1, past BG0: BG1                              |
| (160, 95)      | `0x7C00` | just right of WIN1: outside, BG1                 |
| (103, 20)      | `0x3DFF` | just left of the OBJ window                      |
| (104, 20)      | `0x7C00` | OBJ window: BG0 hidden, BG1 shows                |
| (119, 31)      | `0x7C00` | OBJ window, above WIN1                           |
| (119, 32)      | `0x001F` | WIN1 over the OBJ window                         |
| (180, 100)     | `0x3DE0` | sprite 1, semi-transparent over BG1              |
| (195, 115)     | `0x3DE0` | sprite 1's last pixel                            |
| (196, 116)     | `0x7C00` | just past sprite 1                               |
| (40, 136)      | `0x3FEF` | sprite 2 over BG0, a first target: brightened    |
| (47, 143)      | `0x3FEF` | sprite 2's last pixel                            |
| (48, 144)      | `0x3DFF` | just past sprite 2: BG0, brightened              |
| (140, 64)      | `0x3DE0` | sprite 3 in WIN1: blended though effects are off |
| (147, 71)      | `0x3DE0` | sprite 3's last pixel                            |

Every other pixel follows the same rules.

## mGBA

With the OBJ window on, mGBA 0.10.5 brightens every sprite, first target or
not (without it, only first targets). So sprites are a first target here,
which keeps the frame the same in mGBA and on hardware. Semi-transparent
sprites blend over a second target even where the window turns effects
off, in mGBA (deliberately) and here.
