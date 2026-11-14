# native-serial: what it draws

`just agb-tests` builds this into `$GBAA_DATA/agb/native-serial.gba`. The
native lifter's check runs it (`just lift-check`, the `native-serial`
table of `crates/codegen/lift-check.toml`, on
`units/native-serial-600.toml`); what it exercises is in `src/main.rs`'s
header (unit `native/hw-serial-registers`).

Mode 3, BG2 on, from the first frame of the loop. Each frame `f` (from 0)
stores to the serial port's registers (`0x0400_0120`-`0x0400_015F`), loads
them back and draws one pixel, at row `f / 240 % 4`, column `f % 240`:
everything it loaded (`OUT` in `src/main.rs`) folded into 15 bits
(`colour`). So by frame 600 row 0 holds frames 0-239, row 1 frames
240-479 and row 2 the frames from 480 on, about half of it; the rows fill
again from the top at frame 960. The rest of the frame
buffer is what agb's start-up left in VRAM, the same every run. `OUT`'s
bytes are also compared as RAM by the native check, and the registers
themselves as I/O.

Frame 600's hash is `56d84929d20fa3ae` in our interpreter at `--timing
exact` (2026-11-14). mGBA 0.10.5 draws `8d45278efae3c410`. The play
build's serial port with nothing plugged in answers as mGBA's (`SIOCNT`'s
SI bit, an internal-clock transfer ended at once with its IRQ,
multiplayer's forced bits, `RCNT`'s low four bits) apart from what is kept
on purpose: `JOYCNT`'s and `JOYSTAT`'s bits outside JOY bus mode (GBATEK's
here, every bit stored in mGBA), what the unused halfwords read (0 here,
open bus there) and a byte store to `SIOCNT` (mGBA merges it with what was
last stored, not the register). The list and how it was found is in
`units/native-serial-600.toml`.
