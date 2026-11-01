# bios-sound: expected frame

`just agb-tests` builds this into `$GBAA_DATA/agb/bios-sound.gba`. It makes
the BIOS sound driver's calls once, draws what they left before the first
VBlank, and never changes; `just unit bios-sound` checks frame 10 against
mGBA (`units/bios-sound.toml`).

Mode 3, BG2 on, grey backdrop (`0x4210`). Each word is three pixels: bits
0-14, 15-29 and 30-31 (BGR555, so bit 15 of a pixel is never needed).

| Row | x       | What                                                     | mGBA's BIOS leaves     |
|-----|---------|----------------------------------------------------------|------------------------|
| 0   | 0-143   | r0-r3 after swi 0x1A-0x1E, 0x20-0x24, 0x28, 0x29 (12 px each) | the arguments: `0x03001000 0x11112222 0x33334444 0x55556666` |
| 2   | 0-107   | the 36 words `SoundDriverGetJumpList` (0x2A) writes      | `0x000001B0` each      |
| 4   | 0-11    | r0 - the list's address, r1, r2, r3 after it             | `0x90 0 0x33334444 0x1B0` |
| 6   | 0-2     | r0 after calling the list's first entry with `0xCAFEF00D` | `0xCAFEF00D` (a `bx lr`) |
| 40-79 | 40-79 | green (`0x03E0`) when every row is as listed, red otherwise |                     |

Everything else is the backdrop.
