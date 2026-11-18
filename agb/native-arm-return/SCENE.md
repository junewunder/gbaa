# native-arm-return: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-arm-return.gba`.
The per-function check's second reference runs on it (`just fn-check
$GBAA_DATA/agb/native-arm-return.gba --fn-check 0x080001a4 --fn-check
0x080001b8`: `ar_mov` and `ar_ldm`, per `nm`; holes
`holes/native-arm-return.toml`); what it exercises is in `src/main.rs`'s
header (unit `native/fn-check-interp`). Each must report its interpreter
runs as checked and agreed, and `--plant 0x080001b0 --plant-reference`
(`ar_mov`'s store) must be caught by them.

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
draws two pixels, colours BGR555, low 15 bits:

- `(f % 240, f / 240 % 80)`: `ar_call(f)`, `(f ^ (3f + 1)) + 7`;
- 80 rows below it: `ar_mov(f)`, `3f + 1`.

| Frame | Pixel    | Colour   | Pixel     | Colour   |
|-------|----------|----------|-----------|----------|
| 0     | (0, 0)   | `0x0008` | (0, 80)   | `0x0001` |
| 1     | (1, 0)   | `0x000C` | (1, 80)   | `0x0004` |
| 2     | (2, 0)   | `0x000C` | (2, 80)   | `0x0007` |

The rest of the frame buffer is what agb's start-up left in VRAM, the
same every run. Not yet in the lifter's check (`just lift-check`) nor
compared with mGBA.
