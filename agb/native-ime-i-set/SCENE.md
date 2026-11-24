# native-ime-i-set: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-ime-i-set.gba`.
The stage-A lifter's check runs it (`just lift-check`, the
`native-ime-i-set` table of `crates/codegen/lift-check.toml`, on
`units/native-ime-i-set-601.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/ime-i-set-test-program`).

Mode 3, BG2 on. Each pass of the main loop `f` (from 0) draws one pixel
at `(f % 240, f / 240 % 80)`, BGR555 `0x7C00 | (c2 - c0) << 5 | (c1 -
c0)`: whether the V-blank handler ran by the time `IME` was set again
with I still set (`c1 - c0`) and by the time the `msr` that clears I
returned (`c2 - c0`). On the hardware every pixel is `0x7C20` (blue,
`#0008FF` in a dump): held through the `IME` store, taken at the `msr`.

The picture is `agb/native-irq-hold`'s: the same pixels, a pass a frame
and a half on average, 400 pixels by frame 601, and agb's start-up dots
in the rest of VRAM.

| Build | Pixel | Means |
|-------|-------|-------|
| hardware, mGBA, the play build, the lift | `0x7C20` | held through the `IME` store, taken at the `msr` |
| stage A's old lift (`--irq-at-waits`) | `0x7C21` | run at the `IME` store despite I |
| a play build that holds it past the `msr` | `0x7C00` | not yet run when `is_restore` returns |

The last row is what the play build drew at `--timing native` before
this unit (frame 601's hash `ae93e567ec0a8bc7`: it dropped the `IME`
write's release while I was set, and the `msr` found nothing held).
mGBA 0.10.5 draws frame 601 as hash `abc7ac7a22ed22b3`, as our
interpreter does at `--timing exact` and now `--timing native` (`just
mgba-diff`, 2026-11-24).
