# native-bios-affine: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-bios-affine.gba`.
The native lifter's check runs it (`just lift-check`, the
`native-bios-affine` table of `crates/codegen/lift-check.toml`, on
`units/native-bios-affine-600.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/lift-bios-affine`).

Mode 3, BG2 on, its affine registers set to the identity every frame by
`BgAffineSet` itself (call 2), so the frame buffer shows as written. Each
frame `f` (from 0) makes six calls and draws, at column `f % 240`, one
pixel a call: its r0-r3 after it, folded with what it wrote, in 15 bits
(`colour` in `src/main.rs`), call `k` on row `4k` (rows 0, 4, 8, 12, 16,
20). Row 4 (`BgAffineSet` from ROM's fixed entries) changes only with r3
and its column, row 8 (the identity) only with r3; the others vary with
every frame's arguments. Nothing is cleared, so by frame 600 the rows hold
frames 360-599.

Below them, from rows 1 to about 68, sit columns of dots every 16 pixels
that the program doesn't draw: what VRAM holds when agb hands it over,
the same in mGBA.

The `OUT` buffers and OAM's affine parameters (rewritten in one of eight
slots a frame, by `f % 8`) aren't drawn; the native check compares them
as RAM and display memory.
