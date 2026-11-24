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
ball's `visible` over and steps its `speed` (1, 2, 3, 1, …): the clock's
`next` says which.

A clock, `gClock` (`struct Clock`), keeps its own bitfields: in a halfword
`beats` (10 bits) and `level` (6), in a byte `tick` (4), `next` (2), `run`
and `flash`. Every access to it is at its constant address, not through a
pointer (`clock_step`, `clock_draw`, `AgbMain`). Once the balls are placed
`AgbMain` sets `next` to 0 and `run` (one store, joined). While `run` is
set, each frame `tick` counts up, wrapping at 16; at each wrap `flash`
turns over, `beats` counts one more and `level` is `beats >> 2`. Under the
dots, a row at y 6 shows `tick` pixels and one at y 7 `level` pixels, white
while `flash` is set, else dark grey. The blinking ball is `gBalls[next]`,
and `next` steps on after each blink. (The halfword's fields come first, so
`beats` doesn't straddle a byte's fields' unit, as agbcc would let it;
pret-names places such a field from the struct's start, under a unit test
of its own.)

Mode 3, BG2 on, black. Frame 600: hash `f6f42298fc5e3e54`, the same in
mGBA 0.10.5 and our interpreter, and in both builds (`just mgba-diff`,
2026-11-24).
