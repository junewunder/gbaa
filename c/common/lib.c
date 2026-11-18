/* The C library calls the compilers emit themselves (c/README.md): modern
 * GCC copies a struct with memcpy. Byte loops, our own; built with
 * -fno-tree-loop-distribute-patterns so GCC doesn't make them call
 * themselves. */
#include "gba.h"

void *memcpy(void *dst, const void *src, unsigned long n)
{
    u8 *d = dst;
    const u8 *s = src;

    while (n--)
        *d++ = *s++;
    return dst;
}

void *memset(void *dst, s32 c, unsigned long n)
{
    u8 *d = dst;

    while (n--)
        *d++ = c;
    return dst;
}
