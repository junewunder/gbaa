/* The C test programs' hardware header (c/README.md): our own, the few
 * registers and BIOS calls they use. Built by agbcc (GCC 2.95, C89) and by
 * modern arm-none-eabi-gcc alike. */
#ifndef GBA_H
#define GBA_H

typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef signed char s8;
typedef signed short s16;
typedef signed int s32;

#define REG16(a) (*(volatile u16 *)(a))
#define REG_DISPCNT REG16(0x04000000)
#define REG_DISPSTAT REG16(0x04000004)
#define REG_IE REG16(0x04000200)
#define REG_IME REG16(0x04000208)

#define VRAM ((volatile u16 *)0x06000000)
#define BG_PALETTE ((volatile u16 *)0x05000000)
#define SCREEN_W 240
#define SCREEN_H 160

#define RGB(r, g, b) ((u16)((r) | ((g) << 5) | ((b) << 10)))

/* Modern GCC inlines a called-once function at -O2; agbcc (built with
 * -DAGBCC) never does without `inline`, and knows no noinline. */
#ifdef AGBCC
#define NOINLINE
#else
#define NOINLINE __attribute__((noinline))
#endif

/* An uninitialised global in EWRAM rather than IWRAM's .bss. */
#define EWRAM __attribute__((section(".ewram")))

/* crt0.s */
void VBlankIntrWait(void);

/* Mode 3, BG2 on, the V-blank IRQ on: what every program starts with. */
#define VIDEO_MODE3_VBLANK_IRQ()        \
    do {                                \
        REG_DISPCNT = 3 | (1 << 10);    \
        REG_DISPSTAT = 1 << 3;          \
        REG_IE = 1;                     \
        REG_IME = 1;                    \
    } while (0)

#endif
