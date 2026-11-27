# C test programs

The agb programs in `agb/` are Rust compiled by LLVM; every commercial GBA
game is C, from agbcc (GCC 2.95: pret's games, Metroid) or modern GCC. The
programs here give the lifter's code-shape work C's shapes to be judged on
(unit native/c-test-programs).

`just c-tests` builds each folder but `common/` twice, with agbcc
(jiangzhengwenjz's, `$GBAA_DATA/src/agbcc-jzw`) and with modern
arm-none-eabi-gcc (`$GBAA_DATA/toolchains/arm-gnu-toolchain-15.2.rel1-…`),
both fetched by `scripts/fetch-decomps.sh`, into
`$GBAA_DATA/c/c-<name>-agbcc.{elf,gba}` and `c-<name>-gcc.{elf,gba}`.
Source only in git. GCC writes DWARF 2 (`-gdwarf-2 -gstrict-dwarf`), as
agbcc does (`pret-names` reads GCC 15's default DWARF 5 as well, and the
location lists both give: native/names-location-lists; `gba.ld` keeps
DWARF 5's sections). `names/c-<name>-<cc>.toml`
are the programs' names files (`target/release/pret-names ELF ROM OUT`;
`just c-names NAME` for one whose source's `#define`s the file names,
c/switch's), which give their lifts types (native/lift-typed-access); a
changed ROM needs its file made again, or its lift has no names.

- A program's own `.s` files are assembled into it too (`jump/setjmp.s`,
  `resume/resume.s`).
- `common/`: our own start-up and hardware, no game's or library's code
  (no libtonc, no libgba). `crt0.s` (ARM: points the BIOS's IRQ vector at
  its `irq_handler`, copies `.data`, zeroes `.bss` and `.ewram`, calls
  `AgbMain`; `VBlankIntrWait` in Thumb), `gba.ld` (code and constants in
  ROM, `.data` and `.bss` in IWRAM, section `.ewram` in EWRAM), `gba.h`
  (types, a few registers, `NOINLINE`, `EWRAM`), `draw.c` (mode 3
  rectangles and pixels) and `lib.c` (the `memcpy` and `memset` modern GCC
  calls for a struct copy).
- The entry is `AgbMain`, as in pret's games: agbcc's `main` would call
  `__gccmain` (its constructors).
- agbcc goes pret's way: `arm-none-eabi-cpp -nostdinc -undef -DAGBCC`, then
  `agbcc -mthumb-interwork -O2 -fhex-asm -g` to Thumb assembly, then `as`;
  it links agbcc's `libgcc.a` (`_call_via_rN` for a call through a
  pointer). GCC: `-mcpu=arm7tdmi -mthumb -mthumb-interwork -O2
  -ffreestanding -nostdlib -fno-tree-loop-distribute-patterns`, with GCC's
  Thumb `libgcc.a`. agbcc is C89: declarations at a block's top, no `//`.
- Division and modulo only where a program means them (c/div, c/jump's
  `%`): each is a call of libgcc's, which the lift prints as `/` or `%`
  by the routine's symbol (native/lift-libgcc-div).
- A long loop before the first `VBlankIntrWait` (a full-screen fill) is
  fine: the interpreter starts the beam where mGBA hands over (line 126),
  so such a fill lands on the same frame in both
  (unit oracle/boot-beam-and-bios-costs, `memory/mgba-boot-beam.md`).

| Folder      | Shape                                                    |
|-------------|----------------------------------------------------------|
| `template/` | the starter: a bouncing square; copy it for a new program |
| `fnptr/`    | function pointers in a const struct table, a callback list, a main callback |
| `switch/`   | switches compiled to jump tables: a script interpreter, a state machine; a sparse one compiled to compares |
| `chain/`    | a stack struct passed by pointer down a call chain; a struct returned by value |
| `heap/`     | a first-fit allocator; sprites, a menu and short-lived effects allocated at three call sites, function pointers beside numbers in each |
| `bits/`     | bitfields in a byte and a halfword through a pointer: written one field at a time, constant and computed, and read and written joined by the compiler; a global's bitfields at its constant address |
| `loops/`    | counted loops to a parameter, a global and a field; nested, counting down, breaking out, skipping rounds, a `u8` counter; arrays indexed by the counter: of structs, a field array through a pointer, two by one counter, a pointer stepped beside it |
| `list/`     | a linked list over a static node pool: a free list and a sorted live list through one `next`, inserted into through a `Node **`, removed from and walked each frame, an `act` function pointer per node |
| `jump/`     | errors thrown with `longjmp` out of a recursion to an outer and an inner `setjmp` (`setjmp.s`: the ARM pair behind Thumb entries, a program's own assembly) |
| `resume/`   | restores that load sp and pc from a block of their own (`resume.s`): a search bailing out to its guard (`ldr sp; ldr pc`), a loop's step dropped from below and the loop going on (`ldr sp; b` into it) |
| `lent/`     | a stack struct whose callee reads only its last byte: the address lent lies before the bytes lent (Emerald's RTC status) |
| `borrow/`   | stack structs lent to callees in another file (`cells.c`): a pointer tested for null, an array in a struct written and read at an index |
| `div/`      | signed and unsigned `/` and `%`: by constants, by divisors that may be -1, by ones not proven nonzero; libgcc's calls, agbcc's and modern GCC's |

The eleven programs (twenty-two ROMs) are in the lifter's check (`just
lift-check`, `crates/codegen/lift-check.toml`'s `c-*` tables, routes
`units/c-<name>-<cc>-600.toml`), each with a planted store. A new program:
copy `template/`, write its `SCENE.md`, run `just c-tests`, then add its
two tables and routes as the others are.
