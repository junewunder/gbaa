/* c-loops (c/loops/SCENE.md): counted loops as games write them, most
 * counting to a bound that isn't a constant: a parameter, a global, a
 * struct's field. Nested loops, a loop counting down, one that breaks out
 * early, one that skips rounds, one with a u8 counter (which wraps as a
 * u8 does) and one starting from a value. Each draws a column of bars. */
#include "gba.h"
#include "draw.h"

#define BACKDROP 0
#define NUM_BARS 12

struct Row {
    u8 count;
    u8 hue;
    u16 len[NUM_BARS];
};

struct Row gRow;
u8 gBarCount;
u32 gFrame;
u16 gLens[NUM_BARS];

static const u16 sColors[8] = {
    RGB(31, 0, 0), RGB(31, 16, 0), RGB(31, 31, 0), RGB(0, 31, 0),
    RGB(0, 31, 31), RGB(0, 0, 31), RGB(31, 0, 31), RGB(31, 31, 31),
};

NOINLINE void bar(s32 x, s32 y, s32 len, s32 hue)
{
    if (len > 24)
        len = 24;
    fill_rect(x, y, 24, 2, BACKDROP);
    fill_rect(x, y, len, 2, sColors[hue & 7]);
}

/* The bound a parameter. */
NOINLINE s32 sum(const u16 *a, s32 n)
{
    s32 i, s = 0;

    for (i = 0; i < n; i++)
        s += a[i];
    return s;
}

/* The bound a global (a u8), a call in the body. */
NOINLINE void draw_global(void)
{
    s32 i;

    for (i = 0; i < gBarCount; i++)
        bar(0, 8 + i * 6, gLens[i], i);
}

/* The bound a field, through a pointer. */
NOINLINE void draw_row(const struct Row *r)
{
    s32 i;

    for (i = 0; i < r->count; i++)
        bar(48, 8 + i * 6, r->len[i], r->hue + i);
}

/* Counting down to 0 from a parameter. */
NOINLINE void draw_down(s32 n)
{
    s32 i;

    for (i = n - 1; i >= 0; i--)
        bar(96, 8 + i * 6, gLens[n - 1 - i], i + 2);
}

/* A break inside: the first bar at least `min` long. */
NOINLINE s32 first_long(s32 n, s32 min)
{
    s32 i;

    for (i = 0; i < n; i++) {
        if (gLens[i] >= min)
            break;
    }
    return i;
}

/* A continue inside: every other bar, of several blocks. */
NOINLINE void draw_odd(s32 n)
{
    s32 i;

    for (i = 0; i < n; i++) {
        if ((i & 1) == 0)
            continue;
        bar(144, 8 + i * 6, gLens[i] >> 1, i + 4);
    }
}

/* A u8 counter. */
NOINLINE void grow(u8 n)
{
    u8 i;

    for (i = 0; i < n; i++) {
        gLens[i] += 1 + (i & 3);
        if (gLens[i] > 40)
            gLens[i] = 1;
    }
}

/* Nested, the inner from a value to a bound. */
NOINLINE void grid(s32 rows, s32 from, s32 to)
{
    s32 i, j;

    for (i = 0; i < rows; i++)
        for (j = from; j < to; j++)
            fill_rect(192 + j * 4, 8 + i * 6, 3, 3, sColors[(i + j + (gFrame >> 3)) & 7]);
}

void AgbMain(void)
{
    s32 i, total, at;

    for (i = 0; i < NUM_BARS; i++)
        gLens[i] = 3 * i + 1;
    gRow.hue = 1;
    VIDEO_MODE3_VBLANK_IRQ();
    for (;;) {
        VBlankIntrWait();
        gBarCount = 4 + ((gFrame >> 4) & 7);
        gRow.count = NUM_BARS - gBarCount + 1;
        for (i = 0; i < gRow.count; i++)
            gRow.len[i] = gLens[NUM_BARS - 1 - i];
        /* Half the bars each frame, so drawing ends inside V-blank. */
        if (gFrame & 1) {
            draw_down(1 + ((gFrame >> 1) & 7));
            draw_odd(gBarCount);
        } else {
            draw_global();
            draw_row(&gRow);
        }
        grid(1 + ((gFrame >> 5) & 3), gFrame & 1, 8 - (gFrame & 1));
        total = sum(gLens, gBarCount);
        at = first_long(NUM_BARS, 20 + (gFrame & 15));
        fill_rect(0, 100, 240, 2, BACKDROP);
        fill_rect(0, 100, total & 0xff, 2, RGB(16, 16, 31));
        fill_rect(0, 108, 240, 2, BACKDROP);
        fill_rect(at * 20, 108, 20, 2, RGB(31, 16, 16));
        if ((gFrame & 3) == 0)
            grow(gBarCount);
        gFrame++;
    }
}
