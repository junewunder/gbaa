/* c-bits (c/bits/SCENE.md): structs with bitfields, as games pack an
 * object's flags and small counters into a byte or a halfword. Each ball
 * keeps its direction, speed and colour in one byte's bitfields and its
 * bounce count, trail and visibility in a halfword's; every access to them
 * goes through a pointer to the ball. Writes are constant (`b->up = 0`) and
 * computed (`b->left = !b->left`, `b->bounces++`). A global, the scene's
 * clock, keeps its own bitfields, every access to them at its constant
 * address (`gClock.tick++`), not through a pointer. */
#include "gba.h"
#include "draw.h"

#define BACKDROP 0
#define NUM_BALLS 4
#define SIZE 8

struct Ball {
    s16 x, y;
    u8 left : 1;
    u8 up : 1;
    u8 speed : 2;
    u8 hue : 4;
    u8 id;
    u16 bounces : 6;
    u16 trail : 3;
    u16 visible : 1;
    u16 shade : 5;
};

/* The clock: the beats so far, frames in the beat and the ball the next
 * blink turns over. The halfword's fields come first: agbcc lets a
 * bitfield straddle its type's unit (`u16 beats : 10` after a byte's
 * fields), which pret-names doesn't place. */
struct Clock {
    u16 beats : 10;
    u16 level : 6;
    u8 tick : 4;
    u8 next : 2;
    u8 run : 1;
    u8 flash : 1;
};

struct Ball gBalls[NUM_BALLS];
struct Clock gClock;
u32 gFrame;

static const u16 sColors[16] = {
    RGB(31, 0, 0), RGB(31, 16, 0), RGB(31, 31, 0), RGB(16, 31, 0),
    RGB(0, 31, 0), RGB(0, 31, 16), RGB(0, 31, 31), RGB(0, 16, 31),
    RGB(0, 0, 31), RGB(16, 0, 31), RGB(31, 0, 31), RGB(31, 0, 16),
    RGB(31, 31, 31), RGB(24, 24, 24), RGB(16, 16, 16), RGB(31, 24, 16),
};

/* One field each: a compiler joins writes to fields sharing bytes into
 * one load and store, which these calls keep apart. */
NOINLINE void ball_aim(struct Ball *b, s32 i)
{
    b->left = i & 1;
}

NOINLINE void ball_paint(struct Ball *b, s32 i)
{
    b->hue = i * 3;
}

NOINLINE void ball_show(struct Ball *b)
{
    b->visible = 1;
}

NOINLINE void ball_init(struct Ball *b, s32 i)
{
    b->x = 20 + i * 50;
    b->y = 16 + i * 30;
    b->id = i;
    ball_aim(b, i);
    b->up = 0;
    ball_paint(b, i);
    b->speed = (i & 1) + 1;
    ball_show(b);
    b->shade = 31;
}

NOINLINE void ball_move(struct Ball *b)
{
    s32 step = b->speed;

    if (b->left)
        b->x -= step;
    else
        b->x += step;
    if (b->up)
        b->y -= 1;
    else
        b->y += 1;
    if (b->x <= 0 || b->x + SIZE >= SCREEN_W) {
        b->left = !b->left;
        b->bounces++;
        b->hue = b->hue + 1;
    }
    if (b->y <= 8 || b->y + SIZE >= SCREEN_H) {
        b->up = !b->up;
        b->bounces++;
        b->shade = b->shade - 3;
    }
    b->trail = b->trail + 1;
}

NOINLINE void ball_draw(const struct Ball *b, s32 i)
{
    u16 c = sColors[b->hue];

    if (b->visible)
        fill_rect(b->x, b->y, SIZE, SIZE, c);
    /* Its bounces as a bar along the top, its trail as a dot under it. */
    fill_rect(i * 60, 0, 63, 2, BACKDROP);
    fill_rect(i * 60, 0, b->bounces, 2, RGB(b->shade, b->shade, b->shade));
    fill_rect(i * 60, 4, 8, 2, BACKDROP);
    plot(b->id * 60 + b->trail, 4, c);
}

NOINLINE void ball_blink(struct Ball *b)
{
    b->visible ^= 1;
    if (b->speed == 3)
        b->speed = 1;
    else
        b->speed++;
}

/* A beat each 16 frames: the clock's bar flashes and fills one more. */
NOINLINE void clock_step(void)
{
    if (!gClock.run)
        return;
    gClock.tick = gClock.tick + 1;
    if (gClock.tick == 0) {
        gClock.flash = !gClock.flash;
        gClock.beats++;
        gClock.level = gClock.beats >> 2;
    }
}

NOINLINE void clock_draw(void)
{
    u16 c = gClock.flash ? RGB(31, 31, 31) : RGB(8, 8, 8);

    fill_rect(0, 6, 16, 1, BACKDROP);
    fill_rect(0, 6, gClock.tick, 1, c);
    fill_rect(0, 7, 64, 1, BACKDROP);
    fill_rect(0, 7, gClock.level, 1, c);
}

void AgbMain(void)
{
    s32 i;
    struct Ball *b;

    for (i = 0; i < NUM_BALLS; i++)
        ball_init(&gBalls[i], i);
    gClock.next = 0;
    gClock.run = 1;
    VIDEO_MODE3_VBLANK_IRQ();
    for (;;) {
        VBlankIntrWait();
        for (i = 0, b = gBalls; i < NUM_BALLS; i++, b++) {
            fill_rect(b->x, b->y, SIZE, SIZE, BACKDROP);
            ball_move(b);
            ball_draw(b, i);
        }
        clock_step();
        clock_draw();
        if ((gFrame & 63) == 0) {
            ball_blink(&gBalls[gClock.next]);
            gClock.next++;
        }
        gFrame++;
    }
}
