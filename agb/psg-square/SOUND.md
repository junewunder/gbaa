# psg-square: expected sound

`just agb-tests` builds this into `$GBAA_DATA/agb/psg-square.gba`. It plays
the PSG's square channels 1 and 2 alongside FIFO A, and its check is
`just psg-square`: from mGBA's state after frame 1 (before any note), 119
frames' FIFO records and PSG samples must be byte-identical with mGBA
0.10.5's (`just mgba-audio-diff`).

The screen is in forced blank (white) throughout.

## How it plays

- **Setup** (before the first V-blank): master on; `SOUNDCNT_H` with FIFO
  A at 100% to both speakers on timer 0, reset; DMA1 feeds FIFO A from
  `WAVE`, a triangle wave (period 64 samples, peak ±48, 32 KB, after which
  it reads on into the ROM); timer 0 overflows every 1024 cycles
  (16384 Hz); DMA3 is armed.
- **Every V-blank** DMA3 copies the next row of `SCORE`, nine words, to
  `0x04000060`–`0x04000083` (dest reloaded each time): channel 1's
  sweep, duty/length and envelope, frequency and control; channel 2's;
  zeros for channels 3 and 4; `SOUNDCNT_L` and `SOUNDCNT_H`. So every
  register is rewritten every frame, and a restart only when the row has
  the bit. DMA, not the CPU, writes them, so they land at the same cycle
  in mGBA and in ours whatever the CPU's timing (we have no wait states).
- Square frequencies are `2048 − 32n`: a period of `512 · n` cycles, a
  whole number of samples.

## The score (row = V-blank from 0)

| Row | What                                                                 |
|-----|----------------------------------------------------------------------|
| 0   | `SOUNDCNT_L`: volumes 7/7, channel 1 to both, channel 2 right; PSG 100% |
| 2   | Channel 2: 50% duty, volume 15 held, 512 Hz, restart                 |
| 10  | Channel 1: 25% duty, volume 12 falling (step 2), 409.6 Hz, restart   |
| 20  | Channel 2 to 256 Hz, no restart                                      |
| 30  | Channel 1: sweep up (time 2, shift 3) from 1024, volume 15: overflows and stops |
| 45  | Channel 1: sweep down (time 1, shift 2) from 1800, volume 4 rising   |
| 55  | Channel 2: 12.5% duty, length 4 (stop on), 512 Hz: silent within a frame |
| 60  | Channel 1 no sweep, 50%, volume 10, 341 Hz; channel 2 25%, volume 9 falling, 683 Hz; both restart; both channels to both speakers, left volume 7, right 3 |
| 70  | PSG at 50%                                                           |
| 76  | PSG at 25%                                                           |
| 80  | PSG at 100%; channel 1: volume 0 rising (step 1), 1024 Hz, restart   |
| 90  | Channel 2 restarted with volume 0 falling: DAC off                   |
| 95  | Channel 1: sweep down (time 3, shift 1), length 32, from 2000, volume 13 |
| 105 | Channel 2: 75% duty, volume 8 falling (step 4), 1024 Hz              |

Rows not listed repeat the row before (without the restart bit).

## Where the timing is tight

A V-blank falls `85 + 320·k (mod 512)` cycles into a 512-cycle sample in
mGBA's clock (its frame k after reset, mGBA's HLE BIOS starting the screen
at line 126). Ours runs the whole DMA at the V-blank's cycle; mGBA reaches
the ninth word, `SOUNDCNT_L`/`_H`, tens of cycles in. In rows `8k + 3`
that's after the next sample is taken, so a `SOUNDCNT_H` change there
would reach one sample later in mGBA: the score keeps such changes off
those rows (76, not 75). The first four words land early enough in every
row.

The run length matters too: if the last V-blank falls in the second half
of a 1024-cycle sampling period, ours ends one PSG sample later than mGBA
(ours runs that V-blank's DMA, which samples, before presenting the frame;
mGBA ends the frame first). 119 frames is clear of that.
