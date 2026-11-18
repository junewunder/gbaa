# c-switch: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-switch-agbcc.gba` and
`c-switch-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-switch-agbcc` and `c-switch-gcc`, on
`units/c-switch-<cc>-600.toml`). Both compilers make each switch a jump
table (`mov pc` through a word table after the code).

Mode 3, BG2 on, black. Each frame `script_run` runs `sScript`'s byte
commands until one waits: a yellow 10x10 box at (20, 20), then four green
6x6 boxes stepping (+12, +6) with a white dot above each, then it sets the
pen's state (zigzag, spiral, colour cycling, the square round the screen),
clears (16, 16)-(127, 79) and jumps back. `pen_update` moves a 2x2 pen
(orange, later cycling colours) by its state. A bar at (4, 150) is as long
as the commands run so far, low 7 bits, in a colour cycling every frame.

Frame 600: hash `972d4d564237041d`, the same in mGBA 0.10.5 and our
interpreter (`just mgba-diff`, 2026-11-18).
