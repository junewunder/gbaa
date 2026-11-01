# Recovering structured control flow

One of the four literature reviews behind the structured codegen design (unit
`structured/lit-control-flow`, for `structured/design`; phase 2 of the plan
of 2026-10-21 in [open-decisions.md](../open-decisions.md)). The question is
how to turn each guest function's CFG into Rust `if`, `loop`, labeled blocks
and `match`, with no goto, while the frame hash still matches the
interpreter exactly. Researched 2026-10-24 on quack. The sources were read in
their primary form: the papers' PDFs and the tools' source trees at the
commits given under "Sources". Every source is also listed in
[references.md](../references.md).

## Summary

- **Our CFGs are almost all reducible.** On Emerald, 15,145 of 15,158
  functions (99.9%) are reducible. The 13 irreducible regions hold 422
  instructions, 0.05% of the code, and none has more than 15 blocks. Modern
  GCC builds (pokeemerald_modern, Rogue, Imperium) are 99.5% reducible, with
  0.4–0.5% of their code in irreducible regions. Only LLVM code and Duktape
  (in moonbeam) have much: 13% and 11% of their code. The irreducible
  regions are real compiler output: agbcc peels a loop's first test and
  jumps into the body at two places ("Measured", below).
- **For the reducible part, use Ramsey's translation** ("Beyond Relooper",
  ICFP 2022). It is Peterson, Kasami and Tokura's 1973 algorithm, done in one
  pass over the dominator tree. On reducible input it is the only technique
  reviewed that adds no computation and no code: "every action and test in
  the original … has at most one counterpart". It maps one to one onto
  Rust's `loop`, `'b: { … break 'b }` and `if`. That property is what keeps
  it exact for us. Structuring changes only what follows each edge, and every
  edge keeps v1's lowering exactly: flag writes, cycles, tick, yield.
- **The decompilers' structurers are the wrong family for us.**
  - DREAM re-evaluates conditions, guarding them only by a def-use check.
  - rev.ng duplicates code by combing (1.15× at -O2).
  - SAILR duplicates code and copies conditions.
  - Phoenix, Ghidra and Hex-Rays fall back to goto, which Rust doesn't have.

  All of them aim at readable C for a person. We need code that is exact for
  a machine, and reducibility gives us that without their machinery.
- **For the irreducible regions, keep a local dispatch loop.** Collapse each
  one into a single node, `loop { match pc { … } }` over its blocks, entered
  with `pc` set to the entry taken. This is LLVM's and Cheerp's
  FixIrreducibleControlFlow. In Rust it is v1's own code, so it is exact
  already, and it costs a few hundred instructions on Emerald. Node
  splitting would copy at least 529 instructions on Emerald but 18,592 (16%)
  on moonbeam, and its worst case is exponential. It isn't needed.
- **The hard part is our multiple entries, which no paper addresses.** Every
  block start can be entered: return sites (48% of Emerald's blocks), and
  any block after an IRQ, a frame end or an indirect call unwinds the host
  stack. Every algorithm reviewed assumes one entry. Put a virtual entry
  above all those block starts and nearly every loop becomes irreducible, so
  a structurer degenerates back to v1. The workable design:
  - the structured body runs when the function is called at its entry key;
  - any other key runs v1's `loop { match }` body, kept as the resume path;
  - fewer unwinds mean fewer resumes, so the slow path gets rarer. Candidate
    changes: indirect calls and tail calls through `dispatch::call`, and IRQs
    taken without unwinding.

  This doubles the code until something else shrinks it, so it is the main
  cost `structured/design` has to weigh.
- **Loops with several exits cost nothing in Rust.** 17–36% of loops have
  exits to two or more blocks. Labeled `break` reaches any enclosing block,
  so Ramsey handles them with no flag variable. Phoenix, Cifuentes and DREAM
  need one exit variable or goto per extra exit only because C's `break`
  leaves a single loop.
- **Switches: recovery is done; emission is a `match` that keeps its
  fallback arm.** Value-set analysis already resolves the tables (decision
  5c). Emerald has 659 tables. A case target only the switch reaches goes
  inline in its arm. Only shared targets need a labeled block, which nests
  much less than wasm's `br_table`. The `_` arm returns to the dispatcher,
  as decision 8 requires.
- **LLVM does not already do this for us.** In the built Emerald binary,
  `f_0800dd1d` (GetConnectedChildStrength) sends 15 of its 217 direct jumps
  back to the dispatch head, which is an indirect `jmp *%rax` through a
  table. BattleStringExpandPlaceholders sends 149 of 3,309 there. LLVM
  threads some `pc = CONST; continue` edges and not others.
- **`#[loop_match]` is out for now.** It would make v1's own shape jump
  directly (rust-lang/rust #132306), but it is nightly-only and marked
  incomplete, and we build on stable.

## What we start from

### The code

`crates/codegen` emits one Rust fn per guest function, `fn(&mut M, pc: u32)
-> Flow`, whose body is `loop { match pc { … _ => return Flow::Jump(pc) } }`
([codegen-design.md](../codegen-design.md), decision 8). What a structurer
has to keep:

- **The invariant: all guest state is in `Machine` at every block
  boundary.** Registers are in `cpu.r[]`, flags in the CPSR. A Rust frame
  holds nothing but `pc`. So *which Rust code runs the next block doesn't
  matter*, only that the right block runs next. Structuring changes which
  Rust code that is, and nothing else. This review assumes registers stay in
  `Machine`; locals are the registers review's topic (`structured/lit-registers`).
- **Blocks are superblocks with side exits.** A guarded exit mid-block is
  `if g { …; pc = K; if rt::tick(m, n) { return Flow::Jump(pc) } continue; }`
  (the generated `f_0800dd1d` above, in `out/pokemon-emerald-version`). `g`
  is a `bool` local computed once, before the branch, from the flags the IR
  just set. Every edge has its own tick with its own static cycle count,
  plus `rt::fetch` for the region's wait states.
- **The CFG's edges** come from `func::successors`:
  - static targets;
  - a call's return site (a call is an edge to the block after it);
  - a resolved jump table's targets;
  - a `swi`'s continuation.

  Everything else (returns, tail calls, unresolved or foreign jumps) returns
  `Flow::Jump`, which is already structured: `return` leaves every loop.
- **Every block start is enterable.**
  - Return sites are re-entered by the dispatcher after an unwind.
  - Table targets are dispatch keys.
  - When `tick` reports a yield, the whole host stack unwinds with `pc` at
    the next block. So *any* block can be where a function is re-entered,
    after an IRQ, a halt or a frame end. Each caller up the guest stack is
    then re-entered at its return site.
  - Indirect calls and tail calls unwind the same way (v1's defaults).
- **IWRAM overlays** check their guard (`rt::overlay_valid`) in each block's
  arm, and a block may have patched variants (decision 7). Emerald has 97
  overlays (block versions) in 9 functions.

### What "exact" asks of a structurer

The unit's list, item by item. A technique stays exact if it does what the
"Holds if" column says:

| Concern | Holds if |
|---|---|
| Every flag a later instruction may read | Block bodies are untouched. Flag liveness already runs over the function's static edges (decision 9), and structuring adds no edge and removes none. |
| Cycle counts and tick points | Every CFG edge keeps its own `rt::tick(m, n)` and its yield `return`. Nothing merges two edges' ticks, and no new tick is added. |
| I/O order | Each block's code runs once per visit, in order. No condition is evaluated again: a guard is a `bool` local, so a re-test reads the local, never the bus or the CPSR. |
| IRQs | A yield returns the next block's key, as now. Re-entry at that key must reach the right block (the multiple-entries problem below). |
| Self-patching RAM code | Overlay functions keep v1's form, with a guard in each arm. Structuring them would need a guard at each structured block, which is possible but buys little for about a hundred blocks. |
| The interpreter fallback | Any key a function doesn't own still returns `Flow::Jump`. A jump table keeps its `_` arm. |

Most techniques below are exact by this table or easily made so. Two
things separate them: whether they re-evaluate conditions or duplicate code
(both are exact only with care, and duplication costs build memory, the
thing phase 1 fought), and how they treat multiple entries.

### Does LLVM already do it?

If LLVM's jump threading turned every `pc = CONST; continue` into a direct
branch, structuring would buy only compile-time effects. It doesn't, at
least not reliably. I checked the Emerald binary built on 2026-10-22
(`target/recompiled/release/gbaa-pokemon-emerald-version` in the main
checkout) with `objdump -d`:

- `f_0800dd1d`: its head is `lea -0x800dd1d(%rsi),%eax; ror $1; cmp $0x38;
  …; jmp *%rax`. 15 of the function's 217 direct jumps go back to that head,
  each after `mov $0x800dd69,%esi`, a constant key.
- `f_0814e765`: 149 of its 3,309 direct jumps go to its head.
- `f_08006b5d`: its head is a compare tree, not a single table, so the count
  isn't comparable.

So LLVM threads some edges and leaves others going through an indirect
jump. Which ones, and what it costs at run time, is for the emit unit to
measure. The structured form removes the question.

## Measured

`just cfg-survey ROM [TRUTH]` (`crates/codegen/src/bin/gbaa-cfg-survey.rs`)
builds every ROM function as `gbaa-recompile` does (`emit::build_all`) and
analyses codegen's own CFG. It reports:

- **Reducibility.** Every retreating edge of a DFS must go to a node that
  dominates its source (Cooper–Harvey–Kennedy dominators).
- **The loop nest, Steensgaard-style.** Each SCC is a region. Its entries
  are its blocks with a predecessor outside it. One entry makes a natural
  loop, more than one an irreducible region. Cutting the edges into the
  entries exposes the nested regions.
- **Exits.** For each region, the distinct blocks outside it that its edges
  lead to.
- **Return sites** and **jump tables.**
- **Ramsey nesting, an estimate.** How deep Ramsey's translation nests a
  block: one labeled block per merge-node child of each dominator, plus one
  `loop` per header, summed down the dominator tree.

For the irreducible regions, two costs:

- **Dispatch (what stays `loop { match }`):** the outermost regions'
  instructions.
- **Splitting:** what splitting copies, at least one copy of a region per
  extra entry. Controlled splitting can do better than one copy per entry,
  plain splitting worse.

IWRAM copies are left out. Each ROM takes about 20 s and 300 MB.

| ROM (compiler) | Functions | Instructions | Reducible | In irreducible regions | Splitting copies ≥ | Loops | ≥2 exit targets | Return sites / blocks | Tables (targets) | Ramsey nesting p99 / max |
|---|---|---|---|---|---|---|---|---|---|---|
| Emerald (agbcc) | 15,158 | 814,601 | 99.9% (13 not) | 422 (0.05%) | 529 | 4,351 | 17% | 48% | 659 (6,455) | 8 / 37 |
| Super Mariomon (agbcc) | 16,738 | 922,764 | 99.9% (11) | 300 (0.03%) | 407 | 4,673 | 18% | 48% | 1,155 (6,883) | 8 / 40 |
| WarioWare (Nintendo GCC 2.9x) | 5,192 | 435,897 | 100% | 0 | 0 | 2,362 | 13% | 54% | 214 (1,469) | 9 / 16 |
| Kirby & The Amazing Mirror | 5,606 | 622,688 | 99.9% (4) | 780 (0.1%) | 887 | 1,546 | 21% | 30% | 147 (689) | 13 / 28 |
| pokeemerald_modern (GCC 15.2 -O2) | 12,435 | 760,525 | 99.5% (61) | 3,515 (0.5%) | 6,287 | 4,372 | 35% | 45% | 608 (6,347) | 10 / 53 |
| Emerald Rogue (modern GCC) | 13,366 | 876,870 | 99.5% (71) | 4,709 (0.5%) | 5,362 | 4,673 | 35% | 44% | 644 (6,929) | 12 / 185 |
| Emerald Imperium (modern GCC) | 13,765 | 930,704 | 99.5% (63) | 3,379 (0.4%) | 4,069 | 4,860 | 36% | 44% | 711 (7,126) | 11 / 82 |
| moonbeam (devkitARM GCC, Duktape) | 1,214 | 114,956 | 96.8% (39) | 12,199 (10.6%) | 18,592 | 676 | 43% | 44% | 73 (528) | 17 / 54 |
| june's first-frame ROM (agb, LLVM) | 124 | 17,227 | 96.0% (5) | 2,228 (12.9%) | 6,867 | 201 | 64% | 17% | 11 (15) | 15 / 39 |

Compilers are from [jump-tables.md](jump-tables.md), "ROMs and their
compilers". Kirby isn't in that table; it is first-party, so probably the
same SDK compiler as WarioWare, but that is unchecked.

More from the same runs:

- **Emerald's 13 irreducible regions** have 2–15 blocks each, 2 or 3
  entries, and 1 or 2 exits. Most sit in small functions:
  - GetConnectedChildStrength, 8 blocks;
  - SortSprites, 18;
  - Cmd_jumpifnexttargetvalid;
  - MultiBootMain;
  - the m4a driver's CgbSound (55 blocks; its region has 15 blocks and 3
    entries);
  - and others.

  The largest irreducible regions elsewhere:
  - modern GCC: up to 36 blocks (pokeemerald_modern) and 120 blocks (Rogue);
  - moonbeam: 473 blocks with 16 entries, Duktape's bytecode interpreter;
  - june's first-frame ROM: 160 blocks, in qrcodegen's `encode_codewords`
    and agb's main.
- **Why GetConnectedChildStrength is irreducible** (`src/link_rfu_3.c`,
  `for (i = 0; i < 4; flags >>= 1, i++) { if (flags & 1) { … } }`). agbcc
  tests `flags & 1` once before the loop (`0x0800dd34`) and branches either
  to the `if` body (`0x0800dd3a`) or to the increment (`0x0800dd4e`). The
  loop is therefore entered at two blocks. It is a real two-entry loop, not
  an artefact of our CFG.
- **Loop depth:** at most 4 on agbcc, 5 on modern GCC.
- **Sizes:** 92% of Emerald's functions have 16 blocks or fewer, and 100
  have more than 64. The largest, AbilityBattleEffects, has 349 blocks and
  2,701 instructions. The largest in Rogue has 1,563 blocks.
- **Nesting:** the deep cases are a few huge functions.
  - Rogue's 185 is a 1,509-block function, and Imperium's 82 is similar.
    These numbers are upper bounds: case targets only the switch reaches go
    inline in their arms and nest nothing.
  - p99 is 8–18 on every ROM, which is no problem for rustc.
- **One known gap:** one function each in pokeemerald_modern, Rogue and
  Imperium has a block that codegen built but that `successors` doesn't
  reach from the entry. In pokeemerald_modern it is Anim_FourPetal's
  `0x0816135f`. The survey leaves that block out and lists it. It is for
  `structured/cfg-stats` to explain, since an unreachable block can't be
  placed in a structured body (it would stay resume-only).

## The techniques

Each entry says what the technique does, whether it can stay exact for us
(the table above), and what it would cost.

### Relooper (Zakai 2011; Binaryen)

**What it does.** It takes the blocks and a set of entries, and repeats:

- Simple: one entry that can't be returned to.
- Loop: every entry can be returned to. The body is everything that reaches
  an entry.
- Multiple: several entries, each with blocks only it reaches. Each entry
  gets one arm.

Multi-entry shapes are told apart at run time by a `label` variable: set it
before branching in, test it on arrival (paper §3.2). Binaryen's current
`Relooper.cpp` changes the order. It tries Multiple first whenever there
are several entries, and keeps an independent group only if nothing outside
it branches into it. Irreducible flow becomes a multi-entry `LoopShape`
with an `if (label == …)` chain inside ("Multiple entries are possible for
the block, however, which is necessary for irreducible control flow"). There
is no node splitting.

**Exact for us?** Yes, with care.

- The label is a host local, so writing and testing it touches no guest
  state.
- Binaryen's `Branch` contract is a warning: "A condition must not have side
  effects, as the Relooper can reorder or eliminate condition checking."
  Ours are pure `bool` locals, so this holds, but only because the IR
  computes guards before the exit.

**Cost.**

- The label traffic is what Cheerp measured against it: Relooper "is very
  easily tricked … into emitting unnecessary label assignments".
- Relooper's output shape depends on heuristics, not on the dominator tree,
  so it is harder to reason about than Ramsey's.
- c2rust (C to Rust) uses a Relooper with a `current_block` enum and labeled
  blocks and loops. Its irreducible output is `loop { match current_block
  { … } }`, which is v1's shape.
- **Take from it:** the label is the right tool for an irreducible region,
  and conditions must be pure. Use Ramsey for the rest.

### Stackifier (LLVM's WebAssembly backend; Cheerp)

**What LLVM does.** It sorts blocks topologically, ignoring back edges, and
keeps loops contiguous (`WebAssemblyCFGSort.cpp`). Then it places markers
(`WebAssemblyCFGStackify.cpp`):

- a `loop` at each header, ending after the loop's bottom block;
- a `block` for each forward branch to a non-adjacent target, opened at the
  nearest common dominator of the target's forward predecessors and closed
  just before the target.

It emits only `block`, `loop`, `br`, `br_if` and `br_table`, never `if`.
Irreducible regions are fixed first (`WebAssemblyFixIrreducibleControlFlow.cpp`):

- each multi-entry SCC gets one dispatch block, a `br_table` on a fresh
  register;
- each edge into an entry goes through a routing block that sets the
  register;
- the pass recurses into inner loops.

The file states the design: "minimal intervention", "do not duplicate code
in order to resolve irreducibility".

**What Cheerp adds.** Cheerp's `CFGStackifier.cpp` adds if/else nesting: a
successor whose only forward predecessor is the current block goes inside
the branch. Its blog weighs splitting ("no extra runtime cost" but
"can theoretically increase the code size exponentially") against a
dispatcher ("only linearly in the worst case, but at the cost of a small
runtime penalty"), and Cheerp chose the dispatcher.

**Exact for us?** Yes: it moves no code and re-evaluates nothing.

**Cost.**

- With no `if`, rustc gets more labeled blocks and `break`s to digest than
  Ramsey's form gives it.
- The dispatcher costs a register write and an indirect jump on every edge
  into the region, back edges included.
- **Take from it:** the dispatcher for irreducible regions, scoped to the
  region only.

### Ramsey, "Beyond Relooper" (ICFP 2022)

**What it does.** It works top-down over the dominator tree, with children
in reverse postorder (RPO):

- "A node X that has two or more forward inedges is a merge node." Each
  merge child of X gets a `block`, with X's code inside it, and the child's
  code after it ("nodes with smaller reverse postorder numbers go on the
  inside").
- A conditional becomes `if`, with its non-merge children inlined in the
  arms.
- "A node X that has a back inedge is a loop header, and its translation is
  wrapped in a loop form."
- A branch backward, or to a merge node, becomes `br i`. The index is the
  depth in a context stack of `IfThenElse | LoopHeadedBy | BlockFollowedBy`.
  Any other branch inlines the target's subtree.
- Switches (§7.3) become `br_table`, with *all* the node's children placed
  after blocks, since wasm's table can't hold code.

**What it claims, and its limits.**

- The translation "preserves the semantics" and "introduces no additional
  computation". Statically, "every action and test in the original Cmm has
  at most one counterpart".
- It needs a reducible graph: "every loop has a unique entry node",
  otherwise "no nesting could work" (§6).
- GHC splits nodes first, greedily, and the paper admits this "duplicates
  many tens of times more nodes than an optimal sequence".

**Exact for us?** Yes, and by construction.

- No duplication means each block's code, its ticks included, appears once.
- No added computation means no new read of guest state.
- Edges keep their code. Ramsey's "actions" are our block bodies, and each
  branch becomes inline code, `break 'b`, `continue 'l` or the existing
  `return`.
- This one-counterpart property is also cheap to check on the output: every
  `insn!` marker of the function appears exactly once in the structured
  body. That is a check for the validation review (`structured/lit-validation`).

**Cost.**

- Linear in the CFG, with no run-time cost.
- Output size equals v1's minus the `match` scaffolding.
- Nesting depth is what we measured: p99 at most 18.
- **Rust does better than wasm here.**
  - Rust's `if` can hold the non-merge children, as Cheerp does.
  - A Rust `match` arm can hold code, so a switch only needs a block for
    targets that are merge nodes.
  - `break 'b value` isn't needed.
- **Take from it:** the algorithm, for every reducible function and for the
  reducible outer graph of the rest.

### `#[loop_match]` (rust-lang/rust #132306)

`#[loop_match] 'a: loop { state = 'blk: { match state { … } } }`, with
`#[const_continue] break 'blk NEXT`, lowers a constant transition to a
direct jump into the right arm. It was merged as a lang experiment
(PR #138780, 2025-06). The feature is `loop_match`, "incomplete", nightly
only. RFC 3720 (a different syntax) is unmerged. It is exactly v1's shape
with the indirect jump gone, so it would be the cheapest experiment, and it
would serve the resume body and irreducible regions too.

Restrictions:

- the scrutinee must be a local;
- no guards on the arms;
- integer or fieldless-enum states;
- the body must be exactly that assignment.

Our toolchain is pinned to stable (`rust-toolchain.toml`), so this is out
until it stabilises. It is worth a note in the design as the thing to
revisit.

### Decompilers' structurers

All of these aim at C a person can read, with as few gotos as possible.

- **Phoenix** (Schwartz et al., USENIX Security 2013). Structural analysis
  with semantics-preserving schemas: Seq, IfThen, IfThenElse, (Inc)Switch,
  While, DoWhile and NatLoop, with edge predicates checked as well as
  shape.
  - What doesn't match is "virtualized" into a goto, break or continue.
  - Of several loop entries, "the one with the most incoming edges is
    selected", and the edges into the others become gotos.
  - It duplicates nothing and re-evaluates nothing.
  - Without goto, every virtualized edge would need a flag. For us its
    contribution is the warning in its Fig. 3: plain structural analysis's
    natural-loop schema is unsound, collapsing two loop exits so that both
    can appear taken.
- **DREAM / "No More Gotos"** (Yakdan et al., NDSS 2015).
  - Acyclic regions become `if (reaching condition) { node }` in
    topological order, then refined into if/else, switch and else-if
    chains.
  - Multi-entry loops get a structuring variable and a new header that tests
    it. Multi-exit loops send every exit to one successor that tests the
    exits' reaching conditions *again, after the loop*.
  - Conditions may appear more than once in the output (§IV-D). If a tested
    variable "might change" between uses, it saves the value in a new
    Boolean.
  - It reached zero gotos on coreutils, all tests passing. angr's
    measurement of its output: 600% more Booleans and twice the CFG edit
    distance of the others.
  - **Exact for us?** Only with a sound def-use check over guest memory
    (I/O registers change under you), or by saving every condition in a
    local. With the second, it collapses into Erosa–Hendren's flags. Our
    multi-exit loops don't need any of it.
- **SAILR** (Basque et al., USENIX Security 2024) and angr.
  - It traces Hex-Rays' gotos on GCC -O2 to nine compiler transformations,
    then undoes them. Code the compiler duplicated is merged back, guarded
    by a *copied* condition (`DuplicationReverter` synthesises a new
    conditional jump from an earlier one). Merged tails are duplicated
    again (`CrossJumpReverter`).
  - Its gotos: 2,673, against 6,115 for Hex-Rays, 6,575 for Ghidra and
    8,497 for angr's Phoenix on 7,355 Debian functions.
  - **Exact for us?** Its deoptimizations aim at source likeness, which we
    don't need, and each one duplicates code or re-tests a condition. Not
    for us.
- **Ghidra** (`blockaction.cc`, `CollapseStructure::collapseAll`).
  - Rules are applied to a fixpoint: Goto, Cat, ProperIf, IfElse, WhileDo,
    DoWhile, InfLoop, Switch, then short-circuit `||`/`&&`.
  - When stuck, `selectGoto` uses `TraceDAG` scoring to choose an edge to
    make a goto.
  - Irreducible edges are marked (`findIrreducible`) and always become
    gotos. There is no node splitting.
  - Its switch recovery (`jumptable.cc`: backward slicing to the index,
    guards bounding it, emulation of the table load) is the same job our
    value-set analysis already does (decision 5c, jump-tables.md).
- **Hex-Rays.** No source. Its own documentation says only "structural
  analysis … Edges that could not be structured are represented as goto
  regions". The options `HO_FAST_STRUCTURAL` ("may occasionally produce some
  unnecessary gotos") and `HO_UNMERGE_TAILS` ("reduce number of gotos by
  duplicating code") are documented. Guilfanov's Black Hat 2018 paper
  covers microcode, not structuring. The goto counts in the papers above
  are other people's measurements.
- **rev.ng** (Gussoni et al., "A Comb for Decompiled C Code", ASIA CCS
  2020; `revng-c` RestructureCFG).
  - Each loop's retreating edges are normalised through a state variable
    and an entry dispatcher.
  - Abnormal entries are removed by "first iteration outlining" (copying
    the first iteration out).
  - Exits go through an exit dispatcher.
  - "Combing" then duplicates every node between a conditional and its
    post-dominator that the conditional doesn't dominate.
  - Zero gotos, at 1.15× code on -O2 (1.32× on -O3, a tail to about 2.4×).
    angr measured its calls at +116%.
  - **Exact for us?** Yes: each path still runs one copy of each block, and
    the new state is a local. But the duplication is exactly what phase 1
    fought (rustc's memory grows with statements), and it buys readability
    we don't need.
- **Cifuentes** (thesis 1994, ch. 6). Interval-based loops, 2-way
  conditionals found by dominators, and compound conditions. She refuses
  node splitting ("modifies the semantics") and leaves irreducible graphs
  as gotos.
- **Erosa and Hendren** ("Taming Control Flow", 1994). Each goto becomes a
  flag set once and moved outward or inward through loops. Nothing is
  duplicated and each condition is evaluated once. That makes it the exact
  answer if we ever needed goto elimination, and our dispatch-loop region
  is the coarse version of it.

### Irreducible flow

Three answers appear in the sources:

| Answer | Who | Exact? | Cost on our ROMs |
|---|---|---|---|
| Node splitting | GHC (greedy), Janssen–Corporaal (controlled), Unger–Mueller | Yes (a path runs one copy of each block) | ≥529 instructions on Emerald, ≥18,592 (16%) on moonbeam. Uncontrolled splitting grew Janssen–Corporaal's 17 procedures by 235%, worst 1,183%. Controlled splitting grew them by 2.9%. The worst case is exponential (Carter, Ferrante and Thomborson 2003). Ticks are copied with their blocks, so nothing changes at run time. |
| Dispatch variable for the region | LLVM, Cheerp, Relooper, c2rust, DREAM, rev.ng | Yes (a host local) | A store plus an indirect jump per edge into the region. In Rust, a `loop { match pc }` over the region's blocks: v1's own code. |
| goto | Phoenix, Ghidra, Hex-Rays, Cifuentes | n/a | Rust has none. |

For us the dispatch region is the clear default. It is the code we already
emit, so it is exact by the same evidence as v1, and it is linear. The
regions are tiny where it matters (Emerald) and large only in code built
around interpreters (Duktape) or LLVM state machines. Node splitting could
be a later option for a small region with two entries, if a profile shows
the dispatch there.

### Multiple entries

None of the sources has our problem: a function that may be *resumed* at
any block. The two options they would suggest both fail:

- **A virtual entry above every block start makes almost every loop
  irreducible.** Each call inside a loop puts a return site in its body, and
  48% of Emerald's blocks are return sites. Even counting return sites
  alone, the dispatch region would be most of every function: v1 again.
- **Splitting per resume point** (rooting a copy of the function at each
  one) multiplies the code by the number of return sites.

What works is to serve the two kinds of entry with two bodies:

- **Called at its entry key** (every Rust call from another function, and a
  dispatcher call of a function's entry): the structured body.
- **Any other key** (a resume after an unwind, a jump-table target reached
  through the dispatcher): v1's `loop { match }` body, unchanged.

A resumed call finishes its current invocation in the v1 body, then returns
as usual. The next call of that function is structured again. By the
invariant, both bodies are correct from any block boundary, so a function
may even move from one to the other. One case doesn't end: a function that
never returns, such as a main loop resumed after every frame, stays in v1
for good. What decides how much code runs in the slow body is how often we
unwind. Today that is every IRQ, halt and frame end, every indirect call and
every tail call (codegen-design.md, "Defaults taken"). Each of those can be
made not to unwind without changing guest-visible state, because the same
blocks run with the same ticks:

- an indirect call through `dispatch::call`, as stub calls already are;
- a tail call returned to the caller's own trampoline rather than to the
  dispatcher;
- an IRQ taken by a nested dispatcher inside `tick`;
- a frame end that parks the run thread rather than unwinding it.

All of those are `structured/design`'s to weigh, with dispatcher counts from
a real run. The cost of two bodies is about twice the code, so twice the
rustc memory, which phase 1 just halved. Possible mitigations: emit the
resume body only for functions that have a non-entry key in the dispatch
table *and* are seen resumed in a recompile-time run (as `variants.rs`
runs the ROM), and let the rest resume through the interpreter until
their next call.

### Loops with several exits

17% of Emerald's loops, and 35% on modern GCC, leave to two or more
distinct blocks. In C this is what forces Phoenix's virtualized gotos,
DREAM's exit cascades and rev.ng's exit dispatcher. In Rust, `break 'b` out
of any number of enclosing loops and blocks is plain code, and Ramsey's
translation places each exit target after a block that encloses the loop.
They cost nothing, need no variable, and re-evaluate nothing.

### Switches from jump tables

Recovery is settled: value-set analysis resolves 675 of 676 agbcc tables
and 583 of 617 modern-GCC ones exactly, with no wrong target (decision 5c,
[jump-tables.md](jump-tables.md)). Cifuentes and Van Emmerik's normal forms
and Ghidra's `jumptable.cc` are the same idea by slicing. What structuring
adds is the emission:

- **Lower the dynamic jump as now.** `exit(m, at, thumb, e)` runs
  `apply_exit` (the interpreter's alignment and T-bit code) and gives the
  key. Then `match` on the key: one arm per target, each with its own edge
  code (tick and yield), and `_ => return Flow::Jump(pc)` for anything
  else. That fallback arm is decision 8's.
- **Inline a target only the switch reaches into its arm.** A target that
  is a merge node (a shared default, cases that share a body) gets a
  labeled block around the `match`. Emerald's largest table has 69
  targets, Rogue's 198.
- **The guard before the table** (`cmp r0, #N; bhi default`) is an ordinary
  conditional edge. No need to fold it into the `match` the way Ghidra's
  `foldInOneGuard` does: folding would drop the guard's own flag writes and
  cycles.

## What our design should take

For `structured/design`:

1. **Ramsey's dominator-tree translation for every reducible function and
   for the reducible outer graph of the others.**
   - Use Rust's `if` for non-merge children (Cheerp's improvement), `match`
     arms with inline code for non-merge case targets, and labeled blocks
     and `break 'b` for merge nodes and every loop exit.
   - Each CFG edge's code is v1's exactly: flag writes, cycles, tick and
     yield `return`. The structurer only chooses what follows.
2. **A collapsed node for each irreducible region**, `loop { match pc }`
   over its blocks (LLVM's FixIrreducibleControlFlow, in v1's own form).
   No node splitting for now.
3. **Two bodies per function:** structured from the entry key, v1's
   `loop { match }` from any other key, with overlay functions staying v1
   only. Weigh the doubled code against fewer unwinds: indirect calls, tail
   calls, IRQs and frame ends that don't unwind would make resumes rare
   enough to emit the resume body lazily or let the interpreter take them.
   This is the one place where the design has a real choice to make; the
   rest follows from the sources.
4. **No technique that re-evaluates a condition or duplicates a block**
   (DREAM's repeated conditions and exit cascades, SAILR's
   deoptimizations, rev.ng's combing). The first is exact only with a
   def-use proof over memory that includes I/O; the second costs build
   memory for readability we don't need. If an extra test is ever needed,
   save the value in a `bool` local first (Erosa–Hendren, DREAM §IV-D).
5. **A cheap static check of the output:** each guest instruction's
   `insn!` marker appears exactly once in a function's structured body, and
   every edge's tick is present. That is Ramsey's "at most one counterpart"
   made testable, beside the frame hash and block lockstep (for
   `structured/lit-validation`).
6. **Measure before building:** the run-time share of resumed (non-entry)
   dispatcher calls, and LLVM's threading of v1's edges, which is partial
   (see "Does LLVM already do it?"). Both decide how much the structured
   form buys.
7. **Revisit `#[loop_match]` when it stabilises.** It would speed v1's form,
   which we keep for resumes and irreducible regions, with no structuring
   at all.

## Sources

Read from the primary text, on 2026-10-24. Source trees at the commit given.

- Alon Zakai, "Emscripten: An LLVM-to-JavaScript Compiler", SPLASH '11
  Companion, pp. 301–312, doi 10.1145/2048147.2048224. Read: the 2013-05-14
  revision in emscripten-core/emscripten `docs/paper.pdf`, §3.2.
- Binaryen, `src/cfg/Relooper.{h,cpp}`, github.com/WebAssembly/binaryen at
  `f1f3a483`.
- LLVM, `llvm/lib/Target/WebAssembly/WebAssembly{FixIrreducibleControlFlow,
  CFGSort,CFGStackify,ISelLowering}.cpp`, github.com/llvm/llvm-project at
  `3f550f8b`.
- Yuri Iozzelli, "Solving the structured control flow problem once and for
  all", Leaning Technologies, 2019-04-11 (Medium; read via the Wayback
  Machine, snapshot 20240106222428), and Cheerp's
  `llvm/lib/CheerpWriter/CFGStackifier.cpp` and
  `llvm/lib/CheerpUtils/FixIrreducibleControlFlow.cpp`,
  github.com/leaningtech/cheerp-compiler at `8adfb412`.
- Norman Ramsey, "Beyond Relooper: Recursive Translation of Unstructured
  Control Flow to Structured Control Flow (Functional Pearl)", PACMPL 6,
  ICFP, Article 90, 2022, doi 10.1145/3547621. Read: the author's copy,
  cs.tufts.edu/~nr/pubs/relooper.pdf.
- c2rust, `c2rust-transpile/src/cfg/` (relooper.rs, structures.rs,
  multiples.rs, loops.rs), github.com/immunant/c2rust at `820a9408`.
- Rust `#[loop_match]`: tracking issue rust-lang/rust #132306,
  implementation PR #138780, `rustc_feature/src/unstable.rs` at `9cd159cf`;
  RFC 3720 (rust-lang/rfcs PR, unmerged) at `12af0ec8`.
- Khaled Yakdan, Sebastian Eschweiler, Elmar Gerhards-Padilla, Matthew
  Smith, "No More Gotos: Decompilation Using Pattern-Independent
  Control-Flow Structuring and Semantics-Preserving Transformations", NDSS
  2015.
- Edward J. Schwartz, JongHyup Lee, Maverick Woo, David Brumley, "Native x86
  Decompilation Using Semantics-Preserving Structural Analysis and Iterative
  Control-Flow Structuring", USENIX Security 2013.
- Zion Leonahenahe Basque et al., "Ahoy SAILR! There is No Need to DREAM of
  C: A Compiler-Aware Structuring Algorithm for Binary Decompilation",
  USENIX Security 2024; angr `angr/analyses/decompiler/` (structuring/,
  region_identifier.py, optimization_passes/), github.com/angr/angr at
  `4f8708be`.
- Ghidra decompiler, `Ghidra/Features/Decompiler/src/decompile/cpp/`
  (blockaction.cc, block.cc, jumptable.cc),
  github.com/NationalSecurityAgency/ghidra at `16400303`.
- Hex-Rays: docs.hex-rays.com (microcode view, decompiler configuration);
  Ilfak Guilfanov, "Decompiler Internals: Microcode", Black Hat USA 2018.
- Andrea Gussoni, Alessandro Di Federico, Pietro Fezzardi, Giovanni Agosta,
  "A Comb for Decompiled C Code", ASIA CCS 2020; revng-c
  `lib/RestructureCFG/` at `cddd9039`, and revng at `352c75e3`.
- Cristina Cifuentes, "Reverse Compilation Techniques", PhD thesis, QUT,
  1994, ch. 6; Cristina Cifuentes and Mike Van Emmerik, "Recovery of Jump
  Table Case Statements from Binary Code", Science of Computer Programming
  40(2–3), 2001.
- Ana Erosa and Laurie Hendren, "Taming Control Flow: A Structured Approach
  to Eliminating Goto Statements", ICCL 1994.
- Johan Janssen and Henk Corporaal, "Making Graphs Reducible with
  Controlled Node Splitting", TOPLAS 19(6), 1997; Sebastian Unger and Frank
  Mueller, "Handling Irreducible Loops: Optimized Node Splitting vs.
  DJ-Graphs", Euro-Par 2001 (the TOPLAS 2002 version wasn't reachable);
  Larry Carter, Jeanne Ferrante, Clark Thomborson, "Folklore Confirmed:
  Reducible Flow Graphs are Exponentially Larger", POPL 2003.

Not read beyond the abstract: Havlak's "Nesting of Reducible and
Irreducible Loops" (TOPLAS 1997), Ramalingam's comparison of loop-nesting
forests (TOPLAS 2002), and Stanier and Watson's survey of irreducibility
(SPE 2012). None of the sources measures ARM or GBA code; the table above
is the only measurement for our ROMs.

## Reproducing

```text
just cfg-survey "$HOME/ROMs/gba/Pokemon - Emerald Version.gba" "$GBAA_DATA/truth/pokeemerald.funcs.tsv"
just cfg-survey "$GBAA_DATA/src/pokeemerald/pokeemerald_modern.gba" "$GBAA_DATA/truth/pokeemerald_modern.funcs.tsv"
just cfg-survey ROM            # any other ROM, unnamed
```

Each run takes about 20 s and 300 MB on Emerald. The LLVM threading check
is `objdump -d` on the built `gbaa-pokemon-emerald-version` binary, counting
direct jumps to each function's dispatch head; it needs a built Emerald,
which is heavy.
