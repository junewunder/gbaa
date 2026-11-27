# c-div: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-div-agbcc.gba` and
`c-div-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-div-agbcc` and `c-div-gcc`, on
`units/c-div-<cc>-600.toml`).

Mode 3, BG2 on, black. A 16x16 square moves as c/template's does, one
pixel right and down a frame from (1, 1), turning at the screen's edges.
Its colour comes from C's `/` and `%`, each a call of libgcc's division
(agbcc: `__divsi3`, `__modsi3`, `__udivsi3`, `__umodsi3`; modern GCC:
`__aeabi_idiv`, `__aeabi_idivmod`, `__aeabi_uidiv`, `__aeabi_uidivmod`
through Thumb veneers): red the low 5 bits of the square's x less 112,
times 9, over a divisor that is -1, 1 or 3 times 3 or 7 (`sdiv`); green 16
more than the y less 72, times 13, remainder 2 to 9 (`smod`); blue the low 5 bits of the frame count times 7 over 1 to 16
(`udiv`), plus the frame count remainder 3 to 10 (`umod`), plus
`consts`: the x less the y (it goes negative) over 7 and remainder 5,
signed,
and the frame count over 3 and remainder 10, unsigned. No divisor is 0.
The lift prints each call as `/` or `%` (native/lift-libgcc-div), the
signed ones whose divisor may be -1 by `wrapping_div` and `wrapping_rem`.

Frame 600: hash `9e94d70227375125`, the same in mGBA 0.10.5 and our
interpreter (`just mgba-diff`, 2026-11-27).
