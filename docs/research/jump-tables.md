# Jump-table recovery

Research for issue #5 (decision 5c in [decisions.md](../decisions.md)): how
do we recover jump tables, the indexed branches a `switch` compiles to? Two
approaches were built and run on every ROM on quack: per-compiler pattern
matchers, and a value-set analysis that knows no compiler. Researched
2026-10-19 on quack.

## Summary

- **Recommendation: value-set analysis (V), no per-compiler matchers.**
  Accuracy barely separates the two: they tie on agbcc, and V is ahead by
  3 tables out of 617 on modern GCC. Cost separates them. Both approaches
  need V's machinery anyway, for the far-jump `bl` rule below. V needs no
  per-compiler code, and it is `Sem<V = AbsVal>` from decision 3, which
  becomes required rather than "later, optionally". Adding the matchers on
  top as a fallback would buy 17 more modern-GCC tables (2.8%); that is the
  option not taken (see "Recommendation").
- **Checked against ground truth** (ELF symbols plus the ROM bytes), tables
  resolved exactly:
  - pokeemerald (agbcc), 676 tables: gbadisasm's matcher 675, our matchers
    675, V 675. All three miss the same one.
  - pokeemerald built here with GCC 15.2 (`make modern`), 617 tables:
    gbadisasm's matcher 0, our matchers 580, V-alone 583, either of the
    two 600.
  - Neither approach produced a wrong target on either build.
- **On ROMs without symbols** the counts are only internally consistent.
  V resolves more on every non-agbcc ROM: Emerald Rogue 449 vs 433,
  Imperium 497 vs 483, moonbeam 30 vs 28, WarioWare 207 vs 204. Wherever
  both resolve a branch they give the same targets, on every ROM.
- **No ROM on quack has an LLVM jump table.** an agb ROM (Rust/LLVM) has
  no indexed branch at all. A rustc-compiled sample shows LLVM's two forms,
  and both approaches resolve them.
- **Both approaches depend on knowing that agbcc's and GCC's `bl` is
  sometimes a far jump.** It is the default branch of most big switches.
  #6 proposed a rule based on function extents. A semantic rule measured
  better: probe the target, and if every return reached pops the caller's
  frame, the `bl` is a jump. It finds 547 of 548 far-jump `bl`s on Emerald
  and 721 of 732 on the modern build, with no false ones. This changes what
  decision 5a should say (see "Effects on decisions").
- **Cost.** The V prototype is ~730 non-blank lines of Rust. In gbaa about
  250 of them would come from `Sem`, so the estimated cost is the abstract
  domain, a per-function fixpoint driver and the far-jump probe: ~500
  lines (an estimate, not measured). It analyses a whole 32 MB ROM in
  2–4 s, plus 12–51 s for the far-jump rounds (unoptimised). The matchers are ~280 lines for six idioms; every new
  compiler or idiom means more.
- **Review changed the numbers.** The first draft overstated V. Its
  prototype kept results from early visits that the final, joined state
  couldn't support, widened right after bounds checks, and re-probed far
  jumps after their rounds had finished. All three are fixed, and every
  number here is from the fixed tool.

## ROMs and their compilers

Every ROM in `~/ROMs/gba/`, plus two local builds. "Idiom counts" come from
`idioms.py` (see "Reproducing"): a byte scan for each compiler's switch
dispatch, so data can add a few stray hits.

| ROM | Compiler | Evidence |
|---|---|---|
| Pokemon - Emerald Version | agbcc (GCC 2.95 fork) | SHA1 matches pret's pokeemerald, which builds with agbcc (#6); idiom counts: agbcc 679, gcc-O2 1 |
| pokeemerald_modern (local build) | arm-none-eabi-gcc 15.2.1, `-O2` | built here with `make modern` from pokeemerald `731ad5bf`; agbcc 5, gcc-O2 607 |
| Super Mariomon | agbcc | pokeemerald-based (header code `BPEE`, strings `gflib/window.c`, `src/mystery_gift.c`); agbcc 777, gcc-O2 0 |
| Emerald Rogue | modern GCC | pokeemerald-based (header `POKEMON EMER`/`BPEE`); agbcc 2, gcc-O2 659 |
| Emerald Imperium | modern GCC | as Rogue; agbcc 6, gcc-O2 719 |
| WarioWare, Inc.: Mega Microgame$! | GCC 2.9x family (Nintendo's SDK compiler, which agbcc forks) | first-party, m4a song names (`m_BGM_*_AGB`); agbcc 208, gcc-O2 0. Its idioms are agbcc's, so the two can't be told apart from code. |
| moonbeam | modern GCC (devkitARM era) | strings `newlib-3.3.0/newlib/libc/stdlib/dtoa.c` and Duktape's `arm32 unknown gcc`; gcc-O2 26; libgcc's `__gnu_thumb1_case_uqi`/`_uhi` bodies at `0x08042e00`/`0x08042e14` |
| llvm.gba (local sample) | rustc nightly-2026-02-14 (LLVM) | two `match` functions built for `thumbv4t-none-eabi` and `armv4t-none-eabi` |

## Definitions

**Code map.** With `--elf`, code is exactly the ELF's `$t`/`$a` mapping-symbol
regions, decoded linearly. Functions are the ELF's `FUNC` symbols, as in #6's
truth file. Without symbols, code is what recursive descent reaches. It
starts from the entry (ARM `0x08000000`) and from a pointer scan, as in #6:
word-aligned ROM words pointing into ROM, kept if the target is Thumb
`push {…, lr}` (odd pointer) or ARM `stmdb sp!, {…, lr}` (4-aligned even
pointer). Descent follows `b`/`b<cond>`/`bl` and every resolved table, from
either approach.

**Candidate** (decoder level, over our `armv4t::decode` enum): an
instruction that writes PC and is neither a branch, a return nor a call.

- A **return** is one of: `bx lr`, `mov pc, lr`, any `ldm`/`pop` loading pc,
  `ldr pc, [sp], #4`, or `bx rN` right after a `pop` that loads rN (the
  agbcc/LLVM interworking return `pop {r0}; bx r0`).
- A **call** is `bx`/`mov pc` right after `mov lr, pc`.
- A `bl` also counts as a candidate when its target is a short leaf (no
  branches, up to its first return) that writes lr with an ALU op or `ldr`
  and returns with `bx lr`/`mov pc, lr`. Such a helper returns to a computed
  address. This is how `__gnu_thumb1_case_*` works.

**Indexed branch** (the counts below are of these): a candidate whose target
comes from a table indexed by a register. Walk back from the jump through at
most 8 straight-line instructions, stopping at any PC write, following the
registers the target is computed from. The branch is indexed if one of these
holds:

- the target comes from a load whose address is the sum of two registers: a
  register-offset load, or a base built by a register-register `add`, pc
  included;
- the jump itself is `add pc, rM` or a register-offset `ldr pc`;
- it is a `bl` candidate (computed-return helper).

Per ROM this matches:

- agbcc, WarioWare: Thumb `mov pc, rT` after `ldr rT, [rA, #0]` with
  `add rA, rB, rI`.
- Modern GCC: Thumb `mov pc, rT` after `ldr rT, [rB, rI]`, and
  `bl __gnu_thumb1_case_*`.
- LLVM: Thumb `add pc, rX` and ARM `ldr pc, [rB, rI, lsl #2]`.
- pret ELF code maps: ARM `bx` through a RAM table (the IRQ dispatcher,
  `IntrMain`). Descent never reaches it, and the agb ROM has none.

**Resolved.** An approach resolves an indexed branch if it gives a finite
target list, every target in ROM. The ground truth for "resolved correctly"
comes from a pret ELF:

1. A table is a maximal run of words, all inside one function, where each
   word is the address of a `$t`/`$a` instruction in that same function.
   Inline tables lie in the function's `$d` regions. Out-of-line tables lie
   in a ROM data section, and there every entry must be mid-function.
2. A run is split wherever a data word of the function points into its
   middle. Each table's start must be held in that function's literal pool.
3. A result matches the truth table whose address range contains the
   result's table address:
   - **exact** if the target sets are equal;
   - **subset** if every target is right but some cases are left out;
   - **wrong** if it has a target the table lacks.

Emerald has 676 truth tables (10,081 entries, 5 to 353 entries each) and
the modern build has 617, all held in a literal pool. Byte-sized LLVM or
`__gnu_thumb1_case` tables aren't covered by this rule. No ELF build here
has any.

**Checked vs only internally consistent.** Counts for Emerald and the
modern build are checked against that truth. Counts for Super Mariomon,
WarioWare, Rogue, Imperium and moonbeam aren't: no symbols
exist. For those ROMs the evidence is:

- P and V agree on every branch both resolve;
- every V target of an indexed branch lies within 20 KB of its jump
  (`0x4fd2` at most, on Super Mariomon);
- the disagreements were inspected by hand (see "Where they disagree").

The LLVM sample was checked by hand: its targets against the rustc
assembly listing (`llvm-sample/swt.s`) and `objdump` of the linked image.

## The patterns each compiler emits

Addresses are in the ROM named. Each sequence is a few instruction words.

**agbcc / GCC 2.9x.** Emerald `GetBgControlAttribute`, `0x080014d2`:

```
subs r0, r5, #1 ; cmp r0, #7 ; bhi default
lsls r0, r0, #2 ; ldr r1, =0x080014ec ; adds r0, r0, r1 ; ldr r0, [r0, #0] ; mov pc, r0
```

The table follows inline, after one pool word. When the default is far
away, the bounds check becomes `bls ok; bl default`, a `bl` used as a jump.
Example: `DoTradeAnim_Cable` at `0x0807bbf8`, `ldr r1, =0x10b ; cmp r5, r1 ;
bls ; bl 0x0807cfb4`. WarioWare's are the same, with a near default:
`0x080033a2 cmp r3, #31 ; bls ; b 0x080038fe ; lsls ; ldr ; adds ; ldr ;
mov pc`. The tables hold Thumb addresses with bit 0 clear.

**Modern GCC, `-O2`, Thumb.** Emerald Rogue `0x0800103e`:

```
cmp r1, #8 ; bhi default ; ldr r2, =table ; lsls r1, r1, #2 ; ldr r2, [r2, r1] ; mov pc, r2
```

The table is out of line in `.rodata`, often megabytes from the code; at
`0x08007ec0` it is at `0x091e1a34`. GCC hoists the base out of loops into a
callee-saved register: Rogue `0x08007eba` uses `mov r1, r9`. It narrows
`s16` and `u8` values before the bounds check and then indexes with the
original register. Modern-build `AnimTask_SetGrayscaleOrOriginalPal`,
`0x08013a52`:

```
ldrsh r0, [r4, r3] ; lsls r3, r0, #16 ; lsrs r3, r3, #16 ; cmp r3, #7 ; bhi ; ldr r2, =table ; lsls r3, r0, #2 ; ldr r3, [r2, r3]
```

It also merges cases: `lsrs r3, r2, #1` before a 7-entry table bounded by
`cmp r2, #13` (`WonderNews_Enter`, `0x0813a14c`). Far defaults use `bl` as
in agbcc: the probe found 4,177 far-jump `bl`s in Rogue, and four sampled by
hand were all real.

**Modern GCC with `__gnu_thumb1_case_*`** (libgcc, used when optimising for
size). moonbeam `0x08038838`:

```
cmp r0, #7 ; bls ; b default ; bl 0x08042e14
```

`0x08042e14` is `__gnu_thumb1_case_uhi`. The table of halfword offsets
follows the `bl` inline at `0x08038842`. The helper reads the table at
`lr & ~1`, then adds twice the entry to lr and returns:
`push {r0, r1} ; mov r1, lr ; lsrs r1, #1 ; lsls r0, #1 ; lsls r1, #1 ;
ldrh r1, [r1, r0] ; lsls r1, #1 ; add lr, r1 ; pop {r0, r1} ; bx lr`.
The byte form (`_uqi`, `ldrb`) is at `0x08042e00`. moonbeam has 13
reachable call sites.

**LLVM (rustc), Thumb (`thumbv4t`).** No ROM on quack has one. In the
sample (`llvm.gba`, `0x08000020`):

```
cmp r0, #9 ; bhi default ; add r0, pc ; ldrb r0, [r0, #4] ; lsls r0, r0, #1 ; add pc, r0
```

The inline table holds the byte `(target − (add pc + 4)) / 2`, or a
halfword when the targets are far. This is the TBB idea built from v4T
instructions.

**LLVM, ARM (`armv4t`).** Sample `0x08000090`:

```
cmp r0, #9 ; bhi default ; adr r1, table ; ldr pc, [r1, r0, lsl #2]
```

The table holds words, inline.

**An indexed branch that isn't a switch.** Emerald's crt0 `IntrMain`,
`0x08000370`, ARM: `bx r0` through the IRQ handler table in RAM. Neither
approach resolves it, and neither should: the table is filled at run time.
It is a dynamic exit (decision 5b). It is counted only with ELF code maps:
descent never reaches `IntrMain`, which runs from IWRAM.

## The approaches as built

All three run over the same code map and candidates.

**P0, gbadisasm's matcher as-is** (`disasm.c:198-295` at
`camthesaxman/gbadisasm@e35982bd`). A five-state matcher (`lsl`;
`ldr [pc]`; `add`; `ldr`; `mov pc`), with one instruction of grace. The
table length comes from walking entries until the lowest target above the
table. It covers agbcc only.

**P, per-compiler matchers.** Six shapes:

- agbcc word table;
- GCC `-O2` word table;
- GCC `__gnu_thumb1_case_{uqi,sqi,uhi,shi}`, with the helper found by its
  exact libgcc body;
- LLVM Thumb1 byte/halfword table;
- ARM `ldr pc, [rB, rI, lsl #2]`, with rB = pc or a constant;
- ARM `add pc, pc, rI, lsl #2`.

The length comes from the bounds check: the nearest `cmp rG, #N` (or
`cmp rG, rK` with rK a constant built in the window) before a
`bhi`/`bls`/`bcs`/`bcc`, where rG is the index or a plain copy of it.

**V, value-set analysis** (`vsa.rs`). It runs forward over each function
from its entry. Each register and stack slot holds one of:

- an unknown with an identity;
- a set of up to 1,024 constants, remembering the ROM range it was loaded
  from;
- a strided interval;
- a stack offset.

Its rules:

- After `cmp rX, #k`, a conditional branch refines rX on each edge, and
  also every register holding the same unknown.
- A load from a bounded set of ROM addresses reads them all.
- A small leaf callee runs inline with lr known. This is how the
  `__gnu_thumb1_case` helpers resolve, with no knowledge of them.
- Values are widened after six visits, only at loop heads (targets of
  backward edges), so a value bounded by a check is never widened before
  its use. Calls clobber r0–r3, r12 and lr, except those of r0–r3 and
  r12 a short callee with no calls of its own never writes (read from its
  code, up to 64 instructions, following its branches; since 2026-11-06).
- A jump to lr's entry value, unchanged, is a return wherever it is held:
  m4a's `ply_*` routines do `mov r12, lr; bl ld_r3_tp_adr_i; ...; bx r12`
  (native/recovery-saved-lr-return). With the two rules jt-score's
  unresolved jumps went 38 to 22 (pokeemerald) and 53 to 37 (modern), and
  nothing else it scores moved.
- A jump resolves when its target value at the fixpoint is a finite set
  inside ROM. When a jump is reached from several function entries, it
  counts as resolved only if every one of them resolves it.

There is no compiler-specific code anywhere in V.

In descent mode, discovery follows the union of P's and V's resolutions, so
both see the same code. "V-alone" is a second run where V follows only its
own resolutions. It is the V reported for ROMs without symbols, and it
differs from V by at most one table on every ROM.

## Results

**Checked against ground truth.** Tables resolved exactly, out of the truth
total:

| Build, code map | Truth tables | P0 (gbadisasm) | P | V | V-alone | P or V-alone | Wrong targets (any approach) |
|---|---|---|---|---|---|---|---|
| Emerald (agbcc), ELF | 676 | 675 | 675 | 675 | 675 | 675 | 0 |
| Emerald (agbcc), descent | 676 | 661 | 661 | 661 | 661 | 661 | 0 |
| modern build (GCC 15.2), ELF | 617 | 0 | 580 | 584 | 583 | 600 | 0 |
| modern build, descent | 617 | 0 | 379 | 386 | 385 | 394 | 0 |

- **The one Emerald table everything misses** is `BerryFix_SetScene`. It
  computes `r4 = r6 * 4` before checking `cmp r6, #5`, so the bound is on
  one register and the index on another.
- **The descent rows** miss tables in code that descent never reached.
  Emerald's 15 misses are 14 tables in unreached code plus BerryFix; the
  modern build misses 223–238. That is a discovery-recall problem, not a
  jump-table one. Descent reached 759,395 of the 788,442 instructions the
  agbcc ELF maps (96%), but only 575,388 of 810,403 (71%) on the modern
  build (the `reached` lines in the logs). Modern GCC starts only 61% of
  Thumb functions with
  `push {…, lr}` (agbcc: 93%), and #6's pointer scan requires one.
- **V-alone's 34 misses on the modern build:**
  - 28 are the narrowed-index idiom above. P resolves 15 of them.
  - 2 compute the index before the bounds check (`WonderNews_Enter` and
    `_Exit`, the merged-case form, like BerryFix).
  - 2 have the table base hoisted into a high register on top of another
    problem.
  - 2 are other cases.
- **P's 37 misses on the modern build:** the table base hoisted into a high
  register, the guard far from the dispatch, the merged cases, and the
  narrowed index wherever GCC picked different register names. P resolves
  the narrowed idiom only when the narrowed copy and the index happen to
  share a register: right answers, reached by matching register names
  rather than by reasoning about the values.
- **Both approaches over-read during this work.** P's first version
  followed `lsrs r3, r2, #1` back to the bound on r2 and read 14 entries of
  a 7-entry table, the extra 7 being the next table's. An earlier V build
  read 37 entries around a 5-entry moonbeam table (`0x08033e9a`) when it
  got an inconsistent far-jump answer for the `bl` default. Each over-read
  came from an upstream assumption failing: the guard register for P, the
  far-jump rule for V. Both causes are fixed in the tools. An over-read adds targets but keeps the real ones,
  so at run time only real arms are taken. The cost is garbage decoded as
  code during discovery.

**ROMs without symbols (only internally consistent).** Indexed branches
resolved:

| ROM (compiler) | Instructions reached | Candidates | Indexed | P0 | P | V-alone | P or V-alone | Same targets | Different | P only | V only | Neither |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Super Mariomon (agbcc) | 887,823 | 1,186 | 752 | 751 | 749 | 751 | 751 | 749 | 0 | 0 | 2 | 1 |
| WarioWare (GCC 2.9x) | 410,181 | 216 | 208 | 204 | 204 | 207 | 207 | 204 | 0 | 0 | 3 | 1 |
| Emerald Rogue (modern GCC) | 720,770 | 620 | 466 | 0 | 433 | 449 | 457 | 425 | 0 | 8 | 24 | 9 |
| Emerald Imperium (modern GCC) | 735,550 | 692 | 519 | 0 | 483 | 497 | 508 | 472 | 0 | 11 | 25 | 11 |
| moonbeam (modern GCC) | 107,461 | 125 | 38 | 0 | 28 | 30 | 32 | 26 | 0 | 2 | 4 | 6 |
| llvm.gba sample (LLVM), checked against its bytes | 89 | 10 | 2 | 0 | 2 | 2 | 2 | 2 | 0 | 0 | 0 | 0 |

"Same targets" means both approaches resolve the branch with identical
target sets. On Super Mariomon, gbadisasm's matcher (P0) resolves both
V-only branches (with V's targets), which our P misses: its length rule
doesn't need the bounds check. Super Mariomon's V also resolves 415
non-indexed candidates to a single constant: ARM `ldr ip, [pc]; bx ip`
interworking veneers and Thumb `bx pc` mode switches, which is decision
5b's const-prop.

**Where they disagree, by hand inspection:**

- V only: Super Mariomon `0x08055318`. It is a 415-entry table whose bound
  `cmp r1, r0` has r0 built outside P's window.
- V only: WarioWare `0x0800a46c`. A loop with the table base loaded above
  it, outside P's window.
- V only: Rogue `0x08007ec0`. The base is in r9.
- P only: Rogue `0x0802d2c2` and similar. The narrowed-index idiom.
- P only: moonbeam `0x0803df1e`. A far-jump `bl` default the probe missed,
  so V loses the bound.
- Neither: Rogue `0x0802f970` (the narrowed-index idiom with different
  registers), and Rogue `0x08184db0`, the narrowed idiom again in the
  code the modern build calls `Anim_FourPetal`, which other paths reach
  with the switch value unknown.

## The far-jump `bl`

agbcc and GCC both use Thumb `bl` as an unconditional branch when the
target is out of `b` range. In switches it is the default. Treat it as a
call and lr is clobbered and r0–r3 are havocked on its fallthrough. The
join at the dispatch then loses the bound, and V resolved only 634 of
Emerald's 676 tables before this was handled.

Rules measured against the ELFs. Truth: a Thumb `bl` whose target prints
as `func+0x…` is a far jump. The one exception is a target that is a lone
`bx rN`: GCC's local call-via-register stub, which makes an indirect call,
not a jump. The modern build has 713 of those and agbcc has none.

| Rule | Emerald (548 far jumps of 56,316 `bl`s): found / false / missed | modern build (732 of 57,044): found / false / missed |
|---|---|---|
| target isn't `push {…, lr}` and lies forward | 539 / 4,071 / 9 | 161 / 17,703 / 571 |
| a `b<cond>` right before skips exactly the `bl`, and the target isn't `push` | 282 / 31 / 266 | 385 / 82 / 347 |
| #6's extent rule (target inside the caller's ELF extent) | exact by construction, but needs extents that descent doesn't know | same |
| **sp balance**: probe from the target; far jump iff every return reached has sp above its entry value (it pops the caller's frame) | **547 / 0 / 1** | **721 / 0 / 11** |

The sp-balance probe uses V's stack tracking and needs nothing about the
caller. Probes nest, because a default block can loop back through another
far jump. They are computed in rounds: each round uses the previous round's
answers until nothing changes. Emerald, Super Mariomon, WarioWare and
Rogue settle in 2–3 rounds. The modern build, Imperium and moonbeam stopped
at the prototype's cap of 8 rounds without settling (the counts above are
from round 8). A real
implementation should compute this as a proper fixpoint over the call
graph.

## Cost

- **V.** ~730 non-blank lines in the prototype (`vsa.rs`), in four parts:
  - the abstract domain and join/widen, ~200 lines;
  - per-instruction transfer functions, ~250 lines, which in gbaa are
    `Sem<V = AbsVal>` (decision 3) and so mostly free;
  - the fixpoint driver with refinement on branches and inline leaf calls,
    ~200 lines;
  - the far-jump probe, ~40 lines.

  Run time, single-threaded per ROM, measured with all 11 runs sharing
  quack's 24 threads (from the logs):
  - Emerald with ELF code map: 2.2 s for the V-alone pass, 16.4 s
    including the far-jump rounds;
  - 32 MB hacks: 2.0–3.4 s for the V-alone pass, and 15–53 s in total;
  - memory: 22–98 MB.

  Both parallelise per function. Estimated cost in gbaa: ~500 new lines
  plus the `AbsVal` interpretation of `Sem`.
- **P.** ~280 lines for six shapes (gbadisasm's alone is 59), plus the
  helper signatures. Each new compiler, compiler version or idiom (hoisted
  bases, narrowed indices, guards built from constants) needs more, and it
  fails silently on an unknown compiler: P0 resolves 0 of 617 on modern
  GCC. Run time is negligible.
- **Unresolved jumps cost speed, not correctness**, since they become
  dynamic exits under decisions 5b and 6.
- **What can go wrong is a resolved table:**
  - A *too-short* target list breaks the program unless codegen has a
    fallback. V can produce one: it isn't fully sound, since it ignores
    stores through unknown pointers. So codegen keeps a fallback arm to the
    dispatcher on every resolved table, and a missed case becomes a slow
    path.
  - A *too-long* list (an over-read; both approaches produced one) keeps
    every real target, so it runs correctly, but it decodes data as code.
    Cheap guards: every target must decode, and must land inside the
    function's discovered extent.

## Recommendation

Use value-set analysis (`Sem<V = AbsVal>`) for jump tables, with no
per-compiler matchers.

- Codegen keeps a dispatcher fallback arm on each resolved table, and
  discovery rejects tables with targets that don't decode or that leave
  the function.
- Add the sp-balance far-jump probe to discovery, computed as a real
  fixpoint.
- If profiling later shows modern-GCC games spending time in unresolved
  switches, add backward refinement through the IR's definitions, as
  Ghidra's switch analysis does. That recovers the narrowed-index idiom
  (28 of V's 34 modern-GCC misses) and the index-computed-before-the-check
  form, without matching register names.

Why not matchers too: on the one modern-GCC build with ground truth they
would add 17 tables (583 → 600 of 617). That costs ~280 lines per family of
compilers, plus upkeep, and a failure mode (silent mis-tracing of registers)
that the bug in this study showed. A miss only costs speed.

On accuracy the evidence is close, and the call rests on cost. Nothing
built later depends on this choice yet: matchers can be added behind V
without changing the design. So work can go ahead on V, with the question
at the end of this doc left open for june.

## Effects on decisions

`decisions.md` is unchanged. Proposed changes:

- **5c:** settle on value-set analysis as above, rather than leaving both
  candidates open.
- **3:** `Sem<V = AbsVal>` moves from "later, optionally" to required for
  discovery, since jump tables and the far-jump rule both use it.
- **5a:** discovery needs the far-jump `bl` rule. Use sp balance rather
  than #6's extent rule: 547/548 on agbcc and 721/732 on modern GCC, no
  false ones, and it needs no function extents. #6's rule was exact only
  with ELF extents, which descent doesn't have. Also, GCC's `bl` to a local
  `bx rN` stub is an indirect call: the call's target is in rN, not the
  stub.
- **5d:** #6's pointer scan requires a `push {…, lr}` target. That holds for
  93% of agbcc functions but only 61% of modern-GCC functions, so descent
  reached 71% of the modern build's code against 96% of Emerald's. The 5d
  validation needs another test for leaf functions on modern GCC.
- **Codegen (decision 8, open):** every resolved table keeps a fallback arm
  to the dispatcher, and discovery sanity-checks table targets.

## Reproducing

Nothing here is committed. Tools live in
`~/.local/share/gbaa/tools/jumptables/`; outputs and logs in
`~/.local/share/gbaa/jt/<rom>/`.

- `jt` (Rust, `src/main.rs`, `src/vsa.rs`). It depends on
  `crates/armv4t` in the main checkout (`~/Documents/homelab/gbaa`, at
  `a40b03f`). It writes `cands.tsv`, with one row per candidate and each
  approach's table and targets, and prints the counts. Build:
  `cargo build --release`. Run: `target/release/jt ROM [--elf DIR] --out DIR`.
- `run-all.sh` runs `jt` on every ROM above in parallel, including the
  LLVM sample, which it builds first. It writes `$GBAA_DATA/jt/<name>/` and
  `<name>.log`.
- `truth.py ELF ROM OUTDIR` writes `code.tsv`, `funcs.tsv` and
  `tables.tsv`: the code map and truth tables. Used on
  `src/pokeemerald/pokeemerald.elf` (→ `jt/emerald`) and
  `pokeemerald_modern.elf` (→ `jt/modern`). Run with `python3 -I`.
- `score.py DIR` scores `cands.tsv` against `tables.tsv`: the "checked"
  table, including the "P or V-alone" column. Run with `python3 -I`.
- `compare.py DIR...` gives the symbol-less table: P vs V-alone per
  branch. Run with `python3 -I`.
- `farjump.py ELF ROM JTDIR` gives the far-jump rule table and the
  `push {…, lr}` percentages. JTDIR is `jt/emerald` or `jt/modern`, which
  supply `farjumps.txt` and `funcs.tsv`. Run with `python3 -I`.
- `idioms.py ROM...` gives the idiom counts in the compiler table. Run
  with `python3 -I`.
- `llvm-sample/build.sh` builds `llvm.gba` from `sw.rs` and `start.s`.
- The modern build, in `$GBAA_DATA/src/pokeemerald` at `731ad5bf`, with the
  same environment as #6's build and the toolchain on PATH:
  `PATH=$GBAA_DATA/toolchains/arm-gnu-toolchain-15.2.rel1-x86_64-arm-none-eabi/bin:$PATH make modern -j8`.
  It took 12 s and wrote `pokeemerald_modern.{gba,elf,map}`.

## Limits

- **LLVM is covered only by a two-function sample.** No ROM on quack has an
  LLVM jump table, and the sample's rustc (nightly-2026-02-14) is newer than
  the one that built an agb ROM.
- **The modern build isn't a shipped ROM.** It is pokeemerald's C under GCC
  15.2. Rogue and Imperium were built with whatever devkitARM their authors
  had.
- **V is a prototype.** It is non-relational, ignores stores through
  unknown pointers, and caps sets at 1,024 values. Its far-jump rounds don't
  converge within 8 on three of the ROMs.
- **The extra rules came after looking at the data.** The far-jump rule,
  widening at loop heads, and two bugs in P were all found while measuring
  these same ROMs. The agbcc numbers are on the data that shaped the
  tools. The modern build was added late and shaped only the
  call-via-stub correction.
- **The truth rule finds word tables only.** Byte and halfword tables would
  need their own rule. No ELF used here has any.

## Question for june

Jump tables by value-set analysis alone, or with per-compiler matchers as
a fallback? The accuracy evidence is close (see "Recommendation"), so this
is yours to overrule. Work proceeds on the pick in the meantime.

- **V alone (pick).** No compiler-specific code. On the modern-GCC build it
  misses 34 of 617 tables, which run through the dispatcher (slower, still
  correct) until backward refinement is added.
- **V plus matchers.** 17 more modern-GCC tables now (600 of 617). Costs
  ~280 lines for the compilers seen here, more for each new compiler or
  idiom, and a second way to get a table wrong.
