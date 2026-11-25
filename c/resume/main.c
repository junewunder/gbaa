/* c-resume (c/resume/SCENE.md): two context restores that load sp and pc
 * from a block of their own (resume.s), not setjmp's ldm. A search bails
 * out to its guard, which kept its sp and the return site of its call in
 * gGuard (as an OAM builder gives up on a full buffer); a loop's step drops
 * the rest of itself and the loop goes on with the next item, from the sp
 * and registers the loop keeps in gLoop (as an entity loop goes on after
 * an entity deletes itself). Each frame draws what came back. */
#include "gba.h"
#include "draw.h"
#include "resume.h"

/* resume.s's blocks: gGuard[1] sp, [2] the resume site; gLoop[0] the item
 * under way, [1] the sum so far, [3] sp, [4] the count. */
u32 gGuard[3];
u32 gLoop[5];
u32 gFrame;
u32 gBails;
u32 gSkips;
u32 gDeepest;

/* Down one level a call, bailing out at the limit with its depth. */
NOINLINE u32 search(u32 n, u32 limit)
{
    if (n > gDeepest)
        gDeepest = n;
    if (n == limit) {
        gBails++;
        bail(n * 3 + 1);
    }
    if (n >= 9)
        return n;
    return search(n + 1, limit) + n * 2;
}

/* A step's skip from a call below it. */
NOINLINE u32 nudge(u32 i)
{
    gSkips++;
    skip(i + 20);
    return 0;
}

/* One item of run_list's loop: skipped from below on every fourth, and
 * from itself on odd frames' next ones. */
NOINLINE u32 step(u32 i)
{
    u32 k = (i + gFrame) & 3;

    if (k == 0)
        return nudge(i) + 1;
    if (k == 1 && (gFrame & 1)) {
        gSkips++;
        skip(i * 2);
    }
    return i * 3 + 1;
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
    u32 got;
    u32 sum;

    gDeepest = 0;
    got = guard_call(gFrame & 15);
    sum = run_list((gFrame & 7) + 1);
    bar(20, got, RGB(0, 31, 8));
    bar(30, gDeepest * 16, RGB(8, 8, 31));
    bar(40, sum * 2, RGB(31, 8, 0));
    bar(50, gBails & 127, RGB(31, 31, 0));
    bar(60, gSkips & 127, RGB(31, 0, 31));
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
