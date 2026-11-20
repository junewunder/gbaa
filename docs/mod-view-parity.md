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
| Names by default from every map in `$GBAA_DATA/names` that names the ROM by its sha1 (library maps too) | `gbaa-lift` reads only the repo's `names/` unless `--names` is given; `just lift` on Metroid names nothing | Unit: `native/lift-names-default` |
| Library names for games unlike the reference (BSim-style matching, Opus as judge, other decomps as references) | These units write names files. The lift reads names files as they are | Has it (through the names files) |
| RAM global names (`[data]`) | `g.ram.gMainGameMode`, `g.ram.gTally` (`native::globals`) | Has it |
| A data address the code takes, by name (`addr!(gHeap + 8)`) | `r0 = 0x8000638u32` is `sColors`. The report says "0 data named, at 0 constants": `named.rs` only matches `let vN: u32 = 0x…;`, and inlined constants stopped printing that form | Unit: `native/lift-data-names-inline` (a regression) |
| BIOS calls by name | `LZ77UncompVram(g, …)`, `hw::halt(g)` (the prelude's BIOS calls) | Has it |
| GBA I/O registers by name (`REG_TM1CNT_L`, `codegen::io_regs`) | `st16(g, 0x4000208u32 & !1, 0x1u32)` | Unit: `native/lift-io-names` |
| Value names (`[values]`, pret's constants where a field takes them) | Numbers (`gMainGameMode = 0x1u32 as u16`) | Unit: `native/lift-value-names` |
| Text where the code loads a string (decomp charmaps, community tables, a table from screenshots) | The bare address | Unit: `native/lift-text` |
| The bytes behind ROM data pointers | Bare addresses, read at run time | Unit: `native/rom-statics` (filed; ROM objects become typed statics) |

### Types and paths

| The mod view | The lift today | Status |
|---|---|---|
| A pointer access as a typed path (`Ptr<T>`, field accessors from `[types]`) | `g.get(Ref::<t::Pen>::new(v18).state())`: 77 on c-switch-agbcc, 33 on c-fnptr-gcc (`native::fields`) | Has it |
| A global by its type (`gPen.x`) | `Ram` types a global by its access widths: `gPen: [u16; 4]`, `g.ram.gPen[3] = 0x2a9f` | Unit: `native/lift-struct-globals` |
| A bitfield write as a field set (`set_bits`), constant or computed | Only whole fields are named. No C program has a bitfield | Unit: `native/lift-bitfields` |
| Strength-reduced indexing as `idx(i)` | `.idx(n)` with a constant `n` only (`types::Step::Idx`) | Unit: `native/lift-index-names` |
| Types on Metroid | `mzm.toml` has no `[types]`, `[locals]` or `[lines]` (the ELF has no DWARF), so Metroid's lift has 0 named fields | Unit: `native/metroid-dwarf-map` |

### Functions and locals

| The mod view | The lift today | Status |
|---|---|---|
| Parameters and return values from prototypes (`r0 = call!(GetMonData2, …)`) | Shapes come from the summaries, which is exact and covers games without prototypes: `fn plot(g, sp, a0, a1, a2) -> Result<u32, Exit>`, `(r0, r1) = SoftResetVBlankCallback(g, sp)?` | Has it (counts and returns). Names: next row |
| Parameter names, and DWARF local names where liveness agrees | `a0`, `r4`, `v18` (`pen_update`'s `r4` is `p`) | Unit: `native/lift-local-names` (needs `native/lift-register-locals`, in progress) |
| Locals as variables, values not re-synced to registers | `let vN` values inlined into their use; registers are still `r0`-`r14` | Unit: `native/lift-register-locals` (in progress) |
| `file:line` comments from DWARF's line table | None | Unit: `native/lift-line-comments` |
| Literal-pool loads folded into the access | Constants inline: `st16(g, 0x4000004u32 & !1, 0x8u32)`, `g.ram.gTally` | Has it |
| Loops with their variables, `for` loops | `'l2: loop`; 95 `for` and 601 `loop` on Metroid (`native/lift-for-loops`) | Has it |
| Switch shapes | A compare tree as a `match` on case values (`tally`: `5 => …`, `0x28 => …`). A jump table as a `match` on the target address (`pen_update`). The mod view never wrote jump tables (its design's gap 9) | Has it (more than the mod view had) |
| Callee-saved registers proven per callee, reloads dropped | The summaries' callee-saves proof; 584 of 2,623 unproven on Metroid (soft) | Has it |
| No timing in the printed code (the "without timing" builds) | The lift has no cycle counting at all | Has it |

### Mods

| The mod view | The lift today | Status |
|---|---|---|
| Cut chosen functions into `mods/<game>/<mod>/` with a pristine `orig/`, re-cut by three-way merge (`--cut`, `--mod`) | Stand-ins (`[[standin]]` in a holes file, `--replace FILE`), checked by `just fn-check`. No cut: an editor copies a function out of `out/` by hand | Unit: `native/lift-mod-cut` |
| A call a mod adds needs no ROM address | A replacement calls any lifted function by its Rust name | Has it |
| One command builds a mod and shows its frames beside stock | None | Unit: `native/lift-mod-run` |
| A cut file says what its macros mean to an editor | The lift has no `at`, `ret!` or `regs!` to explain. The cut's header covers what is left | In `native/lift-mod-cut` |
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
