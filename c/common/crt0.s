@ The C test programs' start-up (c/README.md): our own, no game's or
@ library's code. The BIOS leaves the CPU in system mode with its stacks
@ set, as for agb's programs, so this sets none: it sets WAITCNT, points the
@ BIOS's IRQ vector at irq_handler, copies .data to IWRAM, zeroes .bss and
@ .ewram and calls AgbMain (not main: agbcc's main calls __gccmain, its
@ constructors).

    .section .crt0, "ax", %progbits
    .arm
    .align 2
    .global _start
_start:
    b start_vector

    @ The cartridge header: left blank, as agb's programs leave theirs.
    .space 188

start_vector:
    @ The waitstates cartridges use (WS0 3,1; prefetch on), as games set.
    ldr r0, =0x04000204
    ldr r1, =0x4317
    strh r1, [r0]

    ldr r0, =irq_handler
    ldr r1, =0x03007FFC
    str r0, [r1]

    @ .data: its bytes in ROM to IWRAM, a word at a time.
    ldr r0, =__data_load
    ldr r1, =__data_start
    ldr r2, =__data_end
1:  cmp r1, r2
    ldrlo r3, [r0], #4
    strlo r3, [r1], #4
    blo 1b

    @ .bss: zeroed.
    ldr r1, =__bss_start
    ldr r2, =__bss_end
    mov r3, #0
2:  cmp r1, r2
    strlo r3, [r1], #4
    blo 2b

    @ .ewram: zeroed too.
    ldr r1, =__ewram_start
    ldr r2, =__ewram_end
4:  cmp r1, r2
    strlo r3, [r1], #4
    blo 4b

    ldr r0, =AgbMain
    mov lr, pc
    bx r0
3:  b 3b
    .pool

@ The IRQ handler the BIOS calls (ARM, IRQ mode): acknowledges what is
@ both enabled and requested in IF and in the BIOS's flags at 0x03007FF8,
@ which VBlankIntrWait waits on. No mode switch and no stack.
    .arm
    .align 2
    .global irq_handler
irq_handler:
    ldr r3, =0x04000200
    ldr r2, [r3]                @ IE, and IF in the top half
    and r2, r2, r2, lsr #16     @ enabled and requested
    strh r2, [r3, #2]           @ IF: acknowledged
    ldr r3, =0x03007FF8
    ldrh r1, [r3]
    orr r1, r1, r2
    strh r1, [r3]
    bx lr
    .pool

@ BIOS calls (Thumb), for both compilers alike.
    .thumb
    .align 1
    .global VBlankIntrWait
    .thumb_func
VBlankIntrWait:
    swi 0x05
    bx lr
