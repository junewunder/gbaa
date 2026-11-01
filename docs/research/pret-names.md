# Emerald's names from pokeemerald's symbols

The mod view's first source map (decision 2, "Two views of a ROM"): pret's
pokeemerald ELF, built byte-identical to Emerald, matched to the function
entries codegen compiles and the data their pools point at.
`just pret-names` (crates/pret-names) writes
`$GBAA_DATA/names/pokeemerald.toml` and prints the report below;
`just fetch-pret` (scripts/fetch-pret.sh) clones pokeemerald `731ad5bf` and
agbcc `da598c1d`, pinned, and builds the ELF. Numbers from quack,
2026-10-26, discovery at main `e415368`.

## Results

| | |
|---|---|
| Our function entries (`Ctx::entries`, ROM) | 15,158 |
| Named by a pret function symbol at the exact key, mode included | 15,143 |
| Named by an asm label (`__umul3232H32`, libgcc) | 1 |
| In pret data: discovery took data for code | 13 |
| No symbol at all (inside `gMultiBootProgram_BerryGlitchFix`, the multiboot program sent to another GBA) | 1 |
| A pret function in the other mode, or a mid-function entry | 0 |

So **15,144 of 15,158 (99.9%) get an exact name**, and the 14 others are
discovery false positives: battle scripts (`BattleScript_MoveHPDrain`),
`gSpeciesInfo`, movement tables, text, and the multiboot payload. None is a
continuation root inside a pret function: every entry discovery adds is a
real function start.

**pret functions we don't compile: 716 of 15,859.**

| Why | Count | Examples |
|---|---|---|
| Unreached, and no ROM word holds its address: unused code | 631 | `CheckHeap`, `GetBgX`, `Unused_LoadBgPalette` |
| Unreached, but a ROM word holds its address (a pointer discovery doesn't follow) | 70 | `SpriteCB_Status`, `Cmd_getexp`, `HandleTurnActionSelectionState` |
| Inside data discovery found: m4a's code `m4aSoundInit` copies to IWRAM | 13 | `SoundMainRAM`, `ply_goto`, `ply_patt` |
| Discovered, no code in its mode (also in the IWRAM copy) | 1 | `MPlayJumpTableCopy` |
| Reached, but only by falling into it | 1 | `Init` (crt0: `Start` runs into it) |

The 70 pointed-at functions are the ones a mod could reach that we'd run
in the interpreter; whether each is live is the discovery group's question.

**Data.** 56,486 pret data symbols (1,062 in RAM; a typed object beats a
global label beats a local one at a shared address, the rest kept as
aliases). Of the words in our literal pools that point into RAM, 30,993 of
31,992 (96.9%) land on a symbol and 966 inside one (a field: `gHeap+8`);
33 are unnamed. Into ROM data: 7,413 of 8,731 (84.9%) on a symbol, 202
inside one, 1,116 unnamed (mostly `INCBIN`'d graphics with no label).

**RAM-copied code.** No function is linked at a RAM address; pret links
the IWRAM code at its ROM address and copies it (`CpuCopy32(SoundMainRAM &
~1, SoundMainRAM_Buffer, 0x800)`). Discovery finds the copy, 0x082df0e0 →
0x03001aa8 (2,048 bytes), but no entry into it (`SoundMain` jumps to it
with `bx` on a pool word, `SoundMainRAM_Buffer + 1`,
`pokeemerald@731ad5bf:src/m4a_1.s:76-82`), so it isn't a static overlay and the
names file keys its 14 functions by run address under `[copies]`
(`SoundMainRAM` at `0x03001aa9`), ready for the overlays `codegen::seen`
finds at run time. The Thumb bit: function keys are interworking (bit 0 set
for Thumb), as codegen's are; data labels are byte addresses, so a Thumb
key's bit 0 is kept when looking one up (`BattleScript_MoveUsedIsInLove`
is at an odd address).

## The names file

`$GBAA_DATA/names/pokeemerald.toml`, 4.8 MB, plain TOML (it holds only
addresses and names, decision 2's legality line). Tables, keyed by hex
address:

- `[map]`: `rom_sha1`, `source` (`pret/pokeemerald@<commit>`) and
  `format = 1`, the header codegen's loader (`codegen::names`) checks.
- `[functions]`: our entry → `{ name, size, file }` (`label = true` for an
  asm label; `rom` for a run address). `file` is the object file from the
  symbol table's `STT_FILE` (`main.o`): exact for a static, and for a
  global the file of the nearest local symbol at or below it in code.
- `[inside]`: an entry with no symbol, `{ func, offset }` into a pret
  function (empty on Emerald).
- `[unnamed]`: an entry with no function name, and why
  (`"data: gSpeciesInfo+0x11"`).
- `[missed]`: pret functions we don't compile, `{ name, size, why }`.
- `[copies]`: each BIOS copy out of ROM by destination, `{ rom, len,
  functions }`.
- `[data]`: `{ name, size, file, aliases }`, `file` for statics only.

The ELF also carries DWARF 2 (locals and parameters with their register or
stack slot, struct types, file and line): the source of names below the
function level for later units.

## The modern-GCC build

`just pret-names pokeemerald_modern` (2026-10-28, mod/cross-compiler-match):
12,398 of 12,435 entries named exactly (99.7%); 29 are inside a pret
function with no symbol of their own, 7 in data. Modern GCC names every
object `<stdin>`, so pret-names takes the objects from the linker map
beside the ELF (`map_sections`, `name_stdin_files`). It has no DWARF, so
no prototypes, locals or types. `cross-compiler-matching.md` uses it as
the truth for matching across the compiler change.
