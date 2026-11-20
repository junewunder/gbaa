/* The C test programs' linker script (c/README.md): code and constants in
 * ROM, .data and .bss in IWRAM below the BIOS's stacks, nothing in EWRAM
 * unless a program puts it in section .ewram (zeroed, like .bss). */
OUTPUT_FORMAT("elf32-littlearm")
OUTPUT_ARCH(arm)
ENTRY(_start)

MEMORY {
    rom   (rx)  : ORIGIN = 0x08000000, LENGTH = 32M
    ewram (w!x) : ORIGIN = 0x02000000, LENGTH = 256K
    iwram (w!x) : ORIGIN = 0x03000000, LENGTH = 28K
}

SECTIONS {
    .text : {
        KEEP(*(.crt0))
        *(.text .text.*)
        *(.glue_7 .glue_7t)
        . = ALIGN(4);
    } > rom

    .rodata : {
        *(.rodata .rodata.*)
        . = ALIGN(4);
    } > rom

    .data : {
        __data_start = .;
        *(.data .data.*)
        . = ALIGN(4);
        __data_end = .;
    } > iwram AT> rom
    __data_load = LOADADDR(.data);

    .bss (NOLOAD) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > iwram

    .ewram (NOLOAD) : {
        __ewram_start = .;
        *(.ewram .ewram.*)
        . = ALIGN(4);
        __ewram_end = .;
    } > ewram

    .debug_info     0 : { *(.debug_info) }
    .debug_abbrev   0 : { *(.debug_abbrev) }
    .debug_line     0 : { *(.debug_line) }
    .debug_aranges  0 : { *(.debug_aranges) }
    .debug_str      0 : { *(.debug_str) }
    .debug_frame    0 : { *(.debug_frame) }
    .debug_loc      0 : { *(.debug_loc) }
    .debug_ranges   0 : { *(.debug_ranges) }
    .debug_rnglists 0 : { *(.debug_rnglists) }
    .debug_line_str 0 : { *(.debug_line_str) }
    .debug_loclists 0 : { *(.debug_loclists) }
    .debug_str_offsets 0 : { *(.debug_str_offsets) }
    .debug_addr     0 : { *(.debug_addr) }
    .symtab  0 : { *(.symtab) }
    .strtab  0 : { *(.strtab) }
    .shstrtab 0 : { *(.shstrtab) }
    /DISCARD/ : { *(*) }
}
