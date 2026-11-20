# What the mod view did, and whether the lift does it

The owner, 2026-11-19: the native lift is what the mod view
([mod-view-design.md](mod-view-design.md)) was meant to be. Everything done
for the mod view should be done for the lift, and then the mod view is
retired (`mod/retire`). This table goes through the mod view's design and
the mod group's 50 done units, one row for each thing the mod view prints
or does. Each row says whether the lift has it, which unit will add it, or
that it isn't wanted and why. Unit `native/mod-view-parity`, 2026-11-19 on
quack.

**How it was judged:** by reading lifts made that day, not by unit titles.
The lifts were Metroid (`gbaa-lift` on pret's `mzm_us.gba` with
`mods/metroid/holes.toml`, `--names names --names $GBAA_DATA/names`:
2,623 functions, 307,678 lines) and the C program `c/switch` built by agbcc
(`c-switch-agbcc`, plus `c-fnptr-gcc`). Functions read: Metroid's
`SoftReset`, `SoftResetCheck`, `CallLZ77UncompVram`; c-switch's `AgbMain`,
`pen_update`, `tally`.

## The table

"Has it" means the lift prints or does it today. "Unit" names the open unit
that adds it. The owner agreed the "Not wanted" list (2026-11-19).

### Names

| The mod view | The lift today | Status |
|---|---|---|
| Function names from a source map (pret's symbols, `[functions]`) | `pub fn SoftResetCheck`, `pub fn pen_update`; 2,611 of 2,623 named on Metroid (`codegen::native::named`) | Has it |
| Names by default from every map in `$GBAA_DATA/names` that names the ROM by its sha1 (library maps too) | `gbaa-lift` with no `--names` layers the repo's `names/` over `$GBAA_DATA/names` (`--no-data-names` leaves it out); `just lift` on Metroid names 2,611 of 2,623 functions | Has it (`native/lift-names-default`) |
| Library names for games unlike the reference (BSim-style matching, Opus as judge, other decomps as references) | These units write names files. The lift reads names files as they are | Has it (through the names files) |
| RAM global names (`[data]`) | `g.ram.gMainGameMode`, `g.ram.gTally` (`native::globals`) | Has it |
| A data address the code takes, by name (`addr!(gHeap + 8)`) | `r0 = sColors /* 0x08000638 */;`, `(gHeap + 0x8) /* … */` inside one: every `0x…u32` literal inside a named datum, wherever it prints (`codegen::native::named`), each a `pub const` in `main.rs`; 2026-11-20: 1,934 data at 8,187 constants on Metroid, 9 at 13 on c-switch-agbcc; `data_constants_unnamed` (a sum in a match pattern) barred, 0 on the C programs and Metroid | Has it (`native/lift-data-names-inline`) |
| BIOS calls by name | `LZ77UncompVram(g, …)`, `hw::halt(g)` (the prelude's BIOS calls) | Has it |
| GBA I/O registers by name (`REG_TM1CNT_L`, `codegen::io_regs`) | `st16(g, REG_IME, 0x1u32)`: every constant-address I/O load and store names its register (`io_regs::base`; `(REG_VCOUNT + 0x1)` inside one), each a `pub const REG_*: u32` in `main.rs`; `io_unnamed` (an address no register holds, such as mGBA's debug port) barred, 0 on the C programs and Metroid | Has it (`native/lift-io-names`) |
| Value names (`[values]`, pret's constants where a field takes them) | A constant stored to or compared with a field or global that takes a family, or passed to a parameter of an enum's type, by its name (`g.ram.gMainGameMode = GM_INTRO as u16`, `v33 > PEN_HOLD`, `pen_set(g, r13, PEN_LEFT)`), each a `pub const` in `main.rs`; `[values]` now also has the enums a field or global takes by its C type (mzm's `MAKE_ENUM`s, c/switch's `PenState`). `values_named`: 6 on each c-switch row, 150 on Metroid | Has it (`native/lift-value-names`) |
| Text where the code loads a string (decomp charmaps, community tables, a table from screenshots) | `// text: sFileScreenTextPointers[2] sEnglishTextPointers_FileScreen: ["Start Game\nStart Game (Link)", …]` above the statement that uses a constant pointing at text or a table of it, where the names files give a text table (`native/lift-text`, 2026-11-19: 8 on Metroid); and each such string a static in `src/text.rs`, a literal an edit of which shows on screen (`native/lift-text-editable`, 2026-11-20: 121 on Metroid); no byte preview where none is known | Has it |
| The bytes behind ROM data pointers | A load the value sets bound to ROM indexes a typed static (`ROM_0808CAD0[rix(…)]`, `native/rom-data-statics`); a pointer loaded from ROM is a bare address, read through at run time | Partly: function words as `FnRef`s are `native/rom-statics` |

### Types and paths

| The mod view | The lift today | Status |
|---|---|---|
| A pointer access as a typed path (`Ptr<T>`, field accessors from `[types]`) | `g.get(Ref::<t::Pen>::new(v18).state())`: 77 on c-switch-agbcc, 33 on c-fnptr-gcc (`native::fields`) | Has it |
| A global by its type (`gPen.x`) | A global the names files type as a plain struct (no union or bitfield) is that struct in `Ram` (`gPen: ram_t::Pen`, `#[repr(C)]`), so `g.ram.gPen.color = 0x2a9f`; a word over two whole fields writes each (`gPen.x`, `gPen.y`, c-switch-gcc). A union, a bitfield or an access across a field's edge keeps the access-width type (`[u32; 2]`). Metroid: 3 globals (`gMusicTrackInfo: ram_t::MusicTrackInfo`; most of its structs hold a union or bitfield) | Has it (`native/lift-struct-globals`) |
| A bitfield write as a field set (`set_bits`), constant or computed | Only whole fields are named. No C program has a bitfield | Unit: `native/lift-bitfields` |
| Strength-reduced indexing as `idx(i)` | `.idx(n)` with a constant `n` only (`types::Step::Idx`) | Unit: `native/lift-index-names` |
| Types on Metroid | `just fetch-decomps` builds mzm with agbcc `-g` (`scripts/agbcc-dwarf`; the ROM's sha1 unchanged), so `mzm.toml` has 488 `[types]`, 4,754 located `[locals]` and 62,685 `[lines]`; Metroid's lift names 7,267 field accesses (`Ref::<t::SamusData>::new(r3).pose()`), 2026-11-20 | Has it (`native/metroid-dwarf-map`) |

### Functions and locals

| The mod view | The lift today | Status |
|---|---|---|
| Parameters and return values from prototypes (`r0 = call!(GetMonData2, …)`) | Shapes come from the summaries, which is exact and covers games without prototypes: `fn plot(g, sp, a0, a1, a2) -> Result<u32, Exit>`, `(r0, r1) = SoftResetVBlankCallback(g, sp)?` | Has it (counts and returns). Names: next row |
| Parameter names, and DWARF local names where liveness agrees | Register locals and parameters by name where liveness agrees (`pen_update`'s `r4` is `p`, `tally(g, r13, mut n: u32)`), else `// dwarf r0: row` where the range starts; `locals_named` 13 and `locals_noted` 4 on c-switch-agbcc. Block-local values stay `vN`, stack variables `lN` | Has it for registers (`native/lift-local-names`); values and stack slots not named |
| Locals as variables, values not re-synced to registers | `let vN` values inlined into their use; registers are still `r0`-`r14` | Unit: `native/lift-register-locals` (in progress) |
| `file:line` comments from DWARF's line table | `// c/switch/main.c:218` above the statement each source line's first instruction starts, where the names files have `[lines]` (`line_notes`: 126 on c-switch-agbcc, 105 to 181 on the C programs; 61,260 on Metroid) | Has it (`native/lift-line-comments`, 2026-11-20) |
| Literal-pool loads folded into the access | Constants inline: `st16(g, REG_DISPSTAT, 0x8u32)`, `g.ram.gTally` | Has it |
| Loops with their variables, `for` loops | `'l2: loop`; 95 `for` and 601 `loop` on Metroid (`native/lift-for-loops`) | Has it |
| Switch shapes | A compare tree as a `match` on case values (`tally`: `5 => …`, `0x28 => …`). A jump table as a `match` on the target address (`pen_update`). The mod view never wrote jump tables (its design's gap 9) | Has it (more than the mod view had) |
| Callee-saved registers proven per callee, reloads dropped | The summaries' callee-saves proof; 584 of 2,623 unproven on Metroid (soft) | Has it |
| No timing in the printed code (the "without timing" builds) | The lift has no cycle counting at all | Has it |

### Mods

| The mod view | The lift today | Status |
|---|---|---|
| Cut chosen functions into `mods/<game>/<mod>/` with a pristine `orig/`, re-cut by three-way merge (`--cut`, `--mod`) | `gbaa-lift --cut DIR KEY...` (names allowed) writes `src/<name>.rs`, `orig/` and `replace.toml`; `--replace DIR/replace.toml` builds with it; a re-cut merges (`git merge-file`). On c-switch-agbcc an unedited cut of `tally` and `pen_update` passes the native check, a one-constant edit fails it at frame 31, and a re-cut from an older print merges with no conflict | Has it (`native/lift-mod-cut`, 2026-11-20) |
| A call a mod adds needs no ROM address | A replacement calls any lifted function by its Rust name | Has it |
| One command builds a mod and shows its frames beside stock | `just lift-mod-run ROM DIR ROUTE` builds the lift stock and with the cut at DIR, lists the frames that differ (`31-600`), writes side-by-side PNGs of up to 16, and fn-checks each cut against its lift. On c-switch-agbcc a one-constant edit of `pen_update` differs at 570 of 600 frames and fails its fn-check; an unedited cut lists none and passes | Has it (`native/lift-mod-run`, 2026-11-20) |
| A cut file says what its macros mean to an editor | The lift has no `at`, `ret!` or `regs!` to explain. A cut's header says what must stay as cut (each `pub fn` line's signature, `Result` or not) and what `g`, `r13`, `lN`, `vN`, `RET`, `pc(..)` and `bank_sp` are | Has it (`native/lift-mod-cut`) |
| An agent makes a small real mod and says what hurt | Not tried on a lift | Unit: `native/lift-agent-mod` |
| Proof the named build is the unnamed one (fn-diff, same states after names) | The native check (`route-check --native`), the per-function check (`just fn-check`), `lift-check`'s planted faults | Has it |

### Not wanted

The owner agreed to leave all seven out (2026-11-19).

| The mod view | Why not for the lift |
|---|---|
| `insn!` markers, and assembly as text or comments (`--asm-text`, `--asm-comments`) | The lift is meant to read as a program, not a CPU. Each block's guest pc stays (`runtime::guest_pc::set`, only in check builds once `native/lift-one-entry` lands), and `just dis` gives the assembly at that pc |
| A readable form per function (`--readable LIST`) beside the play view | The lift is one form for every function |
| Timing macros (`tick!`, `call!` writing lr, `ret!`, `regs!`) | The lift has no cycle counting or guest call frames for them to stand for |
| Resumes inside a modded function (run in the interpreter, counted) | A lifted program never resumes at a guest pc: no save states mid-function, no `MAX_DEPTH` unwind |
| The naming loop for games with no decomp (agents name a sample, scored against pret, leak controls) | The owner parked its gate ("june sees the naming score before it runs at scale", 2026-11: naming isn't ready). The loop writes a names file, and the lift reads names files as they are. Port it if she unparks it: the agent would see the lift's print of each function instead of the readable one |
| `just mod-find` (the functions that read or write a struct field) | The lift prints typed accesses as text, so `grep 'Ref::<t::Pen>::new(.*)\.x()'` over `out/<game>/src` lists them. A tool isn't needed until a game's lift is too big to grep |
| Snapshots of the readable print (`just snapshots`, `just snapshots-readable`) | They held a printer change against fixed Emerald prints in the private repo. The lift's equivalents are the C programs' lifts (public, rebuilt each check), their bars in `lift-check.toml`, and the generated numbers file |
