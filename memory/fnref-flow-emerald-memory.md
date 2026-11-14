---
title: Why Emerald's function-address flow doesn't settle yet
hook: not pointers stepping; big sets gain forms of the same 5,157 origins a round; saturating them (tried 2026-11-14) made it worse
open: true
---

How it got here (where the memory went, the Ref fix, the first speed-ups):
`fnref-flow-emerald-history.md`.

**How to measure** (quack). `--callee-sets` is on main since 2026-11-08
(indirect-call-args landed): `just long start … --mem 9G -- bash -c
'target/release/gbaa-recompile --summaries ROM --callee-sets …'` (copy the
binary out first if you'll rebuild while it runs: "Text file busy"). Run it as
`just long start … --mem 9G -- bash -c '… > /tmp/x.out 2> /tmp/x.log'`
(write to files: a piped grep loses its buffer when the cap kills it).
`GBAA_FNREF_TRACE=` (empty) still turns tracing on: use `env -u`.
A progress print every 500 functions (RSS from /proc/self/statm, token counts
of entries, rets, cells, by Rc pointer) found each source below fast.

**Profiling (2026-11-12, quack).** `perf` can't open events here (paranoid
2, and SELinux); `eu-stack -p PID` works (ptrace_scope 0). Build a copy
with `CARGO_TARGET_DIR=/tmp/x CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`,
run it, and every 0.4 s append `eu-stack -p $pid | head -40` to a file;
count frames (demangle with `c++filt`), and for inlined hot spots
`addr2line -e BIN -i -p` on the frame address less the load base
(`main`'s printed address less `nm BIN | grep ' T main$'`). 60-100
samples found each slow spot (the history note's list and the ones below).

**2026-11-13, after merging main (rom-read-bound and later):** the same
command, `just long start NAME --mem 8G`: 11:16 wall (flow 610.8 s), 2.44
GB peak, exit 0; 14:55 (flow 807 s) on an earlier build with four other
workers' jobs running. The first line now gives the stop: `didn't settle:
48096722 tokens held, over its bound, after 27776 runs`; 0 of 553 sets
complete. Ask for 8G, not 16G: on a busy night a 16G job never gets its
turn (three waited over an hour while 4G jobs passed them). The
native-tasks bar went 57 to 59 by june's call
(d/raise-native-tasks-unproven-function); the reason is beside the bar.

**What the 48M tokens were (2026-11-13, `native/fnref-flow-emerald`).**
Copies. Counted by content (the trace's `held` lines, every 4,096
functions run: sets by pointer, and those not equal to one before), the
run that stopped at the bound held 44M tokens in 86,000 sets, 3M of them
distinct: one 3,409-token set 685 times in entries, results and calls by
value. Three makers of copies: a join into a set others share makes a new
one (two entries reaching the same tokens by different joins); a join
whose additions the cap drops again (a ROM pointer under a "from here
on") copied the set and left it equal; and `all_holed` kept 845 of a
function's own large sets alive (9M tokens). Now: `Sets` finds a kept set
by content (weakly held; `Rc::make_mut` moves a changed one, so the table
is never stale), `join_capped` asks `takes_in` first, `all_holed` holds
weak pointers, and `G::held` counts the caches it missed (11M).

**Speed, in the order found** (eu-stack, as above; each exact: the agb
metrics and Emerald's per-round counts didn't move). Ops on large values
kept by the operands' kept sets (`G::op_kept`: shift, `mvn`, binary ops,
compares, `expand`); a call by value joining each distinct result set
once (`Once`: 6,000 callees return a few sets); a cell's token logged the
first time any cell gets it (`cells_all`), and `exp_update` making each
read's gain once. Round 0: 88 s to 31 s. Round 1 ends at 177 s (never
before), round 2 at 536 s; rounds 3 to 5 at 1,079, 1,629 and 2,194 s with
`just lift-check` running beside them.

**Where it stands.** It doesn't settle in seven rounds (stopped by hand
at 53 minutes, in round 7): every round reruns 15,200 functions, and
12,300 entries change in each though memory has all but stopped (pile
16,920, 23,841, 24,309, 24,574, 24,632, 24,632 tokens after rounds 1 to
6, unchanged in round 6; holes 9,414 and taken 6,664 from round 2 on).
Held tokens are flat (24M after round 3, 22M in round 7), peak RSS 1.5
GB: the bound is no longer what stops it. Next: (1) find what 12,300
entries gain a round with memory still (print an entry's new tokens
under the trace). Not checked, the first suspect: ROM pointers stepping
down. `bin_tok` makes `RomFrom(a) - n` a `RomFrom(a - n)` and `Rom(a) -
n` a lower `Rom`, the cap only takes in pointers above a "from here on",
and arithmetic on functions' addresses (holes) leaves `Rom`s all over the
code, each its own run of data: a pointer walked down through calls would
move a step a round through every entry the big callee sets share, with
no end before the 256 rounds. (2) rerun a function by the `Ref` values
it looked at, not by pile width; (3) left in
the round-2 profile: `St::join`'s cap at every edge (17%), shift
`split_refs` misses when the Refs' values change (17%), a load's
`join_capped` per pointer (8%). The op memo's results aren't `share`d
(one 1,426-token set 91 times in the cache): cheap to add.

**Checking a flow change is exact (2026-11-13).** Metroid settles, so it
shows what Emerald can't: put `git show origin/main:…/fnref.rs` in the
tree, build with another `CARGO_TARGET_DIR`, copy the binary out,
restore, and run both with the trace. This branch against main: the same
output, 20 rounds, the same end counts and hole addresses; 15.1 s against
42.7 s, 173k tokens held against 2.06M. The review found `join_capped`
inexact where pointers a "from here on" covers push the cap past its 16
exact pointers (fixed; the test now compares each case with `join`), and
the op memo wrong under `GBAA_FNREF_OFF=move` (off under any of them).

**What the entries gain (2026-11-14, `GBAA_FNREF_GAINS=1`).** Not ROM
pointers stepping down: no ROM read "from here on" is new after round 2
(3,700, 3,152, 66, then 0), and only 5 ever start below one of their
object's. The origins all entries and results hold between them stop at
5,153 after round 1 (5,157 later); their function tokens keep growing
(12.4k, 14.9k, ...): the large sets (13-15k tokens; 17,900 registers over
4,096 after round 1, 38,500 after round 2) each gain other *forms* of
addresses already there (Lo, LoUp, Part, Cmp) and refs other sets already
hold, a step a round, and every entry sharing one changes. Memory still
grows too (pile 22.4k, 23.5k, 24.4k after rounds 2-4), so every round
reruns ~14.5k functions.

**Tried, didn't help (code not kept; `/tmp` copies gone).** (1) Running a
function dirtied mid-round in the same round when it comes later (order
alternating up and down by round): Metroid's output identical (27 rounds
against 23, same time), Emerald's rounds no shorter (r1 170 s, r2 406, r3
718, r4 1,063). (2) Saturating: an entry or result over 4,096 tokens takes
every form of every origin large ones hold: each became its own 32k-token
set (different refs and ROM pointers beside), held tokens headed for the
48M bound in round 2. (3) Making them all one shared set (every token any
holds, capped, forms closed): Metroid identical (largest set 664 tokens),
but on Emerald the shared set dragged its ROM pointers into every large
value: 9,251 new ROM reads in round 2, pile 56k, round 2 took 1,226 s, 47.5M
tokens held. So the next try must not hand ROM pointers and refs around:
close forms only, or collapse a large value's function tokens to a mark
("every function's every form") the ops and holes know.
