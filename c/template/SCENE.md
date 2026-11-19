# c-template: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-template-agbcc.gba` and
`c-template-gcc.gba`; both draw the same frames. Mode 3, BG2 on: a 16x16
square, colour `0x031F` (BGR555), starts at (1, 1) after the first V-blank
and moves one pixel right and down a frame, turning at the screen's edges;
the rest of the screen stays black. Frame 600: hash `1a689037bbefb525`,
the same in mGBA 0.10.5 and our interpreter (`just mgba-diff`, 2026-11-18).

Copying it: copy the folder to `c/<name>/`, change `main.c`, write this
file for it, and run `just c-tests`. Don't fill the whole screen before the
first `VBlankIntrWait`: that loop lands a frame later in mGBA than in our
interpreter (see c/README.md).
