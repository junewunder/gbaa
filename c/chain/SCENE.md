# c-chain: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-chain-agbcc.gba` and
`c-chain-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-chain-agbcc` and `c-chain-gcc`, on
`units/c-chain-<cc>-600.toml`).

Mode 3, BG2 on, black. Four boxes (10, 14, 8 and 12 square; orange, blue,
green, pink) move and bounce off the rectangle (8, 8)-(231, 143); `frame`
builds the bounds in a `struct Ctx` on its stack and passes its address to
`step_world`, `move_body` and `bounce`. When two boxes overlap both swap to
their colours' complements (`rect_union`'s result, returned by value, goes
to `gUnion`). A yellow bar at (4, 150) is twice the wall hits so far, low 6
bits; a 6x3 mark at (4 + 8i, 156) shows the last box to hit a wall, green
while all four were drawn.

Frame 600: hash `33230e1c837269b5`, the same in mGBA 0.10.5 and our
interpreter (`just mgba-diff`, 2026-11-18).
