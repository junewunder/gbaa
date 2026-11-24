/* c-jump (c/jump/SCENE.md): errors thrown with longjmp out of a call
 * chain to the setjmp that caught them, as a script engine throws (an
 * interpreter's error unwinding to its protected call). Each catch keeps
 * its jmp_buf on its own stack frame and a global pointer to it, as
 * Duktape does, the inner one put back as it was when it is done. Each frame a
 * search recurses down to a depth that changes with the frame; it throws
 * to the frame's outer catch when it reaches its limit, or to an inner
 * catch at depth 7 on odd limits, or returns normally every thirteenth
 * frame. Each catch draws a bar from what it caught and from a value it
 * kept in a register across the setjmp. */
#include "gba.h"
#include "draw.h"
#include "jmp.h"

jmp_buf *gOuter;
jmp_buf *gInner;
u32 gFrame;
u32 gOuterCaught;
u32 gInnerCaught;
u32 gDeepest;
u32 gSum;

/* Down one level a call, throwing at the limit (its depth, 1 for depth 0
 * as longjmp makes it) or, on an odd limit past 7, to the inner catch. */
NOINLINE u32 dig(u32 n, u32 limit)
{
    if (n > gDeepest)
        gDeepest = n;
    if (n == limit)
        longjmp(*gOuter, n);
    if (n == 7 && (limit & 1))
        longjmp(*gInner, 0);
    if (n >= 12)
        return n;
    return dig(n + 1, limit) + n * 3;
}

/* The inner catch: what it kept (in r4-r7 across the setjmp) and whether
 * it caught. */
NOINLINE u32 guarded(u32 limit, u32 seed)
{
    jmp_buf env;
    jmp_buf *outer = gInner;
    u32 keep = seed * 5 + 3;
    u32 got;

    gInner = &env;
    if (setjmp(env) != 0) {
        gInner = outer;
        gInnerCaught++;
        return keep + 40;
    }
    got = dig(0, limit);
    gInner = outer;
    return got + keep;
}

NOINLINE void bar(s32 row, s32 len, u16 color)
{
    if (len > 232)
        len = 232;
    fill_rect(4, row, 232, 6, 0);
    fill_rect(4, row, len, 6, color);
}

NOINLINE void frame(void)
{
    jmp_buf env;
    u32 limit = gFrame % 13;
    u32 seed = gFrame & 31;
    int r;

    gDeepest = 0;
    gOuter = &env;
    r = setjmp(env);
    if (r == 0) {
        if (limit == 12) {
            gSum = seed + 7;
        } else {
            gSum = guarded(limit, seed);
        }
        bar(20, gSum, RGB(0, 31, 8));
        bar(30, 0, 0);
    } else {
        gOuterCaught++;
        bar(20, 0, 0);
        bar(30, r * 20 + seed, RGB(31, 8, 0));
    }
    bar(40, gDeepest * 16, RGB(8, 8, 31));
    bar(50, gOuterCaught & 127, RGB(31, 31, 0));
    bar(60, gInnerCaught & 127, RGB(31, 0, 31));
}

void AgbMain(void)
{
    VIDEO_MODE3_VBLANK_IRQ();
    for (;;) {
        VBlankIntrWait();
        frame();
        gFrame++;
    }
}
