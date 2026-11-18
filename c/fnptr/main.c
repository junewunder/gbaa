/* c-fnptr (c/fnptr/SCENE.md): function pointers two ways, as games use
 * them. Each object's kind picks its update and draw from a const table of
 * structs of function pointers in ROM (sObjOps); each frame runs a list of
 * callbacks in RAM (gCallbacks), which callbacks add to and take from, and
 * a main callback pointer (gMainCallback) that one state replaces with the
 * next. */
#include "gba.h"
#include "draw.h"

#define BACKDROP 0
#define MAX_OBJS 6
#define MAX_CALLBACKS 6

struct Obj {
    s16 x, y;
    s16 dx, dy;
    u16 color;
    u8 kind;
    u8 timer;
};

struct ObjOps {
    void (*update)(struct Obj *obj);
    void (*draw)(const struct Obj *obj, u16 color);
    u16 size;
    u16 tint;
};

typedef void (*Callback)(u32 frame);

struct Obj gObjs[MAX_OBJS];
u8 gNumObjs;
Callback gCallbacks[MAX_CALLBACKS];
u8 gNumCallbacks;
void (*gMainCallback)(void);
u32 gFrame;
u32 gBeats;
u32 gState;
u8 gSpawnTimer;
u8 gSpawnKind;

static void bounce_update(struct Obj *obj);
static void orbit_update(struct Obj *obj);
static void blink_update(struct Obj *obj);
static void draw_box(const struct Obj *obj, u16 color);
static void draw_cross(const struct Obj *obj, u16 color);

static const struct ObjOps sObjOps[] = {
    { bounce_update, draw_box, 8, RGB(31, 8, 8) },
    { orbit_update, draw_cross, 5, RGB(8, 31, 8) },
    { blink_update, draw_box, 12, RGB(8, 8, 31) },
};

static void bounce_update(struct Obj *obj)
{
    obj->x += obj->dx;
    obj->y += obj->dy;
    if (obj->x <= 0 || obj->x + 12 >= SCREEN_W)
        obj->dx = -obj->dx;
    if (obj->y <= 0 || obj->y + 12 >= SCREEN_H)
        obj->dy = -obj->dy;
}

static void orbit_update(struct Obj *obj)
{
    /* A square path: dx, dy turn a quarter every 32 frames. */
    s16 t;

    obj->x += obj->dx;
    obj->y += obj->dy;
    if (++obj->timer == 32) {
        obj->timer = 0;
        t = obj->dx;
        obj->dx = -obj->dy;
        obj->dy = t;
    }
}

static void blink_update(struct Obj *obj)
{
    obj->timer++;
    if (obj->timer & 16)
        obj->color = RGB(31, 31, 31);
    else
        obj->color = sObjOps[obj->kind].tint;
}

static void draw_box(const struct Obj *obj, u16 color)
{
    s32 size = sObjOps[obj->kind].size;

    fill_rect(obj->x, obj->y, size, size, color);
}

static void draw_cross(const struct Obj *obj, u16 color)
{
    s32 size = sObjOps[obj->kind].size;

    fill_rect(obj->x - size, obj->y, 2 * size + 1, 1, color);
    fill_rect(obj->x, obj->y - size, 1, 2 * size + 1, color);
}

NOINLINE void add_obj(u8 kind, s16 x, s16 y, s16 dx, s16 dy)
{
    struct Obj *obj;

    if (gNumObjs == MAX_OBJS)
        return;
    obj = &gObjs[gNumObjs++];
    obj->x = x;
    obj->y = y;
    obj->dx = dx;
    obj->dy = dy;
    obj->kind = kind;
    obj->timer = 0;
    obj->color = sObjOps[kind].tint;
}

NOINLINE void add_callback(Callback cb)
{
    if (gNumCallbacks < MAX_CALLBACKS)
        gCallbacks[gNumCallbacks++] = cb;
}

NOINLINE void remove_callback(Callback cb)
{
    u32 i, j;

    for (i = 0; i < gNumCallbacks; i++) {
        if (gCallbacks[i] == cb) {
            for (j = i + 1; j < gNumCallbacks; j++)
                gCallbacks[j - 1] = gCallbacks[j];
            gNumCallbacks--;
            return;
        }
    }
}

static void cb_objects(u32 frame)
{
    u32 i;
    struct Obj *obj;
    const struct ObjOps *ops;

    for (i = 0; i < gNumObjs; i++) {
        obj = &gObjs[i];
        ops = &sObjOps[obj->kind];
        ops->draw(obj, BACKDROP);
        ops->update(obj);
        ops->draw(obj, obj->color);
    }
}

static void cb_beat(u32 frame)
{
    if ((frame & 15) == 0)
        gBeats++;
    fill_rect(4, 150, gBeats & 127, 4, RGB(31, 31, 0));
}

static void cb_spawn(u32 frame)
{
    /* Adds an object every 90 frames, then takes itself off the list.
     * (No division: libgcc's would be the lift's to read too.) */
    if (++gSpawnTimer == 90) {
        gSpawnTimer = 0;
        add_obj(gSpawnKind, 100 + gNumObjs * 8, 40, 1, -1);
        if (++gSpawnKind == 3)
            gSpawnKind = 0;
    }
    if (gNumObjs == MAX_OBJS)
        remove_callback(cb_spawn);
}

static void main_cb_run(void);

static void main_cb_wait(void)
{
    /* A minute's first second: nothing moves but the beat. */
    if (gFrame == 60)
        gMainCallback = main_cb_run;
}

static void main_cb_run(void)
{
    u32 i;

    for (i = 0; i < gNumCallbacks; i++)
        gCallbacks[i](gFrame);
    gState++;
}

void AgbMain(void)
{
    VIDEO_MODE3_VBLANK_IRQ();
    add_obj(0, 20, 20, 2, 1);
    add_obj(1, 120, 60, 1, 0);
    add_obj(2, 200, 120, 0, 0);
    add_callback(cb_objects);
    add_callback(cb_spawn);
    add_callback(cb_beat);
    gMainCallback = main_cb_wait;
    for (;;) {
        VBlankIntrWait();
        gMainCallback();
        gFrame++;
    }
}
