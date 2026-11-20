# c-bits: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-bits-agbcc.gba` and
`c-bits-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-bits-agbcc` and `c-bits-gcc`, on
`units/c-bits-<cc>-600.toml`).

Four 8x8 balls (`struct Ball` in `gBalls`), each with its state in
bitfields: in one byte `left`, `up` (its direction), `speed` (1 to 3
pixels a frame across) and `hue` (an index into 16 colours); in a halfword
`bounces` (6 bits), `trail` (3), `visible` and `shade` (5). Every access
goes through a pointer to the ball. `ball_init` sets them one field at a
time, partly through `ball_aim`, `ball_paint` and `ball_show`, which each
write one field (a compiler joins writes to fields sharing bytes, which
these calls keep apart); `ball_move` reads and writes several, which
agbcc and GCC join into one load or store.

Each frame each ball is erased, moved (`speed` pixels across, one down or
up) and drawn in its `hue` if `visible`. A wall turns it round, counts a
bounce and steps its `hue` (left and right walls) or takes 3 off its
`shade`, wrapping at 0 (top and bottom; the top wall is at y 8). Along the top, ball `i`'s
bounces show as a bar at x `60 i`, `bounces` pixels wide, grey at its
`shade`; under it a dot in the ball's colour at x `60 i + trail`, which
moves one pixel a frame over eight. Every 64 frames `ball_blink` turns one
ball's `visible` over and steps its `speed` (1, 2, 3, 1, …).

Mode 3, BG2 on, black. Frame 600: hash `5622c9d81c4b4c64`, the same in
mGBA 0.10.5 and our interpreter, and in both builds (`just mgba-diff`,
2026-11-20).
