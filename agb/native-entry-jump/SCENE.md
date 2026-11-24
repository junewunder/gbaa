# native-entry-jump: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-entry-jump.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-entry-jump` table of `crates/codegen/lift-check.toml`, on
`units/native-entry-jump-600.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/entry-jump-test-program`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
draws four pixels in column `f % 240`, colours BGR555, low 15 bits of:

- row `f / 240 % 40`: `ej_a(f)`, `(4f + 7 + (f & 1)) ^ 0x3C3C`: `ej_b`
  entered by a `b` with r4 = 3f + 7 and the carry f's bit 0;
- 40 rows below: `ej_bp(f)`, `f ^ 0x3C3C`: `ej_b` called through a
  pointer with r4 0 and the carry clear;
- 80 rows below: `ej_c(f)`, `((f + 0x1234) + (f ^ 0x99) + (f >> 1 & 1)) ^
  0x5280`: `ej_d`'s call returning into `ej_e` with `ej_d`'s r0 and
  `ej_c`'s r4 and carry;
- 120 rows below: `ej_ep(f)`, `f ^ 0x5280`: `ej_e` called through a
  pointer with r4 0 and the carry clear.

Nothing is ever cleared, so by frame 600 rows 0-2, 40-42, 80-82 and
120-122 hold frames 0-599. The rest of the frame buffer is what agb's
start-up left in VRAM (sparse dots), the same every run.

| Frame | Column, row | `ej_a`   | `ej_bp`  | `ej_c`   | `ej_ep`  |
|-------|-------------|----------|----------|----------|----------|
| 0     | 0, 0        | `0x3C3B` | `0x3C3C` | `0x404D` | `0x5280` |
| 1     | 1, 0        | `0x3C30` | `0x3C3D` | `0x404D` | `0x5281` |
| 3     | 3, 0        | `0x3C28` | `0x3C3F` | `0x4052` | `0x5283` |
| 101   | 101, 0      | `0x3DA0` | `0x3C59` | `0x4115` | `0x52E5` |
| 599   | 119, 2      | `0x3558` | `0x3E6B` | `0x45DA` | `0x50D7` |

(Frame 101's read from our interpreter's VRAM at frame 600, frame 599's
from `RESULTS`, `gbaa-interp --peek`.) A lift that enters `ej_b` by its
plain entry where `ej_a` jumps into it draws `f ^ 0x3C3C` in the top row
(frame 0: `0x3C3C`); one that takes `ej_c`'s call for one that never
returns stops at frame 0. mGBA 0.10.5 draws frame 600 as hash
`034490afe017b4ef`, as our interpreter does (`just mgba-diff`,
2026-11-24).
