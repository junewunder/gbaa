/* The C template (c/README.md, c/template/SCENE.md): a square that moves
 * one pixel a frame and bounces off the screen's edges. Copy the folder to
 * start a C test program. */
#include "gba.h"
#include "draw.h"

#define SIZE 16

s32 gX, gY;
s32 gDx = 1, gDy = 1;
u32 gFrame;

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
    VIDEO_MODE3_VBLANK_IRQ();
    for (;;) {
        VBlankIntrWait();
        fill_rect(gX, gY, SIZE, SIZE, 0);
        step();
        fill_rect(gX, gY, SIZE, SIZE, RGB(31, 24, 0));
        gFrame++;
    }
}
