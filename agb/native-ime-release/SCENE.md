# native-ime-release: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-ime-release.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-ime-release` table of `crates/codegen/lift-check.toml`, on
`units/native-ime-release-601.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/ime-release`).

Mode 3, BG2 on. Each pass of the main loop `f` (from 0) draws one pixel
at `(f % 240, f / 240 % 80)`, BGR555 `0x7C00 | (c2 - c0) << 5 | (c1 -
c0)`: whether the V-blank handler ran while `IME` was clear (`c1 - c0`)
and by the time the store that sets `IME` again returned (`c2 - c0`). On
the hardware every pixel is `0x7C20` (blue, `#0008FF` in a dump): held
while `IME` is clear, taken at the store.

The picture is `agb/native-irq-hold`'s: the same pixels, a pass a frame
and a half on average (agb's `wait_for_vblank` skips the wait when a
V-blank came since the last call), 400 pixels by frame 601, and agb's
start-up dots in the rest of VRAM.

| Build | Pixel | Means |
|-------|-------|-------|
| hardware, mGBA, the play build, the lift | `0x7C20` | held while `IME` is clear, taken at the store |
| a build that holds it to the next wait (the old lift, `--ime-at-waits`) | `0x7C00` | not yet run when the store returns |

The last row is what the play build drew at `--timing native` before
this unit (frame 601's hash `0346a4614af57b13`; it opened its IRQ window
only where time passed). mGBA 0.10.5 draws frame 601 as hash
`abc7ac7a22ed22b3`, as our interpreter does at `--timing exact` and
`--timing native` (`just mgba-diff`, 2026-11-06). At frame 600 our exact
interpreter has drawn the 400th pixel and mGBA not yet, so the route
stops at 601.
