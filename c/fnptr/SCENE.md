# c-fnptr: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-fnptr-agbcc.gba` and
`c-fnptr-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-fnptr-agbcc` and `c-fnptr-gcc`, on
`units/c-fnptr-<cc>-600.toml`).

Mode 3, BG2 on, black. For its first 60 frames only `main_cb_wait` runs (the
main callback), then `main_cb_run` calls each callback in `gCallbacks`:

- `cb_objects`: each object erased, updated and drawn through its kind's
  `sObjOps` entry: kind 0 a red 8x8 box bouncing off the edges, kind 1 a
  green cross (arms of 5) going round a square, kind 2 a blue 12x12 box
  that blinks white every 16 frames;
- `cb_spawn`: a new object every 90 frames (kinds 0, 1, 2 in turn) until
  there are six, then it takes itself off the list (`remove_callback`);
- `cb_beat`: a yellow bar at (4, 150), 4 high, a pixel longer every 16
  frames.

Frame 600: hash `77bab1e79577f255`, the same in mGBA 0.10.5 and our
interpreter (`just mgba-diff`, 2026-11-18).
