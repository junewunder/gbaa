/* c-borrow's callees (cells.c), compiled apart from their callers as a
 * game's files are: neither compiler sees that every caller passes a
 * stack address. */
#ifndef BORROW_H
#define BORROW_H

#include "gba.h"

struct Mark {
    u8 pad[3];
    u8 mark;
};

struct Cells {
    u8 cell[8];
};

u32 mark_or(const struct Mark *m, u32 d);
void cell_set(struct Cells *c, u32 i, u32 v);
u32 cell_get(const struct Cells *c, u32 i);

#endif
