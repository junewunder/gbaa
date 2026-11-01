# Knowing a codegen transformation stayed exact

This is one of the four literature reviews behind the structured codegen design. It is
unit `structured/lit-validation`, for `structured/design`, and part of phase 2 of the
plan of 2026-10-21 in [open-decisions.md](../open-decisions.md). The
question: when we turn `loop { match pc }` into `if`s and loops, and later
move guest registers into Rust locals, how do we know the output still
matches the interpreter, by something finer and cheaper than a frame hash?
The literature: translation validation, CompCert's validated passes, SMT
equivalence checking (Alive2 and its relatives), validation of binary
lifters, differential testing and fuzzing of compilers and emulators, and
how other recompilers and emulators test their translators.

Researched 2026-10-24 on quack. Papers were read in full text where they
could be fetched (the exceptions are listed under "Sources"). Source trees
were shallow-cloned at the commits listed there and read, never built or
run. Every source is also in [references.md](../references.md). The two
companion reviews this one leans on are
[structured-control-flow.md](structured-control-flow.md) and
[static-recompilers.md](static-recompilers.md).

## Summary

- **Today the interpreter is not a lockstep state reference for compiled
  code, and won't be without a block-timed mode.** Decision 11 ticks
  compiled code once per block and the interpreter once per instruction.
  So an I/O read mid-block sees a different clock, and IRQs land at
  different instructions. Measured on Emerald with the built binary
  (below): at frames 300, 600 and 1500 the frame hash matches, but our
  exact save states differ in 25, 24 and 983 payload bytes. The IRQ
  placement shows in `lr_irq` and a word of the IRQ stack in IWRAM at all
  three frames, and at 1500 in the C flag of the CPSR and the IRQ SPSR.
  The rest is the timing state of I/O, sound and the scheduler, and `pc`,
  partly because the interpreter's frame ends after any instruction and
  compiled code's only at a block exit. Only one other GBA tool co-simulates, gbarecomp, and its
  generated code charges cycles per instruction, so its states align with
  its interpreter's at any cycle. Ours don't.
  - Consequence: **for structuring and registers-as-locals, the reference
    should be v1's own output** (today's `loop { match pc }` code), which
    has the same tick points. A block-timed interpreter mode (below) would
    make the interpreter one too. The interpreter stays the reference for the
    instruction semantics (`sst`, `sst-ir`, `sst-gen`) and for frames. A
    structured function and its v1 function must agree on the full state
    and the ordered bus trace at every tick, not only at frame ends.
- **Translation validation is the right frame, and the cheap kind is
  enough.** Pnueli et al. (1998) and Necula (2000) check each run of the
  compiler rather than the compiler. CompCert does the same for its
  heuristic passes. An untrusted OCaml oracle proposes, and a small checker
  either accepts or makes compilation fail: register allocation
  (`backend/Allocation.v`), linearization (`backend/Linearize.v`: "every
  reachable instruction occurs exactly once"), switch compilation
  (`validate_switch`), and in CompCert-KVX block duplication and
  tunneling. The data on false alarms is clear:
  - narrow validators written for one pass had none (Rideau–Leroy's
    register allocation, lazy code motion, software pipelining);
  - general ones refused 10–36% of functions (Necula; Peggy; Tristan,
    Govereau and Morrisett).

  So: **one narrow checker per transformation, run at recompile time.** A
  function that fails it stays in v1 form, as decision 10 already says for
  ABI checks.
- **Structuring needs no SMT at all.** It changes only the shape of the
  control flow, never the state. The CompCert Linearize/Duplicate pattern
  fits exactly: the structurer reports which original block each piece of
  structured code is, and a checker verifies four things:
  - each block appears exactly once, with its code unchanged;
  - each edge goes to the original successor;
  - each edge keeps its flag writes, its cycles and its tick;
  - every key that isn't reached falls through to the dispatcher.

  The simulation relation is then the identity on state. This is Ramsey's
  "at most one counterpart" made into a pass check, and it is the same
  static check [structured-control-flow.md](structured-control-flow.md)
  item 5 asks for. Necula's hard limit ("all branches in the target program
  must correspond to branches in the source program") holds for us by
  construction.
- **Registers as locals is register allocation run backwards, and
  Rideau and Leroy's validator is the model.** It is a backward dataflow
  over equations `local = guest register`, plus a structural pairing of
  each IR op with its lowered op. At every point where state leaves the
  function's control, the equations must say that `Machine` holds every
  register. Those points are a tick that may yield, a call, `rt::exit`, a
  guard that fails into the interpreter, and `swi`. They report no false
  alarms and +6% compile time; the proof is a tenth of what verifying the
  allocator would take. Dead-flag elimination fits the same frame (VOC's
  guarded partial data abstraction; Song et al.'s "constrained-equivalent"
  rules with an online flag-liveness check). That is also where the gap the
  companion review found shows up: **a yield is a point where every flag
  is observable**, because `IntrMain` saves the SPSR to IWRAM. A checker
  of decision 9 that treats yields that way rejects today's output. That is
  the right answer, and it is fixed by computing the dropped flag writes
  on the yield path (static-recompilers.md, item 2).
- **SMT per block is feasible and stricter than Alive2's problem, but it
  is the third thing to build, not the first.**
  - Our IR has no undefined behaviour, poison or undef. Refinement becomes
    plain equivalence, and blocks are loop-free.
  - The IR keeps effects in order, so the two sides' loads, stores, ticks
    and exits can be paired by position. Each pair of loads returns the
    same fresh variable, an uninterpreted bus. Alive2 by contrast can't do
    volatile at all (`llvm2alive.cpp`: `// TODO: Add support for
    isVolatile()`), and it orders calls only through threaded memory.
  - Alive2 found 47 LLVM bugs, but on whole programs timeouts were as
    common as proofs (SQLite: 2,314 validated, 2,102 timeouts).
  - STOKE and DDEC (Sharma et al. 2013) show the order to build in: random
    test cases first, a solver only on what survives, and every
    counterexample turned into a test case.
  - Multiplies need to be uninterpreted functions. Alive, STOKE and
    Dasgupta et al. all hit solver blow-ups on them (McSema's `mulq` ran
    24 hours).
- **Differential testing from real states is what finds bugs cheaply,
  and we have most of it built.**
  - `sst-ir` already runs a million random 2–6-instruction blocks from
    SingleStepTests states through the IR builder and const-prop and
    compares each against the interpreter: registers and the ordered bus
    trace, 0 differ, 16 s. It doesn't see timing: the evaluator runs
    `Timing` ops as nothing and the interpreter side doesn't tick.
  - `sst-gen` does single instructions through generated Rust.
  - `call-<name>` binaries call one guest function compiled or
    interpreted.
  - What's missing is a corpus of real entry states. Csmith, EMI and
    PokeEMU all say uniform random states miss the real paths. PokeEMU's
    QEMU `iret` bug needed two pages, "very low probability if the address
    … were chosen uniformly at random".
  - EMI's idea adapts well: at a function's entry, randomise everything
    liveness calls dead. The live outputs must not change. That tests the
    assumption flag liveness and register summaries rest on.
- **Other emulators' practice, in one line each.**
  - Box64 has a real per-opcode co-simulation, `BOX64_DYNAREC_TEST`. It
    skips flags ("flags are volatile, so don't test them").
  - FEX runs every test at 1-instruction and 500-instruction block sizes,
    making block granularity a differential axis, and snapshots the
    generated code for every guest instruction (InstructionCountCI).
  - Dolphin bisects by hand with per-instruction-group JIT-off switches.
  - DuckStation runs one hardware test suite under its interpreter, cached
    interpreter and recompiler, storing results to RAM "because the
    recompiler doesn't write registers back". That is the visibility
    problem registers-as-locals will give us.
  - gbarecomp hashes the full state on a cycle stride, with sub-hashes per
    subsystem. Before trusting any A-vs-B result it runs four gates:
    1. recomp vs recomp shows no divergence;
    2. interp vs interp shows none;
    3. an injected fault is found at the right checkpoint ("the ONLY gate
       that catches a silently-blind coordinator");
    4. a periodic byte compare is done even when the hashes match.
  - remill runs each test under all 128 combinations of the x86 flags.
- **What to take, in order of cost.** These are in "What our design should
  take":
  1. a static structure check;
  2. v1-vs-structured co-simulation on full state at ticks, with the
     gbarecomp gates;
  3. per-function differential runs from recorded real entry states, plus
     dead-state randomisation;
  4. the register-equation checker when locals arrive;
  5. SMT on blocks last, if at all.

## What we start from

The code is in [codegen-design.md](../codegen-design.md); decisions 8, 10
and 11 in [decisions.md](../decisions.md); flag liveness (open decision 9)
in [open-decisions.md](../open-decisions.md) and, as built, in
[sem-design.md](../sem-design.md). What matters here:

- One Rust fn per guest function, `loop { match pc }` over its blocks. All
  guest state lives in `Machine` at every block boundary. Every block start
  is a key the dispatcher can enter.
- Each block exit runs that edge's flag writes, adds its static cycles and
  calls `rt::tick`. When the tick says yield, it does
  `return Flow::Jump(pc)`. Emerald has 153,229 ticks.
- Flag liveness (decision 9) drops flag writes no static edge of the
  function reads. Every other exit keeps all four live.
- Each guest instruction carries an `insn!(addr, "…")` marker in the
  printed code: 814,601 in Emerald. A structure check can count these.
- The IR (`analysis::ir`) is pure 32-bit and 1-bit operations, plus
  effects that keep their order: `Load`, `Store`, `Timing`, register and
  PSR writes, and exits. It has a reference evaluator,
  `analysis::ir::eval::run`, that runs a block on the interpreter's own
  `Cpu` and `Bus` (but evaluates `Timing` ops to nothing).

### The checks we have

| Check | Compares | Against | Granularity | Cost |
|---|---|---|---|---|
| `just sst` | registers, banks, PSRs, data bus transactions in order | SingleStepTests' after-state (NanoBoyAdvance) | one instruction | seconds |
| `just sst-ir` | the same, IR builder + `optimize` | SST, and the interpreter on 1M random blocks of 2–6 SST opcodes from SST states | instruction; straight-line block | 16 s for both tests (run 2026-10-24: 997,643 blocks, 0 differ) |
| `just sst-gen` | the same through generated Rust | the interpreter | one instruction per fn | 22,500 fns, 14 s build |
| `call-<name>` | registers after one guest function | `--interp` | function | one build of the game's crate |
| `recompile-check`, `first-frame`, `just unit` | last frame hash and registers (top four CPSR bits excused); route checkpoints add the compiled share | `--interp`, and mGBA for the interpreter | frame | a game's crate build (Emerald: minutes, several GB) |
| `own-state-check`, `rewind-check` | frame hashes after a state round trip | the same build straight through | frame | as above |

SST itself doesn't record cycles reliably. Its README says "the correct
number of cycles and transactions happening on the correct cycle are not
recorded. The correct order is." So no check today tests that a
transformation kept a tick's place or its cycle count, except through a
frame hash many ticks later.

### What "exact" asks of a check

A transformation is exact if a check that sees all of these can't tell the
two outputs apart:

| Observable | Where it shows | A check sees it if |
|---|---|---|
| Every flag a later instruction may read | CPSR at any point another piece of code can run: a yield (the IRQ handler saves the SPSR to IWRAM), a call, an exit, a guard failing into the interpreter | it compares the full CPSR at those points, not only at frame ends |
| Cycle counts and tick points | the argument and place of each `rt::tick`, `rt::fetch`, `rt::stall` and timing op | it compares the ordered list of timing events with their cycle counts |
| I/O order | each `m.bus.load/store`, in order, with width, address and value | it compares the ordered bus trace, as SST does |
| IRQs | which tick yields, and `cpu.pc` (the key) and the full state there | it compares full state at each tick that yields |
| Self-patching RAM code | the overlay guard runs before each block of RAM code, `rt::tick_ram` at its exits, and the per-block patched versions (`codegen::variants`) are chosen as now | the structure check keeps every guard, RAM tick and version choice on its block; runs include the patching ROMs |
| The interpreter fallback | every key the code doesn't own leaves through `_` to the dispatcher | the structure check proves every key not reached in the structured body still leaves through `_` |

### Measured: interpreted and recompiled states at a frame boundary

To see how far a full-state compare is from passing today, Emerald was run
with the crate built on 2026-10-23
(`gbaa-lane1/target/recompiled/release/gbaa-pokemon-emerald-version`).
Each run went to frame N twice, once recompiled and once with `--interp`,
saving our exact state (`runtime::snap`) after frame N. Then the two files
were compared byte by byte (commands under "Reproducing"; the counts below
leave out the header's payload CRC). The state is saved at the frame's end
(`desktop/src/lib.rs`, `--state-out`), before the `settle` step that makes
`--regs` comparable, so `pc` is where each run stopped: the interpreter
after the instruction that crossed V-blank, compiled code at a block exit.

| Frame | Frame hash | `--regs` | Payload bytes that differ | Where |
|---|---|---|---|---|
| 300 | equal (`6378906e719f0552`) | equal | 25 | `cpu.pc` (low byte, `c6` vs `c8`); `lr_irq`; one IWRAM word at `0x03007F9C` (the IRQ stack); 22 after OAM (I/O, DMA, sound, scheduler) |
| 600 | equal (`60b59b6da83fdb18`) | — | 24 | `lr_irq`; one IWRAM byte; 22 after OAM |
| 1500 | equal (`fca753e560aaad10`) | — | 983 | `cpu.pc` (`c6` vs `cc`); the CPSR's and the IRQ SPSR's top byte (C, `6` vs `4`); `lr_irq`; two IWRAM bytes; 977 after OAM |

The frames are the same, so by today's oracle both builds are correct. The
state isn't the same. An IRQ was taken at a different instruction (at 600
`pc` is equal and `lr_irq` still differs), so a different return address
sits in `lr_irq` and on the IRQ stack. The devices' clocks differ, partly
from that and partly from where each run stopped. By frame 1500 the sound state has drifted in
almost a thousand bytes, though the picture hasn't. This is decision 11's
known cost ("IRQs land at slightly different instructions … so the checks
compare final state … not traces"), now measured on the full state. It
means that a full-state co-simulation against the interpreter, gbarecomp's
oracle, would report a divergence at the first IRQ in every game. The
reason is decision 11's design, not a bug.

Two references do share our tick points:

- **v1's output.** A structured function must match it at every tick.
- **A block-timed interpreter**: run the interpreter's `step` over one
  compiled block's instructions without ticking, then tick once with the
  block's summed cost at the block's exit, as compiled code does. Both
  sides already call the same `rt::stall`/`Bus::data_cycles` accounting,
  so this would be a mode of the fallback, not a new interpreter. It would
  make the interpreter the full-state reference for codegen too, and the
  only reference that can catch a bug v1 and the structured form share.
  Until then, a `call-<name>` run with IRQs masked already compares CPU
  and memory exactly against `--interp`, leaving out only the clock and
  timing state.
  (Built 2026-10-24 as `--interp-blocks`; with it Emerald's states at
  frames 300, 600 and 1500 are byte-identical to the recompiled build's.
  See `structured/block-timed-interp` in
  `docs/structured-codegen-design.md`.)

## The techniques

Each section says what the technique checks, whether it can stay exact for
us by the table above, and what it would cost.

### Translation validation (Pnueli, Siegel and Singerman 1998; Necula 2000; Rinard 1999; VOC 2003)

**Pnueli et al.** (TACAS 1998) coined the term. They check "each
individual translation (i.e. a run of the compiler)". An analyzer writes a
proof that a "rudimentary proof checker" accepts. The proof is a
refinement mapping from target states to source states. That mapping, and
the invariant it needs, are *generated from the known shape of the code
generator* (§5.2), with one observation location per loop iteration. That
works because "a single loop iteration in the source corresponds to a
single iteration in the target" (§6). The point for us: when the
transformation is ours, we know its shape, and the mapping can be emitted
by the pass rather than searched for. Our ticks are those observation
locations.

**Necula** (PLDI 2000) validated GCC's passes with no help from GCC. His
simulation relation is a set of (source pc, target pc, equalities between
temporaries). It is checked by symbolic evaluation from one related pair
to the next. Observables are "the same sequence of function calls and
returns", extended in footnote 1 to "memory reads and writes to volatile
locations". That is our bus trace and tick list. Branch matching is a
heuristic (§5.2, "Navigating Branches"). Its hard limit is that every
target branch must correspond to a source branch. False alarms, from
Table 2 (branch failures and unsolved constraints): register allocation
0% and 0.1%, scheduling 0% and 0.01%, CSE 1.5% and 3.5%, loop
optimisation 3.0% and 3.2%. Compilation ran about 4× slower. The
validator was 10,500 lines of ML.

**Rinard**'s credible compilation (MIT-LCS-TR-776, 1999) has the compiler
emit the proof: standard invariants for analysis results, and simulation
invariants relating the two CFGs. Two cautions apply to us:

- Simulation one way doesn't preserve non-termination (§5). Our guest
  code mostly runs forever, so a check must compare at ticks, not only at
  function exits.
- The final printer stays trusted (§6). The proposed remedy is to keep the
  last representation one node per instruction, so printing is trivial.
  Ours is: one `insn!` per instruction, and prims printed one-to-one.

**VOC** (Zuck, Pnueli, Fang, Goldberg, J.UCS 2003) gives the general rule.
It treats "each I/O device as a variable, and each I/O operation
removes/appends elements to" it (§2.2), so I/O order is part of the state.
It splits transformations into two kinds:

- *Structure-preserving*: there are control points where observables can
  be matched, with at least one in every loop.
- *Structure-modifying*: loop interchange, tiling and fusion, which need
  other rules.

The data abstraction is *partial and guarded*: a source variable may have
no target counterpart at some points (dead code, code motion). That is
exactly a dead flag.

**Exact for us?** Yes. Every structuring and registers-as-locals step we
plan is structure-preserving: guest block starts stay control points, and
every loop keeps its tick. Nothing we plan is structure-modifying (no
unrolling, no fusion), and the companion reviews rule those out anyway.
**Cost:** a general validator like Necula's is a large project with a
false-alarm rate. The pass-specific form below is small.

### CompCert's validated passes

CompCert verifies most passes in Coq. For the heuristic ones, an untrusted
OCaml oracle produces the result and a verified checker accepts it or
fails compilation. Tristan and Leroy (POPL 2008, §2.1): a validator V is
correct if `V(c1, c2) = true ⇒ c1 ≤ c2`. Otherwise "compilation is
aborted". In the source at `e276a3f`:

- **Register allocation** (`backend/Allocation.v`, "Register allocation by
  external oracle and a posteriori validation"). The oracle is
  `backend/Regalloc.ml` with `IRC.ml`. The checker is described in Rideau
  and Leroy (CC 2010) and proved in `backend/Allocproof.v`
  (`transf_program_correct`, a forward simulation RTL → LTL). It has two
  parts:
  1. a structural check: each RTL instruction is paired with a
     structurally similar LTL block whose successors agree;
  2. a backward dataflow over sets of equations `pseudoreg = location`.
     Backward, because the sets stay as small as the live ranges.

  They report no false alarms, several bugs caught in the spilling
  heuristics, +20% register allocation time (+6% total), and a 900-line
  proof for a 350-line validator against over 10,000 lines to verify the
  allocator: "a ten-fold reduction".
- **Linearization** (`backend/Linearize.v`). The node order comes from
  untrusted Caml (`enumerate_aux`, in `Linearizeaux.ml`). Correct code
  needs only that "every reachable instruction occurs exactly once in the
  enumeration". Failures are "duplicates in enumeration" or "wrong
  enumeration".
- **Switches** (`backend/Selection.v`): `compile_switch` in OCaml,
  `validate_switch` proved in `common/Switch.v`.
- **CompCert-KVX** (`ae52184`), a fork with more of these:
  - `backend/Duplicate.v`: "RTL node duplication using external oracle".
    `verify_mapping` checks that each new node copies its old node and that
    successors map consistently.
  - `RTLTunneling.v` and `LTLTunneling.v`: the oracle returns a branch
    target and a distance that must strictly decrease ("bad distance in
    Lbranch"), which rules out creating an infinite silent loop.
- Validators outside mainline, from the papers:
  - lazy code motion (PLDI 2009): no false alarms on 17 benchmarks,
    validation 22.5% of the pass's time, and "control flow restructuring …
    is not allowed by our validator";
  - software pipelining (POPL 2010): about 300 lines of OCaml, "many bugs"
    found in the pipeliner, no false alarms, under 5% overhead.

CompCert's observables (`common/Events.v`) are traces of system calls,
volatile loads and stores, and annotations, plus how the program ends.

**Exact for us?** Yes, and this is the pattern to copy. Our structurer and
register assigner are heuristics (which irreducible regions to collapse,
which registers to keep in locals). A checker for each is small. And a
failure has a safe answer CompCert doesn't have: keep the function in v1
form. **Cost:** a few hundred lines of Rust per pass, run at recompile
time over 15,158 functions. That is cheap next to printing. We don't need
Coq: the checker is small enough to review, and it is tested by the
differential runs below.

### Equality saturation and parameterized equivalence (Stepp, Tate and Lerner 2011; Kundu, Tatlock and Lerner 2009)

**Peggy** (Stepp, Tate, Lerner, CAV 2011) puts both functions in one
program expression graph, with the heap as an explicit value, and
saturates it with axioms until both sides' results coincide. It is blind
to where code is placed, so code motion and scheduling are free. It
validated 74–88% of functions after a 10-pass LLVM pipeline on SPEC2006.
Failures came from "incomplete axioms for linear arithmetic; insufficient
alias information". Tristan, Govereau and Morrisett (PLDI 2011) report a
similar 80%.

**PEC** (Kundu, Tatlock, Lerner, PLDI 2009) proves a rewrite *rule*
correct once, over programs with metavariables, rather than each output.

**Exact for us?** Peggy's placement-blindness is the wrong property for
us. Moving a load across a tick is a bug here, and an equality-saturation
validator would accept it unless every bus access and tick were threaded
as an ordered effect, which removes its advantage. PEC suits a fixed set
of peephole rules (the lean printer's folds), not structuring. **Cost:**
high, with a 12–36% refusal rate. Not for us.

### SMT equivalence checking: Alive, Alive2, Souper, STOKE and DDEC

**Alive** (Lopes et al., PLDI 2015) proves InstCombine rewrite templates
correct for all widths. It translated 334 and found 8 wrong. Proofs take
seconds, but "several hours or longer" with multiplication or division
(§6.1).

**Alive2** (Lopes, Lee, Hur, Liu, Regehr, PLDI 2021; source at `defe4a4`)
checks that each target function refines its source:

- `TransformVerify::verify` in `tools/transform.cpp` runs a series of
  queries: UB, return domain, poison, undef, values, memory.
- Loops are unrolled to a bound; paths past the bound go to a sink that is
  never checked.
- A call to an unknown function is "a pure function that takes its
  arguments as well as the current memory as inputs, and returns a value
  and a fresh memory" (§6). Target calls are matched to source calls by a
  fresh index variable. Order is enforced only through the threaded memory
  and per-function call counts.
- Volatile and atomic accesses are refused (`llvm2alive.cpp`).
- SMT timeout 10 s by default (`--smt-to`). A timeout is reported, never
  counted as a bug or a pass.

Results: 47 LLVM bugs. On whole programs with a 1-minute timeout: SQLite
2,314 validated, 38 violations, 2,102 timeouts, 7,543 unsupported (§8.4).

**Souper** (Sasnauskas et al. 2017) synthesizes and proves replacements
for loop-free integer DAGs, treating loads as free variables.

**STOKE** (Schkufza, Sharma, Aiken, ASPLOS 2013) validates with a solver
too slowly ("well below 100" per second) to use in its search. So it
scores candidates on 32 test cases and proves only the survivors,
loop-free, from the same initial state. A counterexample "is used to
produce a new testcase". It treats multiplication and division as
uninterpreted functions with "identical random values when executed on
identical inputs", because the solver diverges on them.

**DDEC** (Sharma, Schkufza, Churchill, Aiken, OOPSLA 2013) proves loops
equivalent with no knowledge of the transformation:

- It finds cut points by matching test traces, and linear invariants from
  the test states (null spaces).
- Each path pair goes to Z3.
- It is sound ("insufficient data will cause the proof to fail") but
  slow: 0.13 s to 166 s per function, one at 12,889 s.

**Exact for us?** Per block, yes, and more easily than for LLVM:

- Our IR has no UB, poison or undef, so Alive2's refinement checks become
  one equality check.
- Our blocks are loop-free, so there is no unrolling.
- Effects keep their order in the IR, so the two sides' event lists can be
  compared by position. That is stricter than Alive2's call matching, and
  it is what I/O order needs.

The encoding:

1. Make the registers, banks, flags and PSRs symbolic.
2. Walk both sides, building a list of events: each `Load`, `Store`,
   `Timing` and exit, with its guard, width, address and value.
3. Give the k-th load on each side the same fresh variable.
4. Assert the lists have equal length and kinds, with equal guards and
   payloads, the same exit, and equal final state on what is live out.
5. Make the multiply prims uninterpreted when both sides have the same
   ones.

What is live out must be everything at a yield, for the reason in the
summary. Because the semantics are written once over the `Sem` trait, a
third `Sem` implementation that builds solver terms would give the
interpreter's side of the check for free. That would check the builder and
passes against `Sem` itself, the job `sst-ir` does by testing.

**Cost:** a solver dependency (or SMT-LIB text to an external `z3`, kept
out of `just check`), a few hundred lines for the encoder, and timeouts to
handle as "unknown". Structuring and registers-as-locals don't change what
a block computes, so per-block SMT doesn't check them. It checks
block-local rewrites: const-prop, the lean printer's folds, dead flags
within a block. Those are already tested by a million random blocks in
`sst-ir`, except for their timing ops, which `sst-ir` doesn't compare.
Worth it only if a rewrite gets clever enough that random blocks
stop reaching its cases.

### Validating binary lifters (Dasgupta et al. 2020; MeanDiff 2017; Song et al. 2019; Sewell et al. 2013)

**Dasgupta, Dinesh, Venkatesh, Adve, Fletcher** (PLDI 2020; the paper
was not reachable, so the source is the abstract and the artifact repo at
`579377e`) validate McSema in two stages:

1. Each x86 instruction's lifted LLVM is checked against a formal x86
   semantics in K, with one Z3 query per register, flag or memory value.
   This found 29 new McSema bugs. Several appeared only when both operands
   are the same register.
2. For whole programs, a "compositional lifter" builds a reference lifting
   from the checked instructions. Both liftings are normalized with 17 LLVM
   passes and matched as data-dependence graphs. 2,254 of 2,348 functions
   were proved; the largest took 2 minutes.

The second stage is the shape our structure check takes: validate the
small pieces once, then check whole functions structurally.

**MeanDiff** (Kim et al., ASE 2017) lifts one instruction with three
lifters, translates all three to one IR and compares them with Z3. It
found 24 bugs, one hit by 1 input in 2^32 (`bsf`). It doesn't compare
flags the manual leaves undefined.

**Song et al.** (USENIX ATC 2019) verify learned ARM→x86 translation
rules with symbolic execution. They then accept "constrained-equivalent"
rules: an unmatched condition code (ARM `subs` sets C, x86 `decl`
doesn't) is recorded and checked dead by a "lightweight online analysis"
when the rule is applied (§4.3–4.4). That is dead-flag elimination, made
checkable per use.

**Sewell, Myreen, Klein** (PLDI 2013) validate gcc's ARM output for seL4
against the C. Loops are matched at split points found by near-exhaustive
search (33 of 43 on the first candidate). It fails at `-O2` on unrolling
and code motion. Volatile and hardware accesses are left out.

**Exact for us?** The two-stage shape, yes. MeanDiff's and Box64's
skipping of undefined flags, no: our reference is our own interpreter, and
every flag it computes is defined. **Cost:** the per-instruction stage is
what SST already gives us. The structural stage is the structure check.

### Two versions of one program (regression verification, product programs)

v1 against structured is two versions of one program, which is what
regression verification proves equivalent. **Godlin and Strichman**
(DAC 2009; read through Sayedoff and Strichman, arXiv:2207.14364) work
bottom-up over the call graph:

- functions already proved equal become uninterpreted functions;
- loops become recursion;
- the loop-free remainder goes to a bounded model checker (CBMC).

It "works well for recursive functions as long as they are in sync, and
typically fails otherwise." **Barthe, Crespo and Kunz** (FM 2011) build a
product program that steps the two sides together where they align and
separately where they don't. That handles different control structures,
unlike a plain cross-product, but needs relational invariants written by
hand.

**Bansal and Aiken** (OSDI 2008) learn PowerPC→x86 peephole rules and
check each in two stages, "a fast execution test and a slower boolean
test" (SAT over every bit of state). Equivalence is modulo a register map
and live registers, with condition codes handled explicitly (§5.3). It is
STOKE's order, ten years earlier, inside a binary translator. **Hasabnis
and Sekar** (ASPLOS 2016; abstract only) learn the asm→IR mapping from
GCC's code generator; nothing in it applies to checking our own
transformations. **Liu and Wang** (ISSTA 2020; abstract) test decompilers
by recompiling their output and comparing behaviour with the original:
1,423 error-triggering inputs and 13 confirmed bugs across four
decompilers. That is our frame-hash oracle in another field.

**Exact for us?** Yes, and our case is the easy one. Structuring keeps
every guest block, and every block start stays a point where the two
sides are in sync with equal state. So Godlin–Strichman's "in sync" case
always holds: each block is the same IR on both sides, and the proof
reduces to matching edges. That is the structure check, without a model
checker. A product program or DDEC-style invariant search would be needed
only for a transformation that moved code across block boundaries, and
the design rules those out. **Cost:** none beyond the structure check;
the general tools aren't needed.

### Formal ARMv4T models (Sail, Isla, L3)

There is no ARMv4T Sail model. Armstrong et al. (POPL 2019) "translate
only the AArch64 64-bit part". Isla (CAV 2021) turns Sail into
per-instruction traces of ordered events constrained by SMT, the exact
shape the encoding above wants, but only for ISAs with Sail models. HOL4's
L3 ARM model (HOL4 `ea4969d`,
`examples/l3-machine-code/arm/model/armScript.sml`) does have `ARMv4T`
and a Thumb encoding, per Fox and Myreen (ITP 2010). It has no timing.
Where the manual says UNPREDICTABLE but the ARM7TDMI does something
definite (`ldm`/`stm` with the base in the list, the multiply carry), it
won't agree with our interpreter.

**Exact for us?** As an independent check of the semantics, partly; it
says nothing about timing. **Cost:** substantial. SST, built from
NanoBoyAdvance, is the independent semantics check we have, and it
passes. Not needed for codegen validation, which compares our code with
our code.

### Differential testing and fuzzing (McKeeman 1998; Csmith 2011; EMI 2014)

**McKeeman** (Digital Technical Journal 1998; full text not reachable)
named the method: run many generated inputs through several
implementations and treat any disagreement as a bug.

**Csmith** (Yang, Chen, Eide, Regehr, PLDI 2011) found "more than 325
previously unknown bugs". It took two things:

- generating programs "while avoiding the undefined and unspecified
  behaviors that would destroy its ability to automatically find
  wrong-code bugs";
- an oracle that is one checksum of the global state, printed at exit.

For us the full-state hash is that checksum. We have no undefined
behaviour to avoid, because the interpreter defines everything.

**EMI** (Le, Afshari, Su, PLDI 2014) runs a program on an input, records
coverage, and deletes code that wasn't executed. Every variant must give
the same output on that input. It found 147 bugs in GCC and LLVM, and the
abstract says it "can be adapted to validate program transformation and
analysis systems in general". Two adaptations for us:

- **Equivalence modulo dead state.** At a function's entry, randomise every
  flag (and, once registers have summaries, every register) that liveness
  says is dead there. The function's live outputs and bus trace must not
  change. This tests the assumption decisions 9 and 10 rest on, which
  nothing tests today. A flag that liveness wrongly calls dead shows up as
  a different branch or value.
- **Perturbing ROM bytes no run executed**, then recompiling: the executed
  part must still match. This tests discovery and jump tables more than
  codegen. It is noted for the discovery thread, not taken here.

**Exact for us?** Yes. Differential testing compares outputs and needs no
proof. Its weakness is coverage. **Cost:** low, given the harnesses below.

### Testing emulators (EmuFuzzer 2009; KEmuFuzzer 2010; PokeEMU 2012; SingleStepTests; risu)

**EmuFuzzer** (Martignoni, Paleari, Fresi Roglia, Bruschi, ISSTA 2009;
method read in their WOOT 2009 paper) and **KEmuFuzzer** (ISSTA 2010)
work the same way. They set the same state (pc, registers, memory,
exceptions) on hardware and an emulator, run one instruction, and compare.
Data are random; opcodes are enumerated with structured operands.
KEmuFuzzer's diff (`x86_cpustate_diff`) ignores what the targets don't
model, such as FPU reserved bits and debug registers.

**PokeEMU** (Martignoni, McCamant, Poosankam, Song, Maniatis, ASPLOS
2012) runs symbolic execution over Bochs to choose states that reach every
path of each instruction. It made 610,516 tests, with complete path
coverage for about 95% of instructions, in 545 CPU-hours. QEMU differed
in 60,770 and Bochs in 15,219. One finding was memory access *order*: QEMU
pops `iret`'s stack items in the wrong order. That bug "can be significant
only if the values read lie on different pages… very low probability if
the address and segment limit were chosen uniformly at random". Uniform
random states miss what path-guided or recorded states find.

**SingleStepTests ARM7TDMI** (`e3097d8`) has 20,000 cases per file,
"generated from fleroviux's excellent NanoBoyAdvance". Each case holds the
initial and final registers and an ordered transaction list (fetch, read
or write, with size, address and data). Memory is answered from the list.
Order is recorded; cycles are not reliable (quoted above). We already run
it three ways: interpreter, IR and generated Rust.

**risu** (QEMU/Linaro; read in the rth7680 fork at `eed2249`, because
Linaro's server didn't answer) is the closest to what we lack:

- Random instruction *sequences* are generated from pattern files
  (`arm.risu`, `thumb.risu`), with constraints per encoding.
- After each instruction a trap compares the master's registers (real
  hardware) with the apprentice's (QEMU). It compares
  `cpsr & 0xF80F0000`, the flags only.
- A trace mode records the master once, so later runs need no hardware.

`sst-ir`'s random blocks are the same idea over our IR. What isn't done is
sequences through generated Rust: `sst-gen` is one instruction per fn.

**Exact for us?** Yes for semantics. Codegen transformations need longer
sequences and real states, below.

### How other emulators and recompilers test their translators

- **gbarecomp** (`COSIM_ORACLE.md` at `7bb70ec`) is the most complete
  oracle in the field, and the only GBA one.
  - It pairs the recompiled code with its own interpreter in one binary:
    "Oracle independence is NOT a requirement".
  - Both run to the same value of the master cycle counter, on a stride
    fixed at launch (65,536 by default), comparing "the clock-keyed
    full-state chain hash, NOT on block leaders (the recomp's CFG blocks
    and the interpreter's per-instruction steps do not align)".
  - The hash covers the CPU (pc excluded, since the recompiler keeps it
    only at dispatch), all memory (page hashes kept up to date on write),
    and I/O, PPU, audio, DMA, timers with their prescaler phase, the
    prefetch buffer and the clock, each as its own sub-hash. Host-only
    state is excluded.
  - It runs the four gates listed in the summary.
  - On a mismatch it dumps the sub-hashes and the last 8,192 checkpoints,
    and `--start-cycle` re-runs a window finely.
  - Bugs found: a dropped 3-cycle refill on `movs pc, lr`, the interpreter
    not banking r8–r12, and a per-instruction cycle skew between the
    codegen and interpreter cost models. After those, Minish Cap, LeafGreen
    and Emerald ran clean to frame ~298.
  - This works for them because their generated code charges cycles per
    instruction (`g_runtime_cycles += cycles` at the per-instruction charge
    site). For us it works only between two codegen outputs with the same
    tick points, or against a block-timed interpreter (above).
- **Box64** (`420eda6`, `src/emu/x64test.c`): `BOX64_DYNAREC_TEST=1`
  runs "Each opcode … on interpreter and on DynaRec, regs and memory are
  compared". A shadow interpreter takes a dry single step from a copy of
  the state, and the registers and touched memory are compared. On a
  mismatch it prints and resyncs, so one run reports several bugs. Flags
  are skipped because Box64 computes them lazily.
- **FEX** (`9eb5d92`):
  - Every `unittests/ASM` test carries its expected registers as JSON and
    runs three ways: one instruction per block, up to 500, and 500 with
    multiblock. A bug that shows at one block size and not another points
    at the block-level code.
  - InstructionCountCI snapshots the host code generated for each guest
    instruction, so any change to generated code shows in review.
- **Dolphin** (`c12121f`, `JitBase.h`): `JITDISABLE(setting)` drops one
  instruction group to the interpreter, and bugs are bisected by hand over
  a dozen group switches. FifoCI hashes frame dumps per commit.
- **DuckStation** (`8d9f6e8`) runs hardware-verified test ROMs under its
  interpreter, cached interpreter and recompiler. Results go to RAM
  "because the recompiler doesn't write registers back when exiting".
- **mGBA's CInema** (`fd9dc5c`) runs per-test frame ranges, with input,
  against PNG baselines. That is our route checkpoints, by another name.
- **remill** (`56918a8`) runs each instruction test natively and lifted,
  from the same random stack, under all 128 combinations of seven x86
  flags. Undefined flags are masked per test (`TEST_IGNORE_FLAGS`).
- **XenonRecomp** (registers as locals among its options) checks only by
  recompiling Xenia's PPC instruction tests. **PS2Recomp** has unit tests.
  **N64Recomp** documents none. None of the console recompilers checks a
  transformation against an untransformed build. gbarecomp's L1 codegen
  harness (one generated function per IR op, against the interpreter) is
  our `sst-gen`.

### Narrowing a divergence

Zeller and Hildebrandt's delta debugging (TSE 2002) shrinks a failing
input, or isolates the smallest difference between a passing and a
failing one. The emulator form is bisection over time by state hashes:

1. Find the first frame whose hash differs.
2. Restore the state from the frame before.
3. Hash at a finer stride.
4. Dump fields at the first differing checkpoint.

gbarecomp does steps 3–4 with `--start-cycle`. We have exact states at
frame boundaries (`runtime::snap`, `--state-at`), so step 2 is free.
Dolphin's group switches and FEX's block sizes are the other kind of
bisection: over *which transformation* is on, not over time.

## Verdict, by technique

| Technique | Flags | Ticks and cycles | I/O order | IRQs | RAM code | Fallback | Cost for us |
|---|---|---|---|---|---|---|---|
| Frame hash (today) | only via the picture | only via the picture | only via the picture | only via the picture | yes, end to end | yes | a game build per run |
| Structure check (CompCert Linearize/Duplicate) | each edge's writes kept | each edge's tick kept | blocks unchanged | yields kept | guards kept | `_` arm kept | ~hundreds of lines, at recompile time |
| Full-state hash at ticks, v1 vs structured (gbarecomp) | yes, at every tick | yes | via state (add a bus-trace hash for order) | yes | yes | yes | a second table in a debug build; incremental page hashes, checkpoints at yields and on a stride |
| Per-function differential runs from recorded entry states | yes | if ticks are logged | if the bus is logged | if IRQs are injected | if patching ROMs are in the corpus | yes | the `call-` harness plus a recorder |
| Dead-state randomisation (EMI) | tests liveness itself | — | — | — | — | — | small, on top of the above |
| Register-equation checker (Rideau–Leroy) | as dead-flag equations | at exits | — | at yields | at guards | at exits | ~hundreds of lines, when locals arrive |
| Per-block SMT (Alive2-style, STOKE order) | yes | as events | as events | as all-live points | — | — | a solver, an encoder, timeouts |
| Regression verification, product programs | in sync at block starts: reduces to the structure check | same | same | same | same | same | none beyond the structure check |
| General validators (Necula, Peggy) | yes | only with effects threaded | same | same | — | — | large, 10–36% false alarms |
| Formal model (L3 ARMv4T) | for semantics | no | partly | no | no | no | large |

## What our design should take

For `structured/design`:

1. **Make v1's output the reference for codegen transformations**, and say
   so in the design. The interpreter isn't one at state level today
   (measured above: IRQs land at other instructions, and timing state
   differs). v1 has the same tick points as any structured or
   registers-in-locals form. Keep the interpreter as the reference for
   semantics and frames, as now. v1 can't catch a bug it shares with the
   transformed form. For that, build the block-timed interpreter mode,
   which makes the interpreter a full-state reference too. It is worth
   doing when the first transformation lands, since it also checks v1.
2. **A structure check for every structured function, at recompile time**
   (CompCert's Linearize/Duplicate pattern, Ramsey's "at most one
   counterpart"). The structurer records, for each piece of output, which
   v1 block or edge it is. The checker verifies:
   - every block's IR is emitted once, unchanged, with its `insn!`
     markers, its overlay guard, its `rt::tick_ram` and its choice of
     patched version (`codegen::variants`);
   - every edge goes to the original successor, with the original flag
     writes, cycles, tick and yield `return`;
   - every key in the table that the structured body doesn't start at
     still reaches the dispatcher (or the v1 body).

   A function that fails stays in v1 form and is counted in `report.txt`.
   It is a pure function over the plan, so it is unit-tested in
   `cargo test -p codegen` and runs on every recompile.
3. **v1-vs-structured co-simulation on full state, gbarecomp's way.**
   - A debug build carries both bodies for a chosen set of functions, or
     two builds run side by side.
   - Our ticks are the shared clock, so checkpoints are ticks, not a cycle
     stride. Hashing the whole snap payload (about 640 KB on Emerald) at
     each of tens of thousands of ticks a frame would cost gigabytes of
     hashing a frame. So, as gbarecomp does, keep per-page hashes of
     memory updated on write, and hash at every yield and on a stride of
     ticks, plus a running hash of the bus trace for I/O order.
   - Compare at each checkpoint, with sub-hashes for CPU, each memory,
     I/O, sound and the scheduler.
   - Take the gates before believing a result: v1 vs v1 and structured vs
     structured show no divergence, an injected one-byte fault is found at
     its tick, and a periodic byte compare is done even when the hashes
     match.
   - Narrow by frame hashes, then the exact state from the frame before,
     then per-tick hashes.
   - Run it on the route checkpoints we have, so it costs runs, not new
     routes.
4. **Per-function differential runs from real entry states,** extending
   `call-<name>`:
   - Record the `Machine` at function entries during an interpreted run
     (a sample per function, with the pages it touches).
   - Mutate each recorded state: all 16 NZCV values (remill's lesson),
     randomised dead state (EMI), perturbed live values.
   - Run each function in v1 and in the transformed form to its return,
     comparing full state, the bus trace and the tick list.
   - Run several configurations: structuring on or off, locals on or off,
     flag liveness on or off (dead-flag elimination is itself a
     transformation; its reference is the same function with liveness
     off). One that alone fails names the
     transformation (FEX's block sizes, Dolphin's switches).

   This is finer than a frame and needs no route. It is the "cheap
   per-function differential run" the card asked about. Its blind spot is
   coverage, which recorded states mostly fix (Csmith, PokeEMU).
5. **Treat a yield as a point where every flag is live, in the checker and
   in codegen.** v1 and the structured form share flag liveness, so item
   3 can't see the stale-flags gap. Item 3's machinery run with liveness
   on against liveness off (same tick points) does: it fails at the first
   IRQ after a dropped flag write, in the CPSR saved to the IRQ stack,
   until the yield path computes the dropped writes (static-recompilers.md,
   item 2). That is the test for that fix: a memory compare at a
   checkpoint, as that review asked.
6. **When registers move into locals, a Rideau–Leroy checker:**
   - backward equations `local = guest register`;
   - each IR op paired with its lowered op;
   - on every tick's yield path, and at every call, exit, failing guard
     and `swi`, every guest register equal to its `Machine` slot (written
     back there only, so a tick that doesn't yield costs nothing).

   Fail into v1 form, as decision 10's safe mode already does. Its
   per-function differential runs in item 4 compare `Machine` only at
   those points, since locals are invisible elsewhere (DuckStation's
   problem).
7. **risu-style sequences through generated Rust:** extend `sst-gen` from
   one instruction per fn to `sst-ir`'s random 2–6-instruction blocks, and
   to short multi-block functions with branches, against the interpreter.
   Compare the timing calls too (the list of `rt::tick`/`rt::fetch`/
   `rt::stall`/timing-op arguments against the interpreter's
   `armv4t::cycles::cost`), which `sst-ir` doesn't. Cheap (the generators
   exist), and it tests the printer on sequences, which nothing does below
   a ROM.
8. **Leave SMT for later.** Per-block SMT checks rewrites inside a block,
   which `sst-ir`'s million random blocks already test, and it doesn't
   check structuring or locals. Revisit it if a block-level rewrite gets
   too clever for random testing to reach its cases. Then follow STOKE's
   order (tests first, solver on survivors, counterexamples back as tests),
   build the solver side as a `Sem` implementation, and keep it out of
   `just check`.
9. **Snapshot a fixed set of generated functions** (FEX's
   InstructionCountCI): a handful of Emerald functions of each shape,
   printed, compared in review. It is not a correctness check. It makes
   every change to generated code visible.

## Sources

Read in full text unless marked.

Translation validation and validated passes (papers as text, source
shallow-cloned):

- A. Pnueli, M. Siegel, E. Singerman, "Translation Validation", TACAS
  1998, LNCS 1384, pp. 151–166.
- G. C. Necula, "Translation Validation for an Optimizing Compiler", PLDI
  2000, pp. 83–95.
- M. Rinard, "Credible Compilation", MIT-LCS-TR-776, 1999.
- L. Zuck, A. Pnueli, Y. Fang, B. Goldberg, "VOC: A Methodology for the
  Translation Validation of Optimizing Compilers", J.UCS 9(3), 2003.
- J.-B. Tristan, X. Leroy, "Formal Verification of Translation Validators:
  A Case Study on Instruction Scheduling Optimizations", POPL 2008;
  "Verified Validation of Lazy Code Motion", PLDI 2009; "A Simple,
  Verified Validator for Software Pipelining", POPL 2010.
- S. Rideau, X. Leroy, "Validating Register Allocation and Spilling", CC
  2010.
- CompCert, github.com/AbsInt/CompCert at `e276a3f`
  (`backend/Allocation.v`, `Allocproof.v`, `Linearize.v`, `Selection.v`,
  `common/Switch.v`, `common/Events.v`); CompCert-KVX,
  gricad-gitlab.univ-grenoble-alpes.fr/certicompil/compcert-kvx at
  `ae52184` (`backend/Duplicate.v`, `RTLTunneling.v`).
- M. Stepp, R. Tate, S. Lerner, "Equality-Based Translation Validator for
  LLVM", CAV 2011.
- J.-B. Tristan, P. Govereau, G. Morrisett, "Evaluating Value-Graph
  Translation Validation for LLVM", PLDI 2011.
- S. Kundu, Z. Tatlock, S. Lerner, "Proving Optimizations Correct Using
  Parameterized Program Equivalence", PLDI 2009.
- T. Sewell, M. Myreen, G. Klein, "Translation Validation for a Verified
  OS Kernel", PLDI 2013.
- C. Song et al., "Towards Constrained-Equivalent Translation Rules for
  DBT" (USENIX ATC 2019).

SMT equivalence and lifter validation:

- N. Lopes, D. Menendez, S. Nagarakatte, J. Regehr, "Provably Correct
  Peephole Optimizations with Alive", PLDI 2015.
- N. Lopes, J. Lee, C.-K. Hur, Z. Liu, J. Regehr, "Alive2: Bounded
  Translation Validation for LLVM", PLDI 2021; github.com/AliveToolkit/alive2
  at `defe4a4` (`tools/transform.cpp`, `llvm_util/llvm2alive.cpp`,
  `llvm_util/cmd_args_list.h`).
- R. Sasnauskas et al., "Souper: A Synthesizing Superoptimizer",
  arXiv:1711.04422.
- E. Schkufza, R. Sharma, A. Aiken, "Stochastic Superoptimization", ASPLOS
  2013; R. Sharma, E. Schkufza, B. Churchill, A. Aiken, "Data-Driven
  Equivalence Checking", OOPSLA 2013 (from github.com/StanfordPL/stoke at
  `98d8a0f`).
- S. Bansal, A. Aiken, "Binary Translation Using Peephole
  Superoptimizers", OSDI 2008.
- S. Dasgupta, S. Dinesh, D. Venkatesh, V. Adve, C. Fletcher, "Scalable
  Validation of Binary Lifters", PLDI 2020 (abstract and the artifact
  github.com/sdasgup3/PLDI20-Artifact-Evaluation at `579377e`; the paper
  was not reachable); S. Dasgupta et al., "A Complete Formal Semantics of
  x86-64 User-Level Instruction Set Architecture", PLDI 2019.
- S. Kim et al., "Testing Intermediate Representations for Binary
  Analysis" (MeanDiff), ASE 2017.
- N. Hasabnis, R. Sekar, "Lifting Assembly to Intermediate
  Representation", ASPLOS 2016 (abstract and the LISC README only).
- A. Armstrong et al., "ISA Semantics for ARMv8-A, RISC-V, and
  CHERI-MIPS", POPL 2019; A. Armstrong et al., "Isla", CAV 2021 (in part).
- HOL4's L3 ARM model, github.com/HOL-Theorem-Prover/HOL at `ea4969d`
  (`examples/l3-machine-code/arm/`); A. Fox, M. Myreen, "A Trustworthy
  Monadic Formalization of the ARMv7 Instruction Set Architecture", ITP
  2010 (abstract).
- G. Barthe, J. M. Crespo, C. Kunz, "Relational Verification Using
  Product Programs", FM 2011; B. Godlin, O. Strichman, "Regression
  Verification", DAC 2009 (through Sayedoff and Strichman,
  arXiv:2207.14364).

Differential testing, fuzzing and emulator practice:

- W. McKeeman, "Differential Testing for Software", Digital Technical
  Journal 10(1), 1998 (not reachable; cited).
- X. Yang, Y. Chen, E. Eide, J. Regehr, "Finding and Understanding Bugs
  in C Compilers", PLDI 2011.
- V. Le, M. Afshari, Z. Su, "Compiler Validation via Equivalence Modulo
  Inputs", PLDI 2014.
- L. Martignoni, R. Paleari, G. Fresi Roglia, D. Bruschi, "Testing CPU
  Emulators", ISSTA 2009, and "Testing System Virtual Machines", ISSTA
  2010 (abstracts; the method from Paleari et al., "A fistful of
  red-pills", WOOT 2009, and the KEmuFuzzer source, mirror
  github.com/oliver-wm/kemufuzzer at `780ef60`).
- L. Martignoni, S. McCamant, P. Poosankam, D. Song, P. Maniatis,
  "Path-Exploration Lifting: Hi-Fi Tests for Lo-Fi Emulators", ASPLOS
  2012.
- C. Liu, S. Wang, "How Far We Have Come: Testing Decompilation
  Correctness of C Decompilers", ISSTA 2020 (abstract).
- A. Zeller, R. Hildebrandt, "Simplifying and Isolating Failure-Inducing
  Input", TSE 28(2), 2002.
- SingleStepTests ARM7TDMI, github.com/SingleStepTests/ARM7TDMI at
  `e3097d8`.
- risu, read in github.com/rth7680/risu at `eed2249` (`risu_reginfo_arm.c`,
  `risugen_arm.pm`, `arm.risu`); upstream is
  git.linaro.org/people/peter.maydell/risu.git.
- mstan's gbarecomp, github.com/mstan/gbarecomp at `7bb70ec`
  (`COSIM_ORACLE.md`, `src/debug/cosim*`, `oracle/gba_cosim.py`).
- Box64, github.com/ptitSeb/box64 at `420eda6` (`src/emu/x64test.c`,
  `docs/USAGE.md`); FEX, github.com/FEX-Emu/FEX at `9eb5d92`
  (`unittests/ASM`, `unittests/InstructionCountCI`); Dolphin,
  github.com/dolphin-emu/dolphin at `c12121f`
  (`Source/Core/Core/PowerPC/JitCommon/JitBase.h`) and
  github.com/dolphin-emu/fifoci at `1cf03c9`; DuckStation,
  github.com/stenzek/duckstation at `8d9f6e8`
  (`src/core-tests/psxdiag_tests.cpp`); mGBA, github.com/mgba-emu/mgba at
  `fd9dc5c` (`cinema/`, `src/platform/test/cinema-main.c`); remill,
  github.com/lifting-bits/remill at `56918a8` (`tests/X86/`).

## Reproducing

The state comparison (about 10 s; uses a built Emerald crate, here lane
1's from 2026-10-23; any `gbaa-<game>` from `just build-recompiled` will
do):

```sh
B=target/recompiled/release/gbaa-pokemon-emerald-version
R="$HOME/ROMs/gba/Pokemon - Emerald Version.gba"
cap=(systemd-run --user --scope -q -p MemoryMax=3G -p MemorySwapMax=0)
for n in 300 600 1500; do
  "${cap[@]}" $B "$R" --headless --frames $n --hash --state-out rec-$n.state --state-at $n
  "${cap[@]}" $B "$R" --headless --frames $n --hash --state-out int-$n.state --state-at $n --interp
  cmp -l rec-$n.state int-$n.state | wc -l   # minus up to 4: the header's payload CRC
done
```

Payload offsets, from `runtime::snap`'s field order (header 24 bytes): r0–r14
24–83, `pc` 84, `cpsr` 88, `banks` 92–259 (IRQ bank's r14 at 228),
`spsr` 260–283, EWRAM from 284, IWRAM from 262,428, then palette, VRAM,
OAM (ends 395,547), then the save memory, I/O, DMA, sound, BIOS state,
PPU, scheduler and prefetch.

`just sst-ir`, for the random-block numbers.
