/* c-borrow's callees (c/borrow/SCENE.md): a pointer tested for null before
 * its field is read, and an array in a struct written and read at an
 * index. */
#include "borrow.h"

u32 mark_or(const struct Mark *m, u32 d)
{
    if (m == 0)
        return d;
    return m->mark & 0x1f;
}

void cell_set(struct Cells *c, u32 i, u32 v)
{
    c->cell[i & 7] = v;
}

u32 cell_get(const struct Cells *c, u32 i)
{
    return c->cell[i & 7];
}
