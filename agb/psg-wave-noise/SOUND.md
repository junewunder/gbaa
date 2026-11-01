# psg-wave-noise: expected sound

`just agb-tests` builds this into `$GBAA_DATA/agb/psg-wave-noise.gba`. It
plays the PSG's wave channel 3 and noise channel 4 alongside FIFO A, and
its check is `just psg-wave-noise`: from mGBA's state after frame 1
(before any note), 119 frames' FIFO records and PSG samples must be
byte-identical with mGBA 0.10.5's (`just mgba-audio-diff`).

The screen is in forced blank (white) throughout.

## How it plays

- **Setup** (before the first V-blank): master on; `SOUNDCNT_H` with FIFO
  A at 100% to both speakers on timer 0, reset; wave RAM bank 0 gets a
  triangle (0 up to 15 and down) and bank 1 a sawtooth (0–15 twice), with
  channel 3's DAC off and bank 0 selected; DMA1 feeds FIFO A from `WAVE`
  (a triangle, period 64 samples, peak ±48, as in `psg-square`); timer 0
  overflows every 1024 cycles (16384 Hz); DMA3 is armed.
- **Every V-blank** DMA3 copies the next row of the score's DMA3 words,
  five words, to `0x04000078`–`0x0400008B` (dest reloaded each time):
  channel 4's length/envelope and clock/control, `SOUNDCNT_L` and `_H`,
  `SOUNDCNT_X` (`0x80`) and `SOUNDBIAS` (`0x200`, the BIOS's). So channel
  4 is rewritten every frame, and restarted only when the row has the bit.
- **Channel 3** is written only in the rows that change it (or restart
  it): the frame before, the CPU arms DMA2, one-shot at V-blank, with the
  row's two words for `0x04000070`–`0x04000077`. DMA2 outranks DMA3, so
  they land first. Not every frame, because each store to `SOUND3CNT_X`
  while the channel plays sets its next step, `4 · (6 + 2 · (2048 −
  rate))` cycles on (see below).
- **Wave RAM**: just after a row's V-blank, if the row has new wave data,
  the CPU stores it (four words) into the bank not played. The CPU's
  timing differs between the emulators (we have no wait states), but a
  store to the bank not played isn't heard, so it doesn't matter when it
  lands. In 64-sample mode both banks play, so no row then writes any.
- Channel 3 rates are `2048 − 64n`, a step every `512 · n` cycles, and
  channel 4 clocks are multiples of 512 cycles: whole numbers of samples.

## The score (row = V-blank from 0)

| Row | What                                                                 |
|-----|----------------------------------------------------------------------|
| 0   | `SOUNDCNT_L`: volumes 7/7, channels 3 and 4 to both; PSG 100%        |
| 2   | Channel 3: DAC on, bank 1 (sawtooth), 100%, a step every 1024 cycles, restart; triangle written to bank 0 |
| 10  | Channel 3 at 50%                                                     |
| 14  | 25%                                                                  |
| 18  | 75%                                                                  |
| 22  | Muted                                                                |
| 26  | Bank 0 (the triangle) at 100%, no restart; steps written to bank 1   |
| 32  | Bank 1 (the steps); a mixed pattern written to bank 0               |
| 36  | 64-sample mode (both banks), a step every 512 cycles                 |
| 44  | DAC off: channel 3 stops (and holds its last output)                 |
| 48  | Bank 1, length 16/256 s (stop on), every 1536 cycles, restart: stops in row 51 |
| 56  | Bank 0, no length, every 2048 cycles, restart                        |
| 60  | Channel 3 muted. Channel 4: volume 15 held, 15-bit, every 512 cycles, restart |
| 66  | Channel 4: 7-bit, every 512 cycles, restart                          |
| 72  | Volume 12 falling (step 1), 15-bit, every 1024 cycles, restart       |
| 80  | Volume 3 rising (step 2), 7-bit, every 3072 cycles, restart          |
| 88  | Length 2/256 s (stop on), volume 15, every 1024 cycles, restart: silent within a frame |
| 94  | Volume 10 held, 15-bit, every 2048 cycles, no length, restart        |
| 100 | A new clock, every 1024 cycles, no restart                           |
| 104 | Channel 3 at 100% to the left, channel 4 to the right; left volume 7, right 3; PSG at 50% |
| 110 | PSG at 25%                                                           |
| 114 | PSG at 100%; channel 3 restarted in 64-sample mode; channel 4 restarted at volume 0 rising (step 1) |

Rows not listed repeat the row before (without the restart bit).

## Where the timing is tight

As in `psg-square`: a V-blank falls `85 + 320·k (mod 512)` cycles into a
512-cycle sample in mGBA's clock, and mGBA's DMA reaches each word some
cycles after it, where ours writes them all at the V-blank's cycle. In
rows `8k + 3` the V-blank is 469 cycles in, so channel 3's next step
after a `SOUND3CNT_X` store, 24 cycles plus whole samples on, falls just
before a sample in ours and just after it in mGBA. An earlier version
rewrote channel 3 every frame and differed in exactly those rows; now no
row `8k + 3` writes channel 3. Channel 4's restart only sets when it next
clocks (no 24-cycle offset), and none falls in those rows either.

The run length: 119 frames, for `psg-square`'s reason.
