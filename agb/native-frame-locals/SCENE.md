# native-frame-locals: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-frame-locals.gba`.
The native lifter's check runs it (`just lift-check`, the
`native-frame-locals` table of `crates/codegen/lift-check.toml`, on
`units/native-frame-locals-600.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/frame-locals`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
draws two pixels, colours BGR555:

- `(f % 240, f / 240 % 80)`: `fl_keep(f)`,
  `((f + 7) ^ ((f & 0xFFFF) << 8)) + 0x5A`, low 15 bits: three values
  kept in stack locals across a call;
- 80 rows below it: `fl_esc(f)`, `2f + 1`: a slot whose address
  `fl_double` writes through.

Nothing is ever cleared, so by frame 600 rows 0-2 and 80-82 hold frames
0-599. The rest of the frame buffer is what agb's start-up left in VRAM
(sparse dots), the same every run.

| Frame | Pixel     | Colour   | Pixel      | Colour   |
|-------|-----------|----------|------------|----------|
| 0     | (0, 0)    | `0x0061` | (0, 80)    | `0x0001` |
| 1     | (1, 0)    | `0x0162` | (1, 80)    | `0x0003` |
| 3     | (3, 0)    | `0x0364` | (3, 80)    | `0x0007` |
| 101   | (101, 0)  | `0x65C6` | (101, 80)  | `0x00CB` |
| 599   | (119, 2)  | `0x55B8` | (119, 82)  | `0x04AF` |

A lift whose `fl_keep` stores `x + 8` for `x + 7` in its local (the check's
planted control, `--plant-slot=0x08000182`) draws every top-row pixel one
more (frame 0: `0x0062`). mGBA 0.10.5 draws the frames above (frame 600's
hash `810ca660f7f77795`, `target/release/mgba-dump ROM --frames 600
--hash`, 2026-11-06).
