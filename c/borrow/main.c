/* c-borrow (c/borrow/SCENE.md): stack structs lent to callees in another
 * file (cells.c) that test the pointer for null and index an array in it,
 * as a game's helpers do with a caller's buffer. A square moves as
 * c/template's does, coloured from what the callees give back. */
#include "gba.h"
#include "draw.h"
#include "borrow.h"

#define SIZE 16

s32 gX, gY;
s32 gDx = 1, gDy = 1;
u32 gFrame;
u32 gMarks;

NOINLINE u32 tint(u32 n)
{
    struct Mark m;
    struct Cells c;

    m.mark = n;
    cell_set(&c, n, mark_or(&m, 0));
    return cell_get(&c, n);
}

static void step(void)
{
    gX += gDx;
    gY += gDy;
    if (gX <= 0 || gX + SIZE >= SCREEN_W)
        gDx = -gDx;
    if (gY <= 0 || gY + SIZE >= SCREEN_H)
        gDy = -gDy;
}

void AgbMain(void)
{
    u32 m;

    VIDEO_MODE3_VBLANK_IRQ();
    for (;;) {
        VBlankIntrWait();
        fill_rect(gX, gY, SIZE, SIZE, 0);
        step();
        m = tint(gFrame >> 4);
        gMarks += m;
        fill_rect(gX, gY, SIZE, SIZE, RGB(31, m, 0));
        gFrame++;
    }
}
