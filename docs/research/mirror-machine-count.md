# What still reads as a machine in Mirror's lift, counted

Unit `native/mirror-machine-count`, 2026-11-20 on quack. The owner
(2026-11-19) wants netcode, menus and custom graphics written straight
into Kirby & the Amazing Mirror's lift as ordinary Rust, and was dismayed
that the guest stack and guest-address pointers are still there. This
counts, over the whole lift, each thing that reads as a machine and not a
program, ranks them, quotes a few lines of each, and names the unit that
removes it. Whole functions stay in `mods/kirby-mirror/`; the quotes here
are a line or two.

## How it was counted

- **The lift.** main at 75394d98, `gbaa-lift` on the Amazing Mirror with
  `mods/kirby-mirror/holes.toml` (less its two `[[open_bus]]` entries,
  which this lifter doesn't know yet: another lane's work) and the default
  names (katam's map from `$GBAA_DATA/names`, the repo's
  `names/kirby-amazing-mirror.toml` over it), into
  `out/lift-mirror-count/` (gitignored). The holes file's `skip_fn_flow`
  is set, so the function flow didn't run (see call_fn below). 5,661
  functions, 0 hard holes; the function files (`src/m_*.rs`) are 724,957
  lines.
- **The 30 functions read**, the ones a netcode or menu mod would touch
  (memory/kirby-amazing-mirror.md, `docs/research/kirby-controller.md`):
  the Kirby controller (`KirbyTask`, `KirbyReadInput`,
  `KirbyPreStateChecks`, `KirbyStand`, `KirbyWalk`, `KirbyEnterJump`,
  `KirbyJumpRise`, `KirbyGroundAccel`, `CreateKirby`); the task system
  (`AgbMain`, `TasksInit`, `TaskCreate`, `TaskDestroy`, `TasksExec`,
  `TaskGetNextSlot`, `TasksDestroyInPriorityRange`); input and menus
  (`GetInput`, `TitleScreenMain`, `sub_08138B44` the file select,
  `sub_08138D64` the main menu back from a sub-game, `sub_08141E74` the
  main menu's task, `PauseMenuMain`); the link code (`MultiSioInit`,
  `MultiSioMain`, `MultiSioSendDataSet`, `MultiSioRecvDataCheck`,
  `MultiSioStart`, `MultiSioStop`, `sub_08032E98` the link screen,
  `sub_080331E0` its task). Each with its `x_`/`a_` entries: 7,640 lines.
- **The counts** are `rg -o PATTERN out/lift-mirror-count/src -g 'm_*.rs'
  | wc -l` (occurrences, not lines; the prelude in `main.rs` left out),
  and the same over the 30 functions. The patterns are in the table. A
  bare address is a hex literal in 0x02000000-0x08ffffff on a line that
  isn't a comment or a `pc(…)`, not one the lift already names
  (`gUnk_03000000 /* 0x03000000 */`, 6,641 of those) and not a return
  address put in r14.
- The lift's own `metrics.toml` counts some of these the lifter's way;
  where one exists it is named, and is the better bar for a unit.

## Ranked

| # | What reads as a machine | Pattern | Whole lift | 30 fns | Open unit that removes it |
|---|---|---|---:|---:|---|
| 1 | Temporaries named `vN` | `\bv[0-9]+\b` | 606,337 | 6,163 | `native/lift-let-names` (2026-11-24): a `let` value takes the name of the global or field it loads, the local or DWARF variable it copies, or what it feeds; 305,363 left (524,112 before it on that day's printer), `lets_named` 76,366 |
| 2 | Register names `r0`-`r14` | `\br([0-9]\|1[0-4])\b` | 359,948 | 3,377 | Mirror's names map has DWARF types, locals and lines (`native/mirror-dwarf-map`, ready): katam's locals name what's left |
| 3 | A u32 as an address: typed pointer built from a bare u32 | `Ref::<…>::new(` | 62,289 | 572 | Mirror's DWARF map (types of parameters and locals), then stage B's typed pointers; none filed for the u32 itself |
| 4 | Block markers `pc(0x…)` | `\bpc\(0x` | 58,170 | 714 | done (`native/lift-pc-markers`, 2026-11-24): 0, kept in the crate's `pcs.txt` and put back by its build script in a `guest-pc` build |
| 5 | Load rotates for a misaligned address | `ShiftType::Ror` | 44,881 | 432 | `native/lift-aligned-loads` (2026-11-24): 12,090 left, raw `ld32`/`ld16` through untyped pointers; `load_rotates_left` 13,040; `native/lift-aligned-params` (2026-11-27): 10,814 (11,291 on main before it) |
| 6 | Adds of zero (Thumb's `adds rd, rs, #0` as a move) | `wrapping_add\(0x0\)` | 41,107 | 367 | done (`native/lift-add-zero`, 2026-11-24): 0 |
| 7 | A u32 as an address: byte loads and stores through a computed value | `\b(ld\|st)(8\|16\|32)\(g,` | 40,309 | 238 | Mirror's DWARF map and `native/lift-index-names` (merging); `load_computed_unknown` 48,412, `store_computed_unknown` 33,281 |
| 8 | Casts | ` as (u8\|u16\|u32\|i8\|i16\|i32\|u64\|i64)\b` | 39,068 | 368 | `native/lift-cast-types` (2026-11-24): 35,531, a value printed at a width compared, narrowed and shifted in it; `casts` (barred on the lift-check programs). `native/lift-signed-values` (2026-11-27): `casts` counts `prim::sext` too (5,849, so 41,564 on the same lift), and 657 lets compared signed are declared `i32`: 40,190 (`lets_signed`), the signed compares of a name 7,998 to 6,781. Left: lets whose cast only moves to their load (`native/lift-signed-loads`), register locals (`native/lift-signed-registers`); `narrowing_casts` 7,264 |
| 9 | sp passed to a callee | `\(g, (r13\|sp)[,)]` | 29,362 | 311 | Exact sp passed only where a stack address reaches RAM (`native/lift-frame-exact-narrow`, ready); stage A keeps sp a parameter by decision |
| 10 | `bank_sp` before a store | `bank_sp\(` | 14,068 | 127 | `native/lift-frame-exact-narrow`; `bank_sp_calls` 14,068 |
| 11 | Bare addresses | see above | 18,562 (RAM 7,545, ROM 11,017) | 198 (129, 69) | RAM: Mirror's DWARF map, `native/lift-global-bitfields` (ready); ROM: function words are `native/fnref-dense` and `native/rom-statics` (both waiting) |
| 12 | Stack addresses `r13.wrapping_add(…)` | `r13\.wrapping_add\(` | 10,534 | 51 | `native/lift-frame-exact-narrow`; `sp_accesses_escaping_slot` 7,218 |
| 13 | Frame locals named `lN` | `\bl[0-9]+\b` | 10,556 | 187 | Mirror's DWARF map |
| 14 | `Result` and `?` where nothing can exit | `Result<`, `\)\?` | 9,231 and 10,256 | 55 and 149 | filed: `native/mirror-flow-on` |
| 15 | sp arithmetic | `r13 = r13\.wrapping_` | 9,039 | 33 | `native/lift-frame-exact-narrow` |
| 16 | sp as a parameter | `(sp\|r13): u32` | 8,511 | 37 | as 9 |
| 17 | Labelled blocks | `'b[0-9]+: \{` | 7,196 | 115 | none (past this unit's six); `labelled_blocks` 7,196 |
| 18 | `pin(…)`, a pointer checked into one global | `\bpin\(` | 2,030 | 69 | `native/lift-index-names`, `native/lift-global-bitfields` |
| 19 | Four-register entries through `Regs::call` | `Regs::call` | 2,191 | 14 | `native/mirror-flow-on` (complete callee sets), `native/fnref-live-registers` (parked) |
| 20 | Return addresses put in r14 | `r14 = 0x` | 1,131 | 10 | none |
| 21 | `call_fn(g, k, r13, [r0, r1, r2, r3])?` | `call_fn\(` | 396 | 17 | filed: `native/mirror-flow-on`; then `native/fnref-dense` |
| 22 | Raw flags `nf` `zf` `cf` `vf` | `\b[nzcv]f\b` | 517 | 0 | none needed in the 30 |

## Each, quoted

**1-2, 13. Names.** The lift names what katam's prototypes and DWARF-less
headers give (`task`, `structSize`, `kirby`, `sendp`), and nothing else:

    let v74: u32 = g.ram.gNumTasks;
    let v92: bool = (v74 as i32) > 127;
    let (mut r0, mut r1, mut r2, mut r3, mut task, mut structSize, mut r6, …) = (s.r[0], …);

A single-use value is already printed inside its use (`native/lift-inline-temps`); what is left
is read twice or across a store. katam builds with agbcc, which `native/mirror-dwarf-map` rebuilds
with debug info for locals; the `vN` lets need a name of their own (the field they load, the
local they feed).

**3, 7, 11, 18. A u32 used as an address.** Field accesses are typed now, but the pointer is a
`u32` built where it is used, and anything without a type is a byte load at a computed address:

    bank_sp(g, r13); g.set(Ref::<t::Kirby>::new(kirby).base().unkC(), (r0 | r1));
    let v56: u32 = ld8(g, r10.wrapping_add(v52));
    let v70: u32 = v63.wrapping_add(0x2023578);
    g.set(Ref::<t::Cutscene>::new(v5).unk0(), 0x8029ef5u32);

The last is a state function stored as its ROM key (Stage-A function pointers are ROM keys, by
decision): `native/fnref-dense` is what turns those into `FnRef`s. ROM table bases
(`.wrapping_add(0x82d7850)`) are the rest of the ROM-range literals.

**4. `pc(…)`.** One per block, so a stop can name where it was; empty in a plain build but in the
text all the same: 58,170 lines, one in every 12.

    pc(0x08152b31);

Since `native/lift-pc-markers` src/ has none: the crate's `pcs.txt` keeps
where each went, and its `build.rs` puts them back in a `guest-pc` build's
copy of each module.

**5. Rotates.** ARM's `ldr` from a misaligned address rotates, so every load the lifter can't
prove aligned keeps the rotate, even through a typed field of a struct it knows is aligned:

    let v4: u32 = g.get(Ref::<t::Kirby>::new(v2).base().flags());
    let v11: u32 = prim::shift(ShiftType::Ror, v4, (v2.wrapping_add(0x8u32) & 0x3u32) << 3, false).0 & r4;
    let v108: u32 = (v74 << 2).wrapping_add(0x3002560);   // then Ror by (v108 & 3) << 3

`v108` is a multiple of 4 plus an aligned base: the rotate is by 0.
Since `native/lift-aligned-loads` (2026-11-24) a typed field's load, a heap
object's, a stack local's and one at an address proven aligned (constants,
sp and the arithmetic between) print without it: 44,881 to 12,090 on the
Amazing Mirror. What's left reads a pointer parameter or a loaded pointer
plus an offset with no type, aligned only as its callers make it.
Since `native/lift-aligned-params` (2026-11-27) a register's low bits known
0 carry across a function's blocks and, into a function only direct calls
of its own code context enter, from all its callers' arguments:
`load_rotates_left` 11,291 to 10,814 (10,482 to 10,077 `ShiftType::Ror`),
1,341 functions entered knowing some. What's left is mostly loaded
pointers and the parameters of functions a pointer reaches (the objects'
callbacks), which no caller's fact may cover.

**6. Adds of zero.** agbcc moves registers with `adds rd, rs, #0`:

    r0 = v2.wrapping_add(0x0);
    task = prim::shift(…).0.wrapping_add(0x0);

Since `native/lift-add-zero` (2026-11-24) the sum prints as the value it
copies (`r0 = v2;`; `narrow::Form::Copy`), whatever reads its flags: 0 in
the whole lift (main at 7995104 with the change, flow on).

**8. Casts.** Halfword and byte arguments narrowed on entry, signed compares as `i32`:

    structSize = r1 as u16 as u32;
    let v34: bool = ((v8 << 16) as i32) >= 0;

Since `native/lift-cast-types` a value the lift printed at a width compares, narrows and shifts in
it (`codegen::native::casts`): `((v13 as i16 as u32) as i32) <= 3` is `(v13 as i16) <= 3`,
`(v2.wrapping_sub(0xa1) as u16 as u32) > 2` is `(v2.wrapping_sub(0xa1) as u16) > 2`, and a signed
compare's constant is an `i32` literal (`-0xe00000`, not `(0xff200000u32 as i32)`). What is left is
mostly `u32` values compared signed, which a local typed `i32` would print bare only if its other
uses (wrapping arithmetic, stores, calls) were `i32` too.

Since `native/lift-signed-values` a `let` is declared `i32` where that saves casts over its
function (`codegen::native::signed`, a pass over the printed text): `let v1: i32 = g.ram.x as i16
as i32;` then `v1 < 0`, `v1.wrapping_sub(1) < 0`, `v1 == -1`, a narrowing store `v1 as u16` as it
was, any other use `v1 as u32`; the right side `((x as i32) >> 16) as u32` is `(x as i32) >> 16`.
Lets copied into one another go together (a minimum cut over the function). On the Amazing Mirror
(lift at 31b453ec's main with the change, holes applied, flow on) 657 lets: `casts` 41,564 (with
`prim::sext` now counted) to 40,190; its native route passes 3,000 frames. What holds the rest,
measured over the 5,513 lets still compared through a cast (5,641 compares): 5,192 have a right
side that is a plain `u32`, mostly a typed 32-bit field's `g.get(…)` or a `u32` global field, so
declaring them `i32` moves the cast to the load (3,371 have no other use: a tie), and the uses
besides compares are copies into register locals (1,307), call arguments (492) and address
arithmetic (about 270). Register locals (`(rN as i32)` 1,132, and the C programs' loop counters)
are untouched: the pass types only lets. A let also shifted right or compared unsigned (`w >> 3`,
`w < v2`) stays `u32` whatever it would save; on the Amazing Mirror none of the 657 was (the same
40,190 `casts` with that rule, and no `(w as u32) >>` or unsigned compare of a cast in its text).

**9-10, 12, 15-16. The guest stack.** Every call passes sp; every function that keeps a frame moves
it; a store first tells the IRQ path where sp is:

    r13 = r13.wrapping_add(0xffffffecu32).wrapping_add(0xfffffffcu32);
    (r0, r3) = IwramMalloc(g, r13, v13, r3);
    let v11: u32 = ld32(g, v10 & !3);       // v10 = r13 + 0x18: an argument on the stack

Stage A keeps the guest stack as IWRAM bytes, sp a parameter, by decision; the open unit narrows
where it is passed exactly.

**14, 19, 21. Result, four-register entries, call_fn.** `Exit` is born in two places: the one
function that soft-resets (in the multiboot code) and `call_fn`, a call through a function
pointer, which may reach any function. With `skip_fn_flow` in Mirror's holes file (the flow took
37 min on 2026-11-14) no indirect call has a callee set (`call_fn_complete` 0 of 376), so every
caller of a function pointer, and every caller of those, returns `Result`. Since `native/mirror-flow-on`
(2026-11-24, the flow on, 6 min for the whole lift, build and run) 33 of the 376 have a
complete callee set and `Result<` falls to 7,979 (`)?` 9,987); `Regs::call` is 2,195
and the `call_fn(` sites still 396. The Kirby state
machine is exactly that:

    [r0, r1, r2, r3] = call_fn(g, if r4 & 1 != 0 { r4 } else { r4 & !3 }, r13, [r0, r1, r2, r3])?;
    pub fn MultiSioSendDataSet(g: &mut Game, mut r13: u32, mut sendp: u32, mut loadRequest: u32) -> Result<u32, Exit> {
    pub fn KirbyTask(g: &mut Game, sp: u32, a: [u32; 4]) -> Result<[u32; 4], Exit> { x_0803ee19(g, Regs::call(sp, a)) }

The lift's `functions_result_unneeded` is 0 (`native::no_result`), but on the lifter's terms: with
no callee sets, a pointer call may exit. Nightmare's lift has the flow back on
(`native/nightmare-flow-on`); `memory/lift-cost-corpus.md` has Mirror's holes-only lift with
`--flow` at 1,054 s on quack.

**17. Labelled blocks.** Branches out of the middle of structured code:

    'b13: {
    'b12: {
        …
                    break 'b13;

**20. Return addresses.** `r14 = 0x8152cc1u32;` before a call whose callee may read lr.

## Filed

Six units, each with its whole-lift count as the bar to lower (the counts
above are the baseline; a unit re-measures at its start, as stage B's
units do):

- `native/mirror-flow-on`: Mirror's lift with the function flow back on;
  bars `call_fn_complete` 0 of 376 and 9,231 `Result<`.
- `native/lift-aligned-loads`: a load proven aligned drops its rotate; bar
  44,881 `ShiftType::Ror`.
- `native/lift-add-zero`: an add of zero prints as the value; bar 41,107
  (done 2026-11-24: 0).
- `native/lift-pc-markers`: block markers out of the text a modder reads;
  bar 58,170 (done 2026-11-24: 0; `pcs.txt` holds the 58,170).
- `native/lift-cast-types`: values typed by width so the casts go; bar
  39,068 (`narrowing_casts` 7,265); 35,531 after it (row 8).
- `native/lift-signed-values`: lets compared signed declared `i32`; 40,190
  of 41,564 once `prim::sext` counts (row 8). Its two follow-ups:
  `native/lift-signed-loads` (a signed C field's load gives `i32`) and
  `native/lift-signed-registers` (register locals too).
- `native/lift-let-names`: a value read twice is named for what it
  holds; bar 606,337 `vN` (`value_lets` 208,204). (Done 2026-11-24:
  305,363, from 524,112 with `--no-let-names`; the rest are compares,
  sums and loads with nothing named to go by.)

Labelled blocks (7,196) and r14 return addresses (1,131) have no unit:
smaller, and past the six.
