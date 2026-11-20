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
are the programs' names files (`target/release/pret-names ELF ROM OUT`),
which give their lifts types (native/lift-typed-access); a changed ROM
needs its file made again, or its lift has no names.

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
- No division or modulo: libgcc's would be the lift's to read too.
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

The four programs (eight ROMs) are in the lifter's check (`just
lift-check`, `crates/codegen/lift-check.toml`'s `c-*` tables, routes
`units/c-<name>-<cc>-600.toml`), each with a planted store. A new program:
copy `template/`, write its `SCENE.md`, run `just c-tests`, then add its
two tables and routes as the others are.
