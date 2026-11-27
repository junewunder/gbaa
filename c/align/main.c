/* c-align (c/align/SCENE.md): loads through pointer parameters that only
 * their callers make aligned (native/lift-aligned-params). rd_pair
 * (align.s) is passed word addresses alone, by sum_pairs walking a table in
 * a loop, so the lift drops its loads' rotates; rd32 and rd16 are passed
 * odd addresses too, and read what the CPU's rotate gives. A square moves
 * as c/template's does, coloured from what they read. */
#include "gba.h"
#include "draw.h"

#define SIZE 16

u32 rd32(const void *p);
u32 rd16(const void *p);
u32 rd_pair(const u32 *p);

static const u32 kPairs[8] = {
    0x00000003, 0x00000004, 0x00000105, 0x00000006,
    0x00010007, 0x00000208, 0x01000009, 0x0000030a,
};

u32 gBytes[2] = { 0x1f0a1504, 0x03111c08 };
s32 gX, gY;
s32 gDx = 1, gDy = 1;
u32 gFrame;
u32 gSum;

NOINLINE u32 sum_pairs(const u32 *p, u32 n)
{
    u32 s = 0;

    while (n != 0) {
        s += rd_pair(p);
        p += 2;
        n--;
    }
    return s;
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
    u32 r, g, b;

    VIDEO_MODE3_VBLANK_IRQ();
    for (;;) {
        VBlankIntrWait();
        fill_rect(gX, gY, SIZE, SIZE, 0);
        step();
        r = rd32((const u8 *)gBytes + (gFrame & 3));
        g = rd16((const u8 *)gBytes + ((gFrame >> 2) & 3));
        b = sum_pairs(kPairs, 1 + ((gFrame >> 4) & 3));
        gSum += r ^ g ^ b;
        fill_rect(gX, gY, SIZE, SIZE, RGB(r & 31, g & 31, b & 31));
        gFrame++;
    }
}
