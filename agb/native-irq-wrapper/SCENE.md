# native-irq-wrapper: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-irq-wrapper.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-irq-wrapper` table of `crates/codegen/lift-check.toml`, on
`units/native-irq-wrapper-600.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/lift-irq-wrapper-tail`).

Mode 3, BG2 on. Frame `f` (from 0) draws one pixel at `(f % 240, f / 240
% 80)`. By frame 600 there are 599: rows 0 and 1 and the first 119 of
row 2. The rest of the frame buffer is what agb's start-up left in VRAM
(sparse dots), the same every run.

Frames 0-31 run under agb's own IRQ wrapper: BGR555 `0x03E0 | count &
0x1F`, green with the V-blank count in red (`0x03E1` to `0x03FF`, then
`0x03E0`).

From frame 32 the program's `iw_main` is the wrapper: `0x7C00 | (seq &
0xF) << 5 | (vblanks & 3) << 2 | vcounts & 3`, blue with the last two
handlers in green and the two counts in red. The V-count line alternates
between 100 and 160, so the pixels repeat every four frames:

| Frames (from 32) | V-count line | Pixel | `seq & 0xF` | Means |
|------------------|--------------|-------|-------------|-------|
| 4k | 100 | `0x7D25` | 9 | V-count's handler, then V-blank's |
| 4k + 1 | 160 | `0x7CCA` | 6 | V-blank's ran first, nested at the wrapper's `msr` |
| 4k + 2 | 100 | `0x7D2F` | 9 | as 4k |
| 4k + 3 | 160 | `0x7CC0` | 6 | as 4k + 1 |

A build that doesn't take the pending V-blank IRQ at the `msr` that
clears I (it runs after the first wrapper returns instead) draws 9 on
every frame. A lift whose wrapper stops after its handler call draws
nothing past frame 32.

mGBA 0.10.5 draws frame 600 as hash `fbf60721245ba62f`, as our
interpreter does (`just mgba-diff`, 2026-11-13).
