# native-ram-jump: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-ram-jump.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-ram-jump` table of `crates/codegen/lift-check.toml`, on
`units/native-ram-jump-600.toml`); what it exercises is in `src/main.rs`'s
header (unit `native/ram-jump-passes-regs`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
draws one pixel at `(f % 240, f / 240 % 160)`, colour BGR555, the low 15
bits of `tj_c`'s `0x23 ^ ((f ^ 0x2A0) << 1)`, with r8 = f + 0x23 and
r9 = f ^ 0x2A0 from `tj_set`, through `tj_jump`'s jump into IWRAM.

Nothing is ever cleared, so by frame 600 rows 0-2 hold frames 0-599. The
rest of the frame buffer is what agb's start-up left in VRAM (sparse
dots), the same every run.

| Frame | Pixel     | Colour   |
|-------|-----------|----------|
| 0     | (0, 0)    | `0x0563` |
| 1     | (1, 0)    | `0x0561` |
| 3     | (3, 0)    | `0x0565` |
| 101   | (101, 0)  | `0x05A9` |
| 599   | (119, 2)  | `0x01CD` |

(Read from our interpreter's VRAM at frame 600, `gbaa-interp --peek`.)
A lift whose call of `tj_jump` passes it r8 and r9 zeroed draws `-f`
(frame 0: `0x0000`, frame 1: `0x7FFF`). mGBA 0.10.5 draws frame 600 as
hash `ba172157a4755a93`, as our interpreter does (`just mgba-diff`,
2026-11-24).
