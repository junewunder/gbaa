/* c-heap (c/heap/SCENE.md): a heap, as games use one. A first-fit
 * allocator over an array in EWRAM (heap_alloc, heap_alloc_zeroed,
 * heap_free); three kinds of object allocated at three call sites, each
 * holding function pointers beside numbers: sprites (an update at +0), a
 * menu (two callbacks and a cursor) and short-lived effects (a count at +0,
 * where a sprite keeps its update, then a step function), which are freed
 * and their memory used again. */
#include "gba.h"
#include "draw.h"

#define HEAP_SIZE 0x800
#define MAX_SPRITES 4
#define MAX_EFFECTS 3

/* The allocator: blocks one after another in gHeap, each a header and its
 * bytes; the first free block that fits is split and taken. */
struct Block {
    struct Block *next;
    u32 size;
    u32 used;
};

EWRAM u8 gHeap[HEAP_SIZE];
struct Block *gHeapHead;

NOINLINE void heap_init(void)
{
    struct Block *b = (struct Block *)gHeap;

    b->next = 0;
    b->size = HEAP_SIZE - sizeof(struct Block);
    b->used = 0;
    gHeapHead = b;
}

NOINLINE void *heap_alloc(u32 size)
{
    struct Block *b, *rest;

    size = (size + 3) & ~3u;
    for (b = gHeapHead; b != 0; b = b->next) {
        if (b->used || b->size < size)
            continue;
        if (b->size >= size + sizeof(struct Block) + 4) {
            rest = (struct Block *)((u8 *)(b + 1) + size);
            rest->next = b->next;
            rest->size = b->size - size - sizeof(struct Block);
            rest->used = 0;
            b->next = rest;
            b->size = size;
        }
        b->used = 1;
        return b + 1;
    }
    return 0;
}

NOINLINE void *heap_alloc_zeroed(u32 size)
{
    u8 *p = heap_alloc(size);
    u32 i;

    if (p != 0) {
        for (i = 0; i < size; i++)
            p[i] = 0;
    }
    return p;
}

NOINLINE void heap_free(void *p)
{
    struct Block *b = (struct Block *)p - 1;
    struct Block *next = b->next;

    b->used = 0;
    /* Joined with the block after it, if that one is free too. */
    if (next != 0 && !next->used) {
        b->size += sizeof(struct Block) + next->size;
        b->next = next->next;
    }
}

struct Sprite {
    void (*update)(struct Sprite *s);
    s16 x, y;
    s16 dx, dy;
    u16 color;
    u8 size;
    u8 timer;
};

struct Menu {
    void (*draw)(struct Menu *m);
    void (*on_wrap)(void);
    u8 cursor;
    u8 count;
    u16 color;
};

struct Effect {
    u32 life;
    void (*step)(struct Effect *e);
    s16 x, y;
};

struct Sprite *gSprites[MAX_SPRITES];
struct Effect *gEffects[MAX_EFFECTS];
struct Menu *gMenu;
u32 gFrame;
u32 gWraps;
u32 gFreed;

static void sprite_bounce(struct Sprite *s)
{
    s->x += s->dx;
    s->y += s->dy;
    if (s->x <= 0 || s->x + s->size >= SCREEN_W)
        s->dx = -s->dx;
    if (s->y <= 0 || s->y + s->size >= SCREEN_H - 16)
        s->dy = -s->dy;
}

static void sprite_blink(struct Sprite *s)
{
    s->timer++;
    if (s->timer & 16)
        s->color = RGB(31, 31, 31);
    else
        s->color = RGB(8, 8, 31);
}

NOINLINE struct Sprite *new_sprite(void (*update)(struct Sprite *s), s16 x, s16 y, u16 color)
{
    struct Sprite *s = heap_alloc_zeroed(sizeof(struct Sprite));

    s->update = update;
    s->x = x;
    s->y = y;
    s->dx = 1;
    s->dy = 1;
    s->color = color;
    s->size = 10;
    return s;
}

static void menu_draw_bar(struct Menu *m)
{
    fill_rect(0, 148, SCREEN_W, 12, 0);
    fill_rect(8 + m->cursor * 24, 150, 16, 8, m->color);
}

static void menu_wrapped(void)
{
    gWraps++;
    fill_rect(232, 0, 8, 8, (u16)(gWraps * 5));
}

NOINLINE struct Menu *new_menu(void)
{
    struct Menu *m = heap_alloc(sizeof(struct Menu));

    m->draw = menu_draw_bar;
    m->on_wrap = menu_wrapped;
    m->cursor = 0;
    m->count = 8;
    m->color = RGB(31, 24, 0);
    return m;
}

static void effect_grow(struct Effect *e)
{
    fill_rect(e->x, e->y, 20 - e->life, 2, RGB(0, 31, 16));
}

static void effect_fall(struct Effect *e)
{
    plot(e->x, e->y + 20 - e->life, RGB(31, 0, 31));
}

NOINLINE void new_effect(u32 slot, void (*step)(struct Effect *e), s16 x, s16 y)
{
    struct Effect *e = heap_alloc(sizeof(struct Effect));

    e->life = 20;
    e->step = step;
    e->x = x;
    e->y = y;
    gEffects[slot] = e;
}

static void run_effects(void)
{
    u32 i;
    struct Effect *e;

    for (i = 0; i < MAX_EFFECTS; i++) {
        e = gEffects[i];
        if (e == 0)
            continue;
        e->step(e);
        if (--e->life == 0) {
            heap_free(e);
            gEffects[i] = 0;
            gFreed++;
        }
    }
}

static void run_sprites(void)
{
    u32 i;
    struct Sprite *s;

    for (i = 0; i < MAX_SPRITES; i++) {
        s = gSprites[i];
        if (s == 0)
            continue;
        fill_rect(s->x, s->y, s->size, s->size, 0);
        s->update(s);
        fill_rect(s->x, s->y, s->size, s->size, s->color);
    }
}

static void run_menu(void)
{
    struct Menu *m = gMenu;

    if ((gFrame & 31) == 0) {
        if (++m->cursor == m->count) {
            m->cursor = 0;
            m->on_wrap();
        }
        m->draw(m);
    }
}

void AgbMain(void)
{
    u32 slot = 0;

    VIDEO_MODE3_VBLANK_IRQ();
    heap_init();
    gMenu = new_menu();
    gSprites[0] = new_sprite(sprite_bounce, 20, 20, RGB(31, 8, 8));
    gSprites[1] = new_sprite(sprite_bounce, 150, 60, RGB(8, 31, 8));
    gSprites[2] = new_sprite(sprite_blink, 200, 100, RGB(8, 8, 31));
    for (;;) {
        VBlankIntrWait();
        run_sprites();
        run_menu();
        /* A new effect every 16 frames, in the next slot that is empty:
         * they live 20, so two are alive at once and their memory is
         * freed and taken again. */
        if ((gFrame & 15) == 0 && gEffects[slot] == 0) {
            new_effect(slot, (gFrame & 16) ? effect_grow : effect_fall,
                       (s16)(16 + (gFrame & 127)), (s16)(110 + slot * 8));
            if (++slot == MAX_EFFECTS)
                slot = 0;
        }
        run_effects();
        gFrame++;
    }
}
