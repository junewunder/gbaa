/* c-div (c/div/SCENE.md): C's / and % on signed and unsigned values, which
 * the compilers turn into calls of libgcc's division (agbcc: __divsi3,
 * __udivsi3, __modsi3, __umodsi3; modern GCC: __aeabi_idiv, __aeabi_uidiv,
 * __aeabi_idivmod, __aeabi_uidivmod). Divisors are constants, values
 * that may be -1, and values the lift can't prove aren't 0
 * (native/lift-libgcc-div). The dividends go negative. A square moves as
 * c/template's does, coloured from the results. */
#include "gba.h"
#include "draw.h"

#define SIZE 16

s32 gX, gY;
s32 gDx = 1, gDy = 1;
u32 gFrame;
u32 gSum;

/* A signed quotient by a divisor that may be -1 (never 0). */
NOINLINE s32 sdiv(s32 a, s32 b)
{
    return a / b;
}

/* A signed remainder by a divisor never 0. */
NOINLINE s32 smod(s32 a, s32 b)
{
    return a % b;
}

/* An unsigned quotient and remainder by divisors never 0. */
NOINLINE u32 udiv(u32 a, u32 b)
{
    return a / b;
}

NOINLINE u32 umod(u32 a, u32 b)
{
    return a % b;
}

/* Divisions by constants, in line. */
NOINLINE u32 consts(s32 c, u32 f)
{
    s32 q = c / 7;
    s32 r = c % 5;
    u32 uq = f / 3;
    u32 ur = f % 10;

    return (u32)(q + r) + uq + ur;
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
    s32 cx, cy, q, r, m1;
    u32 f, uq, ur, k;

    VIDEO_MODE3_VBLANK_IRQ();
    for (;;) {
        VBlankIntrWait();
        fill_rect(gX, gY, SIZE, SIZE, 0);
        step();
        f = gFrame;
        cx = gX - 112;
        cy = gY - 72;
        /* -1, 1, 1, 3: a divisor that may be -1. */
        m1 = ((s32)(f & 3) - 1) | 1;
        q = sdiv(cx * 9, m1 * ((s32)(f & 4) + 3));
        /* A positive divisor: modern GCC's __aeabi_idivmod reads its
         * cmp's N flag in its callee, which the lift doesn't pass yet
         * (native/lift-call-entry-flags). */
        r = smod(cy * 13, (s32)(f >> 3 & 7) + 2);
        uq = udiv(f * 7, (f & 15) + 1);
        ur = umod(f, (f >> 2 & 7) + 3);
        k = consts(cx - cy, f);
        gSum += (u32)q + (u32)r + uq + ur + k;
        fill_rect(gX, gY, SIZE, SIZE,
                  RGB((u32)q & 31, (u32)(r + 16) & 31, (uq + ur + k) & 31));
        gFrame++;
    }
}
