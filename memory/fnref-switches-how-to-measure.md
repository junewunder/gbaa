---
title: Measuring and profiling the function flow's switches
hook: run off and on side by side; eu-stack -i on a line-table build; an empty VAR= is set
---

Where the pieces stand is the tracker's (`just find function flow`) and
`docs/research/fnref-held-words.md`. This is how to measure one (2026-11-20,
the function-flow thread).

**The switches** (`crates/codegen/src/native/fnref.rs`): all three pieces
are on by default since 2026-11-21, and each `=0` turns its piece off, for
measuring against the flow as it was: `GBAA_FNREF_LIVE=0` (registers at a
call, and calls with no callee yet wait), `GBAA_FNREF_RAM=0` (RAM pointers
in a closed world; the allocation sites go with them), `GBAA_FNREF_HEAP=0`
(allocation sites alone). The lift bars are the defaults': with a switch
off some are over. The RAM rule for unseen
callers and the heap's gain on c/heap both need LIVE on too: measure RAM and
HEAP with it.

**Measure in the lift's own run too** (`native/fnref-ram-lift-cost`): the
flow alone on a ROM (`gbaa-recompile --summaries`) has no holes file and
no names. WarioWare and Emerald were at main's cost there and 3 and 2
times it in `just holes-all --flow`, from their holes files' RAM routines.
Run both reports side by side before calling a cost fixed.

**A change behind a switch leaves the other setting as it was.** `just
lift-metrics` prints "0 over their bars, 0 under" by default. Metroid: lift
with main's binary and yours (same switches) with `GBAA_FNREF_TRACE=1` and
`diff` the `hole at` and `fnref round` lines with the seconds cut out
(`sed -E 's/round ([0-9]+): [0-9.]+m?s;/round \1:/'`). Copy each binary out
of `target/` first: a rebuild under a running job is "Text file busy".

**A piece off.** `GBAA_FNREF_RAM=0 just lift-metrics` (the env reaches
`gbaa-lift` through the recipe's scope): the C programs'
`fn_words_not_proven` cells show `v>b!`. Metroid's lift alone, no build: 30 to
60 s, under `systemd-run --user --scope -q -p MemoryMax=3G`. Emerald:
`gbaa-recompile --summaries ROM --callee-sets [--allocators K,K]` under `just
long start NAME --mem 4G`, 9 to 25 minutes.

**Times are only comparable side by side.** Four lanes share the machine: the
same Metroid flow took 25 to 40 s across an hour. Start the off run and the on
run in one command (`( … ) & ( … ) & wait`) and compare within the pair.

**zsh.** `env $x cmd` doesn't split `$x`; and `VAR=$empty cmd` sets VAR to the
empty string, which `std::env::var_os(..).is_some()` counts as set. Pass each
setting by name, and make a throwaway switch test non-empty.

**Profiling.** `perf record` works flat (no `-g`: `perf_event_paranoid` is 2),
and the hot symbols are B-tree and allocator internals with the flow's own
functions inlined into `Run::block`. What names the caller: build with
`CARGO_PROFILE_RELEASE_DEBUG=line-tables-only CARGO_TARGET_DIR=<scratch>` and
sample `eu-stack -i -p PID` every half second for a minute; `-i` prints
inlined frames (`add_all < split_refs < block`). `pgrep -A -n gbaa-recompile-`
finds the PID (process names are cut at 15 characters; the hook wants `-A`).
Per-op timers in `Run::block` (an `Instant` around each `Op`, summed by kind,
printed under the trace) found the first cause in ten minutes; take them out
after.

**What the costs were** (so they aren't found twice): each function's
list of read ranges pushed at every load of every pass (175 million entries
on Metroid with RAM on: `Rd::range` keeps each once); `split_refs` building
a set per Ref; a value holding hundreds of per-object reads (bounds
`OBJS_MAX`, `OBJS_READ`, `OBJ_REFS`); and the per-object copies of global
cells (`in_cell`) counted as "object cells", which reran every function that
read every object at each store to a global.

**What reran what** (`native/fnref-ram-reruns`, 2026-11-21): the trace's
"reruns for memory" line gives, per kind of read, the functions it reran
and those nothing else did. Read it before profiling: Emerald's 2.2 times
was three such lines (every object's cell, the stacks' and any-object
cells, the pile's Refs). With the RAM switch a function reruns by the value
a kept read gave (`Exp::ver`), an object cell's read is always a `Ref`, and
later functions a run changed run in the same round. The first two rounds'
cost was kept reads taking in each object's cell the pile holds (a counter
in `exp_add` by `Src` kind, tokens summed, found it in one Metroid run).
Emerald side by side now: 563 s off, 561 s on, 592 s with the heap switch
(644, 617 and 686 with another job on the machine: compare within a set).

**Landing.** Main moves during the ten minutes `just check` takes. Check
the branch merged with main, merge, and compare `git diff --stat <checked>
HEAD -- . ':!tracker'` before pushing; when only `tracker:` commits came in
the tree is the tested one.
