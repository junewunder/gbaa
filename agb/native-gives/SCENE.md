# native-gives: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-gives.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-gives` table of `crates/codegen/lift-check.toml`, on
`units/native-gives-600.toml`); what it exercises is in `src/main.rs`'s
header (unit `native/lift-gives`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
draws two pixels, colours BGR555, low 15 bits of:

- `(f % 240, f / 240 % 80)`: `a = (f + 0x21) * 3 ^ 0x155`, the r9
  `gv_step` gave back to `gv_mix`;
- 80 rows below it: `(a + 0x1000) * 3 ^ 0x155`, the r9 `gv_twice` gave
  back through its jump into `gv_step`.

Nothing is ever cleared, so by frame 600 rows 0-2 and 80-82 hold frames
0-599. The rest of the frame buffer is what agb's start-up left in VRAM
(sparse dots), the same every run.

| Frame | Pixel     | Colour   | Pixel      | Colour   |
|-------|-----------|----------|------------|----------|
| 0     | (0, 0)    | `0x0136` | (0, 80)    | `0x32F7` |
| 1     | (1, 0)    | `0x0133` | (1, 80)    | `0x32CC` |
| 101   | (101, 0)  | `0x00C7` | (101, 80)  | `0x3300` |
| 599   | (119, 2)  | `0x063D` | (119, 82)  | `0x43E2` |

(Frames 0, 1 and 101 read from our interpreter's VRAM at frame 600,
`gbaa-interp --peek`; 599 by the arithmetic.) A lift whose `gv_mix`
keeps its own r9 after each call draws `f + 0x21` in both rows (frame 0:
`0x0021`, `0x0021`). mGBA 0.10.5 draws frame 600 as hash
`4d01eb2f27a15a02`, as our interpreter does (`just mgba-diff`,
2026-11-20).
