# native-takes: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-takes.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-takes` table of `crates/codegen/lift-check.toml`, on
`units/native-takes-600.toml`); what it exercises is in `src/main.rs`'s
header (unit `native/taker-test-program`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
draws two pixels, colours BGR555, low 15 bits of:

- `(f % 240, f / 240 % 80)`: `tk_a`'s `(2f + 0x11) ^ ((f ^ 0x1C0) << 2)`,
  with r8 = f + 0x11 and r9 = f ^ 0x1C0 from `tk_set`;
- 80 rows below it: `tk_b`'s `0x311 ^ ((f ^ 0x1C0) >> 1)`, with r8 =
  f + 0x311 and the same r9.

Nothing is ever cleared, so by frame 600 rows 0-2 and 80-82 hold frames
0-599. The rest of the frame buffer is what agb's start-up left in VRAM
(sparse dots), the same every run.

| Frame | Pixel     | Colour   | Pixel      | Colour   |
|-------|-----------|----------|------------|----------|
| 0     | (0, 0)    | `0x0711` | (0, 80)    | `0x03F1` |
| 1     | (1, 0)    | `0x0717` | (1, 80)    | `0x03F1` |
| 3     | (3, 0)    | `0x071B` | (3, 80)    | `0x03F0` |
| 101   | (101, 0)  | `0x064F` | (101, 80)  | `0x03C3` |
| 599   | (119, 2)  | `0x0AE3` | (119, 82)  | `0x02DA` |

(Read from our interpreter's VRAM at frame 600, `gbaa-interp --peek`.)
A lift that starts `tk_a` and `tk_b` with r8 and r9 zeroed draws `f` in
the top row and `-f` below it (frame 1: `0x0001`, `0x7FFF`). mGBA 0.10.5
draws frame 600 as hash `26d7d8f6a36ab6e3`, as our interpreter does
(`just mgba-diff`, 2026-11-14).
