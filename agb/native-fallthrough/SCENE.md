# native-fallthrough: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-fallthrough.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-fallthrough` table of `crates/codegen/lift-check.toml`, on
`units/native-fallthrough-600.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/lift-fallthrough`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
draws two pixels, colours BGR555:

- `(f % 240, f / 240 % 80)`: `ft_a(f)`, `(5f + 3 + (f & 1)) ^ 0x5A5A`,
  low 15 bits: `ft_b` with r4 = 4f + 3 and the carry f's bit 0 from
  `ft_a`;
- 80 rows below it: `ft_c(f)`, `f ^ 0x5A5A`: `ft_b` called normally, with
  r4 0 and the carry clear.

Nothing is ever cleared, so by frame 600 rows 0-2 and 80-82 hold frames
0-599. The rest of the frame buffer is what agb's start-up left in VRAM
(sparse dots), the same every run.

| Frame | Pixel     | Colour   | Pixel      | Colour   |
|-------|-----------|----------|------------|----------|
| 0     | (0, 0)    | `0x5A59` | (0, 80)    | `0x5A5A` |
| 1     | (1, 0)    | `0x5A53` | (1, 80)    | `0x5A5B` |
| 3     | (3, 0)    | `0x5A49` | (3, 80)    | `0x5A59` |
| 101   | (101, 0)  | `0x5BA7` | (101, 80)  | `0x5A3F` |
| 599   | (119, 2)  | `0x51ED` | (119, 82)  | `0x580D` |

A lift that starts `ft_b` with r4 and the flags zeroed when `ft_a` falls
into it draws `f ^ 0x5A5A` in the top row too (frame 0: `0x5A5A`). mGBA
0.10.5 draws the frames above (frame 600's hash `34f6c5ecdabbc7b6`,
`just mgba-diff`, 2026-11-01).
