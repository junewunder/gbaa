@ c-resume's guard and loop (c/resume/SCENE.md): two context restores
@ that load sp and pc from a block a function of the program filled, not
@ setjmp's buffer. bail loads sp and pc from gGuard, where guard_call put
@ its sp and the return site of its call (`adr`); skip loads sp and the
@ loop's registers from gLoop, where run_list keeps them, and jumps back
@ into run_list's loop at its call's return site. ARM code behind Thumb
@ `bx pc` entries, as c/jump's. Our own, no library's or game's code.

    .text

    .align 2
    .global guard_call
    .thumb_func
guard_call:
    bx pc
    nop
    .arm
    push {r4, r11, lr}
    ldr r11, =gGuard
    mov r1, r0
    mov r0, #0
    adr r3, 1f
    str r3, [r11, #8]
    str sp, [r11, #4]
    ldr r3, =search
    mov lr, pc
    bx r3
1:  pop {r4, r11, lr}
    bx lr

    .thumb
    .align 2
    .global bail
    .thumb_func
bail:
    bx pc
    nop
    .arm
    ldr r12, =gGuard
    ldr sp, [r12, #4]
    ldr pc, [r12, #8]

    .thumb
    .align 2
    .global run_list
    .thumb_func
run_list:
    bx pc
    nop
    .arm
    push {r4, r5, r6, r11, lr}
    ldr r11, =gLoop
    str sp, [r11, #12]
    str r0, [r11, #16]
    mov r5, r0
    mov r4, #0
    mov r6, #0
2:  cmp r4, r5
    bcs 3f
    str r4, [r11, #0]
    str r6, [r11, #4]
    mov r0, r4
    ldr r3, =step
    mov lr, pc
    bx r3
run_after:
    add r6, r6, r0
    add r4, r4, #1
    b 2b
3:  mov r0, r6
    pop {r4, r5, r6, r11, lr}
    bx lr

    .thumb
    .align 2
    .global skip
    .thumb_func
skip:
    bx pc
    nop
    .arm
    ldr r11, =gLoop
    ldr r4, [r11, #0]
    ldr r6, [r11, #4]
    ldr r5, [r11, #16]
    ldr sp, [r11, #12]
    b run_after

    .ltorg
