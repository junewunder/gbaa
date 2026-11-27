# c-align: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-align-agbcc.gba` and
`c-align-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-align-agbcc` and `c-align-gcc`, on
`units/c-align-<cc>-600.toml`).

Mode 3, BG2 on, black. A 16x16 square moves as c/template's does, one
pixel right and down a frame from (1, 1), turning at the screen's edges.
Its colour comes from three readers in `align.s`, each a load through its
pointer parameter alone, which nothing but the callers make aligned
(native/lift-aligned-params):

- red: `rd32` (`ldr`) of `gBytes` plus the frame count's low two bits, so
  on three frames in four an address 4 doesn't divide, read rotated as the
  CPU does;
- green: `rd16` (`ldrh`) of `gBytes` plus bits 2-3 of the frame count, odd
  on half of them;
- blue: `sum_pairs`, which walks `kPairs` two words at a time in a loop,
  1 to 4 rounds by bits 4-5 of the frame count, calling `rd_pair` (two
  `ldr`s) on each pair: a word address by every caller, so the lift drops
  those loads' rotates.

Each is masked to 5 bits; `gSum` keeps their xor. The lift check's planted
control (`--plant-aligned`, rd32's key) takes rd32 to be passed a word
address: its guest-pc build must stop on the first odd one.

Frame 600: hash `bc55af3569763b25`, the same in mGBA 0.10.5 and our
interpreter (`just mgba-diff`, 2026-11-27).
