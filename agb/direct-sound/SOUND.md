# direct-sound: expected sound

`just agb-tests` builds this into `$GBAA_DATA/agb/direct-sound.gba`. It
plays direct sound the way a software mixer (agb's, the m4a engine's)
drives it, and nothing else: no PSG. It is the native check's direct-sound
program (`just native-check`, `units/direct-sound.toml`; its native port is
`crates/native-spike/src/direct_sound.rs`).

The screen is in forced blank (white) throughout.

## How it plays

- **Buffers** at fixed IWRAM addresses (not statics, so the native port
  keeps them at the same place and the check compares them): FIFO A's two
  halves at `0x03004000` and `0x03004100`, FIFO B's at `0x03004200` and
  `0x03004300`, 256 signed samples each; the synth's state (buffers filled,
  A's phase, B's phase) at `0x03004400`.
- **Setup**: both halves filled; master on, PSG off (`SOUNDCNT_L` 0);
  `SOUNDCNT_H`: FIFO A to the left, B to the right, both 100%, both on
  timer 0, both reset. DMA1 feeds FIFO A from A's half 0, DMA2 FIFO B from
  B's half 0 (32-bit, repeat, special start). Timer 1 counts up on timer
  0's overflows with its IRQ on; timer 0 overflows every 1024 cycles
  (16384 Hz).
- **Timer 1's IRQ** (every 256 samples): DMA1 and DMA2 are switched off and
  on again aimed at the other half (the re-latch), then the half just
  played is filled with the next 256 samples: on A a sawtooth whose step
  (2-6) changes with each buffer, on B a ±40 square whose period (32-56
  samples) does.

## Where the timing is tight

A FIFO refills (4 words) when a pop leaves it 16 bytes or fewer: at pops
1 and 2, then every 16th (17, 33, ...). With timer 1's period 256, a
multiple of 16, every swap would come one pop (1024 cycles) before a
refill, and the handler's re-latch would race it: in our exact timing a
swap whose IRQ waited behind V-blank's handler lost the race at frame 46,
where mGBA's won. So timer 1's first period is 264 (it is enabled with
reload `0x10000 - 264`, which is then rewritten to `0x10000 - 256` for the
overflows after): each swap comes 8 pops before a refill, 8192 cycles for
the re-latch in any emulator's timing.

A refill near a half's end reads on past it (into the next half, or for
B's half 1 into the synth's state): the same bytes in every build, since
the buffers are at fixed addresses.

## Checks

- `just mgba-diff $GBAA_DATA/agb/direct-sound.gba 300`: the frame matches
  mGBA 0.10.5 (`5a89fb4d57df1f25`, forced-blank white).
- From mGBA's state after frame 1 (`mgba-dump ROM --frames 1 --save-state
  S`), `just mgba-audio-diff ROM 299 S`: 81945 FIFO records and 164038 PSG
  samples byte-identical with mGBA's (2026-11-06).
- `just native-check`: the native port against the play build, samples
  equal every frame.
