---
title: Emerald's function-address flow: the memory fix (Ref tokens) and speed-ups
hook: a large read is one Ref token; five slow spots; don't absorb to the object end
---

Settled history. Where the flow stands now, and how to measure and profile
it: `fnref-flow-emerald-memory.md`.

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

**Review fix (2026-11-13).** A kept `Exp` capped each store's tokens onto
its capped value, so its ROM pointers widened by store order and could
leave out a pointer the capped memory read (the rerun test's `snap`)
already took in. `exp_merge` keeps numbers and pointers uncapped
(`Exp::raw`) and caps them all together: the value is the capped read's
again. The agb programs' metrics didn't move, Emerald's time neither.
