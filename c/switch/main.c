/* c-switch (c/switch/SCENE.md): switches dense enough to compile to jump
 * tables. A small script interpreter reads byte commands from ROM, one
 * switch case per command (as games' event scripts do), and a pen's
 * state machine picks its move by a second switch each frame. A third,
 * sparse switch compiles to compares (tally). */
#include "gba.h"
#include "draw.h"

#define BACKDROP 0

enum {
    CMD_END,
    CMD_WAIT,
    CMD_COLOR,
    CMD_MOVE,
    CMD_BOX,
    CMD_JUMP,
    CMD_LOOP,
    CMD_STATE,
    CMD_ADD,
    CMD_CLEAR,
    CMD_DOT,
};

/* A pen's state: a byte, its values an enum's (as mzm's MAKE_ENUM: the
 * typedef is the field's type, the enum its values), which the names file
 * links by that type (native/lift-value-names). */
typedef u8 PenState;
enum PenState_ {
    PEN_IDLE,
    PEN_RIGHT,
    PEN_DOWN,
    PEN_LEFT,
    PEN_UP,
    PEN_ZIGZAG,
    PEN_SPIRAL,
    PEN_HOLD,
};

struct Script {
    const u8 *pc;
    const u8 *start;
    u16 wait;
    u8 loops;
    u8 color;
    s16 x, y;
};

struct Pen {
    s16 x, y;
    PenState state;
    u8 step;
    u16 color;
};

static const u16 sColors[] = {
    RGB(31, 0, 0), RGB(0, 31, 0), RGB(0, 0, 31), RGB(31, 31, 0),
    RGB(31, 0, 31), RGB(0, 31, 31), RGB(31, 31, 31), RGB(16, 16, 16),
};

static const u8 sScript[] = {
    CMD_COLOR, 3,
    CMD_MOVE, 20, 20,
    CMD_BOX, 10,
    CMD_WAIT, 8,
    CMD_ADD, 12, 6, /* byte 9 */
    CMD_COLOR, 1,
    CMD_BOX, 6,
    CMD_DOT,
    CMD_WAIT, 4,
    CMD_LOOP, 4, 9,
    CMD_STATE, PEN_ZIGZAG,
    CMD_WAIT, 30,
    CMD_STATE, PEN_SPIRAL,
    CMD_COLOR, 5,
    CMD_BOX, 4,
    CMD_WAIT, 40,
    CMD_STATE, PEN_HOLD,
    CMD_WAIT, 20,
    CMD_CLEAR,
    CMD_STATE, PEN_RIGHT,
    CMD_JUMP, 2,
};

struct Script gScript;
struct Pen gPen;
u32 gFrame;
u32 gCommands;
u32 gTally;

NOINLINE void pen_set(PenState state)
{
    gPen.state = state;
    gPen.step = 0;
}

/* Runs commands until one waits; returns how many ran. */
NOINLINE u32 script_run(struct Script *s)
{
    u32 ran = 0;
    u8 cmd;

    if (s->wait) {
        s->wait--;
        return 0;
    }
    for (;;) {
        cmd = *s->pc++;
        ran++;
        switch (cmd) {
        case CMD_END:
            s->pc--;
            return ran;
        case CMD_WAIT:
            s->wait = *s->pc++;
            return ran;
        case CMD_COLOR:
            s->color = *s->pc++ & 7;
            break;
        case CMD_MOVE:
            s->x = s->pc[0];
            s->y = s->pc[1];
            s->pc += 2;
            break;
        case CMD_BOX:
            fill_rect(s->x, s->y, *s->pc, *s->pc, sColors[s->color]);
            s->pc++;
            break;
        case CMD_JUMP:
            s->pc = s->start + *s->pc;
            break;
        case CMD_LOOP:
            /* LOOP n, at: back to the script's byte at, n times in all. */
            if (s->loops == 0)
                s->loops = s->pc[0];
            if (--s->loops != 0)
                s->pc = s->start + s->pc[1];
            else
                s->pc += 2;
            break;
        case CMD_STATE:
            pen_set(*s->pc++);
            break;
        case CMD_ADD:
            s->x += (s8)s->pc[0];
            s->y += (s8)s->pc[1];
            s->pc += 2;
            break;
        case CMD_CLEAR:
            fill_rect(16, 16, 112, 64, BACKDROP);
            break;
        case CMD_DOT:
            plot(s->x - 2, s->y - 2, sColors[6]);
            break;
        default:
            /* An unknown command ends the script. */
            s->pc--;
            return ran;
        }
    }
}

NOINLINE void pen_update(struct Pen *p)
{
    switch (p->state) {
    case PEN_IDLE:
        break;
    case PEN_RIGHT:
        p->x++;
        if (p->x >= 230)
            pen_set(PEN_DOWN);
        break;
    case PEN_DOWN:
        p->y++;
        if (p->y >= 130)
            pen_set(PEN_LEFT);
        break;
    case PEN_LEFT:
        p->x--;
        if (p->x <= 10)
            pen_set(PEN_UP);
        break;
    case PEN_UP:
        p->y--;
        if (p->y <= 10)
            pen_set(PEN_RIGHT);
        break;
    case PEN_ZIGZAG:
        p->x += 2;
        p->y += (p->step & 8) ? 2 : -2;
        if (p->x >= 230)
            p->x = 10;
        break;
    case PEN_SPIRAL:
        p->x += (p->step & 16) ? -1 : 1;
        p->y += (p->step & 32) ? -1 : 1;
        break;
    case PEN_HOLD:
        p->color = sColors[p->step & 7];
        break;
    }
    if (p->x < 10)
        p->x = 10;
    if (p->x > 230)
        p->x = 230;
    if (p->y < 10)
        p->y = 10;
    if (p->y > 130)
        p->y = 130;
    p->step++;
}

/* A switch too sparse for a jump table: both compilers compare its value
 * in a binary search (as games' small switches on a stage do). */
NOINLINE void tally(u32 n)
{
    switch (n) {
    case 1:
        gTally += 3;
        break;
    case 5:
        gTally ^= gFrame;
        break;
    case 40:
        gTally = (gTally << 1) | (gTally >> 31);
        break;
    case 100:
        gTally -= gCommands;
        break;
    default:
        gTally++;
        break;
    }
}

void AgbMain(void)
{
    VIDEO_MODE3_VBLANK_IRQ();
    gScript.start = sScript;
    gScript.pc = sScript;
    gPen.x = 10;
    gPen.y = 10;
    gPen.color = RGB(31, 20, 10);
    pen_set(PEN_RIGHT);
    for (;;) {
        VBlankIntrWait();
        gCommands += script_run(&gScript);
        pen_update(&gPen);
        tally(gFrame & 127);
        fill_rect(gPen.x, gPen.y, 2, 2, gPen.color);
        fill_rect(0, 150, (gCommands & 127) + 1, 4, sColors[gFrame & 7]);
        gFrame++;
    }
}
