@ c-jump's setjmp and longjmp (c/jump/SCENE.md): the usual ARM pair, as a
@ C library for the GBA builds it with Thumb-interworking entries: a Thumb
@ `bx pc` into ARM, a store of r4-r12, sp and lr into the buffer, and the
@ load of them back, which returns into setjmp's caller with r0 the value
@ passed (1 for a 0). Our own, no library's code.

    .text
    .align 2

    .global setjmp
    .thumb_func
setjmp:
    bx pc
    nop
    .arm
    stmia r0!, {r4-r12, sp, lr}
    mov r0, #0
    tst lr, #1
    moveq pc, lr
    bx lr

    .thumb
    .align 2
    .global longjmp
    .thumb_func
longjmp:
    bx pc
    nop
    .arm
    ldmia r0!, {r4-r12, sp, lr}
    movs r0, r1
    moveq r0, #1
    tst lr, #1
    moveq pc, lr
    bx lr
