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

Frame 600's hash is `b0cefdd15001e46e` in our interpreter at `--timing
exact` (2026-11-13). mGBA 0.10.5 draws `8d45278efae3c410`: its serial
port with nothing plugged in answers where the play build's keeps what
was stored (`SIOCNT`'s SI bit, an internal-clock transfer ended at once
with its IRQ, multiplayer's forced bits), and it differs in `RCNT`'s low
four bits, `JOYCNT`, `JOYSTAT` and what the unused halfwords read. With
those left out of the pixel both draw `67ad2572c48f1e9b`; the list, read
from mGBA's source and from both builds' `OUT`, is in
`units/native-serial-600.toml`.
