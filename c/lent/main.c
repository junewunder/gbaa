/* c-lent (c/lent/SCENE.md): a stack struct whose address goes to a callee
 * that reads only its last byte, as an RTC driver's reset hands its status
 * to the status setter. The bytes lent are the field's alone, so the
 * address passed lies before them (native/emerald-lent-local-stop). A
 * square moves as c/template's does, coloured from that byte. */
#include "gba.h"
#include "draw.h"

#define SIZE 16

struct Stamp {
    u8 pad[7];
    u8 mark;
};

s32 gX, gY;
s32 gDx = 1, gDy = 1;
u32 gFrame;
u32 gMarks;

NOINLINE u32 stamp_mark(const struct Stamp *s)
{
    return s->mark & 0x1f;
}

NOINLINE u32 stamp(u32 n)
{
    struct Stamp s;

    s.mark = n;
    return stamp_mark(&s);
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
    u32 m;

    VIDEO_MODE3_VBLANK_IRQ();
    for (;;) {
        VBlankIntrWait();
        fill_rect(gX, gY, SIZE, SIZE, 0);
        step();
        m = stamp(gFrame >> 4);
        gMarks += m;
        fill_rect(gX, gY, SIZE, SIZE, RGB(31, m, 0));
        gFrame++;
    }
}
