# native-irq-hold: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-irq-hold.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-irq-hold` table of `crates/codegen/lift-check.toml`, on
`units/native-irq-hold-601.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/lift-irq-hold`).

Mode 3, BG2 on. Each pass of the main loop `f` (from 0) draws one pixel
at `(f % 240, f / 240 % 80)`, BGR555 `0x7C00 | (c2 - c0) << 5 | (c1 -
c0)`: whether the V-blank handler ran inside the critical section (`c1 -
c0`) and by the time `ih_restore` returned (`c2 - c0`). On the hardware
every pixel is `0x7C20` (blue, `#0008FF` in a dump): held inside, taken
at the `msr`.

A pass takes a frame and a half on average: `ih_probe` returns at line
170, and agb's `wait_for_vblank` skips the wait when a V-blank came since
the last call (the one the `msr` let through counts), so passes alternate
between one frame and two. By frame 601 there are 400 pixels: row 0 and
the first 160 of row 1. The rest of the frame buffer is what agb's
start-up left in VRAM (sparse dots), the same every run.

| Build | Pixel | Means |
|-------|-------|-------|
| hardware, mGBA, the play build | `0x7C20` | held in the section, taken at the `msr` |
| stage A's old lift (`--irq-at-waits`) | `0x7C21` | run at the wait inside the section |
| a play build that holds it to the next wait | `0x7C00` | not yet run when `ih_restore` returns |

The last row is what the play build drew at `--timing native` before
this unit (it opened its IRQ window only where time passed). mGBA 0.10.5
draws frame 601 as hash `abc7ac7a22ed22b3`, as our interpreter does at
`--timing exact` and `--timing native` (`just mgba-diff`, 2026-11-01).
At frame 600 our exact interpreter has drawn the 400th pixel and mGBA
not yet (their frame 599 and 600 pictures are a frame apart), so the
route stops at 601.
