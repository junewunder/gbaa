# c-lent: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-lent-agbcc.gba` and
`c-lent-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-lent-agbcc` and `c-lent-gcc`, on
`units/c-lent-<cc>-600.toml`).

Mode 3, BG2 on, black. A 16x16 square moves as c/template's does, one
pixel right and down a frame from (1, 1), turning at the screen's edges.
Its colour is red 31, green the frame count over 16 (low 5 bits), blue 0:
`stamp` writes that count into the last byte of an 8-byte struct on its
stack and passes the struct's address to `stamp_mark`, which reads only
that byte back. The lift lends the struct (native/lift-lent-print), and
the bytes lent are that byte alone, so the address passed lies before
them (native/emerald-lent-local-stop: Emerald's SiiRtcReset hands
SiiRtcSetStatus its status so).

Frame 600: hash `ebde5dc81734f725`, the same in mGBA 0.10.5 and our
interpreter (`just mgba-diff`, 2026-11-25).
