# native-bios-calls: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-bios-calls.gba`.
The native lifter's check runs it (`just lift-check`, the
`native-bios-calls` table of `crates/codegen/lift-check.toml`, on
`units/native-bios-calls-600.toml`); what it exercises is in
`src/main.rs`'s header (unit `native/lift-bios-calls`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
makes every call once and draws:

- rows 0-12, column `f % 240`: one pixel a call, its r0-r3 after it
  folded into 15 bits (`colour` in `src/main.rs`), in this order: `Sqrt`,
  `ArcTan2`, `MidiKey2Freq`, `CpuFastSet` copy, `CpuFastSet` fill,
  `LZ77UnCompWram` from ROM, `LZ77UnCompWram` from RAM, `LZ77UnCompVram`,
  `RLUnCompWram`, `RLUnCompVram`, `SoundBias`, `MultiBoot`, and the
  `CpuFastSet` that copies the LZ77 data to RAM for the next frame;
- rows 40-79: `LZ77UnCompVram`'s 96 bytes (48 pixels) at row
  `40 + f % 40`, one of four 48-pixel slots across by `f / 40 % 4`: the
  literals `0x2211 0x4433 …` and their back-references, bands of yellow,
  green and grey;
- rows 100-139: `RLUnCompVram`'s 80 bytes (40 pixels) at row
  `100 + f % 40`, slot `f / 40 % 4`: magenta (the run of `0x3C`), the
  five literals, cyan (the run of `0xE7`), three literals and the
  padding.

The WRAM-bound outputs (`OUT`'s buffers) and `SOUNDBIAS` aren't drawn; the
native check compares them as RAM and I/O. Every frame with `f % 4 == 1`
waits with `Stop` in place of agb's V-blank wait, waking at the V-blank
IRQ. Nothing is cleared, so by frame 600 rows 0-12 hold frames 360-599
and the VRAM bands every slot. The rest of the frame buffer is what agb's
start-up left in VRAM, the same every run.

Frame 600's hash is `7d3db3b8854e494a` in our interpreter at `--timing
exact` and `--timing native` (2026-11-08). mGBA 0.10.5 (`mgba-dump ROM
--frames 600 --hash`) draws `e5c3533e9bf7cdeb`: rows 3, 4 and 12 differ
because its HLE `CpuFastSet` leaves r2 as given where the play build
leaves `r1 + 4 * count`, and row 11 because its `MultiBoot` leaves r0 as
given where the play build returns 1 (GBATEK's failure, no cable). Every
other row and both VRAM bands are mGBA's.
