# The mod view: source maps and readable code

The design behind decision 2's mod view ("Two views of a ROM", june
2026-10-26), group `mod`, unit `mod/design`. It rests on the naming review
([research/name-recovery.md](research/name-recovery.md)) and on the pret
matching ([research/pret-names.md](research/pret-names.md)), and it is
Emerald first. Written 2026-10-26 on quack, against main `97b7279`.

The play view (what `just recompile` prints today) is unchanged by
everything here. The mod view is the same analysis, structuring and
runtime, printed differently:

1. **names** from a source map, applied to the whole game (identifiers
   only, so it builds as fast as the play view);
2. **a readable form** for the functions a list names (registers as
   locals, short macros, quiet instruction text), the rest as the play
   view prints them;
3. **a mod copy**: the readable, named functions a mod touches, cut out
   once into ordinary Rust in git, which the build then uses in place of
   what it generates.

A fourth part, **the naming loop**, is how a game with no decomp would get
its map. On Emerald it is measured against pret, on a capped sample, before
june decides whether it runs anywhere at scale.

## Summary

- **The source map is a TOML file keyed by address**: functions, data
  (RAM globals and ROM objects), per-function locals by register and
  address range, and a header pinning it to one ROM by hash. Emerald's is
  `$GBAA_DATA/names/pokeemerald.toml` as `just pret-names` already writes
  it, plus a `[map]` header, each function's source file, and (later) a
  `[locals]` table. It is derived, so it is never committed; names a human
  or an agent gives go in a committed `names/<game>.toml`, layered on top.
- **The printer applies it with no change in behaviour.** Link names stay
  `gbaa_f_<key>`, so only Rust identifiers change. A data address prints as
  `addr!(gHeap + 8)`, which expands to the literal. The proof is a
  token-level check: undo every name through the map's inverse and the
  named print must equal the unnamed print, token for token. The
  end-to-end check is `route-check --same-states` on the named and unnamed
  builds over Emerald's new-game route.
- **The readable form** is a per-function switch, `--readable LIST`. It
  turns `--locals` on for those functions only, prints ticks, calls and
  returns as `tick!`, `call!` and `ret!` (`macro_rules!` that expand to
  exactly today's tokens), and leaves the `insn!` markers out (june,
  2026-10-30: they were a third of its lines); `--asm-text` brings each
  back as one line of assembly (`insn!(0x0803_b264, "lsls r0, r0, #17")`),
  `--asm-comments` as a comment (`// 0x0803_b264 lsls r0, r0, #17`). Where
  pret's prototype says a callee returns a value, the call prints as
  `r0 = call!(GetMonData2, …)`.
- **Locals are named only where both sides agree.** DWARF gives one
  location per variable for its whole scope, so a register local is renamed
  only when every live range our liveness finds for that register in the
  function falls inside the variable's. Otherwise the name is a comment
  where the range starts.
- **A mod copy** is cut with `gbaa-recompile --cut`. It goes to
  `mods/<game>/<mod>/` in this (private) repo: a `mod.toml`, one `.rs` per
  function, and a pristine copy of each, to diff against after a re-cut.
  The build skips those functions and compiles the mod's instead, under the
  same link names.
- **The naming loop** gives an agent each sampled function's readable print
  with every name hidden, bottom-up, in rounds. It is scored against pret by
  subtoken precision and recall, plus exact match. Emerald's score is an
  upper bound because LLMs have read pokeemerald, so three controls measure
  that leak. The cap is 300 functions before june looks.
- **The spike toward pret's C** (june's re-aim, 2026-10-26) wrote eight
  Emerald functions by hand as close to pret's C as stays exact. fn-diff
  found all eight exact: 1,584 runs, 0 differ. Every C statement has its
  line and every global and field but three is named, from DWARF types. Each body is
  about 3 times pret's length and a fifth of the play view's, and 64% of
  its lines are the machine: register syncs, pool loads, frame and ticks.
  It proposes the readable printer become a new lowering, DWARF
  `[types]`, and a callee-saves proof (section "How close exact Rust gets
  to pret's C").
- **The group needs three more units:** DWARF locals and prototypes, the
  mod copy, and assembly text for `insn!`. Their order is under "Units".

## The source map

### What it holds

Everything is keyed by address, as codegen already keys things. Function
keys are interworking (bit 0 set for Thumb), data keys are byte addresses.
A map holds only addresses, names, sizes, source paths and C types written
as text, never code. That is decision 2's legality line: the map can be
shared, but generated code can't.

```toml
[map]
rom_sha1 = "f3ae088181bf583e55daf962a92bb46f4f1d07b7"
source = "pret/pokeemerald@731ad5bf"     # or "names/<game>.toml", "agent:<run>"
format = 1

[functions]
0x080003a5 = { name = "AgbMain", size = 288, file = "src/main.c" }
0x0806a519 = { name = "GetMonData2", size = 348, file = "src/pokemon.c",
               proto = "u32 GetMonData2(struct Pokemon *mon, s32 field)" }

[copies]   # code a BIOS copy puts in RAM, keyed by run address (as now)

[data]
0x02000000 = { name = "gHeap", size = 114688 }
0x020244ec = { name = "gPlayerParty", size = 600, type = "struct Pokemon [6]" }

[locals]   # per function; later (unit mod/dwarf-names)
0x0806a519 = [
  { name = "mon",   reg = 0, from = 0x0806a518, to = 0x0806a560 },
  { name = "field", reg = 1, from = 0x0806a518, to = 0x0806a530 },
  { name = "i",     slot = 8, from = 0x0806a530, to = 0x0806a674 },   # sp+8
]
```

- **`[text]`** is the game's character table (below, "Text").
- **`[functions]`, `[copies]`, `[data]`** are what `just pret-names`
  writes now (research/pret-names.md, "The names file"). Its report
  tables (`[inside]`, `[unnamed]`, `[missed]`) stay in the file, and the
  loader ignores them.
- **`file`** comes from the ELF's symbol table: each local symbol follows
  its `STT_FILE`, and 893 files cover the ROM. It costs no DWARF. The
  symbol table names the object (`main.o`, not `src/main.c`: 69 of the 893
  stems match no single source file), so that is what pret-names writes; a
  global function takes the file of the nearest local symbol before it in
  code. A source path for the doc comment needs DWARF's line table. The
  printer uses it to qualify the 225 static names that occur in more than
  one file (below).
- **`proto` and `type`** are text from DWARF, for comments only. The printer
  never derives code from them, except one thing, and that is
  presentation: whether a call's `r0` is shown as its value (below).
  (Superseded for the readable form by june's decision of 2026-10-26:
  see "How close exact Rust gets to pret's C". The rest of this item is
  how it stood.)
  **Types, if at all, stay in that form**: there is no type recovery, and
  no struct layouts in code. registers-and-types.md says why: recovered
  types are presentation only. Turning `r0 + 0x18` into `mon.box.otId`
  would need struct layouts in code, and that changes what the printer is.
  If june wants it after the first mod, it gets a design of its own.
- **`[locals]`** gives each variable a register (`reg`) or a stack slot
  (`slot`, an offset from `sp` after the prologue), over an address range.
  A variable with two homes has two entries. Ranges are kept even though
  the first printer only renames whole-function locals, because an agent
  reading the code wants the ranges, and a smarter printer can use them
  later.

### Where it lives

- **Derived maps stay out of git.** `$GBAA_DATA/names/<build>.toml` is
  rebuilt by `just fetch-pret` and `just pret-names`, both pinned by
  commit. So it passes "nothing lives only on this disk", and pret's 4.8 MB
  of names never sit in our history. A pret rename is a newer pin and a
  rerun.
- **Given names are committed.** `names/<game>.toml` at the repo root
  (new) holds names a human or an agent gave: a mod's new helper, a
  correction, or a whole game's names once the naming loop runs for real.
  Same format, same `rom_sha1`.
- **Layering.** `gbaa-recompile ROM OUT --names A.toml --names B.toml`
  loads them in order, and a later file wins per key. The loader refuses a
  map whose `rom_sha1` isn't the ROM's, and two keys in one file that give
  one name to different addresses.
- **The naming loop's output never feeds a build of Emerald.** It goes to
  `work/mod-naming/<run>/names.toml` (with `source = "agent:<run>"`) and
  is only scored. For Emerald, pret is the truth.

### Names as Rust identifiers

pret's names are C identifiers. Most are valid Rust as they are (the
generated crates already `allow(non_snake_case)`). The loader fixes the
rest once, when it builds the map's identifier table, so every print uses
the same table and the inverse is exact:

- **A Rust keyword** (`type`, `move`, `match`, `box`, `loop`, …) gets a
  trailing `_`.
- **A name the printer or the prelude already uses** gets a trailing `_`.
  That covers `v<n>`, `r<n>`, `f_<hex>`, `m`, `pc`, `cycles`, `sp0`, `a`,
  `k`, `nf`/`zf`/`cf`/`vf`, and everything `base::prelude` exports (`rt`,
  `prim`, `settle`, `yield_at`, `Width`, `Flow`, `M`, …). The loader's list
  is the prelude's export list, so a new export can't silently shadow a
  name.
- **A name that occurs more than once** (225 statics) gets `__` and its
  file stem in every occurrence: `Cmd_call__scrcmd`, `sBgTemplates__party_menu`.
  If the stem collides too, the address goes on as well.
- **Other characters** become `_`: a function-local static's `sBgCnt.135`
  is `sBgCnt_135`. A name left with no identifier names nothing.
- **No name at all** keeps today's form: `f_0803b25d`, or the literal for
  data.

The identifier table maps each identifier to exactly one key. That
one-to-one table is what the check inverts.

### Value names

pret writes a field's values as `#define`s (`OPTIONS_TEXT_SPEED_MID`, not
`1`), which DWARF doesn't keep. `just pret-names` (`pret_names::values`,
unit mod/value-names) reads `#define NAME <number>` from the tree's
`include/constants/*.h` and writes **`[values]`**: per family, its header,
each value's one name, and the fields that take it.

```toml
[values."OPTIONS_TEXT_SPEED"]
file = "include/constants/global.h"
fields = ["struct SaveBlock2.optionsTextSpeed"]
names = [
  { name = "OPTIONS_TEXT_SPEED_SLOW", value = 0 },
  { name = "OPTIONS_TEXT_SPEED_MID", value = 1 },
  { name = "OPTIONS_TEXT_SPEED_FAST", value = 2 },
]
```

- **A family** is a scalar field's name in pret's constant style
  (`optionsTextSpeed`, `OPTIONS_TEXT_SPEED_`) as a prefix: the defines
  of that prefix in the one header holding most of them, less bounds
  (`_COUNT`, `_MAX`, `_MIN`, `_FIRST`, `_LAST`, `_START`, `_END`) and any
  value two names share. Every value must fit the field (its bits, for a
  bitfield), and two must be left.
- **A field takes it** only where pret's source (`src/**/*.c` and `.h`)
  sets or compares a field of that name with one of the family's names
  (`->optionsTextSpeed = OPTIONS_TEXT_SPEED_MID`, `.species ==
  SPECIES_NONE`, a designated initializer), and with no define of another
  header. The prefix alone gave 57 families, many wrong (`cool` takes
  `COOL_RIBBON_*`, `friendship` `FRIENDSHIP_EVENT_*`); with the source's
  evidence, 29 on 118 fields (Emerald's; the modern and FireRed maps
  have no `[types]`, so no fields). A field name two structs use with two
  families (`effect`: `EFFECT_*` and `CONTEST_EFFECT_*`) takes neither.
  pret's text is read, never written: the map holds names and numbers.
- **The print** (`codegen::types`): each name is a `pub const NAME: u32`
  of `base::globals`, unless it is already a datum's, a function's, a
  marker's, an I/O register's or reserved. A constant stored to a field
  that takes a family prints as its name (`m.set(.., p.weather(),
  WEATHER_VOLCANIC_ASH)`), and so does a `cmp` of a field's value
  (`prim::add_with_carry(v19, !OPTIONS_BUTTON_MODE_L_EQUALS_A, true)`:
  `cmp` adds the complement) and a bitfield's `set_bits` value. A value
  with no name stays a number. A DWARF local can't take a value's name.
  `--names-check` checks readable functions untyped, so it sees none;
  states (`readable-states`, `fn-diff`) prove them, as for typed paths.

### Text

A string pointer means more as its words than as `addr!(gText_…)`.
`just pret-names` (`pret_names::text`, unit mod/text-emerald) reads the
tree's character table, `charmap.txt`, as data into **`[text]`**: each
entry's bytes and its quoted character or its name, in the file's order.
pret's string literals are never read.

```toml
[text]
file = "charmap.txt"
chars = [
  { bytes = "BB", char = "A" },
  { bytes = "FD 01", name = "PLAYER" },
  { bytes = "FC 06 01", name = "FONT_NORMAL" },
]
```

The readable print (`codegen::text`, from `lower::text_notes`) decodes the
bytes a pool word points at in the cartridge, and where they are text,
comments the instruction that loads it:

```rust
// src/trainer_hill.c:608
// gText_XMinYDotZSec: "{STR_VAR_1} min. {STR_VAR_2}.{STR_VAR_3} sec."
m.pool(0x081d_5d6a, 0x081d_5dec);
```

- Control codes and placeholders print by name in braces, as pret's
  source writes them; a code's arguments (lengths from pret's
  `GetExtCtrlCodeLength`, src/string_util.c, which the table doesn't
  give) by name for a color or a sound, else in hex: `{COLOR RED}`,
  `{PAUSE 0x0f}`. The first entry for a sequence wins (Latin over the
  Japanese tables), and the longest sequence matches.
- **Only where it looks like text** (`codegen::text`'s doc has the
  rules): it ends in `0xFF`, every byte decodes, no character English
  text doesn't use, mostly ASCII letters and spaces, no long run of
  spaces, and the datum it lands in, if DWARF types it, is a `u8`
  array the string fits. Over all of Emerald readable (2026-10-31),
  1,255 comments, none on graphics, structs or scripts seen; one
  three-byte fill table (`sFillValues`, "zi") passes.
- A comment changes no token of code, so `--names-check` and the same
  states hold as they did; `--names-check` counts the texts.
- The arguments' lengths are the Pokémon engine's. Another game's
  table has its own layout, so it isn't a map's `[text]` (`just
  pret-names` writes none for a game listed below) but a table of its
  own: "Other games' tables".
- **Tables of strings.** A pool word at a table of pointers whose
  entries are all text (or at a table of such tables, as a game's
  per-language ones) prints the strings, as many as fit in 120
  characters (`Charmap::table_note`):
  `// gNatureNamePointers: ["HARDY", "LONELY", "BRAVE", …]`,
  `// sFileScreenTextPointers[2] sEnglishTextPointers_FileScreen:
  ["Start Game\nStart Game (Link)", …]` (the first table that decodes:
  Japanese ones don't). Emerald, every function readable (2026-10-31):
  173 tables besides the 1,254 strings. Discovery's pointer scan takes
  some such tables for code; a word on code only the scan found may
  still print a table, never a string or bytes. `--names-check` counts
  them as `text tables`.

### Other games' tables

Most games don't use ASCII, and not every game has a pret charmap. A
game's own table is listed in `crates/codegen/text-tables.toml` (unit
mod/text-tables) by ROM sha1, with its path under `$GBAA_DATA` (tables
live outside the repo, like ROMs), its syntax (pret's `charmap.txt`, or
the community's Thingy `.tbl`: `41=A`, `/FF=` the end, `*FE=` a new
line, `[NAME]` a code), its layout and its source. `SourceMap::load`
loads it (`codegen::text::Table`) over any `[text]` the maps carry; a
missing file leaves the game's byte previews.

- **Layout** (`codegen::text::Encoding`): bytes a character takes (a
  2-byte table's `8100` is the bytes `00 81`), the unit that ends a
  string, and characters beyond `COMMON` its English text uses
  (Metroid's `？`, and its full-width space). No Pokémon argument rules.
  A 2-byte table's `XX00` code also spells `XXnn` with a value,
  `{GAP(24)}`, as the mzm preprocessor writes it.
- **Reached by.** `reached = "pointer"`: code loads a string's (or a
  table's) address, which the pool-word notes decode. `"index"`: the game
  looks text up by a message number (`codegen::text::Messages`).
  `[game.index]` gives the message table's address and the functions
  whose `r0` is a number (Minish Cap's `MessageFromTarget`,
  `MessageNoOverlap`, `ShowTextBox`, …); a call to one whose `r0` folds
  to a constant in its block prints the message under the call's own
  comment: `// message 0x0f07: "{Sound:01:E8}Heh heh heh...\nSo that's
  what it means..."`. Minish Cap's table is a word per bank (the
  number's high byte), each its offset from the table, and in a bank a
  word per message, the same; message 0 of a bank is empty and prints
  nothing. Such a game's pool words keep their byte previews (a pointer
  scan there finds only bytes that look like ASCII).
- **Codes with arguments** (`codes`, 1-byte tables): a control code and
  how many argument bytes follow it, printed as the game's own source
  writes them, `{Sound:01:E8}`, `{Color:Red}` (Minish Cap's from tmc's
  `tools/src/tmc_strings`; an argument may be `00`, which doesn't end the
  string). `short` prints a code's arguments without leading zeros
  (`{Var:1}`), `named` spells a whole code by its first argument (`06 00`
  is `{Player}`), and `chars` puts a character over the table's own entry
  (tmc's `QT` is `\"`), so a message reads as the decomp's text does.
- **Checked against the decomp** (`gbaa-recompile --messages-check ROM
  JSON`, `Messages::check`): every message of an index game's table,
  decoded, against its decomp's own text by bank and message (tmc's
  `translations/USA.json`); for Minish Cap all 3,699 match.

The games we have (2026-10-31):

| Game | Table | Result |
|---|---|---|
| Metroid: Zero Mission | mzm's `charmap.txt`, 2-byte | 8 tables, all text (`sMessageTextPointers`, `sFileScreenTextPointers`, `sLocationTextPointers`, `sDescriptionTextPointers`); its file select's "Start Game" matches mGBA's screen |
| The Minish Cap | tmc's `charmap.txt` (ASCII, `00` ends) and `tmc_strings`' codes | Reached by index (`gTranslations`, a message number in `r0`). Every function readable (2026-10-31): 29 calls pass a constant message (`--names-check`: 29 texts, 0 failed). All 3,699 messages match tmc's `translations/USA.json` (`--messages-check`); no screenshot, since every such call is after the opening festival (the owner accepted the decomp's text as the check, 2026-11-01) |
| Kirby & the Amazing Mirror | none | No table on Data Crystal (a RAM map only) or romhacking.net; the katam decomp has no charmap. Menus are graphics |
| WarioWare, Inc. | none | No Data Crystal page, nothing on romhacking.net, no decomp. Prompts are graphics |

Metroid's and Minish Cap's text is stored plain, not compressed ("Minish"
is in the ROM 152 times as ASCII); for Kirby and WarioWare nothing found
shows their text stored as characters at all.

### Bytes where no table is known

A stopgap for a game whose text encoding nobody has worked out (no
`[text]` in its maps: WarioWare, the other library-named games). Text
can't be told from other data there, so the readable print
(`codegen::text::Preview`, from `lower::text_notes`; unit
mod/data-preview) shows the bytes behind every pool word that points
into ROM data, marked as such:

```rust
// 0x08d2_1e40: [u8; 12] = [0x01, 0x00, 0x20, …]; encoding unknown
m.pool(0x0800_1234, 0x08d2_1e40);
```

- The bytes run up to the next address other code points at (any
  literal pool's word into the cartridge, `Ctx::pointed`, a BIOS copy's
  source left out) or the map names, at most `max_bytes`
  (`crates/codegen/data-preview.toml`, 32); `, cut at 32` says the cap
  ended it.
- A word that is a function's key or lands on code shows nothing.
- Display only: a comment, so the code still reads the ROM and mods
  and asset overrides keep working; `--names-check` counts the
  previews. Where a map has `[text]`, decoded text (above) takes its
  place and nothing else prints. Only a named print shows them, as
  only it shows text. WarioWare, every function readable
  (2026-10-31): 13,836 previews, the same states as the play view.

## The printer applying it

What changes in a named print, and nothing else:

| Where | Unnamed (play view) | Named |
|---|---|---|
| A function's item, its callers' calls, `ext` declarations | `f_0806a519` | `GetMonData2` |
| Link and export names, `table.rs` symbols | `gbaa_f_0806a519` | unchanged |
| A word constant that is a data address | `0x202_44ec` | `addr!(gPlayerParty)` |
| The same, inside a symbol | `0x200_0008` | `addr!(gHeap + 8)` |
| A doc comment on each function | none | `/// GetMonData2, src/pokemon.c` (and `proto`) |

- **Which constants get named.** A constant gets a name only when it is
  the value of a ROM literal-pool word: the words pret-names measured, of
  which 96.9% land on or inside a RAM symbol and 85% on ROM data. It must
  also land on or inside a `[data]` entry. Arbitrary arithmetic constants
  that happen to fall in RAM's range are not named. If the IR's ROM-constant
  folding doesn't yet carry "this came from a pool word", the source-map
  unit adds that flag. It is a bit on the constant, and it changes no
  lowering.
- **`addr!`** is a `macro_rules!` in the generated prelude, and the map's
  data names are `pub const`s in a `names` module of the `base` crate.
  `addr!(gHeap + 8)` expands to `(names::gHeap + 8)`, with the constant
  typed `u32`. As a macro it is one expression wherever it is printed, so
  precedence can't change.
- **Function names don't touch the parts layout.** Parts and modules stay
  as `PART_INSNS` cuts them, not one module per source file, so a named
  build compiles the same crates with longer identifiers. Changing a name
  rebuilds `base` and the parts that use it, as any change to `base` does.
- **The play view stays unnamed.** With no `--names`, the printer's output
  is byte-identical to today's, and the unit's check includes that.

### The check that proves it

Two checks: a mechanical one at recompile time, and an end-to-end one.

1. **Token equality, `gbaa-recompile --names-check`** (Rust, in codegen).
   Print every function twice from the same lowering, once unnamed and
   once named, as `proc_macro2` token streams, before `prettyplease`.
   Walk the two in lockstep. Where the named stream has:
   - an identifier in the map's table: the unnamed stream must have that
     key's unnamed form at the same place (`f_<key>`, a register local, a
     data literal);
   - `addr!(X)` or `addr!(X + n)`: the unnamed stream must have the
     literal `key(X) + n`;
   - a doc comment or an `insn!` string: skip it;
   - anything else: identical tokens.

   Any other difference fails, with the function and the first differing
   token. This holds by construction if the printer names things only
   through the table. The check is what keeps it true when someone adds a
   printing path. It runs over all of Emerald's functions in seconds (no
   rustc). `just check` gets it as a codegen unit test on a hand-built
   program with a synthetic map that names every function, every
   pool-word constant and a keyword and a colliding static, since no ROM
   is in the repo.
2. **Same states, end to end.** `route-check --same-states` on the named
   and unnamed builds of Emerald, over the new-game route with
   `--every 50`, must be byte-identical at every checkpoint. It is the
   recipe shape `just` already has for liveness and block-timed checks.
   It is heavy (two generated crates), so it is the source-map unit's
   check, not part of `just check`.

The readable form's check is a third one, in its section below. A named
and readable print needs all three.

## The readable form

The play view is printed to be compiled. The readable form is printed to be
read, mostly by an agent and sometimes by june, and it is printed only for
the functions on a list. Everything else keeps the play view's form, so a
build costs what it costs today plus the listed functions. The 58% build
cost of `--locals` (`d/registers-rust-locals-cost-58`) is paid only on the
list. (Measured since: the readable lowering costs far less, +6% CPU with
every function listed; "The check" below.)

`gbaa-recompile ROM OUT --names … --readable FILE`: FILE lists functions
by name or key, one per line (`#` comments). `Config` becomes per-function
for `locals` and the macro forms. `Ctx` holds the set, and lowering asks
`cfg_for(key)`.

What a readable function has:

- **Registers and NZCV as locals** (`--locals`, `crates/codegen/src/locals.rs`),
  named from `[locals]` where that applies (below), else `r4`, `nf`. The
  `locals::check` write-back check runs on each one at recompile time. A
  function it fails falls back to `Machine` registers, and the report
  says so, as now.
- **Ticks, calls and returns as macros**, defined in the generated prelude
  so their expansion is exactly today's tokens:
  - `tick!(m, cycles, 0x0803_b299)` for the `if rt::tick(..) { if let
    Some(k) = yield_at(..) { return Flow::Jump(k) } }` block. A tick whose
    yield path writes flags first passes them:
    `tick!(m, cycles, 0x0803_b299; cpsr = …)`.
  - `call!(GetMonData2, 0x0803_b1bf)` for the `sp0`, `settle` and
    `yielding` sequence and the arm that goes on at the return address.
    The `match pc` arms for other returns stay visible, because they are
    real behaviour.
  - `ret!(m, cycles)` for the return-through-`lr` exit.
  - `stall!` and `fetch!` stay calls (`rt::stall(m, 8, …)`). They carry
    the timing a mod must keep, and hiding them would make editing easy
    to get wrong.
- **Quiet instruction text.** `insn!(0x0803_b264, "lsls r0, r0, #17")`:
  one line, ARM UAL syntax as objdump prints it, from `armv4t::text`
  (`instr.text(thumb)`; a Thumb `bl` prints whole at its first half).
  It is `Config::insn` (`gbaa-recompile --asm-text`; `--asm-comments`
  prints the same text as `// 0x0803_b264 lsls r0, r0, #17`). Without
  either the readable form prints no `insn!` at all (2026-10-30, the
  owner's ask: the spike's eight are 259 lines, not 379) and the play view
  keeps the `Debug` text. `just insn-text` checks it against objdump.
  `insn!` still expands to nothing. A re-cut over an edited `src/` file
  cut with the markers first recasts them in `src/` and `orig/` as the
  new print has them (`modcopy::recast_insns`), so an edit next to one
  doesn't conflict.
- **Names and source as comments.** Above the function: the name, the
  file and the prototype. At each block's first instruction:
  `// src/pokemon.c:2254`, from the DWARF line table once
  `mod/dwarf-names` adds lines to the map (a `[lines]` table: start
  address → line, about one entry per statement). Not pret's C text:
  printing the source line itself would put pret's code in our output.
  Their licence would allow it, but the map's rule is names and addresses
  only, and `file:line` already lets an agent with pokeemerald checked out
  read the line.
- **Call return values, where the convention can be trusted.** With locals
  on, the caller reloads `r0` after a call. When the callee's `proto` says
  it returns a value, the call prints as `r0 = call!(GetMonData2,
  0x0803_b1bf);`, an assignment the macro's expansion already contains. A
  comment names the arguments by register: `// (mon: r0, field: r1) -> u32`.
  "Trusted" means all three of these hold:
  1. the callee has a `proto` (on Emerald, from pret's DWARF);
  2. its return type isn't `void`;
  3. the call site returns at the return address (`settle`'s fast path).

  Nothing is assumed about which registers the callee preserves. Locals
  still write back and reload everything at a call, as
  registers-and-types.md requires, so this is a label on a reload, not an
  ABI. (Since `mod/callee-saves`, r4–r11 and sp are not reloaded after a
  call to a function proven to keep them: gap 2 under "How close exact
  Rust gets to pret's C".) A game with no decomp has no `proto`, so its calls print as plain
  `call!` until something (an agent, a human) writes one into its map.

### Naming locals: only where liveness agrees

The register local `rN` of function F takes a name from `[locals]` only
when **every live range of `rN` in F lies inside that variable's ranges**.
A live range here is a definition to its last use, from the lowering's own
dataflow. Then the whole Rust local is that variable, and renaming it is a
plain identifier change the token check can invert per function.

Otherwise, for example where DWARF puts `fixedPersonality` in `r0` for all
of `CreateBoxMon` but it arrives on the stack, the local keeps `r0`. A
comment goes at each range where DWARF and liveness agree:
`// r0: fixedPersonality`. Stack slots (`slot`) are comments at the loads
and stores that hit `sp + slot`, because the stack is guest memory and
stays guest memory.

The unit's report counts how many DWARF register variables became names
and how many became comments. The review's sample suggests at most about
70% of variables could be names.

**Printed (`mod/dwarf-print`, 2026-10-28; `crates/codegen/src/dwarf.rs`).**
The readable lowering keeps registers in `Machine` and its values as
`vN`, so names attach to values, not to `rN` locals: a readable block's
value takes a variable's name where every register holding it is that
variable's home, from its definition to its last use (to the block's end
if a register carries it out), and `name_vN` where an earlier value of
the same name is still in use. agbcc gives a variable one register for
its whole function or lexical block, so liveness alone can't see a
register reused inside the scope; three type checks stand in: a
pointer isn't a narrow load or a shift, an integer isn't an address as it
is, a `u8`/`u16` isn't the `x << 24`/`x << 16` half of its extension.
Values a `pop` or `ldm` loads and branch targets are never named. Known
miss: a temporary in a variable's register, which is also of its type
(a call's argument computed in r0 where DWARF puts `quantity`), takes
its name. A register read in a variable's scope whose name doesn't
print gets `// r4: keyInput` at the block's start (a carried loop
local, `r6`, too); accesses through `sp` between the prologue's last
`sp` write and the next get `// sp+8: name` (agbcc's `breg13` slots are
from `sp` after the prologue: `ClearPokemonCrySongs`' `tmp` at `sp+0`
after `sub sp, #4`). Each `[lines]` row prints `// src/item.c:130`
before its instruction, and the line in effect at a block's first. A
call to a function with a prototype prints `// r0 =
GetItemPocket(itemId: r0): u8` above `call!` (`void` ones without the
`r0 =`): a comment, not an assignment, since this form has no `r0`
local to assign (the decision "Readable form prints call values as
comments"). Comments are `note!("…");` statements that `emit::pretty`
turns into `//` lines, so the token checks see them. `--names-check
--readable LIST` checks the names as it checks function names, and
resolves each local's uses in both prints: a use must reach the same
`let` (it caught a name reused while a value under it was still printed
inside a later sync block). Emerald, every
function readable (`gbaa-recompile --names-check … --readable
mods/emerald/readable/all.list`): of 20,446 register variables (by
identifier per function), 16,481 printed by name somewhere, 2,726 only
as comments, 1,239 neither; 4,607 stack-slot comments, 49,148 call
comments, 203,328 line comments.

### The check

- **Exact.** Every function on the list printed readable, against the play
  view: `sst-gen`, `fn-diff` with a new `readable` form (one more
  `Config` switch, the list = all its picks), the CPU suites (sst,
  gba-tests, armwrestler) with every function readable, `own-state-check`,
  and Emerald's new-game route by `route-check --same-states` against the
  play-view build. The CPU suites and `own-state-check` are the checks the
  `--locals` spike skipped (june's note on `mod/readable-print`).
- **Shown.** `mods/emerald/snapshots/emerald-readable.rs`: five or so
  Emerald functions, named from pret and printed readable, kept in the
  repo, as `just snapshots` keeps the structured ones. A printer change
  shows up as a diff of that file.
- **Cost.** rustc time and peak memory for a 200-function list. Emerald's
  build doesn't change unless functions are listed.

Measured (unit `mod/readable-emerald`, 2026-10-28, quack, the readable
lowering of `mod/readable-lowering`, pret's map):

- **Same states.** Emerald with every function readable
  (`mods/emerald/readable/all.list`, a `*` line: 15,155 of 15,157 listed
  functions, 112,737 of 112,838 blocks in the readable form) against the
  named play view, `just readable-states … --every 250
  units/emerald-newgame.toml`: all 23 frames of the new-game route
  byte-identical, 642,730 bytes each.
- **Cost.** The whole crate's release build after touching every
  generated file (deps built, 6 jobs, through `systemd-run --wait` for
  the peak and `/usr/bin/time` for the largest process; the other lane
  idle, other projects' jobs running):

  | Emerald, named | Wall | CPU | Peak (all rustcs) | Largest process | Parts on disk |
  |---|---|---|---|---|---|
  | Play view | 271 s | 1,549 s | 4.7 GB | 1.12 GB | 399 MB |
  | 200 functions readable (`200.list`: every 75th of the map) | 261 s | 1,527 s | 4.9 GB | 1.11 GB | 396 MB |
  | Every function readable | 283 s | 1,645 s | 4.5 GB | 1.11 GB | 146 MB |

  A 200-function list costs nothing measurable: its difference is inside
  the run-to-run noise of about ±10 s. Every function readable costs +4%
  wall and +6% CPU at the same peak, from source 2.7 times smaller (the
  macros). So, unlike `--locals` (+58% time, +87% memory on p07), the
  readable form's cost is no reason to keep the list short; what limits
  it is only what a reader needs to see.

## How close exact Rust gets to pret's C (spike)

june's decision of 2026-10-26 (`d/mod-view-prints-maintainable-rust-close-to-pret`)
re-aims the readable form at maintainable Rust near pret's C, not
assembly with names. The spike (unit `mod/decomp-spike`, 2026-10-27 on
quack) wrote eight Emerald functions by hand in the closest Rust it could
that stays exact, in forms a printer could emit, and measured them.
The last part of this section is a proposal. The sections above it still
describe the units as they stood.

**Where it is.** `mods/emerald/decomp-spike/` (private, and now in
`mirror/drop-paths.txt`, since it is Emerald translated): `prelude.rs`
(the macros and types), `readable.rs` (the functions) and
`side-by-side.md` (pret's C beside each, with line counts). This doc
holds no Emerald code, because it is mirrored.

### The form

- **Typed addresses from DWARF.** `Ptr<T>` is a guest address of a
  `T`. Globals are `const`s named from the map and typed from DWARF
  (`Ptr<[u8; 4]>`, `Ptr<[Task; 16]>`); a struct gets a marker type with
  its DWARF size and one accessor per field (`t.prev()` is
  `t.field(5)`); `a.idx(i)` is `&a[i]`. C's `x.f[i].g` reads as
  `x.f().idx(i).g()`.
- **Accesses through the machine.** `g.get(at, p)` and
  `g.set(at, p, v)` are the CPU's load and store at `p`'s width, with the
  prefetch stall after them. Values are registers (`u32`): a load widens,
  a misaligned `u16` rotates as on the CPU, and a store takes the whole
  register, which the bus narrows (the bus trace records it, so `as u8`
  would differ). `g.pool(at, lit)` is a literal-pool load.
- **Locals are the C's variables.** The guest registers live in
  `m.cpu.r`, untouched between the points where something else may read
  them; the Rust locals are pret's variables (`i`, `val`, `mask`).
- **The machine, in macros.** `push!`/`pop!` for the frame; `tick!` at a
  block that runs on (its cycles and resume key); `call!(g, SeedRng @ key
  -> ret, cycles, { sync })`, which leaves the function unless the callee
  came back to `ret`; `ret!` for `bx`; `not_taken!` for a branch that
  falls through. Each takes a **sync block**, `regs!(g; 0 = x, 4 = y; n
  = …, z = …)`: the registers and flags the guest has at that point,
  written from the locals before anything can see them.
- **Names.** Callees by `use f_<key> as Name`, and each function exported
  under its link name, so tables and callers are unchanged.

### Exactness

`gbaa-recompile --fn-diff ROM OUT --hand FILE KEYS` (new, for this spike)
picks only KEYS and adds a `hand` form that compiles FILE in their place.
A key the run never enters borrows the entry states of up to eight
Thumb functions it does (`ClearBattleMonForms`, for one, isn't called before
a battle). With `--exact`: **8 functions, 72 states, 1,584 runs a form,
0 differ** from the reference (bus trace, ticks, full state), and the
fault is caught.

What that does not cover:

- **No run yielded** (0 of 1,584), so no `tick!` sync block was
  exercised. They were written to the same rule as the exits, which were.
- **The new-game route with them swapped in was not run.** It needs the
  build to take a hand file in place of generated functions, which is the
  mod copy's `--mod` (`mod/mod-copy`).
- **Two of the ten weren't written**: `CalcCRC16` (u16 arithmetic, a loop
  in a loop, 315 play-view lines) and `GetBgAttribute` (a jump table of
  ten cases, a nested switch, 790 lines). The time went to the first eight
  and the check. A jump table's form is under "Gaps".

### Scores

pret's C against the readable Rust, by function. "C lines" means pret
statements that have one readable line doing the same thing.
"Machine lines" are the readable body's other lines: pool loads, frame,
ticks, sync blocks and reloads. Line counts are function bodies, braces
included, and the readable count also includes the one or two `///` lines
above each function; the play view is `--locals --asm-text`.

| Function (shape) | pret lines | readable | play view | C lines | Named accesses | Loop | Left raw |
|---|---|---|---|---|---|---|---|
| GetGeneratedTrainerIdLower (leaf getter) | 4 | 6 | 35 | 1 of 1 | 1 of 1 | – | none |
| ClearBattleMonForms (array loop) | 6 | 26 | 105 | 2 of 2 | 1 of 1 | `for i in (0..4).rev()`, the target | `forms + i` in sync |
| SeedRngAndSetTrainerId (calls, I/O) | 7 | 19 | 118 | 4 of 4 | 3 of 3, callee named | – | `val` reloaded from r4 |
| GetPlayerFacingDirection (struct array, bitfield) | 4 | 14 | 71 | 1 of 1 (3 lets) | 2 of 2 | – | bitfield as `& 0xf`; `id*36` in a flag |
| Random (u32 arithmetic) | 6 | 20 | 79 | 3 of 3 | 2 of 2 | – | `ISO_RANDOMIZE1` as pool constants; `rt::multiply` |
| GetBagItemQuantity (pointer, u16) | 4 | 15 | 65 | 1 of 1 (4 lets) | 3 of 3 | – | parameter `Ptr::at(r0)` (printed now: gap 7); `as u16 as u32` |
| EnableInterrupts (u16 or, calls) | 7 | 24 | 145 | 4 of 4 | 2 of 3 | – | second `sRegIE` read through r4 |
| ResetTasks (struct-array loop, memset) | 17 | 49 | 224 | 9 of 9 | 7 of 9 | `loop { … break }`, not `for` | `((i<<2)+i)<<3`; reloads; `TaskDummy` as a pool word; `\| r5` |
| **Eight** | **55** | **173** | **842** | **25 of 25** | **21 of 24** | 1 of 2 as `for` | |

- **Every C statement has its line**, every global and field is named
  but three accesses, and the loop that june gave as the target came out
  as given. agbcc compiled pret's up-counting loop as a down-count, and
  the store order is behaviour, so `(0..4).rev()` is the exact form.
- **The readable body is about 3 times pret's and a fifth of the play
  view's.** Of the 123 non-brace lines, 44 are C lines and 79 are
  machine lines (64%). By kind, the machine lines are sync blocks and the
  flags in them (about 29), pool loads (18), frame push/pop (11), `ret!`
  (8), ticks and `not_taken!` (6), reloads after a call (4), and a
  parameter and an `| r5` read straight from `g.cpu.r`.
- **Each access carries its instruction's address**: the prefetch stall
  after it depends on where it runs. That is the one piece of timing that
  sits inside C lines.

### Gaps, and what each would take

By technique, from the literature in [research/name-recovery.md](research/name-recovery.md),
[research/registers-and-types.md](research/registers-and-types.md) and
[research/structured-control-flow.md](research/structured-control-flow.md).

1. **Sync blocks at exits and ticks** (the largest share). The C caller
   never reads r1–r3, r12 or the flags after a return, but the exact
   state has them. Decompilers drop them by trusting the calling
   convention (Hex-Rays, Ghidra, rev.ng's `abi-enforcement-level`;
   registers-and-types.md's table: not exact). Keeping exactness, the
   printer can **move them out of the body**: one `macro_rules!` per
   exit, defined at the top of the function, or a trailing
   `exits! { … }` block, so the body reads as C and the sync is still
   there. Dropping them needs an interprocedural proof that every return
   site writes them before it reads them. That is a liveness over the call
   graph, and route same-states would check it, but fn-diff would no
   longer compare full state, so it is a change to what "exact" means.
   That is june's call; the default is to keep them, moved out.
2. **Reloads after a call** (3 of 8 functions). Locals in r4–r7 are
   re-read after each call, because nothing promises the callee kept
   them. That also keeps `ResetTasks` from being a `for`. Ghidra and
   rev.ng assume callee-saves from the ABI. Exact instead: **prove it per
   callee**, that every path restores r4–r11 and sp (agbcc's push/pop
   pairs make this a pattern check over the callee and its callees, with
   any unknown indirect call failing it). Where the proof holds, the
   reload goes, and the induction variable survives the call.

   **Built** (unit `mod/callee-saves`, 2026-10-27): `codegen::saves`
   runs each function's IR from its entry, tracking what each register
   and each stack word at a constant offset from the entry sp holds (an
   entry value, the entry sp plus a constant, a constant, or unknown).
   A function keeps r4–r11 and sp if every `bx` to its entry lr finds
   them as they came, and everything it calls or tail-jumps to keeps them
   (the greatest such set, so recursion passes). A call through a register
   or a stub, a jump nothing names, a mode change, a reset `swi` or IWRAM
   code fails it. Jumps to discovery's continuation roots (jump-table
   cases) are followed as part of the function. Two assumptions, which
   C relies on: a store to an address it can't place, or above the entry
   sp, doesn't hit saved slots, and IRQ handlers restore r4–r11. `Config::callee_saves`
   (`--callee-saves`, with `--locals`; on in `--print`) prints a proven
   call's reload as `if pc == RET { reload_kept!(); } else { reload!(); }`,
   so an unwind past the return site still reloads everything, and
   `locals::check` accepts `reload_kept!` only right after a call to a
   proven function. On Emerald, **8,894 of 15,158 functions** pass
   (`gbaa-recompile --saves ROM`, 20 s). Of the 6,264 that fail, 506
   make an indirect call themselves and 5,617 call something that fails,
   mostly through those (the deepest roots: `GetExpandedPlaceholder`,
   `TrackStop`, `CallWindowFunction`, `RenderFont`, task and sprite
   callbacks); 126 jump to code discovery never reached (after a call it
   thought doesn't return). `ResetTasks`, `SeedRngAndSetTrainerId`,
   `EnableInterrupts` and `LoadPalette` pass, and `ResetTasks`' loop
   keeps r4–r7 across its call. fn-diff's `saves` form (`--exact`, every
   function the run enters: 270 picks, 337 compiled, 267 of them proven,
   146 picks calling one, 451 of 568 call sites printed with
   `reload_kept!`): **0 of 13,002 runs differ**.
3. **Literal-pool loads** (18 lines). They are bus accesses with a stall,
   so they stay, but a pool load whose value is only used by the next
   access can be **folded into that access** (`g.get_via(at, lit, x)`),
   with the order unchanged. The IR already knows which constants came
   from a pool word (the source map's `addr!` rule). Most of the 18
   fold this way: all but the ones whose value is used twice or after
   another access.

   **Built** (unit `mod/readable-loops`, 2026-10-28): `lower::fold_pools`
   folds a pool load into the next access when nothing names its value
   (const-prop took it), that access's address is the pool word (within
   an immediate offset, under 0x80), and no other access, timing, stall,
   yield or exit runs between them; it prints as `m.get_via(pool_at, lit,
   at, p)` (`set_via`, `load_via`), the `Game` trait's default methods
   that do the pool load and then the access. A load from a ROM word
   right after (`ldr r0, =X; ldr r0, [r0]`, itself an `m.pool`) doesn't
   take one. In fn-diff's readable module 354 pool loads fold and 561
   stay; in the snapshot's five functions 7 of 10 fold (the three left
   feed a tick's sync or a multiply). The body has a statement fewer per
   fold, but rustfmt wraps the longer calls, so the snapshot grew by 24
   lines: offsets from the function's start (gap 10) would undo that.
   Exact: fn-diff `readable` 0 of 13,002 runs differ (`--exact`).
4. **Loop shapes.** `for` needs an induction variable with an init, a step
   and a compared bound. This is scalar-evolution-style recognition over the structured
   loop (structured-control-flow.md's loop nest, plus the IR's values), which is
   cheap for agbcc's shapes. Down-counts stay down-counts. Where the
   variable is reloaded (gap 2), the loop stays `loop { … break }`.

   **Built** (unit `mod/readable-for-loops`, 2026-10-28): `lower::Carry`
   holds a register in a Rust local `rN` across a structured readable
   function's blocks where every edge into a block is a tick of a
   readable block that writes it or carries it on (forwards: where the
   local equals `Machine`'s register; backwards: where a block reads it
   or carries it on). The tick's sync block still writes the register
   and then sets the local (`r0 = v7;`), and `tick!` takes the carried
   registers after a `;` (`; 0 = r0, 1 = r1`): a yield taken in place
   that left one changed unwinds to the dispatcher at the tick's key, so
   nothing is assumed about IRQ handlers. A call, a return, a play-view
   block, the entry and a collapsed region end every local, so
   `ResetTasks`' counter is carried round its back edge but `gTasks` and
   its `+ 8` are reread after `memset` and the loop stays `loop`.
   `lower::For` prints a one-block loop as `'l1: for r0 in
   (0x202_44e4_u32..=0x202_44e7).rev()` (`.step_by(36)` for a struct
   array) when one tick enters it setting the variable to a constant and
   its branch back steps it: running the block's IR from that constant,
   with the other registers it reads constant from the entry and carried
   round unchanged, takes the branch at every value but the last, at
   most 4,096 times. The branch and the block's exits stay as they were,
   so the last value leaves by the block's own exit, and `unreachable!()`
   follows the loop; the branch back's tick checks the variable against
   the value it steps to. On Emerald with every function readable, 188
   loops print as `for`; in fn-diff's readable module 9, with locals
   declared in 499 functions. Exact: fn-diff `readable` 0 of 13,002 runs
   differ (`--exact`); every function readable type-checks. Of the
   spike's eight, `ClearBattleMonForms` is a `for`, `ResetTasks` a
   `loop` (as the hand version), and their readable bodies are 370 lines
   (250 without `insn!`), 5 more than before: 24 `m.cpu.r` reads where
   there were 29, for the locals' declarations and the `unreachable!()`.
5. **Strength-reduced indexing.** `((i<<2)+i)<<3` plus a base is
   `idx(i)` when 40 is the element's DWARF size. This is a peephole on the
   address expression, the type-guided array recovery Hex-Rays does and
   DIRTY learns. The intermediate sum is still needed where a flag sync
   reads it.

   **Built** (unit `mod/readable-indexing`, 2026-10-28): the lowering
   reads each fused access's address as a constant plus a value times a
   scale (`print::linear`, through adds, shifts left and multiplies by a
   constant); where the constant lands in a typed datum and the scale is
   the size of an array's elements there (`Types::array_at`), the access
   prints `gObjectEvents.idx(id).facingDirection()` and names the index,
   not the address, so the shifts and the sum go unless something else
   (a flag sync) reads them, where they stay inline. A byte array's index
   is taken only for a datum that is one. A register counts as a constant
   where every edge into its block sets it to that constant or carries it
   on: each a tick, or a call to a function `codegen::saves` proves keeps
   r4–r11 and sp (which carries only those), so a readable list now
   proves saves (`lower::known`). That makes `ResetTasks`' r6 `gTasks`
   across its `memset` and its loop `gTasks.idx(r4).isActive()`, and a
   register holding a datum's address the datum (`EnableInterrupts`'
   second `sRegIE`, through r4 after a call). `bits_kept` now skips
   readers that aren't printed (`lsls`'s dead carry) and takes the carry
   of the `lsrs` after it, so the bitfield is named. Exact: fn-diff
   `readable` 0 of 13,002 runs differ (`--exact`). In its readable
   module, 782 of 2,701 accesses get a full path (435 before), 89 a
   partial one (35), 1,830 stay raw (2,231); 255 go through a variable
   index. Of the spike's eight, 22 of 26 accesses print a path (15
   before): all of `ResetTasks`' 8, `GetPlayerFacingDirection`'s 2 and
   `EnableInterrupts`' 4. The four left are the I/O registers
   `REG_TM1CNT_L`/`_H` (no map entry), `GetBagItemQuantity`'s parameter
   (gap 7) and `ClearBattleMonForms`' store through `r0`, a pointer the
   `for` steps over addresses, not an index. The eight's bodies are 370
   lines (250 without `insn!`), two fewer. Not yet: a pointer loaded
   from a datum, indexed (`Ptr::<T>::at(v)` plus a scaled value), and
   pointer induction variables.
6. **Types: DWARF gives them, but at agbcc's access widths.** Globals,
   struct layouts, bitfields and prototypes all come from pret's DWARF (gdb
   read every one the spike needed). The printer must take each access's
   width from the instruction, not the C type: agbcc reads a `u16 : 4`
   bitfield with `ldrb` and two shifts. A bitfield accessor prints as
   `obj.facingDirection()` only where the access matches the pattern for
   its DWARF offset and width; otherwise it prints `& 0xf`.
   Values stay register-wide (`u32`) and C's narrow types show only as
   casts where the code masks. Recovering types for a game with no decomp
   is the TIE/Retypd/DIRTY problem (registers-and-types.md), and is out
   of scope here.

   **Built** (unit `mod/readable-types`, 2026-10-28, with gap 8):
   `codegen::names` loads `[types]` and `codegen::types` reads each C
   type as far as layout (`u8`/`u16`/`u32` by size, `Ptr<T>`, `[T; N]`,
   a struct or union's marker, else `[u8; N]`). A named crate gets a
   `globals` module beside `names` (`base`'s, glob-imported by every
   part; a `fn-diff` form's own): each typed datum as `const gTasks:
   Ptr<[Task; 16]>`, each struct and union as `pub enum Task {}` with its
   `Guest` size and one `const fn` accessor per field in `impl Ptr<Task>`.
   A bitfield's accessor points at the narrowest aligned access holding
   its bits (`facingDirection` is the `u8` at 24). A readable access
   prints a path where its address is a typed datum's, or a pointer
   loaded from one in the same block, plus a constant: the path names a
   field only at the instruction's width, and a bitfield only where its
   access is that narrowest one and, when the byte holds others, the
   load's one use (`& 0xf`, `>> 4`, `<< 28 >> 28`) keeps exactly its
   bits; else it is the deepest object holding the access and
   `.field::<u8>(off)`. So `gTasks.idx(15).next()` and
   `Ptr::<SaveBlock2>::at(v10).encryptionKey()`. Emerald's map gives
   24,602 typed globals. In fn-diff's readable module (270 picks), 435
   of 2,701 accesses get a full path, 35 a partial one, and 2,231 stay
   `Ptr::<uN>::at(v)`: their addresses come in registers (parameters,
   loop variables, computed indexes), which gaps 4, 5 and 7 type. Exact:
   `readable` 0 of 13,002 runs differ (`--exact`). On the spike's eight,
   13 of the hand versions' 21 named accesses come out named; the rest
   are indexing (`gBattleMonForms`, `gObjectEvents`, `ResetTasks`' loop),
   the parameter `quantity`, and the I/O registers (`REG_TM1CNT_L`),
   which have no map entry. A mod's `lib.rs` doesn't import `globals`
   (its own files may define the same names); a cut's file does.

   **Bitfield writes** (unit `mod/bitfield-writes`, 2026-10-30, from
   `docs/research/agent-first-mod.md`, item 1): a bitfield that shares
   its access says its bits in its accessor's type (`optionsTextSpeed()`
   is a `Ptr<Bits<u8, 0, 3>>`, which loads and stores as its `u8`). A
   byte load, its value's run of bits replaced (`(x & k) | c`, `x & k`,
   or `x | c` setting all of the run) and a store to the same address
   print as one `m.set_bits(lat, sat, p.field(), v)` (`set_bits_via` with
   a pool load folded in) where that run is exactly one bitfield the
   store's address names; it expands to the same load and store, and
   gives back the value read and the value stored, bound where they are
   used later (`let (v95, v104) = m.set_bits(..)`, for an exit's
   registers). Only where nothing that could see the order runs between
   the two (no other access, timing, stall or exit), so all of it prints
   at the store; else the load and store print as before. So
   `SetDefaultOptions` (`new_game.c:93-98`) is six `set_bits`, the edit
   to a default the value alone. Exact: readable-states on Emerald, every
   function readable, the same.

   A field set from a computed value `y` (unit
   `mod/computed-bitfield-writes`) prints the same, `v` being `y`, where
   the run is `(x & k) | e` and `e` is `(y & m) << lo`, `(y << lo) & (m <<
   lo)`, `y & m` (the run at bit 0), or `y << lo` for a run reaching the
   byte's top bit (only where the value stored is used by the store alone:
   `set_bits` gives back the field's bits, not `y`'s above the byte), `m`
   the run's ones; each of `e`'s steps used only by the next. So
   `Task_OptionMenuSave` (`option_menu.c:353-358`) is five `set_bits` and
   `just mod-find` counts it a writer of each field.

   **I/O registers** (unit `mod/readable-io-names`, 2026-10-28):
   `codegen::io_regs` is pret's `REG_*` table (`include/gba/io_reg.h`'s
   names, addresses and `vu8`/`vu16`/`vu32` widths, plus GBATEK's JOY
   registers, `POSTFLG` and `HALTCNT`), one name per address and width.
   The `globals` module declares each as a `Ptr` constant (`const
   REG_TM1CNT_L: Ptr<u16>`), and the identifier table reserves the names
   (a map datum called `REG_IE` would print `REG_IE_`). A named readable
   access at a constant address (or a constant plus one) that no typed
   datum holds prints as the register of its width there, else the
   narrowest register holding it and `.field::<uN>(off)`
   (`REG_VCOUNT.field::<u8>(0)`). So `SeedRngAndSetTrainerId` reads
   `REG_TM1CNT_L` and clears `REG_TM1CNT_H`. Unnamed prints are unchanged.
7. **Parameters and return values from prototypes.** With `proto`, the
   printer can print `let quantity: Ptr<u16> = arg!(0);` and
   `ret!(… return dir)` rather than `Ptr::at(g.cpu.r[0])` and `regs!(g;
   0 = dir)`.

   **Built** (unit `mod/readable-protos`, 2026-10-28): `codegen::types`
   reads each function's `proto` as far as its registers
   (`Types::proto_of`, a `Proto`): r0–r3's parameters, each an integer
   (register-wide, `u32`) or a pointer (`Ptr<T>`, an array parameter a
   pointer to its element), ending at the first that isn't one register
   (a struct by value, 64 bits, `...`), and whether r0 is the value
   returned (not `void`, 64 bits or a struct, whose hidden first argument
   leaves it no parameters either). A readable entry block that nothing in
   its function jumps back to (`lower::entered_once`) reads r0–r3 as the
   function was called with them, since its writes wait for its exits, so
   it starts `let quantity: Ptr<u16> = arg!(m, 0);` for each it reads, and
   each read prints the name (`quantity.0` where a `Ptr` needs its
   address); an access through a pointer parameter plus a constant prints
   its path (`m.get(at, quantity)`, `sprite.oam().affineMode()`). Where a
   value of that block takes the same DWARF name (agbcc's copy into a
   saved register, a `u16`'s `<< 16 >> 16`), the parameter is
   `name_arg`, so the value's `let` can't shadow it: `let itemId: u32 =
   (itemId_arg << 16) >> 16;`. A return (`ret!` out to the `lr` or stack
   word the caller left) of a function that returns a value, whose block
   writes r0, prints it as `ret!(m, pc @ …, …, return v40, { regs!(m; 1 =
   v30; …) })`: `arg!` and the `return` write are what `regs!` and
   `m.cpu.r[0]` were, in the same order (r0 is the sync's first write),
   so the form is the same machine. Agreed with `mod/dwarf-print`: a
   call's value stays its comment (`// r0 = Random(): u16`), since there
   is no `r0` local to assign; the return is a macro argument, not an
   assignment. Like `[types]`, prototypes are off in `--names-check
   --readable` (it clears `typed`), whose token walk can't invert a
   restructured access. On Emerald with every function readable, 12,940
   parameters print (5,075 pointers; 6,440 as `_arg`), and 2,187 of
   15,144 `ret!`s a `return`: the rest are `void` or return from a shared
   epilogue whose r0 an earlier block set, which shows only in that
   block's sync (`0 = 1`). Exact: fn-diff `readable` 0 of 13,002 runs
   differ (`--exact`); every function readable type-checks. Of the
   spike's eight, both parameters print (`quantity: Ptr<u16>`,
   `mask: u32`) and all four values returned, 23 of 26 accesses print a
   path (`GetBagItemQuantity`'s through `quantity` too), and the bodies
   are 379 lines (259 without `insn!`, 213 without comments either).
8. **Constants.** A pool word that is a function key prints as its name
   (`TaskDummy`), as data addresses already do. C macros
   (`ISO_RANDOMIZE1`, `TAIL_SENTINEL` compiled to `orr` with -1) aren't
   recoverable and needn't be. `rng * 0x41c64e6d + 0x6073` reads well.
   **Built** (`mod/readable-types`): in every named print, `names` holds
   each function's key too, and a pool word that is one prints as
   `addr!(TaskDummy)`; names-check maps it back to the key.
9. **A jump table** (not written). Its form would be `match` on the
   switch value less the table's base, with one arm per
   `analysis::jumptable` case and `_` for the default edge; the `mov pc`
   exit and its tick stay as one `switch!` macro line. Its case values come
   from the table, and its names (`BG_ATTR_*`) need pret's enums from DWARF,
   which `DW_TAG_enumeration_type` gives.
10. **Instruction addresses on accesses.** These are needed for the stall,
    and there is no literature on it, since decompilers drop timing. Printing them as
    offsets from the function's start (`+0x0a`) would make them shorter.

### What the readable-form units should become

Proposed, for the units of this group. The owner's decision overrides
two lines above: **"no struct layouts in code"** under "What it holds",
and "Types, if at all, stay in that form". The readable form now needs
DWARF types in code, as `Ptr<T>` accessors.

- **Readable print form for chosen functions (`mod/readable-print`)**
  becomes a **new lowering**, not macros around the `--locals` print. It
  prints from the IR and the structuring plan into the spike's form: the
  prelude from `mods/emerald/decomp-spike/prelude.rs` moved into the
  generated prelude, accesses as `get`/`set`, locals as variables, sync
  blocks moved out of the body (gap 1), pool folding (3), `for`
  recognition (4) and indexing (5). Its check is the spike's `hand` form
  made a `readable` fn-diff form, plus the spike's eight functions printed
  automatically, exact, at least as close as the hand versions by the
  table above. Split it if it runs long: accesses, locals and sync first
  (no types, `Ptr<u8>` from raw addresses), then loops and indexing.
  **Split, 2026-10-27.** `mod/readable-print` did the part both forms
  need: the readable list (`gbaa-recompile --readable FILE`, by key or
  name), `Ctx::readable` and `Ctx::cfg_for(key)`, the `readable` fn-diff
  form, `just sst-gen --readable`, the report's count and fallbacks, and
  `mods/emerald/snapshots/emerald-readable.rs`. A readable function is
  for now `--locals` and `--asm-text`. The new lowering is
  `mod/readable-lowering` (accesses, locals as variables, sync blocks
  moved out, the spike's prelude) and then `mod/readable-loops` (`for`,
  pool folding, indexing, types); the snapshot shows each as it lands.
  A named print with locals needed one fix: `locals::check` refused
  `addr!`, so every named readable function fell back to `Machine`.
  **Built, `mod/readable-lowering` (2026-10-27).** `Config::readable`
  (on for the list; `locals` off for it) prints each Thumb ROM block
  so: its `SetReg`s and `SetFlag`s leave their places and are written,
  the last of each before the exit, in that exit's sync block
  (`regs!(m; 0 = v5, 13 = …; n = …)`, `lower::Readable`), so the guest
  registers stay in `Machine` untouched between exits and the block's SSA
  values are its locals (`let v19: u32 = m.get(…)`); an access whose
  instruction has a stall and no other access is `m.get`/`m.set`/`m.pool`
  (`print::Hints::fused`), a misaligned load's rotate folded into
  `m.get`; a Thumb `push`/`pop` is `push!`/`pop!`; and exits are
  `tick!`, `call!` (on only at its return site, the `match pc` gone),
  `call_any!` and `ret!`, each with its sync block. Other exits keep the
  play view's statements after the sync. The spike's types and accesses
  (`Guest`, `Ptr`, `Word`, `Game`) are in every generated prelude
  (`emit::readable_items`), its macros' generated forms at each crate's
  root (`emit::readable_macros`); the hand file keeps its own macros,
  pasted in rather than `include!`d so they shadow. A block keeps the
  play view's statements if it is ARM, touches the PSRs or the user bank,
  has a guarded write, or reads a register it already wrote
  (`report.txt` counts the blocks). Sync blocks are a trailing argument
  of the exit's macro, not a `macro_rules!` at the top of the function:
  macro hygiene resolves a local named in a macro body where the macro is
  defined, before the `let`s it would name. On the spike's eight
  functions the readable bodies are 353 lines (233 without `insn!`)
  against the play view's 877 and the hand versions' 173. Values are
  still `vN`, registers reread at each block start; names, loops, pool
  folding, indexing and types are `mod/readable-loops` and
  `mod/dwarf-names`.
  **Split, 2026-10-28.** `mod/readable-loops` did pool folding (gap 3,
  above). The rest are units of their own: loop variables held across
  a back edge and `for` (gap 4, `mod/readable-for-loops`), `Ptr<T>`
  globals, field accessors and function-key constants from `[types]`
  (gaps 6 and 8, `mod/readable-types`), `idx(i)` (gap 5,
  `mod/readable-indexing`, after both) and parameters and returns from
  prototypes (gap 7, `mod/readable-protos`). `mod/dwarf-print` waits on
  the loop unit, whose variables its names attach to.
- **DWARF locals, lines and prototypes for Emerald (`mod/dwarf-names`)**
  also writes **`[types]`**: each global's type, struct and union layouts
  (field, offset, size, bitfield bits) and enums. `mod/readable-print`
  then needs it for names and types. The first half can run before it.
  **Split, 2026-10-28.** `mod/dwarf-names` did the map: `pret_names::dwarf`
  reads the ELF's DWARF 2 with gimli after mending two agbcc faults (each
  object's `.debug_abbrev` table is unterminated and runs into the next,
  so it is cut at its first repeated code; 27 line programs' lengths fall
  1 to 5 bytes short of the next program). `just pret-names` now writes
  `src` and `proto` on functions (15,025 of 15,143 named), `type` on
  data (24,602 of 56,486: the rest are asm and ROM tables with no
  DWARF), `[locals]` (20,651 register and 2,545 stack-slot variables;
  7,869 more have no location, which agbcc drops for a variable kept in
  more than one place), `[lines]` (186,515 rows: a number in the
  function's `src`, else `"file:line"`) and `[types]` (652 structs,
  unions and enums with field offsets, sizes, types and bitfield bits
  `[lowest, width]`, little-endian; 167 names with two layouts keep the
  first). Every unit and line program reads. agbcc gives one location for
  the whole function or lexical block, so a local's `from`/`to` is that
  scope, not a live range. `codegen::names` loads `[locals]` and
  `[lines]` (`SourceMap::locals`, `lines`, `Item::src`); `[types]` is
  read by `mod/readable-types` (gap 6). The named print's doc comment now has the
  prototype. The printer's half (names where liveness agrees, `file:line`
  per block, `r0 = call!`, the report and checks) is
  `mod/dwarf-print`.
- **Readable form exact on the CPU suites and own-state-check
  (`mod/readable-exact`)** is unchanged in what it runs, but it now tests a
  different lowering. Add fn-diff states recorded at IRQ points, so that
  sync blocks on the yield path run: the spike's runs never yielded.
  **Run, 2026-10-28.** A readable list's `*` (`mods/emerald/readable/all.list`)
  prints every function readable; `just readable-states` builds it beside
  the play view and compares their exact states (`route-check
  --same-states`). `just readable-suites --every 25`: jsmolka's five tests and both
  armwrestler builds, along new routes (`units/jsmolka-*.toml`,
  `units/armwrestler-*.toml`, every page a checkpoint), the same at every
  point; the form takes Thumb ROM blocks, so thumb.gba had 122 of 177 in
  it and armwrestler 64 of 201, the ARM tests almost none. `just sst-gen
  500 4000 --readable`: 0 differ. `own-state-check` with `all.list`
  passes on Emerald (1000 from reset, 100 frames, 75 distinct; every
  one of its 15,167 functions readable, 112,738 of 112,940 blocks in the
  form, IWRAM copies' included) and armwrestler (200, 150). The yield path's sync blocks were already exercised by fn-diff
  on Emerald (`readable` 0 of 13,002 runs differ, 737 of them yield), so
  no IRQ-point states were added. Nothing differed, so nothing in the
  printer changed; the report now counts the IWRAM copies' functions
  against the list (it called them "not functions").
- **Readable form on Emerald: same-states route and build cost
  (`mod/readable-emerald`)** is unchanged, plus the spike's hand file
  swapped in once `--mod` exists (`mod/mod-copy`), as a second route
  check.
- **New: callee-saved registers proven per callee** (gap 2), a codegen
  analysis with its own check (fn-diff with the reloads dropped, on every
  function the proof passes). It is the one change that turns most `loop`s
  back into `for`s.
- **New, june's call: whether exact may stop at the return** (gap 1). If
  scratch registers and flags dead at every return site may differ, the
  sync blocks at exits go. The default is no: keep them, moved out of the
  body.

## A mod copy

Decision 2: names stay outside the code until a mod starts, and from then
on the mod is ordinary Rust in git. The cut is the moment code leaves the
generator.

- **Where.** `mods/<game>/<mod>/` in this repo, which is private. The
  generated code is the game translated (decision 2's legality line), so
  a mod is a few dozen functions at most, never the whole game, as the
  Emerald snapshots already are. It never goes in a public repo.
- **The cut.** `gbaa-recompile ROM --names … --cut mods/emerald/<mod>
  FUNCS…` writes:
  - `mod.toml`: `rom_sha1`; the gbaa commit and the map's `source` at the
    cut; the functions by key and name.
  - `src/<Name>.rs`: one per function, readable and named, with its
    `#[unsafe(export_name = "gbaa_f_<key>")]`.
  - `orig/<Name>.rs`: the same prints, never edited, so a later re-cut
    can be three-way merged (`git merge-file src orig new`) when codegen
    or names change underneath.
- **The build.** `gbaa-recompile ROM OUT --names … --mod mods/emerald/<mod>`
  doesn't print the listed functions. It adds a `modded` part crate whose
  `lib.rs` takes each of the mod's `src/*.rs` (and any helper files the mod
  adds) as a module by `#[path]`. `table.rs` and every caller reach a
  modded function through its link name, so they are unchanged; the other
  functions keep the parts an unmodded build gives them. A mod can also add new functions. They
  have no key, so they are only reachable from modded code, as plain Rust
  calls.
- **Macros and types of a mod's own.** The `modded` crate's root defines
  the generated `regs!`, `push!`, `tick!`, `ret!`, `call!` and `pop!`
  ahead of the mod's modules. A mod file can shadow them with its own
  `macro_rules!` of the same names, written out in that file; one that
  arrives through `include!` can't (rustc's E0659, a macro-expanded macro
  may not shadow). Types and helpers go in a helper file in `src/`, as
  `pub` items that the function files reach through `use super::*`.
  Fields on the generated `Ptr<T>` go in a trait of the mod's own, since
  an `impl Ptr<T>` would be an inherent impl on another crate's type.
  `mods/emerald/spike-hand/` (the spike's eight hand-written functions,
  unit `mod/spike-hand-mod`) is built that way: same states as the named
  build on the new-game route.
- **A first edit.** `mods/emerald/text-speed/` (unit
  `mod/agent-first-mod`) edits `SetDefaultOptions` to fast text and window
  frame 5; what was hard to read or edit, ranked, with the fix each
  suggests: `docs/research/agent-first-mod.md`.
- **Resumes.** With the guest stack, a resume inside a function is rare
  (one a frame on Emerald, in m4a), but a save-state load, a rewind or a
  `MAX_DEPTH` unwind lands at a `cpu.pc`. If that is inside a modded
  function, the dispatcher runs the original ROM code from there in the
  interpreter until it returns, so the mod is skipped for that one call.
  The cut keeps the function's inner keys as the generator made them, so
  resumes at those keys enter the mod's copy. The runtime counts resumes
  that land inside a modded function's extent, and the desktop's report
  prints the count, so the gap is visible and not silent. Not allowing it
  at all would mean refusing save states with mods, and that is june's
  call if it ever bites. A call the mod added has no ROM code to go on in;
  its record does that ("A call a mod adds", below).
- **The check** (unit `mod/mod-copy`): an unedited cut of ten functions
  builds, and the new-game route's states are byte-identical to the
  unmodded named build (`route-check --same-states`). Then a one-constant
  edit (for example the starting money) shows on screen. The resume count
  is reported for the route.

### Editing a cut

The readable form keeps three kinds of number that pret's C doesn't have,
so that an unedited cut runs cycle for cycle as stock. Every cut file's
header (`codegen::modcopy`'s `HEAD`) says in a few lines what each is and
what an edit does with it; this is the why (`docs/research/agent-first-mod.md`,
item 4, found an editor left guessing).

- **An access's address** (`m.get(0x0808_4340, …)`, the `pat`, `lit` and
  `at` of `get_via`): the Thumb instruction the access was. The `Game`
  trait (`codegen::emit`) uses `at` only to price the prefetch stall after
  the access (`rt::stall`, by `at`'s region and the next fetch), and `lit`
  as the pool word the pointer is loaded from; neither picks what is read
  or written. An access an editor adds takes the nearest one's addresses:
  that runs from the same region, so its stall is priced as the
  neighbour's would be, and the value is right whichever is given. A
  made-up address outside ROM would price the stall as for another region.
- **The cost** of `ret!`, `tick!` and `call!` (`54 + rt::fetch(m, 8, true,
  21, 20)`): the cycles of the block before the exit, as the generator
  counted them for the code as cut. It stays as cut. An edit changes what
  the code does, not what the guest's instructions were, so there is no
  right count to recompute; the game sees a few cycles' difference only as
  timing (a later IRQ, a later frame end), which a mod isn't expected to
  keep exact. Exact timing is what an unedited cut proves (`mod-states`).
- **`regs!`**: the guest registers and flags the block leaves in
  `Machine` (the readable form keeps registers there, not in locals).
  Callers see them. r0 is the return value of a function that has one, and
  r4-r11 and sp are callee-saved, so those must stay as cut (or, for r0,
  be the edited return value). The rest (r1-r3, r12, the flags) a caller
  doesn't rely on under the C ABI; they are there so that an unedited cut's
  states are byte-identical to stock. An edit that deletes the line whose
  value `regs!` names gives that register any value (0); it then differs
  from stock in a register nothing reads.
- **`pc @ …`** in `ret!` is the exit instruction's address, which
  `rt::exit` hands the CPU with the exit; it stays as cut.
- **`call!(m, pc, f -> ret, …)`**: `ret` is the return site. The macro
  writes it to lr (the guest's `bl` does), so `regs!` doesn't repeat it,
  and the callee pushes it, returns through it and `call!` checks it came
  back there; it stays as cut. `f`'s entry key isn't spelled: the macro
  takes it from `keys::f`, a constant per function in the generated
  prelude (`emit::keys_module`), so a call reads as the callee's name
  (`mod/call-writes-lr`, from `docs/research/code-without-rom-addresses.md`).

A re-cut merges the new print into `src/` (`git merge-file`): the header is
the same text in `src/`, `orig/` and the new print unless the generator
changes it, and then an editor's untouched header takes the new one with no
conflict.

### A call a mod adds

A call the cut didn't have has no ROM return site, so a mod writes it with
none (`mod/mod-call-form`, from `docs/research/code-without-rom-addresses.md`):

```rust
call!(m, pc, VBlankIntrWait);
call!(m, pc, m4aSongNumStart, { regs!(m; 0 = SE_JUMP); });
let n: u32 = call!(m, pc, Random); // the callee's r0
```

- **The return key.** lr gets a reserved key above everything the GBA
  maps, `0xF000_0000` and up (`rt::modcall::call_key`, an FNV hash of the
  call's `file!()`, `line!()` and `column!()`, Thumb bit set). The callee
  pushes it, returns through it, and the macro takes that key as "came
  back", as `call!` does a ROM return site. The call costs no cycles.
- **The record.** Below the callee's stack the call leaves 18 words of
  guest RAM: r0 to r14 and the CPSR as the block found them (before the
  sync), `pc` (the key the block began at: the function's entry, or the
  last `tick!` or `call!`'s target) and the reserved key. Back at the key,
  every register and flag is put back as the call found them
  (`rt::modcall::leave`), sp above the record and lr too: the code around
  the call sees only what the callee did to memory, and its r0 as the
  macro's value. (The first try left lr at the reserved key, and
  `GetInput`'s prologue, after the call, pushed it as its own return.)
- **A frame lost inside the call.** A save-state load, a rewind, a
  netplay rollback, an unwind (`--unwind`, `MAX_DEPTH`) or a script that
  moves the CPU drops the mod's Rust frame while the callee is running.
  The record is guest RAM, so it survives: when the callee returns to the
  reserved key and nothing waits there, the dispatcher puts the record's
  registers back and goes on at its key (`rt::modcall::resume`). That runs
  the block again from its start: the stock code in the interpreter if the
  block is past the function's entry, as any resume inside a modded
  function does (see Resumes above), or the mod's function from its top if
  it is the entry. Each is counted, as `Stats::mod_call_resumes` and one of
  `mod_resumes`, and the desktop's `mod:` line prints both. A reserved key
  with no record at sp is a fault (`Fault::ModCall`), not garbage run.
  A script's breakpoints run frames in the interpreter, which does the
  same at the key (`Machine::run_frame_debug`). Without the guest stack
  (`--unwind`) every frame end inside the call loses it, so a call that
  always spans one (the half-speed check's `VBlankIntrWait`) runs again
  and again and the game stops there: 1195 call resumes in 1200 frames,
  a white screen.
- **Where to put it.** First in its block (at the function's top, or just
  after a `tick!` or `call!`), so that what runs again after a lost frame
  is only the call and what follows it, not stores the block made before
  it. The sync must also give sp (`13 = …`) if the block pushed before
  the call (the readable form writes sp to `Machine` only at the block's
  exit), or the callee's stack lands on the pushed words.
- **The check** (`mods/katam/half-speed/`): Kirby & the Amazing Mirror's
  `GetInput` with a second `VBlankIntrWait` at its top, so the game runs
  at half speed and every other frame end falls inside the added call. A
  state saved there, loaded, and a rewind that stops there each go on and
  count one call resume; see `docs/tools/commands.md`, "A mod copy".

## The naming loop

For games with no decomp, an agent names functions. On Emerald we can
measure how well it does, but only on a sample, capped at **300
functions**, before june looks (unit `mod/naming-gate`).

**The sample's run** (2026-10-27, `docs/research/naming-sample.md`,
`crates/naming`): subtoken F1 0.155 from code alone, 0.222 bottom-up
(0.292 where callees are named), 0.224 with callers; the no-code control
0.076, and 2 of 10 recognition trials named a gen-3 Pokémon game. About
11,500 tokens and 5.5 s a function a round. The FireRed control
(2026-10-28): 0 of 30 FireRed functions with Emerald's code under another
name were answered with Emerald's name; round-0 F1 0.102.

### The sample

- Draw 300 of the 15,144 functions pret names, seeded and recorded. Stratify
  by size: under 16 instructions, 16–100, over 100, 100 functions each.
  Leave out the libgcc and m4a asm (pret names them from asm labels, and a
  matcher would name them in any game).
- Draw whole call-graph subtrees, not only single functions: about 30 roots
  with their callees down to the leaves, then fill to 300 at random. A
  subtree is what lets bottom-up rounds pass names upward, and the report
  must show that gain.

### What the agent sees, per function

One prompt per function. The pieces are built by a Rust tool
(`crates/naming`, new: `naming prepare` and `naming score`), and the
session drives the agents:

- the function's **readable print**, with every pret name hidden. It is
  printed through a map made from the agent's own names so far, so
  unnamed callees show as `f_<key>`, and pool-word data as `addr!(d_<addr>)`
  with our own size estimate (the gap to the next pool target), never
  pret's;
- its **callees** and **callers**: key, and the agent's name if it has
  given one, with one line of what it said each does;
- **strings** it points at, decoded with the game's charset, if a pool
  word lands in text. This is optional for the first sample, and the
  report says whether it was used;
- no pret names, no file, no prototype, no DWARF.

The agent answers in TOML: a name, a one-line summary, and names for the
data it touches.

### Rounds

- **Round 0:** every sampled function with no callee names, so each one is
  named from its code alone.
- **Round 1:** bottom-up over the subtrees, each function seeing its
  callees' round-1 names.
- **Round 2:** once more, now with callers' names too.
- **Vote:** where several functions name the same datum, the most common
  name wins. Data scores use that name.

The score per round is the "how it improves as more callees are named"
curve the unit asks for.

### Scoring against pret

- **Subtoken precision and recall** (and F1) are the main number. Split on
  `_`, case changes and digits, lowercase, and drop `the`/`a`.
  `Task_HandleChooseMonInput` becomes {task, handle, choose, mon, input}.
- **Exact match**, case-insensitive, sits beside it. Pret's own decomps
  share only 28–53% of names, so exact is a floor, not a target.
- **An LLM judge** (same meaning, yes or no) is a third number only if
  cheap, and is reported apart.
- Each is reported per size class and per round, for functions and for
  data. Cost is tokens and seconds per function, from each call's usage.

### Measuring the leak

Any LLM has likely read pokeemerald, so Emerald's score is **an upper
bound for an unnamed game**. Three controls, each on the same sample or a
slice of it:

1. **No code.** Give only "Pokémon Emerald (U), function at 0x0806a519,
   N instructions" and ask for its name. What scores here is memory.
2. **Recognition.** Ask the agent which game the code is from, given three
   functions. If it says Pokémon, the main score is mostly recall.
3. **A sibling.** 30 FireRed functions (pret's pokefirered builds its own
   ROM; `fetch-pret` adds it, pinned) whose Emerald counterpart has a
   different name. If the agent answers with Emerald's name, it is
   remembering, not reading.

The report puts REBENCH's leak-free 0.1–0.4 token F1 beside ours as the
lower reference. Locals are left to a later round: the review shows
they're the hardest part, and DWARF is truth only for the 70% in
registers.

### The cap

The unit stops at 300 functions and three rounds. It doesn't run on all
of Emerald, nor on another game, until june has passed the naming gate.
The gate shows the score, the controls, the cost per function, and 20
functions side by side with pret's names.

## Units

The group as it stands after this design. The first three below exist and
are edited to match; the next three are new.

| Unit | What | Needs |
|---|---|---|
| Source map file, and the printer applying it (`mod/source-map`) | Loader with header, layering and identifier table; `file` from the symtab in pret-names; names for functions and pool-word data via `addr!`; `--names-check`; same states on the new-game route | this |
| Readable print form for chosen functions (`mod/readable-print`) | `--readable LIST`, per-function `Config`, `tick!`/`call!`/`ret!`, the `readable` fn-diff form, the CPU suites and `own-state-check` with every function readable, snapshots | this |
| Agents name a sample of Emerald, scored against pret (`mod/naming-sample`) | `crates/naming`, the 300-function sample, three rounds, three controls | source-map, readable-print |
| DWARF locals, lines and prototypes for Emerald (`mod/dwarf-names`) | `[locals]`, `[lines]`, `proto`/`type` from the ELF's DWARF; naming register locals where liveness agrees; call return values | source-map, readable-print |
| Assembly text for insn! markers (`mod/insn-text`) | `Display` for armv4t's ARM and Thumb instructions in UAL; checked against mGBA's or objdump's text on the SST and jsmolka ROMs | this |
| Cut a mod copy and build with it (`mod/mod-copy`) | `--cut` and `--mod`, the `modded` part, the resume count; unedited cut = same states, one edit shows | source-map, readable-print |
| june sees the naming score (`mod/naming-gate`) | unchanged | naming-sample |
| june makes a small real mod (`mod/first-mod-gate`) | unchanged | mod-copy, dwarf-names, insn-text |

- **DWARF reading** is done in Rust with `gimli`, not through gdb, because
  of the owner's rule that new data logic is Rust. binutils rejects
  agbcc's `.debug_abbrev` ("not zero terminated"). If gimli does too, the
  unit tries a lenient abbrev parse of its own (DWARF 2's abbrev table is
  small) before asking june about a gdb script.
- **The naming sample doesn't wait for DWARF.** Locals are its second
  round, which the cap leaves out, and DWARF names would leak pret's
  answers into its prompts anyway.
- **The first mod waits for everything readable**, so that june judges the
  form as designed, not a half of it.
