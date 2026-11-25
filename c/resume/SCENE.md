# c-resume: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-resume-agbcc.gba` and
`c-resume-gcc.gba`; both draw the same frames. Its two restores
(`resume.s`) load sp and pc from a block a function of the program filled,
not setjmp's buffer (native/lift-restore-saved-block): `bail` loads sp and
the return site `guard_call` kept in `gGuard` (`ldr sp, [r12, #4]; ldr pc,
[r12, #8]`, as an OAM builder gives up on a full buffer), and `skip` loads
sp and the loop's registers `run_list` keeps in `gLoop`, then jumps back
into the loop at its call's return site (`ldr sp, [r11, #12]; b`, as an
entity loop goes on after an entity deletes itself). ARM code behind Thumb
`bx pc` entries.

Each frame: `guard_call` runs `search` down from depth 0 with the limit
the frame count's low four bits: at the limit it bails out with `depth * 3
+ 1`; past 9 it returns its sum. Then `run_list` sums `step(i)` for `i`
below the frame count's low three bits plus one: each item with `(i +
frame) & 3` 0 is skipped from `nudge` below it with `i + 20`, and on odd
frames each with it 1 from `step` itself with `i * 2`; others give `i * 3 +
1`. Mode 3, BG2 on, bars 6 pixels high from x 4, each row cleared first:

- y 20, green (`0x23E0`): what `guard_call` returned;
- y 30, blue (`0x7D08`): the deepest depth `search` reached, 16 pixels each;
- y 40, orange (`0x011F`): `run_list`'s sum, 2 pixels each;
- y 50, yellow: bails so far (low 7 bits); y 60, magenta: skips so far
  (low 7 bits).

Frame 600: hash `cfb36ed68e58a109`, the same in mGBA 0.10.5 and our
interpreter for both builds (`just mgba-diff`, 2026-11-24).
