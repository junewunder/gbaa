@ c-align's readers (c/align/SCENE.md): loads through a pointer parameter
@ with nothing but the callers to say it is aligned. Our own, no library's
@ code: `ldr` and `ldrh` at an address its width doesn't divide read the
@ aligned word or halfword rotated right by 8 bits a byte, as the CPU does.

    .text
    .thumb
    .align 2

@ u32 rd32(const void *p): the word at p, rotated where p is odd.
    .global rd32
    .thumb_func
rd32:
    ldr r0, [r0]
    bx lr

@ u32 rd16(const void *p): the halfword at p, rotated where p is odd.
    .global rd16
    .thumb_func
rd16:
    ldrh r0, [r0]
    bx lr

@ u32 rd_pair(const u32 *p): p[0] + p[1]; its callers pass word addresses.
    .global rd_pair
    .thumb_func
rd_pair:
    ldr r1, [r0]
    ldr r2, [r0, #4]
    add r0, r1, r2
    bx lr
