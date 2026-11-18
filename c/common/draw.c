/* Drawing for the C test programs (c/README.md): mode 3, one halfword per
 * pixel, halfword stores only. */
#include "gba.h"
#include "draw.h"

void fill_rect(s32 x, s32 y, s32 w, s32 h, u16 color)
{
    s32 i, j;
    volatile u16 *row;

    if (x < 0) { w += x; x = 0; }
    if (y < 0) { h += y; y = 0; }
    if (x + w > SCREEN_W) w = SCREEN_W - x;
    if (y + h > SCREEN_H) h = SCREEN_H - y;
    for (j = 0; j < h; j++) {
        row = VRAM + (y + j) * SCREEN_W + x;
        for (i = 0; i < w; i++)
            row[i] = color;
    }
}

void plot(s32 x, s32 y, u16 color)
{
    if (x >= 0 && x < SCREEN_W && y >= 0 && y < SCREEN_H)
        VRAM[y * SCREEN_W + x] = color;
}
