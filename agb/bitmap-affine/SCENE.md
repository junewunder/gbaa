# bitmap-affine: expected frame

`just agb-tests` builds this into `$GBAA_DATA/agb/bitmap-affine.gba`. The
scene is set up once before the first VBlank and never changes; check it
at frame 10 or later (`just unit bitmap-affine` does, from
`units/bitmap-affine.env`):

    just first-frame "$GBAA_DATA/agb/bitmap-affine.gba" 10
    just mgba-diff "$GBAA_DATA/agb/bitmap-affine.gba" 10

Mode 5, page 1 (`DISPCNT` bit 4), BG2 on, `BG2CNT`'s wrap bit set (bitmaps
ignore it). Colours are BGR555.

- Backdrop grey (`0x4210`).
- Page 0 is all white (`0x7FFF`) and must not show anywhere.
- Page 1, 160×128: pixel (bx, by) is `bx / 8 | (by / 8) << 5`, plus blue
  `0x7C00` where bx and by are both even or both odd.
- BG2 matrix (177, 102, -102, 177): 30 degrees, 1.25 screen pixels per
  bitmap pixel. X = -8920, Y = 14464 (20.8 fixed point): bitmap (80, 64)
  at screen (120, 80).

Screen (x, y) shows bitmap `bx = (177·x + 102·y - 8920) >> 8`,
`by = (-102·x + 177·y + 14464) >> 8` (arithmetic shift, floor), or the
backdrop where that is outside 0 ≤ bx < 160, 0 ≤ by < 128.

| Pixel (x, y)   | Colour   | Why                                      |
|----------------|----------|------------------------------------------|
| (0, 0)         | `0x4210` | bitmap (-35, 56): outside                |
| (119, 0)       | `0x7C25` | bitmap (47, 9)                           |
| (120, 80)      | `0x7D0A` | bitmap (80, 64)                          |
| (121, 80)      | `0x00EA` | bitmap (80, 63): no blue                 |
| (60, 40)       | `0x7CE2` | bitmap (22, 60)                          |
| (200, 120)     | `0x7CF2` | bitmap (151, 59)                         |
| (4, 80)        | `0x4210` | bitmap (-1, 110): outside                |
| (5, 80)        | `0x01A0` | bitmap (0, 109): row 80's first inside  |
| (235, 80)      | `0x0053` | bitmap (159, 18): row 80's last inside  |
| (236, 80)      | `0x4210` | bitmap (160, 17): outside, no wrap       |
| (180, 20)      | `0x4210` | bitmap (97, -2): outside                 |
| (239, 159)     | `0x4210` | bitmap (193, 71): outside                |

Every other pixel follows the same formula; all 38,400 of our frame's do
(checked by script on 2026-10-21). mGBA 0.10.5 draws the same frame.
