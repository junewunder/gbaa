# c-loops: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-loops-agbcc.gba` and
`c-loops-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-loops-agbcc` and `c-loops-gcc`, on
`units/c-loops-<cc>-600.toml`).

Counted loops in the shapes C writes them (unit native/lift-for-bounds),
most to a bound that isn't a constant: `sum` to a parameter, `draw_global`
to a global (`gBarCount`, a u8), `draw_row` to a struct's field through a
pointer, `draw_down` counting down to 0 from a parameter, `first_long`
breaking out early (its counter read after the loop), `draw_odd` skipping
rounds with `continue`, `grow` with a `u8` counter, and `grid` two loops
nested, the inner from a value to a bound. `AgbMain` fills `gLens` and
copies it backwards into `gRow.len` with loops to a constant and to a
field. `draw_global`'s and `draw_row`'s bounds are loaded from RAM each
round, around a call: the lift prints them as `for` where nothing a round
runs may store to the bound (unit native/lift-for-ram-bounds).

Arrays indexed by the counter (unit native/lift-for-index): `move_bars`
an array of structs (`gMovers`, 8 bytes each) a field each, `pocket_count`
a field array through a pointer (`p->slots[i].id`, as pret's bag),
`add_rows` two arrays by one counter, and `sum_movers` a pointer stepped
beside the counter (`b++`).

Twelve bar lengths, `gLens`, grow by 1 to 4 every fourth frame and wrap
past 40. Four columns of bars (2 pixels high, at most 24 wide, 6 apart
from y 8): at x 0 the first `gBarCount` (4 to 11, stepping every 16
frames), at x 48 `gRow`'s (the rest, backwards, colours from its `hue`),
at x 96 one to eight counting down, at x 144 every other bar at half
length; the first two columns redraw on even frames, the other two on odd
ones, so drawing ends inside V-blank. At x 192 a grid of 3x3 squares, one
to four rows, its columns 0 to 7 or 1 to 6 by the frame's parity, its
colours cycling every 8 frames. At y 100 a bar the sum of the first
`gBarCount` lengths long (its low byte), at y 108 a 20-pixel bar at the
first length at least 20 to 35. Six movers' lengths grow by 1 or 2 each
frame and wrap past 40: at y 116 a bar their lengths and hues summed long
(low byte); at y 124 one four pixels for each item the pocket's first 4
to 11 slots hold of kind `(frame >> 2) & 3`; at y 132 one `gTotals[(frame >> 1)
& 3]` long (its low 7 bits), the sum of `gLens` and `gRow.len`.

Mode 3, BG2 on, black. Frame 600: hash `f7dfbac3dbb5ca31`, the same in
mGBA 0.10.5 and our interpreter, and in both builds (`just mgba-diff`,
2026-11-24).
