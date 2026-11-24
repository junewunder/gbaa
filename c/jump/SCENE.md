# c-jump: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-jump-agbcc.gba` and
`c-jump-gcc.gba`; both draw the same frames. Its `setjmp` and `longjmp`
(`setjmp.s`) are the usual ARM pair behind Thumb `bx pc` entries, the shape
newlib gives a GBA game (moonbeam's Duktape throws with it): `stmia r0!,
{r4-r12, sp, lr}` and its `ldmia`.

Each frame, with `limit` the frame count mod 13: the outer `setjmp`
(`frame`) arms; unless `limit` is 12, `guarded` arms the inner one and
`dig` recurses from depth 0. At depth `limit` it throws to the outer catch
with the depth (1 for depth 0, as `longjmp` makes a 0); on odd limits past
7 it throws to the inner catch at depth 7 first. Mode 3, BG2 on, bars 6
pixels high from x 4, each row cleared first:

- y 20, green (`0x23E0`): the value `guarded` returned, or `seed + 7` on
  the frames with no search (`seed` the frame count's low five bits); the
  inner catch returns a value it kept across its `setjmp` (`seed * 5 + 3`)
  plus 40;
- y 30, orange (`0x011F`): after an outer catch, 20 per depth caught plus
  `seed`;
- y 40, blue (`0x7D08`): the deepest depth reached, 16 pixels each;
- y 50, yellow: outer catches so far (low 7 bits); y 60, magenta: inner
  catches so far (low 7 bits).

Frame 600: hash `c65f548e64c1e8f9`, the same in mGBA 0.10.5 and our
interpreter for both builds (`just mgba-diff`, 2026-11-24).
