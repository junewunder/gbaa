/* c-chain (c/chain/SCENE.md): structs passed by pointer down a call chain,
 * as games pass their sprites and contexts. The frame builds a context on
 * its stack and hands its address down three calls; each body's rectangle
 * goes by pointer to the collision code; a rectangle comes back by value
 * (through the hidden pointer the ABI passes for a struct over a word). */
#include "gba.h"
#include "draw.h"

#define BACKDROP 0
#define NUM_BODIES 4

struct Rect {
    s16 x, y, w, h;
};

struct Body {
    struct Rect r;
    s16 vx, vy;
    u16 color;
    u16 hits;
};

struct Ctx {
    struct Rect bounds;
    u32 frame;
    u16 hits;
    u16 drawn;
    struct Body *last_hit;
};

struct Body gBodies[NUM_BODIES];
u32 gFrame;
u32 gTotalHits;
struct Rect gUnion;
u16 gLastHitIndex;

NOINLINE s32 overlaps(const struct Rect *a, const struct Rect *b)
{
    return a->x < b->x + b->w && b->x < a->x + a->w
        && a->y < b->y + b->h && b->y < a->y + a->h;
}

NOINLINE struct Rect rect_union(const struct Rect *a, const struct Rect *b)
{
    struct Rect u;
    s16 x2 = a->x + a->w, y2 = a->y + a->h;

    u.x = a->x < b->x ? a->x : b->x;
    u.y = a->y < b->y ? a->y : b->y;
    if (b->x + b->w > x2)
        x2 = b->x + b->w;
    if (b->y + b->h > y2)
        y2 = b->y + b->h;
    u.w = x2 - u.x;
    u.h = y2 - u.y;
    return u;
}

/* The bottom of the chain: a wall hit turns the body round. */
NOINLINE void bounce(struct Ctx *ctx, struct Body *b)
{
    const struct Rect *bd = &ctx->bounds;

    if (b->r.x < bd->x || b->r.x + b->r.w > bd->x + bd->w) {
        b->vx = -b->vx;
        b->r.x += 2 * b->vx;
        b->hits++;
        ctx->hits++;
        ctx->last_hit = b;
    }
    if (b->r.y < bd->y || b->r.y + b->r.h > bd->y + bd->h) {
        b->vy = -b->vy;
        b->r.y += 2 * b->vy;
        b->hits++;
        ctx->hits++;
        ctx->last_hit = b;
    }
}

NOINLINE void move_body(struct Ctx *ctx, struct Body *b)
{
    fill_rect(b->r.x, b->r.y, b->r.w, b->r.h, BACKDROP);
    b->r.x += b->vx;
    b->r.y += b->vy;
    bounce(ctx, b);
}

NOINLINE void draw_body(struct Ctx *ctx, const struct Body *b)
{
    fill_rect(b->r.x, b->r.y, b->r.w, b->r.h, b->color);
    ctx->drawn++;
}

/* The middle: every body moved, then each pair that overlaps marked. */
NOINLINE void step_world(struct Ctx *ctx, struct Body *bodies, u32 n)
{
    u32 i, j;

    for (i = 0; i < n; i++)
        move_body(ctx, &bodies[i]);
    for (i = 0; i < n; i++)
        for (j = i + 1; j < n; j++)
            if (overlaps(&bodies[i].r, &bodies[j].r)) {
                gUnion = rect_union(&bodies[i].r, &bodies[j].r);
                bodies[i].color ^= 0x7fff;
                bodies[j].color ^= 0x7fff;
            }
    for (i = 0; i < n; i++)
        draw_body(ctx, &bodies[i]);
}

NOINLINE void frame(u32 count)
{
    struct Ctx ctx;

    ctx.bounds.x = 8;
    ctx.bounds.y = 8;
    ctx.bounds.w = SCREEN_W - 16;
    ctx.bounds.h = 136;
    ctx.frame = count;
    ctx.hits = 0;
    ctx.drawn = 0;
    ctx.last_hit = 0;
    step_world(&ctx, gBodies, NUM_BODIES);
    gTotalHits += ctx.hits;
    if (ctx.last_hit)
        gLastHitIndex = ctx.last_hit - gBodies;
    fill_rect(4, 150, (gTotalHits & 63) * 2, 4, RGB(31, 31, 0));
    fill_rect(4 + gLastHitIndex * 8, 156, 6, 3, ctx.drawn == NUM_BODIES ? RGB(0, 31, 0) : RGB(31, 0, 0));
}

static void init_body(struct Body *b, s16 x, s16 y, s16 vx, s16 vy, u16 size, u16 color)
{
    b->r.x = x;
    b->r.y = y;
    b->r.w = size;
    b->r.h = size;
    b->vx = vx;
    b->vy = vy;
    b->color = color;
    b->hits = 0;
}

void AgbMain(void)
{
    VIDEO_MODE3_VBLANK_IRQ();
    init_body(&gBodies[0], 20, 20, 2, 1, 10, RGB(31, 16, 0));
    init_body(&gBodies[1], 200, 40, -1, 2, 14, RGB(0, 16, 31));
    init_body(&gBodies[2], 100, 100, 3, -1, 8, RGB(16, 31, 0));
    init_body(&gBodies[3], 60, 120, -2, -2, 12, RGB(31, 0, 16));
    for (;;) {
        VBlankIntrWait();
        frame(gFrame);
        gFrame++;
    }
}
