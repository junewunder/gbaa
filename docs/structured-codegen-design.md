# Structured codegen that still matches exactly

Phase 2 of the plan of 2026-10-21 ([open-decisions.md](open-decisions.md)):
how `crates/codegen` goes from v1's `loop { match pc }`
([codegen-design.md](codegen-design.md)) to `if`s and loops, then to guest
registers in Rust locals, with the frame hash still the interpreter's at
every checkpoint. Written 2026-10-24 on quack (unit `structured/design`)
from the four literature reviews:

- [structured-control-flow.md](research/structured-control-flow.md):
  Ramsey, irreducible regions, multiple entries;
- [static-recompilers.md](research/static-recompilers.md): flags, ticks,
  IRQs, indirect jumps in other tools;
- [registers-and-types.md](research/registers-and-types.md): locals, ABI
  summaries, types;
- [codegen-validation.md](research/codegen-validation.md): how to know a
  transformation stayed exact.

This doc says what to build and in what order. The reviews say why, with
their sources; this doc cites them by their item numbers ("CF 3" is item 3
of structured-control-flow.md's "What our design should take", "SR", "RT"
and "VA" the others). It settles open decisions 8 and 10 where the reviews
decide them ([decisions.md](decisions.md)).

## Summary

- **Every step keeps v1's edges.** Structuring changes only which Rust code
  runs the next block, never what a block or an edge does: each edge keeps
  its flag writes, its cycles, its `rt::tick` and its yield `return`
  (CF 1, SR 4). Nothing merges ticks, moves a yield point, re-evaluates a
  condition or copies a block.
- **The structured body runs only from the function's entry, so the
  stack stops unwinding.** Our hard problem is multiple entries: after an
  unwind every caller is re-entered mid-function (a resume, CF 3), 9.5
  times a frame in Emerald, and structured code can't be. june decided
  (2026-10-24): **one body per function, and three runtime changes landed
  in v1 form before the structured emitter**, so resumes become rare
  rather than interpreted: calls settled where they're made, IRQs and
  halts taken inside the tick, and the guest on its own stack so the frame
  end pauses it instead of unwinding. A spike measured the three together
  on Emerald: 1.0 resume a frame, compiled share 99.2% as in v1, every
  frame hash equal. The resume or so left a frame is interpreted to the
  next entry. Interpreting all resumes, two bodies, async codegen and
  nightly coroutines were weighed and rejected (step 8).
- **Reducible functions get Ramsey's translation; irreducible regions stay
  `loop { match pc }`** as one collapsed node (CF 1, 2). 99.9% of Emerald's
  functions and 99.5% of modern GCC's are reducible.
- **Registers and NZCV move into function locals with no ABI trust** (RT
  2): write back at every point in the registers review's table, reload
  after the ones that write. LLVM's SROA does the promotion. Decision 10's
  summaries only trim call write-backs, so they wait for a measurement.
- **Two exact fixes come first, in v1 form:** drop register writes
  overwritten in the same block (45% of Emerald's register writes, RT 1),
  and compute the flag writes liveness dropped on each tick's yield path,
  so the CPSR an IRQ handler saves is the interpreter's (SR 2).
- **Validation is one narrow check per transformation** (VA 1–7):
  - a structure check at recompile time;
  - v1 as the reference, since it has the same tick points;
  - per-function differential runs from recorded entry states;
  - liveness on against off for the yield-flags fix;
  - multi-instruction `sst-gen`.

  No SMT.
- **Compile cost:** on p07 the front end is 4.7 s of a 60 s build; LLVM is
  the rest. Structuring and locals change what LLVM sees, which no source
  measured for our shape, so every step measures p07 and the whole Emerald
  build before and after. The one prediction that can be made: one body
  per function adds no code, and two would double it.

## What we start from

v1, as built ([codegen-design.md](codegen-design.md), decisions 8, 9 and
11): one Rust fn per guest function, `fn(&mut M, pc: u32) -> Flow`, whose
body is `loop { match pc }` over its blocks. All guest state is in
`Machine` at every block boundary; every block start is a dispatch key;
every edge ends `pc = K; if rt::tick(m, n) { return Flow::Jump(pc) }
continue;`. Calls to known functions are Rust calls with the `match` as the
guard on the return; indirect and tail calls unwind to the dispatcher.

Emerald's `WaitForVBlank` (`f_080008ad`, 3 blocks), trimmed:

```rust
pub fn f_080008ad(m: &mut M, mut pc: u32) -> Flow {
    loop {
        match pc {
            0x0800_08ad => {
                // push {lr}; ldr r2, =gMain; ... cmp r0, #0
                if !v51 {
                    let cycles: u32 = 19 + rt::fetch(m, 8, true, 6, 8);
                    pc = 0x0800_08d1;
                    if rt::tick(m, cycles) { return Flow::Jump(pc); }
                    continue;
                }
                m.cpu.r[3] = 1;
                let cycles: u32 = 20 + rt::fetch(m, 8, true, 7, 8);
                pc = 0x0800_08c7;
                if rt::tick(m, cycles) { return Flow::Jump(pc); }
                continue;
            }
            0x0800_08c7 => {
                // ldrh r1, [r2, #28]; ...; beq 0x080008c6: the spin
                if v27 {
                    let cycles: u32 = 8 + rt::fetch(m, 8, true, 5, 2);
                    pc = 0x0800_08c7;
                    if rt::tick(m, cycles) { return Flow::Jump(pc); }
                    continue;
                }
                let cycles: u32 = 8 + rt::fetch(m, 8, true, 5, 2);
                pc = 0x0800_08d1;
                if rt::tick(m, cycles) { return Flow::Jump(pc); }
                continue;
            }
            0x0800_08d1 => {
                // pop {r0}; bx r0
                pc = rt::exit(m, 0x0800_08d2, true, Exit::Exchange(v6));
                let _ = rt::tick(m, cycles);
                return Flow::Jump(pc);
            }
            _ => return Flow::Jump(pc),
        }
    }
}
```

Numbers today (Emerald, `out/pokemon-emerald-version` as of 2026-10-23,
after the lean printer):

| | Emerald |
|---|---|
| Guest instructions in blocks | 814,601 in 112,843 blocks, 15,158 functions |
| Generated Rust | 286 MiB, 6.7 M lines, 42 part crates; 8.2 lines per guest instruction |
| Register writes / reads | 687,497 / 196,705; 307,395 writes overwritten in the same block (RT, "Measured") |
| `rt::tick` (one per CFG edge) | 153,229 |
| Part p07 (19,880 instructions, 153k lines) | release build 60 s, 740 MB peak; `cargo check` (front end only) 4.7 s, 466 MB |
| Whole crate | 7 min 19 s under an 8 GB cap before the lean printer; 7.2 GB peak with 6 jobs |

The p07 numbers were measured for this doc (2026-10-24, quack, rustc
1.99.1, the generated crate's release profile: opt-level 3, 16 codegen
units, `incremental = false`; `/usr/bin/time` over `cargo build -p
gbaa-pokemon-emerald-version-p07` after touching its `lib.rs`). Before the
lean printer p07 took 1.13 GB and 56 s. So the lean printer cut the front
end's memory, and the time is LLVM's: about 55 of the 60 seconds. That
matters for what the steps below can win: statements the front end no
longer sees save memory, but time only comes from giving LLVM less or
simpler IR.

## Measured: how often functions are resumed

Whether the structured form needs a second, resumable body depends on how
often a function is entered past its entry. `Stats` now counts it
(`runtime::dispatch`, `Code::is_entry`: a key is an entry if it is its
function's lowest block, true for agbcc's functions), and the desktop
prints it with the dispatcher calls.

On the Emerald new-game route (`just unit emerald-newgame`, 5,400 frames,
to the truck):

| | v1 | Resumes interpreted (spike) |
|---|---|---|
| Dispatcher calls | 178,108 (33 a frame) | 162,343 |
| Of them resuming a function past its entry | 51,166 (29%; 9.5 a frame) | 0 |
| Calls that came back yielding (IRQ, frame end) | 10,729 (2 a frame) | 5,295 |
| Instructions interpreted | 2,751,755 | 30,443,154 |
| Guest cycles in compiled code | 99.2% | 93.1% |
| Every checkpoint (4) | recompiled = interpreted | recompiled = interpreted |

The spike made the dispatcher treat every ROM key that isn't an entry as
absent, so the interpreter ran from each resume point until it reached a
function entry. That is exactly what a structured-only build with an
entries-only table would do. Frames stayed exact; the interpreter took 6
more points of the guest cycles. On 3,000 frames of the intro, headless,
the run took 2.0–2.2 s in v1 and 2.6–3.6 s with resumes interpreted (two
runs each; the machine was shared). The desktop already runs about 70×
real time, so this costs headroom, not playability; on the ESP32 it would
matter.

What causes resumes: an unwind returns every compiled frame on the host
stack to the dispatcher, and each is re-entered at its return site. This
doc first guessed that most unwinds were yields (IRQs, the frame end).
The guest-stack spike showed most are not: they cascade from calls that
come back somewhere other than their return site, nearly all in the m4a
sound engine's hand-written code (SoundMainRAM's overlay chains in IWRAM,
and `MidiKeyToFreq` calling `__umul3232H32` through a Thumb-to-ARM stub).
Taking IRQs and the frame end without unwinding removed only 22% of the
resumes; settling those calls in place removed the rest.

The spike (2026-10-24, quack; branch `spike/guest-stack` on origin, not
for merging: environment-variable switches and the `&mut Machine` shortcut
step 7 describes) ran steps 5–7 below on v1 code, on the same route:

| | v1 | Guest stack, IRQs in the tick | Plus calls settled in place |
|---|---|---|---|
| Dispatcher calls | 178,108 | 167,136 | 132,379 |
| Resumes | 51,166 (9.5 a frame) | 40,199 (7.4) | 5,442 (1.0) |
| Calls that came back yielding | 10,729 | 0 | 0 |
| Guest cycles in compiled code | 99.2% | 99.2% | 99.2% |
| All 5,400 frame hashes and final registers | | = v1 | = v1 |

The intro's 3,000 frames went 35,526, 29,586, 3,040 resumes, all five
checkpoints matching. Rewinding (`--rewind 3000+40`, which drops the
suspended guest 40 times) gave v1's hashes, and a save state taken at
frame 2000 while the guest was suspended was byte-identical to v1's. What
is left: `SoundMain_4`, entered once a frame from interpreted IWRAM code,
and 45 `LZ77UnComp*` entries after a `swi`: the interpreter reaching a
compiled key that isn't an entry. Under one body it runs a few
instructions to the next entry.

## The steps

In order. Each is exact on its own, keeps the frame hash the interpreter's
at every checkpoint of every route, and is measured before the next one
starts. A function a step can't handle keeps the previous step's form,
counted in `report.txt`.

### 1. Drop register writes overwritten in the same block

(RT 1; unit `structured/dead-reg-writes`.) The IR builder already forwards
register values inside a block. It should also emit only each register's
last write before the next point where something can read `cpu.r`: an
exit, a call, a `swi`, an `msr`/`mrs`, a user-bank op, or anything in the
registers review's table ("Where state must be in `Machine`"). Nothing
between two writes can read the first, so dropping it is exact. It removes
307,395 of Emerald's 687,497 register writes, about 8% of the statements.
LLVM already deletes some of them where the bus's fast path inlines, so the
gain is mostly front-end memory.

Checks: `just sst-gen`, `just sst-ir`, every route, the CPU suites. Measure
p07 and the whole Emerald build.

As built (2026-10-24): `Block::remove_dead_reg_writes`
(`analysis/src/ir/regs.rs`), run by `build_block` after DCE and by
`Block::optimize`, so `sst-ir`'s random 2–6-opcode blocks check it;
`Config::dead_reg_writes` turns it off for `fn-diff`. Barriers: every
exit (guarded or not), `GetReg` of that register, the user-bank ops,
`msr`, `mrs`. A guarded write kills only an earlier write under the same
guard. Measured on quack the same session, the pass off and on (rustc
1.99.1, 6 jobs, the other lane busy, so times are ±a few seconds):

| Emerald | Before | After |
|---|---|---|
| Register writes `m.cpu.r[i] = ` | 687,497 | 379,932 (−307,565) |
| Generated lines, bytes | 6,675,381, 286 MiB | 6,445,065 (−3.5%), 276 MiB |
| `gbaa-recompile` | 58 s, 632 MB | 55 s, 594 MB |
| p07 release build (touch `lib.rs`) | 23.0 s, 771 MB | 22.7 / 24.5 s, 735 MB |
| Whole crate, all parts and the binary (touched, deps built) | 255 s, peak 4.8 GB | 246 s, peak 5.0 GB |

So front-end memory went down ~5% per part, as expected, and time didn't
move: LLVM was already deleting most of these stores. The whole build's
peak is six rustcs at once, so it moves with scheduling more than with
code. (p07's 23 s against the 60 s above is the machine, not code: the
same build measured today both ways.)

### 2. Exact flags on the yield path

(SR 2, VA 5; unit `structured/yield-flags`.) Flag liveness follows a
function's static edges, but an IRQ can be taken at any tick, and Emerald's
`IntrMain` saves the interrupted CPSR to the IRQ stack in IWRAM. A flag
write liveness dropped on that edge leaves a stale flag in memory. The
frame hash can't see it; a memory compare can. The fix: on each tick's
yield branch, before `return`, compute and write the flag values liveness
dropped on that edge:

```rust
if rt::tick(m, cycles) {
    m.cpu.cpsr = m.cpu.cpsr & 0x3fff_ffff | (n as u32) << 31 | (z as u32) << 30;
    return Flow::Jump(pc);
}
```

The printer must keep the dropped values computable there (they are pure
values of the block). The branch is cold, so it costs code size, not
time; measure how much. Step 9 keeps it: with flags in locals, the
yield path's write-back writes the same exact CPSR.

Check: `gbaa-recompile` gets a switch to turn flag liveness off. Liveness
off and on have the same tick points, so their exact save states
(`--state-out`) at the route checkpoints must be byte-identical. Before the
fix they should differ in the CPSR saved on the IRQ stack in IWRAM (the
first IRQ after a dropped flag write); after it, not at all. (VA's
"Measured" compares the interpreter with compiled code, which differ for
timing reasons too; liveness on against off is the comparison that
isolates this.)

As built (2026-10-24):

- The builder (asked by codegen when liveness is on) puts an
  `Op::YieldFlag(f, v)` before each exit for every flag it has a value for
  there, guarded writes already folded into a `BSelect`. Liveness ignores
  them; after it deletes its dead writes, a forward walk keeps a block's
  `YieldFlag(f)` only where the last write of `f` before the exit was
  deleted, and DCE drops the rest. The lowerer prints the kept ones as one
  CPSR write inside the exit's `if rt::tick(..) { … return }`; their values
  are used only there (the printer may inline a pure one into it, never a
  read or a load).
- That alone isn't exact: a flag dead on an edge into a block that yields
  before writing it is stale there too, and that block has no value for
  it. So liveness keeps a flag live on an edge into a block unless the
  block writes it, unguarded, before its first exit. A guarded `msr cpsr`
  with the flags field reads all four (the builder forwards no flag
  through it).
- Emerald: of 1,213,974 flag writes, liveness kept 281,680 before and keeps
  287,952 now (the edge rule's 6,272); 45,936 exits' yield paths write
  141,807 flags. The generated source grows 2.9% (296.3 to 305.1 MB, with
  liveness off 346.8 MB), the binary 3.2% (120.3 to 124.2 MB).
- Checks: `just fn-diff` with `--exact`, liveness on against off: 0 of
  6,380 runs differ (50 before). `just liveness-states ROM [--every N]
  ROUTE…` (`route-check --same-states`, `docs/tools/commands.md`) builds
  both forms and compares their exact states. At the 10 checkpoints of
  Emerald's intro and new-game routes they matched even before the fix: a
  frame boundary rarely follows an IRQ taken at a stale flag. Every 50th
  frame of the new-game route found one: before, frame 950 differed in 7
  bytes (the CPSR's flags byte, a banked SPSR's and one IWRAM byte, with the
  payload's CRC); after, none of 108 frames differ.

### 3. Each function's CFG, its loops and regions

(CF "Measured"; unit `structured/cfg-stats`.) Move what `gbaa-cfg-survey`
computes into a `codegen` module the structurer uses:

- the function's CFG over codegen's blocks and `func::successors`;
- dominators (Cooper–Harvey–Kennedy);
- the loop nest, Steensgaard-style;
- reducibility;
- each irreducible region, as the set of blocks collapsed into one node;
- the reverse postorder Ramsey's translation walks.

The one block per modern-GCC ROM that codegen builds and `successors`
doesn't reach (Anim_FourPetal's `0x0816135f` in pokeemerald_modern) must
be explained. A block no edge reaches can't be placed in a structured
body, so it can only be reached through the dispatcher, which under the
default means the interpreter.

**Done** (2026-10-24): `codegen::cfg` (`Cfg::of`, `Shape::of`; the
irreducible regions are `Shape::irreducible`, the loop nest
`Shape::regions` with each region's parent, entries and exits). Tarjan is
iterative now, so it needs no big stack at recompile time. `gbaa-cfg-survey`
runs on it and prints byte-for-byte what it did before on all five ROMs
below. Tests on hand-built CFGs: `cargo test -p codegen cfg::`.

The unreached block: in Anim_FourPetal, `movs r3, #0` falls into the
loop head `0x0816127e`, which ends in a jump table discovery leaves
unresolved (modern GCC checks the bound on `r3 & 0xffff` in r1 and indexes
with r3). `build_func` first built `0x0816126b` through the `mov pc, r3`
whole; const-prop knew r3 = 0, folded the table load from ROM and made the
jump a static edge to case 0, `0x0816135f`, which was built. Then the back
edge from `0x081612b0` split the block at `0x0816127f`, and the rebuilt
jump has r3 unknown: an unresolved exit, no successors. So `0x0816135f`
stays a key (the dispatcher can resume there) but no edge reaches it. The
same function is the one such block in Rogue (`0x08184e83`) and Imperium
(`0x081b90a3`), 0x106 past its entry each time. Cases 1–3 aren't in the
function at all: they run interpreted until the table is resolved. For the
structurer this block is unreachable code to leave out, as `Cfg` does.

Measured (`just cfg-survey`, 2026-10-24, quack; 20–65 s and 300–350 MB a
ROM):

| | Emerald | Rogue | Imperium | pokeemerald_modern |
|---|---|---|---|---|
| Functions / blocks | 15,158 / 112,843 | 13,366 / 135,421 | 13,765 / 136,502 | 12,435 / 110,492 |
| Irreducible functions | 13 (0.1%) | 71 (0.5%) | 63 (0.5%) | 61 (0.5%) |
| Irreducible regions; blocks in the outermost | 13; 49 | 85; 696 | 69; 495 | 77; 515 |
| Their instructions (of all) | 422 (0.1%) | 4,709 (0.5%) | 3,379 (0.4%) | 3,515 (0.5%) |
| Loops and regions; with 2+ exit targets | 4,351; 17.4% | 4,673; 35.4% | 4,860; 36.1% | 4,372; 34.9% |
| Deepest loop nest | 4 | 5 | 5 | 5 |
| Return sites (of blocks) | 47.9% | 43.7% | 44.0% | 45.1% |
| Jump tables; targets inside; largest | 659; 6,455; 69 | 644; 6,929; 198 | 711; 7,126; 187 | 608; 6,347; 64 |
| Ramsey nesting median / p90 / p99 / max | 1 / 3 / 8 / 37 | 1 / 4 / 12 / 185 | 1 / 4 / 11 / 82 | 1 / 4 / 10 / 53 |
| Blocks no edge reaches | 0 | 1 | 1 | 1 |

Rogue's 185 is `0x0801b4f1` (7,447 instructions, 1,509 blocks, 2 loops),
its third largest; its largest (9,114, 1,414 blocks) nests 46, Imperium's
(10,260, 1,529) 30. Splitting every irreducible region instead of collapsing it would copy
at least 529 / 5,362 / 4,069 / 6,287 instructions.

### 4. The structuring plan and its check

(CF 1, 2, 4, 5; VA 2; unit `structured/ramsey-plan`.) A pure function
from a function's blocks and CFG to a plan, a tree of:

- `Block(b)`: block `b`'s code, printed as now;
- `Loop(label, body)`: a loop header and what it dominates inside the loop;
- `Labeled(label, body)`: the labeled block Ramsey puts around a merge
  node's predecessors;
- `If(guard, then, else)`, for a two-way exit to non-merge children;
- `Match(table, arms, fallback)`, for a resolved jump table, a case
  target only the switch reaches inline in its arm (CF "Switches");
- `Region(entries, blocks)`: a collapsed irreducible region, printed as v1's
  `loop { match pc }` over its blocks;
- and each edge as one of `Break(label)`, `Continue(label)`, `Next`
  (falls into the following plan node) or `Return` (leaves the function,
  as now).

This is Ramsey's "Beyond Relooper" over the dominator tree, with Cheerp's
`if` for non-merge children. Its property: every block and every edge of
the original has exactly one counterpart, so no code is copied and no
condition is evaluated twice.

The **structure check** (CompCert's Linearize pattern, VA 2) runs on every
plan at recompile time:

- every block appears exactly once, with its IR, its `insn!` markers, its
  overlay guard, its `rt::tick_ram` and its patched-version choice
  unchanged;
- every edge reaches its original successor, with its own flag writes,
  cycles, tick and yield `return`;
- every jump table keeps its `_` arm to the dispatcher;
- every block is reachable from the entry in the plan (else the function
  stays v1).

A function whose plan fails the check is emitted in v1 form and counted in
`report.txt`. The check and the planner are unit-tested on hand-built CFGs
(reducible, irreducible, several loop exits, switches with shared targets,
self loops) in `cargo test -p codegen`. Report the counts over Emerald,
Rogue and Imperium: functions planned, regions collapsed, nesting depth
(CF's estimate was p99 8–12, max 185 on one Rogue function).

**Done** (2026-10-24): `codegen::plan` (`plan::of`, `plan::plan`,
`plan::check`). As built:

- The plan's edges are per exit and target: a block's `Out`s are its
  exits in order, each an `Edge` per target (`Break`, `Continue`, `Next`,
  `Inline`, and `Goto`/`Leave` inside a region) and a `fallback` flag for
  its `return` to the dispatcher. The design's `If` is a conditional exit
  whose edge is `Inline`; its `Match` an exit with several targets or a
  fallback. `Next` is a `break` that ends the labeled block it leaves.
- A merge node is one with two or more forward *edges* in, not
  predecessors: agbcc's `if (x) f();` is `beq` over a `bl` whose return
  site is the `beq`'s target, two edges from one block, so the target
  needs a labeled block. That is why step 3's estimate is below the plan
  for 13–18% of functions; it is above it for 6–11%, because a merge
  node's code follows its own labeled block rather than sitting in it.
- The check covers what the plan holds: every block once, the plan
  starting at the entry, each exit's targets in order and its fallback,
  every `break`/`continue` to a label in scope whose code starts at the
  edge's own target, `Next` only where falling off reaches it, regions
  entered only at their entries and left only to an arm. Each block's IR,
  markers, guards and versions aren't in the plan; keeping them is the
  printer's (step 8).
- Tests on hand-built CFGs (the design's `WaitForVBlank`, a diamond,
  self loops, nested loops with several exits, a switch with shared
  targets, irreducible regions alone and in loops, a broken plan of each
  kind the check must catch): `cargo test -p codegen plan::`.

Measured (`just cfg-survey`, 2026-10-24, quack; the plan adds under a
second a ROM):

| | Emerald | Rogue | Imperium |
|---|---|---|---|
| Functions planned (and checked) | 15,158 of 15,158 | 13,366 of 13,366 | 13,765 of 13,765 |
| With collapsed regions; regions; their blocks | 13; 13; 49 | 71; 76; 696 | 63; 67; 495 |
| Still irreducible once collapsed; failed the check | 0; 0 | 0; 0 | 0; 0 |
| Plan nesting (loops, labeled blocks, regions) median / p90 / p99 / max | 1 / 3 / 7 / 31 | 1 / 4 / 9 / 175 | 1 / 4 / 9 / 52 |
| Printed depth with `if`/`match` arms, median / p90 / p99 / max | 1 / 5 / 11 / 37 | 2 / 6 / 12 / 177 | 2 / 6 / 13 / 53 |

The deepest printed are `0x0814e765` in Emerald (37) and Rogue's `0x0801b4f1` (nesting 175,
where step 3 estimated 185). Rust nests that deep
fine, but rustc's front end recurses on it; if step 8 finds it a problem,
a function past a nesting limit can stay v1.

### 5. Calls settled where they're made

(SR 5; unit `structured/call-trampoline`; in v1 form, before step 8.) A
call whose callee comes back with a key other than its return site
returns that key upward today, and every compiled frame above it unwinds
and later resumes. That happens after a tail call (`b other_fn`), an
unresolved indirect call, a call through a stub, IWRAM overlay code, and
code the interpreter ran. Instead, the call site settles it:

```rust
let sp0 = m.cpu.r[13];
let Flow::Jump(a) = f_x(m, KEY);
let a = if a != RET { rt::settle(m, a, RET, sp0) } else { a };
if a != RET || rt::yielding(m) { return Flow::Jump(a); }
```

`rt::settle` runs the dispatcher nested at the call site, stopping when
the guest reaches `RET` with the caller's sp and mode, and returns where it
stopped. That is wider than this step first said (calling only function
entries the table has): the spike found most resumes come from the sound
engine's overlay and stub calls, which a table-only settle misses. The same
blocks run in the same order with the same ticks; only the host stack
differs. A nested run that strands host frames (a guest returning
somewhere else) is bounded by step 7's depth cap. Keep one helper, so the
printer emits one call per call site.

Check: every route, the CPU suites; resumes per frame, compiled share and
frame time before and after. The spike's version is `rt::settle` in
`crates/runtime/src/dispatch.rs` on its branch.

Done (2026-10-24, quack), without the guest stack: `rt::settle` in
`runtime::dispatch` runs the dispatcher's loop (`run_inner`) nested, with
`stop_at` guarded on sp and mode (bits 0-4 of the CPSR; the key holds T).
Each call site (`Plan::Call`, `Plan::CallVia`, and an unresolved indirect
call, `Plan::Dynamic` with a return site) takes `sp0` before the call and
calls the generated `settle(m, a, RET, sp0)`, which passes the crate's
`CODE`; then `if rt::yielding(m) { return }` and `continue` as before. The
nested run takes IRQs and halts as the outer one does, so an IRQ during a
settled call doesn't unwind either; the frame end and a fault stop it
where they happen and the caller unwinds with that key. It doesn't start
when the frame has ended, the CPU is halted or a fault is set, nor past
`MAX_DEPTH` (64) nested runs. The run's `Stats` reach a nested run through
a pointer in `Machine::nest`, set for the run's length (a copy in the
`Machine` overflowed a debug test's stack), and a call's compiled cycles
leave out what its nested runs counted. The desktop prints `settled: N
calls … (M left elsewhere and unwound), nested D deep at most`.

| Emerald, headless | v1 | Calls settled in place |
|---|---|---|
| New game (5,400 frames): dispatcher calls | 178,108 | 143,431 |
| Resumes | 51,166 (9.5 a frame) | 16,489 (3.1) |
| Calls settled (left elsewhere) | | 24,310 (320) |
| Calls that came back yielding | 10,729 | 10,742 |
| Guest cycles in compiled code | 99.2% | 99.2% (the same cycles) |
| Run time, 3 runs | 2.64, 3.11, 3.74 s | 2.75, 2.82, 2.67 s |
| Intro (3,000 frames): resumes | 35,526 (11.8) | 9,123 (3.0) |
| Calls settled (left elsewhere) | | 16,924 (165) |
| Run time, 3 runs | 1.94, 2.76, 2.24 s | 1.94, 1.86, 2.28 s |

Nesting reached depth 2. The run times are the same within the noise (the
machine was shared with another lane's build). What is left is step 6's:
each IRQ or frame end that unwinds the stack resumes every frame on it.
Every route (`units/*.toml`, ten) passes, recompiled = interpreted.

### 6. IRQs and halts inside the tick

(SR 3; unit `structured/nested-irq`; in v1 form, before step 8.) When a
tick's slow path finds an IRQ takeable (or the CPU halted), run it in
place, as gbarecomp does: write `cpu.pc` = the edge's key, take the IRQ,
and run the dispatcher nested (`stop_at` = that key, guarded on sp and
mode) until the handler returns there. Then the tick reports no yield and
compiled code goes on. If the nested run stops anywhere else (a handler
returned elsewhere, a fault, the depth cap), the tick reports a yield and
the stack unwinds as now. The IRQ is taken at the same tick with the same
state, so this is exact. The printed tick needs the edge's key
(`rt::tick(m, cycles, KEY)`), which the yield return already has. The
frame end is step 7.

Check: every route, `own-state-check`, `rewind-check`, the CPU suites;
resumes per frame, compiled share, frame time and the deepest host stack
before and after (risks.md asks for the stack anyway). The spike's version
is `rt::yield_at`.

Done (2026-10-25, quack), without the guest stack: every tick's yield
path (after its flags) calls the generated `yield_at(m, pc)`, which passes
the crate's `CODE` to `rt::yield_at`; `Some(k)` returns `Flow::Jump(k)`,
`None` goes on. That covers every exit's tick: static and dynamic jumps,
returns (`bx lr`, so the caller's call site doesn't unwind either), calls
before they're made, `swi`s that halt, and unresolved indirect calls.
`rt::yield_at` takes nothing when the frame has ended, a fault is set or
the nesting is at `MAX_DEPTH` (shared with `rt::settle`), and unwinds
then; otherwise it sets `cpu.pc` to the key and runs `run_inner` nested,
stopping at that key guarded on sp and mode (the same guard and counting
as `rt::settle`, through one helper). A nested run the frame end or a
fault stops elsewhere unwinds with the key it stopped at. The desktop's
`settled:` line adds `N IRQs and halts taken in the tick (M unwound)` and
the host stack's depth: `Machine::sync`, the tick's slow path, samples the
stack pointer against the outermost run's start (a sample at every
scanline event, not a bound).

| Emerald, headless | Calls settled (step 5) | IRQs in the tick too |
|---|---|---|
| New game (5,400 frames): dispatcher calls | 143,431 | 138,069 |
| Resumes | 16,489 (3.1 a frame) | 11,128 (2.1) |
| Calls that came back yielding | 10,742 | 5,400 (the frame ends) |
| IRQs and halts taken in the tick (unwound) | | 5,361 (0) |
| Guest cycles in compiled code | 99.2% | 99.2% (the same cycles) |
| Deepest host stack seen, nesting | 4,333 bytes, 2 | 4,397 bytes, 2 |
| Run time, 3 runs alternating | 3.91, 3.89, 2.67 s | 3.89, 3.87, 2.71 s |
| Intro (3,000 frames): resumes | 9,123 (3.0) | 6,167 (2.1) |
| Calls that came back yielding | 5,955 | 3,001 |
| Run time, 3 runs alternating | 2.75, 2.75, 1.82 s | 2.12, 1.82, 1.91 s |

Both builds drew the same frame-5400 and frame-3000 hashes; the "before"
column is this build with `rt::yield_at` unwinding at once, so the same
code but the call. Run time is the same within the noise (another lane was
building). Every call that came back yielding is now a frame end, one a
frame; the 2.1 resumes a frame left are what it unwinds, the compiled
frames on the stack at the frame end, re-entered after it: step 7's.
The host stack grew 64 bytes, a `yield_at` frame under the tick.

Every route (`units/*.toml`, ten) passes, recompiled = interpreted. In the
two hacks most IRQs and halts taken in the tick unwind (Imperium 6,603 of
6,736, Rogue 5,494 of 5,952): they wait in `VBlankIntrWait` (Rogue's run
is 66% halted), and a halt's nested run idles into the V-blank, which is
the frame end. Those unwound in v1 too; step 7 takes them. Their deepest
host stack seen: 4,397 and 5,437 bytes, nesting 3 and 4.

### 7. The guest on its own stack

(Unit `structured/guest-stack`; in v1 form, before step 8.) The frame end
is the yield left every frame: the frontend runs between frames
(rendering, input, audio, Lua, save states, rewind). Instead of unwinding
to reach it, the guest (the dispatcher and compiled code) runs on its own
stack, and the tick that finds the frame over writes `cpu.pc` = the edge's
key and switches to the frontend's stack. When the frontend switches back,
the tick reports no yield and compiled code goes on. A stackful coroutine
(`corosensei` on the desktop) lives in the platform crate; the runtime gets
only a function-pointer hook, so it stays `no_std`.

- **`&mut Machine`.** Own the Machine through a raw pointer
  (`Box::into_raw`). While the guest is alive the frontend touches it only
  through the pointer the guest hands out at the switch (a reborrow of its
  innermost `&mut`), and only until it resumes; the same for `Stats` and
  the dispatcher's context. The spike skipped this (its frontend kept its
  own `Box`, which is UB under Stacked and Tree Borrows).
- **State load and rewind** drop the suspended guest (`force_reset`; its
  frames have no destructors), write through the owner pointer, and start
  the guest again from `cpu.pc`: one resume each. Under one body that
  resume, and any never-returning frame above it (AgbMain's loop), runs
  interpreted until it reaches a function entry; its callees are compiled.
  Accepted.
- **Frontend work stays off the guest stack.** In the window build
  `present_frame` (SDL and GPU drivers) runs inside the tick today; it
  moves to the frontend side of the switch.
- **Guests that return somewhere else** keep a nested run dispatching, so
  they stay exact but strand host frames (jamulator's "RTI tricks"). Steps
  5–7 share a nesting-depth cap; past it, unwind as today and let the
  interpreter take it. Not seen in Emerald.
- **Stack size.** The spike's high-water mark was 5 KiB at nesting depth 3
  over 5,400 frames (8 KiB ran; 4 KiB hit the guard page). Measure on every
  route, size it with a wide margin and keep a guard page.
- **Platforms.** `corosensei` covers x86_64 and AArch64 on Linux, macOS and
  Windows (the desktop and the Deck), and builds for
  `riscv32imac-unknown-none-elf` with a static stack (the ESP32-P4).
  `riscv32imc` (ESP32-C3) didn't build. Xtensa (ESP32-S3) isn't supported:
  a FreeRTOS task or a hand-written switch of about 30 lines, when the ESP32
  work gets there. It doesn't steer this design.
- **Cost:** 1–2 ns a round trip, about once a frame.

Check: every route of every ROM, the CPU suites, `own-state-check`,
`rewind-check`; resumes per frame (about 1 expected), compiled share, frame
time, the deepest host stack and nesting before and after. The spike's
version is `crates/desktop/src/coro.rs`.

Done (2026-10-25, quack): `crates/desktop/src/coro.rs` runs
`dispatch::run_guest` in a `corosensei` coroutine on a 1 MiB stack (a
guard page below it), the default in a recompiled binary; `--unwind`
keeps v1's way. The runtime has only the hook, `Nest::frame_end`
(`fn(*mut ())`, the machine): with it set, the V-blank hands off the
sound but leaves presenting and the keys to the front end
(`Machine::show_frame`, called after the switch, so SDL never runs on
the guest's stack; no guest instruction runs in between), `yield_at`
switches at the frame end and then takes the V-blank IRQ in the tick,
and the dispatcher loop switches where it finds the frame over (after
the interpreter or a halt's idle) and goes on. The desktop owns the
machine and the `Stats` through `Box::into_raw`; its frame loop uses the
pointer `Guest::frame` hands out until it resumes the guest. A rewind
step, any save-state slot action and a fault drop the guest
(`force_reset`, then `Nest::forget_run`); the next frame starts a new one
from `cpu.pc`. The compiled share is now the guest's cycles less the
interpreter's and the halts' (`guest_counts`): the outermost compiled
frames never return to count theirs. `MAX_DEPTH` still unwinds, to the
nearest dispatcher loop, which switches.

| Emerald, headless | IRQs in the tick (step 6, `--unwind`) | Guest stack |
|---|---|---|
| New game (5,400 frames): dispatcher calls | 138,069 | 132,379 |
| Resumes | 11,128 (2.1 a frame) | 5,442 (1.0) |
| Calls that came back yielding | 5,400 | 0 |
| Calls settled (left elsewhere) | 24,291 (320) | 24,004 (0) |
| IRQs and halts in the tick (unwound) | 5,361 (0) | 10,728 (0) |
| Guest cycles in compiled code | 99.2% | 99.2% |
| Deepest host stack sampled, nesting | 4,397 bytes, 2 | 6,989 bytes, 3 |
| Guest stack written (high-water mark) | | 7,728 bytes |
| Run time, 3 runs alternating | 3.89, 3.88, 3.27 s | 3.14, 2.81, 2.64 s |
| Intro (3,000 frames): resumes | 6,167 (2.1) | 3,040 (1.0) |
| Run time, 3 runs alternating | 2.72, 1.83, 2.82 s | 2.36, 2.76, 1.92 s |

Both draw the same frame-5400 and frame-3000 hashes. Run times are noisy
(load average 5, another lane building); the new game is about 20%
faster, the intro the same within the noise. The IRQs in the tick
doubled because the V-blank's is now taken there too. The host stack
sample now includes the coroutine's own frames and a level more nesting
(the V-blank IRQ under the tick that found the frame end).

The resume left, once a frame, is m4a's `SoundMain` (0x082df05c) resumed
at 0x082df09f, nesting 3 deep (inside the V-blank IRQ's handler): it
jumps to `SoundMainRAM` in IWRAM with a `bx` codegen doesn't see as a
call, and that returns into the middle of `SoundMain`. Step 8 runs that
resume interpreted unless the call is settled. The two hacks resume
almost never (Imperium 89 in 6,800 frames, Rogue 51 in 6,000).

Every route (`units/*.toml`, ten) passes, recompiled = interpreted.
Deepest guest stack written per route: Rogue 10,240 bytes (nesting 5),
Imperium 9,904 (4), armwrestler 9,120, Emerald 7,728, the small ROMs
4,624 to 6,736: the 1 MiB stack has a margin of 100 times. Also passing:
`own-state-check` and `rewind-check` on Emerald (1000, from reset) and
Rogue (1000), `armwrestler` (both builds, recompiled too), `gba-tests`,
`sst`, and a runtime test that a guest run sees the state an unwinding
run stops with at every frame end.

### 8. Emit the plan

(CF 3; unit `structured/emit-structured`; after steps 5–7.) Print each plan as Rust:
`loop`, `'b: { … break 'b }`, `if`, `match`. The function keeps its
`GuestFn` signature, so the dispatch table and `rt::call` don't change, but
it is only ever called at its entry. `WaitForVBlank` becomes:

```rust
pub fn f_080008ad(m: &mut M, _: u32) -> Flow {
    'b08d1: {
        // block 0x080008ad
        if !v51 {
            let cycles: u32 = 19 + rt::fetch(m, 8, true, 6, 8);
            if rt::tick(m, cycles) { return Flow::Jump(0x0800_08d1); }
            break 'b08d1;
        }
        m.cpu.r[3] = 1;
        let cycles: u32 = 20 + rt::fetch(m, 8, true, 7, 8);
        if rt::tick(m, cycles) { return Flow::Jump(0x0800_08c7); }
        loop {
            // block 0x080008c7
            if v27 {
                let cycles: u32 = 8 + rt::fetch(m, 8, true, 5, 2);
                if rt::tick(m, cycles) { return Flow::Jump(0x0800_08c7); }
                continue;
            }
            let cycles: u32 = 8 + rt::fetch(m, 8, true, 5, 2);
            if rt::tick(m, cycles) { return Flow::Jump(0x0800_08d1); }
            break 'b08d1;
        }
    }
    // block 0x080008d1
    let pc = rt::exit(m, 0x0800_08d2, true, Exit::Exchange(v6));
    let _ = rt::tick(m, cycles);
    Flow::Jump(pc)
}
```

What changes and what doesn't:

- **Edges.** `pc = K; continue` becomes the plan's `break`, `continue` or
  fall-through. The yield `return` carries the constant key. Each edge's
  code is otherwise v1's, statement for statement.
- **Calls.** v1's guard was the `match`. Structured code compares the
  returned key with the return site, after step 5's settle:
  `if a != RET || rt::yielding(m) { return Flow::Jump(a); }`, then the
  return site's block follows. A callee that still returned elsewhere
  unwinds, as v1's `_` arm does for any key it doesn't own (v1 also
  continued locally when the key was another of the caller's blocks;
  structured code can't, and doesn't need to).
- **The dispatch table holds function entries only** (and every overlay
  block, below). Return sites, jump-table targets and post-tick block
  starts aren't keys any more, so after one of the rare unwinds steps 5–7
  leave (about one a frame) the interpreter runs from the resume point
  until it reaches a function entry, where compiled code takes over: the
  resumes-interpreted spike above, measured exact. Decision 6's seeds
  must not fill up with resume keys (in the spike the 256 slots filled):
  the fallback records a ROM key as a seed only if it isn't a block of a
  compiled function, which needs the compiled block starts as a second,
  lookup-only table.
- **Irreducible regions** are `Region` nodes: `let mut pc = ENTRY;
  loop { match pc { … } }` over their blocks, set to the entry the edge
  into the region takes, leaving through a `break` to a labeled block
  after which a `match pc` picks the exit successor. `pc` is a local, so
  this re-tests nothing in guest state.
- **Overlays stay v1** (CF, "What exact asks"): RAM functions keep a guard
  per block and patched versions; about a hundred blocks in Emerald. Their
  blocks stay keys, so their resumes stay compiled.
- **Functions that fail the check stay v1**, and their blocks stay keys.

Checks: every Emerald, Rogue and Imperium checkpoint; `sst-gen`,
`gba-tests`, `armwrestler`, `first-frame`, `recompile-check`; the
structure check passes on every function it emits. Measure, before and
after: generated size, p07 and whole-build time and peak memory, frame
time on a fixed route, dispatcher counts and the compiled share. With
steps 5–7 in, the share should stay at v1's (99.2% on the new-game route).

Done (2026-10-25, quack), the printer and the table; the full check and
measurements are their own unit (`structured/emit-measure`):

- `codegen::lower` prints a function from its plan when
  `lower::plan_or_why` gives one: a ROM function (not an overlay) whose
  plan passed the structure check, every block reached from the entry,
  every block ending in one unconditional exit (the fall-throughs rely on
  it), printed nesting at most `MAX_INDENT` (64). Each block's code is
  v1's (`lower_block_then`); only what v1 ended with `continue` becomes the
  edge: `break 'bN`, `continue 'lN`, nothing (a fall-through), the
  target's code inline, or `match pc` over the exit's edges with `_ =>
  return Flow::Jump(pc)` (a call's settle, a table, a `swi`, a rekey).
  `pc` stays a local, set as in v1, so a region is v1's `'rN: loop {
  match pc }` and a goto in it `continue 'rN`. `Config::structured` turns
  it off (`gbaa-recompile --no-structured`; `fn-diff`'s `v1` form).
- The table (`emit::keys`): every function's entry, and every block of
  the functions still in v1 form; the structured functions' other block
  starts go in `Code::inner`, lookup-only, with their extents. The
  fallback doesn't seed them, counts a run that starts at or runs into one
  as a resume, and the block-timed interpreter runs them as blocks.
- `just snapshots` writes `mods/emerald/snapshots/emerald.rs`: the
  smallest Emerald function of each shape (calls, an if, a loop in a
  labeled block, a loop in a loop, a jump table, a region).
- `just sst-gen` had been broken since step 6 (its prelude had no
  `yield_at`); it has one that unwinds now.

| Emerald | v1 (`--no-structured`) | Structured |
|---|---|---|
| Functions printed structured | | 15,158 of 15,158 (Rogue 13,363 of 13,366: 2 too deep, 1 with an unreached block; Imperium 13,764 of 13,765) |
| Dispatch keys (inner block starts) | 112,843 | 15,158 (97,685) |
| Generated parts | 312 MB, 6.85 M lines | 413 MB, 6.93 M lines; without leading spaces 192.3 MB both (+0.02%) |
| Whole build, 6 jobs | 294 s, 352 s | 255 s |
| p07 alone, release | 29.2 s, 24.3 s; 0.74–0.77 GB | 21.7 s, 21.6 s; 0.77 GB |
| New game, headless: dispatcher calls | 132,379 | 132,334 |
| Resumes | 5,442 (calls past an entry) | 20,459 (fallback runs into an inner block) |
| Instructions interpreted | 2,751,755 | 2,773,388 (+0.8%) |
| Guest cycles in compiled code | 99.2% | 99.2% (108k more interpreted of 1.5 G) |
| Run time, one run each | 3.98 s | 3.91 s |

(quack, 2026-10-25, the machine shared with another lane; the whole
builds' peak is the summed rustc RSS sampled each second, 5.0–5.3 GB both
ways, a sample that may miss the top.) The extra bytes are indentation
only. LLVM got simpler IR: p07 builds about 15% faster.

Passing with it: `sst-gen` (22,500 cases, 4,000 sequences, 0 differ), the
codegen and runtime tests, and the four Pokémon routes (Emerald new game
and intro, Rogue, Imperium), recompiled = interpreted at every
checkpoint. Compiled share: Emerald new game 99.2% (v1's), intro 98.8%,
Rogue 97.8%, Imperium 96.5% (v1's for these three not yet measured).
The two resume counts aren't the same measure: v1's counts dispatcher
calls at a key past a function's entry, the structured build's every
interpreter run that reaches a structured function's inner block (where
v1 would stop at a key); the instructions interpreted are the comparable
number, and they barely moved.

Checked and measured against v1 (2026-10-25, quack; unit
`structured/emit-measure`). v1 is each ROM recompiled with
`--no-structured` into `out/<game>-v1`, run along the route with the
arguments `route-check --args` prints:

- Every route (`units/*.toml`, ten) passes with the structured build,
  recompiled = interpreted; `gba-tests`, `armwrestler` (both builds,
  interpreted and recompiled) and `first-frame` (gba-cart1) pass.
- On the four Pokémon routes v1 and structured draw the same hash at every
  frame and end with the same registers. Their exact states
  (`route-check --same-states V1 STRUCTURED --every 100`) are
  byte-identical at every 100th frame and checkpoint of both Emerald routes
  (92 states). In Rogue (4 of 64) and Imperium (1 of 69) they differ in
  only two things the interpreter keeps and compiled code doesn't:
  `Bus::open_bus` (the last opcode the interpreter fetched, `0x47704770`
  against `0x7fff7fff`) and `Machine::prefetch`, its queue (used only by
  a step at the clock it was filled). The structured build interprets a
  few more instructions (resumes run to the next entry), so the
  interpreter last ran somewhere else. Neither is in the CPU or memory,
  and v1 isn't exact in `open_bus` either: compiled code never updates it.

| | Emerald intro | Emerald new game | Rogue | Imperium |
|---|---|---|---|---|
| Compiled share, v1 / structured | 98.8% / 98.8% | 99.2% / 99.2% | 97.8% / 97.8% | 96.5% / 96.5% |
| Instructions interpreted, v1 / structured | 2,671,990 / 2,687,231 | 2,751,755 / 2,773,388 | 2,539,521 / 2,539,572 | 4,192,869 / 4,196,138 |
| Dispatcher calls, v1 / structured | 103,927 / 103,874 | 132,379 / 132,334 | 133,334 / 133,283 | 176,363 / 175,638 |
| Resumes (as above), v1 / structured | 3,850 / 15,411 | 5,442 / 20,459 | 51 / 17,297 | 89 / 43,999 |
| Run time, median of runs (v1 / structured) | 3.2 / 2.6 s (2 each) | 3.0 / 3.1 s (5 each) | 4.0 / 4.6 s (2 each) | 4.6 / 4.0 s (5 each) |
| Peak RSS of the run, v1 / structured | 37 / 33 MB | 52 / 47 MB | 74 / 68 MB | 68 / 61 MB |

Run times swing 2.6–4.4 s for the same binary (the other lane was
building), so they are the same within the noise. RSS is lower by the
dispatch table's 100k fewer keys.

| | Emerald | Rogue | Imperium |
|---|---|---|---|
| Dispatch keys, v1 / structured | 112,843 / 15,158 | 135,358 / 15,480 | 136,413 / 13,777 |
| Generated source without leading spaces, v1 / structured | 192.3 MB both | 212.71 / 212.64 MB | 223.37 / 223.29 MB |
| Binary, v1 / structured | 133.8 / 128.1 MB (−4.2%) | 146.1 / 139.2 MB (−4.7%) | 153.4 / 146.7 MB (−4.4%) |

Rogue's whole build (every part touched, 6 jobs, one after the other):
v1 422 s, structured 351 s, the largest process 2.8 GB both ways (the
link). The other lane was running a job through both (load 6.4 at the
first's start, 4.4 at the second's), so the 17% is an upper bound;
Emerald's above, under the same kind of load: 255 s against v1's 294 and 352.

**Weighed and rejected (june, 2026-10-24):**

- *Resumes interpreted, won back by the first versions of steps 5 and 6
  after the emitter* (the default this doc first proposed): the share
  drops 6 points meanwhile, and they would not have won it back. The
  frame end would still unwind, and a settle that only calls table entries
  misses the sound engine's overlay and stub calls, where most resumes
  come from.
- *Two bodies* (structured at the entry, v1's `loop { match }` for every
  other key): no runtime change, but every function's code twice, the
  thing phase 1 fought, and a confusing base for the readable output
  decision 2 looks toward.
- *Async codegen* (every function an `async fn`, every tick and call an
  `.await`, our own executor; measured in
  [async-codegen-spike.md](research/async-codegen-spike.md)): exact, and sound under Miri if
  generated code re-derives `&mut M` from a raw pointer after every
  `.await`. But p07 converted to it built in +5% time with +55% peak
  memory (1.16 GB against 0.73 GB) and 60× the metadata. 69% of Emerald's
  53,500 call sites cross part crates, where a caller can't name its
  callee's future, so they go through a type-erased frame arena at 1.4–4.5×
  v1's cost a call in a toy. And resumes come back after every state load
  and non-local return. Its one advantage: no second stack, so it runs on
  any ESP32.
- *Nightly coroutines with resume arguments*: the same lowering as async.
  Handing `&mut M` out at each `yield` and back at `resume` compiles with
  no unsafe, but stable async can do the same through a `Cell` slot, and
  `Coroutine` has no stabilization in sight (same file).

### 9. Registers and NZCV as function locals

(RT 2, 3, 7; VA 6; unit `structured/regs-locals`.) Each function holds
the guest registers it reads or writes, and N, Z, C and V, in Rust locals:

- load them from `m.cpu` at the entry (the only key under one body);
- write back exactly at the points in the registers review's table
  ("Where state must be in `Machine`"), and reload after those that write:
  a call, `rt::exit`, `rt::swi`, `msr`/`mrs` and user-bank ops, a failing
  overlay guard, the yield path of every tick, the nested runs of steps 5
  and 6, and step 7's switch to the frontend;
- write back only the registers the function writes, never the ones it
  only reads (the spike's one waste);
- after a call that returns yielding, return without writing back (the
  caller's locals are stale; the callee's state is in `Machine`);
- constant indices or named locals only, never a computed register index,
  so SROA promotes;
- the I bit, the mode and the T bit stay in `cpu.cpsr`.

The table becomes one function in the printer that every runtime call goes
through, so a new `rt::` helper can't be added without saying what it
reads and writes (QEMU's helper flags). No register write is dropped across
a tick, even a dead one: the IRQ entry pushes r0–r3, r12 and lr, and C
handlers push the rest, so every register's value at a yield can reach
memory. With locals that costs only the cold write-back.

Before building: spike p07 three ways, v1, step 1, steps 1 and 9 (RT 4).
Measure rustc's time and peak memory, the object size, and the frame time
of a fixed route; the frame hashes must match all three ways. If locals
make LLVM slower than they make the game faster, stop and say so: the
desktop doesn't need the speed, and compile cost is phase 1's concern.

The check, beside the routes and the CPU suites: a printer-level check that
every runtime call has the write-back its row requires (Rideau and Leroy's
register-allocation validator, run backwards, VA 6), at recompile time, a
function that fails staying in `Machine` form; the per-function
differential runs (below) with locals on against off; a save state taken
at a frame end round-trips (`own-state-check`).

**The spike (2026-10-25, quack, unit `structured/regs-locals`): stopped.
Locals cost far more build than they win, so they are built but off**
(`Config::locals`, `gbaa-recompile --locals`):

- `codegen::locals` holds each function's registers (the ones it reads or
  writes) and N, Z, C, V in locals, loaded at its entry. `Locals::sync`
  is the table as one function: every runtime call the printer makes that
  can read or write the CPU goes through it with its row (`Point`: a
  tick's yield path, a call into guest code, `msr` and the user-bank ops,
  a read outside the locals, a return, a return after a call came back
  yielding). The write-back (only the written registers, and the flags if
  any is written) and the reload print as `wb!()` and `reload!()`, two
  `macro_rules!` each function defines once.
- `locals::check` is the printer-level check: a dataflow walk over the
  printed function (`syn`) that tracks which locals are written since
  `Machine` got them and which are stale since a call; it fails a call
  with a dirty local, a use of a stale one, a return with a dirty one, a
  `Machine` access to a held register outside the macros, a wrong macro
  body, and any call the table has no row for. A function that fails is
  printed in `Machine` form; `report.txt` counts them ("registers in
  locals"). Emerald: none fail. A unit test drops each write-back in turn.
- With locals on, `sst-gen` passed (22,500 cases and 4,000 sequences, 0
  differ) and the new-game route drew the same frame 5400 hash with the
  same dispatcher, settle and IRQ counts as without.

| Emerald, three ways (structured) | Neither | Dead register writes (default) | And locals |
|---|---|---|---|
| Generated parts | 427 MB, 7.18 M lines | 414 MB, 6.94 M lines | 442 MB, 7.99 M lines (12.9 M before the macros) |
| `gbaa-recompile` | 64 s | 63 s | 77–84 s (the check parses every function) |
| p07 alone, release, two builds each | 25.1 s, 24.4 s; 0.76–0.79 GB | 24.0 s, 23.7 s; 0.75–0.76 GB | 38.7 s, 37.5 s; 1.40–1.43 GB |
| p07's rlib | 3.80 MB | 3.75 MB | 5.48 MB |
| Whole build | 260 s, 6 jobs | 243 s, 6 jobs | killed at an 8 GB cap with 6 jobs; with 4, 384 s and LLVM crashed on p37 (below) |
| New game to frame 5400, headless, three runs each | 2.63–3.00 s | 2.65–3.16 s | 2.57–3.55 s |

(quack, shared with another lane, so the run times are noise around 2.7 s:
locals bought nothing measurable. The run includes the interpreter
fallback, the PPU and sound, not only compiled code.) Why it costs: each
of Emerald's 153,000 yield paths writes back and reloads about 10.6
locals (207,000 reloads, 277,500 write-backs in all), so every tick is a
merge point for every held register, and LLVM's SSA graphs grow with
them. LLVM 23.1 (rustc 1.99.1) crashes in `SLPVectorizerPass::
vectorizeStoreChains` on part p37 in this form at any `RUST_MIN_STACK`;
p37 at `opt-level = "s"` (no SLP vectorizer) builds, which is how the run
times above were taken.

What could still make it pay, if the ESP32 ever needs the speed: hold
locals only in functions with no tick inside a loop, or only in the hot
ones a profile names. The object grows 46% either way the yield paths are
printed, which the ESP32's flash would feel too.

**Settled (june, 2026-10-26):** keep it, off by default behind `--locals`.
It is the play view's answer; a future mod view may turn it on per
function. So the routes, CPU suites and own-state-check with locals on
are owed by whoever turns it on, not by this step.

## Validation

Beside each step's own checks above:

- **The reference for a codegen transformation is v1's output, not the
  interpreter** (VA 1). The interpreter ticks per instruction and compiled
  code per block, so their states differ at every IRQ even when the frames
  match (VA, "Measured": 25 to 983 bytes of the exact state at frames 300
  to 1500 of Emerald). v1 and every transformed form share tick points, so
  their states must be identical at every tick. The interpreter stays the
  reference for instruction semantics (`sst`, `sst-ir`, `sst-gen`) and
  for frames (routes).
- **Per-function differential runs from real entry states** (VA 4; unit
  `structured/validation`):
  - record `Machine` at function entries during an interpreted route run,
    a sample per function;
  - mutate each recorded state: all 16 NZCV values, randomised dead state,
    perturbed live values;
  - run the function in two configurations from that state to its return,
    comparing the full state, the ordered bus trace and the list of ticks
    with their cycle counts.

  The configurations: liveness on and off, steps 1, 8 and 9 each on and
  off. One configuration that alone fails names the transformation. This is
  finer than a frame and needs no route; it is the cheap check the reviews
  agree on. It runs from a `just` recipe listed in
  `docs/tools/commands.md`; it needs a build that carries both forms of a
  chosen set of functions.

  As built (2026-10-24): `just fn-diff ROM`, with a `codegen::func::Config`
  switch per transformation (`flag_liveness` so far). Runs stop at the
  function's return or its first yield; at a yield the CPSR's flags are
  excused unless `--exact`. On Emerald liveness on against off differed
  in no run but 50 of 6,380, and those only in the flags at a yield; since
  step 2, in none, with `--exact`.
- **`sst-gen` on sequences** (VA 7; unit `structured/sst-sequences`): random
  2–6-instruction blocks and short multi-block functions with branches
  through generated Rust, against the interpreter, comparing the timing
  calls (`rt::tick`, `rt::fetch`, `rt::stall`, the timing ops) against
  `armv4t::cycles::cost` as well as registers and the bus trace. The
  generators exist (`sst-ir`); nothing below a ROM tests the printer on
  sequences today.

  As built (2026-10-24): `just sst-gen [PER_FILE] [SEQS]` (entry in
  `docs/tools/commands.md`). The cases are built by `build_func` from a
  made-up ROM image, so they get the timing ops, ROM const-prop and flag
  liveness across their blocks; the reference counts `armv4t::cycles`
  beside the interpreter as `Machine::step` does, ending a tick where the
  built blocks end. Breaking any one timing helper (`fetch`, `skipped`,
  `multiply`, `block`, `stall`) or swapping S and N in `lower::cost_expr`
  fails it. At 4000 cases (15,543 ticks) none differs; 12 functions differ
  only in the CPSR's flags at a yield, excused as in `fn-diff` until
  step 2.
- **A block-timed interpreter mode** (VA 1; unit
  `structured/block-timed-interp`, not needed by phase 2's gate): the
  fallback steps a compiled block's instructions without ticking, then
  ticks once with the block's cost, as compiled code does. It makes the
  interpreter a full-state reference for codegen too, the only one that
  can catch a bug v1 and a transformed form share.

  As built (2026-10-24): `--interp-blocks` in a recompiled binary
  (`runtime::dispatch::run_blocks`), checked by `just block-timed-states
  ROM [--every N] ROUTE…` (`docs/tools/commands.md`). Matching exactly
  took pricing as compiled code prices: a block's fetch wait states as of
  its exit (Emerald's `WAITCNT` store repriced its block by 4 cycles),
  and a failed condition's saving taken off the bus count. Each block's
  extent is its table owner's (`EXTENTS`); a block shared by functions
  whose stops split it differently would tick apart, which Emerald never
  showed. Emerald's two routes, every 50th frame: 184 of 184 states
  byte-identical. A transformation that moves ticks must keep `EXTENTS`
  where its ticks are.
- **Snapshots** of a handful of Emerald functions of each shape (a loop, a
  switch, an irreducible region, calls), printed and kept in the repo, so
  every change to generated code shows in review (VA 9). Part of
  `structured/emit-structured`.
- **Not now:** full-state co-simulation at every tick with page hashes
  (VA 3) waits for a divergence the differential runs can't narrow; SMT
  on blocks (VA 8) for a block rewrite random testing can't reach.

## What it should do to compile time and memory

What can be said before building, and what each step measures:

| Step | Expected | Why | Measured by |
|---|---|---|---|
| 1. dead register writes | front-end memory down ~8% of statements; LLVM time down a little | 307k fewer statements; LLVM already removes some | p07 and the whole build, the unit |
| 2. yield-path flags | code up, on cold paths | a few flag writes per edge whose flags were dropped | statements before and after |
| 8. structured, one body | about the same statements; LLVM time unknown | the `match` arms and `pc = K; continue` go; real loops and labeled blocks instead of one dispatch loop. LLVM's loop passes may run on real loops, and its jump threading no longer has to (CF "Does LLVM already do it?" shows it only partly does) | p07, the whole build, frame time |
| 5, 6, 7 | small; nothing for 7 | a settle at each call site that needs one; a key per tick; the switch is runtime-only | p07, resumes per frame |
| 9. locals | front end about the same; LLVM unknown (Instrew: +12% compile time, −14% run time) | memory traffic becomes φs; LLVM no longer reloads `cpu.r` after every opaque call | the three-way p07 spike first. Measured: p07 +58% time, +87% memory, +46% object, no run-time gain; off (step 9) |

The part crates stay until the code is lean enough to merge them; how many
Emerald needs is for the steps' measurements, not this doc.

## Left out, and why

- **Node splitting** for irreducible regions: copies 529 instructions on
  Emerald, 16% of moonbeam, exponential at worst (CF 2).
- **Decompilers' structurers** (DREAM, SAILR, rev.ng's combing, Phoenix):
  they re-evaluate conditions or copy code for readability (CF 4).
- **`#[loop_match]`**: nightly and incomplete; revisit when it stabilises,
  since it would speed v1's form with no structuring (CF 7).
- **Merging ticks or yielding only at back edges**: not exact for us
  (SR 4); only as an experiment the frame hash judges.
- **Decision 10's summaries now**: with locals, a summary only trims a
  call's write-back and reload (about a dozen host loads and stores at
  each of 53,500 call sites); build them only if step 9's spike shows calls
  matter (RT 5).
- **Stack slots in locals, type recovery**: the stack is guest memory, and
  types buy nothing exact (RT 6).
- **QEMU's split NZCV layout**: a separate change to `Cpu`, only if a
  profile shows the CPSR updates (SR 6).
- **A direct-indexed key table**: the dispatcher runs 33 times a frame;
  its binary search isn't the cost.

## Units

The group "Structured codegen that matches exactly", after this design.
Each needs the ones before it on its line; the gate needs all but the
block-timed interpreter.

| Unit | Step | Needs |
|---|---|---|
| `structured/dead-reg-writes` | 1 | this design |
| `structured/yield-flags` | 2 | this design |
| `structured/cfg-stats` | 3 | this design |
| `structured/ramsey-plan` | 4 | cfg-stats |
| `structured/call-trampoline` | 5 | this design |
| `structured/nested-irq` | 6 | call-trampoline (both change call and tick printing) |
| `structured/guest-stack` | 7 | nested-irq |
| `structured/emit-structured` | 8 | ramsey-plan, guest-stack |
| `structured/emit-measure` | 8's check and measurements | emit-structured |
| `structured/regs-locals` | 9 | dead-reg-writes, yield-flags, emit-structured, validation |
| `structured/validation` | differential runs | this design |
| `structured/sst-sequences` | `sst-gen` on sequences | this design |
| `structured/block-timed-interp` | interpreter as full-state reference | this design; not before the gate |
| `structured/more-roms-gate` | june looks | regs-locals, validation, sst-sequences |
