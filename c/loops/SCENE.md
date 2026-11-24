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
field.

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
first length at least 20 to 35.

Mode 3, BG2 on, black. Frame 600: hash `45ce6655274c0721`, the same in
mGBA 0.10.5 and our interpreter, and in both builds (`just mgba-diff`,
2026-11-23).
