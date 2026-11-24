# c-list: what it draws

`just c-tests` builds this into `$GBAA_DATA/c/c-list-agbcc.gba` and
`c-list-gcc.gba`; both draw the same frames. The lifter's check runs both
(`just lift-check`, tables `c-list-agbcc` and `c-list-gcc`, on
`units/c-list-<cc>-600.toml`).

Pointers held in memory and followed (unit native/c-list-test-program):
ten `struct Node`s in a static pool (`gPool`, IWRAM), each with a `next`
link, an `act` function pointer, a position, a speed, a colour, a life and
a size. `pool_init` links them all into a free list (`gFree`) through
`next`; `pool_get` pops one, `pool_put` pushes one back. The live nodes are
a second list through the same field, `gLive` (a `struct List`: its head
and a count), kept sorted by x: `list_insert` walks a `struct Node **`
from `&l->head` to the link to change; `list_remove_dead` walks with the
node before, unlinks every node whose life ran out, erases it and gives it
back to the pool; `list_run` erases each node, calls its `act`, draws it,
and plots a dot halfway to the next node (`n->next->x`); `list_length`
counts the nodes by walking.

Every 8 frames `spawn` takes a node (none when the pool is empty) with one
of three acts from a const table: `act_slide` moves it left and right,
`act_fall` drops it until it lands at y 120 (and dies there),
`act_blink` turns it white and magenta every 4 frames; each lives 40 to
103 frames. The boxes are 4, 6, 8 or 10 pixels square, coloured by kind.
Below them, y 130 to 145, the halfway dots, cleared every 64 frames. At
y 152 a yellow bar 8 pixels per node as walked, at y 156 a cyan one 8 per
node as counted, and a square in the top right corner coloured by how many
were born and died.

Mode 3, BG2 on, black. Frame 600: hash `d24942cc384e478d`, the same in
mGBA 0.10.5 and our interpreter, and in both builds (`just mgba-diff`,
2026-11-24).
