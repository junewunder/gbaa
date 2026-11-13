---
title: Why the function-address flow ran out of memory on Emerald, and the Ref fix
hook: the 48M tokens were copies (sets now kept once by content: 24M, 1.5 GB); 7 rounds in 53 min and still unsettled, 12k entries change a round; profile with eu-stack (perf is blocked)
open: true
---

**How to measure** (quack). `--callee-sets` is on main since 2026-11-08
(indirect-call-args landed): `just long start … --mem 9G -- bash -c
'target/release/gbaa-recompile --summaries ROM --callee-sets …'` (copy the
binary out first if you'll rebuild while it runs: "Text file busy"). Run it as
`just long start … --mem 9G -- bash -c '… > /tmp/x.out 2> /tmp/x.log'`
(write to files: a piped grep loses its buffer when the cap kills it).
`GBAA_FNREF_TRACE=` (empty) still turns tracing on: use `env -u`.
A progress print every 500 functions (RSS from /proc/self/statm, token counts
of entries, rets, cells, by Rc pointer) found each source below fast.

**Where the memory went, in the order found (2026-11-07/08).** Each was a
set of 10-30k tokens, made again at each store, that every function entry
joined into its own copy:
1. pile reads (a load through a heap pointer);
2. every-cell reads (a load from an unknown address);
3. compares of those (`Cmp` forms: 40% of what was left), and `lsl/lsr #16`
   u16 zero-extension (`LoUp`/`Lo` forms);
4. ROM table reads by an unknown index ("from here on" up to next code):
   struct tables holding function pointers, read by byte fields (`Part`);
5. words stored narrower (`strh`/`strb` copies) expanding into cells.

**The fix (b6d5ed3f).** A large read is one token, `Tok::Ref { src, xf }`:
`src` is Pile(w), Cells(w), RomFrom(lo, end, w) or Hull(lo, hi, w); `xf` is a
per-form map (shift, mvn, compare, narrower load or store composed with
`Xf::then`). Memory stores Refs; whoever looks at tokens expands
(`G::expand`/`ref_val`, cached until memory changes, ROM-only ones for
good) and so reads that memory (`rd` bits: reruns when it grows). Under 64
tokens a Ref is expanded where an op makes it, so the agb programs come
out as before. Entries at 8,500 functions: 5.6M tokens (135M at 7,000
before); RSS flat at 1.5 GB.

**Profiling (2026-11-12, quack).** `perf` can't open events here (paranoid
2, and SELinux); `eu-stack -p PID` works (ptrace_scope 0). Build a copy
with `CARGO_TARGET_DIR=/tmp/x CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`,
run it, and every 0.4 s append `eu-stack -p $pid | head -40` to a file;
count frames (demangle with `c++filt`), and for inlined hot spots
`addr2line -e BIN -i -p` on the frame address less the load base
(`main`'s printed address less `nm BIN | grep ' T main$'`). 60-100
samples found each of the below.

**What was slow, in the order fixed (native/fnref-emerald-speed):**
1. `ref_val`'s cache was cleared at every store; rebuilding 30k-token
   expansions was 65% of the time. Now each `Exp` keeps the reads it is
   made of and a position in `G::mem_log` (every token newly stored to the
   pile or a cell) and takes in only what was stored since. Memory is read
   uncapped (`pile_raw`/`cell_raw`): it holds no numbers, so the result
   matches the capped read but for the order ROM pointers widen in.
2. ROM reads "from here on" capped the set after each of up to 64k
   words: `rom_read_capped` skips tokens the cap would drop again.
3. One-at-a-time BTreeSet inserts of thousands of tokens (bin, shift, Adc
   compares, split_refs' rest, unions): `Toks::add_all` rebuilds from
   both sorted runs; `bin` and shift batch their per-token holes.
4. `expand` of several Refs and split_refs' rest are cached by set pointer.
5. Round 1: `cap` per callee's results joined (thousands of callees), and
   cap's absorb pass did a `bound()` + two range searches per RomFrom
   (results hold thousands): results now unioned then capped once; the
   absorb pass is one walk. Entries/results skip the cap when a join adds
   nothing (`join_capped`).

Results: round 0 was 4,352 functions in 6 min; now all 15,158 in 437 s
(stats identical: pile 10480, cells 3736, holes 9159). A whole run at an
intermediate build (before 4-5) finished: 27:37, 2.1 GB peak, but "didn't
settle" (35,409 runs, so not the 256-round bound: a function hit its
step bound, `64 * blocks + 64`; the trace now names it, "unsettled after").
Round 1 at the last build: 256 functions per ~60 s, its time spread over
ref_val/exp_add (new reads), add_all and call_by_value. lift-metrics on
the agb programs: identical to main's table at each step.

**2026-11-12 (later), at ec10365:** a whole run finishes: 11:24 wall, 2.37
GB peak, round 0 in 82 s, but stops unsettled at main's 48M-token bound
(`TOKENS`, 29,696 runs; entries, results, by_value and all_holed grow
together). Settling Emerald means raising or replacing that bound, not
more speed. The one bar over (native-tasks `fn_words_not_proven` 59 > 57)
is evaluation order: main proves vtable words 0x080244c8/cc only through an
early, short-lived 5-token value of sp at f_08000180's `pop`; the settled
values are wide RomFrom sets on both. `Val::cap` letting a RomFrom absorb
Roms up to the next code while `rom_from` reads only to the object end is
a real gap on main, but closing it (absorb to the object end) sent
ref_unexplained from 35 to 93 on every agb program: don't try it as a
quick fix.

**2026-11-13, after merging main (rom-read-bound and later):** the same
command, `just long start NAME --mem 8G`: 11:16 wall (flow 610.8 s), 2.44
GB peak, exit 0; 14:55 (flow 807 s) on an earlier build with four other
workers' jobs running. The first line now gives the stop: `didn't settle:
48096722 tokens held, over its bound, after 27776 runs`; 0 of 553 sets
complete. Ask for 8G, not 16G: on a busy night a 16G job never gets its
turn (three waited over an hour while 4G jobs passed them). The
native-tasks bar went 57 to 59 by june's call
(d/raise-native-tasks-unproven-function); the reason is beside the bar.

**Review fix (2026-11-13).** A kept `Exp` capped each store's tokens onto
its capped value, so its ROM pointers widened by store order and could
leave out a pointer the capped memory read (the rerun test's `snap`)
already took in. `exp_merge` keeps numbers and pointers uncapped
(`Exp::raw`) and caps them all together: the value is the capped read's
again. The agb programs' metrics didn't move, Emerald's time neither.

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
