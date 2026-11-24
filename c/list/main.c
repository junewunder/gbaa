/* c-list (c/list/SCENE.md): a linked list over a node pool, as games keep
 * their objects. Nodes come from a static pool (pool_get, pool_put over a
 * free list linked through `next`); the live ones are linked through the
 * same field, kept sorted by x: walked each frame, one inserted every few
 * frames (a walk through a pointer to the link to change) and the dead
 * ones removed (a walk with the one before). Each node's `act`, a function
 * pointer, is called once a frame, and each node is drawn with a dot
 * halfway to the next one. */
#include "gba.h"
#include "draw.h"

#define POOL_SIZE 10

struct Node {
    struct Node *next;
    void (*act)(struct Node *n);
    s16 x, y;
    s16 dx, dy;
    u16 color;
    u8 life;
    u8 size;
};

struct List {
    struct Node *head;
    u32 count;
};

struct Node gPool[POOL_SIZE];
struct Node *gFree;
struct List gLive;
u32 gFrame;
u32 gBorn;
u32 gDied;

NOINLINE void pool_init(void)
{
    u32 i;

    gFree = 0;
    for (i = 0; i < POOL_SIZE; i++) {
        gPool[i].next = gFree;
        gFree = &gPool[i];
    }
}

NOINLINE struct Node *pool_get(void)
{
    struct Node *n = gFree;

    if (n != 0)
        gFree = n->next;
    return n;
}

NOINLINE void pool_put(struct Node *n)
{
    n->next = gFree;
    gFree = n;
}

/* Linked in before the first node with a larger x: a walk through the
 * address of the link that will point at it. */
NOINLINE void list_insert(struct List *l, struct Node *n)
{
    struct Node **link = &l->head;

    while (*link != 0 && (*link)->x <= n->x)
        link = &(*link)->next;
    n->next = *link;
    *link = n;
    l->count++;
}

/* Every node whose life ran out, unlinked, erased and given back. */
NOINLINE void list_remove_dead(struct List *l)
{
    struct Node *prev = 0;
    struct Node *n = l->head;
    struct Node *next;

    while (n != 0) {
        next = n->next;
        if (n->life == 0) {
            if (prev == 0)
                l->head = next;
            else
                prev->next = next;
            fill_rect(n->x, n->y, n->size, n->size, 0);
            pool_put(n);
            l->count--;
            gDied++;
        } else {
            prev = n;
        }
        n = next;
    }
}

static void act_slide(struct Node *n)
{
    n->x += n->dx;
    if (n->x <= 0 || n->x + n->size >= SCREEN_W)
        n->dx = -n->dx;
    n->life--;
}

static void act_fall(struct Node *n)
{
    n->y += n->dy;
    if (n->y + n->size >= 120) {
        n->y = 120 - n->size;
        n->life = 0;
    } else {
        n->life--;
    }
}

static void act_blink(struct Node *n)
{
    if (n->life & 4)
        n->color = RGB(31, 31, 31);
    else
        n->color = RGB(31, 0, 16);
    n->life--;
}

static void (*const kActs[4])(struct Node *n) = {
    act_slide, act_fall, act_blink, act_slide,
};

static const u16 kColors[4] = {
    RGB(31, 8, 0), RGB(0, 24, 31), RGB(31, 0, 16), RGB(8, 31, 8),
};

NOINLINE void spawn(u32 seed)
{
    struct Node *n = pool_get();
    u32 kind = seed & 3;

    if (n == 0)
        return;
    n->act = kActs[kind];
    n->x = (s16)(8 + ((seed * 37) & 127) + (seed & 63));
    n->y = (s16)(8 + ((seed * 13) & 63));
    n->dx = (seed & 8) ? -2 : 1;
    n->dy = 1 + (s16)(kind & 1);
    n->color = kColors[kind];
    n->life = (u8)(40 + ((seed * 7) & 63));
    n->size = (u8)(4 + (kind << 1));
    list_insert(&gLive, n);
    gBorn++;
}

/* The walk: each node erased, moved by its own act, drawn, and a dot
 * halfway to the one after it. */
NOINLINE void list_run(struct List *l)
{
    struct Node *n;

    for (n = l->head; n != 0; n = n->next) {
        fill_rect(n->x, n->y, n->size, n->size, 0);
        n->act(n);
        fill_rect(n->x, n->y, n->size, n->size, n->color);
        if (n->next != 0)
            plot((n->x + n->next->x) >> 1, 130 + (n->y >> 3), n->color);
    }
}

/* How many nodes the list holds as walked, against its count: a bar of
 * each at the bottom. */
NOINLINE u32 list_length(const struct List *l)
{
    const struct Node *n;
    u32 len = 0;

    for (n = l->head; n != 0; n = n->next)
        len++;
    return len;
}

void AgbMain(void)
{
    u32 len;

    VIDEO_MODE3_VBLANK_IRQ();
    pool_init();
    spawn(5);
    spawn(10);
    for (;;) {
        VBlankIntrWait();
        if ((gFrame & 63) == 0)
            fill_rect(0, 128, SCREEN_W, 24, 0);
        list_run(&gLive);
        list_remove_dead(&gLive);
        if ((gFrame & 7) == 0)
            spawn(gFrame + gDied);
        len = list_length(&gLive);
        fill_rect(0, 152, len << 3, 4, RGB(31, 31, 0));
        fill_rect(len << 3, 152, (POOL_SIZE - len) << 3, 4, 0);
        fill_rect(0, 156, gLive.count << 3, 4, RGB(0, 31, 31));
        fill_rect(gLive.count << 3, 156, (POOL_SIZE - gLive.count) << 3, 4, 0);
        fill_rect(232, 0, 8, 8, (u16)(gBorn * 3 + gDied));
        gFrame++;
    }
}
