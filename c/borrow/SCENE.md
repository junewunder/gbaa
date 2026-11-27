# c-borrow: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-borrow-agbcc.gba` and
`c-borrow-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-borrow-agbcc` and `c-borrow-gcc`, on
`units/c-borrow-<cc>-600.toml`).

Mode 3, BG2 on, black. A 16x16 square moves as c/template's does, one
pixel right and down a frame from (1, 1), turning at the screen's edges.
Its colour is red 31, green the frame count over 16 (low 5 bits), blue 0,
as c/lent's: `tint` puts the count in a 4-byte struct on its stack and
passes its address to `mark_or`, which tests it for null before reading
the byte back; that goes into cell `count & 7` of an 8-byte array struct
on the same stack (`cell_set`), read back from it (`cell_get`). The
callees are in `cells.c`, compiled apart, so neither compiler drops the
null check. The lift borrows all three pointers (native/lift-lend-wider):
the null check of a `Lent` prints as it came and is false, and the cell
accesses go through `p0.idx(i)`, checked against the array's length.

Frame 600: hash `ebde5dc81734f725` (c/lent's), the same in mGBA 0.10.5 and
our interpreter (`just mgba-diff`, 2026-11-27).
